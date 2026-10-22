//! The menu bar popover (design 9.2, M4 plan Task 8): the panel that opens under
//! the menu bar icon and replaces the M1 and M2 menu. This is the view, and it
//! is cross-platform: what opens it under the icon, and what its buttons do, is
//! [`PopoverActions`] (`loctel-macos` implements it, Task 9; the tests record it).
//!
//! The panel is 336 wide, with corners of 18, and from the top it has:
//!
//! 1. the title `loctel` and the status pill (`Receiving`, `Paused` or `Port
//!    problem`, a dot of its colour and its words)
//! 2. a grid of four stats, two by two: spans and log lines per second, traces
//!    and log lines held
//! 3. the endpoints, `OTLP gRPC`, `OTLP HTTP` and `MCP` with their addresses (a
//!    red dot, red text and the word `failed` for one that could not be bound),
//!    and a `Copy` button on the MCP row while it is listening
//! 4. the discard rules line, with a chevron: a click opens the main window on
//!    Discard rules
//! 5. a divider
//! 6. `Open loctel` (⌘O), `Pause receiving` or `Resume receiving`, and `Clear all
//!    data…`, which asks first, in the row: `Clear all traces and logs?` with
//!    `Clear` and `Cancel`
//! 7. where there is a login item (macOS), `Start at login` and its switch
//! 8. `Settings…` (⌘,)
//! 9. a divider
//! 10. `Quit loctel` (⌘Q)
//!
//! What it says is [`popover::from_status`](crate::model::popover::from_status)'s,
//! read when the view opens, once a second after that, and at once after each
//! action that changes what it shows (so `Pause receiving` reads `Resume
//! receiving` as soon as it is clicked). The view holds no rules of its own.
//!
//! **Closing.** The popover closes when it loses focus (the window's, Task 9), on
//! Esc, and after an action that opens a window (`Open loctel`, `Settings…`, the
//! rules line): the view calls [`PopoverActions::close`] for Esc and after those
//! actions. Pause, `Copy`, the login switch and a confirmed clear leave it open.
//!
//! **Keys.** The panel has a key context of its own (`LoctelPopover`) with Esc,
//! ⌘O, ⌘, and ⌘Q bound to the same things as the rows, so they work wherever in
//! the panel the keyboard is. The view takes the keyboard when it is first drawn
//! and takes it back whenever what had it leaves the frame, as the main window's
//! shell does (decision 0040).
//!
//! **Glass.** [`PopoverView::with_backdrop`] attaches the platform's backdrop
//! (`loctel_macos::glass::attach`, Task 9). With one the panel has no fill, so
//! the glass shows, and the tiles and rows are translucent; the view clears the
//! window colour Kit's `Root` paints over it, and tells the backdrop where the
//! panel is: one [`GlassRegion`], whenever it changes. Without one the panel is
//! opaque, which is how it looks on Linux and in the tests.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, Bounds, Context, FocusHandle, Focusable, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Pixels, Point, Render,
    Rgba, StatefulInteractiveElement as _, Styled as _, Subscription, Window, actions, canvas, div,
    prelude::FluentBuilder as _, px,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _};

use crate::app::{
    Backdrop, GlassRegion, LoginItem, Screen, UiBackend, clear_root_background, kit_theme,
};
use crate::model::popover::{self, Popover, Tone};
use crate::{FONT_FAMILY, StatusModel, Theme};

/// The panel's width, in logical pixels.
pub const POPOVER_WIDTH: f32 = 336.0;
/// The radius of the panel's corners.
pub const POPOVER_RADIUS: f32 = 18.0;

/// How often the popover reads the engine again.
pub const POPOVER_REFRESH: Duration = Duration::from_secs(1);

