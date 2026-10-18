//! Serde encoding into one output buffer shared by all LLSD formats.

use crate::{
    Date, Error, Limits, Result,
    value::{DATE_NEWTYPE, URI_NEWTYPE, UUID_NEWTYPE},
};
use base::UUID;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::ser::{
    self, Impossible, Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple, SerializeTupleStruct,
    SerializeTupleVariant,
};
use std::{fmt, io::Write};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Format {
    Xml,
    Binary,
    Notation,
}

const XML_PREFIX: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><llsd>";
const XML_SUFFIX: &[u8] = b"</llsd>";

pub(crate) fn to_vec<T: Serialize + ?Sized>(value: &T, format: Format) -> Result<Vec<u8>> {
    encode(value, format, None)
}
pub(crate) fn to_vec_with_max_bytes<T: Serialize + ?Sized>(value: &T, max_bytes: usize) -> Result<Vec<u8>> {
    encode(value, Format::Xml, Some(max_bytes))
}
fn encode<T: Serialize + ?Sized>(value: &T, format: Format, max_bytes: Option<usize>) -> Result<Vec<u8>> {
    let max_bytes = max_bytes.map(|max| max.checked_sub(XML_SUFFIX.len()).ok_or(Error::LimitExceeded("encoded length"))).transpose()?;
    let mut serializer = Serializer { output: Vec::new(), format, max_bytes, depth: 0 };
    serializer.append(match format {
        Format::Xml => XML_PREFIX,
        Format::Binary => b"<?llsd/binary?>\n",
        Format::Notation => b"<?llsd/notation?>\n",
    })?;
    value.serialize(&mut serializer)?;
    if format == Format::Xml {
        serializer.output.extend_from_slice(XML_SUFFIX);
    }
    Ok(serializer.output)
}
pub(crate) fn to_writer<T: Serialize + ?Sized, W: Write>(mut writer: W, value: &T, format: Format) -> Result<()> {
    writer.write_all(&to_vec(value, format)?)?;
    Ok(())
}

