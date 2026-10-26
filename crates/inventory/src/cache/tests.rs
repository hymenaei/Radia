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

struct NestedFixture {
    inventory: Inventory,
    agent: Uuid,
    root: CategoryKey,
    parent: CategoryKey,
    child: CategoryKey,
    new_item: Item,
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
        server_descendent_count: None,
    };
    let folder = CategoryKey::new(owner, FolderId::new(Uuid::from_u128(2)));
    let child_folder = Category {
        key: folder,
        parent: Some(root.key),
        preferred_folder_type: FolderType::Object,
        name: Arc::from("Objects"),
        server_version: ServerVersion::from_raw(folder_version),
        server_descendent_count: None,
    };
    let item = item(owner, agent, folder, 3, 4, "Object", "Cached object");
    let mut inventory = Inventory::new(root.clone(), None, InventoryLimits::default())?;
    inventory.apply_folder_fetch(
        root.key,
        root.server_version,
        root.server_descendent_count,
        vec![InventoryChild::Category(child_folder)],
    )?;
    Ok(Fixture { inventory, agent, folder, item })
}

fn nested_fixture(parent_version: i32, child_version: i32) -> Result<NestedFixture, Box<dyn Error>> {
    let agent = Uuid::from_u128(200);
    let owner = InventoryOwner::new(agent);
    let root = Category {
        key: CategoryKey::new(owner, FolderId::new(Uuid::from_u128(20))),
        parent: None,
        preferred_folder_type: FolderType::RootInventory,
        name: Arc::from("Inventory"),
        server_version: ServerVersion::from_raw(1),
        server_descendent_count: None,
    };
    let parent = Category {
        key: CategoryKey::new(owner, FolderId::new(Uuid::from_u128(21))),
        parent: Some(root.key),
        preferred_folder_type: FolderType::Object,
        name: Arc::from("Parent"),
        server_version: ServerVersion::from_raw(parent_version),
        server_descendent_count: None,
    };
    let child = Category {
        key: CategoryKey::new(owner, FolderId::new(Uuid::from_u128(22))),
        parent: Some(parent.key),
        preferred_folder_type: FolderType::Clothing,
        name: Arc::from(format!("Child v{child_version}")),
        server_version: ServerVersion::from_raw(child_version),
        server_descendent_count: None,
    };
    let new_item = item(owner, agent, child.key, 24, 34, "New item", "Fetched child item");
    let inventory = Inventory::from_categories(root.clone(), None, vec![parent.clone(), child.clone()], InventoryLimits::default())?;
    Ok(NestedFixture { inventory, agent, root: root.key, parent: parent.key, child: child.key, new_item })
}

