use assets::Priority;
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU32,
    time::{Duration, Instant},
};

use assets::{Request, TextureSize};
use math::{DVec3, Quat, Vec3};
use proptest::prelude::*;
use scene::{
    Alpha, AssetChange, AssetRequest, AssetTicket, Budget, Camera, Detail, EnvironmentProperty, EnvironmentPush, ExperienceId, Mesh,
    Placement, PreparedMesh, ReadFailure, ReadJob, ReadQuery, ReadReply, ReconcileStep, RecoveryReason, Resolution, SceneReconciler,
    SceneView, Status, SurfaceKind,
};
use utils::AssetId;
use uuid::Uuid;
use world::{
    Appearance, EnvironmentAssignment, EnvironmentScope, EnvironmentSource, FaceAppearance, FieldUpdate, GltfAlphaMode,
    GltfMaterialOverride, GltfMaterialOverrides, Kind, ObjectObservation, ObjectPatch, OrderEvidence::Ordered, ParcelId, ParentUpdate,
    Pose, Region, RegionInfo, RenderMaterials, Shape, SurfaceAppearance, TerrainPatch, TerrainPatchId, World, WorldLimits, WorldReadError,
};

fn world() -> World {
    world_at(DVec3::ZERO)
}

fn world_at(origin: DVec3) -> World {
    world_with_journal(origin, 4096)
}

fn world_with_journal(origin: DVec3, journal_entries: usize) -> World {
    let mut world = World::new(WorldLimits {
        max_region_registrations: 40,
        max_entities: 2048,
        max_entities_per_region: 2048,
        max_tombstones_per_region: 2048,
        max_tombstones: 4096,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(2048),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(4096),
        max_journal_entries: journal_entries,
        max_journal_changes: journal_entries * 2,
        max_region_name_bytes: 256,
        max_recovery_targets: 8,
    });
    let _ = world.adopt_region(Region::new(1)).unwrap();
    let _ = world.patch_region(Region::new(1), RegionInfo { origin: Some(origin), ..RegionInfo::default() }).unwrap();
    world
}

fn source(region: u64, order: u64) -> world::UpdateSource {
    world::UpdateSource::new(Region::new(region), order as u32, Ordered(order), 0)
}

fn add(world: &mut World, id: u128, local: u32, parent: ParentUpdate, x: f32, order: u64) {
    let mut obj = ObjectObservation::new(source(1, order), Uuid::from_u128(id), NonZeroU32::new(local).unwrap(), Kind::Object, parent);
    obj.pose = Some(Pose { position: Some(Vec3::new(x, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
    obj.scale = Some(Vec3::ONE);
    obj.shape = FieldUpdate::Set(Shape::Prim(Default::default()));
    obj.appearance = FieldUpdate::Set(Appearance::Surface);
    let _ = world.observe_object(obj).unwrap();
}

fn move_root(world: &mut World, x: f32, order: u64) {
    let mut patch = ObjectPatch::new(source(1, order), NonZeroU32::new(1).unwrap());
    patch.pose = Some(Pose { position: Some(Vec3::new(x, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
    let _ = world.patch_object(patch).unwrap();
}

fn reply(world: &World, job: &ReadJob) -> Result<ReadReply, ReadFailure> {
    let result = match job.query() {
        ReadQuery::Barrier => return Ok(ReadReply::Barrier(world.read_barrier())),
        ReadQuery::Objects(cursor) => world.object_list_page(*cursor, 256).map(ReadReply::Objects),
        ReadQuery::Regions(cursor) => world.region_page(*cursor, 64).map(ReadReply::Regions),
        ReadQuery::Details(entities) => world.object_detail_batch(entities).map(ReadReply::Details),
        ReadQuery::Changes(cursor) => world.change_page(*cursor, 256).map(ReadReply::Changes),
        ReadQuery::Terrain(region, cursor) => world.terrain_page(*region, *cursor, 8).map(ReadReply::Terrain),
        ReadQuery::Environment(region) => Ok(ReadReply::Environment(world.environment(*region))),
    };
    result.map_err(ReadFailure::World)
}

fn settle(scene: &mut SceneReconciler, view: &mut SceneView, world: &World, now: &mut Instant) -> usize {
    settle_work(scene, view, world, now).0
}

fn settle_work(scene: &mut SceneReconciler, view: &mut SceneView, world: &World, now: &mut Instant) -> (usize, usize) {
    let work = settle_materials(scene, view, world, now);
    assert!(work.requested.is_empty(), "geometry fixture unexpectedly requested assets");
    (work.reads, work.builds)
}

#[derive(Default)]
struct AssetWork {
    reads: usize,
    builds: usize,
    requested: Vec<AssetRequest>,
    released: Vec<AssetTicket>,
}

fn settle_materials(scene: &mut SceneReconciler, view: &mut SceneView, world: &World, now: &mut Instant) -> AssetWork {
    let mut work = AssetWork::default();
    for _ in 0..20_000 {
        match scene.poll(*now) {
            ReconcileStep::Asset(AssetChange::Request(request)) => work.requested.push(request),
            ReconcileStep::Asset(AssetChange::Release(ticket)) => work.released.push(ticket),
            ReconcileStep::Read(job) => {
                work.reads += 1;
                let result = reply(world, &job);
                scene.complete(job, result, *now);
            }
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Build(job) => {
                work.builds += 1;
                scene.complete_mesh(job.run());
            }
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => *now = until,
            ReconcileStep::Idle => return work,
            ReconcileStep::Closed => panic!("unexpected closure"),
        }
    }
    panic!("Scene did not converge: {:?}", scene.status());
}

fn ids(view: &SceneView) -> BTreeSet<u128> {
    view.objects().map(|object| object.detail().summary.id.as_u128()).collect()
}

fn prepared_asset(resource: Request, bytes: Vec<u8>) -> assets::Status {
    let now = Instant::now();
    let mut store = assets::Store::new(assets::Budget::default());
    let route = store.register_route().unwrap();
    let demand = store.request(route, resource, Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let prepared = job.prepare(bytes).unwrap();
    store.complete(job, Ok(prepared), now);
    demand.status()
}

fn material_asset(id: AssetId) -> assets::Status {
    let json = r#"{"asset":{"version":"2.0"},
        "images":[{"uri":"00000000-0000-0000-0000-000000000064"},{"uri":"00000000-0000-0000-0000-0000000000c8"}],
        "textures":[{"source":0},{"source":1}],
        "materials":[{"pbrMetallicRoughness":{"baseColorFactor":[0.2,0.3,0.4,0.5],"baseColorTexture":{"index":0}},
            "normalTexture":{"index":1},"alphaMode":"MASK","alphaCutoff":0.4,"doubleSided":true}]}
    "#;
    let mut envelope = b"{\0\0\0\x03k\0\0\0\x07versions\0\0\0\x031.1k\0\0\0\x04types\0\0\0\x08GLTF 2.0k\0\0\0\x04datas".to_vec();
    envelope.extend((json.len() as u32).to_be_bytes());
    envelope.extend_from_slice(json.as_bytes());
    envelope.push(b'}');
    prepared_asset(Request::PbrMaterial(id), envelope)
}

#[test]
fn environment_changes_follow_world() {
    let mut world = world();
    let region = Region::new(1);
    let budget = Budget::default();
    let (mut scene, mut view) = SceneReconciler::new(budget.clone());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.environment(region).unwrap().region_assignment, None);
    let target = world.environment_target(region, EnvironmentScope::Region).unwrap();
    let assignment = EnvironmentAssignment {
        source: EnvironmentSource::Inline(vec![0; 2 * 1024 * 1024].into()),
        version: Some(1),
        day_length_seconds: Some(14_400),
        day_offset_seconds: Some(0),
        altitudes: Some([1000.0, 2000.0, 3000.0]),
    };
    let _ = world.observe_environment(target, source(1, 1), assignment.clone()).unwrap();
    scene.notify_world_change();
    let (_, builds) = settle_work(&mut scene, &mut view, &world, &mut now);
    assert_eq!(builds, 0, "assignment updates do not rebuild geometry");
    assert_eq!(view.environment(region).unwrap().region_assignment, Some(assignment));
    assert_eq!(view.revision(), Some(world.revision()));
    let parcel = ParcelId::new(7).unwrap();
    let _ = world.observe_agent_parcel(source(1, 2), Some(parcel)).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.environment(region).unwrap().parcel.as_ref().unwrap().parcel, parcel);
    let _ = world.retire_region(region).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.environment(region), None);
    scene.close();
    drop(scene);
    drop(view);
    assert_eq!(budget.charged_bytes(), 0, "retirement and teardown release environment source admission");
}

#[test]
fn versions_fence_sessions_and_close() {
    let (mut scene, view) = SceneReconciler::new(Budget::default());
    let (_other_scene, other) = SceneReconciler::new(Budget::default());
    let version = view.version();
    assert_eq!(view.sequence(), other.sequence());
    assert_eq!(version, view.version(), "unchanged publication keeps its version");
    assert_ne!(version, other.version(), "matching sequence numbers cannot alias across sessions");
    scene.close();
    assert_ne!(version, view.version(), "closing invalidates cached content before another publication");
    assert_eq!(view.status(), Status::Closed);
}

#[test]
fn unseen_objects_get_baselines() {
    let mut world = world();
    for (id, x) in [(1, -10.0), (2, 10.0), (3, -20.0)] {
        add(&mut world, id, id as u32, ParentUpdate::Root, x, id as u64 * 2);
        let mut patch = ObjectPatch::new(source(1, id as u64 * 2 + 1), NonZeroU32::new(id as u32).unwrap());
        patch.surface_appearance = Some(
            SurfaceAppearance::new(
                FaceAppearance { texture_id: Uuid::from_u128(100 + id), ..FaceAppearance::default() },
                if id == 3 {
                    vec![(0, FaceAppearance { texture_id: Uuid::from_u128(203), ..FaceAppearance::default() })]
                } else {
                    Vec::new()
                },
            )
            .unwrap(),
        );
        let _ = world.patch_object(patch).unwrap();
    }
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.camera(camera(0.0));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    for id in [103, 203] {
        assert!(
            work.requested
                .iter()
                .any(|request| request.priority == Priority::Background && request.request.resource().id.uuid() == Uuid::from_u128(id)),
            "off-camera defaults and face-zero overrides both need baseline coverage"
        );
    }
    let baseline = work.requested.iter().find(|request| request.request.resource().id.uuid() == Uuid::from_u128(101)).unwrap();
    assert_eq!(baseline.priority, Priority::Background);
    assert_eq!(baseline.request, Request::Texture { id: AssetId::new(Uuid::from_u128(101)), size: TextureSize::new(32) });
    let object = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(1)).unwrap();
    assert!(object.mesh().is_none(), "baseline loading does not require preparing off-camera geometry");
    scene.complete_asset(
        baseline.ticket,
        prepared_asset(baseline.request, include_bytes!("../../assets/tests/fixtures/gray-4096.j2c").to_vec()),
    );
    settle_materials(&mut scene, &mut view, &world, &mut now);
    scene.camera(Camera::new(DVec3::ZERO, -Vec3::X, std::f64::consts::FRAC_PI_2, [1000, 1000]).unwrap());
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let current = work
        .requested
        .iter()
        .find(|request| request.priority == Priority::Visible && request.request.resource().id.uuid() == Uuid::from_u128(101))
        .unwrap();
    assert_ne!(current.ticket, baseline.ticket);
    let object = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(1)).unwrap();
    assert!(object.mesh().is_some());
    assert!(
        object.materials().unwrap().iter().all(|face| face.base_color.as_ref().unwrap().info().unwrap().size == [32; 2]),
        "newly visible faces retain the regional baseline during refinement"
    );
}

#[test]
fn larger_visible_objects_build_first() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 100.0, 1);
    add(&mut world, 2, 2, ParentUpdate::Root, 10.0, 2);
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.camera(camera(0.0));
    let mut first = None;
    for _ in 0..2000 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete(job, result, now);
            }
            ReconcileStep::Publish(batch) => {
                view.apply(batch).unwrap();
                first = view.objects().find(|object| object.mesh().is_some()).map(|object| object.detail().summary.id);
                if first.is_some() {
                    break;
                }
            }
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            _ => panic!("scene stopped before publishing geometry"),
        }
    }
    assert_eq!(first, Some(Uuid::from_u128(2)), "large on-screen geometry must overtake older, smaller requests");
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(view.objects().all(|object| object.mesh().is_some()), "smaller requests still complete");
}

