use std::{f64::consts::PI, num::NonZeroU32, time::Instant};

use assets::{LoadJob, Priority, Request, Store};
use math::{DVec3, Quat, Vec3};
use render::scene::{
    AssetChange, AssetRequest, AssetTicket, Budget, Camera, Detail, MeshJob, ReadFailure, ReadJob, ReadQuery, ReadReply, ReconcileStep,
    Reconciler, SceneView, Status,
};
use utils::AssetId;
use uuid::Uuid;
use world::{
    Appearance, FaceAppearance, FieldUpdate, Kind, ObjectObservation, ObjectPatch, OrderEvidence::Ordered, ParentUpdate, Pose, PrimShape,
    Region, RegionInfo, SculptFlags, Shape, SurfaceAppearance, UpdateSource, World, WorldLimits, WorldReadError,
};

const INITIAL_ORDER: u64 = 100_000;
const DEFAULT_TEXTURE_ID: u128 = 9_999_999;

pub struct AssetWork {
    pub requested: Vec<AssetRequest>,
    pub released: Vec<AssetTicket>,
}

pub struct Row {
    region: Region,
    local_id: NonZeroU32,
}

pub fn scene_world(count: usize, region_count: usize) -> (World, Vec<Row>) {
    let mut world = World::new(WorldLimits {
        max_region_registrations: 8,
        max_entities: 65_536,
        max_entities_per_region: count,
        max_tombstones_per_region: 4_096,
        max_tombstones: 16_384,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(4_096),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(16_384),
        max_journal_entries: 4_096,
        max_journal_changes: 65_536,
        max_region_name_bytes: 256,
        max_recovery_targets: 256,
    });
    let regions = (0..region_count).map(|index| Region::new(index as u64 + 1)).collect::<Vec<_>>();
    for (index, region) in regions.iter().copied().enumerate() {
        let _adopted = world.adopt_region(region).expect("benchmark region adoption");
        let _metadata = world
            .patch_region(region, RegionInfo { origin: Some(DVec3::new(index as f64 * 256.0, 0.0, 0.0)), ..RegionInfo::default() })
            .expect("benchmark region metadata");
    }
    let appearance = SurfaceAppearance::new(
        FaceAppearance { texture: Some(AssetId::new(Uuid::from_u128(DEFAULT_TEXTURE_ID))), ..FaceAppearance::default() },
        Vec::new(),
    )
    .expect("benchmark default surface appearance");
    let per_region = count.div_ceil(region_count);
    let mut rows = Vec::with_capacity(count);
    for index in 0..count {
        let region_index = index / per_region;
        let region = regions[region_index];
        let local_id = NonZeroU32::new((index % per_region + 1) as u32).expect("benchmark local ID");
        let id = Uuid::from_u128(index as u128 + 1);
        let order = INITIAL_ORDER + index as u64;
        let mut observation = ObjectObservation::new(
            UpdateSource::new(region, order as u32, Ordered(order), 0),
            id,
            local_id,
            Kind::Object,
            ParentUpdate::Root,
        );
        observation.pose = Some(Pose {
            position: Some(Vec3::new((local_id.get() % 100) as f32, (local_id.get() / 100) as f32, 0.0)),
            rotation: Some(Quat::IDENTITY),
        });
        observation.scale = Some(Vec3::ONE);
        observation.shape = FieldUpdate::Set(Shape::Prim(prim_shape(index)));
        observation.appearance = FieldUpdate::Set(Appearance::Surface);
        observation.surface_appearance = Some(appearance.clone());
        let _applied = world.observe_object(observation).expect("benchmark object introduction");
        rows.push(Row { region, local_id });
    }
    (world, rows)
}

