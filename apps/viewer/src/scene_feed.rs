use std::{
    collections::HashMap,
    sync::OnceLock,
    time::{Duration, Instant},
};

use base::UUID;
use futures::{
    FutureExt, StreamExt,
    channel::oneshot,
    future::{AbortHandle, Abortable, BoxFuture, pending},
    pin_mut,
    stream::FuturesUnordered,
};
use scene::{
    ApplyError, AssetChange, AssetRequest, AssetTicket, Batch, Budget, Camera, ReadFailure, ReadJob, ReadQuery, ReadReply, ReconcileStep,
    SceneReconciler, SceneView,
};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};
use world::{MAX_OBJECT_PAGE_ITEMS, MAX_REGION_PAGE_ITEMS, MAX_TERRAIN_PAGE_PATCHES, MAX_WORLD_CHANGE_PAGE_ITEMS};

use crate::{
    asset_coordinator::{AssetHandle, RequestError},
    session::EnvironmentInbox,
    world_reader::{ReadError, WorldReader},
};

#[cfg(test)]
#[path = "scene_feed_tests.rs"]
mod tests;

pub(crate) struct SceneSources {
    pub(crate) reader: WorldReader,
    pub(crate) assets: AssetHandle,
    pub(crate) environments: EnvironmentInbox,
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

    async fn next(&mut self) -> Option<AssetStatusEvent> {
        if self.pending.is_empty() { pending().await } else { self.pending.next().await.flatten() }
    }

    fn resume(&mut self, mut event: AssetStatusEvent) {
        if !self.cancel.contains_key(&event.ticket) {
            return;
        }
        if matches!(event.status, assets::Status::StaleSession) {
            self.cancel.remove(&event.ticket);
            return;
        }
        if let Some(mut demand) = event.demand.take() {
            self.push(event.ticket, async move {
                let status = demand.changed().await;
                AssetStatusEvent { ticket: event.ticket, status, demand: Some(demand) }
            });
        } else {
            self.cancel.remove(&event.ticket);
        }
    }
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
                    RequestError::NoRoute | RequestError::Admission(assets::AdmissionError::Route) => assets::Failure::RouteClosed,
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

pub(crate) struct SceneFeed {
    pub(crate) runtime: Handle,
    pub(crate) view: SceneView,
    pub(crate) budget: Budget,
    pub(crate) agent_id: UUID,
    pub(crate) water_height: Option<f64>,
    camera: watch::Sender<Option<Camera>>,
    incoming: mpsc::Receiver<Batch>,
    _cancel: oneshot::Sender<()>,
}

impl SceneFeed {
    pub(crate) fn start(
        sources: SceneSources,
        agent_id: UUID,
        camera: watch::Sender<Option<Camera>>,
        runtime: Handle,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        static MEMORY: OnceLock<Budget> = OnceLock::new();
        let budget = MEMORY.get_or_init(Budget::default).clone();
        let (scene, view) = SceneReconciler::new(budget.clone());
        let (sender, incoming) = mpsc::channel(1);
        let (cancel, cancelled) = oneshot::channel();
        let observations = camera.subscribe();
        drop(runtime.clone().spawn(run(scene, sources, sender, runtime.clone(), cancelled, observations, wake)));
        Self { runtime, view, budget, agent_id, water_height: None, camera, incoming, _cancel: cancel }
    }

    pub(crate) fn camera(&self, camera: Camera) {
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
        if self.view.status() == scene::Status::Closed {
            self.water_height = None;
            self.incoming.close();
            let _ = self.incoming.try_recv();
            return Ok(());
        }
        if let Ok(batch) = self.incoming.try_recv() {
            let before = self.view.status();
            self.view.apply(batch)?;
            self.water_height = water_height(&self.view, self.agent_id);
            if self.view.status() != before {
                tracing::info!(status = ?self.view.status(), objects = self.view.object_count(), regions = self.view.region_count(), patches = self.view.patch_count(), revision = self.view.revision().map(|r| r.get()), "scene reconciliation status changed");
            }
        }
        Ok(())
    }
}

fn water_height(view: &SceneView, agent: UUID) -> Option<f64> {
    let region = view.objects().find(|object| object.detail().summary.id == agent)?.detail().summary.address.region();
    let height = view.water_height(region)?;
    let origin = view.regions().find(|source| source.summary().region == region)?.summary().info.resolved_origin()?;
    Some(origin.z + f64::from(height))
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

async fn run(
    mut scene: SceneReconciler,
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
            scene.camera(camera);
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
                meshes.push(runtime.spawn_blocking(move || job.run()));
                Some(Instant::now())
            }
            ReconcileStep::Publish(batch) => {
                if sender.try_send(batch).is_err() {
                    break;
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
                    Some(command) => if let Err(error) = scene.push_environment(command.region, command.push) { tracing::warn!(?error, "experience environment push rejected"); },
                    None => break,
                },
                (job, reply) = reading => scene.complete(job, reply, Instant::now()),
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
        if let Some(event) = asset_event {
            scene.complete_asset(event.ticket, event.status.clone());
            assets.resume(event);
        }
    }
    scene.close();
    wake();
}
