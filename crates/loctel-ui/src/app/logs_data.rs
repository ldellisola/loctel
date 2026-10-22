//! The live data of the Logs screen (design 9.2, M4 plan Task 4): the lines of
//! the list, its volume histogram and what the user set to shape them, kept
//! fresh from the store.
//!
//! [`LogsModel`] is an entity the Logs views observe, like
//! [`HomeModel`](super::HomeModel) for the home screen. It owns the LogQL bar's
//! text, the level toggles, the time range and the live tail switch, and the
//! result of the searches they make: the rows, the record behind each, the
//! histogram, which line is expanded and whether the list is in context mode.
//!
//! - **Searching.** A change of the query, the levels or the range reads the
//!   first page ([`PAGE_SIZE`] = 500 lines, newest first) and replaces the
//!   list. The old rows stay until it arrives. A query that does not parse
//!   changes nothing but its state: the list keeps the last good query's rows.
//! - **Paging.** [`load_more`](LogsModel::load_more) reads the next page and
//!   appends it, until the list holds [`MAX_ROWS`] (20,000): past that it stops
//!   and [`capped`](LogsModel::capped) says so (the footer). The view asks as
//!   rows within [`PREFETCH_ROWS`] of the end are drawn.
//! - **Live tail.** While the tail is on and the list is at the top, the newest
//!   page is read every [`TAIL_REFRESH`] (500 ms) and joined to the rows
//!   ([`merge_head`]): new lines go on top, a line is never in the list twice,
//!   and a flood that outruns a page starts the list over from it. The view tells
//!   the model when the user scrolls away from the top
//!   ([`set_at_top`](LogsModel::set_at_top)); that pauses the tail, so nothing
//!   is ever put above rows the user is looking at. Scrolling back, or
//!   [`jump_to_newest`](LogsModel::jump_to_newest), resumes it. The histogram is
//!   read every [`HISTOGRAM_REFRESH`] (2 s) while the tail is on.
//! - **Lines around** ([`show_around`](LogsModel::show_around)): the list shows
//!   what the store holds around one line, unfiltered and oldest first, with the
//!   line marked. The tail and the paging stand still until
//!   [`back_to_search`](LogsModel::back_to_search) (or any change of the search)
//!   brings the search back.
//!
//! **Reads never block a frame.** Every store read runs on GPUI's background
//! executor on a copy of its inputs, and its result comes back to the
//! foreground, where the pure part ([`model::logs`](crate::model::logs)) makes
//! the rows. A read that a newer one has replaced is thrown away when it
//! arrives.
//!
//! **Telling the list.** The list is a variable-height GPUI `list` (an expanded
//! line is taller), whose state the view owns. The model says what happened to
//! the rows as [`LogsModelEvent`]s, in order, so the view can keep the list's
//! count and its measured heights right.
//!
//! The model reads only while it is *active* ([`set_active`](LogsModel::set_active),
//! which the shell sets as the Logs screen comes and goes): a screen that is not
//! showing costs no reads. `docs/areas/ui.md` ("The Logs screen") has the rules.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, EventEmitter};
use loctel_model::LogRecord;
use loctel_query::logql::LogQuery;
use loctel_store::{HistogramBucket, LevelSet, LogFilter, LogId, Page, PageRequest, Store};

use super::backend::UiBackend;
use super::data::TimeRange;
use crate::model::colors::ServiceColors;
use crate::model::histogram::{self, Bars};
use crate::model::logs::{
    self, LogQueryState, LogRow, MAX_ROWS, PAGE_SIZE, context_banner, context_count_line,
    count_line, details, footer, merge_head,
};

/// How often the newest page is read again while the tail runs.
pub const TAIL_REFRESH: Duration = Duration::from_millis(500);

/// How often the histogram is read again while the tail runs.
pub const HISTOGRAM_REFRESH: Duration = Duration::from_secs(2);

/// How many buckets the histogram has (design: 48).
pub const HISTOGRAM_BUCKETS: usize = 48;

/// How many lines before and after a log "Show lines around this one" reads.
pub const CONTEXT_BEFORE: usize = 25;
pub const CONTEXT_AFTER: usize = 25;

