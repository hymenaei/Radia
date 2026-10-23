use serde::de::{self, DeserializeSeed, value::StringDeserializer};

use crate::{
    Error, Limits, Result,
    value::{BINARY_NEWTYPE, DATE_NEWTYPE, URI_NEWTYPE, UUID_NEWTYPE},
};

pub(crate) struct Budget {
    pub(crate) limits: Limits,
    values: usize,
    depth: usize,
}

impl Budget {
    pub(crate) fn new(limits: Limits) -> Self {
        Self { limits, values: 0, depth: 0 }
    }

    pub(crate) fn count_value(&mut self) -> Result<()> {
        self.values = self.values.checked_add(1).ok_or(Error::LimitExceeded("value count"))?;
        if self.values > self.limits.max_values {
            return Err(Error::LimitExceeded("value count"));
        }
        Ok(())
    }

    pub(crate) fn admit_container_count(&self, count: usize) -> Result<()> {
        if count > self.limits.max_container_items {
            return Err(Error::LimitExceeded("container items"));
        }
        if count > self.limits.max_values.saturating_sub(self.values) {
            return Err(Error::LimitExceeded("value count"));
        }
        Ok(())
    }

    pub(crate) fn enter_container(&mut self) -> Result<()> {
        self.depth = self.depth.checked_add(1).ok_or(Error::LimitExceeded("nesting depth"))?;
        if self.depth > self.limits.max_depth {
            self.depth -= 1;
            return Err(Error::LimitExceeded("nesting depth"));
        }
        Ok(())
    }

    pub(crate) fn leave_container(&mut self) {
        self.depth -= 1;
    }
}

pub(crate) struct Cursor<'de> {
    input: &'de [u8],
    offset: usize,
}

impl<'de> Cursor<'de> {
    pub(crate) fn new(input: &'de [u8]) -> Self {
        Self { input, offset: 0 }
    }

    pub(crate) fn remaining(&self) -> &'de [u8] {
        self.input.get(self.offset..).unwrap_or_default()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.offset == self.input.len()
    }

    pub(crate) fn is_next(&self, expected: u8) -> bool {
        self.input.get(self.offset) == Some(&expected)
    }

    pub(crate) fn peek(&self) -> Result<u8> {
        self.input.get(self.offset).copied().ok_or(Error::UnexpectedEof)
    }

    pub(crate) fn take(&mut self, length: usize) -> Result<&'de [u8]> {
        let end = self.offset.checked_add(length).ok_or(Error::LimitExceeded("input bytes"))?;
        let value = self.input.get(self.offset..end).ok_or(Error::UnexpectedEof)?;
        self.offset = end;
        Ok(value)
    }

    pub(crate) fn byte(&mut self) -> Result<u8> {
        let value = self.peek()?;
        self.offset += 1;
        Ok(value)
    }

    pub(crate) fn expect(&mut self, expected: u8, message: &'static str) -> Result<()> {
        if self.byte()? == expected { Ok(()) } else { Err(Error::InvalidValue(message.into())) }
    }

    pub(crate) fn skip_ascii_whitespace(&mut self) {
        while self.remaining().first().is_some_and(u8::is_ascii_whitespace) {
            self.offset += 1;
        }
    }
}

pub(crate) enum Wrapper {
    Uuid,
    Uri,
    Date,
    Binary,
}

impl Wrapper {
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            UUID_NEWTYPE => Some(Self::Uuid),
            URI_NEWTYPE => Some(Self::Uri),
            DATE_NEWTYPE => Some(Self::Date),
            BINARY_NEWTYPE => Some(Self::Binary),
            _ => None,
        }
    }
}

pub(crate) struct VariantEnumAccess<V> {
    variant: String,
    access: V,
}

impl<V> VariantEnumAccess<V> {
    pub(crate) fn new(variant: String, access: V) -> Self {
        Self { variant, access }
    }
}

impl<'de, V> de::EnumAccess<'de> for VariantEnumAccess<V>
where
    V: de::VariantAccess<'de, Error = Error>,
{
    type Error = Error;
    type Variant = V;

    fn variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<(S::Value, Self::Variant)> {
        let variant = seed.deserialize(StringDeserializer::<Error>::new(self.variant))?;
        Ok((variant, self.access))
    }
}

pub(crate) struct VariantAccess<'a, D, U, F> {
    de: &'a mut D,
    unit: U,
    finish: F,
}

impl<'a, D, U, F> VariantAccess<'a, D, U, F> {
    pub(crate) fn new(de: &'a mut D, unit: U, finish: F) -> Self {
        Self { de, unit, finish }
    }
}

impl<'de, 'a, D, U, F> de::VariantAccess<'de> for VariantAccess<'a, D, U, F>
where
    for<'b> &'b mut D: de::Deserializer<'de, Error = Error>,
    U: FnOnce(&mut D) -> Result<()>,
    F: FnOnce(&mut D) -> Result<()>,
{
    type Error = Error;

    fn unit_variant(self) -> Result<()> {
        let Self { de, unit, finish } = self;
        unit(&mut *de)?;
        finish(&mut *de)
    }

    fn newtype_variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<S::Value> {
        let Self { de, finish, .. } = self;
        let value = seed.deserialize(&mut *de)?;
        finish(&mut *de)?;
        Ok(value)
    }

    fn tuple_variant<V: de::Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> {
        let Self { de, finish, .. } = self;
        let value = de::Deserializer::deserialize_tuple(&mut *de, len, visitor)?;
        finish(&mut *de)?;
        Ok(value)
    }

    fn struct_variant<V: de::Visitor<'de>>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value> {
        let Self { de, finish, .. } = self;
        let value = de::Deserializer::deserialize_struct(&mut *de, "", fields, visitor)?;
        finish(&mut *de)?;
        Ok(value)
    }
}

pub(crate) fn no_finish<D>(_: &mut D) -> Result<()> {
    Ok(())
}
