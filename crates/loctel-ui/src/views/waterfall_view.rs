//! The waterfall of the trace view (design 9.2, M3 plan Task 8): the Find box
//! and the two toggles over it, the ruler and a `uniform_list` of 28 px rows,
//! one per span, in tree order.
//!
//! [`WaterfallView`] holds no data: the rows, the selection and what is folded
//! are the [`TraceModel`]'s (a [`Waterfall`](crate::model::waterfall::Waterfall)
//! over the trace), and this view draws them and writes the user's clicks and
//! keystrokes back. What it keeps is the find input's text (the input owns it),
//! the list's scroll position and where its parts were laid out in the latest
//! frame, for tests.
//!
//! A row, left to right:
//!
//! | Part | What |
//! |---|---|
//! | label | indent `6 + depth × 16`, a chevron (click folds the children), a 3 px bar in the service's colour, a red `!` circle on an erroring span, the name (ellipsis) and, after it, the other side of a call in tertiary text |
//! | timeline | the bar at the span's offset and length, in the service's colour at 0.75 opacity (full for an erroring or the selected span; an erroring one also has a red outline), and a diamond at each `exception` event |
//! | duration | the span's length, right-aligned |
//!
//! The selected row is tinted with the accent at 18%; a row that matches the
//! find text has a faint amber tint. Nothing in a row can widen the layout: the
//! label column is a share of the width and cuts its text.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, Div, Entity, Focusable as _, FontWeight,
    Hsla, InteractiveElement as _, IntoElement, ParentElement as _, PathBuilder, Pixels, Point,
    Render, Rgba, ScrollStrategy, StatefulInteractiveElement as _, Styled as _, Subscription,
    UniformListScrollHandle, Window, canvas, div, point, prelude::FluentBuilder as _, px, relative,
    uniform_list,
};
use gpui_kit::base::{Scrollbar, ScrollbarAxis};
use gpui_kit::component::button::{Button, Toggle, ToggleVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, IconName, Sizable as _};

use super::trace_view::{Frame, TracePart, measure};
use crate::Theme;
use crate::app::{TraceModel, TraceModelEvent};
use crate::model::waterfall::WaterfallRow;

/// The height of a row, in pixels.
pub const ROW_HEIGHT: f32 = 28.0;
/// The height of the ruler under the toolbar, in pixels.
pub const RULER_HEIGHT: f32 = 24.0;
/// The share of a row's width the label (indent, name) takes; the rest is the
/// timeline and the duration.
const LABEL_SHARE: f32 = 0.42;
/// The width of the duration column.
const DURATION_WIDTH: f32 = 72.0;
/// The indent of a root row, and what each level adds.
const INDENT_BASE: f32 = 6.0;
const INDENT_STEP: f32 = 16.0;
/// The chevron's box and the icon in it.
const CHEVRON_BOX: f32 = 16.0;
const CHEVRON_ICON: f32 = 12.0;
/// The service colour bar of a row: width and height.
const COLOR_BAR_WIDTH: f32 = 3.0;
const COLOR_BAR_HEIGHT: f32 = 14.0;
/// The red circle of an erroring span.
const ERROR_ICON: f32 = 14.0;
/// The bar of a span: its height, and how far from the top of the row.
const BAR_HEIGHT: f32 = 12.0;
/// The least a bar is wide, in pixels, so a very short span is still seen.
const BAR_MIN_WIDTH: f32 = 2.0;
/// The opacity of a bar that is neither erroring nor selected.
const BAR_OPACITY: f32 = 0.75;
/// The diamond of an exception event: its size, and the ring round it.
const MARKER_SIZE: f32 = 9.0;
const MARKER_RING: f32 = 2.0;
/// The widest the note after a name is, in pixels (the name gets the rest).
const NOTE_MAX_WIDTH: f32 = 190.0;
/// How much faster the note shrinks than the name when the label is short of
/// room (flex-shrink factors; the name's is 1).
const NOTE_SHRINK: f32 = 4.0;
/// The width of the find box.
const FIND_WIDTH: f32 = 220.0;
/// The padding of the toolbar's and the ruler's sides.
const PAD_X: f32 = 12.0;

