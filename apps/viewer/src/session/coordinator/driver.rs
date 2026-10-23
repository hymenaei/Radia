use std::{
    future::pending,
    ops::ControlFlow,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use futures::{
    FutureExt,
    channel::{mpsc as futures_mpsc, oneshot},
    executor::block_on,
    future::{BoxFuture, Shared},
    pin_mut,
};
use math::DVec3;
use network::{AgentSession, CoverageLoss, CoverageSource, Delivery, Event, Grid, PacketOrder, SimulatorId, capabilities::Client};
use protocol::{lludp::Message, login::ResponseData};
use render::scene::{Camera, EnvironmentPush};
use tokio::{
    runtime::Handle,
    sync::{mpsc, watch},
};
use tracing::Instrument;
use world::UpdateSource;

use super::{
    WriteLock,
    events::{SimulatorScope, WorldHandoff, handle_simulator_message, order_evidence, route_recovery_targets},
    input::{Input, next_query_state_maintenance_delay, select_drain_input},
    media::ObjectMediaIntake,
    presentation::{AuthenticationUpdate, FavoritesUpdate, PresentationMailbox, SessionReadyUpdate, ViewerUpdate},
};
use crate::{
    asset_coordinator::AssetCoordinator,
    inventory::favorites::load_favorites,
    remembered_login::{self, RememberedLogin},
    session::{
        RegionLookup, Session, SessionHandle, agent,
        environment::{Completion, EnvironmentIntake},
        experience_environment::{self, EnvironmentCommand, EnvironmentInbox},
        inbound::object_media_requests,
        object_media::{ObjectMediaFetcher, ObjectMediaResult},
        recovery::{self, RecoveryScheduler},
        terrain::{DecodedTerrain, TerrainDecodeQueue},
        world::{IngestSummary, SessionWorld, viewer_world_limits},
    },
    world_reader::{ReadRequest, WorldReadHost},
};

const CAMERA_UPDATE_INTERVAL: Duration = Duration::from_millis(100);
const CAMERA_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

type Cancellation = Shared<BoxFuture<'static, ()>>;

pub(crate) struct CredentialPersistence {
    pub(crate) grid: Grid,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) remember_login: bool,
    pub(crate) write_lock: WriteLock,
}

struct SessionDriver {
    session: Session,
    session_handle: SessionHandle,
    runtime: Handle,
    mailbox: PresentationMailbox,
    cancellation: Cancellation,
    credentials: CredentialPersistence,
    session_world: SessionWorld,
    reads: WorldReadHost,
    terrain: TerrainDecodeQueue,
    environments: EnvironmentIntake,
    environment_pushes: mpsc::Sender<EnvironmentCommand>,
    environment_inbox: EnvironmentInbox,
    assets: AssetCoordinator,
    terrain_received: bool,
    primary_simulator: Option<SimulatorId>,
    handoff: WorldHandoff,
    active_session: Option<AgentSession>,
    active_login_data: Option<ResponseData>,
    camera: watch::Sender<Option<Camera>>,
    observations: watch::Receiver<Option<Camera>>,
    last_camera: Option<(SimulatorId, DVec3, Camera)>,
    camera_sent: Option<Instant>,
    region_lookup: Option<RegionLookup>,
    query_state_maintenance_started: Instant,
    query_state_maintenance: BoxFuture<'static, ()>,
    media: ObjectMediaIntake,
    recovery_queue: Arc<Mutex<RecoveryScheduler>>,
    recovery_wake: futures_mpsc::Sender<()>,
}

