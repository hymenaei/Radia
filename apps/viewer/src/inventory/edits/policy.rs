use std::collections::HashSet;

use ::inventory::{Category, CategoryKey, FolderType, InventoryNodeId, InventoryNodeKey, InventoryOwner, InventorySnapshot};

use super::types::InventoryEditRejection;

#[derive(Clone, Copy)]
pub(crate) enum EditIntent {
    CreateFolder { parent: CategoryKey },
    Rename { node: InventoryNodeKey },
    Move { node: InventoryNodeKey, destination: Option<CategoryKey> },
    Delete { node: InventoryNodeKey },
    EmptyTrash,
}

pub(crate) fn validate_edit(snapshot: &InventorySnapshot, owner: InventoryOwner, intent: EditIntent) -> Result<(), InventoryEditRejection> {
    match intent {
        EditIntent::CreateFolder { parent } => {
            validate_owner(parent.owner(), owner)?;
            let category = snapshot.category(parent).ok_or(InventoryEditRejection::MissingCategory)?;
            if folder_blocks_creation(snapshot, category.key) {
                return Err(InventoryEditRejection::OperationNotAllowed);
            }
        }
        EditIntent::Rename { node } => {
            validate_owner(node.owner(), owner)?;
            match node.id() {
                InventoryNodeId::Category(id) => {
                    let key = CategoryKey::new(owner, id);
                    let category = snapshot.category(key).ok_or(InventoryEditRejection::MissingCategory)?;
                    validate_category_source(snapshot, category)?;
                    if category_is_within(snapshot, key, FolderType::Trash) {
                        return Err(InventoryEditRejection::OperationNotAllowed);
                    }
                }
                InventoryNodeId::Item(id) => {
                    let item = snapshot.item(::inventory::ItemKey::new(owner, id)).ok_or(InventoryEditRejection::MissingItem)?;
                    if item.inventory_type == ::inventory::InventoryType::CallingCard
                        || category_is_within(snapshot, item.parent, FolderType::Trash)
                        || category_is_within(snapshot, item.parent, FolderType::Inbox)
                    {
                        return Err(InventoryEditRejection::OperationNotAllowed);
                    }
                }
            }
        }
        EditIntent::Move { node, destination } => {
            validate_owner(node.owner(), owner)?;
            let from = match node.id() {
                InventoryNodeId::Category(id) => {
                    let key = CategoryKey::new(owner, id);
                    let category = snapshot.category(key).ok_or(InventoryEditRejection::MissingCategory)?;
                    validate_category_source(snapshot, category)?;
                    category.parent
                }
                InventoryNodeId::Item(id) => {
                    let item = snapshot.item(::inventory::ItemKey::new(owner, id)).ok_or(InventoryEditRejection::MissingItem)?;
                    if category_is_within(snapshot, item.parent, FolderType::CurrentOutfit) {
                        return Err(InventoryEditRejection::OperationNotAllowed);
                    }
                    Some(item.parent)
                }
            };
            if let Some(destination) = destination {
                validate_owner(destination.owner(), owner)?;
                let target = snapshot.category(destination).ok_or(InventoryEditRejection::MissingCategory)?;
                if category_is_within(snapshot, target.key, FolderType::CurrentOutfit) {
                    return Err(InventoryEditRejection::OperationNotAllowed);
                }
                if let InventoryNodeId::Category(id) = node.id()
                    && from != Some(destination)
                    && folder_would_cycle(CategoryKey::new(owner, id), destination, snapshot)
                {
                    return Err(InventoryEditRejection::FolderCycle);
                }
            }
        }
        EditIntent::Delete { node } => {
            validate_owner(node.owner(), owner)?;
            match node.id() {
                InventoryNodeId::Category(id) => {
                    let key = CategoryKey::new(owner, id);
                    let category = snapshot.category(key).ok_or(InventoryEditRejection::MissingCategory)?;
                    validate_category_source(snapshot, category)?;
                    if category_is_within(snapshot, key, FolderType::Trash) {
                        return Err(InventoryEditRejection::OperationNotAllowed);
                    }
                }
                InventoryNodeId::Item(id) => {
                    let item = snapshot.item(::inventory::ItemKey::new(owner, id)).ok_or(InventoryEditRejection::MissingItem)?;
                    if category_is_within(snapshot, item.parent, FolderType::Trash)
                        || category_is_within(snapshot, item.parent, FolderType::CurrentOutfit)
                    {
                        return Err(InventoryEditRejection::OperationNotAllowed);
                    }
                }
            }
        }
        EditIntent::EmptyTrash => {
            snapshot.category_with_type(owner, FolderType::Trash).ok_or(InventoryEditRejection::MissingTrash)?;
        }
    }
    Ok(())
}

