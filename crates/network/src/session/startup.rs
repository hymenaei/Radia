use std::collections::HashMap;

use protocol::{lludp::Message, login::ResponseData};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::{AbortHandle, JoinSet},
};
use tracing::Instrument;
use uuid::Uuid;

use super::routing::PrimaryEventBacklog;
use super::{
    ConnectionError, ConnectionTaskResult, NEIGHBOR_COMMAND_CAPACITY, PrimaryRoute, PrimaryState, RuntimeAction, RuntimeInputs,
    SessionDispatcher, SessionError, SessionRuntime, SessionRuntimeOptions, SessionState, SimulatorCommand, SimulatorConnection,
    SimulatorConnectionOptions, SimulatorConnectionState, SimulatorEvent, SimulatorMemoryBudget, SimulatorRequest, SimulatorTaskOutcome,
    track_capability_requests,
};
use crate::{
    AgentSession,
    capabilities::{CapabilityBudget, Client},
    circuit::pacing::OutboundBudget,
    error::Error,
    event::{CLIENT_EVENT_CAPACITY, SimulatorId},
};

pub(super) enum SessionStartup {
    Connecting { region_handshake: bool, movement_complete: bool, primary_announced: bool },
    Ready { primary_announced: bool, ready_published: bool },
}

impl Default for SessionStartup {
    fn default() -> Self {
        Self::Connecting { region_handshake: false, movement_complete: false, primary_announced: false }
    }
}

impl SessionStartup {
    pub(super) fn observe_primary_message(&mut self, message: &Message, agent_id: Uuid, session_id: Uuid) -> bool {
        let Self::Connecting { region_handshake: seen_handshake, movement_complete: seen_movement, primary_announced } = self else {
            return false;
        };
        let region_handshake = matches!(message, Message::RegionHandshake(_));
        let movement_complete = matches!(message, Message::AgentMovementComplete(movement)
            if movement.agent_data.agent_id == agent_id && movement.agent_data.session_id == session_id);
        *seen_handshake |= region_handshake;
        *seen_movement |= movement_complete;
        if *seen_handshake && *seen_movement {
            let primary_announced = *primary_announced;
            *self = Self::Ready { primary_announced, ready_published: false };
            true
        } else {
            false
        }
    }

    pub(super) fn mark_primary_announced(&mut self) {
        match self {
            Self::Connecting { primary_announced, .. } | Self::Ready { primary_announced, .. } => *primary_announced = true,
        }
    }

    pub(super) fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    pub(super) fn should_publish_ready(&self) -> bool {
        matches!(self, Self::Ready { primary_announced: true, ready_published: false })
    }

    pub(super) fn mark_ready_published(&mut self) {
        if let Self::Ready { ready_published, .. } = self {
            *ready_published = true;
        }
    }
}

struct RootConnectionStart {
    simulator_id: SimulatorId,
    endpoint: std::net::SocketAddr,
    session: AgentSession,
    options: SimulatorConnectionOptions,
    events: mpsc::Sender<SimulatorEvent>,
    neighbor_requests: mpsc::Sender<SimulatorRequest>,
    shutdown: watch::Receiver<bool>,
    memory: SimulatorMemoryBudget,
    commands: mpsc::Receiver<SimulatorCommand>,
    logout: mpsc::Receiver<oneshot::Sender<Result<(), Error>>>,
}

