use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

use protocol::lludp::Message;
use tokio::{
    sync::{mpsc, watch},
    task::JoinError,
};
use tracing::Instrument;
use url::Url;

use super::handoff::PromotionSetup;
use super::{
    ConnectionError, ConnectionTaskResult, HandoffCommand, HandoffTarget, NEIGHBOR_COMMAND_CAPACITY, PrimaryState, RunState, RuntimeAction,
    SessionDispatcher, SessionError, SessionState, SimulatorCapabilitySession, SimulatorCommand, SimulatorConnection,
    SimulatorConnectionOptions, SimulatorConnectionState, SimulatorEvent, SimulatorRequest, SimulatorTaskOutcome, is_current_simulator,
    record_event_envelope_loss, remove_simulator_capability,
};
use crate::{
    capabilities::{EVENT_QUEUE_NAME, EventQueueExit},
    circuit::{MAX_NEIGHBOR_CONNECTION_TASKS, MAX_NEIGHBOR_SIMULATORS},
    coverage::CoverageLossReason,
    event::{Event, EventEnvelope, SimulatorId},
};

use super::{handoff::matches_pending_promotion, teleport::is_handoff_message};

impl SessionState {
    pub(super) async fn start_neighbor(
        &mut self,
        origin: SimulatorId,
        region_handle: u64,
        endpoint: SocketAddr,
        handoff: Option<HandoffTarget>,
        setup: &mut PromotionSetup<'_>,
    ) -> Result<bool, SessionError> {
        let connections = &setup.state.neighbor_connections;
        if origin != self.primary.simulator_id || !is_current_simulator(origin, self.simulator_id, connections) {
            return Ok(false);
        }
        if handoff.as_ref().is_some_and(|target| target.region_handle != region_handle || target.endpoint != endpoint) {
            return Ok(false);
        }
        let simulator_id = SimulatorId::allocate().ok_or(SessionError::SimulatorIdSpaceExhausted)?;
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
    ) -> Result<bool, SessionError> {
        tracing::warn!(simulator = simulator_id.get(), region = region_handle, %reason, "neighbor simulator connection was rejected");
        self.publish_event(&setup.application_events, setup.state, Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint })
            .await?;
        Ok(false)
    }

    fn retire_neighbor(&self, region_handle: u64, state: &mut RunState) {
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
        let RunState { neighbors, neighbor_connections, neighbor_task_owners, .. } = state;
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
    ) -> Result<(), SessionError> {
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

impl SessionDispatcher<'_> {
    pub(super) async fn handle_neighbor_event(&mut self, neighbor_event: Option<SimulatorEvent>) -> Result<RuntimeAction, SessionError> {
        match neighbor_event {
            Some(SimulatorEvent::Simulator(event)) => self.handle_simulator_envelope(event).await,
            Some(SimulatorEvent::SendCompleted { simulator_id, cancel_target, error, report_failure }) => {
                self.handle_send_completed(simulator_id, cancel_target, error, report_failure).await
            }
            Some(SimulatorEvent::Finished { simulator_id, region_handle, endpoint, outcome }) => {
                self.handle_simulator_finished(simulator_id, region_handle, endpoint, outcome).await
            }
            None => {
                self.neighbor_events_closed = true;
                Ok(RuntimeAction::Continue)
            }
        }
    }

    async fn handle_simulator_envelope(&mut self, envelope: EventEnvelope) -> Result<RuntimeAction, SessionError> {
        let (event, retained_bytes, coverage_watermark) = envelope.into_parts();
        if let Event::SimulatorMessage { simulator_id, message: Message::AgentMovementComplete(movement), .. } = &event
            && *simulator_id == self.state.simulator_id
            && movement.agent_data.agent_id == self.state.session.agent_id
            && movement.agent_data.session_id == self.state.session.session_id
        {
            self.state.register_root_connection(movement.data.region_handle, &mut self.run_state.neighbor_connections);
        }
        let is_current = match &event {
            Event::SimulatorConnected { simulator_id, .. } | Event::SimulatorMessage { simulator_id, .. } => {
                is_current_simulator(*simulator_id, self.state.simulator_id, &self.run_state.neighbor_connections)
            }
            _ => false,
        };
        if !is_current {
            return Ok(RuntimeAction::Continue);
        }
        self.handle_current_simulator_event(event, retained_bytes, coverage_watermark).await
    }

    async fn handle_current_simulator_event(
        &mut self,
        event: Event,
        retained_bytes: Option<crate::event::EventRetention>,
        coverage_watermark: Option<crate::coverage::CoverageWatermark>,
    ) -> Result<RuntimeAction, SessionError> {
        if let Event::SimulatorMessage { simulator_id, message, .. } = &event
            && *simulator_id == self.state.simulator_id
            && self.startup.observe_primary_message(message, self.state.session.agent_id, self.state.session.session_id)
        {
            tracing::info!("simulator session ready");
            tracing::info!(lifecycle = "connected", "network lifecycle changed");
            self.state.primary.state.send_replace(PrimaryState::Ready { generation: 0 });
            if self.startup.should_publish_ready() {
                self.state.publish_session_ready(&self.events, &mut self.run_state).await?;
                self.startup.mark_ready_published();
            }
        }
        if let Event::SimulatorMessage { simulator_id, packet_order, message, .. } = &event
            && is_handoff_message(message)
            && self.run_state.logout_deadline.is_none()
        {
            let mut setup = PromotionSetup {
                state: &mut self.run_state,
                neighbor_requests: self.neighbor_requests.clone(),
                application_events: self.events.clone(),
                neighbor_events: self.neighbor_events.clone(),
                shutdown: self.disconnect_requested.clone(),
                capability_events: self.capability_events.clone(),
            };
            self.state.handle_handoff_message(*simulator_id, message, *packet_order, &mut setup, &mut self.handoff_state).await?;
        }
        if let Event::SimulatorMessage { simulator_id, message: Message::AgentMovementComplete(movement), .. } = &event
            && self.run_state.logout_deadline.is_none()
            && matches_pending_promotion(self.handoff_state.pending(), self.state.primary.simulator_id, *simulator_id, movement)
        {
            self.promote_primary(*simulator_id, event, retained_bytes, coverage_watermark).await?;
            return Ok(RuntimeAction::Continue);
        }
        if let Event::SimulatorConnected { simulator_id, .. } = &event {
            let simulator_id = *simulator_id;
            if simulator_id == self.state.simulator_id {
                self.startup.mark_primary_announced();
            }
            self.state
                .publish_simulator_connected(
                    simulator_id,
                    EventEnvelope::from_parts(event, retained_bytes, coverage_watermark),
                    &self.events,
                    &mut self.run_state,
                )
                .await?;
            if simulator_id == self.state.simulator_id && self.startup.should_publish_ready() {
                self.state.publish_session_ready(&self.events, &mut self.run_state).await?;
                self.startup.mark_ready_published();
            }
        } else if let Event::SimulatorMessage { simulator_id, .. } = &event {
            self.state
                .publish_simulator_event(
                    *simulator_id,
                    EventEnvelope::from_parts(event, retained_bytes, coverage_watermark),
                    &self.events,
                    &mut self.run_state,
                )
                .await?;
        }
        Ok(RuntimeAction::Continue)
    }

    pub(super) fn handle_pending_neighbor_finish(
        &mut self,
        permit: Result<mpsc::OwnedPermit<SimulatorEvent>, mpsc::error::SendError<()>>,
    ) -> RuntimeAction {
        match permit {
            Ok(permit) => {
                if let Some(finish) = self.pending_neighbor_finishes.pop_front() {
                    permit.send(finish);
                }
            }
            Err(_) => self.pending_neighbor_finishes.clear(),
        }
        RuntimeAction::Continue
    }

    pub(super) async fn handle_neighbor_request(&mut self, request: Option<SimulatorRequest>) -> Result<RuntimeAction, SessionError> {
        if let Some(SimulatorRequest::Enable { simulator_id, region_handle, endpoint }) = request
            && is_current_simulator(simulator_id, self.state.simulator_id, &self.run_state.neighbor_connections)
        {
            let mut setup = PromotionSetup {
                state: &mut self.run_state,
                neighbor_requests: self.neighbor_requests.clone(),
                application_events: self.events.clone(),
                neighbor_events: self.neighbor_events.clone(),
                shutdown: self.disconnect_requested.clone(),
                capability_events: self.capability_events.clone(),
            };
            Box::pin(self.state.start_neighbor(simulator_id, region_handle, endpoint, None, &mut setup)).await?;
        }
        Ok(RuntimeAction::Continue)
    }

    pub(super) fn handle_neighbor_task_join(&mut self, joined: Option<Result<ConnectionTaskResult, JoinError>>) -> RuntimeAction {
        match joined {
            Some(Ok((simulator_id, _region_handle, _endpoint, _outcome))) => {
                self.run_state.neighbor_task_owners.retain(|_, owner| owner.0 != simulator_id);
            }
            Some(Err(error)) => {
                if let Some((simulator_id, region_handle, endpoint)) = self.run_state.neighbor_task_owners.remove(&error.id())
                    && (simulator_id == self.state.simulator_id
                        || simulator_id == self.state.primary.simulator_id
                        || region_handle.is_some_and(|region_handle| {
                            is_current_neighbor(region_handle, simulator_id, &self.run_state.neighbor_connections)
                        })
                        || self.run_state.retiring_simulators.contains(&simulator_id))
                {
                    let outcome = if error.is_cancelled() && self.run_state.retiring_simulators.contains(&simulator_id) {
                        SimulatorTaskOutcome::Finished(Ok(()))
                    } else {
                        SimulatorTaskOutcome::Finished(Err(ConnectionError::NeighborTaskFailed(Arc::new(error))))
                    };
                    let finish = SimulatorEvent::Finished { simulator_id, region_handle, endpoint, outcome };
                    match self.neighbor_events.try_send(finish) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(finish)) => self.pending_neighbor_finishes.push_back(finish),
                        Err(mpsc::error::TrySendError::Closed(_)) => {}
                    }
                }
            }
            None => {}
        }
        RuntimeAction::Continue
    }

    pub(super) async fn handle_simulator_finished(
        &mut self,
        simulator_id: SimulatorId,
        region_handle: Option<u64>,
        endpoint: SocketAddr,
        outcome: SimulatorTaskOutcome,
    ) -> Result<RuntimeAction, SessionError> {
        if !is_current_simulator(simulator_id, self.state.simulator_id, &self.run_state.neighbor_connections)
            && !self.run_state.retiring_simulators.contains(&simulator_id)
        {
            return Ok(RuntimeAction::Continue);
        }
        if let Some(pending) = self.handoff_state.pending().filter(|pending| pending.target == simulator_id) {
            let reason = match &outcome {
                SimulatorTaskOutcome::Finished(Err(_)) => "destination connection ended with an error",
                SimulatorTaskOutcome::Finished(Ok(())) => "destination connection ended before promotion",
                SimulatorTaskOutcome::ConnectFailed(_) => "destination connection failed",
            };
            tracing::info!(simulator = simulator_id.get(), region = pending.handoff.region_handle, reason, "simulator handoff failed");
            self.handoff_state.take_pending();
        }
        if simulator_id != self.state.primary.simulator_id
            && let SimulatorTaskOutcome::Finished(Err(error)) | SimulatorTaskOutcome::ConnectFailed(error) = &outcome
        {
            tracing::warn!(simulator = simulator_id.get(), region = ?region_handle, endpoint = %endpoint, %error, "neighbor simulator connection failed");
        }
        let connection_region = region_handle.or_else(|| {
            self.run_state
                .neighbor_connections
                .iter()
                .find_map(|(region_handle, connection)| (connection.simulator_id == simulator_id).then_some(*region_handle))
        });
        if let Some(region_handle) = connection_region
            && is_current_neighbor(region_handle, simulator_id, &self.run_state.neighbor_connections)
        {
            self.run_state.neighbor_connections.remove(&region_handle);
        }
        self.run_state.retiring_simulators.remove(&simulator_id);
        remove_simulator_capability(&mut self.run_state.simulator_capabilities, simulator_id);
        for event in self.run_state.preconnect_events.remove(simulator_id) {
            record_event_envelope_loss(&self.state.coverage_losses, event, CoverageLossReason::EventAdmission);
        }
        self.run_state.connected_simulators.remove(&simulator_id);
        let event = neighbor_completion_event(simulator_id, region_handle, endpoint, &outcome);
        if simulator_id == self.state.primary.simulator_id {
            if *self.disconnect_requested.borrow() {
                self.state.neighbor_cancellation.send_replace(true);
                return Ok(RuntimeAction::Exit(Ok(())));
            }
            return Ok(RuntimeAction::Exit(match outcome {
                SimulatorTaskOutcome::Finished(Ok(())) => Ok(()),
                SimulatorTaskOutcome::Finished(Err(reason)) | SimulatorTaskOutcome::ConnectFailed(reason) => {
                    Err(SessionError::Connection(reason))
                }
            }));
        }
        if simulator_id == self.state.simulator_id {
            self.state.root_connection_failed = true;
        }
        self.state.publish_event(&self.events, &mut self.run_state, event).await?;
        Ok(RuntimeAction::Continue)
    }

    pub(super) async fn handle_root_connection_join(
        &mut self,
        joined: Option<Result<ConnectionTaskResult, JoinError>>,
    ) -> Result<RuntimeAction, SessionError> {
        if let Some(Err(error)) = joined {
            let reason = ConnectionError::PrimaryTaskFailed(Arc::new(error));
            if let Some(region_handle) = self
                .run_state
                .neighbor_connections
                .iter()
                .find_map(|(region_handle, connection)| (connection.simulator_id == self.state.simulator_id).then_some(*region_handle))
            {
                self.run_state.neighbor_connections.remove(&region_handle);
            }
            remove_simulator_capability(&mut self.run_state.simulator_capabilities, self.state.simulator_id);
            if self.state.primary.simulator_id == self.state.simulator_id {
                return Ok(RuntimeAction::Exit(Err(SessionError::Connection(reason))));
            }
            self.state.root_connection_failed = true;
            let event = neighbor_completion_event(
                self.state.simulator_id,
                None,
                self.state.session.simulator_address,
                &SimulatorTaskOutcome::ConnectFailed(reason),
            );
            self.state.publish_event(&self.events, &mut self.run_state, event).await?;
        }
        Ok(RuntimeAction::Continue)
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
