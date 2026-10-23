//! The Logs screen (M4 plan, Task 4): the toolbar, the LogQL bar, the level
//! toggles, the volume histogram and the list, in the real window over the fake
//! mesh. The references are `snapshots/logs_*.png`; build them with `cargo xtask
//! screenshots --update` and look at them before committing
//! (`docs/areas/ui.md`).
//!
//! The data is `CheckoutFailure`, whose seven logs (3 INF, 1 WRN, 3 ERR) sit at
//! `BASE`, and `LogFlood { records: 200 }` spread over the ten minutes before
//! them (`logs_store`), on a clock half a minute after `BASE`. So the list
//! starts with the checkout's lines, newest first, and the histogram over the
//! default 15 minutes has the flood as a run of low bars and the checkout as the
//! last one, with its three errors at the bottom.
//!
//! What the Review Focus of the plan asks of the live tail is pinned here
//! (`tail_dedupes_and_paused_tail_keeps_position`): lines that arrive faster than
//! the refresh never show twice, and a list scrolled away from the top does not
//! move while the tail is paused.

mod common;

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AppContext as _, Context, Entity, Pixels, Point, px};
use loctel_meshgen::{Generated, Scenario, generate};
use loctel_model::{AttrValue, LogRecord, Resource, Severity};
use loctel_query::logql;
use loctel_rules::RuleLanguage;
use loctel_store::{LevelSet, LogFilter, LogId, ManualClock, PageRequest, Store, StoreConfig};
use loctel_ui::Theme;
use loctel_ui::app::{
    CONTEXT_AFTER, CONTEXT_BEFORE, HISTOGRAM_BUCKETS, LogsModel, MainWindow, Screen, TAIL_REFRESH,
    TimeRange, ToolbarPart,
};
use loctel_ui::model::logs::{
    EXAMPLE_QUERY, LevelBadge, LogQueryState, MAX_ROWS, NEW_LINES_PILL, PAGE_SIZE, span_logs_query,
};
use loctel_ui::model::rules::suggest_from_log;
use loctel_ui::testing::{Driver, FixedBackend, assert_snapshot};
use loctel_ui::views::{HistogramView, LogsPart, LogsView};

use common::{
    BASE, SECOND, SEED, WINDOW, failing_trace_id, feed, part_bounds, part_center, row_named,
    selected_details, trace_model,
};

const MINUTE: u64 = 60 * SECOND;

/// The query of the tests that type one: the two errors of `payments-api`.
const QUERY: &str = "{service_name=\"payments-api\"} | level = \"error\"";

// --- the data ----------------------------------------------------------------------

/// The failing checkout (its 15 spans and 7 logs at `BASE`) and 200 flood
/// lines spread over the 10 minutes before it, on a clock 30 s after `BASE`.
fn logs_store() -> Store {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    feed(&store, &generate(&Scenario::CheckoutFailure, SEED, BASE));
    let mut flood = generate(&Scenario::LogFlood { records: 200 }, SEED, BASE);
    spread(&mut flood, BASE - 10 * MINUTE, 3 * SECOND);
    feed(&store, &flood);
    clock.advance(Duration::from_secs(30));
    store
}

/// Moves the logs of `generated` to `start`, `spacing` apart in the order of
/// their own times (the flood's own spacing is 20 us, all in one bucket of the
/// histogram). The services stay mixed as the scenario mixes them: the request
/// groups its records by service, so the order of the times, not the order in
/// the request, is the order of the lines.
fn spread(generated: &mut Generated, start: u64, spacing: u64) {
    let mut times: Vec<u64> = Vec::new();
    for batch in &generated.batches {
        for request in batch.logs.iter() {
            for resource in &request.resource_logs {
                for scope in &resource.scope_logs {
                    times.extend(scope.log_records.iter().map(|record| record.time_unix_nano));
                }
            }
        }
    }
    times.sort_unstable();
    for batch in &mut generated.batches {
        let Some(request) = &mut batch.logs else {
            continue;
        };
        for resource in &mut request.resource_logs {
            for scope in &mut resource.scope_logs {
                for record in &mut scope.log_records {
                    let rank = times.partition_point(|time| *time < record.time_unix_nano);
                    let at = start + rank as u64 * spacing;
                    record.time_unix_nano = at;
                    record.observed_time_unix_nano = at;
                }
            }
        }
    }
}

/// The window on `store`, with the first frame drawn.
fn window_on(store: Store, theme: &Theme) -> (Driver<MainWindow>, Rc<FixedBackend>) {
    let backend = Rc::new(FixedBackend::new(store));
    let shown = Rc::clone(&backend);
    let driver = Driver::new(WINDOW, theme, move |window, cx| {
        cx.new(|cx| MainWindow::new(shown, None, window, cx))
    });
    (driver, backend)
}

/// Draws until what is pending (a read, its result, the frame that shows it) is
/// done.
fn settle(driver: &mut Driver<MainWindow>) {
    for _ in 0..4 {
        driver.draw();
    }
}

/// Shows the Logs screen, and settles.
fn open_logs(driver: &mut Driver<MainWindow>, query: Option<&str>) {
    let query = query.map(str::to_owned);
    driver.update_root(|main, _, cx| main.navigate(Screen::Logs { query }, cx));
    settle(driver);
}

/// The window on the logs of [`logs_store`], on the Logs screen.
fn logs_window(theme: &Theme) -> Driver<MainWindow> {
    let (mut driver, _) = window_on(logs_store(), theme);
    open_logs(&mut driver, None);
    driver
}

// --- reaching into the window --------------------------------------------------------

fn model(driver: &Driver<MainWindow>) -> Entity<LogsModel> {
    driver.read_root(|main| main.logs().clone())
}

fn view(driver: &Driver<MainWindow>) -> Entity<LogsView> {
    driver.read_root(|main| main.logs_view().clone())
}

/// Calls a method of the logs model.
fn with_model<R>(
    driver: &mut Driver<MainWindow>,
    f: impl FnOnce(&mut LogsModel, &mut Context<LogsModel>) -> R,
) -> R {
    driver.update_root(|main, _, cx| main.logs().clone().update(cx, f))
}

