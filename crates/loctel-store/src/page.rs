//! Paging for the store's searches.

use serde::Serialize;

/// Which page of a search to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageRequest {
    /// The most items to return. 0 returns an empty page with no cursor.
    pub limit: usize,
    /// The `next_cursor` of the previous page, or `None` for the first page.
    /// It is opaque: pass back exactly what the store gave you.
    pub cursor: Option<String>,
}

/// One page of results, in the order the search defines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Pass this as `PageRequest::cursor` to read the next page. `None`
    /// means there is nothing after this page.
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    pub(crate) fn empty() -> Self {
        Page {
            items: Vec::new(),
            next_cursor: None,
        }
    }
}
