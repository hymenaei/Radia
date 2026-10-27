use std::{
    collections::HashSet,
    env,
    error::Error,
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::Arc,
    sync::mpsc,
};

use futures::executor::block_on;
use rusqlite::{Connection, Transaction};
use storage::sqlite::{MAX_QUEUED_BATCHES, OpenStatus, RecoveryReason, SqliteBatch, SqliteError};
use tempfile::tempdir;
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

use super::{MAX_RESTORE_ROWS_PER_TURN, MAX_RESTORE_UNITS_PER_TURN};

use crate::{
    AssetType, Category, CategoryKey, FolderType, Inventory, InventoryCache, InventoryCacheError, InventoryCacheLoad, InventoryCacheWrite,
    InventoryChild, InventoryLimits, InventoryOwner, InventorySnapshot, InventoryType, Item, ItemKey, PermissionMask, Permissions,
    SaleInfo, SaleType, ServerVersion,
};

trait RestoreAll {
    fn apply_to(
        self,
        inventory: &mut Inventory,
        skeleton: &InventorySnapshot,
        excluded: &HashSet<CategoryKey>,
    ) -> Result<usize, InventoryCacheError>;
}

impl RestoreAll for InventoryCacheLoad {
    fn apply_to(
        self,
        inventory: &mut Inventory,
        skeleton: &InventorySnapshot,
        excluded: &HashSet<CategoryKey>,
    ) -> Result<usize, InventoryCacheError> {
        let mut restore = self.prepare_restore(skeleton)?;
        let mut units = 0usize;
        loop {
            let progress = restore.apply_next(inventory, excluded)?;
            units = units.saturating_add(progress.units_processed);
            if progress.finished {
                return Ok(units);
            }
        }
    }
}

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

struct LibraryFixture {
    inventory: Inventory,
    agent: Uuid,
    folder: CategoryKey,
    item: Item,
}

