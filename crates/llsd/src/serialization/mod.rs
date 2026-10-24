mod compound;
mod key;
mod output;
mod text;

pub(crate) use self::output::Output;

use std::{fmt, io::Write, marker::PhantomData};

use serde::ser::{self, Serialize, SerializeMap};
use uuid::Uuid;

use crate::{
    Date, Error, Limits, Result,
    value::{DATE_NEWTYPE, URI_NEWTYPE, UUID_NEWTYPE},
};

use self::{
    compound::Compound,
    text::{TextKind, TextSerializer},
};

#[derive(Clone, Copy)]
pub(crate) enum Container {
    Array,
    Map,
}

impl Container {
    pub(crate) fn start(self) -> u8 {
        match self {
            Self::Array => b'[',
            Self::Map => b'{',
        }
    }

    pub(crate) fn end(self) -> u8 {
        match self {
            Self::Array => b']',
            Self::Map => b'}',
        }
    }
}

pub(crate) enum Text<'a> {
    String(&'a str),
    Uri(&'a str),
    Uuid(Uuid),
    Date(Date),
}

pub(crate) trait Format {
    const PREFIX: &'static [u8];
    const SUFFIX: &'static [u8] = b"";
    const HUMAN_READABLE: bool = true;

    fn boolean(output: &mut Output, value: bool) -> Result<()>;
    fn integer(output: &mut Output, value: i32) -> Result<()>;
    fn real(output: &mut Output, value: f64) -> Result<()>;
    fn undefined(output: &mut Output) -> Result<()>;
    fn text(output: &mut Output, value: Text<'_>) -> Result<()>;
    fn bytes(output: &mut Output, value: &[u8]) -> Result<()>;
    fn open(output: &mut Output, kind: Container, hint: Option<usize>) -> Result<usize>;
    fn close(output: &mut Output, kind: Container, offset: usize, count: usize) -> Result<()>;
    fn key(output: &mut Output, key: &str) -> Result<()>;

    fn separator(_output: &mut Output, _count: usize) -> Result<()> {
        Ok(())
    }

    fn charge_key(_key: &str, _budget: usize) -> Result<()> {
        Ok(())
    }

    fn display<T: fmt::Display + ?Sized>(value: &T, _tag: &str, _budget: Option<usize>) -> Result<String> {
        Ok(value.to_string())
    }
}

struct Serializer<F> {
    output: Output,
    depth: usize,
    _format: PhantomData<F>,
}

impl<F: Format> Serializer<F> {
    fn string(&mut self, text: &str, kind: TextKind) -> Result<()> {
        let value = match kind {
            TextKind::String => Text::String(text),
            TextKind::Uri => Text::Uri(text),
            TextKind::Uuid => Text::Uuid(Uuid::parse_str(text).map_err(|e| Error::InvalidValue(e.to_string()))?),
            TextKind::Date => Text::Date(Date::parse(text)?),
        };
        F::text(&mut self.output, value)
    }

    fn open(&mut self, kind: Container, hint: Option<usize>) -> Result<usize> {
        if self.depth >= Limits::default().max_depth {
            return Err(Error::LimitExceeded("nesting depth"));
        }
        let offset = F::open(&mut self.output, kind, hint)?;
        self.depth += 1;
        Ok(offset)
    }

    fn compound(&mut self, kind: Container, hint: Option<usize>) -> Result<Compound<'_, F>> {
        let offset = self.open(kind, hint)?;
        Ok(Compound { serializer: self, kind, offset, count: 0, pending_key: false, outer_map: None })
    }

    fn variant(&mut self, name: &str, kind: Container, hint: usize) -> Result<Compound<'_, F>> {
        let outer = self.open(Container::Map, Some(1))?;
        let result = F::key(&mut self.output, name).and_then(|()| self.open(kind, Some(hint)));
        match result {
            Ok(offset) => Ok(Compound { serializer: self, kind, offset, count: 0, pending_key: false, outer_map: Some(outer) }),
            Err(error) => {
                self.depth -= 1;
                Err(error)
            }
        }
    }
}

