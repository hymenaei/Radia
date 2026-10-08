use std::{
    fmt,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

use protocol::messages::{AgentUpdate, ChatFromViewer, MapBlockRequest, RegionHandleRequest, RequestMultipleObjects};
use protocol::udp::Message;
use tokio::sync::{
    mpsc::{self, OwnedPermit, Receiver, Sender},
    oneshot, watch,
};

use crate::{
    LoginCredentials,
    capture::{CaptureConfig, CapturePublication, CaptureRecorder, CaptureSession, CaptureState, CaptureStatus, CaptureSummary},
    circuit::Delivery,
    error::Error,
    event::{
        CLIENT_EVENT_CAPACITY, Event, EventEnvelope, EventSendOutcome, PRIMARY_TERMINAL_EVENT_CAPACITY, SimulatorId, send_client_event,
        send_failure_event,
    },
    login::LoginResponse,
    simulator::SimulatorConnection,
    status::{NetworkLifecycle, NetworkStatusHandle, NetworkStatusSnapshot},
};

pub(crate) const COMMAND_CAPACITY: usize = 256;
const NETWORK_THREAD_STACK_SIZE: usize = 8 * 1024 * 1024;
const MAX_OUTBOUND_CHAT_MESSAGE_BYTES: usize = 8 * 1024 - 6 - 4 - 40 - (1 + u8::MAX as usize * 4);
const MAX_OUTBOUND_OBJECT_REQUESTS: usize = u8::MAX as usize;

mod outbound_message {
    pub trait Sealed {}
}

/// A message that may be sent from the viewer to a simulator.
///
/// This sealed trait admits only the outbound messages supported by the network layer.
///
/// ```compile_fail
/// struct Unsupported;
///
/// impl From<Unsupported> for protocol::udp::Message {
///     fn from(_: Unsupported) -> Self {
///         unreachable!()
///     }
/// }
///
/// impl network::OutboundMessage for Unsupported {}
/// ```
pub trait OutboundMessage: outbound_message::Sealed + Into<Message> {}

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

/// A login continuation for an authenticator challenge.
pub struct MfaChallenge {
    commands: Sender<Command>,
    completed: AtomicBool,
}

impl MfaChallenge {
    pub(crate) fn new(commands: Sender<Command>) -> Self {
        Self { commands, completed: AtomicBool::new(false) }
    }

    /// Submits a nonblank code once. Disconnect can still cancel authentication.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty code, a completed challenge, or a closed or full command queue.
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

/// Owns the network worker's bounded event stream and session controls.
///
/// Events arrive in stream order. Dropping the session requests shutdown; [`disconnect`](Self::disconnect) waits for it.
pub struct Session {
    handle: SessionHandle,
    events: Receiver<EventEnvelope>,
}

impl Session {
    /// Starts login on the selected grid and returns before authentication completes.
    ///
    /// # Errors
    ///
    /// Returns an error if the network worker thread cannot start.
    pub fn start(credentials: LoginCredentials) -> Result<Self, Error> {
        Self::start_inner(credentials, None)
    }

    /// # Errors
    ///
    /// Returns an error if the worker or initial capture recording cannot start.
    pub fn start_with_capture(credentials: LoginCredentials, config: CaptureConfig) -> Result<Self, Error> {
        Self::start_inner(credentials, Some(config))
    }

    fn start_inner(credentials: LoginCredentials, capture_config: Option<CaptureConfig>) -> Result<Self, Error> {
        let (command_sender, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let terminal_event_permit = event_sender.clone().try_reserve_owned().expect("new event channel has capacity");
        let (disconnect_sender, disconnect_receiver) = watch::channel(false);
        let (worker_finished_sender, worker_finished) = watch::channel(false);
        let (capture_commands, capture_command_receiver) = mpsc::channel(8);
        let capture = CaptureRecorder::new();
        let client_capture = capture.clone();
        let status = NetworkStatusHandle::new();
        status.set_lifecycle(NetworkLifecycle::LoggingIn);
        let worker_status = status.clone();
        let failure_sender = event_sender.clone();
        let mut failure_shutdown = disconnect_receiver.clone();
        let worker_command_sender = command_sender.clone();

        thread::Builder::new()
            .name("network".into())
            .stack_size(NETWORK_THREAD_STACK_SIZE)
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let error = format!("Could not start the network runtime: {error}");
                        worker_status.record_error(&error);
                        worker_status.set_lifecycle(NetworkLifecycle::Failed);
                        let _ = failure_sender.try_send(EventEnvelope::new(Event::Failed(error)));
                        worker_finished_sender.send_replace(true);
                        return;
                    }
                };

                let capture_session = match capture_config {
                    Some(config) => match CaptureSession::start(config, capture.clone()) {
                        Ok(session) => Some(session),
                        Err(error) => {
                            worker_status.record_error(&error);
                            worker_status.set_lifecycle(NetworkLifecycle::Failed);
                            runtime.block_on(send_failure_event(&failure_sender, error, &mut failure_shutdown));
                            worker_finished_sender.send_replace(true);
                            return;
                        }
                    },
                    None => None,
                };
                if capture_session.is_some() {
                    capture.lifecycle("capture_started");
                }
                let channels = Channels {
                    commands: command_receiver,
                    command_sender: worker_command_sender,
                    events: event_sender,
                    terminal_event_permit,
                    disconnect_requested: disconnect_receiver,
                };
                let shutdown_requested = channels.disconnect_requested.clone();
                let events = channels.events.clone();
                let login_url = credentials.grid.login_url().to_owned();
                let run = run_client(credentials, channels, Some(capture.clone()), worker_status.clone(), login_url);
                let result = runtime.block_on(run_with_capture(
                    run,
                    events,
                    shutdown_requested,
                    capture.clone(),
                    capture_session,
                    capture_command_receiver,
                ));
                if let Err(error) = result {
                    worker_status.record_error(&error);
                    worker_status.set_lifecycle(NetworkLifecycle::Failed);
                    runtime.block_on(send_failure_event(&failure_sender, error, &mut failure_shutdown));
                } else {
                    worker_status.set_lifecycle(NetworkLifecycle::Disconnected);
                }
                runtime.shutdown_background();
                worker_finished_sender.send_replace(true);
            })
            .map_err(Error::WorkerStart)?;

        Ok(Self {
            handle: SessionHandle {
                commands: command_sender,
                disconnect_requested: disconnect_sender,
                worker_finished,
                capture_commands,
                capture: client_capture,
                status,
            },
            events,
        })
    }

    pub fn handle(&self) -> SessionHandle {
        self.handle.clone()
    }

    /// Receives the next event, or `None` after the worker closes the stream.
    pub async fn recv_event(&mut self) -> Option<Event> {
        self.events.recv().await.map(EventEnvelope::into_event)
    }

    /// Starts network capture and waits for the recording file to open.
    ///
    /// # Errors
    ///
    /// Returns an error if capture is already active, startup fails, or the session stops before startup completes.
    pub async fn start_capture(&self, config: CaptureConfig) -> Result<(), Error> {
        self.handle.start_capture(config).await
    }

    pub fn capture_status(&self) -> Option<CaptureStatus> {
        self.handle.capture_status()
    }

    pub fn network_status(&self) -> NetworkStatusSnapshot {
        self.handle.network_status()
    }

    /// Returns the primary simulator endpoint. Neighbor endpoints arrive in lifecycle events.
    pub fn current_simulator(&self) -> Option<SocketAddr> {
        self.handle.network_status().simulator_endpoint
    }

    /// Stops capture and waits for the output file to finish.
    ///
    /// # Errors
    ///
    /// Returns an error if no capture is active, finalization fails, or the session stops before finalization completes.
    pub async fn stop_capture(&self) -> Result<std::path::PathBuf, Error> {
        self.handle.stop_capture().await
    }

    /// Queues a message for the primary simulator. See [`SessionHandle::send`] for size limits and errors.
    pub fn send(&self, message: impl OutboundMessage, delivery: Delivery) -> Result<(), Error> {
        self.handle.send(message, delivery)
    }

    /// Queues a message for one simulator. See [`SessionHandle::send_to`] for delivery and error semantics.
    pub async fn send_to(&self, simulator_id: SimulatorId, message: impl OutboundMessage, delivery: Delivery) -> Result<(), Error> {
        self.handle.send_to(simulator_id, message, delivery).await
    }

    /// Requests shutdown and waits until the network worker exits.
    pub async fn disconnect(&self) {
        self.handle.disconnect().await;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.handle.request_disconnect();
    }
}

