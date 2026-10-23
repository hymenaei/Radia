use std::net::{Ipv4Addr, SocketAddr};

use llsd::Value;
use url::Url;
use uuid::Uuid;

const MAX_SIMULATORS_PER_ENABLE_EVENT: usize = 32;

pub(super) fn decode_enable_simulators(body: &Value) -> Result<Vec<(u64, SocketAddr)>, &'static str> {
    let body = super::handoff::event_fields(body)?;
    let Some(Value::Array(simulators)) = body.get("SimulatorInfo") else {
        return Err("SimulatorInfo is not an LLSD array");
    };
    if simulators.is_empty() || simulators.len() > MAX_SIMULATORS_PER_ENABLE_EVENT {
        return Err("SimulatorInfo has an invalid number of entries");
    }

    simulators
        .iter()
        .map(|simulator| {
            let Value::Map(simulator) = simulator else {
                return Err("SimulatorInfo entry is not an LLSD map");
            };
            let Some(Value::Binary(handle)) = simulator.get("Handle") else {
                return Err("Handle is not LLSD binary data");
            };
            let handle: [u8; 8] = handle.as_slice().try_into().map_err(|_| "Handle is not 8 bytes")?;
            let Some(Value::Binary(ip)) = simulator.get("IP") else {
                return Err("IP is not LLSD binary data");
            };
            let ip: [u8; 4] = ip.as_slice().try_into().map_err(|_| "IP is not 4 bytes")?;
            let Some(Value::Integer(port)) = simulator.get("Port") else {
                return Err("Port is not an LLSD integer");
            };
            let port = u16::try_from(*port).ok().filter(|port| *port != 0).ok_or("Port is outside the valid range")?;
            let endpoint = SocketAddr::from((Ipv4Addr::from(ip), port));
            Ok((u64::from_be_bytes(handle), endpoint))
        })
        .collect()
}

pub(super) fn decode_agent_communication(body: &Value, agent_id: Uuid) -> Result<(SocketAddr, Url), &'static str> {
    let body = super::handoff::event_fields(body)?;
    match body.get("agent-id") {
        Some(Value::Uuid(value)) if *value == agent_id => {}
        Some(Value::Uuid(_)) => return Err("agent-id does not match the logged-in agent"),
        _ => return Err("agent-id is not an LLSD UUID"),
    }
    let Some(Value::String(address)) = body.get("sim-ip-and-port") else {
        return Err("sim-ip-and-port is not an LLSD string");
    };
    let endpoint = address.parse().map_err(|_| "sim-ip-and-port is not a socket address")?;
    let Some(Value::String(seed)) = body.get("seed-capability") else {
        return Err("seed-capability is not an LLSD string");
    };
    let seed = Url::parse(seed).map_err(|_| "seed-capability is not a valid URL")?;
    Ok((endpoint, seed))
}

