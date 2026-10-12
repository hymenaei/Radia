//! Types and configuration for opt-in network capture archives.
//!
//! Archives may contain raw datagrams and capability bodies; treat them as session data.

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{BufWriter, ErrorKind, Write, copy},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use base::UUID;
use protocol::lludp::{Message, Packet, PacketFlags};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use tokio::sync::oneshot;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

pub(crate) const CAPTURE_SCHEMA_VERSION: u16 = 1;
const CAPTURE_ENTRIES: [&str; 3] = ["manifest.json", "events.jsonl", "payloads.bin"];
/// Maximum uncompressed size of any one capture archive entry, in bytes.
pub const MAX_CAPTURE_ENTRY_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Maximum encoded size of one `events.jsonl` record, including its newline.
pub const MAX_CAPTURE_EVENT_LINE_BYTES: usize = 1024 * 1024;
const WRITER_QUEUE_CAPACITY: usize = 1024;
const QUEUED_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Configuration for storing network capture archives.
#[derive(Clone, Debug)]
pub struct CaptureConfig {
    root: PathBuf,
}

impl CaptureConfig {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Manifest metadata. Timestamps use RFC 3339; `payload_bytes` counts uncompressed bytes.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CaptureManifest {
    pub schema_version: u16,
    pub viewer_version: String,
    pub started_at: String,
    pub ended_at: String,
    pub event_count: u64,
    pub dropped_events: u64,
    pub payload_bytes: u64,
    pub state: CaptureState,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
/// Finalization state of a capture archive.
pub enum CaptureState {
    Recording,
    Complete,
    /// Recording finished with dropped events or a writer error.
    Incomplete,
    /// Recording stopped because the network session shut down.
    Interrupted,
}

/// One line from the `events.jsonl` entry of a capture archive.
/// `index` is zero-based; `elapsed_ns` is nanoseconds since capture start.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CaptureEvent {
    pub index: u64,
    pub elapsed_ns: u64,
    #[serde(flatten)]
    pub kind: CaptureEventKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payloads: Vec<CapturePayloadRef>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureEventKind {
    LoginRetry,
    LoginCompleted,
    LoginFailed,
    MfaChallenge,
    CircuitStarting {
        circuit_id: u32,
    },
    CircuitEstablished {
        circuit_id: u32,
    },
    SessionReady {
        circuit_id: u32,
    },
    SessionTimeout {
        circuit_id: u32,
    },
    CapabilitiesDiscovered,
    LogoutStarted {
        circuit_id: u32,
    },
    Disconnected {
        circuit_id: u32,
    },
    NetworkFailed,
    #[serde(rename = "lludp_send", alias = "udp_send")]
    LLUdpSend {
        circuit_id: u32,
        endpoint: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sequence: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reliable: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        resent: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        zerocoded: Option<bool>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        appended_acks: Vec<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        packet_acks: Vec<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    #[serde(rename = "lludp_receive", alias = "udp_receive")]
    LLUdpReceive {
        circuit_id: u32,
        endpoint: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sequence: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reliable: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        resent: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        zerocoded: Option<bool>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        appended_acks: Vec<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        packet_acks: Vec<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    HttpRequest {
        /// Correlates this request with its response or error record.
        request_id: u64,
        method: String,
        /// Logical operation name; capability URLs are never stored here.
        endpoint: String,
        headers: Vec<(String, String)>,
    },
    HttpResponse {
        request_id: u64,
        status: u16,
        /// Nanoseconds from request start to response body completion.
        duration_ns: u64,
        headers: Vec<(String, String)>,
    },
    HttpError {
        request_id: u64,
        phase: HttpErrorPhase,
        /// Nanoseconds from request start to failure.
        duration_ns: u64,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
/// Stage where a captured HTTP operation failed.
pub enum HttpErrorPhase {
    Request,
    Body,
}

/// A byte range in the archive's `payloads.bin` entry.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CapturePayloadRef {
    pub role: String,
    pub offset: u64,
    pub length: u64,
}

enum WriterCommand {
    Record(Box<QueuedRecord>),
    Finish,
    Interrupt,
}

struct QueuedRecord {
    elapsed_ns: u64,
    event: CaptureEventKind,
    payloads: Vec<Vec<u8>>,
    reserved_bytes: usize,
}

struct RecorderState {
    sender: SyncSender<WriterCommand>,
    started: Instant,
    serial: Mutex<()>,
    first_request_id: u64,
    queued_bytes: Arc<AtomicUsize>,
    dropped_records: Arc<AtomicU64>,
    recorded_events: AtomicU64,
    udp_packets: AtomicU64,
    http_requests: AtomicU64,
    http_responses: AtomicU64,
    closed: AtomicBool,
}

struct RecorderRegistry {
    active: Mutex<Option<Arc<RecorderState>>>,
    next_request_id: AtomicU64,
}

#[derive(Clone)]
pub(crate) struct CaptureRecorder {
    registry: Arc<RecorderRegistry>,
}

impl fmt::Debug for CaptureRecorder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("CaptureRecorder").field("active", &self.status().is_some()).finish()
    }
}

pub(crate) struct CaptureSession {
    recorder: CaptureRecorder,
    state: Arc<RecorderState>,
    writer: Option<JoinHandle<Result<CaptureSummary, String>>>,
    capture_path: PathBuf,
    workspace_path: PathBuf,
    lock_path: PathBuf,
}

pub(crate) struct CaptureSummary {
    pub(crate) state: CaptureState,
    pub(crate) dropped_records: u64,
    pub(crate) error: Option<String>,
    pub(crate) capture_path: Option<PathBuf>,
}

const PUBLICATION_PENDING: u8 = 0;
const PUBLICATION_INTERRUPTION_REQUESTED: u8 = 1;
const PUBLICATION_CLAIMED: u8 = 2;

#[derive(Clone, Default)]
pub(crate) struct CapturePublication(Arc<CapturePublicationState>);

#[derive(Default)]
struct CapturePublicationState {
    state: AtomicU8,
    #[cfg(test)]
    before_claim: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl CapturePublication {
    pub(crate) fn request_interruption(&self) {
        let _ = self.0.state.compare_exchange(PUBLICATION_PENDING, PUBLICATION_INTERRUPTION_REQUESTED, Ordering::AcqRel, Ordering::Acquire);
    }

    fn interruption_requested(&self) -> bool {
        self.0.state.load(Ordering::Acquire) == PUBLICATION_INTERRUPTION_REQUESTED
    }

    fn claim(&self, state: CaptureState) -> Result<CaptureState, CaptureState> {
        if state == CaptureState::Recording {
            unreachable!("recording cannot be published");
        }

        match self.0.state.compare_exchange(PUBLICATION_PENDING, PUBLICATION_CLAIMED, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => Ok(state),
            Err(PUBLICATION_INTERRUPTION_REQUESTED) if state == CaptureState::Interrupted => {
                self.0.state.store(PUBLICATION_CLAIMED, Ordering::Release);
                Ok(CaptureState::Interrupted)
            }
            Err(PUBLICATION_INTERRUPTION_REQUESTED) => Err(CaptureState::Interrupted),
            Err(PUBLICATION_CLAIMED) => unreachable!("capture publication was already claimed"),
            Err(_) => unreachable!("invalid capture publication state"),
        }
    }

    #[cfg(test)]
    pub(crate) fn pause_before_claim(&self, reached: oneshot::Sender<()>, release: mpsc::Receiver<()>) {
        let hook = Box::new(move || {
            let _ = reached.send(());
            let _ = release.recv();
        });
        *self.0.before_claim.lock().unwrap_or_else(PoisonError::into_inner) = Some(hook);
    }

    #[cfg(test)]
    fn before_claim(&self) {
        if let Some(hook) = self.0.before_claim.lock().unwrap_or_else(PoisonError::into_inner).take() {
            hook();
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// Recording counters. `active` means new events are accepted; `finalizing` means the archive is being written.
pub struct CaptureStatus {
    pub active: bool,
    pub finalizing: bool,
    pub elapsed: Duration,
    pub recorded_events: u64,
    pub udp_packets: u64,
    pub http_requests: u64,
    pub http_responses: u64,
}

impl CaptureSession {
    pub(crate) fn start(config: CaptureConfig, recorder: CaptureRecorder) -> Result<Self, String> {
        if recorder.is_active() {
            return Err("A capture session is already active.".into());
        }
        fs::create_dir_all(&config.root).map_err(|error| format!("Could not prepare the capture directory: {error}"))?;
        let (capture_path, workspace_path, lock_path) = create_capture_paths(&config.root)?;
        let started_at = now_rfc3339()?;
        let manifest = CaptureManifest {
            schema_version: CAPTURE_SCHEMA_VERSION,
            viewer_version: env!("CARGO_PKG_VERSION").into(),
            started_at: started_at.clone(),
            ended_at: started_at,
            event_count: 0,
            dropped_events: 0,
            payload_bytes: 0,
            state: CaptureState::Recording,
        };
        write_manifest_new(&workspace_path.join("manifest.json"), &manifest)?;

        let records = create_new_file(&workspace_path.join("events.jsonl"))?;
        let payloads = create_new_file(&workspace_path.join("payloads.bin"))?;
        let (sender, receiver) = mpsc::sync_channel(WRITER_QUEUE_CAPACITY);
        let dropped_records = Arc::new(AtomicU64::new(0));
        let writer_dropped_records = Arc::clone(&dropped_records);
        let recorder_queued_bytes = Arc::new(AtomicUsize::new(0));
        let writer_queued_bytes = Arc::clone(&recorder_queued_bytes);
        let writer_path = workspace_path.clone();
        let writer = thread::Builder::new()
            .name("capture-writer".into())
            .spawn(move || write_records(writer_path, records, payloads, receiver, writer_dropped_records, writer_queued_bytes))
            .map_err(|error| format!("Could not start network capture writer: {error}"))?;
        let state = Arc::new(RecorderState {
            sender,
            started: Instant::now(),
            serial: Mutex::new(()),
            first_request_id: recorder.registry.next_request_id.load(Ordering::Acquire),
            queued_bytes: recorder_queued_bytes,
            dropped_records,
            recorded_events: AtomicU64::new(0),
            udp_packets: AtomicU64::new(0),
            http_requests: AtomicU64::new(0),
            http_responses: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        });
        let session =
            Self { recorder: recorder.clone(), state: Arc::clone(&state), writer: Some(writer), capture_path, workspace_path, lock_path };
        if !recorder.install(state) {
            let _ = session.interrupt();
            return Err("A capture session is already active.".into());
        }
        Ok(session)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.capture_path
    }

    pub(crate) fn finish_recording(mut self, publication: &CapturePublication) -> CaptureSummary {
        let mut summary = self.stop_with(WriterCommand::Finish, publication);
        if summary.state == CaptureState::Incomplete && summary.error.is_none() {
            summary.error = Some(format!("Capture is incomplete: {} events were dropped.", summary.dropped_records));
        }
        self.remove_workspace();
        summary
    }

    pub(crate) fn interrupt(mut self) -> CaptureSummary {
        let summary = self.stop_with(WriterCommand::Interrupt, &CapturePublication::default());
        self.remove_workspace();
        summary
    }

    fn remove_workspace(&self) {
        if self.capture_path.is_file() {
            let _ = fs::remove_dir_all(&self.workspace_path);
        }
    }

    fn stop_with(&mut self, command: WriterCommand, publication: &CapturePublication) -> CaptureSummary {
        let mut summary = self.stop_writer(command);
        self.recorder.uninstall(&self.state);
        match write_capture_archive_atomically(&self.workspace_path, &self.capture_path, publication) {
            Ok(state) => summary.state = state,
            Err(error) => {
                let _ = fs::remove_file(&self.lock_path);
                return CaptureSummary {
                    state: summary.state,
                    dropped_records: summary.dropped_records,
                    error: Some(match summary.error {
                        Some(writer_error) => format!("{writer_error}; could not package raw capture: {error}"),
                        None => format!("Could not package raw capture: {error}"),
                    }),
                    capture_path: None,
                };
            }
        }
        let _ = fs::remove_file(&self.lock_path);
        summary.capture_path = Some(self.capture_path.clone());
        summary
    }

    fn stop_writer(&mut self, command: WriterCommand) -> CaptureSummary {
        let should_finish = {
            let _serial = self.state.serial.lock().unwrap_or_else(PoisonError::into_inner);
            // Close under the record lock so accepted events precede the terminal command.
            !self.state.closed.swap(true, Ordering::AcqRel)
        };
        if should_finish {
            let _ = self.state.sender.send(command);
        }
        match self.writer.take().map(JoinHandle::join) {
            Some(Ok(Ok(summary))) => summary,
            Some(Ok(Err(error))) => CaptureSummary {
                state: CaptureState::Incomplete,
                dropped_records: self.state.dropped_records.load(Ordering::Acquire),
                error: Some(error),
                capture_path: None,
            },
            Some(Err(_)) => CaptureSummary {
                state: CaptureState::Incomplete,
                dropped_records: self.state.dropped_records.load(Ordering::Acquire),
                error: Some("The network capture writer stopped unexpectedly.".into()),
                capture_path: None,
            },
            None => CaptureSummary {
                state: CaptureState::Incomplete,
                dropped_records: 0,
                error: Some("The network capture writer was already stopped.".into()),
                capture_path: None,
            },
        }
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        if self.writer.is_some() {
            let _ = self.stop_with(WriterCommand::Interrupt, &CapturePublication::default());
            self.remove_workspace();
        }
    }
}

impl CaptureRecorder {
    pub(crate) fn new() -> Self {
        Self { registry: Arc::new(RecorderRegistry { active: Mutex::new(None), next_request_id: AtomicU64::new(1) }) }
    }

    fn current_state(&self) -> Option<Arc<RecorderState>> {
        self.registry.active.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn active_state(&self) -> Option<Arc<RecorderState>> {
        self.current_state().filter(|state| !state.closed.load(Ordering::Acquire))
    }

    fn is_active(&self) -> bool {
        self.current_state().is_some()
    }

    fn install(&self, state: Arc<RecorderState>) -> bool {
        let mut active = self.registry.active.lock().unwrap_or_else(PoisonError::into_inner);
        if active.is_some() {
            return false;
        }
        *active = Some(state);
        true
    }

    fn uninstall(&self, state: &Arc<RecorderState>) {
        let mut active = self.registry.active.lock().unwrap_or_else(PoisonError::into_inner);
        if active.as_ref().is_some_and(|current| Arc::ptr_eq(current, state)) {
            active.take();
        }
    }

    pub(crate) fn status(&self) -> Option<CaptureStatus> {
        let state = self.current_state()?;
        let finalizing = state.closed.load(Ordering::Acquire);
        Some(CaptureStatus {
            active: !finalizing,
            finalizing,
            elapsed: state.started.elapsed(),
            recorded_events: state.recorded_events.load(Ordering::Acquire),
            udp_packets: state.udp_packets.load(Ordering::Acquire),
            http_requests: state.http_requests.load(Ordering::Acquire),
            http_responses: state.http_responses.load(Ordering::Acquire),
        })
    }

    pub(crate) fn lifecycle(&self, name: &'static str) {
        if let Some(event) = lifecycle_event(name) {
            self.record(event, Vec::new());
        }
    }

    pub(crate) fn circuit_lifecycle(&self, name: &'static str, circuit_id: u32) {
        if let Some(event) = circuit_lifecycle_event(name, circuit_id) {
            self.record(event, Vec::new());
        }
    }

    pub(crate) fn udp(&self, circuit_id: u32, direction: &str, _local: SocketAddr, remote: SocketAddr, bytes: &[u8]) {
        let Some(state) = self.active_state() else {
            return;
        };
        let (sequence, reliable, resent, zerocoded, appended_acks, packet_acks, message) = match Packet::decode(bytes) {
            Ok(packet) => {
                let packet_acks = match packet.message() {
                    Message::PacketAck(message) => message.packets.iter().map(|packet| packet.id).collect(),
                    _ => Vec::new(),
                };
                (
                    Some(packet.sequence()),
                    Some(packet.is_reliable()),
                    Some(packet.is_resent()),
                    Some(bytes.first().is_some_and(|flags| *flags & PacketFlags::ZEROCODED.bits() != 0)),
                    packet.appended_acks().to_vec(),
                    packet_acks,
                    (message_name(packet.message()) != "Unknown").then(|| message_name(packet.message()).to_owned()),
                )
            }
            Err(_) => (None, None, None, None, Vec::new(), Vec::new(), None),
        };
        let event = match direction {
            "in" => CaptureEventKind::LLUdpReceive {
                circuit_id,
                endpoint: remote.to_string(),
                sequence,
                reliable,
                resent,
                zerocoded,
                appended_acks,
                packet_acks,
                message,
            },
            "out" => CaptureEventKind::LLUdpSend {
                circuit_id,
                endpoint: remote.to_string(),
                sequence,
                reliable,
                resent,
                zerocoded,
                appended_acks,
                packet_acks,
                message,
            },
            _ => return,
        };
        Self::record_state(&state, event, vec![bytes.to_vec()]);
    }

    pub(crate) fn http_request(&self, operation: &str, method: &str, headers: Vec<(String, String)>, body: &[u8]) -> u64 {
        let Some(state) = self.active_state() else {
            return 0;
        };
        let request_id = self.registry.next_request_id.fetch_add(1, Ordering::Relaxed);
        Self::record_state(
            &state,
            CaptureEventKind::HttpRequest { request_id, method: method.into(), endpoint: operation.into(), headers },
            vec![body.to_vec()],
        );
        request_id
    }

    pub(crate) fn http_response(
        &self,
        request_id: u64,
        status: Option<u16>,
        headers: Vec<(String, String)>,
        duration: Duration,
        outcome: &str,
        body: Option<&[u8]>,
    ) {
        let Some(state) = self.active_state() else {
            return;
        };
        if request_id == 0 || request_id < state.first_request_id {
            return;
        }
        let duration_ns = duration.as_nanos().min(u128::from(u64::MAX)) as u64;
        match (status, outcome) {
            (Some(status), "complete") => {
                Self::record_state(
                    &state,
                    CaptureEventKind::HttpResponse { request_id, status, duration_ns, headers },
                    body.map(|body| vec![body.to_vec()]).unwrap_or_default(),
                );
            }
            (Some(status), "body_error") => {
                Self::record_state(&state, CaptureEventKind::HttpResponse { request_id, status, duration_ns, headers }, Vec::new());
                Self::record_state(
                    &state,
                    CaptureEventKind::HttpError { request_id, phase: HttpErrorPhase::Body, duration_ns },
                    Vec::new(),
                );
            }
            _ => {
                Self::record_state(
                    &state,
                    CaptureEventKind::HttpError { request_id, phase: HttpErrorPhase::Request, duration_ns },
                    Vec::new(),
                );
            }
        }
    }

    fn record(&self, event: CaptureEventKind, payloads: Vec<Vec<u8>>) -> Option<()> {
        let state = self.active_state()?;
        Self::record_state(&state, event, payloads)
    }

    fn record_state(state: &RecorderState, event: CaptureEventKind, payloads: Vec<Vec<u8>>) -> Option<()> {
        let is_lludp = matches!(&event, CaptureEventKind::LLUdpSend { .. } | CaptureEventKind::LLUdpReceive { .. });
        let is_http_request = matches!(&event, CaptureEventKind::HttpRequest { .. });
        let is_http_response = matches!(&event, CaptureEventKind::HttpResponse { .. });
        let payloads = payloads.into_iter().filter(|payload| is_lludp || !payload.is_empty()).collect::<Vec<_>>();
        let Some(reserved_bytes) = payloads.iter().try_fold(0usize, |total, bytes| total.checked_add(bytes.len())) else {
            state.dropped_records.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        let Ok(_serial) = state.serial.lock() else {
            state.dropped_records.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        if state.closed.load(Ordering::Acquire) {
            return None;
        }
        if !reserve_payload_bytes(&state.queued_bytes, reserved_bytes) {
            state.dropped_records.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        let elapsed_ns = state.started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        let queued = QueuedRecord { elapsed_ns, event, payloads, reserved_bytes };
        if let Err(error) = state.sender.try_send(WriterCommand::Record(Box::new(queued))) {
            state.queued_bytes.fetch_sub(reserved_bytes, Ordering::AcqRel);
            state.dropped_records.fetch_add(1, Ordering::Relaxed);
            if matches!(error, TrySendError::Disconnected(_)) {
                state.closed.store(true, Ordering::Release);
            }
            return None;
        }
        state.recorded_events.fetch_add(1, Ordering::Relaxed);
        if is_lludp {
            state.udp_packets.fetch_add(1, Ordering::Relaxed);
        }
        if is_http_request {
            state.http_requests.fetch_add(1, Ordering::Relaxed);
        }
        if is_http_response {
            state.http_responses.fetch_add(1, Ordering::Relaxed);
        }
        Some(())
    }
}

fn lifecycle_event(name: &str) -> Option<CaptureEventKind> {
    Some(match name {
        "capture_started" | "capture_stopped" | "capture_interrupted" => return None,
        "login_retry" => CaptureEventKind::LoginRetry,
        "login_completed" => CaptureEventKind::LoginCompleted,
        "login_failed" => CaptureEventKind::LoginFailed,
        "mfa_challenge" => CaptureEventKind::MfaChallenge,
        "capabilities_discovered" => CaptureEventKind::CapabilitiesDiscovered,
        "network_failed" => CaptureEventKind::NetworkFailed,
        _ => return None,
    })
}

fn circuit_lifecycle_event(name: &str, circuit_id: u32) -> Option<CaptureEventKind> {
    Some(match name {
        "circuit_starting" | "use_circuit_code_sent" => CaptureEventKind::CircuitStarting { circuit_id },
        "use_circuit_code_acked" => CaptureEventKind::CircuitEstablished { circuit_id },
        "session_ready" => CaptureEventKind::SessionReady { circuit_id },
        "session_timeout" => CaptureEventKind::SessionTimeout { circuit_id },
        "disconnect_started" => CaptureEventKind::LogoutStarted { circuit_id },
        "disconnected" => CaptureEventKind::Disconnected { circuit_id },
        _ => return None,
    })
}

fn reserve_payload_bytes(queued_bytes: &AtomicUsize, amount: usize) -> bool {
    if amount > QUEUED_PAYLOAD_BYTES {
        return false;
    }
    let mut current = queued_bytes.load(Ordering::Acquire);
    loop {
        let Some(next) = current.checked_add(amount).filter(|next| *next <= QUEUED_PAYLOAD_BYTES) else {
            return false;
        };
        match queued_bytes.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(actual) => current = actual,
        }
    }
}

struct SizeLimitedWriter<W> {
    writer: W,
    remaining: u64,
    exceeded_limit: bool,
}

impl<W> SizeLimitedWriter<W> {
    fn new(writer: W, limit: u64) -> Self {
        Self { writer, remaining: limit, exceeded_limit: false }
    }
}

impl<W: Write> Write for SizeLimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if bytes.len() as u64 > self.remaining {
            self.exceeded_limit = true;
            return Err(std::io::Error::new(ErrorKind::WriteZero, "capture archive entry size limit exceeded"));
        }
        let written = self.writer.write(bytes)?;
        self.remaining -= written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

fn write_records(
    path: PathBuf,
    records: File,
    payloads: File,
    receiver: mpsc::Receiver<WriterCommand>,
    dropped_records: Arc<AtomicU64>,
    queued_bytes: Arc<AtomicUsize>,
) -> Result<CaptureSummary, String> {
    let mut records = SizeLimitedWriter::new(BufWriter::new(records), MAX_CAPTURE_ENTRY_BYTES);
    let mut payloads = BufWriter::new(payloads);
    let mut manifest = read_manifest(&path.join("manifest.json"))?;
    let mut payload_offset = 0u64;
    let mut event_count = 0u64;

    let mut writer_error = None;
    let mut interrupted = false;
    while let Ok(command) = receiver.recv() {
        match command {
            WriterCommand::Finish => break,
            WriterCommand::Interrupt => {
                interrupted = true;
                break;
            }
            WriterCommand::Record(record) => {
                let record = *record;
                let role = payload_role(&record.event);
                if !valid_payload_count(&record.event, record.payloads.len()) {
                    queued_bytes.fetch_sub(record.reserved_bytes, Ordering::AcqRel);
                    writer_error = Some("Capture event has an invalid payload count.".to_string());
                    break;
                }
                let mut refs = Vec::with_capacity(record.payloads.len());
                let mut result = Ok(());
                for payload in record.payloads {
                    let length = match u64::try_from(payload.len()) {
                        Ok(length) => length,
                        Err(_) => {
                            result = Err("Capture payload length overflowed.".to_string());
                            break;
                        }
                    };
                    let Some(next_offset) = payload_offset.checked_add(length).filter(|next| *next <= MAX_CAPTURE_ENTRY_BYTES) else {
                        result = Err("Capture payload exceeds the archive entry size limit.".to_string());
                        break;
                    };
                    if let Err(error) = payloads.write_all(&payload) {
                        result = Err(format!("Could not write network capture payload: {error}"));
                        break;
                    }
                    let Some(role) = role else {
                        result = Err("Capture event payload has no role.".to_string());
                        break;
                    };
                    refs.push(CapturePayloadRef { role: role.into(), offset: payload_offset, length });
                    payload_offset = next_offset;
                }
                if let Err(error) = result {
                    queued_bytes.fetch_sub(record.reserved_bytes, Ordering::AcqRel);
                    writer_error = Some(error);
                    break;
                }
                let event = CaptureEvent { index: event_count, elapsed_ns: record.elapsed_ns, kind: record.event, payloads: refs };
                let (line, result, line_exceeded_limit) = {
                    let mut line = SizeLimitedWriter::new(Vec::new(), MAX_CAPTURE_EVENT_LINE_BYTES as u64);
                    let result = serde_json::to_writer(&mut line, &event)
                        .map_err(|error| format!("Could not encode a network capture event: {error}"))
                        .and_then(|()| line.write_all(b"\n").map_err(|error| format!("Could not write network capture event: {error}")));
                    (line.writer, result, line.exceeded_limit)
                };
                if let Err(error) = result {
                    queued_bytes.fetch_sub(record.reserved_bytes, Ordering::AcqRel);
                    writer_error = Some(if line_exceeded_limit {
                        "Capture event exceeds the per-event line size limit.".into()
                    } else if records.exceeded_limit {
                        "Capture events exceed the archive entry size limit.".into()
                    } else {
                        error
                    });
                    break;
                }
                if let Err(error) = records.write_all(&line) {
                    queued_bytes.fetch_sub(record.reserved_bytes, Ordering::AcqRel);
                    writer_error = Some(if records.exceeded_limit {
                        "Capture events exceed the archive entry size limit.".into()
                    } else {
                        format!("Could not write network capture event: {error}")
                    });
                    break;
                }
                event_count = event_count.saturating_add(1);
                queued_bytes.fetch_sub(record.reserved_bytes, Ordering::AcqRel);
            }
        }
    }

    if writer_error.is_none() {
        for result in [
            records.flush().map_err(|error| format!("Could not flush network capture records: {error}")),
            payloads.flush().map_err(|error| format!("Could not flush network capture payloads: {error}")),
            records.writer.get_ref().sync_all().map_err(|error| format!("Could not sync network capture records: {error}")),
            payloads.get_ref().sync_all().map_err(|error| format!("Could not sync network capture payloads: {error}")),
        ] {
            if let Err(error) = result {
                writer_error = Some(error);
                break;
            }
        }
    }

    manifest.dropped_events = dropped_records.load(Ordering::Acquire);
    manifest.event_count = event_count;
    manifest.payload_bytes = payload_offset;
    manifest.ended_at = now_rfc3339()?;
    manifest.state = if interrupted {
        CaptureState::Interrupted
    } else if manifest.dropped_events == 0 && writer_error.is_none() {
        CaptureState::Complete
    } else {
        CaptureState::Incomplete
    };
    replace_manifest(&path.join("manifest.json"), &manifest)?;
    Ok(CaptureSummary { state: manifest.state, dropped_records: manifest.dropped_events, error: writer_error, capture_path: None })
}

fn payload_role(event: &CaptureEventKind) -> Option<&'static str> {
    match event {
        CaptureEventKind::LLUdpSend { .. } | CaptureEventKind::LLUdpReceive { .. } => Some("datagram"),
        CaptureEventKind::HttpRequest { .. } | CaptureEventKind::HttpResponse { .. } => Some("body"),
        _ => None,
    }
}

fn valid_payload_count(event: &CaptureEventKind, payload_count: usize) -> bool {
    match event {
        CaptureEventKind::LLUdpSend { .. } | CaptureEventKind::LLUdpReceive { .. } => payload_count == 1,
        CaptureEventKind::HttpRequest { .. } | CaptureEventKind::HttpResponse { .. } => payload_count <= 1,
        _ => payload_count == 0,
    }
}

fn create_new_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|error| format!("Could not create network capture file: {error}"))
}

fn write_manifest_new(path: &Path, manifest: &CaptureManifest) -> Result<(), String> {
    let file = create_new_file(path)?;
    write_manifest_contents(file, manifest)
}

fn replace_manifest(path: &Path, manifest: &CaptureManifest) -> Result<(), String> {
    let file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|error| format!("Could not open network capture manifest for update: {error}"))?;
    write_manifest_contents(file, manifest)
}

fn write_manifest_contents(file: File, manifest: &CaptureManifest) -> Result<(), String> {
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, manifest).map_err(|error| format!("Could not encode network capture manifest: {error}"))?;
    writer.write_all(b"\n").map_err(|error| format!("Could not write network capture manifest: {error}"))?;
    writer.flush().map_err(|error| format!("Could not flush network capture manifest: {error}"))?;
    writer.get_ref().sync_all().map_err(|error| format!("Could not sync network capture manifest: {error}"))?;
    Ok(())
}

fn read_manifest(path: &Path) -> Result<CaptureManifest, String> {
    let bytes = fs::read(path).map_err(|error| format!("Could not read network capture manifest: {error}"))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("Network capture manifest is invalid: {error}"))
}

fn message_name(message: &Message) -> &'static str {
    match message {
        Message::UseCircuitCode(_) => "UseCircuitCode",
        Message::CompleteAgentMovement(_) => "CompleteAgentMovement",
        Message::RegionHandshake(_) => "RegionHandshake",
        Message::RegionHandshakeReply(_) => "RegionHandshakeReply",
        Message::AgentMovementComplete(_) => "AgentMovementComplete",
        Message::AgentUpdate(_) => "AgentUpdate",
        Message::StartPingCheck(_) => "StartPingCheck",
        Message::CompletePingCheck(_) => "CompletePingCheck",
        Message::PacketAck(_) => "PacketAck",
        Message::LogoutRequest(_) => "LogoutRequest",
        Message::LogoutReply(_) => "LogoutReply",
        Message::ChatFromViewer(_) => "ChatFromViewer",
        Message::ChatFromSimulator(_) => "ChatFromSimulator",
        Message::RegionHandleRequest(_) => "RegionHandleRequest",
        Message::RegionIdAndHandleReply(_) => "RegionIdAndHandleReply",
        Message::MapBlockRequest(_) => "MapBlockRequest",
        Message::MapBlockReply(_) => "MapBlockReply",
        Message::EnableSimulator(_) => "EnableSimulator",
        Message::DisableSimulator(_) => "DisableSimulator",
        Message::ConfirmEnableSimulator(_) => "ConfirmEnableSimulator",
        Message::ObjectUpdate(_) => "ObjectUpdate",
        Message::ObjectUpdateCompressed(_) => "ObjectUpdateCompressed",
        Message::ImprovedTerseObjectUpdate(_) => "ImprovedTerseObjectUpdate",
        Message::ObjectUpdateCached(_) => "ObjectUpdateCached",
        Message::KillObject(_) => "KillObject",
        Message::RequestMultipleObjects(_) => "RequestMultipleObjects",
        Message::Schema(message) => message.schema().name,
        Message::Unknown { .. } => "Unknown",
    }
}

fn create_capture_paths(root: &Path) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    for _ in 0..64 {
        let identifier = capture_identifier();
        let capture_archive = root.join(format!("{identifier}.capture"));
        let lock = root.join(format!(".{identifier}.lock"));
        let workspace = root.join(format!(".{identifier}.work"));
        if capture_archive.exists() {
            continue;
        }
        let lock_file = match OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Could not reserve a capture name: {error}")),
        };
        drop(lock_file);
        match try_create_private_dir(&workspace) {
            Ok(()) => {
                if capture_archive.exists() {
                    let _ = fs::remove_dir(&workspace);
                    let _ = fs::remove_file(&lock);
                    continue;
                }
                return Ok((capture_archive, workspace, lock));
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(&lock);
                continue;
            }
            Err(error) => {
                let _ = fs::remove_file(&lock);
                return Err(format!("Could not create private capture workspace: {error}"));
            }
        }
    }
    Err("Could not allocate a unique capture identifier after 64 attempts.".into())
}

pub(crate) fn capture_identifier() -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let random = UUID::new_v4();
    let mut value = 0u64;
    for byte in &random.as_bytes()[..5] {
        value = (value << 8) | u64::from(*byte);
    }
    (0..8)
        .map(|index| {
            let shift = 35 - index * 5;
            char::from(ALPHABET[((value >> shift) & 0x1f) as usize])
        })
        .collect()
}

