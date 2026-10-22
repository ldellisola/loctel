//! The main window's interactions: the service map's (M3 plan, Task 6), the
//! trace table's (Task 7) and the trace screen's (Task 8: selecting, folding,
//! find, the errors path, Esc and the Back button, the inspector's buttons).
//!
//! The map: clicking a service writes its filter into the query bar, a drag
//! pans, ctrl or cmd plus the wheel and a pinch zoom (0.5 to 2.0), and `Fit`
//! puts the map back. The table: typing a query validates live and keeps the
//! last good rows when it does not parse, Enter and `Run` read again, the two
//! toggles narrow the rows, a click on a row opens its trace and the empty
//! table offers `Clear filters`. The toolbar's (decision 0032): Pause freezes the
//! view while the store goes on receiving, the range menu changes the rows, and
//! Clear all empties the map, the table and the store. The sidebar's (M4 plan,
//! Task 7): the three entries switch screens and every screen keeps its state,
//! ⌘1 to ⌘3 and ⌘, are the keyboard's way, and the trace screen's `Back` returns
//! to the screen the trace was opened from.
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
use loctel_rules::{DropScope, RuleLanguage};
use loctel_store::{PageRequest, Store, TraceFilter};
use loctel_ui::app::{
    HomeModel, INSET, MainWindow, NAV_ITEM_GAP, NAV_ITEM_HEIGHT, NavItem, SIDEBAR_WIDTH, Screen,
    TOOLBAR_HEIGHT, TRAFFIC_LIGHT_SPACE, TimeRange, ToolbarPart, UiBackend, open_main_window,
    open_settings_window, show_screen,
};
use loctel_ui::model::query_bar::{QueryState, service_filter};
use loctel_ui::model::rules::Draft;
use loctel_ui::testing::{Driver, FixedBackend, assert_snapshot, headless_app};
use loctel_ui::views::{
    BACK_LABEL, CLEAR_FILTERS, Camera, EMPTY_NO_DATA, EMPTY_NO_MATCH, INSPECTOR_MAX_WIDTH,
    INSPECTOR_MIN_WIDTH, LogsPart, MAX_ZOOM, MIN_ZOOM, MapView, TablePart, TracePart,
    TraceTableView,
};
use loctel_ui::{KitAssets, Theme};

use common::row_center as trace_row_center;
use common::{
    WINDOW, failing_trace_id, home_store, open_trace, part_bounds, part_center, row_named,
    row_names, selected_details, toolbar, toolbar_bounds, toolbar_center, trace_model,
    waterfall_view,
};

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

// --- the trace screen (Task 8) ------------------------------------------------------------

/// The window over the home data with the failing checkout opened (its
/// failed-in span selected).
fn trace_driver() -> Driver<MainWindow> {
    let mut driver = home_driver(&Theme::dark());
    open_trace(&mut driver, failing_trace_id());
    driver
}

fn selected_name(driver: &Driver<MainWindow>) -> String {
    selected_details(driver).expect("a span is selected").name
}

/// The offset of the selected span, which tells the two `POST`s to the card
/// gateway (236 and 840 ms) and the other `POST`s apart.
fn selected_offset(driver: &Driver<MainWindow>) -> String {
    selected_details(driver).expect("a span is selected").offset
}

fn find_label(driver: &Driver<MainWindow>) -> String {
    let model = trace_model(driver);
    driver.read(|cx| model.read(cx).find_label())
}

fn is_home(driver: &mut Driver<MainWindow>) -> bool {
    screen(driver) == Screen::Home
}

/// The text in the waterfall's find box, as the user sees it.
fn find_text(driver: &Driver<MainWindow>) -> String {
    let waterfall = waterfall_view(driver);
    driver.read(|cx| waterfall.read(cx).find_text(cx))
}

fn find_focused(driver: &mut Driver<MainWindow>) -> bool {
    let waterfall = waterfall_view(driver);
    driver.update_root(|_, window, cx| waterfall.read(cx).find_focused(window, cx))
}

