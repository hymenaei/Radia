pub(crate) mod http;

use std::{
    io::{ErrorKind, Read, Write},
    net::{SocketAddr, SocketAddrV4, TcpListener, TcpStream},
    str,
    sync::{
        self, Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use llsd::{Map, Value, xml};
use protocol::{
    lludp::{Message, Packet},
    messages::{PacketAck, circuit::packet_ack::Packets},
};
use reqwest::Url;
use tokio::{
    net::UdpSocket,
    sync::{Notify, mpsc, watch},
    time::timeout,
};
use uuid::Uuid;

use crate::test_support::http::{read_request, response};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct ConcurrentCapabilityTestServer {
    base: Url,
    requests: mpsc::UnboundedReceiver<()>,
    worker: Option<thread::JoinHandle<usize>>,
}

impl ConcurrentCapabilityTestServer {
    pub(crate) fn start(request_count: usize, response_delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind capability concurrency server");
        listener.set_nonblocking(true).expect("set concurrency listener nonblocking");
        let address = listener.local_addr().expect("read concurrency server address");
        let base = Url::parse(&format!("http://{address}/")).expect("valid local capability URL");
        let mut capability_url = base.clone();
        capability_url.set_path("/capability");
        let body = format!("<llsd><map><key>TestCapability</key><string>{capability_url}</string></map></llsd>");
        let response = response(200, "OK", "application/llsd+xml", None, &body);
        let (request_sender, requests) = mpsc::unbounded_channel();
        let worker = thread::spawn(move || {
            let active = Arc::new(AtomicUsize::new(0));
            let maximum_active = Arc::new(AtomicUsize::new(0));
            let mut handlers = Vec::with_capacity(request_count);
            for _ in 0..request_count {
                let (mut stream, _) = accept_concurrent_request(&listener);
                stream.set_nonblocking(false).expect("set capability test stream blocking");
                stream.set_read_timeout(Some(TEST_TIMEOUT)).expect("set concurrency request timeout");
                let response = response.clone();
                let active = Arc::clone(&active);
                let maximum_active = Arc::clone(&maximum_active);
                let request_sender = request_sender.clone();
                handlers.push(thread::spawn(move || {
                    let _request = read_request(&mut stream);
                    let _ = request_sender.send(());
                    let active_count = active.fetch_add(1, Ordering::Relaxed) + 1;
                    maximum_active.fetch_max(active_count, Ordering::Relaxed);
                    thread::sleep(response_delay);
                    active.fetch_sub(1, Ordering::Relaxed);
                    let _ = stream.write_all(&response);
                }));
            }
            drop(request_sender);
            for handler in handlers {
                handler.join().expect("capability request handler should finish");
            }
            maximum_active.load(Ordering::Relaxed)
        });
        Self { base, requests, worker: Some(worker) }
    }

    pub(crate) fn url(&self, path: &str) -> Url {
        let mut url = self.base.clone();
        url.set_path(path);
        url
    }

    pub(crate) async fn next_request(&mut self) {
        timeout(TEST_TIMEOUT, self.requests.recv())
            .await
            .expect("HTTP request should reach capability server")
            .expect("capability server request stream should remain open");
    }

    pub(crate) fn finish(mut self) -> usize {
        self.worker.take().expect("concurrency server worker").join().expect("concurrency server should stop")
    }
}

pub(crate) struct SendGate {
    pub(crate) paused: watch::Receiver<bool>,
    pub(crate) entered: Arc<Notify>,
}

pub(crate) struct DecodeGate {
    entered: sync::mpsc::Sender<()>,
    released: sync::Mutex<bool>,
    release: sync::Condvar,
}

impl DecodeGate {
    pub(crate) fn new() -> (Arc<Self>, sync::mpsc::Receiver<()>) {
        let (entered, entered_rx) = sync::mpsc::channel();
        (Arc::new(Self { entered, released: sync::Mutex::new(false), release: sync::Condvar::new() }), entered_rx)
    }

    pub(crate) fn release(&self) {
        *self.released.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.release.notify_all();
    }

    pub(crate) fn wait(&self) {
        if self.entered.send(()).is_err() {
            return;
        }
        let mut released = self.released.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while !*released {
            let (next, result) = self.release.wait_timeout(released, TEST_TIMEOUT).unwrap_or_else(|poisoned| poisoned.into_inner());
            released = next;
            if !*released && result.timed_out() {
                panic!("timed out waiting for test to release decoder");
            }
        }
    }
}

pub(crate) enum EventServerResponse {
    Response(Vec<u8>),
    Hold,
    Controlled(sync::mpsc::Receiver<Vec<u8>>),
    DropConnection,
}

pub(crate) struct EventQueueTestServer {
    base: Url,
    pub(crate) requests: sync::mpsc::Receiver<Vec<u8>>,
    stop: sync::mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl EventQueueTestServer {
    pub(crate) fn start(responses: impl FnOnce(&Url) -> Vec<EventServerResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind event queue test server");
        listener.set_nonblocking(true).expect("set test listener nonblocking");
        let address = listener.local_addr().expect("read test server address");
        let base = Url::parse(&format!("http://{address}/")).expect("valid local test URL");
        let responses = responses(&base);
        let (request_sender, requests) = sync::mpsc::channel();
        let (stop, stop_receiver) = sync::mpsc::channel();
        let worker = thread::spawn(move || {
            for response in responses {
                let Some((mut stream, _)) = accept_until_stopped(&listener, &stop_receiver) else {
                    return;
                };
                stream.set_nonblocking(false).expect("set test stream blocking");
                stream.set_read_timeout(Some(Duration::from_secs(5))).expect("set test request timeout");
                let request = read_request(&mut stream);
                request_sender.send(request).expect("test should read request");
                match response {
                    EventServerResponse::Response(response) => {
                        stream.write_all(&response).expect("write test HTTP response");
                    }
                    EventServerResponse::Controlled(response) => {
                        let response = response.recv_timeout(TEST_TIMEOUT).expect("controlled HTTP response");
                        stream.write_all(&response).expect("write controlled HTTP response");
                    }
                    EventServerResponse::Hold => {
                        let mut byte = [0];
                        let _ = stream.read(&mut byte);
                    }
                    EventServerResponse::DropConnection => drop(stream),
                }
            }
        });
        Self { base, requests, stop, worker: Some(worker) }
    }

    pub(crate) fn url(&self, path: &str) -> Url {
        let mut url = self.base.clone();
        url.set_path(path);
        url
    }

    pub(crate) fn next_request(&self) -> Vec<u8> {
        self.requests.recv_timeout(TEST_TIMEOUT).expect("server should receive HTTP request")
    }

    pub(crate) fn stop(&self) {
        let _ = self.stop.send(());
    }

    pub(crate) fn finish(mut self) -> Vec<Vec<u8>> {
        if let Some(worker) = self.worker.take() {
            worker.join().expect("event queue server should stop");
        }
        self.requests.try_iter().collect()
    }
}

pub(crate) struct EventQueueBudgetTestServer {
    base: Url,
    seeds: sync::mpsc::Receiver<()>,
    pub(crate) polls: sync::mpsc::Receiver<TcpStream>,
    stop: sync::mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl EventQueueBudgetTestServer {
    pub(crate) fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind event queue budget server");
        listener.set_nonblocking(true).expect("set budget listener nonblocking");
        let address = listener.local_addr().expect("read budget server address");
        let base = Url::parse(&format!("http://{address}/")).expect("valid event queue test URL");
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        let seed_response = response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url));
        let (seed_sender, seeds) = sync::mpsc::channel();
        let (poll_sender, polls) = sync::mpsc::channel();
        let (stop, stop_receiver) = sync::mpsc::channel();
        let worker = thread::spawn(move || {
            while let Some((mut stream, _)) = accept_until_stopped(&listener, &stop_receiver) {
                stream.set_nonblocking(false).expect("set budget stream blocking");
                stream.set_read_timeout(Some(TEST_TIMEOUT)).expect("set budget request timeout");
                let request = read_request(&mut stream);
                let path = request_line(&request).split_whitespace().nth(1);
                match path {
                    Some("/seed") => {
                        stream.write_all(&seed_response).expect("write seed capability response");
                        seed_sender.send(()).expect("test should observe every seed lookup");
                    }
                    Some("/event-queue") => poll_sender.send(stream).expect("test should receive each event poll"),
                    path => panic!("unexpected capability request path: {path:?}"),
                }
            }
        });
        Self { base, seeds, polls, stop, worker: Some(worker) }
    }

    pub(crate) fn url(&self, path: &str) -> Url {
        let mut url = self.base.clone();
        url.set_path(path);
        url
    }

    pub(crate) fn next_poll(&self) -> TcpStream {
        self.polls.recv_timeout(TEST_TIMEOUT).expect("event queue request should reach budget server")
    }

    pub(crate) fn next_seed(&self) {
        self.seeds.recv_timeout(TEST_TIMEOUT).expect("seed lookup should reach budget server");
    }

    pub(crate) fn stop(&self) {
        let _ = self.stop.send(());
    }

    pub(crate) fn finish(mut self) {
        self.worker.take().expect("budget server worker").join().expect("event queue budget server should stop");
    }
}

fn accept_until_stopped(listener: &TcpListener, stop: &sync::mpsc::Receiver<()>) -> Option<(TcpStream, SocketAddr)> {
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        if stop.try_recv().is_ok() {
            return None;
        }
        match listener.accept() {
            Ok(connection) => return Some(connection),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "event queue test server did not receive HTTP request");
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("accept event queue test connection: {error}"),
        }
    }
}

