mod analysis;
mod details;
#[cfg(test)]
mod draw_benchmark;
mod history;
mod model;
mod palette;
mod process_table;
mod rack;
#[cfg(test)]
mod test_data;
mod theme;

use details::Details;
use futures::{channel::mpsc, FutureExt, StreamExt};
use gpui::{prelude::*, *};
use gpui_component::{
    input::{InputEvent, InputState},
    Root,
};
use gtop_core::{AppState, Snapshot};
use model::{ProcessKey, Search};
use process_table::Processes;
use std::{rc::Rc, sync::Arc, time::Duration};

fn clamp_moment_row(selected: usize, row_count: usize) -> usize {
    selected.min(row_count.saturating_sub(1))
}

#[derive(Clone, Copy, Default)]
struct DrawMetrics {
    mean_ms: Option<f64>,
    fps: Option<f64>,
}

impl DrawMetrics {
    fn record(&mut self, draw_count: u64, elapsed_seconds: f64, sampled_mean_ms: Option<f64>) {
        if let Some(mean_ms) = sampled_mean_ms {
            self.mean_ms = Some(mean_ms);
        }
        if self.mean_ms.is_some() {
            self.fps = Some(draw_count as f64 / elapsed_seconds);
        }
    }
}

struct Monitor {
    core: Option<Arc<AppState>>,
    snapshot: Option<Rc<Snapshot>>,
    table: Entity<Processes>,
    search: Entity<InputState>,
    details: Option<Details>,
    paused: bool,
    history: [history::HistoryView; 3],
    moment: Option<history::MomentSelection>,
    moment_detail: Option<history::MomentDetail>,
    moment_selected: usize,
    moment_rows: Vec<(String, history::ProcessTotal, f64)>,
    view: Option<usize>,
    history_selected: Option<String>,
    history_scroll: UniformListScrollHandle,
    metric: history::Metric,
    settings: palette::Settings,
    settings_writes: Option<mpsc::UnboundedSender<palette::Settings>>,
    settings_writer: Option<Task<()>>,
    settings_open: bool,
    tick_ms: f64,
    draw_metrics: DrawMetrics,
    sample_seconds: f64,
    last_snapshot: Option<f64>,
    focus: FocusHandle,
    draft: Option<palette::Base>,
    colors: [Entity<InputState>; 6],
    mix_name: Entity<InputState>,
    traces: std::collections::HashMap<ProcessKey, std::collections::VecDeque<(f64, f32)>>,
    interval: u64,
    status: String,
    pending_kill: Option<ProcessKey>,
    wake: mpsc::Sender<()>,
    _subscriptions: Vec<Subscription>,
    _refresh: Task<()>,
    _ports: Task<()>,
    _frames: Task<()>,
}

