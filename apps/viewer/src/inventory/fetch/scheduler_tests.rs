use std::{
    collections::HashSet,
    fmt::Write as _,
    io,
    sync::Arc,
    time::{Duration, Instant},
};

use ::inventory::{
    Category, CategoryKey, FolderType, Inventory, InventoryDelta, InventoryLimits, InventoryOwner, MAX_FOLDER_FETCH_CHILDREN,
    MAX_INVENTORY_ITEMS, ServerVersion,
};
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use llsd::{Map, Value, xml};
use network::capabilities::{CapabilityUrl, Client};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::timeout,
};
use url::Url;
use utils::FolderId;
use uuid::Uuid;

use super::{
    BACKGROUND_CONCURRENCY, ForegroundRequest, InventoryFetchHandle, InventoryFetchOutcome, InventoryLoadingStatus, MAX_AIS_DEPTH,
    REQUEST_CAPACITY, RESULT_CAPACITY, fetch_ais, run_scheduler,
};
use crate::inventory::fetch::{FolderApplyOutcome, FolderFetch, InventoryFetchError, InventoryFetchEvent, MAX_FETCH_RESPONSE_BYTES, ais};

const TEST_TIMEOUT: Duration = Duration::from_secs(45);
type SchedulerTask = (
    mpsc::Sender<ForegroundRequest>,
    mpsc::Receiver<InventoryFetchEvent>,
    watch::Sender<Option<::inventory::InventorySnapshot>>,
    JoinHandle<()>,
    oneshot::Sender<()>,
    oneshot::Sender<()>,
);

struct PeerRequest {
    method: String,
    path: String,
    headers: String,
    body: Vec<u8>,
    stream: TcpStream,
}

impl PeerRequest {
    async fn respond(self, status: u16, body: impl AsRef<[u8]>) {
        self.respond_with_headers(status, body, &[]).await;
    }

    async fn respond_with_headers(self, status: u16, body: impl AsRef<[u8]>, headers: &[(&str, &str)]) {
        let body = body.as_ref();
        let reason = match status {
            200 => "OK",
            403 => "Forbidden",
            404 => "Not Found",
            410 => "Gone",
            503 => "Service Unavailable",
            _ => "Response",
        };
        let mut response = format!("HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
        for (name, value) in headers {
            response.push_str(name);
            response.push_str(": ");
            response.push_str(value);
            response.push_str("\r\n");
        }
        response.push_str("\r\n");
        let mut stream = self.stream;
        stream.write_all(response.as_bytes()).await.expect("write peer response headers");
        stream.write_all(body).await.expect("write peer response body");
    }

    async fn wait_for_close(mut self) {
        let mut byte = [0_u8; 1];
        let result = timeout(TEST_TIMEOUT, self.stream.read(&mut byte)).await.expect("peer close timeout");
        match result {
            Ok(0) => {}
            Err(error) if matches!(error.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe) => {}
            result => panic!("expected client to close the in-flight request, received {result:?}"),
        }
    }
}

struct LoopbackPeer {
    base: Url,
    requests: mpsc::Receiver<PeerRequest>,
    task: JoinHandle<()>,
}

impl LoopbackPeer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback peer");
        let address = listener.local_addr().expect("read loopback address");
        let base = Url::parse(&format!("http://{address}")).expect("build peer URL");
        let (sender, requests) = mpsc::channel(16);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let sender = sender.clone();
                tokio::spawn(async move {
                    if let Ok(request) = read_request(stream).await {
                        match sender.send(request).await {
                            Ok(()) | Err(_) => {}
                        }
                    }
                });
            }
        });
        Self { base, requests, task }
    }

    fn url(&self, path: &str) -> Url {
        self.base.join(path).expect("join peer path")
    }

    async fn next(&mut self) -> PeerRequest {
        timeout(TEST_TIMEOUT, self.requests.recv()).await.expect("peer request timeout").expect("peer listener closed")
    }

    async fn grant(&mut self, names: &[(&str, &str)]) {
        let request = self.next().await;
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/seed");
        let value = Value::Map(names.iter().map(|(name, path)| ((*name).to_owned(), Value::String(self.url(path).to_string()))).collect());
        let body = xml::to_string(&value).expect("encode capability grants");
        request.respond(200, body).await;
    }
}

impl Drop for LoopbackPeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_request(mut stream: TcpStream) -> io::Result<PeerRequest> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    let (header_end, content_length) = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let header_end = end + 4;
            let headers = std::str::from_utf8(&bytes[..end]).map_err(io::Error::other)?;
            let content_length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>())
                .transpose()
                .map_err(io::Error::other)?
                .unwrap_or_default();
            break (header_end, content_length);
        }
    };
    while bytes.len() < header_end.saturating_add(content_length) {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    let headers = std::str::from_utf8(&bytes[..header_end - 4]).map_err(io::Error::other)?.to_owned();
    let mut line = headers.lines().next().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?.split_whitespace();
    let method = line.next().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?.to_owned();
    let path = line.next().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?.to_owned();
    Ok(PeerRequest { method, path, headers, body: bytes[header_end..header_end + content_length].to_vec(), stream })
}

fn key(owner: InventoryOwner, id: u128) -> CategoryKey {
    CategoryKey::new(owner, FolderId::new(Uuid::from_u128(id)))
}

fn category(key: CategoryKey, parent: Option<CategoryKey>) -> Category {
    Category {
        key,
        parent,
        preferred_folder_type: FolderType::None,
        name: Arc::from(format!("folder-{}", key.id().uuid().as_u128())),
        server_version: ServerVersion::UNKNOWN,
        server_descendent_count: None,
    }
}

