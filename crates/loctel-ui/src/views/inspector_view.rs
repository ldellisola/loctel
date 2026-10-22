//! The inspector of the trace view (design 9.2, M3 plan Task 8): everything
//! about the selected span, in a scroll area on the right of the waterfall.
//!
//! From the top:
//!
//! | Part | What |
//! |---|---|
//! | title | the span's name, wrapped |
//! | summary | the service with its colour dot, then `Server · 1.59 s · starts at +200 ms`, then the status (a red `!` and red text for `Error`) |
//! | exception card | only for a span with an `exception` event: the type (monospace), the message, the stack trace (monospace, a line per line) and a `Copy` button |
//! | `Attributes`, `Resource` | key and value grids, sorted by key; a value of more than 200 characters is cut and `… (show all)` shows the rest, `(show less)` cuts it again |
//! | buttons | `Traces through this span name` and `Copy span as JSON` |
//!
//! [`InspectorView`] holds no data: the text is the [`TraceModel`]'s
//! [`SpanDetails`](crate::model::inspector::SpanDetails). It keeps which long
//! values are shown whole (for the span that is selected; another span starts
//! with all of them cut) and where its parts were laid out, for tests. The one
//! thing it asks of the shell is [`InspectorEvent::TracesThrough`].
//!
//! Nothing can widen the pane: every row is `min_w_0` and wraps or cuts its
//! text, and the 20 KB value of a test is shown 200 characters at a time.

use std::collections::HashSet;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, Entity, EventEmitter, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render, Rgba,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px, relative,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use loctel_model::SpanId;

use super::trace_view::{Frame, TracePart, measure};
use crate::Theme;
use crate::app::TraceModel;
use crate::model::inspector::{SpanDetails, exception_text, span_json, value_preview};

/// What the inspector says with nothing selected.
pub const NOTHING_SELECTED: &str = "Select a span to see its details";

/// The words of the two buttons at the bottom.
pub const TRACES_THROUGH: &str = "Traces through this span name";
pub const COPY_JSON: &str = "Copy span as JSON";

/// The share of a row's width the key takes in the key and value grids.
const KEY_SHARE: f32 = 0.44;
/// The padding of the pane.
const PAD: f32 = 14.0;

fn rgba(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

/// What the inspector asks of the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectorEvent {
    /// The user pressed `Traces through this span name`: search the home
    /// screen for the traces that have a span with this name.
    TracesThrough(String),
}

/// The two grids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Attributes,
    Resource,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Attributes => "Attributes",
            Section::Resource => "Resource",
        }
    }
}

/// The inspector of the trace view. Make it with
/// `cx.new(|cx| InspectorView::new(model, cx))`.
pub struct InspectorView {
    model: Entity<TraceModel>,
    frame: Rc<Frame>,
    /// The values shown whole: `section:position` of each, for the span in
    /// `shown_for`.
    expanded: HashSet<String>,
    shown_for: Option<SpanId>,
    /// Keeps the observation of the model alive.
    _subscription: Subscription,
}

impl EventEmitter<InspectorEvent> for InspectorView {}

impl InspectorView {
    pub fn new(model: Entity<TraceModel>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&model, |_, _, cx| cx.notify());
        InspectorView {
            model,
            frame: Rc::new(Frame::default()),
            expanded: HashSet::new(),
            shown_for: None,
            _subscription: subscription,
        }
    }

    /// Where `part` is in the window, as of the latest frame; `None` while it
    /// is not on the screen.
    pub fn bounds_of(&self, part: TracePart) -> Option<Bounds<Pixels>> {
        self.frame.borrow().get(&part).copied()
    }

    /// The middle of `part`, as of the latest frame.
    pub fn center_of(&self, part: TracePart) -> Option<Point<Pixels>> {
        self.bounds_of(part).map(|bounds| bounds.center())
    }

    /// How many long values are shown whole now.
    pub fn expanded_values(&self) -> usize {
        self.expanded.len()
    }

    /// What `Copy span as JSON` copies; `None` with nothing selected.
    pub fn json(&self, cx: &App) -> Option<String> {
        let model = self.model.read(cx);
        span_json(model.snapshot()?, model.selected()?)
    }

    fn copy(&self, text: String, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn toggle_value(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
        cx.notify();
    }
}

