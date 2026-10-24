use base64::{Engine, engine::general_purpose::STANDARD};
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor,
    value::{BytesDeserializer, StringDeserializer},
};
use serde::forward_to_deserialize_any;

use crate::{
    Date, Error, Limits, Result,
    decode::{Budget, VariantAccess, VariantEnumAccess, Wrapper, no_finish},
    encoding::{base64_decoded_len, parse_real},
    value::VALUE_ENUM,
};

use super::{
    normalized_text_len,
    tokens::{Token, Tokens},
};

struct Deserializer<'input> {
    tokens: Tokens<'input>,
    budget: Budget,
}

impl<'input> Deserializer<'input> {
    fn new(tokens: Tokens<'input>, limits: Limits) -> Result<Self> {
        let mut de = Self { tokens, budget: Budget::new(limits) };
        de.skip_space()?;
        if !matches!(de.tokens.take()?, Some(Token::Start(tag)) if tag == "llsd") {
            return Err(Error::InvalidValue("expected an LLSD XML root element".into()));
        }
        Ok(de)
    }

    fn finish(&mut self) -> Result<()> {
        self.skip_space()?;
        if !matches!(self.tokens.take()?, Some(Token::End(tag)) if tag == "llsd") {
            return Err(Error::InvalidValue("LLSD XML root element was not closed".into()));
        }
        self.skip_space()?;
        if self.tokens.peek()?.is_some() {
            return Err(Error::TrailingData);
        }
        Ok(())
    }

    fn skip_space(&mut self) -> Result<()> {
        while matches!(self.tokens.peek()?, Some(Token::Text(value) | Token::CharacterReference(value)) if value.trim().is_empty()) {
            self.tokens.take()?;
        }
        Ok(())
    }

    fn next_tag(&mut self) -> Result<(&'static str, bool)> {
        self.skip_space()?;
        match self.tokens.peek()? {
            Some(Token::Start(tag)) => {
                let tag = *tag;
                self.tokens.take()?;
                Ok((tag, false))
            }
            Some(Token::Empty(tag)) => {
                let tag = *tag;
                self.tokens.take()?;
                Ok((tag, true))
            }
            Some(Token::End(_)) => Err(Error::UnexpectedEof),
            _ => Err(Error::InvalidValue("expected an LLSD XML value".into())),
        }
    }

    fn text_value(&mut self, tag: &str, empty: bool) -> Result<String> {
        self.text_value_with_limit(tag, empty, self.budget.limits.max_string_bytes, "string bytes")
    }

    fn text_value_with_limit(&mut self, tag: &str, empty: bool, limit: usize, limit_name: &'static str) -> Result<String> {
        if empty {
            return Ok(String::new());
        }
        let mut value = String::new();
        loop {
            match self.tokens.peek()? {
                Some(Token::Text(text)) => {
                    if value.len().checked_add(normalized_text_len(text)).is_none_or(|length| length > limit) {
                        return Err(Error::LimitExceeded(limit_name));
                    }
                    append_normalized_xml_text(&mut value, text);
                    self.tokens.take()?;
                }
                Some(Token::CharacterReference(text)) => {
                    if value.len().checked_add(text.len()).is_none_or(|length| length > limit) {
                        return Err(Error::LimitExceeded(limit_name));
                    }
                    value.push_str(text);
                    self.tokens.take()?;
                }
                Some(Token::End(end)) if *end == tag => {
                    self.tokens.take()?;
                    break;
                }
                Some(Token::Start(_)) | Some(Token::Empty(_)) => return Err(Error::InvalidValue("nested elements in LLSD scalar".into())),
                Some(Token::End(_)) => return Err(Error::InvalidValue("mismatched LLSD XML element".into())),
                None => return Err(Error::UnexpectedEof),
            }
        }
        Ok(value)
    }

    fn map_key(&mut self) -> Result<String> {
        let (tag, empty) = self.next_tag()?;
        if tag != "key" {
            return Err(Error::InvalidValue("expected LLSD map key".into()));
        }
        self.text_value_with_limit(tag, empty, self.budget.limits.max_map_key_bytes, "map key bytes")
    }

    fn count_value(&mut self) -> Result<()> {
        self.budget.count_value()
    }

    fn container<T>(&mut self, parse: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.budget.enter_container()?;
        let result = parse(self);
        self.budget.leave_container();
        result
    }

    fn consume_enum_unit(&mut self) -> Result<()> {
        self.count_value()?;
        let (tag, empty) = self.next_tag()?;
        if tag != "undef" {
            return Err(Error::InvalidValue("expected LLSD undefined".into()));
        }
        self.text_value(tag, empty).map(|_| ())
    }

    fn finish_enum_map(&mut self) -> Result<()> {
        self.skip_space()?;
        match self.tokens.peek()? {
            Some(Token::End(tag)) if *tag == "map" => {
                self.tokens.take()?;
                Ok(())
            }
            _ => Err(Error::InvalidValue("LLSD enum map must contain exactly one entry".into())),
        }
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
        let text = self.text_value(tag, empty)?;
        if text.is_empty() { Ok(default.to_owned()) } else { Ok(text) }
    }

