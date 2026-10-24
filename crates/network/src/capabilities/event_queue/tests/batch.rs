use std::{
    future::Future,
    pin::pin,
    task::{Context, Waker},
};

use super::*;
use crate::coverage::CoverageLoss;
use crate::fixtures::test_simulator_id;

fn reserve_batch(budget: &Arc<Semaphore>) -> OwnedSemaphorePermit {
    budget.clone().try_acquire_many_owned(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES as u32).expect("reserve one EventQueue batch")
}

fn source() -> CoverageSource {
    CoverageSource::EventQueue { incarnation: 42 }
}

fn event(message: &str) -> CapabilityEvent {
    let mut body = Value::Undefined;
    let retained_bytes = queued_event_retained_size(message.len(), &mut body);
    CapabilityEvent { message: message.into(), body, retained_bytes }
}

fn pending_loss(ledger: &CoverageLossLedger, simulator_id: SimulatorId) -> PendingEventQueueLoss {
    let (events, received) = mpsc::channel(1);
    drop(received);
    let (_shutdown, shutdown_receiver) = watch::channel(false);
    let (_session_closed, session_closed_receiver) = watch::channel(false);
    let stop = Stop::new(shutdown_receiver, session_closed_receiver);
    let notifier = EventQueueLossNotifier::new(events, simulator_id, SocketAddr::from(([127, 0, 0, 1], 0)), stop);
    PendingEventQueueLoss::new(ledger.clone(), simulator_id, source(), notifier)
}

async fn send_batch(
    events: &mpsc::Sender<EventEnvelope>,
    simulator_id: SimulatorId,
    batch: Vec<CapabilityEvent>,
    shutdown: &watch::Receiver<bool>,
    session_closed: &watch::Receiver<bool>,
    budget: &Arc<Semaphore>,
    ledger: &CoverageLossLedger,
) -> EventSendOutcome {
    let mut pending = pending_loss(ledger, simulator_id);
    let stop = Stop::new(shutdown.clone(), session_closed.clone());
    send_event_queue_batch(
        events,
        simulator_id,
        SocketAddr::from(([127, 0, 0, 1], 0)),
        source(),
        0,
        batch,
        &stop,
        reserve_batch(budget),
        ledger,
        &mut pending,
    )
    .await
}

#[test]
fn decodes_second_life_event_body_as_message_blocks() {
    let body = br#"<llsd><map>
          <key>id</key><integer>9</integer>
          <key>events</key><array><map>
            <key>message</key><string>CrossedRegion</string>
            <key>body</key><map>
              <key>AgentData</key><array><map>
                <key>AgentID</key><uuid>00000000-0000-0000-0000-000000000001</uuid>
                <key>SessionID</key><uuid>00000000-0000-0000-0000-000000000002</uuid>
              </map></array>
              <key>RegionData</key><array><map>
                <key>RegionHandle</key><binary encoding="base64">AAAAAAAAEjQ=</binary>
                <key>SimIP</key><binary encoding="base64">wAACCg==</binary>
                <key>SimPort</key><integer>13000</integer>
                <key>SeedCapability</key><uri>https://example.test/seed</uri>
              </map></array>
              <key>Info</key><array><map></map></array>
            </map>
          </map></array>
        </map></llsd>"#;

    let (id, events) = decode_event_batch(body).expect("decode EventQueue response");
    assert_eq!(id, 9);
    assert_eq!(events.len(), 1);
    let Value::Map(fields) = &events[0].body else { panic!("EventQueue body is a block map") };
    std::assert_matches!(fields.get("AgentData"), Some(Value::Array(_)));
    std::assert_matches!(fields.get("RegionData"), Some(Value::Array(_)));
    std::assert_matches!(fields.get("Info"), Some(Value::Array(_)));
    assert!(!fields.contains_key("CrossedRegion"), "message name is carried beside body, not nested inside it");
}

#[test]
fn closed_session_or_shutdown_skips_batch_reservation() {
    for close_session in [false, true] {
        let budget = Arc::new(Semaphore::new(MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES));
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (session_closed, session_closed_receiver) = watch::channel(false);
        if close_session {
            session_closed.send_replace(true);
        } else {
            shutdown.send_replace(true);
        }

        let stop = Stop::new(shutdown_receiver, session_closed_receiver);
        assert!(acquire_event_batch_reservation(Arc::clone(&budget), &stop).is_none());
        assert_eq!(budget.available_permits(), MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES);
    }
}