fn clipboard_text(driver: &Driver<MainWindow>) -> Option<String> {
    driver.read(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
}

#[test]
fn opening_a_trace_selects_the_failed_in_span_and_reads_its_logs() {
    let driver = trace_driver();
    assert_eq!(selected_name(&driver), "POST");
    assert_eq!(selected_offset(&driver), "starts at +236 ms");
    // The failed-in span has no logs of its own; the logs panel says so.
    let model = trace_model(&driver);
    assert!(driver.read(|cx| model.read(cx).logs_loaded()));
    assert!(driver.read(|cx| model.read(cx).logs().is_empty()));
}

#[test]
fn clicking_a_row_selects_its_span_and_the_logs_follow() {
    let mut driver = trace_driver();
    let model = trace_model(&driver);

    // `POST /orders` has three logs of its own (the first, 190 and 1800 ms).
    driver.click(trace_row_center(
        &driver,
        row_named(&driver, "POST /orders"),
    ));
    driver.draw();
    assert_eq!(selected_name(&driver), "POST /orders");
    assert_eq!(driver.read(|cx| model.read(cx).logs().len()), 3);

    // A span with no logs: the panel is empty, and says so.
    driver.click(trace_row_center(
        &driver,
        row_named(&driver, "validate_token"),
    ));
    driver.draw();
    assert_eq!(selected_name(&driver), "validate_token");
    assert!(driver.read(|cx| model.read(cx).logs().is_empty()));
    assert!(driver.read(|cx| model.read(cx).logs_loaded()));
}

#[test]
fn a_click_on_a_row_keeps_the_rows_where_they_are() {
    let mut driver = trace_driver();
    let before = row_names(&driver);
    driver.click(trace_row_center(&driver, 3));
    assert_eq!(row_names(&driver), before);
}

#[test]
fn the_chevron_folds_and_opens_without_selecting() {
    let mut driver = trace_driver();
    let selected = selected_offset(&driver);
    let orders = row_named(&driver, "POST /orders");
    assert_eq!(row_names(&driver).len(), 15);

    let chevron = {
        let waterfall = waterfall_view(&driver);
        driver
            .read(|cx| waterfall.read(cx).chevron_center(orders, cx))
            .expect("the row is in view")
    };
    driver.click(chevron);
    // `POST /orders` has eleven spans under it (every span but the root, the
    // token check, the web-bff `POST` and itself).
    assert_eq!(row_names(&driver).len(), 15 - 11);
    assert_eq!(
        selected_offset(&driver),
        selected,
        "the selection did not move"
    );

    driver.click(chevron);
    assert_eq!(row_names(&driver).len(), 15);
    assert_eq!(selected_offset(&driver), selected);
}

#[test]
fn collapse_all_and_expand_all() {
    let mut driver = trace_driver();
    assert_eq!(row_names(&driver).len(), 15);

    driver.click(part_center(&driver, TracePart::ToggleAll));
    assert_eq!(row_names(&driver), ["POST /api/checkout"], "only the root");
    // The selection (hidden now) stays selected.
    assert_eq!(selected_offset(&driver), "starts at +236 ms");

    // The button now offers the way back.
    let model = trace_model(&driver);
    assert!(driver.read(|cx| model.read(cx).any_folded()));
    driver.click(part_center(&driver, TracePart::ToggleAll));
    assert_eq!(row_names(&driver).len(), 15);
    assert!(!driver.read(|cx| model.read(cx).any_folded()));
}

#[test]
fn the_errors_path_toggle_comes_off_again() {
    let mut driver = trace_driver();
    driver.click(part_center(&driver, TracePart::ErrorsPath));
    assert_eq!(row_names(&driver).len(), 7);
    driver.click(part_center(&driver, TracePart::ErrorsPath));
    assert_eq!(row_names(&driver).len(), 15);
}

#[test]
fn find_counts_and_jumps() {
    let mut driver = trace_driver();
    assert_eq!(find_label(&driver), "", "an empty box says nothing");

    // `card-gateway` is the address of the two `POST`s of payments-api (236
    // and 840 ms); the selected span is the first of them.
    driver.click(part_center(&driver, TracePart::Find));
    driver.type_text("card-gateway");
    assert_eq!(find_text(&driver), "card-gateway");
    assert_eq!(
        find_label(&driver),
        "1 of 2",
        "the selected span is a match"
    );

    driver.key("enter");
    assert_eq!(find_label(&driver), "2 of 2");
    assert_eq!(selected_offset(&driver), "starts at +840 ms", "it jumped");

    driver.key("enter");
    assert_eq!(find_label(&driver), "1 of 2", "and wraps round");
    assert_eq!(selected_offset(&driver), "starts at +236 ms");

    // Another text, with matches the selection is not one of: counted, and
    // the first Enter goes to the first of them.
    for _ in 0..12 {
        driver.key("backspace");
    }
    driver.type_text("postgresql");
    assert_eq!(find_label(&driver), "2 matches");
    driver.key("enter");
    assert_eq!(find_label(&driver), "1 of 2");
    assert_eq!(selected_name(&driver), "SELECT carts");
    driver.key("enter");
    assert_eq!(selected_name(&driver), "SELECT payment_methods");

    // No match: said so, and Enter does nothing.
    for _ in 0..10 {
        driver.key("backspace");
    }
    driver.type_text("zzz");
    assert_eq!(find_label(&driver), "No matches");
    driver.key("enter");
    assert_eq!(selected_name(&driver), "SELECT payment_methods");
}

#[test]
fn find_looks_at_the_names_and_the_services_too() {
    let mut driver = trace_driver();
    driver.click(part_center(&driver, TracePart::Find));
    // `inventory` is the service of two spans (`GET /stock/{sku}` and `MGET`) and
    // the address of the call to them, which the attribute values find: three.
    // No span has it in its name.
    driver.type_text("INVENTORY");
    assert_eq!(find_label(&driver), "3 matches", "ignoring case");
}

#[test]
fn a_jump_scrolls_the_row_into_view() {
    // Matches far down a long list: Enter brings the row into the viewport.
    let mut driver = trace_driver();
    let waterfall = waterfall_view(&driver);
    driver.click(part_center(&driver, TracePart::Find));
    driver.type_text("order.failed process");
    driver.key("enter");
    assert_eq!(selected_name(&driver), "order.failed process");
    let row = row_named(&driver, "order.failed process");
    assert!(
        driver
            .read(|cx| waterfall.read(cx).row_center(row))
            .is_some(),
        "the row of the match is in view"
    );
}

#[test]
fn cmd_f_gives_the_find_box_the_keyboard() {
    let mut driver = trace_driver();
    assert!(!find_focused(&mut driver));
    driver.key("cmd-f");
    assert!(find_focused(&mut driver));
    // And what is typed goes into it.
    driver.type_text("redis");
    assert_eq!(find_text(&driver), "redis");
    assert_eq!(find_label(&driver), "1 match");
}

#[test]
fn escape_goes_back_but_clears_the_find_box_first() {
    let mut driver = trace_driver();
    driver.click(part_center(&driver, TracePart::Find));
    driver.type_text("redis");
    assert_eq!(find_text(&driver), "redis");

    // The first Esc empties the box and stays on the trace.
    driver.key("escape");
    assert_eq!(find_text(&driver), "");
    assert_eq!(find_label(&driver), "");
    assert!(!is_home(&mut driver));
    assert_eq!(selected_offset(&driver), "starts at +236 ms");

    // The second one, with the box empty, goes back.
    driver.key("escape");
    assert!(is_home(&mut driver));
}

#[test]
fn escape_goes_back_without_a_click() {
    // The screen has the keyboard when it opens.
    let mut driver = trace_driver();
    driver.key("escape");
    assert!(is_home(&mut driver));
}

#[test]
fn escape_goes_back_after_a_click_on_a_row() {
    let mut driver = trace_driver();
    driver.click(part_center(&driver, TracePart::Find));
    driver.click(trace_row_center(&driver, 2));
    driver.key("escape");
    assert!(is_home(&mut driver), "a click on a row gives the keys back");
}

#[test]
fn back_returns_home_keeping_query() {
    let mut driver = home_driver(&Theme::dark());
    // Narrow the table to the failing checkout with a typed query.
    type_query(&mut driver, "{ status = error }");
    assert_eq!(row_ids(&driver).len(), 1);
    let failed = row_ids(&driver)[0];

    driver.click(row_center(&driver, 0));
    assert_eq!(screen(&mut driver), Screen::Trace(failed));
    driver.draw();
    driver.draw();

    // The `Back` button is the way back.
    driver.click(part_center(&driver, TracePart::Back));
    assert_eq!(screen(&mut driver), Screen::Home);
    driver.draw();

    // The home screen is as it was left: the query in the bar, the table
    // filtered by it.
    assert_eq!(input_text(&driver), "{ status = error }");
    assert_eq!(query_text(&driver), "{ status = error }");
    assert_eq!(row_ids(&driver), vec![failed]);
    assert_eq!(count_line(&driver), "1 trace \u{b7} newest first");
    // And the trace screen is closed.
    assert!(driver.read_root(|main| main.trace_view().is_none()));
}

#[test]
fn the_waterfall_selection_starts_again_when_a_trace_is_reopened() {
    let mut driver = trace_driver();
    driver.click(trace_row_center(
        &driver,
        row_named(&driver, "validate_token"),
    ));
    assert_eq!(selected_name(&driver), "validate_token");

    driver.key("escape");
    assert!(is_home(&mut driver));
    open_trace(&mut driver, failing_trace_id());
    assert_eq!(selected_offset(&driver), "starts at +236 ms", "failed in");
}

#[test]
fn traces_through_span_name_sets_query() {
    let mut driver = trace_driver();
    driver.click(trace_row_center(
        &driver,
        row_named(&driver, "POST /charges"),
    ));
    assert_eq!(selected_name(&driver), "POST /charges");

    driver.click(part_center(&driver, TracePart::TracesThrough));
    driver.draw();

    // Home, with the query in the bar and the table following it: the two
    // checkouts have a `POST /charges`, the health checks do not.
    assert_eq!(screen(&mut driver), Screen::Home);
    assert_eq!(query_text(&driver), "{ name = \"POST /charges\" }");
    assert_eq!(input_text(&driver), "{ name = \"POST /charges\" }");
    assert_eq!(error_line(&driver), None, "the query parses");
    assert_eq!(row_ids(&driver).len(), 2);
}

#[test]
fn copy_id_copies_the_whole_trace_id() {
    let mut driver = trace_driver();
    driver.click(part_center(&driver, TracePart::CopyId));
    assert_eq!(
        clipboard_text(&driver).as_deref(),
        Some(failing_trace_id().to_string().as_str())
    );
    assert_eq!(failing_trace_id().to_string().len(), 32);
}

#[test]
fn the_inspectors_copy_buttons_copy_the_exception_and_the_span() {
    let mut driver = trace_driver();
    driver.click(trace_row_center(
        &driver,
        row_named(&driver, "POST /charges"),
    ));

    driver.click(part_center(&driver, TracePart::CopyException));
    let copied = clipboard_text(&driver).expect("the exception was copied");
    assert!(
        copied.starts_with(
            "Payments.Api.Gateway.CardGatewayTimeoutException: card-gateway did not respond after 2 attempts\n"
        ),
        "{copied}"
    );
    assert!(copied.contains("CardGatewayClient.ChargeAsync"), "{copied}");

    driver.click(part_center(&driver, TracePart::CopyJson));
    let json = clipboard_text(&driver).expect("the span was copied");
    let value: serde_json::Value = serde_json::from_str(&json).expect("JSON");
    assert_eq!(value["name"], "POST /charges");
    assert_eq!(value["events"][0]["name"], "exception");
}

#[test]
fn the_inspector_resizes_between_280_and_520() {
    let mut driver = trace_driver();
    let width = |driver: &Driver<MainWindow>| {
        f32::from(
            part_bounds(driver, TracePart::Inspector)
                .expect("the inspector")
                .size
                .width,
        )
    };
    assert!((width(&driver) - 340.0).abs() <= 1.5);

    // The handle is on the inspector's left edge.
    let handle = |driver: &Driver<MainWindow>| {
        let bounds = part_bounds(driver, TracePart::Inspector).expect("the inspector");
        point(bounds.origin.x, px(500.0))
    };
    let from = handle(&driver);
    driver.drag(from, from + point(px(-80.0), px(0.0)));
    let wider = width(&driver);
    assert!((wider - 420.0).abs() <= 2.0, "dragged 80 px left: {wider}");

    let from = handle(&driver);
    driver.drag(from, from + point(px(-400.0), px(0.0)));
    assert!(
        (width(&driver) - INSPECTOR_MAX_WIDTH).abs() <= 1.5,
        "no wider than 520: {}",
        width(&driver)
    );

    let from = handle(&driver);
    driver.drag(from, from + point(px(600.0), px(0.0)));
    assert!(
        (width(&driver) - INSPECTOR_MIN_WIDTH).abs() <= 1.5,
        "no narrower than 280: {}",
        width(&driver)
    );
}

#[test]
fn the_trace_screen_fits_the_smallest_window() {
    let mut driver = trace_driver();
    driver.resize((1000, 640));
    driver.draw();
    let inspector = part_bounds(&driver, TracePart::Inspector).expect("the inspector");
    let list = part_bounds(&driver, TracePart::List).expect("the list");
    assert!(inspector.origin.x + inspector.size.width <= px(1000.0 - INSET_PX));
    assert!(
        list.size.width >= px(380.0),
        "the waterfall keeps room: {:?}",
        list.size.width
    );
    assert!(
        list.size.height > px(60.0),
        "and some rows: {:?}",
        list.size.height
    );
    assert_eq!(selected_offset(&driver), "starts at +236 ms");
}

/// The window's inset (the panels sit this far from the edge).
const INSET_PX: f32 = 10.0;

#[test]
fn the_trace_view_follows_the_appearance() {
    let mut driver = trace_driver();
    driver.update_root(|main, _, cx| main.set_appearance(gpui::WindowAppearance::Light, cx));
    driver.draw();
    // Still the same screen, in the other look.
    assert_eq!(selected_offset(&driver), "starts at +236 ms");
    assert!(part_bounds(&driver, TracePart::Inspector).is_some());
}

// --- the toolbar: Live, the time range, Pause and Clear all -----------------------
//
// Design 9.2: the home screen's toolbar has a Live indicator, the time range,
// Pause (which freezes the view while ingest continues) and Clear all. Each
// control is a click on a button of the real window that lands in the
// `HomeModel` (`set_range`, `set_paused`, `clear_all`): nothing is kept in the
// toolbar but whether the range menu is open.

const MINUTE: u64 = 60 * common::SECOND;

/// How many traces the store holds, whatever the window shows.
fn stored_traces(store: &Store) -> usize {
    store
        .search_traces(
            &TraceFilter::default(),
            &PageRequest {
                limit: 1_000,
                cursor: None,
            },
        )
        .items
        .len()
}

fn live_text(driver: &Driver<MainWindow>) -> &'static str {
    let controls = toolbar(driver);
    driver.read(|cx| controls.read(cx).live_text(cx))
}

