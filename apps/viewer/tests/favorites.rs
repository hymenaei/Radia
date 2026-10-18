use protocol::login;
use protocol::login::ResponseData;
use std::{
    collections::BTreeMap,
    io::{ErrorKind, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    str::from_utf8,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use base::UUID;
use llsd::{Value, xml};
use network::capabilities::Client;
use tokio::runtime::Runtime;
use url::Url;
use viewer::inventory::favorites::{favorite_folder_id, fetch_favorite_landmarks};

const AGENT_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const FAVORITES_FOLDER_ID: &str = "11111111-2222-3333-4444-555555555555";

#[test]
fn folder_uses_login_data() {
    let login_data = ResponseData::from_fields(BTreeMap::from([(
        "inventory-skeleton".into(),
        login::Value::Array(vec![
            login::Value::Struct(BTreeMap::from([
                ("folder_id".into(), login::Value::String("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".into())),
                ("name".into(), login::Value::String("My Inventory".into())),
            ])),
            login::Value::Struct(BTreeMap::from([
                ("folder_id".into(), login::Value::String(FAVORITES_FOLDER_ID.into())),
                ("name".into(), login::Value::String("Favorites".into())),
            ])),
        ]),
    )]));

    assert_eq!(favorite_folder_id(&login_data).expect("favorites folder should parse").to_string(), FAVORITES_FOLDER_ID);
}

#[test]
fn empty_folder_requests_items() {
    let runtime = Runtime::new().expect("create local Tokio runtime");
    let server = LocalServer::start(include_str!("fixtures/favorites_empty_folder.xml"), None);
    let seed = server.seed_url();
    let capabilities = runtime.block_on(async { Client::new(seed).expect("active Tokio runtime") });
    let agent_id = UUID::parse_str(AGENT_ID).expect("valid fixture UUID");
    let folder_id = UUID::parse_str(FAVORITES_FOLDER_ID).expect("valid fixture UUID");

    let favorites = runtime.block_on(fetch_favorite_landmarks(&capabilities, agent_id, folder_id)).expect("empty folder is valid");
    assert!(favorites.is_empty());

    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    let inventory_request = parse_request_body(&requests[1]);
    let fields = inventory_request.as_map().expect("LLSD map");
    let folder = fields
        .get("folders")
        .and_then(Value::as_array)
        .and_then(|folders| folders.first())
        .and_then(Value::as_map)
        .expect("requested folder");
    assert_eq!(folder.get("folder_id"), Some(&Value::UUID(folder_id)));
    assert_eq!(folder.get("owner_id"), Some(&Value::UUID(agent_id)));
    assert_eq!(folder.get("sort_order"), Some(&Value::Integer(0)));
    assert_eq!(folder.get("fetch_folders"), Some(&Value::Boolean(true)));
    assert_eq!(folder.get("fetch_items"), Some(&Value::Boolean(true)));
}

#[test]
fn missing_items_rejected() {
    let runtime = Runtime::new().expect("create local Tokio runtime");
    let server = LocalServer::start(include_str!("fixtures/favorites_incomplete_folder.xml"), None);
    let seed = server.seed_url();
    let capabilities = runtime.block_on(async { Client::new(seed).expect("active Tokio runtime") });
    let agent_id = UUID::parse_str(AGENT_ID).expect("valid fixture UUID");
    let folder_id = UUID::parse_str(FAVORITES_FOLDER_ID).expect("valid fixture UUID");

    let error = runtime
        .block_on(fetch_favorite_landmarks(&capabilities, agent_id, folder_id))
        .expect_err("missing items with a nonempty folder is malformed");
    assert!(error.contains("invalid Favorites item list"));
    let _ = server.finish();
}

#[test]
fn landmark_asset_parsed() {
    let runtime = Runtime::new().expect("create local Tokio runtime");
    let server = LocalServer::start(
        include_str!("fixtures/favorites_landmark_folder.xml"),
        Some("Landmark version 1\nglobal_pos 256000 512000 30\n"),
    );
    let seed = server.seed_url();
    let capabilities = runtime.block_on(async { Client::new(seed).expect("active Tokio runtime") });
    let agent_id = UUID::parse_str(AGENT_ID).expect("valid fixture UUID");
    let folder_id = UUID::parse_str(FAVORITES_FOLDER_ID).expect("valid fixture UUID");

    let landmarks = runtime.block_on(fetch_favorite_landmarks(&capabilities, agent_id, folder_id)).expect("landmark fixture is valid");
    assert_eq!(landmarks.len(), 1);
    assert_eq!(landmarks[0].name, "Test Landmark");
    assert_eq!(landmarks[0].global_pos, Some([256000.0, 512000.0, 30.0]));

    let requests = server.finish();
    assert_eq!(requests.len(), 3);
    assert!(String::from_utf8_lossy(&requests[2]).starts_with("GET /asset?landmark_id="));
}

#[test]
fn landmark_asset_request_failure_rejects_partial_results() {
    const ASSET_COUNT: usize = 16;
    const MAX_IN_FLIGHT_ASSET_REQUESTS: usize = 3;

    let runtime = Runtime::new().expect("create local Tokio runtime");
    let server = ConcurrentAssetServer::start_with_failure(landmark_folder_response(ASSET_COUNT), ASSET_COUNT, Some(0));
    let seed = server.seed_url();
    let capabilities = runtime.block_on(async { Client::new(seed).expect("active Tokio runtime") });
    let agent_id = UUID::parse_str(AGENT_ID).expect("valid fixture UUID");
    let folder_id = UUID::parse_str(FAVORITES_FOLDER_ID).expect("valid fixture UUID");

    let error = runtime
        .block_on(fetch_favorite_landmarks(&capabilities, agent_id, folder_id))
        .expect_err("an asset request failure must reject the incomplete snapshot");
    assert!(error.contains("Could not fetch favorite landmark asset"), "unexpected error: {error}");

    let (requests, _) = server.finish();
    let asset_request_count = requests.len().saturating_sub(2);
    assert!(asset_request_count > 0);
    assert!(asset_request_count <= MAX_IN_FLIGHT_ASSET_REQUESTS, "handled {asset_request_count} asset requests");
    assert!(asset_request_count < ASSET_COUNT, "the refresh continued through all candidates after an asset request failed");
}

#[test]
fn malformed_landmark_is_skipped_with_valid_peer() {
    let runtime = Runtime::new().expect("create local Tokio runtime");
    let server = LocalServer::start_with_asset_responses(
        landmark_folder_response(2),
        vec![("200 OK", "Landmark version 1\nglobal_pos 256000 512000 30\n"), ("200 OK", "not a landmark asset")],
    );
    let seed = server.seed_url();
    let capabilities = runtime.block_on(async { Client::new(seed).expect("active Tokio runtime") });
    let agent_id = UUID::parse_str(AGENT_ID).expect("valid fixture UUID");
    let folder_id = UUID::parse_str(FAVORITES_FOLDER_ID).expect("valid fixture UUID");

    let landmarks = runtime
        .block_on(fetch_favorite_landmarks(&capabilities, agent_id, folder_id))
        .expect("one unreadable asset must not discard a readable landmark");
    assert_eq!(landmarks.len(), 1);

    let requests = server.finish();
    assert_eq!(requests.len(), 4);
}

#[test]
fn landmark_fetches_are_bounded_and_keep_folder_order() {
    const ASSET_COUNT: usize = 16;
    const MAX_CONCURRENT_FETCHES: usize = 3;

    let runtime = Runtime::new().expect("create local Tokio runtime");
    let folder_response = landmark_folder_response(ASSET_COUNT);
    let server = ConcurrentAssetServer::start(folder_response, ASSET_COUNT);
    let seed = server.seed_url();
    let capabilities = runtime.block_on(async { Client::new(seed).expect("active Tokio runtime") });
    let agent_id = UUID::parse_str(AGENT_ID).expect("valid fixture UUID");
    let folder_id = UUID::parse_str(FAVORITES_FOLDER_ID).expect("valid fixture UUID");

    let landmarks = runtime.block_on(fetch_favorite_landmarks(&capabilities, agent_id, folder_id));
    let (requests, peak_fetches) = server.finish();
    let landmarks = landmarks.expect("landmark fixtures are valid");
    assert_eq!(landmarks.len(), ASSET_COUNT);
    for (index, landmark) in landmarks.iter().enumerate() {
        assert_eq!(landmark.name, format!("Landmark {index:02}"));
    }

    assert_eq!(requests.len(), ASSET_COUNT + 2);
    assert!(peak_fetches > 1, "asset requests should overlap");
    assert!(peak_fetches <= MAX_CONCURRENT_FETCHES, "peak concurrency was {peak_fetches}");
}

fn parse_request_body(request: &[u8]) -> Value {
    let body_start = request.windows(4).position(|window| window == b"\r\n\r\n").expect("HTTP request headers");
    let body = from_utf8(&request[body_start + 4..]).expect("LLSD request is UTF-8");
    xml::from_str(body).expect("request is valid LLSD XML")
}

fn landmark_folder_response(asset_count: usize) -> String {
    let items = (0..asset_count)
        .map(|index| {
            format!(
                "<map><key>item_id</key><uuid>22222222-3333-4444-5555-{index:012x}</uuid>\
                 <key>asset_id</key><uuid>33333333-4444-5555-6666-{index:012x}</uuid>\
                 <key>inv_type</key><integer>3</integer><key>type</key><integer>3</integer>\
                 <key>name</key><string>Landmark {index:02}</string></map>"
            )
        })
        .collect::<String>();
    format!(
        "<?xml version=\"1.0\"?><llsd><map><key>folders</key><array><map>\
         <key>folder_id</key><uuid>{FAVORITES_FOLDER_ID}</uuid>\
         <key>descendents</key><integer>{asset_count}</integer>\
         <key>categories</key><array></array><key>items</key><array>{items}</array>\
         </map></array></map></llsd>"
    )
}

struct ConcurrentAssetServer {
    address: SocketAddr,
    worker: thread::JoinHandle<(Vec<Vec<u8>>, usize)>,
}

impl ConcurrentAssetServer {
    fn start(folder_response: String, asset_count: usize) -> Self {
        Self::start_with_failure(folder_response, asset_count, None)
    }

    fn start_with_failure(folder_response: String, asset_count: usize, failed_asset_index: Option<usize>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local HTTP server");
        let address = listener.local_addr().expect("read local server address");
        listener.set_nonblocking(true).expect("nonblocking local listener");
        let worker = thread::spawn(move || {
            let capability_response = format!(
                "<?xml version=\"1.0\"?><llsd><map>\
                 <key>FetchInventoryDescendents2</key><string>http://{address}/descendents</string>\
                 <key>FetchInventory2</key><string>http://{address}/items</string>\
                 <key>ViewerAsset</key><string>http://{address}/asset</string>\
                 </map></llsd>"
            );
            let mut requests = Vec::with_capacity(asset_count + 2);
            for body in [&capability_response, &folder_response] {
                let Some(mut stream) = accept_request(&listener, Instant::now() + Duration::from_secs(2)) else {
                    return (requests, 0);
                };
                requests.push(read_request(&mut stream));
                write_response(&mut stream, body, "application/llsd+xml");
            }

            let active = Arc::new(AtomicUsize::new(0));
            let peak = Arc::new(AtomicUsize::new(0));
            let mut asset_workers = Vec::with_capacity(asset_count);
            let asset_timeout = if failed_asset_index.is_some() { 2 } else { 10 };
            let asset_deadline = Instant::now() + Duration::from_secs(asset_timeout);
            for _ in 0..asset_count {
                let Some(mut stream) = accept_request(&listener, asset_deadline) else {
                    break;
                };
                let active = Arc::clone(&active);
                let peak = Arc::clone(&peak);
                asset_workers.push(thread::spawn(move || {
                    let request = read_request(&mut stream);
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    let index = requested_asset_index(&request);
                    thread::sleep(Duration::from_millis(100 + ((asset_count - index) * 5) as u64));
                    if failed_asset_index == Some(index) {
                        write_status_response(&mut stream, "503 Service Unavailable", "temporary asset failure", "text/plain")
                            .expect("write failed asset response");
                    } else {
                        write_response(&mut stream, "Landmark version 1\nglobal_pos 256000 512000 30\n", "application/octet-stream");
                    }
                    active.fetch_sub(1, Ordering::SeqCst);
                    request
                }));
            }
            requests.extend(asset_workers.into_iter().map(|worker| worker.join().expect("asset worker")));
            (requests, peak.load(Ordering::SeqCst))
        });
        Self { address, worker }
    }

    fn seed_url(&self) -> Url {
        Url::parse(&format!("http://{}/seed", self.address)).expect("valid local URL")
    }

    fn finish(self) -> (Vec<Vec<u8>>, usize) {
        self.worker.join().expect("local HTTP worker should finish")
    }
}

fn requested_asset_index(request: &[u8]) -> usize {
    let request = String::from_utf8_lossy(request);
    let asset_id = request
        .lines()
        .next()
        .and_then(|line| line.split("landmark_id=").nth(1))
        .and_then(|value| value.split_whitespace().next())
        .expect("landmark_id query parameter");
    usize::from_str_radix(asset_id.rsplit('-').next().expect("asset UUID suffix"), 16).expect("asset index")
}

fn accept_request(listener: &TcpListener, deadline: Instant) -> Option<TcpStream> {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).expect("blocking accepted connection");
                return Some(stream);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => return None,
            Err(error) => panic!("accept local request: {error}"),
        }
    }
}

