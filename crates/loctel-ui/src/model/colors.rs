//! Which colour a service gets. A service is coloured by when the window
//! first saw it, so it keeps its colour in the map, the table dots, the
//! waterfall and the logs for the whole run (design 9.3).

use std::collections::HashMap;

/// The order services were first seen in, for the whole session.
///
/// [`index`](Self::index) hands out 0, 1, 2, ... in order of first request and
/// never changes an answer it has given. The palette has five colours
/// (`Theme::service_palette`), so a view draws service `index` in palette slot
/// `index % 5`: the sixth service shares the first one's colour, and so on.
/// One `ServiceColors` lives as long as the window's data does; a view that
/// made its own would colour services in whatever order it happened to meet
/// them.
#[derive(Clone, Debug, Default)]
pub struct ServiceColors {
    order: HashMap<String, usize>,
}

impl ServiceColors {
    /// No service seen yet.
    pub fn new() -> ServiceColors {
        ServiceColors::default()
    }

    /// The first-seen position of `service`, assigning the next free one the
    /// first time it is asked for. Names are exact: no trimming and no case
    /// folding.
    pub fn index(&mut self, service: &str) -> usize {
        if let Some(&index) = self.order.get(service) {
            return index;
        }
        let index = self.order.len();
        self.order.insert(service.to_owned(), index);
        index
    }

    /// The position of `service` if it has been seen, without assigning one.
    pub fn peek(&self, service: &str) -> Option<usize> {
        self.order.get(service).copied()
    }
}
