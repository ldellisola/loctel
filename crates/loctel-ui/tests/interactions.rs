//! The home screen's interactions: the service map's (M3 plan, Task 6) and the
//! trace table's (Task 7).
//!
//! The map: clicking a service writes its filter into the query bar, a drag
//! pans, ctrl or cmd plus the wheel and a pinch zoom (0.5 to 2.0), and `Fit`
//! puts the map back. The table: typing a query validates live and keeps the
//! last good rows when it does not parse, Enter and `Run` read again, the two
//! toggles narrow the rows, a click on a row opens its trace and the empty
//! table offers `Clear filters`.
//!
//! They drive the real window (`testing::Driver`) over the home screen's data
//! (`common::home_store`: the failing and the successful checkout and a health
//! check of each service), so a click goes through GPUI's own hit testing, the
//! map's painted frame and the `HomeModel`. The places to click come from the
//! map itself (`MapView::node_center`: the layout's node, through the camera
//! of the latest frame).

mod common;

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AppContext as _, Entity, Modifiers, Pixels, Point, point, px};
use loctel_model::{AttrValue, Resource, Span, SpanId, SpanKind, StatusCode, TraceId};
use loctel_store::Store;
use loctel_ui::Theme;
use loctel_ui::app::{HomeModel, MainWindow, Screen};
use loctel_ui::model::query_bar::{QueryState, service_filter};
use loctel_ui::testing::{Driver, FixedBackend, assert_snapshot};
use loctel_ui::views::{
    CLEAR_FILTERS, Camera, EMPTY_NO_DATA, EMPTY_NO_MATCH, MAX_ZOOM, MIN_ZOOM, MapView, TablePart,
    TraceTableView,
};

use common::{WINDOW, home_store};

const PAYMENTS: &str = "payments-api";

/// The main window over the home data, with its first frame drawn.
fn home_driver(theme: &Theme) -> Driver<MainWindow> {
    let backend = Rc::new(FixedBackend::new(home_store()));
    Driver::new(WINDOW, theme, move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    })
}

