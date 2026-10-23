use super::*;

async fn send_pending(pending: PendingSend, circuit: &mut Circuit, simulator: &UdpSocket) -> Packet {
    let (sent_at, prepared) = pending.result.await.expect("pacer send completes");
    circuit.commit_sent(prepared, sent_at).expect("commit locally sent datagram");
    let mut bytes = [0; 2048];
    let (size, _) = timeout(Duration::from_secs(1), simulator.recv_from(&mut bytes))
        .await
        .expect("simulator receives datagram")
        .expect("read datagram");
    Packet::decode(&bytes[..size]).expect("decode sent datagram")
}

#[test]
fn active_send_uses_queue_capacity() {
    let mut queue = OutboundQueue::new();
    for _ in 0..MAX_NONCONTROL_SENDS {
        assert!(queue.push(queued_for_test(TrafficClass::Fair), None).is_ok());
    }
    assert!(queue.push(queued_for_test(TrafficClass::Control), None).is_ok());

    assert!(queue.push(queued_for_test(TrafficClass::Control), Some(TrafficClass::Control)).is_err());
    assert_eq!(queue.charged_bytes, (MAX_PENDING_SENDS - 1) * OUTBOUND_SEND_CHARGE_BYTES);
}

#[test]
fn control_sends_keep_reserved_capacity() {
    let mut queue = OutboundQueue::new();
    for _ in 0..MAX_NONCONTROL_SENDS {
        assert!(queue.push(queued_for_test(TrafficClass::Fair), None).is_ok());
    }

    assert!(queue.push(queued_for_test(TrafficClass::Fair), None).is_err());
    assert!(queue.push(queued_for_test(TrafficClass::Control), None).is_ok());
    assert!(queue.push(queued_for_test(TrafficClass::Control), None).is_ok());
    assert!(queue.push(queued_for_test(TrafficClass::Control), None).is_err());
}

#[tokio::test(start_paused = true)]
async fn full_reliable_window_keeps_reliable_work_queued_until_ack_packet() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let socket = Arc::new(UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect transport"));
    let budget = OutboundBudget::new(8_000);
    let pacer = budget.register(1, false).await.expect("register simulator pacer");
    let mut circuit = Circuit::new();
    let message = queued_for_test(TrafficClass::Fair).message;
    let mut first_sequence = None;
    for _ in 0..MAX_PENDING_PACKETS {
        let prepared = circuit.prepare_message(message.clone(), Delivery::Reliable).expect("prepare reliable packet");
        first_sequence.get_or_insert(prepared.sequence());
        circuit.commit_sent(prepared, Instant::now()).expect("record reliable packet");
    }
    assert!(!circuit.has_reliable_capacity());

    let mut queue = OutboundQueue::new();
    assert!(queue.push(queued_with_delivery_for_test(TrafficClass::Fair, Delivery::Reliable), None).is_ok());
    assert!(queue.push(queued_for_test(TrafficClass::Fair), None).is_ok());
    let mut deferred_retries = HashMap::new();
    let untracked = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
        .map_err(|(_, error)| error)
        .expect("untracked work should remain eligible")
        .expect("untracked work should bypass a blocked reliable packet");
    let packet = send_pending(untracked, &mut circuit, &simulator).await;
    assert!(!packet.is_reliable());
    assert_eq!(queue.messages.len(), 1, "tracked reliable work remains bounded in the local queue");
    assert!(queue.messages.front().expect("reliable work remains").delivery.tracks_reliable());
    assert!(
        prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
            .map_err(|(_, error)| error)
            .expect("a full window defers queued reliable work")
            .is_none()
    );

    let ack = circuit
        .prepare_receive(50_000, false, [first_sequence.expect("at least one packet")], Instant::now())
        .expect("process ACK for the first outstanding packet");
    circuit.commit_receive(ack);
    let reliable = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
        .map_err(|(_, error)| error)
        .expect("send queued reliable work after ACK")
        .expect("the ACK should open one in-flight slot");
    let packet = send_pending(reliable, &mut circuit, &simulator).await;
    assert!(packet.is_reliable());
    assert!(queue.messages.is_empty());

    drop(pacer);
    budget.shutdown().await;
}