impl Render for InspectorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Every part draws itself again below and measures itself: what was
        // there last frame and is gone now must not be found.
        self.frame.borrow_mut().clear();

        let theme = *cx.global::<Theme>();
        let selected = self.model.read(cx).selected();
        if selected != self.shown_for {
            self.shown_for = selected;
            self.expanded.clear();
        }
        let details = self.model.read(cx).details().cloned();
        let frame = Rc::clone(&self.frame);

        let Some(details) = details else {
            return div()
                .size_full()
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .px(px(PAD))
                .text_size(px(13.0))
                .text_color(theme.text_tertiary)
                .child(NOTHING_SELECTED)
                .child(measure(&frame, TracePart::Inspector))
                .into_any_element();
        };
        let color = self.model.read(cx).service_color(&details.service);

        let content = div()
            .id("inspector-scroll")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .p(px(PAD))
            .child(self.title(&theme, &details))
            .child(self.summary(&theme, &details, color))
            .children(self.exception_card(&theme, &details, cx))
            .child(self.grid(&theme, Section::Attributes, &details.attributes, cx))
            .child(self.grid(&theme, Section::Resource, &details.resource, cx))
            .child(self.buttons(&details, cx))
            .overflow_y_scrollbar();

        div()
            .size_full()
            .relative()
            .child(content)
            .child(measure(&frame, TracePart::Inspector))
            .into_any_element()
    }
}

