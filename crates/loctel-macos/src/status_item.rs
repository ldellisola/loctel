//! The menu bar item: the icon, and what a click on it does.
//!
//! `gpui-tray` owns the `NSStatusItem`. Since Milestone 4 it has **no menu**: a
//! click on the icon arrives as the tray's activation callback (`gpui-tray` calls
//! it only while no menu is attached) and opens or closes the popover
//! ([`crate::popover::toggle`]), which has what the menu had and more: the status,
//! the rates, the endpoints, the rules line and the actions. What the popover shows
//! and does is `loctel_ui::views::PopoverView`; where it opens and how it closes is
//! [`crate::popover`].
//!
//! What stays here is what is about the app and not the popover: the first-launch
//! step, the icon, ⌘Q and ⌘O for the main window, and the one way out
//! ([`quit`]).

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use gpui::{App, Global, KeyBinding, actions};
use gpui_tray::Tray;
use loctel_engine::EngineHandle;

use crate::first_launch::first_launch;
use crate::icon;
use crate::login_item;
use crate::popover;
use crate::window;

actions!(loctel_macos, [OpenWindow, Quit]);

/// Keeps the tray alive: dropping the last handle removes the item from the
/// menu bar. Quit closes it explicitly.
struct StatusItem {
    tray: Tray,
}

impl Global for StatusItem {}

/// Puts loctel in the menu bar: the first-launch step, the icon, the shortcuts of
/// the main window and the popover that a click on the icon opens. It opens no
/// window: a click does.
///
/// `settings_path` is for the first-launch step, and for the windows the popover
/// opens (the main window and Settings), whose `AppBackend` reads and writes the
/// settings there. If the status item cannot be created there is nothing to see or
/// click, so loctel says why on stderr and quits instead of running unseen.
pub fn install(cx: &mut App, engine: EngineHandle, settings_path: PathBuf) {
    for problem in first_launch(&settings_path, || login_item::set_enabled(true)) {
        eprintln!("loctel: {problem}");
    }

    register_actions(cx, &engine, &settings_path);
    popover::install(cx, engine.clone(), settings_path);

    let tray = match build_tray(cx) {
        Ok(tray) => tray,
        Err(error) => {
            eprintln!(
                "loctel: cannot add the menu bar item: {error:#} (`--headless` runs without it)"
            );
            engine.shutdown();
            cx.quit();
            return;
        }
    };
    cx.set_global(StatusItem { tray });
}

/// The status item: the template icon, a tooltip, and no menu, so that a primary
/// click is the activation callback, which toggles the popover.
fn build_tray(cx: &mut App) -> anyhow::Result<Tray> {
    let icon = icon::template_icon().context("cannot load the status item icon")?;
    Tray::builder()
        .icon(icon)
        .tooltip("loctel")
        .on_activate_with(|_, cx| popover::toggle(cx))
        .build(cx)
        .context("the system refused the status item")
}

fn register_actions(cx: &mut App, engine: &EngineHandle, settings_path: &Path) {
    // ⌘Q and ⌘O in loctel's own windows (the main window, Settings): with no
    // menu, nothing else answers them. The popover has the same keys in a key
    // context of its own (`LoctelPopover`, bound by `loctel_ui::app::init`), which
    // is the more specific one while the popover has the keyboard.
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-o", OpenWindow, None),
    ]);

    cx.on_action({
        let engine = engine.clone();
        let settings_path = settings_path.to_owned();
        move |_: &OpenWindow, cx| {
            window::open_or_focus(cx, engine.clone(), settings_path.clone());
        }
    });
    cx.on_action({
        let engine = engine.clone();
        move |_: &Quit, cx| quit(&engine, cx)
    });
}

/// Stops the engine, takes the icon out of the menu bar, then quits.
pub(crate) fn quit(engine: &EngineHandle, cx: &mut App) {
    engine.shutdown();
    if cx.has_global::<StatusItem>() {
        let tray = cx.global::<StatusItem>().tray.clone();
        if let Err(error) = tray.close(cx) {
            eprintln!("loctel: cannot remove the menu bar item: {error}");
        }
    }
    cx.quit();
}
