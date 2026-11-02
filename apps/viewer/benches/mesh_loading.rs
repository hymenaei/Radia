use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU32,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use assets::{Content, Demand, Request, Status, cache::AssetCache};
use criterion::Criterion;
use futures::channel::oneshot;
use llsd::{Value, xml};
use math::{DVec3, Quat, Vec3};
use network::capabilities::Client;
use render::scene::{AssetChange, Camera, ReadFailure, ReadJob, ReadQuery, ReadReply, ReconcileStep, Reconciler, SceneView};
use tempfile::tempdir;
use test_support::http_peer::LoopbackPeer;
use tokio::{runtime::Runtime, sync::mpsc, task::JoinHandle};
use url::Url;
use utils::AssetId;
use uuid::Uuid;
use viewer::asset_coordinator::{AssetCoordinator, AssetHandle, RequestError};
use world::{
    Appearance, FaceAppearance, FieldUpdate, Kind, ObjectObservation, OrderEvidence, ParentUpdate, Pose, Region, RegionInfo, SculptFlags,
    Shape, SurfaceAppearance, UpdateSource, World, WorldLimits,
};

const MESH_OBJECTS: usize = 240;
const MESH_ASSETS: usize = 48;
const PRIM_OBJECTS: usize = 24;
const TEXTURE_IDS: usize = 8;
const PEER_DELAY: Duration = Duration::from_millis(100);
const MESH_TEXTURE_BASE: u128 = 50_000;
const PRIM_TEXTURE_BASE: u128 = 60_000;
const MESH_ASSET_BASE: u128 = 80_000;
static REPORTED_CONFIGS: AtomicU8 = AtomicU8::new(0);

struct SceneFixture {
    world: World,
    scene: Reconciler,
    view: SceneView,
    mesh_objects: Vec<(Uuid, f32)>,
    textures: HashSet<AssetId>,
}

struct TrackedDemand {
    _demand: Demand,
    watcher: JoinHandle<()>,
    request: Request,
}

#[derive(Default)]
struct Sample {
    nearest_10_ready: Option<Duration>,
    all_ready: Option<Duration>,
    textures_ready: Option<Duration>,
    http_requests: u64,
    http_bytes: u64,
}