impl SessionDriver {
    fn new(
        session: Session,
        session_handle: SessionHandle,
        runtime: Handle,
        mailbox: PresentationMailbox,
        cancellation: oneshot::Receiver<()>,
        credentials: CredentialPersistence,
    ) -> Self {
        let cancellation = async move {
            let _ = cancellation.await;
        }
        .boxed()
        .shared();
        let session_world = SessionWorld::new(viewer_world_limits());
        let reads = WorldReadHost::new(session_world.world(), runtime.clone());
        let terrain = TerrainDecodeQueue::default();
        let environments = EnvironmentIntake::new(runtime.clone());
        let (environment_pushes, environment_inbox) = EnvironmentInbox::channel();
        let assets = AssetCoordinator::new(runtime.clone(), cancellation.clone());
        let (camera, observations) = watch::channel(None);
        let media = ObjectMediaIntake::new(ObjectMediaFetcher::new(runtime.clone(), cancellation.clone()));
        let recovery_queue = Arc::new(Mutex::new(RecoveryScheduler::new()));
        let (recovery_wake, recovery_wake_receiver) = futures_mpsc::channel(1);
        drop(
            runtime.spawn(
                recovery::run_recovery_retries(
                    recovery_wake_receiver,
                    Arc::clone(&recovery_queue),
                    session_handle.clone(),
                    cancellation.clone(),
                )
                .in_current_span(),
            ),
        );

        let query_state_maintenance_started = tokio::time::Instant::now().into_std();
        let query_state_maintenance = futures::future::ready(()).boxed();

        Self {
            session,
            session_handle,
            runtime,
            mailbox,
            cancellation,
            credentials,
            session_world,
            reads,
            terrain,
            environments,
            environment_pushes,
            environment_inbox,
            assets,
            terrain_received: false,
            primary_simulator: None,
            handoff: WorldHandoff::default(),
            active_session: None,
            active_login_data: None,
            camera,
            observations,
            last_camera: None,
            camera_sent: None,
            region_lookup: None,
            query_state_maintenance_started,
            query_state_maintenance,
            media,
            recovery_queue,
            recovery_wake,
        }
    }

    async fn run(mut self) {
        loop {
            self.reads.publish(self.session_world.world());
            self.media.flush(&self.session_world);
            let terrain_full = self.terrain.is_full();
            let event = async { if terrain_full { pending::<Option<Event>>().await } else { self.session.recv_event().await } };
            let input = select_drain_input(
                &mut self.reads,
                event,
                self.media.next_result(),
                self.terrain.next_completion(),
                self.environments.next_completion(),
                self.cancellation.clone(),
                &mut self.query_state_maintenance,
            )
            .await;
            let flow = match input {
                Input::Cancelled => ControlFlow::Break(()),
                Input::Event(event) => self.handle_network_event(event).await,
                Input::Media(result) => {
                    self.handle_media_result(result);
                    ControlFlow::Continue(())
                }
                Input::Terrain(decoded) => {
                    self.handle_terrain_completion(decoded);
                    ControlFlow::Continue(())
                }
                Input::Environment(completion) => {
                    self.handle_environment_completion(completion);
                    ControlFlow::Continue(())
                }
                Input::WorldRead(request) => {
                    self.handle_world_read(request);
                    ControlFlow::Continue(())
                }
                Input::QueryStateMaintenance => {
                    self.handle_maintenance();
                    ControlFlow::Continue(())
                }
            };
            if flow.is_break() {
                break;
            }
        }
        self.finish().await;
    }

    fn handle_terrain_completion(&mut self, decoded: DecodedTerrain) {
        let applied = self.session_world.apply_terrain(decoded);
        if !self.terrain_received && applied > 0 {
            self.terrain_received = true;
            tracing::info!(patches = applied, "initial terrain patches received");
        }
    }

    fn handle_environment_completion(&mut self, completion: Option<Box<Completion>>) {
        if let Some(completion) = completion {
            self.environments.complete(&mut self.session_world, *completion);
        }
    }

    fn handle_world_read(&self, request: ReadRequest) {
        request.serve(self.session_world.world());
    }

