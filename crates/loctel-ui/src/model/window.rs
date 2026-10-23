//! Where a list that is too long to hold its rows is scrolled to: the Logs
//! screen at ten million lines (Milestone 5, Task 7).
//!
//! A list of 10,000,000 rows of 30 px is 300 million pixels tall. GPUI's pixels
//! are `f32`, which tells a pixel from the next only up to about 16 million, so a
//! scroll offset in pixels cannot say which row is on top. [`RowWindow`] counts in
//! rows instead: the position of the top row is a `u64`, the scrollbar's fraction
//! an `f64`, and a pixel only ever means "a bit of one row" (the sub-row
//! remainder of a wheel turn, which accumulates until it makes a whole row). It
//! is pure: it holds no rows, only where the window over them is, and the model
//! that owns it reads the rows of [`visible`](RowWindow::visible) from the store
//! (`Store::logs_at`).
//!
//! The list scrolls by whole rows, so a row is never cut by the top of the
//! viewport; the row under the top line is [`top`](RowWindow::top).

use std::ops::Range;

use loctel_store::LogId;

/// The shortest the scrollbar's thumb is, in pixels, however many rows there
/// are: a thumb of a pixel could not be seen or grabbed.
pub const MIN_THUMB_PX: f32 = 24.0;

/// What a change of the total keeps still ([`RowWindow::set_total`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// The window stays at the newest rows: the top is row 0 whatever the
    /// total. The live tail of a list that is at its top.
    Top,
    /// This row stays under the top line. The list is newest first, so a total
    /// that grows grew at the top: the new rows are above the row, and the
    /// window moves down by as many rows as were added. A total that shrank
    /// lost its oldest rows, which are below, so the window does not move. The
    /// caller says which row it was, and corrects the position when the rows it
    /// reads say the row is somewhere else (a late line, a window that lost
    /// lines at its old end while new ones arrived).
    Row(LogId),
    /// The same share of the way down: a list at the middle stays at the
    /// middle. For a total that changed for reasons that say nothing about
    /// where a row went.
    Fraction,
}

/// The window over a list of `total` rows of `row_px` pixels, in a viewport of
/// `viewport_px`. Everything is in rows, `u64` and `f64`.
#[derive(Clone, Debug, PartialEq)]
pub struct RowWindow {
    total: u64,
    row_px: f64,
    viewport_px: f64,
    top: u64,
    /// What a wheel has turned that is not a whole row yet, in pixels.
    carry_px: f64,
}

impl RowWindow {
    /// A window at the top of `total` rows. A row of less than a pixel counts as
    /// one pixel, and a viewport that is not there yet as no pixels (a page is
    /// at least one row).
    pub fn new(total: u64, row_px: f32, viewport_px: f32) -> RowWindow {
        RowWindow {
            total,
            row_px: f64::from(row_px).max(1.0),
            viewport_px: f64::from(viewport_px).max(0.0),
            top: 0,
            carry_px: 0.0,
        }
    }

    /// How many rows the list has.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// The row on top: the position of the first row of the viewport, counted
    /// from the newest, which is 0.
    pub fn top(&self) -> u64 {
        self.top
    }

    /// The height of a row, in pixels.
    pub fn row_px(&self) -> f32 {
        self.row_px as f32
    }

    /// The height of the viewport, in pixels.
    pub fn viewport_px(&self) -> f32 {
        self.viewport_px as f32
    }

    /// How many rows fit whole in the viewport; at least 1.
    pub fn page_rows(&self) -> u64 {
        ((self.viewport_px / self.row_px).floor() as u64).max(1)
    }

    /// How many rows the viewport shows when it is full: those that fit and the
    /// part of one that does not; at least 1.
    pub fn drawn_rows(&self) -> u64 {
        ((self.viewport_px / self.row_px).ceil() as u64).max(1)
    }

    /// The highest the top can be: the one that puts the last row at the bottom
    /// of the viewport.
    pub fn max_top(&self) -> u64 {
        self.total.saturating_sub(self.page_rows())
    }

    /// The positions of the rows in the viewport, `top..` as many as it shows
    /// and the list has.
    pub fn visible(&self) -> Range<u64> {
        let end = self.top.saturating_add(self.drawn_rows()).min(self.total);
        self.top..end
    }

    /// How far down the window is, from 0 (the top) to 1 (the end).
    pub fn fraction(&self) -> f64 {
        match self.max_top() {
            0 => 0.0,
            max => self.top as f64 / max as f64,
        }
    }

