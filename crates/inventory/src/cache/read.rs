use std::{
    collections::{HashMap, HashSet},
    io,
    sync::Arc,
};

use rusqlite::{Row, Transaction, types::Type};
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

use crate::{
    AssetType, Category, CategoryKey, FolderType, InventoryChild, InventoryNodeId, InventoryNodeKey, InventoryOwner, InventoryRoots,
    InventoryType, Item, ItemKey, PermissionMask, Permissions, SaleInfo, SaleType, ServerVersion,
};

use super::{CachedFolder, InventoryCacheLoad, InventoryLimits, MAX_READ_BYTES};

const CATEGORY_ESTIMATED_BYTES: usize = 256;
const ITEM_ESTIMATED_BYTES: usize = 1_024;

struct CategoryRow {
    category: Category,
    complete: bool,
    complete_generation: i64,
}

pub(super) fn read_tree(
    transaction: &Transaction<'_>,
    roots: InventoryRoots,
    agent_id: Uuid,
    library_owner: Option<Uuid>,
    limits: InventoryLimits,
) -> rusqlite::Result<InventoryCacheLoad> {
    let generation: i64 = transaction.query_row("SELECT cache_generation FROM meta WHERE id=1", [], |row| row.get(0))?;
    if generation < 1 {
        return Ok(discarded());
    }
    let (category_count, category_text_bytes): (i64, i64) =
        transaction.query_row("SELECT COUNT(*), COALESCE(SUM(length(CAST(name AS BLOB))), 0) FROM categories", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    let (item_count, item_text_bytes): (i64, i64) = transaction.query_row(
        "SELECT COUNT(*), COALESCE(SUM(length(CAST(name AS BLOB))+length(CAST(description AS BLOB))), 0) FROM items",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let (Ok(category_count), Ok(item_count), Ok(category_text_bytes), Ok(item_text_bytes)) = (
        usize::try_from(category_count),
        usize::try_from(item_count),
        usize::try_from(category_text_bytes),
        usize::try_from(item_text_bytes),
    ) else {
        return Ok(discarded());
    };
    let memory_estimate = category_count
        .checked_mul(CATEGORY_ESTIMATED_BYTES)
        .and_then(|bytes| bytes.checked_add(item_count.checked_mul(ITEM_ESTIMATED_BYTES)?))
        .and_then(|bytes| bytes.checked_add(category_text_bytes))
        .and_then(|bytes| bytes.checked_add(item_text_bytes));
    if category_count > limits.max_categories || item_count > limits.max_items || memory_estimate.is_none_or(|bytes| bytes > MAX_READ_BYTES)
    {
        return Ok(discarded());
    }
    if category_count == 0 && item_count == 0 {
        return Ok(empty());
    }

    let mut categories = read_categories(transaction)?;
    let items = read_items(transaction)?;
    if !tree_is_valid(&categories, &items, roots, agent_id, library_owner, generation) {
        return Ok(discarded());
    }

    let mut children = HashMap::<CategoryKey, Vec<InventoryChild>>::new();
    for row in &categories {
        if let Some(parent) = row.category.parent {
            children.entry(parent).or_default().push(InventoryChild::Category(row.category.clone()));
        }
    }
    for item in items {
        children.entry(item.parent).or_default().push(InventoryChild::Item(item));
    }
    for nodes in children.values_mut() {
        nodes.sort_by_key(|node| match node {
            InventoryChild::Category(category) => InventoryNodeKey::new(category.key.owner(), InventoryNodeId::Category(category.key.id())),
            InventoryChild::Item(item) => InventoryNodeKey::new(item.key.owner(), InventoryNodeId::Item(item.key.id())),
        });
    }

    let mut folders = Vec::new();
    for row in categories.drain(..) {
        if row.complete && row.complete_generation == generation {
            let child_rows = children.remove(&row.category.key).unwrap_or_default();
            folders.push(CachedFolder { category: row.category, children: child_rows });
        }
    }
    Ok(InventoryCacheLoad { folders, discarded: false })
}

fn read_categories(transaction: &Transaction<'_>) -> rusqlite::Result<Vec<CategoryRow>> {
    let mut statement = transaction.prepare(
        "SELECT owner_id, category_id, parent_id, preferred_type, name, server_version,
                server_descendent_count, complete, complete_generation
         FROM categories ORDER BY owner_id, category_id",
    )?;
    statement
        .query_map([], |row| {
            let owner = InventoryOwner::new(read_uuid(row, 0)?);
            let category_id = FolderId::new(read_uuid(row, 1)?);
            let parent_id = row.get::<_, Option<Vec<u8>>>(2)?.map(|bytes| read_uuid_bytes(bytes, 2).map(FolderId::new)).transpose()?;
            let preferred_type = FolderType::from_raw(row.get(3)?);
            let name: String = row.get(4)?;
            let server_version = ServerVersion::from_raw(read_i32(row, 5)?);
            let server_descendent_count = row.get::<_, Option<i64>>(6)?.map(|value| read_u32(value, 6)).transpose()?;
            let complete = read_bool(row, 7)?;
            let complete_generation: i64 = row.get(8)?;
            let category = Category {
                key: CategoryKey::new(owner, category_id),
                parent: parent_id.map(|id| CategoryKey::new(owner, id)),
                preferred_folder_type: preferred_type,
                name: Arc::from(name),
                server_version,
                server_descendent_count,
            };
            Ok(CategoryRow { category, complete, complete_generation })
        })?
        .collect()
}

fn read_items(transaction: &Transaction<'_>) -> rusqlite::Result<Vec<Item>> {
    let mut statement = transaction.prepare(
        "SELECT owner_id, item_id, parent_id, asset_id, asset_type, inventory_type, flags, name, description, created_at,
                creator_id, item_owner_id, group_id, last_owner_id, base_mask, owner_mask, group_mask, everyone_mask,
                next_owner_mask, sale_type, sale_price
         FROM items ORDER BY owner_id, item_id",
    )?;
    statement
        .query_map([], |row| {
            let owner = InventoryOwner::new(read_uuid(row, 0)?);
            let item_id = ItemId::new(read_uuid(row, 1)?);
            let parent_id = FolderId::new(read_uuid(row, 2)?);
            let asset_id = AssetId::new(read_uuid(row, 3)?);
            let asset_type = AssetType::from_raw(read_i32(row, 4)?);
            let inventory_type = InventoryType::from_raw(read_i32(row, 5)?);
            let flags = read_u32(row.get(6)?, 6)?;
            let name: String = row.get(7)?;
            let description: String = row.get(8)?;
            let created_at: i64 = row.get(9)?;
            let permissions = Permissions {
                creator: AvatarId::new(read_uuid(row, 10)?),
                owner: AvatarId::new(read_uuid(row, 11)?),
                group: GroupId::new(read_uuid(row, 12)?),
                last_owner: AvatarId::new(read_uuid(row, 13)?),
                base: PermissionMask::new(read_u32(row.get(14)?, 14)?),
                owner_mask: PermissionMask::new(read_u32(row.get(15)?, 15)?),
                group_mask: PermissionMask::new(read_u32(row.get(16)?, 16)?),
                everyone: PermissionMask::new(read_u32(row.get(17)?, 17)?),
                next_owner: PermissionMask::new(read_u32(row.get(18)?, 18)?),
            };
            let sale = SaleInfo { sale_type: SaleType::from_raw(read_i32(row, 19)?), price: read_i32(row, 20)? };
            Ok(Item {
                key: ItemKey::new(owner, item_id),
                parent: CategoryKey::new(owner, parent_id),
                asset_id,
                asset_type,
                inventory_type,
                flags,
                name: Arc::from(name),
                description: Arc::from(description),
                created_at,
                permissions,
                sale,
            })
        })?
        .collect()
}

fn tree_is_valid(
    categories: &[CategoryRow],
    items: &[Item],
    roots: InventoryRoots,
    agent_id: Uuid,
    library_owner: Option<Uuid>,
    generation: i64,
) -> bool {
    let expected_owners = [Some(InventoryOwner::new(agent_id)), library_owner.map(InventoryOwner::new)];
    let expected_roots = [Some(roots.personal()), roots.library()];
    let mut category_map = HashMap::new();
    let mut root_keys = HashSet::new();
    let mut child_counts = HashMap::<CategoryKey, usize>::new();
    for row in categories {
        let category = &row.category;
        if !expected_owners.contains(&Some(category.key.owner())) || crate::state::validate_category(category).is_err() {
            return false;
        }
        if category_map.insert(category.key, category).is_some() {
            return false;
        }
        match category.parent {
            Some(parent) => {
                if parent.owner() != category.key.owner() {
                    return false;
                }
                let count = child_counts.entry(parent).or_default();
                *count = count.saturating_add(1);
            }
            None => {
                root_keys.insert(category.key);
            }
        }
        if row.complete_generation < 0 || row.complete_generation > generation {
            return false;
        }
    }
    if !root_keys.is_empty() {
        let expected_roots = expected_roots.into_iter().flatten().collect::<HashSet<_>>();
        if root_keys != expected_roots {
            return false;
        }
    }
    for category in categories {
        if let Some(parent) = category.category.parent
            && !category_map.contains_key(&parent)
        {
            return false;
        }
    }
    for item in items {
        if !expected_owners.contains(&Some(item.key.owner()))
            || crate::state::validate_item(item).is_err()
            || !category_map.contains_key(&item.parent)
        {
            return false;
        }
        let count = child_counts.entry(item.parent).or_default();
        *count = count.saturating_add(1);
    }
    for row in categories {
        if row.complete && row.complete_generation == generation {
            let expected_count = row.category.server_descendent_count.and_then(|count| usize::try_from(count).ok());
            if !row.category.server_version.is_known()
                || expected_count != Some(child_counts.get(&row.category.key).copied().unwrap_or_default())
            {
                return false;
            }
        }
    }
    acyclic(&category_map)
}

fn acyclic(categories: &HashMap<CategoryKey, &Category>) -> bool {
    let mut resolved = HashSet::new();
    for start in categories.keys().copied() {
        if resolved.contains(&start) {
            continue;
        }
        let mut current = start;
        let mut path = Vec::new();
        let mut local = HashSet::new();
        loop {
            if resolved.contains(&current) {
                break;
            }
            if !local.insert(current) {
                return false;
            }
            path.push(current);
            match categories.get(&current).and_then(|category| category.parent) {
                Some(parent) => current = parent,
                None => break,
            }
        }
        resolved.extend(path);
    }
    true
}

fn read_uuid(row: &Row<'_>, index: usize) -> rusqlite::Result<Uuid> {
    read_uuid_bytes(row.get(index)?, index)
}

fn read_uuid_bytes(bytes: Vec<u8>, index: usize) -> rusqlite::Result<Uuid> {
    Uuid::from_slice(&bytes).map_err(|error| rusqlite::Error::FromSqlConversionFailure(index, Type::Blob, Box::new(error)))
}

fn read_i32(row: &Row<'_>, index: usize) -> rusqlite::Result<i32> {
    let value: i64 = row.get(index)?;
    i32::try_from(value).map_err(|_| conversion_error(index, Type::Integer))
}

fn read_u32(value: i64, index: usize) -> rusqlite::Result<u32> {
    u32::try_from(value).map_err(|_| conversion_error(index, Type::Integer))
}

fn read_bool(row: &Row<'_>, index: usize) -> rusqlite::Result<bool> {
    match row.get::<_, i64>(index)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(conversion_error(index, Type::Integer)),
    }
}

fn conversion_error(index: usize, kind: Type) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, kind, Box::new(io::Error::new(io::ErrorKind::InvalidData, "invalid inventory row")))
}

fn empty() -> InventoryCacheLoad {
    InventoryCacheLoad { folders: Vec::new(), discarded: false }
}

fn discarded() -> InventoryCacheLoad {
    InventoryCacheLoad { folders: Vec::new(), discarded: true }
}
