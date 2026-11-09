use super::*;
use std::num::NonZeroU64;

fn directory(name: &str, inode: u64) -> DiskNode {
    let mut node = DiskNode::directory(name);
    node.inode = Some((1, NonZeroU64::new(inode).unwrap()));
    node
}

fn row(path: &str, allocated: u64, inode: u64) -> Row {
    Row {
        path: PathBuf::from(path),
        identity: FileId { device: 1, inode },
        directory: true,
        allocated,
        descendants: 0,
        unreadable: 0,
        shared: false,
        skipped_volume: false,
        in_cloud: false,
        cleanable: None,
    }
}

#[test]
fn projection_preserves_cloud_volume_errors_directory_bytes_and_coverage() {
    let mut root = directory("home", 1);
    root.directory_bytes = 4096;
    let mut cloud = directory("node_modules", 2);
    cloud.in_cloud = true;
    cloud.complete = false;
    cloud.directory_bytes = 8192;
    let mut mounted = directory("mount", 3);
    mounted.skipped_volume = true;
    mounted.complete = false;
    let mut unreadable = directory("denied", 4);
    unreadable.read_error = true;
    root.children = vec![cloud, mounted, unreadable];
    gtop_disk::tree::aggregate(&mut root, Default::default());
    let tree = project(Path::new("/fixture"), Path::new("/fixture"), &root);
    let folder = tree.folder(Path::new("/fixture"), 0, false);
    assert_eq!(folder.allocated, 12288);
    assert_eq!(folder.items, 3);
    assert_eq!(folder.errors, 1);
    assert_eq!(tree.cloud_items, 1);
    assert!(!tree.coverage_complete);
    assert!(
        folder
            .rows
            .iter()
            .find(|row| row.path.ends_with("node_modules"))
            .unwrap()
            .in_cloud
    );
    assert!(
        folder
            .rows
            .iter()
            .find(|row| row.path.ends_with("mount"))
            .unwrap()
            .skipped_volume
    );
    assert!(folder.rows.iter().all(|row| row.cleanable.is_none()));
    assert!(folder.rows.iter().all(|row| row.identity.inode > 0));
}

#[test]
fn retained_limit_keeps_partial_production_result_and_blocks_completion() {
    let home = super::super::tests::Scratch::new();
    for index in 0..50 {
        fs::write(home.0.join(format!("file-{index}")), [1; 4096]).unwrap();
    }
    let tree = run_with_options(
        &home.0,
        &home.0,
        open_root(&home.0).unwrap(),
        &AtomicBool::new(false),
        &mut |_| {},
        &mut || Urgency::Foreground,
        ScanOptions {
            max_retained_bytes: 8192,
            ..Default::default()
        },
    );
    assert!(matches!(tree.state, ScanState::Failed(ref message) if message.contains("limit")));
    assert!(tree.nodes[0].descendants > 0);
    assert!(tree.nodes[0].descendants < 50);
    assert!(!tree.coverage_complete);
    assert!(tree.nodes.len() <= PAGE_SIZE + 1);
}

#[test]
fn pending_scan_services_cancel_and_retains_real_completed_subtree() {
    let home = super::super::tests::Scratch::new();
    fs::create_dir_all(home.0.join("child/nested")).unwrap();
    fs::write(home.0.join("child/nested/file"), [1; 4096]).unwrap();
    let (ready, waiting) = mpsc::sync_channel(1);
    let cancel = AtomicBool::new(false);
    let mut saw_partial = false;
    let tree = coordinate(
        Tree::new(&home.0),
        &home.0,
        &cancel,
        &mut |tree| {
            saw_partial |= tree.state == ScanState::Scanning && tree.nodes[0].descendants > 0;
        },
        &mut || {
            if waiting.try_recv().is_ok() {
                cancel.store(true, Ordering::Relaxed);
            }
            Urgency::Foreground
        },
        |shared| {
            let node =
                gtop_disk::scan::scan_with_progress(&home.0, Default::default(), shared.clone())?;
            ready.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !shared.is_cancelled() && Instant::now() < deadline {
                std::thread::park_timeout(Duration::from_millis(5));
            }
            assert!(
                shared.is_cancelled(),
                "coordinator did not service cancellation"
            );
            Ok(node)
        },
    );
    assert!(saw_partial);
    assert_eq!(tree.state, ScanState::Cancelled);
    assert_eq!(tree.nodes[0].descendants, 3);
    assert!(tree.nodes[0].allocated >= 4096);
    assert!(tree
        .nodes
        .iter()
        .any(|node| node.name == "child" && node.identity.inode > 0));
}

