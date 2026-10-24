use std::collections::{HashMap, HashSet, VecDeque};

use tokio::{sync::mpsc, time::Instant};

use super::teleport::is_teleport_cancel;
use super::{
    ConnectionQueueFailure, EVENT_BACKLOG_WARNING_INTERVAL, EVENT_BACKLOG_WARNING_THRESHOLD, MAX_PRECONNECT_EVENTS,
    MAX_QUEUED_SIMULATOR_EVENTS, RuntimeAction, SessionDispatcher, SessionError, queue_connection_message, queue_primary_message,
};
use crate::{
    error::Error,
    event::{Event, EventEnvelope, PRIMARY_TERMINAL_EVENT_CAPACITY, SimulatorId},
    session::Command,
};

#[derive(Default)]
pub(super) struct PrimaryEventBacklog {
    nonempty_since: Option<Instant>,
    last_warning: Option<Instant>,
}

impl PrimaryEventBacklog {
    pub(super) fn observe(&mut self, pending_events: usize, now: Instant) -> Option<u64> {
        self.update_pending(pending_events, now);
        if pending_events < EVENT_BACKLOG_WARNING_THRESHOLD {
            return None;
        }
        if self.last_warning.is_some_and(|last| now.saturating_duration_since(last) < EVENT_BACKLOG_WARNING_INTERVAL) {
            return None;
        }

        self.last_warning = Some(now);
        Some(self.duration_ms(now))
    }

    pub(super) fn update_pending(&mut self, pending_events: usize, now: Instant) {
        if pending_events == 0 {
            self.nonempty_since = None;
        } else if self.nonempty_since.is_none() {
            self.nonempty_since = Some(now);
        }
    }

    pub(super) fn duration_ms(&self, now: Instant) -> u64 {
        self.nonempty_since.map(|since| now.saturating_duration_since(since).as_millis().try_into().unwrap_or(u64::MAX)).unwrap_or_default()
    }
}

#[derive(Default)]
pub(super) struct PreconnectEvents {
    by_simulator: HashMap<SimulatorId, VecDeque<EventEnvelope>>,
    len: usize,
}

impl PreconnectEvents {
    pub(super) fn push(&mut self, simulator_id: SimulatorId, event: EventEnvelope) -> bool {
        if self.len >= MAX_PRECONNECT_EVENTS {
            return false;
        }
        self.by_simulator.entry(simulator_id).or_default().push_back(event);
        self.len += 1;
        true
    }

    pub(super) fn take(&mut self, simulator_id: SimulatorId) -> VecDeque<EventEnvelope> {
        let events = self.by_simulator.remove(&simulator_id).unwrap_or_default();
        self.len -= events.len();
        events
    }

    pub(super) fn remove(&mut self, simulator_id: SimulatorId) -> VecDeque<EventEnvelope> {
        self.take(simulator_id)
    }
}

#[cfg(test)]
impl PreconnectEvents {
    pub(super) fn len(&self) -> usize {
        self.len
    }
}

pub(super) enum PreconnectOutcome {
    Ready(EventEnvelope),
    Buffered,
    Full,
}

pub(super) enum EventQueueOutcome {
    Queued,
    ReceiverClosed,
    BacklogFull,
}

