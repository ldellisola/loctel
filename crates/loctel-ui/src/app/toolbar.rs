//! The toolbar over the content: a title for the screen, and on the home
//! screen the four controls of design §9.2 (on the Logs screen the controls of
//! [`LogsControls`], below).
//!
//! - **Live** shows whether the view is running (`Live`, a green dot) or frozen
//!   (`Paused`, an amber dot).
//! - **The time range** is a button with the current range (`Last 15 minutes`)
//!   that opens a menu of the four ranges ([`TimeRange::ALL`]).
//! - **Pause** freezes the map and the table (`Resume` lets them run again);
//!   ingest carries on either way.
//! - **Clear all** empties the store.
//!
//! [`HomeControls`] holds no state of the data: every control calls the
//! [`HomeModel`] (`set_range`, `set_paused`, `clear_all`), which owns what they
//! set, and the controls redraw when the model changes. The one thing the view
//! keeps is whether the range menu is open. Decision 0032 has the choices.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    Anchor, AnyElement, Bounds, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement, Pixels, Point, Render, Rgba, StatefulInteractiveElement as _, Styled,
    Subscription, Window, canvas, div, prelude::FluentBuilder as _, px,
};
use gpui_kit::component::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;

use super::data::{HomeModel, TimeRange};
use super::logs_data::LogsModel;
use super::shell::Screen;
use crate::Theme;
use crate::model::format;

/// How many characters of a trace ID the toolbar shows (the middle is cut).
const TRACE_ID_CHARS: usize = 17;

/// What the Live indicator says while the view runs.
pub const LIVE_LABEL: &str = "Live";
/// What it says while the view is frozen.
pub const PAUSED_LABEL: &str = "Paused";
/// The Pause button's label while the view runs.
pub const PAUSE_LABEL: &str = "Pause";
/// Its label while the view is frozen.
pub const RESUME_LABEL: &str = "Resume";
/// The Clear all button's label.
pub const CLEAR_ALL_LABEL: &str = "Clear all";
/// The Logs toolbar's live tail pill.
pub const LIVE_TAIL_LABEL: &str = "Live tail";

/// The title of the toolbar on `screen`: `Map & traces` on the home screen,
/// `Map & traces › 4bf92f35…0e0e4736` on a trace (the ID cut in the middle:
/// both ends of it identify it), `Logs` and `Discard rules`.
pub fn toolbar_title(screen: &Screen) -> String {
    match screen {
        Screen::Home => "Map & traces".to_owned(),
        Screen::Logs { .. } => "Logs".to_owned(),
        Screen::Rules { .. } => "Discard rules".to_owned(),
        Screen::Trace(id) => format!(
            "Map & traces \u{203a} {}",
            format::truncate_middle(&id.to_string(), TRACE_ID_CHARS)
        ),
    }
}

/// What the Live indicator says: [`LIVE_LABEL`], or [`PAUSED_LABEL`] while the
/// view is frozen.
pub fn live_label(paused: bool) -> &'static str {
    if paused { PAUSED_LABEL } else { LIVE_LABEL }
}

/// What the Pause button says: [`PAUSE_LABEL`], or [`RESUME_LABEL`] while the
/// view is frozen.
pub fn pause_label(paused: bool) -> &'static str {
    if paused { RESUME_LABEL } else { PAUSE_LABEL }
}

/// A part of the home toolbar a test can ask the place of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolbarPart {
    /// The Live indicator.
    Live,
    /// The Logs toolbar's `Live tail` pill.
    LiveTail,
    /// The button that shows the range and opens the menu.
    Range,
    /// An entry of the range menu; present only while the menu is open.
    RangeOption(TimeRange),
    /// The Pause (or Resume) button.
    Pause,
    /// The Clear all button.
    ClearAll,
}

/// Where the parts of the latest frame are, in window coordinates.
type Frame = RefCell<HashMap<ToolbarPart, Bounds<Pixels>>>;

