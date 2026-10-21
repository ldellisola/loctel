//! The rows of the status item's menu, as plain values.
//!
//! What the menu says and in what order is decided here, with no AppKit or
//! GPUI in sight, so the Linux gate tests it. `status_item.rs` turns these rows
//! into the real menu and runs the commands.

/// What choosing a menu item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    /// Put the MCP address on the clipboard.
    CopyMcpAddress,
    /// Pause receiving, or resume it.
    TogglePause,
    /// Turn the login item on, or off.
    ToggleStartAtLogin,
    /// Stop the engine and quit.
    Quit,
}

/// One row of the menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    /// The status line: shown greyed out, it does nothing.
    Header(String),
    /// A choice.
    Item {
        command: Command,
        label: &'static str,
        /// Drawn with a checkmark.
        checked: bool,
        /// Greyed out and not choosable when false.
        enabled: bool,
    },
    Separator,
}

/// Everything the menu depends on. The status item rebuilds the menu only
/// when this changes, so equal states must mean equal menus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MenuState {
    /// `loctel_ui::menu_text::status_line(..)`.
    pub header: String,
    /// Whether receiving is paused.
    pub paused: bool,
    /// Whether the login item is on (the system's answer, not a setting).
    pub start_at_login: bool,
    /// Whether the MCP server is listening, so there is an address to copy.
    pub has_mcp_address: bool,
}

/// The menu, top to bottom: the status line, copy the MCP address, pause or
/// resume, a separator, start at login, a separator, quit.
pub(crate) fn rows(state: &MenuState) -> Vec<Row> {
    vec![
        Row::Header(state.header.clone()),
        Row::Item {
            command: Command::CopyMcpAddress,
            label: "Copy MCP address",
            checked: false,
            enabled: state.has_mcp_address,
        },
        Row::Item {
            command: Command::TogglePause,
            label: if state.paused {
                "Resume receiving"
            } else {
                "Pause receiving"
            },
            checked: false,
            enabled: true,
        },
        Row::Separator,
        Row::Item {
            command: Command::ToggleStartAtLogin,
            label: "Start at login",
            checked: state.start_at_login,
            enabled: true,
        },
        Row::Separator,
        Row::Item {
            command: Command::Quit,
            label: "Quit loctel",
            checked: false,
            enabled: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> MenuState {
        MenuState {
            header: "Receiving \u{b7} 412 spans/s \u{b7} 1.2k logs/s".to_owned(),
            paused: false,
            start_at_login: true,
            has_mcp_address: true,
        }
    }

    fn item(command: Command, label: &'static str, checked: bool, enabled: bool) -> Row {
        Row::Item {
            command,
            label,
            checked,
            enabled,
        }
    }

    #[test]
    fn rows_are_the_designed_menu_in_order() {
        assert_eq!(
            rows(&state()),
            vec![
                Row::Header("Receiving \u{b7} 412 spans/s \u{b7} 1.2k logs/s".to_owned()),
                item(Command::CopyMcpAddress, "Copy MCP address", false, true),
                item(Command::TogglePause, "Pause receiving", false, true),
                Row::Separator,
                item(Command::ToggleStartAtLogin, "Start at login", true, true),
                Row::Separator,
                item(Command::Quit, "Quit loctel", false, true),
            ]
        );
    }

    #[test]
    fn the_header_is_whatever_the_status_line_says() {
        let mut state = state();
        state.header = "Port problem: otlp-http (address in use)".to_owned();
        assert_eq!(
            rows(&state)[0],
            Row::Header("Port problem: otlp-http (address in use)".to_owned())
        );
    }

    #[test]
    fn pause_reads_resume_while_paused() {
        let mut state = state();
        state.paused = true;
        assert!(rows(&state).contains(&item(
            Command::TogglePause,
            "Resume receiving",
            false,
            true
        )));
        assert!(!rows(&state).iter().any(|row| matches!(
            row,
            Row::Item {
                label: "Pause receiving",
                ..
            }
        )));
    }

    #[test]
    fn start_at_login_has_a_checkmark_only_while_enabled() {
        let mut state = state();
        state.start_at_login = false;
        assert!(rows(&state).contains(&item(
            Command::ToggleStartAtLogin,
            "Start at login",
            false,
            true
        )));
    }

    #[test]
    fn copy_is_disabled_when_the_mcp_server_has_no_address() {
        let mut state = state();
        state.has_mcp_address = false;
        assert!(rows(&state).contains(&item(
            Command::CopyMcpAddress,
            "Copy MCP address",
            false,
            false
        )));
    }

    #[test]
    fn only_the_header_is_not_an_action() {
        let rows = rows(&state());
        let headers = rows
            .iter()
            .filter(|row| matches!(row, Row::Header(_)))
            .count();
        assert_eq!(headers, 1);
        assert!(matches!(rows.first(), Some(Row::Header(_))));
        assert!(matches!(
            rows.last(),
            Some(Row::Item {
                command: Command::Quit,
                ..
            })
        ));
    }
}
