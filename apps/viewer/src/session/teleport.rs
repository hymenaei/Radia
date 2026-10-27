use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};

use llsd::Value;
use math::Vec3;
use network::{AgentSession, Error as NetworkError, SessionHandle, TeleportRequest as NetworkTeleportRequest};
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
    Cancelled,
    TimedOut,
    CrossingConflict { crossing_region_handle: u64, teleport_region_handle: u64 },
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

pub(crate) struct TeleportController {
    command_sender: mpsc::Sender<TeleportCommand>,
    commands: mpsc::Receiver<TeleportCommand>,
    state: watch::Sender<TeleportSnapshot>,
    status: TeleportStatus,
    crossing: CrossingState,
    pending_crossing: Option<Destination>,
    active: Option<ActiveTeleport>,
}

impl TeleportController {
    pub(crate) fn new() -> Self {
        let (command_sender, commands) = mpsc::channel(COMMAND_CAPACITY);
        let initial = TeleportSnapshot { status: TeleportStatus::Idle, crossing: CrossingState::None };
        let (state, _) = watch::channel(initial);
        Self {
            command_sender,
            commands,
            state,
            status: TeleportStatus::Idle,
            crossing: CrossingState::None,
            pending_crossing: None,
            active: None,
        }
    }

    pub(crate) fn handle(&self) -> TeleportHandle {
        TeleportHandle { commands: self.command_sender.clone(), state: self.state.subscribe() }
    }

    pub(super) async fn next_input(&mut self) -> TeleportInput {
        if let Some(deadline) = self.active.as_ref().map(|active| active.deadline) {
            tokio::select! {
                command = self.commands.recv() => TeleportInput::Command(command.expect("teleport command channel stays open")),
                _ = tokio::time::sleep_until(deadline) => TeleportInput::Timeout,
            }
        } else {
            TeleportInput::Command(self.commands.recv().await.expect("teleport command channel stays open"))
        }
    }