/// The title.
const TITLE: &str = "loctel";
/// The words of the rows, and of the question that replaces the clear row.
pub const OPEN_LOCTEL: &str = "Open loctel";
pub const PAUSE_RECEIVING: &str = "Pause receiving";
pub const RESUME_RECEIVING: &str = "Resume receiving";
pub const CLEAR_ALL_DATA: &str = "Clear all data\u{2026}";
pub const START_AT_LOGIN: &str = "Start at login";
pub const OPEN_SETTINGS: &str = "Settings\u{2026}";
pub const QUIT: &str = "Quit loctel";
pub const CLEAR_ASK: &str = "Clear all traces and logs?";
/// The buttons of the question, and of the MCP row.
pub const CLEAR_YES: &str = "Clear";
pub const CLEAR_NO: &str = "Cancel";
pub const COPY: &str = "Copy";
/// The shortcuts shown at the right of the rows.
const SHORTCUT_OPEN: &str = "\u{2318}O";
const SHORTCUT_SETTINGS: &str = "\u{2318},";
const SHORTCUT_QUIT: &str = "\u{2318}Q";

/// The key context of the panel: where the bindings of [`bind_keys`] apply. Not
/// `Popover`, which GPUI Kit's own popover binds its keys in.
const CONTEXT: &str = "LoctelPopover";

// --- the layout, in logical pixels ---------------------------------------------------------
//
// Every row has a fixed height, so the panel is exactly as tall as the sum of
// them (the popover's window is sized to it, see `popover_height`) and asking to
// clear, which swaps one row for another, moves nothing.

/// The padding of the panel's sides and top, inside its hairline.
const PAD: f32 = 12.0;
/// The padding under the last row.
const PAD_BOTTOM: f32 = 8.0;
/// The hairline round the panel.
const BORDER: f32 = 1.0;
/// The title and the pill.
const HEADER_HEIGHT: f32 = 28.0;
/// One stat tile, and the gap between tiles.
const TILE_HEIGHT: f32 = 56.0;
const TILE_GAP: f32 = 8.0;
/// One row of the endpoints, the rules line and the actions.
const ROW_HEIGHT: f32 = 28.0;
/// The width of the endpoints' names (`OTLP gRPC`, `OTLP HTTP`, `MCP`), so their
/// addresses line up.
const NAME_WIDTH: f32 = 72.0;
/// The gap between the header, the tiles, the endpoints and the rules line.
const SECTION_GAP: f32 = 10.0;
/// The gap between the endpoints and the rules line.
const RULES_GAP: f32 = 6.0;
/// A divider: the line and the space above and below it.
const DIVIDER_LINE: f32 = 1.0;
const DIVIDER_SPACE: f32 = 6.0;
/// How many rows the panel has: three endpoints, the rules line and the
/// actions (open, pause, clear, settings, quit; the login row is one more).
const ENDPOINT_ROWS: usize = 3;
const ACTION_ROWS: usize = 5;

/// The height of the panel for a platform with a login item or without, in
/// logical pixels: the window of the popover is this tall (and
/// [`POPOVER_WIDTH`] wide). A login item adds the `Start at login` row.
pub fn popover_height(login_item: bool) -> f32 {
    let divider = DIVIDER_LINE + 2.0 * DIVIDER_SPACE;
    let actions = ACTION_ROWS + usize::from(login_item);
    2.0 * BORDER
        + PAD
        + HEADER_HEIGHT
        + SECTION_GAP
        + (2.0 * TILE_HEIGHT + TILE_GAP)
        + SECTION_GAP
        + (ENDPOINT_ROWS as f32 * ROW_HEIGHT + 2.0 * BORDER)
        + RULES_GAP
        + ROW_HEIGHT
        + divider
        + (actions - 1) as f32 * ROW_HEIGHT
        + divider
        + ROW_HEIGHT
        + PAD_BOTTOM
}

actions!(
    loctel_popover,
    [
        /// Esc: close the popover.
        ClosePopover,
        /// ⌘O: open the main window.
        OpenLoctel,
        /// ⌘,: open the Settings window.
        OpenPopoverSettings,
        /// ⌘Q: quit loctel.
        QuitLoctel,
    ]
);

/// Binds the popover's keys in its key context. [`app::init`](crate::app::init)
/// calls it once per app.
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", ClosePopover, Some(CONTEXT)),
        KeyBinding::new("cmd-o", OpenLoctel, Some(CONTEXT)),
        KeyBinding::new("cmd-,", OpenPopoverSettings, Some(CONTEXT)),
        KeyBinding::new("cmd-q", QuitLoctel, Some(CONTEXT)),
    ]);
}

