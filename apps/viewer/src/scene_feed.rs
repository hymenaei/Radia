use std::{
    collections::{HashMap, HashSet},
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
use math::motion::{self as kinematics};
use network::CircuitMotionStatus;
use render::scene::{
    ApplyError, AssetChange, AssetRequest, AssetTicket, Batch, Budget, Camera, MotionCircuit, Object, ReadFailure, ReadJob, ReadQuery,
    ReadReply, ReconcileStep, Reconciler, SceneView,
};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};
use tracing::Instrument;
use uuid::Uuid;
use world::{Entity, MAX_OBJECT_PAGE_ITEMS, MAX_REGION_PAGE_ITEMS, MAX_TERRAIN_PAGE_PATCHES, MAX_WORLD_CHANGE_PAGE_ITEMS, WorldPose};

use crate::{
    agent_movement::MovementObservation,
    asset_coordinator::{AssetHandle, RequestError},
    session::{AgentMotionSample, EnvironmentInbox},
    world_reader::{ReadError, WorldReader},
};

const MIN_ACTIVE_MOTION_SQUARED: f32 = 1e-6;

#[derive(Default)]
struct DeadReckoning {
    sample: Option<AgentMotionSample>,
    trajectory: Option<kinematics::LinearTrajectory>,
}

impl DeadReckoning {
    fn replace(&mut self, sample: Option<AgentMotionSample>) {
        match sample {
            Some(sample) if sample.is_finite() => {
                let lead = kinematics::half_ping_lead(
                    f64::from(sample.time_dilation),
                    sample.ping_delay,
                    kinematics::FRAME_INTERVAL.as_secs_f64(),
                );
                self.trajectory = Some(kinematics::LinearTrajectory::new(
                    sample.position + sample.velocity.as_dvec3() * lead,
                    sample.velocity,
                    sample.acceleration,
                    f64::from(sample.time_dilation),
                    sample.received_at,
                ));
                self.sample = Some(sample);
            }
            Some(_) => {}
            None => {
                self.sample = None;
                self.trajectory = None;
            }
        }
    }

    fn start_phase_out(&mut self, at: Option<Instant>) {
        let Some(sample) = self.sample else { return };
        if let Some(trajectory) = &mut self.trajectory {
            let phase_out_at = at.map_or(sample.received_at, |at| at.max(sample.received_at));
            trajectory.start_phase_out(phase_out_at.max(sample.received_at + Duration::from_secs_f64(kinematics::PHASE_OUT_TIME)));
        }
    }

    fn resume_at(&mut self, now: Instant, regions: &[kinematics::RegionBounds]) {
        let Some(sample) = self.sample else { return };
        if let Some(trajectory) = &mut self.trajectory {
            trajectory.resume_at(sample.region.get(), now, regions);
        }
    }

    fn pose(&self, now: Instant, regions: &[kinematics::RegionBounds]) -> Option<WorldPose> {
        let sample = self.sample?;
        let position = self.trajectory?.position_at(sample.region.get(), now, regions);
        Some(sample.pose(position))
    }

    fn active(&self, now: Instant) -> bool {
        self.sample.is_some_and(|sample| {
            !self.trajectory.is_some_and(|trajectory| trajectory.is_phase_out_settled(now))
                && (sample.velocity.length_squared() > MIN_ACTIVE_MOTION_SQUARED
                    || sample.acceleration.length_squared() > MIN_ACTIVE_MOTION_SQUARED)
        })
    }
}

pub(crate) struct SceneSources {
    pub(crate) reader: WorldReader,
    pub(crate) assets: AssetHandle,
    pub(crate) environments: EnvironmentInbox,
    pub(crate) motion_circuits: watch::Receiver<Vec<(world::Region, watch::Receiver<CircuitMotionStatus>)>>,
}

pub(crate) struct SceneFeed {
    pub(crate) runtime: Handle,
    pub(crate) view: SceneView,
    pub(crate) budget: Budget,
    pub(crate) assets: AssetHandle,
    agent_id: Uuid,
    agent: Option<Entity>,
    pub(crate) water_height: Option<f64>,
    camera: watch::Sender<Option<Camera>>,
    movement: watch::Sender<Option<MovementObservation>>,
    agent_motion: watch::Receiver<Option<AgentMotionSample>>,
    motion_circuits: watch::Receiver<Vec<(world::Region, watch::Receiver<CircuitMotionStatus>)>>,
    motion_circuit_scratch: Vec<MotionCircuit>,
    motion_region_scratch: Vec<kinematics::RegionBounds>,
    motion_active: HashMap<world::Region, bool>,
    dead_reckoning: DeadReckoning,
    last_movement: Option<MovementObservation>,
    incoming: mpsc::Receiver<Batch>,
    _cancel: oneshot::Sender<()>,
}

