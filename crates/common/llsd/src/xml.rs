//! XML codec for LLSD values and Serde types.
//!
//! Decoders apply [`crate::Limits`]. Encoders write an XML declaration and one `<llsd>` root.
//! Binary elements accept omitted encoding declarations or `encoding="base64"`.

use std::{
    io::{Read, Write},
    str,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use quick_xml::{Reader, events::Event};
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor,
    value::{BytesDeserializer, StringDeserializer},
};

use crate::{
    Error, Limits, Result,
    ser::{self, Format},
    value::{BINARY_NEWTYPE, DATE_NEWTYPE, URI_NEWTYPE, UUID_NEWTYPE, VALUE_ENUM},
};

#[derive(Debug)]
enum Token {
    Start(String),
    Empty(String),
    End(String),
    Text(String),
}

/// Decodes one complete XML LLSD document from UTF-8 bytes using default limits.
///
/// Accepts one `<llsd>` root and trailing whitespace only.
pub fn from_slice<T: DeserializeOwned>(input: &[u8]) -> Result<T> {
    from_slice_with_limits(input, Limits::default())
}

/// Decodes one XML LLSD value from UTF-8 bytes with caller-supplied limits.
pub fn from_slice_with_limits<T: DeserializeOwned>(input: &[u8], limits: Limits) -> Result<T> {
    if input.len() > limits.max_input_bytes {
        return Err(Error::LimitExceeded("input bytes"));
    }
    let tokens = tokenize(input, limits)?;
    let mut de = Deserializer::new(tokens, limits)?;
    let value = T::deserialize(&mut de)?;
    de.finish()?;
    Ok(value)
}

/// Decodes one XML LLSD value from a string with the default limits.
pub fn from_str<T: DeserializeOwned>(input: &str) -> Result<T> {
    from_slice(input.as_bytes())
}

/// Reads and decodes one XML LLSD value with the default limits.
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

/// Encodes a Serde value as one complete XML LLSD document.
pub fn to_vec<T: serde::Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    ser::to_vec(value, Format::Xml)
}

/// Encodes a value as XML with a complete-output byte limit and the default LLSD nesting limit.
///
/// The byte limit includes the XML declaration and the root `<llsd>` element.
/// The encoder checks the limit while serializing, before it builds the complete document.
///
pub fn to_vec_with_max_bytes<T: serde::Serialize + ?Sized>(value: &T, max_bytes: usize) -> Result<Vec<u8>> {
    ser::to_vec_with_max_bytes(value, max_bytes)
}

/// Encodes a Serde value as an XML LLSD string.
pub fn to_string<T: serde::Serialize + ?Sized>(value: &T) -> Result<String> {
    String::from_utf8(to_vec(value)?).map_err(|_| Error::InvalidUtf8)
}

/// Encodes one complete XML LLSD document and writes it to `writer`.
pub fn to_writer<T: serde::Serialize + ?Sized, W: Write>(mut writer: W, value: &T) -> Result<()> {
    ser::to_writer(&mut writer, value, Format::Xml)
}

fn tokenize(input: &[u8], limits: Limits) -> Result<Vec<Token>> {
    let text = str::from_utf8(input).map_err(|_| Error::InvalidUtf8)?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut tokens = Vec::new();
    let mut element_context = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                let name = tag_name(element.name().as_ref())?;
                if name == "binary" {
                    validate_binary_encoding(&element, reader.decoder())?;
                }
                element_context.push((name == "binary", !matches!(name.as_str(), "llsd" | "array" | "map")));
                push_token(&mut tokens, Token::Start(name), limits)?;
            }
            Ok(Event::Empty(element)) => {
                let name = tag_name(element.name().as_ref())?;
                if name == "binary" {
                    validate_binary_encoding(&element, reader.decoder())?;
                }
                push_token(&mut tokens, Token::Empty(name), limits)?;
            }
            Ok(Event::End(element)) => {
                element_context.pop();
                push_token(&mut tokens, Token::End(tag_name(element.name().as_ref())?), limits)?;
            }
            Ok(Event::Text(text)) => {
                let decoded = text.decode().map_err(|error| Error::InvalidValue(error.to_string()))?;
                let value = quick_xml::escape::unescape(&decoded).map_err(|error| Error::InvalidValue(error.to_string()))?.into_owned();
                push_text(&mut tokens, value, &element_context, limits)?;
            }
            Ok(Event::GeneralRef(reference)) => {
                let reference = reference.decode().map_err(|error| Error::InvalidValue(error.to_string()))?;
                let reference = format!("&{reference};");
                let value = quick_xml::escape::unescape(&reference).map_err(|error| Error::InvalidValue(error.to_string()))?.into_owned();
                push_text(&mut tokens, value, &element_context, limits)?;
            }
            Ok(Event::CData(text)) => {
                let value = text.decode().map_err(|error| Error::InvalidValue(error.to_string()))?.into_owned();
                push_text(&mut tokens, value, &element_context, limits)?;
            }
            Ok(Event::DocType(_)) => return Err(Error::InvalidValue("XML document types are not allowed in LLSD".into())),
            Ok(Event::Eof) => break,
            Ok(Event::Decl(_) | Event::PI(_) | Event::Comment(_)) => {}
            Err(error) => return Err(Error::InvalidValue(error.to_string())),
        }
    }
    Ok(tokens)
}

