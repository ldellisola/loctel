//! The trace view (design 9.2, M3 plan Task 8): one trace as a waterfall with
//! an inspector for the selected span and that span's logs. It is the content
//! of [`Screen::Trace`](crate::app::Screen::Trace).
//!
//! [`TraceView`] puts the screen together and holds none of its data:
//!
//! - a **header** in two lines. First the `Traces` back button (Esc does the
//!   same), the root span's name, the red `Failed in …` badge (or the amber
//!   `Errors inside`) and, on the right, the completeness indicator
//!   (`Receiving spans…` in the accent colour, `Complete` in green). Then the
//!   line `1.84 s · 15 spans · 5 services · 14:32:07.412`, the trace ID in the
//!   monospace face cut in the middle, and `Copy ID`.
//! - a **body**: a horizontal GPUI Kit split. On the left the
//!   [`WaterfallView`] over the [`SpanLogsView`] (about 200 px, under a
//!   hairline); on the right the [`InspectorView`], 340 px, which can be
//!   dragged from 280 to 520.
//!
//! The data is the [`TraceModel`]'s (read from the store while the screen is
//! open). The three parts observe it, and what they do goes back into it: a
//! click on a row selects the span, and everything follows from the selection.
//! What the screen asks of the window it emits as [`TraceViewEvent`]: `Back`
//! (the button, or Esc) and `Query` (`Traces through this span name`).
//!
//! Keys: **Esc** goes back, except that while the find box has the keyboard and
//! text in it, it empties the box first; **⌘F** (or Ctrl-F) gives the find box
//! the keyboard. The view takes the keyboard when it opens, so they
//! work without a click.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AppContext as _, Bounds, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Pixels, Point, Render,
    Rgba, Styled as _, Subscription, Window, canvas, div, prelude::FluentBuilder as _, px,
};
use gpui_kit::base::{ResizableState, resizable_panel};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::resizable::h_resizable;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _};
use loctel_model::TraceId;

use super::inspector_view::{InspectorEvent, InspectorView};
use super::span_logs_view::{PANEL_HEIGHT as LOGS_HEIGHT, SpanLogsView};
use super::waterfall_view::WaterfallView;
use crate::Theme;
use crate::app::{LoadState, TraceModel, UiBackend};
use crate::model::colors::ServiceColors;
use crate::model::format;
use crate::model::query_bar::span_name_filter;
use crate::model::trace_view::{Failure, TraceHeader};

/// The inspector's width when the screen opens, and how far it can be dragged,
/// in pixels.
pub const INSPECTOR_WIDTH: f32 = 340.0;
pub const INSPECTOR_MIN_WIDTH: f32 = 280.0;
pub const INSPECTOR_MAX_WIDTH: f32 = 520.0;
/// The least the waterfall's side can be, in pixels.
const WATERFALL_MIN_WIDTH: f32 = 380.0;
/// How many characters of the trace ID the header shows (the middle is cut).
const TRACE_ID_CHARS: usize = 17;
/// The padding of the header's sides.
const PAD_X: f32 = 16.0;

/// What the screen says while the first read is out.
pub const LOADING: &str = "Loading trace\u{2026}";
/// What it says when the store has no such trace.
pub const MISSING: &str = "This trace is not in the store";
/// And why that can be.
pub const MISSING_HINT: &str = "It may have been evicted or cleared.";

fn rgba(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

/// The parts of the trace screen a test can ask the place of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TracePart {
    /// The `Traces` back button.
    Back,
    /// The `Copy ID` button.
    CopyId,
    /// The waterfall's find input.
    Find,
    /// The `Errors path only` toggle.
    ErrorsPath,
    /// The `Collapse all` / `Expand all` button.
    ToggleAll,
    /// The waterfall's list of rows (its viewport).
    List,
    /// The whole inspector pane.
    Inspector,
    /// The inspector's `Traces through this span name` button.
    TracesThrough,
    /// The inspector's `Copy span as JSON` button.
    CopyJson,
    /// The exception card's `Copy` button; present only for a span with an
    /// exception.
    CopyException,
    /// The `… (show all)` (or `(show less)`) link of the last long value of the
    /// inspector; present only when there is one.
    ShowAll,
}

