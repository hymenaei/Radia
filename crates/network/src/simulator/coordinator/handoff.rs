use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap, HashSet},
    mem::replace,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    num::NonZeroU64,
    str::from_utf8,
};

use llsd::Value;
use protocol::{lludp::Message, messages::AgentMovementComplete};
use tokio::{
    sync::{mpsc, watch},
    task::{Id, JoinSet},
};
use url::Url;
use uuid::Uuid;

use super::{
    ConnectionRole, CoordinatorError, CoordinatorRunState, HandoffCommand, PreconnectEvents, SimulatorCapabilitySession,
    SimulatorConnectionState, SimulatorCoordinator, SimulatorEvent, SimulatorRequest, record_event_envelope_loss,
    remove_simulator_capability,
};
use crate::{
    capabilities::{Client, EventQueueExit},
    coverage::{CoverageLossLedger, CoverageLossReason},
    event::{EventEnvelope, PacketOrder, SimulatorId},
};

#[derive(Clone, Eq, PartialEq)]
pub(super) struct HandoffTarget {
    pub(super) agent_id: Uuid,
    pub(super) session_id: Uuid,
    pub(super) region_handle: u64,
    pub(super) endpoint: SocketAddr,
    pub(super) seed_capability: Url,
}

#[derive(Clone)]
pub(super) struct PendingPromotion {
    pub(super) source: SimulatorId,
    pub(super) target: SimulatorId,
    pub(super) generation: HandoffGeneration,
    pub(super) handoff: HandoffTarget,
    pub(super) evidence: HandoffEvidence,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct HandoffGeneration(NonZeroU64);

impl HandoffGeneration {
    pub(super) fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub(super) const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy)]
pub(super) struct HandoffCancellation {
    pub(super) source: SimulatorId,
    pub(super) target: SimulatorId,
    generation: HandoffGeneration,
}

#[derive(Default)]
pub(super) enum HandoffState {
    #[default]
    Idle,
    Preparing(PendingPromotion),
    Conflicted(HandoffCancellation),
}

impl HandoffState {
    pub(super) fn pending(&self) -> Option<&PendingPromotion> {
        match self {
            Self::Preparing(pending) => Some(pending),
            Self::Idle | Self::Conflicted(_) => None,
        }
    }

    pub(super) fn pending_mut(&mut self) -> Option<&mut PendingPromotion> {
        match self {
            Self::Preparing(pending) => Some(pending),
            Self::Idle | Self::Conflicted(_) => None,
        }
    }

    pub(super) fn is_conflicted(&self) -> bool {
        matches!(self, Self::Conflicted(_))
    }

    pub(super) fn cancellation(&self) -> Option<HandoffCancellation> {
        match self {
            Self::Preparing(pending) => Some(handoff_cancellation(pending)),
            Self::Conflicted(cancellation) => Some(*cancellation),
            Self::Idle => None,
        }
    }

    pub(super) fn take_pending(&mut self) -> Option<PendingPromotion> {
        match replace(self, Self::Idle) {
            Self::Preparing(pending) => Some(pending),
            state => {
                *self = state;
                None
            }
        }
    }

    pub(super) fn set_pending(&mut self, pending: PendingPromotion) {
        *self = Self::Preparing(pending);
    }

    pub(super) fn conflict(&mut self, pending: &PendingPromotion) {
        *self = Self::Conflicted(handoff_cancellation(pending));
    }

    pub(super) fn cancel_matching(&mut self, cancellation: HandoffCancellation) -> Option<PendingPromotion> {
        if !matches_handoff_cancellation(self, cancellation) {
            return None;
        }
        self.take_pending()
    }

    pub(super) fn clear_conflict(&mut self, cancellation: HandoffCancellation) -> bool {
        let matches = matches!(self, Self::Conflicted(current)
            if current.source == cancellation.source
                && current.target == cancellation.target
                && current.generation == cancellation.generation);
        if matches {
            *self = Self::Idle;
        }
        matches
    }
}

