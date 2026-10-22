//! Colours and the font. The dark tokens come from the accepted mockups
//! (design section 9.3); the light ones are in the same spirit, with the same
//! hues darkened so text and accents keep their contrast on a pale ground.

use std::borrow::Cow;

use gpui::{App, Global, Rgba, WindowAppearance, rgb};

/// The family every view asks for. It is the bundled Inter (see
/// `assets/fonts/LICENSE.txt`), registered by [`register_fonts`].
pub const FONT_FAMILY: &str = "Inter";

/// Regular (400) and semibold (600).
const FONT_FILES: [&[u8]; 2] = [
    include_bytes!("../assets/fonts/Inter-Regular.otf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.otf"),
];

/// Makes [`FONT_FAMILY`] available to `cx`. Call it once, before the first
/// frame: the window app does it at launch and the screenshot harness for
/// every render. Bundling the font means a screenshot looks the same on every
/// machine, whatever fonts it has installed.
pub fn register_fonts(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> =
        FONT_FILES.iter().map(|&font| Cow::Borrowed(font)).collect();
    cx.text_system()
        .add_fonts(fonts)
        .expect("the bundled fonts are valid");
}

/// Every colour a view uses. Views read these and never write a hex literal.
///
/// The main window keeps the current one as a GPUI global (set by
/// [`kit_theme::apply`](crate::app::kit_theme::apply), which also maps it onto
/// GPUI Kit's theme): a view reads it with `cx.global::<Theme>()` while it
/// renders, so it follows the system appearance without being told.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// Whether this is the dark look. GPUI Kit's theme has a mode of its own
    /// (its defaults, scrollbars and shadows follow it), and this says which.
    pub dark: bool,
    /// The window background.
    pub bg: Rgba,
    /// Raised surfaces: cards and lists.
    pub panel: Rgba,
    /// Hairlines around panels.
    pub border: Rgba,
    pub text: Rgba,
    pub text_secondary: Rgba,
    pub text_tertiary: Rgba,
    /// Links, focus, the "live" colour.
    pub accent: Rgba,
    /// Error fills and dots.
    pub error: Rgba,
    /// Error text, lighter than `error` so it reads on a dark ground.
    pub error_text: Rgba,
    pub warn: Rgba,
    pub ok: Rgba,
    /// One colour per service, assigned in order and wrapped. The same
    /// service keeps its colour on every screen.
    pub service_palette: [Rgba; 5],
}

impl Theme {
    /// The mocked-up look.
    pub fn dark() -> Theme {
        Theme {
            dark: true,
            bg: rgb(0x15161a),
            panel: rgb(0x1a1b20),
            border: rgb(0x2a2b32),
            text: rgb(0xececf1),
            text_secondary: rgb(0xa3a3ad),
            text_tertiary: rgb(0x8b8b95),
            accent: rgb(0x4c9dff),
            error: rgb(0xff6b63),
            error_text: rgb(0xff8a84),
            warn: rgb(0xf5c26b),
            ok: rgb(0x34c759),
            service_palette: [
                rgb(0x4c9dff),
                rgb(0x3cc8b4),
                rgb(0xf0a04b),
                rgb(0xb18cff),
                rgb(0xf07fb0),
            ],
        }
    }

    /// The light look: the dark one's hues, darkened to hold their contrast
    /// on a near-white ground. Only the dark look is mocked, so this is the
    /// builder's reading of it.
    pub fn light() -> Theme {
        Theme {
            dark: false,
            bg: rgb(0xf4f4f6),
            panel: rgb(0xffffff),
            border: rgb(0xdcdce2),
            text: rgb(0x1b1c20),
            text_secondary: rgb(0x4d4d57),
            text_tertiary: rgb(0x62626c),
            accent: rgb(0x0a62d6),
            error: rgb(0xd92d20),
            error_text: rgb(0xb42318),
            warn: rgb(0x9a5b00),
            ok: rgb(0x1a7f37),
            service_palette: [
                rgb(0x0a62d6),
                rgb(0x0f7f70),
                rgb(0xb45f06),
                rgb(0x6f42c1),
                rgb(0xc2306f),
            ],
        }
    }
}

