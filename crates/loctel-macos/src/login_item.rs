//! Start at login, through `SMAppService.mainAppService` (macOS 13 and later;
//! loctel needs 26): the system's own login item for the running app.
//!
//! The system is the source of truth. The user can switch the item off in
//! System Settings > General > Login Items, so [`is_enabled`] asks every time
//! and nothing is cached or mirrored in `settings.toml`.
//!
//! Registering needs a signed app bundle (the release's ad-hoc signature does);
//! from a plain `cargo run` binary it fails, and [`set_enabled`] says so.

use anyhow::anyhow;
use loctel_ui::app::LoginItem;
use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// The system's login item as the window's [`LoginItem`]: what
/// `AppBackend` is given on macOS, so the settings window and the popover
/// ask the system (through [`is_enabled`] and [`set_enabled`]) every time.
pub struct SystemLoginItem;

impl LoginItem for SystemLoginItem {
    fn is_enabled(&self) -> bool {
        is_enabled()
    }

    fn set_enabled(&self, on: bool) -> anyhow::Result<()> {
        set_enabled(on)
    }
}

/// Whether the login item is registered and allowed to run. Registered but
/// waiting for the user's approval in System Settings (`RequiresApproval`)
/// is not enabled: nothing will start at login yet.
pub fn is_enabled() -> bool {
    // SAFETY: `mainAppService` and `status` are plain ServiceManagement calls
    // with no preconditions.
    let status = unsafe { SMAppService::mainAppService().status() };
    status == SMAppServiceStatus::Enabled
}

/// Registers (`on`) or unregisters the app as a login item. Registering when
/// already registered, or unregistering when not, is the system's call: it
/// may be fine or an error, and either way [`is_enabled`] tells the result.
pub fn set_enabled(on: bool) -> anyhow::Result<()> {
    // SAFETY: as in `is_enabled`; the register calls return the system's
    // `NSError` instead of throwing.
    let result = unsafe {
        let service = SMAppService::mainAppService();
        if on {
            service.registerAndReturnError()
        } else {
            service.unregisterAndReturnError()
        }
    };
    result.map_err(|error| {
        anyhow!(
            "cannot {} the login item: {}",
            if on { "register" } else { "unregister" },
            error.localizedDescription()
        )
    })
}
