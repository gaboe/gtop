use crate::palette::{presets, Base};
use gpui::{px, rgb, App};
use gpui_component::{Theme, ThemeMode};
pub const MANTLE: u32 = 0x141618;
pub const RED: u32 = 0xff3b1f;
pub fn init(cx: &mut App) {
    apply(presets()[0].base, cx);
}
pub fn apply(base: Base, cx: &mut App) {
    let t = base.tokens();
    Theme::change(
        if t.light {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        },
        None,
        cx,
    );
    Theme::update(cx, |theme| {
        theme.font_size = px(12.);
        theme.radius = px(0.);
        theme.background = rgb(base.ground).into();
        theme.foreground = rgb(base.silk).into();
        theme.muted_foreground = rgb(t.silk_dim).into();
        theme.border = rgb(t.rule).into();
        theme.input = rgb(t.well).into();
        theme.primary = rgb(base.led).into();
        theme.primary_foreground = rgb(t.on_led).into();
        theme.button_primary = rgb(base.led).into();
        theme.button_primary_foreground = rgb(t.on_led).into();
        theme.button_primary_hover = rgb(base.led).into();
        theme.button_primary_active = rgb(base.led).into();
        theme.ring = rgb(base.led).into();
        theme.accent = rgb(t.plate2).into();
        theme.accent_foreground = rgb(base.silk).into();
        theme.secondary = rgb(base.plate).into();
        theme.secondary_foreground = rgb(base.silk).into();
        theme.button = rgb(base.plate).into();
        theme.button_hover = rgb(t.plate2).into();
        theme.button_active = rgb(t.plate2).into();
        theme.button_foreground = rgb(t.silk_dim).into();
        theme.danger = rgb(base.hot).into();
        theme.danger_foreground = rgb(t.on_led).into();
        theme.table = rgb(t.table).into();
        theme.table_head = rgb(t.table_head).into();
        theme.table_head_foreground = rgb(t.silk_dim).into();
        theme.table_even = theme.table;
        theme.table_hover = rgb(t.plate2).into();
        theme.table_active = rgb(t.plate2).into();
        theme.table_active_border = rgb(base.silk).into();
        theme.table_row_border = rgb(t.row_rule).into();
        theme.scrollbar = rgb(t.rule_hi).into();
    });
}