fn pause_text(driver: &Driver<MainWindow>) -> &'static str {
    let controls = toolbar(driver);
    driver.read(|cx| controls.read(cx).pause_text(cx))
}

fn range_open(driver: &Driver<MainWindow>) -> bool {
    let controls = toolbar(driver);
    driver.read(|cx| controls.read(cx).range_open())
}

fn range_of(driver: &Driver<MainWindow>) -> TimeRange {
    let home = home_of(driver);
    driver.read(|cx| home.read(cx).range())
}

fn is_paused(driver: &Driver<MainWindow>) -> bool {
    let home = home_of(driver);
    driver.read(|cx| home.read(cx).paused())
}

/// Opens the range menu and clicks the entry of `range`.
fn choose_range(driver: &mut Driver<MainWindow>, range: TimeRange) {
    driver.click(toolbar_center(driver, ToolbarPart::Range));
    assert!(range_open(driver), "the button opens the menu");
    driver.click(toolbar_center(driver, ToolbarPart::RangeOption(range)));
}

#[test]
fn the_toolbar_has_the_four_controls_in_order_inside_its_strip() {
    let driver = home_driver(&Theme::dark());
    let bounds = |part| {
        toolbar_bounds(&driver, part)
            .unwrap_or_else(|| panic!("the toolbar has no {part:?} on the screen"))
    };
    let live = bounds(ToolbarPart::Live);
    let range = bounds(ToolbarPart::Range);
    let pause = bounds(ToolbarPart::Pause);
    let clear = bounds(ToolbarPart::ClearAll);

    // Left to right, in the order of the design, without overlapping.
    assert!(live.right() <= range.left(), "Live, then the range");
    assert!(range.right() <= pause.left(), "then Pause");
    assert!(pause.right() <= clear.left(), "then Clear all");

    // Inside the strip of the toolbar (a 48 px strip under the window's 10 px
    // inset), at the right of the content area and in the window.
    let strip_top = px(10.0);
    let strip_bottom = px(10.0 + TOOLBAR_HEIGHT);
    for part in [live, range, pause, clear] {
        assert!(part.top() >= strip_top && part.bottom() <= strip_bottom);
        assert!(part.right() <= px(WINDOW.0 as f32 - 10.0));
        assert!(part.left() > px(240.0 + 200.0), "right of the title");
    }

    assert_eq!(live_text(&driver), "Live");
    assert_eq!(pause_text(&driver), "Pause");
    assert_eq!(range_of(&driver), TimeRange::Last15m);
    assert!(!range_open(&driver));
    assert!(
        toolbar_bounds(&driver, ToolbarPart::RangeOption(TimeRange::Last5m)).is_none(),
        "no menu until it is opened"
    );
}

