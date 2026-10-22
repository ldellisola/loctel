//! The home screen's live data (M3 plan, Task 5): `HomeModel` keeps the map and
//! the trace table's rows fresh from the store every 500 ms, behind the time
//! range, the toggles and the query bar.
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
use loctel_store::{ManualClock, NodeKind, Store, StoreConfig};
use loctel_ui::StatusModel;
use loctel_ui::app::{HOME_MAX_ROWS, HomeData, HomeModel, TimeRange};
use loctel_ui::model::query_bar::{QueryState, service_filter};
use loctel_ui::testing::{FixedBackend, headless_app, listening_status};

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