pub(crate) fn write_capture_archive(source: &Path, destination: &Path) -> Result<(), String> {
    let file = create_new_file(destination).map_err(|error| format!("Could not create capture archive: {error}"))?;
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for name in CAPTURE_ENTRIES {
        archive.start_file(name, options).map_err(|error| format!("Could not write capture archive: {error}"))?;
        let mut input = File::open(source.join(name)).map_err(|error| format!("Could not read capture {name}: {error}"))?;
        copy(&mut input, &mut archive).map_err(|error| format!("Could not write capture {name}: {error}"))?;
    }
    let file = archive.finish().map_err(|error| format!("Could not finish capture archive: {error}"))?;
    file.sync_all().map_err(|error| format!("Could not sync capture archive: {error}"))?;
    Ok(())
}

fn write_capture_archive_atomically(source: &Path, destination: &Path, publication: &CapturePublication) -> Result<CaptureState, String> {
    let parent = destination.parent().ok_or_else(|| "Capture archive path has no parent directory.".to_string())?;
    let manifest_path = source.join("manifest.json");
    loop {
        let mut manifest = read_manifest(&manifest_path)?;
        if publication.interruption_requested() && manifest.state != CaptureState::Interrupted {
            manifest.state = CaptureState::Interrupted;
            replace_manifest(&manifest_path, &manifest)?;
            continue;
        }
        if manifest.state == CaptureState::Recording {
            manifest.state = CaptureState::Incomplete;
            replace_manifest(&manifest_path, &manifest)?;
            continue;
        }
        let temporary = create_archive_temp_path(parent)?;
        if let Err(error) = write_capture_archive(source, &temporary) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        if destination.exists() {
            let _ = fs::remove_file(&temporary);
            return Err("Capture archive path already exists.".into());
        }

        #[cfg(test)]
        publication.before_claim();

        let state = match publication.claim(manifest.state) {
            Ok(state) => state,
            Err(state) => {
                let _ = fs::remove_file(&temporary);
                manifest.state = state;
                replace_manifest(&manifest_path, &manifest)?;
                continue;
            }
        };

        fs::rename(&temporary, destination).map_err(|error| {
            let _ = fs::remove_file(&temporary);
            format!("Could not publish raw capture archive: {error}")
        })?;
        return Ok(state);
    }
}

