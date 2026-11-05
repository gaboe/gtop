//! Removal planning only. Execution stays in gtop's guarded Trash executor.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread;

/// One path the user asked to remove.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    /// Bytes measured when the path was marked; used for the projection.
    pub bytes: u64,
    pub is_dir: bool,
    pub hidden: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocked {
    pub path: PathBuf,
    pub reason: String,
}

/// Non-overlapping targets for guarded Trash preparation.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// Targets to act on, with any target contained in another removed first.
    pub targets: Vec<Target>,
    /// Marked paths covered by a target above; reported, not acted on.
    pub covered: Vec<Target>,
    /// Marked paths that must not be touched.
    pub blocked: Vec<Blocked>,
    /// The scanned root the targets were judged against.
    pub root: PathBuf,
    /// Bytes the targets free on `root`'s volume; see [`Self::reclaim`].
    reclaim: u64,
    /// Bytes the targets free on other volumes; see [`Self::foreign`].
    foreign: u64,
}

impl Plan {
    pub fn bytes(&self) -> u64 {
        self.targets.iter().map(|target| target.bytes).sum()
    }

    /// Bytes the targets free on the volume `root` is on: the only ones the
    /// free space meter may add to that volume's projection.
    ///
    /// [`Self::bytes`] counts every target, which is what a delete list
    /// wants. A mark on another volume — `-X` crosses filesystems, and a
    /// disk mounted under the root can be marked — gives its space back
    /// there, so projecting it here would lend one disk another's bytes.
    pub const fn reclaim(&self) -> u64 {
        self.reclaim
    }

    /// Bytes the targets free on other volumes, which the meter leaves out.
    pub const fn foreign(&self) -> u64 {
        self.foreign
    }

    /// Bytes whose volume could not be told: no saving is claimed for them
    /// either, since a projection the meter cannot stand behind is worse
    /// than a smaller one.
    pub fn unattributed(&self) -> u64 {
        self.bytes().saturating_sub(self.reclaim + self.foreign)
    }

    pub const fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }
}

/// Build the plan for `targets`, which must all live under `root`.
///
/// Paths outside `root` are blocked rather than removed: the tree the user was
/// looking at is the only thing they consented to act on.
pub fn plan(targets: &[Target], root: &Path) -> Plan {
    plan_against(targets, root, &mount_points())
}

/// [`plan`] against a given read of the mount table, for testing.
///
/// One read answers both what may be removed and which volume each target's
/// bytes belong to, so the projection cannot be built from a state of the
/// machine the guards never saw.
fn plan_against(targets: &[Target], root: &Path, mounts: &MountTable) -> Plan {
    let root = normalize(root);
    let mut plan = Plan {
        root: root.clone(),
        ..Plan::default()
    };
    let mut accepted: Vec<Target> = Vec::new();

    if targets.is_empty() {
        return plan;
    }
    // Once per plan, not per target: the review screen plans every frame.
    let home = std::env::home_dir().map(|home| Home::of(&home));
    let real_root = fs::canonicalize(&root).ok();

    for target in targets {
        let path = normalize(&target.path);
        let reason = refuse(&path, &root, home.as_ref(), &mounts.points, &mounts.media)
            .or_else(|| linked(&path, &root, real_root.as_deref(), home.as_ref()));
        if let Some(reason) = reason {
            plan.blocked.push(Blocked {
                path: target.path.clone(),
                reason,
            });
            continue;
        }
        accepted.push(Target {
            path,
            ..target.clone()
        });
    }

    // A path inside another target is removed with it. Keep the outer one and
    // report the inner one so the review screen can explain the nesting.
    accepted.sort_by(|left, right| left.path.cmp(&right.path));
    let mut outer: Vec<Target> = Vec::new();
    for target in accepted {
        if outer
            .iter()
            .any(|candidate| within(&target.path, &candidate.path))
        {
            plan.covered.push(target);
        } else {
            outer.push(target);
        }
    }
    plan.targets = outer;

    // Which volume each target's bytes come back to. Only what comes back to
    // the scanned volume may be shown against its free space.
    for target in &plan.targets {
        let bytes = target.bytes;
        match crate::space::attribution(mounts.mounts.as_deref(), &plan.root, &target.path) {
            crate::space::Attribution::Scanned => plan.reclaim += bytes,
            crate::space::Attribution::Other => plan.foreign += bytes,
            crate::space::Attribution::Unknown => {}
        }
    }
    plan
}

