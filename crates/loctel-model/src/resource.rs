//! The resource (the emitting service) of a span or log record.

use serde::Serialize;

use crate::attr::{Attributes, serialize_attributes, str_attr};

/// What a service calls itself when it does not say.
pub const UNKNOWN_SERVICE: &str = "unknown_service";

/// Attributes describing the emitting entity, shared by all its telemetry.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Resource {
    #[serde(serialize_with = "serialize_attributes")]
    pub attributes: Attributes,
}

impl Resource {
    /// `service.name`, or `"unknown_service"` when it is absent, empty or not
    /// a string. Telemetry is never dropped for lacking a name.
    pub fn service_name(&self) -> &str {
        str_attr(&self.attributes, "service.name").unwrap_or(UNKNOWN_SERVICE)
    }
}