/// What the model tells the list view besides "something changed": what
/// happened to the rows, in the order it happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogsModelEvent {
    /// The rows were replaced: rebuild the list from [`LogsModel::rows`], at
    /// the top.
    Reset,
    /// This many rows were added at the top, and the ones that were there
    /// follow unchanged.
    Prepended(usize),
    /// This many rows were added at the end.
    Appended(usize),
    /// Every row from this index on was dropped: the list is this long now.
    Truncated(usize),
    /// The row at this index changed height (it was expanded or folded).
    Resized(usize),
    /// Bring the row at this index into view, in the middle (the target of the
    /// lines around a log).
    Reveal(usize),
    /// Scroll the list to its top: a new search starts there, and the pill
    /// brought the tail back.
    Top,
}

impl EventEmitter<LogsModelEvent> for LogsModel {}

/// The lines around one log, while the list shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextInfo {
    /// The log asked about.
    pub target: LogId,
    /// Whether the store still held it when the lines were read. When it did
    /// not, the lines are the ones nearest to where it was.
    pub held: bool,
    /// Where the target is among the rows, when [`held`](Self::held).
    pub index: Option<usize>,
}

/// The Logs screen's data and the settings that shape it. Make it with
/// `cx.new(|cx| LogsModel::new(backend, cx))` and observe it from the views.
pub struct LogsModel {
    backend: Rc<dyn UiBackend>,
    /// The session's service colours: the home screen's, so a service keeps its
    /// colour everywhere ([`with_colors`](Self::with_colors)).
    colors: Rc<RefCell<ServiceColors>>,

    // What the user set.
    /// The text in the LogQL bar.
    query_text: String,
    /// What the bar's text is, as typed now.
    query_state: LogQueryState,
    /// What the reads filter with: the latest query that parsed, or none for
    /// an empty bar. An invalid text leaves it as it was.
    applied: Option<Arc<LogQuery>>,
    levels: LevelSet,
    range: TimeRange,
    /// The live tail switch.
    tail: bool,

    // What the views tell.
    /// The list is scrolled away from the top (the view says so).
    scrolled_away: bool,
    /// The Logs screen is the one showing (the shell says so).
    active: bool,

    // What is shown.
    rows: Vec<LogRow>,
    /// The record behind each row, in the same order: the expanded line's
    /// details and the rules suggestion are made from it.
    records: Vec<LogRecord>,
    /// The cursor of the page after the last row, `None` when nothing is left
    /// to load or the list is capped.
    next_cursor: Option<String>,
    capped: bool,
    loaded: bool,
    expanded: Option<LogId>,
    context: Option<ContextInfo>,
    histogram: Bars,

    // Bookkeeping.
    /// Counts the reads that replace the rows. Only results of the latest
    /// count are applied; the tail's and the paging's reads belong to the count
    /// they started in.
    generation: u64,
    /// The latest replacing read has not come back yet.
    reloading: bool,
    head_reading: bool,
    more_reading: bool,
    histogram_generation: u64,
}

impl LogsModel {
    /// The model with the default range, all four levels, the tail on and no
    /// query, reading from `backend`, with colours of its own. It reads nothing
    /// until it is made [active](Self::set_active).
    pub fn new(backend: Rc<dyn UiBackend>, cx: &mut Context<Self>) -> LogsModel {
        Self::with_colors(backend, Rc::new(RefCell::new(ServiceColors::new())), cx)
    }

