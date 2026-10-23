pub(super) use math::{Quat, Vec3, Vec4};
pub(super) use protocol::{
    LLUdpError,
    lludp::{Message, Packet},
    messages::{
        CacheMissType, CachedObjectReference, ExtraParameter, KillObject, ObjectData, ObjectMotion, ObjectRequest, ObjectSound,
        ObjectSpecificData, ObjectText, ObjectUpdate, ObjectUpdateCached, RegionData, RequestAgentData, RequestMultipleObjects,
        object::improved_terse_object_update, object::kill_object,
    },
    types::ObjectShape,
};
pub(super) use test_support::{lludp::vectors, parse_hex_fixture};
pub(super) use uuid::Uuid;

pub(super) fn fixture(hex: &str) -> Vec<u8> {
    parse_hex_fixture(hex).expect("valid protocol fixture")
}

pub(super) fn unique_offset(bytes: &[u8], needle: &[u8]) -> usize {
    let mut offsets = bytes.windows(needle.len()).enumerate().filter_map(|(offset, candidate)| (candidate == needle).then_some(offset));
    let offset = offsets.next().expect("fixture field is present");
    assert!(offsets.next().is_none(), "fixture field {needle:02x?} is unique");
    offset
}

pub(super) fn terse_object(block: &improved_terse_object_update::ObjectBlock) -> &improved_terse_object_update::ObjectData {
    block.as_decoded().expect("expected a decoded improved-terse object block")
}

pub(super) fn terse_object_mut(block: &mut improved_terse_object_update::ObjectBlock) -> &mut improved_terse_object_update::ObjectData {
    block.as_decoded_mut().expect("expected a decoded improved-terse object block")
}

pub(super) fn write_nan(bytes: &mut [u8], offset: usize) {
    bytes[offset..offset + 4].copy_from_slice(&f32::NAN.to_le_bytes());
}

pub(super) fn add_motion_extension(mut bytes: Vec<u8>, local_id: u32, extension: &[u8]) -> Vec<u8> {
    let local_id_offset = unique_offset(&bytes, &local_id.to_le_bytes());
    let motion_length_offset = local_id_offset + 40;
    let motion_length = usize::from(bytes[motion_length_offset]);
    let extended_length = motion_length + extension.len();
    bytes[motion_length_offset] = u8::try_from(extended_length).expect("extended motion block fits Variable 1");
    let motion_end = motion_length_offset + 1 + motion_length;
    bytes.splice(motion_end..motion_end, extension.iter().copied());
    bytes
}

pub(super) fn write_midpoint_components(bytes: &mut [u8], offset: usize, count: usize) {
    let (components, _) = bytes[offset..offset + count * 2].as_chunks_mut::<2>();
    for (index, component) in components.iter_mut().enumerate() {
        let midpoint = 32_767 + (index % 2) as u16;
        component.copy_from_slice(&midpoint.to_le_bytes());
    }
}

pub(super) fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("valid fixture UUID")
}

pub(super) fn decode(hex: &str) -> (Vec<u8>, Packet) {
    let bytes = fixture(hex);
    let packet = Packet::decode(&bytes).expect("decode independent fixture");
    (bytes, packet)
}

pub(super) fn assert_reencodes(packet: &Packet, bytes: &[u8]) {
    assert_eq!(packet.encode().expect("re-encode fixture"), bytes);
}

pub(super) fn empty_object_message(message: &Message) -> Message {
    match message {
        Message::ObjectUpdate(update) => {
            let mut update = update.clone();
            update.objects.clear();
            Message::ObjectUpdate(update)
        }
        Message::ObjectUpdateCompressed(update) => {
            let mut update = update.clone();
            update.objects.clear();
            Message::ObjectUpdateCompressed(update)
        }
        Message::ImprovedTerseObjectUpdate(update) => {
            let mut update = update.clone();
            update.objects.clear();
            Message::ImprovedTerseObjectUpdate(update)
        }
        Message::ObjectUpdateCached(update) => {
            let mut update = update.clone();
            update.objects.clear();
            Message::ObjectUpdateCached(update)
        }
        Message::KillObject(update) => {
            let mut update = update.clone();
            update.object_data.clear();
            Message::KillObject(update)
        }
        Message::ObjectExtraParams(update) => {
            let mut update = update.clone();
            update.object_data.clear();
            Message::ObjectExtraParams(update)
        }
        Message::RequestMultipleObjects(update) => {
            let mut update = update.clone();
            update.objects.clear();
            Message::RequestMultipleObjects(update)
        }
        other => panic!("not an object message: {other:?}"),
    }
}
