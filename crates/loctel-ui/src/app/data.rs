//! The live data of the home screen (design 9.2, M3 plan Task 5): the service
//! map and the trace table's rows, kept fresh from the store.
//!
//! [`HomeModel`] is an entity the home screen's views observe. It owns what the
//! toolbar and the table set (the time range, the two toggles, the query text,
//! pause) and a [`HomeData`], the result of the latest read: the placed map, the
//! rows, whether more traces matched than the table shows, the query bar's
//! state and the status. A timer reads again every 500 ms; a change the user
//! makes reads at once.
//!
//! **Reads never block a frame.** A read (a search over the traces, the service
//! map) runs on GPUI's background executor, on a copy of the inputs, and its
//! result comes back to the foreground, where the cheap pure part runs (the rows
//! and the layout of [`model`](crate::model), which need the session's service
//! colours and the map's memory). A read that a newer one has replaced is thrown
//! away when it arrives, and the timer never starts one while another is
//! running, so a slow search cannot pile up.
//!
//! `docs/areas/ui.md` ("Live data") and decision 0025 have the rules and the
//! reasons.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::Context;
use loctel_query::traceql::TraceQuery;
use loctel_store::{PageRequest, ServiceMap, Store, TimeWindow, TraceFilter, TraceSummary};

use super::backend::UiBackend;
use crate::StatusModel;
use crate::model::colors::ServiceColors;
use crate::model::map_layout::{LayoutMemory, MapLayout, layout};
use crate::model::query_bar::{self, QueryState};
use crate::model::trace_table::{self, TraceRow};

/// How often the home data is read again while the window is open and not
/// paused.
pub const HOME_REFRESH: Duration = Duration::from_millis(500);

/// The most rows the trace table shows: the newest traces that match.
pub const HOME_MAX_ROWS: usize = 1_000;

/// How long a trace must be to count for `Slower than 1 s`: at least this.
pub const SLOW_TRACE: Duration = Duration::from_secs(1);

const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// The window of time the map and the table cover (design 6.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TimeRange {
    Last5m,
    /// The default.
    #[default]
    Last15m,
    LastHour,
    /// Every trace the store holds.
    Everything,
}

impl TimeRange {
    /// The four ranges, in the order the picker lists them.
    pub const ALL: [TimeRange; 4] = [
        TimeRange::Last5m,
        TimeRange::Last15m,
        TimeRange::LastHour,
        TimeRange::Everything,
    ];

    /// What the picker says: `Last 5 minutes`, `Last 15 minutes`, `Last hour`,
    /// `Everything`.
    pub fn label(self) -> &'static str {
        match self {
            TimeRange::Last5m => "Last 5 minutes",
            TimeRange::Last15m => "Last 15 minutes",
            TimeRange::LastHour => "Last hour",
            TimeRange::Everything => "Everything",
        }
    }

    /// The earliest start the range keeps, as nanoseconds since the Unix epoch,
    /// when it is `now` (the store's clock): `now` minus the length of the
    /// range, and 0 if the clock reads less than that. `None` for
    /// [`Everything`](TimeRange::Everything): no limit. A trace that started at
    /// exactly this time is kept.
    pub fn since(self, now: u64) -> Option<u64> {
        self.seconds()
            .map(|seconds| now.saturating_sub(seconds * NANOS_PER_SECOND))
    }

    /// The length of the range in seconds, `None` for `Everything`.
    fn seconds(self) -> Option<u64> {
        match self {
            TimeRange::Last5m => Some(5 * 60),
            TimeRange::Last15m => Some(15 * 60),
            TimeRange::LastHour => Some(60 * 60),
            TimeRange::Everything => None,
        }
    }
}

/// What the home screen shows, as of the latest read.
#[derive(Clone, Debug, PartialEq)]
pub struct HomeData {
    /// The service map over the time range, placed and ready to paint.
    pub map: MapLayout,
    /// The trace table: the newest traces that match, at most
    /// [`HOME_MAX_ROWS`], newest first.
    pub rows: Vec<TraceRow>,
    /// More traces matched than `rows` holds (the footnote `Showing the newest
    /// 1,000`).
    pub truncated: bool,
    /// The state of the text in the query bar, as typed now. The rows may come
    /// from an earlier query: while this is `Invalid`, the last one that was
    /// good is what filters.
    pub query: QueryState,
    /// The status box's model, read with the data.
    pub status: StatusModel,
    /// When the store was read, on the store's clock (nanoseconds since the
    /// Unix epoch). 0 until the first read is done.
    pub refreshed_at: u64,
}

