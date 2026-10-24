use std::{
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    str::from_utf8,
};

use llsd::Value;
use protocol::lludp::Message;
use url::Url;
use uuid::Uuid;

use crate::circuit::HandoffTarget;

pub(super) fn decode_crossed_region(body: &Value, agent_id: Uuid, session_id: Uuid) -> Result<HandoffTarget, &'static str> {
    let body = event_fields(body)?;
    let agent = event_block(body, "AgentData")?;
    if !matches!(agent.get("AgentID"), Some(Value::Uuid(value)) if *value == agent_id) {
        return Err("AgentData.AgentID does not match the authenticated agent");
    }
    if !matches!(agent.get("SessionID"), Some(Value::Uuid(value)) if *value == session_id) {
        return Err("AgentData.SessionID does not match the authenticated session");
    }
    let region = event_block(body, "RegionData")?;
    let Some(Value::Binary(raw_handle)) = region.get("RegionHandle") else {
        return Err("RegionData.RegionHandle is not LLSD binary data");
    };
    let raw_handle: [u8; 8] = raw_handle.as_slice().try_into().map_err(|_| "RegionData.RegionHandle is not 8 bytes")?;
    let Some(Value::Binary(raw_ip)) = region.get("SimIP") else {
        return Err("RegionData.SimIP is not LLSD binary data");
    };
    let raw_ip: [u8; 4] = raw_ip.as_slice().try_into().map_err(|_| "RegionData.SimIP is not 4 bytes")?;
    let port = match region.get("SimPort") {
        Some(Value::Integer(port)) => u16::try_from(*port).ok(),
        Some(Value::Binary(port)) => port.as_slice().try_into().ok().map(u16::from_be_bytes),
        _ => None,
    }
    .filter(|port| *port != 0)
    .ok_or("RegionData.SimPort is outside the valid range")?;
    let seed = match region.get("SeedCapability") {
        Some(Value::String(seed)) => seed.as_str(),
        Some(Value::Uri(seed)) => seed.as_str(),
        _ => return Err("RegionData.SeedCapability is not an LLSD URI or string"),
    };
    make_handoff_target(agent_id, session_id, u64::from_be_bytes(raw_handle), SocketAddr::from((Ipv4Addr::from(raw_ip), port)), seed)
}

pub(super) fn decode_event_teleport_finish(body: &Value, agent_id: Uuid, session_id: Uuid) -> Result<HandoffTarget, &'static str> {
    let info = event_block(event_fields(body)?, "Info")?;
    if !matches!(info.get("AgentID"), Some(Value::Uuid(value)) if *value == agent_id) {
        return Err("Info.AgentID does not match the authenticated agent");
    }
    let Some(Value::Binary(raw_handle)) = info.get("RegionHandle") else {
        return Err("Info.RegionHandle is not LLSD binary data");
    };
    let raw_handle: [u8; 8] = raw_handle.as_slice().try_into().map_err(|_| "Info.RegionHandle is not 8 bytes")?;
    let Some(Value::Binary(raw_ip)) = info.get("SimIP") else {
        return Err("Info.SimIP is not LLSD binary data");
    };
    let raw_ip: [u8; 4] = raw_ip.as_slice().try_into().map_err(|_| "Info.SimIP is not 4 bytes")?;
    let port = match info.get("SimPort") {
        Some(Value::Integer(port)) => u16::try_from(*port).ok(),
        Some(Value::Binary(port)) => port.as_slice().try_into().ok().map(u16::from_be_bytes),
        _ => None,
    }
    .filter(|port| *port != 0)
    .ok_or("Info.SimPort is outside the valid range")?;
    let seed = match info.get("SeedCapability") {
        Some(Value::String(seed)) => seed.as_str(),
        Some(Value::Uri(seed)) => seed.as_str(),
        _ => return Err("Info.SeedCapability is not an LLSD URI or string"),
    };
    make_handoff_target(agent_id, session_id, u64::from_be_bytes(raw_handle), SocketAddr::from((Ipv4Addr::from(raw_ip), port)), seed)
}

pub(super) fn decode_event_teleport_failed(body: &Value, agent_id: Uuid) -> Result<(), &'static str> {
    let info = event_block(event_fields(body)?, "Info")?;
    match info.get("AgentID") {
        Some(Value::Uuid(value)) if *value == agent_id => Ok(()),
        Some(Value::Uuid(_)) => Err("Info.AgentID does not match the authenticated agent"),
        _ => Err("Info.AgentID is not an LLSD UUID"),
    }
}

pub(super) fn event_fields(body: &Value) -> Result<&BTreeMap<String, Value>, &'static str> {
    let Value::Map(fields) = body else {
        return Err("event body is not an LLSD map");
    };
    Ok(fields)
}

pub(super) fn event_block<'a>(fields: &'a BTreeMap<String, Value>, name: &str) -> Result<&'a BTreeMap<String, Value>, &'static str> {
    match fields.get(name) {
        Some(Value::Map(block)) => Ok(block),
        Some(Value::Array(blocks)) => match blocks.as_slice() {
            [Value::Map(block)] => Ok(block),
            _ => Err("required LLSD block does not contain exactly one map"),
        },
        _ => Err("required LLSD block is not a map or single-entry array"),
    }
}

pub(super) fn decode_teleport_finish(message: &Message, agent_id: Uuid, session_id: Uuid) -> Result<HandoffTarget, &'static str> {
    let Message::TeleportFinish(finish) = message else {
        return Err("message is not TeleportFinish");
    };
    if finish.agent_id != agent_id {
        return Err("Info.AgentID does not match the authenticated agent");
    }
    let seed = finish.seed_capability.strip_suffix(&[0]).unwrap_or(&finish.seed_capability);
    let seed = from_utf8(seed).map_err(|_| "Info.SeedCapability is not UTF-8")?;
    make_handoff_target(
        agent_id,
        session_id,
        finish.region_handle,
        SocketAddr::from((Ipv4Addr::from(finish.sim_ip), u16::from_be_bytes(finish.sim_port))),
        seed,
    )
}

fn make_handoff_target(
    agent_id: Uuid,
    session_id: Uuid,
    region_handle: u64,
    endpoint: SocketAddr,
    seed: &str,
) -> Result<HandoffTarget, &'static str> {
    if endpoint.port() == 0
        || endpoint.ip().is_unspecified()
        || endpoint.ip().is_multicast()
        || endpoint.ip() == IpAddr::V4(Ipv4Addr::BROADCAST)
    {
        return Err("destination endpoint is invalid");
    }
    let seed_capability = Url::parse(seed).map_err(|_| "SeedCapability is not a valid URL")?;
    if !matches!(seed_capability.scheme(), "http" | "https")
        || seed_capability.host().is_none()
        || !seed_capability.username().is_empty()
        || seed_capability.password().is_some()
        || seed_capability.fragment().is_some()
    {
        return Err("SeedCapability is not a valid HTTP(S) URL");
    }
    Ok(HandoffTarget { agent_id, session_id, region_handle, endpoint, seed_capability })
}

pub(super) fn is_handoff_message(message: &Message) -> bool {
    matches!(message, Message::TeleportFinish(_) | Message::TeleportFailed(_))
}

pub(super) fn is_teleport_cancel(message: &Message, agent_id: Uuid, session_id: Uuid) -> bool {
    matches!(message, Message::TeleportCancel(cancel) if cancel.agent_id == agent_id && cancel.session_id == session_id)
}
