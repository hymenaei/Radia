use std::{collections::BTreeMap, fmt, ops::Deref};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{EnumAccess, VariantAccess, Visitor},
};
use time::{
    Date as CalendarDate, OffsetDateTime, Time as TimeOfDay, UtcOffset,
    format_description::{parse_borrowed, well_known::Rfc3339},
};

use crate::Error;
use uuid::Uuid;

pub(crate) const UUID_NEWTYPE: &str = "llsd::UUID";
pub(crate) const URI_NEWTYPE: &str = "llsd::URI";
pub(crate) const DATE_NEWTYPE: &str = "llsd::Date";
pub(crate) const BINARY_NEWTYPE: &str = "llsd::Binary";
pub(crate) const VALUE_ENUM: &str = "Value";

pub type Map = BTreeMap<String, Value>;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Undefined,
    Boolean(bool),
    Integer(i32),
    Real(f64),
    Uuid(Uuid),
    String(String),
    Date(Date),

    URI(URI),
    Binary(Binary),
    Array(Vec<Self>),
    Map(Map),
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct URI(String);

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Binary(Vec<u8>);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Date(OffsetDateTime);

impl URI {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<String> for URI {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for URI {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl AsRef<str> for URI {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Deref for URI {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl fmt::Display for URI {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Binary {
    pub fn new(value: impl Into<Vec<u8>>) -> Self {
        Self(value.into())
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }
}

impl From<Vec<u8>> for Binary {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

impl From<&[u8]> for Binary {
    fn from(value: &[u8]) -> Self {
        Self(value.to_vec())
    }
}

impl AsRef<[u8]> for Binary {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Deref for Binary {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl Date {
    pub const UNIX_EPOCH: Self = Self(OffsetDateTime::UNIX_EPOCH);

    pub const fn from_datetime(value: OffsetDateTime) -> Self {
        Self(value)
    }

    pub const fn as_datetime(self) -> OffsetDateTime {
        self.0
    }

    pub fn from_unix_seconds(value: f64) -> Option<Self> {
        if !value.is_finite() {
            return None;
        }
        let nanos = (value * 1_000_000_000.0).round();
        if nanos < i128::MIN as f64 || nanos > i128::MAX as f64 {
            return None;
        }
        OffsetDateTime::from_unix_timestamp_nanos(nanos as i128).ok().map(Self)
    }

    pub fn unix_seconds(self) -> f64 {
        self.0.unix_timestamp_nanos() as f64 / 1_000_000_000.0
    }

    pub(crate) fn parse(value: &str) -> Result<Self, Error> {
        let datetime = if let Ok(datetime) = OffsetDateTime::parse(value, &Rfc3339) {
            datetime
        } else {
            let format = parse_borrowed::<3>("[year]-[month]-[day]").map_err(|error| Error::InvalidValue(error.to_string()))?;
            let date = CalendarDate::parse(value, &format).map_err(|error| Error::InvalidValue(error.to_string()))?;
            date.with_time(TimeOfDay::MIDNIGHT).assume_utc()
        };
        Ok(Self(datetime.to_offset(UtcOffset::UTC)))
    }

