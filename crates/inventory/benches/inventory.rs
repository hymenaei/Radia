use std::{
    hint::black_box,
    mem::size_of,
    sync::Arc,
    time::{Duration, Instant},
};

use ::inventory as inventory_model;
use criterion::{BatchSize, Criterion};
use futures::executor::block_on;
use inventory_model::{
    AssetType, Category, CategoryKey, Inventory, InventoryCache, InventoryChild, InventoryDelta, InventoryLimits, InventoryOwner,
    InventoryType, Item, ItemKey, PermissionMask, Permissions, SaleInfo, SaleType, ServerVersion,
};
use tempfile::tempdir;
use test_support::allocation;
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

const ENTRY_COUNT: usize = 100_000;
const OWNER_ID: u128 = 77;
const ROOT_ID: u128 = 1;

pub(super) fn bench(criterion: &mut Criterion) {
    report_memory();
    bench_cache(criterion);
    criterion.bench_function("inventory/build_100k_items_and_snapshot", |bench| {
        bench.iter_batched(
            item_children,
            |children| {
                let mut inventory = root_inventory();
                let changes = inventory
                    .apply_folder_fetch(inventory.roots().personal(), ServerVersion::from_raw(1), Some(ENTRY_COUNT as u32), children)
                    .expect("benchmark fixture should apply");
                let snapshot = inventory.snapshot();
                black_box((inventory, changes, snapshot))
            },
            BatchSize::LargeInput,
        );
    });

    let inventory = item_inventory();
    criterion.bench_function("inventory/snapshot_100k_items", |bench| bench.iter(|| black_box(inventory.snapshot())));

    let (mut inventory, delta_a, delta_b) = one_item_deltas();
    let mut use_a = false;
    criterion.bench_function("inventory/apply_one_item_delta_100k", |bench| {
        bench.iter_batched(
            || {
                use_a = !use_a;
                if use_a { delta_a.clone() } else { delta_b.clone() }
            },
            |delta| black_box(inventory.apply_delta(delta).expect("single item delta should apply")),
            BatchSize::SmallInput,
        );
    });
    report_delta_allocations();
}

fn bench_cache(criterion: &mut Criterion) {
    let directory = tempdir().expect("benchmark cache directory");
    let path = populate_cache(directory.path());
    report_cache_load_times(directory.path(), &path);
    let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", owner().id(), None).expect("benchmark cache open");
    criterion.bench_function("inventory/cache_warm_read_apply_100k", |bench| {
        bench.iter(|| {
            let mut inventory = cache_skeleton();
            let skeleton = inventory.snapshot();
            let load = block_on(cache.load(&inventory)).expect("benchmark cache read");
            let applied = load.apply_to(&mut inventory, &skeleton, &std::collections::HashSet::new()).expect("benchmark cache apply");
            black_box((inventory, applied));
        });
    });
    block_on(cache.close()).expect("benchmark cache close");
    black_box(path);
}

fn populate_cache(root: &std::path::Path) -> std::path::PathBuf {
    let mut inventory = root_inventory();
    let before = inventory.snapshot();
    let changes = inventory
        .apply_folder_fetch(inventory.roots().personal(), ServerVersion::from_raw(1), Some(ENTRY_COUNT as u32), item_children())
        .expect("benchmark cache fixture should apply");
    let after = inventory.snapshot();
    let (cache, _) = InventoryCache::open_at_root(root, "agni", owner().id(), None).expect("benchmark cache create");
    block_on(cache.submit_bootstrap(&before).expect("benchmark cache bootstrap").wait()).expect("benchmark bootstrap write");
    block_on(cache.submit_changeset(&changes, &before, &after).expect("benchmark cache batch").wait()).expect("benchmark cache write");
    let path = cache.path().to_path_buf();
    block_on(cache.close()).expect("benchmark initial cache close");
    path
}

