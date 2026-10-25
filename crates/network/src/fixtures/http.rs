use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    str::{self, from_utf8},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc as std_mpsc,
    },
    thread,
    time::Duration,
};

use reqwest::Url;
use tokio::{sync::mpsc, time::timeout};

pub(crate) use crate::capabilities::client_with_request_timeout;

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct LocalServer {
    address: SocketAddr,
    worker: thread::JoinHandle<Vec<Vec<u8>>>,
}

impl LocalServer {
    pub(crate) fn start(responses: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local HTTP server");
        let address = listener.local_addr().expect("read local listener address");
        let worker = thread::spawn(move || {
            responses
                .into_iter()
                .map(|response| {
                    let (mut stream, _) = accept_connection(&listener);
                    stream.set_read_timeout(Some(TEST_TIMEOUT)).expect("set local request timeout");
                    let request = read_request(&mut stream);
                    stream.write_all(&response).expect("write local response");
                    request
                })
                .collect()
        });

        Self { address, worker }
    }

    pub(crate) fn url(&self, path: &str) -> Url {
        Url::parse(&format!("http://{}{path}", self.address)).expect("valid local HTTP URL")
    }

    pub(crate) fn finish(self) -> Vec<Vec<u8>> {
        self.worker.join().expect("local HTTP server should finish")
    }
}

pub(crate) struct ConcurrentCapabilityTestServer {
    base: Url,
    requests: mpsc::UnboundedReceiver<()>,
    response_release: std_mpsc::Sender<()>,
    request_count: usize,
    released_responses: usize,
    worker: Option<thread::JoinHandle<usize>>,
}