    fn handle_maintenance(&mut self) {
        if let (Some(simulator), Some(session), Some(view)) = (self.primary_simulator, &self.active_session, *self.observations.borrow())
            && let Some(origin) = self
                .session_world
                .region_for(simulator)
                .and_then(|region| self.session_world.world().region(region))
                .and_then(|info| info.resolved_origin())
        {
            let state = (simulator, origin, view);
            let interval = if self.last_camera == Some(state) { CAMERA_HEARTBEAT_INTERVAL } else { CAMERA_UPDATE_INTERVAL };
            let now = tokio::time::Instant::now().into_std();
            if self.camera_sent.is_none_or(|sent| now.saturating_duration_since(sent) >= interval)
                && let Some(update) = agent::camera(session.agent_id, session.session_id, view, origin)
            {
                self.camera_sent = Some(now);
                match self.session_handle.send(update, Delivery::Unreliable) {
                    Ok(()) => {
                        if self
                            .last_camera
                            .is_none_or(|(previous, _, camera)| previous != simulator || camera.draw_distance() != view.draw_distance())
                        {
                            tracing::debug!(draw_distance = view.draw_distance(), "simulator viewing distance updated");
                        }
                        self.last_camera = Some(state);
                    }
                    Err(error) => {
                        self.last_camera = None;
                        tracing::debug!(%error, "camera update deferred");
                    }
                }
            }
        }
        let has_dirty_pressure = self.session_world.maintain_query_state();
        let elapsed = tokio::time::Instant::now().into_std().duration_since(self.query_state_maintenance_started);
        self.query_state_maintenance = tokio::time::sleep(next_query_state_maintenance_delay(elapsed, has_dirty_pressure)).boxed();
    }

    fn handle_media_result(&mut self, result: Option<ObjectMediaResult>) {
        if let Some(result) = result {
            self.media.apply_result(result, &mut self.session_world);
        } else if self.cancellation.clone().now_or_never().is_none() {
            tracing::warn!("object media worker stopped; media fetching is disabled for this session");
            self.media.disable();
        }
    }

    async fn handle_network_event(&mut self, event: Option<Event>) -> ControlFlow<()> {
        let Some(event) = event else {
            self.mailbox.publish(ViewerUpdate::Failed("The network session ended.".into()));
            return ControlFlow::Break(());
        };
        self.handle_event(event).await
    }

