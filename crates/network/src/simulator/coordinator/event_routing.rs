use std::collections::{HashMap, HashSet, VecDeque};

use tokio::{sync::mpsc, time::Instant};

use super::{
    EVENT_BACKLOG_WARNING_INTERVAL, EVENT_BACKLOG_WARNING_THRESHOLD, MAX_PENDING_SIMULATOR_EVENTS, MAX_PRECONNECT_EVENTS,
    MAX_QUEUED_SIMULATOR_EVENTS, SimulatorEvent,
};
use crate::event::{EventEnvelope, PRIMARY_TERMINAL_EVENT_CAPACITY, SimulatorId};

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

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NeighborEventQueueOutcome {
    Queued,
    Closing,
    ReceiverClosed,
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

pub(super) fn queue_neighbor_event(
    events: &mpsc::Sender<SimulatorEvent>,
    pending_events: &mut VecDeque<EventEnvelope>,
    event: impl Into<EventEnvelope>,
) -> NeighborEventQueueOutcome {
    match queue_neighbor_channel_event(events, pending_events, event.into()) {
        EventQueueOutcome::Queued => {
            if pending_events.len() >= MAX_PENDING_SIMULATOR_EVENTS {
                NeighborEventQueueOutcome::Closing
            } else {
                NeighborEventQueueOutcome::Queued
            }
        }
        EventQueueOutcome::ReceiverClosed => NeighborEventQueueOutcome::ReceiverClosed,
        EventQueueOutcome::BacklogFull => NeighborEventQueueOutcome::Closing,
    }
}

fn queue_neighbor_channel_event(
    events: &mpsc::Sender<SimulatorEvent>,
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

    match events.try_send(SimulatorEvent::Simulator(event)) {
        Ok(()) => EventQueueOutcome::Queued,
        Err(mpsc::error::TrySendError::Closed(_)) => EventQueueOutcome::ReceiverClosed,
        Err(mpsc::error::TrySendError::Full(SimulatorEvent::Simulator(event))) => {
            pending_events.push_back(event);
            EventQueueOutcome::Queued
        }
        Err(mpsc::error::TrySendError::Full(SimulatorEvent::SendCompleted { .. } | SimulatorEvent::Finished { .. })) => {
            unreachable!("only simulator events enter the neighbor event backlog")
        }
    }
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
mod tests {
    use std::{collections::VecDeque, time::Duration};

    use crate::event::{Event, EventEnvelope};
    use tokio::{sync::mpsc, time::Instant};

    use super::{NeighborEventQueueOutcome, PrimaryEventBacklog, primary_event_queue_size, queue_neighbor_event};
    use crate::simulator::coordinator::{EVENT_BACKLOG_WARNING_THRESHOLD, MAX_QUEUED_SIMULATOR_EVENTS, SimulatorEvent};

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
            (4, 3 + MAX_QUEUED_SIMULATOR_EVENTS),
            "occupancy excludes the terminal reservation and includes the pending deque"
        );
    }

    #[test]
    fn full_neighbor_queue_keeps_teardown_open() {
        let (events, receiver) = mpsc::channel(1);
        events.try_send(SimulatorEvent::Simulator(Event::Failed("hold event".into()).into())).expect("fill application event channel");
        let mut pending = VecDeque::with_capacity(MAX_QUEUED_SIMULATOR_EVENTS);
        for _ in 0..MAX_QUEUED_SIMULATOR_EVENTS {
            pending.push_back(Event::Failed("pending event".into()).into());
        }
        assert_eq!(queue_neighbor_event(&events, &mut pending, Event::Failed("overflow".into()),), NeighborEventQueueOutcome::Closing);
        assert_eq!(pending.len(), MAX_QUEUED_SIMULATOR_EVENTS);

        drop(receiver);
        assert_eq!(
            queue_neighbor_event(&events, &mut pending, Event::Failed("receiver closed".into())),
            NeighborEventQueueOutcome::ReceiverClosed
        );
        assert_eq!(pending.len(), MAX_QUEUED_SIMULATOR_EVENTS);
    }
}