impl Theme {
    /// The hairline round a glass part of the window (the sidebar and the
    /// toolbar, when a platform backdrop sits behind them). It is translucent,
    /// so the glass shows through it: the text colour at a low alpha, a light
    /// line on the dark look and a dark one on the light look.
    pub fn glass_hairline(&self) -> Rgba {
        Rgba {
            a: if self.dark { 0.16 } else { 0.14 },
            ..self.text
        }
    }

    /// The highlight just inside a glass part's hairline: the soft light edge
    /// of a lit pane. Always a light line, so the panel colour at a low alpha
    /// on the dark look (where the text colour is the light one) and
    /// the near-white panel fill on the light look.
    pub fn glass_highlight(&self) -> Rgba {
        if self.dark {
            Rgba {
                a: 0.08,
                ..self.text
            }
        } else {
            Rgba {
                a: 0.60,
                ..self.panel
            }
        }
    }

    /// The look that goes with a window's appearance: the dark theme for
    /// `Dark` and `VibrantDark`, the light one for the other two.
    pub fn for_appearance(appearance: WindowAppearance) -> Theme {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Theme::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Theme::light(),
        }
    }
}

impl Global for Theme {}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(color: Rgba) -> u32 {
        let byte = |channel: f32| (channel * 255.0).round() as u32;
        (byte(color.r) << 16) | (byte(color.g) << 8) | byte(color.b)
    }

    /// WCAG relative luminance.
    fn luminance(color: Rgba) -> f32 {
        let linear = |channel: f32| {
            if channel <= 0.03928 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    }

    fn contrast(a: Rgba, b: Rgba) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn dark_tokens_are_the_mockup_values() {
        let theme = Theme::dark();
        assert_eq!(hex(theme.bg), 0x15161a);
        assert_eq!(hex(theme.panel), 0x1a1b20);
        assert_eq!(hex(theme.text), 0xececf1);
        assert_eq!(hex(theme.text_secondary), 0xa3a3ad);
        assert_eq!(hex(theme.text_tertiary), 0x8b8b95);
        assert_eq!(hex(theme.accent), 0x4c9dff);
        assert_eq!(hex(theme.error), 0xff6b63);
        assert_eq!(hex(theme.error_text), 0xff8a84);
        assert_eq!(hex(theme.warn), 0xf5c26b);
        assert_eq!(hex(theme.ok), 0x34c759);
        assert_eq!(
            theme.service_palette.map(hex),
            [0x4c9dff, 0x3cc8b4, 0xf0a04b, 0xb18cff, 0xf07fb0]
        );
    }

    #[test]
    fn both_themes_keep_text_readable() {
        for theme in [Theme::dark(), Theme::light()] {
            for text in [theme.text, theme.text_secondary, theme.text_tertiary] {
                for ground in [theme.bg, theme.panel] {
                    assert!(
                        contrast(text, ground) >= 4.5,
                        "{text:?} on {ground:?} is {:.2}:1",
                        contrast(text, ground)
                    );
                }
            }
            for text in [theme.error_text, theme.ok, theme.accent] {
                assert!(contrast(text, theme.panel) >= 3.0, "{text:?} on the panel");
            }
        }
    }

    #[test]
    fn glass_lines_are_translucent_and_come_from_the_tokens() {
        for theme in [Theme::dark(), Theme::light()] {
            for line in [theme.glass_hairline(), theme.glass_highlight()] {
                assert!(line.a > 0.0 && line.a < 1.0, "{line:?} must let glass show");
            }
            assert_ne!(theme.glass_hairline(), theme.glass_highlight());
            // The hairline is the text colour: light on dark, dark on light.
            assert_eq!(
                (theme.glass_hairline().r, theme.glass_hairline().g),
                (theme.text.r, theme.text.g)
            );
        }
        // The highlight is a light line on both looks: the (light) text
        // colour on the dark one and the white panel on the light one.
        assert_eq!(Theme::dark().glass_highlight().b, Theme::dark().text.b);
        assert_eq!(Theme::light().glass_highlight().b, Theme::light().panel.b);
        assert!(luminance(Theme::light().glass_highlight()) > 0.9);
    }

    #[test]
    fn light_is_not_the_dark_theme() {
        assert_ne!(Theme::light(), Theme::dark());
        assert!(luminance(Theme::light().bg) > luminance(Theme::dark().bg));
    }
}
