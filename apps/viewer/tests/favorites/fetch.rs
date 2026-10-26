use std::{
    assert_matches,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    str::from_utf8,
    sync::Arc,
    thread,
    time::Duration,
};

use inventory::{Category, CategoryKey, FolderType, Inventory, InventoryLimits, InventoryOwner, ServerVersion};
use llsd::{Value, xml};
use network::capabilities::Client;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener as AsyncTcpListener,
    runtime::Runtime,
    sync::oneshot,
    time::timeout,
};
use url::Url;
use utils::FolderId;
use uuid::Uuid;
use viewer::inventory::favorites::{FavoritesError, LandmarkDestination, favorite_folder_id, fetch_favorite_landmarks};

const AGENT_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const FAVORITES_FOLDER_ID: &str = "11111111-2222-3333-4444-555555555555";
const MAX_CONCURRENT_FETCHES: usize = 3;
const SERVER_TIMEOUT: Duration = Duration::from_secs(10);

#[test]
fn folder_uses_preferred_type() {
    let owner_id = Uuid::parse_str(AGENT_ID).expect("valid agent fixture UUID");
    let root_id = Uuid::from_u128(42);
    let owner = InventoryOwner::new(owner_id);
    let root = Category {
        key: CategoryKey::new(owner, FolderId::new(root_id)),
        parent: None,
        preferred_folder_type: FolderType::RootInventory,
        name: Arc::from("My Inventory"),
        server_version: ServerVersion::UNKNOWN,
        server_descendent_count: None,
    };
    let mut inventory = Inventory::new(root, None, InventoryLimits::default()).expect("valid personal inventory root");
    inventory
        .upsert_category(Category {
            key: CategoryKey::new(owner, FolderId::new(Uuid::parse_str(FAVORITES_FOLDER_ID).expect("valid folder fixture UUID"))),
            parent: Some(CategoryKey::new(owner, FolderId::new(root_id))),
            preferred_folder_type: FolderType::Favorite,
            name: Arc::from("Renamed Folder"),
            server_version: ServerVersion::UNKNOWN,
            server_descendent_count: None,
        })
        .expect("add category from skeleton");

    assert_eq!(favorite_folder_id(&inventory.snapshot()).expect("favorite type should resolve").to_string(), FAVORITES_FOLDER_ID);
}

#[test]
fn empty_folder_requests_items() {
    let runtime = Runtime::new().expect("create local Tokio runtime");
    let server = LocalServer::start(include_str!("../fixtures/favorites_empty_folder.xml"), None);
    let seed = server.seed_url();
    let capabilities = runtime.block_on(async { Client::new(seed).expect("active Tokio runtime") });
    let agent_id = Uuid::parse_str(AGENT_ID).expect("valid fixture UUID");
    let folder_id = Uuid::parse_str(FAVORITES_FOLDER_ID).expect("valid fixture UUID");

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
    assert_eq!(folder.get("folder_id"), Some(&Value::Uuid(folder_id)));
    assert_eq!(folder.get("owner_id"), Some(&Value::Uuid(agent_id)));
    assert_eq!(folder.get("sort_order"), Some(&Value::Integer(0)));
    assert_eq!(folder.get("fetch_folders"), Some(&Value::Boolean(true)));
    assert_eq!(folder.get("fetch_items"), Some(&Value::Boolean(true)));
}

#[test]
fn missing_items_rejected() {
    let runtime = Runtime::new().expect("create local Tokio runtime");
    let server = LocalServer::start(include_str!("../fixtures/favorites_incomplete_folder.xml"), None);
    let seed = server.seed_url();
    let capabilities = runtime.block_on(async { Client::new(seed).expect("active Tokio runtime") });
    let agent_id = Uuid::parse_str(AGENT_ID).expect("valid fixture UUID");
    let folder_id = Uuid::parse_str(FAVORITES_FOLDER_ID).expect("valid fixture UUID");

    let error = runtime
        .block_on(fetch_favorite_landmarks(&capabilities, agent_id, folder_id))
        .expect_err("missing items with a nonempty folder is malformed");
    assert_matches!(error, FavoritesError::InvalidFavoritesItemList);
    assert_eq!(error.to_string(), "Second Life returned an invalid Favorites item list.");
    let _ = server.finish();
}

#[test]
fn landmark_asset_parsed() {
    let runtime = Runtime::new().expect("create local Tokio runtime");
    let server = LocalServer::start(
        include_str!("../fixtures/favorites_landmark_folder.xml"),
        Some("Landmark version 1\nglobal_pos 256000 512000 30\n"),
    );
    let seed = server.seed_url();
    let capabilities = runtime.block_on(async { Client::new(seed).expect("active Tokio runtime") });
    let agent_id = Uuid::parse_str(AGENT_ID).expect("valid fixture UUID");
    let folder_id = Uuid::parse_str(FAVORITES_FOLDER_ID).expect("valid fixture UUID");

    let landmarks = runtime.block_on(fetch_favorite_landmarks(&capabilities, agent_id, folder_id)).expect("landmark fixture is valid");
    assert_eq!(landmarks.len(), 1);
    assert_eq!(landmarks[0].name(), "Test Landmark");
    assert_eq!(landmarks[0].destination(), LandmarkDestination::Global([256000.0, 512000.0, 30.0]));

    let requests = server.finish();
    assert_eq!(requests.len(), 3);
    assert!(String::from_utf8_lossy(&requests[2]).starts_with("GET /asset?landmark_id="));
}