fn validate_owner(actual: InventoryOwner, expected: InventoryOwner) -> Result<(), InventoryEditRejection> {
    if actual == expected { Ok(()) } else { Err(InventoryEditRejection::DifferentOwner) }
}

fn validate_category_source(snapshot: &InventorySnapshot, category: &Category) -> Result<(), InventoryEditRejection> {
    if snapshot.roots().contains(category.key) {
        return Err(InventoryEditRejection::RootFolder);
    }
    if category.preferred_folder_type.is_protected() {
        return Err(InventoryEditRejection::ProtectedFolder);
    }
    Ok(())
}

fn folder_blocks_creation(snapshot: &InventorySnapshot, parent: CategoryKey) -> bool {
    let mut current = Some(parent);
    let mut seen = HashSet::new();
    while let Some(key) = current {
        if !seen.insert(key) {
            return true;
        }
        let Some(category) = snapshot.category(key) else { return true };
        if matches!(
            category.preferred_folder_type,
            FolderType::Trash
                | FolderType::CurrentOutfit
                | FolderType::Inbox
                | FolderType::MyOutfits
                | FolderType::MarketplaceListings
                | FolderType::MarketplaceStock
                | FolderType::MarketplaceVersion
        ) || (key == parent && matches!(category.preferred_folder_type, FolderType::Favorite | FolderType::LostAndFound))
        {
            return true;
        }
        current = category.parent;
    }
    false
}

fn category_is_within(snapshot: &InventorySnapshot, folder: CategoryKey, folder_type: FolderType) -> bool {
    let mut current = Some(folder);
    let mut seen = HashSet::new();
    while let Some(key) = current {
        if !seen.insert(key) {
            return true;
        }
        let Some(category) = snapshot.category(key) else { return false };
        if category.preferred_folder_type == folder_type {
            return true;
        }
        current = category.parent;
    }
    false
}

