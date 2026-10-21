use super::*;
use protocol::lludp::BlockValues;

fn message(agent: UUID, invoice: UUID, body: &str) -> Message {
    let schema = SchemaMessage::schema_by_name("GenericMessage").unwrap();
    Message::Schema(
        SchemaMessage::new(
            schema,
            vec![
                BlockValues::single(vec![FieldValue::UUID(agent), FieldValue::UUID(UUID::nil()), FieldValue::UUID(UUID::nil())]),
                BlockValues::single(vec![FieldValue::Bytes(b"PushExpEnvironment\0".to_vec()), FieldValue::UUID(invoice)]),
                BlockValues::single(vec![FieldValue::Bytes(body.as_bytes().to_vec())]),
            ],
        )
        .unwrap(),
    )
}

#[test]
fn invoice_controls_identity() {
    let agent = UUID::from_u128(1);
    let experience = UUID::from_u128(2);
    let body = "{'action':'PushFullEnvironment','public_id':u00000000-0000-0000-0000-000000000003,'action_data':{'asset_id':u00000000-0000-0000-0000-000000000004,'transition_time':r2.5}}";
    let push = message(agent, experience, body);
    let Some(EnvironmentPush::Full { experience: actual, asset, transition }) = decode(&push, agent).unwrap() else {
        panic!("full push missing")
    };
    assert_eq!(actual.uuid(), experience);
    assert_eq!(asset.uuid(), UUID::from_u128(4));
    assert_eq!(transition, Duration::from_millis(2500));
    assert!(decode(&push, UUID::from_u128(5)).is_err(), "wrong-agent push must not be accepted");
}

#[test]
fn missing_patch_clears_experience() {
    let agent = UUID::from_u128(1);
    let experience = UUID::from_u128(2);
    for settings in ["{}", "{'sky':!,'water':!}"] {
        let body = format!("{{'action':'PushPartialEnvironment','action_data':{{'settings':{settings}}}}}");
        let Some(EnvironmentPush::Clear { experience: Some(id), .. }) = decode(&message(agent, experience, &body), agent).unwrap() else {
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
            decode(&message(agent, UUID::nil(), body), agent).unwrap(),
            Some(EnvironmentPush::Clear { experience: None, transition: Duration::ZERO })
        ));
    }
    assert!(decode(&message(agent, experience, "{'action':'PushPartialEnvironment'}"), agent).unwrap().is_none());
}

#[test]
fn partial_values_preserve_sources() {
    let agent = UUID::from_u128(1);
    let body = "{'action':'PushPartialEnvironment','action_data':{'settings':{'sky':{'gamma':r1.4},'water':{'water_fog_density':r2.0}}}}";
    let Some(EnvironmentPush::Partial { sky, water, .. }) = decode(&message(agent, UUID::from_u128(2), body), agent).unwrap() else {
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
        assert!(decode(&message(agent, UUID::from_u128(2), body), agent).is_err(), "invalid push accepted: {body}");
    }
}
