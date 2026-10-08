#![allow(clippy::many_single_char_names)]
#![allow(clippy::type_complexity)]

use crate::error::Error;
use crate::traits::{TryFromValue, TryToValue};
use crate::types::Value;

pub(crate) fn tuple_to_values_1<T>((v,): &(T,)) -> Result<Vec<Value>, Error>
where
    T: TryToValue,
{
    Ok(vec![v.try_to_value()?])
}

pub(crate) fn values_to_tuple_1<T>(values: &[Value]) -> Result<(T,), Error>
where
    T: TryFromValue,
{
    match values {
        [value] => Ok((T::try_from_value(value)?,)),
        _ => Err(Error::parameter_mismatch(values.len(), 1)),
    }
}

pub(crate) fn tuple_to_values_2<A, B>((a, b): &(A, B)) -> Result<Vec<Value>, Error>
where
    A: TryToValue,
    B: TryToValue,
{
    Ok(vec![a.try_to_value()?, b.try_to_value()?])
}

pub(crate) fn values_to_tuple_2<A, B>(values: &[Value]) -> Result<(A, B), Error>
where
    A: TryFromValue,
    B: TryFromValue,
{
    match values {
        [a, b] => Ok((A::try_from_value(a)?, B::try_from_value(b)?)),
        _ => Err(Error::parameter_mismatch(values.len(), 2)),
    }
}

pub(crate) fn tuple_to_values_3<A, B, C>((a, b, c): &(A, B, C)) -> Result<Vec<Value>, Error>
where
    A: TryToValue,
    B: TryToValue,
    C: TryToValue,
{
    Ok(vec![a.try_to_value()?, b.try_to_value()?, c.try_to_value()?])
}

pub(crate) fn values_to_tuple_3<A, B, C>(values: &[Value]) -> Result<(A, B, C), Error>
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
{
    match values {
        [a, b, c] => Ok((A::try_from_value(a)?, B::try_from_value(b)?, C::try_from_value(c)?)),
        _ => Err(Error::parameter_mismatch(values.len(), 3)),
    }
}

pub(crate) fn tuple_to_values_4<A, B, C, D>((a, b, c, d): &(A, B, C, D)) -> Result<Vec<Value>, Error>
where
    A: TryToValue,
    B: TryToValue,
    C: TryToValue,
    D: TryToValue,
{
    Ok(vec![
        a.try_to_value()?,
        b.try_to_value()?,
        c.try_to_value()?,
        d.try_to_value()?,
    ])
}

pub(crate) fn values_to_tuple_4<A, B, C, D>(values: &[Value]) -> Result<(A, B, C, D), Error>
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
    D: TryFromValue,
{
    match values {
        [a, b, c, d] => Ok((
            A::try_from_value(a)?,
            B::try_from_value(b)?,
            C::try_from_value(c)?,
            D::try_from_value(d)?,
        )),
        _ => Err(Error::parameter_mismatch(values.len(), 4)),
    }
}

pub(crate) fn tuple_to_values_5<A, B, C, D, E>((a, b, c, d, e): &(A, B, C, D, E)) -> Result<Vec<Value>, Error>
where
    A: TryToValue,
    B: TryToValue,
    C: TryToValue,
    D: TryToValue,
    E: TryToValue,
{
    Ok(vec![
        a.try_to_value()?,
        b.try_to_value()?,
        c.try_to_value()?,
        d.try_to_value()?,
        e.try_to_value()?,
    ])
}

pub(crate) fn values_to_tuple_5<A, B, C, D, E>(values: &[Value]) -> Result<(A, B, C, D, E), Error>
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
    D: TryFromValue,
    E: TryFromValue,
{
    match values {
        [a, b, c, d, e] => Ok((
            A::try_from_value(a)?,
            B::try_from_value(b)?,
            C::try_from_value(c)?,
            D::try_from_value(d)?,
            E::try_from_value(e)?,
        )),
        _ => Err(Error::parameter_mismatch(values.len(), 5)),
    }
}

