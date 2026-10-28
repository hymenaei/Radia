pub(super) mod inventory;

use std::{
    collections::{HashMap, HashSet},
    future::pending,
    ops::ControlFlow,
    sync::{Arc, Mutex},
};

use ::inventory::{CategoryKey, Inventory, InventoryNodeKey, InventorySnapshot};
use futures::{
    FutureExt,
    channel::oneshot,
    executor::block_on,
    future::{BoxFuture, Shared},
};
use network::{
    AgentSession, CircuitMotionStatus, CoverageLoss, CoverageSource, Delivery, Event, Grid, PacketOrder, SimulatorId,
    capabilities::Client,
};
use protocol::{
    lludp::{FieldValue, Message},
    login::ResponseData,
    messages::{
        AgentRequestSit, AgentSit, RequestAgentData,
        agent::agent_request_sit::{AgentData as SitAgentData, TargetObject},
    },
};
use tokio::{runtime::Handle, sync::watch};
use tracing::Instrument;
use uuid::Uuid;
use world::UpdateSource;

use super::{
    WriteLock,
    events::{MovementCompleteOutcome, SimulatorScope, WorldHandoff, handle_simulator_message, order_evidence, route_recovery_targets},
    input::{Input, select_drain_input},
    inventory_bootstrap::{InventoryBootstrapReport, InventoryWorkerError, spawn_inventory_build},
    inventory_cache::{InventoryCacheEvent, InventoryCacheSession},
    media::ObjectMediaIntake,
    presentation::{AuthenticationUpdate, PresentationMailbox, SessionEnd, SessionReadyUpdate, ViewerUpdate},
};
use crate::{
    asset_coordinator::AssetCoordinator,
    inventory::{InventoryEditController, InventoryEditEvent, InventoryFetchController, InventoryFetchError, MoveExpectation},
    remembered_login::{self, RememberedLogin},
    session::{
        AcceptedSitResponse, AgentControlEvent, AgentControlHandle, AgentControlState, AgentMotionPublisher, CameraUplink, RegionLookup,
        Session, SessionHandle, SitOutcome,
        agent_control::{AgentControlError, Command as AgentControlCommand},
        environment::{Completion, EnvironmentIntake},
        inbound::{decode_sit_response, object_media_requests},
        object_cache::{ObjectCacheEvent, ObjectCacheSession},
        object_media::{ObjectMediaFetcher, ObjectMediaResult},
        recovery::{RecoveryCompletion, RecoveryIntake, ScheduledRecovery, retryable_send_error, simulator_lost_error, warn_unrecovered},
        terrain::{DecodedTerrain, TerrainDecodeQueue},
        world::{IngestSummary, SessionWorld, viewer_world_limits},
    },
    world_reader::{ReadRequest, WorldReadHost},
};

#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::session::experience_environment::{EnvironmentInbox, EnvironmentSender};

type Cancellation = Shared<BoxFuture<'static, ()>>;

fn network_end_after_stream_close(disconnect_requested: bool, failure: Option<String>, primary_disconnected: bool) -> SessionEnd {
    if let Some(failure) = failure {
        SessionEnd::Failed(failure)
    } else if disconnect_requested {
        SessionEnd::Requested
    } else if primary_disconnected {
        SessionEnd::Failed("The primary simulator disconnected.".into())
    } else {
        SessionEnd::Failed("The network session ended.".into())
    }
}

enum DriverInput {
    Session(Input),
    AgentControl(AgentControlEvent),
    InventoryCache(Option<InventoryCacheEvent>),
    ObjectCache(Option<ObjectCacheEvent>),
    InventoryEdit(Option<InventoryEditEvent>),
    InventoryBootstrap(Result<Result<(Inventory, InventoryBootstrapReport), InventoryWorkerError>, tokio::task::JoinError>),
}

#[derive(Default)]
struct PendingHandshakeReplies {
    by_simulator: HashMap<SimulatorId, PendingHandshakeReply>,
}

struct PendingHandshakeReply {
    flags: u32,
    remaining: usize,
}

impl PendingHandshakeReplies {
    fn queue(&mut self, simulator_id: SimulatorId, flags: u32) -> bool {
        match self.by_simulator.get_mut(&simulator_id) {
            Some(reply) => {
                reply.flags = flags;
                reply.remaining = reply.remaining.saturating_add(1);
                false
            }
            None => {
                self.by_simulator.insert(simulator_id, PendingHandshakeReply { flags, remaining: 1 });
                true
            }
        }
    }

    fn take_next(&mut self, simulator_id: SimulatorId) -> Option<u32> {
        if self.by_simulator.get(&simulator_id).is_some_and(|reply| reply.remaining == 0) {
            self.by_simulator.remove(&simulator_id);
            return None;
        }
        let reply = self.by_simulator.get_mut(&simulator_id)?;
        reply.remaining -= 1;
        Some(reply.flags)
    }
}

trait SitTransport {
    async fn send_request(&self, simulator: SimulatorId, request: AgentRequestSit, delivery: Delivery) -> Result<(), network::Error>;

