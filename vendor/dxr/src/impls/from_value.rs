#![allow(clippy::wildcard_enum_match_arm)]

use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::datetime::DateTime;
use crate::error::Error;
use crate::traits::TryFromValue;
use crate::types::Value;

use super::utils::{
    values_to_tuple_1,
    values_to_tuple_2,
    values_to_tuple_3,
    values_to_tuple_4,
    values_to_tuple_5,
    values_to_tuple_6,
    values_to_tuple_7,
    values_to_tuple_8,
};

impl TryFromValue for Value {
    fn try_from_value(value: &Value) -> Result<Value, Error> {
        Ok(value.clone())
    }
}

impl TryFromValue for i32 {
    fn try_from_value(value: &Value) -> Result<i32, Error> {
        match value {
            Value::Integer(int) => Ok(*int),
            t => Err(Error::wrong_type(t.kind(), "i4")),
        }
    }
}

#[cfg(feature = "i8")]
impl TryFromValue for i64 {
    fn try_from_value(value: &Value) -> Result<i64, Error> {
        match value {
            Value::Long(long) => Ok(*long),
            t => Err(Error::wrong_type(t.kind(), "i8")),
        }
    }
}

impl TryFromValue for bool {
    fn try_from_value(value: &Value) -> Result<bool, Error> {
        match value {
            Value::Boolean(boo) => Ok(*boo),
            t => Err(Error::wrong_type(t.kind(), "boolean")),
        }
    }
}

impl TryFromValue for String {
    fn try_from_value(value: &Value) -> Result<String, Error> {
        match value {
            Value::String(string) => Ok(string.to_owned()),
            t => Err(Error::wrong_type(t.kind(), "string")),
        }
    }
}

impl TryFromValue for f64 {
    fn try_from_value(value: &Value) -> Result<f64, Error> {
        match value {
            Value::Double(double) => Ok(*double),
            t => Err(Error::wrong_type(t.kind(), "double")),
        }
    }
}

impl TryFromValue for DateTime {
    fn try_from_value(value: &Value) -> Result<DateTime, Error> {
        match value {
            Value::DateTime(date) => Ok(*date),
            t => Err(Error::wrong_type(t.kind(), "dateTime.iso8861")),
        }
    }
}

#[cfg(feature = "chrono")]
impl TryFromValue for chrono::NaiveDateTime {
    fn try_from_value(value: &Value) -> Result<chrono::NaiveDateTime, Error> {
        match value {
            Value::DateTime(date) => Ok((*date).into()),
            t => Err(Error::wrong_type(t.kind(), "dateTime.iso8861")),
        }
    }
}

#[cfg(feature = "jiff")]
impl TryFromValue for jiff::civil::DateTime {
    fn try_from_value(value: &Value) -> Result<jiff::civil::DateTime, Error> {
        match value {
            Value::DateTime(date) => Ok((*date).into()),
            t => Err(Error::wrong_type(t.kind(), "dateTime.iso8861")),
        }
    }
}

#[cfg(feature = "time")]
impl TryFromValue for time::PrimitiveDateTime {
    fn try_from_value(value: &Value) -> Result<time::PrimitiveDateTime, Error> {
        match value {
            Value::DateTime(date) => Ok((*date).into()),
            t => Err(Error::wrong_type(t.kind(), "dateTime.iso8861")),
        }
    }
}

impl TryFromValue for Vec<u8> {
    fn try_from_value(value: &Value) -> Result<Vec<u8>, Error> {
        match value {
            Value::Base64(bytes) => Ok(bytes.clone()),
            t => Err(Error::wrong_type(t.kind(), "base64")),
        }
    }
}

#[cfg(feature = "nil")]
impl<T> TryFromValue for Option<T>
where
    T: TryFromValue,
{
    fn try_from_value(value: &Value) -> Result<Option<T>, Error> {
        if let Value::Nil = value {
            Ok(None)
        } else {
            Ok(Some(T::try_from_value(value)?))
        }
    }
}

impl TryFromValue for Cow<'_, str> {
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        Ok(Cow::Owned(String::try_from_value(value)?))
    }
}

impl<T> TryFromValue for Cow<'_, T>
where
    T: TryFromValue + Clone,
{
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        Ok(Cow::Owned(T::try_from_value(value)?))
    }
}

impl<T> TryFromValue for Box<T>
where
    T: TryFromValue,
{
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        Ok(Box::new(T::try_from_value(value)?))
    }
}

impl<T> TryFromValue for Rc<T>
where
    T: TryFromValue,
{
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        Ok(Rc::new(T::try_from_value(value)?))
    }
}

