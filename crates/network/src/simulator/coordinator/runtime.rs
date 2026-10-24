use std::{collections::VecDeque, pin::Pin};

use protocol::lludp::Message;
use tokio::{
    sync::{mpsc, watch},
    task::JoinSet,
    time::{self, Interval, Sleep},
};

use super::{
    CLIENT_EVENT_CAPACITY, ConnectionQueueFailure, ConnectionTaskResult, CoordinatorRunState, HANDOFF_COMMAND_RETRY_INTERVAL,
    LOGOUT_TIMEOUT, PrimaryRoute, PrimaryState, SESSION_TIMEOUT, SessionStartup, SimulatorCapabilitySession, SimulatorCoordinator,
    SimulatorEvent, SimulatorRequest, SimulatorTaskOutcome,
    capability_events::redact_seed_capability,
    finish_logout,
    handoff::{
        HandoffOrder, HandoffState, PromotionSetup, cancel_pending_handoff, is_handoff_message, is_teleport_cancel,
        matches_handoff_cancellation, matches_pending_promotion,
    },
    is_current_neighbor, is_current_simulator, neighbor_completion_event, neighbor_event_queue_failure, queue_connection_message,
    queue_primary_message, record_event_envelope_loss, remove_simulator_capability, retry_pending_handoff_commands,
};
use crate::{
    capabilities::{EventQueueExit, EventQueueRoleHandle, EventQueueTask},
    coverage::CoverageLossReason,
    error::Error,
    event::{Event, EventEnvelope},
    session::Command,
};

pub(super) struct CoordinatorRuntime<'a> {
    coordinator: &'a mut SimulatorCoordinator,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<EventEnvelope>,
    disconnect_requested: watch::Receiver<bool>,
    capability_events: mpsc::Sender<EventEnvelope>,
    capability_event_rx: mpsc::Receiver<EventEnvelope>,
    neighbor_events: mpsc::Sender<SimulatorEvent>,
    neighbor_event_rx: mpsc::Receiver<SimulatorEvent>,
    neighbor_requests: mpsc::Sender<SimulatorRequest>,
    neighbor_request_rx: mpsc::Receiver<SimulatorRequest>,
    root_connection_workers: JoinSet<ConnectionTaskResult>,
    event_queue: EventQueueTask,
    active_primary_event_queue: EventQueueRoleHandle,
    event_queue_finished: bool,
    capability_events_closed: bool,
    neighbor_events_closed: bool,
    startup: SessionStartup,
    run_state: CoordinatorRunState,
    pending_neighbor_finishes: VecDeque<SimulatorEvent>,
    handoff_state: HandoffState,
    handoff_command_retry: Interval,
    session_deadline: Pin<Box<Sleep>>,
}

