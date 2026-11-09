//! View controls independent of scanning and GPUI.
use crate::{tree::Metric, treemap::Rect};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Escape {
    Filter,
    Viewport,
    Selection,
    Close,
}
pub fn escape(filter: bool, viewport: Viewport, selected: bool) -> Escape {
    if filter {
        Escape::Filter
    } else if viewport != Viewport::default() {
        Escape::Viewport
    } else if selected {
        Escape::Selection
    } else {
        Escape::Close
    }
}
pub fn draw_depth(depth: u32, deeper: bool) -> u32 {
    if deeper {
        depth.saturating_add(1).min(16)
    } else {
        depth.saturating_sub(1).max(1)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Size,
    Files,
    Age,
}
impl Mode {
    pub const fn metric(self) -> Metric {
        match self {
            Self::Files => Metric::Files,
            Self::Size | Self::Age => Metric::Bytes,
        }
    }
    pub const fn next(self) -> Self {
        match self {
            Self::Size => Self::Files,
            Self::Files => Self::Age,
            Self::Age => Self::Size,
        }
    }
}

pub fn age_label(modified: i64, now: i64) -> &'static str {
    if modified <= 0 {
        return "Unknown age";
    }
    match now.saturating_sub(modified) {
        ..=86400 => "Today",
        86401..=604800 => "This week",
        604801..=2592000 => "This month",
        2592001..=31536000 => "This year",
        _ => "Over a year",
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub scale: f32,
    pub x: f32,
    pub y: f32,
}
impl Default for Viewport {
    fn default() -> Self {
        Self {
            scale: 1.,
            x: 0.,
            y: 0.,
        }
    }
}
impl Viewport {
    pub fn inverse(self, x: f32, y: f32) -> (f32, f32) {
        ((x - self.x) / self.scale, (y - self.y) / self.scale)
    }
    pub fn rect(self, rect: Rect) -> Rect {
        rect.scaled(self.scale).translated(self.x, self.y)
    }
    pub fn zoom(&mut self, factor: f32, x: f32, y: f32) {
        if !factor.is_finite() || !x.is_finite() || !y.is_finite() {
            return;
        }
        let anchor = self.inverse(x, y);
        self.scale = (self.scale * factor).clamp(1., 32.);
        self.x = x - anchor.0 * self.scale;
        self.y = y - anchor.1 * self.scale;
        if self.scale == 1. {
            *self = Self::default();
        }
    }
    pub fn pan(&mut self, x: f32, y: f32) {
        if x.is_finite() && y.is_finite() {
            self.x += x;
            self.y += y;
        }
    }
}

#[derive(Default)]
pub struct History {
    paths: Vec<PathBuf>,
    cursor: usize,
}
impl History {
    pub fn visit(&mut self, path: PathBuf) {
        if self.paths.get(self.cursor) == Some(&path) {
            return;
        }
        self.paths.truncate(if self.paths.is_empty() {
            0
        } else {
            self.cursor + 1
        });
        self.paths.push(path);
        self.cursor = self.paths.len() - 1;
    }
    pub fn travel(&mut self, forward: bool) -> Option<PathBuf> {
        let next = if forward {
            self.cursor.checked_add(1)?
        } else {
            self.cursor.checked_sub(1)?
        };
        let path = self.paths.get(next)?.clone();
        self.cursor = next;
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mode_weights_and_unknown_age_are_explicit() {
        assert_eq!(Mode::Age.metric(), Metric::Bytes);
        assert_eq!(Mode::Files.metric(), Metric::Files);
        assert_eq!(age_label(0, 100000), "Unknown age");
        assert_eq!(age_label(1, 100000), "This week");
    }
    #[test]
    fn escape_clears_filter_before_viewport_before_selection() {
        let zoom = Viewport {
            scale: 2.,
            ..Default::default()
        };
        assert_eq!(escape(true, zoom, true), Escape::Filter);
        assert_eq!(escape(false, zoom, true), Escape::Viewport);
        assert_eq!(escape(false, Viewport::default(), true), Escape::Selection);
        assert_eq!(escape(false, Viewport::default(), false), Escape::Close);
        assert_eq!(draw_depth(1, false), 1);
        assert_eq!(draw_depth(16, true), 16);
    }
    #[test]
    fn zoom_keeps_cursor_anchor_and_hit_transform() {
        let mut view = Viewport::default();
        let rect = Rect::new(40., 20., 80., 60.);
        view.zoom(2., 60., 35.);
        assert_eq!(view.inverse(60., 35.), (60., 35.));
        view.pan(13., -7.);
        let drawn = view.rect(rect);
        let (x, y) = view.inverse(drawn.x + 20., drawn.y + 20.);
        assert!(rect.contains(x, y));
        view.zoom(0.01, 0., 0.);
        assert_eq!(view, Viewport::default());
    }
    #[test]
    fn history_discards_forward_branch_after_a_new_visit() {
        let mut history = History::default();
        for path in ["/a", "/b", "/c"] {
            history.visit(path.into());
        }
        assert_eq!(history.travel(false), Some("/b".into()));
        assert_eq!(history.travel(true), Some("/c".into()));
        history.travel(false);
        history.visit("/d".into());
        assert_eq!(history.travel(true), None);
        assert_eq!(history.travel(false), Some("/b".into()));
    }
}