/// Like [`home_driver`], and the store the window reads, for a test that adds
/// traces while it runs.
fn home_driver_with_store(theme: &Theme) -> (Driver<MainWindow>, Store) {
    let store = home_store();
    let backend = Rc::new(FixedBackend::new(store.clone()));
    let driver = Driver::new(WINDOW, theme, move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    (driver, store)
}

fn map_of(driver: &Driver<MainWindow>) -> Entity<MapView> {
    driver.read_root(|main| main.map().clone())
}

fn home_of(driver: &Driver<MainWindow>) -> Entity<HomeModel> {
    driver.read_root(|main| main.home().clone())
}

/// Where the node of `id` is in the window now.
fn node_center(driver: &Driver<MainWindow>, id: &str) -> Point<Pixels> {
    let map = map_of(driver);
    driver
        .read(|cx| map.read(cx).node_center(id, cx))
        .unwrap_or_else(|| panic!("the map has no node `{id}`"))
}

fn camera(driver: &Driver<MainWindow>) -> Camera {
    let map = map_of(driver);
    driver.read(|cx| map.read(cx).camera())
}

fn fitted(driver: &Driver<MainWindow>) -> bool {
    let map = map_of(driver);
    driver.read(|cx| map.read(cx).is_fitted())
}

fn query_text(driver: &Driver<MainWindow>) -> String {
    let home = home_of(driver);
    driver.read(|cx| home.read(cx).query_text().to_owned())
}

fn selected(driver: &Driver<MainWindow>) -> Option<String> {
    let home = home_of(driver);
    driver.read(|cx| home.read(cx).selected_service().map(str::to_owned))
}

/// A point of the map with no node under it: below the first column, which has
/// the one node `web-bff`.
fn empty_space(driver: &Driver<MainWindow>) -> Point<Pixels> {
    node_center(driver, "web-bff") + point(px(0.0), px(160.0))
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

fn close_points(a: Point<Pixels>, b: Point<Pixels>) -> bool {
    close(f32::from(a.x), f32::from(b.x)) && close(f32::from(a.y), f32::from(b.y))
}

// --- clicking ------------------------------------------------------------------

#[test]
fn click_service_filters() {
    let mut driver = home_driver(&Theme::dark());
    assert_eq!(query_text(&driver), "");

    driver.click(node_center(&driver, PAYMENTS));

    assert_eq!(
        query_text(&driver),
        "{ resource.service.name = \"payments-api\" }"
    );
    assert_eq!(query_text(&driver), service_filter(PAYMENTS));
    assert_eq!(selected(&driver).as_deref(), Some(PAYMENTS));
    // The model took the text as a valid query and read the table with it.
    let home = home_of(&driver);
    let rows = driver.read(|cx| home.read(cx).data().rows.len());
    assert_eq!(rows, 3, "the two checkouts and the health check");

    // The selected node has its ring.
    assert_snapshot("home_payments_selected_dark", &driver.capture());
}

#[test]
fn clicking_the_selected_service_clears_it() {
    let mut driver = home_driver(&Theme::dark());
    driver.click(node_center(&driver, PAYMENTS));
    assert_eq!(selected(&driver).as_deref(), Some(PAYMENTS));

    driver.click(node_center(&driver, PAYMENTS));
    assert_eq!(selected(&driver), None);
    assert_eq!(query_text(&driver), "");
}

#[test]
fn clicking_another_service_moves_the_selection() {
    let mut driver = home_driver(&Theme::dark());
    driver.click(node_center(&driver, PAYMENTS));
    driver.click(node_center(&driver, "orders-api"));
    assert_eq!(selected(&driver).as_deref(), Some("orders-api"));
    assert_eq!(query_text(&driver), service_filter("orders-api"));
}

#[test]
fn clicking_empty_space_clears_the_selection() {
    let mut driver = home_driver(&Theme::dark());
    driver.click(node_center(&driver, PAYMENTS));
    assert_eq!(selected(&driver).as_deref(), Some(PAYMENTS));

    driver.click(empty_space(&driver));
    assert_eq!(selected(&driver), None);
    assert_eq!(query_text(&driver), "");
}

#[test]
fn clicking_empty_space_keeps_a_typed_query() {
    let mut driver = home_driver(&Theme::dark());
    let home = home_of(&driver);
    driver.update_root(|_, _, cx| {
        home.update(cx, |home, cx| home.set_query("{ status = error }", cx));
    });
    driver.draw();

    driver.click(empty_space(&driver));
    assert_eq!(query_text(&driver), "{ status = error }");
}

#[test]
fn clicking_a_database_does_nothing() {
    let mut driver = home_driver(&Theme::dark());
    driver.click(node_center(&driver, PAYMENTS));

    // Only a service has a filter the table can follow: a click on the
    // database neither selects it nor clears the service that is selected.
    driver.click(node_center(&driver, "postgresql"));
    assert_eq!(selected(&driver).as_deref(), Some(PAYMENTS));
    assert_eq!(query_text(&driver), service_filter(PAYMENTS));
}

// --- panning and fitting ---------------------------------------------------------

#[test]
fn drag_pans_and_fit_resets() {
    let mut driver = home_driver(&Theme::dark());
    let start = camera(&driver);
    assert!(fitted(&driver), "a new map is fitted");
    assert!(close(start.zoom, 1.0), "the checkout fits at full size");
    let web = node_center(&driver, "web-bff");

    let from = empty_space(&driver);
    driver.drag(from, from + point(px(60.0), px(40.0)));

    let panned = camera(&driver);
    assert!(!fitted(&driver), "a drag leaves the fit");
    assert!(close(panned.zoom, start.zoom), "a drag does not zoom");
    assert!(close(panned.pan_x, start.pan_x + 60.0), "{panned:?}");
    assert!(close(panned.pan_y, start.pan_y + 40.0), "{panned:?}");
    assert!(
        close_points(
            node_center(&driver, "web-bff"),
            web + point(px(60.0), px(40.0))
        ),
        "the nodes move with the drag"
    );
    assert_eq!(selected(&driver), None, "a drag is not a click");

    // The Fit button puts the map back.
    let button = {
        let map = map_of(&driver);
        driver
            .read(|cx| map.read(cx).fit_button_center())
            .expect("the map has a Fit button")
    };
    driver.click(button);
    assert!(fitted(&driver));
    assert_eq!(camera(&driver), start);
    assert!(close_points(node_center(&driver, "web-bff"), web));
}

#[test]
fn a_drag_that_starts_on_a_node_pans_and_does_not_select() {
    let mut driver = home_driver(&Theme::dark());
    let from = node_center(&driver, PAYMENTS);
    driver.drag(from, from + point(px(30.0), px(-20.0)));

    assert_eq!(selected(&driver), None);
    assert_eq!(query_text(&driver), "");
    assert!(!fitted(&driver));
}

#[test]
fn a_wiggle_under_the_slop_is_still_a_click() {
    let mut driver = home_driver(&Theme::dark());
    let from = node_center(&driver, PAYMENTS);
    driver.drag(from, from + point(px(1.0), px(1.0)));

    assert_eq!(selected(&driver).as_deref(), Some(PAYMENTS));
    assert!(fitted(&driver), "a click does not leave the fit");
}

#[test]
fn plain_scroll_pans_like_a_two_finger_drag() {
    let mut driver = home_driver(&Theme::dark());
    let start = camera(&driver);
    let at = empty_space(&driver);

    // Positive is "later content comes into view": the picture moves up.
    driver.scroll(at, 30.0);
    let moved = camera(&driver);
    assert!(close(moved.pan_y, start.pan_y - 30.0), "{moved:?}");
    assert!(close(moved.pan_x, start.pan_x));
    assert!(close(moved.zoom, start.zoom));
    assert!(!fitted(&driver));
}

// --- zooming ---------------------------------------------------------------------

#[test]
fn ctrl_scroll_zooms_around_the_cursor_within_limits() {
    let mut driver = home_driver(&Theme::dark());
    let web = node_center(&driver, "web-bff");

    // The wheel turned up (a negative `dy` of the driver) zooms in, and the
    // point under the cursor stays under it.
    driver.scroll_with(web, -100.0, Modifiers::control());
    let zoomed = camera(&driver);
    assert!(zoomed.zoom > 1.0 && zoomed.zoom < MAX_ZOOM, "{zoomed:?}");
    assert!(!fitted(&driver));
    assert!(
        close_points(node_center(&driver, "web-bff"), web),
        "the pivot stays put"
    );

    for _ in 0..10 {
        driver.scroll_with(web, -100.0, Modifiers::control());
    }
    assert_eq!(camera(&driver).zoom, MAX_ZOOM, "no further than 2.0");

    for _ in 0..20 {
        driver.scroll_with(web, 100.0, Modifiers::control());
    }
    assert_eq!(camera(&driver).zoom, MIN_ZOOM, "no further than 0.5");
}

#[test]
fn cmd_scroll_zooms_too() {
    let mut driver = home_driver(&Theme::dark());
    let at = node_center(&driver, "orders-api");
    driver.scroll_with(at, -100.0, Modifiers::command());
    assert!(camera(&driver).zoom > 1.0);
}

#[test]
fn pinch_zooms_within_limits() {
    let mut driver = home_driver(&Theme::dark());
    let at = node_center(&driver, "orders-api");

    driver.pinch(at, 0.25);
    let zoomed = camera(&driver);
    assert!(close(zoomed.zoom, 1.25), "{zoomed:?}");
    assert!(close_points(node_center(&driver, "orders-api"), at));

    driver.pinch(at, 5.0);
    assert_eq!(camera(&driver).zoom, MAX_ZOOM);
    driver.pinch(at, -0.99);
    assert_eq!(camera(&driver).zoom, MIN_ZOOM);
}

// --- fitting to the window -------------------------------------------------------

#[test]
fn a_fitted_map_follows_the_window() {
    let mut driver = home_driver(&Theme::dark());
    assert!(close(camera(&driver).zoom, 1.0));

    // 1000 by 640 is the smallest window: the map, 1,072 px wide, shrinks to
    // fit, not below 0.5.
    driver.resize((1000, 640));
    let small = camera(&driver);
    assert!(fitted(&driver), "a resize keeps a fitted map fitted");
    assert!(small.zoom < 1.0 && small.zoom >= MIN_ZOOM, "{small:?}");
    let (bounds, width, height) = {
        let map = map_of(&driver);
        let home = home_of(&driver);
        driver.read(|cx| {
            let layout = &home.read(cx).data().map;
            (map.read(cx).view_bounds(), layout.width, layout.height)
        })
    };
    assert!(small.zoom * width <= f32::from(bounds.size.width) + 0.5);
    assert!(small.zoom * height <= f32::from(bounds.size.height) + 0.5);

    driver.resize(WINDOW);
    assert!(close(camera(&driver).zoom, 1.0), "and back to full size");
}

#[test]
fn a_map_the_user_moved_keeps_its_place_in_a_resize() {
    let mut driver = home_driver(&Theme::dark());
    let from = empty_space(&driver);
    driver.drag(from, from + point(px(25.0), px(0.0)));
    let before = camera(&driver);

    driver.resize((1200, 800));
    assert_eq!(camera(&driver), before);
    assert!(!fitted(&driver));
}

// --- the camera ------------------------------------------------------------------

#[test]
fn fit_never_enlarges_and_never_goes_below_the_minimum() {
    // The picture is smaller than the view: full size, from the corner.
    let fit = Camera::fit((1190.0, 411.0), (800.0, 300.0));
    assert_eq!(fit, Camera::default());
    assert_eq!(fit.zoom, 1.0);

    // Wider than the view: scaled to the width.
    let fit = Camera::fit((600.0, 800.0), (1200.0, 400.0));
    assert!(close(fit.zoom, 0.5));
    // Taller than the view: scaled to the height.
    let fit = Camera::fit((1000.0, 300.0), (1000.0, 400.0));
    assert!(close(fit.zoom, 0.75));
    // Far too big: the floor.
    let fit = Camera::fit((100.0, 100.0), (1000.0, 1000.0));
    assert_eq!(fit.zoom, MIN_ZOOM);
    // Nothing to draw: nothing to fit.
    assert_eq!(Camera::fit((0.0, 0.0), (0.0, 0.0)), Camera::default());
    assert_eq!(Camera::fit((500.0, 500.0), (0.0, 0.0)), Camera::default());
}

#[test]
fn the_camera_maps_both_ways() {
    let camera = Camera {
        zoom: 1.5,
        pan_x: 20.0,
        pan_y: -10.0,
    };
    assert_eq!(camera.to_view(100.0, 40.0), (170.0, 50.0));
    assert_eq!(camera.to_picture(170.0, 50.0), (100.0, 40.0));
}

#[test]
fn zooming_keeps_the_point_under_the_pivot() {
    let camera = Camera {
        zoom: 1.0,
        pan_x: 12.0,
        pan_y: 7.0,
    };
    let pivot = (300.0, 200.0);
    let under = camera.to_picture(pivot.0, pivot.1);

    let zoomed = camera.zoomed(1.6, pivot);
    assert!(close(zoomed.zoom, 1.6));
    let after = zoomed.to_view(under.0, under.1);
    assert!(
        close(after.0, pivot.0) && close(after.1, pivot.1),
        "{after:?}"
    );

    // Clamped at both ends, and still pivoting.
    let most = camera.zoomed(100.0, pivot);
    assert_eq!(most.zoom, MAX_ZOOM);
    let after = most.to_view(under.0, under.1);
    assert!(close(after.0, pivot.0) && close(after.1, pivot.1));
    assert_eq!(camera.zoomed(0.001, pivot).zoom, MIN_ZOOM);
}

#[test]
fn panning_moves_the_picture_by_the_drag() {
    let camera = Camera::default().panned(15.0, -4.0);
    assert_eq!(camera.to_view(0.0, 0.0), (15.0, -4.0));
    assert_eq!(camera.zoom, 1.0);
}

// --- the trace table (Task 7) --------------------------------------------------

/// The traces of `home_store()`: the failing and the successful checkout and one
/// health check of each of the five services.
const TRACES: usize = 7;

fn table_of(driver: &Driver<MainWindow>) -> Entity<TraceTableView> {
    driver.read_root(|main| main.table().clone())
}

/// The trace IDs of the table's rows, top to bottom.
fn row_ids(driver: &Driver<MainWindow>) -> Vec<TraceId> {
    let home = home_of(driver);
    driver.read(|cx| {
        home.read(cx)
            .data()
            .rows
            .iter()
            .map(|row| row.trace_id)
            .collect()
    })
}

/// The middle of a part of the table's panel, as of the latest frame.
fn part(driver: &Driver<MainWindow>, part: TablePart) -> Point<Pixels> {
    let table = table_of(driver);
    driver
        .read(|cx| table.read(cx).center_of(part))
        .unwrap_or_else(|| panic!("the table has no {part:?} on the screen"))
}

fn has_part(driver: &Driver<MainWindow>, part: TablePart) -> bool {
    let table = table_of(driver);
    driver.read(|cx| table.read(cx).center_of(part).is_some())
}

fn row_center(driver: &Driver<MainWindow>, row: usize) -> Point<Pixels> {
    let table = table_of(driver);
    driver
        .read(|cx| table.read(cx).row_center(row))
        .unwrap_or_else(|| panic!("the table has no row {row} on the screen"))
}

/// What is in the query input, as the user sees it.
fn input_text(driver: &Driver<MainWindow>) -> String {
    let table = table_of(driver);
    driver.read(|cx| table.read(cx).input_text(cx))
}

fn error_line(driver: &Driver<MainWindow>) -> Option<String> {
    let table = table_of(driver);
    driver.read(|cx| table.read(cx).error_line(cx))
}

fn empty_state(driver: &Driver<MainWindow>) -> Option<&'static str> {
    let table = table_of(driver);
    driver.read(|cx| table.read(cx).empty_state(cx))
}

fn count_line(driver: &Driver<MainWindow>) -> String {
    let table = table_of(driver);
    driver.read(|cx| table.read(cx).count_line(cx))
}

/// Clicks into the query input and types `text` there.
fn type_query(driver: &mut Driver<MainWindow>, text: &str) {
    driver.click(part(driver, TablePart::QueryInput));
    driver.type_text(text);
}

/// A one-span trace of `web-bff`, a millisecond long, that starts at
/// `start_nanos`; `trace` is its ID (all bytes the same).
fn lone_span(trace: u8, start_nanos: u64) -> Span {
    Span {
        trace_id: TraceId([trace; 16]),
        span_id: SpanId([trace; 8]),
        parent_span_id: None,
        name: "GET /late".to_owned(),
        kind: SpanKind::Server,
        start_unix_nano: start_nanos,
        end_unix_nano: start_nanos + 1_000_000,
        status: StatusCode::Unset,
        status_message: String::new(),
        attributes: Vec::new(),
        events: Vec::new(),
        resource: Arc::new(Resource {
            attributes: vec![(
                "service.name".to_owned(),
                AttrValue::Str("web-bff".to_owned()),
            )],
        }),
        scope_name: "test".to_owned(),
    }
}

fn screen(driver: &mut Driver<MainWindow>) -> Screen {
    driver.update_root(|main, _, _| main.screen().clone())
}

#[test]
fn the_table_lists_every_trace_newest_first() {
    let driver = home_driver(&Theme::dark());
    let home = home_of(&driver);
    let rows = driver.read(|cx| home.read(cx).data().rows.clone());
    assert_eq!(rows.len(), TRACES);
    let started: Vec<&str> = rows.iter().map(|row| row.started.as_str()).collect();
    let mut newest_first = started.clone();
    newest_first.sort_by(|a, b| b.cmp(a));
    assert_eq!(started, newest_first, "newest first");
    // The two checkouts are the oldest: the successful one, then the failing.
    assert_eq!(&started[TRACES - 2..], ["08:00:05.000", "08:00:00.000"]);
    assert_eq!(count_line(&driver), "7 traces \u{b7} newest first");
    assert_eq!(empty_state(&driver), None);
}

#[test]
fn click_row_opens_trace() {
    let mut driver = home_driver(&Theme::dark());
    let home = home_of(&driver);
    let rows = driver.read(|cx| home.read(cx).data().rows.clone());
    let failed = rows
        .iter()
        .position(|row| row.failed_in.is_some())
        .expect("the failing checkout has a row");

    driver.click(row_center(&driver, failed));

    assert_eq!(
        screen(&mut driver),
        Screen::Trace(rows[failed].trace_id),
        "the click opened the failing checkout"
    );
}

#[test]
fn every_row_opens_its_own_trace() {
    // A click lands on the row under the pointer, whatever its place: the
    // geometry the table reports for a row is the one it is drawn in.
    let mut driver = home_driver(&Theme::dark());
    let ids = row_ids(&driver);
    assert_eq!(ids.len(), TRACES);
    for (row, id) in ids.iter().enumerate() {
        driver.update_root(|main, _, cx| main.navigate(Screen::Home, cx));
        driver.draw();
        driver.click(row_center(&driver, row));
        assert_eq!(screen(&mut driver), Screen::Trace(*id), "row {row}");
    }
}

#[test]
fn a_click_outside_the_rows_opens_nothing() {
    let mut driver = home_driver(&Theme::dark());
    // The header of the table, and the query row above it.
    let table = table_of(&driver);
    let top = driver
        .read(|cx| table.read(cx).bounds_of(TablePart::Table))
        .expect("the table is on the screen")
        .origin;
    driver.click(top + point(px(40.0), px(10.0)));
    assert_eq!(screen(&mut driver), Screen::Home, "the header is not a row");
    driver.click(part(&driver, TablePart::QueryInput));
    assert_eq!(screen(&mut driver), Screen::Home);
}

#[test]
fn invalid_query_shows_error_and_keeps_rows() {
    let mut driver = home_driver(&Theme::dark());
    let before = row_ids(&driver);
    assert_eq!(before.len(), TRACES);
    assert_eq!(error_line(&driver), None, "an empty bar has no error");

    type_query(&mut driver, "{ status = ");

    // What was typed is in the bar, the line under it says where the text
    // stops making sense (the 12th column, one past the 11 typed) and the table
    // still shows what it showed.
    assert_eq!(input_text(&driver), "{ status = ");
    let line = error_line(&driver).expect("the bar says what is wrong");
    assert!(line.starts_with("at column 12: "), "{line}");
    assert!(
        line.len() > "at column 12: ".len(),
        "a message follows: {line}"
    );
    assert_eq!(row_ids(&driver), before, "the previous rows are kept");
    assert_eq!(empty_state(&driver), None, "not an empty table");
    assert_eq!(count_line(&driver), "7 traces \u{b7} newest first");

    assert_snapshot("home_query_error_dark", &driver.capture());
}

#[test]
fn the_error_line_goes_when_the_text_parses() {
    let mut driver = home_driver(&Theme::dark());
    type_query(&mut driver, "{ status = ");
    assert!(error_line(&driver).is_some());

    // Finishing the query: it parses, the line goes and the rows are the
    // query's. `status = error` holds for the failing checkout only.
    driver.type_text("error }");
    assert_eq!(input_text(&driver), "{ status = error }");
    assert_eq!(error_line(&driver), None);
    let home = home_of(&driver);
    assert!(matches!(
        driver.read(|cx| home.read(cx).data().query.clone()),
        QueryState::Valid(_)
    ));
    let rows = row_ids(&driver);
    assert_eq!(rows.len(), 1, "typing applies the query, without Enter");
    assert!(driver.read(|cx| home.read(cx).data().rows[0].failed_in.is_some()));
}

#[test]
fn a_query_that_stops_parsing_keeps_the_last_good_rows() {
    let mut driver = home_driver(&Theme::dark());
    type_query(&mut driver, "{ status = error }");
    let good = row_ids(&driver);
    assert_eq!(good.len(), 1);

    // More text, and the query no longer parses: the table does not go back to
    // all seven traces, and does not go empty either.
    driver.type_text(" && ");
    assert!(error_line(&driver).is_some(), "{:?}", input_text(&driver));
    assert_eq!(row_ids(&driver), good, "the last good query still filters");
    assert_eq!(empty_state(&driver), None);
    assert_eq!(count_line(&driver), "1 trace \u{b7} newest first");
}

#[test]
fn a_service_click_fills_the_input_and_clears_it_again() {
    let mut driver = home_driver(&Theme::dark());
    assert_eq!(input_text(&driver), "");

    driver.click(node_center(&driver, PAYMENTS));
    assert_eq!(input_text(&driver), service_filter(PAYMENTS));
    assert_eq!(row_ids(&driver).len(), 3, "the table follows the click");
    assert_eq!(error_line(&driver), None);

    // The selected node again: the selection goes, and so does its text.
    driver.click(node_center(&driver, PAYMENTS));
    assert_eq!(input_text(&driver), "");
    assert_eq!(row_ids(&driver).len(), TRACES);
}

#[test]
fn typing_replaces_what_a_click_wrote() {
    let mut driver = home_driver(&Theme::dark());
    driver.click(node_center(&driver, PAYMENTS));
    // The bar's text is now the user's: a click on empty space keeps it (the
    // test above this one in `clicking_empty_space_keeps_a_typed_query`), and
    // typing on appends to what the click wrote.
    driver.click(part(&driver, TablePart::QueryInput));
    driver.type_text(" ");
    assert_eq!(
        input_text(&driver),
        format!("{} ", service_filter(PAYMENTS))
    );
    assert_eq!(selected(&driver), None, "no longer the click's text");
}

#[test]
fn run_and_enter_read_the_store_again() {
    let (mut driver, store) = home_driver_with_store(&Theme::dark());
    let home = home_of(&driver);
    // Paused: the 500 ms refresh stands still, so the only ways to see a new
    // trace are the ones the user has.
    driver.update_root(|_, _, cx| home.update(cx, |home, cx| home.set_paused(true, cx)));
    driver.draw();
    let now = store.now_unix_nano();
    store.ingest_spans(vec![lone_span(1, now)]);
    driver.advance(Duration::from_millis(1_200));
    assert_eq!(row_ids(&driver).len(), TRACES, "paused: not read yet");

    driver.click(part(&driver, TablePart::Run));
    assert_eq!(row_ids(&driver).len(), TRACES + 1, "`Run` reads again");

    store.ingest_spans(vec![lone_span(2, now + 1_000_000)]);
    driver.advance(Duration::from_millis(1_200));
    assert_eq!(row_ids(&driver).len(), TRACES + 1, "still paused");
    driver.click(part(&driver, TablePart::QueryInput));
    driver.key("enter");
    assert_eq!(row_ids(&driver).len(), TRACES + 2, "Enter reads again");
}

#[test]
fn home_errors_only_dark() {
    let mut driver = home_driver(&Theme::dark());
    let home = home_of(&driver);
    assert!(!driver.read(|cx| home.read(cx).errors_only()));

    driver.click(part(&driver, TablePart::ErrorsOnly));

    assert!(driver.read(|cx| home.read(cx).errors_only()), "switched on");
    let rows = driver.read(|cx| home.read(cx).data().rows.clone());
    assert_eq!(rows.len(), 1, "only the failing checkout");
    assert!(rows[0].failed_in.is_some());
    assert_eq!(count_line(&driver), "1 trace \u{b7} newest first");
    assert_snapshot("home_errors_only_dark", &driver.capture());

    // And off again: every trace.
    driver.click(part(&driver, TablePart::ErrorsOnly));
    assert!(!driver.read(|cx| home.read(cx).errors_only()));
    assert_eq!(row_ids(&driver).len(), TRACES);
}

#[test]
fn the_slow_toggle_keeps_the_traces_of_a_second_or_more() {
    let mut driver = home_driver(&Theme::dark());
    let home = home_of(&driver);
    driver.click(part(&driver, TablePart::SlowOnly));
    assert!(driver.read(|cx| home.read(cx).slow_only()));
    let rows = driver.read(|cx| home.read(cx).data().rows.clone());
    assert_eq!(rows.len(), 2, "the two checkouts; a health check is quick");
    assert!(rows.iter().all(|row| row.root == "POST /api/checkout"));

    // Both toggles together: slow and failing.
    driver.click(part(&driver, TablePart::ErrorsOnly));
    assert_eq!(row_ids(&driver).len(), 1);
    assert!(driver.read(|cx| home.read(cx).errors_only() && home.read(cx).slow_only()));
}

#[test]
fn no_match_shows_clear_filters() {
    let mut driver = home_driver(&Theme::dark());
    assert!(
        !has_part(&driver, TablePart::ClearFilters),
        "rows: no button"
    );

    type_query(
        &mut driver,
        "{ resource.service.name = \"no-such-service\" }",
    );

    assert_eq!(
        error_line(&driver),
        None,
        "the query is fine, it matches nothing"
    );
    assert!(row_ids(&driver).is_empty());
    assert_eq!(empty_state(&driver), Some(EMPTY_NO_MATCH));
    assert_eq!(EMPTY_NO_MATCH, "No traces match");
    assert_eq!(CLEAR_FILTERS, "Clear filters");
    assert!(has_part(&driver, TablePart::ClearFilters));
    assert_snapshot("home_no_match_dark", &driver.capture());

    // The button takes the filters off: the bar is empty and the rows are back.
    driver.click(part(&driver, TablePart::ClearFilters));
    assert_eq!(input_text(&driver), "");
    assert_eq!(row_ids(&driver).len(), TRACES);
    assert_eq!(empty_state(&driver), None);
    assert!(!has_part(&driver, TablePart::ClearFilters));
}

#[test]
fn a_toggle_with_no_match_offers_clear_filters_too() {
    // A store with nothing failing: `Errors only` leaves nothing to show.
    let (mut driver, _) = {
        use loctel_store::{ManualClock, StoreConfig};
        let clock = Arc::new(ManualClock::new(common::BASE));
        let store = Store::new(StoreConfig::default(), clock.clone());
        common::feed(
            &store,
            &loctel_meshgen::generate(
                &loctel_meshgen::Scenario::CheckoutSuccess,
                common::SEED,
                common::BASE,
            ),
        );
        clock.advance(Duration::from_secs(30));
        let backend = Rc::new(FixedBackend::new(store.clone()));
        let driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
            cx.new(|cx| MainWindow::new(backend, None, window, cx))
        });
        (driver, store)
    };
    assert_eq!(row_ids(&driver).len(), 1);
    driver.click(part(&driver, TablePart::ErrorsOnly));
    assert_eq!(empty_state(&driver), Some(EMPTY_NO_MATCH));
    driver.click(part(&driver, TablePart::ClearFilters));
    let home = home_of(&driver);
    assert!(
        !driver.read(|cx| home.read(cx).errors_only()),
        "the toggle is off"
    );
    assert_eq!(row_ids(&driver).len(), 1);
}

