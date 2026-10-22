//! The service map's interactions (M3 plan, Task 6): clicking a service writes
//! its filter into the query bar, a drag pans, ctrl or cmd plus the wheel and a
//! pinch zoom (0.5 to 2.0), and `Fit` puts the map back.
//!
//! They drive the real window (`testing::Driver`) over the home screen's data
//! (`common::home_store`: the failing and the successful checkout and a health
//! check of each service), so a click goes through GPUI's own hit testing, the
//! map's painted frame and the `HomeModel`. The places to click come from the
//! map itself (`MapView::node_center`: the layout's node, through the camera
//! of the latest frame).

mod common;

use std::rc::Rc;

use gpui::{AppContext as _, Entity, Modifiers, Pixels, Point, point, px};
use loctel_ui::Theme;
use loctel_ui::app::{HomeModel, MainWindow};
use loctel_ui::model::query_bar::service_filter;
use loctel_ui::testing::{Driver, FixedBackend, assert_snapshot};
use loctel_ui::views::{Camera, MAX_ZOOM, MIN_ZOOM, MapView};

use common::{WINDOW, home_store};

const PAYMENTS: &str = "payments-api";

/// The main window over the home data, with its first frame drawn.
fn home_driver(theme: &Theme) -> Driver<MainWindow> {
    let backend = Rc::new(FixedBackend::new(home_store()));
    Driver::new(WINDOW, theme, move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    })
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