#[test]
fn view_bounds_control_texture_priority() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 20.0, 1);
    let mut patch = ObjectPatch::new(source(1, 2), NonZeroU32::new(1).unwrap());
    patch.pose = Some(Pose { position: Some(Vec3::new(20.0, 30.0, 0.0)), rotation: None });
    patch.surface_appearance =
        Some(SurfaceAppearance::new(FaceAppearance { texture_id: Uuid::from_u128(101), ..FaceAppearance::default() }, Vec::new()).unwrap());
    let _ = world.patch_object(patch).unwrap();
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.camera(camera(0.0));
    let first = settle_materials(&mut scene, &mut view, &world, &mut now);
    let baseline = first.requested.last().unwrap();
    assert_eq!(baseline.priority, Priority::Background, "off-screen objects cannot compete with visible textures");
    assert_eq!(baseline.request, Request::Texture { id: AssetId::new(Uuid::from_u128(101)), size: TextureSize::new(32) });
    scene.camera(Camera::new(DVec3::ZERO, Vec3::X, std::f64::consts::FRAC_PI_2, [2000, 1000]).unwrap());
    let wide = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(wide.released.contains(&baseline.ticket));
    assert_eq!(wide.requested.last().unwrap().priority, Priority::Visible, "viewport changes refresh the actual visible demand");
    scene.camera(camera(0.0));
    let narrow = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(narrow.requested.last().unwrap().priority, Priority::Background, "leaving the view preserves baseline coverage");
}

#[test]
fn shared_textures_follow_camera() {
    let mut world = world();
    for (id, x) in [(1, 10.0), (2, 100.0)] {
        add(&mut world, id, id as u32, ParentUpdate::Root, x, id as u64 * 2);
        let mut patch = ObjectPatch::new(source(1, id as u64 * 2 + 1), NonZeroU32::new(id as u32).unwrap());
        patch.surface_appearance = Some(
            SurfaceAppearance::new(FaceAppearance { texture_id: Uuid::from_u128(100), ..FaceAppearance::default() }, Vec::new()).unwrap(),
        );
        let _ = world.patch_object(patch).unwrap();
    }
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.camera(camera(0.0));
    let first = settle_materials(&mut scene, &mut view, &world, &mut now).requested[0];
    assert_eq!(first.request, Request::Texture { id: AssetId::new(Uuid::from_u128(100)), size: TextureSize::new(128) });
    let ready = prepared_asset(first.request, include_bytes!("../../assets/tests/fixtures/gray-4096.j2c").to_vec());
    scene.complete_asset(first.ticket, ready.clone());
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(
        view.objects().all(|obj| obj
            .materials()
            .unwrap()
            .iter()
            .all(|face| face.base_color.as_ref().unwrap().info().unwrap().size == [128; 2]))
    );
    let _ = world.remove_object(source(1, 10), NonZeroU32::new(1).unwrap()).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.released, [first.ticket]);
    assert_eq!(work.requested.len(), 1, "removing the nearest user lowers shared demand");
    let low = work.requested[0];
    assert_eq!(low.request, Request::Texture { id: AssetId::new(Uuid::from_u128(100)), size: TextureSize::new(32) });
    scene.complete_asset(low.ticket, assets::Status::Decoding);
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.objects().next().unwrap().materials().unwrap()[0].base_color.as_ref().unwrap().info().unwrap().size, [128; 2]);
    let version = view.version();
    scene.complete_asset(low.ticket, prepared_asset(low.request, include_bytes!("../../assets/tests/fixtures/gray-4096.j2c").to_vec()));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_ne!(view.version(), version, "a new discard level invalidates prepared rendering");
    assert_eq!(view.objects().next().unwrap().materials().unwrap()[0].base_color.as_ref().unwrap().info().unwrap().size, [32; 2]);
    scene.complete_asset(first.ticket, ready);
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(
        view.objects().next().unwrap().materials().unwrap()[0].base_color.as_ref().unwrap().info().unwrap().size,
        [32; 2],
        "stale quality cannot replace current imagery"
    );
    scene.camera(camera(99.0));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.requested.len(), 1);
    let Request::Texture { size, .. } = work.requested[0].request else { panic!("expected texture demand") };
    assert!(size.edge() >= 1024, "approaching a face requests more detail");
    let version = view.version();
    let request = work.requested[0];
    scene.complete_asset(
        request.ticket,
        prepared_asset(request.request, include_bytes!("../../assets/tests/fixtures/gray-4096.j2c").to_vec()),
    );
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_ne!(view.version(), version, "ready-to-ready image replacement must publish its new detail");
    assert_eq!(view.objects().next().unwrap().materials().unwrap()[0].base_color.as_ref().unwrap().info().unwrap().size, [size.edge(); 2]);
}

#[test]
fn material_arrival_preserves_geometry() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 10.0, 1);
    let id = AssetId::new(Uuid::from_u128(9));
    let mut patch = ObjectPatch::new(source(1, 2), NonZeroU32::new(1).unwrap());
    patch.surface_appearance = Some(
        SurfaceAppearance::new(
            FaceAppearance { texture_id: Uuid::from_u128(6), color: [255, 0, 0, 128], ..FaceAppearance::default() },
            Vec::new(),
        )
        .unwrap(),
    );
    patch.render_materials = Some(RenderMaterials::new(vec![(0, id), (1, id)]).unwrap());
    patch.gltf_material_overrides = Some(
        GltfMaterialOverrides::new(vec![
            (
                0,
                GltfMaterialOverride {
                    textures: [Some(Uuid::max()), None, None, None],
                    base_color: Some([0.6, 0.7, 0.8, 1.0]),
                    alpha_mode: Some(GltfAlphaMode::Opaque),
                    double_sided: Some(false),
                    ..GltfMaterialOverride::default()
                },
            ),
            (1, GltfMaterialOverride { base_color: Some([1.0; 4]), ..GltfMaterialOverride::default() }),
        ])
        .unwrap(),
    );
    let _ = world.patch_object(patch).unwrap();
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.camera(camera(0.0));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.requested.len(), 2, "two faces share one material request and one fallback texture request");
    let request = work.requested.iter().find(|request| request.request.resource().kind == assets::Kind::PbrMaterial).unwrap();
    let object = view.objects().next().unwrap();
    let mesh = object.mesh().unwrap().1.id();
    let initial = &object.materials().unwrap()[0];
    assert_eq!(initial.pbr_state.unwrap().resolution, Resolution::Pending);
    assert_eq!(initial.base_color(), [1.0, 0.0, 0.0, 128.0 / 255.0]);
    assert_eq!(initial.alpha, Alpha::Blend);
    let version = view.version();
    let revision = view.revision();
    let ready = material_asset(id);
    scene.complete_asset(request.ticket, ready.clone());
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!((work.reads, work.builds), (0, 0), "material completion must not rehydrate or rebuild geometry");
    assert_ne!(view.version(), version, "material publications invalidate prepared rendering");
    assert_eq!(view.revision(), revision, "World revision alone cannot identify Scene content");
    assert_eq!(work.requested.len(), 1, "only the visible base-color image is fetched at this shading stage");
    let image_request = work.requested[0];
    assert_eq!(image_request.request.resource(), assets::Resource { id: AssetId::new(Uuid::from_u128(100)), kind: assets::Kind::Texture });
    let object = view.objects().next().unwrap();
    assert_eq!(object.mesh().unwrap().1.id(), mesh);
    let faces = object.materials().unwrap();
    assert_eq!(faces[0].base_color(), [0.6, 0.7, 0.8, 1.0]);
    assert_eq!(faces[0].base_color_texture(), assets::TextureRef::Clear);
    assert_eq!(faces[0].alpha, Alpha::Opaque);
    assert!(!faces[0].double_sided());
    assert_eq!(faces[1].base_color(), [1.0; 4]);
    assert_eq!(faces[1].alpha, Alpha::Mask(0.4));
    assert!(faces[1].double_sided());
    let assets::Status::Ready(assets::Content::Material(base)) = ready else { panic!("expected material") };
    let assets::MaterialDescription::Pbr(base) = *base else { panic!("expected PBR") };
    assert_eq!(base.base_color_factor, [0.2, 0.3, 0.4, 0.5]);
    let assets::MaterialDescription::Pbr(effective) = faces[1].material else { panic!("expected PBR") };
    assert_eq!(effective.normal, base.normal, "unshaded channels remain intact");
    scene.complete_asset(
        image_request.ticket,
        prepared_asset(image_request.request, include_bytes!("../../assets/tests/fixtures/rgba.j2c").to_vec()),
    );
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!((work.reads, work.builds), (0, 0));
    let face = &view.objects().next().unwrap().materials().unwrap()[1];
    assert!(face.base_color.is_some());
    assert_eq!(face.base_color_state.unwrap().resolution, Resolution::Ready);
}

