use crate::{
    history::{HistoryView, Metric},
    model::{bytes, duration, ProcessColumn},
    palette::{self, Tokens},
    DrawMetrics, Monitor,
};
use gpui::{prelude::*, *};
use gpui_component::{
    button::{Button, ButtonVariants},
    input::Input,
    Sizable,
};
use gtop_core::{CpuBanks, CpuTopology};
use std::{
    cell::RefCell,
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

// These match Doto's 9 ink span; the 5x7 canvas keeps its 10:14 ratio.
const UNIT_GLYPH_TOP: f32 = 0.155;
const UNIT_GLYPH_HEIGHT: f32 = 0.69;
const UNIT_GLYPH_WIDTH_RATIO: f32 = 10. / 14.;

#[derive(Clone, Copy)]
struct UnitGlyphBounds {
    top: f32,
    width: f32,
    height: f32,
}

fn unit_glyph_bounds(size: f32) -> UnitGlyphBounds {
    let height = size * UNIT_GLYPH_HEIGHT;
    UnitGlyphBounds {
        top: size * UNIT_GLYPH_TOP,
        width: height * UNIT_GLYPH_WIDTH_RATIO,
        height,
    }
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

#[cfg(test)]
static UNIT_GLYPH_TEST_BOUNDS: Mutex<Vec<Bounds<Pixels>>> = Mutex::new(Vec::new());

fn core_time(seconds: f64) -> String {
    let minutes = seconds / 60.;
    if minutes >= 60. {
        format!("{} h {:02} min", minutes as u64 / 60, minutes as u64 % 60)
    } else {
        format!("{minutes:.1} min")
    }
}

fn history_row() -> Div {
    div().w_full().h(px(22.)).flex().items_center()
}

fn history_cell(width: f32, right: bool) -> Div {
    crate::column_widths::cell(width, right)
}

pub fn timestamp(seconds: u64, format: &str) -> String {
    chrono::DateTime::from_timestamp(seconds as i64, 0)
        .map(|t| t.with_timezone(&chrono::Local).format(format).to_string())
        .unwrap_or_else(|| "Unavailable".into())
}
pub fn now() -> f64 {
    #[cfg(test)]
    if crate::draw_benchmark::fixed_clock() {
        return 1_800_000_000.;
    }
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

fn draw_readout_text(metrics: DrawMetrics) -> String {
    format!(
        "CPU DRAW LAST {} MS · {} FPS / 1S",
        metrics
            .mean_ms
            .map(|ms| format!("{ms:.2}"))
            .unwrap_or_else(|| "N/A".into()),
        metrics
            .fps
            .map(|fps| format!("{fps:.1}"))
            .unwrap_or_else(|| "N/A".into())
    )
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
        .child(crate::glow::text(text, hot, t))
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
    let bounds = unit_glyph_bounds(size);
    let glow = !t.light;
    div().ml(px(size * 0.12)).flex_shrink_0().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                #[cfg(test)]
                if !crate::draw_benchmark::fixed_clock() {
                    UNIT_GLYPH_TEST_BOUNDS.lock().unwrap().push(bounds);
                }
                let (lit, ghost_path) = unit_paths(bounds, unit);
                window.paint_path(ghost_path, rgb(ghost));
                if glow {
                    let scale = f32::from(bounds.size.height) / 14.;
                    let rows = UNIT_GLYPHS
                        .iter()
                        .find(|(glyph, _)| *glyph == unit)
                        .unwrap()
                        .1;
                    for (y, row) in rows.iter().enumerate() {
                        for (x, bit) in row.bytes().enumerate() {
                            if bit == b'1' {
                                let radius = px(scale * 0.82);
                                let center = bounds.origin
                                    + point(
                                        px((x as f32 * 2. + 1.) * scale),
                                        px((y as f32 * 2. + 1.) * scale),
                                    );
                                window.paint_drop_shadows(
                                    Bounds::new(
                                        center - point(radius, radius),
                                        gpui::size(radius * 2., radius * 2.),
                                    ),
                                    Corners::all(radius),
                                    &[crate::glow::shadow(led, 8., if hot { 166 } else { 153 })],
                                );
                            }
                        }
                    }
                }
                window.paint_path(lit, rgb(led));
            },
        )
        .mt(px(bounds.top))
        .w(px(bounds.width))
        .h(px(bounds.height)),
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
        .items_start()
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
                .child(crate::glow::text(value, hot, t).whitespace_nowrap()),
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

const WIN_DOT_CACHE_SLOTS: usize = 32;
const WIN_DOT_CACHE_MAX_BYTES: usize = 16 * 1024 * 1024;
const WIN_DOT_CACHE_MAX_VERTICES: usize = 524_288;
const WIN_DOT_CACHE_MAX_ENTRY_VERTICES: usize = 131_072;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WinDotKey {
    x_bits: u32,
    y_bits: u32,
    width_bits: u32,
    height_bits: u32,
}

impl WinDotKey {
    fn from_bounds(bounds: Bounds<Pixels>) -> Option<Self> {
        let x = f32::from(bounds.origin.x);
        let y = f32::from(bounds.origin.y);
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        if [x, y, width, height].into_iter().all(f32::is_finite) {
            Some(Self {
                x_bits: x.to_bits(),
                y_bits: y.to_bits(),
                width_bits: width.to_bits(),
                height_bits: height.to_bits(),
            })
        } else {
            None
        }
    }
}

struct WinDotEntry {
    key: WinDotKey,
    path: Option<Path<Pixels>>,
}

struct WinDotCache {
    entries: [Option<WinDotEntry>; WIN_DOT_CACHE_SLOTS],
}

enum WinDotAdmission {
    Retained(Option<Path<Pixels>>),
    Bypassed(Option<Path<Pixels>>),
}

impl WinDotCache {
    fn new() -> Self {
        Self {
            entries: std::array::from_fn(|_| None),
        }
    }

    fn get_clone(&mut self, key: WinDotKey) -> Option<Option<Path<Pixels>>> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|entry| entry.key == key))?;
        let entry = self.entries[index].take().expect("located cache entry");
        let path = entry.path.clone();
        for position in (1..=index).rev() {
            self.entries[position] = self.entries[position - 1].take();
        }
        self.entries[0] = Some(entry);
        Some(path)
    }

    fn retained_usage(&self) -> Option<(usize, usize)> {
        self.entries.iter().flatten().try_fold(
            (0usize, std::mem::size_of::<RefCell<Self>>()),
            |(vertices, bytes), entry| {
                let capacity = entry
                    .path
                    .as_ref()
                    .map_or(0, |path| path.vertices.capacity());
                let next_vertices = vertices.checked_add(capacity)?;
                let entry_bytes = capacity.checked_mul(std::mem::size_of::<PathVertex<Pixels>>())?;
                let next_bytes = bytes.checked_add(entry_bytes)?;
                Some((next_vertices, next_bytes))
            },
        )
    }

    fn evict_lru(&mut self) -> bool {
        let Some(index) = self.entries.iter().rposition(Option::is_some) else {
            return false;
        };
        self.entries[index] = None;
        true
    }

    fn admit_and_clone(
        &mut self,
        key: WinDotKey,
        path: Option<Path<Pixels>>,
        max_bytes: usize,
        max_vertices: usize,
        max_entry_vertices: usize,
    ) -> WinDotAdmission {
        let capacity = path.as_ref().map_or(0, |path| path.vertices.capacity());
        let Some(entry_bytes) = capacity.checked_mul(std::mem::size_of::<PathVertex<Pixels>>())
        else {
            return WinDotAdmission::Bypassed(path);
        };
        let Some(entry_bytes_with_metadata) =
            std::mem::size_of::<RefCell<Self>>().checked_add(entry_bytes)
        else {
            return WinDotAdmission::Bypassed(path);
        };
        if capacity > max_entry_vertices
            || capacity > max_vertices
            || entry_bytes_with_metadata > max_bytes
        {
            return WinDotAdmission::Bypassed(path);
        }

        loop {
            let Some((retained_vertices, retained_bytes)) = self.retained_usage() else {
                if !self.evict_lru() {
                    return WinDotAdmission::Bypassed(path);
                }
                continue;
            };
            let fits = retained_vertices
                .checked_add(capacity)
                .is_some_and(|total| total <= max_vertices)
                && retained_bytes
                    .checked_add(entry_bytes)
                    .is_some_and(|total| total <= max_bytes);
            let has_slot = self.entries.iter().any(Option::is_none);
            if fits && has_slot {
                break;
            }
            if !self.evict_lru() {
                return WinDotAdmission::Bypassed(path);
            }
        }

        for position in (1..WIN_DOT_CACHE_SLOTS).rev() {
            self.entries[position] = self.entries[position - 1].take();
        }
        let paint_path = path.clone();
        self.entries[0] = Some(WinDotEntry { key, path });
        WinDotAdmission::Retained(paint_path)
    }
}

thread_local! {
    static WIN_DOT_CACHE: RefCell<WinDotCache> = RefCell::new(WinDotCache::new());
}

fn build_win_dots(bounds: Bounds<Pixels>) -> Option<Path<Pixels>> {
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
    path.build().ok()
}

fn cached_win_dots_with(
    cache: &RefCell<WinDotCache>,
    bounds: Bounds<Pixels>,
    build: impl FnOnce(Bounds<Pixels>) -> Option<Path<Pixels>>,
) -> Option<Path<Pixels>> {
    let Some(key) = WinDotKey::from_bounds(bounds) else {
        return build(bounds);
    };

    let cached = { cache.borrow_mut().get_clone(key) };
    if let Some(outcome) = cached {
        return outcome;
    }

    let path = build(bounds);
    match cache.borrow_mut().admit_and_clone(
        key,
        path,
        WIN_DOT_CACHE_MAX_BYTES,
        WIN_DOT_CACHE_MAX_VERTICES,
        WIN_DOT_CACHE_MAX_ENTRY_VERTICES,
    ) {
        WinDotAdmission::Retained(path) | WinDotAdmission::Bypassed(path) => path,
    }
}

