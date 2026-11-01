use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
    thread,
};

use tokio::{
    runtime::Builder,
    sync::{
        mpsc::{self, OwnedPermit, Receiver, Sender},
        watch,
    },
};

use super::{
    COMMAND_CAPACITY, Command, MfaChallenge, NETWORK_THREAD_STACK_SIZE, Session, SessionError, SessionHandle, SessionOptions,
    SessionRuntime, SessionRuntimeOptions,
};
use crate::{
    LoginCredentials,
    coverage::CoverageLossLedger,
    error::Error,
    event::{CLIENT_EVENT_CAPACITY, Event, EventEnvelope, EventSendOutcome, PRIMARY_TERMINAL_EVENT_CAPACITY, send_client_event},
    login::{LoginResponse, login_at_url},
};

pub(super) struct Channels {
    pub(super) commands: Receiver<Command>,
    pub(super) command_sender: Sender<Command>,
    pub(super) events: Sender<EventEnvelope>,
    pub(super) terminal_event_permit: OwnedPermit<EventEnvelope>,
    pub(super) disconnect_requested: watch::Receiver<bool>,
    pub(super) coverage_losses: CoverageLossLedger,
}

pub(super) fn start_at(credentials: LoginCredentials, options: SessionOptions, login_url: String) -> Result<Session, Error> {
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
                runtime.block_on(crate::event::send_failure_event(&failure_sender, error.to_string(), &mut failure_shutdown));
            } else {
                tracing::info!(lifecycle = "disconnected", "network lifecycle changed");
            }
            runtime.shutdown_background();
            worker_finished_sender.send_replace(true);
        })
        .map_err(Error::WorkerStart)?;

    Ok(Session {
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

#[tracing::instrument(name = "network.session", level = "info", skip_all, fields(grid = ?credentials.grid))]
pub(super) async fn run_client(
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
                        Some(Command::SendTo { completed, .. }) | Some(Command::ReplyRegionHandshake { completed, .. }) => {
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

    let Some(runtime) = SessionRuntime::connect(
        session,
        login_data,
        SessionRuntimeOptions {
            coverage_losses,
            session: options,
            startup_disconnect: Some(disconnect_requested.clone()),
            root_transport: None,
            #[cfg(test)]
            application_event_capacity: crate::circuit::admission::SESSION_APPLICATION_EVENT_CREDIT_CAPACITY,
        },
    )
    .await?
    else {
        return Ok(());
    };
    runtime.run(commands, events, disconnect_requested, terminal_event_permit).await
}

pub(super) async fn wait_for_login<F: Future>(login: F, disconnect_requested: &mut watch::Receiver<bool>) -> Option<F::Output> {
    if *disconnect_requested.borrow() {
        return None;
    }

    tokio::select! {
        biased;
        _ = disconnect_requested.changed() => None,
        response = login => Some(response),
    }
}
