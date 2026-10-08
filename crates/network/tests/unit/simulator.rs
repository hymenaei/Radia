use std::{collections::HashSet, net::SocketAddr, time::Duration};

use crate::{
    AgentSession, Error, Grid, LoginResponseData,
    capabilities::{CapabilityBudget, Client, EventQueueExit},
    client::{COMMAND_CAPACITY, Command},
    event::{Event, EventEnvelope, PRIMARY_TERMINAL_EVENT_CAPACITY, SimulatorId},
    simulator::SimulatorConnection,
    status::NetworkStatusHandle,
};
use base::UUID;
use protocol::{
    messages::{
        CacheMissType, ObjectRequest, PacketAck, RequestAgentData, RequestMultipleObjects, StartPingCheck, packet_ack::Packets,
        start_ping_check::PingID as StartPingID,
    },
    udp::{Message, Packet},
};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, oneshot, watch},
    time::{advance, timeout},
};
use url::Url;

const AGENT_ID: &str = "ffeeddcc-bbaa-9988-7766-554433221100";
const SESSION_ID: &str = "00112233-4455-6677-8899-aabbccddeeff";
const CIRCUIT_CODE: u32 = 0x1122_3344;
const WAIT: Duration = Duration::from_secs(2);
const SESSION_EVENT_CAPACITY: usize = 2;
const TEST_EVENT_CAPACITY: usize = crate::event::CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY;

use super::{EVENT_BACKLOG_WARNING_THRESHOLD, MAX_QUEUED_SIMULATOR_EVENTS, PrimaryEventBacklog, primary_event_queue_size, run_session};
use crate::circuit::KEEPALIVE_TIMEOUT;

#[tokio::test(start_paused = true)]
async fn primary_udp_activity_resets_keepalive_timeout() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let endpoint = simulator.local_addr().expect("read simulator address");
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(TEST_EVENT_CAPACITY);
    let (_disconnect, disconnect_requested) = watch::channel(false);
    let client = tokio::spawn(run_session(login_session(endpoint), command_receiver, event_sender, disconnect_requested));

    let (_, peer) = receive(&simulator).await;
    simulator.send_to(&ack(20, 1), peer).await.expect("ack primary circuit");
    receive_message(&simulator, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    simulator.send_to(&ack(21, 2), peer).await.expect("ack initial movement");

    let mut handshake = fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 22);
    simulator.send_to(&handshake, peer).await.expect("send primary handshake");
    let reply = receive_message(&simulator, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    simulator.send_to(&ack(25, reply.sequence()), peer).await.expect("ack primary handshake reply");

    let mut movement_complete = fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    simulator.send_to(&movement_complete, peer).await.expect("send movement completion");
    loop {
        if matches!(next_raw_event(&mut events).await, Event::SessionReady { .. }) {
            break;
        }
    }

    advance(KEEPALIVE_TIMEOUT - Duration::from_secs(1)).await;
    simulator.send_to(&start_ping_packet(24, 9, 0), peer).await.expect("send valid simulator traffic");
    receive_message(&simulator, |message| matches!(message, Message::CompletePingCheck(ping) if ping.ping_id.ping_id == 9)).await;
    advance(KEEPALIVE_TIMEOUT - Duration::from_secs(1)).await;
    assert!(!client.is_finished(), "valid inbound UDP activity must renew the primary inactivity deadline");

    advance(Duration::from_secs(2)).await;
    let result = client.await.expect("primary session task");
    assert!(matches!(result, Err(reason) if reason == "The simulator stopped responding to UDP keepalive traffic."));
    drop(commands);
}

#[tokio::test(start_paused = true)]
async fn neighbor_circuit_fails_after_udp_inactivity() {
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let endpoint = neighbor.local_addr().expect("read neighbor address");
    let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
    let connection = super::NeighborConnection::connect(
        simulator_id,
        login_session(endpoint),
        0x1000_0000_0000_1000,
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect neighbor circuit");
    let (events, mut event_receiver) = mpsc::channel(4);
    let (_shutdown_signal, shutdown) = watch::channel(false);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(4);
    let (_command_sender, commands) = mpsc::channel(4);
    let worker = tokio::spawn(connection.run(events, shutdown, neighbor_requests, commands));

    let (use_circuit, peer) = receive(&neighbor).await;
    assert!(matches!(Packet::decode(&use_circuit).expect("decode UseCircuitCode").message(), Message::UseCircuitCode(_)));
    let acknowledgement = Packet::new(40, Message::PacketAck(PacketAck { packets: vec![Packets { id: 1 }] }))
        .encode()
        .expect("encode circuit acknowledgement");
    neighbor.send_to(&acknowledgement, peer).await.expect("ack neighbor circuit");
    assert!(matches!(next_neighbor_event(&mut event_receiver).await, Event::SimulatorConnected { region_handle: Some(_), .. }));

    advance(KEEPALIVE_TIMEOUT).await;
    let result = worker.await.expect("neighbor task");
    assert!(matches!(result, Err(reason) if reason == "The simulator stopped responding to UDP keepalive traffic."));
}

#[test]
fn backlog_pressure_warning_is_thresholded_and_rate_limited() {
    let start = tokio::time::Instant::now();
    let mut backlog = PrimaryEventBacklog::default();

    assert!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD - 1, start).is_none());
    assert_eq!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start + Duration::from_millis(3)), Some(3));
    assert!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start + Duration::from_secs(10)).is_none());
    assert_eq!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start + Duration::from_secs(10) + Duration::from_millis(3)), Some(10_003));
}

#[test]
fn backlog_pressure_duration_resets_after_queue_drains() {
    let start = tokio::time::Instant::now();
    let mut backlog = PrimaryEventBacklog::default();

    assert_eq!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start), Some(0));
    backlog.observe(0, start + Duration::from_secs(1));
    assert_eq!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start + Duration::from_secs(11)), Some(0));
}

#[tokio::test]
async fn primary_event_queue_size_excludes_reserved_terminal_slot() {
    let (events, _received) = mpsc::channel(4);
    let _terminal_slot = events.clone().reserve_owned().await.expect("reserve terminal event slot");
    events.try_send(EventEnvelope::new(Event::Failed("queued".into()))).expect("queue an application event");

    assert_eq!(
        primary_event_queue_size(&events, 3),
        (4, 3 + MAX_QUEUED_SIMULATOR_EVENTS),
        "occupancy excludes the terminal reservation and includes the pending deque"
    );
}

async fn terminal_after_full_queue(result: Result<(), String>) -> (Event, SimulatorId, SocketAddr) {
    let (events, mut received) = mpsc::channel(2);
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let endpoint = "127.0.0.1:9000".parse().expect("parse simulator endpoint");
    let terminal_slot = events.clone().try_reserve_owned().expect("reserve terminal event slot");
    events.try_send(EventEnvelope::new(Event::Failed("queued before disconnect".into()))).expect("fill the nonterminal slot");
    assert_eq!(events.capacity(), 0, "the ordinary event capacity is full");

    SimulatorConnection::publish_primary_terminal_event(terminal_slot, simulator_id, endpoint, &result);

    assert!(matches!(
        timeout(WAIT, received.recv()).await.expect("queued event should remain observable").map(EventEnvelope::into_event),
        Some(Event::Failed(message)) if message == "queued before disconnect"
    ));
    let terminal_event = timeout(WAIT, received.recv()).await.expect("terminal event should remain observable");
    (terminal_event.expect("terminal event remains observable").into_event(), simulator_id, endpoint)
}

#[tokio::test]
async fn primary_disconnect_survives_full_queue() {
    let (event, simulator_id, endpoint) = terminal_after_full_queue(Ok(())).await;
    assert!(matches!(
        event,
        Event::SimulatorDisconnected { simulator_id: actual_id, region_handle: None, endpoint: actual_endpoint }
            if actual_id == simulator_id && actual_endpoint == endpoint
    ));
}

#[tokio::test]
async fn primary_failure_survives_full_queue() {
    let (event, simulator_id, endpoint) = terminal_after_full_queue(Err("simulator failed".into())).await;
    assert!(matches!(
        event,
        Event::SimulatorFailed { simulator_id: actual_id, region_handle: None, endpoint: actual_endpoint, reason }
            if actual_id == simulator_id && actual_endpoint == endpoint && reason == "simulator failed"
    ));
}

