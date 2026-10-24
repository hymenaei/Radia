use std::{error::Error as StdError, fmt, io, sync::Arc};

use tokio::task::JoinError;

use crate::{
    error::Error,
    transport::{UdpError, lludp::CircuitError, pacing::PacerError},
};

#[derive(Clone, Debug)]
pub(crate) enum ConnectionError {
    Circuit(CircuitError),
    Pacer(PacerError),
    Udp(UdpError),
    Receive(Arc<io::Error>),
    PrimaryTaskFailed(Arc<JoinError>),
    NeighborTaskFailed(Arc<JoinError>),
    ShutdownReason(&'static str),
    ReliableRetryExpired,
    KeepaliveTimeout,
    SendQueueFull,
    NeighborRequestQueueFull,
    SendQueueFullPrioritizing,
    SendNotificationQueueFull,
    SimulatorManagerClosed,
    SendAgeExpired { logout: bool },
    SimulatorConnectionClosed,
    SimulatorNotReady,
    SimulatorCommandQueueFull,
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Circuit(error) => error.fmt(formatter),
            Self::Pacer(error) => error.fmt(formatter),
            Self::Udp(error) => error.fmt(formatter),
            Self::Receive(error) => write!(formatter, "Neighbor simulator connection failed: {error}"),
            Self::PrimaryTaskFailed(error) => write!(formatter, "The primary simulator connection task failed: {error}"),
            Self::NeighborTaskFailed(error) => error.fmt(formatter),
            Self::ShutdownReason(reason) => formatter.write_str(reason),
            Self::ReliableRetryExpired => formatter.write_str("A reliable simulator retransmission exceeded its send age limit."),
            Self::KeepaliveTimeout => formatter.write_str("The simulator stopped responding to LLUDP keepalive traffic."),
            Self::SendQueueFull => formatter.write_str("The simulator send queue is full."),
            Self::NeighborRequestQueueFull => formatter.write_str("The neighbor request queue is full."),
            Self::SendQueueFullPrioritizing => formatter.write_str("The simulator send queue is full while prioritizing traffic."),
            Self::SendNotificationQueueFull => formatter.write_str("The simulator send notification queue is full."),
            Self::SimulatorManagerClosed => formatter.write_str("The simulator manager has closed."),
            Self::SendAgeExpired { logout: true } => formatter.write_str("The simulator logout request exceeded its send age limit."),
            Self::SendAgeExpired { logout: false } => formatter.write_str("A simulator datagram exceeded its queue age limit."),
            Self::SimulatorConnectionClosed => formatter.write_str("The simulator connection has closed."),
            Self::SimulatorNotReady => formatter.write_str("The target simulator is not ready to receive messages."),
            Self::SimulatorCommandQueueFull => formatter.write_str("The target simulator's send queue is full."),
        }
    }
}

impl StdError for ConnectionError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Circuit(error) => Some(error),
            Self::Pacer(error) => Some(error),
            Self::Udp(error) => Some(error),
            Self::Receive(error) => Some(error.as_ref()),
            Self::PrimaryTaskFailed(error) | Self::NeighborTaskFailed(error) => Some(error.as_ref()),
            Self::ShutdownReason(_) => None,
            Self::ReliableRetryExpired
            | Self::KeepaliveTimeout
            | Self::SendQueueFull
            | Self::NeighborRequestQueueFull
            | Self::SendQueueFullPrioritizing
            | Self::SendNotificationQueueFull
            | Self::SimulatorManagerClosed
            | Self::SendAgeExpired { .. }
            | Self::SimulatorConnectionClosed
            | Self::SimulatorNotReady
            | Self::SimulatorCommandQueueFull => None,
        }
    }
}

impl ConnectionError {
    pub(super) fn to_network_error(&self) -> Error {
        match self {
            Self::SendAgeExpired { logout: true } => Error::SimulatorLogoutSendAgeExpired,
            Self::SimulatorConnectionClosed => Error::SimulatorConnectionClosed,
            Self::SimulatorNotReady => Error::SimulatorNotReady,
            Self::SimulatorCommandQueueFull => Error::SimulatorCommandQueueFull,
            _ => Error::SimulatorSend(self.to_string()),
        }
    }
}

impl From<CircuitError> for ConnectionError {
    fn from(error: CircuitError) -> Self {
        Self::Circuit(error)
    }
}

impl From<PacerError> for ConnectionError {
    fn from(error: PacerError) -> Self {
        Self::Pacer(error)
    }
}

impl From<UdpError> for ConnectionError {
    fn from(error: UdpError) -> Self {
        Self::Udp(error)
    }
}
