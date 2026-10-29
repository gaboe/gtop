// Frozen serial accounting from 1236cfb. Filesystem helpers remain shared.
use super::*;

pub(super) fn scan_opened(
    root: &Path,
    home: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Tree),
    on_tick: &mut impl FnMut(),
    last_tick: &mut Instant,
    open: impl FnOnce() -> io::Result<OwnedFd>,
) -> Tree {
    let scan_home = data_home(home).filter(|alias| {
        root.starts_with("/System/Volumes/Data")
            && (alias.starts_with(root) || root.starts_with(alias))
    });
    let home = scan_home.as_deref().unwrap_or(home);
    let mut tree = Tree::new(root);
    let metadata = match fs::symlink_metadata(root) {
        Ok(m) if m.is_dir() => m,
        Ok(_) => {
            tree.state = ScanState::Failed("Scan root must be a directory, not a symlink".into());
            return tree;
        }
        Err(e) => {
            tree.state = ScanState::Failed(e.to_string());
            return tree;
        }
    };
    #[cfg(target_os = "macos")]
    if metadata.st_flags() & 0x40000000 != 0 {
        tree.nodes[0].allocated = metadata.blocks() * 512;
        tree.nodes[0].identity = FileId {
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        tree.nodes[0].in_cloud = true;
        tree.cloud_items = 1;
        tree.state = ScanState::Complete;
        progress(&tree);
        return tree;
    }
    let root_fd = match open() {
        Ok(fd) => fd,
        Err(e) => {
            tree.state = ScanState::Failed(e.to_string());
            return tree;
        }
    };
    let root_id = FileId {
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    if identity(&root_fd).ok() != Some(root_id) {
        tree.state = ScanState::Failed("Root changed before scan".into());
        return tree;
    }
    tree.nodes[0].identity = root_id;
    tree.nodes[0].allocated = metadata.blocks() * 512;
    let mut walker = Walker {
        tree: &mut tree,
        home,
        device: metadata.dev(),
        cancel,
        progress: &mut progress,
        last: Instant::now() - Duration::from_secs(1),
        on_tick,
        last_tick,
        links: HashSet::new(),
    };
    walker.walk(&root_fd, root, 0);
    if tree.state == ScanState::Scanning {
        tree.state = if cancel.load(Ordering::Relaxed) {
            ScanState::Cancelled
        } else {
            ScanState::Complete
        };
    }
    progress(&tree);
    tree
}

fn tick<F: FnMut() + ?Sized>(last_tick: &mut Instant, on_tick: &mut F) {
    if last_tick.elapsed() >= Duration::from_millis(100) {
        on_tick();
        *last_tick = Instant::now();
    }
}

struct Walker<'a, F> {
    tree: &'a mut Tree,
    home: &'a Path,
    device: u64,
    cancel: &'a AtomicBool,
    progress: &'a mut F,
    last: Instant,
    on_tick: &'a mut dyn FnMut(),
    last_tick: &'a mut Instant,
    links: HashSet<(u64, u64)>,
}
impl<F: FnMut(&Tree)> Walker<'_, F> {
    fn walk(&mut self, fd: &OwnedFd, path: &Path, depth: usize) -> (u64, u64, u64) {
        let mut total = (0u64, 0u64, 0u64);
        let result = entries(fd, |entry| self.visit(fd, path, depth, entry, &mut total));
        if result.is_err() {
            total.2 += 1;
            self.tree.nodes[0].unreadable += 1;
        }
        total
    }
    fn visit(
        &mut self,
        fd: &OwnedFd,
        path: &Path,
        depth: usize,
        entry: Entry,
        total: &mut (u64, u64, u64),
    ) -> bool {
        if self.cancel.load(Ordering::Relaxed) || matches!(self.tree.state, ScanState::Failed(_)) {
            return false;
        }
        self.tree.cloud_items += u64::from(entry.in_cloud);
        let skipped_volume = entry.device != self.device;
        let mut shared = false;
        if !entry.directory && entry.links > 1 && !entry.error && !skipped_volume {
            if self.links.len() >= MAX_LINKS && !self.links.contains(&(entry.device, entry.inode)) {
                self.tree.state = ScanState::Failed(
                    "Hard-link tracking limit reached. Sizes are incomplete.".into(),
                );
                return false;
            }
            shared = !self.links.insert((entry.device, entry.inode));
        }
        let allocated = if entry.error || skipped_volume || shared {
            0
        } else {
            entry.allocated
        };
        let child_path = path.join(&entry.name);
        let mut row = Row {
            path: child_path.clone(),
            identity: FileId {
                device: entry.device,
                inode: entry.inode,
            },
            directory: entry.directory,
            allocated,
            descendants: 0,
            unreadable: u64::from(entry.error),
            shared,
            skipped_volume,
            in_cloud: entry.in_cloud,
            cleanable: None,
        };
        self.tree.nodes[0].allocated = self.tree.nodes[0].allocated.saturating_add(allocated);
        self.tree.nodes[0].descendants += 1;
        self.tree.nodes[0].unreadable += row.unreadable;
        tick(self.last_tick, self.on_tick);
        if self.last.elapsed() >= Duration::from_millis(100) {
            (self.progress)(self.tree);
            self.last = Instant::now();
        }
        if row.directory && !row.in_cloud && !skipped_volume && !entry.error {
            let opened = if depth >= 256 {
                Err(io::Error::other("Directory depth limit reached"))
            } else {
                open_child(fd, &entry.name, row.identity)
            };
            match opened {
                Ok(child) => {
                    let (bytes, items, errors) = self.walk(&child, &child_path, depth + 1);
                    row.allocated = row.allocated.saturating_add(bytes);
                    row.descendants = items;
                    row.unreadable += errors;
                }
                Err(_) => {
                    row.unreadable += 1;
                    self.tree.nodes[0].unreadable += 1;
                }
            }
        }
        row.cleanable = if !row.directory || row.in_cloud || skipped_volume || entry.error {
            None
        } else {
            cleanable_kind(&child_path, self.home).or_else(|| {
                (entry.name == "target" && child_path.starts_with(self.home) && local_manifest(fd))
                    .then_some("Cargo build products")
            })
        };
        if row.cleanable.is_some() {
            self.tree.candidates.push(row.clone());
            self.tree
                .candidates
                .sort_unstable_by_key(|r| std::cmp::Reverse(r.allocated));
            self.tree.candidates.truncate(24);
        }
        total.0 = total.0.saturating_add(row.allocated);
        total.1 += row.descendants + 1;
        total.2 += row.unreadable;
        if depth == 0 {
            let node = Node {
                name: entry.name,
                identity: row.identity,
                shared,
                parent: Some(0),
                children: vec![],
                directory: row.directory,
                allocated: row.allocated,
                descendants: row.descendants,
                unreadable: row.unreadable,
                skipped_volume,
                in_cloud: row.in_cloud,
                cleanable: row.cleanable,
            };
            if self.tree.nodes.len() <= PAGE_SIZE {
                self.tree.nodes.push(node);
            } else {
                let smallest = (1..self.tree.nodes.len())
                    .min_by_key(|&i| self.tree.nodes[i].allocated)
                    .unwrap();
                if node.allocated > self.tree.nodes[smallest].allocated {
                    self.tree.nodes[smallest] = node;
                }
            }
            self.tree.nodes[0].children = (1..self.tree.nodes.len()).collect();
        }
        true
    }
}

