use std::{
    collections::HashMap,
    future::Future,
    sync::OnceLock,
    time::{Duration, Instant},
};

use futures::{
    FutureExt, StreamExt,
    channel::oneshot,
    future::{AbortHandle, Abortable, BoxFuture, pending},
    pin_mut,
    stream::FuturesUnordered,
};
use render::scene::{
    ApplyError, AssetChange, AssetRequest, AssetTicket, Batch, Budget, Camera, Object, ReadFailure, ReadJob, ReadQuery, ReadReply,
    ReconcileStep, Reconciler, SceneView,
};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};
use tracing::Instrument;
use uuid::Uuid;
use world::{Entity, MAX_OBJECT_PAGE_ITEMS, MAX_REGION_PAGE_ITEMS, MAX_TERRAIN_PAGE_PATCHES, MAX_WORLD_CHANGE_PAGE_ITEMS};

use crate::{
    asset_coordinator::{AssetHandle, RequestError},
    session::EnvironmentInbox,
    world_reader::{ReadError, WorldReader},
};

pub(crate) struct SceneSources {
    pub(crate) reader: WorldReader,
    pub(crate) assets: AssetHandle,
    pub(crate) environments: EnvironmentInbox,
}

pub(crate) struct SceneFeed {
    pub(crate) runtime: Handle,
    pub(crate) view: SceneView,
    pub(crate) budget: Budget,
    agent_id: Uuid,
    agent: Option<Entity>,
    pub(crate) water_height: Option<f64>,
    camera: watch::Sender<Option<Camera>>,
    incoming: mpsc::Receiver<Batch>,
    _cancel: oneshot::Sender<()>,
}

impl SceneFeed {
    pub(crate) fn start(
        sources: SceneSources,
        agent_id: Uuid,
        camera: watch::Sender<Option<Camera>>,
        runtime: Handle,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        static MEMORY: OnceLock<Budget> = OnceLock::new();
        let budget = MEMORY.get_or_init(Budget::default).clone();
        let (scene, view) = Reconciler::new(budget.clone());
        let (sender, incoming) = mpsc::channel(1);
        let (cancel, cancelled) = oneshot::channel();
        let observations = camera.subscribe();
        drop(runtime.clone().spawn(run(scene, sources, sender, runtime.clone(), cancelled, observations, wake).in_current_span()));
        Self { runtime, view, budget, agent_id, agent: None, water_height: None, camera, incoming, _cancel: cancel }
    }

    pub(crate) fn agent(&self) -> Option<&Object> {
        self.agent.and_then(|entity| self.view.object(entity))
    }

    pub(crate) fn set_camera(&self, camera: Camera) {
        self.camera.send_if_modified(|previous| {
            if *previous == Some(camera) {
                false
            } else {
                *previous = Some(camera);
                true
            }
        });
    }

    pub(crate) fn drain(&mut self) -> Result<(), ApplyError> {
        if self.view.status() == render::scene::Status::Closed {
            self.agent = None;
            self.water_height = None;
            self.incoming.close();
            while let Ok(batch) = self.incoming.try_recv() {
                drop(batch);
            }
            return Ok(());
        }
        if let Ok(batch) = self.incoming.try_recv() {
            let before = self.view.status();
            self.view.apply(batch)?;
            let agent = self
                .agent
                .and_then(|entity| self.view.object(entity))
                .or_else(|| self.view.objects().find(|object| object.detail().summary.id == self.agent_id));
            self.agent = agent.map(|object| object.detail().summary.entity);
            self.water_height = agent.and_then(|agent| water_height(&self.view, agent));
            if self.view.status() != before {
                tracing::info!(status = ?self.view.status(), objects = self.view.object_count(), regions = self.view.region_count(), patches = self.view.patch_count(), revision = self.view.revision().map(|r| r.get()), "scene reconciliation status changed");
            }
        }
        Ok(())
    }
}

struct AssetStatusEvent {
    ticket: AssetTicket,
    status: assets::Status,
    demand: Option<assets::Demand>,
}

struct AssetSubscriptions {
    handle: AssetHandle,
    pending: FuturesUnordered<BoxFuture<'static, Option<AssetStatusEvent>>>,
    cancel: HashMap<AssetTicket, AbortHandle>,
}