impl HomeData {
    /// What is shown before the first read has come back: nothing.
    fn empty(status: StatusModel) -> HomeData {
        HomeData {
            map: MapLayout::default(),
            rows: Vec::new(),
            truncated: false,
            query: QueryState::Empty,
            status,
            refreshed_at: 0,
        }
    }
}

/// The home screen's data and the settings that shape it. Make it with
/// `cx.new(|cx| HomeModel::new(backend, cx))` and observe it from the views.
///
/// Every setter takes the entity's context, redraws the observers when
/// something they show changed, and reads the store again when it changes what
/// is shown, so a click is answered by the next frame or two, not by the next
/// refresh.
pub struct HomeModel {
    backend: Rc<dyn UiBackend>,
    /// The session's service colours, shared with the table and the map (and
    /// handed out for the trace view): a service keeps its colour for the run.
    colors: Rc<RefCell<ServiceColors>>,
    /// Where the map's nodes were put, so they stay there from one read to
    /// the next.
    memory: LayoutMemory,
    data: HomeData,
    /// The text in the query bar.
    query_text: String,
    /// The service a click on the map put in the query, while the query is
    /// still that.
    selected: Option<String>,
    /// What the reads filter with: the latest query that parsed, or none for
    /// an empty bar. An `Invalid` text leaves it as it was.
    applied: Option<Arc<TraceQuery>>,
    range: TimeRange,
    errors_only: bool,
    slow_only: bool,
    paused: bool,
    /// Counts the reads started. Only the read of the latest one is applied.
    generation: u64,
    /// The latest read has not come back yet.
    reading: bool,
    /// A read has been applied: [`data`](HomeModel::data) is what the store held,
    /// not the empty data of a model made a moment ago.
    loaded: bool,
}