fn rgba(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

/// What the waterfall says when the errors filter leaves no row.
pub const NO_ERRORS: &str = "No span of this trace has an error";

/// The waterfall of the trace view. Make it with
/// `cx.new(|cx| WaterfallView::new(model, frame, window, cx))`.
pub struct WaterfallView {
    model: Entity<TraceModel>,
    find: Entity<InputState>,
    scroll: UniformListScrollHandle,
    frame: Rc<Frame>,
    /// Keeps the observation of the model and of the find input alive.
    _subscriptions: [Subscription; 3],
}

impl WaterfallView {
    pub fn new(model: Entity<TraceModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let find = cx.new(|cx| InputState::new(window, cx).placeholder("Find in trace"));
        let scroll = UniformListScrollHandle::new();
        let subscriptions = [
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.subscribe(&model, |view, model, event: &TraceModelEvent, cx| {
                let TraceModelEvent::Reveal(id) = event;
                let row = model
                    .read(cx)
                    .waterfall()
                    .and_then(|waterfall| waterfall.row_of(*id));
                if let Some(row) = row {
                    view.scroll_to_row(row);
                }
            }),
            cx.subscribe_in(
                &find,
                window,
                |view, find, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => {
                        let text = find.read(cx).value().to_string();
                        view.model.update(cx, |model, cx| model.set_find(&text, cx));
                    }
                    InputEvent::PressEnter { .. } => {
                        view.model.update(cx, |model, cx| model.find_next(cx));
                    }
                    InputEvent::Focus | InputEvent::Blur => {}
                },
            ),
        ];
        WaterfallView {
            model,
            find,
            scroll,
            frame: Rc::new(Frame::default()),
            _subscriptions: subscriptions,
        }
    }

    /// Gives the find input the keyboard (what ⌘F does).
    pub fn focus_find(&self, window: &mut Window, cx: &mut App) {
        let focus = self.find.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    /// Whether the find input has the keyboard.
    pub fn find_focused(&self, window: &Window, cx: &App) -> bool {
        self.find.read(cx).focus_handle(cx).is_focused(window)
    }

    /// The text in the find input.
    pub fn find_text(&self, cx: &App) -> String {
        self.find.read(cx).value().to_string()
    }

    /// Empties the find input (and so the find text of the model).
    pub fn clear_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find
            .update(cx, |find, cx| find.set_value("", window, cx));
        // `set_value` says nothing: tell the model what the box shows.
        self.model.update(cx, |model, cx| model.set_find("", cx));
    }

    /// Scrolls so that `row` is in the middle of the list (or as near as the
    /// ends allow). Takes effect at the next frame.
    pub fn scroll_to_row(&self, row: usize) {
        self.scroll
            .scroll_to_item_strict(row, ScrollStrategy::Center);
    }

    /// The first row at least partly in view, as of the latest frame.
    pub fn top_row(&self) -> usize {
        // From the offset: the rows are all one height.
        let scrolled = -self.scroll.0.borrow().base_handle.offset().y;
        (f32::from(scrolled).max(0.0) / ROW_HEIGHT) as usize
    }

    /// Where `part` is in the window, as of the latest frame; `None` while it
    /// is not on the screen.
    pub fn bounds_of(&self, part: TracePart) -> Option<Bounds<Pixels>> {
        self.frame.borrow().get(&part).copied()
    }

    /// The middle of the row at `row` (a position among the rows shown) as of
    /// the latest frame, taking the scroll into account; `None` when the row is
    /// not in view.
    pub fn row_center(&self, row: usize) -> Option<Point<Pixels>> {
        let list = self.bounds_of(TracePart::List)?;
        let offset = self.scroll.0.borrow().base_handle.offset().y;
        let y = list.origin.y + offset + px(ROW_HEIGHT * row as f32 + ROW_HEIGHT / 2.0);
        let visible = y > list.origin.y && y < list.origin.y + list.size.height;
        visible.then(|| point(list.origin.x + list.size.width / 2.0, y))
    }

    /// The middle of the chevron of the row at `row`, the way
    /// [`row_center`](Self::row_center) finds the row.
    pub fn chevron_center(&self, row: usize, cx: &App) -> Option<Point<Pixels>> {
        let center = self.row_center(row)?;
        let list = self.bounds_of(TracePart::List)?;
        let depth = self
            .model
            .read(cx)
            .waterfall()?
            .rows()
            .get(row)
            .map(|row| row.depth)?;
        let x =
            list.origin.x + px(INDENT_BASE + f32::from(depth) * INDENT_STEP + CHEVRON_BOX / 2.0);
        Some(point(x, center.y))
    }

    /// The rows `range` of the list as elements.
    fn rows_in(
        &mut self,
        range: Range<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = *cx.global::<Theme>();
        let cx = &*cx;
        let model = self.model.read(cx);
        let Some(waterfall) = model.waterfall() else {
            return Vec::new();
        };
        let rows = waterfall.rows();
        let end = range.end.min(rows.len());
        let start = range.start.min(end);
        let selected = waterfall.selected();
        let matches = model.matches();
        (start..end)
            .map(|index| {
                let row = &rows[index];
                let matched = matches.binary_search(&index).is_ok();
                render_row(
                    &theme,
                    row,
                    index,
                    selected == Some(row.span_id),
                    matched,
                    cx,
                )
            })
            .collect()
    }

    /// A click on a row: select the span, or fold the placeholder.
    fn click_row(&mut self, row: &WaterfallRow, cx: &mut Context<Self>) {
        let id = row.span_id;
        self.model.update(cx, |model, cx| {
            if row.is_placeholder() {
                model.toggle(id, cx);
            } else {
                model.select(id, cx);
            }
        });
    }
}