#[test]
fn callback_panic_cancels_and_joins_pending_scan() {
    let home = super::super::tests::Scratch::new();
    let joined = AtomicBool::new(false);
    let mut ticks = 0;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        coordinate(
            Tree::new(&home.0),
            &home.0,
            &AtomicBool::new(false),
            &mut |_| {},
            &mut || {
                ticks += 1;
                assert!(ticks < 2, "injected callback panic");
                Urgency::Background
            },
            |shared| {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !shared.is_cancelled() && Instant::now() < deadline {
                    std::thread::park_timeout(Duration::from_millis(5));
                }
                joined.store(shared.is_cancelled(), Ordering::Relaxed);
                Ok(directory("home", 1))
            },
        );
    }));
    assert!(result.is_err());
    assert!(joined.load(Ordering::Relaxed));
}

#[test]
fn depth_limit_keeps_unknown_coverage_visible_on_production_path() {
    let home = super::super::tests::Scratch::new();
    fs::create_dir_all(home.0.join("child/nested")).unwrap();
    fs::write(home.0.join("child/nested/file"), [1; 4096]).unwrap();
    let tree = run_with_options(
        &home.0,
        &home.0,
        open_root(&home.0).unwrap(),
        &AtomicBool::new(false),
        &mut |_| {},
        &mut || Urgency::Background,
        ScanOptions {
            max_depth: Some(1),
            ..Default::default()
        },
    );
    assert_eq!(tree.state, ScanState::Complete);
    assert!(!tree.coverage_complete);
    assert!(tree.nodes[0].unreadable > 0);
    let row = tree.folder(&home.0, 0, false).rows.remove(0);
    assert_eq!(row.path, home.0.join("child"));
    assert!(row.unreadable > 0);
    assert_ne!(row.identity.inode, 0);
}

#[test]
fn scan_child_panic_returns_failure_and_services_the_next_command() {
    let root = Path::new("/fixture");
    let worker = std::thread::spawn(move || {
        let (commands, requests) = std::sync::mpsc::channel();
        commands.send(true).unwrap();
        commands.send(false).unwrap();
        requests
            .iter()
            .take(2)
            .map(|panic| {
                coordinate(
                    Tree::new(root),
                    root,
                    &AtomicBool::new(false),
                    &mut |_| {},
                    &mut || Urgency::Foreground,
                    move |_| {
                        if panic {
                            panic!("injected scanner panic");
                        }
                        Ok(directory("fixture", 1))
                    },
                )
                .state
            })
            .collect::<Vec<_>>()
    });
    let states = worker
        .join()
        .expect("command worker survives scanner panic");
    assert!(matches!(states[0], ScanState::Failed(_)));
    assert_eq!(states[1], ScanState::Complete);
}

#[test]
fn unavailable_named_leaf_projects_as_one_unreadable_item() {
    let root = Path::new("/fixture");
    let mut source = directory("fixture", 1);
    let mut entry = DiskNode::entry("x", NodeKind::Other, 0);
    entry.read_error = true;
    entry.unreadable = 1;
    entry.complete = false;
    source.children.push(entry);
    gtop_disk::tree::aggregate(&mut source, Default::default());
    let tree = project(root, root, &source);
    let folder = tree.folder(root, 0, false);
    assert_eq!(folder.items, 1);
    assert_eq!(folder.errors, 1);
    assert_eq!(folder.rows.len(), 1);
    assert_eq!(folder.rows[0].path, root.join("x"));
    assert_eq!(folder.rows[0].allocated, 0);
    assert_eq!(folder.rows[0].unreadable, 1);
    assert!(gtop_disk::insights::worth_a_look(&source, 1_800_000_000, 24).is_empty());
    assert!(!tree.coverage_complete);
}

#[test]
fn scan_start_error_publishes_failure_and_services_the_next_command() {
    let worker = std::thread::spawn(|| {
        let root = Path::new("/fixture");
        let (commands, requests) = std::sync::mpsc::channel();
        commands.send(true).unwrap();
        commands.send(false).unwrap();
        requests
            .iter()
            .take(2)
            .map(|fail| {
                let mut published = Vec::new();
                let tree = coordinate(
                    Tree::new(root),
                    root,
                    &AtomicBool::new(false),
                    &mut |tree| published.push(tree.state.clone()),
                    &mut || Urgency::Foreground,
                    move |_| {
                        if fail {
                            Err(io::Error::other("injected pool spawn failure"))
                        } else {
                            Ok(directory("fixture", 1))
                        }
                    },
                );
                assert_eq!(published.last(), Some(&tree.state));
                tree.state
            })
            .collect::<Vec<_>>()
    });
    let states = worker
        .join()
        .expect("command worker survives startup error");
    assert!(
        matches!(&states[0], ScanState::Failed(error) if error.contains("injected pool spawn failure"))
    );
    assert_eq!(states[1], ScanState::Complete);
}

#[test]
fn scanner_cancellation_marks_successful_result_cancelled() {
    let root = Path::new("/fixture");
    let mut published_states = Vec::new();
    let tree = coordinate(
        Tree::new(root),
        root,
        &AtomicBool::new(false),
        &mut |tree| published_states.push(tree.state.clone()),
        &mut || Urgency::Foreground,
        |shared| {
            shared.cancel();
            Ok(directory("fixture", 1))
        },
    );
    assert_eq!(tree.state, ScanState::Cancelled);
    assert_eq!(
        published_states
            .iter()
            .filter(|state| **state == ScanState::Cancelled)
            .count(),
        1
    );
}

