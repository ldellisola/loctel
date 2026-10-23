//! The Logs list at ten million lines (Milestone 5, Task 7): the pure window
//! over rows (`model::window::RowWindow`), the list that is read by position
//! through it when more lines match than a page-loaded list holds, the way it
//! keeps its place under the live tail, and the trace view that learns its trace
//! was removed by the disk cap.
//!
//! The data is the bulk logs of `loctel-meshgen` (the ones the performance budgets
//! read): 20 us apart, five services, four levels. The big test builds ten million
//! of them in a spilling store (about 25 s in a debug build: the store crate is
//! built with optimisations in the dev profile, see decision 0051) and looks at
//! the middle of the list; the others use 25,000, which is more than the 20,000 a
//! paged list holds, in a store of many small chunks.

mod common;

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::sync_channel;
use std::thread;
use std::time::Duration;

use gpui::{AppContext as _, Context, Entity, Pixels, Point};
use loctel_ingest::logs_from_request;
use loctel_meshgen::{BULK_START_UNIX_NANO, BulkLogs, Scenario, bulk_logs, generate};
use loctel_model::{LogRecord, Severity};
use loctel_store::{Clock, LevelSet, LogId, ManualClock, Store, StoreConfig};
use loctel_ui::Theme;
use loctel_ui::app::{DONE_REFRESH, LoadState, LogsModel, MainWindow, Screen, TAIL_REFRESH};
use loctel_ui::model::logs::{
    IndexedFilter, MAX_ROWS, PAGE_SIZE, ROW_PX, WINDOWED_ABOVE, indexed_filter, total_line,
};
use loctel_ui::model::window::{Anchor, MIN_THUMB_PX, RowWindow};
use loctel_ui::testing::{Driver, FixedBackend, assert_snapshot};
use loctel_ui::views::{LogsPart, LogsView, NO_LONGER_HELD, TracePart};

use common::{BASE, SECOND, SEED, WINDOW, failing_trace_id, feed, open_trace, part_center};

/// The spacing of the bulk logs, and the time of the first one.
const SPACING: u64 = 20_000;
const START: u64 = BULK_START_UNIX_NANO;

// --- the pure window --------------------------------------------------------------------

fn id(time: u64) -> LogId {
    LogId {
        time_unix_nano: time,
        seq: 0,
    }
}

#[test]
fn row_window_math() {
    // A list of 1,000 rows of 30 px in a viewport of 300 px: ten rows fit.
    let mut window = RowWindow::new(1_000, 30.0, 300.0);
    assert_eq!((window.total(), window.top()), (1_000, 0));
    assert_eq!(window.page_rows(), 10);
    assert_eq!(window.max_top(), 990);
    assert_eq!(window.visible(), 0..10);

    // Scrolling: a part of a row is kept, so ten pixels three times is a row.
    window.scroll_px(10.0);
    window.scroll_px(10.0);
    assert_eq!(window.top(), 0, "20 px is not a row yet");
    window.scroll_px(10.0);
    assert_eq!(window.top(), 1, "30 px is");
    window.scroll_px(65.0);
    assert_eq!(window.top(), 3, "65 px is two rows and five pixels over");
    window.scroll_px(25.0);
    assert_eq!(window.top(), 4, "the five pixels count: 30 px");
    assert_eq!(window.visible(), 4..14);
    window.scroll_px(-30.0);
    assert_eq!(window.top(), 3, "up is the other way");
    // At the top the remainder of a turn is dropped, not kept for later.
    window.scroll_px(-10_000.0);
    assert_eq!(window.top(), 0);
    window.scroll_px(10.0);
    assert_eq!(window.top(), 0, "no 'owed' pixels after hitting the top");
    window.scroll_px(20.0);
    assert_eq!(window.top(), 1);
    // At the end the last row is at the bottom of the viewport.
    window.scroll_px(1.0e9);
    assert_eq!(window.top(), 990);
    assert_eq!(window.visible(), 990..1_000);
    window.scroll_rows(-5);
    assert_eq!(window.top(), 985);

    // The scrollbar's jump.
    window.jump_fraction(0.5);
    assert_eq!(window.top(), 495);
    assert!((window.fraction() - 0.5).abs() < 1e-12);
    window.jump_fraction(1.0);
    assert_eq!(window.top(), 990);
    window.jump_fraction(-3.0);
    assert_eq!(window.top(), 0, "below 0 is the top");
    window.jump_fraction(7.0);
    assert_eq!(window.top(), 990, "past 1 is the end");
    window.jump_fraction(f64::NAN);
    assert_eq!(window.top(), 0, "not a number is the top");

    // The thumb: ten rows of a thousand is 1%, which is under 24 px of a 300 px
    // track (8%), so it is 24 px.
    let (start, length) = window.thumb();
    assert_eq!(start, 0.0);
    assert!((length * 300.0 - MIN_THUMB_PX).abs() < 0.01, "{length}");
    window.jump_fraction(1.0);
    let (start, length) = window.thumb();
    assert!(
        (start + length - 1.0).abs() < 1e-6,
        "at the end it is at the bottom"
    );
    window.jump_fraction(0.5);
    let (start, length) = window.thumb();
    assert!((start - 0.5 * (1.0 - length)).abs() < 1e-6);
    // The pointer: a press at the middle of the track puts the thumb's middle
    // there, and at the ends the thumb is at the ends.
    assert!((window.fraction_for_thumb_centre(0.5) - 0.5).abs() < 1e-9);
    assert_eq!(window.fraction_for_thumb_centre(0.0), 0.0);
    assert_eq!(window.fraction_for_thumb_centre(1.0), 1.0);
    // A list that fits has one thumb the size of the track.
    let small = RowWindow::new(8, 30.0, 300.0);
    assert_eq!(small.thumb(), (0.0, 1.0));
    assert_eq!(small.max_top(), 0);
    assert_eq!(small.visible(), 0..8);
    // A list a little longer than the viewport has a long thumb, not the least.
    let mut short = RowWindow::new(15, 30.0, 300.0);
    let (start, length) = short.thumb();
    assert_eq!(start, 0.0);
    assert!((length - 10.0 / 15.0).abs() < 1e-6);
    short.jump_fraction(1.0);
    let (start, length) = short.thumb();
    assert!((start - (1.0 - length)).abs() < 1e-6);

    // A viewport that is not there yet still holds a row; a taller one shows
    // more rows and never leaves the top past the end.
    let mut window = RowWindow::new(100, 30.0, 0.0);
    assert_eq!(window.page_rows(), 1);
    assert_eq!(window.visible(), 0..1);
    window.jump_fraction(1.0);
    assert_eq!(window.top(), 99);
    window.set_viewport_px(600.0);
    assert_eq!(window.page_rows(), 20);
    assert_eq!(window.top(), 80, "the top stays inside the list");
    // A viewport of a part of a row draws that row too.
    let part = RowWindow::new(100, 30.0, 310.0);
    assert_eq!(part.page_rows(), 10);
    assert_eq!(part.drawn_rows(), 11);
    assert_eq!(part.visible(), 0..11);
    assert_eq!(ROW_PX, 30.0);
}

