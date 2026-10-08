use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use crate::datetime::DateTime;
use crate::error::Error;
use crate::traits::TryToValue;
use crate::types::Value;

//use crate::values::{Array, DateTime, Member, Struct, Value};

use super::utils::{
    tuple_to_values_1,
    tuple_to_values_2,
    tuple_to_values_3,
    tuple_to_values_4,
    tuple_to_values_5,
    tuple_to_values_6,
    tuple_to_values_7,
    tuple_to_values_8,
};

impl<T> TryToValue for &T
where
    T: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        TryToValue::try_to_value(*self)
    }
}

impl TryToValue for Value {
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(self.clone())
    }
}

impl TryToValue for i32 {
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::Integer(*self))
    }
}

#[cfg(feature = "i8")]
impl TryToValue for i64 {
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::Long(*self))
    }
}

impl TryToValue for bool {
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::Boolean(*self))
    }
}

impl TryToValue for String {
    fn try_to_value(&self) -> Result<Value, Error> {
        TryToValue::try_to_value(&self.as_str())
    }
}

impl TryToValue for &str {
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::String(String::from(*self)))
    }
}

impl TryToValue for f64 {
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::Double(*self))
    }
}

impl TryToValue for DateTime {
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::DateTime(*self))
    }
}

#[cfg(feature = "chrono")]
impl TryToValue for chrono::NaiveDateTime {
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::DateTime((*self).into()))
    }
}

#[cfg(feature = "jiff")]
impl TryToValue for jiff::civil::DateTime {
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::DateTime((*self).into()))
    }
}

#[cfg(feature = "time")]
impl TryToValue for time::PrimitiveDateTime {
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::DateTime((*self).into()))
    }
}

impl TryToValue for Vec<u8> {
    fn try_to_value(&self) -> Result<Value, Error> {
        TryToValue::try_to_value(&self.as_slice())
    }
}

impl<const N: usize> TryToValue for [u8; N] {
    fn try_to_value(&self) -> Result<Value, Error> {
        TryToValue::try_to_value(&self.as_slice())
    }
}

impl TryToValue for &[u8] {
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::Base64(self.to_vec()))
    }
}

#[cfg(feature = "nil")]
impl<T> TryToValue for Option<T>
where
    T: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        if let Some(value) = self {
            T::try_to_value(value)
        } else {
            Ok(Value::Nil)
        }
    }
}

impl TryToValue for Cow<'_, str> {
    fn try_to_value(&self) -> Result<Value, Error> {
        match self {
            Cow::Owned(owned) => TryToValue::try_to_value(owned),
            Cow::Borrowed(borrowed) => TryToValue::try_to_value(borrowed),
        }
    }
}

impl<T> TryToValue for Cow<'_, T>
where
    T: TryToValue + Clone,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        match self {
            Cow::Owned(owned) => TryToValue::try_to_value(owned),
            Cow::Borrowed(borrowed) => TryToValue::try_to_value(*borrowed),
        }
    }
}

impl<T> TryToValue for Box<T>
where
    T: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        TryToValue::try_to_value(self.as_ref())
    }
}

impl<T> TryToValue for Rc<T>
where
    T: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        TryToValue::try_to_value(self.as_ref())
    }
}

impl<T> TryToValue for Arc<T>
where
    T: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        TryToValue::try_to_value(self.as_ref())
    }
}

impl<T> TryToValue for Vec<T>
where
    T: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        TryToValue::try_to_value(&self.as_slice())
    }
}

impl<T, const N: usize> TryToValue for [T; N]
where
    T: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        TryToValue::try_to_value(&self.as_slice())
    }
}

impl<T> TryToValue for &[T]
where
    T: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        let values = self
            .iter()
            .map(|value| T::try_to_value(value))
            .collect::<Result<Vec<Value>, Error>>();

        Ok(Value::Array(values?))
    }
}

impl<T, S: std::hash::BuildHasher> TryToValue for HashMap<String, T, S>
where
    T: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        let members = self
            .iter()
            .map(|(k, v)| T::try_to_value(v).map(|v| (k.to_owned(), v)))
            .collect::<Result<BTreeMap<String, Value>, Error>>();

        Ok(Value::Struct(members?))
    }
}

impl<T, S: std::hash::BuildHasher> TryToValue for HashMap<&str, T, S>
where
    T: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        let members = self
            .iter()
            .map(|(k, v)| T::try_to_value(v).map(|v| (String::from(*k), v)))
            .collect::<Result<BTreeMap<String, Value>, Error>>();

        Ok(Value::Struct(members?))
    }
}

impl<T> TryToValue for (T,)
where
    T: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::Array(tuple_to_values_1(self)?))
    }
}

impl<A, B> TryToValue for (A, B)
where
    A: TryToValue,
    B: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::Array(tuple_to_values_2(self)?))
    }
}

impl<A, B, C> TryToValue for (A, B, C)
where
    A: TryToValue,
    B: TryToValue,
    C: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::Array(tuple_to_values_3(self)?))
    }
}

impl<A, B, C, D> TryToValue for (A, B, C, D)
where
    A: TryToValue,
    B: TryToValue,
    C: TryToValue,
    D: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::Array(tuple_to_values_4(self)?))
    }
}

impl<A, B, C, D, E> TryToValue for (A, B, C, D, E)
where
    A: TryToValue,
    B: TryToValue,
    C: TryToValue,
    D: TryToValue,
    E: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::Array(tuple_to_values_5(self)?))
    }
}

impl<A, B, C, D, E, F> TryToValue for (A, B, C, D, E, F)
where
    A: TryToValue,
    B: TryToValue,
    C: TryToValue,
    D: TryToValue,
    E: TryToValue,
    F: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::Array(tuple_to_values_6(self)?))
    }
}

impl<A, B, C, D, E, F, G> TryToValue for (A, B, C, D, E, F, G)
where
    A: TryToValue,
    B: TryToValue,
    C: TryToValue,
    D: TryToValue,
    E: TryToValue,
    F: TryToValue,
    G: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::Array(tuple_to_values_7(self)?))
    }
}

impl<A, B, C, D, E, F, G, H> TryToValue for (A, B, C, D, E, F, G, H)
where
    A: TryToValue,
    B: TryToValue,
    C: TryToValue,
    D: TryToValue,
    E: TryToValue,
    F: TryToValue,
    G: TryToValue,
    H: TryToValue,
{
    fn try_to_value(&self) -> Result<Value, Error> {
        Ok(Value::Array(tuple_to_values_8(self)?))
    }
}

// if needed, implementations for more arguments can be implemented