struct Serializer {
    output: Vec<u8>,
    format: Format,
    max_bytes: Option<usize>,
    depth: usize,
}
impl Serializer {
    fn remaining(&self) -> Option<usize> {
        self.max_bytes.map(|max| max.saturating_sub(self.output.len()))
    }
    fn check_space(&self, length: usize) -> Result<()> {
        if self.remaining().is_some_and(|remaining| length > remaining) {
            return Err(Error::LimitExceeded("encoded length"));
        }
        Ok(())
    }
    fn append(&mut self, bytes: &[u8]) -> Result<()> {
        self.check_space(bytes.len())?;
        self.output.extend_from_slice(bytes);
        Ok(())
    }
    fn xml_text(&mut self, tag: &str, text: &str) -> Result<()> {
        let length = xml_tag_overhead(tag).checked_add(xml_escaped_len(text)?).ok_or(Error::LimitExceeded("encoded length"))?;
        self.check_space(length)?;
        self.output.reserve(length);
        self.output.push(b'<');
        self.output.extend_from_slice(tag.as_bytes());
        self.output.push(b'>');
        escape_xml(text, &mut self.output);
        self.output.extend_from_slice(b"</");
        self.output.extend_from_slice(tag.as_bytes());
        self.output.push(b'>');
        Ok(())
    }
    fn binary_sized(&mut self, tag: u8, bytes: &[u8]) -> Result<()> {
        let length = binary_length(bytes.len())?;
        self.append(&[tag])?;
        self.append(&length)?;
        self.append(bytes)
    }
    fn quoted(&mut self, text: &str, quote: u8) -> Result<()> {
        self.append(&[quote])?;
        for &byte in text.as_bytes() {
            match byte {
                b'\\' => self.append(b"\\\\")?,
                b'\n' => self.append(b"\\n")?,
                b'\r' => self.append(b"\\r")?,
                b'\t' => self.append(b"\\t")?,
                byte if byte == quote => self.append(&[b'\\', byte])?,
                0..=0x1f | 0x7f => {
                    const HEX: &[u8] = b"0123456789abcdef";
                    self.append(&[b'\\', b'x', HEX[(byte >> 4) as usize], HEX[(byte & 15) as usize]])?;
                }
                byte => self.append(&[byte])?,
            }
        }
        self.append(&[quote])
    }
    fn string(&mut self, text: &str, kind: TextKind) -> Result<()> {
        match kind {
            TextKind::Uuid => {
                let uuid = UUID::parse_str(text).map_err(|e| Error::InvalidValue(e.to_string()))?;
                match self.format {
                    Format::Xml => self.xml_text("uuid", &uuid.to_string()),
                    Format::Binary => {
                        self.append(b"u")?;
                        self.append(uuid.as_bytes())
                    }
                    Format::Notation => {
                        self.append(b"u")?;
                        self.append(uuid.to_string().as_bytes())
                    }
                }
            }
            TextKind::Date => {
                let date = Date::parse(text)?;
                match self.format {
                    Format::Xml => self.xml_text("date", &date.format()?),
                    Format::Binary => {
                        self.append(b"d")?;
                        self.append(&date.unix_seconds().to_le_bytes())
                    }
                    Format::Notation => {
                        self.append(b"d")?;
                        self.quoted(&date.format()?, b'"')
                    }
                }
            }
            TextKind::String | TextKind::Uri => match self.format {
                Format::Xml => self.xml_text(if matches!(kind, TextKind::Uri) { "uri" } else { "string" }, text),
                Format::Binary => self.binary_sized(if matches!(kind, TextKind::Uri) { b'l' } else { b's' }, text.as_bytes()),
                Format::Notation => {
                    if matches!(kind, TextKind::Uri) {
                        self.append(b"l")?;
                        self.quoted(text, b'"')
                    } else {
                        self.quoted(text, b'\'')
                    }
                }
            },
        }
    }
    fn bytes(&mut self, bytes: &[u8]) -> Result<()> {
        if self.format == Format::Binary {
            return self.binary_sized(b'b', bytes);
        }
        let encoded_len = bytes
            .len()
            .checked_add(2)
            .and_then(|n| n.checked_div(3))
            .and_then(|n| n.checked_mul(4))
            .ok_or(Error::LimitExceeded("encoded length"))?;
        let (prefix, suffix): (&[u8], &[u8]) = if self.format == Format::Xml { (b"<binary>", b"</binary>") } else { (b"b64\"", b"\"") };
        let length = prefix
            .len()
            .checked_add(encoded_len)
            .and_then(|n| n.checked_add(suffix.len()))
            .ok_or(Error::LimitExceeded("encoded length"))?;
        self.check_space(length)?;
        self.output.reserve(length);
        self.output.extend_from_slice(prefix);
        let start = self.output.len();
        self.output.resize(start + encoded_len, 0);
        STANDARD.encode_slice(bytes, &mut self.output[start..]).map_err(|e| Error::InvalidValue(e.to_string()))?;
        self.output.extend_from_slice(suffix);
        Ok(())
    }
    fn open(&mut self, kind: Container, hint: Option<usize>) -> Result<usize> {
        if self.max_bytes.is_some() && self.depth >= Limits::default().max_depth {
            return Err(Error::LimitExceeded("nesting depth"));
        }
        if self.format == Format::Binary {
            binary_length(hint.unwrap_or(0))?;
        }
        let offset = self.output.len();
        match self.format {
            Format::Xml => self.append(match kind {
                Container::Array => b"<array>",
                Container::Map => b"<map>",
            })?,
            Format::Binary => {
                self.append(&[kind.start()])?;
                self.append(&[0; 4])?;
            }
            Format::Notation => self.append(&[kind.start()])?,
        }
        self.depth += 1;
        Ok(offset)
    }
    fn close(&mut self, kind: Container, offset: usize, count: usize) -> Result<()> {
        match self.format {
            Format::Xml => self.append(match kind {
                Container::Array => b"</array>",
                Container::Map => b"</map>",
            }),
            Format::Binary => {
                let length = binary_length(count)?;
                self.output[offset + 1..offset + 5].copy_from_slice(&length);
                self.append(&[kind.end()])
            }
            Format::Notation => self.append(&[kind.end()]),
        }
    }
    fn key(&mut self, key: &str) -> Result<()> {
        match self.format {
            Format::Xml => self.xml_text("key", key),
            Format::Binary => self.binary_sized(b'k', key.as_bytes()),
            Format::Notation => {
                self.quoted(key, b'\'')?;
                self.append(b":")
            }
        }
    }
    fn compound(&mut self, kind: Container, hint: Option<usize>) -> Result<Compound<'_>> {
        let offset = self.open(kind, hint)?;
        Ok(Compound { serializer: self, kind, offset, count: 0, pending_key: false, outer_map: None })
    }
    fn variant(&mut self, name: &str, kind: Container, hint: usize) -> Result<Compound<'_>> {
        let outer = self.open(Container::Map, Some(1))?;
        let result = self.key(name).and_then(|()| self.open(kind, Some(hint)));
        match result {
            Ok(offset) => Ok(Compound { serializer: self, kind, offset, count: 0, pending_key: false, outer_map: Some(outer) }),
            Err(error) => {
                self.depth -= 1;
                Err(error)
            }
        }
    }
}
#[derive(Clone, Copy)]
enum TextKind {
    String,
    Uuid,
    Uri,
    Date,
}

