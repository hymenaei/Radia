use std::{cell::Cell, collections::HashMap, sync::Arc};

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
use std::num::NonZeroU32;
use tokio::sync::oneshot;
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

use crate::{
    inventory::FolderFetch,
    session::{SitCameraOverride, SitResponseState},
};

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
        let bytes = Packet::new(sequence, message).with_reliable().encode().map_err(|_| network::Error::SessionClosed)?;
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

#[derive(Default)]
struct RecordingSitTransport {
    requests: Cell<usize>,
    sits: Cell<usize>,
}

impl SitTransport for RecordingSitTransport {
    fn send_request(&self, _: AgentRequestSit, delivery: Delivery) -> Result<(), network::Error> {
        assert_eq!(delivery, Delivery::Reliable);
        self.requests.set(self.requests.get() + 1);
        Ok(())
    }

    fn send_sit(&self, _: AgentSit, delivery: Delivery) -> Result<(), network::Error> {
        assert_eq!(delivery, Delivery::Reliable);
        self.sits.set(self.sits.get() + 1);
        Ok(())
    }
}

async fn receive_loopback_message(socket: &tokio::net::UdpSocket) -> Message {
    let mut bytes = [0; 2048];
    let (length, _) = socket.recv_from(&mut bytes).await.expect("loopback packet");
    Packet::decode(&bytes[..length]).expect("valid loopback packet").message().clone()
}

fn try_receive_loopback_message(socket: &tokio::net::UdpSocket) -> Option<Message> {
    try_receive_loopback_packet(socket).map(|packet| packet.message().clone())
}

