mod analysis;
mod column_widths;
mod details;
mod disk;
mod disk_scan;
#[cfg(test)]
mod draw_benchmark;
mod glow;
mod history;
mod model;
mod now_json;
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
    color_picker::ColorPickerState,
    input::{InputEvent, InputState},
    Root,
};
use gtop::perf;
use gtop_core::{AppState, Snapshot};
use model::{ProcessKey, Search};
use process_table::Processes;
use std::{
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

const SESSION_RETENTION: Duration = Duration::from_secs(10 * 60);

fn after_retention(cx: &App, expire: impl FnOnce(&mut App) + 'static) -> Task<()> {
    let timer = cx.background_executor().timer(SESSION_RETENTION);
    cx.spawn(async move |cx| {
        timer.await;
        cx.update(expire);
    })
}

fn refresh_stopped(events: &mut mpsc::Receiver<()>) -> bool {
    loop {
        match events.try_recv() {
            Ok(()) => {}
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Closed) => return true,
        }
    }
}

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

#[derive(Clone, Copy)]
enum HistoryRead {
    Moment,
    Process,
}

#[derive(Default)]
struct HistoryReadErrors([Option<String>; 2]);

impl HistoryReadErrors {
    fn update(&mut self, status: &mut String, read: HistoryRead, error: Option<String>) {
        let owns_status = self.0.iter().flatten().any(|message| message == status);
        let message = error.map(|error| format!("History could not be read: {error}"));
        self.0[read as usize] = message.clone();
        if let Some(message) = message {
            *status = message;
        } else if owns_status {
            *status = self
                .0
                .iter()
                .flatten()
                .next()
                .cloned()
                .unwrap_or_else(|| "Select a process for details".into());
        }
    }
}

type ShutdownFuture = futures::future::Shared<futures::future::LocalBoxFuture<'static, ()>>;

struct ShutdownDrains {
    stopping: Rc<std::cell::Cell<bool>>,
    history: ShutdownFuture,
    settings: ShutdownFuture,
}

