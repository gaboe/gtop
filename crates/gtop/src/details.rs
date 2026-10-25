use crate::rack::RackButton;
use crate::{
    model::{bytes, duration, ProcessKey},
    theme, Monitor,
};
use gpui::{prelude::*, *};
use gpui_component::{
    button::{Button, ButtonVariants},
    Disableable, Sizable,
};
use gtop_core::ProcessInfo;

enum Environment {
    Unrequested,
    Loading,
    Loaded(Vec<String>),
    Failed(String),
}

pub(super) struct Details {
    pub(super) key: ProcessKey,
    last_known: ProcessInfo,
    last_seen: u64,
    pub(super) name: String,
    pub(super) executable: String,
    pub(super) analysis: Option<crate::analysis::ProcessAnalysis>,
    environment: Environment,
    environment_open: bool,
    body_scroll: ScrollHandle,
    pub(super) kill: KillState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum KillState {
    Ready,
    Confirming,
    Sending,
}

pub(super) fn close_details(details: &mut Option<Details>) {
    *details = None;
}

pub(super) fn kill_completed(key: ProcessKey, processes: &[ProcessInfo]) -> bool {
    !processes
        .iter()
        .any(|process| ProcessKey::from(process) == key && process.status != "Zombie")
}

impl Details {
    pub(super) fn observe(&mut self, process: &ProcessInfo, last_seen: u64) {
        if ProcessKey::from(process) == self.key {
            self.last_known = process.clone();
            self.last_seen = last_seen;
            self.name.clone_from(&process.name);
            self.executable.clone_from(&process.executable);
        }
    }

    fn process<'a>(&'a self, processes: &'a [ProcessInfo]) -> &'a ProcessInfo {
        processes
            .iter()
            .find(|process| ProcessKey::from(*process) == self.key)
            .unwrap_or(&self.last_known)
    }

    fn exited(&self, processes: &[ProcessInfo]) -> bool {
        !processes
            .iter()
            .any(|process| ProcessKey::from(process) == self.key)
    }

    fn exit_marker(&self) -> String {
        format!(
            "EXITED · last seen {}",
            crate::rack::timestamp(self.last_seen, "%H:%M:%S")
        )
    }

    fn can_load_environment(&self, processes: &[ProcessInfo]) -> bool {
        !self.exited(processes)
    }

    pub(super) fn cancel_kill_if_exited(&mut self, processes: &[ProcessInfo]) {
        if self.exited(processes) {
            self.cancel_kill(self.key);
        }
    }

    pub(super) fn request_kill(&mut self, rendered_for: ProcessKey, alive: bool) {
        if alive && self.key == rendered_for && self.kill == KillState::Ready {
            self.kill = KillState::Confirming;
        }
    }

    pub(super) fn cancel_kill(&mut self, rendered_for: ProcessKey) {
        if self.key == rendered_for && self.kill == KillState::Confirming {
            self.kill = KillState::Ready;
        }
    }

