//! The main window's and the Settings window's lifecycle on macOS: opening them
//! from the popover, showing loctel in the Dock and Cmd-Tab for as long as one is
//! open, and going back to a menu bar only app when the last one closes.
//!
//! loctel starts as an accessory app (no Dock icon, no window; `loctel-app`
//! sets that and `LSUIElement` in the bundle backs it up). [`open_or_focus`] is
//! the one way into the main window: the popover's `Open loctel` calls it (through
//! [`show_screen`], which then shows a screen), and so does GPUI's reopen event, a
//! click on the Dock icon while no window is open. [`open_settings`] is the way
//! into Settings from the popover; the main window's own ⌘, opens it itself. While
//! either window is open the app is a regular one (the Dock icon, ⌘-Tab); closing
//! the last of them does not end the app (the quit mode is explicit: only `Quit
//! loctel` does), it only takes the Dock icon away again. The popover's own window
//! never counts: it opens and closes with the app staying an accessory.
//!
//! The main window's sidebar and toolbar get Liquid Glass from [`glass::attach`],
//! or GPUI's blurred background when that is not possible; the popover's panel
//! gets it the same way ([`attach_glass`]).
//!
//! Nothing here has run on a Mac. See "Unverified until the M3 Mac check" and
//! "Unverified until the M4 Mac check" in `docs/areas/macos.md`.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{ActivationPolicy, AnyWindowHandle, App, Global, Window, WindowBackgroundAppearance};
use loctel_engine::EngineHandle;
use loctel_ui::app::{AppBackend, Backdrop, Screen};

use crate::login_item::SystemLoginItem;
use crate::{glass, popover};

/// Whether the fallback to the blurred background has been logged.
static FALLBACK_LOGGED: AtomicBool = AtomicBool::new(false);

/// What the app knows about its main window. It is a GPUI global so that the
/// menu's action and the Dock's reopen event, which share nothing else, see the
/// same window.
#[derive(Default)]
struct MainWindowState {
    /// The main window, from when it is opened until it is closed.
    window: Option<AnyWindowHandle>,
    /// Whether the close observer is registered. It is registered once, the
    /// first time a window is opened, and lives as long as the app.
    watching_close: bool,
}

impl Global for MainWindowState {}

/// The backend every loctel window of this app is built over: the engine, the
/// settings in `settings_path`, and the system's login item.
pub(crate) fn backend(engine: &EngineHandle, settings_path: &Path) -> Rc<AppBackend> {
    Rc::new(AppBackend::new(
        engine.clone(),
        settings_path.to_owned(),
        Some(Rc::new(SystemLoginItem)),
    ))
}

/// Shows the main window: brings the open one to the front, or opens it.
///
/// Opening it is, in this order: the Dock icon on (`ActivationPolicy::Regular`),
/// the window (`loctel_ui::app::open_main_window`, with the glass attached to it
/// by `attach_glass`, over an [`AppBackend`] that reads and writes the settings
/// in `settings_path` and has the system's login item) and then the app
/// activated, as GPUI asks for after a switch to `Regular`. If the window cannot
/// be opened the reason goes to stderr and the policy is set again from the
/// windows that are open, so a failed open leaves no Dock icon with nothing
/// behind it.
pub fn open_or_focus(cx: &mut App, engine: EngineHandle, settings_path: PathBuf) {
    let open = cx.default_global::<MainWindowState>().window;
    if let Some(window) = open
        && cx.windows().contains(&window)
    {
        // `update` fails only for a window that is gone, which the check above
        // has just ruled out; either way the app is activated below.
        let _ = window.update(cx, |_, window, _| window.activate_window());
        cx.activate(true);
        return;
    }

    watch_close(cx);
    cx.set_activation_policy(ActivationPolicy::Regular);
    let backend = backend(&engine, &settings_path);
    match loctel_ui::app::open_main_window(backend, cx, attach_glass) {
        Ok(window) => {
            cx.default_global::<MainWindowState>().window = Some(window.into());
            cx.activate(true);
        }
        Err(error) => {
            eprintln!("loctel: cannot open the main window: {error:#}");
            sync_activation_policy(cx);
        }
    }
}

