use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

use tokio::sync::{mpsc, watch};
use tracing::Instrument;
use url::Url;

use super::{
    CoordinatorError, CoordinatorRunState, HandoffCommand, HandoffTarget, NEIGHBOR_COMMAND_CAPACITY, PrimaryState, PromotionSetup,
    SimulatorCapabilitySession, SimulatorCommand, SimulatorConnection, SimulatorConnectionOptions, SimulatorConnectionState,
    SimulatorCoordinator, SimulatorEvent, SimulatorTaskOutcome, is_current_simulator, record_event_envelope_loss,
    remove_simulator_capability,
};
use crate::{
    capabilities::{EVENT_QUEUE_NAME, EventQueueExit},
    coverage::CoverageLossReason,
    event::{Event, SimulatorId},
    simulator::{MAX_NEIGHBOR_CONNECTION_TASKS, MAX_NEIGHBOR_SIMULATORS},
};

impl SimulatorCoordinator {
    pub(super) async fn start_neighbor(
        &mut self,
        origin: SimulatorId,
        region_handle: u64,
        endpoint: SocketAddr,
        handoff: Option<HandoffTarget>,
        setup: &mut PromotionSetup<'_>,
    ) -> Result<bool, CoordinatorError> {
        let connections = &setup.state.neighbor_connections;
        if origin != self.primary.simulator_id || !is_current_simulator(origin, self.simulator_id, connections) {
            return Ok(false);
        }
        if handoff.as_ref().is_some_and(|target| target.region_handle != region_handle || target.endpoint != endpoint) {
            return Ok(false);
        }
        let simulator_id = SimulatorId::allocate().ok_or(CoordinatorError::SimulatorIdSpaceExhausted)?;
        if endpoint.port() == 0
            || endpoint.ip().is_unspecified()
            || endpoint.ip().is_multicast()
            || endpoint.ip() == IpAddr::V4(Ipv4Addr::BROADCAST)
        {
            return self
                .reject_neighbor_start(
                    simulator_id,
                    region_handle,
                    endpoint,
                    "The simulator announced an invalid neighbor endpoint.",
                    setup,
                )
                .await;
        }
        if self.reuse_neighbor(region_handle, endpoint, handoff.as_ref(), &mut setup.state.neighbor_connections) {
            return Ok(true);
        }
        if setup.state.neighbor_connections.get(&region_handle).is_some_and(|current| current.simulator_id == self.simulator_id) {
            return self
                .reject_neighbor_start(
                    simulator_id,
                    region_handle,
                    endpoint,
                    "The original simulator connection cannot be replaced before its handoff completes.",
                    setup,
                )
                .await;
        }
        if (endpoint == self.session.simulator_address && handoff.is_none())
            || setup.state.neighbor_connections.iter().any(|(active_region, active)| {
                *active_region != region_handle
                    && active.endpoint == endpoint
                    && (active.simulator_id != self.simulator_id || handoff.is_none())
            })
        {
            return self
                .reject_neighbor_start(
                    simulator_id,
                    region_handle,
                    endpoint,
                    "The simulator announced an endpoint that is already in use.",
                    setup,
                )
                .await;
        }
        let root_task_count =
            usize::from(setup.state.neighbor_task_owners.values().any(|(task_simulator_id, _, _)| *task_simulator_id == self.simulator_id));
        if setup.state.neighbors.len().saturating_sub(root_task_count) >= MAX_NEIGHBOR_CONNECTION_TASKS {
            return self
                .reject_neighbor_start(simulator_id, region_handle, endpoint, "The neighbor connection task limit has been reached.", setup)
                .await;
        }
        self.retire_neighbor(region_handle, setup.state);
        let neighbor_count =
            setup.state.neighbor_connections.values().filter(|connection| connection.simulator_id != self.simulator_id).count();
        if neighbor_count >= MAX_NEIGHBOR_SIMULATORS {
            return self
                .reject_neighbor_start(
                    simulator_id,
                    region_handle,
                    endpoint,
                    "The simulator announced more neighbor regions than the client can track.",
                    setup,
                )
                .await;
        }

        self.spawn_neighbor(simulator_id, region_handle, endpoint, handoff, setup);
        Ok(true)
    }

