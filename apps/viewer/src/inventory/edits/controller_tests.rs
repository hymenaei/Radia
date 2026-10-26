use std::{collections::HashMap, io, net::SocketAddr, sync::Arc, time::Duration};

use ::inventory::{
    AssetType, Category, CategoryKey, FolderType, Inventory, InventoryDelta, InventoryLimits, InventoryNodeId, InventoryOwner,
    InventorySnapshot, InventoryType, Item, ItemKey, PermissionMask, Permissions, SaleInfo, SaleType, ServerVersion,
};
use futures::FutureExt;
use llsd::{Map, Value, xml};
use network::{Error as NetworkError, capabilities::Client};
use protocol::lludp::{Message, Packet};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    runtime::Handle,
    sync::{mpsc, watch},
    task::JoinHandle,
    time::timeout,
};
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

use super::{
    controller::{EditSession, InventoryMove, MoveTransport},
    *,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(20);

struct PeerRequest {
    method: String,
    path: String,
    body: Vec<u8>,
    stream: TcpStream,
}

impl PeerRequest {
    async fn respond(self, status: u16, body: impl AsRef<[u8]>) {
        let body = body.as_ref();
        let reason = match status {
            200 => "OK",
            410 => "Gone",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "Response",
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/llsd+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let mut stream = self.stream;
        stream.write_all(response.as_bytes()).await.expect("write test response headers");
        stream.write_all(body).await.expect("write test response body");
    }

    fn value(&self) -> Value {
        let body = std::str::from_utf8(&self.body).expect("request LLSD is UTF-8");
        xml::from_str(body).expect("request body is LLSD XML")
    }
}

struct LoopbackPeer {
    base: url::Url,
    requests: mpsc::Receiver<PeerRequest>,
    task: JoinHandle<()>,
}

impl LoopbackPeer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind AIS test peer");
        let address = listener.local_addr().expect("read AIS peer address");
        let base = url::Url::parse(&format!("http://{address}")).expect("build AIS peer URL");
        let (sender, requests) = mpsc::channel(16);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let sender = sender.clone();
                tokio::spawn(async move {
                    if let Ok(request) = read_request(stream).await
                        && let Err(send_error) = sender.send(request).await
                    {
                        drop(send_error);
                    }
                });
            }
        });
        Self { base, requests, task }
    }

    fn url(&self, path: &str) -> url::Url {
        self.base.join(path).expect("join AIS peer path")
    }

    async fn next(&mut self) -> PeerRequest {
        timeout(TEST_TIMEOUT, self.requests.recv()).await.expect("AIS peer request timeout").expect("AIS peer stopped")
    }

    async fn grant(&mut self, capabilities: &[(&str, &str)]) {
        let request = self.next().await;
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/seed");
        let body =
            Value::Map(capabilities.iter().map(|(name, path)| ((*name).to_owned(), Value::String(self.url(path).to_string()))).collect());
        request.respond(200, xml::to_vec(&body).expect("encode capability grants")).await;
    }
}

impl Drop for LoopbackPeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct UdpMovePeer {
    sender: UdpSocket,
    destination: SocketAddr,
}

impl MoveTransport for UdpMovePeer {
    fn send(&self, movement: InventoryMove) -> Result<(), NetworkError> {
        let message = match movement {
            InventoryMove::Item(message) => Message::MoveInventoryItem(message),
            InventoryMove::Folder(message) => Message::MoveInventoryFolder(message),
        };
        let bytes = Packet::new(1, message).with_reliable().encode().map_err(|error| NetworkError::SimulatorSend(error.to_string()))?;
        self.sender.try_send_to(&bytes, self.destination).map(|_| ()).map_err(|error| NetworkError::SimulatorSend(error.to_string()))
    }
}

fn key(owner: InventoryOwner, id: u128) -> CategoryKey {
    CategoryKey::new(owner, FolderId::new(Uuid::from_u128(id)))
}

fn category(key: CategoryKey, parent: Option<CategoryKey>, folder_type: FolderType, name: &str) -> Category {
    Category {
        key,
        parent,
        preferred_folder_type: folder_type,
        name: Arc::from(name),
        server_version: ServerVersion::UNKNOWN,
        server_descendent_count: None,
    }
}

