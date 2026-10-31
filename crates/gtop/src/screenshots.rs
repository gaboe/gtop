use super::*;

fn snapshot() -> Snapshot {
    let mut snapshot = test_data::snapshot();
    let names = [
        "Xcode",
        "Safari",
        "rust-analyzer",
        "WindowServer",
        "cargo",
        "node",
        "Slack",
        "Spotify",
        "OrbStack",
        "kernel_task",
        "Finder",
        "mds_stores",
        "Safari Web Content",
        "Terminal",
        "Code",
        "rustc",
        "Docker",
        "launchd",
        "Activity Monitor",
        "loginwindow",
        "Dock",
        "SystemUIServer",
        "ControlCenter",
        "NotificationCenter",
        "coreaudiod",
        "bluetoothd",
        "sharingd",
        "bird",
        "cloudd",
        "distnoted",
        "cfprefsd",
        "securityd",
        "syslogd",
        "timed",
        "locationd",
        "powerd",
        "logd",
        "configd",
        "postgres",
        "redis-server",
    ];
    snapshot.processes = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let mut p = test_data::process(1000 + i as u32, name, 65. / (1. + i as f32 * 0.6));
            p.user = "demo".into();
            p.gpu_usage = matches!(*name, "Xcode" | "Safari" | "WindowServer" | "Spotify")
                .then_some(8. + i as f32);
            p.memory_usage = (120 + (40 - i) * 24) as u64 * 1024 * 1024;
            p.executable = match *name {
                "Xcode" | "Safari" | "Slack" | "Spotify" | "OrbStack" | "Code" => {
                    format!("/Users/demo/Applications/{name}.app/Contents/MacOS/{name}")
                }
                "cargo" | "rustc" | "rust-analyzer" => format!("/Users/demo/.cargo/bin/{name}"),
                "node" | "postgres" | "redis-server" => format!("/Users/demo/bin/{name}"),
                _ => format!("/System/Library/{name}"),
            };
            p.argv = vec![p.executable.clone()];
            p.command = p.executable.clone();
            p.root = "/Users/demo/Projects/hello-mac".into();
            p.environ = vec!["USER=demo".into(), "HOSTNAME=demo-mac".into()];
            p.threads = Some(4 + i as u32 % 20);
            p.start_time = 1_799_990_000;
            p.run_time = 10_000;
            p.disk_usage = (128_000 + i as u64 * 1024, 32_000);
            p.disk_sample_seconds = Some(3.);
            p.ports = match *name {
                "node" => vec![3000],
                "postgres" => vec![5432],
                "redis-server" => vec![6379],
                _ => vec![],
            };
            p
        })
        .collect::<Vec<_>>()
        .into();
    let s = &mut snapshot.stats;
    s.cpu_usage = vec![38., 52., 28., 44., 18., 25., 12., 20.];
    s.memory_total = 32 << 30;
    s.memory_used = 19 << 30;
    s.memory_free = 13 << 30;
    s.memory_cached = 4 << 30;
    s.swap_used = 256 << 20;
    s.uptime = 172_800;
    s.load_avg = [2.4, 2.1, 1.8];
    s.network_rx_bytes = 2_400_000;
    s.network_tx_bytes = 420_000;
    s.disk_used_bytes = 580_000_000_000;
    s.disk_free_bytes = 414_000_000_000;
    s.disk_purgeable_bytes = 42_000_000_000;
    s.temperatures[0].celsius = 62.;
    s.temperatures.extend(
        [("GPU", 56.), ("SSD", 39.), ("Battery", 31.)].map(|(name, celsius)| {
            gtop_core::monitoring::Temperature {
                name: name.into(),
                celsius,
            }
        }),
    );
    s.gpus = vec![gtop_core::monitoring::GpuInfo {
        name: "Apple M2 Pro".into(),
        vendor: "Apple".into(),
        utilization: Some(24.),
        memory_total: None,
        memory_used: None,
        temperature: Some(56.),
        power_watts: Some(8.),
        core_clock_mhz: Some(900),
    }];
    snapshot.process_observed_at = 1_800_000_000;
    snapshot
}