fn report_cache_load_times(root: &std::path::Path, path: &std::path::Path) {
    let (cache, _) = InventoryCache::open_at_root(root, "agni", owner().id(), None).expect("benchmark first cache open");
    let mut cold_inventory = cache_skeleton();
    let cold_skeleton = cold_inventory.snapshot();
    let started = Instant::now();
    let cold_load = block_on(cache.load(&cold_inventory)).expect("benchmark first cache read");
    let cold_applied =
        cold_load.apply_to(&mut cold_inventory, &cold_skeleton, &std::collections::HashSet::new()).expect("benchmark first cache apply");
    let cold = started.elapsed();
    block_on(cache.close()).expect("benchmark first cache close");

    let (cache, _) = InventoryCache::open_at_root(root, "agni", owner().id(), None).expect("benchmark warm cache open");
    let mut warm_times = Vec::new();
    for _ in 0..5 {
        let mut inventory = cache_skeleton();
        let skeleton = inventory.snapshot();
        let started = Instant::now();
        let load = block_on(cache.load(&inventory)).expect("benchmark warm cache read");
        let applied = load.apply_to(&mut inventory, &skeleton, &std::collections::HashSet::new()).expect("benchmark warm cache apply");
        assert_eq!(applied, 1);
        warm_times.push(started.elapsed());
    }
    warm_times.sort_unstable();
    let warm = warm_times[warm_times.len() / 2];
    block_on(cache.close()).expect("benchmark warm cache close");
    eprintln!(
        "inventory cache at {ENTRY_COUNT} items: file={} bytes, first read/apply={} ms ({cold_applied} folders), warm median read/apply={} ms",
        std::fs::metadata(path).expect("benchmark database metadata").len(),
        duration_millis(cold),
        duration_millis(warm),
    );
}

fn duration_millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn cache_skeleton() -> Inventory {
    let root = Category {
        key: category_key(ROOT_ID),
        parent: None,
        preferred_folder_type: inventory_model::FolderType::RootInventory,
        name: Arc::from("root"),
        server_version: ServerVersion::from_raw(1),
        server_descendent_count: Some(ENTRY_COUNT as u32),
    };
    Inventory::new(root, None, InventoryLimits::default()).expect("benchmark skeleton should be valid")
}

fn report_memory() {
    let (root, root_memory) = allocation::measure(root_inventory);
    let (mut items, item_memory) = allocation::measure(item_inventory);
    let (folders, folder_memory) = allocation::measure(folder_inventory);
    let (snapshot, snapshot_memory) = allocation::measure(|| items.snapshot());
    let (changed_snapshot, changed_memory) = allocation::measure(|| {
        let mut changed_item = item(0);
        changed_item.name = Arc::from("updated item");
        let changes = items.upsert_item(changed_item).expect("single item update should apply");
        drop(changes);
        items.snapshot()
    });
    let item_bytes = item_memory.live_bytes.saturating_sub(root_memory.live_bytes) / ENTRY_COUNT as i128;
    let folder_bytes = folder_memory.live_bytes.saturating_sub(root_memory.live_bytes) / ENTRY_COUNT as i128;

    eprintln!(
        "inventory memory at {ENTRY_COUNT} entries: item model={} B ({item_bytes} B/item), folder model={} B ({folder_bytes} B/folder), snapshot delta={} B, single item change with prior snapshot={} B, Item={} B, Category={} B",
        item_memory.live_bytes,
        folder_memory.live_bytes,
        snapshot_memory.live_bytes,
        changed_memory.live_bytes,
        size_of::<Item>(),
        size_of::<Category>(),
    );
    black_box((root, items, folders, snapshot, changed_snapshot));
}

fn report_delta_allocations() {
    let (mut inventory, delta_a, _) = one_item_deltas();
    let (changes, allocations) = allocation::measure(|| inventory.apply_delta(delta_a).expect("single item delta should apply"));
    eprintln!(
        "inventory apply_one_item_delta_100k allocations: alloc_calls={}, realloc_calls={}, live={} B, peak={} B",
        allocations.allocation_calls, allocations.reallocation_calls, allocations.live_bytes, allocations.peak_bytes,
    );
    black_box((inventory, changes));
}