fn read_model<R>(driver: &Driver<MainWindow>, f: impl FnOnce(&LogsModel) -> R) -> R {
    let model = model(driver);
    driver.read(|cx| f(model.read(cx)))
}

fn ids(driver: &Driver<MainWindow>) -> Vec<LogId> {
    read_model(driver, |logs| {
        logs.rows().iter().map(|row| row.id).collect()
    })
}

fn messages(driver: &Driver<MainWindow>) -> Vec<String> {
    read_model(driver, |logs| {
        logs.rows().iter().map(|row| row.message.clone()).collect()
    })
}

fn screen(driver: &mut Driver<MainWindow>) -> Screen {
    driver.update_root(|main, _, _| main.screen().clone())
}

fn bounds(driver: &Driver<MainWindow>, part: LogsPart) -> Option<gpui::Bounds<Pixels>> {
    let view = view(driver);
    driver.read(|cx| view.read(cx).bounds_of(part))
}

fn center(driver: &Driver<MainWindow>, part: LogsPart) -> Point<Pixels> {
    bounds(driver, part)
        .unwrap_or_else(|| panic!("the Logs screen has no {part:?} on the screen"))
        .center()
}

fn has(driver: &Driver<MainWindow>, part: LogsPart) -> bool {
    bounds(driver, part).is_some()
}

/// The middle of the row at `index` of the list, as drawn now.
fn row_center(driver: &Driver<MainWindow>, index: usize) -> Point<Pixels> {
    let view = view(driver);
    driver
        .read(|cx| view.read(cx).row_center(index))
        .unwrap_or_else(|| panic!("row {index} is not in view"))
}

fn scroll_offset(driver: &Driver<MainWindow>) -> (usize, f32) {
    let view = view(driver);
    driver.read(|cx| view.read(cx).scroll_offset())
}

/// The index of the first row whose message starts with `start`.
fn row_starting(driver: &Driver<MainWindow>, start: &str) -> usize {
    messages(driver)
        .iter()
        .position(|message| message.starts_with(start))
        .unwrap_or_else(|| panic!("no row starts with `{start}`"))
}

/// Puts `generated`'s logs in the store, at `start`, `spacing` apart.
fn add_logs(store: &Store, records: usize, start: u64, spacing: u64, seed: u64) {
    let mut generated = generate(&Scenario::LogFlood { records }, seed, start);
    spread(&mut generated, start, spacing);
    feed(store, &generated);
}

/// One log of `orders-api` at `at` with `body`, for the store.
fn one_log(store: &Store, at: u64, body: &str) {
    store.ingest_logs(vec![LogRecord {
        time_unix_nano: at,
        observed_unix_nano: at,
        severity: Severity::Info,
        severity_text: String::new(),
        body: body.to_owned(),
        attributes: Vec::new(),
        trace_id: None,
        span_id: None,
        resource: Arc::new(Resource {
            attributes: vec![(
                "service.name".to_owned(),
                AttrValue::Str("orders-api".to_owned()),
            )],
        }),
        scope_name: "test".to_owned(),
    }]);
}

// --- the references -----------------------------------------------------------------

#[test]
fn logs_dark() {
    let mut driver = logs_window(&Theme::dark());
    let image = driver.capture();
    assert_eq!(image.dimensions(), (2880, 1800));
    assert_snapshot("logs_dark", &image);
}

#[test]
fn logs_light() {
    let mut driver = logs_window(&Theme::light());
    let image = driver.capture();
    assert_eq!(image.dimensions(), (2880, 1800));
    assert_snapshot("logs_light", &image);
}

#[test]
fn logs_expanded_dark() {
    let mut driver = logs_window(&Theme::dark());
    let charge = row_starting(&driver, "Charge failed");
    driver.click(row_center(&driver, charge));
    settle(&mut driver);

    assert_eq!(
        read_model(&driver, |logs| logs.expanded()),
        Some(ids(&driver)[charge]),
        "the click expanded the line"
    );
    for part in [LogsPart::OpenTrace, LogsPart::ShowAround, LogsPart::Discard] {
        assert!(has(&driver, part), "the expanded line has {part:?}");
    }
    assert_snapshot("logs_expanded_dark", &driver.capture());
}

#[test]
fn logs_query_error_dark() {
    let mut driver = logs_window(&Theme::dark());
    driver.click(center(&driver, LogsPart::QueryInput));
    driver.type_text("{service_name=\"payments-api\"");
    settle(&mut driver);

    // The error is shown under the input, and the rows are the last good ones
    // (all of them: the bar was empty).
    let error = {
        let view = view(&driver);
        driver.read(|cx| view.read(cx).error_line(cx))
    };
    let error = error.expect("an unclosed selector is an error");
    assert!(error.starts_with("at column "), "{error}");
    assert!(matches!(
        read_model(&driver, |logs| logs.state().clone()),
        LogQueryState::Invalid { .. }
    ));
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), 207);
    assert_snapshot("logs_query_error_dark", &driver.capture());
}

#[test]
fn logs_context_dark() {
    let mut driver = logs_window(&Theme::dark());
    // The WRN line of the checkout, with lines of the flood and of the
    // checkout on both sides of it.
    let target = row_starting(&driver, "card-gateway did not answer");
    let id = ids(&driver)[target];
    with_model(&mut driver, |logs, cx| logs.show_around(id, cx));
    settle(&mut driver);

    let (context, banner) = {
        let model = model(&driver);
        let view = view(&driver);
        driver.read(|cx| (model.read(cx).context().cloned(), view.read(cx).banner(cx)))
    };
    let context = context.expect("the list is in context mode");
    assert_eq!(context.target, id);
    assert!(context.held);
    assert_eq!(
        banner.as_deref(),
        Some("Lines around 08:00:01.440"),
        "the banner names the time of the line"
    );
    assert!(has(&driver, LogsPart::BackToSearch));
    assert_snapshot("logs_context_dark", &driver.capture());
}

// --- the state of the screen -----------------------------------------------------------

