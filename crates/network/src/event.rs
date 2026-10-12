use crate::{AgentSession, LoginResponseData, capabilities::Client, client::MfaChallenge};
use llsd::Value;
use protocol::lludp::Message;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::sync::{OwnedSemaphorePermit, mpsc, watch};

pub(crate) const CLIENT_EVENT_CAPACITY: usize = 32;
pub(crate) const PRIMARY_TERMINAL_EVENT_CAPACITY: usize = 1;
static NEXT_SIMULATOR_ID: AtomicU64 = AtomicU64::new(1);

/// Identifies one simulator connection within a network session.
///
/// A replacement connection receives a different ID, even when it uses the same endpoint.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SimulatorId(u64);

impl SimulatorId {
    pub const fn get(self) -> u64 {
        self.0
    }

    #[cfg(test)]
    pub(crate) const fn new(generation: u64) -> Self {
        Self(generation)
    }

    pub(crate) fn allocate() -> Option<Self> {
        NEXT_SIMULATOR_ID.try_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1)).ok().map(Self)
    }
}

/// Ordering evidence for an LLUDP packet within one simulator connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketOrder {
    /// The raw sequence maps to this extended sequence within 2^30 packets of the trusted anchor.
    Ordered(u64),
    /// This packet cannot be mapped safely, but the connection's ordering anchor remains usable.
    /// This includes half-range ambiguity and old packets outside the 2^30-packet reorder window.
    Ambiguous,
    /// A forward jump exceeded the supported progression window or the extended counter overflowed.
    /// Later packet arrivals cannot restore ordering authority for this connection.
    Degraded,
}

pub(crate) enum EventSendOutcome {
    Sent,
    ReceiverClosed,
    Shutdown,
}

pub(crate) struct EventEnvelope {
    pub(crate) event: Event,
    retained_capability_bytes: Option<OwnedSemaphorePermit>,
}

impl EventEnvelope {
    pub(crate) fn new(event: Event) -> Self {
        Self { event, retained_capability_bytes: None }
    }

    pub(crate) fn with_capability_bytes(event: Event, permit: OwnedSemaphorePermit) -> Self {
        Self { event, retained_capability_bytes: Some(permit) }
    }

    pub(crate) fn from_parts(event: Event, retained_capability_bytes: Option<OwnedSemaphorePermit>) -> Self {
        Self { event, retained_capability_bytes }
    }

    pub(crate) fn into_parts(self) -> (Event, Option<OwnedSemaphorePermit>) {
        let Self { event, retained_capability_bytes } = self;
        (event, retained_capability_bytes)
    }

    pub(crate) fn into_event(self) -> Event {
        let (event, retained_capability_bytes) = self.into_parts();
        drop(retained_capability_bytes);
        event
    }
}

impl From<Event> for EventEnvelope {
    fn from(event: Event) -> Self {
        Self::new(event)
    }
}

#[cfg(test)]
pub(crate) struct EventReceiver(mpsc::Receiver<EventEnvelope>);

#[cfg(test)]
impl EventReceiver {
    pub(crate) fn new(receiver: mpsc::Receiver<EventEnvelope>) -> Self {
        Self(receiver)
    }

    pub(crate) async fn recv(&mut self) -> Option<Event> {
        self.0.recv().await.map(EventEnvelope::into_event)
    }

