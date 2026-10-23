use crate::{
    model::{process_order, ProcessColumn, ProcessKey, Search},
    theme,
};
use gpui::{prelude::*, *};
use gpui_component::table::{Column, ColumnSort, TableDelegate, TableState};
use gtop_core::{ProcessInfo, Snapshot};
use std::rc::Rc;

pub struct Processes {
    pub snapshot: Option<Rc<Snapshot>>,
    pub order: Vec<usize>,
    pub search: Search,
    pub selected: Option<ProcessKey>,
    column: ProcessColumn,
    descending: bool,
}

impl Default for Processes {
    fn default() -> Self {
        Self {
            snapshot: None,
            order: vec![],
            search: Search::default(),
            selected: None,
            column: ProcessColumn::Cpu,
            descending: true,
        }
    }
}

impl Processes {
    pub fn reorder(&mut self) {
        self.order = self
            .snapshot
            .as_ref()
            .map(|snapshot| {
                process_order(
                    &snapshot.processes,
                    &self.search,
                    self.column,
                    self.descending,
                )
            })
            .unwrap_or_default();
    }

    pub fn process(&self, row: usize) -> Option<&ProcessInfo> {
        self.snapshot.as_ref()?.processes.get(*self.order.get(row)?)
    }

    pub fn row_for(&self, key: ProcessKey) -> Option<usize> {
        self.order.iter().position(|index| {
            self.snapshot
                .as_ref()
                .is_some_and(|snapshot| ProcessKey::from(&snapshot.processes[*index]) == key)
        })
    }
}

impl TableDelegate for Processes {
    fn columns_count(&self, _: &App) -> usize {
        ProcessColumn::ALL.len()
    }
    fn rows_count(&self, _: &App) -> usize {
        self.order.len()
    }

    fn column(&self, index: usize, _: &App) -> Column {
        let kind = ProcessColumn::ALL[index];
        let width = match kind {
            ProcessColumn::Name => 250.,
            ProcessColumn::Pid => 76.,
            ProcessColumn::Status => 100.,
            ProcessColumn::User => 72.,
            ProcessColumn::Cpu | ProcessColumn::Gpu => 85.,
            ProcessColumn::Disk => 178.,
            ProcessColumn::Runtime => 145.,
            _ => 110.,
        };
        let mut column = Column::new(kind.label(), kind.label())
            .width(px(width))
            .movable(false)
            .sort(if kind == self.column {
                if self.descending {
                    ColumnSort::Descending
                } else {
                    ColumnSort::Ascending
                }
            } else {
                ColumnSort::Default
            });
        if index >= 4 {
            column = column.text_right();
        }
        column
    }

    fn perform_sort(
        &mut self,
        index: usize,
        sort: ColumnSort,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.column = ProcessColumn::ALL[index];
        self.descending = !matches!(sort, ColumnSort::Ascending);
        self.reorder();
        let row = self.selected.and_then(|key| self.row_for(key));
        cx.defer_in(window, move |table, _, cx| {
            if let Some(row) = row {
                table.set_selected_row(row, cx);
            } else {
                table.clear_selection(cx);
            }
        });
        cx.notify();
    }

    fn render_td(
        &mut self,
        row: usize,
        index: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(process) = self.process(row) else {
            return div();
        };
        let kind = ProcessColumn::ALL[index];
        let color = match kind {
            ProcessColumn::Cpu if process.cpu_usage >= 50. => theme::PEACH,
            ProcessColumn::Cpu | ProcessColumn::Gpu => theme::BLUE,
            ProcessColumn::Memory => theme::GREEN,
            ProcessColumn::Status | ProcessColumn::User | ProcessColumn::Pid => theme::SUBTEXT,
            _ => theme::TEXT,
        };
        div()
            .text_size(px(13.))
            .text_color(rgb(color))
            .truncate()
            .child(kind.text(process))
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .p_6()
            .text_color(rgb(theme::SUBTEXT))
            .child(if self.snapshot.is_some() {
                "No matching processes. Change the search to see more."
            } else {
                "Reading processes…"
            })
    }

    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let label = self
            .process(row)
            .map(|process| {
                ProcessColumn::ALL
                    .iter()
                    .map(|column| format!("{} {}", column.label(), column.text(process)))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        div().id(("row", row)).aria_label(label)
    }
}
