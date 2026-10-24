use std::time::Duration;

use crate::event::{Event, EventEnvelope};
use tokio::{sync::mpsc, time::Instant};

use super::{PrimaryEventBacklog, primary_event_queue_size};
use crate::circuit::MAX_QUEUED_SIMULATOR_EVENTS;
use crate::session::EVENT_BACKLOG_WARNING_THRESHOLD;

#[test]
fn backlog_warning_is_rate_limited() {
    let start = Instant::now();
    let mut backlog = PrimaryEventBacklog::default();

    assert!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD - 1, start).is_none());
    assert_eq!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start + Duration::from_millis(3)), Some(3));
    assert!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start + Duration::from_secs(10)).is_none());
    assert_eq!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start + Duration::from_secs(10) + Duration::from_millis(3)), Some(10_003));
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