#[test]
fn binding_edits_reject_old_completion() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 10.0, 1);
    let mut patch = ObjectPatch::new(source(1, 2), NonZeroU32::new(1).unwrap());
    let old = AssetId::new(Uuid::from_u128(9));
    patch.render_materials = Some(RenderMaterials::new(vec![(0, old)]).unwrap());
    let _ = world.patch_object(patch).unwrap();
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.camera(camera(0.0));
    let first = settle_materials(&mut scene, &mut view, &world, &mut now).requested[0];
    scene.complete_asset(first.ticket, material_asset(old));
    let image = settle_materials(&mut scene, &mut view, &world, &mut now).requested[0];
    assert!(matches!(view.objects().next().unwrap().materials().unwrap()[0].material, assets::MaterialDescription::Pbr(_)));
    let current = AssetId::new(Uuid::from_u128(10));
    let mut patch = ObjectPatch::new(source(1, 3), NonZeroU32::new(1).unwrap());
    patch.render_materials = Some(RenderMaterials::new(vec![(0, current)]).unwrap());
    let _ = world.patch_object(patch).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.released.len(), 2);
    assert!(work.released.contains(&first.ticket));
    assert!(work.released.contains(&image.ticket));
    assert_eq!(work.builds, 0);
    let next = work.requested[0];
    scene.complete_asset(first.ticket, material_asset(old));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!((work.reads, work.builds, work.requested.len()), (0, 0, 0));
    let face = &view.objects().next().unwrap().materials().unwrap()[0];
    assert!(matches!(face.material, assets::MaterialDescription::Legacy(_)));
    assert_eq!(face.pbr_state.unwrap().asset, current);
    assert_eq!(face.pbr_state.unwrap().resolution, Resolution::Pending);
    scene.complete_asset(next.ticket, assets::Status::Failed { failure: assets::Failure::NotFound, retry_at: None });
    settle_materials(&mut scene, &mut view, &world, &mut now);
    let face = &view.objects().next().unwrap().materials().unwrap()[0];
    assert_eq!(face.pbr_state.unwrap().resolution, Resolution::Failed(assets::Failure::NotFound));
    assert!(matches!(face.material, assets::MaterialDescription::Legacy(_)));
    scene.close();
    scene.complete_asset(next.ticket, material_asset(current));
    assert!(matches!(scene.poll(now), ReconcileStep::Closed));
}

#[test]
fn shared_demand_releases_last_user() {
    let mut world = world();
    for id in 1..=2 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 10.0, id as u64 * 2);
        let mut patch = ObjectPatch::new(source(1, id as u64 * 2 + 1), NonZeroU32::new(id as u32).unwrap());
        patch.surface_appearance = Some(
            SurfaceAppearance::new(FaceAppearance { texture_id: Uuid::from_u128(100), ..FaceAppearance::default() }, Vec::new()).unwrap(),
        );
        let _ = world.patch_object(patch).unwrap();
    }
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.camera(camera(0.0));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.requested.len(), 1);
    let ticket = work.requested[0].ticket;
    for local in 1..=2 {
        let _ = world.remove_object(source(1, 10 + u64::from(local)), NonZeroU32::new(local).unwrap()).unwrap();
        scene.notify_world_change();
        let work = settle_materials(&mut scene, &mut view, &world, &mut now);
        assert!(work.requested.is_empty());
        if local == 1 {
            assert!(work.released.is_empty());
        } else {
            assert_eq!(work.released, vec![ticket]);
        }
    }
    assert_eq!(view.object_count(), 0);
}

#[test]
fn regional_demands_have_no_cache_cap() {
    let mut world = world();
    for id in 1..=700 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 10.0, id as u64 * 2);
        let mut patch = ObjectPatch::new(source(1, id as u64 * 2 + 1), NonZeroU32::new(id as u32).unwrap());
        patch.surface_appearance = Some(
            SurfaceAppearance::new(
                FaceAppearance { texture_id: Uuid::from_u128(10_000 + id * 6), ..FaceAppearance::default() },
                (1..6)
                    .map(|slot| {
                        (
                            slot,
                            FaceAppearance { texture_id: Uuid::from_u128(10_000 + id * 6 + u128::from(slot)), ..FaceAppearance::default() },
                        )
                    })
                    .collect(),
            )
            .unwrap(),
        );
        let _ = world.patch_object(patch).unwrap();
    }
    let budget = Budget::default();
    let (mut scene, mut view) = SceneReconciler::new(budget.clone());
    let mut now = Instant::now();
    scene.camera(camera(0.0));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let active: Vec<_> = work.requested.iter().filter(|request| !work.released.contains(&request.ticket)).collect();
    assert_eq!(active.len(), 4200, "all observed faces receive demand without a 4096-interest cache cutoff");
    assert!(
        view.objects()
            .flat_map(|object| object.materials().unwrap())
            .all(|face| face.base_color_state.unwrap().resolution != Resolution::Capacity)
    );
    assert!(budget.charged_bytes() <= scene::MEMORY_BYTES);
    let removed = *active[0];
    let local = ((removed.request.resource().id.uuid().as_u128() - 10_000) / 6) as u32;
    let _ = world.remove_object(source(1, 10_000), NonZeroU32::new(local).unwrap()).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.released.len(), 6, "retiring an object releases its six unique texture interests");
    assert!(work.released.contains(&removed.ticket));
    assert!(work.requested.is_empty());
}

#[test]
fn shared_texture_outlives_scene() {
    let mut world = world();
    for id in 1..=25 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 10.0, id as u64 * 2);
        let mut patch = ObjectPatch::new(source(1, id as u64 * 2 + 1), NonZeroU32::new(id as u32).unwrap());
        patch.surface_appearance = Some(
            SurfaceAppearance::new(FaceAppearance { texture_id: Uuid::from_u128(101), ..FaceAppearance::default() }, Vec::new()).unwrap(),
        );
        let _ = world.patch_object(patch).unwrap();
    }
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.camera(camera(0.0));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.requested.len(), 1, "faces share one texture demand");
    let ready = prepared_asset(
        Request::new(work.requested[0].request.resource()),
        include_bytes!("../../assets/tests/fixtures/gray-4096.j2c").to_vec(),
    );
    scene.complete_asset(work.requested[0].ticket, ready);
    let completed = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(completed.released.is_empty());
    let held = view.objects().next().unwrap().materials().unwrap()[0].base_color.clone().unwrap();
    assert!(view.objects().all(|object| object.materials().unwrap()[0].base_color.as_ref().unwrap().id() == held.id()));
    let weak = held.downgrade();
    scene.close();
    drop(scene);
    drop(view);
    assert!(weak.is_alive(), "the explicit consumer retains ownership after scene shutdown");
    drop(held);
    assert!(!weak.is_alive(), "no scene cache retains a hidden strong handle");
}

fn positions(view: &SceneView) -> BTreeMap<u128, f64> {
    view.objects()
        .map(|obj| {
            let Placement::Ready(transform) = obj.placement() else {
                panic!("expected resolved object: {:?}", obj.detail().summary.id);
            };
            (obj.detail().summary.id.as_u128(), transform.pose.position.x)
        })
        .collect()
}

fn camera(x: f64) -> Camera {
    Camera::new(DVec3::new(x, 0.0, 0.0), Vec3::X, std::f64::consts::FRAC_PI_2, [1000, 1000]).unwrap()
}

