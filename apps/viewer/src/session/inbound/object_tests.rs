use base::UUID;
use math::{Quat, Vec3};
use protocol::{
    lludp::{Message, Message::ImprovedTerseObjectUpdate, Message::ObjectUpdate, Message::ObjectUpdateCompressed, Packet},
    messages::{
        ExtraParamUpdate, ExtraParameter, GenericStreamingMessage, ObjectExtraParams, StartPingCheck, improved_terse_object_update,
    },
};
use std::num::{NonZeroU8, NonZeroU32};
use testing::{lludp::vectors, parse_hex_fixture};
use world::{
    Appearance,
    ApplyOutcome::{Introduced, Updated},
    AssetId, AttachmentPoint,
    FieldRejectionReason::InvalidRotation,
    FieldUpdate, Kind, ObjectObservation, ObjectPatch, OrderEvidence, ParentUpdate, Pose, PrimShape, Region, RenderMaterials, SculptFlags,
    SculptType, Shape, UpdateSource, World, WorldLimits,
};

use super::{ObjectOperation, attachment_point, decode_object_message, object_media_requests};

const REGION: Region = Region::new(11);

fn full_prim_shape() -> PrimShape {
    PrimShape {
        path_curve: 16,
        profile_curve: 1,
        path_begin: 0x1000,
        path_end: 0xf000,
        path_scale_x: 128,
        path_scale_y: 129,
        path_shear_x: 1,
        path_shear_y: -1,
        path_twist: 2,
        path_twist_begin: -2,
        path_radius_offset: 3,
        path_taper_x: -3,
        path_taper_y: 4,
        path_revolutions: 252,
        path_skew: 18,
        profile_begin: 0x10fb,
        profile_end: 0x3020,
        profile_hollow: 0x5040,
    }
}

fn compressed_prim_shape() -> PrimShape {
    PrimShape {
        path_curve: 1,
        profile_curve: 16,
        path_begin: 258,
        path_end: 772,
        path_scale_x: 5,
        path_scale_y: 6,
        path_shear_x: 7,
        path_shear_y: 8,
        path_twist: -9,
        path_twist_begin: -10,
        path_radius_offset: -11,
        path_taper_x: -12,
        path_taper_y: -13,
        path_revolutions: 14,
        path_skew: -15,
        profile_begin: 4370,
        profile_end: 4884,
        profile_hollow: 5398,
    }
}

fn asset_id(value: u128) -> AssetId {
    AssetId::new(UUID::from_u128(value))
}

fn packet(fixture: &str) -> Packet {
    let bytes = parse_hex_fixture(fixture).expect("valid LLUDP fixture");
    Packet::decode(&bytes).expect("decode LLUDP fixture")
}

fn valid_texture_entry() -> Vec<u8> {
    let mut bytes = vec![0; 16];
    bytes.push(0);
    bytes.extend_from_slice(&[0; 4]);
    bytes.push(0);
    bytes.extend_from_slice(&1.0_f32.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&1.0_f32.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&0_i16.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&0_i16.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&0_i16.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    bytes.extend_from_slice(&[0; 16]);
    bytes
}

fn packed_texture_entry() -> Vec<u8> {
    let payload = valid_texture_entry();
    let mut field = Vec::with_capacity(payload.len() + 4);
    field.extend_from_slice(&i32::try_from(payload.len()).expect("test payload fits").to_le_bytes());
    field.extend_from_slice(&payload);
    field
}

fn render_material_parameter(bindings: &[(u8, UUID)]) -> ExtraParameter {
    let mut data = vec![u8::try_from(bindings.len()).expect("test binding count fits")];
    for (face_index, material_id) in bindings {
        data.push(*face_index);
        data.extend_from_slice(material_id.as_bytes());
    }
    ExtraParameter { parameter_type: 0x0080, data }
}

fn media_observation(region: Region, id: UUID, initial_material: u128) -> ObjectObservation {
    ObjectObservation {
        pose: Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) }),
        scale: Some(Vec3::ONE),
        shape: FieldUpdate::Set(Shape::Prim(Default::default())),
        appearance: FieldUpdate::Set(Appearance::Surface),
        render_materials: Some(RenderMaterials::new(vec![(0, asset_id(initial_material))]).expect("valid initial material")),
        ..ObjectObservation::new(
            UpdateSource::new(region, 1, OrderEvidence::Ordered(1), 0),
            id,
            NonZeroU32::new(0x0102_0304).expect("nonzero local ID"),
            Kind::Object,
            ParentUpdate::Root,
        )
    }
}

fn extra_param_message(local_id: u32, parameter_type: u16, in_use: bool, data: Vec<u8>) -> Message {
    Message::ObjectExtraParams(ObjectExtraParams {
        agent_id: UUID::nil(),
        session_id: UUID::nil(),
        object_data: vec![ExtraParamUpdate { local_id, parameter_type, in_use, data }],
    })
}

fn apply_extra_param_message(world: &mut World, region: Region, message: &Message, order: u64) -> super::ObjectBatch {
    let batch = decode_object_message(region, order as u32, OrderEvidence::Ordered(order), message).expect("extra-parameter batch");
    for operation in batch.blocks.iter().flatten() {
        if let ObjectOperation::RenderMaterialsPatch(patch) = operation {
            let _ = world.patch_object(patch.clone()).expect("extra-parameter patch applies to the addressed object");
        }
    }
    batch
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
    update.objects[0].as_decoded_mut().expect("fixture block is structurally valid").motion.rotation = rotation;
    let encoded = Packet::new(fixture_packet.sequence(), message).with_zero_coding(false).encode().expect("encode terse rotation");
    let packet = Packet::decode(&encoded).expect("decode terse rotation");
    let batch = decode_object_message(REGION, packet.sequence(), OrderEvidence::Ordered(order), packet.message())
        .expect("terse fixture has an object message");
    let Some(ObjectOperation::Patch(patch)) = batch.blocks.first().and_then(Option::as_ref) else { panic!("expected terse object patch") };
    patch.clone()
}

