use std::{f64::consts::PI, num::NonZeroU32, time::Instant};

use assets::{Budget as AssetBudget, Content, LoadJob, Priority, Request, Status as AssetStatus, Store, TextureSize};
use math::{DVec3, Quat, Vec3};
use render::scene::{
    AssetChange, Budget, Camera, Detail, MeshJob, ReadFailure, ReadJob, ReadQuery, ReadReply, ReconcileStep, Reconciler, SceneView, Status,
};
use utils::AssetId;
use uuid::Uuid;
use world::{
    Appearance, FaceAppearance, FieldUpdate, Kind, ObjectObservation, ObjectPatch, OrderEvidence::Ordered, ParentUpdate, Pose, PrimShape,
    Region, RegionInfo, Shape, SurfaceAppearance, TerrainPatch, TerrainSettings, UpdateSource, World, WorldLimits, WorldReadError,
};

const INITIAL_ORDER: u64 = 100_000;
const DEFAULT_TEXTURE_ID: u128 = 9_999_999;

pub struct Row {
    region: Region,
    local_id: NonZeroU32,
}

pub struct BenchmarkTextures {
    pub store: Store,
    pub route: assets::Route,
    pub textures: std::collections::HashMap<AssetId, assets::TextureHandle>,
}

pub fn scene_world(count: usize, region_count: usize) -> (World, Vec<Row>) {
    scene_world_with(count, region_count, 1, prim_shape, false, None)
}

pub fn prepare_world(count: usize, mesh_count: usize) -> (World, Vec<Row>) {
    scene_world_with(count, 1, 1, |index| prepare_shape(index % mesh_count), true, None)
}

pub fn live_prepare_world(count: usize, mesh_count: usize, texture_count: usize) -> (World, Vec<Row>) {
    let object_textures = texture_count.saturating_sub(4).max(1);
    let (mut world, rows) =
        scene_world_with(count, 1, object_textures, |index| prepare_shape(index % mesh_count), true, Some(object_textures));
    let order_base = INITIAL_ORDER + count as u64 + 100;
    for (index, row) in rows.iter().enumerate() {
        let current = order_base + index as u64;
        let x = ((index * 137) % 256) as f32;
        let y = (row.local_id.get() / 100) as f32;
        let mut patch = ObjectPatch::new(UpdateSource::new(row.region, current as u32, Ordered(current), 0), row.local_id);
        patch.pose = Some(Pose { position: Some(Vec3::new(x, y, 0.0)), rotation: Some(Quat::IDENTITY) });
        let _applied = world.patch_object(patch).expect("benchmark object layout");
    }
    (world, rows)
}

fn scene_world_with(
    count: usize,
    region_count: usize,
    texture_count: usize,
    shape: impl Fn(usize) -> PrimShape,
    terrain: bool,
    terrain_texture_start: Option<usize>,
) -> (World, Vec<Row>) {
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
        if terrain && index == 0 {
            let first = terrain_texture_start.unwrap_or_else(|| texture_count.saturating_sub(4));
            let texture_ids = std::array::from_fn(|slot| Uuid::from_u128(DEFAULT_TEXTURE_ID + (first + slot) as u128));
            let settings = TerrainSettings::new(texture_ids, [0.0; 4], [64.0; 4]).expect("benchmark terrain settings");
            let order = INITIAL_ORDER - 1;
            let _settings = world
                .observe_terrain_settings(UpdateSource::new(region, order as u32, Ordered(order), 0), settings)
                .expect("benchmark terrain settings observation");
        }
    }
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
        observation.shape = FieldUpdate::Set(Shape::Prim(shape(index)));
        observation.appearance = FieldUpdate::Set(Appearance::Surface);
        observation.surface_appearance = Some(
            SurfaceAppearance::new(
                FaceAppearance { texture: Some(texture_asset(index % texture_count)), ..FaceAppearance::default() },
                Vec::new(),
            )
            .expect("benchmark surface appearance"),
        );
        let _applied = world.observe_object(observation).expect("benchmark object introduction");
        rows.push(Row { region, local_id });
    }
    if terrain {
        let region = regions[0];
        let mut order = INITIAL_ORDER + count as u64;
        for y in 0..6 {
            for x in 0..6 {
                let source = UpdateSource::new(region, order as u32, Ordered(order), 0);
                let patch = TerrainPatch::new(x, y, Box::new([0.0; 256])).expect("benchmark terrain patch");
                let _applied = world.observe_terrain_patch(source, patch).expect("benchmark terrain observation");
                order += 1;
            }
        }
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
    let texture = benchmark_texture();
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
            ReconcileStep::Asset(AssetChange::Request(request)) if request.request.resource().kind == assets::Kind::Texture => {
                reconciler.complete_asset(request.ticket, AssetStatus::Ready(Content::Texture(texture.clone())));
            }
            ReconcileStep::Asset(_) => {}
            ReconcileStep::Idle => panic!("prim benchmark has no mesh work"),
            ReconcileStep::Closed => panic!("benchmark scene closed"),
        }
    }
    panic!("prim benchmark did not produce mesh work");
}