    pub(crate) fn try_recv(&mut self) -> Result<Event, mpsc::error::TryRecvError> {
        self.0.try_recv().map(EventEnvelope::into_event)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

pub(crate) async fn send_client_event(
    events: &mpsc::Sender<EventEnvelope>,
    event: Event,
    shutdown: &mut watch::Receiver<bool>,
) -> EventSendOutcome {
    if *shutdown.borrow() {
        return EventSendOutcome::Shutdown;
    }

    tokio::select! {
        biased;
        _ = shutdown.changed() => EventSendOutcome::Shutdown,
        result = events.send(EventEnvelope::new(event)) => match result {
            Ok(()) => EventSendOutcome::Sent,
            Err(_) => EventSendOutcome::ReceiverClosed,
        },
    }
}

pub(crate) async fn send_failure_event(
    events: &mpsc::Sender<EventEnvelope>,
    error: String,
    shutdown: &mut watch::Receiver<bool>,
) -> EventSendOutcome {
    send_client_event(events, Event::Failed(error), shutdown).await
}

/// A login, simulator, capability, capture, or send result from a [`crate::Session`].
/// Capture notifications may be dropped when the bounded event queue is full; explicit stop
/// results come from [`crate::Session::stop_capture`].
pub enum Event {
    MfaRequired {
        challenge: MfaChallenge,
    },
    LoginAccepted {
        mfa_hash: Option<String>,
    },
    /// The primary simulator circuit completed login and its region handshake.
    SessionReady {
        simulator_id: SimulatorId,
        session: Box<AgentSession>,
        capabilities: Client,
        login_data: LoginResponseData,
    },
    CapabilityEvent {
        simulator_id: SimulatorId,
        source: SocketAddr,
        message: String,
        body: Value,
    },
    /// A capability request or poll failed for the identified simulator.
    CapabilityError {
        simulator_id: SimulatorId,
        source: SocketAddr,
        capability: String,
        reason: String,
    },
    /// A simulator circuit completed its circuit handshake. The primary connection has no region handle.
    ///
    /// This does not mean the primary region handshake is complete; `send_to` can still return
    /// [`crate::Error::SimulatorNotReady`] until [`Event::SessionReady`] arrives.
    SimulatorConnected {
        simulator_id: SimulatorId,
        region_handle: Option<u64>,
        endpoint: SocketAddr,
    },
    /// A simulator circuit ended normally. This terminates the connection identified by `simulator_id`.
    SimulatorDisconnected {
        simulator_id: SimulatorId,
        region_handle: Option<u64>,
        endpoint: SocketAddr,
    },
    /// A simulator circuit ended with a network or protocol error.
    SimulatorFailed {
        simulator_id: SimulatorId,
        region_handle: Option<u64>,
        endpoint: SocketAddr,
        reason: String,
    },
    /// A neighbor circuit could not be created, so no live connection was introduced.
    SimulatorConnectFailed {
        simulator_id: SimulatorId,
        region_handle: u64,
        endpoint: SocketAddr,
    },
    SimulatorCapabilities {
        simulator_id: SimulatorId,
        source: SocketAddr,
        capabilities: Client,
    },
    /// The gaining simulator completed the authenticated agent movement handshake.
    ///
    /// Default outbound messages now route to `simulator_id`. The viewer uses the session and
    /// capabilities to update its region-scoped work after applying the explicit world handoff.
    SimulatorPromoted {
        previous_simulator_id: SimulatorId,
        simulator_id: SimulatorId,
        session: Box<AgentSession>,
        capabilities: Client,
    },
    SimulatorMessage {
        simulator_id: SimulatorId,
        source: SocketAddr,
        /// Raw LLUDP packet sequence for protocol diagnostics.
        sequence: u32,
        /// Extended sequence evidence produced by the connection's LLUDP circuit.
        packet_order: PacketOrder,
        message: Message,
    },
    /// Capture recording stopped. `path` is empty if packaging failed.
    /// `error` reports dropped events, writer errors, or packaging errors.
    CaptureFinished {
        path: PathBuf,
        error: Option<String>,
    },
    /// Capture recording stopped after interruption. `path` is empty if packaging failed.
    /// `error` reports writer or packaging failures.
    CaptureInterrupted {
        path: PathBuf,
        error: Option<String>,
    },
    SendFailed(String),
    Failed(String),
}

impl fmt::Debug for Event {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MfaRequired { .. } => formatter.write_str("MfaRequired { challenge: MfaChallenge }"),
            Self::LoginAccepted { mfa_hash } => {
                formatter.debug_struct("LoginAccepted").field("mfa_hash", &mfa_hash.as_ref().map(|_| "[REDACTED]")).finish()
            }
            Self::SessionReady { simulator_id, session, capabilities, login_data } => formatter
                .debug_struct("SessionReady")
                .field("simulator_id", simulator_id)
                .field("session", session)
                .field("capabilities", capabilities)
                .field("login_data", login_data)
                .finish(),
            Self::CapabilityEvent { .. } => formatter
                .debug_struct("CapabilityEvent")
                .field("simulator_id", &"[SESSION LOCAL]")
                .field("source", &"[REDACTED]")
                .field("message", &"[OMITTED]")
                .field("body", &"[OMITTED]")
                .finish(),
            Self::CapabilityError { .. } => formatter
                .debug_struct("CapabilityError")
                .field("simulator_id", &"[SESSION LOCAL]")
                .field("source", &"[REDACTED]")
                .field("capability", &"[REDACTED]")
                .field("reason", &"[REDACTED]")
                .finish(),
            Self::SimulatorConnected { .. } => formatter
                .debug_struct("SimulatorConnected")
                .field("simulator_id", &"[SESSION LOCAL]")
                .field("region_handle", &"[REDACTED]")
                .field("endpoint", &"[REDACTED]")
                .finish(),
            Self::SimulatorDisconnected { .. } => formatter
                .debug_struct("SimulatorDisconnected")
                .field("simulator_id", &"[SESSION LOCAL]")
                .field("region_handle", &"[REDACTED]")
                .field("endpoint", &"[REDACTED]")
                .finish(),
            Self::SimulatorFailed { .. } => formatter
                .debug_struct("SimulatorFailed")
                .field("simulator_id", &"[SESSION LOCAL]")
                .field("region_handle", &"[REDACTED]")
                .field("endpoint", &"[REDACTED]")
                .field("reason", &"[REDACTED]")
                .finish(),
            Self::SimulatorConnectFailed { .. } => formatter
                .debug_struct("SimulatorConnectFailed")
                .field("simulator_id", &"[SESSION LOCAL]")
                .field("region_handle", &"[REDACTED]")
                .field("endpoint", &"[REDACTED]")
                .finish(),
            Self::SimulatorCapabilities { .. } => formatter
                .debug_struct("SimulatorCapabilities")
                .field("simulator_id", &"[SESSION LOCAL]")
                .field("source", &"[REDACTED]")
                .field("capabilities", &"[OMITTED]")
                .finish(),
            Self::SimulatorPromoted { .. } => formatter
                .debug_struct("SimulatorPromoted")
                .field("previous_simulator_id", &"[SESSION LOCAL]")
                .field("simulator_id", &"[SESSION LOCAL]")
                .field("session", &"[REDACTED]")
                .field("capabilities", &"[OMITTED]")
                .finish(),
            Self::SimulatorMessage { sequence, .. } => formatter
                .debug_struct("SimulatorMessage")
                .field("simulator_id", &"[SESSION LOCAL]")
                .field("source", &"[REDACTED]")
                .field("sequence", sequence)
                .field("message", &"[OMITTED]")
                .finish(),
            Self::CaptureFinished { error, .. } => formatter
                .debug_struct("CaptureFinished")
                .field("path", &"[REDACTED]")
                .field("error", &error.as_ref().map(|_| "[REDACTED]"))
                .finish(),
            Self::CaptureInterrupted { error, .. } => formatter
                .debug_struct("CaptureInterrupted")
                .field("path", &"[REDACTED]")
                .field("error", &error.as_ref().map(|_| "[REDACTED]"))
                .finish(),
            Self::SendFailed(_) => formatter.debug_tuple("SendFailed").field(&"[REDACTED]").finish(),
            Self::Failed(_) => formatter.debug_tuple("Failed").field(&"[REDACTED]").finish(),
        }
    }
}

