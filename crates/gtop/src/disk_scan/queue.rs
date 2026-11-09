use super::*;
use gtop_disk::{
    scan::{scan_opened_with_progress, ScanOptions, ScanProgress},
    tree::{Node as DiskNode, NodeKind},
};
use std::sync::{mpsc, Arc};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Urgency {
    Background,
    Foreground,
}

const REPORT_INTERVAL: Duration = Duration::from_millis(100);

pub(super) fn run_with_options(
    root: &Path,
    home: &Path,
    directory: OwnedFd,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Tree),
    on_tick: &mut dyn FnMut() -> Urgency,
    options: ScanOptions,
) -> Tree {
    let mut tree = Tree::new(root);
    if let Ok(id) = identity(&directory) {
        tree.nodes[0].identity = id;
    }
    coordinate(tree, home, cancel, progress, on_tick, move |shared| {
        scan_opened_with_progress(root, directory, options, shared)
    })
}

fn coordinate(
    mut tree: Tree,
    home: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Tree),
    on_tick: &mut dyn FnMut() -> Urgency,
    scan: impl FnOnce(Arc<ScanProgress>) -> io::Result<DiskNode> + Send,
) -> Tree {
    let root = tree.root.clone();
    let scan_progress = Arc::new(ScanProgress::default());
    on_tick();
    progress(&tree);
    if cancel.load(Ordering::Relaxed) {
        scan_progress.cancel();
    }
    std::thread::scope(|scope| {
        let (sender, receiver) = mpsc::sync_channel(1);
        let shared = scan_progress.clone();
        let worker = std::thread::Builder::new()
            .name("disk-scan".into())
            .spawn_scoped(scope, move || {
                let result = scan(shared);
                let _ = sender.send(result);
            });
        let Ok(worker) = worker else {
            tree.state = ScanState::Failed("Could not start scan worker".into());
            progress(&tree);
            return tree;
        };
        struct CancelOnDrop<'a>(&'a ScanProgress);
        impl Drop for CancelOnDrop<'_> {
            fn drop(&mut self) {
                self.0.cancel();
            }
        }
        let _cancel_on_drop = CancelOnDrop(&scan_progress);
        let mut last_report = Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) {
                scan_progress.cancel();
            }
            match receiver.recv_timeout(Duration::from_millis(25)) {
                Ok(result) => {
                    let _ = worker.join();
                    let snapshot = scan_progress.snapshot();
                    match result {
                        Ok(node) => {
                            let observed = tree.observed;
                            tree = project(&root, home, &node);
                            tree.hierarchy = Some(Arc::new(node));
                            tree.observed = observed;
                            tree.state = if cancel.load(Ordering::Relaxed) || snapshot.cancelled {
                                ScanState::Cancelled
                            } else if snapshot.limited {
                                ScanState::Failed(
                                    "Retained scan limit reached. Sizes are incomplete.".into(),
                                )
                            } else {
                                ScanState::Complete
                            };
                        }
                        Err(error) => tree.state = ScanState::Failed(error.to_string()),
                    }
                    on_tick();
                    // A command accepted during the last service call can cancel the result.
                    if cancel.load(Ordering::Relaxed) {
                        tree.state = ScanState::Cancelled;
                    }
                    progress(&tree);
                    return tree;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = worker.join();
                    tree.state = ScanState::Failed("Scan worker stopped without a result".into());
                    progress(&tree);
                    return tree;
                }
            }
            if last_report.elapsed() >= REPORT_INTERVAL {
                on_tick();
                let snapshot = scan_progress.snapshot();
                tree.nodes[0].allocated = snapshot.bytes;
                tree.nodes[0].descendants = snapshot.files.saturating_add(snapshot.dirs);
                tree.nodes[0].unreadable = snapshot.errors;
                if let Some((path, completed)) = scan_progress.take_completed() {
                    retain_completed(&mut tree, &root, &path, &completed, home);
                }
                progress(&tree);
                last_report = Instant::now();
            }
        }
    })
}