#[test]
fn paced_send_expires() {
    let circuit = Circuit::new();
    let queued = queued_for_test(TrafficClass::Fair);
    let expired_at = queued.queued_at + MAX_SEND_AGE;
    let mut pending = Some(pending_for_test(&circuit, queued, TrafficClass::Fair));

    let PendingSendExpiration::QueuedMessage(expired) = expire_pending_send(&mut pending, expired_at).expect("expired pending message")
    else {
        panic!("application message expiration should return its queue owner");
    };

    assert!(pending.is_none(), "expiration cancels the deferred pacer request");
    assert_eq!(expired.class, TrafficClass::Fair);
}

#[test]
fn deferred_reliable_retry_expires() {
    let mut circuit = Circuit::new();
    let original = circuit.prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable).expect("prepare reliable send");
    circuit.commit_sent(original, Instant::now() - Duration::from_secs(6)).expect("record original local send");
    let prepared = circuit.prepare_retransmission(Instant::now()).expect("prepare due retry").expect("retry should be due");
    assert!(prepared.is_retransmission());

    let started_at = Instant::now();
    let meta = PreparedMeta::from(&prepared);
    let result: PendingSendFuture = Box::pin(future::pending());
    let mut pending = Some(PendingSend {
        meta,
        result,
        class: TrafficClass::Fair,
        queued_message: None,
        retry_deadline: Some(started_at + MAX_SEND_AGE),
    });

    let expiration = expire_pending_send(&mut pending, started_at + MAX_SEND_AGE).expect("retry age is bounded");

    assert!(matches!(expiration, PendingSendExpiration::ReliableRetry));
    assert!(pending.is_none(), "expiry cancels the pacer request");
}

#[tokio::test]
async fn control_traffic_cannot_restart_retry_deadline() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let socket = Arc::new(UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect transport"));
    let budget = OutboundBudget::new(8_000);
    let pacer = budget.register(1, false).await.expect("register simulator pacer");
    let mut circuit = Circuit::new();
    let original =
        circuit.prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable).expect("prepare original reliable send");
    let sequence = original.sequence();
    circuit.commit_sent(original, Instant::now() - Duration::from_secs(6)).expect("record original local send");
    let mut queue = OutboundQueue::new();
    let mut deferred_retries = HashMap::new();

    for _ in 0..3 {
        assert!(queue.push(queued_for_test(TrafficClass::Control), None).is_ok());
        let control = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
            .map_err(|(_, error)| error)
            .expect("prepare priority control work")
            .expect("control work should be ready");
        let packet = send_pending(control, &mut circuit, &simulator).await;
        assert!(!packet.is_resent(), "queued Control work retains priority");
    }

    let deferred_since = deferred_retries[&sequence];
    assert_eq!(
        expire_deferred_retries(&circuit, &mut deferred_retries, deferred_since + MAX_SEND_AGE),
        Err(RELIABLE_RETRY_EXPIRED.into()),
        "repeated priority sends cannot extend a due retry's deadline"
    );

    drop(pacer);
    budget.shutdown().await;
}

