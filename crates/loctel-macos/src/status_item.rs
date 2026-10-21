//! The menu bar item: icon, menu and what the menu's commands do.
//!
//! `gpui-tray` owns the `NSStatusItem` and the `NSMenu`. Every menu entry is a
//! GPUI action, so a click arrives as an ordinary action on the app, and after
//! each click the crate rebuilds the menu, which is how "Pause receiving"
//! turns into "Resume receiving" at once. What the menu says is
//! [`crate::menu::rows`]; this file maps it to GPUI items and runs the
//! commands.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use anyhow::Context as _;
use gpui::{App, ClipboardItem, Global, KeyBinding, MenuItem, NoAction, actions};
use gpui_tray::Tray;
use loctel_engine::EngineHandle;
use loctel_ui::StatusModel;
use loctel_ui::menu_text::status_line;

use crate::first_launch::first_launch;
use crate::icon;
use crate::login_item;
use crate::menu::{Command, MenuState, Row, rows};

actions!(
    loctel_macos,
    [CopyMcpAddress, TogglePause, ToggleStartAtLogin, Quit]
);

/// How often the menu is checked against the engine. The header holds the
/// rates, which move all the time.
const REFRESH_EVERY: Duration = Duration::from_secs(2);

/// Keeps the tray alive: dropping the last handle removes the item from the
/// menu bar. Quit closes it explicitly.
struct StatusItem {
    tray: Tray,
}

impl Global for StatusItem {}

/// Puts loctel in the menu bar: the first-launch step, the icon and its menu
/// (header, Copy MCP address, Pause or Resume, Start at login, Quit), the
/// actions behind them, and a 2 second refresh of the header. It opens no
/// window.
///
/// `settings_path` is only for the first-launch step. If the status item
/// cannot be created there is nothing to see or click, so loctel says why on
/// stderr and quits instead of running unseen.
pub fn install(cx: &mut App, engine: EngineHandle, settings_path: PathBuf) {
    for problem in first_launch(&settings_path, || login_item::set_enabled(true)) {
        eprintln!("loctel: {problem}");
    }

    register_actions(cx, &engine);

    // What the menu was last built from, so the refresh can tell when the
    // menu has gone stale and leave it alone otherwise.
    let built = Rc::new(RefCell::new(None::<MenuState>));
    let tray = match build_tray(cx, &engine, &built) {
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
    cx.set_global(StatusItem { tray: tray.clone() });
    keep_header_fresh(cx, engine, tray, built);
}

/// The menu's state, read from the engine and the system.
fn menu_state(engine: &EngineHandle) -> MenuState {
    let status = StatusModel::from_status(&engine.status());
    MenuState {
        header: status_line(&status),
        paused: status.paused,
        start_at_login: login_item::is_enabled(),
        has_mcp_address: engine.mcp_url().is_some(),
    }
}

fn build_tray(
    cx: &mut App,
    engine: &EngineHandle,
    built: &Rc<RefCell<Option<MenuState>>>,
) -> anyhow::Result<Tray> {
    let icon = icon::template_icon().context("cannot load the status item icon")?;
    let engine = engine.clone();
    let built = Rc::clone(built);
    Tray::builder()
        .icon(icon)
        .tooltip("loctel")
        .menu(move |_cx| {
            let state = menu_state(&engine);
            let items = menu_items(&rows(&state));
            *built.borrow_mut() = Some(state);
            items
        })
        .build(cx)
        .context("the system refused the status item")
}

/// Rebuilds the menu every [`REFRESH_EVERY`], but only when something in it
/// changed, so an open menu is not replaced for nothing.
fn keep_header_fresh(
    cx: &mut App,
    engine: EngineHandle,
    tray: Tray,
    built: Rc<RefCell<Option<MenuState>>>,
) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(REFRESH_EVERY).await;
            let stale = built.borrow().as_ref() != Some(&menu_state(&engine));
            if stale && let Err(error) = cx.update(|cx| tray.refresh_menu(cx)) {
                eprintln!("loctel: cannot refresh the menu bar menu: {error}");
            }
        }
    })
    .detach();
}

/// The plain rows as GPUI menu items. Each choice is a GPUI action; the
/// header is a disabled item whose action does nothing.
fn menu_items(rows: &[Row]) -> Vec<MenuItem> {
    rows.iter()
        .map(|row| match row {
            Row::Header(text) => MenuItem::action(text.clone(), NoAction).disabled(true),
            Row::Separator => MenuItem::separator(),
            Row::Item {
                command,
                label,
                checked,
                enabled,
            } => {
                let item = match command {
                    Command::CopyMcpAddress => MenuItem::action(*label, CopyMcpAddress),
                    Command::TogglePause => MenuItem::action(*label, TogglePause),
                    Command::ToggleStartAtLogin => MenuItem::action(*label, ToggleStartAtLogin),
                    Command::Quit => MenuItem::action(*label, Quit),
                };
                item.checked(*checked).disabled(!*enabled)
            }
        })
        .collect()
}

fn register_actions(cx: &mut App, engine: &EngineHandle) {
    // The status item's menu cannot show or answer a shortcut (`gpui-tray`
    // builds its items with no key equivalent), so this binding only works
    // while one of loctel's own windows has the keyboard, which is from the
    // main window in Milestone 3 on.
    cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);

    cx.on_action({
        let engine = engine.clone();
        move |_: &CopyMcpAddress, cx| match engine.mcp_url() {
            Some(url) => cx.write_to_clipboard(ClipboardItem::new_string(url)),
            None => eprintln!("loctel: the MCP server is not listening, so there is no address"),
        }
    });
    cx.on_action({
        let engine = engine.clone();
        move |_: &TogglePause, _| engine.set_paused(!engine.is_paused())
    });
    cx.on_action(|_: &ToggleStartAtLogin, _| {
        if let Err(error) = login_item::set_enabled(!login_item::is_enabled()) {
            eprintln!("loctel: {error:#}");
        }
    });
    cx.on_action({
        let engine = engine.clone();
        move |_: &Quit, cx| quit(&engine, cx)
    });
}

/// Stops the engine, takes the icon out of the menu bar, then quits.
fn quit(engine: &EngineHandle, cx: &mut App) {
    engine.shutdown();
    if cx.has_global::<StatusItem>() {
        let tray = cx.global::<StatusItem>().tray.clone();
        if let Err(error) = tray.close(cx) {
            eprintln!("loctel: cannot remove the menu bar item: {error}");
        }
    }
    cx.quit();
}