fn assert_xyz_rotation(fixture: &str, z: f32, accepted: bool) {
    let packet = xyz_packet(fixture, z);
    let batch = decode_object_message(REGION, packet.sequence(), OrderEvidence::Ordered(1), packet.message())
        .expect("XYZ fixture has an object message");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks.first().and_then(Option::as_ref) else {
        panic!("expected full object observation")
    };
    let id = observation.id;
    let mut world = world();
    let applied = world.observe_object(observation.clone()).expect("decoded object is accepted");
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
    world_with_regions(1)
}

fn world_with_regions(max_region_registrations: usize) -> World {
    let limits = WorldLimits {
        max_region_registrations,
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
    assert!(matches!(world.adopt_region(REGION).unwrap().outcome, world::RegionOutcome::Adopted { .. }));
    world
}

fn assert_observed_rotation_is_valid(fixture: &str) {
    let packet = packet(fixture);
    let batch = decode_object_message(REGION, packet.sequence(), OrderEvidence::Ordered(1), packet.message())
        .expect("fixture has an object message");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks.first().and_then(Option::as_ref) else {
        panic!("expected full object observation")
    };
    let id = observation.id;
    let mut world = world();
    let applied = world.observe_object(observation.clone()).expect("decoded rotation is valid");
    assert!(!applied.rejections.iter().any(|rejection| rejection.reason == InvalidRotation), "rejections: {:?}", applied.rejections);
    let Introduced(entity) = applied.outcome else { panic!("expected object introduction") };
    let rotation = world.object(entity).expect("introduced object exists").rotation().expect("decoded rotation is stored");
    assert!((rotation.length_squared() - 1.0).abs() <= 8.0 * f32::EPSILON);
    assert_eq!(world.entity(id), Some(entity));
}

#[test]
fn decoded_xyz_rotations_pass_world_checks() {
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
        pose: Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) }),
        ..ObjectObservation::new(
            UpdateSource::new(REGION, 1, OrderEvidence::Ordered(1), 0),
            UUID::from_u128(0xdead),
            patch.local_id,
            Kind::Object,
            ParentUpdate::Root,
        )
    };
    let introduced = world.observe_object(base).expect("valid base object");
    let Introduced(entity) = introduced.outcome else { panic!("expected base object introduction") };

    let applied = world.patch_object(patch).expect("decoded terse rotation is valid");
    assert!(!applied.rejections.iter().any(|rejection| rejection.reason == InvalidRotation));
    let rotation = world.object(entity).expect("object remains present").rotation().expect("terse rotation is stored");
    assert!((rotation.length_squared() - 1.0).abs() <= 8.0 * f32::EPSILON);
}

#[test]
fn bad_terse_rotation_is_rejected() {
    let patch = terse_patch(Quat::from_xyzw(1.0, 1.0, 1.0, 1.0), 2);
    let decoded_rotation = patch.pose.expect("terse patch includes pose").rotation.expect("terse patch includes rotation");
    assert!(decoded_rotation.length_squared() > 3.9);
    let mut world = world();
    let base = ObjectObservation {
        pose: Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) }),
        ..ObjectObservation::new(
            UpdateSource::new(REGION, 1, OrderEvidence::Ordered(1), 0),
            UUID::from_u128(0xdead),
            patch.local_id,
            Kind::Object,
            ParentUpdate::Root,
        )
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
fn texture_entries_apply_to_volumes_only() {
    let texture_entry = valid_texture_entry();

    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.objects[0].texture_entry = texture_entry.clone();
    update.objects[1].texture_entry = texture_entry.clone();
    let full_batch = decode_object_message(REGION, 30, OrderEvidence::Ordered(30), &full).expect("full object batch");
    let Some(ObjectOperation::Observe(object)) = full_batch.blocks[0].as_ref() else { panic!("expected volume observation") };
    assert_eq!(object.surface_appearance.as_ref().unwrap().default_face().color, [255; 4]);
    let Some(ObjectOperation::Observe(avatar)) = full_batch.blocks[1].as_ref() else { panic!("expected avatar observation") };
    assert_eq!(avatar.kind, Kind::Avatar);
    assert_eq!(avatar.surface_appearance, None);

    let mut compressed = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    let ObjectUpdateCompressed(update) = &mut compressed else { panic!("expected compressed object fixture") };
    update.objects[0].texture_entry = Some(texture_entry);
    let compressed_batch = decode_object_message(REGION, 31, OrderEvidence::Ordered(31), &compressed).expect("compressed object batch");
    let Some(ObjectOperation::Observe(object)) = compressed_batch.blocks[0].as_ref() else {
        panic!("expected compressed volume observation")
    };
    assert!(object.surface_appearance.is_some());

    let mut terse = packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE).message().clone();
    let ImprovedTerseObjectUpdate(update) = &mut terse else { panic!("expected improved-terse fixture") };
    update.objects[0].as_decoded_mut().expect("fixture block is structurally valid").texture_entry = packed_texture_entry();
    let terse_batch = decode_object_message(REGION, 32, OrderEvidence::Ordered(32), &terse).expect("terse object batch");
    let Some(ObjectOperation::Patch(patch)) = terse_batch.blocks[0].as_ref() else { panic!("expected terse patch") };
    assert!(patch.surface_appearance.is_some());

    let mut terse_avatar = packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE_AVATAR).message().clone();
    let ImprovedTerseObjectUpdate(update) = &mut terse_avatar else { panic!("expected improved-terse avatar fixture") };
    update.objects[0].as_decoded_mut().expect("fixture block is structurally valid").texture_entry = packed_texture_entry();
    let avatar_batch = decode_object_message(REGION, 33, OrderEvidence::Ordered(33), &terse_avatar).expect("terse avatar batch");
    let Some(ObjectOperation::Patch(patch)) = avatar_batch.blocks[0].as_ref() else { panic!("expected terse avatar patch") };
    assert_eq!(patch.surface_appearance, None);
}