/// Trees the operating system owns. A whole-disk scan shows them, because
/// they are part of what fills the disk, but files there belong to packages
/// and removing them by hand breaks the system; pacman, paccache,
/// `journalctl --vacuum` or `brew cleanup` are the right tools. Refused even
/// where permissions would allow it, and even inside them.
///
/// One list for every platform: a macOS tree never exists on Linux, and the
/// other way round, so the extra entries cost nothing. On macOS `/etc`,
/// `/var` and `/tmp` are links into `/private`, which a whole-disk scan
/// reaches by its real name. `/Applications` is left out on purpose: moving
/// an app to the Trash is how macOS uninstalls it, and the apps macOS ships
/// live on the read-only system volume anyway.
#[cfg(not(windows))]
const SYSTEM_TREES: [&str; 24] = [
    "/bin",
    "/boot",
    "/dev",
    "/etc",
    "/lib",
    "/lib64",
    "/lib32",
    "/libx32",
    "/nix/store",
    "/gnu/store",
    "/proc",
    "/run",
    "/sbin",
    "/sys",
    "/usr",
    "/var/lib",
    "/efi",
    // Homebrew on Linux, where it lives outside every user's home.
    "/home/linuxbrew/.linuxbrew",
    // macOS.
    "/System",
    "/Library",
    "/private",
    "/cores",
    "/opt/homebrew",
    "/Network",
];

/// How the guards compare paths: lexically normalized, and where the
/// platform has more than one spelling for a place, in one of them.
///
/// macOS reaches the same directory under two names — `/Users/x` and
/// `/System/Volumes/Data/Users/x` — and its disks ignore case by default, so
/// `/library/caches` is `/Library/Caches`. Windows ignores case too, and
/// `\\?\C:\` is `C:\`. A guard that compared the spelling would be passed by
/// the other one. Only for judging: what is removed is always the path as
/// marked.
fn guard_key(path: &Path) -> PathBuf {
    if cfg!(windows) {
        return windows_key(&normalize(path));
    }
    guard_key_for(path, cfg!(target_os = "macos"))
}

fn guard_key_for(path: &Path, macos: bool) -> PathBuf {
    let path = normalize(path);
    if !macos {
        return path;
    }
    // The Data volume's own root stays as it is: it is under /System.
    let path = match path.strip_prefix(crate::space::MACOS_DATA_VOLUME) {
        Ok(rest) if !rest.as_os_str().is_empty() => Path::new("/").join(rest),
        _ => path,
    };
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

#[cfg(not(windows))]
fn windows_key(path: &Path) -> PathBuf {
    path.to_path_buf()
}

/// `path` is `base` or inside it, by whole components, as the guards
/// compare them.
fn within(path: &Path, base: &Path) -> bool {
    guard_key(path).starts_with(guard_key(base))
}

/// The system trees with their guard keys, computed once: the review screen
/// plans on every frame, for every mark.
fn system_tree_keys() -> &'static [(String, PathBuf)] {
    static KEYS: std::sync::LazyLock<Vec<(String, PathBuf)>> = std::sync::LazyLock::new(|| {
        system_trees()
            .into_iter()
            .map(|tree| {
                let key = guard_key(&tree);
                (tree.display().to_string(), key)
            })
            .collect()
    });
    &KEYS
}

#[cfg(not(windows))]
fn system_trees() -> Vec<PathBuf> {
    SYSTEM_TREES.iter().map(PathBuf::from).collect()
}

/// [`SYSTEM_TREES`] on Windows: Windows itself, installed programs and
/// their machine-wide data, and what Windows keeps at the top of its drive
/// for restore points, recovery and booting. Taken from the environment, so
/// a system elsewhere is still recognized, and the usual places on the
/// system drive besides.

/// The root of the drive Windows runs from, such as `C:\`.

/// What to use instead, said when a system tree is refused.
#[cfg(not(windows))]
const SYSTEM_TOOLS: &str = "remove it with the tool that installed it";

/// The system tree the path keyed `key` is in, if any. The home directory is
/// never system, wherever it lives.
fn system_tree(key: &Path, home: Option<&Path>) -> Option<&'static str> {
    if home.is_some_and(|home| key.starts_with(home)) {
        return None;
    }
    system_tree_keys()
        .iter()
        .find(|(_, tree)| key.starts_with(tree))
        .map(|(name, _)| name.as_str())
}

