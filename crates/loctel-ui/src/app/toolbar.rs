//! The toolbar over the content: a title for the screen, and the controls
//! that belong to it.

use gpui::{FontWeight, IntoElement, ParentElement, Styled, div, px};

use super::shell::Screen;
use crate::Theme;
use crate::model::format;

/// How many characters of a trace ID the toolbar shows (the middle is cut).
const TRACE_ID_CHARS: usize = 17;

/// The title of the toolbar on `screen`: `Map & traces` on the home screen and
/// `Map & traces › 4bf92f35…0e0e4736` on a trace (the ID cut in the middle:
/// both ends of it identify it).
pub fn toolbar_title(screen: &Screen) -> String {
    match screen {
        Screen::Home => "Map & traces".to_owned(),
        Screen::Trace(id) => format!(
            "Map & traces \u{203a} {}",
            format::truncate_middle(&id.to_string(), TRACE_ID_CHARS)
        ),
    }
}

/// The toolbar's contents on `screen`, inside the glass frame the shell draws
/// around it.
pub(super) fn toolbar_contents(theme: &Theme, screen: &Screen) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .gap_3()
        .px(px(18.0))
        .child(
            div()
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(toolbar_title(screen)),
        )
}
