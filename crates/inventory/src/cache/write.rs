use std::collections::BTreeSet;

use crate::{Category, CategoryKey, ChangeSet, InventoryChange, InventoryNodeId, InventorySnapshot, Item, ItemKey};
use rusqlite::{Transaction, params};
use storage::sqlite::{MAX_BATCH_BYTES, MAX_BATCH_ROWS, SqliteBatch, SqliteError};

pub(super) fn bootstrap(snapshot: &InventorySnapshot) -> Result<Vec<SqliteBatch>, SqliteError> {
    categories(snapshot.categories().cloned().collect())
}

pub(super) fn changeset(
    changes: &ChangeSet,
    before: &InventorySnapshot,
    after: &InventorySnapshot,
) -> Result<Vec<SqliteBatch>, SqliteError> {
    let affected = affected_categories(changes, before, after);
    let mut changed_categories = BTreeSet::new();
    let mut changed_items = BTreeSet::new();
    let mut replaced = BTreeSet::new();
    for change in &changes.changes {
        match change {
            InventoryChange::CategoryUpserted(category) => {
                changed_categories.insert(category.key);
            }
            InventoryChange::ItemUpserted(item) => {
                changed_items.insert(item.key);
            }
            InventoryChange::NodeMoved { node, .. } => match node.id() {
                InventoryNodeId::Category(id) => {
                    changed_categories.insert(CategoryKey::new(node.owner(), id));
                }
                InventoryNodeId::Item(id) => {
                    changed_items.insert(ItemKey::new(node.owner(), id));
                }
            },
            InventoryChange::CategoryRemoved(key) => {
                changed_categories.insert(*key);
            }
            InventoryChange::ItemRemoved(key) => {
                changed_items.insert(*key);
            }
            InventoryChange::FolderFetchApplied { folder, .. } => {
                replaced.insert(*folder);
            }
            InventoryChange::VersionSet { .. } => {}
        }
    }
    let category_keys = affected.iter().copied().chain(changed_categories.iter().copied()).collect::<BTreeSet<_>>();
    let category_upserts = category_keys.iter().filter_map(|key| after.category(*key).cloned()).collect::<Vec<_>>();
    let category_deletes = changed_categories.iter().copied().filter(|key| after.category(*key).is_none()).collect::<Vec<_>>();
    let item_upserts = changed_items.iter().filter_map(|key| after.item(*key).cloned()).collect::<Vec<_>>();
    let item_deletes = changed_items.iter().copied().filter(|key| after.item(*key).is_none()).collect::<Vec<_>>();
    let complete = affected
        .iter()
        .copied()
        .filter(|key| after.is_complete(*key))
        .filter_map(|key| {
            let category = after.category(key)?;
            let child_count = u32::try_from(after.child_count(key)).ok()?;
            Some((key, category.server_version.raw(), child_count, replaced.contains(&key)))
        })
        .collect::<Vec<_>>();

    let mut batches = Vec::new();
    batches.extend(invalidate(affected)?);
    batches.extend(categories(category_upserts)?);
    batches.extend(items(item_upserts)?);
    batches.extend(delete_items(item_deletes)?);
    batches.extend(delete_categories(category_deletes)?);
    batches.extend(mark_complete(complete)?);
    Ok(batches)
}

pub(super) fn affected_categories(changes: &ChangeSet, before: &InventorySnapshot, after: &InventorySnapshot) -> Vec<CategoryKey> {
    let mut affected = BTreeSet::new();
    for change in &changes.changes {
        match change {
            InventoryChange::CategoryUpserted(category) => {
                affected.insert(category.key);
                add_parent(&mut affected, before.category(category.key).and_then(|category| category.parent));
                add_parent(&mut affected, after.category(category.key).and_then(|category| category.parent));
            }
            InventoryChange::ItemUpserted(item) => {
                add_parent(&mut affected, before.item(item.key).map(|item| item.parent));
                add_parent(&mut affected, after.item(item.key).map(|item| item.parent));
            }
            InventoryChange::NodeMoved { node, from, to } => {
                affected.insert(*from);
                affected.insert(*to);
                match node.id() {
                    InventoryNodeId::Category(id) => {
                        affected.insert(CategoryKey::new(node.owner(), id));
                    }
                    InventoryNodeId::Item(id) => {
                        let key = ItemKey::new(node.owner(), id);
                        add_parent(&mut affected, before.item(key).map(|item| item.parent));
                        add_parent(&mut affected, after.item(key).map(|item| item.parent));
                    }
                }
            }
            InventoryChange::CategoryRemoved(key) => {
                affected.insert(*key);
                add_parent(&mut affected, before.category(*key).and_then(|category| category.parent));
            }
            InventoryChange::ItemRemoved(key) => add_parent(&mut affected, before.item(*key).map(|item| item.parent)),
            InventoryChange::FolderFetchApplied { folder, .. } | InventoryChange::VersionSet { folder, .. } => {
                affected.insert(*folder);
            }
        }
    }
    affected.extend(changes.refetch_categories.iter().copied());
    affected.into_iter().collect()
}

