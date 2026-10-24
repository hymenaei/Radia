use std::{collections::HashMap, future::pending, net::SocketAddr, num::NonZeroU64, time::Duration};

use llsd::{Map, Value};
use protocol::{
    lludp::Message,
    login::ResponseData,
    messages::{
        AgentMovementComplete,
        agent::agent_movement_complete::{AgentData, Data, SimData},
    },
    messages::{TeleportCancel, TeleportFinish},
};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, watch},
    task::JoinSet,
    time::{self, timeout},
};
use url::Url;
use uuid::Uuid;

use super::{
    CoordinatorRunState, EVENT_BACKLOG_DISCONNECT_REASON, FinishOrder, HANDOFF_COMMAND_RETRY_INTERVAL, HandoffAnnouncement,
    HandoffDecision, HandoffEvidence, HandoffGeneration, HandoffOrder, HandoffState, HandoffTarget, NEIGHBOR_COMMAND_CAPACITY,
    PendingPromotion, PromotionSetup, SimulatorCommand, SimulatorConnectionState, SimulatorMemoryBudget, SimulatorTaskOutcome,
    arbitrate_handoff, connect_coordinator, decode_crossed_region, decode_event_teleport_finish, decode_teleport_finish,
    handoff_cancellation, is_teleport_cancel, neighbor_completion_event, neighbor_event_queue_failure, neighbor_event_queue_task_failure,
    remove_failed_neighbor, retry_pending_handoff_commands, same_handoff_destination, teleport_failure_matches,
};
use crate::simulator::coordinator::capability_events::redact_seed_capability;
use crate::simulator::coordinator::handoff::{matches_handoff_cancellation, matches_pending_promotion};
use crate::{
    AgentSession, Grid,
    capabilities::EventQueueExit,
    event::{
        Event,
        PacketOrder::{Ambiguous, Degraded, Ordered},
        SimulatorId,
    },
};

fn crossed_region(agent_id: Uuid, session_id: Uuid) -> Value {
    Value::Map(Map::from([
        (
            "AgentData".to_owned(),
            Value::Array(vec![Value::Map(Map::from([
                ("AgentID".to_owned(), Value::Uuid(agent_id)),
                ("SessionID".to_owned(), Value::Uuid(session_id)),
            ]))]),
        ),
        (
            "RegionData".to_owned(),
            Value::Array(vec![Value::Map(Map::from([
                ("RegionHandle".to_owned(), Value::Binary(0x1234_u64.to_be_bytes().to_vec().into())),
                ("SimIP".to_owned(), Value::Binary(vec![192, 0, 2, 4].into())),
                ("SimPort".to_owned(), Value::Integer(13_000)),
                ("SeedCapability".to_owned(), Value::Uri("https://example.test/seed".into())),
            ]))]),
        ),
        ("Info".to_owned(), Value::Array(vec![Value::Map(Map::new())])),
    ]))
}

fn teleport_finish_event(agent_id: Uuid, seed: &str) -> Value {
    Value::Map(Map::from([(
        "Info".to_owned(),
        Value::Array(vec![Value::Map(Map::from([
            ("AgentID".to_owned(), Value::Uuid(agent_id)),
            ("RegionHandle".to_owned(), Value::Binary(0x1234_u64.to_be_bytes().to_vec().into())),
            ("SeedCapability".to_owned(), Value::String(seed.to_owned())),
            ("SimIP".to_owned(), Value::Binary(vec![192, 0, 2, 4].into())),
            ("SimPort".to_owned(), Value::Integer(13_000)),
        ]))]),
    )]))
}

fn teleport_finish(agent_id: Uuid, seed: &str) -> Message {
    Message::TeleportFinish(TeleportFinish {
        agent_id,
        location_id: 0,
        sim_ip: [192, 0, 2, 4],
        sim_port: 13_000_u16.to_be_bytes(),
        region_handle: 0x1234,
        seed_capability: seed.as_bytes().to_vec(),
        sim_access: Some(0),
        teleport_flags: Some(0),
    })
}

fn handoff(region_handle: u64, endpoint: &str, seed: &str) -> HandoffTarget {
    HandoffTarget {
        agent_id: Uuid::from_u128(1),
        session_id: Uuid::from_u128(2),
        region_handle,
        endpoint: endpoint.parse().expect("simulator endpoint"),
        seed_capability: Url::parse(seed).expect("seed URL"),
    }
}

