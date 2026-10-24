use std::{
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use llsd::{Limits, Map, Value, binary, notation, xml};
use protocol::lludp::Message;
use render::scene::{EnvironmentProperty, EnvironmentPush};
use tokio::sync::mpsc;
use utils::{AssetId, ExperienceId};
use uuid::Uuid;
use world::Region;

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
    let Message::GenericMessage(message) = message else { return Ok(None) };
    let method = message.method_data.method.as_slice();
    if method.strip_suffix(&[0]).unwrap_or(method) != b"PushExpEnvironment" {
        return Ok(None);
    }
    if message.agent_data.agent_id != agent {
        return Err(InvalidPush);
    }
    let Some(parameter) = message.param_list.as_ref().and_then(|parameters| parameters.first()) else { return Err(InvalidPush) };
    let payload = parameter.parameter.as_slice();
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
    let experience = ExperienceId::new(message.method_data.invoice.unwrap_or_else(Uuid::nil));
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

#[cfg(test)]
mod tests {
    use protocol::messages::GenericMessage;
    use protocol::messages::generic_message::{AgentData, MethodData, Parameter};

    use super::*;

    fn message(agent: Uuid, invoice: Uuid, body: &str) -> Message {
        Message::GenericMessage(GenericMessage {
            agent_data: AgentData { agent_id: agent, session_id: Uuid::nil(), transaction_id: Uuid::nil() },
            method_data: MethodData { method: b"PushExpEnvironment\0".to_vec(), invoice: Some(invoice) },
            param_list: Some(vec![Parameter { parameter: body.as_bytes().to_vec() }]),
        })
    }

    #[test]
    fn invoice_controls_identity() {
        let agent = Uuid::from_u128(1);
        let experience = Uuid::from_u128(2);
        let body = "{'action':'PushFullEnvironment','public_id':u00000000-0000-0000-0000-000000000003,'action_data':{'asset_id':u00000000-0000-0000-0000-000000000004,'transition_time':r2.5}}";
        let push = message(agent, experience, body);
        let Some(EnvironmentPush::Full { experience: actual, asset, transition }) = decode(&push, agent).unwrap() else {
            panic!("full push missing")
        };
        assert!(decode(&message(agent, Uuid::nil(), body), agent).is_err(), "a full push requires a non-nil experience ID");
        assert_eq!(actual.uuid(), experience);
        assert_eq!(asset.uuid(), Uuid::from_u128(4));
        assert_eq!(transition, Duration::from_millis(2500));
        assert!(decode(&push, Uuid::from_u128(5)).is_err(), "wrong-agent push must not be accepted");
    }

    #[test]
    fn missing_patch_clears_experience() {
        let agent = Uuid::from_u128(1);
        let experience = Uuid::from_u128(2);
        for settings in ["{}", "{'sky':!,'water':!}"] {
            let body = format!("{{'action':'PushPartialEnvironment','action_data':{{'settings':{settings}}}}}");
            let Some(EnvironmentPush::Clear { experience: Some(id), .. }) = decode(&message(agent, experience, &body), agent).unwrap()
            else {
                panic!("missing channels must clear")
            };
            assert_eq!(id.uuid(), experience);
        }
        let body = "{'action':'PushPartialEnvironment','action_data':{'settings':{'sky':{},'water':{}}}}";
        let Some(EnvironmentPush::Partial { sky, water, .. }) = decode(&message(agent, experience, body), agent).unwrap() else {
            panic!("empty maps must be a no-op patch")
        };
        assert!(sky.is_empty() && water.is_empty());
        for body in ["{'action':'ClearEnvironment'}", "{'action':'ClearEnvironment','action_data':{'transition_time':i-1}}"] {
            assert!(matches!(
                decode(&message(agent, Uuid::nil(), body), agent).unwrap(),
                Some(EnvironmentPush::Clear { experience: None, transition: Duration::ZERO })
            ));
        }
        assert!(decode(&message(agent, experience, "{'action':'PushPartialEnvironment'}"), agent).unwrap().is_none());
    }

    #[test]
    fn partial_values_preserve_sources() {
        let agent = Uuid::from_u128(1);
        let body =
            "{'action':'PushPartialEnvironment','action_data':{'settings':{'sky':{'gamma':r1.4},'water':{'water_fog_density':r2.0}}}}";
        let Some(EnvironmentPush::Partial { sky, water, .. }) = decode(&message(agent, Uuid::from_u128(2), body), agent).unwrap() else {
            panic!("partial push missing")
        };
        assert_eq!(sky[0].name, "gamma");
        assert_eq!(notation::from_slice::<Value>(&sky[0].source).unwrap(), Value::Real(1.4));
        assert_eq!(water[0].name, "water_fog_density");
        for body in [
            "{'action':'PushFullEnvironment','action_data':{'asset_id':u00000000-0000-0000-0000-000000000000}}",
            "{'action':'PushPartialEnvironment','action_data':{'settings':{'sky':i1}}}",
            "{'action':'ClearEnvironment','action_data':{'transition_time':rnan}}",
        ] {
            assert!(decode(&message(agent, Uuid::from_u128(2), body), agent).is_err(), "invalid push accepted: {body}");
        }
    }
}
