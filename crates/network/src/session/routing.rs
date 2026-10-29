use std::collections::{HashMap, HashSet, VecDeque};

use tokio::{sync::mpsc, time::Instant};

use super::teleport::is_teleport_cancel;
use super::{
    ConnectionQueueFailure, EVENT_BACKLOG_WARNING_INTERVAL, EVENT_BACKLOG_WARNING_THRESHOLD, MAX_PENDING_LIFECYCLE_EVENTS,
    MAX_PENDING_SESSION_EVENTS, MAX_PRECONNECT_EVENTS, RuntimeAction, SessionDispatcher, SessionError, queue_connection_message,
    queue_primary_message,
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

pub(super) enum PreconnectOutcome {
    Ready(EventEnvelope),
    Buffered,
    Full,
}

#[derive(Debug)]
pub(super) enum EventQueueOutcome {
    Queued,
    ReceiverClosed,
    BacklogFull,
    LifecycleBacklogFull,
}

impl SessionDispatcher<'_> {
    pub(super) async fn handle_pending_application_event(
        &mut self,
        permit: Result<mpsc::OwnedPermit<EventEnvelope>, mpsc::error::SendError<()>>,
    ) -> Result<RuntimeAction, SessionError> {
        match permit {
            Ok(permit) => {
                if let Some(mut event) = self.run_state.pending_events.pop_front() {
                    if !event.event.is_application() {
                        self.run_state.pending_lifecycle_events -= 1;
                    }
                    event.release_application_credit();
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
    pending_lifecycle_events: &mut usize,
    event: EventEnvelope,
) -> EventQueueOutcome {
    if events.is_closed() {
        return EventQueueOutcome::ReceiverClosed;
    }

    let is_application = event.event.is_application();
    if !is_application && *pending_lifecycle_events >= MAX_PENDING_LIFECYCLE_EVENTS {
        return EventQueueOutcome::LifecycleBacklogFull;
    }
    if pending_events.len() >= MAX_PENDING_SESSION_EVENTS {
        return if is_application { EventQueueOutcome::BacklogFull } else { EventQueueOutcome::LifecycleBacklogFull };
    }

    if !pending_events.is_empty() {
        pending_events.push_back(event);
        if !is_application {
            *pending_lifecycle_events += 1;
        }
        return EventQueueOutcome::Queued;
    }

    match events.try_reserve() {
        Ok(permit) => {
            let mut event = event;
            event.release_application_credit();
            permit.send(event);
            EventQueueOutcome::Queued
        }
        Err(mpsc::error::TrySendError::Closed(_)) => EventQueueOutcome::ReceiverClosed,
        Err(mpsc::error::TrySendError::Full(_)) => {
            pending_events.push_back(event);
            if !is_application {
                *pending_lifecycle_events += 1;
            }
            EventQueueOutcome::Queued
        }
    }
}

pub(super) fn primary_event_queue_size(events: &mpsc::Sender<EventEnvelope>, pending_events: usize) -> (usize, usize) {
    let channel_capacity = events.max_capacity().saturating_sub(PRIMARY_TERMINAL_EVENT_CAPACITY);
    let channel_occupancy = events.max_capacity().saturating_sub(events.capacity()).saturating_sub(PRIMARY_TERMINAL_EVENT_CAPACITY);
    (channel_occupancy + pending_events, channel_capacity + MAX_PENDING_SESSION_EVENTS)
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, net::SocketAddr, sync::Arc, time::Duration};

    use llsd::Value;

    use crate::event::{Event, EventEnvelope, SimulatorId};
    use tokio::{
        sync::{Semaphore, mpsc},
        time::Instant,
    };

    use super::{
        MAX_PENDING_LIFECYCLE_EVENTS, MAX_PENDING_SESSION_EVENTS, PrimaryEventBacklog, primary_event_queue_size, queue_simulator_event,
    };
    use crate::session::EVENT_BACKLOG_WARNING_THRESHOLD;

    #[test]
    fn backlog_warning_is_rate_limited() {
        let start = Instant::now();
        let mut backlog = PrimaryEventBacklog::default();

        assert!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD - 1, start).is_none());
        assert_eq!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start + Duration::from_millis(3)), Some(3));
        assert!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start + Duration::from_secs(10)).is_none());
        assert_eq!(
            backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start + Duration::from_secs(10) + Duration::from_millis(3)),
            Some(10_003)
        );
    }

    #[test]
    fn backlog_timer_resets_after_drain() {
        let start = Instant::now();
        let mut backlog = PrimaryEventBacklog::default();

        assert_eq!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start), Some(0));
        backlog.observe(0, start + Duration::from_secs(1));
        assert_eq!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start + Duration::from_secs(11)), Some(0));
    }

    #[tokio::test]
    async fn event_queue_reserves_terminal_slot() {
        let (events, _received) = mpsc::channel(4);
        let _terminal_slot = events.clone().reserve_owned().await.expect("reserve terminal event slot");
        events.try_send(EventEnvelope::new(Event::Failed("queued".into()))).expect("queue an application event");

        assert_eq!(
            primary_event_queue_size(&events, 3),
            (4, 3 + MAX_PENDING_SESSION_EVENTS),
            "occupancy excludes the terminal reservation and includes the pending deque"
        );
    }

    #[test]
    fn lifecycle_events_use_bounded_allowance() {
        let (events, mut received) = mpsc::channel(1);
        events.try_send(EventEnvelope::new(lifecycle_event(0))).expect("fill the channel");
        let mut pending = VecDeque::with_capacity(MAX_PENDING_SESSION_EVENTS);
        let mut pending_lifecycle_events = 0;
        for index in 0..crate::circuit::admission::SESSION_APPLICATION_EVENT_CREDIT_CAPACITY {
            std::assert_matches!(
                queue_simulator_event(&events, &mut pending, &mut pending_lifecycle_events, EventEnvelope::new(application_event(index)),),
                super::EventQueueOutcome::Queued
            );
        }
        for index in 1..=MAX_PENDING_LIFECYCLE_EVENTS {
            std::assert_matches!(
                queue_simulator_event(&events, &mut pending, &mut pending_lifecycle_events, EventEnvelope::new(lifecycle_event(index)),),
                super::EventQueueOutcome::Queued
            );
        }
        std::assert_matches!(
            queue_simulator_event(
                &events,
                &mut pending,
                &mut pending_lifecycle_events,
                EventEnvelope::new(lifecycle_event(MAX_PENDING_LIFECYCLE_EVENTS + 1)),
            ),
            super::EventQueueOutcome::LifecycleBacklogFull
        );
        assert_eq!(pending.len(), MAX_PENDING_SESSION_EVENTS);
        assert_eq!(pending_lifecycle_events, MAX_PENDING_LIFECYCLE_EVENTS);
        for (index, event) in pending.iter().take(crate::circuit::admission::SESSION_APPLICATION_EVENT_CREDIT_CAPACITY).enumerate() {
            assert_application_event(&event.event, index);
        }
        std::assert_matches!(pending.back().map(|event| &event.event), Some(Event::SimulatorConnected { simulator_id, .. }) if *simulator_id == SimulatorId::new(33));
        let first = received.try_recv().expect("the channel event remains in front of the pending queue");
        std::assert_matches!(first.event, Event::SimulatorConnected { simulator_id, .. } if simulator_id == SimulatorId::new(1));
    }

    #[test]
    fn application_credit_is_released_at_viewer_channel_handoff() {
        let credits = Arc::new(Semaphore::new(1));
        let permit = Arc::clone(&credits).try_acquire_owned().expect("admit one application event");
        let mut event = EventEnvelope::new(application_event(0));
        event.attach_application_credit(permit);
        let (events, mut received) = mpsc::channel(1);
        let mut pending = VecDeque::new();
        let mut pending_lifecycle_events = 0;

        std::assert_matches!(
            queue_simulator_event(&events, &mut pending, &mut pending_lifecycle_events, event),
            super::EventQueueOutcome::Queued
        );

        assert_eq!(credits.available_permits(), 1);
        assert_application_event(&received.try_recv().expect("event remains in the viewer channel").event, 0);
    }

    fn application_event(index: usize) -> Event {
        Event::CapabilityEvent {
            simulator_id: SimulatorId::new(1),
            source: SocketAddr::from(([127, 0, 0, 1], 9000)),
            message: "Test".into(),
            body: Value::Integer(index.try_into().expect("test index fits LLSD integer")),
        }
    }

    fn lifecycle_event(index: usize) -> Event {
        Event::SimulatorConnected {
            simulator_id: SimulatorId::new(u64::try_from(index + 1).expect("test simulator ID fits")),
            region_handle: None,
            endpoint: SocketAddr::from(([127, 0, 0, 1], 9000)),
        }
    }

    fn assert_application_event(event: &Event, expected: usize) {
        let Event::CapabilityEvent { body: Value::Integer(value), .. } = event else { panic!("expected an application event") };
        assert_eq!(usize::try_from(*value).expect("test event index is nonnegative"), expected);
    }
}
