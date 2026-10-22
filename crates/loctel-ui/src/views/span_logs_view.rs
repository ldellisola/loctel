//! The logs of the selected span (design 9.2, M3 plan Task 8): the panel under
//! the waterfall. A title (`Logs from POST /charges`) with a count, and under it
//! a row per log, oldest first: the time (monospace), the level in a badge and
//! the message on one line, cut with an ellipsis (the whole message is in a
//! tooltip).
//!
//! [`SpanLogsView`] holds no data: the lines are the [`TraceModel`]'s, read from
//! the store for whichever span is selected. With no span selected it says so,
//! and while the read of a span's logs is out it says nothing rather than `No
//! logs`. The list is a `uniform_list`, so a span with a hundred thousand logs
//! costs a screenful.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, Bounds, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, Rgba, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, UniformListScrollHandle, Window,
    div, prelude::FluentBuilder as _, px, uniform_list,
};
use gpui_kit::base::{Scrollbar, ScrollbarAxis};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use loctel_model::Severity;

use super::trace_view::{Frame, TracePart, measure};
use crate::Theme;
use crate::app::TraceModel;
use crate::model::logs::span_logs_query;
use crate::model::trace_view::{logs_count, logs_title};

/// The height of the panel, in pixels (the plan says about 200).
pub const PANEL_HEIGHT: f32 = 200.0;
/// The height of the title row and of a log row, in pixels.
const HEADER_HEIGHT: f32 = 36.0;
const ROW_HEIGHT: f32 = 24.0;
/// The widths of the time and of the level badge.
const TIME_WIDTH: f32 = 92.0;
const BADGE_WIDTH: f32 = 36.0;
/// The padding of the panel's sides.
const PAD_X: f32 = 14.0;

/// The header's button: the logs of the selected span, in the Logs screen.
pub const OPEN_IN_LOGS: &str = "Open in Logs";

/// What the panel says with no span selected.
pub const NOTHING_SELECTED: &str = "Select a span to see its logs";
/// And when the selected span has no logs.
pub const NO_LOGS: &str = "No logs from this span";

fn rgba(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

/// What the panel asks of the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpanLogsEvent {
    /// The user pressed `Open in Logs`: show the Logs screen with this LogQL
    /// query in its bar (the selected span's logs).
    OpenInLogs(String),
}

/// The logs of the selected span. Make it with
/// `cx.new(|cx| SpanLogsView::new(model, cx))`.
pub struct SpanLogsView {
    model: Entity<TraceModel>,
    scroll: UniformListScrollHandle,
    frame: Rc<Frame>,
    /// Keeps the observation of the model alive.
    _subscription: Subscription,
}

impl EventEmitter<SpanLogsEvent> for SpanLogsView {}

