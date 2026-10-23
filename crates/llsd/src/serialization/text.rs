use std::fmt;

use serde::ser::{self, Impossible, Serialize};

use crate::{
    Error, Result,
    value::{DATE_NEWTYPE, URI_NEWTYPE, UUID_NEWTYPE},
};

use super::{Format, Serializer};

#[derive(Clone, Copy)]
pub(super) enum TextKind {
    String,
    Uuid,
    Uri,
    Date,
}

pub(super) struct TextSerializer<'a, F> {
    pub(super) serializer: &'a mut Serializer<F>,
    pub(super) kind: TextKind,
}

impl<F: Format> ser::Serializer for TextSerializer<'_, F> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Impossible<(), Error>;
    type SerializeTuple = Impossible<(), Error>;
    type SerializeTupleStruct = Impossible<(), Error>;
    type SerializeTupleVariant = Impossible<(), Error>;
    type SerializeMap = Impossible<(), Error>;
    type SerializeStruct = Impossible<(), Error>;
    type SerializeStructVariant = Impossible<(), Error>;

    fn serialize_str(self, value: &str) -> Result<()> {
        self.serializer.string(value, self.kind)
    }

    fn serialize_char(self, value: char) -> Result<()> {
        self.serialize_str(value.encode_utf8(&mut [0; 4]))
    }

    fn serialize_unit(self) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_unit_variant(self, _: &'static str, _: u32, variant: &'static str) -> Result<()> {
        self.serialize_str(variant)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, name: &'static str, value: &T) -> Result<()> {
        if matches!(name, UUID_NEWTYPE | URI_NEWTYPE | DATE_NEWTYPE) {
            return Err(Error::InvalidValue("expected a string payload".into()));
        }
        value.serialize(self)
    }

    fn serialize_bool(self, _: bool) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_i8(self, _: i8) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_i16(self, _: i16) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_i32(self, _: i32) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_i64(self, _: i64) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_u8(self, _: u8) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_u16(self, _: u16) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_u32(self, _: u32) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_u64(self, _: u64) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_f32(self, _: f32) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_f64(self, _: f64) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_bytes(self, _: &[u8]) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_none(self) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<()> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(self, _: &'static str, _: u32, _: &'static str, _: &T) -> Result<()> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeTupleStruct> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_tuple_variant(self, _: &'static str, _: u32, _: &'static str, _: usize) -> Result<Self::SerializeTupleVariant> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeStruct> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn serialize_struct_variant(self, _: &'static str, _: u32, _: &'static str, _: usize) -> Result<Self::SerializeStructVariant> {
        Err(Error::InvalidValue("expected a string payload".into()))
    }

    fn collect_str<T: fmt::Display + ?Sized>(self, value: &T) -> Result<()> {
        let tag = match self.kind {
            TextKind::String => "string",
            TextKind::Uuid => "uuid",
            TextKind::Uri => "uri",
            TextKind::Date => "date",
        };
        let text = F::display(value, tag, self.serializer.output.remaining())?;
        self.serializer.string(&text, self.kind)
    }

    fn is_human_readable(&self) -> bool {
        F::HUMAN_READABLE
    }
}
