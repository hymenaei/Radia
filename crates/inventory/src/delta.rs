use std::collections::{BTreeSet, HashMap, HashSet};

use crate::state::{InventoryState, replace_category_record};
use crate::{
    Category, CategoryKey, ChangeSet, Inventory, InventoryChange, InventoryDelta, InventoryError, ServerVersion,
    mutation::{remove_category_subtree, remove_item, upsert_category_state, upsert_item_state},
    state::validate_category,
};

impl Inventory {
    pub fn apply_delta(&mut self, delta: InventoryDelta) -> Result<ChangeSet, InventoryError> {
        validate_delta_size(&delta, self.limits.max_categories, self.limits.max_items)?;

        let previous = self.state.as_ref();
        let mut next = previous.clone();
        let mut changes = Vec::new();
        let mut refetch = BTreeSet::new();

        let stale_versions = delta
            .updated_category_versions
            .iter()
            .filter_map(|(folder, version)| {
                let current = previous.categories.get(folder)?;
                (current.server_version.is_known() && version.is_known() && version.raw() < current.server_version.raw()).then_some(*folder)
            })
            .collect::<BTreeSet<_>>();
        if !stale_versions.is_empty() {
            for folder in stale_versions {
                if let Some(version) = next.categories.get(&folder).map(|category| category.server_version) {
                    replace_folder_metadata(&mut next, folder, version, None, &mut changes)?;
                    refetch.insert(folder);
                }
            }
            return self.publish_with_refetch(next, changes, refetch.into_iter().collect());
        }

        let created_categories = delta
            .created_categories
            .iter()
            .chain(&delta.updated_categories)
            .map(|category| category.key)
            .filter(|folder| !previous.categories.contains_key(folder))
            .collect::<BTreeSet<_>>();
        apply_categories(&mut next, &delta.created_categories, &mut changes)?;
        apply_categories(&mut next, &delta.updated_categories, &mut changes)?;
        for item in delta.created_items.iter().chain(&delta.updated_items) {
            upsert_item_state(&mut next, item.clone(), &mut changes)?;
        }
        for key in &delta.removed_items {
            remove_item(&mut next, *key, &mut changes)?;
        }
        for key in &delta.removed_categories {
            if next.categories.contains_key(key) {
                if next.roots.contains(*key) {
                    return Err(InventoryError::FixedRoot);
                }
                remove_category_subtree(&mut next, *key, &mut changes)?;
            }
        }

        let touched = touched_parents(previous, &next, &delta);
        for folder in &touched {
            if previous.categories.contains_key(folder) {
                account_local_update(previous, &mut next, *folder, &mut changes, &mut refetch)?;
            }
        }
        for (folder, version) in &delta.updated_category_versions {
            apply_authoritative_version(previous, &mut next, *folder, *version, touched.contains(folder), &mut changes, &mut refetch)?;
        }
        for folder in created_categories {
            if next.categories.contains_key(&folder) {
                validate_new_folder(&mut next, folder, &mut changes, &mut refetch)?;
            }
        }

        let categories = next.categories.len();
        let items = next.items.len();
        if categories > self.limits.max_categories {
            return Err(InventoryError::TooManyCategories);
        }
        if items > self.limits.max_items {
            return Err(InventoryError::TooManyItems);
        }
        self.publish_with_refetch(next, changes, refetch.into_iter().collect())
    }
}

fn validate_delta_size(delta: &InventoryDelta, max_categories: usize, max_items: usize) -> Result<(), InventoryError> {
    let category_upserts =
        delta.created_categories.len().checked_add(delta.updated_categories.len()).ok_or(InventoryError::TooManyDeltaOperations)?;
    let item_upserts = delta.created_items.len().checked_add(delta.updated_items.len()).ok_or(InventoryError::TooManyDeltaOperations)?;
    if category_upserts > max_categories
        || delta.removed_categories.len() > max_categories
        || delta.updated_category_versions.len() > max_categories
        || item_upserts > max_items
        || delta.removed_items.len() > max_items
    {
        return Err(InventoryError::TooManyDeltaOperations);
    }
    Ok(())
}

