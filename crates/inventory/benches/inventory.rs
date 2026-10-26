use std::{hint::black_box, mem::size_of, sync::Arc};

use ::inventory as inventory_model;
use criterion::{BatchSize, Criterion};
use inventory_model::{
    AssetType, Category, CategoryKey, Inventory, InventoryChild, InventoryOwner, InventoryType, Item, ItemKey, PermissionMask, Permissions,
    SaleInfo, SaleType, ServerVersion,
};
use test_support::allocation;
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

const ENTRY_COUNT: usize = 100_000;
const OWNER_ID: u128 = 77;
const ROOT_ID: u128 = 1;

pub(super) fn bench(criterion: &mut Criterion) {
    report_memory();
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

fn root_inventory() -> Inventory {
    let root = Category {
        key: category_key(ROOT_ID),
        parent: None,
        preferred_folder_type: inventory_model::FolderType::RootInventory,
        name: Arc::from("root"),
        server_version: ServerVersion::UNKNOWN,
        server_descendent_count: None,
    };
    Inventory::new(root, None).expect("benchmark root should be valid")
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
