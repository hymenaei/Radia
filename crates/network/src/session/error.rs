use std::{error::Error as StdError, fmt};

use super::logout::LogoutAttemptError;
use crate::{capabilities::SetupError, circuit::ConnectionError, error::Error, login::LoginError};

#[derive(Debug)]
pub(crate) enum SessionError {
    Login(LoginError),
    Connection(ConnectionError),
    Capability(SetupError),
    Network(Error),
    Logout(LogoutAttemptError),
    CapabilityClientLimit,
    SimulatorIdSpaceExhausted,
    PrimaryConnectionStoppedDuringStartup,
    SessionStartupTimedOut,
    ApplicationEventBacklogExceeded,
    LifecycleEventBacklogFull,
    SustainedApplicationEventPressure,
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Login(error) => error.fmt(formatter),
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
            Self::ApplicationEventBacklogExceeded => formatter.write_str("The admitted simulator event queue exceeded its capacity."),
            Self::LifecycleEventBacklogFull => formatter.write_str("The lifecycle event queue is full while event delivery is blocked."),
            Self::SustainedApplicationEventPressure => {
                formatter.write_str("Simulator event intake remained blocked beyond the pressure grace period.")
            }
        }
    }
}

impl StdError for SessionError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Login(error) => Some(error),
            Self::Connection(error) => Some(error),
            Self::Capability(error) => Some(error),
            Self::Network(error) => Some(error),
            Self::Logout(error) => Some(error),
            Self::CapabilityClientLimit
            | Self::SimulatorIdSpaceExhausted
            | Self::PrimaryConnectionStoppedDuringStartup
            | Self::SessionStartupTimedOut
            | Self::ApplicationEventBacklogExceeded
            | Self::LifecycleEventBacklogFull
            | Self::SustainedApplicationEventPressure => None,
        }
    }
}

impl From<LoginError> for SessionError {
    fn from(error: LoginError) -> Self {
        Self::Login(error)
    }
}

impl From<ConnectionError> for SessionError {
    fn from(error: ConnectionError) -> Self {
        Self::Connection(error)
    }
}

impl From<Error> for SessionError {
    fn from(error: Error) -> Self {
        Self::Network(error)
    }
}

impl From<SetupError> for SessionError {
    fn from(error: SetupError) -> Self {
        Self::Capability(error)
    }
}

impl From<LogoutAttemptError> for SessionError {
    fn from(error: LogoutAttemptError) -> Self {
        Self::Logout(error)
    }
}