struct TwoFolderFixture {
    inventory: Inventory,
    agent: Uuid,
    first_folder: CategoryKey,
    first_item: Item,
    second_folder: CategoryKey,
    second_item: Item,
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

fn two_folder_fixture() -> Result<TwoFolderFixture, Box<dyn Error>> {
    let mut base = fixture(7)?;
    let root = base.inventory.roots().personal();
    let first = base.inventory.snapshot().category(base.folder).ok_or("missing first folder")?.clone();
    let second_folder = CategoryKey::new(base.folder.owner(), FolderId::new(Uuid::from_u128(5)));
    let second = Category {
        key: second_folder,
        parent: Some(root),
        preferred_folder_type: FolderType::Object,
        name: Arc::from("Second objects"),
        server_version: ServerVersion::from_raw(7),
        server_descendent_count: None,
    };
    base.inventory.apply_folder_fetch(
        root,
        ServerVersion::from_raw(1),
        Some(2),
        vec![InventoryChild::Category(first), InventoryChild::Category(second)],
    )?;
    let second_item = item(base.folder.owner(), base.agent, second_folder, 6, 7, "Second cached item", "Second cached description");
    Ok(TwoFolderFixture {
        inventory: base.inventory,
        agent: base.agent,
        first_folder: base.folder,
        first_item: base.item,
        second_folder,
        second_item,
    })
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

fn library_fixture(agent: Uuid, library_owner: Uuid, folder_version: i32, item_name: &str) -> Result<LibraryFixture, Box<dyn Error>> {
    let personal_owner = InventoryOwner::new(agent);
    let library = InventoryOwner::new(library_owner);
    let personal_root = Category {
        key: CategoryKey::new(personal_owner, FolderId::new(Uuid::from_u128(agent.as_u128() ^ 0x1000))),
        parent: None,
        preferred_folder_type: FolderType::RootInventory,
        name: Arc::from("Inventory"),
        server_version: ServerVersion::from_raw(1),
        server_descendent_count: None,
    };
    let library_root = Category {
        key: CategoryKey::new(library, FolderId::new(Uuid::from_u128(9_000))),
        parent: None,
        preferred_folder_type: FolderType::RootInventory,
        name: Arc::from("Library"),
        server_version: ServerVersion::from_raw(1),
        server_descendent_count: None,
    };
    let folder = CategoryKey::new(library, FolderId::new(Uuid::from_u128(9_001)));
    let library_folder = Category {
        key: folder,
        parent: Some(library_root.key),
        preferred_folder_type: FolderType::Object,
        name: Arc::from("Library Objects"),
        server_version: ServerVersion::from_raw(folder_version),
        server_descendent_count: None,
    };
    let mut inventory = Inventory::new(personal_root, Some(library_root.clone()), InventoryLimits::default())?;
    inventory.apply_folder_fetch(library_root.key, ServerVersion::from_raw(1), Some(1), vec![InventoryChild::Category(library_folder)])?;
    let object = item(library, library_owner, folder, 9_002, 9_003, item_name, "Shared library item");
    Ok(LibraryFixture { inventory, agent, folder, item: object })
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

fn seed_two_folder_cache(root: &std::path::Path) -> Result<(), Box<dyn Error>> {
    let mut fixture = two_folder_fixture()?;
    let (cache, _) = InventoryCache::open_at_root(root, "agni", fixture.agent, None)?;
    block_on(cache.submit_bootstrap(&fixture.inventory.snapshot())?.wait())?;
    persist_fetch(
        &cache,
        &mut fixture.inventory,
        fixture.first_folder,
        ServerVersion::from_raw(7),
        1,
        vec![InventoryChild::Item(fixture.first_item)],
    )?;
    persist_fetch(
        &cache,
        &mut fixture.inventory,
        fixture.second_folder,
        ServerVersion::from_raw(7),
        1,
        vec![InventoryChild::Item(fixture.second_item)],
    )?;
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

    assert_eq!(applied, 2);
    assert_eq!(current.inventory.snapshot().item(current.item.key), Some(&current.item));
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn mismatched_version_restores_children_incomplete_until_fetch_reconciles() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    seed_cache(directory.path(), 7)?;
    let mut current = fixture(8)?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", current.agent, None)?;
    let load = block_on(cache.load(&current.inventory))?;
    let skeleton = current.inventory.snapshot();

    let applied = load.apply_to(&mut current.inventory, &skeleton, &HashSet::new())?;

    assert_eq!(applied, 2);
    assert_eq!(current.inventory.snapshot().item(current.item.key), Some(&current.item));
    assert!(!current.inventory.snapshot().is_complete(current.folder));

    current.inventory.apply_folder_fetch(current.folder, ServerVersion::from_raw(8), Some(0), Vec::new())?;
    assert_eq!(current.inventory.snapshot().item(current.item.key), None);
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn completed_fetch_wins_while_other_cached_folders_restore() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    seed_two_folder_cache(directory.path())?;
    let mut current = two_folder_fixture()?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", current.agent, None)?;
    let load = block_on(cache.load(&current.inventory))?;
    let skeleton = current.inventory.snapshot();
    let mut restore = load.prepare_restore(&skeleton)?;

    let live_item = item(
        current.first_folder.owner(),
        current.agent,
        current.first_folder,
        current.first_item.key.id().uuid().as_u128(),
        current.first_item.asset_id.uuid().as_u128(),
        "Fetched item",
        "Authoritative fetch value",
    );
    let before = current.inventory.snapshot();
    let changes = current.inventory.apply_folder_fetch(
        current.first_folder,
        ServerVersion::from_raw(8),
        Some(1),
        vec![InventoryChild::Item(live_item.clone())],
    )?;
    let after = current.inventory.snapshot();
    let excluded = InventoryCache::affected_categories(&changes, &before, &after).into_iter().collect::<HashSet<_>>();
    assert!(excluded.contains(&current.first_folder));
    while !restore.apply_next(&mut current.inventory, &excluded)?.finished {}

    let snapshot = current.inventory.snapshot();
    assert_eq!(snapshot.item(live_item.key), Some(&live_item));
    assert_eq!(snapshot.item(current.second_item.key), Some(&current.second_item));
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn live_item_update_survives_an_interleaved_partial_restore() -> Result<(), Box<dyn Error>> {
    const ITEM_COUNT: usize = 700;
    let directory = tempdir()?;
    let mut seeded = fixture(7)?;
    let children = (0..ITEM_COUNT)
        .map(|index| {
            InventoryChild::Item(item(
                seeded.folder.owner(),
                seeded.agent,
                seeded.folder,
                1_000_000 + index as u128,
                2_000_000 + index as u128,
                "Cached item",
                "Cached value",
            ))
        })
        .collect::<Vec<_>>();
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", seeded.agent, None)?;
    block_on(cache.submit_bootstrap(&seeded.inventory.snapshot())?.wait())?;
    persist_fetch(&cache, &mut seeded.inventory, seeded.folder, ServerVersion::from_raw(7), ITEM_COUNT as u32, children)?;
    block_on(cache.close())?;

    let mut current = fixture(7)?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", current.agent, None)?;
    let load = block_on(cache.load(&current.inventory))?;
    let skeleton = current.inventory.snapshot();
    let mut restore = load.prepare_restore(&skeleton)?;
    let mut excluded = HashSet::new();
    let first_turn = restore.apply_next(&mut current.inventory, &excluded)?;
    assert_eq!(first_turn.rows_processed, MAX_RESTORE_ROWS_PER_TURN);
    assert!(!first_turn.finished);
    assert!(!current.inventory.snapshot().is_complete(current.folder));
    assert!(current.inventory.snapshot().item(ItemKey::new(current.folder.owner(), ItemId::new(Uuid::from_u128(1_000_000)))).is_some());

    let live_item = item(current.folder.owner(), current.agent, current.folder, 1_000_699, 2_000_699, "Live value", "Updated in session");
    let before = current.inventory.snapshot();
    let changes = current.inventory.upsert_item(live_item.clone())?;
    let after = current.inventory.snapshot();
    excluded.extend(InventoryCache::affected_categories(&changes, &before, &after));
    assert!(excluded.contains(&current.folder));
    while !restore.apply_next(&mut current.inventory, &excluded)?.finished {}

    let snapshot = current.inventory.snapshot();
    assert_eq!(snapshot.item(live_item.key), Some(&live_item));
    assert!(snapshot.item(ItemKey::new(current.folder.owner(), ItemId::new(Uuid::from_u128(1_000_600)))).is_none());
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn restore_units_are_bounded_per_turn() -> Result<(), Box<dyn Error>> {
    const FOLDER_COUNT: usize = MAX_RESTORE_UNITS_PER_TURN + 20;
    let directory = tempdir()?;
    let agent = Uuid::from_u128(801);
    let owner = InventoryOwner::new(agent);
    let root = Category {
        key: CategoryKey::new(owner, FolderId::new(Uuid::from_u128(800))),
        parent: None,
        preferred_folder_type: FolderType::RootInventory,
        name: Arc::from("Inventory"),
        server_version: ServerVersion::from_raw(1),
        server_descendent_count: Some(FOLDER_COUNT as u32),
    };
    let folders = (0..FOLDER_COUNT)
        .map(|index| Category {
            key: CategoryKey::new(owner, FolderId::new(Uuid::from_u128(10_000 + index as u128))),
            parent: Some(root.key),
            preferred_folder_type: FolderType::Object,
            name: Arc::from(format!("Folder {index}")),
            server_version: ServerVersion::from_raw(1),
            server_descendent_count: Some(0),
        })
        .collect::<Vec<_>>();
    let mut inventory = Inventory::from_categories(root, None, folders, InventoryLimits::default())?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", agent, None)?;
    block_on(cache.submit_bootstrap(&inventory.snapshot())?.wait())?;
    let load = block_on(cache.load(&inventory))?;
    let skeleton = inventory.snapshot();
    let mut restore = load.prepare_restore(&skeleton)?;

    let progress = restore.apply_next(&mut inventory, &HashSet::new())?;

    assert_eq!(progress.units_processed, MAX_RESTORE_UNITS_PER_TURN);
    assert!(progress.units_processed <= MAX_RESTORE_UNITS_PER_TURN);
    assert!(!progress.finished);
    assert!(progress.rows_processed <= MAX_RESTORE_ROWS_PER_TURN);
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn missing_skeleton_folders_are_preserved_incomplete_and_old_roots_are_dropped() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    seed_cache(directory.path(), 7)?;
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
    let uncached_folder = Category {
        key: CategoryKey::new(owner, FolderId::new(Uuid::from_u128(10))),
        parent: Some(root.key),
        preferred_folder_type: FolderType::Object,
        name: Arc::from("New skeleton folder"),
        server_version: ServerVersion::from_raw(2),
        server_descendent_count: None,
    };
    let old_cached_folder = CategoryKey::new(owner, FolderId::new(Uuid::from_u128(2)));
    let mut current = Inventory::from_categories(root.clone(), None, vec![uncached_folder.clone()], InventoryLimits::default())?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", agent, None)?;
    let load = block_on(cache.load(&current))?;
    assert!(!load.was_discarded());
    let skeleton = current.snapshot();
    load.apply_to(&mut current, &skeleton, &HashSet::new())?;
    assert!(current.snapshot().category(uncached_folder.key).is_some());
    assert!(!current.snapshot().is_complete(uncached_folder.key));
    block_on(cache.close())?;

    let changed_root = Category {
        key: CategoryKey::new(owner, FolderId::new(Uuid::from_u128(77))),
        parent: None,
        preferred_folder_type: FolderType::RootInventory,
        name: Arc::from("Inventory"),
        server_version: ServerVersion::from_raw(1),
        server_descendent_count: None,
    };
    let mut next_login = Inventory::new(changed_root.clone(), None, InventoryLimits::default())?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", agent, None)?;
    let load = block_on(cache.load(&next_login))?;
    assert!(!load.was_discarded());
    let skeleton = next_login.snapshot();
    assert_eq!(load.apply_to(&mut next_login, &skeleton, &HashSet::new())?, 0);
    assert!(next_login.snapshot().category(changed_root.key).is_some());
    assert_eq!(next_login.snapshot().category(root.key), None);
    assert!(!next_login.snapshot().is_complete(changed_root.key));
    assert!(next_login.snapshot().category(old_cached_folder).is_none());
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

    assert_eq!(load.apply_to(&mut current.inventory, &skeleton, &HashSet::new())?, 3);
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

        assert_eq!(load.apply_to(&mut current.inventory, &skeleton, &HashSet::new())?, 2);
        assert!(current.inventory.snapshot().is_complete(current.folder));
        assert_eq!(current.inventory.snapshot().item(current.item.key), Some(&current.item));
        block_on(bootstrap.wait())?;
        block_on(cache.close())?;
    }
    Ok(())
}

#[test]
fn incomplete_partial_write_stays_incomplete() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    seed_cache(directory.path(), 7)?;
    let fixture = fixture(7)?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", fixture.agent, None)?;
    let folder_id = fixture.folder.id().uuid().as_bytes().to_vec();
    let item_id = fixture.item.key.id().uuid().as_bytes().to_vec();
    let owner_id = fixture.folder.owner().id().as_bytes().to_vec();
    let invalidate = SqliteBatch::new(1, 64, move |transaction| {
        transaction.execute(
            "UPDATE categories SET complete=0
             WHERE scope_id=(SELECT scope_id FROM scopes WHERE grid_id='agni' AND owner_id=?1) AND id=?2",
            (&owner_id, &folder_id),
        )?;
        Ok(())
    })?;
    let owner_id = fixture.folder.owner().id().as_bytes().to_vec();
    let partial = SqliteBatch::new(1, 64, move |transaction: &Transaction<'_>| {
        transaction.execute(
            "UPDATE items SET name='Partial'
             WHERE scope_id=(SELECT scope_id FROM scopes WHERE grid_id='agni' AND owner_id=?1) AND id=?2",
            (&owner_id, &item_id),
        )?;
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

    assert_eq!(load.apply_to(&mut inventory, &skeleton, &HashSet::new())?, 2);
    assert!(!inventory.snapshot().is_complete(fixture.folder));
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
        transaction.execute("UPDATE scopes SET cache_generation=cache_generation WHERE grid_id='agni'", [])?;
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
    let failed_write = cache.submit_changeset(&changes, &before, &after)?;

    release.send(())?;
    block_on(blocker_receipt.wait())?;
    for receipt in queued {
        block_on(receipt.wait())?;
    }
    std::assert_matches!(block_on(failed_write.wait()), Err(InventoryCacheError::Sqlite(SqliteError::QueueFull)));
    block_on(cache.close())?;

    let mut baseline = fixture(7)?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", baseline.agent, None)?;
    let load = block_on(cache.load(&baseline.inventory))?;
    let skeleton = baseline.inventory.snapshot();
    assert_eq!(load.apply_to(&mut baseline.inventory, &skeleton, &HashSet::new())?, 2);
    assert!(!baseline.inventory.snapshot().is_complete(baseline.folder));
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn equal_agent_ids_on_different_grids_use_separate_scopes() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    seed_cache(directory.path(), 7)?;
    let mut agni = fixture(7)?;
    let (agni_cache, _) = InventoryCache::open_at_root(directory.path(), "agni", agni.agent, None)?;
    let agni_load = block_on(agni_cache.load(&agni.inventory))?;
    let agni_skeleton = agni.inventory.snapshot();
    agni_load.apply_to(&mut agni.inventory, &agni_skeleton, &HashSet::new())?;
    assert_eq!(agni.inventory.snapshot().item(agni.item.key), Some(&agni.item));

    let mut aditi = fixture(7)?;
    let (aditi_cache, _) = InventoryCache::open_at_root(directory.path(), "aditi", aditi.agent, None)?;
    assert_eq!(agni_cache.path(), aditi_cache.path());
    let aditi_load = block_on(aditi_cache.load(&aditi.inventory))?;
    let aditi_skeleton = aditi.inventory.snapshot();
    assert_eq!(aditi_load.apply_to(&mut aditi.inventory, &aditi_skeleton, &HashSet::new())?, 0);
    assert_eq!(aditi.inventory.snapshot().item(aditi.item.key), None);

    let mut updated = aditi.item.clone();
    updated.name = Arc::from("Aditi item");
    let before = aditi.inventory.snapshot();
    let changes = aditi.inventory.upsert_item(updated)?;
    let after = aditi.inventory.snapshot();
    block_on(aditi_cache.submit_changeset(&changes, &before, &after)?.wait())?;

    let (agni_again, _) = InventoryCache::open_at_root(directory.path(), "agni", agni.agent, None)?;
    let agni_load = block_on(agni_again.load(&agni.inventory))?;
    let mut reloaded = fixture(7)?;
    let reloaded_skeleton = reloaded.inventory.snapshot();
    agni_load.apply_to(&mut reloaded.inventory, &reloaded_skeleton, &HashSet::new())?;
    assert_eq!(reloaded.inventory.snapshot().item(reloaded.item.key), Some(&reloaded.item));
    block_on(agni_cache.close())?;
    block_on(aditi_cache.close())?;
    block_on(agni_again.close())?;
    Ok(())
}

#[test]
fn agents_share_library_scope_and_validate_each_login_skeleton() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let owner = Uuid::from_u128(900);
    let mut first = library_fixture(Uuid::from_u128(701), owner, 5, "Shared library item")?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", first.agent, Some(owner))?;
    block_on(cache.submit_bootstrap(&first.inventory.snapshot())?.wait())?;
    persist_fetch(
        &cache,
        &mut first.inventory,
        first.folder,
        ServerVersion::from_raw(5),
        1,
        vec![InventoryChild::Item(first.item.clone())],
    )?;
    block_on(cache.close())?;

    let mut same_skeleton = library_fixture(Uuid::from_u128(701), owner, 5, "Shared library item")?;
    let (same_cache, _) = InventoryCache::open_at_root(directory.path(), "agni", same_skeleton.agent, Some(owner))?;
    let same_load = block_on(same_cache.load(&same_skeleton.inventory))?;
    let same_login_skeleton = same_skeleton.inventory.snapshot();
    assert_eq!(same_load.apply_to(&mut same_skeleton.inventory, &same_login_skeleton, &HashSet::new())?, 3);
    assert!(same_skeleton.inventory.snapshot().is_complete(same_skeleton.folder));
    assert_eq!(same_skeleton.inventory.snapshot().item(same_skeleton.item.key), Some(&same_skeleton.item));

    let mut changed_skeleton = library_fixture(Uuid::from_u128(702), owner, 6, "Shared library item")?;
    let (changed_cache, _) = InventoryCache::open_at_root(directory.path(), "agni", changed_skeleton.agent, Some(owner))?;
    let changed_load = block_on(changed_cache.load(&changed_skeleton.inventory))?;
    let changed_login_skeleton = changed_skeleton.inventory.snapshot();
    changed_load.apply_to(&mut changed_skeleton.inventory, &changed_login_skeleton, &HashSet::new())?;
    let restored = changed_skeleton.inventory.snapshot();
    assert!(!restored.is_complete(changed_skeleton.folder));
    assert_eq!(restored.item(changed_skeleton.item.key), Some(&changed_skeleton.item));

    persist_fetch(&changed_cache, &mut changed_skeleton.inventory, changed_skeleton.folder, ServerVersion::from_raw(6), 0, Vec::new())?;
    assert_eq!(changed_skeleton.inventory.snapshot().item(changed_skeleton.item.key), None);
    block_on(same_cache.close())?;
    block_on(changed_cache.close())?;
    Ok(())
}

#[test]
fn schema_and_corrupt_database_are_recreated() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let agent = Uuid::from_u128(703);
    let path = directory.path().join("inventory.sqlite");
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", agent, None)?;
    block_on(cache.close())?;
    let connection = Connection::open(&path)?;
    connection.execute("ALTER TABLE scopes ADD COLUMN kind INTEGER", [])?;
    drop(connection);

    let (cache, status) = InventoryCache::open_at_root(directory.path(), "agni", agent, None)?;
    assert_eq!(status, OpenStatus::Recreated(RecoveryReason::SchemaMismatch));
    block_on(cache.close())?;
    std::fs::write(&path, b"not a SQLite database")?;

    let (cache, status) = InventoryCache::open_at_root(directory.path(), "agni", agent, None)?;
    assert_eq!(status, OpenStatus::Recreated(RecoveryReason::Corrupt));
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn old_per_grid_cache_files_are_removed_when_idle() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let old_grid = directory.path().join("inventory").join("a".repeat(64));
    std::fs::create_dir_all(&old_grid)?;
    let old_file = old_grid.join(format!("{}.sqlite", Uuid::from_u128(704)));
    std::fs::write(&old_file, b"old inventory cache")?;

    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", Uuid::from_u128(704), None)?;

    assert!(!old_file.exists());
    assert_eq!(cache.path(), directory.path().join("inventory.sqlite"));
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn scopes_table_uses_grid_and_owner_without_kind() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let grid_id = "Custom.Grid-01";
    let (cache, _) = InventoryCache::open_at_root(directory.path(), grid_id, Uuid::from_u128(709), None)?;
    let path = cache.path().to_owned();
    block_on(cache.close())?;
    let connection = Connection::open(path)?;
    let mut statement = connection.prepare("PRAGMA table_info(scopes)")?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?.collect::<Result<Vec<_>, _>>()?;
    assert_eq!(columns, ["scope_id", "grid_id", "owner_id", "cache_generation"]);
    assert!(!columns.iter().any(|column| column == "kind"));
    let stored_grid_id: String = connection.query_row("SELECT grid_id FROM scopes", [], |row| row.get(0))?;
    assert_eq!(stored_grid_id, grid_id);
    Ok(())
}

#[test]
fn cold_and_warm_restore_measurement_at_one_hundred_thousand_items() -> Result<(), Box<dyn Error>> {
    use std::time::Instant;

    const ITEM_COUNT: usize = 100_000;
    let directory = tempdir()?;
    let mut seed = fixture(7)?;
    let children = (0..ITEM_COUNT)
        .map(|index| {
            InventoryChild::Item(item(
                seed.folder.owner(),
                seed.agent,
                seed.folder,
                1_000_000 + index as u128,
                2_000_000 + index as u128,
                "Cached item",
                "Large inventory restore fixture",
            ))
        })
        .collect::<Vec<_>>();
    let mut cold = fixture(7)?;
    let cold_children = children.clone();
    let cold_started = Instant::now();
    cold.inventory.apply_folder_fetch(cold.folder, ServerVersion::from_raw(7), Some(ITEM_COUNT as u32), cold_children)?;
    let cold_elapsed = cold_started.elapsed();

    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", seed.agent, None)?;
    let before = seed.inventory.snapshot();
    block_on(cache.submit_bootstrap(&before)?.wait())?;
    let changes = seed.inventory.apply_folder_fetch(seed.folder, ServerVersion::from_raw(7), Some(ITEM_COUNT as u32), children)?;
    let after = seed.inventory.snapshot();
    block_on(cache.submit_changeset(&changes, &before, &after)?.wait())?;
    let path = cache.path().to_owned();
    block_on(cache.close())?;
    let database_bytes = std::fs::metadata(&path)?.len();

    let mut warm = fixture(7)?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", warm.agent, None)?;
    let root = warm.inventory.roots().personal();
    let target_folder = warm.inventory.snapshot().category(warm.folder).ok_or("missing target folder")?.clone();
    let fetch_folder = CategoryKey::new(warm.folder.owner(), FolderId::new(Uuid::from_u128(7_000_000)));
    let fetch_category = Category {
        key: fetch_folder,
        parent: Some(root),
        preferred_folder_type: FolderType::Object,
        name: Arc::from("Fetch during restore"),
        server_version: ServerVersion::from_raw(8),
        server_descendent_count: None,
    };
    warm.inventory.apply_folder_fetch(
        root,
        ServerVersion::from_raw(1),
        Some(2),
        vec![InventoryChild::Category(target_folder), InventoryChild::Category(fetch_category)],
    )?;
    let warm_started = Instant::now();
    let load = block_on(cache.load(&warm.inventory))?;
    let skeleton = warm.inventory.snapshot();
    let mut restore = load.prepare_restore(&skeleton)?;
    let mut excluded = HashSet::new();
    let mut first_visible = None;
    let mut turns = 0usize;
    let mut restored = 0usize;
    loop {
        let progress = restore.apply_next(&mut warm.inventory, &excluded)?;
        restored = restored.saturating_add(progress.units_applied);
        turns = turns.saturating_add(1);
        if first_visible.is_none() && warm.inventory.snapshot().child_count(warm.folder) > 0 {
            first_visible = Some(warm_started.elapsed());
        }
        let fetch_version = ServerVersion::from_raw(100 + i32::try_from(turns)?);
        warm.inventory.apply_folder_fetch(fetch_folder, fetch_version, None, Vec::new())?;
        excluded.insert(fetch_folder);
        if progress.finished {
            break;
        }
    }
    let warm_elapsed = warm_started.elapsed();
    assert_eq!(restored, 1);
    assert!(first_visible.is_some());
    assert!(warm.inventory.snapshot().is_complete(warm.folder));
    assert_eq!(warm.inventory.snapshot().children(warm.folder).count(), ITEM_COUNT);
    println!(
        "inventory_cache_100k cold_model_install_ms={} time_to_first_restored_folder_ms={} total_restore_with_interleaved_fetch_ms={} turns={} database_bytes={}",
        cold_elapsed.as_millis(),
        first_visible.ok_or("first restored folder never became visible")?.as_millis(),
        warm_elapsed.as_millis(),
        turns,
        database_bytes
    );
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn interrupted_batch_keeps_cached_folder_incomplete() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    seed_cache(directory.path(), 7)?;
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("cache::tests::inventory_cache_process_helper")
        .arg("--nocapture")
        .env("RADIA_INVENTORY_CACHE_PROCESS_PATH", directory.path())
        .env("RADIA_INVENTORY_CACHE_PROCESS_MODE", "crash")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("child stdout missing")?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let mut output = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            let status = child.wait()?;
            return Err(format!("cache crash helper exited before opening its transaction: {status}; output: {output}").into());
        }
        if line.trim() == "transaction-open" {
            break;
        }
        output.push_str(&line);
    }
    child.kill()?;
    let _ = child.wait()?;

