use std::collections::{HashMap, HashSet};

use ::inventory::{Category, CategoryKey, FolderType, InventoryDelta, InventoryOwner, InventorySnapshot, Item, ItemKey, ServerVersion};
use protocol::inventory::{AisEditResponse, Category as WireCategory, Child, DecodeError, Item as WireItem};
use utils::{FolderId, ItemId};
use uuid::Uuid;

use crate::inventory::fetch::{category_from_wire, item_from_wire};

use super::types::EditOperation;

pub(crate) struct AisDelta {
    pub(super) delta: InventoryDelta,
    pub(super) created: Option<CategoryKey>,
    confirmed_name: Option<ConfirmedName>,
}

enum ConfirmedName {
    Category { key: CategoryKey, name: String },
    Item { key: ItemKey, name: String },
}

impl AisDelta {
    pub(crate) fn into_delta(mut self, current: &InventorySnapshot) -> InventoryDelta {
        match self.confirmed_name {
            Some(ConfirmedName::Category { key, name }) => {
                if let Some(category) = current.category(key) {
                    let mut category = category.clone();
                    category.name = name.into();
                    self.delta.updated_categories.push(category);
                }
            }
            Some(ConfirmedName::Item { key, name }) => {
                if let Some(item) = current.item(key) {
                    let mut item = item.clone();
                    item.name = name.into();
                    self.delta.updated_items.push(item);
                }
            }
            None => {}
        }
        self.delta
    }
}