/// What the popover asks of the app for what it cannot do itself. The macOS app
/// implements it (Task 9: windows, the engine, the clipboard, the login item);
/// the tests record it.
///
/// The view calls these from inside an event handler, so an implementation that
/// needs the GPUI app (to open a window) must not assume it can borrow it there:
/// hold an `AsyncApp` and run the work on the foreground executor.
pub trait PopoverActions: 'static {
    /// Opens the main window (or brings it forward) on `screen`.
    fn open_main(&self, screen: Screen);
    /// Opens the Settings window, or brings it forward.
    fn open_settings(&self);
    /// Pauses receiving, or resumes it if it is paused.
    fn toggle_pause(&self);
    /// Empties the store. The view has already asked the user.
    fn clear_all(&self);
    /// Puts the MCP URL on the clipboard.
    fn copy_mcp(&self);
    /// Switches the login item. The system may refuse; the view reads what it
    /// holds afterwards and shows that.
    fn set_start_at_login(&self, on: bool);
    /// Shuts the engine down and quits.
    fn quit(&self);
    /// Closes the popover.
    fn close(&self);
}

/// The parts of the popover a test can ask the place of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PopoverPart {
    /// The panel itself: its border box.
    Panel,
    /// The status pill.
    Pill,
    /// One of the four stat tiles, in the order of [`Popover::stats`].
    Stat(usize),
    /// One of the three endpoint rows, in the order of [`Popover::endpoints`].
    Endpoint(usize),
    /// The `Copy` button of the MCP row; there only while MCP is listening.
    CopyMcp,
    /// The discard rules line.
    RulesLine,
    /// `Open loctel`.
    OpenMain,
    /// `Pause receiving` or `Resume receiving`.
    Pause,
    /// `Clear all data…`; not there while it asks.
    ClearAll,
    /// The `Clear` button of the question; there only while it asks.
    ConfirmClear,
    /// The `Cancel` button of the question; there only while it asks.
    CancelClear,
    /// The `Start at login` switch; there only with a login item.
    LoginSwitch,
    /// `Settings…`.
    Settings,
    /// `Quit loctel`.
    Quit,
}

/// Where the parts of the latest frame are, in window coordinates.
type Frame = RefCell<HashMap<PopoverPart, Bounds<Pixels>>>;

/// An invisible canvas over its parent that writes the parent's bounds into
/// `frame` while the frame is prepainted. Add it as a child of a `relative` box
/// that is the size of the part.
fn measure(frame: &Rc<Frame>, part: PopoverPart) -> impl IntoElement {
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

/// The backdrop, and what was last told to it.
struct GlassReport {
    backdrop: Option<Box<dyn Backdrop>>,
    reported: RefCell<Option<GlassRegion>>,
}

impl GlassReport {
    fn new(backdrop: Option<Box<dyn Backdrop>>) -> GlassReport {
        GlassReport {
            backdrop,
            reported: RefCell::new(None),
        }
    }

    /// Passes the panel's `bounds` to the backdrop as its one region, when they
    /// differ from the last time. With no backdrop there is nothing to do.
    fn report(&self, bounds: Bounds<Pixels>) {
        let Some(backdrop) = &self.backdrop else {
            return;
        };
        let region = GlassRegion {
            bounds,
            corner_radius: px(POPOVER_RADIUS),
        };
        let mut reported = self.reported.borrow_mut();
        if reported.as_ref() != Some(&region) {
            backdrop.set_regions(std::slice::from_ref(&region));
            *reported = Some(region);
        }
    }
}

/// One row of the actions, in the order they are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Open,
    Pause,
    Clear,
    Login,
    Settings,
    Quit,
}