    async fn send_sit(&self, simulator: SimulatorId, sit: AgentSit, delivery: Delivery) -> Result<(), network::Error>;
}

impl SitTransport for SessionHandle {
    async fn send_request(&self, simulator: SimulatorId, request: AgentRequestSit, delivery: Delivery) -> Result<(), network::Error> {
        self.send_to(simulator, request, delivery).await
    }

    async fn send_sit(&self, simulator: SimulatorId, sit: AgentSit, delivery: Delivery) -> Result<(), network::Error> {
        self.send_to(simulator, sit, delivery).await
    }
}

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
    network_failure: Option<String>,
    primary_disconnected: bool,
    credentials: CredentialPersistence,
    session_world: SessionWorld,
    reads: WorldReadHost,
    terrain: TerrainDecodeQueue,
    environments: EnvironmentIntake,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    environment_pushes: EnvironmentSender,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    environment_inbox: EnvironmentInbox,
    assets: AssetCoordinator,
    terrain_received: bool,
    primary_simulator: Option<SimulatorId>,
    handoff: WorldHandoff,
    active_session: Option<AgentSession>,
    active_login_data: Option<ResponseData>,
    active_capabilities: Option<Client>,
    favorites_enabled: bool,
    inventory_bootstrap: Option<tokio::task::JoinHandle<Result<(Inventory, InventoryBootstrapReport), InventoryWorkerError>>>,
    inventory_bootstrap_dirty: bool,
    inventory: Option<Inventory>,
    inventory_cache: InventoryCacheSession,
    object_cache: ObjectCacheSession,
    pending_handshake_replies: Arc<Mutex<PendingHandshakeReplies>>,
    inventory_changes: watch::Sender<Option<InventorySnapshot>>,
    rejected_inventory_blocks: u64,
    inventory_fetch: InventoryFetchController,
    inventory_edits: InventoryEditController,
    inventory_pending_moves: HashMap<InventoryNodeKey, (MoveExpectation, tokio::sync::oneshot::Sender<()>)>,
    inventory_known: HashSet<CategoryKey>,
    inventory_complete: HashSet<CategoryKey>,
    inventory_failed: HashSet<CategoryKey>,
    inventory_background_done: bool,
    inventory_last_failure: Option<(CategoryKey, InventoryFetchError)>,
    camera: CameraUplink,
    agent_motion: AgentMotionPublisher,
    motion_statuses: HashMap<SimulatorId, watch::Receiver<CircuitMotionStatus>>,
    region_motion_mapping: HashMap<world::Region, SimulatorId>,
    region_motion_sender: watch::Sender<Vec<(world::Region, watch::Receiver<CircuitMotionStatus>)>>,
    agent_control: AgentControlState,
    agent_control_handle: AgentControlHandle,
    sit_response: watch::Receiver<Option<AcceptedSitResponse>>,
    region_lookup: Option<RegionLookup>,
    media: ObjectMediaIntake,
    recovery: RecoveryIntake,
}

impl SessionDriver {
    fn new(
        session: Session,
        session_handle: SessionHandle,
        runtime: Handle,
        mailbox: PresentationMailbox,
        cancellation: oneshot::Receiver<()>,
        credentials: CredentialPersistence,
        cache: Option<Arc<assets::cache::AssetCache>>,
    ) -> Self {
        let cancellation = async move {
            match cancellation.await {
                Ok(()) | Err(_) => {}
            }
        }
        .boxed()
        .shared();
        let session_world = SessionWorld::new(viewer_world_limits());
        let reads = WorldReadHost::new(session_world.world(), runtime.clone());
        let terrain = TerrainDecodeQueue::default();
        let environments = EnvironmentIntake::new(runtime.clone());
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        let (environment_pushes, environment_inbox) = EnvironmentInbox::channel();
        let assets = AssetCoordinator::new(runtime.clone(), cancellation.clone(), cache);
        let camera = CameraUplink::new();
        let (region_motion_sender, _) = watch::channel(Vec::new());
        let (agent_control_handle, agent_control, sit_response) = AgentControlState::new();
        let media = ObjectMediaIntake::new(ObjectMediaFetcher::new(runtime.clone(), cancellation.clone()));
        let recovery = RecoveryIntake::new();
        let (inventory_changes, _) = watch::channel(None);
        let inventory_edits = InventoryEditController::new();
        let inventory_fetch = InventoryFetchController::new(inventory_edits.handle());
        let mut object_cache = ObjectCacheSession::new(credentials.grid);
        object_cache.start(&runtime);
        let pending_handshake_replies = Arc::new(Mutex::new(PendingHandshakeReplies::default()));

        Self {
            session,
            session_handle,
            runtime,
            mailbox,
            cancellation,
            network_failure: None,
            primary_disconnected: false,
            credentials,
            session_world,
            reads,
            terrain,
            environments,
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            environment_pushes,
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            environment_inbox,
            assets,
            terrain_received: false,
            primary_simulator: None,
            handoff: WorldHandoff::default(),
            active_session: None,
            active_login_data: None,
            active_capabilities: None,
            favorites_enabled: false,
            inventory_bootstrap: None,
            inventory_bootstrap_dirty: false,
            inventory: None,
            inventory_cache: InventoryCacheSession::new(),
            object_cache,
            pending_handshake_replies,
            inventory_changes,
            rejected_inventory_blocks: 0,
            inventory_fetch,
            inventory_edits,
            inventory_pending_moves: HashMap::new(),
            inventory_known: HashSet::new(),
            inventory_complete: HashSet::new(),
            inventory_failed: HashSet::new(),
            inventory_background_done: false,
            inventory_last_failure: None,
            camera,
            agent_motion: AgentMotionPublisher::new(),
            motion_statuses: HashMap::new(),
            region_motion_mapping: HashMap::new(),
            region_motion_sender,
            agent_control,
            agent_control_handle,
            sit_response,
            region_lookup: None,
            media,
            recovery,
        }
    }