    let mut current = fixture(7)?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", current.agent, None)?;
    let load = block_on(cache.load(&current.inventory))?;
    let current_skeleton = current.inventory.snapshot();
    load.apply_to(&mut current.inventory, &current_skeleton, &HashSet::new())?;
    let snapshot = current.inventory.snapshot();
    assert!(!snapshot.is_complete(current.folder));
    assert_eq!(snapshot.item(current.item.key), Some(&current.item));
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn two_processes_write_shared_library_scope() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let owner = Uuid::from_u128(900);
    let first = library_fixture(Uuid::from_u128(705), owner, 5, "Shared library item")?;
    let (seed, _) = InventoryCache::open_at_root(directory.path(), "agni", first.agent, Some(owner))?;
    block_on(seed.submit_bootstrap(&first.inventory.snapshot())?.wait())?;
    let mut seed_inventory = first.inventory.clone();
    persist_fetch(&seed, &mut seed_inventory, first.folder, ServerVersion::from_raw(5), 1, vec![InventoryChild::Item(first.item)])?;
    block_on(seed.close())?;

    let mut parent = library_fixture(Uuid::from_u128(706), owner, 5, "Shared library item")?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", parent.agent, Some(owner))?;
    let before = parent.inventory.snapshot();
    let changes = parent.inventory.apply_folder_fetch(
        parent.folder,
        ServerVersion::from_raw(5),
        Some(1),
        vec![InventoryChild::Item(parent.item.clone())],
    )?;
    let after = parent.inventory.snapshot();