impl HomeModel {
    /// The model with the default range and no filter, reading from `backend`.
    /// It starts the first read at once (the data is empty until it comes back,
    /// a moment later) and the 500 ms timer, which stops when the model is
    /// dropped (the window closed).
    pub fn new(backend: Rc<dyn UiBackend>, cx: &mut Context<Self>) -> HomeModel {
        let status = StatusModel::from_status(&backend.status());
        let mut model = HomeModel {
            backend,
            colors: Rc::new(RefCell::new(ServiceColors::new())),
            memory: LayoutMemory::default(),
            data: HomeData::empty(status),
            query_text: String::new(),
            selected: None,
            applied: None,
            range: TimeRange::default(),
            errors_only: false,
            slow_only: false,
            paused: false,
            generation: 0,
            reading: false,
            loaded: false,
        };
        model.refresh_now(cx);

        cx.spawn(async move |home, cx| {
            loop {
                cx.background_executor().timer(HOME_REFRESH).await;
                // The model is gone once the window closes; that ends the loop.
                if home.update(cx, |home, cx| home.tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        model
    }

    /// What the screen shows now.
    pub fn data(&self) -> &HomeData {
        &self.data
    }

    /// Whether the first read of the store has come back. Until then
    /// [`data`](Self::data) is empty because nothing was read, not because the
    /// store has nothing: a view that says "no traces" waits for this.
    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// The text in the query bar, as typed (even when it does not parse).
    pub fn query_text(&self) -> &str {
        &self.query_text
    }

    /// The service the map has selected: the one whose filter is in the query
    /// bar because a click put it there. `None` once the text is anything else.
    pub fn selected_service(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn range(&self) -> TimeRange {
        self.range
    }

    pub fn errors_only(&self) -> bool {
        self.errors_only
    }

    pub fn slow_only(&self) -> bool {
        self.slow_only
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Whether the table is narrowed by anything the user set: a query that
    /// runs (the last one that parsed, while the text is not valid), `Errors
    /// only` or `Slower than 1 s`. The time range is not counted: it is a
    /// window, with a control of its own. The trace table says `No traces
    /// match` (and offers `Clear filters`) when this is true and nothing is
    /// shown, and `No traces yet` when it is false.
    pub fn has_filters(&self) -> bool {
        self.applied.is_some() || self.errors_only || self.slow_only
    }

    /// The session's service colours: the one list the table, the map and the
    /// trace view's waterfall number services by, so a service has one colour
    /// everywhere. Hand this to a view that colours services; do not make
    /// another.
    pub fn colors(&self) -> Rc<RefCell<ServiceColors>> {
        Rc::clone(&self.colors)
    }

    /// Sets the text of the query bar and reads it: [`query_bar::validate`]
    /// runs at once, so [`data`](Self::data) has the new state when this
    /// returns. A query that parses (or an empty bar) filters the table from the
    /// next read, which this starts. One that does not parse keeps the previous
    /// rows and keeps filtering with the last good query; the state says why.
    ///
    /// A text other than the one a click on the map wrote ends that selection.
    pub fn set_query(&mut self, text: &str, cx: &mut Context<Self>) {
        let state = query_bar::validate(text);
        if self
            .selected
            .as_deref()
            .is_some_and(|service| text != query_bar::service_filter(service))
        {
            self.selected = None;
        }
        self.query_text = text.to_owned();
        let reads = match &state {
            QueryState::Empty => {
                self.applied = None;
                true
            }
            QueryState::Valid(query) => {
                self.applied = Some(Arc::clone(query));
                true
            }
            QueryState::Invalid { .. } => false,
        };
        self.data.query = state;
        cx.notify();
        if reads {
            self.refresh_now(cx);
        }
    }

    /// Keeps only the traces that failed or have errors inside.
    pub fn set_errors_only(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.errors_only != on {
            self.errors_only = on;
            self.settings_changed(cx);
        }
    }

    /// Keeps only the traces that lasted [`SLOW_TRACE`] (1 s) or more.
    pub fn set_slow_only(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.slow_only != on {
            self.slow_only = on;
            self.settings_changed(cx);
        }
    }

    /// Sets the window of time the map and the table cover.
    pub fn set_range(&mut self, range: TimeRange, cx: &mut Context<Self>) {
        if self.range != range {
            self.range = range;
            self.settings_changed(cx);
        }
    }

    /// Takes off every filter [`has_filters`](Self::has_filters) counts: the
    /// query bar is emptied, `Errors only` and `Slower than 1 s` go off (and a
    /// service the map had selected is no longer). The range and the pause stay.
    /// One read follows, whatever changed.
    pub fn clear_filters(&mut self, cx: &mut Context<Self>) {
        self.errors_only = false;
        self.slow_only = false;
        // An empty bar always parses: this empties the text, ends the
        // selection, redraws and starts the one read (after the toggles changed).
        self.set_query("", cx);
    }

    /// Freezes the view (`true`) or lets it run again (`false`, which reads at
    /// once to catch up). Ingest carries on either way. Pause stops the timer
    /// only: a change the user makes while paused (the query, a toggle, the
    /// range, a click on a service) reads the store, because they asked for it.
    pub fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        if self.paused == paused {
            return;
        }
        self.paused = paused;
        cx.notify();
        if !paused {
            self.refresh_now(cx);
        }
    }

    /// Clicks a service on the map (`Some`): its filter,
    /// `{ resource.service.name = "..." }`, goes into the query bar and the
    /// table follows. `None` clears the selection: the query goes back to empty,
    /// but only when it is the selected service's filter. A query the user typed
    /// is not wiped by a click on empty space.
    pub fn select_service(&mut self, service: Option<String>, cx: &mut Context<Self>) {
        match service {
            Some(service) => {
                self.set_query(&query_bar::service_filter(&service), cx);
                // After the query, which ends any earlier selection. The
                // observers hear of both when this call is over.
                self.selected = Some(service);
            }
            None => {
                if self.selected.is_some() {
                    self.set_query("", cx);
                }
            }
        }
    }

    /// Reads the store now, on the background executor, whatever the timer and
    /// pause say. It returns at once; the data changes when the read comes back
    /// (the observers are told then). A read still running is replaced: its
    /// result will not be used.
    pub fn refresh_now(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.reading = true;
        let plan = ReadPlan {
            store: self.backend.store(),
            range: self.range,
            errors_only: self.errors_only,
            slow_only: self.slow_only,
            query: self.applied.clone(),
        };
        cx.spawn(async move |home, cx| {
            let read = cx
                .background_executor()
                .spawn(async move { plan.read() })
                .await;
            // The model is gone once the window closes: nothing to apply to.
            home.update(cx, |home, cx| home.apply(generation, read, cx))
                .ok();
        })
        .detach();
    }

    /// A change of range or toggle: redraw the controls and read.
    fn settings_changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        self.refresh_now(cx);
    }

    /// The timer: a read, unless paused or one is already running.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if !self.paused && !self.reading {
            self.refresh_now(cx);
        }
    }

    /// Takes the result of a read: the rows and the layout, on the foreground
    /// where the session's colours and the map's memory live. A result that is
    /// not the latest read's is dropped. Observers are told only when something
    /// they draw changed, so an idle store costs no frames.
    fn apply(&mut self, generation: u64, read: Read, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        self.reading = false;
        self.loaded = true;

        let utc_offset = self.backend.utc_offset();
        let (rows, map) = {
            // The table first: the services of the newest traces are the first
            // the session has seen, and the map's colours follow theirs.
            let mut colors = self.colors.borrow_mut();
            let rows = trace_table::rows(&read.summaries, &mut colors, utc_offset);
            let map = layout(&read.map, read.window_secs, &mut colors, &mut self.memory);
            (rows, map)
        };
        let status = StatusModel::from_status(&self.backend.status());

        let changed = self.data.map != map
            || self.data.rows != rows
            || self.data.truncated != read.truncated
            || self.data.status != status;
        self.data.map = map;
        self.data.rows = rows;
        self.data.truncated = read.truncated;
        self.data.status = status;
        self.data.refreshed_at = read.now;
        if changed {
            cx.notify();
        }
    }
}

/// Everything a read needs, copied out of the model so it can run on another
/// thread.
struct ReadPlan {
    store: Store,
    range: TimeRange,
    errors_only: bool,
    slow_only: bool,
    query: Option<Arc<TraceQuery>>,
}

/// What a read found, before the rows and the layout are made from it.
struct Read {
    /// The store's clock when the read began.
    now: u64,
    /// The newest matching traces, newest first, at most [`HOME_MAX_ROWS`].
    summaries: Vec<TraceSummary>,
    truncated: bool,
    map: ServiceMap,
    /// The length of time the map was counted over, for its rates.
    window_secs: f64,
}

impl ReadPlan {
    /// Searches the traces and counts the map. The table's filter is the range,
    /// errors only, slow only and the query together; the map's is the range
    /// alone (it is the picture of everything, the table is the list of what
    /// you asked for).
    fn read(&self) -> Read {
        let now = self.store.now_unix_nano();
        let since = self.range.since(now);
        let filter = TraceFilter {
            since_unix_nano: since,
            service: None,
            errors_only: self.errors_only,
            min_duration_nanos: self
                .slow_only
                .then(|| u64::try_from(SLOW_TRACE.as_nanos()).unwrap_or(u64::MAX)),
            query: self.query.clone(),
        };
        let page = self.store.search_traces(
            &filter,
            &PageRequest {
                limit: HOME_MAX_ROWS,
                cursor: None,
            },
        );
        let map = self.store.service_map(TimeWindow {
            since_unix_nano: since.unwrap_or(0),
        });
        Read {
            now,
            truncated: page.next_cursor.is_some(),
            summaries: page.items,
            map,
            window_secs: self.window_secs(now),
        }
    }

    /// How long the map's counts cover: the range, or for `Everything` from the
    /// oldest trace the store holds to now (at least a second, so one fresh
    /// trace is not a rate of thousands). 0 for an empty store.
    fn window_secs(&self, now: u64) -> f64 {
        match self.range.seconds() {
            Some(seconds) => seconds as f64,
            None => self.store.oldest_start_unix_nano().map_or(0.0, |oldest| {
                (now.saturating_sub(oldest) as f64 / NANOS_PER_SECOND as f64).max(1.0)
            }),
        }
    }
}