fn push_token(tokens: &mut Vec<Token>, token: Token, limits: Limits) -> Result<()> {
    let max_tokens = limits.max_values.saturating_mul(8).saturating_add(16);
    if tokens.len() >= max_tokens {
        return Err(Error::LimitExceeded("XML token count"));
    }
    tokens.push(token);
    Ok(())
}

fn push_text(tokens: &mut Vec<Token>, value: String, element_context: &[(bool, bool)], limits: Limits) -> Result<()> {
    let binary = element_context.last().is_some_and(|(binary, _)| *binary);
    let preserve_whitespace = element_context.last().is_some_and(|(_, preserve)| *preserve);
    if !preserve_whitespace && value.bytes().all(|byte| byte.is_ascii_whitespace()) {
        return Ok(());
    }

    if !binary && value.len() > limits.max_string_bytes {
        return Err(Error::LimitExceeded("string bytes"));
    }
    if let Some(Token::Text(existing)) = tokens.last_mut() {
        let length = existing.len().checked_add(value.len()).ok_or(Error::LimitExceeded("string bytes"))?;
        if !binary && length > limits.max_string_bytes {
            return Err(Error::LimitExceeded("string bytes"));
        }
        existing.push_str(&value);
    } else {
        push_token(tokens, Token::Text(value), limits)?;
    }
    Ok(())
}

fn tag_name(name: &[u8]) -> Result<String> {
    str::from_utf8(name).map(str::to_owned).map_err(|_| Error::InvalidUtf8)
}

fn validate_binary_encoding(element: &quick_xml::events::BytesStart<'_>, decoder: quick_xml::encoding::Decoder) -> Result<()> {
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|error| Error::InvalidValue(error.to_string()))?;
        if attribute.key.as_ref() != b"encoding" {
            continue;
        }

        let encoding = attribute
            .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, decoder)
            .map_err(|error| Error::InvalidValue(error.to_string()))?;
        if encoding != "base64" {
            return Err(Error::InvalidValue(format!("unsupported LLSD binary encoding {encoding:?}")));
        }
    }
    Ok(())
}

struct Deserializer {
    tokens: Vec<Token>,
    index: usize,
    limits: Limits,
    values: usize,
    depth: usize,
}

impl Deserializer {
    fn new(tokens: Vec<Token>, limits: Limits) -> Result<Self> {
        let mut index = 0;
        while matches!(tokens.get(index), Some(Token::Text(value)) if value.trim().is_empty()) {
            index += 1;
        }
        if !matches!(tokens.get(index), Some(Token::Start(tag)) if tag == "llsd") {
            return Err(Error::InvalidValue("expected an LLSD XML root element".into()));
        }
        Ok(Self { tokens, index: index + 1, limits, values: 0, depth: 0 })
    }

    fn finish(&mut self) -> Result<()> {
        self.skip_space();
        if !matches!(self.tokens.get(self.index), Some(Token::End(tag)) if tag == "llsd") {
            return Err(Error::InvalidValue("LLSD XML root element was not closed".into()));
        }
        self.index += 1;
        self.skip_space();
        if self.index != self.tokens.len() {
            return Err(Error::TrailingData);
        }
        Ok(())
    }

    fn skip_space(&mut self) {
        while matches!(self.tokens.get(self.index), Some(Token::Text(value)) if value.trim().is_empty()) {
            self.index += 1;
        }
    }

