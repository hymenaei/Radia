use protocol::{
    lludp::{Message, Packet},
    messages::object::RegionData,
};

use super::{EncodedEntry, MAX_OBJECT_CACHE_ENTRY_BYTES, MIN_OBJECT_CACHE_ENTRY_BYTES, StoredEntry};
pub(super) fn stored_entry(entry: &EncodedEntry) -> StoredEntry {
    StoredEntry {
        local_id: i64::from(entry.local_id),
        crc: i64::from(entry.crc),
        update_kind: entry.update_kind,
        update_flags: i64::from(entry.update_flags),
        payload_bytes: i64::try_from(entry.payload.len()).unwrap_or(i64::MAX),
        payload: Some(entry.payload.clone()),
    }
}

pub(super) fn decode_entry(entry: &StoredEntry, local_id: u32) -> Option<Message> {
    let payload_bytes = usize::try_from(entry.payload_bytes).ok()?;
    if entry.local_id != i64::from(local_id)
        || u32::try_from(entry.update_flags).is_err()
        || !(MIN_OBJECT_CACHE_ENTRY_BYTES..=MAX_OBJECT_CACHE_ENTRY_BYTES).contains(&payload_bytes)
    {
        return None;
    }
    let payload = entry.payload.as_ref()?;
    if payload.len() != payload_bytes {
        return None;
    }
    let packet = Packet::decode(payload).ok()?;
    if !packet.extra_header().is_empty() || !packet.trailing_message_bytes().is_empty() || !packet.appended_acks().is_empty() {
        return None;
    }
    let message = packet.into_message();
    let (kind, object_local_id, crc, update_flags) = match &message {
        Message::ObjectUpdate(update) if update.objects.len() == 1 => {
            let object = &update.objects[0];
            (0, object.local_id, object.crc, object.update_flags)
        }
        Message::ObjectUpdateCompressed(update) if update.objects.len() == 1 => {
            let object = &update.objects[0];
            (1, object.local_id, object.crc, object.update_flags)
        }
        _ => return None,
    };
    (entry.update_kind == kind
        && object_local_id == local_id
        && i64::from(crc) == entry.crc
        && i64::from(update_flags) == entry.update_flags)
        .then_some(message)
}

pub(super) fn set_region_data(message: &mut Message, region_data: RegionData) {
    match message {
        Message::ObjectUpdate(update) => update.region_data = region_data,
        Message::ObjectUpdateCompressed(update) => update.region_data = region_data,
        _ => {}
    }
}
