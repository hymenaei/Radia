use network::OutboundMessage;
use protocol::messages::{AgentUpdate, ChatFromViewer, MapBlockRequest, RegionHandleRequest, RequestMultipleObjects, TeleportCancel};

#[test]
fn supported_messages_implement_the_send_contract() {
    fn outbound<T: OutboundMessage>() {}

    outbound::<AgentUpdate>();
    outbound::<ChatFromViewer>();
    outbound::<MapBlockRequest>();
    outbound::<RegionHandleRequest>();
    outbound::<RequestMultipleObjects>();
    outbound::<TeleportCancel>();
}

#[cfg(feature = "testing")]
mod simulator_send {
    use std::{net::SocketAddr, str::from_utf8};

    use network::{Delivery, Event, Grid, LoginCredentials, StartLocation};
    use protocol::{
        ChatType,
        lludp::{Message, Packet},
        messages::{ChatFromViewer, PacketAck, chat::chat_from_viewer, circuit::packet_ack::Packets},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream, UdpSocket},
        spawn,
        sync::watch,
    };
    use uuid::Uuid;

    use ::test_support::lludp::fixture;

    const AGENT_ID: &str = "ffeeddcc-bbaa-9988-7766-554433221100";
    const SESSION_ID: &str = "00112233-4455-6677-8899-aabbccddeeff";

    #[tokio::test(flavor = "multi_thread")]
    async fn session_handle_send_reaches_the_simulator() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
        let (login_url, stop_http, http_server) = fake_login_server(simulator.local_addr().expect("simulator address").port()).await;
        let credentials = LoginCredentials {
            first_name: "Test".into(),
            last_name: "Resident".into(),
            password: "fixture-password".into(),
            grid: Grid::Aditi,
            start_location: StartLocation::Last,
            mfa_hash: String::new(),
        };
        let mut session = network::testing::start_session_at(credentials, login_url).expect("start network session");
        let handle = session.handle();

        let (use_circuit, peer) = receive_packet(&simulator).await;
        std::assert_matches!(use_circuit.message(), Message::UseCircuitCode(_));
        acknowledge(&simulator, peer, use_circuit.sequence()).await;

        let (movement, movement_peer) = receive_packet(&simulator).await;
        assert_eq!(movement_peer, peer);
        std::assert_matches!(movement.message(), Message::CompleteAgentMovement(_));
        acknowledge(&simulator, peer, movement.sequence()).await;

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

        loop {
            let (packet, packet_peer) = receive_packet(&simulator).await;
            assert_eq!(packet_peer, peer);
            if let Message::ChatFromViewer(message) = packet.message() {
                assert_eq!(message.chat_data.message, "outbound test");
                assert!(packet.is_reliable());
                acknowledge(&simulator, peer, packet.sequence()).await;
                break;
            }
            if packet.is_reliable() {
                acknowledge(&simulator, peer, packet.sequence()).await;
            }
        }

        let disconnecting = handle.clone();
        let disconnect = spawn(async move { disconnecting.disconnect().await });
        loop {
            let (packet, packet_peer) = receive_packet(&simulator).await;
            assert_eq!(packet_peer, peer);
            if packet.is_reliable() {
                acknowledge(&simulator, peer, packet.sequence()).await;
            }
            if matches!(packet.message(), Message::LogoutRequest(_)) {
                break;
            }
        }
        disconnect.await.expect("disconnect task");
        stop_http.send_replace(true);
        http_server.await.expect("fake login server");
        drop(session);
    }

    async fn fake_login_server(simulator_port: u16) -> (String, watch::Sender<bool>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake login server");
        let address = listener.local_addr().expect("fake login address");
        let seed_url = format!("http://{address}/seed");
        let login_body = format!(
            concat!(
                "<?xml version=\"1.0\"?><methodResponse><params><param><value><struct>",
                "<member><name>login</name><value><boolean>1</boolean></value></member>",
                "<member><name>first_name</name><value><string>Test</string></value></member>",
                "<member><name>last_name</name><value><string>Resident</string></value></member>",
                "<member><name>message</name><value><string>Welcome</string></value></member>",
                "<member><name>sim_ip</name><value><string>127.0.0.1</string></value></member>",
                "<member><name>sim_port</name><value><int>{simulator_port}</int></value></member>",
                "<member><name>circuit_code</name><value><int>287454020</int></value></member>",
                "<member><name>agent_id</name><value><string>{AGENT_ID}</string></value></member>",
                "<member><name>session_id</name><value><string>{SESSION_ID}</string></value></member>",
                "<member><name>seed_capability</name><value><string>{seed_url}</string></value></member>",
                "</struct></value></param></params></methodResponse>"
            ),
            simulator_port = simulator_port,
            seed_url = seed_url,
            AGENT_ID = AGENT_ID,
            SESSION_ID = SESSION_ID,
        );
        let (stop, mut stop_requested) = watch::channel(false);
        let server = spawn(async move {
            while !*stop_requested.borrow() {
                let accepted = tokio::select! {
                    biased;
                    changed = stop_requested.changed() => {
                        if changed.is_err() || *stop_requested.borrow() { break; }
                        continue;
                    }
                    accepted = listener.accept() => accepted.expect("accept fake HTTP request"),
                };
                let (mut stream, _) = accepted;
                let request = read_request(&mut stream).await;
                let request_line = from_utf8(&request).expect("HTTP request is UTF-8").split("\r\n").next().expect("request line");
                let (content_type, body) = if request_line.starts_with("POST /login.cgi ") {
                    ("text/xml", login_body.as_str())
                } else {
                    assert!(request_line.starts_with("POST /seed "), "unexpected request: {request_line}");
                    ("application/llsd+xml", "<llsd><map></map></llsd>")
                };
                write_response(&mut stream, content_type, body).await;
            }
        });
        (format!("http://{address}/login.cgi"), stop, server)
    }

    async fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut chunk = [0; 2048];
        loop {
            let size = stream.read(&mut chunk).await.expect("read HTTP request");
            assert_ne!(size, 0, "HTTP connection closed before the request completed");
            request.extend_from_slice(&chunk[..size]);
            let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let headers = from_utf8(&request[..header_end]).expect("HTTP headers are UTF-8");
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                return request;
            }
        }
    }

    async fn write_response(stream: &mut TcpStream, content_type: &str, body: &str) {
        let response =
            format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        stream.write_all(response.as_bytes()).await.expect("write HTTP response");
    }

    async fn receive_packet(socket: &UdpSocket) -> (Packet, SocketAddr) {
        let mut bytes = [0; 65_535];
        let (size, peer) = socket.recv_from(&mut bytes).await.expect("receive simulator datagram");
        (Packet::decode(&bytes[..size]).expect("decode simulator packet"), peer)
    }

    async fn acknowledge(socket: &UdpSocket, peer: SocketAddr, sequence: u32) {
        let packet = Packet::new(100, Message::PacketAck(PacketAck { packets: vec![Packets { id: sequence }] }));
        socket.send_to(&packet.encode().expect("encode packet ACK"), peer).await.expect("send packet ACK");
    }

    async fn send_fixture(socket: &UdpSocket, peer: SocketAddr, name: &str, sequence: u32) {
        let mut bytes = fixture(name);
        bytes[0] |= 0x40;
        bytes[1..5].copy_from_slice(&sequence.to_be_bytes());
        socket.send_to(&bytes, peer).await.expect("send simulator fixture");
    }
}