#[test]
fn mesh_reuse_and_replacement() {
    let mut world = world();
    for id in 1..=2 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 100.0, id as u64);
    }
    let budget = Budget::default();
    let (mut scene, mut view) = SceneReconciler::new(budget.clone());
    let mut now = Instant::now();
    scene.camera(camera(-300.0));
    settle(&mut scene, &mut view, &world, &mut now);
    let meshes: Vec<_> = view.objects().map(|object| object.mesh().map(|(key, mesh)| (key, mesh.clone())).unwrap()).collect();
    assert_eq!(meshes[0].0.detail, Detail::Lowest);
    assert_eq!(meshes[0].1.id(), meshes[1].1.id());

    scene.camera(camera(99.0));
    let job = loop {
        match scene.poll(now) {
            ReconcileStep::Publish(batch) => {
                view.apply(batch).unwrap();
                assert!(view.objects().all(|object| object.mesh().unwrap().0.detail == Detail::Lowest));
            }
            ReconcileStep::Build(job) => break job,
            ReconcileStep::Continue => {}
            _ => panic!("expected new detail work"),
        }
    };
    scene.complete_mesh(job.run());
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(view.objects().all(|object| {
        let (key, mesh) = object.mesh().unwrap();
        key.detail == Detail::High && mesh.id() != meshes[0].1.id()
    }));
    let mut patch = ObjectPatch::new(source(1, 3), NonZeroU32::new(1).unwrap());
    patch.shape = FieldUpdate::Set(Shape::Prim(world::PrimShape { profile_curve: 15, ..Default::default() }));
    let _ = world.patch_object(patch).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let changed = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(1)).unwrap();
    assert!(changed.mesh().is_none());
    assert!(matches!(changed.mesh_state(), Some((_, PreparedMesh::Failed(scene::MeshError::UnsupportedCurve)))));
    scene.close();
    drop(scene);
    drop(view);
    assert!(budget.charged_bytes() > 0);
    drop(meshes);
    assert_eq!(budget.charged_bytes(), 0);
}

#[test]
fn mesh_progress_during_read() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 100.0, 1);
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let revision = view.revision();
    move_root(&mut world, 150.0, 2);
    scene.notify_world_change();
    let ReconcileStep::Read(held) = scene.poll(now) else {
        panic!("expected World read");
    };
    scene.camera(camera(99.0));
    let mut built = 0;
    for _ in 0..20 {
        match scene.poll(now) {
            ReconcileStep::Build(job) => {
                built += 1;
                scene.complete_mesh(job.run());
            }
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(_) => break,
            _ => panic!("mesh work must not start another World read"),
        }
    }
    assert_eq!(built, 1);
    assert!(view.objects().next().unwrap().mesh().is_some());
    assert_eq!(positions(&view)[&1], 100.0);
    assert_eq!(view.revision(), revision);
    let result = reply(&world, &held);
    scene.complete(held, result, now);
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(positions(&view)[&1], 150.0);
    assert_eq!(view.revision(), Some(world.revision()));
    assert!(view.objects().next().unwrap().mesh().is_some());

    let acknowledged = view.revision();
    move_root(&mut world, 175.0, 3);
    scene.notify_world_change();
    let ReconcileStep::Read(changes) = scene.poll(now) else {
        panic!("expected journal read");
    };
    let result = reply(&world, &changes);
    scene.complete(changes, result, now);
    let ReconcileStep::Read(details) = scene.poll(now) else {
        panic!("expected hydration");
    };
    scene.complete(details, Err(ReadFailure::Busy), now);
    let ReconcileStep::Publish(status) = scene.poll(now) else {
        panic!("expected recovery status");
    };
    view.apply(status).unwrap();
    assert_eq!(view.revision(), acknowledged, "status-only recovery cannot acknowledge unpublished World changes");
    assert_eq!(positions(&view)[&1], 150.0);
}

#[test]
fn camera_updates_preserve_progress() {
    let mut world = world();
    for id in 1..=300 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 100.0, id as u64);
    }
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    for turn in 0..30 {
        scene.camera(camera(-300.0 - f64::from(turn)));
        match scene.poll(now) {
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Continue | ReconcileStep::Idle => {}
            ReconcileStep::Wait(until) => now = until,
            _ => panic!("no World invalidation expected"),
        }
    }
    assert_eq!(view.objects().filter(|object| object.mesh().is_some()).count(), 300);
    settle(&mut scene, &mut view, &world, &mut now);
}

#[test]
fn mesh_admission_recovery() {
    let mut world = world();
    for id in 1..=1025 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 100.0, id as u64 * 2);
        let mut patch = ObjectPatch::new(source(1, id as u64 * 2 + 1), NonZeroU32::new(id as u32).unwrap());
        patch.shape = FieldUpdate::Set(Shape::Prim(world::PrimShape { profile_begin: id as u16, ..Default::default() }));
        let _ = world.patch_object(patch).unwrap();
    }
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.camera(camera(-300.0));
    for _ in 0..30_000 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete(job, result, now);
            }
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            _ => panic!("expected bounded preparation work"),
        }
        if view.objects().filter(|object| object.mesh().is_some()).count() == 1024
            && view.revision() == Some(world.revision())
            && view.objects().any(|object| matches!(object.mesh_state(), Some((_, PreparedMesh::Capacity))))
            && view.objects().all(|object| object.mesh().is_none() || object.materials().is_some())
        {
            break;
        }
    }
    assert_eq!(view.objects().filter(|object| matches!(object.mesh_state(), Some((_, PreparedMesh::Capacity)))).count(), 1);
    for _ in 0..32 {
        match scene.poll(now) {
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            _ => panic!("unchanged capacity cannot republish or regenerate"),
        }
    }
    let _ = world.remove_object(source(1, 10_000), NonZeroU32::new(1).unwrap()).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.object_count(), 1024);
    assert!(view.objects().all(|object| object.mesh().is_some()));
}

#[test]
fn quality_updates() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 100.0, 1);
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.camera(camera(0.0));
    settle(&mut scene, &mut view, &world, &mut now);
    let revision = view.revision();
    assert_eq!(view.objects().next().unwrap().mesh().unwrap().0.detail, Detail::Low);

    scene.camera(camera(0.0).with_quality(32.0, 1.0).unwrap());
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(view.objects().next().unwrap().mesh().is_none());

    scene.camera(camera(0.0).with_quality(512.0, 4.0).unwrap());
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.objects().next().unwrap().mesh().unwrap().0.detail, Detail::Medium);
    scene.camera(camera(0.0).with_quality(512.0, 0.25).unwrap());
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.objects().next().unwrap().mesh().unwrap().0.detail, Detail::Lowest);
    assert_eq!(view.revision(), revision);
}

#[test]
fn bootstrap_identity_changes() {
    let mut world = world();
    for id in 2..302 {
        add(&mut world, id, id as u32, ParentUpdate::Root, id as f32, id as u64);
    }
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    loop {
        let job = match scene.poll(now) {
            ReconcileStep::Read(job) => job,
            ReconcileStep::Publish(batch) => {
                view.apply(batch).unwrap();
                continue;
            }
            ReconcileStep::Continue => continue,
            _ => panic!("expected bootstrap progress"),
        };
        let objects = matches!(job.query(), ReadQuery::Objects(None));
        let result = reply(&world, &job);
        scene.complete(job, result, now);
        if objects {
            break;
        }
    }
    add(&mut world, 1, 1000, ParentUpdate::Root, 1.0, 1000);
    add(&mut world, 900, 50, ParentUpdate::Root, 900.0, 1001);
    settle(&mut scene, &mut view, &world, &mut now);
    let mut expected: BTreeSet<_> = (2..302).collect();
    expected.remove(&50);
    expected.insert(1);
    expected.insert(900);
    assert_eq!(ids(&view), expected);
    assert_eq!(view.revision(), Some(world.revision()));
    assert_eq!(view.status(), Status::Ready);
}