    fn deserialize_plain_enum<'de, V: Visitor<'de>>(&mut self, visitor: V) -> Result<V::Value> {
        self.skip_space()?;
        match self.tokens.peek()? {
            Some(Token::Empty(tag) | Token::Start(tag)) if *tag == "string" => {
                let variant = self.scalar_for_wrapper("string")?;
                visitor.visit_enum(StringDeserializer::<Error>::new(variant))
            }
            Some(Token::Start(tag)) if *tag == "map" => {
                self.count_value()?;
                let (tag, empty) = self.next_tag()?;
                if tag != "map" || empty {
                    return Err(Error::InvalidValue("LLSD enum map must contain exactly one entry".into()));
                }
                self.container(|de| {
                    de.skip_space()?;
                    let variant = de.map_key()?;
                    if de.budget.limits.max_container_items == 0 {
                        return Err(Error::LimitExceeded("container items"));
                    }
                    let access = VariantAccess::new(de, Self::consume_enum_unit, Self::finish_enum_map);
                    visitor.visit_enum(VariantEnumAccess::new(variant, access))
                })
            }
            _ => Err(Error::InvalidValue("expected an LLSD enum string or single-entry map".into())),
        }
    }

    fn parse_binary(&mut self, tag: &str, empty: bool) -> Result<Vec<u8>> {
        let text = self.text_value_with_limit(tag, empty, self.budget.limits.max_bytes, "input bytes")?;
        let max_encoded_len = self.budget.limits.max_binary_bytes.div_ceil(3).saturating_mul(4);
        let mut compact = Vec::with_capacity(text.len().min(max_encoded_len));
        for byte in text.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
            if compact.len() == max_encoded_len {
                return Err(Error::LimitExceeded("binary bytes"));
            }
            compact.push(byte);
        }
        if base64_decoded_len(&compact)? > self.budget.limits.max_binary_bytes {
            return Err(Error::LimitExceeded("binary bytes"));
        }
        STANDARD.decode(compact).map_err(|error| Error::InvalidValue(error.to_string()))
    }
}

