//! The live data of the trace view (design 9.2, M3 plan Task 8): one trace,
//! kept fresh from the store while it is open.
//!
//! [`TraceModel`] is an entity the views of the trace screen observe (the
//! header, the waterfall, the inspector, the span's logs). It owns the trace's
//! snapshot, the [`Waterfall`] built from it, what the user did to the
//! waterfall (what is folded, the selection, the errors filter, the find text)
//! and the logs of the selected span.
//!
//! - **While the trace is arriving** it reads the store every 500 ms
//!   ([`OPEN_REFRESH`]), and every 2 s ([`DONE_REFRESH`]) once the store calls
//!   it complete. A new snapshot goes through [`Waterfall::rebuild`], so the
//!   folded spans, the selection and the errors filter stay (they are kept by
//!   span ID) while rows arrive, the placeholder `missing parent` goes away and
//!   the tree is re-rooted when the root comes last.
//! - **Reads never block a frame.** Every store read (the trace, a span's
//!   logs) runs on GPUI's background executor on a copy of its inputs, and the
//!   result comes back to the foreground, where the pure models run (the
//!   header, the waterfall, the details). A read that a newer one replaced is
//!   thrown away, and the next read is timed from the moment the last one came
//!   back, so a slow one cannot pile them up. Once the trace is complete and has
//!   the spans the store holds, a refresh only counts them (no copy of a
//!   100,000-span trace every two seconds).
//! - **Selection** follows the span ID. A new model selects the span that
//!   explains the failure, else the root ([`initial_selection`]), as soon as
//!   there is one, and asks the list to scroll to it
//!   ([`TraceModelEvent::Reveal`]).
//!
//! `docs/areas/ui.md` ("The trace view") and decision 0028 have the rules and
//! the reasons.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, EventEmitter};
use loctel_model::{LogRecord, SpanId, TraceId};
use loctel_store::{Store, TraceSnapshot};

use super::backend::UiBackend;
use crate::model::colors::ServiceColors;
use crate::model::inspector::{SpanDetails, span_details};
use crate::model::trace_view::{LogLine, TraceHeader, find_status, header, log_lines};
use crate::model::waterfall::{Waterfall, initial_selection};

/// How often an open trace is read again while the store does not yet call it
/// complete.
pub const OPEN_REFRESH: Duration = Duration::from_millis(500);

/// How often it is read once it is complete (a late span can still come).
pub const DONE_REFRESH: Duration = Duration::from_secs(2);

/// Whether the trace has come back from the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadState {
    /// The first read has not come back yet.
    Loading,
    /// The store has the trace: [`TraceModel::waterfall`] and the rest are
    /// there.
    Loaded,
    /// The first read found no such trace (it was never received, or has been
    /// evicted or cleared).
    Missing,
}

/// What the model tells the views besides "something changed".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceModelEvent {
    /// Bring the row of this span into view: the trace was just opened on it,
    /// or find jumped to it.
    Reveal(SpanId),
}

impl EventEmitter<TraceModelEvent> for TraceModel {}

/// One trace, live. Make it with
/// `cx.new(|cx| TraceModel::new(backend, trace_id, colors, cx))` and observe it
/// from the views.
pub struct TraceModel {
    backend: Rc<dyn UiBackend>,
    trace_id: TraceId,
    /// The session's service colours, the home screen's (a service keeps its
    /// colour for the run).
    colors: Rc<RefCell<ServiceColors>>,
    state: LoadState,
    snapshot: Option<Arc<TraceSnapshot>>,
    header: Option<TraceHeader>,
    waterfall: Option<Waterfall>,
    /// Whether any span with children is folded (the button says `Expand all`).
    any_folded: bool,
    /// The errors filter of the waterfall (the model is the only one that sets
    /// it, so this is what the waterfall has).
    errors_path_only: bool,
    /// The selected span's details, from the snapshot.
    details: Option<SpanDetails>,
    /// The selected span's logs, and whether they have been read yet.
    logs: Vec<LogLine>,
    logs_loaded: bool,
    find_text: String,
    /// The rows (positions among the rows shown) that match the find text.
    matches: Vec<usize>,
    /// A span to select as soon as the trace has it ([`prefer_selection`]
    /// (Self::prefer_selection)), until the user selects one themselves.
    preferred: Option<SpanId>,
    /// The user has chosen a span (a click, a find jump): the model no longer
    /// moves the selection by itself.
    user_selected: bool,
    /// Counts the trace reads started; only the latest one is applied.
    generation: u64,
    /// The same for the logs of the selected span.
    logs_generation: u64,
}

