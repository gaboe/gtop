use super::super::{
    reducer_tests::{assert_same, oracle_scan},
    serial_oracle,
    tests::Scratch,
};
use super::*;
use std::sync::Barrier;

fn scan_options(root: &Path, cancel: &AtomicBool, options: Options) -> Tree {
    foreground_priority();
    let tree = scan_opened(
        root,
        root,
        cancel,
        Callbacks {
            progress: &mut |_| {},
            on_tick: &mut || Urgency::Foreground,
            last_tick: &mut (Instant::now() - Duration::from_secs(1)),
        },
        || open_root(root),
        options,
    );
    background_priority();
    tree
}

fn serial(root: &Path, cancel: &AtomicBool) -> Tree {
    foreground_priority();
    let tree = oracle_scan(root, cancel);
    background_priority();
    tree
}

fn options(width: usize) -> Options {
    Options {
        workers: width,
        initial_urgency: Urgency::Foreground,
        ..Options::default()
    }
}

fn wait_for(signal: &std::sync::mpsc::Receiver<()>, message: &str) {
    signal
        .recv_timeout(Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("timed out waiting for {message}"));
}

fn release(gate: &Arc<(Mutex<bool>, Condvar)>) {
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
}

fn wait_for_release(gate: &Arc<(Mutex<bool>, Condvar)>, message: &str) {
    let (released, timeout) = gate
        .1
        .wait_timeout_while(
            gate.0.lock().unwrap(),
            Duration::from_secs(60),
            |released| !*released,
        )
        .unwrap();
    assert!(
        *released && !timeout.timed_out(),
        "timed out waiting for {message}"
    );
}

fn assert_released(stats: &Stats, width: usize) {
    assert_eq!(stats.live_fds.load(Ordering::Relaxed), 0);
    assert_eq!(stats.receipt_bytes.load(Ordering::Relaxed), 0);
    assert_eq!(stats.speculative_receipt_bytes.load(Ordering::Relaxed), 0);
    assert!(stats.peak_fds.load(Ordering::Relaxed) <= 1 + 2 * width + CACHE_FDS);
}

#[test]
fn queue_matches_oracle_at_all_widths_and_releases_cache_handles() {
    let home = Scratch::new();
    for index in 0..160 {
        let branch = home.0.join(format!("branch-{index:03}"));
        fs::create_dir_all(branch.join("node_modules/pkg")).unwrap();
        fs::create_dir(branch.join("target")).unwrap();
        fs::write(branch.join("Cargo.toml"), b"manifest").unwrap();
        for file in 0..12 {
            fs::write(
                branch.join(format!("node_modules/pkg/file-{file}")),
                [1; 4096],
            )
            .unwrap();
        }
        if index == 0 {
            fs::write(branch.join("link"), [2; 8192]).unwrap();
        } else {
            fs::hard_link(home.0.join("branch-000/link"), branch.join("link")).unwrap();
        }
    }
    let mut deep = home.0.join("deep");
    fs::create_dir(&deep).unwrap();
    for _ in 0..258 {
        deep.push("d");
        fs::create_dir(&deep).unwrap();
    }
    std::os::unix::fs::symlink("branch-000", home.0.join("symlink")).unwrap();
    let sparse = fs::File::create(home.0.join("sparse")).unwrap();
    sparse.set_len(1 << 30).unwrap();
    let cancel = AtomicBool::new(false);
    let expected = serial(&home.0, &cancel);
    for width in [1, 2, 4, 8, 14] {
        let config = options(width);
        let stats = config.stats.clone();
        let actual = scan_options(&home.0, &cancel, config);
        assert_same(actual, serial(&home.0, &cancel));
        assert_eq!(expected.nodes.len(), PAGE_SIZE + 1);
        assert_released(&stats, width);
        assert!(stats.peak_receipt_bytes.load(Ordering::Relaxed) <= RECEIPT_WINDOW);
        assert!(stats.cache_hits.load(Ordering::Relaxed) > 0);
    }
}

