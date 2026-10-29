use super::*;
use std::os::unix::{ffi::OsStringExt, fs::symlink};

pub(super) fn replay(
    path: &Path,
    tape: &[(usize, Entry)],
    at_capacity: bool,
    cut: usize,
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
        device: 1,
        cancel: &cancel,
        progress: &mut progress,
        last: Instant::now(),
        on_tick: &mut on_tick,
        last_tick: &mut last_tick,
        links: if at_capacity {
            (0..MAX_LINKS as u64).map(|inode| (1, inode)).collect()
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

pub(super) fn assert_same(mut actual: Tree, mut expected: Tree) {
    actual.observed = SystemTime::UNIX_EPOCH;
    expected.observed = SystemTime::UNIX_EPOCH;
    assert_eq!(actual.state, expected.state);
    let actual_debug = format!("{actual:?}");
    let expected_debug = format!("{expected:?}");
    assert!(actual_debug == expected_debug, "tree fields differ");
    for page in [0, 1, 7] {
        for cached in [false, true] {
            assert_eq!(
                format!("{:?}", actual.folder(&actual.root, page, cached)),
                format!("{:?}", expected.folder(&expected.root, page, cached)),
            );
        }
    }
    for id in 0..actual.nodes.len() {
        assert_eq!(actual.path(id), expected.path(id));
        assert_eq!(
            format!("{:?}", actual.folder(&actual.path(id), 0, false)),
            format!("{:?}", expected.folder(&expected.path(id), 0, false)),
        );
    }
}

pub(super) fn oracle_scan(path: &Path, cancel: &AtomicBool) -> Tree {
    serial_oracle::scan_opened(
        path,
        path,
        cancel,
        |_| {},
        &mut || {},
        &mut Instant::now(),
        || open_root(path),
    )
}

#[test]
fn reducer_matches_serial_oracle_for_stable_filesystem() {
    let home = tests::Scratch::new();
    for index in 0..180 {
        let branch = home.0.join(format!("project-{index:03}"));
        fs::create_dir_all(branch.join("node_modules/pkg")).unwrap();
        fs::create_dir(branch.join("target")).unwrap();
        fs::write(branch.join("Cargo.toml"), b"manifest").unwrap();
        fs::write(branch.join("node_modules/pkg/data"), vec![1; 4096]).unwrap();
        if index == 0 {
            fs::write(branch.join("shared"), vec![2; 8192]).unwrap();
        } else {
            fs::hard_link(home.0.join("project-000/shared"), branch.join("shared")).unwrap();
        }
    }
    fs::write(home.0.join("unicode-é-界"), b"raw name").unwrap();
    symlink("project-000", home.0.join("directory-link")).unwrap();
    let sparse = fs::File::create(home.0.join("sparse")).unwrap();
    sparse.set_len(1 << 30).unwrap();
    let mut path = home.0.join("deep");
    fs::create_dir(&path).unwrap();
    for _ in 0..258 {
        path.push("d");
        fs::create_dir(&path).unwrap();
    }
    fs::write(path.join("unvisited"), b"depth limit").unwrap();
    let cancel = AtomicBool::new(false);
    let actual = scan(&home.0, &home.0, &cancel, |_| {});
    let expected = oracle_scan(&home.0, &cancel);
    assert_eq!(actual.state, ScanState::Complete);
    assert_eq!(actual.nodes.len(), PAGE_SIZE + 1);
    assert_eq!(actual.candidates.len(), 24);
    assert!(actual.nodes[0].unreadable > 0);
    assert_same(actual, expected);
}

#[test]
fn reducer_matches_serial_oracle_for_observation_tapes_and_cancellation_cuts() {
    let home = tests::Scratch::new();
    let tape: Vec<_> = (0..280)
        .map(|index| {
            let directory = index % 5 == 0;
            (
                if index % 11 == 0 { 256 } else { 0 },
                Entry {
                    name: if index == 1 {
                        std::ffi::OsString::from_vec(vec![b'n', 0xff])
                    } else if directory {
                        format!("absent-{index}").into()
                    } else {
                        format!("entry-{index:03}").into()
                    },
                    device: if index % 13 == 0 { 2 } else { 1 },
                    inode: (index % 31) as u64,
                    directory,
                    allocated: if index == 1 { u64::MAX } else { 0 },
                    error: index % 17 == 0,
                    in_cloud: index % 19 == 0,
                    links: if index % 2 == 0 { 2 } else { 1 },
                },
            )
        })
        .collect();
    for cut in [0, 1, 2, 20, 129, 260, usize::MAX] {
        let (actual, actual_total) = replay(&home.0, &tape, false, cut);
        let (expected, expected_total) = serial_oracle::replay(&home.0, &tape, false, cut);
        assert_eq!(actual_total, expected_total);
        assert_same(actual, expected);
    }
}

#[test]
fn reducer_matches_serial_oracle_at_link_capacity_including_rejected_cloud_entry() {
    let home = tests::Scratch::new();
    let tape: Vec<_> = [2, 2, MAX_LINKS as u64 + 1]
        .into_iter()
        .map(|inode| {
            (
                0,
                Entry {
                    name: format!("link-{inode}").into(),
                    device: 1,
                    inode,
                    directory: false,
                    allocated: 4096,
                    error: false,
                    in_cloud: true,
                    links: 2,
                },
            )
        })
        .collect();
    let (actual, actual_total) = replay(&home.0, &tape, true, usize::MAX);
    let (expected, expected_total) = serial_oracle::replay(&home.0, &tape, true, usize::MAX);
    assert_eq!(actual.cloud_items, 3);
    assert_eq!(actual.nodes[0].descendants, 2);
    assert!(matches!(actual.state, ScanState::Failed(_)));
    assert_eq!(actual_total, expected_total);
    assert_same(actual, expected);
}

#[test]
fn reducer_matches_serial_oracle_when_cancelled_before_start() {
    let home = tests::Scratch::new();
    fs::create_dir(home.0.join("child")).unwrap();
    let cancel = AtomicBool::new(true);
    assert_same(
        scan(&home.0, &home.0, &cancel, |_| {}),
        oracle_scan(&home.0, &cancel),
    );
}

#[test]
fn reducer_matches_serial_oracle_when_hook_cancels_an_entered_directory() {
    let home = tests::Scratch::new();
    fs::create_dir_all(home.0.join("node_modules/pkg")).unwrap();
    fs::write(home.0.join("node_modules/pkg/file"), b"data").unwrap();
    let actual_cancel = AtomicBool::new(false);
    let actual = scan_with_hook(
        &home.0,
        &home.0,
        &actual_cancel,
        |_| {},
        || {
            actual_cancel.store(true, Ordering::Relaxed);
            ScanUrgency::Background
        },
    );
    let expected_cancel = AtomicBool::new(false);
    let expected = serial_oracle::scan_opened(
        &home.0,
        &home.0,
        &expected_cancel,
        |_| {},
        &mut || expected_cancel.store(true, Ordering::Relaxed),
        &mut (Instant::now() - Duration::from_secs(1)),
        || open_root(&home.0),
    );
    assert_eq!(actual.state, ScanState::Cancelled);
    assert_eq!(actual.nodes.len(), 2);
    assert_same(actual, expected);
}
