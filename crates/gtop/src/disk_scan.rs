#[cfg(target_os = "macos")]
use std::os::macos::fs::MetadataExt as _;
use std::{
    collections::HashSet,
    fs, io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime},
};

pub const PAGE_SIZE: usize = 128;
const MAX_LINKS: usize = 250_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileId {
    pub device: u64,
    pub inode: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanState {
    Scanning,
    Complete,
    Cancelled,
    Failed(String),
}
#[derive(Clone, Debug)]
pub struct Node {
    pub name: std::ffi::OsString,
    pub identity: FileId,
    pub shared: bool,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub directory: bool,
    pub allocated: u64,
    pub descendants: u64,
    pub unreadable: u64,
    pub skipped_volume: bool,
    pub in_cloud: bool,
    pub cleanable: Option<&'static str>,
}
#[derive(Debug)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub root: PathBuf,
    pub state: ScanState,
    pub observed: SystemTime,
    candidates: Vec<Row>,
    cloud_items: u64,
    #[cfg(test)]
    path_bytes: usize,
}
#[derive(Clone, Debug)]
pub struct Row {
    pub path: PathBuf,
    pub identity: FileId,
    pub directory: bool,
    pub allocated: u64,
    pub descendants: u64,
    pub unreadable: u64,
    pub shared: bool,
    pub skipped_volume: bool,
    pub in_cloud: bool,
    pub cleanable: Option<&'static str>,
}
#[derive(Clone, Debug)]
pub struct Folder {
    pub root: PathBuf,
    pub path: PathBuf,
    pub state: ScanState,
    pub allocated: u64,
    pub items: u64,
    pub errors: u64,
    pub rows: Vec<Row>,
    pub cleanable: Vec<Row>,
    pub page: usize,
    pub pages: usize,
    pub cached: bool,
    pub smaller_items: u64,
    pub smaller_bytes: u64,
    pub observed: SystemTime,
}
impl Tree {
    fn new(root: &Path) -> Self {
        let node = Node {
            name: std::ffi::OsString::new(),
            identity: FileId {
                device: 0,
                inode: 0,
            },
            shared: false,
            parent: None,
            children: vec![],
            directory: true,
            allocated: 0,
            descendants: 0,
            unreadable: 0,
            skipped_volume: false,
            in_cloud: false,
            cleanable: None,
        };
        Self {
            nodes: vec![node],
            root: root.into(),
            state: ScanState::Scanning,
            observed: SystemTime::now(),
            candidates: vec![],
            cloud_items: 0,
            #[cfg(test)]
            path_bytes: root.as_os_str().len(),
        }
    }
    pub fn path(&self, mut id: usize) -> PathBuf {
        let mut names = Vec::new();
        while let Some(parent) = self.nodes[id].parent {
            names.push(self.nodes[id].name.as_os_str());
            id = parent;
        }
        let mut path = self.root.clone();
        for name in names.into_iter().rev() {
            path.push(name);
        }
        path
    }
    fn row(&self, id: usize) -> Row {
        let n = &self.nodes[id];
        Row {
            path: self.path(id),
            identity: n.identity,
            directory: n.directory,
            allocated: n.allocated,
            descendants: n.descendants,
            unreadable: n.unreadable,
            shared: n.shared,
            skipped_volume: n.skipped_volume,
            in_cloud: n.in_cloud,
            cleanable: n.cleanable,
        }
    }
    #[cfg(test)]
    fn add(
        &mut self,
        parent: usize,
        entry: Entry,
        home: &Path,
        device: u64,
        seen: &mut HashSet<(u64, u64)>,
    ) -> usize {
        let skipped_volume = entry.device != device;
        let shared = !skipped_volume && !entry.error && !seen.insert((entry.device, entry.inode));
        let allocated = if skipped_volume || entry.error || shared {
            0
        } else {
            entry.allocated
        };
        let id = self.nodes.len();
        let cleanable = match entry.name.as_bytes() {
            b"Caches" | b"DerivedData" | b".npm" | b"cache" | b"store" | b".Trash"
            | b"node_modules" | b"target" => {
                cleanable_kind(&self.path(parent).join(&entry.name), home)
            }
            _ => None,
        };

        self.nodes.push(Node {
            name: entry.name,
            identity: FileId {
                device: entry.device,
                inode: entry.inode,
            },
            shared,
            parent: Some(parent),
            children: vec![],
            directory: entry.directory,
            allocated,
            descendants: 0,
            unreadable: u64::from(entry.error),
            skipped_volume,
            in_cloud: entry.in_cloud,
            cleanable,
        });
        self.path_bytes += self.nodes[id].name.len();
        self.nodes[parent].children.push(id);
        let mut ancestor = Some(parent);
        while let Some(id) = ancestor {
            let node = &mut self.nodes[id];
            node.allocated = node.allocated.saturating_add(allocated);
            node.descendants += 1;
            node.unreadable += u64::from(entry.error);
            ancestor = node.parent;
        }
        id
    }
    pub fn folder(&self, path: &Path, page: usize, cached: bool) -> Folder {
        let mut id = 0;
        if let Ok(relative) = path.strip_prefix(&self.root) {
            for component in relative.components() {
                let Some(child) = self.nodes[id]
                    .children
                    .iter()
                    .copied()
                    .find(|&child| self.nodes[child].name == component.as_os_str())
                else {
                    id = 0;
                    break;
                };
                id = child;
            }
        }
        let node = &self.nodes[id];
        let mut children = node.children.clone();
        children.sort_unstable_by(|&a, &b| {
            self.nodes[b]
                .allocated
                .cmp(&self.nodes[a].allocated)
                .then(self.nodes[a].name.cmp(&self.nodes[b].name))
        });
        let pages = children.len().div_ceil(PAGE_SIZE).max(1);
        let page = page.min(pages - 1);
        let rows = children
            .into_iter()
            .skip(page * PAGE_SIZE)
            .take(PAGE_SIZE)
            .map(|id| self.row(id))
            .collect();
        let cleanable = self.candidates.clone();
        let shown_items: u64 = node
            .children
            .iter()
            .map(|&id| self.nodes[id].descendants + 1)
            .sum();
        let shown_bytes: u64 = node
            .children
            .iter()
            .map(|&id| self.nodes[id].allocated)
            .sum();
        Folder {
            root: self.root.clone(),
            path: self.path(id),
            state: self.state.clone(),
            allocated: node.allocated,
            items: node.descendants,
            errors: self.nodes[0].unreadable,
            rows,
            cleanable,
            page,
            pages,
            cached,
            smaller_items: node.descendants.saturating_sub(shown_items),
            smaller_bytes: node.allocated.saturating_sub(shown_bytes),
            observed: self.observed,
        }
    }
}

fn cleanable_kind(path: &Path, home: &Path) -> Option<&'static str> {
    let relative = path.strip_prefix(home).ok()?;
    let known = [
        ("Library/Caches", "App caches, recreated by apps"),
        (
            "Library/Developer/Xcode/DerivedData",
            "Xcode build products and indexes",
        ),
        (".npm", "npm downloaded packages"),
        (".bun/install/cache", "Bun downloaded packages"),
        ("Library/pnpm/store", "pnpm downloaded packages"),
        (".local/share/pnpm/store", "pnpm downloaded packages"),
        (".cargo/registry/cache", "Cargo downloaded archives"),
        (".Trash", "Trash, emptying is not supported"),
    ];
    if let Some((_, description)) = known.iter().find(|(p, _)| relative == Path::new(p)) {
        return Some(description);
    }
    match path.file_name()?.as_bytes() {
        b"node_modules" => Some("Installed JavaScript dependencies"),
        _ => None,
    }
}

fn local_manifest(parent: &OwnedFd) -> bool {
    let Ok(expected) = identity(parent) else {
        return false;
    };
    let Ok(fd) = open_child(parent, std::ffi::OsStr::new("."), expected) else {
        return false;
    };
    let mut found = false;
    let _ = entries(&fd, |entry| {
        if entry.name == "Cargo.toml" {
            found = !entry.in_cloud && !entry.error && !entry.directory;
            false
        } else {
            true
        }
    });
    found
}

struct Entry {
    name: std::ffi::OsString,
    device: u64,
    inode: u64,
    directory: bool,
    allocated: u64,
    error: bool,
    in_cloud: bool,
    links: u32,
}