#[test]
fn queue_finishes_nested_large_directory_schedule_without_output_backpressure() {
    let home = Scratch::new();
    let mut directory = home.0.clone();
    for _ in 0..14 {
        for index in 0..800 {
            fs::create_dir(directory.join(format!("child-{index:04}"))).unwrap();
        }
        let fd = open_root(&directory).unwrap();
        let mut first = None;
        entries(&fd, |entry| {
            first = Some(entry.name);
            false
        })
        .unwrap();
        directory.push(first.unwrap());
    }
    let cancel = AtomicBool::new(false);
    for width in [1, 2, 4, 8, 14] {
        let config = options(width);
        let stats = config.stats.clone();
        let actual = scan_options(&home.0, &cancel, config);
        assert_same(actual, serial(&home.0, &cancel));
        assert_released(&stats, width);
        assert!(stats.peak_receipt_bytes.load(Ordering::Relaxed) <= RECEIPT_WINDOW);
    }
}

#[test]
fn queue_forced_later_completion_preserves_dfs_and_overlaps_real_enumeration() {
    let home = Scratch::new();
    fs::write(home.0.join("source"), [1; 4096]).unwrap();
    let scan = home.0.join("scan");
    fs::create_dir(&scan).unwrap();
    for index in 0..2 {
        let directory = scan.join(format!("child-{index}"));
        fs::create_dir_all(directory.join("node_modules/pkg")).unwrap();
        fs::write(directory.join("node_modules/pkg/payload"), b"same size").unwrap();
    }
    let (shared_name, native_order) = (0..128)
        .find_map(|index| {
            let name = format!("z-shared-{index:03}");
            let shared_path = scan.join(&name);
            fs::hard_link(home.0.join("source"), &shared_path).unwrap();
            let scan_fd = open_root(&scan).unwrap();
            let mut entries_in_order = vec![];
            entries(&scan_fd, |entry| {
                entries_in_order.push((entry.name, entry.directory));
                true
            })
            .unwrap();
            drop(scan_fd);
            let shared_index = entries_in_order
                .iter()
                .position(|(entry_name, _)| entry_name == std::ffi::OsStr::new(&name))
                .unwrap();
            let first_child = entries_in_order[..shared_index]
                .iter()
                .find(|(_, directory)| *directory)
                .map(|(entry_name, _)| scan.join(entry_name));
            if let Some(first_child) = first_child {
                let native_order = entries_in_order
                    .iter()
                    .filter(|(_, directory)| *directory)
                    .map(|(entry_name, _)| entry_name.clone())
                    .collect::<Vec<_>>();
                fs::hard_link(home.0.join("source"), first_child.join("shared")).unwrap();
                Some((name, native_order))
            } else {
                fs::remove_file(shared_path).unwrap();
                None
            }
        })
        .expect("find a root sibling after a scanned child");
    assert_eq!(native_order.len(), 2);
    for width in [2, 4, 8, 14] {
        let later_done = Arc::new((Mutex::new(false), Condvar::new()));
        let order = Arc::new(Mutex::new(vec![]));
        let mut config = options(width);
        let waiting = later_done.clone();
        config.hooks.before_open = Some(Arc::new(move |key| {
            if key == [0] {
                let (lock, wake) = &*waiting;
                let (done, timeout) = wake
                    .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(10), |done| {
                        !*done
                    })
                    .unwrap();
                assert!(
                    *done && !timeout.timed_out(),
                    "later directory must finish before the first"
                );
            }
        }));
        let done = later_done.clone();
        let completed = order.clone();
        config.hooks.completed = Some(Arc::new(move |key| {
            if key.len() == 1 {
                completed.lock().unwrap().push(key[0]);
                if key[0] != 0 {
                    *done.0.lock().unwrap() = true;
                    done.1.notify_all();
                }
            }
        }));
        let actual = scan_options(&scan, &AtomicBool::new(false), config);
        assert_ne!(order.lock().unwrap()[0], 0);
        let later_sibling = actual
            .nodes
            .iter()
            .find(|node| node.name == std::ffi::OsStr::new(&shared_name))
            .expect("later root sibling should be visible in the tree");
        assert!(later_sibling.shared);
        let candidates = actual.candidates.iter().collect::<Vec<_>>();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].allocated, candidates[1].allocated);
        assert_eq!(
            candidates
                .iter()
                .map(|row| row.path.parent().unwrap().file_name().unwrap())
                .collect::<Vec<_>>(),
            native_order
                .iter()
                .map(std::ffi::OsString::as_os_str)
                .collect::<Vec<_>>()
        );
        assert_same(actual, serial(&scan, &AtomicBool::new(false)));
    }
    let barrier = Arc::new(Barrier::new(2));
    let admitted = Arc::new(AtomicUsize::new(0));
    let mut config = options(2);
    let active = admitted.clone();
    config.hooks.after_entry = Some(Arc::new(move |key| {
        if key.len() == 1 && active.fetch_add(1, Ordering::SeqCst) < 2 {
            barrier.wait();
        }
    }));
    let tree = scan_options(&scan, &AtomicBool::new(false), config);
    assert_eq!(tree.state, ScanState::Complete);
    assert!(admitted.load(Ordering::SeqCst) >= 2);
}