fn generation(value: u64) -> HandoffGeneration {
    HandoffGeneration::new(NonZeroU64::new(value).expect("nonzero generation"))
}

#[test]
fn crossed_region_is_bound_to_session() {
    let agent_id = Uuid::from_u128(1);
    let session_id = Uuid::from_u128(2);
    let handoff = decode_crossed_region(&crossed_region(agent_id, session_id), agent_id, session_id).expect("valid crossing");
    assert_eq!(handoff.region_handle, 0x1234);
    assert_eq!(handoff.endpoint, "192.0.2.4:13000".parse().expect("simulator endpoint"));
    assert!(decode_crossed_region(&crossed_region(Uuid::from_u128(3), session_id), agent_id, session_id).is_err());
    assert!(decode_crossed_region(&crossed_region(agent_id, Uuid::from_u128(4)), agent_id, session_id).is_err());

    assert!(decode_crossed_region(&Value::Map(Map::new()), agent_id, session_id).is_err());
}

#[test]
fn teleport_finish_has_checked_destination() {
    let agent_id = Uuid::from_u128(1);
    let session_id = Uuid::from_u128(2);
    let handoff = decode_teleport_finish(&teleport_finish(agent_id, "https://example.test/seed\0"), agent_id, session_id)
        .expect("valid teleport finish");
    assert_eq!(handoff.region_handle, 0x1234);
    assert!(decode_teleport_finish(&teleport_finish(Uuid::from_u128(3), "https://example.test/seed"), agent_id, session_id).is_err());
    assert!(decode_teleport_finish(&teleport_finish(agent_id, "file:///seed"), agent_id, session_id).is_err());
}

#[test]
fn event_queue_teleport_finish_decodes_real_llsd_shape() {
    let agent_id = Uuid::from_u128(1);
    let session_id = Uuid::from_u128(2);
    let body = teleport_finish_event(agent_id, "https://example.test/seed");
    let handoff = decode_event_teleport_finish(&body, agent_id, session_id).expect("valid EventQueue TeleportFinish");
    assert_eq!(handoff.region_handle, 0x1234);
    assert_eq!(handoff.endpoint, "192.0.2.4:13000".parse().expect("simulator endpoint"));
    assert!(decode_event_teleport_finish(&body, Uuid::from_u128(3), session_id).is_err());
    assert!(decode_event_teleport_finish(&teleport_finish_event(agent_id, "file:///seed"), agent_id, session_id).is_err());
}

#[test]
fn promotion_requires_current_matching_destination() {
    let source = SimulatorId::new(1);
    let target = SimulatorId::new(2);
    let handoff = handoff(0x1234, "192.0.2.4:13000", "https://example.test/seed");
    let pending = PendingPromotion { source, target, generation: generation(1), handoff, evidence: HandoffEvidence::default() };
    let movement = AgentMovementComplete {
        agent_data: AgentData { agent_id: Uuid::from_u128(1), session_id: Uuid::from_u128(2) },
        data: Data { region_handle: 0x1234, ..Default::default() },
        sim_data: SimData::default(),
    };

    assert!(matches_pending_promotion(Some(&pending), source, target, &movement));
    assert!(!matches_pending_promotion(Some(&pending), SimulatorId::new(3), target, &movement));
    assert!(!matches_pending_promotion(Some(&pending), source, SimulatorId::new(3), &movement));
    let mut stale = movement.clone();
    stale.data.region_handle = 0x5678;
    assert!(!matches_pending_promotion(Some(&pending), source, target, &stale));
    let mut wrong_agent = movement;
    wrong_agent.agent_data.session_id = Uuid::from_u128(4);
    assert!(!matches_pending_promotion(Some(&pending), source, target, &wrong_agent));
    assert!(!matches_pending_promotion(None, source, target, &wrong_agent));
}