    fn reuse_neighbor(
        &self,
        region_handle: u64,
        endpoint: SocketAddr,
        handoff: Option<&HandoffTarget>,
        connections: &mut HashMap<u64, SimulatorConnectionState>,
    ) -> bool {
        let Some(current) = connections.get_mut(&region_handle).filter(|current| current.endpoint == endpoint) else { return false };
        handoff.is_none_or(|handoff| current.queue_handoff_command(HandoffCommand::Prepare(handoff.clone())))
    }

    async fn reject_neighbor_start(
        &mut self,
        simulator_id: SimulatorId,
        region_handle: u64,
        endpoint: SocketAddr,
        reason: &'static str,
        setup: &mut PromotionSetup<'_>,
    ) -> Result<bool, CoordinatorError> {
        tracing::warn!(simulator = simulator_id.get(), region = region_handle, %reason, "neighbor simulator connection was rejected");
        self.publish_event(
            &setup.application_events,
            &mut setup.state.pending_events,
            Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint },
            &mut setup.state.logout_deadline,
            &mut setup.state.disconnect_reason,
        )
        .await?;
        Ok(false)
    }

    fn retire_neighbor(&self, region_handle: u64, state: &mut CoordinatorRunState) {
        let Some(current) = state.neighbor_connections.remove(&region_handle) else { return };
        if state.connected_simulators.contains(&current.simulator_id) && !current.abort.is_finished() {
            current.log_disconnected(region_handle);
        }
        state.retiring_simulators.insert(current.simulator_id);
        for event in state.preconnect_events.remove(current.simulator_id) {
            record_event_envelope_loss(&self.coverage_losses, event, CoverageLossReason::EventAdmission);
        }
        remove_simulator_capability(&mut state.simulator_capabilities, current.simulator_id);
        current.abort.abort();
    }

    fn spawn_neighbor(
        &mut self,
        simulator_id: SimulatorId,
        region_handle: u64,
        endpoint: SocketAddr,
        handoff: Option<HandoffTarget>,
        setup: &mut PromotionSetup<'_>,
    ) {
        let neighbor_requests = setup.neighbor_requests.clone();
        let neighbor_events = setup.neighbor_events.clone();
        let shutdown = self.neighbor_cancellation.subscribe();
        let state = &mut *setup.state;
        let CoordinatorRunState { neighbors, neighbor_connections, neighbor_task_owners, .. } = state;
        let mut session = self.session.clone();
        session.simulator_address = endpoint;
        if let Some(handoff) = &handoff {
            session.seed_capability = handoff.seed_capability.clone();
        }
        let coverage_losses = self.coverage_losses.clone();
        let packet_memory = self.packet_memory.for_neighbor();
        let connection_packet_memory = packet_memory.clone();
        let outbound_budget = self.outbound_budget.as_ref().expect("session pacing budget exists").handle();
        let (primary_state, primary_state_receiver) = watch::channel(PrimaryState::Neighbor { generation: 0 });
        let (command_sender, command_receiver) = mpsc::channel(NEIGHBOR_COMMAND_CAPACITY);
        let (logout_sender, logout_receiver) = mpsc::channel(1);
        if let Some(handoff) = handoff {
            command_sender.try_send(SimulatorCommand::PrepareHandoff(handoff)).expect("new neighbor command queue has capacity");
        }
        let finish_events = neighbor_events.clone();
        let task = neighbors.spawn(
            async move {
                let outcome = match SimulatorConnection::connect(
                    session,
                    SimulatorConnectionOptions {
                        simulator_id,
                        region_handle: Some(region_handle),
                        coverage_losses,
                        primary_state: Some(primary_state_receiver),
                        budget: outbound_budget,
                        owned_budget: None,
                        transport: None,
                    },
                )
                .await
                {
                    Ok(connection) => SimulatorTaskOutcome::Finished(
                        connection
                            .run(neighbor_events, shutdown, neighbor_requests, command_receiver, logout_receiver, packet_memory)
                            .await,
                    ),
                    Err(error) => SimulatorTaskOutcome::ConnectFailed(error),
                };
                if finish_events
                    .send(SimulatorEvent::Finished { simulator_id, region_handle: Some(region_handle), endpoint, outcome: outcome.clone() })
                    .await
                    .is_err()
                {
                    tracing::debug!(simulator = simulator_id.get(), region = region_handle, "neighbor result receiver closed");
                }
                (simulator_id, Some(region_handle), endpoint, outcome)
            }
            .in_current_span(),
        );
        neighbor_task_owners.insert(task.id(), (simulator_id, Some(region_handle), endpoint));
        neighbor_connections.insert(
            region_handle,
            SimulatorConnectionState {
                simulator_id,
                endpoint,
                abort: task,
                commands: command_sender,
                logout: logout_sender,
                pending_handoff_command: None,
                primary_state: Some(primary_state),
                packet_memory: connection_packet_memory,
            },
        );
    }

    pub(super) async fn start_neighbor_capabilities(
        &mut self,
        simulator_id: SimulatorId,
        endpoint: SocketAddr,
        seed: Url,
        setup: &mut PromotionSetup<'_>,
    ) -> Result<(), CoordinatorError> {
        let client = match self.new_capability_client(seed.clone()) {
            Ok(client) => client,
            Err(_error) => {
                tracing::warn!(simulator = simulator_id.get(), "could not start neighbor capability requests");
                return Ok(());
            }
        };
        let event_queue = match client.start_event_queue(
            simulator_id,
            endpoint,
            setup.capability_events.clone(),
            setup.shutdown.clone(),
            self.coverage_losses.clone(),
        ) {
            Ok(event_queue) => event_queue,
            Err(_error) => {
                client.close_session();
                tracing::warn!(simulator = simulator_id.get(), "could not start neighbor event queue");
                return Ok(());
            }
        };
        let event_queue_role = event_queue.role_handle();
        let task = setup.state.neighbor_event_queues.spawn(async move {
            let exit = event_queue.wait().await;
            (simulator_id, endpoint, exit)
        });
        let task_id = task.id();
        setup.state.neighbor_event_queue_owners.insert(task_id, (simulator_id, endpoint));
        setup.state.simulator_capabilities.insert(
            simulator_id,
            SimulatorCapabilitySession {
                endpoint,
                seed_capability: seed,
                client: client.clone(),
                event_queue_role,
                event_queue_task: Some(task_id),
                event_queue_running: true,
            },
        );
        self.publish_simulator_event(
            simulator_id,
            Event::SimulatorCapabilities { simulator_id, source: endpoint, capabilities: client }.into(),
            &setup.application_events,
            setup.state,
        )
        .await
    }
}

