//! The live data of the main window: the home screen's (M3 plan, Task 5:
//! `HomeModel` keeps the map and the trace table's rows fresh from the store
//! every 500 ms, behind the time range, the toggles and the query bar) and the
//! trace screen's (Task 8: `TraceModel` keeps an open trace fresh, every 500 ms
//! until it is complete and every 2 s after, and its waterfall fills in as
//! spans arrive).
//!
//! The tests run the model in a headless app with no window. Its clock stands
//! still until a test moves it, so the 500 ms refresh fires when the test says;
//! the store has its own `ManualClock`, so what "now" is for the range filter is
//! the test's choice too. The traces are the fixed fake mesh (`CheckoutFailure`
//! and `CheckoutSuccess`, 15 and 14 spans, the failing one 1.84 s long) and
//! `HealthChecks` (one fast span per trace), plus a few one-span traces built by
//! hand where a test needs a trace of an exact length or status.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AppContext as _, Context, Entity, HeadlessAppContext};
use loctel_ingest::{logs_from_request, spans_from_request};
use loctel_meshgen::{Generated, Scenario, generate};
use loctel_model::{AttrValue, Resource, Span, SpanId, SpanKind, StatusCode, TraceId};
use loctel_store::{ManualClock, NodeKind, PageRequest, Store, StoreConfig, TraceFilter};
use loctel_ui::app::{
    DONE_REFRESH, HOME_MAX_ROWS, HomeData, HomeModel, LoadState, OPEN_REFRESH, TimeRange,
    TraceModel,
};
use loctel_ui::model::query_bar::{QueryState, service_filter};
use loctel_ui::model::waterfall::MISSING_PARENT;
use loctel_ui::testing::{Driver, FixedBackend, headless_app, listening_status};
use loctel_ui::views::TraceView;
use loctel_ui::{StatusModel, Theme};

/// 2027-01-15T08:00:00Z. Where the failing checkout starts.
const BASE: u64 = 1_800_000_000_000_000_000;
const MS: u64 = 1_000_000;
const SECOND: u64 = 1_000 * MS;
const MINUTE: u64 = 60 * SECOND;
const SEED: u64 = 7;

const PAYMENTS: &str = "{ resource.service.name = \"payments-api\" }";

/// The model in a headless app, over a store the test fills and the clock that
/// dates it.
struct Fixture {
    // Fields drop in order: the handle to the model goes before the app that
    // owns it.
    home: Entity<HomeModel>,
    store: Store,
    clock: Arc<ManualClock>,
    cx: HeadlessAppContext,
}

impl Fixture {
    /// An empty store whose clock reads `now`, and the model over it, with its
    /// first read done.
    fn new(now: u64) -> Fixture {
        let clock = Arc::new(ManualClock::new(now));
        let store = Store::new(StoreConfig::default(), clock.clone());
        let backend = Rc::new(FixedBackend::new(store.clone()));
        let mut cx = headless_app();
        let home = cx.new(|cx| HomeModel::new(backend, cx));
        let mut fixture = Fixture {
            home,
            store,
            clock,
            cx,
        };
        fixture.settle();
        fixture
    }

    /// Puts a generated scenario into the store, whole.
    fn ingest(&self, generated: &Generated) {
        for batch in &generated.batches {
            if let Some(request) = &batch.traces {
                let converted = spans_from_request(request.clone());
                assert_eq!(converted.rejected, 0, "generated spans are valid");
                self.store.ingest_spans(converted.items);
            }
            if let Some(request) = &batch.logs {
                let converted = logs_from_request(request.clone());
                assert_eq!(converted.rejected, 0, "generated logs are valid");
                self.store.ingest_logs(converted.items);
            }
        }
    }

    /// Runs what is pending (a read and its result) without moving the clock.
    fn settle(&mut self) {
        self.cx.run_until_parked();
    }

    /// Moves the clock by `by` and runs what falls due.
    fn advance(&mut self, by: Duration) {
        self.cx.advance_clock(by);
        self.cx.run_until_parked();
    }

    /// One refresh period.
    fn tick(&mut self) {
        self.advance(Duration::from_millis(500));
    }

    /// Calls a method of the model.
    fn update<R>(&mut self, f: impl FnOnce(&mut HomeModel, &mut Context<HomeModel>) -> R) -> R {
        self.cx.update_entity(&self.home, f)
    }

    /// Reads the model.
    fn read<R>(&self, f: impl FnOnce(&HomeModel) -> R) -> R {
        self.cx.read_entity(&self.home, |home, _| f(home))
    }

    fn data(&self) -> HomeData {
        self.read(|home| home.data().clone())
    }

    /// The trace IDs of the table's rows, top to bottom.
    fn rows(&self) -> Vec<TraceId> {
        self.read(|home| home.data().rows.iter().map(|row| row.trace_id).collect())
    }

    fn set_query(&mut self, text: &str) {
        self.update(|home, cx| home.set_query(text, cx));
        self.settle();
    }
}

/// A one-span trace made by hand: `trace` is its ID (all bytes the same), the
/// root `name` of `service`, `duration_nanos` long from `start_nanos`.
fn lone_trace(
    trace: u8,
    service: &str,
    name: &str,
    start_nanos: u64,
    duration_nanos: u64,
    status: StatusCode,
) -> Span {
    Span {
        trace_id: TraceId([trace; 16]),
        span_id: SpanId([trace; 8]),
        parent_span_id: None,
        name: name.to_owned(),
        kind: SpanKind::Server,
        start_unix_nano: start_nanos,
        end_unix_nano: start_nanos + duration_nanos,
        status,
        status_message: String::new(),
        attributes: Vec::new(),
        events: Vec::new(),
        resource: Arc::new(Resource {
            attributes: vec![(
                "service.name".to_owned(),
                AttrValue::Str(service.to_owned()),
            )],
        }),
        scope_name: "test".to_owned(),
    }
}

fn tid(n: u8) -> TraceId {
    TraceId([n; 16])
}

