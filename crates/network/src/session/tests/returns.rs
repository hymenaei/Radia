use std::sync;

use super::*;

async fn run_event_batch_order_survives_forwarding() {
    use crate::fixtures::http::{EventQueueTestServer, EventServerResponse, response};

    let (release, poll_response) = sync::mpsc::channel();
    let batch = r#"<llsd><map><key>id</key><integer>1</integer><key>events</key><array>
        <map><key>message</key><string>FirstEvent</string><key>body</key><map/></map>
        <map><key>message</key><string>SecondEvent</string><key>body</key><map/></map>
    </array></map></llsd>"#;
    let server = EventQueueTestServer::start(move |base| {
        let seed = format!("<llsd><map><key>EventQueueGet</key><uri>{base}events</uri></map></llsd>");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed)),
            EventServerResponse::Controlled(poll_response),
            EventServerResponse::Hold,
        ]
    });
    let mut harness = Box::pin(connected_neighbor_with_seed(server.url("/seed"))).await;
    release.send(response(200, "OK", "application/llsd+xml", None, batch)).expect("release EventQueue batch after session setup");
    let event = timeout(Duration::from_secs(10), async {
        loop {
            let event = harness.events.recv().await.expect("session event channel remains open").into_event();
            if let Event::CapabilityEventBatch { order, events, .. } = event
                && events.iter().any(|event| event.message == "FirstEvent")
            {
                break (order, events.into_iter().map(|event| event.message).collect::<Vec<_>>());
            }
        }
    })
    .await
    .expect("EventQueue batch reaches the session event stream");

    let (order, names) = event;
    assert_eq!(order.ordinal, 0);
    assert_ne!(order.incarnation, 0);
    assert_eq!(names, ["FirstEvent", "SecondEvent"]);

    harness.shutdown().await;
    assert_eq!(server.finish().len(), 3);
}

