use super::*;
use protocol::messages::CachedObjectReference;

use crate::session::inbound::decode_cached_object;

#[test]
fn fixture_mapping_preserves_packet_order() {
    let packet = packet(vectors::OBJECT_UPDATE);
    let batch = decode_object_message(REGION, packet.sequence(), OrderEvidence::Ordered(17), packet.message()).expect("object batch");

    assert_eq!(batch.invalid_blocks, 0);
    assert_eq!(batch.blocks.len(), 2);
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected valid observation") };
    assert_eq!(observation.source.region(), REGION);
    assert_eq!(observation.source.sequence(), packet.sequence());
    assert_eq!(observation.source.order(), OrderEvidence::Ordered(17));
    assert_eq!(observation.kind, Kind::Object);
    assert_eq!(observation.shape, FieldUpdate::Set(Shape::Prim(full_prim_shape())));
    assert_eq!(observation.appearance, FieldUpdate::Set(Appearance::Surface));
    let Some(ObjectOperation::Observe(avatar)) = batch.blocks[1].as_ref() else { panic!("expected avatar observation") };
    assert_eq!(avatar.kind, Kind::Avatar);
    assert_eq!(avatar.shape, FieldUpdate::Set(Shape::Avatar));
    assert_eq!(avatar.appearance, FieldUpdate::Set(Appearance::Avatar));
}

#[test]
fn motion_updates_preserve_physics_and_time_dilation_fields() {
    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    let Message::ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.objects.truncate(1);
    update.region_data.time_dilation = 0x4000;
    update.objects[0].update_flags |= crate::session::inbound::object::FLAGS_USE_PHYSICS;
    let batch = decode_object_message(REGION, 1, OrderEvidence::Ordered(1), &full).expect("full object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected full observation") };
    assert_eq!(observation.is_physical, Some(true));
    assert_eq!(observation.time_dilation, Some(f32::from(0x4000_u16) / f32::from(u16::MAX)));
    let Message::ObjectUpdate(update) = &full else { panic!("expected cached full object fixture") };
    let reference = CachedObjectReference {
        local_id: update.objects[0].local_id,
        crc: update.objects[0].crc,
        update_flags: update.objects[0].update_flags,
    };
    let mut invalid_fields = 0;
    let cached = decode_cached_object(REGION, 1, OrderEvidence::Ordered(1), 0, reference, &full, &mut invalid_fields);
    let Some(ObjectOperation::Observe(cached)) = cached else { panic!("expected cached observation") };
    assert_eq!(cached.is_physical, Some(true));
    assert_eq!(cached.time_dilation, observation.time_dilation);

    let mut compressed = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    let Message::ObjectUpdateCompressed(update) = &mut compressed else { panic!("expected compressed object fixture") };
    update.objects.truncate(1);
    update.region_data.time_dilation = 0x8000;
    update.objects[0].update_flags |= crate::session::inbound::object::FLAGS_USE_PHYSICS;
    let batch = decode_object_message(REGION, 2, OrderEvidence::Ordered(2), &compressed).expect("compressed object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected compressed observation") };
    assert_eq!(observation.is_physical, Some(true));
    assert_eq!(observation.time_dilation, Some(f32::from(0x8000_u16) / f32::from(u16::MAX)));
    let Message::ObjectUpdateCompressed(update) = &compressed else { panic!("expected cached compressed object fixture") };
    let reference = CachedObjectReference {
        local_id: update.objects[0].local_id,
        crc: update.objects[0].crc,
        update_flags: update.objects[0].update_flags,
    };
    let mut invalid_fields = 0;
    let cached = decode_cached_object(REGION, 2, OrderEvidence::Ordered(2), 0, reference, &compressed, &mut invalid_fields);
    let Some(ObjectOperation::Observe(cached)) = cached else { panic!("expected cached observation") };
    assert_eq!(cached.is_physical, Some(true));
    assert_eq!(cached.time_dilation, observation.time_dilation);

    let mut terse = packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE).message().clone();
    let Message::ImprovedTerseObjectUpdate(update) = &mut terse else { panic!("expected terse fixture") };
    update.region_data.time_dilation = u16::MAX;
    let batch = decode_object_message(REGION, 3, OrderEvidence::Ordered(3), &terse).expect("terse object batch");
    let Some(ObjectOperation::Patch(patch)) = batch.blocks[0].as_ref() else { panic!("expected terse patch") };
    assert_eq!(patch.is_physical, None);
    assert_eq!(patch.time_dilation, Some(1.0));
}

#[test]
fn malformed_terse_block_does_not_discard_later_object() {
    let fixture = packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let Message::ImprovedTerseObjectUpdate(update) = fixture.message() else { panic!("expected terse fixture") };
    let mut blocks = update.objects.clone();
    blocks.insert(0, improved_terse_object_update::ObjectBlock::Malformed { packed_data: vec![1], texture_entry: Vec::new() });
    let message = Message::ImprovedTerseObjectUpdate(improved_terse_object_update::ImprovedTerseObjectUpdate {
        region_data: update.region_data,
        objects: blocks,
    });

    let batch = decode_object_message(REGION, 52, OrderEvidence::Ordered(52), &message).expect("terse object batch");

    assert_eq!(batch.invalid_blocks, 1);
    assert_eq!(batch.blocks.len(), 2);
    assert!(batch.blocks[0].is_none());
    let Some(ObjectOperation::Patch(patch)) = batch.blocks[1].as_ref() else { panic!("valid sibling remains mapped") };
    assert_eq!(patch.source.block_ordinal(), 1);
}

#[test]
fn invalid_blocks_keep_their_packet_position() {
    let mut message = packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut message else { panic!("expected full object fixture") };
    update.objects.truncate(2);
    update.objects[0].id = Uuid::nil();
    let batch = decode_object_message(REGION, 1, OrderEvidence::Ambiguous, &message).expect("object batch");

    assert_eq!(batch.invalid_blocks, 1);
    assert!(batch.blocks[0].is_none());
    assert!(matches!(batch.blocks[1].as_ref(), Some(ObjectOperation::Observe(observation)) if observation.parent == ParentUpdate::Root));
}

#[test]
fn non_object_messages_are_ignored() {
    let message = Message::StartPingCheck(StartPingCheck::default());
    assert!(decode_object_message(REGION, 1, OrderEvidence::Ordered(1), &message).is_none());
}

#[test]
fn empty_snapshot_is_an_explicit_clear() {
    let message = message(0x4175, br"{'id':i7,'te':[],'od':[]}");
    let batch = decode_object_message(Region::new(11), 5, OrderEvidence::Ordered(5), &message).expect("override batch");

    let Some(ObjectOperation::Patch(patch)) = &batch.blocks[0] else { panic!("expected object patch") };
    assert!(patch.pbr_material_overrides.as_ref().unwrap().is_empty());
}
