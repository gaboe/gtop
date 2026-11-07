// Retired scanner. Kept for one commit so removal can be dropped independently.
#[cfg(test)]
#[path = "disk_scan/legacy_queue.rs"]
mod legacy_queue;

#[cfg(test)]
fn legacy_scan(root: &Path, home: &Path, cancel: &AtomicBool, progress: impl FnMut(&Tree)) -> Tree {
    scan_impl(
        root,
        home,
        cancel,
        progress,
        || ScanUrgency::Background,
        legacy_queue::Options {
            initial_urgency: ScanUrgency::Background,
            ..legacy_queue::Options::default()
        },
    )
}

#[cfg(test)]
fn legacy_scan_with_hook(
    root: &Path,
    home: &Path,
    cancel: &AtomicBool,
    progress: impl FnMut(&Tree),
    on_tick: impl FnMut() -> ScanUrgency,
) -> Tree {
    scan_impl(
        root,
        home,
        cancel,
        progress,
        on_tick,
        legacy_queue::Options::default(),
    )
}

#[cfg(test)]
fn scan_impl(
    root: &Path,
    home: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Tree),
    mut on_tick: impl FnMut() -> ScanUrgency,
    options: legacy_queue::Options,
) -> Tree {
    let mut last_tick = Instant::now() - Duration::from_secs(1);
    scan_opened(
        root,
        home,
        cancel,
        legacy_queue::Callbacks {
            progress: &mut progress,
            on_tick: &mut on_tick,
            last_tick: &mut last_tick,
        },
        || open_root(root),
        options,
    )
}


#[cfg(test)]
fn scan_opened(
    root: &Path,
    home: &Path,
    cancel: &AtomicBool,
    callbacks: legacy_queue::Callbacks<'_>,
    open: impl FnOnce() -> io::Result<OwnedFd>,
    options: legacy_queue::Options,
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
        (callbacks.progress)(&tree);
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
    legacy_queue::run(
        &mut tree,
        &root_fd,
        home,
        cancel,
        legacy_queue::Callbacks {
            progress: callbacks.progress,
            on_tick: callbacks.on_tick,
            last_tick: callbacks.last_tick,
        },
        options,
    );
    if tree.state == ScanState::Scanning {
        tree.state = if cancel.load(Ordering::Relaxed) {
            ScanState::Cancelled
        } else {
            ScanState::Complete
        };
    }
    (callbacks.progress)(&tree);
    tree
}


#[cfg(test)]
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
#[cfg(test)]
struct AcceptedEntry {
    name: std::ffi::OsString,
    error: bool,
    row: Row,
}

#[cfg(test)]
struct Reducer<'a> {
    tree: &'a mut Tree,
    device: u64,
    links: &'a mut HashSet<(u64, u64)>,
}

#[cfg(test)]
impl Reducer<'_> {
    fn enter(&mut self, path: &Path, entry: Entry) -> Option<AcceptedEntry> {
        self.tree.cloud_items += u64::from(entry.in_cloud);
        let skipped_volume = entry.device != self.device;
        let mut shared = false;
        if !entry.directory && entry.links > 1 && !entry.error && !skipped_volume {
            if self.links.len() >= MAX_LINKS && !self.links.contains(&(entry.device, entry.inode)) {
                self.tree.state = ScanState::Failed(
                    "Hard-link tracking limit reached. Sizes are incomplete.".into(),
                );
                return None;
            }
            shared = !self.links.insert((entry.device, entry.inode));
        }
        let allocated = if entry.error || skipped_volume || shared {
            0
        } else {
            entry.allocated
        };
        let child_path = path.join(&entry.name);
        let row = Row {
            path: child_path,
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
        Some(AcceptedEntry {
            name: entry.name,
            error: entry.error,
            row,
        })
    }

    fn plain_run(
        &mut self,
        allocated: u64,
        in_cloud: bool,
        count: u64,
        total: &mut (u64, u64, u64),
    ) {
        let bytes = allocated.saturating_mul(count);
        self.tree.cloud_items += if in_cloud { count } else { 0 };
        self.tree.nodes[0].allocated = self.tree.nodes[0].allocated.saturating_add(bytes);
        self.tree.nodes[0].descendants += count;
        total.0 = total.0.saturating_add(bytes);
        total.1 += count;
    }

    fn read_error(&mut self, total: &mut (u64, u64, u64)) {
        total.2 += 1;
        self.tree.nodes[0].unreadable += 1;
    }

    fn descended(&mut self, accepted: &mut AcceptedEntry, result: io::Result<(u64, u64, u64)>) {
        let row = &mut accepted.row;
        match result {
            Ok((bytes, items, errors)) => {
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

    fn finish(&mut self, accepted: AcceptedEntry, depth: usize, total: &mut (u64, u64, u64)) {
        let row = accepted.row;
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
                name: accepted.name,
                identity: row.identity,
                shared: row.shared,
                parent: Some(0),
                children: vec![],
                directory: row.directory,
                allocated: row.allocated,
                descendants: row.descendants,
                unreadable: row.unreadable,
                skipped_volume: row.skipped_volume,
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
    }
}

#[cfg(test)]
impl<F: FnMut(&Tree)> Walker<'_, F> {
    fn reducer(&mut self) -> Reducer<'_> {
        Reducer {
            tree: self.tree,
            device: self.device,
            links: &mut self.links,
        }
    }

    fn walk(&mut self, fd: &OwnedFd, path: &Path, depth: usize) -> (u64, u64, u64) {
        let mut total = (0u64, 0u64, 0u64);
        let result = entries(fd, |entry| self.visit(fd, path, depth, entry, &mut total));
        if result.is_err() {
            self.reducer().read_error(&mut total);
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
        let Some(mut accepted) = self.reducer().enter(path, entry) else {
            return false;
        };
        tick(self.last_tick, self.on_tick);
        if self.last.elapsed() >= Duration::from_millis(100) {
            (self.progress)(self.tree);
            self.last = Instant::now();
        }
        if accepted.row.directory
            && !accepted.row.in_cloud
            && !accepted.row.skipped_volume
            && !accepted.error
        {
            let opened = if depth >= 256 {
                Err(io::Error::other("Directory depth limit reached"))
            } else {
                open_child(fd, &accepted.name, accepted.row.identity)
            };
            let result = opened.map(|child| self.walk(&child, &accepted.row.path, depth + 1));
            self.reducer().descended(&mut accepted, result);
        }
        let row = &mut accepted.row;
        row.cleanable = if !row.directory || row.in_cloud || row.skipped_volume || accepted.error {
            None
        } else {
            cleanable_kind(&row.path, self.home).or_else(|| {
                (accepted.name == "target" && row.path.starts_with(self.home) && local_manifest(fd))
                    .then_some("Cargo build products")
            })
        };
        self.reducer().finish(accepted, depth, total);
        true
    }
}

#[cfg(test)]
fn local_manifest(parent: &OwnedFd) -> bool {
    local_manifest_while(parent, || true)
}
fn local_manifest_while(parent: &OwnedFd, mut keep_going: impl FnMut() -> bool) -> bool {
    let Ok(expected) = identity(parent) else {
        return false;
    };
    let Ok(fd) = open_child(parent, std::ffi::OsStr::new("."), expected) else {
        return false;
    };
    let mut found = false;
    let _ = entries(&fd, |entry| {
        if !keep_going() {
            return false;
        }
        if entry.name == "Cargo.toml" {
            found = !entry.in_cloud && !entry.error && !entry.directory;
            false
        } else {
            true
        }
    });
    found
}


