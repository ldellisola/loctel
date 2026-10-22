//! The sidebar's navigation: which screens it lists, which one is lit, and the
//! keys that go to them.
//!
//! Three entries (M4 plan, Task 7), each with an icon from GPUI Kit's Lucide
//! catalog and a shortcut: `Map & traces` (⌘1), `Logs` (⌘2) and `Discard rules`
//! (⌘3). ⌘, opens the Settings window. The shortcuts are GPUI actions bound by
//! [`init`](super::init) in the key context [`CONTEXT`], which the main window's
//! root element carries, so they work wherever in that window the keyboard is
//! and in no other window.

use gpui::{App, KeyBinding, actions};
use gpui_kit::assets::IconName;

use super::shell::Screen;

actions!(
    loctel_nav,
    [
        /// ⌘1: show the home screen (`Map & traces`).
        ShowMapAndTraces,
        /// ⌘2: show the Logs screen.
        ShowLogs,
        /// ⌘3: show the Discard rules screen.
        ShowRules,
        /// ⌘,: open the Settings window (or bring it forward).
        OpenSettings,
    ]
);

/// The key context of the main window's root element: where the bindings of
/// [`bind_keys`] apply.
pub(super) const CONTEXT: &str = "MainWindow";

/// The keystroke of [`OpenSettings`], in GPUI's syntax (`cmd` is ⌘ on a Mac and
/// the Super key elsewhere).
pub const SETTINGS_SHORTCUT: &str = "cmd-,";

/// One entry of the sidebar's navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NavItem {
    /// The home screen: the service map above the trace table, and from it
    /// the trace view.
    MapAndTraces,
    /// The log list.
    Logs,
    /// The discard rules.
    Rules,
}

impl NavItem {
    /// Every entry, in the order the sidebar lists them.
    pub const ALL: [NavItem; 3] = [NavItem::MapAndTraces, NavItem::Logs, NavItem::Rules];

    /// The text on the sidebar.
    pub fn label(self) -> &'static str {
        match self {
            NavItem::MapAndTraces => "Map & traces",
            NavItem::Logs => "Logs",
            NavItem::Rules => "Discard rules",
        }
    }

    /// The icon in front of the text: a map, a list and a funnel.
    pub fn icon(self) -> IconName {
        match self {
            NavItem::MapAndTraces => IconName::Map,
            NavItem::Logs => IconName::List,
            NavItem::Rules => IconName::Funnel,
        }
    }

    /// The keystroke that shows the entry's screen, in GPUI's syntax: `cmd-1`,
    /// `cmd-2` and `cmd-3`.
    pub fn shortcut(self) -> &'static str {
        match self {
            NavItem::MapAndTraces => "cmd-1",
            NavItem::Logs => "cmd-2",
            NavItem::Rules => "cmd-3",
        }
    }

    /// The screen a click on the entry shows. Logs and Discard rules come
    /// without a query or a draft, so each is shown as it was left.
    pub fn screen(self) -> Screen {
        match self {
            NavItem::MapAndTraces => Screen::Home,
            NavItem::Logs => Screen::Logs { query: None },
            NavItem::Rules => Screen::Rules { draft: None },
        }
    }

    /// Whether `screen` is this entry's, or one reached from it (a trace is
    /// opened from the map and the table, so it stays under `Map & traces`, even
    /// when it was opened from a log).
    pub fn is_active(self, screen: &Screen) -> bool {
        match self {
            NavItem::MapAndTraces => matches!(screen, Screen::Home | Screen::Trace(_)),
            NavItem::Logs => matches!(screen, Screen::Logs { .. }),
            NavItem::Rules => matches!(screen, Screen::Rules { .. }),
        }
    }
}

/// Binds the shortcuts of the main window: ⌘1, ⌘2 and ⌘3 to the three screens
/// and ⌘, to Settings, in the key context [`CONTEXT`].
pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(
            NavItem::MapAndTraces.shortcut(),
            ShowMapAndTraces,
            Some(CONTEXT),
        ),
        KeyBinding::new(NavItem::Logs.shortcut(), ShowLogs, Some(CONTEXT)),
        KeyBinding::new(NavItem::Rules.shortcut(), ShowRules, Some(CONTEXT)),
        KeyBinding::new(SETTINGS_SHORTCUT, OpenSettings, Some(CONTEXT)),
    ]);
}
