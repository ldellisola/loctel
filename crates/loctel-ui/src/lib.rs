//! The loctel GPUI layer. It is thin: views that draw what the engine
//! reports, and the text the menu bar shares with them.
//!
//! - [`StatusView`] is Milestone 1's only view: whether loctel is receiving,
//!   the rates and counts, the discard rules line (Milestone 2) and every
//!   listener with its problem if it has one.
//!   [`open_status_window`] shows it in a window that refreshes every second
//!   from [`loctel_engine::EngineHandle::status`] (the Linux development
//!   window; on macOS the menu bar item comes first).
//! - [`menu_text`] holds the exact strings for the status line, the discard
//!   rules line and the rates, so the macOS menu and the view cannot drift apart.
//! - [`Theme`] holds the colours; [`register_fonts`] the bundled typeface.
//! - `testing` (behind the `screenshot` feature) renders any view headlessly
//!   to a PNG and compares it pixel for pixel with a reference.
//!
//! See `docs/areas/ui.md` for the theme tokens and the screenshot workflow.

pub mod menu_text;
mod status_view;
mod theme;

#[cfg(feature = "screenshot")]
pub mod testing;

pub use status_view::{ListenerLine, StatusModel, StatusView, open_status_window};
pub use theme::{FONT_FAMILY, Theme, register_fonts};
