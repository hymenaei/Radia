use std::borrow::Cow;
use std::collections::BTreeMap;

use crate::datetime::DateTime;
use crate::error::Error;
use crate::fault::Fault;
use crate::serde::{XmlFaultResponse, XmlMethodCall, XmlMethodResponse, XmlType, XmlValue};
use crate::xml::{deserialize_xml, serialize_xml};

#[cfg(doc)]
use crate::traits::{TryFromValue, TryToValue};
#[cfg(doc)]
use std::collections::HashMap;

type Struct = BTreeMap<String, Value>;
type Array = Vec<Value>;

/// XML-RPC value type
///
/// The [`Value`] type is the Rust equivalent of valid XML-RPC values.
///
/// Values can be constructed manually by initializing the enum variant, but it is recommended
/// to use the [`TryFromValue`] and [`TryToValue`] trait implementations for a consistent
/// interface across all types, including [`Vec`], arrays, slices, tuples, [`HashMap`]s, and
/// custom structs when using the derive macros for the [`TryFromValue`] and [`TryToValue`]
/// traits.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Value of type `i4` / `int`
    Integer(i32),
    #[cfg(feature = "i8")]
    /// Value of type `i8`
    ///
    /// This type is not part of the original XML-RPC spec, but is a widely used extension.
    /// Support for <i8> values is optional and can be enabled with the i8 crate feature.
    Long(i64),
    /// Value of type `boolean`
    Boolean(bool),
    /// Value of type `string`
    String(String),
    /// Value of type `double`
    Double(f64),
    /// Value of type `dateTime.iso8601`
    DateTime(DateTime),
    /// Value of type `base64`
    Base64(Vec<u8>),
    /// Value of type `struct`
    Struct(Struct),
    /// Value of type `array`
    Array(Array),
    #[cfg(feature = "nil")]
    /// Value of type `nil`
    ///
    /// This type is not part of the original XML-RPC spec, but is a widely used extension.
    /// Support for `<nil>` values is optional and can be enabled with the `nil` crate feature.
    ///
    /// If enabled, this type is used to emulate support for optional values in XML-RPC, by mapping
    /// Rust [`Option`]s to either their contained [`Value`] if the value is [`Some<T>`], or to a
    /// `<nil/>` the value is [`None`].
    ///
    /// This is consistent with the XML-RPC implementation in the Python `xmlrpc` standard library
    /// module, which also maps `None` values to `<nil/>`.
    Nil,
}

impl Value {
    pub(crate) const fn kind(&self) -> &'static str {
        match self {
            Value::Integer(_) => "i4",
            #[cfg(feature = "i8")]
            Value::Long(_) => "i8",
            Value::Boolean(_) => "boolean",
            Value::String(_) => "string",
            Value::Double(_) => "double",
            Value::DateTime(_) => "dateTime.iso8601",
            Value::Base64(_) => "base64",
            Value::Struct { .. } => "struct",
            Value::Array { .. } => "array",
            #[cfg(feature = "nil")]
            Value::Nil => "nil",
        }
    }
}

impl From<XmlValue<'_>> for Value {
    fn from(value: XmlValue) -> Self {
        match value.into_inner() {
            XmlType::Integer(i) => Value::Integer(i),
            #[cfg(feature = "i8")]
            XmlType::Long(l) => Value::Long(l),
            XmlType::Boolean(b) => Value::Boolean(b),
            XmlType::String(s) => Value::String(String::from(s)),
            XmlType::Double(d) => Value::Double(d),
            XmlType::DateTime(dt) => Value::DateTime(dt),
            XmlType::Base64(b) => Value::Base64(b.to_vec()),
            XmlType::Struct { members } => Value::Struct(utils::struct_from_members(members)),
            XmlType::Array { data } => Value::Array(utils::array_from_data(data)),
            #[cfg(feature = "nil")]
            XmlType::Nil => Value::Nil,
        }
    }
}

impl<'a> From<&'a Value> for XmlValue<'a> {
    fn from(value: &'a Value) -> Self {
        match value {
            Value::Integer(i) => XmlValue::i4(*i),
            #[cfg(feature = "i8")]
            Value::Long(l) => XmlValue::i8(*l),
            Value::Boolean(b) => XmlValue::boolean(*b),
            Value::String(s) => XmlValue::string(Cow::Borrowed(s)),
            Value::Double(d) => XmlValue::double(*d),
            Value::DateTime(dt) => XmlValue::datetime(*dt),
            Value::Base64(b) => XmlValue::base64(Cow::Borrowed(b)),
            Value::Struct(members) => XmlValue::structure(utils::members_from_struct(members)),
            Value::Array(data) => XmlValue::array(utils::data_from_array(data)),
            #[cfg(feature = "nil")]
            Value::Nil => XmlValue::nil(),
        }
    }
}