#[cfg(target_os = "macos")]
fn entries(fd: &OwnedFd, mut visit: impl FnMut(Entry) -> bool) -> io::Result<()> {
    let mut attrs = libc::attrlist {
        bitmapcount: 5,
        reserved: 0,
        commonattr: 0xa204000b,
        volattr: 0,
        dirattr: 8,
        fileattr: 5,
        forkattr: 0,
    };
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let count = unsafe {
            libc::getattrlistbulk(
                fd.as_raw_fd(),
                (&mut attrs as *mut libc::attrlist).cast(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                8,
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count == 0 {
            return Ok(());
        }
        let mut offset = 0;
        for _ in 0..count {
            let record = buffer
                .get(offset..)
                .ok_or_else(|| io::Error::other("Invalid bulk record"))?;
            let (entry, length) = bulk_entry(fd, record)?;
            if !visit(entry) {
                return Ok(());
            }
            offset += length;
        }
    }
}
#[cfg(target_os = "macos")]
fn bulk_entry(fd: &OwnedFd, record: &[u8]) -> io::Result<(Entry, usize)> {
    let u32_at = |i| -> io::Result<u32> {
        Ok(u32::from_ne_bytes(
            record
                .get(i..i + 4)
                .ok_or_else(|| io::Error::other("Short bulk record"))?
                .try_into()
                .unwrap(),
        ))
    };
    let u64_at = |i| -> io::Result<u64> {
        Ok(u64::from_ne_bytes(
            record
                .get(i..i + 8)
                .ok_or_else(|| io::Error::other("Short bulk record"))?
                .try_into()
                .unwrap(),
        ))
    };
    let length = u32_at(0)? as usize;
    if length < 64 || length > record.len() {
        return Err(io::Error::other("Invalid bulk length"));
    }
    let valid = u32_at(4)?;
    let name_offset = 28i64 + i64::from(u32_at(28)? as i32);
    let name_len = u32_at(32)? as usize;
    if name_offset < 0 || name_len == 0 {
        return Err(io::Error::other("Invalid bulk name"));
    }
    let start = name_offset as usize;
    let name = record
        .get(start..start.saturating_add(name_len))
        .filter(|_| start + name_len <= length)
        .ok_or_else(|| io::Error::other("Invalid bulk name range"))?;
    let kind = u32_at(40)?;
    let directory = kind == 2;
    let allocation_valid = if directory {
        u32_at(12)? & 8 != 0
    } else {
        u32_at(16)? & 4 != 0
    };
    let mut entry = Entry {
        name: std::ffi::OsStr::from_bytes(&name[..name_len - 1]).to_owned(),
        device: u64::from(u32_at(36)?),
        inode: u64_at(48)?,
        directory,
        allocated: if allocation_valid {
            u64_at(if directory { 56 } else { 60 })?
        } else {
            0
        },
        in_cloud: valid & 0x00040000 != 0 && u32_at(44)? & 0x40000000 != 0,
        links: if directory { 1 } else { u32_at(56)? },
        error: valid & 0x00040000 == 0
            || (kind == 1 && u32_at(16)? & 1 == 0)
            || u32_at(24)? != 0
            || valid & 0x0200000b != 0x0200000b
            || (!allocation_valid && kind == 1),
    };
    if valid & 0x00040000 != 0 && !entry.in_cloud && (!allocation_valid || entry.error) {
        let name = std::ffi::CString::new(entry.name.as_bytes())?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                fd.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == 0
        {
            let stat = unsafe { stat.assume_init() };
            entry.device = stat.st_dev as u64;
            entry.inode = stat.st_ino;
            entry.allocated = (stat.st_blocks as u64).saturating_mul(512);
            entry.links = u32::from(stat.st_nlink);
            entry.in_cloud = stat.st_flags & 0x40000000 != 0;
            entry.directory = stat.st_mode & libc::S_IFMT == libc::S_IFDIR;
            entry.error = false;
        } else {
            entry.error = true;
        }
    }
    Ok((entry, length))
}
#[cfg(not(target_os = "macos"))]
fn entries(fd: &OwnedFd, mut visit: impl FnMut(Entry) -> bool) -> io::Result<()> {
    for entry in fs::read_dir(format!("/dev/fd/{}", fd.as_raw_fd()))? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if !visit(Entry {
            name: entry.file_name(),
            device: metadata.dev(),
            inode: metadata.ino(),
            directory: metadata.is_dir(),
            allocated: metadata.blocks() * 512,
            error: false,
            in_cloud: false,
            links: metadata.nlink() as u32,
        }) {
            break;
        }
    }
    Ok(())
}

fn open_root(path: &Path) -> io::Result<OwnedFd> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}
fn identity(fd: &OwnedFd) -> io::Result<FileId> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    Ok(FileId {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
    })
}
#[cfg(test)]
fn open_directory(root: &OwnedFd, tree: &Tree, id: usize) -> io::Result<OwnedFd> {
    let mut chain = vec![];
    let mut current = id;
    while let Some(parent) = tree.nodes[current].parent {
        chain.push(current);
        current = parent;
    }
    let mut fd = root.try_clone()?;
    for id in chain.into_iter().rev() {
        let name = std::ffi::CString::new(tree.nodes[id].name.as_bytes())?;
        let child = unsafe {
            libc::openat(
                fd.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if child < 0 {
            return Err(io::Error::last_os_error());
        }
        fd = unsafe { OwnedFd::from_raw_fd(child) };
        if identity(&fd)? != tree.nodes[id].identity {
            return Err(io::Error::other("Directory changed during scan"));
        }
    }
    Ok(fd)
}

pub fn scan(
    root: &Path,
    home: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Tree),
) -> Tree {
    scan_opened(root, home, cancel, &mut progress, || open_root(root))
}

pub fn scan_beneath(
    scope: &Path,
    root: &Path,
    home: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Tree),
) -> Tree {
    let opened = (|| {
        let mut fd = open_root(scope)?;
        let device = identity(&fd)?.device;
        let relative = root.strip_prefix(scope).map_err(io::Error::other)?;
        for component in relative.components() {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::other("Scan cancelled"));
            }
            let Component::Normal(name) = component else {
                return Err(io::Error::other("Invalid folder path"));
            };
            let mut found = None;
            entries(&fd, |entry| {
                if cancel.load(Ordering::Relaxed) {
                    return false;
                }
                if entry.name == name {
                    found = Some(entry);
                    false
                } else {
                    true
                }
            })?;
            let entry = found.ok_or_else(|| io::Error::other("Folder no longer exists"))?;
            if entry.in_cloud || entry.error || !entry.directory || entry.device != device {
                return Err(io::Error::other("Folder is in cloud or unavailable"));
            }
            fd = open_child(
                &fd,
                name,
                FileId {
                    device: entry.device,
                    inode: entry.inode,
                },
            )?;
        }
        Ok(fd)
    })();
    match opened {
        Ok(fd) => scan_opened(root, home, cancel, &mut progress, || Ok(fd)),
        Err(error) => {
            let mut tree = Tree::new(root);
            tree.state = if cancel.load(Ordering::Relaxed) {
                ScanState::Cancelled
            } else {
                ScanState::Failed(error.to_string())
            };
            progress(&tree);
            tree
        }
    }
}
fn scan_opened(
    root: &Path,
    home: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Tree),
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

struct Walker<'a, F> {
    tree: &'a mut Tree,
    home: &'a Path,
    device: u64,
    cancel: &'a AtomicBool,
    progress: &'a mut F,
    last: Instant,
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
fn open_child(parent: &OwnedFd, name: &std::ffi::OsStr, expected: FileId) -> io::Result<OwnedFd> {
    let name = std::ffi::CString::new(name.as_bytes())?;
    let child = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if child < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(child) };
    if identity(&fd)? != expected {
        return Err(io::Error::other("Directory changed during scan"));
    }
    Ok(fd)
}

pub fn foreground_priority() {
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INITIATED, 0);
    }
}

pub fn background_priority() {
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}

