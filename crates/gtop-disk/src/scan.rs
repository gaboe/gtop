//! Parallel filesystem scanning.
//!
//! The shape follows dust, because that shape is the reason dust is fast:
//!
//! * one `rayon::scope` per root, so recursion depth is O(1) however deep the
//!   tree is,
//! * every directory is a `PendingDir` with an atomic completion counter and a
//!   `+1` sentinel, so a directory is only built once its own scan *and* all of
//!   its subdirectory tasks have finished,
//! * children are handed to the parent as finished `Node`s and sizes are
//!   aggregated bottom-up in one pass over the finished tree, which is also
//!   where hardlinks are de-duplicated.
//!
//! What is added on top of dust's approach: live progress counters the UI can
//! poll without locking, and cooperative cancellation so a re-scan can abandon
//! a walk of a large home directory instead of queueing behind it.

use rustix::fs::{AtFlags, Mode, OFlags};
#[cfg(not(any(windows, target_os = "macos")))]
use std::fs::DirEntry;
use std::fs::{self, Metadata};
use std::io;
use std::num::NonZeroU64;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;

use rayon::Scope;
use rustc_hash::FxHashSet;

use crate::scan_threads::{Admission, ControllerGuard, ScanThreads};
use crate::tree::{Metric, Node, NodeKind, Seen, aggregate, aggregate_deduped};

/// Errors kept verbatim before the list is truncated; the count keeps rising.
const MAX_ERROR_DETAIL: usize = 50;

/// How a scan measures and filters the tree.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ScanOptions {
    /// Measure apparent length instead of allocated blocks. Apparent size is
    /// what `ls -l` shows; blocks are what the volume actually spends, which is
    /// what a disk-space tool normally wants.
    pub apparent_size: bool,
    /// Compatibility option. True is rejected because traversal never follows links.
    pub follow_links: bool,
    /// Include dotfiles and dot-directories. On by default, like dust and `du`:
    /// `~/.cache` is frequently the largest directory in a home directory.
    pub include_hidden: bool,
    /// Stay on the root's volume: skip other disks, pseudo filesystems,
    /// network shares, bind mounts and subvolumes. Select those explicitly
    /// as another root. See [`crate::space::mount_boundaries`]. A disk
    /// tool measures a disk, and its free-space meter only means anything
    /// for one volume.
    pub one_filesystem: bool,
    /// Stop descending past this depth. Totals below that depth are then
    /// unknown, which makes an overview scan of a huge tree cheap.
    pub max_depth: Option<usize>,
    /// Count a hardlinked file once instead of once per link.
    pub dedup_hardlinks: bool,
    /// Whether children are ranked by bytes or by file count.
    pub metric: Metric,
    /// Directory admission for this scan; other scans have their own budget.
    #[serde(default)]
    pub threads: ScanThreads,
    /// Conservative retained tree and queued-directory budget. Enumeration
    /// buffers add at most 64 KiB per worker. Exhaustion returns partial data.
    #[serde(default = "default_retained_bytes")]
    pub max_retained_bytes: usize,
}

const fn default_retained_bytes() -> usize {
    256 * 1024 * 1024
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            apparent_size: false,
            follow_links: false,
            include_hidden: true,
            one_filesystem: true,
            max_depth: None,
            dedup_hardlinks: true,
            metric: Metric::Bytes,
            threads: ScanThreads::default(),
            max_retained_bytes: default_retained_bytes(),
        }
    }
}

impl ScanOptions {
    fn cache_compatible(&self, requested: &Self) -> bool {
        self.apparent_size == requested.apparent_size
            && self.follow_links == requested.follow_links
            && self.include_hidden == requested.include_hidden
            && self.one_filesystem == requested.one_filesystem
            && self.dedup_hardlinks == requested.dedup_hardlinks
            && self.metric == requested.metric
            // Depth is relative to each scan root. A wider root changes it.
            && self.max_depth.is_none()
            && requested.max_depth.is_none()
    }
}

/// Counters a running scan publishes for the UI.
///
/// Plain relaxed atomics: this is a progress meter, not a synchronisation
/// point, and the UI reads a snapshot that is allowed to be a few entries
/// stale.
#[derive(Debug, Default)]
pub struct ScanProgress {
    files: AtomicU64,
    dirs: AtomicU64,
    bytes: AtomicU64,
    errors: AtomicU64,
    finished: AtomicBool,
    cancelled: AtomicBool,
    messages: Mutex<Vec<String>>,
    retained_bytes: AtomicUsize,
    limited: AtomicBool,
    /// At most one completed top-level subtree is awaiting a consumer.
    /// Every snapshot clone is charged to the same monotonic memory budget.
    completed: Mutex<Option<(PathBuf, Arc<Node>)>>,
    preview_root: OnceLock<PathBuf>,
    preview: Mutex<crate::scan_preview::Builder>,
    pub(crate) threads: AtomicUsize,
    pub(crate) threads_settled: AtomicBool,
    pub(crate) thread_tuning_complete: AtomicBool,
    pub(crate) system_cpu_limited: AtomicBool,
    pub(crate) worker_transitions: AtomicU64,
}

/// A point-in-time view of [`ScanProgress`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanSnapshot {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub errors: u64,
    pub finished: bool,
    pub cancelled: bool,
    pub limited: bool,
    pub retained_bytes: usize,
    /// Admitted directory jobs, not a machine-wide thread limit.
    pub threads: usize,
    pub threads_settled: bool,
    pub thread_tuning_complete: bool,
    pub system_cpu_limited: bool,
    pub worker_transitions: u64,
    /// Up to [`MAX_ERROR_DETAIL`] unreadable paths, most recent last.
    pub messages: Vec<String>,
}

impl ScanProgress {
    /// Take a real completed subtree, never a preview or an unfinished walk.
    pub fn take_completed(&self) -> Option<(PathBuf, Arc<Node>)> {
        lock(&self.completed).take()
    }

    pub fn preview(&self) -> crate::scan_preview::Preview {
        lock(&self.preview).snapshot()
    }

    fn preview_add(&self, path: &Path, bytes: u64, files: u64) {
        let Some(root) = self.preview_root.get() else {
            return;
        };
        let Ok(below) = path.strip_prefix(root) else {
            return;
        };
        let name = below
            .components()
            .next()
            .map(|part| part.as_os_str().to_string_lossy());
        let mut preview = lock(&self.preview);
        preview.add(name.as_deref(), bytes, files);
        preview.directory(path);
    }

    pub(crate) fn entries(&self) -> u64 {
        self.files.load(Ordering::Relaxed) + self.dirs.load(Ordering::Relaxed)
    }

    /// What a directory's listing found (see `Tally`), or what a reader
    /// that measures in bulk has come across so far.
    pub(crate) fn add(&self, files: u64, dirs: u64, bytes: u64) {
        self.files.fetch_add(files, Ordering::Relaxed);
        self.dirs.fetch_add(dirs, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    fn record_error(&self, path: &Path, error: &io::Error) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        // A scan without Full Disk Access can refuse thousands of paths;
        // only the ones that will be kept are worth formatting.
        let mut messages = lock(&self.messages);
        if messages.len() < MAX_ERROR_DETAIL {
            messages.push(format!("{}: {error}", path.display()));
        }
        drop(messages);
    }

    fn finish(&self) {
        self.finished.store(true, Ordering::Relaxed);
    }

    /// Ask the walk to stop at the next directory boundary.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Stop the walk now, without waiting for the worker thread.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> ScanSnapshot {
        ScanSnapshot {
            files: self.files.load(Ordering::Relaxed),
            dirs: self.dirs.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            finished: self.finished.load(Ordering::Relaxed),
            cancelled: self.is_cancelled(),
            limited: self.limited.load(Ordering::Relaxed),
            retained_bytes: self.retained_bytes.load(Ordering::Relaxed),
            threads: self.threads.load(Ordering::Relaxed),
            threads_settled: self.threads_settled.load(Ordering::Relaxed),
            thread_tuning_complete: self.thread_tuning_complete.load(Ordering::Relaxed),
            system_cpu_limited: self.system_cpu_limited.load(Ordering::Relaxed),
            worker_transitions: self.worker_transitions.load(Ordering::Relaxed),
            messages: lock(&self.messages).clone(),
        }
    }
}

/// A scan running on its own thread.
#[derive(Debug)]
pub struct ScanHandle {
    pub progress: Arc<ScanProgress>,
    result: Receiver<io::Result<Node>>,
}

/// A subtree already measured, reused by a wider scan instead of walked
/// again: widening from `~` to `/` only reads what is outside `~`.
#[derive(Clone, Debug)]
pub struct Known {
    /// Where it is. Compared with the walk's own paths, so give it in the
    /// same form as the root (both canonical).
    pub path: PathBuf,
    pub tree: Arc<Node>,
    /// Options used to produce this tree. Incompatible scans walk it again.
    pub options: ScanOptions,
}

impl ScanHandle {
    /// Start walking `root` on a worker thread.
    pub fn spawn(root: PathBuf, options: ScanOptions) -> Self {
        Self::spawn_with(root, options, None)
    }