fn create_archive_temp_path(parent: &Path) -> Result<PathBuf, String> {
    for _ in 0..64 {
        let path = parent.join(format!(".capture-{}.tmp", capture_identifier()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                drop(file);
                let _ = fs::remove_file(&path);
                return Ok(path);
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Could not reserve a capture archive path: {error}")),
        }
    }
    Err("Could not allocate a temporary capture archive name.".into())
}

fn now_rfc3339() -> Result<String, String> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| format!("Could not format capture timestamp: {error}"))
}

fn try_create_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CaptureRecorder, CaptureSession, CaptureState, CaptureSummary, RecorderState, SizeLimitedWriter, WriterCommand, message_name,
    };
    use protocol::{lludp::Message, messages::RegionIdAndHandleReply};
    use std::{
        io::Write,
        path::PathBuf,
        sync::{
            Arc, Barrier, Mutex,
            atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
            mpsc::sync_channel,
        },
        thread,
        time::Instant,
    };

    #[test]
    fn region_reply_capture_name_uses_rust_identifier_casing() {
        let message = Message::RegionIdAndHandleReply(RegionIdAndHandleReply::default());

        assert_eq!(message_name(&message), "RegionIdAndHandleReply");
    }

    #[test]
    fn capture_entry_writer_enforces_size_limit() {
        let mut writer = SizeLimitedWriter::new(Vec::new(), 3);

        writer.write_all(b"abc").expect("write up to the entry limit");
        assert!(writer.write_all(b"d").is_err());
        assert!(writer.exceeded_limit);
        assert_eq!(writer.writer, b"abc");
    }

    #[test]
    fn capture_finalization_releases_lock_during_write() {
        let (sender, receiver) = sync_channel(1);
        let writer_entered = Arc::new(Barrier::new(2));
        let writer_continue = Arc::new(Barrier::new(2));
        let entered = Arc::clone(&writer_entered);
        let continue_writer = Arc::clone(&writer_continue);
        let writer = thread::spawn(move || {
            assert!(matches!(receiver.recv(), Ok(WriterCommand::Finish)));
            entered.wait();
            continue_writer.wait();
            Ok(CaptureSummary { state: CaptureState::Complete, dropped_records: 0, error: None, capture_path: None })
        });
        let state = Arc::new(RecorderState {
            sender,
            started: Instant::now(),
            serial: Mutex::new(()),
            first_request_id: 1,
            queued_bytes: Arc::new(AtomicUsize::new(0)),
            dropped_records: Arc::new(AtomicU64::new(0)),
            recorded_events: AtomicU64::new(0),
            udp_packets: AtomicU64::new(0),
            http_requests: AtomicU64::new(0),
            http_responses: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        });
        let session = CaptureSession {
            recorder: CaptureRecorder::new(),
            state: Arc::clone(&state),
            writer: Some(writer),
            capture_path: PathBuf::new(),
            workspace_path: PathBuf::new(),
            lock_path: PathBuf::new(),
        };

        let finalizer = thread::spawn(move || {
            let mut session = session;
            session.stop_writer(WriterCommand::Finish)
        });
        writer_entered.wait();
        let record_lock_available = state.serial.try_lock().is_ok();
        writer_continue.wait();
        finalizer.join().expect("capture finalizer thread");

        assert!(record_lock_available, "recording must not remain locked while the writer is joining");
        assert!(state.closed.load(Ordering::Acquire));
    }
}

