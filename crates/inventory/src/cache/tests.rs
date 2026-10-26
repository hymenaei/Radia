use std::{collections::HashSet, error::Error, sync::Arc, sync::mpsc};

use futures::executor::block_on;
use rusqlite::Transaction;
use storage::sqlite::{MAX_QUEUED_BATCHES, SqliteBatch, SqliteError};
use tempfile::tempdir;
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

use crate::{
    AssetType, Category, CategoryKey, FolderType, Inventory, InventoryCache, InventoryCacheError, InventoryChild, InventoryLimits,
    InventoryOwner, InventoryType, Item, ItemKey, PermissionMask, Permissions, SaleInfo, SaleType, ServerVersion,
};

struct Fixture {
    inventory: Inventory,
    agent: Uuid,
    folder: CategoryKey,
    item: Item,
}

fn fixture(folder_version: i32) -> Result<Fixture, Box<dyn Error>> {
    let agent = Uuid::from_u128(100);
    let owner = InventoryOwner::new(agent);
    let root = Category {
        key: CategoryKey::new(owner, FolderId::new(Uuid::from_u128(1))),
        parent: None,
        preferred_folder_type: FolderType::RootInventory,
        name: Arc::from("Inventory"),
        server_version: ServerVersion::from_raw(1),
        server_descendent_count: Some(1),
    };
    let folder = CategoryKey::new(owner, FolderId::new(Uuid::from_u128(2)));
    let child_folder = Category {
        key: folder,
        parent: Some(root.key),
        preferred_folder_type: FolderType::Object,
        name: Arc::from("Objects"),
        server_version: ServerVersion::from_raw(folder_version),
        server_descendent_count: Some(1),
    };
    let item = Item {
        key: ItemKey::new(owner, ItemId::new(Uuid::from_u128(3))),
        parent: folder,
        asset_id: AssetId::new(Uuid::from_u128(4)),
        asset_type: AssetType::Object,
        inventory_type: InventoryType::Object,
        flags: 0,
        name: Arc::from("Object"),
        description: Arc::from("Cached object"),
        created_at: 1,
        permissions: Permissions {
            creator: AvatarId::new(agent),
            owner: AvatarId::new(agent),
            group: GroupId::new(Uuid::from_u128(5)),
            last_owner: AvatarId::new(agent),
            base: PermissionMask::new(u32::MAX),
            owner_mask: PermissionMask::new(u32::MAX),
            group_mask: PermissionMask::new(0),
            everyone: PermissionMask::new(0),
            next_owner: PermissionMask::new(u32::MAX),
        },
        sale: SaleInfo { sale_type: SaleType::NotForSale, price: 0 },
    };
    let mut inventory = Inventory::new(root.clone(), None, InventoryLimits::default())?;
    inventory.apply_folder_fetch(
        root.key,
        root.server_version,
        root.server_descendent_count,
        vec![InventoryChild::Category(child_folder)],
    )?;
    Ok(Fixture { inventory, agent, folder, item })
}

