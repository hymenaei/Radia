use std::{collections::HashMap, sync::Arc};

use ::inventory::{
    AssetType, Category, CategoryKey, FolderType, Inventory, InventoryChild, InventoryLimits, InventoryNodeId, InventoryNodeKey,
    InventoryOwner, InventoryType, Item, ItemKey, PermissionMask, Permissions, SaleInfo, SaleType, ServerVersion,
};
use math::{Quat, Vec3};
use network::Delivery;
use protocol::{
    lludp::{BlockValues, FieldValue, Message, Packet, SchemaMessage},
    messages::{
        AgentRequestSit, AgentSit, AvatarSitResponse,
        agent::agent_request_sit::{AgentData as SitAgentData, TargetObject},
        agent::avatar_sit_response::{SitObject, SitTransform},
    },
};
use tokio::sync::oneshot;
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

use crate::inventory::FolderFetch;

use super::inventory::{apply_inventory_fetch, cache_restore_move_changes, resolve_pending_inventory_moves};
use super::{MoveExpectation, PendingHandshakeReplies, network_end_after_stream_close};
use crate::session::SessionEnd;

#[test]
fn pending_handshake_replies_share_one_drain_and_keep_each_reply() {
    let simulator = network::SimulatorId::new(7);
    let mut pending = PendingHandshakeReplies::default();

    assert!(pending.queue(simulator, 3));
    assert!(!pending.queue(simulator, 3));
    assert!(!pending.queue(simulator, 3));
    assert_eq!(pending.by_simulator.len(), 1);
    assert_eq!(pending.by_simulator.get(&simulator).map(|reply| reply.remaining), Some(3));

    assert_eq!(pending.take_next(simulator), Some(3));
    assert_eq!(pending.take_next(simulator), Some(3));
    assert_eq!(pending.take_next(simulator), Some(3));
    assert_eq!(pending.take_next(simulator), None);
    assert!(pending.by_simulator.is_empty());
}

#[test]
fn requested_disconnect_ends_normally() {
    assert_eq!(network_end_after_stream_close(true, None, true), SessionEnd::Requested);
}

#[test]
fn stream_close_keeps_simulator_failure() {
    assert_eq!(
        network_end_after_stream_close(false, Some("heartbeat timeout".into()), false),
        SessionEnd::Failed("heartbeat timeout".into())
    );
}

#[test]
fn disconnect_request_keeps_failure() {
    assert_eq!(
        network_end_after_stream_close(true, Some("heartbeat timeout".into()), false),
        SessionEnd::Failed("heartbeat timeout".into())
    );
}

#[test]
fn unexpected_primary_disconnect_fails() {
    assert_eq!(network_end_after_stream_close(false, None, true), SessionEnd::Failed("The primary simulator disconnected.".into()));
}

use crate::session::agent_control::{SitTransport, handle_sit_alert, process_agent_control_command, process_sit_response};

struct LoopbackSitTransport<'a> {
    socket: &'a tokio::net::UdpSocket,
    destination: std::net::SocketAddr,
    sequence: std::sync::atomic::AtomicU32,
}

impl LoopbackSitTransport<'_> {
    fn send(&self, delivery: Delivery, message: Message) -> Result<(), network::Error> {
        assert_eq!(delivery, Delivery::Reliable);
        let sequence = self.sequence.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let bytes = Packet::new(sequence, message).encode().map_err(|_| network::Error::SessionClosed)?;
        self.socket.try_send_to(&bytes, self.destination).map(|_| ()).map_err(|_| network::Error::SessionClosed)
    }
}

impl SitTransport for LoopbackSitTransport<'_> {
    fn send_request(&self, request: AgentRequestSit, delivery: Delivery) -> Result<(), network::Error> {
        self.send(delivery, Message::AgentRequestSit(request))
    }

    fn send_sit(&self, sit: AgentSit, delivery: Delivery) -> Result<(), network::Error> {
        self.send(delivery, Message::AgentSit(sit))
    }
}

async fn receive_loopback_message(socket: &tokio::net::UdpSocket) -> Message {
    let mut bytes = [0; 2048];
    let (length, _) = socket.recv_from(&mut bytes).await.expect("loopback packet");
    Packet::decode(&bytes[..length]).expect("valid loopback packet").message().clone()
}

