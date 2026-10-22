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
//!
//! `docs/areas/ui.md` ("Models") has the formats table.

pub mod colors;
pub mod format;
pub mod query_bar;
pub mod trace_table;
