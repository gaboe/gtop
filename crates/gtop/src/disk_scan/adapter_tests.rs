use super::*;
use std::num::NonZeroU64;

fn directory(name: &str, inode: u64) -> DiskNode {
    let mut node = DiskNode::directory(name);
    node.inode = Some((1, NonZeroU64::new(inode).unwrap()));
    node
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
    assert!(folder.cleanable.is_empty());
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
