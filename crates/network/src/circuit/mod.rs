pub(crate) mod admission;
mod error;
mod lifecycle;
mod link;
pub(crate) mod lludp;
mod outbound;
pub(crate) mod pacing;
mod receive;
mod task;
mod udp;

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

use crate::{
    AgentSession,
    circuit::{
        lludp::{Circuit, Delivery},
        pacing::{ConnectionPacer, OutboundBudget, OutboundBudgetHandle, TrafficClass},
    },
    coverage::{CoverageLossLedger, CoverageLossReason, CoverageSource},
    error::Error,
    event::{EventEnvelope, SimulatorId},
};
use outbound::{
    MovementSend, OutboundQueue, PendingSend, QueuedMessage, active_queue_class, agent_throttle_message, expire_deferred_retries,
    expire_pending_send, fail_queued_on_close, flush_send_notifications, logout_message, message_class, preempt_deferred_send,
    prepare_next_send, prepare_shutdown_send, queue_internal, queue_send_notification, report_queued_failure, send_prepared,
};

pub(crate) use admission::{ConnectionRole, SimulatorMemoryBudget};
pub(crate) use error::ConnectionError;
pub(crate) use link::{
    ACK_INTERVAL, HandoffCancellation, HandoffGeneration, HandoffTarget, LOGOUT_TIMEOUT, MAX_CONNECTION_PRECONNECT_EVENTS,
    MAX_NEIGHBOR_CONNECTION_TASKS, MAX_NEIGHBOR_SIMULATORS, MAX_PENDING_SIMULATOR_EVENTS, MAX_PRECONNECT_EVENTS,
    MAX_QUEUED_SIMULATOR_EVENTS, MAX_SIMULATOR_CONNECTIONS, NeighborEventQueueOutcome, PrimaryState, SimulatorCommand, SimulatorEvent,
    SimulatorRequest, SimulatorTaskOutcome, connection_message_has_application_event,
};
#[cfg(test)]
pub(crate) use lludp::test_values::NEW_RELIABLE_ACK_CAPACITY;
#[cfg(test)]
pub(crate) use lludp::test_values::with_sequence;
pub(super) use outbound::bound_send_error;
#[cfg(test)]
pub(crate) use pacing::test_values::pacer_rate;
pub(crate) use receive::queue_neighbor_event;
#[cfg(test)]
pub(crate) use udp::test_values::set_send_gate;
pub(crate) use udp::{UdpError, UdpTransport};

const MAX_PENDING_SENDS: usize = 8;
const CONTROL_SEND_RESERVE: usize = 2;
const MAX_NONCONTROL_SENDS: usize = MAX_PENDING_SENDS - CONTROL_SEND_RESERVE;
const MAX_PENDING_SEND_NOTIFICATIONS: usize = MAX_PENDING_SENDS * 2 + 2;
const MAX_SEND_ERROR_BYTES: usize = 256;
const OUTBOUND_SEND_CHARGE_BYTES: usize = 8 * 1024;
const MAX_PENDING_SEND_BYTES: usize = MAX_PENDING_SENDS * OUTBOUND_SEND_CHARGE_BYTES;
const MAX_SEND_AGE: Duration = Duration::from_secs(5);
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
    pub(super) coverage_losses: CoverageLossLedger,
    pub(super) primary_state: Option<watch::Receiver<PrimaryState>>,
    pub(super) budget: OutboundBudgetHandle,
    pub(super) owned_budget: Option<OutboundBudget>,
    pub(super) transport: Option<UdpTransport>,
}

pub(super) struct SimulatorConnection {
    simulator_id: SimulatorId,
    session: AgentSession,
    region_handle: Option<u64>,
    initial_role: ConnectionRole,
    socket: Arc<UdpTransport>,
    circuit: Circuit,
    use_circuit_sequence: u32,
    handoff: Option<HandoffTarget>,
    handoff_role_generation: Option<u64>,
    movement_requested: bool,
    initial_movement_sent: bool,
    primary_state: PrimaryState,
    agent_throttle_sent: bool,
    coverage_losses: CoverageLossLedger,
    pacer: ConnectionPacer,
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

    pub(super) async fn connect(session: AgentSession, options: SimulatorConnectionOptions) -> Result<Self, ConnectionError> {
        let SimulatorConnectionOptions { simulator_id, region_handle, coverage_losses, primary_state, budget, owned_budget, transport } =
            options;
        let is_primary = region_handle.is_none();
        let pacer = budget.register(simulator_id.get(), is_primary).await?;
        let primary_state_receiver = primary_state.unwrap_or_else(|| {
            let initial_state = if is_primary { PrimaryState::Connecting } else { PrimaryState::Neighbor { generation: 0 } };
            watch::channel(initial_state).1
        });
        let socket = match transport {
            Some(transport) => transport,
            None => UdpTransport::connect(session.simulator_address).await?,
        };
        let socket = Arc::new(socket);
        let mut circuit = Circuit::new();
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
    ) -> Result<(), ConnectionError> {
        task::run(self, events, shutdown, neighbor_requests, commands, logout_commands, packet_memory).await
    }
    fn queue_handoff_movement(&mut self, outbound: &mut OutboundQueue, pending_send: Option<&PendingSend>) -> Result<(), ConnectionError> {
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

fn preflight_loss_reason(error: LLUdpError, message_id: MessageId) -> CoverageLossReason {
    match error {
        LLUdpError::DecodedPacketTooLarge | LLUdpError::AllocationBoundOverflow => CoverageLossReason::DecodeLimit,
        LLUdpError::InvalidMessageId if Message::allocation_strategy(message_id) == AllocationStrategy::Specialized => {
            CoverageLossReason::AllocationEstimateMismatch
        }
        _ => CoverageLossReason::PreflightRejected,
    }
}

#[cfg(test)]
pub(crate) mod test_values {
    use std::sync::Arc;

    use super::{HandoffTarget, SimulatorConnection, udp};
    use crate::fixtures::circuit::SendGate;

    pub(crate) fn set_send_gate(connection: &mut SimulatorConnection, send_gate: SendGate) {
        udp::test_values::set_send_gate(Arc::get_mut(&mut connection.socket).expect("test connection owns its socket"), send_gate);
    }

    pub(crate) fn set_handoff(connection: &mut SimulatorConnection, handoff: HandoffTarget) {
        connection.handoff = Some(handoff);
    }
}

#[cfg(test)]
mod tests;
