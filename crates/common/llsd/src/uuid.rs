use std::fmt;

use serde::{Deserialize, Deserializer, Serializer, de::Visitor};

use crate::value::UUID_NEWTYPE;
use uuid::Uuid;

pub fn serialize<S: Serializer>(value: &Uuid, serializer: S) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_newtype_struct(UUID_NEWTYPE, &value.to_string())
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Uuid, D::Error> {
    deserializer.deserialize_newtype_struct(UUID_NEWTYPE, UuidVisitor)
}

struct UuidVisitor;

impl<'de> Visitor<'de> for UuidVisitor {
    type Value = Uuid;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a UUID")
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error> {
        let value = String::deserialize(deserializer)?;
        Uuid::parse_str(&value).map_err(serde::de::Error::custom)
    }
}
