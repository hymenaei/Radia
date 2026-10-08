mod base64;
mod boolean;
mod datetime;
mod value;

#[cfg(test)]
mod tests;

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::datetime::DateTime;
use crate::error::Error;
use crate::fault::Fault;
use crate::traits::TryFromValue;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename = "value")]
pub(crate) struct XmlValue<'a> {
    #[serde(rename = "$value")]
    value: XmlType<'a>,
}

impl<'a> XmlValue<'a> {
    pub(crate) const fn new(value: XmlType<'a>) -> Self {
        XmlValue { value }
    }

    pub(crate) fn into_inner(self) -> XmlType<'a> {
        self.value
    }

    pub(crate) const fn i4(value: i32) -> Self {
        XmlValue::new(XmlType::Integer(value))
    }

    #[cfg(feature = "i8")]
    pub(crate) const fn i8(value: i64) -> Self {
        XmlValue::new(XmlType::Long(value))
    }

    pub(crate) const fn boolean(value: bool) -> Self {
        XmlValue::new(XmlType::Boolean(value))
    }

    pub(crate) const fn string(value: Cow<'a, str>) -> Self {
        XmlValue::new(XmlType::String(value))
    }

    pub(crate) const fn double(value: f64) -> Self {
        XmlValue::new(XmlType::Double(value))
    }

    pub(crate) const fn datetime(value: DateTime) -> Self {
        XmlValue::new(XmlType::DateTime(value))
    }

    pub(crate) const fn base64(value: Cow<'a, [u8]>) -> Self {
        XmlValue::new(XmlType::Base64(value))
    }

    pub(crate) fn structure(value: XmlStruct<'a>) -> Self {
        XmlValue::new(XmlType::Struct { members: value.members })
    }

    pub(crate) fn array(value: XmlArray<'a>) -> Self {
        XmlValue::new(XmlType::Array { data: value.data })
    }

    #[cfg(feature = "nil")]
    pub(crate) const fn nil() -> Self {
        XmlValue::new(XmlType::Nil)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) enum XmlType<'a> {
    #[serde(rename = "i4", alias = "int")]
    Integer(#[serde(rename = "$value")] i32),
    #[cfg(feature = "i8")]
    #[serde(rename = "i8")]
    Long(#[serde(rename = "$value")] i64),
    #[serde(rename = "boolean", with = "boolean")]
    Boolean(#[serde(rename = "$value")] bool),
    #[serde(rename = "string")]
    String(#[serde(rename = "$value")] Cow<'a, str>),
    #[serde(rename = "double")]
    Double(#[serde(rename = "$value")] f64),
    #[serde(rename = "dateTime.iso8601", with = "datetime")]
    DateTime(#[serde(rename = "$value")] DateTime),
    #[serde(rename = "base64", with = "base64")]
    Base64(#[serde(rename = "$value")] Cow<'a, [u8]>),
    #[serde(rename = "struct")]
    Struct {
        #[serde(default, rename = "member")]
        members: Vec<XmlStructMember<'a>>,
    },
    #[serde(rename = "array")]
    Array {
        #[serde(default)]
        data: XmlArrayData<'a>,
    },
    #[cfg(feature = "nil")]
    #[serde(rename = "nil")]
    Nil,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename = "struct")]
pub(crate) struct XmlStruct<'a> {
    #[serde(default, rename = "member")]
    members: Vec<XmlStructMember<'a>>,
}

impl<'a> XmlStruct<'a> {
    pub(crate) fn new(mut members: Vec<XmlStructMember<'a>>) -> Self {
        members.sort_by(|a, b| a.name.name.cmp(&b.name.name));
        XmlStruct { members }
    }
}

impl PartialEq for XmlStruct<'_> {
    fn eq(&self, other: &Self) -> bool {
        // fast path: different numbers of members
        if self.members.len() != other.members.len() {
            return false;
        }

        // sort members by name before comparing
        let mut self_members: Vec<&XmlStructMember> = self.members.iter().collect();
        let mut other_members: Vec<&XmlStructMember> = other.members.iter().collect();
        self_members.sort_by(|a, b| a.name.name.cmp(&b.name.name));
        other_members.sort_by(|a, b| a.name.name.cmp(&b.name.name));

        self_members == other_members
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename = "member")]
pub(crate) struct XmlStructMember<'a> {
    name: XmlStructMemberName<'a>,
    value: XmlValue<'a>,
}

impl<'a> XmlStructMember<'a> {
    pub(crate) const fn new(name: Cow<'a, str>, value: XmlValue<'a>) -> Self {
        XmlStructMember {
            name: XmlStructMemberName { name },
            value,
        }
    }

    pub(crate) fn into_inner(self) -> (Cow<'a, str>, XmlValue<'a>) {
        (self.name.name, self.value)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename = "name")]
struct XmlStructMemberName<'a> {
    #[serde(rename = "$value")]
    name: Cow<'a, str>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename = "array")]
pub(crate) struct XmlArray<'a> {
    #[serde(default)]
    data: XmlArrayData<'a>,
}

impl<'a> XmlArray<'a> {
    pub(crate) const fn new(values: Vec<XmlValue<'a>>) -> Self {
        XmlArray {
            data: XmlArrayData { values },
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename = "data")]
pub(crate) struct XmlArrayData<'a> {
    #[serde(default, rename = "value")]
    values: Vec<XmlValue<'a>>,
}

impl<'a> XmlArrayData<'a> {
    pub(crate) fn into_inner(self) -> Vec<XmlValue<'a>> {
        self.values
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename = "methodCall")]
pub(crate) struct XmlMethodCall<'a> {
    #[serde(rename = "methodName")]
    name: XmlMethodName<'a>,
    #[serde(default, skip_serializing_if = "XmlRequestParameters::is_empty")]
    params: XmlRequestParameters<'a>,
}