fn apply_categories(state: &mut InventoryState, categories: &[Category], changes: &mut Vec<InventoryChange>) -> Result<(), InventoryError> {
    let mut pending = categories.to_vec();
    while !pending.is_empty() {
        let mut deferred = Vec::new();
        let mut applied = 0usize;
        for mut category in pending {
            validate_category(&category)?;
            if !state.roots.contains(category.key) {
                let parent = category.parent.ok_or(InventoryError::InvalidParent)?;
                if !state.categories.contains_key(&parent) {
                    deferred.push(category);
                    continue;
                }
            }
            if let Some(current) = state.categories.get(&category.key) {
                category.server_version = current.server_version;
                category.server_descendent_count = current.server_descendent_count;
            }
            upsert_category_state(state, category, changes)?;
            applied += 1;
        }
        if deferred.is_empty() {
            break;
        }
        if applied == 0 {
            if categories_have_cycle(&deferred) {
                return Err(InventoryError::Cycle);
            }
            return Err(InventoryError::MissingParent);
        }
        pending = deferred;
    }
    Ok(())
}

fn categories_have_cycle(categories: &[Category]) -> bool {
    let parents = categories.iter().filter_map(|category| category.parent.map(|parent| (category.key, parent))).collect::<HashMap<_, _>>();
    for start in parents.keys().copied() {
        let mut current = start;
        let mut visited = HashSet::new();
        while let Some(parent) = parents.get(&current).copied() {
            if !visited.insert(current) {
                return true;
            }
            current = parent;
        }
    }
    false
}

fn touched_parents(previous: &InventoryState, next: &InventoryState, delta: &InventoryDelta) -> BTreeSet<CategoryKey> {
    let mut touched = BTreeSet::new();
    for key in delta
        .created_categories
        .iter()
        .chain(&delta.updated_categories)
        .map(|category| category.key)
        .chain(delta.removed_categories.iter().copied())
    {
        let before = previous.categories.get(&key);
        let after = next.categories.get(&key);
        add_changed_parents(&mut touched, next, before.and_then(|category| category.parent), after.and_then(|category| category.parent));
    }
    for key in delta.created_items.iter().chain(&delta.updated_items).map(|item| item.key).chain(delta.removed_items.iter().copied()) {
        let before = previous.items.get(&key);
        let after = next.items.get(&key);
        add_changed_parents(&mut touched, next, before.map(|item| item.parent), after.map(|item| item.parent));
    }
    touched
}

fn add_changed_parents(
    touched: &mut BTreeSet<CategoryKey>,
    next: &InventoryState,
    before: Option<CategoryKey>,
    after: Option<CategoryKey>,
) {
    if before != after {
        for parent in before.into_iter().chain(after) {
            if next.categories.contains_key(&parent) {
                touched.insert(parent);
            }
        }
    }
}

fn account_local_update(
    previous: &InventoryState,
    next: &mut InventoryState,
    folder: CategoryKey,
    changes: &mut Vec<InventoryChange>,
    refetch: &mut BTreeSet<CategoryKey>,
) -> Result<(), InventoryError> {
    let before = previous.categories.get(&folder).ok_or(InventoryError::InconsistentIndex)?;
    let before_children = previous.children.get(&folder).ok_or(InventoryError::InconsistentIndex)?.len();
    let after_children = next.children.get(&folder).ok_or(InventoryError::InconsistentIndex)?.len();
    let count_was_known = before.server_version.is_known()
        && before.server_descendent_count.and_then(|count| usize::try_from(count).ok()) == Some(before_children);
    let next_version = count_was_known
        .then(|| before.server_version.raw().checked_add(1).map(ServerVersion::from_raw))
        .flatten()
        .filter(|version| version.is_known());
    let next_count = count_was_known.then(|| u32::try_from(after_children).ok()).flatten();
    let version = next_version.unwrap_or(before.server_version);
    let next_count = next_count.filter(|count| usize::try_from(*count).ok() == Some(after_children));
    replace_folder_metadata(next, folder, version, next_count, changes)?;
    if next_version.is_none() || next_count.is_none() {
        refetch.insert(folder);
    }
    Ok(())
}

