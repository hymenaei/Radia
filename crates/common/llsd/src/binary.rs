//! Binary codec for LLSD values and Serde types.
//!
//! Decoders apply [`crate::Limits`] and reject trailing bytes.
//! The binary marker is optional on input and included in output.
//! Dates use little-endian seconds to match the official viewer ecosystem; real values use network byte order.

use std::io::{Read, Write};

use base::UUID;
use serde::{
    Deserialize,
    de::{
        self, DeserializeOwned, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor,
        value::{BytesDeserializer, F64Deserializer, StringDeserializer},
    },
    forward_to_deserialize_any,
};

use crate::{
    Error, Limits, Result,
    ser::{self, Format},
    value::{BINARY_NEWTYPE, DATE_NEWTYPE, URI_NEWTYPE, UUID_NEWTYPE, VALUE_ENUM},
};

const HEADER_MARKER: &[u8] = b"llsd/binary";

fn strip_header(input: &[u8]) -> Option<&[u8]> {
    if input.get(..2)? != b"<?" {
        return None;
    }

    let mut offset = 2;
    while input.get(offset).is_some_and(u8::is_ascii_whitespace) {
        offset += 1;
    }
    let marker_end = offset.checked_add(HEADER_MARKER.len())?;
    if !input.get(offset..marker_end)?.eq_ignore_ascii_case(HEADER_MARKER) {
        return None;
    }
    offset = marker_end;
    while input.get(offset).is_some_and(u8::is_ascii_whitespace) {
        offset += 1;
    }
    if input.get(offset..offset.checked_add(2)?)? != b"?>" {
        return None;
    }
    offset += 2;
    if input.get(offset..).is_some_and(|rest| rest.starts_with(b"\r\n")) {
        offset += 2;
    } else if input.get(offset).is_some_and(|byte| *byte == b'\n' || *byte == b'\r') {
        offset += 1;
    }
    Some(&input[offset..])
}

/// Decodes one binary LLSD value from bytes with the default limits.
pub fn from_slice<T: DeserializeOwned>(input: &[u8]) -> Result<T> {
    from_slice_with_limits(input, Limits::default())
}

/// Decodes one binary LLSD value with caller-supplied limits.
pub fn from_slice_with_limits<T: DeserializeOwned>(input: &[u8], limits: Limits) -> Result<T> {
    if input.len() > limits.max_input_bytes {
        return Err(Error::LimitExceeded("input bytes"));
    }
    let input = match strip_header(input) {
        Some(rest) => rest.trim_ascii_start(),
        None => input,
    };
    let mut deserializer = Deserializer::new(input, limits);
    let value = T::deserialize(&mut deserializer)?;
    if deserializer.offset != input.len() {
        return Err(Error::TrailingData);
    }
    Ok(value)
}

/// Reads and decodes one binary LLSD value with the default limits.
pub fn from_reader<T: DeserializeOwned, R: Read>(mut reader: R) -> Result<T> {
    from_reader_with_limits(&mut reader, Limits::default())
}

/// Reads at most `max_input_bytes + 1` bytes and decodes them with caller-supplied limits.
pub fn from_reader_with_limits<T: DeserializeOwned, R: Read>(reader: &mut R, limits: Limits) -> Result<T> {
    let max = limits.max_input_bytes.saturating_add(1);
    let mut bytes = Vec::with_capacity(max.min(8192));
    reader.take(max as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limits.max_input_bytes {
        return Err(Error::LimitExceeded("input bytes"));
    }
    from_slice_with_limits(&bytes, limits)
}

/// Encodes a Serde value as one binary LLSD document.
pub fn to_vec<T: serde::Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    ser::to_vec(value, Format::Binary)
}

/// Encodes one complete binary LLSD document and writes it to `writer`.
pub fn to_writer<T: serde::Serialize + ?Sized, W: Write>(mut writer: W, value: &T) -> Result<()> {
    ser::to_writer(&mut writer, value, Format::Binary)
}

struct Deserializer<'de> {
    input: &'de [u8],
    offset: usize,
    limits: Limits,
    values: usize,
    depth: usize,
}

impl<'de> Deserializer<'de> {
    fn new(input: &'de [u8], limits: Limits) -> Self {
        Self { input, offset: 0, limits, values: 0, depth: 0 }
    }

