use std::sync::Arc;

use inventory::{
    AssetType, Category, CategoryKey, FolderType, Inventory, InventoryChild, InventoryDelta, InventoryError, InventoryLimits,
    InventoryOwner, InventoryType, Item, ItemKey, PermissionMask, Permissions, SaleInfo, SaleType, ServerVersion,
};
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

fn uuid(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn owner() -> InventoryOwner {
    InventoryOwner::new(uuid(1))
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

fn inventory_with_folder_and_item() -> (Inventory, CategoryKey, ItemKey) {
    let owner = owner();
    let root = category(owner, 10, None, 4, Some(1));
    let folder = category(owner, 11, Some(10), 7, Some(1));
    let initial_item = item(owner, 12, 11, "before");
    let folder_key = folder.key;
    let item_key = initial_item.key;
    let mut inventory = Inventory::new(root, None, InventoryLimits::default()).expect("root is valid");
    inventory.upsert_category(folder).expect("folder is valid");
    inventory
        .apply_folder_fetch(folder_key, ServerVersion::from_raw(7), Some(1), vec![InventoryChild::Item(initial_item)])
        .expect("fetch is valid");
    (inventory, folder_key, item_key)
}

#[test]
fn applies_categories_before_items_and_uses_returned_versions() {
    let owner = owner();
    let root = category(owner, 10, None, 4, Some(0));
    let root_key = root.key;
    let mut inventory = Inventory::new(root, None, InventoryLimits::default()).expect("root is valid");
    let folder = category(owner, 11, Some(10), 8, Some(0));
    let folder_key = folder.key;
    let created_item = item(owner, 12, 11, "created");

    let change = inventory
        .apply_delta(InventoryDelta {
            created_categories: vec![folder],
            created_items: vec![created_item.clone()],
            updated_category_versions: vec![(root_key, ServerVersion::from_raw(5)), (folder_key, ServerVersion::from_raw(9))],
            ..InventoryDelta::default()
        })
        .expect("delta is valid");

    let snapshot = inventory.snapshot();
    assert_eq!(snapshot.category(root_key).map(|value| value.server_version), Some(ServerVersion::from_raw(5)));
    assert_eq!(snapshot.category(root_key).and_then(|value| value.server_descendent_count), Some(1));
    assert_eq!(snapshot.item(created_item.key).map(|value| value.name.as_ref()), Some("created"));
    assert_eq!(snapshot.category(folder_key).map(|value| value.server_version), Some(ServerVersion::from_raw(9)));
    assert_eq!(snapshot.category(folder_key).and_then(|value| value.server_descendent_count), None);
    assert_eq!(change.refetch_categories, [folder_key]);
    assert!(snapshot.is_complete(root_key));
    assert!(!snapshot.is_complete(folder_key));
}

#[test]
fn returned_version_can_complete_a_new_folder_after_its_items_are_applied() {
    let owner = owner();
    let root = category(owner, 10, None, 4, Some(0));
    let root_key = root.key;
    let mut inventory = Inventory::new(root, None, InventoryLimits::default()).expect("root is valid");
    let folder = category(owner, 11, Some(10), -1, Some(1));
    let folder_key = folder.key;

    let change = inventory
        .apply_delta(InventoryDelta {
            created_categories: vec![folder],
            created_items: vec![item(owner, 12, 11, "created")],
            updated_category_versions: vec![(folder_key, ServerVersion::from_raw(9))],
            ..InventoryDelta::default()
        })
        .expect("authoritative folder version is applied after its children");

    let snapshot = inventory.snapshot();
    let folder = snapshot.category(folder_key).expect("folder exists");
    assert_eq!(snapshot.category(root_key).map(|category| category.server_descendent_count), Some(Some(1)));
    assert_eq!(folder.server_version, ServerVersion::from_raw(9));
    assert_eq!(folder.server_descendent_count, Some(1));
    assert!(snapshot.is_complete(folder_key));
    assert!(change.refetch_categories.is_empty());
}

#[test]
fn item_content_updates_do_not_change_parent_counts_or_versions() {
    let (mut inventory, folder, item_key) = inventory_with_folder_and_item();
    let mut changed = inventory.snapshot().item(item_key).expect("item exists").clone();
    changed.name = Arc::from("after");

    let change =
        inventory.apply_delta(InventoryDelta { updated_items: vec![changed], ..InventoryDelta::default() }).expect("item update is valid");

    let snapshot = inventory.snapshot();
    let folder = snapshot.category(folder).expect("folder exists");
    assert_eq!(folder.server_version, ServerVersion::from_raw(7));
    assert_eq!(folder.server_descendent_count, Some(1));
    assert!(snapshot.is_complete(folder.key));
    assert!(change.refetch_categories.is_empty());
}

#[test]
fn removing_a_subtree_updates_its_surviving_parent() {
    let (mut inventory, folder, _) = inventory_with_folder_and_item();
    let root = inventory.roots().personal();

    let change = inventory
        .apply_delta(InventoryDelta { removed_categories: vec![folder], ..InventoryDelta::default() })
        .expect("subtree removal is valid");

    let snapshot = inventory.snapshot();
    let root = snapshot.category(root).expect("root remains");
    assert_eq!(root.server_version, ServerVersion::from_raw(5));
    assert_eq!(root.server_descendent_count, Some(0));
    assert!(snapshot.category(folder).is_none());
    assert!(change.refetch_categories.is_empty());
}

#[test]
fn mismatched_parent_count_preserves_version_and_requests_refetch() {
    let (mut inventory, folder, _) = inventory_with_folder_and_item();
    let snapshot = inventory.snapshot();
    let existing = snapshot.items_for(folder.owner()).next().expect("one item exists").clone();
    inventory
        .apply_folder_fetch(folder, ServerVersion::from_raw(7), Some(2), vec![InventoryChild::Item(existing)])
        .expect("mismatched server count can be retained as incomplete metadata");

    let change = inventory
        .apply_delta(InventoryDelta {
            created_items: vec![item(folder.owner(), 13, folder.id().uuid().as_u128(), "new")],
            ..InventoryDelta::default()
        })
        .expect("live item update is accepted");

    let snapshot = inventory.snapshot();
    let category = snapshot.category(folder).expect("folder exists");
    assert_eq!(category.server_version, ServerVersion::from_raw(7));
    assert_eq!(category.server_descendent_count, None);
    assert_eq!(change.refetch_categories, [folder]);
    assert!(!snapshot.is_complete(folder));
}

#[test]
fn stale_folder_fetch_does_not_overwrite_a_newer_live_update() {
    let (mut inventory, folder, original_item) = inventory_with_folder_and_item();
    inventory
        .apply_delta(InventoryDelta {
            created_items: vec![item(folder.owner(), 13, folder.id().uuid().as_u128(), "new")],
            ..InventoryDelta::default()
        })
        .expect("live item update is accepted");

    let old_child = inventory.snapshot().item(original_item).expect("original item exists").clone();
    inventory
        .apply_folder_fetch(folder, ServerVersion::from_raw(7), Some(1), vec![InventoryChild::Item(old_child)])
        .expect("older fetch is ignored");

    let snapshot = inventory.snapshot();
    let category = snapshot.category(folder).expect("folder exists");
    assert_eq!(category.server_version, ServerVersion::from_raw(8));
    assert_eq!(category.server_descendent_count, Some(2));
    assert!(snapshot.item(ItemKey::new(folder.owner(), ItemId::new(uuid(13)))).is_some());
}

#[test]
fn stale_version_delta_keeps_newer_contents_and_requests_refetch() {
    let (mut inventory, folder, item_key) = inventory_with_folder_and_item();
    let mut stale = inventory.snapshot().item(item_key).expect("item exists").clone();
    stale.name = Arc::from("stale");

    let change = inventory
        .apply_delta(InventoryDelta {
            updated_items: vec![stale],
            updated_category_versions: vec![(folder, ServerVersion::from_raw(6))],
            ..InventoryDelta::default()
        })
        .expect("stale delta is ignored");

    let snapshot = inventory.snapshot();
    let current_folder = snapshot.category(folder).expect("folder exists");
    assert_eq!(current_folder.server_version, ServerVersion::from_raw(7));
    assert_eq!(current_folder.server_descendent_count, None);
    assert_eq!(snapshot.item(item_key).map(|value| value.name.as_ref()), Some("before"));
    assert_eq!(change.refetch_categories, [folder]);
    assert!(!snapshot.is_complete(folder));
}

#[test]
fn unknown_returned_version_keeps_known_version_but_marks_folder_incomplete() {
    let (mut inventory, folder, _) = inventory_with_folder_and_item();

    let change = inventory
        .apply_delta(InventoryDelta { updated_category_versions: vec![(folder, ServerVersion::UNKNOWN)], ..InventoryDelta::default() })
        .expect("unknown version is a partial outcome");

    let snapshot = inventory.snapshot();
    let category = snapshot.category(folder).expect("folder exists");
    assert_eq!(category.server_version, ServerVersion::from_raw(7));
    assert_eq!(category.server_descendent_count, None);
    assert_eq!(change.refetch_categories, [folder]);
    assert!(!snapshot.is_complete(folder));
}

#[test]
fn invalid_delta_does_not_publish_prior_operations() {
    let owner = owner();
    let root = category(owner, 10, None, 4, Some(0));
    let created = category(owner, 11, Some(10), -1, None);
    let created_key = created.key;
    let mut invalid_item = item(owner, 12, 99, "orphan");
    invalid_item.key = ItemKey::new(owner, ItemId::new(uuid(12)));
    let mut inventory = Inventory::new(root, None, InventoryLimits::default()).expect("root is valid");
    let before = inventory.revision();

    std::assert_matches!(
        inventory.apply_delta(InventoryDelta {
            created_categories: vec![created],
            created_items: vec![invalid_item],
            ..InventoryDelta::default()
        }),
        Err(InventoryError::MissingParent)
    );

    let snapshot = inventory.snapshot();
    assert_eq!(inventory.revision(), before);
    assert!(snapshot.category(created_key).is_none());
}
