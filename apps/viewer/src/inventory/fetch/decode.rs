use std::sync::Arc;

use ::inventory::{
    AssetType, Category, CategoryKey, FolderType, InventoryOwner, InventoryType, Item, ItemKey, PermissionMask, Permissions, SaleInfo,
    SaleType, ServerVersion,
};
use protocol::inventory::{Category as WireCategory, Item as WireItem};
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

use super::{FolderFetch, InventoryFetchError};

pub(super) fn ais_folders(root: &WireCategory, requested: CategoryKey) -> Result<Vec<FolderFetch>, InventoryFetchError> {
    if root.id != requested.id().uuid() {
        return Err(InventoryFetchError::InvalidResponse);
    }
    let mut folders = Vec::new();
    append_ais_folder(root, requested.owner(), &mut folders)?;
    if folders.first().is_none_or(|folder| folder.folder != requested) {
        return Err(InventoryFetchError::InvalidResponse);
    }
    Ok(folders)
}

pub(super) fn append_ais_folder(
    wire: &WireCategory,
    owner: InventoryOwner,
    folders: &mut Vec<FolderFetch>,
) -> Result<(), InventoryFetchError> {
    let folder = CategoryKey::new(owner, FolderId::new(wire.id));
    let Some(wire_children) = &wire.children else { return Ok(()) };
    let mut children = Vec::with_capacity(wire_children.len());
    let mut child_categories = Vec::new();
    for child in wire_children {
        match child {
            protocol::inventory::Child::Category(category) => {
                children.push(::inventory::InventoryChild::Category(to_category(category, owner, Some(folder))?));
                child_categories.push(category);
            }
            protocol::inventory::Child::Item(item) => children.push(::inventory::InventoryChild::Item(to_item(item, owner, folder)?)),
        }
    }
    let fetch = FolderFetch {
        folder,
        version: ServerVersion::from_raw(wire.version.unwrap_or(-1)),
        descendent_count: wire.descendent_count,
        children,
    };
    folders.push(fetch);
    for category in child_categories {
        append_ais_folder(category, owner, folders)?;
    }
    Ok(())
}

pub(crate) fn to_category(
    wire: &WireCategory,
    owner: InventoryOwner,
    parent: Option<CategoryKey>,
) -> Result<Category, InventoryFetchError> {
    validate_identity(wire.owner_id, wire.parent_id, owner, parent)?;
    if wire.name.len() > ::inventory::MAX_INVENTORY_NAME_BYTES {
        return Err(InventoryFetchError::InvalidResponse);
    }
    Ok(Category {
        key: CategoryKey::new(owner, FolderId::new(wire.id)),
        parent,
        preferred_folder_type: FolderType::from_raw(wire.preferred_folder_type.unwrap_or(-1)),
        name: Arc::from(wire.name.as_str()),
        server_version: ServerVersion::from_raw(wire.version.unwrap_or(-1)),
        server_descendent_count: wire.descendent_count,
    })
}

pub(crate) fn to_item(wire: &WireItem, owner: InventoryOwner, parent: CategoryKey) -> Result<Item, InventoryFetchError> {
    validate_identity(wire.owner_id, wire.parent_id, owner, Some(parent))?;
    if wire.name.len() > ::inventory::MAX_INVENTORY_NAME_BYTES || wire.description.len() > ::inventory::MAX_INVENTORY_DESCRIPTION_BYTES {
        return Err(InventoryFetchError::InvalidResponse);
    }
    let permissions = &wire.permissions;
    let sale = &wire.sale_info;
    Ok(Item {
        key: ItemKey::new(owner, ItemId::new(wire.id)),
        parent,
        asset_id: AssetId::new(wire.asset_id.unwrap_or_else(Uuid::nil)),
        asset_type: AssetType::from_raw(wire.asset_type.unwrap_or(-1)),
        inventory_type: InventoryType::from_raw(wire.inventory_type.unwrap_or(-1)),
        flags: wire.flags,
        name: Arc::from(wire.name.as_str()),
        description: Arc::from(wire.description.as_str()),
        created_at: wire.created_at,
        permissions: Permissions {
            creator: AvatarId::new(permissions.creator_id.unwrap_or_else(Uuid::nil)),
            owner: AvatarId::new(permissions.owner_id.unwrap_or_else(Uuid::nil)),
            group: GroupId::new(permissions.group_id.unwrap_or_else(Uuid::nil)),
            last_owner: AvatarId::new(permissions.last_owner_id.unwrap_or_else(Uuid::nil)),
            base: PermissionMask::new(permissions.base_mask.unwrap_or_default()),
            owner_mask: PermissionMask::new(permissions.owner_mask.unwrap_or_default()),
            group_mask: PermissionMask::new(permissions.group_mask.unwrap_or_default()),
            everyone: PermissionMask::new(permissions.everyone_mask.unwrap_or_default()),
            next_owner: PermissionMask::new(permissions.next_owner_mask.unwrap_or_default()),
        },
        sale: SaleInfo { sale_type: SaleType::from_raw(sale.sale_type.unwrap_or_default()), price: sale.sale_price.unwrap_or_default() },
    })
}

fn validate_identity(
    actual_owner: Option<Uuid>,
    actual_parent: Option<Uuid>,
    owner: InventoryOwner,
    parent: Option<CategoryKey>,
) -> Result<(), InventoryFetchError> {
    if actual_owner.is_some_and(|actual| actual != owner.id())
        || parent.is_some_and(|parent| actual_parent.is_some_and(|actual| actual != parent.id().uuid()))
    {
        return Err(InventoryFetchError::InvalidResponse);
    }
    Ok(())
}