/// The popover's root view. Make it with
/// `cx.new(|cx| PopoverView::new(backend, actions, cx))`, add
/// [`with_backdrop`](Self::with_backdrop) where the platform has glass, and put
/// it in a window of 336 by [`popover_height`] inside GPUI Kit's `Root`.
pub struct PopoverView {
    backend: Rc<dyn UiBackend>,
    actions: Rc<dyn PopoverActions>,
    login: Option<Rc<dyn LoginItem>>,
    /// What is drawn.
    popover: Popover,
    /// The same status as the model the status line and the rules line are made
    /// from: the rules line's colour needs what the popover's words do not say.
    status: StatusModel,
    /// `Clear all data…` has been clicked and the row is asking.
    confirming: bool,
    frame: Rc<Frame>,
    glass: Rc<GlassReport>,
    focus: FocusHandle,
    /// The things that need the window have been done: the first frame does them.
    prepared: bool,
    /// Keeps the appearance observer and the focus watcher alive.
    _window: Vec<Subscription>,
}

impl PopoverView {
    /// The popover for what `backend` says now, refreshed every second
    /// ([`POPOVER_REFRESH`]) for as long as the view lives. The theme comes from
    /// the window's appearance when the first frame is drawn, and follows it.
    pub fn new(
        backend: Rc<dyn UiBackend>,
        actions: Rc<dyn PopoverActions>,
        cx: &mut Context<Self>,
    ) -> PopoverView {
        let login = backend.login_item();
        let (popover, status) = read(&*backend, login.as_deref());

        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(POPOVER_REFRESH).await;
                // The view is gone once the window closes; that ends the loop.
                if view.update(cx, |view, cx| view.reload(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        PopoverView {
            backend,
            actions,
            login,
            popover,
            status,
            confirming: false,
            frame: Rc::new(Frame::default()),
            glass: Rc::new(GlassReport::new(None)),
            focus: cx.focus_handle(),
            prepared: false,
            _window: Vec::new(),
        }
    }

    /// Attaches the platform's backdrop (`None` where there is none): the panel
    /// is then glass, and the backdrop is told where it is.
    pub fn with_backdrop(mut self, backdrop: Option<Box<dyn Backdrop>>) -> PopoverView {
        self.glass = Rc::new(GlassReport::new(backdrop));
        self
    }

    // --- what a test reads ---------------------------------------------------------

    /// What is drawn: the pill, the stats, the endpoints and the rest.
    pub fn popover(&self) -> &Popover {
        &self.popover
    }

    /// The action rows, top to bottom, as `(words, shortcut)`: `Open loctel`
    /// (⌘O), `Pause receiving` or `Resume receiving`, `Clear all data…` (the
    /// question while it asks), `Start at login` where there is a login item,
    /// `Settings…` (⌘,) and `Quit loctel` (⌘Q).
    pub fn action_rows(&self) -> Vec<(&'static str, Option<&'static str>)> {
        self.items()
            .into_iter()
            .map(|item| self.words(item))
            .collect()
    }

    /// The endpoint rows as drawn, top to bottom: the name and the address
    /// (`127.0.0.1:4317`, `127.0.0.1:4319/mcp`, and `127.0.0.1:4318 · failed` for
    /// one that is not listening).
    pub fn endpoint_lines(&self) -> Vec<(&'static str, String)> {
        self.popover
            .endpoints
            .iter()
            .map(|(name, address, ok)| (*name, shown(address, *ok)))
            .collect()
    }

    /// Whether the clear row is asking `Clear all traces and logs?`.
    pub fn confirming_clear(&self) -> bool {
        self.confirming
    }

    /// Where `part` is in the window, as of the latest frame. `None` while it is
    /// not on the screen.
    pub fn bounds_of(&self, part: PopoverPart) -> Option<Bounds<Pixels>> {
        self.frame.borrow().get(&part).copied()
    }

    /// The middle of `part`, as of the latest frame.
    pub fn center_of(&self, part: PopoverPart) -> Option<Point<Pixels>> {
        self.bounds_of(part).map(|bounds| bounds.center())
    }

    // --- what it shows -------------------------------------------------------------

    /// The action rows that are drawn, in order.
    fn items(&self) -> Vec<Item> {
        let mut items = vec![Item::Open, Item::Pause, Item::Clear];
        if self.popover.start_at_login.is_some() {
            items.push(Item::Login);
        }
        items.extend([Item::Settings, Item::Quit]);
        items
    }

    /// The words of a row and its shortcut.
    fn words(&self, item: Item) -> (&'static str, Option<&'static str>) {
        match item {
            Item::Open => (OPEN_LOCTEL, Some(SHORTCUT_OPEN)),
            Item::Pause if self.popover.paused => (RESUME_RECEIVING, None),
            Item::Pause => (PAUSE_RECEIVING, None),
            Item::Clear if self.confirming => (CLEAR_ASK, None),
            Item::Clear => (CLEAR_ALL_DATA, None),
            Item::Login => (START_AT_LOGIN, None),
            Item::Settings => (OPEN_SETTINGS, Some(SHORTCUT_SETTINGS)),
            Item::Quit => (QUIT, Some(SHORTCUT_QUIT)),
        }
    }

    // --- what the user does --------------------------------------------------------

    /// Reads the engine and the login item again, and redraws if anything the
    /// popover shows changed.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let (popover, status) = read(&*self.backend, self.login.as_deref());
        if popover != self.popover || status != self.status {
            self.popover = popover;
            self.status = status;
            cx.notify();
        }
    }

    /// A row was clicked, or its key pressed.
    fn activate(&mut self, item: Item, cx: &mut Context<Self>) {
        match item {
            Item::Open => {
                self.actions.open_main(Screen::Home);
                self.actions.close();
            }
            Item::Pause => {
                self.actions.toggle_pause();
                self.reload(cx);
            }
            Item::Clear => {
                self.confirming = true;
                cx.notify();
            }
            Item::Login => {
                let on = self.popover.start_at_login != Some(true);
                self.set_login(on, cx);
            }
            Item::Settings => {
                self.actions.open_settings();
                self.actions.close();
            }
            Item::Quit => self.actions.quit(),
        }
    }

    /// The switch was clicked: the system is asked, and then the popover shows
    /// what the system holds, whatever the answer.
    fn set_login(&mut self, on: bool, cx: &mut Context<Self>) {
        self.actions.set_start_at_login(on);
        self.reload(cx);
    }

    /// Esc: the question, if it is open, is forgotten, and the popover closes.
    fn close(&mut self, cx: &mut Context<Self>) {
        if self.confirming {
            self.confirming = false;
            cx.notify();
        }
        self.actions.close();
    }

    /// The rules line was clicked.
    fn open_rules(&mut self) {
        self.actions.open_main(Screen::Rules { draft: None });
        self.actions.close();
    }

    /// `Clear` of the question: the store is emptied, once.
    fn confirm_clear(&mut self, cx: &mut Context<Self>) {
        self.confirming = false;
        self.actions.clear_all();
        self.reload(cx);
        cx.notify();
    }

    /// `Cancel` of the question: the row is as it was.
    fn cancel_clear(&mut self, cx: &mut Context<Self>) {
        self.confirming = false;
        cx.notify();
    }

    /// What the first frame does, which needs the window: the theme from its
    /// appearance (and following it), the keyboard, and with a backdrop the
    /// window colour cleared (Kit's `Root` paints it over the glass).
    fn prepare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prepared {
            return;
        }
        self.prepared = true;
        kit_theme::apply(&Theme::for_appearance(window.appearance()), cx);
        self._window
            .push(window.observe_window_appearance(|window, cx| {
                kit_theme::apply(&Theme::for_appearance(window.appearance()), cx);
            }));
        // Whatever had the keyboard and is not drawn any more hands it back: Esc
        // and the shortcuts need it inside the panel.
        self._window
            .push(cx.on_focus_lost(window, |view, window, cx| {
                window.focus(&view.focus, cx);
            }));
        let focus = self.focus.clone();
        window.defer(cx, move |window, cx| window.focus(&focus, cx));
        if self.glass.backdrop.is_some() {
            window.defer(cx, |window, cx| clear_root_background(window, cx));
        }
    }
}