    pub(super) async fn handle_command(
        &mut self,
        command: TeleportCommand,
        network: &SessionHandle,
        session: Option<&AgentSession>,
        origin_region_handle: Option<u64>,
        crossing_busy: bool,
    ) {
        match command {
            TeleportCommand::Request(request, completed) => {
                if completed.is_closed() {
                    return;
                }
                let result = self.request(request, network, session, origin_region_handle, crossing_busy).await;
                if let Err(error) = completed.send(result) {
                    drop(error);
                }
            }
            TeleportCommand::Cancel(completed) => {
                if completed.is_closed() {
                    return;
                }
                let result = self.cancel(network, session);
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
        crossing_busy: bool,
    ) -> Result<(), TeleportRequestError> {
        if self.active.is_some() || crossing_busy || matches!(self.crossing, CrossingState::Pending { .. }) {
            return Err(TeleportRequestError::Busy);
        }
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

    fn cancel(&mut self, network: &SessionHandle, session: Option<&AgentSession>) -> Result<(), TeleportRequestError> {
        if self.active.is_none() {
            return Err(TeleportRequestError::NoActiveTeleport);
        }
        let session = session.ok_or(TeleportRequestError::SessionUnavailable)?;
        network
            .send(TeleportCancel { agent_id: session.agent_id, session_id: session.session_id }, network::Delivery::Reliable)
            .map_err(map_network_error)?;
        self.active = None;
        self.status = TeleportStatus::Cancelled;
        self.publish();
        Ok(())
    }

    pub(super) fn on_timeout(&mut self, network: &SessionHandle, session: Option<&AgentSession>) {
        if !self.expire() {
            return;
        }
        if let Some(session) = session
            && let Err(error) =
                network.send(TeleportCancel { agent_id: session.agent_id, session_id: session.session_id }, network::Delivery::Reliable)
        {
            tracing::warn!(%error, "teleport timeout cancellation could not be queued");
        }
    }

    fn expire(&mut self) -> bool {
        if self.active.take().is_none() {
            return false;
        }
        self.status = TeleportStatus::TimedOut;
        self.publish();
        true
    }

    pub(super) fn on_start(&mut self, message: &TeleportStart, origin_region_handle: Option<u64>) {
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
        if message.agent_id != agent_id || self.active.is_none() {
            return;
        }
        let stage = progress_kind(&message.message);
        if let Some(active) = &mut self.active {
            active.flags = message.teleport_flags;
        }
        self.status = TeleportStatus::Progress { stage, flags: message.teleport_flags };
        self.publish();
    }

    pub(super) fn on_finish(&mut self, destination: Destination, flags: u32, sim_access: Option<u8>) -> bool {
        if let (Some(_active), Some(crossing), CrossingState::Pending { region_handle, .. }) =
            (&self.active, self.pending_crossing, &self.crossing)
            && !same_destination(crossing, destination)
        {
            let crossing_region_handle = *region_handle;
            self.active = None;
            self.pending_crossing = None;
            self.crossing = CrossingState::Conflict { region_handle: *region_handle };
            self.status = TeleportStatus::CrossingConflict { crossing_region_handle, teleport_region_handle: destination.region_handle };
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

    pub(super) fn on_failed(&mut self, failure: TeleportFailure) {
        if self.active.take().is_some() {
            self.status = TeleportStatus::Failed(failure);
            self.publish();
        }
    }

    pub(super) fn on_local(&mut self, message: &TeleportLocal, current_region_handle: Option<u64>, agent_id: Uuid) -> bool {
        if message.agent_id != agent_id {
            return false;
        }
        let flags = message.teleport_flags;
        self.active = None;
        self.status = TeleportStatus::Completed { region_handle: current_region_handle, same_region: true, flags, sim_access: None };
        self.publish();
        true
    }

    pub(super) fn on_crossed_region(&mut self, destination: Destination) -> bool {
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
        self.pending_crossing = Some(destination);
        self.crossing = CrossingState::Pending { region_handle: destination.region_handle, during_teleport };
        self.publish();
        false
    }

    pub(super) fn on_simulator_connect_failed(&mut self, region_handle: u64, endpoint: Option<SocketAddr>) {
        if matches!(self.crossing, CrossingState::Pending { region_handle: current, .. } if current == region_handle)
            && self.pending_crossing.is_some_and(|pending| endpoint.is_none_or(|endpoint| pending.endpoint == endpoint))
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
        self.publish();
    }

    pub(super) fn on_simulator_promoted(&mut self, region_handle: Option<u64>, endpoint: SocketAddr) -> bool {
        if let (Some(region_handle), Some(crossing), CrossingState::Pending { region_handle: pending, .. }) =
            (region_handle, self.pending_crossing, &self.crossing)
            && *pending == region_handle
            && crossing.endpoint == endpoint
        {
            self.pending_crossing = None;
            self.crossing = CrossingState::Complete { region_handle };
        }
        let completed = self
            .active
            .as_ref()
            .and_then(|active| active.destination)
            .filter(|destination| Some(destination.region_handle) == region_handle && destination.endpoint == endpoint);
        if let Some(destination) = completed {
            let active = self.active.take().expect("active teleport matches promoted destination");
            self.status = TeleportStatus::Completed {
                region_handle: Some(destination.region_handle),
                same_region: active.origin_region_handle == Some(destination.region_handle),
                flags: active.flags,
                sim_access: active.sim_access,
            };
            self.publish();
            true
        } else {
            self.publish();
            false
        }
    }

    pub(super) fn publish(&self) {
        self.state.send_replace(TeleportSnapshot { status: self.status.clone(), crossing: self.crossing.clone() });
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

#[cfg(test)]
mod tests {
    use super::*;
    use llsd::Map;
    use protocol::messages::TeleportStart;

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
            controller.on_failed(failure);
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
        assert!(!controller.on_crossed_region(crossing));

        std::assert_matches!(controller.state.borrow().crossing, CrossingState::Pending { region_handle: 10, during_teleport: true });
        assert!(controller.on_finish(teleport, 0, Some(13)));
        std::assert_matches!(
            controller.state.borrow().status,
            TeleportStatus::CrossingConflict { crossing_region_handle: 10, teleport_region_handle: 11 }
        );
        assert_eq!(controller.state.borrow().crossing, CrossingState::Conflict { region_handle: 10 });
        assert!(controller.active.is_none());
    }

    #[test]
    fn crossing_after_teleport_finish_reports_and_clears_the_same_conflict() {
        let crossing = Destination { region_handle: 10, endpoint: "127.0.0.1:13010".parse().expect("crossing address") };
        let teleport = Destination { region_handle: 11, endpoint: "127.0.0.1:13011".parse().expect("teleport address") };
        let mut controller = TeleportController::new();
        controller.on_start(&TeleportStart { teleport_flags: 0 }, Some(1));
        assert!(!controller.on_finish(teleport, 0, Some(13)));

        assert!(controller.on_crossed_region(crossing));

        std::assert_matches!(
            controller.state.borrow().status,
            TeleportStatus::CrossingConflict { crossing_region_handle: 10, teleport_region_handle: 11 }
        );
        assert_eq!(controller.state.borrow().crossing, CrossingState::Conflict { region_handle: 10 });
        assert!(controller.active.is_none());
        assert!(controller.pending_crossing.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn teleport_deadline_is_bounded_and_clears_the_active_state() {
        let mut controller = TeleportController::new();
        controller.on_start(&TeleportStart { teleport_flags: 0 }, Some(1));
        let input = {
            let timeout = controller.next_input();
            tokio::pin!(timeout);
            tokio::time::advance(TELEPORT_TIMEOUT).await;
            timeout.await
        };
        std::assert_matches!(input, TeleportInput::Timeout);
        assert!(controller.expire());
        std::assert_matches!(controller.state.borrow().status, TeleportStatus::TimedOut);
        assert!(!controller.expire());
    }

    #[test]
    fn crossing_complete_is_published_for_retry() {
        let destination = Destination { region_handle: 12, endpoint: "127.0.0.1:13012".parse().expect("crossing address") };
        let mut controller = TeleportController::new();
        assert!(!controller.on_crossed_region(destination));

        assert!(!controller.on_simulator_promoted(Some(destination.region_handle), destination.endpoint));
        assert_eq!(controller.state.borrow().crossing, CrossingState::Complete { region_handle: destination.region_handle });
    }
}