fn cached_win_dots(cache: &RefCell<WinDotCache>, bounds: Bounds<Pixels>) -> Option<Path<Pixels>> {
    cached_win_dots_with(cache, bounds, build_win_dots)
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
                    let path = WIN_DOT_CACHE.with(|cache| cached_win_dots(cache, bounds));
                    if let Some(path) = path {
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
    let active = segment_fill(value, n);
    div()
        .flex()
        .gap(px(CPU_SEGMENT_GAP))
        .h(px(8.))
        .w_full()
        .children((0..n).map(|i| {
            let color = if i as f32 >= n as f32 * 0.85 {
                t.base.hot
            } else {
                t.base.led
            };
            div()
                .flex_1()
                .h_full()
                .bg(rgb(if i < active {
                    color
                } else {
                    palette::mix(t.base.led, t.base.ground, 0.12)
                }))
                .when(i < active && !t.light, |bar| {
                    bar.shadow(vec![crate::glow::shadow(t.base.led, 4., 153)])
                })
        }))
}

fn segment_fill(value: f32, n: usize) -> usize {
    if !value.is_finite() || n == 0 {
        return 0;
    }
    (value.clamp(0., 100.) / 100. * n as f32).round() as usize
}

const CPU_SEGMENT_GAP: f32 = 2.;
const CPU_OVERFLOW_WIDTH: f32 = 11.;
const CPU_OVERFLOW_GAP: f32 = 2.;
const CPU_MIN_SEGMENT_WIDTH: f32 = 1.;

fn cpu_segment_count(n: usize, width: f32, overflow: bool) -> usize {
    let reserved = if overflow {
        CPU_OVERFLOW_WIDTH + CPU_OVERFLOW_GAP
    } else {
        0.
    };
    let available = (width - reserved).max(0.);
    let fit = ((available + CPU_SEGMENT_GAP) / (CPU_MIN_SEGMENT_WIDTH + CPU_SEGMENT_GAP)).floor()
        as usize;
    n.min(fit)
}

pub fn cpu_segments(value: f32, n: usize, width: f32, height: f32, t: &Tokens) -> Div {
    let overflow = if value.is_finite() && value > 100. {
        ((value / 100.).ceil() as usize).saturating_sub(1).min(4)
    } else {
        0
    };
    let segment_count = cpu_segment_count(n, width, overflow > 0);
    div()
        .flex()
        .items_center()
        .gap(px(CPU_OVERFLOW_GAP))
        .w(px(width))
        .h(px(height))
        .child(
            segments(value, segment_count, t)
                .flex_1()
                .min_w_0()
                .h(px(height)),
        )
        .when(overflow > 0, |bar| {
            bar.child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .gap(px(1.))
                    .children((0..4).map(|index| {
                        let active = index < overflow;
                        div()
                            .w(px(2.))
                            .h(px(height))
                            .bg(rgb(if active { t.base.hot } else { t.seg_off }))
                            .when(active && !t.light, |tick| {
                                tick.shadow(vec![crate::glow::shadow(t.base.hot, 4., 153)])
                            })
                    })),
            )
        })
}

#[cfg(test)]
mod cpu_segment_tests {
    use super::segment_fill;

    #[test]
    fn cpu_bar_fill_uses_one_core_as_full_scale() {
        assert_eq!(segment_fill(0., 18), 0);
        assert_eq!(segment_fill(54., 18), 10);
        assert_eq!(segment_fill(100., 18), 18);
        assert_eq!(segment_fill(104., 18), 18);
        assert_eq!(segment_fill(400., 18), 18);
        assert_eq!(segment_fill(f32::NAN, 18), 0);
        assert_eq!(segment_fill(-4., 18), 0);
    }
}

fn core_label(prefix: &str, number: usize) -> String {
    format!("{prefix}{}", number + 1)
}

const CORE_BANK_ROW_WIDTH: f32 = 248.;
const CORE_BANK_MIN_WIDTH: f32 = 96.;
const CORE_BANK_FLEX_BASIS: f32 = 40.;
const CORE_BANK_GAP: f32 = 8.;
const CORE_CELL_GAP: f32 = 2.;
const CORE_LABEL_GLYPH_WIDTH: f32 = 4.8;

fn core_label_stride(prefix: &str, count: usize, bank_width: f32) -> usize {
    if count == 0 {
        return 1;
    }

    let label_width = core_label(prefix, count - 1).len() as f32 * CORE_LABEL_GLYPH_WIDTH;
    let cell_width = (bank_width - CORE_CELL_GAP * (count - 1) as f32) / count as f32;
    let label_center_spacing = cell_width + CORE_CELL_GAP;
    (label_width / label_center_spacing).ceil().max(1.) as usize
}

fn hybrid_bank_widths(performance_count: usize, efficiency_count: usize) -> (f32, f32) {
    let total_count = (performance_count + efficiency_count).max(1) as f32;
    let available_width = CORE_BANK_ROW_WIDTH - CORE_BANK_GAP;
    let free_space = available_width - 2. * CORE_BANK_FLEX_BASIS;
    let mut performance_width =
        CORE_BANK_FLEX_BASIS + free_space * performance_count as f32 / total_count;
    let mut efficiency_width = available_width - performance_width;

    if performance_width < CORE_BANK_MIN_WIDTH {
        performance_width = CORE_BANK_MIN_WIDTH;
        efficiency_width = available_width - performance_width;
    } else if efficiency_width < CORE_BANK_MIN_WIDTH {
        efficiency_width = CORE_BANK_MIN_WIDTH;
        performance_width = available_width - efficiency_width;
    }

    (performance_width, efficiency_width)
}

fn core_bank(
    name: &str,
    prefix: &str,
    values: &[(usize, f32)],
    bank_width: f32,
    t: &Tokens,
) -> Div {
    let average = values.iter().map(|(_, value)| value).sum::<f32>() / values.len().max(1) as f32;
    let label_stride = core_label_stride(prefix, values.len(), bank_width);
    div()
        .flex_1()
        .flex_basis(px(CORE_BANK_FLEX_BASIS))
        .flex_grow(values.len() as f32)
        .min_w(px(CORE_BANK_MIN_WIDTH))
        .child(
            div()
                .flex()
                .justify_between()
                .child(label(name, t))
                .child(label(format!("{average:.0}%"), t)),
        )
        .child(div().flex().items_end().gap(px(CORE_CELL_GAP)).children(
            values.iter().enumerate().map(|(number, (_, value))| {
                let active = (value.clamp(0., 100.) / 100. * 12.).round() as usize;
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().flex().flex_col().gap(px(1.)).h(px(36.)).children(
                        (0..12).rev().map(|segment| {
                            div()
                                .flex_1()
                                .bg(rgb(if *value > 90. && segment >= 9 {
                                    t.base.hot
                                } else if segment < active {
                                    t.base.led
                                } else {
                                    palette::mix(t.base.led, t.base.ground, 0.12)
                                }))
                                .when(segment < active && !t.light, |bar| {
                                    bar.shadow(vec![if *value > 90. && segment >= 9 {
                                        crate::glow::shadow(t.base.hot, 4., 153)
                                    } else {
                                        crate::glow::shadow(t.base.led, 4., 140)
                                    }])
                                })
                        }),
                    ))
                    .child(
                        div()
                            .text_center()
                            .font_family("JetBrains Mono")
                            .text_size(px(8.))
                            .whitespace_nowrap()
                            .text_color(rgb(t.silk_dim))
                            .child(if number % label_stride == 0 {
                                core_label(prefix, number)
                            } else {
                                String::new()
                            }),
                    )
            }),
        ))
}