#[test]
fn descendant_pose_updates() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 10.0, 1);
    add(&mut world, 2, 2, ParentUpdate::Local(NonZeroU32::new(1).unwrap()), 2.0, 2);
    add(&mut world, 3, 3, ParentUpdate::Local(NonZeroU32::new(2).unwrap()), 3.0, 3);
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(positions(&view), BTreeMap::from([(1, 10.0), (2, 12.0), (3, 15.0)]));
    move_root(&mut world, 20.0, 4);
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(positions(&view), BTreeMap::from([(1, 20.0), (2, 22.0), (3, 25.0)]));
    let mut patch = ObjectPatch::new(source(1, 5), NonZeroU32::new(2).unwrap());
    patch.parent = Some(ParentUpdate::Root);
    patch.pose = Some(Pose { position: Some(Vec3::new(7.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
    let _ = world.patch_object(patch).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(positions(&view), BTreeMap::from([(1, 20.0), (2, 7.0), (3, 10.0)]));
    let _ = world.adopt_region(Region::new(2)).unwrap();
    let mut transfer =
        ObjectObservation::new(source(2, 1), Uuid::from_u128(2), NonZeroU32::new(9).unwrap(), Kind::Object, ParentUpdate::Root);
    transfer.pose = Some(Pose { position: Some(Vec3::new(8.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
    transfer.scale = Some(Vec3::ONE);
    let _ = world.observe_object(transfer).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    for obj in view.objects().filter(|obj| obj.detail().summary.id != Uuid::from_u128(1)) {
        assert!(matches!(obj.placement(), Placement::Hidden(_)));
    }
    let _ = world.patch_region(Region::new(2), RegionInfo { origin: Some(DVec3::new(100.0, 0.0, 0.0)), ..RegionInfo::default() }).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let root = view.objects().find(|obj| obj.detail().summary.id == Uuid::from_u128(2)).unwrap();
    assert!(matches!(root.placement(), Placement::Ready(transform) if transform.pose.position.x == 108.0));
    let child = view.objects().find(|obj| obj.detail().summary.id == Uuid::from_u128(3)).unwrap();
    assert!(matches!(child.placement(), Placement::Hidden(_)));
    let mut child = ObjectObservation::new(
        source(2, 2),
        Uuid::from_u128(3),
        NonZeroU32::new(10).unwrap(),
        Kind::Object,
        ParentUpdate::Local(NonZeroU32::new(9).unwrap()),
    );
    child.pose = Some(Pose { position: Some(Vec3::new(3.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
    child.scale = Some(Vec3::ONE);
    let _ = world.observe_object(child).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(positions(&view), BTreeMap::from([(1, 20.0), (2, 108.0), (3, 111.0)]));
}

#[test]
fn pending_parent_arrival() {
    let mut world = world();
    add(&mut world, 2, 2, ParentUpdate::Local(NonZeroU32::new(1).unwrap()), 2.0, 1);
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(matches!(view.objects().next().unwrap().placement(), Placement::Hidden(_)));
    add(&mut world, 1, 1, ParentUpdate::Root, 10.0, 2);
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(positions(&view), BTreeMap::from([(1, 10.0), (2, 12.0)]));
}

#[test]
fn own_signed_dimensions() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 10.0, 1);
    add(&mut world, 2, 2, ParentUpdate::Local(NonZeroU32::new(1).unwrap()), 2.0, 2);
    let mut root = ObjectPatch::new(source(1, 3), NonZeroU32::new(1).unwrap());
    root.scale = Some(Vec3::new(100.0, 200.0, 300.0));
    let _ = world.patch_object(root).unwrap();
    let mut child = ObjectPatch::new(source(1, 4), NonZeroU32::new(2).unwrap());
    child.scale = Some(Vec3::new(-2.0, 3.0, 4.0));
    let _ = world.patch_object(child).unwrap();
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let child = view.objects().find(|obj| obj.detail().summary.id == Uuid::from_u128(2)).unwrap();
    let Placement::Ready(transform) = child.placement() else {
        panic!("signed dimensions are retained by World");
    };
    assert_eq!(transform.pose.position.x, 12.0);
    assert_eq!(transform.scale, Vec3::new(-2.0, 3.0, 4.0));
    let mut zero = ObjectPatch::new(source(1, 5), NonZeroU32::new(2).unwrap());
    zero.scale = Some(Vec3::new(0.0, 3.0, 4.0));
    let _ = world.patch_object(zero).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let child = view.objects().find(|obj| obj.detail().summary.id == Uuid::from_u128(2)).unwrap();
    assert_eq!(child.placement(), Placement::Hidden(scene::HiddenReason::InvalidDimensions));
}

#[test]
fn atomic_region_retirement() {
    let mut world = world();
    for id in 1..301 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 0.0, id as u64);
    }
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let _ = world.retire_region(Region::new(1)).unwrap();
    scene.notify_world_change();
    let mut partial = false;
    loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                if let Ok(ReadReply::Changes(page)) = &result {
                    partial |= page.records.last().is_some_and(|record| record.revision > page.completed_revision);
                }
                scene.complete(job, result, now);
                assert_eq!(view.objects().count(), 300);
            }
            ReconcileStep::Publish(batch) => {
                view.apply(batch).unwrap();
                break;
            }
            ReconcileStep::Continue => {}
            _ => panic!("expected retirement continuation"),
        }
    }
    assert!(partial);
    assert_eq!(view.objects().count(), 0);
    assert_eq!(view.regions().count(), 0);
    assert_eq!(view.revision(), Some(world.revision()));
}

#[test]
fn history_recovery_backoff() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 10.0, 1);
    let budget = Budget::default();
    let (mut scene, mut view) = SceneReconciler::new(budget.clone());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let retained = budget.charged_bytes();
    assert!(retained > 0);
    for attempt in 1..=4 {
        scene.notify_world_change();
        let ReconcileStep::Read(job) = scene.poll(now) else {
            panic!("expected recovery read");
        };
        scene.complete(job, Err(ReadFailure::World(WorldReadError::ResyncRequired)), now);
        let ReconcileStep::Publish(batch) = scene.poll(now) else {
            panic!("expected recovery status");
        };
        view.apply(batch).unwrap();
        assert_eq!(positions(&view), BTreeMap::from([(1, 10.0)]));
        assert_eq!(budget.charged_bytes(), retained);
        let expected = if attempt <= 3 {
            Status::Recovering { reason: RecoveryReason::HistoryLost, attempt }
        } else {
            Status::Degraded { reason: RecoveryReason::HistoryLost }
        };
        assert_eq!(view.status(), expected);
        let ReconcileStep::Wait(until) = scene.poll(now) else {
            panic!("expected backoff");
        };
        assert_eq!(until.duration_since(now), Duration::from_secs(if attempt <= 3 { 1 << (attempt - 1) } else { 30 }));
        now = until;
    }
    move_root(&mut world, 20.0, 2);
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(positions(&view), BTreeMap::from([(1, 20.0)]));
    assert_eq!(view.status(), Status::Ready);
}

#[test]
fn publication_backpressure_and_close() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 10.0, 1);
    let budget = Budget::default();
    let (mut scene, mut view) = SceneReconciler::new(budget.clone());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    move_root(&mut world, 20.0, 2);
    scene.notify_world_change();
    let batch = loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete(job, result, now);
            }
            ReconcileStep::Publish(batch) => break batch,
            ReconcileStep::Continue => {}
            _ => panic!("expected bootstrap"),
        }
    };
    let held = budget.charged_bytes();
    for _ in 0..1000 {
        assert!(matches!(scene.poll(now), ReconcileStep::Wait(_)));
    }
    assert_eq!(budget.charged_bytes(), held);
    view.apply(batch).unwrap();
    let retained = view.objects().next().unwrap().clone();
    scene.notify_world_change();
    let ReconcileStep::Read(job) = scene.poll(now) else {
        panic!("expected journal read");
    };
    let result = reply(&world, &job);
    scene.close();
    scene.complete(job, result, now);
    assert!(matches!(scene.poll(now), ReconcileStep::Closed));
    assert_eq!(view.objects().count(), 0);
    assert_eq!(view.status(), Status::Closed);
    drop(scene);
    drop(view);
    assert!(budget.charged_bytes() > 0);
    drop(retained);
    assert_eq!(budget.charged_bytes(), 0);
}

#[test]
fn hydration_identity_conflicts() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 1.0, 1);
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    add(&mut world, 2, 2, ParentUpdate::Root, 2.0, 2);
    scene.notify_world_change();
    let ReconcileStep::Read(job) = scene.poll(now) else {
        panic!("expected journal read");
    };
    let old_page = reply(&world, &job);
    add(&mut world, 2, 1, ParentUpdate::Root, 3.0, 3);
    scene.complete(job, old_page, now);
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(ids(&view), BTreeSet::from([2]));
    assert_eq!(positions(&view), BTreeMap::from([(2, 3.0)]));
    assert!(view.revision().unwrap() < world.revision());
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(ids(&view), BTreeSet::from([2]));
}

#[test]
fn abandoned_publication_recovery() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 1.0, 1);
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    move_root(&mut world, 2.0, 2);
    scene.notify_world_change();
    loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete(job, result, now);
            }
            ReconcileStep::Publish(batch) => {
                drop(batch);
                break;
            }
            ReconcileStep::Continue => {}
            _ => panic!("expected publication"),
        }
    }
    let ReconcileStep::Publish(status) = scene.poll(now) else {
        panic!("expected abandoned delivery recovery");
    };
    view.apply(status).unwrap();
    assert_eq!(positions(&view), BTreeMap::from([(1, 1.0)]));
    assert!(matches!(view.status(), Status::Recovering { reason: RecoveryReason::InvalidReply, .. }));
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(positions(&view), BTreeMap::from([(1, 2.0)]));
}

#[test]
fn terrain_and_water_lifetimes() {
    let mut world = world();
    let first = world::TerrainPatch::new(0, 0, Box::new([7.0; 256])).unwrap();
    let _ = world.observe_terrain_patch(source(1, 1), first).unwrap();
    let _ = world.observe_water_height(source(1, 2), 0.0).unwrap();
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.patch_count(), 1);
    assert_eq!(view.water_height(Region::new(1)), Some(0.0));
    assert_eq!(view.patches().next().unwrap().1.source().heights(), &[7.0; 256]);
    let updated = world::TerrainPatch::new(0, 0, Box::new([8.0; 256])).unwrap();
    let _ = world.observe_terrain_patch(source(1, 3), updated).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.patch_count(), 1);
    assert_eq!(view.patches().next().unwrap().1.source().heights(), &[8.0; 256]);
    let _ = world.retire_region(Region::new(1)).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.region_count(), 0);
    assert_eq!(view.patch_count(), 0);
    assert_eq!(view.water_height(Region::new(1)), None);
    assert_eq!(view.surfaces().count(), 0);
}

#[test]
fn terrain_loading_keeps_history() {
    let mut world = world_with_journal(DVec3::ZERO, 8);
    add(&mut world, 1, 1, ParentUpdate::Root, 0.0, 1);
    for y in 0..16 {
        for x in 0..16 {
            let patch = TerrainPatch::new(x, y, Box::new([1.0; 256])).unwrap();
            let _ = world.observe_terrain_patch(source(1, 2 + u64::from(y) * 16 + u64::from(x)), patch).unwrap();
        }
    }
    let _ = world.observe_water_height(source(1, 300), 20.0).unwrap();
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    let mut terrain_reads = 0;
    for _ in 0..10_000 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                if matches!(job.query(), ReadQuery::Terrain(..)) {
                    terrain_reads += 1;
                    move_root(&mut world, terrain_reads as f32, 1000 + terrain_reads);
                    scene.notify_world_change();
                }
                scene.complete(job, result, now);
            }
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            ReconcileStep::Idle => break,
            _ => panic!("unexpected terrain loading state"),
        }
        assert!(
            !matches!(scene.status(), Status::Recovering { .. } | Status::Degraded { .. }),
            "terrain paging must not starve journal reads"
        );
    }
    assert_eq!(terrain_reads, 32);
    assert_eq!(view.patch_count(), 256);
    assert_eq!(view.water_height(Region::new(1)), Some(20.0));
    assert_eq!(view.revision(), Some(world.revision()));
    assert_eq!(positions(&view), BTreeMap::from([(1, 32.0)]));
}