/// Cloneable send and lifecycle controls for a session.
///
/// Clones share one bounded command queue and disconnect signal.
#[derive(Clone)]
pub struct SessionHandle {
    commands: Sender<Command>,
    disconnect_requested: watch::Sender<bool>,
    worker_finished: watch::Receiver<bool>,
    capture_commands: Sender<CaptureCommand>,
    capture: CaptureRecorder,
    status: NetworkStatusHandle,
}

impl SessionHandle {
    fn request_disconnect(&self) {
        if !*self.worker_finished.borrow() {
            tracing::info!("Network disconnect requested");
            self.status.set_lifecycle(NetworkLifecycle::Disconnecting);
        }
        self.disconnect_requested.send_replace(true);
    }

    pub async fn start_capture(&self, config: CaptureConfig) -> Result<(), Error> {
        let (completed, response) = oneshot::channel();
        self.capture_commands.send(CaptureCommand::Start { config, completed }).await.map_err(|_| Error::SessionClosed)?;
        response.await.map_err(|_| Error::CaptureStartCanceled)?.map_err(Error::Capture)
    }

    pub fn capture_status(&self) -> Option<CaptureStatus> {
        self.capture.status()
    }

    pub fn network_status(&self) -> NetworkStatusSnapshot {
        self.status.snapshot()
    }

