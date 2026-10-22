//! The Logs screen (design 9.2, M4 plan Task 4): the LogQL bar, the level
//! toggles, the volume histogram and the list of lines.
//!
//! [`LogsView`] is two panels, like the home screen. The top one holds, from the
//! top:
//!
//! 1. the search: a `LogQL` chip, the input and `Run`, with the error line
//!    (`at column N: message`) under them while the text does not parse
//! 2. the four level toggles (`ERR`, `WRN`, `INF`, `DBG`; one that is off has no
//!    fill) and, on the right, the count (`207 lines · newest first`,
//!    `500+ lines · newest first`, `20,000+ lines`)
//! 3. the [`HistogramView`]
//!
//! The bottom one is the list: a row of 30 px per line (the time in the monospace
//! face, the level in a badge, the service with its colour dot, the message in
//! monospace on one line with an ellipsis, and the first 8 digits of the trace ID
//! as a link). Over it float the `New lines · Jump to newest` pill while the
//! live tail is paused by scrolling, and, in context mode, a banner on top:
//! `Lines around 14:32:09.194 · Back to search`. Under it, once the list stopped
//! at 20,000 rows, the footer.
//!
//! The toolbar of the screen (the `Live tail` pill, the range and `Clear all`) is
//! [`LogsControls`](crate::app::LogsControls), which this view owns
//! ([`controls`](LogsView::controls)) and the shell puts in its glass toolbar.
//!
//! **A click on a row expands it** where it stands: the row is taller by the
//! details (the whole body, wrapped, then the labels) and three buttons, `Open
//! trace`, `Show lines around this one` and `Discard lines like this…`. The
//! list is GPUI's variable-height `list` for that, and this view keeps its
//! state right as the model reports what happened to the rows
//! ([`LogsModelEvent`]). A click on another row moves the expansion; a click on
//! the trace link opens the trace without expanding.
//!
//! The view holds no data: the rows, the toggles and the query's state are the
//! [`LogsModel`]'s, and what the user does goes back into it. What the view tells
//! the model is where the list is: at the top or scrolled away, which pauses and
//! resumes the live tail. What it asks of the window it emits as
//! [`LogsViewEvent`].
//!
//! Rows within [`PREFETCH_ROWS`] of the end of the loaded rows ask the model for
//! the next page as they are drawn.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, Div, Entity, EventEmitter, FontWeight,
    InteractiveElement as _, IntoElement, ListAlignment, ListOffset, ListState, ParentElement as _,
    Pixels, Point, Render, Rgba, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, canvas, div, list, point, prelude::FluentBuilder as _, px,
};
use gpui_kit::base::{Scrollbar, ScrollbarAxis};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use loctel_model::{SpanId, TraceId};

use super::histogram_view::HistogramView;
use crate::Theme;
use crate::app::{LogsControls, LogsModel, LogsModelEvent, PANEL_RADIUS};
use crate::model::logs::{
    BACK_TO_SEARCH, EXAMPLE_QUERY, LevelBadge, LogRow, NEW_LINES_PILL, PREFETCH_ROWS, error_line,
    level_is_on, levels_with,
};
use crate::model::rules::{Draft, suggest_from_log};

/// What the list says when nothing is filtered and there are no logs.
pub const LOGS_EMPTY_NO_DATA: &str = "No logs yet";
/// What it says when the search leaves nothing.
pub const LOGS_EMPTY_NO_MATCH: &str = "No lines match";

/// The words of the three buttons of an expanded line.
pub const OPEN_TRACE: &str = "Open trace";
pub const SHOW_AROUND: &str = "Show lines around this one";
pub const DISCARD_LIKE_THIS: &str = "Discard lines like this\u{2026}";

/// The height of a row, in pixels.
pub const ROW_HEIGHT: f32 = 30.0;
/// The widths of the columns that do not share what is left, in pixels: the
/// message has the rest.
pub const TIME_WIDTH: f32 = 96.0;
pub const LEVEL_WIDTH: f32 = 44.0;
pub const SERVICE_WIDTH: f32 = 112.0;
pub const TRACE_WIDTH: f32 = 96.0;
/// The padding of the panels' sides, and the gap between the columns.
const PAD_X: f32 = 16.0;
const COLUMN_GAP: f32 = 12.0;
/// How far outside the list the rows are measured, in pixels.
const OVERDRAW: f32 = 300.0;
/// The width of a level badge, and of the service dot.
const BADGE_WIDTH: f32 = 36.0;
const DOT_SIZE: f32 = 8.0;
/// The width of the labels' column in an expanded line's grid.
const LABEL_WIDTH: f32 = 190.0;