impl<'a> XmlMethodCall<'a> {
    pub(crate) fn new<N>(name: N, params: Vec<XmlValue<'a>>) -> Self
    where
        N: Into<Cow<'a, str>>,
    {
        XmlMethodCall {
            name: XmlMethodName { name: name.into() },
            params: XmlRequestParameters {
                params: params.into_iter().map(|value| XmlRequestParameter { value }).collect(),
            },
        }
    }

    pub(crate) fn into_inner(self) -> (Cow<'a, str>, Vec<XmlValue<'a>>) {
        (
            self.name.name,
            self.params.params.into_iter().map(|p| p.value).collect(),
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename = "methodName")]
pub(crate) struct XmlMethodName<'a> {
    #[serde(rename = "$value")]
    name: Cow<'a, str>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename = "methodResponse")]
pub(crate) struct XmlMethodResponse<'a> {
    params: XmlResponseParameters<'a>,
}

impl<'a> XmlMethodResponse<'a> {
    pub(crate) const fn new(value: XmlValue<'a>) -> Self {
        XmlMethodResponse {
            params: XmlResponseParameters {
                params: XmlResponseParameter { params: value },
            },
        }
    }

    pub(crate) fn into_inner(self) -> XmlValue<'a> {
        self.params.params.params
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename = "methodResponse")]
pub(crate) struct XmlFaultResponse<'a> {
    fault: XmlFaultStruct<'a>,
}

impl<'a> From<&'a Fault> for XmlFaultResponse<'a> {
    fn from(fault: &'a Fault) -> Self {
        XmlFaultResponse {
            fault: XmlFaultStruct {
                value: XmlFaultValue {
                    value: XmlStruct::new(vec![
                        XmlStructMember::new(String::from("faultCode").into(), XmlValue::i4(fault.code())),
                        XmlStructMember::new(
                            String::from("faultString").into(),
                            XmlValue::string(Cow::Owned(String::from(fault.string()))),
                        ),
                    ]),
                },
            },
        }
    }
}

impl From<Fault> for XmlFaultResponse<'static> {
    fn from(fault: Fault) -> Self {
        let (code, string) = fault.into_inner();

        XmlFaultResponse {
            fault: XmlFaultStruct {
                value: XmlFaultValue {
                    value: XmlStruct::new(vec![
                        XmlStructMember::new(String::from("faultCode").into(), XmlValue::i4(code)),
                        XmlStructMember::new(String::from("faultString").into(), XmlValue::string(Cow::Owned(string))),
                    ]),
                },
            },
        }
    }
}

impl TryFrom<XmlFaultResponse<'_>> for Fault {
    type Error = Error;

    fn try_from(value: XmlFaultResponse) -> Result<Self, Self::Error> {
        let members = value.fault.value.value.members;
        let members_no = members.len();

        let [first, second]: [XmlStructMember; 2] = members
            .try_into()
            .map_err(|_| Error::parameter_mismatch(members_no, 2))?;

        let fault_code = if first.name.name == "faultCode" {
            first.value
        } else {
            return Err(Error::missing_field("fault", "faultCode"));
        };

        let fault_string = if second.name.name == "faultString" {
            second.value
        } else {
            return Err(Error::missing_field("fault", "faultString"));
        };

        let code: i32 = i32::try_from_value(&fault_code.into())?;
        let string: String = String::try_from_value(&fault_string.into())?;

        Ok(Fault::new(code, string))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename = "fault")]
struct XmlFaultStruct<'a> {
    value: XmlFaultValue<'a>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename = "value")]
struct XmlFaultValue<'a> {
    #[serde(rename = "struct")]
    value: XmlStruct<'a>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename = "params")]
struct XmlRequestParameters<'a> {
    #[serde(default, rename = "param")]
    params: Vec<XmlRequestParameter<'a>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename = "param")]
struct XmlRequestParameter<'a> {
    value: XmlValue<'a>,
}

impl XmlRequestParameters<'_> {
    fn is_empty(&self) -> bool {
        self.params.is_empty()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename = "params")]
struct XmlResponseParameters<'a> {
    #[serde(rename = "param")]
    params: XmlResponseParameter<'a>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename = "param")]
struct XmlResponseParameter<'a> {
    #[serde(rename = "value")]
    params: XmlValue<'a>,
}
