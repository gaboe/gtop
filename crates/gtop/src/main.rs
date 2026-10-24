mod analysis;
mod details;
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
    moment_rows: Vec<(String, history::ProcessTotal, f64)>,
    view: Option<usize>,
    history_selected: Option<String>,
    history_scroll: UniformListScrollHandle,
    metric: history::Metric,
    settings: palette::Settings,
    settings_open: bool,
    tick_ms: f64,
    sample_seconds: f64,
    last_snapshot: Option<f64>,
    focus: FocusHandle,
    draft: Option<palette::Base>,
    colors: [Entity<InputState>; 6],
    mix_name: Entity<InputState>,
    traces: std::collections::HashMap<ProcessKey, std::collections::VecDeque<(f64, f32)>>,
    interval: u64,
    status: String,
    wake: mpsc::Sender<()>,
    _subscriptions: Vec<Subscription>,
    _refresh: Task<()>,
    _ports: Task<()>,
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
                        let current_cpu = this.snapshot.as_ref().and_then(|snapshot| {
                            snapshot.processes.iter().find(|process| {
                                process.pid == details.key.pid
                                    && process.start_time == details.key.start_time
                            })
                        }).map(|process| process.cpu_usage).unwrap_or(0.);
                        (
                            details.key,
                            details.name.clone(),
                            details.command.clone(),
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
                    if events.next().await.is_none() { break; }
                    continue;
                }
                let state = core.clone();
                let port_cache=cached_ports.clone();
                let previous_moment = cached_moment;
                let previous_metric = cached_metric;
                let previous_analysis_key = cached_analysis_key;
                let result = cx.background_executor().spawn(async move {
                    let start = std::time::Instant::now();
                    let mut snapshot = state.snapshot(false, ports);
                    if let Ok(ref mut snapshot)=snapshot {
                        if !ports { if let Ok(cached)=port_cache.lock(){for p in &mut snapshot.processes {if let Some(ports)=cached.get(&p.pid){p.ports=ports.clone();}}} }
                    }
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
                        if let Some((key, name, command, fallback_cpu, needs_analysis)) = selected_detail.as_ref() {
                            if *needs_analysis || previous_analysis_key != Some(*key) || minute_changed {
                                let current_cpu = snapshot.processes.iter().find(|process| {
                                    process.pid == key.pid && process.start_time == key.start_time
                                }).map(|process| process.cpu_usage).unwrap_or(*fallback_cpu);
                                process_analysis = Some((
                                    *key,
                                    history.analyze_process(name, command, current_cpu, minute),
                                ));
                            }
                        }
                        if let Some(selection) = moment {
                            let span_changed = selection.start_minute <= minute
                                && minute < selection.start_minute + selection.step as u64;
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
                        this.moment_rows = detail.ranked(metric);
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
            moment_rows: Vec::new(),
            view: None,
            history_selected: None,
            history_scroll: UniformListScrollHandle::new(),
            metric: history::Metric::Cpu,
            settings,
            settings_open: false,
            tick_ms: 0.,
            sample_seconds: 3.,
            last_snapshot: None,
            focus,
            draft: None,
            colors,
            mix_name,
            traces: std::collections::HashMap::new(),
            interval: 3,
            status: "Reading system information…".into(),
            wake,
            _subscriptions: subscriptions,
            _refresh: refresh,
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

    fn observe_sample(&mut self, snapshot: &Snapshot) {
        let now = rack::now();
        self.sample_seconds = self
            .last_snapshot
            .replace(now)
            .map(|t| (now - t).max(0.001))
            .unwrap_or(self.interval as f64);
        let keys: std::collections::HashSet<_> =
            snapshot.processes.iter().map(ProcessKey::from).collect();
        self.traces.retain(|k, _| keys.contains(k));
        for p in &snapshot.processes {
            let trace = self.traces.entry(ProcessKey::from(p)).or_default();
            trace.push_back((now, p.cpu_usage));
            while trace.front().is_some_and(|(t, _)| *t < now - 90.) || trace.len() > 90 {
                trace.pop_front();
            }
        }
    }
    fn accept(&mut self, snapshot: Snapshot, window: &Window, cx: &mut Context<Self>) {
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
        self.snapshot = Some(snapshot);
        if self.status.starts_with("Reading") || self.status.starts_with("Refresh failed") {
            self.status = "Select a process for details".into();
        }
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
        self.rack(window, cx)
    }
}

actions!(gtop, [Quit]);

fn main() {
    gpui_platform::application()
        .with_assets(gpui_kit_assets::Assets)
        .run(|cx| {
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
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
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
                    cx.new(|cx| Root::new(monitor, window, cx))
                },
            )
            .expect("open gtop window");
            cx.activate(true);
        });
}

#[cfg(test)]
mod wake_tests {
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