    /// Start walking `root`, reusing `known` where the walk reaches it.
    pub fn spawn_with(root: PathBuf, options: ScanOptions, known: Option<Known>) -> Self {
        let progress = Arc::new(ScanProgress::default());
        let context = Arc::new(WalkContext {
            known,
            options,
            progress: Arc::clone(&progress),
            root_device: Mutex::new(None),
            foreign_mounts: OnceLock::new(),
            never_scanned: OnceLock::new(),
            root: Mutex::new(None),
            root_fd: OnceLock::new(),
        });
        let (sender, result) = mpsc::channel();

        // The closure owns the sender, so a failed spawn drops it and the
        // receiver reports the failure. Record the real reason alongside it.
        let worker = thread::Builder::new()
            .name("disktree-scan".into())
            .spawn(move || {
                let outcome = scan_blocking(&root, &context);
                context.progress.finish();
                let _ = sender.send(outcome);
            });
        if let Err(error) = worker {
            progress.record_error(Path::new("scan worker"), &error);
            progress.finish();
        }

        Self { progress, result }
    }

    /// Take the finished tree, if the walk is done.
    pub fn poll(&self) -> Option<io::Result<Node>> {
        match self.result.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(io::Error::other(
                "the scan worker stopped without a result",
            ))),
        }
    }

    /// Abandon the walk; the worker still finishes the directory it is in.
    pub fn cancel(&self) {
        self.progress.cancel();
    }
}

/// Walk `root` and return the aggregated tree. Blocking.
pub fn scan(root: &Path, options: ScanOptions) -> io::Result<Node> {
    let progress = Arc::new(ScanProgress::default());
    let context = Arc::new(WalkContext {
        known: None,
        options,
        progress: Arc::clone(&progress),
        root_device: Mutex::new(None),
        foreign_mounts: OnceLock::new(),
        never_scanned: OnceLock::new(),
        root: Mutex::new(None),
        root_fd: OnceLock::new(),
    });
    let node = scan_blocking(root, &context)?;
    progress.finish();
    Ok(node)
}

/// Blocking scan with caller-owned progress and cancellation.
pub fn scan_with_progress(
    root: &Path,
    options: ScanOptions,
    progress: Arc<ScanProgress>,
) -> io::Result<Node> {
    scan_with_root(root, None, options, progress)
}

/// Scan a root that the caller opened through a checked directory chain.
/// The path observation must still match the pinned descriptor. The scanner
/// never reopens the root by pathname after that check.
pub fn scan_opened_with_progress(
    root: &Path,
    directory: OwnedFd,
    options: ScanOptions,
    progress: Arc<ScanProgress>,
) -> io::Result<Node> {
    scan_with_root(root, Some(directory), options, progress)
}

fn scan_with_root(
    root: &Path,
    directory: Option<OwnedFd>,
    options: ScanOptions,
    progress: Arc<ScanProgress>,
) -> io::Result<Node> {
    let root_fd = OnceLock::new();
    if let Some(directory) = directory {
        let _ = root_fd.set(directory);
    }
    let context = Arc::new(WalkContext {
        known: None,
        options,
        progress: Arc::clone(&progress),
        root_device: Mutex::new(None),
        foreign_mounts: OnceLock::new(),
        never_scanned: OnceLock::new(),
        root: Mutex::new(None),
        root_fd,
    });
    let result = scan_blocking(root, &context);
    progress.finish();
    result
}

struct WalkContext {
    /// A subtree to reuse rather than walk.
    known: Option<Known>,
    options: ScanOptions,
    progress: Arc<ScanProgress>,
    /// Device of the root, resolved once: the `one_filesystem` fallback
    /// when the mount table cannot be read.
    root_device: Mutex<Option<u64>>,
    /// Mount points `one_filesystem` keeps out, from the mount table. Unset
    /// when the table cannot be read, and devices are compared instead.
    foreign_mounts: OnceLock<FxHashSet<PathBuf>>,
    /// Directories no scan enters, on any volume setting: see
    /// [`crate::space::never_scanned`].
    never_scanned: OnceLock<FxHashSet<PathBuf>>,
    /// Set by the root's `complete`, read after the scope joins.
    root: Mutex<Option<Node>>,
    /// Pinned root. Jobs reopen checked descendants relative to this descriptor.
    root_fd: OnceLock<OwnedFd>,
}

impl WalkContext {
    fn cancelled(&self) -> bool {
        self.progress.is_cancelled() || self.progress.limited.load(Ordering::Relaxed)
    }

    fn reserve(&self, bytes: usize) -> bool {
        let reserved = self
            .progress
            .retained_bytes
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(bytes)
                    .filter(|next| *next <= self.options.max_retained_bytes)
            })
            .is_ok();
        if !reserved {
            self.progress.limited.store(true, Ordering::Relaxed);
        }
        reserved
    }

    fn classify(&self, dir: &Path, entry: &mut impl Listed) -> io::Result<Classified> {
        if !self.options.include_hidden && (entry.name().starts_with('.') || entry.hidden()) {
            return Ok(Classified::Skipped);
        }
        let path = entry.entry_path(dir);
        // Covers both name copies, Vec slack, pending ancestry and path copies,
        // task storage, and dedup set overhead. Reservations are never reused.
        let charge = 1024usize.saturating_add(path.as_os_str().len().saturating_mul(8));
        if !self.reserve(charge) {
            return Ok(Classified::Skipped);
        }
        if let Some(error) = entry.unavailable() {
            let facts = entry.facts(false)?;
            let mut node = leaf_node(entry.take_name(), NodeKind::Other, &facts);
            node.raw_name = path.file_name().map(Into::into);
            node.bytes = 0;
            node.read_error = true;
            node.unreadable = 1;
            node.complete = false;
            node.in_cloud = facts.evicted;
            self.progress
                .record_error(&path, &io::Error::from_raw_os_error(error));
            return Ok(Classified::Entry(node));
        }
        let listing = entry.listing()?;
        let facts = entry.facts(self.options.apparent_size)?;
        let kind = match listing {
            Listing::Directory => NodeKind::Directory,
            Listing::Symlink => NodeKind::Symlink,
            Listing::Leaf(kind) => kind,
        };
        let directory = if kind == NodeKind::Directory {
            Some(entry.directory()?)
        } else {
            None
        };
        let mut node = leaf_node(entry.take_name(), kind, &facts);
        node.raw_name = path.file_name().map(Into::into);
        node.in_cloud = facts.evicted;
        node.complete = !facts.evicted;
        if kind != NodeKind::Directory {
            return Ok(Classified::Entry(node));
        }
        let directory = directory.expect("directory kind");
        node.dirs = 1;
        node.files = 0;
        node.directory_bytes = if self.options.apparent_size {
            0
        } else {
            facts.size
        };
        node.bytes = node.directory_bytes;
        let excluded = self
            .never_scanned
            .get()
            .is_some_and(|never| never.contains(&path));
        node.skipped_volume = self.options.one_filesystem
            && (Some(directory.device) != *lock(&self.root_device)
                || self
                    .foreign_mounts
                    .get()
                    .is_some_and(|foreign| foreign.contains(&path)));
        if excluded || node.skipped_volume {
            node.directory_bytes = 0;
            node.bytes = 0;
            node.complete = false;
            return Ok(Classified::Entry(node));
        }
        if node.in_cloud {
            node.complete = false;
            return Ok(Classified::Entry(node));
        }
        if let Some(known) = &self.known
            && known.path == path
            && known.tree.complete
            && known.options.cache_compatible(&self.options)
            && known.tree.identity() == node.identity()
        {
            // Cached trees consume the same budget as newly enumerated entries.
            let charge = retained_charge(&known.tree, path.as_os_str().len());
            if self.reserve(charge) {
                return Ok(Classified::Entry((*known.tree).clone()));
            }
            return Ok(Classified::Skipped);
        }
        Ok(Classified::Subdirectory { path, node })
    }
}

/// What the walk reads about one directory entry.
///
/// `std::fs::DirEntry` everywhere but Windows and macOS. On Windows the
/// standard listing has neither the allocated size nor a file id, so
/// measuring like `du` would cost an open per file; the removed Windows backend lists
/// a directory with both instead. On macOS it would cost an `lstat` per
/// entry, and [`crate::macos`] lists a directory with everything a stat
/// says.
trait Listed {
    fn unavailable(&self) -> Option<i32> {
        None
    }
    /// The entry's path inside `dir`, the directory it was listed from.
    fn entry_path(&self, dir: &Path) -> PathBuf;
    /// Display name. Paths use the raw filename returned by entry_path.
    fn name(&self) -> &str;
    /// The name, moved out for the tree: after this `name` is empty, and
    /// `entry_path`, which must open the real file, is off limits.
    fn take_name(&mut self) -> Box<str>;
    fn listing(&self) -> io::Result<Listing>;
    /// Size, identity and age of a leaf.
    fn facts(&self, apparent_size: bool) -> io::Result<Facts>;
    /// What a directory is, read before the walk enters it.
    fn directory(&self) -> io::Result<Directory>;
    /// Hidden by an attribute rather than by a leading dot: on Windows,
    /// where the listing carries it, so `AppData` is hidden as Explorer
    /// hides it. Finder's `UF_HIDDEN` is not asked for: on macOS, hidden
    /// means a leading dot, as on Linux.
    fn hidden(&self) -> bool {
        false
    }
}

