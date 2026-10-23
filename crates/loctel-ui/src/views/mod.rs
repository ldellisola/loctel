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
//! - [`LogsView`]: the Logs screen (Milestone 4): the LogQL bar, the level
//!   toggles, the [`HistogramView`] and the list of lines, which expand where they
//!   stand.
//!
//! - [`RulesView`]: the Discard rules screen (Milestone 4): the cards of the
//!   rules with their switches and counts, and beside them the
//!   [`RuleEditorView`] of the rule that is open (its fields, live validation and
//!   the preview of the last five minutes).
//!
//! - [`SettingsView`]: the Settings window's form (Milestone 4): the three port
//!   inputs with the state of their endpoints, the login switch where there is a
//!   login item, `Apply` and `Close`.
//!
//! - [`PopoverView`]: the menu bar popover (Milestone 4): the status pill, the four
//!   stats, the endpoints with `Copy`, the rules line and the actions. It does
//!   nothing itself: every action is a call on its [`PopoverActions`], which
//!   `loctel-macos` implements and the tests record.
//!
//! `docs/areas/ui.md` ("The service map", "The trace table", "The trace view",
//! "The Logs screen", "The Discard rules screen", "The Settings window", "The
//! popover") has the look and the interactions.

mod histogram_view;
mod inspector_view;
mod logs_view;
mod map_view;
mod popover_view;
mod rule_editor_view;
mod rules_view;
mod settings_view;
mod span_logs_view;
mod trace_table_view;
mod trace_view;
mod waterfall_view;

pub use histogram_view::HistogramView;
pub use inspector_view::{
    COPY_JSON, InspectorEvent, InspectorView, NOTHING_SELECTED as INSPECTOR_EMPTY, TRACES_THROUGH,
};
pub use logs_view::{
    DISCARD_LIKE_THIS, LOGS_EMPTY_NO_DATA, LOGS_EMPTY_NO_MATCH, LogsPart, LogsView, LogsViewEvent,
    OPEN_TRACE, SHOW_AROUND,
};
pub use map_view::{Camera, EMPTY_HINT, EMPTY_TITLE, MAX_ZOOM, MIN_ZOOM, MapView};
pub(crate) use popover_view::bind_keys as bind_popover_keys;
pub use popover_view::{
    POPOVER_RADIUS, POPOVER_REFRESH, POPOVER_WIDTH, PopoverActions, PopoverPart, PopoverView,
    popover_height,
};
pub use rule_editor_view::{
    APPLIES_LABEL, CANCEL, DELETE, DELETE_ASK, DELETE_CONFIRM, DELETE_KEEP, DISCARD_DRAFT,
    EDITOR_WIDTH, EXTERNAL_CHANGE, KEEP_DRAFT, LANGUAGE_LOGS, LANGUAGE_TRACES, NAME_LABEL,
    QUERY_LABEL, RuleEditorView, SAVE, SCOPE_SPAN, SCOPE_TITLE, SCOPE_TRACE,
};
pub use rules_view::{NEW_RULE, RulesPart, RulesView, TITLE as RULES_TITLE};
pub use settings_view::{
    APPLY, CLOSE, ENDPOINTS, LOGIN_LABEL, REFRESH as SETTINGS_REFRESH, STARTUP, SettingsPart,
    SettingsView, TITLE as SETTINGS_TITLE,
};
pub use span_logs_view::{
    NO_LOGS, NOTHING_SELECTED as LOGS_EMPTY, OPEN_IN_LOGS, PANEL_HEIGHT as LOGS_HEIGHT,
    SpanLogsEvent, SpanLogsView,
};
pub use trace_table_view::{
    CLEAR_FILTERS, COLUMNS, EMPTY_NO_DATA, EMPTY_NO_MATCH, HEADER_HEIGHT, ROW_HEIGHT, TablePart,
    TraceTableEvent, TraceTableView, column_widths,
};
pub use trace_view::{
    BACK_LABEL, INSPECTOR_MAX_WIDTH, INSPECTOR_MIN_WIDTH, INSPECTOR_WIDTH, LOADING, MISSING,
    MISSING_HINT, NO_LONGER_HELD, NO_LONGER_HELD_HINT, TracePart, TraceView, TraceViewEvent,
};
pub use waterfall_view::{
    NO_ERRORS, ROW_HEIGHT as WATERFALL_ROW_HEIGHT, RULER_HEIGHT, WaterfallView,
};