    let mut child = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("cache::tests::inventory_cache_process_helper")
        .arg("--nocapture")
        .env("RADIA_INVENTORY_CACHE_PROCESS_PATH", directory.path())
        .env("RADIA_INVENTORY_CACHE_PROCESS_MODE", "write")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("child stdout missing")?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let mut output = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            let status = child.wait()?;
            return Err(format!("cache write helper exited before readiness: {status}; output: {output}").into());
        }
        if line.trim() == "write-lock-held" {
            break;
        }
        output.push_str(&line);
    }

    let write = cache.submit_changeset(&changes, &before, &after)?;
    let InventoryCacheWrite { result, control, identity, owners } = write;
    let busy = match result {
        Ok(receipt) => block_on(receipt.wait()),
        Err(error) => Err(error),
    };
    std::assert_matches!(busy, Err(SqliteError::Busy));
    child.stdin.take().ok_or("child stdin missing")?.write_all(b"go\n")?;
    assert!(child.wait()?.success());

    let failed_write = InventoryCacheWrite { result: Err(SqliteError::Busy), control, identity, owners };
    std::assert_matches!(block_on(failed_write.wait()), Err(InventoryCacheError::Sqlite(SqliteError::Busy)));
    block_on(cache.close())?;

    let mut next_login = library_fixture(Uuid::from_u128(707), owner, 5, "Shared library item")?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", next_login.agent, Some(owner))?;
    let load = block_on(cache.load(&next_login.inventory))?;
    let next_skeleton = next_login.inventory.snapshot();
    load.apply_to(&mut next_login.inventory, &next_skeleton, &HashSet::new())?;
    assert!(!next_login.inventory.snapshot().is_complete(next_login.folder));
    assert_eq!(next_login.inventory.snapshot().item(next_login.item.key), Some(&next_login.item));

    persist_fetch(
        &cache,
        &mut next_login.inventory,
        next_login.folder,
        ServerVersion::from_raw(5),
        1,
        vec![InventoryChild::Item(next_login.item.clone())],
    )?;
    block_on(cache.close())?;

    let mut final_login = library_fixture(Uuid::from_u128(709), owner, 5, "Shared library item")?;
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", final_login.agent, Some(owner))?;
    let load = block_on(cache.load(&final_login.inventory))?;
    let final_skeleton = final_login.inventory.snapshot();
    load.apply_to(&mut final_login.inventory, &final_skeleton, &HashSet::new())?;
    assert!(final_login.inventory.snapshot().is_complete(final_login.folder));
    assert_eq!(final_login.inventory.snapshot().item(final_login.item.key), Some(&final_login.item));
    block_on(cache.close())?;
    Ok(())
}