#[test]
fn malformed_texture_entries_are_field_local() {
    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.objects[0].texture_entry = vec![0];

    let batch = decode_object_message(REGION, 34, OrderEvidence::Ordered(34), &full).expect("full object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected volume observation") };

    assert_eq!(batch.invalid_blocks, 0);
    assert_eq!(batch.invalid_fields, 1);
    assert_eq!(observation.surface_appearance, None);
    assert!(observation.pose.is_some());
    assert!(observation.scale.is_some());
    assert_eq!(observation.shape, FieldUpdate::Set(Shape::Prim(full_prim_shape())));
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
fn embedded_materials_replace_and_clear() {
    let bindings = [(0, UUID::from_u128(0x310)), (44, UUID::from_u128(0x311))];

    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.objects[0].extra_parameters = vec![render_material_parameter(&bindings)];
    let full_batch = decode_object_message(REGION, 40, OrderEvidence::Ordered(40), &full).expect("full object batch");
    let Some(ObjectOperation::Observe(object)) = full_batch.blocks[0].as_ref() else { panic!("expected volume observation") };
    let materials = object.render_materials.as_ref().expect("PBR parameter decoded");
    assert_eq!(materials.material(0), Some(asset_id(0x310)));
    assert_eq!(materials.material(44), Some(asset_id(0x311)));
    assert_eq!(materials.material(1), None);
    let mut state = world();
    let Introduced(entity) = state.observe_object(object.clone()).expect("PBR update seeds World").outcome else {
        panic!("expected volume object introduction")
    };
    let Some(ObjectOperation::Observe(avatar)) = full_batch.blocks[1].as_ref() else { panic!("expected avatar observation") };
    assert_eq!(avatar.render_materials, None);

    let mut compressed = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    let ObjectUpdateCompressed(update) = &mut compressed else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters = vec![render_material_parameter(&bindings)];
    let compressed_batch = decode_object_message(REGION, 41, OrderEvidence::Ordered(41), &compressed).expect("compressed batch");
    let Some(ObjectOperation::Observe(object)) = compressed_batch.blocks[0].as_ref() else {
        panic!("expected compressed volume observation")
    };
    assert_eq!(object.render_materials.as_ref().unwrap().material(44), Some(asset_id(0x311)));

    let mut absent = packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut absent else { panic!("expected full object fixture") };
    update.objects[0].extra_parameters.clear();
    let absent_batch = decode_object_message(REGION, 42, OrderEvidence::Ordered(42), &absent).expect("full object batch");
    let Some(ObjectOperation::Observe(object)) = absent_batch.blocks[0].as_ref() else { panic!("expected volume observation") };
    assert_eq!(object.render_materials, Some(RenderMaterials::default()));
    assert_eq!(state.observe_object(object.clone()).unwrap().outcome, Updated(entity));
    assert_eq!(state.object(entity).unwrap().render_materials(), None);

    let mut compressed_absent = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    let ObjectUpdateCompressed(update) = &mut compressed_absent else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters.clear();
    let compressed_absent_batch =
        decode_object_message(REGION, 45, OrderEvidence::Ordered(45), &compressed_absent).expect("compressed batch");
    let Some(ObjectOperation::Observe(object)) = compressed_absent_batch.blocks[0].as_ref() else {
        panic!("expected compressed volume observation")
    };
    assert_eq!(object.render_materials, Some(RenderMaterials::default()));

    let mut terse = packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE).message().clone();
    let ImprovedTerseObjectUpdate(update) = &mut terse else { panic!("expected terse fixture") };
    update.objects[0].as_decoded_mut().expect("fixture block is structurally valid").texture_entry = packed_texture_entry();
    let terse_batch = decode_object_message(REGION, 43, OrderEvidence::Ordered(43), &terse).expect("terse batch");
    let Some(ObjectOperation::Patch(patch)) = terse_batch.blocks[0].as_ref() else { panic!("expected terse patch") };
    assert!(patch.surface_appearance.is_some());
    assert_eq!(patch.render_materials, None);
}