fn validate_new_folder(
    state: &mut InventoryState,
    folder: CategoryKey,
    changes: &mut Vec<InventoryChange>,
    refetch: &mut BTreeSet<CategoryKey>,
) -> Result<(), InventoryError> {
    let category = state.categories.get(&folder).cloned().ok_or(InventoryError::InconsistentIndex)?;
    let actual_count = state.children.get(&folder).ok_or(InventoryError::InconsistentIndex)?.len();
    let count_matches = category.server_descendent_count.and_then(|count| usize::try_from(count).ok()) == Some(actual_count);
    if !category.server_version.is_known() || !count_matches {
        replace_folder_metadata(state, folder, category.server_version, None, changes)?;
        refetch.insert(folder);
    }
    Ok(())
}

fn apply_authoritative_version(
    previous: &InventoryState,
    next: &mut InventoryState,
    folder: CategoryKey,
    version: ServerVersion,
    locally_touched: bool,
    changes: &mut Vec<InventoryChange>,
    refetch: &mut BTreeSet<CategoryKey>,
) -> Result<(), InventoryError> {
    let Some(current) = next.categories.get(&folder).cloned() else { return Ok(()) };
    let was_created = !previous.categories.contains_key(&folder);
    if !version.is_known() {
        replace_folder_metadata(next, folder, current.server_version, None, changes)?;
        refetch.insert(folder);
        return Ok(());
    }
    if was_created {
        let child_count = next.children.get(&folder).ok_or(InventoryError::InconsistentIndex)?.len();
        let count_matches = current.server_descendent_count.and_then(|count| usize::try_from(count).ok()) == Some(child_count);
        let count = (version.is_known() && count_matches).then_some(current.server_descendent_count).flatten();
        replace_folder_metadata(next, folder, version, count, changes)?;
        if !version.is_known() || count.is_none() {
            refetch.insert(folder);
        }
        return Ok(());
    }

    let predicted = current.server_version;
    let matches_local = locally_touched && predicted.is_known() && predicted == version;
    if matches_local {
        if !folder_is_complete(next, folder)? {
            refetch.insert(folder);
        }
        replace_folder_metadata(next, folder, version, current.server_descendent_count, changes)?;
        return Ok(());
    }

    if !locally_touched && current.server_version == version {
        return Ok(());
    }

    replace_folder_metadata(next, folder, version, None, changes)?;
    refetch.insert(folder);
    Ok(())
}

fn folder_is_complete(state: &InventoryState, folder: CategoryKey) -> Result<bool, InventoryError> {
    let category = state.categories.get(&folder).ok_or(InventoryError::InconsistentIndex)?;
    let children = state.children.get(&folder).ok_or(InventoryError::InconsistentIndex)?.len();
    Ok(category.server_version.is_known()
        && category.server_descendent_count.and_then(|count| usize::try_from(count).ok()) == Some(children))
}

fn replace_folder_metadata(
    state: &mut InventoryState,
    folder: CategoryKey,
    version: ServerVersion,
    descendent_count: Option<u32>,
    changes: &mut Vec<InventoryChange>,
) -> Result<(), InventoryError> {
    let current = state.categories.get(&folder).cloned().ok_or(InventoryError::InconsistentIndex)?;
    if current.server_version == version && current.server_descendent_count == descendent_count {
        return Ok(());
    }
    let mut updated = current.as_ref().clone();
    updated.server_version = version;
    updated.server_descendent_count = descendent_count;
    replace_category_record(state, updated)?;
    let updated = state.categories.get(&folder).cloned().ok_or(InventoryError::InconsistentIndex)?;
    if current.server_version != version {
        changes.push(InventoryChange::VersionSet { folder, version });
    }
    changes.push(InventoryChange::CategoryUpserted(updated));
    Ok(())
}
