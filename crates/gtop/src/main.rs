mod details;
mod model;
mod process_table;
mod stats;
mod theme;

use details::Details;
use futures::{channel::mpsc, FutureExt, StreamExt};
use gpui::{prelude::*, *};
use gpui_component::{
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    table::{DataTable, TableEvent, TableState},
    Root, Sizable,
};
use gtop_core::{AppState, Snapshot};
use model::{ProcessKey, Search};
use process_table::Processes;
use std::{rc::Rc, sync::Arc, time::Duration};

struct Monitor {
    core: Option<Arc<AppState>>,
    snapshot: Option<Rc<Snapshot>>,
    table: Entity<TableState<Processes>>,
    search: Entity<InputState>,
    details: Option<Details>,
    paused: bool,
    interval: u64,
    status: String,
    wake: mpsc::UnboundedSender<()>,
    _subscriptions: Vec<Subscription>,
    _refresh: Task<()>,
}

impl Monitor {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search name, command, PID, :port or port:N")
        });
        let table = cx.new(|cx| {
            TableState::new(Processes::default(), window, cx)
                .col_movable(false)
                .col_selectable(false)
                .loop_selection(false)
        });
        let (wake, mut events) = mpsc::unbounded();
        let subscriptions = vec![
            cx.subscribe_in(&search, window, |this, input, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let query = input.read(cx).value();
                    let mut changed_ports = false;
                    this.table.update(cx, |table, cx| {
                        let rows = table.delegate_mut();
                        let previous = rows.search.needs_ports();
                        rows.search = Search::new(&query);
                        changed_ports = previous != rows.search.needs_ports();
                        rows.reorder();
                        if let Some(row) = rows.selected.and_then(|key| rows.row_for(key)) {
                            table.set_selected_row(row, cx);
                        } else {
                            table.clear_selection(cx);
                        }
                        cx.notify();
                    });
                    if changed_ports {
                        this.wake();
                    }
                    cx.notify();
                }
            }),
            cx.subscribe_in(&table, window, |this, table, event, _, cx| {
                if let TableEvent::SelectRow(row) | TableEvent::DoubleClickedRow(row) = event {
                    let key = table
                        .read(cx)
                        .delegate()
                        .process(*row)
                        .map(ProcessKey::from);
                    if let Some(key) = key {
                        this.select(key, cx);
                    }
                }
            }),
            cx.observe_window_visibility(window, |this, visibility, _, _| {
                if gtop_core::perf_enabled() {
                    eprintln!("perf visibility visible={}", visibility.is_visible());
                }
                this.wake();
            }),
        ];
        let refresh = cx.spawn_in(window, async move |entity, cx| {
            let core = cx.background_executor().spawn(async { Arc::new(AppState::new()) }).await;
            if entity.update_in(cx, |this, _, _| this.core = Some(core.clone())).is_err() { return; }
            loop {
                let Ok((running, ports, seconds)) = entity.update_in(cx, |this, window, cx| {
                    (!this.paused && window.is_visible(), this.table.read(cx).delegate().search.needs_ports(), this.interval)
                }) else { break; };
                if !running {
                    if events.next().await.is_none() { break; }
                    continue;
                }
                let state = core.clone();
                let result = cx.background_executor().spawn(async move { state.snapshot(false, ports) }).await;
                if entity.update_in(cx, |this, window, cx| {
                    match result {
                        Ok(snapshot) => {
                            if !this.paused && window.is_visible() { this.accept(snapshot, window, cx); }
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
            interval: 3,
            status: "Reading system information…".into(),
            wake,
            _subscriptions: subscriptions,
            _refresh: refresh,
        }
    }

    fn wake(&self) {
        let _ = self.wake.unbounded_send(());
    }

    fn accept(&mut self, snapshot: Snapshot, window: &Window, cx: &mut Context<Self>) {
        let snapshot = Rc::new(snapshot);
        let key = self.details.as_ref().map(|details| details.key);
        self.table.update(cx, |table, cx| {
            table.delegate_mut().snapshot = Some(snapshot.clone());
            table.delegate_mut().reorder();
            if let Some(row) = key.and_then(|key| table.delegate().row_for(key)) {
                table.set_selected_row(row, cx);
            } else {
                table.clear_selection(cx);
            }
            table.refresh(cx);
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
                self.table.read(cx).delegate().order.len(),
                self.snapshot.as_ref().unwrap().processes.len()
            );
        }
        cx.notify();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .bg(rgb(theme::MANTLE))
            .child(
                div()
                    .flex_1()
                    .max_w(px(520.))
                    .child(Input::new(&self.search).cleanable(true)),
            )
            .child(div().flex_1())
            .child(div().text_color(rgb(theme::SUBTEXT)).child("Refresh"))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .children([1, 2, 3, 5, 10].map(|seconds| {
                        Button::new(("interval", seconds as usize))
                            .label(format!("{seconds}s"))
                            .small()
                            .when(self.interval == seconds, |button| button.primary())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.interval = seconds;
                                this.wake();
                                cx.notify();
                            }))
                    })),
            )
            .child(
                Button::new("pause")
                    .label(if self.paused { "Resume" } else { "Pause" })
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.paused = !this.paused;
                        this.wake();
                        cx.notify();
                    })),
            )
    }
}

impl Render for Monitor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.table.read(cx).delegate().order.len();
        let total = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.processes.len())
            .unwrap_or(0);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::BASE))
            .text_color(rgb(theme::TEXT))
            .text_size(px(13.))
            .when_some(self.snapshot.as_ref(), |this, snapshot| {
                this.child(stats::render(&snapshot.stats))
            })
            .child(self.toolbar(cx))
            .child(
                div().flex_1().min_h_0().child(
                    DataTable::new(&self.table)
                        .stripe(true)
                        .bordered(false)
                        .scroll_bounce(false),
                ),
            )
            .when_some(self.details(cx), |this, details| this.child(details))
            .child(
                div()
                    .id("status")
                    .role(Role::Status)
                    .aria_label(format!(
                        "{count} of {total} processes. {}. {}",
                        if self.paused { "Paused" } else { "Live" },
                        self.status
                    ))
                    .flex()
                    .justify_between()
                    .gap_4()
                    .px_3()
                    .py_2()
                    .bg(rgb(theme::MANTLE))
                    .text_size(px(12.))
                    .text_color(rgb(theme::SUBTEXT))
                    .child(format!(
                        "{count} / {total} processes   ·   {}",
                        if self.paused { "Paused" } else { "Live" }
                    ))
                    .child(self.status.clone()),
            )
    }
}

actions!(gtop, [Quit]);

fn main() {
    gpui_platform::application()
        .with_assets(gpui_kit_assets::Assets)
        .run(|cx| {
            gpui_component::init(cx);
            theme::init(cx);
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