pub(super) fn replay(
    path: &Path,
    tape: &[(usize, Entry)],
    at_capacity: bool,
    cut: usize,
) -> (Tree, (u64, u64, u64)) {
    replay_with_device(path, tape, at_capacity, cut, 1)
}

pub(super) fn replay_with_device(
    path: &Path,
    tape: &[(usize, Entry)],
    at_capacity: bool,
    cut: usize,
    device: u64,
) -> (Tree, (u64, u64, u64)) {
    let mut tree = Tree::new(path);
    tree.nodes[0].allocated = u64::MAX - 8192;
    let cancel = AtomicBool::new(false);
    let mut progress = |_: &Tree| {};
    let mut on_tick = || {};
    let mut last_tick = Instant::now();
    let fd = open_root(path).unwrap();
    let mut walker = Walker {
        tree: &mut tree,
        home: path,
        device,
        cancel: &cancel,
        progress: &mut progress,
        last: Instant::now(),
        on_tick: &mut on_tick,
        last_tick: &mut last_tick,
        links: if at_capacity {
            (0..MAX_LINKS as u64).map(|inode| (device, inode)).collect()
        } else {
            HashSet::new()
        },
    };
    let mut total = (0, 0, 0);
    for (index, (depth, entry)) in tape.iter().enumerate() {
        cancel.store(index >= cut, Ordering::Relaxed);
        if !walker.visit(&fd, path, *depth, entry.clone(), &mut total) {
            break;
        }
    }
    if tree.state == ScanState::Scanning {
        tree.state = if cancel.load(Ordering::Relaxed) {
            ScanState::Cancelled
        } else {
            ScanState::Complete
        };
    }
    (tree, total)
}

thread_local! {
    static CUT: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    static VISITED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static CUT_REACHED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
fn entries(fd: &OwnedFd, mut visit: impl FnMut(Entry) -> bool) -> io::Result<()> {
    if CUT.get().is_none() {
        return super::entries(fd, visit);
    }
    super::entries(fd, |entry| {
        if VISITED.get() >= CUT.get().unwrap() {
            CUT_REACHED.set(true);
            return false;
        }
        VISITED.set(VISITED.get() + 1);
        visit(entry)
    })
}
pub(super) fn scan_until(root: &Path, cut: u64) -> Tree {
    CUT.set(Some(cut));
    VISITED.set(0);
    CUT_REACHED.set(false);
    let mut tree = scan_opened(
        root,
        root,
        &AtomicBool::new(false),
        |_| {},
        &mut || {},
        &mut Instant::now(),
        || open_root(root),
    );
    if CUT_REACHED.get() {
        tree.state = ScanState::Cancelled;
    }
    CUT.set(None);
    tree
}
