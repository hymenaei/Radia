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

use super::{CacheIdentity, CachedFolder, InventoryCacheLoad, InventoryLimits, MAX_READ_BYTES};

const CATEGORY_ESTIMATED_BYTES: usize = 256;
const ITEM_ESTIMATED_BYTES: usize = 1_024;

#[derive(Clone, Copy)]
struct Scope {
    id: i64,
    owner: Uuid,
    generation: i64,
}

struct CategoryRow {
    scope_id: i64,
    category: Category,
    complete: bool,
    complete_generation: i64,
}

pub(super) fn read_tree(
    transaction: &Transaction<'_>,
    identity: &CacheIdentity,
    roots: InventoryRoots,
    limits: InventoryLimits,
) -> rusqlite::Result<InventoryCacheLoad> {
    if roots.personal().owner().id() != identity.personal_owner || roots.library().map(|root| root.owner().id()) != identity.library_owner {
        return Ok(discarded());
    }
    let mut scopes = Vec::new();
    scopes.push(read_scope(transaction, identity, identity.personal_owner)?);
    if let (Some(owner), Some(_)) = (identity.library_owner, roots.library()) {
        scopes.push(read_scope(transaction, identity, owner)?);
    }

    let mut category_count = 0usize;
    let mut item_count = 0usize;
    let mut category_text_bytes = 0usize;
    let mut item_text_bytes = 0usize;
    for scope in &scopes {
        let (categories, category_bytes): (i64, i64) = transaction.query_row(
            "SELECT COUNT(*), COALESCE(SUM(length(CAST(name AS BLOB))), 0) FROM categories WHERE scope_id=?1",
            [scope.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let (items, item_bytes): (i64, i64) = transaction.query_row(
            "SELECT COUNT(*), COALESCE(SUM(length(CAST(name AS BLOB))+length(CAST(description AS BLOB))), 0)
             FROM items WHERE scope_id=?1",
            [scope.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let (Ok(categories), Ok(items), Ok(category_bytes), Ok(item_bytes)) =
            (usize::try_from(categories), usize::try_from(items), usize::try_from(category_bytes), usize::try_from(item_bytes))
        else {
            return Ok(discarded());
        };
        category_count = category_count.saturating_add(categories);
        item_count = item_count.saturating_add(items);
        category_text_bytes = category_text_bytes.saturating_add(category_bytes);
        item_text_bytes = item_text_bytes.saturating_add(item_bytes);
    }
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

    let mut categories = Vec::new();
    let mut items = Vec::new();
    for scope in &scopes {
        categories.extend(read_categories(transaction, *scope)?);
        items.extend(read_items(transaction, *scope)?);
    }
    if !tree_is_valid(&categories, &items, &scopes) {
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

    let generations = scopes.iter().map(|scope| (scope.id, scope.generation)).collect::<HashMap<_, _>>();
    let folders = categories
        .into_iter()
        .map(|row| {
            let complete = row.complete && generations.get(&row.scope_id) == Some(&row.complete_generation);
            let child_rows = children.remove(&row.category.key).unwrap_or_default();
            CachedFolder { category: row.category, children: child_rows, complete }
        })
        .collect::<Vec<_>>()
        .into();
    Ok(InventoryCacheLoad { folders, discarded: false })
}

fn read_scope(transaction: &Transaction<'_>, identity: &CacheIdentity, owner: Uuid) -> rusqlite::Result<Scope> {
    transaction.query_row(
        "SELECT scope_id, cache_generation FROM scopes WHERE grid_id=?1 AND owner_id=?2",
        rusqlite::params![identity.grid_id, owner.as_bytes().as_slice()],
        |row| Ok(Scope { id: row.get(0)?, owner, generation: row.get(1)? }),
    )
}

fn read_categories(transaction: &Transaction<'_>, scope: Scope) -> rusqlite::Result<Vec<CategoryRow>> {
    let mut statement = transaction.prepare(
        "SELECT id, parent_id, preferred_type, name, server_version, server_descendent_count, complete, complete_generation
         FROM categories WHERE scope_id=?1 ORDER BY id",
    )?;
    statement
        .query_map([scope.id], |row| {
            let owner = InventoryOwner::new(scope.owner);
            let category_id = FolderId::new(read_uuid(row, 0)?);
            let parent_id = row.get::<_, Option<Vec<u8>>>(1)?.map(|bytes| read_uuid_bytes(bytes, 1).map(FolderId::new)).transpose()?;
            let preferred_type = FolderType::from_raw(row.get(2)?);
            let name: String = row.get(3)?;
            let server_version = ServerVersion::from_raw(read_i32(row, 4)?);
            let server_descendent_count = row.get::<_, Option<i64>>(5)?.map(|value| read_u32(value, 5)).transpose()?;
            let complete = read_bool(row, 6)?;
            let complete_generation: i64 = row.get(7)?;
            let category = Category {
                key: CategoryKey::new(owner, category_id),
                parent: parent_id.map(|id| CategoryKey::new(owner, id)),
                preferred_folder_type: preferred_type,
                name: Arc::from(name),
                server_version,
                server_descendent_count,
            };
            Ok(CategoryRow { scope_id: scope.id, category, complete, complete_generation })
        })?
        .collect()
}

fn read_items(transaction: &Transaction<'_>, scope: Scope) -> rusqlite::Result<Vec<Item>> {
    let mut statement = transaction.prepare(
        "SELECT id, parent_id, asset_id, asset_type, inventory_type, flags, name, description, created_at,
                creator_id, item_owner_id, group_id, last_owner_id, base_mask, owner_mask, group_mask, everyone_mask,
                next_owner_mask, sale_type, sale_price
         FROM items WHERE scope_id=?1 ORDER BY id",
    )?;
    statement
        .query_map([scope.id], |row| {
            let owner = InventoryOwner::new(scope.owner);
            let item_id = ItemId::new(read_uuid(row, 0)?);
            let parent_id = FolderId::new(read_uuid(row, 1)?);
            let asset_id = AssetId::new(read_uuid(row, 2)?);
            let asset_type = AssetType::from_raw(read_i32(row, 3)?);
            let inventory_type = InventoryType::from_raw(read_i32(row, 4)?);
            let flags = read_u32(row.get(5)?, 5)?;
            let name: String = row.get(6)?;
            let description: String = row.get(7)?;
            let created_at: i64 = row.get(8)?;
            let permissions = Permissions {
                creator: AvatarId::new(read_uuid(row, 9)?),
                owner: AvatarId::new(read_uuid(row, 10)?),
                group: GroupId::new(read_uuid(row, 11)?),
                last_owner: AvatarId::new(read_uuid(row, 12)?),
                base: PermissionMask::new(read_u32(row.get(13)?, 13)?),
                owner_mask: PermissionMask::new(read_u32(row.get(14)?, 14)?),
                group_mask: PermissionMask::new(read_u32(row.get(15)?, 15)?),
                everyone: PermissionMask::new(read_u32(row.get(16)?, 16)?),
                next_owner: PermissionMask::new(read_u32(row.get(17)?, 17)?),
            };
            let sale = SaleInfo { sale_type: SaleType::from_raw(read_i32(row, 18)?), price: read_i32(row, 19)? };
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

fn tree_is_valid(categories: &[CategoryRow], items: &[Item], scopes: &[Scope]) -> bool {
    let mut category_map = HashMap::new();
    let mut child_counts = HashMap::<CategoryKey, usize>::new();
    let mut category_counts = HashMap::<i64, usize>::new();
    let mut item_counts = HashMap::<i64, usize>::new();
    let mut root_keys = HashMap::<i64, HashSet<CategoryKey>>::new();
    for item in items {
        let Some(scope) = scopes.iter().find(|scope| scope.owner == item.key.owner().id()) else { return false };
        *item_counts.entry(scope.id).or_default() += 1;
    }
    for row in categories {
        let category = &row.category;
        if !scopes.iter().any(|scope| scope.id == row.scope_id && scope.owner == category.key.owner().id())
            || crate::state::validate_category(category).is_err()
        {
            return false;
        }
        if category_map.insert(category.key, category).is_some() {
            return false;
        }
        *category_counts.entry(row.scope_id).or_default() += 1;
        match category.parent {
            Some(parent) => {
                if parent.owner() != category.key.owner() {
                    return false;
                }
                *child_counts.entry(parent).or_default() += 1;
            }
            None => {
                root_keys.entry(row.scope_id).or_default().insert(category.key);
            }
        }
    }
    let generations = scopes.iter().map(|scope| (scope.id, scope.generation)).collect::<HashMap<_, _>>();
    if categories
        .iter()
        .any(|row| row.complete_generation < 0 || row.complete_generation > generations.get(&row.scope_id).copied().unwrap_or_default())
    {
        return false;
    }
    for scope in scopes {
        if scope.generation < 1 {
            return false;
        }
        let categories_present = category_counts.get(&scope.id).copied().unwrap_or_default() > 0;
        let items_present = item_counts.get(&scope.id).copied().unwrap_or_default() > 0;
        if categories_present {
            let Some(roots) = root_keys.get(&scope.id) else { return false };
            if roots.len() != 1 {
                return false;
            }
        } else if items_present {
            return false;
        }
    }
    for row in categories {
        if let Some(parent) = row.category.parent
            && !category_map.contains_key(&parent)
        {
            return false;
        }
    }
    for item in items {
        if !scopes.iter().any(|scope| scope.owner == item.key.owner().id()) {
            return false;
        }
        if crate::state::validate_item(item).is_err() || !category_map.contains_key(&item.parent) {
            return false;
        }
        *child_counts.entry(item.parent).or_default() += 1;
    }
    for row in categories {
        if row.complete && row.complete_generation == scopes.iter().find(|scope| scope.id == row.scope_id).map_or(0, |s| s.generation) {
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
    InventoryCacheLoad { folders: Vec::new().into(), discarded: false }
}

fn discarded() -> InventoryCacheLoad {
    InventoryCacheLoad { folders: Vec::new().into(), discarded: true }
}
