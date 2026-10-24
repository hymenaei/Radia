use std::net::SocketAddr;
use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

use llsd::Value;
use protocol::lludp::Message;
use protocol::login::ResponseData;
#[cfg(test)]
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::{mpsc, watch};

use crate::simulator::EventRetention;
use crate::{AgentSession, capabilities::Client, coverage::CoverageWatermark, session::MfaChallenge};

pub(crate) const CLIENT_EVENT_CAPACITY: usize = 32;
pub(crate) const PRIMARY_TERMINAL_EVENT_CAPACITY: usize = 1;

static NEXT_SIMULATOR_ID: AtomicU64 = AtomicU64::new(1);

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketOrder {
    Ordered(u64),
    Ambiguous,
    Degraded,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EventQueueBatchOrder {
    pub incarnation: u64,
    pub ordinal: u64,
}

pub(crate) enum EventSendOutcome {
    Sent,
    ReceiverClosed,
    Shutdown,
}

pub struct CapabilityEvent {
    pub message: String,
    pub body: Value,
    pub(crate) retained_bytes: usize,
}

impl fmt::Debug for CapabilityEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("CapabilityEvent").field("message", &"[OMITTED]").field("body", &"[OMITTED]").finish()
    }
}

pub(crate) struct EventEnvelope {
    pub(crate) event: Event,
    retained_bytes: Option<EventRetention>,
    coverage_watermark: Option<CoverageWatermark>,
}

impl EventEnvelope {
    pub(crate) fn new(event: Event) -> Self {
        Self { event, retained_bytes: None, coverage_watermark: None }
    }

    #[cfg(test)]
    pub(crate) fn with_retained_bytes(event: Event, permit: OwnedSemaphorePermit) -> Self {
        Self::with_retention(event, EventRetention::from_permits(vec![permit]))
    }

    #[cfg(test)]
    pub(crate) fn with_retention(event: Event, retention: EventRetention) -> Self {
        Self { event, retained_bytes: Some(retention), coverage_watermark: None }
    }

    #[cfg(test)]
    pub(crate) fn with_coverage_watermark(event: Event, coverage_watermark: CoverageWatermark) -> Self {
        Self { event, retained_bytes: None, coverage_watermark: Some(coverage_watermark) }
    }

    pub(crate) fn with_retention_and_coverage(event: Event, retention: EventRetention, coverage_watermark: CoverageWatermark) -> Self {
        Self { event, retained_bytes: Some(retention), coverage_watermark: Some(coverage_watermark) }
    }

    pub(crate) fn from_parts(event: Event, retained_bytes: Option<EventRetention>, coverage_watermark: Option<CoverageWatermark>) -> Self {
        Self { event, retained_bytes, coverage_watermark }
    }

    pub(crate) fn coverage_watermark(&self) -> Option<CoverageWatermark> {
        self.coverage_watermark
    }

    pub(crate) fn into_parts(self) -> (Event, Option<EventRetention>, Option<CoverageWatermark>) {
        let Self { event, retained_bytes, coverage_watermark } = self;
        (event, retained_bytes, coverage_watermark)
    }

    pub(crate) fn into_event(self) -> Event {
        let (event, retained_bytes, _) = self.into_parts();
        drop(retained_bytes);
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

pub enum Event {
    MfaRequired { challenge: MfaChallenge },
    LoginAccepted { mfa_hash: Option<String> },
    SessionReady { simulator_id: SimulatorId, session: Box<AgentSession>, capabilities: Client, login_data: ResponseData },
    CapabilityEvent { simulator_id: SimulatorId, source: SocketAddr, message: String, body: Value },
    CapabilityEventBatch { simulator_id: SimulatorId, source: SocketAddr, order: EventQueueBatchOrder, events: Vec<CapabilityEvent> },
    CapabilityError { simulator_id: SimulatorId, source: SocketAddr, capability: String, reason: String },
    EventQueueCoverageLost { simulator_id: SimulatorId, source: SocketAddr, reason: String },
    CoverageLost { loss: crate::coverage::CoverageLoss },
    SimulatorConnected { simulator_id: SimulatorId, region_handle: Option<u64>, endpoint: SocketAddr },
    SimulatorDisconnected { simulator_id: SimulatorId, region_handle: Option<u64>, endpoint: SocketAddr },
    SimulatorFailed { simulator_id: SimulatorId, region_handle: Option<u64>, endpoint: SocketAddr, reason: String },
    SimulatorConnectFailed { simulator_id: SimulatorId, region_handle: u64, endpoint: SocketAddr },
    SimulatorCapabilities { simulator_id: SimulatorId, source: SocketAddr, capabilities: Client },
    SimulatorPromoted { previous_simulator_id: SimulatorId, simulator_id: SimulatorId, session: Box<AgentSession>, capabilities: Client },
    SimulatorMessage { simulator_id: SimulatorId, source: SocketAddr, sequence: u32, packet_order: PacketOrder, message: Message },
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
            Self::CapabilityEventBatch { events, .. } => formatter
                .debug_struct("CapabilityEventBatch")
                .field("simulator_id", &"[SESSION LOCAL]")
                .field("source", &"[REDACTED]")
                .field("event_count", &events.len())
                .finish(),
            Self::CapabilityError { .. } => formatter
                .debug_struct("CapabilityError")
                .field("simulator_id", &"[SESSION LOCAL]")
                .field("source", &"[REDACTED]")
                .field("capability", &"[REDACTED]")
                .field("reason", &"[REDACTED]")
                .finish(),
            Self::EventQueueCoverageLost { .. } => formatter
                .debug_struct("EventQueueCoverageLost")
                .field("simulator_id", &"[SESSION LOCAL]")
                .field("source", &"[REDACTED]")
                .field("reason", &"[REDACTED]")
                .finish(),
            Self::CoverageLost { loss } => formatter.debug_struct("CoverageLost").field("loss", loss).finish(),
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
            Self::SendFailed(_) => formatter.debug_tuple("SendFailed").field(&"[REDACTED]").finish(),
            Self::Failed(_) => formatter.debug_tuple("Failed").field(&"[REDACTED]").finish(),
        }
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

#[cfg(test)]
mod redaction_tests {

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
}