#[test]
fn the_list_shows_the_newest_lines_first_and_the_histogram_the_checkout_last() {
    let driver = logs_window(&Theme::dark());
    let rows = read_model(&driver, |logs| logs.rows().to_vec());
    assert_eq!(rows.len(), 207, "7 checkout logs and 200 of the flood");
    // Newest first: the checkout's closing error, then the other six.
    assert_eq!(rows[0].time, "08:00:01.800");
    assert_eq!(rows[0].level, LevelBadge::Err);
    assert!(
        rows.windows(2).all(|pair| pair[0].id > pair[1].id),
        "strictly newest first"
    );

    let bars = read_model(&driver, |logs| logs.histogram().clone());
    assert_eq!(bars.bars.len(), HISTOGRAM_BUCKETS);
    assert_eq!(HISTOGRAM_BUCKETS, 48);
    // 48 buckets of 18.75 s end at the clock, 30 s after the checkout: the
    // checkout is in bucket 46, with the flood's last two lines (9 in all, the
    // tallest bar), and 3 of the 9 are errors. The bucket after it is empty.
    assert_eq!(bars.bars[46], (1.0, (3.0_f64 / 9.0) as f32));
    assert_eq!(bars.bars[47], (0.0, 0.0));
    assert_eq!(
        bars.labels[0], "07:45:30.000",
        "15 minutes before the clock"
    );
    assert_eq!(bars.labels[3], "08:00:30.000", "the right edge is now");
    assert_eq!(
        read_model(&driver, |logs| logs.count_line()),
        "207 lines \u{b7} newest first"
    );
}

#[test]
fn an_empty_store_says_so() {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock);
    let (mut driver, _) = window_on(store, &Theme::dark());
    open_logs(&mut driver, None);
    assert!(read_model(&driver, |logs| logs.loaded()));
    assert!(read_model(&driver, |logs| logs.rows().is_empty()));
    let view = view(&driver);
    assert_eq!(
        driver.read(|cx| view.read(cx).empty_state(cx)),
        Some(loctel_ui::views::LOGS_EMPTY_NO_DATA)
    );
}

#[test]
fn typing_a_query_filters_the_list_and_the_histogram() {
    let mut driver = logs_window(&Theme::dark());
    driver.click(center(&driver, LogsPart::QueryInput));
    driver.type_text("{service_name=\"payments-api\"} | level = \"error\"");
    settle(&mut driver);

    assert_eq!(read_model(&driver, |logs| logs.rows().len()), 2);
    assert!(
        read_model(&driver, |logs| logs.rows().iter().all(|row| row.service
            == "payments-api"
            && row.level == LevelBadge::Err)),
        "{:?}",
        messages(&driver)
    );
    // The histogram counts the same lines: two errors in one bar, all of it
    // error.
    let bars = read_model(&driver, |logs| logs.histogram().bars.clone());
    let lit: Vec<&(f32, f32)> = bars.iter().filter(|bar| bar.0 > 0.0).collect();
    assert_eq!(lit.len(), 1, "{bars:?}");
    assert_eq!(*lit[0], (1.0, 1.0));
}

#[test]
fn an_invalid_query_keeps_the_rows_it_had() {
    let mut driver = logs_window(&Theme::dark());
    driver.click(center(&driver, LogsPart::QueryInput));
    driver.type_text("{service_name=\"payments-api\"} | level = \"error\"");
    settle(&mut driver);
    let errors = ids(&driver);
    assert_eq!(errors.len(), 2, "the checkout's two payments-api errors");

    driver.type_text(" |= ");
    settle(&mut driver);
    assert!(matches!(
        read_model(&driver, |logs| logs.state().clone()),
        LogQueryState::Invalid { .. }
    ));
    assert_eq!(ids(&driver), errors, "the last good rows stay");
}

#[test]
fn the_query_bar_hints_with_a_real_query() {
    assert!(matches!(
        loctel_ui::model::logs::validate(EXAMPLE_QUERY),
        LogQueryState::Valid(_)
    ));
}

#[test]
fn level_toggles_filter() {
    let mut driver = logs_window(&Theme::dark());
    assert_eq!(read_model(&driver, |logs| logs.levels()), LevelSet::ALL);
    let count = |driver: &Driver<MainWindow>| read_model(driver, |logs| logs.rows().len());
    assert_eq!(count(&driver), 207);

    // ERR off: the three errors go.
    driver.click(center(&driver, LogsPart::Level(LevelBadge::Err)));
    settle(&mut driver);
    assert_eq!(
        read_model(&driver, |logs| logs.levels()),
        LevelSet::from_groups(false, true, true, true)
    );
    assert_eq!(count(&driver), 204);
    assert!(read_model(&driver, |logs| logs
        .rows()
        .iter()
        .all(|row| row.level != LevelBadge::Err)));

    // WRN off too: the warning goes.
    driver.click(center(&driver, LogsPart::Level(LevelBadge::Wrn)));
    settle(&mut driver);
    assert_eq!(count(&driver), 203);

    // Only ERR on: the three errors and nothing else.
    driver.click(center(&driver, LogsPart::Level(LevelBadge::Err)));
    driver.click(center(&driver, LogsPart::Level(LevelBadge::Inf)));
    driver.click(center(&driver, LogsPart::Level(LevelBadge::Dbg)));
    settle(&mut driver);
    assert_eq!(
        read_model(&driver, |logs| logs.levels()),
        LevelSet::from_groups(true, false, false, false)
    );
    assert_eq!(count(&driver), 3);
    assert!(read_model(&driver, |logs| logs
        .rows()
        .iter()
        .all(|row| row.level == LevelBadge::Err)));

    // The histogram follows the toggles: the three errors, all in error colour.
    let bars = read_model(&driver, |logs| logs.histogram().bars.clone());
    assert_eq!(bars.iter().filter(|bar| bar.0 > 0.0).count(), 1);

    // Everything off: an empty list, which says so rather than looking broken.
    driver.click(center(&driver, LogsPart::Level(LevelBadge::Err)));
    settle(&mut driver);
    assert_eq!(count(&driver), 0);
}

