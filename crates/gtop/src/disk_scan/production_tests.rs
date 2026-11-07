use super::*;
use std::os::unix::fs::symlink;

#[test]
fn production_scanner_matches_serial_oracle_allocation_counts_and_identity() {
    let home = tests::Scratch::new();
    fs::create_dir_all(home.0.join("project/target")).unwrap();
    fs::write(home.0.join("project/Cargo.toml"), b"manifest").unwrap();
    fs::write(home.0.join("project/target/data"), [1; 8192]).unwrap();
    fs::hard_link(home.0.join("project/target/data"), home.0.join("hardlink")).unwrap();
    symlink("project", home.0.join("link")).unwrap();
    fs::File::create(home.0.join("sparse"))
        .unwrap()
        .set_len(1 << 30)
        .unwrap();
    let cancel = AtomicBool::new(false);
    let expected = serial_oracle::scan_opened(
        &home.0,
        &home.0,
        &cancel,
        |_| {},
        &mut || {},
        &mut Instant::now(),
        || open_root(&home.0),
    );
    let actual = scan(&home.0, &home.0, &cancel, |_| {});
    assert_eq!(actual.state, ScanState::Complete);
    assert_eq!(actual.nodes[0].allocated, expected.nodes[0].allocated);
    assert_eq!(actual.nodes[0].descendants, expected.nodes[0].descendants);
    assert_eq!(actual.nodes[0].identity, expected.nodes[0].identity);
    assert_eq!(actual.nodes[0].unreadable, expected.nodes[0].unreadable);
    assert_eq!(actual.candidates.len(), 1);
    assert_eq!(actual.candidates[0].path, home.0.join("project/target"));
    assert!(actual.coverage_complete);
}

#[test]
fn production_adapter_refuses_root_replacement_after_scope_is_pinned() {
    let home = tests::Scratch::new();
    let root = home.0.join("root");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("local"), b"local").unwrap();
    let mut swapped = false;
    let result = scan_beneath(
        &home.0,
        &root,
        &home.0,
        &AtomicBool::new(false),
        |_| {},
        || {
            // The first callback services scope discovery. The second runs after pinning.
            if !swapped {
                swapped = true;
                return ScanUrgency::Background;
            }
            if !home.0.join("saved").exists() {
                fs::rename(&root, home.0.join("saved")).unwrap();
                fs::create_dir(&root).unwrap();
                fs::write(root.join("replacement"), [1; 4096]).unwrap();
            }
            ScanUrgency::Background
        },
    );
    assert!(matches!(result.state, ScanState::Failed(_)), "{result:?}");
    assert_eq!(result.nodes[0].descendants, 0);
}

#[test]
fn production_cleanup_uses_observed_parent_manifest_and_bounds_candidates() {
    let home = tests::Scratch::new();
    for index in 0..40 {
        let project = home.0.join(format!("project-{index}"));
        fs::create_dir_all(project.join("target")).unwrap();
        fs::write(project.join("Cargo.toml"), b"manifest").unwrap();
        fs::write(project.join("target/data"), [1; 4096]).unwrap();
    }
    fs::create_dir_all(home.0.join("unrelated/target")).unwrap();
    symlink(
        home.0.join("project-0/Cargo.toml"),
        home.0.join("unrelated/Cargo.toml"),
    )
    .unwrap();
    let tree = scan(&home.0, &home.0, &AtomicBool::new(false), |_| {});
    assert_eq!(tree.state, ScanState::Complete);
    assert_eq!(tree.candidates.len(), 24);
    assert!(tree
        .candidates
        .iter()
        .all(|row| row.path != home.0.join("unrelated/target")));
    assert!(tree.candidates.iter().all(|row| row.identity.inode != 0));
}

#[test]
fn production_scan_reports_progress_and_cancels_before_recursive_work() {
    let home = tests::Scratch::new();
    fs::create_dir_all(home.0.join("child/nested")).unwrap();
    fs::write(home.0.join("child/nested/data"), [1; 4096]).unwrap();
    let cancel = AtomicBool::new(false);
    let mut states = Vec::new();
    let tree = scan(&home.0, &home.0, &cancel, |tree| {
        states.push(tree.state.clone());
        if tree.state == ScanState::Scanning {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert_eq!(states.first(), Some(&ScanState::Scanning));
    assert_eq!(states.last(), Some(&ScanState::Cancelled));
    assert_eq!(tree.state, ScanState::Cancelled);
    assert!(!tree.coverage_complete);
}

#[test]
fn production_scan_refuses_symlink_root() {
    let home = tests::Scratch::new();
    fs::create_dir(home.0.join("real")).unwrap();
    symlink("real", home.0.join("link")).unwrap();
    let tree = scan(
        &home.0.join("link"),
        &home.0,
        &AtomicBool::new(false),
        |_| {},
    );
    assert!(matches!(tree.state, ScanState::Failed(_)));
    assert_eq!(tree.nodes[0].descendants, 0);
}

#[cfg(target_os = "macos")]
#[test]
fn scope_bulk_metadata_keeps_unknown_and_cloud_entries_unavailable() {
    fn record(common: u32, flags: u32) -> Vec<u8> {
        let mut record = vec![0; 80];
        for (offset, value) in [
            (0, 80u32),
            (4, common),
            (16, 4),
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
    let home = tests::Scratch::new();
    fs::write(home.0.join("x"), [1; 4096]).unwrap();
    fs::hard_link(home.0.join("x"), home.0.join("second")).unwrap();
    let fd = open_root(&home.0).unwrap();
    let (local, _) = bulk_entry(&fd, &record(0x0204000b, 0)).unwrap();
    assert!(!local.error);
    assert_eq!(local.links, 2);
    let (unknown, _) = bulk_entry(&fd, &record(0x0200000b, 0)).unwrap();
    assert!(unknown.error);
    assert!(!unknown.in_cloud);
    let (cloud, _) = bulk_entry(&fd, &record(0x0204000b, 0x40000000)).unwrap();
    assert!(cloud.in_cloud);
    assert_eq!(cloud.inode, 2);
    assert_eq!(cloud.links, 0);
}