#[cfg(test)]
mod format_tests {
    use crate::http::test_support::{LocalServer, response};
    use std::{
        fs,
        io::{BufRead, BufReader, Cursor, Read, Write},
        net::{SocketAddr, TcpListener},
        path::{Path, PathBuf},
        sync::mpsc,
        thread,
        time::Duration,
    };
    use zip::ZipArchive;

    use llsd::Value;
    use protocol::lludp::{Message, Packet};
    use serde_json::{from_slice, from_str, from_value, json, to_value};
    use test_support::{lludp::vectors, parse_hex_fixture};
    use tokio::{spawn, task::spawn_blocking, time::timeout};
    use url::Url;

    use crate::{
        capabilities,
        capabilities::CapabilityUrl,
        capture::{
            CaptureConfig, CaptureEvent, CaptureEventKind, CaptureManifest, CapturePublication, CaptureRecorder, CaptureSession,
            CaptureState, MAX_CAPTURE_EVENT_LINE_BYTES,
        },
        http::Client,
    };
    use tempfile::{TempDir, tempdir};

    fn start_capture(directory: &TempDir, recorder: &CaptureRecorder) -> CaptureSession {
        CaptureSession::start(CaptureConfig::new(directory.path().join("captures")), recorder.clone()).expect("start capture")
    }

