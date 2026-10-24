use std::{fmt, io};

#[non_exhaustive]
pub enum Error {
    WorkerStart(io::Error),
    CommandQueueFull,
    SimulatorCommandQueueFull,
    OutboundMessageTooLarge,
    SimulatorNotReady,
    SimulatorUnavailable,
    SimulatorSend(String),
    SimulatorLogoutSendAgeExpired,
    EmptyMfaCode,
    MfaChallengeCompleted,
    SessionClosed,
    LoginEnded,
    SimulatorConnectionClosed,
    CapabilityRequestCanceled,
    CapabilitySessionClosed,
    CapabilityHttp(crate::capabilities::HttpError),
    Capability(String),
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
            Self::SimulatorLogoutSendAgeExpired => formatter.write_str("SimulatorLogoutSendAgeExpired"),
            Self::EmptyMfaCode => formatter.write_str("EmptyMfaCode"),
            Self::MfaChallengeCompleted => formatter.write_str("MfaChallengeCompleted"),
            Self::SessionClosed => formatter.write_str("SessionClosed"),
            Self::LoginEnded => formatter.write_str("LoginEnded"),
            Self::SimulatorConnectionClosed => formatter.write_str("SimulatorConnectionClosed"),
            Self::CapabilityRequestCanceled => formatter.write_str("CapabilityRequestCanceled"),
            Self::CapabilitySessionClosed => formatter.write_str("CapabilitySessionClosed"),
            Self::CapabilityHttp(error) => formatter.debug_tuple("CapabilityHttp").field(error).finish(),
            Self::Capability(_) => formatter.debug_tuple("Capability").field(&"[REDACTED]").finish(),
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
            Self::SimulatorLogoutSendAgeExpired => formatter.write_str("The simulator logout request exceeded its send age limit."),
            Self::EmptyMfaCode => formatter.write_str("Enter a non-empty authenticator code."),
            Self::MfaChallengeCompleted => formatter.write_str("This authenticator challenge has already been completed."),
            Self::SessionClosed => formatter.write_str("The network session has stopped."),
            Self::LoginEnded => formatter.write_str("The login request has ended."),
            Self::SimulatorConnectionClosed => formatter.write_str("The simulator connection has closed."),
            Self::CapabilityRequestCanceled => formatter.write_str("The capability request ended unexpectedly."),
            Self::CapabilitySessionClosed => formatter.write_str("The capability connection has closed."),
            Self::CapabilityHttp(error) => write!(formatter, "{error}"),
            Self::Capability(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::WorkerStart(error) => Some(error),
            Self::CapabilityHttp(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod redaction_tests {
    use super::Error;
    use crate::http;

    #[test]
    fn error_debug_redacts_messages_but_display_preserves_them() {
        let error = Error::Capability("capability-secret".into());
        assert!(!format!("{error:?}").contains("capability-secret"));
        assert!(format!("{error}").contains("capability-secret"));

        let error = http::Error::Message("http-response-secret".into());
        assert!(!format!("{error:?}").contains("http-response-secret"));
        assert!(format!("{error}").contains("http-response-secret"));

        let error = Error::CapabilityHttp(http::Error::Message("http-response-secret".into()));
        assert!(!format!("{error:?}").contains("http-response-secret"));
        assert!(format!("{error}").contains("http-response-secret"));
    }
}
