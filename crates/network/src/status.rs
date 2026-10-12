use std::{
    collections::HashSet,
    fmt,
    net::SocketAddr,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
};

const MAX_STATUS_ERROR_CHARS: usize = 512;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
/// Network session lifecycle.
pub enum NetworkLifecycle {
    #[default]
    Idle,
    LoggingIn,
    /// Login succeeded; the primary simulator is connecting.
    Connecting,
    /// The primary simulator region handshake completed.
    Connected,
    /// Simulator circuits and background work are closing.
    Disconnecting,
    Disconnected,
    /// Login or session processing failed.
    Failed,
}

#[derive(Clone, Default, Eq, PartialEq)]
/// Snapshot of aggregate session counters and primary simulator state.
/// Concurrent updates can occur while a snapshot is read, so fields may reflect different instants.
/// `Debug` redacts the simulator endpoint and last error. `Display` includes both values.
pub struct NetworkStatusSnapshot {
    pub lifecycle: NetworkLifecycle,
    pub simulator_endpoint: Option<SocketAddr>,
    pub circuit_established: bool,
    pub udp_packets_sent: u64,
    pub udp_packets_received: u64,
    pub http_requests: u64,
    pub http_responses: u64,
    pub capability_count: usize,
    pub reliable_packets_pending: usize,
    pub reliable_retransmits: u64,
    pub last_error: Option<String>,
}

impl fmt::Debug for NetworkStatusSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NetworkStatusSnapshot")
            .field("lifecycle", &self.lifecycle)
            .field("simulator_endpoint", &self.simulator_endpoint.map(|_| "[REDACTED]"))
            .field("circuit_established", &self.circuit_established)
            .field("udp_packets_sent", &self.udp_packets_sent)
            .field("udp_packets_received", &self.udp_packets_received)
            .field("http_requests", &self.http_requests)
            .field("http_responses", &self.http_responses)
            .field("capability_count", &self.capability_count)
            .field("reliable_packets_pending", &self.reliable_packets_pending)
            .field("reliable_retransmits", &self.reliable_retransmits)
            .field("last_error", &self.last_error.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

impl fmt::Display for NetworkStatusSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let simulator = self.simulator_endpoint.map_or_else(|| "not assigned".to_owned(), |endpoint| endpoint.to_string());
        let circuit = if self.circuit_established { "established" } else { "not established" };
        writeln!(formatter, "State: {}", self.lifecycle)?;
        writeln!(formatter, "Simulator: {simulator} (circuit {circuit})")?;
        writeln!(
            formatter,
            "UDP: {} sent, {} received; HTTP: {} requests, {} responses",
            self.udp_packets_sent, self.udp_packets_received, self.http_requests, self.http_responses
        )?;
        writeln!(formatter, "Capabilities: {}", self.capability_count)?;
        writeln!(formatter, "Reliable packets: {} pending, {} retransmits", self.reliable_packets_pending, self.reliable_retransmits)?;
        if let Some(error) = &self.last_error {
            write!(formatter, "Last error: {error}")?;
        }
        Ok(())
    }
}

impl fmt::Display for NetworkLifecycle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Idle => "idle",
            Self::LoggingIn => "logging in",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Disconnecting => "disconnecting",
            Self::Disconnected => "disconnected",
            Self::Failed => "failed",
        };
        formatter.write_str(label)
    }
}

#[derive(Clone)]
/// Shared network status; circuit handles contribute to the aggregate counters.
pub struct NetworkStatusHandle {
    state: Arc<NetworkStatus>,
    circuit_pending: Arc<AtomicUsize>,
}

impl Default for NetworkStatusHandle {
    fn default() -> Self {
        Self { state: Arc::new(NetworkStatus::default()), circuit_pending: Arc::new(AtomicUsize::new(0)) }
    }
}

impl fmt::Debug for NetworkStatusHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("NetworkStatusHandle").field(&self.snapshot()).finish()
    }
}

