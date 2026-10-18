//! Notation codec for LLSD values and Serde types.
//!
//! Decoders apply [`crate::Limits`], allow trailing whitespace, and reject other trailing data.
//! The notation marker is optional on input and included in output.

use std::{
    io::{Read, Write},
    str,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor,
    value::{BytesDeserializer, StringDeserializer},
};
use serde::forward_to_deserialize_any;

use crate::{
    Date, Error, Limits, Result, base64_decoded_len,
    ser::{self, Format},
    value::{BINARY_NEWTYPE, DATE_NEWTYPE, URI_NEWTYPE, UUID_NEWTYPE, VALUE_ENUM},
};

const HEADER_MARKER: &[u8] = b"llsd/notation";

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

/// Decodes one LLSD notation value from UTF-8 bytes with the default limits.
pub fn from_slice<T: DeserializeOwned>(input: &[u8]) -> Result<T> {
    from_slice_with_limits(input, Limits::default())
}

/// Decodes one LLSD notation value with caller-supplied limits.
pub fn from_slice_with_limits<T: DeserializeOwned>(input: &[u8], limits: Limits) -> Result<T> {
    if input.len() > limits.max_input_bytes {
        return Err(Error::LimitExceeded("input bytes"));
    }
    let input = strip_header(input).unwrap_or(input);
    let mut de = Deserializer::new(input, limits);
    let value = T::deserialize(&mut de)?;
    de.skip_space();
    if de.offset != input.len() {
        return Err(Error::TrailingData);
    }
    Ok(value)
}

/// Decodes one LLSD notation value from a string with the default limits.
pub fn from_str<T: DeserializeOwned>(input: &str) -> Result<T> {
    from_slice(input.as_bytes())
}

/// Reads and decodes one LLSD notation value with the default limits.
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

/// Encodes a Serde value as one LLSD notation document.
pub fn to_vec<T: serde::Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    ser::to_vec(value, Format::Notation)
}

/// Encodes a Serde value as UTF-8 LLSD notation.
pub fn to_string<T: serde::Serialize + ?Sized>(value: &T) -> Result<String> {
    String::from_utf8(to_vec(value)?).map_err(|_| Error::InvalidUtf8)
}