/// A system tree strictly inside the path keyed `key`: removing `/opt` takes
/// `/opt/homebrew` with it, and `/var` takes `/var/lib`.
fn system_tree_below(key: &Path) -> Option<&'static str> {
    system_tree_keys()
        .iter()
        .find(|(_, tree)| tree != key && tree.starts_with(key))
        .map(|(name, _)| name.as_str())
}

/// The home directory, as the guards compare it: by key, and by identity.
#[derive(Debug)]
struct Home {
    key: PathBuf,
    /// The key of the path with every link resolved: `/home` may be a link
    /// to `/data/home`, and a scan of `/data` then shows home under a
    /// spelling [`Self::key`] does not match.
    real: Option<PathBuf>,
    /// Device and inode (volume and file id on Windows), so a spelling
    /// [`guard_key`] cannot fold still matches.
    id: Option<(u64, u64)>,
}

impl Home {
    fn of(path: &Path) -> Self {
        Self {
            key: guard_key(path),
            real: fs::canonicalize(path).ok().map(|real| guard_key(&real)),
            id: identity(path, true),
        }
    }
}

fn refuse(
    path: &Path,
    root: &Path,
    home: Option<&Home>,
    mounts: &[PathBuf],
    media: &[PathBuf],
) -> Option<String> {
    let key = guard_key(path);
    let root_key = guard_key(root);
    let home_key = home.map(|home| home.key.as_path());
    if path.parent().is_none() {
        return Some("the filesystem root cannot be removed".into());
    }
    if key == root_key {
        return Some("the scanned root cannot be removed".into());
    }
    if cfg!(windows)
        && let Some(name) = path.components().find_map(|part| match part {
            Component::Normal(name) => misread_by_win32(&name.to_string_lossy()).then_some(name),
            _ => None,
        })
    {
        return Some(format!(
            "Windows would read {} as a different name, and remove that",
            name.display()
        ));
    }
    if home_key.is_some_and(|home| key == home)
        || home
            .and_then(|home| home.id)
            .is_some_and(|id| identity(path, false) == Some(id))
    {
        return Some("the home directory cannot be removed".into());
    }
    // On Linux `/home` is usually a mount point of its own and refused as
    // one; on macOS `/Users` is on the same volume as `/`, and removing it
    // would empty the home directory before failing on `/Users` itself.
    if home_key.is_some_and(|home| home.starts_with(&key)) {
        return Some("it contains the home directory".into());
    }
    // Elevated through another account's credentials, home is that admin's,
    // and the user's own profile is just another folder; elevation opens
    // every other profile too. So on Windows each directory in the profiles
    // folder (`C:\Users\tobi`, `Public`, `Default`) is refused, taken from
    // Windows and not from home, whose parent could be anything. What is
    // inside a profile stays removable, as under one's own home.
    if profiles_key().is_some_and(|profiles| is_profile(path, &key, profiles)) {
        return Some("a user profile cannot be removed".into());
    }
    // By spelling too: the Data volume's root keeps its name as a key, while
    // everything under it is keyed as under `/`.
    if !key.starts_with(&root_key) && !path.starts_with(root) {
        return Some("outside the scanned root".into());
    }
    if let Some(system) = system_tree(&key, home_key)
        && !(system == "/run"
            && on_media(&key, mounts, media)
            && fs::canonicalize(path).is_ok_and(|real| real == path))
    {
        return Some(format!("part of the system under {system}: {SYSTEM_TOOLS}"));
    }
    if let Some(system) = system_tree_below(&key) {
        return Some(format!("it contains {system}, which is part of the system"));
    }
    if mounts.contains(&key) || is_mount_point(path) {
        return Some("a mount point: removing it would cross onto another filesystem".into());
    }
    if let Some(mount) = mount_below(&key, mounts) {
        return Some(format!(
            "{} is mounted inside it: removing it would reach into another \
             filesystem",
            mount.display()
        ));
    }
    None
}

/// A name Win32 reads as some other one. It strips trailing dots and
/// spaces, so `C:\Users.` opens `C:\Users`; it reads `NUL` or `COM1` in
/// any directory as a device; and after a colon comes a stream of another
/// file. NTFS keeps such names when they arrive through `\\?\` or WSL,
/// and the scan lists them as stored, but the trash and the removal open
/// them through Win32.
fn misread_by_win32(name: &str) -> bool {
    const DEVICES: [&str; 4] = ["con", "prn", "aux", "nul"];
    if name.ends_with(['.', ' ']) || name.contains(':') {
        return true;
    }
    // `nul.txt` was a device too, before Windows 11: refused either way.
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_lowercase();
    let numbered = ["com", "lpt"].iter().any(|device| {
        stem.strip_prefix(device).is_some_and(|number| {
            matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8")
                || matches!(number, "9" | "\u{b9}" | "\u{b2}" | "\u{b3}")
        })
    });
    numbered || DEVICES.contains(&stem.as_str())
}