impl<'a> ser::Serializer for &'a mut Serializer {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Compound<'a>;
    type SerializeTuple = Compound<'a>;
    type SerializeTupleStruct = Compound<'a>;
    type SerializeTupleVariant = Compound<'a>;
    type SerializeMap = Compound<'a>;
    type SerializeStruct = Compound<'a>;
    type SerializeStructVariant = Compound<'a>;

    fn serialize_bool(self, value: bool) -> Result<()> {
        match self.format {
            Format::Xml => self.xml_text("boolean", if value { "true" } else { "false" }),
            Format::Binary => self.append(if value { b"1" } else { b"0" }),
            Format::Notation => self.append(if value { b"true" } else { b"false" }),
        }
    }
    fn serialize_i8(self, value: i8) -> Result<()> {
        self.serialize_i32(i32::from(value))
    }
    fn serialize_i16(self, value: i16) -> Result<()> {
        self.serialize_i32(i32::from(value))
    }
    fn serialize_i32(self, value: i32) -> Result<()> {
        match self.format {
            Format::Xml => self.xml_text("integer", &value.to_string()),
            Format::Binary => {
                self.append(b"i")?;
                self.append(&value.to_be_bytes())
            }
            Format::Notation => {
                self.append(b"i")?;
                self.append(value.to_string().as_bytes())
            }
        }
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
        match self.format {
            Format::Xml => self.xml_text("real", &real_text(value)),
            Format::Binary => {
                self.append(b"r")?;
                self.append(&value.to_be_bytes())
            }
            Format::Notation => {
                self.append(b"r")?;
                self.append(real_text(value).as_bytes())
            }
        }
    }
    fn serialize_char(self, value: char) -> Result<()> {
        self.serialize_str(value.encode_utf8(&mut [0; 4]))
    }
    fn serialize_str(self, value: &str) -> Result<()> {
        self.string(value, TextKind::String)
    }
    fn serialize_bytes(self, value: &[u8]) -> Result<()> {
        self.bytes(value)
    }
    fn serialize_none(self) -> Result<()> {
        self.serialize_unit()
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<()> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<()> {
        match self.format {
            Format::Xml => self.xml_text("undef", ""),
            _ => self.append(b"!"),
        }
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
        let text = match self.remaining() {
            Some(remaining) => bounded_display(value, remaining.saturating_sub(xml_tag_overhead("string")))?,
            None => value.to_string(),
        };
        self.serialize_str(&text)
    }
    fn is_human_readable(&self) -> bool {
        self.format != Format::Binary
    }
}
#[derive(Clone, Copy)]
enum Container {
    Array,
    Map,
}
impl Container {
    fn start(self) -> u8 {
        match self {
            Self::Array => b'[',
            Self::Map => b'{',
        }
    }
    fn end(self) -> u8 {
        match self {
            Self::Array => b']',
            Self::Map => b'}',
        }
    }
}
struct Compound<'a> {
    serializer: &'a mut Serializer,
    kind: Container,
    offset: usize,
    count: usize,
    pending_key: bool,
    outer_map: Option<usize>,
}
impl Compound<'_> {
    fn separator(&mut self) -> Result<()> {
        if self.serializer.format == Format::Notation && self.count > 0 {
            self.serializer.append(b",")?;
        }
        Ok(())
    }
    fn element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.separator()?;
        value.serialize(&mut *self.serializer)?;
        self.count = self.count.checked_add(1).ok_or(Error::LimitExceeded("encoded length"))?;
        Ok(())
    }
    fn finish(self) -> Result<()> {
        if self.pending_key {
            return Err(Error::InvalidValue("map key has no value".into()));
        }
        self.serializer.close(self.kind, self.offset, self.count)?;
        if let Some(offset) = self.outer_map {
            self.serializer.close(Container::Map, offset, 1)?;
        }
        Ok(())
    }
}
impl Drop for Compound<'_> {
    fn drop(&mut self) {
        self.serializer.depth -= 1 + usize::from(self.outer_map.is_some());
    }
}
impl SerializeSeq for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.element(value)
    }
    fn end(self) -> Result<()> {
        self.finish()
    }
}
impl SerializeTuple for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.element(value)
    }
    fn end(self) -> Result<()> {
        self.finish()
    }
}
impl SerializeTupleStruct for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.element(value)
    }
    fn end(self) -> Result<()> {
        self.finish()
    }
}
impl SerializeTupleVariant for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.element(value)
    }
    fn end(self) -> Result<()> {
        self.finish()
    }
}
impl SerializeMap for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<()> {
        if self.pending_key {
            return Err(Error::InvalidValue("map key has no value".into()));
        }
        self.separator()?;
        let key =
            key.serialize(KeySerializer { budget: self.serializer.remaining(), human_readable: self.serializer.format != Format::Binary })?;
        self.serializer.key(&key)?;
        self.pending_key = true;
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        if !self.pending_key {
            return Err(Error::InvalidValue("map value has no key".into()));
        }
        value.serialize(&mut *self.serializer)?;
        self.pending_key = false;
        self.count = self.count.checked_add(1).ok_or(Error::LimitExceeded("encoded length"))?;
        Ok(())
    }
    fn end(self) -> Result<()> {
        self.finish()
    }
}
impl SerializeStruct for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        self.separator()?;
        self.serializer.key(key)?;
        value.serialize(&mut *self.serializer)?;
        self.count = self.count.checked_add(1).ok_or(Error::LimitExceeded("encoded length"))?;
        Ok(())
    }
    fn end(self) -> Result<()> {
        self.finish()
    }
}
impl SerializeStructVariant for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        SerializeStruct::serialize_field(self, key, value)
    }
    fn end(self) -> Result<()> {
        self.finish()
    }
}

