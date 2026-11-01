use std::{collections::HashSet, sync::Arc};

use ::inventory::{
    AssetType, Category, CategoryKey, FolderType, Inventory, InventoryChild, InventoryDelta, InventoryLimits, InventoryNodeId,
    InventoryOwner, InventoryType, Item, ItemKey, PermissionMask, Permissions, SaleInfo, SaleType, ServerVersion,
};
use llsd::Value;
use protocol::{
    lludp::Message,
    messages::{BulkUpdateInventory, InventoryFolderBlock, InventoryItemBlock, SaveAssetIntoInventory, UpdateCreateInventoryItem},
};
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

use super::{apply_event_queue, apply_udp, is_valid_inventory_event};

fn uuid(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn category(owner: InventoryOwner, id: u128, parent: Option<u128>, version: i32, count: Option<u32>) -> Category {
    Category {
        key: CategoryKey::new(owner, FolderId::new(uuid(id))),
        parent: parent.map(|parent| CategoryKey::new(owner, FolderId::new(uuid(parent)))),
        preferred_folder_type: FolderType::None,
        name: Arc::from(format!("folder-{id}")),
        server_version: ServerVersion::from_raw(version),
        server_descendent_count: count,
    }
}

fn item(owner: InventoryOwner, id: u128, parent: u128, name: &str) -> Item {
    Item {
        key: ItemKey::new(owner, ItemId::new(uuid(id))),
        parent: CategoryKey::new(owner, FolderId::new(uuid(parent))),
        asset_id: AssetId::new(uuid(id + 100)),
        asset_type: AssetType::Object,
        inventory_type: InventoryType::Object,
        flags: 0,
        name: Arc::from(name),
        description: Arc::from("description"),
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
    }
}

fn wire_item(id: u128, parent: u128, name: &str) -> InventoryItemBlock {
    InventoryItemBlock {
        item_id: uuid(id),
        folder_id: uuid(parent),
        callback_id: 0,
        creator_id: Uuid::nil(),
        owner_id: Uuid::nil(),
        group_id: Uuid::nil(),
        base_mask: 0,
        owner_mask: 0,
        group_mask: 0,
        everyone_mask: 0,
        next_owner_mask: 0,
        group_owned: false,
        asset_id: uuid(id + 100),
        asset_type: 6,
        inventory_type: 6,
        flags: 0,
        sale_type: 0,
        sale_price: 0,
        name: name.to_owned(),
        description: "description".to_owned(),
        creation_date: 0,
        crc: 0,
    }
}

fn value_map(fields: impl IntoIterator<Item = (String, Value)>) -> Value {
    Value::Map(fields.into_iter().collect())
}

fn event_item(id: u128, parent: u128, name: &str) -> Value {
    value_map([
        ("ItemID".to_owned(), Value::Uuid(uuid(id))),
        ("FolderID".to_owned(), Value::Uuid(uuid(parent))),
        ("CallbackID".to_owned(), Value::Integer(0)),
        ("CreatorID".to_owned(), Value::Uuid(Uuid::nil())),
        ("OwnerID".to_owned(), Value::Uuid(Uuid::nil())),
        ("GroupID".to_owned(), Value::Uuid(Uuid::nil())),
        ("BaseMask".to_owned(), Value::Integer(0)),
        ("OwnerMask".to_owned(), Value::Integer(0)),
        ("GroupMask".to_owned(), Value::Integer(0)),
        ("EveryoneMask".to_owned(), Value::Integer(0)),
        ("NextOwnerMask".to_owned(), Value::Integer(0)),
        ("GroupOwned".to_owned(), Value::Boolean(false)),
        ("AssetID".to_owned(), Value::Uuid(uuid(id + 100))),
        ("Type".to_owned(), Value::Integer(6)),
        ("InvType".to_owned(), Value::Integer(6)),
        ("Flags".to_owned(), Value::Integer(0)),
        ("SaleType".to_owned(), Value::Integer(0)),
        ("SalePrice".to_owned(), Value::Integer(0)),
        ("Name".to_owned(), Value::String(name.to_owned())),
        ("Description".to_owned(), Value::String("description".to_owned())),
        ("CreationDate".to_owned(), Value::Integer(0)),
        ("CRC".to_owned(), Value::Integer(0)),
    ])
}

fn bulk_event_body(agent: Uuid, folders: Vec<Value>, items: Vec<Value>) -> Value {
    value_map([
        (
            "AgentData".to_owned(),
            Value::Array(vec![value_map([
                ("AgentID".to_owned(), Value::Uuid(agent)),
                ("TransactionID".to_owned(), Value::Uuid(Uuid::nil())),
            ])]),
        ),
        ("FolderData".to_owned(), Value::Array(folders)),
        ("ItemData".to_owned(), Value::Array(items)),
    ])
}

fn save_asset_event_body(agent: Uuid, item_id: Uuid, new_asset_id: Uuid) -> Value {
    value_map([
        ("AgentData".to_owned(), Value::Array(vec![value_map([("AgentID".to_owned(), Value::Uuid(agent))])])),
        (
            "InventoryData".to_owned(),
            Value::Array(vec![value_map([
                ("ItemID".to_owned(), Value::Uuid(item_id)),
                ("NewAssetID".to_owned(), Value::Uuid(new_asset_id)),
            ])]),
        ),
    ])
}

fn folder_child_items(inventory: &Inventory, folder: CategoryKey) -> Vec<InventoryChild> {
    let snapshot = inventory.snapshot();
    snapshot
        .children(folder)
        .filter_map(|node| match node.id() {
            InventoryNodeId::Category(id) => snapshot.category(CategoryKey::new(node.owner(), id)).cloned().map(InventoryChild::Category),
            InventoryNodeId::Item(id) => snapshot.item(ItemKey::new(node.owner(), id)).cloned().map(InventoryChild::Item),
        })
        .collect()
}

fn inventory_with_two_folders(agent: Uuid) -> (Inventory, InventoryOwner, [CategoryKey; 2]) {
    let owner = InventoryOwner::new(agent);
    let root = category(owner, 10, None, 4, Some(2));
    let folders = [category(owner, 11, Some(10), 3, Some(0)), category(owner, 12, Some(10), 3, Some(0))];
    let keys = [folders[0].key, folders[1].key];
    let mut inventory = Inventory::new(root, None, InventoryLimits::default()).expect("valid root");
    for folder in folders {
        inventory.upsert_category(folder).expect("valid folder");
    }
    (inventory, owner, keys)
}

#[test]
fn udp_and_event_queue_bulk_updates_reach_the_session_inventory() {
    let agent = uuid(1);
    let owner = InventoryOwner::new(agent);
    let root = category(owner, 10, None, 4, Some(0));
    let root_id = root.key.id().uuid();
    let mut inventory = Inventory::new(root, None, InventoryLimits::default()).expect("valid root");
    let udp_folder = InventoryFolderBlock { folder_id: uuid(11), parent_id: root_id, folder_type: 6, name: "udp".to_owned() };
    let udp = Message::BulkUpdateInventory(BulkUpdateInventory {
        agent_id: agent,
        transaction_id: Uuid::nil(),
        folder_data: vec![udp_folder],
        item_data: vec![wire_item(21, 11, "udp item")],
        invalid_blocks: 0,
    });
    apply_udp(&mut inventory, agent, &udp).expect("UDP update applies").expect("message is handled");
    assert!(inventory.snapshot().category(CategoryKey::new(owner, FolderId::new(uuid(11)))).is_some());
    assert!(inventory.snapshot().item(ItemKey::new(owner, ItemId::new(uuid(21)))).is_some());

    let event_folder = value_map([
        ("FolderID".to_owned(), Value::Uuid(uuid(12))),
        ("ParentID".to_owned(), Value::Uuid(root_id)),
        ("Type".to_owned(), Value::Integer(6)),
        ("Name".to_owned(), Value::String("event queue".to_owned())),
    ]);
    let body = bulk_event_body(agent, vec![event_folder], vec![event_item(22, 12, "event item")]);
    let result = apply_event_queue(&mut inventory, agent, "BulkUpdateInventory", &body)
        .expect("EventQueue update parses and applies")
        .expect("message is handled");
    assert!(!result.changes.is_empty());
    assert!(inventory.snapshot().category(CategoryKey::new(owner, FolderId::new(uuid(12)))).is_some());
    assert!(inventory.snapshot().item(ItemKey::new(owner, ItemId::new(uuid(22)))).is_some());
}

#[test]
fn save_asset_updates_loaded_items_for_udp_and_event_queue() {
    let agent = uuid(1);
    let owner = InventoryOwner::new(agent);
    let root = category(owner, 10, None, 4, Some(0));
    let mut inventory = Inventory::new(root, None, InventoryLimits::default()).expect("valid root");
    let saved_item = item(owner, 20, 10, "saved");
    let item_key = saved_item.key;
    inventory.upsert_item(saved_item).expect("item exists");

    let udp_asset = uuid(220);
    let udp =
        Message::SaveAssetIntoInventory(SaveAssetIntoInventory { agent_id: agent, item_id: item_key.id().uuid(), new_asset_id: udp_asset });
    apply_udp(&mut inventory, agent, &udp).expect("UDP save asset applies").expect("message is handled");
    assert_eq!(inventory.snapshot().item(item_key).map(|item| item.asset_id), Some(AssetId::new(udp_asset)));

    let event_asset = uuid(121);
    let body = save_asset_event_body(agent, item_key.id().uuid(), event_asset);
    apply_event_queue(&mut inventory, agent, "SaveAssetIntoInventory", &body)
        .expect("EventQueue save asset applies")
        .expect("message is handled");
    assert_eq!(inventory.snapshot().item(item_key).map(|item| item.asset_id), Some(AssetId::new(event_asset)));

    let revision = inventory.revision();
    let missing = Message::SaveAssetIntoInventory(SaveAssetIntoInventory { agent_id: agent, item_id: uuid(99), new_asset_id: uuid(199) });
    let outcome = apply_udp(&mut inventory, agent, &missing).expect("missing item is ignored").expect("message is handled");
    assert!(outcome.changes.is_empty());
    assert_eq!(inventory.revision(), revision);
}

#[test]
fn messages_for_another_agent_are_ignored_for_udp_and_event_queue() {
    let agent = uuid(1);
    let other_agent = uuid(2);
    let owner = InventoryOwner::new(agent);
    let root = category(owner, 10, None, 4, Some(0));
    let root_id = root.key.id().uuid();
    let mut inventory = Inventory::new(root, None, InventoryLimits::default()).expect("valid root");
    let udp = Message::BulkUpdateInventory(BulkUpdateInventory {
        agent_id: other_agent,
        transaction_id: Uuid::nil(),
        folder_data: vec![InventoryFolderBlock { folder_id: uuid(11), parent_id: root_id, folder_type: 6, name: "ignored".to_owned() }],
        item_data: Vec::new(),
        invalid_blocks: 0,
    });
    assert!(apply_udp(&mut inventory, agent, &udp).expect("foreign update is ignored").is_none());
    let body = bulk_event_body(other_agent, Vec::new(), Vec::new());
    assert!(apply_event_queue(&mut inventory, agent, "BulkUpdateInventory", &body).expect("foreign event is ignored").is_none());
    assert_eq!(inventory.snapshot().categories().count(), 1);
}

#[test]
fn bootstrap_event_validation_rejects_malformed_and_foreign_events() {
    let agent = uuid(1);
    let foreign_agent = uuid(2);
    let valid = save_asset_event_body(agent, uuid(10), uuid(11));
    let foreign = save_asset_event_body(foreign_agent, uuid(10), uuid(11));
    let malformed = value_map([("AgentData".to_owned(), Value::String("invalid".to_owned()))]);

    assert!(is_valid_inventory_event("SaveAssetIntoInventory", &valid, agent));
    assert!(!is_valid_inventory_event("SaveAssetIntoInventory", &foreign, agent));
    assert!(!is_valid_inventory_event("SaveAssetIntoInventory", &malformed, agent));
    assert!(!is_valid_inventory_event("UnknownMessage", &valid, agent));
}

fn inventory_invariants_hold(inventory: &Inventory, owner: InventoryOwner) -> bool {
    let snapshot = inventory.snapshot();
    let mut expected_children = HashSet::new();
    let roots = snapshot.roots();
    for category in snapshot.categories_for(owner) {
        if roots.contains(category.key) {
            if category.parent.is_some() {
                return false;
            }
        } else {
            let Some(parent) = category.parent else { return false };
            if parent.owner() != owner || snapshot.category(parent).is_none() {
                return false;
            }
            if !expected_children.insert((parent, InventoryNodeId::Category(category.key.id()))) {
                return false;
            }
        }
        let complete = category.server_version.is_known()
            && category.server_descendent_count.and_then(|count| usize::try_from(count).ok()) == Some(snapshot.child_count(category.key));
        if snapshot.is_complete(category.key) != complete {
            return false;
        }
    }
    for item in snapshot.items_for(owner) {
        if item.parent.owner() != owner || snapshot.category(item.parent).is_none() {
            return false;
        }
        if !expected_children.insert((item.parent, InventoryNodeId::Item(item.key.id()))) {
            return false;
        }
    }
    for category in snapshot.categories_for(owner) {
        let actual = snapshot.children(category.key).map(|node| (category.key, node.id())).collect::<HashSet<_>>();
        let expected = expected_children.iter().copied().filter(|(parent, _)| *parent == category.key).collect::<HashSet<_>>();
        if actual != expected {
            return false;
        }
    }
    let incomplete = snapshot.categories_for(owner).filter(|category| !snapshot.is_complete(category.key)).count();
    snapshot.incomplete_category_count(owner) == incomplete && snapshot.owner_is_complete(owner) == (incomplete == 0)
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config { cases: 24, ..proptest::test_runner::Config::default() })]

    #[test]
    fn interleaved_fetch_udp_event_queue_and_delta_updates_preserve_inventory(operations in proptest::collection::vec(proptest::prelude::any::<u8>(), 1..32)) {
        let agent = uuid(1);
        let (mut inventory, owner, folders) = inventory_with_two_folders(agent);
        for (index, operation) in operations.into_iter().enumerate() {
            let folder = folders[usize::from(operation & 1)];
            match operation % 4 {
                0 => {
                    let current_version = inventory.snapshot().category(folder).expect("folder exists").server_version;
                    let version = ServerVersion::from_raw(current_version.raw().saturating_add(1));
                    let count = u32::try_from(folder_child_items(&inventory, folder).len()).expect("bounded child count");
                    inventory
                        .apply_folder_fetch(folder, version, Some(count), folder_child_items(&inventory, folder))
                        .expect("current folder fetch applies");
                }
                1 => {
                    let id = 1_000 + index as u128;
                    let message = Message::UpdateCreateInventoryItem(UpdateCreateInventoryItem {
                        agent_id: agent,
                        sim_approved: false,
                        transaction_id: Uuid::nil(),
                        inventory_data: vec![wire_item(id, folder.id().uuid().as_u128(), "udp item")],
                        invalid_blocks: 0,
                    });
                    apply_udp(&mut inventory, agent, &message).expect("UDP update applies").expect("message is handled");
                }
                2 => {
                    let id = 2_000 + index as u128;
                    let body = bulk_event_body(agent, Vec::new(), vec![event_item(id, folder.id().uuid().as_u128(), "event item")]);
                    apply_event_queue(&mut inventory, agent, "BulkUpdateInventory", &body)
                        .expect("EventQueue update applies")
                        .expect("message is handled");
                }
                _ => {
                    if operation & 0x10 != 0 {
                        let current_version = inventory.snapshot().category(folder).expect("folder exists").server_version;
                        let stale_version = ServerVersion::from_raw(current_version.raw().saturating_sub(1));
                        let delta = InventoryDelta {
                            updated_category_versions: vec![(folder, stale_version)],
                            ..InventoryDelta::default()
                        };
                        inventory.apply_delta(delta).expect("stale version requests a refetch");
                    } else if let Some(item) = inventory.snapshot().items_for(owner).next() {
                        inventory
                            .apply_delta(InventoryDelta {
                                removed_items: vec![item.key],
                                ..InventoryDelta::default()
                            })
                            .expect("item removal applies");
                    } else {
                        inventory
                            .apply_delta(InventoryDelta {
                                created_items: vec![item(owner, 3_000 + index as u128, folder.id().uuid().as_u128(), "delta item")],
                                ..InventoryDelta::default()
                            })
                            .expect("item create applies");
                    }
                }
            }
            proptest::prop_assert!(inventory_invariants_hold(&inventory, owner), "model invariants failed after operation {operation}");
        }
    }
}
