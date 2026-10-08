use std::{error::Error as StdError, fmt, io};

/// Errors returned by the public network API.
#[non_exhaustive]
pub enum Error {
    /// The operating system could not start the network worker thread.
    WorkerStart(io::Error),
    CommandQueueFull,
    /// The session-wide or target simulator send queue is full.
    SimulatorCommandQueueFull,
    /// An outbound message has a variable field above the network send limit.
    OutboundMessageTooLarge,
    /// The target simulator exists but has not completed its connection handshake.
    SimulatorNotReady,
    /// The supplied simulator ID does not identify an active connection in this session.
    SimulatorUnavailable,
    /// A local UDP send to a simulator failed.
    SimulatorSend(String),
    /// The authenticator code is empty after trimming whitespace.
    EmptyMfaCode,
    MfaChallengeCompleted,
    SessionClosed,
    /// The login request ended before it accepted an MFA code.
    LoginEnded,
    /// The targeted simulator connection closed before the queued send completed.
    SimulatorConnectionClosed,
    /// The network session stopped before capture startup completed.
    CaptureStartCanceled,
    /// The network session stopped before capture finalization completed.
    CaptureStopCanceled,
    /// The capability request ended before returning a response.
    CapabilityRequestCanceled,
    /// The capability client belongs to a simulator session that has closed.
    CapabilitySessionClosed,
    Capability(String),
    Capture(String),
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkerStart(_) => formatter.debug_tuple("WorkerStart").field(&"[REDACTED]").finish(),
            Self::CommandQueueFull => formatter.write_str("CommandQueueFull"),
            Self::SimulatorCommandQueueFull => formatter.write_str("SimulatorCommandQueueFull"),
            Self::OutboundMessageTooLarge => formatter.write_str("OutboundMessageTooLarge"),
            Self::SimulatorNotReady => formatter.write_str("SimulatorNotReady"),
            Self::SimulatorUnavailable => formatter.write_str("SimulatorUnavailable"),
            Self::SimulatorSend(_) => formatter.debug_tuple("SimulatorSend").field(&"[REDACTED]").finish(),
            Self::EmptyMfaCode => formatter.write_str("EmptyMfaCode"),
            Self::MfaChallengeCompleted => formatter.write_str("MfaChallengeCompleted"),
            Self::SessionClosed => formatter.write_str("SessionClosed"),
            Self::LoginEnded => formatter.write_str("LoginEnded"),
            Self::SimulatorConnectionClosed => formatter.write_str("SimulatorConnectionClosed"),
            Self::CaptureStartCanceled => formatter.write_str("CaptureStartCanceled"),
            Self::CaptureStopCanceled => formatter.write_str("CaptureStopCanceled"),
            Self::CapabilityRequestCanceled => formatter.write_str("CapabilityRequestCanceled"),
            Self::CapabilitySessionClosed => formatter.write_str("CapabilitySessionClosed"),
            Self::Capability(_) => formatter.debug_tuple("Capability").field(&"[REDACTED]").finish(),
            Self::Capture(_) => formatter.debug_tuple("Capture").field(&"[REDACTED]").finish(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkerStart(error) => write!(formatter, "Could not start the network thread: {error}"),
            Self::CommandQueueFull => formatter.write_str("The network command queue is full."),
            Self::SimulatorCommandQueueFull => formatter.write_str("The target simulator's send queue is full."),
            Self::OutboundMessageTooLarge => formatter.write_str("An outbound message exceeds the network send limit."),
            Self::SimulatorNotReady => formatter.write_str("The target simulator is not ready to receive messages."),
            Self::SimulatorUnavailable => formatter.write_str("The target simulator connection is not active in this session."),
            Self::SimulatorSend(message) => formatter.write_str(message),
            Self::EmptyMfaCode => formatter.write_str("Enter a non-empty authenticator code."),
            Self::MfaChallengeCompleted => formatter.write_str("This authenticator challenge has already been completed."),
            Self::SessionClosed => formatter.write_str("The network session has stopped."),
            Self::LoginEnded => formatter.write_str("The login request has ended."),
            Self::SimulatorConnectionClosed => formatter.write_str("The simulator connection has closed."),
            Self::CaptureStartCanceled => formatter.write_str("The network session stopped before capture could start."),
            Self::CaptureStopCanceled => formatter.write_str("The network session stopped before capture could finish."),
            Self::CapabilityRequestCanceled => formatter.write_str("The capability request ended unexpectedly."),
            Self::CapabilitySessionClosed => formatter.write_str("The capability connection has closed."),
            Self::Capability(message) | Self::Capture(message) => formatter.write_str(message),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::WorkerStart(error) => Some(error),
            _ => None,
        }
    }
}
