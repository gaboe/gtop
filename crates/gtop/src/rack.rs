use crate::{
    history::{HistoryView, Metric},
    model::{bytes, duration, ProcessColumn},
    palette::{self, Tokens},
    Monitor,
};
use gpui::{prelude::*, *};
use gpui_component::{
    button::{Button, ButtonVariants},
    input::Input,
    Sizable,
};
use gtop_core::{CpuBanks, CpuTopology};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

fn settings_escape_closes(key: &str, settings_open: bool, editing: bool) -> bool {
    key == "escape" && settings_open && !editing
}

pub trait RackButton {
    fn rack(self, t: &Tokens) -> Self;
}

impl RackButton for Button {
    fn rack(self, t: &Tokens) -> Self {
        self.outline()
            .rounded_none()
            .border_1()
            .border_color(rgb(t.rule_hi))
            .font_family("Archivo Labels")
            .font_weight(FontWeight::BOLD)
            .text_size(px(10.))
            .text_color(rgb(t.base.silk))
    }
}

pub fn lamp(color: u32, active: bool, t: &Tokens) -> Div {
    div()
        .w(px(6.))
        .h(px(6.))
        .flex_shrink_0()
        .bg(rgb(if active { color } else { t.seg_off }))
        .when(active && !t.light, |lamp| {
            lamp.shadow(vec![BoxShadow {
                inset: false,
                color: rgba((color << 8) | 0x88).into(),
                offset: point(px(0.), px(0.)),
                blur_radius: px(6.),
                spread_radius: px(1.),
            }])
        })
}

fn keycap(key: &str, description: &str, t: &Tokens) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .whitespace_nowrap()
        .child(
            div()
                .border_1()
                .border_color(rgb(t.rule_hi))
                .px(px(4.))
                .py(px(1.))
                .font_family("Silkscreen")
                .text_size(px(8.))
                .text_color(rgb(t.base.silk))
                .child(key.to_string()),
        )
        .child(
            div()
                .text_size(px(10.))
                .text_color(rgb(t.silk_dim))
                .child(description.to_string()),
        )
}

const UNIT_GLYPHS: [(char, [&str; 7]); 2] = [
    (
        '%',
        [
            "11000", "11001", "00010", "00100", "01000", "10011", "00011",
        ],
    ),
    (
        '°',
        [
            "01100", "10010", "10010", "01100", "00000", "00000", "00000",
        ],
    ),
];

type UnitPaths = (Path<Pixels>, Path<Pixels>);
type UnitCacheKey = (char, u32, u32, u32);
static UNIT_PATHS: OnceLock<Mutex<HashMap<UnitCacheKey, UnitPaths>>> = OnceLock::new();

fn core_time(seconds: f64) -> String {
    let minutes = seconds / 60.;
    if minutes >= 60. {
        format!("{} h {:02} min", minutes as u64 / 60, minutes as u64 % 60)
    } else {
        format!("{minutes:.1} min")
    }
}

fn history_cell(weight: f32, right: bool) -> Div {
    div()
        .flex_grow(weight)
        .flex_basis(px(0.))
        .min_w_0()
        .px(px(10.))
        .when(right, |cell| cell.text_right())
}

pub fn timestamp(seconds: u64, format: &str) -> String {
    chrono::DateTime::from_timestamp(seconds as i64, 0)
        .map(|t| t.with_timezone(&chrono::Local).format(format).to_string())
        .unwrap_or_else(|| "Unavailable".into())
}
pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
pub fn tracked(text: impl AsRef<str>) -> String {
    text.as_ref()
        .chars()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join("\u{200a}")
}
pub fn label(text: impl Into<SharedString>, t: &Tokens) -> Div {
    let text: SharedString = text.into();
    div()
        .font_family("Archivo Labels")
        .font_weight(FontWeight::BOLD)
        .text_size(px(10.))
        .text_color(rgb(t.silk_dim))
        .child(tracked(text.as_ref()))
}
pub fn lit(text: String, size: f32, hot: bool, t: &Tokens) -> Div {
    let ghost: String = text
        .chars()
        .map(|c| if c.is_ascii_digit() { '8' } else { c })
        .collect();
    div()
        .relative()
        .font_family("Doto Display")
        .font_weight(FontWeight::BLACK)
        .text_size(px(size))
        .line_height(px(size))
        .text_color(rgb(if hot { t.base.hot } else { t.base.led }))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .text_color(rgb(palette::mix(t.base.led, t.base.ground, 0.18)))
                .child(ghost),
        )
        .child(div().relative().child(text))
}

fn unit_paths(bounds: Bounds<Pixels>, unit: char) -> UnitPaths {
    let key = (
        unit,
        f32::from(bounds.origin.x).to_bits(),
        f32::from(bounds.origin.y).to_bits(),
        f32::from(bounds.size.height).to_bits(),
    );
    let cache = UNIT_PATHS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = cache.lock() {
        if let Some(paths) = cache.get(&key) {
            return paths.clone();
        }
    }
    let rows = UNIT_GLYPHS
        .iter()
        .find_map(|(glyph, rows)| (*glyph == unit).then_some(rows))
        .expect("unit glyph is defined");
    let scale = f32::from(bounds.size.height) / 14.;
    let origin_x = f32::from(bounds.origin.x);
    let origin_y = f32::from(bounds.origin.y);
    let mut lit = PathBuilder::fill();
    let mut ghost = PathBuilder::fill();
    for (y, row) in rows.iter().enumerate() {
        for (x, bit) in row.bytes().enumerate() {
            let center_x = origin_x + (x as f32 * 2. + 1.) * scale;
            let center_y = origin_y + (y as f32 * 2. + 1.) * scale;
            let path = if bit == b'1' { &mut lit } else { &mut ghost };
            let radius = scale * 0.82;
            let points = (0..12)
                .map(|i| {
                    let angle = i as f32 * std::f32::consts::TAU / 12.;
                    point(
                        px(center_x + angle.cos() * radius),
                        px(center_y + angle.sin() * radius),
                    )
                })
                .collect::<Vec<_>>();
            path.add_polygon(&points, true);
        }
    }
    let paths = (
        lit.build().expect("lit unit geometry"),
        ghost.build().expect("ghost unit geometry"),
    );
    if let Ok(mut cache) = cache.lock() {
        if cache.len() >= 32 {
            cache.clear();
        }
        cache.insert(key, paths.clone());
    }
    paths
}

fn unit_glyph(unit: char, size: f32, hot: bool, t: &Tokens) -> Div {
    let led = if hot { t.base.hot } else { t.base.led };
    let ghost = palette::mix(t.base.led, t.base.ground, 0.18);
    div().ml(px(size * 0.12)).flex_shrink_0().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let (lit, ghost_path) = unit_paths(bounds, unit);
                window.paint_path(ghost_path, rgb(ghost));
                window.paint_path(lit, rgb(led));
            },
        )
        .w(px(size * 0.74 * 10. / 14.))
        .h(px(size * 0.74)),
    )
}

