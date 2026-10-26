use std::sync::Arc;

use crate::{
    Category, CategoryKey, FolderType, InventoryError, InventoryNodeId, InventoryOwner, InventoryRevision, Item, ItemKey,
    MAX_INVENTORY_CATEGORIES, MAX_INVENTORY_DESCRIPTION_BYTES, MAX_INVENTORY_NAME_BYTES,
};
use imbl::{HashMap, OrdSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InventoryRoots {
    personal: CategoryKey,
    library: Option<CategoryKey>,
}

impl InventoryRoots {
    pub const fn personal(self) -> CategoryKey {
        self.personal
    }

    pub const fn library(self) -> Option<CategoryKey> {
        self.library
    }

    pub fn contains(self, key: CategoryKey) -> bool {
        self.personal == key || self.library == Some(key)
    }
}

#[derive(Clone)]
pub(crate) struct InventoryState {
    pub(crate) revision: InventoryRevision,
    pub(crate) roots: InventoryRoots,
    pub(crate) categories: HashMap<CategoryKey, Arc<Category>>,
    pub(crate) items: HashMap<ItemKey, Arc<Item>>,
    pub(crate) children: HashMap<CategoryKey, OrdSet<InventoryNodeId>>,
    pub(crate) incomplete_categories: HashMap<InventoryOwner, usize>,
    pub(crate) categories_by_type: HashMap<(InventoryOwner, FolderType), OrdSet<CategoryKey>>,
    pub(crate) preferred_category_by_type: HashMap<(InventoryOwner, FolderType), CategoryKey>,
}

pub struct Inventory {
    pub(crate) state: Arc<InventoryState>,
}

impl Inventory {
    pub fn new(personal_root: Category, library_root: Option<Category>) -> Result<Self, InventoryError> {
        validate_category(&personal_root)?;
        if personal_root.parent.is_some() {
            return Err(InventoryError::InvalidRoot);
        }
        if let Some(root) = &library_root {
            validate_category(root)?;
            if root.parent.is_some() {
                return Err(InventoryError::InvalidRoot);
            }
            if root.key == personal_root.key {
                return Err(InventoryError::DuplicateRoots);
            }
            if root.key.owner() == personal_root.key.owner() {
                return Err(InventoryError::SharedRootOwner);
            }
        }
        let roots = InventoryRoots { personal: personal_root.key, library: library_root.as_ref().map(|root| root.key) };
        let mut state = InventoryState {
            revision: InventoryRevision(0),
            roots,
            categories: HashMap::new(),
            items: HashMap::new(),
            children: HashMap::new(),
            incomplete_categories: HashMap::new(),
            categories_by_type: HashMap::new(),
            preferred_category_by_type: HashMap::new(),
        };
        insert_category_record(&mut state, personal_root)?;
        if let Some(root) = library_root {
            insert_category_record(&mut state, root)?;
        }
        Ok(Self { state: Arc::new(state) })
    }

    pub fn from_categories(
        personal_root: Category,
        library_root: Option<Category>,
        categories: Vec<Category>,
    ) -> Result<Self, InventoryError> {
        let mut inventory = Self::new(personal_root, library_root)?;
        inventory.install_categories(categories)?;
        Ok(inventory)
    }

    pub fn revision(&self) -> InventoryRevision {
        self.state.revision
    }

    pub fn roots(&self) -> InventoryRoots {
        self.state.roots
    }

    pub fn snapshot(&self) -> crate::InventorySnapshot {
        crate::InventorySnapshot { state: Arc::clone(&self.state) }
    }

    pub(crate) fn publish(
        &mut self,
        mut next: InventoryState,
        changes: Vec<crate::InventoryChange>,
    ) -> Result<crate::ChangeSet, InventoryError> {
        if changes.is_empty() {
            return Ok(crate::ChangeSet { revision: self.state.revision, changes });
        }
        let revision = self.state.revision.0.checked_add(1).ok_or(InventoryError::RevisionExhausted)?;
        let revision = InventoryRevision(revision);
        next.revision = revision;
        self.state = Arc::new(next);
        Ok(crate::ChangeSet { revision, changes })
    }

    fn install_categories(&mut self, categories: impl IntoIterator<Item = Category>) -> Result<(), InventoryError> {
        let mut next = self.state.as_ref().clone();
        for category in categories {
            validate_category(&category)?;
            if next.categories.contains_key(&category.key) {
                return Err(InventoryError::DuplicateCategory);
            }
            if next.categories.len() >= MAX_INVENTORY_CATEGORIES {
                return Err(InventoryError::TooManyCategories);
            }
            let Some(parent) = category.parent else {
                return Err(InventoryError::InvalidRoot);
            };
            if parent.owner() != category.key.owner() || !next.categories.contains_key(&parent) {
                return Err(InventoryError::MissingParent);
            }
            insert_child(&mut next, parent, InventoryNodeId::Category(category.key.id()))?;
            replace_category_record(&mut next, category)?;
        }
        self.state = Arc::new(next);
        Ok(())
    }
}

pub(crate) fn insert_category_record(state: &mut InventoryState, category: Category) -> Result<(), InventoryError> {
    if state.categories.contains_key(&category.key) {
        return Err(InventoryError::InconsistentIndex);
    }
    state.children.insert(category.key, OrdSet::new());
    replace_category_record(state, category)
}

pub(crate) fn replace_category_record(state: &mut InventoryState, category: Category) -> Result<(), InventoryError> {
    let key = category.key;
    if !state.children.contains_key(&key) {
        state.children.insert(key, OrdSet::new());
    }
    let children = state.children.get(&key).ok_or(InventoryError::InconsistentIndex)?;
    let child_count = children.len();
    let previous = state.categories.get(&key).cloned();
    let was_complete = previous.as_ref().is_some_and(|previous| category_is_complete(previous, child_count));
    let is_complete = category_is_complete(&category, child_count);
    update_incomplete_count(state, key.owner(), previous.as_ref().is_some_and(|_| !was_complete), !is_complete)?;
    if let Some(previous) = previous.as_ref()
        && previous.preferred_folder_type != category.preferred_folder_type
    {
        remove_category_type(state, key, previous.preferred_folder_type)?;
    }
    if previous.as_ref().is_none_or(|previous| previous.preferred_folder_type != category.preferred_folder_type) {
        insert_category_type(state, key, category.preferred_folder_type)?;
    }
    state.categories.insert(key, Arc::new(category));
    Ok(())
}

pub(crate) fn remove_category_record(state: &mut InventoryState, key: CategoryKey) -> Result<(), InventoryError> {
    let category = state.categories.get(&key).cloned().ok_or(InventoryError::InconsistentIndex)?;
    let children = state.children.get(&key).cloned().ok_or(InventoryError::InconsistentIndex)?;
    if !children.is_empty() {
        return Err(InventoryError::InconsistentIndex);
    }
    update_incomplete_count(state, key.owner(), !category_is_complete(&category, children.len()), false)?;
    remove_category_type(state, key, category.preferred_folder_type)?;
    state.categories.remove(&key);
    state.children.remove(&key);
    Ok(())
}

pub(crate) fn insert_child(state: &mut InventoryState, parent: CategoryKey, child: InventoryNodeId) -> Result<(), InventoryError> {
    let category = state.categories.get(&parent).cloned().ok_or(InventoryError::InconsistentIndex)?;
    let mut children = state.children.get(&parent).cloned().ok_or(InventoryError::InconsistentIndex)?;
    if children.contains(&child) {
        return Err(InventoryError::InconsistentIndex);
    }
    let was_complete = category_is_complete(&category, children.len());
    children.insert(child);
    let is_complete = category_is_complete(&category, children.len());
    update_incomplete_count(state, parent.owner(), !was_complete, !is_complete)?;
    state.children.insert(parent, children);
    Ok(())
}

pub(crate) fn remove_child(state: &mut InventoryState, parent: CategoryKey, child: InventoryNodeId) -> Result<(), InventoryError> {
    let category = state.categories.get(&parent).cloned().ok_or(InventoryError::InconsistentIndex)?;
    let mut children = state.children.get(&parent).cloned().ok_or(InventoryError::InconsistentIndex)?;
    if !children.contains(&child) {
        return Err(InventoryError::InconsistentIndex);
    }
    let was_complete = category_is_complete(&category, children.len());
    children.remove(&child);
    let is_complete = category_is_complete(&category, children.len());
    update_incomplete_count(state, parent.owner(), !was_complete, !is_complete)?;
    state.children.insert(parent, children);
    Ok(())
}

fn category_is_complete(category: &Category, child_count: usize) -> bool {
    category.server_version.is_known()
        && category.server_descendent_count.and_then(|count| usize::try_from(count).ok()) == Some(child_count)
}

fn update_incomplete_count(
    state: &mut InventoryState,
    owner: InventoryOwner,
    was_incomplete: bool,
    is_incomplete: bool,
) -> Result<(), InventoryError> {
    if was_incomplete == is_incomplete {
        return Ok(());
    }
    let count = state.incomplete_categories.get(&owner).copied().unwrap_or_default();
    if is_incomplete {
        let updated = count.checked_add(1).ok_or(InventoryError::InconsistentIndex)?;
        state.incomplete_categories.insert(owner, updated);
    } else {
        let updated = count.checked_sub(1).ok_or(InventoryError::InconsistentIndex)?;
        if updated == 0 {
            state.incomplete_categories.remove(&owner);
        } else {
            state.incomplete_categories.insert(owner, updated);
        }
    }
    Ok(())
}

fn insert_category_type(state: &mut InventoryState, key: CategoryKey, folder_type: FolderType) -> Result<(), InventoryError> {
    let type_key = (key.owner(), folder_type);
    let mut categories = state.categories_by_type.get(&type_key).cloned().unwrap_or_default();
    if categories.contains(&key) {
        return Err(InventoryError::InconsistentIndex);
    }
    categories.insert(key);
    state.categories_by_type.insert(type_key, categories);
    if state.preferred_category_by_type.get(&type_key).is_none_or(|preferred| key < *preferred) {
        state.preferred_category_by_type.insert(type_key, key);
    }
    Ok(())
}

fn remove_category_type(state: &mut InventoryState, key: CategoryKey, folder_type: FolderType) -> Result<(), InventoryError> {
    let type_key = (key.owner(), folder_type);
    let mut categories = state.categories_by_type.get(&type_key).cloned().ok_or(InventoryError::InconsistentIndex)?;
    if !categories.contains(&key) {
        return Err(InventoryError::InconsistentIndex);
    }
    categories.remove(&key);
    if categories.is_empty() {
        state.categories_by_type.remove(&type_key);
        state.preferred_category_by_type.remove(&type_key);
    } else {
        if state.preferred_category_by_type.get(&type_key) == Some(&key) {
            let preferred = categories.iter().next().copied().ok_or(InventoryError::InconsistentIndex)?;
            state.preferred_category_by_type.insert(type_key, preferred);
        }
        state.categories_by_type.insert(type_key, categories);
    }
    Ok(())
}

pub(crate) fn validate_category(category: &Category) -> Result<(), InventoryError> {
    if category.key.owner().id().is_nil() {
        return Err(InventoryError::InvalidOwner);
    }
    if category.key.id().uuid().is_nil() {
        return Err(InventoryError::InvalidCategoryId);
    }
    if category.name.len() > MAX_INVENTORY_NAME_BYTES {
        return Err(InventoryError::NameTooLong);
    }
    if category.parent.is_some_and(|parent| parent.owner() != category.key.owner()) {
        return Err(InventoryError::InvalidParent);
    }
    Ok(())
}

pub(crate) fn validate_item(item: &Item) -> Result<(), InventoryError> {
    if item.key.owner().id().is_nil() {
        return Err(InventoryError::InvalidOwner);
    }
    if item.key.id().uuid().is_nil() {
        return Err(InventoryError::InvalidItemId);
    }
    if item.parent.owner() != item.key.owner() {
        return Err(InventoryError::InvalidParent);
    }
    if item.name.len() > MAX_INVENTORY_NAME_BYTES {
        return Err(InventoryError::NameTooLong);
    }
    if item.description.len() > MAX_INVENTORY_DESCRIPTION_BYTES {
        return Err(InventoryError::DescriptionTooLong);
    }
    Ok(())
}
