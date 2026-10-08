use std::{cell::Cell, fmt, io::Write, rc::Rc};

use crate::{
    Error, Result,
    value::{BINARY_NEWTYPE, DATE_NEWTYPE, URI_NEWTYPE, UUID_NEWTYPE},
};
use base::UUID;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::ser::{
    self, Impossible, Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple, SerializeTupleStruct,
    SerializeTupleVariant,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Format {
    Xml,
    Binary,
    Notation,
}

pub(crate) fn to_vec<T: Serialize + ?Sized>(value: &T, format: Format) -> Result<Vec<u8>> {
    let value = value.serialize(Serializer { format, budget: None, nesting: None })?;
    match format {
        Format::Xml => {
            let mut output = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><llsd>".to_vec();
            output.extend_from_slice(&value);
            output.extend_from_slice(b"</llsd>");
            Ok(output)
        }
        Format::Binary => {
            let mut output = b"<?llsd/binary?>\n".to_vec();
            output.extend_from_slice(&value);
            Ok(output)
        }
        Format::Notation => {
            let mut output = b"<?llsd/notation?>\n".to_vec();
            output.extend_from_slice(&value);
            Ok(output)
        }
    }
}

pub(crate) fn to_vec_with_max_bytes<T: Serialize + ?Sized>(value: &T, max_bytes: usize) -> Result<Vec<u8>> {
    const PREFIX: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><llsd>";
    const SUFFIX: &[u8] = b"</llsd>";

    let envelope_len = PREFIX.len() + SUFFIX.len();
    let remaining = max_bytes.checked_sub(envelope_len).ok_or(Error::LimitExceeded("encoded length"))?;
    let budget = Rc::new(Cell::new(remaining));
    let value = value.serialize(Serializer { format: Format::Xml, budget: Some(budget), nesting: Some(Rc::new(Cell::new(0))) })?;

    let mut output = Vec::with_capacity(envelope_len + value.len());
    output.extend_from_slice(PREFIX);
    output.extend_from_slice(&value);
    output.extend_from_slice(SUFFIX);
    Ok(output)
}

pub(crate) fn to_writer<T: Serialize + ?Sized, W: Write>(mut writer: W, value: &T, format: Format) -> Result<()> {
    writer.write_all(&to_vec(value, format)?)?;
    Ok(())
}

struct Serializer {
    format: Format,
    budget: Option<Rc<Cell<usize>>>,
    nesting: Option<Rc<Cell<usize>>>,
}

impl Serializer {
    fn scalar(&self, tag: &str, text: &str) -> Result<Vec<u8>> {
        if !matches!(tag, "undef" | "boolean" | "integer" | "real" | "uuid" | "string" | "date" | "uri") {
            return Err(Error::InvalidValue(format!("unsupported LLSD scalar tag '{tag}'")));
        }
        if tag == "boolean" && !matches!(text, "true" | "false") {
            return Err(Error::InvalidValue("invalid LLSD boolean".into()));
        }

        match self.format {
            Format::Xml => match &self.budget {
                Some(budget) => xml_scalar_bounded(tag, text, budget),
                None => xml_scalar(tag, text),
            },
            Format::Binary => match tag {
                "undef" => Ok(vec![b'!']),
                "boolean" => Ok(vec![if text == "true" { b'1' } else { b'0' }]),
                "integer" => {
                    let value = text.parse::<i32>().map_err(|error| Error::InvalidValue(format!("invalid LLSD integer: {error}")))?;
                    Ok(binary_fixed(b'i', &value.to_be_bytes()))
                }
                "real" => Ok(binary_fixed(b'r', &parse_real(text)?.to_bits().to_be_bytes())),
                "date" => {
                    let value = crate::Date::parse(text)?.unix_seconds();
                    Ok(binary_fixed(b'd', &value.to_bits().to_le_bytes()))
                }
                "uuid" => {
                    let uuid = UUID::parse_str(text).map_err(|error| Error::InvalidValue(error.to_string()))?;
                    let mut output = vec![b'u'];
                    output.extend_from_slice(uuid.as_bytes());
                    Ok(output)
                }
                "string" => binary_sized(b's', text.as_bytes()),
                "uri" => binary_sized(b'l', text.as_bytes()),
                _ => Err(Error::InvalidValue(format!("unsupported LLSD scalar tag '{tag}'"))),
            },
            Format::Notation => match tag {
                "undef" => Ok(b"!".to_vec()),
                "boolean" => Ok(text.as_bytes().to_vec()),
                "integer" => Ok(format!("i{text}").into_bytes()),
                "real" => Ok(format!("r{text}").into_bytes()),
                "date" => Ok(notation_typed_quoted(b'd', text)),
                "uuid" => Ok(format!("u{text}").into_bytes()),
                "string" => Ok(notation_string(text)),
                "uri" => Ok(notation_typed_quoted(b'l', text)),
                _ => Err(Error::InvalidValue(format!("unsupported LLSD scalar tag '{tag}'"))),
            },
        }
    }

    fn bytes(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        match self.format {
            Format::Xml => match &self.budget {
                Some(budget) => xml_binary_bounded(bytes, budget),
                None => xml_scalar("binary", &STANDARD.encode(bytes)),
            },
            Format::Binary => binary_sized(b'b', bytes),
            Format::Notation => {
                let mut output = b"b64\"".to_vec();
                output.extend_from_slice(STANDARD.encode(bytes).as_bytes());
                output.push(b'"');
                Ok(output)
            }
        }
    }

    fn compound(&self, kind: CompoundKind, values: Vec<Vec<u8>>, keys: Vec<String>) -> Result<Vec<u8>> {
        match (self.format, kind) {
            (Format::Xml, CompoundKind::Array) => match &self.budget {
                Some(budget) => xml_container_bounded("array", values, budget),
                None => Ok(xml_container("array", values)),
            },
            (Format::Xml, CompoundKind::Map) => match &self.budget {
                Some(budget) => xml_map_bounded(values, keys, budget),
                None => xml_map(values, keys),
            },
            (Format::Binary, CompoundKind::Array) => binary_container(b'[', b']', values),
            (Format::Binary, CompoundKind::Map) => binary_map(values, keys),
            (Format::Notation, CompoundKind::Array) => Ok(notation_container(b'[', b']', values)),
            (Format::Notation, CompoundKind::Map) => Ok(notation_map(values, keys)),
        }
    }

    fn special_newtype(&self, name: &str, value: Vec<u8>, charged_to_budget: bool) -> Result<Vec<u8>> {
        if name == BINARY_NEWTYPE {
            return Ok(value);
        }
        if !matches!(name, UUID_NEWTYPE | URI_NEWTYPE | DATE_NEWTYPE) {
            return Ok(value);
        }
        let text = match self.format {
            Format::Xml => extract_xml_string(&value)?,
            Format::Binary => extract_binary_string(&value)?,
            Format::Notation => extract_notation_string(&value)?,
        };
        if charged_to_budget && self.budget.is_some() {
            self.refund(value.len())?;
        }
        if name == UUID_NEWTYPE {
            let uuid = UUID::parse_str(&text).map_err(|error| Error::InvalidValue(error.to_string()))?;
            return match self.format {
                Format::Xml => self.scalar("uuid", &uuid.to_string()),
                Format::Binary => {
                    let mut output = vec![b'u'];
                    output.extend_from_slice(uuid.as_bytes());
                    Ok(output)
                }
                Format::Notation => Ok(format!("u{uuid}").into_bytes()),
            };
        }
        if name == URI_NEWTYPE {
            return match self.format {
                Format::Xml => self.scalar("uri", &text),
                Format::Binary => binary_sized(b'l', text.as_bytes()),
                Format::Notation => Ok(notation_typed_quoted(b'l', &text)),
            };
        }
        if name == DATE_NEWTYPE {
            let date = crate::Date::parse(&text)?;
            return match self.format {
                Format::Xml => self.scalar("date", &date.format()?),
                Format::Binary => Ok(binary_fixed(b'd', &date.unix_seconds().to_bits().to_le_bytes())),
                Format::Notation => Ok(notation_typed_quoted(b'd', &date.format()?)),
            };
        }
        Ok(value)
    }

    fn child(&self) -> Self {
        Self { format: self.format, budget: self.budget.clone(), nesting: self.nesting.clone() }
    }

    fn enter_container(&self) -> Result<Option<NestingGuard>> {
        let Some(nesting) = &self.nesting else {
            return Ok(None);
        };
        let depth = nesting.get().checked_add(1).ok_or(Error::LimitExceeded("nesting depth"))?;
        if depth > crate::Limits::default().max_depth {
            return Err(Error::LimitExceeded("nesting depth"));
        }
        nesting.set(depth);
        Ok(Some(NestingGuard(Rc::clone(nesting))))
    }

    fn refund(&self, length: usize) -> Result<()> {
        if let Some(budget) = &self.budget {
            let remaining = budget.get().checked_add(length).ok_or(Error::LimitExceeded("encoded length"))?;
            budget.set(remaining);
        }
        Ok(())
    }
}

struct NestingGuard(Rc<Cell<usize>>);

impl Drop for NestingGuard {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

impl ser::Serializer for Serializer {
    type Ok = Vec<u8>;
    type Error = Error;
    type SerializeSeq = Compound;
    type SerializeTuple = Compound;
    type SerializeTupleStruct = Compound;
    type SerializeTupleVariant = VariantCompound;
    type SerializeMap = Compound;
    type SerializeStruct = Compound;
    type SerializeStructVariant = VariantCompound;

    fn serialize_bool(self, value: bool) -> Result<Self::Ok> {
        self.scalar("boolean", if value { "true" } else { "false" })
    }

    fn serialize_i8(self, value: i8) -> Result<Self::Ok> {
        self.serialize_i32(i32::from(value))
    }

    fn serialize_i16(self, value: i16) -> Result<Self::Ok> {
        self.serialize_i32(i32::from(value))
    }

    fn serialize_i32(self, value: i32) -> Result<Self::Ok> {
        self.scalar("integer", &value.to_string())
    }

    fn serialize_i64(self, value: i64) -> Result<Self::Ok> {
        self.serialize_i32(i32::try_from(value).map_err(|_| Error::InvalidValue("integer is outside the LLSD i32 range".into()))?)
    }

    fn serialize_u8(self, value: u8) -> Result<Self::Ok> {
        self.serialize_i32(i32::from(value))
    }

    fn serialize_u16(self, value: u16) -> Result<Self::Ok> {
        self.serialize_i32(i32::from(value))
    }

    fn serialize_u32(self, value: u32) -> Result<Self::Ok> {
        self.serialize_i32(i32::try_from(value).map_err(|_| Error::InvalidValue("integer is outside the LLSD i32 range".into()))?)
    }

    fn serialize_u64(self, value: u64) -> Result<Self::Ok> {
        self.serialize_i32(i32::try_from(value).map_err(|_| Error::InvalidValue("integer is outside the LLSD i32 range".into()))?)
    }

    fn serialize_f32(self, value: f32) -> Result<Self::Ok> {
        self.serialize_f64(f64::from(value))
    }

    fn serialize_f64(self, value: f64) -> Result<Self::Ok> {
        self.scalar("real", &real_text(value))
    }

    fn serialize_char(self, value: char) -> Result<Self::Ok> {
        self.serialize_str(&value.to_string())
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok> {
        self.scalar("string", value)
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<Self::Ok> {
        self.bytes(value)
    }

    fn serialize_none(self) -> Result<Self::Ok> {
        self.serialize_unit()
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Self::Ok> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok> {
        self.scalar("undef", "")
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result<Self::Ok> {
        self.serialize_unit()
    }

    fn serialize_unit_variant(self, _: &'static str, _: u32, variant: &'static str) -> Result<Self::Ok> {
        self.serialize_str(variant)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, name: &'static str, value: &T) -> Result<Self::Ok> {
        if self.format == Format::Xml
            && matches!(name, UUID_NEWTYPE | URI_NEWTYPE | DATE_NEWTYPE)
            && let Some(budget) = &self.budget
        {
            let scratch_limit = budget.get().saturating_add(xml_tag_overhead("string"));
            let scratch = Rc::new(Cell::new(scratch_limit));
            let encoded = value.serialize(Serializer { format: self.format, budget: Some(scratch), nesting: self.nesting.clone() })?;
            return self.special_newtype(name, encoded, false);
        }
        let encoded = value.serialize(self.child())?;
        self.special_newtype(name, encoded, true)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok> {
        let depth = self.enter_container()?;
        let mut compound = Compound::map(self.format, 1, self.budget.clone(), self.nesting.clone(), depth);
        if let Some(budget) = &self.budget {
            charge_xml_key(variant, budget)?;
        }
        compound.push(variant.to_owned(), value.serialize(self.child())?);
        compound.finish()
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq> {
        let depth = self.enter_container()?;
        let capacity = len.unwrap_or(0);
        validate_binary_length(self.format, capacity)?;
        Ok(Compound::sequence(self.format, capacity, self.budget, self.nesting, depth))
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple> {
        let depth = self.enter_container()?;
        validate_binary_length(self.format, len)?;
        Ok(Compound::sequence(self.format, len, self.budget, self.nesting, depth))
    }

    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<Self::SerializeTupleStruct> {
        self.serialize_tuple(len)
    }

    fn serialize_tuple_variant(self, _: &'static str, _: u32, variant: &'static str, len: usize) -> Result<Self::SerializeTupleVariant> {
        let map_depth = self.enter_container()?;
        let value_depth = self.enter_container()?;
        validate_binary_length(self.format, len)?;
        Ok(VariantCompound {
            format: self.format,
            name: variant,
            values: bounded_capacity(self.budget.as_ref(), len),
            keys: Vec::new(),
            kind: CompoundKind::Array,
            budget: self.budget,
            nesting: self.nesting,
            _map_depth: map_depth,
            _value_depth: value_depth,
        })
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap> {
        let depth = self.enter_container()?;
        let capacity = len.unwrap_or(0);
        validate_binary_length(self.format, capacity)?;
        Ok(Compound::map(self.format, capacity, self.budget, self.nesting, depth))
    }

    fn serialize_struct(self, _: &'static str, len: usize) -> Result<Self::SerializeStruct> {
        let depth = self.enter_container()?;
        validate_binary_length(self.format, len)?;
        Ok(Compound::map(self.format, len, self.budget, self.nesting, depth))
    }

    fn serialize_struct_variant(self, _: &'static str, _: u32, variant: &'static str, len: usize) -> Result<Self::SerializeStructVariant> {
        let map_depth = self.enter_container()?;
        let value_depth = self.enter_container()?;
        validate_binary_length(self.format, len)?;
        Ok(VariantCompound {
            format: self.format,
            name: variant,
            values: bounded_capacity(self.budget.as_ref(), len),
            keys: bounded_capacity(self.budget.as_ref(), len),
            kind: CompoundKind::Map,
            budget: self.budget,
            nesting: self.nesting,
            _map_depth: map_depth,
            _value_depth: value_depth,
        })
    }

    fn collect_str<T: fmt::Display + ?Sized>(self, value: &T) -> Result<Self::Ok> {
        match &self.budget {
            Some(budget) => {
                let max_text_len = budget.get().saturating_sub(xml_tag_overhead("string"));
                let text = bounded_display(value, max_text_len)?;
                self.serialize_str(&text)
            }
            None => self.serialize_str(&value.to_string()),
        }
    }

    fn is_human_readable(&self) -> bool {
        self.format != Format::Binary
    }
}

#[derive(Clone, Copy)]
enum CompoundKind {
    Array,
    Map,
}

pub(crate) struct Compound {
    format: Format,
    kind: CompoundKind,
    values: Vec<Vec<u8>>,
    keys: Vec<String>,
    pending_key: Option<String>,
    budget: Option<Rc<Cell<usize>>>,
    nesting: Option<Rc<Cell<usize>>>,
    depth: Option<NestingGuard>,
}

impl Compound {
    fn sequence(
        format: Format,
        capacity: usize,
        budget: Option<Rc<Cell<usize>>>,
        nesting: Option<Rc<Cell<usize>>>,
        depth: Option<NestingGuard>,
    ) -> Self {
        Self {
            format,
            kind: CompoundKind::Array,
            values: bounded_capacity(budget.as_ref(), capacity),
            keys: Vec::new(),
            pending_key: None,
            budget,
            nesting,
            depth,
        }
    }

    fn map(
        format: Format,
        capacity: usize,
        budget: Option<Rc<Cell<usize>>>,
        nesting: Option<Rc<Cell<usize>>>,
        depth: Option<NestingGuard>,
    ) -> Self {
        Self {
            format,
            kind: CompoundKind::Map,
            values: bounded_capacity(budget.as_ref(), capacity),
            keys: bounded_capacity(budget.as_ref(), capacity),
            pending_key: None,
            budget,
            nesting,
            depth,
        }
    }

    fn push(&mut self, key: String, value: Vec<u8>) {
        self.keys.push(key);
        self.values.push(value);
    }

    fn finish(self) -> Result<Vec<u8>> {
        let Self { format, kind, values, keys, budget, nesting, depth, .. } = self;
        let result = Serializer { format, budget, nesting }.compound(kind, values, keys);
        drop(depth);
        result
    }
}

impl SerializeSeq for Compound {
    type Ok = Vec<u8>;
    type Error = Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.values.push(value.serialize(Serializer {
            format: self.format,
            budget: self.budget.clone(),
            nesting: self.nesting.clone(),
        })?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok> {
        self.finish()
    }
}

impl SerializeTuple for Compound {
    type Ok = Vec<u8>;
    type Error = Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<Self::Ok> {
        self.finish()
    }
}

impl SerializeTupleStruct for Compound {
    type Ok = Vec<u8>;
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<Self::Ok> {
        self.finish()
    }
}

impl SerializeMap for Compound {
    type Ok = Vec<u8>;
    type Error = Error;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<()> {
        if self.pending_key.is_some() {
            return Err(Error::InvalidValue("map key has no value".into()));
        }
        self.pending_key =
            Some(key.serialize(KeySerializer { budget: self.budget.clone(), human_readable: self.format != Format::Binary })?);
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        let key = self.pending_key.take().ok_or_else(|| Error::InvalidValue("map value has no key".into()))?;
        self.push(key, value.serialize(Serializer { format: self.format, budget: self.budget.clone(), nesting: self.nesting.clone() })?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok> {
        if self.pending_key.is_some() {
            return Err(Error::InvalidValue("map key has no value".into()));
        }
        self.finish()
    }
}

impl SerializeStruct for Compound {
    type Ok = Vec<u8>;
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        if let Some(budget) = &self.budget {
            charge_xml_key(key, budget)?;
        }
        self.push(
            key.to_owned(),
            value.serialize(Serializer { format: self.format, budget: self.budget.clone(), nesting: self.nesting.clone() })?,
        );
        Ok(())
    }

    fn end(self) -> Result<Self::Ok> {
        self.finish()
    }
}

pub(crate) struct VariantCompound {
    format: Format,
    name: &'static str,
    values: Vec<Vec<u8>>,
    keys: Vec<String>,
    kind: CompoundKind,
    budget: Option<Rc<Cell<usize>>>,
    nesting: Option<Rc<Cell<usize>>>,
    _map_depth: Option<NestingGuard>,
    _value_depth: Option<NestingGuard>,
}

impl VariantCompound {
    fn finish(self) -> Result<Vec<u8>> {
        let Self { format, name, values, keys, kind, budget, nesting, _map_depth, _value_depth } = self;
        let value = Serializer { format, budget: budget.clone(), nesting: nesting.clone() }.compound(kind, values, keys)?;
        if let Some(budget) = &budget {
            charge_xml_key(name, budget)?;
        }
        let mut map = Compound::map(format, 1, budget, nesting, None);
        map.push(name.to_owned(), value);
        let result = map.finish();
        drop(_value_depth);
        drop(_map_depth);
        result
    }
}

impl SerializeTupleVariant for VariantCompound {
    type Ok = Vec<u8>;
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.values.push(value.serialize(Serializer {
            format: self.format,
            budget: self.budget.clone(),
            nesting: self.nesting.clone(),
        })?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok> {
        self.finish()
    }
}

impl SerializeStructVariant for VariantCompound {
    type Ok = Vec<u8>;
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        if let Some(budget) = &self.budget {
            charge_xml_key(key, budget)?;
        }
        self.keys.push(key.to_owned());
        self.values.push(value.serialize(Serializer {
            format: self.format,
            budget: self.budget.clone(),
            nesting: self.nesting.clone(),
        })?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok> {
        self.finish()
    }
}

struct KeySerializer {
    budget: Option<Rc<Cell<usize>>>,
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
                let max_text_len = budget.get().saturating_sub(xml_tag_overhead("key"));
                self.owned(bounded_display(value, max_text_len)?)
            }
            None => Ok(value.to_string()),
        }
    }
    fn is_human_readable(&self) -> bool {
        self.human_readable
    }
}

fn xml_scalar(tag: &str, text: &str) -> Result<Vec<u8>> {
    let length = xml_tag_overhead(tag).checked_add(xml_escaped_len(text)?).ok_or(Error::LimitExceeded("encoded length"))?;
    let mut output = Vec::with_capacity(length);
    output.push(b'<');
    output.extend_from_slice(tag.as_bytes());
    output.push(b'>');
    escape_xml(text, &mut output);
    output.extend_from_slice(b"</");
    output.extend_from_slice(tag.as_bytes());
    output.push(b'>');
    Ok(output)
}

fn xml_scalar_bounded(tag: &str, text: &str, budget: &Cell<usize>) -> Result<Vec<u8>> {
    let length = xml_tag_overhead(tag).checked_add(xml_escaped_len(text)?).ok_or(Error::LimitExceeded("encoded length"))?;
    charge_budget(budget, length)?;

    let mut output = Vec::with_capacity(length);
    output.push(b'<');
    output.extend_from_slice(tag.as_bytes());
    output.push(b'>');
    escape_xml(text, &mut output);
    output.extend_from_slice(b"</");
    output.extend_from_slice(tag.as_bytes());
    output.push(b'>');
    Ok(output)
}

fn xml_binary_bounded(bytes: &[u8], budget: &Cell<usize>) -> Result<Vec<u8>> {
    let encoded_len = bytes
        .len()
        .checked_add(2)
        .and_then(|length| length.checked_div(3))
        .and_then(|groups| groups.checked_mul(4))
        .ok_or(Error::LimitExceeded("encoded length"))?;
    let prefix = b"<binary>";
    let suffix = b"</binary>";
    let length = prefix
        .len()
        .checked_add(encoded_len)
        .and_then(|length| length.checked_add(suffix.len()))
        .ok_or(Error::LimitExceeded("encoded length"))?;
    charge_budget(budget, length)?;

    let mut output = vec![0; length];
    output[..prefix.len()].copy_from_slice(prefix);
    let encoded_end = prefix.len() + encoded_len;
    STANDARD.encode_slice(bytes, &mut output[prefix.len()..encoded_end]).map_err(|error| Error::InvalidValue(error.to_string()))?;
    output[encoded_end..].copy_from_slice(suffix);
    Ok(output)
}

fn xml_container_bounded(tag: &str, values: Vec<Vec<u8>>, budget: &Cell<usize>) -> Result<Vec<u8>> {
    let overhead = xml_tag_overhead(tag);
    let content_len =
        values.iter().try_fold(0usize, |length, value| length.checked_add(value.len())).ok_or(Error::LimitExceeded("encoded length"))?;
    let length = overhead.checked_add(content_len).ok_or(Error::LimitExceeded("encoded length"))?;
    charge_budget(budget, overhead)?;

    let mut output = Vec::with_capacity(length);
    output.push(b'<');
    output.extend_from_slice(tag.as_bytes());
    output.push(b'>');
    for value in values {
        output.extend_from_slice(&value);
    }
    output.extend_from_slice(b"</");
    output.extend_from_slice(tag.as_bytes());
    output.push(b'>');
    Ok(output)
}

fn xml_map_bounded(values: Vec<Vec<u8>>, keys: Vec<String>, budget: &Cell<usize>) -> Result<Vec<u8>> {
    if values.len() != keys.len() {
        return Err(Error::InvalidValue("LLSD map key and value counts differ".into()));
    }
    let overhead = xml_tag_overhead("map");
    let content_len = keys.iter().zip(&values).try_fold(0usize, |length, (key, value)| {
        let key_len = xml_tag_overhead("key").checked_add(xml_escaped_len(key)?).ok_or(Error::LimitExceeded("encoded length"))?;
        length.checked_add(key_len).and_then(|length| length.checked_add(value.len())).ok_or(Error::LimitExceeded("encoded length"))
    })?;
    let length = overhead.checked_add(content_len).ok_or(Error::LimitExceeded("encoded length"))?;
    charge_budget(budget, overhead)?;

    let mut output = Vec::with_capacity(length);
    output.extend_from_slice(b"<map>");
    for (key, value) in keys.into_iter().zip(values) {
        output.extend_from_slice(b"<key>");
        escape_xml(&key, &mut output);
        output.extend_from_slice(b"</key>");
        output.extend_from_slice(&value);
    }
    output.extend_from_slice(b"</map>");
    Ok(output)
}

fn charge_xml_key(key: &str, budget: &Cell<usize>) -> Result<()> {
    let length = xml_tag_overhead("key").checked_add(xml_escaped_len(key)?).ok_or(Error::LimitExceeded("encoded length"))?;
    charge_budget(budget, length)
}

fn charge_budget(budget: &Cell<usize>, length: usize) -> Result<()> {
    let remaining = budget.get().checked_sub(length).ok_or(Error::LimitExceeded("encoded length"))?;
    budget.set(remaining);
    Ok(())
}

fn xml_tag_overhead(tag: &str) -> usize {
    5 + tag.len() * 2
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
            value => value.len_utf8(),
        };
        length = length.checked_add(escaped_len).ok_or(Error::LimitExceeded("encoded length"))?;
    }
    Ok(length)
}

fn bounded_capacity<T>(budget: Option<&Rc<Cell<usize>>>, capacity: usize) -> Vec<T> {
    if budget.is_some() { Vec::new() } else { Vec::with_capacity(capacity) }
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

fn xml_container(tag: &str, values: Vec<Vec<u8>>) -> Vec<u8> {
    let mut output = format!("<{tag}>").into_bytes();
    for value in values {
        output.extend_from_slice(&value);
    }
    output.extend_from_slice(format!("</{tag}>").as_bytes());
    output
}

fn xml_map(values: Vec<Vec<u8>>, keys: Vec<String>) -> Result<Vec<u8>> {
    if values.len() != keys.len() {
        return Err(Error::InvalidValue("LLSD map key and value counts differ".into()));
    }

    let overhead = xml_tag_overhead("map");
    let content_len = keys.iter().zip(&values).try_fold(0usize, |length, (key, value)| {
        let key_len = xml_tag_overhead("key").checked_add(xml_escaped_len(key)?).ok_or(Error::LimitExceeded("encoded length"))?;
        length.checked_add(key_len).and_then(|length| length.checked_add(value.len())).ok_or(Error::LimitExceeded("encoded length"))
    })?;
    let length = overhead.checked_add(content_len).ok_or(Error::LimitExceeded("encoded length"))?;

    let mut output = Vec::with_capacity(length);
    output.extend_from_slice(b"<map>");
    for (key, value) in keys.into_iter().zip(values) {
        output.extend_from_slice(b"<key>");
        escape_xml(&key, &mut output);
        output.extend_from_slice(b"</key>");
        output.extend_from_slice(&value);
    }
    output.extend_from_slice(b"</map>");
    Ok(output)
}

fn escape_xml(text: &str, output: &mut Vec<u8>) {
    for character in text.chars() {
        match character {
            '&' => output.extend_from_slice(b"&amp;"),
            '<' => output.extend_from_slice(b"&lt;"),
            '>' => output.extend_from_slice(b"&gt;"),
            '\"' => output.extend_from_slice(b"&quot;"),
            '\'' => output.extend_from_slice(b"&apos;"),
            value => {
                let mut bytes = [0; 4];
                output.extend_from_slice(value.encode_utf8(&mut bytes).as_bytes());
            }
        }
    }
}

fn binary_fixed(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut output = vec![tag];
    output.extend_from_slice(value);
    output
}

fn binary_length(length: usize) -> Result<[u8; 4]> {
    u32::try_from(length).map(u32::to_be_bytes).map_err(|_| Error::LimitExceeded("encoded length"))
}

fn validate_binary_length(format: Format, length: usize) -> Result<()> {
    if format == Format::Binary { binary_length(length).map(|_| ()) } else { Ok(()) }
}

fn binary_sized(tag: u8, value: &[u8]) -> Result<Vec<u8>> {
    let mut output = vec![tag];
    output.extend_from_slice(&binary_length(value.len())?);
    output.extend_from_slice(value);
    Ok(output)
}

fn binary_container(start: u8, end: u8, values: Vec<Vec<u8>>) -> Result<Vec<u8>> {
    let mut output = vec![start];
    output.extend_from_slice(&binary_length(values.len())?);
    for value in values {
        output.extend_from_slice(&value);
    }
    output.push(end);
    Ok(output)
}

fn binary_map(values: Vec<Vec<u8>>, keys: Vec<String>) -> Result<Vec<u8>> {
    if values.len() != keys.len() {
        return Err(Error::InvalidValue("LLSD map key and value counts differ".into()));
    }
    let mut output = vec![b'{'];
    output.extend_from_slice(&binary_length(values.len())?);
    for (key, value) in keys.into_iter().zip(values) {
        output.extend_from_slice(&binary_sized(b'k', key.as_bytes())?);
        output.extend_from_slice(&value);
    }
    output.push(b'}');
    Ok(output)
}

fn notation_container(start: u8, end: u8, values: Vec<Vec<u8>>) -> Vec<u8> {
    let mut output = vec![start];
    for (index, value) in values.into_iter().enumerate() {
        if index > 0 {
            output.push(b',');
        }
        output.extend_from_slice(&value);
    }
    output.push(end);
    output
}

fn notation_map(values: Vec<Vec<u8>>, keys: Vec<String>) -> Vec<u8> {
    let mut output = vec![b'{'];
    for (index, (key, value)) in keys.into_iter().zip(values).enumerate() {
        if index > 0 {
            output.push(b',');
        }
        output.extend_from_slice(&notation_string(&key));
        output.push(b':');
        output.extend_from_slice(&value);
    }
    output.push(b'}');
    output
}

fn notation_string(value: &str) -> Vec<u8> {
    let mut output = vec![b'\''];
    for byte in value.as_bytes() {
        match byte {
            b'\\' => output.extend_from_slice(b"\\\\"),
            b'\'' => output.extend_from_slice(b"\\'"),
            b'\n' => output.extend_from_slice(b"\\n"),
            b'\r' => output.extend_from_slice(b"\\r"),
            b'\t' => output.extend_from_slice(b"\\t"),
            0..=0x1f | 0x7f => output.extend_from_slice(format!("\\x{byte:02x}").as_bytes()),
            _ => output.push(*byte),
        }
    }
    output.push(b'\'');
    output
}

fn notation_typed_quoted(tag: u8, value: &str) -> Vec<u8> {
    let mut output = vec![tag, b'\"'];
    for byte in value.as_bytes() {
        match byte {
            b'\\' => output.extend_from_slice(b"\\\\"),
            b'\"' => output.extend_from_slice(b"\\\""),
            b'\n' => output.extend_from_slice(b"\\n"),
            b'\r' => output.extend_from_slice(b"\\r"),
            b'\t' => output.extend_from_slice(b"\\t"),
            0..=0x1f | 0x7f => output.extend_from_slice(format!("\\x{byte:02x}").as_bytes()),
            _ => output.push(*byte),
        }
    }
    output.push(b'\"');
    output
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

fn parse_real(value: &str) -> Result<f64> {
    match value.to_ascii_lowercase().as_str() {
        "nan" => Ok(f64::NAN),
        "inf" | "+inf" | "infinity" | "+infinity" => Ok(f64::INFINITY),
        "-inf" | "-infinity" => Ok(f64::NEG_INFINITY),
        _ => value.parse().map_err(|error| Error::InvalidValue(format!("invalid LLSD real: {error}"))),
    }
}

fn extract_xml_string(bytes: &[u8]) -> Result<String> {
    let prefix = b"<string>";
    let suffix = b"</string>";
    if !bytes.starts_with(prefix) || !bytes.ends_with(suffix) {
        return Err(Error::InvalidValue("expected a string payload".into()));
    }
    let value = std::str::from_utf8(&bytes[prefix.len()..bytes.len() - suffix.len()]).map_err(|_| Error::InvalidUtf8)?;
    let mut decoded = value.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'");
    decoded = decoded.replace("&amp;", "&");
    Ok(decoded)
}

fn extract_binary_string(bytes: &[u8]) -> Result<String> {
    if bytes.first() != Some(&b's') || bytes.len() < 5 {
        return Err(Error::InvalidValue("expected a string payload".into()));
    }
    let length = u32::from_be_bytes(bytes[1..5].try_into().map_err(|_| Error::UnexpectedEof)?) as usize;
    if bytes.len() != 5 + length {
        return Err(Error::InvalidValue("invalid string payload length".into()));
    }
    String::from_utf8(bytes[5..].to_vec()).map_err(|_| Error::InvalidUtf8)
}

fn extract_notation_string(bytes: &[u8]) -> Result<String> {
    if bytes.len() < 2 || !matches!(bytes.first(), Some(b'\'' | b'\"')) || bytes.last() != bytes.first() {
        return Err(Error::InvalidValue("expected a string payload".into()));
    }
    let quote = bytes[0];
    let mut output = Vec::new();
    let mut index = 1;
    while index + 1 < bytes.len() {
        let byte = bytes[index];
        index += 1;
        if byte == b'\\' {
            let escape = *bytes.get(index).ok_or(Error::UnexpectedEof)?;
            index += 1;
            match escape {
                b'\\' | b'\'' | b'\"' => output.push(escape),
                b'n' => output.push(b'\n'),
                b'r' => output.push(b'\r'),
                b't' => output.push(b'\t'),
                b'x' => {
                    let hex = bytes.get(index..index + 2).ok_or(Error::UnexpectedEof)?;
                    output.push(
                        u8::from_str_radix(std::str::from_utf8(hex).map_err(|_| Error::InvalidUtf8)?, 16)
                            .map_err(|_| Error::InvalidValue("invalid string escape".into()))?,
                    );
                    index += 2;
                }
                _ => return Err(Error::InvalidValue("invalid string escape".into())),
            }
        } else if byte == quote {
            return Err(Error::InvalidValue("unescaped string delimiter".into()));
        } else {
            output.push(byte);
        }
    }
    String::from_utf8(output).map_err(|_| Error::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use super::{Format, Serializer};
    use crate::Error;

    #[test]
    fn invalid_binary_scalars_fail() {
        let serializer = Serializer { format: Format::Binary, budget: None, nesting: None };

        for (tag, text) in [
            ("boolean", "invalid"),
            ("integer", "invalid"),
            ("real", "invalid"),
            ("date", "invalid"),
            ("uuid", "invalid"),
            ("unknown", "value"),
        ] {
            assert!(matches!(serializer.scalar(tag, text), Err(Error::InvalidValue(_))));
        }
    }
}