fn core_banks(usage: &[f32], topology: &CpuTopology, t: &Tokens) -> Div {
    let mut banks = div().flex().gap(px(CORE_BANK_GAP));
    match topology.banks(usage) {
        CpuBanks::Hybrid {
            performance,
            efficiency,
        } => {
            let (performance_width, efficiency_width) =
                hybrid_bank_widths(performance.len(), efficiency.len());
            banks = banks
                .child(core_bank(
                    "P CORES",
                    "P",
                    &performance,
                    performance_width,
                    t,
                ))
                .child(core_bank("E CORES", "E", &efficiency, efficiency_width, t));
        }
        CpuBanks::Cores(cores) => {
            banks = banks.child(core_bank(
                "CORES",
                "",
                &cores.iter().copied().enumerate().collect::<Vec<_>>(),
                CORE_BANK_ROW_WIDTH,
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
                    .when(!t.light, |bar| {
                        bar.shadow(vec![if selected {
                            crate::glow::shadow(t.base.silk, 8., 153)
                        } else {
                            crate::glow::shadow(t.base.led, 3., 115)
                        }])
                    })
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
                let row_text = |text: String| {
                    if index < 3 {
                        crate::glow::text(text, false, t).into_any_element()
                    } else {
                        text.into_any_element()
                    }
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
                    .child(
                        div()
                            .w(px(142.))
                            .min_w_0()
                            .truncate()
                            .child(row_text(name.clone())),
                    )
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
                            .child(row_text(if exited {
                                format!("{value_label} · exited")
                            } else {
                                value_label
                            })),
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

    fn tower(&self, t: &Tokens, cx: &mut Context<Self>) -> Stateful<Div> {
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
                div().px(px(4.)).pb(px(2.)).child(
                    label(
                        self.snapshot
                            .as_ref()
                            .and_then(|s| s.stats.gpus.first())
                            .map(|g| g.name.to_uppercase())
                            .unwrap_or_else(|| "SYSTEM MONITOR".into()),
                        t,
                    )
                    .text_color(rgb(t.silk_dim)),
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
        tower = tower.child(
            win(t)
                .id("disk-open")
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| this.open_disk(window, cx)))
                .child(label("SSD GB", t))
                .child(lit(
                    if stats.disk_total_bytes > 0 {
                        format!(
                            "{:.0} / {:.0}",
                            stats.disk_used_bytes as f64 / 1e9,
                            stats.disk_total_bytes as f64 / 1e9
                        )
                    } else {
                        "N/A".into()
                    },
                    22.,
                    stats.disk_used_bytes as f64 / stats.disk_total_bytes.max(1) as f64 > 0.92,
                    t,
                ))
                .child(segments(
                    stats.disk_used_bytes as f32 / stats.disk_total_bytes.max(1) as f32 * 100.,
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
                        label(draw_readout_text(self.draw_metrics), t).text_color(rgb(t.silk_dim)),
                    ),
            )
            .child(div().flex_1())
            .child(label(format!("UP {}", duration(stats.uptime)), t).whitespace_nowrap())
            .child(
                label(
                    format!(
                        "NET ↓ {}/s ↑ {}/s",
                        bytes(stats.network_rx_bytes),
                        bytes(stats.network_tx_bytes)
                    ),
                    t,
                )
                .whitespace_nowrap(),
            )
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
            use crate::column_widths::{self, Column, Drag, HISTORY};
            let widths = self
                .settings
                .history_widths
                .compute(&HISTORY, self.history_columns.viewport);
            let total = widths.iter().sum::<f32>() + 2.;
            let events = column_widths::events(
                cx,
                self.history_columns.drag.is_some(),
                |this, width, _, cx| {
                    let width = (*width - 2.).max(0.);
                    if (this.history_columns.viewport - width).abs() > 0.5 {
                        this.history_columns.viewport = width;
                        cx.notify();
                    }
                },
                |this, event, _, cx| {
                    if let Some(drag) = this.history_columns.drag {
                        this.settings.history_widths.set(
                            drag.column,
                            drag.start_width + f32::from(event.position.x) - drag.start_x,
                        );
                        cx.stop_propagation();
                        cx.notify();
                    }
                },
                |this, _, window, cx| {
                    if this.history_columns.drag.take().is_some() {
                        this.persist_settings(window, cx);
                        cx.stop_propagation();
                        cx.notify();
                    }
                },
            );
            let headers = history_row()
                .h(px(29.))
                .flex_shrink_0()
                .bg(rgb(t.base.plate))
                .child(div().w(px(22.)).flex_shrink_0())
                .children(
                    [
                        ("PROCESS", Column::Name, false),
                        ("CORE TIME ▼", Column::CoreTime, true),
                        ("PEAK CPU %", Column::Share, false),
                        ("OF ALL CPU", Column::CpuShare, true),
                        ("GPU TIME", Column::GpuTime, true),
                        ("PEAK MEMORY", Column::PeakMemory, true),
                        ("PEAK CPU %", Column::PeakCpu, true),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (name, column, right))| {
                        history_cell(widths[index + 1], right)
                            .relative()
                            .child(
                                label(name, &t).text_color(rgb(if column == Column::CoreTime {
                                    t.base.led
                                } else {
                                    t.silk_dim
                                })),
                            )
                            .child(column_widths::handle().on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    if event.click_count == 2 {
                                        this.history_columns.drag = None;
                                        this.settings.history_widths.reset(column);
                                        this.persist_settings(window, cx);
                                    } else {
                                        let widths = this
                                            .settings
                                            .history_widths
                                            .compute(&HISTORY, this.history_columns.viewport);
                                        this.history_columns.drag = Some(Drag {
                                            column,
                                            start_x: event.position.x.into(),
                                            start_width: widths[index + 1],
                                        });
                                    }
                                    cx.notify();
                                }),
                            ))
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
                        .id("history-table-viewport")
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .overflow_x_scroll()
                        .child(events)
                        .child(
                            div()
                                .w(px(total))
                                .h_full()
                                .flex()
                                .flex_col()
                                .border_1()
                                .border_color(rgb(t.rule))
                                .child(headers)
                                .child(
                                    uniform_list(
                                        "history-rows",
                                        h.rows.len(),
                                        cx.processor(
                                            move |this, range: std::ops::Range<usize>, _, cx| {
                                                let widths = this.settings.history_widths.compute(
                                                    &HISTORY,
                                                    this.history_columns.viewport,
                                                );
                                                let h = &this.history[index];
                                                let t = this
                                                    .draft
                                                    .unwrap_or(this.settings.base())
                                                    .tokens();
                                                range
                                                    .filter_map(|row| {
                                                        let (name, p) = h.rows.get(row)?;
                                                        let selected = this
                                                            .history_selected
                                                            .as_ref()
                                                            .map(|n| n == name)
                                                            .unwrap_or(row == 0);
                                                        let key = name.clone();
                                                        let row_text = |text: String| {
                                                            if selected || row < 3 {
                                                                crate::glow::text(text, false, &t)
                                                                    .into_any_element()
                                                            } else {
                                                                text.into_any_element()
                                                            }
                                                        };
                                                        Some(
                                                            history_row()
                                                                .id(("history-row", row))
                                                                .bg(rgb(if selected {
                                                                    t.plate2
                                                                } else {
                                                                    t.base.ground
                                                                }))
                                                                .on_click(cx.listener(
                                                                    move |this, _, _, cx| {
                                                                        this.history_selected =
                                                                            Some(key.clone());
                                                                        cx.notify();
                                                                    },
                                                                ))
                                                                .border_b_1()
                                                                .border_color(rgb(t.row_rule))
                                                                .text_color(rgb(
                                                                    if selected || row < 3 {
                                                                        t.base.led
                                                                    } else {
                                                                        t.ink_row
                                                                    },
                                                                ))
                                                                .child(
                                                                    div()
                                                                        .w(px(22.))
                                                                        .flex_shrink_0()
                                                                        .px(px(10.))
                                                                        .child(
                                                                            lamp(
                                                                                t.base.led,
                                                                                row < 3,
                                                                                &t,
                                                                            )
                                                                            .w(px(7.))
                                                                            .h(px(12.)),
                                                                        ),
                                                                )
                                                                .child(
                                                                    history_cell(widths[1], false)
                                                                        .truncate()
                                                                        .child(row_text(
                                                                            name.clone(),
                                                                        )),
                                                                )
                                                                .child(
                                                                    history_cell(widths[2], true)
                                                                        .child(row_text(
                                                                            core_time(
                                                                                p.cpu_seconds,
                                                                            ),
                                                                        )),
                                                                )
                                                                .child(
                                                                    history_cell(widths[3], false)
                                                                        .child(cpu_segments(
                                                                            p.peak_cpu,
                                                                            30,
                                                                            (widths[3] - 20.)
                                                                                .min(120.),
                                                                            8.,
                                                                            &t,
                                                                        )),
                                                                )
                                                                .child(
                                                                    history_cell(widths[4], true)
                                                                        .child(row_text(format!(
                                                                            "{:.1}%",
                                                                            p.cpu_seconds
                                                                                / h.total_cpu
                                                                                    .max(0.001)
                                                                                * 100.
                                                                        ))),
                                                                )
                                                                .child(
                                                                    history_cell(widths[5], true)
                                                                        .child(row_text(
                                                                            if p.gpu_seconds > 3. {
                                                                                core_time(
                                                                                    p.gpu_seconds,
                                                                                )
                                                                            } else {
                                                                                "·".into()
                                                                            },
                                                                        )),
                                                                )
                                                                .child(
                                                                    history_cell(widths[6], true)
                                                                        .child(row_text(bytes(
                                                                            p.peak_memory,
                                                                        ))),
                                                                )
                                                                .child(
                                                                    history_cell(widths[7], true)
                                                                        .child(row_text(format!(
                                                                            "{:.1}",
                                                                            p.peak_cpu
                                                                        ))),
                                                                ),
                                                        )
                                                    })
                                                    .collect::<Vec<_>>()
                                            },
                                        ),
                                    )
                                    .track_scroll(&self.history_scroll)
                                    .flex_1()
                                    .min_h_0(),
                                ),
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
                            ("D", "disk"),
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
                                    this.panel_resize.finish();
                                    if let Some(disk) = &mut this.disk {
                                        disk.close();
                                    }
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
            .child(self.tower(&t, cx))
            .child(right);
        if self.settings_open {
            root = root.child(self.palette_panel(&t, cx));
        } else if self.moment.is_some() {
            root = root.child(self.moment_panel(&t, cx));
        } else if let Some(disk) = self.disk_panel(cx) {
            root = root.child(disk.w(px(self.settings.detail_width.pixels())).h_full());
        } else if let Some(details) = self.details(cx) {
            root = root.child(details.h_full());
        }
        root
    }

    pub(super) fn panel_resize_handle(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        crate::column_widths::handle_left().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                if event.click_count == 2 {
                    this.settings.detail_width = this
                        .panel_resize
                        .begin(true, event.position.x.into(), this.settings.detail_width)
                        .unwrap();
                    this.persist_settings(window, cx);
                } else {
                    this.panel_resize.begin(
                        false,
                        event.position.x.into(),
                        this.settings.detail_width,
                    );
                }
                cx.notify();
            }),
        )
    }

    pub(super) fn panel_resize_events(&self, cx: &mut Context<Self>) -> impl IntoElement {
        crate::column_widths::events(
            cx,
            self.panel_resize.is_dragging(),
            |_, _, _, _| {},
            |this, event, _, cx| {
                if let Some(width) = this.panel_resize.width_at(event.position.x.into()) {
                    this.settings.detail_width = width;
                    cx.stop_propagation();
                    cx.notify();
                }
            },
            |this, _, window, cx| {
                if this.panel_resize.finish() {
                    this.persist_settings(window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            },
        )
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
    pub(crate) fn persist_settings(&self, _window: &mut Window, _cx: &mut Context<Self>) {
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
        if let Some(disk) = self
            .disk
            .as_mut()
            .filter(|d| d.open && !self.settings_open && self.moment.is_none())
        {
            if disk.key(k) {
                cx.stop_propagation();
                cx.notify();
                return;
            }
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
            "d" => self.open_disk(window, cx),
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
                    self.panel_resize.finish();
                    if let Some(disk) = &mut self.disk {
                        disk.close();
                    }
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
                    self.panel_resize.finish();
                    crate::details::close_details(&mut self.details);
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
                        let alive = !self.process_exited();
                        self.details.as_mut().unwrap().request_kill(key, alive);
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
    use super::{
        cpu_segments, div, draw_readout_text, history_cell, lit_unit, vfd_digit_width, Bounds,
        DrawMetrics, Path, Pixels, Tokens, UNIT_GLYPHS, UNIT_GLYPH_TEST_BOUNDS,
        WIN_DOT_CACHE_MAX_BYTES, WIN_DOT_CACHE_MAX_ENTRY_VERTICES, WIN_DOT_CACHE_MAX_VERTICES,
        WIN_DOT_CACHE_SLOTS,
    };
    use gpui::{prelude::*, px, TestAppContext, Window};

    struct UnitLayoutTestView {
        tokens: Tokens,
    }

    impl Render for UnitLayoutTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().flex().flex_col().children(
                [(9., '%'), (15., '°'), (22., '%'), (42., '°')]
                    .into_iter()
                    .map(|(size, unit)| lit_unit("99".into(), unit, size, false, &self.tokens)),
            )
        }
    }

    struct CpuMeterLayoutTestView {
        tokens: Tokens,
    }

    impl Render for CpuMeterLayoutTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_col()
                .gap(px(20.))
                .child(cpu_segments(54., 18, 78., 12., &self.tokens))
                .child(
                    history_cell(75., true)
                        .overflow_hidden()
                        .child(cpu_segments(104., 30, 55., 8., &self.tokens)),
                )
        }
    }

    #[test]
    fn core_labels_number_each_bank_from_one() {
        assert_eq!(super::core_label("P", 0), "P1");
        assert_eq!(super::core_label("P", 9), "P10");
        assert_eq!(super::core_label("E", 0), "E1");
        assert_eq!(super::core_label("E", 3), "E4");
        assert_eq!(super::core_label("", 0), "1");
    }

    #[test]
    fn core_label_stride_keeps_labels_inside_their_cells() {
        assert_eq!(super::core_label_stride("P", 12, 144.), 2);
        assert_eq!(super::core_label_stride("P", 24, 144.), 3);
        assert_eq!(super::core_label_stride("E", 4, 96.), 1);
        assert_eq!(super::core_label_stride("", 0, 248.), 1);
    }

    fn doto_nine_ink_span(size: f32) -> (f32, f32) {
        let font = ttf_parser::Face::parse(include_bytes!("../assets/fonts/DotoDisplay.ttf"), 0)
            .expect("Doto Display font");
        let units_per_em = f32::from(font.units_per_em());
        let ascent = f32::from(font.ascender()) / units_per_em;
        let descent = f32::from(font.descender()) / units_per_em;
        let line_height = 1.;
        let leading = (line_height - (ascent - descent)) / 2.;
        let baseline = leading + ascent;
        let digit = font.glyph_index('9').expect("Doto Display 9 glyph");
        let ink = font.glyph_bounding_box(digit).expect("Doto Display 9 ink");
        (
            size * (baseline - f32::from(ink.y_max) / units_per_em),
            size * (baseline - f32::from(ink.y_min) / units_per_em),
        )
    }

    fn original_win_dots(bounds: Bounds<Pixels>) -> Option<Path<Pixels>> {
        let mut path = gpui::PathBuilder::fill();
        for x in (0..f32::from(bounds.size.width) as usize).step_by(3) {
            for y in (0..f32::from(bounds.size.height) as usize).step_by(3) {
                let p = bounds.origin + gpui::point(gpui::px(x as f32), gpui::px(y as f32));
                path.move_to(p);
                path.line_to(p + gpui::point(gpui::px(0.8), gpui::px(0.)));
                path.line_to(p + gpui::point(gpui::px(0.8), gpui::px(0.8)));
                path.line_to(p + gpui::point(gpui::px(0.), gpui::px(0.8)));
                path.close();
            }
        }
        path.build().ok()
    }

    fn point_bits(point: gpui::Point<Pixels>) -> [u32; 2] {
        [f32::from(point.x).to_bits(), f32::from(point.y).to_bits()]
    }

    fn bounds_bits(bounds: Bounds<Pixels>) -> [u32; 4] {
        [
            f32::from(bounds.origin.x).to_bits(),
            f32::from(bounds.origin.y).to_bits(),
            f32::from(bounds.size.width).to_bits(),
            f32::from(bounds.size.height).to_bits(),
        ]
    }

    fn assert_same_win_dot_path(actual: Option<Path<Pixels>>, expected: Option<Path<Pixels>>) {
        match (actual, expected) {
            (None, None) => {}
            (Some(actual), Some(expected)) => {
                assert_eq!(actual.id, expected.id);
                assert_eq!(actual.order, expected.order);
                assert_eq!(bounds_bits(actual.bounds), bounds_bits(expected.bounds));
                assert_eq!(
                    bounds_bits(actual.content_mask.bounds),
                    bounds_bits(expected.content_mask.bounds)
                );
                assert_eq!(actual.vertices.len(), expected.vertices.len());
                for (actual, expected) in actual.vertices.iter().zip(&expected.vertices) {
                    assert_eq!(
                        point_bits(actual.xy_position),
                        point_bits(expected.xy_position)
                    );
                    assert_eq!(
                        [
                            actual.st_position.x.to_bits(),
                            actual.st_position.y.to_bits()
                        ],
                        [
                            expected.st_position.x.to_bits(),
                            expected.st_position.y.to_bits()
                        ]
                    );
                    assert_eq!(
                        bounds_bits(actual.content_mask.bounds),
                        bounds_bits(expected.content_mask.bounds)
                    );
                }
                assert_eq!(actual.color, expected.color);
                assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
            }
            (actual, expected) => panic!(
                "builder outcomes differ: actual={}, expected={}",
                actual.is_some(),
                expected.is_some()
            ),
        }
    }

    fn dot_bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(
            gpui::point(gpui::px(x), gpui::px(y)),
            gpui::size(gpui::px(width), gpui::px(height)),
        )
    }

    fn dot_capacity(path: &Path<Pixels>) -> usize {
        path.vertices.capacity()
    }

    #[test]
    fn win_dot_builder_matches_the_original_geometry_and_outcomes() {
        let adjacent = f32::from_bits(3.0f32.to_bits() + 1);
        let bounds = [
            dot_bounds(11.25, -7.5, 10.5, 7.25),
            dot_bounds(-0.0, 0.0, 3.0, 3.0),
            dot_bounds(0.0, 0.0, 0.5, 0.75),
            dot_bounds(-4.0, 2.0, -3.0, 6.0),
            dot_bounds(0.25, 0.5, 1.0, adjacent),
            dot_bounds(1.0, -1.0, 4.0, 6.0),
            dot_bounds(0.0, 0.0, 500.0, 500.0),
        ];

        let empty = dot_bounds(0.0, 0.0, 0.5, 0.75);
        let empty_cache = std::cell::RefCell::new(super::WinDotCache::new());
        assert_same_win_dot_path(
            super::cached_win_dots(&empty_cache, empty),
            original_win_dots(empty),
        );
        assert!(empty_cache
            .borrow()
            .entries
            .iter()
            .flatten()
            .next()
            .expect("successful empty paths are retained")
            .path
            .as_ref()
            .expect("empty geometry is a successful path")
            .vertices
            .is_empty());
        let overflow = dot_bounds(0.0, 0.0, 500.0, 500.0);
        assert!(super::build_win_dots(overflow).is_none());
        let failed_cache = std::cell::RefCell::new(super::WinDotCache::new());
        assert!(super::cached_win_dots(&failed_cache, overflow).is_none());
        assert_eq!(
            failed_cache.borrow().entries.iter().flatten().count(),
            1,
            "failed builds occupy a cache slot"
        );
        assert!(matches!(
            failed_cache
                .borrow_mut()
                .get_clone(super::WinDotKey::from_bounds(overflow).unwrap()),
            Some(None)
        ));

        let build_count = std::cell::Cell::new(0);
        let counted_cache = std::cell::RefCell::new(super::WinDotCache::new());
        for _ in 0..2 {
            assert!(
                super::cached_win_dots_with(&counted_cache, overflow, |bounds| {
                    build_count.set(build_count.get() + 1);
                    super::build_win_dots(bounds)
                })
                .is_none()
            );
        }
        assert_eq!(build_count.get(), 1);
        assert!(matches!(
            counted_cache
                .borrow_mut()
                .get_clone(super::WinDotKey::from_bounds(overflow).unwrap()),
            Some(None)
        ));
        assert_eq!(counted_cache.borrow().entries.iter().flatten().count(), 1);

        for bounds in bounds {
            assert_same_win_dot_path(super::build_win_dots(bounds), original_win_dots(bounds));
        }
    }

    #[test]
    fn win_dot_cache_keeps_failed_and_empty_success_outcomes_distinct() {
        let failed_bounds = dot_bounds(2.0, 3.0, 500.0, 500.0);
        let empty_bounds = dot_bounds(0.0, 0.0, 0.5, 0.75);
        let cache = std::cell::RefCell::new(super::WinDotCache::new());
        let calls = std::cell::Cell::new(0);

        assert!(super::cached_win_dots_with(&cache, failed_bounds, |_| {
            calls.set(calls.get() + 1);
            None
        })
        .is_none());
        assert!(super::cached_win_dots_with(&cache, failed_bounds, |_| {
            panic!("cached failure must not rebuild")
        })
        .is_none());
        assert_eq!(calls.get(), 1);

        assert!(super::cached_win_dots_with(&cache, empty_bounds, |bounds| {
            calls.set(calls.get() + 1);
            super::build_win_dots(bounds)
        })
        .is_some_and(|path| path.vertices.is_empty()));
        assert!(super::cached_win_dots_with(&cache, empty_bounds, |_| {
            panic!("cached empty success must not rebuild")
        })
        .is_some_and(|path| path.vertices.is_empty()));
        assert_eq!(calls.get(), 2);

        assert_eq!(cache.borrow().entries.iter().flatten().count(), 2);
        assert!(matches!(
            cache
                .borrow_mut()
                .get_clone(super::WinDotKey::from_bounds(failed_bounds).unwrap()),
            Some(None)
        ));
        assert!(cache
            .borrow()
            .entries
            .iter()
            .flatten()
            .find(|entry| entry.key == super::WinDotKey::from_bounds(empty_bounds).unwrap())
            .and_then(|entry| entry.path.as_ref())
            .is_some_and(|path| path.vertices.is_empty()));
    }

    #[test]
    fn win_dot_cache_failure_keys_preserve_exact_bits_and_bypass_nonfinite_bounds() {
        let adjacent = |value: f32| f32::from_bits(value.to_bits() + 1);
        let cases = [
            dot_bounds(1.0, 2.0, 3.0, 4.0),
            dot_bounds(adjacent(1.0), 2.0, 3.0, 4.0),
            dot_bounds(1.0, adjacent(2.0), 3.0, 4.0),
            dot_bounds(1.0, 2.0, adjacent(3.0), 4.0),
            dot_bounds(1.0, 2.0, 3.0, adjacent(4.0)),
            dot_bounds(-0.0, 2.0, 3.0, 4.0),
            dot_bounds(0.125, 2.0, 3.0, 4.0),
        ];
        let cache = std::cell::RefCell::new(super::WinDotCache::new());
        let calls = std::cell::Cell::new(0);
        for bounds in cases {
            for _ in 0..2 {
                assert!(super::cached_win_dots_with(&cache, bounds, |_| {
                    calls.set(calls.get() + 1);
                    None
                })
                .is_none());
            }
        }
        assert_eq!(calls.get(), cases.len());
        assert_eq!(cache.borrow().entries.iter().flatten().count(), cases.len());
        assert_eq!(
            cache.borrow().retained_usage(),
            Some((
                0,
                std::mem::size_of::<std::cell::RefCell<super::WinDotCache>>()
            ))
        );

        let nan = dot_bounds(f32::NAN, 0.0, 3.0, 3.0);
        let infinity = dot_bounds(0.0, f32::INFINITY, 3.0, 3.0);
        for bounds in [nan, infinity] {
            for _ in 0..2 {
                assert!(super::cached_win_dots_with(&cache, bounds, |_| {
                    calls.set(calls.get() + 1);
                    None
                })
                .is_none());
            }
        }
        assert_eq!(calls.get(), cases.len() + 4);
        assert_eq!(cache.borrow().entries.iter().flatten().count(), cases.len());
    }

    #[test]
    fn win_dot_cache_failures_share_lru_slots_and_promote_on_hit() {
        let mut cache = super::WinDotCache::new();
        let bounds = |index: usize| dot_bounds(index as f32 * 9.0, 0.0, 6.0, 6.0);
        let key = |index| super::WinDotKey::from_bounds(bounds(index)).unwrap();

        assert!(matches!(
            cache.admit_and_clone(
                key(0),
                None,
                WIN_DOT_CACHE_MAX_BYTES,
                WIN_DOT_CACHE_MAX_VERTICES,
                WIN_DOT_CACHE_MAX_ENTRY_VERTICES
            ),
            super::WinDotAdmission::Retained(None)
        ));
        let success = super::build_win_dots(bounds(1)).unwrap();
        assert!(matches!(
            cache.admit_and_clone(
                key(1),
                Some(success),
                WIN_DOT_CACHE_MAX_BYTES,
                WIN_DOT_CACHE_MAX_VERTICES,
                WIN_DOT_CACHE_MAX_ENTRY_VERTICES
            ),
            super::WinDotAdmission::Retained(Some(_))
        ));
        for index in 2..WIN_DOT_CACHE_SLOTS {
            assert!(matches!(
                cache.admit_and_clone(
                    key(index),
                    None,
                    WIN_DOT_CACHE_MAX_BYTES,
                    WIN_DOT_CACHE_MAX_VERTICES,
                    WIN_DOT_CACHE_MAX_ENTRY_VERTICES
                ),
                super::WinDotAdmission::Retained(None)
            ));
        }

        assert!(matches!(cache.get_clone(key(0)), Some(None)));
        assert_eq!(cache.entries[0].as_ref().unwrap().key, key(0));
        assert!(matches!(
            cache.admit_and_clone(
                key(WIN_DOT_CACHE_SLOTS),
                None,
                WIN_DOT_CACHE_MAX_BYTES,
                WIN_DOT_CACHE_MAX_VERTICES,
                WIN_DOT_CACHE_MAX_ENTRY_VERTICES
            ),
            super::WinDotAdmission::Retained(None)
        ));
        assert!(cache.get_clone(key(1)).is_none());
        assert!(matches!(cache.get_clone(key(0)), Some(None)));
        assert_eq!(cache.entries.iter().flatten().count(), WIN_DOT_CACHE_SLOTS);
        assert_eq!(cache.retained_usage().unwrap().0, 0);

        let calls = std::cell::Cell::new(0);
        assert!(
            super::cached_win_dots_with(&std::cell::RefCell::new(cache), bounds(1), |b| {
                calls.set(calls.get() + 1);
                super::build_win_dots(b)
            })
            .is_some()
        );
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn win_dot_cache_failure_usage_and_metadata_limits_are_exact() {
        let fixed_bytes = std::mem::size_of::<std::cell::RefCell<super::WinDotCache>>();
        let first_bounds = dot_bounds(0.0, 0.0, 3.0, 3.0);
        let second_bounds = dot_bounds(9.0, 0.0, 3.0, 3.0);
        let first_key = super::WinDotKey::from_bounds(first_bounds).unwrap();
        let second_key = super::WinDotKey::from_bounds(second_bounds).unwrap();
        let mut cache = super::WinDotCache::new();

        assert!(matches!(
            cache.admit_and_clone(first_key, None, fixed_bytes, 0, 0),
            super::WinDotAdmission::Retained(None)
        ));
        assert_eq!(cache.retained_usage(), Some((0, fixed_bytes)));
        assert!(matches!(
            cache.admit_and_clone(second_key, None, fixed_bytes - 1, 0, 0),
            super::WinDotAdmission::Bypassed(None)
        ));
        assert!(matches!(cache.get_clone(first_key), Some(None)));
        assert!(cache.get_clone(second_key).is_none());

        let mut path = super::build_win_dots(second_bounds).unwrap();
        path.vertices.reserve_exact(8);
        let capacity = path.vertices.capacity();
        let bytes = fixed_bytes + capacity * std::mem::size_of::<gpui::PathVertex<Pixels>>();
        assert!(matches!(
            cache.admit_and_clone(second_key, Some(path), bytes, capacity, capacity),
            super::WinDotAdmission::Retained(Some(_))
        ));
        assert_eq!(
            cache.retained_usage(),
            Some((capacity, bytes)),
            "failed entries add fixed metadata only and success uses vector capacity"
        );
        assert_eq!(cache.entries.iter().flatten().count(), 2);
    }

    #[test]
    fn win_dot_cache_failure_admission_evicts_success_over_reduced_vertex_budget() {
        let bounds = |x| dot_bounds(x, 0.0, 6.0, 6.0);
        let success_bounds = bounds(0.0);
        let failed_bounds = bounds(9.0);
        let success_key = super::WinDotKey::from_bounds(success_bounds).unwrap();
        let failed_key = super::WinDotKey::from_bounds(failed_bounds).unwrap();
        let path = super::build_win_dots(success_bounds).unwrap();
        let capacity = dot_capacity(&path);
        let fixed_bytes = std::mem::size_of::<std::cell::RefCell<super::WinDotCache>>();
        let mut cache = super::WinDotCache::new();

        assert!(matches!(
            cache.admit_and_clone(
                success_key,
                Some(path),
                WIN_DOT_CACHE_MAX_BYTES,
                capacity,
                capacity
            ),
            super::WinDotAdmission::Retained(Some(_))
        ));
        assert_eq!(
            cache.retained_usage(),
            Some((
                capacity,
                fixed_bytes + capacity * std::mem::size_of::<gpui::PathVertex<Pixels>>()
            ))
        );

        assert!(matches!(
            cache.admit_and_clone(failed_key, None, WIN_DOT_CACHE_MAX_BYTES, 0, 0),
            super::WinDotAdmission::Retained(None)
        ));
        assert!(cache.get_clone(success_key).is_none());
        assert!(matches!(cache.get_clone(failed_key), Some(None)));
        assert_eq!(cache.retained_usage(), Some((0, fixed_bytes)));
        assert_eq!(cache.entries.iter().flatten().count(), 1);
    }

    #[test]
    fn win_dot_cache_rebuilds_real_failure_after_lru_eviction() {
        let cache = std::cell::RefCell::new(super::WinDotCache::new());
        let failed_bounds = dot_bounds(12.5, 7.0, 500.0, 500.0);
        let fixed_bytes = std::mem::size_of::<std::cell::RefCell<super::WinDotCache>>();
        let calls = std::cell::Cell::new(0);
        let build_failure = |bounds| {
            calls.set(calls.get() + 1);
            super::build_win_dots(bounds)
        };

        assert!(super::cached_win_dots_with(&cache, failed_bounds, build_failure).is_none());
        assert!(super::cached_win_dots_with(&cache, failed_bounds, |_| {
            panic!("warm failed entry must not rebuild")
        })
        .is_none());
        assert_eq!(calls.get(), 1);
        assert_eq!(cache.borrow().entries.iter().flatten().count(), 1);
        assert_eq!(cache.borrow().retained_usage(), Some((0, fixed_bytes)));

        for index in 0..WIN_DOT_CACHE_SLOTS {
            let bounds = dot_bounds(2000.0 + index as f32 * 9.0, 0.0, 3.0, 3.0);
            assert!(super::cached_win_dots_with(&cache, bounds, |_| None).is_none());
        }
        assert_eq!(
            cache.borrow().entries.iter().flatten().count(),
            WIN_DOT_CACHE_SLOTS
        );
        assert_eq!(cache.borrow().retained_usage(), Some((0, fixed_bytes)));

        assert!(super::cached_win_dots_with(&cache, failed_bounds, build_failure).is_none());
        assert_eq!(calls.get(), 2);
        assert!(super::cached_win_dots_with(&cache, failed_bounds, |_| {
            panic!("rebuilt failed entry must be warm")
        })
        .is_none());
        assert_eq!(calls.get(), 2);
        assert_eq!(
            cache.borrow().entries.iter().flatten().count(),
            WIN_DOT_CACHE_SLOTS
        );
        assert_eq!(cache.borrow().retained_usage(), Some((0, fixed_bytes)));
    }

    #[test]
    fn win_dot_thread_local_caches_do_not_share_failed_outcomes() {
        let bounds = dot_bounds(12.5, 0.0, 500.0, 500.0);
        let run_thread = || {
            std::thread::spawn(move || {
                let calls = std::cell::Cell::new(0);
                super::WIN_DOT_CACHE.with(|cache| {
                    for _ in 0..2 {
                        assert!(super::cached_win_dots_with(cache, bounds, |_| {
                            calls.set(calls.get() + 1);
                            None
                        })
                        .is_none());
                    }
                });
                calls.get()
            })
        };
        assert_eq!(run_thread().join().unwrap(), 1);
        assert_eq!(run_thread().join().unwrap(), 1);
    }

    #[test]
    fn win_dot_cache_uses_exact_bounds_and_promotes_hits() {
        let a = dot_bounds(0.0, 0.0, 6.0, 6.0);
        let b = dot_bounds(0.25, 0.0, 6.0, 6.0);
        let c = dot_bounds(0.0, 0.0, 6.0, 6.25);
        let a_key = super::WinDotKey::from_bounds(a).unwrap();
        let b_key = super::WinDotKey::from_bounds(b).unwrap();
        let c_key = super::WinDotKey::from_bounds(c).unwrap();
        let a_path = super::build_win_dots(a).unwrap();
        let capacity = dot_capacity(&a_path);
        let mut cache = super::WinDotCache::new();
        let limits = (
            WIN_DOT_CACHE_MAX_BYTES,
            capacity * 2,
            WIN_DOT_CACHE_MAX_ENTRY_VERTICES,
        );

        let first = cache.admit_and_clone(a_key, Some(a_path), limits.0, limits.1, limits.2);
        match first {
            super::WinDotAdmission::Retained(path) => {
                assert_same_win_dot_path(path, original_win_dots(a));
            }
            super::WinDotAdmission::Bypassed(_) => panic!("first path must be retained"),
        }
        assert_same_win_dot_path(cache.get_clone(a_key).flatten(), original_win_dots(a));
        let b_path = super::build_win_dots(b).unwrap();
        match cache.admit_and_clone(b_key, Some(b_path), limits.0, limits.1, limits.2) {
            super::WinDotAdmission::Retained(path) => {
                assert_same_win_dot_path(path, original_win_dots(b));
            }
            super::WinDotAdmission::Bypassed(_) => panic!("second path must be retained"),
        }
        assert_same_win_dot_path(cache.get_clone(b_key).flatten(), original_win_dots(b));
        let mut changed_a = cache
            .get_clone(a_key)
            .expect("first path remains cached")
            .expect("first path is successful");
        changed_a.vertices[1].xy_position = gpui::point(gpui::px(90.), gpui::px(91.));
        changed_a.content_mask.bounds.origin = gpui::point(gpui::px(80.), gpui::px(81.));
        changed_a.color = gpui::rgb(0xabcdef).into();
        assert_same_win_dot_path(cache.get_clone(b_key).flatten(), original_win_dots(b));
        assert_same_win_dot_path(cache.get_clone(a_key).flatten(), original_win_dots(a));

        let c_path = super::build_win_dots(c).unwrap();
        match cache.admit_and_clone(c_key, Some(c_path), limits.0, limits.1, limits.2) {
            super::WinDotAdmission::Retained(path) => {
                assert_same_win_dot_path(path, original_win_dots(c));
            }
            super::WinDotAdmission::Bypassed(_) => panic!("third path must be retained"),
        }
        assert_same_win_dot_path(cache.get_clone(a_key).flatten(), original_win_dots(a));
        assert!(cache.get_clone(b_key).is_none());
        let rebuilt_b = super::build_win_dots(b).unwrap();
        match cache.admit_and_clone(b_key, Some(rebuilt_b), limits.0, limits.1, limits.2) {
            super::WinDotAdmission::Retained(path) => {
                assert_same_win_dot_path(path, original_win_dots(b));
            }
            super::WinDotAdmission::Bypassed(_) => panic!("rebuilt path must be retained"),
        }
        assert_same_win_dot_path(cache.get_clone(b_key).flatten(), original_win_dots(b));
    }

    #[test]
    fn win_dot_cache_keeps_clone_changes_out_of_retained_geometry() {
        let bounds = dot_bounds(2.25, 4.5, 3.0, 3.0);
        let key = super::WinDotKey::from_bounds(bounds).unwrap();
        let mut cache = super::WinDotCache::new();
        let original = super::build_win_dots(bounds).unwrap();
        let original_vertex = point_bits(original.vertices[0].xy_position);
        let retained = match cache.admit_and_clone(
            key,
            Some(original),
            WIN_DOT_CACHE_MAX_BYTES,
            WIN_DOT_CACHE_MAX_VERTICES,
            WIN_DOT_CACHE_MAX_ENTRY_VERTICES,
        ) {
            super::WinDotAdmission::Retained(path) => path.expect("path is successful"),
            super::WinDotAdmission::Bypassed(_) => panic!("small path must be retained"),
        };
        let mut painted = retained;
        painted.vertices[0].xy_position = gpui::point(gpui::px(90.), gpui::px(91.));
        painted.content_mask.bounds.origin = gpui::point(gpui::px(80.), gpui::px(81.));
        painted.color = gpui::rgb(0xabcdef).into();

        let cached = cache
            .get_clone(key)
            .expect("path remains cached")
            .expect("path is successful");
        assert_eq!(point_bits(cached.vertices[0].xy_position), original_vertex);
        assert_eq!(bounds_bits(cached.content_mask.bounds), [0; 4]);
        assert_eq!(cached.color, gpui::Background::default());
    }

    #[test]
    fn cached_win_dots_returns_independent_success_clones_without_rebuilding() {
        let bounds = dot_bounds(2.25, 4.5, 6.0, 6.0);
        let cache = std::cell::RefCell::new(super::WinDotCache::new());
        let calls = std::cell::Cell::new(0);
        let mut first = super::cached_win_dots_with(&cache, bounds, |bounds| {
            calls.set(calls.get() + 1);
            super::build_win_dots(bounds)
        })
        .expect("small geometry builds");
        let expected = original_win_dots(bounds);
        assert_same_win_dot_path(Some(first.clone()), expected.clone());

        first.vertices[0].xy_position = gpui::point(gpui::px(90.), gpui::px(91.));
        first.bounds.origin = gpui::point(gpui::px(80.), gpui::px(81.));
        first.content_mask.bounds.origin = gpui::point(gpui::px(70.), gpui::px(71.));
        first.color = gpui::rgb(0xabcdef).into();

        let mut second = super::cached_win_dots_with(&cache, bounds, |_| {
            panic!("successful hit must not rebuild")
        })
        .expect("cached geometry remains successful");
        assert_same_win_dot_path(Some(second.clone()), expected.clone());
        second.vertices[0].xy_position = gpui::point(gpui::px(50.), gpui::px(51.));
        second.content_mask.bounds.origin = gpui::point(gpui::px(40.), gpui::px(41.));
        second.color = gpui::rgb(0x123456).into();

        let third = super::cached_win_dots_with(&cache, bounds, |_| {
            panic!("each successful hit must clone the retained path")
        })
        .expect("cached geometry remains successful");
        assert_same_win_dot_path(Some(third), expected);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn win_dot_cache_mixed_outcomes_obey_reduced_byte_and_vertex_limits() {
        let a = dot_bounds(0.0, 0.0, 6.0, 6.0);
        let failed = dot_bounds(4.0, 0.0, 6.0, 6.0);
        let b = dot_bounds(9.0, 0.0, 6.0, 6.0);
        let c = dot_bounds(18.0, 0.0, 6.0, 6.0);
        let keys = [a, failed, b, c].map(|bounds| super::WinDotKey::from_bounds(bounds).unwrap());
        let capacity = dot_capacity(&super::build_win_dots(a).unwrap());
        let entry_bytes = capacity * std::mem::size_of::<gpui::PathVertex<Pixels>>();
        let fixed_bytes = std::mem::size_of::<std::cell::RefCell<super::WinDotCache>>();

        for (byte_limit, vertex_limit) in [
            (fixed_bytes + entry_bytes * 2, capacity * 3),
            (fixed_bytes + entry_bytes * 3, capacity * 2),
        ] {
            let mut cache = super::WinDotCache::new();
            for (index, bounds) in [a, failed, b].into_iter().enumerate() {
                let outcome = if index == 1 {
                    None
                } else {
                    Some(super::build_win_dots(bounds).unwrap())
                };
                assert!(matches!(
                    cache.admit_and_clone(keys[index], outcome, byte_limit, vertex_limit, capacity),
                    super::WinDotAdmission::Retained(_)
                ));
            }
            assert_eq!(
                cache.retained_usage().unwrap(),
                (capacity * 2, fixed_bytes + entry_bytes * 2)
            );

            assert!(matches!(
                cache.admit_and_clone(
                    keys[3],
                    Some(super::build_win_dots(c).unwrap()),
                    byte_limit,
                    vertex_limit,
                    capacity
                ),
                super::WinDotAdmission::Retained(Some(_))
            ));
            assert!(
                cache.get_clone(keys[0]).is_none(),
                "least recent success is evicted"
            );
            assert!(matches!(cache.get_clone(keys[1]), Some(None)));
            assert!(cache
                .get_clone(keys[2])
                .is_some_and(|outcome| outcome.is_some()));
            assert!(cache
                .get_clone(keys[3])
                .is_some_and(|outcome| outcome.is_some()));
            assert_eq!(
                cache.retained_usage().unwrap(),
                (capacity * 2, fixed_bytes + entry_bytes * 2)
            );
        }
    }

    #[test]
    fn win_dot_cache_bounds_many_small_resizes_and_reports_retained_capacity() {
        let mut cache = super::WinDotCache::new();
        for step in 0..128 {
            let bounds = dot_bounds(step as f32 * 0.25, 0.0, 3.0 + step as f32 * 0.125, 3.0);
            let key = super::WinDotKey::from_bounds(bounds).unwrap();
            let path = super::build_win_dots(bounds).unwrap();
            let result = cache.admit_and_clone(
                key,
                Some(path),
                WIN_DOT_CACHE_MAX_BYTES,
                WIN_DOT_CACHE_MAX_VERTICES,
                WIN_DOT_CACHE_MAX_ENTRY_VERTICES,
            );
            assert!(matches!(result, super::WinDotAdmission::Retained(_)));
            let (vertices, bytes) = cache.retained_usage().expect("bounded usage");
            assert!(vertices <= WIN_DOT_CACHE_MAX_VERTICES);
            assert!(bytes <= WIN_DOT_CACHE_MAX_BYTES);
            assert!(cache.entries.iter().flatten().count() <= WIN_DOT_CACHE_SLOTS);
        }
        assert_eq!(cache.entries.iter().flatten().count(), WIN_DOT_CACHE_SLOTS);
        let oldest_bounds = dot_bounds(0.0, 0.0, 3.0, 3.0);
        let newest_bounds = dot_bounds(127.0 * 0.25, 0.0, 3.0 + 127.0 * 0.125, 3.0);
        assert!(cache
            .get_clone(super::WinDotKey::from_bounds(oldest_bounds).unwrap())
            .is_none());
        assert!(cache
            .get_clone(super::WinDotKey::from_bounds(newest_bounds).unwrap())
            .is_some_and(|outcome| outcome.is_some()));
    }

    #[test]
    fn win_dot_cache_bypasses_over_budget_paths_without_evicting_entries() {
        let first_bounds = dot_bounds(0.0, 0.0, 6.0, 6.0);
        let next_bounds = dot_bounds(5.0, 0.0, 6.0, 6.0);
        let first_key = super::WinDotKey::from_bounds(first_bounds).unwrap();
        let next_key = super::WinDotKey::from_bounds(next_bounds).unwrap();
        let first_path = super::build_win_dots(first_bounds).unwrap();
        let capacity = dot_capacity(&first_path);
        let mut cache = super::WinDotCache::new();
        assert!(matches!(
            cache.admit_and_clone(
                first_key,
                Some(first_path),
                WIN_DOT_CACHE_MAX_BYTES,
                capacity,
                capacity
            ),
            super::WinDotAdmission::Retained(_)
        ));

        let next_path = super::build_win_dots(next_bounds).unwrap();
        let bypassed = cache.admit_and_clone(
            next_key,
            Some(next_path),
            WIN_DOT_CACHE_MAX_BYTES,
            capacity,
            capacity - 1,
        );
        match bypassed {
            super::WinDotAdmission::Bypassed(path) => {
                assert_same_win_dot_path(path, original_win_dots(next_bounds));
            }
            super::WinDotAdmission::Retained(_) => panic!("over-entry-limit path must bypass"),
        }
        assert!(cache
            .get_clone(first_key)
            .is_some_and(|outcome| outcome.is_some()));
        assert!(cache.get_clone(next_key).is_none());

        let next_path = super::build_win_dots(next_bounds).unwrap();
        let byte_limit = std::mem::size_of::<std::cell::RefCell<super::WinDotCache>>()
            + capacity * std::mem::size_of::<gpui::PathVertex<Pixels>>()
            - 1;
        match cache.admit_and_clone(
            next_key,
            Some(next_path),
            byte_limit,
            capacity * 2,
            capacity,
        ) {
            super::WinDotAdmission::Bypassed(path) => {
                assert_same_win_dot_path(path, original_win_dots(next_bounds));
            }
            super::WinDotAdmission::Retained(_) => panic!("over-byte-limit path must bypass"),
        }
        assert!(cache
            .get_clone(first_key)
            .is_some_and(|outcome| outcome.is_some()));
    }

    #[test]
    fn win_dot_cache_evicts_for_the_reduced_byte_budget() {
        let a = dot_bounds(0.0, 0.0, 6.0, 6.0);
        let b = dot_bounds(9.0, 0.0, 6.0, 6.0);
        let c = dot_bounds(18.0, 0.0, 6.0, 6.0);
        let a_key = super::WinDotKey::from_bounds(a).unwrap();
        let b_key = super::WinDotKey::from_bounds(b).unwrap();
        let c_key = super::WinDotKey::from_bounds(c).unwrap();
        let a_path = super::build_win_dots(a).unwrap();
        let entry_capacity = dot_capacity(&a_path);
        let entry_bytes = entry_capacity * std::mem::size_of::<gpui::PathVertex<Pixels>>();
        let fixed_bytes = std::mem::size_of::<std::cell::RefCell<super::WinDotCache>>();
        let byte_limit = fixed_bytes + entry_bytes * 2;
        let vertex_limit = entry_capacity * 3;
        let mut cache = super::WinDotCache::new();

        for (key, bounds, path) in [
            (a_key, a, a_path),
            (b_key, b, super::build_win_dots(b).unwrap()),
        ] {
            assert_eq!(dot_capacity(&path), entry_capacity);
            match cache.admit_and_clone(key, Some(path), byte_limit, vertex_limit, entry_capacity) {
                super::WinDotAdmission::Retained(path) => {
                    assert_same_win_dot_path(path, original_win_dots(bounds));
                }
                super::WinDotAdmission::Bypassed(_) => panic!("path fits the byte budget"),
            }
        }
        assert!(cache
            .get_clone(a_key)
            .is_some_and(|outcome| outcome.is_some()));
        assert_eq!(
            cache.retained_usage(),
            Some((entry_capacity * 2, byte_limit))
        );
        let c_path = super::build_win_dots(c).unwrap();
        assert_eq!(dot_capacity(&c_path), entry_capacity);
        match cache.admit_and_clone(
            c_key,
            Some(c_path),
            byte_limit,
            vertex_limit,
            entry_capacity,
        ) {
            super::WinDotAdmission::Retained(path) => {
                assert_same_win_dot_path(path, original_win_dots(c));
            }
            super::WinDotAdmission::Bypassed(_) => panic!("third path evicts one old path"),
        }

        assert!(cache.get_clone(b_key).is_none());
        assert_same_win_dot_path(cache.get_clone(a_key).flatten(), original_win_dots(a));
        assert_same_win_dot_path(cache.get_clone(c_key).flatten(), original_win_dots(c));
        assert_eq!(
            cache.retained_usage(),
            Some((entry_capacity * 2, byte_limit))
        );
        assert!(cache.entries.iter().flatten().count() < WIN_DOT_CACHE_SLOTS);
    }

    #[test]
    fn win_dot_cache_production_byte_limit_retains_only_three_max_capacity_paths() {
        let mut cache = super::WinDotCache::new();
        for index in 0..4 {
            let bounds = dot_bounds(index as f32 * 9.0, 0.0, 3.0, 3.0);
            let key = super::WinDotKey::from_bounds(bounds).unwrap();
            let mut path = super::build_win_dots(bounds).unwrap();
            path.vertices
                .reserve_exact(WIN_DOT_CACHE_MAX_ENTRY_VERTICES - path.vertices.len());
            assert_eq!(dot_capacity(&path), WIN_DOT_CACHE_MAX_ENTRY_VERTICES);
            match cache.admit_and_clone(
                key,
                Some(path),
                WIN_DOT_CACHE_MAX_BYTES,
                WIN_DOT_CACHE_MAX_VERTICES,
                WIN_DOT_CACHE_MAX_ENTRY_VERTICES,
            ) {
                super::WinDotAdmission::Retained(path) => {
                    assert_same_win_dot_path(path, original_win_dots(bounds));
                }
                super::WinDotAdmission::Bypassed(_) => panic!("three paths fit the byte limit"),
            }
        }

        let vertex_bytes =
            WIN_DOT_CACHE_MAX_ENTRY_VERTICES * std::mem::size_of::<gpui::PathVertex<Pixels>>();
        let fixed_bytes = std::mem::size_of::<std::cell::RefCell<super::WinDotCache>>();
        assert!(fixed_bytes + vertex_bytes * 3 <= WIN_DOT_CACHE_MAX_BYTES);
        assert!(fixed_bytes + vertex_bytes * 4 > WIN_DOT_CACHE_MAX_BYTES);
        assert_eq!(cache.entries.iter().flatten().count(), 3);
        let (vertices, bytes) = cache.retained_usage().expect("retained usage is bounded");
        assert_eq!(vertices, WIN_DOT_CACHE_MAX_ENTRY_VERTICES * 3);
        assert!(bytes <= WIN_DOT_CACHE_MAX_BYTES);
        assert!(cache
            .get_clone(super::WinDotKey::from_bounds(dot_bounds(0.0, 0.0, 3.0, 3.0)).unwrap())
            .is_none());
        for index in 1..4 {
            let bounds = dot_bounds(index as f32 * 9.0, 0.0, 3.0, 3.0);
            assert_same_win_dot_path(
                cache
                    .get_clone(super::WinDotKey::from_bounds(bounds).unwrap())
                    .flatten(),
                original_win_dots(bounds),
            );
        }
    }

    #[test]
    fn win_dot_cache_distinguishes_signed_zero_and_adjacent_sizes() {
        let positive_zero = super::WinDotKey::from_bounds(dot_bounds(0.0, 0.0, 3.0, 3.0)).unwrap();
        let negative_zero = super::WinDotKey::from_bounds(dot_bounds(-0.0, 0.0, 3.0, 3.0)).unwrap();
        let one = super::WinDotKey::from_bounds(dot_bounds(0.0, 0.0, 3.0, 3.0)).unwrap();
        let adjacent = super::WinDotKey::from_bounds(dot_bounds(
            0.0,
            0.0,
            f32::from_bits(3.0f32.to_bits() + 1),
            3.0,
        ))
        .unwrap();
        assert_ne!(positive_zero, negative_zero);
        assert_eq!(positive_zero, one);
        assert_ne!(one, adjacent);
        assert!(super::WinDotKey::from_bounds(dot_bounds(f32::NAN, 0.0, 3.0, 3.0)).is_none());
    }

    #[gpui::test]
    async fn virtual_history_rows_match_header_columns(cx: &mut TestAppContext) {
        use super::{history_cell, history_row};
        use gpui::{canvas, px, uniform_list, Div};
        use std::{cell::RefCell, rc::Rc};
        type Widths = Rc<RefCell<Vec<(bool, usize, f32)>>>;
        struct HistoryLayoutTest(Widths);
        fn row(header: bool, widths: Widths) -> Div {
            history_row()
                .child(div().w(px(22.)).flex_shrink_0())
                .children(
                    crate::column_widths::Widths::default()
                        .compute(&crate::column_widths::HISTORY, 1000.)
                        .into_iter()
                        .skip(1)
                        .enumerate()
                        .map(|(column, width)| {
                            let widths = widths.clone();
                            history_cell(width, !matches!(column, 0 | 2)).child(
                                canvas(
                                    move |bounds, _, _| {
                                        widths.borrow_mut().push((
                                            header,
                                            column,
                                            bounds.size.width.into(),
                                        ));
                                    },
                                    |_, _, _, _| {},
                                )
                                .w_full()
                                .h(px(10.)),
                            )
                        }),
                )
        }
        impl Render for HistoryLayoutTest {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let widths = self.0.clone();
                div()
                    .w(px(1000.))
                    .flex()
                    .flex_col()
                    .child(row(true, widths.clone()))
                    .child(
                        uniform_list(
                            "history-layout-test",
                            3,
                            cx.processor(move |_, range: std::ops::Range<usize>, _, _| {
                                range.map(|_| row(false, widths.clone())).collect()
                            }),
                        )
                        .h(px(66.)),
                    )
            }
        }
        let widths = Rc::new(RefCell::new(Vec::new()));
        let window = cx.add_window(|_, _| HistoryLayoutTest(widths.clone()));
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        let widths = widths.borrow();
        for column in 0..7 {
            let header = widths
                .iter()
                .find(|(h, c, _)| *h && *c == column)
                .unwrap()
                .2;
            assert!(header > 40.);
            let rows: Vec<_> = widths
                .iter()
                .filter(|(h, c, _)| !h && *c == column)
                .collect();
            assert!(!rows.is_empty());
            for (_, _, width) in rows {
                assert!(
                    (header - width).abs() < 0.5,
                    "column {column}: header {header}, row {width}"
                );
            }
        }
    }

    #[gpui::test]
    async fn priority_cpu_meter_uses_the_requested_height(cx: &mut TestAppContext) {
        let tokens = crate::palette::presets()[0].base.tokens();
        let window = cx.add_window(|_, _| CpuMeterLayoutTestView { tokens });
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();

        let quads = cx
            .update_window(window.into(), |_, window, _| window.painted_quads())
            .unwrap();
        let scale = cx
            .update_window(window.into(), |_, window, _| window.scale_factor())
            .unwrap();
        let priority: Vec<_> = quads
            .iter()
            .filter(|quad| quad.bounds.origin.y.as_f32() / scale < 20.)
            .collect();
        assert_eq!(priority.len(), 18);
        assert!(priority
            .iter()
            .all(|quad| quad.bounds.size.height.as_f32() / scale == 12.));
    }

    #[gpui::test]
    async fn narrow_history_cpu_meter_keeps_main_segments_visible(cx: &mut TestAppContext) {
        let tokens = crate::palette::presets()[0].base.tokens();
        let window = cx.add_window(|_, _| CpuMeterLayoutTestView { tokens });
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();

        let quads = cx
            .update_window(window.into(), |_, window, _| window.painted_quads())
            .unwrap();
        let scale = cx
            .update_window(window.into(), |_, window, _| window.scale_factor())
            .unwrap();
        let narrow_history: Vec<_> = quads
            .iter()
            .filter(|quad| quad.bounds.origin.y.as_f32() / scale >= 20.)
            .collect();
        assert!(narrow_history.len() > 4, "the full bar must remain visible");
        assert!(narrow_history
            .iter()
            .all(|quad| quad.bounds.size.width.as_f32() / scale > 0.));
        assert!(narrow_history
            .iter()
            .all(|quad| quad.bounds.size.height.as_f32() / scale == 8.));
    }

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

    #[gpui::test]
    async fn vfd_unit_glyph_matches_digit_ink_span_at_supported_sizes(cx: &mut TestAppContext) {
        let tokens = crate::palette::presets()[0].base.tokens();
        let window = cx.add_window(|_, _| UnitLayoutTestView { tokens });
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();

        let mut bounds = std::mem::take(&mut *UNIT_GLYPH_TEST_BOUNDS.lock().unwrap());
        assert!(bounds.len() >= 4 && bounds.len().is_multiple_of(4));
        let bounds = bounds.split_off(bounds.len() - 4);
        let mut row_top = 0.;
        for (glyph, size) in bounds.into_iter().zip([9., 15., 22., 42.]) {
            let (digit_ink_top, digit_ink_bottom) = doto_nine_ink_span(size);
            assert!(
                (f32::from(glyph.origin.y) - row_top - digit_ink_top).abs() < 0.5,
                "unit glyph top differs from digit ink at {size}px: got {}, expected {}",
                f32::from(glyph.origin.y) - row_top,
                digit_ink_top
            );
            assert!(
                (f32::from(glyph.size.height) - (digit_ink_bottom - digit_ink_top)).abs() < 0.5,
                "unit glyph height differs from digit ink at {size}px"
            );
            assert!(
                (f32::from(glyph.size.width) - f32::from(glyph.size.height) * 10. / 14.).abs()
                    < 0.5
            );
            row_top += size;
        }
    }

    #[test]
    fn vfd_digit_box_keeps_three_and_four_character_values_on_one_line() {
        assert!((vfd_digit_width(42., 3) - 90.72).abs() < 0.001);
        assert!((vfd_digit_width(22., 4) - 63.36).abs() < 0.001);
    }

    #[test]
    fn self_draw_readout_shows_values_after_the_first_sample() {
        assert_eq!(
            draw_readout_text(DrawMetrics::default()),
            "CPU DRAW LAST N/A MS · N/A FPS / 1S"
        );
        assert_eq!(
            draw_readout_text(DrawMetrics {
                mean_ms: Some(6.64),
                fps: Some(45.3),
            }),
            "CPU DRAW LAST 6.64 MS · 45.3 FPS / 1S"
        );
    }
}