fn seed_cache(root: &std::path::Path, folder_version: i32) -> Result<(), Box<dyn Error>> {
    let mut fixture = fixture(folder_version)?;
    let (cache, _) = InventoryCache::open_at_root(root, "agni", fixture.agent, None)?;
    block_on(cache.submit_bootstrap(&fixture.inventory.snapshot())?.wait())?;
    let before = fixture.inventory.snapshot();
    let changes = fixture.inventory.apply_folder_fetch(
        fixture.folder,
        ServerVersion::from_raw(folder_version),
        Some(1),
        vec![InventoryChild::Item(fixture.item)],
    )?;
    let after = fixture.inventory.snapshot();
    block_on(cache.submit_changeset(&changes, &before, &after)?.wait())?;
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn matching_version_reuses_folder() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    seed_cache(directory.path(), 7)?;
    let mut current = fixture(7)?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", current.agent, None)?;
    let load = block_on(cache.load(&current.inventory))?;
    let skeleton = current.inventory.snapshot();

    let applied = load.apply_to(&mut current.inventory, &skeleton, &HashSet::new())?;

    assert_eq!(applied, 1);
    assert_eq!(current.inventory.snapshot().item(current.item.key), Some(&current.item));
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn mismatched_version_misses_folder() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    seed_cache(directory.path(), 7)?;
    let mut current = fixture(8)?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", current.agent, None)?;
    let load = block_on(cache.load(&current.inventory))?;
    let skeleton = current.inventory.snapshot();

    let applied = load.apply_to(&mut current.inventory, &skeleton, &HashSet::new())?;

    assert_eq!(applied, 0);
    assert_eq!(current.inventory.snapshot().item(current.item.key), None);
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn worker_stops_before_completion_marker() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    seed_cache(directory.path(), 7)?;
    let fixture = fixture(7)?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", fixture.agent, None)?;
    let folder_id = fixture.folder.id().uuid().as_bytes().to_vec();
    let owner_id = fixture.folder.owner().id().as_bytes().to_vec();
    let invalidate = SqliteBatch::new(1, 64, move |transaction| {
        transaction.execute("UPDATE categories SET complete=0 WHERE owner_id=?1 AND category_id=?2", (&owner_id, &folder_id))?;
        Ok(())
    })?;
    let partial = SqliteBatch::new(1, 64, |transaction: &Transaction<'_>| {
        transaction.execute("UPDATE items SET name='Partial'", [])?;
        Ok(())
    })?;
    let receipt = cache.db.submit_batches(vec![invalidate, partial])?;
    block_on(receipt.wait())?;
    let crate::cache::InventoryCache { db, .. } = cache;
    db.shutdown()?;

    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", fixture.agent, None)?;
    let mut inventory = fixture.inventory;
    let load = block_on(cache.load(&inventory))?;
    let skeleton = inventory.snapshot();

    assert_eq!(load.apply_to(&mut inventory, &skeleton, &HashSet::new())?, 0);
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn full_queue_invalidates_cached_completeness() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    seed_cache(directory.path(), 7)?;
    let mut current = fixture(7)?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", current.agent, None)?;
    let (started, started_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let blocker = SqliteBatch::new(1, 8, move |transaction| {
        transaction.execute("UPDATE meta SET clean_shutdown=0 WHERE id=1", [])?;
        started.send(()).map_err(|_| rusqlite::Error::InvalidQuery)?;
        release_rx.recv().map_err(|_| rusqlite::Error::InvalidQuery)?;
        Ok(())
    })?;
    let blocker_receipt = cache.db.submit_batches(vec![blocker])?;
    started_rx.recv()?;
    let mut queued = Vec::new();
    for _ in 1..MAX_QUEUED_BATCHES {
        queued.push(cache.db.submit_batches(vec![SqliteBatch::new(1, 8, |_| Ok(()))?])?);
    }

    let before = current.inventory.snapshot();
    let mut changed_item = current.item.clone();
    changed_item.name = Arc::from("Updated");
    let changes = current.inventory.upsert_item(changed_item)?;
    let after = current.inventory.snapshot();
    std::assert_matches!(cache.submit_changeset(&changes, &before, &after), Err(InventoryCacheError::Sqlite(SqliteError::QueueFull)));

    release.send(())?;
    block_on(blocker_receipt.wait())?;
    for receipt in queued {
        block_on(receipt.wait())?;
    }
    block_on(cache.close())?;

    let mut baseline = fixture(7)?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", baseline.agent, None)?;
    let load = block_on(cache.load(&baseline.inventory))?;
    let skeleton = baseline.inventory.snapshot();
    assert_eq!(load.apply_to(&mut baseline.inventory, &skeleton, &HashSet::new())?, 0);
    block_on(cache.close())?;
    Ok(())
}