#[test]
fn inventory_cache_process_helper() -> Result<(), Box<dyn Error>> {
    let Some(path) = env::var_os("RADIA_INVENTORY_CACHE_PROCESS_PATH") else { return Ok(()) };
    let mode = env::var("RADIA_INVENTORY_CACHE_PROCESS_MODE")?;
    let agent = if mode == "write" { Uuid::from_u128(708) } else { Uuid::from_u128(704) };
    let owner = Uuid::from_u128(900);
    if mode == "crash" {
        let current = fixture(7)?;
        let (cache, _) = InventoryCache::open_at_root(path, "agni", current.agent, None)?;
        let owner_id = current.folder.owner().id().as_bytes().to_vec();
        let folder_id = current.folder.id().uuid().as_bytes().to_vec();
        let item_id = current.item.key.id().uuid().as_bytes().to_vec();
        let invalidate = SqliteBatch::new(1, 64, move |transaction| {
            transaction.execute(
                "UPDATE categories SET complete=0
                 WHERE scope_id=(SELECT scope_id FROM scopes WHERE grid_id='agni' AND owner_id=?1) AND id=?2",
                (&owner_id, &folder_id),
            )?;
            Ok(())
        })?;
        let owner_id = current.folder.owner().id().as_bytes().to_vec();
        let partial = SqliteBatch::new(1, 64, move |transaction: &Transaction<'_>| {
            transaction.execute(
                "UPDATE items SET name='Uncommitted'
                 WHERE scope_id=(SELECT scope_id FROM scopes WHERE grid_id='agni' AND owner_id=?1) AND id=?2",
                (&owner_id, &item_id),
            )?;
            let mut output = std::io::stdout();
            writeln!(output, "transaction-open").map_err(|_| rusqlite::Error::InvalidQuery)?;
            output.flush().map_err(|_| rusqlite::Error::InvalidQuery)?;
            let mut input = String::new();
            std::io::stdin().read_line(&mut input).map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok(())
        })?;
        let receipt = cache.db.submit_batches(vec![invalidate, partial])?;
        let _ = block_on(receipt.wait());
        return Ok(());
    }

    let fixture = library_fixture(agent, owner, 5, "Shared library item")?;
    let (cache, _) = InventoryCache::open_at_root(path, "agni", fixture.agent, Some(owner))?;
    let owner_id = owner.as_bytes().to_vec();
    let item_id = fixture.item.key.id().uuid().as_bytes().to_vec();
    let batch = SqliteBatch::new(1, 64, move |transaction: &Transaction<'_>| {
        transaction.execute(
            "UPDATE items SET name='Shared library item'
             WHERE scope_id=(SELECT scope_id FROM scopes WHERE grid_id='agni' AND owner_id=?1) AND id=?2",
            (&owner_id, &item_id),
        )?;
        let mut output = std::io::stdout();
        writeln!(output, "write-lock-held").map_err(|_| rusqlite::Error::InvalidQuery)?;
        output.flush().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).map_err(|_| rusqlite::Error::InvalidQuery)?;
        Ok(())
    })?;
    let receipt = cache.db.submit_batches(vec![batch])?;
    block_on(receipt.wait())?;
    block_on(cache.close())?;
    Ok(())
}