/// Where the parts of the latest frame are, in window coordinates.
pub(super) type Frame = RefCell<HashMap<TracePart, Bounds<Pixels>>>;

/// An invisible canvas over its parent that writes the parent's bounds into
/// `frame` while the frame is prepainted. Add it as a child of a `relative` box
/// that is the size of the part.
pub(super) fn measure(frame: &Rc<Frame>, part: TracePart) -> impl IntoElement {
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

/// What the trace screen asks of the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceViewEvent {
    /// Leave the trace: show the home screen as it was.
    Back,
    /// Show the home screen with this TraceQL query in the bar (the query of
    /// `Traces through this span name`).
    Query(String),
}

/// The trace screen. Make it with
/// `cx.new(|cx| TraceView::new(backend, trace_id, window, cx))`.
pub struct TraceView {
    model: Entity<TraceModel>,
    waterfall: Entity<WaterfallView>,
    inspector: Entity<InspectorView>,
    logs: Entity<SpanLogsView>,
    split: Entity<ResizableState>,
    focus: FocusHandle,
    frame: Rc<Frame>,
    /// Keeps the observation of the model and of the inspector alive.
    _subscriptions: [Subscription; 2],
}

impl EventEmitter<TraceViewEvent> for TraceView {}

impl TraceView {
    /// The screen of trace `trace_id`, read from `backend`, with colours of its
    /// own (a service gets the colour of its first appearance in this trace).
    /// The window's own screen uses [`with_colors`](Self::with_colors) so a
    /// service keeps its colour from the map and the table.
    pub fn new(
        backend: Rc<dyn UiBackend>,
        trace_id: TraceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> TraceView {
        Self::with_colors(
            backend,
            trace_id,
            Rc::new(RefCell::new(ServiceColors::new())),
            window,
            cx,
        )
    }

    /// Like [`new`](Self::new), with the session's service colours (the home
    /// screen's [`HomeModel::colors`](crate::app::HomeModel::colors)).
    pub fn with_colors(
        backend: Rc<dyn UiBackend>,
        trace_id: TraceId,
        colors: Rc<RefCell<ServiceColors>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> TraceView {
        let model = cx.new(|cx| TraceModel::new(backend, trace_id, colors, cx));
        let waterfall = cx.new(|cx| WaterfallView::new(model.clone(), window, cx));
        let inspector = cx.new(|cx| InspectorView::new(model.clone(), cx));
        let logs = cx.new(|cx| SpanLogsView::new(model.clone(), cx));
        let split = cx.new(|_| ResizableState::default());
        let focus = cx.focus_handle();
        // So that Esc and the find shortcut work before any click.
        window.focus(&focus, cx);
        let subscriptions = [
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.subscribe(&inspector, |_, _, event: &InspectorEvent, cx| {
                let InspectorEvent::TracesThrough(name) = event;
                cx.emit(TraceViewEvent::Query(span_name_filter(name)));
            }),
        ];
        TraceView {
            model,
            waterfall,
            inspector,
            logs,
            split,
            focus,
            frame: Rc::new(Frame::default()),
            _subscriptions: subscriptions,
        }
    }

    /// The trace's live data.
    pub fn model(&self) -> &Entity<TraceModel> {
        &self.model
    }

    /// The waterfall (its rows, its find box, its scroll).
    pub fn waterfall_view(&self) -> &Entity<WaterfallView> {
        &self.waterfall
    }

    /// The inspector.
    pub fn inspector_view(&self) -> &Entity<InspectorView> {
        &self.inspector
    }

    /// The logs of the selected span.
    pub fn logs_view(&self) -> &Entity<SpanLogsView> {
        &self.logs
    }

    /// The split between the waterfall's side and the inspector.
    pub fn split(&self) -> &Entity<ResizableState> {
        &self.split
    }

    /// Where `part` is in the window, as of the latest frame; `None` while it is
    /// not on the screen.
    pub fn bounds_of(&self, part: TracePart, cx: &gpui::App) -> Option<Bounds<Pixels>> {
        match part {
            TracePart::Back | TracePart::CopyId => self.frame.borrow().get(&part).copied(),
            TracePart::Find | TracePart::ErrorsPath | TracePart::ToggleAll | TracePart::List => {
                self.waterfall.read(cx).bounds_of(part)
            }
            TracePart::Inspector
            | TracePart::TracesThrough
            | TracePart::CopyJson
            | TracePart::CopyException
            | TracePart::ShowAll => self.inspector.read(cx).bounds_of(part),
        }
    }

    /// The middle of `part`, as of the latest frame.
    pub fn center_of(&self, part: TracePart, cx: &gpui::App) -> Option<Point<Pixels>> {
        self.bounds_of(part, cx).map(|bounds| bounds.center())
    }

    /// Whether the `Esc` key (with the find box in the state it is in) goes
    /// back; `false` when it first empties the find box.
    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let find_has_text = {
            let waterfall = self.waterfall.read(cx);
            waterfall.find_focused(window, cx) && !waterfall.find_text(cx).is_empty()
        };
        if find_has_text {
            self.waterfall
                .update(cx, |waterfall, cx| waterfall.clear_find(window, cx));
        } else {
            cx.emit(TraceViewEvent::Back);
        }
    }

    fn copy_id(&self, cx: &mut Context<Self>) {
        if let Some(header) = self.model.read(cx).header() {
            cx.write_to_clipboard(ClipboardItem::new_string(header.trace_id.clone()));
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.key == "escape" {
            self.escape(window, cx);
            cx.stop_propagation();
        } else if keystroke.key == "f"
            && (keystroke.modifiers.platform || keystroke.modifiers.control)
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.shift
        {
            self.waterfall
                .update(cx, |waterfall, cx| waterfall.focus_find(window, cx));
            cx.stop_propagation();
        }
    }
}

impl Render for TraceView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame.borrow_mut().clear();
        let theme = *cx.global::<Theme>();
        let (state, header_data) = {
            let model = self.model.read(cx);
            (model.state(), model.header().cloned())
        };
        let header = self.header(&theme, header_data.as_ref(), state, cx);

        let body = match state {
            LoadState::Loaded => self.body(&theme).into_any_element(),
            LoadState::Loading => centered(&theme, LOADING, None).into_any_element(),
            LoadState::Missing => centered(&theme, MISSING, Some(MISSING_HINT)).into_any_element(),
        };

        div()
            .id("trace-view")
            .key_context("TraceView")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view, event, window, cx| view.on_key_down(event, window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_none().h(px(1.0)).w_full().bg(theme.border))
            .child(div().flex_1().min_h_0().child(body))
    }
}