fn scripted_scan(
    root: &Path,
    tape: Vec<Entry>,
    width: usize,
    cut: Option<u64>,
    capacity: bool,
    read_error: bool,
) -> (Tree, Arc<Stats>) {
    let root_id = identity(&open_root(root).unwrap()).unwrap();
    let mut tree = Tree::new(root);
    tree.nodes[0].identity = root_id;
    tree.nodes[0].allocated = u64::MAX - 8192;
    let mut config = options(width);
    config.hooks.cancel_after = cut;
    if capacity {
        config.hooks.initial_links = (0..MAX_LINKS as u64)
            .map(|inode| (root_id.device, inode))
            .collect();
    }
    config.hooks.tape = Some(Arc::new(move |key| {
        assert!(key.is_empty());
        (tape.clone(), read_error)
    }));
    let stats = config.stats.clone();
    let cancel = AtomicBool::new(false);
    foreground_priority();
    run(
        &mut tree,
        &open_root(root).unwrap(),
        root,
        &cancel,
        Callbacks {
            progress: &mut |_| {},
            on_tick: &mut || Urgency::Foreground,
            last_tick: &mut Instant::now(),
        },
        config,
    );
    background_priority();
    if tree.state == ScanState::Scanning {
        tree.state = if cancel.load(Ordering::Relaxed) {
            ScanState::Cancelled
        } else {
            ScanState::Complete
        };
    }
    (tree, stats)
}

#[test]
fn queue_scripted_link_cap_cloud_error_saturation_and_cancel_prefixes_match_oracle() {
    let home = Scratch::new();
    let root_id = identity(&open_root(&home.0).unwrap()).unwrap();
    for capacity in [false, true] {
        let tape: Vec<_> = (0..180)
            .map(|index| Entry {
                name: if index == 1 {
                    std::ffi::OsStr::from_bytes(&[b'n', 255]).to_owned()
                } else {
                    format!("file-{index:03}").into()
                },
                device: if index % 17 == 0 {
                    root_id.device + 1
                } else {
                    root_id.device
                },
                inode: if index == 179 {
                    MAX_LINKS as u64 + 1
                } else {
                    (index % 7) as u64
                },
                directory: index == 7 || index % 9 == 2 || index % 17 == 0 || index % 19 == 0,
                allocated: if index == 1 { u64::MAX } else { 0 },
                error: index % 19 == 0,
                in_cloud: index % 3 == 2,
                links: 2,
            })
            .collect();
        for width in [1, 2, 4, 8, 14] {
            for cut in [0, 1, 20, 129, 180, usize::MAX] {
                let (actual, stats) = scripted_scan(
                    &home.0,
                    tape.clone(),
                    width,
                    (cut != usize::MAX).then_some(cut as u64),
                    capacity,
                    false,
                );
                let visits: Vec<_> = tape.iter().cloned().map(|entry| (0, entry)).collect();
                let (mut expected, _) = serial_oracle::replay_with_device(
                    &home.0,
                    &visits,
                    capacity,
                    cut,
                    root_id.device,
                );
                expected.nodes[0].identity = root_id;
                assert_same(actual, expected);
                assert_released(&stats, width);
            }
        }
    }
    let entry = Entry {
        name: "file".into(),
        device: root_id.device,
        inode: 42,
        directory: false,
        allocated: 4096,
        error: false,
        in_cloud: false,
        links: 1,
    };
    for width in [1, 2, 4, 8, 14] {
        let (tree, stats) = scripted_scan(&home.0, vec![entry.clone()], width, None, false, true);
        assert_eq!(tree.state, ScanState::Complete);
        assert_eq!(tree.nodes[0].descendants, 1);
        assert_eq!(tree.nodes[0].unreadable, 1);
        assert_released(&stats, width);
    }
}