#[test]
fn only_completed_root_children_are_retained_as_rows() {
    let root = Path::new("/fixture");
    let child = directory("child", 2);
    let nested = directory("nested", 3);
    let mut tree = Tree::new(root);
    retain_completed(&mut tree, root, &root.join("child"), &child, root);
    retain_completed(&mut tree, root, &root.join("child/nested"), &nested, root);
    assert_eq!(tree.nodes.len(), 2);
    assert_eq!(tree.nodes[1].name, "child");
}

#[test]
fn retain_row_keeps_the_page_limit_and_orders_ties_by_name() {
    let root = Path::new("/fixture");
    let mut tree = Tree::new(root);
    for index in 0..PAGE_SIZE {
        retain_row(
            &mut tree,
            row(
                &format!("/fixture/row-{index:03}"),
                index as u64,
                index as u64 + 1,
            ),
        );
    }
    retain_row(&mut tree, row("/fixture/largest", PAGE_SIZE as u64, 999));
    assert_eq!(tree.nodes.len(), PAGE_SIZE + 1);
    assert_eq!(tree.nodes[0].children.len(), PAGE_SIZE);
    assert!(!tree.nodes.iter().any(|node| node.name == "row-000"));
    assert!(tree.nodes.iter().any(|node| node.name == "largest"));

    let mut tied = Tree::new(root);
    for index in 0..PAGE_SIZE {
        retain_row(
            &mut tied,
            row(&format!("/fixture/row-{index:03}"), 10, index as u64 + 1),
        );
    }
    retain_row(&mut tied, row("/fixture/row-000a", 10, 999));
    assert!(!tied.nodes.iter().any(|node| node.name == "row-127"));
    assert!(tied.nodes.iter().any(|node| node.name == "row-000a"));

    retain_row(&mut tied, row("/fixture/row-zzz", 10, 1000));
    assert!(!tied.nodes.iter().any(|node| node.name == "row-zzz"));

    let mut duplicate = Tree::new(root);
    for index in 0..PAGE_SIZE {
        retain_row(
            &mut duplicate,
            row(&format!("/fixture/row-{index:03}"), 10, index as u64 + 1),
        );
    }
    retain_row(&mut duplicate, row("/fixture/row-127", 10, 999));
    assert_eq!(
        duplicate
            .nodes
            .iter()
            .find(|node| node.name == "row-127")
            .unwrap()
            .identity
            .inode,
        128
    );
}

#[test]
fn cargo_target_is_cleanable_only_with_a_plain_readable_manifest() {
    let target = Path::new("/fixture/project/target");
    let home = Path::new("/fixture");
    let mut target_node = directory("target", 10);
    let mut manifest = DiskNode::entry("Cargo.toml", NodeKind::File, 0);
    manifest.inode = Some((1, NonZeroU64::new(11).unwrap()));
    let mut parent = directory("project", 12);
    parent.children.push(manifest.clone());
    assert_eq!(
        node_row(target, &target_node, home, Some(&parent)).cleanable,
        Some("Cargo build products")
    );
    assert_eq!(
        node_row(
            Path::new("/outside/project/target"),
            &target_node,
            home,
            Some(&parent)
        )
        .cleanable,
        None
    );
    assert_eq!(node_row(target, &target_node, home, None).cleanable, None);

    manifest.in_cloud = true;
    parent.children[0] = manifest.clone();
    assert_eq!(
        node_row(target, &target_node, home, Some(&parent)).cleanable,
        None
    );

    manifest.in_cloud = false;
    manifest.read_error = true;
    parent.children[0] = manifest.clone();
    assert_eq!(
        node_row(target, &target_node, home, Some(&parent)).cleanable,
        None
    );

    manifest.read_error = false;
    manifest.kind = NodeKind::Directory;
    parent.children[0] = manifest;
    assert_eq!(
        node_row(target, &target_node, home, Some(&parent)).cleanable,
        None
    );

    target_node.in_cloud = true;
    assert_eq!(
        node_row(target, &target_node, home, Some(&parent)).cleanable,
        None
    );
}

#[test]
fn data_home_overlap_requires_a_data_root_and_overlapping_paths() {
    assert!(data_home_overlaps(
        Path::new("/System/Volumes/Data/Users"),
        Path::new("/System/Volumes/Data/Users/gabo")
    ));
    assert!(!data_home_overlaps(
        Path::new("/Users/gabo"),
        Path::new("/Users/gabo")
    ));
    assert!(data_home_overlaps(
        Path::new("/System/Volumes/Data/Users/gabo"),
        Path::new("/System/Volumes/Data/Users/gabo/.cargo")
    ));
}