async fn send_loopback_message(socket: &tokio::net::UdpSocket, destination: std::net::SocketAddr, message: Message) {
    let bytes = Packet::new(1, message).encode().expect("encode loopback packet");
    socket.send_to(&bytes, destination).await.expect("send loopback packet");
}

fn alert_message(message: &[u8]) -> Message {
    let schema = SchemaMessage::schema_by_name("AlertMessage").expect("AlertMessage schema");
    Message::Schema(
        SchemaMessage::new(
            schema,
            vec![
                BlockValues::single(vec![FieldValue::Bytes(message.to_vec())]),
                BlockValues::new(Vec::new()),
                BlockValues::new(Vec::new()),
            ],
        )
        .expect("valid AlertMessage"),
    )
}

fn sit_response(object_id: Uuid) -> Message {
    Message::AvatarSitResponse(AvatarSitResponse {
        sit_object: SitObject { id: Some(object_id) },
        sit_transform: SitTransform {
            auto_pilot: Some(true),
            sit_position: Some(Vec3::new(4.0, 5.0, 6.0)),
            sit_rotation: Some(Quat::IDENTITY),
            camera_eye_offset: Some(Vec3::new(2.0, 0.0, 1.0)),
            camera_at_offset: Some(Vec3::new(0.0, 0.0, 1.0)),
            force_mouselook: Some(false),
        },
    })
}

fn category(key: CategoryKey, parent: Option<CategoryKey>, version: ServerVersion, descendent_count: Option<u32>) -> Category {
    Category {
        key,
        parent,
        preferred_folder_type: FolderType::None,
        name: Arc::from("folder"),
        server_version: version,
        server_descendent_count: descendent_count,
    }
}

#[test]
fn stale_fetch_does_not_replace_a_complete_newer_folder() {
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let folder = CategoryKey::new(owner, FolderId::new(Uuid::from_u128(2)));
    let root = Category {
        key: folder,
        parent: None,
        preferred_folder_type: FolderType::None,
        name: Arc::from("root"),
        server_version: ServerVersion::from_raw(10),
        server_descendent_count: Some(0),
    };
    let mut inventory = Inventory::new(root, None, InventoryLimits::default()).expect("valid inventory root");
    let revision = inventory.snapshot().revision();
    let stale_item = Item {
        key: ItemKey::new(owner, ItemId::new(Uuid::from_u128(3))),
        parent: folder,
        asset_id: AssetId::new(Uuid::nil()),
        asset_type: AssetType::Notecard,
        inventory_type: InventoryType::Notecard,
        flags: 0,
        name: Arc::from("stale"),
        description: Arc::from("stale response"),
        created_at: 0,
        permissions: Permissions {
            creator: AvatarId::new(Uuid::nil()),
            owner: AvatarId::new(Uuid::nil()),
            group: GroupId::new(Uuid::nil()),
            last_owner: AvatarId::new(Uuid::nil()),
            base: PermissionMask::new(0),
            owner_mask: PermissionMask::new(0),
            group_mask: PermissionMask::new(0),
            everyone: PermissionMask::new(0),
            next_owner: PermissionMask::new(0),
        },
        sale: SaleInfo { sale_type: SaleType::NotForSale, price: 0 },
    };
    let stale_fetch = FolderFetch {
        folder,
        version: ServerVersion::from_raw(9),
        descendent_count: Some(1),
        children: vec![InventoryChild::Item(stale_item)],
    };

    std::assert_matches!(apply_inventory_fetch(&mut inventory, stale_fetch), Ok(None));
    let snapshot = inventory.snapshot();
    assert_eq!(snapshot.revision(), revision);
    assert_eq!(snapshot.items().count(), 0);
    assert_eq!(snapshot.category(folder).map(|category| category.server_version), Some(ServerVersion::from_raw(10)));
}

