use std::collections::{HashMap, HashSet};

use ::inventory::{Category, CategoryKey, FolderType, InventoryDelta, InventoryOwner, InventorySnapshot, Item, ItemKey, ServerVersion};
use protocol::inventory::{AisEditResponse, Category as WireCategory, Child, DecodeError, Item as WireItem};
use utils::{FolderId, ItemId};
use uuid::Uuid;

use super::{
    super::fetch::{category_from_wire, item_from_wire},
    types::EditOperation,
};

pub(super) struct AisDelta {
    pub(super) delta: InventoryDelta,
    pub(super) created: Option<CategoryKey>,
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
            if !response.updated_categories.contains(&category.id().uuid())
                && response.category.as_ref().is_none_or(|entry| entry.id != category.id().uuid())
            {
                return Err(DecodeError::InvalidResponse);
            }
            let current = snapshot.category(*category).ok_or(DecodeError::InvalidResponse)?;
            if let Some(updated) = delta.updated_categories.iter_mut().find(|entry| entry.key == *category) {
                updated.name = name.clone().into();
            } else if let Some(updated) = delta.created_categories.iter_mut().find(|entry| entry.key == *category) {
                updated.name = name.clone().into();
            } else {
                let mut updated = current.clone();
                updated.name = name.clone().into();
                delta.updated_categories.push(updated);
            }
            None
        }
        EditOperation::RenameItem { item, name } => {
            if !response.updated_items.contains(&item.id().uuid())
                && response.item.as_ref().is_none_or(|entry| entry.id != item.id().uuid())
            {
                return Err(DecodeError::InvalidResponse);
            }
            let current = snapshot.item(*item).ok_or(DecodeError::InvalidResponse)?;
            if let Some(updated) = delta.updated_items.iter_mut().find(|entry| entry.key == *item) {
                updated.name = name.clone().into();
            } else {
                let mut updated = current.clone();
                updated.name = name.clone().into();
                delta.updated_items.push(updated);
            }
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
        if !delta.created_categories.iter().any(|category| category.key == key)
            && !delta.updated_categories.iter().any(|category| category.key == key)
        {
            let category = snapshot.category(key).ok_or(DecodeError::InvalidResponse)?;
            delta.updated_categories.push(category.clone());
        }
    }
    for id in &response.updated_items {
        let key = ItemKey::new(owner, ItemId::new(*id));
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

    Ok(AisDelta { delta, created })
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

    use ::inventory::{Inventory, InventoryLimits};

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
        inventory.apply_delta(converted.delta).expect("apply converted confirmation");

        let snapshot = inventory.snapshot();
        assert_eq!(snapshot.category(folder).map(|category| category.name.as_ref()), Some("after"));
        assert!(snapshot.category(key(owner, 5)).is_some());
        assert!(snapshot.category(key(owner, 6)).is_some());
        assert!(snapshot.category(removed_folder).is_none());
    }
}