/// Shows `screen` in the main window, opening the window first if it is not open
/// ([`open_or_focus`]): what the popover's `Open loctel` (the home screen) and its
/// rules line (Discard rules) do. If the window could not be opened, that has been
/// said on stderr, and there is nothing to show.
pub(crate) fn show_screen(
    cx: &mut App,
    engine: EngineHandle,
    settings_path: PathBuf,
    screen: Screen,
) {
    open_or_focus(cx, engine, settings_path);
    let Some(window) = cx.default_global::<MainWindowState>().window else {
        return;
    };
    if let Err(error) = loctel_ui::app::show_screen(window, screen, cx) {
        eprintln!("loctel: cannot show that screen in the main window: {error:#}");
    }
}

/// Shows the Settings window: opens it, or brings the open one to the front
/// (`loctel_ui::app::open_settings_window`, which takes care of both). The Dock
/// icon is on while it is open (`ActivationPolicy::Regular`), and goes when the
/// last loctel window closes. If it cannot be opened the reason goes to stderr
/// and the policy is set again from the windows that are open.
pub(crate) fn open_settings(cx: &mut App, engine: EngineHandle, settings_path: PathBuf) {
    watch_close(cx);
    cx.set_activation_policy(ActivationPolicy::Regular);
    let backend = backend(&engine, &settings_path);
    if let Err(error) = loctel_ui::app::open_settings_window(backend, cx) {
        eprintln!("loctel: cannot open the Settings window: {error:#}");
        sync_activation_policy(cx);
    }
}

/// `Regular` while a loctel window other than the popover is open, `Accessory`
/// when none is. The popover is left out: it is a pop-up of an app that lives in
/// the menu bar, and must not put loctel in the Dock for as long as it is open.
fn sync_activation_policy(cx: &mut App) {
    let popover = popover::open_window_handle(cx);
    let open = cx
        .windows()
        .into_iter()
        .filter(|window| Some(*window) != popover)
        .count();
    cx.set_activation_policy(if open > 0 {
        ActivationPolicy::Regular
    } else {
        ActivationPolicy::Accessory
    });
}

/// Puts Liquid Glass behind the new window's sidebar and toolbar
/// ([`glass::attach`]). Without it (no `NSGlassEffectView`, or attaching failed)
/// the window gets GPUI's `Blurred` background instead, which is the fallback,
/// and the shell paints its opaque panels; that is logged once per run.
pub(crate) fn attach_glass(window: &mut Window) -> Option<Box<dyn Backdrop>> {
    let backdrop = glass::attach(window);
    if backdrop.is_none() {
        window.set_background_appearance(WindowBackgroundAppearance::Blurred);
        if !FALLBACK_LOGGED.swap(true, Ordering::Relaxed) {
            eprintln!(
                "loctel: Liquid Glass is unavailable (it needs macOS 26 and an AppKit window), \
                 so the window uses the blurred background instead"
            );
        }
    }
    backdrop
}

/// Puts the app back to `Accessory` when the last loctel window closes (the Dock
/// icon and the Cmd-Tab entry go), and forgets the main window when it is the one
/// that closed so the next [`open_or_focus`] opens a new one. Registered once.
fn watch_close(cx: &mut App) {
    let state = cx.default_global::<MainWindowState>();
    if std::mem::replace(&mut state.watching_close, true) {
        return;
    }
    // Called for every window that closes, whichever it is: the main window is
    // forgotten, and the policy follows the windows that are left.
    cx.on_window_closed(|cx, closed| {
        let state = cx.default_global::<MainWindowState>();
        if state
            .window
            .is_some_and(|window| window.window_id() == closed)
        {
            state.window = None;
        }
        sync_activation_policy(cx);
    })
    .detach();
}
