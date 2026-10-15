use std::num::{NonZeroU8, NonZeroU32};

use base::UUID;
use protocol::{
    lludp::{
        Message,
        Message::{ImprovedTerseObjectUpdate, KillObject, ObjectUpdate, ObjectUpdateCached, ObjectUpdateCompressed},
    },
    messages::{CachedObjectReference, ObjectData, improved_terse_object_update, object_update_compressed},
};
use world::{
    AttachmentPoint, Kind, LinearMotion, LocalAddress, MotionPatch, ObjectObservation, ObjectPatch, OrderEvidence, ParentUpdate, Pose,
    Region, RegionHandle, UpdateSource,
};

/// Object operations decoded from one simulator message, in packet block order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ObjectBatch {
    pub(crate) region: Region,
    pub(crate) region_handle: Option<RegionHandle>,
    pub(crate) blocks: Vec<Option<ObjectOperation>>,
    pub(crate) invalid_blocks: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ObjectOperation {
    Observe(ObjectObservation),
    Patch(ObjectPatch),
    Remove { source: UpdateSource, local_id: NonZeroU32 },
    Cached { source: UpdateSource, local_id: NonZeroU32 },
}

impl ObjectOperation {
    pub(crate) fn source(self) -> UpdateSource {
        match self {
            Self::Observe(observation) => observation.source,
            Self::Patch(patch) => patch.source,
            Self::Remove { source, .. } | Self::Cached { source, .. } => source,
        }
    }

    pub(crate) fn address(self) -> LocalAddress {
        let (source, local_id) = match self {
            Self::Observe(observation) => (observation.source, observation.local_id),
            Self::Patch(patch) => (patch.source, patch.local_id),
            Self::Remove { source, local_id } | Self::Cached { source, local_id } => (source, local_id),
        };
        LocalAddress::new(source.region(), local_id)
    }
}

/// Converts a decoded LLUDP object message into ordered world-domain operations.
///
/// This function does not inspect or mutate World state. Invalid object blocks are
/// represented by `None` at their original packet index.
pub(crate) fn decode_object_message(region: Region, sequence: u32, order: OrderEvidence, message: &Message) -> Option<ObjectBatch> {
    let mut invalid_blocks = 0;
    let blocks = match message {
        ObjectUpdate(update) => map_blocks(region, sequence, order, &update.objects, full_observation, &mut invalid_blocks),
        ObjectUpdateCompressed(update) => map_blocks(region, sequence, order, &update.objects, compressed_observation, &mut invalid_blocks),
        ImprovedTerseObjectUpdate(update) => map_blocks(region, sequence, order, &update.objects, terse_patch, &mut invalid_blocks),
        ObjectUpdateCached(update) => map_blocks(region, sequence, order, &update.objects, cached_patch, &mut invalid_blocks),
        KillObject(update) => map_blocks(
            region,
            sequence,
            order,
            &update.object_data,
            |object, source| Ok(ObjectOperation::Remove { source, local_id: local_id(object.local_id)? }),
            &mut invalid_blocks,
        ),
        _ => return None,
    };

    Some(ObjectBatch { region, region_handle: object_region_handle(message), blocks, invalid_blocks })
}

fn object_region_handle(message: &Message) -> Option<RegionHandle> {
    let raw_handle = match message {
        ObjectUpdate(update) => update.region_data.region_handle,
        ObjectUpdateCompressed(update) => update.region_data.region_handle,
        ImprovedTerseObjectUpdate(update) => update.region_data.region_handle,
        ObjectUpdateCached(update) => update.region_data.region_handle,
        _ => return None,
    };
    Some(RegionHandle::new(raw_handle))
}

fn map_blocks<T>(
    region: Region,
    sequence: u32,
    order: OrderEvidence,
    blocks: &[T],
    mut to_operation: impl FnMut(&T, UpdateSource) -> Result<ObjectOperation, InvalidBlock>,
    invalid_blocks: &mut usize,
) -> Vec<Option<ObjectOperation>> {
    blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            let source = u32::try_from(index).ok().map(|ordinal| UpdateSource::new(region, sequence, order, ordinal));
            let operation = source.and_then(|source| to_operation(block, source).ok());
            if operation.is_none() {
                *invalid_blocks += 1;
            }
            operation
        })
        .collect()
}