pub(super) fn delta_from_ais_response(
    response: &AisEditResponse,
    operation: &EditOperation,
    snapshot: &InventorySnapshot,
    owner: InventoryOwner,
) -> Result<AisDelta, DecodeError> {
    let created_categories = response.created_categories.iter().copied().collect::<HashSet<_>>();
    let updated_categories = response.updated_categories.iter().copied().collect::<HashSet<_>>();
    let created_items = response.created_items.iter().copied().collect::<HashSet<_>>();
    let updated_items = response.updated_items.iter().copied().collect::<HashSet<_>>();
    if created_categories.iter().any(|id| updated_categories.contains(id)) || created_items.iter().any(|id| updated_items.contains(id)) {
        return Err(DecodeError::InvalidResponse);
    }

    let created_category = match operation {
        EditOperation::CreateFolder { .. } => {
            if response.created_categories.len() > 1 {
                return Err(DecodeError::InvalidResponse);
            }
            let metadata_id = response.created_categories.first().copied();
            let category_id = response.category.as_ref().map(|category| category.id);
            if metadata_id.is_some() && category_id.is_some() && metadata_id != category_id {
                return Err(DecodeError::InvalidResponse);
            }
            metadata_id.or(category_id)
        }
        _ => None,
    };

    let mut categories = HashMap::new();
    let mut category_order = Vec::new();
    let mut items = HashMap::new();
    let mut item_order = Vec::new();
    if let Some(category) = &response.category {
        append_category(
            category,
            root_category_parent(category, operation, snapshot, owner),
            owner,
            &mut categories,
            &mut category_order,
            &mut items,
            &mut item_order,
        )?;
    }
    if let Some(item) = &response.item {
        append_item(item, root_item_parent(item, operation, snapshot, owner), owner, &mut items, &mut item_order)?;
    }

    let mut delta = InventoryDelta::default();
    for id in category_order {
        let category = categories.remove(&id).ok_or(DecodeError::InvalidResponse)?;
        if created_categories.contains(&category.key.id().uuid()) || snapshot.category(category.key).is_none() {
            delta.created_categories.push(category);
        } else {
            delta.updated_categories.push(category);
        }
    }
    for id in item_order {
        let item = items.remove(&id).ok_or(DecodeError::InvalidResponse)?;
        if created_items.contains(&item.key.id().uuid()) || snapshot.item(item.key).is_none() {
            delta.created_items.push(item);
        } else {
            delta.updated_items.push(item);
        }
    }

    let mut confirmed_name = None;
    let created = match operation {
        EditOperation::CreateFolder { parent, name, preferred_type } => {
            let created_id = created_category.filter(|id| !id.is_nil()).ok_or(DecodeError::InvalidResponse)?;
            let key = CategoryKey::new(owner, FolderId::new(created_id));
            if let Some(category) = delta.created_categories.iter_mut().find(|category| category.key == key) {
                if category.parent != Some(*parent) || category.name.as_ref() != name {
                    return Err(DecodeError::InvalidResponse);
                }
            } else {
                delta.created_categories.push(Category {
                    key,
                    parent: Some(*parent),
                    preferred_folder_type: *preferred_type,
                    name: name.clone().into(),
                    server_version: ServerVersion::UNKNOWN,
                    server_descendent_count: None,
                });
            }
            Some(key)
        }
        EditOperation::RenameCategory { category, name } => {
            if created_categories.contains(&category.id().uuid()) {
                return Err(DecodeError::InvalidResponse);
            }
            if !response.updated_categories.contains(&category.id().uuid())
                && response.category.as_ref().is_none_or(|entry| entry.id != category.id().uuid())
            {
                return Err(DecodeError::InvalidResponse);
            }
            delta.created_categories.retain(|entry| entry.key != *category);
            delta.updated_categories.retain(|entry| entry.key != *category);
            confirmed_name = Some(ConfirmedName::Category { key: *category, name: name.clone() });
            None
        }
        EditOperation::RenameItem { item, name } => {
            if created_items.contains(&item.id().uuid()) {
                return Err(DecodeError::InvalidResponse);
            }
            if !response.updated_items.contains(&item.id().uuid())
                && response.item.as_ref().is_none_or(|entry| entry.id != item.id().uuid())
            {
                return Err(DecodeError::InvalidResponse);
            }
            delta.created_items.retain(|entry| entry.key != *item);
            delta.updated_items.retain(|entry| entry.key != *item);
            confirmed_name = Some(ConfirmedName::Item { key: *item, name: name.clone() });
            None
        }
        EditOperation::MoveItem { .. }
        | EditOperation::MoveFolder { .. }
        | EditOperation::DeleteItem(_)
        | EditOperation::DeleteFolder(_)
        | EditOperation::EmptyTrash => None,
    };

    if response.created_categories.iter().any(|id| {
        !delta.created_categories.iter().any(|category| category.key.id().uuid() == *id)
            && !delta.updated_categories.iter().any(|category| category.key.id().uuid() == *id)
    }) || response.created_items.iter().any(|id| {
        !delta.created_items.iter().any(|item| item.key.id().uuid() == *id)
            && !delta.updated_items.iter().any(|item| item.key.id().uuid() == *id)
    }) {
        return Err(DecodeError::InvalidResponse);
    }

    for id in &response.updated_categories {
        let key = CategoryKey::new(owner, FolderId::new(*id));
        if matches!(&confirmed_name, Some(ConfirmedName::Category { key: target, .. }) if *target == key) {
            continue;
        }
        if !delta.created_categories.iter().any(|category| category.key == key)
            && !delta.updated_categories.iter().any(|category| category.key == key)
        {
            let category = snapshot.category(key).ok_or(DecodeError::InvalidResponse)?;
            delta.updated_categories.push(category.clone());
        }
    }
    for id in &response.updated_items {
        let key = ItemKey::new(owner, ItemId::new(*id));
        if matches!(&confirmed_name, Some(ConfirmedName::Item { key: target, .. }) if *target == key) {
            continue;
        }
        if !delta.created_items.iter().any(|item| item.key == key) && !delta.updated_items.iter().any(|item| item.key == key) {
            let item = snapshot.item(key).ok_or(DecodeError::InvalidResponse)?;
            delta.updated_items.push(item.clone());
        }
    }

    let mut removed_items = HashSet::new();
    removed_items.extend(response.removed_items.iter().copied());
    removed_items.extend(response.category_items_removed.iter().copied());
    removed_items.extend(response.broken_links_removed.iter().copied());
    delta.removed_items.extend(removed_items.into_iter().map(|id| ItemKey::new(owner, ItemId::new(id))));
    delta.removed_categories.extend(response.categories_removed.iter().copied().map(|id| CategoryKey::new(owner, FolderId::new(id))));
    delta.updated_category_versions.extend(
        response
            .updated_category_versions
            .iter()
            .map(|(id, version)| (CategoryKey::new(owner, FolderId::new(*id)), ServerVersion::from_raw(*version))),
    );

    Ok(AisDelta { delta, created, confirmed_name })
}

