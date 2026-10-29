#[cfg(target_os = "macos")]
use super::*;
#[cfg(target_os = "macos")]
use std::prelude::v1::test;
thread_local! {
    static FIXED_CLOCK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static LONG_DETAIL_REVIEW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static DETAIL_STRESS_REVIEW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TABLE_RENDERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RENDERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) fn fixed_clock() -> bool {
    FIXED_CLOCK.with(|v| v.get())
}

pub(super) fn record_render() {
    RENDERS.with(|count| count.set(count.get() + 1));
}

pub(super) fn record_table_render() {
    TABLE_RENDERS.with(|count| count.set(count.get() + 1));
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
fn fixed_snapshot() -> Snapshot {
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
            let mut process = test_data::process(pid, &format!("process-{pid:04}"), cpu);
            if pid == 1 && DETAIL_STRESS_REVIEW.with(std::cell::Cell::get) {
                process.executable = "/Users/čšž/日本/app".into();
                process.argv = std::iter::once(process.executable.clone())
                    .chain((0..10_000).map(|i| format!("argument-{i}")))
                    .collect();
                process.command = "x".repeat(200_000);
            } else if pid == 1 && LONG_DETAIL_REVIEW.with(std::cell::Cell::get) {
                process.executable =
                    format!("/Users/gtop/{}worker", "long_directory_name/".repeat(8));
                let long_argument = format!("https://example.invalid/{}", "token".repeat(25));
                process.argv = vec![process.executable.clone(), "--config".into(), long_argument];
                process.command = format!("kernel_task/{}", "renamed_process".repeat(8));
            }
            // Keep fixture rows out of the test process cost readout.
            process.pid += 1_000_000;
            process
        })
        .collect::<Vec<_>>()
        .into();
    assert_eq!(s.processes.len(), processes);
    assert!(s.processes.iter().all(|p| p.pid != std::process::id()));
    let process_cpu: f32 = s.processes.iter().map(|p| p.cpu_usage).sum();
    assert!((process_cpu - total).abs() < 0.01);
    assert!(process_cpu <= cores as f32 * 100. + 0.01);
    s.stats.memory_total = 32 << 30;
    s.stats.memory_used = 19 << 30;
    s.stats.memory_free = 13 << 30;
    s
}