#[tokio::test]
async fn shutdown_limits_sends_to_control_and_acks() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let socket = Arc::new(UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect transport"));
    let budget = OutboundBudget::new(8_000);
    let pacer = budget.register(1, false).await.expect("register simulator pacer");
    let mut circuit = Circuit::new();
    let original =
        circuit.prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable).expect("prepare original reliable send");
    circuit.commit_sent(original, Instant::now() - Duration::from_secs(6)).expect("record original send");
    let mut queue = OutboundQueue::new();
    assert!(queue.push(queued_for_test(TrafficClass::Fair), None).is_ok());
    assert!(queue.push(queued_for_test(TrafficClass::Control), None).is_ok());

    let control = prepare_shutdown_send(&socket, &pacer, &mut circuit, &mut queue, false)
        .map_err(|(_, error)| error)
        .expect("prepare shutdown control")
        .expect("queued control should be sent");
    let packet = send_pending(control, &mut circuit, &simulator).await;
    assert!(!packet.is_resent());
    assert_eq!(packet.message(), &Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 1 } }));
    assert_eq!(queue.messages.len(), 1, "shutdown leaves Fair work queued");
    assert!(
        prepare_shutdown_send(&socket, &pacer, &mut circuit, &mut queue, false)
            .map_err(|(_, error)| error)
            .expect("do not retry or send Fair work")
            .is_none()
    );

    let admission = circuit.prepare_receive(42, true, [], Instant::now()).expect("prepare inbound ACK requirement");
    circuit.commit_receive(admission);
    let ack = prepare_shutdown_send(&socket, &pacer, &mut circuit, &mut queue, true)
        .map_err(|(_, error)| error)
        .expect("prepare shutdown ACK")
        .expect("pending acknowledgement should be sent");
    let packet = send_pending(ack, &mut circuit, &simulator).await;
    assert_eq!(packet.message(), &Message::PacketAck(PacketAck { packets: vec![Packets { id: 42 }] }));

    drop(pacer);
    budget.shutdown().await;
}

#[tokio::test]
async fn ack_piggyback_keeps_payload_class() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let socket = Arc::new(UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect transport"));
    let budget = OutboundBudget::new(8_000);
    let pacer = budget.register(1, false).await.expect("register simulator pacer");
    let mut circuit = Circuit::new();
    let admission = circuit.prepare_receive(42, true, [], Instant::now()).expect("reserve reliable receive");
    circuit.commit_receive(admission);
    let mut queue = OutboundQueue::new();
    let mut deferred_retries = HashMap::new();
    assert!(queue.push(queued_for_test(TrafficClass::Fair), None).is_ok());

    let pending = match prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false) {
        Ok(Some(pending)) => pending,
        Ok(None) => panic!("pending ACK and payload should be prepared"),
        Err((_, error)) => panic!("prepare piggyback: {error}"),
    };

    assert_eq!(pending.class, TrafficClass::Fair);
    let packet = send_pending(pending, &mut circuit, &simulator).await;
    assert!(packet.appended_acks().contains(&42));
    drop(pacer);
    budget.shutdown().await;
}