#[cfg(test)]
mod redaction_tests {
    use std::path::PathBuf;

    use llsd::Value;

    use super::{Event, SimulatorId};

    #[test]
    fn event_debug_hides_server_errors_and_endpoints() {
        let event = Event::Failed("server-session-secret".into());
        assert!(!format!("{event:?}").contains("server-session-secret"));

        let event = Event::CapabilityEvent {
            simulator_id: SimulatorId::new(1),
            source: "192.0.2.90:13000".parse().expect("valid endpoint"),
            message: "session-event-secret".into(),
            body: Value::String("event-body-secret".into()),
        };
        let debug = format!("{event:?}");
        assert!(!debug.contains("192.0.2.90"));
        assert!(!debug.contains("session-event-secret"));
        assert!(!debug.contains("event-body-secret"));

        let event = Event::CapabilityError {
            simulator_id: SimulatorId::new(2),
            source: "192.0.2.91:13000".parse().expect("valid endpoint"),
            capability: "private-capability-name".into(),
            reason: "origin-error-secret".into(),
        };
        let debug = format!("{event:?}");
        for secret in ["192.0.2.91", "private-capability-name", "origin-error-secret"] {
            assert!(!debug.contains(secret));
        }
        assert!(matches!(&event, Event::CapabilityError { reason, .. } if reason == "origin-error-secret"));

        let event = Event::SimulatorFailed {
            simulator_id: SimulatorId::new(3),
            region_handle: Some(0x1234),
            endpoint: "192.0.2.92:13000".parse().expect("valid endpoint"),
            reason: "simulator-error-secret".into(),
        };
        let debug = format!("{event:?}");
        for secret in ["192.0.2.92", "4660", "simulator-error-secret"] {
            assert!(!debug.contains(secret));
        }
    }

    #[test]
    fn capture_event_debug_redacts_local_details() {
        let finished = Event::CaptureFinished {
            path: PathBuf::from("C:/users/alice/captures/private-session.vcap"),
            error: Some("could not write private-session-token".into()),
        };
        let interrupted = Event::CaptureInterrupted {
            path: PathBuf::from("C:/users/alice/captures/private-session.vcap"),
            error: Some("could not write private-session-token".into()),
        };

        for debug in [format!("{finished:?}"), format!("{interrupted:?}")] {
            assert!(!debug.contains("alice"));
            assert!(!debug.contains("private-session"));
            assert!(!debug.contains("private-session-token"));
            assert!(debug.contains("[REDACTED]"));
        }
    }
}
