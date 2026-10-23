//! The contrast of two colours, by the WCAG 2 formula: the ratio of their
//! relative luminances, from 1 (the same colour) to 21 (black on white).
//!
//! It is what `tests/contrast.rs` holds the [`Theme`](crate::Theme) to in both
//! looks (text at least 4.5 on the grounds it is read on, the hairline at least
//! 1.3 against the panel), so a token that is too faint fails a test and names
//! itself. Nothing in the app calls it while running.

use gpui::Rgba;

/// The WCAG relative luminance of `color`, 0 for black to 1 for white. The
/// colour is taken as opaque sRGB: its alpha is not part of it.
pub fn relative_luminance(color: Rgba) -> f64 {
    // sRGB to linear light. WCAG 2 prints the knee as 0.03928, which is a
    // rounding of the standard's 0.04045; no 8-bit channel falls between them.
    let linear = |channel: f32| {
        let channel = f64::from(channel).clamp(0.0, 1.0);
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

/// The WCAG contrast ratio of `a` and `b`: `(lighter + 0.05) / (darker + 0.05)`
/// over their relative luminances. It does not matter which is the text and
/// which the ground.
pub fn ratio(a: Rgba, b: Rgba) -> f64 {
    let (a, b) = (relative_luminance(a), relative_luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