    pub async fn stop_capture(&self) -> Result<std::path::PathBuf, Error> {
        let (completed, response) = oneshot::channel();
        self.capture_commands
            .send(CaptureCommand::Stop { completed, publication: CapturePublication::default() })
            .await
            .map_err(|_| Error::SessionClosed)?;
        response.await.map_err(|_| Error::CaptureStopCanceled)?.map_err(Error::Capture)
    }

    /// Queues a message for the primary simulator. A later send failure arrives as [`Event::SendFailed`].
    ///
    /// Chat text is limited to 7,121 allocated bytes; `RequestMultipleObjects` is limited to 255 allocated entries.
    ///
    /// # Errors
    ///
    /// Returns [`Error::OutboundMessageTooLarge`] for oversized fields, or a queue error when admission fails.
    pub fn send(&self, message: impl OutboundMessage, delivery: Delivery) -> Result<(), Error> {
        let message = message.into();
        validate_outbound_message(&message)?;
        try_send_command(&self.commands, Command::SendMessage { message, delivery }, Error::SimulatorConnectionClosed)
    }

    /// Sends a message to an active simulator connection.
    ///
    /// Success means the local UDP send succeeded, not that the remote simulator received or acknowledged it. Variable-field limits match [`Self::send`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::SimulatorUnavailable`] if the ID is inactive, [`Error::SimulatorNotReady`] during connection setup,
    /// [`Error::SimulatorCommandQueueFull`] when a send queue is full, or [`Error::OutboundMessageTooLarge`] for oversized fields.
    /// Shutdown returns [`Error::SessionClosed`]; a connection that closes during the send returns [`Error::SimulatorConnectionClosed`].
    pub async fn send_to(&self, simulator_id: SimulatorId, message: impl OutboundMessage, delivery: Delivery) -> Result<(), Error> {
        let message = message.into();
        validate_outbound_message(&message)?;
        let (completed, response) = oneshot::channel();
        self.commands.try_send(Command::SendTo { simulator_id, message, delivery, completed }).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => Error::SimulatorCommandQueueFull,
            mpsc::error::TrySendError::Closed(_) => Error::SessionClosed,
        })?;
        response.await.map_err(|_| {
            if *self.disconnect_requested.borrow() || *self.worker_finished.borrow() || self.commands.is_closed() {
                Error::SessionClosed
            } else {
                Error::SimulatorConnectionClosed
            }
        })?
    }

    /// Requests shutdown and waits until the network worker exits.
    pub async fn disconnect(&self) {
        self.request_disconnect();
        let mut worker_finished = self.worker_finished.clone();
        if !*worker_finished.borrow() {
            let _ = worker_finished.wait_for(|finished| *finished).await;
        }
    }
}

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
}

enum CaptureCommand {
    Start { config: CaptureConfig, completed: oneshot::Sender<Result<(), String>> },
    Stop { completed: oneshot::Sender<Result<std::path::PathBuf, String>>, publication: CapturePublication },
}

enum CaptureOperationResult {
    Started(Result<CaptureSession, String>),
    Stopped(Result<CaptureSummary, String>),
    Interrupted { path: std::path::PathBuf, error: Option<String> },
}

