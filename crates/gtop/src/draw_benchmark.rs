#[cfg(target_os = "macos")]
use super::*;
#[cfg(target_os = "macos")]
use std::prelude::v1::test;
thread_local! {
    static FIXED_CLOCK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static RENDERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) fn fixed_clock() -> bool {
    FIXED_CLOCK.with(|v| v.get())
}

pub(super) fn record_render() {
    RENDERS.with(|count| count.set(count.get() + 1));
}

fn view_selected(name: &str, selected_views: Option<&str>) -> bool {
    match selected_views {
        Some(views) => views.split(',').any(|view| view == name),
        None => name != "disk",
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::view_selected;
    use std::prelude::v1::test;

    #[test]
    fn disk_draw_view_is_opt_in() {
        for view in ["live", "history1h", "history24h", "detail"] {
            assert!(view_selected(view, None), "default excludes {view}");
        }
        assert!(!view_selected("disk", None));
        assert!(view_selected("disk", Some("disk")));
        assert!(!view_selected("live", Some("disk")));
    }
}

#[cfg(target_os = "macos")]
fn dimension(name: &str, default: usize) -> usize {
    let maximum = match name {
        "GTOP_DRAW_PROCESSES" => 100_000,
        "GTOP_DRAW_CORES" => 256,
        "GTOP_DRAW_N" => 100_000,
        _ => 16_384,
    };
    let value =
        std::env::var(name).map_or(default, |v| v.parse().expect("invalid benchmark dimension"));
    assert!(
        (1..=maximum).contains(&value),
        "{name} must be in 1..={maximum}"
    );
    value
}

#[cfg(target_os = "macos")]
fn fixture(window: &mut Window, cx: &mut Context<Monitor>) -> Monitor {
    let settings = palette::Settings::default();
    let base = settings.base();
    let focus = cx.focus_handle();
    let snapshot = Rc::new({
        let mut s = test_data::snapshot();
        let processes = dimension("GTOP_DRAW_PROCESSES", 1000);
        let cores = dimension("GTOP_DRAW_CORES", 14);
        s.stats.cpu_usage = (0..cores).map(|i| 15. + (i % 14) as f32 * 5.).collect();
        let total: f32 = s.stats.cpu_usage.iter().sum();
        let active = processes.min(16);
        let weight = (active * (active + 1) / 2) as f32;
        s.processes = (1..=u32::try_from(processes).unwrap())
            .map(|pid| {
                let cpu = if pid as usize <= active {
                    total * (active + 1 - pid as usize) as f32 / weight
                } else {
                    0.
                };
                test_data::process(pid, &format!("process-{pid:04}"), cpu)
            })
            .collect::<Vec<_>>()
            .into();
        assert_eq!(s.processes.len(), processes);
        let process_cpu: f32 = s.processes.iter().map(|p| p.cpu_usage).sum();
        assert!((process_cpu - total).abs() < 0.01);
        assert!(process_cpu <= cores as f32 * 100. + 0.01);
        s.stats.memory_total = 32 << 30;
        s.stats.memory_used = 19 << 30;
        s.stats.memory_free = 13 << 30;
        s
    });
    let table = cx.new(|_| {
        let mut table = Processes::default();
        table.snapshot = Some(snapshot.clone());
        table.base = base;
        table.root_focus = Some(focus.clone());
        table.reorder();
        table
    });
    let history = std::array::from_fn(|index| {
        let count = [60, 480, 1440][index];
        let start_minute = 30_000_000 - count as u64;
        let mut rows: Vec<_> = snapshot
            .processes
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    history::ProcessTotal {
                        cpu_seconds: p.cpu_usage as f64 / 100. * count as f64 * 60.,
                        gpu_seconds: 0.,
                        peak_memory: p.memory_usage,
                        peak_cpu: p.cpu_usage,
                    },
                )
            })
            .collect();
        rows.sort_by(|a, b| {
            b.1.cpu_seconds
                .total_cmp(&a.1.cpu_seconds)
                .then(a.0.cmp(&b.0))
        });
        assert!(rows.windows(2).all(|pair| {
            pair[0].1.cpu_seconds > pair[1].1.cpu_seconds
                || (pair[0].1.cpu_seconds == pair[1].1.cpu_seconds && pair[0].0 <= pair[1].0)
        }));
        assert!(rows
            .iter()
            .all(|row| row.1.cpu_seconds <= rows[0].1.cpu_seconds));
        let mut view = history::HistoryView {
            columns: (0..count)
                .map(|i| {
                    Some(history::Column {
                        minute: start_minute + i as u64,
                        seconds: 60.,
                        cpu: snapshot
                            .stats
                            .cpu_usage
                            .iter()
                            .map(|v| *v as f64)
                            .sum::<f64>()
                            / snapshot.stats.cpu_usage.len() as f64,
                        memory: 59.,
                        gpu: Some(20.),
                        temperature: Some(65.),
                        peak_temperature: Some(80.),
                        gpu_seconds: 60.,
                        temperature_seconds: 60.,
                    })
                })
                .collect(),
            start_minute,
            total_cpu: rows.iter().map(|(_, p)| p.cpu_seconds).sum(),
            covered: count,
            cached_leaders: Default::default(),
            rows,
        };
        view.cached_leaders = history::Metric::ALL.map(|metric| view.leaders(metric));
        for metric in history::Metric::ALL {
            assert_eq!(view.cached_leaders[metric as usize], view.leaders(metric));
        }
        assert_eq!(
            view.cached_leaders[0].len(),
            snapshot.processes.len().min(3)
        );
        view
    });
    let colors = palette::ColorField::ALL.map(|field| {
        cx.new(|cx| InputState::new(window, cx).default_value(format!("#{:06x}", field.get(base))))
    });
    let color_pickers = palette::ColorField::ALL.map(|field| {
        cx.new(|cx| ColorPickerState::new(window, cx).default_value(rgb(field.get(base))))
    });
    let subscriptions = Monitor::palette_subscriptions(&colors, &color_pickers, window, cx);
    Monitor {
        core: None,
        snapshot: Some(snapshot.clone()),
        table,
        search: cx.new(|cx| InputState::new(window, cx).placeholder("name, pid, command or port")),
        details: None,
        disk: None,
        paused: false,
        history,
        moment: None,
        moment_detail: None,
        moment_selected: 0,
        moment_rows: vec![],
        view: None,
        history_selected: None,
        history_scroll: UniformListScrollHandle::new(),
        history_columns: crate::column_widths::TableState::default(),
        metric: history::Metric::Cpu,
        settings,
        settings_writes: None,
        settings_writer: None,
        settings_open: false,
        tick_ms: 12.,
        draw_metrics: DrawMetrics::default(),
        sample_seconds: 3.,
        last_snapshot: None,
        focus,
        draft: None,
        colors,
        color_pickers,
        mix_name: cx.new(|cx| InputState::new(window, cx)),
        traces: [(
            ProcessKey::from(&snapshot.processes[0]),
            (0..90)
                .map(|i| (1_800_000_000. - 89. + i as f64, (i % 50) as f32))
                .collect(),
        )]
        .into(),
        interval: 3,
        status: String::new(),
        pending_kill: None,
        wake: mpsc::channel(1).0,
        _subscriptions: subscriptions,
        _refresh: Some(Task::ready(())),
        shutdown: None,
        _ports: Task::ready(()),
        _frames: Task::ready(()),
        _perf_writer: Task::ready(()),
    }
}