/// An invisible canvas over its parent that writes the parent's bounds into
/// `frame` while the frame is prepainted. Add it as a child of a `relative` box
/// that is the size of the part.
fn measure(frame: &Rc<Frame>, part: ToolbarPart) -> impl IntoElement {
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

fn rgba(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

/// The controls of the home screen's toolbar. Make it with
/// `cx.new(|cx| HomeControls::new(home, cx))`; it reads and writes `home`.
pub struct HomeControls {
    home: Entity<HomeModel>,
    /// The range menu is showing.
    range_open: bool,
    frame: Rc<Frame>,
    /// Keeps the observation of the model alive.
    _home: Subscription,
}

impl HomeControls {
    pub fn new(home: Entity<HomeModel>, cx: &mut Context<Self>) -> HomeControls {
        let subscription = cx.observe(&home, |_, _, cx| cx.notify());
        HomeControls {
            home,
            range_open: false,
            frame: Rc::new(Frame::default()),
            _home: subscription,
        }
    }

    /// Whether the range menu is open.
    pub fn range_open(&self) -> bool {
        self.range_open
    }

    /// Where `part` is in the window, as of the latest frame. `None` while it is
    /// not on the screen (the toolbar of another screen, a closed range menu).
    pub fn bounds_of(&self, part: ToolbarPart) -> Option<Bounds<Pixels>> {
        self.frame.borrow().get(&part).copied()
    }

    /// The middle of `part`, as of the latest frame.
    pub fn center_of(&self, part: ToolbarPart) -> Option<Point<Pixels>> {
        self.bounds_of(part).map(|bounds| bounds.center())
    }

    /// What the Live indicator says now.
    pub fn live_text(&self, cx: &gpui::App) -> &'static str {
        live_label(self.home.read(cx).paused())
    }

    /// What the Pause button says now.
    pub fn pause_text(&self, cx: &gpui::App) -> &'static str {
        pause_label(self.home.read(cx).paused())
    }

    /// Forgets where the parts were: the toolbar of another screen draws none of
    /// them, and the shell calls this while it shows one.
    pub(super) fn forget_places(&self) {
        self.frame.borrow_mut().clear();
    }

    fn set_range_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.range_open != open {
            self.range_open = open;
            cx.notify();
        }
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        self.home.update(cx, |home, cx| {
            let paused = home.paused();
            home.set_paused(!paused, cx);
        });
    }

    fn clear_all(&mut self, cx: &mut Context<Self>) {
        self.home.update(cx, |home, cx| home.clear_all(cx));
    }
}

impl Render for HomeControls {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Every part draws itself again below and measures itself: what was
        // there last frame and is gone now must not be found.
        self.frame.borrow_mut().clear();

        let theme = *cx.global::<Theme>();
        let (paused, range) = {
            let home = self.home.read(cx);
            (home.paused(), home.range())
        };
        let frame = &self.frame;
        let controls = cx.entity();

        let live = div()
            .relative()
            .flex_none()
            .h(px(26.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(7.0))
            .rounded(px(14.0))
            .border_1()
            .border_color(theme.border)
            .text_size(px(12.0))
            .text_color(theme.text_secondary)
            .child(
                div()
                    .size(px(8.0))
                    .flex_none()
                    .rounded_full()
                    .bg(if paused { theme.warn } else { theme.ok }),
            )
            .child(live_label(paused))
            .child(measure(frame, ToolbarPart::Live));

        let range_button = Popover::new("toolbar-range")
            .anchor(Anchor::TopLeft)
            .open(self.range_open)
            .on_open_change({
                let controls = controls.clone();
                move |open, _window, cx| {
                    let open = *open;
                    controls.update(cx, |controls, cx| controls.set_range_open(open, cx));
                }
            })
            .trigger(
                Button::new("toolbar-range-button")
                    .label(range.label())
                    .outline()
                    .dropdown_caret(true),
            )
            .p(px(4.0))
            .content({
                let home = self.home.clone();
                let frame = Rc::clone(frame);
                move |_state, _window, _cx| {
                    let pick = {
                        let home = home.clone();
                        let controls = controls.clone();
                        move |option: TimeRange, cx: &mut gpui::App| {
                            home.update(cx, |home, cx| home.set_range(option, cx));
                            controls.update(cx, |controls, cx| controls.set_range_open(false, cx));
                        }
                    };
                    range_menu(&theme, range, &frame, pick)
                }
            });
        let range = div()
            .relative()
            .flex_none()
            .child(range_button)
            .child(measure(frame, ToolbarPart::Range));

        let pause_button = Button::new("toolbar-pause")
            .label(pause_label(paused))
            .icon(if paused {
                IconName::Play
            } else {
                IconName::Pause
            });
        let pause_button = if paused {
            pause_button.primary()
        } else {
            pause_button.outline()
        };
        let pause = div()
            .relative()
            .flex_none()
            .child(
                pause_button.on_click(cx.listener(|controls, _, _, cx| controls.toggle_pause(cx))),
            )
            .child(measure(frame, ToolbarPart::Pause));

        let clear = div()
            .relative()
            .flex_none()
            .child(
                Button::new("toolbar-clear-all")
                    .label(CLEAR_ALL_LABEL)
                    .icon(IconName::Delete)
                    .outline()
                    .on_click(cx.listener(|controls, _, _, cx| controls.clear_all(cx))),
            )
            .child(measure(frame, ToolbarPart::ClearAll));

        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(live)
            .child(range)
            .child(pause)
            .child(clear)
    }
}