#[test]
fn non_volume_updates_clear_render_materials() {
    let bindings = [(0, UUID::from_u128(0x320))];
    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.objects[0].extra_parameters = vec![render_material_parameter(&bindings)];

    let mut state = world();
    let initial_batch = decode_object_message(REGION, 50, OrderEvidence::Ordered(50), &full).expect("initial volume batch");
    let Some(ObjectOperation::Observe(initial)) = initial_batch.blocks[0].as_ref() else { panic!("expected volume observation") };
    let Introduced(entity) = state.observe_object(initial.clone()).expect("initial PBR object applies").outcome else {
        panic!("expected object introduction")
    };

    let mut full_non_volume = full.clone();
    let ObjectUpdate(update) = &mut full_non_volume else { panic!("expected full object fixture") };
    update.objects[0].pcode = 14;
    update.objects[0].extra_parameters.clear();
    let full_batch = decode_object_message(REGION, 51, OrderEvidence::Ordered(51), &full_non_volume).expect("full batch");
    let Some(ObjectOperation::Observe(observation)) = full_batch.blocks[0].as_ref() else { panic!("expected non-volume observation") };
    assert_eq!(observation.render_materials, Some(RenderMaterials::default()));
    assert_eq!(state.observe_object(observation.clone()).expect("full update applies").outcome, Updated(entity));
    assert_eq!(state.object(entity).expect("object remains").render_materials(), None);

    let restored_batch = decode_object_message(REGION, 52, OrderEvidence::Ordered(52), &full).expect("restored volume batch");
    let Some(ObjectOperation::Observe(observation)) = restored_batch.blocks[0].as_ref() else {
        panic!("expected restored volume observation")
    };
    assert_eq!(state.observe_object(observation.clone()).expect("restored PBR applies").outcome, Updated(entity));
    assert!(state.object(entity).expect("object remains").render_materials().is_some());

    let mut compressed_non_volume = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    let ObjectUpdateCompressed(update) = &mut compressed_non_volume else { panic!("expected compressed object fixture") };
    update.objects[0].id = initial.id;
    update.objects[0].local_id = initial.local_id.get();
    update.objects[0].pcode = 14;
    update.objects[0].extra_parameters.clear();
    let compressed_batch = decode_object_message(REGION, 53, OrderEvidence::Ordered(53), &compressed_non_volume).expect("compressed batch");
    let Some(ObjectOperation::Observe(observation)) = compressed_batch.blocks[0].as_ref() else {
        panic!("expected compressed non-volume observation")
    };
    assert_eq!(observation.render_materials, Some(RenderMaterials::default()));
    assert_eq!(state.observe_object(observation.clone()).expect("compressed update applies").outcome, Updated(entity));
    assert_eq!(state.object(entity).expect("object remains").render_materials(), None);
}

#[test]
fn malformed_render_materials_are_field_local() {
    for parameters in
        [vec![ExtraParameter { parameter_type: 0x0080, data: vec![1] }], vec![ExtraParameter { parameter_type: 0x0080, data: vec![0, 0] }]]
    {
        let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
        let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
        update.objects[0].extra_parameters = parameters;

        let batch = decode_object_message(REGION, 44, OrderEvidence::Ordered(44), &full).expect("full object batch");
        let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected volume observation") };
        assert_eq!(observation.render_materials, None);
        assert!(observation.pose.is_some());
        assert!(observation.scale.is_some());
        assert_eq!(observation.shape, FieldUpdate::Set(Shape::Prim(full_prim_shape())));
    }
}

#[test]
fn u8_material_count_skips_extra_faces() {
    let bindings: Vec<_> = (0..usize::from(u8::MAX))
        .map(|index| {
            let face = if index < world::MAX_SURFACE_FACES { index as u8 } else { u8::MAX };
            (face, UUID::from_u128(0x400 + index as u128))
        })
        .collect();

    let mut compressed = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    let ObjectUpdateCompressed(update) = &mut compressed else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters = vec![render_material_parameter(&bindings)];

    let encoded = Packet::new(45, compressed).with_zero_coding(false).encode().expect("full U8 count fits compressed ObjectUpdate");
    let packet = Packet::decode(&encoded).expect("decode full U8 count compressed ObjectUpdate");
    let batch = decode_object_message(REGION, 45, OrderEvidence::Ordered(45), packet.message()).expect("compressed object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected volume observation") };
    let materials = observation.render_materials.as_ref().expect("PBR parameter decoded");
    assert_eq!(materials.material(44), Some(asset_id(0x42c)));
    assert_eq!(materials.material(45), None);
}

#[test]
fn full_materials_follow_variable1_limit() {
    for count in [14_usize, 15] {
        let bindings = (0..count).map(|face| (face as u8, UUID::from_u128(0x500 + face as u128))).collect::<Vec<_>>();
        let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
        let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
        update.objects[0].extra_parameters = vec![render_material_parameter(&bindings)];

        let encoded = Packet::new(46, full).with_zero_coding(false).encode();
        if count == 14 {
            let encoded = encoded.expect("14 records fit the full-update ExtraParams Variable 1 envelope");
            let packet = Packet::decode(&encoded).expect("decode 14-record full ObjectUpdate");
            let batch = decode_object_message(REGION, 46, OrderEvidence::Ordered(46), packet.message()).expect("full object batch");
            let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected full volume observation") };
            assert_eq!(observation.render_materials.as_ref().unwrap().bindings().count(), 14);
        } else {
            assert!(matches!(encoded, Err(protocol::LLUdpError::VariableFieldTooLong)));
        }
    }
}