#[test]
fn queue_single_flat_directory_can_exceed_admission_window() {
    let home = Scratch::new();
    for index in 0..400 {
        fs::write(home.0.join(format!("file-{index}")), b"data").unwrap();
    }
    let mut config = options(4);
    config.window = 4096;
    let stats = config.stats.clone();
    assert_same(
        scan_options(&home.0, &AtomicBool::new(false), config),
        serial(&home.0, &AtomicBool::new(false)),
    );
    assert!(stats.peak_receipt_bytes.load(Ordering::Relaxed) > 4096);
    assert_eq!(stats.oversized.load(Ordering::Relaxed), 1);
    assert_released(&stats, 4);
}

#[test]
fn queue_defers_oversized_speculative_receipts_until_they_are_required() {
    let home = Scratch::new();
    let root = home.0.join("scan");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("first")).unwrap();
    fs::create_dir(root.join("later")).unwrap();
    let root_fd = open_root(&root).unwrap();
    let mut native_order = vec![];
    entries(&root_fd, |entry| {
        native_order.push(entry.name);
        true
    })
    .unwrap();
    drop(root_fd);
    assert_eq!(native_order.len(), 2);
    let later = root.join(&native_order[1]);
    for index in 0..1600 {
        fs::create_dir(later.join(format!("directory-{index:04}"))).unwrap();
    }

    let mut config = options(4);
    config.window = 256 * 1024;
    let window = config.window;
    let stats = config.stats.clone();
    let observed = stats.clone();
    config.hooks.before_open = Some(Arc::new(move |key| {
        if key == [0] {
            let deadline = Instant::now() + Duration::from_secs(10);
            while observed.abandons.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(observed.abandons.load(Ordering::Relaxed) > 0);
        }
    }));

    assert_same(
        scan_options(&root, &AtomicBool::new(false), config),
        serial(&root, &AtomicBool::new(false)),
    );
    assert!(stats.abandons.load(Ordering::Relaxed) > 0);
    assert_eq!(stats.jobs.load(Ordering::Relaxed), 1603);
    let workers = stats.effective_workers.load(Ordering::Relaxed);
    let reservation = window / (workers + 2);
    assert!(stats.peak_speculative_receipt_bytes.load(Ordering::Relaxed) > reservation);
    let bound = window + workers * reservation;
    assert!(stats.peak_speculative_receipt_bytes.load(Ordering::Relaxed) <= bound);
    assert_released(&stats, 4);
}

#[test]
fn queue_promotion_keeps_an_in_progress_read_and_publishes_its_children() {
    let home = Scratch::new();
    let root = home.0.join("scan");
    fs::create_dir(&root).unwrap();
    let first = root.join("first");
    fs::create_dir(&first).unwrap();
    for index in 0..2400 {
        fs::create_dir(first.join(format!("child-{index:04}"))).unwrap();
    }

    let mut config = options(4);
    config.window = 128 * 1024;
    let stats = config.stats.clone();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (needed_tx, needed_rx) = std::sync::mpsc::channel();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let waiting = gate.clone();
    let entries_seen = Arc::new(AtomicUsize::new(0));
    let count = entries_seen.clone();
    config.hooks.after_entry = Some(Arc::new(move |key| {
        if key == [0] && count.fetch_add(1, Ordering::Relaxed) == 7 {
            let _ = started_tx.send(());
            wait_for_release(&waiting, "promotion release");
        }
    }));
    config.hooks.needed = Some(Arc::new(move |key| {
        if key == [0] {
            let _ = needed_tx.send(());
        }
    }));

    let scan_root = root.clone();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        done_tx
            .send(scan_options(&scan_root, &AtomicBool::new(false), config))
            .unwrap();
    });
    wait_for(
        &started_rx,
        "the speculative read to pause below its reservation",
    );
    wait_for(&needed_rx, "the reducer to require the paused route");
    release(&gate);
    let actual = done_rx
        .recv_timeout(Duration::from_secs(60))
        .expect("promoted read must finish without abandoning or reopening");
    assert_same(actual, serial(&root, &AtomicBool::new(false)));
    assert_eq!(stats.abandons.load(Ordering::Relaxed), 0);
    assert!(stats.oversized.load(Ordering::Relaxed) > 0);
    assert_eq!(stats.jobs.load(Ordering::Relaxed), 2402);
    assert_released(&stats, 4);
}

