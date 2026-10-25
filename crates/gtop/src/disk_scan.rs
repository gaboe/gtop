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
const MAX_NODES: usize = 12_000_000;
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
    pub cleanable: Option<&'static str>,
}
#[derive(Debug)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub root: PathBuf,
    pub state: ScanState,
    pub observed: SystemTime,
    candidates: Vec<usize>,
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
            cleanable: None,
        };
        Self {
            nodes: vec![node],
            root: root.into(),
            state: ScanState::Scanning,
            observed: SystemTime::now(),
            candidates: vec![],
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
            cleanable: n.cleanable,
        }
    }
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
        if cleanable.is_some() {
            self.candidates.push(id);
        }
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
    fn error(&mut self, id: usize) {
        let mut ancestor = Some(id);
        while let Some(id) = ancestor {
            self.nodes[id].unreadable += 1;
            ancestor = self.nodes[id].parent;
        }
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
        let mut cleanable = self.candidates.clone();
        cleanable.sort_unstable_by_key(|&id| std::cmp::Reverse(self.nodes[id].allocated));
        let cleanable = cleanable
            .into_iter()
            .take(24)
            .map(|id| self.row(id))
            .collect();
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
            observed: self.observed,
        }
    }
}

pub fn cleanable_kind(path: &Path, home: &Path) -> Option<&'static str> {
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
        b"target" if path.parent()?.join("Cargo.toml").is_file() => Some("Cargo build products"),
        _ => None,
    }
}

struct Entry {
    name: std::ffi::OsString,
    device: u64,
    inode: u64,
    directory: bool,
    allocated: u64,
    error: bool,
}

#[cfg(target_os = "macos")]
fn entries(fd: &OwnedFd, mut visit: impl FnMut(Entry) -> bool) -> io::Result<()> {
    let mut attrs = libc::attrlist {
        bitmapcount: 5,
        reserved: 0,
        commonattr: 0xa200000b,
        volattr: 0,
        dirattr: 8,
        fileattr: 4,
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
            if length < 60 || length > record.len() {
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
                inode: u64_at(44)?,
                directory,
                allocated: if allocation_valid { u64_at(52)? } else { 0 },
                error: u32_at(24)? != 0
                    || valid & 0x0200000b != 0x0200000b
                    || (!allocation_valid && kind == 1),
            };
            if !allocation_valid || entry.error {
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
                    entry.directory = stat.st_mode & libc::S_IFMT == libc::S_IFDIR;
                    entry.error = false;
                } else {
                    entry.error = true;
                }
            }
            if !visit(entry) {
                return Ok(());
            }
            offset += length;
        }
    }
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
    let scan_home = data_home(home)
        .filter(|alias| root.starts_with("/System/Volumes/Data") && alias.starts_with(root));
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
    let root_fd = match open_root(root) {
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
    let mut seen = HashSet::from([(metadata.dev(), metadata.ino())]);
    let mut pending = vec![0];
    let mut last = Instant::now() - Duration::from_secs(1);
    while let Some(id) = pending.pop() {
        if cancel.load(Ordering::Relaxed) {
            tree.state = ScanState::Cancelled;
            break;
        }
        let fd = match open_directory(&root_fd, &tree, id) {
            Ok(fd) => fd,
            Err(_) => {
                tree.error(id);
                continue;
            }
        };
        let result = entries(&fd, |entry| {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            if tree.nodes.len() >= MAX_NODES
                || tree.path_bytes + entry.name.len() > 256 * 1024 * 1024
            {
                tree.state = ScanState::Failed(format!(
                    "Scan memory limit reached, at most {MAX_NODES} entries or 256 MiB of raw names. Sizes are incomplete."
                ));
                return false;
            }
            let child = tree.add(id, entry, home, metadata.dev(), &mut seen);
            if tree.nodes[child].directory
                && !tree.nodes[child].skipped_volume
                && !tree.nodes[child].shared
                && tree.nodes[child].unreadable == 0
            {
                pending.push(child);
            }
            if last.elapsed() >= Duration::from_millis(100) {
                progress(&tree);
                last = Instant::now();
            }
            true
        });
        if result.is_err() {
            tree.error(id);
        }
        if matches!(tree.state, ScanState::Failed(_)) {
            break;
        }
        if last.elapsed() >= Duration::from_millis(100) {
            progress(&tree);
            last = Instant::now();
        }
    }
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

#[derive(Debug)]
pub struct Confirmation {
    pub path: PathBuf,
    pub allocated: u64,
    device: u64,
    inode: u64,
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
        Ok(Self {
            path,
            allocated,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    fn validate(&self, home: &Path) -> io::Result<()> {
        if guarded_path(&self.path, home)? != self.path {
            return Err(io::Error::other("Path changed after confirmation"));
        }
        let metadata = fs::symlink_metadata(&self.path)?;
        if (metadata.dev(), metadata.ino()) != (self.device, self.inode) {
            return Err(io::Error::other("Item changed after confirmation"));
        }
        Ok(())
    }
    pub fn trash(self, home: &Path) -> io::Result<PathBuf> {
        self.validate(home)?;
        #[cfg(target_os = "macos")]
        {
            use objc2_foundation::{NSFileManager, NSString, NSURL};
            let path = self
                .path
                .to_str()
                .ok_or_else(|| io::Error::other("Path is not valid UTF-8"))?;
            let mut destination = None;
            NSFileManager::defaultManager()
                .trashItemAtURL_resultingItemURL_error(
                    &NSURL::fileURLWithPath(&NSString::from_str(path)),
                    Some(&mut destination),
                )
                .map_err(|e| io::Error::other(e.to_string()))?;
            let destination = destination.and_then(|url| url.path()).map(|path| PathBuf::from(path.to_string()))
                .ok_or_else(|| io::Error::other("Move result unconfirmed. Destination URL was not returned. Rescan before any retry."))?;
            let metadata = fs::symlink_metadata(&destination)?;
            if (metadata.dev(), metadata.ino()) != (self.device, self.inode) {
                return Err(io::Error::other("Move result unconfirmed. Destination identity differs. Rescan before any retry."));
            }
            Ok(destination)
        }
        #[cfg(not(target_os = "macos"))]
        Err(io::Error::other("Move to Trash requires macOS"))
    }
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
    let start = Instant::now();
    let tree = scan(&root, &root, &AtomicBool::new(false), |_| {});
    println!(
        "disk_scan root={} state={:?} allocated={} items={} errors={} wall_seconds={:.6} coverage={}",
        root.display(),
        tree.state,
        tree.nodes[0].allocated,
        tree.nodes[0].descendants,
        tree.nodes[0].unreadable,
        start.elapsed().as_secs_f64(),
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
        assert_eq!(tree.nodes.iter().filter(|n| n.shared).count(), 1);
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
            if t.nodes.len() > 1 {
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
    #[ignore = "moves a newly created scratch file using the native macOS Trash API"]
    fn native_trash_moves_only_the_created_scratch_file() {
        let home = Scratch::new();
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
        assert_eq!(
            tree.path_bytes,
            home.0.as_os_str().len() + "empty".len() + raw.len()
        );
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