impl SpanLogsView {
    pub fn new(model: Entity<TraceModel>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&model, |_, _, cx| cx.notify());
        SpanLogsView {
            model,
            scroll: UniformListScrollHandle::new(),
            frame: Rc::new(Frame::default()),
            _subscription: subscription,
        }
    }

    /// Where `part` (`OpenInLogs`) is in the window, as of the latest frame;
    /// `None` while it is not on the screen (no span is selected).
    pub fn bounds_of(&self, part: TracePart) -> Option<Bounds<Pixels>> {
        self.frame.borrow().get(&part).copied()
    }

    /// The query `Open in Logs` writes into the Logs screen's bar: the selected
    /// span's logs (`{service_name="payments-api"} | span_id = "..."`), `None`
    /// with no span selected.
    pub fn open_in_logs_query(&self, cx: &gpui::App) -> Option<String> {
        let model = self.model.read(cx);
        let service = &model.details()?.service;
        Some(span_logs_query(service, model.selected()?))
    }

    fn open_in_logs(&mut self, cx: &mut Context<Self>) {
        if let Some(query) = self.open_in_logs_query(cx) {
            cx.emit(SpanLogsEvent::OpenInLogs(query));
        }
    }

    /// The title row's text: `Logs from POST /charges` and the count, or
    /// `None` with no span selected.
    pub fn title(&self, cx: &gpui::App) -> Option<(String, String)> {
        let model = self.model.read(cx);
        let name = &model.details()?.name;
        Some((logs_title(name), logs_count(model.logs().len())))
    }

    /// The rows `range` of the list.
    fn rows_in(
        &mut self,
        range: Range<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = *cx.global::<Theme>();
        let mono = cx.theme().mono_font_family.clone();
        let logs = self.model.read(cx).logs();
        let end = range.end.min(logs.len());
        let start = range.start.min(end);
        logs[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let message = SharedString::from(line.message.clone());
                div()
                    .id(("span-log", start + i))
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .px(px(PAD_X))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex_none()
                            .w(px(TIME_WIDTH))
                            .font_family(mono.clone())
                            .text_size(px(11.5))
                            .text_color(theme.text_tertiary)
                            .child(line.time.clone()),
                    )
                    .child(badge(&theme, line.severity, line.level))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(theme.text)
                            .child(message.clone()),
                    )
                    .tooltip(move |window, cx| Tooltip::new(message.clone()).build(window, cx))
                    .into_any_element()
            })
            .collect()
    }
}

/// The level's badge: three letters on a tint of the level's colour.
fn badge(theme: &Theme, severity: Severity, text: &'static str) -> impl IntoElement {
    let (fill, ink) = match severity {
        Severity::Error | Severity::Fatal => (rgba(theme.error, 0.16), theme.error_text),
        Severity::Warn => (rgba(theme.warn, 0.16), theme.warn),
        Severity::Info => (rgba(theme.accent, 0.16), theme.accent),
        Severity::Debug | Severity::Trace | Severity::Unspecified => {
            (rgba(theme.text_tertiary, 0.16), theme.text_secondary)
        }
    };
    div()
        .flex_none()
        .w(px(BADGE_WIDTH))
        .h(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(fill)
        .text_color(ink)
        .text_size(px(10.0))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

impl Render for SpanLogsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame.borrow_mut().clear();
        let theme = *cx.global::<Theme>();
        let model = self.model.read(cx);
        let title = model.details().map(|details| logs_title(&details.name));
        let count = model.logs().len();
        let loaded = model.logs_loaded();
        let frame = Rc::clone(&self.frame);

        let header = div()
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .px(px(PAD_X))
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(title.clone().unwrap_or_else(|| "Logs".to_owned())),
            )
            .when(title.is_some() && loaded, |header| {
                header.child(
                    div()
                        .flex_none()
                        .text_size(px(11.5))
                        .text_color(theme.text_tertiary)
                        .child(logs_count(count)),
                )
            })
            .when(title.is_some(), |header| {
                header.child(div().flex_1()).child(
                    div()
                        .relative()
                        .flex_none()
                        .child(
                            Button::new("open-in-logs")
                                .label(OPEN_IN_LOGS)
                                .small()
                                .ghost()
                                .on_click(cx.listener(|view, _, _, cx| view.open_in_logs(cx))),
                        )
                        .child(measure(&frame, TracePart::OpenInLogs)),
                )
            });

        let message = if title.is_none() {
            Some(NOTHING_SELECTED)
        } else if loaded && count == 0 {
            Some(NO_LOGS)
        } else {
            None
        };

        let body = div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                uniform_list("span-logs", count, cx.processor(Self::rows_in))
                    .track_scroll(&self.scroll)
                    .size_full(),
            )
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::new(&self.scroll)
                        .id("span-logs-scrollbar")
                        .axis(ScrollbarAxis::Vertical)
                        .viewport_from_layout(),
                ),
            )
            .when_some(message, |body, message| {
                body.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.5))
                        .text_color(theme.text_tertiary)
                        .child(message),
                )
            });

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
    }
}