impl Focusable for PopoverView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// What the engine and the login item say, as the popover and as the status
/// model its rules line's colour comes from.
fn read(backend: &dyn UiBackend, login: Option<&dyn LoginItem>) -> (Popover, StatusModel) {
    let status = backend.status();
    let popover = popover::from_status(&status, login.map(|login| login.is_enabled()));
    (popover, StatusModel::from_status(&status))
}

/// The word that marks an endpoint that is not listening, after its address, so
/// the row says it in words as well as in red.
const FAILED: &str = "failed";

/// An endpoint's address as the row shows it: the MCP URL without its scheme
/// (`127.0.0.1:4319/mcp`), which would not fit beside the `Copy` button (`Copy`
/// puts the whole URL on the clipboard), and `127.0.0.1:4318 · failed` for
/// an endpoint that could not be bound.
fn shown(address: &str, ok: bool) -> String {
    let address = address.strip_prefix("http://").unwrap_or(address);
    if ok {
        address.to_owned()
    } else {
        format!("{address} \u{b7} {FAILED}")
    }
}

/// The surface of a tile or a group of rows: the panel colour, translucent over
/// glass.
fn fill(theme: &Theme, glass: bool) -> Rgba {
    if glass {
        rgba(theme.panel, if theme.dark { 0.45 } else { 0.55 })
    } else {
        theme.panel
    }
}