    fn next_tag(&mut self) -> Result<(String, bool)> {
        self.skip_space();
        match self.tokens.get(self.index) {
            Some(Token::Start(tag)) => {
                let tag = tag.clone();
                self.index += 1;
                Ok((tag, false))
            }
            Some(Token::Empty(tag)) => {
                let tag = tag.clone();
                self.index += 1;
                Ok((tag, true))
            }
            Some(Token::End(_)) => Err(Error::UnexpectedEof),
            _ => Err(Error::InvalidValue("expected an LLSD XML value".into())),
        }
    }

    fn text_value(&mut self, tag: &str, empty: bool) -> Result<String> {
        self.text_value_with_limit(tag, empty, self.limits.max_string_bytes, "string bytes")
    }

    fn text_value_with_limit(&mut self, tag: &str, empty: bool, limit: usize, limit_name: &'static str) -> Result<String> {
        if empty {
            return Ok(String::new());
        }
        let mut value = String::new();
        loop {
            match self.tokens.get(self.index) {
                Some(Token::Text(text)) => {
                    value.push_str(text);
                    self.index += 1;
                }
                Some(Token::End(end)) if end == tag => {
                    self.index += 1;
                    break;
                }
                Some(Token::Start(_)) | Some(Token::Empty(_)) => return Err(Error::InvalidValue("nested elements in LLSD scalar".into())),
                Some(Token::End(_)) => return Err(Error::InvalidValue("mismatched LLSD XML element".into())),
                None => return Err(Error::UnexpectedEof),
            }
            if value.len() > limit {
                return Err(Error::LimitExceeded(limit_name));
            }
        }
        Ok(value)
    }

    fn count_value(&mut self) -> Result<()> {
        self.values = self.values.checked_add(1).ok_or(Error::LimitExceeded("value count"))?;
        if self.values > self.limits.max_values {
            return Err(Error::LimitExceeded("value count"));
        }
        Ok(())
    }

    fn container<T>(&mut self, parse: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.depth = self.depth.checked_add(1).ok_or(Error::LimitExceeded("nesting depth"))?;
        if self.depth > self.limits.max_depth {
            self.depth -= 1;
            return Err(Error::LimitExceeded("nesting depth"));
        }
        let result = parse(self);
        self.depth -= 1;
        result
    }

    fn scalar_for_wrapper(&mut self, expected: &str) -> Result<String> {
        self.count_value()?;
        let (tag, empty) = self.next_tag()?;
        if tag != expected {
            return Err(Error::InvalidValue(format!("expected LLSD {expected}")));
        }
        let default = match expected {
            "uuid" => "00000000-0000-0000-0000-000000000000",
            "date" => "1970-01-01T00:00:00Z",
            _ => "",
        };
        let text = self.text_value(&tag, empty)?;
        if text.is_empty() { Ok(default.to_owned()) } else { Ok(text) }
    }

    fn deserialize_plain_enum<'de, V: Visitor<'de>>(&mut self, visitor: V) -> Result<V::Value> {
        self.skip_space();
        match self.tokens.get(self.index) {
            Some(Token::Empty(tag) | Token::Start(tag)) if tag == "string" => {
                let variant = self.scalar_for_wrapper("string")?;
                visitor.visit_enum(StringDeserializer::<Error>::new(variant))
            }
            Some(Token::Start(tag)) if tag == "map" => {
                self.count_value()?;
                let (tag, empty) = self.next_tag()?;
                if tag != "map" || empty {
                    return Err(Error::InvalidValue("LLSD enum map must contain exactly one entry".into()));
                }
                self.container(|de| {
                    de.skip_space();
                    let (key_tag, key_empty) = de.next_tag()?;
                    if key_tag != "key" {
                        return Err(Error::InvalidValue("expected LLSD enum map key".into()));
                    }
                    let variant = de.text_value(&key_tag, key_empty)?;
                    if variant.len() > de.limits.max_map_key_bytes {
                        return Err(Error::LimitExceeded("map key bytes"));
                    }
                    if de.limits.max_container_items == 0 {
                        return Err(Error::LimitExceeded("container items"));
                    }
                    visitor.visit_enum(XmlSerdeEnumAccess { de, variant })
                })
            }
            _ => Err(Error::InvalidValue("expected an LLSD enum string or single-entry map".into())),
        }
    }

