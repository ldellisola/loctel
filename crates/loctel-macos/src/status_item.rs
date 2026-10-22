//! The menu bar item: icon, menu and what the menu's commands do.
//!
//! `gpui-tray` owns the `NSStatusItem` and the `NSMenu`. Every menu entry is a
//! GPUI action, so a click arrives as an ordinary action on the app, and after
//! each click the crate rebuilds the menu, which is how "Pause receiving"
//! turns into "Resume receiving" at once. What the menu says is
//! [`crate::menu::rows`]; this file maps it to GPUI items and runs the
//! commands (`Open loctel` is [`crate::window::open_or_focus`]).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::rc::Rc;
use std::time::Duration;

use anyhow::Context as _;
use gpui::{App, ClipboardItem, Global, KeyBinding, MenuItem, NoAction, actions};
use gpui_tray::Tray;
use loctel_engine::EngineHandle;
use loctel_ui::StatusModel;
use loctel_ui::menu_text::{rules_line, status_line};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

use crate::first_launch::first_launch;
use crate::icon;
use crate::login_item;
use crate::menu::{Command, MenuState, Row, rows};
use crate::window;

actions!(
    loctel_macos,
    [
        OpenWindow,
        CopyMcpAddress,
        TogglePause,
        EditRules,
        ToggleStartAtLogin,
        Quit
    ]
);

/// How often the menu is checked against the engine. The header holds the
/// rates, and the rules line the share dropped; both move all the time.
const REFRESH_EVERY: Duration = Duration::from_secs(2);

/// Keeps the tray alive: dropping the last handle removes the item from the
/// menu bar. Quit closes it explicitly.
struct StatusItem {
    tray: Tray,
}

impl Global for StatusItem {}

/// Puts loctel in the menu bar: the first-launch step, the icon and its menu
/// (status line, discard rules line, Open loctel, Copy MCP address, Pause or
/// Resume, Edit discard rules, Start at login, Quit), the actions behind them,
/// and a 2 second refresh of the two lines. It opens no window: `Open loctel`
/// does.
///
/// `settings_path` is for the first-launch step, and for the main window's
/// `AppBackend` (`Open loctel`), which reads and writes the settings there. If
/// the status item cannot be created there is nothing to see or click, so loctel says why on
/// stderr and quits instead of running unseen.
pub fn install(cx: &mut App, engine: EngineHandle, settings_path: PathBuf) {
    for problem in first_launch(&settings_path, || login_item::set_enabled(true)) {
        eprintln!("loctel: {problem}");
    }

    register_actions(cx, &engine, &settings_path);

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
        rules: rules_line(&status),
        paused: status.paused,
        start_at_login: login_item::is_enabled(),
        has_mcp_address: engine.mcp_url().is_some(),
        has_rules_file: engine.rules_path().is_some(),
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
                    Command::OpenWindow => MenuItem::action(*label, OpenWindow),
                    Command::CopyMcpAddress => MenuItem::action(*label, CopyMcpAddress),
                    Command::TogglePause => MenuItem::action(*label, TogglePause),
                    Command::EditRules => MenuItem::action(*label, EditRules),
                    Command::ToggleStartAtLogin => MenuItem::action(*label, ToggleStartAtLogin),
                    Command::Quit => MenuItem::action(*label, Quit),
                };
                item.checked(*checked).disabled(!*enabled)
            }
        })
        .collect()
}

fn register_actions(cx: &mut App, engine: &EngineHandle, settings_path: &Path) {
    // The status item's menu cannot show or answer a shortcut (`gpui-tray`
    // builds its items with no key equivalent), so these bindings only work
    // while one of loctel's own windows has the keyboard, which is the main
    // window. Cmd-O there brings it to the front.
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
        move |_: &CopyMcpAddress, cx| match engine.mcp_url() {
            Some(url) => cx.write_to_clipboard(ClipboardItem::new_string(url)),
            None => eprintln!("loctel: the MCP server is not listening, so there is no address"),
        }
    });
    cx.on_action({
        let engine = engine.clone();
        move |_: &TogglePause, _| engine.set_paused(!engine.is_paused())
    });
    cx.on_action({
        let engine = engine.clone();
        move |_: &EditRules, _| match engine.rules_path() {
            Some(path) => {
                if let Err(error) = open_in_text_editor(&path) {
                    eprintln!("loctel: cannot open {}: {error:#}", path.display());
                }
            }
            None => {
                eprintln!("loctel: the discard rules are not in a file, so there is none to edit")
            }
        }
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

/// Opens `path` in the default text editor. First the way Finder opens a file
/// (`NSWorkspace` `openURL` with the file URL, which uses whatever app the
/// system has for `.toml`); if no app handles that type, or the path is not
/// text, `open -t`, which asks for the default text editor outright. The file
/// is there because the engine writes it at start, so a failure here means it
/// was deleted since, or the system has no text editor to offer.
fn open_in_text_editor(path: &Path) -> anyhow::Result<()> {
    if let Some(text) = path.to_str() {
        let url = NSURL::fileURLWithPath(&NSString::from_str(text));
        if NSWorkspace::sharedWorkspace().openURL(&url) {
            return Ok(());
        }
    }
    let status = Process::new("open")
        .arg("-t")
        .arg(path)
        .status()
        .context("cannot run `open -t`")?;
    anyhow::ensure!(status.success(), "`open -t` failed ({status})");
    Ok(())
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
