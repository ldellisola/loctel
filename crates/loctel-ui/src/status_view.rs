//! The status view: what the engine is doing, in one window.

use std::time::Duration;

use gpui::{
    App, AppContext as _, Bounds, Context, FontWeight, IntoElement, ParentElement, Render, Styled,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div, px, size,
};
use loctel_engine::{EngineHandle, EngineStatus, ListenerState};

use crate::menu_text::{format_rate, rules_line, status_line};
use crate::theme::{FONT_FAMILY, Theme, register_fonts};

/// How often the window asks the engine for a fresh status.
const REFRESH_EVERY: Duration = Duration::from_secs(1);

/// The window's size, in logical pixels. The screenshot tests render at the
/// same size.
const WINDOW_SIZE: (f32, f32) = (900.0, 600.0);

/// One endpoint as the view shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ListenerLine {
    /// `otlp-grpc`, `otlp-http` or `mcp`.
    pub name: String,
    /// Where it listens, or where it was asked to and could not.
    pub address: String,
    /// Whether it is listening.
    pub ok: bool,
    /// Why it is not, when `ok` is false.
    pub error: Option<String>,
}

/// Everything the status view and the menu need from an [`EngineStatus`],
/// as plain values: comparable (so a refresh that changes nothing does not
/// redraw) and easy to build in a test.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusModel {
    pub paused: bool,
    pub listeners: Vec<ListenerLine>,
    pub spans_per_sec: f64,
    pub logs_per_sec: f64,
    pub traces_held: usize,
    pub logs_held: usize,
    /// How many discard rules are switched on, whether or not their queries
    /// parse.
    pub rules_on: usize,
    /// How many of those do not parse, so they drop nothing. A rule that is
    /// switched off is not counted, even with a draft query that has an error.
    pub rules_with_errors: usize,
    /// Whether `rules.toml` could not be used the last time it was read (the
    /// rules in force are then the last good ones). The error's text stays in
    /// the engine's status: the menu line is fixed words.
    pub rules_file_error: bool,
    /// The share of what arrived that the rules dropped, since launch: 0 to 1.
    pub dropped_fraction: f64,
}

impl StatusModel {
    /// Reads the engine's snapshot. A failed listener keeps the address it
    /// was asked to bind, so the view can say which port is the problem.
    pub fn from_status(status: &EngineStatus) -> StatusModel {
        let listeners = status
            .listeners
            .iter()
            .map(|listener| match &listener.state {
                ListenerState::Listening(address) => ListenerLine {
                    name: listener.name.to_owned(),
                    address: address.to_string(),
                    ok: true,
                    error: None,
                },
                ListenerState::Failed(error) => ListenerLine {
                    name: listener.name.to_owned(),
                    address: listener.requested.to_string(),
                    ok: false,
                    error: Some(error.clone()),
                },
            })
            .collect();
        StatusModel {
            paused: status.paused,
            listeners,
            spans_per_sec: status.spans_per_sec,
            logs_per_sec: status.logs_per_sec,
            traces_held: status.stats.traces,
            logs_held: status.stats.logs,
            rules_on: status.rules.iter().filter(|rule| rule.spec.enabled).count(),
            rules_with_errors: status
                .rules
                .iter()
                .filter(|rule| rule.spec.enabled && rule.error.is_some())
                .count(),
            rules_file_error: status.rules_file_error.is_some(),
            dropped_fraction: status.dropped_fraction,
        }
    }
}

/// The view: a headline, four numbers, the discard rules line and the
/// endpoint list. It draws its
/// [`StatusModel`] and nothing else, which is what makes it testable by
/// screenshot.
pub struct StatusView {
    model: StatusModel,
    theme: Theme,
}

impl StatusView {
    pub fn new(model: StatusModel, theme: Theme) -> StatusView {
        StatusView { model, theme }
    }

    /// Shows a new model. Redraws only when it differs from the current one.
    pub fn set_model(&mut self, model: StatusModel, cx: &mut Context<Self>) {
        if self.model != model {
            self.model = model;
            cx.notify();
        }
    }