    fn parse_binary(&mut self, tag: &str, empty: bool) -> Result<Vec<u8>> {
        let text = self.text_value_with_limit(tag, empty, self.limits.max_input_bytes, "input bytes")?;
        let max_encoded_len = self.limits.max_binary_bytes.div_ceil(3).saturating_mul(4);
        let mut compact = Vec::with_capacity(text.len().min(max_encoded_len));
        for byte in text.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
            if compact.len() == max_encoded_len {
                return Err(Error::LimitExceeded("binary bytes"));
            }
            compact.push(byte);
        }
        if crate::base64_decoded_len(&compact)? > self.limits.max_binary_bytes {
            return Err(Error::LimitExceeded("binary bytes"));
        }
        STANDARD.decode(compact).map_err(|error| Error::InvalidValue(error.to_string()))
    }
}

impl<'de> de::Deserializer<'de> for &mut Deserializer {
    type Error = Error;
    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        let (tag, empty) = self.next_tag()?;
        match tag.as_str() {
            "undef" => {
                self.text_value(&tag, empty)?;
                visitor.visit_unit()
            }
            "boolean" => {
                let value = self.text_value(&tag, empty)?;
                match value.trim() {
                    "" | "0" | "false" => visitor.visit_bool(false),
                    "1" | "true" => visitor.visit_bool(true),
                    _ => Err(Error::InvalidValue("invalid LLSD boolean".into())),
                }
            }
            "integer" => {
                let value = self.text_value(&tag, empty)?;
                let number = if value.trim().is_empty() {
                    0
                } else {
                    value.trim().parse::<i32>().map_err(|_| Error::InvalidValue("LLSD integer is outside the i32 range".into()))?
                };
                visitor.visit_i32(number)
            }
            "real" => {
                let value = self.text_value(&tag, empty)?;
                let number = if value.trim().is_empty() { 0.0 } else { parse_real(value.trim())? };
                visitor.visit_f64(number)
            }
            "uuid" | "string" | "uri" | "date" => {
                let default = match tag.as_str() {
                    "uuid" => "00000000-0000-0000-0000-000000000000",
                    "date" => "1970-01-01T00:00:00Z",
                    _ => "",
                };
                let text = self.text_value(&tag, empty)?;
                visitor.visit_string(if text.is_empty() { default.to_owned() } else { text })
            }
            "binary" => visitor.visit_byte_buf(self.parse_binary(&tag, empty)?),
            "array" => {
                if empty {
                    return visitor.visit_seq(EmptySeqAccess);
                }
                self.container(|de| {
                    let mut access = XmlSeqAccess { de, ended: false, count: 0 };
                    let result = visitor.visit_seq(&mut access);
                    match result {
                        Ok(value) => {
                            access.finish("array")?;
                            Ok(value)
                        }
                        Err(error) => Err(error),
                    }
                })
            }
            "map" => {
                if empty {
                    return visitor.visit_map(EmptyMapAccess);
                }
                self.container(|de| {
                    let mut access = XmlMapAccess { de, ended: false, expects_value: false, count: 0 };
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
            _ => Err(Error::InvalidValue(format!("unknown LLSD XML element <{tag}>"))),
        }
    }
    fn deserialize_bool<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_i8<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_i16<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_i32<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_i64<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_u8<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_u16<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_u32<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_u64<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_f32<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_f64<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_char<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_str<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_string<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        let (tag, empty) = self.next_tag()?;
        if tag != "binary" {
            return Err(Error::InvalidValue("expected LLSD binary".into()));
        }
        let bytes = self.parse_binary(&tag, empty)?;
        visitor.visit_bytes(&bytes)
    }
    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        let (tag, empty) = self.next_tag()?;
        if tag != "binary" {
            return Err(Error::InvalidValue("expected LLSD binary".into()));
        }
        visitor.visit_byte_buf(self.parse_binary(&tag, empty)?)
    }
    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        if matches!(self.tokens.get(self.index), Some(Token::Empty(tag) | Token::Start(tag)) if tag == "undef") {
            self.count_value()?;
            let (tag, empty) = self.next_tag()?;
            self.text_value(&tag, empty)?;
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }
    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        let (tag, empty) = self.next_tag()?;
        if tag != "undef" {
            return Err(Error::InvalidValue("expected LLSD undefined".into()));
        }
        self.text_value(&tag, empty)?;
        visitor.visit_unit()
    }
    fn deserialize_unit_struct<V: Visitor<'de>>(self, _: &'static str, v: V) -> Result<V::Value> {
        self.deserialize_unit(v)
    }
    fn deserialize_newtype_struct<V: Visitor<'de>>(self, name: &'static str, visitor: V) -> Result<V::Value> {
        match name {
            UUID_NEWTYPE => visitor.visit_newtype_struct(StringDeserializer::<Error>::new(self.scalar_for_wrapper("uuid")?)),
            URI_NEWTYPE => visitor.visit_newtype_struct(StringDeserializer::<Error>::new(self.scalar_for_wrapper("uri")?)),
            DATE_NEWTYPE => {
                let text = self.scalar_for_wrapper("date")?;
                crate::Date::parse(&text)?;
                visitor.visit_newtype_struct(StringDeserializer::<Error>::new(text))
            }
            BINARY_NEWTYPE => {
                self.count_value()?;
                let (tag, empty) = self.next_tag()?;
                if tag != "binary" {
                    return Err(Error::InvalidValue("expected LLSD binary".into()));
                }
                let bytes = self.parse_binary(&tag, empty)?;
                visitor.visit_newtype_struct(BytesDeserializer::<Error>::new(&bytes))
            }
            _ => visitor.visit_newtype_struct(self),
        }
    }
    fn deserialize_seq<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_tuple<V: Visitor<'de>>(self, _: usize, v: V) -> Result<V::Value> {
        self.deserialize_seq(v)
    }
    fn deserialize_tuple_struct<V: Visitor<'de>>(self, _: &'static str, _: usize, v: V) -> Result<V::Value> {
        self.deserialize_seq(v)
    }
    fn deserialize_map<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_struct<V: Visitor<'de>>(self, _: &'static str, _: &'static [&'static str], v: V) -> Result<V::Value> {
        self.deserialize_map(v)
    }
    fn deserialize_enum<V: Visitor<'de>>(self, name: &'static str, _: &'static [&'static str], visitor: V) -> Result<V::Value> {
        self.skip_space();
        if name == VALUE_ENUM {
            let variant = match self.tokens.get(self.index) {
                Some(Token::Empty(tag) | Token::Start(tag)) => match tag.as_str() {
                    "undef" => "Undefined",
                    "boolean" => "Boolean",
                    "integer" => "Integer",
                    "real" => "Real",
                    "uuid" => "UUID",
                    "string" => "String",
                    "date" => "Date",
                    "uri" => "URI",
                    "binary" => "Binary",
                    "array" => "Array",
                    "map" => "Map",
                    _ => return Err(Error::InvalidValue("unknown LLSD XML element".into())),
                },
                _ => return Err(Error::UnexpectedEof),
            };
            visitor.visit_enum(XmlDynamicEnumAccess { de: self, variant })
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

struct EmptySeqAccess;
impl<'de> SeqAccess<'de> for EmptySeqAccess {
    type Error = Error;
    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, _: T) -> Result<Option<T::Value>> {
        Ok(None)
    }
    fn size_hint(&self) -> Option<usize> {
        Some(0)
    }
}
struct EmptyMapAccess;
impl<'de> MapAccess<'de> for EmptyMapAccess {
    type Error = Error;
    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, _: K) -> Result<Option<K::Value>> {
        Ok(None)
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, _: V) -> Result<V::Value> {
        Err(Error::UnexpectedEof)
    }
    fn size_hint(&self) -> Option<usize> {
        Some(0)
    }
}