fn rgba(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

/// The parts of the Logs screen a test can ask the place of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LogsPart {
    /// The LogQL input.
    QueryInput,
    /// The `Run` button.
    Run,
    /// A level toggle.
    Level(LevelBadge),
    /// The histogram.
    Histogram,
    /// The list (its viewport).
    List,
    /// The `New lines · Jump to newest` pill; present only while the tail is
    /// paused by scrolling.
    Pill,
    /// The context banner; present only in context mode.
    Banner,
    /// The `Back to search` button of the banner.
    BackToSearch,
    /// The footer of a list that stopped at 20,000 rows.
    Footer,
    /// The `Open trace` button of the expanded line; present only for a line in
    /// a trace.
    OpenTrace,
    /// The `Show lines around this one` button of the expanded line.
    ShowAround,
    /// The `Discard lines like this…` button of the expanded line.
    Discard,
}

/// Where the parts of the latest frame are, in window coordinates.
type Frame = RefCell<HashMap<LogsPart, Bounds<Pixels>>>;

/// An invisible canvas over its parent that writes the parent's bounds into
/// `frame` while the frame is prepainted. Add it as a child of a `relative` box
/// that is the size of the part.
fn measure(frame: &Rc<Frame>, part: LogsPart) -> impl IntoElement {
    let frame = Rc::clone(frame);
    canvas(
        move |bounds, _window, _cx| {
            frame.borrow_mut().insert(part, bounds);
        },
        |_, (), _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// What the Logs screen asks of the window.
#[derive(Clone, Debug, PartialEq)]
pub enum LogsViewEvent {
    /// Show the trace a log belongs to, with the span it was emitted in
    /// selected.
    OpenTrace {
        trace: TraceId,
        span: Option<SpanId>,
    },
    /// Open the discard rules screen with this rule in the editor, unsaved.
    Discard(Draft),
}

/// The Logs screen. Make it with `cx.new(|cx| LogsView::new(model, window, cx))`;
/// it reads and writes `model`.
pub struct LogsView {
    model: Entity<LogsModel>,
    input: Entity<InputState>,
    histogram: Entity<HistogramView>,
    controls: Entity<LogsControls>,
    list: ListState,
    frame: Rc<Frame>,
    /// Keeps the observation of the model, its events and the input alive.
    _subscriptions: [Subscription; 3],
}

impl EventEmitter<LogsViewEvent> for LogsView {}

impl LogsView {
    pub fn new(model: Entity<LogsModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(EXAMPLE_QUERY));
        let histogram = cx.new(|cx| HistogramView::new(model.clone(), cx));
        let controls = cx.new(|cx| LogsControls::new(model.clone(), cx));
        let rows = model.read(cx).rows().len();
        let list = ListState::new(rows, ListAlignment::Top, px(OVERDRAW))
            .with_uniform_item_height(px(ROW_HEIGHT));
        let subscriptions = [
            cx.observe_in(&model, window, |view, model, window, cx| {
                view.model_changed(&model, window, cx);
            }),
            cx.subscribe(&model, |view, _model, event: &LogsModelEvent, cx| {
                view.rows_changed(event, cx);
            }),
            cx.subscribe_in(
                &input,
                window,
                |view, input, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => {
                        let text = input.read(cx).value().to_string();
                        view.model.update(cx, |model, cx| {
                            // Not an echo of what the model just put there.
                            if model.query_text() != text {
                                model.set_query(&text, cx);
                            }
                        });
                    }
                    InputEvent::PressEnter { .. } => view.run(cx),
                    InputEvent::Focus | InputEvent::Blur => {}
                },
            ),
        ];
        LogsView {
            model,
            input,
            histogram,
            controls,
            list,
            frame: Rc::new(Frame::default()),
            _subscriptions: subscriptions,
        }
    }

    /// The controls of the screen's toolbar: the shell puts them in its glass
    /// toolbar while the Logs screen shows.
    pub fn controls(&self) -> &Entity<LogsControls> {
        &self.controls
    }

    /// The histogram under the toggles.
    pub fn histogram_view(&self) -> &Entity<HistogramView> {
        &self.histogram
    }

    /// Where `part` is in the window, as of the latest frame. `None` while it is
    /// not on the screen (the pill of a tail that runs, the buttons of a line
    /// that is not expanded).
    pub fn bounds_of(&self, part: LogsPart) -> Option<Bounds<Pixels>> {
        self.frame.borrow().get(&part).copied()
    }

    /// The middle of `part`, as of the latest frame.
    pub fn center_of(&self, part: LogsPart) -> Option<Point<Pixels>> {
        self.bounds_of(part).map(|bounds| bounds.center())
    }

    /// The middle of the row at `index`, as of the latest frame; `None` when the
    /// row has not been drawn (it is above the viewport, or not laid out yet).
    pub fn row_center(&self, index: usize) -> Option<Point<Pixels>> {
        self.list.bounds_for_item(index).map(|bounds| {
            // The row proper, not the details of an expanded one.
            point(bounds.center().x, bounds.top() + px(ROW_HEIGHT / 2.0))
        })
    }

    /// The middle of the trace link of the row at `index`, as of the latest
    /// frame. `None` when the row has not been drawn or is in no trace.
    pub fn trace_link_center(&self, index: usize) -> Option<Point<Pixels>> {
        let bounds = self.list.bounds_for_item(index)?;
        Some(point(
            bounds.right() - px(PAD_X + TRACE_WIDTH / 2.0),
            bounds.top() + px(ROW_HEIGHT / 2.0),
        ))
    }

    /// Where the list is scrolled to: the index of the first row in view and how
    /// many pixels of it are above the viewport. `(0, 0.0)` is the top.
    pub fn scroll_offset(&self) -> (usize, f32) {
        let top = self.list.logical_scroll_top();
        (top.item_ix, f32::from(top.offset_in_item))
    }

    /// The text in the LogQL input.
    pub fn input_text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// The line under the input: `at column N: message` while the text does not
    /// parse, `None` otherwise.
    pub fn error_line(&self, cx: &App) -> Option<String> {
        error_line(self.model.read(cx).state())
    }

    /// The context banner's words (`Lines around 08:00:01.440`, or `This line is
    /// no longer held`), `None` outside context mode.
    pub fn banner(&self, cx: &App) -> Option<String> {
        self.model.read(cx).banner()
    }

    /// What an empty list says ([`LOGS_EMPTY_NO_DATA`] or
    /// [`LOGS_EMPTY_NO_MATCH`]), `None` while it has rows or has not read yet.
    pub fn empty_state(&self, cx: &App) -> Option<&'static str> {
        let model = self.model.read(cx);
        if !model.rows().is_empty() || !model.loaded() {
            return None;
        }
        Some(if model.has_filters() {
            LOGS_EMPTY_NO_MATCH
        } else {
            LOGS_EMPTY_NO_DATA
        })
    }

    /// Reads the search again from the first page: what Enter and `Run` do.
    fn run(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| model.refresh_now(cx));
    }

    /// The model changed: when something else than the input changed the text
    /// (`Open in Logs`), the input follows.
    fn model_changed(
        &mut self,
        model: &Entity<LogsModel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let wanted = model.read(cx).query_text().to_owned();
        if self.input.read(cx).value() != wanted.as_str() {
            self.input
                .update(cx, |input, cx| input.set_value(wanted, window, cx));
        }
        cx.notify();
    }

    /// The rows changed: the list's count, and what it measured, follow.
    fn rows_changed(&mut self, event: &LogsModelEvent, cx: &mut Context<Self>) {
        match *event {
            LogsModelEvent::Reset => {
                let rows = self.model.read(cx).rows().len();
                self.list.reset_with_uniform_height(rows, px(ROW_HEIGHT));
            }
            LogsModelEvent::Prepended(added) => {
                let at_top = self.at_top();
                self.list.splice(0..0, added);
                // A list that was scrolled and came back to the top keeps its
                // place under the new rows (a splice keeps what the viewport
                // shows); the tail is there to show them, so it goes to the top.
                // A list the user has just scrolled away from, in the frame
                // before the model hears of it, is left exactly where it is.
                if at_top {
                    self.to_top();
                }
                self.hint_heights();
            }
            LogsModelEvent::Appended(added) => {
                let end = self.list.item_count();
                self.list.splice(end..end, added);
                self.hint_heights();
            }
            LogsModelEvent::Truncated(len) => {
                let end = self.list.item_count();
                if len < end {
                    self.list.splice(len..end, 0);
                }
            }
            LogsModelEvent::Resized(index) => self.list.remeasure_items(index..index + 1),
            LogsModelEvent::Reveal(index) => {
                // About the middle of the viewport; the first frame has not
                // measured it, so a screenful is assumed.
                let viewport = f32::from(self.list.viewport_bounds().size.height);
                let rows_in_view = if viewport > 0.0 {
                    (viewport / ROW_HEIGHT) as usize
                } else {
                    16
                };
                self.list.scroll_to(ListOffset {
                    item_ix: index.saturating_sub(rows_in_view / 2),
                    offset_in_item: px(0.0),
                });
            }
            LogsModelEvent::Top => self.to_top(),
        }
        cx.notify();
    }

    /// Whether the list shows its very top.
    fn at_top(&self) -> bool {
        let offset = self.list.logical_scroll_top();
        offset.item_ix == 0 && offset.offset_in_item <= px(0.5)
    }

    fn to_top(&self) {
        self.list.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.0),
        });
    }

    /// Gives the rows a splice added the height they will most likely have, so
    /// the scrollbar and the length of the list are right before they are drawn.
    fn hint_heights(&self) {
        let _ = self.list.clone().with_uniform_item_height(px(ROW_HEIGHT));
    }
}