#[test]
fn older_parent_result_preserves_newer_child_version() {
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = CategoryKey::new(owner, FolderId::new(Uuid::from_u128(2)));
    let child = CategoryKey::new(owner, FolderId::new(Uuid::from_u128(3)));
    let item_key = ItemKey::new(owner, ItemId::new(Uuid::from_u128(4)));
    let mut inventory =
        Inventory::new(category(root, None, ServerVersion::UNKNOWN, None), None, InventoryLimits::default()).expect("valid inventory root");
    inventory.upsert_category(category(child, Some(root), ServerVersion::UNKNOWN, None)).expect("add child folder");
    let item = Item {
        key: item_key,
        parent: child,
        asset_id: AssetId::new(Uuid::nil()),
        asset_type: AssetType::Notecard,
        inventory_type: InventoryType::Notecard,
        flags: 0,
        name: Arc::from("new item"),
        description: Arc::from("new child response"),
        created_at: 0,
        permissions: Permissions {
            creator: AvatarId::new(Uuid::nil()),
            owner: AvatarId::new(Uuid::nil()),
            group: GroupId::new(Uuid::nil()),
            last_owner: AvatarId::new(Uuid::nil()),
            base: PermissionMask::new(0),
            owner_mask: PermissionMask::new(0),
            group_mask: PermissionMask::new(0),
            everyone: PermissionMask::new(0),
            next_owner: PermissionMask::new(0),
        },
        sale: SaleInfo { sale_type: SaleType::NotForSale, price: 0 },
    };

    apply_inventory_fetch(
        &mut inventory,
        FolderFetch {
            folder: child,
            version: ServerVersion::from_raw(10),
            descendent_count: Some(1),
            children: vec![InventoryChild::Item(item)],
        },
    )
    .expect("child response applies")
    .expect("child response is not stale");
    let stale_child = category(child, Some(root), ServerVersion::from_raw(9), Some(0));
    apply_inventory_fetch(
        &mut inventory,
        FolderFetch {
            folder: root,
            version: ServerVersion::from_raw(3),
            descendent_count: Some(1),
            children: vec![InventoryChild::Category(stale_child)],
        },
    )
    .expect("parent response applies")
    .expect("parent response is not stale");

    let snapshot = inventory.snapshot();
    let child_category = snapshot.category(child).expect("child remains present");
    assert_eq!(child_category.server_version, ServerVersion::from_raw(10));
    assert_eq!(child_category.server_descendent_count, Some(1));
    assert!(snapshot.is_complete(child));
    assert!(snapshot.item(item_key).is_some());
}

#[tokio::test]
async fn folder_fetch_move_resolves_pending_inventory_edit() {
    let owner = InventoryOwner::new(Uuid::from_u128(21));
    let root = CategoryKey::new(owner, FolderId::new(Uuid::from_u128(22)));
    let first = CategoryKey::new(owner, FolderId::new(Uuid::from_u128(23)));
    let second = CategoryKey::new(owner, FolderId::new(Uuid::from_u128(24)));
    let folder = CategoryKey::new(owner, FolderId::new(Uuid::from_u128(25)));
    let mut inventory =
        Inventory::new(category(root, None, ServerVersion::UNKNOWN, None), None, InventoryLimits::default()).expect("valid inventory root");
    inventory.upsert_category(category(first, Some(root), ServerVersion::UNKNOWN, None)).expect("add first folder");
    inventory.upsert_category(category(second, Some(root), ServerVersion::UNKNOWN, None)).expect("add second folder");
    inventory.upsert_category(category(folder, Some(first), ServerVersion::UNKNOWN, None)).expect("add moved folder");

    let node = InventoryNodeKey::new(owner, InventoryNodeId::Category(folder.id()));
    let expectation = MoveExpectation { node, from: first, to: second };
    let (confirmed, confirmation) = oneshot::channel();
    let mut pending = HashMap::from([(node, (expectation, confirmed))]);
    let changes = apply_inventory_fetch(
        &mut inventory,
        FolderFetch {
            folder: second,
            version: ServerVersion::UNKNOWN,
            descendent_count: Some(1),
            children: vec![InventoryChild::Category(category(folder, Some(second), ServerVersion::UNKNOWN, Some(0)))],
        },
    )
    .expect("apply destination fetch")
    .expect("destination fetch changes inventory");
    let selected = changes
        .changes
        .iter()
        .find(|change| matches!(change, ::inventory::InventoryChange::NodeMoved { node: changed, .. } if *changed == node));
    std::assert_matches!(
        selected,
        Some(::inventory::InventoryChange::NodeMoved { node: changed, from, to })
            if *changed == node && *from == first && *to == second
    );
    resolve_pending_inventory_moves(&mut pending, &changes);
    confirmation.await.expect("fetch change set confirms pending edit");
    assert!(pending.is_empty());
}