fn inventory_with_children() -> (Inventory, CategoryKey, CategoryKey, CategoryKey) {
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = key(owner, 1);
    let first = key(owner, 2);
    let second = key(owner, 3);
    let mut inventory = Inventory::new(category(root, None), None, InventoryLimits::default()).expect("valid inventory root");
    inventory.upsert_category(category(first, Some(root))).expect("add first child folder");
    inventory.upsert_category(category(second, Some(root))).expect("add second child folder");
    (inventory, root, first, second)
}

fn root_response(root: CategoryKey, children: &[CategoryKey], child_count: u32) -> Vec<u8> {
    let categories = children
        .iter()
        .map(|child| {
            Value::Map(Map::from([
                ("category_id".into(), Value::Uuid(child.id().uuid())),
                ("name".into(), Value::String(format!("folder-{}", child.id().uuid().as_u128()))),
                ("parent_id".into(), Value::Uuid(root.id().uuid())),
                ("version".into(), Value::Integer(1)),
                ("descendents".into(), Value::Integer(child_count as i32)),
            ]))
        })
        .collect::<Vec<_>>();
    let response = Value::Map(Map::from([
        ("category_id".into(), Value::Uuid(root.id().uuid())),
        ("name".into(), Value::String(format!("folder-{}", root.id().uuid().as_u128()))),
        ("version".into(), Value::Integer(2)),
        ("descendents".into(), Value::Integer(children.len() as i32)),
        ("_embedded".into(), Value::Map(Map::from([("categories".into(), Value::Array(categories))]))),
    ]));
    xml::to_vec(&response).expect("encode root response")
}

fn root_response_with_child_version(root: CategoryKey, child: CategoryKey, version: i32, child_count: u32) -> Vec<u8> {
    let child = Value::Map(Map::from([
        ("category_id".into(), Value::Uuid(child.id().uuid())),
        ("name".into(), Value::String(format!("folder-{}", child.id().uuid().as_u128()))),
        ("parent_id".into(), Value::Uuid(root.id().uuid())),
        ("version".into(), Value::Integer(version)),
        ("descendents".into(), Value::Integer(child_count as i32)),
    ]));
    let response = Value::Map(Map::from([
        ("category_id".into(), Value::Uuid(root.id().uuid())),
        ("name".into(), Value::String(format!("folder-{}", root.id().uuid().as_u128()))),
        ("version".into(), Value::Integer(2)),
        ("descendents".into(), Value::Integer(1)),
        ("_embedded".into(), Value::Map(Map::from([("categories".into(), Value::Array(vec![child]))]))),
    ]));
    xml::to_vec(&response).expect("encode parent response with updated child metadata")
}

fn empty_folder_response(folder: CategoryKey, version: i32) -> Vec<u8> {
    let response = Value::Map(Map::from([
        ("category_id".into(), Value::Uuid(folder.id().uuid())),
        ("name".into(), Value::String(format!("folder-{}", folder.id().uuid().as_u128()))),
        ("version".into(), Value::Integer(version)),
        ("descendents".into(), Value::Integer(0)),
        (
            "_embedded".into(),
            Value::Map(Map::from([
                ("categories".into(), Value::Array(Vec::new())),
                ("items".into(), Value::Array(Vec::new())),
                ("links".into(), Value::Array(Vec::new())),
            ])),
        ),
    ]));
    xml::to_vec(&response).expect("encode empty folder response")
}

fn full_folder_response(folder: CategoryKey, item_count: usize, id_prefix: u128) -> Vec<u8> {
    assert!(item_count <= MAX_FOLDER_FETCH_CHILDREN);
    let mut body = String::with_capacity(item_count.saturating_mul(240).saturating_add(256));
    write!(
        body,
        "<llsd><map><key>category_id</key><uuid>{}</uuid><key>name</key><string>large</string><key>version</key><integer>1</integer><key>descendents</key><integer>{item_count}</integer><key>_embedded</key><map><key>items</key><array>",
        folder.id().uuid()
    )
    .expect("write large-folder header");
    let parent_id = folder.id().uuid();
    for index in 0..item_count {
        let item_id = Uuid::from_u128(id_prefix + index as u128);
        write!(body, "<map><key>item_id</key><uuid>{item_id}</uuid><key>parent_id</key><uuid>{parent_id}</uuid><key>name</key><string>i</string><key>type</key><integer>7</integer><key>inv_type</key><integer>7</integer></map>")
            .expect("write large-folder item");
    }
    body.push_str("</array></map></map></llsd>");
    body.into_bytes()
}

fn llsd_folder(folder: CategoryKey) -> Value {
    llsd_folder_with_children(folder, &[])
}

fn llsd_folder_with_children(folder: CategoryKey, children: &[CategoryKey]) -> Value {
    Value::Map(Map::from([
        ("folder_id".into(), Value::Uuid(folder.id().uuid())),
        ("owner_id".into(), Value::Uuid(folder.owner().id())),
        ("version".into(), Value::Integer(4)),
        ("descendents".into(), Value::Integer(children.len() as i32)),
        ("categories".into(), Value::Array(children.iter().map(|child| llsd_category(*child, folder)).collect())),
        ("items".into(), Value::Array(Vec::new())),
    ]))
}