fn item(owner: InventoryOwner, parent: CategoryKey, id: u128, name: &str) -> Item {
    Item {
        key: ItemKey::new(owner, ItemId::new(Uuid::from_u128(id))),
        parent,
        asset_id: AssetId::new(Uuid::nil()),
        asset_type: AssetType::Notecard,
        inventory_type: InventoryType::Notecard,
        flags: 0,
        name: Arc::from(name),
        description: Arc::from(""),
        created_at: 0,
        permissions: Permissions {
            creator: AvatarId::new(Uuid::nil()),
            owner: AvatarId::new(owner.id()),
            group: GroupId::new(Uuid::nil()),
            last_owner: AvatarId::new(Uuid::nil()),
            base: PermissionMask::new(0),
            owner_mask: PermissionMask::new(0),
            group_mask: PermissionMask::new(0),
            everyone: PermissionMask::new(0),
            next_owner: PermissionMask::new(0),
        },
        sale: SaleInfo { sale_type: SaleType::NotForSale, price: 0 },
    }
}

fn inventory_fixture()
-> (Inventory, InventoryOwner, CategoryKey, CategoryKey, CategoryKey, CategoryKey, CategoryKey, ItemKey, ItemKey, CategoryKey) {
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = key(owner, 2);
    let first = key(owner, 3);
    let second = key(owner, 4);
    let trash = key(owner, 5);
    let protected = key(owner, 6);
    let trash_child = key(owner, 7);
    let item_key = ItemKey::new(owner, ItemId::new(Uuid::from_u128(8)));
    let trash_item_key = ItemKey::new(owner, ItemId::new(Uuid::from_u128(9)));
    let mut inventory = Inventory::new(category(root, None, FolderType::RootInventory, "root"), None, InventoryLimits::default())
        .expect("valid personal root");
    for category in [
        category(first, Some(root), FolderType::None, "first"),
        category(second, Some(root), FolderType::None, "second"),
        category(trash, Some(root), FolderType::Trash, "trash"),
        category(protected, Some(root), FolderType::Unknown(900), "protected"),
        category(trash_child, Some(trash), FolderType::None, "trash child"),
    ] {
        inventory.upsert_category(category).expect("add fixture category");
    }
    inventory.upsert_item(item(owner, first, 8, "item")).expect("add fixture item");
    inventory.upsert_item(item(owner, trash, 9, "trash item")).expect("add Trash item");
    (inventory, owner, root, first, second, trash, protected, item_key, trash_item_key, trash_child)
}

fn start_controller(
    peer: &LoopbackPeer,
    inventory: &Inventory,
    session: EditSession,
    transport: Arc<dyn MoveTransport>,
) -> (InventoryEditController, watch::Sender<Option<InventorySnapshot>>) {
    let client = Client::new(peer.url("/seed")).expect("active Tokio runtime");
    let (model_sender, model) = watch::channel(Some(inventory.snapshot()));
    let mut controller = InventoryEditController::new();
    controller.start_with_transport(&Handle::current(), client, session, transport, model, futures::future::pending().boxed().shared());
    (controller, model_sender)
}

async fn apply_confirmation(
    controller: &mut InventoryEditController,
    inventory: &mut Inventory,
    model: &watch::Sender<Option<InventorySnapshot>>,
) {
    let Some(InventoryEditEvent::Apply { confirmation, applied }) = controller.next_event().await else {
        panic!("expected AIS confirmation event");
    };
    let current = inventory.snapshot();
    let result = inventory.apply_delta(confirmation.into_delta(&current)).is_ok();
    model.send_replace(Some(inventory.snapshot()));
    applied.send(result).expect("worker waits for confirmation result");
}