#[test]
fn incremental_materials_replace_and_clear() {
    let mut state = world_with_regions(2);
    let object_id = UUID::from_u128(0x600);
    let neighbor_id = UUID::from_u128(0x601);
    let first = media_observation(REGION, object_id, 0x610);
    let Introduced(first_entity) = state.observe_object(first).expect("introduce addressed object").outcome else {
        panic!("expected object introduction")
    };
    let neighbor_region = Region::new(12);
    let _ = state.adopt_region(neighbor_region).expect("adopt neighbor region");
    let neighbor = media_observation(neighbor_region, neighbor_id, 0x620);
    let Introduced(neighbor_entity) = state.observe_object(neighbor).expect("introduce same local ID in neighbor").outcome else {
        panic!("expected neighbor object introduction")
    };

    let replacement = render_material_parameter(&[(44, UUID::from_u128(0x630))]).data;
    let set = extra_param_message(0x0102_0304, 0x0080, true, replacement);
    let batch = apply_extra_param_message(&mut state, REGION, &set, 2);
    assert_eq!(batch.invalid_blocks, 0);
    {
        let object = state.object(first_entity).expect("addressed object remains");
        assert_eq!(object.render_materials().unwrap().material(0), None);
        assert_eq!(object.render_materials().unwrap().material(44), Some(asset_id(0x630)));
        assert_eq!(object.shape(), Some(Shape::Prim(Default::default())));
        assert_eq!(object.appearance(), Some(Appearance::Surface));
    }
    assert_eq!(state.object(neighbor_entity).unwrap().render_materials().unwrap().material(0), Some(asset_id(0x620)));

    let clear = extra_param_message(0x0102_0304, 0x0080, false, vec![1]);
    let batch = apply_extra_param_message(&mut state, REGION, &clear, 3);
    assert_eq!(batch.invalid_blocks, 0);
    {
        let object = state.object(first_entity).expect("addressed object remains");
        assert_eq!(object.render_materials(), None);
        assert_eq!(object.shape(), Some(Shape::Prim(Default::default())));
        assert_eq!(object.appearance(), Some(Appearance::Surface));
    }
    assert_eq!(state.object(neighbor_entity).unwrap().render_materials().unwrap().material(0), Some(asset_id(0x620)));

    let set_again = extra_param_message(0x0102_0304, 0x0080, true, render_material_parameter(&[(1, UUID::from_u128(0x631))]).data);
    apply_extra_param_message(&mut state, REGION, &set_again, 4);
    let malformed = extra_param_message(0x0102_0304, 0x0080, true, vec![1]);
    let batch = apply_extra_param_message(&mut state, REGION, &malformed, 5);
    assert_eq!(batch.invalid_blocks, 1);
    assert!(batch.blocks[0].is_none());
    assert_eq!(state.object(first_entity).unwrap().render_materials().unwrap().material(1), Some(asset_id(0x631)));

    let unknown_clear = extra_param_message(0x0102_0304, 0x0030, false, Vec::new());
    apply_extra_param_message(&mut state, REGION, &unknown_clear, 6);
    assert_eq!(state.object(first_entity).unwrap().render_materials().unwrap().material(1), Some(asset_id(0x631)));
}

#[test]
fn repeated_material_params_replace_in_order() {
    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.objects[0].texture_entry = valid_texture_entry();
    update.objects[0].extra_parameters = vec![
        render_material_parameter(&[(0, UUID::from_u128(0x313))]),
        ExtraParameter { parameter_type: 0x0080, data: vec![1] },
        render_material_parameter(&[(44, UUID::from_u128(0x314))]),
    ];

    let batch = decode_object_message(REGION, 46, OrderEvidence::Ordered(46), &full).expect("full object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected volume observation") };
    let materials = observation.render_materials.as_ref().expect("PBR parameter decoded");
    assert_eq!(batch.invalid_fields, 1);
    assert_eq!(materials.material(0), None);
    assert_eq!(materials.material(44), Some(asset_id(0x314)));

    let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.objects[0].extra_parameters =
        vec![render_material_parameter(&[(0, UUID::from_u128(0x313))]), ExtraParameter { parameter_type: 0x0080, data: vec![1] }];
    let batch = decode_object_message(REGION, 47, OrderEvidence::Ordered(47), &full).expect("full object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected volume observation") };
    assert_eq!(batch.invalid_fields, 1);
    assert_eq!(observation.render_materials, None);
}

#[test]
fn compressed_update_keeps_parent_and_angular() {
    let packet = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let batch =
        decode_object_message(REGION, packet.sequence(), OrderEvidence::Ordered(18), packet.message()).expect("compressed object batch");

    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected compressed observation") };
    assert_eq!(observation.parent, ParentUpdate::Local(NonZeroU32::new(0x0506_0708).expect("nonzero parent ID")));
    assert_eq!(observation.angular_velocity, Some(Vec3::new(0.5, 0.0, -0.5)));
    assert_eq!(observation.linear_motion, None);
    assert_eq!(observation.shape, FieldUpdate::Set(Shape::Prim(compressed_prim_shape())));
    assert_eq!(observation.appearance, FieldUpdate::Set(Appearance::Surface));
}

#[test]
fn prim_parameters_follow_ordered_world_updates() {
    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut full else { panic!("expected full fixture") };
    update.objects.truncate(1);
    let id = update.objects[0].id;
    let local_id = update.objects[0].local_id;
    let mut world = world_with_regions(1);
    let batch = decode_object_message(REGION, 1, OrderEvidence::Ordered(1), &full).unwrap();
    let Some(ObjectOperation::Observe(observation)) = &batch.blocks[0] else { panic!("expected observation") };
    let Introduced(entity) = world.observe_object(observation.clone()).unwrap().outcome else { panic!("expected introduction") };
    assert_eq!(world.object(entity).unwrap().shape(), Some(Shape::Prim(full_prim_shape())));

    let mut compressed = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    let ObjectUpdateCompressed(update) = &mut compressed else { panic!("expected compressed fixture") };
    update.objects[0].id = id;
    update.objects[0].local_id = local_id;
    update.objects[0].shape = None;
    let batch = decode_object_message(REGION, 2, OrderEvidence::Ordered(2), &compressed).unwrap();
    let Some(ObjectOperation::Observe(observation)) = &batch.blocks[0] else { panic!("expected observation") };
    assert_eq!(observation.shape, FieldUpdate::Keep);
    let _ = world.observe_object(observation.clone()).unwrap();
    assert_eq!(world.object(entity).unwrap().shape(), Some(Shape::Prim(full_prim_shape())));

    let ObjectUpdateCompressed(update) = &mut compressed else { unreachable!() };
    let ObjectUpdate(original) = &full else { unreachable!() };
    let mut shape = original.objects[0].shape;
    shape.path_curve = u8::MAX;
    shape.profile_curve = u8::MAX;
    shape.path_shear_x = 128;
    shape.path_shear_y = 127;
    shape.profile_hollow = u16::MAX;
    update.objects[0].shape = Some(shape);
    let expected = PrimShape {
        path_curve: u8::MAX,
        profile_curve: u8::MAX,
        path_shear_x: i8::MIN,
        path_shear_y: i8::MAX,
        profile_hollow: u16::MAX,
        ..full_prim_shape()
    };
    let batch = decode_object_message(REGION, 3, OrderEvidence::Ordered(3), &compressed).unwrap();
    let Some(ObjectOperation::Observe(observation)) = &batch.blocks[0] else { panic!("expected observation") };
    assert_eq!(observation.shape, FieldUpdate::Set(Shape::Prim(expected)));
    let _ = world.observe_object(observation.clone()).unwrap();
    assert_eq!(world.object(entity).unwrap().shape(), Some(Shape::Prim(expected)));

    let revision = world.revision();
    let batch = decode_object_message(REGION, 1, OrderEvidence::Ordered(1), &full).unwrap();
    let Some(ObjectOperation::Observe(observation)) = &batch.blocks[0] else { panic!("expected observation") };
    let _ = world.observe_object(observation.clone()).unwrap();
    assert_eq!(world.object(entity).unwrap().shape(), Some(Shape::Prim(expected)));
    assert_eq!(world.revision(), revision);
}