pub fn prim_job(shape: PrimShape, detail: Detail) -> (Reconciler, MeshJob) {
    let (mut world, rows) = scene_world(1, 1);
    let row = &rows[0];
    let mut patch = ObjectPatch::new(UpdateSource::new(row.region, 200_000, Ordered(200_000), 0), row.local_id);
    patch.shape = FieldUpdate::Set(Shape::Prim(shape));
    let _applied = world.patch_object(patch).expect("benchmark prim shape");
    let camera = camera(detail);
    let (mut reconciler, mut view) = Reconciler::new(Budget::default());
    reconciler.set_camera(camera);
    let mut now = Instant::now();
    for _ in 0..100_000 {
        match reconciler.poll(now) {
            ReconcileStep::Read(job) => {
                let reply = reply(&world, &job);
                reconciler.complete_read(job, reply, now);
            }
            ReconcileStep::Build(job) => return (reconciler, job),
            ReconcileStep::Publish(batch) => view.apply(batch).expect("benchmark scene publication"),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            ReconcileStep::Asset(_) => {}
            ReconcileStep::Idle => panic!("prim benchmark has no mesh work"),
            ReconcileStep::Closed => panic!("benchmark scene closed"),
        }
    }
    panic!("prim benchmark did not produce mesh work");
}

pub fn first_full_view_batch(world: &World, expected_objects: usize) {
    let (mut reconciler, mut view) = Reconciler::new(Budget::default());
    reconciler.set_camera(scene_camera());
    let mut now = Instant::now();
    let mut mesh_jobs = 0;
    let mut asset_requests = 0;
    for _ in 0..100_000 {
        match reconciler.poll(now) {
            ReconcileStep::Read(job) => {
                let reply = reply(world, &job);
                reconciler.complete_read(job, reply, now);
            }
            ReconcileStep::Publish(batch) => {
                view.apply(batch).expect("benchmark scene publication");
                if view.object_count() == expected_objects {
                    assert!(mesh_jobs > 0, "initial scene did not plan any visible prim meshes");
                    assert!(asset_requests > 0, "initial scene did not request the benchmark texture");
                    return;
                }
            }
            ReconcileStep::Build(job) => {
                mesh_jobs += 1;
                reconciler.complete_mesh(job.run());
            }
            ReconcileStep::Asset(AssetChange::Request(_)) => asset_requests += 1,
            ReconcileStep::Asset(AssetChange::Release(_)) => {}
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            ReconcileStep::Idle if view.object_count() == expected_objects => {
                assert!(mesh_jobs > 0, "initial scene did not plan any visible prim meshes");
                assert!(asset_requests > 0, "initial scene did not request the benchmark texture");
                return;
            }
            ReconcileStep::Idle => panic!("initial scene did not publish all objects"),
            ReconcileStep::Closed => panic!("benchmark scene closed"),
        }
    }
    panic!("initial scene did not converge");
}

pub fn settle(world: &World, expected_objects: usize) -> (Reconciler, SceneView, Instant) {
    let (mut reconciler, mut view) = Reconciler::new(Budget::default());
    reconciler.set_camera(scene_camera());
    let mut now = Instant::now();
    for _ in 0..100_000 {
        match reconciler.poll(now) {
            ReconcileStep::Read(job) => {
                let reply = reply(world, &job);
                reconciler.complete_read(job, reply, now);
            }
            ReconcileStep::Publish(batch) => view.apply(batch).expect("benchmark scene publication"),
            ReconcileStep::Build(job) => reconciler.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            ReconcileStep::Asset(_) => {}
            ReconcileStep::Idle => {
                assert_eq!(view.object_count(), expected_objects);
                assert_eq!(view.status(), Status::Ready);
                return (reconciler, view, now);
            }
            ReconcileStep::Closed => panic!("benchmark scene closed"),
        }
    }
    panic!("initial scene did not settle");
}