#[test]
fn terrain_prepares_during_publication() {
    let mut world = world();
    for x in 0..8 {
        let patch = TerrainPatch::new(x, 0, Box::new([1.0; 256])).unwrap();
        let _ = world.observe_terrain_patch(source(1, u64::from(x) + 1), patch).unwrap();
    }
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    for x in 0..8 {
        let patch = TerrainPatch::new(x, 0, Box::new([2.0; 256])).unwrap();
        let _ = world.observe_terrain_patch(source(1, u64::from(x) + 20), patch).unwrap();
    }
    scene.notify_world_change();
    let mut running = Vec::new();
    let held = loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete(job, result, now);
            }
            ReconcileStep::Publish(batch) => break batch,
            ReconcileStep::Build(job) => running.push(job),
            ReconcileStep::Continue => {}
            _ => panic!("expected terrain update"),
        }
    };
    for job in running {
        scene.complete_mesh(job.run());
    }
    let mut built = 0;
    for _ in 0..100 {
        match scene.poll(now) {
            ReconcileStep::Build(job) => {
                built += 1;
                scene.complete_mesh(job.run());
            }
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete(job, result, now);
            }
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(_) | ReconcileStep::Idle => break,
            ReconcileStep::Publish(_) => panic!("only one publication may await delivery"),
            _ => panic!("unexpected terrain preparation state"),
        }
    }
    assert!(built > 0, "a slow frame must not stop terrain preparation");
    assert_eq!(ground_mesh(&view, 1, 0, 0).bounds().min.z, 1.0, "undelivered work must not alter the published view");
    view.apply(held).unwrap();
    settle(&mut scene, &mut view, &world, &mut now);
    for x in 0..8 {
        assert_eq!(ground_mesh(&view, 1, x, 0).bounds().min.z, 2.0);
    }
}

#[test]
fn terrain_streams_during_bootstrap() {
    let mut world = world();
    for id in 1..=300 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 0.0, id as u64);
    }
    for x in 0..8 {
        let patch = TerrainPatch::new(x, 0, Box::new([1.0; 256])).unwrap();
        let _ = world.observe_terrain_patch(source(1, u64::from(x) + 400), patch).unwrap();
    }
    let _ = world.observe_water_height(source(1, 500), 20.0).unwrap();
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    let mut observed = false;
    for _ in 0..1000 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                if matches!(job.query(), ReadQuery::Objects(_)) && !observed {
                    observed = true;
                    assert_eq!(view.patch_count(), 8, "terrain must appear before object enumeration completes");
                    assert_eq!(view.surfaces().count(), 9);
                    assert_eq!(view.status(), Status::Loading);
                    assert_eq!(view.revision(), None, "progressive startup must not acknowledge unreconciled World changes");
                }
                let result = reply(&world, &job);
                scene.complete(job, result, now);
            }
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            ReconcileStep::Idle => break,
            _ => panic!("unexpected bootstrap state"),
        }
    }
    assert!(observed);
    assert_eq!(view.object_count(), 300);
    assert_eq!(view.revision(), Some(world.revision()));
    assert_eq!(view.status(), Status::Ready);
}

#[test]
fn bootstrap_ignores_frame_latency() {
    let mut world = world();
    for id in 1..=300 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 0.0, id as u64);
    }
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    let held = loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete(job, result, now);
            }
            ReconcileStep::Publish(batch) => break batch,
            ReconcileStep::Continue => {}
            _ => panic!("expected startup publication"),
        }
    };
    for _ in 0..1000 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete(job, result, now);
            }
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(_) => break,
            ReconcileStep::Publish(_) => panic!("a second batch cannot overtake the held batch"),
            _ => panic!("unexpected startup state"),
        }
    }
    assert_eq!(view.object_count(), 0, "pending work cannot leak into the view");
    view.apply(held).unwrap();
    let reads = settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(reads, 0, "bootstrap reads must complete independently of frame delivery");
    assert_eq!(view.object_count(), 300);
    assert_eq!(view.revision(), Some(world.revision()));
}

#[test]
fn startup_failure_retains_terrain() {
    let mut world = world();
    for id in 1..=300 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 0.0, id as u64);
    }
    let patch = TerrainPatch::new(0, 0, Box::new([1.0; 256])).unwrap();
    let _ = world.observe_terrain_patch(source(1, 400), patch).unwrap();
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) if matches!(job.query(), ReadQuery::Objects(_)) => {
                assert_eq!(ground_mesh(&view, 1, 0, 0).bounds().min.z, 1.0);
                scene.complete(job, Err(ReadFailure::Busy), now);
                break;
            }
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete(job, result, now);
            }
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            _ => panic!("expected progressive startup"),
        }
    }
    let patch = TerrainPatch::new(0, 0, Box::new([9.0; 256])).unwrap();
    let _ = world.observe_terrain_patch(source(1, 500), patch).unwrap();
    let mut checked = false;
    for _ in 0..1000 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                if matches!(job.query(), ReadQuery::Objects(_)) && !checked {
                    assert!(matches!(view.status(), Status::Recovering { .. }));
                    let surface = view
                        .surfaces()
                        .find(|surface| matches!(surface.kind, SurfaceKind::Ground(_)))
                        .expect("recovery must preserve previously published terrain");
                    let PreparedMesh::Ready(mesh) = &surface.mesh else { panic!("retained terrain must stay ready") };
                    assert_eq!(mesh.mesh().bounds().min.z, 1.0, "recovery cannot expose partially rebuilt terrain");
                    checked = true;
                }
                let result = reply(&world, &job);
                scene.complete(job, result, now);
            }
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            ReconcileStep::Idle => break,
            _ => panic!("unexpected recovery state"),
        }
    }
    assert!(checked);
    assert_eq!(view.object_count(), 300);
    assert_eq!(ground_mesh(&view, 1, 0, 0).bounds().min.z, 9.0);
    assert_eq!(view.revision(), Some(world.revision()));
    assert_eq!(view.status(), Status::Ready);
}

#[test]
fn water_precedes_ground_builds() {
    let mut world = world();
    for x in 0..8 {
        let patch = TerrainPatch::new(x, 0, Box::new([1.0; 256])).unwrap();
        let _ = world.observe_terrain_patch(source(1, u64::from(x) + 1), patch).unwrap();
    }
    let _ = world.observe_water_height(source(1, 10), 20.0).unwrap();
    let _ = world.adopt_region(Region::new(2)).unwrap();
    let _ = world.patch_region(Region::new(2), RegionInfo { origin: Some(DVec3::X * 256.0), ..RegionInfo::default() }).unwrap();
    let _ = world.observe_water_height(source(2, 1), 25.0).unwrap();
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    let mut appeared = BTreeSet::new();
    for _ in 0..1000 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete(job, result, now);
            }
            ReconcileStep::Publish(batch) => {
                view.apply(batch).unwrap();
                for surface in view.surfaces() {
                    if appeared.insert(surface.region) {
                        assert_eq!(surface.kind, SurfaceKind::Water, "observed water must not wait behind its terrain queue");
                        let PreparedMesh::Ready(mesh) = &surface.mesh else { panic!("water mesh must be ready") };
                        assert_eq!(mesh.mesh().vertices().len(), 4);
                    }
                }
                if appeared.len() == 2 {
                    assert_eq!(view.water_height(Region::new(2)), Some(25.0));
                    return;
                }
            }
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            _ => panic!("expected water build progress"),
        }
    }
    panic!("water did not become visible");
}

fn ground_mesh(view: &SceneView, region: u64, x: u8, y: u8) -> &Mesh {
    let kind = SurfaceKind::Ground(TerrainPatchId::new(x, y).unwrap());
    let surface = view.surfaces().find(|surface| surface.region == Region::new(region) && surface.kind == kind).unwrap();
    let PreparedMesh::Ready(mesh) = &surface.mesh else { panic!("expected ready terrain") };
    mesh.mesh()
}