/// Encodes one complete LLSD notation document and writes it to `writer`.
pub fn to_writer<T: serde::Serialize + ?Sized, W: Write>(mut writer: W, value: &T) -> Result<()> {
    ser::to_writer(&mut writer, value, Format::Notation)
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
    fn skip_space(&mut self) {
        while self.input.get(self.offset).is_some_and(u8::is_ascii_whitespace) {
            self.offset += 1;
        }
    }
    fn peek(&mut self) -> Result<u8> {
        self.skip_space();
        self.input.get(self.offset).copied().ok_or(Error::UnexpectedEof)
    }
    fn byte(&mut self) -> Result<u8> {
        let value = self.peek()?;
        self.offset += 1;
        Ok(value)
    }
    fn take(&mut self, length: usize) -> Result<&'de [u8]> {
        let end = self.offset.checked_add(length).ok_or(Error::LimitExceeded("input bytes"))?;
        let value = self.input.get(self.offset..end).ok_or(Error::UnexpectedEof)?;
        self.offset = end;
        Ok(value)
    }
    fn expect(&mut self, expected: u8) -> Result<()> {
        if self.byte()? == expected { Ok(()) } else { Err(Error::InvalidValue("invalid LLSD notation delimiter".into())) }
    }
    fn expect_immediate(&mut self, expected: u8) -> Result<()> {
        let actual = *self.input.get(self.offset).ok_or(Error::UnexpectedEof)?;
        self.offset += 1;
        if actual == expected { Ok(()) } else { Err(Error::InvalidValue("invalid LLSD notation delimiter".into())) }
    }
    fn count_value(&mut self) -> Result<()> {
        self.values = self.values.checked_add(1).ok_or(Error::LimitExceeded("value count"))?;
        if self.values > self.limits.max_values {
            return Err(Error::LimitExceeded("value count"));
        }
        Ok(())
    }
    fn container<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.depth = self.depth.checked_add(1).ok_or(Error::LimitExceeded("nesting depth"))?;
        if self.depth > self.limits.max_depth {
            self.depth -= 1;
            return Err(Error::LimitExceeded("nesting depth"));
        }
        let result = f(self);
        self.depth -= 1;
        result
    }
    fn deserialize_plain_enum<V: Visitor<'de>>(&mut self, visitor: V) -> Result<V::Value> {
        match self.peek()? {
            b'\'' | b'"' => {
                self.count_value()?;
                visitor.visit_enum(StringDeserializer::<Error>::new(self.quoted()?))
            }
            b's' => {
                self.count_value()?;
                self.offset += 1;
                visitor.visit_enum(StringDeserializer::<Error>::new(self.typed_string()?))
            }
            b'{' => {
                self.count_value()?;
                self.expect(b'{')?;
                if self.limits.max_container_items == 0 {
                    return Err(Error::LimitExceeded("container items"));
                }
                self.container(|de| {
                    let variant = match de.peek()? {
                        b'\'' | b'"' => de.quoted()?,
                        b's' => {
                            de.offset += 1;
                            de.typed_string()?
                        }
                        _ => return Err(Error::InvalidValue("LLSD enum map key must be a string".into())),
                    };
                    if variant.len() > de.limits.max_map_key_bytes {
                        return Err(Error::LimitExceeded("map key bytes"));
                    }
                    de.skip_space();
                    de.expect(b':')?;
                    visitor.visit_enum(NotationSerdeEnumAccess { de, variant })
                })
            }
            _ => Err(Error::InvalidValue("expected an LLSD enum string or single-entry map".into())),
        }
    }
    fn token(&mut self) -> Result<&'de [u8]> {
        self.skip_space();
        let start = self.offset;
        while let Some(byte) = self.input.get(self.offset) {
            if byte.is_ascii_whitespace() || matches!(*byte, b',' | b']' | b'}' | b':') {
                break;
            }
            self.offset += 1;
        }
        if self.offset == start {
            return Err(Error::UnexpectedEof);
        }
        Ok(&self.input[start..self.offset])
    }
    fn quoted(&mut self) -> Result<String> {
        self.quoted_with_limit(self.limits.max_string_bytes, "string bytes")
    }
    fn quoted_with_limit(&mut self, max_bytes: usize, limit_name: &'static str) -> Result<String> {
        self.skip_space();
        let quote = self.byte()?;
        if !matches!(quote, b'\'' | b'"') {
            return Err(Error::InvalidValue("expected a quoted LLSD string".into()));
        }
        let mut output = Vec::new();
        loop {
            let byte = *self.input.get(self.offset).ok_or(Error::UnexpectedEof)?;
            self.offset += 1;
            if byte == quote {
                break;
            }
            if byte != b'\\' {
                output.push(byte);
                if output.len() > max_bytes {
                    return Err(Error::LimitExceeded(limit_name));
                }
                continue;
            }
            let escape = self.byte()?;
            match escape {
                b'\\' => output.push(b'\\'),
                b'\'' => output.push(b'\''),
                b'"' => output.push(b'"'),
                b'a' => output.push(0x07),
                b'n' => output.push(b'\n'),
                b'r' => output.push(b'\r'),
                b't' => output.push(b'\t'),
                b'v' => output.push(0x0b),
                b'b' => output.push(0x08),
                b'f' => output.push(0x0c),
                b'x' => output.push(self.hex_digits(2)? as u8),
                b'u' => push_codepoint(&mut output, self.hex_digits(4)?)?,
                b'U' => push_codepoint(&mut output, self.hex_digits(8)?)?,
                _ => return Err(Error::InvalidValue("invalid LLSD notation escape".into())),
            }
            if output.len() > max_bytes {
                return Err(Error::LimitExceeded(limit_name));
            }
        }
        if output.len() > max_bytes {
            return Err(Error::LimitExceeded(limit_name));
        }
        String::from_utf8(output).map_err(|_| Error::InvalidUtf8)
    }
    fn hex_digits(&mut self, count: usize) -> Result<u32> {
        let end = self.offset.checked_add(count).ok_or(Error::UnexpectedEof)?;
        let digits = self.input.get(self.offset..end).ok_or(Error::UnexpectedEof)?;
        self.offset = end;
        let digits = str::from_utf8(digits).map_err(|_| Error::InvalidUtf8)?;
        u32::from_str_radix(digits, 16).map_err(|_| Error::InvalidValue("invalid hexadecimal escape".into()))
    }
    fn typed_string(&mut self) -> Result<String> {
        self.skip_space();
        if self.input.get(self.offset) == Some(&b'(') {
            self.offset += 1;
            let count = self.decimal_length(self.limits.max_string_bytes, "string bytes")?;
            self.expect(b')')?;
            let quote = self.byte()?;
            if !matches!(quote, b'\'' | b'"') {
                return Err(Error::InvalidValue("expected a quoted LLSD string".into()));
            }
            let value = String::from_utf8(self.take(count)?.to_vec()).map_err(|_| Error::InvalidUtf8)?;
            self.expect_immediate(quote)?;
            Ok(value)
        } else {
            self.quoted()
        }
    }
    fn decimal_length(&mut self, limit: usize, name: &'static str) -> Result<usize> {
        let start = self.offset;
        while self.input.get(self.offset).is_some_and(u8::is_ascii_digit) {
            self.offset += 1;
        }
        if self.offset == start {
            return Err(Error::InvalidValue("expected a string length".into()));
        }
        let value = str::from_utf8(&self.input[start..self.offset])
            .map_err(|_| Error::InvalidUtf8)?
            .parse::<usize>()
            .map_err(|_| Error::InvalidValue("invalid LLSD string length".into()))?;
        if value > limit {
            return Err(Error::LimitExceeded(name));
        }
        Ok(value)
    }
    fn number_token(&mut self, typed: u8) -> Result<&'de str> {
        if self.byte()? != typed {
            return Err(Error::InvalidValue("invalid LLSD number type".into()));
        }
        str::from_utf8(self.token()?).map_err(|_| Error::InvalidUtf8)
    }
    fn parse_real(&mut self) -> Result<f64> {
        match self.number_token(b'r')?.to_ascii_lowercase().as_str() {
            "nan" => Ok(f64::NAN),
            "inf" | "+inf" | "infinity" | "+infinity" => Ok(f64::INFINITY),
            "-inf" | "-infinity" => Ok(f64::NEG_INFINITY),
            value => value.parse().map_err(|_| Error::InvalidValue("invalid LLSD real".into())),
        }
    }
    fn parse_binary(&mut self) -> Result<Vec<u8>> {
        self.skip_space();
        if self.input.get(self.offset..).is_some_and(|rest| rest.starts_with(b"b(")) {
            self.offset += 2;
            let length = self.decimal_length(self.limits.max_binary_bytes, "binary bytes")?;
            self.expect(b')')?;
            self.expect(b'"')?;
            let bytes = self.take(length)?.to_vec();
            if self.input.get(self.offset) != Some(&b'"') {
                return Err(Error::InvalidValue("length-prefixed LLSD binary has the wrong length".into()));
            }
            self.offset += 1;
            return Ok(bytes);
        }
        let prefix = self.input.get(self.offset..self.offset + 3).ok_or(Error::UnexpectedEof)?;
        let base16 = match prefix {
            b"b16" => true,
            b"b64" => false,
            _ => return Err(Error::InvalidValue("expected b16 or b64 LLSD binary".into())),
        };
        self.offset += 3;
        let max_encoded_len = if base16 {
            self.limits.max_binary_bytes.saturating_mul(2)
        } else {
            self.limits.max_binary_bytes.div_ceil(3).saturating_mul(4)
        };
        let value = self.quoted_with_limit(max_encoded_len, "binary bytes")?;
        let decoded_len = if base16 { value.len().div_ceil(2) } else { base64_decoded_len(value.as_bytes())? };
        if decoded_len > self.limits.max_binary_bytes {
            return Err(Error::LimitExceeded("binary bytes"));
        }
        if base16 {
            let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
            if !remainder.is_empty() {
                return Err(Error::InvalidValue("invalid b16 LLSD binary length".into()));
            }
            pairs
                .iter()
                .map(|pair| {
                    let high = hex_nibble(pair[0]).ok_or_else(|| Error::InvalidValue("invalid b16 LLSD binary".into()))?;
                    let low = hex_nibble(pair[1]).ok_or_else(|| Error::InvalidValue("invalid b16 LLSD binary".into()))?;
                    Ok(high << 4 | low)
                })
                .collect()
        } else {
            STANDARD.decode(value).map_err(|error| Error::InvalidValue(error.to_string()))
        }
    }
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn push_codepoint(output: &mut Vec<u8>, value: u32) -> Result<()> {
    let character = char::from_u32(value).ok_or_else(|| Error::InvalidValue("invalid Unicode escape".into()))?;
    let mut bytes = [0; 4];
    output.extend_from_slice(character.encode_utf8(&mut bytes).as_bytes());
    Ok(())
}

