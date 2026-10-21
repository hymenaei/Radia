use std::fmt;

use serde::{
    Deserializer,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};

use crate::Failure;

struct Seed<'a> {
    depth: usize,
    remaining: &'a mut usize,
}

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = Value;
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Value, D::Error> {
        if self.depth > 16 || *self.remaining == 0 {
            return Err(de::Error::custom("material JSON limit"));
        }
        *self.remaining -= 1;
        decoder.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Seed<'_> {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded material JSON")
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value).map(Value::Number).ok_or_else(|| E::custom("non-finite number"))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.into()))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        loop {
            let mut exhausted = 0;
            let remaining = if values.len() == 512 { &mut exhausted } else { &mut *self.remaining };
            let value = seq.next_element_seed(Seed { depth: self.depth + 1, remaining })?;
            let Some(value) = value else {
                break;
            };
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if key.len() > 128 || values.len() == 512 || values.contains_key(&key) {
                return Err(de::Error::custom("material map limit or duplicate key"));
            }
            let value = map.next_value_seed(Seed { depth: self.depth + 1, remaining: self.remaining })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

pub(crate) fn parse(bytes: &[u8]) -> Result<Value, Failure> {
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(Failure::TooLarge);
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = Seed { depth: 0, remaining: &mut 4096 }.deserialize(&mut decoder).map_err(|_| Failure::Malformed)?;
    decoder.end().map_err(|_| Failure::Malformed)?;
    Ok(value)
}