#[test]
fn row_window_totals_that_grow_keep_what_the_anchor_says() {
    let new = || {
        let mut window = RowWindow::new(1_000, 30.0, 300.0);
        window.jump_fraction(0.5);
        assert_eq!(window.top(), 495);
        window
    };

    // Top: the newest rows stay in view, whatever came.
    let mut window = new();
    window.set_total(1_100, Anchor::Top);
    assert_eq!((window.total(), window.top()), (1_100, 0));

    // Row: the list is newest first, so 100 new rows are above the row, and the
    // window moves down by as many.
    let mut window = new();
    window.set_total(1_100, Anchor::Row(id(5)));
    assert_eq!(window.top(), 595);
    // A total that shrank lost its oldest rows, which are below: no move.
    let mut window = new();
    window.set_total(900, Anchor::Row(id(5)));
    assert_eq!(window.top(), 495);
    // And never past the end.
    let mut window = new();
    window.set_total(300, Anchor::Row(id(5)));
    assert_eq!(window.top(), 290);

    // Fraction: the same share of the way down.
    let mut window = new();
    window.set_total(1_100, Anchor::Fraction);
    assert_eq!(window.top(), 545, "half of 1,090");
    let mut window = new();
    window.set_total(110, Anchor::Fraction);
    assert_eq!(window.top(), 50, "half of 100");

    // set_top puts it where a read found the row.
    let mut window = new();
    window.set_total(1_100, Anchor::Row(id(5)));
    window.set_top(598);
    assert_eq!(window.top(), 598);
    window.set_top(5_000);
    assert_eq!(window.top(), 1_090);

    // An empty list.
    let mut window = RowWindow::new(0, 30.0, 300.0);
    assert_eq!(window.visible(), 0..0);
    window.scroll_px(300.0);
    window.jump_fraction(0.7);
    assert_eq!(window.top(), 0);
    assert_eq!(window.thumb(), (0.0, 1.0));
}

#[test]
fn row_window_has_no_f32_limits_at_ten_billion_rows() {
    const TOTAL: u64 = 10_000_000_000;
    let mut window = RowWindow::new(TOTAL, 30.0, 900.0);
    assert_eq!(window.page_rows(), 30);
    assert_eq!(window.max_top(), TOTAL - 30);

    // An f32 cannot tell 5,000,000,000 from its neighbours (it steps by 512
    // there): the window can.
    window.jump_fraction(0.5);
    assert_eq!(window.top(), (TOTAL - 30) / 2);
    let middle = window.top();
    window.scroll_px(30.0);
    assert_eq!(window.top(), middle + 1, "one row, exactly");
    window.scroll_px(-30.0);
    assert_eq!(window.top(), middle);
    for _ in 0..3 {
        window.scroll_px(10.0);
    }
    assert_eq!(window.top(), middle + 1, "30 px in three turns");
    let visible = window.visible();
    assert_eq!(visible.end - visible.start, 30);
    assert_eq!(visible.start, middle + 1);

    // The ends are exact too, and the thumb is the least a thumb is.
    window.jump_fraction(1.0);
    assert_eq!(window.top(), TOTAL - 30);
    assert_eq!(window.visible().end, TOTAL);
    window.scroll_px(f32::MAX);
    assert_eq!(window.top(), TOTAL - 30);
    let (start, length) = window.thumb();
    assert!((length * 900.0 - MIN_THUMB_PX).abs() < 0.01);
    assert!((start + length - 1.0).abs() < 1e-6);
    // Totals that grow keep a row exactly, a row of ten billion.
    window.jump_fraction(0.25);
    let before = window.top();
    window.set_total(TOTAL + 7, Anchor::Row(id(1)));
    assert_eq!(window.top(), before + 7);
}

// --- the part of a search the store counts --------------------------------------------------

fn toggles(err: bool, wrn: bool, inf: bool, dbg: bool) -> LevelSet {
    LevelSet::from_groups(err, wrn, inf, dbg)
}

/// The levels of exactly these severities.
fn levels(severities: &[Severity]) -> LevelSet {
    severities
        .iter()
        .fold(LevelSet::default(), |set, severity| set.with(*severity))
}

fn parsed(text: &str) -> loctel_query::logql::LogQuery {
    loctel_query::logql::parse(text).unwrap_or_else(|error| panic!("`{text}`: {error:?}"))
}