#[test]
fn clear_all_empties_the_store_and_the_view_at_once_and_receiving_goes_on() {
    let mut f = Fixture::new(BASE + SECOND);
    f.ingest(&generate(&Scenario::CheckoutFailure, SEED, BASE));
    f.tick();
    assert_eq!(f.rows().len(), 1);
    assert!(!f.data().map.nodes.is_empty());
    f.update(|home, cx| home.set_errors_only(true, cx));
    f.update(|home, cx| home.set_range(TimeRange::LastHour, cx));
    f.settle();

    // Paused, so the timer would show nothing for a long time: the clear reads
    // by itself.
    f.update(|home, cx| home.set_paused(true, cx));
    f.update(|home, cx| home.clear_all(cx));
    f.settle();
    assert!(f.rows().is_empty(), "the table is empty at once");
    assert!(f.data().map.nodes.is_empty(), "so is the map");
    assert_eq!(
        f.store
            .search_traces(
                &TraceFilter::default(),
                &PageRequest {
                    limit: 10,
                    cursor: None
                }
            )
            .items
            .len(),
        0,
        "the store itself is empty, not only the view"
    );
    // What the user set stays: the filters, the range and the pause.
    assert!(f.read(|home| home.errors_only() && home.paused()));
    assert_eq!(f.read(|home| home.range()), TimeRange::LastHour);

    // Receiving carries on: after the pause the next trace shows.
    f.ingest(&generate(
        &Scenario::CheckoutFailure,
        SEED + 1,
        BASE + 2 * SECOND,
    ));
    f.update(|home, cx| home.set_paused(false, cx));
    f.settle();
    assert_eq!(f.rows().len(), 1);
}

// --- the time range ---------------------------------------------------------

#[test]
fn time_range_labels_defaults_and_cut_off_times() {
    assert_eq!(TimeRange::default(), TimeRange::Last15m);
    let labels: Vec<&str> = TimeRange::ALL.iter().map(|range| range.label()).collect();
    assert_eq!(
        labels,
        [
            "Last 5 minutes",
            "Last 15 minutes",
            "Last hour",
            "Everything"
        ]
    );

    let now = 10_000 * SECOND;
    assert_eq!(TimeRange::Last5m.since(now), Some(now - 5 * MINUTE));
    assert_eq!(TimeRange::Last15m.since(now), Some(now - 15 * MINUTE));
    assert_eq!(TimeRange::LastHour.since(now), Some(now - 60 * MINUTE));
    assert_eq!(TimeRange::Everything.since(now), None);
    // A clock that reads less than the range is long: from the start of time,
    // never an underflow.
    assert_eq!(TimeRange::LastHour.since(MINUTE), Some(0));
}

// --- refreshing -------------------------------------------------------------

#[test]
fn refresh_picks_up_new_traces() {
    let mut f = Fixture::new(BASE + SECOND);
    let first = f.data();
    assert!(first.rows.is_empty(), "an empty store has no rows");
    assert!(first.map.nodes.is_empty());
    assert!(!first.truncated);
    assert_eq!(first.query, QueryState::Empty);
    assert_eq!(first.refreshed_at, BASE + SECOND, "the first read is done");

    let failure = generate(&Scenario::CheckoutFailure, SEED, BASE);
    f.ingest(&failure);
    f.settle();
    assert!(f.rows().is_empty(), "nothing reads until the timer fires");

    // The timer is every 500 ms, not sooner.
    f.advance(Duration::from_millis(499));
    assert!(f.rows().is_empty(), "499 ms is not yet");
    f.clock.advance(Duration::from_secs(1));
    f.advance(Duration::from_millis(1));
    let data = f.data();
    assert_eq!(f.rows(), [failure.trace_ids[0]], "the new trace is listed");
    assert_eq!(
        data.refreshed_at,
        BASE + 2 * SECOND,
        "read at the store's now"
    );
    let names: Vec<&str> = data.map.nodes.iter().map(|node| node.id.as_str()).collect();
    assert!(names.contains(&"web-bff") && names.contains(&"payments-api"));

    // The next trace is newer, so it comes first.
    let success = generate(&Scenario::CheckoutSuccess, SEED, BASE + 5 * SECOND);
    f.ingest(&success);
    f.tick();
    assert_eq!(f.rows(), [success.trace_ids[0], failure.trace_ids[0]]);

    // Another period with nothing new: still the same rows.
    f.tick();
    assert_eq!(f.rows().len(), 2);
}

#[test]
fn refresh_now_reads_in_the_background() {
    let mut f = Fixture::new(BASE + SECOND);
    f.ingest(&generate(&Scenario::CheckoutFailure, SEED, BASE));

    f.update(|home, cx| home.refresh_now(cx));
    assert!(
        f.rows().is_empty(),
        "the call returns before the read is done: it runs on the background executor"
    );
    f.settle();
    assert_eq!(f.rows().len(), 1, "and its result is applied when it is");
}

#[test]
fn idle_refreshes_do_not_notify() {
    let mut f = Fixture::new(BASE + SECOND);
    f.ingest(&generate(&Scenario::CheckoutFailure, SEED, BASE));
    let notified = Rc::new(Cell::new(0));
    let count = Rc::clone(&notified);
    let home = f.home.clone();
    let _subscription =
        f.cx.update(move |cx| cx.observe(&home, move |_, _| count.set(count.get() + 1)));

    f.tick();
    assert_eq!(notified.get(), 1, "new data redraws the views once");

    // Time passes and nothing changes: the data is read again (and says when),
    // but a redraw would show the same pixels.
    let refreshed = f.data().refreshed_at;
    f.clock.advance(Duration::from_secs(1));
    f.tick();
    f.tick();
    assert_eq!(notified.get(), 1, "same data, no redraw");
    assert_eq!(f.data().refreshed_at, refreshed + SECOND, "but it was read");
}

#[test]
fn status_is_read_with_the_data() {
    let f = Fixture::new(BASE);
    assert_eq!(
        f.data().status,
        StatusModel::from_status(&listening_status())
    );
}