impl Render for WaterfallView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Every part draws itself again below and measures itself: what was
        // there last frame and is gone now must not be found.
        self.frame.borrow_mut().clear();

        let theme = *cx.global::<Theme>();
        let model = self.model.read(cx);
        let count = model
            .waterfall()
            .map_or(0, |waterfall| waterfall.rows().len());
        let ruler = model
            .waterfall()
            .map(|waterfall| waterfall.ruler())
            .unwrap_or_default();
        let errors_only = model.errors_path_only();
        let any_folded = model.any_folded();
        let find_label = model.find_label();
        let frame = &self.frame;

        let find = div()
            .relative()
            .flex_none()
            .w(px(FIND_WIDTH))
            .child(Input::new(&self.find).small().cleanable(true))
            .child(measure(frame, TracePart::Find));

        let count_label = div()
            .flex_none()
            .min_w(px(56.0))
            .text_size(px(12.0))
            .text_color(theme.text_secondary)
            .child(find_label);

        let errors_toggle = div()
            .relative()
            .flex_none()
            .child(
                Toggle::new("errors-path-only")
                    .label("Errors path only")
                    .small()
                    .outline()
                    .checked(errors_only)
                    .when(errors_only, |toggle| {
                        toggle
                            .bg(rgba(theme.accent, 0.16))
                            .text_color(theme.accent)
                            .border_color(theme.accent)
                    })
                    .on_click({
                        let model = self.model.clone();
                        move |on, _window, cx| {
                            model.update(cx, |model, cx| model.set_errors_path_only(*on, cx));
                        }
                    }),
            )
            .child(measure(frame, TracePart::ErrorsPath));

        let fold_all = div()
            .relative()
            .flex_none()
            .child(
                Button::new("fold-all")
                    .label(if any_folded {
                        "Expand all"
                    } else {
                        "Collapse all"
                    })
                    .small()
                    .outline()
                    .on_click({
                        let model = self.model.clone();
                        move |_, _window, cx| {
                            model.update(cx, |model, cx| {
                                if model.any_folded() {
                                    model.expand_all(cx);
                                } else {
                                    model.collapse_all(cx);
                                }
                            });
                        }
                    }),
            )
            .child(measure(frame, TracePart::ToggleAll));

        let toolbar = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(PAD_X))
            .py(px(8.0))
            .child(find)
            .child(count_label)
            .child(errors_toggle)
            .child(div().flex_1())
            .child(fold_all);

        let ruler_row = ruler_row(&theme, &ruler);

        let list = div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                uniform_list("trace-rows", count, cx.processor(Self::rows_in))
                    .track_scroll(&self.scroll)
                    .size_full(),
            )
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::new(&self.scroll)
                        .id("trace-rows-scrollbar")
                        .axis(ScrollbarAxis::Vertical)
                        .viewport_from_layout(),
                ),
            )
            .when(count == 0, |list| {
                list.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(13.0))
                        .text_color(theme.text_secondary)
                        .child(if errors_only {
                            NO_ERRORS
                        } else {
                            "This trace has no spans"
                        }),
                )
            })
            .child(measure(frame, TracePart::List));

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(toolbar)
            .child(ruler_row)
            .child(div().flex_none().h(px(1.0)).w_full().bg(theme.border))
            .child(list)
    }
}

