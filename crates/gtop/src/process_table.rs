use crate::{
    model::{process_order, ProcessColumn, ProcessKey, Search},
    palette::{presets, Base},
    rack,
};
use gpui::{prelude::*, *};
use gtop_core::{ProcessInfo, Snapshot};
use std::rc::Rc;

const TOP_ROWS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowTier {
    Priority,
    Remaining,
}

fn row_tier(row: usize) -> RowTier {
    if row < TOP_ROWS {
        RowTier::Priority
    } else {
        RowTier::Remaining
    }
}

pub struct SelectProcess(pub ProcessKey);
pub struct Processes {
    pub snapshot: Option<Rc<Snapshot>>,
    pub order: Vec<usize>,
    pub search: Search,
    pub selected: Option<ProcessKey>,
    pub column: ProcessColumn,
    pub descending: bool,
    pub base: Base,
    pub compact: bool,
    pub root_focus: Option<FocusHandle>,
    scroll: UniformListScrollHandle,
}
impl EventEmitter<SelectProcess> for Processes {}
impl Default for Processes {
    fn default() -> Self {
        Self {
            snapshot: None,
            order: vec![],
            search: Search::default(),
            selected: None,
            column: ProcessColumn::Cpu,
            descending: true,
            base: presets()[0].base,
            compact: false,
            root_focus: None,
            scroll: UniformListScrollHandle::new(),
        }
    }
}
impl Processes {
    pub fn reorder(&mut self) {
        self.order = self
            .snapshot
            .as_ref()
            .map(|s| process_order(&s.processes, &self.search, self.column, self.descending))
            .unwrap_or_default();
    }
    pub fn process(&self, row: usize) -> Option<&ProcessInfo> {
        self.snapshot.as_ref()?.processes.get(*self.order.get(row)?)
    }
    pub fn row_for(&self, key: ProcessKey) -> Option<usize> {
        self.order.iter().position(|index| {
            self.snapshot
                .as_ref()
                .is_some_and(|s| ProcessKey::from(&s.processes[*index]) == key)
        })
    }
    pub fn navigate(&mut self, row: usize) {
        if let Some(p) = self.process(row) {
            self.selected = Some(ProcessKey::from(p));
            if row >= TOP_ROWS {
                self.scroll
                    .scroll_to_item(row - TOP_ROWS, ScrollStrategy::Top);
            }
        }
    }
    fn columns(&self) -> Vec<ProcessColumn> {
        ProcessColumn::ALL
            .into_iter()
            .filter(|c| !self.compact || !matches!(c, ProcessColumn::User | ProcessColumn::Command))
            .collect()
    }
    fn width(&self, c: ProcessColumn) -> f32 {
        match c {
            ProcessColumn::Lamp => 22.,
            ProcessColumn::Name => {
                if self.compact {
                    150.
                } else {
                    225.
                }
            }
            ProcessColumn::Pid => 70.,
            ProcessColumn::User => 85.,
            ProcessColumn::Cpu => 160.,
            ProcessColumn::Memory => 100.,
            ProcessColumn::Gpu => 65.,
            ProcessColumn::Ports => 90.,
            ProcessColumn::Command => 320.,
        }
    }
    pub fn sort(&mut self, column: ProcessColumn, toggle: bool) {
        self.descending = if toggle && self.column == column {
            !self.descending
        } else {
            column != ProcessColumn::Pid && column != ProcessColumn::Name
        };
        self.column = column;
        self.reorder();
    }

