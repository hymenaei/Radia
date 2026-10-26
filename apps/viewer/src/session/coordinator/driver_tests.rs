use std::sync::Arc;

use ::inventory::{
    AssetType, Category, CategoryKey, FolderType, Inventory, InventoryChild, InventoryLimits, InventoryOwner, InventoryType, Item, ItemKey,
    PermissionMask, Permissions, SaleInfo, SaleType, ServerVersion,
};
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

use super::{FolderFetch, apply_inventory_fetch};

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