#[test]
fn the_range_narrows_the_list() {
    let mut driver = logs_window(&Theme::dark());
    // The flood runs from 10 minutes before the checkout; the clock is 30 s
    // after it. The last 5 minutes keep the checkout and the flood's last 5
    // minutes less 30 s, which is 90 lines of 3 s.
    with_model(&mut driver, |logs, cx| {
        logs.set_range(TimeRange::Last5m, cx)
    });
    settle(&mut driver);
    let rows = read_model(&driver, |logs| logs.rows().len());
    assert!((90..=100).contains(&(rows - 7)), "{rows}");
    assert!(rows < 207);

    with_model(&mut driver, |logs, cx| {
        logs.set_range(TimeRange::Everything, cx)
    });
    settle(&mut driver);
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), 207);
    // Everything starts the histogram at the oldest line held.
    let first = read_model(&driver, |logs| logs.histogram().labels[0].clone());
    assert_eq!(first, "07:50:00.000", "the flood's first line");
}

#[test]
fn clicking_a_row_expands_it_and_another_row_moves_the_expansion() {
    let mut driver = logs_window(&Theme::dark());
    driver.click(row_center(&driver, 1));
    settle(&mut driver);
    let second = ids(&driver)[1];
    assert_eq!(read_model(&driver, |logs| logs.expanded()), Some(second));
    let details = read_model(&driver, |logs| logs.expanded_details()).unwrap();
    assert_eq!(details[0].0, "body", "the body first");
    assert!(
        details.iter().any(|(label, _)| label == "service_name"),
        "{details:?}"
    );

    // A click on the same row folds it again.
    driver.click(row_center(&driver, 1));
    settle(&mut driver);
    assert_eq!(read_model(&driver, |logs| logs.expanded()), None);

    // A click on another row while one is open moves the expansion.
    driver.click(row_center(&driver, 1));
    settle(&mut driver);
    driver.click(row_center(&driver, 0));
    settle(&mut driver);
    assert_eq!(
        read_model(&driver, |logs| logs.expanded()),
        Some(ids(&driver)[0])
    );
}

// --- the histogram's pixels ---------------------------------------------------------------------

/// A colour as the 8 bit channels of an opaque pixel.
fn opaque(color: gpui::Rgba) -> image::Rgba<u8> {
    let byte = |channel: f32| (channel * 255.0).round() as u8;
    image::Rgba([byte(color.r), byte(color.g), byte(color.b), 255])
}

#[test]
fn the_histogram_bars_are_64_px_with_the_errors_at_the_bottom() {
    let theme = Theme::dark();
    let mut driver = logs_window(&theme);

    // The heights the next frame draws: the checkout's bar is the tallest, 64 px,
    // with the errors (3 of its 9 lines) a third of it; an empty bucket is a stub.
    let heights = {
        let histogram = driver.read_root(|main| main.logs_view().clone());
        let histogram = driver.read(|cx| histogram.read(cx).histogram_view().clone());
        driver.read(|cx| HistogramView::heights(histogram.read(cx), cx))
    };
    assert_eq!(heights.len(), 48);
    assert_eq!(heights[46].0, 64.0);
    assert!(
        (heights[46].1 - 64.0 / 3.0).abs() < 0.01,
        "{:?}",
        heights[46]
    );
    assert_eq!(heights[0], (1.0, 0.0), "an empty bucket is a 1 px stub");
    assert!(
        heights[20].0 > 2.0 && heights[20].1 == 0.0,
        "{:?}",
        heights[20]
    );

    // And the pixels agree. The bars fill the histogram's box less 16 px on each
    // side, 2 px apart, from its top.
    let image = driver.capture();
    let at = |x: f32, y: f32| *image.get_pixel((x * 2.0) as u32, (y * 2.0) as u32);
    let area = bounds(&driver, LogsPart::Histogram).expect("the histogram is drawn");
    let (left, top) = (f32::from(area.left()) + 16.0, f32::from(area.top()));
    let width = f32::from(area.size.width) - 32.0;
    let bar = (width - 47.0 * 2.0) / 48.0;
    let x = |index: usize| left + index as f32 * (bar + 2.0) + bar / 2.0;

    // The checkout's bar: error colour at the bottom, the hairline colour above
    // it, the panel colour over the top of nothing.
    assert_eq!(
        at(x(46), top + 62.0),
        opaque(theme.error),
        "errors at the bottom"
    );
    assert_eq!(
        at(x(46), top + 20.0),
        opaque(theme.border),
        "the rest above them"
    );
    // A flood bar has no errors: all hairline colour, and nothing above it.
    assert_eq!(at(x(20), top + 62.0), opaque(theme.border));
    assert_eq!(
        at(x(20), top + 2.0),
        opaque(theme.panel),
        "bars are not all 64 px"
    );
    // An empty bucket: the 1 px stub at the bottom and nothing above.
    assert_eq!(at(x(0), top + 63.5), opaque(theme.border));
    assert_eq!(at(x(0), top + 40.0), opaque(theme.panel));
}

// --- paging ------------------------------------------------------------------------------

#[test]
fn paging_loads_more_near_end() {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    feed(
        &store,
        &generate(&Scenario::LogFlood { records: 3_000 }, SEED, BASE),
    );
    clock.advance(Duration::from_secs(30));
    let (mut driver, _) = window_on(store, &Theme::dark());
    open_logs(&mut driver, None);

    assert_eq!(read_model(&driver, |logs| logs.rows().len()), PAGE_SIZE);
    assert_eq!(PAGE_SIZE, 500);
    assert!(read_model(&driver, |logs| logs.more()), "2,500 more wait");
    assert_eq!(
        read_model(&driver, |logs| logs.count_line()),
        "500+ lines \u{b7} newest first"
    );

    // Near the top nothing is read.
    driver.scroll(center(&driver, LogsPart::List), 3_000.0);
    settle(&mut driver);
    assert_eq!(
        read_model(&driver, |logs| logs.rows().len()),
        PAGE_SIZE,
        "100 rows down is not within 50 of the end"
    );

    // To the end: the next page is read.
    driver.scroll(center(&driver, LogsPart::List), 1_000_000.0);
    settle(&mut driver);
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), 2 * PAGE_SIZE);

    // Strictly newest first, no line twice, across the join of the pages.
    let ids = ids(&driver);
    assert!(ids.windows(2).all(|pair| pair[0] > pair[1]));
}