pub fn lit_unit(value: String, unit: char, size: f32, hot: bool, t: &Tokens) -> Div {
    let width_chars = value.chars().count().max(3);
    let value = format!("{value:>width_chars$}");
    let ghost: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_digit() || c.is_whitespace() {
                '8'
            } else {
                c
            }
        })
        .collect();
    let width = px(vfd_digit_width(size, width_chars));
    div()
        .flex()
        .items_baseline()
        .flex_shrink_0()
        .child(
            div()
                .relative()
                .w(width)
                .h(px(size))
                .flex_shrink_0()
                .whitespace_nowrap()
                .font_family("Doto Display")
                .font_weight(FontWeight::BLACK)
                .text_size(px(size))
                .line_height(px(size))
                .text_color(rgb(if hot { t.base.hot } else { t.base.led }))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .text_color(rgb(palette::mix(t.base.led, t.base.ground, 0.18)))
                        .whitespace_nowrap()
                        .child(ghost),
                )
                .child(div().relative().whitespace_nowrap().child(value)),
        )
        .child(unit_glyph(unit, size, hot, t))
}

fn vfd_digit_width(size: f32, characters: usize) -> f32 {
    size * 0.72 * characters.max(3) as f32
}

fn unit_with_peak(
    name: &str,
    average: Option<f64>,
    peak: Option<f64>,
    unit: char,
    size: f32,
    hot: bool,
    t: &Tokens,
) -> Div {
    div()
        .flex()
        .flex_col()
        .child(label(name, t))
        .child(
            average
                .map(|v| lit_unit(format!("{v:3.0}"), unit, size, hot, t))
                .unwrap_or_else(|| lit("·".into(), size, false, t)),
        )
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(3.))
                .child(label("PEAK", t))
                .child(
                    peak.map(|v| lit_unit(format!("{v:3.0}"), unit, 9., hot, t))
                        .unwrap_or_else(|| lit("·".into(), 9., false, t)),
                ),
        )
}
pub fn screws(t: &Tokens) -> Div {
    let mut d = div().absolute().size_full();
    for (right, bottom) in [(false, false), (true, false), (false, true), (true, true)] {
        d = d.child(
            div()
                .absolute()
                .w(px(7.))
                .h(px(7.))
                .rounded_full()
                .border_1()
                .border_color(rgb(t.rule_hi))
                .when(right, |d| d.right(px(5.)))
                .when(!right, |d| d.left(px(5.)))
                .when(bottom, |d| d.bottom(px(5.)))
                .when(!bottom, |d| d.top(px(5.)))
                .child(
                    div()
                        .absolute()
                        .top(px(1.))
                        .left(px(1.))
                        .w(px(4.))
                        .h(px(4.))
                        .children((0..4).map(|index| {
                            div()
                                .absolute()
                                .left(px(index as f32))
                                .top(px(3. - index as f32))
                                .w(px(1.))
                                .h(px(1.))
                                .bg(rgb(t.rule_hi))
                        })),
                ),
        );
    }
    d
}
pub fn win(t: &Tokens) -> Div {
    let dots = palette::mix(t.base.led, palette::mix(t.base.ground, 0, 0.8), 0.07);
    div()
        .relative()
        .overflow_hidden()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(px(6.))
        .px(px(12.))
        .py(px(10.))
        .bg(rgb(palette::mix(t.base.ground, 0, 0.8)))
        .border_1()
        .border_color(rgb(0))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let mut path = PathBuilder::fill();
                    for x in (0..f32::from(bounds.size.width) as usize).step_by(3) {
                        for y in (0..f32::from(bounds.size.height) as usize).step_by(3) {
                            let p = bounds.origin + point(px(x as f32), px(y as f32));
                            path.move_to(p);
                            path.line_to(p + point(px(0.8), px(0.)));
                            path.line_to(p + point(px(0.8), px(0.8)));
                            path.line_to(p + point(px(0.), px(0.8)));
                            path.close();
                        }
                    }
                    if let Ok(path) = path.build() {
                        window.paint_path(path, rgb(dots));
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
}
pub fn hatch(color: u32) -> Div {
    div().flex_1().h_full().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let w = f32::from(bounds.size.width);
                let h = f32::from(bounds.size.height);
                let mut path = PathBuilder::stroke(px(2.));
                for k in (0..(w + h) as usize).step_by(5) {
                    let k = k as f32;
                    path.move_to(bounds.origin + point(px((k - h).max(0.)), px(k.min(h))));
                    path.line_to(bounds.origin + point(px(k.min(w)), px((k - w).max(0.))));
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, rgb(color));
                }
            },
        )
        .size_full(),
    )
}

pub fn segments(value: f32, n: usize, t: &Tokens) -> Div {
    div()
        .flex()
        .gap(px(2.))
        .h(px(8.))
        .w_full()
        .children((0..n).map(|i| {
            div().flex_1().h_full().bg(rgb(
                if i < (value.clamp(0., 100.) / 100. * n as f32).round() as usize {
                    if i as f32 >= n as f32 * 0.85 {
                        t.base.hot
                    } else {
                        t.base.led
                    }
                } else {
                    palette::mix(t.base.led, t.base.ground, 0.12)
                },
            ))
        }))
}

fn core_bank(name: &str, values: &[(usize, f32)], t: &Tokens) -> Div {
    let average = values.iter().map(|(_, value)| value).sum::<f32>() / values.len().max(1) as f32;
    div()
        .flex_1()
        .min_w_0()
        .child(
            div()
                .flex()
                .justify_between()
                .child(label(name, t))
                .child(label(format!("{average:.0} %"), t)),
        )
        .child(
            div()
                .flex()
                .items_end()
                .gap(px(2.))
                .children(values.iter().map(|(index, value)| {
                    let active = (value.clamp(0., 100.) / 100. * 12.).round() as usize;
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().flex().flex_col().gap(px(1.)).h(px(36.)).children(
                            (0..12).rev().map(|segment| {
                                div().flex_1().bg(rgb(if *value > 90. && segment >= 9 {
                                    t.base.hot
                                } else if segment < active {
                                    t.base.led
                                } else {
                                    palette::mix(t.base.led, t.base.ground, 0.12)
                                }))
                            }),
                        ))
                        .child(
                            div()
                                .text_center()
                                .font_family("JetBrains Mono")
                                .text_size(px(8.))
                                .text_color(rgb(t.silk_dim))
                                .child((index + 1).to_string()),
                        )
                })),
        )
}

