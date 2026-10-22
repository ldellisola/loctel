//! The macOS shell: the menu bar status item and its popover, the windows'
//! lifecycle, and start at login.
//!
//! The crate is empty on every other OS (and its dependencies are macOS-only),
//! except for the parts that hold decisions and are plain Rust: the popover's
//! anchoring, the first-launch step, the icon files and the glass frames' y flip.
//! Those are compiled for the tests on any OS, so the Linux gate runs them; the
//! code that draws and drives AppKit is only type-checked off a Mac
//! (`script/check-macos.sh`).
//!
//! - `install(cx, engine, settings_path)` puts loctel in the menu bar. It is
//!   all `loctel-app` calls at launch. The status item has no menu: a click on
//!   it opens the popover (`popover`), the panel under the icon that shows the
//!   status, the rates, the endpoints and the rules line, and has the actions
//!   (Open loctel, Pause receiving, Clear all data, Start at login, Settings,
//!   Quit). Where it opens is worked out from the mouse (`popover::anchor`),
//!   because `gpui-tray` does not say where its icon is.
//! - `open_or_focus(cx, engine, settings_path)` shows the main window, and the
//!   Dock icon with it, over an `AppBackend` that has the system's login item
//!   (`login_item::SystemLoginItem`): the popover's `Open loctel` runs it, and
//!   `loctel-app` runs it on GPUI's reopen event (a Dock click with no window
//!   open). The Settings window (the popover's `Settings...`, or the main
//!   window's own command-comma) keeps the Dock icon too. Closing the last of the
//!   windows takes the Dock icon away again; the app keeps running.
//! - `glass::attach(window)` puts Liquid Glass (`NSGlassEffectView`) behind the
//!   main window's sidebar and toolbar, and behind the popover's panel;
//!   `open_or_focus` and the popover pass it to the window, and fall back to
//!   GPUI's blurred background without it.
//! - `login_item` turns start at login on and off (`SMAppService`);
//!   `login_item::SystemLoginItem` is it as the windows' `LoginItem`.
//!
//! All `objc2` and `unsafe` code in the project lives in this crate. See
//! `docs/areas/macos.md`, and read its "Unverified until the M1 Mac check",
//! "Unverified until the M2 Mac check", "Unverified until the M3 Mac check" and
//! "Unverified until the M4 Mac check" lists before treating anything here as
//! proven.

#[cfg(any(target_os = "macos", test))]
mod first_launch;
#[cfg(any(target_os = "macos", test))]
mod glass;
#[cfg(any(target_os = "macos", test))]
mod icon;
#[cfg(any(target_os = "macos", test))]
mod popover;

#[cfg(target_os = "macos")]
pub mod login_item;
#[cfg(target_os = "macos")]
mod status_item;
#[cfg(target_os = "macos")]
pub mod window;

#[cfg(target_os = "macos")]
pub use status_item::install;
#[cfg(target_os = "macos")]
pub use window::open_or_focus;
