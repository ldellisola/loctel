//! The contrast of the theme (M5 plan, Task 8), by the WCAG relative-luminance
//! formula, in both looks. Text has to read on the two grounds it is drawn on,
//! the coloured texts on the panel, and the hairline has to stay visible round it.
//!
//! The thresholds are the plan's Global Constraints:
//!
//! | what | against | at least |
//! |---|---|---|
//! | `text`, `text_secondary`, `text_tertiary` | `bg` and `panel` | 4.5 |
//! | `error_text`, `warn`, `accent` (as text) | `panel` | 4.5 |
//! | `border` | `panel` | 1.3 |
//!
//! A failing pair is named with its ratio, so the token to move is in the
//! message. Moving one keeps its hue family (`docs/areas/ui.md`, "Contrast").

use gpui::{Rgba, rgb};
use loctel_ui::Theme;
use loctel_ui::contrast::ratio;

/// The least ratio of a text on a ground it is read on.
const TEXT: f64 = 4.5;
/// The least ratio of the hairline against the panel it surrounds.
const HAIRLINE: f64 = 1.3;

/// Every pair the plan names for `theme`: `(what, ratio, least)`.
fn pairs(theme: &Theme) -> Vec<(String, f64, f64)> {
    let mut pairs = Vec::new();
    let texts = [
        ("text", theme.text),
        ("text_secondary", theme.text_secondary),
        ("text_tertiary", theme.text_tertiary),
    ];
    let grounds = [("bg", theme.bg), ("panel", theme.panel)];
    for (text, color) in texts {
        for (ground, background) in grounds {
            pairs.push((
                format!("{text} on {ground}"),
                ratio(color, background),
                TEXT,
            ));
        }
    }
    for (text, color) in [
        ("error_text", theme.error_text),
        ("warn", theme.warn),
        ("accent", theme.accent),
    ] {
        pairs.push((format!("{text} on panel"), ratio(color, theme.panel), TEXT));
    }
    pairs.push((
        "border against panel".to_owned(),
        ratio(theme.border, theme.panel),
        HAIRLINE,
    ));
    pairs
}

/// Asserts every pair of `theme`, naming all the ones that fail.
fn assert_contrast(look: &str, theme: &Theme) {
    let pairs = pairs(theme);
    assert_eq!(pairs.len(), 6 + 3 + 1, "the pairs of the plan");
    let failing: Vec<String> = pairs
        .iter()
        .filter(|(_, found, least)| found < least)
        .map(|(what, found, least)| format!("{what}: {found:.2}:1, needs {least}:1"))
        .collect();
    assert!(
        failing.is_empty(),
        "the {look} theme has pairs that are too faint:\n  {}",
        failing.join("\n  ")
    );
}

#[test]
fn theme_contrast_dark() {
    assert_contrast("dark", &Theme::dark());
}

#[test]
fn theme_contrast_light() {
    assert_contrast("light", &Theme::light());
}

#[test]
fn ratio_is_the_wcag_formula() {
    let white = rgb(0xffffff);
    let black = rgb(0x000000);
    // The extremes of the scale, and a colour against itself.
    assert!(
        (ratio(white, black) - 21.0).abs() < 1e-9,
        "{}",
        ratio(white, black)
    );
    assert!((ratio(white, white) - 1.0).abs() < 1e-12);
    assert!((ratio(black, black) - 1.0).abs() < 1e-12);

    // The order of the two does not matter.
    let grey = rgb(0x767676);
    assert_eq!(ratio(grey, white), ratio(white, grey));

    // The well-known `#767676` on white, the lightest grey that passes 4.5:1,
    // and the next one up that does not.
    assert!(
        (ratio(grey, white) - 4.54).abs() < 0.005,
        "{}",
        ratio(grey, white)
    );
    assert!(ratio(rgb(0x777777), white) < 4.5);

    // A mid colour from a computation by hand: the relative luminance of
    // `#2a2b32` is about 0.0246 and of `#1a1b20` about 0.0110.
    let ratio_of_hairline = ratio(rgb(0x2a2b32), rgb(0x1a1b20));
    assert!(
        (ratio_of_hairline - 1.22).abs() < 0.01,
        "{ratio_of_hairline}"
    );

    // Alpha is not part of it: the tokens are opaque, and a translucent colour
    // is judged by its channels.
    let clear = Rgba { a: 0.0, ..grey };
    assert_eq!(ratio(clear, white), ratio(grey, white));
}

#[test]
fn a_pair_that_fails_is_reported_by_name() {
    // The check says which token to move: a theme with a faint tertiary text
    // fails with exactly that token's pairs, on the two grounds, and no others.
    let faint = Theme {
        text_tertiary: rgb(0xc8c8d0),
        ..Theme::light()
    };
    let found = pairs(&faint);
    let failing: Vec<&str> = found
        .iter()
        .filter(|(_, found, least)| found < least)
        .map(|(what, _, _)| what.as_str())
        .collect();
    assert_eq!(
        failing,
        ["text_tertiary on bg", "text_tertiary on panel"],
        "{found:?}"
    );
}