    fn peek(&self) -> Result<u8> {
        self.input.get(self.offset).copied().ok_or(Error::UnexpectedEof)
    }

    fn take(&mut self, length: usize) -> Result<&'de [u8]> {
        let end = self.offset.checked_add(length).ok_or(Error::LimitExceeded("input bytes"))?;
        let value = self.input.get(self.offset..end).ok_or(Error::UnexpectedEof)?;
        self.offset = end;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn expect(&mut self, expected: u8) -> Result<()> {
        if self.byte()? == expected { Ok(()) } else { Err(Error::InvalidValue("invalid LLSD binary delimiter".into())) }
    }

    fn count_value(&mut self) -> Result<()> {
        self.values = self.values.checked_add(1).ok_or(Error::LimitExceeded("value count"))?;
        if self.values > self.limits.max_values {
            return Err(Error::LimitExceeded("value count"));
        }
        Ok(())
    }

    fn read_length(&mut self, limit: usize, name: &'static str) -> Result<usize> {
        let value = u32::from_be_bytes(self.take(4)?.try_into().map_err(|_| Error::UnexpectedEof)?) as usize;
        if value > limit {
            return Err(Error::LimitExceeded(name));
        }
        Ok(value)
    }

    fn read_text(&mut self, limit: usize, name: &'static str) -> Result<String> {
        let length = self.read_length(limit, name)?;
        String::from_utf8(self.take(length)?.to_vec()).map_err(|_| Error::InvalidUtf8)
    }

    fn read_array_count(&mut self) -> Result<usize> {
        let count = self.read_length(self.limits.max_container_items, "container items")?;
        if count > self.limits.max_values.saturating_sub(self.values) {
            return Err(Error::LimitExceeded("value count"));
        }
        Ok(count)
    }

    fn read_uuid(&mut self) -> Result<String> {
        let bytes = self.take(16)?;
        let mut value = [0; 16];
        value.copy_from_slice(bytes);
        Ok(UUID::from_bytes(value).to_string())
    }

    fn read_binary(&mut self) -> Result<Vec<u8>> {
        let length = self.read_length(self.limits.max_binary_bytes, "binary bytes")?;
        Ok(self.take(length)?.to_vec())
    }

    fn consume_undefined(&mut self) -> Result<()> {
        self.count_value()?;
        self.expect(b'!')
    }

    fn container<T>(&mut self, visitor: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.depth = self.depth.checked_add(1).ok_or(Error::LimitExceeded("nesting depth"))?;
        if self.depth > self.limits.max_depth {
            self.depth -= 1;
            return Err(Error::LimitExceeded("nesting depth"));
        }
        let result = visitor(self);
        self.depth -= 1;
        result
    }

    fn deserialize_plain_enum<V: Visitor<'de>>(&mut self, visitor: V) -> Result<V::Value> {
        match self.peek()? {
            b's' => {
                self.count_value()?;
                self.expect(b's')?;
                let variant = self.read_text(self.limits.max_string_bytes, "string bytes")?;
                visitor.visit_enum(StringDeserializer::<Error>::new(variant))
            }
            b'{' => {
                self.count_value()?;
                self.expect(b'{')?;
                let count = self.read_array_count()?;
                if count != 1 {
                    return Err(Error::InvalidValue("LLSD enum map must contain exactly one entry".into()));
                }
                self.container(|de| {
                    de.expect(b'k')?;
                    let variant = de.read_text(de.limits.max_map_key_bytes, "map key bytes")?;
                    visitor.visit_enum(BinarySerdeEnumAccess { de, variant })
                })
            }
            _ => Err(Error::InvalidValue("expected an LLSD enum string or single-entry map".into())),
        }
    }
}