fn benchmark_texture() -> assets::TextureHandle {
    benchmark_texture_for(texture_asset(0))
}

pub fn benchmark_textures(count: usize) -> BenchmarkTextures {
    let now = Instant::now();
    let mut store = Store::new(AssetBudget::default());
    let route = store.register_route().expect("benchmark texture route");
    let mut textures = std::collections::HashMap::with_capacity(count);
    for index in 0..count {
        let id = texture_asset(index);
        let demand = store
            .request(route, Request::Texture { id, size: TextureSize::new(64) }, Priority::Visible, now)
            .expect("benchmark texture request");
        let LoadJob::Fetch(fetch) = store.next_job(now).expect("benchmark texture fetch") else {
            panic!("unexpected benchmark texture job")
        };
        let input = fetch.complete(Ok(test_support::texture::GRAY_4096_J2C.to_vec())).expect("benchmark texture response");
        let decode = store.start_decode(input).expect("benchmark texture decode");
        store.complete(decode.run(), now);
        let AssetStatus::Ready(Content::Texture(texture)) = demand.status() else { panic!("benchmark texture did not decode") };
        textures.insert(id, texture);
    }
    BenchmarkTextures { store, route, textures }
}

pub fn complete_texture_refinements(fixture: &mut BenchmarkTextures, indexes: [usize; 2], size: u32, now: Instant) -> usize {
    let mut completed = 0;
    for index in indexes {
        let id = texture_asset(index);
        let demand = fixture
            .store
            .request(fixture.route, Request::Texture { id, size: TextureSize::new(size) }, Priority::Visible, now)
            .expect("benchmark texture refinement request");
        let Some(job) = fixture.store.next_job(now) else {
            assert!(matches!(demand.status(), AssetStatus::Ready(_)), "benchmark texture refinement has no job or ready content");
            continue;
        };
        complete_benchmark_texture_job(&mut fixture.store, job, now);
        assert!(matches!(demand.status(), AssetStatus::Ready(_)), "benchmark texture refinement did not complete");
        completed += 1;
    }
    completed
}

fn complete_benchmark_texture_job(store: &mut Store, mut job: LoadJob, now: Instant) {
    loop {
        match job {
            LoadJob::ReadCache(cache) => {
                job = store.complete_cache_read(cache.run(), now).expect("benchmark cache read continuation");
            }
            LoadJob::Fetch(fetch) => {
                let input = fetch.complete(Ok(test_support::texture::GRAY_4096_J2C.to_vec())).expect("benchmark texture response");
                let completion = store.start_decode(input).expect("benchmark texture decode admission").run();
                store.complete(completion, now);
                return;
            }
            LoadJob::Decode(input) => {
                let completion = store.start_decode(input).expect("benchmark texture decode admission").run();
                store.complete(completion, now);
                return;
            }
        }
    }
}

fn benchmark_texture_for(id: AssetId) -> assets::TextureHandle {
    let now = Instant::now();
    let mut store = Store::new(AssetBudget::default());
    let route = store.register_route().expect("benchmark texture route");
    let demand = store
        .request(route, Request::Texture { id, size: TextureSize::new(64) }, Priority::Visible, now)
        .expect("benchmark texture request");
    let LoadJob::Fetch(fetch) = store.next_job(now).expect("benchmark texture fetch") else { panic!("texture cache is empty") };
    let input = fetch.complete(Ok(test_support::texture::RGB_J2C.to_vec())).expect("benchmark texture response");
    let decode = store.start_decode(input).expect("benchmark texture decode");
    store.complete(decode.run(), now);
    let AssetStatus::Ready(Content::Texture(texture)) = demand.status() else { panic!("benchmark texture did not decode") };
    texture
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
    settle_with_camera(world, expected_objects, scene_camera())
}

