use std::str;

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor,
    value::{BytesDeserializer, StringDeserializer},
};
use serde::forward_to_deserialize_any;

use crate::{
    Date, Error, Limits, Result,
    decode::{Budget, Cursor, VariantAccess, VariantEnumAccess, Wrapper, no_finish},
    encoding::{base64_decoded_len, strip_header},
    value::VALUE_ENUM,
};

struct Deserializer<'de> {
    cursor: Cursor<'de>,
    budget: Budget,
}

impl<'de> Deserializer<'de> {
    fn new(input: &'de [u8], limits: Limits) -> Self {
        Self { cursor: Cursor::new(input), budget: Budget::new(limits) }
    }

    fn skip_space(&mut self) {
        self.cursor.skip_ascii_whitespace();
    }

    fn peek(&mut self) -> Result<u8> {
        self.skip_space();
        self.cursor.peek()
    }

    fn byte(&mut self) -> Result<u8> {
        self.skip_space();
        self.cursor.byte()
    }

    fn take(&mut self, length: usize) -> Result<&'de [u8]> {
        self.cursor.take(length)
    }

    fn expect(&mut self, expected: u8) -> Result<()> {
        self.skip_space();
        self.cursor.expect(expected, "invalid LLSD notation delimiter")
    }

    fn byte_immediate(&mut self) -> Result<u8> {
        self.cursor.byte()
    }

    fn expect_immediate(&mut self, expected: u8) -> Result<()> {
        self.cursor.expect(expected, "invalid LLSD notation delimiter")
    }

    fn count_value(&mut self) -> Result<()> {
        self.budget.count_value()
    }

    fn container<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.budget.enter_container()?;
        let result = f(self);
        self.budget.leave_container();
        result
    }

    fn finish_enum_map(&mut self) -> Result<()> {
        self.skip_space();
        self.expect(b'}')
    }

    fn consume_enum_unit(&mut self) -> Result<()> {
        self.count_value()?;
        if self.byte()? == b'!' { Ok(()) } else { Err(Error::InvalidValue("expected LLSD undefined".into())) }
    }

    fn deserialize_plain_enum<V: Visitor<'de>>(&mut self, visitor: V) -> Result<V::Value> {
        match self.peek()? {
            b'\'' | b'"' => {
                self.count_value()?;
                visitor.visit_enum(StringDeserializer::<Error>::new(self.quoted()?))
            }
            b's' => {
                self.count_value()?;
                self.cursor.byte()?;
                visitor.visit_enum(StringDeserializer::<Error>::new(self.typed_string()?))
            }
            b'{' => {
                self.count_value()?;
                self.expect(b'{')?;
                if self.budget.limits.max_container_items == 0 {
                    return Err(Error::LimitExceeded("container items"));
                }
                self.container(|de| {
                    let variant = de.map_key()?;
                    de.skip_space();
                    de.expect(b':')?;
                    let access = VariantAccess::new(de, Self::consume_enum_unit, Self::finish_enum_map);
                    visitor.visit_enum(VariantEnumAccess::new(variant, access))
                })
            }
            _ => Err(Error::InvalidValue("expected an LLSD enum string or single-entry map".into())),
        }
    }

    fn token(&mut self) -> Result<&'de [u8]> {
        self.skip_space();
        let remaining = self.cursor.remaining();
        let length = remaining
            .iter()
            .position(|byte| byte.is_ascii_whitespace() || matches!(*byte, b',' | b']' | b'}' | b':'))
            .unwrap_or(remaining.len());
        if length == 0 {
            return Err(Error::UnexpectedEof);
        }
        self.cursor.take(length)
    }

    fn quoted(&mut self) -> Result<String> {
        self.quoted_with_limit(self.budget.limits.max_string_bytes, "string bytes")
    }

    fn quoted_with_limit(&mut self, max_bytes: usize, limit_name: &'static str) -> Result<String> {
        self.skip_space();
        let quote = self.byte()?;
        if !matches!(quote, b'\'' | b'"') {
            return Err(Error::InvalidValue("expected a quoted LLSD string".into()));
        }
        let mut output = Vec::new();
        loop {
            let byte = self.cursor.byte()?;
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
            let escape = self.byte_immediate()?;
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
        let digits = self.cursor.take(count)?;
        let digits = str::from_utf8(digits).map_err(|_| Error::InvalidUtf8)?;
        u32::from_str_radix(digits, 16).map_err(|_| Error::InvalidValue("invalid hexadecimal escape".into()))
    }

    fn typed_string(&mut self) -> Result<String> {
        self.typed_string_with_limit(self.budget.limits.max_string_bytes, "string bytes")
    }

    fn typed_string_with_limit(&mut self, limit: usize, name: &'static str) -> Result<String> {
        self.skip_space();
        if self.cursor.is_next(b'(') {
            self.cursor.byte()?;
            let count = self.decimal_length(limit, name)?;
            self.expect(b')')?;
            let quote = self.byte()?;
            if !matches!(quote, b'\'' | b'"') {
                return Err(Error::InvalidValue("expected a quoted LLSD string".into()));
            }
            let value = String::from_utf8(self.take(count)?.to_vec()).map_err(|_| Error::InvalidUtf8)?;
            self.expect_immediate(quote)?;
            Ok(value)
        } else {
            self.quoted_with_limit(limit, name)
        }
    }

    fn map_key(&mut self) -> Result<String> {
        match self.peek()? {
            b'\'' | b'"' => self.quoted_with_limit(self.budget.limits.max_map_key_bytes, "map key bytes"),
            b's' => {
                self.cursor.byte()?;
                self.typed_string_with_limit(self.budget.limits.max_map_key_bytes, "map key bytes")
            }
            _ => Err(Error::InvalidValue("LLSD map key must be a string".into())),
        }
    }

    fn decimal_length(&mut self, limit: usize, name: &'static str) -> Result<usize> {
        let remaining = self.cursor.remaining();
        let length = remaining.iter().position(|byte| !byte.is_ascii_digit()).unwrap_or(remaining.len());
        if length == 0 {
            return Err(Error::InvalidValue("expected a string length".into()));
        }
        let value = str::from_utf8(self.cursor.take(length)?)
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
        crate::encoding::parse_real(self.number_token(b'r')?)
    }

    fn parse_binary(&mut self) -> Result<Vec<u8>> {
        self.skip_space();
        if self.cursor.remaining().starts_with(b"b(") {
            self.cursor.take(2)?;
            let length = self.decimal_length(self.budget.limits.max_binary_bytes, "binary bytes")?;
            self.expect(b')')?;
            self.expect(b'"')?;
            let bytes = self.take(length)?.to_vec();
            if !self.cursor.is_next(b'"') {
                return Err(Error::InvalidValue("length-prefixed LLSD binary has the wrong length".into()));
            }
            self.cursor.byte()?;
            return Ok(bytes);
        }
        let prefix = self.cursor.remaining().get(..3).ok_or(Error::UnexpectedEof)?;
        let base16 = match prefix {
            b"b16" => true,
            b"b64" => false,
            _ => return Err(Error::InvalidValue("expected b16 or b64 LLSD binary".into())),
        };
        self.cursor.take(3)?;
        let max_encoded_len = if base16 {
            self.budget.limits.max_binary_bytes.saturating_mul(2)
        } else {
            self.budget.limits.max_binary_bytes.div_ceil(3).saturating_mul(4)
        };
        let value = self.quoted_with_limit(max_encoded_len, "binary bytes")?;
        let decoded_len = if base16 { value.len().div_ceil(2) } else { base64_decoded_len(value.as_bytes())? };
        if decoded_len > self.budget.limits.max_binary_bytes {
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

impl<'de> de::Deserializer<'de> for &mut Deserializer<'de> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        match self.peek()? {
            b'!' => {
                self.cursor.byte()?;
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
                self.cursor.byte()?;
                visitor.visit_string(str::from_utf8(self.token()?).map_err(|_| Error::InvalidUtf8)?.to_owned())
            }
            b'd' => {
                self.cursor.byte()?;
                let text = self.quoted()?;
                let date = Date::parse(&text)?;
                visitor.visit_f64(date.unix_seconds())
            }
            b'l' => {
                self.cursor.byte()?;
                visitor.visit_string(self.typed_string()?)
            }
            b'b' => visitor.visit_byte_buf(self.parse_binary()?),
            b'\'' | b'"' => visitor.visit_string(self.quoted()?),
            b's' => {
                self.cursor.byte()?;
                visitor.visit_string(self.typed_string()?)
            }
            b'[' => {
                self.cursor.byte()?;
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
                self.cursor.byte()?;
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
            self.cursor.byte()?;
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
        match Wrapper::from_name(name) {
            Some(Wrapper::Uuid) => {
                self.count_value()?;
                if self.byte()? != b'u' {
                    return Err(Error::InvalidValue("expected LLSD UUID".into()));
                }
                visitor.visit_newtype_struct(StringDeserializer::<Error>::new(
                    str::from_utf8(self.token()?).map_err(|_| Error::InvalidUtf8)?.to_owned(),
                ))
            }
            Some(Wrapper::Uri) => {
                self.count_value()?;
                if self.byte()? != b'l' {
                    return Err(Error::InvalidValue("expected LLSD URI".into()));
                }
                visitor.visit_newtype_struct(StringDeserializer::<Error>::new(self.typed_string()?))
            }
            Some(Wrapper::Date) => {
                self.count_value()?;
                if self.byte()? != b'd' {
                    return Err(Error::InvalidValue("expected LLSD date".into()));
                }
                let text = self.quoted()?;
                visitor.visit_newtype_struct(StringDeserializer::<Error>::new(text))
            }
            Some(Wrapper::Binary) => {
                self.count_value()?;
                let value = self.parse_binary()?;
                visitor.visit_newtype_struct(BytesDeserializer::<Error>::new(&value))
            }
            None => visitor.visit_newtype_struct(self),
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
        if self.de.cursor.is_next(b']') {
            self.de.cursor.byte()?;
            self.ended = true;
            return Ok(None);
        }
        if !self.first {
            self.de.expect(b',')?;
            self.de.skip_space();
        }
        self.first = false;
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
        if self.de.cursor.is_next(b'}') {
            self.de.cursor.byte()?;
            self.ended = true;
            return Ok(None);
        }
        if !self.first {
            self.de.expect(b',')?;
        }
        self.first = false;
        let key = self.de.map_key()?;
        self.de.skip_space();
        self.de.expect(b':')?;
        self.expects_value = true;
        self.count = self.count.checked_add(1).ok_or(Error::LimitExceeded("container items"))?;
        if self.count > self.de.budget.limits.max_container_items {
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

pub fn from_slice_with_limits<T: DeserializeOwned>(input: &[u8], limits: Limits) -> Result<T> {
    if input.len() > limits.max_bytes {
        return Err(Error::LimitExceeded("input bytes"));
    }
    let input = strip_header(input, b"llsd/notation").unwrap_or(input);
    let mut de = Deserializer::new(input, limits);
    let value = T::deserialize(&mut de)?;
    de.skip_space();
    if !de.cursor.is_empty() {
        return Err(Error::TrailingData);
    }
    Ok(value)
}