#[test]
fn compressed_shape_classification() {
    let fixture_packet = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    for (parameter_type, subtype, asset_uuid, expected) in [
        (
            0x0030,
            0x41,
            UUID::from_u128(0x111),
            Some(Shape::Sculpt {
                asset_id: asset_id(0x111),
                sculpt_type: SculptType::Sphere,
                flags: SculptFlags { invert: true, mirror: false },
            }),
        ),
        (
            0x0060,
            0x85,
            UUID::from_u128(0x222),
            Some(Shape::Mesh { asset_id: asset_id(0x222), flags: SculptFlags { invert: false, mirror: true } }),
        ),
        (
            0x0030,
            0x42,
            UUID::from_u128(0x224),
            Some(Shape::Sculpt {
                asset_id: asset_id(0x224),
                sculpt_type: SculptType::Torus,
                flags: SculptFlags { invert: true, mirror: false },
            }),
        ),
        (
            0x0030,
            0x83,
            UUID::from_u128(0x225),
            Some(Shape::Sculpt {
                asset_id: asset_id(0x225),
                sculpt_type: SculptType::Plane,
                flags: SculptFlags { invert: false, mirror: true },
            }),
        ),
        (
            0x0030,
            0xc4,
            UUID::from_u128(0x226),
            Some(Shape::Sculpt {
                asset_id: asset_id(0x226),
                sculpt_type: SculptType::Cylinder,
                flags: SculptFlags { invert: true, mirror: true },
            }),
        ),
        (0x0030, 0, UUID::nil(), Some(Shape::Prim(compressed_prim_shape()))),
        (0x0030, 6, UUID::from_u128(0x333), None),
        (
            0x0030,
            0xc7,
            UUID::from_u128(0x333),
            Some(Shape::Sculpt {
                asset_id: asset_id(0xbe29_3869_d0d9_0a69_5989_ad27_f194_6fd4),
                sculpt_type: SculptType::Sphere,
                flags: SculptFlags::default(),
            }),
        ),
        (
            0x0030,
            0x09,
            UUID::from_u128(0x333),
            Some(Shape::Sculpt {
                asset_id: asset_id(0xbe29_3869_d0d9_0a69_5989_ad27_f194_6fd4),
                sculpt_type: SculptType::Sphere,
                flags: SculptFlags::default(),
            }),
        ),
    ] {
        let mut data = vec![0; 17];
        data[..16].copy_from_slice(asset_uuid.as_bytes());
        data[16] = subtype;
        let mut message = fixture_packet.message().clone();
        let ObjectUpdateCompressed(update) = &mut message else { panic!("expected compressed object fixture") };
        update.objects[0].extra_parameters = vec![ExtraParameter { parameter_type, data }];
        let batch = decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(19), &message)
            .expect("compressed object batch");
        let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected compressed observation") };
        assert_eq!(
            observation.shape,
            expected.map_or(FieldUpdate::Clear, FieldUpdate::Set),
            "extra parameter type {parameter_type:#06x}, subtype {subtype}"
        );
        if subtype & 0x38 == 0
            && (1..=5).contains(&(subtype & 0x07))
            && let Some(Shape::Sculpt { asset_id, .. } | Shape::Mesh { asset_id, .. }) = expected
        {
            assert_eq!(asset_id.uuid(), asset_uuid);
            assert_ne!(observation.id, asset_uuid);
        }
        assert_eq!(observation.appearance, FieldUpdate::Set(Appearance::Surface));
    }

    let mut message = fixture_packet.message().clone();
    let ObjectUpdateCompressed(update) = &mut message else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters = vec![ExtraParameter { parameter_type: 0x0030, data: vec![0; 16] }];
    let batch =
        decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(20), &message).expect("compressed object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected compressed observation") };
    assert_eq!(observation.shape, FieldUpdate::Keep);

    let mut first_data = vec![0; 17];
    first_data[..16].copy_from_slice(UUID::from_u128(0x444).as_bytes());
    first_data[16] = 1;
    let mut last_data = vec![0; 17];
    last_data[..16].copy_from_slice(UUID::from_u128(0x555).as_bytes());
    last_data[16] = 5;
    let mut message = fixture_packet.message().clone();
    let ObjectUpdateCompressed(update) = &mut message else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters =
        vec![ExtraParameter { parameter_type: 0x0030, data: first_data }, ExtraParameter { parameter_type: 0x0060, data: last_data }];
    let batch =
        decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(20), &message).expect("compressed object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected compressed observation") };
    assert_eq!(observation.shape, FieldUpdate::Set(Shape::Mesh { asset_id: asset_id(0x555), flags: SculptFlags::default() }));

    let mut malformed_data = vec![0; 16];
    malformed_data[..16].copy_from_slice(UUID::from_u128(0x666).as_bytes());
    let mut valid_data = vec![0; 17];
    valid_data[..16].copy_from_slice(UUID::from_u128(0x777).as_bytes());
    valid_data[16] = 5;
    let mut message = fixture_packet.message().clone();
    let ObjectUpdateCompressed(update) = &mut message else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters = vec![
        ExtraParameter { parameter_type: 0x0030, data: malformed_data.clone() },
        ExtraParameter { parameter_type: 0x0060, data: valid_data.clone() },
    ];
    let batch =
        decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(20), &message).expect("compressed object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected compressed observation") };
    assert_eq!(observation.shape, FieldUpdate::Set(Shape::Mesh { asset_id: asset_id(0x777), flags: SculptFlags::default() }));

    let mut message = fixture_packet.message().clone();
    let ObjectUpdateCompressed(update) = &mut message else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters =
        vec![ExtraParameter { parameter_type: 0x0060, data: valid_data }, ExtraParameter { parameter_type: 0x0030, data: malformed_data }];
    let batch =
        decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(20), &message).expect("compressed object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected compressed observation") };
    assert_eq!(observation.shape, FieldUpdate::Keep);
}