    fn take_kill_confirmation(&mut self, rendered_for: ProcessKey) -> Option<ProcessKey> {
        if self.key != rendered_for || self.kill != KillState::Confirming {
            return None;
        }
        self.kill = KillState::Sending;
        Some(self.key)
    }
}

impl Monitor {
    pub(super) fn select(&mut self, key: ProcessKey, cx: &mut Context<Self>) {
        self.moment = None;
        if self
            .details
            .as_ref()
            .is_some_and(|details| details.key == key)
        {
            return;
        }
        let process = self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .processes
                .iter()
                .find(|process| ProcessKey::from(*process) == key)
                .cloned()
        });
        let Some(process) = process else { return };
        let name = process.name.clone();
        let executable = process.executable.clone();
        self.details = Some(Details {
            key,
            last_known: process,
            last_seen: crate::rack::now() as u64,
            name,
            executable,
            analysis: None,
            environment: Environment::Unrequested,
            environment_open: false,
            body_scroll: ScrollHandle::new(),
            kill: KillState::Ready,
        });
        self.wake();
        self.table.update(cx, |table, _cx| {
            table.selected = Some(key);
            if let Some(row) = table.row_for(key) {
                table.selected = table.process(row).map(ProcessKey::from);
            }
        });
        cx.notify();
    }

    fn selected_process(&self) -> Option<&ProcessInfo> {
        let details = self.details.as_ref()?;
        Some(details.process(&self.snapshot.as_ref()?.processes))
    }

    pub(super) fn process_exited(&self) -> bool {
        self.details.as_ref().is_some_and(|details| {
            self.snapshot
                .as_ref()
                .is_none_or(|snapshot| details.exited(&snapshot.processes))
        })
    }

    fn environment_enabled(&self) -> bool {
        self.details.as_ref().is_some_and(|details| {
            self.snapshot
                .as_ref()
                .is_some_and(|snapshot| details.can_load_environment(&snapshot.processes))
        })
    }

    pub(super) fn fetch_environment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.process_exited() {
            return;
        }
        let Some(core) = self.core.clone() else {
            return;
        };
        let Some(details) = self.details.as_mut() else {
            return;
        };
        if matches!(
            details.environment,
            Environment::Loaded(_) | Environment::Loading
        ) {
            details.environment_open = !details.environment_open;
            if details.environment_open {
                details.body_scroll.scroll_to_bottom();
            }
            cx.notify();
            return;
        }
        details.environment_open = true;
        details.body_scroll.scroll_to_bottom();
        let key = details.key;
        details.environment = Environment::Loading;
        cx.notify();
        cx.spawn_in(window, async move |entity, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { core.process_environ_if_matches(key.pid, key.start_time) })
                .await;
            let _ = entity.update_in(cx, |this, _, cx| {
                if let Some(details) = this.details.as_mut().filter(|details| details.key == key) {
                    details.body_scroll.scroll_to_bottom();
                    details.environment = match result {
                        Ok(values) => Environment::Loaded(values),
                        Err(error) => Environment::Failed(error),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn kill(
        &mut self,
        rendered_for: ProcessKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.process_exited() {
            return;
        }
        let Some(core) = self.core.clone() else {
            return;
        };
        let Some(details) = self.details.as_mut() else {
            return;
        };
        let Some(key) = details.take_kill_confirmation(rendered_for) else {
            return;
        };
        cx.notify();
        cx.spawn_in(window, async move |entity, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { core.kill_process_if_matches(key.pid, key.start_time) })
                .await;
            let _ = entity.update_in(cx, |this, _, cx| {
                this.status = match result {
                    Ok(true) => {
                        this.pending_kill = Some(key);
                        format!("Kill signal sent to PID {}. Waiting for refresh.", key.pid)
                    }
                    Ok(false) => format!(
                        "Could not kill PID {}. It exited, changed, or permission was denied.",
                        key.pid
                    ),
                    Err(error) => format!("Kill failed: {error}"),
                };
                if let Some(details) = this.details.as_mut().filter(|details| details.key == key) {
                    details.kill = KillState::Ready;
                }
                this.wake();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn confirmation(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let t = self.draft.unwrap_or(self.settings.base()).tokens();
        let d = self.details.as_ref()?;
        if d.kill != KillState::Confirming {
            return None;
        }
        let key = d.key;
        Some(
            div()
                .id("kill-confirmation")
                .role(Role::AlertDialog)
                .flex()
                .items_center()
                .gap_3()
                .p(px(8.))
                .bg(rgb(theme::MANTLE))
                .border_1()
                .border_color(rgb(theme::RED))
                .child(format!(
                    "Kill {} · PID {} · started {}?",
                    self.selected_process()
                        .map(|p| p.name.as_str())
                        .unwrap_or("exited process"),
                    key.pid,
                    crate::rack::timestamp(key.start_time, "%Y-%m-%d %H:%M:%S")
                ))
                .child(
                    Button::new("confirm-kill")
                        .label("Kill · Y")
                        .rack(&t)
                        .danger()
                        .small()
                        .on_click(cx.listener(move |this, _, w, cx| this.kill(key, w, cx))),
                )
                .child(
                    Button::new("cancel-kill")
                        .label("Keep · N")
                        .rack(&t)
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(d) = this.details.as_mut() {
                                d.cancel_kill(key);
                            }
                            cx.notify();
                        })),
                ),
        )
    }
    pub(super) fn details(&self, cx: &mut Context<Self>) -> Option<Div> {
        let details = self.details.as_ref()?;
        let key = details.key;
        let t = self.draft.unwrap_or(self.settings.base()).tokens();
        let mut frame = div()
            .relative()
            .child(crate::rack::screws(&t))
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(px(380.))
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .bg(rgb(t.base.plate))
            .border_1()
            .border_color(rgb(t.rule_hi));
        let mut panel = div().flex().flex_col().flex_shrink_0();
        if let Some(p) = self.selected_process() {
            if self.process_exited() {
                frame = frame.child(crate::rack::label(details.exit_marker(), &t));
            }
            frame = frame.child(
                div()
                    .flex_shrink_0()
                    .p(px(18.))
                    .border_b_1()
                    .border_color(rgb(t.rule))
                    .child(crate::rack::label(
                        format!("PID {} · {} · CLICK ANOTHER ROW TO SWITCH", p.pid, p.user),
                        &t,
                    ))
                    .child(
                        div()
                            .font_family("Archivo Display")
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .text_size(px(30.))
                            .child(p.name.to_uppercase()),
                    ),
            );
            let trace = self.traces.get(&key);
            let top = self.snapshot.as_ref().unwrap().stats.cpu_usage.len() as f32 * 50.;
            let trace_now = if self.process_exited() {
                trace
                    .and_then(|points| points.back().map(|(time, _)| *time))
                    .unwrap_or_else(crate::rack::now)
            } else {
                crate::rack::now()
            };
            let bars = div()
                .h(px(64.))
                .flex()
                .items_end()
                .gap(px(1.))
                .children((0..60).map(|i| {
                    let at = trace_now - 90. + i as f64 * 1.5;
                    let v = trace
                        .and_then(|t| {
                            t.iter()
                                .rev()
                                .find(|(time, _)| *time <= at + 1.5 && *time > at - 3.)
                        })
                        .map(|(_, v)| *v);
                    div()
                        .flex_1()
                        .h(px(v
                            .map(|v| (v / top.max(1.) * 64.).clamp(1., 64.))
                            .unwrap_or(1.)))
                        .bg(rgb(v
                            .map(|v| if v > 400. { t.base.hot } else { t.base.led })
                            .unwrap_or(t.led_off)))
                }));
            panel = panel.child(
                div().p(px(18.)).child(
                    crate::rack::win(&t)
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .child(crate::rack::label("CPU · LAST 90 S", &t))
                                .child(crate::rack::lit_unit(
                                    format!("{:3.0}", p.cpu_usage),
                                    '%',
                                    22.,
                                    p.cpu_usage > 400.,
                                    &t,
                                )),
                        )
                        .child(bars),
                ),
            );
            panel = panel.child(analysis_panel(
                details.analysis.as_ref(),
                &details.name,
                self.process_exited().then_some(p.cpu_usage),
                self.snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.stats.cpu_usage.len())
                    .unwrap_or(1),
                &t,
            ));
            panel = panel.child(
                div()
                    .p(px(18.))
                    .border_b_1()
                    .border_color(rgb(t.rule))
                    .child(
                        crate::rack::win(&t)
                            .child(
                                div()
                                    .flex()
                                    .gap(px(10.))
                                    .child(detail_readout("MEMORY", bytes(p.memory_usage), &t))
                                    .child(detail_readout(
                                        "GPU",
                                        p.gpu_usage
                                            .map(|v| format!("{v:.1}%"))
                                            .unwrap_or_else(|| "N/A".into()),
                                        &t,
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(10.))
                                    .child(detail_readout(
                                        "DISK READ",
                                        format!(
                                            "{:.1} MB/s",
                                            p.disk_usage.0 as f64
                                                / p.disk_sample_seconds
                                                    .unwrap_or(self.sample_seconds)
                                                / 1e6
                                        ),
                                        &t,
                                    ))
                                    .child(detail_readout(
                                        "DISK WRITE",
                                        format!(
                                            "{:.1} MB/s",
                                            p.disk_usage.1 as f64
                                                / p.disk_sample_seconds
                                                    .unwrap_or(self.sample_seconds)
                                                / 1e6
                                        ),
                                        &t,
                                    )),
                            ),
                    )
                    .child(detail_line(
                        "Ports",
                        if p.ports.is_empty() {
                            "none".into()
                        } else {
                            p.ports
                                .iter()
                                .map(u16::to_string)
                                .collect::<Vec<_>>()
                                .join(" ")
                        },
                    ))
                    .child(detail_line(
                        "Started",
                        crate::rack::timestamp(p.start_time, "%x %X"),
                    ))
                    .child(detail_line("Running for", duration(p.run_time))),
            );
            panel = panel
                .child(detail_line("Executable", p.executable.clone()))
                .child(detail_line("Arguments", p.argv.join(" ")));
            panel = panel.child(
                div()
                    .p(px(18.))
                    .border_b_1()
                    .border_color(rgb(t.rule))
                    .child(crate::rack::label("COMMAND", &t))
                    .child(
                        div()
                            .id("command-value")
                            .max_h(px(64.))
                            .overflow_y_scroll()
                            .p_2()
                            .bg(rgb(t.well))
                            .child(selectable("command-text", &p.command)),
                    ),
            );
        }
        let environment = match &details.environment {
            Environment::Unrequested => div(),
            Environment::Loading => div().child("Reading environment..."),
            Environment::Failed(e) => div().text_color(rgb(t.base.hot)).child(e.clone()),
            Environment::Loaded(values) if details.environment_open => {
                if values.is_empty() {
                    div().child("This process exposes no environment variables.")
                } else {
                    div().child(selectable("environment-text", &values.join("\n")))
                }
            }
            Environment::Loaded(_) => div().child("Environment hidden"),
        };
        panel = panel.child(
            div()
                .id("environment-scroll")
                .max_h(px(240.))
                .flex_shrink_0()
                .overflow_y_scroll()
                .p(px(18.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(crate::rack::label("ENVIRONMENT", &t))
                        .child(
                            Button::new("toggle-environment")
                                .label(if details.environment_open {
                                    "HIDE · E"
                                } else {
                                    "LOAD · E"
                                })
                                .rack(&t)
                                .small()
                                .disabled(!self.environment_enabled())
                                .on_click(
                                    cx.listener(|this, _, w, cx| this.fetch_environment(w, cx)),
                                ),
                        ),
                )
                .child(environment),
        );
        Some(
            frame.child(detail_body(panel, &details.body_scroll)).child(
                div()
                    .flex()
                    .gap_2()
                    .flex_shrink_0()
                    .p(px(18.))
                    .child(
                        Button::new("close-details")
                            .flex_1()
                            .label("Close · Esc")
                            .rack(&t)
                            .on_click(cx.listener(|this, _, _, cx| {
                                close_details(&mut this.details);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("kill")
                            .flex_1()
                            .label("Kill · X")
                            .rack(&t)
                            .danger()
                            .disabled(self.process_exited())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let alive = !this.process_exited();
                                if let Some(d) = this.details.as_mut() {
                                    d.request_kill(key, alive);
                                }
                                cx.notify();
                            })),
                    ),
            ),
        )
    }
}

fn detail_body(content: Div, scroll: &ScrollHandle) -> Stateful<Div> {
    div()
        .id("detail-body")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(scroll)
        .child(content)
}

fn analysis_sample(
    history: &crate::analysis::ProcessHistory,
    last_seen_cpu: Option<f32>,
) -> (&'static str, String) {
    match last_seen_cpu {
        Some(cpu) => (
            "LAST SEEN",
            crate::analysis::sample_sentence("Last seen", cpu, history.busy_average),
        ),
        None => ("NOW", history.now.clone()),
    }
}

fn analysis_panel(
    analysis: Option<&crate::analysis::ProcessAnalysis>,
    name: &str,
    last_seen_cpu: Option<f32>,
    cores: usize,
    t: &crate::palette::Tokens,
) -> Div {
    let mut panel = div().p(px(18.)).border_b_1().border_color(rgb(t.rule));
    let Some(analysis) = analysis else {
        return panel.child(crate::rack::win(t).child("Reading process analysis…"));
    };
    panel = panel.child(
        crate::rack::win(t)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(crate::rack::label("WHAT IT IS", t))
                    .child(crate::rack::label(analysis.identity.source, t)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(14.))
                            .text_color(rgb(t.base.silk))
                            .child(analysis.identity.what.clone()),
                    )
                    .child(crate::rack::label(analysis.identity.kind.to_uppercase(), t)),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(t.ink_row))
                    .child(analysis.identity.about.clone()),
            ),
    );
    let mut history_panel = crate::rack::win(t).child(
        div()
            .flex()
            .justify_between()
            .child(crate::rack::label("HISTORY · 24 H", t))
            .when(analysis.history.is_some(), |d| {
                d.child(crate::rack::label(format!("ALL {name} PROCESSES"), t))
            }),
    );
    if let Some(history) = &analysis.history {
        let (sample_label, sample_text) = analysis_sample(history, last_seen_cpu);
        history_panel =
            history_panel.child(div().flex().items_end().gap(px(1.)).h(px(40.)).children(
                history.columns.iter().map(|value| match value {
                    None => {
                        crate::rack::hatch(crate::palette::mix(t.base.led, t.base.ground, 0.14))
                    }
                    Some(value) => {
                        let height = (value / (cores.max(1) as f32 * 50.) * 40.).clamp(2., 40.);
                        div().flex_1().min_w_0().h_full().flex().items_end().child(
                            div().w_full().h(px(height)).bg(rgb(if *value > 400. {
                                t.base.hot
                            } else {
                                t.base.led
                            })),
                        )
                    }
                }),
            ));
        history_panel = history_panel.child(
            div()
                .flex()
                .justify_between()
                .child(crate::rack::label("24 H AGO", t))
                .child(crate::rack::label(sample_label, t)),
        );
        history_panel = history_panel.child(
            div()
                .text_size(px(11.))
                .text_color(rgb(t.ink_row))
                .child(format!("{} {}", history.pattern, sample_text)),
        );
        history_panel = history_panel
            .child(detail_line(
                "Core time",
                format!("{:.1} min", history.core_minutes),
            ))
            .child(detail_line(
                "Busy average",
                format!("{:.0} %", history.busy_average),
            ))
            .child(detail_line(
                "Peak CPU",
                format!(
                    "{:.0} % at {}",
                    history.peak_cpu,
                    crate::rack::timestamp(history.peak_minute * 60, "%H:%M")
                ),
            ))
            .child(detail_line(
                "Longest busy run",
                format!("{} min over 50 %", history.longest_busy_minutes),
            ))
            .child(detail_line("Peak memory", bytes(history.peak_memory)));
    } else {
        history_panel = history_panel.child(
            div()
                .text_size(px(12.))
                .child("Not seen in the recorded history. gtop records only while it runs."),
        );
    }
    panel.child(history_panel)
}