pub fn register(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("build benchmark runtime");
    let mesh_assets = mesh_assets();
    let warm_directory = tempdir().expect("create warm cache directory");
    let warm_cache = Arc::new(AssetCache::open(warm_directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open warm cache"));
    let mut warm_sample = Sample::default();
    let mut previous_counts = None;
    let mut stable_counts = 0;
    for _ in 0..8 {
        warm_sample = runtime.block_on(run_measurement(Arc::clone(&warm_cache), Arc::clone(&mesh_assets), true));
        let counts = (warm_sample.http_requests, warm_sample.http_bytes);
        if previous_counts == Some(counts) {
            stable_counts += 1;
        } else {
            stable_counts = 0;
        }
        if stable_counts == 1 {
            break;
        }
        previous_counts = Some(counts);
    }
    report_counts_once(CacheMode::Warm, &warm_sample);

    let mut group = c.benchmark_group("mesh_loading");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(2));

    group.bench_function("cold/nearest_10_ready", |b| {
        b.iter_custom(|iterations| measure_iterations(iterations, &runtime, &mesh_assets, CacheMode::Cold, Metric::Nearest, None))
    });
    group.bench_function("cold/all_ready", |b| {
        b.iter_custom(|iterations| measure_iterations(iterations, &runtime, &mesh_assets, CacheMode::Cold, Metric::All, None))
    });
    group.bench_function("cold/textures_ready", |b| {
        b.iter_custom(|iterations| measure_iterations(iterations, &runtime, &mesh_assets, CacheMode::Cold, Metric::Textures, None))
    });
    group.bench_function("warm/nearest_10_ready", |b| {
        b.iter_custom(|iterations| {
            measure_iterations(iterations, &runtime, &mesh_assets, CacheMode::Warm, Metric::Nearest, Some(&warm_cache))
        })
    });
    group.bench_function("warm/all_ready", |b| {
        b.iter_custom(|iterations| measure_iterations(iterations, &runtime, &mesh_assets, CacheMode::Warm, Metric::All, Some(&warm_cache)))
    });
    group.bench_function("warm/textures_ready", |b| {
        b.iter_custom(|iterations| {
            measure_iterations(iterations, &runtime, &mesh_assets, CacheMode::Warm, Metric::Textures, Some(&warm_cache))
        })
    });
    group.finish();
}

#[derive(Clone, Copy)]
enum CacheMode {
    Cold,
    Warm,
}

#[derive(Clone, Copy)]
enum Metric {
    Nearest,
    All,
    Textures,
}

fn measure_iterations(
    iterations: u64,
    runtime: &Runtime,
    mesh_assets: &Arc<HashMap<Uuid, Vec<u8>>>,
    mode: CacheMode,
    metric: Metric,
    warm_cache: Option<&Arc<AssetCache>>,
) -> Duration {
    let mut elapsed = Duration::ZERO;
    for _ in 0..iterations {
        let sample = match mode {
            CacheMode::Cold => {
                let directory = tempdir().expect("create cold cache directory");
                let cache = Arc::new(AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cold cache"));
                runtime.block_on(run_measurement(cache, Arc::clone(mesh_assets), false))
            }
            CacheMode::Warm => runtime.block_on(run_measurement(
                Arc::clone(warm_cache.expect("warm mode has a primed cache")),
                Arc::clone(mesh_assets),
                false,
            )),
        };
        report_counts_once(mode, &sample);
        elapsed += match metric {
            Metric::Nearest => sample.nearest_10_ready.expect("nearest meshes become ready"),
            Metric::All => sample.all_ready.expect("all meshes become ready"),
            Metric::Textures => sample.textures_ready.expect("all benchmark textures become ready"),
        };
    }
    elapsed
}

fn report_counts_once(mode: CacheMode, sample: &Sample) {
    let (label, bit) = match mode {
        CacheMode::Cold => ("cold", 1),
        CacheMode::Warm => ("warm", 2),
    };
    if REPORTED_CONFIGS.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        eprintln!("mesh_loading {label}: requests={} bytes={}", sample.http_requests, sample.http_bytes);
    }
}

async fn run_measurement(cache: Arc<AssetCache>, mesh_assets: Arc<HashMap<Uuid, Vec<u8>>>, drain_demands: bool) -> Sample {
    let peer_assets = Arc::clone(&mesh_assets);
    let peer = LoopbackPeer::new_parallel(PEER_DELAY, move |request, base| respond(request, base, &peer_assets));
    let region = Region::new(1);
    let (cancel, cancelled) = oneshot::channel();
    let service = AssetCoordinator::new(
        tokio::runtime::Handle::current(),
        async move {
            let _ = cancelled.await;
        },
        Some(cache),
    );
    service.register(region, Client::new(peer.url().join("seed").expect("build capability URL")).expect("build HTTP client"));
    let handle = service.handle();
    let mut fixture = scene_fixture(region);
    let (status_sender, mut statuses) = mpsc::channel(1024);
    let start = Instant::now();
    let nearest_count = fixture.mesh_objects.len().div_ceil(10);
    let mut nearest_objects = fixture.mesh_objects.clone();
    nearest_objects.sort_unstable_by(|left, right| left.1.total_cmp(&right.1));
    let nearest_ids = nearest_objects.iter().take(nearest_count).map(|(id, _)| *id).collect::<HashSet<_>>();
    let mut demands = HashMap::<render::scene::AssetTicket, TrackedDemand>::new();
    let mut texture_ready = HashSet::new();
    let mut sample = Sample::default();

    let completed = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            while let Ok((ticket, status)) = statuses.try_recv() {
                apply_status(ticket, status, &mut fixture, &mut demands, &mut texture_ready);
            }
            update_times(start, &fixture.view, &nearest_ids, fixture.mesh_objects.len(), &fixture.textures, &texture_ready, &mut sample);
            let metrics_ready = sample.nearest_10_ready.is_some() && sample.all_ready.is_some() && sample.textures_ready.is_some();
            let demands_ready = demands.values().all(|demand| matches!(demand._demand.status(), Status::Ready(_)));
            if metrics_ready && (!drain_demands || demands_ready) {
                break;
            }
            match fixture.scene.poll(Instant::now()) {
                ReconcileStep::Asset(AssetChange::Request(request)) => {
                    let demand = acquire(&handle, request.region, request.request, request.priority, request.importance).await;
                    let watcher = watch_demand(request.ticket, demand.clone(), status_sender.clone());
                    demands.insert(request.ticket, TrackedDemand { _demand: demand, watcher, request: request.request });
                }
                ReconcileStep::Asset(AssetChange::Release(ticket)) => {
                    if let Some(demand) = demands.remove(&ticket) {
                        demand.watcher.abort();
                    }
                }
                ReconcileStep::Asset(AssetChange::UpdateImportance { ticket, importance }) => {
                    demands.get(&ticket).expect("importance update has an active demand")._demand.update_importance(importance);
                }
                ReconcileStep::Read(job) => {
                    let response = read_world(&fixture.world, &job);
                    fixture.scene.complete_read(job, response, Instant::now());
                }
                ReconcileStep::Build(job) => fixture.scene.complete_mesh(job.run()),
                ReconcileStep::Publish(batch) => fixture.view.apply(batch).expect("apply scene batch"),
                ReconcileStep::Continue => {}
                ReconcileStep::Wait(deadline) => {
                    tokio::select! {
                        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {}
                        status = statuses.recv() => {
                            let (ticket, status) = status.expect("asset status channel stays open");
                            apply_status(ticket, status, &mut fixture, &mut demands, &mut texture_ready);
                        }
                    }
                }
                ReconcileStep::Idle => {
                    let (ticket, status) = statuses.recv().await.expect("asset status channel stays open");
                    apply_status(ticket, status, &mut fixture, &mut demands, &mut texture_ready);
                }
                ReconcileStep::Closed => panic!("scene closed during loading benchmark"),
            }
        }
    })
    .await;
    if completed.is_err() {
        let published_meshes = fixture
            .view
            .objects()
            .filter(|object| matches!(object.geometry(), render::scene::Geometry::Mesh(_)) && object.mesh().is_some())
            .count();
        let ready_demands = demands.values().filter(|demand| matches!(demand._demand.status(), Status::Ready(_))).count();
        let (queued, fetching, decoding, failed, stale) =
            demands.values().fold((0, 0, 0, 0, 0), |counts, demand| match demand._demand.status() {
                Status::Queued => (counts.0 + 1, counts.1, counts.2, counts.3, counts.4),
                Status::Fetching => (counts.0, counts.1 + 1, counts.2, counts.3, counts.4),
                Status::Decoding => (counts.0, counts.1, counts.2 + 1, counts.3, counts.4),
                Status::Failed { .. } => (counts.0, counts.1, counts.2, counts.3 + 1, counts.4),
                Status::StaleSession => (counts.0, counts.1, counts.2, counts.3, counts.4 + 1),
                Status::Ready(_) | Status::Refining(_) | Status::RefinementFailed { .. } => counts,
            });
        let (headers_ready, headers_total, lods_ready, lods_total, other_ready, other_total) =
            demands.values().fold((0, 0, 0, 0, 0, 0), |counts, demand| {
                let ready = usize::from(matches!(demand._demand.status(), Status::Ready(_)));
                match demand.request {
                    Request::MeshHeader(_) => (counts.0 + ready, counts.1 + 1, counts.2, counts.3, counts.4, counts.5),
                    Request::MeshLod(_) => (counts.0, counts.1, counts.2 + ready, counts.3 + 1, counts.4, counts.5),
                    _ => (counts.0, counts.1, counts.2, counts.3, counts.4 + ready, counts.5 + 1),
                }
            });
        let (headers_queued, headers_fetching, lods_queued, lods_fetching, other_queued, other_fetching) =
            demands.values().fold((0, 0, 0, 0, 0, 0), |counts, demand| {
                let queued = usize::from(matches!(demand._demand.status(), Status::Queued));
                let fetching = usize::from(matches!(demand._demand.status(), Status::Fetching));
                match demand.request {
                    Request::MeshHeader(_) => (counts.0 + queued, counts.1 + fetching, counts.2, counts.3, counts.4, counts.5),
                    Request::MeshLod(_) => (counts.0, counts.1, counts.2 + queued, counts.3 + fetching, counts.4, counts.5),
                    _ => (counts.0, counts.1, counts.2, counts.3, counts.4 + queued, counts.5 + fetching),
                }
            });
        eprintln!(
            "mesh_loading timeout: nearest={} all={} textures={}/{} meshes={}/{} headers={}/{} lods={}/{} other={}/{} demands={}/{} queued={} fetching={} decoding={} failed={} stale={} states=headers({},{}) lods({},{}) other({},{}) memory={:?} cache={:?}",
            sample.nearest_10_ready.is_some(),
            sample.all_ready.is_some(),
            texture_ready.len(),
            fixture.textures.len(),
            published_meshes,
            fixture.mesh_objects.len(),
            headers_ready,
            headers_total,
            lods_ready,
            lods_total,
            other_ready,
            other_total,
            ready_demands,
            demands.len(),
            queued,
            fetching,
            decoding,
            failed,
            stale,
            headers_queued,
            headers_fetching,
            lods_queued,
            lods_fetching,
            other_queued,
            other_fetching,
            handle.memory_stats(),
            handle.cache_stats(),
        );
        for (_, demand) in demands.drain() {
            demand.watcher.abort();
        }
        drop(handle);
        cancel.send(()).unwrap();
        service.stop().await;
        let (requests, bytes) = peer.finish_stats();
        eprintln!("mesh_loading timeout network: requests={} bytes={}", requests.len(), bytes);
        panic!("mesh loading benchmark exceeded the hang guard");
    }

    for (_, demand) in demands.drain() {
        demand.watcher.abort();
    }
    drop(handle);
    cancel.send(()).unwrap();
    service.stop().await;
    let (requests, bytes) = peer.finish_stats();
    sample.http_requests = u64::try_from(requests.len()).unwrap();
    sample.http_bytes = bytes;
    sample
}

