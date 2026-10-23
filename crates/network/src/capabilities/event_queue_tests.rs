use std::{future::Future, net::SocketAddr};
use tokio::task::yield_now;
use tokio::time::timeout;

use super::*;

const TEST_TIMEOUT: Duration = Duration::from_secs(2);

#[tokio::test]
async fn completion_survives_cancelled_polling_and_repeated_reads() {
    for exit in [Some(EventQueueExit::StoppedWithCoverageLoss), None] {
        let (completed, completion) = oneshot::channel();
        let (role, _) = EventQueueRoleHandle::channel(true);
        let mut task = EventQueueTask::new(tokio::spawn(async {}), completion, role);
        tokio::select! {
            biased;
            exit = task.completion() => panic!("completion is not ready: {exit:?}"),
            _ = async {} => {},
        }

        if let Some(exit) = exit {
            completed.send(exit).expect("completion receiver survives polling cancellation");
        } else {
            drop(completed);
        }
        let expected = exit.unwrap_or(EventQueueExit::TaskFailed);
        assert_eq!(task.completion().await, expected);
        assert_eq!(task.completion().await, expected);
        assert_eq!(task.wait().await, expected);
    }
}

#[tokio::test]
async fn cancelling_wait_aborts_the_worker_after_completion_is_reported() {
    use std::{
        future::pending,
        pin::pin,
        task::{Context, Waker},
    };

    let (worker_lifetime, worker_dropped) = oneshot::channel::<()>();
    let worker = tokio::spawn(async move {
        let _lifetime = worker_lifetime;
        pending::<()>().await;
    });
    let (completed, completion) = oneshot::channel();
    completed.send(EventQueueExit::Stopped).expect("report completion before the worker exits");
    let (role, _) = EventQueueRoleHandle::channel(true);
    let task = EventQueueTask::new(worker, completion, role);
    {
        let mut wait = pin!(task.wait());
        assert!(wait.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    }

    assert!(timeout(TEST_TIMEOUT, worker_dropped).await.expect("cancelled wait must abort its worker").is_err());
}

#[test]
fn poll_retry_after_hint_is_bounded() {
    assert_eq!(event_queue_retry_delay_with_hint(2, None), Duration::from_secs(7));
    assert_eq!(event_queue_retry_delay_with_hint(2, Some(Duration::from_secs(5))), Duration::from_secs(5));
    assert_eq!(event_queue_retry_delay_with_hint(2, Some(Duration::from_secs(90))), Duration::from_secs(30));
}

async fn close_while_pending<F>(future: F, session_closed: &watch::Sender<bool>) -> RetryOutcome
where
    F: Future<Output = RetryOutcome>,
{
    tokio::pin!(future);
    tokio::select! {
        biased;
        outcome = &mut future => return outcome,
        _ = yield_now() => {}
    }
    session_closed.send_replace(true);
    timeout(TEST_TIMEOUT, &mut future).await.expect("session close cancels retry")
}

#[tokio::test]
async fn poll_backoff_stops_when_session_closes() {
    let (events, _received) = mpsc::channel(1);
    let (_shutdown, mut shutdown) = watch::channel(false);
    let (session_closed, session_closed_receiver) = watch::channel(false);
    let mut error_count = 0;

    assert_eq!(
        close_while_pending(
            retry_event_queue(
                SimulatorId::new(1),
                &mut error_count,
                SocketAddr::from(([127, 0, 0, 1], 0)),
                &events,
                "poll failed".into(),
                &mut shutdown,
                &session_closed_receiver,
            ),
            &session_closed,
        )
        .await,
        RetryOutcome::Stopped
    );
}

#[tokio::test]
async fn seed_backoff_stops_when_session_closes() {
    let (events, _received) = mpsc::channel(1);
    let (_shutdown, mut shutdown) = watch::channel(false);
    let (session_closed, session_closed_receiver) = watch::channel(false);
    let mut retries = 0;

    assert_eq!(
        close_while_pending(
            retry_seed_capability_lookup(
                SimulatorId::new(1),
                &mut retries,
                SocketAddr::from(([127, 0, 0, 1], 0)),
                &events,
                "seed lookup failed".into(),
                Some(Duration::from_secs(30)),
                &mut shutdown,
                &session_closed_receiver,
            ),
            &session_closed,
        )
        .await,
        RetryOutcome::Stopped
    );
}

#[tokio::test]
async fn poll_limit_notification_stops_when_session_closes() {
    let (events, mut received) = mpsc::channel(1);
    events.try_send(EventEnvelope::new(Event::Failed("occupy event channel".into()))).expect("fill event channel");
    let (_shutdown, mut shutdown) = watch::channel(false);
    let (session_closed, session_closed_receiver) = watch::channel(false);
    let mut error_count = MAX_EVENT_QUEUE_ERRORS;

    assert_eq!(
        close_while_pending(
            retry_event_queue(
                SimulatorId::new(1),
                &mut error_count,
                SocketAddr::from(([127, 0, 0, 1], 0)),
                &events,
                "poll failed".into(),
                &mut shutdown,
                &session_closed_receiver,
            ),
            &session_closed,
        )
        .await,
        RetryOutcome::Stopped
    );
    assert!(matches!(received.try_recv().map(EventEnvelope::into_event), Ok(Event::Failed(_))));
}

#[tokio::test]
async fn seed_limit_notification_stops_when_session_closes() {
    let (events, mut received) = mpsc::channel(1);
    events.try_send(EventEnvelope::new(Event::Failed("occupy event channel".into()))).expect("fill event channel");
    let (_shutdown, mut shutdown) = watch::channel(false);
    let (session_closed, session_closed_receiver) = watch::channel(false);
    let mut retries = SEED_LOOKUP_RETRY_LIMIT;

    assert_eq!(
        close_while_pending(
            retry_seed_capability_lookup(
                SimulatorId::new(1),
                &mut retries,
                SocketAddr::from(([127, 0, 0, 1], 0)),
                &events,
                "seed lookup failed".into(),
                Some(Duration::from_secs(30)),
                &mut shutdown,
                &session_closed_receiver,
            ),
            &session_closed,
        )
        .await,
        RetryOutcome::Stopped
    );
    assert!(matches!(received.try_recv().map(EventEnvelope::into_event), Ok(Event::Failed(_))));
}
#[cfg(test)]
mod retained_size_tests {
    use super::*;

    #[test]
    fn counts_owned_buffer_capacity() {
        let mut uri_text = String::with_capacity(4096);
        uri_text.push_str("uri");
        let uri_capacity = uri_text.capacity();
        let mut uri = Value::Uri(Uri::from(uri_text));
        assert_eq!(llsd_heap_size(&mut uri), uri_capacity);
        assert!(matches!(&uri, Value::Uri(value) if value.as_str() == "uri"));

        let mut bytes = Vec::with_capacity(4096);
        bytes.extend_from_slice(b"bin");
        let binary_capacity = bytes.capacity();
        let mut binary = Value::Binary(Binary::from(bytes));
        assert_eq!(llsd_heap_size(&mut binary), binary_capacity);
        assert!(matches!(&binary, Value::Binary(value) if value.as_slice() == b"bin"));
    }
}
#[cfg(test)]
mod event_queue_batch_tests {
    use super::*;
    use crate::{
        coverage::CoverageLoss,
        http::{ResponseMemoryBudget, ResponseMemoryClass},
    };
    use std::{
        future::Future,
        pin::pin,
        task::{Context, Waker},
    };
    use tokio::{spawn, task::yield_now};

    fn reserve_batch(budget: &Arc<Semaphore>) -> OwnedSemaphorePermit {
        budget.clone().try_acquire_many_owned(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES as u32).expect("reserve one EventQueue batch")
    }

    fn source() -> CoverageSource {
        CoverageSource::EventQueue { incarnation: 42 }
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
        assert!(matches!(fields.get("AgentData"), Some(Value::Array(_))));
        assert!(matches!(fields.get("RegionData"), Some(Value::Array(_))));
        assert!(matches!(fields.get("Info"), Some(Value::Array(_))));
        assert!(!fields.contains_key("CrossedRegion"), "message name is carried beside body, not nested inside it");
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
        let notifier = EventQueueLossNotifier::new(
            events,
            simulator_id,
            SocketAddr::from(([127, 0, 0, 1], 0)),
            shutdown_receiver,
            session_closed_receiver,
        );
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
        send_event_queue_batch(
            events,
            simulator_id,
            SocketAddr::from(([127, 0, 0, 1], 0)),
            source(),
            0,
            batch,
            shutdown,
            session_closed,
            reserve_batch(budget),
            ledger,
            &mut pending,
        )
        .await
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

            assert!(acquire_event_batch_reservation(Arc::clone(&budget), &shutdown_receiver, &session_closed_receiver).is_none());
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
        let simulator_id = SimulatorId::new(3);
        let notifier = EventQueueLossNotifier::new(events, simulator_id, SocketAddr::from(([127, 0, 0, 1], 0)), shutdown, session_closed);
        let mut pending = PendingEventQueueLoss::new(ledger.clone(), simulator_id, source(), notifier);
        {
            let mut notification = pin!(pending.notify("admission failed".into()));
            assert!(notification.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        }
        pending.commit();
        drop(pending);

        assert!(matches!(received.recv().await.map(EventEnvelope::into_event), Some(Event::Failed(_))));
        assert!(matches!(
            received.recv().await.map(EventEnvelope::into_event),
            Some(Event::EventQueueCoverageLost { simulator_id: id, reason, .. }) if id == simulator_id && reason == "admission failed"
        ));
        assert!(received.recv().await.is_none(), "one recorded loss emits one notification");
        assert!(matches!(
            ledger.snapshot().losses.as_slice(),
            [CoverageLoss::Source(loss)] if loss.watermark.generation == 1 && loss.reason == CoverageLossReason::EventQueueAdmission
        ));
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
            SimulatorId::new(1),
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
        assert!(matches!(
            ledger.snapshot().losses.as_slice(),
            [CoverageLoss::Source(loss)]
                if loss.watermark.source == source() && loss.reason == CoverageLossReason::EventQueueAdmission
        ));
        assert!(matches!(received.try_recv().map(EventEnvelope::into_event), Ok(Event::Failed(_))));
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
                SimulatorId::new(3),
                vec![event("First"), event("Second")],
                &shutdown_receiver,
                &session_closed_receiver,
                &queued_event_bytes,
                &ledger,
            ));
            assert!(admission.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        }
        assert!(matches!(
            ledger.snapshot().losses.as_slice(),
            [CoverageLoss::Source(loss)] if loss.reason == CoverageLossReason::EventQueueAdmission
        ));
        assert!(matches!(received.try_recv().map(EventEnvelope::into_event), Ok(Event::Failed(_))));
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
                SimulatorId::new(1),
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
        assert!(matches!(
            &envelope.event,
            Event::CapabilityEventBatch { order: EventQueueBatchOrder { incarnation: 42, ordinal: 0 }, events, .. } if events.len() == 1
        ));
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
        let mut admission = pin!(send_batch(
            &events,
            SimulatorId::new(1),
            batch,
            &shutdown_receiver,
            &session_closed_receiver,
            &queued_event_bytes,
            &ledger,
        ));
        assert!(admission.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        assert_eq!(queued_event_bytes.available_permits(), MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES - expected_bytes);
        assert!(matches!(received.recv().await.map(EventEnvelope::into_event), Some(Event::Failed(_))));
        assert!(matches!(admission.await, EventSendOutcome::Sent));
        let envelope = received.recv().await.expect("one complete batch envelope");
        let Event::CapabilityEventBatch { events: batch, .. } = &envelope.event else {
            panic!("the EventQueue response must be one application event")
        };
        assert_eq!(batch.iter().map(|event| event.message.as_str()).collect::<Vec<_>>(), ["First", "Second"]);
        assert_eq!(
            envelope.coverage_watermark(),
            Some(ledger.watermark(SimulatorId::new(1), source())),
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
        assert!(acquire_event_batch_reservation(Arc::clone(&queued_event_bytes), &shutdown_receiver, &session_closed_receiver).is_none());

        drop(reservations.pop());
        let permit = acquire_event_batch_reservation(Arc::clone(&queued_event_bytes), &shutdown_receiver, &session_closed_receiver)
            .expect("released batch capacity should admit another poll");
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
            SimulatorId::new(1),
            vec![event("test")],
            &shutdown_receiver,
            &session_closed_receiver,
            &queued_event_bytes,
            &ledger,
        ));
        assert!(admission.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        drop(received);
        assert!(matches!(admission.await, EventSendOutcome::ReceiverClosed));
        assert!(matches!(
            ledger.snapshot().losses.as_slice(),
            [CoverageLoss::Source(loss)] if loss.reason == CoverageLossReason::EventQueueAdmission
        ));
        assert_eq!(queued_event_bytes.available_permits(), MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES);
    }

    #[tokio::test]
    async fn exhausted_budget_skips_decode() {
        let body = b"<llsd><map><key>id</key><integer>1</integer><key>events</key><array/></map></llsd>".to_vec();
        let available = body.len() + LLSD_SCAN_WORKING_BYTES;
        let budget = ResponseMemoryBudget::new(0, 0, available);
        let raw = budget.try_reserve(ResponseMemoryClass::EventQueue, body.len()).expect("reserve input");
        let workspace = budget.try_reserve(ResponseMemoryClass::EventQueue, LLSD_SCAN_WORKING_BYTES).expect("reserve scanner");
        let event_queues = Arc::new(Semaphore::new(1));
        let event_queue_permit = Arc::clone(&event_queues).acquire_owned().await.expect("EventQueue slot");
        let result = decode_admitted_event_batch_with(body, Some(raw), Some(workspace), event_queue_permit, |_| {
            panic!("decoder must not run without memory admission")
        })
        .await;
        assert!(matches!(result, Err(BatchDecodeError::Resource(_))));
        assert_eq!(event_queues.available_permits(), 1, "decode rejection returns its EventQueue slot");
        assert!(budget.try_reserve(ResponseMemoryClass::EventQueue, available).is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancellation_holds_decode_memory() {
        let body = b"<llsd><map><key>id</key><integer>1</integer><key>events</key><array/></map></llsd>".to_vec();
        let decoded_bytes = event_batch_allocation_bound(&body).expect("bound valid batch");
        let reserved_bytes = body.len() + decoded_bytes;
        let budget = ResponseMemoryBudget::new(0, 0, reserved_bytes);
        let raw_memory = budget.try_reserve(ResponseMemoryClass::EventQueue, body.len()).expect("reserve raw response");
        let decoded_memory = budget.try_reserve(ResponseMemoryClass::EventQueue, decoded_bytes).expect("reserve decode workspace");
        let event_queues = Arc::new(Semaphore::new(1));
        let event_queue_permit = Arc::clone(&event_queues).acquire_owned().await.expect("EventQueue slot");
        let (entered, entered_rx) = oneshot::channel();
        let (release, release_rx) = sync::mpsc::channel();
        let task =
            spawn(decode_admitted_event_batch_with(body, Some(raw_memory), Some(decoded_memory), event_queue_permit, move |bytes| {
                let _ = entered.send(());
                release_rx.recv().expect("release blocking decoder");
                decode_event_batch(bytes)
            }));

        entered_rx.await.expect("decoder should enter before cancellation");
        task.abort();
        let error = match task.await {
            Ok(_) => panic!("async decoder task should be canceled"),
            Err(error) => error,
        };
        assert!(error.is_cancelled());
        assert!(budget.try_reserve(ResponseMemoryClass::EventQueue, 1).is_err(), "both permits remain held during decoding");
        assert_eq!(event_queues.available_permits(), 0, "cancellation cannot admit a replacement poll before decode ends");

        release.send(()).expect("unblock decoder");
        time::timeout(Duration::from_secs(1), async {
            loop {
                let memory_released = match budget.try_reserve(ResponseMemoryClass::EventQueue, reserved_bytes) {
                    Ok(permit) => {
                        drop(permit);
                        true
                    }
                    Err(_) => false,
                };
                let event_queue_released = match Arc::clone(&event_queues).try_acquire_owned() {
                    Ok(permit) => {
                        drop(permit);
                        true
                    }
                    Err(_) => false,
                };
                if memory_released && event_queue_released {
                    break;
                }
                yield_now().await;
            }
        })
        .await
        .expect("decoder completion should release response memory and the EventQueue slot");
    }
}

#[cfg(test)]
mod client_event_queue_tests {
    use std::{
        io::Write,
        thread,
        time::{Duration, Instant},
    };

    use crate::capabilities::budget::{
        MAX_CAPABILITY_RESPONSE_BYTES, MAX_CONCURRENT_CAPABILITY_REQUESTS, MAX_CONCURRENT_EVENT_QUEUES, MAX_EVENT_QUEUE_RESPONSE_BYTES,
        MAX_LLSD_XML_ELEMENTS,
    };
    use crate::capabilities::event_queue::{MAX_EVENT_QUEUE_ERRORS, MAX_QUEUED_CAPABILITY_EVENT_BYTES};
    use crate::test_support::http::{LocalServer, response};
    use crate::test_support::{
        ConcurrentCapabilityTestServer, DecodeGate, EventQueueBudgetTestServer, EventQueueTestServer, EventServerResponse, MALFORMED_BATCH,
        TWO_EVENT_BATCH, acknowledgement, decode_request_body, hold_response, map_field, receive_message, receive_udp, request_body,
        request_line, seed_response, test_source, write_response,
    };
    use crate::{
        AgentSession,
        Error::CapabilitySessionClosed,
        Grid,
        capabilities::begin_process_measurement,
        capabilities::{CapabilityBudget, CapabilityUrl, Client, EventQueueExit},
        coverage::CoverageLossLedger,
        event::{CLIENT_EVENT_CAPACITY, Event, EventEnvelope, EventReceiver, PRIMARY_TERMINAL_EVENT_CAPACITY, SimulatorId},
        http,
        session::{COMMAND_CAPACITY, Command},
        simulator::run_session,
        status::NetworkStatusHandle,
        transport::lludp::Delivery,
    };
    use ::test_support::{lludp::vectors, parse_hex_fixture};
    use llsd::{Value, xml};
    use protocol::{
        ChatType,
        lludp::{Message, Packet},
        messages::{ChatFromViewer, chat::chat_from_viewer},
    };
    use reqwest::StatusCode;
    use tokio::{
        net::UdpSocket,
        sync::{mpsc, watch},
        task::{spawn, yield_now},
        time::{sleep, timeout},
    };
    use uuid::Uuid;

    use super::*;

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    fn available_event_queue_permits(budget: &CapabilityBudget) -> usize {
        budget.primary_event_queues.available_permits() + budget.neighbor_event_queues.available_permits()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shared_budget_bounds_event_polls() {
        let task_count = MAX_CONCURRENT_EVENT_QUEUES + 1;
        let server = EventQueueBudgetTestServer::start();
        let budget = CapabilityBudget::new();
        let clients = (0..2)
            .map(|_| {
                Client::new_with_status_and_budget(server.url("/seed"), NetworkStatusHandle::default(), budget.clone())
                    .expect("active Tokio runtime")
            })
            .collect::<Vec<_>>();
        let (events, received_events) = mpsc::channel(1);
        let mut received_events = EventReceiver::new(received_events);
        events.try_send(EventEnvelope::new(Event::SendFailed("hold event forwarding".into()))).expect("fill the application event channel");
        let (shutdown, shutdown_requested) = watch::channel(false);
        let mut tasks = Vec::with_capacity(task_count);

        for index in 0..task_count {
            let client = clients[index % clients.len()].clone();
            let task = client
                .start_event_queue_with_role(
                    SimulatorId::new(index as u64 + 1),
                    test_source(),
                    events.clone(),
                    shutdown_requested.clone(),
                    CoverageLossLedger::new(),
                    index == 0,
                )
                .expect("start event queue");
            tasks.push(task);
        }

        let mut polls = (0..MAX_CONCURRENT_EVENT_QUEUES).map(|_| server.next_poll()).collect::<Vec<_>>();
        for _ in 0..task_count {
            server.next_seed();
        }
        let unexpected_poll = server.polls.recv_timeout(Duration::from_millis(150)).ok();
        let admitted = unexpected_poll.is_none();
        let mut forwarded_batch = false;

        if admitted {
            let wait_started = Instant::now();
            write_response(
                polls.pop().expect("one event poll to release"),
                response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH),
            );
            assert!(matches!(
                timeout(TEST_TIMEOUT, received_events.recv()).await.expect("held event channel item").expect("event channel remains open"),
                Event::SendFailed(message) if message == "hold event forwarding"
            ));
            timeout(TEST_TIMEOUT, async {
                while received_events.is_empty() {
                    yield_now().await;
                }
            })
            .await
            .expect("first event should fill the application channel");
            let no_poll_while_forwarding = server.polls.recv_timeout(Duration::from_millis(150)).is_err();
            let batch_event = timeout(TEST_TIMEOUT, received_events.recv())
                .await
                .expect("complete decoded batch should be forwarded")
                .expect("event channel remains open");
            let next_poll = server.next_poll();
            let observed_wait = wait_started.elapsed();
            eprintln!(
                "observed EventQueue wait from confirmed contention to next server poll (includes slot release and HTTP handling): {observed_wait:?}"
            );
            polls.push(next_poll);
            forwarded_batch = no_poll_while_forwarding
                && matches!(batch_event, Event::CapabilityEventBatch { events, .. }
                        if events.iter().map(|event| event.message.as_str()).collect::<Vec<_>>() == ["First", "Second"]);
        } else if let Some(poll) = unexpected_poll {
            polls.push(poll);
        }

        shutdown.send_replace(true);
        drop(polls);

        timeout(TEST_TIMEOUT, async {
            for task in tasks {
                assert_eq!(task.wait().await, EventQueueExit::Stopped);
            }
        })
        .await
        .expect("all event queue tasks should stop");
        server.stop();
        server.finish();

        assert!(admitted, "a shared EventQueueGet permit must hold until the batch is decoded and forwarded");
        assert!(forwarded_batch, "the permit must remain held until every decoded event is forwarded");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn response_classes_share_budget() {
        let event_server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
                hold_response(),
            ]
        });
        let ordinary_body = "<llsd><integer>42</integer></llsd>";
        let ordinary_server = LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, ordinary_body)]);
        let mut budget = CapabilityBudget::new();
        let ordinary_memory_limit = 64 * 1024;
        let event_queue_memory_limit = 2 * 1024 * 1024;
        budget.response_memory = http::ResponseMemoryBudget::new(ordinary_memory_limit, event_queue_memory_limit, 0);
        let ordinary_request_capacity = budget.ordinary_requests.available_permits();
        let event_queue_capacity = available_event_queue_permits(&budget);
        let queued_event_capacity = budget.queued_event_bytes.available_permits();
        let client = Client::new_with_status_and_budget(event_server.url("/seed"), NetworkStatusHandle::default(), budget.clone())
            .expect("active Tokio runtime");
        let (event_sender, mut received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start EventQueue");

        let seed_request = event_server.next_request();
        let first_poll = event_server.next_request();
        assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&first_poll).starts_with("POST /event-queue HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));

        let batch = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("EventQueue response should be decoded")
            .expect("event channel remains open");
        assert!(matches!(&batch.event, Event::CapabilityEventBatch { events, .. } if events.len() == 2));
        assert!(budget.queued_event_bytes.available_permits() < queued_event_capacity, "decoded batch must retain its queue reservation");
        let second_poll = event_server.next_request();
        assert_eq!(map_field(&decode_request_body(&second_poll), "ack"), Some(&Value::Integer(5)));
        assert_eq!(available_event_queue_permits(&budget), event_queue_capacity - 1, "long poll holds its concurrency slot");

        let ordinary_url = CapabilityUrl::parse(ordinary_server.url("/ordinary").as_str()).expect("valid ordinary capability URL");
        let (gate, entered_receiver) = DecodeGate::new();
        let ordinary_client = client.clone();
        let decode_gate = gate.clone();
        let ordinary_task = spawn(async move { ordinary_client.get_with_decode_hook(&ordinary_url, move || decode_gate.wait()).await });

        assert!(entered_receiver.recv_timeout(TEST_TIMEOUT).is_ok(), "ordinary response should decode while EventQueue is polling");
        assert!(budget.response_memory.try_reserve(http::ResponseMemoryClass::Ordinary, ordinary_memory_limit).is_err());
        assert!(available_event_queue_permits(&budget) < event_queue_capacity);
        gate.release();
        assert_eq!(
            timeout(TEST_TIMEOUT, ordinary_task)
                .await
                .expect("ordinary decode should finish")
                .expect("ordinary request task")
                .expect("ordinary response should decode"),
            Value::Integer(42)
        );
        assert!(
            budget.response_memory.try_reserve(http::ResponseMemoryClass::Ordinary, ordinary_memory_limit).is_ok(),
            "ordinary raw-response admission returns after decode"
        );
        assert_eq!(ordinary_server.finish().len(), 1);

        shutdown.send_replace(true);
        assert_eq!(timeout(TEST_TIMEOUT, task.wait()).await.expect("EventQueue should stop"), EventQueueExit::Stopped);
        let event_queue_memory = budget
            .response_memory
            .try_reserve(http::ResponseMemoryClass::EventQueue, event_queue_memory_limit)
            .expect("decode-memory admission returns when ownership transfers to the queued-event budget");
        drop(event_queue_memory);
        assert!(
            budget.queued_event_bytes.available_permits() < queued_event_capacity,
            "the queued batch keeps its retained-event reservation until the envelope is dropped"
        );
        drop(batch);
        assert_eq!(event_server.finish().len(), 0, "the seed and both polls were already observed");
        assert_eq!(budget.ordinary_requests.available_permits(), ordinary_request_capacity);
        assert_eq!(available_event_queue_permits(&budget), event_queue_capacity);
        assert_eq!(budget.queued_event_bytes.available_permits(), queued_event_capacity);
        let event_queue_memory = budget
            .response_memory
            .try_reserve(http::ResponseMemoryClass::EventQueue, event_queue_memory_limit)
            .expect("EventQueue response admission returns after batch drop and shutdown");
        drop(event_queue_memory);

        drop(client);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual process-wide allocator sample; run with cargo test -p network --lib -- --ignored event_queue_peak_with_ordinary_live --nocapture"]
    async fn event_queue_peak_with_ordinary_live() {
        event_queue_peak_sample().await;
    }

    async fn event_queue_peak_sample() {
        const EVENT_QUEUE_COUNT: usize = MAX_CONCURRENT_EVENT_QUEUES;
        const ORDINARY_REQUEST_COUNT: usize = MAX_CONCURRENT_CAPABILITY_REQUESTS;
        const EVENT_COUNT: usize = 4;
        const EVENT_PAYLOAD_BYTES: usize = MAX_QUEUED_CAPABILITY_EVENT_BYTES - 16 * 1024;
        const ORDINARY_PAYLOAD_BYTES: usize = 64 * 1024;

        let event_payload = "e".repeat(EVENT_PAYLOAD_BYTES);
        let mut event_body = String::from("<llsd><map><key>id</key><integer>6</integer><key>events</key><array>");
        for _ in 0..EVENT_COUNT {
            event_body.push_str("<map><key>message</key><string>ObjectUpdate</string><key>body</key><map><key>payload</key><string>");
            event_body.push_str(&event_payload);
            event_body.push_str("</string></map></map>");
        }
        event_body.push_str("</array></map></llsd>");
        let ordinary_payload = "o".repeat(ORDINARY_PAYLOAD_BYTES);
        let ordinary_body = format!("<llsd><string>{ordinary_payload}</string></llsd>");
        let event_value_bound = xml::value_allocation_bound(event_body.as_bytes(), crate::capabilities::request::capability_llsd_limits())
            .expect("bound EventQueue XML decode");
        let event_batch_bound = event_batch_allocation_bound(event_body.as_bytes()).expect("bound EventQueue batch decode");
        let event_response = response(200, "OK", "application/llsd+xml", None, &event_body);
        let ordinary_response = response(200, "OK", "application/llsd+xml", None, &ordinary_body);
        let event_server = EventQueueBudgetTestServer::start();
        let ordinary_server = LocalServer::start(vec![ordinary_response; ORDINARY_REQUEST_COUNT]);
        let budget = CapabilityBudget::new();
        let queued_event_capacity = budget.queued_event_bytes.available_permits();
        let client = Client::new_with_status_and_budget(event_server.url("/seed"), NetworkStatusHandle::default(), budget.clone())
            .expect("active Tokio runtime");
        let (event_sender, mut received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let mut event_tasks = Vec::with_capacity(EVENT_QUEUE_COUNT);
        for index in 0..EVENT_QUEUE_COUNT {
            let task = client
                .start_event_queue_with_role(
                    SimulatorId::new(index as u64 + 1),
                    test_source(),
                    event_sender.clone(),
                    shutdown_requested.clone(),
                    CoverageLossLedger::new(),
                    index == 0,
                )
                .expect("start EventQueue");
            event_tasks.push(task);
        }

        let mut event_polls = Vec::with_capacity(EVENT_QUEUE_COUNT);
        for _ in 0..EVENT_QUEUE_COUNT {
            event_server.next_seed();
            event_polls.push(event_server.next_poll());
        }
        assert_eq!(available_event_queue_permits(&budget), 0, "all configured EventQueue slots should be occupied");

        let ordinary_url = CapabilityUrl::parse(ordinary_server.url("/ordinary").as_str()).expect("valid capability URL");
        let (gate, entered_receiver) = DecodeGate::new();
        let mut ordinary_tasks = Vec::with_capacity(ORDINARY_REQUEST_COUNT);
        for _ in 0..ORDINARY_REQUEST_COUNT {
            let ordinary_client = client.clone();
            let ordinary_url = ordinary_url.clone();
            let decode_gate = gate.clone();
            ordinary_tasks
                .push(spawn(async move { ordinary_client.get_with_decode_hook(&ordinary_url, move || decode_gate.wait()).await }));
        }
        for _ in 0..ORDINARY_REQUEST_COUNT {
            entered_receiver.recv_timeout(TEST_TIMEOUT).expect("ordinary response should pause with its body admitted");
        }
        assert_eq!(budget.ordinary_requests.available_permits(), 0, "all configured ordinary request slots should be occupied");

        let measurement = begin_process_measurement();
        thread::scope(|scope| {
            for mut poll in event_polls {
                let event_response = &event_response;
                scope.spawn(move || poll.write_all(event_response).expect("write EventQueue response"));
            }
        });

        let mut batches = Vec::with_capacity(EVENT_QUEUE_COUNT);
        for _ in 0..EVENT_QUEUE_COUNT {
            let batch = timeout(TEST_TIMEOUT, received_events.recv())
                .await
                .expect("EventQueue response should decode")
                .expect("event channel remains open");
            assert!(matches!(&batch.event, Event::CapabilityEventBatch { events, .. } if events.len() == EVENT_COUNT));
            batches.push(batch);
        }
        let queued_event_bytes_remaining = budget.queued_event_bytes.available_permits();
        assert!(queued_event_bytes_remaining < queued_event_capacity, "decoded batches should retain their queue reservations");

        shutdown.send_replace(true);
        event_server.stop();
        for task in event_tasks {
            assert_eq!(timeout(TEST_TIMEOUT, task.wait()).await.expect("EventQueue should stop"), EventQueueExit::Stopped);
        }
        event_server.finish();

        let (baseline_bytes, live_delta_bytes, peak_bytes, elapsed_ns) = measurement.finish();
        let peak_delta_bytes = peak_bytes.saturating_sub(baseline_bytes);
        let total_batch_bound = event_batch_bound.saturating_mul(EVENT_QUEUE_COUNT);
        eprintln!(
            "Configured-concurrency EventQueue process allocation sample: event_queues={EVENT_QUEUE_COUNT} ordinary_requests={ORDINARY_REQUEST_COUNT} event_response={} B ordinary_response={} B events_per_batch={} value_bound_per_queue={} B batch_bound_per_queue={} B queued_event_bytes_remaining={} ordinary_limit={} B event_queue_limit={} B process_baseline={} B process_live_delta={} B process_peak={} B process_peak_delta={} B peak_delta_to_all_batch_bounds={:.2} elapsed={} ns. Four ordinary decoders remain paused with their bodies admitted while nine EventQueue responses are sent concurrently; all decoded EventQueue batches remain retained through the sample. Process-wide allocator sample includes test harness, Reqwest/Hyper, and Tokio; excludes allocator overhead and RSS.",
            event_body.len(),
            ordinary_body.len(),
            EVENT_COUNT,
            event_value_bound,
            event_batch_bound,
            queued_event_bytes_remaining,
            MAX_CAPABILITY_RESPONSE_BYTES * ORDINARY_REQUEST_COUNT,
            (2 * MAX_EVENT_QUEUE_RESPONSE_BYTES) * EVENT_QUEUE_COUNT + MAX_QUEUED_CAPABILITY_EVENT_BYTES,
            baseline_bytes,
            live_delta_bytes,
            peak_bytes,
            peak_delta_bytes,
            peak_delta_bytes as f64 / total_batch_bound.max(1) as f64,
            elapsed_ns,
        );

        gate.release();
        for task in ordinary_tasks {
            assert_eq!(
                timeout(TEST_TIMEOUT, task)
                    .await
                    .expect("ordinary decode should finish")
                    .expect("ordinary request task")
                    .expect("ordinary response should decode"),
                Value::String(ordinary_payload.clone())
            );
        }
        assert_eq!(ordinary_server.finish().len(), ORDINARY_REQUEST_COUNT);
        drop(client);
        drop(batches);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_poll_wait_cancels_on_close() {
        let server = EventQueueBudgetTestServer::start();
        let budget = CapabilityBudget::new();
        let clients = (0..2)
            .map(|_| {
                Client::new_with_status_and_budget(server.url("/seed"), NetworkStatusHandle::default(), budget.clone())
                    .expect("active Tokio runtime")
            })
            .collect::<Vec<_>>();
        let (events, _received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let mut tasks = Vec::with_capacity(MAX_CONCURRENT_EVENT_QUEUES);

        for index in 0..MAX_CONCURRENT_EVENT_QUEUES {
            let task = clients[0]
                .start_event_queue_with_role(
                    SimulatorId::new(index as u64 + 1),
                    test_source(),
                    events.clone(),
                    shutdown_requested.clone(),
                    CoverageLossLedger::new(),
                    index == 0,
                )
                .expect("start admitted event queue");
            tasks.push(task);
        }
        let polls = (0..MAX_CONCURRENT_EVENT_QUEUES).map(|_| server.next_poll()).collect::<Vec<_>>();
        for _ in 0..MAX_CONCURRENT_EVENT_QUEUES {
            server.next_seed();
        }

        let waiting_client = clients[1].clone();
        let waiting_task = waiting_client
            .start_primary_event_queue(
                SimulatorId::new(MAX_CONCURRENT_EVENT_QUEUES as u64 + 1),
                test_source(),
                events,
                shutdown_requested.clone(),
                CoverageLossLedger::new(),
            )
            .expect("start waiting event queue");
        server.next_seed();
        assert!(server.polls.recv_timeout(Duration::from_millis(150)).is_err(), "the waiting queue must not start a poll");

        waiting_client.close_session();
        let waiting_exit = timeout(Duration::from_millis(500), waiting_task.wait()).await;
        shutdown.send_replace(true);
        drop(polls);

        timeout(TEST_TIMEOUT, async {
            for task in tasks {
                assert_eq!(task.wait().await, EventQueueExit::Stopped);
            }
        })
        .await
        .expect("admitted event queues should stop");
        server.stop();
        server.finish();

        assert_eq!(waiting_exit.expect("session close should cancel permit acquisition"), EventQueueExit::Stopped);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn waiting_event_queue_gets_slot_after_active_poll_stops() {
        let active_server = EventQueueBudgetTestServer::start();
        let waiting_server = EventQueueBudgetTestServer::start();
        let primary_server = EventQueueBudgetTestServer::start();
        let budget = CapabilityBudget::new();
        let active_client = Client::new_with_status_and_budget(active_server.url("/seed"), NetworkStatusHandle::default(), budget.clone())
            .expect("active Tokio runtime");
        let waiting_client =
            Client::new_with_status_and_budget(waiting_server.url("/seed"), NetworkStatusHandle::default(), budget.clone())
                .expect("active Tokio runtime");
        let primary_client = Client::new_with_status_and_budget(primary_server.url("/seed"), NetworkStatusHandle::default(), budget)
            .expect("active Tokio runtime");
        let (events, _received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let mut active_tasks = Vec::with_capacity(MAX_CONCURRENT_EVENT_QUEUES - 1);

        for index in 0..MAX_CONCURRENT_EVENT_QUEUES - 1 {
            active_tasks.push(
                active_client
                    .start_event_queue(
                        SimulatorId::new(index as u64 + 1),
                        test_source(),
                        events.clone(),
                        shutdown_requested.clone(),
                        CoverageLossLedger::new(),
                    )
                    .expect("start admitted EventQueue"),
            );
        }
        for _ in 0..MAX_CONCURRENT_EVENT_QUEUES - 1 {
            active_server.next_seed();
        }
        let mut active_polls = (0..MAX_CONCURRENT_EVENT_QUEUES - 1).map(|_| active_server.next_poll()).collect::<Vec<_>>();

        let waiting_task = waiting_client
            .start_event_queue(
                SimulatorId::new(MAX_CONCURRENT_EVENT_QUEUES as u64),
                test_source(),
                events.clone(),
                shutdown_requested.clone(),
                CoverageLossLedger::new(),
            )
            .expect("start waiting EventQueue");
        waiting_server.next_seed();

        let primary_task = primary_client
            .start_primary_event_queue(
                SimulatorId::new(MAX_CONCURRENT_EVENT_QUEUES as u64 + 1),
                test_source(),
                events,
                shutdown_requested.clone(),
                CoverageLossLedger::new(),
            )
            .expect("start primary EventQueue");
        primary_server.next_seed();
        let primary_poll =
            primary_server.polls.recv_timeout(Duration::from_secs(1)).expect("the primary EventQueue uses its reserved poll slot");
        assert!(
            waiting_server.polls.recv_timeout(Duration::from_millis(150)).is_err(),
            "neighbor polls cannot take the reserved primary slot"
        );

        active_polls
            .remove(0)
            .write_all(&response(404, "Not Found", "text/plain", None, "stopped"))
            .expect("stop one active EventQueue poll");
        let released_at = Instant::now();
        let waiting_poll = waiting_server
            .polls
            .recv_timeout(Duration::from_secs(1))
            .expect("a queued simulator should use the released EventQueue slot promptly");
        assert!(released_at.elapsed() < Duration::from_secs(1), "a queued simulator should use the released EventQueue slot promptly");

        shutdown.send_replace(true);
        drop((active_polls, waiting_poll, primary_poll));
        for task in active_tasks {
            assert_ne!(task.wait().await, EventQueueExit::TaskFailed, "active queue should stop without panicking");
        }
        assert_eq!(waiting_task.wait().await, EventQueueExit::Stopped);
        assert_eq!(primary_task.wait().await, EventQueueExit::Stopped);
        active_server.stop();
        waiting_server.stop();
        primary_server.stop();
        active_server.finish();
        waiting_server.finish();
        primary_server.finish();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn primary_event_queue_slot_follows_role_changes() {
        let neighbor_server = EventQueueBudgetTestServer::start();
        let old_primary_server = EventQueueBudgetTestServer::start();
        let new_primary_server = EventQueueBudgetTestServer::start();
        let budget = CapabilityBudget::new();
        let neighbor_client =
            Client::new_with_status_and_budget(neighbor_server.url("/seed"), NetworkStatusHandle::default(), budget.clone())
                .expect("active Tokio runtime");
        let old_primary_client =
            Client::new_with_status_and_budget(old_primary_server.url("/seed"), NetworkStatusHandle::default(), budget.clone())
                .expect("active Tokio runtime");
        let new_primary_client =
            Client::new_with_status_and_budget(new_primary_server.url("/seed"), NetworkStatusHandle::default(), budget)
                .expect("active Tokio runtime");
        let (events, _received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let old_coverage_losses = CoverageLossLedger::new();
        let mut old_losses_changed = old_coverage_losses.subscribe();
        let mut neighbors = Vec::with_capacity(MAX_CONCURRENT_EVENT_QUEUES - 1);

        for index in 0..MAX_CONCURRENT_EVENT_QUEUES - 1 {
            neighbors.push(
                neighbor_client
                    .start_event_queue(
                        SimulatorId::new(index as u64 + 1),
                        test_source(),
                        events.clone(),
                        shutdown_requested.clone(),
                        CoverageLossLedger::new(),
                    )
                    .expect("start neighbor EventQueue"),
            );
        }
        for _ in 0..MAX_CONCURRENT_EVENT_QUEUES - 1 {
            neighbor_server.next_seed();
        }
        let neighbor_polls = (0..MAX_CONCURRENT_EVENT_QUEUES - 1).map(|_| neighbor_server.next_poll()).collect::<Vec<_>>();

        let old_primary = old_primary_client
            .start_primary_event_queue(
                SimulatorId::new(MAX_CONCURRENT_EVENT_QUEUES as u64),
                test_source(),
                events.clone(),
                shutdown_requested.clone(),
                old_coverage_losses.clone(),
            )
            .expect("start old primary EventQueue");
        old_primary_server.next_seed();
        let mut old_poll = old_primary_server.next_poll();

        let partial_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/llsd+xml\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{}",
            TWO_EVENT_BATCH.len() + 32,
            TWO_EVENT_BATCH
        );
        old_poll.write_all(partial_response.as_bytes()).expect("send a partial event batch from the old simulator");
        old_primary.role_handle().set_primary(false);
        timeout(Duration::from_secs(1), old_losses_changed.changed())
            .await
            .expect("canceling a live EventQueue poll must record uncertain coverage")
            .expect("coverage ledger remains available");
        assert!(old_coverage_losses.snapshot().losses.iter().any(|loss| {
            matches!(loss, crate::coverage::CoverageLoss::Source(record)
                    if record.watermark.simulator_id == SimulatorId::new(MAX_CONCURRENT_EVENT_QUEUES as u64)
                        && record.reason == crate::coverage::CoverageLossReason::EventQueueAdmission)
        }));
        let new_primary = new_primary_client
            .start_event_queue(
                SimulatorId::new(MAX_CONCURRENT_EVENT_QUEUES as u64 + 1),
                test_source(),
                events,
                shutdown_requested.clone(),
                CoverageLossLedger::new(),
            )
            .expect("start promoted EventQueue");
        new_primary.role_handle().set_primary(true);
        new_primary_server.next_seed();
        let new_poll = new_primary_server
            .polls
            .recv_timeout(Duration::from_secs(1))
            .expect("the promoted EventQueue acquires the released primary slot");

        new_primary.role_handle().set_primary(false);
        old_primary.role_handle().set_primary(true);
        let returned_poll =
            old_primary_server.polls.recv_timeout(Duration::from_secs(1)).expect("a re-promoted EventQueue reacquires the primary slot");

        shutdown.send_replace(true);
        drop((neighbor_polls, old_poll, new_poll, returned_poll));
        for task in neighbors {
            assert_ne!(task.wait().await, EventQueueExit::TaskFailed, "neighbor queue should stop without panicking");
        }
        assert_eq!(old_primary.wait().await, EventQueueExit::Stopped);
        assert_eq!(new_primary.wait().await, EventQueueExit::Stopped);
        neighbor_server.stop();
        old_primary_server.stop();
        new_primary_server.stop();
        neighbor_server.finish();
        old_primary_server.finish();
        new_primary_server.finish();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_frees_slot() {
        let mut server = ConcurrentCapabilityTestServer::start(MAX_CONCURRENT_CAPABILITY_REQUESTS + 1, Duration::from_secs(1));
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let mut requests = (0..MAX_CONCURRENT_CAPABILITY_REQUESTS)
            .map(|_| {
                let client = client.clone();
                spawn(async move { client.request_capabilities(&["TestCapability"]).await })
            })
            .collect::<Vec<_>>();

        for _ in 0..MAX_CONCURRENT_CAPABILITY_REQUESTS {
            server.next_request().await;
        }

        let client = client.clone();
        requests.push(spawn(async move { client.request_capabilities(&["TestCapability"]).await }));
        let cancelled = requests.remove(0);
        cancelled.abort();
        let _ = cancelled.await;

        timeout(Duration::from_millis(750), server.next_request()).await.expect("the canceled request should release its concurrency slot");

        for request in requests {
            request.abort();
            let _ = request.await;
        }
        server.finish();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn close_stops_client() {
        let server = EventQueueTestServer::start(|_| vec![hold_response()]);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let retained_client = client.clone();
        let request = spawn(async move { retained_client.request_capabilities(&["TestCapability"]).await });
        let _ = server.next_request();

        client.close_session_and_wait().await;
        let error = timeout(TEST_TIMEOUT, request)
            .await
            .expect("closing the session should cancel the HTTP request")
            .expect("capability request task should finish")
            .expect_err("closed session cannot return capabilities");
        assert!(matches!(error, CapabilitySessionClosed));

        let error = client.request_capabilities(&["TestCapability"]).await.expect_err("retained client must reject new requests");
        assert!(matches!(error, CapabilitySessionClosed));
        assert!(server.finish().is_empty(), "no additional request should reach the server after session close");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn close_cancels_queue() {
        let mut server = ConcurrentCapabilityTestServer::start(MAX_CONCURRENT_CAPABILITY_REQUESTS, Duration::from_secs(1));
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let mut requests = (0..=MAX_CONCURRENT_CAPABILITY_REQUESTS)
            .map(|_| {
                let client = client.clone();
                spawn(async move { client.request_capabilities(&["TestCapability"]).await })
            })
            .collect::<Vec<_>>();

        for _ in 0..MAX_CONCURRENT_CAPABILITY_REQUESTS {
            server.next_request().await;
        }
        yield_now().await;
        client.close_session();

        for request in requests.drain(..) {
            let error = timeout(TEST_TIMEOUT, request)
                .await
                .expect("closing the session should end queued and active requests")
                .expect("capability request task should finish")
                .expect_err("closed session cannot return capabilities");
            assert!(matches!(error, CapabilitySessionClosed));
        }
        assert_eq!(server.finish(), MAX_CONCURRENT_CAPABILITY_REQUESTS, "only requests already in flight should reach the server");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_queue_ack_advances_after_batch_admission() {
        const TWO_EVENT_RESPONSE: &str = "<llsd><map><key>id</key><integer>4</integer><key>events</key><array><map><key>message</key><string>FirstEvent</string></map><map><key>message</key><string>SecondEvent</string></map></array></map></llsd>";
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/opaque/event/queue-token");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_RESPONSE)),
                hold_response(),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(1);
        event_sender.try_send(EventEnvelope::new(Event::Failed("hold first event slot".into()))).expect("fill event channel");
        let mut received_events = EventReceiver::new(received_events);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        let seed_request = server.next_request();
        let first_poll = server.next_request();
        sleep(Duration::from_millis(100)).await;
        assert!(matches!(server.requests.try_recv(), Err(sync::mpsc::TryRecvError::Empty)), "ack must wait for batch admission");

        assert!(matches!(received_events.recv().await, Some(Event::Failed(_))));
        let batch = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("event queue should deliver one complete batch")
            .expect("event channel should remain open");
        let Event::CapabilityEventBatch { source, events, .. } = batch else {
            panic!("EventQueue response must be admitted as one complete batch");
        };
        assert_eq!(source, test_source());
        assert_eq!(events.iter().map(|event| event.message.as_str()).collect::<Vec<_>>(), ["FirstEvent", "SecondEvent"]);
        drop(events);

        let second_poll = server.next_request();
        assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
        assert!(request_body(&seed_request).contains("EventQueueGet"));
        assert!(request_line(&first_poll).starts_with("POST /opaque/event/queue-token HTTP/1.1"));
        let first_poll = decode_request_body(&first_poll);
        assert_eq!(map_field(&first_poll, "ack"), Some(&Value::Undefined));
        assert!(request_line(&second_poll).starts_with("POST /opaque/event/queue-token HTTP/1.1"));
        let second_poll = decode_request_body(&second_poll);
        assert_eq!(map_field(&second_poll, "ack"), Some(&Value::Integer(4)));
        assert_eq!(map_field(&second_poll, "done"), Some(&Value::Boolean(false)));

        shutdown.send_replace(true);
        timeout(TEST_TIMEOUT, task.wait()).await.expect("event queue should stop after shutdown");
        assert!(server.finish().is_empty(), "shutdown must stop further event polling");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fast_502_retries() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(502, "Upstream error", "text/plain", None, "Upstream error: ")),
                hold_response(),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, mut received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        let seed_request = server.next_request();
        let first_poll = server.next_request();
        sleep(Duration::from_millis(100)).await;
        assert!(matches!(server.requests.try_recv(), Err(sync::mpsc::TryRecvError::Empty)), "a fast 502 should use retry backoff");

        shutdown.send_replace(true);
        timeout(TEST_TIMEOUT, task.wait()).await.expect("retry backoff should stop on shutdown");
        server.stop();
        assert!(server.finish().is_empty());
        assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&first_poll).starts_with("POST /event-queue HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));
        assert!(received_events.try_recv().is_err(), "empty batches should not emit events");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bad_batch_not_dispatched() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/events");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, MALFORMED_BATCH)),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(1);
        event_sender
            .try_send(EventEnvelope::new(Event::Failed("hold the only event slot".into())))
            .expect("fill the event channel before the malformed response");
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let coverage_losses = CoverageLossLedger::new();
        let mut loss_changes = coverage_losses.subscribe();
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        let seed_request = server.next_request();
        let poll_request = server.next_request();
        assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&poll_request).starts_with("POST /events HTTP/1.1"));
        timeout(TEST_TIMEOUT, loss_changes.changed())
            .await
            .expect("durable coverage loss should wake subscribers")
            .expect("coverage ledger remains open");
        assert!(matches!(
            received_events.try_recv(),
            Ok(Event::Failed(reason)) if reason == "hold the only event slot"
        ));
        let notification = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("loss notification should follow after a slot opens")
            .expect("event channel should remain open");
        assert!(matches!(
            notification,
            Event::EventQueueCoverageLost { simulator_id, source, reason }
                if simulator_id == SimulatorId::new(1) && source == test_source() && reason.contains("response")
        ));
        assert!(received_events.try_recv().is_err(), "one queue incarnation emits one loss notification");
        assert_eq!(
            task.wait().await,
            EventQueueExit::StoppedWithCoverageLoss,
            "an irrecoverable malformed batch stops this queue incarnation"
        );
        assert!(matches!(
            coverage_losses.snapshot().losses.as_slice(),
            [crate::coverage::CoverageLoss::Source(loss)]
                if loss.watermark.simulator_id == SimulatorId::new(1)
                    && matches!(loss.watermark.source, crate::coverage::CoverageSource::EventQueue { .. })
                    && loss.reason == crate::coverage::CoverageLossReason::EventQueueAdmission
                    && loss.retired
        ));

        assert!(server.finish().is_empty(), "a malformed batch must stop polling");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn redelivered_batch_is_suppressed() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/events");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
                hold_response(),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        let seed_request = server.next_request();
        let first_poll = server.next_request();
        assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));
        let first_batch = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("first response batch should be admitted")
            .expect("event channel should remain open");
        assert!(matches!(
            first_batch,
            Event::CapabilityEventBatch { events, .. }
                if events.iter().map(|event| event.message.as_str()).collect::<Vec<_>>() == ["First", "Second"]
        ));

        let redelivery_poll = server.next_request();
        assert_eq!(map_field(&decode_request_body(&redelivery_poll), "ack"), Some(&Value::Integer(5)));
        let next_poll = server.next_request();
        assert_eq!(map_field(&decode_request_body(&next_poll), "ack"), Some(&Value::Integer(5)));
        assert!(received_events.try_recv().is_err(), "the same response id in this queue incarnation must not republish its events");

        shutdown.send_replace(true);
        assert_eq!(timeout(TEST_TIMEOUT, task.wait()).await.expect("shutdown should stop the held poll"), EventQueueExit::Stopped);
        server.stop();
        assert!(server.finish().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lost_batch_is_redelivered_on_new_queue_incarnation() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/events");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
                hold_response(),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let simulator_id = SimulatorId::new(1);
        let source = test_source();
        let coverage_losses = CoverageLossLedger::new();
        let queued_event_capacity = client.budget.queued_event_bytes.available_permits();
        let batch_reservation_floor = queued_event_capacity
            .checked_sub(crate::capabilities::budget::MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES)
            .expect("session queue budget includes one EventQueue batch reservation");

        let (first_sender, first_receiver) = mpsc::channel(1);
        first_sender
            .try_send(EventEnvelope::new(Event::Failed("hold the first queue slot".into())))
            .expect("fill the first EventQueue channel");
        let mut first_receiver = EventReceiver::new(first_receiver);
        let (first_shutdown, first_shutdown_requested) = watch::channel(false);
        let first_task = client
            .start_event_queue(simulator_id, source, first_sender.clone(), first_shutdown_requested, coverage_losses.clone())
            .expect("start first EventQueue");

        assert!(request_line(&server.next_request()).starts_with("POST /seed HTTP/1.1"));
        let first_poll = server.next_request();
        assert!(request_line(&first_poll).starts_with("POST /events HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));
        timeout(TEST_TIMEOUT, async {
            while client.budget.queued_event_bytes.available_permits() <= batch_reservation_floor {
                yield_now().await;
            }
        })
        .await
        .expect("the returned batch should reach queue admission while the first channel is full");
        assert_eq!(first_sender.capacity(), 0, "the first queue slot remains occupied");

        first_shutdown.send_replace(true);
        assert_eq!(
            timeout(TEST_TIMEOUT, first_task.wait()).await.expect("the first EventQueue should stop after shutdown"),
            EventQueueExit::Stopped
        );
        assert!(
            matches!(server.requests.try_recv(), Err(sync::mpsc::TryRecvError::Empty)),
            "the unadmitted batch must not advance the ACK or start another poll"
        );
        assert!(matches!(
            first_receiver.try_recv(),
            Ok(Event::Failed(message)) if message == "hold the first queue slot"
        ));
        assert!(first_receiver.try_recv().is_err(), "the first queue must not publish a batch prefix or loss wake-up after shutdown");

        let first_loss = match coverage_losses.snapshot().losses.as_slice() {
            [crate::coverage::CoverageLoss::Source(loss)] => *loss,
            losses => panic!("expected one durable EventQueue coverage loss, got {losses:?}"),
        };
        assert_eq!(first_loss.watermark.simulator_id, simulator_id);
        assert!(matches!(first_loss.watermark.source, crate::coverage::CoverageSource::EventQueue { .. }));
        assert_eq!(first_loss.reason, crate::coverage::CoverageLossReason::EventQueueAdmission);
        assert!(first_loss.retired, "stopping the first queue retires its loss record");

        let (second_sender, second_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut second_receiver = EventReceiver::new(second_receiver);
        let (second_shutdown, second_shutdown_requested) = watch::channel(false);
        let second_task = client
            .start_event_queue(simulator_id, source, second_sender, second_shutdown_requested, coverage_losses.clone())
            .expect("start a fresh EventQueue incarnation");

        assert!(request_line(&server.next_request()).starts_with("POST /seed HTTP/1.1"));
        let redelivery_poll = server.next_request();
        assert!(request_line(&redelivery_poll).starts_with("POST /events HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&redelivery_poll), "ack"), Some(&Value::Undefined));
        let redelivered_batch = timeout(TEST_TIMEOUT, second_receiver.recv())
            .await
            .expect("the fresh queue should admit the redelivered batch")
            .expect("the second EventQueue channel should remain open");
        assert!(matches!(
            redelivered_batch,
            Event::CapabilityEventBatch { simulator_id: id, events, .. }
                if id == simulator_id
                    && events.iter().map(|event| event.message.as_str()).collect::<Vec<_>>() == ["First", "Second"]
        ));
        assert!(second_receiver.try_recv().is_err(), "the complete redelivery is published exactly once");

        let held_poll = server.next_request();
        assert_eq!(map_field(&decode_request_body(&held_poll), "ack"), Some(&Value::Integer(5)));
        second_shutdown.send_replace(true);
        assert_eq!(
            timeout(TEST_TIMEOUT, second_task.wait()).await.expect("the second EventQueue should stop cleanly"),
            EventQueueExit::Stopped
        );
        server.stop();
        assert!(server.finish().is_empty(), "all expected seed and poll requests were observed");
        assert!(
            matches!(
                coverage_losses.snapshot().losses.as_slice(),
                [crate::coverage::CoverageLoss::Source(current)]
                    if current.watermark == first_loss.watermark && current.retired
            ),
            "the first incarnation's loss remains pending for reconciliation"
        );
    }

    #[test]
    fn bad_event_fields() {
        for body in [
            "<llsd><map><key>events</key><array></array></map></llsd>",
            "<llsd><map><key>id</key><integer>5</integer><key>events</key><string>not an array</string></map></llsd>",
            "<llsd><map><key>id</key><string>five</string><key>events</key><array></array></map></llsd>",
            "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><string>not an event map</string></array></map></llsd>",
            "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>body</key><string>missing message name</string></map></array></map></llsd>",
            "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>message</key><integer>5</integer></map></array></map></llsd>",
        ] {
            assert!(decode_event_batch(body.as_bytes()).is_err(), "malformed batch should be rejected");
        }
    }

    #[test]
    fn event_batch_limits() {
        let large_body = "x".repeat(MAX_QUEUED_CAPABILITY_EVENT_BYTES + 1);
        let xml = format!(
            "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>message</key><string>Large</string><key>body</key><string>{large_body}</string></map></array></map></llsd>"
        );
        let error = decode_event_batch(xml.as_bytes()).expect_err("oversized event body should be rejected");
        assert!(error.contains("event exceeded"));

        let one_event = "<map><key>message</key><string>x</string></map>";
        let events = one_event.repeat(1025);
        let xml = format!("<llsd><map><key>id</key><integer>5</integer><key>events</key><array>{events}</array></map></llsd>");
        let error = decode_event_batch(xml.as_bytes()).expect_err("oversized event batch should be rejected");
        assert!(error.contains("1024-event limit"));

        let events = one_event.repeat(1024);
        let xml = format!("<llsd><map><key>id</key><integer>5</integer><key>events</key><array>{events}</array></map></llsd>");
        let (_, decoded) = decode_event_batch(xml.as_bytes()).expect("the maximum event count is accepted");
        assert_eq!(decoded.len(), 1024);

        let mut exact_response = b"<llsd><map><key>id</key><integer>5</integer><key>events</key><array/></map></llsd>".to_vec();
        exact_response.resize(MAX_EVENT_QUEUE_RESPONSE_BYTES, b' ');
        let (_, decoded) = decode_event_batch(&exact_response).expect("a response at the byte limit is accepted");
        assert!(decoded.is_empty());

        exact_response.push(b' ');
        let error = decode_event_batch(&exact_response).expect_err("oversized response should be rejected");
        assert!(error.contains("byte limit"));
    }

    #[test]
    fn event_data_limits() {
        let nested = format!("{}<undef/>{}", "<array>".repeat(65), "</array>".repeat(65));
        let xml = format!("<llsd>{nested}</llsd>");
        let error = decode_event_batch(xml.as_bytes()).expect_err("excessive LLSD nesting should be rejected before parsing");
        assert!(error.contains("nesting exceeded"));

        let body = "x".repeat(200_000);
        let one_event = format!("<map><key>message</key><string>Large</string><key>body</key><string>{body}</string></map>");
        let events = one_event.repeat(42);
        let xml = format!("<llsd><map><key>id</key><integer>5</integer><key>events</key><array>{events}</array></map></llsd>");
        let error = decode_event_batch(xml.as_bytes()).expect_err("aggregate event data should be bounded");
        assert!(error.contains("retained-data limit"));
    }

    #[test]
    fn map_budget() {
        let fields = (0..2_000).map(|index| format!("<key>field{index:04}</key><integer>1</integer>")).collect::<String>();
        let xml = format!(
            "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>message</key><string>CompactMap</string><key>body</key><map>{fields}</map></map></array></map></llsd>"
        );
        let Err(error) = decode_event_batch(xml.as_bytes()) else {
            panic!("decoded map storage should exceed the event memory limit");
        };
        assert!(error.contains("event exceeded"), "{error}");
    }

    #[test]
    fn xml_element_limit() {
        let values = "<undef/>".repeat(MAX_LLSD_XML_ELEMENTS);
        let xml = format!("<llsd><array>{values}</array></llsd>");
        let Err(error) = decode_event_batch(xml.as_bytes()) else {
            panic!("excessive LLSD values should be rejected during preflight");
        };
        assert!(error.contains("element limit"));
    }

    #[test]
    fn viewer_retry_backoff() {
        assert_eq!(event_queue_retry_delay(0), Duration::from_secs(1));
        assert_eq!(event_queue_retry_delay(1), Duration::from_secs(4));
        assert_eq!(event_queue_retry_delay(2), Duration::from_secs(7));
        assert_eq!(event_queue_retry_delay(14), Duration::from_secs(43));
    }

    #[test]
    fn long_poll_error_classification() {
        for status in
            [StatusCode::INTERNAL_SERVER_ERROR, StatusCode::BAD_GATEWAY, StatusCode::SERVICE_UNAVAILABLE, StatusCode::GATEWAY_TIMEOUT]
        {
            assert!(!expected_empty_poll(status, Duration::from_secs(10) - Duration::from_nanos(1)));
            assert!(expected_empty_poll(status, Duration::from_secs(10)));
        }
        let linden_error = StatusCode::from_u16(499).expect("499 is a valid HTTP status");
        for status in [linden_error, StatusCode::NOT_FOUND, StatusCode::TOO_MANY_REQUESTS] {
            assert!(!expected_empty_poll(status, Duration::from_secs(10)));
            assert!(!expected_empty_poll(status, Duration::from_secs(30)));
        }
        for status in [
            StatusCode::TOO_MANY_REQUESTS,
            linden_error,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
        ] {
            assert!(retryable_event_queue_error(status), "HTTP {status} should retry as an ordinary error");
        }
        for status in [StatusCode::BAD_REQUEST, StatusCode::UNAUTHORIZED, StatusCode::NOT_FOUND] {
            assert!(!retryable_event_queue_error(status), "HTTP {status} should stop this poll incarnation");
        }
        let fast_timeout =
            http::Error::Timeout { elapsed: Duration::from_secs(9), status: None, retry_after: None, message: String::new() };
        let long_timeout =
            http::Error::Timeout { elapsed: Duration::from_secs(10), status: None, retry_after: None, message: String::new() };
        assert!(!fast_timeout.is_timeout_after(Duration::from_secs(10)));
        assert!(long_timeout.is_timeout_after(Duration::from_secs(10)));

        let partial_body_timeout = http::Error::Timeout {
            elapsed: Duration::from_secs(10),
            status: Some(StatusCode::OK),
            retry_after: None,
            message: String::new(),
        };
        assert!(!partial_body_timeout.is_timeout_after(Duration::from_secs(10)));
    }

    #[tokio::test]
    async fn poll_timeout_is_typed() {
        let server = EventQueueTestServer::start(|_| vec![hold_response()]);
        let client = http::Client::new(Some(Duration::from_millis(25)), false, None).expect("build short-timeout HTTP client");

        let error = post_event_queue(&client, &server.url("/event-queue"), &Value::Undefined, 1024)
            .await
            .expect_err("held response should time out");
        assert!(error.is_timeout());
        assert_eq!(server.finish().len(), 1);
    }

    #[test]
    fn seed_backoff_caps() {
        assert_eq!(seed_capability_retry_delay(1), Duration::from_secs(1));
        assert_eq!(seed_capability_retry_delay(2), Duration::from_secs(2));
        assert_eq!(seed_capability_retry_delay(3), Duration::from_secs(4));
        assert_eq!(seed_capability_retry_delay(4), Duration::from_secs(8));
        assert_eq!(seed_capability_retry_delay(5), Duration::from_secs(8));
        assert_eq!(seed_retry_delay(2, None), Duration::from_secs(2));
        assert_eq!(seed_retry_delay(1, Some(Duration::ZERO)), Duration::ZERO);
        assert_eq!(seed_retry_delay(3, Some(Duration::from_secs(15))), Duration::from_secs(15));
        assert_eq!(seed_retry_delay(1, Some(Duration::from_secs(90))), Duration::from_secs(30));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown_cancels_long_poll() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/held-long-poll");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                hold_response(),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, _received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let coverage_losses = CoverageLossLedger::new();
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        let seed = server.next_request();
        assert!(request_line(&seed).starts_with("POST /seed HTTP/1.1"));
        let poll = server.next_request();
        assert!(request_line(&poll).starts_with("POST /held-long-poll HTTP/1.1"));
        shutdown.send_replace(true);
        timeout(TEST_TIMEOUT, task.wait()).await.expect("shutdown should cancel the held request");
        assert!(coverage_losses.snapshot().losses.is_empty(), "an intentional shutdown is not a coverage loss");
        assert!(server.finish().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn aborting_poll_records_coverage_loss() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/held-long-poll");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                hold_response(),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let simulator_id = SimulatorId::new(1);
        let coverage_losses = CoverageLossLedger::new();
        let mut loss_changes = coverage_losses.subscribe();
        let task = client
            .start_event_queue(simulator_id, test_source(), event_sender, shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        let seed = server.next_request();
        assert!(request_line(&seed).starts_with("POST /seed HTTP/1.1"));
        let poll = server.next_request();
        assert!(request_line(&poll).starts_with("POST /held-long-poll HTTP/1.1"));
        drop(task);

        let loss = timeout(TEST_TIMEOUT, async {
            loop {
                if let [crate::coverage::CoverageLoss::Source(loss)] = coverage_losses.snapshot().losses.as_slice()
                    && loss.retired
                {
                    break *loss;
                }
                loss_changes.changed().await.expect("coverage ledger remains open");
            }
        })
        .await
        .expect("aborting an in-flight poll should record and retire its source loss");
        assert_eq!(loss.watermark.simulator_id, simulator_id);
        assert!(matches!(loss.watermark.source, crate::coverage::CoverageSource::EventQueue { .. }));
        assert_eq!(loss.reason, crate::coverage::CoverageLossReason::EventQueueAdmission);
        let notification = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("aborting a poll should still wake the event receiver")
            .expect("event channel should remain open");
        assert!(matches!(
            notification,
            Event::EventQueueCoverageLost { simulator_id: id, source, reason }
                if id == simulator_id && source == test_source() && reason.contains("canceled")
        ));
        server.stop();
        assert!(server.finish().is_empty(), "the two observed requests were already consumed");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn loss_notification_waits_for_event_channel_capacity() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/malformed-batch");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, "<llsd><map>")),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(1);
        let mut received_events = EventReceiver::new(received_events);
        event_sender
            .send(EventEnvelope::new(Event::Failed("fixture occupying the event channel".into())))
            .await
            .expect("fill the single event slot");
        assert_eq!(event_sender.capacity(), 0);

        let (_shutdown, shutdown_requested) = watch::channel(false);
        let simulator_id = SimulatorId::new(1);
        let coverage_losses = CoverageLossLedger::new();
        let mut loss_changes = coverage_losses.subscribe();
        let task = client
            .start_event_queue(simulator_id, test_source(), event_sender.clone(), shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        assert!(request_line(&server.next_request()).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&server.next_request()).starts_with("POST /malformed-batch HTTP/1.1"));
        let loss = timeout(TEST_TIMEOUT, async {
            loop {
                let snapshot = coverage_losses.snapshot();
                if let [crate::coverage::CoverageLoss::Source(loss)] = snapshot.losses.as_slice()
                    && loss.watermark.simulator_id == simulator_id
                    && loss.reason == crate::coverage::CoverageLossReason::EventQueueAdmission
                {
                    break *loss;
                }
                loss_changes.changed().await.expect("coverage ledger remains open");
            }
        })
        .await
        .expect("the durable loss fact should be recorded while its notification is blocked");
        assert_eq!(event_sender.capacity(), 0, "the wake-up must wait behind the queued event");

        assert!(matches!(
            received_events.recv().await,
            Some(Event::Failed(message)) if message == "fixture occupying the event channel"
        ));
        let notification = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("the loss wake-up should be sent after a slot opens")
            .expect("event channel should remain open");
        assert!(matches!(
            notification,
            Event::EventQueueCoverageLost { simulator_id: id, source, reason }
                if id == simulator_id && source == test_source() && reason.contains("EventQueueGet")
        ));
        assert!(received_events.try_recv().is_err(), "one queue incarnation emits only one loss wake-up");
        assert_eq!(task.wait().await, EventQueueExit::StoppedWithCoverageLoss);
        assert!(matches!(
            coverage_losses.snapshot().losses.as_slice(),
            [crate::coverage::CoverageLoss::Source(current)]
                if current.watermark == loss.watermark && current.retired
        ));
        assert!(server.finish().is_empty(), "the observed requests were already consumed");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn decode_pressure_stops_queue() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(
                    200,
                    "OK",
                    "application/llsd+xml",
                    None,
                    "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>message</key><string>test</string></map></array></map></llsd>",
                )),
            ]
        });
        let mut client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        client.budget.response_memory = http::ResponseMemoryBudget::new(0, 0, 64 * 1024);
        let (sender, receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut receiver = EventReceiver::new(receiver);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let losses = CoverageLossLedger::new();
        let task =
            client.start_event_queue(SimulatorId::new(1), test_source(), sender, shutdown_requested, losses.clone()).expect("start queue");
        server.next_request();
        let poll = server.next_request();
        assert_eq!(map_field(&decode_request_body(&poll), "ack"), Some(&Value::Undefined));
        let notification = timeout(TEST_TIMEOUT, receiver.recv()).await.expect("decode rejection notification").expect("open receiver");
        assert!(matches!(notification, Event::EventQueueCoverageLost { reason, .. } if reason.contains("decode memory")));
        assert_eq!(
            timeout(TEST_TIMEOUT, task.wait()).await.expect("queue stops after admission failure"),
            EventQueueExit::StoppedWithCoverageLoss
        );
        assert!(matches!(losses.snapshot().losses.as_slice(), [crate::coverage::CoverageLoss::Source(loss)] if loss.retired));
        assert!(receiver.try_recv().is_err(), "no partial batch is published");
        assert!(client.budget.response_memory.try_reserve(http::ResponseMemoryClass::EventQueue, 64 * 1024).is_ok());
        assert!(server.finish().is_empty(), "rejected batch cannot produce an ACK poll");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unavailable_queue_errors() {
        let server = EventQueueTestServer::start(|_| {
            vec![EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, "<llsd><map></map></llsd>"))]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        let event = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("missing capability should be reported")
            .expect("event channel should remain open");
        let Event::CapabilityError { capability, reason, .. } = event else {
            panic!("expected a capability-scoped error");
        };
        assert_eq!(capability, "EventQueueGet");
        assert!(reason.contains("EventQueueGet"));
        assert_eq!(server.finish().len(), 1);
        task.wait().await;

        shutdown.send_replace(true);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn not_found_stops_polling() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let coverage_losses = CoverageLossLedger::new();
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        let reason = receive_poll_error(&mut received_events).await;
        assert!(reason.contains("HTTP 404"));
        assert_eq!(server.finish().len(), 2, "404 must not trigger another poll");
        assert_eq!(task.wait().await, EventQueueExit::StoppedWithCoverageLoss);
        assert!(matches!(
            coverage_losses.snapshot().losses.as_slice(),
            [crate::coverage::CoverageLoss::Source(loss)]
                if matches!(loss.watermark.source, crate::coverage::CoverageSource::EventQueue { .. })
                    && loss.reason == crate::coverage::CoverageLossReason::EventQueueAdmission
                    && loss.retired
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn primary_event_queue_404_keeps_active_session_connected() {
        let (response_sender, response_receiver) = sync::mpsc::channel();
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Controlled(response_receiver),
            ]
        });
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let agent_id = Uuid::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("agent UUID");
        let session_id = Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("session UUID");
        let session = AgentSession {
            first_name: "Test".into(),
            last_name: "Resident".into(),
            grid: Grid::Agni,
            message: "Welcome".into(),
            simulator_address: simulator.local_addr().expect("simulator address"),
            agent_id,
            session_id,
            circuit_code: 0x1122_3344,
            seed_capability: server.url("/seed"),
        };
        let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, event_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let mut events = EventReceiver::new(event_receiver);
        let (disconnect, disconnect_requested) = watch::channel(false);
        let worker = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

        let seed_request = server.next_request();
        let poll_request = server.next_request();
        assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&poll_request).starts_with("POST /event-queue HTTP/1.1"));

        let (_, peer) = receive_udp(&simulator).await;
        simulator.send_to(&acknowledgement(20, 1), peer).await.expect("ack circuit setup");
        let (movement, _) = receive_udp(&simulator).await;
        assert!(matches!(Packet::decode(&movement).expect("decode movement").message(), Message::CompleteAgentMovement(_)));
        simulator.send_to(&acknowledgement(21, 2), peer).await.expect("ack initial movement");

        let mut handshake = test_fixture(vectors::REGION_HANDSHAKE);
        handshake[0] |= 0x40;
        set_sequence(&mut handshake, 22);
        simulator.send_to(&handshake, peer).await.expect("send region handshake");
        let (reply, _) = receive_udp(&simulator).await;
        let reply = Packet::decode(&reply).expect("decode handshake reply");
        assert!(matches!(reply.message(), Message::RegionHandshakeReply(_)));
        simulator.send_to(&acknowledgement(23, reply.sequence()), peer).await.expect("ack handshake reply");

        let mut movement_complete = test_fixture(vectors::AGENT_MOVEMENT_COMPLETE);
        movement_complete[0] |= 0x40;
        set_sequence(&mut movement_complete, 24);
        simulator.send_to(&movement_complete, peer).await.expect("complete agent movement");

        timeout(TEST_TIMEOUT, async {
            loop {
                match events.recv().await.expect("session event stream remains open") {
                    Event::SessionReady { .. } => break,
                    Event::Failed(reason) => panic!("primary session failed before the EventQueue response: {reason}"),
                    _ => continue,
                }
            }
        })
        .await
        .expect("primary session should become ready before the held EventQueue response");

        response_sender
            .send(response(404, "Not Found", "text/plain", None, "not found"))
            .expect("EventQueue request is still waiting for its response");
        timeout(TEST_TIMEOUT, async {
            loop {
                match events.recv().await.expect("active session event stream remains open") {
                    Event::EventQueueCoverageLost { .. } => break,
                    Event::Failed(reason) => panic!("EventQueue 404 must not end the active session: {reason}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("EventQueue coverage loss should be reported");
        timeout(TEST_TIMEOUT, async {
            loop {
                match events.recv().await.expect("active session event stream remains open") {
                    Event::CapabilityError { capability, reason, .. } if capability == EVENT_QUEUE_NAME && reason.contains("HTTP 404") => {
                        break;
                    }
                    Event::Failed(reason) => panic!("EventQueue 404 must not end the active session: {reason}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("EventQueue capability failure should be reported");

        let chat = Message::ChatFromViewer(ChatFromViewer {
            agent_data: chat_from_viewer::AgentData { agent_id, session_id },
            chat_data: chat_from_viewer::ChatData { message: "still connected".into(), chat_type: ChatType::Normal, channel: 0 },
        });
        let send_result = commands.try_send(Command::SendMessage { message: chat, delivery: Delivery::Reliable });
        let message =
            receive_message(&simulator, |message| matches!(message, Message::ChatFromViewer(_) | Message::LogoutRequest(_))).await;
        if matches!(message.message(), Message::LogoutRequest(_)) {
            simulator.send_to(&acknowledgement(25, message.sequence()), peer).await.expect("ack unexpected active simulator logout");
            let mut logout_reply = test_fixture(vectors::LOGOUT_REPLY);
            logout_reply[0] |= 0x40;
            set_sequence(&mut logout_reply, 26);
            simulator.send_to(&logout_reply, peer).await.expect("reply to unexpected active simulator logout");
            let result = timeout(TEST_TIMEOUT, worker).await.expect("failed session should stop").expect("session task");
            assert!(result.is_err(), "the pre-fix runtime treats EventQueue loss as terminal");
            panic!("EventQueue 404 ended the active viewer session");
        }
        send_result.expect("active session must continue accepting simulator commands");
        assert!(matches!(
            message.message(),
            Message::ChatFromViewer(chat) if chat.chat_data.message == "still connected"
        ));
        simulator.send_to(&acknowledgement(25, message.sequence()), peer).await.expect("ack post-failure chat");

        disconnect.send_replace(true);
        let logout = receive_message(&simulator, |message| matches!(message, Message::LogoutRequest(_))).await;
        simulator.send_to(&acknowledgement(26, logout.sequence()), peer).await.expect("ack explicit active simulator logout");
        let mut logout_reply = test_fixture(vectors::LOGOUT_REPLY);
        logout_reply[0] |= 0x40;
        set_sequence(&mut logout_reply, 27);
        simulator.send_to(&logout_reply, peer).await.expect("reply to explicit active simulator logout");
        let result = timeout(TEST_TIMEOUT, worker).await.expect("explicit logout should stop the session").expect("session task");
        assert!(result.is_ok(), "explicit logout is a normal session end");
        assert!(server.finish().is_empty(), "404 must end polling without another request");
    }

    fn test_fixture(source: &str) -> Vec<u8> {
        parse_hex_fixture(source).expect("valid LLUDP fixture")
    }

    fn set_sequence(bytes: &mut [u8], sequence: u32) {
        bytes[1..5].copy_from_slice(&sequence.to_be_bytes());
    }

    async fn receive_poll_error(events: &mut EventReceiver) -> String {
        let loss = timeout(TEST_TIMEOUT, events.recv())
            .await
            .expect("coverage loss should be reported before the poll error")
            .expect("event channel should remain open");
        assert!(matches!(loss, Event::EventQueueCoverageLost { .. }));

        let event = timeout(TEST_TIMEOUT, events.recv())
            .await
            .expect("poll error should be reported after coverage loss")
            .expect("event channel should remain open");
        let Event::CapabilityError { capability, reason, .. } = event else {
            panic!("expected an EventQueueGet capability error");
        };
        assert_eq!(capability, EVENT_QUEUE_NAME);
        reason
    }

    #[tokio::test]
    async fn retry_limit_is_terminal() {
        let mut error_count = MAX_EVENT_QUEUE_ERRORS;
        let (events, received_events) = mpsc::channel(1);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, mut shutdown_requested) = watch::channel(false);
        let (_session_closed, session_closed) = watch::channel(false);

        let outcome = retry_event_queue(
            SimulatorId::new(1),
            &mut error_count,
            test_source(),
            &events,
            "EventQueueGet returned HTTP 500.".into(),
            &mut shutdown_requested,
            &session_closed,
        )
        .await;
        let Some(Event::CapabilityError { capability, reason, .. }) = received_events.recv().await else {
            panic!("retry exhaustion must report a capability error");
        };

        assert_eq!(outcome, RetryOutcome::LimitReached);
        assert_eq!(capability, "EventQueueGet");
        assert!(reason.contains("repeated errors"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_queue_retry_limit_records_coverage_before_reporting_error() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            let mut responses =
                vec![EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url)))];
            for _ in 0..=MAX_EVENT_QUEUE_ERRORS {
                responses.push(EventServerResponse::Response(response(503, "Service Unavailable", "text/plain", Some("0"), "try again")));
            }
            responses
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(1);
        event_sender.try_send(EventEnvelope::new(Event::Failed("occupy event channel".into()))).expect("fill event channel");
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let simulator_id = SimulatorId::new(1);
        let coverage_losses = CoverageLossLedger::new();
        let mut loss_changes = coverage_losses.subscribe();
        let task = client
            .start_event_queue(simulator_id, test_source(), event_sender, shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        assert!(request_line(&server.next_request()).starts_with("POST /seed HTTP/1.1"));
        for _ in 0..=MAX_EVENT_QUEUE_ERRORS {
            let poll = server.next_request();
            assert!(request_line(&poll).starts_with("POST /event-queue HTTP/1.1"));
            assert_eq!(map_field(&decode_request_body(&poll), "ack"), Some(&Value::Undefined));
        }

        let loss = timeout(TEST_TIMEOUT, async {
            loop {
                let snapshot = coverage_losses.snapshot();
                if let [crate::coverage::CoverageLoss::Source(loss)] = snapshot.losses.as_slice()
                    && loss.watermark.generation == 1
                {
                    break *loss;
                }
                loss_changes.changed().await.expect("coverage ledger remains open");
            }
        })
        .await
        .expect("retry exhaustion records coverage while the event channel is full");
        assert_eq!(loss.reason, crate::coverage::CoverageLossReason::EventQueueAdmission);
        assert_eq!(loss.watermark.simulator_id, simulator_id);

        assert!(matches!(received_events.recv().await, Some(Event::Failed(_))));
        let Some(Event::CapabilityError { capability, reason, .. }) =
            timeout(TEST_TIMEOUT, received_events.recv()).await.expect("retry error is reported")
        else {
            panic!("retry exhaustion must report a capability error");
        };
        assert_eq!(capability, EVENT_QUEUE_NAME);
        assert!(reason.contains("repeated errors"));
        assert!(matches!(
            timeout(TEST_TIMEOUT, received_events.recv()).await.expect("coverage loss is reported"),
            Some(Event::EventQueueCoverageLost { simulator_id: id, source, .. })
                if id == simulator_id && source == test_source()
        ));

        assert_eq!(timeout(TEST_TIMEOUT, task.wait()).await.expect("retry limit stops polling"), EventQueueExit::RetryLimitReached);
        assert!(matches!(
            coverage_losses.snapshot().losses.as_slice(),
            [crate::coverage::CoverageLoss::Source(record)]
                if record.watermark.generation == 1
                    && record.reason == crate::coverage::CoverageLossReason::EventQueueAdmission
                    && record.retired
        ));
        assert!(server.finish().is_empty(), "all expected requests were observed and no extra retry should be sent");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn seed_lookup_retries() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(503, "Service Unavailable", "text/plain", None, "try again")),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        assert!(receive_poll_error(&mut received_events).await.contains("HTTP 404"));
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
        assert!(request_line(&requests[0]).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&requests[1]).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&requests[2]).starts_with("POST /event-queue HTTP/1.1"));
        task.wait().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rate_limit_retries() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(429, "Too Many Requests", "text/plain", Some("0"), "try again")),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        assert!(receive_poll_error(&mut received_events).await.contains("HTTP 404"));
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
        assert!(request_line(&requests[0]).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&requests[1]).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&requests[2]).starts_with("POST /event-queue HTTP/1.1"));
        task.wait().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_queue_retry_after_keeps_ack() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(499, "Client Closed Request", "text/plain", Some("0"), "busy")),
                EventServerResponse::Response(response(429, "Too Many Requests", "text/plain", Some("0"), "busy")),
                EventServerResponse::Response(response(503, "Service Unavailable", "text/plain", Some("0"), "busy")),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
                EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "stopped")),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let coverage_losses = CoverageLossLedger::new();
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        let seed = server.next_request();
        let first_poll = server.next_request();
        assert!(request_line(&seed).starts_with("POST /seed HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));

        let retry_499_at = Instant::now();
        let after_499_poll = server.next_request();
        assert!(retry_499_at.elapsed() < Duration::from_millis(750), "499 Retry-After: 0 should bypass the default backoff");
        assert_eq!(map_field(&decode_request_body(&after_499_poll), "ack"), Some(&Value::Undefined), "499 retry keeps the previous ACK");
        let retry_429_at = Instant::now();
        let after_429_poll = server.next_request();
        assert!(retry_429_at.elapsed() < Duration::from_millis(750), "Retry-After: 0 should bypass the default one-second backoff");
        assert_eq!(map_field(&decode_request_body(&after_429_poll), "ack"), Some(&Value::Undefined), "429 retry keeps the previous ACK");
        let retry_503_at = Instant::now();
        let after_503_poll = server.next_request();
        assert!(retry_503_at.elapsed() < Duration::from_millis(750), "503 Retry-After: 0 should bypass the default backoff");
        assert_eq!(map_field(&decode_request_body(&after_503_poll), "ack"), Some(&Value::Undefined), "503 retry keeps the previous ACK");

        let batch =
            timeout(TEST_TIMEOUT, received_events.recv()).await.expect("admitted EventQueue batch").expect("event channel stays open");
        assert!(matches!(batch, Event::CapabilityEventBatch { events, .. } if events.len() == 2));
        let next_poll = server.next_request();
        assert_eq!(
            map_field(&decode_request_body(&next_poll), "ack"),
            Some(&Value::Integer(5)),
            "ACK advances only after full batch admission"
        );

        assert!(receive_poll_error(&mut received_events).await.contains("HTTP 404"));
        assert!(!coverage_losses.snapshot().losses.is_empty(), "a possibly lost poll response must remain durable");
        assert_eq!(task.wait().await, EventQueueExit::StoppedWithCoverageLoss);
        assert!(server.finish().is_empty(), "all expected requests were observed and no extra retry should be sent");
    }

    async fn event_queue_stops_after_ambiguous_transport_failure(failed_response: EventServerResponse) {
        let server = EventQueueTestServer::start(move |base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                failed_response,
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let simulator_id = SimulatorId::new(1);
        let coverage_losses = CoverageLossLedger::new();
        let task = client
            .start_event_queue(simulator_id, test_source(), event_sender, shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        assert!(request_line(&server.next_request()).starts_with("POST /seed HTTP/1.1"));
        let failure_poll = server.next_request();
        assert!(request_line(&failure_poll).starts_with("POST /event-queue HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&failure_poll), "ack"), Some(&Value::Undefined));
        let loss = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("the ambiguous poll should report durable loss")
            .expect("event channel should remain open");
        assert!(matches!(loss, Event::EventQueueCoverageLost { reason, .. } if reason.contains("request failed")));
        assert_eq!(
            timeout(TEST_TIMEOUT, task.wait()).await.expect("ambiguous outcome stops this poll incarnation"),
            EventQueueExit::StoppedWithCoverageLoss
        );
        assert!(
            matches!(
                coverage_losses.snapshot().losses.as_slice(),
                [crate::coverage::CoverageLoss::Source(record)]
                    if record.watermark.simulator_id == simulator_id
                        && record.reason == crate::coverage::CoverageLossReason::EventQueueAdmission
                        && record.retired
            ),
            "the loss must remain in the durable ledger after the poll incarnation stops"
        );
        assert!(server.finish().is_empty());
        drop(shutdown);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_queue_stops_after_connection_failure() {
        event_queue_stops_after_ambiguous_transport_failure(EventServerResponse::DropConnection).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_queue_stops_after_partial_response() {
        let body = TWO_EVENT_BATCH;
        let partial = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/llsd+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len() + 32,
            body
        )
        .into_bytes();
        event_queue_stops_after_ambiguous_transport_failure(EventServerResponse::Response(partial)).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn seed_transport_retries() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::DropConnection,
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        assert!(receive_poll_error(&mut received_events).await.contains("HTTP 404"));
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
        assert!(request_line(&requests[0]).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&requests[1]).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&requests[2]).starts_with("POST /event-queue HTTP/1.1"));
        task.wait().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn permanent_seed_error_stops() {
        let server = EventQueueTestServer::start(|_| {
            vec![EventServerResponse::Response(response(401, "Unauthorized", "text/plain", None, "denied"))]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        let event = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("permanent seed failure should be reported")
            .expect("event channel should remain open");
        assert!(matches!(event, Event::CapabilityError { reason, .. } if reason.contains("HTTP 401")));
        assert_eq!(server.finish().len(), 1, "permanent errors must not be retried");
        task.wait().await;
    }
    #[tokio::test]
    async fn chunked_event_limit() {
        let response = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Transfer-Encoding: chunked\r\n",
            "Connection: close\r\n\r\n",
            "3\r\nabc\r\n",
            "3\r\ndef\r\n",
            "0\r\n\r\n"
        );
        let server = LocalServer::start(vec![response.as_bytes().to_vec()]);
        let client = http::Client::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

        let error = match post_event_queue(&client, &server.url("/event-queue"), &Value::Undefined, 5).await {
            Err(error) => error,
            Ok(_) => panic!("chunked response exceeds limit"),
        };

        assert!(error.to_string().contains("5-byte limit"));
        assert_eq!(server.finish().len(), 1);
    }
}