/// Why `path`, inside `root` as spelled, is somewhere else in fact, if it
/// is. The guards compare spellings, and a link or junction on the way
/// down makes a spelling inside the root name something outside it: the
/// trash, and removal on Windows, follow it. Links above the root are the
/// user's own choice of root, so the real path is compared with the real
/// root. The home directory is looked for again by real path, too.
///
/// Anything that cannot be resolved passes: the removal of a path that is
/// gone fails on its own.
fn linked(
    path: &Path,
    root: &Path,
    real_root: Option<&Path>,
    home: Option<&Home>,
) -> Option<String> {
    let (parent, name) = (path.parent()?, path.file_name()?);
    let real = guard_key(&fs::canonicalize(parent).ok()?.join(name));
    if let (Some(real_root), Ok(rest)) = (real_root, path.strip_prefix(root))
        && real != guard_key(&real_root.join(rest))
    {
        return Some(format!(
            "reached through a link: it is really {}",
            real.display()
        ));
    }
    if home
        .and_then(|home| home.real.as_ref())
        .is_some_and(|home| home.starts_with(&real))
    {
        return Some("it contains the home directory".into());
    }
    None
}

/// `(device, inode)`: what makes two spellings one directory entry.
/// `follow` decides whether a symlink at `path` answers for itself or for
/// what it points to.
#[cfg(unix)]
fn identity(path: &Path, follow: bool) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = if follow {
        fs::metadata(path)
    } else {
        fs::symlink_metadata(path)
    };
    meta.ok().map(|meta| (meta.dev(), meta.ino()))
}

/// `(volume serial, file id)`. Always what a link leads to: a junction to
/// the home directory is refused as the home directory, which is only ever
/// too careful.

#[cfg(not(any(unix, windows)))]
const fn identity(_path: &Path, _follow: bool) -> Option<(u64, u64)> {
    None
}

/// How long a read of the mount table is used before it is read again. A
/// disk plugged in is noticed within this, and the removal itself stops at a
/// mount boundary whatever the table said.
const MOUNTS_FRESH: std::time::Duration = std::time::Duration::from_secs(2);

/// Paths and safe media exceptions must come from the same snapshot.
#[derive(Default)]
struct MountTable {
    points: Vec<PathBuf>,
    media: Vec<PathBuf>,
    /// The table itself where the platform keeps one, which tells the
    /// scanned volume from another for the meter's projection. `None` where
    /// there is none to read, and the platform's own device test answers.
    mounts: Option<Vec<crate::space::Mount>>,
}

fn read_mount_table() -> MountTable {
    // The same table the scan reads for its volume, so a projection is
    // judged by the rule the tree itself was built under.
    let mounts = crate::space::mount_table();
    if let Ok(table) = fs::read_to_string("/proc/self/mountinfo") {
        let views = crate::space::parse_mountinfo(&table);
        return MountTable {
            points: views.iter().map(|mount| guard_key(&mount.point)).collect(),
            media: media_roots(&views),
            mounts,
        };
    }
    MountTable {
        points: read_mount_points(),
        media: Vec::new(),
        mounts,
    }
}

/// Only real disks mounted by the desktop under /run/media qualify. A
/// directory with that spelling, a bind of a system tree, and virtual
/// filesystems retain the /run guard. Missing information fails closed.
fn media_roots(mounts: &[crate::space::MountView]) -> Vec<PathBuf> {
    mounts
        .iter()
        .filter(|mount| {
            mount.point.starts_with("/run/media")
                && mount.point != Path::new("/run/media")
                // An overmounted disk can remain in mountinfo. Ambiguous
                // stacked views must never grant a deletion exception.
                && mounts.iter().filter(|other| other.point == mount.point).count() == 1
                && mount.fs_root == Path::new("/")
                && mount.source.starts_with("/dev/")
                && matches!(
                    mount.fstype.as_str(),
                    "ext2"
                        | "ext3"
                        | "ext4"
                        | "btrfs"
                        | "xfs"
                        | "f2fs"
                        | "vfat"
                        | "exfat"
                        | "ntfs"
                        | "ntfs3"
                        | "fuseblk"
                        | "fuse.ntfs-3g"
                )
                && !mounts.iter().any(|other| {
                    !other.point.starts_with("/run/media")
                        && (other.point == Path::new("/")
                            || system_tree(&other.point, None).is_some())
                        && (other.device == mount.device
                            || other.source == mount.source)
                })
        })
        .map(|mount| mount.point.clone())
        .collect()
}