/// The volume observed while listing a directory.
struct Directory {
    device: u64,
}

/// What an entry is, as far as the walk is concerned.
enum Listing {
    Directory,
    Symlink,
    Leaf(NodeKind),
}

/// What a leaf contributes.
struct Facts {
    size: u64,
    identity: Option<(u64, u64)>,
    modified: i64,
    shared: bool,
    /// The file may have another name the walk could meet: its identity is
    /// worth keeping for hardlink de-duplication.
    evicted: bool,
}

impl Facts {
    #[cfg(not(target_os = "macos"))]
    fn of(meta: &Metadata, apparent_size: bool) -> Self {
        Self {
            size: measure(meta, apparent_size),
            identity: file_identity(meta),
            modified: modified_seconds(meta),
            shared: {
                use std::os::unix::fs::MetadataExt;
                meta.nlink() > 1
            },
            evicted: is_dataless(meta),
        }
    }
}

/// A standard directory entry with its name decoded once, up front.
#[cfg(not(any(windows, target_os = "macos")))]
struct Named {
    entry: DirEntry,
    name: Box<str>,
}

#[cfg(not(any(windows, target_os = "macos")))]
impl Listed for Named {
    fn entry_path(&self, dir: &Path) -> PathBuf {
        dir.join(self.entry.file_name())
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn take_name(&mut self) -> Box<str> {
        std::mem::take(&mut self.name)
    }

    fn listing(&self) -> io::Result<Listing> {
        let file_type = self.entry.file_type()?;
        Ok(if file_type.is_symlink() {
            Listing::Symlink
        } else if file_type.is_dir() {
            Listing::Directory
        } else if file_type.is_file() {
            Listing::Leaf(NodeKind::File)
        } else {
            Listing::Leaf(NodeKind::Other)
        })
    }

    fn facts(&self, apparent_size: bool) -> io::Result<Facts> {
        fs::symlink_metadata(self.entry.path()).map(|meta| Facts::of(&meta, apparent_size))
    }

    fn directory(&self) -> io::Result<Directory> {
        fs::symlink_metadata(self.entry.path()).map(|meta| Directory {
            device: device_of(&meta),
        })
    }
}

#[cfg(target_os = "macos")]
impl Listed for crate::macos::Entry {
    fn unavailable(&self) -> Option<i32> {
        self.error()
    }
    fn entry_path(&self, dir: &Path) -> PathBuf {
        self.path(dir)
    }

    fn name(&self) -> &str {
        self.name()
    }

    fn take_name(&mut self) -> Box<str> {
        self.take_name()
    }

    fn listing(&self) -> io::Result<Listing> {
        Ok(match self.kind() {
            crate::macos::Kind::Link => Listing::Symlink,
            crate::macos::Kind::Directory => Listing::Directory,
            crate::macos::Kind::File => Listing::Leaf(NodeKind::File),
            crate::macos::Kind::Other => Listing::Leaf(NodeKind::Other),
        })
    }

    fn facts(&self, apparent_size: bool) -> io::Result<Facts> {
        Ok(Facts {
            size: if apparent_size {
                self.apparent()
            } else {
                self.allocated()
            },
            identity: Some(self.identity()),
            modified: self.modified(),
            shared: self.shared(),
            evicted: self.evicted(),
        })
    }

    fn directory(&self) -> io::Result<Directory> {
        Ok(Directory {
            device: self.device(),
        })
    }
}

/// List a directory the way [`Listed`] describes.
#[cfg(not(any(windows, target_os = "macos")))]
fn list(fd: OwnedFd) -> io::Result<impl Iterator<Item = io::Result<Named>>> {
    use std::os::fd::AsRawFd;
    let directory = fs::read_dir(format!("/dev/fd/{}", fd.as_raw_fd()))?;
    Ok(directory.map(move |entry| {
        let _pinned = &fd;
        entry.map(|entry| Named {
            name: entry
                .file_name()
                .into_string()
                .map_or_else(|raw| raw.to_string_lossy().into(), String::into_boxed_str),
            entry,
        })
    }))
}

#[cfg(target_os = "macos")]
fn list(fd: OwnedFd) -> io::Result<crate::macos::ReadDir> {
    crate::macos::read_dir_fd(fd)
}

/// What a directory entry turned out to be.
enum Classified {
    /// Descend into this directory on a new task.
    Subdirectory { path: PathBuf, node: Node },
    /// A leaf that contributes size.
    Entry(Node),
    /// Filtered out, unreadable, or a symlink we chose not to follow.
    Skipped,
}

/// One directory being walked, plus the counter that decides when it is done.
#[derive(Debug)]
struct PendingDir {
    path: PathBuf,
    parent: Option<Arc<Self>>,
    /// Starts at 1 for the directory itself; one more per subdirectory task.
    /// When it reaches zero the directory is complete.
    pending: AtomicUsize,
    /// The node so far, moved out whole once the directory is complete.
    partial: Mutex<Partial>,
    read_error: AtomicBool,
    depth: usize,
}

/// What a directory's node is made of before it is built: the name the
/// listing gave it, and the files found directly in it plus the finished
/// subdirectories handed back up.
#[derive(Debug)]
struct Partial {
    node: Node,
}

impl PendingDir {
    fn new(path: PathBuf, name: Box<str>, parent: Option<Arc<Self>>, depth: usize) -> Self {
        Self {
            path,
            parent,
            pending: AtomicUsize::new(1),
            partial: Mutex::new(Partial {
                node: Node::directory(name),
            }),
            read_error: AtomicBool::new(false),
            depth,
        }
    }

    /// Turn a completed directory into a node. Only called when `pending` has
    /// reached zero, so every child is already in `self.partial`.
    ///
    /// The finishing pass derives totals after every child is present.
    fn build(&self) -> Node {
        let mut partial = lock(&self.partial);
        let mut node = std::mem::replace(&mut partial.node, Node::directory(""));
        node.raw_name = self.path.file_name().map(Into::into);
        node.read_error = self.read_error.load(Ordering::Relaxed);
        node
    }
}

fn scan_blocking(root: &Path, context: &Arc<WalkContext>) -> io::Result<Node> {
    scan_blocking_with_pool(root, context, &WALK_POOL, || {
        build_walk_pool(|builder| builder.build())
    })
}

fn scan_blocking_with_pool(
    root: &Path,
    context: &Arc<WalkContext>,
    default_pool: &WalkPoolCache,
    build_default_pool: impl FnOnce() -> io::Result<rayon::ThreadPool>,
) -> io::Result<Node> {
    let _ = context.progress.preview_root.set(root.to_path_buf());
    let owned = fixed_pool(context.options.threads)?;
    if let Some(pool) = owned {
        return scan_on_pool(root, context, &pool);
    }
    let pool = default_pool.get_or_try_init(build_default_pool)?;
    scan_on_pool(root, context, &pool)
}

/// Explicit fixed budgets cover enumeration, the Windows file-table path and
/// tree finishing. A scan owns its pool so a new setting needs no global reset.
/// Default core callers and experimental adaptive scans retain the shared pool.
fn fixed_pool(threads: ScanThreads) -> io::Result<Option<rayon::ThreadPool>> {
    if threads.adaptive || threads.max_threads == usize::MAX {
        return Ok(None);
    }
    let cpus = thread::available_parallelism().map_or(1, usize::from);
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max_threads.clamp(1, cpus))
        .thread_name(|index| format!("disktree-scan-{index}"))
        .start_handler(normal_priority)
        .build()
        .map(Some)
        .map_err(io::Error::other)
}