fn core_banks(usage: &[f32], topology: &CpuTopology, t: &Tokens) -> Div {
    let mut banks = div().flex().gap(px(8.));
    match topology.banks(usage) {
        CpuBanks::Hybrid {
            performance,
            efficiency,
        } => {
            banks = banks
                .child(core_bank("P CORES", &performance, t))
                .child(core_bank("E CORES", &efficiency, t));
        }
        CpuBanks::Cores(cores) => {
            banks = banks.child(core_bank(
                "CORES",
                &cores.iter().copied().enumerate().collect::<Vec<_>>(),
                t,
            ))
        }
    }
    banks
}
fn readout(name: &str, value: String, t: &Tokens) -> Div {
    let display = if let Some(number) = value.strip_suffix('%') {
        lit_unit(number.to_string(), '%', 22., false, t)
    } else if let Some(number) = value.strip_suffix('°') {
        lit_unit(number.to_string(), '°', 22., false, t)
    } else {
        lit(value, 22., false, t)
    };
    div()
        .flex_1()
        .child(label(name.to_uppercase(), t))
        .child(display)
}
fn tape(
    view: &HistoryView,
    metric: Metric,
    height: f32,
    mem_total: u64,
    t: &Tokens,
    selected_moment: Option<crate::history::MomentSelection>,
    cx: &mut Context<Monitor>,
) -> Div {
    let step = if view.columns.len() > 480 {
        10
    } else if view.columns.len() > 60 {
        5
    } else {
        1
    };
    div().flex().items_end().gap(px(1.)).h(px(height)).children(
        view.columns.chunks(step).enumerate().map(|(index, group)| {
            let selection = crate::history::MomentSelection {
                start_minute: view.start_minute + (index * step) as u64,
                step,
            };
            let selected = selected_moment == Some(selection);
            let click = cx.listener(move |this, _, window, cx| {
                window.focus(&this.focus, cx);
                this.select_moment(selection, cx);
            });
            let values: Vec<_> = group
                .iter()
                .flatten()
                .filter_map(|b| b.value(metric).map(|value| (value, b.weight(metric))))
                .collect();
            let column = if group.iter().all(Option::is_none) {
                hatch(palette::mix(t.base.led, t.base.ground, 0.14))
            } else if values.is_empty() {
                div().flex_1().h(px(1.)).bg(rgb(t.led_off))
            } else {
                let value = crate::history::weighted_average(values).unwrap_or(0.);
                let percent = match metric {
                    Metric::Cpu | Metric::Gpu => value,
                    Metric::Memory => value / mem_total.max(1) as f64 * 100.,
                    Metric::Temperature => (value - 30.) / 80. * 100.,
                };
                div()
                    .flex_1()
                    .h(px((percent as f32 / 100. * height).clamp(2., height)))
                    .bg(rgb(
                        if value
                            > match metric {
                                Metric::Memory => mem_total as f64 * 0.92,
                                Metric::Temperature => 90.,
                                _ => 70.,
                            }
                        {
                            t.base.hot
                        } else {
                            t.base.led
                        },
                    ))
            };
            column
                .id(("moment-column", index))
                .role(Role::Button)
                .cursor_pointer()
                .when(selected, |column| column.bg(rgb(t.base.silk)))
                .on_click(click)
        }),
    )
}