#[test]
fn landmark_asset_request_failure_rejects_partial_results() {
    const ASSET_COUNT: usize = 16;

    let runtime = Runtime::new().expect("create local Tokio runtime");
    let agent_id = Uuid::parse_str(AGENT_ID).expect("valid fixture UUID");
    let folder_id = Uuid::parse_str(FAVORITES_FOLDER_ID).expect("valid fixture UUID");

    let (result, requests, peak_fetches) = runtime.block_on(async {
        let mut server = ConcurrentAssetServer::start_with_failure(landmark_folder_response(ASSET_COUNT), ASSET_COUNT, Some(0)).await;
        let capabilities = Client::new(server.seed_url()).expect("active Tokio runtime");
        let fetch_capabilities = capabilities.clone();
        let fetch = tokio::spawn(async move { fetch_favorite_landmarks(&fetch_capabilities, agent_id, folder_id).await });
        server.first_batch_size().await;
        let result = fetch.await.expect("landmark fetch task");
        let (requests, peak_fetches) = server.finish().await;
        (result, requests, peak_fetches)
    });

    let error = result.expect_err("an asset request failure must reject the incomplete snapshot");
    assert_matches!(&error, FavoritesError::FetchLandmarkAsset { .. });
    assert!(error.to_string().contains("Could not fetch favorite landmark asset"), "unexpected error: {error}");

    let asset_request_count = requests.len().saturating_sub(2);
    assert!(asset_request_count > 0);
    assert_eq!(peak_fetches, MAX_CONCURRENT_FETCHES, "the first batch should reach the configured concurrency");
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
    let agent_id = Uuid::parse_str(AGENT_ID).expect("valid fixture UUID");
    let folder_id = Uuid::parse_str(FAVORITES_FOLDER_ID).expect("valid fixture UUID");

    let landmarks = runtime
        .block_on(fetch_favorite_landmarks(&capabilities, agent_id, folder_id))
        .expect("one unreadable asset must not discard a readable landmark");
    assert_eq!(landmarks.len(), 1);

    let requests = server.finish();
    assert_eq!(requests.len(), 4);
    let valid_asset_index = requested_asset_index(&requests[2]);
    assert_eq!(landmarks[0].name(), format!("Landmark {valid_asset_index:02}"));
    assert_eq!(landmarks[0].destination(), LandmarkDestination::Global([256000.0, 512000.0, 30.0]));
}

#[test]
fn landmark_fetches_overlap_and_keep_folder_order() {
    const ASSET_COUNT: usize = 16;

    let runtime = Runtime::new().expect("create local Tokio runtime");
    let folder_response = landmark_folder_response(ASSET_COUNT);
    let agent_id = Uuid::parse_str(AGENT_ID).expect("valid fixture UUID");
    let folder_id = Uuid::parse_str(FAVORITES_FOLDER_ID).expect("valid fixture UUID");

    let (landmarks, requests, peak_fetches) = runtime.block_on(async {
        let mut server = ConcurrentAssetServer::start(folder_response, ASSET_COUNT).await;
        let capabilities = Client::new(server.seed_url()).expect("active Tokio runtime");
        let fetch_capabilities = capabilities.clone();
        let fetch = tokio::spawn(async move { fetch_favorite_landmarks(&fetch_capabilities, agent_id, folder_id).await });
        server.first_batch_size().await;
        let landmarks = fetch.await.expect("landmark fetch task");
        let (requests, peak_fetches) = server.finish().await;
        (landmarks, requests, peak_fetches)
    });

    let landmarks = landmarks.expect("landmark fixtures are valid");
    assert_eq!(landmarks.len(), ASSET_COUNT);
    for (index, landmark) in landmarks.iter().enumerate() {
        assert_eq!(landmark.name(), format!("Landmark {index:02}"));
        assert_eq!(landmark.destination(), LandmarkDestination::Global([256000.0 + index as f64, 512000.0, 30.0]));
    }

    assert_eq!(requests.len(), ASSET_COUNT + 2);
    assert_eq!(peak_fetches, MAX_CONCURRENT_FETCHES, "asset requests should fill the initial concurrent batch");
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
    first_batch: oneshot::Receiver<usize>,
    worker: tokio::task::JoinHandle<(Vec<Vec<u8>>, usize)>,
}

impl ConcurrentAssetServer {
    async fn start(folder_response: String, asset_count: usize) -> Self {
        Self::start_with_failure(folder_response, asset_count, None).await
    }