impl InspectorView {
    fn title(&self, theme: &Theme, details: &SpanDetails) -> impl IntoElement {
        div()
            .min_w_0()
            .text_size(px(15.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.text)
            .child(details.name.clone())
    }

    fn summary(&self, theme: &Theme, details: &SpanDetails, color: usize) -> impl IntoElement {
        let dot = theme.service_palette[color % theme.service_palette.len()];
        let erroring = details.status.starts_with("Error");
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .min_w_0()
                    .child(div().flex_none().size(px(9.0)).rounded_full().bg(dot))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.0))
                            .text_color(theme.text)
                            .child(details.service.clone()),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.text_secondary)
                    .child(format!(
                        "{} \u{b7} {} \u{b7} {}",
                        details.kind, details.duration, details.offset
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .min_w_0()
                    .text_size(px(12.0))
                    .when(erroring, |status| {
                        status.text_color(theme.error_text).child(
                            div()
                                .flex_none()
                                .size(px(13.0))
                                .rounded_full()
                                .bg(theme.error)
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(9.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.bg)
                                .child("!"),
                        )
                    })
                    .when(!erroring, |status| status.text_color(theme.text_secondary))
                    .child(div().min_w_0().child(details.status.clone())),
            )
    }

    /// The card of the span's exception, if it has one.
    fn exception_card(
        &self,
        theme: &Theme,
        details: &SpanDetails,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let exception = details.exception.clone()?;
        let mono = cx.theme().mono_font_family.clone();
        let text = exception_text(&exception);
        let frame = Rc::clone(&self.frame);
        let copy = div()
            .relative()
            .flex_none()
            .child(
                Button::new("copy-exception")
                    .label("Copy")
                    .small()
                    .ghost()
                    .on_click(cx.listener(move |view, _, _, cx| view.copy(text.clone(), cx))),
            )
            .child(measure(&frame, TracePart::CopyException));

        let stack = exception.stacktrace.as_deref().unwrap_or("");
        Some(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .p(px(12.0))
                .rounded(px(10.0))
                .border_1()
                .border_color(rgba(theme.error, 0.40))
                .bg(rgba(theme.error, 0.08))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.error_text)
                                .child("EXCEPTION"),
                        )
                        .child(copy),
                )
                .when(!exception.type_name.is_empty(), |card| {
                    card.child(
                        div()
                            .min_w_0()
                            .font_family(mono.clone())
                            .text_size(px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.error_text)
                            .child(exception.type_name.clone()),
                    )
                })
                .when(!exception.message.is_empty(), |card| {
                    card.child(
                        div()
                            .min_w_0()
                            .text_size(px(12.5))
                            .text_color(theme.text)
                            .child(exception.message.clone()),
                    )
                })
                .when(!stack.is_empty(), |card| {
                    // A line per line, wrapped where it is too long: what
                    // `pre-wrap` does.
                    card.child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .font_family(mono.clone())
                            .text_size(px(11.5))
                            .text_color(theme.text_secondary)
                            .children(stack.lines().map(|line| {
                                div().min_h(px(15.0)).min_w_0().child(line.to_owned())
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    /// `Attributes` or `Resource`: the heading with its count and the grid.
    fn grid(
        &self,
        theme: &Theme,
        section: Section,
        pairs: &[(String, String)],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let heading = div()
            .flex()
            .items_baseline()
            .gap(px(6.0))
            .pb(px(4.0))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(section.title()),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.text_tertiary)
                    .child(pairs.len().to_string()),
            );
        let rows = pairs
            .iter()
            .enumerate()
            .map(|(position, (key, value))| {
                let id = format!("{}:{position}", section.title());
                let whole = self.expanded.contains(&id);
                self.pair(theme, key, value, id, whole, cx)
            })
            .collect::<Vec<_>>();
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(heading)
            .when(pairs.is_empty(), |grid| {
                grid.child(
                    div()
                        .pt(px(4.0))
                        .text_size(px(12.0))
                        .text_color(theme.text_tertiary)
                        .child("None"),
                )
            })
            .children(rows)
    }

    /// One key and its value. A value of more than 200 characters shows its
    /// first 200 and `… (show all)`, until it is clicked.
    fn pair(
        &self,
        theme: &Theme,
        key: &str,
        value: &str,
        id: String,
        whole: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let preview = value_preview(value);
        let shown = match (preview, whole) {
            (Some(cut), false) => cut,
            _ => value,
        };
        let link = |label: &'static str, id: String, cx: &mut Context<Self>| {
            let frame = Rc::clone(&self.frame);
            div()
                .relative()
                .child(
                    div()
                        .id(SharedString::from(format!("toggle-{id}")))
                        .cursor_pointer()
                        .text_size(px(12.0))
                        .text_color(theme.accent)
                        .child(label)
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.toggle_value(id.clone(), cx);
                        })),
                )
                .child(measure(&frame, TracePart::ShowAll))
        };
        div()
            .flex()
            .gap(px(10.0))
            .py(px(3.0))
            .min_w_0()
            .child(
                div()
                    .flex_none()
                    .w(relative(KEY_SHARE))
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(theme.text_tertiary)
                    .child(key.to_owned()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .text_size(px(12.0))
                    .text_color(theme.text)
                    .child(div().min_w_0().child(shown.to_owned()))
                    .when(preview.is_some() && !whole, |value| {
                        value.child(link("\u{2026} (show all)", id.clone(), cx))
                    })
                    .when(preview.is_some() && whole, |value| {
                        value.child(link("(show less)", id.clone(), cx))
                    }),
            )
            .into_any_element()
    }

    fn buttons(&self, details: &SpanDetails, cx: &mut Context<Self>) -> impl IntoElement {
        let name = details.name.clone();
        let frame = Rc::clone(&self.frame);
        let through = div()
            .relative()
            .w_full()
            .child(
                Button::new("traces-through")
                    .label(TRACES_THROUGH)
                    .small()
                    .outline()
                    .w_full()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(InspectorEvent::TracesThrough(name.clone()));
                    })),
            )
            .child(measure(&frame, TracePart::TracesThrough));
        let json = div()
            .relative()
            .w_full()
            .child(
                Button::new("copy-json")
                    .label(COPY_JSON)
                    .small()
                    .outline()
                    .w_full()
                    .on_click(cx.listener(|view, _, _, cx| {
                        if let Some(json) = view.json(cx) {
                            view.copy(json, cx);
                        }
                    })),
            )
            .child(measure(&frame, TracePart::CopyJson));
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .pt(px(4.0))
            .child(through)
            .child(json)
    }
}
