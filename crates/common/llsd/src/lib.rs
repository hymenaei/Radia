//! Parsers and serializers for Linden Lab Structured Data (LLSD).
//!
//! Use [`Value`] for dynamically shaped data or Serde types with the [`xml`], [`binary`], and [`notation`] codecs.
//! Unit enum variants use LLSD strings. Newtype variants use a single-entry map from variant name to payload; tuple and struct variants use an array or map payload, respectively.
//!
//! # Examples
//!
//! ```
//! use llsd::Value;
//!
//! let value = Value::String("ready".into());
//! let encoded = llsd::xml::to_vec(&value)?;
//! let decoded = llsd::xml::from_slice::<Value>(&encoded)?;
//! assert_eq!(decoded, value);
//! # Ok::<(), llsd::Error>(())
//! ```

mod error;
mod limits;
mod ser;
mod value;

pub mod binary;
pub mod notation;
pub mod uuid;
pub mod xml;

pub use error::{Error, Result};
pub use limits::Limits;
pub use value::{Binary, Date, Map, URI, Value};

pub(crate) fn base64_decoded_len(input: &[u8]) -> Result<usize> {
    if !input.len().is_multiple_of(4) {
        return Err(Error::InvalidValue("invalid LLSD base64 length".into()));
    }
    let padding = input.iter().rev().take_while(|&&byte| byte == b'=').count();
    if padding > 2 {
        return Err(Error::InvalidValue("invalid LLSD base64 padding".into()));
    }
    input
        .len()
        .checked_div(4)
        .and_then(|groups| groups.checked_mul(3))
        .and_then(|length| length.checked_sub(padding))
        .ok_or_else(|| Error::InvalidValue("invalid LLSD base64 length".into()))
}