#[test]
fn paging_reads_everything_there_is_and_stops() {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    feed(
        &store,
        &generate(&Scenario::LogFlood { records: 1_200 }, SEED, BASE),
    );
    clock.advance(Duration::from_secs(30));
    let (mut driver, _) = window_on(store, &Theme::dark());
    open_logs(&mut driver, None);

    for _ in 0..6 {
        with_model(&mut driver, |logs, cx| logs.load_more(cx));
        settle(&mut driver);
    }
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), 1_200);
    assert!(!read_model(&driver, |logs| logs.more()), "nothing is left");
    assert_eq!(
        read_model(&driver, |logs| logs.count_line()),
        "1,200 lines \u{b7} newest first"
    );
    let unique: std::collections::HashSet<LogId> = ids(&driver).into_iter().collect();
    assert_eq!(unique.len(), 1_200, "no line twice");
}

#[test]
fn the_list_stops_at_20000_rows_with_a_footer() {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    feed(
        &store,
        &generate(&Scenario::LogFlood { records: 21_000 }, SEED, BASE),
    );
    clock.advance(Duration::from_secs(30));
    let (mut driver, _) = window_on(store, &Theme::dark());
    // Milestone 5: a list of more than 20,000 lines that the store can count (no
    // query, or a selector of a service and levels) is not paged but windowed
    // (`tests/logs_scale.rs`). The paged list, and its cap, are what any other
    // search gets: this regex on the service matches every line and cannot be
    // counted, so the list is the paged one over the same 21,000 lines.
    open_logs(&mut driver, Some(r#"{service_name=~".+"}"#));

    // 40 pages of 500 make 20,000; the loop asks for more than that.
    for _ in 0..45 {
        with_model(&mut driver, |logs, cx| logs.load_more(cx));
        driver.draw();
    }
    settle(&mut driver);

    assert_eq!(read_model(&driver, |logs| logs.rows().len()), MAX_ROWS);
    assert_eq!(MAX_ROWS, 20_000);
    assert!(read_model(&driver, |logs| logs.capped()));
    assert_eq!(
        read_model(&driver, |logs| logs.footer()),
        Some("Showing the newest 20,000 matching lines")
    );
    assert_eq!(
        read_model(&driver, |logs| logs.count_line()),
        "20,000+ lines"
    );
    assert!(
        has(&driver, LogsPart::Footer),
        "the footer is on the screen"
    );

    // Asking again reads nothing more.
    with_model(&mut driver, |logs, cx| logs.load_more(cx));
    settle(&mut driver);
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), MAX_ROWS);
}

// --- the live tail ------------------------------------------------------------------------

fn unique(ids: &[LogId]) -> bool {
    ids.windows(2).all(|pair| pair[0] > pair[1])
}

#[test]
fn tail_dedupes_and_paused_tail_keeps_position() {
    let store = logs_store();
    let (mut driver, _) = window_on(store.clone(), &Theme::dark());
    open_logs(&mut driver, None);
    assert!(read_model(&driver, |logs| logs.tail()), "the tail is on");
    let tick = |driver: &mut Driver<MainWindow>| {
        driver.advance(TAIL_REFRESH);
        settle(driver);
    };

    // A few lines arrive: they are prepended once.
    let mut at = BASE + 5 * SECOND;
    add_logs(&store, 30, at, 1_000, 11);
    at += SECOND;
    tick(&mut driver);
    let after_small = ids(&driver);
    assert_eq!(after_small.len(), 237, "30 new lines on top of 207");
    assert!(unique(&after_small), "newest first, no line twice");
    assert_eq!(
        read_model(&driver, |logs| logs.rows()[0].id.time_unix_nano),
        BASE + 5 * SECOND + 29_000,
        "the newest line is on top"
    );

    // Nothing new: a tick changes nothing.
    tick(&mut driver);
    assert_eq!(ids(&driver), after_small);

    // A flood: more lines than a page arrive between two refreshes. The
    // list cannot join the page to what it holds without a gap, so it starts
    // over from the page: the newest 500, once each.
    add_logs(&store, 1_200, at, 1_000, 12);
    at += SECOND;
    tick(&mut driver);
    let after_flood = ids(&driver);
    assert_eq!(after_flood.len(), PAGE_SIZE);
    assert!(unique(&after_flood));
    assert_eq!(after_flood[0].time_unix_nano, at - SECOND + 1_199_000);

    // A late line, older than the newest but inside the page: it shows once,
    // in its place.
    let late_at = after_flood[3].time_unix_nano - 500;
    one_log(&store, late_at, "a late line");
    tick(&mut driver);
    let after_late = ids(&driver);
    assert!(unique(&after_late), "no line twice, in order");
    assert_eq!(
        after_late
            .iter()
            .filter(|id| id.time_unix_nano == late_at)
            .count(),
        1,
        "the late line is in the list once"
    );
    assert!(messages(&driver).iter().any(|m| m == "a late line"));

    // Scrolling away from the top pauses the tail and shows the pill.
    assert!(!has(&driver, LogsPart::Pill), "no pill at the top");
    driver.scroll(center(&driver, LogsPart::List), 600.0);
    settle(&mut driver);
    assert!(read_model(&driver, |logs| logs.tail_paused()));
    assert!(has(&driver, LogsPart::Pill), "the pill is there");
    assert_eq!(NEW_LINES_PILL, "New lines \u{b7} Jump to newest");
    let held = ids(&driver);
    let place = scroll_offset(&driver);
    assert!(place.0 > 0, "the list moved: {place:?}");

    // Lines keep arriving, a few and then a flood, through several refreshes:
    // the rows and the scroll position stay exactly where they were.
    add_logs(&store, 100, at, 1_000, 13);
    at += SECOND;
    tick(&mut driver);
    add_logs(&store, 900, at, 1_000, 14);
    tick(&mut driver);
    tick(&mut driver);
    assert_eq!(ids(&driver), held, "a paused tail adds nothing");
    assert_eq!(scroll_offset(&driver), place, "and the list does not move");

    // The pill brings back the newest lines, at the top.
    driver.click(center(&driver, LogsPart::Pill));
    settle(&mut driver);
    assert!(!read_model(&driver, |logs| logs.tail_paused()));
    assert!(!has(&driver, LogsPart::Pill));
    assert_eq!(scroll_offset(&driver).0, 0, "back at the top");
    let resumed = ids(&driver);
    assert!(unique(&resumed));
    assert!(
        resumed[0].time_unix_nano >= at + 899_000,
        "the newest line is on top again: {:?}",
        resumed[0]
    );

    // Scrolling back to the top resumes it as well.
    driver.scroll(center(&driver, LogsPart::List), 600.0);
    settle(&mut driver);
    assert!(read_model(&driver, |logs| logs.tail_paused()));
    add_logs(&store, 10, at + 2 * SECOND, 1_000, 15);
    driver.scroll(center(&driver, LogsPart::List), -1_000_000.0);
    settle(&mut driver);
    tick(&mut driver);
    assert!(
        !read_model(&driver, |logs| logs.tail_paused()),
        "at the top"
    );
    assert!(unique(&ids(&driver)));
    assert_eq!(
        read_model(&driver, |logs| logs.rows()[0].id.time_unix_nano),
        at + 2 * SECOND + 9_000,
        "the lines that came meanwhile are on top"
    );
    assert_eq!(scroll_offset(&driver).0, 0);
}