#[tokio::test]
async fn fair_sends_follow_retries() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let socket = Arc::new(UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect transport"));
    let budget = OutboundBudget::new(8_000);
    let pacer = budget.register(1, false).await.expect("register simulator pacer");
    let mut circuit = Circuit::new();
    let original =
        circuit.prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable).expect("prepare original reliable send");
    circuit.commit_sent(original, Instant::now() - Duration::from_secs(6)).expect("track original send");
    let admission = circuit.prepare_receive(42, true, [], Instant::now()).expect("reserve ACK");
    circuit.commit_receive(admission);
    let mut queue = OutboundQueue::new();
    let mut deferred_retries = HashMap::new();
    assert!(queue.push(queued_for_test(TrafficClass::Fair), None).is_ok());

    let retry = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
        .map_err(|(_, error)| error)
        .expect("prepare retry")
        .expect("due retry should be ready");
    let packet = send_pending(retry, &mut circuit, &simulator).await;
    assert!(packet.is_resent());

    let fair = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, true)
        .map_err(|(_, error)| error)
        .expect("prepare Fair work after retry")
        .expect("queued Fair work should run before another retry");
    assert_eq!(fair.class, TrafficClass::Fair);
    let packet = send_pending(fair, &mut circuit, &simulator).await;
    assert!(!packet.is_resent());
    assert!(packet.appended_acks().contains(&42));
    drop(pacer);
    budget.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn retry_loss_is_reported_once_per_reliable_generation() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let socket = Arc::new(UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect transport"));
    let budget = OutboundBudget::new(20_000);
    let pacer = budget.register(1, false).await.expect("register simulator pacer");
    let handle = pacer.handle();
    let mut circuit = Circuit::new();
    let original =
        circuit.prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable).expect("prepare original reliable send");
    circuit.commit_sent(original, Instant::now() - Duration::from_secs(6)).expect("track original send");
    let mut queue = OutboundQueue::new();
    let mut deferred_retries = HashMap::new();

    let first = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
        .map_err(|(_, error)| error)
        .expect("prepare first retry")
        .expect("first retry should be due");
    assert_eq!(handle.rate_for_test(), 10_000);
    drop(first);

    advance(Duration::from_secs(2)).await;
    let second = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
        .map_err(|(_, error)| error)
        .expect("prepare deferred retry again")
        .expect("the unsent retry remains due");
    assert_eq!(handle.rate_for_test(), 10_000, "preemption does not report the same retry generation twice");
    let sequence = second.meta.sequence;
    drop(second);
    let sent_at = Instant::now();
    let prepared =
        circuit.prepare_retransmission(sent_at).expect("prepare retransmission after pacer preemption").expect("the retry remains due");
    circuit.commit_sent(prepared, sent_at).expect("commit the locally sent retry");
    deferred_retries.remove(&sequence);

    advance(Duration::from_secs(60)).await;
    let next_generation = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
        .map_err(|(_, error)| error)
        .expect("prepare next retry generation")
        .expect("next retry should be due");
    assert_eq!(handle.rate_for_test(), 5_000, "a later retry generation reports a new loss");
    drop(next_generation);

    drop(pacer);
    budget.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn primary_priority_sends_during_neighbor_retry_storm() {
    const NEIGHBOR_COUNT: usize = 4;
    const RETRIES_PER_NEIGHBOR: usize = 64;

    let mut simulator_sockets = Vec::new();
    let mut transports = Vec::new();
    for _ in 0..=NEIGHBOR_COUNT {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
        let address = simulator.local_addr().expect("simulator address");
        transports.push(Arc::new(UdpTransport::connect(address).await.expect("connect simulator transport")));
        simulator_sockets.push(simulator);
    }

    let budget = OutboundBudget::new(1_000);
    let mut pacers = Vec::new();
    for id in 1..=NEIGHBOR_COUNT + 1 {
        pacers.push(budget.register(id as u64, id == 1).await.expect("register simulator pacer"));
    }

    let burst = vec![0; crate::transport::pacing::MAX_BURST_BYTES as usize];
    pacers[0].handle().send(&transports[0], &burst, TrafficClass::Fair).await.expect("drain the initial shared pacing burst");

    let now = Instant::now();
    let original_sent_at = now - Duration::from_secs(6);
    let mut retry_workers = Vec::new();
    let mut retry_counts = Vec::new();
    for neighbor in 0..NEIGHBOR_COUNT {
        let mut retries = Vec::new();
        for offset in 0..RETRIES_PER_NEIGHBOR {
            let sequence = u32::try_from(neighbor * RETRIES_PER_NEIGHBOR + offset + 1).expect("small test sequence");
            let mut circuit = Circuit::with_sequence(sequence);
            let original = circuit
                .prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable)
                .expect("prepare overdue reliable packet");
            circuit.commit_sent(original, original_sent_at).expect("track overdue reliable packet");
            let retry = circuit.prepare_retransmission(now).expect("prepare due retransmission").expect("retry is due");
            let packet = Packet::decode(retry.bytes()).expect("decode prepared retransmission");
            assert!(packet.is_reliable() && packet.is_resent(), "the storm consists of actual Circuit retry datagrams");
            retries.push((circuit, retry));
        }

        let socket = Arc::clone(&transports[neighbor + 1]);
        let pacer = pacers[neighbor + 1].handle();
        let sent = Arc::new(AtomicUsize::new(0));
        let sent_by_worker = Arc::clone(&sent);
        retry_workers.push(spawn(async move {
            for (mut circuit, retry) in retries {
                let pending = begin_retry_send(Arc::clone(&socket), pacer.clone(), retry, Instant::now() + MAX_SEND_AGE);
                let PendingSend { result, .. } = pending;
                let (sent_at, prepared) = result.await?;
                circuit.commit_sent(prepared, sent_at)?;
                sent_by_worker.fetch_add(1, Ordering::Relaxed);
            }
            Ok::<(), String>(())
        }));
        retry_counts.push(sent);
    }

    let control_bytes =
        Packet::new(90_001, Message::PacketAck(PacketAck { packets: vec![Packets { id: 42 }] })).encode().expect("encode primary ACK");
    let control_pacer = pacers[0].handle();
    let control_socket = Arc::clone(&transports[0]);
    let control = spawn(async move { control_pacer.send(&control_socket, &control_bytes, TrafficClass::Control).await });

    for _ in 0..100 {
        if control.is_finished() {
            break;
        }
        advance(Duration::from_millis(10)).await;
        for _ in 0..8 {
            yield_now().await;
        }
    }
    assert!(control.is_finished(), "primary control must pass the queued neighbor retry load");
    control.await.expect("primary control task").expect("send primary ACK");

    let deadline_bytes = parse_hex_fixture(vectors::AGENT_UPDATE_INITIAL).expect("parse AgentUpdate fixture");
    assert!(matches!(Packet::decode(&deadline_bytes).expect("decode AgentUpdate fixture").message(), Message::AgentUpdate(_)));
    let deadline_pacer = pacers[0].handle();
    let deadline_socket = Arc::clone(&transports[0]);
    let deadline = spawn(async move { deadline_pacer.send(&deadline_socket, &deadline_bytes, TrafficClass::Deadline).await });

    for _ in 0..100 {
        if deadline.is_finished() {
            break;
        }
        advance(Duration::from_millis(10)).await;
        for _ in 0..8 {
            yield_now().await;
        }
    }
    assert!(deadline.is_finished(), "primary deadline traffic must pass the queued neighbor retry load");
    deadline.await.expect("primary deadline task").expect("send primary AgentUpdate");

    for _ in 0..100 {
        advance(Duration::from_millis(10)).await;
        for _ in 0..8 {
            yield_now().await;
        }
    }

    assert!(
        retry_counts.iter().all(|sent| sent.load(Ordering::Relaxed) > 0),
        "each neighbor retransmits while primary priority traffic is sent"
    );
    assert!(retry_workers.iter().all(|worker| !worker.is_finished()), "the neighbor retry queues remain backlogged after primary service");

    for worker in retry_workers {
        worker.abort();
        let _ = worker.await;
    }
    drop(pacers);
    budget.shutdown().await;
    drop(simulator_sockets);
}

