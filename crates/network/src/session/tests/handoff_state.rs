use std::time::Duration;

use protocol::login::ResponseData;
use tokio::{net::UdpSocket, time::timeout};

use super::*;
use crate::{
    Error,
    session::{SessionRuntime, tests::login_session},
};

#[test]
fn evidence_retains_the_newest_packet_finish() {
    let mut evidence = HandoffEvidence::default();
    evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::Packet(PacketOrder::Ordered(10))));
    evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::Packet(PacketOrder::Ordered(8))));
    assert_eq!(evidence.teleport_finish, Some(FinishOrder::Packet(10)));

    evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::Packet(PacketOrder::Ordered(12))));
    assert_eq!(evidence.teleport_finish, Some(FinishOrder::Packet(12)));
}

#[test]
fn evidence_retains_the_newest_event_queue_finish() {
    let mut evidence = HandoffEvidence::default();
    evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::EventQueue { incarnation: 4, batch: 7, index: 3 }));
    evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::EventQueue { incarnation: 4, batch: 7, index: 2 }));
    assert_eq!(evidence.teleport_finish, Some(FinishOrder::EventQueue { incarnation: 4, batch: 7, index: 3 }));

    evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::EventQueue { incarnation: 4, batch: 8, index: 0 }));
    assert_eq!(evidence.teleport_finish, Some(FinishOrder::EventQueue { incarnation: 4, batch: 8, index: 0 }));
}

#[test]
fn evidence_discards_ordering_across_sources() {
    let mut evidence = HandoffEvidence::default();
    evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::Packet(PacketOrder::Ordered(10))));
    evidence.record(HandoffAnnouncement::TeleportFinish(HandoffOrder::EventQueue { incarnation: 4, batch: 7, index: 3 }));
    assert_eq!(evidence.teleport_finish, Some(FinishOrder::Unusable));
}

#[tokio::test]
async fn handoff_capability_requests_are_waited_for_after_retirement() {
    let root = UdpSocket::bind("127.0.0.1:0").await.expect("bind root simulator");
    let destination = UdpSocket::bind("127.0.0.1:0").await.expect("bind destination");
    let session = login_session(root.local_addr().expect("root endpoint"));
    let target = HandoffTarget {
        agent_id: session.agent_id,
        session_id: session.session_id,
        region_handle: 42,
        endpoint: destination.local_addr().expect("destination endpoint"),
        seed_capability: session.seed_capability.clone(),
    };
    let mut connection = SessionRuntime::connect(session, ResponseData::default(), crate::session::tests::test_options())
        .await
        .expect("session startup")
        .expect("primary connection starts");
    let id = SimulatorId::allocate().expect("destination ID");
    let (events, _receiver) = mpsc::channel(4);
    let (_shutdown, shutdown) = watch::channel(false);
    let mut queues = JoinSet::new();
    let mut owners = HashMap::new();
    let mut capabilities = HashMap::new();
    let client = connection
        .ensure_handoff_capabilities(id, &target, events, shutdown, &mut queues, &mut owners, &mut capabilities)
        .expect("start destination capability");
    assert_eq!(connection.capability_request_waiters.len(), 2);
    remove_simulator_capability(&mut capabilities, id);
    assert!(capabilities.is_empty());
    timeout(Duration::from_secs(2), connection.finish_capability_request_waiters())
        .await
        .expect("wait for root and retired destination capability requests");
    assert!(connection.capability_request_waiters.is_empty());
    while timeout(Duration::from_secs(2), queues.join_next()).await.expect("join retired EventQueue").is_some() {}
    std::assert_matches!(client.request_capabilities(&["TestCapability"]).await, Err(Error::CapabilitySessionClosed));
}
