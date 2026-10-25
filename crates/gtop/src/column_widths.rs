use gpui::{prelude::*, *};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Column {
    Lamp,
    Name,
    Pid,
    User,
    Cpu,
    Memory,
    Gpu,
    Ports,
    Command,
    CoreTime,
    Share,
    CpuShare,
    GpuTime,
    PeakMemory,
    PeakCpu,
}
impl Column {
    pub fn default_width(self) -> f32 {
        match self {
            Self::Lamp => 22.,
            Self::Name => 200.,
            Self::Pid => 70.,
            Self::User => 75.,
            Self::Cpu => 180.,
            Self::Memory => 90.,
            Self::Gpu => 65.,
            Self::Ports => 80.,
            Self::Command => 320.,
            Self::CoreTime => 120.,
            Self::Share => 160.,
            Self::CpuShare => 110.,
            Self::GpuTime => 100.,
            Self::PeakMemory => 125.,
            Self::PeakCpu => 115.,
        }
    }
    pub fn minimum(self) -> f32 {
        match self {
            Self::Lamp => 22.,
            Self::Name => 100.,
            Self::Pid => 60.,
            Self::User => 60.,
            Self::Cpu => 145.,
            Self::Memory => 80.,
            Self::Gpu => 60.,
            Self::Ports => 65.,
            Self::Command => 100.,
            Self::CoreTime => 110.,
            Self::Share => 75.,
            Self::CpuShare => 105.,
            Self::GpuTime => 95.,
            Self::PeakMemory => 120.,
            Self::PeakCpu => 110.,
        }
    }
    fn clamp_width(self, width: f32) -> f32 {
        if self == Self::Lamp {
            return 22.;
        }
        if width.is_finite() {
            width.clamp(self.minimum(), 2400.)
        } else {
            self.default_width()
        }
    }
}