fn add_parent(affected: &mut BTreeSet<CategoryKey>, parent: Option<CategoryKey>) {
    if let Some(parent) = parent {
        affected.insert(parent);
    }
}

fn invalidate(keys: Vec<CategoryKey>) -> Result<Vec<SqliteBatch>, SqliteError> {
    chunks(keys, |chunk, bytes| {
        SqliteBatch::new(chunk.len(), bytes, move |transaction| {
            let mut statement = transaction.prepare(
                "UPDATE categories
                 SET complete_generation=CASE WHEN complete=1 THEN complete_generation ELSE 0 END, complete=0
                 WHERE owner_id=?1 AND category_id=?2",
            )?;
            for key in chunk {
                statement.execute(params![key.owner().id().as_bytes().as_slice(), key.id().uuid().as_bytes().as_slice()])?;
            }
            Ok(())
        })
    })
}

fn categories(values: Vec<Category>) -> Result<Vec<SqliteBatch>, SqliteError> {
    let mut batches = Vec::new();
    let mut chunk = Vec::new();
    let mut bytes = 0usize;
    for category in values {
        let row_bytes = 96usize.saturating_add(category.name.len());
        if !chunk.is_empty() && (chunk.len() >= MAX_BATCH_ROWS || bytes.saturating_add(row_bytes) > MAX_BATCH_BYTES) {
            batches.push(category_batch(std::mem::take(&mut chunk), bytes)?);
            bytes = 0;
        }
        bytes = bytes.saturating_add(row_bytes);
        chunk.push(category);
    }
    if !chunk.is_empty() {
        batches.push(category_batch(chunk, bytes)?);
    }
    Ok(batches)
}

fn category_batch(values: Vec<Category>, bytes: usize) -> Result<SqliteBatch, SqliteError> {
    SqliteBatch::new(values.len(), bytes, move |transaction| upsert_categories(transaction, values))
}

fn upsert_categories(transaction: &Transaction<'_>, categories: Vec<Category>) -> rusqlite::Result<()> {
    let mut statement = transaction.prepare(
        "INSERT INTO categories
            (owner_id, category_id, parent_id, preferred_type, name, server_version, server_descendent_count, complete, complete_generation)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0)
         ON CONFLICT(owner_id, category_id) DO UPDATE SET
            parent_id=excluded.parent_id,
            preferred_type=excluded.preferred_type,
            name=excluded.name,
            server_version=excluded.server_version,
            server_descendent_count=CASE WHEN categories.server_version=excluded.server_version
                                              AND excluded.server_descendent_count IS NULL
                                         THEN categories.server_descendent_count ELSE excluded.server_descendent_count END,
            complete=CASE WHEN categories.server_version=excluded.server_version
                               AND (excluded.server_descendent_count IS NULL
                                    OR categories.server_descendent_count IS excluded.server_descendent_count)
                          THEN categories.complete ELSE 0 END,
            complete_generation=CASE WHEN categories.server_version=excluded.server_version
                                          AND (excluded.server_descendent_count IS NULL
                                               OR categories.server_descendent_count IS excluded.server_descendent_count)
                                     THEN categories.complete_generation ELSE 0 END",
    )?;
    for category in categories {
        let parent = category.parent.map(|parent| *parent.id().uuid().as_bytes());
        let parent = parent.as_ref().map(|id| id.as_slice());
        statement.execute(params![
            category.key.owner().id().as_bytes().as_slice(),
            category.key.id().uuid().as_bytes().as_slice(),
            parent,
            category.preferred_folder_type.raw(),
            category.name.as_ref(),
            category.server_version.raw(),
            category.server_descendent_count.map(i64::from),
        ])?;
    }
    Ok(())
}

fn items(values: Vec<Item>) -> Result<Vec<SqliteBatch>, SqliteError> {
    let mut batches = Vec::new();
    let mut chunk = Vec::new();
    let mut bytes = 0usize;
    for item in values {
        let row_bytes = 256usize.saturating_add(item.name.len()).saturating_add(item.description.len());
        if !chunk.is_empty() && (chunk.len() >= MAX_BATCH_ROWS || bytes.saturating_add(row_bytes) > MAX_BATCH_BYTES) {
            batches.push(item_batch(std::mem::take(&mut chunk), bytes)?);
            bytes = 0;
        }
        bytes = bytes.saturating_add(row_bytes);
        chunk.push(item);
    }
    if !chunk.is_empty() {
        batches.push(item_batch(chunk, bytes)?);
    }
    Ok(batches)
}

fn item_batch(values: Vec<Item>, bytes: usize) -> Result<SqliteBatch, SqliteError> {
    SqliteBatch::new(values.len(), bytes, move |transaction| upsert_items(transaction, values))
}