#[derive(Default)]
struct NetworkStatus {
    lifecycle: AtomicU8,
    simulator_endpoint: Mutex<Option<SocketAddr>>,
    circuit_established: AtomicU8,
    udp_packets_sent: AtomicU64,
    udp_packets_received: AtomicU64,
    http_requests: AtomicU64,
    http_responses: AtomicU64,
    capabilities: Mutex<HashSet<String>>,
    reliable_packets_pending: AtomicUsize,
    reliable_retransmits: AtomicU64,
    last_error: Mutex<Option<String>>,
}

impl NetworkStatusHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> NetworkStatusSnapshot {
        NetworkStatusSnapshot {
            lifecycle: lifecycle_from_u8(self.state.lifecycle.load(Ordering::Relaxed)),
            simulator_endpoint: *self.state.simulator_endpoint.lock().unwrap_or_else(PoisonError::into_inner),
            circuit_established: self.state.circuit_established.load(Ordering::Relaxed) != 0,
            udp_packets_sent: self.state.udp_packets_sent.load(Ordering::Relaxed),
            udp_packets_received: self.state.udp_packets_received.load(Ordering::Relaxed),
            http_requests: self.state.http_requests.load(Ordering::Relaxed),
            http_responses: self.state.http_responses.load(Ordering::Relaxed),
            capability_count: self.state.capabilities.lock().unwrap_or_else(PoisonError::into_inner).len(),
            reliable_packets_pending: self.state.reliable_packets_pending.load(Ordering::Relaxed),
            reliable_retransmits: self.state.reliable_retransmits.load(Ordering::Relaxed),
            last_error: self.state.last_error.lock().unwrap_or_else(PoisonError::into_inner).clone(),
        }
    }

    pub(crate) fn for_circuit(&self) -> Self {
        Self { state: self.state.clone(), circuit_pending: Arc::new(AtomicUsize::new(0)) }
    }

    pub(crate) fn set_lifecycle(&self, lifecycle: NetworkLifecycle) {
        self.state.lifecycle.store(lifecycle as u8, Ordering::Relaxed);
    }

    pub(crate) fn set_simulator_endpoint(&self, endpoint: SocketAddr) {
        *self.state.simulator_endpoint.lock().unwrap_or_else(PoisonError::into_inner) = Some(endpoint);
    }

    pub(crate) fn clear_simulator(&self) {
        *self.state.simulator_endpoint.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.state.circuit_established.store(0, Ordering::Relaxed);
        self.set_reliable_packets_pending(0);
    }

    pub(crate) fn set_circuit_established(&self, established: bool) {
        self.state.circuit_established.store(u8::from(established), Ordering::Relaxed);
    }

    pub(crate) fn note_udp_sent(&self) {
        self.state.udp_packets_sent.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn note_udp_received(&self) {
        self.state.udp_packets_received.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn note_http_request(&self) {
        self.state.http_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn note_http_response(&self) {
        self.state.http_responses.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn note_capabilities<'a>(&self, names: impl IntoIterator<Item = &'a str>) {
        let mut capabilities = self.state.capabilities.lock().unwrap_or_else(PoisonError::into_inner);
        capabilities.extend(names.into_iter().map(str::to_owned));
    }

    pub(crate) fn set_reliable_packets_pending(&self, count: usize) {
        let previous = self.circuit_pending.swap(count, Ordering::Relaxed);
        if count > previous {
            self.state.reliable_packets_pending.fetch_add(count - previous, Ordering::Relaxed);
        } else if previous > count {
            self.state.reliable_packets_pending.fetch_sub(previous - count, Ordering::Relaxed);
        }
    }

    pub(crate) fn note_retransmit(&self) {
        self.state.reliable_retransmits.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_error(&self, error: impl Into<String>) {
        let error = error.into();
        let sanitized = redact_urls(&error, MAX_STATUS_ERROR_CHARS);
        *self.state.last_error.lock().unwrap_or_else(PoisonError::into_inner) = Some(sanitized);
    }
}

fn redact_urls(value: &str, max_chars: usize) -> String {
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len().min(max_chars));
    let mut cursor = 0;
    let mut remaining = max_chars;

    while cursor < bytes.len() && remaining > 0 {
        let Some(start) = (cursor..bytes.len()).find(|&index| starts_http_url(bytes, index)) else {
            push_limited(&mut output, &value[cursor..], &mut remaining);
            break;
        };
        push_limited(&mut output, &value[cursor..start], &mut remaining);
        if remaining == 0 {
            break;
        }

        let mut end = start;
        while end < bytes.len() && !bytes[end].is_ascii_whitespace() && !matches!(bytes[end], b'"' | b'\'' | b'<' | b'>' | b'`') {
            end += 1;
        }
        push_limited(&mut output, "[redacted URL]", &mut remaining);
        cursor = end;
    }

    output
}

fn push_limited(output: &mut String, value: &str, remaining: &mut usize) {
    for character in value.chars() {
        if *remaining == 0 {
            break;
        }
        output.push(if character.is_control() { ' ' } else { character });
        *remaining -= 1;
    }
}

fn starts_http_url(bytes: &[u8], index: usize) -> bool {
    bytes.get(index..index + 7).is_some_and(|scheme| scheme.eq_ignore_ascii_case(b"http://"))
        || bytes.get(index..index + 8).is_some_and(|scheme| scheme.eq_ignore_ascii_case(b"https://"))
}

fn lifecycle_from_u8(value: u8) -> NetworkLifecycle {
    match value {
        value if value == NetworkLifecycle::LoggingIn as u8 => NetworkLifecycle::LoggingIn,
        value if value == NetworkLifecycle::Connecting as u8 => NetworkLifecycle::Connecting,
        value if value == NetworkLifecycle::Connected as u8 => NetworkLifecycle::Connected,
        value if value == NetworkLifecycle::Disconnecting as u8 => NetworkLifecycle::Disconnecting,
        value if value == NetworkLifecycle::Disconnected as u8 => NetworkLifecycle::Disconnected,
        value if value == NetworkLifecycle::Failed as u8 => NetworkLifecycle::Failed,
        _ => NetworkLifecycle::Idle,
    }
}

#[cfg(test)]
mod tests {
    use super::{NetworkLifecycle, NetworkStatusHandle};

    #[test]
    fn snapshot_tracks_counters() {
        let status = NetworkStatusHandle::new();
        let shared = status.clone();
        shared.note_udp_sent();
        shared.note_http_request();
        shared.note_http_response();
        shared.note_udp_received();
        shared.note_capabilities(["Seed", "EventQueueGet", "Seed"]);
        shared.set_simulator_endpoint("127.0.0.1:13000".parse().expect("loopback endpoint"));
        shared.set_circuit_established(true);
        shared.set_reliable_packets_pending(2);
        shared.note_retransmit();
        shared.record_error("fixture failure");
        shared.set_lifecycle(NetworkLifecycle::Connected);

        let snapshot = status.snapshot();
        assert_eq!(snapshot.lifecycle, NetworkLifecycle::Connected);
        assert_eq!(snapshot.udp_packets_sent, 1);
        assert_eq!(snapshot.udp_packets_received, 1);
        assert_eq!(snapshot.http_requests, 1);
        assert_eq!(snapshot.http_responses, 1);
        assert_eq!(snapshot.capability_count, 2);
        assert_eq!(snapshot.simulator_endpoint, Some("127.0.0.1:13000".parse().expect("loopback endpoint")));
        assert!(snapshot.circuit_established);
        assert_eq!(snapshot.reliable_packets_pending, 2);
        assert_eq!(snapshot.reliable_retransmits, 1);
        assert_eq!(snapshot.last_error.as_deref(), Some("fixture failure"));
    }

    #[test]
    fn clear_simulator_resets_live_state() {
        let status = NetworkStatusHandle::new();
        status.set_simulator_endpoint("127.0.0.1:13000".parse().expect("loopback endpoint"));
        status.set_circuit_established(true);
        status.set_reliable_packets_pending(2);
        status.note_retransmit();
        status.note_retransmit();

        status.clear_simulator();

        let snapshot = status.snapshot();
        assert_eq!(snapshot.simulator_endpoint, None);
        assert!(!snapshot.circuit_established);
        assert_eq!(snapshot.reliable_packets_pending, 0);
        assert_eq!(snapshot.reliable_retransmits, 2);
    }

    #[test]
    fn circuits_aggregate_status() {
        let session = NetworkStatusHandle::new();
        let first = session.for_circuit();
        let second = session.for_circuit();
        session.set_reliable_packets_pending(1);
        first.set_reliable_packets_pending(2);
        second.set_reliable_packets_pending(3);
        first.note_retransmit();
        second.note_retransmit();
        session.set_simulator_endpoint("127.0.0.1:13000".parse().expect("primary endpoint"));
        session.set_circuit_established(true);
        session.set_lifecycle(NetworkLifecycle::Connected);

        let snapshot = session.snapshot();
        assert_eq!(snapshot.reliable_packets_pending, 6);
        assert_eq!(snapshot.reliable_retransmits, 2);
        assert_eq!(snapshot.lifecycle, NetworkLifecycle::Connected);
        assert_eq!(snapshot.simulator_endpoint, Some("127.0.0.1:13000".parse().expect("primary endpoint")));
        assert!(snapshot.circuit_established);

        first.set_reliable_packets_pending(1);
        second.set_reliable_packets_pending(0);
        session.clear_simulator();
        let snapshot = session.snapshot();
        assert_eq!(snapshot.reliable_packets_pending, 1);
        assert_eq!(snapshot.lifecycle, NetworkLifecycle::Connected);
        assert_eq!(snapshot.simulator_endpoint, None);
        assert!(!snapshot.circuit_established);

        first.set_reliable_packets_pending(0);
        assert_eq!(session.snapshot().reliable_packets_pending, 0);
    }

    #[test]
    fn errors_redact_urls() {
        let status = NetworkStatusHandle::new();
        status.record_error("request failed at https://cap.example/secret-token?auth=private\nsecond line");

        assert_eq!(status.snapshot().last_error.as_deref(), Some("request failed at [redacted URL] second line"));
    }
}

#[cfg(test)]
mod redaction_tests {
    use super::{NetworkLifecycle, NetworkStatusHandle, NetworkStatusSnapshot};

    #[test]
    fn status_debug_redacts_secrets() {
        let snapshot = NetworkStatusSnapshot {
            lifecycle: NetworkLifecycle::Connected,
            simulator_endpoint: Some("192.0.2.77:13000".parse().expect("valid endpoint")),
            circuit_established: true,
            udp_packets_sent: 1,
            udp_packets_received: 2,
            http_requests: 3,
            http_responses: 3,
            capability_count: 4,
            reliable_packets_pending: 0,
            reliable_retransmits: 1,
            last_error: Some("server-error-secret".into()),
        };

        let debug = format!("{snapshot:?}");

        assert!(!debug.contains("192.0.2.77"));
        assert!(!debug.contains("server-error-secret"));
        assert!(format!("{snapshot}").contains("server-error-secret"));

        let status = NetworkStatusHandle::new();
        status.set_simulator_endpoint("192.0.2.78:13000".parse().expect("valid endpoint"));
        status.record_error("server-handle-error-secret");
        let debug = format!("{status:?}");
        assert!(!debug.contains("192.0.2.78"));
        assert!(!debug.contains("server-handle-error-secret"));
    }
}
