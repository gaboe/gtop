use crate::{
    model::{self, bytes, duration, ProcessKey},
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
    environment: Environment,
    kill: KillState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KillState {
    Ready,
    Confirming,
    Sending,
}

impl Details {
    fn request_kill(&mut self, rendered_for: ProcessKey) {
        if self.key == rendered_for && self.kill == KillState::Ready {
            self.kill = KillState::Confirming;
        }
    }

    fn cancel_kill(&mut self, rendered_for: ProcessKey) {
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
        if self
            .details
            .as_ref()
            .is_some_and(|details| details.key == key)
        {
            return;
        }
        self.details = Some(Details {
            key,
            environment: Environment::Unrequested,
            kill: KillState::Ready,
        });
        self.table.update(cx, |table, cx| {
            table.delegate_mut().selected = Some(key);
            if let Some(row) = table.delegate().row_for(key) {
                table.set_selected_row(row, cx);
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

    fn fetch_environment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(core) = self.core.clone() else {
            return;
        };
        let Some(details) = self.details.as_mut() else {
            return;
        };
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

    fn kill(&mut self, rendered_for: ProcessKey, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn details(&self, cx: &mut Context<Self>) -> Option<Div> {
        let details = self.details.as_ref()?;
        let rendered_for = details.key;
        let heading = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(
                        self.selected_process()
                            .map(|process| format!("{}  ·  PID {}", process.name, process.pid))
                            .unwrap_or_else(|| format!("PID {} has exited", details.key.pid)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("kill")
                            .label(if details.kill == KillState::Sending {
                                "Sending…"
                            } else {
                                "Kill process"
                            })
                            .danger()
                            .small()
                            .disabled(
                                self.selected_process().is_none()
                                    || details.kill == KillState::Sending,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(details) = this.details.as_mut() {
                                    details.request_kill(rendered_for);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("close-details")
                            .label("Close details")
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.details = None;
                                this.table.update(cx, |table, cx| {
                                    table.delegate_mut().selected = None;
                                    table.clear_selection(cx);
                                });
                                cx.notify();
                            })),
                    ),
            );
        let mut content = div()
            .id("details-scroll")
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2();
        if let Some(process) = self.selected_process() {
            let command = process.command.clone();
            content = content
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_start()
                        .gap_3()
                        .child(
                            div()
                                .w(px(80.))
                                .flex_shrink_0()
                                .text_color(rgb(theme::SUBTEXT))
                                .child("Command"),
                        )
                        .child(
                            div()
                                .id("command-value")
                                .flex_1()
                                .min_w_0()
                                .overflow_x_scroll()
                                .child(command.clone()),
                        )
                        .child(Button::new("copy-command").label("Copy").small().on_click(
                            move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(command.clone()))
                            },
                        )),
                )
                .child(detail_line(
                    "Root",
                    if process.root.is_empty() {
                        "Unavailable".into()
                    } else {
                        process.root.clone()
                    },
                ))
                .child(detail_line(
                    "Resources",
                    format!(
                        "CPU {:.1}%   GPU {}   RAM {}   VIRT {}   Run time {}",
                        process.cpu_usage,
                        model::ProcessColumn::Gpu.text(process),
                        bytes(process.memory_usage),
                        bytes(process.virtual_memory),
                        duration(process.run_time)
                    ),
                ))
                .child(detail_line(
                    "Parent",
                    format!(
                        "{}   User {}   Status {}",
                        process.ppid, process.user, process.status
                    ),
                ));
            let children: Vec<_> = self
                .snapshot
                .as_ref()
                .unwrap()
                .processes
                .iter()
                .filter(|child| child.ppid == process.pid)
                .collect();
            content = content.child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(80.))
                            .text_color(rgb(theme::SUBTEXT))
                            .child("Children"),
                    )
                    .when(children.is_empty(), |this| this.child("None"))
                    .children(children.into_iter().map(|child| {
                        let key = ProcessKey::from(child);
                        Button::new(("child", child.pid as usize))
                            .label(format!("{} ({})", child.name, child.pid))
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| this.select(key, cx)))
                    })),
            );
        }
        let environment = match &details.environment {
            Environment::Unrequested => div().child(
                Button::new("environment")
                    .label("Load environment variables")
                    .small()
                    .disabled(self.selected_process().is_none())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.fetch_environment(window, cx)),
                    ),
            ),
            Environment::Loading => div()
                .text_color(rgb(theme::SUBTEXT))
                .child("Reading environment variables…"),
            Environment::Failed(error) => div()
                .text_color(rgb(theme::RED))
                .child(format!("Environment read failed: {error}")),
            Environment::Loaded(values) => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(format!("Environment variables ({})", values.len()))
                        .child(
                            Button::new("copy-environment")
                                .label("Copy")
                                .small()
                                .on_click({
                                    let text = values.join("\n");
                                    move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            text.clone(),
                                        ))
                                    }
                                }),
                        ),
                )
                .when(values.is_empty(), |this| {
                    this.child("No variables are available. The process may deny access.")
                })
                .children(values.iter().map(|value| {
                    div()
                        .text_size(px(12.))
                        .font_family("Menlo")
                        .child(value.clone())
                })),
        };
        content = content.child(environment.flex_shrink_0());
        Some(
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .h(px(275.))
                .gap_3()
                .p_3()
                .bg(rgb(theme::MANTLE))
                .border_t_1()
                .border_color(rgb(theme::SURFACE))
                .child(heading)
                .when(details.kill == KillState::Confirming, |this| {
                    this.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(div().text_color(rgb(theme::RED)).child(format!(
                                "Kill PID {}? Unsaved work can be lost.",
                                details.key.pid
                            )))
                            .child(
                                Button::new("confirm-kill")
                                    .label(format!("Kill PID {}", rendered_for.pid))
                                    .danger()
                                    .small()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.kill(rendered_for, window, cx)
                                    })),
                            )
                            .child(Button::new("cancel-kill").label("Cancel").small().on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(details) = this.details.as_mut() {
                                        details.cancel_kill(rendered_for);
                                    }
                                    cx.notify();
                                }),
                            )),
                    )
                })
                .child(content),
        )
    }
}

fn detail_line(label: &str, value: String) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .gap_3()
        .child(
            div()
                .w(px(80.))
                .flex_shrink_0()
                .text_color(rgb(theme::SUBTEXT))
                .child(label.to_string()),
        )
        .child(div().flex_1().child(value))
}

#[cfg(test)]
mod tests {
    use super::{Details, Environment, KillState};
    use crate::model::ProcessKey;

    fn selected(key: ProcessKey) -> Details {
        Details {
            key,
            environment: Environment::Unrequested,
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