fn upsert_items(transaction: &Transaction<'_>, items: Vec<Item>) -> rusqlite::Result<()> {
    let mut statement = transaction.prepare(
        "INSERT INTO items
            (owner_id, item_id, parent_id, asset_id, asset_type, inventory_type, flags, name, description, created_at,
             creator_id, item_owner_id, group_id, last_owner_id, base_mask, owner_mask, group_mask, everyone_mask,
             next_owner_mask, sale_type, sale_price)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
         ON CONFLICT(owner_id, item_id) DO UPDATE SET
            parent_id=excluded.parent_id, asset_id=excluded.asset_id, asset_type=excluded.asset_type,
            inventory_type=excluded.inventory_type, flags=excluded.flags, name=excluded.name,
            description=excluded.description, created_at=excluded.created_at, creator_id=excluded.creator_id,
            item_owner_id=excluded.item_owner_id, group_id=excluded.group_id, last_owner_id=excluded.last_owner_id,
            base_mask=excluded.base_mask, owner_mask=excluded.owner_mask, group_mask=excluded.group_mask,
            everyone_mask=excluded.everyone_mask, next_owner_mask=excluded.next_owner_mask,
            sale_type=excluded.sale_type, sale_price=excluded.sale_price",
    )?;
    for item in items {
        statement.execute(params![
            item.key.owner().id().as_bytes().as_slice(),
            item.key.id().uuid().as_bytes().as_slice(),
            item.parent.id().uuid().as_bytes().as_slice(),
            item.asset_id.uuid().as_bytes().as_slice(),
            item.asset_type.raw(),
            item.inventory_type.raw(),
            i64::from(item.flags),
            item.name.as_ref(),
            item.description.as_ref(),
            item.created_at,
            item.permissions.creator.uuid().as_bytes().as_slice(),
            item.permissions.owner.uuid().as_bytes().as_slice(),
            item.permissions.group.uuid().as_bytes().as_slice(),
            item.permissions.last_owner.uuid().as_bytes().as_slice(),
            i64::from(item.permissions.base.bits()),
            i64::from(item.permissions.owner_mask.bits()),
            i64::from(item.permissions.group_mask.bits()),
            i64::from(item.permissions.everyone.bits()),
            i64::from(item.permissions.next_owner.bits()),
            item.sale.sale_type.raw(),
            item.sale.price,
        ])?;
    }
    Ok(())
}

fn delete_items(keys: Vec<ItemKey>) -> Result<Vec<SqliteBatch>, SqliteError> {
    chunks(keys, |chunk, bytes| {
        SqliteBatch::new(chunk.len(), bytes, move |transaction| {
            let mut statement = transaction.prepare("DELETE FROM items WHERE owner_id=?1 AND item_id=?2")?;
            for key in chunk {
                statement.execute(params![key.owner().id().as_bytes().as_slice(), key.id().uuid().as_bytes().as_slice()])?;
            }
            Ok(())
        })
    })
}

fn delete_categories(keys: Vec<CategoryKey>) -> Result<Vec<SqliteBatch>, SqliteError> {
    chunks(keys, |chunk, bytes| {
        SqliteBatch::new(chunk.len(), bytes, move |transaction| {
            let mut statement = transaction.prepare("DELETE FROM categories WHERE owner_id=?1 AND category_id=?2")?;
            for key in chunk {
                statement.execute(params![key.owner().id().as_bytes().as_slice(), key.id().uuid().as_bytes().as_slice()])?;
            }
            Ok(())
        })
    })
}

fn mark_complete(values: Vec<(CategoryKey, i32, u32, bool)>) -> Result<Vec<SqliteBatch>, SqliteError> {
    chunks(values, |chunk, bytes| {
        SqliteBatch::new(chunk.len(), bytes, move |transaction| {
            let generation: i64 = transaction.query_row("SELECT cache_generation FROM meta WHERE id=1", [], |row| row.get(0))?;
            let mut statement = transaction.prepare(
                "UPDATE categories SET complete=1, complete_generation=?1
                 WHERE owner_id=?2 AND category_id=?3 AND server_version=?4 AND server_descendent_count=?5
                   AND (?6=1 OR complete_generation=?1)",
            )?;
            for (key, version, child_count, replace) in chunk {
                statement.execute(params![
                    generation,
                    key.owner().id().as_bytes().as_slice(),
                    key.id().uuid().as_bytes().as_slice(),
                    version,
                    i64::from(child_count),
                    i64::from(replace),
                ])?;
            }
            Ok(())
        })
    })
}

fn chunks<T, F>(values: Vec<T>, mut make: F) -> Result<Vec<SqliteBatch>, SqliteError>
where
    F: FnMut(Vec<T>, usize) -> Result<SqliteBatch, SqliteError>,
{
    let mut batches = Vec::new();
    let mut chunk = Vec::new();
    let mut bytes = 0usize;
    for value in values {
        if !chunk.is_empty() && chunk.len() >= MAX_BATCH_ROWS {
            batches.push(make(std::mem::take(&mut chunk), bytes)?);
            bytes = 0;
        }
        chunk.push(value);
        bytes = bytes.saturating_add(64);
    }
    if !chunk.is_empty() {
        batches.push(make(chunk, bytes)?);
    }
    Ok(batches)
}

impl From<rusqlite::Error> for super::InventoryCacheError {
    fn from(error: rusqlite::Error) -> Self {
        super::InventoryCacheError::Sqlite(SqliteError::Sqlite(error))
    }
}