fn scan_on_pool(
    root: &Path,
    context: &Arc<WalkContext>,
    pool: &rayon::ThreadPool,
) -> io::Result<Node> {
    if context.options.follow_links {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Following symlinks is not supported",
        ));
    }
    let root_meta = fs::symlink_metadata(root)?;
    if !root_meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Scan root must be a directory, not a symlink",
        ));
    }
    let mut root_node = Node::directory(file_name(root));
    root_node.inode = file_identity(&root_meta).and_then(|(d, i)| Some((d, NonZeroU64::new(i)?)));
    root_node.directory_bytes = if context.options.apparent_size {
        0
    } else {
        measure(&root_meta, false)
    };
    root_node.bytes = root_node.directory_bytes;
    root_node.modified = modified_seconds(&root_meta);
    root_node.in_cloud = is_dataless(&root_meta);
    if root_node.in_cloud {
        root_node.complete = false;
        return Ok(root_node);
    }
    if context.root_fd.get().is_none() {
        let fd = rustix::fs::open(root, directory_flags(), Mode::empty())?;
        let _ = context.root_fd.set(fd);
    }
    let opened = rustix::fs::fstat(context.root_fd.get().expect("pinned scan root"))?;
    if Some((opened.st_dev as u64, opened.st_ino)) != root_node.identity() {
        return Err(io::Error::other("Root changed before scan"));
    }
    *lock(&context.root_device) = Some(device_of(&root_meta));
    let canonical = root.canonicalize()?;
    let mut never = crate::space::never_scanned(root, &canonical);
    if context.options.include_hidden && context.options.max_depth.is_none() {
        never.extend(crate::space::repeated_mounts_for(
            root,
            &canonical,
            context.options.one_filesystem,
        ));
    }
    let _ = context.never_scanned.set(never.into_iter().collect());
    if context.options.one_filesystem
        && let Some(foreign) = crate::space::mount_boundaries_for(&canonical)
    {
        let _ = context.foreign_mounts.set(foreign.into_iter().collect());
    }
    context.reserve(1024usize.saturating_add(root.as_os_str().len().saturating_mul(8)));
    let root_dir = Arc::new(PendingDir::new(
        root.to_path_buf(),
        file_name(root),
        None,
        0,
    ));
    lock(&root_dir.partial).node = root_node;
    let context: &WalkContext = context;
    pool.install(|| {
        let mut threads = context.options.threads.normalized();
        threads.max_threads = threads.max_threads.min(rayon::current_num_threads());
        context
            .progress
            .threads
            .store(threads.max_threads, Ordering::Relaxed);
        context
            .progress
            .threads_settled
            .store(true, Ordering::Relaxed);
        context
            .progress
            .thread_tuning_complete
            .store(!threads.adaptive, Ordering::Relaxed);
        if !threads.adaptive && threads.max_threads == rayon::current_num_threads() {
            // Pool-sized fixed scans need no admission lock or ready queue. Keep
            // existing core callers on Rayon's direct scheduling path.
            rayon::scope(|scope| {
                dispatch_fixed(scope, &root_dir, context);
            });
        } else {
            let admission = Arc::new(Admission::new(root_dir, threads.max_threads));
            // The controller owns no Rayon worker and is joined before finishing
            // the tree, where entry throughput no longer describes useful work.
            let _controller = ControllerGuard::spawn(
                threads,
                Arc::clone(&admission),
                Arc::clone(&context.progress),
            )?;
            rayon::scope(|scope| {
                dispatch(scope, admission.start(), context, &admission);
            });
        }
        Ok::<(), io::Error>(())
    })?;

    let node = lock(&context.root).take();
    let node =
        node.ok_or_else(|| io::Error::other(format!("{} produced no tree", root.display())))?;
    let mut node = finish_tree(node, &context.options, pool);
    if context.progress.limited.load(Ordering::Relaxed) || context.progress.is_cancelled() {
        node.complete = false;
    }
    Ok(node)
}

/// The walk's workers, and the file table reader's. Listing directories
/// contends in the file system, and past some number of threads more of
/// them only queue: on Windows a home directory of 3.4 million files
/// walked in 6.5 s on 16 threads and 8.1 s on all 24. That cap was
/// measured on one 24-thread NTFS laptop; another disk or CPU may want
/// another. Elsewhere the default count. A pool of its own on every
/// platform all the same: work the UI hands the global pool, such as
/// re-ranking a tree, must not queue behind a scan's blocking reads.
#[derive(Default)]
struct WalkPoolCache(Mutex<Option<Arc<rayon::ThreadPool>>>);

impl WalkPoolCache {
    fn get_or_try_init(
        &self,
        build: impl FnOnce() -> io::Result<rayon::ThreadPool>,
    ) -> io::Result<Arc<rayon::ThreadPool>> {
        let mut cache = lock(&self.0);
        if let Some(pool) = cache.as_ref() {
            return Ok(Arc::clone(pool));
        }
        let built = Arc::new(build()?);
        *cache = Some(Arc::clone(&built));
        Ok(built)
    }
}

static WALK_POOL: WalkPoolCache = WalkPoolCache(Mutex::new(None));

fn build_walk_pool(
    build: impl FnOnce(
        rayon::ThreadPoolBuilder,
    ) -> Result<rayon::ThreadPool, rayon::ThreadPoolBuildError>,
) -> io::Result<rayon::ThreadPool> {
    let builder = rayon::ThreadPoolBuilder::new()
        .thread_name(|index| format!("disktree-walk-{index}"))
        .start_handler(normal_priority);
    #[cfg(windows)]
    let builder = builder.num_threads(
        thread::available_parallelism()
            .map_or(1, usize::from)
            .min(16),
    );
    build(builder).map_err(io::Error::other)
}

/// Entries counted before the shared progress counters are touched. Every
/// worker adding to the same counters per entry keeps one cache line
/// bouncing between cores; per directory, or per this many entries in a
/// wide one so the meter still moves, is invisible to a reader.
const TALLY_EVERY: u64 = 1024;

/// What one directory's listing has found so far, for the progress meter.
#[derive(Default)]
struct Tally {
    files: u64,
    dirs: u64,
    bytes: u64,
}

impl Tally {
    const fn add(&mut self, node: &Node) {
        if node.is_dir() {
            // A subtree taken whole from an earlier scan.
            self.files += node.files;
            self.dirs += node.dirs as u64;
        } else {
            // A link counts as a file on the meter, as it always has,
            // though the tree does not count it.
            self.files += 1;
        }
        self.bytes += node.bytes;
    }

    fn flush(&mut self, progress: &ScanProgress, path: &Path) {
        progress.preview_add(path, self.bytes, self.files);
        progress.add(self.files, self.dirs, self.bytes);
        *self = Self::default();
    }
}

/// Read one directory, spawn a task per subdirectory, then report completion.
///
/// Flat tasks inside the scope: the worker stack never grows with tree depth.
fn dispatch_fixed<'scope>(
    scope: &Scope<'scope>,
    dir: &Arc<PendingDir>,
    context: &'scope WalkContext,
) {
    for child in walk(dir, context) {
        scope.spawn(move |scope| {
            dispatch_fixed(scope, &child, context);
        });
    }
}

/// Read one directory, spawn a task per subdirectory, then report completion.
///
/// Flat tasks inside the scope: the worker stack never grows with tree depth.
fn dispatch<'scope>(
    scope: &Scope<'scope>,
    jobs: Vec<Arc<PendingDir>>,
    context: &'scope WalkContext,
    admission: &Arc<Admission<Arc<PendingDir>>>,
) {
    for dir in jobs {
        let admission = Arc::clone(admission);
        scope.spawn(move |scope| {
            let mut next = Some(dir);
            while let Some(dir) = next.take() {
                let children = walk(&dir, context);
                let mut jobs = admission.complete(children);
                // Continue locally into one ready descendant; only surplus
                // jobs are offered to other workers. A running directory is
                // never split, and a retirement returns from this loop.
                next = jobs.pop();
                dispatch(scope, jobs, context, &admission);
            }
        });
    }
}

fn walk(dir: &Arc<PendingDir>, context: &WalkContext) -> Vec<Arc<PendingDir>> {
    if context.cancelled() {
        lock(&dir.partial).node.complete = false;
        signal_done(dir, context);
        return Vec::new();
    }
    let listing = open_pending(dir, context).and_then(list);
    walk_listing(dir, context, listing)
}

fn walk_listing(
    dir: &Arc<PendingDir>,
    context: &WalkContext,
    listing: io::Result<impl Iterator<Item = io::Result<impl Listed>>>,
) -> Vec<Arc<PendingDir>> {
    let mut subdirs = Vec::new();
    let mut tally = Tally {
        bytes: lock(&dir.partial).node.directory_bytes,
        ..Tally::default()
    };
    match listing {
        Ok(entries) => {
            for entry in entries {
                if context.cancelled() {
                    lock(&dir.partial).node.complete = false;
                    break;
                }
                let classified =
                    entry.and_then(|mut entry| context.classify(&dir.path, &mut entry));
                match classified {
                    Ok(Classified::Subdirectory { path, node }) => {
                        tally.dirs += 1;
                        if dir.depth >= context.options.max_depth.unwrap_or(256).min(256) {
                            let mut node = node;
                            node.complete = false;
                            node.read_error = true;
                            context.progress.record_error(
                                &path,
                                &io::Error::other("Directory depth limit reached"),
                            );
                            lock(&dir.partial).node.children.push(node);
                        } else {
                            let child = Arc::new(PendingDir::new(
                                path,
                                node.name.clone(),
                                Some(Arc::clone(dir)),
                                dir.depth + 1,
                            ));
                            lock(&child.partial).node = node;
                            dir.pending.fetch_add(1, Ordering::AcqRel);
                            subdirs.push(child);
                        }
                    }
                    Ok(Classified::Entry(node)) => {
                        tally.add(&node);
                        lock(&dir.partial).node.children.push(node);
                    }
                    Ok(Classified::Skipped) => {}
                    Err(error) => {
                        context.progress.record_error(&dir.path, &error);
                        let mut partial = lock(&dir.partial);
                        partial.node.entry_errors = partial.node.entry_errors.saturating_add(1);
                        partial.node.complete = false;
                    }
                }
                if tally.files + tally.dirs >= TALLY_EVERY {
                    tally.flush(&context.progress, &dir.path);
                }
            }
        }
        Err(error) => {
            context.progress.record_error(&dir.path, &error);
            dir.read_error.store(true, Ordering::Relaxed);
        }
    }
    if context.cancelled() {
        lock(&dir.partial).node.complete = false;
    }
    tally.flush(&context.progress, &dir.path);
    signal_done(dir, context);
    subdirs
}

fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

fn open_pending(dir: &Arc<PendingDir>, context: &WalkContext) -> io::Result<OwnedFd> {
    let mut chain = Vec::new();
    let mut current = dir;
    while let Some(parent) = &current.parent {
        chain.push(current);
        current = parent;
    }
    let root = context
        .root_fd
        .get()
        .ok_or_else(|| io::Error::other("No scan root descriptor"))?;
    let mut fd = rustix::io::dup(root)?;
    for part in chain.into_iter().rev() {
        if context.cancelled() {
            return Err(io::Error::other("Scan stopped"));
        }
        let partial = lock(&part.partial);
        let name = partial.node.filesystem_name();
        let stat = rustix::fs::statat(&fd, name, AtFlags::SYMLINK_NOFOLLOW)?;
        #[cfg(target_os = "macos")]
        if stat.st_flags & 0x40000000 != 0 {
            return Err(io::Error::other("Directory moved to cloud"));
        }
        let expected = partial.node.identity();
        let identity = (stat.st_dev as u64, stat.st_ino);
        if expected != Some(identity) {
            return Err(io::Error::other("Directory changed during scan"));
        }
        let next = rustix::fs::openat(&fd, name, directory_flags(), Mode::empty())?;
        let opened = rustix::fs::fstat(&next)?;
        if (opened.st_dev as u64, opened.st_ino) != identity {
            return Err(io::Error::other("Directory changed while opening"));
        }
        fd = next;
    }
    Ok(fd)
}

fn retained_charge(node: &Node, path_bytes: usize) -> usize {
    node.children.iter().fold(
        1024usize.saturating_add(path_bytes.saturating_mul(8)),
        |sum, child| {
            sum.saturating_add(retained_charge(
                child,
                path_bytes.saturating_add(child.filesystem_name().len() + 1),
            ))
        },
    )
}

/// Report that one task for `dir` is done: either its own scan, or one of its
/// children. The last one to report builds the node and bubbles up.
///
/// This is the whole reason for the `+1` sentinel: a directory is only built
/// once its own scan *and* every subdirectory task has finished, no matter
/// which of them lands last.
fn signal_done(dir: &Arc<PendingDir>, context: &WalkContext) {
    if dir.pending.fetch_sub(1, Ordering::AcqRel) != 1 {
        return;
    }
    let mut node = dir.build();
    node.complete &= !node.read_error;
    for child in &node.children {
        node.complete &= child.complete;
    }
    if dir.depth == 1 && node.complete {
        publish_completed(
            &context.progress,
            &dir.path,
            &node,
            &context.options,
            |snapshot| finish_snapshot(snapshot, &context.options),
        );
    }
    let Some(parent) = &dir.parent else {
        *lock(&context.root) = Some(node);
        return;
    };
    lock(&parent.partial).node.children.push(node);
    signal_done(parent, context);
}

fn publish_completed(
    progress: &ScanProgress,
    path: &Path,
    node: &Node,
    options: &ScanOptions,
    finish: impl FnOnce(&mut Node),
) {
    if lock(&progress.completed).is_some() {
        return;
    }
    let charge = retained_charge(node, path.as_os_str().len());
    if progress
        .retained_bytes
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
            used.checked_add(charge)
                .filter(|next| *next <= options.max_retained_bytes / 2)
        })
        .is_err()
    {
        return;
    }
    let mut snapshot = node.clone();
    finish(&mut snapshot);
    let mut completed = lock(&progress.completed);
    if completed.is_none() {
        *completed = Some((path.into(), Arc::new(snapshot)));
    }
}

/// Charge a hardlinked file once, derive every aggregate from the result,
/// then classify. On the walk's pool: the UI's own work on the global pool
/// must not queue behind a scan finishing.
fn finish_tree(mut node: Node, options: &ScanOptions, pool: &rayon::ThreadPool) -> Node {
    pool.install(|| {
        if options.dedup_hardlinks {
            aggregate_deduped(&mut node, options.metric, &Seen::new());
        } else {
            aggregate(&mut node, options.metric);
        }
        crate::classify::classify(&mut node);
    });
    node
}

/// A leaf retains its observed identity independently of hardlink accounting.
fn leaf_node(name: Box<str>, kind: NodeKind, facts: &Facts) -> Node {
    let mut node = Node::entry(name, kind, facts.size);
    node.inode = facts
        .identity
        .and_then(|(device, file)| Some((device, NonZeroU64::new(file)?)));
    node.modified = facts.modified;
    node.hardlinked = facts.shared;
    node
}

fn finish_snapshot(node: &mut Node, options: &ScanOptions) {
    if options.dedup_hardlinks {
        aggregate_deduped(node, options.metric, &Seen::new());
    } else {
        aggregate(node, options.metric);
    }
    crate::classify::classify(node);
}

fn normal_priority(_: usize) {
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_DEFAULT, 0);
    }
}

/// Last write time in Unix seconds, from the metadata the walk already has:
/// age costs no extra system call.
pub(crate) fn modified_seconds(meta: &Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

pub(crate) fn measure(meta: &Metadata, apparent_size: bool) -> u64 {
    if apparent_size {
        meta.len()
    } else {
        allocated_bytes(meta).unwrap_or(meta.len())
    }
}

/// Allocated bytes, `st_blocks * 512`: sparse files spend less than they claim,
/// and this is the number that matches `du`.
///
/// Returns `Option` because only Unix reports allocated blocks; elsewhere the
/// caller falls back to the apparent length.
#[allow(
    clippy::unnecessary_wraps,
    reason = "the Option is the non-Unix answer, where the caller falls back"
)]
#[cfg(unix)]
fn allocated_bytes(meta: &Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt as _;
    Some(meta.blocks().saturating_mul(512))
}

#[cfg(not(unix))]
const fn allocated_bytes(_meta: &Metadata) -> Option<u64> {
    None
}

/// Whether a directory's contents are only in the cloud: an iCloud Drive or
/// File Provider folder macOS has evicted. Listing one makes macOS fetch its
/// entries from the server, so a disk scan must not; the folder holds no
/// local blocks, so skipping it loses nothing a scan measures.
///
/// A `stat` or `lstat` carries the flag without touching the contents.
#[cfg(target_os = "macos")]
fn is_dataless(meta: &Metadata) -> bool {
    use std::os::macos::fs::MetadataExt as _;
    // `SF_DATALESS` from <sys/stat.h>; the libc crate does not name it.
    const SF_DATALESS: u32 = 0x4000_0000;
    meta.st_flags() & SF_DATALESS != 0
}

#[cfg(not(target_os = "macos"))]
const fn is_dataless(_meta: &Metadata) -> bool {
    false
}

#[cfg(unix)]
fn device_of(meta: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    meta.dev()
}

#[cfg(not(unix))]
const fn device_of(_meta: &Metadata) -> u64 {
    0
}