impl<'a, F: Format> ser::Serializer for &'a mut Serializer<F> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Compound<'a, F>;
    type SerializeTuple = Compound<'a, F>;
    type SerializeTupleStruct = Compound<'a, F>;
    type SerializeTupleVariant = Compound<'a, F>;
    type SerializeMap = Compound<'a, F>;
    type SerializeStruct = Compound<'a, F>;
    type SerializeStructVariant = Compound<'a, F>;

    fn serialize_bool(self, value: bool) -> Result<()> {
        F::boolean(&mut self.output, value)
    }

    fn serialize_i8(self, value: i8) -> Result<()> {
        self.serialize_i32(i32::from(value))
    }

    fn serialize_i16(self, value: i16) -> Result<()> {
        self.serialize_i32(i32::from(value))
    }

    fn serialize_i32(self, value: i32) -> Result<()> {
        F::integer(&mut self.output, value)
    }

    fn serialize_i64(self, value: i64) -> Result<()> {
        self.serialize_i32(i32::try_from(value).map_err(|_| Error::InvalidValue("integer is outside the LLSD i32 range".into()))?)
    }

    fn serialize_u8(self, value: u8) -> Result<()> {
        self.serialize_i32(i32::from(value))
    }

    fn serialize_u16(self, value: u16) -> Result<()> {
        self.serialize_i32(i32::from(value))
    }

    fn serialize_u32(self, value: u32) -> Result<()> {
        self.serialize_u64(u64::from(value))
    }

    fn serialize_u64(self, value: u64) -> Result<()> {
        self.serialize_i32(i32::try_from(value).map_err(|_| Error::InvalidValue("integer is outside the LLSD i32 range".into()))?)
    }

    fn serialize_f32(self, value: f32) -> Result<()> {
        self.serialize_f64(f64::from(value))
    }

    fn serialize_f64(self, value: f64) -> Result<()> {
        F::real(&mut self.output, value)
    }

    fn serialize_char(self, value: char) -> Result<()> {
        self.serialize_str(value.encode_utf8(&mut [0; 4]))
    }

    fn serialize_str(self, value: &str) -> Result<()> {
        self.string(value, TextKind::String)
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<()> {
        F::bytes(&mut self.output, value)
    }

    fn serialize_none(self) -> Result<()> {
        self.serialize_unit()
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<()> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<()> {
        F::undefined(&mut self.output)
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result<()> {
        self.serialize_unit()
    }

    fn serialize_unit_variant(self, _: &'static str, _: u32, variant: &'static str) -> Result<()> {
        self.serialize_str(variant)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, name: &'static str, value: &T) -> Result<()> {
        let kind = match name {
            UUID_NEWTYPE => TextKind::Uuid,
            URI_NEWTYPE => TextKind::Uri,
            DATE_NEWTYPE => TextKind::Date,
            _ => return value.serialize(self),
        };
        value.serialize(TextSerializer { serializer: self, kind })
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(self, _: &'static str, _: u32, variant: &'static str, value: &T) -> Result<()> {
        let mut map = self.compound(Container::Map, Some(1))?;
        SerializeMap::serialize_entry(&mut map, variant, value)?;
        SerializeMap::end(map)
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq> {
        self.compound(Container::Array, len)
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<Self::SerializeTupleStruct> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(self, _: &'static str, _: u32, variant: &'static str, len: usize) -> Result<Self::SerializeTupleVariant> {
        self.variant(variant, Container::Array, len)
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap> {
        self.compound(Container::Map, len)
    }

    fn serialize_struct(self, _: &'static str, len: usize) -> Result<Self::SerializeStruct> {
        self.serialize_map(Some(len))
    }

    fn serialize_struct_variant(self, _: &'static str, _: u32, variant: &'static str, len: usize) -> Result<Self::SerializeStructVariant> {
        self.variant(variant, Container::Map, len)
    }

    fn collect_str<T: fmt::Display + ?Sized>(self, value: &T) -> Result<()> {
        let text = F::display(value, "string", self.output.remaining())?;
        self.serialize_str(&text)
    }

    fn is_human_readable(&self) -> bool {
        F::HUMAN_READABLE
    }
}

pub(crate) fn to_vec<F: Format, T: Serialize + ?Sized>(value: &T, max_bytes: Option<usize>) -> Result<Vec<u8>> {
    let mut serializer = Serializer::<F> { output: Output::new(max_bytes), depth: 0, _format: PhantomData };
    serializer.output.append(F::PREFIX)?;
    value.serialize(&mut serializer)?;
    serializer.output.append(F::SUFFIX)?;
    Ok(serializer.output.bytes)
}

pub(crate) fn to_writer<F: Format, T: Serialize + ?Sized, W: Write>(mut writer: W, value: &T) -> Result<()> {
    writer.write_all(&to_vec::<F, _>(value, None)?)?;
    Ok(())
}
