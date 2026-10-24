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
    pub(super) name: String,
    pub(super) command: String,
    pub(super) analysis: Option<crate::analysis::ProcessAnalysis>,
    environment: Environment,
    environment_open: bool,
    pub(super) kill: KillState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum KillState {
    Ready,
    Confirming,
    Sending,
}

impl Details {
    pub(super) fn request_kill(&mut self, rendered_for: ProcessKey) {
        if self.key == rendered_for && self.kill == KillState::Ready {
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
        let (name, command) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .processes
                    .iter()
                    .find(|process| ProcessKey::from(*process) == key)
                    .map(|process| (process.name.clone(), process.command.clone()))
            })
            .unwrap_or_default();
        self.details = Some(Details {
            key,
            name,
            command,
            analysis: None,
            environment: Environment::Unrequested,
            environment_open: false,
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
        let key = self.details.as_ref()?.key;
        self.snapshot
            .as_ref()?
            .processes
            .iter()
            .find(|process| ProcessKey::from(*process) == key)
    }

    pub(super) fn fetch_environment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            cx.notify();
            return;
        }
        details.environment_open = true;
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
                    key.start_time
                ))
                .child(
                    Button::new("confirm-kill")
                        .label("Kill · Y")
                        .danger()
                        .small()
                        .on_click(cx.listener(move |this, _, w, cx| this.kill(key, w, cx))),
                )
                .child(
                    Button::new("cancel-kill")
                        .label("Keep · N")
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
        let mut panel = div()
            .relative()
            .child(crate::rack::screws(&t))
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(px(380.))
            .h_full()
            .bg(rgb(t.base.plate))
            .border_1()
            .border_color(rgb(t.rule_hi));
        if let Some(p) = self.selected_process() {
            panel = panel.child(
                div()
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
            let bars = div()
                .h(px(64.))
                .flex()
                .items_end()
                .gap(px(1.))
                .children((0..60).map(|i| {
                    let at = crate::rack::now() - 90. + i as f64 * 1.5;
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
                            .child(detail_line("Memory", bytes(p.memory_usage)))
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .child(crate::rack::label("GPU", &t))
                                    .child(
                                        p.gpu_usage
                                            .map(|v| {
                                                crate::rack::lit_unit(
                                                    format!("{v:4.1}"),
                                                    '%',
                                                    14.,
                                                    false,
                                                    &t,
                                                )
                                            })
                                            .unwrap_or_else(|| {
                                                crate::rack::lit("N/A".into(), 14., false, &t)
                                            }),
                                    ),
                            )
                            .child(detail_line(
                                "Disk read",
                                format!(
                                    "{:.1} MB/s",
                                    p.disk_usage.0 as f64 / self.sample_seconds / 1e6
                                ),
                            ))
                            .child(detail_line(
                                "Disk write",
                                format!(
                                    "{:.1} MB/s",
                                    p.disk_usage.1 as f64 / self.sample_seconds / 1e6
                                ),
                            )),
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
            panel = panel.child(
                div()
                    .p(px(18.))
                    .border_b_1()
                    .border_color(rgb(t.rule))
                    .child(crate::rack::label("COMMAND", &t))
                    .child(
                        div()
                            .id("command-value")
                            .overflow_x_scroll()
                            .p_2()
                            .bg(rgb(t.well))
                            .child(selectable("command-text", &p.command)),
                    ),
            );
        } else {
            panel = panel
                .child(format!("PID {} has exited", key.pid))
                .child(analysis_panel(
                    details.analysis.as_ref(),
                    &details.name,
                    self.snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.stats.cpu_usage.len())
                        .unwrap_or(1),
                    &t,
                ));
        }
        let environment = match &details.environment {
            Environment::Unrequested => div().child(
                Button::new("environment")
                    .label("Load · E")
                    .small()
                    .on_click(cx.listener(|this, _, w, cx| this.fetch_environment(w, cx))),
            ),
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
                .min_h_0()
                .overflow_y_scroll()
                .p(px(18.))
                .child(crate::rack::label("ENVIRONMENT", &t))
                .when(
                    !matches!(details.environment, Environment::Unrequested),
                    |d| {
                        d.child(
                            Button::new("toggle-environment")
                                .label(if details.environment_open {
                                    "Hide · E"
                                } else {
                                    "Load · E"
                                })
                                .small()
                                .on_click(
                                    cx.listener(|this, _, w, cx| this.fetch_environment(w, cx)),
                                ),
                        )
                    },
                )
                .child(environment),
        );
        Some(
            panel.child(div().flex_1()).child(
                div()
                    .flex()
                    .gap_2()
                    .p(px(18.))
                    .child(
                        Button::new("close-details")
                            .label("Close · Esc")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.details = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("kill")
                            .label("Kill · X")
                            .danger()
                            .disabled(self.selected_process().is_none())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(d) = this.details.as_mut() {
                                    d.request_kill(key);
                                }
                                cx.notify();
                            })),
                    ),
            ),
        )
    }
}

fn analysis_panel(
    analysis: Option<&crate::analysis::ProcessAnalysis>,
    name: &str,
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
                .child(crate::rack::label("NOW", t)),
        );
        history_panel = history_panel.child(
            div()
                .text_size(px(11.))
                .text_color(rgb(t.ink_row))
                .child(format!("{} {}", history.pattern, history.now)),
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
    use super::{Details, Environment, KillState};
    use crate::model::ProcessKey;

    fn selected(key: ProcessKey) -> Details {
        Details {
            key,
            name: "builder".into(),
            command: "builder".into(),
            analysis: None,
            environment: Environment::Unrequested,
            environment_open: false,
            kill: KillState::Ready,
        }
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
            details.request_kill(current);
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
        details.request_kill(key);
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
        details.request_kill(stale);
        assert_eq!(details.take_kill_confirmation(key), None);
        details.request_kill(key);
        details.cancel_kill(key);
        assert_eq!(details.take_kill_confirmation(key), None);
        details.request_kill(key);
        details.cancel_kill(stale);
        assert_eq!(details.take_kill_confirmation(key), Some(key));
    }
}