impl<'de> de::Deserializer<'de> for &mut Deserializer<'de> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        let tag = self.byte()?;
        match tag {
            b'!' => visitor.visit_unit(),
            b'0' => visitor.visit_bool(false),
            b'1' => visitor.visit_bool(true),
            b'i' => {
                let value = i32::from_be_bytes(self.take(4)?.try_into().map_err(|_| Error::UnexpectedEof)?);
                visitor.visit_i32(value)
            }
            b'r' => {
                let value = f64::from_bits(u64::from_be_bytes(self.take(8)?.try_into().map_err(|_| Error::UnexpectedEof)?));
                visitor.visit_f64(value)
            }
            b'd' => {
                let value = f64::from_bits(u64::from_le_bytes(self.take(8)?.try_into().map_err(|_| Error::UnexpectedEof)?));
                visitor.visit_f64(value)
            }
            b'u' => visitor.visit_string(self.read_uuid()?),
            b's' => visitor.visit_string(self.read_text(self.limits.max_string_bytes, "string bytes")?),
            b'l' => visitor.visit_string(self.read_text(self.limits.max_string_bytes, "string bytes")?),
            b'b' => visitor.visit_byte_buf(self.read_binary()?),
            b'[' => {
                let count = self.read_array_count()?;
                self.container(|de| {
                    let mut access = BinarySeqAccess { de, remaining: count, ended: false };
                    let result = visitor.visit_seq(&mut access);
                    match result {
                        Ok(value) => {
                            access.finish()?;
                            Ok(value)
                        }
                        Err(error) => Err(error),
                    }
                })
            }
            b'{' => {
                let count = self.read_array_count()?;
                self.container(|de| {
                    let mut access = BinaryMapAccess { de, remaining: count, expects_value: false, ended: false };
                    let result = visitor.visit_map(&mut access);
                    match result {
                        Ok(value) => {
                            access.finish()?;
                            Ok(value)
                        }
                        Err(error) => Err(error),
                    }
                })
            }
            other => Err(Error::InvalidValue(format!("unknown LLSD binary marker 0x{other:02x}"))),
        }
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string
    }
    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        if self.byte()? != b'b' {
            return Err(Error::InvalidValue("expected LLSD binary".into()));
        }
        let length = self.read_length(self.limits.max_binary_bytes, "binary bytes")?;
        visitor.visit_bytes(self.take(length)?)
    }
    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        if self.byte()? != b'b' {
            return Err(Error::InvalidValue("expected LLSD binary".into()));
        }
        visitor.visit_byte_buf(self.read_binary()?)
    }
    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        if self.peek()? == b'!' {
            self.consume_undefined()?;
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }
    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.consume_undefined()?;
        visitor.visit_unit()
    }
    fn deserialize_unit_struct<V: Visitor<'de>>(self, _: &'static str, visitor: V) -> Result<V::Value> {
        self.deserialize_unit(visitor)
    }
    fn deserialize_newtype_struct<V: Visitor<'de>>(self, name: &'static str, visitor: V) -> Result<V::Value> {
        match name {
            UUID_NEWTYPE => {
                self.count_value()?;
                if self.byte()? != b'u' {
                    return Err(Error::InvalidValue("expected LLSD UUID".into()));
                }
                visitor.visit_newtype_struct(StringDeserializer::<Error>::new(self.read_uuid()?))
            }
            URI_NEWTYPE => {
                self.count_value()?;
                if self.byte()? != b'l' {
                    return Err(Error::InvalidValue("expected LLSD URI".into()));
                }
                visitor
                    .visit_newtype_struct(StringDeserializer::<Error>::new(self.read_text(self.limits.max_string_bytes, "string bytes")?))
            }
            DATE_NEWTYPE => {
                self.count_value()?;
                if self.byte()? != b'd' {
                    return Err(Error::InvalidValue("expected LLSD date".into()));
                }
                let value = f64::from_bits(u64::from_le_bytes(self.take(8)?.try_into().map_err(|_| Error::UnexpectedEof)?));
                visitor.visit_newtype_struct(F64Deserializer::<Error>::new(value))
            }
            BINARY_NEWTYPE => {
                self.count_value()?;
                if self.byte()? != b'b' {
                    return Err(Error::InvalidValue("expected LLSD binary".into()));
                }
                visitor.visit_newtype_struct(BytesDeserializer::<Error>::new(&self.read_binary()?))
            }
            _ => visitor.visit_newtype_struct(self),
        }
    }
    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        if self.byte()? != b'[' {
            return Err(Error::InvalidValue("expected LLSD array".into()));
        }
        let count = self.read_array_count()?;
        self.container(|de| {
            let mut access = BinarySeqAccess { de, remaining: count, ended: false };
            let result = visitor.visit_seq(&mut access);
            match result {
                Ok(value) => {
                    access.finish()?;
                    Ok(value)
                }
                Err(error) => Err(error),
            }
        })
    }
    fn deserialize_tuple<V: Visitor<'de>>(self, _: usize, visitor: V) -> Result<V::Value> {
        self.deserialize_seq(visitor)
    }
    fn deserialize_tuple_struct<V: Visitor<'de>>(self, _: &'static str, _: usize, visitor: V) -> Result<V::Value> {
        self.deserialize_seq(visitor)
    }
    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        if self.byte()? != b'{' {
            return Err(Error::InvalidValue("expected LLSD map".into()));
        }
        let count = self.read_array_count()?;
        self.container(|de| {
            let mut access = BinaryMapAccess { de, remaining: count, expects_value: false, ended: false };
            let result = visitor.visit_map(&mut access);
            match result {
                Ok(value) => {
                    access.finish()?;
                    Ok(value)
                }
                Err(error) => Err(error),
            }
        })
    }
    fn deserialize_struct<V: Visitor<'de>>(self, _: &'static str, _: &'static [&'static str], visitor: V) -> Result<V::Value> {
        self.deserialize_map(visitor)
    }
    fn deserialize_enum<V: Visitor<'de>>(self, name: &'static str, _: &'static [&'static str], visitor: V) -> Result<V::Value> {
        if name == VALUE_ENUM {
            let variant = match self.peek()? {
                b'!' => "Undefined",
                b'0' | b'1' => "Boolean",
                b'i' => "Integer",
                b'r' => "Real",
                b'u' => "UUID",
                b's' => "String",
                b'd' => "Date",
                b'l' => "URI",
                b'b' => "Binary",
                b'[' => "Array",
                b'{' => "Map",
                other => return Err(Error::InvalidValue(format!("unknown LLSD binary marker 0x{other:02x}"))),
            };
            visitor.visit_enum(BinaryDynamicEnumAccess { de: self, variant })
        } else {
            self.deserialize_plain_enum(visitor)
        }
    }
    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        Ignored::deserialize(self)?;
        visitor.visit_unit()
    }
    fn is_human_readable(&self) -> bool {
        false
    }
}

