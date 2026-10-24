use std::{fmt, marker::PhantomData};

use serde::ser::{self, Impossible, Serialize};

use crate::{Error, Result, encoding::real_text};

use super::Format;

pub(super) struct KeySerializer<F> {
    pub(super) budget: Option<usize>,
    pub(super) _format: PhantomData<F>,
}

impl<F: Format> KeySerializer<F> {
    fn owned(self, key: String) -> Result<String> {
        if let Some(budget) = &self.budget {
            F::charge_key(&key, *budget)?;
        }
        Ok(key)
    }
}

impl<F: Format> ser::Serializer for KeySerializer<F> {
    type Ok = String;
    type Error = Error;
    type SerializeSeq = Impossible<String, Error>;
    type SerializeTuple = Impossible<String, Error>;
    type SerializeTupleStruct = Impossible<String, Error>;
    type SerializeTupleVariant = Impossible<String, Error>;
    type SerializeMap = Impossible<String, Error>;
    type SerializeStruct = Impossible<String, Error>;
    type SerializeStructVariant = Impossible<String, Error>;

    fn serialize_str(self, value: &str) -> Result<String> {
        if let Some(budget) = &self.budget {
            F::charge_key(value, *budget)?;
        }
        Ok(value.to_owned())
    }

    fn serialize_char(self, value: char) -> Result<String> {
        self.owned(value.to_string())
    }

    fn serialize_unit(self) -> Result<String> {
        self.owned(String::new())
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result<String> {
        self.serialize_unit()
    }

    fn serialize_unit_variant(self, _: &'static str, _: u32, variant: &'static str) -> Result<String> {
        if let Some(budget) = &self.budget {
            F::charge_key(variant, *budget)?;
        }
        Ok(variant.to_owned())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _: &'static str, value: &T) -> Result<String> {
        value.serialize(self)
    }

    fn serialize_bool(self, value: bool) -> Result<String> {
        self.owned(value.to_string())
    }

    fn serialize_i8(self, value: i8) -> Result<String> {
        self.owned(value.to_string())
    }

    fn serialize_i16(self, value: i16) -> Result<String> {
        self.owned(value.to_string())
    }

    fn serialize_i32(self, value: i32) -> Result<String> {
        self.owned(value.to_string())
    }

    fn serialize_i64(self, value: i64) -> Result<String> {
        self.owned(value.to_string())
    }

    fn serialize_u8(self, value: u8) -> Result<String> {
        self.owned(value.to_string())
    }

    fn serialize_u16(self, value: u16) -> Result<String> {
        self.owned(value.to_string())
    }

    fn serialize_u32(self, value: u32) -> Result<String> {
        self.owned(value.to_string())
    }

    fn serialize_u64(self, value: u64) -> Result<String> {
        self.owned(value.to_string())
    }

    fn serialize_f32(self, value: f32) -> Result<String> {
        self.owned(real_text(f64::from(value)))
    }

    fn serialize_f64(self, value: f64) -> Result<String> {
        self.owned(real_text(value))
    }

    fn serialize_bytes(self, _: &[u8]) -> Result<String> {
        Err(Error::InvalidValue("map keys must be strings".into()))
    }

    fn serialize_none(self) -> Result<String> {
        Err(Error::InvalidValue("map keys must be strings".into()))
    }

    fn serialize_some<T: Serialize + ?Sized>(self, _: &T) -> Result<String> {
        Err(Error::InvalidValue("map keys must be strings".into()))
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(self, _: &'static str, _: u32, _: &'static str, _: &T) -> Result<String> {
        Err(Error::InvalidValue("map keys must be strings".into()))
    }

    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq> {
        Err(Error::InvalidValue("map keys must be strings".into()))
    }

    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple> {
        Err(Error::InvalidValue("map keys must be strings".into()))
    }

    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeTupleStruct> {
        Err(Error::InvalidValue("map keys must be strings".into()))
    }

    fn serialize_tuple_variant(self, _: &'static str, _: u32, _: &'static str, _: usize) -> Result<Self::SerializeTupleVariant> {
        Err(Error::InvalidValue("map keys must be strings".into()))
    }

    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap> {
        Err(Error::InvalidValue("map keys must be strings".into()))
    }

    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeStruct> {
        Err(Error::InvalidValue("map keys must be strings".into()))
    }

    fn serialize_struct_variant(self, _: &'static str, _: u32, _: &'static str, _: usize) -> Result<Self::SerializeStructVariant> {
        Err(Error::InvalidValue("map keys must be strings".into()))
    }

    fn collect_str<T: fmt::Display + ?Sized>(self, value: &T) -> Result<String> {
        let text = F::display(value, "key", self.budget)?;
        self.owned(text)
    }

    fn is_human_readable(&self) -> bool {
        F::HUMAN_READABLE
    }
}
