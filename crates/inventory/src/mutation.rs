use std::collections::HashSet;
use std::sync::Arc;

use utils::FolderId;

use crate::state::{
    InventoryState, insert_child, remove_category_record, remove_child, replace_category_record, validate_category, validate_item,
};
use crate::{
    Category, CategoryKey, ChangeSet, Inventory, InventoryChange, InventoryError, InventoryNodeId, InventoryNodeKey, InventoryOwner, Item,
    ItemKey, ServerVersion,
};

pub const MAX_FOLDER_FETCH_CHILDREN: usize = 100_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InventoryChild {
    Category(Category),
    Item(Item),
}

impl InventoryChild {
    fn node_key(&self) -> InventoryNodeKey {
        match self {
            Self::Category(category) => InventoryNodeKey::new(category.key.owner(), InventoryNodeId::Category(category.key.id())),
            Self::Item(item) => InventoryNodeKey::new(item.key.owner(), InventoryNodeId::Item(item.key.id())),
        }
    }

    fn parent(&self) -> CategoryKey {
        match self {
            Self::Category(category) => category.parent.unwrap_or(category.key),
            Self::Item(item) => item.parent,
        }
    }
}

impl Inventory {
    pub fn upsert_category(&mut self, category: Category) -> Result<ChangeSet, InventoryError> {
        validate_category(&category)?;
        let mut next = self.state.as_ref().clone();
        let mut changes = Vec::new();
        upsert_category_state(&mut next, category, &mut changes)?;
        self.publish(next, changes)
    }

    pub fn upsert_item(&mut self, item: Item) -> Result<ChangeSet, InventoryError> {
        validate_item(&item)?;
        let mut next = self.state.as_ref().clone();
        let mut changes = Vec::new();
        upsert_item_state(&mut next, item, &mut changes)?;
        self.publish(next, changes)
    }

    pub fn move_node(&mut self, key: InventoryNodeKey, parent: CategoryKey) -> Result<ChangeSet, InventoryError> {
        if key.owner() != parent.owner() {
            return Err(InventoryError::InvalidParent);
        }
        if !self.state.categories.contains_key(&parent) {
            return Err(InventoryError::MissingParent);
        }
        let mut next = self.state.as_ref().clone();
        let mut changes = Vec::new();
        match key.id() {
            InventoryNodeId::Category(id) => {
                let category_key = CategoryKey::new(key.owner(), id);
                if self.state.roots.contains(category_key) {
                    return Err(InventoryError::FixedRoot);
                }
                let mut category =
                    next.categories.get(&category_key).map(|category| category.as_ref().clone()).ok_or(InventoryError::MissingCategory)?;
                category.parent = Some(parent);
                upsert_category_state(&mut next, category, &mut changes)?;
            }
            InventoryNodeId::Item(id) => {
                let item_key = ItemKey::new(key.owner(), id);
                let mut item = next.items.get(&item_key).map(|item| item.as_ref().clone()).ok_or(InventoryError::MissingItem)?;
                item.parent = parent;
                upsert_item_state(&mut next, item, &mut changes)?;
            }
        }
        self.publish(next, changes)
    }

    pub fn remove_subtree(&mut self, owner: InventoryOwner, folder_id: FolderId) -> Result<ChangeSet, InventoryError> {
        let root = CategoryKey::new(owner, folder_id);
        if self.state.roots.contains(root) {
            return Err(InventoryError::FixedRoot);
        }
        if !self.state.categories.contains_key(&root) {
            return self.publish(self.state.as_ref().clone(), Vec::new());
        }
        let mut next = self.state.as_ref().clone();
        let mut changes = Vec::new();
        remove_category_subtree(&mut next, root, &mut changes)?;
        self.publish(next, changes)
    }