fn llsd_category(folder: CategoryKey, parent: CategoryKey) -> Value {
    Value::Map(Map::from([
        ("folder_id".into(), Value::Uuid(folder.id().uuid())),
        ("owner_id".into(), Value::Uuid(folder.owner().id())),
        ("parent_id".into(), Value::Uuid(parent.id().uuid())),
        ("name".into(), Value::String(format!("folder-{}", folder.id().uuid().as_u128()))),
        ("version".into(), Value::Integer(1)),
        ("descendents".into(), Value::Integer(0)),
        ("type_default".into(), Value::Integer(-1)),
    ]))
}

fn scheduler(client: Client, snapshot: ::inventory::InventorySnapshot) -> SchedulerTask {
    let (requests_tx, requests_rx) = mpsc::channel(REQUEST_CAPACITY);
    let (events_tx, events_rx) = mpsc::channel(RESULT_CAPACITY);
    let (model_tx, model_rx) = watch::channel(Some(snapshot.clone()));
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let cancellation: Shared<BoxFuture<'static, ()>> = async move {
        drop(cancel_rx.await);
    }
    .boxed()
    .shared();
    let (stop_tx, stop_rx) = oneshot::channel();
    let task = tokio::spawn(run_scheduler(client, snapshot, model_rx, requests_rx, events_tx, cancellation, stop_rx));
    (requests_tx, events_rx, model_tx, task, cancel_tx, stop_tx)
}

async fn next_event(events: &mut mpsc::Receiver<InventoryFetchEvent>) -> InventoryFetchEvent {
    timeout(TEST_TIMEOUT, events.recv()).await.expect("fetch event timeout").expect("fetch event channel closed")
}

fn take_folder_event(event: InventoryFetchEvent, expected: CategoryKey) -> (FolderFetch, oneshot::Sender<FolderApplyOutcome>) {
    match event {
        InventoryFetchEvent::Folder { fetch, applied } => {
            assert_eq!(fetch.folder, expected);
            (fetch, applied)
        }
        event => panic!("expected folder response, received {event:?}"),
    }
}

fn apply_and_acknowledge(
    inventory: &mut Inventory,
    model: &watch::Sender<Option<::inventory::InventorySnapshot>>,
    fetch: FolderFetch,
    applied: oneshot::Sender<FolderApplyOutcome>,
) -> Result<(), ::inventory::InventoryError> {
    let result = inventory.apply_folder_fetch(fetch.folder, fetch.version, fetch.descendent_count, fetch.children);
    if result.is_ok() {
        model.send_replace(Some(inventory.snapshot()));
    }
    applied.send(FolderApplyOutcome { rejected: result.is_err() }).expect("scheduler waits for folder apply outcome");
    result.map(|_| ())
}

#[tokio::test]
async fn ais_recursively_decodes_folders_items_and_hypermedia_metadata() {
    let mut peer = LoopbackPeer::start().await;
    let client = Client::new(peer.url("/seed")).expect("active Tokio runtime");
    let capability = CapabilityUrl::parse(peer.url("/ais").as_str()).expect("valid AIS URL");
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = key(owner, 10);
    let child = key(owner, 11);
    let item = Value::Map(Map::from([
        ("item_id".into(), Value::Uuid(Uuid::from_u128(12))),
        ("parent_id".into(), Value::Uuid(child.id().uuid())),
        ("name".into(), Value::String("card".into())),
        ("desc".into(), Value::String("sample".into())),
        ("type".into(), Value::Integer(7)),
        ("inv_type".into(), Value::Integer(7)),
    ]));
    let child_category = Value::Map(Map::from([
        ("category_id".into(), Value::Uuid(child.id().uuid())),
        ("parent_id".into(), Value::Uuid(root.id().uuid())),
        ("name".into(), Value::String("child".into())),
        ("version".into(), Value::Integer(4)),
        ("descendents".into(), Value::Integer(1)),
        ("_base_uri".into(), Value::String("https://inventory.example/".into())),
        (
            "_links".into(),
            Value::Map(Map::from([("self".into(), Value::Map(Map::from([("href".into(), Value::String("/category/child".into()))])))])),
        ),
        (
            "_embedded".into(),
            Value::Map(Map::from([("items".into(), Value::Array(vec![item])), ("links".into(), Value::Array(Vec::new()))])),
        ),
    ]));
    let body = Value::Map(Map::from([
        ("category_id".into(), Value::Uuid(root.id().uuid())),
        ("name".into(), Value::String("root".into())),
        ("version".into(), Value::Integer(3)),
        ("descendents".into(), Value::Integer(1)),
        ("_base_uri".into(), Value::String("https://inventory.example/".into())),
        (
            "_links".into(),
            Value::Map(Map::from([("self".into(), Value::Map(Map::from([("href".into(), Value::String("/category/root".into()))])))])),
        ),
        ("_embedded".into(), Value::Map(Map::from([("categories".into(), Value::Array(vec![child_category]))]))),
    ]));
    let response = xml::to_vec(&body).expect("encode AIS fixture");
    let responder = tokio::spawn(async move {
        let request = peer.next().await;
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, format!("/ais/category/{}/children?depth=16", root.id().uuid()));
        assert!(request.headers.to_ascii_lowercase().contains("accept: application/llsd+xml"));
        request.respond(200, response).await;
    });

    let folders = ais::fetch_once(&client, &capability, root, MAX_AIS_DEPTH).await.expect("recursive AIS response");
    responder.await.expect("peer responder");
    assert_eq!(folders.len(), 2);
    assert_eq!(folders[0].folder, root);
    assert_eq!(folders[0].children.len(), 1);
    assert_eq!(folders[1].folder, child);
    assert_eq!(folders[1].children.len(), 1);
}