impl<'de> de::Deserializer<'de> for &mut Deserializer<'de> {
    type Error = Error;
    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        match self.peek()? {
            b'!' => {
                self.offset += 1;
                visitor.visit_unit()
            }
            b'0' | b'1' | b't' | b'T' | b'f' | b'F' => {
                let token = self.token()?;
                match token {
                    b"1" | b"t" | b"T" | b"true" | b"TRUE" => visitor.visit_bool(true),
                    b"0" | b"f" | b"F" | b"false" | b"FALSE" => visitor.visit_bool(false),
                    _ => Err(Error::InvalidValue("invalid LLSD boolean".into())),
                }
            }
            b'i' => {
                let text = self.number_token(b'i')?;
                let value = text.parse::<i32>().map_err(|_| Error::InvalidValue("LLSD integer is outside the i32 range".into()))?;
                visitor.visit_i32(value)
            }
            b'r' => visitor.visit_f64(self.parse_real()?),
            b'u' => {
                self.offset += 1;
                visitor.visit_string(str::from_utf8(self.token()?).map_err(|_| Error::InvalidUtf8)?.to_owned())
            }
            b'd' => {
                self.offset += 1;
                let text = self.quoted()?;
                let date = Date::parse(&text)?;
                visitor.visit_f64(date.unix_seconds())
            }
            b'l' => {
                self.offset += 1;
                visitor.visit_string(self.typed_string()?)
            }
            b'b' => visitor.visit_byte_buf(self.parse_binary()?),
            b'\'' | b'"' => visitor.visit_string(self.quoted()?),
            b's' => {
                self.offset += 1;
                visitor.visit_string(self.typed_string()?)
            }
            b'[' => {
                self.offset += 1;
                self.container(|de| {
                    let mut access = NotationSeqAccess { de, first: true, ended: false, count: 0 };
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
                self.offset += 1;
                self.container(|de| {
                    let mut access = NotationMapAccess { de, first: true, expects_value: false, ended: false, count: 0 };
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
            _ => Err(Error::InvalidValue("unknown LLSD notation value".into())),
        }
    }
    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string
    }
    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        let bytes = self.parse_binary()?;
        visitor.visit_bytes(&bytes)
    }
    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        visitor.visit_byte_buf(self.parse_binary()?)
    }
    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        if self.peek()? == b'!' {
            self.offset += 1;
            self.count_value()?;
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }
    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        if self.byte()? != b'!' {
            return Err(Error::InvalidValue("expected LLSD undefined".into()));
        }
        visitor.visit_unit()
    }
    fn deserialize_unit_struct<V: Visitor<'de>>(self, _: &'static str, v: V) -> Result<V::Value> {
        self.deserialize_unit(v)
    }
    fn deserialize_newtype_struct<V: Visitor<'de>>(self, name: &'static str, visitor: V) -> Result<V::Value> {
        match name {
            UUID_NEWTYPE => {
                self.count_value()?;
                if self.byte()? != b'u' {
                    return Err(Error::InvalidValue("expected LLSD UUID".into()));
                }
                visitor.visit_newtype_struct(StringDeserializer::<Error>::new(
                    str::from_utf8(self.token()?).map_err(|_| Error::InvalidUtf8)?.to_owned(),
                ))
            }
            URI_NEWTYPE => {
                self.count_value()?;
                if self.byte()? != b'l' {
                    return Err(Error::InvalidValue("expected LLSD URI".into()));
                }
                visitor.visit_newtype_struct(StringDeserializer::<Error>::new(self.typed_string()?))
            }
            DATE_NEWTYPE => {
                self.count_value()?;
                if self.byte()? != b'd' {
                    return Err(Error::InvalidValue("expected LLSD date".into()));
                }
                let text = self.quoted()?;
                visitor.visit_newtype_struct(StringDeserializer::<Error>::new(text))
            }
            BINARY_NEWTYPE => {
                self.count_value()?;
                let value = self.parse_binary()?;
                visitor.visit_newtype_struct(BytesDeserializer::<Error>::new(&value))
            }
            _ => visitor.visit_newtype_struct(self),
        }
    }
    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_tuple<V: Visitor<'de>>(self, _: usize, visitor: V) -> Result<V::Value> {
        self.deserialize_seq(visitor)
    }
    fn deserialize_tuple_struct<V: Visitor<'de>>(self, _: &'static str, _: usize, visitor: V) -> Result<V::Value> {
        self.deserialize_seq(visitor)
    }
    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_struct<V: Visitor<'de>>(self, _: &'static str, _: &'static [&'static str], visitor: V) -> Result<V::Value> {
        self.deserialize_map(visitor)
    }
    fn deserialize_enum<V: Visitor<'de>>(self, name: &'static str, _: &'static [&'static str], visitor: V) -> Result<V::Value> {
        if name == VALUE_ENUM {
            let variant = match self.peek()? {
                b'!' => "Undefined",
                b'0' | b'1' | b't' | b'T' | b'f' | b'F' => "Boolean",
                b'i' => "Integer",
                b'r' => "Real",
                b'u' => "UUID",
                b'\'' | b'"' | b's' => "String",
                b'd' => "Date",
                b'l' => "URI",
                b'b' => "Binary",
                b'[' => "Array",
                b'{' => "Map",
                _ => return Err(Error::InvalidValue("unknown LLSD notation value".into())),
            };
            visitor.visit_enum(NotationDynamicEnumAccess { de: self, variant })
        } else {
            self.deserialize_plain_enum(visitor)
        }
    }
    fn deserialize_identifier<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_ignored_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(IgnoredVisitor)?;
        v.visit_unit()
    }
    fn is_human_readable(&self) -> bool {
        true
    }
}