impl Monitor {
    fn moment_panel(&self, t: &Tokens, cx: &mut Context<Self>) -> Stateful<Div> {
        let Some(selection) = self.moment else {
            return div().id("moment-panel");
        };
        let step = selection.step;
        let start = selection.start_minute * 60;
        let end = selection.start_minute.saturating_add(step as u64) * 60;
        let when = if step == 1 {
            timestamp(start, "%H:%M")
        } else {
            format!("{}–{}", timestamp(start, "%H:%M"), timestamp(end, "%H:%M"))
        };
        let detail = self
            .moment_detail
            .as_ref()
            .filter(|d| d.selection == selection);
        let recorded = detail.map(|d| d.recorded_minutes).unwrap_or(0);
        let mut panel = div()
            .id("moment-panel")
            .w(px(380.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(t.base.plate))
            .border_1()
            .border_color(rgb(t.rule))
            .child(
                div()
                    .px(px(14.))
                    .py(px(12.))
                    .border_b_1()
                    .border_color(rgb(t.rule))
                    .child(label(
                        format!(
                            "{} · {} MIN · {} RECORDED · ← → STEP",
                            timestamp(start, "%x"),
                            step,
                            recorded
                        ),
                        t,
                    ))
                    .child(lit(when, 26., false, t)),
            );
        if detail.is_some() && recorded == 0 {
            panel =
                panel.child(div().flex_1().p(px(14.)).child(
                    win(t).child("gtop was not running. Nothing was recorded for this span."),
                ));
        } else if let Some(detail) = detail {
            let values = [
                (
                    Metric::Cpu,
                    "CPU",
                    detail.cpu_average,
                    detail.cpu_peak,
                    Some('%'),
                ),
                (
                    Metric::Memory,
                    "Memory",
                    detail.memory_average,
                    detail.memory_peak,
                    None,
                ),
                (
                    Metric::Gpu,
                    "GPU",
                    detail.gpu_average,
                    detail.gpu_peak,
                    Some('%'),
                ),
                (
                    Metric::Temperature,
                    "CPU °C",
                    detail.temperature_average,
                    detail.temperature_peak,
                    Some('°'),
                ),
            ];
            let mut metrics = div().grid().grid_cols(2).gap(px(6.)).p(px(10.));
            for (metric, name, average, peak, unit) in values {
                let active = metric == self.metric;
                let readout = if let Some(unit) = unit {
                    unit_with_peak(name, average, peak, unit, 18., false, t)
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .child(label(name, t))
                        .child(lit(
                            average
                                .map(|v| bytes(v as u64))
                                .unwrap_or_else(|| "·".into()),
                            18.,
                            false,
                            t,
                        ))
                        .child(label(
                            peak.map(|v| format!("PEAK {}", bytes(v as u64)))
                                .unwrap_or_else(|| "PEAK ·".into()),
                            t,
                        ))
                };
                metrics = metrics.child(
                    div()
                        .p(px(8.))
                        .border_1()
                        .border_color(rgb(if active { t.base.silk } else { t.rule }))
                        .child(readout),
                );
            }
            let rows = self.moment_rows.clone();
            let max = rows.first().map(|r| r.2).unwrap_or(1.).max(0.001);
            let mut list = div().flex().flex_col().gap(px(1.));
            for (index, (name, total, value)) in rows.into_iter().enumerate() {
                let process = self.snapshot.as_ref().and_then(|snapshot| {
                    crate::model::running_history_process(&name, &snapshot.processes)
                });
                let exited = process.is_none();
                let value_label = match self.metric {
                    Metric::Memory => bytes(total.peak_memory),
                    Metric::Gpu => format!("{:.1} min", total.gpu_seconds / 60.),
                    _ => format!("{:.1} min", total.cpu_seconds / 60.),
                };
                let mut row = div()
                    .id(("moment-process", index))
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(if self.moment_selected == index {
                        t.base.silk
                    } else {
                        t.row_rule
                    }))
                    .text_color(rgb(if index < 3 { t.base.led } else { t.ink_row }))
                    .child(
                        div()
                            .w(px(6.))
                            .h(px(6.))
                            .flex_shrink_0()
                            .bg(rgb(if index < 3 { t.base.led } else { t.seg_off })),
                    )
                    .child(div().w(px(142.)).min_w_0().truncate().child(name.clone()))
                    .child(
                        div()
                            .w(px(92.))
                            .child(segments((value / max * 100.) as f32, 16, t)),
                    )
                    .child(
                        div()
                            .w(px(76.))
                            .text_right()
                            .text_size(px(10.))
                            .child(if exited {
                                format!("{value_label} · exited")
                            } else {
                                value_label
                            }),
                    );
                if !exited {
                    let process_name = name.clone();
                    row = row
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(process) = this.snapshot.as_ref().and_then(|snapshot| {
                                crate::model::running_history_process(
                                    &process_name,
                                    &snapshot.processes,
                                )
                            }) {
                                this.select(crate::model::ProcessKey::from(process), cx);
                            }
                        }));
                }
                list = list.child(row);
            }
            let ranking = match self.metric {
                Metric::Cpu => "CORE TIME",
                Metric::Memory => "PEAK MEMORY",
                Metric::Gpu => "GPU TIME",
                Metric::Temperature => "CORE TIME",
            };
            panel = panel
                .child(div().p(px(10.)).child(win(t).child(metrics)))
                .child(
                    div()
                        .px(px(12.))
                        .py(px(8.))
                        .child(label(format!("TOP PROCESSES · {ranking}"), t)),
                )
                .child(div().px(px(8.)).child(list))
                .child(div().px(px(12.)).py(px(8.)).child(label(
                    "CLICK A RUNNING PROCESS TO OPEN IT. EXITED PROCESSES KEEP ONLY THEIR HISTORY.",
                    t,
                )));
        } else if detail.is_none() {
            panel = panel.child(div().flex_1().p(px(14.)).child("Reading history…"));
        }
        let close = Button::new("close-moment")
            .label("Close · Esc")
            .rack(t)
            .small()
            .on_click(cx.listener(|this, _, _, cx| {
                this.moment = None;
                cx.notify();
            }));
        let mut actions = div().mt_auto().p(px(8.)).flex().gap(px(6.)).child(close);
        if self.view.is_none() {
            actions = actions.child(
                Button::new("moment-full-hour")
                    .label("Full hour · 1")
                    .rack(t)
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.view = Some(0);
                        this.moment = None;
                        cx.notify();
                    })),
            );
        }
        panel.child(actions)
    }

    fn tower(&self, t: &Tokens) -> Stateful<Div> {
        let mut tower = div()
            .relative()
            .id("tower")
            .child(screws(t))
            .w(px(300.))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(12.))
            .pt(px(14.))
            .pb(px(12.))
            .bg(rgb(t.base.plate))
            .border_1()
            .border_color(rgb(t.rule))
            .child(
                div()
                    .px(px(4.))
                    .pb(px(2.))
                    .child(
                        label(
                            self.snapshot
                                .as_ref()
                                .and_then(|s| s.stats.gpus.first())
                                .map(|g| g.name.to_uppercase())
                                .unwrap_or_else(|| "SYSTEM MONITOR".into()),
                            t,
                        )
                        .text_color(rgb(t.silk_dim)),
                    )
                    .child(
                        div()
                            .font_family("Archivo Display")
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .text_size(px(48.))
                            .line_height(px(40.))
                            .child("GTOP"),
                    ),
            );
        let Some(s) = self.snapshot.as_ref() else {
            return tower.child("Reading system information");
        };
        let stats = &s.stats;
        let cores = stats.cpu_usage.len();
        let cpu = stats.cpu_usage.iter().sum::<f32>() / cores.max(1) as f32;
        tower = tower.child(
            win(t)
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(label(s.cpu_topology.label(cores), t))
                        .child(label(format!("LOAD {:.1}", stats.load_avg[0]), t)),
                )
                .child(lit_unit(format!("{cpu:3.0}"), '%', 42., cpu > 85., t))
                .child(core_banks(&stats.cpu_usage, &s.cpu_topology, t)),
        );
        tower = tower.child(
            win(t)
                .child(
                    div()
                        .flex()
                        .gap(px(10.))
                        .child(div().flex_1().child(label("MEM GB", t)).child(lit(
                            format!("{:.1}", stats.memory_used as f64 / 1e9),
                            22.,
                            stats.memory_used as f64 / stats.memory_total.max(1) as f64 > 0.92,
                            t,
                        )))
                        .child(readout(
                            "Swap GB",
                            format!("{:.1}", stats.swap_used as f64 / 1e9),
                            t,
                        )),
                )
                .child(segments(
                    stats.memory_used as f32 / stats.memory_total.max(1) as f32 * 100.,
                    40,
                    t,
                )),
        );
        let gpu = stats.gpus.first();
        let util = gpu.and_then(|g| g.utilization);
        tower = tower.child(
            win(t)
                .child(
                    div()
                        .flex()
                        .gap(px(10.))
                        .child(readout(
                            "GPU",
                            util.map(|v| format!("{v:3.0}%"))
                                .unwrap_or_else(|| "N/A".into()),
                            t,
                        ))
                        .child(readout(
                            "VRAM GB",
                            gpu.and_then(|g| g.memory_used)
                                .map(|v| format!("{:.1}", v as f64 / 1e9))
                                .unwrap_or_else(|| "N/A".into()),
                            t,
                        )),
                )
                .child(segments(util.unwrap_or(0.), 40, t)),
        );
        tower = tower.child(win(t).child(label("TEMPERATURE °C", t)).child(
            div().flex().gap(px(8.)).children(
                [("CPU", 90.), ("GPU", 90.), ("SSD", 70.), ("Battery", 45.)].map(
                    |(name, limit)| {
                        let value = stats
                            .temperatures
                            .iter()
                            .find(|v| v.name == name)
                            .map(|v| v.celsius);
                        div()
                            .flex_1()
                            .child(
                                value
                                    .map(|value| lit(format!("{value:2.0}"), 22., value > limit, t))
                                    .unwrap_or_else(|| lit("N/A".into(), 22., false, t)),
                            )
                            .child(label(if name == "Battery" { "BAT" } else { name }, t))
                    },
                ),
            ),
        ));
        let own = s.processes.iter().find(|p| p.pid == std::process::id());
        let cost = |name: &str, value: String, unit: &str| {
            div()
                .flex()
                .items_baseline()
                .gap(px(4.))
                .child(label(name.to_string(), t).text_color(rgb(t.silk_dim)))
                .child(
                    div()
                        .font_family("Silkscreen")
                        .text_size(px(10.))
                        .text_color(rgb(t.base.ok))
                        .child(value),
                )
                .child(label(unit.to_string(), t).text_color(rgb(t.silk_dim)))
        };
        tower
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .px(px(4.))
                    .pt(px(4.))
                    .child(label("● GTOP SELF", t).text_color(rgb(t.base.silk)))
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .child(cost(
                                "CPU",
                                own.map(|p| format!("{:.2}", p.cpu_usage))
                                    .unwrap_or_else(|| "N/A".into()),
                                "%",
                            ))
                            .child(cost(
                                "MEM",
                                own.map(|p| format!("{:.0}", p.memory_usage as f64 / 1_000_000.))
                                    .unwrap_or_else(|| "N/A".into()),
                                "MB",
                            ))
                            .child(cost("TICK", format!("{:.1}", self.tick_ms), "MS")),
                    )
                    .child(
                        label("FRAME UNAVAILABLE · FPS UNAVAILABLE", t).text_color(rgb(t.silk_dim)),
                    ),
            )
            .child(div().flex_1())
            .child(label(
                format!(
                    "UP {} · NET ↓ {} ↑ {}/s",
                    duration(stats.uptime),
                    bytes(stats.network_rx_bytes),
                    bytes(stats.network_tx_bytes)
                ),
                t,
            ))
    }
    pub fn rack(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = self.draft.unwrap_or(self.settings.base()).tokens();
        let compact = self.settings_open || self.details.is_some() || self.moment.is_some();
        if self.table.read(cx).compact != compact {
            self.table.update(cx, |p, cx| {
                p.compact = compact;
                cx.notify();
            });
        }
        let mut nav = div().flex().gap(px(6.)).flex_shrink_0();
        for (i, name) in ["Live", "1 h", "8 h", "24 h"].into_iter().enumerate() {
            let active = self.view == i.checked_sub(1);
            nav = nav.child(
                div()
                    .id(("view", i))
                    .role(Role::Button)
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(7.))
                    .bg(rgb(if active { t.plate2 } else { t.base.plate }))
                    .border_1()
                    .border_color(rgb(if active { t.rule_hi } else { t.rule }))
                    .child(lamp(t.base.ok, active, &t))
                    .child(label(name.to_uppercase(), &t).text_color(rgb(if active {
                        t.base.silk
                    } else {
                        t.silk_dim
                    })))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let view = i.checked_sub(1);
                        if this.view != view {
                            this.view = view;
                            cx.notify();
                        }
                    })),
            );
        }
        nav = nav
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .gap(px(10.))
                    .bg(rgb(t.well))
                    .border_1()
                    .border_color(rgb(t.rule))
                    .child(label("FIND", &t).text_color(rgb(t.silk_dim)))
                    .child(Input::new(&self.search).appearance(false).cleanable(true)),
            )
            .child(
                div()
                    .id("pause")
                    .role(Role::Button)
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(7.))
                    .bg(rgb(t.base.plate))
                    .border_1()
                    .border_color(rgb(t.rule))
                    .child(lamp(
                        if self.paused { t.base.hot } else { t.base.ok },
                        true,
                        &t,
                    ))
                    .child(label(if self.paused { "PAUSED" } else { "RUNNING" }, &t))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.paused = !this.paused;
                        this.wake();
                        cx.notify();
                    })),
            );
        let mem = self
            .snapshot
            .as_ref()
            .map(|s| s.stats.memory_total)
            .unwrap_or(1);
        let mut right = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(nav);
        if let Some(index) = self.view {
            let h = &self.history[index];
            let axis = div()
                .flex()
                .justify_between()
                .children([0., 0.25, 0.5, 0.75, 1.].map(|f| {
                    let end = h
                        .columns
                        .iter()
                        .rev()
                        .flatten()
                        .next()
                        .map(|c| c.minute)
                        .unwrap_or(now() as u64 / 60);
                    let at = end - (h.columns.len().saturating_sub(1) as f64 * (1. - f)) as u64;
                    label(timestamp(at * 60, "%H:%M"), &t)
                }));
            let step = [1, 5, 10][index];
            let coverage = format!("RECORDED {} OF {} MIN", h.covered, [60, 480, 1440][index]);
            let heading = div()
                .flex()
                .justify_between()
                .child(label(
                    format!(
                        "TOTAL CPU · LAST {} H · ONE COLUMN PER {step} MIN",
                        [1, 8, 24][index]
                    ),
                    &t,
                ))
                .child(label(coverage, &t));
            let headers = div()
                .h(px(29.))
                .flex()
                .items_center()
                .bg(rgb(t.base.plate))
                .child(div().w(px(22.)).flex_shrink_0())
                .children(
                    [
                        ("PROCESS", 28., false),
                        ("CORE TIME ▼", 12., true),
                        ("SHARE", 20., false),
                        ("OF ALL CPU", 10., true),
                        ("GPU TIME", 9., true),
                        ("PEAK MEMORY", 12., true),
                        ("PEAK CPU %", 10., true),
                    ]
                    .map(|(name, weight, right)| {
                        history_cell(weight, right).child(label(name, &t).text_color(rgb(
                            if name.starts_with("CORE") {
                                t.base.led
                            } else {
                                t.silk_dim
                            },
                        )))
                    }),
                );
            right = right
                .child(
                    win(&t)
                        .child(heading)
                        .child(tape(h, Metric::Cpu, 150., mem, &t, self.moment, cx))
                        .child(axis),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .border_1()
                        .border_color(rgb(t.rule))
                        .child(headers)
                        .child(
                            uniform_list(
                                "history-rows",
                                h.rows.len(),
                                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                    let h = &this.history[index];
                                    let t = this.draft.unwrap_or(this.settings.base()).tokens();
                                    range
                                        .filter_map(|row| {
                                            let (name, p) = h.rows.get(row)?;
                                            let selected = this
                                                .history_selected
                                                .as_ref()
                                                .map(|n| n == name)
                                                .unwrap_or(row == 0);
                                            let key = name.clone();
                                            Some(
                                                div()
                                                    .id(("history-row", row))
                                                    .bg(rgb(if selected {
                                                        t.plate2
                                                    } else {
                                                        t.base.ground
                                                    }))
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.history_selected = Some(key.clone());
                                                        cx.notify();
                                                    }))
                                                    .h(px(22.))
                                                    .flex()
                                                    .items_center()
                                                    .border_b_1()
                                                    .border_color(rgb(t.row_rule))
                                                    .text_color(rgb(if row < 3 {
                                                        t.base.led
                                                    } else {
                                                        t.ink_row
                                                    }))
                                                    .child(
                                                        div()
                                                            .w(px(22.))
                                                            .flex_shrink_0()
                                                            .px(px(10.))
                                                            .child(
                                                                lamp(t.base.led, row < 3, &t)
                                                                    .w(px(7.))
                                                                    .h(px(12.)),
                                                            ),
                                                    )
                                                    .child(
                                                        history_cell(28., false)
                                                            .truncate()
                                                            .child(name.clone()),
                                                    )
                                                    .child(
                                                        history_cell(12., true)
                                                            .child(core_time(p.cpu_seconds)),
                                                    )
                                                    .child(
                                                        history_cell(20., false).child(
                                                            segments(
                                                                (p.cpu_seconds
                                                                    / h.rows
                                                                        .first()
                                                                        .map(|r| r.1.cpu_seconds)
                                                                        .unwrap_or(1.)
                                                                        .max(0.001)
                                                                    * 100.)
                                                                    as f32,
                                                                30,
                                                                &t,
                                                            )
                                                            .w_full()
                                                            .max_w(px(120.)),
                                                        ),
                                                    )
                                                    .child(history_cell(10., true).child(format!(
                                                        "{:.1}%",
                                                        p.cpu_seconds / h.total_cpu.max(0.001)
                                                            * 100.
                                                    )))
                                                    .child(history_cell(9., true).child(
                                                        if p.gpu_seconds > 3. {
                                                            core_time(p.gpu_seconds)
                                                        } else {
                                                            "·".into()
                                                        },
                                                    ))
                                                    .child(
                                                        history_cell(12., true)
                                                            .child(bytes(p.peak_memory)),
                                                    )
                                                    .child(
                                                        history_cell(10., true)
                                                            .child(format!("{:.1}", p.peak_cpu)),
                                                    ),
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                }),
                            )
                            .track_scroll(&self.history_scroll)
                            .flex_1()
                            .min_h_0(),
                        ),
                );
        } else {
            let h = &self.history[0];
            let mut head = div()
                .flex()
                .items_center()
                .gap(px(14.))
                .child(label("LAST HOUR", &t));
            let mut tabs = div().flex().gap(px(2.));
            for (i, m) in Metric::ALL.into_iter().enumerate() {
                tabs = tabs.child(
                    Button::new(("metric", i))
                        .label(format!(
                            "{} {}",
                            if self.metric == m { "■" } else { "□" },
                            m.label().to_uppercase()
                        ))
                        .rack(&t)
                        .small()
                        .font_family("Archivo Labels")
                        .text_size(px(9.))
                        .when(self.metric == m, |b| b.primary())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.metric = m;
                            if this.moment.is_some() {
                                this.wake();
                            }
                            cx.notify();
                        })),
                );
            }
            head = head
                .child(tabs)
                .child(div().flex_1())
                .child(label("V SWITCHES · 1 OPENS THE FULL HOUR", &t));
            right = right
                .child(
                    win(&t)
                        .child(head)
                        .child(
                            div().flex().gap(px(16.)).children(
                                h.cached_leaders[self.metric as usize]
                                    .iter()
                                    .map(|(name, v)| {
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(label(name, &t))
                                            .child(if self.metric == Metric::Temperature {
                                                lit_unit(format!("{v:3.0}"), '°', 15., false, &t)
                                            } else {
                                                lit(
                                                    match self.metric {
                                                        Metric::Cpu | Metric::Gpu => {
                                                            format!("{:.1}m", v / 60.)
                                                        }
                                                        Metric::Memory => bytes(*v as u64),
                                                        Metric::Temperature => unreachable!(),
                                                    },
                                                    15.,
                                                    false,
                                                    &t,
                                                )
                                            })
                                    }),
                            ),
                        )
                        .child(tape(h, self.metric, 54., mem, &t, self.moment, cx)),
                )
                .child(div().flex_1().min_h_0().child(self.table.clone()));
        }
        let operation_status =
            !self.status.starts_with("Reading") && self.status != "Select a process for details";
        if let Some(confirmation) = self.confirmation(cx) {
            right = right.child(confirmation);
        } else {
            right = right.child(
                div()
                    .id("keys")
                    .role(Role::Status)
                    .flex()
                    .items_center()
                    .gap_2()
                    .p(px(8.))
                    .bg(rgb(t.base.plate))
                    .border_1()
                    .border_color(rgb(t.rule))
                    .children(
                        [
                            ("/", "find"),
                            ("J K", "move"),
                            ("↵", "detail"),
                            ("C M G P", "sort"),
                            ("SPACE", "pause"),
                            ("X", "kill"),
                            ("TAB", "history"),
                            ("T", "palette"),
                        ]
                        .into_iter()
                        .filter(|(key, _)| {
                            !operation_status
                                && (!compact || matches!(*key, "/" | "J K" | "↵" | "X"))
                        })
                        .map(|(key, text)| keycap(key, text, &t)),
                    )
                    .child(div().flex_1())
                    .when(operation_status, |strip| {
                        strip.child(
                            div()
                                .id("operation-status")
                                .role(Role::Status)
                                .text_size(px(10.))
                                .text_color(rgb(if self.status.ends_with("stopped.") {
                                    t.base.ok
                                } else {
                                    t.base.hot
                                }))
                                .child(self.status.clone()),
                        )
                    })
                    .child(
                        Button::new("settings")
                            .label("Settings · S")
                            .rack(&t)
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings_open = !this.settings_open;
                                if this.settings_open {
                                    this.moment = None;
                                }
                                cx.notify();
                            })),
                    ),
            );
        }

        let mut root = div()
            .size_full()
            .track_focus(&self.focus)
            .key_context("Gtop")
            .capture_action(cx.listener(|this, _: &crate::ToggleView, window, cx| {
                this.switch_view_from_tab(false, window, cx)
            }))
            .capture_action(cx.listener(|this, _: &crate::PreviousView, window, cx| {
                this.switch_view_from_tab(true, window, cx)
            }))
            .capture_action(cx.listener(
                |this, _: &gpui_component::input::IndentInline, window, cx| {
                    this.switch_view_from_tab(false, window, cx)
                },
            ))
            .capture_action(cx.listener(
                |this, _: &gpui_component::input::OutdentInline, window, cx| {
                    this.switch_view_from_tab(true, window, cx)
                },
            ))
            .capture_key_down(cx.listener(Self::key))
            .flex()
            .gap(px(8.))
            .p(px(10.))
            .bg(rgb(t.base.ground))
            .text_color(rgb(t.base.silk))
            .font_family("JetBrains Mono")
            .text_size(px(12.))
            .child(self.tower(&t))
            .child(right);
        if self.settings_open {
            root = root.child(self.palette_panel(&t, cx));
        } else if self.moment.is_some() {
            root = root.child(self.moment_panel(&t, cx));
        } else if let Some(details) = self.details(cx) {
            root = root.child(details.w(px(380.)).h_full());
        }
        root
    }
    pub fn apply_palette(&mut self, cx: &mut Context<Self>) {
        let base = self.draft.unwrap_or(self.settings.base());
        crate::theme::apply(base, cx);
        self.table.update(cx, |table, cx| {
            table.base = base;
            cx.notify();
        });
        cx.notify();
    }
    fn persist_settings(&self, _window: &mut Window, _cx: &mut Context<Self>) {
        if let Some(settings_writes) = self.settings_writes.as_ref() {
            let _ = settings_writes.unbounded_send(self.settings.clone());
        }
    }
    fn use_palette(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.active == index && self.draft.is_none() {
            return;
        }
        self.settings.active = index;
        self.draft = None;
        let b = self.settings.base();
        for (input, v) in self
            .colors
            .iter()
            .zip([b.ground, b.plate, b.silk, b.led, b.hot, b.ok])
        {
            input.update(cx, |input, cx| {
                input.set_value(format!("#{v:06x}"), window, cx)
            });
        }
        self.apply_palette(cx);
        self.persist_settings(window, cx);
    }
    fn random_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut state = now().to_bits();
        let r = std::array::from_fn(|_| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            (state >> 11) as f64 / (1u64 << 53) as f64
        });
        let b = palette::random_base(r);
        self.draft = Some(b);
        for (input, v) in self
            .colors
            .iter()
            .zip([b.ground, b.plate, b.silk, b.led, b.hot, b.ok])
        {
            input.update(cx, |input, cx| {
                input.set_value(format!("#{v:06x}"), window, cx)
            });
        }
        self.apply_palette(cx);
    }
    fn switch_view_from_tab(&mut self, reverse: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            if self.focus.is_focused(window) {
                window.focus(
                    &self.colors[if reverse { 5 } else { 0 }]
                        .read(cx)
                        .focus_handle(cx),
                    cx,
                );
            } else if reverse {
                window.focus_prev(cx);
            } else {
                window.focus_next(cx);
            }
            cx.stop_propagation();
            return;
        }
        let editing = self.mix_name.read(cx).focus_handle(cx).is_focused(window)
            || self
                .colors
                .iter()
                .any(|input| input.read(cx).focus_handle(cx).is_focused(window));
        if editing {
            return;
        }
        if !self.search.read(cx).focus_handle(cx).is_focused(window) {
            self.view = if self.view.is_some() { None } else { Some(0) };
            window.focus(&self.focus, cx);
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.platform
            || event.keystroke.modifiers.control
            || event.keystroke.modifiers.alt
        {
            return;
        }
        let k = event.keystroke.key.as_str();
        let search_focused = self.search.read(cx).focus_handle(cx).is_focused(window);
        let editing = search_focused
            || self.mix_name.read(cx).focus_handle(cx).is_focused(window)
            || self
                .colors
                .iter()
                .any(|i| i.read(cx).focus_handle(cx).is_focused(window));
        if k == "tab" {
            self.switch_view_from_tab(event.keystroke.modifiers.shift, window, cx);
            window.prevent_default();
            return;
        }
        if editing {
            if k == "escape" || k == "enter" {
                if k == "escape" && self.moment.is_some() {
                    self.moment = None;
                } else if search_focused && k == "escape" {
                    self.search.update(cx, |s, cx| s.set_value("", window, cx));
                    self.table.update(cx, |table, cx| {
                        table.search = crate::model::Search::default();
                        table.reorder();
                        table.selected = None;
                        cx.notify();
                    });
                    self.wake();
                }
                window.focus(&self.focus, cx);
                cx.stop_propagation();
            }
            return;
        }
        if settings_escape_closes(k, self.settings_open, editing) {
            self.settings_open = false;
            window.focus(&self.focus, cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.settings_open && !self.focus.is_focused(window) {
            return;
        }
        if let Some(selection) = self.moment {
            match k {
                "left" | "h" | "right" | "l" => {
                    let direction = if matches!(k, "left" | "h") { -1 } else { 1 };
                    let view = &self.history[self.view.unwrap_or(0)];
                    let end = view.start_minute + view.columns.len() as u64;
                    let next =
                        crate::history::step_moment(selection, direction, view.start_minute, end);
                    if next == selection {
                        cx.stop_propagation();
                        return;
                    }
                    self.moment = Some(next);
                    if next != selection {
                        self.moment_detail = None;
                        self.moment_selected = 0;
                        self.moment_rows.clear();
                    }
                    self.wake();
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                "escape" => {
                    self.moment = None;
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                "j" | "down" | "k" | "up" => {
                    self.moment_selected = if matches!(k, "j" | "down") {
                        (self.moment_selected + 1).min(self.moment_rows.len().saturating_sub(1))
                    } else {
                        self.moment_selected.saturating_sub(1)
                    };
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                "enter" => {
                    let key = self.moment_rows.get(self.moment_selected).and_then(|row| {
                        self.snapshot
                            .as_ref()
                            .and_then(|snapshot| {
                                crate::model::running_history_process(&row.0, &snapshot.processes)
                            })
                            .map(crate::model::ProcessKey::from)
                    });
                    if let Some(key) = key {
                        self.select(key, cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }
        if let Some(d) = self
            .details
            .as_ref()
            .filter(|d| d.kill == crate::details::KillState::Confirming)
        {
            let key = d.key;
            match k {
                "y" | "enter" => self.kill(key, window, cx),
                "n" | "escape" => {
                    self.details.as_mut().unwrap().cancel_kill(key);
                }
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        match k {
            "enter" | "left" | "right" if self.view.is_some() => {
                let view = &self.history[self.view.unwrap()];
                if let Some(selection) = crate::history::newest_moment(view) {
                    self.select_moment(selection, cx);
                }
            }
            "/" => window.focus(&self.search.read(cx).focus_handle(cx), cx),
            "space" => {
                self.paused = !self.paused;
                self.wake();
            }
            "tab" => self.view = if self.view.is_some() { None } else { Some(0) },
            "1" => self.view = Some(0),
            "8" => self.view = Some(1),
            "2" => self.view = Some(2),
            "v" => {
                self.metric = self.metric.next();
                if self.moment.is_some() {
                    self.wake();
                }
            }
            "s" => {
                self.settings_open = !self.settings_open;
                if self.settings_open {
                    self.moment = None;
                }
            }
            "t" => self.use_palette(
                (self.settings.active + 1) % self.settings.palettes().len(),
                window,
                cx,
            ),
            "r" if self.settings_open => self.random_palette(window, cx),
            "escape" => {
                if self.settings_open {
                    self.settings_open = false;
                } else {
                    self.details = None;
                }
            }
            "e" if self.details.is_some() => self.fetch_environment(window, cx),
            "enter" | "x" if self.view.is_none() => {
                let key = crate::model::action_target(
                    if k == "x" {
                        crate::model::ProcessAction::Kill
                    } else {
                        crate::model::ProcessAction::Detail
                    },
                    self.details.as_ref().map(|d| d.key),
                    self.table.read(cx).selected,
                    self.table
                        .read(cx)
                        .process(0)
                        .map(crate::model::ProcessKey::from),
                );
                if let Some(key) = key {
                    self.select(key, cx);
                    if k == "x" {
                        self.details.as_mut().unwrap().request_kill(key);
                    }
                }
            }
            "c" | "m" | "g" | "p" => {
                self.table.update(cx, |table, cx| {
                    let rows = &mut *table;
                    rows.column = match k {
                        "m" => ProcessColumn::Memory,
                        "g" => ProcessColumn::Gpu,
                        "p" => ProcessColumn::Pid,
                        _ => ProcessColumn::Cpu,
                    };
                    rows.descending = k != "p";
                    rows.reorder();
                    cx.notify();
                });
            }
            "j" | "k" | "down" | "up" if self.view.is_some() => {
                let h = &self.history[self.view.unwrap()];
                let current = self
                    .history_selected
                    .as_ref()
                    .and_then(|name| h.rows.iter().position(|r| &r.0 == name))
                    .unwrap_or(0);
                let next = if k == "j" || k == "down" {
                    (current + 1).min(h.rows.len().saturating_sub(1))
                } else {
                    current.saturating_sub(1)
                };
                if let Some((name, _)) = h.rows.get(next) {
                    self.history_selected = Some(name.clone());
                    self.history_scroll
                        .scroll_to_item(next, ScrollStrategy::Top);
                }
            }
            "j" | "k" | "down" | "up" => {
                self.table.update(cx, |table, cx| {
                    let rows = &mut *table;
                    let current = rows.selected.and_then(|k| rows.row_for(k)).unwrap_or(0);
                    let next = if k == "j" || k == "down" {
                        (current + 1).min(rows.order.len().saturating_sub(1))
                    } else {
                        current.saturating_sub(1)
                    };
                    rows.selected = rows.process(next).map(crate::model::ProcessKey::from);
                    table.navigate(next);
                    cx.notify();
                });
                if self.details.is_some() {
                    if let Some(key) = self.table.read(cx).selected {
                        self.select(key, cx);
                    }
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
    fn palette_panel(&self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let mut list = div().flex().flex_col().gap(px(4.));
        for (i, p) in self.settings.palettes().into_iter().enumerate() {
            list = list.child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        div()
                            .id(("palette", i))
                            .role(Role::Button)
                            .cursor_pointer()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px(px(8.))
                            .py(px(6.))
                            .border_1()
                            .border_color(rgb(
                                if i == self.settings.active && self.draft.is_none() {
                                    t.base.silk
                                } else {
                                    t.rule
                                },
                            ))
                            .child(
                                div()
                                    .flex()
                                    .border_1()
                                    .border_color(rgb(t.rule_hi))
                                    .children(
                                        [p.base.plate, p.base.led, p.base.hot, p.base.ok].map(
                                            |color| div().w(px(12.)).h(px(12.)).bg(rgb(color)),
                                        ),
                                    ),
                            )
                            .child(label(p.name.to_uppercase(), t))
                            .on_click(
                                cx.listener(move |this, _, w, cx| this.use_palette(i, w, cx)),
                            ),
                    )
                    .when(i >= 5, |d| {
                        d.child(
                            Button::new(("delete-palette", i))
                                .label("Del")
                                .rack(t)
                                .on_click(cx.listener(move |this, _, w, cx| {
                                    this.settings.saved.remove(i - 5);
                                    this.use_palette(0, w, cx);
                                })),
                        )
                    }),
            );
        }
        div()
            .w(px(380.))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .gap_3()
            .p(px(18.))
            .bg(rgb(t.base.plate))
            .border_1()
            .border_color(rgb(t.rule_hi))
            .child(label("SETTINGS · S", t))
            .child(
                div()
                    .text_size(px(30.))
                    .font_family("Archivo Labels")
                    .child("PALETTE"),
            )
            .child(label(
                "PRESETS AND SAVED MIXES                         T CYCLES",
                t,
            ))
            .child(list)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(label("MIX YOUR OWN", t))
                    .child(
                        Button::new("random")
                            .label("Random · R")
                            .rack(t)
                            .on_click(cx.listener(|this, _, w, cx| this.random_palette(w, cx))),
                    ),
            )
            .children(
                [
                    ("Ground", t.base.ground),
                    ("Panel", t.base.plate),
                    ("Labels", t.base.silk),
                    ("LED", t.base.led),
                    ("Hot", t.base.hot),
                    ("OK and ports", t.base.ok),
                ]
                .into_iter()
                .enumerate()
                .map(|(i, (name, color))| {
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().w(px(34.)).h(px(20.)).bg(rgb(color)))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.))
                                .text_color(rgb(t.base.silk))
                                .child(name),
                        )
                        .child(div().w(px(110.)).child(Input::new(&self.colors[i])))
                }),
            )
            .child(label(
                if self.draft.is_some() {
                    "UNSAVED MIX, ALREADY LIVE"
                } else {
                    "CHANGE A COLOR OR ROLL RANDOM"
                },
                t,
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(Input::new(&self.mix_name)))
                    .child(
                        Button::new("save-palette")
                            .label("Save")
                            .rack(t)
                            .on_click(cx.listener(|this, _, w, cx| {
                                if let Some(base) = this.draft {
                                    let name = this.mix_name.read(cx).value().trim().to_string();
                                    this.settings.saved.push(palette::Palette {
                                        name: if name.is_empty() {
                                            format!("Mix {}", this.settings.saved.len() + 1)
                                        } else {
                                            name
                                        },
                                        base,
                                    });
                                    let i = this.settings.palettes().len() - 1;
                                    this.use_palette(i, w, cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("discard-palette")
                            .label("Discard")
                            .rack(t)
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.use_palette(this.settings.active, w, cx)
                            })),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(label("REFRESH", t))
                    .children([1, 2, 3, 5, 10].into_iter().map(|seconds| {
                        Button::new(("interval", seconds as usize))
                            .label(format!("{seconds}s"))
                            .rack(t)
                            .small()
                            .when(self.interval == seconds, |b| b.primary())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.interval != seconds {
                                    this.interval = seconds;
                                    this.wake();
                                    cx.notify();
                                }
                            }))
                    })),
            )
            .child(
                Button::new("close-settings")
                    .label("Close · Esc")
                    .rack(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings_open = false;
                        cx.notify();
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{vfd_digit_width, UNIT_GLYPHS};
    #[test]
    fn settings_escape_closes_from_button_focus_but_keeps_text_editing_behavior() {
        assert!(super::settings_escape_closes("escape", true, false));
        assert!(!super::settings_escape_closes("escape", true, true));
        assert!(!super::settings_escape_closes("enter", true, false));
    }

    #[test]
    fn unit_geometry_retention_is_bounded_when_positions_change() {
        for x in 0..100 {
            super::unit_paths(
                gpui::Bounds::new(
                    gpui::point(gpui::px(x as f32), gpui::px(0.)),
                    gpui::size(gpui::px(10.), gpui::px(14.)),
                ),
                '%',
            );
        }
        assert!(super::UNIT_PATHS.get().unwrap().lock().unwrap().len() <= 32);
    }

    #[test]
    fn vfd_unit_glyphs_match_the_5_by_7_design() {
        assert_eq!(
            UNIT_GLYPHS,
            [
                (
                    '%',
                    ["11000", "11001", "00010", "00100", "01000", "10011", "00011"]
                ),
                (
                    '°',
                    ["01100", "10010", "10010", "01100", "00000", "00000", "00000"]
                ),
            ]
        );
    }

    #[test]
    fn vfd_digit_box_keeps_three_and_four_character_values_on_one_line() {
        assert!((vfd_digit_width(42., 3) - 90.72).abs() < 0.001);
        assert!((vfd_digit_width(22., 4) - 63.36).abs() < 0.001);
    }
}
