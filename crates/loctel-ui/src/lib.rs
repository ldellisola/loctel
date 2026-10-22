//! The loctel GPUI layer. It is thin: views that draw what the engine
//! reports, and the text the menu bar shares with them.
//!
//! - [`StatusView`] is Milestone 1's only view: whether loctel is receiving,
//!   the rates and counts, the discard rules line (Milestone 2) and every
//!   listener with its problem if it has one.
//!   [`open_status_window`] shows it in a window that refreshes every second
//!   from [`loctel_engine::EngineHandle::status`] (the Linux development
//!   window; on macOS the menu bar item comes first).
//! - [`model`] holds the pure models of the main window (Milestone 3): the
//!   formats of durations, clock times, rates and error shares, the colour a
//!   service keeps, the trace table's rows, the query bar's state, the
//!   place of every node of the service map, the rows of a trace's waterfall
//!   and the text of the span inspector. No GPUI in them, so plain tests pin
//!   every string and every place.
//! - [`app`] is the main window's shell (Milestone 3): the floating sidebar, the
//!   toolbar and the content area, the [`UiBackend`](app::UiBackend) that gives
//!   views the store and the engine's status, the bridge from [`Theme`] to GPUI
//!   Kit's theme, [`open_main_window`](app::open_main_window), and
//!   [`HomeModel`](app::HomeModel), the home screen's live data (the map and
//!   the table's rows, read every 500 ms behind a time range, two toggles and
//!   the query bar).
//! - [`views`] holds the views of the main window's screens: the live
//!   [`MapView`](views::MapView) and the [`TraceTableView`](views::TraceTableView)
//!   (the TraceQL bar, the two toggles and the table) of the home screen so far
//!   (the trace view follows).
//! - [`menu_text`] holds the exact strings for the status line, the discard
//!   rules line and the rates, so the macOS menu and the view cannot drift apart.
//! - [`Theme`] holds the colours; [`register_fonts`] the bundled typeface.
//! - `testing` (behind the `screenshot` feature) renders any view headlessly
//!   to a PNG and compares it pixel for pixel with a reference.
//!
//! See `docs/areas/ui.md` for the theme tokens and the screenshot workflow.

pub mod app;
pub mod menu_text;
pub mod model;
mod status_view;
mod theme;
pub mod views;

#[cfg(feature = "screenshot")]
pub mod testing;

/// The icons GPUI Kit's components draw (chevrons, a clear button, sort
/// arrows). An app registers them with `Application::with_assets`, or its
/// components draw no icons; the screenshot harness does.
pub use gpui_kit::assets::Assets as KitAssets;
pub use status_view::{ListenerLine, StatusModel, StatusView, open_status_window};
pub use theme::{FONT_FAMILY, Theme, register_fonts};
