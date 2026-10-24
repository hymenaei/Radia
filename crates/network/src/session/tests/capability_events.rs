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
            let Value::Array(events) = fields.get("nested").expect("nested value") else { panic!("nested event should remain an array") };
            let Value::Map(event) = &events[0] else { panic!("nested event should remain a map") };
            assert_eq!(event.get("SeedCapability"), Some(&Value::String("[redacted]".to_owned())));
            assert_eq!(event.get("seed-capability"), Some(&Value::String("[redacted]".to_owned())));
        }
        _ => panic!("event body should remain a map"),
    }
}