#[test]
fn urgent_send_preempts_fair() {
    let circuit = Circuit::new();
    let mut pending = Some(pending_for_test(&circuit, queued_for_test(TrafficClass::Fair), TrafficClass::Fair));
    let mut queue = OutboundQueue::new();
    assert!(queue.push(queued_for_test(TrafficClass::Control), Some(TrafficClass::Fair)).is_ok());

    preempt_deferred_send(&mut pending, &mut queue, &circuit, false).expect("preempt deferred fair send");

    assert!(pending.is_none(), "the lower-class pacer request should be canceled");
    assert_eq!(queue.pop_next().expect("control send first").class, TrafficClass::Control);
    assert_eq!(queue.pop_next().expect("requeued fair send").class, TrafficClass::Fair);
}

#[test]
fn blocked_reliable_control_does_not_preempt_paced_fair_send() {
    let mut circuit = Circuit::new();
    let reliable = queued_for_test(TrafficClass::Fair).message;
    for _ in 0..MAX_PENDING_PACKETS {
        let prepared = circuit.prepare_message(reliable.clone(), Delivery::Reliable).expect("prepare tracked packet");
        circuit.commit_sent(prepared, Instant::now()).expect("track sent packet");
    }
    assert!(!circuit.has_reliable_capacity());

    let fair = queued_for_test(TrafficClass::Fair);
    let mut pending = Some(pending_for_test(&circuit, fair, TrafficClass::Fair));
    let mut queue = OutboundQueue::new();
    assert!(queue.push(queued_with_delivery_for_test(TrafficClass::Control, Delivery::Reliable), None).is_ok());

    preempt_deferred_send(&mut pending, &mut queue, &circuit, false).expect("check pending send priority");

    assert!(pending.is_some(), "ineligible reliable Control work must not cancel the Fair send");
    assert_eq!(queue.messages.len(), 1, "reliable Control work remains queued until an ACK opens a slot");
}

