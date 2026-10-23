use serde::{
    de::{
        self, DeserializeOwned, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor,
        value::{BytesDeserializer, F64Deserializer, StringDeserializer},
    },
    forward_to_deserialize_any,
};
use uuid::Uuid;

use crate::{
    Error, Limits, Result,
    decode::{Budget, Cursor, VariantAccess, VariantEnumAccess, Wrapper, no_finish},
    encoding::strip_header,
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

    fn peek(&self) -> Result<u8> {
        self.cursor.peek()
    }

    fn take(&mut self, length: usize) -> Result<&'de [u8]> {
        self.cursor.take(length)
    }

    fn byte(&mut self) -> Result<u8> {
        self.cursor.byte()
    }

    fn expect(&mut self, expected: u8) -> Result<()> {
        self.cursor.expect(expected, "invalid LLSD binary delimiter")
    }

    fn count_value(&mut self) -> Result<()> {
        self.budget.count_value()
    }

    fn read_length(&mut self, limit: usize, name: &'static str) -> Result<usize> {
        let value = u32::from_be_bytes(self.take(4)?.try_into().map_err(|_| Error::UnexpectedEof)?);
        let value = usize::try_from(value).map_err(|_| Error::LimitExceeded(name))?;
        if value > limit {
            return Err(Error::LimitExceeded(name));
        }
        Ok(value)
    }

    fn read_text(&mut self, limit: usize, name: &'static str) -> Result<String> {
        let length = self.read_length(limit, name)?;
        String::from_utf8(self.take(length)?.to_vec()).map_err(|_| Error::InvalidUtf8)
    }

    fn read_container_count(&mut self) -> Result<usize> {
        let count = u32::from_be_bytes(self.take(4)?.try_into().map_err(|_| Error::UnexpectedEof)?);
        let count = usize::try_from(count).map_err(|_| Error::LimitExceeded("container items"))?;
        self.budget.admit_container_count(count)?;
        Ok(count)
    }

    fn read_uuid(&mut self) -> Result<String> {
        let bytes = self.take(16)?;
        let value: [u8; 16] = bytes.try_into().map_err(|_| Error::UnexpectedEof)?;
        Ok(Uuid::from_bytes(value).to_string())
    }

    fn read_binary(&mut self) -> Result<Vec<u8>> {
        let length = self.read_length(self.budget.limits.max_binary_bytes, "binary bytes")?;
        Ok(self.take(length)?.to_vec())
    }

    fn consume_undefined(&mut self) -> Result<()> {
        self.count_value()?;
        self.expect(b'!')
    }

    fn container<T>(&mut self, visitor: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.budget.enter_container()?;
        let result = visitor(self);
        self.budget.leave_container();
        result
    }

    fn finish_enum_map(&mut self) -> Result<()> {
        self.expect(b'}')
    }

    fn visit_seq<V: Visitor<'de>>(&mut self, visitor: V) -> Result<V::Value> {
        let count = self.read_container_count()?;
        self.container(|de| {
            let mut access = BinarySeqAccess { de, remaining: count, ended: false };
            let value = visitor.visit_seq(&mut access)?;
            access.finish()?;
            Ok(value)
        })
    }

    fn visit_map<V: Visitor<'de>>(&mut self, visitor: V) -> Result<V::Value> {
        let count = self.read_container_count()?;
        self.container(|de| {
            let mut access = BinaryMapAccess { de, remaining: count, expects_value: false, ended: false };
            let value = visitor.visit_map(&mut access)?;
            access.finish()?;
            Ok(value)
        })
    }

    fn deserialize_plain_enum<V: Visitor<'de>>(&mut self, visitor: V) -> Result<V::Value> {
        match self.peek()? {
            b's' => {
                self.count_value()?;
                self.expect(b's')?;
                let variant = self.read_text(self.budget.limits.max_string_bytes, "string bytes")?;
                visitor.visit_enum(StringDeserializer::<Error>::new(variant))
            }
            b'{' => {
                self.count_value()?;
                self.expect(b'{')?;
                let count = self.read_container_count()?;
                if count != 1 {
                    return Err(Error::InvalidValue("LLSD enum map must contain exactly one entry".into()));
                }
                self.container(|de| {
                    de.expect(b'k')?;
                    let variant = de.read_text(de.budget.limits.max_map_key_bytes, "map key bytes")?;
                    let access = VariantAccess::new(de, Self::consume_undefined, Self::finish_enum_map);
                    visitor.visit_enum(VariantEnumAccess::new(variant, access))
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
                let value = f64::from_be_bytes(self.take(8)?.try_into().map_err(|_| Error::UnexpectedEof)?);
                visitor.visit_f64(value)
            }
            b'd' => {
                let value = f64::from_le_bytes(self.take(8)?.try_into().map_err(|_| Error::UnexpectedEof)?);
                visitor.visit_f64(value)
            }
            b'u' => visitor.visit_string(self.read_uuid()?),
            b's' => visitor.visit_string(self.read_text(self.budget.limits.max_string_bytes, "string bytes")?),
            b'l' => visitor.visit_string(self.read_text(self.budget.limits.max_string_bytes, "string bytes")?),
            b'b' => visitor.visit_byte_buf(self.read_binary()?),
            b'[' => self.visit_seq(visitor),
            b'{' => self.visit_map(visitor),
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
        let length = self.read_length(self.budget.limits.max_binary_bytes, "binary bytes")?;
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
        match Wrapper::from_name(name) {
            Some(Wrapper::Uuid) => {
                self.count_value()?;
                if self.byte()? != b'u' {
                    return Err(Error::InvalidValue("expected LLSD UUID".into()));
                }
                visitor.visit_newtype_struct(StringDeserializer::<Error>::new(self.read_uuid()?))
            }
            Some(Wrapper::Uri) => {
                self.count_value()?;
                if self.byte()? != b'l' {
                    return Err(Error::InvalidValue("expected LLSD URI".into()));
                }
                visitor.visit_newtype_struct(StringDeserializer::<Error>::new(
                    self.read_text(self.budget.limits.max_string_bytes, "string bytes")?,
                ))
            }
            Some(Wrapper::Date) => {
                self.count_value()?;
                if self.byte()? != b'd' {
                    return Err(Error::InvalidValue("expected LLSD date".into()));
                }
                let value = f64::from_le_bytes(self.take(8)?.try_into().map_err(|_| Error::UnexpectedEof)?);
                visitor.visit_newtype_struct(F64Deserializer::<Error>::new(value))
            }
            Some(Wrapper::Binary) => {
                self.count_value()?;
                if self.byte()? != b'b' {
                    return Err(Error::InvalidValue("expected LLSD binary".into()));
                }
                visitor.visit_newtype_struct(BytesDeserializer::<Error>::new(&self.read_binary()?))
            }
            None => visitor.visit_newtype_struct(self),
        }
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.count_value()?;
        if self.byte()? != b'[' {
            return Err(Error::InvalidValue("expected LLSD array".into()));
        }
        self.visit_seq(visitor)
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
        self.visit_map(visitor)
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
            let access = VariantAccess::new(self, Deserializer::consume_undefined, no_finish);
            visitor.visit_enum(VariantEnumAccess::new(variant.to_owned(), access))
        } else {
            self.deserialize_plain_enum(visitor)
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(IgnoredAny)?;
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
        let key = self.de.read_text(self.de.budget.limits.max_map_key_bytes, "map key bytes")?;
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

pub fn from_slice_with_limits<T: DeserializeOwned>(input: &[u8], limits: Limits) -> Result<T> {
    if input.len() > limits.max_bytes {
        return Err(Error::LimitExceeded("input bytes"));
    }
    let input = match strip_header(input, b"llsd/binary") {
        Some(rest) => rest.trim_ascii_start(),
        None => input,
    };
    let mut de = Deserializer::new(input, limits);
    let value = T::deserialize(&mut de)?;
    if !de.cursor.is_empty() {
        return Err(Error::TrailingData);
    }
    Ok(value)
}