fn on_media(key: &Path, mounts: &[PathBuf], media: &[PathBuf]) -> bool {
    // Nested mounts do not inherit their parent's exception.
    mounts
        .iter()
        .filter(|point| key.starts_with(point))
        .max_by_key(|point| point.components().count())
        .is_some_and(|point| key != point && media.contains(point))
}

/// The last read of the mount table, and whether a read is under way.
struct MountCache {
    read: Option<std::time::Instant>,
    mounts: Option<Arc<MountTable>>,
    refreshing: bool,
}

static MOUNTS: std::sync::Mutex<MountCache> = std::sync::Mutex::new(MountCache {
    read: None,
    mounts: None,
    refreshing: false,
});

/// Every mount point on this machine, as guard keys, as last read.
///
/// Never blocks on the read: the review screen plans on every frame, and on
/// macOS the table comes from running `mount`. A stale table starts a read on
/// its own thread and is used meanwhile; before the first read lands it is
/// empty, which is why the app primes it at start with
/// [`prime_mount_points`].
fn mount_points() -> Arc<MountTable> {
    let mut cache = MOUNTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fresh = cache.read.is_some_and(|read| read.elapsed() < MOUNTS_FRESH);
    if !fresh && !cache.refreshing {
        cache.refreshing = true;
        let spawned = thread::Builder::new()
            .name("disktree-mounts".into())
            .spawn(|| {
                let mounts = Arc::new(read_mount_table());
                let mut cache = MOUNTS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                cache.mounts = Some(mounts);
                cache.read = Some(std::time::Instant::now());
                cache.refreshing = false;
            });
        if spawned.is_err() {
            cache.refreshing = false;
        }
    }
    cache.mounts.clone().unwrap_or_default()
}

/// Start reading the mount table, so it is there by the time anything is
/// marked.
pub fn prime_mount_points() {
    let _ = mount_points();
}

/// Read the mount points now: from `/proc/self/mounts`, on macOS from
/// `mount`, which has no `/proc`, and on Windows from the volume list.
/// Empty where none can be read.
fn read_mount_points() -> Vec<PathBuf> {
    let points: Vec<PathBuf> = if cfg!(windows) {
        windows_mount_points()
    } else if let Ok(table) = fs::read_to_string("/proc/self/mounts") {
        crate::space::parse_mounts(&table)
            .into_iter()
            .map(|mount| mount.point)
            .collect()
    } else if cfg!(target_os = "macos") {
        Command::new("/sbin/mount")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| {
                crate::space::parse_macos_mounts(&String::from_utf8_lossy(&output.stdout))
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    points.iter().map(|point| guard_key(point)).collect()
}

#[cfg(not(windows))]
const fn windows_mount_points() -> Vec<PathBuf> {
    Vec::new()
}

/// The key of the folder every user profile is in, `C:\Users` as
/// installed; asked of Windows once, since it does not move while running.

#[cfg(not(windows))]
const fn profiles_key() -> Option<&'static Path> {
    None
}

/// Whether `path`, keyed `key`, is a profile: a directory directly in
/// `profiles`. A file or a link there is no profile and stays removable.
fn is_profile(path: &Path, key: &Path, profiles: &Path) -> bool {
    key.parent() == Some(profiles) && fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// A mount point strictly inside the path keyed `key`, if there is one. A
/// scan that stays on one filesystem never shows what is mounted there, so
/// the user cannot have meant to remove it.
fn mount_below<'a>(key: &Path, mounts: &'a [PathBuf]) -> Option<&'a Path> {
    mounts
        .iter()
        .find(|mount| mount.as_path() != key && mount.starts_with(key))
        .map(PathBuf::as_path)
}

/// Whether `path` sits on a different device than its parent, i.e. is a
/// mount point. Descending into one would delete data the user never marked.
#[cfg(unix)]
pub fn is_mount_point(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_dir() {
        return false;
    }
    let Some(parent) = path.parent() else {
        return true;
    };
    match fs::symlink_metadata(parent) {
        Ok(parent_meta) => parent_meta.dev() != meta.dev(),
        // A directory whose parent cannot be stat'ed is not worth the risk.
        Err(_) => true,
    }
}