#[test]
fn table_and_map_colour_a_service_the_same() {
    let mut f = Fixture::new(BASE + SECOND);
    f.ingest(&generate(&Scenario::CheckoutFailure, SEED, BASE));
    f.tick();

    let data = f.data();
    let row = &data.rows[0];
    assert_eq!(
        row.services[0],
        ("web-bff".to_owned(), 0),
        "the entry point is 0"
    );
    for (service, index) in &row.services {
        let node = data
            .map
            .nodes
            .iter()
            .find(|node| node.kind == NodeKind::Service && &node.id == service)
            .unwrap_or_else(|| panic!("{service} is on the map"));
        assert_eq!(node.color, Some(*index), "{service} has one colour");
    }
    // The model hands out the colours it uses, for the views of other screens.
    let colors = f.read(|home| home.colors());
    assert_eq!(colors.borrow().peek("web-bff"), Some(0));
    assert_eq!(colors.borrow().peek("orders-api"), Some(1));
}

#[test]
fn the_map_keeps_its_places_across_refreshes() {
    let mut f = Fixture::new(BASE + SECOND);
    // Five services that call nobody: five nodes in the first column.
    f.ingest(&generate(&Scenario::HealthChecks { count: 1 }, SEED, BASE));
    f.tick();
    let before = f.data().map;
    assert_eq!(before.nodes.len(), 5);

    // The checkout joins them with calls and databases. A map laid out from
    // nothing would put orders-api in the second column; this one has already
    // drawn it and does not move it.
    f.ingest(&generate(&Scenario::CheckoutFailure, SEED, BASE + SECOND));
    f.tick();
    let after = f.data().map;
    assert!(after.nodes.len() > before.nodes.len(), "new nodes appeared");
    for node in &before.nodes {
        let now = after
            .nodes
            .iter()
            .find(|other| other.id == node.id && other.kind == node.kind)
            .expect("a node that was drawn is still drawn");
        assert_eq!((now.x, now.y), (node.x, node.y), "{} did not move", node.id);
    }
}

// --- pause ------------------------------------------------------------------

#[test]
fn paused_does_not_refresh() {
    let mut f = Fixture::new(BASE + SECOND);
    f.update(|home, cx| home.set_paused(true, cx));
    assert!(f.read(|home| home.paused()));
    let refreshed = f.data().refreshed_at;

    f.ingest(&generate(&Scenario::CheckoutFailure, SEED, BASE));
    for _ in 0..4 {
        f.tick();
    }
    assert!(f.rows().is_empty(), "two seconds on, the view is frozen");
    assert_eq!(f.data().refreshed_at, refreshed);

    // Resuming catches up at once, without waiting for the next period.
    f.update(|home, cx| home.set_paused(false, cx));
    f.settle();
    assert_eq!(f.rows().len(), 1);
    assert!(!f.read(|home| home.paused()));

    // And the timer runs again.
    f.ingest(&generate(
        &Scenario::CheckoutSuccess,
        SEED,
        BASE + 5 * SECOND,
    ));
    f.tick();
    assert_eq!(f.rows().len(), 2);
}

#[test]
fn changes_made_while_paused_still_apply() {
    let mut f = Fixture::new(BASE + SECOND);
    f.ingest(&generate(&Scenario::CheckoutFailure, SEED, BASE));
    f.ingest(&generate(
        &Scenario::CheckoutSuccess,
        SEED,
        BASE + 5 * SECOND,
    ));
    f.update(|home, cx| home.set_paused(true, cx));
    f.tick();
    assert!(f.rows().is_empty(), "frozen before the user does anything");

    // Pause stops the timer, not the user: a change they make reads the store.
    f.update(|home, cx| home.set_errors_only(true, cx));
    f.settle();
    assert_eq!(f.rows().len(), 1, "errors only, read although paused");
}

// --- the time range ---------------------------------------------------------

#[test]
fn range_filters_old_traces() {
    // A failure at the start; two checkouts 40 and 50 minutes on; it is 52
    // minutes in.
    let now = BASE + 52 * MINUTE;
    let mut f = Fixture::new(now);
    let failure = generate(&Scenario::CheckoutFailure, SEED, BASE);
    let earlier = generate(&Scenario::CheckoutSuccess, 8, BASE + 40 * MINUTE);
    let latest = generate(&Scenario::CheckoutSuccess, 9, BASE + 50 * MINUTE);
    for generated in [&failure, &earlier, &latest] {
        f.ingest(generated);
    }
    let (a, b, c) = (
        failure.trace_ids[0],
        earlier.trace_ids[0],
        latest.trace_ids[0],
    );

    f.tick();
    assert_eq!(
        f.read(|home| home.range()),
        TimeRange::Last15m,
        "the default"
    );
    assert_eq!(
        f.rows(),
        [c, b],
        "the last 15 minutes: the failure is 52 old"
    );
    assert!(
        f.data().map.nodes.iter().all(|node| !node.failing),
        "the map follows the range: the failure is not in it"
    );

    f.update(|home, cx| home.set_range(TimeRange::Last5m, cx));
    f.settle();
    assert_eq!(f.rows(), [c], "the last 5 minutes");

    f.update(|home, cx| home.set_range(TimeRange::LastHour, cx));
    f.settle();
    assert_eq!(f.rows(), [c, b, a], "the last hour");
    assert!(
        f.data()
            .map
            .nodes
            .iter()
            .any(|node| node.id == "payments-api" && node.failing),
        "and the failure is on the map"
    );

    f.update(|home, cx| home.set_range(TimeRange::Everything, cx));
    f.settle();
    assert_eq!(f.rows(), [c, b, a], "everything");
}

#[test]
fn range_keeps_a_trace_that_started_exactly_at_the_cut() {
    let mut f = Fixture::new(BASE + 5 * MINUTE);
    let failure = generate(&Scenario::CheckoutFailure, SEED, BASE);
    f.ingest(&failure);
    f.update(|home, cx| home.set_range(TimeRange::Last5m, cx));
    f.settle();
    assert_eq!(
        f.rows(),
        [failure.trace_ids[0]],
        "5 minutes ago to the nanosecond"
    );

    f.clock.advance(Duration::from_nanos(1));
    f.update(|home, cx| home.refresh_now(cx));
    f.settle();
    assert!(f.rows().is_empty(), "a nanosecond older is out");
}