async fn acquire(handle: &AssetHandle, region: Region, request: Request, priority: assets::Priority, importance: u16) -> Demand {
    loop {
        match handle.request_with_importance(region, request, priority, importance).await {
            Ok(demand) => return demand,
            Err(RequestError::Busy) => tokio::task::yield_now().await,
            Err(error) => panic!("benchmark asset demand failed: {error:?}"),
        }
    }
}

fn watch_demand(
    ticket: render::scene::AssetTicket,
    mut demand: Demand,
    sender: mpsc::Sender<(render::scene::AssetTicket, Status)>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let initial = demand.status();
        let mut status = initial;
        loop {
            let terminal = matches!(&status, Status::Failed { .. } | Status::StaleSession);
            if status.content().is_some() || terminal {
                if sender.send((ticket, status.clone())).await.is_err() {
                    return;
                }
                if terminal || matches!(&status, Status::Ready(_)) {
                    return;
                }
            }
            status = demand.changed().await;
        }
    })
}

fn apply_status(
    ticket: render::scene::AssetTicket,
    status: Status,
    fixture: &mut SceneFixture,
    demands: &mut HashMap<render::scene::AssetTicket, TrackedDemand>,
    texture_ready: &mut HashSet<AssetId>,
) {
    let Some(demand) = demands.get_mut(&ticket) else { return };
    if let Request::Texture { id, .. } = demand.request
        && status.content().is_some_and(|content| matches!(content, Content::Texture(_)))
    {
        texture_ready.insert(id);
    }
    fixture.scene.complete_asset(ticket, status);
}