#[test]
fn duplicate_reack_waits_for_ack_deadline_before_preempting_send() {
    let mut circuit = Circuit::new();
    let now = Instant::now();
    let received = circuit.prepare_receive(42, true, [], now).expect("reserve initial reliable packet");
    circuit.commit_receive(received);
    let ack = circuit.prepare_acknowledgements().expect("prepare initial ACK").expect("initial ACK should be pending");
    circuit.commit_sent(ack, now).expect("clear initial ACK");

    let outgoing = circuit
        .prepare_message(Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 2 } }), Delivery::Reliable)
        .expect("prepare an outstanding reliable packet");
    let acknowledged_sequence = outgoing.sequence();
    circuit.commit_sent(outgoing, now).expect("record the outgoing reliable packet");
    assert!(circuit.has_pending(acknowledged_sequence));

    let queued = queued_for_test(TrafficClass::Fair);
    let mut pending = Some(pending_for_test(&circuit, queued, TrafficClass::Fair));
    let mut queue = OutboundQueue::new();
    let duplicate = circuit.prepare_receive(42, true, [acknowledged_sequence], now).expect("admit duplicate and process its appended ACK");
    assert!(matches!(duplicate, ReceiveAdmission::Duplicate(_)));
    assert!(!circuit.has_pending(acknowledged_sequence), "a duplicate's appended ACK still acknowledges outgoing data");
    assert!(circuit.has_pending_acks(), "duplicate should enqueue a re-ACK");

    preempt_deferred_send(&mut pending, &mut queue, &circuit, false).expect("defer duplicate re-ACK until its deadline");

    assert!(pending.is_some(), "an early re-ACK should not cancel a packet already waiting for the pacer");
    assert!(queue.messages.is_empty(), "the pending payload stays in flight before the ACK deadline");
}

#[tokio::test]
async fn due_ack_preempts_deferred_payload_and_runs_before_retry() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let socket = Arc::new(UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect transport"));
    let budget = OutboundBudget::new(8_000);
    let pacer = budget.register(1, false).await.expect("register simulator pacer");
    let mut circuit = Circuit::new();
    let now = Instant::now();
    let original = circuit
        .prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable)
        .expect("prepare outstanding reliable packet");
    circuit.commit_sent(original, now - Duration::from_secs(6)).expect("track outstanding reliable packet");
    let mut pending = Some(pending_for_test(&circuit, queued_for_test(TrafficClass::Fair), TrafficClass::Fair));
    let admission = circuit.prepare_receive(42, true, [], now).expect("reserve reliable receive");
    circuit.commit_receive(admission);
    let mut queue = OutboundQueue::new();

    preempt_deferred_send(&mut pending, &mut queue, &circuit, true).expect("prioritize due ACK");

    assert!(pending.is_none(), "a due standalone ACK cancels a pacer-deferred payload");
    assert!(queue.messages.iter().any(|queued| queued.class == TrafficClass::Fair));
    let mut deferred_retries = HashMap::new();
    let ack = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, true, false)
        .map_err(|(_, error)| error)
        .expect("prepare due standalone ACK")
        .expect("due ACK should be prepared first");
    let packet = send_pending(ack, &mut circuit, &simulator).await;
    assert!(matches!(packet.message(), Message::PacketAck(_)));
    assert!(queue.messages.iter().any(|queued| queued.class == TrafficClass::Fair));

    drop(pacer);
    budget.shutdown().await;
}
