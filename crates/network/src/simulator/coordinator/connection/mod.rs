mod lifecycle;
mod outbound;
mod receive;
mod task;

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};

use protocol::{
    LLUdpError,
    lludp::{AllocationStrategy, Message, MessageId},
    messages::{
        CompleteAgentMovement, ThrottleRates, UseCircuitCode, agent::complete_agent_movement, circuit::use_circuit_code::CircuitCode,
    },
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{self, Instant},
};

use super::{
    ACK_INTERVAL, ConnectionRole, EVENT_BACKLOG_DISCONNECT_REASON, HandoffCancellation, HandoffTarget, LOGOUT_TIMEOUT,
    MAX_PRECONNECT_EVENTS, MAX_QUEUED_SIMULATOR_EVENTS, NEIGHBOR_PRECONNECT_OVERFLOW_REASON, PrimaryState, SimulatorCommand,
    SimulatorEvent, SimulatorMemoryBudget, SimulatorRequest, connection_message_has_application_event, finish_logout, queue_neighbor_event,
    record_coverage_loss, record_event_envelope_loss,
};
use crate::{
    AgentSession,
    coverage::{CoverageLossLedger, CoverageLossReason, CoverageSource},
    error::Error,
    event::{EventEnvelope, SimulatorId},
    status::NetworkStatusHandle,
    transport::{
        UdpTransport,
        lludp::{Circuit, Delivery},
        pacing::{ConnectionPacer, OutboundBudget, OutboundBudgetHandle, TrafficClass},
    },
};
use outbound::*;

const MAX_PENDING_SENDS: usize = 8;
const CONTROL_SEND_RESERVE: usize = 2;
const MAX_NONCONTROL_SENDS: usize = MAX_PENDING_SENDS - CONTROL_SEND_RESERVE;
const MAX_PENDING_SEND_NOTIFICATIONS: usize = MAX_PENDING_SENDS * 2 + 2;
const MAX_SEND_ERROR_BYTES: usize = 256;
const OUTBOUND_SEND_CHARGE_BYTES: usize = 8 * 1024;
const MAX_PENDING_SEND_BYTES: usize = MAX_PENDING_SENDS * OUTBOUND_SEND_CHARGE_BYTES;
const MAX_SEND_AGE: Duration = Duration::from_secs(5);
const SEND_QUEUE_EXPIRED: &str = "A simulator datagram exceeded its queue age limit.";
const RELIABLE_RETRY_EXPIRED: &str = "A reliable simulator retransmission exceeded its send age limit.";
const DEFAULT_AGENT_THROTTLE_RATES: ThrottleRates = ThrottleRates {
    resend_bits_per_second: 460_800.0,
    land_bits_per_second: 317_440.0,
    wind_bits_per_second: 63_488.0,
    cloud_bits_per_second: 63_488.0,
    task_bits_per_second: 1_564_672.0,
    texture_bits_per_second: 1_564_672.0,
    asset_bits_per_second: 573_440.0,
};

pub(super) struct SimulatorConnectionOptions {
    pub(super) simulator_id: SimulatorId,
    pub(super) region_handle: Option<u64>,
    pub(super) status: NetworkStatusHandle,
    pub(super) coverage_losses: CoverageLossLedger,
    pub(super) primary_state: Option<watch::Receiver<PrimaryState>>,
    pub(super) budget: OutboundBudgetHandle,
    pub(super) owned_budget: Option<OutboundBudget>,
    pub(super) transport: Option<UdpTransport>,
}

pub(super) struct SimulatorConnection {
    pub(super) simulator_id: SimulatorId,
    pub(super) session: AgentSession,
    pub(super) region_handle: Option<u64>,
    initial_role: ConnectionRole,
    pub(super) socket: Arc<UdpTransport>,
    pub(super) circuit: Circuit,
    pub(super) use_circuit_sequence: u32,
    pub(super) handoff: Option<HandoffTarget>,
    handoff_role_generation: Option<u64>,
    pub(super) movement_requested: bool,
    pub(super) initial_movement_sent: bool,
    primary_state: PrimaryState,
    agent_throttle_sent: bool,
    pub(super) coverage_losses: CoverageLossLedger,
    pub(super) pacer: ConnectionPacer,
    _owned_budget: Option<OutboundBudget>,
    primary_state_receiver: Option<watch::Receiver<PrimaryState>>,
}

struct ConnectionTask {
    connection: SimulatorConnection,
    events: mpsc::Sender<SimulatorEvent>,
    shutdown: watch::Receiver<bool>,
    neighbor_requests: mpsc::Sender<SimulatorRequest>,
    commands: mpsc::Receiver<SimulatorCommand>,
    logout_commands: Option<mpsc::Receiver<oneshot::Sender<Result<(), Error>>>>,
    packet_memory: SimulatorMemoryBudget,
    phase: lifecycle::Phase,
    preconnect_messages: VecDeque<EventEnvelope>,
    pending_events: VecDeque<EventEnvelope>,
    pending_notifications: VecDeque<SimulatorEvent>,
    outbound: OutboundQueue,
    pending_send: Option<PendingSend>,
    deferred_retries: HashMap<u32, Instant>,
    priority_logout: Option<QueuedMessage>,
    ack_timer: time::Interval,
    ack_timer_due: bool,
    ping_timer: time::Interval,
    last_udp_received: Instant,
    last_send_was_retry: bool,
    event_delivery_turn: bool,
    packet_rejections: receive::PacketRejections,
}