fn update_times(
    start: Instant,
    view: &SceneView,
    nearest_ids: &HashSet<Uuid>,
    total_meshes: usize,
    expected_textures: &HashSet<AssetId>,
    ready_textures: &HashSet<AssetId>,
    sample: &mut Sample,
) {
    let mut ready_meshes = HashSet::new();
    for object in view.objects() {
        if matches!(object.geometry(), render::scene::Geometry::Mesh(_)) && object.mesh().is_some() {
            ready_meshes.insert(object.detail().summary.id);
        }
    }
    let elapsed = start.elapsed();
    if sample.nearest_10_ready.is_none() && nearest_ids.iter().all(|id| ready_meshes.contains(id)) {
        sample.nearest_10_ready = Some(elapsed);
    }
    if sample.all_ready.is_none() && ready_meshes.len() == total_meshes {
        sample.all_ready = Some(elapsed);
    }
    if sample.textures_ready.is_none() && expected_textures.is_subset(ready_textures) {
        sample.textures_ready = Some(elapsed);
    }
}

fn read_world(world: &World, job: &ReadJob) -> Result<ReadReply, ReadFailure> {
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

fn scene_fixture(region: Region) -> SceneFixture {
    let capacity = MESH_OBJECTS + PRIM_OBJECTS;
    let tombstones_per_region = capacity.max(2048);
    let tombstones = tombstones_per_region * 2;
    let mut world = World::new(WorldLimits {
        max_region_registrations: 40,
        max_entities: capacity,
        max_entities_per_region: capacity,
        max_tombstones_per_region: tombstones_per_region,
        max_tombstones: tombstones,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(tombstones_per_region),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(tombstones),
        max_journal_entries: 4096,
        max_journal_changes: 8192,
        max_region_name_bytes: 256,
        max_recovery_targets: 8,
    });
    drop(world.adopt_region(region).expect("register benchmark region"));
    drop(
        world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).expect("set benchmark region origin"),
    );
    let textures = (0..TEXTURE_IDS).map(|slot| AssetId::new(Uuid::from_u128(MESH_TEXTURE_BASE + slot as u128))).collect::<Vec<_>>();
    let mut texture_set = textures.iter().copied().collect::<HashSet<_>>();
    let mut mesh_objects = Vec::with_capacity(MESH_OBJECTS);
    let mut order = 1_u64;

    for index in 0..PRIM_OBJECTS {
        let id = Uuid::from_u128(u128::try_from(index + 1).unwrap());
        let texture = AssetId::new(Uuid::from_u128(PRIM_TEXTURE_BASE + u128::try_from(index).unwrap()));
        texture_set.insert(texture);
        let mut object = observation(region, id, u32::try_from(index + 1).unwrap(), order, 40.0 + index as f32);
        object.shape = FieldUpdate::Set(Shape::Prim(world::PrimShape::default()));
        object.surface_appearance = Some(surface_appearance(texture, &[]));
        drop(world.observe_object(object).expect("add benchmark prim"));
        order += 1;
    }

    for index in 0..MESH_OBJECTS {
        let id = Uuid::from_u128(1000 + u128::try_from(index).unwrap());
        let asset_index = index / (MESH_OBJECTS / MESH_ASSETS);
        let asset_id = AssetId::new(Uuid::from_u128(MESH_ASSET_BASE + u128::try_from(asset_index).unwrap()));
        let mut object = observation(region, id, u32::try_from(PRIM_OBJECTS + index + 1).unwrap(), order, (MESH_OBJECTS - index) as f32);
        object.shape = FieldUpdate::Set(Shape::Mesh { asset_id, flags: SculptFlags::default() });
        object.surface_appearance = Some(surface_appearance(textures[0], &textures[1..]));
        drop(world.observe_object(object).expect("add benchmark mesh"));
        mesh_objects.push((id, (MESH_OBJECTS - index) as f32));
        order += 1;
    }

    let (mut scene, view) = Reconciler::new(render::scene::Budget::default());
    scene.set_camera(Camera::new(DVec3::ZERO, Vec3::X, std::f64::consts::FRAC_PI_3, [1000, 1000]).expect("benchmark camera"));
    SceneFixture { world, scene, view, mesh_objects, textures: texture_set }
}