async fn run_event_queue_teleport_finish_promotes_destination() {
    use llsd::{Map, Value, xml};

    use crate::fixtures::http::{EventQueueTestServer, EventServerResponse, response};

    let (release, poll_response) = sync::mpsc::channel();
    let server = EventQueueTestServer::start(|base| {
        let seed = format!("<llsd><map><key>EventQueueGet</key><uri>{base}events</uri></map></llsd>");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed)),
            EventServerResponse::Controlled(poll_response),
            EventServerResponse::Hold,
        ]
    });
    let seed_capability = server.url("/seed");
    let mut harness = Box::pin(connected_neighbor_with_seed(seed_capability.clone())).await;
    let session = AgentSession { seed_capability, ..login_session(harness.primary.local_addr().expect("primary simulator address")) };
    let destination = UdpSocket::bind("127.0.0.1:0").await.expect("bind teleport destination");
    let destination_endpoint = destination.local_addr().expect("teleport destination address");
    let destination_region = 0x3000_0000_0000_3000_u64;
    let SocketAddr::V4(destination_endpoint) = destination_endpoint else { panic!("test simulator must use IPv4") };
    let event = Value::Map(Map::from([(
        "Info".into(),
        Value::Array(vec![Value::Map(Map::from([
            ("AgentID".into(), Value::Uuid(session.agent_id)),
            ("RegionHandle".into(), Value::Binary(destination_region.to_be_bytes().to_vec().into())),
            ("SimIP".into(), Value::Binary(destination_endpoint.ip().octets().to_vec().into())),
            ("SimPort".into(), Value::Integer(i32::from(destination_endpoint.port()))),
            ("SeedCapability".into(), Value::Uri(session.seed_capability.to_string().into())),
            ("TeleportFlags".into(), Value::Integer(7)),
            ("SimAccess".into(), Value::Integer(13)),
        ]))]),
    )]));
    let body = xml::to_string(&Value::Map(Map::from([
        ("id".into(), Value::Integer(1)),
        (
            "events".into(),
            Value::Array(vec![Value::Map(Map::from([("message".into(), Value::String("TeleportFinish".into())), ("body".into(), event)]))]),
        ),
    ])))
    .expect("encode TeleportFinish EventQueue batch");
    release.send(response(200, "OK", "application/llsd+xml", None, &body)).expect("deliver TeleportFinish event");

    timeout(WAIT, async {
        loop {
            if let Event::CapabilityEventBatch { events, .. } = next_raw_event(&mut harness.events).await
                && let Some(event) = events.iter().find(|event| event.message == "TeleportFinish")
            {
                let fields = crate::session::teleport::event_fields(&event.body).expect("published finish body");
                let info = crate::session::teleport::event_block(fields, "Info").expect("published finish Info");
                assert_eq!(info.get("SeedCapability"), Some(&Value::String("[redacted]".into())));
                break;
            }
        }
    })
    .await
    .expect("TeleportFinish from EventQueue reaches the session event stream");

    let (bytes, destination_peer) = timeout(WAIT, receive(&destination)).await.expect("destination circuit should be established");
    let use_circuit = Packet::decode(&bytes).expect("decode destination circuit packet");
    std::assert_matches!(use_circuit.message(), Message::UseCircuitCode(_));
    assert!(use_circuit.is_reliable());
    destination.send_to(&ack(80, use_circuit.sequence()), destination_peer).await.expect("ack destination circuit");

    let mut handshake = test_fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 81);
    destination.send_to(&handshake, destination_peer).await.expect("send destination handshake");
    let reply = receive_message(&destination, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    let movement = receive_message(&destination, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    destination.send_to(&ack(82, reply.sequence()), destination_peer).await.expect("ack destination handshake reply");
    destination.send_to(&ack(83, movement.sequence()), destination_peer).await.expect("ack destination movement request");
    destination
        .send_to(&movement_complete_packet(84, session.agent_id, session.session_id, destination_region), destination_peer)
        .await
        .expect("complete movement into teleport destination");

    timeout(WAIT, async {
        loop {
            match next_raw_event(&mut harness.events).await {
                Event::SimulatorPromoted { previous_simulator_id, simulator_id, session, .. } => {
                    assert_eq!(previous_simulator_id, harness.primary_id);
                    assert_ne!(simulator_id, harness.primary_id);
                    assert_eq!(session.simulator_address, destination.local_addr().expect("teleport destination address"));
                    break;
                }
                Event::Failed(reason) => panic!("EventQueue teleport handoff failed: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("EventQueue teleport destination is promoted after movement completion");

    harness.disconnect.send_replace(true);
    receive_message(&destination, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(timeout(WAIT, harness.client).await.expect("shutdown completes").expect("session task").is_ok());
    assert_eq!(server.finish().len(), 3);
}

async fn run_return_handoff_reuses_root_connection() {
    let mut harness = Box::pin(connected_neighbor()).await;
    let session = login_session(harness.primary.local_addr().expect("primary endpoint"));
    let neighbor_region = 0x1000_0000_0000_1000;
    let seed = session.seed_capability.as_str();

    let to_neighbor = Packet::new(
        100,
        teleport_finish_to(session.agent_id, harness.neighbor.local_addr().expect("neighbor endpoint"), neighbor_region, seed),
    )
    .with_reliable()
    .encode()
    .expect("encode first handoff");
    harness.primary.send_to(&to_neighbor, harness.primary_peer).await.expect("announce neighbor handoff");
    let mut handshake = test_fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 102);
    harness.neighbor.send_to(&handshake, harness.neighbor_peer).await.expect("handshake with neighbor");
    let reply = receive_message(&harness.neighbor, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    let movement = receive_message(&harness.neighbor, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    harness.neighbor.send_to(&ack(103, reply.sequence()), harness.neighbor_peer).await.expect("ack neighbor handshake");
    harness.neighbor.send_to(&ack(104, movement.sequence()), harness.neighbor_peer).await.expect("ack neighbor movement");
    harness
        .neighbor
        .send_to(&movement_complete_packet(105, session.agent_id, session.session_id, neighbor_region), harness.neighbor_peer)
        .await
        .expect("complete movement into neighbor");
    timeout(WAIT, async {
        loop {
            if matches!(
                next_raw_event(&mut harness.events).await,
                Event::SimulatorPromoted { previous_simulator_id, simulator_id, .. }
                    if previous_simulator_id == harness.primary_id && simulator_id == harness.neighbor_id
            ) {
                break;
            }
        }
    })
    .await
    .expect("neighbor should become primary");

    let to_root = Packet::new(
        110,
        teleport_finish_to(
            session.agent_id,
            harness.primary.local_addr().expect("original simulator endpoint"),
            harness.primary_region_handle,
            seed,
        ),
    )
    .with_reliable()
    .encode()
    .expect("encode return handoff");
    harness.neighbor.send_to(&to_root, harness.neighbor_peer).await.expect("announce return to root region");
    let (movement, peer) =
        receive_message_with_peer(&harness.primary, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    assert_eq!(peer, harness.primary_peer, "return handoff must use the original live circuit");
    harness.primary.send_to(&ack(111, movement.sequence()), peer).await.expect("ack return movement");
    harness
        .primary
        .send_to(&movement_complete_packet(112, session.agent_id, session.session_id, harness.primary_region_handle), peer)
        .await
        .expect("complete movement back to root");
    receive_ack(&harness.primary, 112).await;
    timeout(WAIT, async {
        loop {
            match next_raw_event(&mut harness.events).await {
                Event::SimulatorPromoted { previous_simulator_id, simulator_id, session, .. } => {
                    assert_eq!(previous_simulator_id, harness.neighbor_id);
                    assert_eq!(simulator_id, harness.primary_id);
                    assert_eq!(session.simulator_address, harness.primary.local_addr().expect("root endpoint"));
                    break;
                }
                Event::Failed(reason) => panic!("return handoff failed: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("root region should be promoted");

    harness
        .commands
        .send(Command::SendMessage { message: object_request(), delivery: Delivery::Unreliable })
        .await
        .expect("send through the reused root circuit");
    let (_, route_peer) =
        receive_message_with_peer(&harness.primary, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
    assert_eq!(route_peer, harness.primary_peer);
    harness.shutdown().await;
}

async fn run_stale_cancel() {
    use llsd::{Map, xml};

    use crate::fixtures::http::{EventQueueTestServer, EventServerResponse, response};

    let (release, poll_response) = sync::mpsc::channel();
    let server = EventQueueTestServer::start(|base| {
        let seed = format!("<llsd><map><key>EventQueueGet</key><uri>{base}events</uri></map></llsd>");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed)),
            EventServerResponse::Controlled(poll_response),
            EventServerResponse::Hold,
        ]
    });
    let mut harness = Box::pin(connected_neighbor_with_seed(server.url("/seed"))).await;
    let target = harness.neighbor.local_addr().expect("target address");
    let session = login_session(harness.primary.local_addr().expect("primary address"));
    let region = 0x1000_0000_0000_1000;
    let finish = Packet::new(60, teleport_finish_to(session.agent_id, target, region, session.seed_capability.as_str()))
        .with_reliable()
        .encode()
        .expect("handoff packet");
    harness.primary.send_to(&finish, harness.primary_peer).await.expect("announce handoff");
    loop {
        if matches!(next_raw_event(&mut harness.events).await, Event::SimulatorMessage { message: Message::TeleportFinish(_), .. }) {
            break;
        }
    }
    receive_ack(&harness.primary, 60).await;
    let mut handshake = test_fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 70);
    harness.neighbor.send_to(&handshake, harness.neighbor_peer).await.expect("handshake target");
    let reply = receive_message(&harness.neighbor, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    let movement = receive_message(&harness.neighbor, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    harness.neighbor.send_to(&ack(71, reply.sequence()), harness.neighbor_peer).await.expect("ack handshake");
    harness.neighbor.send_to(&ack(72, movement.sequence()), harness.neighbor_peer).await.expect("ack movement");

    let cancel = Message::TeleportCancel(TeleportCancel { agent_id: session.agent_id, session_id: session.session_id });
    harness.pause_sends.send_replace(true);
    let (completed, mut canceled) = oneshot::channel();
    harness
        .commands
        .send(Command::SendTo { simulator_id: harness.primary_id, message: cancel, delivery: Delivery::Reliable, completed })
        .await
        .expect("queue cancellation");
    timeout(WAIT, harness.send_entered.notified()).await.expect("hold cancellation send");
    let SocketAddr::V4(target) = target else { panic!("IPv4 target") };
    let crossing = Value::Map(Map::from([
        (
            "AgentData".into(),
            Value::Array(vec![Value::Map(Map::from([
                ("AgentID".into(), Value::Uuid(session.agent_id)),
                ("SessionID".into(), Value::Uuid(session.session_id)),
            ]))]),
        ),
        (
            "RegionData".into(),
            Value::Array(vec![Value::Map(Map::from([
                ("RegionHandle".into(), Value::Binary(region.to_be_bytes().to_vec().into())),
                ("SimIP".into(), Value::Binary(target.ip().octets().to_vec().into())),
                ("SimPort".into(), Value::Integer(i32::from(target.port()))),
                ("SeedCapability".into(), Value::Uri(session.seed_capability.to_string().into())),
            ]))]),
        ),
        ("Info".into(), Value::Array(vec![Value::Map(Map::new())])),
    ]));
    let body = xml::to_string(&Value::Map(Map::from([
        ("id".into(), Value::Integer(1)),
        (
            "events".into(),
            Value::Array(vec![Value::Map(Map::from([
                ("message".into(), Value::String("CrossedRegion".into())),
                ("body".into(), crossing),
            ]))]),
        ),
    ])))
    .expect("encode crossing batch");
    release.send(response(200, "OK", "application/llsd+xml", None, &body)).expect("deliver newer crossing evidence");
    loop {
        if let Event::CapabilityEventBatch { events, .. } = next_raw_event(&mut harness.events).await
            && let Some(event) = events.iter().find(|event| event.message == "CrossedRegion")
        {
            let fields = crate::session::teleport::event_fields(&event.body).expect("published CrossedRegion body");
            let region = crate::session::teleport::event_block(fields, "RegionData").expect("published RegionData block");
            assert_eq!(region.get("SeedCapability"), Some(&Value::String("[redacted]".into())));
            break;
        }
    }
    std::assert_matches!(canceled.try_recv(), Err(oneshot::error::TryRecvError::Empty));
    harness.pause_sends.send_replace(false);
    let sent = receive_message(&harness.primary, |message| matches!(message, Message::TeleportCancel(_))).await;
    assert!(timeout(WAIT, canceled).await.expect("cancel completes").expect("cancel reply").is_ok());
    harness.primary.send_to(&ack(61, sent.sequence()), harness.primary_peer).await.expect("ack old cancellation");
    harness.primary.send_to(&start_ping_packet(62, 9, 0), harness.primary_peer).await.expect("completion barrier");
    receive_message(&harness.primary, |message| matches!(message, Message::CompletePingCheck(ping) if ping.ping_id.ping_id == 9)).await;
    harness
        .neighbor
        .send_to(&movement_complete_packet(73, session.agent_id, session.session_id, region), harness.neighbor_peer)
        .await
        .expect("complete newer handoff");
    loop {
        match next_raw_event(&mut harness.events).await {
            Event::SimulatorPromoted { previous_simulator_id, simulator_id, .. } => {
                assert_eq!(previous_simulator_id, harness.primary_id);
                assert_eq!(simulator_id, harness.neighbor_id);
                break;
            }
            Event::SimulatorDisconnected { simulator_id, .. } if simulator_id == harness.neighbor_id => {
                panic!("stale cancellation retired the newer target")
            }
            Event::Failed(reason) => panic!("handoff failed: {reason}"),
            _ => {}
        }
    }
    harness.disconnect.send_replace(true);
    receive_message(&harness.neighbor, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(timeout(WAIT, harness.client).await.expect("shutdown finishes").expect("runtime task").is_ok());
    assert_eq!(server.finish().len(), 3);
}

async fn run_disconnect_logs_out_promoted_primary() {
    use crate::event::Event::SimulatorMessage;
    use protocol::lludp::Message::RegionHandshake;

    let mut harness = Box::pin(connected_neighbor()).await;
    let target_region = 0x1000_0000_0000_1000;
    let target_endpoint = harness.neighbor.local_addr().expect("destination simulator address");
    let session = login_session(harness.primary.local_addr().expect("primary simulator address"));
    let teleport_finish =
        Packet::new(30, teleport_finish_to(session.agent_id, target_endpoint, target_region, session.seed_capability.as_str()))
            .with_reliable()
            .encode()
            .expect("encode TeleportFinish");
    harness.primary.send_to(&teleport_finish, harness.primary_peer).await.expect("send TeleportFinish");

    timeout(WAIT, async {
        loop {
            if matches!(
                next_raw_event(&mut harness.events).await,
                SimulatorMessage { simulator_id, message: Message::TeleportFinish(_), .. }
                    if simulator_id == harness.primary_id
            ) {
                break;
            }
        }
    })
    .await
    .expect("manager should accept the handoff before the shutdown race");

    let mut handshake = test_fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 41);
    harness.neighbor.send_to(&handshake, harness.neighbor_peer).await.expect("send destination handshake");
    let reply = receive_message(&harness.neighbor, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    let movement_request = receive_message(&harness.neighbor, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    harness.neighbor.send_to(&ack(42, reply.sequence()), harness.neighbor_peer).await.expect("ack destination handshake reply");
    harness.neighbor.send_to(&ack(43, movement_request.sequence()), harness.neighbor_peer).await.expect("ack destination movement request");
    timeout(WAIT, async {
        loop {
            if matches!(
                next_raw_event(&mut harness.events).await,
                SimulatorMessage { simulator_id, message: RegionHandshake(_), .. } if simulator_id == harness.neighbor_id
            ) {
                break;
            }
        }
    })
    .await
    .expect("manager should be waiting for the destination movement completion");

    let movement = movement_complete_packet(44, session.agent_id, session.session_id, target_region);
    harness.neighbor.send_to(&movement, harness.neighbor_peer).await.expect("send the destination movement completion");
    timeout(WAIT, async {
        loop {
            match next_raw_event(&mut harness.events).await {
                Event::SimulatorPromoted { previous_simulator_id, simulator_id, .. } => {
                    assert_eq!(previous_simulator_id, harness.primary_id);
                    assert_eq!(simulator_id, harness.neighbor_id);
                    break;
                }
                Event::Failed(reason) => panic!("promotion failed: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("destination movement completion should promote the simulator");

    harness.disconnect.send_replace(true);
    let logout = timeout(WAIT, receive_message(&harness.neighbor, |message| matches!(message, Message::LogoutRequest(_))))
        .await
        .expect("shutdown should send LogoutRequest to the promoted primary");
    assert!(logout.is_reliable());
    std::assert_matches!(logout.message(), Message::LogoutRequest(_));
    timeout(WAIT, async {
        loop {
            match next_raw_event(&mut harness.events).await {
                Event::SimulatorDisconnected { simulator_id, .. } if simulator_id == harness.neighbor_id => break,
                Event::Failed(reason) => panic!("shutdown after promotion failed: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("the promoted simulator should emit its terminal lifecycle event");
    assert!(timeout(WAIT, harness.client).await.expect("shutdown should finish").expect("manager task").is_ok());
}

#[test]
fn event_batch_preserves_forwarded_order() {
    run_large_stack_test(run_event_batch_order_survives_forwarding);
}

#[test]
fn event_queue_teleport_finish_promotes_destination() {
    run_large_stack_test(run_event_queue_teleport_finish_promotes_destination);
}

#[test]
fn return_handoff_reuses_root_connection() {
    run_large_stack_test(run_return_handoff_reuses_root_connection);
}

#[test]
fn stale_cancel_keeps_new_handoff() {
    run_large_stack_test(run_stale_cancel);
}

#[test]
fn disconnect_logs_out_promoted_primary() {
    run_large_stack_test(run_disconnect_logs_out_promoted_primary);
}