#[test]
fn cancel_matches_handoff_generation() {
    let pending = PendingPromotion {
        source: SimulatorId::new(1),
        target: SimulatorId::new(2),
        generation: generation(1),
        handoff: handoff(0x1234, "192.0.2.4:13000", "https://example.test/seed"),
        evidence: HandoffEvidence::default(),
    };
    let cancel = handoff_cancellation(&pending);
    let newer = PendingPromotion { generation: generation(2), ..pending.clone() };
    let mut state = HandoffState::default();
    state.set_pending(pending.clone());
    let mut newer_state = HandoffState::default();
    newer_state.set_pending(newer.clone());

    assert_eq!(newer.target, pending.target);
    assert!(matches_handoff_cancellation(&state, cancel));
    assert!(!matches_handoff_cancellation(&newer_state, cancel));
    state.conflict(&pending);
    assert!(matches_handoff_cancellation(&state, cancel));
    assert!(state.clear_conflict(cancel));
    assert!(!matches_handoff_cancellation(&state, cancel));
    assert!(!newer_state.clear_conflict(cancel), "a stale cancellation cannot clear a newer handoff");
}

#[tokio::test(start_paused = true)]
async fn full_handoff_queue_recovers() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let mut connection = connect_coordinator(
        AgentSession {
            first_name: "Test".into(),
            last_name: "Resident".into(),
            grid: Grid::Agni,
            message: "Welcome".into(),
            simulator_address: simulator.local_addr().expect("simulator address"),
            agent_id: Uuid::from_u128(1),
            session_id: Uuid::from_u128(2),
            circuit_code: 1,
            seed_capability: Url::parse("http://127.0.0.1:1/seed").expect("seed URL"),
        },
        ResponseData::default(),
    )
    .await
    .expect("connect primary circuit");
    let origin = connection.simulator_id;
    let target = SimulatorId::allocate().expect("allocate neighbor simulator ID");
    let endpoint = "127.0.0.1:13001".parse::<SocketAddr>().expect("neighbor endpoint");
    let mut run_state = CoordinatorRunState::new();
    let abort = run_state.neighbors.spawn(pending::<(SimulatorId, Option<u64>, SocketAddr, SimulatorTaskOutcome)>());
    let (commands, mut command_receiver) = mpsc::channel(NEIGHBOR_COMMAND_CAPACITY);
    for _ in 0..NEIGHBOR_COMMAND_CAPACITY {
        assert!(commands.try_send(SimulatorCommand::CancelHandoff).is_ok());
    }
    run_state.neighbor_connections.insert(
        0x1234,
        SimulatorConnectionState {
            simulator_id: target,
            endpoint,
            abort,
            commands,
            logout: mpsc::channel(1).0,
            pending_handoff_command: None,
            primary_state: None,
            packet_memory: SimulatorMemoryBudget::new().for_neighbor(),
        },
    );
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(1);
    let (application_events, _application_event_receiver) = mpsc::channel(8);
    let (neighbor_events, _neighbor_event_receiver) = mpsc::channel(8);
    let (_shutdown_signal, shutdown) = watch::channel(false);
    let (capability_events, _capability_event_receiver) = mpsc::channel(1);
    let mut setup =
        PromotionSetup { state: &mut run_state, neighbor_requests, application_events, neighbor_events, shutdown, capability_events };

    let accepted = timeout(
        Duration::from_millis(100),
        connection.start_neighbor(
            origin,
            0x1234,
            endpoint,
            Some(handoff(0x1234, "127.0.0.1:13001", "http://127.0.0.1:1/seed")),
            &mut setup,
        ),
    )
    .await
    .expect("full connection queue must not stall coordinator")
    .expect("queue saturation is recoverable");

    assert!(accepted, "a full queue should defer handoff preparation instead of losing it");
    assert_eq!(setup.state.neighbor_connections.get(&0x1234).expect("neighbor remains tracked").commands.capacity(), 0);
    assert!(setup.state.neighbor_connections.get(&0x1234).expect("neighbor remains tracked").pending_handoff_command.is_some());

    let mut retry = time::interval(HANDOFF_COMMAND_RETRY_INTERVAL);
    retry.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    retry_pending_handoff_commands(&mut retry, &mut setup.state.neighbor_connections).await;
    assert!(setup.state.neighbor_connections.get(&0x1234).expect("neighbor remains tracked").pending_handoff_command.is_some());

    command_receiver.recv().await.expect("drain one queued command");
    let before_retry = HANDOFF_COMMAND_RETRY_INTERVAL - Duration::from_millis(1);
    assert!(
        timeout(before_retry, retry_pending_handoff_commands(&mut retry, &mut setup.state.neighbor_connections)).await.is_err(),
        "deferred handoff should remain pending until its retry tick"
    );
    assert!(setup.state.neighbor_connections.get(&0x1234).expect("neighbor remains tracked").pending_handoff_command.is_some());
    time::advance(Duration::from_millis(1)).await;
    retry_pending_handoff_commands(&mut retry, &mut setup.state.neighbor_connections).await;
    assert!(setup.state.neighbor_connections.get(&0x1234).expect("neighbor remains tracked").pending_handoff_command.is_none());
    let mut prepare_sent = false;
    while let Ok(command) = command_receiver.try_recv() {
        if matches!(command, SimulatorCommand::PrepareHandoff(_)) {
            prepare_sent = true;
        }
    }
    assert!(prepare_sent, "the deferred handoff command should enter the queue when capacity returns");
    setup.state.neighbors.shutdown().await;
}