fn accept_concurrent_request(listener: &TcpListener) -> (TcpStream, SocketAddr) {
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        match listener.accept() {
            Ok(connection) => return connection,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "capability server did not receive a request");
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("accept test connection: {error}"),
        }
    }
}

pub(crate) fn request_line(request: &[u8]) -> &str {
    str::from_utf8(request).expect("request should be UTF-8").split("\r\n").next().expect("request line")
}

pub(crate) fn request_body(request: &[u8]) -> &str {
    str::from_utf8(request).expect("request should be UTF-8").split_once("\r\n\r\n").expect("request should include body").1
}

pub(crate) fn decode_request_body(request: &[u8]) -> Value {
    xml::from_str(request_body(request)).expect("request body should contain LLSD XML")
}

pub(crate) fn map_field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_map()?.get(key)
}

pub(crate) const TWO_EVENT_BATCH: &str = "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>message</key><string>First</string></map><map><key>message</key><string>Second</string></map></array></map></llsd>";
pub(crate) const MALFORMED_BATCH: &str = "<llsd><map>";

pub(crate) fn write_response(mut stream: TcpStream, response: Vec<u8>) {
    stream.write_all(&response).expect("write controlled HTTP response");
}

pub(crate) fn seed_response(event_url: &Url) -> String {
    format!("<llsd><map><key>EventQueueGet</key><string>{event_url}</string></map></llsd>")
}