fn write_response(stream: &mut TcpStream, body: &str, content_type: &str) {
    write_status_response(stream, "200 OK", body, content_type).expect("write local response");
}

fn write_status_response(stream: &mut TcpStream, status: &str, body: &str, content_type: &str) -> std::io::Result<()> {
    let response =
        format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    stream.write_all(response.as_bytes())
}

struct LocalServer {
    address: SocketAddr,
    worker: thread::JoinHandle<Vec<Vec<u8>>>,
}

impl LocalServer {
    fn start(folder_response: &'static str, asset_response: Option<&'static str>) -> Self {
        Self::start_with_asset_responses(folder_response.to_owned(), asset_response.into_iter().map(|body| ("200 OK", body)).collect())
    }

    fn start_with_asset_responses(folder_response: String, asset_responses: Vec<(&'static str, &'static str)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local HTTP server");
        let address = listener.local_addr().expect("read local server address");
        let worker = thread::spawn(move || {
            let capability_response = format!(
                "<?xml version=\"1.0\"?><llsd><map>\
                 <key>FetchInventoryDescendents2</key><string>http://{address}/descendents</string>\
                 <key>FetchInventory2</key><string>http://{address}/items</string>\
                 <key>ViewerAsset</key><string>http://{address}/asset</string>\
                 </map></llsd>"
            );
            let mut requests = Vec::new();
            for body in [capability_response.as_str(), folder_response.as_str()] {
                let (mut stream, _) = listener.accept().expect("accept local request");
                let request = read_request(&mut stream);
                write_response(&mut stream, body, "application/llsd+xml");
                requests.push(request);
            }
            for (status, body) in asset_responses {
                let (mut stream, _) = listener.accept().expect("accept local asset request");
                let request = read_request(&mut stream);
                write_status_response(&mut stream, status, body, "application/octet-stream").expect("write local asset response");
                requests.push(request);
            }
            requests
        });
        Self { address, worker }
    }

    fn seed_url(&self) -> Url {
        Url::parse(&format!("http://{}/seed", self.address)).expect("valid local URL")
    }

    fn finish(self) -> Vec<Vec<u8>> {
        self.worker.join().expect("local HTTP worker should finish")
    }
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let size = stream.read(&mut chunk).expect("read local request");
        if size == 0 {
            return request;
        }
        request.extend_from_slice(&chunk[..size]);
        if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = from_utf8(&request[..header_end]).expect("HTTP headers UTF-8");
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
}