#[test]
fn handoff_duplicates_ignore_seed_changes() {
    let current = handoff(0x1234, "192.0.2.4:13000", "https://example.test/old-seed");
    let announced = handoff(0x1234, "192.0.2.4:13000", "https://example.test/new-seed");
    let pending = PendingPromotion {
        source: SimulatorId::new(1),
        target: SimulatorId::new(2),
        generation: generation(1),
        handoff: current.clone(),
        evidence: HandoffEvidence { crossed_region: false, teleport_finish: Some(FinishOrder::Packet(8)) },
    };

    assert!(same_handoff_destination(&current, &announced));
    assert_eq!(arbitrate_handoff(&pending, &announced, HandoffAnnouncement::CrossedRegion), HandoffDecision::SameDestination);
}

#[test]
fn contradictory_handoffs_need_same_circuit_order() {
    let current = PendingPromotion {
        source: SimulatorId::new(1),
        target: SimulatorId::new(2),
        generation: generation(1),
        handoff: handoff(0x1234, "192.0.2.4:13000", "https://example.test/seed"),
        evidence: HandoffEvidence { crossed_region: false, teleport_finish: Some(FinishOrder::Packet(8)) },
    };
    let other = handoff(0x5678, "192.0.2.5:13000", "https://example.test/other");

    assert_eq!(
        arbitrate_handoff(&current, &other, HandoffAnnouncement::TeleportFinish(HandoffOrder::Packet(Ordered(9)))),
        HandoffDecision::ReplaceNewer
    );
    assert_eq!(
        arbitrate_handoff(&current, &other, HandoffAnnouncement::TeleportFinish(HandoffOrder::Packet(Ordered(7)))),
        HandoffDecision::IgnoreStale
    );
    assert_eq!(
        arbitrate_handoff(&current, &other, HandoffAnnouncement::TeleportFinish(HandoffOrder::Packet(Ordered(8)))),
        HandoffDecision::Conflict
    );
    assert_eq!(
        arbitrate_handoff(&current, &other, HandoffAnnouncement::TeleportFinish(HandoffOrder::Packet(Ambiguous))),
        HandoffDecision::Conflict
    );
    assert_eq!(arbitrate_handoff(&current, &other, HandoffAnnouncement::CrossedRegion), HandoffDecision::Conflict);

    let crossed = PendingPromotion { evidence: HandoffEvidence { crossed_region: true, ..current.evidence }, ..current.clone() };
    assert_eq!(
        arbitrate_handoff(&crossed, &other, HandoffAnnouncement::TeleportFinish(HandoffOrder::Packet(Ordered(9)))),
        HandoffDecision::Conflict
    );

    let queue_current = PendingPromotion {
        evidence: HandoffEvidence {
            crossed_region: false,
            teleport_finish: Some(FinishOrder::EventQueue { incarnation: 1, batch: 4, index: 2 }),
        },
        ..current.clone()
    };
    assert_eq!(
        arbitrate_handoff(
            &queue_current,
            &other,
            HandoffAnnouncement::TeleportFinish(HandoffOrder::EventQueue { incarnation: 1, batch: 5, index: 0 }),
        ),
        HandoffDecision::ReplaceNewer
    );
    assert_eq!(
        arbitrate_handoff(
            &queue_current,
            &other,
            HandoffAnnouncement::TeleportFinish(HandoffOrder::EventQueue { incarnation: 1, batch: 4, index: 1 }),
        ),
        HandoffDecision::IgnoreStale
    );
    assert_eq!(
        arbitrate_handoff(
            &queue_current,
            &other,
            HandoffAnnouncement::TeleportFinish(HandoffOrder::EventQueue { incarnation: 2, batch: 5, index: 0 }),
        ),
        HandoffDecision::Conflict,
        "a restarted EventQueue has a different ordering domain",
    );
    let current_after_signed_wire_id_range = PendingPromotion {
        evidence: HandoffEvidence {
            crossed_region: false,
            teleport_finish: Some(FinishOrder::EventQueue { incarnation: 1, batch: i32::MAX as u64, index: 1 }),
        },
        ..current
    };
    assert_eq!(
        arbitrate_handoff(
            &current_after_signed_wire_id_range,
            &other,
            HandoffAnnouncement::TeleportFinish(HandoffOrder::EventQueue { incarnation: 1, batch: i32::MAX as u64 + 1, index: 0 }),
        ),
        HandoffDecision::ReplaceNewer,
        "local batch order does not inherit the signed wire ID range",
    );
    assert_eq!(
        arbitrate_handoff(&queue_current, &other, HandoffAnnouncement::TeleportFinish(HandoffOrder::Packet(Ordered(9)))),
        HandoffDecision::Conflict,
        "order from different transports cannot be compared",
    );
}

