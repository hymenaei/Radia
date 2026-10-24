use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    mem::replace,
    net::SocketAddr,
};

use super::routing::PreconnectEvents;
use super::{
    ConnectionError, HandoffCommand, PrimaryRoute, RunState, RuntimeAction, SessionDispatcher, SessionError, SessionState,
    SimulatorCapabilitySession, SimulatorConnectionState, SimulatorEvent, SimulatorRequest, record_event_envelope_loss,
    remove_simulator_capability,
};
use crate::{
    capabilities::{Client, EventQueueExit},
    circuit::{HandoffCancellation, HandoffGeneration, HandoffTarget},
    coverage::{CoverageLossLedger, CoverageLossReason},
    event::EventRetention,
    event::{Event, EventEnvelope, PacketOrder, SimulatorId},
};
use protocol::{lludp::Message, messages::AgentMovementComplete};
use tokio::{
    sync::{mpsc, watch},
    task::{Id, JoinSet},
};

#[derive(Clone)]
pub(crate) struct PendingPromotion {
    pub(crate) source: SimulatorId,
    pub(crate) target: SimulatorId,
    pub(crate) generation: HandoffGeneration,
    pub(crate) handoff: HandoffTarget,
    pub(crate) evidence: HandoffEvidence,
}

#[derive(Default)]
pub(crate) enum HandoffState {
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
pub(crate) struct HandoffEvidence {
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
    pub(super) state: &'a mut RunState,
    pub(super) neighbor_requests: mpsc::Sender<SimulatorRequest>,
    pub(super) application_events: mpsc::Sender<EventEnvelope>,
    pub(super) neighbor_events: mpsc::Sender<SimulatorEvent>,
    pub(super) shutdown: watch::Receiver<bool>,
    pub(super) capability_events: mpsc::Sender<EventEnvelope>,
}

impl SessionState {
    fn ensure_handoff_capabilities(
        &mut self,
        simulator_id: SimulatorId,
        handoff: &HandoffTarget,
        capability_events: mpsc::Sender<EventEnvelope>,
        shutdown: watch::Receiver<bool>,
        neighbor_event_queues: &mut JoinSet<(SimulatorId, SocketAddr, EventQueueExit)>,
        neighbor_event_queue_owners: &mut HashMap<Id, (SimulatorId, SocketAddr)>,
        simulator_capabilities: &mut HashMap<SimulatorId, SimulatorCapabilitySession>,
    ) -> Result<Client, SessionError> {
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
    ) -> Result<(), SessionError> {
        if source != self.primary.simulator_id || state.is_conflicted() {
            return Ok(());
        }
        if self.primary.region_handle == Some(handoff.region_handle) && self.primary.endpoint == handoff.endpoint {
            return Ok(());
        }

        let mut evidence = HandoffEvidence::default();
        if !self.resolve_pending_handoff(source, &handoff, announcement, setup, state, &mut evidence) {
            return Ok(());
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

    fn resolve_pending_handoff(
        &mut self,
        source: SimulatorId,
        handoff: &HandoffTarget,
        announcement: HandoffAnnouncement,
        setup: &mut PromotionSetup<'_>,
        state: &mut HandoffState,
        evidence: &mut HandoffEvidence,
    ) -> bool {
        let Some(current) = state.pending().cloned() else {
            return true;
        };
        match arbitrate_handoff(&current, handoff, announcement) {
            HandoffDecision::SameDestination => {
                let Some(current) = state.pending_mut() else { unreachable!("a cloned pending handoff remains active") };
                current.evidence.record(announcement);
                let same_live_target = current.source == source
                    && setup.state.neighbor_connections.get(&current.handoff.region_handle).is_some_and(|connection| {
                        connection.simulator_id == current.target && connection.endpoint == current.handoff.endpoint
                    });
                if same_live_target {
                    let Some(generation) = self.next_handoff_generation() else {
                        return false;
                    };
                    current.generation = generation;
                    return false;
                }
                *evidence = current.evidence;
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
            HandoffDecision::IgnoreStale => return false,
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
                return false;
            }
        }
        true
    }

    pub(super) async fn handle_handoff_message(
        &mut self,
        source: SimulatorId,
        message: &Message,
        packet_order: PacketOrder,
        setup: &mut PromotionSetup<'_>,
        state: &mut HandoffState,
    ) -> Result<(), SessionError> {
        if source != self.primary.simulator_id || state.is_conflicted() {
            return Ok(());
        }
        match message {
            Message::TeleportFinish(_) => {
                match super::teleport::decode_teleport_finish(message, self.session.agent_id, self.session.session_id) {
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
                }
            }
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

impl SessionDispatcher<'_> {
    pub(super) fn handle_handoff_command_retry(&mut self) -> RuntimeAction {
        RuntimeAction::Continue
    }

    pub(super) async fn handle_send_completed(
        &mut self,
        simulator_id: SimulatorId,
        cancel_target: Option<HandoffCancellation>,
        error: Option<ConnectionError>,
        report_failure: bool,
    ) -> Result<RuntimeAction, SessionError> {
        if let Some(error) = error {
            if report_failure {
                self.state
                    .publish_event(
                        &self.events,
                        &mut self.run_state,
                        Event::SendFailed(crate::circuit::bound_send_error(error.to_string())),
                    )
                    .await?;
            }
        } else if simulator_id == self.state.primary.simulator_id
            && let Some(cancel) = cancel_target
        {
            if self.handoff_state.clear_conflict(cancel) {
                return Ok(RuntimeAction::Continue);
            }
            if matches_handoff_cancellation(&self.handoff_state, cancel)
                && let Some(pending) = self.handoff_state.cancel_matching(cancel)
            {
                cancel_pending_handoff(
                    pending,
                    "teleport cancelled",
                    self.state.simulator_id,
                    &mut self.run_state.neighbor_connections,
                    &mut self.run_state.retiring_simulators,
                    &mut self.run_state.preconnect_events,
                    &mut self.run_state.connected_simulators,
                    &mut self.run_state.simulator_capabilities,
                    &self.state.coverage_losses,
                );
            }
        }
        Ok(RuntimeAction::Continue)
    }

    pub(super) async fn promote_primary(
        &mut self,
        simulator_id: SimulatorId,
        event: Event,
        retained_bytes: Option<EventRetention>,
        coverage_watermark: Option<crate::coverage::CoverageWatermark>,
    ) -> Result<(), SessionError> {
        let promotion = self.handoff_state.take_pending().expect("matching promotion is pending");
        let Some(connection) = self
            .run_state
            .neighbor_connections
            .get(&promotion.handoff.region_handle)
            .filter(|connection| connection.simulator_id == promotion.target && connection.endpoint == promotion.handoff.endpoint)
        else {
            tracing::info!(
                simulator = promotion.target.get(),
                region = promotion.handoff.region_handle,
                reason = "destination circuit is stale",
                "simulator handoff failed"
            );
            return Ok(());
        };
        let Some(next_primary) = PrimaryRoute::from_neighbor(promotion.handoff.region_handle, connection) else {
            tracing::error!(
                simulator = promotion.target.get(),
                region = promotion.handoff.region_handle,
                reason = "destination primary route is unavailable",
                "simulator handoff failed"
            );
            return Ok(());
        };
        let Some((capabilities, promoted_event_queue)) = self
            .run_state
            .simulator_capabilities
            .get(&promotion.target)
            .filter(|capabilities| capabilities.endpoint == promotion.handoff.endpoint)
            .map(|capabilities| (capabilities.client.clone(), capabilities.event_queue_role.clone()))
        else {
            tracing::warn!(
                simulator = promotion.target.get(),
                region = promotion.handoff.region_handle,
                reason = "destination capabilities are unavailable",
                "simulator handoff failed"
            );
            return Ok(());
        };
        let previous_simulator_id = self.state.primary.simulator_id;
        self.state
            .publish_simulator_event(
                simulator_id,
                EventEnvelope::from_parts(event, retained_bytes, coverage_watermark),
                &self.events,
                &mut self.run_state,
            )
            .await?;
        if self.run_state.logout_deadline.is_some() {
            return Ok(());
        }
        let mut session = self.state.session.clone();
        session.simulator_address = promotion.handoff.endpoint;
        session.seed_capability = promotion.handoff.seed_capability;
        let role_generation = promotion.generation.get();
        self.state.promote_primary(next_primary, role_generation);
        tracing::info!(simulator = promotion.target.get(), region = promotion.handoff.region_handle, "simulator promoted");
        self.active_primary_event_queue.set_primary(false);
        promoted_event_queue.set_primary(true);
        self.active_primary_event_queue = promoted_event_queue;
        self.state
            .publish_event(
                &self.events,
                &mut self.run_state,
                Event::SimulatorPromoted {
                    previous_simulator_id,
                    simulator_id: promotion.target,
                    session: Box::new(session),
                    capabilities,
                },
            )
            .await
    }
}

pub(crate) fn handoff_cancellation(pending: &PendingPromotion) -> HandoffCancellation {
    HandoffCancellation { source: pending.source, target: pending.target, generation: pending.generation }
}

pub(crate) fn matches_handoff_cancellation(state: &HandoffState, cancellation: HandoffCancellation) -> bool {
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

pub(crate) fn matches_pending_promotion(
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

#[cfg(test)]
#[path = "tests/handoff_state.rs"]
mod tests;