#[test]
fn an_expanded_line_stays_open_as_the_tail_adds_lines_above_it() {
    let store = logs_store();
    let (mut driver, _) = window_on(store.clone(), &Theme::dark());
    open_logs(&mut driver, None);
    let charge = row_starting(&driver, "Charge failed");
    let id = ids(&driver)[charge];
    driver.click(row_center(&driver, charge));
    settle(&mut driver);
    assert_eq!(read_model(&driver, |logs| logs.expanded()), Some(id));

    add_logs(&store, 5, BASE + 5 * SECOND, 1_000, 61);
    driver.advance(TAIL_REFRESH);
    settle(&mut driver);

    assert_eq!(read_model(&driver, |logs| logs.rows().len()), 212);
    assert_eq!(read_model(&driver, |logs| logs.expanded()), Some(id));
    assert_eq!(
        ids(&driver).iter().position(|row| *row == id),
        Some(charge + 5),
        "it moved down by what was added"
    );
    // Still drawn open: its buttons are on the screen.
    assert!(has(&driver, LogsPart::ShowAround));
}

#[test]
fn the_tail_toggle_stops_the_list_and_the_histogram() {
    let store = logs_store();
    let (mut driver, _) = window_on(store.clone(), &Theme::dark());
    open_logs(&mut driver, None);

    // Off: the list is static, new lines do not show.
    with_model(&mut driver, |logs, cx| logs.set_tail(false, cx));
    let before = ids(&driver);
    let bars = read_model(&driver, |logs| logs.histogram().clone());
    add_logs(&store, 20, BASE + 5 * SECOND, 1_000, 21);
    for _ in 0..5 {
        driver.advance(Duration::from_secs(1));
    }
    settle(&mut driver);
    assert_eq!(ids(&driver), before);
    assert_eq!(read_model(&driver, |logs| logs.histogram().clone()), bars);
    assert!(!has(&driver, LogsPart::Pill), "no pill with the tail off");

    // On again: it catches up.
    with_model(&mut driver, |logs, cx| logs.set_tail(true, cx));
    driver.advance(TAIL_REFRESH);
    settle(&mut driver);
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), 227);
}

#[test]
fn the_histogram_is_read_again_every_two_seconds() {
    let store = logs_store();
    let (mut driver, _) = window_on(store.clone(), &Theme::dark());
    open_logs(&mut driver, None);
    let before = read_model(&driver, |logs| logs.histogram().clone());

    add_logs(&store, 300, BASE + 5 * SECOND, 20_000_000, 31);
    driver.advance(Duration::from_millis(1_000));
    settle(&mut driver);
    assert_eq!(
        read_model(&driver, |logs| logs.histogram().clone()),
        before,
        "not yet"
    );
    driver.advance(Duration::from_millis(1_100));
    settle(&mut driver);
    assert_ne!(
        read_model(&driver, |logs| logs.histogram().clone()),
        before,
        "after two seconds"
    );
}

// --- lines around ---------------------------------------------------------------------------

#[test]
fn show_lines_around_this_one_and_back() {
    let mut driver = logs_window(&Theme::dark());
    let target = row_starting(&driver, "card-gateway did not answer");
    let id = ids(&driver)[target];

    // Through the expanded line's button, as a user does.
    driver.click(row_center(&driver, target));
    settle(&mut driver);
    driver.click(center(&driver, LogsPart::ShowAround));
    settle(&mut driver);

    let context = read_model(&driver, |logs| logs.context().cloned()).expect("context mode");
    assert_eq!(context.target, id);
    let rows = read_model(&driver, |logs| logs.rows().to_vec());
    // The 7 checkout lines and the flood's last ones: 25 before the target
    // and 25 after it where the store has them (the 6 others of the
    // checkout are after it or before it).
    assert!(rows.len() <= CONTEXT_BEFORE + 1 + CONTEXT_AFTER);
    assert_eq!(CONTEXT_BEFORE, 25);
    assert_eq!(CONTEXT_AFTER, 25);
    let index = read_model(&driver, |logs| logs.context_target()).expect("the target is held");
    assert_eq!(rows[index].id, id);
    assert!(
        rows.windows(2).all(|pair| pair[0].id < pair[1].id),
        "the lines around a log read oldest first, like a file"
    );
    // The tail does not run in context mode: a new line changes nothing.
    assert!(!read_model(&driver, |logs| logs.tail_paused()));

    driver.click(center(&driver, LogsPart::BackToSearch));
    settle(&mut driver);
    assert!(read_model(&driver, |logs| logs.context().is_none()));
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), 207);
    assert!(!has(&driver, LogsPart::BackToSearch));
}

