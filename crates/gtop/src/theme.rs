use gpui::{px, rgb, App};
use gpui_component::{Theme, ThemeMode};

pub const BASE: u32 = 0x1e1e2e;
pub const MANTLE: u32 = 0x181825;
pub const SURFACE: u32 = 0x313244;
pub const TEXT: u32 = 0xcdd6f4;
pub const SUBTEXT: u32 = 0xa6adc8;
pub const BLUE: u32 = 0x89b4fa;
pub const RED: u32 = 0xf38ba8;
pub const PEACH: u32 = 0xfab387;
pub const GREEN: u32 = 0xa6e3a1;
pub const TEAL: u32 = 0x94e2d5;

pub fn init(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |theme| {
        theme.font_size = px(13.);
        theme.background = rgb(BASE).into();
        theme.foreground = rgb(TEXT).into();
        theme.muted_foreground = rgb(SUBTEXT).into();
        theme.border = rgb(SURFACE).into();
        theme.input = rgb(SURFACE).into();
        theme.primary = rgb(BLUE).into();
        theme.primary_foreground = rgb(MANTLE).into();
        theme.button_primary = rgb(BLUE).into();
        theme.button_primary_foreground = rgb(MANTLE).into();
        theme.button_primary_hover = rgb(0xb4befe).into();
        theme.button_primary_active = rgb(0x74c7ec).into();
        theme.ring = rgb(BLUE).into();
        theme.accent = rgb(SURFACE).into();
        theme.accent_foreground = rgb(TEXT).into();
        theme.secondary = rgb(SURFACE).into();
        theme.secondary_foreground = rgb(TEXT).into();
        theme.button = rgb(SURFACE).into();
        theme.button_hover = rgb(0x45475a).into();
        theme.button_active = rgb(0x585b70).into();
        theme.button_foreground = rgb(TEXT).into();
        theme.danger = rgb(RED).into();
        theme.danger_foreground = rgb(MANTLE).into();
        theme.table = rgb(BASE).into();
        theme.table_head = rgb(MANTLE).into();
        theme.table_head_foreground = rgb(SUBTEXT).into();
        theme.table_even = rgb(0x222233).into();
        theme.table_hover = rgb(SURFACE).into();
        theme.table_active = rgb(0x363951).into();
        theme.table_active_border = rgb(BLUE).into();
        theme.table_row_border = rgb(SURFACE).into();
        theme.scrollbar = rgb(0x585b70).into();
    });
}