#[test]
fn the_toolbar_fits_the_smallest_window() {
    // 1000 by 640: the content is 750 px wide. The title, then the four
    // controls, with no overlap and nothing past the window's edge, and the
    // range menu still opens.
    let mut driver = home_driver(&Theme::dark());
    driver.resize((1000, 640));
    let bounds = |driver: &Driver<MainWindow>, part| {
        toolbar_bounds(driver, part)
            .unwrap_or_else(|| panic!("the toolbar has no {part:?} on the screen"))
    };
    let live = bounds(&driver, ToolbarPart::Live);
    let range = bounds(&driver, ToolbarPart::Range);
    let pause = bounds(&driver, ToolbarPart::Pause);
    let clear = bounds(&driver, ToolbarPart::ClearAll);
    // The title starts 18 px in from the toolbar's left edge (x = 240) and is
    // about 110 px wide.
    assert!(live.left() > px(240.0 + 18.0 + 110.0), "clear of the title");
    assert!(live.right() <= range.left());
    assert!(range.right() <= pause.left());
    assert!(pause.right() <= clear.left());
    assert!(clear.right() <= px(1000.0 - 10.0), "inside the window");

    driver.click(range.center());
    assert!(range_open(&driver));
    let option = bounds(&driver, ToolbarPart::RangeOption(TimeRange::Everything));
    assert!(option.right() <= px(1000.0) && option.bottom() <= px(640.0));
}