struct NotationSeqAccess<'a, 'de> {
    de: &'a mut Deserializer<'de>,
    first: bool,
    ended: bool,
    count: usize,
}
impl NotationSeqAccess<'_, '_> {
    fn finish(&mut self) -> Result<()> {
        if self.ended {
            return Ok(());
        }
        self.de.skip_space();
        self.de.expect(b']')?;
        self.ended = true;
        Ok(())
    }
}
impl<'de> SeqAccess<'de> for NotationSeqAccess<'_, 'de> {
    type Error = Error;
    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        self.de.skip_space();
        if self.de.input.get(self.de.offset) == Some(&b']') {
            self.de.offset += 1;
            self.ended = true;
            return Ok(None);
        }
        if !self.first {
            self.de.expect(b',')?;
            self.de.skip_space();
        }
        self.first = false;
        self.count += 1;
        if self.count > self.de.limits.max_container_items {
            return Err(Error::LimitExceeded("container items"));
        }
        seed.deserialize(&mut *self.de).map(Some)
    }
    fn size_hint(&self) -> Option<usize> {
        None
    }
}

struct NotationMapAccess<'a, 'de> {
    de: &'a mut Deserializer<'de>,
    first: bool,
    expects_value: bool,
    ended: bool,
    count: usize,
}
impl NotationMapAccess<'_, '_> {
    fn finish(&mut self) -> Result<()> {
        if self.ended {
            return Ok(());
        }
        if self.expects_value {
            return Err(Error::InvalidValue("LLSD map is missing a value".into()));
        }
        self.de.skip_space();
        self.de.expect(b'}')?;
        self.ended = true;
        Ok(())
    }
}
impl<'de> MapAccess<'de> for NotationMapAccess<'_, 'de> {
    type Error = Error;
    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        self.de.skip_space();
        if self.expects_value {
            return Err(Error::InvalidValue("LLSD map value was not read".into()));
        }
        if self.de.input.get(self.de.offset) == Some(&b'}') {
            self.de.offset += 1;
            self.ended = true;
            return Ok(None);
        }
        if !self.first {
            self.de.expect(b',')?;
        }
        self.first = false;
        let key = match self.de.peek()? {
            b'\'' | b'"' => self.de.quoted()?,
            b's' => {
                self.de.offset += 1;
                self.de.typed_string()?
            }
            _ => return Err(Error::InvalidValue("LLSD map key must be a string".into())),
        };
        if key.len() > self.de.limits.max_map_key_bytes {
            return Err(Error::LimitExceeded("map key bytes"));
        }
        self.de.skip_space();
        self.de.expect(b':')?;
        self.expects_value = true;
        self.count += 1;
        if self.count > self.de.limits.max_container_items {
            return Err(Error::LimitExceeded("container items"));
        }
        seed.deserialize(StringDeserializer::<Error>::new(key)).map(Some)
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        if !self.expects_value {
            return Err(Error::InvalidValue("LLSD map key is missing".into()));
        }
        self.expects_value = false;
        seed.deserialize(&mut *self.de)
    }
    fn size_hint(&self) -> Option<usize> {
        None
    }
}