impl Render for LogsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame.borrow_mut().clear();
        let theme = *cx.global::<Theme>();

        // The list's count is the model's: a safety net for any event this view
        // has not mirrored, so the list can never ask for a row that is not
        // there.
        let rows = self.model.read(cx).rows().len();
        if self.list.item_count() != rows {
            self.list.reset_with_uniform_height(rows, px(ROW_HEIGHT));
        }

        // Tell the model where the list is. Scrolling away from the top pauses
        // the tail and scrolling back resumes it. The model is updated after
        // this frame, not during it.
        let at_top = self.at_top();
        if self.model.read(cx).scrolled_away() == at_top {
            cx.defer_in(window, move |view, _window, cx| {
                view.model
                    .update(cx, |model, cx| model.set_at_top(at_top, cx));
            });
        }

        let (state_error, levels, count, paused, banner, empty, footer) = {
            let model = self.model.read(cx);
            (
                error_line(model.state()),
                model.levels(),
                model.count_line(),
                model.tail_paused(),
                model.banner(),
                self.empty_state(cx),
                model.footer(),
            )
        };
        let frame = &self.frame;

        // --- the search panel ----------------------------------------------------
        let chip = div()
            .flex_none()
            .h(px(24.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .rounded(px(7.0))
            .bg(rgba(theme.accent, 0.16))
            .text_color(theme.accent)
            .text_size(px(12.0))
            .font_weight(FontWeight::SEMIBOLD)
            .child("LogQL");

        let input = div()
            .relative()
            .flex_1()
            .min_w_0()
            .font_family(cx.theme().mono_font_family.clone())
            .child(Input::new(&self.input).cleanable(true))
            .child(measure(frame, LogsPart::QueryInput));

        let run = div()
            .relative()
            .flex_none()
            .child(
                Button::new("logs-run")
                    .label("Run")
                    .primary()
                    .on_click(cx.listener(|view, _, _, cx| view.run(cx))),
            )
            .child(measure(frame, LogsPart::Run));

        let query_row = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(PAD_X))
            .pt(px(14.0))
            .child(chip)
            .child(input)
            .child(run);

        let error_row = state_error.map(|line| {
            div()
                .flex_none()
                .px(px(PAD_X))
                .pt(px(6.0))
                .text_size(px(12.0))
                .text_color(theme.error_text)
                .child(line)
        });

        let toggles = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(PAD_X))
            .py(px(12.0))
            .children(LevelBadge::ALL.into_iter().map(|badge| {
                level_toggle(
                    &theme,
                    frame,
                    badge,
                    level_is_on(levels, badge),
                    self.model.clone(),
                )
            }))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.text_secondary)
                    .child(count),
            );

        let histogram = div()
            .relative()
            .flex_none()
            .px(px(PAD_X))
            .pb(px(12.0))
            .child(self.histogram.clone())
            .child(measure(frame, LogsPart::Histogram));

        let search = panel(&theme)
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .child(query_row)
            .children(error_row)
            .child(toggles)
            .child(histogram);

        // --- the list panel --------------------------------------------------------
        let banner_row = banner.map(|text| {
            div()
                .relative()
                .flex_none()
                .h(px(40.0))
                .px(px(PAD_X))
                .flex()
                .items_center()
                .gap(px(8.0))
                .bg(rgba(theme.accent, 0.10))
                .border_b_1()
                .border_color(theme.border)
                .text_size(px(12.5))
                .text_color(theme.text)
                .child(text)
                .child(div().text_color(theme.text_tertiary).child("\u{b7}"))
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .child(
                            Button::new("logs-back-to-search")
                                .label(BACK_TO_SEARCH)
                                .small()
                                .ghost()
                                .on_click({
                                    let model = self.model.clone();
                                    move |_, _window, cx| {
                                        model.update(cx, |model, cx| model.back_to_search(cx));
                                    }
                                }),
                        )
                        .child(measure(frame, LogsPart::BackToSearch)),
                )
                .child(measure(frame, LogsPart::Banner))
        });

        let rows_list = {
            let model = self.model.clone();
            let view = cx.entity();
            let frame = Rc::clone(frame);
            let mono = cx.theme().mono_font_family.clone();
            list(self.list.clone(), move |index, _window, cx| {
                render_row(index, &model, &view, &frame, &mono, cx)
            })
            .size_full()
        };

        let list_area = div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(rows_list)
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::new(&self.list)
                        .id("logs-scrollbar")
                        .axis(ScrollbarAxis::Vertical)
                        .viewport_from_layout(),
                ),
            )
            .when_some(empty, |area, text| {
                area.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_secondary)
                        .child(text),
                )
            })
            .when(paused, |area| {
                area.child(
                    div()
                        .absolute()
                        .top(px(10.0))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(pill(&theme, frame, self.model.clone())),
                )
            })
            .child(measure(frame, LogsPart::List));

        let footer_row = footer.map(|text| {
            div()
                .relative()
                .flex_none()
                .h(px(30.0))
                .flex()
                .items_center()
                .justify_center()
                .border_t_1()
                .border_color(theme.border)
                .text_size(px(12.0))
                .text_color(theme.text_secondary)
                .child(text)
                .child(measure(frame, LogsPart::Footer))
        });

        let list_panel = panel(&theme)
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .children(banner_row)
            .child(list_area)
            .children(footer_row);

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(crate::app::INSET))
            .child(search)
            .child(list_panel)
    }
}