#[derive(Debug)]
pub struct Confirmation {
    pub path: PathBuf,
    pub allocated: u64,
    expected: FileId,
    parent: OwnedFd,
    home_fd: OwnedFd,
    name: std::ffi::CString,
    home: PathBuf,
}
fn data_home(home: &Path) -> Option<PathBuf> {
    let relative = home.strip_prefix("/").ok()?;
    let alias = Path::new("/System/Volumes/Data").join(relative);
    let ordinary = fs::symlink_metadata(home).ok()?;
    let alternate = fs::symlink_metadata(&alias).ok()?;
    (ordinary.is_dir()
        && alternate.is_dir()
        && (ordinary.dev(), ordinary.ino()) == (alternate.dev(), alternate.ino()))
        .then_some(alias)
}
fn home_spelling(path: &Path, home: &Path) -> PathBuf {
    if let Some(alias) = data_home(home) {
        if let Ok(relative) = path.strip_prefix(alias) {
            return home.join(relative);
        }
    }
    path.into()
}
pub fn guarded_path(path: &Path, home: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(io::Error::other(
            "Use an absolute path without parent components",
        ));
    }
    let home = home.canonicalize()?;
    let canonical = home_spelling(&path.canonicalize()?, &home);
    if ["/System", "/usr", "/bin", "/sbin"]
        .iter()
        .any(|p| canonical.starts_with(p))
        || canonical == home
        || !canonical.starts_with(&home)
        || canonical.starts_with(home.join(".Trash"))
        || fs::symlink_metadata(path)?.file_type().is_symlink()
    {
        return Err(io::Error::other(
            "Only items inside your home can move to Trash. Trash cannot be emptied here.",
        ));
    }
    Ok(canonical)
}
impl Confirmation {
    pub fn new(path: &Path, allocated: u64, observed: FileId, home: &Path) -> io::Result<Self> {
        let path = guarded_path(path, home)?;
        let metadata = fs::symlink_metadata(&path)?;
        if observed
            != (FileId {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        {
            return Err(io::Error::other(
                "Item changed since scan. Rescan before cleanup.",
            ));
        }
        if metadata.dev() != fs::metadata(home)?.dev() {
            return Err(io::Error::other("Cleanup cannot cross volumes"));
        }
        let home = home.canonicalize()?;
        let home_fd = open_root(&home)?;
        let mut parent = home_fd.try_clone()?;
        let relative = path
            .parent()
            .unwrap()
            .strip_prefix(&home)
            .map_err(io::Error::other)?;
        for component in relative.components() {
            let name = std::ffi::CString::new(component.as_os_str().as_bytes())?;
            let fd = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            parent = unsafe { OwnedFd::from_raw_fd(fd) };
        }
        let name = std::ffi::CString::new(path.file_name().unwrap().as_bytes())?;
        let confirmation = Self {
            path,
            allocated,
            expected: observed,
            parent,
            home_fd,
            name,
            home,
        };
        confirmation.validate(&confirmation.home)?;
        Ok(confirmation)
    }
    fn validate(&self, home: &Path) -> io::Result<()> {
        if home != self.home && home.canonicalize()? != self.home {
            return Err(io::Error::other("Home changed after confirmation"));
        }
        if entry_identity(&self.parent, &self.name)? != self.expected {
            return Err(io::Error::other("Item changed after confirmation"));
        }
        Ok(())
    }
    pub fn trash(self, home: &Path) -> io::Result<PathBuf> {
        self.trash_with_hook(home, || {}, || {})
    }
    fn trash_with_hook(
        self,
        home: &Path,
        before_move: impl FnOnce(),
        after_move: impl FnOnce(),
    ) -> io::Result<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            if unsafe { libc::mkdirat(self.home_fd.as_raw_fd(), c".Trash".as_ptr(), 0o700) } != 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::EEXIST) {
                    return Err(error);
                }
            }
            let trash_fd = unsafe {
                libc::openat(
                    self.home_fd.as_raw_fd(),
                    c".Trash".as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if trash_fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let trash = unsafe { OwnedFd::from_raw_fd(trash_fd) };
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe { libc::fstat(trash.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            let stat = unsafe { stat.assume_init() };
            if stat.st_uid != unsafe { libc::getuid() } {
                return Err(io::Error::other(
                    "Your Trash belongs to another user. Move refused.",
                ));
            }
            if stat.st_dev as u64 != self.expected.device {
                return Err(io::Error::other(
                    "The item and your Trash are on different volumes. Move refused.",
                ));
            }
            let name_max = unsafe { libc::fpathconf(trash.as_raw_fd(), libc::_PC_NAME_MAX) };
            if name_max <= 0 {
                return Err(io::Error::other(
                    "Cannot determine the Trash filename limit. Move refused.",
                ));
            }
            let mut before_move = Some(before_move);
            let original = Path::new(std::ffi::OsStr::from_bytes(self.name.as_bytes()));
            for number in 0..10_000 {
                let name = trash_name(original, number, name_max as usize)?;
                let name = std::ffi::CString::new(name.as_bytes())?;
                self.validate(home)?;
                // The final-name check and rename are separate syscalls in one pinned directory.
                // A same-user writer there can win this race. The destination check detects it.
                if let Some(hook) = before_move.take() {
                    hook();
                }
                let result = unsafe {
                    libc::renameatx_np(
                        self.parent.as_raw_fd(),
                        self.name.as_ptr(),
                        trash.as_raw_fd(),
                        name.as_ptr(),
                        libc::RENAME_EXCL,
                    )
                };
                if result != 0 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() == Some(libc::EEXIST) {
                        continue;
                    }
                    return Err(error);
                }
                after_move();
                let destination = descriptor_path(&trash).map_err(|error| io::Error::other(format!(
                    "Move result unconfirmed. Cannot find the Trash directory: {error}. Check Trash before any retry."
                )))?.join(std::ffi::OsStr::from_bytes(name.as_bytes()));
                let moved = entry_identity(&trash, &name).map_err(|error| io::Error::other(format!(
                    "Move result unconfirmed. Cannot check the item in Trash at {}: {error}. Rescan before any retry.", destination.display())))?;
                if moved != self.expected {
                    return Err(io::Error::other(format!("The item now at {} is not the one you selected. Something changed these files during the move. Check Trash.", destination.display())));
                }
                // Descriptor-relative rename does not write Finder's Put Back metadata.
                return Ok(destination);
            }
            Err(io::Error::other(
                "Cannot find an unused Trash filename after 10000 attempts. Move refused.",
            ))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (home, before_move, after_move);
            Err(io::Error::other("Move to Trash requires macOS"))
        }
    }
}

#[cfg(target_os = "macos")]
fn descriptor_path(fd: &OwnedFd) -> io::Result<PathBuf> {
    let mut path = vec![0u8; libc::PATH_MAX as usize];
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETPATH, path.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let end = path
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| io::Error::other("Trash path is not terminated"))?;
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&path[..end])))
}

#[cfg(target_os = "macos")]
fn trash_name(original: &Path, number: usize, limit: usize) -> io::Result<std::ffi::OsString> {
    use std::os::unix::ffi::OsStringExt;
    let original_bytes = original.as_os_str().as_bytes();
    if number == 0 && original_bytes.len() <= limit {
        return Ok(original.as_os_str().to_owned());
    }
    let suffix = format!(" {number}");
    let available = limit
        .checked_sub(suffix.len())
        .filter(|available| *available > 0)
        .ok_or_else(|| io::Error::other("Trash filename limit is too small"))?;
    let stem = original
        .file_stem()
        .unwrap_or(original.as_os_str())
        .as_bytes();
    let mut extension = original.extension().map(|value| value.as_bytes());
    extension = extension.filter(|value| value.len() + 1 < available);
    let stem_end = |stem_limit| {
        let mut end = stem.len().min(stem_limit);
        if let Ok(text) = std::str::from_utf8(stem) {
            while !text.is_char_boundary(end) {
                end -= 1;
            }
        }
        end
    };
    let mut stem_limit = available - extension.map_or(0, |value| value.len() + 1);
    let mut end = stem_end(stem_limit);
    if end == 0 && extension.is_some() {
        extension = None;
        stem_limit = available;
        end = stem_end(stem_limit);
    }
    if end == 0 {
        return Err(io::Error::other(
            "Trash filename limit cannot hold the filename",
        ));
    }
    let mut name = stem[..end].to_vec();
    name.extend_from_slice(suffix.as_bytes());
    if let Some(extension) = extension {
        name.push(b'.');
        name.extend_from_slice(extension);
    }
    Ok(std::ffi::OsString::from_vec(name))
}

fn entry_identity(parent: &OwnedFd, name: &std::ffi::CStr) -> io::Result<FileId> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    Ok(FileId {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
    })
}

pub fn system_root() -> PathBuf {
    let data = Path::new("/System/Volumes/Data");
    if data.is_dir() {
        data.into()
    } else {
        "/".into()
    }
}