impl SessionDispatcher<'_> {
    pub(super) async fn handle_pending_application_event(
        &mut self,
        permit: Result<mpsc::OwnedPermit<EventEnvelope>, mpsc::error::SendError<()>>,
    ) -> Result<RuntimeAction, SessionError> {
        match permit {
            Ok(permit) => {
                if let Some(event) = self.run_state.pending_events.pop_front() {
                    permit.send(event);
                }
                self.state.event_backlog.update_pending(self.run_state.pending_events.len(), Instant::now());
                Ok(RuntimeAction::Continue)
            }
            Err(_) if self.run_state.logout_deadline.is_none() => {
                self.state.disconnect_once(false).await?;
                Ok(RuntimeAction::Exit(Ok(())))
            }
            Err(_) => Ok(RuntimeAction::Continue),
        }
    }

    pub(super) async fn handle_command(&mut self, command: Option<Command>) -> Result<RuntimeAction, SessionError> {
        if *self.disconnect_requested.borrow() {
            self.state.graceful_disconnect(false).await?;
            return Ok(RuntimeAction::Exit(Ok(())));
        }
        match command {
            Some(Command::SubmitMfaCode(_)) => {}
            Some(Command::SendMessage { message, delivery }) => {
                if !self.startup.is_ready() {
                    self.state
                        .publish_event(
                            &self.events,
                            &mut self.run_state,
                            Event::SendFailed("The simulator session is still starting.".into()),
                        )
                        .await?;
                    return Ok(RuntimeAction::Continue);
                }

                let result = queue_primary_message(
                    &self.state.primary.commands,
                    self.state.session.agent_id,
                    self.state.session.session_id,
                    message,
                    delivery,
                    self.handoff_state.cancellation(),
                );
                if let Err(failure) = result {
                    self.state.publish_event(&self.events, &mut self.run_state, Event::SendFailed(failure.error.to_string())).await?;
                }
            }
            Some(Command::SendTo { simulator_id, message, delivery, completed }) => {
                let cancel_handoff = simulator_id == self.state.primary.simulator_id
                    && is_teleport_cancel(&message, self.state.session.agent_id, self.state.session.session_id);
                let cancel_target = cancel_handoff.then(|| self.handoff_state.cancellation()).flatten();
                let commands = self
                    .run_state
                    .neighbor_connections
                    .values()
                    .find(|connection| {
                        connection.simulator_id == simulator_id
                            && (simulator_id != self.state.simulator_id || !self.state.root_connection_failed)
                    })
                    .map(|connection| &connection.commands);
                let mut completion = Some(completed);
                let result = if simulator_id == self.state.simulator_id && !self.startup.is_ready() {
                    Err(ConnectionQueueFailure { error: Error::SimulatorNotReady, completion: completion.take() })
                } else if let Some(commands) = commands {
                    queue_connection_message(commands, message, delivery, completion.take(), cancel_target, false)
                } else {
                    Err(ConnectionQueueFailure { error: Error::SimulatorUnavailable, completion: completion.take() })
                };
                if let Err(failure) = result
                    && let Some(completion) = failure.completion
                    && let Err(error) = completion.send(Err(failure.error))
                {
                    drop(error);
                }
            }
            None => {
                self.state.graceful_disconnect(self.state.logout_request_sent).await?;
                return Ok(RuntimeAction::Exit(Ok(())));
            }
        }
        Ok(RuntimeAction::Continue)
    }
}

pub(super) fn queue_until_connected(
    connected: &HashSet<SimulatorId>,
    pending: &mut PreconnectEvents,
    simulator_id: SimulatorId,
    event: EventEnvelope,
) -> PreconnectOutcome {
    if connected.contains(&simulator_id) {
        return PreconnectOutcome::Ready(event);
    }
    if pending.push(simulator_id, event) { PreconnectOutcome::Buffered } else { PreconnectOutcome::Full }
}

pub(super) fn queue_simulator_event(
    events: &mpsc::Sender<EventEnvelope>,
    pending_events: &mut VecDeque<EventEnvelope>,
    event: EventEnvelope,
) -> EventQueueOutcome {
    if events.is_closed() {
        return EventQueueOutcome::ReceiverClosed;
    }

    if !pending_events.is_empty() {
        return if pending_events.len() < MAX_QUEUED_SIMULATOR_EVENTS {
            pending_events.push_back(event);
            EventQueueOutcome::Queued
        } else {
            EventQueueOutcome::BacklogFull
        };
    }

    match events.try_send(event) {
        Ok(()) => EventQueueOutcome::Queued,
        Err(mpsc::error::TrySendError::Closed(_)) => EventQueueOutcome::ReceiverClosed,
        Err(mpsc::error::TrySendError::Full(event)) => {
            pending_events.push_back(event);
            EventQueueOutcome::Queued
        }
    }
}

pub(super) fn primary_event_queue_size(events: &mpsc::Sender<EventEnvelope>, pending_events: usize) -> (usize, usize) {
    let channel_capacity = events.max_capacity().saturating_sub(PRIMARY_TERMINAL_EVENT_CAPACITY);
    let channel_occupancy = events.max_capacity().saturating_sub(events.capacity()).saturating_sub(PRIMARY_TERMINAL_EVENT_CAPACITY);
    (channel_occupancy + pending_events, channel_capacity + MAX_QUEUED_SIMULATOR_EVENTS)
}

#[cfg(test)]
#[path = "tests/event_routing.rs"]
mod tests;