    async fn start_with_failure(folder_response: String, asset_count: usize, failed_asset_index: Option<usize>) -> Self {
        let listener = AsyncTcpListener::bind("127.0.0.1:0").await.expect("bind local HTTP server");
        let address = listener.local_addr().expect("read local server address");
        let (first_batch_sender, first_batch) = oneshot::channel();
        let worker = tokio::spawn(async move {
            let capability_response = format!(
                "<?xml version=\"1.0\"?><llsd><map>\
                 <key>FetchInventoryDescendents2</key><string>http://{address}/descendents</string>\
                 <key>FetchInventory2</key><string>http://{address}/items</string>\
                 <key>ViewerAsset</key><string>http://{address}/asset</string>\
                 </map></llsd>"
            );
            let mut requests = Vec::with_capacity(asset_count + 2);
            for body in [&capability_response, &folder_response] {
                let (mut stream, _) = listener.accept().await.expect("accept local capability request");
                requests.push(read_async_request(&mut stream).await);
                write_async_response(&mut stream, body, "application/llsd+xml").await.expect("write local capability response");
            }

            let mut batch = Vec::with_capacity(MAX_CONCURRENT_FETCHES);
            let mut peak = 0;
            if asset_count > 0 {
                while batch.len() < asset_count.min(MAX_CONCURRENT_FETCHES) {
                    let (mut stream, _) = listener.accept().await.expect("accept landmark asset request");
                    let request = read_async_request(&mut stream).await;
                    let index = requested_asset_index(&request);
                    batch.push((stream, request, index));
                }

                peak = batch.len();
                let _ = first_batch_sender.send(peak);
                let batch_size = batch.len();

                if let Some(failed_index) = failed_asset_index.filter(|failed| batch.iter().any(|(_, _, index)| index == failed)) {
                    let (stream, _, _) = batch.iter_mut().find(|(_, _, index)| *index == failed_index).expect("failed request in batch");
                    write_async_status_response(stream, "503 Service Unavailable", "temporary asset failure", "text/plain")
                        .await
                        .expect("write failed asset response");
                    requests.extend(batch.into_iter().map(|(_, request, _)| request));
                    return (requests, peak);
                }

                for (mut stream, request, index) in batch {
                    requests.push(request);
                    let body = format!("Landmark version 1\nglobal_pos {} 512000 30\n", 256000 + index);
                    write_async_response(&mut stream, &body, "application/octet-stream").await.expect("write local asset response");
                }

                for _ in batch_size..asset_count {
                    let (mut stream, _) = listener.accept().await.expect("accept local asset request");
                    let request = read_async_request(&mut stream).await;
                    let index = requested_asset_index(&request);
                    requests.push(request);
                    if failed_asset_index == Some(index) {
                        write_async_status_response(&mut stream, "503 Service Unavailable", "temporary asset failure", "text/plain")
                            .await
                            .expect("write failed asset response");
                        return (requests, peak);
                    }
                    let body = format!("Landmark version 1\nglobal_pos {} 512000 30\n", 256000 + index);
                    write_async_response(&mut stream, &body, "application/octet-stream").await.expect("write local asset response");
                }
            }
            (requests, peak)
        });
        Self { address, first_batch, worker }
    }

    fn seed_url(&self) -> Url {
        Url::parse(&format!("http://{}/seed", self.address)).expect("valid local URL")
    }

    async fn first_batch_size(&mut self) -> usize {
        timeout(SERVER_TIMEOUT, &mut self.first_batch)
            .await
            .expect("initial asset requests should reach the server before the failure deadline")
            .expect("server should report its first asset batch")
    }

    async fn finish(self) -> (Vec<Vec<u8>>, usize) {
        let mut worker = self.worker;
        match timeout(SERVER_TIMEOUT, &mut worker).await {
            Ok(result) => result.expect("local HTTP worker should finish"),
            Err(_) => {
                worker.abort();
                let error = worker.await.expect_err("timed-out local HTTP worker should be cancelled");
                assert!(error.is_cancelled(), "local HTTP worker stopped unexpectedly: {error}");
                panic!("local HTTP worker did not finish before the failure deadline");
            }
        }
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

fn write_response(stream: &mut TcpStream, body: &str, content_type: &str) {
    write_status_response(stream, "200 OK", body, content_type).expect("write local response");
}

fn write_status_response(stream: &mut TcpStream, status: &str, body: &str, content_type: &str) -> std::io::Result<()> {
    let response =
        format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    stream.write_all(response.as_bytes())
}

async fn write_async_response(stream: &mut tokio::net::TcpStream, body: &str, content_type: &str) -> std::io::Result<()> {
    write_async_status_response(stream, "200 OK", body, content_type).await
}

async fn write_async_status_response(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    body: &str,
    content_type: &str,
) -> std::io::Result<()> {
    let response =
        format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    stream.write_all(response.as_bytes()).await
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
        if request_is_complete(&request) {
            return request;
        }
    }
}

async fn read_async_request(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let size = stream.read(&mut chunk).await.expect("read local request");
        if size == 0 {
            return request;
        }
        request.extend_from_slice(&chunk[..size]);
        if request_is_complete(&request) {
            return request;
        }
    }
}

fn request_is_complete(request: &[u8]) -> bool {
    let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") else {
        return false;
    };
    let headers = from_utf8(&request[..header_end]).expect("HTTP headers UTF-8");
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
        })
        .unwrap_or(0);
    request.len() >= header_end + 4 + content_length
}