fn one_item_deltas() -> (Inventory, InventoryDelta, InventoryDelta) {
    let inventory = item_inventory();
    let original = inventory.snapshot().item(item(0).key).expect("benchmark item should exist").clone();
    let mut changed_a = original.clone();
    changed_a.name = Arc::from("delta-a");
    let mut changed_b = original;
    changed_b.name = Arc::from("delta-b");
    (
        inventory,
        InventoryDelta { updated_items: vec![changed_a], ..InventoryDelta::default() },
        InventoryDelta { updated_items: vec![changed_b], ..InventoryDelta::default() },
    )
}

fn root_inventory() -> Inventory {
    let root = Category {
        key: category_key(ROOT_ID),
        parent: None,
        preferred_folder_type: inventory_model::FolderType::RootInventory,
        name: Arc::from("root"),
        server_version: ServerVersion::UNKNOWN,
        server_descendent_count: None,
    };
    Inventory::new(root, None, InventoryLimits::default()).expect("benchmark root should be valid")
}

fn item_inventory() -> Inventory {
    let mut inventory = root_inventory();
    let changes = inventory
        .apply_folder_fetch(inventory.roots().personal(), ServerVersion::from_raw(1), Some(ENTRY_COUNT as u32), item_children())
        .expect("benchmark items should apply");
    drop(changes);
    inventory
}

fn folder_inventory() -> Inventory {
    let mut inventory = root_inventory();
    let changes = inventory
        .apply_folder_fetch(inventory.roots().personal(), ServerVersion::from_raw(1), Some(ENTRY_COUNT as u32), folder_children())
        .expect("benchmark folders should apply");
    drop(changes);
    inventory
}

fn item_children() -> Vec<InventoryChild> {
    (0..ENTRY_COUNT).map(|index| InventoryChild::Item(item(index))).collect()
}

fn folder_children() -> Vec<InventoryChild> {
    (0..ENTRY_COUNT)
        .map(|index| {
            InventoryChild::Category(Category {
                key: category_key(index as u128 + 2),
                parent: Some(category_key(ROOT_ID)),
                preferred_folder_type: inventory_model::FolderType::None,
                name: Arc::from(format!("folder-{index}")),
                server_version: ServerVersion::UNKNOWN,
                server_descendent_count: None,
            })
        })
        .collect()
}

fn item(index: usize) -> Item {
    Item {
        key: ItemKey::new(owner(), ItemId::new(uuid(index as u128 + 2))),
        parent: category_key(ROOT_ID),
        asset_id: AssetId::new(uuid(index as u128 + ENTRY_COUNT as u128 + 2)),
        asset_type: AssetType::Notecard,
        inventory_type: InventoryType::Notecard,
        flags: 0,
        name: Arc::from(format!("item-{index}")),
        description: Arc::from(""),
        created_at: 0,
        permissions: Permissions {
            creator: AvatarId::new(Uuid::nil()),
            owner: AvatarId::new(Uuid::nil()),
            group: GroupId::new(Uuid::nil()),
            last_owner: AvatarId::new(Uuid::nil()),
            base: PermissionMask::new(0),
            owner_mask: PermissionMask::new(0),
            group_mask: PermissionMask::new(0),
            everyone: PermissionMask::new(0),
            next_owner: PermissionMask::new(0),
        },
        sale: SaleInfo { sale_type: SaleType::NotForSale, price: 0 },
    }
}

fn owner() -> InventoryOwner {
    InventoryOwner::new(uuid(OWNER_ID))
}

fn category_key(id: u128) -> CategoryKey {
    CategoryKey::new(owner(), FolderId::new(uuid(id)))
}

fn uuid(id: u128) -> Uuid {
    Uuid::from_u128(id)
}