    pub fn apply_folder_fetch(
        &mut self,
        folder: CategoryKey,
        version: ServerVersion,
        descendent_count: Option<u32>,
        children: Vec<InventoryChild>,
    ) -> Result<ChangeSet, InventoryError> {
        if children.len() > MAX_FOLDER_FETCH_CHILDREN {
            return Err(InventoryError::TooManyFolderChildren);
        }
        if !self.state.categories.contains_key(&folder) {
            return Err(InventoryError::MissingCategory);
        }
        let mut incoming = HashSet::with_capacity(children.len());
        for child in &children {
            if child.node_key().owner() != folder.owner() || child.parent() != folder {
                return Err(InventoryError::InvalidParent);
            }
            if !incoming.insert(child.node_key()) {
                return Err(InventoryError::DuplicateFetchChild);
            }
            match child {
                InventoryChild::Category(category) => {
                    validate_category(category)?;
                    if self.state.roots.contains(category.key) {
                        return Err(InventoryError::FixedRoot);
                    }
                }
                InventoryChild::Item(item) => validate_item(item)?,
            }
        }

        let old_children = self.state.children.get(&folder).cloned().ok_or(InventoryError::InconsistentIndex)?;
        let mut next = self.state.as_ref().clone();
        let mut changes = Vec::new();
        for child in children {
            match child {
                InventoryChild::Category(category) => upsert_category_state(&mut next, category, &mut changes)?,
                InventoryChild::Item(item) => upsert_item_state(&mut next, item, &mut changes)?,
            }
        }
        for stale in old_children.iter().copied() {
            let key = InventoryNodeKey::new(folder.owner(), stale);
            if incoming.contains(&key) {
                continue;
            }
            match stale {
                InventoryNodeId::Category(id) => {
                    remove_category_subtree(&mut next, CategoryKey::new(folder.owner(), id), &mut changes)?;
                }
                InventoryNodeId::Item(id) => {
                    remove_item(&mut next, ItemKey::new(folder.owner(), id), &mut changes)?;
                }
            }
        }

        let mut updated_folder =
            next.categories.get(&folder).map(|category| category.as_ref().clone()).ok_or(InventoryError::InconsistentIndex)?;
        updated_folder.server_version = version;
        updated_folder.server_descendent_count = descendent_count;
        let previous = next.categories.get(&folder).ok_or(InventoryError::InconsistentIndex)?;
        if previous.as_ref() != &updated_folder {
            replace_category_record(&mut next, updated_folder)?;
            let updated_folder = next.categories.get(&folder).cloned().ok_or(InventoryError::InconsistentIndex)?;
            changes.push(InventoryChange::CategoryUpserted(updated_folder));
        }
        changes.push(InventoryChange::FolderFetchApplied { folder, child_count: incoming.len() });
        self.publish(next, changes)
    }

    pub fn set_version(&mut self, folder: CategoryKey, version: ServerVersion) -> Result<ChangeSet, InventoryError> {
        let Some(current) = self.state.categories.get(&folder) else {
            return Err(InventoryError::MissingCategory);
        };
        if current.server_version == version {
            return self.publish(self.state.as_ref().clone(), Vec::new());
        }
        let mut next = self.state.as_ref().clone();
        let mut updated = current.as_ref().clone();
        updated.server_version = version;
        replace_category_record(&mut next, updated)?;
        let updated = next.categories.get(&folder).cloned().ok_or(InventoryError::InconsistentIndex)?;
        self.publish(next, vec![InventoryChange::VersionSet { folder, version }, InventoryChange::CategoryUpserted(updated)])
    }
}

fn upsert_category_state(state: &mut InventoryState, category: Category, changes: &mut Vec<InventoryChange>) -> Result<(), InventoryError> {
    validate_category(&category)?;
    let key = category.key;
    if state.roots.contains(key) {
        if category.parent.is_some() {
            return Err(InventoryError::InvalidRoot);
        }
    } else {
        let parent = category.parent.ok_or(InventoryError::InvalidParent)?;
        if parent.owner() != key.owner() || !state.categories.contains_key(&parent) {
            return Err(InventoryError::MissingParent);
        }
        ensure_no_cycle(state, key, parent)?;
    }
    let previous = state.categories.get(&key).cloned();
    if previous.as_deref() == Some(&category) {
        return Ok(());
    }
    if previous.is_none() && state.categories.len() >= crate::MAX_INVENTORY_CATEGORIES {
        return Err(InventoryError::TooManyCategories);
    }
    let parent_changed = previous.as_ref().is_some_and(|previous| previous.parent != category.parent);
    if parent_changed && let Some(old_parent) = previous.as_ref().and_then(|previous| previous.parent) {
        remove_child(state, old_parent, InventoryNodeId::Category(key.id()))?;
    }
    if let Some(parent) = category.parent
        && (previous.is_none() || parent_changed)
    {
        insert_child(state, parent, InventoryNodeId::Category(key.id()))?;
        if let Some(old_parent) = previous.as_ref().and_then(|previous| previous.parent) {
            changes.push(InventoryChange::NodeMoved {
                node: InventoryNodeKey::new(key.owner(), InventoryNodeId::Category(key.id())),
                from: old_parent,
                to: parent,
            });
        }
    }
    replace_category_record(state, category)?;
    let category = state.categories.get(&key).cloned().ok_or(InventoryError::InconsistentIndex)?;
    changes.push(InventoryChange::CategoryUpserted(category));
    Ok(())
}