/// `(device, inode)`, or `None` where the platform does not expose them.
/// Hardlink de-duplication and symlink loop detection both depend on this, so
/// they are simply unavailable rather than silently wrong elsewhere. Windows
/// has no file id in `Metadata` on stable Rust; its listing carries one
/// instead (see the removed Windows backend).
#[allow(
    clippy::unnecessary_wraps,
    reason = "the Option is the non-Unix answer, so both arms must agree"
)]
#[cfg(unix)]
fn file_identity(meta: &Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
const fn file_identity(_meta: &Metadata) -> Option<(u64, u64)> {
    None
}

/// The display name of a scanned root: its final component, or the path itself
/// for `/`.
pub(crate) fn file_name(path: &Path) -> Box<str> {
    path.file_name()
        .map_or_else(
            || path.to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
        .into()
}

/// A poisoned lock means another task panicked; the data is still a valid tree
/// prefix, and refusing to read it would turn one panic into two.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::Metric;
    use std::fs;
    use tempfile::TempDir;

    /// Apparent sizes, so the assertions are about the tree rather than about
    /// how the filesystem rounds a small file up to a block.
    fn options() -> ScanOptions {
        ScanOptions {
            apparent_size: true,
            ..ScanOptions::default()
        }
    }

    fn write(root: &Path, relative: &str, bytes: usize) -> PathBuf {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, vec![b'x'; bytes]).expect("write");
        path
    }

    fn scan_dir(root: &Path, options: &ScanOptions) -> Node {
        scan(root, options.clone()).expect("scan")
    }

    fn child<'a>(node: &'a Node, name: &str) -> &'a Node {
        node.children
            .iter()
            .find(|child| &*child.name == name)
            .unwrap_or_else(|| panic!("no child named {name} in {:?}", node.name))
    }

    #[test]
    fn fixed_worker_pools_change_size_without_reinitializing_rayon() {
        let cpus = thread::available_parallelism().map_or(1, usize::from);
        // Two live pools have independent limits; constructing the first must
        // not freeze the choice for a later scan or the global UI pool.
        let make = |count| {
            fixed_pool(ScanThreads {
                max_threads: count,
                ..ScanThreads::default()
            })
            .expect("pool")
            .expect("explicit fixed pool")
        };
        let small = make(1);
        let large = make(4);
        assert_eq!(small.install(rayon::current_num_threads), 1);
        assert_eq!(large.install(rayon::current_num_threads), 4.min(cpus));
        assert!(
            fixed_pool(ScanThreads::default())
                .expect("default")
                .is_none()
        );
        assert!(
            fixed_pool(ScanThreads {
                adaptive: true,
                max_threads: 4,
                ..ScanThreads::default()
            })
            .expect("adaptive")
            .is_none()
        );
    }

    #[test]
    fn fixed_worker_pools_preserve_a_wide_deep_tree() {
        let temp = TempDir::new().expect("tempdir");
        for index in 0..96 {
            write(temp.path(), &format!("d{index}/nested/file"), index + 1);
        }
        for threads in [1, 2, 4, 8, usize::MAX] {
            let mut settings = options();
            settings.threads.max_threads = threads;
            let tree = scan_dir(temp.path(), &settings);
            assert_eq!(tree.files, 96);
            assert_eq!(tree.dirs, 193);
            assert_eq!(tree.bytes, (1..=96).sum::<u64>());
        }
    }

    #[test]
    fn totals_are_summed_bottom_up() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        write(root, "a/one.bin", 1000);
        write(root, "a/nested/two.bin", 2000);
        write(root, "b/three.bin", 500);

        let tree = scan_dir(root, &options());
        assert_eq!(tree.bytes, 3500);
        assert_eq!(tree.files, 3);
        assert_eq!(tree.dirs, 4, "root, a, a/nested, b");
        assert_eq!(child(&tree, "a").bytes, 3000);
        assert_eq!(child(child(&tree, "a"), "nested").bytes, 2000);
        assert_eq!(child(&tree, "b").bytes, 500);
    }

    #[test]
    fn children_are_ranked_largest_first() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        write(root, "small.bin", 10);
        write(root, "large.bin", 1000);
        write(root, "medium.bin", 100);

        let tree = scan_dir(root, &options());
        let names: Vec<&str> = tree.children.iter().map(|c| &*c.name).collect();
        assert_eq!(names, vec!["large.bin", "medium.bin", "small.bin"]);
    }

    #[test]
    fn a_directory_reports_its_own_bytes_separately() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        write(root, "direct.bin", 700);
        write(root, "sub/deep.bin", 300);

        let tree = scan_dir(root, &options());
        assert_eq!(tree.own_bytes(), 700);
        assert_eq!(tree.own_files(), 1);
        assert_eq!(tree.bytes, 1000);
        assert_eq!(tree.files, 2);
    }

    #[test]
    fn hardlinks_are_charged_once_by_default() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        let original = write(root, "original.bin", 4096);
        fs::hard_link(&original, root.join("link.bin")).expect("hard link");

        let deduped = scan_dir(root, &options());
        assert_eq!(deduped.bytes, 4096);
        assert_eq!(deduped.files, 2, "both names still exist");

        let counted = scan_dir(
            root,
            &ScanOptions {
                dedup_hardlinks: false,
                ..options()
            },
        );
        assert_eq!(counted.bytes, 8192);
    }

    #[test]
    fn hidden_entries_are_included_by_default_and_excluded_on_request() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        write(root, ".cache/blob.bin", 900);
        write(root, "visible.bin", 100);

        let default = scan_dir(root, &options());
        assert_eq!(default.bytes, 1000);
        assert_eq!(child(&default, ".cache").bytes, 900);

        let without = scan_dir(
            root,
            &ScanOptions {
                include_hidden: false,
                ..options()
            },
        );
        assert_eq!(without.bytes, 100);
    }

    /// A link to a directory: a symbolic link on Unix, a junction on
    /// Windows, which needs no privilege to make. `false` if it could not be
    /// made.
    fn link_dir(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
    }

    #[test]
    fn symlinks_are_not_followed_by_default() {
        let temp = TempDir::new().expect("tempdir");
        let outside = TempDir::new().expect("tempdir");
        write(outside.path(), "elsewhere.bin", 5000);
        let root = temp.path();
        write(root, "real.bin", 100);
        assert!(link_dir(outside.path(), &root.join("link")), "a link");

        let tree = scan_dir(root, &options());
        // Only the real file's bytes; the link itself holds just its target
        // string, and its 5000-byte destination is outside this tree.
        assert!(tree.bytes < 200, "{}", tree.bytes);
        let link = child(&tree, "link");
        assert_eq!(link.kind, NodeKind::Symlink);
        assert!(link.bytes < 100, "a link holds only its target string");
    }

    #[test]
    fn following_links_is_rejected() {
        let temp = TempDir::new().unwrap();
        let error = scan(
            temp.path(),
            ScanOptions {
                follow_links: true,
                ..options()
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    /// Disk usage is what the volume allocated, not the length: whole
    /// blocks, so it covers the data and ends on a block boundary.
    #[test]
    fn disk_usage_is_whole_allocation_units() {
        let temp = TempDir::new().expect("tempdir");
        // Noise, so a compressing filesystem cannot store it in less.
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let noise: Vec<u8> = (0..100_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state.to_le_bytes()[0]
            })
            .collect();
        fs::write(temp.path().join("data.bin"), noise).expect("write");
        let tree = scan_dir(temp.path(), &ScanOptions::default());
        assert!(tree.bytes >= 100_000, "{}", tree.bytes);
        assert_eq!(tree.bytes % 512, 0, "{}", tree.bytes);
        let apparent = scan_dir(temp.path(), &options());
        assert_eq!(apparent.bytes, 100_000);
    }

    #[test]
    fn max_depth_stops_descending_but_keeps_direct_files() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        write(root, "here.bin", 10);
        write(root, "a/there.bin", 20);
        write(root, "a/b/far.bin", 30);

        let tree = scan_dir(
            root,
            &ScanOptions {
                max_depth: Some(1),
                ..options()
            },
        );
        assert_eq!(tree.own_bytes(), 10);
        let a = child(&tree, "a");
        assert_eq!(a.bytes, 20, "direct files at the cut-off depth still count");
        assert!(
            a.children
                .iter()
                .filter(|child| child.is_dir())
                .all(|child| !child.complete),
            "descending stopped at depth 1"
        );
        assert_eq!(a.files, 1);
    }

    #[test]
    fn file_count_metric_ranks_by_entries() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        for index in 0..5 {
            write(root, &format!("many/f{index}.bin"), 1);
        }
        write(root, "one/huge.bin", 100_000);

        let tree = scan_dir(root, &options());
        assert_eq!(child(&tree, "one").bytes, 100_000);
        assert_eq!(tree.children[0].name.as_ref(), "one");

        let by_files = scan_dir(
            root,
            &ScanOptions {
                metric: Metric::Files,
                ..options()
            },
        );
        assert_eq!(by_files.children[0].name.as_ref(), "many");
        assert_eq!(by_files.children[0].files, 5);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_directory_is_recorded_not_fatal() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        write(root, "readable.bin", 11);
        let locked = root.join("locked");
        fs::create_dir_all(&locked).expect("mkdir");
        write(&locked, "hidden.bin", 22);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod");

        struct RestorePermissions(PathBuf);
        impl Drop for RestorePermissions {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
            }
        }
        let _restore = RestorePermissions(locked.clone());
        assert_ne!(
            unsafe { libc::geteuid() },
            0,
            "permission test requires an unprivileged user"
        );
        let denied = fs::read_dir(&locked).expect_err("fixture directory must deny enumeration");
        assert_eq!(denied.kind(), io::ErrorKind::PermissionDenied);
        let progress = Arc::new(ScanProgress::default());
        let context = Arc::new(WalkContext {
            known: None,
            options: options(),
            progress: Arc::clone(&progress),
            root_device: Mutex::new(None),
            foreign_mounts: OnceLock::new(),
            never_scanned: OnceLock::new(),
            root: Mutex::new(None),
            root_fd: OnceLock::new(),
        });
        let tree = scan_blocking(root, &context).expect("scan still succeeds");
        assert_eq!(tree.bytes, 11);
        let node = child(&tree, "locked");
        assert!(node.read_error);
        assert_eq!(node.unreadable, 1);
        assert_eq!(tree.unreadable, 1);
        assert!(!node.complete);
        assert!(!tree.complete);
        assert_eq!(progress.snapshot().errors, 1);
        assert!(!progress.snapshot().messages.is_empty());
    }

    #[test]
    fn a_file_root_is_rejected() {
        let temp = TempDir::new().expect("tempdir");
        let path = write(temp.path(), "file.bin", 1);
        let error = scan(&path, options()).expect_err("a file is not a tree");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_spawned_scan_reports_progress_and_a_result() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        write(root, "a/one.bin", 1024);
        write(root, "b/two.bin", 2048);

        let handle = ScanHandle::spawn(root.to_path_buf(), options());
        let mut outcome = None;
        for _ in 0..2000 {
            if let Some(result) = handle.poll() {
                outcome = Some(result);
                break;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }
        let tree = outcome.expect("the scan finished").expect("a tree");
        assert_eq!(tree.bytes, 3072);

        let snapshot = handle.progress.snapshot();
        assert!(snapshot.finished);
        assert_eq!(snapshot.files, 2);
        assert_eq!(snapshot.errors, 0);
        let preview = handle.progress.preview();
        assert!(
            preview
                .entries
                .iter()
                .any(|e| e.name == "a" && e.bytes == 1024)
        );
        assert!(
            preview
                .entries
                .iter()
                .any(|e| e.name == "b" && e.bytes == 2048)
        );
        assert_eq!(
            preview.entries.iter().map(|e| e.bytes).sum::<u64>(),
            tree.bytes
        );
    }

    #[test]
    fn a_wider_scan_reuses_the_subtree_it_already_knows() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().canonicalize().expect("canonical");
        let inner = root.join("inner");
        fs::create_dir_all(inner.join("deep")).expect("mkdir");
        fs::write(inner.join("deep/a.bin"), vec![0_u8; 4096]).expect("write");
        fs::write(root.join("outside.bin"), vec![0_u8; 8192]).expect("write");
        let known = scan(&inner, options()).expect("inner scan");

        // Changed on disk after the inner scan: a memoized subtree must not
        // see it, which is how this test knows the walk skipped it.
        fs::write(inner.join("deep/b.bin"), vec![0_u8; 4096]).expect("write");

        let handle = ScanHandle::spawn_with(
            root,
            options(),
            Some(Known {
                path: inner,
                tree: Arc::new(known.clone()),
                options: options(),
            }),
        );
        let tree = loop {
            if let Some(outcome) = handle.poll() {
                break outcome.expect("wide scan");
            }
            thread::sleep(std::time::Duration::from_millis(2));
        };
        let reused = tree.child_named("inner").expect("inner is in the tree");
        assert_eq!(reused.files, known.files, "b.bin was never read");
        assert_eq!(reused.bytes, known.bytes);
        assert!(
            tree.child_named("outside.bin").is_some(),
            "the rest was walked"
        );
        assert_eq!(tree.files, known.files + 1);
    }

    /// A real whole-disk scan, run by hand: `cargo test -p disktree-core
    /// -- --ignored --nocapture whole_disk`. Prints what it found, so the
    /// volume rules can be checked against this machine's mounts.
    #[test]
    #[ignore = "walks the whole disk"]
    fn whole_disk_smoke() {
        let home = std::env::home_dir().expect("a home directory");
        let root = crate::space::volume_root_for(&home).expect("a volume root");
        let started = std::time::Instant::now();
        let tree = scan(&root, ScanOptions::default()).expect("scan");
        println!("root {} in {:.1?}", root.display(), started.elapsed());
        println!(
            "total {} files {}",
            crate::size::human_bytes(tree.bytes),
            tree.files
        );
        for child in tree.children.iter().take(14) {
            println!(
                "  {:>10}  {}",
                crate::size::human_bytes(child.bytes),
                child.name
            );
        }
        let skipped = crate::space::foreign_mounts_for(&root).unwrap_or_default();
        println!("left out: {skipped:?}");
        for needle in ["c", "cache", "node_modules", "zzzz"] {
            let started = std::time::Instant::now();
            let found = crate::filter::filter(&tree, &[], needle).expect("needle");
            println!(
                "filter {needle:?}: {} matches, {} in {:.1?}",
                found.count,
                crate::size::human_bytes(found.bytes),
                started.elapsed()
            );
        }
    }
}