impl Monitor {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = palette::Settings::load();
        theme::apply(settings.base(), cx);
        let base = settings.base();
        let colors = [
            base.ground,
            base.plate,
            base.silk,
            base.led,
            base.hot,
            base.ok,
        ]
        .map(|v| cx.new(|cx| InputState::new(window, cx).default_value(format!("#{v:06x}"))));
        let mix_name = cx.new(|cx| InputState::new(window, cx).placeholder("Name this mix"));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("name, pid, command or port"));
        let table = cx.new(|_| {
            let mut p = Processes::default();
            p.root_focus = Some(focus.clone());
            p.base = settings.base();
            p
        });
        let (wake, mut events) = mpsc::channel(1);
        let (settings_writes, mut settings_events) = mpsc::unbounded();
        let settings_writer = cx.spawn_in(window, async move |entity, cx| {
            while let Some(settings) = palette::next_settings(&mut settings_events).await {
                let result = cx
                    .background_executor()
                    .spawn(async move { settings.save() })
                    .await;
                if let Err(error) = result {
                    let _ = entity.update_in(cx, |this, _, cx| {
                        this.status = format!("Palette could not be saved: {error}");
                        cx.notify();
                    });
                }
            }
        });
        let mut subscriptions = vec![
            cx.subscribe_in(&search, window, |this, input, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let query = input.read(cx).value();
                    let mut changed_ports = false;
                    this.table.update(cx, |table, cx| {
                        let rows = &mut *table;
                        let previous = rows.search.needs_ports();
                        rows.search = Search::new(&query);
                        changed_ports = previous != rows.search.needs_ports();
                        rows.reorder();
                        if let Some(row) = rows.selected.and_then(|key| rows.row_for(key)) {
                            table.selected = table.process(row).map(ProcessKey::from);
                        } else {
                            table.selected = None;
                        }
                        cx.notify();
                    });
                    if changed_ports {
                        this.wake();
                    }
                    cx.notify();
                }
            }),
            cx.subscribe_in(&table, window, |this, _, event, window, cx| {
                window.focus(&this.focus, cx);
                let process_table::SelectProcess(key) = event;
                this.settings_open = false;
                this.select(*key, cx);
            }),
            cx.observe_window_visibility(window, |this, visibility, _, _| {
                if gtop_core::perf_enabled() {
                    eprintln!("perf visibility visible={}", visibility.is_visible());
                }
                this.wake();
            }),
        ];
        for (index, input) in colors.iter().enumerate() {
            subscriptions.push(
                cx.subscribe_in(input, window, move |this, input, event, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        let text = input.read(cx).value();
                        if let Some(v) = text
                            .strip_prefix('#')
                            .filter(|s| s.len() == 6)
                            .and_then(|s| u32::from_str_radix(s, 16).ok())
                        {
                            let mut b = this.draft.unwrap_or(this.settings.base());
                            if [b.ground, b.plate, b.silk, b.led, b.hot, b.ok][index] == v {
                                return;
                            }
                            match index {
                                0 => b.ground = v,
                                1 => b.plate = v,
                                2 => b.silk = v,
                                3 => b.led = v,
                                4 => b.hot = v,
                                _ => b.ok = v,
                            };
                            this.draft = Some(b);
                            this.apply_palette(cx);
                        }
                    }
                }),
            );
        }
        let cached_ports = Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            u32,
            Vec<u16>,
        >::new()));
        let port_cache = cached_ports.clone();
        let ports_task = cx.spawn_in(window, async move |entity, cx| loop {
            let Ok(running) =
                entity.update_in(cx, |this, window, _| !this.paused && window.is_visible())
            else {
                break;
            };
            if running {
                let cache = port_cache.clone();
                cx.background_executor()
                    .spawn(async move {
                        let ports = gtop_core::monitoring::collect_ports_by_pid();
                        if let Ok(mut cached) = cache.lock() {
                            *cached = ports;
                        }
                    })
                    .await;
            }
            cx.background_executor()
                .timer(Duration::from_secs(10))
                .await;
        });
        let refresh = cx.spawn_in(window, async move |entity, cx| {
            let core = cx.background_executor().spawn(async { Arc::new(AppState::new()) }).await;
            if entity.update_in(cx, |this, _, _| this.core = Some(core.clone())).is_err() { return; }
            let mut history = cx.background_executor().spawn(async { history::History::load(&history::support_path("history.json"), rack::now() as u64) }).await;
            let mut cached_minute = None;
            let mut cached_moment = None;
            let mut cached_metric = None;
            let mut cached_analysis_key = None;
            loop {
                while events.try_recv().is_ok() {}
                let Ok((running, ports, seconds, moment, metric, selected_detail)) = entity.update_in(cx, |this, window, cx| {
                    let selected_detail = this.details.as_ref().map(|details| {
                        let current_cpu = this.snapshot.as_ref().map(|snapshot| crate::analysis::current_cpu(&snapshot.processes, &details.name)).unwrap_or(0.);
                        (
                            details.key,
                            details.name.clone(),
                            details.executable.clone(),
                            current_cpu,
                            details.analysis.is_none(),
                        )
                    });
                    (!this.paused, window.is_visible() && this.table.read(cx).search.needs_ports(), this.interval, this.moment, this.metric, selected_detail)
                }) else { break; };
                if moment.is_none() {
                    cached_moment = None;
                    cached_metric = None;
                }
                if !running {
                    history.suspend();
                    if let Some(selection) = moment {
                        if cached_moment != Some(selection) || cached_metric != Some(metric) {
                            let detail = history.moment(selection);
                            let _ = entity.update_in(cx, |this, _, cx| {
                                if this.moment == Some(selection) {
                                    this.replace_moment_rows(detail.ranked(metric));
                                    this.moment_detail = Some(detail);
                                    cx.notify();
                                }
                            });
                            cached_moment = Some(selection);
                            cached_metric = Some(metric);
                        }
                    }
                    let pending = entity.update_in(cx, |this, _, _| this.pending_kill).ok().flatten();
                    if let Some(key) = pending {
                        let state = core.clone();
                        let stopped = cx.background_executor().spawn(async move {
                            state.process_stopped(key.pid, key.start_time)
                        }).await;
                        let _ = entity.update_in(cx, |this, _, cx| {
                            if this.pending_kill == Some(key) && matches!(stopped, Ok(true)) {
                                this.pending_kill = None;
                                this.status = format!("PID {} stopped.", key.pid);
                                cx.notify();
                            }
                        });
                    }
                    if let Some((key, name, command, cpu, needs_analysis)) = selected_detail.as_ref() {
                        if *needs_analysis || cached_analysis_key != Some(*key) {
                            let analysis = history.analyze_process(
                                name,
                                command,
                                *cpu,
                                rack::now() as u64 / 60,
                            );
                            let _ = entity.update_in(cx, |this, _, cx| {
                                if let Some(details) = this.details.as_mut().filter(|details| details.key == *key) {
                                    details.analysis = Some(analysis);
                                    cx.notify();
                                }
                            });
                        }
                        cached_analysis_key = Some(*key);
                    } else {
                        cached_analysis_key = None;
                    }
                    if pending.is_some() {
                        let timer = cx.background_executor().timer(Duration::from_secs(seconds)).fuse();
                        let event = events.next().fuse();
                        futures::pin_mut!(timer, event);
                        futures::select! { _ = timer => {}, event = event => { if event.is_none() { break; } } }
                    } else if events.next().await.is_none() { break; }
                    continue;
                }
                let state = core.clone();
                let port_cache=cached_ports.clone();
                let previous_moment = cached_moment;
                let previous_metric = cached_metric;
                let previous_analysis_key = cached_analysis_key;
                let result = cx.background_executor().spawn(async move {
                    let start = std::time::Instant::now();
                    let snapshot = if !ports {
                        match port_cache.lock() {
                            Ok(cached) => state.snapshot_with_cached_ports(false, &cached),
                            Err(_) => state.snapshot(false, false),
                        }
                    } else {
                        state.snapshot(false, true)
                    };
                    let tick_ms = start.elapsed().as_secs_f64() * 1000.;
                    let mut views = None;
                    let mut moment_detail = None;
                    let mut process_analysis = None;
                    let mut history_save = None;
                    if let Ok(ref snapshot) = snapshot {
                        let now = rack::now();

                        history.record(snapshot, now, seconds as f64);
                        let minute = now as u64 / 60;
                        let minute_changed = cached_minute != Some(minute);
                        if let Some((key, name, command, _fallback_cpu, needs_analysis)) = selected_detail.as_ref() {
                            if *needs_analysis || previous_analysis_key != Some(*key) || minute_changed {
                                let current_cpu = crate::analysis::current_cpu(&snapshot.processes, name);
                                process_analysis = Some((
                                    *key,
                                    history.analyze_process(name, command, current_cpu, minute),
                                ));
                            }
                        }
                        if let Some(selection) = moment {
                            let span_changed = selection.start_minute < history.touched.end
                                && history.touched.start < selection.start_minute + selection.step as u64;
                            if previous_moment != Some(selection) || previous_metric != Some(metric) || span_changed {
                                moment_detail = Some(history.moment(selection));
                            }
                        }
                        if minute_changed {
                            cached_minute = Some(minute);
                            views = Some([1,8,24].map(|hours| history.view(hours, now as u64)));
                            history_save=Some(history.save(&history::support_path("history.json")).map_err(|e|e.to_string()));
                        }
                    }
                    let selected_key = match selected_detail {
                        Some((key, _, _, _, _)) if snapshot.is_ok() || previous_analysis_key == Some(key) => Some(key),
                        _ => None,
                    };
                    (snapshot, tick_ms, views, moment_detail, process_analysis, history, cached_minute, history_save, moment, metric, selected_key)
                }).await;
                let (result, tick_ms, views, moment_detail, process_analysis, returned_history, returned_minute, history_save, selection, metric, selected_key) = result;
                cached_minute=returned_minute;
                cached_analysis_key = selected_key;
                history = returned_history;
                if moment_detail.is_some() {
                    cached_moment = selection;
                    cached_metric = selection.map(|_| metric);
                }
                if entity.update_in(cx, |this, window, cx| {
                    if let Some(views) = views {
                        let minute = rack::now() as u64 / 60;
                        let oldest = minute.saturating_sub(1439);
                        if this.moment.is_some_and(|selection| !history::selection_in_window(selection, oldest, minute + 1)) {
                            this.moment = None;
                            this.moment_detail = None;
                            this.moment_rows.clear();
                        }
                        this.history = views;
                    }
                    if let Some(detail) = moment_detail.filter(|_| this.moment == selection) {
                        this.replace_moment_rows(detail.ranked(metric));
                        this.moment_detail = Some(detail);
                    }
                    if let Some((key, analysis)) = process_analysis {
                        if let Some(details) = this.details.as_mut().filter(|details| details.key == key) {
                            details.analysis = Some(analysis);
                        }
                    }
                    if let Some(save)=history_save {
                        if let Err(error)=save {this.status=format!("History could not be saved: {error}");}
                        else if this.status.starts_with("History could not be saved") {this.status="Select a process for details".into();}
                    }
                    match result {
                        Ok(snapshot) => {
                            if !this.paused {
                                this.observe_sample(&snapshot);
                                if window.is_visible() {
                                    this.tick_ms = tick_ms;
                                    this.accept(snapshot, window, cx);
                                }
                            }
                        }
                        Err(error) => { this.status = format!("Refresh failed: {error}"); cx.notify(); }
                    }
                }).is_err() { break; }
                let timer = cx.background_executor().timer(Duration::from_secs(seconds)).fuse();
                let event = events.next().fuse();
                futures::pin_mut!(timer, event);
                futures::select! { _ = timer => {}, event = event => { if event.is_none() { break; } } }
            }
        });
        let frames = cx.spawn_in(window, async move |entity, cx| {
            let mut previous = None;
            let mut sampled_at = std::time::Instant::now();
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let now = std::time::Instant::now();
                let elapsed = now.duration_since(sampled_at).as_secs_f64();
                sampled_at = now;
                if entity
                    .update_in(cx, |this, window, _| {
                        let snapshot = window.frame_duration_snapshot();
                        let mut draws = snapshot.draw_duration_histogram.clone();
                        if let Some(baseline) = previous.as_ref() {
                            if draws.subtract(baseline).is_err() {
                                previous = Some(snapshot.draw_duration_histogram);
                                return;
                            }
                        }
                        let draw_ms = (!draws.is_empty()).then(|| draws.mean() / 1_000_000.);
                        this.draw_metrics.record(draws.len(), elapsed, draw_ms);
                        if gtop_core::perf_enabled() {
                            eprintln!(
                                "perf frames draws={} seconds={elapsed:.6} draw_mean_ms={} fps={}",
                                draws.len(),
                                draw_ms
                                    .map(|v| format!("{v:.6}"))
                                    .unwrap_or_else(|| "unavailable".into()),
                                this.draw_metrics
                                    .fps
                                    .map(|fps| format!("{fps:.3}"))
                                    .unwrap_or_else(|| "unavailable".into())
                            );
                        }
                        previous = Some(snapshot.draw_duration_histogram);
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            core: None,
            snapshot: None,
            table,
            search,
            details: None,
            paused: false,
            history: std::array::from_fn(|_| history::HistoryView::default()),
            moment: None,
            moment_detail: None,
            moment_selected: 0,
            moment_rows: Vec::new(),
            view: None,
            history_selected: None,
            history_scroll: UniformListScrollHandle::new(),
            metric: history::Metric::Cpu,
            settings,
            settings_writes: Some(settings_writes),
            settings_writer: Some(settings_writer),
            settings_open: false,
            tick_ms: 0.,
            draw_metrics: DrawMetrics::default(),
            sample_seconds: 3.,
            last_snapshot: None,
            focus,
            draft: None,
            colors,
            mix_name,
            traces: std::collections::HashMap::new(),
            interval: 3,
            status: "Reading system information…".into(),
            pending_kill: None,
            wake,
            _subscriptions: subscriptions,
            _refresh: refresh,
            _frames: frames,
            _ports: ports_task,
        }
    }

    fn wake(&mut self) {
        let _ = self.wake.try_send(());
    }

    fn select_moment(&mut self, selection: history::MomentSelection, cx: &mut Context<Self>) {
        if self.moment == Some(selection) {
            self.moment = None;
        } else {
            self.moment = Some(selection);
            self.moment_selected = 0;
            if self
                .moment_detail
                .as_ref()
                .is_none_or(|detail| detail.selection != selection)
            {
                self.moment_detail = None;
                self.moment_rows.clear();
            }
            self.details = None;
            self.settings_open = false;
        }
        self.wake();
        cx.notify();
    }

    fn replace_moment_rows(&mut self, rows: Vec<(String, history::ProcessTotal, f64)>) {
        self.moment_selected = clamp_moment_row(self.moment_selected, rows.len());
        self.moment_rows = rows;
    }

    fn observe_sample(&mut self, snapshot: &Snapshot) {
        if matches!(snapshot.process_data, gtop_core::ProcessData::Stale { .. }) {
            return;
        }
        if let Some(key) = self
            .pending_kill
            .filter(|key| crate::details::kill_completed(*key, &snapshot.processes))
        {
            self.pending_kill = None;
            self.status = format!("PID {} stopped.", key.pid);
        }
        let now = rack::now();
        self.sample_seconds = self
            .last_snapshot
            .replace(now)
            .map(|t| (now - t).max(0.001))
            .unwrap_or(self.interval as f64);
        let keys: std::collections::HashSet<_> =
            snapshot.processes.iter().map(ProcessKey::from).collect();
        let detail_key = self.details.as_ref().map(|details| details.key);
        self.traces
            .retain(|key, _| keys.contains(key) || Some(*key) == detail_key);
        if let Some(details) = self.details.as_mut() {
            details.cancel_kill_if_exited(&snapshot.processes);
            if let Some(process) = snapshot
                .processes
                .iter()
                .find(|process| ProcessKey::from(*process) == details.key)
            {
                details.observe(process, now as u64);
            }
        }
        for p in snapshot.processes.iter() {
            let trace = self.traces.entry(ProcessKey::from(p)).or_default();
            trace.push_back((now, p.cpu_usage));
            while trace.front().is_some_and(|(t, _)| *t < now - 90.) || trace.len() > 90 {
                trace.pop_front();
            }
        }
    }
    fn accept(&mut self, snapshot: Snapshot, window: &Window, cx: &mut Context<Self>) {
        if let Some(details) = self.details.as_mut() {
            if let Some(history) = details
                .analysis
                .as_mut()
                .and_then(|analysis| analysis.history.as_mut())
            {
                history.now = crate::analysis::now_sentence(
                    crate::analysis::current_cpu(&snapshot.processes, &details.name),
                    history.busy_average,
                );
            }
        }
        let snapshot = Rc::new(snapshot);
        let key = self
            .details
            .as_ref()
            .map(|details| details.key)
            .or(self.table.read(cx).selected);
        self.table.update(cx, |table, cx| {
            table.snapshot = Some(snapshot.clone());
            table.reorder();
            if let Some(row) = key.and_then(|key| table.row_for(key)) {
                table.selected = table.process(row).map(ProcessKey::from);
            } else {
                table.selected = None;
            }

            cx.notify();
        });
        update_process_status(&mut self.status, &snapshot.process_data);
        self.snapshot = Some(snapshot);
        if gtop_core::perf_enabled() {
            eprintln!(
                "perf ui tick visible=true active={} rows={} total={}",
                window.is_window_active(),
                self.table.read(cx).order.len(),
                self.snapshot.as_ref().unwrap().processes.len()
            );
        }
        cx.notify();
    }
}

impl Render for Monitor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        draw_benchmark::record_render();
        self.rack(window, cx)
    }
}