#[tokio::test]
async fn ais_item_endpoint_decodes_a_link_target_only_when_requested() {
    let mut peer = LoopbackPeer::start().await;
    let client = Client::new(peer.url("/seed")).expect("active Tokio runtime");
    let capability = CapabilityUrl::parse(peer.url("/ais").as_str()).expect("valid AIS URL");
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let parent = key(owner, 15);
    let item_id = Uuid::from_u128(16);
    let body = xml::to_vec(&Value::Map(Map::from([
        ("item_id".into(), Value::Uuid(item_id)),
        ("parent_id".into(), Value::Uuid(parent.id().uuid())),
        ("owner_id".into(), Value::Uuid(owner.id())),
        ("name".into(), Value::String("linked landmark".into())),
        ("asset_id".into(), Value::Uuid(Uuid::from_u128(17))),
        ("type".into(), Value::Integer(3)),
        ("inv_type".into(), Value::Integer(3)),
        ("_base_uri".into(), Value::String("https://inventory.example/".into())),
        (
            "_links".into(),
            Value::Map(Map::from([("self".into(), Value::Map(Map::from([("href".into(), Value::String("/item/target".into()))])))])),
        ),
    ])))
    .expect("encode AIS item fixture");
    let responder = tokio::spawn(async move {
        let request = peer.next().await;
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, format!("/ais/item/{item_id}"));
        request.respond(200, body).await;
    });

    let item = ais::fetch_item_once(&client, &capability, owner, item_id).await.expect("AIS linked item");
    responder.await.expect("peer responder");
    assert_eq!(item.key.id().uuid(), item_id);
    assert_eq!(item.parent, parent);
    assert_eq!(item.name.as_ref(), "linked landmark");
}

#[tokio::test]
async fn fallback_uses_personal_and_library_descendents_capabilities() {
    let mut peer = LoopbackPeer::start().await;
    let client = Client::new(peer.url("/seed")).expect("active Tokio runtime");
    let personal_owner = InventoryOwner::new(Uuid::from_u128(1));
    let library_owner = InventoryOwner::new(Uuid::from_u128(2));
    let personal_root = key(personal_owner, 10);
    let library_root = key(library_owner, 20);
    let mut inventory = Inventory::new(category(personal_root, None), Some(category(library_root, None)), InventoryLimits::default())
        .expect("valid inventory roots");
    let (requests, mut events, model, task, _cancel, stop) = scheduler(client, inventory.snapshot());
    peer.grant(&[("FetchInventoryDescendents2", "/personal"), ("FetchLibDescendents2", "/library")]).await;

    let first = peer.next().await;
    let second = peer.next().await;
    let mut root_paths = [first.path.as_str(), second.path.as_str()];
    root_paths.sort_unstable();
    assert_eq!(root_paths, ["/library", "/personal"]);
    assert_eq!(first.method, "POST");
    assert_eq!(second.method, "POST");
    for request in [first, second] {
        let payload: Value = xml::from_slice_with_limits(&request.body, llsd::Limits::default()).expect("decode LLSD request");
        let folders = payload.as_map().and_then(|fields| fields.get("folders")).and_then(Value::as_array).expect("folders array");
        assert_eq!(folders.len(), 1);
        assert!(folders.len() <= 10);
        let fields = folders[0].as_map().expect("folder request map");
        assert!(fields.contains_key("folder_id"));
        assert!(fields.contains_key("owner_id"));
        let path = request.path.clone();
        let folder = if path == "/personal" { personal_root } else { library_root };
        let response = Value::Map(Map::from([("folders".into(), Value::Array(vec![llsd_folder(folder)]))]));
        request.respond(200, xml::to_string(&response).expect("encode fallback response")).await;
    }
    let mut applied = HashSet::new();
    loop {
        match next_event(&mut events).await {
            InventoryFetchEvent::Folder { fetch, applied: response } => {
                let folder = fetch.folder;
                apply_and_acknowledge(&mut inventory, &model, fetch, response).expect("apply fallback folder response");
                applied.insert(folder);
            }
            InventoryFetchEvent::BackgroundDone => break,
            InventoryFetchEvent::Failed { folder, error } => panic!("fallback failed for {folder:?}: {error}"),
        }
    }
    assert_eq!(applied, HashSet::from([personal_root, library_root]));
    drop(requests);
    stop.send(()).expect("stop fallback scheduler");
    task.await.expect("scheduler stopped");
}