    pub(crate) fn format(self) -> Result<String, Error> {
        self.0.to_offset(UtcOffset::UTC).format(&Rfc3339).map_err(|error| Error::InvalidValue(error.to_string()))
    }
}

impl From<OffsetDateTime> for Date {
    fn from(value: OffsetDateTime) -> Self {
        Self(value)
    }
}

impl From<Date> for OffsetDateTime {
    fn from(value: Date) -> Self {
        value.0
    }
}

impl Value {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_integer(&self) -> Option<i32> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_real(&self) -> Option<f64> {
        match self {
            Self::Real(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_uuid(&self) -> Option<Uuid> {
        match self {
            Self::Uuid(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            Self::URI(value) => Some(value.as_str()),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&Map> {
        match self {
            Self::Map(value) => Some(value),
            _ => None,
        }
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Undefined => serializer.serialize_unit(),
            Self::Boolean(value) => serializer.serialize_bool(*value),
            Self::Integer(value) => serializer.serialize_i32(*value),
            Self::Real(value) => serializer.serialize_f64(*value),
            Self::Uuid(value) => serializer.serialize_newtype_struct(UUID_NEWTYPE, &value.to_string()),
            Self::String(value) => serializer.serialize_str(value),
            Self::Date(value) => value.serialize(serializer),
            Self::URI(value) => value.serialize(serializer),
            Self::Binary(value) => value.serialize(serializer),
            Self::Array(value) => value.serialize(serializer),
            Self::Map(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_enum(VALUE_ENUM, &[], ValueVisitor)
    }
}

struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an LLSD value")
    }

    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> std::result::Result<Self::Value, A::Error> {
        let (variant, value) = data.variant::<String>()?;
        match variant.as_str() {
            "Undefined" => {
                value.unit_variant()?;
                Ok(Value::Undefined)
            }
            "Boolean" => value.newtype_variant().map(Value::Boolean),
            "Integer" => value.newtype_variant().map(Value::Integer),
            "Real" => value.newtype_variant().map(Value::Real),
            "UUID" => {
                let value = value.newtype_variant::<String>()?;
                Uuid::parse_str(&value).map(Value::Uuid).map_err(serde::de::Error::custom)
            }
            "String" => value.newtype_variant().map(Value::String),
            "Date" => value.newtype_variant().map(Value::Date),
            "URI" => value.newtype_variant().map(Value::URI),
            "Binary" => value.newtype_variant().map(Value::Binary),
            "Array" => value.newtype_variant().map(Value::Array),
            "Map" => value.newtype_variant().map(Value::Map),
            _ => Err(serde::de::Error::unknown_variant(
                &variant,
                &["Undefined", "Boolean", "Integer", "Real", "UUID", "String", "Date", "URI", "Binary", "Array", "Map"],
            )),
        }
    }
}

impl Serialize for URI {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_newtype_struct(URI_NEWTYPE, &self.0)
    }
}

impl<'de> Deserialize<'de> for URI {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_newtype_struct(URI_NEWTYPE, URIVisitor)
    }
}

struct URIVisitor;

impl<'de> Visitor<'de> for URIVisitor {
    type Value = URI;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an LLSD URI")
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error> {
        String::deserialize(deserializer).map(URI)
    }
}

impl Serialize for Date {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let value = self.0.format(&Rfc3339).map_err(serde::ser::Error::custom)?;
        serializer.serialize_newtype_struct(DATE_NEWTYPE, &value)
    }
}

impl<'de> Deserialize<'de> for Date {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_newtype_struct(DATE_NEWTYPE, DateVisitor)
    }
}

struct DateVisitor;

impl<'de> Visitor<'de> for DateVisitor {
    type Value = Date;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an LLSD date")
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error> {
        deserializer.deserialize_any(DatePayloadVisitor)
    }
}

struct DatePayloadVisitor;

impl<'de> Visitor<'de> for DatePayloadVisitor {
    type Value = Date;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an LLSD date or Unix timestamp")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Date::parse(value).map_err(E::custom)
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> std::result::Result<Self::Value, E> {
        self.visit_str(&value)
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> std::result::Result<Self::Value, E> {
        Date::from_unix_seconds(value).ok_or_else(|| E::custom(Error::InvalidValue("invalid LLSD date timestamp".into())))
    }
}

impl Serialize for Binary {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_newtype_struct(BINARY_NEWTYPE, &ByteSlice(&self.0))
    }
}

struct ByteSlice<'a>(&'a [u8]);

impl Serialize for ByteSlice<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.0)
    }
}

impl<'de> Deserialize<'de> for Binary {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_newtype_struct(BINARY_NEWTYPE, BinaryVisitor)
    }
}

struct BinaryVisitor;

impl<'de> Visitor<'de> for BinaryVisitor {
    type Value = Binary;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LLSD binary data")
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error> {
        deserializer.deserialize_byte_buf(BinaryPayloadVisitor)
    }
}

struct BinaryPayloadVisitor;

impl<'de> Visitor<'de> for BinaryPayloadVisitor {
    type Value = Binary;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("binary data")
    }

    fn visit_bytes<E: serde::de::Error>(self, value: &[u8]) -> std::result::Result<Self::Value, E> {
        Ok(Binary(value.to_vec()))
    }

    fn visit_byte_buf<E: serde::de::Error>(self, value: Vec<u8>) -> std::result::Result<Self::Value, E> {
        Ok(Binary(value))
    }
}
