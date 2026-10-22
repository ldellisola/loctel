//! What the window tests of the main window share: the store the home screen
//! is shown over, loaded from the fixed fake mesh.

use std::sync::Arc;
use std::time::Duration;

use loctel_ingest::{logs_from_request, spans_from_request};
use loctel_meshgen::{Generated, Scenario, generate};
use loctel_store::{ManualClock, Store, StoreConfig};

/// 2027-01-15T08:00:00Z. Where the failing checkout starts.
pub const BASE: u64 = 1_800_000_000_000_000_000;
pub const SECOND: u64 = 1_000_000_000;
pub const SEED: u64 = 7;

/// The size the plan fixes for the window, in logical pixels.
pub const WINDOW: (u32, u32) = (1440, 900);

/// Puts a generated scenario into `store`, whole.
pub fn feed(store: &Store, generated: &Generated) {
    for batch in &generated.batches {
        if let Some(request) = &batch.traces {
            let converted = spans_from_request(request.clone());
            assert_eq!(converted.rejected, 0, "generated spans are valid");
            store.ingest_spans(converted.items);
        }
        if let Some(request) = &batch.logs {
            let converted = logs_from_request(request.clone());
            assert_eq!(converted.rejected, 0, "generated logs are valid");
            store.ingest_logs(converted.items);
        }
    }
}

/// The home screen's data: the failing checkout (at `BASE`), the successful
/// one five seconds later and one health check of each service ten seconds
/// after the first. The store's clock reads `BASE` while they arrive and a
/// minute and ten seconds later when this returns, so every trace is complete
/// (the quiet period is 10 s: until then a trace's `card-gateway` is not a node
/// of the map) and inside the default range of 15 minutes.
pub fn home_store() -> Store {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    for (scenario, at) in [
        (Scenario::CheckoutFailure, BASE),
        (Scenario::CheckoutSuccess, BASE + 5 * SECOND),
        (Scenario::HealthChecks { count: 1 }, BASE + 10 * SECOND),
    ] {
        feed(&store, &generate(&scenario, SEED, at));
    }
    clock.advance(Duration::from_secs(70));
    store
}