#[test]
fn pause_freezes_the_view_while_ingest_continues() {
    let (mut driver, store) = home_driver_with_store(&Theme::dark());
    assert_eq!(row_ids(&driver).len(), TRACES);
    assert_eq!(stored_traces(&store), TRACES);

    driver.click(toolbar_center(&driver, ToolbarPart::Pause));
    assert!(is_paused(&driver), "the click paused the view");
    assert_eq!(live_text(&driver), "Paused");
    assert_eq!(pause_text(&driver), "Resume");

    // Traces keep arriving, and the view does not move: not after one refresh
    // period, not after several.
    let now = store.now_unix_nano();
    store.ingest_spans(vec![lone_span(1, now)]);
    driver.advance(Duration::from_millis(1_200));
    store.ingest_spans(vec![lone_span(2, now + 1_000_000)]);
    driver.advance(Duration::from_millis(1_200));
    assert_eq!(stored_traces(&store), TRACES + 2, "ingest went on");
    assert_eq!(row_ids(&driver).len(), TRACES, "the table is frozen");
    let home = home_of(&driver);
    assert_eq!(
        driver.read(|cx| home.read(cx).data().map.nodes.len()),
        9,
        "and so is the map"
    );

    // The same button resumes, and the view catches up at once.
    driver.click(toolbar_center(&driver, ToolbarPart::Pause));
    assert!(!is_paused(&driver));
    assert_eq!(live_text(&driver), "Live");
    assert_eq!(pause_text(&driver), "Pause");
    assert_eq!(row_ids(&driver).len(), TRACES + 2, "caught up");

    // And it runs again by itself.
    store.ingest_spans(vec![lone_span(3, now + 2_000_000)]);
    driver.advance(Duration::from_millis(600));
    assert_eq!(row_ids(&driver).len(), TRACES + 3);
}

#[test]
fn the_range_menu_lists_the_four_ranges_and_a_choice_changes_the_rows() {
    let (mut driver, store) = home_driver_with_store(&Theme::dark());
    // One trace ten minutes old (inside the default 15 minutes, outside 5) and
    // one thirty minutes old (outside the default, inside the hour).
    let now = store.now_unix_nano();
    store.ingest_spans(vec![
        lone_span(1, now - 10 * MINUTE),
        lone_span(2, now - 30 * MINUTE),
    ]);
    driver.advance(Duration::from_millis(600));
    assert_eq!(range_of(&driver), TimeRange::Last15m);
    assert_eq!(row_ids(&driver).len(), TRACES + 1, "the ten minute one");

    // The button opens a menu of the four ranges, top to bottom.
    driver.click(toolbar_center(&driver, ToolbarPart::Range));
    assert!(range_open(&driver));
    let ys: Vec<_> = TimeRange::ALL
        .iter()
        .map(|&range| {
            toolbar_bounds(&driver, ToolbarPart::RangeOption(range))
                .unwrap_or_else(|| panic!("the menu has no entry for {range:?}"))
                .top()
        })
        .collect();
    assert!(
        ys.windows(2).all(|pair| pair[0] < pair[1]),
        "in order: {ys:?}"
    );

    // Last 5 minutes drops the ten minute one and closes the menu.
    driver.click(toolbar_center(
        &driver,
        ToolbarPart::RangeOption(TimeRange::Last5m),
    ));
    assert_eq!(range_of(&driver), TimeRange::Last5m);
    assert!(!range_open(&driver), "a choice closes the menu");
    assert!(toolbar_bounds(&driver, ToolbarPart::RangeOption(TimeRange::Last5m)).is_none());
    assert_eq!(row_ids(&driver).len(), TRACES);

    // Last hour has the thirty minute one too.
    choose_range(&mut driver, TimeRange::LastHour);
    assert_eq!(range_of(&driver), TimeRange::LastHour);
    assert_eq!(row_ids(&driver).len(), TRACES + 2);

    // Everything, and back to the default.
    choose_range(&mut driver, TimeRange::Everything);
    assert_eq!(range_of(&driver), TimeRange::Everything);
    assert_eq!(row_ids(&driver).len(), TRACES + 2);
    choose_range(&mut driver, TimeRange::Last15m);
    assert_eq!(row_ids(&driver).len(), TRACES + 1);
}

#[test]
fn a_click_outside_closes_the_range_menu_and_keeps_the_range() {
    let mut driver = home_driver(&Theme::dark());
    driver.click(toolbar_center(&driver, ToolbarPart::Range));
    assert!(range_open(&driver));

    // Somewhere that is not the menu: the table's query bar.
    driver.click(part(&driver, TablePart::QueryInput));
    assert!(!range_open(&driver), "closed");
    assert_eq!(range_of(&driver), TimeRange::Last15m, "and nothing changed");
}

#[test]
fn clear_all_empties_the_map_and_the_table_and_receiving_goes_on() {
    let (mut driver, store) = home_driver_with_store(&Theme::dark());
    assert_eq!(row_ids(&driver).len(), TRACES);
    let home = home_of(&driver);

    driver.click(toolbar_center(&driver, ToolbarPart::ClearAll));
    assert_eq!(stored_traces(&store), 0, "the store is empty");
    assert!(row_ids(&driver).is_empty(), "so is the table, at once");
    assert_eq!(driver.read(|cx| home.read(cx).data().map.nodes.len()), 0);
    assert_eq!(empty_state(&driver), Some(EMPTY_NO_DATA));
    assert_eq!(
        map_empty_message(&driver).map(|(title, _)| title),
        Some("Waiting for telemetry")
    );

    // Receiving carries on: a new trace shows by the next refresh.
    store.ingest_spans(vec![lone_span(1, store.now_unix_nano())]);
    driver.advance(Duration::from_millis(600));
    assert_eq!(row_ids(&driver).len(), 1);
}

#[test]
fn clear_all_clears_a_frozen_view_too() {
    let mut driver = home_driver(&Theme::dark());
    driver.click(toolbar_center(&driver, ToolbarPart::Pause));
    assert!(is_paused(&driver));
    driver.click(toolbar_center(&driver, ToolbarPart::ClearAll));
    assert!(row_ids(&driver).is_empty(), "the user asked: it reads");
    assert!(is_paused(&driver), "and the view stays paused");
}

fn map_empty_message(driver: &Driver<MainWindow>) -> Option<(&'static str, &'static str)> {
    let map = map_of(driver);
    driver.read(|cx| map.read(cx).empty_message(cx))
}

// --- the sidebar, the keys and Back (M4 plan, Task 7) -----------------------------------

/// Draws until what is pending (a read, its result, the frame that shows it) is
/// done.
fn settle(driver: &mut Driver<MainWindow>) {
    for _ in 0..4 {
        driver.draw();
    }
}

