use std::{collections::VecDeque, pin::Pin};
use tokio::{
    sync::{mpsc, watch},
    task::JoinSet,
    time::{self, Interval, Sleep},
};

use super::handoff::HandoffState;
use super::{
    CLIENT_EVENT_CAPACITY, ConnectionTaskResult, CoordinatorError, CoordinatorRunState, CoordinatorRuntimeInputs, CoordinatorState,
    HANDOFF_COMMAND_RETRY_INTERVAL, LOGOUT_TIMEOUT, SESSION_TIMEOUT, SessionStartup, SimulatorCapabilitySession, SimulatorEvent,
    SimulatorRequest, retry_pending_handoff_commands,
};
use crate::{
    capabilities::{EventQueueRoleHandle, EventQueueTask},
    event::EventEnvelope,
    session::Command,
};

pub(super) struct CoordinatorRuntime<'a> {
    pub(super) coordinator: &'a mut CoordinatorState,
    pub(super) commands: mpsc::Receiver<Command>,
    pub(super) events: mpsc::Sender<EventEnvelope>,
    pub(super) disconnect_requested: watch::Receiver<bool>,
    pub(super) capability_events: mpsc::Sender<EventEnvelope>,
    pub(super) capability_event_rx: mpsc::Receiver<EventEnvelope>,
    pub(super) neighbor_events: mpsc::Sender<SimulatorEvent>,
    pub(super) neighbor_event_rx: mpsc::Receiver<SimulatorEvent>,
    pub(super) neighbor_requests: mpsc::Sender<SimulatorRequest>,
    pub(super) neighbor_request_rx: mpsc::Receiver<SimulatorRequest>,
    pub(super) root_connection_workers: JoinSet<ConnectionTaskResult>,
    pub(super) event_queue: EventQueueTask,
    pub(super) active_primary_event_queue: EventQueueRoleHandle,
    pub(super) event_queue_finished: bool,
    pub(super) capability_events_closed: bool,
    pub(super) neighbor_events_closed: bool,
    pub(super) startup: SessionStartup,
    pub(super) run_state: CoordinatorRunState,
    pub(super) pending_neighbor_finishes: VecDeque<SimulatorEvent>,
    pub(super) handoff_state: HandoffState,
    pub(super) handoff_command_retry: Interval,
    pub(super) session_deadline: Pin<Box<Sleep>>,
}

pub(super) enum RuntimeAction {
    Continue,
    Exit(Result<(), CoordinatorError>),
}

impl<'a> CoordinatorRuntime<'a> {
    pub(super) async fn start(
        coordinator: &'a mut CoordinatorState,
        runtime_inputs: CoordinatorRuntimeInputs,
        commands: mpsc::Receiver<Command>,
        events: mpsc::Sender<EventEnvelope>,
        disconnect_requested: watch::Receiver<bool>,
    ) -> Result<Self, CoordinatorError> {
        let (capability_events, capability_event_rx) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let CoordinatorRuntimeInputs { neighbor_event_receiver, neighbor_request_receiver, root_connection_workers } = runtime_inputs;
        let neighbor_events = coordinator.neighbor_events.clone();
        let neighbor_requests = coordinator.neighbor_requests.clone();
        let mut root_connection_workers = root_connection_workers;
        let event_queue = match coordinator.capabilities.start_primary_event_queue(
            coordinator.simulator_id,
            coordinator.session.simulator_address,
            capability_events.clone(),
            disconnect_requested.clone(),
            coordinator.coverage_losses.clone(),
        ) {
            Ok(event_queue) => event_queue,
            Err(error) => {
                coordinator.neighbor_cancellation.send_replace(true);
                root_connection_workers.shutdown().await;
                return Err(error.into());
            }
        };
        let active_primary_event_queue = event_queue.role_handle();
        let mut run_state = CoordinatorRunState::new();
        run_state.simulator_capabilities.insert(
            coordinator.simulator_id,
            SimulatorCapabilitySession {
                endpoint: coordinator.session.simulator_address,
                seed_capability: coordinator.session.seed_capability.clone(),
                client: coordinator.capabilities.clone(),
                event_queue_role: active_primary_event_queue.clone(),
                event_queue_task: None,
                event_queue_running: true,
            },
        );
        let mut handoff_command_retry = time::interval(HANDOFF_COMMAND_RETRY_INTERVAL);
        handoff_command_retry.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

        Ok(Self {
            coordinator,
            commands,
            events,
            disconnect_requested,
            capability_events,
            capability_event_rx,
            neighbor_events,
            neighbor_event_rx: neighbor_event_receiver,
            neighbor_requests,
            neighbor_request_rx: neighbor_request_receiver,
            root_connection_workers,
            event_queue,
            active_primary_event_queue,
            event_queue_finished: false,
            capability_events_closed: false,
            neighbor_events_closed: false,
            startup: SessionStartup::default(),
            run_state,
            pending_neighbor_finishes: VecDeque::new(),
            handoff_state: HandoffState::default(),
            handoff_command_retry,
            session_deadline: Box::pin(time::sleep(SESSION_TIMEOUT)),
        })
    }