fn observation(region: Region, id: Uuid, local_id: u32, order: u64, x: f32) -> ObjectObservation {
    let mut object = ObjectObservation::new(
        UpdateSource::new(region, order as u32, OrderEvidence::Ordered(order), 0),
        id,
        NonZeroU32::new(local_id).expect("benchmark local ID is nonzero"),
        Kind::Object,
        ParentUpdate::Root,
    );
    object.pose = Some(Pose { position: Some(Vec3::new(x, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
    object.scale = Some(Vec3::splat(3.0));
    object.appearance = FieldUpdate::Set(Appearance::Surface);
    object
}

fn surface_appearance(default_texture: AssetId, textures: &[AssetId]) -> SurfaceAppearance {
    let overrides = textures
        .iter()
        .enumerate()
        .map(|(index, texture)| (u8::try_from(index + 1).unwrap(), FaceAppearance { texture: Some(*texture), ..FaceAppearance::default() }))
        .collect();
    SurfaceAppearance::new(FaceAppearance { texture: Some(default_texture), ..FaceAppearance::default() }, overrides)
        .expect("valid benchmark texture entry")
}

fn mesh_assets() -> Arc<HashMap<Uuid, Vec<u8>>> {
    Arc::new(
        (0..MESH_ASSETS)
            .map(|index| {
                let id = Uuid::from_u128(MESH_ASSET_BASE + u128::try_from(index).unwrap());
                let lods =
                    if index < MESH_ASSETS / 4 { [(0, 4), (1, 8), (2, 64), (3, 512)] } else { [(0, 16), (1, 64), (2, 512), (3, 2048)] };
                (id, test_support::mesh::asset_with_seeded_lod_vertices(&lods, TEXTURE_IDS, index as u64 + 17))
            })
            .collect(),
    )
}

fn grants(base: &Url) -> Vec<u8> {
    xml::to_vec(&Value::Map(
        [
            ("ViewerAsset".into(), Value::String(base.join("content").expect("build asset capability URL").to_string())),
            ("RenderMaterials".into(), Value::String(base.join("materials").expect("build materials capability URL").to_string())),
        ]
        .into(),
    ))
    .expect("serialize benchmark capabilities")
}

fn respond(request: &[u8], base: &Url, mesh_assets: &HashMap<Uuid, Vec<u8>>) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let request = String::from_utf8_lossy(request);
    let target = request.lines().next().and_then(|line| line.split_whitespace().nth(1)).unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path == "/seed" {
        return (200, Vec::new(), grants(base));
    }
    if path == "/content/" && query.starts_with("texture_id=") {
        return (200, Vec::new(), test_support::texture::RGBA_J2C.to_vec());
    }
    if path != "/content/" || !query.starts_with("mesh_id=") {
        return (404, Vec::new(), Vec::new());
    }
    let Some((_, id)) = query.split_once('=') else { return (400, Vec::new(), Vec::new()) };
    let Ok(id) = Uuid::parse_str(id) else { return (400, Vec::new(), Vec::new()) };
    let Some(asset) = mesh_assets.get(&id) else { return (404, Vec::new(), Vec::new()) };
    let Some(range) = parse_range(request.as_bytes()) else { return (416, Vec::new(), Vec::new()) };
    let end = range.end.min(asset.len());
    let Some(bytes) = asset.get(range.start..end) else { return (416, Vec::new(), Vec::new()) };
    if bytes.is_empty() {
        return (416, Vec::new(), Vec::new());
    }
    let content_range = format!("bytes {}-{}/{}", range.start, end - 1, asset.len());
    (206, vec![("Content-Range".into(), content_range)], bytes.to_vec())
}

fn parse_range(request: &[u8]) -> Option<std::ops::Range<usize>> {
    let request = String::from_utf8_lossy(request);
    let value = request
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("range").then_some(value.trim())
        })?
        .strip_prefix("bytes=")?;
    let (start, end) = value.split_once('-')?;
    let start = usize::try_from(start.parse::<u64>().ok()?).ok()?;
    let end = usize::try_from(end.parse::<u64>().ok()?).ok()?.checked_add(1)?;
    Some(start..end)
}