fn selectable(id: &'static str, text: &str) -> gpui_component::text::TextView {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    gpui_component::text::TextView::html(id, format!("<pre>{escaped}</pre>"))
        .selectable(true)
        .scrollable(false)
}
fn detail_readout(name: &str, value: String, t: &crate::palette::Tokens) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .child(crate::rack::label(name.to_string(), t))
        .child(if let Some(number) = value.strip_suffix('%') {
            crate::rack::lit_unit(number.to_string(), '%', 22., false, t)
        } else {
            crate::rack::lit(value, 22., false, t)
        })
}

fn detail_line(label: &str, value: String) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .gap_2()
        .py(px(3.))
        .child(
            div()
                .w(px(96.))
                .font_family("Archivo Labels")
                .text_size(px(10.))
                .child(label.to_uppercase()),
        )
        .child(div().flex_1().min_w_0().child(value))
}

#[cfg(test)]
mod tests {
    use super::{kill_completed, Details, Environment, KillState};
    use crate::model::ProcessKey;
    use gpui::ScrollHandle;

    fn selected(key: ProcessKey) -> Details {
        let process = crate::test_data::process(key.pid, "builder", 42.);
        let mut process = process;
        process.start_time = key.start_time;
        Details {
            key,
            last_known: process,
            last_seen: 1_800_000_000,
            name: "builder".into(),
            executable: "/bin/builder".into(),
            analysis: None,
            environment: Environment::Unrequested,
            environment_open: false,
            body_scroll: ScrollHandle::new(),
            kill: KillState::Ready,
        }
    }

