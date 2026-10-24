mod decode;
mod encoding;
mod error;
mod limits;
mod serialization;
mod value;

pub mod binary;
pub mod notation;
pub mod xml;

pub use self::error::{Error, Result};
pub use self::limits::Limits;
pub use self::value::{Binary, Date, Map, Uri, Value, uuid};