fn folder_would_cycle(folder: CategoryKey, destination: CategoryKey, snapshot: &InventorySnapshot) -> bool {
    let mut current = Some(destination);
    let mut visited = HashSet::new();
    while let Some(key) = current {
        if key == folder || !visited.insert(key) {
            return true;
        }
        current = snapshot.category(key).and_then(|category| category.parent);
    }
    false
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ::inventory::{
        AssetType, Category, CategoryKey, FolderType, Inventory, InventoryLimits, InventoryNodeId, InventoryNodeKey, InventoryOwner,
        InventoryType, Item, ItemKey, PermissionMask, Permissions, SaleInfo, SaleType, ServerVersion,
    };
    use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
    use uuid::Uuid;

    use super::{EditIntent, validate_edit};
    use crate::inventory::InventoryEditRejection;

    struct Fixture {
        inventory: Inventory,
        owner: InventoryOwner,
        trash: CategoryKey,
        object: CategoryKey,
        clothing: CategoryKey,
        current_outfit: CategoryKey,
        current_outfit_child: CategoryKey,
        inbox: CategoryKey,
        trash_child: CategoryKey,
        object_item: ItemKey,
        current_outfit_item: ItemKey,
        trash_item: ItemKey,
    }

    fn key(owner: InventoryOwner, id: u128) -> CategoryKey {
        CategoryKey::new(owner, FolderId::new(Uuid::from_u128(id)))
    }

    fn category(key: CategoryKey, parent: Option<CategoryKey>, folder_type: FolderType) -> Category {
        Category {
            key,
            parent,
            preferred_folder_type: folder_type,
            name: Arc::from("folder"),
            server_version: ServerVersion::UNKNOWN,
            server_descendent_count: None,
        }
    }

    fn item(owner: InventoryOwner, parent: CategoryKey, id: u128) -> Item {
        Item {
            key: ItemKey::new(owner, ItemId::new(Uuid::from_u128(id))),
            parent,
            asset_id: AssetId::new(Uuid::nil()),
            asset_type: AssetType::Notecard,
            inventory_type: InventoryType::Notecard,
            flags: 0,
            name: Arc::from("item"),
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

    fn fixture() -> Fixture {
        let owner = InventoryOwner::new(Uuid::from_u128(1));
        let root = key(owner, 2);
        let trash = key(owner, 3);
        let object = key(owner, 4);
        let clothing = key(owner, 5);
        let current_outfit = key(owner, 6);
        let inbox = key(owner, 7);
        let trash_child = key(owner, 8);
        let current_outfit_child = key(owner, 12);
        let object_item = ItemKey::new(owner, ItemId::new(Uuid::from_u128(9)));
        let trash_item = ItemKey::new(owner, ItemId::new(Uuid::from_u128(10)));
        let current_outfit_item = ItemKey::new(owner, ItemId::new(Uuid::from_u128(11)));
        let mut inventory = Inventory::new(category(root, None, FolderType::RootInventory), None, InventoryLimits::default())
            .expect("valid inventory root");
        for entry in [
            category(trash, Some(root), FolderType::Trash),
            category(object, Some(root), FolderType::Object),
            category(clothing, Some(root), FolderType::Clothing),
            category(current_outfit, Some(root), FolderType::CurrentOutfit),
            category(inbox, Some(root), FolderType::Inbox),
            category(trash_child, Some(trash), FolderType::None),
            category(current_outfit_child, Some(current_outfit), FolderType::None),
        ] {
            inventory.upsert_category(entry).expect("valid category");
        }
        inventory.upsert_item(item(owner, object, object_item.id().uuid().as_u128())).expect("valid object item");
        inventory.upsert_item(item(owner, current_outfit, current_outfit_item.id().uuid().as_u128())).expect("valid Current Outfit item");
        inventory.upsert_item(item(owner, trash, trash_item.id().uuid().as_u128())).expect("valid Trash item");
        Fixture {
            inventory,
            owner,
            trash,
            object,
            clothing,
            current_outfit,
            current_outfit_child,
            inbox,
            trash_child,
            object_item,
            current_outfit_item,
            trash_item,
        }
    }

    fn category_node(key: CategoryKey) -> InventoryNodeKey {
        InventoryNodeKey::new(key.owner(), InventoryNodeId::Category(key.id()))
    }

    fn item_node(key: ItemKey) -> InventoryNodeKey {
        InventoryNodeKey::new(key.owner(), InventoryNodeId::Item(key.id()))
    }

    #[test]
    fn protected_contents_remain_editable() {
        let fixture = fixture();
        let snapshot = fixture.inventory.snapshot();
        assert_eq!(validate_edit(&snapshot, fixture.owner, EditIntent::Rename { node: item_node(fixture.object_item) }), Ok(()));
        assert_eq!(
            validate_edit(
                &snapshot,
                fixture.owner,
                EditIntent::Move { node: item_node(fixture.object_item), destination: Some(fixture.clothing) }
            ),
            Ok(())
        );
        assert_eq!(validate_edit(&snapshot, fixture.owner, EditIntent::Delete { node: item_node(fixture.object_item) }), Ok(()));
        assert_eq!(validate_edit(&snapshot, fixture.owner, EditIntent::CreateFolder { parent: fixture.object }), Ok(()));
        for intent in [
            EditIntent::Rename { node: category_node(fixture.object) },
            EditIntent::Move { node: category_node(fixture.object), destination: Some(fixture.clothing) },
            EditIntent::Delete { node: category_node(fixture.object) },
        ] {
            assert_eq!(validate_edit(&snapshot, fixture.owner, intent), Err(InventoryEditRejection::ProtectedFolder));
        }
    }

    #[test]
    fn trash_restore_rules_are_scoped() {
        let fixture = fixture();
        let snapshot = fixture.inventory.snapshot();
        assert_eq!(
            validate_edit(&snapshot, fixture.owner, EditIntent::Rename { node: item_node(fixture.trash_item) }),
            Err(InventoryEditRejection::OperationNotAllowed)
        );
        assert_eq!(
            validate_edit(
                &snapshot,
                fixture.owner,
                EditIntent::Move { node: item_node(fixture.trash_item), destination: Some(fixture.object) }
            ),
            Ok(())
        );
        assert_eq!(
            validate_edit(
                &snapshot,
                fixture.owner,
                EditIntent::Move { node: item_node(fixture.current_outfit_item), destination: Some(fixture.object) }
            ),
            Err(InventoryEditRejection::OperationNotAllowed)
        );
        assert_eq!(
            validate_edit(
                &snapshot,
                fixture.owner,
                EditIntent::Move { node: category_node(fixture.current_outfit_child), destination: Some(fixture.object) }
            ),
            Ok(())
        );
        assert_eq!(
            validate_edit(
                &snapshot,
                fixture.owner,
                EditIntent::Move { node: item_node(fixture.object_item), destination: Some(fixture.trash) }
            ),
            Ok(())
        );
        assert_eq!(
            validate_edit(&snapshot, fixture.owner, EditIntent::Delete { node: item_node(fixture.trash_item) }),
            Err(InventoryEditRejection::OperationNotAllowed)
        );
        assert_eq!(
            validate_edit(&snapshot, fixture.owner, EditIntent::Delete { node: item_node(fixture.current_outfit_item) }),
            Err(InventoryEditRejection::OperationNotAllowed)
        );
        assert_eq!(
            validate_edit(&snapshot, fixture.owner, EditIntent::Delete { node: category_node(fixture.current_outfit_child) }),
            Ok(())
        );
        assert_eq!(
            validate_edit(
                &snapshot,
                fixture.owner,
                EditIntent::Move { node: category_node(fixture.trash_child), destination: Some(fixture.object) }
            ),
            Ok(())
        );
        assert_eq!(
            validate_edit(&snapshot, fixture.owner, EditIntent::Rename { node: category_node(fixture.trash_child) }),
            Err(InventoryEditRejection::OperationNotAllowed)
        );
        assert_eq!(
            validate_edit(&snapshot, fixture.owner, EditIntent::Delete { node: category_node(fixture.trash_child) }),
            Err(InventoryEditRejection::OperationNotAllowed)
        );
        assert_eq!(
            validate_edit(&snapshot, fixture.owner, EditIntent::CreateFolder { parent: fixture.trash }),
            Err(InventoryEditRejection::OperationNotAllowed)
        );
        assert_eq!(
            validate_edit(
                &snapshot,
                fixture.owner,
                EditIntent::Move { node: item_node(fixture.object_item), destination: Some(fixture.trash) }
            ),
            Ok(())
        );
        assert_eq!(validate_edit(&snapshot, fixture.owner, EditIntent::EmptyTrash), Ok(()));
    }

    #[test]
    fn special_destinations_keep_their_rules() {
        let fixture = fixture();
        let snapshot = fixture.inventory.snapshot();
        assert_eq!(
            validate_edit(
                &snapshot,
                fixture.owner,
                EditIntent::Move { node: item_node(fixture.object_item), destination: Some(fixture.current_outfit) }
            ),
            Err(InventoryEditRejection::OperationNotAllowed)
        );
        assert_eq!(
            validate_edit(&snapshot, fixture.owner, EditIntent::CreateFolder { parent: fixture.current_outfit }),
            Err(InventoryEditRejection::OperationNotAllowed)
        );
        assert_eq!(
            validate_edit(&snapshot, fixture.owner, EditIntent::CreateFolder { parent: fixture.inbox }),
            Err(InventoryEditRejection::OperationNotAllowed)
        );
    }
}