#[cfg(test)]
mod parity_tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, symlink};

    fn walk_pool() -> Arc<rayon::ThreadPool> {
        WALK_POOL
            .get_or_try_init(|| build_walk_pool(|builder| builder.build()))
            .unwrap()
    }

    fn pending(root: &Path) -> (Arc<WalkContext>, Arc<PendingDir>) {
        let context = Arc::new(WalkContext {
            known: None,
            options: ScanOptions {
                apparent_size: true,
                ..ScanOptions::default()
            },
            progress: Arc::default(),
            root_device: Mutex::new(None),
            foreign_mounts: OnceLock::new(),
            never_scanned: OnceLock::new(),
            root: Mutex::new(None),
            root_fd: OnceLock::new(),
        });
        let fd = rustix::fs::open(root, directory_flags(), Mode::empty()).unwrap();
        context.root_fd.set(fd).unwrap();
        let meta = fs::symlink_metadata(root).unwrap();
        *lock(&context.root_device) = Some(meta.dev());
        let dir = Arc::new(PendingDir::new(root.into(), file_name(root), None, 0));
        lock(&dir.partial).node.inode = Some((meta.dev(), NonZeroU64::new(meta.ino()).unwrap()));
        (context, dir)
    }

    #[test]
    fn symlink_and_directory_replacements_are_not_traversed() {
        for link in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            fs::create_dir(temp.path().join("child")).unwrap();
            fs::write(outside.path().join("secret"), [7; 8192]).unwrap();
            let (context, root) = pending(temp.path());
            let children = walk(&root, &context);
            assert_eq!(children.len(), 1);
            fs::rename(temp.path().join("child"), temp.path().join("original")).unwrap();
            if link {
                symlink(outside.path(), temp.path().join("child")).unwrap();
            } else {
                fs::create_dir(temp.path().join("child")).unwrap();
                fs::write(temp.path().join("child/secret"), [7; 8192]).unwrap();
            }
            assert!(walk(&children[0], &context).is_empty());
            let node = finish_tree(
                lock(&context.root).take().unwrap(),
                &context.options,
                &walk_pool(),
            );
            assert_eq!(node.bytes, 0);
            assert_eq!(node.unreadable, 1);
            assert!(!node.complete);
            assert!(node.children[0].children.is_empty());
        }
    }

    #[test]
    fn ancestor_replacement_cannot_redirect_a_queued_descendant() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("a/b")).unwrap();
        let (context, root) = pending(temp.path());
        let a = walk(&root, &context).pop().unwrap();
        let b = walk(&a, &context).pop().unwrap();
        fs::rename(temp.path().join("a"), temp.path().join("saved")).unwrap();
        fs::create_dir_all(temp.path().join("a/b")).unwrap();
        fs::write(temp.path().join("a/b/foreign"), [7; 8192]).unwrap();
        walk(&b, &context);
        let tree = finish_tree(
            lock(&context.root).take().unwrap(),
            &context.options,
            &walk_pool(),
        );
        assert_eq!(tree.bytes, 0);
        assert_eq!(tree.unreadable, 1);
        assert!(!tree.complete);
    }

    #[test]
    fn completed_subtree_survives_cancellation_with_real_identity_and_totals() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["a", "b"] {
            fs::create_dir(temp.path().join(name)).unwrap();
            fs::write(temp.path().join(name).join("data"), [1; 4096]).unwrap();
        }
        let (context, root) = pending(temp.path());
        let children = walk(&root, &context);
        assert_eq!(children.len(), 2);
        walk(&children[0], &context);
        let (path, complete) = context
            .progress
            .take_completed()
            .expect("real subtree published");
        assert_eq!(complete.bytes, 4096);
        assert_eq!(complete.files, 1);
        assert!(complete.complete);
        assert_eq!(
            complete.identity(),
            file_identity(&fs::symlink_metadata(path).unwrap())
        );
        assert!(complete.children[0].identity().is_some());
        context.progress.cancel();
        walk(&children[1], &context);
        let tree = finish_tree(
            lock(&context.root).take().unwrap(),
            &context.options,
            &walk_pool(),
        );
        assert_eq!(tree.bytes, 4096);
        assert_eq!(tree.children.len(), 2);
        assert!(!tree.complete);
        let snapshot = context.progress.snapshot();
        assert!(snapshot.cancelled);
        assert_eq!(snapshot.files, 1);
    }

    #[test]
    fn default_pool_spawn_error_returns_an_error_without_unwinding() {
        let result = build_walk_pool(|builder| {
            builder
                .spawn_handler(|_| Err(io::Error::other("injected default pool spawn failure")))
                .build()
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("injected default pool spawn failure")
        );
    }

    #[test]
    fn default_pool_recovers_after_transient_build_failure() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("visible"), [1; 16]).unwrap();
        let cache = WalkPoolCache::default();
        let calls = AtomicUsize::new(0);
        fn scan(
            root: &Path,
            cache: &WalkPoolCache,
            build_default_pool: impl FnOnce() -> io::Result<rayon::ThreadPool>,
        ) -> io::Result<Node> {
            let context = Arc::new(WalkContext {
                known: None,
                options: ScanOptions::default(),
                progress: Arc::default(),
                root_device: Mutex::new(None),
                foreign_mounts: OnceLock::new(),
                never_scanned: OnceLock::new(),
                root: Mutex::new(None),
                root_fd: OnceLock::new(),
            });
            scan_blocking_with_pool(root, &context, cache, build_default_pool)
        }

        let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            scan(temp.path(), &cache, || {
                calls.fetch_add(1, Ordering::Relaxed);
                build_walk_pool(|builder| {
                    builder
                        .spawn_handler(|_| {
                            Err(io::Error::other("injected one-time pool start failure"))
                        })
                        .build()
                })
            })
        }))
        .expect("a pool start error must not unwind the scan caller");
        assert!(first.is_err());

        let second = scan(temp.path(), &cache, || {
            calls.fetch_add(1, Ordering::Relaxed);
            build_walk_pool(|builder| builder.build())
        })
        .expect("a later default scan must retry pool construction");

        assert_eq!(calls.load(Ordering::Relaxed), 2);
        assert!(second.complete);
        assert_eq!(second.files, 1);
        assert_eq!(
            second.children[0].filesystem_name(),
            std::ffi::OsStr::new("visible")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn compact_unavailable_record_survives_the_core_walk() {
        let temp = tempfile::tempdir().unwrap();
        let (context, root) = pending(temp.path());
        let listing = crate::macos::tests::unavailable_listing(temp.path());
        assert!(walk_listing(&root, &context, Ok(listing)).is_empty());
        let tree = finish_tree(
            lock(&context.root).take().unwrap(),
            &context.options,
            &walk_pool(),
        );
        assert_eq!(tree.children.len(), 1);
        let child = &tree.children[0];
        assert_eq!(child.filesystem_name(), std::ffi::OsStr::new("x"));
        assert_eq!(child.bytes, 0);
        assert!(child.read_error);
        assert_eq!(child.unreadable, 1);
        assert_eq!(tree.unreadable, 1);
        assert!(!tree.complete);
        assert_eq!(context.progress.snapshot().errors, 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unavailable_observed_directory_is_not_traversed_or_charged() {
        use std::os::unix::ffi::OsStrExt;
        for fallback in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let (context, root) = pending(temp.path());
            let listing = crate::macos::tests::observed_unavailable_listing(temp.path(), fallback);
            assert!(walk_listing(&root, &context, Ok(listing)).is_empty());
            let tree = finish_tree(
                lock(&context.root).take().unwrap(),
                &context.options,
                &walk_pool(),
            );
            assert_eq!(tree.children.len(), 1);
            let child = &tree.children[0];
            assert_eq!(child.filesystem_name().as_bytes(), &[0xff]);
            assert_eq!(child.identity(), Some((7, 42)));
            assert!(child.in_cloud && child.read_error);
            assert_eq!(child.bytes, 0);
            assert_eq!(tree.unreadable, 1);
            assert!(!tree.complete);
        }
    }

    struct Cloud {
        directory: bool,
    }
    impl Listed for Cloud {
        fn entry_path(&self, dir: &Path) -> PathBuf {
            dir.join("evicted")
        }
        fn name(&self) -> &str {
            "evicted"
        }
        fn take_name(&mut self) -> Box<str> {
            "evicted".into()
        }
        fn listing(&self) -> io::Result<Listing> {
            Ok(if self.directory {
                Listing::Directory
            } else {
                Listing::Leaf(NodeKind::File)
            })
        }
        fn facts(&self, _: bool) -> io::Result<Facts> {
            Ok(Facts {
                size: 512,
                identity: Some((7, 42)),
                modified: 1,
                shared: false,
                evicted: true,
            })
        }
        fn directory(&self) -> io::Result<Directory> {
            Ok(Directory { device: 7 })
        }
    }

    #[test]
    fn cloud_files_and_directories_retain_observations_without_scheduling_a_walk() {
        let temp = tempfile::tempdir().unwrap();
        let (mut context, _) = pending(temp.path());
        let context = Arc::get_mut(&mut context).unwrap();
        context.options.apparent_size = false;
        *lock(&context.root_device) = Some(7);
        for directory in [true, false] {
            let Classified::Entry(node) = context
                .classify(temp.path(), &mut Cloud { directory })
                .unwrap()
            else {
                panic!("a cloud row must not schedule a directory walk");
            };
            assert!(node.in_cloud);
            assert_eq!(node.bytes, 512);
            assert_eq!(node.identity(), Some((7, 42)));
            assert_eq!(node.is_dir(), directory);
        }
    }

    #[test]
    fn resource_exhaustion_returns_partial_coverage_with_bounded_reservations() {
        let temp = tempfile::tempdir().unwrap();
        for i in 0..100 {
            fs::write(temp.path().join(format!("f{i}")), [1; 4096]).unwrap();
        }
        let progress = Arc::new(ScanProgress::default());
        let tree = scan_with_progress(
            temp.path(),
            ScanOptions {
                max_retained_bytes: 12_000,
                ..ScanOptions::default()
            },
            Arc::clone(&progress),
        )
        .unwrap();
        let state = progress.snapshot();
        assert!(state.finished && state.limited);
        assert!(!state.cancelled);
        assert!(state.retained_bytes <= 12_000);
        assert!(!tree.complete);
        assert!(!tree.children.is_empty());
        assert!(tree.children.len() < 100);
    }

    #[test]
    fn depth_limit_retains_unknown_directory_instead_of_an_empty_success() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("a/b")).unwrap();
        fs::write(temp.path().join("a/b/data"), [1; 4096]).unwrap();
        let tree = scan(
            temp.path(),
            ScanOptions {
                max_depth: Some(0),
                ..ScanOptions::default()
            },
        )
        .unwrap();
        assert_eq!(tree.children.len(), 1);
        assert!(!tree.children[0].complete);
        assert!(!tree.complete);
    }

    #[test]
    fn directory_allocations_and_unreadable_counts_survive_reaggregation() {
        let mut root = Node::directory("root");
        root.directory_bytes = 512;
        let mut child = Node::directory("child");
        child.directory_bytes = 1024;
        child.entry_errors = 2;
        child.complete = false;
        root.children.push(child);
        for _ in 0..2 {
            aggregate(&mut root, Metric::Bytes);
            assert_eq!(root.bytes, 1536);
            assert_eq!(root.unreadable, 2);
            assert!(!root.complete);
        }
    }

    #[test]
    fn identities_are_retained_for_single_link_leaves_and_all_directories() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("d")).unwrap();
        fs::write(temp.path().join("d/file"), [1; 4096]).unwrap();
        let tree = scan(temp.path(), ScanOptions::default()).unwrap();
        for (node, path) in [
            (&tree, temp.path().to_path_buf()),
            (&tree.children[0], temp.path().join("d")),
            (&tree.children[0].children[0], temp.path().join("d/file")),
        ] {
            let meta = fs::symlink_metadata(path).unwrap();
            assert_eq!(node.identity(), Some((meta.dev(), meta.ino())));
        }
    }

    #[test]
    fn root_symlink_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("real")).unwrap();
        symlink("real", temp.path().join("link")).unwrap();
        assert_eq!(
            scan(&temp.path().join("link"), ScanOptions::default())
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn foreign_volume_directory_is_visible_with_unknown_contents_and_zero_charge() {
        let temp = tempfile::tempdir().unwrap();
        let (context, _) = pending(temp.path());
        *lock(&context.root_device) = Some(8);
        let Classified::Entry(node) = context
            .classify(temp.path(), &mut Cloud { directory: true })
            .unwrap()
        else {
            panic!("foreign volumes must not schedule a walk");
        };
        assert!(node.skipped_volume);
        assert!(!node.complete);
        assert_eq!(node.bytes, 0);
        assert_eq!(node.identity(), Some((7, 42)));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn verified_data_volume_alias_has_identical_accounting_and_identity() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("child")).unwrap();
        fs::write(temp.path().join("child/file"), [1; 4096]).unwrap();
        let ordinary = temp.path().canonicalize().unwrap();
        let alias = Path::new("/System/Volumes/Data").join(ordinary.strip_prefix("/").unwrap());
        let left = fs::symlink_metadata(&ordinary).unwrap();
        let right = fs::symlink_metadata(&alias).unwrap();
        assert_eq!((left.dev(), left.ino()), (right.dev(), right.ino()));
        let a = scan(&ordinary, ScanOptions::default()).unwrap();
        let b = scan(&alias, ScanOptions::default()).unwrap();
        assert_eq!(a.identity(), b.identity());
        assert_eq!(a.bytes, b.bytes);
        assert_eq!(a.files, b.files);
        assert_eq!(a.dirs, b.dirs);
        assert!(a.complete && b.complete);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn default_pool_workers_use_normal_qos() {
        let classes: Vec<u32> = walk_pool().broadcast(|_| {
            let mut class = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
            let mut relative = 0;
            unsafe {
                libc::pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut relative);
            }
            class as u32
        });
        assert_eq!(classes.len(), rayon::current_num_threads());
        assert!(
            classes
                .iter()
                .all(|class| *class == libc::qos_class_t::QOS_CLASS_DEFAULT as u32)
        );
    }
}

#[cfg(test)]
#[path = "scan_review_tests.rs"]
mod review_tests;