    /// Starts refreshing from `engine` once a second, for as long as the
    /// view lives.
    pub fn refresh_from(&self, engine: EngineHandle, cx: &mut Context<Self>) {
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(REFRESH_EVERY).await;
                let model = StatusModel::from_status(&engine.status());
                // The view is gone once the window closes; that ends the loop.
                if view
                    .update(cx, |view, cx| view.set_model(model, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }
}

/// Opens the status window on `cx`: the dark theme, refreshed every second
/// from `engine`. The app no longer opens it (it opens the main window, see
/// [`crate::app::open_main_window`]); it is a standalone view. Fails when the
/// platform has no display to open a window on.
pub fn open_status_window(
    engine: EngineHandle,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<StatusView>> {
    register_fonts(cx);
    let bounds = Bounds::centered(None, size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("loctel".into()),
            ..TitlebarOptions::default()
        }),
        ..WindowOptions::default()
    };
    cx.open_window(options, move |_window, cx| {
        cx.new(|cx| {
            let model = StatusModel::from_status(&engine.status());
            let view = StatusView::new(model, Theme::dark());
            view.refresh_from(engine, cx);
            view
        })
    })
}

impl Render for StatusView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        let model = &self.model;

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_6()
            .p_8()
            .bg(theme.bg)
            .text_color(theme.text)
            .font_family(FONT_FAMILY)
            .text_size(px(14.0))
            .child(header(theme, model))
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(stat(theme, "SPANS / S", format_rate(model.spans_per_sec)))
                    .child(stat(theme, "LOGS / S", format_rate(model.logs_per_sec)))
                    .child(stat(
                        theme,
                        "TRACES HELD",
                        format_rate(model.traces_held as f64),
                    ))
                    .child(stat(
                        theme,
                        "LOGS HELD",
                        format_rate(model.logs_held as f64),
                    )),
            )
            .child(rules_row(theme, model))
            .child(endpoints(theme, model))
    }
}

/// The title and the status pill. The pill carries the same line as the
/// menu bar header, in words, so it never relies on its colour alone.
fn header(theme: &Theme, model: &StatusModel) -> impl IntoElement {
    let dot = if model.listeners.iter().any(|listener| !listener.ok) {
        theme.error
    } else if model.paused {
        theme.warn
    } else {
        theme.ok
    };
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(22.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child("loctel"),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_4()
                .py_2()
                .rounded_full()
                .bg(theme.panel)
                .border_1()
                .border_color(theme.border)
                .text_color(theme.text_secondary)
                .child(div().size(px(8.0)).rounded_full().bg(dot))
                .child(status_line(model)),
        )
}

/// A labelled number on a card.
fn stat(theme: &Theme, label: &'static str, value: String) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap_1()
        .p_4()
        .rounded_lg()
        .bg(theme.panel)
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .text_size(px(11.0))
                .text_color(theme.text_tertiary)
                .child(label),
        )
        .child(
            div()
                .text_size(px(28.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child(value),
        )
}

/// The discard rules line, under the rates: the same words as the menu bar's
/// rules line. A problem is red in the dot and the text, and the words say it
/// too, so the colour is never the only sign.
fn rules_row(theme: &Theme, model: &StatusModel) -> impl IntoElement {
    let problem = model.rules_file_error || model.rules_with_errors > 0;
    let (dot, text) = if problem {
        (theme.error, theme.error_text)
    } else if model.rules_on == 0 {
        (theme.text_tertiary, theme.text_secondary)
    } else {
        (theme.ok, theme.text_secondary)
    };
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_1()
        .text_color(text)
        .child(div().size(px(8.0)).rounded_full().bg(dot))
        .child(rules_line(model))
}

/// The endpoint list: a row per listener, with the OS error under a failed
/// one.
fn endpoints(theme: &Theme, model: &StatusModel) -> impl IntoElement {
    let last = model.listeners.len().saturating_sub(1);
    div()
        .flex()
        .flex_col()
        .rounded_lg()
        .bg(theme.panel)
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .px_4()
                .py_3()
                .border_b_1()
                .border_color(theme.border)
                .text_size(px(11.0))
                .text_color(theme.text_tertiary)
                .child("ENDPOINTS"),
        )
        .children(
            model
                .listeners
                .iter()
                .enumerate()
                .map(|(index, listener)| endpoint_row(theme, listener, index == last)),
        )
}

fn endpoint_row(theme: &Theme, listener: &ListenerLine, last: bool) -> impl IntoElement {
    let (dot, state) = if listener.ok {
        (
            theme.ok,
            div()
                .text_color(theme.text_secondary)
                .child("Listening".to_owned()),
        )
    } else {
        let error = listener.error.as_deref().unwrap_or("unknown error");
        (
            theme.error,
            div()
                .text_color(theme.error_text)
                .child(format!("Failed \u{b7} {error}")),
        )
    };
    let row = div()
        .flex()
        .items_center()
        .gap_4()
        .px_4()
        .py_3()
        .child(div().size(px(8.0)).rounded_full().bg(dot))
        .child(
            div()
                .w(px(110.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child(listener.name.clone()),
        )
        .child(
            div()
                .w(px(150.0))
                .text_color(theme.text_secondary)
                .child(listener.address.clone()),
        )
        .child(state);
    if last {
        row
    } else {
        row.border_b_1().border_color(theme.border)
    }
}