#[tokio::test]
async fn fallback_batches_no_more_than_ten_folders_per_request() {
    let mut peer = LoopbackPeer::start().await;
    let client = Client::new(peer.url("/seed")).expect("active Tokio runtime");
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = key(owner, 70);
    let children = (71..82).map(|id| key(owner, id)).collect::<Vec<_>>();
    let mut root_category = category(root, None);
    root_category.server_version = ServerVersion::from_raw(1);
    root_category.server_descendent_count = Some(children.len() as u32);
    let mut inventory = Inventory::new(root_category, None, InventoryLimits::default()).expect("valid inventory root");
    for child in &children {
        inventory.upsert_category(category(*child, Some(root))).expect("add fallback child");
    }
    let (requests, mut events, model, task, _cancel, stop) = scheduler(client, inventory.snapshot());
    peer.grant(&[("FetchInventoryDescendents2", "/personal")]).await;

    let first_batch = peer.next().await;
    let second_batch = peer.next().await;
    let mut loaded = HashSet::new();
    for request in [first_batch, second_batch] {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/personal");
        let payload: Value = xml::from_slice_with_limits(&request.body, llsd::Limits::default()).expect("decode batch request");
        let requested = payload.as_map().and_then(|fields| fields.get("folders")).and_then(Value::as_array).expect("folder batch");
        assert!(requested.len() <= 10);
        assert!(!requested.is_empty());
        for value in requested {
            let fields = value.as_map().expect("folder request entry");
            let id = fields.get("folder_id").and_then(Value::as_uuid).expect("requested folder ID");
            loaded.insert(id);
        }
        let folders = requested
            .iter()
            .map(|value| {
                let fields = value.as_map().expect("folder request entry");
                let id = fields.get("folder_id").and_then(Value::as_uuid).expect("requested folder ID");
                llsd_folder(key(owner, id.as_u128()))
            })
            .collect();
        let response = Value::Map(Map::from([("folders".into(), Value::Array(folders))]));
        request.respond(200, xml::to_string(&response).expect("encode child fallback response")).await;
    }
    assert_eq!(loaded, children.iter().map(|child| child.id().uuid()).collect());

    let mut completed = HashSet::new();
    loop {
        match next_event(&mut events).await {
            InventoryFetchEvent::Folder { fetch, applied: response } => {
                let folder = fetch.folder;
                apply_and_acknowledge(&mut inventory, &model, fetch, response).expect("apply fallback batch folder response");
                completed.insert(folder);
            }
            InventoryFetchEvent::BackgroundDone => break,
            InventoryFetchEvent::Failed { folder, error } => panic!("fallback failed for {folder:?}: {error}"),
        }
    }
    assert_eq!(completed, children.into_iter().collect());
    drop(requests);
    stop.send(()).expect("stop batched fallback scheduler");
    task.await.expect("scheduler stopped");
}

#[tokio::test]
async fn recursive_403_reduces_depth_until_the_server_accepts_the_request() {
    let mut peer = LoopbackPeer::start().await;
    let client = Client::new(peer.url("/seed")).expect("active Tokio runtime");
    let capability = CapabilityUrl::parse(peer.url("/ais").as_str()).expect("valid AIS URL");
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = key(owner, 30);
    let successful_body = empty_folder_response(root, 1);
    let responder = tokio::spawn(async move {
        for depth in [16, 8, 4, 2, 1, 0] {
            let request = peer.next().await;
            assert_eq!(request.path, format!("/ais/category/{}/children?depth={depth}", root.id().uuid()));
            if depth == 0 {
                request.respond(200, successful_body.clone()).await;
            } else {
                request.respond(403, "").await;
            }
        }
    });

    let fetched = fetch_ais(&client, &capability, root, MAX_AIS_DEPTH).await.expect("reduced-depth request");
    responder.await.expect("peer responder");
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].folder, root);
}

#[tokio::test]
async fn a_depth_zero_403_is_reported_as_a_too_large_folder() {
    let mut peer = LoopbackPeer::start().await;
    let client = Client::new(peer.url("/seed")).expect("active Tokio runtime");
    let capability = CapabilityUrl::parse(peer.url("/ais").as_str()).expect("valid AIS URL");
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = key(owner, 31);
    let responder = tokio::spawn(async move {
        let request = peer.next().await;
        assert_eq!(request.path, format!("/ais/category/{}/children?depth=0", root.id().uuid()));
        request.respond(403, "").await;
    });

    std::assert_matches!(fetch_ais(&client, &capability, root, 0).await, Err(InventoryFetchError::FolderTooLarge));
    responder.await.expect("peer responder");
}

#[tokio::test]
async fn transient_http_failure_retries_and_succeeds() {
    let mut peer = LoopbackPeer::start().await;
    let client = Client::new(peer.url("/seed")).expect("active Tokio runtime");
    let capability = CapabilityUrl::parse(peer.url("/ais").as_str()).expect("valid AIS URL");
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = key(owner, 40);
    let successful_body = empty_folder_response(root, 1);
    let responder = tokio::spawn(async move {
        let first = peer.next().await;
        assert_eq!(first.path, format!("/ais/category/{}/children?depth=0", root.id().uuid()));
        first.respond_with_headers(429, "", &[("Retry-After", "0")]).await;
        let second = peer.next().await;
        assert_eq!(second.path, format!("/ais/category/{}/children?depth=0", root.id().uuid()));
        second.respond_with_headers(503, "", &[("Retry-After", "0")]).await;
        let third = peer.next().await;
        assert_eq!(third.path, format!("/ais/category/{}/children?depth=0", root.id().uuid()));
        third.respond(200, successful_body).await;
    });

    let fetched = fetch_ais(&client, &capability, root, 0).await.expect("retry should recover");
    responder.await.expect("peer responder");
    assert_eq!(fetched.len(), 1);
}