impl Drop for SimulatorConnection {
    fn drop(&mut self) {
        self.coverage_losses.retire(self.simulator_id, CoverageSource::Lludp);
    }
}

impl SimulatorConnection {
    fn current_primary_state(&self) -> PrimaryState {
        self.primary_state_receiver.as_ref().map_or(self.primary_state, |receiver| *receiver.borrow())
    }

    fn clear_handoff(&mut self, outbound: &mut OutboundQueue, pending_send: &mut Option<PendingSend>) {
        self.handoff = None;
        self.handoff_role_generation = None;
        self.movement_requested = false;
        if pending_send
            .as_ref()
            .is_some_and(|pending| pending.queued_message.as_ref().is_some_and(|queued| queued.movement == MovementSend::Handoff))
        {
            *pending_send = None;
        }
        outbound.cancel_handoff_movement();
    }

    fn invalidate_stale_handoff(&mut self, role_generation: u64, outbound: &mut OutboundQueue, pending_send: &mut Option<PendingSend>) {
        if self.handoff_role_generation.is_some_and(|generation| generation != role_generation) {
            self.clear_handoff(outbound, pending_send);
        }
    }

    pub(super) async fn connect(session: AgentSession, options: SimulatorConnectionOptions) -> Result<Self, String> {
        let SimulatorConnectionOptions {
            simulator_id,
            region_handle,
            status,
            coverage_losses,
            primary_state,
            budget,
            owned_budget,
            transport,
        } = options;
        let is_primary = region_handle.is_none();
        let pacer = budget.register(simulator_id.get(), is_primary).await?;
        let primary_state_receiver = primary_state.unwrap_or_else(|| {
            let initial_state = if is_primary { PrimaryState::Connecting } else { PrimaryState::Neighbor { generation: 0 } };
            watch::channel(initial_state).1
        });
        let socket = match transport {
            Some(transport) => transport,
            None => UdpTransport::connect_with_status(session.simulator_address, status.clone()).await?,
        };
        let socket = Arc::new(socket);
        let circuit_status = if is_primary { status.clone() } else { status.for_circuit() };
        let mut circuit = Circuit::with_status(circuit_status);
        let prepared = circuit.prepare_message(
            Message::UseCircuitCode(UseCircuitCode {
                circuit_code: CircuitCode { code: session.circuit_code, session_id: session.session_id, agent_id: session.agent_id },
            }),
            Delivery::Reliable,
        )?;
        let use_circuit_sequence = send_prepared(&socket, &mut circuit, &pacer, prepared, TrafficClass::Control).await?;
        Ok(Self {
            simulator_id,
            session,
            region_handle,
            initial_role: if is_primary { ConnectionRole::Primary } else { ConnectionRole::Neighbor },
            socket,
            circuit,
            use_circuit_sequence,
            handoff: None,
            handoff_role_generation: None,
            movement_requested: false,
            initial_movement_sent: false,
            primary_state: if is_primary { PrimaryState::Connecting } else { PrimaryState::Neighbor { generation: 0 } },
            agent_throttle_sent: false,
            coverage_losses,
            pacer,
            _owned_budget: owned_budget,
            primary_state_receiver: Some(primary_state_receiver),
        })
    }

    #[tracing::instrument(name = "simulator.connection", level = "info", skip_all, fields(simulator = self.simulator_id.get(), region = self.region_handle, role = ?self.initial_role))]
    pub(super) async fn run(
        self,
        events: mpsc::Sender<SimulatorEvent>,
        shutdown: watch::Receiver<bool>,
        neighbor_requests: mpsc::Sender<SimulatorRequest>,
        commands: mpsc::Receiver<SimulatorCommand>,
        logout_commands: mpsc::Receiver<oneshot::Sender<Result<(), Error>>>,
        packet_memory: SimulatorMemoryBudget,
    ) -> Result<(), String> {
        task::run(self, events, shutdown, neighbor_requests, commands, logout_commands, packet_memory).await
    }
    fn queue_handoff_movement(&mut self, outbound: &mut OutboundQueue, pending_send: Option<&PendingSend>) -> Result<(), String> {
        if self.movement_requested
            || self.handoff.is_none()
            || outbound.has_handoff_movement()
            || pending_send
                .is_some_and(|pending| pending.queued_message.as_ref().is_some_and(|queued| queued.movement == MovementSend::Handoff))
        {
            return Ok(());
        }
        queue_internal(
            outbound,
            Message::CompleteAgentMovement(CompleteAgentMovement {
                agent_data: complete_agent_movement::AgentData {
                    agent_id: self.session.agent_id,
                    session_id: self.session.session_id,
                    circuit_code: self.session.circuit_code,
                },
            }),
            Delivery::Reliable,
            TrafficClass::Deadline,
            MovementSend::Handoff,
            active_queue_class(pending_send),
        )
    }
}

#[cfg(test)]
mod tests;

fn preflight_loss_reason(error: LLUdpError, message_id: MessageId) -> CoverageLossReason {
    match error {
        LLUdpError::DecodedPacketTooLarge | LLUdpError::AllocationBoundOverflow => CoverageLossReason::DecodeLimit,
        LLUdpError::InvalidMessageId if Message::allocation_strategy(message_id) == AllocationStrategy::Specialized => {
            CoverageLossReason::AllocationEstimateMismatch
        }
        _ => CoverageLossReason::PreflightRejected,
    }
}