/// The hairline of a tile, a group and a divider.
fn line(theme: &Theme, glass: bool) -> Rgba {
    if glass {
        theme.glass_hairline()
    } else {
        theme.border
    }
}

/// The colour of a pill's or a row's dot for a tone.
fn tone_colors(theme: &Theme, tone: Tone) -> (Rgba, Rgba) {
    match tone {
        Tone::Ok => (theme.ok, theme.text),
        Tone::Warn => (theme.warn, theme.text),
        Tone::Error => (theme.error, theme.error_text),
    }
}

impl Render for PopoverView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prepare(window, cx);
        self.frame.borrow_mut().clear();
        let theme = *cx.global::<Theme>();
        let glass = self.glass.backdrop.is_some();
        let mono = cx.theme().mono_font_family.clone();
        let frame = Rc::clone(&self.frame);
        let fill = fill(&theme, glass);
        let line = line(&theme, glass);
        let hover = rgba(theme.text, 0.07);

        // --- the title and the pill ---------------------------------------------------
        let (dot, ink) = tone_colors(&theme, self.popover.pill.tone);
        let pill = div()
            .relative()
            .flex_none()
            .h(px(24.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(7.0))
            .rounded_full()
            .border_1()
            .border_color(line)
            .bg(fill)
            .text_size(px(12.0))
            .text_color(ink)
            .child(div().flex_none().size(px(7.0)).rounded_full().bg(dot))
            .child(self.popover.pill.text)
            .child(measure(&frame, PopoverPart::Pill));
        let header = div()
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(16.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(TITLE),
            )
            .child(pill);

        // --- the stats ---------------------------------------------------------------
        let tile = |index: usize| {
            let (value, label) = &self.popover.stats[index];
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(TILE_HEIGHT))
                .px(px(12.0))
                .flex()
                .flex_col()
                .justify_center()
                .rounded(px(10.0))
                .border_1()
                .border_color(line)
                .bg(fill)
                .child(
                    div()
                        .truncate()
                        .text_size(px(20.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(value.clone()),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.5))
                        .text_color(theme.text_tertiary)
                        .child(*label),
                )
                .child(measure(&frame, PopoverPart::Stat(index)))
        };
        let stats = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(TILE_GAP))
            .child(div().flex().gap(px(TILE_GAP)).child(tile(0)).child(tile(1)))
            .child(div().flex().gap(px(TILE_GAP)).child(tile(2)).child(tile(3)));

        // --- the endpoints -----------------------------------------------------------
        let last = self.popover.endpoints.len().saturating_sub(1);
        let endpoints = div()
            .flex_none()
            .flex()
            .flex_col()
            .rounded(px(10.0))
            .border_1()
            .border_color(line)
            .bg(fill)
            .overflow_hidden()
            .children(self.popover.endpoints.iter().enumerate().map(
                |(index, (name, address, ok))| {
                    let (dot, ink) = if *ok {
                        (theme.ok, theme.text)
                    } else {
                        (theme.error, theme.error_text)
                    };
                    let copy = (*name == "MCP" && *ok).then(|| {
                        div()
                            .relative()
                            .flex_none()
                            .child(
                                Button::new("popover-copy")
                                    .label(COPY)
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|view, _, _, _cx| {
                                        view.actions.copy_mcp();
                                    })),
                            )
                            .child(measure(&frame, PopoverPart::CopyMcp))
                    });
                    div()
                        .relative()
                        .h(px(ROW_HEIGHT))
                        .flex_none()
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .when(index != last, |row| row.border_b_1().border_color(line))
                        .child(div().flex_none().size(px(7.0)).rounded_full().bg(dot))
                        .child(
                            div()
                                .flex_none()
                                .w(px(NAME_WIDTH))
                                .whitespace_nowrap()
                                .text_size(px(12.5))
                                .text_color(theme.text_secondary)
                                .child(*name),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(mono.clone())
                                .text_size(px(11.5))
                                .text_color(ink)
                                .child(shown(address, *ok)),
                        )
                        .children(copy)
                        .child(measure(&frame, PopoverPart::Endpoint(index)))
                },
            ));

        // --- the rules line ----------------------------------------------------------
        let problem = self.status.rules_file_error || self.status.rules_with_errors > 0;
        let (rules_dot, rules_ink) = if problem {
            (theme.error, theme.error_text)
        } else if self.status.rules_on == 0 {
            (theme.text_tertiary, theme.text_secondary)
        } else {
            (theme.ok, theme.text_secondary)
        };
        let rules = div()
            .id("popover-rules")
            .relative()
            .flex_none()
            .h(px(ROW_HEIGHT))
            // The dot lines up with the dots of the endpoints above, which are
            // inside their group's hairline.
            .px(px(10.0 + BORDER))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(px(8.0))
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(|view, _, _, _cx| view.open_rules()))
            .child(div().flex_none().size(px(7.0)).rounded_full().bg(rules_dot))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.5))
                    .text_color(rules_ink)
                    .child(self.popover.rules_line.clone()),
            )
            .child(
                Icon::new(IconName::ChevronRight)
                    .size(px(14.0))
                    .text_color(Hsla::from(theme.text_tertiary)),
            )
            .child(measure(&frame, PopoverPart::RulesLine));

        // --- the actions -------------------------------------------------------------
        let rows = self
            .items()
            .into_iter()
            .enumerate()
            .map(|(index, item)| self.action_row(item, index, &theme, hover, &frame, cx))
            .collect::<Vec<_>>();
        let mut rows = rows.into_iter();
        // `Quit loctel` is the last of them, below the second divider.
        let quit = rows.next_back();
        let divider = || {
            div()
                .flex_none()
                .my(px(DIVIDER_SPACE))
                .h(px(DIVIDER_LINE))
                .w_full()
                .bg(line)
        };

        // --- the panel ---------------------------------------------------------------
        let report = Rc::clone(&self.glass);
        let measure_panel = {
            let frame = Rc::clone(&frame);
            // A border-box measurement: the panel's own bounds, border and all.
            canvas(
                move |bounds, _window, _cx| {
                    frame.borrow_mut().insert(PopoverPart::Panel, bounds);
                    report.report(bounds);
                },
                |_, (), _, _| {},
            )
            .absolute()
            .top(px(-BORDER))
            .left(px(-BORDER))
            .right(px(-BORDER))
            .bottom(px(-BORDER))
        };
        let panel = div()
            .relative()
            .flex_none()
            .w(px(POPOVER_WIDTH))
            .flex()
            .flex_col()
            .px(px(PAD))
            .pt(px(PAD))
            .pb(px(PAD_BOTTOM))
            .rounded(px(POPOVER_RADIUS))
            .overflow_hidden()
            .border_1()
            .border_color(if glass {
                theme.glass_hairline()
            } else {
                theme.border
            })
            .when(!glass, |panel| panel.bg(theme.bg))
            .child(measure_panel)
            .when(glass, |panel| {
                // Inside the hairline, with the radius of its inner edge.
                panel.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .rounded(px(POPOVER_RADIUS - BORDER))
                        .border_1()
                        .border_color(theme.glass_highlight()),
                )
            })
            .child(header)
            .child(div().flex_none().h(px(SECTION_GAP)))
            .child(stats)
            .child(div().flex_none().h(px(SECTION_GAP)))
            .child(endpoints)
            .child(div().flex_none().h(px(RULES_GAP)))
            .child(rules)
            .child(divider())
            .children(rows)
            .child(divider())
            .children(quit);

        div()
            .id("popover")
            .size_full()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|view, _: &ClosePopover, _, cx| view.close(cx)))
            .on_action(cx.listener(|view, _: &OpenLoctel, _, cx| view.activate(Item::Open, cx)))
            .on_action(cx.listener(|view, _: &OpenPopoverSettings, _, cx| {
                view.activate(Item::Settings, cx);
            }))
            .on_action(cx.listener(|view, _: &QuitLoctel, _, cx| view.activate(Item::Quit, cx)))
            .font_family(FONT_FAMILY)
            .text_size(px(13.0))
            .text_color(theme.text)
            .child(panel)
    }
}