#[test]
fn teleport_failure_needs_newer_local_order() {
    let pending = PendingPromotion {
        source: SimulatorId::new(1),
        target: SimulatorId::new(2),
        generation: generation(1),
        handoff: handoff(0x1234, "192.0.2.4:13000", "https://example.test/seed"),
        evidence: HandoffEvidence { crossed_region: false, teleport_finish: Some(FinishOrder::Packet(8)) },
    };

    assert!(teleport_failure_matches(Some(&pending), HandoffOrder::Packet(Ordered(9))));
    assert!(!teleport_failure_matches(Some(&pending), HandoffOrder::Packet(Ordered(8))));
    assert!(!teleport_failure_matches(Some(&pending), HandoffOrder::Packet(Ordered(7))));
    assert!(!teleport_failure_matches(Some(&pending), HandoffOrder::Packet(Ambiguous)));
    assert!(!teleport_failure_matches(Some(&pending), HandoffOrder::Packet(Degraded)));

    let crossed = PendingPromotion { evidence: HandoffEvidence { crossed_region: true, ..pending.evidence }, ..pending.clone() };
    assert!(!teleport_failure_matches(Some(&crossed), HandoffOrder::Packet(Ordered(9))));
    let unusable =
        PendingPromotion { evidence: HandoffEvidence { crossed_region: false, teleport_finish: Some(FinishOrder::Unusable) }, ..pending };
    assert!(!teleport_failure_matches(Some(&unusable), HandoffOrder::Packet(Ordered(9))));

    let event_queue = PendingPromotion {
        evidence: HandoffEvidence {
            crossed_region: false,
            teleport_finish: Some(FinishOrder::EventQueue { incarnation: 1, batch: 4, index: 2 }),
        },
        ..unusable
    };
    assert!(teleport_failure_matches(Some(&event_queue), HandoffOrder::EventQueue { incarnation: 1, batch: 4, index: 3 }));
    assert!(!teleport_failure_matches(Some(&event_queue), HandoffOrder::EventQueue { incarnation: 1, batch: 4, index: 2 }));
    assert!(!teleport_failure_matches(Some(&event_queue), HandoffOrder::EventQueue { incarnation: 2, batch: 4, index: 3 }));
    assert!(!teleport_failure_matches(Some(&event_queue), HandoffOrder::Packet(Ordered(99))));
}

#[test]
fn teleport_cancel_and_crossing_seed_are_redacted() {
    let message = Message::TeleportCancel(TeleportCancel { agent_id: Uuid::from_u128(1), session_id: Uuid::from_u128(2) });
    assert!(is_teleport_cancel(&message, Uuid::from_u128(1), Uuid::from_u128(2)));
    assert!(!is_teleport_cancel(&message, Uuid::from_u128(1), Uuid::from_u128(3)));

    let mut redacted = crossed_region(Uuid::from_u128(1), Uuid::from_u128(2));
    redact_seed_capability(&mut redacted);
    let fields = crate::simulator::coordinator::handoff::event_fields(&redacted).expect("crossed region event");
    let region = crate::simulator::coordinator::handoff::event_block(fields, "RegionData").expect("region data block");
    assert_eq!(region.get("SeedCapability"), Some(&Value::String("[redacted]".into())));

    let mut finish = teleport_finish_event(Uuid::from_u128(1), "https://sim.invalid/private");
    redact_seed_capability(&mut finish);
    let fields = crate::simulator::coordinator::handoff::event_fields(&finish).expect("teleport finish event");
    let info = crate::simulator::coordinator::handoff::event_block(fields, "Info").expect("teleport finish info");
    assert_eq!(info.get("SeedCapability"), Some(&Value::String("[redacted]".into())));
}