impl TraceView {
    /// The header: the back button, the name, the badge and the indicator, and
    /// under them the numbers, the ID and `Copy ID`.
    fn header(
        &self,
        theme: &Theme,
        header: Option<&TraceHeader>,
        state: LoadState,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let frame = &self.frame;
        let mono = cx.theme().mono_font_family.clone();

        let back = div()
            .relative()
            .flex_none()
            .child(
                Button::new("trace-back")
                    .label("Traces")
                    .icon(IconName::ArrowLeft)
                    .small()
                    .ghost()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(TraceViewEvent::Back))),
            )
            .child(measure(frame, TracePart::Back));

        let name = match (header, state) {
            (Some(header), _) => header.root.clone(),
            (None, LoadState::Missing) => "Trace not found".to_owned(),
            (None, _) => "Loading\u{2026}".to_owned(),
        };
        let arrived = header.is_none_or(|header| header.root_arrived);
        let name = div()
            .min_w_0()
            .max_w(gpui::relative(0.4))
            .truncate()
            .text_size(px(15.0))
            .font_weight(FontWeight::SEMIBOLD)
            .when(arrived, |name| name.text_color(theme.text))
            .when(!arrived, |name| name.text_color(theme.text_tertiary))
            .child(name);

        let badge = header.and_then(|header| badge(theme, &header.failure));

        let indicator = header.map(|header| {
            let (color, text) = if header.complete {
                (theme.ok, header.completeness())
            } else {
                (theme.accent, header.completeness())
            };
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(px(12.0))
                .text_color(color)
                .child(div().size(px(7.0)).rounded_full().bg(color))
                .child(text)
        });

        let first = div()
            .flex_none()
            .h(px(44.0))
            .px(px(PAD_X - 6.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(back)
            .child(name)
            .children(badge)
            .child(div().flex_1())
            .children(indicator);

        let second = header.map(|header| {
            let id = format::truncate_middle(&header.trace_id, TRACE_ID_CHARS);
            div()
                .flex_none()
                .px(px(PAD_X))
                .pb(px(10.0))
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.5))
                        .text_color(theme.text_secondary)
                        .child(header.summary.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .font_family(mono)
                        .text_size(px(12.0))
                        .text_color(theme.text_tertiary)
                        .child(id),
                )
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .child(
                            Button::new("copy-id")
                                .label("Copy ID")
                                .icon(IconName::Copy)
                                .small()
                                .ghost()
                                .on_click(cx.listener(|view, _, _, cx| view.copy_id(cx))),
                        )
                        .child(measure(frame, TracePart::CopyId)),
                )
        });

        div()
            .flex_none()
            .flex()
            .flex_col()
            .child(first)
            .children(second)
    }

    /// The body: the waterfall over the logs, a split, the inspector.
    fn body(&self, theme: &Theme) -> impl IntoElement {
        let left = div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex_1().min_h_0().child(self.waterfall.clone()))
            .child(div().flex_none().h(px(1.0)).w_full().bg(theme.border))
            .child(
                div()
                    .flex_none()
                    .h(px(LOGS_HEIGHT))
                    .child(self.logs.clone()),
            );

        h_resizable("trace-split")
            .with_state(&self.split)
            .child(
                resizable_panel()
                    .size_range(px(WATERFALL_MIN_WIDTH)..Pixels::MAX)
                    .child(left),
            )
            .child(
                resizable_panel()
                    .size(px(INSPECTOR_WIDTH))
                    .size_range(px(INSPECTOR_MIN_WIDTH)..px(INSPECTOR_MAX_WIDTH))
                    .fixed()
                    .child(self.inspector.clone()),
            )
    }
}