async fn confirm_udp_move(
    controller: &mut InventoryEditController,
    socket: &UdpSocket,
    inventory: &mut Inventory,
    model: &watch::Sender<Option<InventorySnapshot>>,
    owner: InventoryOwner,
    session_id: Uuid,
) {
    let Some(InventoryEditEvent::MoveStarted { expectation, confirmed, registered }) = controller.next_event().await else {
        panic!("expected UDP move registration event");
    };
    let mut pending = HashMap::from([(expectation.node, (expectation, confirmed))]);
    registered.send(()).expect("move worker waits for registration");
    let mut bytes = [0; 4096];
    let received = timeout(TEST_TIMEOUT, socket.recv_from(&mut bytes)).await.expect("outbound move packet timeout");
    let (size, _) = received.expect("read outbound move packet");
    let packet = Packet::decode(&bytes[..size]).expect("decode outbound move packet");
    assert!(packet.is_reliable());
    let message = packet.into_message();
    match (expectation.node.id(), &message) {
        (InventoryNodeId::Item(id), Message::MoveInventoryItem(message)) => {
            assert_eq!(message.agent_id, owner.id());
            assert_eq!(message.session_id, session_id);
            assert_eq!(message.inventory_data[0].item_id, id.uuid());
            assert_eq!(message.inventory_data[0].folder_id, expectation.to.id().uuid());
        }
        (InventoryNodeId::Category(id), Message::MoveInventoryFolder(message)) => {
            assert_eq!(message.agent_id, owner.id());
            assert_eq!(message.session_id, session_id);
            assert_eq!(message.inventory_data[0].folder_id, id.uuid());
            assert_eq!(message.inventory_data[0].parent_id, expectation.to.id().uuid());
        }
        (_, message) => panic!("unexpected outbound move: {message:?}"),
    }
    let update = crate::session::apply_inventory_udp(inventory, owner.id(), &message)
        .expect("intake inbound UDP move update")
        .expect("move update belongs to the active owner");
    assert_eq!(update.rejected_blocks, 0);
    let selected = update
        .changes
        .changes
        .iter()
        .find(|change| matches!(change, ::inventory::InventoryChange::NodeMoved { node, .. } if *node == expectation.node));
    std::assert_matches!(
        selected,
        Some(::inventory::InventoryChange::NodeMoved { node, from, to })
            if *node == expectation.node && *from == expectation.from && *to == expectation.to
    );
    crate::session::resolve_pending_inventory_moves(&mut pending, &update.changes);
    assert!(pending.is_empty());
    model.send_replace(Some(inventory.snapshot()));
}

fn metadata(name: &str, ids: &[Uuid]) -> Vec<u8> {
    let value = Value::Map(Map::from([(name.to_owned(), Value::Array(ids.iter().copied().map(Value::Uuid).collect()))]));
    xml::to_vec(&value).expect("encode AIS metadata response")
}

async fn read_request(mut stream: TcpStream) -> io::Result<PeerRequest> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    let (header_end, content_length) = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..end]).map_err(io::Error::other)?;
            let content_length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>())
                .transpose()
                .map_err(io::Error::other)?
                .unwrap_or_default();
            if content_length > 1024 * 1024 {
                return Err(io::Error::other("test request exceeded its body bound"));
            }
            break (end + 4, content_length);
        }
    };
    while bytes.len() < header_end.saturating_add(content_length) {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    let headers = std::str::from_utf8(&bytes[..header_end - 4]).map_err(io::Error::other)?;
    let mut line = headers.lines().next().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?.split_whitespace();
    let method = line.next().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?.to_owned();
    let path = line.next().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?.to_owned();
    Ok(PeerRequest { method, path, body: bytes[header_end..header_end + content_length].to_vec(), stream })
}