#[test]
fn queue_demand_between_abandon_and_defer_claims_the_deferred_route() {
    let home = Scratch::new();
    let root = home.0.join("scan");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("first")).unwrap();
    fs::create_dir(root.join("second")).unwrap();
    let root_fd = open_root(&root).unwrap();
    let mut root_order = vec![];
    entries(&root_fd, |entry| {
        root_order.push(entry.name);
        true
    })
    .unwrap();
    drop(root_fd);
    let first = root.join(&root_order[0]);
    for index in 0..400 {
        fs::create_dir(first.join(format!("child-{index:04}"))).unwrap();
    }

    let mut config = options(4);
    config.window = 8 * 1024;
    let stats = config.stats.clone();
    let (abandoned_tx, abandoned_rx) = std::sync::mpsc::channel();
    let (needed_tx, needed_rx) = std::sync::mpsc::channel();
    let (root_paused_tx, root_paused_rx) = std::sync::mpsc::channel();
    let root_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let wait_for_root = root_gate.clone();
    let entries_seen = Arc::new(AtomicUsize::new(0));
    let count = entries_seen.clone();
    config.hooks.after_entry = Some(Arc::new(move |key| {
        if key.is_empty() && count.fetch_add(1, Ordering::Relaxed) == 1 {
            let _ = root_paused_tx.send(());
            wait_for_release(&wait_for_root, "root release");
        }
    }));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let waiting = gate.clone();
    config.hooks.abandoned = Some(Arc::new(move |key| {
        if key == [0] {
            let _ = abandoned_tx.send(());
            wait_for_release(&waiting, "defer release");
        }
    }));
    config.hooks.needed = Some(Arc::new(move |key| {
        if key == [0] {
            let _ = needed_tx.send(());
        }
    }));
    let opens = Arc::new(AtomicUsize::new(0));
    let open_count = opens.clone();
    config.hooks.before_open = Some(Arc::new(move |key| {
        if key == [0] {
            open_count.fetch_add(1, Ordering::Relaxed);
        }
    }));

    let scan_root = root.clone();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        done_tx
            .send(scan_options(&scan_root, &AtomicBool::new(false), config))
            .unwrap();
    });
    wait_for(&abandoned_rx, "the speculative read to abandon");
    wait_for(
        &root_paused_rx,
        "the parent read to pause after publishing the route",
    );
    release(&root_gate);
    wait_for(&needed_rx, "the reducer to request the route before defer");
    release(&gate);
    let actual = done_rx
        .recv_timeout(Duration::from_secs(60))
        .expect("the deferred required route must become claimable");
    assert_same(actual, serial(&root, &AtomicBool::new(false)));
    assert_eq!(stats.abandons.load(Ordering::Relaxed), 1);
    assert!(opens.load(Ordering::Relaxed) >= 2);
    assert_released(&stats, 4);
}

#[test]
fn queue_bounds_finished_speculation_with_many_empty_siblings() {
    let home = Scratch::new();
    let root = home.0.join("scan");
    fs::create_dir(&root).unwrap();
    for index in 0..14 {
        let sibling = root.join(format!("sibling-{index:02}"));
        fs::create_dir(&sibling).unwrap();
        for child in 0..30 {
            fs::create_dir(sibling.join(format!("empty-{child:03}"))).unwrap();
        }
    }
    let root_fd = open_root(&root).unwrap();
    let mut order = vec![];
    entries(&root_fd, |entry| {
        order.push(entry.name);
        true
    })
    .unwrap();
    drop(root_fd);
    assert!(order.len() >= 8);

    let mut config = options(4);
    config.window = 64 * 1024;
    let window = config.window;
    let stats = config.stats.clone();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (release_first, first_rx) = std::sync::mpsc::channel();
    let first_rx = Arc::new(Mutex::new(first_rx));
    let wait_for_siblings = first_rx.clone();
    let completed_siblings = Arc::new(AtomicUsize::new(0));
    let completed = completed_siblings.clone();
    config.hooks.before_open = Some(Arc::new(move |key| {
        if key == [0] {
            let _ = release_tx.send(());
            wait_for(
                &wait_for_siblings.lock().unwrap(),
                "later sibling receipts to finish",
            );
        }
    }));
    config.hooks.completed = Some(Arc::new(move |key| {
        if key.len() == 1 && key[0] != 0 && completed.fetch_add(1, Ordering::Relaxed) == 2 {
            let _ = release_first.send(());
        }
    }));

    let scan_root = root.clone();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        done_tx
            .send(scan_options(&scan_root, &AtomicBool::new(false), config))
            .unwrap();
    });
    wait_for(&release_rx, "the first directory to block");
    let actual = done_rx
        .recv_timeout(Duration::from_secs(60))
        .expect("bounded speculative output must let the scan finish");
    assert_same(actual, serial(&root, &AtomicBool::new(false)));
    let workers = stats.effective_workers.load(Ordering::Relaxed);
    let reservation = window / (workers + 2);
    assert!(stats.peak_speculative_receipt_bytes.load(Ordering::Relaxed) > reservation);
    assert!(
        stats.peak_speculative_receipt_bytes.load(Ordering::Relaxed)
            <= window + workers * reservation
    );
    assert_released(&stats, 4);
}