#[test]
fn lines_around_a_line_that_is_gone_say_so_and_show_what_remains() {
    // Review Focus 5. The log is evicted between the click and the read.
    let store = logs_store();
    let (mut driver, _) = window_on(store.clone(), &Theme::dark());
    open_logs(&mut driver, None);
    let target = row_starting(&driver, "card-gateway did not answer");
    let id = ids(&driver)[target];

    // Cleared and refilled with other lines: the ID is never given again.
    store.clear();
    add_logs(&store, 60, BASE, 1_000, 41);
    with_model(&mut driver, |logs, cx| logs.show_around(id, cx));
    settle(&mut driver);

    let context = read_model(&driver, |logs| logs.context().cloned()).expect("context mode");
    assert!(!context.held);
    assert_eq!(read_model(&driver, |logs| logs.context_target()), None);
    let view = view(&driver);
    assert_eq!(
        driver.read(|cx| view.read(cx).banner(cx)).as_deref(),
        Some("This line is no longer held")
    );
    assert!(
        read_model(&driver, |logs| !logs.rows().is_empty()),
        "the lines nearest to where it was are shown"
    );
    assert!(has(&driver, LogsPart::BackToSearch));
}

#[test]
fn changing_the_search_leaves_context_mode() {
    let mut driver = logs_window(&Theme::dark());
    let id = ids(&driver)[2];
    with_model(&mut driver, |logs, cx| logs.show_around(id, cx));
    settle(&mut driver);
    assert!(read_model(&driver, |logs| logs.context().is_some()));

    with_model(&mut driver, |logs, cx| {
        logs.set_levels(LevelSet::from_groups(true, false, false, false), cx)
    });
    settle(&mut driver);
    assert!(read_model(&driver, |logs| logs.context().is_none()));
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), 3);
}

// --- the toolbar ------------------------------------------------------------------------------

#[test]
fn the_toolbar_has_the_live_tail_pill_the_range_and_clear_all() {
    let mut driver = logs_window(&Theme::dark());
    let toolbar = driver.read_root(|main| main.logs_view().clone());
    let part = |driver: &Driver<MainWindow>, part: ToolbarPart| {
        let toolbar = toolbar.clone();
        driver.read(|cx| toolbar.read(cx).controls().read(cx).bounds_of(part))
    };
    for p in [
        ToolbarPart::LiveTail,
        ToolbarPart::Range,
        ToolbarPart::ClearAll,
    ] {
        assert!(part(&driver, p).is_some(), "{p:?} is in the toolbar");
    }
    assert!(
        part(&driver, ToolbarPart::Pause).is_none(),
        "no Pause: it is the home screen's"
    );
    assert_eq!(
        loctel_ui::app::toolbar_title(&Screen::Logs { query: None }),
        "Logs"
    );

    // The pill toggles the tail.
    let pill = part(&driver, ToolbarPart::LiveTail).unwrap().center();
    driver.click(pill);
    assert!(!read_model(&driver, |logs| logs.tail()));
    driver.click(pill);
    assert!(read_model(&driver, |logs| logs.tail()));

    // The range button opens the menu; a choice sets the model's range.
    driver.click(part(&driver, ToolbarPart::Range).unwrap().center());
    driver.click(
        part(&driver, ToolbarPart::RangeOption(TimeRange::Everything))
            .expect("the menu is open")
            .center(),
    );
    settle(&mut driver);
    assert_eq!(
        read_model(&driver, |logs| logs.range()),
        TimeRange::Everything
    );

    // Clear all empties the store and the list.
    driver.click(part(&driver, ToolbarPart::ClearAll).unwrap().center());
    settle(&mut driver);
    assert!(read_model(&driver, |logs| logs.rows().is_empty()));
}

// --- leaving the screen -----------------------------------------------------------------------

#[test]
fn the_logs_screen_keeps_its_query_and_rows_while_another_screen_shows() {
    let mut driver = logs_window(&Theme::dark());
    driver.click(center(&driver, LogsPart::QueryInput));
    driver.type_text(QUERY);
    settle(&mut driver);
    let rows = ids(&driver);
    assert_eq!(rows.len(), 2);

    driver.update_root(|main, _, cx| main.navigate(Screen::Home, cx));
    settle(&mut driver);
    driver.update_root(|main, _, cx| main.navigate(Screen::Logs { query: None }, cx));
    settle(&mut driver);
    assert_eq!(
        read_model(&driver, |logs| logs.query_text().to_owned()),
        QUERY
    );
    assert_eq!(ids(&driver), rows);
    let view = view(&driver);
    assert_eq!(driver.read(|cx| view.read(cx).input_text(cx)), QUERY);
}

#[test]
fn a_hidden_logs_screen_reads_nothing() {
    // The model does not read the store on its timers while another screen
    // shows: a new line is not in the rows until the screen is shown again.
    let store = logs_store();
    let (mut driver, _) = window_on(store.clone(), &Theme::dark());
    open_logs(&mut driver, None);
    driver.update_root(|main, _, cx| main.navigate(Screen::Home, cx));
    settle(&mut driver);
    add_logs(&store, 20, BASE + 5 * SECOND, 1_000, 51);
    for _ in 0..6 {
        driver.advance(Duration::from_secs(1));
    }
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), 207);

    driver.update_root(|main, _, cx| main.navigate(Screen::Logs { query: None }, cx));
    driver.advance(TAIL_REFRESH);
    settle(&mut driver);
    assert_eq!(
        read_model(&driver, |logs| logs.rows().len()),
        227,
        "caught up"
    );
}

// --- from a log to somewhere else ----------------------------------------------------------------

#[test]
fn open_trace_from_log_selects_span() {
    let mut driver = logs_window(&Theme::dark());
    let charge = row_starting(&driver, "Charge failed");
    let (trace, span) = read_model(&driver, |logs| {
        (logs.rows()[charge].trace_id, logs.rows()[charge].span_id)
    });
    assert_eq!(trace, Some(failing_trace_id()));
    assert!(span.is_some());

    driver.click(row_center(&driver, charge));
    settle(&mut driver);
    driver.click(center(&driver, LogsPart::OpenTrace));
    for _ in 0..4 {
        driver.draw();
    }

    assert_eq!(screen(&mut driver), Screen::Trace(failing_trace_id()));
    // Not the span the trace opens on (the failed-in `POST` to card-gateway):
    // the span the log was emitted in.
    let selected = trace_model(&driver);
    assert_eq!(
        driver.read(|cx| selected.read(cx).selected()),
        span,
        "the log's span is selected"
    );
    assert_eq!(selected_details(&driver).unwrap().name, "POST /charges");
    assert!(row_named(&driver, "POST /charges") > 0);
}