struct Monitor {
    core: Option<Arc<AppState>>,
    snapshot: Option<Rc<Snapshot>>,
    table: Entity<Processes>,
    search: Entity<InputState>,
    details: Option<Details>,
    panel_resize: details::PanelResize,
    disk: Option<disk::Disk>,
    paused: bool,
    history: [history::HistoryView; 3],
    moment: Option<history::MomentSelection>,
    moment_detail: Option<history::MomentDetail>,
    moment_selected: usize,
    moment_rows: Vec<(String, history::ProcessTotal, f64)>,
    view: Option<usize>,
    history_selected: Option<String>,
    history_scroll: UniformListScrollHandle,
    history_columns: column_widths::TableState,
    history_sort: history::HistorySort,
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
    color_pickers: [Entity<ColorPickerState>; 6],
    picker_hex_synced: [bool; 6],
    mix_name: Entity<InputState>,
    traces: std::collections::HashMap<ProcessKey, std::collections::VecDeque<(f64, f32)>>,
    interval: u64,
    status: String,
    pending_kill: Option<ProcessKey>,
    wake: mpsc::Sender<()>,
    _subscriptions: Vec<Subscription>,
    focus_restore: Option<Subscription>,
    _refresh: Option<Task<()>>,
    shutdown: Option<ShutdownDrains>,
    _ports: Task<()>,
    _perf_writer: Task<()>,
    _now_json: Option<now_json::Writer>,
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
        let color_pickers = palette::ColorField::ALL.map(|field| {
            cx.new(|cx| ColorPickerState::new(window, cx).default_value(rgb(field.get(base))))
        });
        let mix_name = cx.new(|cx| InputState::new(window, cx).placeholder("Name this mix"));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("name, pid, command or port"));
        let table = cx.new(|_| {
            let mut p = Processes::default();
            p.root_focus = Some(focus.clone());
            p.base = settings.base();
            p.cpu_scale = settings.cpu_scale;
            p.widths = settings.process_widths.clone();
            p
        });
        let stopping = Rc::new(std::cell::Cell::new(false));
        let settings_stopping = stopping.clone();
        let history_stopping = stopping.clone();
        let (wake, mut events) = mpsc::channel(1);
        let (settings_writes, mut settings_events) = mpsc::unbounded();
        let entity = cx.entity().downgrade();
        let mut settings_cx = window.to_async(cx);
        let settings_drain = async move {
            let cx = &mut settings_cx;
            while let Some(settings) = palette::next_settings(&mut settings_events).await {
                let result = cx
                    .background_executor()
                    .spawn(async move { settings.save() })
                    .await;
                if settings_stopping.get() {
                    continue;
                }
                if let Err(error) = result {
                    let _ = entity.update_in(cx, |this, _, cx| {
                        this.status = format!("Settings could not be saved: {error}");
                        cx.notify();
                    });
                }
            }
        }
        .boxed_local()
        .shared();
        let settings_writer = cx.foreground_executor().spawn(settings_drain.clone());
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
            cx.subscribe_in(
                &table,
                window,
                |this, _, event: &process_table::WidthsChanged, window, cx| {
                    this.settings.process_widths = event.0.clone();
                    if event.1 {
                        this.persist_settings(window, cx);
                    }
                },
            ),
            cx.observe_window_visibility(window, |this, visibility, _, _| {
                if gtop_core::perf_enabled() {
                    eprintln!("perf visibility visible={}", visibility.is_visible());
                }
                if let Some(disk) = &this.disk {
                    disk.set_visible(visibility.is_visible());
                }
                this.wake();
            }),
        ];
        subscriptions.extend(Self::palette_subscriptions(
            &colors,
            &color_pickers,
            window,
            cx,
        ));
        let cached_ports = Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            u32,
            Vec<u16>,
        >::new()));
        let perf = Arc::new(perf::PerfCounters::default());
        let history_perf = perf.clone();
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
        let entity = cx.entity().downgrade();
        let mut refresh_cx = window.to_async(cx);
        let history_drain = async move {
            let cx = &mut refresh_cx;
            let core = cx.background_executor().spawn(async { Arc::new(AppState::new()) }).await;
            if history_stopping.get() || entity.update_in(cx, |this, _, _| this.core = Some(core.clone())).is_err() { return; }
            let opened = cx.background_executor().spawn(async {
                history::History::open(&history::support_path("history.sqlite"), &history::support_path("history.json"), rack::now() as u64)
            }).await;
            let mut history = match opened {
                Ok(history) => history,
                Err(error) => {
                    if !history_stopping.get() { let _ = entity.update_in(cx, |this, _, cx| {
                        this.status = format!("History could not be opened: {error}");
                        cx.notify();
                    }); }
                    history::History::default()
                }
            };
            let mut cached_minute = None;
            let mut cached_moment = None;
            let mut cached_metric = None;
            let mut cached_analysis_key = None;
            let mut read_errors = HistoryReadErrors::default();
            loop {
                if history_stopping.get() || refresh_stopped(&mut events) { break; }
                let Ok((running, ports, seconds, moment, metric, selected_detail)) = entity.update_in(cx, |this, window, cx| {
                    let selected_detail = this.details.as_ref().map(|details| {
                        (
                            details.key,
                            details.name.clone(),
                            details.executable.clone(),
                            details.analysis.is_none(),
                        )
                    });
                    (!this.paused, window.is_visible() && this.table.read(cx).search.needs_ports(), this.interval, this.moment, this.metric, selected_detail)
                }) else { break; };
                history_perf.set_refresh_interval(seconds);
                if moment.is_none() {
                    cached_moment = None;
                    cached_metric = None;
                }
                if !running {
                    history.suspend();
                    let mut history_retry = false;
                    if let Some(selection) = moment {
                        if cached_moment != Some(selection) || cached_metric != Some(metric) {
                            let (returned_history, detail) = cx.background_executor().spawn(async move {
                                let detail = history.moment(selection);
                                (history, detail)
                            }).await;
                            history = returned_history;
                            if history_stopping.get() { break; }
                            match detail {
                                Ok(detail) => {
                                    let _ = entity.update_in(cx, |this, _, cx| {
                                        read_errors.update(&mut this.status, HistoryRead::Moment, None);
                                        cx.notify();
                                        if this.moment == Some(selection) {
                                            this.replace_moment_rows(detail.ranked(metric));
                                            this.moment_detail = Some(detail);
                                            cx.notify();
                                        }
                                    });
                                    cached_moment = Some(selection);
                                    cached_metric = Some(metric);
                                }
                                Err(error) => {
                                    history_retry = true;
                                    let _ = entity.update_in(cx, |this, _, cx| { read_errors.update(&mut this.status, HistoryRead::Moment, Some(error.to_string())); cx.notify(); });
                                }
                            }
                        }
                    }
                    let pending = entity.update_in(cx, |this, _, _| this.pending_kill).ok().flatten();
                    if let Some(key) = pending {
                        let state = core.clone();
                        let stopped = cx.background_executor().spawn(async move {
                            state.process_stopped(key.pid, key.start_time)
                        }).await;
                        if history_stopping.get() { break; }
                        let _ = entity.update_in(cx, |this, _, cx| {
                            if this.pending_kill == Some(key) && matches!(stopped, Ok(true)) {
                                this.pending_kill = None;
                                this.status = format!("PID {} stopped.", key.pid);
                                cx.notify();
                            }
                        });
                    }
                    if let Some((key, name, command, needs_analysis)) = selected_detail.as_ref() {
                        if *needs_analysis || cached_analysis_key != Some(*key) {
                            let (name, command) = (name.clone(), command.clone());
                            let (returned_history, analysis) = cx.background_executor().spawn(async move {
                                let analysis = history.analyze_process(
                                    &name,
                                    &command,
                                    rack::now() as u64 / 60,
                                );
                                (history, analysis)
                            }).await;
                            history = returned_history;
                            if history_stopping.get() { break; }
                            match analysis {
                                Ok(analysis) => {
                                    let _ = entity.update_in(cx, |this, _, cx| {
                                        read_errors.update(&mut this.status, HistoryRead::Process, None);
                                        cx.notify();
                                        if let Some(details) = this.details.as_mut().filter(|details| details.key == *key) {
                                            details.analysis = Some(analysis);
                                            cx.notify();
                                        }
                                    });
                                    cached_analysis_key = Some(*key);
                                }
                                Err(error) => {
                                    history_retry = true;
                                    cached_analysis_key = None;
                                    let _ = entity.update_in(cx, |this, _, cx| { read_errors.update(&mut this.status, HistoryRead::Process, Some(error.to_string())); cx.notify(); });
                                }
                            }
                        }
                    } else {
                        cached_analysis_key = None;
                    }
                    if pending.is_some() || history_retry {
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
                let sample_perf = history_perf.clone();
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
                    let sample_duration = start.elapsed();
                    sample_perf.add_sampler(sample_duration);
                    let tick_ms = sample_duration.as_secs_f64() * 1000.;
                    let mut views = None;
                    let mut moment_detail = None;
                    let mut process_analysis = None;
                    let mut history_save = None;
                    if let Ok(ref snapshot) = snapshot {
                        let now = rack::now();

                        let update = (|| -> history::Result<()> {
                        let history_record_started = std::time::Instant::now();
                        let recorded = history.record(snapshot, now, seconds as f64);
                        sample_perf.add_history_write(history_record_started.elapsed());
                        recorded?;
                        let minute = now as u64 / 60;
                        let minute_changed = cached_minute != Some(minute);
                        if let Some((key, name, command, needs_analysis)) = selected_detail.as_ref() {
                            if *needs_analysis || previous_analysis_key != Some(*key) || minute_changed {
                                process_analysis = Some((
                                    *key,
                                    history.analyze_process(
                                        name,
                                        command,
                                        minute,
                                    )?,
                                ));
                            }
                        }
                        if let Some(selection) = moment {
                            let span_changed = selection.start_minute < history.touched.end
                                && history.touched.start < selection.start_minute + selection.step as u64;
                            if previous_moment != Some(selection) || previous_metric != Some(metric) || span_changed {
                                moment_detail = Some(history.moment(selection)?);
                            }
                        }
                        if minute_changed {
                            views = Some([history.view(1,now as u64)?, history.view(8,now as u64)?, history.view(24,now as u64)?]);
                            let write_started = std::time::Instant::now();
                            let persisted = history.persist(now as u64);
                            sample_perf.add_history_write(write_started.elapsed());
                            persisted?;
                            cached_minute = Some(minute);
                            history_save=Some(Ok(()));
                        }
                        Ok(())
                        })();
                        if let Err(error) = update {
                            views = None;
                            moment_detail = None;
                            process_analysis = None;
                            history_save = Some(Err(error.to_string()));
                        }
                    }
                    let selected_key = match selected_detail {
                        Some((key, _, _, _)) if !matches!(history_save, Some(Err(_))) && (snapshot.is_ok() || previous_analysis_key == Some(key)) => Some(key),
                        _ => None,
                    };
                    (snapshot, tick_ms, views, moment_detail, process_analysis, history, cached_minute, history_save, moment, metric, selected_key)
                }).await;
                let (result, tick_ms, views, moment_detail, process_analysis, returned_history, returned_minute, history_save, selection, metric, selected_key) = result;
                cached_minute=returned_minute;
                cached_analysis_key = selected_key;
                history = returned_history;
                if history_stopping.get() { break; }
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
                        read_errors.update(&mut this.status, HistoryRead::Moment, None);
                        this.replace_moment_rows(detail.ranked(metric));
                        this.moment_detail = Some(detail);
                    }
                    if let Some((key, analysis)) = process_analysis {
                        if let Some(details) = this.details.as_mut().filter(|details| details.key == key) {
                            read_errors.update(&mut this.status, HistoryRead::Process, None);
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
                                if let Some(writer) = &this._now_json {
                                    let stats = &snapshot.stats;
                                    writer.update(now_json::SampledMetrics {
                                        time: rack::now() as u64,
                                        interval_s: seconds,
                                        sampled_at: Instant::now(),
                                        cpu_percent: (stats.cpu_usage.iter().sum::<f32>()
                                            / stats.cpu_usage.len().max(1) as f32)
                                            as f64,
                                        memory_used_bytes: stats.memory_used,
                                        memory_total_bytes: stats.memory_total,
                                        disk_free_bytes: (stats.disk_total_bytes > 0)
                                            .then_some(stats.disk_free_bytes),
                                        disk_total_bytes: (stats.disk_total_bytes > 0)
                                            .then_some(stats.disk_total_bytes),
                                        disk_io_totals: stats.disk_io_totals,
                                        rx_bytes_per_s: stats.network_rx_bytes,
                                        tx_bytes_per_s: stats.network_tx_bytes,
                                        temperature_c: stats.temperatures.iter()
                                            .find(|temperature| temperature.name == "SSD")
                                            .map(|temperature| temperature.celsius as f64),
                                    });
                                }
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
            cx.background_executor().spawn(async move {
                if let Err(error) = history.persist(rack::now() as u64) {
                    eprintln!("History could not be saved at shutdown: {error}");
                }
            }).await;
        }.boxed_local().shared();
        let refresh = cx.foreground_executor().spawn(history_drain.clone());
        let frame_perf = perf.clone();
        let frames = cx.spawn_in(window, async move |entity, cx| {
            let mut previous = None;
            let mut sampled_at = std::time::Instant::now();
            let mut cpu_baseline = perf::CpuBaseline::new(perf::process_cpu_seconds());
            let mut system_cpu_baseline = perf::SystemCpuBaseline::new(perf::system_cpu_ticks());
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let now = std::time::Instant::now();
                let elapsed = now.duration_since(sampled_at).as_secs_f64();
                sampled_at = now;
                let state = perf::window_state();
                let cpu = cpu_baseline.sample(perf::process_cpu_seconds());
                let system_cpu_busy = system_cpu_baseline.sample(perf::system_cpu_ticks());
                frame_perf.add_window_sample(
                    state,
                    Duration::from_secs_f64(elapsed),
                    cpu,
                    perf::load_average_1m(),
                    system_cpu_busy,
                );
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
                        frame_perf.add_draws(
                            draws.len(),
                            Duration::from_secs_f64(
                                draw_ms.unwrap_or_default() * draws.len() as f64 / 1000.,
                            ),
                        );
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
        let writer_perf = perf.clone();
        let writer_executor = cx.background_executor().clone();
        let timer_executor = writer_executor.clone();
        let perf_writer = writer_executor.spawn(async move {
            let started = std::time::Instant::now();
            let mut recorder = match perf::PerfRecorder::open(
                &history::support_path("perf.sqlite"),
                writer_perf,
            ) {
                Ok(recorder) => recorder,
                Err(error) => {
                    eprintln!("Performance telemetry could not open: {error}");
                    return;
                }
            };
            loop {
                timer_executor.timer(Duration::from_secs(60)).await;
                if let Err(error) = recorder.record_if_due(started.elapsed(), rack::now() as u64) {
                    eprintln!("Performance telemetry could not be saved: {error}");
                }
            }
        });
        let now_json = match now_json::Writer::start(
            history::support_path("now.json")
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_path_buf(),
        ) {
            Ok(writer) => Some(writer),
            Err(error) => {
                eprintln!("Current metrics writer could not start: {error}");
                None
            }
        };
        Self {
            core: None,
            snapshot: None,
            table,
            search,
            details: None,
            panel_resize: details::PanelResize::default(),
            disk: None,
            paused: false,
            history: std::array::from_fn(|_| history::HistoryView::default()),
            moment: None,
            moment_detail: None,
            moment_selected: 0,
            moment_rows: Vec::new(),
            view: None,
            history_selected: None,
            history_scroll: UniformListScrollHandle::new(),
            history_columns: column_widths::TableState::default(),
            history_sort: history::HistorySort::default(),
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
            color_pickers,
            picker_hex_synced: [false; 6],
            mix_name,
            traces: std::collections::HashMap::new(),
            interval: 3,
            status: "Reading system information…".into(),
            pending_kill: None,
            wake,
            _subscriptions: subscriptions,
            focus_restore: None,
            _refresh: Some(refresh),
            shutdown: Some(ShutdownDrains {
                stopping,
                history: history_drain,
                settings: settings_drain,
            }),
            _frames: frames,
            _ports: ports_task,
            _perf_writer: perf_writer,
            _now_json: now_json,
        }
    }

    fn wake(&mut self) {
        let _ = self.wake.try_send(());
    }

    fn select_moment(&mut self, selection: history::MomentSelection, cx: &mut Context<Self>) {
        if self.moment == Some(selection) {
            self.moment = None;
        } else {
            if let Some(disk) = &mut self.disk {
                disk.close();
            }
            self.panel_resize.finish();
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
    if disk_scan::headless() {
        return;
    }
    let pending_quit: Rc<std::cell::RefCell<Option<Task<()>>>> = Rc::default();
    let reopen_quit = pending_quit.clone();
    let application = gpui_platform::application();
    application.on_reopen(move |cx| {
        reopen_quit.borrow_mut().take();
        cx.activate(true);
        for window in cx.windows() {
            let _ = window.update(cx, |_, window, _| window.activate_window());
        }
    });
    application
        .with_assets(gpui_kit_assets::Assets)
        .run(move |cx| {
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
                            if let Some(drains) = &monitor.shutdown {
                                drains.stopping.set(true);
                            }
                            monitor.wake.close_channel();
                            (
                                monitor._refresh.take(),
                                monitor.shutdown.take(),
                                monitor.settings_writes.take(),
                                monitor.settings_writer.take(),
                                monitor._now_json.take(),
                                monitor.settings.clone(),
                            )
                        })
                    });
                async move {
                    if let Some((refresh, drains, sender, writer, now_json, settings)) = state {
                        // Shutdown blocks foreground tasks. Poll their shared futures here.
                        drop(refresh);
                        drop(writer);
                        drop(now_json);
                        if let Some(ShutdownDrains {
                            history,
                            settings: settings_drain,
                            ..
                        }) = drains
                        {
                            // GPUI gives both drains the same 200 ms deadline.
                            futures::join!(history, async {
                                if let Some(sender) = sender {
                                    palette::finish_writer(sender, settings, settings_drain).await;
                                }
                            });
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
                move |window, cx| {
                    let visible_quit = pending_quit.clone();
                    window
                        .observe_window_visibility(move |visibility, _, _| {
                            if visibility.is_visible() {
                                visible_quit.borrow_mut().take();
                            }
                        })
                        .detach();
                    let close_quit = pending_quit.clone();
                    window.on_window_should_close(cx, move |_, cx| {
                        cx.hide();
                        *close_quit.borrow_mut() = Some(after_retention(cx, |cx| cx.quit()));
                        false
                    });
                    let monitor = cx.new(|cx| {
                        let mut monitor = Monitor::new(window, cx);
                        if std::env::args().any(|arg| arg == "--disk") {
                            monitor.open_disk(window, cx);
                        }
                        monitor
                    });
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
    fn retained_window_timeout_can_be_cancelled_on_reopen() {
        use gpui::TestAppContext;
        use std::{cell::Cell, rc::Rc};

        let cx = TestAppContext::single();
        let expired = Rc::new(Cell::new(false));
        let flag = expired.clone();
        let pending = cx.update(|cx| super::after_retention(cx, move |_| flag.set(true)));
        drop(pending);
        cx.background_executor
            .advance_clock(super::SESSION_RETENTION);
        cx.run_until_parked();
        assert!(!expired.get());

        let flag = expired.clone();
        let _pending = cx.update(|cx| super::after_retention(cx, move |_| flag.set(true)));
        cx.background_executor
            .advance_clock(super::SESSION_RETENTION);
        cx.run_until_parked();
        assert!(expired.get());
    }

    #[test]
    fn quit_drains_partial_history_with_queued_wakes() {
        use futures::{channel::mpsc, FutureExt, StreamExt};
        use gpui::{AppContext, Task, TestAppContext};
        use std::{cell::RefCell, rc::Rc};

        struct Holder {
            wake: mpsc::Sender<()>,
            refresh: Option<Task<()>>,
            drain: Option<super::ShutdownFuture>,
        }
        let directory = std::env::temp_dir().join(format!("gtop-quit-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("history.sqlite");
        let mut history =
            crate::history::History::open(&path, &directory.join("absent.json"), 6000).unwrap();
        let snapshot = crate::test_data::snapshot();
        history.record(&snapshot, 6000., 3.).unwrap();
        history.persist(6000).unwrap();
        history.record(&snapshot, 6003., 3.).unwrap();
        let mut cx = TestAppContext::single();
        let (wake, mut events) = mpsc::channel(1);
        let background = cx.background_executor.clone();
        let drain = async move {
            while !super::refresh_stopped(&mut events) {
                if events.next().await.is_none() {
                    break;
                }
            }
            background
                .spawn(async move {
                    history.persist(6003).unwrap();
                })
                .await;
        }
        .boxed_local()
        .shared();
        let refresh = cx.foreground_executor().spawn(drain.clone());
        let holder = cx.new(|_| Holder {
            wake,
            refresh: Some(refresh),
            drain: Some(drain),
        });
        let keep = Rc::new(RefCell::new(holder.clone()));
        cx.run_until_parked();
        holder.update(&mut cx, |holder, _| {
            holder.wake.try_send(()).unwrap();
        });
        cx.update(|app| {
            app.on_app_quit(move |cx| {
                let refresh = keep.borrow().update(cx, |holder, _| {
                    holder.wake.close_channel();
                    drop(holder.refresh.take());
                    holder.drain.take()
                });
                async move {
                    if let Some(refresh) = refresh {
                        refresh.await;
                    }
                }
            })
            .detach()
        });
        cx.quit();
        cx.run_until_parked();
        let connection = rusqlite::Connection::open(&path).unwrap();
        let seconds: f64 = connection
            .query_row(
                "SELECT seconds FROM system WHERE resolution=1 AND minute=100",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            (seconds - 3.001).abs() < 1e-9,
            "durable seconds after quit: {seconds}"
        );
        drop(connection);
        drop(holder);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn closed_wake_channel_stops_before_another_refresh() {
        let (mut sender, mut receiver) = futures::channel::mpsc::channel(1);
        assert!(!super::refresh_stopped(&mut receiver));
        sender.try_send(()).unwrap();
        sender.close_channel();
        assert!(super::refresh_stopped(&mut receiver));
    }

    #[test]
    fn recovered_history_read_clears_its_warning() {
        for read in [super::HistoryRead::Moment, super::HistoryRead::Process] {
            let mut errors = super::HistoryReadErrors::default();
            let mut status = "Select a process for details".into();
            errors.update(&mut status, read, Some("database busy".into()));
            assert_eq!(status, "History could not be read: database busy");
            errors.update(&mut status, read, None);
            assert_eq!(status, "Select a process for details");
        }
    }

    #[test]
    fn recovered_history_read_preserves_other_read_failure() {
        let mut errors = super::HistoryReadErrors::default();
        let mut status = String::new();
        errors.update(
            &mut status,
            super::HistoryRead::Moment,
            Some("moment busy".into()),
        );
        errors.update(
            &mut status,
            super::HistoryRead::Process,
            Some("process busy".into()),
        );
        errors.update(&mut status, super::HistoryRead::Process, None);
        assert_eq!(status, "History could not be read: moment busy");
        errors.update(&mut status, super::HistoryRead::Moment, None);
        assert_eq!(status, "Select a process for details");
    }

    #[test]
    fn recovered_history_read_preserves_unrelated_status() {
        let mut errors = super::HistoryReadErrors::default();
        let mut status = String::new();
        errors.update(
            &mut status,
            super::HistoryRead::Moment,
            Some("database busy".into()),
        );
        status = "Settings could not be saved: disk full".into();
        errors.update(&mut status, super::HistoryRead::Moment, None);
        assert_eq!(status, "Settings could not be saved: disk full");
    }

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