/// A message in the middle of the body.
fn centered(theme: &Theme, title: &'static str, hint: Option<&'static str>) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .child(
            div()
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(title),
        )
        .children(hint.map(|hint| {
            div()
                .text_size(px(12.5))
                .text_color(theme.text_secondary)
                .child(hint)
        }))
}

/// The failure badge: red `Failed in …` or amber `Errors inside`, with a
/// symbol so the colour is not alone (design 9.3).
fn badge(theme: &Theme, failure: &Failure) -> Option<gpui::AnyElement> {
    let text = failure.badge()?;
    let (ink, fill) = match failure {
        Failure::FailedIn(_) => (theme.error_text, rgba(theme.error, 0.16)),
        Failure::ErrorsInside | Failure::None => (theme.warn, rgba(theme.warn, 0.16)),
    };
    let symbol = div()
        .flex_none()
        .size(px(13.0))
        .rounded_full()
        .bg(ink)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(9.0))
        .font_weight(FontWeight::BOLD)
        .text_color(theme.bg)
        .child("!");
    Some(
        div()
            .min_w_0()
            .h(px(24.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(px(7.0))
            .bg(fill)
            .text_color(ink)
            .text_size(px(12.0))
            .font_weight(FontWeight::SEMIBOLD)
            .child(symbol)
            .child(div().min_w_0().truncate().child(text))
            .into_any_element(),
    )
}