enum CaptureOperationReply {
    Start(oneshot::Sender<Result<(), String>>),
    Stop(oneshot::Sender<Result<std::path::PathBuf, String>>),
}

async fn run_with_capture<F>(
    run: F,
    events: Sender<EventEnvelope>,
    shutdown_requested: watch::Receiver<bool>,
    capture: CaptureRecorder,
    mut capture_session: Option<CaptureSession>,
    mut capture_commands: Receiver<CaptureCommand>,
) -> Result<(), String>
where
    F: Future<Output = Result<(), String>> + Send,
{
    tokio::pin!(run);

    let mut network_result = None;
    let mut operation: Option<Pin<Box<dyn Future<Output = CaptureOperationResult> + Send>>> = None;
    let mut operation_reply = None;
    let mut stop_publication: Option<CapturePublication> = None;

    loop {
        if network_result.is_some() && *shutdown_requested.borrow() {
            let stopping_capture = matches!(operation_reply.as_ref(), Some(CaptureOperationReply::Stop(_)));
            if stopping_capture && let Some(publication) = stop_publication.take() {
                publication.request_interruption();
            }
            if let Some(reply) = operation_reply.take() {
                let error = "The viewer is shutting down before the capture operation completes.".to_string();
                match reply {
                    CaptureOperationReply::Start(completed) => {
                        let _ = completed.send(Err(error));
                    }
                    CaptureOperationReply::Stop(completed) => {
                        let _ = completed.send(Err(error));
                    }
                }
            }
            if let Some(pending) = operation.take() {
                if stopping_capture {
                    drop(pending);
                } else {
                    let _ = pending.await;
                }
            }
        }

        if network_result.is_some() && operation.is_none() {
            if let Some(session) = capture_session.take() {
                capture.lifecycle("capture_interrupted");
                operation = Some(interrupt_capture(session));
            } else {
                break;
            }
        }

        tokio::select! {
            biased;
            result = async { operation.as_mut().expect("capture operation exists").as_mut().await }, if operation.is_some() => {
                operation = None;
                stop_publication = None;
                match (result, operation_reply.take()) {
                    (CaptureOperationResult::Started(Ok(session)), Some(CaptureOperationReply::Start(completed))) => {
                        capture_session = Some(session);
                        let _ = completed.send(Ok(()));
                    }
                    (CaptureOperationResult::Started(Err(error)), Some(CaptureOperationReply::Start(completed))) => {
                        let _ = completed.send(Err(error));
                    }
                    (CaptureOperationResult::Stopped(Ok(summary)), reply) => {
                        let completion = match (summary.error.clone(), summary.capture_path.clone()) {
                            (Some(error), _) => Err(error),
                            (None, Some(path)) => Ok(path),
                            (None, None) => Err("Capture finalization did not produce an export.".into()),
                        };
                        if let Some(CaptureOperationReply::Stop(completed)) = reply {
                            let _ = completed.send(completion);
                        }
                    }
                    (CaptureOperationResult::Stopped(Err(error)), Some(CaptureOperationReply::Stop(completed))) => {
                        let _ = completed.send(Err(error));
                    }
                    (CaptureOperationResult::Interrupted { path, error }, _) => {
                        let _ = events.try_send(EventEnvelope::new(Event::CaptureInterrupted { path, error }));
                    }
                    (CaptureOperationResult::Started(_), _) | (CaptureOperationResult::Stopped(_), _) => {}
                }
            }
            command = capture_commands.recv(), if operation.is_none() => {
                match command {
                    Some(CaptureCommand::Start { config, completed }) if network_result.is_none() => {
                        if capture_session.is_some() {
                            let _ = completed.send(Err("A capture session is already active.".into()));
                            continue;
                        }
                        let capture = capture.clone();
                        operation_reply = Some(CaptureOperationReply::Start(completed));
                        operation = Some(start_capture(config, capture));
                    }
                    Some(CaptureCommand::Stop { completed, publication }) if network_result.is_none() => {
                        let Some(session) = capture_session.take() else {
                            let _ = completed.send(Err("No capture session is active.".into()));
                            continue;
                        };
                        capture.lifecycle("capture_stopped");
                        operation_reply = Some(CaptureOperationReply::Stop(completed));
                        operation = Some(stop_capture(session, publication.clone(), events.clone()));
                        stop_publication = Some(publication);
                    }
                    Some(CaptureCommand::Start { completed, .. }) => {
                        let _ = completed.send(Err("The network client has stopped.".into()));
                    }
                    Some(CaptureCommand::Stop { completed, .. }) => {
                        let _ = completed.send(Err("The network client has stopped.".into()));
                    }
                    None => {}
                }
            }
            result = &mut run, if network_result.is_none() => {
                if result.is_err() {
                    capture.lifecycle("network_failed");
                }
                network_result = Some(result);
            }
        }
    }

    network_result.unwrap_or(Ok(()))
}

