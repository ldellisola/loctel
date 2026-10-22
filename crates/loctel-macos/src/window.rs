//! The main window's lifecycle on macOS: opening it from the menu bar, showing
//! loctel in the Dock and Cmd-Tab for as long as it is open, and going back to
//! a menu bar only app when it closes.
//!
//! loctel starts as an accessory app (no Dock icon, no window; `loctel-app`
//! sets that and `LSUIElement` in the bundle backs it up). [`open_or_focus`] is
//! the one way in: the menu's `Open loctel` calls it, and so does GPUI's reopen
//! event, a click on the Dock icon while no window is open. Closing the main
//! window does not end the app (the quit mode is explicit: only `Quit loctel`
//! does); it only takes the Dock icon away again.
//!
//! The window's sidebar and toolbar get Liquid Glass from [`glass::attach`], or
//! GPUI's blurred background when that is not possible.
//!
//! Nothing here has run on a Mac. See "Unverified until the M3 Mac check" in
//! `docs/areas/macos.md`.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{ActivationPolicy, AnyWindowHandle, App, Global, Window, WindowBackgroundAppearance};
use loctel_engine::EngineHandle;
use loctel_ui::app::Backdrop;

use crate::glass;

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

/// Shows the main window: brings the open one to the front, or opens it.
///
/// Opening it is, in this order: the Dock icon on (`ActivationPolicy::Regular`),
/// the window (`loctel_ui::app::open_main_window`, with the glass attached to it
/// by `attach_glass`) and then the app activated,
/// as GPUI asks for after a switch to `Regular`. If the window cannot be opened
/// the reason goes to stderr and the app goes back to `Accessory`, so a failed
/// open leaves no Dock icon with nothing behind it.
pub fn open_or_focus(cx: &mut App, engine: EngineHandle) {
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
    match loctel_ui::app::open_main_window(Rc::new(engine), cx, attach_glass) {
        Ok(window) => {
            cx.default_global::<MainWindowState>().window = Some(window.into());
            cx.activate(true);
        }
        Err(error) => {
            eprintln!("loctel: cannot open the main window: {error:#}");
            cx.set_activation_policy(ActivationPolicy::Accessory);
        }
    }
}

/// Puts Liquid Glass behind the new window's sidebar and toolbar
/// ([`glass::attach`]). Without it (no `NSGlassEffectView`, or attaching failed)
/// the window gets GPUI's `Blurred` background instead, which is the fallback,
/// and the shell paints its opaque panels; that is logged once per run.
fn attach_glass(window: &mut Window) -> Option<Box<dyn Backdrop>> {
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

/// Puts the app back to `Accessory` when the main window closes (the Dock icon
/// and the Cmd-Tab entry go), and forgets the window so the next
/// [`open_or_focus`] opens a new one. Registered once.
fn watch_close(cx: &mut App) {
    let state = cx.default_global::<MainWindowState>();
    if std::mem::replace(&mut state.watching_close, true) {
        return;
    }
    // Called for every window that closes, whichever it is; only the main
    // window matters.
    cx.on_window_closed(|cx, closed| {
        let state = cx.default_global::<MainWindowState>();
        if state
            .window
            .is_some_and(|window| window.window_id() == closed)
        {
            state.window = None;
            cx.set_activation_policy(ActivationPolicy::Accessory);
        }
    })
    .detach();
}
