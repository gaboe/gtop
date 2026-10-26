use crate::{
    column_widths::{self, Column, Drag, TableState, Widths},
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
pub struct WidthsChanged(pub Widths, pub bool);
impl EventEmitter<WidthsChanged> for Processes {}

impl From<ProcessColumn> for Column {
    fn from(column: ProcessColumn) -> Self {
        match column {
            ProcessColumn::Lamp => Self::Lamp,
            ProcessColumn::Name => Self::Name,
            ProcessColumn::Pid => Self::Pid,
            ProcessColumn::User => Self::User,
            ProcessColumn::Cpu => Self::Cpu,
            ProcessColumn::Memory => Self::Memory,
            ProcessColumn::Gpu => Self::Gpu,
            ProcessColumn::Ports => Self::Ports,
            ProcessColumn::Command => Self::Command,
        }
    }
}
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
    pub widths: Widths,
    sizing: TableState,
    resolved: Vec<(ProcessColumn, f32)>,
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
            widths: Widths::default(),
            sizing: TableState::default(),
            resolved: Vec::new(),
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
    fn computed_widths(&self) -> Vec<f32> {
        self.widths.compute(
            &self
                .columns()
                .into_iter()
                .map(Column::from)
                .collect::<Vec<_>>(),
            self.sizing.viewport,
        )
    }
    fn width(&self, column: ProcessColumn) -> f32 {
        self.resolved
            .iter()
            .find(|(kind, _)| *kind == column)
            .unwrap()
            .1
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
                .bg(rgb(if selected { t.selection } else { t.table }))
                .cursor_pointer()
                .children(self.resolved.iter().copied().map(|(kind, width)| {
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
                        .w(px(width))
                        .flex_shrink_0()
                        .px(px(10.))
                        .text_size(px(if priority { 14. } else { 12. }))
                        .when(priority && kind == ProcessColumn::Name, |d| {
                            d.font_weight(FontWeight::SEMIBOLD)
                        })
                        .text_color(rgb(color))
                        .whitespace_nowrap()
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
                        d.flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(rack::cpu_segments(
                                p.cpu_usage,
                                if priority { 18 } else { 14 },
                                if priority { 78. } else { 69. },
                                if priority { 12. } else { 8. },
                                t,
                            ))
                    })
                    .child(
                        if (selected || priority && culprit)
                            && !(culprit && kind == ProcessColumn::Command)
                        {
                            crate::glow::text(kind.text(p), false, t).into_any_element()
                        } else {
                            kind.text(p).into_any_element()
                        },
                    )
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
        self.resolved = columns.into_iter().zip(self.computed_widths()).collect();
        let header = div()
            .flex()
            .h(px(29.))
            .flex_shrink_0()
            .bg(rgb(t.table_head))
            .border_b_1()
            .border_color(rgb(t.rule_hi))
            .children(self.resolved.iter().copied().map(|(kind, width)| {
                let selected = self.column == kind;
                div()
                    .id(("header", kind as usize))
                    .role(Role::ColumnHeader)
                    .w(px(width))
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .relative()
                    .px(px(10.))
                    .py(px(7.))
                    .font_family("Archivo Labels")
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(10.))
                    .text_color(rgb(if selected { t.base.led } else { t.silk_dim }))
                    .cursor_pointer()
                    .child(div().overflow_hidden().child(crate::rack::tracked(format!(
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
                    ))))
                    .when(kind != ProcessColumn::Lamp, |d| {
                        d.child(column_widths::handle().on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                let column = Column::from(kind);
                                if event.click_count == 2 {
                                    this.sizing.drag = None;
                                    this.widths.reset(column);
                                    cx.emit(WidthsChanged(this.widths.clone(), true));
                                } else {
                                    this.sizing.drag = Some(Drag {
                                        column,
                                        start_x: event.position.x.into(),
                                        start_width: this.width(kind),
                                    });
                                }
                                cx.notify();
                            }),
                        ))
                    })
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
        let total = self.resolved.iter().map(|(_, width)| width).sum::<f32>() + 2.;
        let events = column_widths::events(
            cx,
            self.sizing.drag.is_some(),
            |this, width, _, cx| {
                let width = (*width - 2.).max(0.);
                if (this.sizing.viewport - width).abs() > 0.5 {
                    this.sizing.viewport = width;
                    cx.notify();
                }
            },
            |this, event, _, cx| {
                if let Some(drag) = this.sizing.drag {
                    this.widths.set(
                        drag.column,
                        drag.start_width + f32::from(event.position.x) - drag.start_x,
                    );
                    cx.emit(WidthsChanged(this.widths.clone(), false));
                    cx.stop_propagation();
                    cx.notify();
                }
            },
            |this, _, _, cx| {
                if this.sizing.drag.take().is_some() {
                    cx.emit(WidthsChanged(this.widths.clone(), true));
                    cx.stop_propagation();
                    cx.notify();
                }
            },
        );
        div()
            .id("process-table-viewport")
            .relative()
            .size_full()
            .min_w_0()
            .overflow_x_scroll()
            .child(events)
            .child(
                div()
                    .id("process-table")
                    .role(Role::Table)
                    .flex()
                    .flex_col()
                    .w(px(total))
                    .h_full()
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgb(t.well_rule))
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
                    }),
            )
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
    #[gpui::test]
    async fn name_handle_resizes_and_resets_without_sort_or_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{point, px, Modifiers, MouseButton, MouseDownEvent};
        let (table, cx) = cx.add_window_view(|_, _| {
            let mut table = Processes {
                snapshot: Some(Rc::new(crate::test_data::snapshot())),
                ..Default::default()
            };
            table.reorder();
            table
        });
        cx.simulate_resize(gpui::size(px(1500.), px(600.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let (start_width, sort, order) = cx.update(|_, cx| {
            let table = table.read(cx);
            (
                table.width(ProcessColumn::Name),
                table.column,
                table.order.clone(),
            )
        });
        let handle = point(px(22. + start_width - 2.), px(15.));
        cx.simulate_mouse_down(handle, MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let moved = point(handle.x - px(40.), px(100.));
        cx.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|_, cx| {
            let table = table.read(cx);
            assert_eq!(table.width(ProcessColumn::Name), start_width - 40.);
            assert_eq!(table.column, sort);
            assert_eq!(table.order, order);
            assert!(table.selected.is_none());
            assert!(table.sizing.drag.is_none());
        });
        let handle = point(handle.x - px(40.), px(15.));
        cx.simulate_event(MouseDownEvent {
            position: handle,
            button: MouseButton::Left,
            click_count: 2,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        cx.simulate_mouse_up(handle, MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|_, cx| {
            let table = table.read(cx);
            assert_eq!(table.width(ProcessColumn::Name), start_width);
            assert_eq!(table.column, sort);
            assert!(table.selected.is_none());
        });
    }

    #[gpui::test]
    async fn quick_handle_release_ends_drag_before_repaint(cx: &mut gpui::TestAppContext) {
        use gpui::{
            point, px, InputEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
            MouseUpEvent,
        };
        let (table, cx) = cx.add_window_view(|_, _| Processes::default());
        cx.simulate_resize(gpui::size(px(1500.), px(600.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let x = cx.update(|_, cx| 22. + table.read(cx).width(ProcessColumn::Name) - 2.);
        let position = point(px(x), px(15.));
        cx.update(|window, cx| {
            window.dispatch_event(
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
            window.dispatch_event(
                MouseUpEvent {
                    position,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                }
                .to_platform_input(),
                cx,
            );
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let before = cx.update(|_, cx| table.read(cx).width(ProcessColumn::Name));
        cx.simulate_event(MouseMoveEvent {
            position: point(position.x - px(40.), px(100.)),
            pressed_button: None,
            modifiers: Modifiers::default(),
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|_, cx| {
            let table = table.read(cx);
            assert!(table.sizing.drag.is_none());
            assert_eq!(table.width(ProcessColumn::Name), before);
        });
    }

    #[gpui::test]
    async fn quick_drag_before_repaint_resizes_and_ends_drag(cx: &mut gpui::TestAppContext) {
        use gpui::{
            point, px, InputEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
            MouseUpEvent,
        };
        let (table, cx) = cx.add_window_view(|_, _| Processes::default());
        cx.simulate_resize(gpui::size(px(1500.), px(600.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let (x, before) = cx.update(|_, cx| {
            let table = table.read(cx);
            (
                22. + table.width(ProcessColumn::Name) - 2.,
                table.width(ProcessColumn::Name),
            )
        });
        let start = point(px(x), px(15.));
        let end = point(px(x - 40.), px(100.));
        cx.update(|window, cx| {
            window.dispatch_event(
                MouseDownEvent {
                    position: start,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                MouseMoveEvent {
                    position: end,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                MouseUpEvent {
                    position: end,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                }
                .to_platform_input(),
                cx,
            );
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|_, cx| {
            let table = table.read(cx);
            assert_eq!(table.width(ProcessColumn::Name), before - 40.);
            assert!(table.sizing.drag.is_none());
        });
    }

    #[gpui::test]
    async fn unpressed_mouse_move_does_not_resize_a_drag(cx: &mut gpui::TestAppContext) {
        use gpui::{point, px, Modifiers, MouseButton, MouseMoveEvent};
        let (table, cx) = cx.add_window_view(|_, _| Processes::default());
        cx.simulate_resize(gpui::size(px(1500.), px(600.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let (x, before) = cx.update(|_, cx| {
            let table = table.read(cx);
            (
                22. + table.width(ProcessColumn::Name) - 2.,
                table.width(ProcessColumn::Name),
            )
        });
        cx.simulate_mouse_down(
            point(px(x), px(15.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_event(MouseMoveEvent {
            position: point(px(x - 40.), px(100.)),
            pressed_button: None,
            modifiers: Modifiers::default(),
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|_, cx| {
            let table = table.read(cx);
            assert!(table.sizing.drag.is_some());
            assert_eq!(table.width(ProcessColumn::Name), before);
        });
        cx.simulate_mouse_up(
            point(px(x - 40.), px(100.)),
            MouseButton::Left,
            Modifiers::default(),
        );
    }

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