fn item(owner: InventoryOwner, agent: Uuid, parent: CategoryKey, item_id: u128, asset_id: u128, name: &str, description: &str) -> Item {
    Item {
        key: ItemKey::new(owner, ItemId::new(Uuid::from_u128(item_id))),
        parent,
        asset_id: AssetId::new(Uuid::from_u128(asset_id)),
        asset_type: AssetType::Object,
        inventory_type: InventoryType::Object,
        flags: 0,
        name: Arc::from(name),
        description: Arc::from(description),
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
    }
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

fn persist_fetch(
    cache: &InventoryCache,
    inventory: &mut Inventory,
    folder: CategoryKey,
    version: ServerVersion,
    descendent_count: u32,
    children: Vec<InventoryChild>,
) -> Result<(), Box<dyn Error>> {
    let before = inventory.snapshot();
    let changes = inventory.apply_folder_fetch(folder, version, Some(descendent_count), children)?;
    let after = inventory.snapshot();
    block_on(cache.submit_changeset(&changes, &before, &after)?.wait())?;
    Ok(())
}

fn seed_nested_cache(root: &std::path::Path, parent_version: i32, child_version: i32) -> Result<(), Box<dyn Error>> {
    let mut fixture = nested_fixture(parent_version, child_version)?;
    let (cache, _) = InventoryCache::open_at_root(root, "agni", fixture.agent, None)?;
    block_on(cache.submit_bootstrap(&fixture.inventory.snapshot())?.wait())?;
    let snapshot = fixture.inventory.snapshot();
    let parent = snapshot.category(fixture.parent).ok_or("missing parent folder")?.clone();
    let child = snapshot.category(fixture.child).ok_or("missing child folder")?.clone();
    persist_fetch(&cache, &mut fixture.inventory, fixture.root, ServerVersion::from_raw(1), 1, vec![InventoryChild::Category(parent)])?;
    persist_fetch(
        &cache,
        &mut fixture.inventory,
        fixture.parent,
        ServerVersion::from_raw(parent_version),
        1,
        vec![InventoryChild::Category(child)],
    )?;
    persist_fetch(&cache, &mut fixture.inventory, fixture.child, ServerVersion::from_raw(child_version), 0, Vec::new())?;
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
fn unchanged_parent_does_not_restore_a_changed_child() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    seed_nested_cache(directory.path(), 10, 20)?;
    let mut current = nested_fixture(10, 21)?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", current.agent, None)?;
    let load = block_on(cache.load(&current.inventory))?;
    let skeleton = current.inventory.snapshot();
    let bootstrap = cache.submit_bootstrap(&skeleton)?;

    assert_eq!(load.apply_to(&mut current.inventory, &skeleton, &HashSet::new())?, 2);
    let restored = current.inventory.snapshot();
    assert!(restored.is_complete(current.root));
    assert!(restored.is_complete(current.parent));
    assert!(!restored.is_complete(current.child));
    let child = restored.category(current.child).ok_or("missing restored child")?;
    assert_eq!(child.server_version, ServerVersion::from_raw(21));
    assert_eq!(child.server_descendent_count, None);
    assert_eq!(child.name.as_ref(), "Child v21");
    assert_eq!(restored.item(current.new_item.key), None);

    let before = current.inventory.snapshot();
    let changes = current.inventory.apply_folder_fetch(
        current.child,
        ServerVersion::from_raw(21),
        Some(1),
        vec![InventoryChild::Item(current.new_item.clone())],
    )?;
    let fetched = current.inventory.snapshot();
    assert!(fetched.is_complete(current.child));
    assert_eq!(fetched.item(current.new_item.key), Some(&current.new_item));
    block_on(bootstrap.wait())?;
    block_on(cache.submit_changeset(&changes, &before, &fetched)?.wait())?;
    block_on(cache.close())?;

    let mut next_login = nested_fixture(10, 21)?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", next_login.agent, None)?;
    let load = block_on(cache.load(&next_login.inventory))?;
    let skeleton = next_login.inventory.snapshot();
    assert_eq!(load.apply_to(&mut next_login.inventory, &skeleton, &HashSet::new())?, 3);
    let restored = next_login.inventory.snapshot();
    assert!(restored.is_complete(next_login.parent));
    assert!(restored.is_complete(next_login.child));
    assert_eq!(restored.item(next_login.new_item.key), Some(&next_login.new_item));
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn unchanged_parent_and_child_restore_as_complete() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    seed_nested_cache(directory.path(), 10, 20)?;
    let mut current = nested_fixture(10, 20)?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", current.agent, None)?;
    let load = block_on(cache.load(&current.inventory))?;
    let skeleton = current.inventory.snapshot();

    assert_eq!(load.apply_to(&mut current.inventory, &skeleton, &HashSet::new())?, 3);
    let restored = current.inventory.snapshot();
    assert!(restored.is_complete(current.root));
    assert!(restored.is_complete(current.parent));
    assert!(restored.is_complete(current.child));
    assert_eq!(restored.item(current.new_item.key), None);
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn bootstrap_preserves_matching_cache_completeness_across_logins() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    seed_cache(directory.path(), 7)?;

    for _ in 0..2 {
        let mut current = fixture(7)?;
        let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", current.agent, None)?;
        let load = block_on(cache.load(&current.inventory))?;
        let skeleton = current.inventory.snapshot();
        let bootstrap = cache.submit_bootstrap(&skeleton)?;

        assert_eq!(load.apply_to(&mut current.inventory, &skeleton, &HashSet::new())?, 1);
        assert!(current.inventory.snapshot().is_complete(current.folder));
        assert_eq!(current.inventory.snapshot().item(current.item.key), Some(&current.item));
        block_on(bootstrap.wait())?;
        block_on(cache.close())?;
    }
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
