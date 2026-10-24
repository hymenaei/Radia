use std::{fmt, ops::Deref};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Visitor};

use super::BINARY_NEWTYPE;

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Binary(Vec<u8>);

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

impl Serialize for Binary {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_newtype_struct(BINARY_NEWTYPE, &ByteSlice(&self.0))
    }
}

impl<'de> Deserialize<'de> for Binary {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_newtype_struct(BINARY_NEWTYPE, BinaryVisitor)
    }
}

struct ByteSlice<'a>(&'a [u8]);

impl Serialize for ByteSlice<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.0)
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
