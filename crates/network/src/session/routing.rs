use std::collections::{HashMap, HashSet, VecDeque};

use protocol::lludp::Message;
use tokio::{sync::mpsc, time::Instant};

use super::simulators::MAX_PENDING_REGION_HANDSHAKE_REPLIES;
use super::teleport::is_teleport_cancel;
use super::{
    ConnectionQueueFailure, EVENT_BACKLOG_WARNING_INTERVAL, EVENT_BACKLOG_WARNING_THRESHOLD, MAX_PENDING_LIFECYCLE_EVENTS,
    MAX_PENDING_SESSION_EVENTS, MAX_PRECONNECT_EVENTS, RuntimeAction, SessionDispatcher, SessionError, SimulatorCommand,
    queue_connection_message, queue_primary_message,
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
    pub(super) fn flush_pending_handshake_replies(&mut self) {
        if self.run_state.pending_region_handshake_replies.is_empty() {
            return;
        }
        let mut pending = std::mem::take(&mut self.run_state.pending_region_handshake_replies);
        flush_handshake_reply_queue(&mut pending, |simulator_id| self.simulator_commands(simulator_id));
        self.run_state.pending_region_handshake_replies = pending;
    }

    fn simulator_commands(&self, simulator_id: SimulatorId) -> Option<mpsc::Sender<SimulatorCommand>> {
        if simulator_id == self.state.primary.simulator_id && !self.state.root_connection_failed {
            return Some(self.state.primary.commands.clone());
        }
        self.run_state
            .neighbor_connections
            .values()
            .find(|connection| connection.simulator_id == simulator_id)
            .map(|connection| connection.commands.clone())
    }

    fn has_pending_handshake_reply(&self, simulator_id: SimulatorId) -> bool {
        self.run_state.pending_region_handshake_replies.iter().any(|(pending_id, _)| *pending_id == simulator_id)
    }

    fn queue_handshake_reply(&mut self, simulator_id: SimulatorId, command: SimulatorCommand) {
        let commands = self.simulator_commands(simulator_id);
        if queue_handshake_reply(commands.as_ref(), &mut self.run_state.pending_region_handshake_replies, simulator_id, command) {
            self.abort_simulator_connection(simulator_id);
        }
    }

    fn abort_simulator_connection(&self, simulator_id: SimulatorId) {
        let abort = self
            .run_state
            .neighbor_connections
            .values()
            .find(|connection| connection.simulator_id == simulator_id)
            .map(|connection| connection.abort.clone())
            .or_else(|| (simulator_id == self.state.simulator_id).then(|| self.state.root_connection_abort.clone()).flatten());
        if let Some(abort) = abort {
            abort.abort();
        }
    }

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
                if self.has_pending_handshake_reply(self.state.primary.simulator_id) {
                    self.state
                        .publish_event(&self.events, &mut self.run_state, Event::SendFailed(Error::SimulatorCommandQueueFull.to_string()))
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
                } else if self.has_pending_handshake_reply(simulator_id) {
                    Err(ConnectionQueueFailure { error: Error::SimulatorCommandQueueFull, completion: completion.take() })
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
            Some(Command::ReplyRegionHandshake { simulator_id, flags, completed }) => {
                let message = Message::RegionHandshakeReply(protocol::messages::RegionHandshakeReply {
                    agent_data: protocol::messages::region::region_handshake_reply::AgentData {
                        agent_id: self.state.session.agent_id,
                        session_id: self.state.session.session_id,
                    },
                    region_info: protocol::messages::region::region_handshake_reply::RegionInfo { flags },
                });
                let command = SimulatorCommand::ReplyRegionHandshake { message, completed };
                self.queue_handshake_reply(simulator_id, command);
            }
            None => {
                self.state.graceful_disconnect(self.state.logout_request_sent).await?;
                return Ok(RuntimeAction::Exit(Ok(())));
            }
        }
        Ok(RuntimeAction::Continue)
    }
}

fn complete_handshake_reply(command: SimulatorCommand, error: Error) {
    if let SimulatorCommand::ReplyRegionHandshake { completed, .. } = command
        && let Err(error) = completed.send(Err(error))
    {
        drop(error);
    }
}

fn queue_handshake_reply(
    commands: Option<&mpsc::Sender<SimulatorCommand>>,
    pending: &mut VecDeque<(SimulatorId, SimulatorCommand)>,
    simulator_id: SimulatorId,
    command: SimulatorCommand,
) -> bool {
    if pending.iter().any(|(pending_id, _)| *pending_id == simulator_id) {
        return defer_handshake_reply(pending, simulator_id, command);
    }
    let Some(commands) = commands else {
        complete_handshake_reply(command, Error::SimulatorUnavailable);
        return false;
    };
    match commands.try_send(command) {
        Ok(()) => false,
        Err(mpsc::error::TrySendError::Full(command)) => defer_handshake_reply(pending, simulator_id, command),
        Err(mpsc::error::TrySendError::Closed(command)) => {
            complete_handshake_reply(command, Error::SimulatorConnectionClosed);
            false
        }
    }
}

