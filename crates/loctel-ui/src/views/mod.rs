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
//! - [`TraceView`]: the trace screen, made of the [`WaterfallView`] (the span
//!   tree with find, folding and the errors path), the [`InspectorView`] (the
//!   selected span) and the [`SpanLogsView`] (its logs).
//!
//! `docs/areas/ui.md` ("The service map", "The trace table", "The trace view")
//! has the look and the interactions.

mod inspector_view;
mod map_view;
mod span_logs_view;
mod trace_table_view;
mod trace_view;
mod waterfall_view;

pub use inspector_view::{
    COPY_JSON, InspectorEvent, InspectorView, NOTHING_SELECTED as INSPECTOR_EMPTY, TRACES_THROUGH,
};
pub use map_view::{Camera, EMPTY_HINT, EMPTY_TITLE, MAX_ZOOM, MIN_ZOOM, MapView};
pub use span_logs_view::{
    NO_LOGS, NOTHING_SELECTED as LOGS_EMPTY, PANEL_HEIGHT as LOGS_HEIGHT, SpanLogsView,
};
pub use trace_table_view::{
    CLEAR_FILTERS, COLUMNS, EMPTY_NO_DATA, EMPTY_NO_MATCH, HEADER_HEIGHT, ROW_HEIGHT, TablePart,
    TraceTableEvent, TraceTableView, column_widths,
};
pub use trace_view::{
    INSPECTOR_MAX_WIDTH, INSPECTOR_MIN_WIDTH, INSPECTOR_WIDTH, LOADING, MISSING, MISSING_HINT,
    TracePart, TraceView, TraceViewEvent,
};
pub use waterfall_view::{
    NO_ERRORS, ROW_HEIGHT as WATERFALL_ROW_HEIGHT, RULER_HEIGHT, WaterfallView,
};