    fn row(
        &self,
        row: usize,
        t: &crate::palette::Tokens,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let p = self.process(row)?;
        let key = ProcessKey::from(p);
        let priority = row_tier(row) == RowTier::Priority;
        let culprit = p.cpu_usage > 100. && row < 4;
        let selected = self.selected == Some(key);
        let cores = self.snapshot.as_ref()?.stats.cpu_usage.len().max(1);
        let height = if priority { 30. } else { 22. };
        Some(
            div()
                .id(("process", row))
                .role(Role::Row)
                .aria_label(format!(
                    "{} PID {} CPU {:.1}% memory {} ports {}",
                    p.name,
                    p.pid,
                    p.cpu_usage,
                    crate::model::bytes(p.memory_usage),
                    ProcessColumn::Ports.text(p)
                ))
                .h(px(height))
                .flex()
                .items_center()
                .border_b_1()
                .border_color(rgb(t.row_rule))
                .bg(rgb(if selected {
                    crate::palette::mix(t.plate2, t.base.led, 0.7)
                } else {
                    crate::palette::mix(t.base.ground, 0, 0.8)
                }))
                .cursor_pointer()
                .children(self.columns().into_iter().map(|kind| {
                    let color = if selected || (priority && culprit) {
                        t.base.led
                    } else if !priority {
                        t.silk_dim
                    } else if kind == ProcessColumn::Ports {
                        t.base.ok
                    } else if kind == ProcessColumn::Command {
                        t.cmd
                    } else {
                        t.ink_row
                    };
                    let cell = div()
                        .w(px(self.width(kind)))
                        .flex_shrink_0()
                        .when(kind == ProcessColumn::Command, |d| {
                            d.flex_1().min_w_0().flex_shrink(1.)
                        })
                        .px(px(10.))
                        .text_size(px(if priority { 14. } else { 12. }))
                        .when(priority && kind == ProcessColumn::Name, |d| {
                            d.font_weight(FontWeight::SEMIBOLD)
                        })
                        .text_color(rgb(color))
                        .truncate();
                    if kind == ProcessColumn::Lamp {
                        return cell.child(div().w(px(7.)).h(px(12.)).bg(rgb(
                            if p.cpu_usage > 400. {
                                if now_blink() {
                                    t.base.hot
                                } else {
                                    t.hot_off
                                }
                            } else if culprit {
                                t.base.led
                            } else {
                                t.seg_off
                            },
                        )));
                    }
                    cell.when(kind == ProcessColumn::Cpu, |d| {
                        d.flex().items_center().justify_end().gap_2().child(
                            rack::segments(
                                p.cpu_usage / cores as f32,
                                if priority { 18 } else { 14 },
                                t,
                            )
                            .w(px(if priority { 78. } else { 69. }))
                            .h(px(if priority { 12. } else { 8. })),
                        )
                    })
                    .child(kind.text(p))
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(key);
                    cx.emit(SelectProcess(key));
                    cx.notify();
                })),
        )
    }
}
impl Render for Processes {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.base.tokens();
        let columns = self.columns();
        let header = div()
            .flex()
            .h(px(29.))
            .flex_shrink_0()
            .bg(rgb(crate::palette::mix(t.base.plate, 0, 0.6)))
            .border_b_1()
            .border_color(rgb(t.rule_hi))
            .children(columns.into_iter().map(|kind| {
                let selected = self.column == kind;
                div()
                    .id(("header", kind as usize))
                    .role(Role::ColumnHeader)
                    .w(px(self.width(kind)))
                    .flex_shrink_0()
                    .when(kind == ProcessColumn::Command, |d| {
                        d.flex_1().min_w_0().flex_shrink(1.)
                    })
                    .px(px(10.))
                    .py(px(7.))
                    .font_family("Archivo Labels")
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(10.))
                    .text_color(rgb(if selected { t.base.led } else { t.silk_dim }))
                    .cursor_pointer()
                    .child(crate::rack::tracked(format!(
                        "{}{}",
                        kind.label().to_uppercase(),
                        if selected {
                            if self.descending {
                                " ▼"
                            } else {
                                " ▲"
                            }
                        } else {
                            ""
                        }
                    )))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(focus) = &this.root_focus {
                            window.focus(focus, cx);
                        }
                        this.sort(kind, true);
                        cx.notify();
                    }))
            }));
        let top_count = self.order.len().min(TOP_ROWS);
        let has_remainder = self.order.len() > TOP_ROWS;
        let top_rows = div()
            .flex()
            .flex_col()
            .children((0..top_count).filter_map(|row| self.row(row, &t, cx)));
        let remainder = uniform_list(
            "process-rows-rest",
            self.order.len().saturating_sub(TOP_ROWS),
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                let t = this.base.tokens();
                range
                    .filter_map(|row| this.row(row + TOP_ROWS, &t, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .min_h_0();
        div()
            .id("process-table")
            .role(Role::Table)
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .border_1()
            .border_color(rgb(0))
            .child(header)
            .when(self.order.is_empty(), |d| {
                d.child(div().p_4().child(if self.snapshot.is_none() {
                    "Reading processes..."
                } else {
                    "No process matches. Esc clears the search."
                }))
            })
            .when(!self.order.is_empty(), |d| {
                d.child(top_rows)
                    .when(has_remainder, |d| {
                        d.child(div().h(px(1.)).w_full().bg(rgb(t.rule_hi)))
                    })
                    .child(remainder)
            })
    }
}
fn now_blink() -> bool {
    (rack::now() as u64).is_multiple_of(2)
}

#[cfg(test)]
mod tests {
    use super::{row_tier, Processes, RowTier, SelectProcess, TOP_ROWS};
    use crate::model::{ProcessColumn, ProcessKey, Search};
    use std::rc::Rc;
    #[test]
    fn sort_has_two_states_and_keeps_identity() {
        let s = crate::test_data::snapshot();
        let key = ProcessKey::from(&s.processes[2]);
        let mut p = Processes {
            snapshot: Some(Rc::new(s)),
            selected: Some(key),
            ..Default::default()
        };
        p.sort(ProcessColumn::Pid, false);
        assert!(!p.descending);
        assert_eq!(p.process(0).unwrap().pid, 1);
        p.sort(ProcessColumn::Pid, true);
        assert!(p.descending);
        assert_eq!(p.process(0).unwrap().pid, 4);
        p.sort(ProcessColumn::Pid, true);
        assert!(!p.descending);
        assert_eq!(p.selected, Some(key));
        p.search = Search::new("quiet");
        p.reorder();
        assert!(p.row_for(key).is_none());
        assert!(p
            .order
            .iter()
            .all(|i| ProcessKey::from(&p.snapshot.as_ref().unwrap().processes[*i]) != key));
    }
    #[test]
    fn rendered_identity_stays_bound_after_reorder() {
        let s = crate::test_data::snapshot();
        let mut p = Processes {
            snapshot: Some(Rc::new(s)),
            ..Default::default()
        };
        p.sort(ProcessColumn::Pid, false);
        let rendered = SelectProcess(ProcessKey::from(p.process(0).unwrap()));
        p.sort(ProcessColumn::Pid, true);
        assert_ne!(rendered.0, ProcessKey::from(p.process(0).unwrap()));
        assert_eq!(rendered.0.pid, 1);
    }

    #[test]
    fn classifies_the_current_order_top_ten() {
        assert_eq!(row_tier(0), RowTier::Priority);
        assert_eq!(row_tier(TOP_ROWS - 1), RowTier::Priority);
        assert_eq!(row_tier(TOP_ROWS), RowTier::Remaining);
    }
}