#[test]
fn queue_rejects_identity_replacements_and_stays_within_fd_budget() {
    let home = Scratch::new();
    fs::create_dir(home.0.join("child")).unwrap();
    fs::write(home.0.join("child/original"), b"original").unwrap();
    let saved = home.0.join("saved");
    let root = home.0.clone();
    let mut config = options(4);
    let root_done = Arc::new((Mutex::new(false), Condvar::new()));
    let completed = root_done.clone();
    config.hooks.completed = Some(Arc::new(move |key| {
        if key.is_empty() {
            *completed.0.lock().unwrap() = true;
            completed.1.notify_all();
        }
    }));
    config.hooks.before_open = Some(Arc::new(move |key| {
        if !key.is_empty() {
            let (done, timeout) = root_done
                .1
                .wait_timeout_while(
                    root_done.0.lock().unwrap(),
                    Duration::from_secs(10),
                    |done| !*done,
                )
                .unwrap();
            assert!(*done && !timeout.timed_out());
            fs::rename(root.join("child"), &saved).unwrap();
            fs::create_dir(root.join("child")).unwrap();
            fs::write(root.join("child/replacement"), b"replacement").unwrap();
        }
    }));
    let stats = config.stats.clone();
    let tree = scan_options(&home.0, &AtomicBool::new(false), config);
    assert_eq!(tree.nodes[0].descendants, 1);
    assert_eq!(tree.nodes[0].unreadable, 1);
    assert_eq!(tree.nodes[1].descendants, 0);
    assert_released(&stats, 4);
}

#[test]
fn queue_partial_worker_spawn_failure_joins_started_workers_and_releases_descriptors() {
    let home = Scratch::new();
    fs::write(home.0.join("file"), b"contents").unwrap();
    let baseline = descriptor_count();
    let mut config = options(4);
    config.hooks.spawn_failure = Some(Arc::new(|index| index == 1));
    let stats = config.stats.clone();
    let actual = scan_options(&home.0, &AtomicBool::new(false), config);
    assert_same(actual, serial(&home.0, &AtomicBool::new(false)));
    assert!(stats.effective_workers.load(Ordering::Relaxed) >= 2);
    assert_released(&stats, 4);
    if let (Some(before), Some(after)) = (baseline, descriptor_count()) {
        assert_eq!(after, before);
    }

    let mut config = options(4);
    config.hooks.spawn_failure = Some(Arc::new(|_| true));
    let stats = config.stats.clone();
    let failed = scan_options(&home.0, &AtomicBool::new(false), config);
    assert!(matches!(failed.state, ScanState::Failed(_)));
    assert_released(&stats, 4);
}

#[test]
fn queue_coordinator_callback_panic_stops_and_joins_workers() {
    let home = Scratch::new();
    fs::write(home.0.join("file"), b"contents").unwrap();
    let stats = Arc::<Stats>::default();
    let worker_stats = stats.clone();
    let root = home.0.clone();
    let (sent, received) = std::sync::mpsc::channel();
    let runner = std::thread::spawn(move || {
        let before = descriptor_count();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            foreground_priority();
            let _tree = scan_opened(
                &root,
                &root,
                &AtomicBool::new(false),
                Callbacks {
                    progress: &mut |_| {},
                    on_tick: &mut || panic!("injected coordinator callback panic"),
                    last_tick: &mut (Instant::now() - Duration::from_secs(1)),
                },
                || open_root(&root),
                Options {
                    workers: 4,
                    initial_urgency: Urgency::Foreground,
                    stats: worker_stats.clone(),
                    ..Options::default()
                },
            );
        }));
        let after = descriptor_count();
        sent.send((result.is_err(), before, after)).unwrap();
    });
    let (panicked, before, after) = received
        .recv_timeout(Duration::from_secs(10))
        .expect("callback panic must not hang scoped worker joins");
    runner.join().unwrap();
    assert!(panicked);
    if let (Some(before), Some(after)) = (before, after) {
        assert_eq!(after, before);
    }
    assert_released(&stats, 4);
}

