//! The bridge from loctel's [`Theme`] to GPUI Kit's. Kit components (the
//! table, the input, the buttons, the toggles) paint themselves from Kit's
//! `Theme` global, so the main window maps our tokens onto it: one set of
//! colours, in both appearances, and the same bundled font.

use gpui::{App, Hsla, Rgba, black, px};
use gpui_kit::component::{Theme as KitTheme, ThemeColor, ThemeMode};

use crate::theme::{FONT_FAMILY, Theme};

/// The base text size of Kit's components, in pixels. Our own views use the
/// same size.
const FONT_SIZE: f32 = 13.0;

/// Makes `theme` the look of the app: stores it as the [`Theme`] global (what
/// our views read), switches Kit to the matching mode, lays our colours over
/// Kit's, and sets the font family to [`FONT_FAMILY`]. Every window redraws.
///
/// Kit has to be initialised ([`init`](super::init)).
pub fn apply(theme: &Theme, cx: &mut App) {
    cx.set_global(*theme);
    let mode = if theme.dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    // Loads Kit's registered theme for the mode. It replaces the colours, so
    // ours go on in a second step.
    KitTheme::change(mode, None, cx);
    KitTheme::update(cx, |kit| {
        kit.colors = kit_colors(theme, kit.colors);
        kit.font_family = FONT_FAMILY.into();
        kit.font_size = px(FONT_SIZE);
        kit.radius = px(8.0);
        kit.radius_lg = px(16.0);
    });
}

fn hsla(color: Rgba) -> Hsla {
    color.into()
}

/// `color` at `alpha`: the tints of the accent and the text that mark a hover
/// or a selection.
fn tint(color: Rgba, alpha: f32) -> Hsla {
    hsla(Rgba { a: alpha, ..color })
}

/// Kit's colours with ours laid over them: `base` is Kit's own set for the
/// mode, so every token we do not name keeps a sensible value. The names are
/// Kit's: `background` is the window, `primary` the accent (a primary button,
/// a switched-on toggle), `accent` Kit's hover tint, `table`, `list`, `input`
/// and `button` the surfaces of those components.
pub fn kit_colors(theme: &Theme, base: ThemeColor) -> ThemeColor {
    // Text on an accent or error fill: the window colour reads on both, in
    // both looks.
    let on_fill = hsla(theme.bg);
    let hover = tint(theme.text, 0.06);
    let pressed = tint(theme.text, 0.10);
    let selected = tint(theme.accent, 0.18);
    let transparent = tint(theme.bg, 0.0);

    ThemeColor {
        background: hsla(theme.bg),
        foreground: hsla(theme.text),
        border: hsla(theme.border),
        input: hsla(theme.border),
        caret: hsla(theme.text),
        ring: hsla(theme.accent),
        selection: tint(theme.accent, 0.30),
        link: hsla(theme.accent),
        link_hover: hsla(theme.accent),
        link_active: hsla(theme.accent),
        muted: hsla(theme.border),
        muted_foreground: hsla(theme.text_tertiary),
        overlay: black().opacity(0.45),
        window_border: hsla(theme.border),

        // Kit's `accent` is the tint under a hovered menu item or list row.
        accent: hover,
        accent_foreground: hsla(theme.text),

        primary: hsla(theme.accent),
        primary_hover: hsla(theme.accent),
        primary_active: hsla(theme.accent),
        primary_foreground: on_fill,
        secondary: hsla(theme.panel),
        secondary_hover: hover,
        secondary_active: pressed,
        secondary_foreground: hsla(theme.text),
        danger: hsla(theme.error),
        danger_hover: hsla(theme.error),
        danger_active: hsla(theme.error),
        danger_foreground: on_fill,
        warning: hsla(theme.warn),
        warning_hover: hsla(theme.warn),
        warning_active: hsla(theme.warn),
        warning_foreground: on_fill,
        success: hsla(theme.ok),
        success_hover: hsla(theme.ok),
        success_active: hsla(theme.ok),
        success_foreground: on_fill,
        info: hsla(theme.accent),
        info_hover: hsla(theme.accent),
        info_active: hsla(theme.accent),
        info_foreground: on_fill,

        button: hsla(theme.panel),
        button_hover: hover,
        button_active: pressed,
        button_foreground: hsla(theme.text),
        button_primary: hsla(theme.accent),
        button_primary_hover: hsla(theme.accent),
        button_primary_active: hsla(theme.accent),
        button_primary_foreground: on_fill,
        button_danger: hsla(theme.error),
        button_danger_hover: hsla(theme.error),
        button_danger_active: hsla(theme.error),
        button_danger_foreground: on_fill,

        popover: hsla(theme.panel),
        popover_foreground: hsla(theme.text),

        list: hsla(theme.panel),
        list_hover: hover,
        list_active: selected,
        list_active_border: hsla(theme.accent),
        list_even: hsla(theme.panel),
        list_head: hsla(theme.panel),

        table: hsla(theme.panel),
        table_head: hsla(theme.panel),
        table_head_foreground: hsla(theme.text_secondary),
        table_foot: hsla(theme.panel),
        table_foot_foreground: hsla(theme.text_secondary),
        table_even: hsla(theme.panel),
        table_hover: hover,
        table_active: selected,
        table_active_border: hsla(theme.accent),
        table_row_border: hsla(theme.border),

        sidebar: hsla(theme.panel),
        sidebar_foreground: hsla(theme.text),
        sidebar_border: hsla(theme.border),
        sidebar_accent: selected,
        sidebar_accent_foreground: hsla(theme.text),
        sidebar_primary: hsla(theme.accent),
        sidebar_primary_foreground: on_fill,

        title_bar: hsla(theme.panel),
        title_bar_border: hsla(theme.border),

        scrollbar: transparent,
        scrollbar_thumb: tint(theme.text_tertiary, 0.55),
        scrollbar_thumb_hover: tint(theme.text_tertiary, 0.85),

        switch: hsla(theme.border),
        progress_bar: hsla(theme.accent),
        slider_bar: hsla(theme.accent),
        slider_thumb: hsla(theme.accent),

        ..base
    }
}