#[test]
fn terrain_samples_and_gaps() {
    let mut world = world();
    let plane = |x: u8, y: u8| {
        TerrainPatch::new(
            x,
            y,
            Box::new(std::array::from_fn(|index| {
                7.0 + 2.0 * (f32::from(x) * 16.0 + (index % 16) as f32) + 3.0 * (f32::from(y) * 16.0 + (index / 16) as f32)
            })),
        )
        .unwrap()
    };
    let _ = world.observe_terrain_patch(source(1, 1), plane(0, 0)).unwrap();
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let mesh = ground_mesh(&view, 1, 0, 0);
    assert_eq!(mesh.vertices().len(), 256);
    assert_eq!(mesh.indices().len(), 15 * 15 * 6, "unknown neighbors must leave uncovered cells");
    assert_eq!(mesh.vertices()[0].position, Vec3::new(0.0, 0.0, 7.0));
    assert_eq!(mesh.vertices()[255].position, Vec3::new(15.0, 15.0, 82.0));
    assert!(!view.surfaces().any(|surface| surface.kind == SurfaceKind::Water));
    for (order, (x, y)) in [(1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
        let _ = world.observe_terrain_patch(source(1, order as u64 + 2), plane(x, y)).unwrap();
    }
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let mesh = ground_mesh(&view, 1, 0, 0);
    assert_eq!(mesh.vertices().len(), 289);
    assert_eq!(mesh.indices().len(), 1536);
    assert_eq!(mesh.vertices()[288].position, Vec3::new(16.0, 16.0, 87.0));
    assert_eq!(&mesh.indices()[..6], &[0, 1, 18, 0, 18, 17]);
    let normal = mesh.vertices()[8 * 17 + 8].normal;
    assert!((normal - Vec3::new(-2.0, -3.0, 1.0).normalize()).length() < 1e-6);
    for triangle in mesh.indices().as_chunks::<3>().0 {
        let [a, b, c] = triangle.map(|index| mesh.vertices()[index as usize].position);
        assert!((b - a).cross(c - a).z > 0.0);
    }
    let extreme =
        TerrainPatch::new(4, 4, Box::new(std::array::from_fn(|index| if index % 2 == 0 { f32::MAX } else { -f32::MAX }))).unwrap();
    let _ = world.observe_terrain_patch(source(1, 10), extreme).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(ground_mesh(&view, 1, 4, 4).vertices().iter().all(|vertex| vertex.position.is_finite() && vertex.normal.is_finite()));
}

#[test]
fn terrain_extent_validation() {
    for dimensions in [[0, 0], [512, 256]] {
        let mut world = world();
        let _ = world.observe_terrain_patch(source(1, 1), TerrainPatch::new(0, 0, Box::new([0.0; 256])).unwrap()).unwrap();
        let (mut scene, mut view) = SceneReconciler::new(Budget::default());
        let now = Instant::now();
        loop {
            match scene.poll(now) {
                ReconcileStep::Read(job) => {
                    let mut result = reply(&world, &job);
                    if let Ok(ReadReply::Terrain(Some(page))) = &mut result {
                        page.dimensions = dimensions;
                        scene.complete(job, result, now);
                        break;
                    }
                    scene.complete(job, result, now);
                }
                ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
                ReconcileStep::Continue => {}
                _ => panic!("expected terrain hydration"),
            }
        }
        assert!(matches!(scene.status(), Status::Recovering { reason: RecoveryReason::InvalidReply, .. }));
        assert_eq!(view.surfaces().count(), 0);
    }
}

#[test]
fn terrain_region_edges() {
    let origin = DVec3::new(1_000_000_000.0, 1_000_000_000.0, 0.0);
    let mut world = world_at(origin);
    let _ = world.adopt_region(Region::new(2)).unwrap();
    let _ = world.patch_region(Region::new(2), RegionInfo { origin: Some(origin + DVec3::X * 256.0), ..RegionInfo::default() }).unwrap();
    let _ = world.observe_terrain_patch(source(1, 1), TerrainPatch::new(15, 0, Box::new([10.0; 256])).unwrap()).unwrap();
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(ground_mesh(&view, 1, 15, 0).bounds().max.x, 15.0, "active neighbor without samples must not be clamped");
    let _ = world.observe_terrain_patch(source(2, 2), TerrainPatch::new(0, 0, Box::new([20.0; 256])).unwrap()).unwrap();
    let _ = world.observe_water_height(source(1, 3), 17.0).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let west = ground_mesh(&view, 1, 15, 0);
    let east = ground_mesh(&view, 2, 0, 0);
    assert_eq!(west.bounds().max.x, 16.0);
    let west_edge = west.vertices().iter().find(|v| v.position.x == 16.0 && v.position.y == 8.0).unwrap();
    let east_edge = east.vertices().iter().find(|v| v.position.x == 0.0 && v.position.y == 8.0).unwrap();
    assert_eq!(west_edge.position.z, 20.0);
    assert_eq!(west_edge.normal, east_edge.normal);
    let positions: BTreeMap<_, _> = view.surfaces().map(|s| ((s.region, s.kind), s.position)).collect();
    let west_position = positions[&(Region::new(1), SurfaceKind::Ground(TerrainPatchId::new(15, 0).unwrap()))];
    let east_position = positions[&(Region::new(2), SurfaceKind::Ground(TerrainPatchId::new(0, 0).unwrap()))];
    assert_eq!(west_position + west_edge.position.as_dvec3(), east_position + east_edge.position.as_dvec3());
    assert_eq!(west_position, origin + DVec3::X * 240.0);
    let water = view.surfaces().find(|s| s.kind == SurfaceKind::Water).unwrap();
    assert_eq!(water.region, Region::new(1));
    assert_eq!(water.position, origin);
    let PreparedMesh::Ready(mesh) = &water.mesh else { panic!("expected water") };
    assert_eq!(mesh.mesh().bounds().min, Vec3::new(0.0, 0.0, 17.0));
    assert_eq!(mesh.mesh().bounds().max, Vec3::new(256.0, 256.0, 17.0));
    let _ = world.retire_region(Region::new(1)).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(view.surfaces().all(|s| s.region == Region::new(2) && s.position == east_position));
}

#[test]
fn terrain_rebuild_bounds() {
    let mut world = world();
    for y in 0..16 {
        for x in 0..16 {
            let _ = world
                .observe_terrain_patch(
                    source(1, u64::from(y) * 16 + u64::from(x) + 1),
                    TerrainPatch::new(x, y, Box::new([0.0; 256])).unwrap(),
                )
                .unwrap();
        }
    }
    let budget = Budget::default();
    let (mut scene, mut view) = SceneReconciler::new(budget.clone());
    let mut now = Instant::now();
    assert_eq!(settle_work(&mut scene, &mut view, &world, &mut now).1, 256);
    let ids = |view: &SceneView| {
        view.surfaces()
            .map(|s| {
                let PreparedMesh::Ready(mesh) = &s.mesh else { panic!("expected ready mesh") };
                (s.kind, mesh.id())
            })
            .collect::<BTreeMap<_, _>>()
    };
    let baseline = ids(&view);
    for order in 300..332 {
        let _ = world.observe_terrain_patch(source(1, order), TerrainPatch::new(8, 8, Box::new([order as f32; 256])).unwrap()).unwrap();
        scene.notify_world_change();
        assert_eq!(
            settle_work(&mut scene, &mut view, &world, &mut now).1,
            9,
            "isolated patch edit must only rebuild its normal and edge dependencies"
        );
        for (kind, id) in ids(&view) {
            let SurfaceKind::Ground(patch) = kind else { unreachable!() };
            let (x, y) = patch.coordinates();
            if !(7..=9).contains(&x) || !(7..=9).contains(&y) {
                assert_eq!(id, baseline[&kind], "unaffected patches must retain geometry");
            }
        }
    }
    let before_water = ids(&view);
    let _ = world.observe_water_height(source(1, 400), 24.0).unwrap();
    scene.notify_world_change();
    assert_eq!(settle_work(&mut scene, &mut view, &world, &mut now).1, 1);
    for (kind, id) in before_water {
        assert_eq!(ids(&view)[&kind], id);
    }
    assert!(budget.high_water_bytes() < 16 * 1024 * 1024);
    eprintln!("256 terrain patches, 32 edits: 9 rebuilds per edit, peak {} bytes", budget.high_water_bytes());
    scene.close();
    drop(scene);
    drop(view);
    assert_eq!(budget.charged_bytes(), 0);
}

#[test]
fn retired_terrain_jobs() {
    let mut world = world();
    let _ = world.observe_terrain_patch(source(1, 1), TerrainPatch::new(0, 0, Box::new([f32::MAX; 256])).unwrap()).unwrap();
    let budget = Budget::default();
    let (mut scene, mut view) = SceneReconciler::new(budget.clone());
    let mut now = Instant::now();
    let job = loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete(job, result, now);
            }
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Build(job) => break job,
            ReconcileStep::Continue => {}
            _ => panic!("expected terrain work"),
        }
    };
    let _ = world.retire_region(Region::new(1)).unwrap();
    let _ = world.adopt_region(Region::new(2)).unwrap();
    let _ = world.patch_region(Region::new(2), RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).unwrap();
    let _ = world.observe_terrain_patch(source(2, 2), TerrainPatch::new(0, 0, Box::new([9.0; 256])).unwrap()).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    scene.complete_mesh(job.run());
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.surfaces().count(), 1);
    assert_eq!(ground_mesh(&view, 2, 0, 0).bounds().min.z, 9.0);
    scene.close();
    drop(scene);
    drop(view);
    assert_eq!(budget.charged_bytes(), 0);
}

#[test]
fn bounded_reconciliation() {
    let mut world = world();
    for id in 1..=2048 {
        let parent = match id {
            2 => ParentUpdate::Local(NonZeroU32::new(1).unwrap()),
            3 => ParentUpdate::Local(NonZeroU32::new(2).unwrap()),
            _ => ParentUpdate::Root,
        };
        add(&mut world, id, id as u32, parent, 1.0, id as u64);
    }
    let budget = Budget::default();
    let (mut scene, mut view) = SceneReconciler::new(budget.clone());
    let mut now = Instant::now();
    let start = Instant::now();
    let bootstrap_reads = settle(&mut scene, &mut view, &world, &mut now);
    let bootstrap_time = start.elapsed();
    assert_eq!(view.object_count(), 2048);
    assert!(bootstrap_reads <= 300);
    let unchanged = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(2048)).unwrap().clone();
    move_root(&mut world, 10.0, 3000);
    scene.notify_world_change();
    let start = Instant::now();
    let movement_reads = settle(&mut scene, &mut view, &world, &mut now);
    let movement_time = start.elapsed();
    assert!(movement_reads <= 3, "local movement should only hydrate the affected subtree");
    let after = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(2048)).unwrap();
    assert!(std::ptr::eq(unchanged.detail(), after.detail()));
    let child = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(3)).unwrap();
    assert!(matches!(child.placement(), Placement::Ready(transform) if transform.pose.position.x == 12.0));
    eprintln!(
        "2048 objects: bootstrap {:?}, {} reads; three-object movement {:?}, {} reads; admission peak {} bytes",
        bootstrap_time,
        bootstrap_reads,
        movement_time,
        movement_reads,
        budget.high_water_bytes()
    );
}