fn append_category(
    wire: &WireCategory,
    parent: Option<CategoryKey>,
    owner: InventoryOwner,
    categories: &mut HashMap<Uuid, Category>,
    category_order: &mut Vec<Uuid>,
    items: &mut HashMap<Uuid, Item>,
    item_order: &mut Vec<Uuid>,
) -> Result<(), DecodeError> {
    let category = category_from_wire(wire, owner, parent).map_err(|_| DecodeError::InvalidResponse)?;
    let key = category.key;
    if insert_unique(categories, wire.id, category)? {
        category_order.push(wire.id);
    }
    for child in wire.children.iter().flatten() {
        match child {
            Child::Category(child) => append_category(child, Some(key), owner, categories, category_order, items, item_order)?,
            Child::Item(child) => append_item(child, Some(key), owner, items, item_order)?,
        }
    }
    Ok(())
}

fn append_item(
    wire: &WireItem,
    parent: Option<CategoryKey>,
    owner: InventoryOwner,
    items: &mut HashMap<Uuid, Item>,
    item_order: &mut Vec<Uuid>,
) -> Result<(), DecodeError> {
    let parent =
        parent.or_else(|| wire.parent_id.map(|id| CategoryKey::new(owner, FolderId::new(id)))).ok_or(DecodeError::InvalidResponse)?;
    let item = item_from_wire(wire, owner, parent).map_err(|_| DecodeError::InvalidResponse)?;
    if insert_unique(items, wire.id, item)? {
        item_order.push(wire.id);
    }
    Ok(())
}

fn insert_unique<T: PartialEq>(values: &mut HashMap<Uuid, T>, id: Uuid, value: T) -> Result<bool, DecodeError> {
    match values.entry(id) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(value);
            Ok(true)
        }
        std::collections::hash_map::Entry::Occupied(entry) if entry.get() == &value => Ok(false),
        std::collections::hash_map::Entry::Occupied(_) => Err(DecodeError::InvalidResponse),
    }
}

fn root_category_parent(
    category: &WireCategory,
    operation: &EditOperation,
    snapshot: &InventorySnapshot,
    owner: InventoryOwner,
) -> Option<CategoryKey> {
    match operation {
        EditOperation::CreateFolder { parent, .. } => Some(*parent),
        EditOperation::RenameCategory { category: key, .. } if category.id == key.id().uuid() => snapshot.category(*key)?.parent,
        _ => category_parent_key(category, owner),
    }
}

fn root_item_parent(
    item: &WireItem,
    operation: &EditOperation,
    snapshot: &InventorySnapshot,
    owner: InventoryOwner,
) -> Option<CategoryKey> {
    match operation {
        EditOperation::RenameItem { item: key, .. } if item.id == key.id().uuid() => snapshot.item(*key).map(|item| item.parent),
        _ => item.parent_id.map(|id| CategoryKey::new(owner, FolderId::new(id))),
    }
}

fn category_parent_key(category: &WireCategory, owner: InventoryOwner) -> Option<CategoryKey> {
    category.parent_id.map(|id| CategoryKey::new(owner, FolderId::new(id)))
}

