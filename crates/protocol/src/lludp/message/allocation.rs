use std::mem::size_of;

use super::{AllocationStrategy, TypedMessage};
use crate::{
    lludp::{error::LLUdpError, schema_message::SchemaMessage, types::MessageId},
    messages::{ExtraParameter, object},
    schema,
};

const _: () = {
    assert!(size_of::<crate::messages::region::region_handshake::RegionInfo4>() <= 512);
    assert!(size_of::<crate::messages::circuit::packet_ack::Packets>() <= 512);
    assert!(size_of::<crate::messages::agent::logout_reply::InventoryData>() <= 512);
    assert!(size_of::<crate::messages::map::map_block_reply::Data>() <= 512);
    assert!(size_of::<ExtraParameter>() <= 32);
    assert!(size_of::<object::object_update::ObjectData>() <= 512);
    assert!(size_of::<object::object_update_compressed::ObjectData>() <= 512);
    assert!(size_of::<object::improved_terse_object_update::ObjectData>() <= 512);
    assert!(size_of::<object::improved_terse_object_update::ObjectBlock>() <= 512);
    assert!(size_of::<object::object_update_cached::CachedObjectReference>() <= 512);
    assert!(size_of::<object::kill_object::ObjectData>() <= 512);
    assert!(size_of::<object::request_multiple_objects::ObjectRequest>() <= 512);
    assert!(size_of::<object::object_extra_params::ExtraParamUpdate>() <= 512);
    assert!(
        size_of::<crate::messages::circuit::packet_ack::Packets>()
            <= size_of::<crate::lludp::BlockInstance>() + size_of::<crate::lludp::FieldValue>()
    );
    assert!(
        size_of::<crate::messages::agent::logout_reply::InventoryData>()
            <= size_of::<crate::lludp::BlockInstance>() + size_of::<crate::lludp::FieldValue>()
    );
    assert!(
        size_of::<crate::messages::map::map_block_reply::Data>()
            <= size_of::<crate::lludp::BlockInstance>() + 9 * size_of::<crate::lludp::FieldValue>()
    );
    assert!(
        size_of::<object::object_update_cached::CachedObjectReference>()
            <= size_of::<crate::lludp::BlockInstance>() + 3 * size_of::<crate::lludp::FieldValue>()
    );
    assert!(
        size_of::<object::kill_object::ObjectData>() <= size_of::<crate::lludp::BlockInstance>() + size_of::<crate::lludp::FieldValue>()
    );
    assert!(
        size_of::<object::request_multiple_objects::ObjectRequest>()
            <= size_of::<crate::lludp::BlockInstance>() + 2 * size_of::<crate::lludp::FieldValue>()
    );
    assert!(
        size_of::<crate::messages::agent::avatar_appearance::VisualParam>()
            <= size_of::<crate::lludp::BlockInstance>() + size_of::<crate::lludp::FieldValue>()
    );
    assert!(
        size_of::<crate::messages::agent::avatar_appearance::AppearanceData>()
            <= size_of::<crate::lludp::BlockInstance>() + 3 * size_of::<crate::lludp::FieldValue>()
    );
    assert!(
        size_of::<crate::messages::agent::avatar_appearance::AppearanceHover>()
            <= size_of::<crate::lludp::BlockInstance>() + size_of::<crate::lludp::FieldValue>()
    );
    assert!(
        size_of::<crate::messages::agent::avatar_appearance::AttachmentBlock>()
            <= size_of::<crate::lludp::BlockInstance>() + 2 * size_of::<crate::lludp::FieldValue>()
    );
    assert!(
        size_of::<crate::messages::generic_message::Parameter>()
            <= size_of::<crate::lludp::BlockInstance>() + size_of::<crate::lludp::FieldValue>()
    );
    assert!(size_of::<crate::messages::parcel::parcel_properties::ParcelData>() <= 60 * size_of::<crate::lludp::FieldValue>());
    assert!(size_of::<crate::messages::agent::teleport_failed::AlertInfo>() <= 2 * size_of::<crate::lludp::FieldValue>());
    assert!(size_of::<crate::messages::region::region_info::RegionInfo3>() <= size_of::<crate::lludp::FieldValue>());
    assert!(size_of::<crate::messages::region::region_info::RegionInfo5>() <= 7 * size_of::<crate::lludp::FieldValue>());
    assert!(size_of::<crate::messages::region::region_info::CombatSettings>() <= 6 * size_of::<crate::lludp::FieldValue>());
};