impl<T> TryFromValue for Arc<T>
where
    T: TryFromValue,
{
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        Ok(Arc::new(T::try_from_value(value)?))
    }
}

impl<T> TryFromValue for Vec<T>
where
    T: TryFromValue,
{
    fn try_from_value(value: &Value) -> Result<Vec<T>, Error> {
        let values = match value {
            Value::Array(members) => Ok(members),
            t => Err(Error::wrong_type(t.kind(), "array")),
        };

        values?.iter().map(T::try_from_value).collect()
    }
}

impl<T, const N: usize> TryFromValue for [T; N]
where
    T: TryFromValue,
{
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        let values = match value {
            Value::Array(members) => Ok(members),
            t => Err(Error::wrong_type(t.kind(), "array")),
        }?;

        let mapped: Vec<T> = values
            .iter()
            .map(T::try_from_value)
            .collect::<Result<Vec<T>, Error>>()?;
        let len = mapped.len();

        mapped.try_into().map_err(|_| Error::parameter_mismatch(len, N))
    }
}

impl<T, S: std::hash::BuildHasher> TryFromValue for HashMap<String, T, S>
where
    T: TryFromValue,
    S: Default,
{
    fn try_from_value(value: &Value) -> Result<HashMap<String, T, S>, Error> {
        let values = match value {
            Value::Struct(members) => Ok(members),
            t => Err(Error::wrong_type(t.kind(), "struct")),
        };

        values?
            .iter()
            .map(|(k, v)| {
                let name = k.clone();
                match T::try_from_value(v) {
                    Ok(value) => Ok((name, value)),
                    Err(error) => Err(error),
                }
            })
            .collect()
    }
}

// some implementations for exact numbers of values (with possibly different types)

impl TryFromValue for () {
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        match value {
            Value::Array(members) => {
                let values = members;

                match values.len() {
                    0 => Ok(()),
                    n => Err(Error::parameter_mismatch(n, 0)),
                }
            },
            #[cfg(feature = "nil")]
            Value::Nil => Ok(()),
            other => Err(Error::wrong_type(other.kind(), "array | nil")),
        }
    }
}

impl<T> TryFromValue for (T,)
where
    T: TryFromValue,
{
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        if let Value::Array(members) = value {
            values_to_tuple_1(members)
        } else {
            Err(Error::wrong_type(value.kind(), "array"))
        }
    }
}

impl<A, B> TryFromValue for (A, B)
where
    A: TryFromValue,
    B: TryFromValue,
{
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        if let Value::Array(members) = value {
            values_to_tuple_2(members)
        } else {
            Err(Error::wrong_type(value.kind(), "array"))
        }
    }
}

impl<A, B, C> TryFromValue for (A, B, C)
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
{
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        if let Value::Array(members) = value {
            values_to_tuple_3(members)
        } else {
            Err(Error::wrong_type(value.kind(), "array"))
        }
    }
}

impl<A, B, C, D> TryFromValue for (A, B, C, D)
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
    D: TryFromValue,
{
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        if let Value::Array(members) = value {
            values_to_tuple_4(members)
        } else {
            Err(Error::wrong_type(value.kind(), "array"))
        }
    }
}

impl<A, B, C, D, E> TryFromValue for (A, B, C, D, E)
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
    D: TryFromValue,
    E: TryFromValue,
{
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        if let Value::Array(members) = value {
            values_to_tuple_5(members)
        } else {
            Err(Error::wrong_type(value.kind(), "array"))
        }
    }
}

impl<A, B, C, D, E, F> TryFromValue for (A, B, C, D, E, F)
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
    D: TryFromValue,
    E: TryFromValue,
    F: TryFromValue,
{
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        if let Value::Array(members) = value {
            values_to_tuple_6(members)
        } else {
            Err(Error::wrong_type(value.kind(), "array"))
        }
    }
}

impl<A, B, C, D, E, F, G> TryFromValue for (A, B, C, D, E, F, G)
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
    D: TryFromValue,
    E: TryFromValue,
    F: TryFromValue,
    G: TryFromValue,
{
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        if let Value::Array(members) = value {
            values_to_tuple_7(members)
        } else {
            Err(Error::wrong_type(value.kind(), "array"))
        }
    }
}

impl<A, B, C, D, E, F, G, H> TryFromValue for (A, B, C, D, E, F, G, H)
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
    D: TryFromValue,
    E: TryFromValue,
    F: TryFromValue,
    G: TryFromValue,
    H: TryFromValue,
{
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        if let Value::Array(members) = value {
            values_to_tuple_8(members)
        } else {
            Err(Error::wrong_type(value.kind(), "array"))
        }
    }
}

// if needed, implementations for more arguments can be implemented