impl<'de, 'input> de::Deserializer<'de> for &mut Deserializer<'input> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        let (tag, empty) = self.next_tag()?;
        match tag {
            "undef" => {
                self.text_value(tag, empty)?;
                visitor.visit_unit()
            }
            "boolean" => {
                let value = self.text_value(tag, empty)?;
                match value.trim() {
                    "" | "0" | "false" => visitor.visit_bool(false),
                    "1" | "true" => visitor.visit_bool(true),
                    _ => Err(Error::InvalidValue("invalid LLSD boolean".into())),
                }
            }
            "integer" => {
                let value = self.text_value(tag, empty)?;
                let number = if value.trim().is_empty() {
                    0
                } else {
                    value.trim().parse::<i32>().map_err(|_| Error::InvalidValue("LLSD integer is outside the i32 range".into()))?
                };
                visitor.visit_i32(number)
            }
            "real" => {
                let value = self.text_value(tag, empty)?;
                let number = if value.trim().is_empty() { 0.0 } else { parse_real(value.trim())? };
                visitor.visit_f64(number)
            }
            "uuid" | "string" | "uri" | "date" => {
                let default = match tag {
                    "uuid" => "00000000-0000-0000-0000-000000000000",
                    "date" => "1970-01-01T00:00:00Z",
                    _ => "",
                };
                let text = self.text_value(tag, empty)?;
                visitor.visit_string(if text.is_empty() { default.to_owned() } else { text })
            }
            "binary" => visitor.visit_byte_buf(self.parse_binary(tag, empty)?),
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
    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        let (tag, empty) = self.next_tag()?;
        if tag != "binary" {
            return Err(Error::InvalidValue("expected LLSD binary".into()));
        }
        let bytes = self.parse_binary(tag, empty)?;
        visitor.visit_bytes(&bytes)
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        let (tag, empty) = self.next_tag()?;
        if tag != "binary" {
            return Err(Error::InvalidValue("expected LLSD binary".into()));
        }
        visitor.visit_byte_buf(self.parse_binary(tag, empty)?)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        if matches!(self.tokens.peek()?, Some(Token::Empty(tag) | Token::Start(tag)) if *tag == "undef") {
            self.count_value()?;
            let (tag, empty) = self.next_tag()?;
            self.text_value(tag, empty)?;
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
        self.text_value(tag, empty)?;
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _: &'static str, v: V) -> Result<V::Value> {
        self.deserialize_unit(v)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, name: &'static str, visitor: V) -> Result<V::Value> {
        match Wrapper::from_name(name) {
            Some(Wrapper::Uuid) => visitor.visit_newtype_struct(StringDeserializer::<Error>::new(self.scalar_for_wrapper("uuid")?)),
            Some(Wrapper::Uri) => visitor.visit_newtype_struct(StringDeserializer::<Error>::new(self.scalar_for_wrapper("uri")?)),
            Some(Wrapper::Date) => {
                let text = self.scalar_for_wrapper("date")?;
                Date::parse(&text)?;
                visitor.visit_newtype_struct(StringDeserializer::<Error>::new(text))
            }
            Some(Wrapper::Binary) => {
                self.count_value()?;
                let (tag, empty) = self.next_tag()?;
                if tag != "binary" {
                    return Err(Error::InvalidValue("expected LLSD binary".into()));
                }
                let bytes = self.parse_binary(tag, empty)?;
                visitor.visit_newtype_struct(BytesDeserializer::<Error>::new(&bytes))
            }
            None => visitor.visit_newtype_struct(self),
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
        self.skip_space()?;
        if name == VALUE_ENUM {
            let variant = match self.tokens.peek()? {
                Some(Token::Empty(tag) | Token::Start(tag)) => match *tag {
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
            let access = VariantAccess::new(self, Deserializer::consume_enum_unit, no_finish);
            visitor.visit_enum(VariantEnumAccess::new(variant.to_owned(), access))
        } else {
            self.deserialize_plain_enum(visitor)
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_any(IgnoredAny)?;
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

struct XmlSeqAccess<'a, 'input> {
    de: &'a mut Deserializer<'input>,
    ended: bool,
    count: usize,
}

impl<'a, 'input> XmlSeqAccess<'a, 'input> {
    fn finish(&mut self, tag: &str) -> Result<()> {
        if self.ended {
            return Ok(());
        }
        self.de.skip_space()?;
        match self.de.tokens.peek()? {
            Some(Token::End(end)) if *end == tag => {
                self.de.tokens.take()?;
                self.ended = true;
                Ok(())
            }
            _ => Err(Error::InvalidValue("LLSD array was not fully read".into())),
        }
    }
}

impl<'de, 'a, 'input> SeqAccess<'de> for XmlSeqAccess<'a, 'input> {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        self.de.skip_space()?;
        if matches!(self.de.tokens.peek()?, Some(Token::End(tag)) if *tag == "array") {
            self.de.tokens.take()?;
            self.ended = true;
            return Ok(None);
        }
        self.count = self.count.checked_add(1).ok_or(Error::LimitExceeded("container items"))?;
        if self.count > self.de.budget.limits.max_container_items {
            return Err(Error::LimitExceeded("container items"));
        }
        seed.deserialize(&mut *self.de).map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        None
    }
}

struct XmlMapAccess<'a, 'input> {
    de: &'a mut Deserializer<'input>,
    ended: bool,
    expects_value: bool,
    count: usize,
}

impl<'a, 'input> XmlMapAccess<'a, 'input> {
    fn finish(&mut self) -> Result<()> {
        if self.ended {
            return Ok(());
        }
        if self.expects_value {
            return Err(Error::InvalidValue("LLSD map is missing a value".into()));
        }
        self.de.skip_space()?;
        match self.de.tokens.peek()? {
            Some(Token::End(tag)) if *tag == "map" => {
                self.de.tokens.take()?;
                self.ended = true;
                Ok(())
            }
            _ => Err(Error::InvalidValue("LLSD map was not fully read".into())),
        }
    }
}

impl<'de, 'a, 'input> MapAccess<'de> for XmlMapAccess<'a, 'input> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        self.de.skip_space()?;
        if self.expects_value {
            return Err(Error::InvalidValue("LLSD map value was not read".into()));
        }
        if matches!(self.de.tokens.peek()?, Some(Token::End(tag)) if *tag == "map") {
            self.de.tokens.take()?;
            self.ended = true;
            return Ok(None);
        }
        self.count = self.count.checked_add(1).ok_or(Error::LimitExceeded("container items"))?;
        if self.count > self.de.budget.limits.max_container_items {
            return Err(Error::LimitExceeded("container items"));
        }
        let key = self.de.map_key()?;
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

pub fn from_slice_with_limits<T: DeserializeOwned>(input: &[u8], limits: Limits) -> Result<T> {
    if input.len() > limits.max_bytes {
        return Err(Error::LimitExceeded("input bytes"));
    }
    let tokens = Tokens::new(input, limits)?;
    let mut de = Deserializer::new(tokens, limits)?;
    let value = T::deserialize(&mut de)?;
    de.finish()?;
    Ok(value)
}

fn append_normalized_xml_text(output: &mut String, text: &str) {
    if !text.as_bytes().contains(&b'\r') {
        output.push_str(text);
        return;
    }

    let bytes = text.as_bytes();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' {
            output.push_str(&text[start..index]);
            output.push('\n');
            index += 1;
            if bytes.get(index) == Some(&b'\n') {
                index += 1;
            }
            start = index;
        } else {
            index += 1;
        }
    }
    output.push_str(&text[start..]);
}