fn retain_completed(tree: &mut Tree, root: &Path, path: &Path, completed: &DiskNode, home: &Path) {
    if path.parent() == Some(root) {
        let row = node_row(path, completed, home, None);
        retain_row(tree, row);
    }
}

fn descendants(node: &DiskNode) -> u64 {
    node.children.iter().fold(0u64, |count, child| {
        count.saturating_add(1).saturating_add(descendants(child))
    })
}

fn node_row(path: &Path, node: &DiskNode, home: &Path, parent: Option<&DiskNode>) -> Row {
    let cleanable = if node.is_dir() && !node.in_cloud && !node.skipped_volume && !node.read_error {
        cleanable_kind(path, home).or_else(|| {
            (node.filesystem_name() == "target"
                && path.starts_with(home)
                && parent.is_some_and(|parent| {
                    parent.children.iter().any(|child| {
                        child.filesystem_name() == "Cargo.toml"
                            && child.kind == NodeKind::File
                            && !child.in_cloud
                            && !child.read_error
                    })
                }))
            .then_some("Cargo build products")
        })
    } else {
        None
    };
    let (device, inode) = node.identity().unwrap_or_default();
    Row {
        path: path.into(),
        identity: FileId { device, inode },
        directory: node.is_dir(),
        allocated: node.bytes,
        descendants: descendants(node),
        unreadable: node.unreadable,
        shared: node.shared,
        skipped_volume: node.skipped_volume,
        in_cloud: node.in_cloud,
        cleanable,
    }
}

fn data_home_overlaps(root: &Path, alias: &Path) -> bool {
    root.starts_with("/System/Volumes/Data") && (alias.starts_with(root) || root.starts_with(alias))
}

fn retain_row(tree: &mut Tree, row: Row) {
    let node = Node {
        name: row.path.file_name().unwrap_or_default().into(),
        identity: row.identity,
        parent: Some(0),
        children: vec![],
        directory: row.directory,
        allocated: row.allocated,
        descendants: row.descendants,
        unreadable: row.unreadable,
        shared: row.shared,
        skipped_volume: row.skipped_volume,
        in_cloud: row.in_cloud,
        cleanable: row.cleanable,
    };
    if tree.nodes.len() <= PAGE_SIZE {
        tree.nodes.push(node);
    } else {
        let smallest = (1..tree.nodes.len())
            .min_by(|&a, &b| {
                tree.nodes[a]
                    .allocated
                    .cmp(&tree.nodes[b].allocated)
                    .then_with(|| tree.nodes[b].name.cmp(&tree.nodes[a].name))
            })
            .unwrap();
        if node.allocated > tree.nodes[smallest].allocated
            || (node.allocated == tree.nodes[smallest].allocated
                && node.name < tree.nodes[smallest].name)
        {
            tree.nodes[smallest] = node;
        }
    }
    tree.nodes[0].children = (1..tree.nodes.len()).collect();
}

fn project(root: &Path, home: &Path, source: &DiskNode) -> Tree {
    let scan_home = data_home(home).filter(|alias| data_home_overlaps(root, alias));
    let home = scan_home.as_deref().unwrap_or(home);
    let mut tree = Tree::new(root);
    let row = node_row(root, source, home, None);
    tree.nodes[0].identity = row.identity;
    tree.nodes[0].allocated = row.allocated;
    tree.nodes[0].descendants = row.descendants;
    tree.nodes[0].unreadable = row.unreadable;
    tree.nodes[0].in_cloud = row.in_cloud;
    tree.coverage_complete = source.complete;
    fn visit(tree: &mut Tree, path: &Path, source: &DiskNode, home: &Path) {
        tree.cloud_items += u64::from(source.in_cloud);
        for child in &source.children {
            let child_path = path.join(child.filesystem_name());
            if path == tree.root {
                retain_row(tree, node_row(&child_path, child, home, Some(source)));
            }
            visit(tree, &child_path, child, home);
        }
    }
    visit(&mut tree, root, source, home);
    tree
}

#[cfg(test)]
#[path = "adapter_tests.rs"]
mod tests;