#[tokio::test]
async fn every_edit_path_uses_confirmation_from_its_loopback_peer() {
    let mut http = LoopbackPeer::start().await;
    let udp = UdpSocket::bind("127.0.0.1:0").await.expect("bind UDP move peer");
    let udp_address = udp.local_addr().expect("read UDP peer address");
    let sender = UdpSocket::bind("127.0.0.1:0").await.expect("bind UDP sender");
    let transport: Arc<dyn MoveTransport> = Arc::new(UdpMovePeer { sender, destination: udp_address });
    let (mut inventory, owner, root, first, second, trash, _, item, trash_item, trash_child) = inventory_fixture();
    let session_id = Uuid::from_u128(20);
    let (mut controller, model) = start_controller(&http, &inventory, EditSession { agent_id: owner.id(), session_id }, transport);
    let handle = controller.handle();

    let create_id = Uuid::from_u128(21);
    let create = tokio::spawn({
        let handle = handle.clone();
        async move { handle.create_folder(root, "created", FolderType::None).await }
    });
    http.grant(&[("InventoryAPIv3", "/ais")]).await;
    let request = http.next().await;
    assert_eq!(request.method, "POST");
    assert!(request.path.starts_with(&format!("/ais/category/{}?tid=", root.id().uuid())));
    let body = request.value();
    let category = body.as_map().and_then(|fields| fields.get("categories")).and_then(Value::as_array).and_then(|values| values.first());
    assert_eq!(category.and_then(Value::as_map).and_then(|fields| fields.get("name")), Some(&Value::String("created".into())));
    request.respond(200, metadata("_created_categories", &[create_id])).await;
    apply_confirmation(&mut controller, &mut inventory, &model).await;
    let created = CategoryKey::new(owner, FolderId::new(create_id));
    assert_eq!(create.await.expect("create result"), InventoryEditOutcome::Created(created));

    let rename_category = tokio::spawn({
        let handle = handle.clone();
        async move { handle.rename_category(created, "renamed folder").await }
    });
    let request = http.next().await;
    assert_eq!(request.method, "PATCH");
    assert_eq!(request.path, format!("/ais/category/{}", create_id));
    let later_rename = tokio::spawn({
        let handle = handle.clone();
        async move { handle.rename_category(created, "second rename").await }
    });
    let fields = request.value();
    assert_eq!(fields.as_map().and_then(|fields| fields.get("name")), Some(&Value::String("renamed folder".into())));
    request.respond(200, metadata("_updated_categories", &[create_id])).await;
    apply_confirmation(&mut controller, &mut inventory, &model).await;
    assert_eq!(rename_category.await.expect("category rename result"), InventoryEditOutcome::Applied);
    let request = http.next().await;
    assert_eq!(request.method, "PATCH");
    assert_eq!(request.path, format!("/ais/category/{}", create_id));
    let fields = request.value();
    assert_eq!(fields.as_map().and_then(|fields| fields.get("name")), Some(&Value::String("second rename".into())));
    request.respond(200, metadata("_updated_categories", &[create_id])).await;
    apply_confirmation(&mut controller, &mut inventory, &model).await;
    assert_eq!(later_rename.await.expect("queued category rename result"), InventoryEditOutcome::Applied);
    assert_eq!(inventory.snapshot().category(created).map(|category| category.name.as_ref()), Some("second rename"));

    let rename_item = tokio::spawn({
        let handle = handle.clone();
        async move { handle.rename_item(item, "renamed item").await }
    });
    let request = http.next().await;
    assert_eq!(request.method, "PATCH");
    assert_eq!(request.path, format!("/ais/item/{}", item.id().uuid()));
    request.respond(200, metadata("_updated_items", &[item.id().uuid()])).await;
    apply_confirmation(&mut controller, &mut inventory, &model).await;
    assert_eq!(rename_item.await.expect("item rename result"), InventoryEditOutcome::Applied);
    assert_eq!(inventory.snapshot().item(item).map(|item| item.name.as_ref()), Some("renamed item"));

    let empty_trash = tokio::spawn({
        let handle = handle.clone();
        async move { handle.empty_trash().await }
    });
    let request = http.next().await;
    assert_eq!(request.method, "DELETE");
    assert_eq!(request.path, format!("/ais/category/{}/children", trash.id().uuid()));
    let response = Value::Map(Map::from([
        ("_removed_items".into(), Value::Array(vec![Value::Uuid(trash_item.id().uuid())])),
        ("_categories_removed".into(), Value::Array(vec![Value::Uuid(trash_child.id().uuid())])),
    ]));
    request.respond(200, xml::to_vec(&response).expect("encode empty Trash response")).await;
    apply_confirmation(&mut controller, &mut inventory, &model).await;
    assert_eq!(empty_trash.await.expect("empty Trash result"), InventoryEditOutcome::Applied);
    assert!(inventory.snapshot().item(trash_item).is_none());
    assert!(inventory.snapshot().category(trash_child).is_none());

    let move_item = tokio::spawn({
        let handle = handle.clone();
        async move { handle.move_item(item, second).await }
    });
    confirm_udp_move(&mut controller, &udp, &mut inventory, &model, owner, session_id).await;
    assert_eq!(move_item.await.expect("move item result"), InventoryEditOutcome::Applied);

    let move_folder = tokio::spawn({
        let handle = handle.clone();
        async move { handle.move_folder(second, first).await }
    });
    confirm_udp_move(&mut controller, &udp, &mut inventory, &model, owner, session_id).await;
    assert_eq!(move_folder.await.expect("move folder result"), InventoryEditOutcome::Applied);

    let delete_item = tokio::spawn({
        let handle = handle.clone();
        async move { handle.delete_item(item).await }
    });
    confirm_udp_move(&mut controller, &udp, &mut inventory, &model, owner, session_id).await;
    assert_eq!(delete_item.await.expect("delete item result"), InventoryEditOutcome::Applied);

    let delete_folder = tokio::spawn(async move { handle.delete_folder(second).await });
    confirm_udp_move(&mut controller, &udp, &mut inventory, &model, owner, session_id).await;
    assert_eq!(delete_folder.await.expect("delete folder result"), InventoryEditOutcome::Applied);

    controller.stop().await;
}

