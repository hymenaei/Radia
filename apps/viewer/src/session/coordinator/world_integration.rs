use std::{
    future::Future,
    io::{ErrorKind, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{
        Arc,
        mpsc::{self, RecvTimeoutError, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use base::UUID;
use futures::{FutureExt, channel::oneshot, pin_mut};
use gpui::{ForegroundExecutor, TestAppContext, TestDispatcher};
use network::{Event, Grid, LoginCredentials, NetworkLifecycle, StartLocation};
use protocol::lludp::Message::ObjectUpdate;
use protocol::{
    lludp::{Message, Packet},
    messages::{
        AgentMovementComplete,
        agent_movement_complete::{AgentData as MovementAgentData, Data as MovementData, SimData as MovementSimData},
    },
};
use testing::{lludp::vectors, parse_hex_fixture};
use world::{Kind, ObjectDetailResult, QueryCompleteness, ValueFilter, WorldChange, WorldQuery, WorldQueryBudget};

use super::{PresentationMailbox, drain_session, fixture_packet, write_lock};

enum LocalPeerCommand {
    SendObjectUpdate,
    Stop,
}

struct LocalNetworkPeer {
    login_url: String,
    commands: mpsc::Sender<LocalPeerCommand>,
    completion: oneshot::Receiver<Result<(), String>>,
    worker: Option<thread::JoinHandle<Result<(), String>>>,
}

impl LocalNetworkPeer {
    fn stop(&self) -> Result<(), String> {
        self.commands.send(LocalPeerCommand::Stop).map_err(|error| error.to_string())
    }

    async fn wait<T>(&mut self, deadline: &mut LocalTestDeadline, future: impl Future<Output = T>, phase: &'static str) -> T {
        let future = future.fuse();
        let peer_exit = (&mut self.completion).fuse();
        let expired = (&mut deadline.expired).fuse();
        pin_mut!(future, peer_exit, expired);
        futures::select! {
            value = future => value,
            exit = peer_exit => match exit {
                Ok(Ok(())) => panic!("local network peer exited before {phase}"),
                Ok(Err(error)) => panic!("local network peer failed before {phase}: {error}"),
                Err(_) => panic!("local network peer stopped without reporting a result before {phase}"),
            },
            _ = expired => panic!("VW-200 integration test exceeded its 30-second deadline during {phase}"),
        }
    }

    async fn wait_for_exit(&mut self, deadline: &mut LocalTestDeadline) -> Result<(), String> {
        let completion = (&mut self.completion).fuse();
        let expired = (&mut deadline.expired).fuse();
        pin_mut!(completion, expired);
        let result = futures::select! {
            result = completion => result.map_err(|_| "local network peer stopped without reporting a result".to_owned())?,
            _ = expired => panic!("VW-200 integration test exceeded its 30-second deadline during peer shutdown"),
        };
        let joined =
            self.worker.take().expect("local peer worker is joined once").join().expect("local network peer thread does not panic");
        if joined != result {
            return Err("local network peer completion disagreed with its worker result".into());
        }
        result
    }
}

impl Drop for LocalNetworkPeer {
    fn drop(&mut self) {
        let _ = self.commands.send(LocalPeerCommand::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct LocalTestDeadline {
    expired: oneshot::Receiver<()>,
    cancel_timer: mpsc::Sender<()>,
    timer_worker: Option<thread::JoinHandle<()>>,
}

impl LocalTestDeadline {
    fn start() -> Self {
        let (expired_sender, expired) = oneshot::channel();
        let (cancel_timer, timer_receiver) = mpsc::channel();
        let timer_worker = thread::spawn(move || {
            if matches!(timer_receiver.recv_timeout(Duration::from_secs(30)), Err(RecvTimeoutError::Timeout)) {
                let _ = expired_sender.send(());
            }
        });
        Self { expired, cancel_timer, timer_worker: Some(timer_worker) }
    }
}

impl Drop for LocalTestDeadline {
    fn drop(&mut self) {
        let _ = self.cancel_timer.send(());
        if let Some(timer_worker) = self.timer_worker.take() {
            let _ = timer_worker.join();
        }
    }
}

fn start_local_network_peer() -> LocalNetworkPeer {
    let login_listener = TcpListener::bind("127.0.0.1:0").expect("bind local login server");
    login_listener.set_nonblocking(true).expect("set local login server nonblocking");
    let login_address = login_listener.local_addr().expect("local login address");
    let simulator = UdpSocket::bind("127.0.0.1:0").expect("bind local simulator");
    simulator.set_read_timeout(Some(Duration::from_millis(20))).expect("set simulator read timeout");
    let simulator_address = simulator.local_addr().expect("local simulator address");
    let (commands, command_receiver) = mpsc::channel();
    let (completion_sender, completion) = oneshot::channel();

    let worker = thread::spawn(move || {
        let result = run_local_network_peer(login_listener, login_address, simulator, simulator_address, &command_receiver);
        let _ = completion_sender.send(result.clone());
        result
    });

    LocalNetworkPeer { login_url: format!("http://{login_address}/login.cgi"), commands, completion, worker: Some(worker) }
}

fn run_local_network_peer(
    login_listener: TcpListener,
    login_address: SocketAddr,
    simulator: UdpSocket,
    simulator_address: SocketAddr,
    command_receiver: &mpsc::Receiver<LocalPeerCommand>,
) -> Result<(), String> {
    let (mut login_stream, _) = accept_local_connection(&login_listener, command_receiver, "login request")?;
    login_stream.set_read_timeout(Some(Duration::from_secs(1))).map_err(|error| error.to_string())?;
    let _ = read_local_http_request(&mut login_stream)?;
    let seed_url = format!("http://{login_address}/seed");
    let login_body = format!(
        concat!(
            "<?xml version=\"1.0\"?><methodResponse><params><param><value><struct>",
            "<member><name>login</name><value><boolean>1</boolean></value></member>",
            "<member><name>first_name</name><value><string>Session</string></value></member>",
            "<member><name>last_name</name><value><string>Avatar</string></value></member>",
            "<member><name>message</name><value><string>Welcome</string></value></member>",
            "<member><name>sim_ip</name><value><string>127.0.0.1</string></value></member>",
            "<member><name>sim_port</name><value><int>{sim_port}</int></value></member>",
            "<member><name>circuit_code</name><value><int>287454020</int></value></member>",
            "<member><name>agent_id</name><value><string>ffeeddcc-bbaa-9988-7766-554433221100</string></value></member>",
            "<member><name>session_id</name><value><string>00112233-4455-6677-8899-aabbccddeeff</string></value></member>",
            "<member><name>seed_capability</name><value><string>{seed_url}</string></value></member>",
            "<member><name>mfa_hash</name><value><string>returned-mfa</string></value></member>",
            "<member><name>inventory-skeleton</name><value><array><data><value><struct>",
            "<member><name>folder_id</name><value><string>12345678-1234-5678-1234-567812345678</string></value></member>",
            "<member><name>name</name><value><string>Favorites</string></value></member>",
            "</struct></value></data></array></value></member>",
            "</struct></value></param></params></methodResponse>"
        ),
        sim_port = simulator_address.port(),
        seed_url = seed_url,
    );
    write_local_http_response(&mut login_stream, "text/xml", &login_body)?;

    let (mut seed_stream, _) = accept_local_connection(&login_listener, command_receiver, "seed capability request")?;
    seed_stream.set_read_timeout(Some(Duration::from_secs(1))).map_err(|error| error.to_string())?;
    let _ = read_local_http_request(&mut seed_stream)?;
    write_local_http_response(&mut seed_stream, "application/llsd+xml", "<llsd><map></map></llsd>")?;

    let mut server_sequence = 20;
    let (_, peer) = receive_local_client_message(&simulator, &mut server_sequence, command_receiver, |message| {
        matches!(message, Message::UseCircuitCode(_))
    })?;
    let (_, movement_request_peer) = receive_local_client_message(&simulator, &mut server_sequence, command_receiver, |message| {
        matches!(message, Message::CompleteAgentMovement(_))
    })?;
    if movement_request_peer != peer {
        return Err("network client changed its simulator endpoint during bootstrap".into());
    }

    send_local_packet(&simulator, peer, &mut server_sequence, local_reliable_fixture(vectors::REGION_HANDSHAKE))?;
    let (_, reply_peer) = receive_local_client_message(&simulator, &mut server_sequence, command_receiver, |message| {
        matches!(message, Message::RegionHandshakeReply(_))
    })?;
    if reply_peer != peer {
        return Err("network client changed its simulator endpoint during region handshake".into());
    }
    let movement_complete = Packet::new(
        server_sequence,
        Message::AgentMovementComplete(AgentMovementComplete {
            agent_data: MovementAgentData {
                agent_id: UUID::from_u128(0xffeeddccbbaa99887766554433221100),
                session_id: UUID::from_u128(0x00112233445566778899aabbccddeeff),
            },
            data: MovementData { region_handle: 0x1000_2000_3000_4000, ..MovementData::default() },
            sim_data: MovementSimData::default(),
        }),
    )
    .with_reliable()
    .encode()
    .map_err(|error| error.to_string())?;
    send_local_packet(&simulator, peer, &mut server_sequence, movement_complete)?;

    loop {
        match command_receiver.try_recv() {
            Ok(LocalPeerCommand::SendObjectUpdate) => {
                let message = world_object_update();
                let packet = Packet::new(server_sequence, message).with_reliable().encode().map_err(|error| error.to_string())?;
                send_local_packet(&simulator, peer, &mut server_sequence, packet)?;
            }
            Ok(LocalPeerCommand::Stop) | Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {}
        }

        let mut bytes = [0; 65_535];
        match simulator.recv_from(&mut bytes) {
            Ok((size, source)) => {
                let packet = Packet::decode(&bytes[..size]).map_err(|error| error.to_string())?;
                if packet.is_reliable() {
                    send_local_ack(&simulator, source, &mut server_sequence, packet.sequence())?;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::ConnectionReset
                ) => {}
            Err(error) => return Err(error.to_string()),
        }
    }

    Ok(())
}

fn accept_local_connection(
    listener: &TcpListener,
    commands: &mpsc::Receiver<LocalPeerCommand>,
    phase: &'static str,
) -> Result<(TcpStream, SocketAddr), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        check_local_peer_command(commands)?;
        match listener.accept() {
            Ok((connection, address)) => {
                connection.set_nonblocking(false).map_err(|error| error.to_string())?;
                return Ok((connection, address));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(format!("timed out waiting for local {phase}"));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn check_local_peer_command(commands: &mpsc::Receiver<LocalPeerCommand>) -> Result<(), String> {
    match commands.try_recv() {
        Ok(LocalPeerCommand::Stop) | Err(TryRecvError::Disconnected) => Err("local network peer was stopped".into()),
        Ok(LocalPeerCommand::SendObjectUpdate) => Err("object update requested before simulator bootstrap completed".into()),
        Err(TryRecvError::Empty) => Ok(()),
    }
}

fn read_local_http_request(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut request = Vec::new();
    let mut chunk = [0; 2048];
    loop {
        if Instant::now() >= deadline {
            return Err("timed out reading local HTTP request".into());
        }
        let count = match stream.read(&mut chunk) {
            Ok(count) => count,
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) && Instant::now() < deadline => continue,
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Err("timed out reading local HTTP request".into());
            }
            Err(error) => return Err(error.to_string()),
        };
        if count == 0 {
            return Err("local HTTP peer closed before completing a request".into());
        }
        request.extend_from_slice(&chunk[..count]);
        let Some(body_start) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let content_length = request[..body_start]
            .split(|byte| *byte == b'\n')
            .filter_map(|line| std::str::from_utf8(line).ok())
            .find_map(|line| {
                let (name, value) = line.trim().split_once(':')?;
                name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
            })
            .unwrap_or(0);
        if request.len() >= body_start + 4 + content_length {
            return Ok(request);
        }
    }
}

fn write_local_http_response(stream: &mut TcpStream, content_type: &str, body: &str) -> Result<(), String> {
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
        .map_err(|error| error.to_string())
}

fn local_reliable_fixture(source: &str) -> Vec<u8> {
    let mut bytes = parse_hex_fixture(source).expect("valid LLUDP fixture");
    bytes[0] |= 0x40;
    bytes
}

fn send_local_packet(socket: &UdpSocket, peer: SocketAddr, sequence: &mut u32, mut bytes: Vec<u8>) -> Result<(), String> {
    bytes[1..5].copy_from_slice(&sequence.to_be_bytes());
    *sequence = sequence.checked_add(1).ok_or_else(|| "local simulator sequence overflow".to_owned())?;
    socket.send_to(&bytes, peer).map(|_| ()).map_err(|error| error.to_string())
}

fn send_local_ack(socket: &UdpSocket, peer: SocketAddr, sequence: &mut u32, acknowledged: u32) -> Result<(), String> {
    let mut bytes = vec![0];
    bytes.extend_from_slice(&sequence.to_be_bytes());
    bytes.extend_from_slice(&[0, 0xff, 0xff, 0xff, 0xfb, 1]);
    bytes.extend_from_slice(&acknowledged.to_le_bytes());
    *sequence = sequence.checked_add(1).ok_or_else(|| "local simulator sequence overflow".to_owned())?;
    socket.send_to(&bytes, peer).map(|_| ()).map_err(|error| error.to_string())
}

fn receive_local_client_message(
    socket: &UdpSocket,
    server_sequence: &mut u32,
    commands: &mpsc::Receiver<LocalPeerCommand>,
    expected: impl Fn(&Message) -> bool,
) -> Result<(Packet, SocketAddr), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        check_local_peer_command(commands)?;
        let mut bytes = [0; 65_535];
        match socket.recv_from(&mut bytes) {
            Ok((size, peer)) => {
                let packet = Packet::decode(&bytes[..size]).map_err(|error| error.to_string())?;
                if packet.is_reliable() {
                    send_local_ack(socket, peer, server_sequence, packet.sequence())?;
                }
                if expected(packet.message()) {
                    return Ok((packet, peer));
                }
            }
            Err(error)
                if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
                    && std::time::Instant::now() < deadline => {}
            Err(error) => return Err(error.to_string()),
        }
        if std::time::Instant::now() >= deadline {
            return Err("timed out waiting for expected client LLUDP message".into());
        }
    }
}

fn world_object_update() -> Message {
    let mut message = fixture_packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut message else { panic!("fixture must be ObjectUpdate") };
    update.region_data.region_handle = 0x1000_2000_3000_4000;
    update.objects.truncate(1);
    update.objects[0].id = UUID::from_u128(0x0123456789abcdef0123456789abcdef);
    update.objects[0].local_id = 73;
    update.objects[0].pcode = 9;
    update.objects[0].parent_local_id = 0;
    message
}

#[test]
fn local_update_reaches_world() {
    let dispatcher = TestDispatcher::new(0);
    dispatcher.scheduler().allow_parking();
    let cx = TestAppContext::build(dispatcher.clone(), Some("local_update_reaches_world"));
    let _entity_refcounts = cx.app.borrow().ref_counts_drop_handle();

    ForegroundExecutor::new(Arc::new(dispatcher.clone())).block_test(local_update_reaches_world_inner(&cx));

    cx.run_until_parked();
    cx.update(|cx| {
        cx.background_executor().forbid_parking();
        cx.quit();
    });
    cx.run_until_parked();
    drop(cx);
    dispatcher.drain_tasks();
}

async fn local_update_reaches_world_inner(cx: &TestAppContext) {
    let mut deadline = LocalTestDeadline::start();
    let mut peer = start_local_network_peer();
    let credentials = LoginCredentials {
        first_name: "Input".into(),
        last_name: "Name".into(),
        password: "fixture-password".into(),
        grid: Grid::Aditi,
        start_location: StartLocation::Last,
        mfa_hash: String::new(),
    };
    let mut network_session = network::testing::start_session_at(credentials, peer.login_url.clone()).expect("start local Network Session");
    let login_event = peer.wait(&mut deadline, network_session.recv_event(), "login acceptance").await;
    assert!(matches!(login_event, Some(Event::LoginAccepted { .. })));

    let session_handle = network_session.handle();
    // The local TCP/UDP peer wakes the test through OS threads.
    let background = cx.executor();
    let (mailbox, mut presentation) = PresentationMailbox::new(401);
    let (cancel, cancellation) = futures::channel::oneshot::channel();
    let coordinator = background.clone().spawn(drain_session(
        network_session,
        session_handle.clone(),
        background.clone(),
        mailbox,
        cancellation,
        Grid::Aditi,
        "Input Name".into(),
        "fixture-password".into(),
        false,
        write_lock(),
    ));

    let ready = loop {
        let snapshot = peer
            .wait(&mut deadline, presentation.next(), "coordinator session readiness")
            .await
            .expect("coordinator publishes session readiness");
        if let Some(failure) = snapshot.failure {
            panic!("network session failed before readiness: {failure}");
        }
        if let Some(ready) = snapshot.session_ready {
            break ready;
        }
    };
    let reader = ready.world_state;
    let initial_barrier = peer.wait(&mut deadline, reader.barrier(), "initial world barrier").await.expect("read initial world barrier");
    assert_eq!(initial_barrier.object_count, 0);

    peer.commands.send(LocalPeerCommand::SendObjectUpdate).expect("send local ObjectUpdate command");
    let barrier = loop {
        let barrier = peer.wait(&mut deadline, reader.barrier(), "world barrier after ObjectUpdate").await.expect("read world barrier");
        if barrier.object_count == 1 {
            break barrier;
        }
    };
    let page = peer
        .wait(&mut deadline, reader.object_list_page(None, 16), "object list after ObjectUpdate")
        .await
        .expect("read simulator objects");
    assert_eq!(barrier.object_count, 1);
    let object = page
        .objects
        .iter()
        .find(|object| object.id == UUID::from_u128(0x0123456789abcdef0123456789abcdef))
        .expect("decoded object reaches World reader");
    assert_eq!(object.address.local_id().get(), 73);

    let query = peer
        .wait(
            &mut deadline,
            reader.query(WorldQuery::live().kind(ValueFilter::Is(Kind::Object)), None, WorldQueryBudget::default()),
            "query after ObjectUpdate",
        )
        .await
        .expect("query simulator objects");
    assert_eq!(query.completeness, QueryCompleteness::Exact);
    assert!(query.items.iter().any(|item| item.id() == object.id), "query returns the network-fed object");

    let details = peer
        .wait(&mut deadline, reader.object_details(&[object.entity]), "object details after ObjectUpdate")
        .await
        .expect("read network-fed object details");
    let [ObjectDetailResult::Found(detail)] = details.details.as_slice() else { panic!("live network-fed object has details") };
    assert_eq!(detail.summary.id, object.id);
    assert_eq!(detail.summary.kind, Kind::Object);
    assert_eq!(detail.summary.address, object.address);

    let inspector = peer
        .wait(
            &mut deadline,
            crate::devtools::capture_snapshot_for_test(&reader, Some((object.id, object.entity))),
            "Developer Tools snapshot after ObjectUpdate",
        )
        .await
        .expect("capture the network-fed object in Developer Tools");
    assert_eq!(inspector.object_count, 1);
    assert_eq!(inspector.selected_id, Some(object.id));
    assert_eq!(inspector.selected_parent.as_deref(), Some("Root"));

    assert!(inspector.objects.iter().any(|(id, _, _, _)| *id == object.id));

    let changes = peer
        .wait(&mut deadline, reader.changes(initial_barrier.changes, 16), "World changes after ObjectUpdate")
        .await
        .expect("read World journal changes");
    assert!(changes.records.iter().any(|record| matches!(
        record.change,
        WorldChange::EntityAdded { id, address, .. } if id == object.id && address == object.address
    )));

    let regions =
        peer.wait(&mut deadline, reader.region_page(None, 16), "region page after ObjectUpdate").await.expect("read simulator region");
    assert!(regions.regions.iter().any(|region| {
        region.region == object.address.region() && region.info.handle == Some(world::RegionHandle::new(0x1000_2000_3000_4000))
    }));

    let proxy_objects = peer
        .wait(
            &mut deadline,
            crate::session::world::proxy_box::capture_proxy_objects_for_test(reader.clone()),
            "proxy consumer snapshot after ObjectUpdate",
        )
        .await
        .expect("proxy consumer reads the production World");
    assert!(proxy_objects.iter().any(|(id, address)| *id == object.id && *address == object.address));

    cancel.send(()).expect("cancel coordinator session");
    peer.wait(&mut deadline, coordinator, "coordinator shutdown").await;
    peer.wait(&mut deadline, session_handle.disconnect(), "network session disconnect").await;
    assert_eq!(session_handle.network_status().lifecycle, NetworkLifecycle::Disconnected);
    peer.stop().expect("stop local network peer");
    peer.wait_for_exit(&mut deadline).await.expect("local network peer exits cleanly");
}