    /// Like [`new`](Self::new), with the session's service colours
    /// ([`HomeModel::colors`](super::HomeModel::colors)) so a service has one
    /// colour on every screen.
    pub fn with_colors(
        backend: Rc<dyn UiBackend>,
        colors: Rc<RefCell<ServiceColors>>,
        cx: &mut Context<Self>,
    ) -> LogsModel {
        let model = LogsModel {
            backend,
            colors,
            query_text: String::new(),
            query_state: LogQueryState::Empty,
            applied: None,
            levels: LevelSet::ALL,
            range: TimeRange::default(),
            tail: true,
            scrolled_away: false,
            active: false,
            rows: Vec::new(),
            records: Vec::new(),
            next_cursor: None,
            capped: false,
            loaded: false,
            expanded: None,
            context: None,
            histogram: empty_bars(),
            generation: 0,
            reloading: false,
            head_reading: false,
            more_reading: false,
            histogram_generation: 0,
        };

        cx.spawn(async move |logs, cx| {
            loop {
                cx.background_executor().timer(TAIL_REFRESH).await;
                // The model is gone once the window closes; that ends the loop.
                if logs.update(cx, |logs, cx| logs.tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |logs, cx| {
            loop {
                cx.background_executor().timer(HISTOGRAM_REFRESH).await;
                if logs.update(cx, |logs, cx| logs.histogram_tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        model
    }

    // --- what it holds ----------------------------------------------------------------

    /// The rows of the list: newest first in a search, oldest first in context
    /// mode.
    pub fn rows(&self) -> &[LogRow] {
        &self.rows
    }

    /// The record behind the row at `index`.
    pub fn record(&self, index: usize) -> Option<&LogRecord> {
        self.records.get(index)
    }

    /// The volume histogram over the range, for the lines that match the
    /// search: 48 bars and four labels. Flat, with blank labels, until the first
    /// read is back.
    pub fn histogram(&self) -> &Bars {
        &self.histogram
    }

    /// What the LogQL bar's text is, as typed now (empty, a query, or an error
    /// with its column).
    pub fn state(&self) -> &LogQueryState {
        &self.query_state
    }

    /// The text in the bar, as typed (even when it does not parse).
    pub fn query_text(&self) -> &str {
        &self.query_text
    }

    pub fn levels(&self) -> LevelSet {
        self.levels
    }

    pub fn range(&self) -> TimeRange {
        self.range
    }

    /// The live tail switch.
    pub fn tail(&self) -> bool {
        self.tail
    }

    /// Whether the view last said the list is scrolled away from the top.
    pub fn scrolled_away(&self) -> bool {
        self.scrolled_away
    }

    /// Whether the tail is on but standing still because the list is scrolled
    /// away from the top (the pill shows). Never in context mode: the tail is
    /// not running there for another reason.
    pub fn tail_paused(&self) -> bool {
        self.tail && self.scrolled_away && self.context.is_none()
    }

    /// The first read of the search has come back: an empty [`rows`](Self::rows)
    /// means no lines, not "not read yet".
    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// Whether the Logs screen is the one showing.
    pub fn active(&self) -> bool {
        self.active
    }

    /// Whether more lines wait than the list holds (another page, or the cap).
    pub fn more(&self) -> bool {
        self.next_cursor.is_some() || self.capped
    }

    /// The list stopped at [`MAX_ROWS`] with more lines matching: the footer
    /// says `Showing the newest 20,000 matching lines`.
    pub fn capped(&self) -> bool {
        self.capped
    }

    /// The footer's text, when the list is capped.
    pub fn footer(&self) -> Option<&'static str> {
        footer(self.capped)
    }

    /// The count on the right of the level toggles. See
    /// [`count_line`](crate::model::logs::count_line); in context mode it is the
    /// lines shown, `oldest first`.
    pub fn count_line(&self) -> String {
        if self.context.is_some() {
            return context_count_line(self.rows.len());
        }
        count_line(self.rows.len(), self.more(), self.capped)
    }

    /// Whether the list is narrowed by anything the user set: a query that
    /// runs, or a level toggle that is off. The range is a window, not a filter.
    /// The empty list says `No lines match` then, `No logs yet` otherwise.
    pub fn has_filters(&self) -> bool {
        self.applied.is_some() || self.levels != LevelSet::ALL
    }

    /// The expanded line, if one is.
    pub fn expanded(&self) -> Option<LogId> {
        self.expanded
    }

    /// The expanded line's record.
    pub fn expanded_record(&self) -> Option<&LogRecord> {
        let id = self.expanded?;
        self.rows
            .iter()
            .position(|row| row.id == id)
            .and_then(|index| self.records.get(index))
    }

    /// The expanded line's details grid: the whole body, then the labels
    /// ([`details`](crate::model::logs::details)).
    pub fn expanded_details(&self) -> Option<Vec<(String, String)>> {
        self.expanded_record().map(details)
    }

    /// The lines around one log, while the list shows them.
    pub fn context(&self) -> Option<&ContextInfo> {
        self.context.as_ref()
    }

    /// Where the target of the lines around a log is among the rows: `None`
    /// outside context mode and when the target is no longer held.
    pub fn context_target(&self) -> Option<usize> {
        self.context.as_ref().and_then(|context| context.index)
    }

    /// The words of the context banner (`Lines around 14:32:09.194`, or `This
    /// line is no longer held`), `None` outside context mode.
    pub fn banner(&self) -> Option<String> {
        let context = self.context.as_ref()?;
        Some(context_banner(
            context.target,
            context.held,
            self.backend.utc_offset(),
        ))
    }

    /// The session's service colours.
    pub fn colors(&self) -> Rc<RefCell<ServiceColors>> {
        Rc::clone(&self.colors)
    }

    // --- what the user does ---------------------------------------------------------------

    /// Sets the text of the LogQL bar. [`state`](Self::state) is new when this
    /// returns. An empty bar or a query that parses searches again from the
    /// first page (and leaves context mode); a text that does not parse changes
    /// only the state, and the list keeps filtering with the last good query.
    pub fn set_query(&mut self, text: &str, cx: &mut Context<Self>) {
        let state = logs::validate(text);
        self.query_text = text.to_owned();
        let searches = match &state {
            LogQueryState::Empty => {
                self.applied = None;
                true
            }
            LogQueryState::Valid(query) => {
                self.applied = Some(Arc::clone(query));
                true
            }
            LogQueryState::Invalid { .. } => false,
        };
        self.query_state = state;
        cx.notify();
        if searches {
            self.search_changed(cx);
        }
    }

    /// Sets which levels the list and the histogram keep (the four toggles).
    pub fn set_levels(&mut self, levels: LevelSet, cx: &mut Context<Self>) {
        if self.levels != levels {
            self.levels = levels;
            cx.notify();
            self.search_changed(cx);
        }
    }

    /// Sets the window of time the list and the histogram cover.
    pub fn set_range(&mut self, range: TimeRange, cx: &mut Context<Self>) {
        if self.range != range {
            self.range = range;
            cx.notify();
            self.search_changed(cx);
        }
    }

    /// Turns the live tail on or off. Off, the list and the histogram stand
    /// still; on, it catches up at once.
    pub fn set_tail(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.tail == on {
            return;
        }
        self.tail = on;
        cx.notify();
        if on {
            self.tick(cx);
            self.histogram_tick(cx);
        }
    }

    /// Reads the next page and appends it. Nothing happens in context mode,
    /// while a read of the list is out, when there is no next page and at the
    /// cap.
    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.context.is_some() || self.more_reading || self.reloading || self.capped {
            return;
        }
        let Some(cursor) = self.next_cursor.clone() else {
            return;
        };
        self.more_reading = true;
        let generation = self.generation;
        let limit = PAGE_SIZE.min(MAX_ROWS.saturating_sub(self.rows.len()));
        let plan = self.plan(Some(cursor.clone()), limit);
        cx.spawn(async move |logs, cx| {
            let page = cx
                .background_executor()
                .spawn(async move { plan.read() })
                .await;
            logs.update(cx, |logs, cx| {
                logs.apply_more(generation, &cursor, page, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Expands the line `id` (`Some`), which folds the one that was open, or
    /// folds the open one (`None`). An ID that is not in the list is the same
    /// as `None`.
    pub fn expand(&mut self, id: Option<LogId>, cx: &mut Context<Self>) {
        let wanted = id.filter(|id| self.rows.iter().any(|row| row.id == *id));
        if self.expanded == wanted {
            return;
        }
        let before = self.expanded;
        self.expanded = wanted;
        for id in [before, wanted].into_iter().flatten() {
            if let Some(index) = self.rows.iter().position(|row| row.id == id) {
                cx.emit(LogsModelEvent::Resized(index));
            }
        }
        cx.notify();
    }

    /// Shows what the store holds around the log `id`: up to
    /// [`CONTEXT_BEFORE`] lines before it and [`CONTEXT_AFTER`] after, with
    /// every filter ignored, oldest first. The list changes when the read is
    /// back.
    pub fn show_around(&mut self, id: LogId, cx: &mut Context<Self>) {
        // A newer read replaces what is out: this is the list now.
        self.generation += 1;
        let generation = self.generation;
        self.reloading = true;
        self.head_reading = false;
        self.more_reading = false;
        let store = self.backend.store();
        cx.spawn(async move |logs, cx| {
            let around = cx
                .background_executor()
                .spawn(async move { store.logs_around(id, CONTEXT_BEFORE, CONTEXT_AFTER) })
                .await;
            logs.update(cx, |logs, cx| logs.apply_around(generation, id, around, cx))
                .ok();
        })
        .detach();
    }

    /// Leaves context mode: the search comes back, from the first page.
    pub fn back_to_search(&mut self, cx: &mut Context<Self>) {
        if self.context.is_some() {
            self.context = None;
            self.scroll_to_top(cx);
            cx.notify();
            self.reload(cx);
        }
    }

    /// The view says whether the list is at its top. Scrolling away pauses the
    /// tail; coming back to the top resumes it, and the lines that arrived
    /// meanwhile join the list at once.
    pub fn set_at_top(&mut self, at_top: bool, cx: &mut Context<Self>) {
        let away = !at_top;
        if self.scrolled_away == away {
            return;
        }
        self.scrolled_away = away;
        cx.notify();
        if !away {
            self.tick(cx);
        }
    }

    /// The pill's click: the tail resumes from the newest lines (the list is
    /// read again from the first page, and the view puts it at the top).
    pub fn jump_to_newest(&mut self, cx: &mut Context<Self>) {
        self.scroll_to_top(cx);
        cx.notify();
        self.reload(cx);
    }

    /// Reads the search again from the first page, whatever the tail says: what
    /// Enter and `Run` do.
    pub fn refresh_now(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
        self.read_histogram(cx);
    }

    /// Empties the store, traces and logs, through the backend (the toolbar's
    /// `Clear all`; receiving carries on), and reads at once. The query, the
    /// levels and the range stay.
    pub fn clear_all(&mut self, cx: &mut Context<Self>) {
        self.backend.clear_all();
        self.context = None;
        self.scroll_to_top(cx);
        cx.notify();
        self.reload(cx);
        self.read_histogram(cx);
    }

    /// The shell says whether the Logs screen is showing. While it is not, the
    /// timers read nothing. Becoming active reads the search the first time,
    /// and afterwards catches up the way the tail does (a list the user scrolled
    /// away from, or with the tail off, is left as it is).
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if !active {
            return;
        }
        if self.loaded {
            self.tick(cx);
        } else if !self.reloading {
            self.reload(cx);
        }
        self.read_histogram(cx);
    }

    // --- inside -------------------------------------------------------------------------------

    /// A change of what is searched: back to the search from the first page.
    fn search_changed(&mut self, cx: &mut Context<Self>) {
        self.context = None;
        self.scroll_to_top(cx);
        self.reload(cx);
        self.read_histogram(cx);
    }

    /// The list goes to its top, and the tail is no longer paused by a scroll.
    fn scroll_to_top(&mut self, cx: &mut Context<Self>) {
        self.scrolled_away = false;
        cx.emit(LogsModelEvent::Top);
    }

    /// What a read needs of the model: the filter, copied out so it can run on
    /// another thread.
    fn plan(&self, cursor: Option<String>, limit: usize) -> ReadPlan {
        ReadPlan {
            store: self.backend.store(),
            range: self.range,
            levels: (self.levels != LevelSet::ALL).then_some(self.levels),
            query: self.applied.clone(),
            cursor,
            limit,
        }
    }

    /// Reads the first page and, when it is back, replaces the list with it.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.reloading = true;
        self.head_reading = false;
        self.more_reading = false;
        let plan = self.plan(None, PAGE_SIZE);
        cx.spawn(async move |logs, cx| {
            let page = cx
                .background_executor()
                .spawn(async move { plan.read() })
                .await;
            logs.update(cx, |logs, cx| logs.apply_reload(generation, page, cx))
                .ok();
        })
        .detach();
    }

    /// The tail's timer: the newest page, unless the tail has a reason to stand
    /// still.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let running = self.active
            && self.tail
            && !self.scrolled_away
            && self.context.is_none()
            && self.loaded
            && !self.reloading
            && !self.head_reading;
        if !running {
            return;
        }
        self.head_reading = true;
        let generation = self.generation;
        let plan = self.plan(None, PAGE_SIZE);
        cx.spawn(async move |logs, cx| {
            let page = cx
                .background_executor()
                .spawn(async move { plan.read() })
                .await;
            logs.update(cx, |logs, cx| logs.apply_head(generation, page, cx))
                .ok();
        })
        .detach();
    }

    /// The histogram's timer.
    fn histogram_tick(&mut self, cx: &mut Context<Self>) {
        if self.active && self.tail {
            self.read_histogram(cx);
        }
    }

    /// Reads the histogram on the background executor.
    fn read_histogram(&mut self, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        self.histogram_generation += 1;
        let generation = self.histogram_generation;
        let plan = self.plan(None, 0);
        cx.spawn(async move |logs, cx| {
            let read = cx
                .background_executor()
                .spawn(async move { plan.read_histogram() })
                .await;
            logs.update(cx, |logs, cx| logs.apply_histogram(generation, read, cx))
                .ok();
        })
        .detach();
    }

    fn apply_histogram(&mut self, generation: u64, read: HistogramRead, cx: &mut Context<Self>) {
        if generation != self.histogram_generation {
            return;
        }
        let bars = histogram::bars(
            &read.buckets,
            read.since,
            read.until,
            self.backend.utc_offset(),
        );
        if self.histogram != bars {
            self.histogram = bars;
            cx.notify();
        }
    }

    /// Makes the rows of `items` and appends them to `rows` and `records`.
    fn push_rows(&mut self, items: Vec<(LogId, LogRecord)>) {
        let utc_offset = self.backend.utc_offset();
        let rows = {
            let mut colors = self.colors.borrow_mut();
            logs::rows(&items, &mut colors, utc_offset)
        };
        self.rows.extend(rows);
        self.records
            .extend(items.into_iter().map(|(_, record)| record));
    }

    /// The rows of `items`, for a list that is built from scratch.
    fn replace_rows(&mut self, items: Vec<(LogId, LogRecord)>) {
        self.rows.clear();
        self.records.clear();
        self.push_rows(items);
    }

    /// Drops the expanded line when the list no longer has it.
    fn forget_gone_expansion(&mut self) {
        if let Some(id) = self.expanded
            && !self.rows.iter().any(|row| row.id == id)
        {
            self.expanded = None;
        }
    }

    /// The first page of a search is back: it is the list now.
    fn apply_reload(
        &mut self,
        generation: u64,
        page: Page<(LogId, LogRecord)>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        self.reloading = false;
        self.loaded = true;
        self.context = None;
        self.next_cursor = page.next_cursor;
        self.capped = false;
        self.replace_rows(page.items);
        self.forget_gone_expansion();
        cx.emit(LogsModelEvent::Reset);
        cx.notify();
    }

    /// The newest page, read by the tail, is back: join it to the rows.
    fn apply_head(
        &mut self,
        generation: u64,
        page: Page<(LogId, LogRecord)>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        self.head_reading = false;
        // The user may have scrolled away, or turned the tail off, while the
        // read was out: then nothing is put above what they look at.
        if !self.tail || self.scrolled_away || self.context.is_some() || !self.active {
            return;
        }
        let Page {
            items,
            next_cursor: fresh_cursor,
        } = page;
        let fresh: Vec<LogId> = items.iter().map(|(id, _)| *id).collect();
        let Some(keep_from) = merge_head(&self.rows, |row| row.id, &fresh) else {
            // No join without a gap (or nothing matches any more): start over
            // from the page. A read of the next page that is out is for rows
            // that are gone.
            self.generation += 1;
            self.more_reading = false;
            self.next_cursor = fresh_cursor;
            self.capped = false;
            self.replace_rows(items);
            self.forget_gone_expansion();
            cx.emit(LogsModelEvent::Reset);
            cx.notify();
            return;
        };

        let old_ids: Vec<LogId> = self.rows[..keep_from].iter().map(|row| row.id).collect();
        let old_len = self.rows.len();
        // The page supersedes the first `keep_from` rows, which it holds again.
        let kept_rows = self.rows.split_off(keep_from);
        let kept_records = self.records.split_off(keep_from);
        self.rows.clear();
        self.records.clear();
        self.push_rows(items);
        self.rows.extend(kept_rows);
        self.records.extend(kept_records);
        if keep_from == old_len {
            // The page reaches past every row held: its end is the list's.
            self.next_cursor = fresh_cursor;
        }

        // A pure prepend: the rows the page superseded are the tail of the page
        // as they were, so the list only grew at the top.
        let added = fresh.len().saturating_sub(keep_from);
        let prepended = fresh.len() >= keep_from && fresh[added..] == old_ids[..];
        let mut changed = false;
        if prepended {
            if added > 0 {
                debug_assert_eq!(added, self.rows.len() - old_len);
                cx.emit(LogsModelEvent::Prepended(added));
                changed = true;
            }
        } else {
            self.generation += 1;
            self.more_reading = false;
            cx.emit(LogsModelEvent::Reset);
            changed = true;
        }
        if self.rows.len() > MAX_ROWS {
            // Past the cap the oldest rows go. The cursor of the page after the
            // last row cannot be made again, and at the cap nothing is read.
            self.rows.truncate(MAX_ROWS);
            self.records.truncate(MAX_ROWS);
            self.next_cursor = None;
            self.capped = true;
            if prepended {
                cx.emit(LogsModelEvent::Truncated(MAX_ROWS));
            }
            changed = true;
        }
        self.forget_gone_expansion();
        if changed {
            cx.notify();
        }
    }

    /// The page after the last row, read by `load_more` with `cursor`, is back.
    fn apply_more(
        &mut self,
        generation: u64,
        cursor: &str,
        page: Page<(LogId, LogRecord)>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        self.more_reading = false;
        // The list's end moved while the read was out (the tail joined a page
        // that reached past it): this page would repeat rows.
        if self.next_cursor.as_deref() != Some(cursor) || self.context.is_some() {
            return;
        }
        let last = self.rows.last().map(|row| row.id);
        let mut items = page.items;
        // Belt and braces: whatever the cursor said, a row is added only if it
        // is older than the last one held.
        items.retain(|(id, _)| last.is_none_or(|last| *id < last));
        let added = items.len();
        self.push_rows(items);
        self.next_cursor = page.next_cursor;
        if self.rows.len() >= MAX_ROWS {
            self.capped = self.next_cursor.is_some();
            self.next_cursor = None;
        }
        if added > 0 {
            cx.emit(LogsModelEvent::Appended(added));
        }
        cx.notify();
    }

    /// The lines around a log are back: they are the list now.
    fn apply_around(
        &mut self,
        generation: u64,
        target: LogId,
        around: loctel_store::Around,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        self.reloading = false;
        self.loaded = true;
        self.next_cursor = None;
        self.capped = false;
        self.scrolled_away = false;
        self.replace_rows(around.lines);
        self.expanded = None;
        self.context = Some(ContextInfo {
            target,
            held: around.target.is_some(),
            index: around.target,
        });
        cx.emit(LogsModelEvent::Reset);
        if let Some(index) = around.target {
            cx.emit(LogsModelEvent::Reveal(index));
        }
        cx.notify();
    }
}

/// The histogram before its first read: no bars and no labels.
fn empty_bars() -> Bars {
    Bars {
        bars: vec![(0.0, 0.0); HISTOGRAM_BUCKETS],
        labels: Default::default(),
    }
}

/// Everything a read needs, copied out of the model so it can run on another
/// thread.
struct ReadPlan {
    store: Store,
    range: TimeRange,
    levels: Option<LevelSet>,
    query: Option<Arc<LogQuery>>,
    cursor: Option<String>,
    limit: usize,
}

/// What the histogram's read found.
struct HistogramRead {
    buckets: Vec<HistogramBucket>,
    since: u64,
    until: u64,
}

impl ReadPlan {
    /// The filter of the search, for the window that ends now.
    fn filter(&self, now: u64) -> LogFilter {
        LogFilter {
            since_unix_nano: self.range.since(now),
            levels: self.levels,
            query: self.query.clone(),
            ..LogFilter::default()
        }
    }

    /// One page of the search, newest first.
    fn read(&self) -> Page<(LogId, LogRecord)> {
        let filter = self.filter(self.store.now_unix_nano());
        self.store.search_logs_keyed(
            &filter,
            &PageRequest {
                limit: self.limit,
                cursor: self.cursor.clone(),
            },
        )
    }

    /// The histogram of the lines that match, over the range up to now. Under
    /// `Everything` it starts at the oldest line held.
    fn read_histogram(&self) -> HistogramRead {
        let until = self.store.now_unix_nano();
        let since = self
            .range
            .since(until)
            .or_else(|| self.store.oldest_log_unix_nano())
            .unwrap_or(until)
            .min(until);
        let filter = self.filter(until);
        HistogramRead {
            buckets: self
                .store
                .log_histogram(&filter, since, until, HISTOGRAM_BUCKETS),
            since,
            until,
        }
    }
}