fn try_receive_loopback_packet(socket: &tokio::net::UdpSocket) -> Option<Packet> {
    let mut bytes = [0; 2048];
    let (length, _) = socket.try_recv_from(&mut bytes).ok()?;
    Some(Packet::decode(&bytes[..length]).expect("valid loopback packet"))
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

fn sit_response(object_id: Uuid, auto_pilot: bool) -> Message {
    Message::AvatarSitResponse(AvatarSitResponse {
        sit_object: SitObject { id: Some(object_id) },
        sit_transform: SitTransform {
            auto_pilot: Some(auto_pilot),
            sit_position: Some(Vec3::new(4.0, 5.0, 6.0)),
            sit_rotation: Some(Quat::IDENTITY),
            camera_eye_offset: Some(Vec3::new(2.0, 0.0, 1.0)),
            camera_at_offset: Some(Vec3::new(0.0, 0.0, 1.0)),
            force_mouselook: Some(false),
        },
    })
}

fn sit_world_with_linkset(root_id: Uuid, child_id: Uuid) -> world::World {
    use world::{Kind, ObjectObservation, OrderEvidence::Ordered, ParentUpdate, Region, RegionInfo, UpdateSource, World};

    let mut world = World::new(crate::session::world::viewer_world_limits());
    let region = Region::new(1);
    let _ = world.adopt_region(region).expect("region");
    let _ = world.patch_region(region, RegionInfo { origin: Some(math::DVec3::ZERO), ..RegionInfo::default() }).expect("region origin");
    let root_local_id = NonZeroU32::new(1).expect("root local ID");
    let child_local_id = NonZeroU32::new(2).expect("child local ID");
    let _ = world
        .observe_object(ObjectObservation::new(
            UpdateSource::new(region, 1, Ordered(1), 0),
            root_id,
            root_local_id,
            Kind::Object,
            ParentUpdate::Root,
        ))
        .expect("root object");
    let _ = world
        .observe_object(ObjectObservation::new(
            UpdateSource::new(region, 2, Ordered(2), 0),
            child_id,
            child_local_id,
            Kind::Object,
            ParentUpdate::Local(root_local_id),
        ))
        .expect("child object");
    world
}

fn empty_sit_world() -> world::World {
    world::World::new(crate::session::world::viewer_world_limits())
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

    send_loopback_message(&simulator_socket, viewer_address, sit_response(object_id, false)).await;
    let response = receive_loopback_message(&viewer_socket).await;
    let mut camera = CameraUplink::new();
    camera.movement_sender().send_replace(Some(crate::agent_movement::MovementObservation {
        control_flags: crate::agent_movement::FLY,
        body_rotation: Quat::IDENTITY,
        always_run: false,
        stop_sequence: 0,
    }));
    let world = sit_world_with_linkset(object_id, Uuid::from_u128(0x64));
    process_sit_response(&transport, &mut agent_control, &mut camera, &world, simulator, Some((agent_id, session_id)), &response);
    assert_eq!(receive_loopback_message(&simulator_socket).await, Message::AgentSit(AgentSit { agent_id, session_id }));
    assert_eq!(call.await.expect("sit task"), Ok(SitOutcome::Accepted { auto_pilot: false, force_mouselook: false }));
    assert_eq!(camera.movement_sender().borrow().expect("current movement").control_flags & crate::agent_movement::FLY, 0);
    assert_eq!(
        *camera_updates.borrow(),
        Some(SitResponseState {
            generation: 0,
            object_id,
            requested_object_id: object_id,
            camera_override: Some(SitCameraOverride { eye_offset: Vec3::new(2.0, 0.0, 1.0), at_offset: Vec3::new(0.0, 0.0, 1.0) }),
            force_mouselook: false,
        })
    );
}

#[tokio::test]
async fn root_response_accepts_child_sit() {
    use crate::{
        session::{AgentControlEvent, AgentControlState, CameraUplink},
        test_helpers::simulator_id,
    };

    let simulator_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback simulator");
    let viewer_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback viewer");
    let simulator = simulator_id();
    let agent_id = Uuid::from_u128(0x91);
    let session_id = Uuid::from_u128(0x92);
    let root_id = Uuid::from_u128(0x93);
    let child_id = Uuid::from_u128(0x94);
    let world = sit_world_with_linkset(root_id, child_id);
    let (handle, mut agent_control, sit_updates) = AgentControlState::new();
    let call = tokio::spawn(async move { handle.sit_on_object(child_id, Vec3::ZERO).await });
    let AgentControlEvent::Command(Some(command)) = agent_control.next_event().await else { panic!("sit command") };
    let transport = LoopbackSitTransport {
        socket: &viewer_socket,
        destination: simulator_socket.local_addr().expect("simulator address"),
        sequence: std::sync::atomic::AtomicU32::new(1),
    };
    let mut camera = CameraUplink::new();
    process_agent_control_command(&transport, &mut agent_control, &mut camera, Some(simulator), Some((agent_id, session_id)), command);
    assert_eq!(
        receive_loopback_message(&simulator_socket).await,
        Message::AgentRequestSit(AgentRequestSit {
            agent_data: SitAgentData { agent_id, session_id },
            target_object: TargetObject { target_id: child_id, offset: Vec3::ZERO },
        })
    );

    send_loopback_message(&simulator_socket, viewer_socket.local_addr().expect("viewer address"), sit_response(root_id, false)).await;
    let response = receive_loopback_message(&viewer_socket).await;
    process_sit_response(&transport, &mut agent_control, &mut camera, &world, simulator, Some((agent_id, session_id)), &response);

    let packet = try_receive_loopback_packet(&simulator_socket).expect("AgentSit packet");
    assert!(packet.is_reliable());
    assert_eq!(packet.message(), &Message::AgentSit(AgentSit { agent_id, session_id }));
    assert_eq!(call.await.expect("sit task"), Ok(crate::session::SitOutcome::Accepted { auto_pilot: false, force_mouselook: false }));
    assert_eq!(
        *sit_updates.borrow(),
        Some(SitResponseState {
            generation: 0,
            object_id: root_id,
            requested_object_id: child_id,
            camera_override: Some(SitCameraOverride { eye_offset: Vec3::new(2.0, 0.0, 1.0), at_offset: Vec3::new(0.0, 0.0, 1.0) }),
            force_mouselook: false,
        })
    );
}

#[tokio::test]
async fn invalid_response_sends_no_sit() {
    use crate::{
        session::{AgentControlError, AgentControlEvent, AgentControlState, CameraUplink},
        test_helpers::simulator_id,
    };

    let simulator = simulator_id();
    let agent_id = Uuid::from_u128(0x68);
    let session_id = Uuid::from_u128(0x69);
    let world = empty_sit_world();
    for (index, invalid_transform) in [Some(true), Some(false), None].into_iter().enumerate() {
        let object_id = Uuid::from_u128(0x70 + index as u128);
        let (handle, mut agent_control, sit_updates) = AgentControlState::new();
        let call = tokio::spawn(async move { handle.sit_on_object(object_id, Vec3::ZERO).await });
        let AgentControlEvent::Command(Some(command)) = agent_control.next_event().await else { panic!("sit command") };
        let transport = RecordingSitTransport::default();
        let mut camera = CameraUplink::new();
        process_agent_control_command(&transport, &mut agent_control, &mut camera, Some(simulator), Some((agent_id, session_id)), command);

        let Message::AvatarSitResponse(mut response) = sit_response(object_id, false) else { panic!("sit response") };
        match invalid_transform {
            Some(true) => response.sit_transform.sit_position = None,
            Some(false) => response.sit_transform.sit_rotation = Some(Quat::from_xyzw(0.0, 0.0, 0.0, 0.0)),
            None => {}
        }
        process_sit_response(
            &transport,
            &mut agent_control,
            &mut camera,
            &world,
            simulator,
            Some((agent_id, session_id)),
            &Message::AvatarSitResponse(response),
        );

        assert_eq!(call.await.expect("sit task"), Err(AgentControlError::InvalidSitResponse), "invalid transform: {invalid_transform:?}");
        assert_eq!(transport.requests.get(), 1);
        assert_eq!(transport.sits.get(), 0);
        assert_eq!(*sit_updates.borrow(), None);
    }
}

#[tokio::test]
async fn auto_pilot_waits_for_parent() {
    use crate::{
        session::{AgentControlEvent, AgentControlState, CameraUplink, SitOutcome},
        test_helpers::simulator_id,
    };
    use world::{Kind, ObjectObservation, ObjectPatch, OrderEvidence::Ordered, Parent, ParentUpdate, Pose, Region, UpdateSource};

    let simulator_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback simulator");
    let viewer_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback viewer");
    let simulator = simulator_id();
    let agent_id = Uuid::from_u128(0x7a);
    let session_id = Uuid::from_u128(0x7b);
    let object_id = Uuid::from_u128(0x7c);
    let mut world = sit_world_with_linkset(object_id, Uuid::from_u128(0x7d));
    let region = Region::new(1);
    let agent_local_id = NonZeroU32::new(3).expect("agent local ID");
    let seat_local_id = NonZeroU32::new(1).expect("seat local ID");
    world.set_self_agent(agent_id).expect("self agent");
    let mut agent =
        ObjectObservation::new(UpdateSource::new(region, 3, Ordered(3), 0), agent_id, agent_local_id, Kind::Avatar, ParentUpdate::Root);
    agent.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
    agent.scale = Some(Vec3::new(0.6, 0.45, 1.9));
    assert!(world.observe_object(agent).expect("standing agent").revision.is_some());

    let (handle, mut agent_control, sit_updates) = AgentControlState::new();
    let call = tokio::spawn({
        let handle = handle.clone();
        async move { handle.sit_on_object(object_id, Vec3::ZERO).await }
    });
    let AgentControlEvent::Command(Some(command)) = agent_control.next_event().await else { panic!("sit command") };
    let transport = LoopbackSitTransport {
        socket: &viewer_socket,
        destination: simulator_socket.local_addr().expect("simulator address"),
        sequence: std::sync::atomic::AtomicU32::new(1),
    };
    let mut camera = CameraUplink::new();
    camera.movement_sender().send_replace(Some(crate::agent_movement::MovementObservation {
        control_flags: crate::agent_movement::FLY,
        body_rotation: Quat::IDENTITY,
        always_run: false,
        stop_sequence: 0,
    }));
    process_agent_control_command(&transport, &mut agent_control, &mut camera, Some(simulator), Some((agent_id, session_id)), command);
    assert_eq!(
        receive_loopback_message(&simulator_socket).await,
        Message::AgentRequestSit(AgentRequestSit {
            agent_data: SitAgentData { agent_id, session_id },
            target_object: TargetObject { target_id: object_id, offset: Vec3::ZERO },
        })
    );

    let Message::AvatarSitResponse(mut response) = sit_response(object_id, true) else { panic!("sit response") };
    response.sit_transform.force_mouselook = Some(true);
    send_loopback_message(&simulator_socket, viewer_socket.local_addr().expect("viewer address"), Message::AvatarSitResponse(response))
        .await;
    let response = receive_loopback_message(&viewer_socket).await;
    process_sit_response(&transport, &mut agent_control, &mut camera, &world, simulator, Some((agent_id, session_id)), &response);

    assert_eq!(call.await.expect("sit task"), Ok(SitOutcome::Accepted { auto_pilot: true, force_mouselook: true }));
    assert_eq!(camera.movement_sender().borrow().expect("current movement").control_flags & crate::agent_movement::FLY, 0);
    assert_eq!(
        *sit_updates.borrow(),
        Some(SitResponseState {
            generation: 0,
            object_id,
            requested_object_id: object_id,
            camera_override: Some(SitCameraOverride { eye_offset: Vec3::new(2.0, 0.0, 1.0), at_offset: Vec3::new(0.0, 0.0, 1.0) }),
            force_mouselook: true,
        })
    );
    let agent = world.entity(agent_id).and_then(|entity| world.object(entity)).expect("agent object");
    std::assert_matches!(agent.parent(), Parent::Root);
    assert_eq!(try_receive_loopback_message(&simulator_socket), None);

    let mut parent = ObjectPatch::new(UpdateSource::new(region, 4, Ordered(4), 0), agent_local_id);
    parent.parent = Some(ParentUpdate::Local(seat_local_id));
    assert!(world.patch_object(parent).expect("simulator parent update").revision.is_some());
    let agent = world.entity(agent_id).and_then(|entity| world.object(entity)).expect("agent object");
    std::assert_matches!(agent.parent(), Parent::Resolved(_));
    assert_eq!(try_receive_loopback_message(&simulator_socket), None);
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

#[tokio::test]
async fn action_overflow_preserves_sit() {
    use crate::{
        session::{AgentControlError, AgentControlEvent, AgentControlState, CameraUplink, SitOutcome},
        test_helpers::simulator_id,
    };

    let simulator_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback simulator");
    let viewer_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback viewer");
    let simulator = simulator_id();
    let agent_id = Uuid::from_u128(0x91);
    let session_id = Uuid::from_u128(0x92);
    let object_id = Uuid::from_u128(0x93);
    let world = sit_world_with_linkset(object_id, Uuid::from_u128(0x94));
    let (handle, mut agent_control, _) = AgentControlState::new();
    let transport = LoopbackSitTransport {
        socket: &viewer_socket,
        destination: simulator_socket.local_addr().expect("simulator address"),
        sequence: std::sync::atomic::AtomicU32::new(1),
    };
    let mut camera = CameraUplink::new();

    loop {
        let ground_sit = tokio::spawn({
            let handle = handle.clone();
            async move { handle.sit_on_ground().await }
        });
        let AgentControlEvent::Command(Some(command)) = agent_control.next_event().await else { panic!("ground-sit command") };
        process_agent_control_command(&transport, &mut agent_control, &mut camera, Some(simulator), Some((agent_id, session_id)), command);
        match ground_sit.await.expect("ground-sit task") {
            Ok(()) => {}
            Err(AgentControlError::QueueFull) => break,
            result => panic!("unexpected ground-sit result: {result:?}"),
        }
    }

    let sit = tokio::spawn({
        let handle = handle.clone();
        async move { handle.sit_on_object(object_id, Vec3::ZERO).await }
    });
    let AgentControlEvent::Command(Some(command)) = agent_control.next_event().await else { panic!("sit command") };
    process_agent_control_command(&transport, &mut agent_control, &mut camera, Some(simulator), Some((agent_id, session_id)), command);
    assert_eq!(
        receive_loopback_message(&simulator_socket).await,
        Message::AgentRequestSit(AgentRequestSit {
            agent_data: SitAgentData { agent_id, session_id },
            target_object: TargetObject { target_id: object_id, offset: Vec3::ZERO },
        })
    );

    let stand = tokio::spawn(async move { handle.stand_up().await });
    let AgentControlEvent::Command(Some(command)) = agent_control.next_event().await else { panic!("stand-up command") };
    process_agent_control_command(&transport, &mut agent_control, &mut camera, Some(simulator), Some((agent_id, session_id)), command);
    assert_eq!(stand.await.expect("stand task"), Err(AgentControlError::QueueFull));

    send_loopback_message(&simulator_socket, viewer_socket.local_addr().expect("viewer address"), sit_response(object_id, false)).await;
    let response = receive_loopback_message(&viewer_socket).await;
    process_sit_response(&transport, &mut agent_control, &mut camera, &world, simulator, Some((agent_id, session_id)), &response);
    assert_eq!(receive_loopback_message(&simulator_socket).await, Message::AgentSit(AgentSit { agent_id, session_id }));
    assert_eq!(sit.await.expect("sit task"), Ok(SitOutcome::Accepted { auto_pilot: false, force_mouselook: false }));
}