pub(super) fn neighbor_completion_event(
    simulator_id: SimulatorId,
    region_handle: Option<u64>,
    endpoint: SocketAddr,
    outcome: &SimulatorTaskOutcome,
) -> Event {
    match outcome {
        SimulatorTaskOutcome::Finished(Ok(())) => Event::SimulatorDisconnected { simulator_id, region_handle, endpoint },
        SimulatorTaskOutcome::Finished(Err(reason)) => {
            Event::SimulatorFailed { simulator_id, region_handle, endpoint, reason: reason.to_string() }
        }
        SimulatorTaskOutcome::ConnectFailed(reason) => match region_handle {
            Some(region_handle) => Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint },
            None => Event::SimulatorFailed { simulator_id, region_handle: None, endpoint, reason: reason.to_string() },
        },
    }
}

pub(super) fn is_current_neighbor(
    region_handle: u64,
    simulator_id: SimulatorId,
    connections: &HashMap<u64, SimulatorConnectionState>,
) -> bool {
    connections.get(&region_handle).is_some_and(|current| current.simulator_id == simulator_id)
}

pub(super) fn neighbor_event_queue_failure(
    simulator_id: SimulatorId,
    source: SocketAddr,
    exit: EventQueueExit,
    primary_id: SimulatorId,
    connections: &HashMap<u64, SimulatorConnectionState>,
) -> Option<Event> {
    let reason = match exit {
        EventQueueExit::Stopped => return None,
        EventQueueExit::StoppedWithCoverageLoss => "The neighbor simulator EventQueueGet stopped after losing event coverage.",
        EventQueueExit::Unavailable => "The neighbor simulator does not have a usable EventQueueGet capability.",
        EventQueueExit::RetryLimitReached => "The neighbor simulator EventQueueGet stopped after repeated errors.",
        EventQueueExit::TaskFailed => "The neighbor simulator EventQueueGet task ended unexpectedly.",
    };
    if !is_current_simulator(simulator_id, primary_id, connections) {
        return None;
    }
    tracing::warn!(simulator = simulator_id.get(), source = %source, ?exit, reason, "neighbor simulator event queue stopped");
    Some(Event::CapabilityError { simulator_id, source, capability: EVENT_QUEUE_NAME.to_owned(), reason: reason.into() })
}