#[tokio::test]
async fn recorded_loss_survives_commit() {
    let (events, mut received) = mpsc::channel(1);
    events.try_send(EventEnvelope::new(Event::Failed("occupied".into()))).expect("fill event channel");
    let (_shutdown, shutdown) = watch::channel(false);
    let (_session_closed, session_closed) = watch::channel(false);
    let ledger = CoverageLossLedger::new();
    let simulator_id = test_simulator_id(3);
    let stop = Stop::new(shutdown, session_closed);
    let notifier = EventQueueLossNotifier::new(events, simulator_id, SocketAddr::from(([127, 0, 0, 1], 0)), stop);
    let mut pending = PendingEventQueueLoss::new(ledger.clone(), simulator_id, source(), notifier);
    {
        let mut notification = pin!(pending.notify("admission failed".into()));
        assert!(notification.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    }
    pending.commit();
    drop(pending);

    std::assert_matches!(received.recv().await.map(EventEnvelope::into_event), Some(Event::Failed(_)));
    std::assert_matches!(
        received.recv().await.map(EventEnvelope::into_event),
        Some(Event::EventQueueCoverageLost { simulator_id: id, reason, .. }) if id == simulator_id && reason == "admission failed"
    );
    assert!(received.recv().await.is_none(), "one recorded loss emits one notification");
    std::assert_matches!(
        ledger.snapshot().losses.as_slice(),
        [CoverageLoss::Source(loss)] if loss.watermark.generation == 1 && loss.reason == CoverageLossReason::EventQueueAdmission
    );
}

#[tokio::test]
async fn cancelled_batch_records_loss_without_prefix() {
    let (events, mut received) = mpsc::channel(1);
    events.try_send(EventEnvelope::new(Event::Failed("occupy the event slot".into()))).expect("fill event channel");
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let (_session_closed, session_closed_receiver) = watch::channel(false);
    let queued_event_bytes = Arc::new(Semaphore::new(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES));
    let ledger = CoverageLossLedger::new();
    let changed = ledger.subscribe();
    let mut admission = pin!(send_batch(
        &events,
        test_simulator_id(1),
        vec![event("First"), event("Second")],
        &shutdown_receiver,
        &session_closed_receiver,
        &queued_event_bytes,
        &ledger,
    ));
    assert!(admission.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    shutdown.send_replace(true);
    assert!(matches!(admission.await, EventSendOutcome::Shutdown));
    let mut changed = changed;
    changed.changed().await.expect("durable loss wakes ledger subscribers");
    std::assert_matches!(
        ledger.snapshot().losses.as_slice(),
        [CoverageLoss::Source(loss)]
            if loss.watermark.source == source() && loss.reason == CoverageLossReason::EventQueueAdmission
    );
    std::assert_matches!(received.try_recv().map(EventEnvelope::into_event), Ok(Event::Failed(_)));
    assert!(received.try_recv().is_err(), "no event from a partial batch may escape");
    assert_eq!(queued_event_bytes.available_permits(), MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES);
}

#[tokio::test]
async fn dropping_waiting_batch_admission_records_loss() {
    let (events, mut received) = mpsc::channel(1);
    events.try_send(EventEnvelope::new(Event::Failed("occupy the event slot".into()))).expect("fill event channel");
    let (_shutdown, shutdown_receiver) = watch::channel(false);
    let (_session_closed, session_closed_receiver) = watch::channel(false);
    let queued_event_bytes = Arc::new(Semaphore::new(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES));
    let ledger = CoverageLossLedger::new();
    {
        let mut admission = pin!(send_batch(
            &events,
            test_simulator_id(3),
            vec![event("First"), event("Second")],
            &shutdown_receiver,
            &session_closed_receiver,
            &queued_event_bytes,
            &ledger,
        ));
        assert!(admission.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    }
    std::assert_matches!(
        ledger.snapshot().losses.as_slice(),
        [CoverageLoss::Source(loss)] if loss.reason == CoverageLossReason::EventQueueAdmission
    );
    std::assert_matches!(received.try_recv().map(EventEnvelope::into_event), Ok(Event::Failed(_)));
    assert!(received.try_recv().is_err(), "cancellation cannot expose a batch prefix");
    assert_eq!(queued_event_bytes.available_permits(), MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES);
}

#[tokio::test]
async fn accepts_event_at_size_limit_as_one_batch_event() {
    let (events, mut received) = mpsc::channel(1);
    let (_shutdown, shutdown_receiver) = watch::channel(false);
    let (_session_closed, session_closed_receiver) = watch::channel(false);
    let queued_event_bytes = Arc::new(Semaphore::new(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES));
    let ledger = CoverageLossLedger::new();
    let mut at_limit = event("limit");
    at_limit.retained_bytes = MAX_QUEUED_CAPABILITY_EVENT_BYTES;
    let expected_bytes = capability_event_batch_retained_size(from_ref(&at_limit), 1);

    assert!(matches!(
        send_batch(
            &events,
            test_simulator_id(1),
            vec![at_limit],
            &shutdown_receiver,
            &session_closed_receiver,
            &queued_event_bytes,
            &ledger,
        )
        .await,
        EventSendOutcome::Sent
    ));
    let envelope = received.recv().await.expect("one batch envelope");
    std::assert_matches!(
        &envelope.event,
        Event::CapabilityEventBatch { order: EventQueueBatchOrder { incarnation: 42, ordinal: 0 }, events, .. } if events.len() == 1
    );
    assert_eq!(queued_event_bytes.available_permits(), MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES - expected_bytes);
    drop(envelope);
    assert_eq!(queued_event_bytes.available_permits(), MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES);
}

#[tokio::test]
async fn event_batch_admission_is_atomic() {
    let (events, mut received) = mpsc::channel(1);
    events.try_send(EventEnvelope::new(Event::Failed("occupied".into()))).expect("fill event channel");
    let (_shutdown, shutdown_receiver) = watch::channel(false);
    let (_session_closed, session_closed_receiver) = watch::channel(false);
    let queued_event_bytes = Arc::new(Semaphore::new(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES));
    let ledger = CoverageLossLedger::new();
    let batch = vec![event("First"), event("Second")];
    let expected_bytes = capability_event_batch_retained_size(&batch, batch.capacity());
    let mut admission =
        pin!(send_batch(&events, test_simulator_id(1), batch, &shutdown_receiver, &session_closed_receiver, &queued_event_bytes, &ledger,));
    assert!(admission.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    assert_eq!(queued_event_bytes.available_permits(), MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES - expected_bytes);
    std::assert_matches!(received.recv().await.map(EventEnvelope::into_event), Some(Event::Failed(_)));
    assert!(matches!(admission.await, EventSendOutcome::Sent));
    let envelope = received.recv().await.expect("one complete batch envelope");
    let Event::CapabilityEventBatch { events: batch, .. } = &envelope.event else {
        panic!("the EventQueue response must be one application event")
    };
    assert_eq!(batch.iter().map(|event| event.message.as_str()).collect::<Vec<_>>(), ["First", "Second"]);
    assert_eq!(
        envelope.coverage_watermark(),
        Some(ledger.watermark(test_simulator_id(1), source())),
        "the batch carries its source watermark"
    );
    assert!(received.try_recv().is_err(), "one batch is one Network intake item");
    assert_eq!(queued_event_bytes.available_permits(), MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES - expected_bytes);
    drop(envelope);
    assert_eq!(queued_event_bytes.available_permits(), MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES);
}

#[test]
fn event_budget_retries_after_dequeue() {
    let queued_event_bytes = Arc::new(Semaphore::new(MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES));
    let mut reservations = Vec::with_capacity(MAX_CONCURRENT_EVENT_QUEUES);
    for _ in 0..MAX_CONCURRENT_EVENT_QUEUES {
        reservations.push(reserve_batch(&queued_event_bytes));
    }
    assert_eq!(queued_event_bytes.available_permits(), 0);

    let (_shutdown, shutdown_receiver) = watch::channel(false);
    let (_session_closed, session_closed_receiver) = watch::channel(false);
    let stop = Stop::new(shutdown_receiver, session_closed_receiver);
    assert!(acquire_event_batch_reservation(Arc::clone(&queued_event_bytes), &stop).is_none());

    drop(reservations.pop());
    let (_shutdown, shutdown_receiver) = watch::channel(false);
    let (_session_closed, session_closed_receiver) = watch::channel(false);
    let stop = Stop::new(shutdown_receiver, session_closed_receiver);
    let permit =
        acquire_event_batch_reservation(Arc::clone(&queued_event_bytes), &stop).expect("released batch capacity should admit another poll");
    assert_eq!(queued_event_bytes.available_permits(), 0);
    drop(permit);
    drop(reservations);
    assert_eq!(queued_event_bytes.available_permits(), MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES);
}

#[tokio::test]
async fn receiver_close_records_loss_and_releases_batch_bytes() {
    let (events, received) = mpsc::channel(1);
    let (_shutdown, shutdown_receiver) = watch::channel(false);
    let (_session_closed, session_closed_receiver) = watch::channel(false);
    let queued_event_bytes = Arc::new(Semaphore::new(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES));
    let ledger = CoverageLossLedger::new();
    events.try_send(EventEnvelope::new(Event::Failed("hold the only downstream slot".into()))).expect("fill event channel");
    let mut admission = pin!(send_batch(
        &events,
        test_simulator_id(1),
        vec![event("test")],
        &shutdown_receiver,
        &session_closed_receiver,
        &queued_event_bytes,
        &ledger,
    ));
    assert!(admission.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    drop(received);
    assert!(matches!(admission.await, EventSendOutcome::ReceiverClosed));
    std::assert_matches!(
        ledger.snapshot().losses.as_slice(),
        [CoverageLoss::Source(loss)] if loss.reason == CoverageLossReason::EventQueueAdmission
    );
    assert_eq!(queued_event_bytes.available_permits(), MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES);
}
