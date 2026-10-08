use crate::datetime::DateTime;
use serde::{Deserialize, Deserializer, Serializer};

pub(crate) fn from_str(s: &str) -> Result<DateTime, String> {
    match s.parse() {
        Ok(date) => Ok(date),
        Err(error) => Err(error.to_string()),
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn serialize<S>(datetime: &DateTime, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let string = datetime.to_string();
    serializer.serialize_str(&string)
}

pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<DateTime, D::Error>
where
    D: Deserializer<'de>,
{
    let string = String::deserialize(deserializer)?;
    from_str(&string).map_err(serde::de::Error::custom)
}
