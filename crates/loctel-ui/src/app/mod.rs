//! The main window: its shell (the floating sidebar, the toolbar and the
//! content), the backend the views read from, and the bridge to GPUI Kit's
//! theme. The screens go in the content area, as views (`src/views`) over the
//! models of `src/model`.
//!
//! - [`UiBackend`]: the store, the engine's status, the discard rules, the
//!   settings, pause and the login item; [`AppBackend`] is the real one over
//!   the engine.
//! - [`HomeModel`]: the home screen's live data (the map, the table's rows) and
//!   the settings that shape it (range, toggles, query, pause).
//! - [`TraceModel`]: the trace screen's live data (the trace read from the store
//!   while it is open, its waterfall, the selection and the selected span's
//!   logs).
//! - [`LogsModel`]: the Logs screen's live data (the rows of the list, read in
//!   pages, kept fresh by the live tail, and the histogram) and the settings that
//!   shape it (query, levels, range, tail).
//! - [`RulesModel`]: the Discard rules screen's live data (the rules with what
//!   each has dropped, read every second) and the editor session: the draft, its
//!   preview of the last five minutes and the check that the rules file did not
//!   change behind it.
//! - [`HomeControls`]: the home toolbar's Live indicator, time range, Pause and
//!   Clear all, over the [`HomeModel`]; [`LogsControls`] the Logs toolbar's
//!   Live tail pill, range and Clear all, over the [`LogsModel`].
//! - [`MainWindow`] is the root view; [`Screen`] says what it shows.
//! - [`open_main_window`] opens it, and [`show_screen`] shows a screen in a main
//!   window that is already open, from its handle alone (what the macOS popover's
//!   `Open loctel` and its rules line do).
//! - [`open_settings_window`] opens the Settings window (Milestone 4), 520 by 480
//!   and not resizable, or focuses the one that is open; what it shows is
//!   [`SettingsView`](crate::views::SettingsView).
//! - [`Backdrop`] and [`GlassRegion`]: how the shell tells a platform where its
//!   glass parts are.
//! - [`kit_theme`]: our [`Theme`](crate::Theme) onto Kit's.
//!
//! `docs/areas/ui.md` ("The main window") has the layout and the rules.

mod app_backend;
mod backend;
mod data;
pub mod kit_theme;
mod logs_data;
mod nav;
mod rules_data;
mod settings_window;
mod shell;
mod sidebar;
mod toolbar;
mod trace_data;

use std::rc::Rc;

use anyhow::Context as _;
use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Global, TitlebarOptions, WindowBounds,
    WindowHandle, WindowOptions, px, size,
};
use gpui_kit::base::Root;

pub use app_backend::{AppBackend, LoginItem};
pub use backend::UiBackend;
pub use data::{HOME_MAX_ROWS, HOME_REFRESH, HomeData, HomeModel, SLOW_TRACE, TimeRange};
pub use logs_data::{
    CONTEXT_AFTER, CONTEXT_BEFORE, ContextInfo, HISTOGRAM_BUCKETS, HISTOGRAM_REFRESH, LogsModel,
    LogsModelEvent, TAIL_REFRESH,
};
pub use nav::{NavItem, OpenSettings, SETTINGS_SHORTCUT, ShowLogs, ShowMapAndTraces, ShowRules};
pub use rules_data::{PREVIEW_DEBOUNCE, PreviewState, RULES_REFRESH, RuleEditor, RulesModel};
pub use settings_window::{SETTINGS_SIZE, open_settings_window};
pub(crate) use shell::clear_root_background;
pub use shell::{
    Backdrop, DEFAULT_SIZE, GlassRegion, INSET, MIN_SIZE, MainWindow, PANEL_RADIUS, SIDEBAR_WIDTH,
    Screen, TOOLBAR_HEIGHT, content_width,
};
pub use sidebar::{NAV_ITEM_GAP, NAV_ITEM_HEIGHT, TRAFFIC_LIGHT_SPACE};
pub use toolbar::{
    CLEAR_ALL_LABEL, HomeControls, LIVE_LABEL, LIVE_TAIL_LABEL, LogsControls, PAUSE_LABEL,
    PAUSED_LABEL, RESUME_LABEL, ToolbarPart, live_label, pause_label, toolbar_title,
};
pub use trace_data::{DONE_REFRESH, LoadState, OPEN_REFRESH, TraceModel, TraceModelEvent};

/// Marks that [`init`] has run on an app.
struct Ready;

impl Global for Ready {}

/// Gets an app ready for the main window: the bundled font, and GPUI Kit
/// (its components, key bindings and registries). It does it once per app;
/// calling it again does nothing, so every way of opening a window can call it.
pub fn init(cx: &mut App) {
    if cx.has_global::<Ready>() {
        return;
    }
    crate::register_fonts(cx);
    gpui_kit::init(cx);
    nav::bind_keys(cx);
    crate::views::bind_popover_keys(cx);
    cx.set_global(Ready);
}

/// Opens the main window on `cx`: 1440 by 900, never smaller than 1000 by 640,
/// titled `loctel`, with the shell of [`MainWindow`] in GPUI Kit's `Root`.
///
/// `attach_backdrop` runs once, inside the new window, before the shell is
/// built: a platform that can put something behind the glass parts attaches it
/// to the window and returns it (macOS: `loctel_macos::glass::attach`); `|_| None` opens a window
/// with opaque panels. Reduced motion is left at the system's value.
///
/// Fails when the platform has no display to open a window on.
pub fn open_main_window(
    backend: Rc<dyn UiBackend>,
    cx: &mut App,
    attach_backdrop: impl FnOnce(&mut gpui::Window) -> Option<Box<dyn Backdrop>>,
) -> anyhow::Result<WindowHandle<Root>> {
    init(cx);
    let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(titlebar()),
        window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
        ..WindowOptions::default()
    };
    cx.open_window(options, move |window, cx| {
        let backdrop = attach_backdrop(window);
        let main = cx.new(|cx| MainWindow::new(backend, backdrop, window, cx));
        cx.new(|cx| Root::new(main, window, cx))
    })
}

/// Shows `screen` in the main window `window`, which is a window that
/// [`open_main_window`] opened: [`MainWindow::navigate`], reached from the
/// window's handle alone. A platform shell that keeps only the handle (the macOS
/// one) uses it to open the main window on a screen of its own choice.
///
/// Fails when the window has closed, or when it is not a main window (the
/// Settings window, say).
pub fn show_screen(window: AnyWindowHandle, screen: Screen, cx: &mut App) -> anyhow::Result<()> {
    let window = window
        .downcast::<Root>()
        .context("the window is not a loctel window")?;
    window.update(cx, |root, _window, cx| {
        let main = root
            .view()
            .clone()
            .downcast::<MainWindow>()
            .map_err(|_| anyhow::anyhow!("the window does not show the main window"))?;
        main.update(cx, |main, cx| main.navigate(screen, cx));
        Ok(())
    })?
}

/// The title bar: the title everywhere; on macOS the bar is transparent and
/// the traffic lights sit in the sidebar's top strip (unverified on a Mac).
fn titlebar() -> TitlebarOptions {
    let mut titlebar = TitlebarOptions {
        title: Some("loctel".into()),
        ..TitlebarOptions::default()
    };
    if cfg!(target_os = "macos") {
        titlebar.appears_transparent = true;
        titlebar.traffic_light_position = Some(gpui::point(px(20.0), px(20.0)));
    }
    titlebar
}
