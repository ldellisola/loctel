//! The sidebar's navigation: which screens it lists and which one is lit.

use super::shell::Screen;

/// One entry of the sidebar's navigation. Logs and Discard rules arrive in
/// Milestone 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavItem {
    /// The home screen: the service map above the trace table, and from it
    /// the trace view.
    MapAndTraces,
}

impl NavItem {
    /// Every entry, in the order the sidebar lists them.
    pub const ALL: [NavItem; 1] = [NavItem::MapAndTraces];

    /// The text on the sidebar.
    pub fn label(self) -> &'static str {
        match self {
            NavItem::MapAndTraces => "Map & traces",
        }
    }

    /// The screen a click on the entry shows.
    pub fn screen(self) -> Screen {
        match self {
            NavItem::MapAndTraces => Screen::Home,
        }
    }

    /// Whether `screen` is this entry's, or one reached from it (a trace is
    /// opened from the map and the table, so it stays under `Map & traces`).
    pub fn is_active(self, screen: &Screen) -> bool {
        match self {
            NavItem::MapAndTraces => matches!(screen, Screen::Home | Screen::Trace(_)),
        }
    }
}