#[derive(Clone, Copy)]
struct InvalidBlock;

fn full_observation(block: &ObjectData, source: UpdateSource) -> Result<ObjectOperation, InvalidBlock> {
    let id = non_nil_uuid(block.id)?;
    let local_id = local_id(block.local_id)?;
    Ok(ObjectOperation::Observe(ObjectObservation {
        source,
        id,
        local_id,
        kind: object_kind(block.pcode),
        parent: parent(block.parent_local_id)?,
        pose: Some(Pose { position: Some(block.motion.position), rotation: Some(block.motion.rotation) }),
        linear_motion: Some(LinearMotion { velocity: Some(block.motion.velocity), acceleration: Some(block.motion.acceleration) }),
        angular_velocity: Some(block.motion.angular_velocity),
        scale: Some(block.scale),
        attachment_point: attachment_point(block.state),
    }))
}

fn compressed_observation(block: &object_update_compressed::ObjectData, source: UpdateSource) -> Result<ObjectOperation, InvalidBlock> {
    Ok(ObjectOperation::Observe(ObjectObservation {
        source,
        id: non_nil_uuid(block.id)?,
        local_id: local_id(block.local_id)?,
        kind: object_kind(block.pcode),
        parent: parent(block.parent_local_id)?,
        pose: Some(Pose { position: Some(block.position), rotation: Some(block.rotation) }),
        linear_motion: None,
        angular_velocity: block.angular_velocity,
        scale: Some(block.scale),
        attachment_point: attachment_point(block.state),
    }))
}

fn terse_patch(block: &improved_terse_object_update::ObjectData, source: UpdateSource) -> Result<ObjectOperation, InvalidBlock> {
    let mut patch = ObjectPatch::new(source, local_id(block.local_id)?);
    patch.pose = Some(Pose { position: Some(block.motion.position), rotation: Some(block.motion.rotation) });
    patch.linear_motion = MotionPatch { velocity: Some(block.motion.velocity), acceleration: Some(block.motion.acceleration) };
    patch.angular_velocity = Some(block.motion.angular_velocity);
    patch.attachment_point = attachment_point(block.state);
    Ok(ObjectOperation::Patch(patch))
}

fn cached_patch(block: &CachedObjectReference, source: UpdateSource) -> Result<ObjectOperation, InvalidBlock> {
    Ok(ObjectOperation::Cached { source, local_id: local_id(block.local_id)? })
}

fn local_id(value: u32) -> Result<NonZeroU32, InvalidBlock> {
    NonZeroU32::new(value).ok_or(InvalidBlock)
}

fn non_nil_uuid(value: UUID) -> Result<UUID, InvalidBlock> {
    if value.is_nil() { Err(InvalidBlock) } else { Ok(value) }
}

fn attachment_point(state: u8) -> Option<Option<AttachmentPoint>> {
    let point = state.rotate_left(4) & !0x80;
    Some(NonZeroU8::new(point).map(AttachmentPoint::new))
}

fn parent(raw_local_id: u32) -> Result<ParentUpdate, InvalidBlock> {
    if raw_local_id == 0 { Ok(ParentUpdate::Root) } else { Ok(ParentUpdate::Local(local_id(raw_local_id)?)) }
}

fn object_kind(pcode: u8) -> Kind {
    match pcode {
        47 => Kind::Avatar,
        1..=63 if matches!(pcode & 0x0f, 1..=9) => Kind::Object,
        14 | 15 | 95 | 111 | 143 | 159 | 239 | 255 => Kind::Object,
        _ => Kind::Unknown(pcode),
    }
}