impl From<Value> for XmlValue<'_> {
    fn from(value: Value) -> Self {
        match value {
            Value::Integer(i) => XmlValue::i4(i),
            #[cfg(feature = "i8")]
            Value::Long(l) => XmlValue::i8(l),
            Value::Boolean(b) => XmlValue::boolean(b),
            Value::String(s) => XmlValue::string(s.into()),
            Value::Double(d) => XmlValue::double(d),
            Value::DateTime(dt) => XmlValue::datetime(dt),
            Value::Base64(b) => XmlValue::base64(b.into()),
            Value::Struct(members) => XmlValue::structure(utils::members_from_struct_owned(members)),
            Value::Array(data) => XmlValue::array(utils::data_from_array_owned(data)),
            #[cfg(feature = "nil")]
            Value::Nil => XmlValue::nil(),
        }
    }
}

/// XML-RPC method call type
///
/// The [`MethodCall`] type is the Rust equivalent of the contents of an XML-RPC method call.
#[derive(Debug)]
pub struct MethodCall<'a> {
    /// Name of the called method
    pub name: Cow<'a, str>,
    /// Dynamically typed method arguments
    pub params: Vec<Value>,
}

impl MethodCall<'_> {
    /// Deserialize method call from XML.
    pub fn from_xml(string: &str) -> Result<Self, Error> {
        let value: XmlMethodCall = deserialize_xml(string).map_err(|_| Error::invalid_data(String::from(string)))?;

        let (name, params) = value.into_inner();

        Ok(MethodCall {
            name,
            params: params.into_iter().map(Value::from).collect(),
        })
    }

    /// Serialize method call as XML.
    pub fn to_xml(&self) -> Result<String, Error> {
        let value = XmlMethodCall::new(self.name.as_ref(), self.params.iter().map(Into::into).collect());

        serialize_xml(&value).map_err(|error| Error::invalid_data(error.to_string()))
    }
}

/// XML-RPC method call type
///
/// The [`MethodResponse`] type is the Rust equivalent of the contents of an XML-RPC response.
#[derive(Debug)]
pub struct MethodResponse {
    /// Method return value
    pub value: Value,
}

impl MethodResponse {
    /// Deserialize method response from XML.
    pub fn from_xml(string: &str) -> Result<Self, Error> {
        let value: XmlMethodResponse =
            deserialize_xml(string).map_err(|_| Error::invalid_data(String::from(string)))?;

        Ok(MethodResponse {
            value: value.into_inner().into(),
        })
    }

    /// Serialize method response as XML.
    pub fn to_xml(&self) -> Result<String, Error> {
        let value = XmlMethodResponse::new((&self.value).into());

        serialize_xml(&value).map_err(|error| Error::invalid_data(error.to_string()))
    }
}

/// XML-RPC fault response type
///
/// The [`FaultResponse`] type is the Rust equivalent of the contents of an XML-RPC fault response.
#[derive(Debug)]
pub struct FaultResponse {
    /// Fault response value
    pub fault: Fault,
}

impl FaultResponse {
    /// Deserialize fault response from XML.
    pub fn from_xml(string: &str) -> Result<Self, Error> {
        let value: XmlFaultResponse = deserialize_xml(string).map_err(|_| Error::invalid_data(String::from(string)))?;

        Ok(FaultResponse {
            fault: Fault::try_from(value)?,
        })
    }

    /// Serialize fault response as XML.
    pub fn to_xml(&self) -> Result<String, Error> {
        let value = XmlFaultResponse::from(&self.fault);

        serialize_xml(&value).map_err(|error| Error::invalid_data(error.to_string()))
    }
}

mod utils {
    use crate::serde::{XmlArray, XmlArrayData, XmlStruct, XmlStructMember};

    use super::{Array, Struct};

    pub(super) fn struct_from_members(members: Vec<XmlStructMember>) -> Struct {
        members
            .into_iter()
            .map(|m| {
                let (name, value) = m.into_inner();
                (name.into(), value.into())
            })
            .collect()
    }

    pub(super) fn members_from_struct(value: &Struct) -> XmlStruct<'_> {
        let members = value
            .iter()
            .map(|(k, v)| XmlStructMember::new(k.into(), v.into()))
            .collect();
        XmlStruct::new(members)
    }

    pub(super) fn members_from_struct_owned(value: Struct) -> XmlStruct<'static> {
        let members = value
            .into_iter()
            .map(|(k, v)| XmlStructMember::new(k.into(), v.into()))
            .collect();
        XmlStruct::new(members)
    }

    pub(super) fn array_from_data(members: XmlArrayData) -> Array {
        members.into_inner().into_iter().map(Into::into).collect()
    }

    pub(super) fn data_from_array(value: &Array) -> XmlArray<'_> {
        let data = value.iter().map(Into::into).collect();
        XmlArray::new(data)
    }

    pub(super) fn data_from_array_owned(value: Array) -> XmlArray<'static> {
        let data = value.into_iter().map(Into::into).collect();
        XmlArray::new(data)
    }
}