    #[test]
    fn analysis_uses_last_sample_wording_after_exit() {
        let key = ProcessKey {
            pid: 1,
            start_time: 1,
        };
        let mut details = selected(key);
        let mut process = crate::test_data::process(1, "builder", 814.);
        process.start_time = 1;
        details.observe(&process, 6003);
        let mut snapshot = crate::test_data::snapshot();
        snapshot.processes =
            std::sync::Arc::new(vec![crate::test_data::process(1, "builder", 140.)]);
        let mut history = crate::history::History::default();
        history.record(&snapshot, 6000., 3.);
        history.record(&snapshot, 6003., 3.);
        let analysis = history.analyze_process("builder", "builder", 814., 100);
        let history = analysis.history.unwrap();
        let processes = vec![process];
        let live = super::analysis_sample(
            &history,
            details
                .exited(&processes)
                .then_some(details.process(&processes).cpu_usage),
        );
        assert_eq!(live.0, "NOW");
        assert_eq!(live.1, "Now 814 %, 5.8× its usual busy load.");
        let exited = super::analysis_sample(
            &history,
            details
                .exited(&[])
                .then_some(details.process(&[]).cpu_usage),
        );
        assert_eq!(exited.0, "LAST SEEN");
        assert_eq!(exited.1, "Last seen 814 %, 5.8× its usual busy load.");
        assert!(!exited.1.contains("Now"));
    }