pub const HISTORY: [Column; 8] = [
    Column::Lamp,
    Column::Name,
    Column::CoreTime,
    Column::Share,
    Column::CpuShare,
    Column::GpuTime,
    Column::PeakMemory,
    Column::PeakCpu,
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Widths {
    columns: BTreeMap<Column, f32>,
    fill_name: bool,
}
impl Default for Widths {
    fn default() -> Self {
        Self {
            columns: BTreeMap::new(),
            fill_name: true,
        }
    }
}
impl Widths {
    pub fn set(&mut self, column: Column, width: f32) {
        self.columns.insert(column, column.clamp_width(width));
        self.fill_name = column != Column::Name;
    }
    pub fn reset(&mut self, column: Column) {
        self.columns.remove(&column);
        self.fill_name = true;
    }
    pub fn compute(&self, columns: &[Column], viewport: f32) -> Vec<f32> {
        let mut widths: Vec<_> = columns
            .iter()
            .map(|c| {
                let default = if *c == Column::Name
                    && columns.contains(&Column::Cpu)
                    && !columns.contains(&Column::Command)
                {
                    150.
                } else {
                    c.default_width()
                };
                c.clamp_width(*self.columns.get(c).unwrap_or(&default))
            })
            .collect();
        let total: f32 = widths.iter().sum();
        let spare = viewport - total;
        if spare >= 0. {
            let fill = if !self.fill_name {
                columns
                    .iter()
                    .position(|c| matches!(c, Column::Command | Column::Share))
                    .or_else(|| columns.iter().position(|c| *c == Column::Cpu))
            } else {
                columns.iter().position(|c| *c == Column::Name)
            };
            if let Some(fill) = fill {
                widths[fill] += spare;
            }
        } else {
            let mut deficit = -spare;
            for (width, column) in widths.iter_mut().zip(columns) {
                let shrink = if self.columns.contains_key(column) {
                    0.
                } else {
                    deficit.min(*width - column.minimum())
                };
                *width -= shrink;
                deficit -= shrink;
            }
        }
        widths
    }
}

#[derive(Clone, Copy)]
pub struct Drag {
    pub column: Column,
    pub start_x: f32,
    pub start_width: f32,
}
#[derive(Default)]
pub struct TableState {
    pub viewport: f32,
    pub drag: Option<Drag>,
}

pub fn cell(width: f32, right: bool) -> Div {
    div()
        .w(px(width))
        .flex_shrink_0()
        .px(px(10.))
        .overflow_hidden()
        .whitespace_nowrap()
        .when(right, |d| d.text_right())
}

pub fn handle() -> Stateful<Div> {
    div()
        .id("column-resize")
        .absolute()
        .right_0()
        .top_0()
        .h_full()
        .w(px(7.))
        .cursor(CursorStyle::ResizeColumn)
        .child(
            div()
                .absolute()
                .right_0()
                .top(px(5.))
                .bottom(px(5.))
                .w(px(1.))
                .bg(rgba(0x88888880)),
        )
        .on_click(|_, _, cx| cx.stop_propagation())
}

pub fn events<T: 'static>(
    cx: &Context<T>,
    dragging: bool,
    measure: impl Fn(&mut T, &f32, &mut Window, &mut Context<T>) + 'static,
    movement: impl Fn(&mut T, &MouseMoveEvent, &mut Window, &mut Context<T>) + 'static,
    finish: impl Fn(&mut T, &MouseUpEvent, &mut Window, &mut Context<T>) + 'static,
) -> impl IntoElement {
    let measure = cx.listener(measure);
    let movement = cx.listener(movement);
    let finish = cx.listener(finish);
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            measure(&f32::from(bounds.size.width), window, cx);
            if dragging {
                let movement = std::rc::Rc::new(movement);
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        movement(event, window, cx);
                    }
                });
                let finish = std::rc::Rc::new(finish);
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        finish(event, window, cx);
                    }
                });
            }
        },
    )
    .absolute()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::{Column, Widths, HISTORY};
    #[test]
    fn hidden_preferences_survive_narrow_and_wide_layouts() {
        let mut widths = Widths::default();
        widths.set(Column::Command, 480.);
        let compact = [Column::Lamp, Column::Name, Column::Cpu];
        assert_eq!(widths.compute(&compact, 500.), vec![22., 298., 180.]);
        assert_eq!(widths.compute(&compact, 10.), vec![22., 100., 145.]);
        let full = [Column::Lamp, Column::Name, Column::Cpu, Column::Command];
        assert_eq!(widths.compute(&full, 1000.), vec![22., 318., 180., 480.]);
        assert_eq!(widths.compute(&full, 10.), vec![22., 100., 145., 480.]);
        assert_eq!(widths.compute(&full, 1000.), vec![22., 318., 180., 480.]);
    }

    #[test]
    fn history_name_resize_and_reset_move_fill() {
        let mut widths = Widths::default();
        let initial = widths.compute(&HISTORY, 1400.);
        assert_eq!(initial.iter().sum::<f32>(), 1400.);
        widths.set(Column::Name, initial[1] - 50.);
        let smaller = widths.compute(&HISTORY, 1400.);
        assert_eq!(smaller[1], initial[1] - 50.);
        assert_eq!(smaller[3], initial[3] + 50.);
        widths.reset(Column::Name);
        assert_eq!(widths.compute(&HISTORY, 1400.), initial);
    }

    #[test]
    fn clamp_reset_fill_and_narrow_viewport() {
        let columns = [Column::Name, Column::Command];
        let mut widths = Widths::default();
        assert_eq!(widths.compute(&columns, 800.), vec![480., 320.]);
        widths.set(Column::Name, 300.);
        assert_eq!(widths.compute(&columns, 800.), vec![300., 500.]);
        widths.set(Column::Name, -100.);
        assert_eq!(widths.compute(&columns, 800.), vec![100., 700.]);
        assert_eq!(widths.compute(&columns, 10.), vec![100., 100.]);
        widths.reset(Column::Name);
        assert_eq!(widths.compute(&columns, 800.), vec![480., 320.]);
        widths.set(Column::Name, 600.);
        assert_eq!(widths.compute(&columns, 800.), vec![600., 200.]);
        widths.set(Column::Command, 150.);
        assert_eq!(widths.compute(&columns, 800.), vec![650., 150.]);
        widths.reset(Column::Name);
        widths.reset(Column::Command);
        widths.set(Column::Command, f32::NAN);
        assert_eq!(widths.compute(&columns, 800.), vec![480., 320.]);
    }
}
