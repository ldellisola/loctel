//! The macOS shell: the menu bar status item, and start at login.
//!
//! The crate is empty on every other OS (and its dependencies are macOS-only),
//! except for the parts that hold decisions and are plain Rust: the menu's
//! rows, the first-launch step and the icon files. Those are compiled for the
//! tests on any OS, so the Linux gate runs them; the code that draws and
//! drives AppKit is only type-checked off a Mac (`script/check-macos.sh`).
//!
//! - `install(cx, engine, settings_path)` puts loctel in the menu bar. It is
//!   all `loctel-app` calls.
//! - `login_item` turns start at login on and off (`SMAppService`).
//!
//! All `objc2` and `unsafe` code in the project lives in this crate. See
//! `docs/areas/macos.md`, and read its "Unverified until the M1 Mac check"
//! list before treating anything here as proven.

#[cfg(any(target_os = "macos", test))]
mod first_launch;
#[cfg(any(target_os = "macos", test))]
mod icon;
#[cfg(any(target_os = "macos", test))]
mod menu;

#[cfg(target_os = "macos")]
pub mod login_item;
#[cfg(target_os = "macos")]
mod status_item;

#[cfg(target_os = "macos")]
pub use status_item::install;