#[tokio::test]
async fn cache_restore_move_resolves_pending_inventory_edit() {
    let owner = InventoryOwner::new(Uuid::from_u128(31));
    let root = CategoryKey::new(owner, FolderId::new(Uuid::from_u128(32)));
    let first = CategoryKey::new(owner, FolderId::new(Uuid::from_u128(33)));
    let second = CategoryKey::new(owner, FolderId::new(Uuid::from_u128(34)));
    let folder = CategoryKey::new(owner, FolderId::new(Uuid::from_u128(35)));
    let mut inventory =
        Inventory::new(category(root, None, ServerVersion::UNKNOWN, None), None, InventoryLimits::default()).expect("valid inventory root");
    inventory.upsert_category(category(first, Some(root), ServerVersion::UNKNOWN, None)).expect("add first folder");
    inventory.upsert_category(category(second, Some(root), ServerVersion::UNKNOWN, None)).expect("add second folder");
    inventory.upsert_category(category(folder, Some(first), ServerVersion::UNKNOWN, None)).expect("add cached moved folder");

    let before = inventory.snapshot();
    inventory.move_node(InventoryNodeKey::new(owner, InventoryNodeId::Category(folder.id())), second).expect("restore cached folder move");
    let after = inventory.snapshot();
    let node = InventoryNodeKey::new(owner, InventoryNodeId::Category(folder.id()));
    let expectation = MoveExpectation { node, from: first, to: second };
    let (confirmed, confirmation) = oneshot::channel();
    let mut pending = HashMap::from([(node, (expectation, confirmed))]);
    let changes = cache_restore_move_changes(&pending, &before, &after);
    resolve_pending_inventory_moves(&mut pending, &changes);
    confirmation.await.expect("cache restore change set confirms pending edit");
    assert!(pending.is_empty());
}

#[tokio::test]
async fn unrelated_alert_preserves_pending_sit() {
    use crate::{
        session::{AgentControlEvent, AgentControlState, CameraUplink, SitCameraOverride, SitOutcome, SitResponseState},
        test_helpers::simulator_id,
    };

    let simulator_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback simulator");
    let viewer_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback viewer");
    let simulator = simulator_id();
    let agent_id = Uuid::from_u128(0x61);
    let session_id = Uuid::from_u128(0x62);
    let object_id = Uuid::from_u128(0x63);
    let offset = Vec3::new(1.0, 2.0, 3.0);
    let (handle, mut agent_control, camera_updates) = AgentControlState::new();
    let call = tokio::spawn({
        let handle = handle.clone();
        async move { handle.sit_on_object(object_id, offset).await }
    });
    let AgentControlEvent::Command(Some(command)) = agent_control.next_event().await else { panic!("sit command") };
    let transport = LoopbackSitTransport {
        socket: &viewer_socket,
        destination: simulator_socket.local_addr().expect("simulator address"),
        sequence: std::sync::atomic::AtomicU32::new(1),
    };
    let mut camera = CameraUplink::new();
    process_agent_control_command(&transport, &mut agent_control, &mut camera, Some(simulator), Some((agent_id, session_id)), command);

    let expected_request =
        AgentRequestSit { agent_data: SitAgentData { agent_id, session_id }, target_object: TargetObject { target_id: object_id, offset } };
    assert_eq!(receive_loopback_message(&simulator_socket).await, Message::AgentRequestSit(expected_request));

    let viewer_address = viewer_socket.local_addr().expect("viewer address");
    send_loopback_message(&simulator_socket, viewer_address, alert_message(b"Unrelated notice")).await;
    let alert = receive_loopback_message(&viewer_socket).await;
    std::assert_matches!(handle_sit_alert(&mut agent_control, Some(simulator), simulator, &alert), false);

    send_loopback_message(&simulator_socket, viewer_address, sit_response(object_id)).await;
    let response = receive_loopback_message(&viewer_socket).await;
    process_sit_response(&transport, &mut agent_control, simulator, Some((agent_id, session_id)), &response);
    assert_eq!(receive_loopback_message(&simulator_socket).await, Message::AgentSit(AgentSit { agent_id, session_id }));
    assert_eq!(call.await.expect("sit task"), Ok(SitOutcome::Accepted { auto_pilot: true, force_mouselook: false }));
    assert_eq!(
        *camera_updates.borrow(),
        Some(SitResponseState {
            object_id,
            accepted: true,
            camera_override: Some(SitCameraOverride { eye_offset: Vec3::new(2.0, 0.0, 1.0), at_offset: Vec3::new(0.0, 0.0, 1.0) }),
        })
    );
}