impl TraceModel {
    /// The model of trace `trace_id`, reading from `backend`. It starts the
    /// first read at once (the state is [`LoadState::Loading`] until it comes
    /// back, a moment later) and the refresh timer, which stops when the model
    /// is dropped (the screen closed). `colors` is the home screen's
    /// ([`HomeModel::colors`](super::HomeModel::colors)) so a service has one
    /// colour everywhere; a standalone view may pass a fresh one.
    pub fn new(
        backend: Rc<dyn UiBackend>,
        trace_id: TraceId,
        colors: Rc<RefCell<ServiceColors>>,
        cx: &mut Context<Self>,
    ) -> TraceModel {
        let mut model = TraceModel {
            backend,
            trace_id,
            colors,
            state: LoadState::Loading,
            snapshot: None,
            header: None,
            waterfall: None,
            any_folded: false,
            errors_path_only: false,
            details: None,
            logs: Vec::new(),
            logs_loaded: false,
            find_text: String::new(),
            matches: Vec::new(),
            preferred: None,
            user_selected: false,
            generation: 0,
            logs_generation: 0,
        };
        model.refresh_now(cx);
        model
    }

    pub fn trace_id(&self) -> TraceId {
        self.trace_id
    }

    pub fn state(&self) -> LoadState {
        self.state
    }

    /// What the header says. `None` until the trace is loaded.
    pub fn header(&self) -> Option<&TraceHeader> {
        self.header.as_ref()
    }

    /// The waterfall: its rows, the selection, what is folded. `None` until the
    /// trace is loaded.
    pub fn waterfall(&self) -> Option<&Waterfall> {
        self.waterfall.as_ref()
    }

    /// The trace as of the latest read that changed it.
    pub fn snapshot(&self) -> Option<&Arc<TraceSnapshot>> {
        self.snapshot.as_ref()
    }

    /// The selected span, if any.
    pub fn selected(&self) -> Option<SpanId> {
        self.waterfall.as_ref().and_then(Waterfall::selected)
    }

    /// The selected span's details for the inspector. `None` with nothing
    /// selected.
    pub fn details(&self) -> Option<&SpanDetails> {
        self.details.as_ref()
    }

    /// The selected span's logs, oldest first.
    pub fn logs(&self) -> &[LogLine] {
        &self.logs
    }

    /// Whether [`logs`](Self::logs) has been read for the selected span (an
    /// empty list before that is not "no logs").
    pub fn logs_loaded(&self) -> bool {
        self.logs_loaded
    }

    /// The service's number in the session's colours (the view takes `% 5`).
    pub fn service_color(&self, service: &str) -> usize {
        self.colors.borrow_mut().index(service)
    }