    pub(super) async fn shutdown(mut self) {
        self.coordinator.capabilities.close_session();
        for capability in self.run_state.simulator_capabilities.values() {
            capability.client.close_session();
        }
        self.coordinator.neighbor_cancellation.send_replace(true);
        self.root_connection_workers.shutdown().await;
        self.run_state.neighbors.shutdown().await;
        while self.run_state.neighbor_event_queues.join_next().await.is_some() {}
        self.event_queue.wait().await;
        self.coordinator.capabilities.close_session_and_wait().await;
        for capability in self.run_state.simulator_capabilities.values() {
            capability.client.close_session_and_wait().await;
        }
    }

    #[tracing::instrument(name = "simulator.session", level = "info", skip_all)]
    pub(super) async fn run(&mut self) -> Result<(), CoordinatorError> {
        if *self.disconnect_requested.borrow() {
            self.coordinator.graceful_disconnect(false).await?;
            return Ok(());
        }
        loop {
            let action = tokio::select! {
                _ = self.disconnect_requested.changed() => self.handle_disconnect_changed().await?,
                result = async {
                    self.coordinator.pending_primary_logout
                        .as_mut()
                        .expect("pending logout branch requires an active request")
                        .wait()
                        .await
                }, if self.coordinator.pending_primary_logout.is_some() => self.handle_primary_logout_completed(result)?,
                _ = &mut self.session_deadline, if !self.startup.is_ready() && !*self.disconnect_requested.borrow() => {
                    self.handle_session_startup_timeout()
                }
                _ = time::sleep_until(self.run_state.logout_deadline.unwrap_or_else(|| time::Instant::now() + LOGOUT_TIMEOUT)), if self.run_state.logout_deadline.is_some() => {
                    self.handle_logout_deadline().await?
                }
                _ = self.events.closed(), if self.run_state.logout_deadline.is_none() => self.handle_event_receiver_closed().await?,
                _ = retry_pending_handoff_commands(&mut self.handoff_command_retry, &mut self.run_state.neighbor_connections),
                    if self.run_state.neighbor_connections.values().any(|connection| connection.pending_handoff_command.is_some()) => {
                        self.handle_handoff_command_retry()
                    }
                capability_event = self.capability_event_rx.recv(), if !self.capability_events_closed => self.handle_capability_event(capability_event).await?,
                neighbor_event = self.neighbor_event_rx.recv(), if !self.neighbor_events_closed => self.handle_neighbor_event(neighbor_event).await?,
                permit = self.neighbor_events.clone().reserve_owned(), if !self.pending_neighbor_finishes.is_empty() && !self.neighbor_events.is_closed() => {
                    self.handle_pending_neighbor_finish(permit)
                }
                joined = self.run_state.neighbor_event_queues.join_next_with_id(), if !self.run_state.neighbor_event_queues.is_empty() => {
                    self.handle_neighbor_event_queue_join(joined).await?
                }
                exit = self.event_queue.completion(), if !self.event_queue_finished => self.handle_primary_event_queue_completion(exit),
                permit = self.events.clone().reserve_owned(), if !self.run_state.pending_events.is_empty() && !self.events.is_closed() => {
                    self.handle_pending_application_event(permit).await?
                }
                request = self.neighbor_request_rx.recv(), if self.run_state.logout_deadline.is_none() => self.handle_neighbor_request(request).await?,
                joined = self.coordinator.capability_request_waiters.join_next(), if !self.coordinator.capability_request_waiters.is_empty() => {
                    self.handle_capability_request_join(joined)
                }
                joined = self.root_connection_workers.join_next(), if !self.root_connection_workers.is_empty() => {
                    self.handle_root_connection_join(joined).await?
                }
                joined = self.run_state.neighbors.join_next(), if !self.run_state.neighbors.is_empty() => self.handle_neighbor_task_join(joined),
                command = self.commands.recv(), if self.run_state.logout_deadline.is_none() => self.handle_command(command).await?,
            };
            match action {
                RuntimeAction::Continue => {}
                RuntimeAction::Exit(result) => return result,
            }
        }
    }
}