    /// Scrolls by `dy` pixels, which are rows of `row_px` each: positive shows
    /// later (older) rows, negative earlier ones. A part of a row is kept and
    /// added to the next turn, so a wheel that turns 10 px at a time moves a row
    /// every third turn of a 30 px row, and a list is never stuck for want of a
    /// whole row. At either end the remainder is dropped.
    pub fn scroll_px(&mut self, dy: f32) {
        let moved = self.carry_px + f64::from(dy);
        let rows = (moved / self.row_px).trunc();
        self.carry_px = moved - rows * self.row_px;
        // `as` saturates, so a wild delta is only a long way.
        self.move_rows(rows as i64);
    }

    /// Scrolls by `rows` rows (negative goes up), dropping any remainder of a
    /// turn.
    pub fn scroll_rows(&mut self, rows: i64) {
        self.carry_px = 0.0;
        self.move_rows(rows);
    }

    /// The scrollbar's jump: the window at `f` of the way down, 0 the top and 1
    /// the end. A fraction outside 0 to 1 is the nearest end, and one that is not
    /// a number is the top.
    pub fn jump_fraction(&mut self, f: f64) {
        let f = if f.is_nan() { 0.0 } else { f.clamp(0.0, 1.0) };
        self.carry_px = 0.0;
        self.top = ((self.max_top() as f64) * f).round() as u64;
        self.top = self.top.min(self.max_top());
    }

    /// Puts the top at `top` (the end, when that is past it).
    pub fn set_top(&mut self, top: u64) {
        self.carry_px = 0.0;
        self.top = top.min(self.max_top());
    }

    /// The list has `total` rows now. `keep` says what stays still: see
    /// [`Anchor`]. The top never ends up past the end.
    pub fn set_total(&mut self, total: u64, keep: Anchor) {
        let before = self.total;
        let fraction = self.fraction();
        self.total = total;
        match keep {
            Anchor::Top => {
                self.top = 0;
                self.carry_px = 0.0;
            }
            Anchor::Row(_) => {
                if total > before {
                    self.top = self.top.saturating_add(total - before);
                }
            }
            Anchor::Fraction => {
                self.top = ((self.max_top() as f64) * fraction).round() as u64;
                self.carry_px = 0.0;
            }
        }
        self.top = self.top.min(self.max_top());
    }

    /// The viewport is `viewport_px` tall now (the window was resized). The top
    /// stays unless it would be past the end.
    pub fn set_viewport_px(&mut self, viewport_px: f32) {
        self.viewport_px = f64::from(viewport_px).max(0.0);
        self.top = self.top.min(self.max_top());
    }

    /// The scrollbar's thumb as `(start, length)`, both fractions of the track,
    /// which is as tall as the viewport. The length is the share of the rows the
    /// viewport shows, but never under [`MIN_THUMB_PX`], and the start puts the
    /// thumb at the top of the track for the first row and at the bottom for the
    /// last. A list that fits is one thumb the length of the track.
    pub fn thumb(&self) -> (f32, f32) {
        if self.total <= self.page_rows() {
            return (0.0, 1.0);
        }
        let shown = self.page_rows() as f64 / self.total as f64;
        let least = (f64::from(MIN_THUMB_PX) / self.viewport_px.max(1.0)).min(1.0);
        let length = shown.max(least).min(1.0);
        let start = self.fraction() * (1.0 - length);
        (start as f32, length as f32)
    }

    /// The fraction to [`jump_fraction`](Self::jump_fraction) to so that the
    /// middle of the thumb is under a pointer `at` of the way down the track
    /// (0 to 1): what a click or a drag on the track means.
    pub fn fraction_for_thumb_centre(&self, at: f64) -> f64 {
        let (_, length) = self.thumb();
        let length = f64::from(length);
        if length >= 1.0 {
            return 0.0;
        }
        ((at - length / 2.0) / (1.0 - length)).clamp(0.0, 1.0)
    }

    /// Moves the top by `rows`, within the list; a move that hits an end drops
    /// the remainder of a turn.
    fn move_rows(&mut self, rows: i64) {
        let wanted = i128::from(self.top) + i128::from(rows);
        let max = i128::from(self.max_top());
        let clamped = wanted.clamp(0, max);
        if clamped != wanted {
            self.carry_px = 0.0;
        }
        self.top = clamped as u64;
    }
}