fn start_capture(config: CaptureConfig, capture: CaptureRecorder) -> Pin<Box<dyn Future<Output = CaptureOperationResult> + Send>> {
    Box::pin(async move {
        let task = tokio::task::spawn_blocking(move || {
            let session = CaptureSession::start(config, capture.clone())?;
            capture.lifecycle("capture_started");
            Ok::<_, String>(session)
        });
        CaptureOperationResult::Started(task.await.map_err(|error| format!("Capture startup failed: {error}")).and_then(|result| result))
    })
}

fn stop_capture(
    session: CaptureSession,
    publication: CapturePublication,
    events: Sender<EventEnvelope>,
) -> Pin<Box<dyn Future<Output = CaptureOperationResult> + Send>> {
    let task = tokio::task::spawn_blocking(move || {
        let summary = session.finish_recording(&publication);
        let path = summary.capture_path.clone().unwrap_or_default();
        let event = if summary.state == CaptureState::Interrupted {
            Event::CaptureInterrupted { path, error: summary.error.clone() }
        } else {
            Event::CaptureFinished { path, error: summary.error.clone() }
        };
        let _ = events.try_send(EventEnvelope::new(event));
        summary
    });
    let operation: Pin<Box<dyn Future<Output = CaptureOperationResult> + Send>> = Box::pin(async move {
        match task.await {
            Ok(summary) => CaptureOperationResult::Stopped(Ok(summary)),
            Err(error) => CaptureOperationResult::Stopped(Err(format!("Capture finalization failed: {error}"))),
        }
    });
    operation
}

fn interrupt_capture(session: CaptureSession) -> Pin<Box<dyn Future<Output = CaptureOperationResult> + Send>> {
    let path = session.path().to_path_buf();
    Box::pin(async move {
        let task = tokio::task::spawn_blocking(move || session.interrupt());
        let error = match task.await {
            Ok(summary) => summary.error,
            Err(error) => Some(format!("Capture interruption failed: {error}")),
        };
        CaptureOperationResult::Interrupted { path, error }
    })
}

#[tracing::instrument(name = "network.session", level = "info", skip_all, fields(grid = ?credentials.grid))]
async fn run_client(
    credentials: LoginCredentials,
    channels: Channels,
    capture: Option<CaptureRecorder>,
    status: NetworkStatusHandle,
    login_url: String,
) -> Result<(), String> {
    let Channels { mut commands, command_sender, events, terminal_event_permit, mut disconnect_requested } = channels;
    let mut mfa_token = String::new();
    let mut mfa_hash = credentials.mfa_hash.clone();
    let (session, login_data) = loop {
        let response = match wait_for_login(
            crate::login::login_with_status_at(&credentials, &mfa_token, &mfa_hash, &login_url, capture.clone(), status.clone()),
            &mut disconnect_requested,
        )
        .await
        {
            Some(Ok(response)) => response,
            Some(Err(error)) => {
                tracing::warn!("Login failed");
                if let Some(capture) = &capture {
                    capture.lifecycle("login_failed");
                }
                return Err(error);
            }
            None => return Ok(()),
        };
        match response {
            LoginResponse::Authenticated { session, mfa_hash: returned_hash, data } => {
                tracing::info!(target: "network::client", "Login authenticated");
                if let Some(capture) = &capture {
                    capture.lifecycle("login_completed");
                }
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
                if let Some(capture) = &capture {
                    capture.lifecycle("mfa_challenge");
                }
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
                            let _ = completed.send(Err(Error::SimulatorNotReady));
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

    let simulator = SimulatorConnection::connect_with_status(session, login_data, capture, status).await?;
    simulator.run(commands, events, disconnect_requested, terminal_event_permit).await
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
#[path = "../tests/unit/client.rs"]
mod client_tests;
