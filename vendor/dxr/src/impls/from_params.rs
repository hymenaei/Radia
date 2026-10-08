use std::collections::HashMap;

use crate::datetime::DateTime;
use crate::error::Error;
use crate::traits::{TryFromParams, TryFromValue};
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

// for simple values, just call the impls for singletons / one-tuples

impl TryFromParams for Value {
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        let (value,): (Self,) = TryFromParams::try_from_params(values)?;
        Ok(value)
    }
}

impl TryFromParams for i32 {
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        let (value,): (Self,) = TryFromParams::try_from_params(values)?;
        Ok(value)
    }
}

#[cfg(feature = "i8")]
impl TryFromParams for i64 {
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        let (value,): (Self,) = TryFromParams::try_from_params(values)?;
        Ok(value)
    }
}

impl TryFromParams for bool {
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        let (value,): (Self,) = TryFromParams::try_from_params(values)?;
        Ok(value)
    }
}

impl TryFromParams for String {
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        let (value,): (Self,) = TryFromParams::try_from_params(values)?;
        Ok(value)
    }
}

impl TryFromParams for f64 {
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        let (value,): (Self,) = TryFromParams::try_from_params(values)?;
        Ok(value)
    }
}

impl TryFromParams for DateTime {
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        let (value,): (Self,) = TryFromParams::try_from_params(values)?;
        Ok(value)
    }
}

#[cfg(feature = "chrono")]
impl TryFromParams for chrono::NaiveDateTime {
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        let (value,): (DateTime,) = TryFromParams::try_from_params(values)?;
        Ok(value.into())
    }
}

#[cfg(feature = "jiff")]
impl TryFromParams for jiff::civil::DateTime {
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        let (value,): (DateTime,) = TryFromParams::try_from_params(values)?;
        Ok(value.into())
    }
}

#[cfg(feature = "time")]
impl TryFromParams for time::PrimitiveDateTime {
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        let (value,): (DateTime,) = TryFromParams::try_from_params(values)?;
        Ok(value.into())
    }
}

impl TryFromParams for Vec<u8> {
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        let (value,): (Self,) = TryFromParams::try_from_params(values)?;
        Ok(value)
    }
}

// handle optional values twice (not sure if this is a good idea):
// - check whether there *is* a value
// - check whether it is a <nil> value

#[cfg(feature = "nil")]
impl<T> TryFromParams for Option<T>
where
    T: TryFromValue,
{
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        // one value: convert or return None if it is a <nil/> value
        match values {
            [value] => Ok(Option::try_from_value(value)?),
            [] => Ok(None),
            _ => Err(Error::parameter_mismatch(values.len(), 1)),
        }
    }
}

// use collections as they are without unwrapping them

impl<T> TryFromParams for Vec<T>
where
    T: TryFromValue,
{
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        values.iter().map(T::try_from_value).collect()
    }
}

impl TryFromParams for () {
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        match values.len() {
            0 => Ok(()),
            n => Err(Error::parameter_mismatch(n, 0)),
        }
    }
}

// treat maps as a single value of a struct

impl<T, S: std::hash::BuildHasher> TryFromParams for HashMap<String, T, S>
where
    T: TryFromValue,
    S: Default,
{
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        let (value,): (Self,) = TryFromParams::try_from_params(values)?;
        Ok(value)
    }
}

// treat tuples as collections of values of different types

impl<T> TryFromParams for (T,)
where
    T: TryFromValue,
{
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        values_to_tuple_1(values)
    }
}

impl<A, B> TryFromParams for (A, B)
where
    A: TryFromValue,
    B: TryFromValue,
{
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        values_to_tuple_2(values)
    }
}

impl<A, B, C> TryFromParams for (A, B, C)
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
{
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        values_to_tuple_3(values)
    }
}

impl<A, B, C, D> TryFromParams for (A, B, C, D)
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
    D: TryFromValue,
{
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        values_to_tuple_4(values)
    }
}

impl<A, B, C, D, E> TryFromParams for (A, B, C, D, E)
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
    D: TryFromValue,
    E: TryFromValue,
{
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        values_to_tuple_5(values)
    }
}

impl<A, B, C, D, E, F> TryFromParams for (A, B, C, D, E, F)
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
    D: TryFromValue,
    E: TryFromValue,
    F: TryFromValue,
{
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        values_to_tuple_6(values)
    }
}

impl<A, B, C, D, E, F, G> TryFromParams for (A, B, C, D, E, F, G)
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
    D: TryFromValue,
    E: TryFromValue,
    F: TryFromValue,
    G: TryFromValue,
{
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        values_to_tuple_7(values)
    }
}

impl<A, B, C, D, E, F, G, H> TryFromParams for (A, B, C, D, E, F, G, H)
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
    fn try_from_params(values: &[Value]) -> Result<Self, Error> {
        values_to_tuple_8(values)
    }
}

// if needed, implementations for more arguments can be implemented
