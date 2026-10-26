use std::collections::HashSet;
use std::sync::Arc;

use utils::FolderId;

use crate::state::{
    InventoryState, insert_child, remove_category_record, remove_child, replace_category_record, validate_category, validate_item,
};
use crate::{
    Category, CategoryKey, ChangeSet, Inventory, InventoryChange, InventoryError, InventoryLimits, InventoryNodeId, InventoryNodeKey,
    InventoryOwner, Item, ItemKey, ServerVersion,
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
        if next.categories.len() > self.limits.max_categories {
            return Err(InventoryError::TooManyCategories);
        }
        self.publish(next, changes)
    }

    pub fn upsert_item(&mut self, item: Item) -> Result<ChangeSet, InventoryError> {
        validate_item(&item)?;
        let mut next = self.state.as_ref().clone();
        let mut changes = Vec::new();
        upsert_item_state(&mut next, item, &mut changes)?;
        if next.items.len() > self.limits.max_items {
            return Err(InventoryError::TooManyItems);
        }
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
        if children.len() > self.limits.max_folder_fetch_children {
            return Err(InventoryError::TooManyFolderChildren);
        }
        if !self.state.categories.contains_key(&folder) {
            return Err(InventoryError::MissingCategory);
        }
        let mut incoming = HashSet::with_capacity(children.len());
        let mut new_categories = 0usize;
        let mut new_items = 0usize;
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
            match child {
                InventoryChild::Category(category) if !self.state.categories.contains_key(&category.key) => {
                    new_categories = new_categories.checked_add(1).ok_or(InventoryError::TooManyCategories)?;
                }
                InventoryChild::Item(item) if !self.state.items.contains_key(&item.key) => {
                    new_items = new_items.checked_add(1).ok_or(InventoryError::TooManyItems)?;
                }
                _ => {}
            }
        }

        let old_children = self.state.children.get(&folder).cloned().ok_or(InventoryError::InconsistentIndex)?;
        let mut removal = SubtreeRemoval::default();
        for stale in old_children.iter().copied() {
            let key = InventoryNodeKey::new(folder.owner(), stale);
            if incoming.contains(&key) {
                continue;
            }
            match stale {
                InventoryNodeId::Category(id) => {
                    removal.collect_category(&self.state, CategoryKey::new(folder.owner(), id), &incoming)?;
                }
                InventoryNodeId::Item(id) => {
                    removal.collect_item(&self.state, ItemKey::new(folder.owner(), id), folder, &incoming)?;
                }
            }
        }
        check_final_capacity(&self.state, &removal, new_categories, new_items, self.limits)?;

        let mut next = self.state.as_ref().clone();
        let mut changes = Vec::new();
        let preserved = std::mem::take(&mut removal.preserved);
        if !preserved.is_empty() {
            for child in &children {
                if !preserved.contains(&child.node_key()) {
                    continue;
                }
                match child {
                    InventoryChild::Category(category) => upsert_category_state(&mut next, category.clone(), &mut changes)?,
                    InventoryChild::Item(item) => upsert_item_state(&mut next, item.clone(), &mut changes)?,
                }
            }
        }
        removal.apply(&mut next, &mut changes)?;
        for child in children {
            if preserved.contains(&child.node_key()) {
                continue;
            }
            match child {
                InventoryChild::Category(category) => upsert_category_state(&mut next, category, &mut changes)?,
                InventoryChild::Item(item) => upsert_item_state(&mut next, item, &mut changes)?,
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
    let mut removal = SubtreeRemoval::default();
    removal.collect_category(state, root, &HashSet::new())?;
    removal.apply(state, changes)
}

#[derive(Default)]
struct SubtreeRemoval {
    categories: Vec<CategoryKey>,
    items: Vec<ItemKey>,
    visited: HashSet<InventoryNodeId>,
    preserved: HashSet<InventoryNodeKey>,
}

impl SubtreeRemoval {
    fn collect_category(
        &mut self,
        state: &InventoryState,
        root: CategoryKey,
        preserved: &HashSet<InventoryNodeKey>,
    ) -> Result<(), InventoryError> {
        if state.roots.contains(root) {
            return Err(InventoryError::FixedRoot);
        }
        let category = state.categories.get(&root).ok_or(InventoryError::InconsistentIndex)?;
        let parent = category.parent.ok_or(InventoryError::InvalidRoot)?;
        let mut pending = vec![(InventoryNodeId::Category(root.id()), parent)];
        while let Some((node, expected_parent)) = pending.pop() {
            if !self.visited.insert(node) {
                return Err(InventoryError::Cycle);
            }
            let node_key = InventoryNodeKey::new(root.owner(), node);
            if preserved.contains(&node_key) {
                self.preserved.insert(node_key);
                continue;
            }
            match node {
                InventoryNodeId::Category(id) => {
                    let key = CategoryKey::new(root.owner(), id);
                    let category = state.categories.get(&key).ok_or(InventoryError::InconsistentIndex)?;
                    if category.parent != Some(expected_parent) {
                        return Err(InventoryError::InconsistentIndex);
                    }
                    self.categories.push(key);
                    let children = state.children.get(&key).ok_or(InventoryError::InconsistentIndex)?;
                    pending.extend(children.iter().map(|child| (*child, key)));
                }
                InventoryNodeId::Item(id) => {
                    let key = ItemKey::new(root.owner(), id);
                    let item = state.items.get(&key).ok_or(InventoryError::InconsistentIndex)?;
                    if item.parent != expected_parent {
                        return Err(InventoryError::InconsistentIndex);
                    }
                    self.items.push(key);
                }
            }
        }
        Ok(())
    }

    fn collect_item(
        &mut self,
        state: &InventoryState,
        key: ItemKey,
        parent: CategoryKey,
        preserved: &HashSet<InventoryNodeKey>,
    ) -> Result<(), InventoryError> {
        let node = InventoryNodeId::Item(key.id());
        if !self.visited.insert(node) {
            return Err(InventoryError::Cycle);
        }
        let node_key = InventoryNodeKey::new(key.owner(), node);
        if preserved.contains(&node_key) {
            self.preserved.insert(node_key);
            return Ok(());
        }
        let item = state.items.get(&key).ok_or(InventoryError::InconsistentIndex)?;
        if item.parent != parent {
            return Err(InventoryError::InconsistentIndex);
        }
        self.items.push(key);
        Ok(())
    }

    fn apply(self, state: &mut InventoryState, changes: &mut Vec<InventoryChange>) -> Result<(), InventoryError> {
        for key in self.items {
            remove_item(state, key, changes)?;
        }
        for key in self.categories.into_iter().rev() {
            let category = state.categories.get(&key).cloned().ok_or(InventoryError::InconsistentIndex)?;
            if let Some(parent) = category.parent {
                remove_child(state, parent, InventoryNodeId::Category(key.id()))?;
            }
            remove_category_record(state, key)?;
            changes.push(InventoryChange::CategoryRemoved(key));
        }
        Ok(())
    }
}

fn check_final_capacity(
    state: &InventoryState,
    removal: &SubtreeRemoval,
    new_categories: usize,
    new_items: usize,
    limits: InventoryLimits,
) -> Result<(), InventoryError> {
    let categories_after_removal = state.categories.len().checked_sub(removal.categories.len()).ok_or(InventoryError::InconsistentIndex)?;
    let items_after_removal = state.items.len().checked_sub(removal.items.len()).ok_or(InventoryError::InconsistentIndex)?;
    let category_count = categories_after_removal.checked_add(new_categories).ok_or(InventoryError::TooManyCategories)?;
    let item_count = items_after_removal.checked_add(new_items).ok_or(InventoryError::TooManyItems)?;
    if category_count > limits.max_categories {
        return Err(InventoryError::TooManyCategories);
    }
    if item_count > limits.max_items {
        return Err(InventoryError::TooManyItems);
    }
    Ok(())
}