struct TextSerializer<'a> {
    serializer: &'a mut Serializer,
    kind: TextKind,
}
impl ser::Serializer for TextSerializer<'_> {
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
        let text = match self.serializer.remaining() {
            Some(remaining) => bounded_display(value, remaining.saturating_sub(xml_tag_overhead(tag)))?,
            None => value.to_string(),
        };
        self.serializer.string(&text, self.kind)
    }
    fn is_human_readable(&self) -> bool {
        self.serializer.format != Format::Binary
    }
}
struct KeySerializer {
    budget: Option<usize>,
    human_readable: bool,
}

impl KeySerializer {
    fn owned(self, key: String) -> Result<String> {
        if let Some(budget) = &self.budget {
            charge_xml_key(&key, budget)?;
        }
        Ok(key)
    }
}

impl ser::Serializer for KeySerializer {
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
            charge_xml_key(value, budget)?;
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
            charge_xml_key(variant, budget)?;
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
        match &self.budget {
            Some(budget) => {
                let max_text_len = budget.saturating_sub(xml_tag_overhead("key"));
                self.owned(bounded_display(value, max_text_len)?)
            }
            None => Ok(value.to_string()),
        }
    }
    fn is_human_readable(&self) -> bool {
        self.human_readable
    }
}

fn binary_length(length: usize) -> Result<[u8; 4]> {
    u32::try_from(length).map(u32::to_be_bytes).map_err(|_| Error::LimitExceeded("encoded length"))
}
fn xml_tag_overhead(tag: &str) -> usize {
    5 + tag.len() * 2
}
fn charge_xml_key(key: &str, budget: &usize) -> Result<()> {
    let length = xml_tag_overhead("key").checked_add(xml_escaped_len(key)?).ok_or(Error::LimitExceeded("encoded length"))?;
    if length > *budget {
        return Err(Error::LimitExceeded("encoded length"));
    }
    Ok(())
}
fn xml_escaped_len(text: &str) -> Result<usize> {
    let mut length = 0usize;
    for character in text.chars() {
        if matches!(character, '\u{0}'..='\u{8}' | '\u{b}'..='\u{c}' | '\u{e}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}') {
            return Err(Error::InvalidValue("text contains a character forbidden in XML 1.0".into()));
        }
        let escaped_len = match character {
            '&' => 5,
            '<' | '>' => 4,
            '"' | '\'' => 6,
            '\r' => 5,
            value => value.len_utf8(),
        };
        length = length.checked_add(escaped_len).ok_or(Error::LimitExceeded("encoded length"))?;
    }
    Ok(length)
}