struct NotationDynamicEnumAccess<'a, 'de> {
    de: &'a mut Deserializer<'de>,
    variant: &'static str,
}

struct NotationSerdeEnumAccess<'a, 'de> {
    de: &'a mut Deserializer<'de>,
    variant: String,
}

impl<'a, 'de> EnumAccess<'de> for NotationSerdeEnumAccess<'a, 'de> {
    type Error = Error;
    type Variant = NotationSerdeVariantAccess<'a, 'de>;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant)> {
        let variant = seed.deserialize(StringDeserializer::<Error>::new(self.variant))?;
        Ok((variant, NotationSerdeVariantAccess { de: self.de }))
    }
}

struct NotationSerdeVariantAccess<'a, 'de> {
    de: &'a mut Deserializer<'de>,
}

impl NotationSerdeVariantAccess<'_, '_> {
    fn finish(self) -> Result<()> {
        self.de.skip_space();
        self.de.expect(b'}')
    }
}

impl<'de> VariantAccess<'de> for NotationSerdeVariantAccess<'_, 'de> {
    type Error = Error;

    fn unit_variant(self) -> Result<()> {
        self.de.count_value()?;
        if self.de.byte()? != b'!' {
            return Err(Error::InvalidValue("expected LLSD undefined".into()));
        }
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
impl<'a, 'de> EnumAccess<'de> for NotationDynamicEnumAccess<'a, 'de> {
    type Error = Error;
    type Variant = NotationDynamicVariantAccess<'a, 'de>;
    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant)> {
        let variant = seed.deserialize(StringDeserializer::<Error>::new(self.variant.to_owned()))?;
        Ok((variant, NotationDynamicVariantAccess { de: self.de }))
    }
}
struct NotationDynamicVariantAccess<'a, 'de> {
    de: &'a mut Deserializer<'de>,
}
impl<'de> VariantAccess<'de> for NotationDynamicVariantAccess<'_, 'de> {
    type Error = Error;
    fn unit_variant(self) -> Result<()> {
        self.de.count_value()?;
        if self.de.byte()? == b'!' { Ok(()) } else { Err(Error::InvalidValue("expected LLSD undefined".into())) }
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

struct IgnoredVisitor;
impl<'de> Visitor<'de> for IgnoredVisitor {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("any LLSD value")
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_i32<E: de::Error>(self, _: i32) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_str<E: de::Error>(self, _: &str) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_string<E: de::Error>(self, _: String) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_bytes<E: de::Error>(self, _: &[u8]) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_byte_buf<E: de::Error>(self, _: Vec<u8>) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<(), A::Error> {
        while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<(), A::Error> {
        while map.next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?.is_some() {}
        Ok(())
    }
}
