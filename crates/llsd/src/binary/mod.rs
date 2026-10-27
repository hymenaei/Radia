mod de;
mod ser;

pub use de::{from_slice_prefix_with_limits, from_slice_with_limits};

use std::io::{Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{Limits, Result, encoding::read_limited};

pub fn from_slice<T: DeserializeOwned>(input: &[u8]) -> Result<T> {
    from_slice_with_limits(input, Limits::default())
}

pub fn from_reader<T: DeserializeOwned, R: Read>(reader: R) -> Result<T> {
    from_reader_with_limits(reader, Limits::default())
}

pub fn from_reader_with_limits<T: DeserializeOwned, R: Read>(mut reader: R, limits: Limits) -> Result<T> {
    let bytes = read_limited(&mut reader, limits.max_bytes)?;
    from_slice_with_limits(&bytes, limits)
}

pub fn to_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    crate::serialization::to_vec::<ser::Encoder, _>(value, None)
}

pub fn to_writer<T: Serialize + ?Sized, W: Write>(writer: W, value: &T) -> Result<()> {
    crate::serialization::to_writer::<ser::Encoder, _, _>(writer, value)
}