    async fn handle_event(&mut self, event: Event) -> ControlFlow<()> {
        use Event::{
            CapabilityError, CapabilityEvent, CapabilityEventBatch, CoverageLost, EventQueueCoverageLost, Failed, LoginAccepted,
            MfaRequired, SendFailed, SessionReady, SimulatorCapabilities, SimulatorConnectFailed, SimulatorConnected,
            SimulatorDisconnected, SimulatorFailed, SimulatorMessage, SimulatorPromoted,
        };

        match event {
            CoverageLost { loss } => {
                self.handle_coverage_loss(&loss);
                ControlFlow::Continue(())
            }
            MfaRequired { challenge } => {
                self.mailbox.publish(ViewerUpdate::Authentication(AuthenticationUpdate::MfaRequired(challenge)));
                ControlFlow::Continue(())
            }
            LoginAccepted { mfa_hash } => {
                self.handle_login_accepted(mfa_hash);
                ControlFlow::Continue(())
            }
            SessionReady { simulator_id, session, capabilities, login_data } => {
                self.handoff.session_ready(&mut self.session_world, simulator_id, session.agent_id);
                self.environments.session_ready(&mut self.session_world, simulator_id, &capabilities, self.primary_simulator);
                if let Some(region) = self.session_world.region_for(simulator_id) {
                    self.assets.register(region, capabilities.clone());
                }
                self.active_login_data = Some(login_data.clone());
                self.on_primary_changed(simulator_id, *session, capabilities, Some(login_data));
                ControlFlow::Continue(())
            }
            SimulatorPromoted { previous_simulator_id, simulator_id, session, capabilities } => {
                if let Some(region) = self.session_world.region_for(simulator_id) {
                    let push = EnvironmentPush::Clear { experience: None, transition: Duration::ZERO };
                    if let Err(error) = self.environment_pushes.try_send(EnvironmentCommand { region, push }) {
                        tracing::error!(simulator = simulator_id.get(), region = region.get(), %error, "experience environment crossing reset could not be admitted");
                        self.mailbox.publish(ViewerUpdate::Failed("The environment update queue is unavailable.".into()));
                        return ControlFlow::Break(());
                    }
                }
                self.environments.simulator_promoted(
                    &mut self.session_world,
                    previous_simulator_id,
                    simulator_id,
                    &capabilities,
                    self.primary_simulator,
                );
                if let Some(summary) = self.handoff.promote(&mut self.session_world, previous_simulator_id, simulator_id) {
                    self.process_ingest_summary(simulator_id, summary);
                }
                let region = self.session_world.region_for(simulator_id).map(|region| region.get());
                tracing::info!(simulator = simulator_id.get(), region, "primary simulator promoted");
                self.on_primary_changed(simulator_id, *session, capabilities, self.active_login_data.clone());
                ControlFlow::Continue(())
            }
            SimulatorConnected { simulator_id, region_handle, .. } => {
                self.handoff.simulator_connected(&mut self.session_world, simulator_id, region_handle);
                if region_handle.is_none() {
                    self.primary_simulator = Some(simulator_id);
                }
                ControlFlow::Continue(())
            }
            SimulatorMessage { simulator_id, sequence, packet_order, source, message } => {
                self.handle_simulator_message(simulator_id, sequence, packet_order, source, &message).await;
                ControlFlow::Continue(())
            }
            SendFailed(error) => {
                tracing::warn!(%error, "simulator send failed");
                ControlFlow::Continue(())
            }
            EventQueueCoverageLost { reason, simulator_id, .. } => {
                let region = self.session_world.region_for(simulator_id).map(|region| region.get());
                tracing::warn!(simulator = simulator_id.get(), region, %reason, "simulator event delivery may have a gap; world data may be incomplete");
                ControlFlow::Continue(())
            }
            Failed(error) => {
                self.mailbox.publish(ViewerUpdate::Failed(error));
                ControlFlow::Break(())
            }
            SimulatorDisconnected { simulator_id, .. } | SimulatorFailed { simulator_id, .. } => {
                let region = self.session_world.region_for(simulator_id).map(|region| region.get());
                tracing::info!(simulator = simulator_id.get(), region, "simulator disconnected");
                self.environments.simulator_disconnected(&mut self.session_world, simulator_id);
                if let Some(region) = self.session_world.region_for(simulator_id) {
                    self.assets.retire(region);
                }
                self.handoff.simulator_disconnected(&mut self.session_world, simulator_id);
                self.recovery_queue.lock().unwrap_or_else(PoisonError::into_inner).cancel_simulator(simulator_id);
                self.media.forget_simulator(simulator_id);
                if Some(simulator_id) == self.primary_simulator {
                    self.primary_simulator = None;
                }
                ControlFlow::Continue(())
            }
            CapabilityEvent { simulator_id, message, body, .. } => {
                self.environments.capability(&mut self.session_world, simulator_id, &message, &body, self.primary_simulator);
                ControlFlow::Continue(())
            }
            CapabilityEventBatch { simulator_id, events, .. } => {
                self.environments.capability_batch(
                    &mut self.session_world,
                    simulator_id,
                    events.iter().map(|event| (event.message.as_str(), &event.body)),
                    self.primary_simulator,
                );
                ControlFlow::Continue(())
            }
            CapabilityError { .. } | SimulatorConnectFailed { .. } => ControlFlow::Continue(()),
            SimulatorCapabilities { simulator_id, capabilities, .. } => {
                self.environments.simulator_capabilities(&mut self.session_world, simulator_id, &capabilities, self.primary_simulator);
                if let Some(region) = self.session_world.region_for(simulator_id) {
                    self.assets.register(region, capabilities.clone());
                }
                self.media.set_client(simulator_id, capabilities);
                ControlFlow::Continue(())
            }
        }
    }

    fn handle_coverage_loss(&mut self, loss: &CoverageLoss) {
        match loss {
            CoverageLoss::Source(record) => {
                let simulator_id = record.watermark.simulator_id;
                let region = self.session_world.region_for(simulator_id).map(|region| region.get());
                let source = match record.watermark.source {
                    CoverageSource::Lludp => "lludp",
                    CoverageSource::EventQueue { .. } => "event_queue",
                };
                tracing::warn!(simulator = simulator_id.get(), region, source, reason = ?record.reason, "world data may be incomplete");
                self.session.acknowledge_coverage_loss(record.watermark);
            }
            CoverageLoss::SessionOverflow { generation } => {
                tracing::warn!(generation, "world event queue overflowed; world data may be incomplete");
                self.session.acknowledge_coverage_overflow(*generation);
            }
        }
    }