actions!(gtop, [Quit, ToggleView, PreviousView]);

fn main() {
    gpui_platform::application()
        .with_assets(gpui_kit_assets::Assets)
        .run(|cx| {
            let quit_monitor: Rc<std::cell::RefCell<Option<Entity<Monitor>>>> =
                Rc::new(std::cell::RefCell::new(None));
            let quit_monitor_for_shutdown = quit_monitor.clone();
            cx.on_app_quit(move |cx| {
                let state = quit_monitor_for_shutdown
                    .borrow()
                    .as_ref()
                    .cloned()
                    .map(|monitor| {
                        cx.update_entity(&monitor, |monitor, _| {
                            (
                                monitor.settings_writes.take(),
                                monitor.settings_writer.take(),
                                monitor.settings.clone(),
                            )
                        })
                    });
                async move {
                    if let Some((sender, writer, settings)) = state {
                        if let (Some(sender), Some(writer)) = (sender, writer) {
                            palette::finish_writer(sender, settings, writer).await;
                        }
                    }
                }
            })
            .detach();
            gpui_component::init(cx);
            theme::init(cx);
            for bytes in [
                include_bytes!("../assets/fonts/DotoDisplay.ttf").as_slice(),
                include_bytes!("../assets/fonts/ArchivoDisplay.ttf").as_slice(),
                include_bytes!("../assets/fonts/JetBrainsMono.ttf").as_slice(),
                include_bytes!("../assets/fonts/ArchivoLabels.ttf").as_slice(),
                include_bytes!("../assets/fonts/Silkscreen.ttf").as_slice(),
            ] {
                cx.text_system()
                    .add_fonts(vec![std::borrow::Cow::Borrowed(bytes)])
                    .expect("bundled font");
            }
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("tab", ToggleView, Some("Gtop")),
                KeyBinding::new("shift-tab", PreviousView, Some("Gtop")),
            ]);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.set_menus([Menu::new("gtop").items([MenuItem::action("Quit gtop", Quit)])]);
            let bounds = Bounds::centered(None, size(px(1480.), px(930.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(1200.), px(720.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("gtop".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let monitor = cx.new(|cx| Monitor::new(window, cx));
                    *quit_monitor.borrow_mut() = Some(monitor.clone());
                    cx.new(|cx| Root::new(monitor, window, cx))
                },
            )
            .expect("open gtop window");
            cx.activate(true);
        });
}

#[cfg(test)]
mod wake_tests {
    use super::DrawMetrics;

    #[test]
    fn draw_metrics_are_unavailable_until_the_first_completed_draw() {
        let mut metrics = DrawMetrics::default();
        metrics.record(0, 1., None);
        assert_eq!(metrics.mean_ms, None);
        assert_eq!(metrics.fps, None);

        metrics.record(45, 1., Some(6.64));
        assert_eq!(metrics.mean_ms, Some(6.64));
        assert_eq!(metrics.fps, Some(45.));

        metrics.record(0, 1., None);
        assert_eq!(metrics.mean_ms, Some(6.64));
        assert_eq!(metrics.fps, Some(0.));
    }

    #[test]
    fn moment_row_selection_stays_within_replacement_ranking() {
        assert_eq!(super::clamp_moment_row(7, 1), 0);
        assert_eq!(super::clamp_moment_row(7, 8), 7);
        assert_eq!(super::clamp_moment_row(7, 0), 0);
    }

    #[test]
    fn repeated_controls_coalesce_and_can_be_drained() {
        let (mut send, mut receive) = futures::channel::mpsc::channel(1);
        let mut sent = 0;
        for _ in 0..100 {
            if send.try_send(()).is_ok() {
                sent += 1;
            }
        }
        assert!(sent <= 2);
        let mut drained = 0;
        while receive.try_recv().is_ok() {
            drained += 1;
        }
        assert_eq!(sent, drained);
        assert!(send.try_send(()).is_ok());
    }
}

fn update_process_status(status: &mut String, data: &gtop_core::ProcessData) {
    match data {
        gtop_core::ProcessData::Stale { since, reason } => {
            *status = format!(
                "Process data stale for {}s: {reason}",
                since.elapsed().as_secs()
            );
        }
        gtop_core::ProcessData::Fresh => {
            if status.starts_with("Reading")
                || status.starts_with("Refresh failed")
                || status.starts_with("Process data stale")
            {
                *status = "Select a process for details".into();
            }
        }
    }
}

#[cfg(test)]
mod process_status_tests {
    #[test]
    fn stale_status_shows_reason_and_age_then_clears_on_fresh_data() {
        let mut status = "Reading system information".to_string();
        super::update_process_status(
            &mut status,
            &gtop_core::ProcessData::Stale {
                since: std::time::Instant::now() - std::time::Duration::from_secs(7),
                reason: "enumeration failed".into(),
            },
        );
        assert_eq!(status, "Process data stale for 7s: enumeration failed");
        super::update_process_status(&mut status, &gtop_core::ProcessData::Fresh);
        assert_eq!(status, "Select a process for details");
    }
}