#[test]
fn the_trace_link_of_a_row_opens_the_trace_too() {
    let mut driver = logs_window(&Theme::dark());
    let first = 0;
    assert_eq!(
        read_model(&driver, |logs| logs.rows()[first].trace_short.clone()),
        Some(failing_trace_id().to_string()[..8].to_owned())
    );
    let view = view(&driver);
    let place = driver
        .read(|cx| view.read(cx).trace_link_center(first))
        .expect("the row has a trace link");
    driver.click(place);
    for _ in 0..4 {
        driver.draw();
    }
    assert_eq!(screen(&mut driver), Screen::Trace(failing_trace_id()));
    assert_eq!(
        read_model(&driver, |logs| logs.expanded()),
        None,
        "the click on the link did not also expand the row"
    );
}

#[test]
fn discard_like_this_opens_editor_with_suggestion() {
    let mut driver = logs_window(&Theme::dark());
    let charge = row_starting(&driver, "Charge failed");
    driver.click(row_center(&driver, charge));
    settle(&mut driver);
    let record = read_model(&driver, |logs| logs.expanded_record().cloned())
        .expect("the expanded line's record");
    let expected = suggest_from_log(&record);
    assert_eq!(expected.language, RuleLanguage::LogQl);

    driver.click(center(&driver, LogsPart::Discard));
    settle(&mut driver);

    // The rules screen with the editor open on the suggestion, unsaved.
    let shown = screen(&mut driver);
    let Screen::Rules { draft: Some(draft) } = shown else {
        panic!("expected the rules screen with a draft, got {shown:?}");
    };
    assert_eq!(draft, expected);
    assert!(
        draft
            .name
            .starts_with("Lines like \"Charge failed for order 8812"),
        "{}",
        draft.name
    );
    assert!(
        draft
            .query
            .starts_with("{service_name=\"payments-api\"} |= \"Charge failed for order 8812"),
        "{}",
        draft.query
    );
}

#[test]
fn open_in_logs_from_trace_view_sets_query() {
    let (mut driver, _) = window_on(logs_store(), &Theme::dark());
    common::open_trace(&mut driver, failing_trace_id());
    // Select `POST /charges`, whose four logs are the point.
    let row = row_named(&driver, "POST /charges");
    driver.click(common::row_center(&driver, row));
    let (service, span) = {
        let model = trace_model(&driver);
        driver.read(|cx| {
            let model = model.read(cx);
            (
                model.details().unwrap().service.clone(),
                model.selected().unwrap(),
            )
        })
    };
    assert_eq!(service, "payments-api");
    assert!(
        part_bounds(&driver, loctel_ui::views::TracePart::OpenInLogs).is_some(),
        "the span-logs header has the button"
    );

    driver.click(part_center(
        &driver,
        loctel_ui::views::TracePart::OpenInLogs,
    ));
    settle(&mut driver);

    let query = span_logs_query(&service, span);
    assert_eq!(
        screen(&mut driver),
        Screen::Logs {
            query: Some(query.clone())
        }
    );
    assert_eq!(
        read_model(&driver, |logs| logs.query_text().to_owned()),
        query
    );
    let view = view(&driver);
    assert_eq!(
        driver.read(|cx| view.read(cx).input_text(cx)),
        query,
        "the bar shows it"
    );
    // And the list is the span's four logs.
    assert!(matches!(
        read_model(&driver, |logs| logs.state().clone()),
        LogQueryState::Valid(_)
    ));
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), 4);
    assert!(logql::parse(&query).is_ok());
}

#[test]
fn navigating_to_logs_with_a_query_applies_it_each_time() {
    let mut driver = logs_window(&Theme::dark());
    open_logs(&mut driver, Some(QUERY));
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), 2);
    // The same screen value again, after the user typed something else.
    driver.click(center(&driver, LogsPart::QueryInput));
    driver.type_text(" |= \"zzzz\"");
    settle(&mut driver);
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), 0);
    open_logs(&mut driver, Some(QUERY));
    assert_eq!(
        read_model(&driver, |logs| logs.rows().len()),
        2,
        "the query asked for is the one shown"
    );
}

#[test]
fn the_logs_screen_fits_the_smallest_window() {
    let mut driver = logs_window(&Theme::dark());
    driver.resize((1000, 640));
    settle(&mut driver);
    for part in [
        LogsPart::QueryInput,
        LogsPart::Run,
        LogsPart::Level(LevelBadge::Err),
        LogsPart::Level(LevelBadge::Dbg),
        LogsPart::Histogram,
        LogsPart::List,
    ] {
        let at = bounds(&driver, part).unwrap_or_else(|| panic!("{part:?} is not drawn"));
        assert!(
            at.right() <= px(1000.0) && at.bottom() <= px(640.0),
            "{part:?} is inside the window: {at:?}"
        );
    }
    let list = bounds(&driver, LogsPart::List).unwrap();
    assert!(list.size.height > px(150.0), "the list has room: {list:?}");
}

#[test]
fn the_page_of_the_first_read_is_what_the_store_gives() {
    // The model's first page is the store's: the same lines in the same order.
    let store = logs_store();
    let (mut driver, _) = window_on(store.clone(), &Theme::dark());
    open_logs(&mut driver, None);
    let page = store.search_logs_keyed(
        &LogFilter {
            since_unix_nano: Some(store.now_unix_nano() - 15 * MINUTE),
            ..LogFilter::default()
        },
        &PageRequest {
            limit: PAGE_SIZE,
            cursor: None,
        },
    );
    let want: Vec<LogId> = page.items.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids(&driver), want);
    let severities: Vec<Severity> = page
        .items
        .iter()
        .map(|(_, record): &(LogId, LogRecord)| record.severity)
        .collect();
    assert_eq!(
        severities.iter().filter(|s| **s >= Severity::Error).count(),
        3
    );
}