#[test]
fn map_rates_are_counts_over_the_range() {
    // 100 health checks a service, one a second from BASE: 100 requests each.
    let mut f = Fixture::new(BASE + 100 * SECOND);
    f.ingest(&generate(
        &Scenario::HealthChecks { count: 100 },
        SEED,
        BASE,
    ));
    f.tick();
    let subtitle = |f: &Fixture| {
        f.data()
            .map
            .nodes
            .iter()
            .find(|node| node.id == "web-bff")
            .expect("web-bff is on the map")
            .subtitle
            .clone()
    };

    assert_eq!(
        subtitle(&f),
        "0.1 req/s \u{b7} 0% err",
        "100 over 15 minutes"
    );
    f.update(|home, cx| home.set_range(TimeRange::Last5m, cx));
    f.settle();
    assert_eq!(
        subtitle(&f),
        "0.3 req/s \u{b7} 0% err",
        "100 over 5 minutes"
    );
    f.update(|home, cx| home.set_range(TimeRange::LastHour, cx));
    f.settle();
    assert_eq!(subtitle(&f), "0.0 req/s \u{b7} 0% err", "100 over an hour");
    // Everything has no length of its own: the data covers 100 seconds, from
    // the first trace in the store to now.
    f.update(|home, cx| home.set_range(TimeRange::Everything, cx));
    f.settle();
    assert_eq!(
        subtitle(&f),
        "1.0 req/s \u{b7} 0% err",
        "100 in 100 seconds"
    );
}

// --- the query bar ----------------------------------------------------------

#[test]
fn set_query_validates_at_once() {
    let mut f = Fixture::new(BASE + SECOND);
    f.ingest(&generate(&Scenario::CheckoutFailure, SEED, BASE));
    f.ingest(&generate(
        &Scenario::CheckoutSuccess,
        SEED,
        BASE + 5 * SECOND,
    ));
    f.tick();
    assert_eq!(f.rows().len(), 2);

    // The state is there when the call returns; the rows follow with the read.
    f.update(|home, cx| home.set_query("{ status = error }", cx));
    assert!(matches!(f.data().query, QueryState::Valid(_)));
    assert_eq!(f.rows().len(), 2, "the rows wait for the read");
    f.settle();
    assert_eq!(f.rows().len(), 1, "only the failing checkout has an error");
    assert_eq!(
        f.read(|home| home.query_text().to_owned()),
        "{ status = error }"
    );

    f.set_query("   ");
    assert_eq!(f.data().query, QueryState::Empty, "blanks are no query");
    assert_eq!(f.rows().len(), 2);
}

#[test]
fn invalid_query_keeps_previous_rows() {
    let mut f = Fixture::new(BASE + SECOND);
    let failure = generate(&Scenario::CheckoutFailure, SEED, BASE);
    f.ingest(&failure);
    f.ingest(&generate(
        &Scenario::CheckoutSuccess,
        SEED,
        BASE + 5 * SECOND,
    ));
    f.tick();
    f.set_query("{ status = error }");
    assert_eq!(f.rows(), [failure.trace_ids[0]]);

    // Half a query: the error is known at once (at the end, column 12) and the
    // table is as it was, not empty.
    f.update(|home, cx| home.set_query("{ status = ", cx));
    match f.data().query {
        QueryState::Invalid {
            column,
            ref message,
        } => {
            assert_eq!(column, 12, "{message}");
            assert!(!message.is_empty());
        }
        other => panic!("expected an error, got {other:?}"),
    }
    assert_eq!(
        f.rows(),
        [failure.trace_ids[0]],
        "the previous rows, at once"
    );
    f.settle();
    assert_eq!(f.rows(), [failure.trace_ids[0]], "and after the read");
    assert_eq!(f.read(|home| home.query_text().to_owned()), "{ status = ");

    // The view stays live: a new failing trace is listed by the last good
    // query, while the error stays shown.
    let second = generate(&Scenario::CheckoutFailure, 9, BASE + 10 * SECOND);
    f.ingest(&second);
    f.tick();
    assert_eq!(f.rows(), [second.trace_ids[0], failure.trace_ids[0]]);
    assert!(matches!(f.data().query, QueryState::Invalid { .. }));

    // Fixing the text applies it; clearing it shows everything.
    f.set_query("{ status = error }");
    assert!(matches!(f.data().query, QueryState::Valid(_)));
    assert_eq!(f.rows().len(), 2);
    f.set_query("");
    assert_eq!(f.rows().len(), 3);
}

#[test]
fn select_service_writes_query() {
    let mut f = Fixture::new(BASE + SECOND);
    let failure = generate(&Scenario::CheckoutFailure, SEED, BASE);
    let success = generate(&Scenario::CheckoutSuccess, SEED, BASE + 5 * SECOND);
    f.ingest(&failure);
    f.ingest(&success);
    // One health check per service: only payments-api's has that service.
    f.ingest(&generate(
        &Scenario::HealthChecks { count: 1 },
        SEED,
        BASE + 6 * SECOND,
    ));
    f.tick();
    assert_eq!(f.rows().len(), 7);
    assert_eq!(
        f.read(|home| home.selected_service().map(str::to_owned)),
        None
    );

    f.update(|home, cx| home.select_service(Some("payments-api".to_owned()), cx));
    f.settle();
    assert_eq!(f.read(|home| home.query_text().to_owned()), PAYMENTS);
    assert_eq!(PAYMENTS, service_filter("payments-api"));
    assert!(matches!(f.data().query, QueryState::Valid(_)));
    assert_eq!(
        f.read(|home| home.selected_service().map(str::to_owned))
            .as_deref(),
        Some("payments-api")
    );
    let rows = f.rows();
    assert_eq!(
        rows.len(),
        3,
        "the two checkouts and payments-api's own probe"
    );
    assert!(rows.contains(&failure.trace_ids[0]) && rows.contains(&success.trace_ids[0]));

    // Another service replaces it.
    f.update(|home, cx| home.select_service(Some("orders-api".to_owned()), cx));
    f.settle();
    assert_eq!(
        f.read(|home| home.query_text().to_owned()),
        service_filter("orders-api")
    );
    assert_eq!(
        f.rows().len(),
        3,
        "the two checkouts and orders-api's probe"
    );

    // Clearing the selection clears the query and brings every trace back.
    f.update(|home, cx| home.select_service(None, cx));
    f.settle();
    assert_eq!(f.read(|home| home.query_text().to_owned()), "");
    assert_eq!(f.data().query, QueryState::Empty);
    assert_eq!(
        f.read(|home| home.selected_service().map(str::to_owned)),
        None
    );
    assert_eq!(f.rows().len(), 7);
}