/// Where the sidebar entry `item` is in the window now.
fn nav_bounds(driver: &Driver<MainWindow>, item: NavItem) -> gpui::Bounds<Pixels> {
    driver
        .read_root(|main| main.nav_bounds(item))
        .unwrap_or_else(|| panic!("the sidebar has no `{}` entry on the screen", item.label()))
}

/// A click on the sidebar entry `item`, and the frames it takes to show the
/// screen.
fn click_nav(driver: &mut Driver<MainWindow>, item: NavItem) {
    driver.click(nav_bounds(driver, item).center());
    settle(driver);
}

/// A key chord (`cmd-2`), and the frames it takes to show what it did.
fn press(driver: &mut Driver<MainWindow>, chord: &str) {
    driver.key(chord);
    settle(driver);
}

const LOGS: Screen = Screen::Logs { query: None };
const RULES: Screen = Screen::Rules { draft: None };

/// What the user typed in the Logs bar in the tests below.
const LOG_QUERY: &str = "{service_name=\"payments-api\"} | level = \"error\"";

fn logs_of(driver: &Driver<MainWindow>) -> Entity<loctel_ui::app::LogsModel> {
    driver.read_root(|main| main.logs().clone())
}

fn logs_query(driver: &Driver<MainWindow>) -> String {
    let logs = logs_of(driver);
    driver.read(|cx| logs.read(cx).query_text().to_owned())
}

fn windows_open(driver: &Driver<MainWindow>) -> usize {
    driver.read(|cx| cx.windows().len())
}

#[test]
fn the_sidebar_lists_the_three_screens() {
    let driver = home_driver(&Theme::dark());
    assert_eq!(
        NavItem::ALL.map(NavItem::label),
        ["Map & traces", "Logs", "Discard rules"]
    );
    assert_eq!(
        NavItem::ALL.map(NavItem::screen),
        [Screen::Home, LOGS, RULES]
    );
    assert_eq!(
        NavItem::ALL.map(NavItem::shortcut),
        ["cmd-1", "cmd-2", "cmd-3"]
    );

    // Top to bottom under the strip the traffic lights sit in, each as high as
    // the entries always were and as wide as the sidebar less its padding.
    let mut top = INSET + TRAFFIC_LIGHT_SPACE;
    for item in NavItem::ALL {
        let bounds = nav_bounds(&driver, item);
        assert!(
            close_to(f32::from(bounds.origin.y), top, 1.5),
            "{} starts at {top}, not {:?}",
            item.label(),
            bounds.origin.y
        );
        assert!(close_to(
            f32::from(bounds.size.height),
            NAV_ITEM_HEIGHT,
            0.01
        ));
        assert!(
            f32::from(bounds.origin.x) > INSET && f32::from(bounds.right()) < INSET + SIDEBAR_WIDTH,
            "{} is inside the sidebar: {bounds:?}",
            item.label()
        );
        top = f32::from(bounds.bottom()) + NAV_ITEM_GAP;
    }
}

fn close_to(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs() <= tolerance
}

#[test]
fn nav_items_are_lit_by_their_screens() {
    let draft = Draft {
        name: "Noise".to_owned(),
        language: RuleLanguage::LogQl,
        query: "{service_name=\"web-bff\"}".to_owned(),
        scope: DropScope::Trace,
        enabled: true,
    };
    let lit = |screen: &Screen| -> Vec<&'static str> {
        NavItem::ALL
            .into_iter()
            .filter(|item| item.is_active(screen))
            .map(NavItem::label)
            .collect()
    };
    assert_eq!(lit(&Screen::Home), ["Map & traces"]);
    // A trace is reached from the map and the table, and stays under them.
    assert_eq!(lit(&Screen::Trace(failing_trace_id())), ["Map & traces"]);
    assert_eq!(lit(&LOGS), ["Logs"]);
    assert_eq!(
        lit(&Screen::Logs {
            query: Some(LOG_QUERY.to_owned())
        }),
        ["Logs"]
    );
    assert_eq!(lit(&RULES), ["Discard rules"]);
    assert_eq!(
        lit(&Screen::Rules { draft: Some(draft) }),
        ["Discard rules"]
    );
}

#[test]
fn sidebar_switches_screens_and_keeps_state() {
    let mut driver = home_driver(&Theme::dark());
    assert_eq!(screen(&mut driver), Screen::Home);

    // The home screen: a query that narrows the table to the failing checkout.
    type_query(&mut driver, "{ status = error }");
    assert_eq!(row_ids(&driver).len(), 1);

    // Logs: a query typed there, and the live tail switched off.
    click_nav(&mut driver, NavItem::Logs);
    assert_eq!(screen(&mut driver), LOGS);
    let logs = logs_of(&driver);
    driver.update_root(|_, _, cx| {
        logs.update(cx, |logs, cx| {
            logs.set_query(LOG_QUERY, cx);
            logs.set_tail(false, cx);
        });
    });
    settle(&mut driver);
    assert_eq!(logs_query(&driver), LOG_QUERY);

    // Discard rules: a new rule with a name in the editor, not saved.
    click_nav(&mut driver, NavItem::Rules);
    assert_eq!(screen(&mut driver), RULES);
    let rules = driver.read_root(|main| main.rules().clone());
    driver.update_root(|_, _, cx| {
        rules.update(cx, |rules, cx| {
            rules.new_rule(cx);
            rules.set_name("Drop the noise", cx);
        });
    });
    settle(&mut driver);

    // Back to the first screen: it is as it was left.
    click_nav(&mut driver, NavItem::MapAndTraces);
    assert_eq!(screen(&mut driver), Screen::Home);
    assert_eq!(query_text(&driver), "{ status = error }");
    assert_eq!(input_text(&driver), "{ status = error }");
    assert_eq!(row_ids(&driver).len(), 1);

    // Logs again: the query and the tail setting stayed.
    click_nav(&mut driver, NavItem::Logs);
    assert_eq!(screen(&mut driver), LOGS);
    assert_eq!(logs_query(&driver), LOG_QUERY);
    assert!(
        !driver.read(|cx| logs.read(cx).tail()),
        "the tail stays off"
    );
    assert!(
        driver.read(|cx| logs
            .read(cx)
            .rows()
            .iter()
            .all(|row| row.service == "payments-api")),
        "the list is the query's"
    );

    // And the editor is open on its draft, unsaved.
    click_nav(&mut driver, NavItem::Rules);
    assert_eq!(screen(&mut driver), RULES);
    let name = driver.read(|cx| {
        rules
            .read(cx)
            .editor()
            .map(|editor| editor.draft().name.clone())
    });
    assert_eq!(name.as_deref(), Some("Drop the noise"));

    // A click on the entry that is lit shows the same screen.
    click_nav(&mut driver, NavItem::Rules);
    assert_eq!(screen(&mut driver), RULES);
    assert!(driver.read(|cx| rules.read(cx).editor().is_some()));
}

