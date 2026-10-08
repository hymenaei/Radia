use crate::{AgentSession, LoginResponseData, capabilities::Client, client::MfaChallenge};
use llsd::Value;
use protocol::udp::Message;
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
    #[cfg(test)]
    pub(crate) const fn new(generation: u64) -> Self {
        Self(generation)
    }

    pub(crate) fn allocate() -> Option<Self> {
        NEXT_SIMULATOR_ID.try_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1)).ok().map(Self)
    }
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
    /// Message received from a simulator; `sequence` is its LLUDP packet sequence number.
    SimulatorMessage {
        simulator_id: SimulatorId,
        source: SocketAddr,
        sequence: u32,
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