pub(super) fn redact_seed_capability(body: &mut Value) {
    fn redact(value: &mut Value) {
        match value {
            Value::Map(fields) => {
                for (name, value) in fields {
                    if matches!(name.as_str(), "SeedCapability" | "seed-capability") {
                        *value = Value::String("[redacted]".to_owned());
                    } else {
                        redact(value);
                    }
                }
            }
            Value::Array(values) => values.iter_mut().for_each(redact),
            _ => {}
        }
    }

    redact(body);
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use llsd::{Map, Value};
    use url::Url;
    use uuid::Uuid;

    use super::MAX_SIMULATORS_PER_ENABLE_EVENT;
    use super::{decode_agent_communication, decode_enable_simulators, redact_seed_capability};

    fn simulator(handle: Vec<u8>, ip: Vec<u8>, port: i32) -> Value {
        Value::Map(Map::from([
            ("Handle".to_owned(), Value::Binary(handle.into())),
            ("IP".to_owned(), Value::Binary(ip.into())),
            ("Port".to_owned(), Value::Integer(port)),
        ]))
    }

    fn make_enable_event(entries: Vec<Value>) -> Value {
        Value::Map(Map::from([("SimulatorInfo".to_owned(), Value::Array(entries))]))
    }

    #[test]
    fn enable_event() {
        let value = make_enable_event(vec![simulator(0x1000_u64.to_be_bytes().to_vec(), [192, 0, 2, 7].to_vec(), 13_000)]);
        let endpoint = "192.0.2.7:13000".parse::<SocketAddr>().expect("simulator endpoint");
        assert_eq!(decode_enable_simulators(&value).expect("valid neighbor event"), vec![(0x1000, endpoint)]);
    }

    #[test]
    fn bad_entry_lengths() {
        let value = make_enable_event(vec![simulator(vec![0; 7], vec![127, 0, 0], 13_000)]);
        assert_eq!(decode_enable_simulators(&value), Err("Handle is not 8 bytes"));

        let value = make_enable_event(vec![simulator(vec![0; 8], vec![127, 0, 0], 13_000)]);
        assert_eq!(decode_enable_simulators(&value), Err("IP is not 4 bytes"));
    }

    #[test]
    fn bad_port() {
        let value = make_enable_event(vec![simulator(vec![0; 8], vec![127, 0, 0, 1], 0)]);
        assert_eq!(decode_enable_simulators(&value), Err("Port is outside the valid range"));
    }

    #[test]
    fn too_many_entries() {
        let entries = (0..=MAX_SIMULATORS_PER_ENABLE_EVENT)
            .map(|index| simulator((index as u64).to_be_bytes().to_vec(), vec![127, 0, 0, 1], 13_000))
            .collect();
        assert_eq!(decode_enable_simulators(&make_enable_event(entries)), Err("SimulatorInfo has an invalid number of entries"));
    }

    #[test]
    fn agent_communication() {
        let agent = Uuid::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("agent UUID");
        let body = Value::Map(Map::from([
            ("agent-id".to_owned(), Value::Uuid(agent)),
            ("sim-ip-and-port".to_owned(), Value::String("192.0.2.7:13000".to_owned())),
            ("seed-capability".to_owned(), Value::String("https://sim.example.invalid/seed".to_owned())),
        ]));
        let endpoint = "192.0.2.7:13000".parse::<SocketAddr>().expect("simulator endpoint");
        let seed = Url::parse("https://sim.example.invalid/seed").expect("seed URL");
        assert_eq!(decode_agent_communication(&body, agent), Ok((endpoint, seed)));
        assert_eq!(decode_agent_communication(&body, Uuid::default()), Err("agent-id does not match the logged-in agent"));
    }

    #[test]
    fn seed_is_redacted() {
        let mut body = Value::Map(Map::from([("seed-capability".to_owned(), Value::String("https://sim.invalid/private".to_owned()))]));
        redact_seed_capability(&mut body);
        assert_eq!(body, Value::Map(Map::from([("seed-capability".to_owned(), Value::String("[redacted]".to_owned()))])));
    }

    #[test]
    fn redacts_nested_seed_in_place() {
        let unrelated = "x".repeat(32 * 1024);
        let mut body = Value::Map(Map::from([
            ("unrelated".to_owned(), Value::String(unrelated)),
            (
                "nested".to_owned(),
                Value::Array(vec![Value::Map(Map::from([
                    ("SeedCapability".to_owned(), Value::String("https://sim.invalid/private".to_owned())),
                    ("seed-capability".to_owned(), Value::String("https://sim.invalid/private-too".to_owned())),
                ]))]),
            ),
        ]));
        let unrelated_ptr = match &body {
            Value::Map(fields) => match fields.get("unrelated").expect("unrelated value") {
                Value::String(value) => value.as_ptr(),
                _ => panic!("unrelated value should be a string"),
            },
            _ => panic!("event body should be a map"),
        };

        redact_seed_capability(&mut body);

        match &body {
            Value::Map(fields) => {
                let Value::String(value) = fields.get("unrelated").expect("unrelated value") else {
                    panic!("unrelated value should remain a string")
                };
                assert_eq!(value.as_ptr(), unrelated_ptr, "redaction must preserve unrelated owned allocations");
                let Value::Array(events) = fields.get("nested").expect("nested value") else {
                    panic!("nested event should remain an array")
                };
                let Value::Map(event) = &events[0] else { panic!("nested event should remain a map") };
                assert_eq!(event.get("SeedCapability"), Some(&Value::String("[redacted]".to_owned())));
                assert_eq!(event.get("seed-capability"), Some(&Value::String("[redacted]".to_owned())));
            }
            _ => panic!("event body should remain a map"),
        }
    }
}
