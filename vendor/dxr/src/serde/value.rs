//! This mod contains the deserialization logic for the XML-RPC value types.
//!
//! This manual deserialization is necessary to support scalar string values
//! without a `<type>` element, e.g. `<value>foo</value>` instead of
//! `<value><string>foo</string></value>` (which are both valid XML-RPC).

use std::borrow::Cow;
use std::fmt;

#[cfg(feature = "nil")]
use serde::de::IgnoredAny;
use serde::{
    Deserialize,
    de::{self, Deserializer, Visitor},
};

use crate::serde::XmlValue;

struct ValueVisitor {}

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = XmlValue<'static>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a valid XML-RPC scalar value")
    }

    #[allow(clippy::too_many_lines)]
    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: de::MapAccess<'de>,
    {
        const FIELDS: &[&str] = &[
            "i4",
            "int",
            #[cfg(feature = "i8")]
            "i8",
            "boolean",
            "string",
            "double",
            "dateTime.iso8601",
            "base64",
            "struct",
            "array",
            #[cfg(feature = "nil")]
            "nil",
        ];

        #[derive(Debug)]
        enum Field {
            I4,
            #[cfg(feature = "i8")]
            I8,
            Boolean,
            String,
            Double,
            DateTime,
            Base64,
            Struct,
            Array,
            #[cfg(feature = "nil")]
            Nil,
        }

        impl<'de> Deserialize<'de> for Field {
            fn deserialize<D>(deserializer: D) -> Result<Field, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct FieldVisitor;

                impl Visitor<'_> for FieldVisitor {
                    type Value = Field;

                    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                        formatter.write_str("field should be called `value`")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Field, E>
                    where
                        E: de::Error,
                    {
                        match value {
                            "i4" => Ok(Field::I4),
                            "int" => Ok(Field::I4),
                            #[cfg(feature = "i8")]
                            "i8" => Ok(Field::I8),
                            "boolean" => Ok(Field::Boolean),
                            "string" => Ok(Field::String),
                            "double" => Ok(Field::Double),
                            "dateTime.iso8601" => Ok(Field::DateTime),
                            "base64" => Ok(Field::Base64),
                            "struct" => Ok(Field::Struct),
                            "array" => Ok(Field::Array),
                            #[cfg(feature = "nil")]
                            "nil" => Ok(Field::Nil),
                            "$value" => Ok(Field::String),
                            "$text" => Ok(Field::String),
                            _ => Err(de::Error::unknown_field(value, FIELDS)),
                        }
                    }
                }

                deserializer.deserialize_identifier(FieldVisitor)
            }
        }

        let value = if let Some(key) = map.next_key::<Field>()? {
            match key {
                Field::I4 => {
                    let value = map.next_value()?;
                    Ok(XmlValue::i4(value))
                },
                #[cfg(feature = "i8")]
                Field::I8 => {
                    let value = map.next_value()?;
                    Ok(XmlValue::i8(value))
                },
                Field::Boolean => {
                    let string: String = map.next_value()?;
                    super::boolean::from_str(&string)
                        .map(XmlValue::boolean)
                        .map_err(de::Error::custom)
                },
                Field::String => {
                    let value: String = map.next_value()?;
                    Ok(XmlValue::string(value.into()))
                },
                Field::Double => {
                    let value = map.next_value()?;
                    Ok(XmlValue::double(value))
                },
                Field::DateTime => {
                    let string: String = map.next_value()?;
                    super::datetime::from_str(&string)
                        .map(XmlValue::datetime)
                        .map_err(de::Error::custom)
                },
                Field::Base64 => {
                    let string: String = map.next_value()?;
                    super::base64::from_str(&string)
                        .map(XmlValue::base64)
                        .map_err(de::Error::custom)
                },
                Field::Struct => {
                    let value = map.next_value()?;
                    Ok(XmlValue::structure(value))
                },
                Field::Array => {
                    let value = map.next_value()?;
                    Ok(XmlValue::array(value))
                },
                #[cfg(feature = "nil")]
                Field::Nil => {
                    let _: IgnoredAny = map.next_value()?;
                    Ok(XmlValue::nil())
                },
            }
        } else {
            // <value></value>
            return Ok(XmlValue::string(Cow::Borrowed("")));
        };

        if let Some(key) = map.next_key::<Field>()? {
            Err(de::Error::custom(format!("Unexpected key: {key:?}")))
        } else {
            value
        }
    }
}

impl<'de> Deserialize<'de> for XmlValue<'_> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ValueVisitor {})
    }
}