#[tokio::test]
async fn missing_folder_keeps_walk_retryable() {
    let mut peer = LoopbackPeer::start().await;
    let client = Client::new(peer.url("/seed")).expect("active Tokio runtime");
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = key(owner, 41);
    let missing = key(owner, 42);
    let valid = key(owner, 43);
    let inventory = Inventory::new(category(root, None), None, InventoryLimits::default()).expect("valid inventory root");
    let (requests, mut events, model, task, _cancel, stop) = scheduler(client, inventory.snapshot());
    peer.grant(&[("InventoryAPIv3", "/ais")]).await;

    let root_request = peer.next().await;
    assert_eq!(root_request.path, format!("/ais/category/{}/children?depth=16", root.id().uuid()));
    root_request.respond(200, root_response(root, &[missing, valid], 1)).await;
    let (fetch, applied) = take_folder_event(next_event(&mut events).await, root);
    let mut inventory = inventory;
    apply_and_acknowledge(&mut inventory, &model, fetch, applied).expect("apply root response");

    let first_request = peer.next().await;
    let second_request = peer.next().await;
    let missing_path = format!("/ais/category/{}/children?depth=0", missing.id().uuid());
    let valid_path = format!("/ais/category/{}/children?depth=0", valid.id().uuid());
    let (missing_request, valid_request) =
        if first_request.path == missing_path { (first_request, second_request) } else { (second_request, first_request) };
    assert_eq!(missing_request.path, missing_path);
    assert_eq!(valid_request.path, valid_path);
    missing_request.respond(404, "").await;
    valid_request.respond(200, empty_folder_response(valid, 2)).await;

    let mut missing_reported = false;
    let mut valid_applied = false;
    loop {
        match next_event(&mut events).await {
            InventoryFetchEvent::Failed { folder, error } => {
                assert_eq!(folder, missing);
                std::assert_matches!(error, InventoryFetchError::RequestFailed);
                missing_reported = true;
            }
            InventoryFetchEvent::Folder { fetch, applied } => {
                assert_eq!(fetch.folder, valid);
                apply_and_acknowledge(&mut inventory, &model, fetch, applied).expect("apply surviving folder response");
                valid_applied = true;
            }
            InventoryFetchEvent::BackgroundDone => break,
        }
    }
    assert!(missing_reported);
    assert!(valid_applied);
    assert!(inventory.snapshot().is_complete(valid));
    assert!(!inventory.snapshot().is_complete(missing));

    let (_status_sender, status) = watch::channel(InventoryLoadingStatus::default());
    let handle = InventoryFetchHandle { requests: requests.clone(), status };
    for _ in 0..2 {
        let completion = handle.request_folder(missing).expect("queue folder retry");
        let retry = peer.next().await;
        assert_eq!(retry.path, missing_path);
        retry.respond(404, "").await;
        match next_event(&mut events).await {
            InventoryFetchEvent::Failed { folder, error } => {
                assert_eq!(folder, missing);
                std::assert_matches!(error, InventoryFetchError::RequestFailed);
            }
            event => panic!("expected failed folder retry, received {event:?}"),
        }
        assert_eq!(completion.await.expect("fetch outcome delivered"), InventoryFetchOutcome::Failed);
    }

    let completion = handle.request_folder(missing).expect("queue successful folder retry");
    let retry = peer.next().await;
    assert_eq!(retry.path, missing_path);
    retry.respond(200, empty_folder_response(missing, 2)).await;
    let (fetch, applied) = take_folder_event(next_event(&mut events).await, missing);
    apply_and_acknowledge(&mut inventory, &model, fetch, applied).expect("apply retried folder response");
    assert_eq!(completion.await.expect("fetch outcome delivered"), InventoryFetchOutcome::Succeeded);
    assert!(inventory.snapshot().is_complete(missing));

    drop(requests);
    stop.send(()).expect("stop scheduler after folder retry");
    task.await.expect("scheduler stopped");
}

#[tokio::test]
async fn rejected_folder_can_be_requested_again() {
    let mut peer = LoopbackPeer::start().await;
    let client = Client::new(peer.url("/seed")).expect("active Tokio runtime");
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = key(owner, 45);
    let child = key(owner, 46);
    let mut inventory = Inventory::new(category(root, None), None, InventoryLimits::default()).expect("valid inventory root");
    let (requests, mut events, model, task, _cancel, stop) = scheduler(client, inventory.snapshot());
    peer.grant(&[("InventoryAPIv3", "/ais")]).await;

    let first = peer.next().await;
    first.respond(200, root_response(root, &[child, child], 0)).await;
    let InventoryFetchEvent::Folder { fetch, applied } = next_event(&mut events).await else {
        panic!("expected first folder result");
    };
    std::assert_matches!(
        inventory.apply_folder_fetch(fetch.folder, fetch.version, fetch.descendent_count, fetch.children),
        Err(::inventory::InventoryError::DuplicateFetchChild)
    );
    applied.send(FolderApplyOutcome { rejected: true }).expect("scheduler waits for rejected apply outcome");
    assert!(!inventory.snapshot().is_complete(root));

    requests.send(ForegroundRequest::background(root)).await.expect("queue retry after rejected model apply");
    let retry = peer.next().await;
    assert_eq!(retry.path, format!("/ais/category/{}/children?depth=0", root.id().uuid()));
    retry.respond(200, empty_folder_response(root, 2)).await;
    let fetch = loop {
        match next_event(&mut events).await {
            InventoryFetchEvent::Folder { fetch, applied } => break (fetch, applied),
            InventoryFetchEvent::BackgroundDone => {}
            InventoryFetchEvent::Failed { folder, error } => panic!("retry failed for {folder:?}: {error}"),
        }
    };
    let (fetch, applied) = fetch;
    assert_eq!(fetch.folder, root);
    apply_and_acknowledge(&mut inventory, &model, fetch, applied).expect("retry applies clean response");
    assert!(inventory.snapshot().is_complete(root));

    drop(requests);
    stop.send(()).expect("stop scheduler after retry");
    task.await.expect("scheduler stopped");
}

