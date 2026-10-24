mod outbound_message {
    pub trait Sealed {}
}

use std::{
    collections::{HashMap, HashSet},
    error::Error as StdError,
    fmt,
    future::Future,
    num::NonZeroU64,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use protocol::lludp::Message;
use protocol::messages::{AgentUpdate, ChatFromViewer, MapBlockRequest, RegionHandleRequest, RequestMultipleObjects, TeleportCancel};
use tokio::{
    runtime::Builder,
    sync::{
        mpsc::{self, OwnedPermit, Receiver, Sender},
        oneshot, watch,
    },
};

use crate::{
    LoginCredentials,
    coverage::{CoverageLoss, CoverageLossLedger, CoverageLossRecord, CoverageSnapshot, CoverageSource, CoverageWatermark},
    error::Error,
    event::{
        CLIENT_EVENT_CAPACITY, Event, EventEnvelope, EventSendOutcome, PRIMARY_TERMINAL_EVENT_CAPACITY, SimulatorId, send_client_event,
        send_failure_event,
    },
    login::{LoginError, LoginResponse, login_at_url},
    simulator::{CoordinatorError, SimulatorCoordinator, SimulatorCoordinatorOptions},
    transport::lludp::Delivery,
};

pub(crate) const COMMAND_CAPACITY: usize = 256;
pub(crate) const NETWORK_THREAD_STACK_SIZE: usize = 8 * 1024 * 1024;
pub const DEFAULT_UPSTREAM_BYTES_PER_SECOND: NonZeroU64 = NonZeroU64::new(64_000).unwrap();
const MAX_OUTBOUND_CHAT_MESSAGE_BYTES: usize = 8 * 1024 - 6 - 4 - 40 - (1 + u8::MAX as usize * 4);
const MAX_OUTBOUND_OBJECT_REQUESTS: usize = u8::MAX as usize;

#[derive(Debug)]
enum SessionError {
    Login(LoginError),
    Simulator(CoordinatorError),
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Login(error) => write!(formatter, "{error}"),
            Self::Simulator(error) => error.fmt(formatter),
        }
    }
}

impl StdError for SessionError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Login(error) => Some(error),
            Self::Simulator(error) => Some(error),
        }
    }
}

impl From<CoordinatorError> for SessionError {
    fn from(error: CoordinatorError) -> Self {
        Self::Simulator(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionOptions {
    upstream_bytes_per_second: NonZeroU64,
}

impl SessionOptions {
    pub const fn new(upstream_bytes_per_second: NonZeroU64) -> Self {
        Self { upstream_bytes_per_second }
    }

    pub const fn upstream_bytes_per_second(self) -> NonZeroU64 {
        self.upstream_bytes_per_second
    }
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self::new(DEFAULT_UPSTREAM_BYTES_PER_SECOND)
    }
}

pub trait OutboundMessage: outbound_message::Sealed + Into<Message> {}

pub struct MfaChallenge {
    commands: Sender<Command>,
    completed: AtomicBool,
}

impl MfaChallenge {
    pub(crate) fn new(commands: Sender<Command>) -> Self {
        Self { commands, completed: AtomicBool::new(false) }
    }

    pub fn complete(&self, code: &str) -> Result<(), Error> {
        let code = code.trim();
        if code.is_empty() {
            return Err(Error::EmptyMfaCode);
        }
        if self.completed.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return Err(Error::MfaChallengeCompleted);
        }
        match try_send_command(&self.commands, Command::SubmitMfaCode(code.to_owned()), Error::LoginEnded) {
            Err(error @ Error::CommandQueueFull) => {
                self.completed.store(false, Ordering::Release);
                Err(error)
            }
            result => result,
        }
    }
}

impl fmt::Debug for MfaChallenge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MfaChallenge")
    }
}

pub struct Session {
    handle: SessionHandle,
    events: Receiver<EventEnvelope>,
    coverage_losses: CoverageLossLedger,
    coverage_changed: watch::Receiver<u64>,
    pending_event: Option<EventEnvelope>,
    surfaced_coverage: HashMap<(SimulatorId, CoverageSource), u64>,
    surfaced_overflow_generation: u64,
}

impl Session {
    #[cfg(test)]
    fn new_for_test(handle: SessionHandle, events: Receiver<EventEnvelope>) -> Self {
        Self::new_for_test_with_coverage(handle, events, CoverageLossLedger::new())
    }

