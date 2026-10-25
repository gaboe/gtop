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
        colors: std::array::from_fn(|_| cx.new(|cx| InputState::new(window, cx))),
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
        _subscriptions: vec![],
        _refresh: Task::ready(()),
        _ports: Task::ready(()),
        _frames: Task::ready(()),
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
                .all(|name| ["live", "history1h", "history24h", "detail"].contains(&name)),
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
        ] {
            if selected_views
                .as_ref()
                .is_some_and(|views| !views.split(',').any(|v| v == name))
            {
                continue;
            }
            cx.update(|cx| {
                monitor.update(cx, |m, cx| {
                    m.view = view;
                    m.details = None;
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
            samples.sort_by(f64::total_cmp);
            println!("draw view={name} scale={scale} median_ms={:.6} min_ms={:.6} max_ms={:.6} n={n} errors=0 draws={} fixture_processes={} cores={} size={width}x{height} text=platform", (samples[(n-1)/2] + samples[n/2]) / 2., samples[0], samples[n-1], n+5, cx.read(|cx| monitor.read(cx).snapshot.as_ref().unwrap().processes.len()), cx.read(|cx| monitor.read(cx).snapshot.as_ref().unwrap().stats.cpu_usage.len()));
        }
    }
}