#[cfg(test)]
mod tests {
    use base::UUID;
    use math::{Quat, Vec3};
    use protocol::{
        lludp::{Message, Message::ImprovedTerseObjectUpdate, Message::ObjectUpdate, Message::ObjectUpdateCompressed, Packet},
        messages::StartPingCheck,
    };
    use std::num::{NonZeroU8, NonZeroU32};
    use testing::{lludp::vectors, parse_hex_fixture};
    use world::{
        ApplyOutcome::{Introduced, RegionAdopted},
        AttachmentPoint,
        FieldRejectionReason::InvalidRotation,
        Kind, ObjectObservation, ObjectPatch, OrderEvidence, ParentUpdate, Pose, Region, UpdateSource, World, WorldLimits,
    };

    use super::{ObjectOperation, attachment_point, decode_object_message};

    const REGION: Region = Region::new(11);

    fn packet(fixture: &str) -> Packet {
        let bytes = parse_hex_fixture(fixture).expect("valid LLUDP fixture");
        Packet::decode(&bytes).expect("decode LLUDP fixture")
    }

    fn xyz_packet(fixture: &str, z: f32) -> Packet {
        let fixture_packet = packet(fixture);
        let mut message = fixture_packet.message().clone();
        let marker = Quat::from_xyzw(0.137, 0.293, 0.419, 0.846);
        match &mut message {
            ObjectUpdate(update) => update.objects[0].motion.rotation = marker,
            ObjectUpdateCompressed(update) => update.objects[0].rotation = marker,
            _ => panic!("expected XYZ object update"),
        }
        let mut bytes = Packet::new(fixture_packet.sequence(), message).with_zero_coding(false).encode().expect("encode marker packet");
        let marker_packet = Packet::decode(&bytes).expect("decode marker packet");
        let rotation = match marker_packet.message() {
            ObjectUpdate(update) => update.objects[0].motion.rotation,
            ObjectUpdateCompressed(update) => update.objects[0].rotation,
            _ => panic!("expected XYZ object update"),
        };
        let mut needle = Vec::with_capacity(12);
        for component in [rotation.x, rotation.y, rotation.z] {
            needle.extend_from_slice(&component.to_le_bytes());
        }
        let mut offsets = bytes.windows(needle.len()).enumerate().filter_map(|(offset, window)| (window == needle).then_some(offset));
        let offset = offsets.next().expect("marker rotation is in encoded message");
        assert!(offsets.next().is_none(), "marker rotation is unique in encoded message");
        bytes[offset..offset + 4].copy_from_slice(&z.to_le_bytes());
        bytes[offset + 4..offset + 12].fill(0);
        Packet::decode(&bytes).expect("decode raw XYZ rotation")
    }

    fn terse_patch(rotation: Quat, order: u64) -> ObjectPatch {
        let fixture_packet = packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
        let mut message = fixture_packet.message().clone();
        let ImprovedTerseObjectUpdate(update) = &mut message else { panic!("expected terse fixture") };
        update.objects[0].motion.rotation = rotation;
        let encoded = Packet::new(fixture_packet.sequence(), message).with_zero_coding(false).encode().expect("encode terse rotation");
        let packet = Packet::decode(&encoded).expect("decode terse rotation");
        let batch = decode_object_message(REGION, packet.sequence(), OrderEvidence::Ordered(order), packet.message())
            .expect("terse fixture has an object message");
        let Some(ObjectOperation::Patch(patch)) = batch.blocks.first().copied().flatten() else { panic!("expected terse object patch") };
        patch
    }

    fn assert_xyz_rotation(fixture: &str, z: f32, accepted: bool) {
        let packet = xyz_packet(fixture, z);
        let batch = decode_object_message(REGION, packet.sequence(), OrderEvidence::Ordered(1), packet.message())
            .expect("XYZ fixture has an object message");
        let Some(ObjectOperation::Observe(observation)) = batch.blocks.first().copied().flatten() else {
            panic!("expected full object observation")
        };
        let id = observation.id;
        let mut world = world();
        let applied = world.observe_object(observation).expect("decoded object is accepted");
        let rejected = applied.rejections.iter().any(|rejection| rejection.reason == InvalidRotation);
        assert_eq!(rejected, !accepted);
        let Introduced(entity) = applied.outcome else { panic!("expected object introduction") };
        let rotation = world.object(entity).expect("introduced object exists").rotation();
        if accepted {
            assert!(rotation.is_some());
            assert!((rotation.unwrap().length_squared() - 1.0).abs() <= 8.0 * f32::EPSILON);
        } else {
            assert!(rotation.is_none());
        }
        assert_eq!(world.entity(id), Some(entity));
    }