#[test]
fn clearing_the_selection_leaves_a_query_the_user_typed() {
    let mut f = Fixture::new(BASE + SECOND);
    f.ingest(&generate(&Scenario::CheckoutFailure, SEED, BASE));
    f.ingest(&generate(
        &Scenario::CheckoutSuccess,
        SEED,
        BASE + 5 * SECOND,
    ));
    f.tick();

    // Click empty space with nothing selected: the typed query stays.
    f.set_query("{ status = error }");
    f.update(|home, cx| home.select_service(None, cx));
    f.settle();
    assert_eq!(
        f.read(|home| home.query_text().to_owned()),
        "{ status = error }"
    );
    assert_eq!(f.rows().len(), 1);

    // Typing over a service's query ends the selection; clearing it afterwards
    // does not wipe what was typed.
    f.update(|home, cx| home.select_service(Some("payments-api".to_owned()), cx));
    f.settle();
    assert!(f.read(|home| home.selected_service().is_some()));
    f.set_query("{ status = error }");
    assert_eq!(
        f.read(|home| home.selected_service().map(str::to_owned)),
        None
    );
    f.update(|home, cx| home.select_service(None, cx));
    f.settle();
    assert_eq!(
        f.read(|home| home.query_text().to_owned()),
        "{ status = error }"
    );
}

// --- the toggles ------------------------------------------------------------

#[test]
fn errors_only_and_slow_only_combine() {
    let mut f = Fixture::new(BASE + MINUTE);
    let failure = generate(&Scenario::CheckoutFailure, SEED, BASE);
    let success = generate(&Scenario::CheckoutSuccess, SEED, BASE + 5 * SECOND);
    f.ingest(&failure);
    f.ingest(&success);
    f.ingest(&generate(
        &Scenario::HealthChecks { count: 1 },
        SEED,
        BASE + 6 * SECOND,
    ));
    f.store.ingest_spans(vec![
        // Fast and failing, slow and fine (exactly 1 s), and a hair too fast.
        lone_trace(
            1,
            "web-bff",
            "GET /boom",
            BASE + 20 * SECOND,
            5 * MS,
            StatusCode::Error,
        ),
        lone_trace(
            2,
            "web-bff",
            "GET /report",
            BASE + 21 * SECOND,
            SECOND,
            StatusCode::Unset,
        ),
        lone_trace(
            3,
            "web-bff",
            "GET /almost",
            BASE + 22 * SECOND,
            SECOND - 1,
            StatusCode::Unset,
        ),
    ]);
    let (failed, ok) = (failure.trace_ids[0], success.trace_ids[0]);
    f.tick();
    assert_eq!(f.rows().len(), 2 + 5 + 3, "no toggle: every trace");

    f.update(|home, cx| home.set_errors_only(true, cx));
    f.settle();
    assert_eq!(f.rows(), [tid(1), failed], "errors only, newest first");
    assert!(f.read(|home| home.errors_only()));

    f.update(|home, cx| home.set_errors_only(false, cx));
    f.update(|home, cx| home.set_slow_only(true, cx));
    f.settle();
    assert_eq!(
        f.rows(),
        [tid(2), ok, failed],
        "slower than 1 s, 1 s included"
    );
    assert!(f.read(|home| home.slow_only()));

    f.update(|home, cx| home.set_errors_only(true, cx));
    f.settle();
    assert_eq!(f.rows(), [failed], "both: failing and slow");

    // With a valid query as well, all three must hold.
    f.update(|home, cx| home.set_slow_only(false, cx));
    f.set_query("{ resource.service.name = \"web-bff\" }");
    assert_eq!(f.rows(), [tid(1), failed], "errors only in web-bff");
    f.update(|home, cx| home.set_slow_only(true, cx));
    f.settle();
    assert_eq!(f.rows(), [failed], "slow, failing and in web-bff");
    f.set_query("{ resource.service.name = \"payments-api\" }");
    assert_eq!(f.rows(), [failed], "slow, failing and in payments-api");
    f.set_query("{ resource.service.name = \"no-such-service\" }");
    assert!(f.rows().is_empty(), "no trace matches");

    // Off again: the whole list.
    f.update(|home, cx| {
        home.set_errors_only(false, cx);
        home.set_slow_only(false, cx);
    });
    f.set_query("");
    assert_eq!(f.rows().len(), 10);
}

/// Reads run on the background executor, which can finish them in any order.
/// Only the latest one may be applied: the result of a read that a newer
/// setting replaced would put the old filter's rows under the new filter's
/// controls.
#[test]
fn a_replaced_read_is_dropped() {
    let mut f = Fixture::new(BASE + SECOND);
    f.ingest(&generate(&Scenario::CheckoutFailure, SEED, BASE));
    f.ingest(&generate(
        &Scenario::CheckoutSuccess,
        SEED,
        BASE + 5 * SECOND,
    ));

    // Two settings in one go: two reads are in flight, the first with errors
    // only, the second with everything. Many rounds, so that the order they
    // finish in is not always the order they started.
    for round in 0..20 {
        f.update(|home, cx| {
            home.set_errors_only(true, cx);
            home.set_errors_only(false, cx);
        });
        f.settle();
        assert_eq!(f.rows().len(), 2, "round {round}: the latest setting wins");
        f.update(|home, cx| {
            home.set_slow_only(true, cx);
            home.set_errors_only(true, cx);
            home.set_errors_only(false, cx);
            home.set_slow_only(false, cx);
        });
        f.settle();
        assert_eq!(
            f.rows().len(),
            2,
            "round {round}: four reads, the last wins"
        );
    }
}