    #[cfg(test)]
    fn new_for_test_with_coverage(handle: SessionHandle, events: Receiver<EventEnvelope>, coverage_losses: CoverageLossLedger) -> Self {
        let coverage_changed = coverage_losses.subscribe();
        Self {
            handle,
            events,
            coverage_losses,
            coverage_changed,
            pending_event: None,
            surfaced_coverage: HashMap::new(),
            surfaced_overflow_generation: 0,
        }
    }

    pub fn start(credentials: LoginCredentials) -> Result<Self, Error> {
        Self::start_with_options(credentials, SessionOptions::default())
    }

    pub fn start_with_options(credentials: LoginCredentials, options: SessionOptions) -> Result<Self, Error> {
        let login_url = credentials.grid.login_url().to_owned();
        Self::start_at(credentials, options, login_url)
    }

    #[cfg(feature = "testing")]
    pub(crate) fn start_with_login_url_for_test(credentials: LoginCredentials, login_url: String) -> Result<Self, Error> {
        Self::start_at(credentials, SessionOptions::default(), login_url)
    }

    fn start_at(credentials: LoginCredentials, options: SessionOptions, login_url: String) -> Result<Self, Error> {
        let (command_sender, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let coverage_losses = CoverageLossLedger::new();
        let coverage_changed = coverage_losses.subscribe();
        let terminal_event_permit = event_sender.clone().try_reserve_owned().expect("new event channel has capacity");
        let (disconnect_sender, disconnect_receiver) = watch::channel(false);
        let (worker_finished_sender, worker_finished) = watch::channel(false);
        tracing::info!(lifecycle = "logging_in", "network lifecycle changed");
        let failure_sender = event_sender.clone();
        let mut failure_shutdown = disconnect_receiver.clone();
        let worker_command_sender = command_sender.clone();
        let worker_coverage_losses = coverage_losses.clone();

        thread::Builder::new()
            .name("network".into())
            .stack_size(NETWORK_THREAD_STACK_SIZE)
            .spawn(move || {
                let runtime = match Builder::new_multi_thread().enable_all().build() {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let error = format!("Could not start the network runtime: {error}");
                        tracing::info!(lifecycle = "failed", "network lifecycle changed");
                        terminal_event_permit.send(EventEnvelope::new(Event::Failed(error)));
                        worker_finished_sender.send_replace(true);
                        return;
                    }
                };

                let channels = Channels {
                    commands: command_receiver,
                    command_sender: worker_command_sender,
                    events: event_sender,
                    terminal_event_permit,
                    disconnect_requested: disconnect_receiver,
                    coverage_losses: worker_coverage_losses,
                };
                let result = runtime.block_on(run_client(credentials, channels, login_url, options));
                if let Err(error) = result {
                    tracing::info!(lifecycle = "failed", "network lifecycle changed");
                    runtime.block_on(send_failure_event(&failure_sender, error.to_string(), &mut failure_shutdown));
                } else {
                    tracing::info!(lifecycle = "disconnected", "network lifecycle changed");
                }
                runtime.shutdown_background();
                worker_finished_sender.send_replace(true);
            })
            .map_err(Error::WorkerStart)?;

        Ok(Self {
            handle: SessionHandle {
                commands: command_sender,
                disconnect_requested: disconnect_sender,
                command_admission_closed: Arc::new(Mutex::new(false)),
                worker_finished,
            },
            events,
            coverage_losses,
            coverage_changed,
            pending_event: None,
            surfaced_coverage: HashMap::new(),
            surfaced_overflow_generation: 0,
        })
    }

    pub fn handle(&self) -> SessionHandle {
        self.handle.clone()
    }