    fn finish_capture(session: CaptureSession) -> PathBuf {
        let summary = session.finish_recording(&CapturePublication::default());
        assert_eq!(summary.state, CaptureState::Complete, "capture failed: {:?}", summary.error);
        summary.capture_path.unwrap_or_else(|| panic!("finished capture path: {:?}", summary.error))
    }

    fn archive_entry(path: &Path, name: &str) -> Vec<u8> {
        let file = fs::File::open(path).expect("open capture archive");
        let mut archive = ZipArchive::new(file).expect("decode ZIP container");
        let mut entry = archive.by_name(name).expect("find capture entry");
        let mut contents = Vec::new();
        entry.read_to_end(&mut contents).expect("read capture entry");
        contents
    }

    fn archive_entries(path: &Path) -> Vec<String> {
        let file = fs::File::open(path).expect("open capture archive");
        let mut archive = ZipArchive::new(file).expect("decode ZIP container");
        (0..archive.len()).map(|index| archive.by_index(index).expect("read capture entry").name().to_owned()).collect()
    }

    fn capture_events(path: &Path) -> Vec<CaptureEvent> {
        BufReader::new(Cursor::new(archive_entry(path, "events.jsonl")))
            .lines()
            .map(|line| from_str(&line.expect("read capture event")).expect("parse capture event"))
            .collect()
    }

