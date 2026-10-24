use std::collections::VecDeque;

use tokio::sync::mpsc;

use crate::{
    circuit::{MAX_QUEUED_SIMULATOR_EVENTS, NeighborEventQueueOutcome, SimulatorEvent, queue_neighbor_event},
    event::Event,
};

#[test]
fn full_neighbor_queue_keeps_teardown_open() {
    let (events, receiver) = mpsc::channel(1);
    events.try_send(SimulatorEvent::Simulator(Event::Failed("hold event".into()).into())).expect("fill application event channel");
    let mut pending = VecDeque::with_capacity(MAX_QUEUED_SIMULATOR_EVENTS);
    for _ in 0..MAX_QUEUED_SIMULATOR_EVENTS {
        pending.push_back(Event::Failed("pending event".into()).into());
    }
    assert_eq!(queue_neighbor_event(&events, &mut pending, Event::Failed("overflow".into())), NeighborEventQueueOutcome::Closing);
    assert_eq!(pending.len(), MAX_QUEUED_SIMULATOR_EVENTS);

    drop(receiver);
    assert_eq!(
        queue_neighbor_event(&events, &mut pending, Event::Failed("receiver closed".into())),
        NeighborEventQueueOutcome::ReceiverClosed
    );
    assert_eq!(pending.len(), MAX_QUEUED_SIMULATOR_EVENTS);
}