#[test]
fn a_sidebar_click_on_a_trace_leaves_it() {
    let mut driver = trace_driver();
    click_nav(&mut driver, NavItem::Logs);
    assert_eq!(screen(&mut driver), LOGS);
    assert!(
        driver.read_root(|main| main.trace_view().is_none()),
        "the trace screen is closed"
    );
    click_nav(&mut driver, NavItem::MapAndTraces);
    assert_eq!(screen(&mut driver), Screen::Home);
}

#[test]
fn keyboard_shortcuts_switch_screens() {
    let mut driver = home_driver(&Theme::dark());

    press(&mut driver, "cmd-2");
    assert_eq!(screen(&mut driver), LOGS);
    press(&mut driver, "cmd-3");
    assert_eq!(screen(&mut driver), RULES);
    press(&mut driver, "cmd-1");
    assert_eq!(screen(&mut driver), Screen::Home);

    // From the trace screen, whose view has the keyboard.
    open_trace(&mut driver, failing_trace_id());
    press(&mut driver, "cmd-2");
    assert_eq!(screen(&mut driver), LOGS);
    // Its view is gone, and the keys still work.
    press(&mut driver, "cmd-3");
    assert_eq!(screen(&mut driver), RULES);
    press(&mut driver, "cmd-1");
    assert_eq!(screen(&mut driver), Screen::Home);

    // With the query input of the table holding the keyboard, and text in it.
    driver.click(part(&driver, TablePart::QueryInput));
    driver.type_text("{ status");
    press(&mut driver, "cmd-2");
    assert_eq!(screen(&mut driver), LOGS);
    press(&mut driver, "cmd-1");
    assert_eq!(
        input_text(&driver),
        "{ status",
        "the chord was not typed, and the text stayed"
    );

    // A click in an empty part of the window gives the keys to the window too.
    driver.click(point(px(INSET + SIDEBAR_WIDTH / 2.0), px(700.0)));
    press(&mut driver, "cmd-3");
    assert_eq!(screen(&mut driver), RULES);
}

/// The window over a store with nothing in it: a click low in the table lands on
/// the table's own empty area, not on a row.
fn empty_driver() -> Driver<MainWindow> {
    let backend = Rc::new(FixedBackend::new(Store::new(
        loctel_store::StoreConfig::default(),
        Arc::new(loctel_store::ManualClock::new(0)),
    )));
    Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    })
}

#[test]
fn shortcuts_work_after_a_click_in_a_part_of_a_screen_that_goes_away() {
    // What the real window did: a click in the table's empty area gives the
    // keyboard to a handle of the home screen; leaving the home screen takes it
    // out of the frame but not out of the app, so the window has to take the
    // keyboard back or the keys would have nowhere to be heard.
    let mut driver = empty_driver();
    driver.click(point(px(700.0), px(800.0)));
    press(&mut driver, "cmd-2");
    assert_eq!(screen(&mut driver), LOGS);
    press(&mut driver, "cmd-3");
    assert_eq!(screen(&mut driver), RULES);
    press(&mut driver, "cmd-1");
    assert_eq!(screen(&mut driver), Screen::Home);

    // The same from the query input, whose handle lives as long as the window.
    driver.click(part(&driver, TablePart::QueryInput));
    press(&mut driver, "cmd-3");
    assert_eq!(screen(&mut driver), RULES);
    press(&mut driver, "cmd-2");
    assert_eq!(screen(&mut driver), LOGS);
}

#[test]
fn the_settings_shortcut_opens_one_settings_window() {
    let mut driver = home_driver(&Theme::dark());
    assert_eq!(windows_open(&driver), 1);

    press(&mut driver, "cmd-,");
    assert_eq!(windows_open(&driver), 2, "the Settings window is open");
    assert_eq!(screen(&mut driver), Screen::Home, "and the screen stayed");

    // Asking again brings that window forward and opens no second one.
    press(&mut driver, "cmd-,");
    assert_eq!(windows_open(&driver), 2);
}

/// On the Logs screen: expands a log of the failing checkout (unless it is
/// expanded already, as a line the user left open is when they come back to the
/// screen) and clicks its `Open trace`: the trace screen, opened from the Logs
/// screen.
fn open_trace_from_logs(driver: &mut Driver<MainWindow>) {
    let logs = logs_of(driver);
    let (at, id) = driver
        .read(|cx| {
            logs.read(cx)
                .rows()
                .iter()
                .enumerate()
                .find(|(_, row)| row.trace_id == Some(failing_trace_id()))
                .map(|(at, row)| (at, row.id))
        })
        .expect("a log of the failing checkout is listed");
    let view = driver.read_root(|main| main.logs_view().clone());
    if driver.read(|cx| logs.read(cx).expanded()) != Some(id) {
        let row = driver
            .read(|cx| view.read(cx).row_center(at))
            .expect("the row is in view");
        driver.click(row);
        settle(driver);
    }
    let open = driver
        .read(|cx| view.read(cx).bounds_of(LogsPart::OpenTrace))
        .expect("the expanded line has `Open trace`")
        .center();
    driver.click(open);
    settle(driver);
}