pub fn settle_with_camera(world: &World, expected_objects: usize, camera: Camera) -> (Reconciler, SceneView, Instant) {
    let (mut reconciler, mut view) = Reconciler::new(Budget::default());
    reconciler.set_camera(camera);
    let mut now = Instant::now();
    let texture = benchmark_texture();
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
            ReconcileStep::Asset(AssetChange::Request(request)) if request.request.resource().kind == assets::Kind::Texture => {
                reconciler.complete_asset(request.ticket, AssetStatus::Ready(Content::Texture(texture.clone())));
            }
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

pub fn settle_with_textures(
    world: &World,
    expected_objects: usize,
    camera: Camera,
    textures: &std::collections::HashMap<AssetId, assets::TextureHandle>,
) -> (Reconciler, SceneView, Instant) {
    let (mut reconciler, mut view) = Reconciler::new(Budget::default());
    reconciler.set_camera(camera);
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
            ReconcileStep::Asset(AssetChange::Request(request)) if request.request.resource().kind == assets::Kind::Texture => {
                let id = request.request.resource().id;
                let texture = textures.get(&id).expect("benchmark texture fixture contains requested asset").clone();
                reconciler.complete_asset(request.ticket, AssetStatus::Ready(Content::Texture(texture)));
            }
            ReconcileStep::Asset(_) => {}
            ReconcileStep::Idle => {
                assert_eq!(view.object_count(), expected_objects);
                assert_eq!(view.status(), Status::Ready);
                return (reconciler, view, now);
            }
            ReconcileStep::Closed => panic!("benchmark scene closed"),
        }
    }
    panic!("live-like benchmark scene did not settle");
}

pub fn update_many(world: &mut World, rows: &[Row], count: usize, order: &mut u64, generation: u64) {
    let offset = (generation % 2) as f32;
    for row in rows.iter().take(count) {
        let current = *order;
        *order += 1;
        let mut patch = ObjectPatch::new(UpdateSource::new(row.region, current as u32, Ordered(current), 0), row.local_id);
        let x = (((row.local_id.get() as usize - 1) * 137) % 256) as f32 + offset;
        let y = (row.local_id.get() / 100) as f32;
        patch.pose = Some(Pose { position: Some(Vec3::new(x, y, 0.0)), rotation: Some(Quat::IDENTITY) });
        let _applied = world.patch_object(patch).expect("benchmark object update");
    }
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

pub fn update_one(world: &mut World, row: &Row, order: &mut u64, generation: u64) {
    let current = *order;
    *order += 1;
    let mut patch = ObjectPatch::new(UpdateSource::new(row.region, current as u32, Ordered(current), 0), row.local_id);
    let offset = (generation % 2) as f32;
    patch.pose = Some(Pose {
        position: Some(Vec3::new((row.local_id.get() % 100) as f32 + offset, (row.local_id.get() / 100) as f32, 0.0)),
        rotation: Some(Quat::IDENTITY),
    });
    let _applied = world.patch_object(patch).expect("benchmark object update");
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

pub fn prepare_camera() -> Camera {
    Camera::new(DVec3::new(50.0, 15.0, 120.0), -Vec3::Z, PI / 2.0, [1024, 1024])
        .expect("benchmark prepare camera")
        .with_quality(512.0, 1.0)
        .expect("benchmark camera quality")
}

fn texture_asset(index: usize) -> AssetId {
    AssetId::new(Uuid::from_u128(DEFAULT_TEXTURE_ID + index as u128))
}

fn scene_camera() -> Camera {
    Camera::new(DVec3::new(128.0, 12.0, 200.0), -Vec3::Z, PI / 2.0, [1024, 1024])
        .expect("benchmark scene camera")
        .with_quality(512.0, 1.0)
        .expect("benchmark camera quality")
}

fn prepare_shape(index: usize) -> PrimShape {
    let twist = (index % 201) as i16;
    let twist = if twist <= 100 { twist } else { twist - 201 };
    PrimShape {
        profile_curve: 2,
        profile_hollow: 12_500,
        path_scale_x: 100 + (index / 201) as u8,
        path_twist: twist as i8,
        ..PrimShape::default()
    }
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
