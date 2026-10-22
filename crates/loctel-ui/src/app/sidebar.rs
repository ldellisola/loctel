//! The sidebar: the room for the window's traffic lights, the navigation and,
//! at the bottom, the status box.

use gpui::{
    AnyElement, FontWeight, InteractiveElement as _, IntoElement, ParentElement, Rgba,
    StatefulInteractiveElement as _, Styled, div, px,
};

use super::nav::NavItem;
use super::shell::{MainWindow, Screen};
use crate::menu_text::{rules_line, status_line};
use crate::{ListenerLine, StatusModel, Theme};

/// The height of the strip at the top of the sidebar that the macOS traffic
/// lights sit in (on Linux it is just room), in pixels.
pub const TRAFFIC_LIGHT_SPACE: f32 = 52.0;

/// The height of a navigation entry, in pixels.
pub const NAV_ITEM_HEIGHT: f32 = 34.0;

/// The sidebar's contents: inside the frame the shell draws.
pub(super) fn sidebar_contents(
    theme: &Theme,
    screen: &Screen,
    status: &StatusModel,
    cx: &mut gpui::Context<MainWindow>,
) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(div().h(px(TRAFFIC_LIGHT_SPACE)).flex_none())
        .child(
            div().flex().flex_col().gap(px(2.0)).px(px(10.0)).children(
                NavItem::ALL
                    .iter()
                    .map(|&item| nav_item(theme, item, screen, cx)),
            ),
        )
        .child(div().flex_1())
        .child(status_box(theme, status))
}

/// One entry of the navigation, lit when it is the current screen's.
fn nav_item(
    theme: &Theme,
    item: NavItem,
    screen: &Screen,
    cx: &mut gpui::Context<MainWindow>,
) -> AnyElement {
    let active = item.is_active(screen);
    let row = div()
        .id(item.label())
        .h(px(NAV_ITEM_HEIGHT))
        .flex()
        .items_center()
        .px(px(12.0))
        .rounded(px(10.0))
        .cursor_pointer()
        .text_size(px(13.0))
        .on_click(cx.listener(move |main, _, _, cx| main.navigate(item.screen(), cx)));
    if active {
        row.bg(Rgba {
            a: 0.18,
            ..theme.accent
        })
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text)
        .child(item.label())
        .into_any_element()
    } else {
        row.text_color(theme.text_secondary)
            .hover(|style| {
                style.bg(Rgba {
                    a: 0.06,
                    ..theme.text
                })
            })
            .child(item.label())
            .into_any_element()
    }
}

/// The status box: what the engine is doing in one line (the words the macOS
/// menu header uses), the discard rules line, and where it listens.
fn status_box(theme: &Theme, status: &StatusModel) -> impl IntoElement {
    let dot = if status.listeners.iter().any(|listener| !listener.ok) {
        theme.error
    } else if status.paused {
        theme.warn
    } else {
        theme.ok
    };
    div()
        .m(px(10.0))
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .rounded(px(12.0))
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(8.0))
                .text_size(px(12.0))
                .text_color(theme.text)
                .child(
                    div()
                        .mt(px(4.0))
                        .size(px(8.0))
                        .flex_none()
                        .rounded_full()
                        .bg(dot),
                )
                .child(div().min_w_0().child(status_line(status))),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(theme.text_tertiary)
                .child(rules_line(status)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .pt(px(8.0))
                .border_t_1()
                .border_color(theme.border)
                .children(
                    status
                        .listeners
                        .iter()
                        .map(|listener| endpoint_row(theme, listener)),
                ),
        )
}

/// One endpoint: its name, and the address it listens on (or was asked to
/// and could not, in red).
fn endpoint_row(theme: &Theme, listener: &ListenerLine) -> impl IntoElement {
    let (dot, address) = if listener.ok {
        (theme.ok, theme.text_secondary)
    } else {
        (theme.error, theme.error_text)
    };
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .text_size(px(11.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_color(theme.text_tertiary)
                .child(div().size(px(6.0)).flex_none().rounded_full().bg(dot))
                .child(endpoint_label(&listener.name)),
        )
        .child(div().text_color(address).child(listener.address.clone()))
}

/// The words for an endpoint's name: `otlp-grpc` is `OTLP gRPC`, `otlp-http`
/// is `OTLP HTTP`, `mcp` is `MCP`. An unknown name is shown as it is.
fn endpoint_label(name: &str) -> String {
    match name {
        "otlp-grpc" => "OTLP gRPC".to_owned(),
        "otlp-http" => "OTLP HTTP".to_owned(),
        "mcp" => "MCP".to_owned(),
        other => other.to_owned(),
    }
}