#[test]
fn queue_coordinator_ticks_while_reading_and_workers_restore_actual_qos() {
    let home = Scratch::new();
    for index in 0..240 {
        fs::write(home.0.join(format!("file-{index}")), b"data").unwrap();
    }
    let mut config = options(4);
    config.hooks.after_entry = Some(Arc::new(|_| std::thread::sleep(Duration::from_millis(3))));
    let readbacks = Arc::new(Mutex::new(vec![]));
    let observed = readbacks.clone();
    config.hooks.priority = Some(Arc::new(move |index, urgency, restored| {
        #[cfg(target_os = "macos")]
        {
            let mut class = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
            let mut relative = 0;
            assert_eq!(
                unsafe {
                    libc::pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut relative)
                },
                0
            );
            let expected = if urgency == Urgency::Foreground {
                25
            } else {
                17
            };
            assert_eq!(class as u32, expected);
        }
        observed
            .lock()
            .unwrap()
            .push((index, urgency == Urgency::Foreground, restored));
    }));
    let stats = config.stats.clone();
    let ticks = std::cell::Cell::new(0);
    let mut progress = vec![];
    foreground_priority();
    let tree = scan_opened(
        &home.0,
        &home.0,
        &AtomicBool::new(false),
        Callbacks {
            progress: &mut |tree| progress.push((Instant::now(), tree.nodes[0].descendants)),
            on_tick: &mut || {
                ticks.set(ticks.get() + 1);
                if ticks.get() == 1 {
                    Urgency::Background
                } else {
                    Urgency::Foreground
                }
            },
            last_tick: &mut Instant::now(),
        },
        || open_root(&home.0),
        config,
    );
    background_priority();
    assert_eq!(tree.state, ScanState::Complete);
    assert!(ticks.get() >= 3);
    assert!(progress.iter().any(|(_, count)| *count == 0));
    assert!(progress
        .windows(2)
        .all(|pair| pair[1].0.duration_since(pair[0].0) < Duration::from_millis(500)));
    for index in 0..4 {
        assert!(readbacks.lock().unwrap().contains(&(index, true, false)));
        assert!(readbacks.lock().unwrap().contains(&(index, false, true)));
    }
    assert_released(&stats, 4);
}

#[test]
fn queue_cancellation_wakes_idle_workers_and_joins_readers() {
    let home = Scratch::new();
    for index in 0..240 {
        fs::write(home.0.join(format!("file-{index}")), b"data").unwrap();
    }
    for width in [1, 2, 4, 8, 14] {
        let mut config = options(width);
        config.hooks.after_entry = Some(Arc::new(|_| std::thread::sleep(Duration::from_millis(2))));
        let stats = config.stats.clone();
        let cancel = AtomicBool::new(false);
        let stopped = std::cell::Cell::new(None);
        foreground_priority();
        let tree = scan_opened(
            &home.0,
            &home.0,
            &cancel,
            Callbacks {
                progress: &mut |_| {
                    if stopped.get().is_none() {
                        stopped.set(Some(Instant::now()));
                        cancel.store(true, Ordering::Relaxed);
                    }
                },
                on_tick: &mut || Urgency::Foreground,
                last_tick: &mut Instant::now(),
            },
            || open_root(&home.0),
            config,
        );
        background_priority();
        assert_eq!(tree.state, ScanState::Cancelled);
        assert_eq!(tree.nodes[0].descendants, 0);
        assert!(stopped.get().unwrap().elapsed() < Duration::from_secs(2));
        assert_released(&stats, width);
    }
}