#[cfg(target_os = "macos")]
fn fixture(window: &mut Window, cx: &mut Context<Monitor>) -> Monitor {
    let settings = palette::Settings::default();
    let base = settings.base();
    let focus = cx.focus_handle();
    let snapshot = Rc::new(fixed_snapshot());
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
    let mut subscriptions = Monitor::palette_subscriptions(&colors, &color_pickers, window, cx);
    subscriptions.push(
        cx.subscribe_in(&table, window, |this, _, event, window, cx| {
            window.focus(&this.focus, cx);
            let process_table::SelectProcess(key) = event;
            this.settings_open = false;
            this.select(*key, cx);
        }),
    );
    Monitor {
        core: None,
        snapshot: Some(snapshot.clone()),
        table,
        search: cx.new(|cx| InputState::new(window, cx).placeholder("name, pid, command or port")),
        details: None,
        panel_resize: details::PanelResize::default(),
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
        history_sort: crate::history::HistorySort::default(),
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
        picker_hex_synced: [false; 6],
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
        focus_restore: None,
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

#[cfg(target_os = "macos")]
#[test]
#[ignore = "release-only detail stress draw outlier check"]
fn very_long_detail_open_and_redraw_outlier_check() {
    struct ClockGuard;
    impl Drop for ClockGuard {
        fn drop(&mut self) {
            FIXED_CLOCK.with(|value| value.set(false));
            DETAIL_STRESS_REVIEW.with(|value| value.set(false));
        }
    }
    let _clock = ClockGuard;
    FIXED_CLOCK.with(|value| value.set(true));
    DETAIL_STRESS_REVIEW.with(|value| value.set(true));
    let mut cx = TestAppContext::build_with_text_system(
        TestDispatcher::new(0),
        Some("very_long_detail_open_and_redraw_outlier_check"),
        Arc::new(gpui_apple::AppleTextSystem::new()),
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
    let mut monitor = None;
    let handle = cx.open_window(size(px(1480.), px(960.)), |window, cx| {
        let entity = cx.new(|cx| fixture(window, cx));
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let monitor = monitor.unwrap();
    let key =
        cx.read(|cx| ProcessKey::from(&monitor.read(cx).snapshot.as_ref().unwrap().processes[0]));
    let mut samples = Vec::new();
    for i in 0..16 {
        let start = thread_cpu_ns();
        cx.update(|cx| {
            monitor.update(cx, |monitor, _| monitor.details = None);
        });
        cx.update(|cx| {
            monitor.update(cx, |monitor, cx| monitor.select(key, cx));
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
        let elapsed = thread_cpu_ns() - start;
        assert!(elapsed > 0);
        if i >= 5 {
            samples.push(elapsed as f64 / 1_000_000.);
        }
    }
    samples.sort_by(f64::total_cmp);
    let median = (samples[(samples.len() - 1) / 2] + samples[samples.len() / 2]) / 2.;
    let minimum = samples[0];
    let maximum = samples[samples.len() - 1];
    assert!(
        maximum <= median * 5.,
        "stress draw outlier check: {samples:?}"
    );
    println!(
        "detail_stress_outlier_check arguments=10000 command_bytes=200000 min_ms={minimum:.3} median_ms={median:.3} max_ms={maximum:.3} runs={} bound=5x_median uniform_slowdown_detection=false errors=0",
        samples.len()
    );
}

#[cfg(all(target_os = "macos", gtop_draw_benchmark))]
#[test]
#[ignore = "release-only headless interaction CPU benchmark"]
fn monitor_interaction_benchmark() {
    use gpui::InputEvent;
    FIXED_CLOCK.with(|v| v.set(true));
    let mut cx = TestAppContext::build_with_text_system(
        TestDispatcher::new(0),
        Some("monitor_interaction_benchmark"),
        Arc::new(gpui_apple::AppleTextSystem::new()),
    );
    cx.update(|cx| {
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
    let mut monitor = None;
    let handle = cx.open_window(size(px(1480.), px(960.)), |window, cx| {
        let entity = cx.new(|cx| fixture(window, cx));
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let monitor = monitor.unwrap();
    cx.update_window(handle.into(), |_, w, cx| {
        monitor.update(cx, |m, cx| m.accept(fixed_snapshot(), w, cx));
    })
    .unwrap();
    let n = dimension("GTOP_DRAW_N", 31);
    let mut visual = VisualTestContext::from_window(handle.into(), &cx);
    gpui::draw_benchmark_observe(false);
    println!(
        "interaction_benchmark pid={} timer=Darwin_CLOCK_THREAD_CPUTIME_ID errors=0 size=1480x960",
        std::process::id()
    );
    for view in ["live", "detail", "history", "palette"] {
        cx.update(|cx| {
            monitor.update(cx, |m, cx| {
                m.view = (view == "history").then_some(2);
                m.details = None;
                m.settings_open = view == "palette";
                if view == "detail" {
                    m.select(
                        ProcessKey::from(&m.snapshot.as_ref().unwrap().processes[0]),
                        cx,
                    );
                }
                cx.notify();
            });
        });
        for _ in 0..5 {
            cx.update_window(handle.into(), |_, w, cx| w.draw(cx).clear(cx))
                .unwrap();
        }
        let viewport = visual
            .debug_bounds(if view == "history" {
                "interaction-history-viewport"
            } else {
                "interaction-process-viewport"
            })
            .unwrap();
        let button = visual
            .debug_bounds(if view == "detail" {
                "copy-executable"
            } else {
                "interaction-view-0"
            })
            .unwrap();
        println!("interaction_bounds view={view} viewport={viewport:?} button={button:?}");
        for action in [
            "notify",
            "live_sample",
            "stationary",
            "rows",
            "headers",
            "buttons",
            "scroll",
            "text_drag",
            "select",
        ] {
            if action == "live_sample" && view != "live" {
                continue;
            }
            if action == "select" && view != "live" {
                continue;
            }
            if action == "text_drag" && view != "detail" {
                continue;
            }
            let text = if action == "text_drag" {
                Some(visual.debug_bounds("executable-value").unwrap())
            } else {
                None
            };
            let mut events_ns = 0;
            let scene_before = cx
                .update_window(handle.into(), |_, w, _| w.draw_benchmark_scene().5)
                .unwrap();
            let before_renders = RENDERS.with(|v| v.get());
            let before_table_renders = TABLE_RENDERS.with(|v| v.get());
            let mut samples = if action == "live_sample" {
                (0..n).map(|_| fixed_snapshot()).collect::<Vec<_>>()
            } else {
                Vec::new()
            }
            .into_iter();
            gpui::draw_benchmark_record_draws(true);
            for i in 0..n {
                let position = match action {
                    "text_drag" => {
                        text.unwrap().origin + point(px(4. + (i % 2) as f32 * 40.), px(6.))
                    }
                    "headers" => viewport.origin + point(px(30. + (i % 5) as f32 * 60.), px(14.)),
                    "buttons" if i % 2 == 0 => button.center(),
                    "buttons" => viewport.origin + point(px(60.), px(45.)),
                    "stationary" => viewport.origin + point(px(60.), px(45.)),
                    "scroll" => viewport.origin + point(px(60.), viewport.size.height - px(2.)),
                    _ => viewport.origin + point(px(60.), px(45. + (i % 8) as f32 * 30.)),
                };
                let start = thread_cpu_ns();
                cx.update_window(handle.into(), |_, w, cx| {
                    if action == "live_sample" {
                        monitor.update(cx, |m, cx| m.accept(samples.next().unwrap(), w, cx));
                        return;
                    }
                    if action == "notify" {
                        monitor.update(cx, |_, cx| cx.notify());
                        return;
                    }
                    if action == "text_drag" && i == 0 {
                        w.dispatch_event(
                            MouseDownEvent {
                                position,
                                button: MouseButton::Left,
                                click_count: 1,
                                modifiers: Modifiers::default(),
                                first_mouse: false,
                            }
                            .to_platform_input(),
                            cx,
                        );
                    }
                    w.dispatch_event(
                        MouseMoveEvent {
                            position,
                            pressed_button: (action == "text_drag").then_some(MouseButton::Left),
                            modifiers: Modifiers::default(),
                        }
                        .to_platform_input(),
                        cx,
                    );
                    if action == "scroll" {
                        w.dispatch_event(
                            ScrollWheelEvent {
                                position,
                                delta: ScrollDelta::Pixels(point(px(0.), px(-44.))),
                                ..Default::default()
                            }
                            .to_platform_input(),
                            cx,
                        );
                    }
                    if action == "select" {
                        w.dispatch_event(
                            MouseDownEvent {
                                position,
                                button: MouseButton::Left,
                                click_count: 1,
                                modifiers: Modifiers::default(),
                                first_mouse: false,
                            }
                            .to_platform_input(),
                            cx,
                        );
                        w.dispatch_event(
                            MouseUpEvent {
                                position,
                                button: MouseButton::Left,
                                click_count: 1,
                                modifiers: Modifiers::default(),
                            }
                            .to_platform_input(),
                            cx,
                        );
                    }
                })
                .unwrap();
                events_ns += thread_cpu_ns() - start;
                if action == "text_drag" && i % 2 == 1 {
                    assert!(
                        !cx.update_window(handle.into(), |_, w, cx| {
                            gpui_base::TextSelection::selected_text(w, cx)
                        })
                        .unwrap()
                        .is_empty(),
                        "drag must select text"
                    );
                }
            }
            let timings = gpui::draw_benchmark_take_draws();
            gpui::draw_benchmark_record_draws(false);
            let draws = timings.len();
            let draws_ns: u64 = timings.iter().sum();
            assert!(events_ns >= draws_ns);
            let scene = cx
                .update_window(handle.into(), |_, w, _| w.draw_benchmark_scene().5)
                .unwrap();
            let changed_scenes = usize::from(scene != scene_before);
            if action == "select" {
                assert!(
                    cx.read(|cx| monitor.read(cx).details.is_some()),
                    "row click must open detail"
                );
            }
            if action == "scroll" {
                assert!(
                    changed_scenes > 0,
                    "scroll must change rendered content in {view}"
                );
            }
            let renders = RENDERS.with(|v| v.get()) - before_renders;
            let table_renders = TABLE_RENDERS.with(|v| v.get()) - before_table_renders;
            if action == "live_sample" {
                assert_eq!(
                    table_renders, n,
                    "each accepted sample must render Processes once"
                );
            }
            if matches!(action, "notify" | "live_sample") {
                assert_eq!(draws, n, "each notification must draw once");
                assert_eq!(renders, n, "each notification must render Monitor once");
                assert_eq!(changed_scenes, 0, "unchanged data must preserve the scene");
            }
            if action == "text_drag" {
                cx.update_window(handle.into(), |_, w, cx| {
                    w.dispatch_event(
                        MouseUpEvent {
                            position: text.unwrap().center(),
                            button: MouseButton::Left,
                            click_count: 1,
                            modifiers: Modifiers::default(),
                        }
                        .to_platform_input(),
                        cx,
                    );
                    w.draw(cx).clear(cx);
                })
                .unwrap();
            }
            println!("interaction view={view} action={action} events={n} draws={draws} renders={renders} table_renders={table_renders} changed_scenes={changed_scenes} scene={scene:016x} event_cpu_ms={:.6} draw_cpu_ms={:.6} cpu_ms_per_draw={:.6} errors=0",
                (events_ns - draws_ns) as f64 / 1e6, draws_ns as f64 / 1e6,
                draws_ns as f64 / 1e6 / draws.max(1) as f64);
        }
    }
    FIXED_CLOCK.with(|v| v.set(false));
}

#[cfg(all(target_os = "macos", gtop_draw_benchmark))]
#[test]
#[ignore = "headless warm-cache scene comparison; use draw-benchmark instrumentation"]
fn warm_dot_cache_transitions() {
    FIXED_CLOCK.with(|v| v.set(true));
    let mut cx = TestAppContext::build_with_text_system(
        TestDispatcher::new(0),
        Some("warm_dot_cache_transitions"),
        Arc::new(gpui_apple::AppleTextSystem::new()),
    );
    cx.update(|cx| {
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
    let mut monitor = None;
    let handle = cx.open_window(size(px(1480.), px(960.)), |window, cx| {
        let entity = cx.new(|cx| fixture(window, cx));
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let monitor = monitor.unwrap();
    cx.simulate_window_scale_factor_change(handle.into(), 1.);
    for _ in 0..5 {
        cx.update_window(handle.into(), |_, w, cx| w.draw(cx).clear(cx))
            .unwrap();
    }
    for transition in [
        "led",
        "background",
        "aluminium",
        "amber",
        "resize",
        "scale1.5",
        "scale2",
        "scale1",
    ] {
        match transition {
            "resize" => cx.simulate_window_resize(handle.into(), size(px(1300.), px(880.))),
            "scale1.5" => cx.simulate_window_scale_factor_change(handle.into(), 1.5),
            "scale2" => cx.simulate_window_scale_factor_change(handle.into(), 2.),
            "scale1" => cx.simulate_window_scale_factor_change(handle.into(), 1.),
            _ => cx.update(|cx| {
                monitor.update(cx, |m, cx| {
                    match transition {
                        "led" => {
                            let mut base = m.settings.base();
                            base.led = 0x43d6ff;
                            m.settings.edit_base(base);
                        }
                        "background" => {
                            let mut base = m.settings.base();
                            base.ground = 0x142536;
                            m.settings.edit_base(base);
                        }
                        "aluminium" => m.settings.active = 4,
                        "amber" => m.settings.active = 0,
                        _ => unreachable!(),
                    }
                    let base = m.settings.base();
                    m.table.update(cx, |table, cx| {
                        table.base = base;
                        cx.notify();
                    });
                    theme::apply(base, cx);
                    cx.notify();
                })
            }),
        }
        cx.update_window(handle.into(), |_, w, cx| {
            w.draw(cx).clear(cx);
            let cached = w.draw_benchmark_scene().5;
            rack::clear_dot_views(cx);
            w.refresh();
            w.draw(cx).clear(cx);
            assert_eq!(
                cached,
                w.draw_benchmark_scene().5,
                "warm transition {transition}"
            );
            println!("transition={transition} scene={cached:016x} parity=true");
        })
        .unwrap();
    }
    cx.update_window(handle.into(), |_, w, cx| {
        rack::assert_dot_view_bounds(w, cx)
    })
    .unwrap();
    FIXED_CLOCK.with(|v| v.set(false));
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
                        let mut analysis =
                            analysis::analyze("process-0001", "/bin/process-0001", 30_000_000, []);
                        analysis.history = Some(analysis::ProcessHistory {
                            columns: (0..144).map(|i| Some((i % 60) as f32)).collect(),
                            pattern: "Recurring work".into(),
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
struct GhostLayerReview {
    tokens: palette::Tokens,
    content: GhostLayerContent,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
enum GhostLayerContent {
    Full,
    Dots,
    Unit(char),
}

#[cfg(target_os = "macos")]
impl Render for GhostLayerReview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let content = div()
            .flex()
            .items_start()
            .gap(px(24.))
            .bg(rgb(self.tokens.base.ground));
        match self.content {
            GhostLayerContent::Full => content.child(
                crate::rack::win(&self.tokens)
                    .child(crate::rack::lit("48".into(), 42., false, &self.tokens))
                    .child(crate::rack::lit_unit(
                        "48".into(),
                        '%',
                        42.,
                        false,
                        &self.tokens,
                    )),
            ),
            GhostLayerContent::Dots => {
                content.child(crate::rack::win(&self.tokens).w(px(160.)).h(px(80.)))
            }
            GhostLayerContent::Unit(unit) => content.child(crate::rack::unit_glyph_for_test(
                unit,
                42.,
                false,
                &self.tokens,
            )),
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires a Metal device; set GTOP_GHOST_RENDER_DIR"]
fn headless_ghost_layers_match_palette_mode() {
    let directory = std::path::PathBuf::from(
        std::env::var("GTOP_GHOST_RENDER_DIR").expect("ghost render directory"),
    );
    std::fs::create_dir_all(&directory).unwrap();
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
        include_bytes!("../assets/fonts/JetBrainsMono.ttf").as_slice(),
    ] {
        cx.text_system()
            .add_fonts(vec![std::borrow::Cow::Borrowed(bytes)])
            .unwrap();
    }

    let mut view = None;
    let handle = cx
        .open_window(size(px(600.), px(100.)), |_, cx| {
            let entity = cx.new(|_| GhostLayerReview {
                tokens: palette::presets()[0].base.tokens(),
                content: GhostLayerContent::Full,
            });
            view = Some(entity.clone());
            entity
        })
        .unwrap();
    let view = view.unwrap();
    for (name, base, expect_ghost) in [
        ("aluminium", palette::presets()[4].base, false),
        ("amber", palette::presets()[0].base, true),
    ] {
        for (layer, content) in [
            ("full", GhostLayerContent::Full),
            ("dots", GhostLayerContent::Dots),
            ("percent", GhostLayerContent::Unit('%')),
            ("degree", GhostLayerContent::Unit('°')),
        ] {
            let mut tokens = base.tokens();
            // A chromatic probe separates unit ghosts from antialiased graphite ink.
            if matches!(content, GhostLayerContent::Unit(_)) {
                tokens.base.led = 0xff00ff;
            }
            cx.update(|cx| {
                view.update(cx, |view, _| {
                    view.tokens = tokens;
                    view.content = content;
                })
            });
            if matches!(content, GhostLayerContent::Full) {
                crate::rack::DIGIT_GHOST_TEST_TEXTS.with(|recorded| recorded.borrow_mut().clear());
            }
            cx.update_window(handle.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
            let image = cx.capture_screenshot(handle.into()).unwrap();
            image
                .save(directory.join(format!("{name}-{layer}.png")))
                .unwrap();
            if let GhostLayerContent::Unit(_) = content {
                let ghost = palette::mix(tokens.base.led, tokens.base.ground, 0.18);
                let ghost_rgba = [
                    ((ghost >> 16) & 0xff) as u8,
                    ((ghost >> 8) & 0xff) as u8,
                    (ghost & 0xff) as u8,
                    0xff,
                ];
                let ghost_pixels = image.pixels().filter(|pixel| pixel.0 == ghost_rgba).count();
                println!("{name} {layer} ghost pixel count was {ghost_pixels}");
                assert!(
                    (ghost_pixels > 0) == expect_ghost,
                    "{name} {layer} ghost pixel count matching {ghost_rgba:?} was {ghost_pixels}"
                );
            }

            if matches!(content, GhostLayerContent::Full) {
                let rendered_ghosts = crate::rack::DIGIT_GHOST_TEST_TEXTS
                    .with(|recorded| std::mem::take(&mut *recorded.borrow_mut()));
                assert_eq!(
                    rendered_ghosts.contains(&"88".into()),
                    expect_ghost,
                    "{name} lit digit ghost layers were {rendered_ghosts:?}"
                );
                assert_eq!(
                    rendered_ghosts.contains(&"888".into()),
                    expect_ghost,
                    "{name} lit_unit digit ghost layers were {rendered_ghosts:?}"
                );
            }

            // Text antialiasing can match the dot colour, so inspect an empty readout.
            if matches!(content, GhostLayerContent::Dots) {
                let dots = palette::mix(base.led, tokens.readout, 0.07);
                let dots_rgba = [
                    ((dots >> 16) & 0xff) as u8,
                    ((dots >> 8) & 0xff) as u8,
                    (dots & 0xff) as u8,
                    0xff,
                ];
                let dot_pixels = image.pixels().filter(|pixel| pixel.0 == dots_rgba).count();
                println!("{name} readout dot pixel count was {dot_pixels}");
                assert_eq!(
                    dot_pixels > 0,
                    expect_ghost,
                    "{name} readout dot pixel count was {dot_pixels}"
                );
            }
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "offscreen palette review; set GTOP_PALETTE_RENDER_DIR"]
fn aluminium_offscreen_review() {
    let directory = std::path::PathBuf::from(
        std::env::var("GTOP_PALETTE_RENDER_DIR").expect("render directory"),
    );
    std::fs::create_dir_all(&directory).unwrap();
    let disk_home = directory.join("disk-home");
    std::fs::create_dir_all(&disk_home).unwrap();
    for i in 0..160 {
        std::fs::write(disk_home.join(format!("file-{i:03}")), [1u8; 4096]).unwrap();
    }
    FIXED_CLOCK.with(|v| v.set(true));
    LONG_DETAIL_REVIEW.with(|v| v.set(true));
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
        .open_window(size(px(1480.), px(1200.)), |window, cx| {
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
    for (name, view, detail, kill) in [
        ("live", None, false, false),
        ("history", Some(2), false, false),
        ("detail", None, true, false),
        ("kill-confirmation", None, true, true),
        ("disk", None, false, false),
        ("palette", None, false, false),
        ("picker", None, false, false),
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
                    m.prepare_disk_scroll_test(disk_home.clone());
                }
                if detail {
                    m.select(
                        ProcessKey::from(&m.snapshot.as_ref().unwrap().processes[0]),
                        cx,
                    );
                }
                if kill {
                    let key = ProcessKey::from(&m.snapshot.as_ref().unwrap().processes[0]);
                    m.details.as_mut().unwrap().request_kill(key, true);
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

    for (palette_name, palette_index) in [("amber", 0), ("aluminium", 4)] {
        cx.update(|cx| {
            monitor.update(cx, |m, cx| {
                m.settings.active = palette_index;
                let base = m.settings.base();
                theme::apply(base, cx);
                m.table.update(cx, |table, cx| {
                    table.base = base;
                    cx.notify();
                });
                m.view = None;
                m.settings_open = false;
                m.details = None;
                m.select(
                    ProcessKey::from(&m.snapshot.as_ref().unwrap().processes[0]),
                    cx,
                );
                cx.notify();
            });
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
        cx.capture_screenshot(handle.into())
            .unwrap()
            .save(directory.join(format!("{palette_name}-detail-long.png")))
            .unwrap();
    }
    FIXED_CLOCK.with(|v| v.set(false));
    LONG_DETAIL_REVIEW.with(|v| v.set(false));
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

#[cfg(target_os = "macos")]
#[test]
fn random_draft_color_edits_stay_unsaved_until_save_or_discard() {
    use gpui::{prelude::*, px, size, TestAppContext, TestDispatcher, VisualTestContext};
    use gpui_component::Root;

    let mut cx = TestAppContext::build_with_text_system(
        TestDispatcher::new(0),
        Some("random-draft-edit"),
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
            m.settings.active = 5;
            m.settings.saved.push(palette::Palette {
                name: "Saved mix".into(),
                base: palette::presets()[0].base,
            });
            m.settings_open = true;
            m.draft = Some(palette::presets()[4].base);
            m.settings_writes = Some(sender);
        });
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let monitor = monitor.unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        monitor.update(cx, |m, cx| {
            let picker = m.color_pickers[palette::ColorField::Led as usize].clone();
            picker.update(cx, |picker, cx| {
                picker.update_color(rgb(0x654321).into(), window, cx)
            });
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let m = monitor.read(cx);
        assert_eq!(m.settings.active, 5);
        assert_eq!(m.settings.saved.len(), 1);
        assert_eq!(m.settings.saved[0].base, palette::presets()[0].base);
        assert_eq!(m.draft.unwrap().led, 0x654321);
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle.into(), &cx);
    let discard = visual.debug_bounds("discard-palette").unwrap();
    visual.simulate_click(discard.center(), Modifiers::default());
    cx.update(|cx| {
        let m = monitor.read(cx);
        assert!(m.draft.is_none());
        assert_eq!(m.settings.active, 5);
        assert_eq!(m.settings.base(), palette::presets()[0].base);
    });
    let restored = futures::executor::block_on(palette::next_settings(&mut receiver)).unwrap();
    assert_eq!(restored.base(), palette::presets()[0].base);
    assert_eq!(restored.saved.len(), 1);
}

#[cfg(target_os = "macos")]
#[test]
fn saving_an_edited_random_draft_creates_one_saved_copy() {
    use gpui::{prelude::*, px, size, TestAppContext, TestDispatcher, VisualTestContext};
    use gpui_component::Root;

    let mut cx = TestAppContext::build_with_text_system(
        TestDispatcher::new(0),
        Some("save-edited-random-draft"),
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
            m.settings.active = 5;
            m.settings.saved.push(palette::Palette {
                name: "Saved mix".into(),
                base: palette::presets()[0].base,
            });
            m.settings_open = true;
            m.draft = Some(palette::presets()[4].base);
            m.settings_writes = Some(sender);
        });
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let monitor = monitor.unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        monitor.update(cx, |m, cx| {
            m.color_pickers[palette::ColorField::Led as usize].update(cx, |picker, cx| {
                picker.update_color(rgb(0x654321).into(), window, cx)
            });
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle.into(), &cx);
    let save = visual.debug_bounds("save-palette").unwrap();
    visual.simulate_click(save.center(), Modifiers::default());
    cx.update(|cx| {
        let m = monitor.read(cx);
        assert!(m.draft.is_none());
        assert_eq!(m.settings.active, 6);
        assert_eq!(m.settings.saved.len(), 2);
        assert_eq!(m.settings.base().led, 0x654321);
        assert_eq!(m.settings.saved[0].base, palette::presets()[0].base);
    });
    let saved = futures::executor::block_on(palette::next_settings(&mut receiver)).unwrap();
    assert_eq!(saved.active, 6);
    assert_eq!(saved.saved.len(), 2);
    assert_eq!(saved.base().led, 0x654321);
}

#[cfg(target_os = "macos")]
#[test]
fn entering_the_untouched_picker_hex_keeps_aluminium_rgb() {
    use gpui::{prelude::*, px, size, TestAppContext, TestDispatcher};
    use gpui_component::Root;

    let mut cx = TestAppContext::build_with_text_system(
        TestDispatcher::new(0),
        Some("untouched-picker-hex"),
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
        entity.update(cx, |m, cx| {
            m.settings.active = 4;
            m.settings_open = true;
            m.settings_writes = Some(sender);
            let base = m.settings.base();
            m.color_pickers[palette::ColorField::Led as usize].update(cx, |picker, cx| {
                picker.set_value(rgb(base.led), window, cx);
                picker.set_open(true, cx);
            });
        });
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let monitor = monitor.unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
        let picker = monitor.read(cx).color_pickers[palette::ColorField::Led as usize].clone();
        window.focus(&picker.read(cx).hex_input().read(cx).focus_handle(cx), cx);
    })
    .unwrap();
    cx.simulate_keystrokes(handle.into(), "enter");
    cx.run_until_parked();
    cx.update(|cx| {
        let m = monitor.read(cx);
        assert_eq!(m.settings.base().led, 0x2b2f33);
        assert!(m.settings.saved.is_empty());
        assert_eq!(m.settings.active, 4);
    });
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn deleting_a_saved_palette_while_amber_is_active_queues_settings() {
    use gpui::{prelude::*, px, size, TestAppContext, TestDispatcher, VisualTestContext};
    use gpui_component::Root;

    let mut cx = TestAppContext::build_with_text_system(
        TestDispatcher::new(0),
        Some("delete-saved-palette"),
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
            m.settings.saved.push(palette::Palette {
                name: "Saved mix".into(),
                base: palette::presets()[1].base,
            });
            m.settings.active = 0;
            m.settings_open = true;
            m.settings_writes = Some(sender);
        });
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let monitor = monitor.unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle.into(), &cx);
    let delete = visual.debug_bounds("delete-palette-5").unwrap();
    visual.simulate_click(delete.center(), Modifiers::default());
    cx.update(|cx| {
        let m = monitor.read(cx);
        assert_eq!(m.settings.active, 0);
        assert!(m.settings.saved.is_empty());
    });
    let queued = futures::executor::block_on(palette::next_settings(&mut receiver)).unwrap();
    assert_eq!(queued.active, 0);
    assert!(queued.saved.is_empty());
}

#[cfg(target_os = "macos")]
#[gpui::test]
async fn history_headers_sort_rows_and_history_clicks_follow_process_liveness(
    cx: &mut TestAppContext,
) {
    use gpui::{point, px, Modifiers, MouseButton};

    cx.update(|cx| {
        gpui_component::init(cx);
        theme::init(cx);
    });
    let (monitor, visual) = cx.add_window_view(fixture);
    visual.simulate_resize(gpui::size(px(1480.), px(960.)));
    monitor.update(visual, |monitor, cx| {
        monitor.view = Some(0);
        monitor.history[0].rows.push((
            "exited-process".into(),
            history::ProcessTotal {
                cpu_seconds: 1.,
                ..Default::default()
            },
        ));
        cx.notify();
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let name_header = visual.debug_bounds("history-header-0").unwrap();
    let initial_sort = history::HistorySort::default();
    let handle = point(name_header.right() - px(1.), name_header.center().y);
    visual.simulate_mouse_down(handle, MouseButton::Left, Modifiers::default());
    let moved = point(handle.x + px(24.), handle.y);
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        monitor.read_with(visual, |monitor, _| monitor.history_sort),
        initial_sort
    );

    let name_header = visual.debug_bounds("history-header-0").unwrap();
    visual.simulate_click(name_header.center(), Modifiers::default());
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        monitor.read_with(visual, |monitor, _| monitor.history_sort),
        history::HistorySort {
            column: history::HistorySortColumn::Name,
            direction: history::SortDirection::Ascending,
        }
    );
    assert_eq!(
        monitor.read_with(visual, |monitor, _| monitor.history_selected.clone()),
        Some("process-0001".into())
    );

    let exited_row = visual.debug_bounds("history-row-0").unwrap();
    visual.simulate_click(exited_row.center(), Modifiers::default());
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        monitor.read_with(visual, |monitor, _| monitor.history_selected.clone()),
        Some("exited-process".into())
    );
    assert!(monitor.read_with(visual, |monitor, _| monitor.details.is_none()));

    let core_time_header = visual.debug_bounds("history-header-1").unwrap();
    visual.simulate_click(core_time_header.center(), Modifiers::default());
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        monitor.read_with(visual, |monitor, _| monitor.history_selected.clone()),
        Some("exited-process".into())
    );
    let running_row = visual.debug_bounds("history-row-0").unwrap();
    visual.simulate_click(running_row.center(), Modifiers::default());
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(monitor.read_with(visual, |monitor, _| monitor.details.is_some()));
    assert_eq!(
        monitor.read_with(visual, |monitor, _| monitor.history_selected.clone()),
        Some("process-0001".into())
    );
}

#[cfg(target_os = "macos")]
#[gpui::test]
async fn history_enter_opens_running_selection_and_keeps_exited_selection(
    _cx: &mut TestAppContext,
) {
    use gpui::{px, size, Modifiers, TestAppContext, TestDispatcher, VisualTestContext};
    use gpui_component::Root;

    let mut cx = TestAppContext::build_with_text_system(
        TestDispatcher::new(0),
        Some("history-enter-selection"),
        Arc::new(gpui_apple::AppleTextSystem::new()),
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        theme::init(cx);
    });
    let mut monitor = None;
    let handle = cx.open_window(size(px(1480.), px(960.)), |window, cx| {
        let entity = cx.new(|cx| fixture(window, cx));
        entity.update(cx, |monitor, cx| {
            monitor.view = Some(0);
            monitor.history[0].rows.push((
                "exited-process".into(),
                history::ProcessTotal {
                    cpu_seconds: 1.,
                    ..Default::default()
                },
            ));
            window.focus(&monitor.focus, cx);
        });
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let monitor = monitor.unwrap();
    let mut visual = VisualTestContext::from_window(handle.into(), &cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    })
    .unwrap();

    let name_header = visual.debug_bounds("history-header-0").unwrap();
    visual.simulate_click(name_header.center(), Modifiers::default());
    cx.simulate_keystrokes(handle.into(), "j");
    cx.simulate_keystrokes(handle.into(), "enter");
    cx.update_window(handle.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.update(|cx| {
        let monitor = monitor.read(cx);
        assert_eq!(monitor.history_selected.as_deref(), Some("process-0002"));
        assert_eq!(
            monitor.details.as_ref().map(|details| details.key),
            Some(ProcessKey::from(
                &monitor.snapshot.as_ref().unwrap().processes[1]
            ))
        );
        assert!(monitor.moment.is_none());
    });

    monitor.update(&mut visual, |monitor, cx| {
        monitor.details = None;
        monitor.history_selected = Some("exited-process".into());
        cx.notify();
    });
    cx.simulate_keystrokes(handle.into(), "enter");
    cx.update_window(handle.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.update(|cx| {
        let monitor = monitor.read(cx);
        assert_eq!(monitor.history_selected.as_deref(), Some("exited-process"));
        assert!(monitor.details.is_none());
        assert!(monitor.moment.is_none());
    });
}

#[cfg(target_os = "macos")]
#[gpui::test]
async fn tower_title_stays_clear_of_the_upper_left_screw(cx: &mut TestAppContext) {
    use gpui::{prelude::*, px, size, VisualTestContext};

    struct TowerHeader(palette::Tokens);
    impl Render for TowerHeader {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .w(px(300.))
                .h(px(40.))
                .child(rack::screws(&self.0, "tower"))
                .child(rack::tower_title("SYSTEM MONITOR", &self.0))
        }
    }

    for index in 0..palette::presets().len() {
        let handle = cx.open_window(size(px(320.), px(80.)), |_, _| {
            TowerHeader(palette::presets()[index].base.tokens())
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
        let mut visual = VisualTestContext::from_window(handle.into(), cx);
        let title = visual.debug_bounds("tower-title").unwrap();
        let screw = visual.debug_bounds("tower-screw-top-left").unwrap();
        assert!(
            title.origin.x >= screw.origin.x + screw.size.width,
            "preset {index}: title {title:?} overlaps screw {screw:?}"
        );
    }
}

#[cfg(target_os = "macos")]
#[gpui::test]
async fn detail_focus_returns_after_escape(cx: &mut TestAppContext) {
    detail_focus_transition(cx, "escape");
}

#[cfg(target_os = "macos")]
#[gpui::test]
async fn detail_focus_returns_after_navigation(cx: &mut TestAppContext) {
    detail_focus_transition(cx, "j");
}

#[cfg(target_os = "macos")]
#[gpui::test]
async fn detail_focus_returns_after_settings(cx: &mut TestAppContext) {
    detail_focus_transition(cx, "s");
}

#[cfg(target_os = "macos")]
#[gpui::test]
async fn detail_close_button_restores_monitor_focus_after_text_selection(cx: &mut TestAppContext) {
    detail_button_focus_transition(cx, "close-details");
}

#[cfg(target_os = "macos")]
#[gpui::test]
async fn detail_kill_confirmation_restores_monitor_focus_after_text_selection(
    cx: &mut TestAppContext,
) {
    detail_button_focus_transition(cx, "confirm-kill");
}

#[cfg(target_os = "macos")]
#[gpui::test]
async fn detail_kill_completion_restores_focus_and_reaps_owned_child(cx: &mut TestAppContext) {
    use gpui::{point, px, size, Modifiers, MouseButton};

    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

    let mut child = ChildGuard(
        std::process::Command::new("/bin/sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    );
    let core = Arc::new(gtop_core::AppState::new());
    let snapshot = core.snapshot(false, false).unwrap();
    let process = snapshot
        .processes
        .iter()
        .find(|process| process.pid == child.0.id())
        .unwrap()
        .clone();
    let key = ProcessKey::from(&process);

    cx.update(|cx| {
        gpui_component::init(cx);
        theme::init(cx);
    });
    let mut monitor = None;
    let (root, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let mut monitor = fixture(window, cx);
            monitor.core = Some(core.clone());
            monitor.snapshot = Some(Rc::new(snapshot));
            monitor.select(key, cx);
            monitor
        });
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let _root = root;
    let monitor = monitor.unwrap();
    visual.simulate_resize(size(px(1480.), px(960.)));
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let bounds = visual.debug_bounds("executable-value").unwrap();
    let start = bounds.origin + point(px(1.), px(5.));
    let end = bounds.origin + point(px(46.), px(5.));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| {
        assert!(!gpui_base::TextSelection::selected_text(window, cx).is_empty());
    });

    let kill = visual.debug_bounds("kill").unwrap();
    visual.simulate_click(kill.center(), Modifiers::default());
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let confirm = visual.debug_bounds("confirm-kill").unwrap();
    visual.simulate_click(confirm.center(), Modifiers::default());
    visual.run_until_parked();

    visual.update(|window, cx| {
        assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
        assert_eq!(
            monitor.read(cx).pending_kill,
            Some(key),
            "kill status: {}",
            monitor.read(cx).status
        );
        assert!(monitor.read(cx).status.starts_with("Kill signal sent"));
    });
    assert!(!child.0.wait().unwrap().success());
}

#[cfg(target_os = "macos")]
#[gpui::test]
async fn detail_kill_completion_reports_exited_child_and_restores_focus(cx: &mut TestAppContext) {
    use gpui::{px, size, Modifiers};

    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

    let mut child = ChildGuard(
        std::process::Command::new("/bin/sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    );
    let core = Arc::new(gtop_core::AppState::new());
    let snapshot = core.snapshot(false, false).unwrap();
    let process = snapshot
        .processes
        .iter()
        .find(|process| process.pid == child.0.id())
        .unwrap()
        .clone();
    let key = ProcessKey::from(&process);

    cx.update(|cx| {
        gpui_component::init(cx);
        theme::init(cx);
    });
    let mut monitor = None;
    let (root, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let mut monitor = fixture(window, cx);
            monitor.core = Some(core.clone());
            monitor.snapshot = Some(Rc::new(snapshot));
            monitor.select(key, cx);
            monitor
        });
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let _root = root;
    let monitor = monitor.unwrap();
    visual.simulate_resize(size(px(1480.), px(960.)));
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let kill = visual.debug_bounds("kill").unwrap();
    visual.simulate_click(kill.center(), Modifiers::default());
    visual.update(|window, cx| window.draw(cx).clear(cx));

    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());

    let confirm = visual.debug_bounds("confirm-kill").unwrap();
    visual.simulate_click(confirm.center(), Modifiers::default());
    visual.run_until_parked();

    visual.update(|window, cx| {
        let monitor = monitor.read(cx);
        assert_eq!(window.focused(cx), Some(monitor.focus.clone()));
        assert_eq!(monitor.pending_kill, None);
        assert!(matches!(
            monitor.details.as_ref().unwrap().kill,
            details::KillState::Ready
        ));
        assert_eq!(
            monitor.status,
            format!(
                "Could not kill PID {}. It exited, changed, or permission was denied.",
                child.0.id()
            )
        );
    });
}

#[cfg(target_os = "macos")]
#[gpui::test]
async fn detail_copy_state_releases_after_close(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        theme::init(cx);
    });
    let mut monitor = None;
    let (root, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let mut monitor = fixture(window, cx);
            let key = ProcessKey::from(&monitor.snapshot.as_ref().unwrap().processes[0]);
            monitor.select(key, cx);
            monitor
        });
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let _root = root;
    let monitor = monitor.unwrap();
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let participant_alive = monitor.read_with(visual, |monitor, _| {
        monitor
            .details
            .as_ref()
            .unwrap()
            .copy_selection
            .participant_alive_check(details::DetailTextSection::Executable)
    });

    monitor.update(visual, |monitor, cx| {
        monitor.details = None;
        cx.notify();
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    assert!(!participant_alive());
}

#[cfg(target_os = "macos")]
fn detail_button_focus_transition(cx: &mut TestAppContext, button: &str) {
    use gpui::{px, size, MouseButton};

    cx.update(|cx| {
        gpui_component::init(cx);
        theme::init(cx);
    });
    let mut monitor = None;
    let (root, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let mut monitor = fixture(window, cx);
            let key = ProcessKey::from(&monitor.snapshot.as_ref().unwrap().processes[0]);
            monitor.select(key, cx);
            monitor
        });
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let _root = root;
    let monitor = monitor.unwrap();
    visual.simulate_resize(size(px(1480.), px(960.)));
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let bounds = visual.debug_bounds("executable-value").unwrap();
    let start = bounds.origin + point(px(1.), px(5.));
    let end = bounds.origin + point(px(46.), px(5.));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| {
        assert!(!gpui_base::TextSelection::selected_text(window, cx).is_empty());
    });

    if button == "confirm-kill" {
        let kill = visual.debug_bounds("kill").unwrap();
        visual.simulate_click(kill.center(), Modifiers::default());
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let confirm = visual.debug_bounds("confirm-kill").unwrap();
        visual.simulate_click(confirm.center(), Modifiers::default());
    } else {
        let close = visual.debug_bounds("close-details").unwrap();
        visual.simulate_click(close.center(), Modifiers::default());
    }
    visual.run_until_parked();
    visual.update(|window, cx| {
        assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
    });
    if button == "close-details" {
        assert!(monitor.read_with(visual, |monitor, _| monitor.details.is_none()));
    }
}

#[cfg(target_os = "macos")]
fn detail_focus_transition(cx: &mut TestAppContext, key: &str) {
    cx.update(|cx| {
        gpui_component::init(cx);
        theme::init(cx);
    });
    let mut monitor = None;
    let (root, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| {
            let mut monitor = fixture(window, cx);
            let key = ProcessKey::from(&monitor.snapshot.as_ref().unwrap().processes[0]);
            monitor.select(key, cx);
            monitor
        });
        monitor = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let _root = root;
    let monitor = monitor.unwrap();
    visual.simulate_resize(size(px(1480.), px(960.)));
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let bounds = visual.debug_bounds("executable-value").unwrap();
    let start = bounds.origin + point(px(1.), px(5.));
    let end = bounds.origin + point(px(46.), px(5.));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| {
        assert!(window.focused(cx).is_some());
        assert_ne!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
        assert!(!gpui_base::TextSelection::selected_text(window, cx).is_empty());
    });
    let before = monitor.read_with(visual, |monitor, cx| monitor.table.read(cx).selected);
    visual.simulate_keystrokes(key);
    visual.run_until_parked();
    visual.update(|window, cx| {
        assert_eq!(
            window.focused(cx),
            Some(monitor.read(cx).focus.clone()),
            "focus after {key}"
        );
    });
    match key {
        "escape" => {
            assert!(monitor.read_with(visual, |monitor, _| monitor.details.is_none()));
            visual.simulate_keystrokes("j");
            assert_ne!(
                monitor.read_with(visual, |monitor, cx| monitor.table.read(cx).selected),
                before
            );
            visual.simulate_keystrokes("/");
            visual.update(|window, cx| {
                assert_eq!(
                    window.focused(cx),
                    Some(monitor.read(cx).search.read(cx).focus_handle(cx))
                )
            });
        }
        "j" => {
            let first = monitor.read_with(visual, |monitor, cx| monitor.table.read(cx).selected);
            assert_ne!(first, before);
            visual.simulate_keystrokes("j");
            visual.run_until_parked();
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
            assert_ne!(
                monitor.read_with(visual, |monitor, cx| monitor.table.read(cx).selected),
                first
            );
        }
        "s" => {
            assert!(monitor.read_with(visual, |monitor, _| monitor.settings_open));
            visual.simulate_keystrokes("escape");
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
            assert!(!monitor.read_with(visual, |monitor, _| monitor.settings_open));
        }
        _ => unreachable!(),
    }
}