    fn payload<'a>(payloads: &'a [u8], event: &CaptureEvent) -> &'a [u8] {
        let reference = event.payloads.first().expect("capture event payload reference");
        let start = usize::try_from(reference.offset).expect("payload offset");
        let end = start + usize::try_from(reference.length).expect("payload length");
        &payloads[start..end]
    }

    fn check_capture(path: &Path) -> (CaptureManifest, Vec<CaptureEvent>, Vec<u8>, usize) {
        assert_eq!(archive_entries(path), ["manifest.json", "events.jsonl", "payloads.bin"]);
        let manifest: CaptureManifest = from_slice(&archive_entry(path, "manifest.json")).expect("parse capture manifest");
        let events = capture_events(path);
        let payloads = archive_entry(path, "payloads.bin");

        assert_eq!(manifest.schema_version, 1);
        assert!(!manifest.viewer_version.is_empty());
        assert_ne!(manifest.state, CaptureState::Recording);
        assert_eq!(manifest.event_count, events.len() as u64);
        assert_eq!(manifest.payload_bytes, payloads.len() as u64);

        let mut elapsed_ns = 0;
        let mut decoded_packets = 0;
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event.index, index as u64);
            assert!(event.elapsed_ns >= elapsed_ns);
            elapsed_ns = event.elapsed_ns;

            for reference in &event.payloads {
                let end = reference.offset.checked_add(reference.length).expect("payload range does not overflow");
                assert!(end <= manifest.payload_bytes, "payload range exceeds payloads.bin");
            }

            match &event.kind {
                CaptureEventKind::LLUdpSend { sequence, reliable, resent, zerocoded, appended_acks, packet_acks, .. }
                | CaptureEventKind::LLUdpReceive { sequence, reliable, resent, zerocoded, appended_acks, packet_acks, .. } => {
                    assert_eq!(event.payloads.len(), 1);
                    assert_eq!(event.payloads[0].role, "datagram");
                    match Packet::decode(payload(&payloads, event)) {
                        Ok(packet) => {
                            decoded_packets += 1;
                            assert_eq!(*sequence, Some(packet.sequence()));
                            assert_eq!(*reliable, Some(packet.is_reliable()));
                            assert_eq!(*resent, Some(packet.is_resent()));
                            assert_eq!(*zerocoded, Some(packet.is_zerocoded().expect("read packet zero-code flag")));
                            assert_eq!(appended_acks.as_slice(), packet.appended_acks());
                            let actual_packet_acks = match packet.message() {
                                Message::PacketAck(message) => message.packets.iter().map(|ack| ack.id).collect::<Vec<_>>(),
                                _ => Vec::new(),
                            };
                            assert_eq!(*packet_acks, actual_packet_acks);
                        }
                        Err(_) => {
                            assert!(sequence.is_none() && reliable.is_none() && resent.is_none() && zerocoded.is_none());
                            assert!(appended_acks.is_empty() && packet_acks.is_empty());
                        }
                    }
                }
                CaptureEventKind::HttpRequest { .. } | CaptureEventKind::HttpResponse { .. } => {
                    assert!(event.payloads.len() <= 1);
                    assert!(event.payloads.iter().all(|reference| reference.role == "body"));
                }
                _ => assert!(event.payloads.is_empty()),
            }
        }

        (manifest, events, payloads, decoded_packets)
    }

    #[test]
    fn lludp_capture_kind_names_keep_legacy_read_support() {
        let legacy = json!({ "kind": "udp_receive", "circuit_id": 3, "endpoint": "127.0.0.1:13000" });
        let event: CaptureEventKind = from_value(legacy).expect("read legacy UDP capture kind");
        assert!(matches!(event, CaptureEventKind::LLUdpReceive { circuit_id: 3, .. }));

        let event = CaptureEventKind::LLUdpSend {
            circuit_id: 3,
            endpoint: "127.0.0.1:13000".into(),
            sequence: None,
            reliable: None,
            resent: None,
            zerocoded: None,
            appended_acks: Vec::new(),
            packet_acks: Vec::new(),
            message: None,
        };
        let encoded = to_value(event).expect("write LLUDP capture kind");
        assert_eq!(encoded["kind"], "lludp_send");
    }

    #[test]
    fn capture_stop() {
        let directory = tempdir().expect("create capture test directory");
        let recorder = CaptureRecorder::new();
        let session = start_capture(&directory, &recorder);
        let request_body = b"<login><username>private-user</username><password>private-password-931</password></login>";
        let response_body = b"<response><session>private-session-822</session></response>";
        let request_id = recorder.http_request("login", "POST", vec![("content-type".into(), "text/xml".into())], request_body);
        recorder.http_response(
            request_id,
            Some(200),
            vec![("content-type".into(), "text/xml".into())],
            Duration::from_millis(11),
            "complete",
            Some(response_body),
        );
        recorder.lifecycle("login_completed");
        let datagram = parse_hex_fixture(vectors::CHAT_FROM_SIMULATOR).expect("parse independent chat vector");
        let local: SocketAddr = "127.0.0.1:50000".parse().expect("local endpoint");
        let remote: SocketAddr = "198.51.100.44:13000".parse().expect("simulator endpoint");
        recorder.udp(512347, "in", local, remote, &datagram);

        let capture = finish_capture(session);
        let (manifest, events, payloads, decoded_packets) = check_capture(&capture);
        assert_eq!(manifest.state, CaptureState::Complete);
        assert_eq!(decoded_packets, 1);
        assert!(payloads.windows(request_body.len()).any(|part| part == request_body));
        assert!(payloads.windows(response_body.len()).any(|part| part == response_body));
        assert!(events.iter().any(|event| matches!(event.kind, CaptureEventKind::LoginCompleted)));

        let request = events
            .iter()
            .find_map(|event| match &event.kind {
                CaptureEventKind::HttpRequest { request_id, .. } => Some(*request_id),
                _ => None,
            })
            .expect("captured HTTP request");
        assert!(
            events.iter().any(|event| matches!(event.kind, CaptureEventKind::HttpResponse { request_id, .. } if request_id == request))
        );
        let lludp_receive =
            events.iter().find(|event| matches!(event.kind, CaptureEventKind::LLUdpReceive { .. })).expect("captured LLUDP receive");
        assert_eq!(payload(&payloads, lludp_receive), datagram);
    }

    #[test]
    fn oversized_event_is_omitted_and_marks_capture_incomplete() {
        let directory = tempdir().expect("create capture test directory");
        let recorder = CaptureRecorder::new();
        let session = start_capture(&directory, &recorder);
        recorder.http_request("test", "GET", vec![("x-large".into(), "x".repeat(MAX_CAPTURE_EVENT_LINE_BYTES))], &[]);

        let summary = session.finish_recording(&CapturePublication::default());
        assert_eq!(summary.state, CaptureState::Incomplete);
        assert_eq!(summary.error.as_deref(), Some("Capture event exceeds the per-event line size limit."));

        let path = summary.capture_path.expect("incomplete capture archive");
        let (manifest, events, _, _) = check_capture(&path);
        assert_eq!(manifest.state, CaptureState::Incomplete);
        assert!(events.is_empty());
    }

    #[test]
    fn large_http_body_does_not_count_toward_event_line_limit() {
        let directory = tempdir().expect("create capture test directory");
        let recorder = CaptureRecorder::new();
        let session = start_capture(&directory, &recorder);
        let body = vec![b'x'; MAX_CAPTURE_EVENT_LINE_BYTES + 1];
        recorder.http_request("test", "POST", Vec::new(), &body);

        let capture = finish_capture(session);
        let (_, events, payloads, _) = check_capture(&capture);

        assert_eq!(events.len(), 1);
        assert_eq!(payload(&payloads, &events[0]), body);
        assert!(archive_entry(&capture, "events.jsonl").len() <= MAX_CAPTURE_EVENT_LINE_BYTES);
    }

    #[tokio::test]
    async fn capability_endpoints_are_captured() {
        let directory = tempdir().expect("create capture test directory");
        let recorder = CaptureRecorder::new();
        let session = start_capture(&directory, &recorder);
        let capability_server = LocalServer::start(vec![
            response(200, "OK", "application/llsd+xml", None, "<llsd><integer>9</integer></llsd>"),
            response(200, "OK", "application/octet-stream", None, "named"),
            response(200, "OK", "application/octet-stream", None, "unsafe"),
            response(200, "OK", "application/octet-stream", None, "manual"),
        ]);
        let named_url = capability_server.url("/named-secret-token");
        let unsafe_url = capability_server.url("/unsafe-secret-token");
        let seed_body =
            format!("<llsd><map><key>TestCapability</key><uri>{named_url}</uri><key>bad/name</key><uri>{unsafe_url}</uri></map></llsd>");
        let seed_server = LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, &seed_body)]);
        let client = capabilities::Client::new_with_capture(seed_server.url("/seed"), Some(recorder.clone())).expect("capability client");
        let capabilities = client.request_capabilities(&["TestCapability", "bad/name"]).await.expect("capability discovery");

        let named = capabilities.get("TestCapability").expect("named capability").with_query_pair("request", "private-value");
        let unsafe_name = capabilities.get("bad/name").expect("capability with non-label name");
        assert_eq!(client.post::<_, Value>(&named, &Value::Undefined).await.expect("named POST"), Value::Integer(9));
        assert_eq!(client.get_bytes(&named).await.expect("named request"), b"named");
        assert_eq!(client.get_bytes(unsafe_name).await.expect("unsafe-name request"), b"unsafe");
        let manual = CapabilityUrl::parse(capability_server.url("/manual-secret-token").as_str()).expect("manual URL handle");
        assert_eq!(client.get_bytes(&manual).await.expect("manual request"), b"manual");
        client.close_session();

        assert_eq!(seed_server.finish().len(), 1);
        assert_eq!(capability_server.finish().len(), 4);
        let capture = finish_capture(session);
        let (_, events, _, _) = check_capture(&capture);
        let endpoints = events
            .iter()
            .filter_map(|event| match &event.kind {
                CaptureEventKind::HttpRequest { endpoint, .. } => Some(endpoint.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(endpoints, ["seed_capability", "TestCapability", "TestCapability", "capability_get", "capability_get"]);
        assert!(endpoints.iter().all(|endpoint| !endpoint.contains("secret-token")));
        assert!(endpoints.iter().all(|endpoint| !endpoint.contains("127.0.0.1")));
    }

    #[test]
    fn capture_interrupt() {
        let directory = tempdir().expect("create capture test directory");
        let recorder = CaptureRecorder::new();
        let session = start_capture(&directory, &recorder);
        let path = session.path().to_path_buf();
        recorder.lifecycle("login_completed");

        let summary = session.interrupt();
        assert_eq!(summary.state, CaptureState::Interrupted);
        assert_eq!(summary.capture_path.as_deref(), Some(path.as_path()));
        let (manifest, events, _, _) = check_capture(&path);
        assert_eq!(manifest.state, CaptureState::Interrupted);
        assert!(events.iter().any(|event| matches!(event.kind, CaptureEventKind::LoginCompleted)));
    }

    #[test]
    fn http_errors() {
        let directory = tempdir().expect("create capture test directory");
        let recorder = CaptureRecorder::new();
        let session = start_capture(&directory, &recorder);
        let request_id = recorder.http_request(
            "capability_get",
            "GET",
            vec![("accept".into(), "application/llsd+xml".into()), ("accept".into(), "application/llsd+xml".into())],
            &[],
        );
        recorder.http_response(request_id, None, Vec::new(), Duration::from_millis(3), "send_error", None);

        let capture = finish_capture(session);
        let (_, events, _, _) = check_capture(&capture);
        assert!(events.iter().any(|event| matches!(
        &event.kind,
        CaptureEventKind::HttpRequest { request_id: id, headers, .. }
            if *id == request_id && headers == &[("accept".into(), "application/llsd+xml".into()), ("accept".into(), "application/llsd+xml".into())]
    )));
        assert!(events.iter().any(|event| matches!(event.kind, CaptureEventKind::HttpError { request_id: id, .. } if id == request_id)));
        assert!(!events.iter().any(|event| matches!(event.kind, CaptureEventKind::HttpResponse { .. })));
    }

    #[tokio::test]
    async fn pending_http() {
        let directory = tempdir().expect("create capture test directory");
        let recorder = CaptureRecorder::new();
        let session = start_capture(&directory, &recorder);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local HTTP server");
        let address = listener.local_addr().expect("read local HTTP address");
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept local HTTP request");
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut chunk).expect("read local HTTP request");
                assert_ne!(count, 0, "client closed before sending HTTP headers");
                request.extend_from_slice(&chunk[..count]);
            }
            accepted_tx.send(()).expect("signal accepted request");
            release_rx.recv_timeout(Duration::from_secs(10)).expect("release held HTTP response");
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").expect("send local HTTP response");
        });
        let client = Client::new(Some(Duration::from_secs(5)), true, None).expect("build HTTP client").with_capture(Some(recorder));
        let request_url = format!("http://{address}/capability");
        let request = spawn(async move { client.get(&Url::parse(&request_url).expect("local HTTP URL")).await });

        spawn_blocking(move || accepted_rx.recv_timeout(Duration::from_secs(5)).expect("HTTP request reached local server"))
            .await
            .expect("wait for local HTTP server");
        let capture = finish_capture(session);
        release_tx.send(()).expect("release local HTTP response");
        let response = timeout(Duration::from_secs(5), request)
            .await
            .expect("HTTP request should finish")
            .expect("HTTP task should finish")
            .expect("receive held HTTP response");
        assert_eq!(response.body, b"ok");
        server.join().expect("local HTTP server should finish");

        let (_, events, _, _) = check_capture(&capture);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].kind, CaptureEventKind::HttpRequest { .. }));
    }
}