    fn world() -> World {
        let limits = WorldLimits {
            max_region_registrations: 1,
            max_entities: 8,
            max_entities_per_region: 8,
            max_tombstones_per_region: 8,
            max_tombstones: 8,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(8),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(8),
            max_journal_entries: 8,
            max_journal_changes: 32,
            max_region_name_bytes: 64,
            max_recovery_targets: 8,
        };
        let mut world = World::new(limits);
        assert_eq!(world.adopt_region(REGION).unwrap().outcome, RegionAdopted);
        world
    }

    fn assert_observed_rotation_is_valid(fixture: &str) {
        let packet = packet(fixture);
        let batch = decode_object_message(REGION, packet.sequence(), OrderEvidence::Ordered(1), packet.message())
            .expect("fixture has an object message");
        let Some(ObjectOperation::Observe(observation)) = batch.blocks.first().copied().flatten() else {
            panic!("expected full object observation")
        };
        let id = observation.id;
        let mut world = world();
        let applied = world.observe_object(observation).expect("decoded rotation is valid");
        assert!(!applied.rejections.iter().any(|rejection| rejection.reason == InvalidRotation), "rejections: {:?}", applied.rejections);
        let Introduced(entity) = applied.outcome else { panic!("expected object introduction") };
        let rotation = world.object(entity).expect("introduced object exists").rotation().expect("decoded rotation is stored");
        assert!((rotation.length_squared() - 1.0).abs() <= 8.0 * f32::EPSILON);
        assert_eq!(world.entity(id), Some(entity));
    }