impl AssetSubscriptions {
    fn new(handle: AssetHandle) -> Self {
        Self { handle, pending: FuturesUnordered::new(), cancel: HashMap::new() }
    }

    fn push(&mut self, ticket: AssetTicket, future: impl Future<Output = AssetStatusEvent> + Send + 'static) {
        let (cancel, registration) = AbortHandle::new_pair();
        self.cancel.insert(ticket, cancel);
        self.pending.push(async move { Abortable::new(future, registration).await.ok() }.boxed());
    }

    fn apply_change(&mut self, change: AssetChange) {
        match change {
            AssetChange::Request(request) => {
                let handle = self.handle.clone();
                self.push(request.ticket, acquire_asset_demand(handle, request));
            }
            AssetChange::Release(ticket) => {
                if let Some(cancel) = self.cancel.remove(&ticket) {
                    cancel.abort();
                }
            }
        }
    }

    async fn next(&mut self) -> Option<(AssetTicket, assets::Status)> {
        let event = if self.pending.is_empty() { pending().await } else { self.pending.next().await.flatten() };
        let AssetStatusEvent { ticket, status, demand } = event?;
        let terminal = matches!(
            &status,
            assets::Status::StaleSession
                | assets::Status::Failed { failure: assets::Failure::RouteClosed | assets::Failure::SessionClosed, .. }
        );
        self.resume(ticket, terminal, demand);
        Some((ticket, status))
    }

    fn resume(&mut self, ticket: AssetTicket, terminal: bool, demand: Option<assets::Demand>) {
        if !self.cancel.contains_key(&ticket) {
            return;
        }
        if terminal {
            self.cancel.remove(&ticket);
            return;
        }
        if let Some(mut demand) = demand {
            self.push(ticket, async move {
                let status = demand.changed().await;
                AssetStatusEvent { ticket, status, demand: Some(demand) }
            });
        } else {
            self.cancel.remove(&ticket);
        }
    }
}

fn water_height(view: &SceneView, agent: &Object) -> Option<f64> {
    let region = agent.detail().summary.address.region();
    let height = view.water_height(region)?;
    let origin = view.region(region)?.summary().info.resolved_origin()?;
    Some(origin.z + f64::from(height))
}