#[tokio::test]
async fn ais_failures_and_gone_refresh_do_not_apply_unconfirmed_changes() {
    let mut http = LoopbackPeer::start().await;
    let (inventory, owner, _root, _folder, _second, trash, _, item, _, _) = inventory_fixture();
    let (mut controller, _model) = start_controller(
        &http,
        &inventory,
        EditSession { agent_id: owner.id(), session_id: Uuid::from_u128(22) },
        Arc::new(NoopMoveTransport),
    );
    let handle = controller.handle();
    let rename = tokio::spawn({
        let handle = handle.clone();
        async move { handle.rename_item(item, "not confirmed").await }
    });
    http.grant(&[("InventoryAPIv3", "/ais")]).await;
    let request = http.next().await;
    assert_eq!(request.method, "PATCH");
    request.respond(503, b"{}").await;
    assert_eq!(rename.await.expect("failed rename result"), InventoryEditOutcome::ServerError);
    assert_eq!(inventory.snapshot().item(item).map(|item| item.name.as_ref()), Some("item"));

    let empty = tokio::spawn({
        let handle = handle.clone();
        async move { handle.empty_trash().await }
    });
    let request = http.next().await;
    assert_eq!(request.method, "DELETE");
    request.respond(410, b"{}").await;
    let Some(InventoryEditEvent::RefreshGone { folder, refreshed }) = controller.next_event().await else {
        panic!("expected refresh request after 410");
    };
    assert_eq!(folder, trash);
    refreshed.send(true).expect("edit worker waits for refresh scheduling");
    assert_eq!(empty.await.expect("gone result"), InventoryEditOutcome::Gone);
    assert!(inventory.snapshot().item(item).is_some());

    assert_eq!(handle.rename_category(folder, "rejected").await, InventoryEditOutcome::Rejected(InventoryEditRejection::ProtectedFolder));
    let roots = inventory.snapshot().roots();
    assert_eq!(
        handle.rename_category(roots.personal(), "rejected").await,
        InventoryEditOutcome::Rejected(InventoryEditRejection::RootFolder)
    );
    assert!(http.requests.try_recv().is_err());

    controller.stop().await;
}

struct NoopMoveTransport;

impl MoveTransport for NoopMoveTransport {
    fn send(&self, _movement: InventoryMove) -> Result<(), NetworkError> {
        Err(NetworkError::SessionClosed)
    }
}