struct XmlSeqAccess<'a> {
    de: &'a mut Deserializer,
    ended: bool,
    count: usize,
}
impl XmlSeqAccess<'_> {
    fn finish(&mut self, tag: &str) -> Result<()> {
        if self.ended {
            return Ok(());
        }
        self.de.skip_space();
        match self.de.tokens.get(self.de.index) {
            Some(Token::End(end)) if end == tag => {
                self.de.index += 1;
                self.ended = true;
                Ok(())
            }
            _ => Err(Error::InvalidValue("LLSD array was not fully read".into())),
        }
    }
}
impl<'de> SeqAccess<'de> for XmlSeqAccess<'_> {
    type Error = Error;
    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        self.de.skip_space();
        if matches!(self.de.tokens.get(self.de.index), Some(Token::End(tag)) if tag == "array") {
            self.de.index += 1;
            self.ended = true;
            return Ok(None);
        }
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

struct XmlMapAccess<'a> {
    de: &'a mut Deserializer,
    ended: bool,
    expects_value: bool,
    count: usize,
}
impl XmlMapAccess<'_> {
    fn finish(&mut self) -> Result<()> {
        if self.ended {
            return Ok(());
        }
        if self.expects_value {
            return Err(Error::InvalidValue("LLSD map is missing a value".into()));
        }
        self.de.skip_space();
        match self.de.tokens.get(self.de.index) {
            Some(Token::End(tag)) if tag == "map" => {
                self.de.index += 1;
                self.ended = true;
                Ok(())
            }
            token => Err(Error::InvalidValue(format!("LLSD map was not fully read at {:?}", token))),
        }
    }
}
impl<'de> MapAccess<'de> for XmlMapAccess<'_> {
    type Error = Error;
    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        self.de.skip_space();
        if self.expects_value {
            return Err(Error::InvalidValue("LLSD map value was not read".into()));
        }
        if matches!(self.de.tokens.get(self.de.index), Some(Token::End(tag)) if tag == "map") {
            self.de.index += 1;
            self.ended = true;
            return Ok(None);
        }
        self.count += 1;
        if self.count > self.de.limits.max_container_items {
            return Err(Error::LimitExceeded("container items"));
        }
        let (tag, empty) = self.de.next_tag()?;
        if tag != "key" {
            return Err(Error::InvalidValue("expected LLSD map key".into()));
        }
        let key = self.de.text_value(&tag, empty)?;
        if key.len() > self.de.limits.max_map_key_bytes {
            return Err(Error::LimitExceeded("map key bytes"));
        }
        self.expects_value = true;
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

