//! The models of the main window: everything a view shows that is not a
//! pixel. They are plain Rust over the store's types, with no GPUI in them
//! (a colour is an index into `Theme::service_palette`), so the rules of the
//! design (the formats, the failure display of 9.4, the colours of 9.3) are
//! tested with plain `assert_eq!`s, and a view only lays out what it is given.
//!
//! - [`format`]: durations, clock times, rates, error shares, cut-off text.
//! - [`colors`]: [`ServiceColors`](colors::ServiceColors), the colour a
//!   service keeps for the whole run.
//! - [`trace_table`]: the rows of the trace table, and the `Failed in` text.
//! - [`query_bar`]: the state of the TraceQL bar, and the query a click on a
//!   service writes into it.
//! - [`map_layout`]: where each node of the service map goes, and what it
//!   says, kept stable as services come and go.
//! - [`waterfall`]: the rows of the trace view's waterfall (tree order, bars,
//!   folding, find, the errors path), kept as the trace fills in.
//! - [`inspector`]: the text of the selected span's inspector panel.
//! - [`trace_view`]: the trace view's header, the find box's count and the
//!   lines of the selected span's logs.
//!
//! `docs/areas/ui.md` ("Models") has the formats table.

pub mod colors;
pub mod format;
pub mod inspector;
pub mod map_layout;
pub mod query_bar;
pub mod trace_table;
pub mod trace_view;
pub mod waterfall;