/// A floating content panel: rounded, with the panel fill and a hairline.
fn panel(theme: &Theme) -> Div {
    div()
        .relative()
        .rounded(px(PANEL_RADIUS))
        .bg(theme.panel)
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
}

/// One of the four level toggles: its three letters in the level's colour. On,
/// it has a tint of the colour and a ring of it; off, no fill and a hairline.
fn level_toggle(
    theme: &Theme,
    frame: &Rc<Frame>,
    badge: LevelBadge,
    on: bool,
    model: Entity<LogsModel>,
) -> impl IntoElement {
    let ink = level_ink(theme, badge);
    div()
        .relative()
        .flex_none()
        .child(
            div()
                .id(("logs-level", badge as usize))
                .h(px(24.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .rounded(px(7.0))
                .border_1()
                .cursor_pointer()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ink)
                .when(on, |toggle| {
                    toggle.bg(rgba(ink, 0.16)).border_color(rgba(ink, 0.55))
                })
                .when(!on, |toggle| toggle.border_color(theme.border))
                .on_click(move |_, _window, cx| {
                    model.update(cx, |model, cx| {
                        let levels = levels_with(model.levels(), badge, !on);
                        model.set_levels(levels, cx);
                    });
                })
                .child(badge.label()),
        )
        .child(measure(frame, LogsPart::Level(badge)))
}

/// The colour of a level's letters, on the toggle and in the badge of a row.
fn level_ink(theme: &Theme, badge: LevelBadge) -> Rgba {
    match badge {
        LevelBadge::Err => theme.error_text,
        LevelBadge::Wrn => theme.warn,
        LevelBadge::Inf => theme.text_secondary,
        LevelBadge::Dbg => theme.text_tertiary,
    }
}

/// The colour of a level's badge fill.
fn level_fill(theme: &Theme, badge: LevelBadge) -> Rgba {
    match badge {
        LevelBadge::Err => rgba(theme.error, 0.16),
        LevelBadge::Wrn => rgba(theme.warn, 0.16),
        LevelBadge::Inf => rgba(theme.text_secondary, 0.14),
        LevelBadge::Dbg => rgba(theme.text_tertiary, 0.14),
    }
}

/// The `New lines · Jump to newest` pill: a click brings back the live tail.
fn pill(theme: &Theme, frame: &Rc<Frame>, model: Entity<LogsModel>) -> impl IntoElement {
    div()
        .relative()
        .flex_none()
        .child(
            div()
                .id("logs-pill")
                .h(px(28.0))
                .px(px(14.0))
                .flex()
                .items_center()
                .rounded(px(14.0))
                .bg(theme.accent)
                .text_color(theme.bg)
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .on_click(move |_, _window, cx| {
                    model.update(cx, |model, cx| model.jump_to_newest(cx));
                })
                .child(NEW_LINES_PILL),
        )
        .child(measure(frame, LogsPart::Pill))
}

/// One row of the list, with the details under it when it is the expanded one.
/// It asks the model for the next page when it is within [`PREFETCH_ROWS`] of
/// the end of the loaded rows.
fn render_row(
    index: usize,
    model: &Entity<LogsModel>,
    view: &Entity<LogsView>,
    frame: &Rc<Frame>,
    mono: &SharedString,
    cx: &mut App,
) -> AnyElement {
    let theme = *cx.global::<Theme>();
    let (row, details, record, target, total) = {
        let logs = model.read(cx);
        let Some(row) = logs.rows().get(index).cloned() else {
            return div().into_any_element();
        };
        let expanded = logs.expanded() == Some(row.id);
        (
            row,
            expanded.then(|| logs.expanded_details()).flatten(),
            expanded.then(|| logs.record(index).cloned()).flatten(),
            logs.context_target() == Some(index),
            logs.rows().len(),
        )
    };
    if index + PREFETCH_ROWS >= total {
        model.update(cx, |model, cx| model.load_more(cx));
    }

    let id = row.id;
    let open = details.is_some();
    let head = line(&theme, &row, index, mono, target, open, view).on_click({
        let model = model.clone();
        move |_, _window, cx| {
            model.update(cx, |model, cx| {
                let next = if model.expanded() == Some(id) {
                    None
                } else {
                    Some(id)
                };
                model.expand(next, cx);
            });
        }
    });
    match (details, record) {
        (Some(details), Some(record)) => div()
            .w_full()
            .flex()
            .flex_col()
            .child(head)
            .child(expanded_part(
                &theme, &row, &details, &record, index, mono, model, view, frame,
            ))
            .into_any_element(),
        _ => div().w_full().child(head).into_any_element(),
    }
}

/// The line of a row: the five columns.
fn line(
    theme: &Theme,
    row: &LogRow,
    index: usize,
    mono: &SharedString,
    target: bool,
    open: bool,
    view: &Entity<LogsView>,
) -> gpui::Stateful<Div> {
    let palette = theme.service_palette[row.color % theme.service_palette.len()];
    let badge = div().flex_none().w(px(LEVEL_WIDTH)).child(
        div()
            .w(px(BADGE_WIDTH))
            .h(px(16.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .bg(level_fill(theme, row.level))
            .text_color(level_ink(theme, row.level))
            .text_size(px(10.0))
            .font_weight(FontWeight::SEMIBOLD)
            .child(row.level.label()),
    );

    let trace = {
        let cell = div()
            .id(("log-trace", index))
            .flex_none()
            .w(px(TRACE_WIDTH))
            .font_family(mono.clone())
            .text_size(px(12.0));
        match (&row.trace_short, row.trace_id) {
            (Some(short), Some(trace)) => {
                let span = row.span_id;
                let view = view.clone();
                cell.text_color(theme.accent)
                    .cursor_pointer()
                    .hover(|style| style.underline())
                    .on_click(move |_, _window, cx| {
                        // The link opens the trace; the row is not expanded too.
                        cx.stop_propagation();
                        view.update(cx, |_, cx| {
                            cx.emit(LogsViewEvent::OpenTrace { trace, span });
                        });
                    })
                    .child(short.clone())
            }
            _ => cell,
        }
    };

    let hover = rgba(theme.text, 0.05);
    div()
        .id(("log-row", index))
        .w_full()
        .h(px(ROW_HEIGHT))
        .px(px(PAD_X))
        .flex()
        .items_center()
        .gap(px(COLUMN_GAP))
        .cursor_pointer()
        .when(target, |row| row.bg(rgba(theme.accent, 0.18)))
        .when(open && !target, |row| row.bg(rgba(theme.accent, 0.08)))
        .when(!target && !open, |row| {
            row.hover(move |style| style.bg(hover))
        })
        .child(
            div()
                .flex_none()
                .w(px(TIME_WIDTH))
                .font_family(mono.clone())
                .text_size(px(12.0))
                .text_color(theme.text_tertiary)
                .child(row.time.clone()),
        )
        .child(badge)
        .child(
            div()
                .flex_none()
                .w(px(SERVICE_WIDTH))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .size(px(DOT_SIZE))
                        .flex_none()
                        .rounded_full()
                        .bg(palette),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.5))
                        .text_color(theme.text_secondary)
                        .child(row.service.clone()),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(mono.clone())
                .text_size(px(12.0))
                .text_color(theme.text)
                .child(row.message.clone()),
        )
        .child(trace)
}

/// The part of an expanded line under its row: the grid of the whole body and the
/// labels, and the three buttons.
#[allow(clippy::too_many_arguments)]
fn expanded_part(
    theme: &Theme,
    row: &LogRow,
    details: &[(String, String)],
    record: &loctel_model::LogRecord,
    index: usize,
    mono: &SharedString,
    model: &Entity<LogsModel>,
    view: &Entity<LogsView>,
    frame: &Rc<Frame>,
) -> impl IntoElement {
    let grid = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .children(details.iter().map(|(label, value)| {
            div()
                .w_full()
                .flex()
                .gap(px(COLUMN_GAP))
                .child(
                    div()
                        .flex_none()
                        .w(px(LABEL_WIDTH))
                        .text_size(px(11.5))
                        .text_color(theme.text_tertiary)
                        .child(label.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .font_family(mono.clone())
                        .text_size(px(12.0))
                        .text_color(if label == "body" {
                            theme.text
                        } else {
                            theme.text_secondary
                        })
                        .child(value.clone()),
                )
        }));

    let id = row.id;
    let open_trace = row.trace_id.map(|trace| {
        let span = row.span_id;
        let view = view.clone();
        div()
            .relative()
            .flex_none()
            .child(
                Button::new(("logs-open-trace", index))
                    .label(OPEN_TRACE)
                    .small()
                    .outline()
                    .on_click(move |_, _window, cx| {
                        view.update(cx, |_, cx| {
                            cx.emit(LogsViewEvent::OpenTrace { trace, span });
                        });
                    }),
            )
            .child(measure(frame, LogsPart::OpenTrace))
    });
    let show_around = div()
        .relative()
        .flex_none()
        .child(
            Button::new(("logs-show-around", index))
                .label(SHOW_AROUND)
                .small()
                .outline()
                .on_click({
                    let model = model.clone();
                    move |_, _window, cx| {
                        model.update(cx, |model, cx| model.show_around(id, cx));
                    }
                }),
        )
        .child(measure(frame, LogsPart::ShowAround));
    let discard = div()
        .relative()
        .flex_none()
        .child(
            Button::new(("logs-discard", index))
                .label(DISCARD_LIKE_THIS)
                .small()
                .outline()
                .on_click({
                    let view = view.clone();
                    let draft = suggest_from_log(record);
                    move |_, _window, cx| {
                        let draft = draft.clone();
                        view.update(cx, |_, cx| cx.emit(LogsViewEvent::Discard(draft)));
                    }
                }),
        )
        .child(measure(frame, LogsPart::Discard));

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .px(px(PAD_X))
        .py(px(12.0))
        .bg(rgba(theme.text, 0.03))
        .border_t_1()
        .border_b_1()
        .border_color(theme.border)
        .child(grid)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .children(open_trace)
                .child(show_around)
                .child(discard),
        )
}