pub(crate) fn test_source() -> SocketAddr {
    "127.0.0.1:13000".parse().expect("fixture simulator endpoint")
}

pub(crate) fn simulator_events(ip: [u8; 4], port: u16, agent_id: Uuid, seed: &Url) -> String {
    let simulator = Value::Map(Map::from([
        ("Handle".to_owned(), Value::Binary(0x1000_0000_0000_1000_u64.to_be_bytes().to_vec().into())),
        ("IP".to_owned(), Value::Binary(ip.to_vec().into())),
        ("Port".to_owned(), Value::Integer(i32::from(port))),
    ]));
    let enable = Value::Map(Map::from([
        ("message".to_owned(), Value::String("EnableSimulator".to_owned())),
        ("body".to_owned(), Value::Map(Map::from([("SimulatorInfo".to_owned(), Value::Array(vec![simulator]))]))),
    ]));
    let establish = Value::Map(Map::from([
        ("message".to_owned(), Value::String("EstablishAgentCommunication".to_owned())),
        (
            "body".to_owned(),
            Value::Map(Map::from([
                ("agent-id".to_owned(), Value::Uuid(agent_id)),
                (
                    "sim-ip-and-port".to_owned(),
                    Value::String(format!("{}:{}", ip.iter().map(u8::to_string).collect::<Vec<_>>().join("."), port)),
                ),
                ("seed-capability".to_owned(), Value::String(seed.to_string())),
            ])),
        ),
    ]));
    let batch = Value::Map(Map::from([("id".to_owned(), Value::Integer(1)), ("events".to_owned(), Value::Array(vec![enable, establish]))]));
    xml::to_string(&batch).expect("encode local LLSD event queue response")
}

pub(crate) fn crossed_region_events(agent_id: Uuid, session_id: Uuid, region_handle: u64, endpoint: SocketAddrV4, seed: &Url) -> String {
    let crossed_region = Value::Map(Map::from([
        ("message".to_owned(), Value::String("CrossedRegion".to_owned())),
        (
            "body".to_owned(),
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
                        ("RegionHandle".to_owned(), Value::Binary(region_handle.to_be_bytes().to_vec().into())),
                        ("SimIP".to_owned(), Value::Binary(endpoint.ip().octets().to_vec().into())),
                        ("SimPort".to_owned(), Value::Integer(i32::from(endpoint.port()))),
                        ("SeedCapability".to_owned(), Value::String(seed.to_string())),
                    ]))]),
                ),
            ])),
        ),
    ]));
    let batch = Value::Map(Map::from([("id".to_owned(), Value::Integer(1)), ("events".to_owned(), Value::Array(vec![crossed_region]))]));
    xml::to_string(&batch).expect("encode local CrossedRegion event")
}

pub(crate) async fn receive_udp(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    let mut buffer = [0; 65_535];
    let (size, endpoint) = timeout(TEST_TIMEOUT, socket.recv_from(&mut buffer))
        .await
        .expect("receive local simulator packet")
        .expect("read local simulator packet");
    (buffer[..size].to_vec(), endpoint)
}

pub(crate) async fn receive_message(socket: &UdpSocket, expected: impl Fn(&Message) -> bool) -> Packet {
    timeout(TEST_TIMEOUT, async {
        loop {
            let (bytes, _) = receive_udp(socket).await;
            let packet = Packet::decode(&bytes).expect("decode local simulator packet");
            if expected(packet.message()) {
                return packet;
            }
        }
    })
    .await
    .expect("receive expected local simulator message")
}

pub(crate) fn acknowledgement(sequence: u32, received_sequence: u32) -> Vec<u8> {
    Packet::new(sequence, Message::PacketAck(PacketAck { packets: vec![Packets { id: received_sequence }] }))
        .encode()
        .expect("encode simulator circuit acknowledgement")
}

pub(crate) fn hold_response() -> EventServerResponse {
    EventServerResponse::Hold
}