/// The entries of the range menu: the four ranges, the current one lit. A click
/// calls `pick` with the range (which sets it and closes the menu: the home
/// toolbar's and the Logs toolbar's differ in what they set).
fn range_menu(
    theme: &Theme,
    current: TimeRange,
    frame: &Rc<Frame>,
    pick: impl Fn(TimeRange, &mut gpui::App) + Clone + 'static,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .min_w(px(176.0))
        .children(
            TimeRange::ALL
                .into_iter()
                .enumerate()
                .map(|(index, option)| {
                    let active = option == current;
                    let pick = pick.clone();
                    div()
                        .relative()
                        .child(
                            div()
                                .id(("toolbar-range-option", index))
                                .h(px(30.0))
                                .px(px(10.0))
                                .flex()
                                .items_center()
                                .rounded(px(8.0))
                                .cursor_pointer()
                                .text_size(px(13.0))
                                .text_color(theme.text)
                                .when(active, |row| {
                                    row.bg(rgba(theme.accent, 0.18))
                                        .font_weight(FontWeight::SEMIBOLD)
                                })
                                .when(!active, |row| {
                                    let hover = rgba(theme.text, 0.06);
                                    row.hover(move |style| style.bg(hover))
                                })
                                .on_click(move |_, _window, cx| pick(option, cx))
                                .child(option.label()),
                        )
                        .child(measure(frame, ToolbarPart::RangeOption(option)))
                }),
        )
        .into_any_element()
}

/// The controls of the Logs screen's toolbar: the `Live tail` pill, the range
/// button with its menu (the same menu as the home toolbar's) and `Clear all`.
/// Make it with `cx.new(|cx| LogsControls::new(logs, cx))`; it reads and writes
/// `logs`.
///
/// Like [`HomeControls`] it holds no data: the model owns the tail switch and
/// the range, and the controls redraw when it changes. What it keeps is whether
/// the range menu is open and where its parts were drawn.
pub struct LogsControls {
    logs: Entity<LogsModel>,
    range_open: bool,
    frame: Rc<Frame>,
    /// Keeps the observation of the model alive.
    _logs: Subscription,
}

impl LogsControls {
    pub fn new(logs: Entity<LogsModel>, cx: &mut Context<Self>) -> LogsControls {
        let subscription = cx.observe(&logs, |_, _, cx| cx.notify());
        LogsControls {
            logs,
            range_open: false,
            frame: Rc::new(Frame::default()),
            _logs: subscription,
        }
    }

    /// Whether the range menu is open.
    pub fn range_open(&self) -> bool {
        self.range_open
    }

    /// Where `part` is in the window, as of the latest frame. `None` while it is
    /// not on the screen (the toolbar of another screen, a closed range menu).
    pub fn bounds_of(&self, part: ToolbarPart) -> Option<Bounds<Pixels>> {
        self.frame.borrow().get(&part).copied()
    }

    /// The middle of `part`, as of the latest frame.
    pub fn center_of(&self, part: ToolbarPart) -> Option<Point<Pixels>> {
        self.bounds_of(part).map(|bounds| bounds.center())
    }

    /// Forgets where the parts were: the toolbar of another screen draws none of
    /// them, and the shell calls this while it shows one.
    pub(super) fn forget_places(&self) {
        self.frame.borrow_mut().clear();
    }

    fn set_range_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.range_open != open {
            self.range_open = open;
            cx.notify();
        }
    }
}