struct BinarySeqAccess<'a, 'de> {
    de: &'a mut Deserializer<'de>,
    remaining: usize,
    ended: bool,
}

impl BinarySeqAccess<'_, '_> {
    fn finish(&mut self) -> Result<()> {
        if self.ended {
            return Ok(());
        }
        if self.remaining != 0 {
            return Err(Error::InvalidValue("LLSD array was not fully read".into()));
        }
        self.de.expect(b']')?;
        self.ended = true;
        Ok(())
    }
}

impl<'de> SeqAccess<'de> for BinarySeqAccess<'_, 'de> {
    type Error = Error;
    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        if self.remaining == 0 {
            self.finish()?;
            return Ok(None);
        }
        self.remaining -= 1;
        seed.deserialize(&mut *self.de).map(Some)
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.remaining)
    }
}

struct BinaryMapAccess<'a, 'de> {
    de: &'a mut Deserializer<'de>,
    remaining: usize,
    expects_value: bool,
    ended: bool,
}

impl BinaryMapAccess<'_, '_> {
    fn finish(&mut self) -> Result<()> {
        if self.ended {
            return Ok(());
        }
        if self.remaining != 0 || self.expects_value {
            return Err(Error::InvalidValue("LLSD map was not fully read".into()));
        }
        self.de.expect(b'}')?;
        self.ended = true;
        Ok(())
    }
}

impl<'de> MapAccess<'de> for BinaryMapAccess<'_, 'de> {
    type Error = Error;
    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        if self.expects_value {
            return Err(Error::InvalidValue("LLSD map value was not read".into()));
        }
        if self.remaining == 0 {
            self.finish()?;
            return Ok(None);
        }
        self.de.expect(b'k')?;
        let key = self.de.read_text(self.de.limits.max_map_key_bytes, "map key bytes")?;
        self.expects_value = true;
        seed.deserialize(StringDeserializer::<Error>::new(key)).map(Some)
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        if !self.expects_value {
            return Err(Error::InvalidValue("LLSD map key is missing".into()));
        }
        self.expects_value = false;
        self.remaining -= 1;
        seed.deserialize(&mut *self.de)
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.remaining)
    }
}