#[test]
fn neighbor_overload_reports_failure() {
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let region_handle = 0x1000_0000_0000_1000;
    let endpoint = "127.0.0.1:9000".parse().expect("parse endpoint");

    assert!(matches!(
        neighbor_completion_event(
            simulator_id,
            Some(region_handle),
            endpoint,
            &SimulatorTaskOutcome::Finished(Err(EVENT_BACKLOG_DISCONNECT_REASON.into())),
        ),
        Event::SimulatorFailed {
            simulator_id: failed_id,
            region_handle: Some(failed_region),
            endpoint: failed_endpoint,
            reason,
        } if failed_id == simulator_id
            && failed_region == region_handle
            && failed_endpoint == endpoint
            && reason == EVENT_BACKLOG_DISCONNECT_REASON
    ));
}

#[tokio::test]
async fn event_queue_panic_reaches_simulator() {
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let primary_id = SimulatorId::allocate().expect("allocate primary ID");
    let source = "127.0.0.1:9000".parse::<SocketAddr>().expect("parse endpoint");
    let mut neighbors = JoinSet::<()>::new();
    let abort = neighbors.spawn(pending::<()>());
    let (commands, _) = mpsc::channel::<SimulatorCommand>(1);
    let connections = HashMap::from([(
        1,
        SimulatorConnectionState {
            simulator_id,
            endpoint: source,
            abort,
            commands,
            logout: mpsc::channel(1).0,
            pending_handoff_command: None,
            primary_state: None,
            packet_memory: SimulatorMemoryBudget::new().for_neighbor(),
        },
    )]);

    let mut event_queues = JoinSet::<(SimulatorId, SocketAddr, EventQueueExit)>::new();
    let task = event_queues.spawn(async { panic!("injected EventQueue worker panic") });
    let mut owners = HashMap::from([(task.id(), (simulator_id, source))]);
    let Some(Err(error)) = event_queues.join_next().await else {
        panic!("injected worker should fail");
    };
    assert!(error.is_panic());

    assert!(matches!(
        neighbor_event_queue_task_failure(&error, &mut owners, primary_id, &connections),
        Some(Event::CapabilityError { simulator_id: failed_id, source: failed_source, .. })
            if failed_id == simulator_id && failed_source == source
    ));
    assert!(owners.is_empty());
    neighbors.abort_all();
}

#[tokio::test]
async fn stale_event_queue_completion_is_ignored() {
    let stale_id = SimulatorId::allocate().expect("allocate stale simulator ID");
    let current_id = SimulatorId::allocate().expect("allocate replacement simulator ID");
    let primary_id = SimulatorId::allocate().expect("allocate primary ID");
    let source = "127.0.0.1:9001".parse::<SocketAddr>().expect("parse endpoint");
    let mut neighbors = JoinSet::<()>::new();
    let (commands, _) = mpsc::channel::<SimulatorCommand>(1);
    let mut connections = HashMap::from([(
        1,
        SimulatorConnectionState {
            simulator_id: stale_id,
            endpoint: source,
            abort: neighbors.spawn(pending::<()>()),
            commands,
            logout: mpsc::channel(1).0,
            pending_handoff_command: None,
            primary_state: None,
            packet_memory: SimulatorMemoryBudget::new().for_neighbor(),
        },
    )]);

    let mut event_queues = JoinSet::<(SimulatorId, SocketAddr, EventQueueExit)>::new();
    let task = event_queues.spawn(async { panic!("injected stale EventQueue worker panic") });
    let mut owners = HashMap::from([(task.id(), (stale_id, source))]);
    let Some(Err(error)) = event_queues.join_next().await else {
        panic!("injected worker should fail");
    };
    assert!(error.is_panic());

    let (commands, _) = mpsc::channel::<SimulatorCommand>(1);
    let replacement = SimulatorConnectionState {
        simulator_id: current_id,
        endpoint: source,
        abort: neighbors.spawn(pending::<()>()),
        commands,
        logout: mpsc::channel(1).0,
        pending_handoff_command: None,
        primary_state: None,
        packet_memory: SimulatorMemoryBudget::new().for_neighbor(),
    };
    if let Some(stale) = connections.insert(1, replacement) {
        stale.abort.abort();
    }

    assert!(neighbor_event_queue_task_failure(&error, &mut owners, primary_id, &connections).is_none());
    assert!(neighbor_event_queue_failure(stale_id, source, EventQueueExit::TaskFailed, primary_id, &connections).is_none());
    assert!(owners.is_empty());
    neighbors.abort_all();
}