impl Render for LogsControls {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame.borrow_mut().clear();

        let theme = *cx.global::<Theme>();
        let (tail, paused, range) = {
            let logs = self.logs.read(cx);
            // Standing still: scrolled away from the top, or showing the lines
            // around one log.
            (
                logs.tail(),
                logs.tail_paused() || logs.context().is_some(),
                logs.range(),
            )
        };
        let frame = &self.frame;
        let controls = cx.entity();

        // On: the green dot of a tail that runs, an amber one while scrolling
        // away (or the lines around a log) has stopped it; off: a hollow ring
        // and no tint.
        let dot = div().size(px(8.0)).flex_none().rounded_full();
        let dot = if !tail {
            dot.border_1().border_color(theme.text_tertiary)
        } else if paused {
            dot.bg(theme.warn)
        } else {
            dot.bg(theme.ok)
        };
        let live_tail = div()
            .relative()
            .flex_none()
            .child(
                div()
                    .id("logs-live-tail")
                    .h(px(26.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .rounded(px(14.0))
                    .border_1()
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .when(tail, |pill| {
                        pill.border_color(theme.accent)
                            .bg(rgba(theme.accent, 0.16))
                            .text_color(theme.accent)
                    })
                    .when(!tail, |pill| {
                        pill.border_color(theme.border)
                            .text_color(theme.text_secondary)
                    })
                    .on_click(cx.listener(|controls, _, _, cx| {
                        controls.logs.update(cx, |logs, cx| {
                            let on = !logs.tail();
                            logs.set_tail(on, cx);
                        });
                    }))
                    .child(dot)
                    .child(LIVE_TAIL_LABEL),
            )
            .child(measure(frame, ToolbarPart::LiveTail));

        let range_button = Popover::new("logs-toolbar-range")
            .anchor(Anchor::TopLeft)
            .open(self.range_open)
            .on_open_change({
                let controls = controls.clone();
                move |open, _window, cx| {
                    let open = *open;
                    controls.update(cx, |controls, cx| controls.set_range_open(open, cx));
                }
            })
            .trigger(
                Button::new("logs-toolbar-range-button")
                    .label(range.label())
                    .outline()
                    .dropdown_caret(true),
            )
            .p(px(4.0))
            .content({
                let logs = self.logs.clone();
                let frame = Rc::clone(frame);
                move |_state, _window, _cx| {
                    let pick = {
                        let logs = logs.clone();
                        let controls = controls.clone();
                        move |option: TimeRange, cx: &mut gpui::App| {
                            logs.update(cx, |logs, cx| logs.set_range(option, cx));
                            controls.update(cx, |controls, cx| controls.set_range_open(false, cx));
                        }
                    };
                    range_menu(&theme, range, &frame, pick)
                }
            });
        let range = div()
            .relative()
            .flex_none()
            .child(range_button)
            .child(measure(frame, ToolbarPart::Range));

        let clear = div()
            .relative()
            .flex_none()
            .child(
                Button::new("logs-clear-all")
                    .label(CLEAR_ALL_LABEL)
                    .icon(IconName::Delete)
                    .outline()
                    .on_click(cx.listener(|controls, _, _, cx| {
                        controls.logs.update(cx, |logs, cx| logs.clear_all(cx));
                    })),
            )
            .child(measure(frame, ToolbarPart::ClearAll));

        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(live_tail)
            .child(range)
            .child(clear)
    }
}

/// The toolbar's contents on `screen`, inside the glass frame the shell draws
/// around it: the title, and at the right the controls of the screen (the home
/// screen's, or the Logs screen's; a trace and the rules screen have the title
/// alone).
pub(super) fn toolbar_contents(
    theme: &Theme,
    screen: &Screen,
    controls: &Entity<HomeControls>,
    logs_controls: &Entity<LogsControls>,
) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .gap_3()
        .px(px(18.0))
        .child(
            div()
                .min_w_0()
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(toolbar_title(screen)),
        )
        .when(matches!(screen, Screen::Home), |toolbar| {
            toolbar.child(div().flex_1()).child(controls.clone())
        })
        .when(matches!(screen, Screen::Logs { .. }), |toolbar| {
            toolbar.child(div().flex_1()).child(logs_controls.clone())
        })
}