pub fn headless() -> bool {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_none_or(|a| a != "--disk-scan") {
        return false;
    }
    let root = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()));
    background_priority();
    let start = Instant::now();
    let tree = scan(&root, &root, &AtomicBool::new(false), |_| {});
    println!(
        "disk_scan root={} state={:?} allocated={} items={} errors={} wall_seconds={:.6} cloud_items={} retained_rows={} coverage={}",
        root.display(),
        tree.state,
        tree.nodes[0].allocated,
        tree.nodes[0].descendants,
        tree.nodes[0].unreadable,
        start.elapsed().as_secs_f64(),
        tree.cloud_items,
        tree.nodes.len().saturating_sub(1) + tree.candidates.len(),
        if tree.state == ScanState::Complete && tree.nodes[0].unreadable == 0 { "full" } else { "incomplete" }
    );
    if tree.state != ScanState::Complete {
        std::process::exit(1);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::AtomicU64;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    pub(super) struct Scratch(pub PathBuf);
    impl Scratch {
        pub(super) fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "gtop-disk-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn observed(path: &Path) -> FileId {
        let m = fs::symlink_metadata(path).unwrap();
        FileId {
            device: m.dev(),
            inode: m.ino(),
        }
    }
    #[test]
    fn bulk_matches_stat_allocated_blocks_with_hardlinks_symlinks_sparse_files_and_directories() {
        let temp = Scratch::new();
        fs::create_dir(temp.0.join("nested")).unwrap();
        fs::write(temp.0.join("nested/data"), vec![7u8; 100_000]).unwrap();
        fs::hard_link(temp.0.join("nested/data"), temp.0.join("hardlink")).unwrap();
        symlink("nested", temp.0.join("link")).unwrap();
        let sparse = fs::File::create(temp.0.join("sparse")).unwrap();
        sparse.set_len(1 << 30).unwrap();
        fs::write(temp.0.join("empty"), []).unwrap();
        let tree = scan(&temp.0, &temp.0, &AtomicBool::new(false), |_| {});
        assert_eq!(tree.state, ScanState::Complete);
        assert_eq!(tree.nodes[0].unreadable, 0, "{tree:?}");
        assert_eq!(tree.nodes[0].descendants, 6);
        let mut seen = HashSet::new();
        let mut total = 0;
        for (id, node) in tree.nodes.iter().enumerate() {
            let path = tree.path(id);
            let m = fs::symlink_metadata(&path).unwrap();
            assert_eq!(node.identity, observed(&path));
            if seen.insert((m.dev(), m.ino())) {
                total += m.blocks() * 512;
            }
            if !node.directory && !node.shared {
                assert_eq!(node.allocated, m.blocks() * 512, "{}", path.display());
            }
        }
        assert_eq!(tree.nodes[0].allocated, total);
        let nested = scan(
            &temp.0.join("nested"),
            &temp.0,
            &AtomicBool::new(false),
            |_| {},
        );
        assert_eq!(nested.nodes[0].descendants, 1);
        assert!(!tree
            .nodes
            .iter()
            .enumerate()
            .any(|(id, _)| tree.path(id) == temp.0.join("link/data")));
    }
    #[test]
    fn scan_cancels_after_work_and_keeps_partial_results() {
        let temp = Scratch::new();
        for i in 0..20 {
            fs::write(temp.0.join(format!("file-{i}")), [1; 4096]).unwrap();
        }
        let cancel = AtomicBool::new(false);
        let tree = scan(&temp.0, &temp.0, &cancel, |t| {
            if t.nodes[0].descendants > 0 {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(tree.state, ScanState::Cancelled);
        assert!(tree.nodes[0].descendants > 0 && tree.nodes[0].descendants < 20);
    }
    #[test]
    fn other_device_is_visible_but_has_no_charged_bytes() {
        let mut tree = Tree::new(Path::new("/fixture"));
        let id = tree.add(
            0,
            Entry {
                name: "mounted".into(),
                device: 2,
                inode: 4,
                directory: true,
                allocated: 8192,
                error: false,
                in_cloud: false,
                links: 1,
            },
            Path::new("/fixture"),
            1,
            &mut HashSet::new(),
        );
        assert!(tree.nodes[id].skipped_volume);
        assert_eq!(tree.nodes[0].allocated, 0);
        assert_eq!(tree.nodes[0].descendants, 1);
    }
    #[test]
    fn guard_refuses_system_outside_home_trash_and_symlink_escape() {
        let home = Scratch::new();
        let outside = Scratch::new();
        fs::create_dir(home.0.join("cache")).unwrap();
        fs::create_dir(home.0.join(".Trash")).unwrap();
        symlink(&outside.0, home.0.join("escape")).unwrap();
        for path in [
            Path::new("/System"),
            Path::new("/usr"),
            &outside.0,
            &home.0,
            &home.0.join(".Trash"),
            &home.0.join("escape"),
        ] {
            assert!(guarded_path(path, &home.0).is_err(), "{}", path.display());
        }
        assert!(guarded_path(&home.0.join("cache"), &home.0).is_ok());
    }
    #[test]
    fn confirmation_binds_scanned_identity_and_revalidates_replacement() {
        let home = Scratch::new();
        let path = home.0.join("cache");
        fs::write(&path, [1; 4096]).unwrap();
        let id = observed(&path);
        let proposal = Confirmation::new(&path, 4096, id, &home.0).unwrap();
        fs::rename(&path, home.0.join("old")).unwrap();
        fs::write(&path, [2; 4096]).unwrap();
        assert!(Confirmation::new(&path, 4096, id, &home.0).is_err());
        assert!(proposal.validate(&home.0).is_err());
    }
    #[test]
    fn directory_swap_to_symlink_is_not_traversed() {
        let home = Scratch::new();
        let outside = Scratch::new();
        fs::create_dir(home.0.join("a")).unwrap();
        fs::write(outside.0.join("secret"), [1; 4096]).unwrap();
        let mut tree = Tree::new(&home.0);
        let m = fs::metadata(home.0.join("a")).unwrap();
        let id = tree.add(
            0,
            Entry {
                name: "a".into(),
                device: m.dev(),
                inode: m.ino(),
                directory: true,
                allocated: m.blocks() * 512,
                error: false,
                in_cloud: false,
                links: 1,
            },
            &home.0,
            m.dev(),
            &mut HashSet::new(),
        );
        fs::rename(home.0.join("a"), home.0.join("old")).unwrap();
        symlink(&outside.0, home.0.join("a")).unwrap();
        assert!(open_directory(&open_root(&home.0).unwrap(), &tree, id).is_err());
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn native_trash_moves_only_the_created_scratch_file() {
        let home = Scratch::new();
        fs::create_dir(home.0.join(".Trash")).unwrap();
        let path = home
            .0
            .join(format!("gtop-native-trash-fixture-{}", std::process::id()));
        fs::write(&path, b"gtop scratch Trash verification").unwrap();
        let before = observed(&path);
        let destination = Confirmation::new(
            &path,
            fs::metadata(&path).unwrap().blocks() * 512,
            before,
            &home.0,
        )
        .unwrap()
        .trash(&home.0)
        .unwrap();
        assert!(!path.exists());
        assert_eq!(observed(&destination), before);
        assert_eq!(
            fs::read(&destination).unwrap(),
            b"gtop scratch Trash verification"
        );
        println!(
            "scratch_trash source={} destination={} identity=matched",
            path.display(),
            destination.display()
        );
        fs::remove_file(destination).unwrap();
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn trash_ancestor_swap_stays_in_pinned_tree() {
        let home = Scratch::new();
        let outside = Scratch::new();
        fs::create_dir(home.0.join(".Trash")).unwrap();
        fs::create_dir_all(home.0.join("project/nested")).unwrap();
        fs::create_dir(outside.0.join("nested")).unwrap();
        let path = home.0.join("project/nested/cache");
        fs::write(&path, b"confirmed").unwrap();
        let expected = observed(&path);
        let proposal = Confirmation::new(&path, 0, expected, &home.0).unwrap();
        let destination = proposal
            .trash_with_hook(
                &home.0,
                || {
                    fs::rename(home.0.join("project"), home.0.join("saved")).unwrap();
                    symlink(&outside.0, home.0.join("project")).unwrap();
                    fs::write(outside.0.join("nested/cache"), b"outside").unwrap();
                },
                || {},
            )
            .unwrap();
        assert_eq!(observed(&destination), expected);
        assert_eq!(fs::read(&destination).unwrap(), b"confirmed");
        assert_eq!(
            fs::read(outside.0.join("nested/cache")).unwrap(),
            b"outside"
        );
        assert!(!home.0.join("saved/nested/cache").exists());
        assert_eq!(fs::read_dir(outside.0.join("nested")).unwrap().count(), 1);
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn trash_name_and_ancestor_swap_never_recovers_outside_home() {
        let home = Scratch::new();
        let outside = Scratch::new();
        fs::create_dir(home.0.join(".Trash")).unwrap();
        fs::create_dir(home.0.join("project")).unwrap();
        let path = home.0.join("project/selected");
        fs::write(&path, b"confirmed").unwrap();
        fs::write(home.0.join("project/replacement"), b"replacement").unwrap();
        let expected = observed(&path);
        let replacement = observed(&home.0.join("project/replacement"));
        let proposal = Confirmation::new(&path, 0, expected, &home.0).unwrap();
        let error = proposal
            .trash_with_hook(
                &home.0,
                || {
                    fs::rename(&path, home.0.join("project/saved")).unwrap();
                    fs::rename(home.0.join("project/replacement"), &path).unwrap();
                },
                || {
                    fs::rename(home.0.join("project"), home.0.join("saved-project")).unwrap();
                    symlink(&outside.0, home.0.join("project")).unwrap();
                },
            )
            .unwrap_err();
        let destination = home.0.join(".Trash/selected");
        assert_eq!(error.to_string(), format!("The item now at {} is not the one you selected. Something changed these files during the move. Check Trash.", destination.display()));
        assert_eq!(observed(&destination), replacement);
        assert_eq!(observed(&home.0.join("saved-project/saved")), expected);
        assert_eq!(fs::read_dir(&outside.0).unwrap().count(), 0);
        assert!(!home.0.join("saved-project/selected").exists());
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn trash_name_swap_reports_exact_destination_without_recovery() {
        for occupied in [false, true] {
            let home = Scratch::new();
            fs::create_dir(home.0.join(".Trash")).unwrap();
            let path = home.0.join("cache");
            fs::write(&path, b"confirmed").unwrap();
            fs::write(home.0.join("replacement"), b"replacement").unwrap();
            let replacement = observed(&home.0.join("replacement"));
            let expected = observed(&path);
            let proposal = Confirmation::new(&path, 0, expected, &home.0).unwrap();
            let error = proposal
                .trash_with_hook(
                    &home.0,
                    || {
                        fs::rename(&path, home.0.join("saved")).unwrap();
                        fs::rename(home.0.join("replacement"), &path).unwrap();
                    },
                    || {
                        if occupied {
                            fs::write(&path, b"new occupant").unwrap();
                        }
                    },
                )
                .unwrap_err();
            let destination = home.0.join(".Trash/cache");
            assert_eq!(error.to_string(), format!("The item now at {} is not the one you selected. Something changed these files during the move. Check Trash.", destination.display()));
            assert_eq!(observed(&destination), replacement);
            assert_eq!(fs::read(&destination).unwrap(), b"replacement");
            assert_eq!(observed(&home.0.join("saved")), expected);
            if occupied {
                assert_eq!(fs::read(&path).unwrap(), b"new occupant");
            } else {
                assert!(!path.exists());
            }
        }
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn trash_collision_preserves_existing_items_and_extension() {
        let home = Scratch::new();
        fs::create_dir(home.0.join(".Trash")).unwrap();
        for name in ["cache.txt", "cache 1.txt"] {
            fs::write(home.0.join(".Trash").join(name), b"existing").unwrap();
        }
        let path = home.0.join("cache.txt");
        fs::write(&path, b"confirmed").unwrap();
        let expected = observed(&path);
        let destination = Confirmation::new(&path, 0, expected, &home.0)
            .unwrap()
            .trash(&home.0)
            .unwrap();
        assert_eq!(destination, home.0.join(".Trash/cache 2.txt"));
        assert_eq!(observed(&destination), expected);
        for name in ["cache.txt", "cache 1.txt"] {
            assert_eq!(
                fs::read(home.0.join(".Trash").join(name)).unwrap(),
                b"existing"
            );
        }
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn trash_refuses_other_device_before_moving() {
        let home = Scratch::new();
        fs::create_dir(home.0.join(".Trash")).unwrap();
        let path = home.0.join("cache");
        fs::write(&path, b"confirmed").unwrap();
        let mut proposal = Confirmation::new(&path, 0, observed(&path), &home.0).unwrap();
        // Exercise the production device gate without mounting a volume.
        proposal.expected.device ^= 1;
        let error = proposal
            .trash_with_hook(
                &home.0,
                || panic!("move reached"),
                || panic!("move reached"),
            )
            .unwrap_err();
        assert!(error.to_string().contains("different volumes"));
        assert_eq!(fs::read(&path).unwrap(), b"confirmed");
        assert_eq!(fs::read_dir(home.0.join(".Trash")).unwrap().count(), 0);
    }
    #[test]
    fn missing_root_is_failed_not_empty_complete() {
        let temp = Scratch::new();
        let tree = scan(
            &temp.0.join("missing"),
            &temp.0,
            &AtomicBool::new(false),
            |_| {},
        );
        assert!(matches!(tree.state, ScanState::Failed(_)));
    }
    #[test]
    fn empty_child_reports_its_own_count_and_raw_non_utf8_names_roundtrip() {
        use std::os::unix::ffi::OsStringExt;
        let home = Scratch::new();
        fs::create_dir(home.0.join("empty")).unwrap();
        let raw = std::ffi::OsString::from_vec(vec![b'f', 0xff]);
        let mut tree = scan(&home.0, &home.0, &AtomicBool::new(false), |_| {});
        tree.add(
            0,
            Entry {
                name: raw.clone(),
                device: 1,
                inode: 1,
                directory: false,
                allocated: 0,
                error: false,
                in_cloud: false,
                links: 1,
            },
            &home.0,
            1,
            &mut HashSet::new(),
        );
        assert_eq!(tree.folder(&home.0, 0, false).items, 2);
        let child = tree.folder(&home.0.join("empty"), 0, false);
        assert_eq!(child.path, home.0.join("empty"));
        assert_eq!(child.items, 0);
        assert!(child.rows.is_empty());
        assert!(tree
            .folder(&home.0, 0, false)
            .rows
            .iter()
            .any(|r| r.path.file_name() == Some(raw.as_os_str())));
    }
    #[test]
    fn verified_data_home_alias_has_same_guard_and_categories() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let Some(alias) = data_home(&home) else {
            return;
        };
        for relative in ["Library/Caches", ".cargo", ".npm"] {
            let ordinary = home.join(relative);
            let alternate = alias.join(relative);
            assert_eq!(home_spelling(&alternate, &home), ordinary);
            assert_eq!(
                cleanable_kind(&alternate, &alias),
                cleanable_kind(&ordinary, &home)
            );
            if ordinary.exists() {
                assert_eq!(
                    guarded_path(&alternate, &home).unwrap(),
                    guarded_path(&ordinary, &home).unwrap()
                );
            }
        }
        for path in [
            alias.clone(),
            alias.join(".Trash"),
            PathBuf::from("/System"),
            PathBuf::from("/usr"),
        ] {
            assert!(guarded_path(&path, &home).is_err());
        }
        let scratch = Scratch::new();
        assert_eq!(
            home_spelling(Path::new("/System/Volumes/Data/Users/other"), &scratch.0),
            Path::new("/System/Volumes/Data/Users/other")
        );
    }
    #[test]
    fn large_folder_keeps_only_largest_rows_and_exact_totals() {
        let home = Scratch::new();
        for i in 0..600 {
            fs::write(home.0.join(format!("file-{i}")), vec![1; (i + 1) * 64]).unwrap();
        }
        let mut total = fs::metadata(&home.0).unwrap().blocks() * 512;
        for entry in fs::read_dir(&home.0).unwrap() {
            total += fs::symlink_metadata(entry.unwrap().path())
                .unwrap()
                .blocks()
                * 512;
        }
        let tree = scan(&home.0, &home.0, &AtomicBool::new(false), |_| {});
        assert_eq!(tree.state, ScanState::Complete);
        assert_eq!(tree.nodes.len(), PAGE_SIZE + 1);
        let folder = tree.folder(&home.0, 0, false);
        assert_eq!(folder.allocated, total);
        assert_eq!(folder.items, 600);
        assert_eq!(folder.smaller_items, 600 - PAGE_SIZE as u64);
        assert!(folder
            .rows
            .windows(2)
            .all(|p| p[0].allocated >= p[1].allocated));
        let min_retained = folder.rows.last().unwrap().allocated;
        for entry in fs::read_dir(&home.0).unwrap() {
            let entry = entry.unwrap();
            if !folder.rows.iter().any(|row| row.path == entry.path()) {
                assert!(fs::symlink_metadata(entry.path()).unwrap().blocks() * 512 <= min_retained);
            }
        }
    }
    #[test]
    fn cloud_directory_is_counted_without_open_or_cleanup_probe() {
        let home = Scratch::new();
        let cancel = AtomicBool::new(false);
        let mut tree = Tree::new(&home.0);
        let mut progress = |_: &Tree| {};
        let mut walker = Walker {
            tree: &mut tree,
            home: &home.0,
            device: 1,
            cancel: &cancel,
            progress: &mut progress,
            last: Instant::now(),
            links: HashSet::new(),
        };
        let mut total = (0, 0, 0);
        assert!(walker.visit(
            &open_root(&home.0).unwrap(),
            &home.0,
            0,
            Entry {
                name: "target".into(),
                device: 1,
                inode: 2,
                directory: true,
                allocated: 4096,
                error: false,
                in_cloud: true,
                links: 1,
            },
            &mut total
        ));
        assert_eq!(total, (4096, 1, 0));
        assert!(tree.nodes[1].in_cloud);
        assert_eq!(tree.nodes[1].descendants, 0);
        assert!(tree.candidates.is_empty());
    }
    #[cfg(target_os = "macos")]
    fn bulk_fixture(common: u32, file: u32, flags: u32) -> Vec<u8> {
        let mut record = vec![0; 80];
        for (offset, value) in [
            (0, 80u32),
            (4, common),
            (16, file),
            (28, 44),
            (32, 2),
            (36, 1),
            (40, 1),
            (44, flags),
        ] {
            record[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
        }
        record[48..56].copy_from_slice(&2u64.to_ne_bytes());
        record[60..68].copy_from_slice(&4096u64.to_ne_bytes());
        record[72] = b'x';
        record
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn bulk_missing_link_count_uses_local_fallback_only_with_known_flags() {
        let home = Scratch::new();
        fs::write(home.0.join("x"), [1; 4096]).unwrap();
        fs::hard_link(home.0.join("x"), home.0.join("second")).unwrap();
        let fd = open_root(&home.0).unwrap();
        let (local, _) = bulk_entry(&fd, &bulk_fixture(0x0204000b, 4, 0)).unwrap();
        assert!(!local.error);
        assert_eq!(local.links, 2);
        let (unknown, _) = bulk_entry(&fd, &bulk_fixture(0x0200000b, 4, 0)).unwrap();
        assert!(unknown.error);
        assert!(!unknown.in_cloud);
        assert_eq!(unknown.inode, 2);
        assert_eq!(unknown.links, 0);
        let mut directory_record = bulk_fixture(0x0200000b, 4, 0);
        directory_record[12..16].copy_from_slice(&8u32.to_ne_bytes());
        directory_record[40..44].copy_from_slice(&2u32.to_ne_bytes());
        directory_record[56..64].copy_from_slice(&4096u64.to_ne_bytes());
        let (unknown_directory, _) = bulk_entry(&fd, &directory_record).unwrap();
        assert!(unknown_directory.directory);
        assert!(unknown_directory.error);
        assert!(!unknown_directory.in_cloud);
        let mut tree = Tree::new(&home.0);
        let cancel = AtomicBool::new(false);
        let mut progress = |_: &Tree| {};
        let mut walker = Walker {
            tree: &mut tree,
            home: &home.0,
            device: 1,
            cancel: &cancel,
            progress: &mut progress,
            last: Instant::now(),
            links: HashSet::new(),
        };
        let mut total = (0, 0, 0);
        assert!(walker.visit(&fd, &home.0, 0, unknown_directory, &mut total));
        assert_eq!(total, (0, 1, 1));
        let (cloud, _) = bulk_entry(&fd, &bulk_fixture(0x0204000b, 4, 0x40000000)).unwrap();
        assert!(cloud.in_cloud);
        assert_eq!(cloud.inode, 2);
        assert_eq!(cloud.links, 0);
    }
    #[test]
    fn cancelled_scope_discovery_returns_cancelled_without_entries() {
        let home = Scratch::new();
        fs::create_dir(home.0.join("child")).unwrap();
        let tree = scan_beneath(
            &home.0,
            &home.0.join("child"),
            &home.0,
            &AtomicBool::new(true),
            |_| {},
        );
        assert_eq!(tree.state, ScanState::Cancelled);
        assert_eq!(tree.nodes[0].descendants, 0);
    }
    #[test]
    fn duplicate_hardlink_at_capacity_needs_no_new_tracking_slot() {
        let home = Scratch::new();
        let mut tree = Tree::new(&home.0);
        let cancel = AtomicBool::new(false);
        let mut progress = |_: &Tree| {};
        let mut walker = Walker {
            tree: &mut tree,
            home: &home.0,
            device: 1,
            cancel: &cancel,
            progress: &mut progress,
            last: Instant::now(),
            links: (0..MAX_LINKS as u64).map(|inode| (1, inode)).collect(),
        };
        let mut total = (0, 0, 0);
        assert!(walker.visit(
            &open_root(&home.0).unwrap(),
            &home.0,
            0,
            Entry {
                name: "duplicate".into(),
                device: 1,
                inode: 2,
                directory: false,
                allocated: 4096,
                error: false,
                in_cloud: false,
                links: 2,
            },
            &mut total
        ));
        assert_eq!(total, (0, 1, 0));
        assert!(walker.tree.nodes[1].shared);
        assert!(!matches!(walker.tree.state, ScanState::Failed(_)));
    }
    #[test]
    fn subtree_scan_rejects_symlink_ancestors_and_parent_components() {
        let home = Scratch::new();
        let outside = Scratch::new();
        fs::create_dir(outside.0.join("nested")).unwrap();
        symlink(&outside.0, home.0.join("escape")).unwrap();
        for path in [home.0.join("escape/nested"), home.0.join("../outside")] {
            let tree = scan_beneath(&home.0, &path, &home.0, &AtomicBool::new(false), |_| {});
            assert!(matches!(tree.state, ScanState::Failed(_)));
            assert_eq!(tree.nodes[0].descendants, 0);
        }
    }
    #[test]
    fn pages_are_bounded_and_sorted() {
        let mut tree = Tree::new(Path::new("/fixture"));
        for i in 0..300 {
            tree.add(
                0,
                Entry {
                    name: format!("file-{i}").into(),
                    device: 1,
                    inode: i + 1,
                    directory: false,
                    allocated: i,
                    error: false,
                    in_cloud: false,
                    links: 1,
                },
                Path::new("/fixture"),
                1,
                &mut HashSet::new(),
            );
        }
        let folder = tree.folder(Path::new("/fixture"), 0, false);
        assert_eq!(folder.rows.len(), PAGE_SIZE);
        assert_eq!(folder.pages, 3);
        assert!(folder
            .rows
            .windows(2)
            .all(|p| p[0].allocated >= p[1].allocated));
        assert_eq!(tree.folder(Path::new("/fixture"), 2, false).rows.len(), 44);
    }
}

#[cfg(test)]
mod cleanup_regressions {
    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn cleanup_manifest_stays_in_pinned_parent() {
        let home = tests::Scratch::new();
        let outside = tests::Scratch::new();
        fs::create_dir_all(home.0.join("project/nested")).unwrap();
        fs::create_dir(home.0.join("project/nested/target")).unwrap();
        fs::create_dir(outside.0.join("nested")).unwrap();
        fs::write(outside.0.join("nested/Cargo.toml"), "outside manifest").unwrap();
        let pinned = open_root(&home.0.join("project/nested")).unwrap();
        let metadata = fs::metadata(home.0.join("project/nested/target")).unwrap();
        fs::rename(home.0.join("project"), home.0.join("saved-project")).unwrap();
        symlink(&outside.0, home.0.join("project")).unwrap();
        assert!(!home.0.join("saved-project/nested/Cargo.toml").exists());
        let mut tree = Tree::new(&home.0);
        let cancel = AtomicBool::new(false);
        let mut progress = |_: &Tree| {};
        let mut walker = Walker {
            tree: &mut tree,
            home: &home.0,
            device: metadata.dev(),
            cancel: &cancel,
            progress: &mut progress,
            last: Instant::now(),
            links: HashSet::new(),
        };
        let mut total = (0, 0, 0);
        assert!(walker.visit(
            &pinned,
            &home.0.join("project/nested"),
            0,
            Entry {
                name: "target".into(),
                device: metadata.dev(),
                inode: metadata.ino(),
                directory: true,
                allocated: metadata.blocks() * 512,
                error: false,
                in_cloud: false,
                links: 1
            },
            &mut total
        ));
        println!(
            "manifest_probe outside_only_manifest=true candidates={:?}",
            tree.candidates
        );
        assert!(tree.candidates.is_empty());
        fs::write(
            home.0.join("saved-project/nested/Cargo.toml"),
            "pinned manifest",
        )
        .unwrap();
        assert!(local_manifest(&pinned));
    }

    #[test]
    fn ordinary_files_with_cache_names_are_not_cleanable() {
        let home = tests::Scratch::new();
        fs::write(home.0.join("node_modules"), "personal scratch notes").unwrap();
        fs::create_dir(home.0.join("project")).unwrap();
        fs::write(
            home.0.join("project/Cargo.toml"),
            "[package]\nname=\"fixture\"\nversion=\"0.1.0\"",
        )
        .unwrap();
        fs::write(home.0.join("project/target"), "personal scratch notes").unwrap();
        let tree = scan(&home.0, &home.0, &AtomicBool::new(false), |_| {});
        println!("ordinary_files candidates={:?}", tree.candidates);
        assert!(tree.candidates.is_empty());
        fs::remove_file(home.0.join("project/target")).unwrap();
        fs::create_dir(home.0.join("project/target")).unwrap();
        let tree = scan(&home.0, &home.0, &AtomicBool::new(false), |_| {});
        assert_eq!(tree.candidates.len(), 1);
        assert_eq!(tree.candidates[0].cleanable, Some("Cargo build products"));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod mech {
    use super::tests::Scratch;
    use super::*;
    use std::os::unix::fs::symlink;

    fn id(path: &Path) -> FileId {
        let m = fs::symlink_metadata(path).unwrap();
        FileId {
            device: m.dev(),
            inode: m.ino(),
        }
    }
    fn fixture() -> (Scratch, PathBuf, Confirmation) {
        let home = Scratch::new();
        fs::create_dir(home.0.join(".Trash")).unwrap();
        let path = home.0.join("selected");
        fs::write(&path, b"confirmed").unwrap();
        let c = Confirmation::new(&path, 0, id(&path), &home.0).unwrap();
        (home, path, c)
    }
    fn fds() -> usize {
        (0..4096)
            .filter(|fd| unsafe { libc::fcntl(*fd, libc::F_GETFD) } >= 0)
            .count()
    }

    #[test]
    fn final_name_race_is_detected() {
        let (home, path, c) = fixture();
        let expected = id(&path);
        let replacement = home.0.join("replacement");
        fs::write(&replacement, b"unconfirmed").unwrap();
        let replaced = id(&replacement);
        let error = c
            .trash_with_hook(
                &home.0,
                || {
                    fs::rename(&path, home.0.join("saved")).unwrap();
                    fs::rename(&replacement, &path).unwrap();
                },
                || {},
            )
            .unwrap_err();
        assert_eq!(id(&home.0.join("saved")), expected);
        assert_eq!(id(&home.0.join(".Trash/selected")), replaced);
        assert_eq!(
            fs::read(home.0.join(".Trash/selected")).unwrap(),
            b"unconfirmed"
        );
        println!("wrong_leaf expected={expected:?} moved={replaced:?} error={error}");
    }

    #[test]
    fn renamed_trash_reports_actual_destination() {
        let (home, path, c) = fixture();
        let outside = Scratch::new();
        let expected = id(&path);
        let moved_trash = outside.0.join("moved-trash");
        let returned = c
            .trash_with_hook(
                &home.0,
                || {
                    fs::rename(home.0.join(".Trash"), &moved_trash).unwrap();
                    fs::create_dir(home.0.join(".Trash")).unwrap();
                },
                || {},
            )
            .unwrap();
        assert!(returned.exists());
        assert!(!path.exists());
        assert_eq!(returned, moved_trash.join("selected"));
        assert_eq!(id(&returned), expected);
        assert_eq!(fs::read_dir(home.0.join(".Trash")).unwrap().count(), 0);
        println!(
            "renamed_trash returned={} actual={} identity={expected:?}",
            returned.display(),
            moved_trash.join("selected").display()
        );
    }

    #[test]
    fn renamed_home_reports_actual_destination() {
        let (home, path, c) = fixture();
        let outside = Scratch::new();
        let saved = outside.0.join("saved-home");
        let expected = id(&path);
        fs::rename(&home.0, &saved).unwrap();
        fs::create_dir(&home.0).unwrap();
        fs::create_dir(home.0.join(".Trash")).unwrap();
        let returned = c.trash(&home.0).unwrap();
        assert!(returned.exists());
        assert_eq!(returned, saved.join(".Trash/selected"));
        assert_eq!(id(&returned), expected);
        println!(
            "renamed_home returned={} actual={}",
            returned.display(),
            saved.join(".Trash/selected").display()
        );
    }

    #[test]
    fn destination_replacement_reports_neutral_message() {
        let (home, path, c) = fixture();
        let expected = id(&path);
        let destination = home.0.join(".Trash/selected");
        let error = c
            .trash_with_hook(
                &home.0,
                || {},
                || {
                    assert_eq!(id(&destination), expected);
                    fs::rename(&destination, home.0.join(".Trash/moved-confirmed")).unwrap();
                    fs::write(&destination, b"post-move replacement").unwrap();
                },
            )
            .unwrap_err();
        assert_eq!(id(&home.0.join(".Trash/moved-confirmed")), expected);
        assert_eq!(error.to_string(), format!("The item now at {} is not the one you selected. Something changed these files during the move. Check Trash.", destination.display()));
        println!(
            "destination_replacement confirmed={} replacement={} error={error}",
            home.0.join(".Trash/moved-confirmed").display(),
            destination.display()
        );
    }

    #[test]
    fn longest_filename_collision_preserves_extension() {
        let home = Scratch::new();
        fs::create_dir(home.0.join(".Trash")).unwrap();
        let name = format!("{}.txt", "a".repeat(251));
        let path = home.0.join(&name);
        let existing = home.0.join(".Trash").join(&name);
        fs::write(&path, b"confirmed").unwrap();
        fs::write(&existing, b"existing").unwrap();
        let c = Confirmation::new(&path, 0, id(&path), &home.0).unwrap();
        let moved = c.trash(&home.0).unwrap();
        assert_eq!(moved.file_name().unwrap().as_bytes().len(), 255);
        assert_eq!(moved.extension().unwrap(), "txt");
        assert_eq!(fs::read(&moved).unwrap(), b"confirmed");
        assert!(!path.exists());
        assert_eq!(fs::read(existing).unwrap(), b"existing");
        println!(
            "longest_collision bytes={} destination={}",
            name.len(),
            moved.display()
        );
    }

    #[test]
    fn symlink_and_regular_trash_fail_without_move() {
        for kind in ["symlink", "regular"] {
            let (home, path, c) = fixture();
            let outside = Scratch::new();
            let expected = id(&path);
            fs::remove_dir(home.0.join(".Trash")).unwrap();
            match kind {
                "symlink" => symlink(&outside.0, home.0.join(".Trash")).unwrap(),
                "regular" => fs::write(home.0.join(".Trash"), b"file").unwrap(),
                _ => (),
            }
            let error = c.trash(&home.0).unwrap_err();
            assert_eq!(id(&path), expected);
            assert_eq!(fs::read_dir(&outside.0).unwrap().count(), 0);
            println!("trash_{kind} error={error}");
        }
    }

    #[test]
    fn parent_components_and_symlink_escape_are_refused() {
        let (home, path, c) = fixture();
        drop(c);
        let outside = Scratch::new();
        fs::write(outside.0.join("external"), b"outside").unwrap();
        symlink(&outside.0, home.0.join("escape")).unwrap();
        symlink(&path, home.0.join("leaf-link")).unwrap();
        assert!(Confirmation::new(
            &home.0.join("escape/external"),
            0,
            id(&outside.0.join("external")),
            &home.0
        )
        .is_err());
        assert!(Confirmation::new(&home.0.join("leaf-link"), 0, id(&path), &home.0).is_err());
        assert!(
            Confirmation::new(&home.0.join("fake/../selected"), 0, id(&path), &home.0).is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"confirmed");
        assert_eq!(fs::read(outside.0.join("external")).unwrap(), b"outside");
    }

    #[test]
    fn unicode_normalization_and_special_names_preserve_identity() {
        let home = Scratch::new();
        fs::create_dir(home.0.join(".Trash")).unwrap();
        for name in [
            std::ffi::OsString::from("café.txt"),
            std::ffi::OsString::from("cafe\u{301}.txt"),
            std::ffi::OsString::from(".hidden"),
            std::ffi::OsString::from("trailing."),
        ] {
            let path = home.0.join(&name);
            fs::write(&path, b"confirmed").unwrap();
            let expected = id(&path);
            let moved = Confirmation::new(&path, 0, expected, &home.0)
                .unwrap()
                .trash(&home.0)
                .unwrap();
            assert_eq!(id(&moved), expected);
            println!(
                "name source={name:?} destination={:?}",
                moved.file_name().unwrap()
            );
        }
    }

    #[test]
    fn no_descriptor_growth_on_success_failure_or_drop() {
        if std::env::var_os("GTOP_FD_PROBE_CHILD").is_none() {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "disk_scan::mech::no_descriptor_growth_on_success_failure_or_drop",
                    "--exact",
                    "--nocapture",
                ])
                .env("GTOP_FD_PROBE_CHILD", "1")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }

        let baseline = fds();
        for index in 0..300 {
            let (home, path, c) = fixture();
            match index % 3 {
                0 => {
                    c.trash(&home.0).unwrap();
                }
                1 => {
                    fs::remove_dir(home.0.join(".Trash")).unwrap();
                    fs::write(home.0.join(".Trash"), b"not a directory").unwrap();
                    assert!(c.trash(&home.0).is_err());
                }
                _ => {
                    drop(c);
                }
            }
            drop(home);
            let _ = path;
        }
        assert_eq!(fds(), baseline);
        println!("descriptor_count before={baseline} after={}", fds());
    }

    #[test]
    fn many_collisions_preserve_all_existing_files() {
        let (home, path, c) = fixture();
        for n in 0..1024 {
            let name = if n == 0 {
                "selected".into()
            } else {
                format!("selected {n}")
            };
            fs::write(home.0.join(".Trash").join(name), b"existing").unwrap();
        }
        let expected = id(&path);
        let moved = c.trash(&home.0).unwrap();
        assert_eq!(moved, home.0.join(".Trash/selected 1024"));
        assert_eq!(id(&moved), expected);
        for n in 0..1024 {
            let name = if n == 0 {
                "selected".into()
            } else {
                format!("selected {n}")
            };
            assert_eq!(
                fs::read(home.0.join(".Trash").join(name)).unwrap(),
                b"existing"
            );
        }
    }

    #[test]
    fn collision_attempt_limit_refuses_without_moving_source() {
        let (home, path, c) = fixture();
        let expected = id(&path);
        for number in 0..10_000 {
            let name = trash_name(Path::new("selected"), number, 255).unwrap();
            fs::write(home.0.join(".Trash").join(name), b"existing").unwrap();
        }
        let error = c.trash(&home.0).unwrap_err();
        assert!(error.to_string().contains("after 10000 attempts"));
        assert_eq!(id(&path), expected);
        assert_eq!(fs::read(&path).unwrap(), b"confirmed");
        for entry in fs::read_dir(home.0.join(".Trash")).unwrap() {
            assert_eq!(fs::read(entry.unwrap().path()).unwrap(), b"existing");
        }
        assert_eq!(fs::read_dir(home.0.join(".Trash")).unwrap().count(), 10_000);
    }

    #[test]
    fn exclusive_rename_handles_racing_collision_and_rechecks_source() {
        for replace_source in [false, true] {
            let (home, path, c) = fixture();
            let expected = id(&path);
            let result = c.trash_with_hook(
                &home.0,
                || {
                    fs::write(home.0.join(".Trash/selected"), b"racing occupant").unwrap();
                    if replace_source {
                        fs::rename(&path, home.0.join("saved")).unwrap();
                        fs::write(&path, b"unconfirmed").unwrap();
                    }
                },
                || {},
            );
            assert_eq!(
                fs::read(home.0.join(".Trash/selected")).unwrap(),
                b"racing occupant"
            );
            if replace_source {
                let error = result.unwrap_err();
                assert_eq!(error.to_string(), "Item changed after confirmation");
                assert_eq!(id(&home.0.join("saved")), expected);
                assert_eq!(fs::read(&path).unwrap(), b"unconfirmed");
                assert!(!home.0.join(".Trash/selected 1").exists());
            } else {
                let moved = result.unwrap();
                assert_eq!(moved, home.0.join(".Trash/selected 1"));
                assert_eq!(id(&moved), expected);
            }
        }
    }

    #[test]
    fn mismatch_with_renamed_trash_reports_actual_destination() {
        let (home, path, c) = fixture();
        let outside = Scratch::new();
        let actual = outside.0.join("relocated-trash");
        let error = c
            .trash_with_hook(
                &home.0,
                || {
                    fs::rename(home.0.join(".Trash"), &actual).unwrap();
                    fs::create_dir(home.0.join(".Trash")).unwrap();
                    fs::rename(&path, home.0.join("saved")).unwrap();
                    fs::write(&path, b"unconfirmed").unwrap();
                },
                || {},
            )
            .unwrap_err();
        assert!(!home.0.join(".Trash/selected").exists());
        assert_eq!(fs::read(actual.join("selected")).unwrap(), b"unconfirmed");
        assert!(error
            .to_string()
            .contains(&actual.join("selected").display().to_string()));
        assert!(!error
            .to_string()
            .contains(&home.0.join(".Trash/selected").display().to_string()));
        println!(
            "mismatch_relocated actual={} error={error}",
            actual.join("selected").display()
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod trash_name_regressions {
    use super::*;

    #[test]
    fn missing_trash_is_created_private_and_move_preserves_identity() {
        let home = tests::Scratch::new();
        let path = home.0.join("selected");
        fs::write(&path, b"confirmed").unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let expected = FileId {
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        let moved = Confirmation::new(&path, 0, expected, &home.0)
            .unwrap()
            .trash(&home.0)
            .unwrap();
        assert_eq!(moved, home.0.join(".Trash/selected"));
        assert_eq!(fs::read(&moved).unwrap(), b"confirmed");
        assert_eq!(fs::metadata(&moved).unwrap().ino(), expected.inode);
        let trash = fs::metadata(home.0.join(".Trash")).unwrap();
        assert_eq!(trash.mode() & 0o777, 0o700);
        assert_eq!(trash.uid(), unsafe { libc::getuid() });
        assert!(!path.exists());
    }

    #[test]
    fn collision_names_fit_byte_limit_and_keep_utf8_boundaries() {
        for original in [
            "x".repeat(255),
            format!("{}.txt", "é".repeat(125)),
            format!("a.{}", "x".repeat(253)),
        ] {
            for number in [1, 99, 9999] {
                let name = trash_name(Path::new(&original), number, 255).unwrap();
                let text = name.to_str().unwrap();
                assert!(text.len() <= 255);
                assert!(text.contains(&format!(" {number}")));
                if original.ends_with(".txt") {
                    assert!(text.ends_with(".txt"));
                }
            }
        }
    }

    #[test]
    fn native_multibyte_and_extensionless_collisions_preserve_existing_files() {
        for name in ["x".repeat(255), format!("{}.txt", "é".repeat(125))] {
            let home = tests::Scratch::new();
            fs::create_dir(home.0.join(".Trash")).unwrap();
            let source = home.0.join(&name);
            let existing = home.0.join(".Trash").join(&name);
            fs::write(&source, b"selected").unwrap();
            fs::write(&existing, b"existing").unwrap();
            let m = fs::metadata(&source).unwrap();
            let expected = FileId {
                device: m.dev(),
                inode: m.ino(),
            };
            let moved = Confirmation::new(&source, 0, expected, &home.0)
                .unwrap()
                .trash(&home.0)
                .unwrap();
            assert!(moved.file_name().unwrap().to_str().is_some());
            assert!(moved.file_name().unwrap().as_bytes().len() <= 255);
            assert_eq!(fs::read(&existing).unwrap(), b"existing");
            assert_eq!(fs::read(&moved).unwrap(), b"selected");
            assert_eq!(fs::metadata(&moved).unwrap().ino(), expected.inode);
        }
    }

    #[test]
    fn multibyte_stems_with_long_extensions_resolve_trash_collisions() {
        for (name, expected_name) in [
            (format!("😀.{}", "x".repeat(250)), "😀 1"),
            (format!("界.{}", "x".repeat(251)), "界 1"),
        ] {
            assert_eq!(name.len(), 255);
            let home = tests::Scratch::new();
            fs::create_dir(home.0.join(".Trash")).unwrap();
            let source = home.0.join(&name);
            let existing = home.0.join(".Trash").join(&name);
            fs::write(&source, b"selected").unwrap();
            fs::write(&existing, b"existing").unwrap();
            let source_metadata = fs::metadata(&source).unwrap();
            let expected = FileId {
                device: source_metadata.dev(),
                inode: source_metadata.ino(),
            };
            let existing_metadata = fs::metadata(&existing).unwrap();
            let existing_id = FileId {
                device: existing_metadata.dev(),
                inode: existing_metadata.ino(),
            };

            let moved = Confirmation::new(&source, 0, expected, &home.0)
                .unwrap()
                .trash(&home.0)
                .unwrap();

            assert_eq!(moved, home.0.join(".Trash").join(expected_name));
            assert_eq!(fs::read(&moved).unwrap(), b"selected");
            let moved_metadata = fs::metadata(&moved).unwrap();
            assert_eq!(moved_metadata.dev(), expected.device);
            assert_eq!(moved_metadata.ino(), expected.inode);
            assert!(!source.exists());
            assert_eq!(fs::read(&existing).unwrap(), b"existing");
            let existing_after = fs::metadata(&existing).unwrap();
            assert_eq!(existing_after.dev(), existing_id.device);
            assert_eq!(existing_after.ino(), existing_id.inode);
        }
    }
}