pub(super) fn trash_folder(snapshot: &InventorySnapshot, owner: InventoryOwner) -> Option<CategoryKey> {
    snapshot.category_with_type(owner, FolderType::Trash).map(|category| category.key)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use ::inventory::{
        AssetType, Inventory, InventoryLimits, InventoryNodeId, InventoryNodeKey, InventoryType, PermissionMask, Permissions, SaleInfo,
        SaleType,
    };
    use utils::{AssetId, AvatarId, GroupId};

    use super::*;

    fn key(owner: InventoryOwner, id: u128) -> CategoryKey {
        CategoryKey::new(owner, FolderId::new(Uuid::from_u128(id)))
    }

    fn category(key: CategoryKey, parent: Option<CategoryKey>, name: &str) -> Category {
        Category {
            key,
            parent,
            preferred_folder_type: FolderType::None,
            name: Arc::from(name),
            server_version: ServerVersion::UNKNOWN,
            server_descendent_count: None,
        }
    }

    fn wire_category(id: Uuid, owner: Uuid, parent: Option<Uuid>, name: &str, children: Vec<Child>) -> WireCategory {
        WireCategory {
            id,
            owner_id: Some(owner),
            parent_id: parent,
            name: name.to_owned(),
            version: Some(1),
            descendent_count: Some(children.len() as u32),
            preferred_folder_type: Some(-1),
            children: Some(children),
        }
    }

    #[test]
    fn ais_delta_keeps_nested_parent_order_and_combines_removal_metadata() {
        let owner = InventoryOwner::new(Uuid::from_u128(1));
        let root = key(owner, 2);
        let folder = key(owner, 3);
        let removed_folder = key(owner, 4);
        let mut inventory = Inventory::new(category(root, None, "root"), None, InventoryLimits::default()).expect("valid root");
        inventory.upsert_category(category(folder, Some(root), "before")).expect("add folder");
        inventory.upsert_category(category(removed_folder, Some(root), "removed")).expect("add removable folder");

        let child_id = Uuid::from_u128(5);
        let grandchild_id = Uuid::from_u128(6);
        let grandchild = wire_category(grandchild_id, owner.id(), Some(child_id), "grandchild", Vec::new());
        let child = wire_category(child_id, owner.id(), Some(folder.id().uuid()), "child", vec![Child::Category(grandchild)]);
        let response = AisEditResponse {
            category: Some(wire_category(folder.id().uuid(), owner.id(), Some(root.id().uuid()), "after", vec![Child::Category(child)])),
            item: None,
            created_categories: vec![child_id, grandchild_id],
            created_items: Vec::new(),
            updated_categories: vec![folder.id().uuid()],
            updated_items: Vec::new(),
            removed_items: vec![Uuid::from_u128(7)],
            categories_removed: vec![removed_folder.id().uuid()],
            category_items_removed: vec![Uuid::from_u128(8)],
            broken_links_removed: vec![Uuid::from_u128(9)],
            updated_category_versions: BTreeMap::from([(root.id().uuid(), 12)]),
        };
        let operation = EditOperation::RenameCategory { category: folder, name: "after".to_owned() };

        let converted = delta_from_ais_response(&response, &operation, &inventory.snapshot(), owner).expect("convert AIS response");

        assert_eq!(
            converted.delta.created_categories.iter().map(|category| category.key.id().uuid()).collect::<Vec<_>>(),
            vec![child_id, grandchild_id]
        );
        assert_eq!(converted.delta.removed_items.len(), 3);
        assert_eq!(converted.delta.removed_categories, vec![removed_folder]);
        assert_eq!(converted.delta.updated_category_versions, vec![(root, ServerVersion::from_raw(12))]);
        let current = inventory.snapshot();
        inventory.apply_delta(converted.into_delta(&current)).expect("apply converted confirmation");

        let snapshot = inventory.snapshot();
        assert_eq!(snapshot.category(folder).map(|category| category.name.as_ref()), Some("after"));
        assert!(snapshot.category(key(owner, 5)).is_some());
        assert!(snapshot.category(key(owner, 6)).is_some());
        assert!(snapshot.category(removed_folder).is_none());
    }

    #[test]
    fn rename_confirmation_changes_only_the_current_record_name() {
        let owner = InventoryOwner::new(Uuid::from_u128(11));
        let root = key(owner, 12);
        let first = key(owner, 13);
        let second = key(owner, 14);
        let folder = key(owner, 15);
        let item = ItemKey::new(owner, ItemId::new(Uuid::from_u128(16)));
        let mut inventory = Inventory::new(category(root, None, "root"), None, InventoryLimits::default()).expect("valid root");
        inventory.upsert_category(category(first, Some(root), "first")).expect("add first folder");
        inventory.upsert_category(category(second, Some(root), "second")).expect("add second folder");
        inventory.upsert_category(category(folder, Some(first), "before folder")).expect("add renamed folder");
        inventory
            .upsert_item(Item {
                key: item,
                parent: first,
                asset_id: AssetId::new(Uuid::from_u128(17)),
                asset_type: AssetType::Notecard,
                inventory_type: InventoryType::Notecard,
                flags: 0,
                name: Arc::from("before item"),
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
            })
            .expect("add renamed item");

        let initial = inventory.snapshot();
        let category_operation = EditOperation::RenameCategory { category: folder, name: "after folder".to_owned() };
        let category_response = AisEditResponse {
            category: None,
            item: None,
            created_categories: Vec::new(),
            created_items: Vec::new(),
            updated_categories: vec![folder.id().uuid()],
            updated_items: Vec::new(),
            removed_items: Vec::new(),
            categories_removed: Vec::new(),
            category_items_removed: Vec::new(),
            broken_links_removed: Vec::new(),
            updated_category_versions: BTreeMap::new(),
        };
        let category_confirmation =
            delta_from_ais_response(&category_response, &category_operation, &initial, owner).expect("convert category rename metadata");
        inventory
            .move_node(InventoryNodeKey::new(owner, InventoryNodeId::Category(folder.id())), second)
            .expect("apply newer category move");
        let category_delta = category_confirmation.into_delta(&inventory.snapshot());
        inventory.apply_delta(category_delta).expect("apply category name confirmation");

        let item_operation = EditOperation::RenameItem { item, name: "after item".to_owned() };
        let item_response = AisEditResponse {
            category: None,
            item: None,
            created_categories: Vec::new(),
            created_items: Vec::new(),
            updated_categories: Vec::new(),
            updated_items: vec![item.id().uuid()],
            removed_items: Vec::new(),
            categories_removed: Vec::new(),
            category_items_removed: Vec::new(),
            broken_links_removed: Vec::new(),
            updated_category_versions: BTreeMap::new(),
        };
        let item_confirmation =
            delta_from_ais_response(&item_response, &item_operation, &initial, owner).expect("convert item rename metadata");
        inventory.move_node(InventoryNodeKey::new(owner, InventoryNodeId::Item(item.id())), second).expect("apply newer item move");
        let item_delta = item_confirmation.into_delta(&inventory.snapshot());
        inventory.apply_delta(item_delta).expect("apply item name confirmation");

        let snapshot = inventory.snapshot();
        assert_eq!(snapshot.category(folder).map(|entry| (entry.parent, entry.name.as_ref())), Some((Some(second), "after folder")));
        assert_eq!(snapshot.item(item).map(|entry| (entry.parent, entry.name.as_ref())), Some((second, "after item")));

        let missing_confirmation =
            delta_from_ais_response(&item_response, &item_operation, &snapshot, owner).expect("convert another item rename confirmation");
        inventory.apply_delta(InventoryDelta { removed_items: vec![item], ..InventoryDelta::default() }).expect("apply newer item removal");
        let current = inventory.snapshot();
        inventory.apply_delta(missing_confirmation.into_delta(&current)).expect("apply stale name confirmation");
        assert!(inventory.snapshot().item(item).is_none());
    }
}
