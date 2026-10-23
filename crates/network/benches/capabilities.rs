use std::{
    hint::black_box,
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    str::from_utf8,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use criterion::{Criterion, Throughput};
use llsd::Value;
use network::capabilities::{CapabilityUrl, Client};
use reqwest::Url;
use serde::Serialize;
use tokio::runtime::Builder;

const REQUESTED_CAPABILITIES: &[&str] = &["EventQueueGet", "FetchInventory2", "GetTexture"];

pub fn bench(c: &mut Criterion) {
    bench_seed_discovery(c);
    bench_capability_post(c);
}

struct LoopbackServer {
    address: SocketAddr,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl LoopbackServer {
    fn start(response_body: impl FnOnce(SocketAddr) -> String) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = Arc::clone(&running);
        let response = response_body(address);
        let thread = thread::spawn(move || {
            while thread_running.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => serve_connection(stream, &response, &thread_running),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(1)),
                    Err(_) => break,
                }
            }
        });

        Ok(Self { address, running, thread: Some(thread) })
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve_connection(mut stream: TcpStream, response_body: &str, running: &AtomicBool) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let response_header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/llsd+xml\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
        response_body.len()
    );
    let mut request = Vec::with_capacity(4096);
    let mut bytes = [0; 4096];

    while running.load(Ordering::Relaxed) {
        let request_end = loop {
            match stream.read(&mut bytes) {
                Ok(0) => return,
                Ok(read) => {
                    request.extend_from_slice(&bytes[..read]);
                    if request.len() > 64 * 1024 {
                        return;
                    }
                    if let Some(request_end) = complete_request_len(&request) {
                        break request_end;
                    }
                }
                Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                    if !running.load(Ordering::Relaxed) {
                        return;
                    }
                }
                Err(_) => return,
            }
        };

        request.drain(..request_end);
        if stream.write_all(response_header.as_bytes()).is_err() || stream.write_all(response_body.as_bytes()).is_err() {
            return;
        }
    }
}

fn complete_request_len(request: &[u8]) -> Option<usize> {
    let header_end = request.windows(4).position(|window| window == b"\r\n\r\n")? + 4;
    let header = from_utf8(&request[..header_end]).ok()?;
    let content_length = header
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
        })
        .unwrap_or(0);
    let request_end = header_end.checked_add(content_length)?;
    (request.len() >= request_end).then_some(request_end)
}

fn bench_seed_discovery(c: &mut Criterion) {
    let server = LoopbackServer::start(|address| {
        format!(
            "<?xml version=\"1.0\"?><llsd><map><key>EventQueueGet</key><uri>http://{address}/events</uri><key>FetchInventory2</key><uri>http://{address}/inventory</uri><key>GetTexture</key><uri>http://{address}/texture</uri></map></llsd>"
        )
    })
    .expect("start local seed capability server");
    let runtime = Builder::new_current_thread().enable_all().build().expect("build Tokio runtime");
    let seed = Url::parse(&format!("http://{}/seed", server.address)).expect("parse local seed URL");
    let client = runtime.block_on(async { Client::new(seed).expect("create capability client") });
    let warmup = runtime.block_on(client.request_capabilities(REQUESTED_CAPABILITIES)).expect("warm up capability discovery");
    assert_eq!(warmup.len(), REQUESTED_CAPABILITIES.len());

    let mut group = c.benchmark_group("network/capability_discovery");
    group.throughput(Throughput::Elements(1));
    group.bench_function("three_capabilities", |b| {
        b.iter(|| black_box(runtime.block_on(client.request_capabilities(REQUESTED_CAPABILITIES)).expect("discover capabilities")));
    });
    group.finish();
}

#[derive(Serialize)]
struct CapabilityRequest {
    item_id: &'static str,
}

fn bench_capability_post(c: &mut Criterion) {
    let server = LoopbackServer::start(|_| {
        "<?xml version=\"1.0\"?><llsd><map><key>accepted</key><boolean>true</boolean><key>revision</key><integer>42</integer></map></llsd>"
            .to_owned()
    })
    .expect("start local capability server");
    let runtime = Builder::new_current_thread().enable_all().build().expect("build Tokio runtime");
    let seed = Url::parse(&format!("http://{}/seed", server.address)).expect("parse local seed URL");
    let capability = CapabilityUrl::parse(&format!("http://{}/inventory", server.address)).expect("parse local capability URL");
    let client = runtime.block_on(async { Client::new(seed).expect("create capability client") });
    let request = CapabilityRequest { item_id: "benchmark-item" };
    let warmup: Value = runtime.block_on(client.post(&capability, &request)).expect("warm up capability POST");
    let fields = warmup.as_map().expect("response LLSD map");
    assert_eq!(fields.get("accepted").and_then(Value::as_bool), Some(true));
    assert_eq!(fields.get("revision").and_then(Value::as_integer), Some(42));

    let mut group = c.benchmark_group("network/capability_post");
    group.throughput(Throughput::Elements(1));
    group.bench_function("xml_request_response", |b| {
        b.iter(|| black_box(runtime.block_on(client.post(&capability, &request)).expect("post capability")));
    });
    group.finish();
}