#[tokio::test]
async fn missing_ais_capability_uses_legacy_category_creation() {
    let mut http = LoopbackPeer::start().await;
    let (mut inventory, owner, root, ..) = inventory_fixture();
    let (mut controller, model) = start_controller(
        &http,
        &inventory,
        EditSession { agent_id: owner.id(), session_id: Uuid::from_u128(23) },
        Arc::new(NoopMoveTransport),
    );
    let handle = controller.handle();
    let create = tokio::spawn(async move { handle.create_folder(root, "legacy", FolderType::None).await });
    http.grant(&[("CreateInventoryCategory", "/legacy")]).await;
    let request = http.next().await;
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/legacy");
    let body = request.value();
    let fields = body.as_map().expect("legacy request map");
    let id = fields.get("folder_id").and_then(Value::as_uuid).expect("requested folder ID");
    assert_eq!(fields.get("parent_id"), Some(&Value::Uuid(root.id().uuid())));
    assert_eq!(fields.get("type"), Some(&Value::Integer(FolderType::None.raw())));
    assert_eq!(fields.get("name"), Some(&Value::String("legacy".into())));
    let response = Value::Map(Map::from([
        ("folder_id".into(), Value::Uuid(id)),
        ("parent_id".into(), Value::Uuid(root.id().uuid())),
        ("type".into(), Value::Integer(FolderType::None.raw())),
        ("name".into(), Value::String("legacy".into())),
    ]));
    request.respond(200, xml::to_vec(&response).expect("encode legacy confirmation")).await;
    apply_confirmation(&mut controller, &mut inventory, &model).await;
    let key = CategoryKey::new(owner, FolderId::new(id));
    assert_eq!(create.await.expect("legacy create result"), InventoryEditOutcome::Created(key));
    assert!(inventory.snapshot().category(key).is_some());
    controller.stop().await;
}

#[tokio::test]
async fn move_confirmation_timeout_invalidates_both_folders_for_refetch() {
    let http = LoopbackPeer::start().await;
    let udp = UdpSocket::bind("127.0.0.1:0").await.expect("bind UDP move peer");
    let address = udp.local_addr().expect("read UDP peer address");
    let sender = UdpSocket::bind("127.0.0.1:0").await.expect("bind UDP sender");
    let transport: Arc<dyn MoveTransport> = Arc::new(UdpMovePeer { sender, destination: address });
    let (mut inventory, owner, _root, first, second, _, _, item, _, _) = inventory_fixture();
    let (mut controller, model) =
        start_controller(&http, &inventory, EditSession { agent_id: owner.id(), session_id: Uuid::from_u128(24) }, transport);
    let handle = controller.handle();
    let move_item = tokio::spawn(async move { handle.move_item(item, second).await });

    let Some(InventoryEditEvent::MoveStarted { expectation, confirmed: _confirmed, registered }) =
        timeout(TEST_TIMEOUT, controller.next_event()).await.expect("move registration timeout")
    else {
        panic!("expected UDP move registration event");
    };
    registered.send(()).expect("move worker waits for registration");
    let mut bytes = [0; 4096];
    let (size, _) = timeout(TEST_TIMEOUT, udp.recv_from(&mut bytes)).await.expect("outbound move packet timeout").expect("read packet");
    assert!(Packet::decode(&bytes[..size]).expect("decode outbound packet").is_reliable());
    let Some(InventoryEditEvent::MoveTimedOut { expectation: timed_out, refreshed }) =
        timeout(TEST_TIMEOUT, controller.next_event()).await.expect("move confirmation timeout event")
    else {
        panic!("expected timeout invalidation event");
    };
    assert_eq!(timed_out, expectation);
    let mut delta = InventoryDelta::default();
    delta.updated_category_versions.extend([(first, ServerVersion::UNKNOWN), (second, ServerVersion::UNKNOWN)]);
    let changes = inventory.apply_delta(delta).expect("mark move folders incomplete");
    assert!(changes.refetch_categories.contains(&first));
    assert!(changes.refetch_categories.contains(&second));
    model.send_replace(Some(inventory.snapshot()));
    refreshed.send(true).expect("move worker waits for refetch scheduling");
    std::assert_matches!(move_item.await.expect("timed out move result"), InventoryEditOutcome::TimedOut);
    controller.stop().await;
}