#[derive(Clone, Copy)]
pub(super) enum HandoffAnnouncement {
    CrossedRegion,
    TeleportFinish(HandoffOrder),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HandoffOrder {
    Packet(PacketOrder),
    EventQueue { incarnation: u64, batch: u64, index: usize },
    Unavailable,
}

impl HandoffOrder {
    fn finish_order(self) -> FinishOrder {
        match self {
            Self::Packet(PacketOrder::Ordered(sequence)) => FinishOrder::Packet(sequence),
            Self::EventQueue { incarnation, batch, index } => FinishOrder::EventQueue { incarnation, batch, index },
            Self::Packet(PacketOrder::Ambiguous | PacketOrder::Degraded) | Self::Unavailable => FinishOrder::Unusable,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FinishOrder {
    Packet(u64),
    EventQueue { incarnation: u64, batch: u64, index: usize },
    Unusable,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct HandoffEvidence {
    pub(super) crossed_region: bool,
    pub(super) teleport_finish: Option<FinishOrder>,
}

impl HandoffEvidence {
    fn record(&mut self, announcement: HandoffAnnouncement) {
        match announcement {
            HandoffAnnouncement::CrossedRegion => self.crossed_region = true,
            HandoffAnnouncement::TeleportFinish(order) => {
                let incoming = order.finish_order();
                self.teleport_finish = Some(match (self.teleport_finish, incoming) {
                    (None, incoming) => incoming,
                    (Some(current), incoming) => match compare_finish_order(current, incoming) {
                        Some(Ordering::Greater) => incoming,
                        Some(Ordering::Less | Ordering::Equal) => current,
                        None => FinishOrder::Unusable,
                    },
                });
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HandoffDecision {
    SameDestination,
    ReplaceNewer,
    IgnoreStale,
    Conflict,
}

pub(super) struct PromotionSetup<'a> {
    pub(super) state: &'a mut CoordinatorRunState,
    pub(super) neighbor_requests: mpsc::Sender<SimulatorRequest>,
    pub(super) application_events: mpsc::Sender<EventEnvelope>,
    pub(super) neighbor_events: mpsc::Sender<SimulatorEvent>,
    pub(super) shutdown: watch::Receiver<bool>,
    pub(super) capability_events: mpsc::Sender<EventEnvelope>,
}

impl SimulatorCoordinator {
    fn ensure_handoff_capabilities(
        &mut self,
        simulator_id: SimulatorId,
        handoff: &HandoffTarget,
        capability_events: mpsc::Sender<EventEnvelope>,
        shutdown: watch::Receiver<bool>,
        neighbor_event_queues: &mut JoinSet<(SimulatorId, SocketAddr, EventQueueExit)>,
        neighbor_event_queue_owners: &mut HashMap<Id, (SimulatorId, SocketAddr)>,
        simulator_capabilities: &mut HashMap<SimulatorId, SimulatorCapabilitySession>,
    ) -> Result<Client, CoordinatorError> {
        if let Some(capability) = simulator_capabilities.get(&simulator_id)
            && capability.endpoint == handoff.endpoint
            && capability.seed_capability == handoff.seed_capability
            && capability.event_queue_running
        {
            return Ok(capability.client.clone());
        }
        if let Some(capability) = simulator_capabilities.remove(&simulator_id) {
            capability.client.close_session();
        }
        let client = self.new_capability_client(handoff.seed_capability.clone())?;
        let event_queue =
            match client.start_event_queue(simulator_id, handoff.endpoint, capability_events, shutdown, self.coverage_losses.clone()) {
                Ok(queue) => queue,
                Err(error) => {
                    client.close_session();
                    return Err(error.into());
                }
            };
        let endpoint = handoff.endpoint;
        let event_queue_role = event_queue.role_handle();
        let task = neighbor_event_queues.spawn(async move {
            let exit = event_queue.wait().await;
            (simulator_id, endpoint, exit)
        });
        let task_id = task.id();
        neighbor_event_queue_owners.insert(task_id, (simulator_id, endpoint));
        simulator_capabilities.insert(
            simulator_id,
            SimulatorCapabilitySession {
                endpoint,
                seed_capability: handoff.seed_capability.clone(),
                client: client.clone(),
                event_queue_role,
                event_queue_task: Some(task_id),
                event_queue_running: true,
            },
        );
        Ok(client)
    }

    pub(super) async fn request_handoff(
        &mut self,
        source: SimulatorId,
        handoff: HandoffTarget,
        announcement: HandoffAnnouncement,
        setup: &mut PromotionSetup<'_>,
        state: &mut HandoffState,
    ) -> Result<(), CoordinatorError> {
        if source != self.primary.simulator_id || state.is_conflicted() {
            return Ok(());
        }
        if self.primary.region_handle == Some(handoff.region_handle) && self.primary.endpoint == handoff.endpoint {
            return Ok(());
        }

        let mut evidence = HandoffEvidence::default();
        if let Some(current) = state.pending().cloned() {
            match arbitrate_handoff(&current, &handoff, announcement) {
                HandoffDecision::SameDestination => {
                    let Some(current) = state.pending_mut() else { unreachable!("a cloned pending handoff remains active") };
                    current.evidence.record(announcement);
                    let same_live_target = current.source == source
                        && setup.state.neighbor_connections.get(&current.handoff.region_handle).is_some_and(|connection| {
                            connection.simulator_id == current.target && connection.endpoint == current.handoff.endpoint
                        });
                    if same_live_target {
                        let Some(generation) = self.next_handoff_generation() else {
                            return Ok(());
                        };
                        current.generation = generation;
                        return Ok(());
                    }
                    evidence = current.evidence;
                    let current = state.take_pending().expect("same-destination handoff remains pending");
                    cancel_pending_handoff(
                        current,
                        "destination connection is no longer active",
                        self.simulator_id,
                        &mut setup.state.neighbor_connections,
                        &mut setup.state.retiring_simulators,
                        &mut setup.state.preconnect_events,
                        &mut setup.state.connected_simulators,
                        &mut setup.state.simulator_capabilities,
                        &self.coverage_losses,
                    );
                }
                HandoffDecision::ReplaceNewer => {
                    let current = state.take_pending().expect("replaceable handoff remains pending");
                    cancel_pending_handoff(
                        current,
                        "newer teleport finish received",
                        self.simulator_id,
                        &mut setup.state.neighbor_connections,
                        &mut setup.state.retiring_simulators,
                        &mut setup.state.preconnect_events,
                        &mut setup.state.connected_simulators,
                        &mut setup.state.simulator_capabilities,
                        &self.coverage_losses,
                    );
                }
                HandoffDecision::IgnoreStale => return Ok(()),
                HandoffDecision::Conflict => {
                    state.conflict(&current);
                    cancel_pending_handoff(
                        current,
                        "conflicting handoff announcement",
                        self.simulator_id,
                        &mut setup.state.neighbor_connections,
                        &mut setup.state.retiring_simulators,
                        &mut setup.state.preconnect_events,
                        &mut setup.state.connected_simulators,
                        &mut setup.state.simulator_capabilities,
                        &self.coverage_losses,
                    );
                    tracing::warn!(
                        simulator = self.primary.simulator_id.get(),
                        region = handoff.region_handle,
                        "conflicting simulator handoff announcements; waiting for TeleportCancel"
                    );
                    return Ok(());
                }
            }
        }
        evidence.record(announcement);
        let Some(generation) = self.next_handoff_generation() else {
            return Ok(());
        };

        let source = self.primary.simulator_id;
        let prepared = self.start_neighbor(source, handoff.region_handle, handoff.endpoint, Some(handoff.clone()), setup).await?;
        if !prepared {
            return Ok(());
        }

        let Some(connection) =
            setup.state.neighbor_connections.get(&handoff.region_handle).filter(|connection| connection.endpoint == handoff.endpoint)
        else {
            return Ok(());
        };
        let simulator_id = connection.simulator_id;
        if let Err(_error) = self.ensure_handoff_capabilities(
            simulator_id,
            &handoff,
            setup.capability_events.clone(),
            setup.shutdown.clone(),
            &mut setup.state.neighbor_event_queues,
            &mut setup.state.neighbor_event_queue_owners,
            &mut setup.state.simulator_capabilities,
        ) {
            tracing::warn!(
                simulator = simulator_id.get(),
                region = handoff.region_handle,
                reason = "destination capabilities failed",
                "simulator handoff failed"
            );
            if let Some(connection) = setup.state.neighbor_connections.get_mut(&handoff.region_handle) {
                connection.queue_handoff_command(HandoffCommand::Cancel);
            }
            return Ok(());
        }
        let region_handle = handoff.region_handle;
        state.set_pending(PendingPromotion { source, target: simulator_id, generation, handoff, evidence });
        tracing::info!(simulator = simulator_id.get(), region = region_handle, "simulator promotion started");
        Ok(())
    }

    pub(super) async fn handle_handoff_message(
        &mut self,
        source: SimulatorId,
        message: &Message,
        packet_order: PacketOrder,
        setup: &mut PromotionSetup<'_>,
        state: &mut HandoffState,
    ) -> Result<(), CoordinatorError> {
        if source != self.primary.simulator_id || state.is_conflicted() {
            return Ok(());
        }
        match message {
            Message::TeleportFinish(_) => match decode_teleport_finish(message, self.session.agent_id, self.session.session_id) {
                Ok(handoff) => {
                    self.request_handoff(
                        source,
                        handoff,
                        HandoffAnnouncement::TeleportFinish(HandoffOrder::Packet(packet_order)),
                        setup,
                        state,
                    )
                    .await
                }
                Err(_reason) => Ok(()),
            },
            Message::TeleportFailed(failed) if failed.agent_id == self.session.agent_id => {
                if teleport_failure_matches(state.pending(), HandoffOrder::Packet(packet_order))
                    && let Some(pending) = state.take_pending()
                {
                    cancel_pending_handoff(
                        pending,
                        "teleport failed",
                        self.simulator_id,
                        &mut setup.state.neighbor_connections,
                        &mut setup.state.retiring_simulators,
                        &mut setup.state.preconnect_events,
                        &mut setup.state.connected_simulators,
                        &mut setup.state.simulator_capabilities,
                        &self.coverage_losses,
                    );
                }
                Ok(())
            }
            Message::TeleportFailed(_) => Ok(()),
            _ => Ok(()),
        }
    }
}

pub(super) fn connection_message_has_application_event(
    message: &Message,
    role: ConnectionRole,
    handoff: Option<&HandoffTarget>,
    movement_requested: bool,
) -> bool {
    match message {
        Message::DisableSimulator(_)
        | Message::LogoutReply(_)
        | Message::StartPingCheck(_)
        | Message::CompletePingCheck(_)
        | Message::PacketAck(_) => false,
        Message::AgentMovementComplete(message) => match role {
            ConnectionRole::Primary => true,
            ConnectionRole::Neighbor => handoff.is_some_and(|handoff| {
                movement_requested
                    && message.agent_data.agent_id == handoff.agent_id
                    && message.agent_data.session_id == handoff.session_id
                    && message.data.region_handle == handoff.region_handle
            }),
        },
        _ => true,
    }
}

pub(super) fn handoff_cancellation(pending: &PendingPromotion) -> HandoffCancellation {
    HandoffCancellation { source: pending.source, target: pending.target, generation: pending.generation }
}

pub(super) fn matches_handoff_cancellation(state: &HandoffState, cancellation: HandoffCancellation) -> bool {
    state.cancellation().is_some_and(|current| {
        current.source == cancellation.source && current.target == cancellation.target && current.generation == cancellation.generation
    })
}

fn compare_finish_order(current: FinishOrder, incoming: FinishOrder) -> Option<Ordering> {
    match (current, incoming) {
        (FinishOrder::Packet(current), FinishOrder::Packet(incoming)) => Some(incoming.cmp(&current)),
        (
            FinishOrder::EventQueue { incarnation: current_incarnation, batch: current_batch, index: current_index },
            FinishOrder::EventQueue { incarnation: incoming_incarnation, batch: incoming_batch, index: incoming_index },
        ) if current_incarnation == incoming_incarnation => Some((incoming_batch, incoming_index).cmp(&(current_batch, current_index))),
        _ => None,
    }
}

pub(super) fn same_handoff_destination(left: &HandoffTarget, right: &HandoffTarget) -> bool {
    left.agent_id == right.agent_id
        && left.session_id == right.session_id
        && left.region_handle == right.region_handle
        && left.endpoint == right.endpoint
}

pub(super) fn arbitrate_handoff(
    current: &PendingPromotion,
    incoming: &HandoffTarget,
    announcement: HandoffAnnouncement,
) -> HandoffDecision {
    if same_handoff_destination(&current.handoff, incoming) {
        return HandoffDecision::SameDestination;
    }
    if current.evidence.crossed_region {
        return HandoffDecision::Conflict;
    }
    let Some(current_order) = current.evidence.teleport_finish else {
        return HandoffDecision::Conflict;
    };
    let HandoffAnnouncement::TeleportFinish(incoming) = announcement else {
        return HandoffDecision::Conflict;
    };
    let incoming_order = incoming.finish_order();
    let Some(order) = compare_finish_order(current_order, incoming_order) else { return HandoffDecision::Conflict };
    match order {
        Ordering::Greater => HandoffDecision::ReplaceNewer,
        Ordering::Less => HandoffDecision::IgnoreStale,
        Ordering::Equal => HandoffDecision::Conflict,
    }
}

pub(super) fn teleport_failure_matches(pending: Option<&PendingPromotion>, order: HandoffOrder) -> bool {
    let Some(pending) = pending else { return false };
    if pending.evidence.crossed_region {
        return false;
    }
    pending.evidence.teleport_finish.is_some_and(|finish| compare_finish_order(finish, order.finish_order()) == Some(Ordering::Greater))
}

pub(super) fn cancel_pending_handoff(
    current: PendingPromotion,
    reason: &'static str,
    root_simulator_id: SimulatorId,
    neighbor_connections: &mut HashMap<u64, SimulatorConnectionState>,
    retiring_simulators: &mut HashSet<SimulatorId>,
    preconnect_events: &mut PreconnectEvents,
    connected_simulators: &mut HashSet<SimulatorId>,
    simulator_capabilities: &mut HashMap<SimulatorId, SimulatorCapabilitySession>,
    coverage_losses: &CoverageLossLedger,
) {
    tracing::info!(simulator = current.target.get(), region = current.handoff.region_handle, reason, "simulator handoff cancelled");
    if current.target == root_simulator_id {
        if let Some(connection) =
            neighbor_connections.get_mut(&current.handoff.region_handle).filter(|connection| connection.simulator_id == current.target)
        {
            connection.queue_handoff_command(HandoffCommand::Cancel);
        }
        return;
    }
    if neighbor_connections.get(&current.handoff.region_handle).is_some_and(|connection| connection.simulator_id == current.target)
        && let Some(connection) = neighbor_connections.remove(&current.handoff.region_handle)
    {
        retiring_simulators.insert(connection.simulator_id);
        for event in preconnect_events.remove(connection.simulator_id) {
            record_event_envelope_loss(coverage_losses, event, CoverageLossReason::EventAdmission);
        }
        if connected_simulators.remove(&connection.simulator_id) && !connection.abort.is_finished() {
            connection.log_disconnected(current.handoff.region_handle);
        }
        remove_simulator_capability(simulator_capabilities, connection.simulator_id);
        connection.abort.abort();
    }
}

pub(super) fn matches_pending_promotion(
    pending: Option<&PendingPromotion>,
    active_primary: SimulatorId,
    simulator_id: SimulatorId,
    movement: &AgentMovementComplete,
) -> bool {
    pending.is_some_and(|pending| {
        pending.source == active_primary
            && pending.target == simulator_id
            && movement.agent_data.agent_id == pending.handoff.agent_id
            && movement.agent_data.session_id == pending.handoff.session_id
            && movement.data.region_handle == pending.handoff.region_handle
    })
}

pub(super) fn decode_crossed_region(body: &Value, agent_id: Uuid, session_id: Uuid) -> Result<HandoffTarget, &'static str> {
    let body = event_fields(body)?;
    let agent = event_block(body, "AgentData")?;
    if !matches!(agent.get("AgentID"), Some(Value::Uuid(value)) if *value == agent_id) {
        return Err("AgentData.AgentID does not match the authenticated agent");
    }
    if !matches!(agent.get("SessionID"), Some(Value::Uuid(value)) if *value == session_id) {
        return Err("AgentData.SessionID does not match the authenticated session");
    }
    let region = event_block(body, "RegionData")?;
    let Some(Value::Binary(raw_handle)) = region.get("RegionHandle") else {
        return Err("RegionData.RegionHandle is not LLSD binary data");
    };
    let raw_handle: [u8; 8] = raw_handle.as_slice().try_into().map_err(|_| "RegionData.RegionHandle is not 8 bytes")?;
    let Some(Value::Binary(raw_ip)) = region.get("SimIP") else {
        return Err("RegionData.SimIP is not LLSD binary data");
    };
    let raw_ip: [u8; 4] = raw_ip.as_slice().try_into().map_err(|_| "RegionData.SimIP is not 4 bytes")?;
    let port = match region.get("SimPort") {
        Some(Value::Integer(port)) => u16::try_from(*port).ok(),
        Some(Value::Binary(port)) => port.as_slice().try_into().ok().map(u16::from_be_bytes),
        _ => None,
    }
    .filter(|port| *port != 0)
    .ok_or("RegionData.SimPort is outside the valid range")?;
    let seed = match region.get("SeedCapability") {
        Some(Value::String(seed)) => seed.as_str(),
        Some(Value::Uri(seed)) => seed.as_str(),
        _ => return Err("RegionData.SeedCapability is not an LLSD URI or string"),
    };
    make_handoff_target(agent_id, session_id, u64::from_be_bytes(raw_handle), SocketAddr::from((Ipv4Addr::from(raw_ip), port)), seed)
}

pub(super) fn decode_event_teleport_finish(body: &Value, agent_id: Uuid, session_id: Uuid) -> Result<HandoffTarget, &'static str> {
    let info = event_block(event_fields(body)?, "Info")?;
    if !matches!(info.get("AgentID"), Some(Value::Uuid(value)) if *value == agent_id) {
        return Err("Info.AgentID does not match the authenticated agent");
    }
    let Some(Value::Binary(raw_handle)) = info.get("RegionHandle") else {
        return Err("Info.RegionHandle is not LLSD binary data");
    };
    let raw_handle: [u8; 8] = raw_handle.as_slice().try_into().map_err(|_| "Info.RegionHandle is not 8 bytes")?;
    let Some(Value::Binary(raw_ip)) = info.get("SimIP") else {
        return Err("Info.SimIP is not LLSD binary data");
    };
    let raw_ip: [u8; 4] = raw_ip.as_slice().try_into().map_err(|_| "Info.SimIP is not 4 bytes")?;
    let port = match info.get("SimPort") {
        Some(Value::Integer(port)) => u16::try_from(*port).ok(),
        Some(Value::Binary(port)) => port.as_slice().try_into().ok().map(u16::from_be_bytes),
        _ => None,
    }
    .filter(|port| *port != 0)
    .ok_or("Info.SimPort is outside the valid range")?;
    let seed = match info.get("SeedCapability") {
        Some(Value::String(seed)) => seed.as_str(),
        Some(Value::Uri(seed)) => seed.as_str(),
        _ => return Err("Info.SeedCapability is not an LLSD URI or string"),
    };
    make_handoff_target(agent_id, session_id, u64::from_be_bytes(raw_handle), SocketAddr::from((Ipv4Addr::from(raw_ip), port)), seed)
}

pub(super) fn decode_event_teleport_failed(body: &Value, agent_id: Uuid) -> Result<(), &'static str> {
    let info = event_block(event_fields(body)?, "Info")?;
    match info.get("AgentID") {
        Some(Value::Uuid(value)) if *value == agent_id => Ok(()),
        Some(Value::Uuid(_)) => Err("Info.AgentID does not match the authenticated agent"),
        _ => Err("Info.AgentID is not an LLSD UUID"),
    }
}

pub(super) fn event_fields(body: &Value) -> Result<&BTreeMap<String, Value>, &'static str> {
    let Value::Map(fields) = body else {
        return Err("event body is not an LLSD map");
    };
    Ok(fields)
}

pub(super) fn event_block<'a>(fields: &'a BTreeMap<String, Value>, name: &str) -> Result<&'a BTreeMap<String, Value>, &'static str> {
    match fields.get(name) {
        Some(Value::Map(block)) => Ok(block),
        Some(Value::Array(blocks)) => match blocks.as_slice() {
            [Value::Map(block)] => Ok(block),
            _ => Err("required LLSD block does not contain exactly one map"),
        },
        _ => Err("required LLSD block is not a map or single-entry array"),
    }
}

pub(super) fn decode_teleport_finish(message: &Message, agent_id: Uuid, session_id: Uuid) -> Result<HandoffTarget, &'static str> {
    let Message::TeleportFinish(finish) = message else {
        return Err("message is not TeleportFinish");
    };
    if finish.agent_id != agent_id {
        return Err("Info.AgentID does not match the authenticated agent");
    }
    let seed = finish.seed_capability.strip_suffix(&[0]).unwrap_or(&finish.seed_capability);
    let seed = from_utf8(seed).map_err(|_| "Info.SeedCapability is not UTF-8")?;
    make_handoff_target(
        agent_id,
        session_id,
        finish.region_handle,
        SocketAddr::from((Ipv4Addr::from(finish.sim_ip), u16::from_be_bytes(finish.sim_port))),
        seed,
    )
}

fn make_handoff_target(
    agent_id: Uuid,
    session_id: Uuid,
    region_handle: u64,
    endpoint: SocketAddr,
    seed: &str,
) -> Result<HandoffTarget, &'static str> {
    if endpoint.port() == 0
        || endpoint.ip().is_unspecified()
        || endpoint.ip().is_multicast()
        || endpoint.ip() == IpAddr::V4(Ipv4Addr::BROADCAST)
    {
        return Err("destination endpoint is invalid");
    }
    let seed_capability = Url::parse(seed).map_err(|_| "SeedCapability is not a valid URL")?;
    if !matches!(seed_capability.scheme(), "http" | "https")
        || seed_capability.host().is_none()
        || !seed_capability.username().is_empty()
        || seed_capability.password().is_some()
        || seed_capability.fragment().is_some()
    {
        return Err("SeedCapability is not a valid HTTP(S) URL");
    }
    Ok(HandoffTarget { agent_id, session_id, region_handle, endpoint, seed_capability })
}

pub(super) fn is_handoff_message(message: &Message) -> bool {
    matches!(message, Message::TeleportFinish(_) | Message::TeleportFailed(_))
}

pub(super) fn is_teleport_cancel(message: &Message, agent_id: Uuid, session_id: Uuid) -> bool {
    matches!(message, Message::TeleportCancel(cancel) if cancel.agent_id == agent_id && cancel.session_id == session_id)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use protocol::login::ResponseData;
    use tokio::{net::UdpSocket, time::timeout};

    use super::*;
    use crate::{Error, simulator::coordinator::tests::login_session};

    #[test]
    fn evidence_retains_the_newest_packet_finish() {
        let mut evidence = HandoffEvidence::default();
        evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::Packet(PacketOrder::Ordered(10))));
        evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::Packet(PacketOrder::Ordered(8))));
        assert_eq!(evidence.teleport_finish, Some(FinishOrder::Packet(10)));

        evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::Packet(PacketOrder::Ordered(12))));
        assert_eq!(evidence.teleport_finish, Some(FinishOrder::Packet(12)));
    }

    #[test]
    fn evidence_retains_the_newest_event_queue_finish() {
        let mut evidence = HandoffEvidence::default();
        evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::EventQueue { incarnation: 4, batch: 7, index: 3 }));
        evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::EventQueue { incarnation: 4, batch: 7, index: 2 }));
        assert_eq!(evidence.teleport_finish, Some(FinishOrder::EventQueue { incarnation: 4, batch: 7, index: 3 }));

        evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::EventQueue { incarnation: 4, batch: 8, index: 0 }));
        assert_eq!(evidence.teleport_finish, Some(FinishOrder::EventQueue { incarnation: 4, batch: 8, index: 0 }));
    }

    #[test]
    fn evidence_discards_ordering_across_sources() {
        let mut evidence = HandoffEvidence::default();
        evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::Packet(PacketOrder::Ordered(10))));
        evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::EventQueue { incarnation: 4, batch: 7, index: 3 }));
        assert_eq!(evidence.teleport_finish, Some(FinishOrder::Unusable));
    }

    #[tokio::test]
    async fn handoff_capability_requests_are_waited_for_after_retirement() {
        let root = UdpSocket::bind("127.0.0.1:0").await.expect("bind root simulator");
        let destination = UdpSocket::bind("127.0.0.1:0").await.expect("bind destination");
        let session = login_session(root.local_addr().expect("root endpoint"));
        let target = HandoffTarget {
            agent_id: session.agent_id,
            session_id: session.session_id,
            region_handle: 42,
            endpoint: destination.local_addr().expect("destination endpoint"),
            seed_capability: session.seed_capability.clone(),
        };
        let mut connection =
            SimulatorCoordinator::connect(session, ResponseData::default(), crate::simulator::coordinator::tests::test_options())
                .await
                .expect("coordinator startup")
                .expect("primary connection starts");
        let id = SimulatorId::allocate().expect("destination ID");
        let (events, _receiver) = mpsc::channel(4);
        let (_shutdown, shutdown) = watch::channel(false);
        let mut queues = JoinSet::new();
        let mut owners = HashMap::new();
        let mut capabilities = HashMap::new();
        let client = connection
            .ensure_handoff_capabilities(id, &target, events, shutdown, &mut queues, &mut owners, &mut capabilities)
            .expect("start destination capability");
        assert_eq!(connection.capability_request_waiters.len(), 2);
        remove_simulator_capability(&mut capabilities, id);
        assert!(capabilities.is_empty());
        timeout(Duration::from_secs(2), connection.finish_capability_request_waiters())
            .await
            .expect("wait for root and retired destination capability requests");
        assert!(connection.capability_request_waiters.is_empty());
        while timeout(Duration::from_secs(2), queues.join_next()).await.expect("join retired EventQueue").is_some() {}
        assert!(matches!(client.request_capabilities(&["TestCapability"]).await, Err(Error::CapabilitySessionClosed)));
    }
}