#[tokio::test]
async fn refetch_signal_fetches_a_folder_the_scheduler_knows_as_complete() {
    let mut peer = LoopbackPeer::start().await;
    let client = Client::new(peer.url("/seed")).expect("active Tokio runtime");
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = key(owner, 47);
    let mut root_category = category(root, None);
    root_category.server_version = ServerVersion::from_raw(1);
    root_category.server_descendent_count = Some(0);
    let mut inventory = Inventory::new(root_category, None, InventoryLimits::default()).expect("valid complete inventory root");
    assert!(inventory.snapshot().is_complete(root));

    let (requests, mut events, model, task, _cancel, stop) = scheduler(client, inventory.snapshot());
    peer.grant(&[("InventoryAPIv3", "/ais")]).await;
    std::assert_matches!(next_event(&mut events).await, InventoryFetchEvent::BackgroundDone);

    let changes = inventory
        .apply_delta(InventoryDelta { updated_category_versions: vec![(root, ServerVersion::from_raw(0))], ..InventoryDelta::default() })
        .expect("older live version requests a refetch");
    assert_eq!(changes.refetch_categories, [root]);
    model.send_replace(Some(inventory.snapshot()));
    let (_status, status_receiver) = tokio::sync::watch::channel(InventoryLoadingStatus::default());
    let handle = InventoryFetchHandle {
        requests: requests.clone(),
        status: status_receiver,
        edits: crate::inventory::InventoryEditController::new().handle(),
    };
    handle.refetch_folder(root).expect("queue live-update refetch");
    let request = peer.next().await;
    assert_eq!(request.path, format!("/ais/category/{}/children?depth=0", root.id().uuid()));
    request.respond(200, empty_folder_response(root, 2)).await;

    let InventoryFetchEvent::Folder { fetch, applied } = next_event(&mut events).await else {
        panic!("expected folder fetch after refetch signal");
    };
    assert_eq!(fetch.folder, root);
    apply_and_acknowledge(&mut inventory, &model, fetch, applied).expect("apply live-update refetch response");

    drop(requests);
    stop.send(()).expect("stop scheduler");
    task.await.expect("scheduler stopped");
}

#[tokio::test]
async fn parent_response_requeues_a_completed_child_when_its_new_count_is_unknown() {
    let mut peer = LoopbackPeer::start().await;
    let client = Client::new(peer.url("/seed")).expect("active Tokio runtime");
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = key(owner, 48);
    let child = key(owner, 49);
    let mut inventory = Inventory::new(category(root, None), None, InventoryLimits::default()).expect("valid inventory root");
    inventory.upsert_category(category(child, Some(root))).expect("valid child folder");
    let (requests, mut events, model, task, _cancel, stop) = scheduler(client, inventory.snapshot());
    peer.grant(&[("InventoryAPIv3", "/ais")]).await;

    let first = peer.next().await;
    let second = peer.next().await;
    let (root_request, child_request) =
        if first.path.ends_with(&format!("{}/children?depth=16", root.id().uuid())) { (first, second) } else { (second, first) };
    assert_eq!(root_request.path, format!("/ais/category/{}/children?depth=16", root.id().uuid()));
    assert_eq!(child_request.path, format!("/ais/category/{}/children?depth=0", child.id().uuid()));

    child_request.respond(200, empty_folder_response(child, 10)).await;
    let (child_fetch, child_applied) = take_folder_event(next_event(&mut events).await, child);
    apply_and_acknowledge(&mut inventory, &model, child_fetch, child_applied).expect("apply first child response");
    assert!(inventory.snapshot().is_complete(child));

    root_request.respond(200, root_response_with_child_version(root, child, 11, 1)).await;
    let (fetch, applied) = take_folder_event(next_event(&mut events).await, root);
    apply_and_acknowledge(&mut inventory, &model, fetch, applied).expect("apply parent response");
    assert!(!inventory.snapshot().is_complete(child));

    let child_retry = peer.next().await;
    assert_eq!(child_retry.path, format!("/ais/category/{}/children?depth=0", child.id().uuid()));
    child_retry.respond(200, empty_folder_response(child, 12)).await;
    let (fetch, applied) = take_folder_event(next_event(&mut events).await, child);
    apply_and_acknowledge(&mut inventory, &model, fetch, applied).expect("apply child retry");
    assert!(inventory.snapshot().is_complete(child));
    std::assert_matches!(next_event(&mut events).await, InventoryFetchEvent::BackgroundDone);

    drop(requests);
    stop.send(()).expect("stop scheduler after child retry");
    task.await.expect("scheduler stopped");
}