impl RootConnectionStart {
    fn spawn(self, workers: &mut JoinSet<ConnectionTaskResult>) -> (AbortHandle, oneshot::Receiver<Result<(), ConnectionError>>) {
        let Self { simulator_id, endpoint, session, options, events, neighbor_requests, shutdown, memory, commands, logout } = self;
        let (ready_sender, ready_receiver) = oneshot::channel::<Result<(), ConnectionError>>();
        let abort = workers.spawn(
            async move {
                let outcome = match SimulatorConnection::connect(session, options).await {
                    Ok(connection) => {
                        if ready_sender.send(Ok(())).is_err() {
                            tracing::debug!(
                                simulator = simulator_id.get(),
                                "startup waiter closed after the primary connection became ready"
                            );
                        }
                        SimulatorTaskOutcome::Finished(
                            connection.run(events.clone(), shutdown, neighbor_requests, commands, logout, memory).await,
                        )
                    }
                    Err(error) => {
                        if ready_sender.send(Err(error.clone())).is_err() {
                            tracing::debug!(simulator = simulator_id.get(), "startup waiter closed after the primary connection failed");
                        }
                        SimulatorTaskOutcome::ConnectFailed(error)
                    }
                };
                if events
                    .send(SimulatorEvent::Finished { simulator_id, region_handle: None, endpoint, outcome: outcome.clone() })
                    .await
                    .is_err()
                {
                    tracing::debug!(simulator = simulator_id.get(), "simulator connection result receiver closed");
                }
                (simulator_id, None, endpoint, outcome)
            }
            .in_current_span(),
        );
        (abort, ready_receiver)
    }
}

impl SessionRuntime {
    pub(super) async fn connect_inner(
        session: AgentSession,
        login_data: ResponseData,
        options: SessionRuntimeOptions,
    ) -> Result<Option<Self>, SessionError> {
        let application_event_capacity = options.application_event_capacity();
        let SessionRuntimeOptions { coverage_losses, session: options, mut startup_disconnect, root_transport, .. } = options;
        async {
            if startup_disconnect.as_ref().is_some_and(|disconnect| *disconnect.borrow()) {
                return Ok(None);
            }
            let simulator_id = SimulatorId::allocate().ok_or(SessionError::SimulatorIdSpaceExhausted)?;
            let capability_budget = CapabilityBudget::new();
            let capabilities = Client::new_with_budget(session.seed_capability.clone(), capability_budget.clone())?;
            let outbound_budget = OutboundBudget::new(options.upstream_bytes_per_second().get());
            let mut capability_request_waiters = JoinSet::new();
            track_capability_requests(&mut capability_request_waiters, &capabilities);
            let (neighbor_events, neighbor_event_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY);
            let (neighbor_requests, neighbor_request_receiver) = mpsc::channel(32);
            let (root_commands, root_command_receiver) = mpsc::channel(NEIGHBOR_COMMAND_CAPACITY);
            let (root_logout, root_logout_receiver) = mpsc::channel(1);
            let (primary_state_sender, primary_state_receiver) = watch::channel(PrimaryState::Connecting);
            let (neighbor_cancellation, _) = watch::channel(false);
            let packet_memory = SimulatorMemoryBudget::with_application_capacity(application_event_capacity);
            let mut root_connection_workers = JoinSet::<ConnectionTaskResult>::new();
            let (root_connection_abort, ready_receiver) = RootConnectionStart {
                simulator_id,
                endpoint: session.simulator_address,
                session: session.clone(),
                options: SimulatorConnectionOptions {
                    simulator_id,
                    region_handle: None,
                    coverage_losses: coverage_losses.clone(),
                    primary_state: Some(primary_state_receiver),
                    budget: outbound_budget.handle(),
                    owned_budget: None,
                    transport: root_transport,
                },
                events: neighbor_events.clone(),
                neighbor_requests: neighbor_requests.clone(),
                shutdown: neighbor_cancellation.subscribe(),
                memory: packet_memory.clone(),
                commands: root_command_receiver,
                logout: root_logout_receiver,
            }
            .spawn(&mut root_connection_workers);
            match Self::wait_for_primary_startup(&mut startup_disconnect, ready_receiver).await {
                Ok(true) => {}
                Ok(false) => {
                    Self::shutdown_connect_attempt(
                        &mut root_connection_workers,
                        &capabilities,
                        &mut capability_request_waiters,
                        outbound_budget,
                    )
                    .await;
                    return Ok(None);
                }
                Err(error) => {
                    Self::shutdown_connect_attempt(
                        &mut root_connection_workers,
                        &capabilities,
                        &mut capability_request_waiters,
                        outbound_budget,
                    )
                    .await;
                    return Err(error);
                }
            }
            Ok(Some(Self {
                state: SessionState {
                    simulator_id,
                    primary: PrimaryRoute {
                        simulator_id,
                        memory: packet_memory.clone(),
                        endpoint: session.simulator_address,
                        region_handle: None,
                        commands: root_commands,
                        state: primary_state_sender,
                        logout: root_logout,
                    },
                    root_connection_abort: Some(root_connection_abort),
                    neighbor_events,
                    neighbor_requests,
                    neighbor_cancellation,
                    root_connection_failed: false,
                    logout_request_sent: false,
                    logout_attempted: false,
                    logout_unavailable: None,
                    pending_primary_logout: None,
                    session,
                    capabilities,
                    capability_budget,
                    capability_request_waiters,
                    login_data,
                    event_backlog: PrimaryEventBacklog::default(),
                    packet_memory,
                    coverage_losses,
                    outbound_budget: Some(outbound_budget),
                    handoff_generation: Some(0),
                },
                runtime_inputs: RuntimeInputs { neighbor_event_receiver, neighbor_request_receiver, root_connection_workers },
            }))
        }
        .await
    }

