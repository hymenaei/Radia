use protocol::{
    LLUdpError,
    lludp::{Message, Packet},
    messages::{ExtraParamUpdate, ObjectExtraParams, improved_terse_object_update, object_update, object_update_compressed},
};
use test_support::{lludp::vectors, parse_hex_fixture};
use uuid::Uuid;

fn decode(hex: &str) -> (Vec<u8>, Packet) {
    let bytes = parse_hex_fixture(hex).expect("valid protocol fixture");
    let packet = Packet::decode(&bytes).expect("decode independent fixture");
    (bytes, packet)
}

fn unique_offset(bytes: &[u8], needle: &[u8]) -> usize {
    let mut offsets = bytes.windows(needle.len()).enumerate().filter_map(|(offset, candidate)| (candidate == needle).then_some(offset));
    let offset = offsets.next().expect("fixture field is present");
    assert!(offsets.next().is_none(), "fixture field {needle:02x?} is unique");
    offset
}

fn assert_reencodes(packet: &Packet, bytes: &[u8]) {
    assert_eq!(packet.encode().expect("re-encode fixture"), bytes);
}

#[test]
fn object_extra_params_validates_param_size() {
    let (bytes, packet) = decode(vectors::OBJECT_EXTRA_PARAMS);
    let Message::ObjectExtraParams(message) = packet.message() else {
        panic!("expected ObjectExtraParams");
    };
    assert_eq!(message.agent_id, Uuid::from_bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]));
    assert_eq!(message.session_id, Uuid::from_bytes([16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31]));
    let mut expected_data = vec![1, 0];
    expected_data.extend(0x20..=0x2f);
    assert_eq!(
        message.object_data,
        [ExtraParamUpdate { local_id: 0x0102_0304, parameter_type: 0x0080, in_use: true, data: expected_data }]
    );
    assert_reencodes(&packet, &bytes);

    let mut mismatched_size = bytes;
    let local_id_offset = unique_offset(&mismatched_size, &0x0102_0304_u32.to_le_bytes());
    let parameter_size_offset = local_id_offset + 7;
    mismatched_size[parameter_size_offset..parameter_size_offset + 4].copy_from_slice(&17_u32.to_le_bytes());
    assert!(matches!(Packet::decode(&mismatched_size), Err(LLUdpError::InvalidMessage)));
}

#[test]
fn object_extra_params_checks_clear_size() {
    let message = ObjectExtraParams {
        agent_id: Uuid::nil(),
        session_id: Uuid::nil(),
        object_data: vec![ExtraParamUpdate { local_id: 0x0102_0304, parameter_type: 0x0080, in_use: false, data: vec![1] }],
    };
    let bytes = Packet::new(1, Message::ObjectExtraParams(message)).with_zero_coding(false).encode().expect("encode clear message");
    let mut mismatched_size = bytes;
    let local_id_offset = unique_offset(&mismatched_size, &0x0102_0304_u32.to_le_bytes());
    let parameter_size_offset = local_id_offset + 7;
    mismatched_size[parameter_size_offset..parameter_size_offset + 4].copy_from_slice(&2_u32.to_le_bytes());
    assert!(matches!(Packet::decode(&mismatched_size), Err(LLUdpError::InvalidMessage)));
}

#[test]
fn extra_params_respect_variable1_limit() {
    let parameter_data = |count: u8| {
        let mut data = vec![count];
        data.resize(1 + usize::from(count) * 17, 0);
        data
    };
    let update = |data| ObjectExtraParams {
        agent_id: Uuid::nil(),
        session_id: Uuid::nil(),
        object_data: vec![ExtraParamUpdate { local_id: 1, parameter_type: 0x0080, in_use: true, data }],
    };

    let fourteen_bindings = Packet::new(1, Message::ObjectExtraParams(update(parameter_data(14)))).with_zero_coding(false).encode();
    assert!(fourteen_bindings.is_ok(), "14 bindings fit ObjectExtraParams ParamData Variable 1");

    let fifteen_bindings = Packet::new(1, Message::ObjectExtraParams(update(parameter_data(15)))).with_zero_coding(false).encode();
    assert!(matches!(fifteen_bindings, Err(LLUdpError::VariableFieldTooLong)));
}

#[test]
fn texture_entry_paths_enforce_bounds() {
    let payload = vec![0; 47];
    let regular_limit = vec![0; object_update::MAX_TEXTURE_ENTRY_BYTES];
    assert!(object_update::decode_texture_entry(&regular_limit).unwrap().is_some());
    assert_eq!(object_update::decode_texture_entry(&vec![0; object_update::MAX_TEXTURE_ENTRY_BYTES + 1]), Err(LLUdpError::InvalidMessage));

    let compressed_limit = vec![0; object_update_compressed::MAX_TEXTURE_ENTRY_BYTES];
    assert!(object_update_compressed::decode_texture_entry(&compressed_limit).unwrap().is_some());
    assert_eq!(
        object_update_compressed::decode_texture_entry(&vec![0; object_update_compressed::MAX_TEXTURE_ENTRY_BYTES + 1]),
        Err(LLUdpError::InvalidMessage)
    );

    let binary_data = |payload: &[u8]| {
        let mut field = Vec::with_capacity(payload.len() + 4);
        field.extend_from_slice(&i32::try_from(payload.len()).expect("test field length fits").to_le_bytes());
        field.extend_from_slice(payload);
        field
    };
    let terse_limit = binary_data(&vec![0; improved_terse_object_update::MAX_TEXTURE_ENTRY_BYTES]);
    assert_eq!(terse_limit.len(), improved_terse_object_update::MAX_TEXTURE_ENTRY_FIELD_BYTES);
    assert!(improved_terse_object_update::decode_texture_entry(&terse_limit).unwrap().is_some());
    assert!(improved_terse_object_update::decode_texture_entry(&binary_data(&payload)).unwrap().is_some());
    assert_eq!(
        improved_terse_object_update::decode_texture_entry(&vec![0; improved_terse_object_update::MAX_TEXTURE_ENTRY_FIELD_BYTES + 1]),
        Err(LLUdpError::InvalidMessage)
    );
    assert_eq!(improved_terse_object_update::decode_texture_entry(&binary_data(&vec![0; 1021])), Err(LLUdpError::InvalidMessage));
}