#[cfg(target_os = "macos")]
fn thread_cpu_ns() -> u64 {
    unsafe extern "C" {
        fn clock_gettime_nsec_np(clock_id: u32) -> u64;
    }
    // Darwin CLOCK_THREAD_CPUTIME_ID.
    unsafe { clock_gettime_nsec_np(16) }
}

#[test]
#[ignore = "release-only full Monitor draw CPU benchmark"]
#[cfg(target_os = "macos")]
fn full_monitor_draw_benchmark() {
    println!("draw_benchmark pid={} errors=0 timer=Darwin_CLOCK_THREAD_CPUTIME_ID excludes=arena_clear,scene_hash,present,gpu", std::process::id());
    struct ClockGuard;
    impl Drop for ClockGuard {
        fn drop(&mut self) {
            FIXED_CLOCK.with(|v| v.set(false));
        }
    }
    let _clock = ClockGuard;
    FIXED_CLOCK.with(|v| v.set(true));
    let text_system = Arc::new(gpui_apple::AppleTextSystem::new());
    let mut cx = TestAppContext::build_with_text_system(
        TestDispatcher::new(0),
        Some("full_monitor_draw_benchmark"),
        text_system,
    );
    cx.update(|cx| {
        #[cfg(gtop_draw_benchmark)]
        cx.draw_benchmark_assets(gpui_kit_assets::Assets);
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
    let width = dimension("GTOP_DRAW_WIDTH", 1480) as f32;
    let height = dimension("GTOP_DRAW_HEIGHT", 960) as f32;
    let mut monitor = None;
    let handle = cx.open_window(size(px(width), px(height)), |window, cx| {
        let entity = cx.new(|cx| fixture(window, cx));
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let monitor = monitor.unwrap();
    let n = dimension("GTOP_DRAW_N", 31);
    #[cfg(gtop_draw_benchmark)]
    let observed =
        std::env::var("GTOP_DRAW_MODE").unwrap_or_else(|_| "validate".into()) == "validate";
    #[cfg(gtop_draw_benchmark)]
    gpui::draw_benchmark_observe(observed);
    assert!(n >= 5);
    let selected_views = std::env::var("GTOP_DRAW_VIEWS").ok();
    if let Some(views) = selected_views.as_ref() {
        assert!(
            views
                .split(',')
                .all(|name| ["live", "history1h", "history24h", "detail", "disk"].contains(&name)),
            "unknown benchmark view"
        );
    }
    #[cfg(gtop_draw_benchmark)]
    let scales = vec![1., 2.];
    #[cfg(not(gtop_draw_benchmark))]
    let scales = vec![2.];
    for scale in scales {
        #[cfg(gtop_draw_benchmark)]
        cx.draw_benchmark_scale(handle.into(), scale);
        for (name, view, detail) in [
            ("live", None, false),
            ("history1h", Some(0), false),
            ("history24h", Some(2), false),
            ("detail", None, true),
            ("disk", None, false),
        ] {
            if !view_selected(name, selected_views.as_deref()) {
                continue;
            }
            cx.update(|cx| {
                monitor.update(cx, |m, cx| {
                    m.view = view;
                    m.details = None;
                    m.disk = None;
                    if name == "disk" {
                        m.prepare_disk_draw_benchmark();
                    }
                    if detail {
                        m.select(
                            ProcessKey::from(&m.snapshot.as_ref().unwrap().processes[0]),
                            cx,
                        );
                        let mut analysis = analysis::analyze(
                            "process-0001",
                            "/bin/process-0001",
                            1.,
                            30_000_000,
                            [],
                        );
                        analysis.history = Some(analysis::ProcessHistory {
                            columns: (0..144).map(|i| Some((i % 60) as f32)).collect(),
                            pattern: "Recurring work".into(),
                            now: "Running".into(),
                            core_minutes: 120.,
                            busy_average: 35.,
                            peak_cpu: 60.,
                            peak_minute: 29_999_900,
                            longest_busy_minutes: 60,
                            peak_memory: 512 << 20,
                            recorded_minutes: 1440,
                        });
                        m.details.as_mut().unwrap().analysis = Some(analysis);
                    }
                    cx.notify();
                })
            });
            let mut samples = Vec::new();
            #[cfg(gtop_draw_benchmark)]
            let mut scenes = Vec::new();
            for i in 0..n + 5 {
                let elapsed = cx
                    .update_window(handle.into(), |_, window, cx| {
                        assert_eq!(window.viewport_size(), size(px(width), px(height)));
                        window.refresh();
                        let before = RENDERS.with(|n| n.get());
                        let start = thread_cpu_ns();
                        let clear = window.draw(cx);
                        let elapsed = thread_cpu_ns() - start;
                        clear.clear(cx);
                        assert_eq!(
                            RENDERS.with(|n| n.get()),
                            before + 1,
                            "full Monitor render must run inside timing"
                        );
                        elapsed
                    })
                    .unwrap();
                assert!(elapsed > 0);
                if i >= 5 {
                    samples.push(elapsed as f64 / 1_000_000.);
                    #[cfg(gtop_draw_benchmark)]
                    if observed {
                        scenes.push(
                            cx.update_window(handle.into(), |_, w, _| w.draw_benchmark_scene())
                                .unwrap(),
                        );
                    }
                }
            }
            #[cfg(gtop_draw_benchmark)]
            if observed {
                cx.update_window(handle.into(), |_, w, _| w.draw_benchmark_equality_probes())
                    .unwrap();
                println!("equality_probes view={name} scale={scale} atlas_bytes=passed color_space=passed");
                assert!(
                    scenes.iter().all(|scene| scene == &scenes[0]),
                    "scene changed across identical full draws: {scenes:?}"
                );
                assert!(scenes[0].0 > 0 && scenes[0].1 > 0 && scenes[0].3 > 0 && scenes[0].4 > 0);
                println!("scene view={name} scale={scale} paths={} vertices={} primitives={} elements={} glyph_sprites={} hash={:016x} equal={n}/{n}", scenes[0].0, scenes[0].1, scenes[0].2, scenes[0].3, scenes[0].4, scenes[0].5);
            }
            println!("samples view={name} scale={scale} thread_cpu_ms={samples:?}");
            if name == "disk" {
                cx.read(|cx| monitor.read(cx).assert_disk_scan_pending());
            }
            samples.sort_by(f64::total_cmp);
            println!("draw view={name} scale={scale} median_ms={:.6} min_ms={:.6} max_ms={:.6} n={n} errors=0 draws={} fixture_processes={} cores={} size={width}x{height} text=platform", (samples[(n-1)/2] + samples[n/2]) / 2., samples[0], samples[n-1], n+5, cx.read(|cx| monitor.read(cx).snapshot.as_ref().unwrap().processes.len()), cx.read(|cx| monitor.read(cx).snapshot.as_ref().unwrap().stats.cpu_usage.len()));
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "full Monitor draws share glyph instrumentation; run this test in isolation"]
fn disk_keyboard_scroll_reveals_selected_row_after_summary() {
    let home = std::env::temp_dir().join(format!("gtop-scroll-regression-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    for i in 0..160 {
        std::fs::write(home.join(format!("file-{i:03}")), [1u8; 4096]).unwrap();
    }
    let mut cx = TestAppContext::build_with_text_system(
        TestDispatcher::new(0),
        Some("review-ui"),
        Arc::new(gpui_apple::AppleTextSystem::new()),
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
    let handle = cx.open_window(size(px(1480.), px(960.)), |window, cx| {
        let entity = cx.new(|cx| fixture(window, cx));
        entity.update(cx, |m, cx| {
            m.prepare_disk_scroll_test(home.clone());
            window.focus(&m.focus, cx);
        });
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let m = monitor.unwrap();
    let mut visual = VisualTestContext::from_window(handle.into(), &cx);
    cx.update_window(handle.into(), |_, w, cx| {
        w.refresh();
        w.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(handle.into(), "down");
    cx.update_window(handle.into(), |_, w, cx| {
        w.refresh();
        w.draw(cx).clear(cx);
    })
    .unwrap();
    assert_eq!(cx.update(|cx| m.read(cx).disk_selected_for_test()), 127);
    let body = visual.debug_bounds("disk-body").unwrap();
    let selected = visual.debug_bounds("disk-row-127").unwrap();
    println!("selected={selected:?} viewport={body:?}");
    assert!(selected.top() >= body.top());
    assert!(selected.bottom() <= body.bottom());
    cx.update(|cx| m.update(cx, |m, _| m.disk.as_mut().unwrap().close()));
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "offscreen palette review; set GTOP_PALETTE_RENDER_DIR"]
fn aluminium_offscreen_review() {
    let directory = std::path::PathBuf::from(
        std::env::var("GTOP_PALETTE_RENDER_DIR").expect("render directory"),
    );
    std::fs::create_dir_all(&directory).unwrap();
    FIXED_CLOCK.with(|v| v.set(true));
    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(gpui_kit_assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
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
            let entity = cx.new(|cx| fixture(window, cx));
            entity.update(cx, |m, cx| {
                m.settings.active = 4;
                let base = m.settings.base();
                m.table.update(cx, |table, cx| {
                    table.base = base;
                    cx.notify();
                });
                theme::apply(base, cx);
                for field in palette::ColorField::ALL {
                    m.color_pickers[field as usize].update(cx, |picker, cx| {
                        picker.set_value(rgb(field.get(base)), window, cx)
                    });
                    m.colors[field as usize].update(cx, |input, cx| {
                        input.set_value(format!("#{:06x}", field.get(base)), window, cx)
                    });
                }
            });
            monitor = Some(entity.clone());
            cx.new(|cx| Root::new(entity, window, cx))
        })
        .unwrap();
    let monitor = monitor.unwrap();
    for (name, view, detail) in [
        ("live", None, false),
        ("history", Some(2), false),
        ("detail", None, true),
        ("disk", None, false),
        ("palette", None, false),
        ("picker", None, false),
    ] {
        cx.update(|cx| {
            monitor.update(cx, |m, cx| {
                m.view = view;
                m.settings_open = matches!(name, "palette" | "picker");
                if name == "picker" {
                    m.color_pickers[palette::ColorField::Led as usize]
                        .update(cx, |picker, cx| picker.set_open(true, cx));
                }
                m.details = None;
                m.disk = None;
                if name == "disk" {
                    m.prepare_disk_draw_benchmark();
                }
                if detail {
                    m.select(
                        ProcessKey::from(&m.snapshot.as_ref().unwrap().processes[0]),
                        cx,
                    );
                }
                cx.notify();
            })
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
        cx.capture_screenshot(handle.into())
            .unwrap()
            .save(directory.join(format!("{name}.png")))
            .unwrap();
    }
    FIXED_CLOCK.with(|v| v.set(false));
}

#[cfg(target_os = "macos")]
#[test]
fn picker_events_update_live_palette_and_coalesce_saved_settings() {
    let mut cx = TestAppContext::build_with_text_system(
        TestDispatcher::new(0),
        Some("palette-events"),
        Arc::new(gpui_apple::AppleTextSystem::new()),
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        theme::init(cx);
    });
    let (sender, mut receiver) = mpsc::unbounded();
    let mut monitor = None;
    let handle = cx.open_window(size(px(1480.), px(960.)), |window, cx| {
        let entity = cx.new(|cx| fixture(window, cx));
        entity.update(cx, |m, _| {
            m.settings_writes = Some(sender);
        });
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let monitor = monitor.unwrap();
    let original = palette::presets()[0].base;
    for field in palette::ColorField::ALL {
        cx.update_window(handle.into(), |_, window, cx| {
            let picker = monitor.read(cx).color_pickers[field as usize].clone();
            picker.update(cx, |picker, cx| {
                picker.update_color(gpui::rgba(0x12345680).into(), window, cx)
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let m = monitor.read(cx);
            assert_eq!(field.get(m.settings.base()), 0x123456);
            assert_eq!(m.table.read(cx).base, m.settings.base());
            assert_eq!(m.settings.saved.len(), 1);
            assert_eq!(
                m.color_pickers[field as usize].read(cx).value(),
                Some(rgb(0x123456).into())
            );
            assert_eq!(m.settings.active, palette::presets().len());
            assert_eq!(
                m.colors[field as usize].read(cx).value().as_str(),
                "#123456"
            );
        });
    }
    cx.update_window(handle.into(), |_, window, cx| {
        let input = monitor.read(cx).colors[palette::ColorField::Led as usize].clone();
        window.focus(&input.read(cx).focus_handle(cx), cx);
        input.update(cx, |input, cx| {
            input.set_value("#654321", window, cx);
            cx.emit(InputEvent::Change);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let m = monitor.read(cx);
        assert_eq!(m.settings.base().led, 0x654321);
        assert!(m.colors[palette::ColorField::Led as usize]
            .read(cx)
            .focus_handle(cx)
            .is_focused(window));
        assert_eq!(
            m.color_pickers[palette::ColorField::Led as usize]
                .read(cx)
                .value(),
            Some(rgb(0x654321).into())
        );
    })
    .unwrap();
    let latest = futures::executor::block_on(palette::next_settings(&mut receiver)).unwrap();
    assert_eq!(latest.base().led, 0x654321);
    assert_eq!(latest.saved.len(), 1);
    assert_eq!(palette::presets()[0].base, original);
}