    #[test]
    fn decoded_xyz_rotations_are_accepted_by_world() {
        assert_observed_rotation_is_valid(vectors::OBJECT_UPDATE);
        assert_observed_rotation_is_valid(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    }

    #[test]
    fn xyz_decoder_tolerance_matches_world() {
        let inside = f32::from_bits(1.0_f32.to_bits() + 260);
        let outside = f32::from_bits(1.0_f32.to_bits() + 261);
        for fixture in [vectors::OBJECT_UPDATE, vectors::OBJECT_UPDATE_COMPRESSED_TYPED] {
            assert_xyz_rotation(fixture, inside, true);
            assert_xyz_rotation(fixture, outside, false);
        }
    }

    #[test]
    fn terse_rotation_roundoff_is_accepted() {
        let patch = terse_patch(Quat::from_xyzw(-0.54654974, 0.48178887, 0.47275522, -0.4956464), 2);
        let decoded_rotation = patch.pose.expect("terse patch includes pose").rotation.expect("terse patch includes rotation");
        let norm_error = decoded_rotation.length_squared() - 1.0;
        assert!((0.00005..0.00006).contains(&norm_error), "unexpected quantized norm error: {norm_error}");
        let mut world = world();
        let base = ObjectObservation {
            source: UpdateSource::new(REGION, 1, OrderEvidence::Ordered(1), 0),
            id: UUID::from_u128(0xdead),
            local_id: patch.local_id,
            kind: Kind::Object,
            parent: ParentUpdate::Root,
            pose: Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) }),
            linear_motion: None,
            angular_velocity: None,
            scale: None,
            attachment_point: None,
        };
        let introduced = world.observe_object(base).expect("valid base object");
        let Introduced(entity) = introduced.outcome else { panic!("expected base object introduction") };

        let applied = world.patch_object(patch).expect("decoded terse rotation is valid");
        assert!(!applied.rejections.iter().any(|rejection| rejection.reason == InvalidRotation));
        let rotation = world.object(entity).expect("object remains present").rotation().expect("terse rotation is stored");
        assert!((rotation.length_squared() - 1.0).abs() <= 8.0 * f32::EPSILON);
    }

    #[test]
    fn invalid_terse_rotation_is_rejected_by_world() {
        let patch = terse_patch(Quat::from_xyzw(1.0, 1.0, 1.0, 1.0), 2);
        let decoded_rotation = patch.pose.expect("terse patch includes pose").rotation.expect("terse patch includes rotation");
        assert!(decoded_rotation.length_squared() > 3.9);
        let mut world = world();
        let base = ObjectObservation {
            source: UpdateSource::new(REGION, 1, OrderEvidence::Ordered(1), 0),
            id: UUID::from_u128(0xdead),
            local_id: patch.local_id,
            kind: Kind::Object,
            parent: ParentUpdate::Root,
            pose: Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) }),
            linear_motion: None,
            angular_velocity: None,
            scale: None,
            attachment_point: None,
        };
        let Introduced(entity) = world.observe_object(base).expect("valid base object").outcome else {
            panic!("expected base object introduction")
        };

        let applied = world.patch_object(patch).expect("invalid rotation is rejected as a field group");

        assert!(applied.rejections.iter().any(|rejection| rejection.reason == InvalidRotation));
        assert_eq!(world.object(entity).expect("base object remains").rotation(), Some(Quat::IDENTITY));
    }

    #[test]
    fn fixture_mapping_preserves_packet_order() {
        let packet = packet(vectors::OBJECT_UPDATE);
        let batch = decode_object_message(REGION, packet.sequence(), OrderEvidence::Ordered(17), packet.message()).expect("object batch");

        assert_eq!(batch.invalid_blocks, 0);
        assert_eq!(batch.blocks.len(), 2);
        let Some(ObjectOperation::Observe(observation)) = batch.blocks[0] else { panic!("expected valid observation") };
        assert_eq!(observation.source.region(), REGION);
        assert_eq!(observation.source.sequence(), packet.sequence());
        assert_eq!(observation.source.order(), OrderEvidence::Ordered(17));
        assert_eq!(observation.kind, Kind::Object);
    }

    #[test]
    fn compressed_update_keeps_parent_and_angular() {
        let packet = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
        let batch = decode_object_message(REGION, packet.sequence(), OrderEvidence::Ordered(18), packet.message())
            .expect("compressed object batch");

        let Some(ObjectOperation::Observe(observation)) = batch.blocks[0] else { panic!("expected compressed observation") };
        assert_eq!(observation.parent, ParentUpdate::Local(NonZeroU32::new(0x0506_0708).expect("nonzero parent ID")));
        assert_eq!(observation.angular_velocity, Some(Vec3::new(0.5, 0.0, -0.5)));
        assert_eq!(observation.linear_motion, None);
    }

    #[test]
    fn invalid_blocks_keep_their_packet_position() {
        let mut message = packet(vectors::OBJECT_UPDATE).message().clone();
        let ObjectUpdate(update) = &mut message else { panic!("expected full object fixture") };
        update.objects.truncate(2);
        update.objects[0].id = UUID::nil();
        let batch = decode_object_message(REGION, 1, OrderEvidence::Ambiguous, &message).expect("object batch");

        assert_eq!(batch.invalid_blocks, 1);
        assert!(batch.blocks[0].is_none());
        assert!(matches!(batch.blocks[1], Some(ObjectOperation::Observe(observation)) if observation.parent == ParentUpdate::Root));
    }

    #[test]
    fn non_object_messages_are_ignored() {
        let message = Message::StartPingCheck(StartPingCheck::default());
        assert!(decode_object_message(REGION, 1, OrderEvidence::Ordered(1), &message).is_none());
    }

    #[test]
    fn attachment_state_decodes_wire_nibbles() {
        assert_eq!(attachment_point(0), Some(None));
        assert_eq!(attachment_point(0x10), Some(Some(AttachmentPoint::new(NonZeroU8::new(1).unwrap()))));
        assert_eq!(attachment_point(0x18), Some(Some(AttachmentPoint::new(NonZeroU8::new(1).unwrap()))));
        assert_eq!(attachment_point(0x80), Some(Some(AttachmentPoint::new(NonZeroU8::new(8).unwrap()))));
        assert_eq!(attachment_point(0x88), Some(Some(AttachmentPoint::new(NonZeroU8::new(8).unwrap()))));
    }
}