/// The ruler: the five labels over the timeline, in the same columns as the
/// rows (the label column and the duration column are empty).
fn ruler_row(theme: &Theme, labels: &[String; 5]) -> impl IntoElement {
    const LABEL_BOX: f32 = 64.0;
    let at = |index: usize, text: &String| {
        let share = index as f32 / 4.0;
        let label = div()
            .absolute()
            .top_0()
            .h_full()
            .w(px(LABEL_BOX))
            .flex()
            .items_center()
            .text_size(px(11.0))
            .text_color(theme.text_tertiary);
        let label = match index {
            0 => label.left_0().justify_start(),
            4 => label.right_0().justify_end(),
            _ => label
                .left(relative(share))
                .ml(px(-LABEL_BOX / 2.0))
                .justify_center(),
        };
        label.child(text.clone())
    };
    div()
        .flex_none()
        .h(px(RULER_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .child(div().w(relative(LABEL_SHARE)).h_full().flex_none())
        .child(
            div()
                .relative()
                .flex_1()
                .h_full()
                .children(labels.iter().enumerate().map(|(i, text)| at(i, text))),
        )
        .child(div().w(px(DURATION_WIDTH)).h_full().flex_none())
}

/// One row of the waterfall.
fn render_row(
    theme: &Theme,
    row: &WaterfallRow,
    index: usize,
    selected: bool,
    matched: bool,
    cx: &Context<WaterfallView>,
) -> AnyElement {
    let placeholder = row.is_placeholder();
    let palette = theme.service_palette[row.color % theme.service_palette.len()];
    let indent = INDENT_BASE + f32::from(row.depth) * INDENT_STEP;

    let chevron = chevron(theme, row, index, cx);
    let service_bar = div()
        .flex_none()
        .w(px(COLOR_BAR_WIDTH))
        .h(px(COLOR_BAR_HEIGHT))
        .rounded(px(2.0))
        .bg(if placeholder { theme.border } else { palette })
        .mr(px(6.0));
    let error_icon = row.is_error.then(|| {
        div()
            .flex_none()
            .mr(px(6.0))
            .size(px(ERROR_ICON))
            .rounded_full()
            .bg(theme.error)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(10.0))
            .font_weight(FontWeight::BOLD)
            .text_color(theme.bg)
            .child("!")
    });
    let name = div()
        .min_w_0()
        .truncate()
        .text_size(px(12.5))
        .when(placeholder, |name| {
            name.italic().text_color(theme.text_tertiary)
        })
        .when(!placeholder, |name| name.text_color(theme.text))
        .child(row.name.clone());
    // The service after the name, and for a call the other side of it:
    // `payments-api → card-gateway`.
    let note = (!placeholder).then(|| {
        let text = match &row.peer_note {
            Some(peer) => format!("{} \u{2192} {peer}", row.service),
            None => row.service.clone(),
        };
        let mut note = div()
            .min_w_0()
            .max_w(px(NOTE_MAX_WIDTH))
            .truncate()
            .ml(px(8.0))
            .text_size(px(11.0))
            .text_color(theme.text_tertiary)
            .child(text);
        // When the label is short of room the note gives way before the name.
        note.style().flex_shrink = Some(NOTE_SHRINK);
        note
    });

    let label = div()
        .w(relative(LABEL_SHARE))
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .child(div().flex_none().w(px(indent)))
        .child(chevron)
        .child(service_bar)
        .children(error_icon)
        .child(name)
        .children(note);

    let timeline = timeline(theme, row, palette, selected);

    let duration = div()
        .flex_none()
        .w(px(DURATION_WIDTH))
        .pr(px(PAD_X))
        .flex()
        .justify_end()
        .text_size(px(11.5))
        .text_color(theme.text_secondary)
        .child(row.duration.clone());

    let span = row.clone();
    let background = if selected {
        Some(rgba(theme.accent, 0.18))
    } else if matched {
        Some(rgba(theme.warn, 0.12))
    } else {
        None
    };
    div()
        .id(("trace-row", index))
        .w_full()
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .cursor_pointer()
        .when_some(background, |row, color| row.bg(color))
        .when(!selected, |row| {
            row.hover(|style| style.bg(rgba(theme.text, 0.05)))
        })
        .on_click(cx.listener(move |view, _, _, cx| view.click_row(&span, cx)))
        .child(label)
        .child(timeline)
        .child(duration)
        .into_any_element()
}

/// The chevron of a row that has children: down while open, right while folded;
/// a click folds or opens. A leaf has an empty box of the same size, so names
/// line up.
fn chevron(
    theme: &Theme,
    row: &WaterfallRow,
    index: usize,
    cx: &Context<WaterfallView>,
) -> AnyElement {
    let slot = div()
        .flex_none()
        .size(px(CHEVRON_BOX))
        .flex()
        .items_center()
        .justify_center();
    if !row.has_children {
        return slot.into_any_element();
    }
    let icon = if row.collapsed {
        IconName::ChevronRight
    } else {
        IconName::ChevronDown
    };
    let id = row.span_id;
    slot.id(("trace-chevron", index))
        .rounded(px(4.0))
        .hover(|style| style.bg(rgba(theme.text, 0.10)))
        .child(
            Icon::new(icon)
                .size(px(CHEVRON_ICON))
                .text_color(Hsla::from(theme.text_secondary)),
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            // The row's own click selects; this one only folds.
            cx.stop_propagation();
            view.model.update(cx, |model, cx| model.toggle(id, cx));
        }))
        .into_any_element()
}

