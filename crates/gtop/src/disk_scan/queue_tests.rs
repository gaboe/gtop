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

fn assert_released(stats: &Stats, width: usize) {
    assert_eq!(stats.live_fds.load(Ordering::Relaxed), 0);
    assert_eq!(stats.receipt_bytes.load(Ordering::Relaxed), 0);
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
    for index in 0..12 {
        let directory = home.0.join(format!("child-{index}"));
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("file"), [1; 4096]).unwrap();
    }
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
        let actual = scan_options(&home.0, &AtomicBool::new(false), config);
        assert_ne!(order.lock().unwrap()[0], 0);
        assert_same(actual, serial(&home.0, &AtomicBool::new(false)));
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
    let tree = scan_options(&home.0, &AtomicBool::new(false), config);
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
            finished: BTreeMap::new(),
            needed: None,
            reserved: 0,
            stopping: false,
        }),
        wake: Condvar::new(),
        reservation: 1024,
        window: 4096,
        stats: stats.clone(),
        background: Mutex::new(()),
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