#[tokio::test]
async fn neighbor_panic_removes_only_its_generation() {
    let failed_id = SimulatorId::allocate().expect("allocate failed simulator ID");
    let other_id = SimulatorId::allocate().expect("allocate other simulator ID");
    let failed_endpoint = "127.0.0.1:9002".parse::<SocketAddr>().expect("parse failed endpoint");
    let other_endpoint = "127.0.0.1:9003".parse::<SocketAddr>().expect("parse other endpoint");
    let mut tasks = JoinSet::<()>::new();
    let failed_task = tasks.spawn(async { panic!("injected neighbor worker panic") });
    let other_task = tasks.spawn(pending::<()>());
    let (failed_commands, _) = mpsc::channel::<SimulatorCommand>(1);
    let (other_commands, _) = mpsc::channel::<SimulatorCommand>(1);
    let mut connections = HashMap::from([
        (
            1,
            SimulatorConnectionState {
                simulator_id: failed_id,
                endpoint: failed_endpoint,
                abort: failed_task,
                commands: failed_commands,
                logout: mpsc::channel(1).0,
                pending_handoff_command: None,
                primary_state: None,
                packet_memory: SimulatorMemoryBudget::new().for_neighbor(),
            },
        ),
        (
            2,
            SimulatorConnectionState {
                simulator_id: other_id,
                endpoint: other_endpoint,
                abort: other_task,
                commands: other_commands,
                logout: mpsc::channel(1).0,
                pending_handoff_command: None,
                primary_state: None,
                packet_memory: SimulatorMemoryBudget::new().for_neighbor(),
            },
        ),
    ]);

    let Some(Err(error)) = tasks.join_next().await else {
        panic!("injected neighbor worker should panic");
    };
    assert!(error.is_panic());
    assert_eq!(remove_failed_neighbor(&error, &mut connections), Some((1, failed_id, failed_endpoint)));
    assert!(!connections.contains_key(&1));
    assert!(connections.get(&2).is_some_and(|connection| connection.simulator_id == other_id));

    let stale_id = SimulatorId::allocate().expect("allocate stale simulator ID");
    let replacement_id = SimulatorId::allocate().expect("allocate replacement simulator ID");
    let stale_endpoint = "127.0.0.1:9004".parse::<SocketAddr>().expect("parse stale endpoint");
    let stale_task = tasks.spawn(async { panic!("injected late neighbor worker panic") });
    let (commands, _) = mpsc::channel::<SimulatorCommand>(1);
    connections.insert(
        3,
        SimulatorConnectionState {
            simulator_id: stale_id,
            endpoint: stale_endpoint,
            abort: stale_task,
            commands,
            logout: mpsc::channel(1).0,
            pending_handoff_command: None,
            primary_state: None,
            packet_memory: SimulatorMemoryBudget::new().for_neighbor(),
        },
    );
    let current_task = tasks.spawn(pending::<()>());
    let (commands, _) = mpsc::channel::<SimulatorCommand>(1);
    connections.insert(
        3,
        SimulatorConnectionState {
            simulator_id: replacement_id,
            endpoint: stale_endpoint,
            abort: current_task,
            commands,
            logout: mpsc::channel(1).0,
            pending_handoff_command: None,
            primary_state: None,
            packet_memory: SimulatorMemoryBudget::new().for_neighbor(),
        },
    );

    let Some(Err(error)) = tasks.join_next().await else {
        panic!("injected stale worker should panic");
    };
    assert!(error.is_panic());
    assert_eq!(remove_failed_neighbor(&error, &mut connections), None);
    assert!(connections.get(&3).is_some_and(|connection| connection.simulator_id == replacement_id));
    assert_eq!(connections.len(), 2);
    tasks.abort_all();
}