    async fn run(mut self) {
        loop {
            self.reads.publish(self.session_world.world());
            self.media.flush(&self.session_world);
            let terrain_full = self.terrain.is_full();
            let event = async { if terrain_full { pending::<Option<Event>>().await } else { self.session.recv_event().await } };
            let inventory_bootstrap = async {
                match self.inventory_bootstrap.as_mut() {
                    Some(task) => Some(task.await),
                    None => pending().await,
                }
            };
            let input = tokio::select! {
                input = select_drain_input(
                    &mut self.reads,
                    event,
                    self.media.next_result(),
                    self.terrain.next_completion(),
                    self.environments.next_completion(),
                    self.cancellation.clone(),
                    self.camera.next_tick(),
                    self.recovery.next_completion(),
                    self.inventory_fetch.next_event(),
                ) => DriverInput::Session(input),
                event = self.agent_control.next_event() => DriverInput::AgentControl(event),
                cache_event = self.inventory_cache.next_event() => DriverInput::InventoryCache(cache_event),
                cache_event = self.object_cache.next_event() => DriverInput::ObjectCache(cache_event),
                edit_event = self.inventory_edits.next_event() => DriverInput::InventoryEdit(edit_event),
                bootstrap = inventory_bootstrap => DriverInput::InventoryBootstrap(bootstrap.expect("bootstrap task must be pending")),
            };
            let flow = match input {
                DriverInput::InventoryBootstrap(result) => {
                    self.inventory_bootstrap = None;
                    match result {
                        Ok(Ok((inventory, report))) => self.finish_inventory_bootstrap(inventory, report),
                        Ok(Err(error)) => {
                            tracing::warn!(%error, "login inventory could not be initialized");
                            self.start_favorites_load_for_active();
                        }
                        Err(error) => {
                            tracing::warn!(%error, "login inventory worker failed");
                            self.start_favorites_load_for_active();
                        }
                    }
                    ControlFlow::Continue(())
                }
                DriverInput::InventoryCache(Some(event)) => {
                    let before = self.inventory.as_ref().map(Inventory::snapshot);
                    let outcome =
                        self.inventory.as_mut().map(|inventory| self.inventory_cache.handle_event(&self.runtime, event, inventory));
                    if let Some(outcome) = outcome
                        && let Some(snapshot) = outcome.snapshot
                    {
                        if let Some(before) = before {
                            let changes = self::inventory::cache_restore_move_changes(&self.inventory_pending_moves, &before, &snapshot);
                            self.process_inventory_changes(&changes, &snapshot);
                        }
                        self.inventory_changes.send_replace(Some(snapshot));
                    }
                    ControlFlow::Continue(())
                }
                DriverInput::InventoryCache(None) => ControlFlow::Continue(()),
                DriverInput::ObjectCache(Some(event)) => {
                    if let Some((simulator_id, flags)) = self.object_cache.handle_event(event) {
                        reply_region_handshake(
                            &self.runtime,
                            Arc::clone(&self.pending_handshake_replies),
                            self.session_handle.clone(),
                            simulator_id,
                            flags,
                        );
                    }
                    ControlFlow::Continue(())
                }
                DriverInput::ObjectCache(None) => ControlFlow::Continue(()),
                DriverInput::InventoryEdit(Some(event)) => {
                    self.handle_inventory_edit_event(event);
                    ControlFlow::Continue(())
                }
                DriverInput::InventoryEdit(None) => ControlFlow::Continue(()),
                DriverInput::AgentControl(event) => {
                    self.handle_agent_control_event(event).await;
                    ControlFlow::Continue(())
                }
                DriverInput::Session(input) => match input {
                    Input::Cancelled => {
                        self.mailbox.publish(ViewerUpdate::Ended);
                        ControlFlow::Break(())
                    }
                    Input::Event(event) => self.handle_network_event(event).await,
                    Input::InventoryFetch(Some(event)) => {
                        self.handle_inventory_fetch(event);
                        ControlFlow::Continue(())
                    }
                    Input::InventoryFetch(None) => ControlFlow::Continue(()),
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
                    Input::CameraTick => {
                        self.handle_camera_tick();
                        ControlFlow::Continue(())
                    }
                    Input::Recovery(completion) => {
                        self.handle_recovery_completion(completion);
                        ControlFlow::Continue(())
                    }
                },
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

    fn handle_camera_tick(&mut self) {
        let simulator = self.primary_simulator;
        let origin = self.primary_region_origin();
        self.camera.tick(&self.session_handle, simulator, self.active_session.as_ref(), origin);
    }

    fn primary_region_origin(&self) -> Option<math::DVec3> {
        self.primary_simulator
            .and_then(|simulator| self.session_world.region_for(simulator))
            .and_then(|region| self.session_world.world().region(region))
            .and_then(|info| info.resolved_origin())
    }

    async fn handle_agent_control_event(&mut self, event: AgentControlEvent) {
        match event {
            AgentControlEvent::Command(Some(command)) => self.handle_agent_control_command(command).await,
            AgentControlEvent::Command(None) => {}
            AgentControlEvent::SitTimedOut => self.agent_control.timeout_sit(),
        }
    }

    async fn handle_agent_control_command(&mut self, command: AgentControlCommand) {
        process_agent_control_command(
            &self.session_handle,
            &mut self.agent_control,
            &mut self.camera,
            self.primary_simulator,
            self.active_session.as_ref().map(|session| (session.agent_id, session.session_id)),
            command,
        )
        .await;
    }

    async fn handle_sit_response(&mut self, simulator: SimulatorId, message: &Message) {
        process_sit_response(
            &self.session_handle,
            &mut self.agent_control,
            simulator,
            self.active_session.as_ref().map(|session| (session.agent_id, session.session_id)),
            message,
        )
        .await;
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
            let end = network_end_after_stream_close(
                self.session_handle.is_disconnect_requested(),
                self.network_failure.take(),
                self.primary_disconnected,
            );
            self.mailbox.publish(match end {
                SessionEnd::Requested => ViewerUpdate::Ended,
                SessionEnd::Failed(error) => ViewerUpdate::Failed(error),
            });
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
                if self.inventory.is_none() && self.inventory_bootstrap.is_none() {
                    self.inventory_bootstrap = Some(spawn_inventory_build(&self.runtime, session.agent_id, login_data.clone()));
                }
                self.favorites_enabled = true;
                self.environments.session_ready(&mut self.session_world, simulator_id, &capabilities, self.primary_simulator);
                if let Some(region) = self.session_world.region_for(simulator_id) {
                    self.assets.register(region, capabilities.clone());
                }
                self.on_primary_changed(simulator_id, *session, capabilities, Some(login_data));
                ControlFlow::Continue(())
            }
            SimulatorPromoted { previous_simulator_id, simulator_id, session, capabilities } => {
                #[cfg(any(target_os = "windows", target_os = "linux"))]
                if let Some(region) = self.session_world.region_for(simulator_id)
                    && let Err(error) = self.environment_pushes.clear_region(region)
                {
                    tracing::error!(simulator = simulator_id.get(), region = region.get(), %error, "experience environment crossing reset could not be admitted");
                    self.mailbox.publish(ViewerUpdate::Failed("The environment update queue is unavailable.".into()));
                    return ControlFlow::Break(());
                }
                self.environments.simulator_promoted(
                    &mut self.session_world,
                    previous_simulator_id,
                    simulator_id,
                    &capabilities,
                    self.primary_simulator,
                );
                let summary = self.handoff.promote(&mut self.session_world, previous_simulator_id, simulator_id);
                let region = self.session_world.region_for(simulator_id).map(|region| region.get());
                tracing::info!(simulator = simulator_id.get(), region, "primary simulator promoted");
                self.on_primary_changed(simulator_id, *session, capabilities, self.active_login_data.clone());
                if let Some(summary) = summary {
                    let touched_agent = self.summary_touches_agent(&summary);
                    self.process_ingest_summary(simulator_id, summary);
                    if touched_agent {
                        self.publish_world_agent_motion(std::time::Instant::now());
                    }
                }
                if let Some((movement, received_at)) = self.handoff.apply_promoted_movement_complete(
                    &mut self.session_world,
                    simulator_id,
                    self.active_session.as_ref().map(|session| (session.agent_id, session.session_id)),
                ) {
                    self.publish_arrival_agent_motion(simulator_id, &movement, received_at);
                }
                self.publish_region_motion();
                ControlFlow::Continue(())
            }
            SimulatorConnected { simulator_id, region_handle, .. } => {
                let primary_origin_before = (self.primary_simulator == Some(simulator_id)).then(|| self.primary_region_origin()).flatten();
                self.handoff.simulator_connected(&mut self.session_world, simulator_id, region_handle);
                if region_handle.is_none() {
                    self.primary_simulator = Some(simulator_id);
                    self.camera.request_update();
                }
                if self.primary_simulator == Some(simulator_id) {
                    self.camera.region_origin_changed(primary_origin_before, self.primary_region_origin());
                }
                self.publish_region_motion();
                ControlFlow::Continue(())
            }
            Event::SimulatorMotionStatus { simulator_id, motion_status } => {
                self.motion_statuses.insert(simulator_id, motion_status);
                self.publish_region_motion();
                ControlFlow::Continue(())
            }
            SimulatorMessage { simulator_id, sequence, packet_order, source, received_at, ping_delay, message } => {
                self.handle_inventory_udp(&message);
                self.handle_simulator_message(simulator_id, sequence, packet_order, source, received_at, ping_delay, message).await;
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
            SimulatorDisconnected { simulator_id, .. } => self.handle_simulator_disconnect(simulator_id, None),
            SimulatorFailed { simulator_id, reason, .. } => self.handle_simulator_disconnect(simulator_id, Some(reason)),
            CapabilityEvent { simulator_id, message, body, .. } => {
                self.handle_inventory_event_queue(&message, &body);
                self.environments.capability(&mut self.session_world, simulator_id, &message, &body, self.primary_simulator);
                ControlFlow::Continue(())
            }
            CapabilityEventBatch { simulator_id, events, .. } => {
                for event in &events {
                    self.handle_inventory_event_queue(&event.message, &event.body);
                }
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
        received_at: std::time::Instant,
        ping_delay: Option<std::time::Duration>,
        message: Message,
    ) {
        let primary_origin_before = (self.primary_simulator == Some(simulator_id)).then(|| self.primary_region_origin()).flatten();
        let region = self.session_world.region_for(simulator_id);
        if let Message::RegionHandshake(handshake) = &message {
            self.object_cache.set_region_identity(simulator_id, handshake);
            if let Some(flags) = self.object_cache.begin_region_load(simulator_id, &self.runtime) {
                reply_region_handshake(
                    &self.runtime,
                    Arc::clone(&self.pending_handshake_replies),
                    self.session_handle.clone(),
                    simulator_id,
                    flags,
                );
            }
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        if matches!(packet_order, PacketOrder::Ordered(_))
            && Some(simulator_id) == self.primary_simulator
            && let (Some(agent), Some(region)) = (self.active_session.as_ref(), region)
            && let Err(error) = self.environment_pushes.publish(&message, agent.agent_id, region, simulator_id)
        {
            tracing::warn!(simulator = simulator_id.get(), region = region.get(), %error, "experience environment push could not be admitted");
        }

        let mut media_requests =
            region.map(|region| object_media_requests(region, sequence, order_evidence(packet_order), &message)).unwrap_or_default();
        if let Some(region) = region {
            self.terrain.enqueue(&message, UpdateSource::new(region, sequence, order_evidence(packet_order), 0), &self.runtime);
        }

        let agent_id = self.active_session.as_ref().map(|session| session.agent_id);
        let agent_data =
            self.active_session.as_ref().map(|session| RequestAgentData { agent_id: session.agent_id, session_id: session.session_id });
        let result = {
            let recovery = &mut self.recovery;
            let mut on_object_chunk = |targets, updated| {
                recovery.schedule(targets, updated, agent_data);
            };
            self.handoff
                .apply_message_cooperatively(
                    &mut self.session_world,
                    simulator_id,
                    sequence,
                    packet_order,
                    received_at,
                    ping_delay,
                    &message,
                    agent_id,
                    &mut self.object_cache,
                    &mut on_object_chunk,
                    &mut media_requests,
                )
                .await
        };
        let mut touched_agent = false;
        if let Some(outcome) = result {
            touched_agent = outcome.touched_agent;
            if let Some((simulator_id, summary)) = outcome.summary {
                self.process_ingest_summary(simulator_id, summary);
            }
        }
        if touched_agent && self.primary_simulator == Some(simulator_id) {
            self.publish_world_agent_motion(received_at);
        }

        self.environments.simulator_message(&mut self.session_world, simulator_id, packet_order, &message, self.primary_simulator);
        for request in media_requests {
            if let Some(target) = self.session_world.prepare_object_media(simulator_id, request) {
                self.media.queue(target, &self.session_world);
            }
        }

        if message.name() == "AlertMessage" {
            if handle_sit_alert(&mut self.agent_control, self.primary_simulator, simulator_id, &message) {
                tracing::debug!(simulator = simulator_id.get(), "sit request was refused");
            }
        } else {
            self.handle_sit_response(simulator_id, &message).await;
        }

        if let Message::AgentMovementComplete(movement) = &message {
            let outcome = self.handoff.movement_complete(
                &mut self.session_world,
                simulator_id,
                self.active_session.as_ref().map(|session| (session.agent_id, session.session_id)),
                movement,
                received_at,
            );
            if outcome == MovementCompleteOutcome::Applied {
                self.publish_arrival_agent_motion(simulator_id, movement, received_at);
            }
        }
        let scope = if Some(simulator_id) == self.primary_simulator { SimulatorScope::Primary } else { SimulatorScope::Neighbor };
        let region_name = region.and_then(|region| self.session_world.world().region(region)).and_then(|info| info.name.as_deref());
        handle_simulator_message(
            scope,
            source,
            &message,
            self.active_session.as_ref(),
            self.region_lookup.as_ref(),
            region_name,
            &self.mailbox,
        );
        self.object_cache.record_update(simulator_id, message);
        if self.primary_simulator == Some(simulator_id) {
            self.camera.region_origin_changed(primary_origin_before, self.primary_region_origin());
        }
        self.publish_region_motion();
    }

    fn on_primary_changed(
        &mut self,
        simulator_id: SimulatorId,
        session: AgentSession,
        capabilities: Client,
        login_data: Option<ResponseData>,
    ) {
        self.camera.request_update();
        self.agent_control.cancel_if_simulator_changed(simulator_id);
        self.media.set_client(simulator_id, capabilities.clone());
        self.network_failure = None;
        self.primary_disconnected = false;
        self.primary_simulator = Some(simulator_id);
        self.active_session = Some(session.clone());
        if let Some(login_data) = login_data {
            self.active_login_data = Some(login_data);
        }
        self.active_capabilities = Some(capabilities);
        let lookup = RegionLookup::new(self.session_handle.clone(), simulator_id, &session, self.runtime.clone());
        self.region_lookup = Some(lookup);
        self.publish_session_ready_update(simulator_id, session);
        self.start_favorites_load_for_active();
    }

    fn handle_simulator_disconnect(&mut self, simulator_id: SimulatorId, reason: Option<String>) -> ControlFlow<()> {
        if self.primary_simulator == Some(simulator_id) {
            match reason {
                Some(reason) if !reason.trim().is_empty() => self.network_failure = Some(reason),
                Some(_) => self.network_failure = Some("The primary simulator failed.".into()),
                None => self.primary_disconnected = true,
            }
        }

        let region = self.session_world.region_for(simulator_id).map(|region| region.get());
        tracing::info!(simulator = simulator_id.get(), region, "simulator disconnected");
        self.camera.simulator_disconnected(simulator_id);
        self.environments.simulator_disconnected(&mut self.session_world, simulator_id);
        if let Some(region) = self.session_world.region_for(simulator_id) {
            self.assets.retire(region);
        }
        self.handoff.simulator_disconnected(&mut self.session_world, simulator_id);
        self.motion_statuses.remove(&simulator_id);
        self.publish_region_motion();
        self.object_cache.forget_simulator(simulator_id);
        self.recovery.cancel_simulator(simulator_id);
        self.media.forget_simulator(simulator_id);
        if Some(simulator_id) == self.primary_simulator {
            self.agent_control.cancel_sit(SitOutcome::SimulatorChanged);
            self.primary_simulator = None;
            self.camera.request_update();
        }
        ControlFlow::Continue(())
    }

    fn publish_session_ready_update(&self, simulator_id: SimulatorId, session: AgentSession) {
        self.mailbox.publish(ViewerUpdate::SessionReady(SessionReadyUpdate {
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            environments: self.environment_inbox.clone(),
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            camera: self.camera.sender(),
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            movement: self.camera.movement_sender(),
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            agent_motion: self.agent_motion.receiver(),
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            motion_circuits: self.region_motion_sender.subscribe(),
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            assets: self.assets.handle(),
            session: session.clone(),
            agent_control: self.agent_control_handle.clone(),
            region_name: self.session_world.region_name(simulator_id).map(str::to_owned),
            inventory_updates: self.inventory_changes.subscribe(),
            inventory_fetch: self.inventory.as_ref().map(|_| self.inventory_fetch.handle()),
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            world_reader: self.reads.reader(),
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            sit_response: self.sit_response.clone(),
        }));
    }

    fn process_ingest_summary(&mut self, simulator_id: SimulatorId, summary: IngestSummary) {
        summary.log_incomplete_data(simulator_id, self.session_world.region_for(simulator_id));
        let targets = route_recovery_targets(&self.session_world, summary.recovery_requests());
        let updated = summary.recovery_cancellation_addresses().collect();
        let agent_data =
            self.active_session.as_ref().map(|session| RequestAgentData { agent_id: session.agent_id, session_id: session.session_id });
        self.recovery.schedule(targets, updated, agent_data);
    }

    fn summary_touches_agent(&self, summary: &IngestSummary) -> bool {
        self.active_session
            .as_ref()
            .and_then(|session| self.session_world.world().entity(session.agent_id))
            .is_some_and(|entity| summary.touches_entity(entity))
    }

    fn publish_world_agent_motion(&mut self, received_at: std::time::Instant) {
        let Some(agent_id) = self.active_session.as_ref().map(|session| session.agent_id) else { return };
        self.agent_motion.publish_world(self.session_world.world(), agent_id, received_at);
    }

    fn publish_region_motion(&mut self) {
        let mapping: HashMap<_, _> = self
            .motion_statuses
            .keys()
            .filter_map(|simulator_id| self.session_world.region_for(*simulator_id).map(|region| (region, *simulator_id)))
            .collect();
        if mapping == self.region_motion_mapping {
            return;
        }
        self.region_motion_mapping = mapping;
        let circuits = self
            .region_motion_mapping
            .iter()
            .filter_map(|(region, simulator_id)| self.motion_statuses.get(simulator_id).map(|status| (*region, status.clone())))
            .collect();
        self.region_motion_sender.send_replace(circuits);
    }

    fn publish_arrival_agent_motion(
        &mut self,
        simulator_id: SimulatorId,
        movement: &protocol::messages::AgentMovementComplete,
        received_at: std::time::Instant,
    ) {
        let Some(session) = self.active_session.as_ref() else { return };
        let Some(region) = self.session_world.region_for(simulator_id) else { return };
        let Some(origin) = self.session_world.world().region(region).and_then(world::RegionInfo::resolved_origin) else {
            return;
        };
        let ping_delay = self.motion_statuses.get(&simulator_id).and_then(|status| status.borrow().measured_ping_delay);
        if self.agent_motion.publish_arrival(session.agent_id, session.session_id, movement, region, origin, received_at, ping_delay) {
            self.camera.request_update();
        }
    }

    fn handle_recovery_completion(&mut self, completion: RecoveryCompletion) {
        match completion {
            RecoveryCompletion::Deadline { aged_out, due } => {
                warn_unrecovered("age_limit", &aged_out);
                let (exhausted, recoveries): (Vec<_>, Vec<_>) = due.into_iter().partition(ScheduledRecovery::is_exhausted);
                self.recovery.degrade_recoveries(&exhausted);
                warn_unrecovered("send_limit", &exhausted);
                if let Err(oversized) = self.recovery.start_send(&self.session_handle, recoveries) {
                    self.recovery.degrade_recoveries(&oversized.recoveries);
                    if let Some(first) = oversized.recoveries.first() {
                        tracing::error!(
                            simulator = first.simulator_id.get(),
                            region = first.key.region().get(),
                            "object recovery request exceeded its byte budget"
                        );
                    }
                }
            }
            RecoveryCompletion::Send { recoveries, result, completed_at } => {
                let Some(first) = recoveries.first() else { return };
                let simulator_id = first.simulator_id;
                let region = first.key.region().get();
                match result {
                    Ok(()) => self.recovery.mark_sent(&recoveries, completed_at),
                    Err(error) if retryable_send_error(&error) => {
                        self.recovery.defer(&recoveries, completed_at);
                        tracing::debug!(simulator = simulator_id.get(), region, %error, "object recovery send deferred");
                    }
                    Err(error) => {
                        if simulator_lost_error(&error) {
                            self.recovery.degrade_simulator(simulator_id);
                        } else {
                            self.recovery.degrade_recoveries(&recoveries);
                        }
                        tracing::warn!(simulator = simulator_id.get(), region, %error, "could not request object updates");
                    }
                }
            }
        }
    }

    async fn finish(mut self) {
        if let Some(inventory_bootstrap) = self.inventory_bootstrap.take() {
            inventory_bootstrap.abort();
        }
        self.agent_control.fail_sit(AgentControlError::SessionEnded);
        self.inventory_pending_moves.clear();
        self.inventory_edits.stop().await;
        self.inventory_fetch.stop().await;
        self.recovery.cancel();
        self.reads.finish();
        drop(self.environments);
        drop(self.terrain);
        self.assets.stop().await;
        self.inventory_cache.close().await;
        self.object_cache.close().await;
        for (simulator_id, error) in self.session_world.retire_all() {
            let region = self.session_world.region_for(simulator_id).map(|region| region.get());
            tracing::warn!(simulator = simulator_id.get(), region, %error, "could not retire simulator world region on session shutdown");
        }
    }
}

fn reply_region_handshake(
    runtime: &Handle,
    pending_replies: Arc<Mutex<PendingHandshakeReplies>>,
    session_handle: SessionHandle,
    simulator_id: SimulatorId,
    flags: u32,
) {
    let mut pending = pending_replies.lock().unwrap_or_else(|error| error.into_inner());
    if !pending.by_simulator.contains_key(&simulator_id) {
        match session_handle.try_reply_region_handshake(simulator_id, flags) {
            Ok(response) => {
                drop(pending);
                drop(runtime.spawn(async move {
                    match response.await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            tracing::warn!(simulator = simulator_id.get(), %error, "region handshake reply could not be sent");
                        }
                        Err(_) => {
                            tracing::warn!(simulator = simulator_id.get(), "region handshake reply ended with the network session");
                        }
                    }
                }));
                return;
            }
            Err(network::Error::CommandQueueFull) => {}
            Err(error) => {
                tracing::warn!(simulator = simulator_id.get(), %error, "region handshake reply could not be sent");
                return;
            }
        }
    }
    let should_drain = pending.queue(simulator_id, flags);
    drop(pending);
    if !should_drain {
        return;
    }
    drop(runtime.spawn(async move {
        loop {
            let next_flags = pending_replies.lock().unwrap_or_else(|error| error.into_inner()).take_next(simulator_id);
            let Some(next_flags) = next_flags else { break };
            if let Err(error) = session_handle.reply_region_handshake(simulator_id, next_flags).await {
                tracing::warn!(simulator = simulator_id.get(), %error, "region handshake reply could not be sent");
                pending_replies.lock().unwrap_or_else(|error| error.into_inner()).by_simulator.remove(&simulator_id);
                break;
            }
        }
    }));
}

#[tracing::instrument(name = "viewer.session", level = "info", skip_all)]
pub(crate) async fn drain_session(
    session: Session,
    session_handle: SessionHandle,
    runtime: Handle,
    mailbox: PresentationMailbox,
    cancellation: oneshot::Receiver<()>,
    credentials: CredentialPersistence,
    cache: Option<Arc<assets::cache::AssetCache>>,
) {
    SessionDriver::new(session, session_handle, runtime, mailbox, cancellation, credentials, cache).run().await;
}

async fn process_agent_control_command<T: SitTransport>(
    transport: &T,
    agent_control: &mut AgentControlState,
    camera: &mut CameraUplink,
    primary_simulator: Option<SimulatorId>,
    active_agent: Option<(Uuid, Uuid)>,
    command: AgentControlCommand,
) {
    match command {
        AgentControlCommand::SitOnObject { object_id, offset, reply } => {
            if agent_control.has_pending_sit() {
                if reply.send(Err(AgentControlError::Busy)).is_err() {
                    tracing::trace!("sit caller stopped before receiving the busy outcome");
                }
                return;
            }
            let (Some(simulator), Some((agent_id, session_id))) = (primary_simulator, active_agent) else {
                if reply.send(Err(AgentControlError::SimulatorUnavailable)).is_err() {
                    tracing::trace!("sit caller stopped before receiving the simulator outcome");
                }
                return;
            };
            let request = AgentRequestSit {
                agent_data: SitAgentData { agent_id, session_id },
                target_object: TargetObject { target_id: object_id, offset },
            };
            match transport.send_request(simulator, request, Delivery::Reliable).await {
                Ok(()) => agent_control.begin_sit(object_id, simulator, reply),
                Err(error) => {
                    tracing::debug!(simulator = simulator.get(), %error, "sit request could not be queued");
                    if reply.send(Err(AgentControlError::SendFailed)).is_err() {
                        tracing::trace!("sit caller stopped before receiving the send outcome");
                    }
                }
            }
        }
        AgentControlCommand::SitOnGround { reply } => {
            if primary_simulator.is_none() || active_agent.is_none() {
                if reply.send(Err(AgentControlError::SimulatorUnavailable)).is_err() {
                    tracing::trace!("ground-sit caller stopped before receiving the simulator outcome");
                }
                return;
            }
            agent_control.cancel_sit(SitOutcome::Cancelled);
            agent_control.clear_sit_response();
            camera.request_action(crate::agent_movement::MovementAction::SitOnGround);
            if reply.send(Ok(())).is_err() {
                tracing::trace!("ground-sit caller stopped before receiving the queued outcome");
            }
        }
        AgentControlCommand::StandUp { reply } => {
            if primary_simulator.is_none() || active_agent.is_none() {
                if reply.send(Err(AgentControlError::SimulatorUnavailable)).is_err() {
                    tracing::trace!("stand-up caller stopped before receiving the simulator outcome");
                }
                return;
            }
            agent_control.cancel_sit(SitOutcome::Cancelled);
            agent_control.clear_sit_response();
            camera.request_action(crate::agent_movement::MovementAction::StandUp);
            if reply.send(Ok(())).is_err() {
                tracing::trace!("stand-up caller stopped before receiving the queued outcome");
            }
        }
    }
}

async fn process_sit_response<T: SitTransport>(
    transport: &T,
    agent_control: &mut AgentControlState,
    simulator: SimulatorId,
    active_agent: Option<(Uuid, Uuid)>,
    message: &Message,
) {
    let Some(response) = decode_sit_response(message) else { return };
    if !agent_control.expects_response(simulator, response.object_id) {
        return;
    }
    let Some((agent_id, session_id)) = active_agent else {
        agent_control.fail_sit(AgentControlError::SimulatorUnavailable);
        return;
    };
    let sit = AgentSit { agent_id, session_id };
    match transport.send_sit(simulator, sit, Delivery::Reliable).await {
        Ok(()) => {
            agent_control.accept_sit(
                simulator,
                response.object_id,
                response.auto_pilot,
                response.force_mouselook,
                response.camera_override,
            );
        }
        Err(error) => {
            tracing::debug!(simulator = simulator.get(), %error, "sit acceptance could not be queued");
            agent_control.fail_sit(AgentControlError::SendFailed);
        }
    }
}

fn is_sit_refusal_alert(message: &Message) -> bool {
    let Message::Schema(alert) = message else { return false };
    if message.name() != "AlertMessage" {
        return false;
    }
    let Some(FieldValue::Bytes(message)) =
        alert.blocks().first().and_then(|block| block.instances.first()).and_then(|instance| instance.fields.first())
    else {
        return false;
    };
    std::str::from_utf8(message).is_ok_and(|message| message.trim_end_matches('\0') == "SitFailed")
}

fn handle_sit_alert(
    agent_control: &mut AgentControlState,
    primary_simulator: Option<SimulatorId>,
    simulator: SimulatorId,
    message: &Message,
) -> bool {
    primary_simulator == Some(simulator) && is_sit_refusal_alert(message) && agent_control.refuse_sit(simulator)
}

#[cfg(test)]
mod tests;
