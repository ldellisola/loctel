//! The rows of the status item's menu, as plain values.
//!
//! What the menu says and in what order is decided here, with no AppKit or
//! GPUI in sight, so the Linux gate tests it. `status_item.rs` turns these rows
//! into the real menu and runs the commands.

/// What choosing a menu item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    /// Show the main window: open it, or bring the open one to the front.
    OpenWindow,
    /// Put the MCP address on the clipboard.
    CopyMcpAddress,
    /// Pause receiving, or resume it.
    TogglePause,
    /// Open `rules.toml` in the default text editor.
    EditRules,
    /// Turn the login item on, or off.
    ToggleStartAtLogin,
    /// Stop the engine and quit.
    Quit,
}

/// One row of the menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    /// A line of text: shown greyed out, it does nothing. The menu has two,
    /// the status line and the discard rules line under it.
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
    /// `loctel_ui::menu_text::rules_line(..)`.
    pub rules: String,
    /// Whether receiving is paused.
    pub paused: bool,
    /// Whether the login item is on (the system's answer, not a setting).
    pub start_at_login: bool,
    /// Whether the MCP server is listening, so there is an address to copy.
    pub has_mcp_address: bool,
    /// Whether the engine has a rules file (`rules_path()` is not `None`), so
    /// there is something to open.
    pub has_rules_file: bool,
}

/// The menu, top to bottom: the status line, the discard rules line, open
/// loctel, a separator, copy the MCP address, pause or resume, edit the discard
/// rules, a separator, start at login, a separator, quit.
pub(crate) fn rows(state: &MenuState) -> Vec<Row> {
    vec![
        Row::Header(state.header.clone()),
        Row::Header(state.rules.clone()),
        Row::Item {
            command: Command::OpenWindow,
            label: "Open loctel",
            checked: false,
            enabled: true,
        },
        Row::Separator,
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
        Row::Item {
            command: Command::EditRules,
            label: "Edit discard rules\u{2026}",
            checked: false,
            enabled: state.has_rules_file,
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
            rules: "1 discard rule on \u{b7} 31% dropped".to_owned(),
            paused: false,
            start_at_login: true,
            has_mcp_address: true,
            has_rules_file: true,
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
                Row::Header("1 discard rule on \u{b7} 31% dropped".to_owned()),
                item(Command::OpenWindow, "Open loctel", false, true),
                Row::Separator,
                item(Command::CopyMcpAddress, "Copy MCP address", false, true),
                item(Command::TogglePause, "Pause receiving", false, true),
                item(
                    Command::EditRules,
                    "Edit discard rules\u{2026}",
                    false,
                    true
                ),
                Row::Separator,
                item(Command::ToggleStartAtLogin, "Start at login", true, true),
                Row::Separator,
                item(Command::Quit, "Quit loctel", false, true),
            ]
        );
    }

    #[test]
    fn rows_include_rules_header_and_edit_item() {
        let rows = rows(&state());

        // The exact order: status header, rules header, Open loctel,
        // separator, Copy MCP address, Pause, Edit discard rules, separator,
        // Start at login, separator, Quit.
        let order: Vec<&str> = rows
            .iter()
            .map(|row| match row {
                Row::Header(text) => text.as_str(),
                Row::Item { label, .. } => label,
                Row::Separator => "---",
            })
            .collect();
        assert_eq!(
            order,
            [
                "Receiving \u{b7} 412 spans/s \u{b7} 1.2k logs/s",
                "1 discard rule on \u{b7} 31% dropped",
                "Open loctel",
                "---",
                "Copy MCP address",
                "Pause receiving",
                "Edit discard rules\u{2026}",
                "---",
                "Start at login",
                "---",
                "Quit loctel",
            ]
        );

        // The rules line is a header (greyed out, does nothing), right under
        // the status line.
        assert!(matches!(&rows[1], Row::Header(_)));
        // The edit item is a plain action, with no checkmark.
        assert_eq!(
            rows[6],
            item(
                Command::EditRules,
                "Edit discard rules\u{2026}",
                false,
                true
            )
        );
    }

    #[test]
    fn rows_have_open_first_after_headers() {
        let rows = rows(&state());

        // The two header rows, then `Open loctel`, then a separator.
        assert!(matches!(&rows[0], Row::Header(_)));
        assert!(matches!(&rows[1], Row::Header(_)));
        assert_eq!(
            rows[2],
            item(Command::OpenWindow, "Open loctel", false, true)
        );
        assert_eq!(rows[3], Row::Separator);

        // It is the only way to open the window, so it is there exactly once,
        // and always choosable: the window needs nothing from the engine that
        // can be missing (the Copy and Edit items depend on an address and a
        // file; this one does not).
        let opens = rows
            .iter()
            .filter(|row| {
                matches!(
                    row,
                    Row::Item {
                        command: Command::OpenWindow,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(opens, 1);

        let mut bare = state();
        bare.has_mcp_address = false;
        bare.has_rules_file = false;
        bare.paused = true;
        assert_eq!(
            super::rows(&bare)[2],
            item(Command::OpenWindow, "Open loctel", false, true)
        );
    }

    #[test]
    fn the_rules_header_is_whatever_the_rules_line_says() {
        let mut state = state();
        state.rules = "Discard rules file has an error".to_owned();
        assert_eq!(
            rows(&state)[1],
            Row::Header("Discard rules file has an error".to_owned())
        );
    }

    #[test]
    fn edit_rules_is_disabled_when_the_engine_has_no_rules_file() {
        // `rules_path()` is `None` when the rules live in memory only: there
        // is nothing to open.
        let mut state = state();
        state.has_rules_file = false;
        assert!(rows(&state).contains(&item(
            Command::EditRules,
            "Edit discard rules\u{2026}",
            false,
            false
        )));
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
    fn only_the_two_headers_are_not_actions() {
        let rows = rows(&state());
        let headers = rows
            .iter()
            .filter(|row| matches!(row, Row::Header(_)))
            .count();
        assert_eq!(headers, 2);
        assert!(matches!(rows.first(), Some(Row::Header(_))));
        assert!(matches!(rows.get(1), Some(Row::Header(_))));
        assert!(matches!(
            rows.last(),
            Some(Row::Item {
                command: Command::Quit,
                ..
            })
        ));
    }
}
