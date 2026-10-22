//! The sidebar: the room for the window's traffic lights, the navigation and,
//! at the bottom, the status box.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, Bounds, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement,
    Pixels, Rgba, StatefulInteractiveElement as _, Styled, canvas, div, px,
};
use gpui_kit::component::Icon;

use super::nav::NavItem;
use super::shell::{MainWindow, Screen};
use crate::menu_text::{rules_line, status_line};
use crate::{ListenerLine, StatusModel, Theme};

/// The height of the strip at the top of the sidebar that the macOS traffic
/// lights sit in (on Linux it is just room), in pixels.
pub const TRAFFIC_LIGHT_SPACE: f32 = 52.0;

/// The height of a navigation entry, in pixels.
pub const NAV_ITEM_HEIGHT: f32 = 34.0;

/// The space between two navigation entries, in pixels.
pub const NAV_ITEM_GAP: f32 = 2.0;

/// The size of the icon in front of an entry's text, in pixels.
const NAV_ICON_SIZE: f32 = 16.0;

/// Where the navigation entries were laid out in the latest frame, in window
/// coordinates (for the tests, through `MainWindow::nav_bounds`).
pub(super) type NavFrame = RefCell<HashMap<NavItem, Bounds<Pixels>>>;

/// The sidebar's contents: inside the frame the shell draws.
pub(super) fn sidebar_contents(
    theme: &Theme,
    screen: &Screen,
    status: &StatusModel,
    frame: &Rc<NavFrame>,
    cx: &mut gpui::Context<MainWindow>,
) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(div().h(px(TRAFFIC_LIGHT_SPACE)).flex_none())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(NAV_ITEM_GAP))
                .px(px(10.0))
                .children(
                    NavItem::ALL
                        .iter()
                        .map(|&item| nav_item(theme, item, screen, frame, cx)),
                ),
        )
        .child(div().flex_1())
        .child(status_box(theme, status))
}

/// One entry of the navigation: its icon and its text, lit when it is the
/// current screen's. Its bounds go into `frame` while the frame is prepainted.
fn nav_item(
    theme: &Theme,
    item: NavItem,
    screen: &Screen,
    frame: &Rc<NavFrame>,
    cx: &mut gpui::Context<MainWindow>,
) -> AnyElement {
    let active = item.is_active(screen);
    let measure = {
        let frame = Rc::clone(frame);
        canvas(
            move |bounds, _window, _cx| {
                frame.borrow_mut().insert(item, bounds);
            },
            |_, (), _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    };
    let icon_color = if active {
        theme.accent
    } else {
        theme.text_secondary
    };
    let icon = Icon::new(item.icon())
        .size(px(NAV_ICON_SIZE))
        .text_color(Hsla::from(icon_color));
    let row = div()
        .id(item.label())
        .relative()
        .h(px(NAV_ITEM_HEIGHT))
        .flex()
        .items_center()
        .gap(px(10.0))
        .px(px(12.0))
        .rounded(px(10.0))
        .cursor_pointer()
        .text_size(px(13.0))
        .on_click(cx.listener(move |main, _, _, cx| main.navigate(item.screen(), cx)));
    let row = if active {
        row.bg(Rgba {
            a: 0.18,
            ..theme.accent
        })
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text)
    } else {
        row.text_color(theme.text_secondary).hover(|style| {
            style.bg(Rgba {
                a: 0.06,
                ..theme.text
            })
        })
    };
    row.child(div().flex_none().child(icon))
        .child(item.label())
        .child(measure)
        .into_any_element()
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