struct BinaryDynamicEnumAccess<'a, 'de> {
    de: &'a mut Deserializer<'de>,
    variant: &'static str,
}

impl<'a, 'de> EnumAccess<'de> for BinaryDynamicEnumAccess<'a, 'de> {
    type Error = Error;
    type Variant = BinaryDynamicVariantAccess<'a, 'de>;
    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant)> {
        let value = seed.deserialize(StringDeserializer::<Error>::new(self.variant.to_owned()))?;
        Ok((value, BinaryDynamicVariantAccess { de: self.de }))
    }
}

struct BinaryDynamicVariantAccess<'a, 'de> {
    de: &'a mut Deserializer<'de>,
}

struct BinarySerdeEnumAccess<'a, 'de> {
    de: &'a mut Deserializer<'de>,
    variant: String,
}

impl<'a, 'de> EnumAccess<'de> for BinarySerdeEnumAccess<'a, 'de> {
    type Error = Error;
    type Variant = BinarySerdeVariantAccess<'a, 'de>;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant)> {
        let variant = seed.deserialize(StringDeserializer::<Error>::new(self.variant))?;
        Ok((variant, BinarySerdeVariantAccess { de: self.de }))
    }
}

struct BinarySerdeVariantAccess<'a, 'de> {
    de: &'a mut Deserializer<'de>,
}

impl BinarySerdeVariantAccess<'_, '_> {
    fn finish(self) -> Result<()> {
        self.de.expect(b'}')
    }
}

impl<'de> VariantAccess<'de> for BinarySerdeVariantAccess<'_, 'de> {
    type Error = Error;

    fn unit_variant(self) -> Result<()> {
        self.de.consume_undefined()?;
        self.finish()
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        let value = seed.deserialize(&mut *self.de)?;
        self.finish()?;
        Ok(value)
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> {
        let value = de::Deserializer::deserialize_tuple(&mut *self.de, len, visitor)?;
        self.finish()?;
        Ok(value)
    }

    fn struct_variant<V: Visitor<'de>>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value> {
        let value = de::Deserializer::deserialize_struct(&mut *self.de, "", fields, visitor)?;
        self.finish()?;
        Ok(value)
    }
}

impl<'de> VariantAccess<'de> for BinaryDynamicVariantAccess<'_, 'de> {
    type Error = Error;
    fn unit_variant(self) -> Result<()> {
        self.de.consume_undefined()
    }
    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        seed.deserialize(&mut *self.de)
    }
    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> {
        de::Deserializer::deserialize_tuple(&mut *self.de, len, visitor)
    }
    fn struct_variant<V: Visitor<'de>>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value> {
        de::Deserializer::deserialize_struct(&mut *self.de, "", fields, visitor)
    }
}

struct Ignored;
impl<'de> Deserialize<'de> for Ignored {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct IgnoredVisitor;
        impl<'de> Visitor<'de> for IgnoredVisitor {
            type Value = Ignored;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("any LLSD value")
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(Ignored)
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> std::result::Result<Self::Value, E> {
                Ok(Ignored)
            }
            fn visit_i32<E: de::Error>(self, _: i32) -> std::result::Result<Self::Value, E> {
                Ok(Ignored)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<Self::Value, E> {
                Ok(Ignored)
            }
            fn visit_str<E: de::Error>(self, _: &str) -> std::result::Result<Self::Value, E> {
                Ok(Ignored)
            }
            fn visit_string<E: de::Error>(self, _: String) -> std::result::Result<Self::Value, E> {
                Ok(Ignored)
            }
            fn visit_bytes<E: de::Error>(self, _: &[u8]) -> std::result::Result<Self::Value, E> {
                Ok(Ignored)
            }
            fn visit_byte_buf<E: de::Error>(self, _: Vec<u8>) -> std::result::Result<Self::Value, E> {
                Ok(Ignored)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error> {
                while seq.next_element::<Ignored>()?.is_some() {}
                Ok(Ignored)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Self::Value, A::Error> {
                while map.next_entry::<Ignored, Ignored>()?.is_some() {}
                Ok(Ignored)
            }
        }
        deserializer.deserialize_any(IgnoredVisitor)
    }
}