/// The timeline cell of a row: the faint lines at a quarter, a half and three
/// quarters, the bar and the diamonds of the exception events.
fn timeline(theme: &Theme, row: &WaterfallRow, palette: Rgba, selected: bool) -> Div {
    let mut cell = div().relative().flex_1().h_full().overflow_hidden();
    for quarter in 1..4 {
        cell = cell.child(
            div()
                .absolute()
                .top_0()
                .left(relative(quarter as f32 / 4.0))
                .w(px(1.0))
                .h_full()
                .bg(rgba(theme.border, 0.7)),
        );
    }
    if row.is_placeholder() {
        return cell;
    }
    let opacity = if row.is_error || selected {
        1.0
    } else {
        BAR_OPACITY
    };
    let bar = div()
        .absolute()
        .top(px((ROW_HEIGHT - BAR_HEIGHT) / 2.0))
        .left(relative(row.start_frac))
        .w(relative(row.width_frac))
        .min_w(px(BAR_MIN_WIDTH))
        .h(px(BAR_HEIGHT))
        .rounded(px(3.0))
        .bg(rgba(palette, opacity))
        .when(row.is_error, |bar| bar.border_1().border_color(theme.error));
    let mut cell = cell.child(bar);
    for &at in &row.exception_fracs {
        cell = cell.child(marker(theme, at));
    }
    cell
}

/// A diamond at `at` (a share of the timeline's width), with a ring in the
/// panel's colour so it reads over a bar and over the background.
fn marker(theme: &Theme, at: f32) -> impl IntoElement {
    let ring = Hsla::from(theme.panel);
    let fill = Hsla::from(theme.text);
    let size = MARKER_SIZE + 2.0 * MARKER_RING;
    div()
        .absolute()
        .top(px((ROW_HEIGHT - size) / 2.0))
        .left(relative(at))
        .ml(px(-size / 2.0))
        .size(px(size))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, _| {
                    let center = bounds.center();
                    let diamond = |half: f32| {
                        let mut path = PathBuilder::fill();
                        path.add_polygon(
                            &[
                                point(center.x, center.y - px(half)),
                                point(center.x + px(half), center.y),
                                point(center.x, center.y + px(half)),
                                point(center.x - px(half), center.y),
                            ],
                            true,
                        );
                        path.build().ok()
                    };
                    if let Some(path) = diamond(size / 2.0) {
                        window.paint_path(path, ring);
                    }
                    if let Some(path) = diamond(MARKER_SIZE / 2.0) {
                        window.paint_path(path, fill);
                    }
                },
            )
            .size_full(),
        )
}
