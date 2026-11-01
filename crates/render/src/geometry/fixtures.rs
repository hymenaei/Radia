use std::assert_matches;
use std::{num::NonZeroU32, time::Instant};

use math::{DVec3, Quat, Vec3};
use uuid::Uuid;
use world::{
    Appearance, FaceAppearance, FieldUpdate, Kind, ObjectObservation, OrderEvidence, ParentUpdate, Pose, PrimShape, Region, Shape,
    SurfaceAppearance, UpdateSource, World, WorldLimits,
};

use crate::scene::{Budget, Camera, ReadQuery, ReadReply, ReconcileStep, Reconciler, SceneView};

pub(super) fn scene(alpha: u8) -> (Reconciler, SceneView, Camera) {
    scene_with_scales(alpha, [Vec3::splat(2.0); 2])
}

pub(super) fn scene_with_scales(alpha: u8, scales: [Vec3; 2]) -> (Reconciler, SceneView, Camera) {
    scene_with_mapping(alpha, scales, world::TextureMapping::Default)
}

pub(super) fn scene_with_mapping(alpha: u8, scales: [Vec3; 2], mapping: world::TextureMapping) -> (Reconciler, SceneView, Camera) {
    let (_, scene, view, camera) = scene_with_world(alpha, scales, mapping);
    (scene, view, camera)
}

pub(super) fn scene_with_agent() -> (Reconciler, SceneView, Camera) {
    let (_, scene, view, camera) =
        scene_with_world_with_agent(255, [Vec3::splat(2.0); 2], world::TextureMapping::Default, Some(Uuid::from_u128(1)));
    (scene, view, camera)
}

pub(super) fn scene_with_world(alpha: u8, scales: [Vec3; 2], mapping: world::TextureMapping) -> (World, Reconciler, SceneView, Camera) {
    scene_with_world_with_agent(alpha, scales, mapping, None)
}

fn scene_with_world_with_agent(
    alpha: u8,
    scales: [Vec3; 2],
    mapping: world::TextureMapping,
    agent_id: Option<Uuid>,
) -> (World, Reconciler, SceneView, Camera) {
    let mut world = World::new(WorldLimits {
        max_region_registrations: 1,
        max_entities: 2,
        max_entities_per_region: 2,
        max_tombstones_per_region: 1,
        max_tombstones: 1,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
        max_journal_entries: 16,
        max_journal_changes: 64,
        max_region_name_bytes: 64,
        max_recovery_targets: 4,
    });
    let region = Region::new(1);
    assert_matches!(world.adopt_region(region).unwrap().outcome, world::RegionOutcome::Adopted { .. });
    let _ = world.patch_region(region, world::RegionInfo { origin: Some(DVec3::ZERO), ..Default::default() }).unwrap();
    for ((id, y, mut color, shape), scale) in [
        (1, -2.0, [255, 0, 0, 255], PrimShape::default()),
        (2, 2.0, [0, 255, 0, 255], PrimShape { path_scale_x: 150, ..PrimShape::default() }),
    ]
    .into_iter()
    .zip(scales)
    {
        color[3] = alpha;
        let object_id = Uuid::from_u128(u128::from(id));
        let kind = if Some(object_id) == agent_id { Kind::Avatar } else { Kind::Object };
        let mut object = ObjectObservation::new(
            UpdateSource::new(region, id, OrderEvidence::Ordered(u64::from(id)), 0),
            object_id,
            NonZeroU32::new(id).unwrap(),
            kind,
            ParentUpdate::Root,
        );
        let position = if alpha == 255 { Vec3::new(10.0, y, 0.0) } else { Vec3::new(if id == 1 { 10.0 } else { 14.0 }, y * 0.35, 0.0) };
        object.pose = Some(Pose { position: Some(position), rotation: Some(Quat::IDENTITY) });
        object.scale = Some(scale);
        object.shape = FieldUpdate::Set(Shape::Prim(shape));
        object.appearance = FieldUpdate::Set(Appearance::Surface);
        object.surface_appearance =
            Some(SurfaceAppearance::new(FaceAppearance { color, mapping, ..Default::default() }, Vec::new()).unwrap());
        assert_matches!(world.observe_object(object).unwrap().outcome, world::ApplyOutcome::Introduced(_));
    }
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [128, 128]).unwrap();
    scene.set_camera(camera);
    let mut now = Instant::now();
    loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = match job.query() {
                    ReadQuery::Barrier => Ok(ReadReply::Barrier(world.read_barrier())),
                    ReadQuery::Objects(cursor) => world.object_list_page(*cursor, 256).map(ReadReply::Objects),
                    ReadQuery::Regions(cursor) => world.region_page(*cursor, 64).map(ReadReply::Regions),
                    ReadQuery::Details(entities) => world.object_detail_batch(entities).map(ReadReply::Details),
                    ReadQuery::Changes(cursor) => world.change_page(*cursor, 256).map(ReadReply::Changes),
                    ReadQuery::Terrain(region, cursor) => world.terrain_page(*region, *cursor, 8).map(ReadReply::Terrain),
                    ReadQuery::Environment(region) => Ok(ReadReply::Environment(world.environment(*region))),
                };
                scene.complete_read(job, result.map_err(crate::scene::ReadFailure::World), now);
            }
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Publish(batch) => {
                view.apply(batch).unwrap();
            }
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            ReconcileStep::Idle => {
                assert_eq!(view.object_count(), 2);
                assert!(view.objects().all(|object| object.mesh().is_some() && object.materials().is_some()));
                return (world, scene, view, camera);
            }
            _ => panic!("fixture stopped before publishing geometry"),
        }
    }
}
