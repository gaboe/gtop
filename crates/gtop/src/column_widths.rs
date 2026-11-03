use gpui::{prelude::*, *};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Column {
    Lamp,
    Name,
    Pid,
    User,
    Cpu,
    Memory,
    Age,
    Gpu,
    Ports,
    Command,
    CoreTime,
    Share,
    CpuShare,
    GpuTime,
    PeakMemory,
    PeakCpu,
    DiskRead,
    DiskWrite,
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
            Self::Age => 100.,
            Self::Gpu => 65.,
            Self::Ports => 80.,
            Self::Command => 320.,
            Self::CoreTime => 120.,
            Self::Share => 160.,
            Self::CpuShare => 110.,
            Self::GpuTime => 100.,
            Self::PeakMemory => 125.,
            Self::PeakCpu => 115.,
            Self::DiskRead | Self::DiskWrite => 100.,
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
            Self::Age => 88.,
            Self::Gpu => 60.,
            Self::Ports => 65.,
            Self::Command => 100.,
            Self::CoreTime => 110.,
            Self::Share => 75.,
            Self::CpuShare => 105.,
            Self::GpuTime => 95.,
            Self::PeakMemory => 120.,
            Self::PeakCpu => 110.,
            Self::DiskRead | Self::DiskWrite => 90.,
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

pub const HISTORY: [Column; 10] = [
    Column::Lamp,
    Column::Name,
    Column::CoreTime,
    Column::Share,
    Column::CpuShare,
    Column::GpuTime,
    Column::PeakMemory,
    Column::PeakCpu,
    Column::DiskRead,
    Column::DiskWrite,
];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Widths {
    columns: BTreeMap<Column, f32>,
    #[serde(default)]
    reset_columns: BTreeSet<Column>,
}
impl Widths {
    pub fn set(&mut self, column: Column, width: f32) {
        self.reset_columns.remove(&column);
        self.columns.insert(column, column.clamp_width(width));
    }
    pub fn reset(&mut self, column: Column) {
        if column == Column::Name {
            self.columns.remove(&column);
            self.reset_columns.remove(&column);
        } else {
            self.columns.insert(column, column.default_width());
            self.reset_columns.insert(column);
        }
    }
    pub fn compute(&self, columns: &[Column], viewport: f32) -> Vec<f32> {
        let mut widths: Vec<_> = columns
            .iter()
            .map(|c| c.clamp_width(self.columns.get(c).copied().unwrap_or(c.default_width())))
            .collect();
        let total: f32 = widths.iter().sum();
        let spare = viewport - total;
        if spare >= 0. {
            let fill = if self.columns.contains_key(&Column::Name) {
                columns
                    .iter()
                    .position(|c| {
                        matches!(c, Column::Command | Column::Share)
                            && !self.columns.contains_key(c)
                    })
                    .or_else(|| {
                        columns
                            .iter()
                            .position(|c| *c == Column::Cpu && !self.columns.contains_key(c))
                    })
                    .or_else(|| {
                        columns.iter().rposition(|column| {
                            !matches!(column, Column::Lamp | Column::Name)
                                && !self.columns.contains_key(column)
                        })
                    })
            } else {
                columns.iter().position(|c| *c == Column::Name)
            };
            if let Some(fill) = fill {
                widths[fill] += spare;
            }
        } else {
            let mut deficit = -spare;
            for (width, column) in widths.iter_mut().zip(columns) {
                let shrink =
                    if self.columns.contains_key(column) && !self.reset_columns.contains(column) {
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
    resize_handle("column-resize", false)
}

pub fn handle_left() -> Stateful<Div> {
    resize_handle("detail-resize", true)
}

fn pin_to_edge<E: Styled>(element: E, left: bool) -> E {
    if left {
        element.left_0()
    } else {
        element.right_0()
    }
}

fn resize_handle(id: &'static str, left: bool) -> Stateful<Div> {
    pin_to_edge(div().id(id).absolute(), left)
        .top_0()
        .h_full()
        .w(px(7.))
        .cursor(CursorStyle::ResizeColumn)
        .child(
            pin_to_edge(div().absolute(), left)
                .top(px(5.))
                .bottom(px(5.))
                .w(px(1.))
                .bg(rgba(0x88888880)),
        )
        .on_click(|_, _, cx| cx.stop_propagation())
}

pub fn events<T: 'static>(
    cx: &Context<T>,
    _dragging: bool,
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
            let movement = std::rc::Rc::new(movement);
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture
                    && event.pressed_button == Some(MouseButton::Left)
                {
                    movement(event, window, cx);
                }
            });
            let finish = std::rc::Rc::new(finish);
            window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture {
                    finish(event, window, cx);
                }
            });
        },
    )
    .absolute()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::{Column, Widths, HISTORY};

    #[test]
    fn age_minimum_fits_999_day_label_in_priority_rows() {
        let font = ttf_parser::Face::parse(include_bytes!("../assets/fonts/JetBrainsMono.ttf"), 0)
            .expect("bundled JetBrains Mono font");
        let label = "999d 00h";
        let units_per_em = f32::from(font.units_per_em());
        let text_width = label
            .chars()
            .map(|character| {
                let glyph = font.glyph_index(character).expect("duration glyph");
                f32::from(font.glyph_hor_advance(glyph).expect("glyph advance")) / units_per_em
                    * 14.
            })
            .sum::<f32>();

        assert!(Column::Age.minimum() >= text_width + 20.);
    }

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
    fn non_name_changes_preserve_explicit_name_width_in_live_and_history_tables() {
        let live = crate::model::ProcessColumn::ALL.map(Column::from);
        for (columns, other) in [
            (live.as_slice(), Column::Cpu),
            (HISTORY.as_slice(), Column::CoreTime),
        ] {
            let mut widths = Widths::default();
            widths.set(Column::Name, 150.);
            let name_index = columns
                .iter()
                .position(|column| *column == Column::Name)
                .unwrap();
            assert_eq!(widths.compute(columns, 1400.)[name_index], 150.);

            widths.set(other, other.default_width() + 20.);
            assert_eq!(widths.compute(columns, 1400.)[name_index], 150.);

            widths.reset(other);
            assert_eq!(widths.compute(columns, 1400.)[name_index], 150.);
        }
    }

    #[test]
    fn age_is_a_live_column_and_stays_out_of_history() {
        let live = crate::model::ProcessColumn::ALL.map(Column::from);
        assert!(live.contains(&Column::Age));
        assert!(!HISTORY.contains(&Column::Age));
    }

    #[test]
    fn explicit_fill_columns_preserve_full_width_in_live_and_history_tables() {
        let live = crate::model::ProcessColumn::ALL.map(Column::from);
        for (columns, fill) in [
            (live.as_slice(), Column::Command),
            (HISTORY.as_slice(), Column::Share),
        ] {
            let mut widths = Widths::default();
            widths.set(Column::Name, 150.);
            let name_index = columns
                .iter()
                .position(|column| *column == Column::Name)
                .unwrap();
            let fill_index = columns.iter().position(|column| *column == fill).unwrap();
            let requested_width = fill.default_width() + 30.;

            widths.set(fill, requested_width);
            let resized = widths.compute(columns, 1400.);
            assert_eq!(resized[name_index], 150.);
            assert_eq!(resized[fill_index], requested_width);
            assert_eq!(resized.iter().sum::<f32>(), 1400.);
        }
    }

    #[test]
    fn resetting_live_fill_column_restores_its_default_width() {
        let live = crate::model::ProcessColumn::ALL.map(Column::from);
        let mut widths = Widths::default();
        widths.set(Column::Name, 150.);
        widths.set(Column::Command, 350.);
        widths.reset(Column::Command);
        let reset = widths.compute(&live, 1400.);
        assert_eq!(reset[1], 150.);
        let command = live
            .iter()
            .position(|column| *column == Column::Command)
            .unwrap();
        assert_eq!(reset[command], Column::Command.default_width());
        assert_eq!(reset.iter().sum::<f32>(), 1400.);
        widths.reset(Column::Name);
        assert!(widths.compute(&live, 1400.)[1] > Column::Name.default_width());
    }

    #[test]
    fn resetting_history_fill_column_restores_its_default_width() {
        let mut widths = Widths::default();
        widths.set(Column::Name, 150.);
        widths.set(Column::Share, 190.);
        widths.reset(Column::Share);
        let reset = widths.compute(&HISTORY, 1400.);
        assert_eq!(reset[1], 150.);
        assert_eq!(reset[3], Column::Share.default_width());
        assert_eq!(reset.iter().sum::<f32>(), 1400.);
        widths.reset(Column::Name);
        assert!(widths.compute(&HISTORY, 1400.)[1] > Column::Name.default_width());
    }

    #[test]
    fn old_saved_name_width_ignores_stale_name_fill_flag() {
        let mut widths: Widths =
            serde_json::from_str(r#"{"columns":{"name":150.0},"fill_name":true}"#).unwrap();
        let live = crate::model::ProcessColumn::ALL.map(Column::from);
        let name_index = live
            .iter()
            .position(|column| *column == Column::Name)
            .unwrap();
        assert_eq!(widths.compute(&live, 1400.)[name_index], 150.);

        widths.set(Column::Cpu, 200.);
        assert_eq!(widths.compute(&live, 1400.)[name_index], 150.);
        widths.reset(Column::Cpu);
        assert_eq!(widths.compute(&live, 1400.)[name_index], 150.);
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
        assert_eq!(widths.compute(&columns, 800.), vec![600., 150.]);
        widths.reset(Column::Name);
        widths.reset(Column::Command);
        widths.set(Column::Command, f32::NAN);
        assert_eq!(widths.compute(&columns, 800.), vec![480., 320.]);
    }

    #[test]
    fn saved_name_hands_spare_width_to_cpu_then_to_the_last_free_column() {
        let compact = crate::model::ProcessColumn::ALL
            .into_iter()
            .filter(|column| {
                !matches!(
                    column,
                    crate::model::ProcessColumn::User | crate::model::ProcessColumn::Command
                )
            })
            .map(Column::from)
            .collect::<Vec<_>>();
        let mut widths = Widths::default();
        widths.set(Column::Name, 150.);
        assert_eq!(
            widths.compute(&compact, 1400.),
            vec![22., 150., 70., 823., 90., 100., 65., 80.]
        );
        widths.set(Column::Cpu, 180.);
        assert_eq!(
            widths.compute(&compact, 1400.),
            vec![22., 150., 70., 180., 90., 100., 65., 723.]
        );

        let mut widths = Widths::default();
        widths.set(Column::Name, 150.);
        widths.set(Column::Share, 160.);
        let history = widths.compute(&HISTORY, 1400.);
        assert_eq!(history[0], 22.);
        assert_eq!(history[9], 398.);
        assert_eq!(history.iter().sum::<f32>(), 1400.);
    }

    #[test]
    fn overflow_shrinks_free_columns_in_order_only_as_far_as_needed() {
        let live = crate::model::ProcessColumn::ALL.map(Column::from);
        assert_eq!(
            Widths::default().compute(&live, 1000.),
            vec![22., 100., 60., 60., 145., 80., 88., 60., 65., 320.]
        );
        let compact = [Column::Lamp, Column::Name, Column::Cpu];
        assert_eq!(
            Widths::default().compute(&compact, 330.),
            vec![22., 128., 180.]
        );
    }

    mod rendered {
        use super::super::{events, handle, handle_left};
        use gpui::{
            div, point, prelude::*, px, Context, Modifiers, MouseButton, TestAppContext, Window,
        };

        struct Handles;

        impl Render for Handles {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .relative()
                            .w(px(200.))
                            .h(px(20.))
                            .debug_selector(|| "right-parent".into())
                            .child(handle().debug_selector(|| "right-handle".into())),
                    )
                    .child(
                        div()
                            .relative()
                            .w(px(200.))
                            .h(px(20.))
                            .debug_selector(|| "left-parent".into())
                            .child(handle_left().debug_selector(|| "left-handle".into())),
                    )
            }
        }

        #[gpui::test]
        async fn resize_handles_sit_on_their_named_edge(cx: &mut TestAppContext) {
            let (_, visual) = cx.add_window_view(|_, _| Handles);
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let right_parent = visual.debug_bounds("right-parent").unwrap();
            let right = visual.debug_bounds("right-handle").unwrap();
            assert_eq!(right.size.width, px(7.));
            assert_eq!(right.right(), right_parent.right());
            let left_parent = visual.debug_bounds("left-parent").unwrap();
            let left = visual.debug_bounds("left-handle").unwrap();
            assert_eq!(left.size.width, px(7.));
            assert_eq!(left.left(), left_parent.left());
        }

        #[derive(Default)]
        struct Drag {
            measured: f32,
            moves: usize,
            releases: usize,
        }

        impl Render for Drag {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .relative()
                    .w(px(300.))
                    .h(px(100.))
                    .child(events(
                        cx,
                        false,
                        |this: &mut Self, width, _, _| this.measured = *width,
                        |this, _, _, _| this.moves += 1,
                        |this, _, _, _| this.releases += 1,
                    ))
                    .child(
                        div()
                            .id("consumes-mouse")
                            .absolute()
                            .size_full()
                            .on_mouse_move(|_, _, cx| cx.stop_propagation())
                            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
                    )
            }
        }

        #[gpui::test]
        async fn drag_events_reach_the_table_before_elements_that_consume_them(
            cx: &mut TestAppContext,
        ) {
            let (view, visual) = cx.add_window_view(|_, _| Drag::default());
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let inside = point(px(150.), px(50.));
            visual.simulate_mouse_move(inside, None, Modifiers::default());
            view.read_with(visual, |drag, _| {
                assert_eq!(drag.measured, 300.);
                assert_eq!(
                    drag.moves, 0,
                    "a move without a pressed button does not drag"
                );
            });
            visual.simulate_mouse_move(inside, Some(MouseButton::Left), Modifiers::default());
            view.read_with(visual, |drag, _| assert_eq!(drag.moves, 1));
            visual.simulate_mouse_up(inside, MouseButton::Left, Modifiers::default());
            view.read_with(visual, |drag, _| assert_eq!(drag.releases, 1));
        }
    }
}
