use std::{error::Error as StdError, fmt};

use super::{MAX_PRECONNECT_EVENTS, connection::ConnectionError, logout::LogoutAttemptError};
use crate::{capabilities::SetupError, error::Error};

#[derive(Debug)]
pub(crate) enum CoordinatorError {
    Connection(ConnectionError),
    Capability(SetupError),
    Network(Error),
    Logout(LogoutAttemptError),
    CapabilityClientLimit,
    SimulatorIdSpaceExhausted,
    PrimaryConnectionStoppedDuringStartup,
    SessionStartupTimedOut,
    PreconnectBufferOverflow,
    ShutdownReason(&'static str),
}

impl fmt::Display for CoordinatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(error) => error.fmt(formatter),
            Self::Capability(error) => error.fmt(formatter),
            Self::Network(error) => error.fmt(formatter),
            Self::Logout(error) => error.fmt(formatter),
            Self::CapabilityClientLimit => formatter.write_str("The capability client limit has been reached."),
            Self::SimulatorIdSpaceExhausted => formatter.write_str("Simulator connection ID space is exhausted."),
            Self::PrimaryConnectionStoppedDuringStartup => {
                formatter.write_str("The primary simulator connection task stopped during startup.")
            }
            Self::SessionStartupTimedOut => formatter.write_str("Timed out while establishing the simulator session."),
            Self::PreconnectBufferOverflow => {
                write!(formatter, "A simulator connection exceeded the {MAX_PRECONNECT_EVENTS}-event pre-connect buffer.")
            }
            Self::ShutdownReason(reason) => formatter.write_str(reason),
        }
    }
}

impl StdError for CoordinatorError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Connection(error) => Some(error),
            Self::Capability(error) => Some(error),
            Self::Network(error) => Some(error),
            Self::Logout(error) => Some(error),
            Self::CapabilityClientLimit
            | Self::SimulatorIdSpaceExhausted
            | Self::PrimaryConnectionStoppedDuringStartup
            | Self::SessionStartupTimedOut
            | Self::PreconnectBufferOverflow
            | Self::ShutdownReason(_) => None,
        }
    }
}

impl From<ConnectionError> for CoordinatorError {
    fn from(error: ConnectionError) -> Self {
        Self::Connection(error)
    }
}

impl From<Error> for CoordinatorError {
    fn from(error: Error) -> Self {
        Self::Network(error)
    }
}

impl From<SetupError> for CoordinatorError {
    fn from(error: SetupError) -> Self {
        Self::Capability(error)
    }
}

impl From<LogoutAttemptError> for CoordinatorError {
    fn from(error: LogoutAttemptError) -> Self {
        Self::Logout(error)
    }
}