#[test]
#[ignore = "writes fixture-only README screenshots with headless Metal"]
fn readme_screenshots() {
    struct ClockGuard;
    impl Drop for ClockGuard {
        fn drop(&mut self) {
            FIXED_CLOCK.with(|v| v.set(false));
        }
    }
    let _clock = ClockGuard;
    FIXED_CLOCK.with(|v| v.set(true));
    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(gpui_kit_assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        theme::init(cx);
    });
    for bytes in [
        include_bytes!("../assets/fonts/DotoDisplay.ttf").as_slice(),
        include_bytes!("../assets/fonts/ArchivoDisplay.ttf").as_slice(),
        include_bytes!("../assets/fonts/JetBrainsMono.ttf").as_slice(),
        include_bytes!("../assets/fonts/ArchivoLabels.ttf").as_slice(),
        include_bytes!("../assets/fonts/Silkscreen.ttf").as_slice(),
    ] {
        cx.text_system()
            .add_fonts(vec![std::borrow::Cow::Borrowed(bytes)])
            .unwrap();
    }
    let mut monitor = None;
    let handle = cx
        .open_window(size(px(1480.), px(960.)), |window, cx| {
            let entity = cx.new(|cx| {
                let mut monitor = fixture_from_snapshot(snapshot(), window, cx);
                for view in &mut monitor.history {
                    for (i, column) in view.columns.iter_mut().enumerate() {
                        let column = column.as_mut().unwrap();
                        let wave = (i as f64 / 11.).sin();
                        column.cpu = 26. + 15. * wave + 8. * (i as f64 / 3.).cos();
                        column.memory = 54. + 5. * wave;
                        column.gpu = Some(20. + 12. * wave);
                        column.temperature = Some(58. + 7. * wave);
                        column.io = history::IoHistory {
                            bytes: [
                                Some(144_000_000.),
                                Some(25_200_000.),
                                Some(720_000_000.),
                                Some(180_000_000.),
                            ],
                            peaks: [
                                Some(4_000_000.),
                                Some(800_000.),
                                Some(30_000_000.),
                                Some(8_000_000.),
                            ],
                            seconds: [60.; 4],
                            free: Some(414_000_000_000.),
                            free_seconds: 60.,
                        };
                    }
                }
                monitor
            });
            monitor = Some(entity.clone());
            cx.new(|cx| Root::new(entity, window, cx))
        })
        .expect("headless Metal window");
    let monitor = monitor.unwrap();
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots");
    std::fs::create_dir_all(&output).unwrap();
    for (name, history, detail) in [
        ("live", None, false),
        ("detail", None, true),
        ("history24h", Some(2), false),
    ] {
        cx.update(|cx| {
            monitor.update(cx, |m, cx| {
                m.view = history;
                m.details = None;
                if detail {
                    m.select(
                        ProcessKey::from(&m.snapshot.as_ref().unwrap().processes[0]),
                        cx,
                    );
                    let mut analysis = analysis::analyze(
                        "Xcode",
                        "/Users/demo/Applications/Xcode.app/Contents/MacOS/Xcode",
                        30_000_000,
                        [],
                    );
                    analysis.history = Some(analysis::ProcessHistory {
                        disk_columns: Default::default(),
                        disk_read_bytes: 450_000_000.,
                        disk_write_bytes: 128_000_000.,
                        columns: (0..144)
                            .map(|i| Some(24. + 18. * (i as f32 / 7.).sin()))
                            .collect(),
                        pattern: "Recurring work".into(),
                        core_minutes: 120.,
                        busy_average: 35.,
                        peak_cpu: 80.,
                        peak_minute: 29_999_900,
                        longest_busy_minutes: 45,
                        peak_memory: 1400 << 20,
                        recorded_minutes: 1440,
                    });
                    m.details.as_mut().unwrap().analysis = Some(analysis);
                }
                cx.notify();
            })
        });
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update_window(handle.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
        }
        let image = cx
            .capture_screenshot(handle.into())
            .expect("headless Metal screenshot");
        image.save(output.join(format!("{name}.png"))).unwrap();
        println!("{name}.png {}x{}", image.width(), image.height());
    }
}
