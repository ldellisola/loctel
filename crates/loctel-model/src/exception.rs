//! Exceptions, as semantic conventions carry them (`exception.*`).

use serde::{Deserialize, Serialize};

use crate::attr::{AttrValue, Attributes, attr};

/// An exception reported on a span event or a log record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExceptionInfo {
    /// `exception.type`, or empty when absent.
    pub type_name: String,
    /// `exception.message`, or empty when absent.
    pub message: String,
    /// `exception.stacktrace`, when present.
    pub stacktrace: Option<String>,
}

const TYPE: &str = "exception.type";
const MESSAGE: &str = "exception.message";
const STACKTRACE: &str = "exception.stacktrace";

impl ExceptionInfo {
    /// Reads the three `exception.*` attributes; missing ones are empty.
    pub(crate) fn from_attributes(attrs: &Attributes) -> Self {
        let text = |key| attr(attrs, key).map(AttrValue::to_display_string);
        ExceptionInfo {
            type_name: text(TYPE).unwrap_or_default(),
            message: text(MESSAGE).unwrap_or_default(),
            stacktrace: text(STACKTRACE),
        }
    }

    /// Like [`Self::from_attributes`], but `None` unless at least one of the
    /// three attributes is present.
    pub(crate) fn find_in_attributes(attrs: &Attributes) -> Option<Self> {
        [TYPE, MESSAGE, STACKTRACE]
            .iter()
            .any(|key| attr(attrs, key).is_some())
            .then(|| Self::from_attributes(attrs))
    }
}