#[test]
fn an_empty_table_is_no_traces_yet() {
    use loctel_store::{ManualClock, StoreConfig};
    let store = Store::new(StoreConfig::default(), Arc::new(ManualClock::new(0)));
    let backend = Rc::new(FixedBackend::new(store));
    let driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    assert_eq!(empty_state(&driver), Some(EMPTY_NO_DATA));
    assert_eq!(EMPTY_NO_DATA, "No traces yet");
    assert!(
        !has_part(&driver, TablePart::ClearFilters),
        "nothing to clear: there is no filter"
    );
    assert_eq!(count_line(&driver), "0 traces \u{b7} newest first");
}

#[test]
fn the_table_is_laid_out_inside_its_panel() {
    // At the default window and at the smallest one the columns fit the panel:
    // no horizontal scroll, and the table's area is the panel's.
    let mut driver = home_driver(&Theme::dark());
    for size in [WINDOW, (1000, 640)] {
        driver.resize(size);
        driver.draw();
        let table = table_of(&driver);
        let (bounds, widths) = driver.read(|cx| {
            let table = table.read(cx);
            (
                table.bounds_of(TablePart::Table).expect("measured"),
                table.column_widths(),
            )
        });
        let total: f32 = widths.iter().sum();
        assert!(
            total <= f32::from(bounds.size.width) + 0.5,
            "{size:?}: the columns ({total}) fit the table ({:?})",
            bounds.size.width
        );
        assert!(
            total > f32::from(bounds.size.width) - 40.0,
            "{size:?}: and use most of it ({total} of {:?})",
            bounds.size.width
        );
    }
}