    async fn wait_for_primary_startup(
        startup_disconnect: &mut Option<watch::Receiver<bool>>,
        mut ready_receiver: oneshot::Receiver<Result<(), ConnectionError>>,
    ) -> Result<bool, SessionError> {
        let ready = match startup_disconnect {
            Some(disconnect) => tokio::select! {
                biased;
                result = &mut ready_receiver => Some(result),
                _ = disconnect.changed() => None,
            },
            None => Some(ready_receiver.await),
        };
        match ready {
            Some(Ok(Ok(()))) => Ok(true),
            Some(Ok(Err(error))) => Err(SessionError::Connection(error)),
            Some(Err(_)) => Err(SessionError::PrimaryConnectionStoppedDuringStartup),
            None => Ok(false),
        }
    }

    async fn shutdown_connect_attempt(
        root_connection_workers: &mut JoinSet<ConnectionTaskResult>,
        capabilities: &Client,
        capability_request_waiters: &mut JoinSet<()>,
        outbound_budget: OutboundBudget,
    ) {
        root_connection_workers.shutdown().await;
        capabilities.close_session_and_wait().await;
        while let Some(result) = capability_request_waiters.join_next().await {
            if let Err(error) = result {
                tracing::error!(%error, "Capability request waiter failed during startup shutdown");
            }
        }
        outbound_budget.shutdown().await;
    }
}

impl SessionState {
    pub(super) fn register_root_connection(&mut self, region_handle: u64, connections: &mut HashMap<u64, SimulatorConnectionState>) {
        if let Some(current_region) = connections
            .iter()
            .find_map(|(current_region, connection)| (connection.simulator_id == self.simulator_id).then_some(*current_region))
        {
            if current_region != region_handle
                && let Some(connection) = connections.remove(&current_region)
            {
                connections.insert(region_handle, connection);
            }
        } else {
            let Some(abort) = self.root_connection_abort.take() else {
                return;
            };
            connections.insert(
                region_handle,
                SimulatorConnectionState {
                    simulator_id: self.simulator_id,
                    endpoint: self.session.simulator_address,
                    abort,
                    commands: self.primary.commands.clone(),
                    logout: self.primary.logout.clone(),
                    pending_handoff_command: None,
                    primary_state: Some(self.primary.state.clone()),
                    packet_memory: self.packet_memory.clone(),
                },
            );
        }
        if self.primary.simulator_id == self.simulator_id {
            self.primary.region_handle = Some(region_handle);
        }
    }
}

impl SessionDispatcher<'_> {
    pub(super) fn handle_session_startup_timeout(&mut self) -> RuntimeAction {
        tracing::warn!("simulator session timed out");
        RuntimeAction::Exit(Err(SessionError::SessionStartupTimedOut))
    }
}