#[test]
fn queue_admission_limits_normal_receipts_while_first_directory_is_delayed() {
    let home = Scratch::new();
    fs::write(home.0.join("source"), [1; 4096]).unwrap();
    let root = home.0.join("scan");
    fs::create_dir(&root).unwrap();
    for index in 0..64 {
        let branch = root.join(format!("child-{index:03}"));
        fs::create_dir(&branch).unwrap();
        for file in 0..100 {
            fs::hard_link(
                home.0.join("source"),
                branch.join(format!("file-{file:03}")),
            )
            .unwrap();
        }
    }
    let mut config = options(8);
    config.window = 256 * 1024;
    let stats = config.stats.clone();
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let waiting = release.clone();
    config.hooks.before_open = Some(Arc::new(move |key| {
        if key == [0] {
            let (done, timeout) = waiting
                .1
                .wait_timeout_while(waiting.0.lock().unwrap(), Duration::from_secs(10), |done| {
                    !*done
                })
                .unwrap();
            assert!(
                *done && !timeout.timed_out(),
                "later receipts must fill the admission window"
            );
        }
    }));
    let fill = stats.clone();
    config.hooks.completed = Some(Arc::new(move |key| {
        if !key.is_empty() && fill.receipt_bytes.load(Ordering::Relaxed) >= 128 * 1024 {
            *release.0.lock().unwrap() = true;
            release.1.notify_all();
        }
    }));
    let tree = scan_options(&root, &AtomicBool::new(false), config);
    assert_same(tree, serial(&root, &AtomicBool::new(false)));
    assert!(stats.peak_receipt_bytes.load(Ordering::Relaxed) >= 128 * 1024);
    assert!(stats.peak_receipt_bytes.load(Ordering::Relaxed) <= 256 * 1024);
    assert_eq!(stats.oversized.load(Ordering::Relaxed), 0);
    assert_eq!(stats.required_over_window.load(Ordering::Relaxed), 0);
    assert_released(&stats, 8);
}

#[test]
fn queue_effective_width_preserves_descriptor_reserve() {
    #[cfg(target_os = "macos")]
    for (limit, used, requested, expected) in [
        (256, 3, 14, (14, 32)),
        (100, 3, 14, (14, 4)),
        (80, 10, 14, (2, 1)),
        (70, 10, 14, (1, 0)),
        (256, 3, 1, (1, 32)),
    ] {
        assert_eq!(limits_for(requested, limit, used), expected);
    }
}

#[test]
fn queue_first_progress_does_not_depend_on_a_recent_scope_tick() {
    let stats = Arc::<Stats>::default();
    let queue = Queue {
        state: Mutex::new(State {
            ready: BTreeMap::new(),
            deferred: BTreeMap::new(),
            finished: BTreeMap::new(),
            active: BTreeSet::new(),
            needed: None,
            reserved: 0,
            stopping: false,
        }),
        wake: Condvar::new(),
        reservation: 1024,
        window: 4096,
        stats: stats.clone(),
        background: Mutex::new(()),
        abandoned_hook: None,
        needed_hook: None,
    };
    let cancel = AtomicBool::new(false);
    let control = Control {
        cancel: &cancel,
        stop: AtomicBool::new(false),
        foreground: AtomicBool::new(false),
    };
    let config = options(1);
    let progress = std::cell::Cell::new(0);
    let ticks = std::cell::Cell::new(0);
    let mut on_progress = |_: &Tree| progress.set(progress.get() + 1);
    let mut on_tick = || {
        ticks.set(ticks.get() + 1);
        Urgency::Background
    };
    let mut last_tick = Instant::now();
    let mut coordinator = Coordinator {
        queue: &queue,
        control: &control,
        options: &config,
        root: Path::new("/fixture"),
        progress: &mut on_progress,
        on_tick: &mut on_tick,
        last_tick: &mut last_tick,
        last_report: Instant::now(),
        reported: false,
    };
    let tree = Tree::new(Path::new("/fixture"));
    coordinator.service(&tree, true);
    coordinator.service(&tree, true);
    assert_eq!(progress.get(), 1);
    assert_eq!(ticks.get(), 0);
}

#[test]
fn queue_cancellation_splits_plain_runs_and_unwinds_candidates_like_oracle() {
    let home = Scratch::new();
    fs::create_dir_all(home.0.join("node_modules/pkg")).unwrap();
    for index in 0..120 {
        fs::write(
            home.0.join(format!("node_modules/pkg/file-{index:03}")),
            [1; 4096],
        )
        .unwrap();
    }
    for width in [1, 2, 4, 8, 14] {
        for cut in [0, 1, 2, 3, 17, 81] {
            let mut config = options(width);
            config.hooks.cancel_after = Some(cut);
            let stats = config.stats.clone();
            let actual = scan_options(&home.0, &AtomicBool::new(false), config);
            foreground_priority();
            let expected = serial_oracle::scan_until(&home.0, cut);
            background_priority();
            assert_same(actual, expected);
            assert_released(&stats, width);
        }
    }
}
