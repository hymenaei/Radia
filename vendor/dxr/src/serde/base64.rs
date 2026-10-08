use std::borrow::Cow;

use base64::DecodeError;
use serde::{Deserialize, Deserializer, Serializer};

use crate::base64::{decode, encode};

pub(crate) fn from_str(s: &str) -> Result<Cow<'static, [u8]>, DecodeError> {
    // filter out optional whitespace from input string:
    // some XML-RPC implementations line-wrap base64 encoded strings
    let stripped: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    decode(stripped).map(Cow::Owned)
}

pub(crate) fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let string = encode(bytes);
    serializer.serialize_str(&string)
}

pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Cow<'static, [u8]>, D::Error>
where
    D: Deserializer<'de>,
{
    let string = String::deserialize(deserializer)?;
    from_str(&string).map_err(serde::de::Error::custom)
}