#[tokio::test]
async fn sit_failure_alert_returns_refused() {
    use crate::{
        session::{AgentControlEvent, AgentControlState, CameraUplink, SitOutcome},
        test_helpers::simulator_id,
    };

    let simulator_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback simulator");
    let viewer_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback viewer");
    let simulator = simulator_id();
    let agent_id = Uuid::from_u128(0x71);
    let session_id = Uuid::from_u128(0x72);
    let object_id = Uuid::from_u128(0x73);
    let (handle, mut agent_control, _) = AgentControlState::new();
    let call = tokio::spawn(async move { handle.sit_on_object(object_id, Vec3::ZERO).await });
    let AgentControlEvent::Command(Some(command)) = agent_control.next_event().await else { panic!("sit command") };
    let transport = LoopbackSitTransport {
        socket: &viewer_socket,
        destination: simulator_socket.local_addr().expect("simulator address"),
        sequence: std::sync::atomic::AtomicU32::new(1),
    };
    process_agent_control_command(
        &transport,
        &mut agent_control,
        &mut CameraUplink::new(),
        Some(simulator),
        Some((agent_id, session_id)),
        command,
    );
    let Message::AgentRequestSit(request) = receive_loopback_message(&simulator_socket).await else { panic!("AgentRequestSit") };
    assert_eq!(request.target_object.target_id, object_id);

    send_loopback_message(&simulator_socket, viewer_socket.local_addr().expect("viewer address"), alert_message(b"SitFailed\0")).await;
    let alert = receive_loopback_message(&viewer_socket).await;
    std::assert_matches!(handle_sit_alert(&mut agent_control, Some(simulator), simulator, &alert), true);
    assert_eq!(call.await.expect("sit task"), Ok(SitOutcome::Refused));
}

#[tokio::test]
async fn queue_full_send_keeps_sit_intake_live() {
    use crate::{
        session::{AgentControlEvent, AgentControlState, CameraUplink, SitOutcome},
        test_helpers::simulator_id,
    };

    struct FullFirstRequest {
        requests: std::sync::atomic::AtomicUsize,
    }

    impl SitTransport for FullFirstRequest {
        fn send_request(&self, _: AgentRequestSit, delivery: Delivery) -> Result<(), network::Error> {
            assert_eq!(delivery, Delivery::Reliable);
            if self.requests.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
                Err(network::Error::SimulatorCommandQueueFull)
            } else {
                Ok(())
            }
        }

        fn send_sit(&self, _: AgentSit, delivery: Delivery) -> Result<(), network::Error> {
            assert_eq!(delivery, Delivery::Reliable);
            Ok(())
        }
    }

    let transport = FullFirstRequest { requests: std::sync::atomic::AtomicUsize::new(0) };
    let simulator = simulator_id();
    let agent_id = Uuid::from_u128(0x81);
    let session_id = Uuid::from_u128(0x82);
    let (handle, mut agent_control, _) = AgentControlState::new();

    let failed = tokio::spawn({
        let handle = handle.clone();
        async move { handle.sit_on_object(Uuid::from_u128(0x83), Vec3::ZERO).await }
    });
    let AgentControlEvent::Command(Some(command)) = agent_control.next_event().await else { panic!("sit command") };
    process_agent_control_command(
        &transport,
        &mut agent_control,
        &mut CameraUplink::new(),
        Some(simulator),
        Some((agent_id, session_id)),
        command,
    );
    assert_eq!(failed.await.expect("failed sit task"), Err(crate::session::AgentControlError::SendFailed));

    let pending = tokio::spawn(async move { handle.sit_on_object(Uuid::from_u128(0x84), Vec3::ZERO).await });
    let AgentControlEvent::Command(Some(command)) = agent_control.next_event().await else { panic!("next sit command") };
    process_agent_control_command(
        &transport,
        &mut agent_control,
        &mut CameraUplink::new(),
        Some(simulator),
        Some((agent_id, session_id)),
        command,
    );

    let next_simulator_event = alert_message(b"SitFailed");
    assert!(handle_sit_alert(&mut agent_control, Some(simulator), simulator, &next_simulator_event));
    assert_eq!(pending.await.expect("pending sit task"), Ok(SitOutcome::Refused));
}