impl<'a> CoordinatorRuntime<'a> {
    pub(super) async fn start(
        coordinator: &'a mut SimulatorCoordinator,
        commands: mpsc::Receiver<Command>,
        events: mpsc::Sender<EventEnvelope>,
        disconnect_requested: watch::Receiver<bool>,
    ) -> Result<Self, String> {
        let (capability_events, capability_event_rx) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let neighbor_events = coordinator.neighbor_events.clone();
        let neighbor_event_rx = coordinator.neighbor_event_receiver.take().expect("neighbor events are received once");
        let neighbor_requests = coordinator.neighbor_requests.clone();
        let neighbor_request_rx = coordinator.neighbor_request_receiver.take().expect("neighbor requests are received once");
        let mut root_connection_workers = coordinator.root_connection_workers.take().expect("root connection workers are started once");
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
                return Err(error);
            }
        };
        let active_primary_event_queue = event_queue.role_handle();
        let event_queue_finished = false;
        let capability_events_closed = false;
        let neighbor_events_closed = false;
        let startup = SessionStartup::default();
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
        let pending_neighbor_finishes = VecDeque::new();
        let handoff_state = HandoffState::default();
        let mut handoff_command_retry = time::interval(HANDOFF_COMMAND_RETRY_INTERVAL);
        handoff_command_retry.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let session_deadline = Box::pin(time::sleep(SESSION_TIMEOUT));

        Ok(Self {
            coordinator,
            commands,
            events,
            disconnect_requested,
            capability_events,
            capability_event_rx,
            neighbor_events,
            neighbor_event_rx,
            neighbor_requests,
            neighbor_request_rx,
            root_connection_workers,
            event_queue,
            active_primary_event_queue,
            event_queue_finished,
            capability_events_closed,
            neighbor_events_closed,
            startup,
            run_state,
            pending_neighbor_finishes,
            handoff_state,
            handoff_command_retry,
            session_deadline,
        })
    }

    #[tracing::instrument(name = "simulator.session", level = "info", skip_all)]
    pub(super) async fn run(self) -> Result<(), String> {
        let Self {
            coordinator,
            mut commands,
            events,
            mut disconnect_requested,
            capability_events,
            mut capability_event_rx,
            neighbor_events,
            mut neighbor_event_rx,
            neighbor_requests,
            mut neighbor_request_rx,
            mut root_connection_workers,
            mut event_queue,
            mut active_primary_event_queue,
            mut event_queue_finished,
            mut capability_events_closed,
            mut neighbor_events_closed,
            mut startup,
            mut run_state,
            mut pending_neighbor_finishes,
            mut handoff_state,
            mut handoff_command_retry,
            mut session_deadline,
        } = self;
        let result = async {
        if *disconnect_requested.borrow() {
            coordinator.graceful_disconnect(false).await?;
            return Ok(());
        }
        loop {
            tokio::select! {
                _ = disconnect_requested.changed() => {
                    coordinator.graceful_disconnect(coordinator.logout_request_sent).await?;
                    return Ok(());
                }
                result = async {
                    coordinator.pending_primary_logout
                        .as_mut()
                        .expect("pending logout branch requires an active request")
                        .wait()
                        .await
                }, if coordinator.pending_primary_logout.is_some() => {
                    coordinator.pending_primary_logout = None;
                    coordinator.apply_logout_attempt_result(result)?;
                }
                _ = &mut session_deadline, if !startup.is_ready() && !*disconnect_requested.borrow() => {
                    tracing::warn!("simulator session timed out");
                    return Err("Timed out while establishing the simulator session.".into());
                }
                _ = time::sleep_until(run_state.logout_deadline.unwrap_or_else(|| time::Instant::now() + LOGOUT_TIMEOUT)), if run_state.logout_deadline.is_some() => {
                    coordinator.finish_pending_logout().await?;
                    if *disconnect_requested.borrow() {
                        coordinator.neighbor_cancellation.send_replace(true);
                        return Ok(());
                    }
                    return finish_logout(run_state.disconnect_reason);
                }
                _ = events.closed(), if run_state.logout_deadline.is_none() => {
                    coordinator.graceful_disconnect(false).await?;
                    return Ok(());
                }
                _ = retry_pending_handoff_commands(&mut handoff_command_retry, &mut run_state.neighbor_connections),
                    if run_state.neighbor_connections.values().any(|connection| connection.pending_handoff_command.is_some()) => {}
                capability_event = capability_event_rx.recv(), if !capability_events_closed => {
                    match capability_event {
                        Some(envelope) => {
                            let (event, retained_bytes, coverage_watermark) = envelope.into_parts();
                            match event {
                        Event::CapabilityEvent { simulator_id, source, message, mut body } => {
                            if !is_current_simulator(simulator_id, coordinator.simulator_id, &run_state.neighbor_connections) {
                                continue;
                            }
                            let mut setup = PromotionSetup {
                                state: &mut run_state,
                                neighbor_requests: neighbor_requests.clone(),
                                application_events: events.clone(),
                                neighbor_events: neighbor_events.clone(),
                                shutdown: disconnect_requested.clone(),
                                capability_events: capability_events.clone(),
                            };
                            coordinator.process_capability_event(
                                simulator_id,
                                &message,
                                &body,
                                None,
                                &mut setup,
                                &mut handoff_state,
                            )
                            .await?;
                            redact_seed_capability(&mut body);
                            coordinator.publish_simulator_event(
                                simulator_id,
                                EventEnvelope::from_parts(
                                    Event::CapabilityEvent {
                                        simulator_id,
                                        source,
                                        message,
                                        body,
                                    },
                                    retained_bytes,
                                    coverage_watermark,
                                ),
                                &events, &mut run_state,
                            )
                            .await?;
                        }
                        Event::CapabilityEventBatch { simulator_id, source, order, events: mut batch } => {
                            if !is_current_simulator(simulator_id, coordinator.simulator_id, &run_state.neighbor_connections) {
                                continue;
                            }
                            let mut setup = PromotionSetup {
                                state: &mut run_state,
                                neighbor_requests: neighbor_requests.clone(),
                                application_events: events.clone(),
                                neighbor_events: neighbor_events.clone(),
                                shutdown: disconnect_requested.clone(),
                                capability_events: capability_events.clone(),
                            };
                            for (index, event) in batch.iter().enumerate() {
                                coordinator.process_capability_event(
                                    simulator_id,
                                    &event.message,
                                    &event.body,
                                    Some(HandoffOrder::EventQueue { incarnation: order.incarnation, batch: order.ordinal, index }),
                                    &mut setup,
                                    &mut handoff_state,
                                )
                                .await?;
                            }
                            for event in &mut batch {
                                redact_seed_capability(&mut event.body);
                            }
                            coordinator.publish_simulator_event(
                                simulator_id,
                                EventEnvelope::from_parts(
                                    Event::CapabilityEventBatch { simulator_id, source, order, events: batch },
                                    retained_bytes,
                                    coverage_watermark,
                                ),
                                &events, &mut run_state,
                            )
                            .await?;
                        }
                        Event::CapabilityError { simulator_id, source, capability, reason }
                            if is_current_simulator(simulator_id, coordinator.simulator_id, &run_state.neighbor_connections) =>
                        {
                            coordinator.publish_simulator_event(
                                simulator_id,
                                EventEnvelope::from_parts(
                                    Event::CapabilityError { simulator_id, source, capability, reason },
                                    retained_bytes,
                                    coverage_watermark,
                                ),
                                &events, &mut run_state,
                            ).await?;
                        }
                        Event::CapabilityError { .. } => {}
                        event => {
                            coordinator.publish_event(
                                &events,
                                &mut run_state.pending_events,
                                EventEnvelope::from_parts(event, retained_bytes, coverage_watermark),
                                &mut run_state.logout_deadline,
                                &mut run_state.disconnect_reason,
                            ).await?;
                        }
                            }
                        }
                        None => capability_events_closed = true,
                    }
                }
                neighbor_event = neighbor_event_rx.recv(), if !neighbor_events_closed => {
                    match neighbor_event {
                        Some(SimulatorEvent::Simulator(event)) => {
                            let (event, retained_bytes, coverage_watermark) = event.into_parts();
                            if let Event::SimulatorMessage {
                                simulator_id,
                                message: Message::AgentMovementComplete(movement),
                                ..
                            } = &event
                                && *simulator_id == coordinator.simulator_id
                                && movement.agent_data.agent_id == coordinator.session.agent_id
                                && movement.agent_data.session_id == coordinator.session.session_id
                            {
                                coordinator.register_root_connection(movement.data.region_handle, &mut run_state.neighbor_connections);
                            }
                            let is_current = match &event {
                                Event::SimulatorConnected { simulator_id, .. }
                                | Event::SimulatorMessage { simulator_id, .. } => {
                                    is_current_simulator(*simulator_id, coordinator.simulator_id, &run_state.neighbor_connections)
                                }
                                _ => false,
                            };
                            if is_current {
                                if let Event::SimulatorMessage { simulator_id, message, .. } = &event
                                    && *simulator_id == coordinator.simulator_id
                                    && startup.observe_primary_message(message, coordinator.session.agent_id, coordinator.session.session_id)
                                {
                                        tracing::info!("simulator session ready");
                                        tracing::info!(lifecycle = "connected", "network lifecycle changed");
                                        coordinator.primary.state.send_replace(PrimaryState::Ready { generation: 0 });
                                        if startup.should_publish_ready() {
                                            coordinator.publish_session_ready(
                                                &events,
                                                &mut run_state.pending_events,
                                                &mut run_state.logout_deadline,
                                                &mut run_state.disconnect_reason,
                                            ).await?;
                                            startup.mark_ready_published();
                                        }
                                }
                                if let Event::SimulatorMessage { simulator_id, packet_order, message, .. } = &event
                                    && is_handoff_message(message)
                                    && run_state.logout_deadline.is_none()
                                {
                                    let mut setup = PromotionSetup {
                                        state: &mut run_state,
                                        neighbor_requests: neighbor_requests.clone(),
                                        application_events: events.clone(),
                                        neighbor_events: neighbor_events.clone(),
                                        shutdown: disconnect_requested.clone(),
                                        capability_events: capability_events.clone(),
                                    };
                                    coordinator.handle_handoff_message(
                                        *simulator_id,
                                        message,
                                        *packet_order,
                                        &mut setup,
                                        &mut handoff_state,
                                    )
                                    .await?;
                                }
                                if let Event::SimulatorMessage {
                                    simulator_id,
                                    message: Message::AgentMovementComplete(movement),
                                    ..
                                } = &event
                                    && run_state.logout_deadline.is_none()
                                    && matches_pending_promotion(
                                        handoff_state.pending(),
                                        coordinator.primary.simulator_id,
                                        *simulator_id,
                                        movement,
                                    )
                                {
                                    let promoted_simulator_id = *simulator_id;
                                    let promotion = handoff_state.take_pending().expect("matching promotion is pending");
                                    let Some(connection) = run_state.neighbor_connections
                                        .get(&promotion.handoff.region_handle)
                                        .filter(|connection| {
                                            connection.simulator_id == promotion.target
                                                && connection.endpoint == promotion.handoff.endpoint
                                        })
                                    else {
                                        tracing::info!(
                                            simulator = promotion.target.get(),
                                            region = promotion.handoff.region_handle,
                                            reason = "destination circuit is stale",
                                            "simulator handoff failed"
                                        );
                                        continue;
                                    };
                                    let Some(next_primary) = PrimaryRoute::from_neighbor(promotion.handoff.region_handle, connection) else {
                                        tracing::info!(
                                            simulator = promotion.target.get(),
                                            region = promotion.handoff.region_handle,
                                            reason = "destination primary route is unavailable",
                                            "simulator handoff failed"
                                        );
                                        continue;
                                    };
                                    let Some((capabilities, promoted_event_queue)) = run_state.simulator_capabilities
                                        .get(&promotion.target)
                                        .filter(|capabilities| capabilities.endpoint == promotion.handoff.endpoint)
                                        .map(|capabilities| {
                                            (capabilities.client.clone(), capabilities.event_queue_role.clone())
                                        })
                                    else {
                                        tracing::info!(
                                            simulator = promotion.target.get(),
                                            region = promotion.handoff.region_handle,
                                            reason = "destination capabilities are unavailable",
                                            "simulator handoff failed"
                                        );
                                        continue;
                                    };
                                    let previous_simulator_id = coordinator.primary.simulator_id;
                                    coordinator.publish_simulator_event(
                                        promoted_simulator_id,
                                        EventEnvelope::from_parts(event, retained_bytes, coverage_watermark),
                                        &events, &mut run_state,
                                    )
                                    .await?;
                                    if run_state.logout_deadline.is_some() {
                                        continue;
                                    }
                                    let mut session = coordinator.session.clone();
                                    session.simulator_address = promotion.handoff.endpoint;
                                    session.seed_capability = promotion.handoff.seed_capability;
                                    let role_generation = promotion.generation.get();
                                    coordinator.promote_primary(next_primary, role_generation);
                                    tracing::info!(
                                        simulator = promotion.target.get(),
                                        region = promotion.handoff.region_handle,
                                        "simulator promoted"
                                    );
                                    active_primary_event_queue.set_primary(false);
                                    promoted_event_queue.set_primary(true);
                                    active_primary_event_queue = promoted_event_queue;
                                    coordinator.publish_event(
                                        &events,
                                        &mut run_state.pending_events,
                                        Event::SimulatorPromoted {
                                            previous_simulator_id,
                                            simulator_id: promotion.target,
                                            session: Box::new(session),
                                            capabilities,
                                        },
                                        &mut run_state.logout_deadline,
                                        &mut run_state.disconnect_reason,
                                    )
                                    .await?;
                                    continue;
                                }
                                if let Event::SimulatorConnected { simulator_id, .. } = &event {
                                    let simulator_id = *simulator_id;
                                    if simulator_id == coordinator.simulator_id {
                                        startup.mark_primary_announced();
                                    }
                                    coordinator.publish_simulator_connected(
                                        simulator_id,
                                        EventEnvelope::from_parts(event, retained_bytes, coverage_watermark),
                                        &events,
                                        &mut run_state.pending_events,
                                        &mut run_state.preconnect_events,
                                        &mut run_state.connected_simulators,
                                        &mut run_state.logout_deadline,
                                        &mut run_state.disconnect_reason,
                                    ).await?;
                                    if simulator_id == coordinator.simulator_id && startup.should_publish_ready() {
                                        coordinator.publish_session_ready(
                                            &events,
                                            &mut run_state.pending_events,
                                            &mut run_state.logout_deadline,
                                            &mut run_state.disconnect_reason,
                                        ).await?;
                                        startup.mark_ready_published();
                                    }
                                } else if let Event::SimulatorMessage { simulator_id, .. } = &event {
                                    coordinator.publish_simulator_event(
                                        *simulator_id,
                                        EventEnvelope::from_parts(event, retained_bytes, coverage_watermark),
                                        &events, &mut run_state,
                                    ).await?;
                                }
                            }
                        }
                        Some(SimulatorEvent::SendCompleted { simulator_id, cancel_target, error, report_failure }) => {
                            if let Some(error) = error {
                                if report_failure {
                                    coordinator.publish_event(
                                        &events,
                                        &mut run_state.pending_events,
                                        Event::SendFailed(error),
                                        &mut run_state.logout_deadline,
                                        &mut run_state.disconnect_reason,
                                    ).await?;
                                }
                            } else if simulator_id == coordinator.primary.simulator_id
                                && let Some(cancel) = cancel_target
                            {
                                if handoff_state.clear_conflict(cancel) {
                                    continue;
                                }
                                if matches_handoff_cancellation(&handoff_state, cancel)
                                    && let Some(pending) = handoff_state.cancel_matching(cancel)
                                {
                                    cancel_pending_handoff(
                                        pending,
                                        "teleport cancelled",
                                        coordinator.simulator_id,
                                        &mut run_state.neighbor_connections,
                                        &mut run_state.retiring_simulators,
                                        &mut run_state.preconnect_events,
                                        &mut run_state.connected_simulators,
                                        &mut run_state.simulator_capabilities,
                                        &coordinator.coverage_losses,
                                    );
                                }
                            }
                        }
                        Some(SimulatorEvent::Finished { simulator_id, region_handle, endpoint, outcome }) => {
                            if is_current_simulator(simulator_id, coordinator.simulator_id, &run_state.neighbor_connections)
                                || run_state.retiring_simulators.contains(&simulator_id)
                            {
                                if let Some(pending) = handoff_state.pending().filter(|pending| pending.target == simulator_id) {
                                    let reason = match &outcome {
                                        SimulatorTaskOutcome::Finished(Err(_)) => "destination connection ended with an error",
                                        SimulatorTaskOutcome::Finished(Ok(())) => "destination connection ended before promotion",
                                        SimulatorTaskOutcome::ConnectFailed(_) => "destination connection failed",
                                    };
                                    tracing::info!(
                                        simulator = simulator_id.get(),
                                        region = pending.handoff.region_handle,
                                        reason,
                                        "simulator handoff failed"
                                    );
                                    handoff_state.take_pending();
                                }
                                let connection_region = region_handle.or_else(|| {
                                    run_state.neighbor_connections.iter().find_map(|(region_handle, connection)| {
                                        (connection.simulator_id == simulator_id).then_some(*region_handle)
                                    })
                                });
                                if let Some(region_handle) = connection_region
                                    && is_current_neighbor(region_handle, simulator_id, &run_state.neighbor_connections)
                                {
                                    run_state.neighbor_connections.remove(&region_handle);
                                }
                                run_state.retiring_simulators.remove(&simulator_id);
                                remove_simulator_capability(&mut run_state.simulator_capabilities, simulator_id);
                                for event in run_state.preconnect_events.remove(simulator_id) {
                                    record_event_envelope_loss(&coordinator.coverage_losses, event, CoverageLossReason::EventAdmission);
                                }
                                run_state.connected_simulators.remove(&simulator_id);
                                let event = neighbor_completion_event(simulator_id, region_handle, endpoint, &outcome);
                            if simulator_id == coordinator.primary.simulator_id {
                                    if *disconnect_requested.borrow() {
                                        coordinator.neighbor_cancellation.send_replace(true);
                                        return finish_logout(run_state.disconnect_reason);
                                    }
                                    return match outcome {
                                        SimulatorTaskOutcome::Finished(Ok(())) => finish_logout(run_state.disconnect_reason),
                                        SimulatorTaskOutcome::Finished(Err(reason)) | SimulatorTaskOutcome::ConnectFailed(reason) => {
                                            Err(reason)
                                        }
                                    };
                                }
                                if simulator_id == coordinator.simulator_id {
                                    coordinator.root_connection_failed = true;
                                }
                                coordinator.publish_event(
                                    &events,
                                    &mut run_state.pending_events,
                                    event,
                                    &mut run_state.logout_deadline,
                                    &mut run_state.disconnect_reason,
                                ).await?;
                            }
                        }
                        None => neighbor_events_closed = true,
                    }
                }
                permit = neighbor_events.reserve(), if !pending_neighbor_finishes.is_empty() && !neighbor_events.is_closed() => {
                    match permit {
                        Ok(permit) => {
                            if let Some(finish) = pending_neighbor_finishes.pop_front() {
                                permit.send(finish);
                            }
                        }
                        Err(_) => pending_neighbor_finishes.clear(),
                    }
                }
                joined = run_state.neighbor_event_queues.join_next_with_id(), if !run_state.neighbor_event_queues.is_empty() => {
                    match joined {
                        Some(Ok((task_id, (simulator_id, source, exit)))) => {
                            run_state.neighbor_event_queue_owners.remove(&task_id);
                            let current_queue = run_state.simulator_capabilities.get_mut(&simulator_id).is_some_and(|capability| {
                                if capability.endpoint == source && capability.event_queue_task == Some(task_id) {
                                    capability.event_queue_running = false;
                                    true
                                } else {
                                    false
                                }
                            });
                            if !current_queue {
                                continue;
                            }
                            if simulator_id == coordinator.primary.simulator_id
                                && exit != EventQueueExit::Stopped
                                && !*disconnect_requested.borrow()
                                && run_state.logout_deadline.is_none()
                            {
                                coordinator.note_primary_event_queue_exit(exit);
                            }
                            if simulator_id != coordinator.primary.simulator_id
                                && let Some(event) = neighbor_event_queue_failure(simulator_id, source, exit, coordinator.simulator_id, &run_state.neighbor_connections)
                            {
                                coordinator.publish_event(
                                    &events,
                                    &mut run_state.pending_events,
                                    event,
                                    &mut run_state.logout_deadline,
                                    &mut run_state.disconnect_reason,
                                ).await?;
                            }
                        }
                        Some(Err(error)) => {
                            let task_id = error.id();
                            let current_queue = run_state.neighbor_event_queue_owners.get(&task_id).copied().is_some_and(
                                |(simulator_id, source)| {
                                    run_state.simulator_capabilities.get_mut(&simulator_id).is_some_and(|capability| {
                                        if capability.endpoint == source && capability.event_queue_task == Some(task_id) {
                                            capability.event_queue_running = false;
                                            true
                                        } else {
                                            false
                                        }
                                    })
                                },
                            );
                            if !current_queue {
                                run_state.neighbor_event_queue_owners.remove(&task_id);
                                continue;
                            }
                            let Some((simulator_id, source)) = run_state.neighbor_event_queue_owners.remove(&task_id) else {
                                continue;
                            };
                            let active_primary_queue = simulator_id == coordinator.primary.simulator_id;
                            if active_primary_queue && !*disconnect_requested.borrow() && run_state.logout_deadline.is_none() {
                                coordinator.note_primary_event_queue_exit(EventQueueExit::TaskFailed);
                            }
                            if !active_primary_queue
                                && let Some(event) = neighbor_event_queue_failure(
                                    simulator_id,
                                    source,
                                    EventQueueExit::TaskFailed,
                                    coordinator.simulator_id,
                                    &run_state.neighbor_connections,
                                )
                            {
                                coordinator.publish_event(
                                    &events,
                                    &mut run_state.pending_events,
                                    event,
                                    &mut run_state.logout_deadline,
                                    &mut run_state.disconnect_reason,
                                ).await?;
                            }
                        }
                        None => {}
                    }
                }
                exit = event_queue.completion(), if !event_queue_finished => {
                    event_queue_finished = true;
                    let initial_queue_current = run_state.simulator_capabilities.get_mut(&coordinator.simulator_id).is_some_and(|capability| {
                        if capability.endpoint == coordinator.session.simulator_address
                            && capability.seed_capability == coordinator.session.seed_capability
                            && capability.event_queue_task.is_none()
                        {
                            capability.event_queue_running = false;
                            true
                        } else {
                            false
                        }
                    });
                    if coordinator.primary.simulator_id == coordinator.simulator_id
                        && initial_queue_current
                        && !*disconnect_requested.borrow()
                        && run_state.logout_deadline.is_none()
                    {
                        coordinator.note_primary_event_queue_exit(exit);
                    } else if initial_queue_current
                        && matches!(exit, EventQueueExit::TaskFailed | EventQueueExit::RetryLimitReached | EventQueueExit::Unavailable)
                    {
                        tracing::warn!(simulator = coordinator.simulator_id.get(), "former primary simulator EventQueueGet ended after promotion");
                    }
                }
                permit = events.reserve(), if !run_state.pending_events.is_empty() && !events.is_closed() => {
                    match permit {
                        Ok(permit) => {
                            if let Some(event) = run_state.pending_events.pop_front() {
                                permit.send(event);
                            }
                            coordinator.event_backlog.update_pending(run_state.pending_events.len(), time::Instant::now());
                        }
                        Err(_) if run_state.logout_deadline.is_none() => {
                            coordinator.disconnect_once(false).await?;
                            return Ok(());
                        }
                        Err(_) => {}
                    }
                }
                request = neighbor_request_rx.recv(), if run_state.logout_deadline.is_none() => {
                    if let Some(SimulatorRequest::Enable { simulator_id, region_handle, endpoint }) = request
                        && is_current_simulator(simulator_id, coordinator.simulator_id, &run_state.neighbor_connections)
                    {
                        let mut setup = PromotionSetup {
                            state: &mut run_state,
                            neighbor_requests: neighbor_requests.clone(),
                            application_events: events.clone(),
                            neighbor_events: neighbor_events.clone(),
                            shutdown: disconnect_requested.clone(),
                            capability_events: capability_events.clone(),
                        };
                        Box::pin(coordinator.start_neighbor(
                            simulator_id,
                            region_handle,
                            endpoint,
                            None,
                            &mut setup,
                        )).await?;
                    }
                }
                joined = coordinator.capability_request_waiters.join_next(), if !coordinator.capability_request_waiters.is_empty() => {
                    if matches!(joined, Some(Err(_))) {
                        tracing::error!("Capability request waiter task failed");
                    }
                }
                joined = root_connection_workers.join_next(), if !root_connection_workers.is_empty() => {
                    if let Some(Err(error)) = joined {
                        let reason = format!("The primary simulator connection task failed: {error}");
                        if let Some(region_handle) = run_state.neighbor_connections
                            .iter()
                            .find_map(|(region_handle, connection)| (connection.simulator_id == coordinator.simulator_id).then_some(*region_handle))
                        {
                            run_state.neighbor_connections.remove(&region_handle);
                        }
                        remove_simulator_capability(&mut run_state.simulator_capabilities, coordinator.simulator_id);
                        if coordinator.primary.simulator_id == coordinator.simulator_id {
                            return Err(reason);
                        }
                        coordinator.root_connection_failed = true;
                        let event = neighbor_completion_event(
                            coordinator.simulator_id,
                            None,
                            coordinator.session.simulator_address,
                            &SimulatorTaskOutcome::ConnectFailed(reason),
                        );
                        coordinator.publish_event(
                            &events,
                            &mut run_state.pending_events,
                            event,
                            &mut run_state.logout_deadline,
                            &mut run_state.disconnect_reason,
                        )
                        .await?;
                    }
                }
                joined = run_state.neighbors.join_next(), if !run_state.neighbors.is_empty() => {
                    match joined {
                        Some(Ok((simulator_id, _region_handle, _endpoint, _outcome))) => {
                            run_state.neighbor_task_owners.retain(|_, owner| owner.0 != simulator_id);
                        }
                        Some(Err(error)) => {
                            if let Some((simulator_id, region_handle, endpoint)) = run_state.neighbor_task_owners.remove(&error.id())
                                && (simulator_id == coordinator.simulator_id
                                    || simulator_id == coordinator.primary.simulator_id
                                    || region_handle.is_some_and(|region_handle| is_current_neighbor(region_handle, simulator_id, &run_state.neighbor_connections))
                                    || run_state.retiring_simulators.contains(&simulator_id))
                            {
                                let outcome = if error.is_cancelled() && run_state.retiring_simulators.contains(&simulator_id) {
                                    SimulatorTaskOutcome::Finished(Ok(()))
                                } else {
                                    SimulatorTaskOutcome::Finished(Err(error.to_string()))
                                };
                                let finish = SimulatorEvent::Finished { simulator_id, region_handle, endpoint, outcome };
                                match neighbor_events.try_send(finish) {
                                    Ok(()) => {}
                                    Err(mpsc::error::TrySendError::Full(finish)) => pending_neighbor_finishes.push_back(finish),
                                    Err(mpsc::error::TrySendError::Closed(_)) => {}
                                }
                            }
                        }
                        None => {}
                    }
                }
                command = commands.recv(), if run_state.logout_deadline.is_none() => {
                    if *disconnect_requested.borrow() {
                        coordinator.graceful_disconnect(false).await?;
                        return Ok(());
                    }
                    match command {
                        Some(Command::SubmitMfaCode(_)) => {}
                        Some(Command::SendMessage { message, delivery }) => {
                            if !startup.is_ready() {
                                coordinator.publish_event(
                                    &events,
                                    &mut run_state.pending_events,
                                    Event::SendFailed("The simulator session is still starting.".into()),
                                    &mut run_state.logout_deadline,
                                    &mut run_state.disconnect_reason,
                                ).await?;
                                continue;
                            }

                            let result = queue_primary_message(
                                &coordinator.primary.commands,
                                coordinator.session.agent_id,
                                coordinator.session.session_id,
                                message,
                                delivery,
                                handoff_state.cancellation(),
                            );
                            if let Err(failure) = result {
                                coordinator.publish_event(
                                    &events,
                                    &mut run_state.pending_events,
                                    Event::SendFailed(failure.error.to_string()),
                                    &mut run_state.logout_deadline,
                                    &mut run_state.disconnect_reason,
                                ).await?;
                            }
                        }
                        Some(Command::SendTo {
                            simulator_id,
                            message,
                            delivery,
                            completed,
                        }) => {
                            let cancel_handoff = simulator_id == coordinator.primary.simulator_id
                                && is_teleport_cancel(&message, coordinator.session.agent_id, coordinator.session.session_id);
                            let cancel_target = cancel_handoff.then(|| handoff_state.cancellation()).flatten();
                            let commands = run_state
                                .neighbor_connections
                                .values()
                                .find(|connection| connection.simulator_id == simulator_id && (simulator_id != coordinator.simulator_id || !coordinator.root_connection_failed))
                                .map(|connection| &connection.commands);
                            let mut completion = Some(completed);
                            let result = if simulator_id == coordinator.simulator_id && !startup.is_ready() {
                                Err(ConnectionQueueFailure { error: Error::SimulatorNotReady, completion: completion.take() })
                            } else if let Some(commands) = commands {
                                queue_connection_message(commands, message, delivery, completion.take(), cancel_target, false)
                            } else {
                                Err(ConnectionQueueFailure { error: Error::SimulatorUnavailable, completion: completion.take() })
                            };
                            if let Err(failure) = result
                                && let Some(completion) = failure.completion
                            {
                                let _ = completion.send(Err(failure.error));
                            }
                        }
                        None => {
                            coordinator.graceful_disconnect(coordinator.logout_request_sent).await?;
                            return Ok(());
                        }
                    }
                }
            }
        }
        }
        .await;
        coordinator.capabilities.close_session();
        for capability in run_state.simulator_capabilities.values() {
            capability.client.close_session();
        }
        coordinator.neighbor_cancellation.send_replace(true);
        root_connection_workers.shutdown().await;
        run_state.neighbors.shutdown().await;
        while run_state.neighbor_event_queues.join_next().await.is_some() {}
        let _ = event_queue.wait().await;
        coordinator.capabilities.close_session_and_wait().await;
        for capability in run_state.simulator_capabilities.values() {
            capability.client.close_session_and_wait().await;
        }
        result
    }
}