/// The pixels of `image` (taken at scale 2) inside the logical rectangle from
/// `(x0, y0)` to `(x1, y1)` that differ from `other`'s.
fn pixels_differing(
    image: &image::RgbaImage,
    other: &image::RgbaImage,
    (x0, y0): (f32, f32),
    (x1, y1): (f32, f32),
) -> usize {
    let scaled = |v: f32| (v * 2.0) as u32;
    let mut count = 0;
    for y in scaled(y0)..scaled(y1) {
        for x in scaled(x0)..scaled(x1) {
            if image.get_pixel(x, y) != other.get_pixel(x, y) {
                count += 1;
            }
        }
    }
    count
}

#[test]
fn the_services_dots_have_a_tooltip_with_the_names() {
    let mut driver = home_driver(&Theme::dark());
    let table = table_of(&driver);
    let (bounds, widths) = driver.read(|cx| {
        let table = table.read(cx);
        (
            table.bounds_of(TablePart::Table).expect("measured"),
            table.column_widths(),
        )
    });
    // The services cell of the failing checkout, the last row: its first dot.
    let services_x = f32::from(bounds.origin.x) + widths[..3].iter().sum::<f32>() + 20.0;
    let failed_row = row_center(&driver, TRACES - 1);
    let above = row_center(&driver, TRACES - 2);

    // The tooltip opens above and to the left of the pointer, over the row
    // before this one, where that row has nothing but its own background: a band
    // of that row, looked at before and after the pointer rests on the dots.
    let band = (
        (services_x - 380.0, f32::from(above.y) + 9.0),
        (services_x - 30.0, f32::from(above.y) + 17.0),
    );
    let before = driver.capture();

    // Resting on the name of the trace shows nothing there.
    driver.hover(point(px(300.0), failed_row.y));
    driver.advance(Duration::from_millis(1_500));
    let on_the_name = driver.capture();
    assert_eq!(
        pixels_differing(&before, &on_the_name, band.0, band.1),
        0,
        "no tooltip over the trace's name"
    );

    driver.hover(point(px(services_x), failed_row.y));
    driver.advance(Duration::from_millis(1_500));
    let on_the_dots = driver.capture();
    let changed = pixels_differing(&before, &on_the_dots, band.0, band.1);
    assert!(
        changed > 500,
        "a tooltip is drawn over the row above ({changed} pixels changed)"
    );
}

#[test]
fn the_clear_button_of_the_input_empties_the_query() {
    let mut driver = home_driver(&Theme::dark());
    type_query(&mut driver, "{ status = error }");
    assert_eq!(row_ids(&driver).len(), 1);

    // The input's own clear button, at its right end (it shows once there is
    // text).
    let input = {
        let table = table_of(&driver);
        driver
            .read(|cx| table.read(cx).bounds_of(TablePart::QueryInput))
            .expect("the input is on the screen")
    };
    driver.click(point(
        input.origin.x + input.size.width - px(14.0),
        input.center().y,
    ));

    assert_eq!(input_text(&driver), "");
    let home = home_of(&driver);
    assert_eq!(
        driver.read(|cx| home.read(cx).query_text().to_owned()),
        "",
        "the model has the text the input shows"
    );
    assert_eq!(
        row_ids(&driver).len(),
        TRACES,
        "and the table is whole again"
    );
}