async fn acquire_asset_demand(handle: AssetHandle, request: AssetRequest) -> AssetStatusEvent {
    loop {
        match handle.request(request.region, request.request, request.priority).await {
            Ok(demand) => return AssetStatusEvent { ticket: request.ticket, status: demand.status(), demand: Some(demand) },
            Err(RequestError::Busy) => {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(error) => {
                let failure = match error {
                    RequestError::Closed | RequestError::Admission(assets::AdmissionError::Closed) => assets::Failure::SessionClosed,
                    RequestError::Admission(assets::AdmissionError::Route) => assets::Failure::RouteClosed,
                    _ => assets::Failure::Malformed,
                };
                return AssetStatusEvent {
                    ticket: request.ticket,
                    status: assets::Status::Failed { failure, retry_at: None },
                    demand: None,
                };
            }
        }
    }
}

async fn execute_world_read(reader: &WorldReader, job: &ReadJob) -> Result<ReadReply, ReadFailure> {
    let result = match job.query() {
        ReadQuery::Barrier => reader.barrier().await.map(ReadReply::Barrier),
        ReadQuery::Objects(cursor) => reader.objects(*cursor, MAX_OBJECT_PAGE_ITEMS).await.map(ReadReply::Objects),
        ReadQuery::Regions(cursor) => reader.regions(*cursor, MAX_REGION_PAGE_ITEMS).await.map(ReadReply::Regions),
        ReadQuery::Details(entities) => reader.details(entities).await.map(ReadReply::Details),
        ReadQuery::Changes(cursor) => reader.changes(*cursor, MAX_WORLD_CHANGE_PAGE_ITEMS).await.map(ReadReply::Changes),
        ReadQuery::Terrain(region, cursor) => reader.terrain(*region, *cursor, MAX_TERRAIN_PAGE_PATCHES).await.map(ReadReply::Terrain),
        ReadQuery::Environment(region) => reader.environment(*region).await.map(ReadReply::Environment),
    };
    result.map_err(|error| match error {
        ReadError::Busy => ReadFailure::Busy,
        ReadError::Shutdown | ReadError::StaleSession => ReadFailure::Closed,
        ReadError::World(error) => ReadFailure::World(error),
    })
}

#[tracing::instrument(name = "scene.feed", level = "debug", skip_all)]
async fn run(
    mut scene: Reconciler,
    sources: SceneSources,
    sender: mpsc::Sender<Batch>,
    runtime: Handle,
    cancelled: oneshot::Receiver<()>,
    mut camera: watch::Receiver<Option<Camera>>,
    wake: impl Fn(),
) {
    let SceneSources { mut reader, assets, environments } = sources;
    let mut environments = environments.take();
    let mut textures = assets.texture_events();
    let mut assets = AssetSubscriptions::new(assets);
    let cancelled = cancelled.fuse();
    pin_mut!(cancelled);
    let mut reads = FuturesUnordered::new();
    let mut meshes = FuturesUnordered::new();
    loop {
        if let Some(camera) = *camera.borrow_and_update() {
            scene.set_camera(camera);
        }
        let until = match scene.poll(Instant::now()) {
            ReconcileStep::Asset(change) => {
                assets.apply_change(change);
                Some(Instant::now())
            }
            ReconcileStep::Read(job) => {
                let reader = reader.clone();
                reads.push(async move {
                    let reply = execute_world_read(&reader, &job).await;
                    (job, reply)
                });
                Some(Instant::now())
            }
            ReconcileStep::Build(job) => {
                let span = tracing::Span::current();
                meshes.push(runtime.spawn_blocking(move || span.in_scope(|| job.run())));
                Some(Instant::now())
            }
            ReconcileStep::Publish(batch) => {
                match sender.try_send(batch) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        tracing::error!("scene publication exceeded its single-batch limit");
                        break;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                }
                wake();
                Some(Instant::now())
            }
            ReconcileStep::Continue => Some(Instant::now()),
            ReconcileStep::Idle => None,
            ReconcileStep::Wait(until) => Some(until),
            ReconcileStep::Closed => break,
        };
        let asset_event = {
            let reading = async { if reads.is_empty() { pending().await } else { reads.next().await.expect("nonempty reads") } }.fuse();
            let building =
                async { if meshes.is_empty() { pending().await } else { meshes.next().await.expect("nonempty mesh jobs") } }.fuse();
            let changed = reader.changed().fuse();
            let pushed = async {
                match &mut environments {
                    Some(receiver) => receiver.recv().await,
                    None => pending().await,
                }
            }
            .fuse();
            let moved = camera.changed().fuse();
            let available = assets.next().fuse();
            let released = textures.changed().fuse();
            let timer = async {
                match until {
                    Some(until) if until <= Instant::now() => tokio::task::yield_now().await,
                    Some(until) => tokio::time::sleep_until(tokio::time::Instant::from_std(until)).await,
                    None => pending().await,
                }
            }
            .fuse();
            pin_mut!(reading, building, changed, moved, available, released, timer, pushed);
            let mut asset_event = None;
            futures::select_biased! {
                _ = cancelled => break,
                command = pushed => match command {
                    Some(command) => if let Err(error) = scene.push_environment(command.region, command.push) { tracing::warn!(region = command.region.get(), ?error, "experience environment push rejected"); },
                    None => break,
                },
                (job, reply) = reading => {
                    if let Err(failure) = &reply && !matches!(failure, ReadFailure::Busy) {
                        tracing::debug!(?failure, "scene world read did not complete");
                    }
                    scene.complete_read(job, reply, Instant::now());
                },
                completion = building => match completion {
                    Ok(completion) => scene.complete_mesh(completion),
                    Err(error) => {
                        tracing::error!(%error, "scene mesh worker stopped unexpectedly");
                        break;
                    }
                },
                event = available => asset_event = event,

                result = released => { if result.is_err() { break; } wake(); }
                result = moved => { if result.is_err() { break; } }
                result = changed => { if result.is_err() { break; } scene.notify_world_change(); }
                _ = timer => {}
            }
            asset_event
        };
        if let Some((ticket, status)) = asset_event {
            scene.complete_asset(ticket, status);
        }
    }
    scene.close();
    wake();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset_coordinator::AssetCoordinator;
    use crate::world_reader::ReadRequest;
    use crate::world_reader::WorldReadHost;
    use futures::{Future, FutureExt, join};
    use math::{DVec3, Quat, Vec3};
    use render::scene::{EnvironmentProperty, EnvironmentPush, Status};
    use std::{
        future::{pending, ready},
        num::NonZeroU32,
        time::Duration,
    };
    use utils::{AssetId, ExperienceId};
    use uuid::Uuid;
    use world::{
        Appearance, FaceAppearance, FieldUpdate, Kind, ObjectObservation, ObjectPatch, OrderEvidence::Ordered, ParentUpdate, Pose, Region,
        RegionInfo, Shape, SurfaceAppearance, UpdateSource, World, WorldLimits,
    };

    async fn select_read<T>(service: &mut WorldReadHost, intake: impl Future<Output = T>) -> Result<T, ReadRequest> {
        if let Some(request) = service.take_reserved() {
            return Err(request);
        }
        let result = {
            let intake = intake.fuse();
            let request = service.next_request().fuse();
            pin_mut!(intake, request);
            futures::select_biased! {
                input = intake => Ok(input),
                request = request => Err(request),
            }
        };
        if result.is_ok() {
            service.record_intake_turn();
        }
        result
    }

    fn world() -> World {
        World::new(WorldLimits {
            max_region_registrations: 2,
            max_entities: 16,
            max_entities_per_region: 16,
            max_tombstones_per_region: 16,
            max_tombstones: 16,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(16),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(16),
            max_journal_entries: 32,
            max_journal_changes: 128,
            max_region_name_bytes: 64,
            max_recovery_targets: 8,
        })
    }

    #[tokio::test]
    async fn water_height_follows_agent_region() {
        let mut world = world();
        let agent = Uuid::from_u128(1);
        for region in [Region::new(1), Region::new(2)] {
            let _ = world.adopt_region(region).unwrap();
            let _ = world
                .patch_region(
                    region,
                    RegionInfo { origin: Some(DVec3::new((region.get() - 1) as f64 * 256.0, 0.0, 7.0)), ..RegionInfo::default() },
                )
                .unwrap();
        }
        let _ = world
            .observe_object(ObjectObservation::new(
                UpdateSource::new(Region::new(1), 1, Ordered(1), 0),
                agent,
                NonZeroU32::new(1).unwrap(),
                Kind::Avatar,
                ParentUpdate::Root,
            ))
            .unwrap();
        let _ = world.observe_water_height(UpdateSource::new(Region::new(2), 1, Ordered(1), 0), 31.0).unwrap();
        let budget = Budget::default();
        let (mut scene, view) = Reconciler::new(budget.clone());
        let (sender, incoming) = mpsc::channel(1);
        let (cancel, _) = oneshot::channel();
        let (camera, _) = watch::channel(None);
        let mut feed = SceneFeed {
            runtime: Handle::current(),
            view,
            budget,
            agent_id: agent,
            agent: None,
            water_height: None,
            camera,
            incoming,
            _cancel: cancel,
        };
        let mut hydrate = |world: &World| {
            scene.notify_world_change();
            let now = Instant::now();
            for _ in 0..1000 {
                match scene.poll(now) {
                    ReconcileStep::Read(job) => {
                        let reply = match job.query() {
                            ReadQuery::Barrier => Ok(ReadReply::Barrier(world.read_barrier())),
                            ReadQuery::Objects(cursor) => world.object_list_page(*cursor, 256).map(ReadReply::Objects),
                            ReadQuery::Regions(cursor) => world.region_page(*cursor, 64).map(ReadReply::Regions),
                            ReadQuery::Details(entities) => world.object_detail_batch(entities).map(ReadReply::Details),
                            ReadQuery::Changes(cursor) => world.change_page(*cursor, 256).map(ReadReply::Changes),
                            ReadQuery::Terrain(region, cursor) => world.terrain_page(*region, *cursor, 8).map(ReadReply::Terrain),
                            ReadQuery::Environment(region) => Ok(ReadReply::Environment(world.environment(*region))),
                        };
                        scene.complete_read(job, reply.map_err(ReadFailure::World), now);
                    }
                    ReconcileStep::Publish(batch) => {
                        sender.try_send(batch).unwrap();
                        feed.drain().unwrap();
                    }
                    ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
                    ReconcileStep::Continue => {}
                    ReconcileStep::Idle => return (feed.water_height, feed.agent().map(|object| object.detail().summary.entity)),
                    _ => panic!("expected scene reconciliation"),
                }
            }
            panic!("scene did not settle");
        };
        let (height, original) = hydrate(&world);
        assert_eq!(height, None, "another region's water must not supply the water height");
        let original = original.expect("the observed avatar is available before its water height");
        let _ = world.observe_water_height(UpdateSource::new(Region::new(1), 2, Ordered(2), 0), 17.0).unwrap();
        assert_eq!(hydrate(&world), (Some(24.0), Some(original)));
        let _ = world.retire_region(Region::new(1)).unwrap();
        assert_eq!(hydrate(&world), (None, None), "retirement must remove both the avatar and previous water height");
        let _ = world
            .observe_object(ObjectObservation::new(
                UpdateSource::new(Region::new(2), 3, Ordered(3), 0),
                agent,
                NonZeroU32::new(1).unwrap(),
                Kind::Avatar,
                ParentUpdate::Root,
            ))
            .unwrap();
        let (height, replacement) = hydrate(&world);
        assert_eq!(height, Some(38.0), "the new avatar region supplies its own observed height");
        assert_ne!(replacement.expect("the replacement avatar is available"), original, "a retired entity must not remain cached");
    }

    #[tokio::test]
    async fn pending_asset_preserves_scene() {
        let mut world = world();
        let region = Region::new(1);
        let _ = world.adopt_region(region).unwrap();
        let _ = world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).unwrap();
        let mut object = ObjectObservation::new(
            UpdateSource::new(region, 1, Ordered(1), 0),
            Uuid::from_u128(1),
            NonZeroU32::new(1).unwrap(),
            Kind::Object,
            ParentUpdate::Root,
        );
        object.pose = Some(Pose { position: Some(Vec3::new(10.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
        object.scale = Some(Vec3::ONE);
        object.shape = FieldUpdate::Set(Shape::Prim(Default::default()));
        object.appearance = FieldUpdate::Set(Appearance::Surface);
        object.surface_appearance = Some(
            SurfaceAppearance::new(
                FaceAppearance { texture: Some(AssetId::new(Uuid::from_u128(100))), color: [255, 0, 0, 255], ..FaceAppearance::default() },
                Vec::new(),
            )
            .unwrap(),
        );
        let _ = world.observe_object(object).unwrap();
        let mut reads = WorldReadHost::new(&world, Handle::current());
        let assets = AssetCoordinator::new(Handle::current(), pending());
        let (_pushes, environments) = crate::session::EnvironmentInbox::channel();
        let mut feed = SceneFeed::start(
            SceneSources { reader: reads.reader(), environments, assets: assets.handle() },
            Uuid::from_u128(2),
            watch::channel(None).0,
            Handle::current(),
            || {},
        );
        feed.set_camera(Camera::new(DVec3::ZERO, Vec3::X, 1.0, [1000, 1000]).unwrap());
        let receiving = async {
            loop {
                let batch = feed.incoming.recv().await.expect("scene remains active");
                feed.view.apply(batch).unwrap();
                if let Some(face) = feed.view.objects().next().and_then(|object| object.materials()).and_then(|faces| faces.first())
                    && let Some(binding) = &face.base_color_texture
                    && binding.state().resolution == render::material::Resolution::Pending
                {
                    assert_eq!(face.base_color_factor(), [1.0, 0.0, 0.0, 1.0]);
                    assert!(binding.image().is_none());
                    assert_eq!(face.alpha, render::material::AlphaBehavior::Unknown);
                    break;
                }
            }
        }
        .fuse();
        let serving = async {
            loop {
                let Err(request) = select_read(&mut reads, pending::<()>()).await else { unreachable!() };
                request.serve(&world);
            }
        }
        .fuse();
        pin_mut!(receiving, serving);
        futures::select_biased! {
            _ = receiving => {}
            _ = serving => unreachable!(),
        }
    }

    #[tokio::test]
    async fn stalled_publication_preserves_intake() {
        let mut world = world();
        let region = Region::new(1);
        let _ = world.adopt_region(region).unwrap();
        let _ = world
            .observe_object(ObjectObservation::new(
                UpdateSource::new(region, 1, Ordered(1), 0),
                Uuid::from_u128(1),
                NonZeroU32::new(1).unwrap(),
                Kind::Object,
                ParentUpdate::Root,
            ))
            .unwrap();
        let mut service = WorldReadHost::new(&world, Handle::current());
        let reader = service.reader();
        let assets = AssetCoordinator::new(Handle::current(), pending());
        let (pushes, environments) = crate::session::EnvironmentInbox::channel();
        let mut feed = SceneFeed::start(
            SceneSources { reader: reader.clone(), environments, assets: assets.handle() },
            Uuid::from_u128(1),
            watch::channel(None).0,
            Handle::current(),
            || {},
        );
        let batch = {
            let receiving = async {
                loop {
                    let batch = feed.incoming.recv().await.expect("live Scene publication");
                    if batch.status() == Status::Ready {
                        break batch;
                    }
                    feed.view.apply(batch).unwrap();
                }
            }
            .fuse();
            let serving = async {
                loop {
                    let Err(request) = select_read(&mut service, pending::<()>()).await else { unreachable!() };
                    request.serve(&world);
                }
            }
            .fuse();
            pin_mut!(receiving, serving);
            futures::select_biased! {
                batch = receiving => batch,
                _ = serving => unreachable!(),
            }
        };
        assert_eq!(feed.view.status(), Status::Loading);
        let initial = world.revision();
        let (barrier, turns) = join!(reader.barrier(), async {
            let mut turns = 0;
            while turns < 1024 {
                match select_read(&mut service, ready(())).await {
                    Ok(()) => {
                        turns += 1;
                        let mut patch = ObjectPatch::new(
                            UpdateSource::new(region, turns + 1, Ordered(u64::from(turns + 1)), 0),
                            NonZeroU32::new(1).unwrap(),
                        );
                        patch.pose = Some(Pose { position: Some(Vec3::new(turns as f32, 0.0, 0.0)), rotation: None });
                        let _ = world.patch_object(patch).unwrap();
                        service.publish(&world);
                    }
                    Err(request) => request.serve(&world),
                }
            }
            turns
        });
        assert_eq!(turns, 1024);
        assert!(barrier.unwrap().revision > initial);
        assert!(world.revision() > initial);
        let experience = ExperienceId::new(Uuid::from_u128(44)).unwrap();
        let push = EnvironmentPush::Partial {
            experience,
            sky: vec![EnvironmentProperty { name: "gamma".into(), source: b"r1.4".as_slice().into() }],
            water: Vec::new(),
            transition: Duration::ZERO,
        };
        assert!(pushes.send(crate::session::EnvironmentCommand { region, push }).await.is_ok());
        feed.view.apply(batch).unwrap();
        assert_eq!(feed.view.object_count(), 1);
        {
            let receiving = async {
                loop {
                    let batch = feed.incoming.recv().await.expect("live Scene publication");
                    feed.view.apply(batch).unwrap();
                    if let Some(environment) = feed.view.viewer_environment() {
                        assert_eq!(environment.sky_properties()["gamma"].experience(), experience);
                        break;
                    }
                }
            }
            .fuse();
            let serving = async {
                loop {
                    let Err(request) = select_read(&mut service, pending::<()>()).await else { unreachable!() };
                    request.serve(&world);
                }
            }
            .fuse();
            {
                pin_mut!(receiving, serving);
                futures::select_biased! { _ = receiving => {}, _ = serving => unreachable!() }
            }
        }
        service.finish();
        assert!(feed.incoming.recv().await.is_none());
        assert_eq!(feed.view.status(), Status::Closed);
    }
}