impl SceneFeed {
    pub(crate) fn start(
        sources: SceneSources,
        agent_id: Uuid,
        camera: watch::Sender<Option<Camera>>,
        movement: watch::Sender<Option<MovementObservation>>,
        mut agent_motion: watch::Receiver<Option<AgentMotionSample>>,
        runtime: Handle,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let assets = sources.assets.clone();
        let motion_circuits = sources.motion_circuits.clone();
        static MEMORY: OnceLock<Budget> = OnceLock::new();
        let budget = MEMORY.get_or_init(Budget::default).clone();
        let (scene, view) = Reconciler::new(budget.clone());
        let (sender, incoming) = mpsc::channel(1);
        let (cancel, cancelled) = oneshot::channel();
        let observations = camera.subscribe();
        let agent_motions = agent_motion.clone();
        let mut dead_reckoning = DeadReckoning::default();
        dead_reckoning.replace(*agent_motion.borrow_and_update());
        drop(
            runtime
                .clone()
                .spawn(run(scene, sources, sender, runtime.clone(), cancelled, observations, agent_motions, wake).in_current_span()),
        );
        Self {
            runtime,
            view,
            budget,
            assets,
            agent_id,
            agent: None,
            water_height: None,
            camera,
            movement,
            agent_motion,
            motion_circuits,
            motion_circuit_scratch: Vec::new(),
            motion_region_scratch: Vec::new(),
            motion_active: HashMap::new(),
            dead_reckoning,
            last_movement: None,
            incoming,
            _cancel: cancel,
        }
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) fn camera_pose(&mut self) -> Option<WorldPose> {
        self.camera_pose_at(Instant::now())
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) fn camera_pose_at(&mut self, now: Instant) -> Option<WorldPose> {
        self.camera_pose_and_seed_at(now).map(|(pose, _)| pose)
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) fn camera_pose_and_seed_at(&mut self, now: Instant) -> Option<(WorldPose, (u64, math::Quat))> {
        if self.agent_motion.has_changed().is_ok_and(|changed| changed) {
            self.dead_reckoning.replace(*self.agent_motion.borrow_and_update());
        }
        self.refresh_motion_circuits(now);
        let sample = self.dead_reckoning.sample?;
        let mut pose = self.dead_reckoning.pose(now, &self.motion_region_scratch)?;
        if !sample.parented
            && let Some(ground) = self.view.terrain_height_at(pose.position)
        {
            pose.position.z = pose.position.z.max(ground + 0.5 * f64::from(sample.scale.z));
        }
        Some((pose, (sample.seed, sample.simulator_rotation)))
    }

    pub(crate) fn agent_id(&self) -> Uuid {
        self.agent_id
    }

    pub(crate) fn agent_motion_active(&self, now: Instant) -> bool {
        self.dead_reckoning.active(now)
    }

    pub(crate) fn refresh_motion_circuits(&mut self, now: Instant) {
        self.motion_region_scratch.clear();
        self.motion_region_scratch.extend(self.view.regions().filter_map(|region| {
            region
                .summary()
                .info
                .resolved_origin()
                .map(|origin| kinematics::RegionBounds { region_id: region.summary().region.get(), origin })
        }));
        self.motion_circuit_scratch.clear();
        {
            let circuits = self.motion_circuits.borrow_and_update();
            self.motion_circuit_scratch.extend(circuits.iter().map(|(region, receiver)| {
                let status = *receiver.borrow();
                MotionCircuit { region: *region, last_packet_received_at: status.last_packet_received_at, connected: status.connected }
            }));
        }
        self.view.set_motion_circuits(&self.motion_circuit_scratch);
    }

    fn phase_out_for(&self, region: world::Region, sampled_at: Instant, now: Instant) -> bool {
        let status =
            self.motion_circuits.borrow().iter().find(|(candidate, _)| *candidate == region).map(|(_, receiver)| *receiver.borrow());
        status.is_none_or(|status| kinematics::phase_out_needed(sampled_at, now, status.last_packet_received_at, status.connected))
    }

    pub(crate) fn set_camera(&self, camera: Option<Camera>) {
        self.camera.send_if_modified(|previous| {
            if *previous == camera {
                false
            } else {
                *previous = camera;
                true
            }
        });
    }

    pub(crate) fn set_movement(&mut self, movement: MovementObservation) {
        let changed = self.last_movement.is_none_or(|previous| {
            previous.control_flags != movement.control_flags
                || previous.always_run != movement.always_run
                || previous.stop_sequence != movement.stop_sequence
                || previous.body_rotation.dot(movement.body_rotation).abs() < crate::agent_movement::BODY_ROTATION_DOT_THRESHOLD
        });
        if changed {
            self.last_movement = Some(movement);
            self.movement.send_if_modified(|previous| {
                if *previous == Some(movement) {
                    false
                } else {
                    *previous = Some(movement);
                    true
                }
            });
        }
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
                    _ => assets::Failure::Network,
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
    mut agent_motion: watch::Receiver<Option<AgentMotionSample>>,
    wake: impl Fn(),
) {
    let SceneSources { mut reader, assets, environments, motion_circuits: _ } = sources;
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
            let motion_updated = agent_motion.changed().fuse();
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
            pin_mut!(reading, building, changed, moved, motion_updated, available, released, timer, pushed);
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
                result = motion_updated => { if result.is_err() { break; } wake(); }
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
#[path = "scene_feed_tests.rs"]
mod tests;
