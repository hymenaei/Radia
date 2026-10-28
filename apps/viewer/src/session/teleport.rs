use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};

use llsd::Value;
use math::Vec3;
use network::{
    AgentSession, Error as NetworkError, HandoffCancellationOutcome, HandoffEventOutcome, HandoffGeneration, PacketOrder, SessionHandle,
    SimulatorId, TeleportRequest as NetworkTeleportRequest,
};
use protocol::messages::{
    AlertInfo, TeleportCancel, TeleportFailed, TeleportFinish, TeleportLandmarkRequest as ProtocolLandmarkRequest, TeleportLocal,
    TeleportLocationRequest as ProtocolLocationRequest, TeleportProgress as ProtocolProgress, TeleportStart,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::Instant,
};
use uuid::Uuid;

const COMMAND_CAPACITY: usize = 8;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const TELEPORT_TIMEOUT: Duration = Duration::from_secs(90);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(2);
const CANCEL_RETRY_INTERVAL: Duration = Duration::from_millis(100);
const CANCEL_MAX_ATTEMPTS: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TeleportLocationRequest {
    pub region_handle: u64,
    pub position: Vec3,
    pub look_at: Vec3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TeleportLandmarkRequest {
    pub landmark_asset_id: Uuid,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TeleportRequestError {
    Busy,
    NoActiveTeleport,
    InvalidPosition,
    QueueFull,
    SessionUnavailable,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TeleportSnapshot {
    pub status: TeleportStatus,
    pub crossing: CrossingState,
    pub start_sequence: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TeleportStatus {
    Idle,
    Requested,
    Started { flags: u32 },
    Progress { stage: TeleportProgressKind, flags: u32 },
    Connecting { region_handle: u64 },
    Completed { region_handle: Option<u64>, same_region: bool, flags: u32, sim_access: Option<u8> },
    Failed(TeleportFailure),
    Cancelling { outcome: TeleportCancelOutcome, attempts: u8 },
    CancelFailed { outcome: TeleportCancelOutcome, attempts: u8, reason: TeleportCancelFailure },
    Cancelled,
    TimedOut,
    AwaitingHandoffUpdate,
    CrossingConflict { crossing_region_handle: u64, teleport_region_handle: u64 },
    HandoffConflict { active_region_handle: Option<u64>, conflicting_region_handle: u64 },
    SupersededByCrossing { region_handle: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TeleportCancelOutcome {
    Requested,
    TimedOut,
    CrossingConflict { crossing_region_handle: u64, teleport_region_handle: u64 },
    HandoffConflict { active_region_handle: Option<u64>, conflicting_region_handle: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TeleportCancelFailure {
    QueueFull,
    SessionUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CrossingState {
    None,
    Pending { region_handle: u64, during_teleport: bool },
    Complete { region_handle: u64 },
    Failed { region_handle: u64 },
    Conflict { region_handle: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TeleportProgressKind {
    SendingDestination,
    Redirecting,
    Relaying,
    SendingHome,
    SendingLandmark,
    Completing,
    CompletedFrom,
    Resolving,
    Contacting,
    Arriving,
    Requesting,
    Pending,
    Unknown(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TeleportFailureReason {
    InvalidTeleport,
    InvalidRegionHandoff,
    Blocked,
    LandmarkUnavailable,
    Timeout,
    NoHelpIsland,
    NoAccess,
    MissingAttachment,
    TooManyUploads,
    ExpiredTeleport,
    ExpiredRegionHandoff,
    PreexistingTeleport,
    NoHost,
    NoInventoryHost,
    AgeRequired,
    RegionUsageBlocked,
    DestinationUnavailable,
    Unknown(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeleportFailure {
    pub reason: TeleportFailureReason,
    pub server_reason: Vec<u8>,
    pub alert_info: Vec<TeleportAlertInfo>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeleportAlertInfo {
    pub message: Vec<u8>,
    pub extra_params: Vec<u8>,
}

#[derive(Clone)]
pub struct TeleportHandle {
    commands: mpsc::Sender<TeleportCommand>,
    state: watch::Receiver<TeleportSnapshot>,
}

impl TeleportHandle {
    pub fn state(&self) -> TeleportSnapshot {
        self.state.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<TeleportSnapshot> {
        self.state.clone()
    }

    pub async fn request_location(&self, request: TeleportLocationRequest) -> Result<(), TeleportRequestError> {
        if !request.position.is_finite() || !request.look_at.is_finite() {
            return Err(TeleportRequestError::InvalidPosition);
        }
        self.submit(|completed| TeleportCommand::Request(TeleportRequest::Location(request), completed)).await
    }

    pub async fn request_landmark(&self, request: TeleportLandmarkRequest) -> Result<(), TeleportRequestError> {
        self.submit(|completed| TeleportCommand::Request(TeleportRequest::Landmark(request), completed)).await
    }

    pub async fn teleport_home(&self) -> Result<(), TeleportRequestError> {
        self.request_landmark(TeleportLandmarkRequest { landmark_asset_id: Uuid::nil() }).await
    }

    pub async fn cancel(&self) -> Result<(), TeleportRequestError> {
        self.submit(TeleportCommand::Cancel).await
    }

    async fn submit(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<(), TeleportRequestError>>) -> TeleportCommand,
    ) -> Result<(), TeleportRequestError> {
        let (completed, response) = oneshot::channel();
        self.commands.try_send(command(completed)).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => TeleportRequestError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => TeleportRequestError::SessionUnavailable,
        })?;
        match tokio::time::timeout(COMMAND_TIMEOUT, response).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) | Err(_) => Err(TeleportRequestError::SessionUnavailable),
        }
    }
}

#[derive(Debug)]
pub(super) enum TeleportRequest {
    Location(TeleportLocationRequest),
    Landmark(TeleportLandmarkRequest),
}

#[derive(Debug)]
pub(super) enum TeleportCommand {
    Request(TeleportRequest, oneshot::Sender<Result<(), TeleportRequestError>>),
    Cancel(oneshot::Sender<Result<(), TeleportRequestError>>),
}

#[derive(Debug)]
pub(super) enum TeleportInput {
    Command(TeleportCommand),
    Timeout,
    CancelRetry,
    CancelResult(TeleportCancelResult),
}

#[derive(Debug)]
pub(super) struct TeleportCancelResult {
    generation: u64,
    result: Result<(), TeleportCancelFailure>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Destination {
    pub(super) region_handle: u64,
    pub(super) endpoint: SocketAddr,
}

struct ActiveTeleport {
    started: bool,
    origin_region_handle: Option<u64>,
    destination: Option<Destination>,
    flags: u32,
    sim_access: Option<u8>,
    deadline: Instant,
}

#[derive(Clone, Copy)]
struct PendingCrossing {
    destination: Destination,
    deadline: Instant,
}

struct PendingCancellation {
    generation: u64,
    handoff_generation: Option<HandoffGeneration>,
    outcome: TeleportCancelOutcome,
    attempts: u8,
    deadline: Instant,
    retry_at: Instant,
    last_error: Option<TeleportCancelFailure>,
    in_flight: bool,
    final_result: Option<Result<(), TeleportCancelFailure>>,
}

pub(crate) struct TeleportController {
    command_sender: mpsc::Sender<TeleportCommand>,
    commands: mpsc::Receiver<TeleportCommand>,
    cancel_result_sender: mpsc::Sender<TeleportCancelResult>,
    cancel_results: mpsc::Receiver<TeleportCancelResult>,
    state: watch::Sender<TeleportSnapshot>,
    status: TeleportStatus,
    crossing: CrossingState,
    pending_crossing: Option<PendingCrossing>,
    active: Option<ActiveTeleport>,
    handoff_generation: Option<HandoffGeneration>,
    handoff_destination: Option<Destination>,
    pending_cancellation: Option<PendingCancellation>,
    awaiting_handoff_event: bool,
    latest_handoff_generation: Option<HandoffGeneration>,
    start_sequence: u64,
    last_lifecycle_order: Option<(SimulatorId, u64)>,
    last_capability_order: Option<(SimulatorId, u64, u64, usize)>,
    cancellation_generation: u64,
}

impl TeleportController {
    pub(crate) fn new() -> Self {
        let (command_sender, commands) = mpsc::channel(COMMAND_CAPACITY);
        let (cancel_result_sender, cancel_results) = mpsc::channel(1);
        let initial = TeleportSnapshot { status: TeleportStatus::Idle, crossing: CrossingState::None, start_sequence: 0 };
        let (state, _) = watch::channel(initial);
        Self {
            command_sender,
            commands,
            cancel_result_sender,
            cancel_results,
            state,
            status: TeleportStatus::Idle,
            crossing: CrossingState::None,
            pending_crossing: None,
            active: None,
            handoff_generation: None,
            handoff_destination: None,
            pending_cancellation: None,
            awaiting_handoff_event: false,
            latest_handoff_generation: None,
            start_sequence: 0,
            last_lifecycle_order: None,
            last_capability_order: None,
            cancellation_generation: 0,
        }
    }

    pub(crate) fn handle(&self) -> TeleportHandle {
        TeleportHandle { commands: self.command_sender.clone(), state: self.state.subscribe() }
    }

    pub(super) fn accept_lifecycle_packet(&mut self, simulator_id: SimulatorId, packet_order: PacketOrder) -> bool {
        let PacketOrder::Ordered(order) = packet_order else { return true };
        if self.last_lifecycle_order.is_some_and(|(last_simulator, last_order)| last_simulator == simulator_id && order <= last_order) {
            return false;
        }
        self.last_lifecycle_order = Some((simulator_id, order));
        true
    }

    pub(super) fn accept_capability_order(&mut self, simulator_id: SimulatorId, incarnation: u64, batch: u64, index: usize) -> bool {
        let incoming = (incarnation, batch, index);
        if self.last_capability_order.is_some_and(|(last_simulator, last_incarnation, last_batch, last_index)| {
            last_simulator == simulator_id && incoming <= (last_incarnation, last_batch, last_index)
        }) {
            return false;
        }
        self.last_capability_order = Some((simulator_id, incarnation, batch, index));
        true
    }

    pub(super) fn accept_local_packet(&mut self, simulator_id: SimulatorId, packet_order: PacketOrder) -> bool {
        self.pending_cancellation.is_none() && !self.awaiting_handoff_event && self.accept_lifecycle_packet(simulator_id, packet_order)
    }

    pub(super) async fn next_input(&mut self) -> TeleportInput {
        if let Some(deadline) = self.next_deadline() {
            tokio::select! {
                biased;
                result = self.cancel_results.recv() => TeleportInput::CancelResult(result.expect("teleport cancel result channel stays open")),
                command = self.commands.recv() => TeleportInput::Command(command.expect("teleport command channel stays open")),
                _ = tokio::time::sleep_until(deadline) => {
                    if self.pending_cancellation.is_some() { TeleportInput::CancelRetry } else { TeleportInput::Timeout }
                },
            }
        } else {
            tokio::select! {
                biased;
                result = self.cancel_results.recv() => TeleportInput::CancelResult(result.expect("teleport cancel result channel stays open")),
                command = self.commands.recv() => TeleportInput::Command(command.expect("teleport command channel stays open")),
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        if self.awaiting_handoff_event {
            return None;
        }
        if let Some(cancel) = &self.pending_cancellation {
            if cancel.final_result.is_some() {
                return None;
            }
            return Some(if cancel.in_flight { cancel.deadline } else { cancel.retry_at.min(cancel.deadline) });
        }
        [self.active.as_ref().map(|active| active.deadline), self.pending_crossing.map(|crossing| crossing.deadline)]
            .into_iter()
            .flatten()
            .min()
    }

    pub(super) async fn handle_command(
        &mut self,
        command: TeleportCommand,
        network: &SessionHandle,
        session: Option<&AgentSession>,
        primary_simulator: Option<SimulatorId>,
        origin_region_handle: Option<u64>,
    ) {
        match command {
            TeleportCommand::Request(request, completed) => {
                if completed.is_closed() {
                    return;
                }
                let result = self.request(request, network, session, origin_region_handle).await;
                if let Err(error) = completed.send(result) {
                    drop(error);
                }
            }
            TeleportCommand::Cancel(completed) => {
                if completed.is_closed() {
                    return;
                }
                let result = self.cancel(network, session, primary_simulator);
                if let Err(error) = completed.send(result) {
                    drop(error);
                }
            }
        }
    }

    async fn request(
        &mut self,
        request: TeleportRequest,
        network: &SessionHandle,
        session: Option<&AgentSession>,
        origin_region_handle: Option<u64>,
    ) -> Result<(), TeleportRequestError> {
        self.check_request_available()?;
        let session = session.ok_or(TeleportRequestError::SessionUnavailable)?;
        let request = match request {
            TeleportRequest::Location(request) => NetworkTeleportRequest::Location(ProtocolLocationRequest {
                agent_id: session.agent_id,
                session_id: session.session_id,
                region_handle: request.region_handle,
                position: request.position,
                look_at: request.look_at,
            }),
            TeleportRequest::Landmark(request) => NetworkTeleportRequest::Landmark(ProtocolLandmarkRequest {
                agent_id: session.agent_id,
                session_id: session.session_id,
                landmark_id: request.landmark_asset_id,
            }),
        };
        let previous_status = self.status.clone();
        self.status = TeleportStatus::Requested;
        self.active = Some(ActiveTeleport {
            started: false,
            origin_region_handle,
            destination: None,
            flags: 0,
            sim_access: None,
            deadline: Instant::now() + TELEPORT_TIMEOUT,
        });
        self.publish();
        let result = match tokio::time::timeout(COMMAND_TIMEOUT, network.request_teleport(request)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(NetworkError::SimulatorHandoffInProgress)) => Err(TeleportRequestError::Busy),
            Ok(Err(error)) => Err(map_network_error(error)),
            Err(_) => Err(TeleportRequestError::SessionUnavailable),
        };
        if result.is_err() {
            self.active = None;
            self.status = previous_status;
            self.publish();
        }
        result
    }

    fn request_is_busy(&self) -> bool {
        self.active.is_some()
            || self.pending_cancellation.is_some()
            || self.handoff_generation.is_some()
            || self.awaiting_handoff_event
            || self.pending_crossing.is_some()
            || matches!(self.crossing, CrossingState::Pending { .. })
    }

    fn check_request_available(&self) -> Result<(), TeleportRequestError> {
        if self.request_is_busy() { Err(TeleportRequestError::Busy) } else { Ok(()) }
    }

    fn cancel(
        &mut self,
        network: &SessionHandle,
        session: Option<&AgentSession>,
        primary_simulator: Option<SimulatorId>,
    ) -> Result<(), TeleportRequestError> {
        if self.pending_cancellation.is_some() {
            return Ok(());
        }
        let Some(outcome) = self.cancellation_outcome() else {
            return Err(TeleportRequestError::NoActiveTeleport);
        };
        self.begin_cancellation(outcome);
        self.attempt_cancellation(network, primary_simulator, session);
        Ok(())
    }

    pub(super) fn on_timeout(&mut self, network: &SessionHandle, primary_simulator: Option<SimulatorId>, session: Option<&AgentSession>) {
        let now = Instant::now();
        if !self.expire(now) {
            return;
        }
        self.attempt_cancellation(network, primary_simulator, session);
    }

    fn expire(&mut self, now: Instant) -> bool {
        if self.pending_cancellation.is_some() {
            return false;
        }
        let active_expired = self.active.as_ref().is_some_and(|active| active.deadline <= now);
        let crossing_expired = self.pending_crossing.is_some_and(|crossing| crossing.deadline <= now);
        if active_expired {
            self.begin_cancellation(TeleportCancelOutcome::TimedOut);
            return true;
        }
        if crossing_expired && matches!(self.crossing, CrossingState::Pending { .. }) {
            self.begin_cancellation(TeleportCancelOutcome::TimedOut);
            return true;
        }
        false
    }

    pub(super) fn on_cancel_retry(
        &mut self,
        network: &SessionHandle,
        primary_simulator: Option<SimulatorId>,
        session: Option<&AgentSession>,
    ) {
        let now = Instant::now();
        let Some((deadline, last_error, in_flight, final_result)) = self
            .pending_cancellation
            .as_ref()
            .map(|cancel| (cancel.deadline, cancel.last_error.clone(), cancel.in_flight, cancel.final_result.is_some()))
        else {
            return;
        };
        if final_result {
            return;
        }
        if now >= deadline {
            let reason = if in_flight {
                TeleportCancelFailure::SessionUnavailable
            } else {
                last_error.unwrap_or(TeleportCancelFailure::SessionUnavailable)
            };
            self.finish_cancellation_attempt(Err(reason), now);
        } else {
            self.attempt_cancellation(network, primary_simulator, session);
        }
    }

    pub(super) fn cancel_conflicted(
        &mut self,
        network: &SessionHandle,
        primary_simulator: Option<SimulatorId>,
        session: Option<&AgentSession>,
    ) {
        if self.pending_cancellation.is_some() {
            return;
        }
        let outcome = self.cancellation_outcome().unwrap_or(TeleportCancelOutcome::Requested);
        self.begin_cancellation(outcome);
        self.attempt_cancellation(network, primary_simulator, session);
    }

    fn cancellation_outcome(&self) -> Option<TeleportCancelOutcome> {
        match &self.status {
            TeleportStatus::CrossingConflict { crossing_region_handle, teleport_region_handle } => {
                Some(TeleportCancelOutcome::CrossingConflict {
                    crossing_region_handle: *crossing_region_handle,
                    teleport_region_handle: *teleport_region_handle,
                })
            }
            TeleportStatus::HandoffConflict { active_region_handle, conflicting_region_handle } => {
                Some(TeleportCancelOutcome::HandoffConflict {
                    active_region_handle: *active_region_handle,
                    conflicting_region_handle: *conflicting_region_handle,
                })
            }
            _ if self.active.is_some() || self.pending_crossing.is_some() || self.handoff_generation.is_some() => {
                Some(TeleportCancelOutcome::Requested)
            }
            _ => None,
        }
    }

    pub(super) fn begin_cancellation(&mut self, outcome: TeleportCancelOutcome) {
        self.active = None;
        let now = Instant::now();
        self.cancellation_generation = self.cancellation_generation.wrapping_add(1);
        self.pending_cancellation = Some(PendingCancellation {
            generation: self.cancellation_generation,
            handoff_generation: self.handoff_generation,
            outcome: outcome.clone(),
            attempts: 0,
            deadline: now + CANCEL_TIMEOUT,
            retry_at: now,
            last_error: None,
            in_flight: false,
            final_result: None,
        });
        self.status = TeleportStatus::Cancelling { outcome, attempts: 0 };
        self.publish();
    }

    fn attempt_cancellation(&mut self, network: &SessionHandle, primary_simulator: Option<SimulatorId>, session: Option<&AgentSession>) {
        if !self.prepare_cancellation_attempt(Instant::now()) {
            return;
        }
        let Some(primary_simulator) = primary_simulator else {
            self.finish_cancellation_attempt(Err(TeleportCancelFailure::SessionUnavailable), Instant::now());
            return;
        };
        let Some(session) = session else {
            self.finish_cancellation_attempt(Err(TeleportCancelFailure::SessionUnavailable), Instant::now());
            return;
        };
        let Some(cancel) = self.pending_cancellation.as_ref() else { return };
        let generation = cancel.generation;
        let handoff_generation = cancel.handoff_generation;
        let timeout = cancel.deadline.saturating_duration_since(Instant::now());
        let agent_id = session.agent_id;
        let session_id = session.session_id;
        let network = network.clone();
        let result_sender = self.cancel_result_sender.clone();
        tokio::spawn(async move {
            let result = match tokio::time::timeout(
                timeout,
                network.send_teleport_cancel(
                    primary_simulator,
                    TeleportCancel { agent_id, session_id },
                    network::Delivery::Reliable,
                    handoff_generation,
                ),
            )
            .await
            {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(map_cancel_error(error)),
                Err(_) => Err(TeleportCancelFailure::SessionUnavailable),
            };
            if let Err(error) = result_sender.send(TeleportCancelResult { generation, result }).await {
                tracing::debug!(%error, "teleport cancellation result arrived after its session ended");
            }
        });
    }

    fn prepare_cancellation_attempt(&mut self, now: Instant) -> bool {
        let Some(current) = self.pending_cancellation.as_ref() else { return false };
        if current.final_result.is_some() {
            return false;
        }
        if current.in_flight && now < current.deadline {
            return false;
        }
        if current.attempts >= CANCEL_MAX_ATTEMPTS || now >= current.deadline {
            let reason = current.last_error.clone().unwrap_or(TeleportCancelFailure::SessionUnavailable);
            self.finish_cancellation_attempt(Err(reason), now);
            return false;
        }
        let Some(cancel) = self.pending_cancellation.as_mut() else { return false };
        if cancel.in_flight {
            return false;
        }
        cancel.attempts = cancel.attempts.saturating_add(1);
        cancel.in_flight = true;
        self.status = TeleportStatus::Cancelling { outcome: cancel.outcome.clone(), attempts: cancel.attempts };
        self.publish();
        true
    }

    fn finish_cancellation_attempt(&mut self, result: Result<(), TeleportCancelFailure>, now: Instant) {
        let Some(cancel) = self.pending_cancellation.as_mut() else { return };
        if cancel.final_result.is_some() {
            return;
        }
        cancel.in_flight = false;
        let attempts = cancel.attempts;
        let deadline = cancel.deadline;
        let generation = cancel.generation;
        match result {
            Ok(()) => {
                cancel.final_result = Some(Ok(()));
            }
            Err(reason) => {
                if attempts >= CANCEL_MAX_ATTEMPTS || now >= deadline {
                    cancel.final_result = Some(Err(reason));
                } else {
                    if let Some(cancel) = self.pending_cancellation.as_mut()
                        && cancel.generation == generation
                    {
                        cancel.last_error = Some(reason);
                        cancel.retry_at = (now + CANCEL_RETRY_INTERVAL).min(deadline);
                        self.status = TeleportStatus::Cancelling { outcome: cancel.outcome.clone(), attempts };
                    }
                }
            }
        }
        self.publish();
    }

    pub(super) fn handoff_cancellation_ready(&self) -> bool {
        self.pending_cancellation.as_ref().is_some_and(|cancel| cancel.final_result.is_some())
    }

    pub(super) fn cancellation_handoff_generation(&self) -> Option<HandoffGeneration> {
        self.pending_cancellation.as_ref().and_then(|cancel| cancel.handoff_generation)
    }

    pub(super) fn finish_cancellation(&mut self, handoff_result: Result<HandoffCancellationOutcome, TeleportCancelFailure>) {
        let Some(cancel) = self.pending_cancellation.take() else { return };
        let Some(send_result) = cancel.final_result else {
            self.pending_cancellation = Some(cancel);
            return;
        };
        self.status = match handoff_result {
            Ok(HandoffCancellationOutcome::Stale) => {
                self.awaiting_handoff_event = true;
                TeleportStatus::AwaitingHandoffUpdate
            }
            Ok(HandoffCancellationOutcome::Cancelled) => {
                self.finish_cancelled_state(&cancel.outcome);
                match send_result {
                    Ok(()) => terminal_cancel_status(cancel.outcome),
                    Err(reason) => TeleportStatus::CancelFailed { outcome: cancel.outcome, attempts: cancel.attempts, reason },
                }
            }
            Err(reason) => {
                self.finish_cancelled_state(&cancel.outcome);
                TeleportStatus::CancelFailed { outcome: cancel.outcome, attempts: cancel.attempts, reason }
            }
        };
        self.publish();
    }

    fn finish_cancelled_state(&mut self, outcome: &TeleportCancelOutcome) {
        self.handoff_generation = None;
        self.handoff_destination = None;
        self.awaiting_handoff_event = false;
        if let Some(crossing) = self.pending_crossing.take() {
            self.crossing = if matches!(outcome, TeleportCancelOutcome::TimedOut) {
                CrossingState::Failed { region_handle: crossing.destination.region_handle }
            } else {
                CrossingState::None
            };
        } else if matches!(self.crossing, CrossingState::Conflict { .. } | CrossingState::Pending { .. }) {
            self.crossing = CrossingState::None;
        }
    }

    fn observe_handoff_outcome(&mut self, destination: Destination, outcome: HandoffEventOutcome) {
        if !self.accept_handoff_outcome(outcome) {
            return;
        }
        if let HandoffEventOutcome::Accepted { generation: Some(generation) } = outcome {
            self.latest_handoff_generation = Some(generation);
            self.handoff_generation = Some(generation);
            self.handoff_destination = Some(destination);
        } else if let HandoffEventOutcome::Conflict { generation } = outcome {
            self.latest_handoff_generation = Some(generation);
            self.handoff_generation = Some(generation);
        }
        self.awaiting_handoff_event = false;
        if self
            .pending_cancellation
            .as_ref()
            .is_some_and(|cancel| outcome.generation().is_some_and(|generation| Some(generation) != cancel.handoff_generation))
        {
            self.pending_cancellation = None;
        }
    }

    fn accept_handoff_outcome(&self, outcome: HandoffEventOutcome) -> bool {
        let Some(generation) = outcome.generation() else {
            return outcome.is_accepted();
        };
        match self.latest_handoff_generation {
            None => true,
            Some(latest) if generation.get() > latest.get() => true,
            Some(latest) if generation == latest => self.handoff_generation == Some(generation),
            Some(_) => false,
        }
    }

    pub(super) fn on_cancel_result(&mut self, result: TeleportCancelResult) {
        let now = Instant::now();
        if self.pending_cancellation.as_ref().is_some_and(|cancel| cancel.generation == result.generation && cancel.in_flight) {
            self.finish_cancellation_attempt(result.result, now);
        }
    }

    pub(super) fn on_start(&mut self, message: &TeleportStart, origin_region_handle: Option<u64>) {
        if self.pending_cancellation.is_some() || self.awaiting_handoff_event {
            return;
        }
        self.start_sequence = self.start_sequence.wrapping_add(1);
        let active = self.active.get_or_insert_with(|| ActiveTeleport {
            started: false,
            origin_region_handle,
            destination: None,
            flags: 0,
            sim_access: None,
            deadline: Instant::now() + TELEPORT_TIMEOUT,
        });
        active.started = true;
        active.flags = message.teleport_flags;
        self.status = TeleportStatus::Started { flags: message.teleport_flags };
        self.publish();
    }

    pub(super) fn on_progress(&mut self, message: &ProtocolProgress, agent_id: Uuid) {
        if message.agent_id != agent_id || self.active.is_none() || self.awaiting_handoff_event {
            return;
        }
        let stage = progress_kind(&message.message);
        if let Some(active) = &mut self.active {
            active.flags = message.teleport_flags;
        }
        self.status = TeleportStatus::Progress { stage, flags: message.teleport_flags };
        self.publish();
    }

    pub(super) fn on_finish(&mut self, destination: Destination, flags: u32, sim_access: Option<u8>, outcome: HandoffEventOutcome) -> bool {
        if !self.accept_handoff_outcome(outcome) {
            return false;
        }
        self.observe_handoff_outcome(destination, outcome);
        if self.pending_cancellation.is_some() {
            return false;
        }
        if let (Some(crossing), CrossingState::Pending { region_handle, .. }) = (self.pending_crossing, &self.crossing)
            && !same_destination(crossing.destination, destination)
        {
            let crossing_region_handle = *region_handle;
            self.active = None;
            self.pending_crossing = None;
            self.crossing = CrossingState::Conflict { region_handle: *region_handle };
            self.status = TeleportStatus::CrossingConflict { crossing_region_handle, teleport_region_handle: destination.region_handle };
            self.publish();
            return true;
        }
        if outcome.is_conflict() {
            let active_region_handle =
                self.active.as_ref().and_then(|active| active.destination.map(|destination| destination.region_handle));
            self.active = None;
            self.status = TeleportStatus::HandoffConflict { active_region_handle, conflicting_region_handle: destination.region_handle };
            self.publish();
            return true;
        }
        let active = self.active.get_or_insert_with(|| ActiveTeleport {
            started: false,
            origin_region_handle: None,
            destination: None,
            flags: 0,
            sim_access: None,
            deadline: Instant::now() + TELEPORT_TIMEOUT,
        });
        active.destination = Some(destination);
        active.flags = flags;
        active.sim_access = sim_access;
        self.status = TeleportStatus::Connecting { region_handle: destination.region_handle };
        self.publish();
        false
    }

    pub(super) fn on_failed(&mut self, failure: TeleportFailure, outcome: HandoffEventOutcome) {
        if !outcome.is_accepted() || !self.accept_handoff_outcome(outcome) {
            return;
        }
        if let Some(generation) = outcome.generation() {
            self.latest_handoff_generation = Some(generation);
        }
        let cancellation_matches =
            self.pending_cancellation.as_ref().is_some_and(|cancel| cancel.handoff_generation == outcome.generation());
        if cancellation_matches
            || outcome.generation().is_some_and(|generation| {
                self.pending_cancellation.as_ref().is_some_and(|cancel| Some(generation) != cancel.handoff_generation)
            })
        {
            self.pending_cancellation = None;
        }
        if self.pending_cancellation.is_some() {
            return;
        }
        let was_awaiting = self.awaiting_handoff_event;
        self.awaiting_handoff_event = false;
        self.handoff_generation = None;
        self.handoff_destination = None;
        if self.active.take().is_some() || was_awaiting {
            self.status = TeleportStatus::Failed(failure);
            self.publish();
        }
    }

    pub(super) fn on_local(&mut self, message: &TeleportLocal, current_region_handle: Option<u64>, agent_id: Uuid) -> bool {
        if message.agent_id != agent_id || self.pending_cancellation.is_some() || self.awaiting_handoff_event {
            return false;
        }
        let flags = message.teleport_flags;
        self.active = None;
        self.status = TeleportStatus::Completed { region_handle: current_region_handle, same_region: true, flags, sim_access: None };
        self.publish();
        true
    }

    pub(super) fn on_crossed_region(&mut self, destination: Destination, outcome: HandoffEventOutcome) -> bool {
        if !self.accept_handoff_outcome(outcome) {
            return false;
        }
        let superseded_cancellation = self
            .pending_cancellation
            .as_ref()
            .is_some_and(|cancel| outcome.generation().is_some_and(|generation| Some(generation) != cancel.handoff_generation));
        self.observe_handoff_outcome(destination, outcome);
        if self.pending_cancellation.is_some() {
            return false;
        }
        let during_teleport = self.active.as_ref().is_some_and(|active| active.started);
        if let Some(teleport_destination) = self.active.as_ref().and_then(|active| active.destination)
            && !same_destination(teleport_destination, destination)
        {
            self.active = None;
            self.pending_crossing = None;
            self.crossing = CrossingState::Conflict { region_handle: destination.region_handle };
            self.status = TeleportStatus::CrossingConflict {
                crossing_region_handle: destination.region_handle,
                teleport_region_handle: teleport_destination.region_handle,
            };
            self.publish();
            return true;
        }
        if outcome.is_conflict() {
            let active_region_handle =
                self.active.as_ref().and_then(|active| active.destination.map(|destination| destination.region_handle));
            self.active = None;
            self.pending_crossing = None;
            self.crossing = CrossingState::Conflict { region_handle: destination.region_handle };
            self.status = TeleportStatus::HandoffConflict { active_region_handle, conflicting_region_handle: destination.region_handle };
            self.publish();
            return true;
        }
        let deadline = self
            .pending_crossing
            .filter(|pending| same_destination(pending.destination, destination))
            .map_or_else(|| Instant::now() + TELEPORT_TIMEOUT, |pending| pending.deadline);
        self.pending_crossing = Some(PendingCrossing { destination, deadline });
        self.crossing = CrossingState::Pending { region_handle: destination.region_handle, during_teleport };
        if superseded_cancellation {
            self.active = None;
            self.status = TeleportStatus::SupersededByCrossing { region_handle: destination.region_handle };
        }
        self.publish();
        false
    }

    pub(super) fn on_simulator_connect_failed(&mut self, region_handle: u64, endpoint: Option<SocketAddr>) {
        let handoff_failed = self.handoff_destination.is_some_and(|destination| {
            destination.region_handle == region_handle && endpoint.is_none_or(|endpoint| destination.endpoint == endpoint)
        });
        if matches!(self.crossing, CrossingState::Pending { region_handle: current, .. } if current == region_handle)
            && self.pending_crossing.is_some_and(|pending| endpoint.is_none_or(|endpoint| pending.destination.endpoint == endpoint))
        {
            self.pending_crossing = None;
            self.crossing = CrossingState::Failed { region_handle };
        }
        if self.active.as_ref().is_some_and(|active| {
            active.destination.is_some_and(|destination| {
                destination.region_handle == region_handle && endpoint.is_none_or(|endpoint| destination.endpoint == endpoint)
            })
        }) {
            self.active = None;
            self.status = TeleportStatus::Failed(TeleportFailure {
                reason: TeleportFailureReason::DestinationUnavailable,
                server_reason: Vec::new(),
                alert_info: Vec::new(),
            });
        }
        if handoff_failed {
            self.pending_cancellation = None;
            self.handoff_generation = None;
            self.handoff_destination = None;
            self.awaiting_handoff_event = false;
        }
        self.publish();
    }

    pub(super) fn on_simulator_promoted(&mut self, region_handle: Option<u64>, endpoint: SocketAddr) -> bool {
        let promotion_matches_handoff = self
            .handoff_destination
            .is_some_and(|destination| Some(destination.region_handle) == region_handle && destination.endpoint == endpoint);
        let completed_crossing = if let (Some(region_handle), Some(crossing), CrossingState::Pending { region_handle: pending, .. }) =
            (region_handle, self.pending_crossing, &self.crossing)
            && *pending == region_handle
            && crossing.destination.endpoint == endpoint
        {
            self.pending_crossing = None;
            self.crossing = CrossingState::Complete { region_handle };
            Some(region_handle)
        } else {
            None
        };
        let completed = self
            .active
            .as_ref()
            .and_then(|active| active.destination)
            .filter(|destination| Some(destination.region_handle) == region_handle && destination.endpoint == endpoint);
        if let Some(destination) = completed {
            let active = self.active.take().expect("active teleport matches promoted destination");
            self.pending_cancellation = None;
            self.handoff_generation = None;
            self.handoff_destination = None;
            self.awaiting_handoff_event = false;
            self.status = TeleportStatus::Completed {
                region_handle: Some(destination.region_handle),
                same_region: active.origin_region_handle == Some(destination.region_handle),
                flags: active.flags,
                sim_access: active.sim_access,
            };
            self.publish();
            true
        } else if let Some(region_handle) = completed_crossing
            && self.active.take().is_some()
        {
            self.pending_cancellation = None;
            self.handoff_generation = None;
            self.handoff_destination = None;
            self.awaiting_handoff_event = false;
            self.status = TeleportStatus::SupersededByCrossing { region_handle };
            self.publish();
            false
        } else if promotion_matches_handoff {
            self.pending_cancellation = None;
            self.handoff_generation = None;
            self.handoff_destination = None;
            self.awaiting_handoff_event = false;
            if let Some(region_handle) = region_handle {
                self.status = TeleportStatus::SupersededByCrossing { region_handle };
            }
            self.publish();
            false
        } else {
            self.publish();
            false
        }
    }

    pub(super) fn region_handle_for_promotion(&self, endpoint: SocketAddr) -> Option<u64> {
        self.active
            .as_ref()
            .and_then(|active| active.destination)
            .filter(|destination| destination.endpoint == endpoint)
            .map(|destination| destination.region_handle)
            .or_else(|| {
                self.pending_crossing
                    .filter(|crossing| crossing.destination.endpoint == endpoint)
                    .map(|crossing| crossing.destination.region_handle)
            })
    }

    pub(super) fn publish(&self) {
        self.state.send_replace(TeleportSnapshot {
            status: self.status.clone(),
            crossing: self.crossing.clone(),
            start_sequence: self.start_sequence,
        });
    }
}

impl Default for TeleportController {
    fn default() -> Self {
        Self::new()
    }
}

pub(super) fn destination_from_udp(message: &TeleportFinish) -> Option<Destination> {
    let port = u16::from_be_bytes(message.sim_port);
    (port != 0)
        .then_some(Destination { region_handle: message.region_handle, endpoint: SocketAddr::from((Ipv4Addr::from(message.sim_ip), port)) })
}

pub(super) fn finish_metadata_from_event(body: &Value, agent_id: Uuid) -> Option<(Destination, u32, Option<u8>)> {
    let info = event_block(event_fields(body)?, "Info")?;
    if !matches!(info.get("AgentID"), Some(Value::Uuid(id)) if *id == agent_id) {
        return None;
    }
    let destination = decode_destination(info, "RegionHandle")?;
    let flags = match info.get("TeleportFlags") {
        Some(Value::Integer(flags)) => u32::try_from(*flags).ok()?,
        Some(Value::Binary(flags)) => u32::from_be_bytes(flags.as_slice().try_into().ok()?),
        _ => 0,
    };
    let sim_access = match info.get("SimAccess") {
        Some(Value::Integer(access)) => u8::try_from(*access).ok(),
        Some(Value::Binary(access)) => access.as_slice().first().copied(),
        _ => None,
    };
    Some((destination, flags, sim_access))
}

pub(super) fn crossed_region_from_event(body: &Value, agent_id: Uuid, session_id: Uuid) -> Option<Destination> {
    let fields = event_fields(body)?;
    let agent = event_block(fields, "AgentData")?;
    if !matches!(agent.get("AgentID"), Some(Value::Uuid(id)) if *id == agent_id)
        || !matches!(agent.get("SessionID"), Some(Value::Uuid(id)) if *id == session_id)
    {
        return None;
    }
    let region = event_block(fields, "RegionData")?;
    decode_destination(region, "RegionHandle")
}

pub(super) fn failure_from_udp(message: &TeleportFailed) -> TeleportFailure {
    failure(message.reason.clone(), message.alert_info.iter().map(alert_info).collect())
}

pub(super) fn failure_from_event(body: &Value, agent_id: Uuid) -> Option<TeleportFailure> {
    let info = event_block(event_fields(body)?, "Info")?;
    if !matches!(info.get("AgentID"), Some(Value::Uuid(id)) if *id == agent_id) {
        return None;
    }
    let reason = info.get("Reason").and_then(value_bytes).unwrap_or_default();
    let alerts = match info.get("AlertInfo") {
        Some(Value::Array(alerts)) => alerts.iter().filter_map(event_alert_info).collect(),
        Some(Value::Map(alert)) => event_alert_info(&Value::Map(alert.clone())).into_iter().collect(),
        _ => Vec::new(),
    };
    Some(failure(reason, alerts))
}

fn failure(server_reason: Vec<u8>, alert_info: Vec<TeleportAlertInfo>) -> TeleportFailure {
    let alert_reason = alert_info.first().map(|alert| alert.message.as_slice()).unwrap_or_default();
    let reason = parse_failure_reason(&server_reason).or_else(|| parse_failure_reason(alert_reason)).unwrap_or_else(|| {
        TeleportFailureReason::Unknown(if server_reason.is_empty() { alert_reason.to_vec() } else { server_reason.clone() })
    });
    TeleportFailure { reason, server_reason, alert_info }
}

fn parse_failure_reason(value: &[u8]) -> Option<TeleportFailureReason> {
    let value = value.strip_suffix(&[0]).unwrap_or(value);
    Some(match value {
        b"invalid_tport" => TeleportFailureReason::InvalidTeleport,
        b"invalid_region_handoff" => TeleportFailureReason::InvalidRegionHandoff,
        b"blocked_tport" => TeleportFailureReason::Blocked,
        b"nolandmark_tport" => TeleportFailureReason::LandmarkUnavailable,
        b"timeout_tport" => TeleportFailureReason::Timeout,
        b"NoHelpIslandTP" => TeleportFailureReason::NoHelpIsland,
        b"noaccess_tport" => TeleportFailureReason::NoAccess,
        b"missing_attach_tport" => TeleportFailureReason::MissingAttachment,
        b"too_many_uploads_tport" => TeleportFailureReason::TooManyUploads,
        b"expired_tport" => TeleportFailureReason::ExpiredTeleport,
        b"expired_region_handoff" => TeleportFailureReason::ExpiredRegionHandoff,
        b"preexisting_tport" => TeleportFailureReason::PreexistingTeleport,
        b"no_host" => TeleportFailureReason::NoHost,
        b"no_inventory_host" => TeleportFailureReason::NoInventoryHost,
        b"MustGetAgeRegion" => TeleportFailureReason::AgeRequired,
        b"RegionTPSpecialUsageBlocked" => TeleportFailureReason::RegionUsageBlocked,
        _ => return None,
    })
}

fn progress_kind(message: &[u8]) -> TeleportProgressKind {
    let message = message.strip_suffix(&[0]).unwrap_or(message);
    match message {
        b"sending_dest" => TeleportProgressKind::SendingDestination,
        b"redirecting" => TeleportProgressKind::Redirecting,
        b"relaying" => TeleportProgressKind::Relaying,
        b"sending_home" => TeleportProgressKind::SendingHome,
        b"sending_landmark" => TeleportProgressKind::SendingLandmark,
        b"completing" => TeleportProgressKind::Completing,
        b"completed_from" => TeleportProgressKind::CompletedFrom,
        b"resolving" => TeleportProgressKind::Resolving,
        b"contacting" => TeleportProgressKind::Contacting,
        b"arriving" => TeleportProgressKind::Arriving,
        b"requesting" => TeleportProgressKind::Requesting,
        b"pending" => TeleportProgressKind::Pending,
        _ => TeleportProgressKind::Unknown(message.to_vec()),
    }
}

fn same_destination(left: Destination, right: Destination) -> bool {
    left.region_handle == right.region_handle && left.endpoint == right.endpoint
}

fn decode_destination(fields: &BTreeMap<String, Value>, handle_name: &str) -> Option<Destination> {
    let Value::Binary(handle) = fields.get(handle_name)? else { return None };
    let region_handle = u64::from_be_bytes(handle.as_slice().try_into().ok()?);
    let Value::Binary(ip) = fields.get("SimIP")? else { return None };
    let ip = Ipv4Addr::from(<[u8; 4]>::try_from(ip.as_slice()).ok()?);
    let port = match fields.get("SimPort")? {
        Value::Integer(port) => u16::try_from(*port).ok()?,
        Value::Binary(port) => u16::from_be_bytes(port.as_slice().try_into().ok()?),
        _ => return None,
    };
    (port != 0).then_some(Destination { region_handle, endpoint: SocketAddr::from((ip, port)) })
}

fn event_fields(value: &Value) -> Option<&BTreeMap<String, Value>> {
    let Value::Map(fields) = value else { return None };
    Some(fields)
}

fn event_block<'a>(fields: &'a BTreeMap<String, Value>, name: &str) -> Option<&'a BTreeMap<String, Value>> {
    match fields.get(name)? {
        Value::Map(block) => Some(block),
        Value::Array(blocks) => match blocks.as_slice() {
            [Value::Map(block)] => Some(block),
            _ => None,
        },
        _ => None,
    }
}

fn event_alert_info(value: &Value) -> Option<TeleportAlertInfo> {
    let Value::Map(fields) = value else { return None };
    Some(TeleportAlertInfo {
        message: value_bytes(fields.get("Message")?)?,
        extra_params: fields.get("ExtraParams").and_then(value_bytes).unwrap_or_default(),
    })
}

fn value_bytes(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::Binary(bytes) => Some(bytes.to_vec()),
        Value::String(value) => Some(value.as_bytes().to_vec()),
        _ => None,
    }
}

fn alert_info(info: &AlertInfo) -> TeleportAlertInfo {
    TeleportAlertInfo { message: info.message.clone(), extra_params: info.extra_params.clone() }
}

fn map_network_error(error: NetworkError) -> TeleportRequestError {
    match error {
        NetworkError::SimulatorHandoffInProgress => TeleportRequestError::Busy,
        NetworkError::CommandQueueFull | NetworkError::SimulatorCommandQueueFull => TeleportRequestError::QueueFull,
        NetworkError::SessionClosed
        | NetworkError::SimulatorConnectionClosed
        | NetworkError::SimulatorUnavailable
        | NetworkError::SimulatorNotReady
        | NetworkError::LoginEnded => TeleportRequestError::SessionUnavailable,
        NetworkError::InvalidTeleportRequest => TeleportRequestError::InvalidPosition,
        _ => TeleportRequestError::SessionUnavailable,
    }
}

fn map_cancel_error(error: NetworkError) -> TeleportCancelFailure {
    match error {
        NetworkError::CommandQueueFull | NetworkError::SimulatorCommandQueueFull => TeleportCancelFailure::QueueFull,
        _ => TeleportCancelFailure::SessionUnavailable,
    }
}

fn terminal_cancel_status(outcome: TeleportCancelOutcome) -> TeleportStatus {
    match outcome {
        TeleportCancelOutcome::Requested => TeleportStatus::Cancelled,
        TeleportCancelOutcome::TimedOut => TeleportStatus::TimedOut,
        TeleportCancelOutcome::CrossingConflict { crossing_region_handle, teleport_region_handle } => {
            TeleportStatus::CrossingConflict { crossing_region_handle, teleport_region_handle }
        }
        TeleportCancelOutcome::HandoffConflict { active_region_handle, conflicting_region_handle } => {
            TeleportStatus::HandoffConflict { active_region_handle, conflicting_region_handle }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::*;
    use llsd::Map;
    use proptest::prelude::*;
    use protocol::messages::TeleportStart;

    fn handoff_generation(value: u64) -> HandoffGeneration {
        HandoffGeneration::new(std::num::NonZeroU64::new(value).expect("handoff generation must be nonzero"))
    }

    fn accepted_outcome() -> HandoffEventOutcome {
        HandoffEventOutcome::accepted(None)
    }

    fn conflict_outcome() -> HandoffEventOutcome {
        HandoffEventOutcome::conflict(handoff_generation(1))
    }

    fn cancelled_handoff() -> Result<HandoffCancellationOutcome, TeleportCancelFailure> {
        Ok(HandoffCancellationOutcome::Cancelled)
    }

    #[test]
    fn start_sequence_survives_a_fast_local_completion() {
        let agent_id = Uuid::from_u128(1);
        let mut controller = TeleportController::new();
        let handle = controller.handle();

        assert_eq!(handle.state().start_sequence, 0);
        controller.on_start(&TeleportStart { teleport_flags: 4 }, Some(22));
        assert_eq!(handle.state().start_sequence, 1);
        assert!(controller.on_local(
            &TeleportLocal { agent_id, location_id: 0, position: Vec3::new(10.0, 20.0, 30.0), look_at: Vec3::Y, teleport_flags: 4 },
            Some(22),
            agent_id,
        ));
        assert_eq!(handle.state().start_sequence, 1);
    }

    #[test]
    fn official_failure_reason_classes_are_typed_and_keep_alert_info() {
        let cases = [
            (b"invalid_tport".as_slice(), TeleportFailureReason::InvalidTeleport),
            (b"invalid_region_handoff", TeleportFailureReason::InvalidRegionHandoff),
            (b"blocked_tport", TeleportFailureReason::Blocked),
            (b"nolandmark_tport", TeleportFailureReason::LandmarkUnavailable),
            (b"timeout_tport", TeleportFailureReason::Timeout),
            (b"NoHelpIslandTP", TeleportFailureReason::NoHelpIsland),
            (b"noaccess_tport", TeleportFailureReason::NoAccess),
            (b"missing_attach_tport", TeleportFailureReason::MissingAttachment),
            (b"too_many_uploads_tport", TeleportFailureReason::TooManyUploads),
            (b"expired_tport", TeleportFailureReason::ExpiredTeleport),
            (b"expired_region_handoff", TeleportFailureReason::ExpiredRegionHandoff),
            (b"preexisting_tport", TeleportFailureReason::PreexistingTeleport),
            (b"no_host", TeleportFailureReason::NoHost),
            (b"no_inventory_host", TeleportFailureReason::NoInventoryHost),
            (b"MustGetAgeRegion", TeleportFailureReason::AgeRequired),
            (b"RegionTPSpecialUsageBlocked", TeleportFailureReason::RegionUsageBlocked),
        ];
        for (reason, expected) in cases {
            let alert_message = [reason, &[0]].concat();
            let alert_info = vec![TeleportAlertInfo { message: alert_message, extra_params: b"params".to_vec() }];
            let failure = failure([reason, &[0]].concat(), alert_info.clone());
            assert_eq!(failure.reason, expected);
            assert_eq!(failure.server_reason, [reason, &[0]].concat());
            assert_eq!(failure.alert_info, alert_info);
            assert_eq!(super::failure(Vec::new(), alert_info.clone()).reason, expected);

            let mut controller = TeleportController::new();
            controller.on_start(&TeleportStart { teleport_flags: 0 }, Some(1));
            controller.on_failed(failure, accepted_outcome());
            std::assert_matches!(controller.state.borrow().status, TeleportStatus::Failed(_));
        }
        assert_eq!(failure(b"custom_reason".to_vec(), Vec::new()).reason, TeleportFailureReason::Unknown(b"custom_reason".to_vec()));
    }

    #[test]
    fn teleport_local_completes_without_changing_regions() {
        let agent_id = Uuid::from_u128(1);
        let mut controller = TeleportController::new();
        controller.on_start(&TeleportStart { teleport_flags: 4 }, Some(22));

        assert!(controller.on_local(
            &TeleportLocal { agent_id, location_id: 0, position: Vec3::new(10.0, 20.0, 30.0), look_at: Vec3::Y, teleport_flags: 4 },
            Some(22),
            agent_id,
        ));
        assert_eq!(
            controller.state.borrow().status,
            TeleportStatus::Completed { region_handle: Some(22), same_region: true, flags: 4, sim_access: None }
        );
    }

    #[test]
    fn event_queue_finish_metadata_is_typed_without_seed_capability() {
        let agent_id = Uuid::from_u128(1);
        let region_handle = 0x1122_3344_5566_7788_u64;
        let info = Map::from([
            ("AgentID".into(), Value::Uuid(agent_id)),
            ("RegionHandle".into(), Value::Binary(region_handle.to_be_bytes().to_vec().into())),
            ("SimIP".into(), Value::Binary([127, 0, 0, 1].to_vec().into())),
            ("SimPort".into(), Value::Integer(13000)),
            ("TeleportFlags".into(), Value::Integer(7)),
            ("SimAccess".into(), Value::Integer(13)),
        ]);
        let body = Value::Map(Map::from([("Info".into(), Value::Array(vec![Value::Map(info)]))]));

        assert_eq!(
            finish_metadata_from_event(&body, agent_id),
            Some((Destination { region_handle, endpoint: "127.0.0.1:13000".parse().expect("simulator endpoint") }, 7, Some(13)))
        );
    }

    #[test]
    fn crossing_and_teleport_destinations_compare_region_and_endpoint() {
        let left = Destination { region_handle: 4, endpoint: "127.0.0.1:13000".parse().expect("socket address") };
        let same = Destination { region_handle: 4, endpoint: "127.0.0.1:13000".parse().expect("socket address") };
        let different = Destination { region_handle: 5, endpoint: "127.0.0.1:13000".parse().expect("socket address") };
        assert!(same_destination(left, same));
        assert!(!same_destination(left, different));
    }

    #[test]
    fn crossing_during_a_started_teleport_reports_network_conflict_and_resolves_crossing() {
        let crossing = Destination { region_handle: 10, endpoint: "127.0.0.1:13010".parse().expect("crossing address") };
        let teleport = Destination { region_handle: 11, endpoint: "127.0.0.1:13011".parse().expect("teleport address") };
        let mut controller = TeleportController::new();
        controller.on_start(&TeleportStart { teleport_flags: 0 }, Some(1));
        assert!(!controller.on_crossed_region(crossing, accepted_outcome()));

        std::assert_matches!(controller.state.borrow().crossing, CrossingState::Pending { region_handle: 10, during_teleport: true });
        assert!(controller.on_finish(teleport, 0, Some(13), conflict_outcome()));
        std::assert_matches!(
            controller.state.borrow().status,
            TeleportStatus::CrossingConflict { crossing_region_handle: 10, teleport_region_handle: 11 }
        );
        assert_eq!(controller.state.borrow().crossing, CrossingState::Conflict { region_handle: 10 });
        assert!(controller.active.is_none());
    }

    #[test]
    fn crossing_conflicts_with_finish_without_a_teleport_start() {
        let crossing = Destination { region_handle: 10, endpoint: "127.0.0.1:13010".parse().expect("crossing address") };
        let teleport = Destination { region_handle: 11, endpoint: "127.0.0.1:13011".parse().expect("teleport address") };
        let mut controller = TeleportController::new();
        assert!(!controller.on_crossed_region(crossing, accepted_outcome()));

        assert!(controller.active.is_none());
        assert!(controller.on_finish(teleport, 0, Some(13), conflict_outcome()));

        std::assert_matches!(
            controller.state.borrow().status,
            TeleportStatus::CrossingConflict { crossing_region_handle: 10, teleport_region_handle: 11 }
        );
        assert_eq!(controller.state.borrow().crossing, CrossingState::Conflict { region_handle: 10 });
        assert!(controller.active.is_none());
        assert!(controller.pending_crossing.is_none());
    }

    #[test]
    fn crossing_after_teleport_start_completes_as_a_conflict_when_finish_names_another_destination() {
        let crossing = Destination { region_handle: 10, endpoint: "127.0.0.1:13010".parse().expect("crossing address") };
        let teleport = Destination { region_handle: 11, endpoint: "127.0.0.1:13011".parse().expect("teleport address") };
        let mut controller = TeleportController::new();
        controller.on_start(&TeleportStart { teleport_flags: 0 }, Some(1));

        assert!(!controller.on_crossed_region(crossing, accepted_outcome()));
        assert_eq!(controller.state.borrow().crossing, CrossingState::Pending { region_handle: 10, during_teleport: true });
        assert!(controller.on_finish(teleport, 0, Some(13), conflict_outcome()));

        std::assert_matches!(
            controller.state.borrow().status,
            TeleportStatus::CrossingConflict { crossing_region_handle: 10, teleport_region_handle: 11 }
        );
        assert_eq!(controller.state.borrow().crossing, CrossingState::Conflict { region_handle: 10 });
        assert!(controller.active.is_none());
        assert!(controller.pending_crossing.is_none());
    }

    #[test]
    fn crossing_promotion_supersedes_unfinished_teleport_and_allows_retry() {
        let crossing = Destination { region_handle: 10, endpoint: "127.0.0.1:13010".parse().expect("crossing address") };
        let mut controller = TeleportController::new();
        controller.on_start(&TeleportStart { teleport_flags: 0 }, Some(1));
        assert!(!controller.on_crossed_region(crossing, accepted_outcome()));

        assert!(!controller.on_simulator_promoted(Some(crossing.region_handle), crossing.endpoint));

        assert_eq!(controller.state.borrow().crossing, CrossingState::Complete { region_handle: crossing.region_handle });
        assert_eq!(controller.state.borrow().status, TeleportStatus::SupersededByCrossing { region_handle: crossing.region_handle });
        assert!(controller.active.is_none());
        assert!(controller.pending_crossing.is_none());
    }

    #[test]
    fn crossing_after_teleport_finish_reports_and_clears_the_same_conflict() {
        let crossing = Destination { region_handle: 10, endpoint: "127.0.0.1:13010".parse().expect("crossing address") };
        let teleport = Destination { region_handle: 11, endpoint: "127.0.0.1:13011".parse().expect("teleport address") };
        let mut controller = TeleportController::new();
        controller.on_start(&TeleportStart { teleport_flags: 0 }, Some(1));
        assert!(!controller.on_finish(teleport, 0, Some(13), accepted_outcome()));

        assert!(controller.on_crossed_region(crossing, conflict_outcome()));

        std::assert_matches!(
            controller.state.borrow().status,
            TeleportStatus::CrossingConflict { crossing_region_handle: 10, teleport_region_handle: 11 }
        );
        assert_eq!(controller.state.borrow().crossing, CrossingState::Conflict { region_handle: 10 });
        assert!(controller.active.is_none());
        assert!(controller.pending_crossing.is_none());
    }

    #[test]
    fn ignored_handoff_notices_do_not_change_the_accepted_destination_or_failure_state() {
        let accepted = Destination { region_handle: 10, endpoint: "127.0.0.1:13010".parse().expect("accepted address") };
        let rejected = Destination { region_handle: 11, endpoint: "127.0.0.1:13011".parse().expect("rejected address") };
        let mut controller = TeleportController::new();
        controller.on_start(&TeleportStart { teleport_flags: 0 }, Some(1));
        assert!(!controller.on_finish(accepted, 5, Some(13), accepted_outcome()));
        let before = controller.state.borrow().clone();

        assert!(!controller.on_finish(rejected, 8, Some(21), HandoffEventOutcome::Ignored));
        controller.on_failed(
            TeleportFailure { reason: TeleportFailureReason::Blocked, server_reason: b"blocked_tport".to_vec(), alert_info: Vec::new() },
            HandoffEventOutcome::Ignored,
        );
        assert!(!controller.on_crossed_region(rejected, HandoffEventOutcome::Ignored));

        assert_eq!(controller.state.borrow().status, before.status);
        assert_eq!(controller.state.borrow().crossing, before.crossing);
        assert_eq!(controller.active.as_ref().and_then(|active| active.destination), Some(accepted));
        assert_eq!(controller.active.as_ref().map(|active| active.flags), Some(5));
        assert_eq!(controller.active.as_ref().and_then(|active| active.sim_access), Some(13));
    }

    #[test]
    fn rejected_crossing_does_not_leave_a_pending_busy_state() {
        let rejected = Destination { region_handle: 11, endpoint: "127.0.0.1:13011".parse().expect("rejected address") };
        let mut controller = TeleportController::new();

        assert!(!controller.on_crossed_region(rejected, HandoffEventOutcome::Ignored));

        assert_eq!(controller.state.borrow().crossing, CrossingState::None);
        assert!(controller.pending_crossing.is_none());
        assert!(controller.active.is_none());
    }

    #[test]
    fn network_crossing_conflict_is_not_published_as_pending() {
        let destination = Destination { region_handle: 11, endpoint: "127.0.0.1:13011".parse().expect("conflicting address") };
        let mut controller = TeleportController::new();

        assert!(controller.on_crossed_region(destination, conflict_outcome()));

        assert_eq!(controller.state.borrow().crossing, CrossingState::Conflict { region_handle: destination.region_handle });
        assert!(controller.pending_crossing.is_none());
        assert!(controller.active.is_none());
        std::assert_matches!(
            controller.state.borrow().status,
            TeleportStatus::HandoffConflict { active_region_handle: None, conflicting_region_handle: 11 }
        );
    }

    #[test]
    fn accepted_return_destination_supplies_its_region_handle_for_reused_simulator_promotion() {
        let destination = Destination { region_handle: 10, endpoint: "127.0.0.1:13010".parse().expect("destination") };
        let mut controller = TeleportController::new();
        controller.on_start(&TeleportStart { teleport_flags: 0 }, Some(20));
        assert!(!controller.on_finish(destination, 0, Some(13), accepted_outcome()));

        assert_eq!(controller.region_handle_for_promotion(destination.endpoint), Some(destination.region_handle));
        assert_eq!(controller.region_handle_for_promotion("127.0.0.1:13011".parse().expect("other endpoint")), None);
    }

    #[tokio::test(start_paused = true)]
    async fn teleport_deadline_keeps_crossing_until_cancellation_finishes() {
        let mut controller = TeleportController::new();
        controller.on_start(&TeleportStart { teleport_flags: 0 }, Some(1));
        let crossing = Destination { region_handle: 2, endpoint: "127.0.0.1:13002".parse().expect("crossing address") };
        assert!(!controller.on_crossed_region(crossing, accepted_outcome()));
        let input = {
            let timeout = controller.next_input();
            tokio::pin!(timeout);
            tokio::time::advance(TELEPORT_TIMEOUT).await;
            timeout.await
        };
        std::assert_matches!(input, TeleportInput::Timeout);
        assert!(controller.expire(Instant::now()));
        assert!(controller.active.is_none());
        assert!(controller.pending_crossing.is_some());
        assert_eq!(
            controller.state.borrow().crossing,
            CrossingState::Pending { region_handle: crossing.region_handle, during_teleport: true }
        );
        assert!(controller.pending_cancellation.is_some());
        assert!(controller.request_is_busy());
        assert!(controller.prepare_cancellation_attempt(Instant::now()));
        controller.finish_cancellation_attempt(Ok(()), Instant::now());
        assert!(controller.request_is_busy());
        controller.finish_cancellation(cancelled_handoff());
        assert!(controller.pending_crossing.is_none());
        assert_eq!(controller.state.borrow().crossing, CrossingState::Failed { region_handle: crossing.region_handle });
        std::assert_matches!(controller.state.borrow().status, TeleportStatus::TimedOut);
        assert!(!controller.expire(Instant::now()));
        assert!(!controller.request_is_busy());
    }

    #[tokio::test(start_paused = true)]
    async fn expired_crossing_cancels_before_a_new_request_is_allowed() {
        let crossing = Destination { region_handle: 2, endpoint: "127.0.0.1:13002".parse().expect("crossing address") };
        let mut controller = TeleportController::new();
        assert!(!controller.on_crossed_region(crossing, accepted_outcome()));

        let input = {
            let timeout = controller.next_input();
            tokio::pin!(timeout);
            tokio::time::advance(TELEPORT_TIMEOUT).await;
            timeout.await
        };
        std::assert_matches!(input, TeleportInput::Timeout);
        assert!(controller.expire(Instant::now()));
        assert_eq!(
            controller.state.borrow().crossing,
            CrossingState::Pending { region_handle: crossing.region_handle, during_teleport: false }
        );
        assert!(controller.pending_crossing.is_some());
        assert!(controller.pending_cancellation.is_some());
        assert_eq!(controller.check_request_available(), Err(TeleportRequestError::Busy));

        assert!(controller.prepare_cancellation_attempt(Instant::now()));
        controller.finish_cancellation_attempt(Err(TeleportCancelFailure::QueueFull), Instant::now());
        assert_eq!(
            controller.state.borrow().crossing,
            CrossingState::Pending { region_handle: crossing.region_handle, during_teleport: false }
        );
        assert_eq!(controller.check_request_available(), Err(TeleportRequestError::Busy));

        let retry = {
            let retry = controller.next_input();
            tokio::pin!(retry);
            tokio::time::advance(CANCEL_RETRY_INTERVAL).await;
            retry.await
        };
        std::assert_matches!(retry, TeleportInput::CancelRetry);
        assert!(controller.prepare_cancellation_attempt(Instant::now()));
        controller.finish_cancellation_attempt(Ok(()), Instant::now());
        assert_eq!(
            controller.state.borrow().crossing,
            CrossingState::Pending { region_handle: crossing.region_handle, during_teleport: false }
        );
        assert_eq!(controller.check_request_available(), Err(TeleportRequestError::Busy));
        controller.finish_cancellation(cancelled_handoff());
        assert_eq!(controller.state.borrow().crossing, CrossingState::Failed { region_handle: crossing.region_handle });
        assert!(controller.pending_crossing.is_none());
        assert!(controller.pending_cancellation.is_none());
        assert_eq!(controller.check_request_available(), Ok(()));
    }

    #[tokio::test(start_paused = true)]
    async fn expired_crossing_failure_releases_terminal_state() {
        let crossing = Destination { region_handle: 2, endpoint: "127.0.0.1:13002".parse().expect("crossing address") };
        let mut controller = TeleportController::new();
        assert!(!controller.on_crossed_region(crossing, accepted_outcome()));

        let timeout = {
            let timeout = controller.next_input();
            tokio::pin!(timeout);
            tokio::time::advance(TELEPORT_TIMEOUT).await;
            timeout.await
        };
        std::assert_matches!(timeout, TeleportInput::Timeout);
        assert!(controller.expire(Instant::now()));

        for attempt in 1..=CANCEL_MAX_ATTEMPTS {
            assert_eq!(
                controller.state.borrow().crossing,
                CrossingState::Pending { region_handle: crossing.region_handle, during_teleport: false }
            );
            assert!(controller.prepare_cancellation_attempt(Instant::now()));
            controller.finish_cancellation_attempt(Err(TeleportCancelFailure::QueueFull), Instant::now());
            if attempt < CANCEL_MAX_ATTEMPTS {
                let retry = {
                    let retry = controller.next_input();
                    tokio::pin!(retry);
                    tokio::time::advance(CANCEL_RETRY_INTERVAL).await;
                    retry.await
                };
                std::assert_matches!(retry, TeleportInput::CancelRetry);
            }
        }

        assert_eq!(
            controller.state.borrow().crossing,
            CrossingState::Pending { region_handle: crossing.region_handle, during_teleport: false }
        );
        assert_eq!(controller.check_request_available(), Err(TeleportRequestError::Busy));
        controller.finish_cancellation(cancelled_handoff());
        std::assert_matches!(
            controller.state.borrow().status,
            TeleportStatus::CancelFailed { outcome: TeleportCancelOutcome::TimedOut, attempts: CANCEL_MAX_ATTEMPTS, .. }
        );
        assert_eq!(controller.state.borrow().crossing, CrossingState::Failed { region_handle: crossing.region_handle });
        assert!(controller.pending_crossing.is_none());
        assert!(controller.pending_cancellation.is_none());
        assert_eq!(controller.check_request_available(), Ok(()));
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_after_accepted_login_crossing_retries_a_full_queue_and_allows_next_teleport() {
        let crossing = Destination { region_handle: 12, endpoint: "127.0.0.1:13012".parse().expect("login simulator") };
        let next = Destination { region_handle: 13, endpoint: "127.0.0.1:13013".parse().expect("next simulator") };
        let mut controller = TeleportController::new();

        assert!(!controller.on_crossed_region(crossing, accepted_outcome()));
        assert_eq!(controller.check_request_available(), Err(TeleportRequestError::Busy));
        controller.begin_cancellation(TeleportCancelOutcome::Requested);
        assert!(controller.pending_crossing.is_some());
        assert_eq!(
            controller.state.borrow().crossing,
            CrossingState::Pending { region_handle: crossing.region_handle, during_teleport: false }
        );
        assert_eq!(controller.check_request_available(), Err(TeleportRequestError::Busy));

        assert!(controller.prepare_cancellation_attempt(Instant::now()));
        controller.finish_cancellation_attempt(Err(TeleportCancelFailure::QueueFull), Instant::now());
        std::assert_matches!(controller.state.borrow().status, TeleportStatus::Cancelling { attempts: 1, .. });
        let retry = {
            let retry = controller.next_input();
            tokio::pin!(retry);
            tokio::time::advance(CANCEL_RETRY_INTERVAL).await;
            retry.await
        };
        std::assert_matches!(retry, TeleportInput::CancelRetry);
        assert!(controller.prepare_cancellation_attempt(Instant::now()));
        let generation = controller.pending_cancellation.as_ref().expect("cancellation pending").generation;
        controller
            .cancel_result_sender
            .send(TeleportCancelResult { generation, result: Ok(()) })
            .await
            .expect("report cancellation send completion");
        let TeleportInput::CancelResult(result) = controller.next_input().await else { panic!("cancel send result expected") };
        controller.on_cancel_result(result);
        assert!(controller.pending_crossing.is_some());
        assert_eq!(controller.check_request_available(), Err(TeleportRequestError::Busy));
        controller.finish_cancellation(cancelled_handoff());
        assert_eq!(controller.state.borrow().status, TeleportStatus::Cancelled);
        assert!(controller.pending_crossing.is_none());
        assert_eq!(controller.state.borrow().crossing, CrossingState::None);
        assert_eq!(controller.check_request_available(), Ok(()));

        controller.on_start(&TeleportStart { teleport_flags: 1 }, Some(1));
        assert!(!controller.on_finish(next, 1, Some(13), accepted_outcome()));
        assert!(controller.on_simulator_promoted(Some(next.region_handle), next.endpoint));
        assert_eq!(
            controller.state.borrow().status,
            TeleportStatus::Completed { region_handle: Some(next.region_handle), same_region: false, flags: 1, sim_access: Some(13) }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn failed_cancellation_is_bounded_and_leaves_the_controller_available() {
        let next = Destination { region_handle: 13, endpoint: "127.0.0.1:13013".parse().expect("next simulator") };
        let mut controller = TeleportController::new();

        controller.on_start(&TeleportStart { teleport_flags: 0 }, Some(1));
        controller.begin_cancellation(TeleportCancelOutcome::Requested);
        for attempt in 1..=CANCEL_MAX_ATTEMPTS {
            assert!(controller.prepare_cancellation_attempt(Instant::now()));
            controller.finish_cancellation_attempt(Err(TeleportCancelFailure::QueueFull), Instant::now());
            if attempt < CANCEL_MAX_ATTEMPTS {
                let retry = {
                    let retry = controller.next_input();
                    tokio::pin!(retry);
                    tokio::time::advance(CANCEL_RETRY_INTERVAL).await;
                    retry.await
                };
                std::assert_matches!(retry, TeleportInput::CancelRetry);
            }
        }

        controller.finish_cancellation(cancelled_handoff());
        assert_eq!(
            controller.state.borrow().status,
            TeleportStatus::CancelFailed {
                outcome: TeleportCancelOutcome::Requested,
                attempts: CANCEL_MAX_ATTEMPTS,
                reason: TeleportCancelFailure::QueueFull,
            }
        );
        assert!(controller.pending_cancellation.is_none());
        assert!(controller.pending_crossing.is_none());
        assert_eq!(controller.check_request_available(), Ok(()));

        controller.on_start(&TeleportStart { teleport_flags: 2 }, Some(1));
        assert!(!controller.on_finish(next, 2, Some(13), accepted_outcome()));
        assert!(controller.on_simulator_promoted(Some(next.region_handle), next.endpoint));
        std::assert_matches!(controller.state.borrow().status, TeleportStatus::Completed { .. });
    }

    #[test]
    fn crossing_complete_is_published_for_retry() {
        let destination = Destination { region_handle: 12, endpoint: "127.0.0.1:13012".parse().expect("crossing address") };
        let mut controller = TeleportController::new();
        assert!(!controller.on_crossed_region(destination, accepted_outcome()));

        assert!(!controller.on_simulator_promoted(Some(destination.region_handle), destination.endpoint));
        assert_eq!(controller.state.borrow().crossing, CrossingState::Complete { region_handle: destination.region_handle });
    }

    #[test]
    fn older_local_and_handoff_notices_cannot_replace_a_newer_completed_teleport() {
        let simulator = SimulatorId::new(1);
        let destination = model_destination(2);
        let old_destination = model_destination(1);
        let mut controller = TeleportController::new();

        assert!(controller.accept_lifecycle_packet(simulator, PacketOrder::Ordered(10)));
        controller.on_start(&TeleportStart { teleport_flags: 0 }, Some(1));
        assert!(controller.accept_lifecycle_packet(simulator, PacketOrder::Ordered(20)));
        assert!(!controller.on_finish(destination, 7, Some(13), HandoffEventOutcome::accepted(Some(handoff_generation(2)))));
        let newer_state = controller.state.borrow().clone();

        assert!(!controller.accept_local_packet(simulator, PacketOrder::Ordered(19)));
        assert_eq!(*controller.state.borrow(), newer_state);
        assert!(controller.on_simulator_promoted(Some(destination.region_handle), destination.endpoint));
        let completed_state = controller.state.borrow().clone();

        assert!(!controller.on_finish(old_destination, 3, Some(13), HandoffEventOutcome::accepted(Some(handoff_generation(1)))));
        controller.on_failed(failure(b"timeout_tport\0".to_vec(), Vec::new()), HandoffEventOutcome::accepted(Some(handoff_generation(1))));
        assert_eq!(*controller.state.borrow(), completed_state);
        assert_eq!(controller.handoff_generation, None);
        assert_eq!(controller.latest_handoff_generation, Some(handoff_generation(2)));
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ModelOrder {
        Packet(u64),
        EventQueue(u64),
    }

    impl ModelOrder {
        fn incoming_cmp(self, current: Self) -> Option<Ordering> {
            match (self, current) {
                (Self::Packet(incoming), Self::Packet(current)) | (Self::EventQueue(incoming), Self::EventQueue(current)) => {
                    Some(incoming.cmp(&current))
                }
                _ => None,
            }
        }
    }

    #[derive(Clone, Copy)]
    struct ModelHandoff {
        generation: HandoffGeneration,
        destination: Destination,
        target: SimulatorId,
        crossed_region: bool,
        finish_order: Option<ModelOrder>,
    }

    struct HandoffModel {
        primary: SimulatorId,
        primary_destination: Destination,
        pending: Option<ModelHandoff>,
        conflicted: bool,
        latest_generation: u64,
    }

    impl HandoffModel {
        fn new() -> Self {
            Self {
                primary: SimulatorId::new(1),
                primary_destination: Destination { region_handle: 19, endpoint: "127.0.0.1:13019".parse().expect("primary endpoint") },
                pending: None,
                conflicted: false,
                latest_generation: 0,
            }
        }

        fn accepts_teleport_request(&self) -> bool {
            self.pending.is_none() && !self.conflicted
        }

        fn generation(&mut self) -> HandoffGeneration {
            self.latest_generation += 1;
            handoff_generation(self.latest_generation)
        }

        fn announce(
            &mut self,
            source: SimulatorId,
            destination: Destination,
            order: Option<ModelOrder>,
            crossed_region: bool,
        ) -> HandoffEventOutcome {
            if source != self.primary || self.conflicted || same_destination(self.primary_destination, destination) {
                return HandoffEventOutcome::Ignored;
            }
            if let Some(current) = self.pending {
                if same_destination(current.destination, destination) {
                    let generation = self.generation();
                    let Some(pending) = self.pending.as_mut() else { unreachable!() };
                    pending.generation = generation;
                    pending.crossed_region |= crossed_region;
                    if let Some(order) = order {
                        pending.finish_order = Some(order);
                    }
                    return HandoffEventOutcome::accepted(Some(generation));
                }
                let replace = if current.crossed_region || crossed_region {
                    None
                } else {
                    order.zip(current.finish_order).and_then(|(incoming, current)| incoming.incoming_cmp(current))
                };
                match replace {
                    Some(Ordering::Greater) => {}
                    Some(Ordering::Less) => return HandoffEventOutcome::Ignored,
                    Some(Ordering::Equal) | None => {
                        self.conflicted = true;
                        return HandoffEventOutcome::conflict(current.generation);
                    }
                }
            }
            let generation = self.generation();
            self.pending = Some(ModelHandoff {
                generation,
                destination,
                target: SimulatorId::new(destination.region_handle + 1_000),
                crossed_region,
                finish_order: order,
            });
            self.conflicted = false;
            HandoffEventOutcome::accepted(Some(generation))
        }

        fn fail(&mut self, source: SimulatorId, order: ModelOrder) -> HandoffEventOutcome {
            if source != self.primary || self.conflicted {
                return HandoffEventOutcome::Ignored;
            }
            let Some(current) = self.pending else { return HandoffEventOutcome::accepted(None) };
            if current.crossed_region || current.finish_order.and_then(|finish| order.incoming_cmp(finish)) != Some(Ordering::Greater) {
                return HandoffEventOutcome::Ignored;
            }
            self.pending = None;
            HandoffEventOutcome::accepted(Some(current.generation))
        }

        fn cancel(&mut self, generation: Option<HandoffGeneration>) -> HandoffCancellationOutcome {
            let Some(current) = self.pending else {
                self.conflicted = false;
                return HandoffCancellationOutcome::Cancelled;
            };
            if generation != Some(current.generation) {
                return HandoffCancellationOutcome::Stale;
            }
            self.pending = None;
            self.conflicted = false;
            HandoffCancellationOutcome::Cancelled
        }

        fn disconnect(&mut self, simulator: SimulatorId) {
            if self.pending.is_some_and(|pending| pending.target == simulator) {
                self.pending = None;
                self.conflicted = false;
            }
        }

        fn promote(&mut self) -> Option<Destination> {
            if self.conflicted {
                return None;
            }
            let pending = self.pending.take()?;
            self.primary = pending.target;
            self.primary_destination = pending.destination;
            Some(pending.destination)
        }
    }

    #[derive(Clone, Copy)]
    enum ModelNotice {
        Start { source: SimulatorId, order: ModelOrder, flags: u32 },
        Local { source: SimulatorId, order: ModelOrder, flags: u32 },
        Finish { source: SimulatorId, order: ModelOrder, destination: Destination, outcome: HandoffEventOutcome },
        Failed { source: SimulatorId, order: ModelOrder, outcome: HandoffEventOutcome },
        Crossed { source: SimulatorId, order: ModelOrder, destination: Destination, outcome: HandoffEventOutcome },
    }

    #[derive(Clone, Debug, PartialEq)]
    struct ModelViewerState {
        status: TeleportStatus,
        crossing: CrossingState,
        active_destination: Option<Destination>,
        handoff_generation: Option<HandoffGeneration>,
        latest_handoff_generation: Option<HandoffGeneration>,
        awaiting_handoff_event: bool,
    }

    fn model_destination(index: u8) -> Destination {
        let index = index % 4;
        Destination {
            region_handle: 20 + u64::from(index),
            endpoint: format!("127.0.0.1:{}", 13_020 + u16::from(index)).parse().expect("destination endpoint"),
        }
    }

    fn model_order_is_accepted(controller: &mut TeleportController, simulator: SimulatorId, order: ModelOrder) -> bool {
        match order {
            ModelOrder::Packet(sequence) => controller.accept_lifecycle_packet(simulator, PacketOrder::Ordered(sequence)),
            ModelOrder::EventQueue(batch) => controller.accept_capability_order(simulator, 1, batch, 0),
        }
    }

    fn model_viewer_state(controller: &TeleportController) -> ModelViewerState {
        ModelViewerState {
            status: controller.status.clone(),
            crossing: controller.crossing.clone(),
            active_destination: controller.active.as_ref().and_then(|active| active.destination),
            handoff_generation: controller
                .handoff_generation
                .or_else(|| controller.pending_cancellation.as_ref().and_then(|cancel| cancel.handoff_generation)),
            latest_handoff_generation: controller.latest_handoff_generation,
            awaiting_handoff_event: controller.awaiting_handoff_event,
        }
    }

    fn model_begin_cancellation(controller: &mut TeleportController) {
        if controller.pending_cancellation.is_none()
            && let Some(outcome) = controller.cancellation_outcome()
        {
            controller.begin_cancellation(outcome);
            controller.prepare_cancellation_attempt(Instant::now());
        }
    }

    fn model_finish_cancellation(controller: &mut TeleportController, handoffs: &mut HandoffModel) {
        if !controller.handoff_cancellation_ready() {
            return;
        }
        let outcome = handoffs.cancel(controller.cancellation_handoff_generation());
        controller.finish_cancellation(Ok(outcome));
    }

    fn model_retry_or_finish_cancellation(
        controller: &mut TeleportController,
        handoffs: &mut HandoffModel,
        result: Result<(), TeleportCancelFailure>,
    ) {
        if controller.pending_cancellation.as_ref().is_some_and(|cancel| !cancel.in_flight && cancel.final_result.is_none()) {
            controller.prepare_cancellation_attempt(Instant::now() + CANCEL_RETRY_INTERVAL);
        }
        if controller.pending_cancellation.as_ref().is_some_and(|cancel| cancel.in_flight) {
            controller.finish_cancellation_attempt(result, Instant::now());
            model_finish_cancellation(controller, handoffs);
        }
    }

    fn model_deliver(controller: &mut TeleportController, handoffs: &mut HandoffModel, notices: &mut Vec<ModelNotice>, index: usize) {
        let notice = notices.remove(index);
        let before = model_viewer_state(controller);
        let mut stale = false;
        match notice {
            ModelNotice::Start { source, order, flags } => {
                if source == handoffs.primary && model_order_is_accepted(controller, source, order) {
                    controller.on_start(&TeleportStart { teleport_flags: flags }, Some(handoffs.primary_destination.region_handle));
                } else {
                    stale = true;
                }
            }
            ModelNotice::Local { source, order, flags } => {
                if source == handoffs.primary && model_order_is_accepted(controller, source, order) {
                    controller.on_local(
                        &TeleportLocal {
                            agent_id: Uuid::from_u128(1),
                            location_id: 0,
                            position: Vec3::new(1.0, 2.0, 3.0),
                            look_at: Vec3::Y,
                            teleport_flags: flags,
                        },
                        Some(handoffs.primary_destination.region_handle),
                        Uuid::from_u128(1),
                    );
                } else {
                    stale = true;
                }
            }
            ModelNotice::Finish { source, order, destination, outcome } => {
                if source != handoffs.primary
                    || !model_order_is_accepted(controller, source, order)
                    || !controller.accept_handoff_outcome(outcome)
                {
                    stale = true;
                } else if controller.on_finish(destination, 1, Some(13), outcome) {
                    model_begin_cancellation(controller);
                }
            }
            ModelNotice::Failed { source, order, outcome } => {
                if source != handoffs.primary
                    || !model_order_is_accepted(controller, source, order)
                    || !controller.accept_handoff_outcome(outcome)
                {
                    stale = true;
                } else {
                    controller.on_failed(failure(b"timeout_tport\0".to_vec(), Vec::new()), outcome);
                }
            }
            ModelNotice::Crossed { source, order, destination, outcome } => {
                if source != handoffs.primary
                    || !model_order_is_accepted(controller, source, order)
                    || !controller.accept_handoff_outcome(outcome)
                {
                    stale = true;
                } else if controller.on_crossed_region(destination, outcome) {
                    model_begin_cancellation(controller);
                }
            }
        }
        if stale {
            assert_eq!(model_viewer_state(controller), before, "a stale notice changed viewer teleport state");
        }
    }

    fn model_invariants(controller: &TeleportController, handoffs: &HandoffModel, notices: &[ModelNotice]) -> Result<(), TestCaseError> {
        let viewer_generation =
            controller.handoff_generation.or_else(|| controller.pending_cancellation.as_ref().and_then(|cancel| cancel.handoff_generation));
        let network_generation = handoffs.pending.map(|pending| pending.generation);
        if viewer_generation != network_generation {
            let reconciliation_pending = !notices.is_empty()
                || controller.awaiting_handoff_event
                || controller.pending_cancellation.as_ref().is_some_and(|cancel| cancel.in_flight || cancel.final_result.is_some());
            prop_assert!(reconciliation_pending, "viewer and network handoff generations diverged after operations settled");
        }

        if controller.check_request_available() == Err(TeleportRequestError::Busy) {
            let operation_pending = controller.active.is_some()
                || controller.pending_crossing.is_some()
                || controller.pending_cancellation.is_some()
                || (controller.awaiting_handoff_event && (handoffs.pending.is_some() || !notices.is_empty()));
            prop_assert!(operation_pending, "busy teleport state has no deadline or operation that can resolve it");
        }

        prop_assert!(controller.latest_handoff_generation.is_none_or(|generation| generation.get() <= handoffs.latest_generation));
        if let Some(pending) = handoffs.pending
            && viewer_generation == Some(pending.generation)
            && !handoffs.conflicted
        {
            prop_assert_eq!(controller.handoff_destination, Some(pending.destination));
        }

        if handoffs.pending.is_none()
            && notices.is_empty()
            && controller.pending_cancellation.is_none()
            && !controller.awaiting_handoff_event
            && controller.active.is_none()
            && controller.pending_crossing.is_none()
        {
            prop_assert_eq!(controller.check_request_available(), Ok(()), "a settled teleport controller stayed Busy");
        }
        Ok(())
    }

    #[test]
    fn cancellation_cleanup_for_an_old_generation_keeps_a_refreshed_crossing() {
        let mut controller = TeleportController::new();
        let mut handoffs = HandoffModel::new();
        let destination = model_destination(2);
        let primary = handoffs.primary;
        let first = handoffs.announce(primary, destination, None, true);
        assert!(!controller.on_crossed_region(destination, first));
        let first_generation = first.generation().expect("accepted crossing generation");

        controller.begin_cancellation(TeleportCancelOutcome::Requested);
        assert!(controller.prepare_cancellation_attempt(Instant::now()));
        controller.finish_cancellation_attempt(Ok(()), Instant::now());
        assert!(controller.handoff_cancellation_ready());

        let refreshed = handoffs.announce(primary, destination, None, true);
        assert!(refreshed.generation().is_some_and(|generation| generation.get() > first_generation.get()));
        assert!(!controller.on_crossed_region(destination, refreshed));

        let stale_cleanup = handoffs.cancel(Some(first_generation));
        assert_eq!(stale_cleanup, HandoffCancellationOutcome::Stale);
        controller.finish_cancellation(Ok(stale_cleanup));
        assert_eq!(controller.handoff_generation, refreshed.generation());
        assert_eq!(handoffs.pending.map(|pending| pending.generation), refreshed.generation());
        assert!(controller.pending_cancellation.is_none());
        assert_eq!(controller.check_request_available(), Err(TeleportRequestError::Busy));

        let promoted = handoffs.promote().expect("refreshed crossing is still promotable");
        assert!(!controller.on_simulator_promoted(Some(promoted.region_handle), promoted.endpoint));
        assert_eq!(controller.state.borrow().crossing, CrossingState::Complete { region_handle: promoted.region_handle });
        assert_eq!(controller.check_request_available(), Ok(()));
    }

    fn run_teleport_handoff_model(operations: &[u8]) -> Result<(), TestCaseError> {
        let mut controller = TeleportController::new();
        let mut handoffs = HandoffModel::new();
        let mut notices = Vec::new();

        for (step, raw) in operations.iter().copied().enumerate() {
            let source = if raw & 1 == 0 { handoffs.primary } else { SimulatorId::new(99) };
            let destination = model_destination((raw >> 1) & 3);
            let packet_order = ModelOrder::Packet(1 + u64::from(raw >> 4));
            let event_order = ModelOrder::EventQueue(step as u64 + 1);
            match raw % 14 {
                0 => {
                    if controller.check_request_available().is_ok() && handoffs.accepts_teleport_request() {
                        controller.active = Some(ActiveTeleport {
                            started: false,
                            origin_region_handle: Some(handoffs.primary_destination.region_handle),
                            destination: None,
                            flags: 0,
                            sim_access: None,
                            deadline: Instant::now() + TELEPORT_TIMEOUT,
                        });
                        controller.status = TeleportStatus::Requested;
                        controller.publish();
                    }
                }
                1 => notices.push(ModelNotice::Start { source, order: packet_order, flags: u32::from(raw >> 3) }),
                2 => notices.push(ModelNotice::Local { source, order: packet_order, flags: u32::from(raw >> 3) }),
                3 => {
                    let order = if raw & 0x80 == 0 { packet_order } else { event_order };
                    let outcome = handoffs.announce(source, destination, Some(order), false);
                    notices.push(ModelNotice::Finish { source, order, destination, outcome });
                }
                4 => {
                    let order = if raw & 0x80 == 0 { packet_order } else { event_order };
                    let outcome = handoffs.fail(source, order);
                    notices.push(ModelNotice::Failed { source, order, outcome });
                }
                5 => {
                    let outcome = handoffs.announce(source, destination, None, true);
                    notices.push(ModelNotice::Crossed { source, order: event_order, destination, outcome });
                }
                6 => {
                    let notice_count = notices.len();
                    if notice_count != 0 {
                        model_deliver(&mut controller, &mut handoffs, &mut notices, usize::from(raw) % notice_count);
                    }
                }
                7 => model_begin_cancellation(&mut controller),
                8 => model_retry_or_finish_cancellation(&mut controller, &mut handoffs, Ok(())),
                9 => model_retry_or_finish_cancellation(&mut controller, &mut handoffs, Err(TeleportCancelFailure::QueueFull)),
                10 => model_retry_or_finish_cancellation(&mut controller, &mut handoffs, Err(TeleportCancelFailure::SessionUnavailable)),
                11 => {
                    if let Some(cancel) = controller.pending_cancellation.as_ref() {
                        let now = cancel.deadline + Duration::from_millis(1);
                        let reason = if cancel.in_flight {
                            TeleportCancelFailure::SessionUnavailable
                        } else {
                            cancel.last_error.clone().unwrap_or(TeleportCancelFailure::SessionUnavailable)
                        };
                        controller.finish_cancellation_attempt(Err(reason), now);
                        model_finish_cancellation(&mut controller, &mut handoffs);
                    } else if controller.expire(Instant::now() + TELEPORT_TIMEOUT + Duration::from_secs(1)) {
                        controller.prepare_cancellation_attempt(Instant::now());
                    }
                }
                12 => {
                    let target = handoffs.pending.map_or_else(|| SimulatorId::new(99), |pending| pending.target);
                    let disconnected = if raw & 0x08 == 0 { target } else { SimulatorId::new(99) };
                    let pending_destination =
                        handoffs.pending.filter(|pending| pending.target == disconnected).map(|pending| pending.destination);
                    handoffs.disconnect(disconnected);
                    if let Some(destination) = pending_destination {
                        controller.on_simulator_connect_failed(destination.region_handle, Some(destination.endpoint));
                    }
                }
                13 => {
                    if let Some(destination) = handoffs.promote() {
                        controller.on_simulator_promoted(Some(destination.region_handle), destination.endpoint);
                    }
                }
                _ => unreachable!(),
            }
            model_invariants(&controller, &handoffs, &notices)?;
        }

        while !notices.is_empty() {
            let last_notice = notices.len() - 1;
            model_deliver(&mut controller, &mut handoffs, &mut notices, last_notice);
            model_invariants(&controller, &handoffs, &notices)?;
        }
        model_invariants(&controller, &handoffs, &notices)
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 96, .. ProptestConfig::default() })]

        #[test]
        fn teleport_and_network_handoff_stateful_model(operations in prop::collection::vec(any::<u8>(), 1..96)) {
            run_teleport_handoff_model(&operations)?;
        }
    }
}
