//! The macOS shell: the menu bar status item, the main window's lifecycle, and
//! start at login.
//!
//! The crate is empty on every other OS (and its dependencies are macOS-only),
//! except for the parts that hold decisions and are plain Rust: the menu's
//! rows, the first-launch step, the icon files and the glass frames' y flip.
//! Those are compiled for the tests on any OS, so the Linux gate runs them; the
//! code that draws and drives AppKit is only type-checked off a Mac
//! (`script/check-macos.sh`).
//!
//! - `install(cx, engine, settings_path)` puts loctel in the menu bar. It is
//!   all `loctel-app` calls at launch.
//! - `open_or_focus(cx, engine)` shows the main window, and the Dock icon with
//!   it: the menu's `Open loctel` runs it, and `loctel-app` runs it on GPUI's
//!   reopen event (a Dock click with no window open). Closing the window takes
//!   the Dock icon away again; the app keeps running.
//! - `glass::attach(window)` puts Liquid Glass (`NSGlassEffectView`) behind the
//!   main window's sidebar and toolbar; `open_or_focus` passes it to the
//!   window, and falls back to GPUI's blurred background without it.
//! - `login_item` turns start at login on and off (`SMAppService`).
//!
//! All `objc2` and `unsafe` code in the project lives in this crate. See
//! `docs/areas/macos.md`, and read its "Unverified until the M1 Mac check",
//! "Unverified until the M2 Mac check" and "Unverified until the M3 Mac check"
//! lists before treating anything here as proven.

#[cfg(any(target_os = "macos", test))]
mod first_launch;
#[cfg(any(target_os = "macos", test))]
mod glass;
#[cfg(any(target_os = "macos", test))]
mod icon;
#[cfg(any(target_os = "macos", test))]
mod menu;

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