// --- the limit --------------------------------------------------------------

#[test]
fn truncation_flag_over_1000() {
    assert_eq!(HOME_MAX_ROWS, 1_000);
    // 200 probes for each of the 5 services: exactly 1,000 traces, one every
    // second per service.
    let now = BASE + 300 * SECOND;
    let mut f = Fixture::new(now);
    let probes = generate(&Scenario::HealthChecks { count: 200 }, SEED, BASE);
    assert_eq!(probes.trace_ids.len(), 1_000);
    f.ingest(&probes);
    f.tick();
    let data = f.data();
    assert_eq!(data.rows.len(), 1_000, "all of them fit");
    assert!(!data.truncated, "exactly the limit is not more than it");

    // One more, newer than the rest: the oldest probe falls off.
    f.store.ingest_spans(vec![lone_trace(
        1,
        "web-bff",
        "GET /latest",
        BASE + 250 * SECOND,
        MS,
        StatusCode::Unset,
    )]);
    f.tick();
    let data = f.data();
    assert_eq!(data.rows.len(), 1_000, "at most 1,000 rows");
    assert!(data.truncated, "and the table says there are more");
    assert_eq!(data.rows[0].trace_id, tid(1), "the newest is first");
    let rows = f.rows();
    assert!(
        !rows.contains(&probes.trace_ids[0]),
        "the oldest trace is the one left out"
    );

    // A filter that matches fewer than the limit clears the flag.
    f.update(|home, cx| home.set_errors_only(true, cx));
    f.settle();
    let data = f.data();
    assert!(data.rows.is_empty() && !data.truncated);
}

// --- Clear filters (the trace table's empty state) -----------------------------

#[test]
fn has_filters_and_clear_filters() {
    let mut f = Fixture::new(BASE + MINUTE);
    f.ingest(&generate(&Scenario::CheckoutFailure, SEED, BASE));
    f.ingest(&generate(
        &Scenario::CheckoutSuccess,
        SEED,
        BASE + 5 * SECOND,
    ));
    f.tick();
    assert!(!f.read(|home| home.has_filters()), "nothing set");
    assert_eq!(f.rows().len(), 2);

    // Each of the three things the user can narrow the table with counts.
    f.update(|home, cx| home.set_errors_only(true, cx));
    assert!(f.read(|home| home.has_filters()), "errors only");
    f.update(|home, cx| home.set_errors_only(false, cx));
    assert!(!f.read(|home| home.has_filters()));
    f.update(|home, cx| home.set_slow_only(true, cx));
    assert!(f.read(|home| home.has_filters()), "slower than 1 s");
    f.update(|home, cx| home.set_slow_only(false, cx));
    f.set_query("{ status = error }");
    assert!(f.read(|home| home.has_filters()), "a query");
    // A text that does not parse is no filter: the rows are the last good
    // query's, which here is the same one.
    f.set_query("{ status = ");
    assert!(
        f.read(|home| home.has_filters()),
        "the last good query still filters"
    );
    f.set_query("   ");
    assert!(!f.read(|home| home.has_filters()), "blanks are no query");

    // All three at once, and nothing matches.
    f.update(|home, cx| {
        home.set_errors_only(true, cx);
        home.set_slow_only(true, cx);
    });
    f.set_query("{ resource.service.name = \"no-such-service\" }");
    f.settle();
    assert!(f.rows().is_empty(), "nothing matches");
    assert!(f.read(|home| home.has_filters()));

    // Clear filters: the query, both toggles, in one read.
    f.update(|home, cx| home.clear_filters(cx));
    f.settle();
    assert!(!f.read(|home| home.has_filters()));
    assert!(!f.read(|home| home.errors_only() || home.slow_only()));
    assert_eq!(f.read(|home| home.query_text().to_owned()), "");
    assert_eq!(f.data().query, QueryState::Empty);
    assert_eq!(f.rows().len(), 2, "the whole list is back");
}

#[test]
fn clear_filters_keeps_the_range_and_the_pause_and_ends_a_selection() {
    let mut f = Fixture::new(BASE + MINUTE);
    f.ingest(&generate(&Scenario::CheckoutFailure, SEED, BASE));
    f.tick();
    f.update(|home, cx| {
        home.set_range(TimeRange::LastHour, cx);
        home.select_service(Some("payments-api".to_owned()), cx);
    });
    f.settle();
    assert_eq!(
        f.read(|home| home.selected_service().map(str::to_owned)),
        Some("payments-api".to_owned())
    );

    f.update(|home, cx| home.clear_filters(cx));
    f.settle();
    assert_eq!(
        f.read(|home| home.range()),
        TimeRange::LastHour,
        "not a filter"
    );
    assert_eq!(
        f.read(|home| home.selected_service().map(str::to_owned)),
        None
    );
    assert_eq!(f.read(|home| home.query_text().to_owned()), "");
}

#[test]
fn loaded_says_the_first_read_is_in() {
    // An empty store: the data is empty before the read and after it, and only
    // `loaded` tells the two apart.
    let clock = Arc::new(ManualClock::new(BASE));
    let backend = Rc::new(FixedBackend::new(Store::new(StoreConfig::default(), clock)));
    let mut cx = headless_app();
    let home = cx.new(|cx| HomeModel::new(backend, cx));
    assert!(!cx.read_entity(&home, |home, _| home.loaded()), "not yet");
    cx.run_until_parked();
    assert!(cx.read_entity(&home, |home, _| home.loaded()), "read");
    assert!(cx.read_entity(&home, |home, _| home.data().rows.is_empty()));
}

// --- the open trace (Task 8) ------------------------------------------------------------