    pub async fn recv_event(&mut self) -> Option<Event> {
        loop {
            if let Some(envelope) = self.pending_event.take() {
                let watermark = envelope.coverage_watermark();
                if let Some(loss) = self.next_coverage_loss(watermark) {
                    self.pending_event = Some(envelope);
                    return Some(Event::CoverageLost { loss });
                }
                if self.has_unacknowledged_coverage_loss(watermark) {
                    self.pending_event = Some(envelope);
                    if self.coverage_changed.changed().await.is_err() {
                        return self.pending_event.take().map(EventEnvelope::into_event);
                    }
                    continue;
                }
                return Some(envelope.into_event());
            }

            match self.events.try_recv() {
                Ok(envelope) => {
                    self.pending_event = Some(envelope);
                    continue;
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    if let Some(loss) = self.next_coverage_loss(None) {
                        return Some(Event::CoverageLost { loss });
                    }
                    return None;
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
            }

            if let Some(loss) = self.next_coverage_loss(None) {
                return Some(Event::CoverageLost { loss });
            }

            tokio::select! {
                biased;
                envelope = self.events.recv() => match envelope {
                    Some(envelope) => self.pending_event = Some(envelope),
                    None => {
                        if let Some(loss) = self.next_coverage_loss(None) {
                            return Some(Event::CoverageLost { loss });
                        }
                        return None;
                    }
                },
                changed = self.coverage_changed.changed() => {
                    if changed.is_ok()
                        && let Some(loss) = self.next_coverage_loss(None)
                    {
                        return Some(Event::CoverageLost { loss });
                    }
                }
            }
        }
    }

    pub fn coverage_losses(&self) -> CoverageSnapshot {
        self.coverage_losses.snapshot()
    }

    pub fn acknowledge_coverage_loss(&mut self, watermark: CoverageWatermark) {
        let scope = (watermark.simulator_id, watermark.source);
        if self.surfaced_coverage.get(&scope).is_some_and(|generation| watermark.generation <= *generation) {
            self.coverage_losses.acknowledge(watermark);
            self.prune_surfaced_coverage();
        }
    }

    pub fn acknowledge_coverage_overflow(&mut self, generation: u64) {
        if generation > 0 && generation <= self.surfaced_overflow_generation {
            self.coverage_losses.acknowledge_overflow(generation);
            self.prune_surfaced_coverage();
        }
    }

    fn next_coverage_loss(&mut self, watermark: Option<CoverageWatermark>) -> Option<CoverageLoss> {
        let snapshot = self.coverage_losses.snapshot();
        if let Some(latest_generation) = snapshot.overflow_generation {
            let first_pending = snapshot.acknowledged_overflow_generation.saturating_add(1);
            let eligible_generation = watermark.map_or(Some(latest_generation), |watermark| {
                (watermark.overflow_generation >= first_pending).then_some(latest_generation.min(watermark.overflow_generation))
            });
            if let Some(generation) = eligible_generation
                && generation > self.surfaced_overflow_generation
            {
                self.surfaced_overflow_generation = generation;
                return Some(CoverageLoss::SessionOverflow { generation });
            }
        }

        for loss in snapshot.losses {
            let CoverageLoss::Source(record) = loss else { continue };
            let key = (record.watermark.simulator_id, record.watermark.source);
            let surfaced = self.surfaced_coverage.get(&key).copied().unwrap_or_default();
            let eligible_generation = watermark.map_or(Some(record.watermark.generation), |watermark| {
                (watermark.simulator_id == key.0 && watermark.source == key.1 && watermark.generation >= record.pending_from_generation)
                    .then_some(record.watermark.generation.min(watermark.generation))
            });
            let Some(generation) = eligible_generation else { continue };
            if generation <= surfaced {
                continue;
            }
            self.surfaced_coverage.insert(key, generation);
            let overflow_generation = watermark.map_or(record.watermark.overflow_generation, |watermark| {
                record.watermark.overflow_generation.min(watermark.overflow_generation)
            });
            return Some(CoverageLoss::Source(CoverageLossRecord {
                watermark: CoverageWatermark { generation, overflow_generation, ..record.watermark },
                ..record
            }));
        }
        None
    }

    fn has_unacknowledged_coverage_loss(&self, watermark: Option<CoverageWatermark>) -> bool {
        let snapshot = self.coverage_losses.snapshot();
        if let Some(generation) = snapshot.overflow_generation {
            let first_pending = snapshot.acknowledged_overflow_generation.saturating_add(1);
            if watermark.is_none_or(|watermark| watermark.overflow_generation >= first_pending)
                && (snapshot.overflow_exhausted || generation > snapshot.acknowledged_overflow_generation)
            {
                return true;
            }
        }

        snapshot.losses.iter().any(|loss| {
            let CoverageLoss::Source(record) = loss else { return false };
            watermark.is_none_or(|watermark| {
                watermark.simulator_id == record.watermark.simulator_id
                    && watermark.source == record.watermark.source
                    && watermark.generation >= record.pending_from_generation
            })
        })
    }

    fn prune_surfaced_coverage(&mut self) {
        let pending = self.coverage_losses.snapshot();
        let pending_sources = pending
            .losses
            .iter()
            .filter_map(|loss| match loss {
                CoverageLoss::Source(record) => Some((record.watermark.simulator_id, record.watermark.source)),
                CoverageLoss::SessionOverflow { .. } => None,
            })
            .collect::<HashSet<_>>();
        self.surfaced_coverage.retain(|scope, _| pending_sources.contains(scope));
        if pending.overflow_generation.is_none() {
            self.surfaced_overflow_generation = 0;
        }
    }

    pub fn send(&self, message: impl OutboundMessage, delivery: Delivery) -> Result<(), Error> {
        self.handle.send(message, delivery)
    }

    pub async fn send_to(&self, simulator_id: SimulatorId, message: impl OutboundMessage, delivery: Delivery) -> Result<(), Error> {
        self.handle.send_to(simulator_id, message, delivery).await
    }

    pub async fn disconnect(&self) {
        self.handle.disconnect().await;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.handle.request_disconnect();
    }
}

#[derive(Clone)]
pub struct SessionHandle {
    commands: Sender<Command>,
    disconnect_requested: watch::Sender<bool>,
    command_admission_closed: Arc<Mutex<bool>>,
    worker_finished: watch::Receiver<bool>,
}

impl SessionHandle {
    fn request_disconnect(&self) {
        let mut admission_closed = self.command_admission_closed.lock().unwrap_or_else(|error| error.into_inner());
        *admission_closed = true;
        if !*self.worker_finished.borrow() && !*self.disconnect_requested.borrow() {
            tracing::info!(lifecycle = "disconnecting", "network lifecycle changed");
        }
        self.disconnect_requested.send_replace(true);
    }

