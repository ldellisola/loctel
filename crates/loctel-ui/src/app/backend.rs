//! What the main window needs from the engine, as a trait, so the views never
//! name the engine and a test can hand them a fixed one.

use loctel_engine::{EngineHandle, EngineStatus};
use loctel_store::Store;

use crate::model::format;

/// The window's view of the running app.
///
/// The views read the store and the status, and ask for two actions. They hold
/// it as `Rc<dyn UiBackend>`: it lives on GPUI's foreground thread, and the
/// parts that cross threads are the [`Store`] (a cheap clone) a refresh moves
/// onto the background executor.
pub trait UiBackend: 'static {
    /// The store the receivers fill. A clone is the same store.
    fn store(&self) -> Store;
    /// A snapshot of pause, endpoints, rates, what the store holds and the
    /// discard rules.
    fn status(&self) -> EngineStatus;
    /// Empties the store (traces and logs; the `*_received` totals stay).
    /// Receiving carries on.
    fn clear_all(&self);
    /// The offset of the system's time zone from UTC, in seconds, for the
    /// clock times the tables show. Tests return 0.
    fn utc_offset(&self) -> i32;
}

impl UiBackend for EngineHandle {
    fn store(&self) -> Store {
        EngineHandle::store(self).clone()
    }

    fn status(&self) -> EngineStatus {
        EngineHandle::status(self)
    }

    fn clear_all(&self) {
        EngineHandle::store(self).clear();
    }

    fn utc_offset(&self) -> i32 {
        format::local_utc_offset()
    }
}