    /// Whether the trace is complete (the store has seen no new span for its
    /// quiet period). `false` until loaded.
    pub fn complete(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.complete)
    }

    /// How long until the next read: [`OPEN_REFRESH`] while the trace is
    /// arriving (and before it has loaded), [`DONE_REFRESH`] once it is
    /// complete.
    pub fn refresh_interval(&self) -> Duration {
        if self.complete() {
            DONE_REFRESH
        } else {
            OPEN_REFRESH
        }
    }

    /// Whether any span is folded: the toggle button says `Expand all` then,
    /// `Collapse all` otherwise.
    pub fn any_folded(&self) -> bool {
        self.any_folded
    }

    /// Whether the waterfall shows only the errors and the way to them.
    pub fn errors_path_only(&self) -> bool {
        self.errors_path_only
    }

    // --- what the user does ----------------------------------------------------

    /// Asks for `span` to be the selected span as soon as the trace has it, in
    /// place of the one a trace opens on (the failure, else the root): what
    /// `Open trace` of a log does. It is meant for a model that was just made.
    /// A span the trace does not have yet is waited for (the trace may still be
    /// arriving); once the user selects a span themselves, the request is over.
    pub fn prefer_selection(&mut self, span: SpanId) {
        self.preferred = Some(span);
        self.user_selected = false;
    }

    /// Selects the span `id`: its ancestors open so the row is shown, the
    /// inspector shows it and its logs are read. An ID the trace does not have
    /// (the placeholder's) changes nothing.
    pub fn select(&mut self, id: SpanId, cx: &mut Context<Self>) {
        let (Some(waterfall), Some(snapshot)) = (&mut self.waterfall, &self.snapshot) else {
            return;
        };
        if span_details(snapshot, id).is_none() {
            return;
        }
        self.user_selected = true;
        let before = waterfall.selected();
        waterfall.select(id);
        self.changed(before, cx);
    }

    /// Folds the children of `id`, or opens them again.
    pub fn toggle(&mut self, id: SpanId, cx: &mut Context<Self>) {
        self.edit(cx, |waterfall| waterfall.toggle(id));
    }

    /// Folds every span that has children.
    pub fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.edit(cx, Waterfall::collapse_all);
    }

    /// Opens every span.
    pub fn expand_all(&mut self, cx: &mut Context<Self>) {
        self.edit(cx, Waterfall::expand_all);
    }

    /// Shows only the erroring spans and their ancestors, or everything.
    pub fn set_errors_path_only(&mut self, on: bool, cx: &mut Context<Self>) {
        self.errors_path_only = on;
        self.edit(cx, |waterfall| waterfall.set_errors_path_only(on));
    }

    /// Sets the find text: the rows that match it are marked and counted. It
    /// does not move the selection (Enter does, [`find_next`](Self::find_next)).
    pub fn set_find(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.find_text != text {
            self.find_text = text.to_owned();
            self.rematch();
            cx.notify();
        }
    }

    /// The text in the find box.
    pub fn find_text(&self) -> &str {
        &self.find_text
    }

    /// The rows that match the find text, top to bottom (positions among the
    /// rows shown). The placeholder row is never one.
    pub fn matches(&self) -> &[usize] {
        &self.matches
    }

    /// Which match the selected span is (its position in
    /// [`matches`](Self::matches), from 0), `None` when it is not one.
    pub fn current_match(&self) -> Option<usize> {
        let waterfall = self.waterfall.as_ref()?;
        let row = waterfall.row_of(waterfall.selected()?)?;
        self.matches.iter().position(|&m| m == row)
    }

    /// What the count next to the find box says: `2 of 5`, `5 matches`, `No
    /// matches`, or nothing for an empty box.
    pub fn find_label(&self) -> String {
        find_status(
            self.find_text.is_empty(),
            self.matches.len(),
            self.current_match(),
        )
    }

    /// Jumps to the next match: selects the first match below the selected row
    /// (the first match of all when none is selected or the selection is at or
    /// below the last), wrapping round, and asks the list to scroll to it.
    /// Nothing happens without matches.
    pub fn find_next(&mut self, cx: &mut Context<Self>) {
        let Some(waterfall) = &self.waterfall else {
            return;
        };
        let selected_row = waterfall.selected().and_then(|id| waterfall.row_of(id));
        let next = selected_row
            .and_then(|row| self.matches.iter().copied().find(|&m| m > row))
            .or_else(|| self.matches.first().copied());
        let Some(row) = next else {
            return;
        };
        let id = waterfall.rows()[row].span_id;
        self.select(id, cx);
        cx.emit(TraceModelEvent::Reveal(id));
    }

    /// Reads the trace now, whatever the timer says. It returns at once; the
    /// data changes when the read comes back (the observers are told then).
    pub fn refresh_now(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let plan = ReadPlan {
            store: self.backend.store(),
            trace_id: self.trace_id,
            known: self
                .snapshot
                .as_ref()
                .map(|snapshot| (snapshot.spans.len(), snapshot.complete)),
            span: self.selected(),
        };
        cx.spawn(async move |model, cx| {
            let read = cx
                .background_executor()
                .spawn(async move { plan.read() })
                .await;
            // The model is gone once the screen closes: nothing to apply to.
            model
                .update(cx, |model, cx| model.apply(generation, read, cx))
                .ok();
        })
        .detach();
    }

    // --- inside ------------------------------------------------------------------

    /// Starts the wait for the next read, once a read has come back: the
    /// interval is the one the trace's state calls for *now* (so a trace that
    /// just went complete is read every 2 s from then on). The wait ends in a
    /// read unless one was started in the meantime (a read the user asked for),
    /// which has its own wait.
    fn schedule_next(&mut self, cx: &mut Context<Self>) {
        let generation = self.generation;
        let wait = self.refresh_interval();
        cx.spawn(async move |model, cx| {
            cx.background_executor().timer(wait).await;
            // The model is gone once the screen closes; that ends the chain.
            model
                .update(cx, |model, cx| {
                    if model.generation == generation {
                        model.refresh_now(cx);
                    }
                })
                .ok();
        })
        .detach();
    }

    /// Changes the waterfall with `change` (folding, the filter), then brings
    /// everything that depends on its rows up to date.
    fn edit(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Waterfall)) {
        let Some(waterfall) = &mut self.waterfall else {
            return;
        };
        let before = waterfall.selected();
        change(waterfall);
        self.changed(before, cx);
    }

    /// The waterfall changed (`selection_before` is what was selected before):
    /// the find matches, the folded flag and, when the selection moved, the
    /// details and the logs follow, and the observers redraw.
    fn changed(&mut self, selection_before: Option<SpanId>, cx: &mut Context<Self>) {
        self.rematch();
        self.refresh_flags();
        if self.selected() != selection_before {
            self.refresh_details();
            self.logs.clear();
            self.logs_loaded = false;
            self.read_logs(cx);
        }
        cx.notify();
    }

    fn refresh_flags(&mut self) {
        self.any_folded = self
            .waterfall
            .as_ref()
            .is_some_and(|waterfall| waterfall.rows().iter().any(|row| row.collapsed));
    }

    /// The details of the selected span, from the snapshot.
    fn refresh_details(&mut self) {
        self.details = match (&self.snapshot, self.selected()) {
            (Some(snapshot), Some(id)) => span_details(snapshot, id),
            _ => None,
        };
    }

    /// The rows that match the find text, without the placeholder.
    fn rematch(&mut self) {
        self.matches = match &self.waterfall {
            Some(waterfall) => {
                let mut matches = waterfall.find(&self.find_text);
                matches.retain(|&row| !waterfall.rows()[row].is_placeholder());
                matches
            }
            None => Vec::new(),
        };
    }

    /// Reads the logs of the selected span on the background executor.
    fn read_logs(&mut self, cx: &mut Context<Self>) {
        let Some(span) = self.selected() else {
            return;
        };
        self.logs_generation += 1;
        let generation = self.logs_generation;
        let store = self.backend.store();
        let trace = self.trace_id;
        cx.spawn(async move |model, cx| {
            let logs = cx
                .background_executor()
                .spawn(async move { store.logs_for_span(trace, span) })
                .await;
            model
                .update(cx, |model, cx| {
                    if generation == model.logs_generation {
                        model.set_logs(span, &logs, cx);
                    }
                })
                .ok();
        })
        .detach();
    }

    /// Takes the logs of `span` when it is still the selected span. Observers
    /// are told only when what they draw changed.
    fn set_logs(&mut self, span: SpanId, logs: &[LogRecord], cx: &mut Context<Self>) {
        if self.selected() != Some(span) {
            return;
        }
        let lines = log_lines(logs, self.backend.utc_offset());
        if !self.logs_loaded || self.logs != lines {
            self.logs = lines;
            self.logs_loaded = true;
            cx.notify();
        }
    }

    /// Takes the result of a read. A result that is not the latest read's is
    /// dropped.
    fn apply(&mut self, generation: u64, read: Read, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        match read.trace {
            TraceRead::Unchanged => {}
            TraceRead::Gone => {
                // A trace that was there and is gone (evicted, cleared) keeps
                // what the screen shows; one never found is missing.
                if self.snapshot.is_none() {
                    self.state = LoadState::Missing;
                    cx.notify();
                }
            }
            TraceRead::Snapshot(snapshot) => self.take_snapshot(snapshot, cx),
        }
        if let Some((span, logs)) = read.logs {
            self.set_logs(span, &logs, cx);
        }
        // Last, so the wait is the one the trace's new state calls for.
        self.schedule_next(cx);
    }

    /// Builds (the first time) or rebuilds the waterfall from a snapshot that
    /// has more or fewer spans, or went complete.
    fn take_snapshot(&mut self, snapshot: TraceSnapshot, cx: &mut Context<Self>) {
        let unchanged = self.snapshot.as_ref().is_some_and(|old| {
            old.spans.len() == snapshot.spans.len() && old.complete == snapshot.complete
        });
        if unchanged {
            return;
        }
        let selection_before = self.selected();
        let header = header(&snapshot, self.backend.utc_offset());
        {
            let mut colors = self.colors.borrow_mut();
            match &mut self.waterfall {
                Some(waterfall) => waterfall.rebuild(&snapshot, &mut colors),
                None => self.waterfall = Some(Waterfall::new(&snapshot, &mut colors)),
            }
        }
        // The first selection: the span asked for ([`prefer_selection`]
        // (Self::prefer_selection)) once the trace has it, else the failure,
        // else the root, as soon as there is one. After that the selection is
        // the user's.
        let wanted = self
            .preferred
            .filter(|&id| !self.user_selected && span_details(&snapshot, id).is_some());
        let reveal = match &mut self.waterfall {
            Some(waterfall) => {
                if let Some(id) = wanted {
                    self.preferred = None;
                    (waterfall.selected() != Some(id)).then(|| {
                        waterfall.select(id);
                        id
                    })
                } else if waterfall.selected().is_none() {
                    initial_selection(&snapshot).inspect(|&id| waterfall.select(id))
                } else {
                    None
                }
            }
            None => None,
        };
        self.header = Some(header);
        self.snapshot = Some(Arc::new(snapshot));
        self.state = LoadState::Loaded;
        // The selected span stays, but the trace around it may have moved (a
        // root that came last starts earlier): its details are read again.
        self.refresh_details();
        self.changed(selection_before, cx);
        if let Some(id) = reveal {
            cx.emit(TraceModelEvent::Reveal(id));
        }
    }
}

