use std::{net::SocketAddr, num::NonZeroU64, time::Duration};

use protocol::lludp::Message;
use reqwest::Url;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::{ConnectionError, ConnectionRole};
use crate::{
    circuit::lludp::Delivery,
    error::Error,
    event::{EventEnvelope, SimulatorId},
};

pub(crate) const MAX_NEIGHBOR_SIMULATORS: usize = 32;
pub(crate) const MAX_NEIGHBOR_CONNECTION_TASKS: usize = MAX_NEIGHBOR_SIMULATORS * 2;
pub(crate) const MAX_SIMULATOR_CONNECTIONS: usize = MAX_NEIGHBOR_CONNECTION_TASKS + 1;
pub(crate) const ACK_INTERVAL: Duration = Duration::from_millis(100);
pub(crate) const LOGOUT_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const MAX_PENDING_SIMULATOR_EVENTS: usize = 1024;
pub(crate) const MAX_QUEUED_SIMULATOR_EVENTS: usize = MAX_PENDING_SIMULATOR_EVENTS;
pub(crate) const MAX_PRECONNECT_EVENTS: usize = 1024;

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct HandoffTarget {
    pub(crate) agent_id: Uuid,
    pub(crate) session_id: Uuid,
    pub(crate) region_handle: u64,
    pub(crate) endpoint: SocketAddr,
    pub(crate) seed_capability: Url,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct HandoffGeneration(NonZeroU64);

impl HandoffGeneration {
    pub(crate) fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy)]
pub(crate) struct HandoffCancellation {
    pub(crate) source: SimulatorId,
    pub(crate) target: SimulatorId,
    pub(crate) generation: HandoffGeneration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PrimaryState {
    Neighbor { generation: u64 },
    Connecting,
    Ready { generation: u64 },
}

impl PrimaryState {
    pub(crate) fn role_generation(self) -> u64 {
        match self {
            Self::Neighbor { generation } | Self::Ready { generation } => generation,
            Self::Connecting => 0,
        }
    }

    pub(crate) fn connection_role(self) -> ConnectionRole {
        match self {
            Self::Neighbor { .. } => ConnectionRole::Neighbor,
            Self::Connecting | Self::Ready { .. } => ConnectionRole::Primary,
        }
    }
}

pub(crate) enum SimulatorRequest {
    Enable { simulator_id: SimulatorId, region_handle: u64, endpoint: SocketAddr },
}

#[derive(Clone)]
pub(crate) enum SimulatorTaskOutcome {
    Finished(Result<(), ConnectionError>),
    ConnectFailed(ConnectionError),
}

pub(crate) enum SimulatorEvent {
    Simulator(EventEnvelope),
    SendCompleted {
        simulator_id: SimulatorId,
        cancel_target: Option<HandoffCancellation>,
        error: Option<ConnectionError>,
        report_failure: bool,
    },
    Finished {
        simulator_id: SimulatorId,
        region_handle: Option<u64>,
        endpoint: SocketAddr,
        outcome: SimulatorTaskOutcome,
    },
}

pub(crate) enum SimulatorCommand {
    Send {
        message: Message,
        delivery: Delivery,
        completed: Option<oneshot::Sender<Result<(), Error>>>,
        cancel_target: Option<HandoffCancellation>,
        report_failure: bool,
    },
    PrepareHandoff(HandoffTarget),
    CancelHandoff,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NeighborEventQueueOutcome {
    Queued,
    BacklogFull,
    ReceiverClosed,
}

pub(crate) fn connection_message_has_application_event(
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
