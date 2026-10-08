use serde::{Deserialize, Deserializer, Serializer};

pub(crate) fn from_str(s: &str) -> Result<bool, String> {
    match s {
        "1" => Ok(true),
        "0" => Ok(false),
        _ => Err(format!("Unsupported boolean value: {s}")),
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn serialize<S>(boolean: &bool, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let string = match boolean {
        true => "1",
        false => "0",
    };
    serializer.serialize_str(string)
}

pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let string = String::deserialize(deserializer)?;
    from_str(&string).map_err(serde::de::Error::custom)
}
