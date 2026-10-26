use std::{
    hint::black_box,
    sync::Arc,
    time::{Duration, Instant},
};

use ::inventory::{
    AssetType, Category, CategoryKey, FolderType, Inventory, InventoryChild, InventoryLimits, InventoryOwner, InventoryType, Item, ItemKey,
    PermissionMask, Permissions, SaleInfo, SaleType, ServerVersion,
};
use criterion::Criterion;
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;
use viewer::inventory::browser::{self, MAX_SEARCH_RESULTS, SearchTypeFilter};

const SEARCH_ITEMS: usize = 100_000;
const ROW_ITEMS: usize = 10_000;
const OWNER_ID: u128 = 77;
const ROOT_ID: u128 = 1;
const ROW_FOLDER_ID: u128 = 2;
const SEARCH_QUERY: &str = "item-09";

pub(super) fn bench(c: &mut Criterion) {
    let search_snapshot = search_snapshot();
    let (result_count, search_distribution) = measure_search(&search_snapshot, 300);
    eprintln!(
        "inventory browser search over {SEARCH_ITEMS} items: median={} us, p99={} us (query `{SEARCH_QUERY}`, {result_count} results, truncated, result cap {MAX_SEARCH_RESULTS})",
        search_distribution.0.as_secs_f64() * 1_000_000.0,
        search_distribution.1.as_secs_f64() * 1_000_000.0,
    );
    c.bench_function("inventory_browser/search_100k_items", |bench| {
        bench.iter(|| {
            black_box(browser::search(&search_snapshot, SEARCH_QUERY, SearchTypeFilter::All, || false));
        });
    });

    let (row_snapshot, expanded) = row_snapshot();
    let (row_count, row_distribution) = measure_rows(&row_snapshot, &expanded, 300);
    eprintln!(
        "inventory browser row build expanding {ROW_ITEMS} children: median={} us, p99={} us ({} visible rows)",
        row_distribution.0.as_secs_f64() * 1_000_000.0,
        row_distribution.1.as_secs_f64() * 1_000_000.0,
        row_count,
    );
    c.bench_function("inventory_browser/build_rows_expanded_10k", |bench| {
        bench.iter(|| black_box(browser::build_visible_rows(&row_snapshot, row_snapshot.roots().personal(), &expanded)));
    });
}

fn measure_search(snapshot: &::inventory::InventorySnapshot, samples: usize) -> (usize, (Duration, Duration)) {
    let mut elapsed = Vec::with_capacity(samples);
    let mut result_count = 0;
    for _ in 0..samples {
        let start = Instant::now();
        let results = browser::search(snapshot, SEARCH_QUERY, SearchTypeFilter::All, || false).expect("search completes");
        elapsed.push(start.elapsed());
        result_count = results.items.len();
    }
    (result_count, percentiles(&mut elapsed))
}

fn measure_rows(
    snapshot: &::inventory::InventorySnapshot,
    expanded: &std::collections::HashSet<CategoryKey>,
    samples: usize,
) -> (usize, (Duration, Duration)) {
    let mut elapsed = Vec::with_capacity(samples);
    let mut row_count = 0;
    for _ in 0..samples {
        let start = Instant::now();
        let rows = browser::build_visible_rows(snapshot, snapshot.roots().personal(), expanded);
        elapsed.push(start.elapsed());
        row_count = rows.rows.len();
    }
    (row_count, percentiles(&mut elapsed))
}

fn percentiles(samples: &mut [Duration]) -> (Duration, Duration) {
    samples.sort_unstable();
    let median = samples[(samples.len() - 1) / 2];
    let p99_index = samples.len().saturating_mul(99).div_ceil(100).saturating_sub(1);
    (median, samples[p99_index])
}

fn search_snapshot() -> ::inventory::InventorySnapshot {
    let mut inventory = inventory();
    let children = (0..SEARCH_ITEMS).map(|index| InventoryChild::Item(item(index, ROOT_ID))).collect();
    inventory
        .apply_folder_fetch(inventory.roots().personal(), ServerVersion::from_raw(1), Some(SEARCH_ITEMS as u32), children)
        .expect("search fixture applies");
    inventory.snapshot()
}

fn row_snapshot() -> (::inventory::InventorySnapshot, std::collections::HashSet<CategoryKey>) {
    let mut inventory = inventory();
    let personal = inventory.roots().personal();
    let folder = Category {
        key: category_key(ROW_FOLDER_ID),
        parent: Some(personal),
        preferred_folder_type: FolderType::None,
        name: Arc::from("Large folder"),
        server_version: ServerVersion::UNKNOWN,
        server_descendent_count: None,
    };
    inventory.upsert_category(folder).expect("row fixture folder applies");
    let children = (0..ROW_ITEMS).map(|index| InventoryChild::Item(item(index, ROW_FOLDER_ID))).collect();
    inventory
        .apply_folder_fetch(
            CategoryKey::new(owner(), FolderId::new(uuid(ROW_FOLDER_ID))),
            ServerVersion::from_raw(1),
            Some(ROW_ITEMS as u32),
            children,
        )
        .expect("row fixture children apply");
    (inventory.snapshot(), std::collections::HashSet::from([CategoryKey::new(owner(), FolderId::new(uuid(ROW_FOLDER_ID)))]))
}

fn inventory() -> Inventory {
    let root = Category {
        key: category_key(ROOT_ID),
        parent: None,
        preferred_folder_type: FolderType::RootInventory,
        name: Arc::from("Personal"),
        server_version: ServerVersion::UNKNOWN,
        server_descendent_count: None,
    };
    Inventory::new(root, None, InventoryLimits::default()).expect("fixture roots are valid")
}

fn item(index: usize, parent_id: u128) -> Item {
    let id = index as u128 + 100;
    Item {
        key: ItemKey::new(owner(), ItemId::new(uuid(id))),
        parent: category_key(parent_id),
        asset_id: AssetId::new(uuid(id + 1_000_000)),
        asset_type: AssetType::Notecard,
        inventory_type: InventoryType::Notecard,
        flags: 0,
        name: Arc::from(format!("item-{index:06}")),
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