    fn handle_login_accepted(&self, mfa_hash: Option<String>) {
        let mailbox = self.mailbox.clone();
        let write_lock = self.credentials.write_lock.clone();
        let username = self.credentials.username.clone();
        let password = self.credentials.password.clone();
        let grid = self.credentials.grid;
        let remember_login = self.credentials.remember_login;
        let span = tracing::Span::current();
        drop(self.runtime.spawn_blocking(move || {
            let _span = span.enter();
            let _guard = block_on(write_lock.lock());
            let mut warning = None;
            let remembered_login = if remember_login {
                let saved = RememberedLogin { username, password, mfa_hash: mfa_hash.unwrap_or_default() };
                match remembered_login::save(grid, &saved) {
                    Ok(()) => Some(saved),
                    Err(error) => {
                        warning = Some(error);
                        None
                    }
                }
            } else {
                if let Err(error) = remembered_login::clear(grid) {
                    warning = Some(error);
                }
                None
            };
            mailbox.publish(ViewerUpdate::Authentication(AuthenticationUpdate::CredentialsUpdated { remembered_login, warning }));
        }));
    }

    async fn handle_simulator_message(
        &mut self,
        simulator_id: SimulatorId,
        sequence: u32,
        packet_order: PacketOrder,
        source: std::net::SocketAddr,
        message: &Message,
    ) {
        if matches!(packet_order, PacketOrder::Ordered(_))
            && Some(simulator_id) == self.primary_simulator
            && let (Some(agent), Some(region)) = (self.active_session.as_ref(), self.session_world.region_for(simulator_id))
        {
            match experience_environment::decode(message, agent.agent_id) {
                Ok(Some(push)) => match self.environment_pushes.try_send(EnvironmentCommand { region, push }) {
                    Ok(()) => {}
                    Err(error) => {
                        tracing::warn!(simulator = simulator_id.get(), region = region.get(), %error, "experience environment push could not be admitted")
                    }
                },
                Ok(None) => {}
                Err(_) => tracing::debug!(simulator = simulator_id.get(), region = region.get(), "invalid experience environment push"),
            }
        }

        let media_requests = self
            .session_world
            .region_for(simulator_id)
            .map(|region| object_media_requests(region, sequence, order_evidence(packet_order), message))
            .unwrap_or_default();
        if let Some(region) = self.session_world.region_for(simulator_id) {
            self.terrain.enqueue(message, UpdateSource::new(region, sequence, order_evidence(packet_order), 0), &self.runtime);
        }

        let agent_id = self.active_session.as_ref().map(|session| session.agent_id);
        let result = {
            let active_session = self.active_session.as_ref();
            let recovery_queue = &self.recovery_queue;
            let recovery_wake = &mut self.recovery_wake;
            let mut on_object_chunk = |targets, updated| {
                recovery::schedule_recovery_requests(targets, updated, active_session, recovery_queue, recovery_wake);
            };
            self.handoff
                .apply_message_cooperatively(
                    &mut self.session_world,
                    simulator_id,
                    sequence,
                    packet_order,
                    message,
                    agent_id,
                    &mut on_object_chunk,
                )
                .await
        };
        if let Some((simulator_id, summary)) = result {
            self.process_ingest_summary(simulator_id, summary);
        }

        self.environments.simulator_message(&mut self.session_world, simulator_id, packet_order, message, self.primary_simulator);
        for request in media_requests {
            if let Some(target) = self.session_world.prepare_object_media(simulator_id, request) {
                self.media.queue(target, &self.session_world);
            }
        }

        if Some(simulator_id) == self.primary_simulator
            && self.observations.borrow().is_none()
            && let Message::AgentMovementComplete(movement) = message
            && let Some(session) = &self.active_session
            && let Some(update) = agent::arrival(session.agent_id, session.session_id, movement)
            && let Err(error) = self.session_handle.send(update, Delivery::Reliable)
        {
            tracing::debug!(%error, "arrival camera update deferred");
        }
        let scope = if Some(simulator_id) == self.primary_simulator { SimulatorScope::Primary } else { SimulatorScope::Neighbor };
        let region_name = self.session_world.region_name(simulator_id);
        handle_simulator_message(
            scope,
            source,
            message.clone(),
            self.active_session.as_ref(),
            self.region_lookup.as_ref(),
            region_name,
            &self.mailbox,
        );
    }