pub fn update_one_percent(world: &mut World, rows: &[Row], order: &mut u64, generation: u64) {
    let changed = (rows.len() / 100).max(1);
    let offset = (generation % 2) as f32;
    for (index, row) in rows.iter().take(changed).enumerate() {
        let current = *order;
        *order += 1;
        let mut patch = ObjectPatch::new(UpdateSource::new(row.region, current as u32, Ordered(current), 0), row.local_id);
        patch.pose =
            Some(Pose { position: Some(Vec3::new(100.0 + offset + index as f32 / 100.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
        let _applied = world.patch_object(patch).expect("benchmark object update");
    }
}

pub fn first_publish(reconciler: &mut Reconciler, view: &mut SceneView, world: &World, now: &mut Instant) {
    reconciler.notify_world_change();
    for _ in 0..100_000 {
        match reconciler.poll(*now) {
            ReconcileStep::Read(job) => {
                let reply = reply(world, &job);
                reconciler.complete_read(job, reply, *now);
            }
            ReconcileStep::Publish(batch) => {
                view.apply(batch).expect("benchmark scene publication");
                return;
            }
            ReconcileStep::Build(job) => reconciler.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => *now = until,
            ReconcileStep::Asset(_) => {}
            ReconcileStep::Idle => panic!("object update did not publish"),
            ReconcileStep::Closed => panic!("benchmark scene closed"),
        }
    }
    panic!("object update did not converge");
}

pub fn ready_distinct_mesh_scene(count: usize) -> (Reconciler, SceneView, World, Instant) {
    let world = distinct_mesh_world(count);
    let (mut reconciler, mut view) = Reconciler::new(Budget::default());
    reconciler.set_camera(mesh_camera(false));
    let mut now = Instant::now();
    let bytes = test_support::mesh::asset(&[2, 3], 1);

    let work = mesh_asset_work(&mut reconciler, &mut view, &world, &mut now);
    let headers: Vec<_> = work.requested.into_iter().filter(|request| matches!(request.request, Request::MeshHeader(_))).collect();
    assert_eq!(headers.len(), count, "every distinct mesh object requests its own header");
    for header in headers {
        reconciler.complete_asset(header.ticket, prepared_asset(header.request, &bytes));
    }

    let work = mesh_asset_work(&mut reconciler, &mut view, &world, &mut now);
    let highs: Vec<_> = work
        .requested
        .into_iter()
        .filter(|request| matches!(request.request, Request::MeshLod(block) if block.lod() == assets::mesh::MeshLod::High))
        .collect();
    assert_eq!(highs.len(), count, "every distinct mesh object requests its selected LOD");
    for high in highs {
        reconciler.complete_asset(high.ticket, prepared_asset(high.request, &bytes));
    }
    let work = mesh_asset_work(&mut reconciler, &mut view, &world, &mut now);
    assert_eq!(work.released.len(), count, "ready selected LODs release their fallback requests");
    assert!(view.objects().all(|object| object.mesh_lod().is_some_and(|(_, lod)| lod == assets::mesh::MeshLod::High)));
    (reconciler, view, world, now)
}

pub fn request_distinct_mesh_refinement(reconciler: &mut Reconciler, view: &mut SceneView, world: &World, now: &mut Instant) -> AssetWork {
    reconciler.set_camera(mesh_camera(true));
    mesh_asset_work(reconciler, view, world, now)
}

fn distinct_mesh_world(count: usize) -> World {
    let mut world = World::new(WorldLimits {
        max_region_registrations: 1,
        max_entities: count,
        max_entities_per_region: count,
        max_tombstones_per_region: 1,
        max_tombstones: 1,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
        max_journal_entries: 4096,
        max_journal_changes: count * 4,
        max_region_name_bytes: 64,
        max_recovery_targets: 4,
    });
    let region = Region::new(1);
    let _adopted = world.adopt_region(region).expect("mesh benchmark region adoption");
    let _metadata = world
        .patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() })
        .expect("mesh benchmark region metadata");
    for index in 0..count {
        let order = INITIAL_ORDER + index as u64;
        let local_id = NonZeroU32::new(u32::try_from(index + 1).expect("mesh benchmark local ID fits u32"))
            .expect("mesh benchmark local ID is nonzero");
        let mut object = ObjectObservation::new(
            UpdateSource::new(region, order as u32, Ordered(order), 0),
            Uuid::from_u128(index as u128 + 1),
            local_id,
            Kind::Object,
            ParentUpdate::Root,
        );
        object.pose = Some(Pose { position: Some(Vec3::new(2.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
        object.scale = Some(Vec3::ONE);
        object.shape = FieldUpdate::Set(Shape::Mesh {
            asset_id: AssetId::new(Uuid::from_u128(50_000_000 + index as u128)),
            flags: SculptFlags::default(),
        });
        object.appearance = FieldUpdate::Set(Appearance::Surface);
        let _applied = world.observe_object(object).expect("mesh benchmark object introduction");
    }
    world
}

fn mesh_asset_work(reconciler: &mut Reconciler, view: &mut SceneView, world: &World, now: &mut Instant) -> AssetWork {
    let mut work = AssetWork { requested: Vec::new(), released: Vec::new() };
    for _ in 0..100_000 {
        match reconciler.poll(*now) {
            ReconcileStep::Asset(AssetChange::Request(request)) => work.requested.push(request),
            ReconcileStep::Asset(AssetChange::Release(ticket)) => work.released.push(ticket),
            ReconcileStep::Read(job) => {
                let reply = reply(world, &job);
                reconciler.complete_read(job, reply, *now);
            }
            ReconcileStep::Build(job) => reconciler.complete_mesh(job.run()),
            ReconcileStep::Publish(batch) => view.apply(batch).expect("mesh benchmark scene publication"),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => *now = until,
            ReconcileStep::Idle => return work,
            ReconcileStep::Closed => panic!("mesh benchmark scene closed"),
        }
    }
    panic!("mesh benchmark scene did not converge");
}

fn prepared_asset(request: Request, bytes: &[u8]) -> assets::Status {
    let now = Instant::now();
    let mut store = Store::new(assets::Budget::default());
    let route = store.register_route().expect("mesh benchmark asset route");
    let demand = store.request(route, request, Priority::Visible, now).expect("mesh benchmark asset demand");
    let LoadJob::Fetch(job) = store.next_job(now).expect("mesh benchmark asset fetch") else {
        panic!("mesh benchmark asset must be absent from the new store");
    };
    let input = job.complete(Ok(bytes.to_vec())).expect("mesh benchmark asset bytes");
    let completion = store.start_decode(input).expect("mesh benchmark decode admission").run();
    store.complete(completion, now);
    demand.status()
}

fn camera(detail: Detail) -> Camera {
    let (distance, height) = match detail {
        Detail::Lowest => (10.0, 16),
        Detail::Low => (10.0, 80),
        Detail::Medium => (8.0, 256),
        Detail::High => (3.0, 256),
    };
    Camera::new(DVec3::new(0.0, 0.0, distance), -Vec3::Z, PI / 2.0, [height, height]).expect("benchmark camera")
}

fn prim_shape(index: usize) -> PrimShape {
    match index % 4 {
        0 => PrimShape::default(),
        1 => PrimShape { profile_curve: 0, ..PrimShape::default() },
        2 => PrimShape { path_curve: 0x20, profile_curve: 5, ..PrimShape::default() },
        _ => PrimShape { path_curve: 0x20, profile_curve: 0, path_scale_x: 150, path_scale_y: 150, ..PrimShape::default() },
    }
}

fn scene_camera() -> Camera {
    Camera::new(DVec3::new(128.0, 12.0, 200.0), -Vec3::Z, PI / 2.0, [1024, 1024])
        .expect("benchmark scene camera")
        .with_quality(512.0, 1.0)
        .expect("benchmark camera quality")
}

fn mesh_camera(medium: bool) -> Camera {
    let position = if medium { DVec3::new(-8.0, 0.0, 0.0) } else { DVec3::ZERO };
    Camera::new(position, Vec3::X, PI / 3.0, [1000, 1000]).expect("mesh benchmark camera")
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
    result.map_err(|error: WorldReadError| ReadFailure::World(error))
}
