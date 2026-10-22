//! The views of the main window's screens (Milestone 3): thin GPUI views over
//! the models of `src/model` and the live data of `src/app`. They lay out what
//! they are given and hold no rules of their own; the rules (what a node says,
//! where it goes, which rows there are) live in the models and are tested
//! there.
//!
//! - [`MapView`]: the live service map of the home screen, drawn on a canvas,
//!   with pan, zoom and a click on a service that filters the table.
//! - [`TraceTableView`]: the trace table under it, with the TraceQL bar, the two
//!   toggles and a click on a row that opens the trace.
//!
//! `docs/areas/ui.md` ("The service map", "The trace table") has the look and
//! the interactions.

mod map_view;
mod trace_table_view;

pub use map_view::{Camera, EMPTY_HINT, EMPTY_TITLE, MAX_ZOOM, MIN_ZOOM, MapView};
pub use trace_table_view::{
    CLEAR_FILTERS, COLUMNS, EMPTY_NO_DATA, EMPTY_NO_MATCH, HEADER_HEIGHT, ROW_HEIGHT, TablePart,
    TraceTableEvent, TraceTableView, column_widths,
};