    fn on_primary_changed(
        &mut self,
        simulator_id: SimulatorId,
        session: AgentSession,
        capabilities: Client,
        login_data: Option<ResponseData>,
    ) {
        self.media.set_client(simulator_id, capabilities.clone());
        self.primary_simulator = Some(simulator_id);
        self.active_session = Some(session.clone());
        if let Some(login_data) = &login_data {
            self.active_login_data = Some(login_data.clone());
        }
        let lookup = RegionLookup::new(self.session_handle.clone(), simulator_id, &session, self.runtime.clone());
        self.region_lookup = Some(lookup.clone());
        self.mailbox.publish(ViewerUpdate::SessionReady(SessionReadyUpdate {
            environments: self.environment_inbox.clone(),
            camera: self.camera.clone(),
            assets: self.assets.handle(),
            session: session.clone(),
            region_lookup: lookup.clone(),
            region_name: self.session_world.region_name(simulator_id).map(str::to_owned),
            world_reader: self.reads.reader(),
        }));
        if let Some(login_data) = login_data {
            self.spawn_favorites_load(session, capabilities, lookup, login_data);
        }
    }

    fn spawn_favorites_load(&self, session: AgentSession, capabilities: Client, lookup: RegionLookup, login_data: ResponseData) {
        let mailbox = self.mailbox.clone();
        let username = self.credentials.username.clone();
        let cancelled = self.cancellation.clone();
        drop(
            self.runtime.spawn(
                async move {
                    let load = load_favorites(&capabilities, &lookup, &session, &login_data).fuse();
                    pin_mut!(load);
                    let cancelled = cancelled.fuse();
                    pin_mut!(cancelled);
                    futures::select! {
                        result = load => mailbox.publish(ViewerUpdate::Favorites(FavoritesUpdate {
                            grid: session.grid,
                            agent_id: session.agent_id,
                            session_id: session.session_id,
                            username,
                            result,
                        })),
                        _ = cancelled => {}
                    }
                }
                .in_current_span(),
            ),
        );
    }

    fn process_ingest_summary(&mut self, simulator_id: SimulatorId, summary: IngestSummary) {
        summary.log_incomplete_data(simulator_id, self.session_world.region_for(simulator_id));
        recovery::schedule_recovery_requests(
            route_recovery_targets(&self.session_world, summary.recovery_targets()),
            summary.recovery_cancellation_addresses().collect(),
            self.active_session.as_ref(),
            &self.recovery_queue,
            &mut self.recovery_wake,
        );
    }

    async fn finish(mut self) {
        self.reads.finish();
        drop(self.environments);
        drop(self.terrain);
        self.assets.stop().await;
        for (simulator_id, error) in self.session_world.retire_all() {
            let region = self.session_world.region_for(simulator_id).map(|region| region.get());
            tracing::warn!(simulator = simulator_id.get(), region, %error, "could not retire simulator world region on session shutdown");
        }
    }
}

#[tracing::instrument(name = "viewer.session", level = "info", skip_all)]
pub(crate) async fn drain_session(
    session: Session,
    session_handle: SessionHandle,
    runtime: Handle,
    mailbox: PresentationMailbox,
    cancellation: oneshot::Receiver<()>,
    credentials: CredentialPersistence,
) {
    SessionDriver::new(session, session_handle, runtime, mailbox, cancellation, credentials).run().await;
}
