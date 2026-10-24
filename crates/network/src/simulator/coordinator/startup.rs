use std::collections::HashMap;

use protocol::{lludp::Message, login::ResponseData};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
};
use tracing::Instrument;
use uuid::Uuid;

use super::{
    ConnectionTaskResult, NEIGHBOR_COMMAND_CAPACITY, PrimaryEventBacklog, PrimaryRoute, PrimaryState, SimulatorConnection,
    SimulatorConnectionOptions, SimulatorConnectionState, SimulatorCoordinator, SimulatorCoordinatorOptions, SimulatorEvent,
    SimulatorMemoryBudget, SimulatorTaskOutcome, track_capability_requests,
};
use crate::{
    AgentSession,
    capabilities::{CapabilityBudget, Client},
    event::{CLIENT_EVENT_CAPACITY, SimulatorId},
    transport::pacing::{OutboundBudget, OutboundBudgetHandle},
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

impl SimulatorCoordinator {
    pub(super) async fn connect_inner(
        session: AgentSession,
        login_data: ResponseData,
        options: SimulatorCoordinatorOptions,
    ) -> Result<Option<Self>, String> {
        let SimulatorCoordinatorOptions { coverage_losses, session: options, mut startup_disconnect, root_transport } = options;
        async {
            if startup_disconnect.as_ref().is_some_and(|disconnect| *disconnect.borrow()) {
                return Ok(None);
            }
            let simulator_id = SimulatorId::allocate().ok_or_else(|| "Simulator connection ID space is exhausted.".to_string())?;
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
            let packet_memory = SimulatorMemoryBudget::new();
            let (ready_sender, ready_receiver) = oneshot::channel::<Result<(), String>>();
            let mut root_connection_workers = JoinSet::<ConnectionTaskResult>::new();
            let root_id = simulator_id;
            let root_endpoint = session.simulator_address;
            let root_events = neighbor_events.clone();
            let root_neighbor_requests = neighbor_requests.clone();
            let root_shutdown = neighbor_cancellation.subscribe();
            let root_memory = packet_memory.clone();
            let root_session = session.clone();
            let root_coverage_losses = coverage_losses.clone();
            let root_budget: OutboundBudgetHandle = outbound_budget.handle();
            let root_connection_abort = root_connection_workers.spawn(
                async move {
                    let outcome = match SimulatorConnection::connect(
                        root_session,
                        SimulatorConnectionOptions {
                            simulator_id: root_id,
                            region_handle: None,
                            coverage_losses: root_coverage_losses,
                            primary_state: Some(primary_state_receiver),
                            budget: root_budget,
                            owned_budget: None,
                            transport: root_transport,
                        },
                    )
                    .await
                    {
                        Ok(connection) => {
                            let _ = ready_sender.send(Ok(()));
                            SimulatorTaskOutcome::Finished(
                                connection
                                    .run(
                                        root_events.clone(),
                                        root_shutdown,
                                        root_neighbor_requests,
                                        root_command_receiver,
                                        root_logout_receiver,
                                        root_memory,
                                    )
                                    .await,
                            )
                        }
                        Err(error) => {
                            let _ = ready_sender.send(Err(error.clone()));
                            SimulatorTaskOutcome::ConnectFailed(error)
                        }
                    };
                    let _ = root_events
                        .send(SimulatorEvent::Finished {
                            simulator_id: root_id,
                            region_handle: None,
                            endpoint: root_endpoint,
                            outcome: outcome.clone(),
                        })
                        .await;
                    (root_id, None, root_endpoint, outcome)
                }
                .in_current_span(),
            );
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
                neighbor_event_receiver: Some(neighbor_event_receiver),
                neighbor_requests,
                neighbor_request_receiver: Some(neighbor_request_receiver),
                root_connection_workers: Some(root_connection_workers),
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
                handoff_generation: 0,
            }))
        }
        .await
    }

    async fn wait_for_primary_startup(
        startup_disconnect: &mut Option<watch::Receiver<bool>>,
        mut ready_receiver: oneshot::Receiver<Result<(), String>>,
    ) -> Result<bool, String> {
        let ready = match startup_disconnect {
            Some(disconnect) => tokio::select! {
                biased;
                result = &mut ready_receiver => Some(result),
                _ = disconnect.changed() => None,
            },
            None => Some(ready_receiver.await),
        };
        match ready {
            Some(Ok(result)) => result.map(|()| true),
            Some(Err(_)) => Err("The primary simulator connection task stopped during startup.".to_string()),
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
