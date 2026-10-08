//! Serde adapter for using UUIDs with LLSD codecs.

use std::fmt;

use serde::{Deserialize, Deserializer, Serializer, de::Visitor};

use crate::value::UUID_NEWTYPE;
use base::UUID;

/// Serializes the UUID as an LLSD UUID value.
pub fn serialize<S: Serializer>(value: &UUID, serializer: S) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_newtype_struct(UUID_NEWTYPE, &value.to_string())
}

/// Deserializes an LLSD UUID value.
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<UUID, D::Error> {
    deserializer.deserialize_newtype_struct(UUID_NEWTYPE, UUIDVisitor)
}

struct UUIDVisitor;

impl<'de> Visitor<'de> for UUIDVisitor {
    type Value = UUID;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a UUID")
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error> {
        let value = String::deserialize(deserializer)?;
        UUID::parse_str(&value).map_err(serde::de::Error::custom)
    }
}