impl ConcurrentCapabilityTestServer {
    pub(crate) fn start(request_count: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind capability concurrency server");
        let address = listener.local_addr().expect("read concurrency server address");
        let base = Url::parse(&format!("http://{address}/")).expect("valid local capability URL");
        let mut capability_url = base.clone();
        capability_url.set_path("/capability");
        let body = format!("<llsd><map><key>TestCapability</key><string>{capability_url}</string></map></llsd>");
        let response = response(200, "OK", "application/llsd+xml", None, &body);
        let (request_sender, requests) = mpsc::unbounded_channel();
        let (response_release, response_releases) = std_mpsc::channel();
        let response_releases = Arc::new(Mutex::new(response_releases));
        let worker = thread::spawn(move || {
            let active = Arc::new(AtomicUsize::new(0));
            let maximum_active = Arc::new(AtomicUsize::new(0));
            let mut handlers = Vec::with_capacity(request_count);
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().expect("accept capability concurrency request");
                stream.set_read_timeout(Some(TEST_TIMEOUT)).expect("set concurrency request timeout");
                let response = response.clone();
                let active = Arc::clone(&active);
                let maximum_active = Arc::clone(&maximum_active);
                let request_sender = request_sender.clone();
                let response_releases = Arc::clone(&response_releases);
                handlers.push(thread::spawn(move || {
                    let _request = read_request(&mut stream);
                    let active_count = active.fetch_add(1, Ordering::Relaxed) + 1;
                    maximum_active.fetch_max(active_count, Ordering::Relaxed);
                    let _ = request_sender.send(());
                    response_releases
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .recv_timeout(TEST_TIMEOUT)
                        .expect("test should release the capability response");
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
        Self { base, requests, response_release, request_count, released_responses: 0, worker: Some(worker) }
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

    pub(crate) fn release_response(&mut self) {
        self.response_release.send(()).expect("capability response worker should remain open");
        self.released_responses += 1;
    }

    pub(crate) fn finish(mut self) -> usize {
        for _ in 0..self.request_count.saturating_sub(self.released_responses) {
            self.response_release.send(()).expect("capability response worker should remain open");
        }
        self.worker.take().expect("concurrency server worker").join().expect("concurrency server should stop")
    }
}

pub(crate) enum EventServerResponse {
    Response(Vec<u8>),
    Hold,
    Controlled(std_mpsc::Receiver<Vec<u8>>),
    DropConnection,
}

pub(crate) struct EventQueueTestServer {
    base: Url,
    address: SocketAddr,
    pub(crate) requests: std_mpsc::Receiver<Vec<u8>>,
    async_requests: mpsc::UnboundedReceiver<Vec<u8>>,
    stop: std_mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

pub(crate) struct EventQueueBudgetTestServer {
    base: Url,
    address: SocketAddr,
    seeds: std_mpsc::Receiver<()>,
    pub(crate) polls: std_mpsc::Receiver<TcpStream>,
    stop: std_mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl EventQueueBudgetTestServer {
    pub(crate) fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind event queue budget server");
        let address = listener.local_addr().expect("read budget server address");
        let base = Url::parse(&format!("http://{address}/")).expect("valid event queue test URL");
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        let seed_response = seed_grant(&event_url);
        let (seed_sender, seeds) = std_mpsc::channel();
        let (poll_sender, polls) = std_mpsc::channel();
        let (stop, stop_receiver) = std_mpsc::channel();
        let worker = thread::spawn(move || {
            loop {
                let (mut stream, _) = listener.accept().expect("accept event queue budget request");
                if stop_receiver.try_recv().is_ok() {
                    return;
                }
                stream.set_read_timeout(Some(TEST_TIMEOUT)).expect("set budget request timeout");
                let request = read_request(&mut stream);
                match request_line(&request).split_whitespace().nth(1) {
                    Some("/seed") => {
                        stream.write_all(&seed_response).expect("write seed capability response");
                        seed_sender.send(()).expect("test should observe every seed lookup");
                    }
                    Some("/event-queue") => poll_sender.send(stream).expect("test should receive each event poll"),
                    path => panic!("unexpected capability request path: {path:?}"),
                }
            }
        });
        Self { base, address, seeds, polls, stop, worker: Some(worker) }
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
        if self.stop.send(()).is_ok()
            && let Ok(stream) = TcpStream::connect(self.address)
        {
            drop(stream);
        }
    }

    pub(crate) fn finish(mut self) {
        self.worker.take().expect("budget server worker").join().expect("event queue budget server should stop");
    }
}

impl EventQueueTestServer {
    pub(crate) fn start(responses: impl FnOnce(&Url) -> Vec<EventServerResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind event queue test server");
        let address = listener.local_addr().expect("read test server address");
        let base = Url::parse(&format!("http://{address}/")).expect("valid local test URL");
        let responses = responses(&base);
        let (request_sender, requests) = std_mpsc::channel();
        let (async_request_sender, async_requests) = mpsc::unbounded_channel();
        let (stop, stop_receiver) = std_mpsc::channel();
        let worker = thread::spawn(move || {
            for response in responses {
                let Some((mut stream, _)) = accept_until_stopped(&listener, &stop_receiver) else {
                    return;
                };
                stream.set_read_timeout(Some(TEST_TIMEOUT)).expect("set test request timeout");
                let request = read_request(&mut stream);
                request_sender.send(request.clone()).expect("test should read request");
                match response {
                    EventServerResponse::Response(response) => {
                        stream.write_all(&response).expect("write test HTTP response");
                        async_request_sender.send(request).expect("async test should read request");
                    }
                    EventServerResponse::Controlled(response) => {
                        async_request_sender.send(request.clone()).expect("async test should read request");
                        let response = response.recv_timeout(TEST_TIMEOUT).expect("controlled HTTP response");
                        stream.write_all(&response).expect("write controlled HTTP response");
                    }
                    EventServerResponse::Hold => {
                        async_request_sender.send(request).expect("async test should read request");
                        let mut byte = [0];
                        let _ = stream.read(&mut byte);
                    }
                    EventServerResponse::DropConnection => {
                        drop(stream);
                        async_request_sender.send(request).expect("async test should read request");
                    }
                }
            }
        });
        Self { base, address, requests, async_requests, stop, worker: Some(worker) }
    }

    pub(crate) fn url(&self, path: &str) -> Url {
        let mut url = self.base.clone();
        url.set_path(path);
        url
    }

    pub(crate) fn next_request(&self) -> Vec<u8> {
        self.requests.recv_timeout(TEST_TIMEOUT).expect("server should receive HTTP request")
    }

    pub(crate) async fn next_request_async(&mut self) -> Vec<u8> {
        self.async_requests.recv().await.expect("server should receive HTTP request")
    }

    pub(crate) fn stop(&self) {
        if self.stop.send(()).is_ok()
            && let Ok(stream) = TcpStream::connect(self.address)
        {
            drop(stream);
        }
    }

    pub(crate) fn finish(mut self) -> Vec<Vec<u8>> {
        if let Some(worker) = self.worker.take() {
            worker.join().expect("event queue server should stop");
        }
        self.requests.try_iter().collect()
    }
}

pub(crate) fn write_response(mut stream: TcpStream, response: Vec<u8>) {
    stream.write_all(&response).expect("write controlled HTTP response");
}

pub(crate) fn response(status: u16, reason: &str, content_type: &str, retry_after: Option<&str>, body: &str) -> Vec<u8> {
    let retry_after = retry_after.map(|value| format!("Retry-After: {value}\r\n")).unwrap_or_default();
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{retry_after}\r\n{body}",
        body.len()
    )
    .into_bytes()
}

pub(crate) fn request_line(request: &[u8]) -> &str {
    str::from_utf8(request).expect("request should be UTF-8").split("\r\n").next().expect("request line")
}

pub(crate) fn seed_response(event_url: &Url) -> String {
    format!("<llsd><map><key>EventQueueGet</key><string>{event_url}</string></map></llsd>")
}

pub(crate) fn llsd_response(body: &str) -> Vec<u8> {
    response(200, "OK", "application/llsd+xml", None, body)
}

pub(crate) fn seed_grant(event_url: &Url) -> Vec<u8> {
    llsd_response(&seed_response(event_url))
}

pub(crate) fn event_batch_body(id: u64, messages: &[&str]) -> String {
    let events = messages
        .iter()
        .map(|message| format!("<map><key>message</key><string>{}</string></map>", escape_xml_text(message)))
        .collect::<String>();
    format!("<llsd><map><key>id</key><integer>{id}</integer><key>events</key><array>{events}</array></map></llsd>")
}

pub(crate) fn event_batch_response(id: u64, messages: &[&str]) -> Vec<u8> {
    llsd_response(&event_batch_body(id, messages))
}

fn escape_xml_text(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub(crate) fn hold_response() -> EventServerResponse {
    EventServerResponse::Hold
}

fn accept_connection(listener: &TcpListener) -> (TcpStream, SocketAddr) {
    listener.accept().expect("accept local HTTP request")
}

fn accept_until_stopped(listener: &TcpListener, stop: &std_mpsc::Receiver<()>) -> Option<(TcpStream, SocketAddr)> {
    let connection = listener.accept().expect("accept event queue test connection");
    if stop.try_recv().is_ok() { None } else { Some(connection) }
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let size = stream.read(&mut chunk).expect("read local HTTP request");
        if size == 0 {
            return request;
        }
        request.extend_from_slice(&chunk[..size]);
        if let Some(body_start) = header_body_start(&request) {
            let content_length = request[..body_start]
                .split(|byte| *byte == b'\n')
                .filter_map(|line| from_utf8(line).ok())
                .find_map(|line| {
                    let (name, value) = line.trim().split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
                })
                .unwrap_or(0);
            if request.len() >= body_start + 4 + content_length {
                return request;
            }
        }
    }
}

fn header_body_start(request: &[u8]) -> Option<usize> {
    request.windows(4).position(|window| window == b"\r\n\r\n")
}