impl PopoverView {
    /// One row of the actions: its words at the left, and its shortcut or its
    /// switch at the right. The clear row is the question and its two buttons
    /// while it asks.
    fn action_row(
        &self,
        item: Item,
        index: usize,
        theme: &Theme,
        hover: Rgba,
        frame: &Rc<Frame>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let part = match item {
            Item::Open => PopoverPart::OpenMain,
            Item::Pause => PopoverPart::Pause,
            Item::Clear => PopoverPart::ClearAll,
            Item::Login => PopoverPart::LoginSwitch,
            Item::Settings => PopoverPart::Settings,
            Item::Quit => PopoverPart::Quit,
        };
        let (words, shortcut) = self.words(item);
        let row = div()
            .id(("popover-row", index))
            .relative()
            .flex_none()
            .h(px(ROW_HEIGHT))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(px(8.0))
            .text_size(px(13.0))
            .text_color(theme.text);

        if item == Item::Clear && self.confirming {
            // The question replaces the row: no hover, no click of the row.
            let yes = div()
                .relative()
                .flex_none()
                .child(
                    Button::new("popover-clear-yes")
                        .label(CLEAR_YES)
                        .small()
                        .danger()
                        .on_click(cx.listener(|view, _, _, cx| view.confirm_clear(cx))),
                )
                .child(measure(frame, PopoverPart::ConfirmClear));
            let no = div()
                .relative()
                .flex_none()
                .child(
                    Button::new("popover-clear-no")
                        .label(CLEAR_NO)
                        .small()
                        .outline()
                        .on_click(cx.listener(|view, _, _, cx| view.cancel_clear(cx))),
                )
                .child(measure(frame, PopoverPart::CancelClear));
            return row
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.5))
                        .child(words),
                )
                .child(yes)
                .child(no)
                .into_any_element();
        }

        let row = row
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |view, _, _, cx| view.activate(item, cx)))
            .child(div().flex_1().min_w_0().truncate().child(words));
        let row = match (item, shortcut) {
            (Item::Login, _) => {
                let on = self.popover.start_at_login == Some(true);
                // The switch takes its own click, so the row does not toggle it back.
                row.child(
                    div()
                        .id("popover-login-switch")
                        .relative()
                        .flex_none()
                        .on_click(|_, _window, cx| cx.stop_propagation())
                        .child(Switch::new("popover-login").checked(on).small().on_click(
                            cx.listener(|view, on: &bool, _, cx| {
                                let on = *on;
                                view.set_login(on, cx);
                            }),
                        ))
                        .child(measure(frame, part)),
                )
            }
            (_, Some(shortcut)) => row.child(
                div()
                    .flex_none()
                    .text_size(px(12.0))
                    .text_color(theme.text_tertiary)
                    .child(shortcut),
            ),
            (_, None) => row,
        };
        // Every row but the login row is measured whole.
        let row = if item == Item::Login {
            row
        } else {
            row.child(measure(frame, part))
        };
        row.into_any_element()
    }
}