fn defer_handshake_reply(
    pending: &mut VecDeque<(SimulatorId, SimulatorCommand)>,
    simulator_id: SimulatorId,
    command: SimulatorCommand,
) -> bool {
    if pending.len() < MAX_PENDING_REGION_HANDSHAKE_REPLIES {
        pending.push_back((simulator_id, command));
        false
    } else {
        complete_handshake_reply(command, Error::SimulatorCommandQueueFull);
        true
    }
}

fn flush_handshake_reply_queue(
    pending: &mut VecDeque<(SimulatorId, SimulatorCommand)>,
    mut simulator_commands: impl FnMut(SimulatorId) -> Option<mpsc::Sender<SimulatorCommand>>,
) {
    let pending_count = pending.len();
    let mut blocked = HashSet::new();
    for _ in 0..pending_count {
        let Some((simulator_id, command)) = pending.pop_front() else { break };
        if blocked.contains(&simulator_id) {
            pending.push_back((simulator_id, command));
            continue;
        }
        let Some(commands) = simulator_commands(simulator_id) else {
            complete_handshake_reply(command, Error::SimulatorUnavailable);
            continue;
        };
        match commands.try_send(command) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(command)) => {
                blocked.insert(simulator_id);
                pending.push_back((simulator_id, command));
            }
            Err(mpsc::error::TrySendError::Closed(command)) => {
                complete_handshake_reply(command, Error::SimulatorConnectionClosed);
            }
        }
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

    use crate::circuit::SimulatorCommand;
    use crate::event::{Event, EventEnvelope, SimulatorId};
    use tokio::{
        sync::{Semaphore, mpsc, oneshot},
        time::Instant,
    };

    use super::{
        MAX_PENDING_LIFECYCLE_EVENTS, MAX_PENDING_REGION_HANDSHAKE_REPLIES, MAX_PENDING_SESSION_EVENTS, PrimaryEventBacklog,
        flush_handshake_reply_queue, primary_event_queue_size, queue_handshake_reply, queue_simulator_event,
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

    #[tokio::test]
    async fn handshake_backlog_preserves_order() {
        let simulator_id = SimulatorId::new(1);
        let (commands, mut received) = mpsc::channel(1);
        commands.try_send(SimulatorCommand::CancelHandoff).expect("fill the simulator command queue");
        let mut pending = VecDeque::new();
        let (first_completed, first_response) = oneshot::channel();
        let (second_completed, second_response) = oneshot::channel();

        assert!(!queue_handshake_reply(Some(&commands), &mut pending, simulator_id, handshake_reply(first_completed)));
        assert!(!queue_handshake_reply(Some(&commands), &mut pending, simulator_id, handshake_reply(second_completed)));
        assert_eq!(pending.len(), 2);

        let Ok(SimulatorCommand::CancelHandoff) = received.try_recv() else {
            panic!("expected queued handoff cancellation");
        };
        flush_handshake_reply_queue(&mut pending, |id| (id == simulator_id).then(|| commands.clone()));
        assert_eq!(pending.len(), 1, "one reply fits in the freed queue slot");
        let SimulatorCommand::ReplyRegionHandshake { completed, .. } = received.try_recv().expect("first reply keeps its place") else {
            panic!("expected first handshake reply");
        };
        completed.send(Ok(())).expect("first reply receiver remains open");
        first_response.await.expect("first reply completes").expect("first reply succeeds");

        flush_handshake_reply_queue(&mut pending, |id| (id == simulator_id).then(|| commands.clone()));
        let SimulatorCommand::ReplyRegionHandshake { completed, .. } = received.try_recv().expect("second reply follows first") else {
            panic!("expected second handshake reply");
        };
        completed.send(Ok(())).expect("second reply receiver remains open");
        second_response.await.expect("second reply completes").expect("second reply succeeds");
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn handshake_backlog_is_bounded() {
        let simulator_id = SimulatorId::new(1);
        let (commands, _received) = mpsc::channel(1);
        commands.try_send(SimulatorCommand::CancelHandoff).expect("fill the simulator command queue");
        let mut pending = VecDeque::new();
        let mut overflow = None;

        for index in 0..=MAX_PENDING_REGION_HANDSHAKE_REPLIES {
            let (completed, response) = oneshot::channel();
            let overflowed = queue_handshake_reply(Some(&commands), &mut pending, simulator_id, handshake_reply(completed));
            if index == MAX_PENDING_REGION_HANDSHAKE_REPLIES {
                assert!(overflowed);
                overflow = Some(response);
            }
        }

        assert_eq!(pending.len(), MAX_PENDING_REGION_HANDSHAKE_REPLIES);
        std::assert_matches!(overflow.expect("the extra reply is rejected").await, Ok(Err(crate::error::Error::SimulatorCommandQueueFull)));
    }

    fn handshake_reply(completed: oneshot::Sender<Result<(), crate::error::Error>>) -> SimulatorCommand {
        SimulatorCommand::ReplyRegionHandshake {
            message: protocol::lludp::Message::RegionHandshakeReply(protocol::messages::RegionHandshakeReply {
                agent_data: protocol::messages::region::region_handshake_reply::AgentData {
                    agent_id: uuid::Uuid::nil(),
                    session_id: uuid::Uuid::nil(),
                },
                region_info: protocol::messages::region::region_handshake_reply::RegionInfo { flags: 0 },
            }),
            completed,
        }
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