#[test]
fn full_shape_classification() {
    let fixture_packet = packet(vectors::OBJECT_UPDATE);
    let mut regular = fixture_packet.message().clone();
    let Message::ObjectUpdate(update) = &mut regular else { panic!("expected full object fixture") };
    update.objects.truncate(1);
    update.objects[0].pcode = 9;
    let batch = decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(18), &regular).expect("full batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected full observation") };
    assert_eq!(observation.shape, FieldUpdate::Set(Shape::Prim(full_prim_shape())));

    let mut message = fixture_packet.message().clone();
    let Message::ObjectUpdate(update) = &mut message else { panic!("expected full object fixture") };
    update.objects.truncate(1);
    update.objects[0].pcode = 9;
    let mut data = vec![0; 17];
    data[..16].copy_from_slice(UUID::from_u128(0x111).as_bytes());
    data[16] = 1;
    update.objects[0].extra_parameters = vec![ExtraParameter { parameter_type: 0x0030, data }];

    let batch = decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(19), &message).expect("full batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected full observation") };

    assert_eq!(
        observation.shape,
        FieldUpdate::Set(Shape::Sculpt { asset_id: asset_id(0x111), sculpt_type: SculptType::Sphere, flags: SculptFlags::default() })
    );
    assert_eq!(observation.appearance, FieldUpdate::Set(Appearance::Surface));
}

#[test]
fn malformed_shape_params_preserve_shape() {
    let fixture_packet = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let mut valid_message = fixture_packet.message().clone();
    let ObjectUpdateCompressed(update) = &mut valid_message else { panic!("expected compressed object fixture") };
    let mut sculpt_data = vec![0; 17];
    sculpt_data[..16].copy_from_slice(UUID::from_u128(0x666).as_bytes());
    sculpt_data[16] = 0xc1;
    update.objects[0].extra_parameters = vec![ExtraParameter { parameter_type: 0x0030, data: sculpt_data }];
    let valid_batch = decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(22), &valid_message)
        .expect("compressed object batch");
    let Some(ObjectOperation::Observe(valid_observation)) = valid_batch.blocks[0].as_ref() else {
        panic!("expected compressed observation")
    };

    let mut world = world();
    let Introduced(entity) = world.observe_object(valid_observation.clone()).expect("valid sculpt update").outcome else {
        panic!("expected object introduction")
    };
    assert_eq!(
        world.object(entity).expect("object remains").shape(),
        Some(Shape::Sculpt {
            asset_id: asset_id(0x666),
            sculpt_type: SculptType::Sphere,
            flags: SculptFlags { invert: true, mirror: true },
        })
    );

    let mut malformed_message = fixture_packet.message().clone();
    let ObjectUpdateCompressed(update) = &mut malformed_message else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters = vec![ExtraParameter { parameter_type: 0x0030, data: vec![0; 16] }];
    update.objects[0].scale = Vec3::splat(2.0);
    let malformed_batch = decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(23), &malformed_message)
        .expect("compressed object batch");
    let Some(ObjectOperation::Observe(malformed_observation)) = malformed_batch.blocks[0].as_ref() else {
        panic!("expected compressed observation")
    };
    assert_eq!(malformed_observation.shape, FieldUpdate::Keep);

    let applied = world.observe_object(malformed_observation.clone()).expect("unrelated valid fields still apply");
    assert!(matches!(applied.outcome, Updated(_)));
    assert_eq!(
        world.object(entity).expect("object remains").shape(),
        Some(Shape::Sculpt {
            asset_id: asset_id(0x666),
            sculpt_type: SculptType::Sphere,
            flags: SculptFlags { invert: true, mirror: true },
        })
    );
    assert_eq!(world.object(entity).expect("object remains").scale(), Some(Vec3::splat(2.0)));
}

#[test]
fn terse_preserves_shape_and_appearance() {
    let packet = packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let batch = decode_object_message(REGION, packet.sequence(), OrderEvidence::Ordered(21), packet.message()).expect("terse batch");
    let Some(ObjectOperation::Patch(patch)) = batch.blocks[0].as_ref() else { panic!("expected terse patch") };
    assert_eq!(patch.shape, FieldUpdate::Keep);
    assert_eq!(patch.appearance, FieldUpdate::Keep);
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
    assert!(matches!(batch.blocks[1].as_ref(), Some(ObjectOperation::Observe(observation)) if observation.parent == ParentUpdate::Root));
}