/// Whether `path` is a folder another volume is mounted on.

#[cfg(not(any(unix, windows)))]
pub fn is_mount_point(_path: &Path) -> bool {
    false
}

/// Lexically normalize a path: resolve `.` and `..` without touching the
/// filesystem, so a symlink can never redirect a guard.
///
/// A `..` at the root has nowhere to go, so `/..` is `/` rather than the empty
/// path. A leading `..` on a relative path is preserved.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() && out.as_os_str().is_empty() {
                    out.push("..");
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                out.push(component.as_os_str());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    fn target(path: &Path, bytes: u64) -> Target {
        Target {
            path: path.to_path_buf(),
            bytes,
            is_dir: path.is_dir(),
            hidden: false,
        }
    }

    /// [`system_tree`] for plain paths, keyed the way [`refuse`] keys them.
    #[cfg(not(windows))]
    fn tree_of(path: &Path, home: &Path) -> Option<&'static str> {
        system_tree(&guard_key(path), Some(&guard_key(home)))
    }

    fn tree() -> TempDir {
        let temp = TempDir::new().expect("tempdir");
        fs::create_dir_all(temp.path().join("a/b")).expect("mkdir");
        fs::write(temp.path().join("a/one.bin"), vec![b'x'; 10]).expect("write");
        fs::write(temp.path().join("a/c.bin"), vec![b'x'; 20]).expect("write");
        fs::create_dir_all(temp.path().join("other")).expect("mkdir");
        temp
    }

    #[test]
    fn a_plan_keeps_only_the_outermost_targets() {
        let temp = tree();
        let root = temp.path();
        let inner = target(&root.join("a/b"), 5);
        let outer = target(&root.join("a"), 30);
        let separate = target(&root.join("other"), 0);

        let plan = plan(&[inner.clone(), outer.clone(), separate.clone()], root);
        assert_eq!(plan.targets.len(), 2);
        assert!(plan.targets.contains(&outer));
        assert!(plan.targets.contains(&separate));
        assert_eq!(plan.covered.len(), 1);
        assert_eq!(plan.covered[0].path, inner.path);
        assert_eq!(plan.bytes(), 30);
    }

    /// Marks may sit on another volume — `-X` crosses filesystems, and a
    /// disk mounted under the root can be marked — but the meter measures
    /// the scanned volume, so only the marks whose bytes come back to it may
    /// be projected onto its free space.
    #[test]
    fn only_the_scanned_volumes_marks_are_projected() {
        let table = "\
/dev/mapper/root / btrfs rw,subvol=/@ 0 0
/dev/mapper/root /home btrfs rw,subvol=/@home 0 0
/dev/mapper/root /.snapshots btrfs rw,subvol=/@snapshots 0 0
/dev/mapper/data /data ext4 rw 0 0
";
        let mounts = MountTable {
            points: crate::space::parse_mounts(table)
                .iter()
                .map(|mount| guard_key(&mount.point))
                .collect(),
            media: Vec::new(),
            mounts: Some(crate::space::parse_mounts(table)),
        };
        let marked = [
            target(Path::new("/home/tobi/junk"), 100),
            target(Path::new("/data/junk"), 200),
            target(Path::new("/.snapshots/7/junk"), 400),
        ];
        let plan = plan_against(&marked, Path::new("/"), &mounts);
        assert!(
            plan.blocked.is_empty(),
            "a mark inside another volume is legal: {:?}",
            plan.blocked
        );
        assert_eq!(plan.bytes(), 700);
        assert_eq!(plan.reclaim(), 100, "the Btrfs subvolume is this disk");
        assert_eq!(plan.foreign(), 600, "another disk, and a snapshot");
        assert_eq!(plan.unattributed(), 0);

        // A mark inside another is removed with it, so its bytes are counted
        // once here as they are in the total.
        let nested = [
            target(Path::new("/home/tobi/junk"), 100),
            target(Path::new("/home/tobi/junk/deeper"), 60),
        ];
        let plan = plan_against(&nested, Path::new("/"), &mounts);
        assert_eq!(plan.covered.len(), 1);
        assert_eq!(plan.bytes(), 100);
        assert_eq!(plan.reclaim(), 100);

        // Where the table cannot place the paths, nothing is claimed: a
        // projection the meter cannot stand behind is worse than none.
        let partial = MountTable {
            mounts: Some(crate::space::parse_mounts(
                "/dev/mapper/data /data ext4 rw 0 0\n",
            )),
            ..MountTable::default()
        };
        let root = Path::new("/disktree-untabled");
        let plan = plan_against(&[target(&root.join("junk"), 50)], root, &partial);
        assert_eq!(plan.bytes(), 50);
        assert_eq!(plan.reclaim(), 0);
        assert_eq!(plan.foreign(), 0);
        assert_eq!(plan.unattributed(), 50, "no saving may be claimed");
    }

    #[test]
    fn the_root_and_home_are_refused() {
        let temp = tree();
        let root = temp.path();
        let home = std::env::home_dir();
        let mut targets = vec![target(Path::new("/"), 0), target(root, 0)];
        if let Some(home) = &home {
            targets.push(target(home, 0));
        }

        let plan = plan(&targets, root);
        assert!(plan.is_empty());
        assert_eq!(plan.blocked.len(), targets.len());
        assert!(
            plan.blocked
                .iter()
                .any(|blocked| blocked.reason.contains("home directory"))
                || home.is_none()
        );
    }

    /// Removing a directory takes what is inside it, so the directories
    /// above home are refused too: in a whole-disk scan, `C:\Users` is an
    /// ordinary directory, and so is `/home` without a separate volume.
    #[test]
    fn the_directories_above_home_are_refused() {
        let home = Home::of(Path::new("/home/disktree-test-user"));
        let reason = refuse(Path::new("/home"), Path::new("/"), Some(&home), &[], &[]);
        assert!(
            reason.is_some_and(|reason| reason.contains("home directory")),
            "the home directory is inside it"
        );
        assert_eq!(
            refuse(
                Path::new("/home/other"),
                Path::new("/"),
                Some(&home),
                &[],
                &[]
            ),
            None,
            "a sibling of home is not above it"
        );
    }

    #[test]
    fn paths_outside_the_root_are_refused() {
        let temp = tree();
        let plan = plan(&[target(Path::new("/etc/passwd"), 1)], temp.path());
        assert!(plan.is_empty());
        assert_eq!(plan.blocked[0].reason, "outside the scanned root");
    }

    #[test]
    fn sibling_prefixes_are_not_treated_as_containment() {
        let temp = tree();
        let root = temp.path();
        fs::create_dir_all(root.join("a-real")).expect("mkdir");
        let plan = plan(
            &[target(&root.join("a"), 1), target(&root.join("a-real"), 1)],
            root,
        );
        assert_eq!(plan.targets.len(), 2, "a-real is not inside a");
        assert!(plan.covered.is_empty());
    }

    #[test]
    fn normalize_resolves_dots_without_the_filesystem() {
        assert_eq!(
            normalize(Path::new("/home/tobi/./src/../src/")),
            PathBuf::from("/home/tobi/src")
        );
        assert_eq!(normalize(Path::new("a/../b")), PathBuf::from("b"));
        assert_eq!(normalize(Path::new("/..")), PathBuf::from("/"));
        assert_eq!(normalize(Path::new("../x")), PathBuf::from("../x"));
    }

    #[test]
    fn mount_points_are_recognized() {
        let temp = tree();
        assert!(!is_mount_point(temp.path()));
        // /proc is a mount point everywhere Linux runs this test.
        if Path::new("/proc/self").exists() {
            assert!(is_mount_point(Path::new("/proc")));
        }
    }

    #[test]
    fn a_directory_holding_the_home_directory_is_refused() {
        // /Users in a whole-disk scan on macOS: the same volume as /, so no
        // mount point, and removing it would empty home first.
        let home = Path::new("/Users/tobi");
        let reason = refuse(
            Path::new("/Users"),
            Path::new("/"),
            Some(&Home::of(home)),
            &[],
            &[],
        )
        .expect("refused");
        assert!(reason.contains("home directory"), "{reason}");
        assert!(
            refuse(
                Path::new("/Users/tobi/src"),
                Path::new("/"),
                Some(&Home::of(home)),
                &[],
                &[]
            )
            .is_none(),
            "inside home is the user's"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn a_directory_holding_a_system_tree_is_refused() {
        let home = Path::new("/Users/tobi");
        for (path, inside) in [("/opt", "/opt/homebrew"), ("/var", "/var/lib")] {
            let reason = refuse(
                Path::new(path),
                Path::new("/"),
                Some(&Home::of(home)),
                &[],
                &[],
            )
            .expect("refused");
            assert!(reason.contains(inside), "{path}: {reason}");
        }
        assert!(
            refuse(
                Path::new("/opt/local"),
                Path::new("/"),
                Some(&Home::of(home)),
                &[],
                &[]
            )
            .is_none(),
            "a sibling of a system tree is not its parent"
        );
    }

    #[test]
    fn a_target_with_a_mount_inside_is_refused() {
        let temp = tree();
        let root = temp.path();
        let mounts = vec![guard_key(&root.join("a/b")), guard_key(&root.join("other"))];
        assert_eq!(
            mount_below(&guard_key(&root.join("a")), &mounts),
            Some(guard_key(&root.join("a/b")).as_path())
        );
        assert_eq!(
            mount_below(&guard_key(&root.join("a/b")), &mounts),
            None,
            "itself"
        );
        let reason = refuse(&root.join("a"), root, None, &mounts, &[]).expect("refused");
        assert!(reason.contains("mounted inside"), "{reason}");
        assert_eq!(
            refuse(&root.join("a/one.bin"), root, None, &mounts, &[]),
            None
        );
    }

    #[test]
    fn nothing_is_mounted_below_a_temp_dir() {
        let temp = tree();
        assert_eq!(
            mount_below(&guard_key(temp.path()), &read_mount_points()),
            None
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn this_macs_mount_table_is_read_without_proc() {
        let mounts = read_mount_points();
        assert!(mounts.contains(&guard_key(Path::new("/"))), "{mounts:?}");
        // Asked of `mount_below` itself: `refuse` would turn /System/Volumes
        // away as a system tree before it looked at the mounts.
        assert!(
            mount_below(&guard_key(Path::new("/System/Volumes")), &mounts).is_some(),
            "the Data volume is mounted there"
        );
    }

    #[test]
    fn the_mount_table_is_read_off_the_calling_thread_and_then_kept() {
        prime_mount_points();
        // The first read lands on its own thread; wait for it, briefly.
        let mut mounts = mount_points();
        for _ in 0..500 {
            if !mounts.points.is_empty() {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(10));
            mounts = mount_points();
        }
        assert_eq!(
            mounts.points,
            read_mount_table().points,
            "the same table, cached"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_volume_is_judged_by_its_mount_where_the_kernel_names_one() {
        let temp = tree();
        let dir = open_dir(rustix::fs::CWD, temp.path()).expect("open");
        let here = Volume::of(&dir).expect("volume");
        let mut other = Volume::of(&dir).expect("volume");
        assert!(here.contains(&other));
        // A btrfs subvolume: another device, the same mount.
        other.device.st_dev = other.device.st_dev.wrapping_add(1);
        other.mount = Some(7);
        let mount = Volume {
            device: here.device,
            mount: Some(7),
        };
        assert!(mount.contains(&other), "same mount, other device");
        // A bind mount: the same device, another mount.
        let bind = Volume {
            device: here.device,
            mount: Some(8),
        };
        assert!(!mount.contains(&bind));
        // No mount IDs, as on macOS: the device decides.
        let (mut plain, mut moved) = (
            Volume::of(&dir).expect("volume"),
            Volume::of(&dir).expect("volume"),
        );
        plain.mount = None;
        moved.mount = None;
        moved.device.st_dev = moved.device.st_dev.wrapping_add(1);
        assert!(!plain.contains(&moved));
    }

    #[test]
    fn macos_guard_keys_fold_the_data_volume_and_case() {
        let key = |path: &str| guard_key_for(Path::new(path), true);
        assert_eq!(
            key("/System/Volumes/Data/Users/Tobi/src"),
            PathBuf::from("/users/tobi/src")
        );
        assert_eq!(key("/Library/./Caches"), PathBuf::from("/library/caches"));
        assert_eq!(
            key("/System/Volumes/Data"),
            PathBuf::from("/system/volumes/data"),
            "the Data volume itself stays under /System"
        );
        assert_eq!(
            guard_key_for(Path::new("/Library/X"), false),
            PathBuf::from("/Library/X"),
            "Linux paths are compared as spelled"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_guards_see_through_other_spellings() {
        let home = Path::new("/Users/tobi");
        let data = Path::new(crate::space::MACOS_DATA_VOLUME);
        assert!(
            refuse(
                &data.join("Users/tobi/src"),
                data,
                Some(&Home::of(home)),
                &[],
                &[]
            )
            .is_none(),
            "home under the Data volume's name is still home"
        );
        let reason = refuse(
            Path::new("/library/caches"),
            Path::new("/"),
            Some(&Home::of(home)),
            &[],
            &[],
        )
        .expect("refused");
        assert!(reason.contains("/Library"), "{reason}");
    }
}
