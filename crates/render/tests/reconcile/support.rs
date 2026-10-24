pub(super) use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU32,
    time::{Duration, Instant},
};

pub(super) use assets::{Priority, Request, mesh::Mesh};
pub(super) use math::{DVec3, Quat, Vec3};
pub(super) use proptest::prelude::{ProptestConfig, prop, prop_assert_eq, proptest};
pub(super) use render::scene::{
    AssetChange, AssetRequest, AssetTicket, Budget, Camera, Detail, EnvironmentProperty, EnvironmentPush, MeshJob, Placement, PreparedMesh,
    ReadFailure, ReadJob, ReadQuery, ReadReply, ReconcileStep, Reconciler, RecoveryReason, SceneView, Status, SurfaceKind,
};
pub(super) use utils::{AssetId, ExperienceId};
pub(super) use uuid::Uuid;
pub(super) use world::{
    Appearance, EnvironmentAssignment, EnvironmentScope, EnvironmentSource, FieldUpdate, Kind, ObjectObservation, ObjectPatch,
    OrderEvidence::Ordered, ParcelLocalId, ParentUpdate, Pose, Region, RegionInfo, Shape, TerrainPatch, TerrainPatchId, World, WorldLimits,
    WorldReadError,
};

pub(super) fn world() -> World {
    world_at(DVec3::ZERO)
}

pub(super) fn world_at(origin: DVec3) -> World {
    world_with_journal(origin, 4096)
}

pub(super) fn world_with_journal(origin: DVec3, journal_entries: usize) -> World {
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

pub(super) fn source(region: u64, order: u64) -> world::UpdateSource {
    world::UpdateSource::new(Region::new(region), order as u32, Ordered(order), 0)
}

pub(super) fn add(world: &mut World, id: u128, local: u32, parent: ParentUpdate, x: f32, order: u64) {
    let mut obj = ObjectObservation::new(source(1, order), Uuid::from_u128(id), NonZeroU32::new(local).unwrap(), Kind::Object, parent);
    obj.pose = Some(Pose { position: Some(Vec3::new(x, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
    obj.scale = Some(Vec3::ONE);
    obj.shape = FieldUpdate::Set(Shape::Prim(Default::default()));
    obj.appearance = FieldUpdate::Set(Appearance::Surface);
    let _ = world.observe_object(obj).unwrap();
}

pub(super) fn move_root(world: &mut World, x: f32, order: u64) {
    let mut patch = ObjectPatch::new(source(1, order), NonZeroU32::new(1).unwrap());
    patch.pose = Some(Pose { position: Some(Vec3::new(x, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
    let _ = world.patch_object(patch).unwrap();
}

pub(super) fn reply(world: &World, job: &ReadJob) -> Result<ReadReply, ReadFailure> {
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

pub(super) fn settle(scene: &mut Reconciler, view: &mut SceneView, world: &World, now: &mut Instant) -> usize {
    settle_work(scene, view, world, now).0
}

pub(super) fn settle_work(scene: &mut Reconciler, view: &mut SceneView, world: &World, now: &mut Instant) -> (usize, usize) {
    let work = settle_materials(scene, view, world, now);
    assert!(work.requested.is_empty(), "geometry fixture unexpectedly requested assets");
    (work.reads, work.builds)
}

#[derive(Default)]
pub(super) struct AssetWork {
    pub(super) reads: usize,
    pub(super) builds: usize,
    pub(super) requested: Vec<AssetRequest>,
    pub(super) released: Vec<AssetTicket>,
}

pub(super) fn settle_materials(scene: &mut Reconciler, view: &mut SceneView, world: &World, now: &mut Instant) -> AssetWork {
    let mut work = AssetWork::default();
    for _ in 0..20_000 {
        match scene.poll(*now) {
            ReconcileStep::Asset(AssetChange::Request(request)) => work.requested.push(request),
            ReconcileStep::Asset(AssetChange::Release(ticket)) => work.released.push(ticket),
            ReconcileStep::Read(job) => {
                work.reads += 1;
                let result = reply(world, &job);
                scene.complete_read(job, result, *now);
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

pub(super) fn next_terrain_job(scene: &mut Reconciler, view: &mut SceneView, world: &World, now: &mut Instant) -> MeshJob {
    for _ in 0..20_000 {
        match scene.poll(*now) {
            ReconcileStep::Read(job) => {
                let result = reply(world, &job);
                scene.complete_read(job, result, *now);
            }
            ReconcileStep::Build(job) => return job,
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => *now = until,
            ReconcileStep::Asset(_) => panic!("terrain fixture unexpectedly requested assets"),
            ReconcileStep::Idle => panic!("expected terrain work"),
            ReconcileStep::Closed => panic!("unexpected closure"),
        }
    }
    panic!("terrain work did not start");
}

pub(super) fn ids(view: &SceneView) -> BTreeSet<u128> {
    view.objects().map(|object| object.detail().summary.id.as_u128()).collect()
}

pub(super) fn prepared_asset(resource: Request, bytes: Vec<u8>) -> assets::Status {
    let now = Instant::now();
    let mut store = assets::Store::new(assets::Budget::default());
    let route = store.register_route().unwrap();
    let demand = store.request(route, resource, Priority::Visible, now).unwrap();
    let assets::LoadJob::Fetch(job) = store.next_job(now).unwrap() else { panic!("fresh asset request unexpectedly used cached source") };
    let completion = match job.complete(Ok(bytes)) {
        Ok(input) => store.start_decode(input).expect("decode admission").run(),
        Err(failure) => failure.into(),
    };
    store.complete(completion, now);
    demand.status()
}

pub(super) fn positions(view: &SceneView) -> BTreeMap<u128, f64> {
    view.objects()
        .map(|obj| {
            let Placement::Ready(transform) = obj.placement() else {
                panic!("expected resolved object: {:?}", obj.detail().summary.id);
            };
            (obj.detail().summary.id.as_u128(), transform.pose.position.x)
        })
        .collect()
}

pub(super) fn camera(x: f64) -> Camera {
    Camera::new(DVec3::new(x, 0.0, 0.0), Vec3::X, std::f64::consts::FRAC_PI_2, [1000, 1000]).unwrap()
}
