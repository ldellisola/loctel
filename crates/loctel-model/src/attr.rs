//! Attribute values and lookup.

use std::fmt;

use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Serialize, Serializer};

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

/// The most entries a collection reserves room for up front. The sizes a
/// binary format reports come from the data, so a damaged length must not be
/// able to ask for gigabytes before the first entry is read.
const RESERVE_MAX: usize = 1024;

/// Two shapes, by [`Serializer::is_human_readable`].
///
/// - **Human readable (JSON):** plain JSON shapes: strings, numbers, booleans,
///   hex strings for bytes, arrays, and objects for maps (no enum tags).
/// - **Binary (postcard, the segment files of the store):** the tagged
///   variants of [`Tagged`]. A binary format cannot be asked "what comes
///   next", and the plain shapes would also lose the difference between bytes
///   and a string.
impl Serialize for AttrValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if !serializer.is_human_readable() {
            return Tagged::serialize(self, serializer);
        }
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

/// Reads back what [`Serialize`] wrote, in either shape.
///
/// A binary format comes back exactly. JSON reads each value by its shape,
/// which cannot be exact in three places, all of them gaps in JSON itself:
///
/// - a byte value was written as a hex string and reads back as
///   [`AttrValue::Str`];
/// - `null`, which is what JSON makes of NaN and the infinities, reads back as
///   a NaN [`AttrValue::Double`];
/// - an integer above `i64::MAX` (not something this crate writes) reads as a
///   double.
impl<'de> Deserialize<'de> for AttrValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            deserializer.deserialize_any(ValueVisitor)
        } else {
            Tagged::deserialize(deserializer)
        }
    }
}

/// The binary shape of [`AttrValue`]: a derived, tagged enum that serde's
/// `remote` support applies to the real one.
#[derive(Serialize, Deserialize)]
#[serde(remote = "AttrValue")]
enum Tagged {
    Str(String),
    Bool(bool),
    Int(i64),
    Double(f64),
    Bytes(Vec<u8>),
    Array(Vec<AttrValue>),
    Map(Vec<(String, AttrValue)>),
}

/// Reads one JSON-shaped value.
struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = AttrValue;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an attribute value: a string, number, boolean, array or object")
    }

    fn visit_bool<E>(self, value: bool) -> Result<AttrValue, E> {
        Ok(AttrValue::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<AttrValue, E> {
        Ok(AttrValue::Int(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<AttrValue, E> {
        Ok(match i64::try_from(value) {
            Ok(value) => AttrValue::Int(value),
            Err(_) => AttrValue::Double(value as f64),
        })
    }

    fn visit_f64<E>(self, value: f64) -> Result<AttrValue, E> {
        Ok(AttrValue::Double(value))
    }

    fn visit_str<E>(self, value: &str) -> Result<AttrValue, E> {
        Ok(AttrValue::Str(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<AttrValue, E> {
        Ok(AttrValue::Str(value))
    }

    fn visit_unit<E>(self) -> Result<AttrValue, E> {
        Ok(AttrValue::Double(f64::NAN))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<AttrValue, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(RESERVE_MAX));
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(AttrValue::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<AttrValue, A::Error> {
        collect_pairs(map).map(AttrValue::Map)
    }
}

/// Every entry of a map, in order, repeated keys included.
fn collect_pairs<'de, A: MapAccess<'de>>(mut map: A) -> Result<Vec<(String, AttrValue)>, A::Error> {
    let mut pairs = Vec::with_capacity(map.size_hint().unwrap_or(0).min(RESERVE_MAX));
    while let Some(pair) = map.next_entry()? {
        pairs.push(pair);
    }
    Ok(pairs)
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

/// `deserialize_with` helper, the other half of [`serialize_attributes`]: the
/// entries of the map in order, a repeated key kept as often as it appears.
pub(crate) fn deserialize_attributes<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Attributes, D::Error> {
    struct PairsVisitor;

    impl<'de> Visitor<'de> for PairsVisitor {
        type Value = Attributes;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a map of attribute names to values")
        }

        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Attributes, A::Error> {
            collect_pairs(map)
        }
    }

    deserializer.deserialize_map(PairsVisitor)
}