    #[gpui::test]
    async fn detail_body_scrolls_to_lower_content(cx: &mut gpui::TestAppContext) {
        use gpui::{prelude::*, *};
        struct DetailLayoutTest(ScrollHandle);
        impl Render for DetailLayoutTest {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .w(px(380.))
                    .h(px(600.))
                    .flex()
                    .flex_col()
                    .child(div().h(px(80.)).flex_shrink_0())
                    .child(super::detail_body(
                        div().h(px(1200.)).flex_shrink_0(),
                        &self.0,
                    ))
                    .child(div().h(px(60.)).flex_shrink_0())
            }
        }
        let scroll = ScrollHandle::new();
        let window = cx.add_window(|_, _| DetailLayoutTest(scroll.clone()));
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert_eq!(scroll.bounds().size.height, px(460.));
        assert_eq!(scroll.offset().y, px(0.));
        scroll.scroll_to_bottom();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert_eq!(scroll.offset().y, px(-740.));
    }

    #[test]
    fn kill_completion_accepts_exit_zombie_and_reused_pid() {
        let mut snapshot = crate::test_data::snapshot();
        let key = ProcessKey::from(&snapshot.processes[0]);
        assert!(!kill_completed(key, &snapshot.processes));
        std::sync::Arc::make_mut(&mut snapshot.processes)[0].status = "Zombie".into();
        assert!(kill_completed(key, &snapshot.processes));
        std::sync::Arc::make_mut(&mut snapshot.processes)[0].status = "Running".into();
        std::sync::Arc::make_mut(&mut snapshot.processes)[0].start_time += 1;
        assert!(kill_completed(key, &snapshot.processes));
        std::sync::Arc::make_mut(&mut snapshot.processes).clear();
        assert!(kill_completed(key, &snapshot.processes));
    }

    #[test]
    fn detail_keeps_its_last_sample_after_exit_and_pid_reuse() {
        let mut process = crate::test_data::process(42, "builder", 42.);
        process.start_time = 100;
        process.command = "builder --release".into();
        process.argv = vec!["builder".into(), "--release".into()];
        process.ports = vec![8080];
        process.environ = vec!["MODE=release".into()];
        let key = ProcessKey::from(&process);
        let mut details = selected(key);
        details.observe(&process, 1_800_000_123);
        details.environment = Environment::Loaded(process.environ.clone());

        let mut replacement = process.clone();
        replacement.start_time = 200;
        replacement.name = "replacement".into();
        replacement.cpu_usage = 99.;

        assert_eq!(details.process(&[]).name, "builder");
        assert_eq!(details.process(&[]).command, "builder --release");
        assert_eq!(details.process(&[]).argv, ["builder", "--release"]);
        assert_eq!(details.process(&[]).ports, [8080]);
        assert_eq!(details.process(&[]).user, "test");
        assert!(matches!(
            &details.environment,
            Environment::Loaded(values) if values == &["MODE=release"]
        ));
        let replacement_snapshot = [replacement];
        assert_eq!(details.process(&replacement_snapshot).name, "builder");
        assert!(details.exited(&[]));
        assert!(details.exited(&replacement_snapshot));
        let marker = details.exit_marker();
        let time = marker.strip_prefix("EXITED · last seen ").unwrap();
        assert_eq!(time.len(), 8);
        assert_eq!(
            time.chars().filter(|character| *character == ':').count(),
            2
        );
        assert!(!details.can_load_environment(&[]));
        assert!(details.can_load_environment(&[process]));
    }

    #[test]
    fn matching_process_updates_the_last_known_values() {
        let key = ProcessKey {
            pid: 42,
            start_time: 100,
        };
        let mut details = selected(key);
        let mut latest = crate::test_data::process(key.pid, "builder", 88.);
        latest.start_time = key.start_time;
        latest.memory_usage = 4096;
        details.observe(&latest, 1_800_000_321);
        assert_eq!(details.process(&[]).cpu_usage, 88.);
        assert_eq!(details.process(&[]).memory_usage, 4096);
        assert_eq!(details.last_seen, 1_800_000_321);
    }

    #[test]
    fn close_clears_the_open_detail() {
        let mut details = Some(selected(ProcessKey {
            pid: 42,
            start_time: 100,
        }));
        super::close_details(&mut details);
        assert!(details.is_none());
    }

    #[test]
    fn pending_kill_confirmation_clears_on_exit_or_pid_reuse() {
        let key = ProcessKey {
            pid: 42,
            start_time: 100,
        };
        let mut details = selected(key);
        details.request_kill(key, true);
        details.cancel_kill_if_exited(&[details.last_known.clone()]);
        assert!(details.kill == KillState::Confirming);
        details.cancel_kill_if_exited(&[]);
        assert!(details.kill == KillState::Ready);
        details.request_kill(key, true);
        let mut replacement = details.last_known.clone();
        replacement.start_time += 1;
        details.cancel_kill_if_exited(&[replacement]);
        assert!(details.kill == KillState::Ready);
        assert!(details.take_kill_confirmation(key).is_none());
    }

    #[test]
    fn exited_process_cannot_request_kill() {
        let mut details = selected(ProcessKey {
            pid: 42,
            start_time: 100,
        });
        details.request_kill(details.key, false);
        assert!(matches!(details.kill, KillState::Ready));
    }

    #[test]
    fn confirmation_must_match_the_drawn_process() {
        let drawn = ProcessKey {
            pid: 42,
            start_time: 100,
        };
        for current in [
            ProcessKey {
                pid: 43,
                start_time: 100,
            },
            ProcessKey {
                pid: 42,
                start_time: 200,
            },
        ] {
            let mut details = selected(current);
            details.request_kill(current, true);
            assert_eq!(details.take_kill_confirmation(drawn), None);
        }
    }

    #[test]
    fn confirmation_is_required_and_can_only_be_consumed_once() {
        let key = ProcessKey {
            pid: 42,
            start_time: 100,
        };
        let mut details = selected(key);
        assert_eq!(details.take_kill_confirmation(key), None);
        details.request_kill(key, true);
        assert_eq!(details.take_kill_confirmation(key), Some(key));
        assert_eq!(details.take_kill_confirmation(key), None);
    }

    #[test]
    fn canceled_and_stale_requests_do_not_authorize_a_kill() {
        let key = ProcessKey {
            pid: 42,
            start_time: 100,
        };
        let stale = ProcessKey {
            pid: 43,
            start_time: 100,
        };
        let mut details = selected(key);
        details.request_kill(stale, true);
        assert_eq!(details.take_kill_confirmation(key), None);
        details.request_kill(key, true);
        details.cancel_kill(key);
        assert_eq!(details.take_kill_confirmation(key), None);
        details.request_kill(key, true);
        details.cancel_kill(stale);
        assert_eq!(details.take_kill_confirmation(key), Some(key));
    }
}