fn upsert_item_state(state: &mut InventoryState, item: Item, changes: &mut Vec<InventoryChange>) -> Result<(), InventoryError> {
    validate_item(&item)?;
    if !state.categories.contains_key(&item.parent) {
        return Err(InventoryError::MissingParent);
    }
    let key = item.key;
    let previous = state.items.get(&key).cloned();
    if previous.as_deref() == Some(&item) {
        return Ok(());
    }
    if previous.is_none() && state.items.len() >= crate::MAX_INVENTORY_ITEMS {
        return Err(InventoryError::TooManyItems);
    }
    if let Some(previous) = &previous
        && previous.parent != item.parent
    {
        remove_child(state, previous.parent, InventoryNodeId::Item(key.id()))?;
        changes.push(InventoryChange::NodeMoved {
            node: InventoryNodeKey::new(key.owner(), InventoryNodeId::Item(key.id())),
            from: previous.parent,
            to: item.parent,
        });
    }
    if previous.as_ref().is_none_or(|previous| previous.parent != item.parent) {
        insert_child(state, item.parent, InventoryNodeId::Item(key.id()))?;
    }
    let item = Arc::new(item);
    state.items.insert(key, Arc::clone(&item));
    changes.push(InventoryChange::ItemUpserted(item));
    Ok(())
}

fn ensure_no_cycle(state: &InventoryState, key: CategoryKey, mut parent: CategoryKey) -> Result<(), InventoryError> {
    let mut visited = HashSet::new();
    loop {
        if parent == key || !visited.insert(parent) {
            return Err(InventoryError::Cycle);
        }
        let category = state.categories.get(&parent).ok_or(InventoryError::MissingParent)?;
        match category.parent {
            Some(ancestor) => parent = ancestor,
            None => return Ok(()),
        }
    }
}

fn remove_item(state: &mut InventoryState, key: ItemKey, changes: &mut Vec<InventoryChange>) -> Result<(), InventoryError> {
    let Some(item) = state.items.get(&key).cloned() else { return Ok(()) };
    remove_child(state, item.parent, InventoryNodeId::Item(key.id()))?;
    state.items.remove(&key);
    changes.push(InventoryChange::ItemRemoved(key));
    Ok(())
}

fn remove_category_subtree(
    state: &mut InventoryState,
    root: CategoryKey,
    changes: &mut Vec<InventoryChange>,
) -> Result<(), InventoryError> {
    if state.roots.contains(root) {
        return Err(InventoryError::FixedRoot);
    }
    let Some(category) = state.categories.get(&root) else { return Ok(()) };
    category.parent.ok_or(InventoryError::InvalidRoot)?;
    let mut category_order = Vec::new();
    let mut item_keys = Vec::new();
    let mut pending = vec![InventoryNodeId::Category(root.id())];
    let mut visited = HashSet::new();
    while let Some(node) = pending.pop() {
        if !visited.insert(node) {
            return Err(InventoryError::Cycle);
        }
        match node {
            InventoryNodeId::Category(id) => {
                let key = CategoryKey::new(root.owner(), id);
                if !state.categories.contains_key(&key) {
                    return Err(InventoryError::InconsistentIndex);
                }
                category_order.push(key);
                let children = state.children.get(&key).ok_or(InventoryError::InconsistentIndex)?;
                pending.extend(children.iter().copied());
            }
            InventoryNodeId::Item(id) => {
                let key = ItemKey::new(root.owner(), id);
                if !state.items.contains_key(&key) {
                    return Err(InventoryError::InconsistentIndex);
                }
                item_keys.push(key);
            }
        }
    }
    for key in item_keys {
        remove_item(state, key, changes)?;
    }
    for key in category_order.into_iter().rev() {
        let category = state.categories.get(&key).cloned().ok_or(InventoryError::InconsistentIndex)?;
        if let Some(parent) = category.parent {
            remove_child(state, parent, InventoryNodeId::Category(key.id()))?;
        }
        remove_category_record(state, key)?;
        changes.push(InventoryChange::CategoryRemoved(key));
    }
    Ok(())
}