#[tokio::test]
async fn foreground_request_preempts_in_flight_background_folders() {
    let mut peer = LoopbackPeer::start().await;
    let client = Client::new(peer.url("/seed")).expect("active Tokio runtime");
    let (mut inventory, root, first, second) = inventory_with_children();
    let (requests, mut events, model, task, _cancel, stop) = scheduler(client, inventory.snapshot());
    peer.grant(&[("InventoryAPIv3", "/ais")]).await;

    let root_request = peer.next().await;
    assert_eq!(root_request.path, format!("/ais/category/{}/children?depth=16", root.id().uuid()));
    root_request.respond(200, root_response(root, &[first, second], 1)).await;
    let (root_fetch, root_applied) = take_folder_event(next_event(&mut events).await, root);
    assert!(root_fetch.children.len() == 2);
    apply_and_acknowledge(&mut inventory, &model, root_fetch, root_applied).expect("apply root response");

    let background_one = peer.next().await;
    let background_two = peer.next().await;
    let background_paths = HashSet::from([background_one.path.as_str(), background_two.path.as_str()]);
    let first_path = format!("/ais/category/{}/children?depth=0", first.id().uuid());
    let second_path = format!("/ais/category/{}/children?depth=0", second.id().uuid());
    assert_eq!(background_paths, HashSet::from([first_path.as_str(), second_path.as_str()]));

    requests.send(ForegroundRequest::background(first)).await.expect("enqueue foreground folder");
    tokio::join!(background_one.wait_for_close(), background_two.wait_for_close());
    let foreground = peer.next().await;
    assert_eq!(foreground.path, first_path);
    foreground.respond(200, empty_folder_response(first, 2)).await;
    let (fetch, applied) = take_folder_event(next_event(&mut events).await, first);
    assert_eq!(fetch.version.raw(), 2);
    apply_and_acknowledge(&mut inventory, &model, fetch, applied).expect("apply foreground response");

    let resumed_background = peer.next().await;
    assert_eq!(resumed_background.path, second_path);
    resumed_background.respond(200, empty_folder_response(second, 2)).await;
    let (fetch, applied) = take_folder_event(next_event(&mut events).await, second);
    apply_and_acknowledge(&mut inventory, &model, fetch, applied).expect("apply resumed background response");
    std::assert_matches!(next_event(&mut events).await, InventoryFetchEvent::BackgroundDone);

    drop(requests);
    stop.send(()).expect("stop foreground scheduler");
    task.await.expect("scheduler stopped");
}

#[tokio::test]
async fn session_cancellation_stops_an_in_flight_folder_request() {
    let mut peer = LoopbackPeer::start().await;
    let client = Client::new(peer.url("/seed")).expect("active Tokio runtime");
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = key(owner, 50);
    let inventory = Inventory::new(category(root, None), None, InventoryLimits::default()).expect("valid inventory root");
    let (_requests, _events, _model, task, cancel, _stop) = scheduler(client, inventory.snapshot());
    peer.grant(&[("InventoryAPIv3", "/ais")]).await;
    let request = peer.next().await;
    assert_eq!(request.path, format!("/ais/category/{}/children?depth=16", root.id().uuid()));
    cancel.send(()).expect("cancel session");
    request.wait_for_close().await;
    timeout(TEST_TIMEOUT, task).await.expect("cancelled scheduler timeout").expect("scheduler task");
}

#[tokio::test]
async fn one_hundred_thousand_items_load_with_bounded_responses_and_concurrency() {
    const ITEMS_PER_FOLDER: usize = 50_000;
    const _: () = assert!(MAX_INVENTORY_ITEMS >= ITEMS_PER_FOLDER * 2);
    let mut peer = LoopbackPeer::start().await;
    let client = Client::new(peer.url("/seed")).expect("active Tokio runtime");
    let (mut inventory, root, first, second) = inventory_with_children();
    let started = Instant::now();
    let (requests, mut events, model, task, _cancel, stop) = scheduler(client, inventory.snapshot());
    peer.grant(&[("InventoryAPIv3", "/ais")]).await;

    let root_request = peer.next().await;
    root_request.respond(200, root_response(root, &[first, second], ITEMS_PER_FOLDER as u32)).await;
    let (root_fetch, root_applied) = take_folder_event(next_event(&mut events).await, root);
    apply_and_acknowledge(&mut inventory, &model, root_fetch, root_applied).expect("apply root response");

    let first_request = peer.next().await;
    let second_request = peer.next().await;
    let inflight = [first_request.path.as_str(), second_request.path.as_str()].into_iter().collect::<HashSet<_>>();
    let first_path = format!("/ais/category/{}/children?depth=0", first.id().uuid());
    let second_path = format!("/ais/category/{}/children?depth=0", second.id().uuid());
    assert_eq!(inflight, HashSet::from([first_path.as_str(), second_path.as_str()]));
    let peak_in_flight = 2;
    assert_eq!(peak_in_flight, BACKGROUND_CONCURRENCY);

    let first_body = full_folder_response(first, ITEMS_PER_FOLDER, 10_000);
    let second_body = full_folder_response(second, ITEMS_PER_FOLDER, 100_000);
    let largest_response = first_body.len().max(second_body.len());
    assert!(largest_response <= MAX_FETCH_RESPONSE_BYTES);
    first_request.respond(200, first_body).await;
    second_request.respond(200, second_body).await;

    let mut applied_folders = HashSet::new();
    while applied_folders.len() < 2 {
        let (fetch, applied) = match next_event(&mut events).await {
            InventoryFetchEvent::Folder { fetch, applied } => (fetch, applied),
            event => panic!("expected large folder response, received {event:?}"),
        };
        assert!(fetch.folder == first || fetch.folder == second);
        let folder = fetch.folder;
        apply_and_acknowledge(&mut inventory, &model, fetch, applied).expect("apply large folder response");
        applied_folders.insert(folder);
    }
    std::assert_matches!(next_event(&mut events).await, InventoryFetchEvent::BackgroundDone);
    let snapshot = inventory.snapshot();
    assert_eq!(snapshot.items().count(), ITEMS_PER_FOLDER * 2);
    assert_eq!(snapshot.categories().count(), 3);
    assert!(snapshot.is_complete(root));
    assert!(snapshot.is_complete(first));
    assert!(snapshot.is_complete(second));
    drop(requests);
    stop.send(()).expect("stop large inventory scheduler");
    task.await.expect("scheduler stopped");
    eprintln!(
        "loaded {} items in {:?}; peak in-flight requests: {}; largest response: {} bytes",
        ITEMS_PER_FOLDER * 2,
        started.elapsed(),
        peak_in_flight,
        largest_response
    );
}