#[test]
fn trace_back_returns_to_logs_when_opened_from_logs() {
    let mut driver = home_driver(&Theme::dark());
    click_nav(&mut driver, NavItem::Logs);
    // The user typed a query: going back must not put another one in its place.
    let logs = logs_of(&driver);
    let query = "{service_name=\"payments-api\"}";
    driver.update_root(|_, _, cx| logs.update(cx, |logs, cx| logs.set_query(query, cx)));
    settle(&mut driver);

    open_trace_from_logs(&mut driver);
    assert_eq!(screen(&mut driver), Screen::Trace(failing_trace_id()));

    driver.click(part_center(&driver, TracePart::Back));
    settle(&mut driver);
    assert_eq!(screen(&mut driver), LOGS, "Back is to the Logs screen");
    assert_eq!(logs_query(&driver), query, "with its query as it was");
    assert!(
        driver.read_root(|main| main.trace_view().is_none()),
        "and the trace screen is closed"
    );

    // Esc is the same as the button.
    open_trace_from_logs(&mut driver);
    assert_eq!(screen(&mut driver), Screen::Trace(failing_trace_id()));
    driver.key("escape");
    settle(&mut driver);
    assert_eq!(screen(&mut driver), LOGS);
    assert_eq!(logs_query(&driver), query);
}

#[test]
fn trace_back_returns_home_when_opened_from_home() {
    let mut driver = trace_driver();
    driver.click(part_center(&driver, TracePart::Back));
    settle(&mut driver);
    assert_eq!(screen(&mut driver), Screen::Home);

    // The way a trace was opened last time does not matter: from Logs it went
    // back to Logs, and from the table it goes back to the table.
    click_nav(&mut driver, NavItem::Logs);
    open_trace_from_logs(&mut driver);
    driver.click(part_center(&driver, TracePart::Back));
    settle(&mut driver);
    assert_eq!(screen(&mut driver), LOGS);
    click_nav(&mut driver, NavItem::MapAndTraces);
    driver.click(row_center(&driver, 0));
    settle(&mut driver);
    assert!(matches!(screen(&mut driver), Screen::Trace(_)));
    driver.click(part_center(&driver, TracePart::Back));
    settle(&mut driver);
    assert_eq!(screen(&mut driver), Screen::Home);
}

#[test]
fn the_back_button_says_back() {
    assert_eq!(BACK_LABEL, "Back");
}

#[test]
fn the_sidebar_icons_are_bundled() {
    use gpui::AssetSource as _;
    // Map is among Kit's default icons; the list and the funnel are not, so
    // `KitAssets` carries them on top.
    for item in NavItem::ALL {
        let path = item.icon().path();
        let loaded = KitAssets.load(&path).unwrap_or_else(|error| {
            panic!(
                "the icon of `{}` ({path}) is not bundled: {error}",
                item.label()
            )
        });
        let bytes = loaded.expect("the icon loads");
        assert!(
            std::str::from_utf8(&bytes).unwrap().contains("<svg"),
            "{path}"
        );
    }
    // And what the components draw is still there.
    for path in [
        "icons/arrow-left.svg",
        "icons/chevron-down.svg",
        "icons/copy.svg",
    ] {
        assert!(KitAssets.load(path).unwrap().is_some(), "{path}");
    }
    assert_eq!(
        NavItem::ALL.map(|item| item.icon().path().to_string()),
        ["icons/map.svg", "icons/list.svg", "icons/funnel.svg"]
    );
}

#[test]
fn show_screen_navigates_the_window_open_main_window_opened() {
    // What the popover's `open_main` does once the main window is open (M4 plan,
    // Task 9): the macOS shell holds only the window's handle, and shows a screen
    // through it.
    let mut cx = headless_app();
    let backend: Rc<dyn UiBackend> = Rc::new(FixedBackend::new(home_store()));
    let window = cx
        .update(|cx| open_main_window(Rc::clone(&backend), cx, |_| None))
        .expect("the main window opens");
    let screen = |cx: &mut gpui::HeadlessAppContext| {
        cx.update(|cx| {
            window.update(cx, |root, _, cx| {
                let main = root
                    .view()
                    .clone()
                    .downcast::<MainWindow>()
                    .expect("the root shows the main window");
                main.read(cx).screen().clone()
            })
        })
        .expect("the window is open")
    };
    assert_eq!(screen(&mut cx), Screen::Home);

    cx.update(|cx| show_screen(window.into(), Screen::Rules { draft: None }, cx))
        .expect("the rules screen shows");
    assert_eq!(screen(&mut cx), Screen::Rules { draft: None });

    let query = Some(r#"{service_name="payments-api"}"#.to_owned());
    cx.update(|cx| show_screen(window.into(), Screen::Logs { query }, cx))
        .expect("the logs screen shows");
    assert!(matches!(screen(&mut cx), Screen::Logs { .. }));

    cx.update(|cx| show_screen(window.into(), Screen::Home, cx))
        .expect("the home screen shows");
    assert_eq!(screen(&mut cx), Screen::Home);
}

#[test]
fn show_screen_refuses_a_window_that_is_not_the_main_window_or_is_gone() {
    let mut cx = headless_app();
    let backend: Rc<dyn UiBackend> = Rc::new(FixedBackend::new(home_store()));
    // The Settings window is a loctel window with another view in its `Root`.
    let settings = cx
        .update(|cx| open_settings_window(Rc::clone(&backend), cx))
        .expect("the settings window opens");
    let error = cx
        .update(|cx| show_screen(settings.into(), Screen::Home, cx))
        .expect_err("the Settings window has no screens");
    assert!(format!("{error:#}").contains("main window"), "{error:#}");

    // A window that has closed.
    cx.update(|cx| settings.update(cx, |_, window, _| window.remove_window()))
        .expect("the window closes");
    cx.run_until_parked();
    assert!(
        cx.update(|cx| show_screen(settings.into(), Screen::Home, cx))
            .is_err()
    );
}
