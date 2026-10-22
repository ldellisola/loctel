//! What the window tests of the main window share: the store the home screen
//! is shown over, loaded from the fixed fake mesh.

use std::sync::Arc;
use std::time::Duration;

use gpui::{Bounds, Entity, Pixels, Point};
use loctel_ingest::{logs_from_request, spans_from_request};
use loctel_meshgen::{Generated, Scenario, generate};
use loctel_model::TraceId;
use loctel_store::{ManualClock, Store, StoreConfig};
use loctel_ui::app::{HomeControls, MainWindow, Screen, ToolbarPart, TraceModel};
use loctel_ui::model::inspector::SpanDetails;
use loctel_ui::testing::Driver;
use loctel_ui::views::{TracePart, TraceView, WaterfallView};

/// 2027-01-15T08:00:00Z. Where the failing checkout starts.
pub const BASE: u64 = 1_800_000_000_000_000_000;
pub const SECOND: u64 = 1_000_000_000;
pub const SEED: u64 = 7;

/// The size the plan fixes for the window, in logical pixels.
#[allow(dead_code)] // the Settings window has a size of its own
pub const WINDOW: (u32, u32) = (1440, 900);

/// Puts a generated scenario into `store`, whole.
pub fn feed(store: &Store, generated: &Generated) {
    for batch in &generated.batches {
        if let Some(request) = &batch.traces {
            let converted = spans_from_request(request.clone());
            assert_eq!(converted.rejected, 0, "generated spans are valid");
            store.ingest_spans(converted.items);
        }
        if let Some(request) = &batch.logs {
            let converted = logs_from_request(request.clone());
            assert_eq!(converted.rejected, 0, "generated logs are valid");
            store.ingest_logs(converted.items);
        }
    }
}

/// The home screen's data: the failing checkout (at `BASE`), the successful
/// one five seconds later and one health check of each service ten seconds
/// after the first. The store's clock reads `BASE` while they arrive and a
/// minute and ten seconds later when this returns, so every trace is complete
/// (the quiet period is 10 s: until then a trace's `card-gateway` is not a node
/// of the map) and inside the default range of 15 minutes.
#[allow(dead_code)] // not every test file shows the home screen
pub fn home_store() -> Store {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    for (scenario, at) in [
        (Scenario::CheckoutFailure, BASE),
        (Scenario::CheckoutSuccess, BASE + 5 * SECOND),
        (Scenario::HealthChecks { count: 1 }, BASE + 10 * SECOND),
    ] {
        feed(&store, &generate(&scenario, SEED, at));
    }
    clock.advance(Duration::from_secs(70));
    store
}

/// The ID of the failing checkout of [`home_store`] (the meshgen scenario is
/// deterministic, so the ID is too).
#[allow(dead_code)] // not every test file opens a trace
pub fn failing_trace_id() -> TraceId {
    generate(&Scenario::CheckoutFailure, SEED, BASE).trace_ids[0]
}

/// Shows the trace screen of `id` in the window and draws until it has read
/// the trace: the screen is made by the first frame after the navigation, its
/// read comes back by the next, and the frame after that draws the rows.
#[allow(dead_code)] // not every test file opens a trace
pub fn open_trace(driver: &mut Driver<MainWindow>, id: TraceId) {
    driver.update_root(|main, _, cx| main.navigate(Screen::Trace(id), cx));
    for _ in 0..3 {
        driver.draw();
    }
}

/// The trace screen of the window (it must be open and drawn once).
#[allow(dead_code)]
pub fn trace_view(driver: &Driver<MainWindow>) -> Entity<TraceView> {
    driver.read_root(|main| main.trace_view().cloned().expect("a trace screen is open"))
}

/// The live data of the open trace.
#[allow(dead_code)]
pub fn trace_model(driver: &Driver<MainWindow>) -> Entity<TraceModel> {
    let view = trace_view(driver);
    driver.read(|cx| view.read(cx).model().clone())
}

/// The waterfall of the open trace.
#[allow(dead_code)]
pub fn waterfall_view(driver: &Driver<MainWindow>) -> Entity<WaterfallView> {
    let view = trace_view(driver);
    driver.read(|cx| view.read(cx).waterfall_view().clone())
}

/// The names of the rows of the waterfall, top to bottom.
#[allow(dead_code)]
pub fn row_names(driver: &Driver<MainWindow>) -> Vec<String> {
    let model = trace_model(driver);
    driver.read(|cx| {
        model
            .read(cx)
            .waterfall()
            .expect("the trace is loaded")
            .rows()
            .iter()
            .map(|row| row.name.clone())
            .collect()
    })
}

/// The position among the rows of the first row named `name`.
#[allow(dead_code)]
pub fn row_named(driver: &Driver<MainWindow>, name: &str) -> usize {
    row_names(driver)
        .iter()
        .position(|row| row == name)
        .unwrap_or_else(|| panic!("no row named `{name}`"))
}

/// What the inspector shows: the details of the selected span.
#[allow(dead_code)]
pub fn selected_details(driver: &Driver<MainWindow>) -> Option<SpanDetails> {
    let model = trace_model(driver);
    driver.read(|cx| model.read(cx).details().cloned())
}

/// Where `part` of the trace screen is in the window now.
#[allow(dead_code)]
pub fn part_bounds(driver: &Driver<MainWindow>, part: TracePart) -> Option<Bounds<Pixels>> {
    let view = trace_view(driver);
    driver.read(|cx| view.read(cx).bounds_of(part, cx))
}

/// The middle of `part` of the trace screen.
#[allow(dead_code)]
pub fn part_center(driver: &Driver<MainWindow>, part: TracePart) -> Point<Pixels> {
    part_bounds(driver, part)
        .unwrap_or_else(|| panic!("the trace screen has no {part:?} on the screen"))
        .center()
}

/// The middle of the row at `row`, as drawn now.
#[allow(dead_code)]
pub fn row_center(driver: &Driver<MainWindow>, row: usize) -> Point<Pixels> {
    let waterfall = waterfall_view(driver);
    driver
        .read(|cx| waterfall.read(cx).row_center(row))
        .unwrap_or_else(|| panic!("row {row} is not in view"))
}

/// The controls of the home toolbar.
#[allow(dead_code)]
pub fn toolbar(driver: &Driver<MainWindow>) -> Entity<HomeControls> {
    driver.read_root(|main| main.toolbar().clone())
}

/// Where `part` of the home toolbar is in the window now, `None` while it is
/// not on the screen (an entry of the range menu while the menu is closed).
#[allow(dead_code)]
pub fn toolbar_bounds(driver: &Driver<MainWindow>, part: ToolbarPart) -> Option<Bounds<Pixels>> {
    let controls = toolbar(driver);
    driver.read(|cx| controls.read(cx).bounds_of(part))
}

/// The middle of `part` of the home toolbar.
#[allow(dead_code)]
pub fn toolbar_center(driver: &Driver<MainWindow>, part: ToolbarPart) -> Point<Pixels> {
    toolbar_bounds(driver, part)
        .unwrap_or_else(|| panic!("the toolbar has no {part:?} on the screen"))
        .center()
}
