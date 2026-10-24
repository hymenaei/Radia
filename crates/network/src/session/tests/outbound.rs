use std::net::SocketAddr;

use protocol::{
    ChatType,
    lludp::{Message, Packet},
    messages::{ChatFromViewer, chat::chat_from_viewer},
};
use tokio::{net::UdpSocket, spawn};
use uuid::Uuid;

use super::*;
use crate::fixtures::{
    http::{EventQueueTestServer, EventServerResponse, llsd_response, response},
    session::{login_xml, receive_client_message, receive_udp, server_ack, set_sequence, test_fixture},
};

const AGENT_ID: &str = "ffeeddcc-bbaa-9988-7766-554433221100";
const SESSION_ID: &str = "00112233-4455-6677-8899-aabbccddeeff";

fn fake_login_server(simulator_port: u16) -> (String, EventQueueTestServer) {
    let server = EventQueueTestServer::start(|base| {
        let seed_url = base.join("seed").expect("seed URL");
        let login_body = login_xml(simulator_port, seed_url.as_str());
        vec![
            EventServerResponse::Response(response(200, "OK", "text/xml", None, &login_body)),
            EventServerResponse::Response(llsd_response("<llsd><map></map></llsd>")),
        ]
    });
    (server.url("/login.cgi").to_string(), server)
}

async fn send_fixture(socket: &UdpSocket, peer: SocketAddr, name: &str, sequence: u32) {
    let mut bytes = test_fixture(name);
    bytes[0] |= 0x40;
    set_sequence(&mut bytes, sequence);
    socket.send_to(&bytes, peer).await.expect("send simulator fixture");
}

#[tokio::test(flavor = "multi_thread")]
async fn session_handle_send_reaches_the_simulator() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let (login_url, http_server) = fake_login_server(simulator.local_addr().expect("simulator address").port());
    let credentials = LoginCredentials {
        first_name: "Test".into(),
        last_name: "Resident".into(),
        password: "fixture-password".into(),
        grid: Grid::Aditi,
        start_location: StartLocation::Last,
        mfa_hash: String::new(),
    };
    let mut session = crate::session::thread::start_at(credentials, SessionOptions::default(), login_url).expect("start network session");
    let handle = session.handle();

    let (use_circuit, peer) = receive_udp(&simulator).await;
    let use_circuit = Packet::decode(&use_circuit).expect("decode UseCircuitCode");
    simulator.send_to(&server_ack(100, use_circuit.sequence()), peer).await.expect("acknowledge UseCircuitCode");

    let (_, movement) = receive_client_message(&simulator, peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    simulator.send_to(&server_ack(101, movement.sequence()), peer).await.expect("acknowledge initial movement");

    send_fixture(&simulator, peer, "region_handshake.hex", 20).await;
    send_fixture(&simulator, peer, "agent_movement_complete.hex", 21).await;

    loop {
        match session.recv_event().await {
            Some(Event::SessionReady { .. }) => break,
            Some(Event::Failed(error)) => panic!("session failed before readiness: {error}"),
            Some(_) => {}
            None => panic!("session event stream closed before readiness"),
        }
    }

    let agent_id = Uuid::parse_str(AGENT_ID).expect("valid agent ID");
    let session_id = Uuid::parse_str(SESSION_ID).expect("valid session ID");
    handle
        .send(
            ChatFromViewer {
                agent_data: chat_from_viewer::AgentData { agent_id, session_id },
                chat_data: chat_from_viewer::ChatData { message: "outbound test".into(), chat_type: ChatType::Normal, channel: 0 },
            },
            Delivery::Reliable,
        )
        .expect("queue viewer chat");

    let (_, chat_packet) = receive_client_message(&simulator, peer, |message| matches!(message, Message::ChatFromViewer(_))).await;
    std::assert_matches!(chat_packet.message(), Message::ChatFromViewer(message) if message.chat_data.message == "outbound test");
    assert!(chat_packet.is_reliable());
    simulator.send_to(&server_ack(102, chat_packet.sequence()), peer).await.expect("acknowledge viewer chat");

    let disconnecting = handle.clone();
    let disconnect = spawn(async move { disconnecting.disconnect().await });
    let (_, logout) = receive_client_message(&simulator, peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    simulator.send_to(&server_ack(103, logout.sequence()), peer).await.expect("acknowledge logout");
    disconnect.await.expect("disconnect task");
    assert_eq!(http_server.finish().len(), 2);
    drop(session);
}