#[test]
fn recovery_memory_admission() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 1.0, 1);
    let budget = Budget::default();
    let (mut scene, mut view) = SceneReconciler::new(budget.clone());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let retained = budget.charged_bytes();
    scene.notify_world_change();
    let ReconcileStep::Read(job) = scene.poll(now) else {
        panic!("expected journal read");
    };
    scene.complete(job, Err(ReadFailure::World(WorldReadError::ResyncRequired)), now);
    let ReconcileStep::Publish(status) = scene.poll(now) else {
        panic!("expected recovery status");
    };
    view.apply(status).unwrap();
    let ReconcileStep::Wait(until) = scene.poll(now) else {
        panic!("expected backoff");
    };
    now = until;
    let ReconcileStep::Read(job) = scene.poll(now) else {
        panic!("expected new barrier");
    };
    let mut oversized = world.read_barrier();
    oversized.object_count = scene::MAX_OBJECTS + 1;
    scene.complete(job, Ok(ReadReply::Barrier(oversized)), now);
    let ReconcileStep::Publish(status) = scene.poll(now) else {
        panic!("expected capacity status");
    };
    view.apply(status).unwrap();
    assert!(matches!(view.status(), Status::Recovering { reason: RecoveryReason::Capacity, .. }));
    assert_eq!(ids(&view), BTreeSet::from([1]));
    let ReconcileStep::Wait(until) = scene.poll(now) else {
        panic!("expected backoff");
    };
    now = until;
    loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let details = matches!(job.query(), ReadQuery::Details(_));
                let result = reply(&world, &job);
                scene.complete(job, result, now);
                if details {
                    break;
                }
            }
            ReconcileStep::Continue => {}
            _ => panic!("expected staging hydration"),
        }
    }
    assert!(budget.charged_bytes() >= 2 * retained);
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(budget.charged_bytes(), retained);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn incarnation_churn(ops in prop::collection::vec((0u8..3, 1u32..17), 1..100)) {
        let mut world = world();
        let (mut scene, mut view) = SceneReconciler::new(Budget::default());
        let mut now = Instant::now();
        settle(&mut scene, &mut view, &world, &mut now);
        let mut expected = BTreeMap::new();
        for (index, (op, address)) in ops.into_iter().enumerate() {
            let order = index as u64 + 1;
            if op == 0 {
                let _ = world.remove_object(source(1, order), NonZeroU32::new(address).unwrap()).unwrap();
                expected.remove(&address);
            } else {
                let id = if op == 1 { u128::from(address) } else { 1000 + index as u128 };
                add(&mut world, id, address, ParentUpdate::Root, order as f32, order);
                expected.insert(address, id);
            }
            scene.notify_world_change(); settle(&mut scene, &mut view, &world, &mut now);
            prop_assert_eq!(ids(&view), expected.values().copied().collect());
            prop_assert_eq!(view.objects().count(), expected.len());
        }
    }
}

#[test]
fn experience_properties_keep_current_owner() {
    let world = world();
    let region = world.region_page(None, 1).unwrap().regions[0].region;
    let budget = Budget::default();
    let (mut scene, mut view) = SceneReconciler::new(budget.clone());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let a = ExperienceId::new(Uuid::from_u128(10)).unwrap();
    let b = ExperienceId::new(Uuid::from_u128(20)).unwrap();
    for (experience, name, value) in [(a, "gamma", "r1.0"), (a, "blue_density", "[r0.1,r0.2,r0.3]"), (b, "gamma", "r2.0")] {
        scene
            .push_environment(
                region,
                EnvironmentPush::Partial {
                    experience,
                    sky: vec![EnvironmentProperty { name: name.into(), source: value.as_bytes().into() }],
                    water: Vec::new(),
                    transition: Duration::ZERO,
                },
            )
            .unwrap();
        settle(&mut scene, &mut view, &world, &mut now);
    }
    scene.push_environment(region, EnvironmentPush::Clear { experience: Some(a), transition: Duration::from_secs(1) }).unwrap();
    settle(&mut scene, &mut view, &world, &mut now);
    let invalid = EnvironmentPush::Partial {
        experience: a,
        sky: vec![EnvironmentProperty { name: "gamma".into(), source: b"r9.0".as_slice().into() }],
        water: vec![EnvironmentProperty { name: "".into(), source: b"r1.0".as_slice().into() }],
        transition: Duration::ZERO,
    };
    assert_eq!(scene.push_environment(region, invalid), Err(scene::EnvironmentPushError::Invalid));
    settle(&mut scene, &mut view, &world, &mut now);
    let environment = view.viewer_environment().unwrap();
    assert_eq!(environment.sky_properties().len(), 1);
    assert_eq!(environment.sky_properties()["gamma"].experience(), b);
    assert_eq!(environment.sky_properties()["gamma"].source(), b"r2.0");
    scene.notify_world_change();
    let ReconcileStep::Read(job) = scene.poll(now) else { panic!("expected journal read") };
    scene.complete(job, Err(ReadFailure::World(WorldReadError::ResyncRequired)), now);
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.viewer_environment().unwrap().sky_properties()["gamma"].experience(), b, "World resync must preserve viewer overrides");
    let retained = view.viewer_environment().unwrap().clone();
    scene.push_environment(region, EnvironmentPush::Clear { experience: Some(b), transition: Duration::ZERO }).unwrap();
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(view.viewer_environment().unwrap().sky_properties().is_empty(), "clearing the winner must not resurrect overwritten values");
    scene.close();
    drop(scene);
    drop(view);
    assert!(budget.charged_bytes() > 0, "retained viewer snapshot must keep its admission");
    assert_eq!(retained.sky_properties()["gamma"].source(), b"r2.0");
    drop(retained);
    assert_eq!(budget.charged_bytes(), 0);
}

#[test]
fn experience_assets_order_and_clear() {
    let world = world();
    let region = world.region_page(None, 1).unwrap().regions[0].region;
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let a = ExperienceId::new(Uuid::from_u128(10)).unwrap();
    let b = ExperienceId::new(Uuid::from_u128(20)).unwrap();
    let mut requests = Vec::new();
    for experience in [a, b] {
        scene
            .push_environment(
                region,
                EnvironmentPush::Full { experience, asset: AssetId::new(experience.uuid()), transition: Duration::ZERO },
            )
            .unwrap();
        requests.extend(settle_materials(&mut scene, &mut view, &world, &mut now).requested);
    }
    let ready = prepared_asset(requests[0].request, b"{'type':'sky','name':'test'}".to_vec());
    scene.complete_asset(requests[1].ticket, ready.clone());
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(view.viewer_environment().is_none(), "later resolution must wait for earlier push");
    scene.complete_asset(requests[0].ticket, ready.clone());
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.viewer_environment().unwrap().sky().unwrap().experience(), b);
    assert!(view.viewer_environment().unwrap().water().is_none());
    scene
        .push_environment(region, EnvironmentPush::Full { experience: a, asset: AssetId::new(a.uuid()), transition: Duration::ZERO })
        .unwrap();
    let pending = settle_materials(&mut scene, &mut view, &world, &mut now).requested.pop().unwrap();
    scene.push_environment(region, EnvironmentPush::Clear { experience: Some(a), transition: Duration::ZERO }).unwrap();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.contains(&pending.ticket));
    scene.complete_asset(pending.ticket, ready);
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.viewer_environment().unwrap().sky().unwrap().experience(), b, "late completion must not restore cleared experience");
}

#[test]
fn cleared_sky_restores_experience_day() {
    let world = world();
    let region = world.region_page(None, 1).unwrap().regions[0].region;
    let (mut scene, mut view) = SceneReconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let a = ExperienceId::new(Uuid::from_u128(10)).unwrap();
    let b = ExperienceId::new(Uuid::from_u128(20)).unwrap();
    for (experience, source) in [
        (
            a,
            "{'type':'daycycle','frames':{'w':{'type':'water'},'s':{'type':'sky'}},'tracks':[[{'key_keyframe':r0,'key_name':'w'}],[{'key_keyframe':r0,'key_name':'s'}],[],[],[]]}",
        ),
        (b, "{'type':'sky'}"),
    ] {
        scene
            .push_environment(
                region,
                EnvironmentPush::Full { experience, asset: AssetId::new(experience.uuid()), transition: Duration::ZERO },
            )
            .unwrap();
        let request = settle_materials(&mut scene, &mut view, &world, &mut now).requested.pop().unwrap();
        scene.complete_asset(request.ticket, prepared_asset(request.request, source.as_bytes().to_vec()));
        settle_materials(&mut scene, &mut view, &world, &mut now);
    }
    let current = view.viewer_environment().unwrap();
    assert_eq!(current.sky().unwrap().experience(), b);
    assert_eq!(current.water().unwrap().experience(), a);
    scene.push_environment(region, EnvironmentPush::Clear { experience: Some(b), transition: Duration::ZERO }).unwrap();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(
        view.viewer_environment().unwrap().sky().unwrap().experience(),
        a,
        "clearing fixed sky must reveal the active experience day"
    );
    scene
        .push_environment(region, EnvironmentPush::Full { experience: b, asset: AssetId::new(b.uuid()), transition: Duration::ZERO })
        .unwrap();
    let request = settle_materials(&mut scene, &mut view, &world, &mut now).requested.pop().unwrap();
    scene.complete_asset(request.ticket, assets::Status::Failed { failure: assets::Failure::Malformed, retry_at: None });
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.viewer_environment().unwrap().sky().unwrap().experience(), a, "failed push must retain the previous environment");
    scene.push_environment(region, EnvironmentPush::Clear { experience: None, transition: Duration::ZERO }).unwrap();
    settle(&mut scene, &mut view, &world, &mut now);
    let current = view.viewer_environment().unwrap();
    assert!(current.sky().is_none() && current.water().is_none());
}
