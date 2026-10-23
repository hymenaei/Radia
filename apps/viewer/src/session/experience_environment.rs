use std::{
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use llsd::{Limits, Map, Value, binary, notation, xml};
use protocol::lludp::{FieldValue, Message, SchemaMessage};
use scene::{EnvironmentProperty, EnvironmentPush, ExperienceId};
use tokio::sync::mpsc;
use utils::AssetId;
use uuid::Uuid;
use world::Region;

#[cfg(test)]
#[path = "experience_environment_tests.rs"]
mod tests;

pub(crate) struct EnvironmentCommand {
    pub(crate) region: Region,
    pub(crate) push: EnvironmentPush,
}

#[derive(Clone)]
pub(crate) struct EnvironmentInbox(Arc<Mutex<Option<mpsc::Receiver<EnvironmentCommand>>>>);

impl EnvironmentInbox {
    pub(crate) fn channel() -> (mpsc::Sender<EnvironmentCommand>, Self) {
        let (sender, receiver) = mpsc::channel(64);
        (sender, Self(Arc::new(Mutex::new(Some(receiver)))))
    }

    pub(crate) fn take(&self) -> Option<mpsc::Receiver<EnvironmentCommand>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct InvalidPush;

pub(super) fn decode(message: &Message, agent: Uuid) -> Result<Option<EnvironmentPush>, InvalidPush> {
    let Message::Schema(message) = message else { return Ok(None) };
    if message.schema().name != "GenericMessage" {
        return Ok(None);
    }
    let Some(FieldValue::Bytes(method)) = field(message, "MethodData", "Method") else { return Err(InvalidPush) };
    if method.strip_suffix(&[0]).unwrap_or(method) != b"PushExpEnvironment" {
        return Ok(None);
    }
    if field(message, "AgentData", "AgentID") != Some(&FieldValue::Uuid(agent)) {
        return Err(InvalidPush);
    }
    let Some(FieldValue::Uuid(invoice)) = field(message, "MethodData", "Invoice") else { return Err(InvalidPush) };
    let Some(FieldValue::Bytes(payload)) = field(message, "ParamList", "Parameter") else { return Err(InvalidPush) };
    if payload.len() > 255 {
        return Err(InvalidPush);
    }
    let payload = payload.strip_suffix(&[0]).unwrap_or(payload);
    let start = payload.iter().position(|byte| !byte.is_ascii_whitespace()).unwrap_or(payload.len());
    let payload = payload.get(start..).ok_or(InvalidPush)?;
    let limits = Limits {
        max_bytes: 255,
        max_depth: 8,
        max_values: 128,
        max_container_items: 56,
        max_string_bytes: 255,
        max_map_key_bytes: 128,
        max_binary_bytes: 0,
    };
    let parsed = if payload.starts_with(b"<?llsd/binary") {
        binary::from_slice_with_limits(payload, limits)
    } else if payload.starts_with(b"<") && !payload.starts_with(b"<?llsd/notation") {
        xml::from_slice_with_limits(payload, limits)
    } else {
        notation::from_slice_with_limits(payload, limits)
    };
    let Value::Map(body) = parsed.map_err(|_| InvalidPush)? else { return Err(InvalidPush) };
    let Some(Value::String(action)) = body.get("action") else { return Err(InvalidPush) };
    let empty = Map::new();
    let data = match body.get("action_data") {
        None | Some(Value::Undefined) => &empty,
        Some(Value::Map(data)) => data,
        _ => return Err(InvalidPush),
    };
    let seconds = match data.get("transition_time") {
        None | Some(Value::Undefined) => 0.0,
        Some(Value::Integer(value)) => f64::from(*value),
        Some(Value::Real(value)) if value.is_finite() => *value,
        _ => return Err(InvalidPush),
    };
    let transition = Duration::try_from_secs_f64(seconds.max(0.0)).map_err(|_| InvalidPush)?;
    let experience = ExperienceId::new(*invoice);
    match action.as_str() {
        "ClearEnvironment" => Ok(Some(EnvironmentPush::Clear { experience, transition })),
        "PushFullEnvironment" => {
            let experience = experience.ok_or(InvalidPush)?;
            let Some(Value::Uuid(asset)) = data.get("asset_id") else { return Err(InvalidPush) };
            if asset.is_nil() {
                return Err(InvalidPush);
            }
            Ok(Some(EnvironmentPush::Full { experience, asset: AssetId::new(*asset), transition }))
        }
        "PushPartialEnvironment" => {
            let experience = experience.ok_or(InvalidPush)?;
            let settings = match data.get("settings") {
                None | Some(Value::Undefined) => return Ok(None),
                Some(Value::Map(settings)) => settings,
                _ => return Err(InvalidPush),
            };
            let sky = settings.get("sky").filter(|value| !matches!(value, Value::Undefined));
            let water = settings.get("water").filter(|value| !matches!(value, Value::Undefined));
            if sky.is_none() && water.is_none() {
                return Ok(Some(EnvironmentPush::Clear { experience: Some(experience), transition }));
            }
            Ok(Some(EnvironmentPush::Partial { experience, sky: properties(sky)?, water: properties(water)?, transition }))
        }
        _ => Err(InvalidPush),
    }
}

fn properties(value: Option<&Value>) -> Result<Vec<EnvironmentProperty>, InvalidPush> {
    let Some(value) = value else { return Ok(Vec::new()) };
    let Value::Map(fields) = value else { return Err(InvalidPush) };
    fields
        .iter()
        .map(|(name, value)| {
            let source = notation::to_vec(value).map_err(|_| InvalidPush)?;
            Ok(EnvironmentProperty { name: name.clone(), source: source.into() })
        })
        .collect()
}

fn field<'a>(message: &'a SchemaMessage, block: &str, name: &str) -> Option<&'a FieldValue> {
    let index = message.schema().blocks.iter().position(|schema| schema.name == block)?;
    let schema = message.schema().blocks.get(index)?;
    let field = schema.fields.iter().position(|field| field.name == name)?;
    if message.field_was_supplied(index, 0, field) != Some(true) {
        return None;
    }
    message.blocks().get(index)?.instances.first()?.fields.get(field)
}