struct XmlDynamicEnumAccess<'a> {
    de: &'a mut Deserializer,
    variant: &'static str,
}
impl<'a, 'de> EnumAccess<'de> for XmlDynamicEnumAccess<'a> {
    type Error = Error;
    type Variant = XmlDynamicVariantAccess<'a>;
    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant)> {
        let variant = seed.deserialize(StringDeserializer::<Error>::new(self.variant.to_owned()))?;
        Ok((variant, XmlDynamicVariantAccess { de: self.de }))
    }
}

struct XmlSerdeEnumAccess<'a> {
    de: &'a mut Deserializer,
    variant: String,
}

impl<'a, 'de> EnumAccess<'de> for XmlSerdeEnumAccess<'a> {
    type Error = Error;
    type Variant = XmlSerdeVariantAccess<'a>;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant)> {
        let variant = seed.deserialize(StringDeserializer::<Error>::new(self.variant))?;
        Ok((variant, XmlSerdeVariantAccess { de: self.de }))
    }
}

struct XmlSerdeVariantAccess<'a> {
    de: &'a mut Deserializer,
}

impl XmlSerdeVariantAccess<'_> {
    fn finish(self) -> Result<()> {
        self.de.skip_space();
        match self.de.tokens.get(self.de.index) {
            Some(Token::End(tag)) if tag == "map" => {
                self.de.index += 1;
                Ok(())
            }
            _ => Err(Error::InvalidValue("LLSD enum map must contain exactly one entry".into())),
        }
    }
}

impl<'de> VariantAccess<'de> for XmlSerdeVariantAccess<'_> {
    type Error = Error;

    fn unit_variant(self) -> Result<()> {
        self.de.count_value()?;
        let (tag, empty) = self.de.next_tag()?;
        if tag != "undef" {
            return Err(Error::InvalidValue("expected LLSD undefined".into()));
        }
        self.de.text_value(&tag, empty)?;
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

struct XmlDynamicVariantAccess<'a> {
    de: &'a mut Deserializer,
}
impl<'de> VariantAccess<'de> for XmlDynamicVariantAccess<'_> {
    type Error = Error;
    fn unit_variant(self) -> Result<()> {
        self.de.count_value()?;
        let (tag, empty) = self.de.next_tag()?;
        if tag != "undef" {
            return Err(Error::InvalidValue("expected LLSD undefined".into()));
        }
        self.de.text_value(&tag, empty).map(|_| ())
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

fn parse_real(value: &str) -> Result<f64> {
    match value.to_ascii_lowercase().as_str() {
        "nan" => Ok(f64::NAN),
        "inf" | "+inf" | "infinity" | "+infinity" => Ok(f64::INFINITY),
        "-inf" | "-infinity" => Ok(f64::NEG_INFINITY),
        _ => value.parse().map_err(|_| Error::InvalidValue("invalid LLSD real".into())),
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