fn bounded_display<T: fmt::Display + ?Sized>(value: &T, max_encoded_len: usize) -> Result<String> {
    let mut output = BoundedDisplay { value: String::new(), encoded_len: 0, max_encoded_len, error: None };
    let formatted = fmt::write(&mut output, format_args!("{value}"));
    if let Some(error) = output.error.take() {
        return Err(error);
    }
    if formatted.is_err() {
        return Err(Error::InvalidValue("string formatting failed".into()));
    }
    Ok(output.value)
}

struct BoundedDisplay {
    value: String,
    encoded_len: usize,
    max_encoded_len: usize,
    error: Option<Error>,
}

impl fmt::Write for BoundedDisplay {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let length = match xml_escaped_len(text) {
            Ok(length) => length,
            Err(error) => {
                self.error = Some(error);
                return Err(fmt::Error);
            }
        };
        let Some(encoded_len) = self.encoded_len.checked_add(length) else {
            self.error = Some(Error::LimitExceeded("encoded length"));
            return Err(fmt::Error);
        };
        if encoded_len > self.max_encoded_len {
            self.error = Some(Error::LimitExceeded("encoded length"));
            return Err(fmt::Error);
        }
        self.value.push_str(text);
        self.encoded_len = encoded_len;
        Ok(())
    }
}

fn escape_xml(text: &str, output: &mut Vec<u8>) {
    for character in text.chars() {
        match character {
            '&' => output.extend_from_slice(b"&amp;"),
            '<' => output.extend_from_slice(b"&lt;"),
            '>' => output.extend_from_slice(b"&gt;"),
            '\"' => output.extend_from_slice(b"&quot;"),
            '\'' => output.extend_from_slice(b"&apos;"),
            '\r' => output.extend_from_slice(b"&#13;"),
            value => {
                let mut bytes = [0; 4];
                output.extend_from_slice(value.encode_utf8(&mut bytes).as_bytes());
            }
        }
    }
}

fn real_text(value: f64) -> String {
    if value.is_nan() {
        "nan".into()
    } else if value == f64::INFINITY {
        "inf".into()
    } else if value == f64::NEG_INFINITY {
        "-inf".into()
    } else {
        value.to_string()
    }
}
