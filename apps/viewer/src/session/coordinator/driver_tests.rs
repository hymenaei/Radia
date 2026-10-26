use std::sync::Arc;

use ::inventory::{
    AssetType, Category, CategoryKey, FolderType, Inventory, InventoryChild, InventoryLimits, InventoryOwner, InventoryType, Item, ItemKey,
    PermissionMask, Permissions, SaleInfo, SaleType, ServerVersion,
};
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

use super::{FolderFetch, apply_inventory_fetch};

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
