use std::num::{NonZeroU8, NonZeroU32};

use math::{Quat, UvTransform, Vec3};
use protocol::{
    lludp::{Message, Message::ImprovedTerseObjectUpdate, Message::ObjectUpdate, Message::ObjectUpdateCompressed, Packet},
    messages::{
        ExtraParamUpdate, ExtraParameter, GenericStreamingMessage, ObjectExtraParams, StartPingCheck, object::improved_terse_object_update,
    },
};
use test_support::{lludp::vectors, parse_hex_fixture};
use utils::AssetId;
use uuid::Uuid;
use world::{
    Appearance,
    ApplyOutcome::{Introduced, Updated},
    AttachmentPoint, FaceAppearance,
    FieldRejectionReason::InvalidRotation,
    FieldUpdate, Kind, ObjectObservation, ObjectPatch, OrderEvidence, ParentUpdate, PbrMaterialAssignments, Pose, PrimShape, Region,
    SculptFlags, SculptType, Shape, TextureMapping, UpdateSource, World, WorldLimits,
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
    AssetId::new(Uuid::from_u128(value))
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

fn render_material_parameter(bindings: &[(u8, Uuid)]) -> ExtraParameter {
    let mut data = vec![u8::try_from(bindings.len()).expect("test binding count fits")];
    for (face_index, material_id) in bindings {
        data.push(*face_index);
        data.extend_from_slice(material_id.as_bytes());
    }
    ExtraParameter { parameter_type: 0x0080, data }
}

fn media_observation(region: Region, id: Uuid, initial_material: u128) -> ObjectObservation {
    ObjectObservation {
        pose: Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) }),
        scale: Some(Vec3::ONE),
        shape: FieldUpdate::Set(Shape::Prim(Default::default())),
        appearance: FieldUpdate::Set(Appearance::Surface),
        pbr_materials: Some(PbrMaterialAssignments::new(vec![(0, asset_id(initial_material))]).expect("valid initial material")),
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
        agent_id: Uuid::nil(),
        session_id: Uuid::nil(),
        object_data: vec![ExtraParamUpdate { local_id, parameter_type, in_use, data }],
    })
}

fn apply_extra_param_message(world: &mut World, region: Region, message: &Message, order: u64) -> super::ObjectBatch {
    let batch = decode_object_message(region, order as u32, OrderEvidence::Ordered(order), message).expect("extra-parameter batch");
    for operation in batch.blocks.iter().flatten() {
        if let ObjectOperation::Patch(patch) = operation {
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

fn message(method: u16, data: &[u8]) -> Message {
    Message::GenericStreamingMessage(GenericStreamingMessage { method, data: data.to_vec() })
}

#[path = "object_appearance_tests.rs"]
mod appearance;
#[path = "object_media_tests.rs"]
mod media;
#[path = "object_shape_tests.rs"]
mod shape;
#[path = "object_transforms_tests.rs"]
mod transforms;
#[path = "object_updates_tests.rs"]
mod updates;