pub(crate) fn tuple_to_values_6<A, B, C, D, E, F>((a, b, c, d, e, f): &(A, B, C, D, E, F)) -> Result<Vec<Value>, Error>
where
    A: TryToValue,
    B: TryToValue,
    C: TryToValue,
    D: TryToValue,
    E: TryToValue,
    F: TryToValue,
{
    Ok(vec![
        a.try_to_value()?,
        b.try_to_value()?,
        c.try_to_value()?,
        d.try_to_value()?,
        e.try_to_value()?,
        f.try_to_value()?,
    ])
}

pub(crate) fn values_to_tuple_6<A, B, C, D, E, F>(values: &[Value]) -> Result<(A, B, C, D, E, F), Error>
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
    D: TryFromValue,
    E: TryFromValue,
    F: TryFromValue,
{
    match values {
        [a, b, c, d, e, f] => Ok((
            A::try_from_value(a)?,
            B::try_from_value(b)?,
            C::try_from_value(c)?,
            D::try_from_value(d)?,
            E::try_from_value(e)?,
            F::try_from_value(f)?,
        )),
        _ => Err(Error::parameter_mismatch(values.len(), 6)),
    }
}

pub(crate) fn tuple_to_values_7<A, B, C, D, E, F, G>(
    (a, b, c, d, e, f, g): &(A, B, C, D, E, F, G),
) -> Result<Vec<Value>, Error>
where
    A: TryToValue,
    B: TryToValue,
    C: TryToValue,
    D: TryToValue,
    E: TryToValue,
    F: TryToValue,
    G: TryToValue,
{
    Ok(vec![
        a.try_to_value()?,
        b.try_to_value()?,
        c.try_to_value()?,
        d.try_to_value()?,
        e.try_to_value()?,
        f.try_to_value()?,
        g.try_to_value()?,
    ])
}

pub(crate) fn values_to_tuple_7<A, B, C, D, E, F, G>(values: &[Value]) -> Result<(A, B, C, D, E, F, G), Error>
where
    A: TryFromValue,
    B: TryFromValue,
    C: TryFromValue,
    D: TryFromValue,
    E: TryFromValue,
    F: TryFromValue,
    G: TryFromValue,
{
    match values {
        [a, b, c, d, e, f, g] => Ok((
            A::try_from_value(a)?,
            B::try_from_value(b)?,
            C::try_from_value(c)?,
            D::try_from_value(d)?,
            E::try_from_value(e)?,
            F::try_from_value(f)?,
            G::try_from_value(g)?,
        )),
        _ => Err(Error::parameter_mismatch(values.len(), 7)),
    }
}

pub(crate) fn tuple_to_values_8<A, B, C, D, E, F, G, H>(
    (a, b, c, d, e, f, g, h): &(A, B, C, D, E, F, G, H),
) -> Result<Vec<Value>, Error>
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
    Ok(vec![
        a.try_to_value()?,
        b.try_to_value()?,
        c.try_to_value()?,
        d.try_to_value()?,
        e.try_to_value()?,
        f.try_to_value()?,
        g.try_to_value()?,
        h.try_to_value()?,
    ])
}

pub(crate) fn values_to_tuple_8<A, B, C, D, E, F, G, H>(values: &[Value]) -> Result<(A, B, C, D, E, F, G, H), Error>
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
    match values {
        [a, b, c, d, e, f, g, h] => Ok((
            A::try_from_value(a)?,
            B::try_from_value(b)?,
            C::try_from_value(c)?,
            D::try_from_value(d)?,
            E::try_from_value(e)?,
            F::try_from_value(f)?,
            G::try_from_value(g)?,
            H::try_from_value(h)?,
        )),
        _ => Err(Error::parameter_mismatch(values.len(), 8)),
    }
}

// if needed, implementations for more arguments can be implemented
