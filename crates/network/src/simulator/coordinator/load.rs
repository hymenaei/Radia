use super::*;

#[tokio::test]
#[ignore = "manual loopback latency measurement; run with --release --ignored --nocapture"]
async fn connection_latency() {
    let mut harness = connected_neighbor().await;
    let direct_server = UdpSocket::bind("127.0.0.1:0").await.expect("bind direct simulator");
    let direct_socket = UdpTransport::connect(direct_server.local_addr().expect("direct address")).await.expect("connect direct socket");
    let mut direct_circuit = Circuit::new();
    let mut direct = Vec::with_capacity(1024);
    let mut coordinated = Vec::with_capacity(1024);
    for index in 0..1088 {
        let started = std::time::Instant::now();
        direct_circuit.send(&direct_socket, object_request(), Delivery::Unreliable).await.expect("direct send");
        let elapsed = started.elapsed();
        receive_message(&direct_server, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
        if index >= 64 {
            direct.push(elapsed);
        }

        let (completed, response) = oneshot::channel();
        let started = std::time::Instant::now();
        harness
            .commands
            .send(Command::SendTo {
                simulator_id: harness.primary_id,
                message: object_request(),
                delivery: Delivery::Unreliable,
                completed,
                dispatch_probe: None,
            })
            .await
            .expect("queue coordinated send");
        response.await.expect("send completion").expect("coordinated local send");
        let elapsed = started.elapsed();
        receive_message(&harness.primary, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
        if index >= 64 {
            coordinated.push(elapsed);
        }
    }
    let mut intake = Vec::with_capacity(16);
    let mut acknowledgements = Vec::with_capacity(16);
    for sequence in 100..116 {
        let mut chat = fixture("chat_from_simulator.hex");
        chat[0] |= 0x40;
        set_sequence(&mut chat, sequence);
        let started = std::time::Instant::now();
        harness.neighbor.send_to(&chat, harness.neighbor_peer).await.expect("send measured reliable data");
        loop {
            if matches!(next_raw_event(&mut harness.events).await, Event::SimulatorMessage { simulator_id, sequence: received, .. } if simulator_id == harness.neighbor_id && received == sequence)
            {
                break;
            }
        }
        intake.push(started.elapsed());
        receive_ack(&harness.neighbor, sequence).await;
        acknowledgements.push(started.elapsed());
    }
    for (name, mut samples) in [
        ("direct local send", direct),
        ("coordinated local send", coordinated),
        ("coordinator intake", intake),
        ("ACK timer", acknowledgements),
    ] {
        samples.sort_unstable();
        eprintln!(
            "{name}: n={}, p50={:?}, p95={:?}, p99={:?}, max={:?}",
            samples.len(),
            samples[samples.len() / 2],
            samples[(samples.len() - 1) * 95 / 100],
            samples[(samples.len() - 1) * 99 / 100],
            samples.last().expect("samples")
        );
    }
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual measurement; run cargo test -p network --release five_simulator_load -- --ignored --nocapture"]
async fn five_simulator_load() {
    let mut harness = connected_neighbor().await;
    let mut additional_neighbors = Vec::with_capacity(3);
    for index in 0..3 {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
        let endpoint = simulator.local_addr().expect("neighbor address");
        let region_handle = 0x2000_0000_0000_2000 + u64::try_from(index).expect("neighbor index fits u64");
        announce_neighbor(
            &harness.primary,
            harness.primary_peer,
            endpoint,
            region_handle,
            31 + u32::try_from(index).expect("neighbor index fits u32"),
        )
        .await;

        let (use_circuit, peer) = receive(&simulator).await;
        let use_circuit = Packet::decode(&use_circuit).expect("decode neighbor UseCircuitCode");
        assert!(matches!(use_circuit.message(), Message::UseCircuitCode(_)));
        simulator.send_to(&ack(40, use_circuit.sequence()), peer).await.expect("ack neighbor circuit");

        let simulator_id = timeout(WAIT, async {
            loop {
                if let Event::SimulatorConnected { simulator_id, region_handle: Some(actual), endpoint: actual_endpoint } =
                    next_raw_event(&mut harness.events).await
                    && actual == region_handle
                    && actual_endpoint == endpoint
                {
                    break simulator_id;
                }
            }
        })
        .await
        .expect("neighbor connection should become active");
        additional_neighbors.push((simulator, simulator_id, peer));
    }

    let warmup = Duration::from_secs(1);
    let duration = Duration::from_secs(10);
    let measurement_start = Instant::now() + warmup;
    let measurement_end = measurement_start + duration;
    let drain_end = measurement_end + Duration::from_secs(1);
    let mut events = std::mem::replace(&mut harness.events, mpsc::channel(1).1);
    let event_drain = spawn(async move {
        let mut drained = 0usize;
        while events.recv().await.is_some() {
            drained += 1;
        }
        drained
    });
    let process_memory = begin_process_measurement();
    let started = std::time::Instant::now();
    let (
        primary_load,
        first_neighbor_load,
        second_neighbor_load,
        third_neighbor_load,
        fourth_neighbor_load,
        primary_sends,
        first_neighbor_sends,
        second_neighbor_sends,
        third_neighbor_sends,
        fourth_neighbor_sends,
    ) = tokio::join!(
        run_load_receiver(&harness.primary, harness.primary_peer, 0, measurement_start, measurement_end, drain_end),
        run_load_receiver(&harness.neighbor, harness.neighbor_peer, 1, measurement_start, measurement_end, drain_end),
        run_load_receiver(&additional_neighbors[0].0, additional_neighbors[0].2, 2, measurement_start, measurement_end, drain_end,),
        run_load_receiver(&additional_neighbors[1].0, additional_neighbors[1].2, 3, measurement_start, measurement_end, drain_end,),
        run_load_receiver(&additional_neighbors[2].0, additional_neighbors[2].2, 4, measurement_start, measurement_end, drain_end,),
        loaded_send_window(&harness.commands, harness.primary_id, measurement_start, measurement_end),
        loaded_send_window(&harness.commands, harness.neighbor_id, measurement_start, measurement_end),
        loaded_send_window(&harness.commands, additional_neighbors[0].1, measurement_start, measurement_end),
        loaded_send_window(&harness.commands, additional_neighbors[1].1, measurement_start, measurement_end),
        loaded_send_window(&harness.commands, additional_neighbors[2].1, measurement_start, measurement_end),
    );
    let elapsed = started.elapsed();
    let (process_baseline, process_live_delta, process_peak, process_elapsed_ns) = process_memory.finish();
    let process_peak_delta = process_peak.saturating_sub(process_baseline);
    let primary_fair_bytes = primary_load.fair_bytes;
    let first_neighbor_fair_bytes = first_neighbor_load.fair_bytes;
    let second_neighbor_fair_bytes = second_neighbor_load.fair_bytes;
    let third_neighbor_fair_bytes = third_neighbor_load.fair_bytes;
    let fourth_neighbor_fair_bytes = fourth_neighbor_load.fair_bytes;
    let measurements = [
        ("primary", primary_fair_bytes, primary_sends),
        ("neighbor-1", first_neighbor_fair_bytes, first_neighbor_sends),
        ("neighbor-2", second_neighbor_fair_bytes, second_neighbor_sends),
        ("neighbor-3", third_neighbor_fair_bytes, third_neighbor_sends),
        ("neighbor-4", fourth_neighbor_fair_bytes, fourth_neighbor_sends),
    ];
    let total_bytes: usize = measurements.iter().map(|(_, bytes, _)| *bytes).sum();
    let neighbor_average =
        (first_neighbor_fair_bytes + second_neighbor_fair_bytes + third_neighbor_fair_bytes + fourth_neighbor_fair_bytes) as f64 / 4.0;
    assert!(neighbor_average > 0.0, "neighbors received no traffic during the load run");
    eprintln!(
        "five-simulator load: capture=off, warmup={warmup:?}, measured={duration:?}, elapsed={elapsed:?}, rate={:.0} fair B/s, primary/neighbor-average={:.2}, configured-rate={} B/s",
        total_bytes as f64 / duration.as_secs_f64(),
        primary_fair_bytes as f64 / neighbor_average,
        crate::session::DEFAULT_UPSTREAM_BYTES_PER_SECOND.get()
    );
    eprintln!(
        "five-simulator process allocation sample: baseline={process_baseline} B, live_delta={process_live_delta} B, peak_delta={process_peak_delta} B, elapsed={process_elapsed_ns} ns; process-wide allocator sample includes test harness, Tokio, sockets, and event drain, excludes allocator metadata and RSS"
    );

    if let Some(probe) = &harness.pacer_probe {
        let snapshot = probe.snapshot(measurement_start.into(), measurement_end.into());
        let all_fair_flows =
            snapshot.pending_duration.get(&TrafficClass::Fair).and_then(|counts| counts.get(&5)).copied().unwrap_or_default();
        eprintln!("five-simulator scheduler window: all-five-fair-backlogged={all_fair_flows:?}");
        for (simulator_id, connection) in snapshot.connections {
            let fair = connection.classes.get(&TrafficClass::Fair).copied().unwrap_or_default();
            eprintln!(
                "five-simulator scheduler simulator={simulator_id}: grants={}, granted-bytes={}, wire-bytes={}, wait-p50={:?}, wait-p95={:?}, wait-p99={:?}, retries={}, retry-wire-bytes={}, queue-peak-messages={}, queue-peak-charged-bytes={}, queue-peak-oldest-age={:?}, deferred-retry-expired={}, active-retry-expired={}",
                fair.grants,
                fair.granted_bytes,
                fair.wire_bytes,
                fair.wait_p50,
                fair.wait_p95,
                fair.wait_p99,
                connection.retry_attempts,
                connection.retry_wire_bytes,
                connection.peak_queue_messages,
                connection.peak_queue_bytes,
                connection.peak_queue_age,
                connection.deferred_retry_expirations,
                connection.active_retry_expirations,
            );
            for (class, metrics) in connection.classes {
                eprintln!(
                    "five-simulator local queue simulator={simulator_id} class={class:?}: peak-oldest-age={:?}, outbound-queue-expired={}, pending-send-expired={}",
                    metrics.peak_queue_age, metrics.outbound_queue_expirations, metrics.pending_send_expirations,
                );
            }
        }
    }

    for (name, bytes, (mut latencies, mut queue_residence, errors)) in measurements {
        assert!(bytes > 0, "{name} received no traffic during the load run");
        assert!(!latencies.is_empty(), "{name} completed no local sends during the load run");
        latencies.sort_unstable();
        queue_residence.sort_unstable();
        let p50 = latencies[(latencies.len() - 1) * 50 / 100];
        let p95 = latencies[(latencies.len() - 1) * 95 / 100];
        let p99 = latencies[(latencies.len() - 1) * 99 / 100];
        let queue_p50 = queue_residence[(queue_residence.len() - 1) * 50 / 100];
        let queue_p95 = queue_residence[(queue_residence.len() - 1) * 95 / 100];
        let queue_p99 = queue_residence[(queue_residence.len() - 1) * 99 / 100];
        eprintln!(
            "{name}: fair-bytes={bytes}, rate={:.0} B/s, local-sends={}, coordinator-dispatches={}, errors={errors:?}, command-queue residence p50={queue_p50:?} p95={queue_p95:?} p99={queue_p99:?}, producer-to-local-send p50={p50:?} p95={p95:?} p99={p99:?}",
            bytes as f64 / duration.as_secs_f64(),
            latencies.len(),
            queue_residence.len()
        );
    }
    for (name, report) in [
        ("primary", primary_load.acknowledgements),
        ("neighbor-1", first_neighbor_load.acknowledgements),
        ("neighbor-2", second_neighbor_load.acknowledgements),
        ("neighbor-3", third_neighbor_load.acknowledgements),
        ("neighbor-4", fourth_neighbor_load.acknowledgements),
    ] {
        assert!(report.sent >= 300, "{name} sent too few reliable samples for useful ACK percentiles: {}", report.sent);
        assert!(report.appended.len() + report.standalone.len() >= 300, "{name} received too few ACKs for useful percentiles");
        assert_eq!(report.outstanding, 0, "{name} left reliable samples unacknowledged after drain");
        assert_eq!(report.appended.len() + report.standalone.len(), report.sent, "{name} did not observe every reliable ACK");
        eprintln!(
            "five-simulator inbound ACK simulator={name}: sent={}, acknowledged={}, outstanding={}, appended={}, appended-p50/p95/p99={:?}, standalone={}, standalone-p50/p95/p99={:?}",
            report.sent,
            report.appended.len() + report.standalone.len(),
            report.outstanding,
            report.appended.len(),
            duration_percentiles(&report.appended),
            report.standalone.len(),
            duration_percentiles(&report.standalone),
        );
    }

    let timer_start = Instant::now() + Duration::from_millis(100);
    let timer_end = timer_start + Duration::from_secs(3);
    let timer_drain_end = timer_end + Duration::from_millis(500);
    let (timer_acknowledgements, first_neighbor_sends, second_neighbor_sends, third_neighbor_sends, fourth_neighbor_sends) = tokio::join!(
        run_timer_ack_receiver(&harness.primary, harness.primary_peer, timer_start, timer_end, timer_drain_end),
        loaded_send_window(&harness.commands, harness.neighbor_id, timer_start, timer_end),
        loaded_send_window(&harness.commands, additional_neighbors[0].1, timer_start, timer_end),
        loaded_send_window(&harness.commands, additional_neighbors[1].1, timer_start, timer_end),
        loaded_send_window(&harness.commands, additional_neighbors[2].1, timer_start, timer_end),
    );
    assert!(timer_acknowledgements.sent >= 80, "timer-only ACK phase sent too few reliable samples");
    assert_eq!(timer_acknowledgements.outstanding, 0, "timer-only ACK phase left samples unacknowledged");
    assert_eq!(
        timer_acknowledgements.appended.len() + timer_acknowledgements.standalone.len(),
        timer_acknowledgements.sent,
        "primary-idle ACK phase did not observe every ACK"
    );
    eprintln!(
        "five-simulator primary-idle ACK under four-simulator outbound load: sent={}, appended={}, appended-p50/p95/p99={:?}, standalone={}, standalone-p50/p95/p99={:?}, outstanding={}",
        timer_acknowledgements.sent,
        timer_acknowledgements.appended.len(),
        duration_percentiles(&timer_acknowledgements.appended),
        timer_acknowledgements.standalone.len(),
        duration_percentiles(&timer_acknowledgements.standalone),
        timer_acknowledgements.outstanding,
    );
    for (name, (_, queue_residence, errors)) in [
        ("neighbor-1", first_neighbor_sends),
        ("neighbor-2", second_neighbor_sends),
        ("neighbor-3", third_neighbor_sends),
        ("neighbor-4", fourth_neighbor_sends),
    ] {
        assert!(!queue_residence.is_empty(), "{name} had no command dispatches during the timer-only ACK phase");
        eprintln!("timer-only ACK phase outbound {name}: coordinator-dispatches={}, queue-full/errors={errors:?}", queue_residence.len());
    }

    let drained = [
        ("primary", drain_pending_datagrams(&harness.primary)),
        ("neighbor-1", drain_pending_datagrams(&harness.neighbor)),
        ("neighbor-2", drain_pending_datagrams(&additional_neighbors[0].0)),
        ("neighbor-3", drain_pending_datagrams(&additional_neighbors[1].0)),
        ("neighbor-4", drain_pending_datagrams(&additional_neighbors[2].0)),
    ];
    eprintln!("five-simulator burst setup: prior datagrams drained={drained:?}");

    const BURST_SENDS_PER_SIMULATOR: usize = 6;
    let burst_start = Instant::now();
    let burst_barrier = Arc::new(Barrier::new(5));
    let (primary_burst, first_neighbor_burst, second_neighbor_burst, third_neighbor_burst, fourth_neighbor_burst) = tokio::join!(
        run_burst_on_simulator(
            &harness.commands,
            harness.primary_id,
            &harness.primary,
            BURST_SENDS_PER_SIMULATOR,
            Arc::clone(&burst_barrier)
        ),
        run_burst_on_simulator(
            &harness.commands,
            harness.neighbor_id,
            &harness.neighbor,
            BURST_SENDS_PER_SIMULATOR,
            Arc::clone(&burst_barrier)
        ),
        run_burst_on_simulator(
            &harness.commands,
            additional_neighbors[0].1,
            &additional_neighbors[0].0,
            BURST_SENDS_PER_SIMULATOR,
            Arc::clone(&burst_barrier),
        ),
        run_burst_on_simulator(
            &harness.commands,
            additional_neighbors[1].1,
            &additional_neighbors[1].0,
            BURST_SENDS_PER_SIMULATOR,
            Arc::clone(&burst_barrier),
        ),
        run_burst_on_simulator(
            &harness.commands,
            additional_neighbors[2].1,
            &additional_neighbors[2].0,
            BURST_SENDS_PER_SIMULATOR,
            burst_barrier,
        ),
    );
    let burst_elapsed = burst_start.elapsed();
    let burst_reports = [
        ("primary", primary_burst),
        ("neighbor-1", first_neighbor_burst),
        ("neighbor-2", second_neighbor_burst),
        ("neighbor-3", third_neighbor_burst),
        ("neighbor-4", fourth_neighbor_burst),
    ];
    let burst_bytes: usize = burst_reports.iter().map(|(_, report)| report.received.bytes).sum();
    let burst_received: usize = burst_reports.iter().map(|(_, report)| report.received.requests).sum();
    eprintln!(
        "five-simulator Fair burst: capture=off, sends-per-simulator={BURST_SENDS_PER_SIMULATOR}, objects-per-request={}, received={burst_received}, wire-bytes={burst_bytes}, elapsed={burst_elapsed:?}, aggregate-rate={:.0} B/s, configured-rate={} B/s",
        u8::MAX as usize,
        burst_bytes as f64 / burst_elapsed.as_secs_f64(),
        crate::session::DEFAULT_UPSTREAM_BYTES_PER_SECOND.get()
    );
    for (name, report) in burst_reports {
        eprintln!(
            "{name} burst: sent={}, received={}, wire-bytes={}, receive-timed-out={}, send-errors={:?}, command-residence p50/p95/p99={:?}, producer-to-local-send p50/p95/p99={:?}, receive-first-to-last={:?}",
            report.sent.successful,
            report.received.requests,
            report.received.bytes,
            report.received.timed_out,
            report.sent.errors,
            duration_percentiles(&report.sent.queue_residence),
            duration_percentiles(&report.sent.latencies),
            report.received.first_to_last,
        );
        assert_eq!(report.sent.successful, report.received.requests, "{name} burst sends should arrive at its loopback simulator");
    }
    if let Some(probe) = &harness.pacer_probe {
        let snapshot = probe.snapshot(burst_start.into(), Instant::now().into());
        for (simulator_id, connection) in snapshot.connections {
            let fair = connection.classes.get(&TrafficClass::Fair).copied().unwrap_or_default();
            eprintln!(
                "five-simulator burst scheduler simulator={simulator_id}: fair-grants={}, granted-bytes={}, wire-bytes={}, wait-p50/p95/p99={:?}, queue-peak-messages={}, queue-peak-charged-bytes={}, queue-peak-oldest-age={:?}, deferred-retry-expired={}, active-retry-expired={}",
                fair.grants,
                fair.granted_bytes,
                fair.wire_bytes,
                (fair.wait_p50, fair.wait_p95, fair.wait_p99),
                connection.peak_queue_messages,
                connection.peak_queue_bytes,
                connection.peak_queue_age,
                connection.deferred_retry_expirations,
                connection.active_retry_expirations,
            );
            for (class, metrics) in connection.classes {
                eprintln!(
                    "five-simulator burst local queue simulator={simulator_id} class={class:?}: peak-oldest-age={:?}, outbound-queue-expired={}, pending-send-expired={}",
                    metrics.peak_queue_age, metrics.outbound_queue_expirations, metrics.pending_send_expirations,
                );
            }
        }
    }
    harness.shutdown().await;
    eprintln!("five-simulator event drain: {} events consumed", event_drain.await.expect("event drain task"));
}

#[derive(Default)]
struct LoadAcknowledgements {
    sent: usize,
    appended: Vec<Duration>,
    standalone: Vec<Duration>,
    outstanding: usize,
}

struct LoadReceiverReport {
    fair_bytes: usize,
    acknowledgements: LoadAcknowledgements,
}

struct BurstSendReport {
    successful: usize,
    errors: HashMap<String, usize>,
    latencies: Vec<Duration>,
    queue_residence: Vec<Duration>,
}

#[derive(Default)]
struct BurstReceiveReport {
    requests: usize,
    bytes: usize,
    first_to_last: Option<Duration>,
    timed_out: bool,
}

struct BurstReport {
    sent: BurstSendReport,
    received: BurstReceiveReport,
}

fn drain_pending_datagrams(socket: &UdpSocket) -> usize {
    let mut buffer = [0; 65_535];
    let mut drained = 0;
    loop {
        match socket.try_recv_from(&mut buffer) {
            Ok(_) => drained += 1,
            Err(error) if error.kind() == ErrorKind::WouldBlock => return drained,
            Err(error) => panic!("drain prior loopback datagrams before burst: {error}"),
        }
    }
}

async fn run_burst_on_simulator(
    commands: &mpsc::Sender<Command>,
    simulator_id: SimulatorId,
    socket: &UdpSocket,
    send_count: usize,
    barrier: Arc<Barrier>,
) -> BurstReport {
    let (sent, expected) = oneshot::channel();
    let send = async {
        barrier.wait().await;
        send_burst(commands, simulator_id, send_count, sent).await
    };
    let receive = receive_burst(socket, expected);
    let (sent, received) = tokio::join!(send, receive);
    BurstReport { sent, received }
}

async fn send_burst(
    commands: &mpsc::Sender<Command>,
    simulator_id: SimulatorId,
    send_count: usize,
    sent: oneshot::Sender<usize>,
) -> BurstSendReport {
    let mut pending = VecDeque::with_capacity(send_count);
    let mut report = BurstSendReport {
        successful: 0,
        errors: HashMap::new(),
        latencies: Vec::with_capacity(send_count),
        queue_residence: Vec::with_capacity(send_count),
    };

    for _ in 0..send_count {
        let message = burst_object_request();
        let started_at = Instant::now();
        let (completed, response) = oneshot::channel();
        let (dispatch_probe, dispatched) = oneshot::channel();
        let permit = match commands.reserve().await {
            Ok(permit) => permit,
            Err(_) => {
                *report.errors.entry("coordinator command channel closed".to_owned()).or_insert(0) += 1;
                break;
            }
        };
        let enqueued_at = Instant::now();
        permit.send(Command::SendTo {
            simulator_id,
            message,
            delivery: Delivery::Unreliable,
            completed,
            dispatch_probe: Some(dispatch_probe),
        });
        pending.push_back((response, dispatched, started_at, enqueued_at));
    }

    for (response, dispatched, started_at, enqueued_at) in pending {
        match dispatched.await {
            Ok(dispatched_at) => report.queue_residence.push(dispatched_at.duration_since(enqueued_at)),
            Err(_) => *report.errors.entry("coordinator dispatch cancelled".to_owned()).or_insert(0) += 1,
        }
        match response.await {
            Ok(Ok(())) => {
                report.successful += 1;
                report.latencies.push(started_at.elapsed());
            }
            Ok(Err(error)) => *report.errors.entry(error.to_string()).or_insert(0) += 1,
            Err(_) => *report.errors.entry("send completion cancelled".to_owned()).or_insert(0) += 1,
        }
    }

    let _ = sent.send(report.successful);
    report
}

async fn receive_burst(socket: &UdpSocket, mut expected: oneshot::Receiver<usize>) -> BurstReceiveReport {
    let mut report = BurstReceiveReport::default();
    let mut expected_requests = None;
    let mut first = None;
    let mut last = None;
    let mut server_sequence = 30_000;
    let mut buffer = vec![0; 65_535];
    let received = timeout(Duration::from_secs(5), async {
        loop {
            if expected_requests.is_some_and(|count| report.requests >= count) {
                break;
            }
            tokio::select! {
                count = &mut expected, if expected_requests.is_none() => {
                    expected_requests = Some(count.unwrap_or_default());
                }
                received = socket.recv_from(&mut buffer) => {
                    let Ok((size, peer)) = received else { break };
                    let packet = Packet::decode(&buffer[..size]).expect("decode burst packet");
                    match packet.message() {
                        Message::RequestMultipleObjects(_) => {
                            report.requests += 1;
                            report.bytes = report.bytes.saturating_add(size);
                            let at = Instant::now();
                            first.get_or_insert(at);
                            last = Some(at);
                        }
                        Message::StartPingCheck(ping) => {
                            let reply = Packet::new(
                                server_sequence,
                                Message::CompletePingCheck(CompletePingCheck { ping_id: ping.ping_id }),
                            )
                            .encode()
                            .expect("encode burst ping reply");
                            socket.send_to(&reply, peer).await.expect("reply to viewer ping during burst phase");
                            server_sequence = server_sequence.wrapping_add(1);
                        }
                        _ => {}
                    }
                }
            }
        }
    })
    .await;
    report.timed_out = received.is_err();
    report.first_to_last = first.zip(last).map(|(first, last)| last.duration_since(first));
    report
}

async fn run_load_receiver(
    socket: &UdpSocket,
    peer: SocketAddr,
    simulator_index: u32,
    measurement_start: Instant,
    measurement_end: Instant,
    drain_end: Instant,
) -> LoadReceiverReport {
    let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    heartbeat.tick().await;
    let mut sequence = 100;
    let mut ping_id = 0;
    let mut reliable_sequence = 100_000 + simulator_index * 10_000;
    let reliable_period = Duration::from_millis(25);
    let mut reliable_send = tokio::time::interval_at((measurement_start + reliable_period).into(), reliable_period);
    reliable_send.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut buffer = vec![0; 65_535];
    let mut fair_bytes = 0usize;
    let mut pending_acknowledgements = HashMap::with_capacity(512);
    let mut acknowledgements = LoadAcknowledgements::default();
    loop {
        let remaining = drain_end.saturating_duration_since(Instant::now());
        if remaining == Duration::ZERO {
            acknowledgements.outstanding = pending_acknowledgements.len();
            return LoadReceiverReport { fair_bytes, acknowledgements };
        }
        let received = tokio::select! {
            _ = reliable_send.tick(), if Instant::now() < measurement_end => {
                let mut packet = fixture("chat_from_simulator.hex");
                packet[0] |= 0x40;
                set_sequence(&mut packet, reliable_sequence);
                let sent_at = Instant::now();
                pending_acknowledgements.insert(reliable_sequence, sent_at);
                socket.send_to(&packet, peer).await.expect("send reliable load sample");
                acknowledgements.sent += 1;
                reliable_sequence = reliable_sequence.wrapping_add(1);
                continue;
            }
            _ = heartbeat.tick() => {
                socket.send_to(&start_ping_packet(sequence, ping_id, 0), peer).await.expect("keep simulator circuit active");
                sequence = sequence.wrapping_add(1);
                ping_id = ping_id.wrapping_add(1);
                continue;
            }
            received = timeout(remaining, socket.recv_from(&mut buffer)) => received,
        };
        match received {
            Ok(Ok((size, _))) => {
                let received_at = Instant::now();
                let packet = Packet::decode(&buffer[..size]).expect("decode outgoing simulator packet");
                if received_at >= measurement_start
                    && received_at < measurement_end
                    && matches!(packet.message(), Message::RequestMultipleObjects(_))
                {
                    fair_bytes = fair_bytes.saturating_add(size);
                }
                record_load_acknowledgements(&packet, received_at, &mut pending_acknowledgements, &mut acknowledgements);
            }
            _ => {
                acknowledgements.outstanding = pending_acknowledgements.len();
                return LoadReceiverReport { fair_bytes, acknowledgements };
            }
        }
    }
}

async fn run_timer_ack_receiver(
    socket: &UdpSocket,
    peer: SocketAddr,
    measurement_start: Instant,
    measurement_end: Instant,
    drain_end: Instant,
) -> LoadAcknowledgements {
    let reliable_period = Duration::from_millis(25);
    let mut reliable_send = tokio::time::interval_at((measurement_start + reliable_period).into(), reliable_period);
    reliable_send.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut buffer = vec![0; 65_535];
    let mut reliable_sequence = 120_000;
    let mut server_sequence = 20_000;
    let mut pending_acknowledgements = HashMap::with_capacity(160);
    let mut acknowledgements = LoadAcknowledgements::default();

    loop {
        let remaining = drain_end.saturating_duration_since(Instant::now());
        if remaining == Duration::ZERO {
            acknowledgements.outstanding = pending_acknowledgements.len();
            return acknowledgements;
        }

        let received = tokio::select! {
            _ = reliable_send.tick(), if Instant::now() < measurement_end => {
                let mut packet = fixture("chat_from_simulator.hex");
                packet[0] |= 0x40;
                set_sequence(&mut packet, reliable_sequence);
                pending_acknowledgements.insert(reliable_sequence, Instant::now());
                socket.send_to(&packet, peer).await.expect("send timer-only reliable sample");
                acknowledgements.sent += 1;
                reliable_sequence = reliable_sequence.wrapping_add(1);
                continue;
            }
            received = timeout(remaining, socket.recv_from(&mut buffer)) => received,
        };

        let Ok(Ok((size, _))) = received else {
            acknowledgements.outstanding = pending_acknowledgements.len();
            return acknowledgements;
        };

        let received_at = Instant::now();
        let packet = Packet::decode(&buffer[..size]).expect("decode timer-only ACK phase packet");
        record_load_acknowledgements(&packet, received_at, &mut pending_acknowledgements, &mut acknowledgements);
        if let Message::StartPingCheck(ping) = packet.message() {
            let reply = Packet::new(server_sequence, Message::CompletePingCheck(CompletePingCheck { ping_id: ping.ping_id }))
                .encode()
                .expect("encode timer-only phase ping reply");
            socket.send_to(&reply, peer).await.expect("reply to viewer ping during timer-only phase");
            server_sequence = server_sequence.wrapping_add(1);
        }
    }
}

fn record_load_acknowledgements(
    packet: &Packet,
    received_at: Instant,
    pending: &mut HashMap<u32, Instant>,
    report: &mut LoadAcknowledgements,
) {
    for acknowledged_sequence in packet.appended_acks() {
        if let Some(sent_at) = pending.remove(acknowledged_sequence) {
            report.appended.push(received_at.duration_since(sent_at));
        }
    }
    if let Message::PacketAck(packet_ack) = packet.message() {
        for acknowledged_sequence in packet_ack.packets.iter().map(|packet| packet.id) {
            if let Some(sent_at) = pending.remove(&acknowledged_sequence) {
                report.standalone.push(received_at.duration_since(sent_at));
            }
        }
    }
}

async fn loaded_send_window(
    commands: &mpsc::Sender<Command>,
    simulator_id: SimulatorId,
    measurement_start: Instant,
    measurement_end: Instant,
) -> (Vec<Duration>, Vec<Duration>, HashMap<String, usize>) {
    const SEND_WINDOW: usize = 8;

    let mut pending = VecDeque::with_capacity(SEND_WINDOW);
    let mut latencies = Vec::with_capacity(1024);
    let mut queue_residence = Vec::with_capacity(1024);
    let mut errors = HashMap::new();
    loop {
        while pending.len() < SEND_WINDOW && Instant::now() < measurement_end {
            let (completed, response) = oneshot::channel();
            let (dispatch_probe, dispatched) = oneshot::channel();
            let started_at = Instant::now();
            let permit = match commands.reserve().await {
                Ok(permit) => permit,
                Err(_) => {
                    *errors.entry("coordinator command channel closed".to_owned()).or_insert(0) += 1;
                    return (latencies, queue_residence, errors);
                }
            };
            let enqueued_at = Instant::now();
            let in_measurement = enqueued_at >= measurement_start;
            permit.send(Command::SendTo {
                simulator_id,
                message: object_request(),
                delivery: Delivery::Unreliable,
                completed,
                dispatch_probe: Some(dispatch_probe),
            });
            pending.push_back((response, dispatched, started_at, enqueued_at, in_measurement));
        }

        let Some((response, dispatched, started_at, enqueued_at, in_measurement)) = pending.pop_front() else {
            return (latencies, queue_residence, errors);
        };
        match dispatched.await {
            Ok(dispatched_at) if in_measurement => queue_residence.push(dispatched_at.duration_since(enqueued_at)),
            Err(_) if in_measurement => *errors.entry("coordinator dispatch cancelled".to_owned()).or_insert(0) += 1,
            _ => {}
        }
        match response.await {
            Ok(Ok(())) if in_measurement => latencies.push(started_at.elapsed()),
            Ok(Err(error)) if in_measurement => *errors.entry(error.to_string()).or_insert(0) += 1,
            Err(_) if in_measurement => *errors.entry("send completion cancelled".to_owned()).or_insert(0) += 1,
            _ => {}
        }
    }
}

#[tokio::test]
async fn disconnect_cancels_primary_startup() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let endpoint = simulator.local_addr().expect("primary simulator address");
    let (send_gate, pause_sends, send_entered) = UdpTransport::new_send_gate();
    pause_sends.send_replace(true);
    let (disconnect, disconnect_requested) = watch::channel(false);
    let status = NetworkStatusHandle::default();
    let startup = spawn(SimulatorCoordinator::connect_with_gate_and_disconnect(
        login_session(endpoint),
        ResponseData::default(),
        None,
        status.clone(),
        send_gate,
        disconnect_requested,
    ));

    timeout(WAIT, send_entered.notified()).await.expect("initial UseCircuitCode reaches the gated local send");
    let mut packet = [0; 1024];
    assert_eq!(simulator.try_recv_from(&mut packet).expect_err("gated send must not reach the simulator").kind(), ErrorKind::WouldBlock);

    disconnect.send_replace(true);
    assert!(
        timeout(WAIT, startup)
            .await
            .expect("startup cancellation is bounded")
            .expect("startup task")
            .expect("clean cancellation")
            .is_none()
    );
    let status = status.snapshot();
    assert_eq!(status.lifecycle, NetworkLifecycle::Disconnected);
    assert_eq!(status.udp_packets_sent, 0);
    assert!(status.last_error.is_none());
}

#[tokio::test]
async fn stalled_send_is_isolated() {
    let mut harness = connected_neighbor().await;
    harness.pause_sends.send_replace(true);
    let (completed, mut response) = oneshot::channel();
    harness
        .commands
        .send(Command::SendTo {
            simulator_id: harness.primary_id,
            message: object_request(),
            delivery: Delivery::Reliable,
            completed,
            dispatch_probe: None,
        })
        .await
        .expect("dispatch primary send");
    timeout(WAIT, harness.send_entered.notified()).await.expect("primary enters local send");
    assert!(matches!(response.try_recv(), Err(oneshot::error::TryRecvError::Empty)));

    let neighbor_send = spawn({
        let commands = harness.commands.clone();
        let id = harness.neighbor_id;
        async move { send_targeted(&commands, id, object_request()).await }
    });
    let sent = receive_message(&harness.neighbor, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
    assert!(timeout(WAIT, neighbor_send).await.expect("unrelated send completion").expect("send task").is_ok());
    harness.neighbor.send_to(&ack(41, sent.sequence()), harness.neighbor_peer).await.expect("ack neighbor request");

    let mut chat = fixture("chat_from_simulator.hex");
    chat[0] |= 0x40;
    set_sequence(&mut chat, 42);
    harness.neighbor.send_to(&chat, harness.neighbor_peer).await.expect("send unrelated reliable data");
    loop {
        if matches!(next_raw_event(&mut harness.events).await, Event::SimulatorMessage { simulator_id, sequence: 42, .. } if simulator_id == harness.neighbor_id)
        {
            break;
        }
    }
    receive_ack(&harness.neighbor, 42).await;
    harness.neighbor.send_to(&start_ping_packet(43, 9, 0), harness.neighbor_peer).await.expect("ping unrelated connection");
    receive_message(&harness.neighbor, |message| matches!(message, Message::CompletePingCheck(ping) if ping.ping_id.ping_id == 9)).await;
    assert!(matches!(response.try_recv(), Err(oneshot::error::TryRecvError::Empty)), "primary local send is still pending");

    let (abandoned, abandoned_receiver) = oneshot::channel();
    drop(abandoned_receiver);
    harness
        .commands
        .send(Command::SendTo {
            simulator_id: harness.primary_id,
            message: object_request(),
            delivery: Delivery::Reliable,
            completed: abandoned,
            dispatch_probe: None,
        })
        .await
        .expect("admit abandoned send");
    harness.neighbor.send_to(&start_ping_packet(44, 10, 0), harness.neighbor_peer).await.expect("ping after abandoned send");
    receive_message(&harness.neighbor, |message| matches!(message, Message::CompletePingCheck(ping) if ping.ping_id.ping_id == 10)).await;

    harness.disconnect.send_replace(true);
    assert!(
        timeout(LOGOUT_TIMEOUT + WAIT, harness.client)
            .await
            .expect("bounded shutdown despite stalled send")
            .expect("coordinator task")
            .is_ok()
    );
    assert!(matches!(response.await, Ok(Err(Error::SimulatorConnectionClosed))));
    assert_eq!(harness.packet_memory.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
}

#[tokio::test]
async fn abandoned_send_does_not_block() {
    let harness = connected_neighbor().await;
    harness.pause_sends.send_replace(true);

    let (first, first_response) = oneshot::channel();
    harness
        .commands
        .send(Command::SendTo {
            simulator_id: harness.primary_id,
            message: object_request(),
            delivery: Delivery::Reliable,
            completed: first,
            dispatch_probe: None,
        })
        .await
        .expect("dispatch first send");
    drop(first_response);
    timeout(WAIT, harness.send_entered.notified()).await.expect("first send should stall in the local socket");

    let (second, second_response) = oneshot::channel();
    harness
        .commands
        .send(Command::SendTo {
            simulator_id: harness.primary_id,
            message: object_request(),
            delivery: Delivery::Reliable,
            completed: second,
            dispatch_probe: None,
        })
        .await
        .expect("queue second send");
    drop(second_response);

    harness.pause_sends.send_replace(false);
    for ack_sequence in 24..26 {
        let sent = receive_message(&harness.primary, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
        harness.primary.send_to(&ack(ack_sequence, sent.sequence()), harness.primary_peer).await.expect("ack abandoned send");
    }

    let (completed, response) = oneshot::channel();
    harness
        .commands
        .send(Command::SendTo {
            simulator_id: harness.primary_id,
            message: object_request(),
            delivery: Delivery::Reliable,
            completed,
            dispatch_probe: None,
        })
        .await
        .expect("dispatch send after abandoned completions");
    let sent = receive_message(&harness.primary, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
    timeout(WAIT, response).await.expect("later send completion should arrive").expect("send reply").expect("later send succeeds");
    harness.primary.send_to(&ack(26, sent.sequence()), harness.primary_peer).await.expect("ack later send");

    harness.shutdown().await;
}