pub(crate) fn typed_allocation_bound<T: TypedMessage>(payload: &[u8]) -> Result<(AllocationStrategy, usize), LLUdpError> {
    let bound = match T::ALLOCATION {
        AllocationStrategy::NoAlloc | AllocationStrategy::Opaque => payload.len(),
        AllocationStrategy::SchemaDerived => {
            let metadata = schema::find_by_id(T::ID).ok_or(LLUdpError::InvalidMessageId)?;
            SchemaMessage::allocation_bound(metadata, payload)?
        }
        AllocationStrategy::Specialized => T::preflight_allocation_bound(payload)?,
    };
    Ok((T::ALLOCATION, bound))
}

pub(crate) fn fallback_allocation_bound(id: MessageId, payload: &[u8]) -> Result<(AllocationStrategy, usize), LLUdpError> {
    if let Some(metadata) = schema::find_by_id(id) {
        Ok((AllocationStrategy::SchemaDerived, SchemaMessage::allocation_bound(metadata, payload)?))
    } else {
        Ok((AllocationStrategy::Opaque, payload.len()))
    }
}

pub(crate) fn capacity_bytes(capacity: usize, element_size: usize) -> usize {
    capacity.saturating_mul(element_size)
}

pub(crate) fn with_payload(payload: &[u8], additional: usize) -> Result<usize, LLUdpError> {
    payload.len().checked_add(additional).ok_or(LLUdpError::AllocationBoundOverflow)
}

pub(crate) fn add_capacity(total: &mut usize, capacity: usize, element_size: usize) {
    *total = total.saturating_add(capacity_bytes(capacity, element_size));
}

pub(crate) fn add_string_capacity(total: &mut usize, value: &String) {
    add_capacity(total, value.capacity(), size_of::<u8>());
}

pub(crate) fn add_extra_parameter_capacity(total: &mut usize, parameters: &Vec<ExtraParameter>) {
    add_capacity(total, parameters.capacity(), size_of::<ExtraParameter>());
    for parameter in parameters {
        add_capacity(total, parameter.data.capacity(), size_of::<u8>());
    }
}

pub(crate) fn add_object_update_capacity(total: &mut usize, object: &object::object_update::ObjectData) {
    add_capacity(total, object.texture_entry.capacity(), size_of::<u8>());
    add_capacity(total, object.texture_anim.capacity(), size_of::<u8>());
    add_capacity(total, object.name_value.capacity(), size_of::<u8>());
    add_capacity(total, object.data.capacity(), size_of::<u8>());
    add_string_capacity(total, &object.text.text);
    add_string_capacity(total, &object.media_url);
    add_capacity(total, object.particle_system.capacity(), size_of::<u8>());
    add_extra_parameter_capacity(total, &object.extra_parameters);
}

pub(crate) fn add_object_update_compressed_capacity(total: &mut usize, object: &object::object_update_compressed::ObjectData) {
    if let Some(object::object_update_compressed::ObjectSpecificData::ScratchPad(bytes)) = &object.data {
        add_capacity(total, bytes.capacity(), size_of::<u8>());
    }
    if let Some(text) = &object.text {
        add_string_capacity(total, &text.text);
    }
    if let Some(media_url) = &object.media_url {
        add_string_capacity(total, media_url);
    }
    if let Some(bytes) = &object.legacy_particle_system {
        add_capacity(total, bytes.capacity(), size_of::<u8>());
    }
    add_extra_parameter_capacity(total, &object.extra_parameters);
    if let Some(name_value) = &object.name_value {
        add_string_capacity(total, name_value);
    }
    if let Some(bytes) = &object.texture_entry {
        add_capacity(total, bytes.capacity(), size_of::<u8>());
    }
    if let Some(bytes) = &object.texture_animation {
        add_capacity(total, bytes.capacity(), size_of::<u8>());
    }
    if let Some(bytes) = &object.particle_system {
        add_capacity(total, bytes.capacity(), size_of::<u8>());
    }
}