#[tokio::test]
async fn full_event_queue_keeps_ping_and_shutdown_responsive() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let endpoint = simulator.local_addr().expect("read simulator address");
    let connection = SimulatorConnection::connect_with_status(
        login_session(endpoint),
        LoginResponseData::default(),
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect simulator circuit");
    let (_, peer) = receive(&simulator).await;

    let (events, mut received) = mpsc::channel(2);
    let terminal_event_permit = events.clone().try_reserve_owned().expect("reserve terminal event slot");
    events.try_send(EventEnvelope::new(Event::Failed("fill event queue".into()))).expect("fill event queue");
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, disconnect_requested) = tokio::sync::watch::channel(false);
    let run = tokio::spawn(connection.run(command_receiver, events, disconnect_requested, terminal_event_permit));

    let mut chat = fixture("chat_from_simulator.hex");
    chat[0] |= 0x40;
    set_sequence(&mut chat, 1);
    simulator.send_to(&chat, peer).await.expect("send reliable data while event queue is full");
    simulator.send_to(&start_ping_packet(2, 8, 0), peer).await.expect("send ping while event queue is full");
    let mut chat_acknowledged = false;
    let mut ping_answered = false;
    timeout(WAIT, async {
        while !(chat_acknowledged && ping_answered) {
            let (bytes, _) = receive(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode primary control reply");
            chat_acknowledged |= packet.appended_acks().contains(&1)
                || matches!(packet.message(), Message::PacketAck(ack) if ack.packets.iter().any(|item| item.id == 1));
            ping_answered |= matches!(packet.message(), Message::CompletePingCheck(ping) if ping.ping_id.ping_id == 8);
        }
    })
    .await
    .expect("primary must ACK reliable data and answer pings while application delivery is blocked");
    assert!(!run.is_finished(), "simulator should stay active until shutdown");
    disconnect.send_replace(true);
    receive_message(&simulator, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(timeout(WAIT, run).await.expect("shutdown should finish with a full event queue").expect("simulator task").is_ok());
    assert!(matches!(received.recv().await.map(EventEnvelope::into_event), Some(Event::Failed(message)) if message == "fill event queue"));
    assert!(matches!(
        received.recv().await.map(EventEnvelope::into_event),
        Some(Event::SimulatorDisconnected { region_handle: None, endpoint: actual_endpoint, .. }) if actual_endpoint == endpoint
    ));
    assert!(timeout(WAIT, received.recv()).await.expect("event stream should close").is_none());
}

#[tokio::test]
async fn capability_events_wait_for_simulator_connection() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let endpoint = simulator.local_addr().expect("read simulator address");
    let mut connection = SimulatorConnection::connect_with_status(
        login_session(endpoint),
        LoginResponseData::default(),
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect simulator circuit");
    let simulator_id = connection.simulator_id;
    let (events, mut received) = mpsc::channel(4);
    let mut pending_events = std::collections::VecDeque::new();
    let mut preconnect_events = super::PreconnectEvents::default();
    let mut connected_simulators = std::collections::HashSet::new();
    let mut neighbors = std::collections::HashMap::new();
    let mut capabilities = std::collections::HashMap::new();
    let mut drop_application_messages = false;
    let mut logout_deadline = None;
    let mut disconnect_reason = None;
    let queued_bytes = std::sync::Arc::new(tokio::sync::Semaphore::new(10));
    let mut first_event_permit = Some(queued_bytes.clone().acquire_many_owned(7).await.expect("reserve capability event bytes"));

    for (index, message) in ["First", "Second"].into_iter().enumerate() {
        let event = Event::CapabilityEvent { simulator_id, source: endpoint, message: message.to_owned(), body: llsd::Value::Undefined };
        let envelope = if index == 0 {
            EventEnvelope::with_capability_bytes(event, first_event_permit.take().expect("first event permit"))
        } else {
            EventEnvelope::new(event)
        };
        connection
            .publish_simulator_event(
                simulator_id,
                envelope,
                &events,
                &mut pending_events,
                &mut preconnect_events,
                &mut connected_simulators,
                &mut neighbors,
                &mut capabilities,
                &mut drop_application_messages,
                &mut logout_deadline,
                &mut disconnect_reason,
            )
            .await
            .expect("buffer pre-connect capability event");
    }
    assert_eq!(preconnect_events.len, 2);
    assert_eq!(queued_bytes.available_permits(), 3, "the preconnect buffer must retain capability bytes");
    assert!(received.try_recv().is_err(), "capability events must wait for SimulatorConnected");

    connection
        .publish_simulator_connected(
            simulator_id,
            Event::SimulatorConnected { simulator_id, region_handle: None, endpoint },
            &events,
            &mut pending_events,
            &mut preconnect_events,
            &mut connected_simulators,
            &mut drop_application_messages,
            &mut logout_deadline,
            &mut disconnect_reason,
        )
        .await
        .expect("publish connection before buffered events");
    assert_eq!(queued_bytes.available_permits(), 3, "publishing must transfer the permit with its event");

    assert!(
        matches!(received.recv().await.map(EventEnvelope::into_event), Some(Event::SimulatorConnected { simulator_id: id, .. }) if id == simulator_id)
    );
    assert_eq!(queued_bytes.available_permits(), 3);
    for expected in ["First", "Second"] {
        assert!(matches!(
            received.recv().await.map(EventEnvelope::into_event),
            Some(Event::CapabilityEvent { simulator_id: id, message, .. }) if id == simulator_id && message == expected
        ));
        if expected == "First" {
            assert_eq!(queued_bytes.available_permits(), 10, "recv_event handoff must release retained bytes");
        }
    }
    assert_eq!(preconnect_events.len, 0);
}

#[tokio::test]
async fn primary_intro_precedes_early_udp_data() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(TEST_EVENT_CAPACITY);
    let (disconnect, disconnect_requested) = tokio::sync::watch::channel(false);
    let client = tokio::spawn(run_session(
        login_session(simulator.local_addr().expect("primary simulator address")),
        command_receiver,
        event_sender,
        disconnect_requested,
    ));

    let (_, peer) = receive(&simulator).await;
    let mut chat = fixture("chat_from_simulator.hex");
    chat[0] |= 0x40;
    set_sequence(&mut chat, 19);
    simulator.send_to(&chat, peer).await.expect("send data before circuit acknowledgement");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(events.try_recv().is_err(), "primary application data must wait for the simulator introduction");

    simulator.send_to(&ack(20, 1), peer).await.expect("ack primary circuit");
    receive_message(&simulator, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    let first = next_raw_event(&mut events).await;
    let simulator_id = match first {
        Event::SimulatorConnected { simulator_id, region_handle: None, .. } => simulator_id,
        event => panic!("expected primary introduction before other events, got {event:?}"),
    };
    assert!(matches!(
        next_raw_event(&mut events).await,
        Event::SimulatorMessage { simulator_id: id, sequence: 19, message: Message::ChatFromSimulator(_), .. }
            if id == simulator_id
    ));

    disconnect.send_replace(true);
    receive_message(&simulator, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(timeout(Duration::from_millis(200), client).await.expect("disconnect should finish").expect("client task").is_ok());
    drop(commands);
}

#[test]
fn preconnect_buffer_is_bounded() {
    let simulator_id = SimulatorId::new(51);
    let mut pending = super::PreconnectEvents::default();
    for _ in 0..super::MAX_PRECONNECT_EVENTS {
        assert!(matches!(
            super::queue_until_connected(
                &std::collections::HashSet::new(),
                &mut pending,
                simulator_id,
                EventEnvelope::new(Event::Failed("pending".into()))
            ),
            super::PreconnectOutcome::Buffered
        ));
    }
    let overflow = super::queue_until_connected(
        &std::collections::HashSet::new(),
        &mut pending,
        simulator_id,
        EventEnvelope::new(Event::Failed("overflow".into())),
    );
    assert!(matches!(overflow, super::PreconnectOutcome::Full));
    assert_eq!(pending.len, super::MAX_PRECONNECT_EVENTS);
}

#[tokio::test]
async fn simulator_removal_releases_preconnect_bytes() {
    let simulator_id = SimulatorId::new(52);
    let budget = std::sync::Arc::new(tokio::sync::Semaphore::new(10));
    let permit = budget.clone().acquire_many_owned(7).await.expect("reserve event bytes");
    let mut pending = super::PreconnectEvents::default();
    assert!(pending.push(
        simulator_id,
        EventEnvelope::with_capability_bytes(
            Event::CapabilityEvent {
                simulator_id,
                source: "127.0.0.1:13001".parse().expect("simulator address"),
                message: "Pending".into(),
                body: llsd::Value::Undefined,
            },
            permit,
        ),
    ));
    assert_eq!(budget.available_permits(), 3);

    pending.remove(simulator_id);
    assert_eq!(pending.len, 0);
    assert_eq!(budget.available_permits(), 10);
}

#[tokio::test]
async fn replacing_unannounced_neighbor_releases_queued_state() {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let old_neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind old neighbor");
    let new_neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind replacement neighbor");
    let old_endpoint = old_neighbor.local_addr().expect("read old neighbor address");
    let new_endpoint = new_neighbor.local_addr().expect("read replacement address");
    let region_handle = 0x1000_0000_0000_1000;
    let mut connection = SimulatorConnection::connect_with_status(
        login_session(primary.local_addr().expect("primary simulator address")),
        LoginResponseData::default(),
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect primary circuit");
    let _ = receive(&primary).await;
    let (application_events, _application_event_receiver) = mpsc::channel(8);
    let (neighbor_events, _neighbor_event_receiver) = mpsc::channel::<super::NeighborEvent>(8);
    let (_shutdown_signal, shutdown) = tokio::sync::watch::channel(false);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(8);
    let mut neighbors = tokio::task::JoinSet::new();
    let mut connections = std::collections::HashMap::new();
    let mut retiring_simulators = HashSet::new();
    let mut neighbor_task_owners = std::collections::HashMap::new();
    let mut preconnect_events = super::PreconnectEvents::default();
    let mut simulator_capabilities = std::collections::HashMap::new();
    let mut pending_events = std::collections::VecDeque::new();
    let mut drop_application_messages = false;
    let mut logout_deadline = None;
    let mut disconnect_reason = None;

    connection
        .start_neighbor(
            connection.simulator_id,
            region_handle,
            old_endpoint,
            &mut neighbors,
            &mut connections,
            &mut retiring_simulators,
            &mut neighbor_task_owners,
            &mut preconnect_events,
            &mut simulator_capabilities,
            neighbor_requests.clone(),
            application_events.clone(),
            neighbor_events.clone(),
            shutdown.clone(),
            &mut pending_events,
            &mut drop_application_messages,
            &mut logout_deadline,
            &mut disconnect_reason,
        )
        .await
        .expect("start initial neighbor");
    let _ = receive(&old_neighbor).await;
    let old_id = connections.get(&region_handle).expect("old neighbor is tracked").simulator_id;

    let queued_bytes = std::sync::Arc::new(tokio::sync::Semaphore::new(10));
    let retained = queued_bytes.clone().acquire_many_owned(7).await.expect("reserve queued capability bytes");
    assert!(preconnect_events.push(
        old_id,
        EventEnvelope::with_capability_bytes(
            Event::CapabilityEvent {
                simulator_id: old_id,
                source: old_endpoint,
                message: "EventQueueGet".into(),
                body: llsd::Value::Undefined,
            },
            retained,
        ),
    ));
    simulator_capabilities.insert(
        old_endpoint,
        super::SimulatorCapabilitySession {
            simulator_id: old_id,
            client: Client::new_with_status_and_budget(
                Url::parse("http://127.0.0.1:1/seed").expect("seed URL"),
                None,
                NetworkStatusHandle::default(),
                CapabilityBudget::new(),
            )
            .expect("create old capability client"),
        },
    );
    assert_eq!(queued_bytes.available_permits(), 3);

    connection
        .start_neighbor(
            connection.simulator_id,
            region_handle,
            new_endpoint,
            &mut neighbors,
            &mut connections,
            &mut retiring_simulators,
            &mut neighbor_task_owners,
            &mut preconnect_events,
            &mut simulator_capabilities,
            neighbor_requests,
            application_events,
            neighbor_events,
            shutdown,
            &mut pending_events,
            &mut drop_application_messages,
            &mut logout_deadline,
            &mut disconnect_reason,
        )
        .await
        .expect("replace unannounced neighbor");
    let _ = receive(&new_neighbor).await;

    assert_ne!(connections.get(&region_handle).expect("replacement remains tracked").simulator_id, old_id);
    assert!(retiring_simulators.contains(&old_id));
    assert_eq!(preconnect_events.len, 0, "replacement drops old pre-connect events immediately");
    assert_eq!(queued_bytes.available_permits(), 10, "replacement releases retained event bytes immediately");
    assert!(!simulator_capabilities.contains_key(&old_endpoint), "replacement stops the old capability session immediately");

    neighbors.abort_all();
    while neighbors.join_next().await.is_some() {}
}

#[tokio::test]
async fn preconnect_overflow_fails_only_the_unannounced_neighbor() {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let primary_endpoint = primary.local_addr().expect("read primary address");
    let mut connection = SimulatorConnection::connect_with_status(
        login_session(primary_endpoint),
        LoginResponseData::default(),
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect primary circuit");
    let simulator_id = SimulatorId::new(53);
    let neighbor_endpoint = "127.0.0.1:13003".parse().expect("parse neighbor endpoint");
    let region_handle = 0x1000_0000_0000_1003;
    let neighbor_task = tokio::spawn(std::future::pending::<()>());
    let (neighbor_commands, _neighbor_command_receiver) = mpsc::channel(1);
    let mut neighbors = std::collections::HashMap::from([(
        region_handle,
        super::NeighborConnectionState {
            simulator_id,
            endpoint: neighbor_endpoint,
            abort: neighbor_task.abort_handle(),
            commands: neighbor_commands,
        },
    )]);
    let mut preconnect_events = super::PreconnectEvents::default();
    for _ in 0..super::MAX_PRECONNECT_EVENTS {
        assert!(preconnect_events.push(simulator_id, EventEnvelope::new(Event::Failed("buffered".into()))));
    }
    let (events, mut received) = mpsc::channel(1);
    let mut pending_events = std::collections::VecDeque::new();
    let mut connected_simulators = std::collections::HashSet::new();
    let mut capabilities = std::collections::HashMap::new();
    let mut drop_application_messages = false;
    let mut logout_deadline = None;
    let mut disconnect_reason = None;

    connection
        .fail_preconnect_overflow(
            simulator_id,
            &events,
            &mut pending_events,
            &mut preconnect_events,
            &mut connected_simulators,
            &mut neighbors,
            &mut capabilities,
            &mut drop_application_messages,
            &mut logout_deadline,
            &mut disconnect_reason,
        )
        .await
        .expect("neighbor overflow should not log out the primary simulator");

    assert!(neighbors.is_empty(), "overflow must remove the affected neighbor");
    assert!(preconnect_events.by_simulator.is_empty());
    assert_eq!(preconnect_events.len, 0);
    assert!(!connected_simulators.contains(&simulator_id));
    assert!(neighbor_task.await.expect_err("overflow should cancel the neighbor worker").is_cancelled());
    assert!(matches!(
        received.recv().await.map(EventEnvelope::into_event),
        Some(Event::SimulatorFailed { simulator_id: failed_id, region_handle: Some(handle), endpoint, .. })
            if failed_id == simulator_id && handle == region_handle && endpoint == neighbor_endpoint
    ));
}

#[tokio::test]
async fn neighbor_circuits_are_independent() {
    let mut harness = Box::pin(connected_neighbor()).await;
    let neighbor_peer = harness.neighbor_peer;
    let neighbor_id = harness.neighbor_id;
    assert_ne!(harness.primary_id, neighbor_id);

    let mut neighbor_handshake = fixture("region_handshake.hex");
    neighbor_handshake[0] |= 0x40;
    set_sequence(&mut neighbor_handshake, 41);
    harness.neighbor.send_to(&neighbor_handshake, neighbor_peer).await.expect("send neighbor handshake");
    let reply = receive_message(&harness.neighbor, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    assert!(reply.appended_acks().contains(&41));
    assert!(matches!(
        next_event(&mut harness.events).await,
        Event::SimulatorMessage { simulator_id, source, sequence: 41, message: Message::RegionHandshake(_) }
            if simulator_id == neighbor_id && source == harness.neighbor.local_addr().expect("neighbor address")
    ));

    let mut disable = fixture("disable_simulator.hex");
    set_sequence(&mut disable, 42);
    harness.neighbor.send_to(&disable, neighbor_peer).await.expect("disable neighbor circuit");
    loop {
        if matches!(next_event(&mut harness.events).await, Event::SimulatorDisconnected { simulator_id, region_handle: Some(0x1000_0000_0000_1000), .. } if simulator_id == neighbor_id)
        {
            break;
        }
    }

    let mut chat = fixture("chat_from_simulator.hex");
    chat[0] |= 0x40;
    set_sequence(&mut chat, 43);
    harness.neighbor.send_to(&chat, neighbor_peer).await.expect("send after neighbor disabled");
    let mut buffer = [0; 65_535];
    match timeout(Duration::from_millis(50), harness.neighbor.recv_from(&mut buffer)).await {
        Err(_) | Ok(Err(_)) => {}
        Ok(Ok((size, _))) => {
            panic!("disabled circuit sent another packet: {:?}", Packet::decode(&buffer[..size]).expect("decode packet").message())
        }
    }

    harness.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn neighbor_delivers_accepted_data_before_terminal() {
    let mut harness = Box::pin(connected_neighbor()).await;

    let mut chat = fixture("chat_from_simulator.hex");
    chat[0] |= 0x40;
    set_sequence(&mut chat, 41);
    harness.neighbor.send_to(&chat, harness.neighbor_peer).await.expect("send final neighbor message");

    let mut disable = fixture("disable_simulator.hex");
    set_sequence(&mut disable, 42);
    harness.neighbor.send_to(&disable, harness.neighbor_peer).await.expect("close neighbor circuit");

    assert!(matches!(
        next_event(&mut harness.events).await,
        Event::SimulatorMessage { simulator_id, sequence: 41, message: Message::ChatFromSimulator(_), .. }
            if simulator_id == harness.neighbor_id
    ));
    assert!(matches!(
        next_event(&mut harness.events).await,
        Event::SimulatorDisconnected { simulator_id, region_handle: Some(0x1000_0000_0000_1000), .. }
            if simulator_id == harness.neighbor_id
    ));

    harness.shutdown().await;
}

#[tokio::test]
async fn object_local_ids_are_scoped_to_simulators() {
    let mut harness = Box::pin(connected_neighbor()).await;

    let mut primary_update = fixture("object_update.hex");
    set_sequence(&mut primary_update, 44);
    harness.primary.send_to(&primary_update, harness.primary_peer).await.expect("send primary object update");

    let mut neighbor_update = fixture("object_update.hex");
    set_sequence(&mut neighbor_update, 45);
    harness.neighbor.send_to(&neighbor_update, harness.neighbor_peer).await.expect("send neighbor object update");

    let mut received_simulators = Vec::with_capacity(2);
    timeout(WAIT, async {
        while received_simulators.len() < 2 {
            if let Event::SimulatorMessage { simulator_id, message: Message::ObjectUpdate(update), .. } =
                next_event(&mut harness.events).await
            {
                assert_eq!(update.objects[0].local_id, 0x1020_3040);
                received_simulators.push(simulator_id);
            }
        }
    })
    .await
    .expect("receive the object update from both circuits");

    assert!(received_simulators.contains(&harness.primary_id));
    assert!(received_simulators.contains(&harness.neighbor_id));
    assert_ne!(harness.primary_id, harness.neighbor_id);
    harness.shutdown().await;
}

#[tokio::test]
async fn neighbor_targets_and_replacements() {
    let mut harness = Box::pin(connected_neighbor()).await;
    receive_ack(&harness.primary, 23).await;
    send_targeted(&harness.commands, harness.neighbor_id, object_request())
        .await
        .expect("route an object request to the selected simulator");
    let (_, sent_peer) =
        receive_message_with_peer(&harness.neighbor, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
    assert_eq!(sent_peer, harness.neighbor_peer);
    let mut buffer = [0; 65_535];
    assert!(timeout(Duration::from_millis(50), harness.primary.recv_from(&mut buffer)).await.is_err());

    let mut disable = fixture("disable_simulator.hex");
    set_sequence(&mut disable, 42);
    harness.neighbor.send_to(&disable, harness.neighbor_peer).await.expect("disable neighbor circuit");
    loop {
        if matches!(next_event(&mut harness.events).await, Event::SimulatorDisconnected { simulator_id, .. } if simulator_id == harness.neighbor_id)
        {
            break;
        }
    }
    assert!(matches!(send_targeted(&harness.commands, harness.neighbor_id, object_request()).await, Err(Error::SimulatorUnavailable)));

    let neighbor_endpoint = harness.neighbor.local_addr().expect("neighbor address");
    announce_neighbor(&harness.primary, harness.primary_peer, neighbor_endpoint, 0x1000_0000_0000_1000, 50).await;
    let (bytes, replacement_peer) = receive(&harness.neighbor).await;
    assert!(matches!(Packet::decode(&bytes).expect("decode replacement circuit packet").message(), Message::UseCircuitCode(_)));
    harness.neighbor.send_to(&ack(60, 1), replacement_peer).await.expect("ack replacement circuit");
    let replacement_id = loop {
        match next_event(&mut harness.events).await {
            Event::SimulatorConnected { simulator_id, region_handle: Some(0x1000_0000_0000_1000), endpoint }
                if endpoint == neighbor_endpoint =>
            {
                break simulator_id;
            }
            _ => {}
        }
    };
    assert_ne!(replacement_id, harness.neighbor_id);
    assert!(matches!(send_targeted(&harness.commands, harness.neighbor_id, object_request()).await, Err(Error::SimulatorUnavailable)));
    send_targeted(&harness.commands, replacement_id, object_request()).await.expect("send to replacement");
    let (_, sent_peer) =
        receive_message_with_peer(&harness.neighbor, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
    assert_eq!(sent_peer, replacement_peer);
    harness.shutdown().await;
}

#[tokio::test]
async fn old_neighbor_completion_keeps_replacement_route() {
    let mut harness = Box::pin(connected_neighbor()).await;
    let replacement = UdpSocket::bind("127.0.0.1:0").await.expect("bind replacement neighbor");
    let replacement_endpoint = replacement.local_addr().expect("replacement address");
    let region_handle = 0x1000_0000_0000_1000;

    announce_neighbor(&harness.primary, harness.primary_peer, replacement_endpoint, region_handle, 50).await;
    let (bytes, replacement_peer) = receive(&replacement).await;
    assert!(matches!(Packet::decode(&bytes).expect("decode replacement circuit packet").message(), Message::UseCircuitCode(_)));
    replacement.send_to(&ack(60, 1), replacement_peer).await.expect("ack replacement circuit");

    let mut replacement_id = None;
    let mut old_disconnected = false;
    timeout(WAIT, async {
        while replacement_id.is_none() || !old_disconnected {
            match next_event(&mut harness.events).await {
                Event::SimulatorConnected { simulator_id, region_handle: Some(handle), endpoint }
                    if handle == region_handle && endpoint == replacement_endpoint =>
                {
                    replacement_id = Some(simulator_id);
                }
                Event::SimulatorDisconnected { simulator_id, region_handle: Some(handle), .. }
                    if simulator_id == harness.neighbor_id && handle == region_handle =>
                {
                    old_disconnected = true;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("observe old completion and replacement introduction");

    let replacement_id = replacement_id.expect("replacement connection ID");
    assert_ne!(replacement_id, harness.neighbor_id);
    send_targeted(&harness.commands, replacement_id, object_request()).await.expect("old completion must preserve the replacement route");
    let (_, sent_peer) = receive_message_with_peer(&replacement, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
    assert_eq!(sent_peer, replacement_peer);

    harness.shutdown().await;
}

#[tokio::test]
async fn neighbor_endpoint_validation() {
    let mut harness = Box::pin(connected_neighbor()).await;
    let primary_endpoint = harness.primary.local_addr().expect("primary simulator address");
    let neighbor_endpoint = harness.neighbor.local_addr().expect("neighbor simulator address");
    let candidates = [
        (0x2000_0000_0000_0000, primary_endpoint),
        (0x3000_0000_0000_0000, neighbor_endpoint),
        (0x4000_0000_0000_0000, SocketAddr::from(([127, 0, 0, 1], 0))),
        (0x5000_0000_0000_0000, SocketAddr::from(([0, 0, 0, 0], 13000))),
        (0x6000_0000_0000_0000, SocketAddr::from(([224, 0, 0, 1], 13000))),
        (0x7000_0000_0000_0000, SocketAddr::from(([255, 255, 255, 255], 13000))),
    ];

    for (index, (region_handle, endpoint)) in candidates.into_iter().enumerate() {
        announce_neighbor(&harness.primary, harness.primary_peer, endpoint, region_handle, 50 + index as u32).await;
        let failed_endpoint = timeout(WAIT, async {
            loop {
                if let Event::SimulatorConnectFailed { region_handle: failed_region, endpoint, .. } = next_event(&mut harness.events).await
                    && failed_region == region_handle
                {
                    break endpoint;
                }
            }
        })
        .await
        .expect("invalid or duplicate endpoint should fail before starting a connection");
        assert_eq!(failed_endpoint, endpoint);
    }

    harness.shutdown().await;
}

#[tokio::test]
async fn duplicate_replacement_keeps_old_neighbor() {
    let mut harness = Box::pin(connected_neighbor()).await;
    let other = UdpSocket::bind("127.0.0.1:0").await.expect("bind other neighbor");
    let other_endpoint = other.local_addr().expect("read other neighbor address");
    let other_region = 0x2000_0000_0000_2000;
    announce_neighbor(&harness.primary, harness.primary_peer, other_endpoint, other_region, 50).await;
    let (bytes, other_peer) = receive(&other).await;
    assert!(matches!(Packet::decode(&bytes).expect("decode other neighbor bootstrap").message(), Message::UseCircuitCode(_)));
    other.send_to(&ack(60, 1), other_peer).await.expect("ack other neighbor bootstrap");
    loop {
        if matches!(
            next_event(&mut harness.events).await,
            Event::SimulatorConnected { region_handle: Some(handle), endpoint, .. }
                if handle == other_region && endpoint == other_endpoint
        ) {
            break;
        }
    }

    let old_id = harness.neighbor_id;
    let old_region = 0x1000_0000_0000_1000;
    announce_neighbor(&harness.primary, harness.primary_peer, other_endpoint, old_region, 51).await;
    timeout(WAIT, async {
        loop {
            if matches!(
                next_event(&mut harness.events).await,
                Event::SimulatorConnectFailed { region_handle, endpoint, .. }
                    if region_handle == old_region && endpoint == other_endpoint
            ) {
                break;
            }
        }
    })
    .await
    .expect("duplicate endpoint replacement should be rejected");

    send_targeted(&harness.commands, old_id, object_request()).await.expect("the existing route should remain active");
    let (_, sent_peer) =
        receive_message_with_peer(&harness.neighbor, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
    assert_eq!(sent_peer, harness.neighbor_peer);
    harness.shutdown().await;
}

#[tokio::test]
async fn enable_announcements_over_limit_do_not_stall_primary() {
    let mut harness = Box::pin(connected_neighbor()).await;
    let mut sockets = Vec::with_capacity(super::MAX_NEIGHBOR_SIMULATORS);
    for _ in 0..super::MAX_NEIGHBOR_SIMULATORS {
        sockets.push(UdpSocket::bind("127.0.0.1:0").await.expect("bind announced neighbor"));
    }
    let addresses = sockets.iter().map(|socket| socket.local_addr().expect("read neighbor address")).collect::<Vec<_>>();
    let regions = (0..super::MAX_NEIGHBOR_SIMULATORS).map(|index| 0x3000_0000_0000_0000 + index as u64).collect::<Vec<_>>();
    let first_sequence = 60;
    let primary = &harness.primary;
    let primary_peer = harness.primary_peer;
    let primary_id = harness.primary_id;
    let events = &mut harness.events;

    let send_announcements = async {
        for index in 0..addresses.len() {
            announce_neighbor(primary, primary_peer, addresses[index], regions[index], first_sequence + index as u32).await;
        }
    };
    let observe_announcements = async {
        let mut seen = vec![false; addresses.len()];
        let mut rejected_endpoint = None;
        timeout(WAIT, async {
            while seen.iter().any(|received| !received) || rejected_endpoint.is_none() {
                match next_event(events).await {
                    Event::SimulatorMessage { simulator_id, sequence, message: Message::EnableSimulator(_), .. }
                        if simulator_id == primary_id && (first_sequence..first_sequence + addresses.len() as u32).contains(&sequence) =>
                    {
                        seen[(sequence - first_sequence) as usize] = true;
                    }
                    Event::SimulatorConnectFailed { region_handle, endpoint, .. }
                        if region_handle == *regions.last().expect("one announcement") =>
                    {
                        rejected_endpoint = Some(endpoint);
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("primary should process all neighbor announcements");
        (seen, rejected_endpoint)
    };

    let (_, (seen, rejected_endpoint)) = tokio::join!(send_announcements, observe_announcements);
    assert!(seen.into_iter().all(|received| received));
    assert_eq!(rejected_endpoint, addresses.last().copied());

    for socket in &sockets[..super::MAX_NEIGHBOR_SIMULATORS - 1] {
        let (bytes, _) = receive(socket).await;
        assert!(matches!(Packet::decode(&bytes).expect("decode admitted neighbor bootstrap").message(), Message::UseCircuitCode(_)));
    }
    let rejected = sockets.last().expect("the 33rd neighbor announcement");
    let mut buffer = [0; 65_535];
    assert!(timeout(Duration::from_millis(50), rejected.recv_from(&mut buffer)).await.is_err());

    harness.primary.send_to(&start_ping_packet(150, 7, 0), harness.primary_peer).await.expect("ping primary after announcement burst");
    assert!(matches!(
        receive_message(&harness.primary, |message| matches!(message, Message::CompletePingCheck(_))).await.message(),
        Message::CompletePingCheck(message) if message.ping_id.ping_id == 7
    ));
    harness.shutdown().await;
}

#[tokio::test]
async fn neighbor_shutdown_preempts_start() {
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
    let (event_sender, _event_receiver) = mpsc::channel(1);
    let (shutdown_signal, shutdown) = tokio::sync::watch::channel(false);
    shutdown_signal.send_replace(true);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(1);
    let (_command_sender, commands) = mpsc::channel(1);
    let neighbor_connection = super::NeighborConnection::connect(
        simulator_id,
        login_session(endpoint),
        0x1000_0000_0000_1000,
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect neighbor circuit");

    let result = timeout(Duration::from_millis(100), neighbor_connection.run(event_sender, shutdown, neighbor_requests, commands)).await;
    assert!(matches!(result, Ok(Ok(()))), "a pre-signaled shutdown should stop the neighbor immediately");
}

#[tokio::test]
async fn neighbor_receiver_close_preempts() {
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
    let (event_sender, mut event_receiver) = mpsc::channel(1);
    let (_shutdown_signal, shutdown) = tokio::sync::watch::channel(false);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(1);
    let (_command_sender, commands) = mpsc::channel(1);
    let neighbor_connection = super::NeighborConnection::connect(
        simulator_id,
        login_session(endpoint),
        0x1000_0000_0000_1000,
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect neighbor circuit");
    let task = tokio::spawn(neighbor_connection.run(event_sender, shutdown, neighbor_requests, commands));

    let (use_circuit, peer) = receive(&neighbor).await;
    assert!(matches!(Packet::decode(&use_circuit).expect("decode neighbor circuit packet").message(), Message::UseCircuitCode(_)));
    neighbor.send_to(&ack(40, 1), peer).await.expect("ack neighbor circuit");
    assert!(matches!(next_neighbor_event(&mut event_receiver).await, Event::SimulatorConnected { .. }));

    drop(event_receiver);
    assert!(
        timeout(Duration::from_millis(200), task)
            .await
            .expect("neighbor should stop after event receiver closes")
            .expect("neighbor task")
            .is_ok()
    );
}

#[tokio::test]
async fn neighbor_ping_is_delayed() {
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
    let (event_sender, mut event_receiver) = mpsc::channel(1);
    let (shutdown_signal, shutdown) = tokio::sync::watch::channel(false);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(1);
    let (_command_sender, commands) = mpsc::channel(1);
    let neighbor_connection = super::NeighborConnection::connect(
        simulator_id,
        login_session(endpoint),
        0x1000_0000_0000_1000,
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect neighbor circuit");
    let task = tokio::spawn(neighbor_connection.run(event_sender, shutdown, neighbor_requests, commands));

    let (use_circuit, peer) = receive(&neighbor).await;
    assert!(matches!(Packet::decode(&use_circuit).expect("decode neighbor circuit packet").message(), Message::UseCircuitCode(_)));
    neighbor.send_to(&ack(40, 1), peer).await.expect("ack neighbor circuit");
    assert!(matches!(next_neighbor_event(&mut event_receiver).await, Event::SimulatorConnected { .. }));

    let mut buffer = [0; 65_535];
    assert!(
        timeout(Duration::from_millis(100), neighbor.recv_from(&mut buffer)).await.is_err(),
        "the neighbor should wait for its ping interval after connecting"
    );

    shutdown_signal.send_replace(true);
    assert!(timeout(Duration::from_millis(200), task).await.expect("neighbor should stop on shutdown").expect("neighbor task").is_ok());
}

#[tokio::test]
async fn neighbor_intro_precedes_early_udp_data() {
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
    let (event_sender, mut event_receiver) = mpsc::channel(4);
    let (shutdown_signal, shutdown) = tokio::sync::watch::channel(false);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(1);
    let (_command_sender, commands) = mpsc::channel(1);
    let connection = super::NeighborConnection::connect(
        simulator_id,
        login_session(endpoint),
        0x1000_0000_0000_1000,
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect neighbor circuit");
    let task = tokio::spawn(connection.run(event_sender, shutdown, neighbor_requests, commands));

    let (use_circuit, peer) = receive(&neighbor).await;
    assert!(matches!(Packet::decode(&use_circuit).expect("decode circuit packet").message(), Message::UseCircuitCode(_)));

    let mut chat = fixture("chat_from_simulator.hex");
    chat[0] |= 0x40;
    set_sequence(&mut chat, 41);
    neighbor.send_to(&chat, peer).await.expect("send data before circuit acknowledgement");
    assert!(
        timeout(Duration::from_millis(50), event_receiver.recv()).await.is_err(),
        "application data must wait for the simulator introduction"
    );

    neighbor.send_to(&ack(40, 1), peer).await.expect("ack neighbor circuit");
    assert!(matches!(
        next_neighbor_event(&mut event_receiver).await,
        Event::SimulatorConnected { simulator_id: id, .. } if id == simulator_id
    ));
    assert!(matches!(
        next_neighbor_event(&mut event_receiver).await,
        Event::SimulatorMessage { simulator_id: id, sequence: 41, message: Message::ChatFromSimulator(_), .. } if id == simulator_id
    ));

    shutdown_signal.send_replace(true);
    assert!(timeout(Duration::from_millis(200), task).await.expect("neighbor stops").expect("neighbor task").is_ok());
}

#[tokio::test]
async fn neighbor_task_limit_preserves_current() {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let (application_events, mut received_events) = mpsc::channel(super::MAX_NEIGHBOR_CONNECTION_TASKS + 1);
    let (neighbor_events, _neighbor_event_receiver) = mpsc::channel::<super::NeighborEvent>(super::MAX_NEIGHBOR_CONNECTION_TASKS + 1);
    let (_shutdown_signal, shutdown) = tokio::sync::watch::channel(false);
    let mut connection = SimulatorConnection::connect_with_status(
        login_session(primary.local_addr().expect("primary simulator address")),
        LoginResponseData::default(),
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect primary circuit");
    let _ = receive(&primary).await;

    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(32);
    let mut neighbors = tokio::task::JoinSet::new();
    let mut connections = std::collections::HashMap::new();
    let mut retiring_simulators = HashSet::new();
    let mut neighbor_task_owners = std::collections::HashMap::new();
    let mut preconnect_events = super::PreconnectEvents::default();
    let mut simulator_capabilities = std::collections::HashMap::new();
    let mut pending_events = std::collections::VecDeque::new();
    let mut drop_application_messages = false;
    let mut logout_deadline = None;
    let mut disconnect_reason = None;
    let target_region = 0x1000_0000_0000_1000;
    let mut sockets = Vec::with_capacity(super::MAX_NEIGHBOR_CONNECTION_TASKS + 1);

    for index in 0..super::MAX_NEIGHBOR_SIMULATORS {
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
        let region_handle = if index == 0 { target_region } else { 0x2000_0000_0000_0000 + index as u64 };
        connection
            .start_neighbor(
                connection.simulator_id,
                region_handle,
                socket.local_addr().expect("neighbor address"),
                &mut neighbors,
                &mut connections,
                &mut retiring_simulators,
                &mut neighbor_task_owners,
                &mut preconnect_events,
                &mut simulator_capabilities,
                neighbor_requests.clone(),
                application_events.clone(),
                neighbor_events.clone(),
                shutdown.clone(),
                &mut pending_events,
                &mut drop_application_messages,
                &mut logout_deadline,
                &mut disconnect_reason,
            )
            .await
            .expect("start a neighbor circuit");
        let (bytes, _) = receive(&socket).await;
        assert!(matches!(Packet::decode(&bytes).expect("decode neighbor bootstrap").message(), Message::UseCircuitCode(_)));
        sockets.push(socket);
    }

    assert_eq!(connections.len(), super::MAX_NEIGHBOR_SIMULATORS);
    assert_eq!(neighbors.len(), super::MAX_NEIGHBOR_SIMULATORS);
    let mut current_peer = None;
    for _ in 0..super::MAX_NEIGHBOR_SIMULATORS {
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind replacement simulator");
        let endpoint = socket.local_addr().expect("replacement address");
        connection
            .start_neighbor(
                connection.simulator_id,
                target_region,
                endpoint,
                &mut neighbors,
                &mut connections,
                &mut retiring_simulators,
                &mut neighbor_task_owners,
                &mut preconnect_events,
                &mut simulator_capabilities,
                neighbor_requests.clone(),
                application_events.clone(),
                neighbor_events.clone(),
                shutdown.clone(),
                &mut pending_events,
                &mut drop_application_messages,
                &mut logout_deadline,
                &mut disconnect_reason,
            )
            .await
            .expect("replace target neighbor");
        let (bytes, peer) = receive(&socket).await;
        assert!(matches!(Packet::decode(&bytes).expect("decode replacement bootstrap").message(), Message::UseCircuitCode(_)));
        current_peer = Some(peer);
        sockets.push(socket);
    }

    assert_eq!(connections.len(), super::MAX_NEIGHBOR_SIMULATORS);
    assert_eq!(neighbors.len(), super::MAX_NEIGHBOR_CONNECTION_TASKS);
    let current_id = connections.get(&target_region).expect("current target connection").simulator_id;
    let rejected = UdpSocket::bind("127.0.0.1:0").await.expect("bind rejected simulator");
    let rejected_endpoint = rejected.local_addr().expect("rejected endpoint");
    connection
        .start_neighbor(
            connection.simulator_id,
            target_region,
            rejected_endpoint,
            &mut neighbors,
            &mut connections,
            &mut retiring_simulators,
            &mut neighbor_task_owners,
            &mut preconnect_events,
            &mut simulator_capabilities,
            neighbor_requests.clone(),
            application_events.clone(),
            neighbor_events.clone(),
            shutdown.clone(),
            &mut pending_events,
            &mut drop_application_messages,
            &mut logout_deadline,
            &mut disconnect_reason,
        )
        .await
        .expect("reject a replacement at the task cap");

    assert_eq!(connections.get(&target_region).expect("current target remains").simulator_id, current_id);
    assert_eq!(neighbors.len(), super::MAX_NEIGHBOR_CONNECTION_TASKS);
    let failure = loop {
        if let Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint } =
            received_events.recv().await.expect("event channel remains open").into_event()
            && region_handle == target_region
            && endpoint == rejected_endpoint
        {
            break simulator_id;
        }
    };
    assert_ne!(failure, current_id);

    let current_socket = sockets.last().expect("last admitted target socket");
    current_socket
        .send_to(&start_ping_packet(200, 7, 0), current_peer.expect("target socket peer"))
        .await
        .expect("ping current target connection");
    assert!(matches!(
        receive_message(current_socket, |message| matches!(message, Message::CompletePingCheck(_))).await.message(),
        Message::CompletePingCheck(message) if message.ping_id.ping_id == 7
    ));
    let mut buffer = [0; 65_535];
    assert!(timeout(Duration::from_millis(50), rejected.recv_from(&mut buffer)).await.is_err());

    for _ in 0..super::MAX_NEIGHBOR_SIMULATORS {
        let Some(Err(error)) = neighbors.join_next().await else {
            panic!("replaced neighbor task should be joined as canceled");
        };
        assert!(error.is_cancelled());
    }
    assert_eq!(neighbors.len(), super::MAX_NEIGHBOR_SIMULATORS);

    let recovered = UdpSocket::bind("127.0.0.1:0").await.expect("bind recovered simulator");
    let recovered_endpoint = recovered.local_addr().expect("recovered endpoint");
    connection
        .start_neighbor(
            connection.simulator_id,
            target_region,
            recovered_endpoint,
            &mut neighbors,
            &mut connections,
            &mut retiring_simulators,
            &mut neighbor_task_owners,
            &mut preconnect_events,
            &mut simulator_capabilities,
            neighbor_requests,
            application_events,
            neighbor_events,
            shutdown,
            &mut pending_events,
            &mut drop_application_messages,
            &mut logout_deadline,
            &mut disconnect_reason,
        )
        .await
        .expect("task capacity should recover after canceled workers are joined");
    let (bytes, _) = receive(&recovered).await;
    assert!(matches!(Packet::decode(&bytes).expect("decode recovered neighbor").message(), Message::UseCircuitCode(_)));
    assert_eq!(connections.len(), super::MAX_NEIGHBOR_SIMULATORS);
    assert_eq!(neighbors.len(), super::MAX_NEIGHBOR_SIMULATORS + 1);
}

#[tokio::test]
async fn neighbor_simulator_limit_is_bounded() {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let (application_events, mut received_events) = mpsc::channel(super::MAX_NEIGHBOR_SIMULATORS + 1);
    let (neighbor_events, _neighbor_event_receiver) = mpsc::channel::<super::NeighborEvent>(1);
    let (_shutdown_signal, shutdown) = tokio::sync::watch::channel(false);
    let mut connection = SimulatorConnection::connect_with_status(
        login_session(primary.local_addr().expect("primary simulator address")),
        LoginResponseData::default(),
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect primary circuit");
    let _ = receive(&primary).await;

    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(32);
    let mut neighbors = tokio::task::JoinSet::new();
    let mut connections = std::collections::HashMap::new();
    let mut retiring_simulators = HashSet::new();
    let mut neighbor_task_owners = std::collections::HashMap::new();
    let mut preconnect_events = super::PreconnectEvents::default();
    let mut simulator_capabilities = std::collections::HashMap::new();
    let mut pending_events = std::collections::VecDeque::new();
    let mut drop_application_messages = false;
    let mut logout_deadline = None;
    let mut disconnect_reason = None;
    let mut sockets = Vec::with_capacity(super::MAX_NEIGHBOR_SIMULATORS + 1);

    for index in 0..=super::MAX_NEIGHBOR_SIMULATORS {
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
        let endpoint = socket.local_addr().expect("neighbor address");
        let region_handle = 0x1000_0000_0000_0000 + index as u64;
        connection
            .start_neighbor(
                connection.simulator_id,
                region_handle,
                endpoint,
                &mut neighbors,
                &mut connections,
                &mut retiring_simulators,
                &mut neighbor_task_owners,
                &mut preconnect_events,
                &mut simulator_capabilities,
                neighbor_requests.clone(),
                application_events.clone(),
                neighbor_events.clone(),
                shutdown.clone(),
                &mut pending_events,
                &mut drop_application_messages,
                &mut logout_deadline,
                &mut disconnect_reason,
            )
            .await
            .expect("process neighbor announcement");

        if index < super::MAX_NEIGHBOR_SIMULATORS {
            let (bytes, _) = receive(&socket).await;
            assert!(matches!(Packet::decode(&bytes).expect("decode neighbor bootstrap").message(), Message::UseCircuitCode(_)));
            sockets.push(socket);
        } else {
            assert_eq!(connections.len(), super::MAX_NEIGHBOR_SIMULATORS);
            assert_eq!(neighbors.len(), super::MAX_NEIGHBOR_SIMULATORS);
            let rejected_endpoint = endpoint;
            let failure = timeout(WAIT, async {
                loop {
                    if let Event::SimulatorConnectFailed { simulator_id, region_handle: failed_region, endpoint } =
                        received_events.recv().await.expect("event channel remains open").into_event()
                        && failed_region == region_handle
                    {
                        break (simulator_id, endpoint);
                    }
                }
            })
            .await
            .expect("the 33rd unique simulator should be rejected");
            assert_eq!(failure.1, rejected_endpoint);
            assert!(!connections.contains_key(&region_handle));
            assert_eq!(connections.len(), super::MAX_NEIGHBOR_SIMULATORS);
            assert_eq!(neighbors.len(), super::MAX_NEIGHBOR_SIMULATORS);
            sockets.push(socket);
        }
    }

    neighbors.abort_all();
    while neighbors.join_next().await.is_some() {}
}

#[tokio::test]
async fn foreign_ids_are_unavailable() {
    let first = Box::pin(connected_neighbor()).await;
    let second = Box::pin(connected_neighbor()).await;

    assert_ne!(first.primary_id, second.primary_id);
    assert!(matches!(send_targeted(&first.commands, second.primary_id, object_request()).await, Err(Error::SimulatorUnavailable)));

    first.shutdown().await;
    second.shutdown().await;
}

#[tokio::test]
async fn neighbor_backpressure_controls() {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let primary_endpoint = primary.local_addr().expect("primary simulator address");
    let neighbor_endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let session = login_session(primary_endpoint);
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = tokio::sync::watch::channel(false);
    let client = tokio::spawn(run_session(session, command_receiver, event_sender.clone(), disconnect_requested));

    let (_, primary_peer) = receive(&primary).await;
    primary.send_to(&ack(20, 1), primary_peer).await.expect("ack primary circuit");
    receive_message(&primary, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    primary.send_to(&ack(21, 2), primary_peer).await.expect("ack initial movement");

    let mut handshake = fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 22);
    primary.send_to(&handshake, primary_peer).await.expect("send primary handshake");
    receive_message(&primary, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 22, .. }));

    let mut movement_complete = fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    primary.send_to(&movement_complete, primary_peer).await.expect("send primary movement completion");
    assert!(matches!(next_event(&mut events).await, Event::SessionReady { .. }));
    assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 23, .. }));

    event_sender
        .try_send(EventEnvelope::new(Event::Failed("fill application event channel".into())))
        .expect("hold the only ordinary event slot");
    let (neighbor_ip, neighbor_port) = match neighbor_endpoint {
        SocketAddr::V4(address) => (address.ip().octets(), address.port().to_be_bytes()),
        SocketAddr::V6(_) => panic!("fake neighbor should use IPv4"),
    };
    let mut enable = fixture("enable_simulator.hex");
    set_sequence(&mut enable, 30);
    enable[10..18].copy_from_slice(&0x1000_0000_0000_1000u64.to_le_bytes());
    enable[18..22].copy_from_slice(&neighbor_ip);
    enable[22..24].copy_from_slice(&neighbor_port);
    primary.send_to(&enable, primary_peer).await.expect("announce neighbor simulator");

    let (use_circuit, neighbor_peer) = receive(&neighbor).await;
    let use_circuit = Packet::decode(&use_circuit).expect("decode neighbor UseCircuitCode");
    assert_eq!(use_circuit.sequence(), 1);
    assert!(matches!(use_circuit.message(), Message::UseCircuitCode(_)));
    neighbor.send_to(&ack(40, 1), neighbor_peer).await.expect("ack neighbor circuit");

    let mut reliable_chat = fixture("chat_from_simulator.hex");
    reliable_chat[0] |= 0x40;
    set_sequence(&mut reliable_chat, 41);
    neighbor.send_to(&reliable_chat, neighbor_peer).await.expect("send reliable neighbor chat");
    neighbor.send_to(&start_ping_packet(42, 7, 0), neighbor_peer).await.expect("send neighbor ping check");

    let replies = timeout(WAIT, async {
        let mut acknowledged_chat = false;
        let mut answered_ping = false;
        let mut buffer = [0; 65_535];
        while !(acknowledged_chat && answered_ping) {
            let (size, _) = neighbor.recv_from(&mut buffer).await.expect("read neighbor reply");
            let packet = Packet::decode(&buffer[..size]).expect("decode neighbor reply");
            acknowledged_chat |= packet.appended_acks().contains(&41)
                || matches!(packet.message(), Message::PacketAck(ack) if ack.packets.iter().any(|packet| packet.id == 41));
            answered_ping |= matches!(packet.message(), Message::CompletePingCheck(message) if message.ping_id.ping_id == 7);
        }
    })
    .await;
    assert!(replies.is_ok(), "neighbor must ACK reliable traffic and answer ping checks while event delivery is blocked");

    disconnect_signal.send_replace(true);
    receive_message(&primary, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(timeout(Duration::from_millis(200), client).await.expect("disconnect should finish").expect("client task").is_ok());
}

#[tokio::test]
async fn neighbor_overload_keeps_control() {
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let neighbor_endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
    let (event_sender, mut event_receiver) = mpsc::channel(1);
    event_sender
        .try_send(super::NeighborEvent::Simulator(Event::Failed("hold application events".into())))
        .expect("fill neighbor event channel");
    let (_shutdown_signal, shutdown) = tokio::sync::watch::channel(false);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(32);
    let (_commands, command_receiver) = mpsc::channel(super::NEIGHBOR_COMMAND_CAPACITY);
    let neighbor_connection = super::NeighborConnection::connect(
        simulator_id,
        login_session(neighbor_endpoint),
        0x1000_0000_0000_1000,
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect neighbor circuit");
    let mut task = tokio::spawn(neighbor_connection.run(event_sender, shutdown, neighbor_requests, command_receiver));

    let (use_circuit, neighbor_peer) = receive(&neighbor).await;
    assert!(matches!(Packet::decode(&use_circuit).expect("decode neighbor circuit packet").message(), Message::UseCircuitCode(_)));
    neighbor.send_to(&ack(40, 1), neighbor_peer).await.expect("ack neighbor circuit");
    let first_sequence = 41;
    let burst_len = super::MAX_QUEUED_SIMULATOR_EVENTS * 4;
    let last_sequence = first_sequence + u32::try_from(burst_len).expect("small simulator burst") - 1;
    for sequence in first_sequence..=last_sequence {
        let mut chat = fixture("chat_from_simulator.hex");
        chat[0] |= 0x40;
        set_sequence(&mut chat, sequence);
        neighbor.send_to(&chat, neighbor_peer).await.expect("send reliable burst packet");
    }

    neighbor.send_to(&start_ping_packet(last_sequence + 1, 7, 0), neighbor_peer).await.expect("send ping after the consumer pause");

    let mut acknowledged_sequences = HashSet::with_capacity(burst_len);
    let mut answered_ping = false;
    timeout(WAIT, async {
        while !(acknowledged_sequences.len() == burst_len && answered_ping) {
            let (bytes, _) = receive(&neighbor).await;
            let packet = Packet::decode(&bytes).expect("decode neighbor control reply");
            acknowledged_sequences.extend(
                packet.appended_acks().iter().copied().filter(|sequence| (*sequence >= first_sequence) && (*sequence <= last_sequence)),
            );
            if let Message::PacketAck(ack) = packet.message() {
                acknowledged_sequences.extend(
                    ack.packets.iter().map(|item| item.id).filter(|sequence| (*sequence >= first_sequence) && (*sequence <= last_sequence)),
                );
            }
            answered_ping |= matches!(packet.message(), Message::CompletePingCheck(message) if message.ping_id.ping_id == 7);
        }
    })
    .await
    .expect("neighbor ACK and ping handling should continue through a paused-consumer burst");
    assert_eq!(acknowledged_sequences.len(), burst_len, "every reliable burst packet must be acknowledged");

    let mut duplicate_chat = fixture("chat_from_simulator.hex");
    duplicate_chat[0] |= 0x40;
    set_sequence(&mut duplicate_chat, first_sequence);
    neighbor.send_to(&duplicate_chat, neighbor_peer).await.expect("retransmit a reliable packet after overload begins");
    receive_ack(&neighbor, first_sequence).await;

    let mut disable = fixture("disable_simulator.hex");
    disable[0] |= 0x40;
    set_sequence(&mut disable, last_sequence + 2);
    neighbor.send_to(&disable, neighbor_peer).await.expect("disable the overloaded neighbor");
    receive_ack(&neighbor, last_sequence + 2).await;

    assert!(!task.is_finished(), "the neighbor should remain alive while queued events cannot drain");
    assert!(
        timeout(super::LOGOUT_TIMEOUT + Duration::from_secs(1), &mut task)
            .await
            .expect("overload teardown should finish within its deadline")
            .expect("neighbor task should not panic")
            .expect_err("backpressure must end with an explicit overload failure")
            .contains(super::EVENT_BACKLOG_DISCONNECT_REASON)
    );
    assert!(
        matches!(event_receiver.try_recv(), Ok(super::NeighborEvent::Simulator(Event::Failed(message))) if message == "hold application events")
    );
    assert!(
        matches!(event_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected)),
        "overflow must not deliver queued application updates after failure"
    );
}

#[test]
fn full_neighbor_queue_keeps_teardown_open() {
    let (events, receiver) = mpsc::channel(1);
    events.try_send(super::NeighborEvent::Simulator(Event::Failed("hold event".into()))).expect("fill application event channel");
    let mut pending = std::collections::VecDeque::with_capacity(super::MAX_QUEUED_SIMULATOR_EVENTS);
    for _ in 0..super::MAX_QUEUED_SIMULATOR_EVENTS {
        pending.push_back(Event::Failed("pending event".into()));
    }
    let mut closing = false;
    let mut teardown_deadline = None;
    let mut close_reason = None;

    assert!(super::queue_neighbor_event(
        &events,
        &mut pending,
        Event::Failed("overflow".into()),
        &mut closing,
        &mut teardown_deadline,
        &mut close_reason,
    ));
    assert!(closing);
    assert!(teardown_deadline.is_some());
    assert_eq!(close_reason, Some(super::EVENT_BACKLOG_DISCONNECT_REASON));
    assert_eq!(pending.len(), super::MAX_QUEUED_SIMULATOR_EVENTS);

    drop(receiver);
    assert!(!super::queue_neighbor_event(
        &events,
        &mut pending,
        Event::Failed("receiver closed".into()),
        &mut closing,
        &mut teardown_deadline,
        &mut close_reason,
    ));
}

#[tokio::test]
async fn retry_limit_disconnects_primary() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let endpoint = simulator.local_addr().expect("read simulator address");
    let mut connection = SimulatorConnection::connect_with_status(
        login_session(endpoint),
        LoginResponseData::default(),
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect simulator circuit");
    let (_, peer) = receive(&simulator).await;

    connection.handle_primary_event_queue_exit(EventQueueExit::Stopped, false).await.expect("ordinary poll stop keeps session");
    let mut buffer = [0; 65_535];
    assert_eq!(
        simulator.try_recv_from(&mut buffer).expect_err("ordinary poll stop sends no logout").kind(),
        std::io::ErrorKind::WouldBlock
    );

    let error = connection
        .handle_primary_event_queue_exit(EventQueueExit::RetryLimitReached, false)
        .await
        .expect_err("repeated primary poll errors fail the session");
    assert!(error.contains("EventQueueGet"));
    let (bytes, logout_peer) = receive(&simulator).await;
    let logout = Packet::decode(&bytes).expect("decode logout");
    assert!(matches!(logout.message(), Message::LogoutRequest(_)));
    assert_eq!(logout_peer, peer);
}

#[tokio::test]
async fn canceled_primary_event_queue_keeps_circuit_open() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let mut connection = SimulatorConnection::connect_with_status(
        login_session(simulator.local_addr().expect("read simulator address")),
        LoginResponseData::default(),
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect simulator circuit");
    let _ = receive(&simulator).await;

    connection
        .handle_primary_event_queue_exit(EventQueueExit::PollCanceled, false)
        .await
        .expect("a canceled long poll is not a failed session");

    let mut buffer = [0; 65_535];
    assert_eq!(simulator.try_recv_from(&mut buffer).expect_err("poll cancellation sends no logout").kind(), std::io::ErrorKind::WouldBlock);
    drop(connection);
}

#[tokio::test]
async fn retry_avoids_double_logout() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let endpoint = simulator.local_addr().expect("read simulator address");
    let mut connection = SimulatorConnection::connect_with_status(
        login_session(endpoint),
        LoginResponseData::default(),
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect simulator circuit");
    let (_, peer) = receive(&simulator).await;

    let (events, _event_receiver) = mpsc::channel(1);
    let mut pending_events = std::collections::VecDeque::with_capacity(super::MAX_PENDING_SIMULATOR_EVENTS);
    for _ in 1..super::MAX_PENDING_SIMULATOR_EVENTS {
        pending_events.push_back(EventEnvelope::new(Event::Failed("pending event".into())));
    }
    let mut drop_application_messages = false;
    let mut logout_deadline = None;
    let mut disconnect_reason = None;
    connection
        .publish_event(
            &events,
            &mut pending_events,
            &mut drop_application_messages,
            Event::Failed("backlog limit".into()),
            &mut logout_deadline,
            &mut disconnect_reason,
        )
        .await
        .expect("backlog sends first logout");
    assert!(logout_deadline.is_some());

    let (bytes, logout_peer) = receive(&simulator).await;
    let logout = Packet::decode(&bytes).expect("decode backlog logout");
    assert!(matches!(logout.message(), Message::LogoutRequest(_)));
    assert_eq!(logout_peer, peer);

    let error = connection
        .handle_primary_event_queue_exit(EventQueueExit::RetryLimitReached, logout_deadline.is_some())
        .await
        .expect_err("repeated primary poll errors fail the session");
    assert!(error.contains("EventQueueGet"));

    let mut buffer = [0; 65_535];
    assert!(timeout(Duration::from_millis(50), simulator.recv_from(&mut buffer)).await.is_err());
}

fn login_session(simulator_address: SocketAddr) -> AgentSession {
    AgentSession {
        first_name: "Test".into(),
        last_name: "Resident".into(),
        grid: Grid::Agni,
        message: "Welcome".into(),
        simulator_address,
        agent_id: UUID::parse_str(AGENT_ID).expect("agent UUID"),
        session_id: UUID::parse_str(SESSION_ID).expect("session UUID"),
        circuit_code: CIRCUIT_CODE,
        seed_capability: Url::parse("http://127.0.0.1:1/seed").expect("local seed capability URL"),
    }
}

struct NeighborHarness {
    primary: UdpSocket,
    neighbor: UdpSocket,
    primary_peer: SocketAddr,
    neighbor_peer: SocketAddr,
    primary_id: SimulatorId,
    neighbor_id: SimulatorId,
    commands: mpsc::Sender<Command>,
    events: mpsc::Receiver<EventEnvelope>,
    disconnect: tokio::sync::watch::Sender<bool>,
    client: tokio::task::JoinHandle<Result<(), String>>,
}

impl NeighborHarness {
    async fn shutdown(self) {
        self.disconnect.send_replace(true);
        receive_message(&self.primary, |message| matches!(message, Message::LogoutRequest(_))).await;
        assert!(timeout(Duration::from_millis(200), self.client).await.expect("disconnect should finish").expect("client task").is_ok());
    }
}

async fn connected_neighbor() -> NeighborHarness {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let primary_endpoint = primary.local_addr().expect("primary simulator address");
    let neighbor_endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(TEST_EVENT_CAPACITY);
    let (disconnect, disconnect_requested) = tokio::sync::watch::channel(false);
    let client = tokio::spawn(run_session(login_session(primary_endpoint), command_receiver, event_sender, disconnect_requested));

    let (_, primary_peer) = receive(&primary).await;
    primary.send_to(&ack(20, 1), primary_peer).await.expect("ack primary circuit");
    receive_message(&primary, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    primary.send_to(&ack(21, 2), primary_peer).await.expect("ack initial movement");
    let mut handshake = fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 22);
    primary.send_to(&handshake, primary_peer).await.expect("send primary handshake");
    receive_message(&primary, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    let mut movement_complete = fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    primary.send_to(&movement_complete, primary_peer).await.expect("complete primary movement");

    let mut primary_id = None;
    let mut ready_id = None;
    let mut movement_seen = false;
    while primary_id.is_none() || ready_id.is_none() || !movement_seen {
        match next_raw_event(&mut events).await {
            Event::SimulatorConnected { simulator_id, region_handle: None, .. } => primary_id = Some(simulator_id),
            Event::SessionReady { simulator_id, .. } => ready_id = Some(simulator_id),
            Event::SimulatorMessage { sequence: 23, .. } => movement_seen = true,
            _ => {}
        }
    }
    let primary_id = primary_id.expect("primary connection ID");
    assert_eq!(ready_id, Some(primary_id));

    announce_neighbor(&primary, primary_peer, neighbor_endpoint, 0x1000_0000_0000_1000, 30).await;
    let (bytes, neighbor_peer) = receive(&neighbor).await;
    let use_circuit = Packet::decode(&bytes).expect("decode neighbor UseCircuitCode");
    assert_eq!(use_circuit.sequence(), 1);
    assert!(use_circuit.is_reliable());
    assert!(matches!(use_circuit.message(), Message::UseCircuitCode(_)));
    assert_ne!(neighbor_peer, primary_peer, "each simulator must have a separate UDP circuit");
    neighbor.send_to(&ack(40, 1), neighbor_peer).await.expect("ack neighbor circuit");
    let neighbor_id = loop {
        match next_event(&mut events).await {
            Event::SimulatorConnected { simulator_id, region_handle: Some(0x1000_0000_0000_1000), endpoint }
                if endpoint == neighbor_endpoint =>
            {
                break simulator_id;
            }
            _ => {}
        }
    };
    NeighborHarness { primary, neighbor, primary_peer, neighbor_peer, primary_id, neighbor_id, commands, events, disconnect, client }
}

fn ack(sequence: u32, acknowledged: u32) -> Vec<u8> {
    let mut packet = fixture("packet_ack.hex");
    set_sequence(&mut packet, sequence);
    packet[10] = 1;
    packet[11..15].copy_from_slice(&acknowledged.to_le_bytes());
    packet.truncate(15);
    packet
}

fn start_ping_packet(sequence: u32, ping_id: u8, oldest_unacked: u32) -> Vec<u8> {
    Packet::new(sequence, Message::StartPingCheck(StartPingCheck { ping_id: StartPingID { ping_id, oldest_unacked } }))
        .encode()
        .expect("encode ping check")
}

async fn receive(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    let mut buffer = [0; 65_535];
    let (size, endpoint) = timeout(WAIT, socket.recv_from(&mut buffer)).await.expect("receive simulator datagram").expect("read datagram");
    (buffer[..size].to_vec(), endpoint)
}

async fn receive_message(socket: &UdpSocket, expected: impl Fn(&Message) -> bool) -> Packet {
    timeout(WAIT, async {
        loop {
            let (bytes, _) = receive(socket).await;
            let packet = Packet::decode(&bytes).expect("decode simulator packet");
            if expected(packet.message()) {
                return packet;
            }
        }
    })
    .await
    .expect("receive expected simulator message")
}

async fn receive_message_with_peer(socket: &UdpSocket, expected: impl Fn(&Message) -> bool) -> (Packet, SocketAddr) {
    timeout(WAIT, async {
        loop {
            let (bytes, peer) = receive(socket).await;
            let packet = Packet::decode(&bytes).expect("decode simulator packet");
            if expected(packet.message()) {
                return (packet, peer);
            }
        }
    })
    .await
    .expect("receive expected simulator message")
}

async fn receive_ack(socket: &UdpSocket, sequence: u32) {
    timeout(WAIT, async {
        loop {
            let (bytes, _) = receive(socket).await;
            let packet = Packet::decode(&bytes).expect("decode simulator acknowledgement");
            if packet.appended_acks().contains(&sequence)
                || matches!(packet.message(), Message::PacketAck(ack) if ack.packets.iter().any(|item| item.id == sequence))
            {
                return;
            }
        }
    })
    .await
    .expect("receive reliable packet acknowledgement");
}

async fn announce_neighbor(primary: &UdpSocket, peer: SocketAddr, endpoint: SocketAddr, region_handle: u64, sequence: u32) {
    let (ip, port) = match endpoint {
        SocketAddr::V4(address) => (address.ip().octets(), address.port().to_be_bytes()),
        SocketAddr::V6(_) => panic!("fake neighbor should use IPv4"),
    };
    let mut enable = fixture("enable_simulator.hex");
    set_sequence(&mut enable, sequence);
    enable[10..18].copy_from_slice(&region_handle.to_le_bytes());
    enable[18..22].copy_from_slice(&ip);
    enable[22..24].copy_from_slice(&port);
    primary.send_to(&enable, peer).await.expect("announce neighbor simulator");
}

async fn send_targeted(commands: &mpsc::Sender<Command>, simulator_id: SimulatorId, message: Message) -> Result<(), Error> {
    let (completed, response) = oneshot::channel();
    commands
        .send(Command::SendTo { simulator_id, message, delivery: crate::Delivery::Reliable, completed })
        .await
        .map_err(|_| Error::SessionClosed)?;
    response.await.map_err(|_| Error::SessionClosed)?
}

fn object_request() -> Message {
    Message::RequestMultipleObjects(RequestMultipleObjects {
        agent_data: RequestAgentData {
            agent_id: UUID::parse_str(AGENT_ID).expect("agent UUID"),
            session_id: UUID::parse_str(SESSION_ID).expect("session UUID"),
        },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 77 }],
    })
}

async fn next_event(events: &mut mpsc::Receiver<EventEnvelope>) -> Event {
    loop {
        let event = next_raw_event(events).await;
        if matches!(&event, Event::SimulatorConnected { region_handle: None, .. }) {
            continue;
        }
        return event;
    }
}

async fn next_neighbor_event(events: &mut mpsc::Receiver<super::NeighborEvent>) -> Event {
    timeout(WAIT, async {
        match events.recv().await.expect("neighbor event channel remains open") {
            super::NeighborEvent::Simulator(event) => event,
            super::NeighborEvent::Finished { .. } => panic!("a standalone neighbor worker must not emit its manager completion"),
        }
    })
    .await
    .expect("receive neighbor event")
}

async fn next_raw_event(events: &mut mpsc::Receiver<EventEnvelope>) -> Event {
    timeout(WAIT, async {
        loop {
            let event = events.recv().await.expect("event channel remains open").into_event();
            if matches!(&event, Event::CapabilityError { capability, .. } if capability == "EventQueueGet") {
                continue;
            }
            return event;
        }
    })
    .await
    .expect("receive network event")
}

fn fixture(name: &str) -> Vec<u8> {
    let source = test_support::udp::vectors::get(name).unwrap_or_else(|| panic!("unknown fixture {name}"));
    test_support::parse_hex_fixture(source).unwrap_or_else(|error| panic!("invalid fixture {name}: {error}"))
}

fn set_sequence(bytes: &mut [u8], sequence: u32) {
    bytes[1..5].copy_from_slice(&sequence.to_be_bytes());
}