    pub fn send(&self, message: impl OutboundMessage, delivery: Delivery) -> Result<(), Error> {
        let message = message.into();
        validate_outbound_message(&message)?;
        let admission_closed = self.command_admission_closed.lock().unwrap_or_else(|error| error.into_inner());
        if *admission_closed || *self.disconnect_requested.borrow() || *self.worker_finished.borrow() {
            return Err(Error::SessionClosed);
        }
        try_send_command(&self.commands, Command::SendMessage { message, delivery }, Error::SimulatorConnectionClosed)
    }

    pub async fn send_to(&self, simulator_id: SimulatorId, message: impl OutboundMessage, delivery: Delivery) -> Result<(), Error> {
        let message = message.into();
        validate_outbound_message(&message)?;
        let mut disconnect_requested = self.disconnect_requested.subscribe();
        let (completed, response) = oneshot::channel();
        {
            let admission_closed = self.command_admission_closed.lock().unwrap_or_else(|error| error.into_inner());
            if *admission_closed || *self.disconnect_requested.borrow() || *self.worker_finished.borrow() {
                return Err(Error::SessionClosed);
            }
            self.commands.try_send(Command::SendTo { simulator_id, message, delivery, completed }).map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => Error::SimulatorCommandQueueFull,
                mpsc::error::TrySendError::Closed(_) => Error::SessionClosed,
            })?;
        }
        tokio::select! {
            biased;
            _ = disconnect_requested.changed() => Err(Error::SessionClosed),
            result = response => result.map_err(|_| {
                if *self.disconnect_requested.borrow() || *self.worker_finished.borrow() || self.commands.is_closed() {
                    Error::SessionClosed
                } else {
                    Error::SimulatorConnectionClosed
                }
            })?,
        }
    }

    pub async fn disconnect(&self) {
        self.request_disconnect();
        let mut worker_finished = self.worker_finished.clone();
        if !*worker_finished.borrow()
            && let Err(error) = worker_finished.wait_for(|finished| *finished).await
        {
            tracing::debug!(%error, "network worker finished while disconnect was waiting");
        }
    }
}

pub(crate) enum Command {
    SubmitMfaCode(String),
    SendMessage { message: Message, delivery: Delivery },
    SendTo { simulator_id: SimulatorId, message: Message, delivery: Delivery, completed: oneshot::Sender<Result<(), Error>> },
}

struct Channels {
    commands: Receiver<Command>,
    command_sender: Sender<Command>,
    events: Sender<EventEnvelope>,
    terminal_event_permit: OwnedPermit<EventEnvelope>,
    disconnect_requested: watch::Receiver<bool>,
    coverage_losses: CoverageLossLedger,
}

impl outbound_message::Sealed for TeleportCancel {}

impl OutboundMessage for TeleportCancel {}

impl outbound_message::Sealed for AgentUpdate {}

impl OutboundMessage for AgentUpdate {}

impl outbound_message::Sealed for ChatFromViewer {}

impl OutboundMessage for ChatFromViewer {}

impl outbound_message::Sealed for MapBlockRequest {}

impl OutboundMessage for MapBlockRequest {}

impl outbound_message::Sealed for RegionHandleRequest {}

impl OutboundMessage for RegionHandleRequest {}

impl outbound_message::Sealed for RequestMultipleObjects {}

impl OutboundMessage for RequestMultipleObjects {}