/// Everything a read needs, copied out of the model so it can run on another
/// thread.
struct ReadPlan {
    store: Store,
    trace_id: TraceId,
    /// The span count and completeness of the snapshot the model has.
    known: Option<(usize, bool)>,
    /// The span whose logs to read along: the selected one.
    span: Option<SpanId>,
}

/// What a read found.
struct Read {
    trace: TraceRead,
    logs: Option<(SpanId, Vec<LogRecord>)>,
}

enum TraceRead {
    /// The model's snapshot is the trace as it is: a complete trace with the
    /// spans the store holds.
    Unchanged,
    /// The store does not hold the trace.
    Gone,
    Snapshot(TraceSnapshot),
}

impl ReadPlan {
    fn read(&self) -> Read {
        // A count under the read lock, without a copy: a complete trace that
        // has not grown is left alone (completeness only ever turns on, and a
        // new span changes the count).
        let held = self
            .store
            .with_trace_spans(self.trace_id, |spans| spans.len());
        let trace = match (held, self.known) {
            (None, _) => TraceRead::Gone,
            (Some(held), Some((known, true))) if held == known => TraceRead::Unchanged,
            _ => self
                .store
                .trace(self.trace_id)
                .map_or(TraceRead::Gone, TraceRead::Snapshot),
        };
        let logs = self
            .span
            .map(|span| (span, self.store.logs_for_span(self.trace_id, span)));
        Read { trace, logs }
    }
}
