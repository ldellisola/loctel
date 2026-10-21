//! Attribute values and lookup.

use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};

use crate::ids::hex_string;

/// An OTLP attribute value.
#[derive(Clone, Debug, PartialEq)]
pub enum AttrValue {
    Str(String),
    Bool(bool),
    Int(i64),
    Double(f64),
    Bytes(Vec<u8>),
    Array(Vec<AttrValue>),
    Map(Vec<(String, AttrValue)>),
}

/// Ordered key/value pairs, as OTLP sends them. Lookup is by [`attr`].
pub type Attributes = Vec<(String, AttrValue)>;

/// The first attribute called `key`.
// The signature is the plan's own; callers hold `Attributes` (a `Vec`).
#[allow(clippy::ptr_arg)]
pub fn attr<'a>(attrs: &'a Attributes, key: &str) -> Option<&'a AttrValue> {
    attrs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

/// The first attribute called `key`, when it is a non-empty string.
pub(crate) fn str_attr<'a>(attrs: &'a [(String, AttrValue)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(name, _)| name == key)
        .and_then(|(_, value)| match value {
            AttrValue::Str(text) if !text.is_empty() => Some(text.as_str()),
            _ => None,
        })
}

impl AttrValue {
    /// A human-readable form: strings as-is, numbers and booleans via
    /// `to_string` (doubles with Rust's `{}`), bytes as lowercase hex, and
    /// arrays and maps as compact JSON.
    pub fn to_display_string(&self) -> String {
        match self {
            AttrValue::Str(text) => text.clone(),
            AttrValue::Bool(value) => value.to_string(),
            AttrValue::Int(value) => value.to_string(),
            AttrValue::Double(value) => value.to_string(),
            AttrValue::Bytes(bytes) => hex_string(bytes),
            // Our own types always serialize; the default is only a seatbelt.
            AttrValue::Array(_) | AttrValue::Map(_) => {
                serde_json::to_string(self).unwrap_or_default()
            }
        }
    }
}

/// Plain JSON shapes: strings, numbers, booleans, hex strings for bytes,
/// arrays, and objects for maps (no enum tags).
impl Serialize for AttrValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            AttrValue::Str(text) => serializer.serialize_str(text),
            AttrValue::Bool(value) => serializer.serialize_bool(*value),
            AttrValue::Int(value) => serializer.serialize_i64(*value),
            AttrValue::Double(value) => serializer.serialize_f64(*value),
            AttrValue::Bytes(bytes) => serializer.serialize_str(&hex_string(bytes)),
            AttrValue::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            AttrValue::Map(pairs) => serialize_attributes(pairs, serializer),
        }
    }
}

/// `serialize_with` helper: attributes as a JSON object (in order) instead of
/// an array of pairs.
pub(crate) fn serialize_attributes<S: Serializer>(
    attrs: &[(String, AttrValue)],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(attrs.len()))?;
    for (key, value) in attrs {
        map.serialize_entry(key, value)?;
    }
    map.end()
}