#[test]
fn a_search_is_indexed_when_it_is_a_service_and_levels() {
    let all = LevelSet::ALL;
    // No query: the toggles alone.
    assert_eq!(
        indexed_filter(None, all),
        Some(IndexedFilter {
            service: None,
            levels: all
        })
    );
    assert_eq!(
        indexed_filter(None, toggles(true, false, false, false))
            .unwrap()
            .levels,
        toggles(true, false, false, false)
    );

    // A service selector.
    let query = parsed(r#"{service_name="payments-api"}"#);
    let spec = indexed_filter(Some(&query), all).unwrap();
    assert_eq!(spec.service.as_deref(), Some("payments-api"));
    assert_eq!(spec.levels, all);

    // Levels, any way they are written, narrow the toggles. They are the level
    // labels of LogQL, one per severity: `error` is not `fatal`, though the ERR
    // toggle holds both.
    let query = parsed(r#"{service_name="payments-api", level="error"}"#);
    let spec = indexed_filter(Some(&query), all).unwrap();
    assert_eq!(spec.levels, levels(&[Severity::Error]));
    let query = parsed(r#"{level=~"warn|error"}"#);
    let spec = indexed_filter(Some(&query), all).unwrap();
    assert_eq!(spec.service, None);
    assert_eq!(spec.levels, levels(&[Severity::Warn, Severity::Error]));
    let query = parsed(r#"{level!="debug", level!="trace"}"#);
    let spec = indexed_filter(Some(&query), all).unwrap();
    assert_eq!(
        spec.levels,
        levels(&[
            Severity::Unspecified,
            Severity::Info,
            Severity::Warn,
            Severity::Error,
            Severity::Fatal
        ])
    );
    let query = parsed(r#"{level=~"error|fatal"}"#);
    let spec = indexed_filter(Some(&query), all).unwrap();
    assert_eq!(spec.levels, toggles(true, false, false, false));
    // ...and the toggles narrow them: the lines have to pass both.
    let query = parsed(r#"{level=~"warn|error"}"#);
    let spec = indexed_filter(Some(&query), toggles(false, true, true, true)).unwrap();
    assert_eq!(spec.levels, levels(&[Severity::Warn]));
    // `unknown` is the level label of a line with no severity.
    let query = parsed(r#"{level="unknown"}"#);
    let spec = indexed_filter(Some(&query), all).unwrap();
    assert_eq!(spec.levels, levels(&[Severity::Unspecified]));
    // A level that is none of the seven keeps nothing.
    let query = parsed(r#"{level="loud"}"#);
    let spec = indexed_filter(Some(&query), all).unwrap();
    assert_eq!(spec.levels, LevelSet::default());

    // What the store cannot count: anything after the selector, a service that is
    // not one name, two services, any other label.
    for text in [
        r#"{service_name="a"} |= "x""#,
        r#"{service_name="a"} | json"#,
        r#"{service_name="a"} | level = "error""#,
        r#"{service_name=~"a.*"}"#,
        r#"{service_name!="a"}"#,
        r#"{service_name="a", service_name="b"}"#,
        r#"{scope_name="x"}"#,
        r#"{service_name="a", host_name="h"}"#,
    ] {
        assert_eq!(
            indexed_filter(Some(&parsed(text)), all),
            None,
            "`{text}` is not indexed"
        );
    }
    // The same service twice is one service.
    let same = parsed(r#"{service_name="a", service_name="a"}"#);
    assert!(indexed_filter(Some(&same), all).is_some());

    // The store's filter for it: the window, the service and the levels only.
    let spec = IndexedFilter {
        service: Some("a".to_owned()),
        levels: toggles(true, true, false, false),
    };
    let filter = spec.filter(Some(5), Some(9));
    assert!(filter.is_indexed());
    assert_eq!(filter.since_unix_nano, Some(5));
    assert_eq!(filter.until_unix_nano, Some(9));
    assert_eq!(filter.service.as_deref(), Some("a"));
    assert_eq!(filter.levels, Some(toggles(true, true, false, false)));
    let none = IndexedFilter {
        service: None,
        levels: all,
    }
    .filter(None, None);
    assert_eq!(none.levels, None, "every level is no restriction");

    assert_eq!(
        total_line(10_000_000),
        "10,000,000 lines \u{b7} newest first"
    );
    assert_eq!(total_line(1), "1 line \u{b7} newest first");
    assert_eq!(WINDOWED_ABOVE, 20_000);
}

// --- the data -------------------------------------------------------------------------------

/// A store that spills, with the log limit of the app's (a store with a run
/// directory keeps 20,000,000 logs, decision 0053: what the engine passes, so that
/// the ten million lines of these tests are ones the app holds) and the other
/// limits raised so that it holds everything the tests put in, and `chunk` logs to a
/// chunk of its log segments.
fn store_config(dir: &Path, chunk: usize) -> StoreConfig {
    StoreConfig {
        spill_dir: Some(dir.to_path_buf()),
        max_traces: 1_000_000,
        max_log_records: StoreConfig::SPILLING_MAX_LOG_RECORDS,
        disk_cap_bytes: 64 * 1024 * 1024 * 1024,
        log_chunk_max: chunk,
        log_block_rows: 512,
        ..StoreConfig::default()
    }
}

/// Makes the batches of `logs` into records on a thread of its own and hands them
/// to `consume` on this one: the data of ten million logs is most of a test's
/// time, and the store takes the last batch while the next is made.
fn pipelined(logs: BulkLogs, mut consume: impl FnMut(Vec<LogRecord>)) {
    thread::scope(|scope| {
        let (send, receive) = sync_channel::<Vec<LogRecord>>(8);
        scope.spawn(move || {
            for batch in logs {
                let converted = logs_from_request(batch.logs.expect("a logs batch"));
                assert_eq!(converted.rejected, 0, "generated logs are valid");
                if send.send(converted.items).is_err() {
                    break;
                }
            }
        });
        for records in receive {
            consume(records);
        }
    });
}

/// Puts `logs` in `store`, the clock following their times.
fn feed_bulk(store: &Store, clock: &ManualClock, logs: BulkLogs) {
    pipelined(logs, |records| {
        if let Some(latest) = records.iter().map(|log| log.time_unix_nano).max()
            && latest > clock.now_unix_nano()
        {
            clock.set(latest);
        }
        store.ingest_logs(records);
    });
}

/// The bulk logs of a store that is being added to: each call to `more` adds
/// lines newer than all it added before, with a seed of its own.
struct Feed {
    store: Store,
    clock: Arc<ManualClock>,
    /// The time of the next line.
    next: u64,
    seeds: u64,
    // The directory the segments are in goes when this does.
    _dir: tempfile::TempDir,
}

impl Feed {
    /// A spilling store of `logs` lines from `START`, in chunks of 4,096 lines,
    /// on a clock a second past the last of them.
    fn new(logs: usize) -> Feed {
        let dir = tempfile::tempdir().expect("a temp dir");
        let clock = Arc::new(ManualClock::new(START));
        let store = Store::new(store_config(dir.path(), 4_096), clock.clone());
        let mut feed = Feed {
            store,
            clock,
            next: START,
            seeds: 0,
            _dir: dir,
        };
        feed.more(logs);
        feed
    }

    /// `n` lines newer than the ones before, and the clock a second after them.
    fn more(&mut self, n: usize) {
        self.seeds += 1;
        feed_bulk(
            &self.store,
            &self.clock,
            bulk_logs(n, 100 + self.seeds).starting_at(self.next),
        );
        self.next += n as u64 * SPACING;
        self.clock.set(self.next + SECOND);
    }
}

// --- reaching into the window -----------------------------------------------------------------

fn window_on(store: Store, theme: &Theme) -> Driver<MainWindow> {
    let backend = Rc::new(FixedBackend::new(store));
    Driver::new(WINDOW, theme, move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    })
}

/// Draws until what is pending (a read, its result, the frame that shows it, the
/// read that frame's measure asks for) is done.
fn settle(driver: &mut Driver<MainWindow>) {
    for _ in 0..6 {
        driver.draw();
    }
}

fn open_logs(driver: &mut Driver<MainWindow>) {
    driver.update_root(|main, _, cx| main.navigate(Screen::Logs { query: None }, cx));
    settle(driver);
}

/// The window on `store`, on the Logs screen.
fn logs_window(store: Store, theme: &Theme) -> Driver<MainWindow> {
    let mut driver = window_on(store, theme);
    open_logs(&mut driver);
    driver
}

fn model(driver: &Driver<MainWindow>) -> Entity<LogsModel> {
    driver.read_root(|main| main.logs().clone())
}

fn view(driver: &Driver<MainWindow>) -> Entity<LogsView> {
    driver.read_root(|main| main.logs_view().clone())
}

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

/// How many lines a windowed list has.
fn total(driver: &Driver<MainWindow>) -> u64 {
    read_model(driver, |logs| logs.total()).expect("the list is windowed")
}

/// The position of the row on top of a windowed list.
fn top(driver: &Driver<MainWindow>) -> u64 {
    read_model(driver, |logs| {
        logs.row_window().expect("the list is windowed").top()
    })
}

/// The log on top of a windowed list: the one under its top line.
fn top_id(driver: &Driver<MainWindow>) -> LogId {
    read_model(driver, |logs| {
        let window = logs.row_window().expect("the list is windowed");
        let index = logs
            .row_at(window.top())
            .expect("the row on top has been read");
        logs.rows()[index].id
    })
}

/// The IDs of the rows in view, top to bottom.
fn visible_ids(driver: &Driver<MainWindow>) -> Vec<LogId> {
    read_model(driver, |logs| {
        let window = logs.row_window().expect("the list is windowed");
        window
            .visible()
            .map(|position| {
                let index = logs
                    .row_at(position)
                    .unwrap_or_else(|| panic!("row {position} in view has not been read"));
                logs.rows()[index].id
            })
            .collect()
    })
}

/// Where the list is scrolled to: its first row in view and the part of it above
/// the viewport.
fn scroll_offset(driver: &Driver<MainWindow>) -> (usize, f32) {
    let view = view(driver);
    driver.read(|cx| view.read(cx).scroll_offset())
}

/// The middle of the row at `index` among the model's rows, as drawn now.
fn row_center(driver: &Driver<MainWindow>, index: usize) -> Point<Pixels> {
    let view = view(driver);
    driver
        .read(|cx| view.read(cx).row_center(index))
        .unwrap_or_else(|| panic!("row {index} is not in view"))
}

/// A tick of the live tail: its timer fires and what it read is drawn.
fn tick(driver: &mut Driver<MainWindow>) {
    driver.advance(TAIL_REFRESH);
    settle(driver);
}

// --- the screens --------------------------------------------------------------------------------

#[test]
fn indexed_and_query_paths_switch() {
    let feed = Feed::new(25_000);
    let mut driver = logs_window(feed.store.clone(), &Theme::dark());

    // No query, and more lines than a page-loaded list holds: windowed.
    assert!(read_model(&driver, |logs| logs.windowed()));
    assert_eq!(total(&driver), 25_000);
    assert_eq!(
        read_model(&driver, |logs| logs.footer_text()),
        Some("25,000 lines \u{b7} newest first".to_owned())
    );
    assert_eq!(
        read_model(&driver, |logs| logs.count_line()),
        "25,000 lines \u{b7} newest first"
    );
    assert!(
        has(&driver, LogsPart::Scrollbar),
        "the list has its own scrollbar"
    );
    assert!(has(&driver, LogsPart::Footer));
    assert!(
        read_model(&driver, |logs| logs.rows().len()) < PAGE_SIZE,
        "it holds the rows in view and a viewport on each side, not a page"
    );
    // Newest first, from the first line: the row on top is the newest.
    assert_eq!(top(&driver), 0);
    assert_eq!(top_id(&driver).time_unix_nano, START + 24_999 * SPACING);

    // A query with a line filter cannot be counted by the store: the paged list of
    // Milestone 4, with its first page and no scrollbar of its own.
    let timeouts = r#"{service_name="payments-api"} |= "timeout""#;
    with_model(&mut driver, |logs, cx| logs.set_query(timeouts, cx));
    settle(&mut driver);
    assert!(!read_model(&driver, |logs| logs.windowed()));
    assert_eq!(read_model(&driver, |logs| logs.total()), None);
    assert!(!has(&driver, LogsPart::Scrollbar));
    assert!(
        !has(&driver, LogsPart::Footer),
        "a paged list has a footer only at its cap"
    );
    assert!(read_model(&driver, |logs| !logs.rows().is_empty()));
    assert!(
        read_model(&driver, |logs| logs.rows().iter().all(|row| row.service
            == "payments-api"
            && row.message.contains("timeout"))),
        "the query filtered the rows"
    );

    // An indexed selector that matches 20,000 lines or fewer is paged: the first
    // page of 500, and more to come.
    with_model(&mut driver, |logs, cx| {
        logs.set_query(r#"{service_name="payments-api"}"#, cx);
    });
    settle(&mut driver);
    assert!(!read_model(&driver, |logs| logs.windowed()));
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), PAGE_SIZE);
    assert!(read_model(&driver, |logs| logs.more()));

    // An indexed selector that matches more is windowed again, over the lines it
    // matches.
    with_model(&mut driver, |logs, cx| {
        logs.set_query(r#"{level=~".+"}"#, cx)
    });
    settle(&mut driver);
    assert!(read_model(&driver, |logs| logs.windowed()));
    assert_eq!(total(&driver), 25_000);
    assert!(has(&driver, LogsPart::Scrollbar));

    // The toggles are part of the search: with the debug lines off (a third of
    // the lines) the list is under the cap.
    with_model(&mut driver, |logs, cx| {
        logs.set_levels(LevelSet::from_groups(true, true, true, false), cx);
    });
    settle(&mut driver);
    assert!(!read_model(&driver, |logs| logs.windowed()));
    with_model(&mut driver, |logs, cx| logs.set_levels(LevelSet::ALL, cx));
    settle(&mut driver);
    assert!(read_model(&driver, |logs| logs.windowed()));

    // Back to no query: the same list.
    with_model(&mut driver, |logs, cx| logs.set_query("", cx));
    settle(&mut driver);
    assert!(read_model(&driver, |logs| logs.windowed()));
    assert_eq!(total(&driver), 25_000);
    assert_eq!(top(&driver), 0);

    // The range is a window of time, and it counts too: five minutes of data the
    // clock has gone past is no lines.
    feed.clock.set(START + 3_600 * SECOND);
    with_model(&mut driver, |logs, cx| {
        logs.set_range(loctel_ui::app::TimeRange::Last5m, cx);
    });
    settle(&mut driver);
    assert!(!read_model(&driver, |logs| logs.windowed()));
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), 0);
}

#[test]
fn the_paged_list_stays_paged_up_to_the_cap() {
    // 20,000 lines is a paged list (the cap itself); 20,001 is windowed.
    let mut feed = Feed::new(MAX_ROWS);
    let mut driver = logs_window(feed.store.clone(), &Theme::dark());
    assert!(!read_model(&driver, |logs| logs.windowed()));
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), PAGE_SIZE);

    // One line more, and the next read of the search (here: Run) is windowed.
    feed.more(1);
    with_model(&mut driver, |logs, cx| logs.refresh_now(cx));
    settle(&mut driver);
    assert!(read_model(&driver, |logs| logs.windowed()));
    assert_eq!(total(&driver), MAX_ROWS as u64 + 1);
}

#[test]
fn a_paged_list_whose_tail_reaches_the_cap_becomes_windowed() {
    // Just under the cap, paged all the way down (40 pages of 500 hold 19,990).
    let mut feed = Feed::new(MAX_ROWS - 10);
    let mut driver = logs_window(feed.store.clone(), &Theme::dark());
    assert!(!read_model(&driver, |logs| logs.windowed()));
    for _ in 0..45 {
        with_model(&mut driver, |logs, cx| logs.load_more(cx));
        driver.draw();
    }
    settle(&mut driver);
    assert_eq!(read_model(&driver, |logs| logs.rows().len()), MAX_ROWS - 10);
    assert!(!read_model(&driver, |logs| logs.capped()));

    // The tail brings more than the cap: the paged list would drop its oldest
    // rows and stop at 20,000, but the store counts these lines, so the list is
    // read again, windowed, from the top where the tail was.
    feed.more(100);
    tick(&mut driver);
    settle(&mut driver);
    assert!(read_model(&driver, |logs| logs.windowed()));
    assert_eq!(total(&driver), MAX_ROWS as u64 + 90);
    assert_eq!(top(&driver), 0);
    assert!(!read_model(&driver, |logs| logs.capped()));
    assert_eq!(
        top_id(&driver).time_unix_nano,
        START + (MAX_ROWS as u64 + 89) * SPACING
    );
}

#[test]
fn wheel_scrollbar_and_drag_scroll_a_windowed_list() {
    let feed = Feed::new(25_000);
    let mut driver = logs_window(feed.store.clone(), &Theme::dark());
    assert!(read_model(&driver, |logs| logs.windowed()));
    let list = center(&driver, LogsPart::List);

    // The wheel scrolls rows: three turns of 10 px are one row of 30 px.
    for _ in 0..2 {
        driver.scroll(list, 10.0);
    }
    assert_eq!(top(&driver), 0, "20 px is not a row yet");
    driver.scroll(list, 10.0);
    assert_eq!(top(&driver), 1);
    driver.scroll(list, 300.0);
    assert_eq!(top(&driver), 11);
    // The row on top is the one at that position of the list, newest first.
    assert_eq!(
        top_id(&driver).time_unix_nano,
        START + (24_999 - 11) * SPACING
    );
    assert_eq!(scroll_offset(&driver), (11, 0.0));
    // Scrolling away from the top shows the pill for the tail, which keeps going.
    assert!(has(&driver, LogsPart::Pill));
    assert!(read_model(&driver, |logs| logs.tail_paused()));

    // A long scroll reads the rows it reaches.
    driver.scroll(list, 30.0 * 2_000.0);
    settle(&mut driver);
    assert_eq!(top(&driver), 2_011);
    assert_eq!(
        top_id(&driver).time_unix_nano,
        START + (24_999 - 2_011) * SPACING
    );

    // A press on the track moves the thumb's middle there: near the bottom is
    // near the oldest lines.
    let track = bounds(&driver, LogsPart::Scrollbar).expect("the track");
    let near_end = gpui::point(track.center().x, track.top() + track.size.height * 0.9);
    driver.click(near_end);
    settle(&mut driver);
    let fraction = top(&driver) as f64
        / read_model(&driver, |logs| logs.row_window().unwrap().max_top()) as f64;
    assert!((fraction - 0.9).abs() < 0.02, "{fraction}");
    let thumb = bounds(&driver, LogsPart::Thumb).expect("the thumb");
    assert!(
        (thumb.center().y - near_end.y).abs() < px_of(2.0),
        "the thumb's middle is under the pointer"
    );
    let rows = visible_ids(&driver);
    assert!(
        rows.windows(2).all(|pair| pair[0] > pair[1]),
        "newest first, one after the other"
    );

    // A drag of the thumb carries it, and the rows follow.
    let from = thumb.center();
    let to = gpui::point(from.x, track.top() + track.size.height * 0.25);
    driver.drag(from, to);
    settle(&mut driver);
    let fraction = top(&driver) as f64
        / read_model(&driver, |logs| logs.row_window().unwrap().max_top()) as f64;
    assert!((fraction - 0.25).abs() < 0.02, "{fraction}");
    assert_eq!(
        top_id(&driver).time_unix_nano,
        START + (24_999 - top(&driver)) * SPACING,
        "the rows are the ones at that place"
    );

    // The pill brings back the newest lines.
    driver.click(center(&driver, LogsPart::Pill));
    settle(&mut driver);
    assert_eq!(top(&driver), 0);
    assert!(!has(&driver, LogsPart::Pill));
    assert_eq!(top_id(&driver).time_unix_nano, START + 24_999 * SPACING);

    // A row can be expanded where it stands, and the list still scrolls.
    let row = read_model(&driver, |logs| logs.row_at(2)).expect("row 2");
    driver.click(row_center(&driver, row));
    settle(&mut driver);
    assert!(read_model(&driver, |logs| logs.expanded()).is_some());
    assert!(
        has(&driver, LogsPart::ShowAround),
        "the details are on the screen"
    );
    driver.scroll(list, 300.0);
    assert_eq!(top(&driver), 10);

    // Lines around one: context mode is the short list of the lines around it,
    // not a window, and back to the search is the window again.
    let id = read_model(&driver, |logs| logs.rows()[row].id);
    with_model(&mut driver, |logs, cx| logs.show_around(id, cx));
    settle(&mut driver);
    assert!(!read_model(&driver, |logs| logs.windowed()));
    assert!(read_model(&driver, |logs| logs.context().is_some()));
    with_model(&mut driver, |logs, cx| logs.back_to_search(cx));
    settle(&mut driver);
    assert!(read_model(&driver, |logs| logs.windowed()));
    assert_eq!(top(&driver), 0);
}

fn px_of(value: f32) -> Pixels {
    gpui::px(value)
}

#[test]
fn tail_keeps_anchor_when_scrolled() {
    let mut feed = Feed::new(25_000);
    let mut driver = logs_window(feed.store.clone(), &Theme::dark());
    assert!(read_model(&driver, |logs| logs.windowed()));
    assert_eq!(total(&driver), 25_000);
    let newest = top_id(&driver);

    // At the top, the tail keeps the list at the top: the newest lines come in,
    // and the list is still on the newest.
    feed.more(300);
    tick(&mut driver);
    assert_eq!(total(&driver), 25_300);
    assert_eq!(top(&driver), 0, "the list stays at the top");
    assert!(top_id(&driver) > newest, "and the newest line is on top");
    assert_eq!(top_id(&driver).time_unix_nano, START + 25_299 * SPACING);
    assert!(!has(&driver, LogsPart::Pill));

    // Scrolled to the middle, the tail keeps the row under the top line: new lines
    // are above it, the total grows, the row does not move on the screen.
    with_model(&mut driver, |logs, cx| logs.jump_fraction(0.5, cx));
    settle(&mut driver);
    let place = top(&driver);
    assert!(place > 10_000, "the middle of 25,300 lines: {place}");
    let held = top_id(&driver);
    let rows = visible_ids(&driver);
    assert!(
        has(&driver, LogsPart::Pill),
        "away from the top, the pill shows"
    );

    feed.more(300);
    tick(&mut driver);
    assert_eq!(total(&driver), 25_600);
    assert_eq!(
        top(&driver),
        place + 300,
        "the window moved down with what came"
    );
    assert_eq!(
        top_id(&driver),
        held,
        "the row under the top line is the same"
    );
    assert_eq!(visible_ids(&driver), rows, "so are all the rows in view");

    // A flood in a tick keeps it too, and several ticks in a row.
    feed.more(5_000);
    tick(&mut driver);
    tick(&mut driver);
    assert_eq!(total(&driver), 30_600);
    assert_eq!(top(&driver), place + 5_300);
    assert_eq!(top_id(&driver), held);

    // A line that arrives late, in the middle of the list and above the top line,
    // moves the row down by one, and the row is still the one on top.
    let late = held.time_unix_nano + 3 * SPACING + 7;
    feed.store.ingest_logs(vec![late_log(late)]);
    tick(&mut driver);
    assert_eq!(total(&driver), 30_601);
    assert_eq!(top_id(&driver), held, "found where it is now");
    assert_eq!(top(&driver), place + 5_301);

    // Lines that leave the window at its old end (the range is 15 minutes) do not
    // move it: positions are counted from the newest. 20 minutes later, with the
    // newest data older than the range, the list is over.
    // The scroll is the user's: a move while the tail reads is not undone.
    driver.scroll(center(&driver, LogsPart::List), 30.0 * 40.0);
    feed.more(10);
    tick(&mut driver);
    assert_eq!(top(&driver), place + 5_301 + 40 + 10);

    // The tail off freezes the list: new lines do not show, scrolling still works,
    // and the total stands still.
    with_model(&mut driver, |logs, cx| logs.set_tail(false, cx));
    let frozen = total(&driver);
    feed.more(200);
    for _ in 0..3 {
        tick(&mut driver);
    }
    assert_eq!(total(&driver), frozen);
    assert!(!has(&driver, LogsPart::Pill), "no pill with the tail off");
    driver.scroll(center(&driver, LogsPart::List), 30.0 * 100.0);
    settle(&mut driver);
    assert_eq!(total(&driver), frozen);
    let rows = visible_ids(&driver);
    assert!(rows.windows(2).all(|pair| pair[0] > pair[1]));
    // On again: it catches up, the row on top the same.
    let held = top_id(&driver);
    with_model(&mut driver, |logs, cx| logs.set_tail(true, cx));
    settle(&mut driver);
    assert_eq!(total(&driver), frozen + 200);
    assert_eq!(top_id(&driver), held);
}

fn late_log(at: u64) -> LogRecord {
    use loctel_model::{AttrValue, Resource};
    LogRecord {
        time_unix_nano: at,
        observed_unix_nano: at,
        severity: Severity::Info,
        severity_text: String::new(),
        body: "a late line".to_owned(),
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
    }
}

#[test]
fn a_windowed_list_has_a_histogram_made_from_the_counts() {
    let feed = Feed::new(25_000);
    let mut driver = logs_window(feed.store.clone(), &Theme::dark());
    assert!(read_model(&driver, |logs| logs.windowed()));
    // The histogram is read for the same lines, from what the store keeps about
    // its chunks (a scan of every line would be a read of ten million every two
    // seconds), and shows them.
    let bars = read_model(&driver, |logs| logs.histogram().clone());
    assert!(
        bars.bars.iter().any(|(total, _)| *total > 0.0),
        "the lines are in the bars"
    );
    assert!(
        bars.bars.iter().any(|(_, errors)| *errors > 0.0),
        "and so are the errors, about 5% of them"
    );

    // A selector of one service is a list of a fifth of the lines, and the store
    // counts it: the model's own count is the store's.
    with_model(&mut driver, |logs, cx| {
        logs.set_query(r#"{service_name="payments-api"}"#, cx);
    });
    settle(&mut driver);
    let spec = IndexedFilter {
        service: Some("payments-api".to_owned()),
        levels: LevelSet::ALL,
    };
    let held = feed
        .store
        .log_count(&spec.filter(None, None))
        .expect("an indexed filter counts");
    assert!(held > 4_000 && held < 6_000, "{held}");
    assert!(
        !read_model(&driver, |logs| logs.windowed()),
        "under the cap"
    );
    assert_eq!(
        read_model(&driver, |logs| logs.count_line()),
        format!("{}+ lines \u{b7} newest first", PAGE_SIZE)
    );
}

// --- ten million lines ---------------------------------------------------------------------------

#[test]
fn ten_million_scroll_to_middle_dark() {
    const LOGS: usize = 10_000_000;
    let dir = tempfile::tempdir().expect("a temp dir");
    let clock = Arc::new(ManualClock::new(START));
    let store = Store::new(store_config(dir.path(), 65_536), clock.clone());
    feed_bulk(&store, &clock, bulk_logs(LOGS, SEED));
    // A second past the last line, so the range of 15 minutes covers all of them,
    // and a minute and a half for the last chunk to seal.
    let last = START + (LOGS as u64 - 1) * SPACING;
    clock.set(last + SECOND);
    clock.advance(Duration::from_secs(90));
    let stats = store.stats();
    assert_eq!(stats.logs, LOGS, "the store holds every line");
    assert!(stats.disk_bytes > 0, "and has put them in segment files");

    let mut driver = logs_window(store.clone(), &Theme::dark());
    assert!(read_model(&driver, |logs| logs.windowed()));
    assert_eq!(total(&driver), LOGS as u64);
    assert_eq!(top(&driver), 0);
    assert_eq!(
        top_id(&driver).time_unix_nano,
        last,
        "the newest line is on top"
    );

    // Only the rows in view and a viewport on each side are in memory.
    let held = read_model(&driver, |logs| logs.rows().len());
    assert!(held < 200, "{held} rows held");

    // The scrollbar's jump to the middle.
    with_model(&mut driver, |logs, cx| logs.jump_fraction(0.5, cx));
    settle(&mut driver);
    let position = top(&driver);
    let max_top = read_model(&driver, |logs| logs.row_window().unwrap().max_top());
    assert_eq!(
        position,
        max_top.div_ceil(2),
        "half way down 10,000,000 lines"
    );
    assert!((4_999_000..=5_001_000).contains(&position), "{position}");

    // The rows in view are the ones at that place, continuous: each is 20 us older
    // than the one above it, and the one on top is exactly where the position says.
    let ids = visible_ids(&driver);
    assert!(ids.len() >= 10, "{} rows in view", ids.len());
    assert_eq!(
        ids[0].time_unix_nano,
        last - position * SPACING,
        "the row at position {position}"
    );
    for pair in ids.windows(2) {
        assert_eq!(
            pair[0].time_unix_nano - pair[1].time_unix_nano,
            SPACING,
            "the rows are continuous"
        );
    }
    // And so are all the rows held, across the places the model read them.
    let all: Vec<LogId> = read_model(&driver, |logs| logs.rows().iter().map(|r| r.id).collect());
    for pair in all.windows(2) {
        assert_eq!(pair[0].time_unix_nano - pair[1].time_unix_nano, SPACING);
    }

    // The footer says how many lines there are, and the thumb is in the middle of
    // its track.
    assert_eq!(
        read_model(&driver, |logs| logs.footer_text()),
        Some("10,000,000 lines \u{b7} newest first".to_owned())
    );
    assert!(has(&driver, LogsPart::Footer));
    let track = bounds(&driver, LogsPart::Scrollbar).expect("the track");
    let thumb = bounds(&driver, LogsPart::Thumb).expect("the thumb");
    assert!(thumb.size.height >= px_of(MIN_THUMB_PX - 0.5));
    assert!(
        (thumb.center().y - track.center().y).abs() < track.size.height * 0.02,
        "the thumb is in the middle of the track"
    );
    let image = driver.capture();
    assert_eq!(image.dimensions(), (2880, 1800));
    assert_snapshot("logs_10m_middle_dark", &image);

    // A turn of the wheel from there moves rows, exactly.
    driver.scroll(center(&driver, LogsPart::List), 30.0 * 5.0);
    settle(&mut driver);
    assert_eq!(top(&driver), position + 5);
    assert_eq!(
        top_id(&driver).time_unix_nano,
        last - (position + 5) * SPACING
    );
    // And to the very end of the list, the oldest line.
    with_model(&mut driver, |logs, cx| logs.jump_fraction(1.0, cx));
    settle(&mut driver);
    let ids = visible_ids(&driver);
    assert_eq!(
        ids.last().map(|id| id.time_unix_nano),
        Some(START),
        "the oldest line is at the bottom"
    );
    drop(dir);
}

// --- a trace the disk cap removed -----------------------------------------------------------------

#[test]
fn evicted_open_trace_shows_gone() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(
        StoreConfig::spilling_for_tests(dir.path().to_path_buf()),
        clock.clone(),
    );
    feed(&store, &generate(&Scenario::CheckoutFailure, SEED, BASE));
    // The trace is complete and spilled: it is in a segment file, which is what
    // the disk cap deletes.
    clock.advance(Duration::from_secs(70));
    let _ = store.stats();
    assert!(store.spilled_traces() >= 1, "the trace is on disk");

    let backend = Rc::new(FixedBackend::new(store.clone()));
    let mut driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    let id = failing_trace_id();
    open_trace(&mut driver, id);
    let trace = driver.read_root(|main| main.trace_view().cloned().expect("a trace screen"));
    let model = driver.read(|cx| trace.read(cx).model().clone());
    assert_eq!(driver.read(|cx| model.read(cx).state()), LoadState::Loaded);
    assert_eq!(driver.read(|cx| trace.read(cx).notice(cx)), None);

    // The disk cap goes below what the files take: the oldest segment, with the
    // trace, is deleted at once.
    store.set_disk_cap(1);
    assert!(store.trace(id).is_none(), "the cap removed the trace");

    // The screen finds out at its next read (a complete trace is read every 2 s),
    // and says so, instead of showing what it last had.
    driver.advance(DONE_REFRESH);
    for _ in 0..3 {
        driver.draw();
    }
    assert_eq!(driver.read(|cx| model.read(cx).state()), LoadState::Evicted);
    assert_eq!(
        NO_LONGER_HELD,
        "This trace is no longer held (the disk cap removed it)"
    );
    assert_eq!(
        driver.read(|cx| trace.read(cx).notice(cx)),
        Some(NO_LONGER_HELD)
    );
    assert_snapshot("trace_evicted_dark", &driver.capture());

    // The Back button is there, and it goes back.
    let back = part_center(&driver, TracePart::Back);
    driver.click(back);
    assert_eq!(
        driver.update_root(|main, _, _| main.screen().clone()),
        Screen::Home
    );
}

#[test]
fn a_trace_that_comes_back_shows_again() {
    // A late span rehydrates a trace that was gone from the screen's view: the
    // trace is on the screen again, not stuck at "no longer held".
    let dir = tempfile::tempdir().expect("a temp dir");
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(
        StoreConfig::spilling_for_tests(dir.path().to_path_buf()),
        clock.clone(),
    );
    let generated = generate(&Scenario::CheckoutFailure, SEED, BASE);
    feed(&store, &generated);
    clock.advance(Duration::from_secs(70));
    let _ = store.stats();

    let backend = Rc::new(FixedBackend::new(store.clone()));
    let mut driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    open_trace(&mut driver, failing_trace_id());
    let trace = driver.read_root(|main| main.trace_view().cloned().expect("a trace screen"));
    let model = driver.read(|cx| trace.read(cx).model().clone());

    store.set_disk_cap(1);
    driver.advance(DONE_REFRESH);
    for _ in 0..3 {
        driver.draw();
    }
    assert_eq!(driver.read(|cx| model.read(cx).state()), LoadState::Evicted);

    // The same trace arrives again (an exporter that retries): it is held, and
    // the screen shows it.
    store.set_disk_cap(4 * 1024 * 1024 * 1024);
    feed(&store, &generated);
    driver.advance(DONE_REFRESH);
    for _ in 0..3 {
        driver.draw();
    }
    assert_eq!(driver.read(|cx| model.read(cx).state()), LoadState::Loaded);
    assert_eq!(driver.read(|cx| trace.read(cx).notice(cx)), None);
}