/// The trace screen on its own, as a window's root view. The views read the
/// theme from a global that the main window sets up; here the test does.
fn trace_screen(
    backend: Rc<FixedBackend>,
    id: loctel_model::TraceId,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> Entity<TraceView> {
    loctel_ui::app::kit_theme::apply(&Theme::dark(), cx);
    cx.new(|cx| TraceView::new(backend, id, window, cx))
}

/// A trace screen over a store the test fills batch by batch, in a window
/// whose clock the test moves.
struct OpenTrace {
    driver: Driver<TraceView>,
    store: Store,
    clock: Arc<ManualClock>,
    generated: Generated,
    /// The batches ingested so far.
    sent: usize,
}

impl OpenTrace {
    /// A store holding the first `batches` of `scenario` (its clock reads
    /// `BASE`), and the trace screen of the scenario's trace over it.
    fn new(scenario: &Scenario, batches: usize) -> OpenTrace {
        let clock = Arc::new(ManualClock::new(BASE));
        let store = Store::new(StoreConfig::default(), clock.clone());
        let generated = generate(scenario, SEED, BASE);
        let id = generated.trace_ids[0];
        let mut open = OpenTrace {
            // Made over an empty store, then filled: see below.
            driver: Driver::new((1440, 900), &Theme::dark(), {
                let backend = Rc::new(FixedBackend::new(store.clone()));
                move |window, cx| trace_screen(backend, id, window, cx)
            }),
            store,
            clock,
            generated,
            sent: 0,
        };
        // The screen was opened over an empty store (its first read found the
        // trace missing); the batches arrive and the next refresh has them.
        open.send(batches);
        open.advance(OPEN_REFRESH);
        open
    }

    /// Ingests the next `n` batches.
    fn send(&mut self, n: usize) {
        for batch in &self.generated.batches[self.sent..self.sent + n] {
            let request = batch.traces.clone().expect("a batch of spans");
            self.store.ingest_spans(spans_from_request(request).items);
        }
        self.sent += n;
    }

    fn model(&self) -> Entity<TraceModel> {
        self.driver.read_root(|view| view.model().clone())
    }

    /// Moves the window's clock (the timers of the model) by `by`.
    fn advance(&mut self, by: Duration) {
        self.driver.advance(by);
    }

    /// The names of the rows, top to bottom, the placeholder included.
    fn rows(&self) -> Vec<(u16, String)> {
        let model = self.model();
        self.driver.read(|cx| {
            model
                .read(cx)
                .waterfall()
                .map(|waterfall| {
                    waterfall
                        .rows()
                        .iter()
                        .map(|row| (row.depth, row.name.clone()))
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// How many rows are spans (not the `missing parent` placeholder).
    fn real_rows(&self) -> usize {
        self.rows()
            .iter()
            .filter(|(_, name)| name != MISSING_PARENT)
            .count()
    }

    fn has_placeholder(&self) -> bool {
        self.rows().iter().any(|(_, name)| name == MISSING_PARENT)
    }

    fn read<R>(&self, f: impl FnOnce(&TraceModel) -> R) -> R {
        let model = self.model();
        self.driver.read(|cx| f(model.read(cx)))
    }

    /// The offset text of the selected span, which names it.
    fn selected_span(&self) -> Option<(String, String)> {
        self.read(|model| {
            model
                .details()
                .map(|details| (details.name.clone(), details.offset.clone()))
        })
    }
}

#[test]
fn open_trace_fills_in_as_spans_arrive() {
    // LateOutOfOrder: the ten spans at depth 3 and deeper first, the three
    // middle ones after 50 ms, the root last.
    let mut open = OpenTrace::new(&Scenario::LateOutOfOrder, 1);
    assert_eq!(open.generated.batches.len(), 3);
    open.driver.draw();
    open.driver.draw();

    // The first batch: ten spans, hanging under the placeholder.
    assert_eq!(open.read(|model| model.state()), LoadState::Loaded);
    assert_eq!(open.real_rows(), 10);
    assert!(
        open.has_placeholder(),
        "the parent of these is not here yet"
    );
    assert_eq!(open.rows()[0].1, MISSING_PARENT);
    assert!(!open.read(|model| model.complete()));

    // The user selects a span they are looking at.
    let id = open.read(|model| {
        let waterfall = model.waterfall().expect("loaded");
        waterfall
            .rows()
            .iter()
            .find(|row| row.name == "GET /stock/{sku}")
            .expect("an inventory span")
            .span_id
    });
    open.driver.update_root(|view, _, cx| {
        view.model().update(cx, |model, cx| model.select(id, cx));
    });
    open.driver.draw();
    let chosen = open.selected_span().expect("selected");
    assert_eq!(chosen.0, "GET /stock/{sku}");

    // 50 ms later the three middle spans arrive. The next refresh (500 ms)
    // has 13 rows, the placeholder is still there and the selection has not
    // moved.
    open.clock.advance(Duration::from_millis(50));
    open.send(1);
    open.advance(OPEN_REFRESH);
    assert_eq!(open.real_rows(), 13, "10 then 13");
    assert!(open.has_placeholder(), "the root has not come");
    assert_eq!(open.read(|model| model.selected()), Some(id), "kept");
    assert_eq!(
        open.selected_span().map(|(name, _)| name).as_deref(),
        Some("GET /stock/{sku}")
    );

    // The root, last: 14 rows, no placeholder, and the tree is rooted at it.
    open.clock.advance(Duration::from_millis(50));
    open.send(1);
    open.advance(OPEN_REFRESH);
    assert_eq!(open.real_rows(), 14, "13 then 14");
    assert!(!open.has_placeholder(), "the root arrived");
    assert_eq!(open.rows()[0], (0, "POST /api/checkout".to_owned()));
    assert_eq!(
        open.rows().iter().filter(|(depth, _)| *depth == 0).count(),
        1,
        "one root"
    );
    assert_eq!(
        open.read(|model| model.selected()),
        Some(id),
        "kept throughout"
    );
    let header = open.read(|model| model.header().cloned()).expect("loaded");
    assert_eq!(header.root, "POST /api/checkout");
    assert!(header.root_arrived);
    // The span's offset is now counted from the root, which started first.
    assert_eq!(
        open.selected_span().expect("selected").0,
        "GET /stock/{sku}"
    );
    assert_ne!(
        open.selected_span().expect("selected").1,
        chosen.1,
        "the trace's start moved earlier with the root, and the offset with it"
    );
}

#[test]
fn the_selection_made_before_the_root_survives_a_folded_ancestor() {
    // What the user folded before the rest arrived stays folded after.
    let mut open = OpenTrace::new(&Scenario::LateOutOfOrder, 1);
    open.driver.draw();
    open.driver.draw();
    let placeholder = open.read(|model| {
        model
            .waterfall()
            .expect("loaded")
            .rows()
            .iter()
            .find(|row| row.is_placeholder())
            .expect("a placeholder")
            .span_id
    });
    open.driver.update_root(|view, _, cx| {
        view.model()
            .update(cx, |model, cx| model.toggle(placeholder, cx));
    });
    open.driver.draw();
    assert_eq!(
        open.rows().len(),
        1,
        "everything folded into the placeholder"
    );

    open.clock.advance(Duration::from_millis(50));
    open.send(2);
    open.advance(OPEN_REFRESH);
    // The root arrived: the placeholder is gone, and its folding with it.
    assert!(!open.has_placeholder());
    assert_eq!(open.real_rows(), 14);
}

#[test]
fn the_header_says_receiving_until_the_trace_is_complete() {
    let mut open = OpenTrace::new(&Scenario::LateOutOfOrder, 1);
    open.driver.draw();
    open.driver.draw();
    assert!(!open.read(|model| model.header().unwrap().complete));
    assert_eq!(
        open.read(|model| model.header().unwrap().completeness()),
        "Receiving spans\u{2026}"
    );

    // Quiet for the store's period: the next refresh calls it complete.
    open.clock.advance(Duration::from_secs(60));
    open.advance(OPEN_REFRESH);
    assert!(open.read(|model| model.complete()));
    assert_eq!(
        open.read(|model| model.header().unwrap().completeness()),
        "Complete"
    );
}

#[test]
fn an_open_trace_is_read_every_500_ms_and_a_complete_one_every_2_s() {
    assert_eq!(OPEN_REFRESH, Duration::from_millis(500));
    assert_eq!(DONE_REFRESH, Duration::from_secs(2));

    let mut open = OpenTrace::new(&Scenario::LateOutOfOrder, 1);
    open.driver.draw();
    open.driver.draw();
    assert_eq!(open.read(|model| model.refresh_interval()), OPEN_REFRESH);

    // While arriving, a new span is picked up within 500 ms.
    open.send(1);
    open.advance(Duration::from_millis(400));
    assert_eq!(open.real_rows(), 10, "not yet");
    open.advance(Duration::from_millis(150));
    assert_eq!(open.real_rows(), 13, "after 550 ms");

    // Complete: from then on a read takes 2 s.
    open.clock.advance(Duration::from_secs(60));
    open.advance(OPEN_REFRESH);
    assert!(open.read(|model| model.complete()));
    assert_eq!(open.read(|model| model.refresh_interval()), DONE_REFRESH);

    open.send(1);
    open.advance(Duration::from_millis(1_000));
    assert_eq!(
        open.real_rows(),
        13,
        "a complete trace is not read every 500 ms"
    );
    open.advance(Duration::from_millis(1_500));
    assert_eq!(open.real_rows(), 14, "but within 2 s");
}

#[test]
fn a_trace_the_store_does_not_have_is_missing_and_one_that_arrives_loads() {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    let generated = generate(&Scenario::CheckoutSuccess, SEED, BASE);
    let id = generated.trace_ids[0];
    let backend = Rc::new(FixedBackend::new(store.clone()));
    let mut driver = Driver::new((1440, 900), &Theme::dark(), move |window, cx| {
        trace_screen(backend, id, window, cx)
    });
    driver.draw();
    driver.draw();
    let model = driver.read_root(|view| view.model().clone());
    assert_eq!(
        driver.read(|cx| model.read(cx).state()),
        LoadState::Missing,
        "nothing received yet"
    );

    // The trace turns up: the next refresh has it.
    for batch in &generated.batches {
        store.ingest_spans(spans_from_request(batch.traces.clone().unwrap()).items);
    }
    driver.advance(OPEN_REFRESH);
    assert_eq!(driver.read(|cx| model.read(cx).state()), LoadState::Loaded);
    assert_eq!(
        driver.read(|cx| model.read(cx).waterfall().unwrap().rows().len()),
        14
    );
}

#[test]
fn logs_that_arrive_for_the_selected_span_show_up() {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    let generated = generate(&Scenario::CheckoutFailure, SEED, BASE);
    let id = generated.trace_ids[0];
    // The spans first, the logs a moment later.
    for batch in &generated.batches {
        store.ingest_spans(spans_from_request(batch.traces.clone().unwrap()).items);
    }
    let backend = Rc::new(FixedBackend::new(store.clone()));
    let mut driver = Driver::new((1440, 900), &Theme::dark(), move |window, cx| {
        trace_screen(backend, id, window, cx)
    });
    driver.draw();
    driver.draw();
    let model = driver.read_root(|view| view.model().clone());
    // Select `POST /charges`, which logs four lines.
    let charges = driver.read(|cx| {
        model
            .read(cx)
            .waterfall()
            .unwrap()
            .rows()
            .iter()
            .find(|row| row.name == "POST /charges")
            .unwrap()
            .span_id
    });
    driver.update_root(|view, _, cx| {
        view.model()
            .update(cx, |model, cx| model.select(charges, cx));
    });
    driver.draw();
    assert!(driver.read(|cx| model.read(cx).logs().is_empty()));
    assert!(driver.read(|cx| model.read(cx).logs_loaded()));

    for batch in &generated.batches {
        if let Some(request) = &batch.logs {
            store.ingest_logs(logs_from_request(request.clone()).items);
        }
    }
    driver.advance(OPEN_REFRESH);
    assert_eq!(driver.read(|cx| model.read(cx).logs().len()), 4);
}