fn validate_outbound_message(message: &Message) -> Result<(), Error> {
    match message {
        Message::ChatFromViewer(message) if message.chat_data.message.capacity() > MAX_OUTBOUND_CHAT_MESSAGE_BYTES => {
            Err(Error::OutboundMessageTooLarge)
        }
        Message::RequestMultipleObjects(message) if message.objects.capacity() > MAX_OUTBOUND_OBJECT_REQUESTS => {
            Err(Error::OutboundMessageTooLarge)
        }
        _ => Ok(()),
    }
}

fn try_send_command(commands: &Sender<Command>, command: Command, closed: Error) -> Result<(), Error> {
    commands.try_send(command).map_err(|error| match error {
        mpsc::error::TrySendError::Full(_) => Error::CommandQueueFull,
        mpsc::error::TrySendError::Closed(_) => closed,
    })
}

#[tracing::instrument(name = "network.session", level = "info", skip_all, fields(grid = ?credentials.grid))]
async fn run_client(
    credentials: LoginCredentials,
    channels: Channels,
    login_url: String,
    options: SessionOptions,
) -> Result<(), SessionError> {
    let Channels { mut commands, command_sender, events, terminal_event_permit, mut disconnect_requested, coverage_losses } = channels;
    let mut mfa_token = String::new();
    let mut mfa_hash = credentials.mfa_hash.clone();
    let (session, login_data) = loop {
        let response = match wait_for_login(login_at_url(&credentials, &mfa_token, &mfa_hash, &login_url), &mut disconnect_requested).await
        {
            Some(Ok(response)) => response,
            Some(Err(error)) => {
                tracing::warn!("Login failed");
                return Err(SessionError::Login(error));
            }
            None => return Ok(()),
        };
        match response {
            LoginResponse::Authenticated { session, mfa_hash: returned_hash, data } => {
                tracing::info!(target: "network::client", "Login authenticated");
                tracing::info!(lifecycle = "connecting", "network lifecycle changed");
                let remembered_hash = returned_hash.or_else(|| (!mfa_hash.is_empty()).then(|| mfa_hash.clone()));
                if !matches!(
                    send_client_event(&events, Event::LoginAccepted { mfa_hash: remembered_hash }, &mut disconnect_requested).await,
                    EventSendOutcome::Sent
                ) {
                    return Ok(());
                }
                break (*session, data);
            }
            LoginResponse::MfaChallenge => {
                tracing::info!("Login requires MFA");
                mfa_token.clear();
                mfa_hash.clear();
                let challenge = MfaChallenge::new(command_sender.clone());
                if !matches!(
                    send_client_event(&events, Event::MfaRequired { challenge }, &mut disconnect_requested).await,
                    EventSendOutcome::Sent
                ) {
                    return Ok(());
                }

                loop {
                    let command = tokio::select! {
                        biased;
                        _ = disconnect_requested.changed() => return Ok(()),
                        command = commands.recv() => command,
                    };
                    match command {
                        Some(Command::SubmitMfaCode(code)) if !code.trim().is_empty() => {
                            mfa_token = code.trim().to_owned();
                            break;
                        }
                        Some(Command::SubmitMfaCode(_)) => {}
                        Some(Command::SendMessage { .. }) => {
                            if !matches!(
                                send_client_event(
                                    &events,
                                    Event::SendFailed("Sign in before sending a simulator message.".into()),
                                    &mut disconnect_requested,
                                )
                                .await,
                                EventSendOutcome::Sent
                            ) {
                                return Ok(());
                            }
                        }
                        Some(Command::SendTo { completed, .. }) => {
                            if let Err(error) = completed.send(Err(Error::SimulatorNotReady)) {
                                drop(error);
                            }
                        }
                        None => return Ok(()),
                    }
                }
            }
        }
    };
    drop(mfa_token);
    drop(mfa_hash);
    drop(credentials);

    let Some(simulator) = SimulatorCoordinator::connect(
        session,
        login_data,
        SimulatorCoordinatorOptions {
            coverage_losses,
            session: options,
            startup_disconnect: Some(disconnect_requested.clone()),
            root_transport: None,
        },
    )
    .await?
    else {
        return Ok(());
    };
    simulator.run(commands, events, disconnect_requested, terminal_event_permit).await.map_err(SessionError::from)
}

async fn wait_for_login<F: Future>(login: F, disconnect_requested: &mut watch::Receiver<bool>) -> Option<F::Output> {
    if *disconnect_requested.borrow() {
        return None;
    }

    tokio::select! {
        biased;
        _ = disconnect_requested.changed() => None,
        response = login => Some(response),
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