#[test]
fn non_object_messages_are_ignored() {
    let message = Message::StartPingCheck(StartPingCheck::default());
    assert!(decode_object_message(REGION, 1, OrderEvidence::Ordered(1), &message).is_none());
}

#[test]
fn media_requests_cover_full_and_compressed() {
    let marker = "x-mv:12/00000000-0000-0000-0000-000000000001";
    let id = UUID::from_u128(0x1234);
    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    let Message::ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.objects.truncate(1);
    update.objects[0].pcode = 9;
    update.objects[0].id = id;
    update.objects[0].local_id = 7;
    update.objects[0].media_url = marker.into();

    let requests = object_media_requests(REGION, 8, OrderEvidence::Ordered(8), &full);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].object_id, id);
    assert_eq!(requests[0].local_id.get(), 7);
    assert_eq!(requests[0].version, 12);
    assert_eq!(requests[0].source.region(), REGION);

    let mut compressed = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    let Message::ObjectUpdateCompressed(update) = &mut compressed else { panic!("expected compressed fixture") };
    update.objects.truncate(1);
    update.objects[0].pcode = 9;
    update.objects[0].id = id;
    update.objects[0].local_id = 7;
    update.objects[0].media_url = Some(marker.into());
    let requests = object_media_requests(REGION, 9, OrderEvidence::Ordered(9), &compressed);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].object_id, id);
    assert_eq!(requests[0].version, 12);
}

#[test]
fn media_requests_require_volume_marker() {
    let mut message = packet(vectors::OBJECT_UPDATE).message().clone();
    {
        let Message::ObjectUpdate(update) = &mut message else { panic!("expected full object fixture") };
        update.objects.truncate(1);
        update.objects[0].pcode = 47;
        update.objects[0].media_url = "x-mv:12/00000000-0000-0000-0000-000000000001".into();
    }
    assert!(object_media_requests(REGION, 1, OrderEvidence::Ordered(1), &message).is_empty());

    {
        let Message::ObjectUpdate(update) = &mut message else { panic!("expected full object fixture") };
        update.objects[0].pcode = 9;
        update.objects[0].media_url = "https://media.test/object".into();
    }
    assert!(object_media_requests(REGION, 2, OrderEvidence::Ordered(2), &message).is_empty());
}

#[test]
fn attachment_state_decodes_wire_nibbles() {
    assert_eq!(attachment_point(0), FieldUpdate::Clear);
    assert_eq!(attachment_point(0x10), FieldUpdate::Set(AttachmentPoint::new(NonZeroU8::new(1).unwrap())));
    assert_eq!(attachment_point(0x18), FieldUpdate::Set(AttachmentPoint::new(NonZeroU8::new(1).unwrap())));
    assert_eq!(attachment_point(0x80), FieldUpdate::Set(AttachmentPoint::new(NonZeroU8::new(8).unwrap())));
    assert_eq!(attachment_point(0x88), FieldUpdate::Set(AttachmentPoint::new(NonZeroU8::new(8).unwrap())));
}

fn message(method: u16, data: &[u8]) -> Message {
    Message::GenericStreamingMessage(GenericStreamingMessage { method, data: data.to_vec() })
}

#[test]
fn override_message_targets_local_id() {
    let message = message(0x4175, br"{'id':i7,'te':[i4],'od':[{'mf':r0.5}]}");
    let batch = decode_object_message(Region::new(11), 5, OrderEvidence::Ordered(5), &message).expect("override batch");

    let Some(ObjectOperation::Patch(patch)) = &batch.blocks[0] else { panic!("expected object patch") };
    assert_eq!(patch.local_id.get(), 7);
    assert_eq!(patch.source.region(), Region::new(11));
    assert_eq!(patch.gltf_material_overrides.as_ref().unwrap().face(4).unwrap().metallic, Some(0.5));
}

#[test]
fn malformed_override_is_ignored() {
    let message = message(0x4175, b"not LLSD");
    let batch = decode_object_message(Region::new(11), 5, OrderEvidence::Ordered(5), &message).expect("override batch");

    assert_eq!(batch.invalid_blocks, 1);
    assert!(batch.blocks.iter().all(Option::is_none));
}

#[test]
fn unrelated_streaming_method_is_ignored() {
    let message = message(0x1234, br"{'id':i7,'te':[],'od':[]}");

    assert!(decode_object_message(Region::new(11), 5, OrderEvidence::Ordered(5), &message).is_none());
}

#[test]
fn empty_snapshot_is_an_explicit_clear() {
    let message = message(0x4175, br"{'id':i7,'te':[],'od':[]}");
    let batch = decode_object_message(Region::new(11), 5, OrderEvidence::Ordered(5), &message).expect("override batch");

    let Some(ObjectOperation::Patch(patch)) = &batch.blocks[0] else { panic!("expected object patch") };
    assert!(patch.gltf_material_overrides.as_ref().unwrap().is_empty());
}

#[test]
fn compressed_owner_metadata_does_not_populate_object_properties() {
    let packet = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let message = packet.message().clone();
    let Message::ObjectUpdateCompressed(update) = &message else { panic!("expected compressed object fixture") };
    let first_owner = update.objects[0].owner_id;
    assert_ne!(first_owner, UUID::from_u128(0));

    let batch = decode_object_message(REGION, packet.sequence(), OrderEvidence::Ordered(100), &message)
        .expect("compressed fixture has an object message");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks.first().and_then(Option::as_ref) else {
        panic!("expected compressed observation")
    };
    let mut world = world();
    let introduced = world.observe_object(observation.clone()).expect("decoded compressed observation applies");
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };
    assert_eq!(world.object(entity).unwrap().properties().owner_id, None);
    assert_eq!(world.object(entity).unwrap().properties().creator_id, None);
}
