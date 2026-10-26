use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use inventory::{
    AssetType, Category, CategoryKey, FolderType, Inventory, InventoryChild, InventoryError, InventoryLimits, InventoryNodeId,
    InventoryNodeKey, InventoryOwner, InventoryType, Item, ItemKey, LinkResolution, PermissionMask, Permissions, SaleInfo, SaleType,
    ServerVersion,
};
use proptest::prelude::*;
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

fn uuid(id: u128) -> Uuid {
    Uuid::from_u128(id)
}

fn owner(id: u128) -> InventoryOwner {
    InventoryOwner::new(uuid(id))
}

fn category(owner: InventoryOwner, id: u128, parent: Option<u128>, name: &str) -> Category {
    Category {
        key: CategoryKey::new(owner, FolderId::new(uuid(id))),
        parent: parent.map(|parent| CategoryKey::new(owner, FolderId::new(uuid(parent)))),
        preferred_folder_type: FolderType::None,
        name: Arc::from(name),
        server_version: ServerVersion::UNKNOWN,
        server_descendent_count: None,
    }
}

fn item(owner: InventoryOwner, id: u128, parent: u128, asset_type: AssetType, asset_id: u128) -> Item {
    Item {
        key: ItemKey::new(owner, ItemId::new(uuid(id))),
        parent: CategoryKey::new(owner, FolderId::new(uuid(parent))),
        asset_id: AssetId::new(uuid(asset_id)),
        asset_type,
        inventory_type: InventoryType::Unknown(777),
        flags: 0,
        name: Arc::from(format!("item-{id}")),
        description: Arc::from("fixture"),
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

fn inventory(owner: InventoryOwner) -> Inventory {
    Inventory::new(category(owner, 1, None, "root"), None, InventoryLimits::default()).expect("valid root")
}

fn inventory_with_limits(owner: InventoryOwner, limits: InventoryLimits) -> Inventory {
    Inventory::new(category(owner, 1, None, "root"), None, limits).expect("valid root")
}

fn invariant(snapshot: &inventory::InventorySnapshot) -> Option<&'static str> {
    let roots = snapshot.roots();
    let categories = snapshot.categories().collect::<Vec<_>>();
    let mut expected_children = HashSet::new();
    let mut owners = HashSet::new();
    let mut categories_by_type = HashMap::<(InventoryOwner, FolderType), Vec<CategoryKey>>::new();
    for category in &categories {
        owners.insert(category.key.owner());
        categories_by_type.entry((category.key.owner(), category.preferred_folder_type)).or_default().push(category.key);
        if roots.contains(category.key) {
            if category.parent.is_some() {
                return Some("root has a parent");
            }
        } else {
            let Some(parent) = category.parent else { return Some("category has no parent") };
            if parent.owner() != category.key.owner() || snapshot.category(parent).is_none() {
                return Some("category parent is missing");
            }
            if !expected_children.insert((parent, InventoryNodeId::Category(category.key.id()))) {
                return Some("category is indexed twice");
            }
        }
        let mut path = HashSet::new();
        let mut current = category.key;
        loop {
            if !path.insert(current) {
                return Some("category parent chain cycles");
            }
            let Some(current_category) = snapshot.category(current) else { return Some("parent chain has a missing category") };
            match current_category.parent {
                Some(parent) => current = parent,
                None if roots.contains(current) => break,
                None => return Some("parent chain does not reach a root"),
            }
        }
        let expected_complete = category.server_version.is_known()
            && category.server_descendent_count.and_then(|count| usize::try_from(count).ok()) == Some(snapshot.child_count(category.key));
        if snapshot.is_complete(category.key) != expected_complete {
            return Some("category completeness disagrees with its fields");
        }
    }
    for item in snapshot.items() {
        if item.parent.owner() != item.key.owner() || snapshot.category(item.parent).is_none() {
            return Some("item parent is missing");
        }
        if !expected_children.insert((item.parent, InventoryNodeId::Item(item.key.id()))) {
            return Some("item is indexed twice");
        }
    }
    for category in categories {
        let actual = snapshot.children(category.key).map(|node| (category.key, node.id())).collect::<HashSet<_>>();
        let expected = expected_children.iter().copied().filter(|(parent, _)| *parent == category.key).collect::<HashSet<_>>();
        if actual != expected {
            return Some("child index disagrees with parent links");
        }
    }
    for owner in owners {
        let expected_incomplete = snapshot.categories_for(owner).filter(|category| !snapshot.is_complete(category.key)).count();
        if snapshot.incomplete_category_count(owner) != expected_incomplete {
            return Some("incomplete-category count disagrees with a full recount");
        }
        if snapshot.owner_is_complete(owner) != (expected_incomplete == 0) {
            return Some("owner completeness disagrees with a full recount");
        }
    }
    for ((owner, folder_type), mut keys) in categories_by_type {
        keys.sort_unstable();
        if snapshot.category_with_type(owner, folder_type).map(|category| category.key) != keys.first().copied() {
            return Some("preferred folder-type index disagrees with categories");
        }
    }
    None
}

#[test]
fn cycle_attempt_keeps_state() {
    let owner = owner(9);
    let mut inventory = inventory(owner);
    let root = inventory.roots().personal();
    let first = category(owner, 2, Some(1), "first");
    let second = category(owner, 3, Some(2), "second");
    let first_key = first.key;
    let second_key = second.key;
    inventory.upsert_category(first).expect("add first category");
    inventory.upsert_category(second).expect("add second category");
    let before = inventory.snapshot();

    let result = inventory.move_node(
        InventoryNodeKey::new(owner, InventoryNodeId::Category(FolderId::new(uuid(2)))),
        CategoryKey::new(owner, FolderId::new(uuid(3))),
    );

    assert_eq!(result, Err(InventoryError::Cycle));
    assert_eq!(inventory.revision(), before.revision());
    assert!(before.category(root).is_some());
    let after = inventory.snapshot();
    assert_eq!(after.category(first_key).and_then(|category| category.parent), Some(root));
    assert_eq!(after.category(second_key).and_then(|category| category.parent), Some(first_key));
    assert_eq!(invariant(&after), None);
}

#[test]
fn removed_id_can_be_reused() {
    let owner = owner(10);
    let mut inventory = inventory(owner);
    let category_key = CategoryKey::new(owner, FolderId::new(uuid(2)));
    let item_key = ItemKey::new(owner, ItemId::new(uuid(3)));
    inventory.upsert_category(category(owner, 2, Some(1), "old")).expect("add category");
    inventory.upsert_item(item(owner, 3, 2, AssetType::Notecard, 4)).expect("add item");

    inventory.remove_subtree(owner, category_key.id()).expect("remove subtree");

    assert!(inventory.snapshot().category(category_key).is_none());
    assert!(inventory.snapshot().item(item_key).is_none());
    inventory.upsert_category(category(owner, 2, Some(1), "new")).expect("reuse category ID");
    inventory.upsert_item(item(owner, 3, 2, AssetType::Notecard, 5)).expect("reuse item ID");

    let snapshot = inventory.snapshot();
    assert_eq!(snapshot.category(category_key).map(|category| category.name.as_ref()), Some("new"));
    assert!(snapshot.item(item_key).is_some());
    assert_eq!(invariant(&snapshot), None);
}

#[test]
fn fetch_replaces_direct_children() {
    let owner = owner(11);
    let mut inventory = inventory(owner);
    let root = inventory.roots().personal();
    let folder = CategoryKey::new(owner, FolderId::new(uuid(2)));
    let retained = ItemKey::new(owner, ItemId::new(uuid(3)));
    let removed = ItemKey::new(owner, ItemId::new(uuid(4)));
    inventory.upsert_category(category(owner, 2, Some(1), "folder")).expect("add folder");
    inventory.upsert_item(item(owner, 3, 2, AssetType::Notecard, 30)).expect("add retained item");
    inventory.upsert_item(item(owner, 4, 2, AssetType::Notecard, 40)).expect("add stale item");

    let changes = inventory
        .apply_folder_fetch(
            folder,
            ServerVersion::from_raw(8),
            Some(2),
            vec![
                InventoryChild::Item(item(owner, 3, 2, AssetType::Notecard, 31)),
                InventoryChild::Category(category(owner, 5, Some(2), "added")),
            ],
        )
        .expect("apply folder result");

    let snapshot = inventory.snapshot();
    assert!(changes.changes.len() >= 3);
    assert!(snapshot.item(retained).is_some());
    assert!(snapshot.item(removed).is_none());
    assert!(snapshot.category(CategoryKey::new(owner, FolderId::new(uuid(5)))).is_some());
    assert!(!snapshot.is_complete(root));
    assert!(snapshot.is_complete(folder));
    assert_eq!(invariant(&snapshot), None);
}

#[test]
fn fetch_replaces_items_at_capacity() {
    let owner = owner(16);
    let limits = InventoryLimits { max_categories: 2, max_items: 1, max_folder_fetch_children: 1 };
    let mut inventory = inventory_with_limits(owner, limits);
    let folder = CategoryKey::new(owner, FolderId::new(uuid(2)));
    inventory.upsert_category(category(owner, 2, Some(1), "folder")).expect("add folder");
    inventory
        .apply_folder_fetch(folder, ServerVersion::UNKNOWN, Some(1), vec![InventoryChild::Item(item(owner, 3, 2, AssetType::Notecard, 30))])
        .expect("fill item capacity");

    inventory
        .apply_folder_fetch(
            folder,
            ServerVersion::from_raw(1),
            Some(1),
            vec![InventoryChild::Item(item(owner, 5, 2, AssetType::Notecard, 50))],
        )
        .expect("replace item at capacity");
    let snapshot = inventory.snapshot();
    assert_eq!(snapshot.items().count(), limits.max_items);
    assert!(snapshot.item(ItemKey::new(owner, ItemId::new(uuid(3)))).is_none());
    assert!(snapshot.item(ItemKey::new(owner, ItemId::new(uuid(5)))).is_some());
    assert!(snapshot.is_complete(folder));
    assert_eq!(invariant(&snapshot), None);
}

#[test]
fn fetch_replaces_categories_at_capacity() {
    let owner = owner(17);
    let limits = InventoryLimits { max_categories: 2, max_items: 0, max_folder_fetch_children: 1 };
    let mut inventory = inventory_with_limits(owner, limits);
    let root = inventory.roots().personal();
    inventory
        .apply_folder_fetch(root, ServerVersion::UNKNOWN, Some(1), vec![InventoryChild::Category(category(owner, 2, Some(1), "old"))])
        .expect("fill category capacity");

    inventory
        .apply_folder_fetch(root, ServerVersion::from_raw(1), Some(1), vec![InventoryChild::Category(category(owner, 3, Some(1), "new"))])
        .expect("replace category at capacity");
    let snapshot = inventory.snapshot();
    assert_eq!(snapshot.categories().count(), limits.max_categories);
    assert!(snapshot.category(CategoryKey::new(owner, FolderId::new(uuid(2)))).is_none());
    assert!(snapshot.category(CategoryKey::new(owner, FolderId::new(uuid(3)))).is_some());
    assert!(snapshot.is_complete(root));
    assert_eq!(invariant(&snapshot), None);
}

#[test]
fn fetch_keeps_incoming_category_moved_out_of_stale_subtree() {
    let owner = owner(18);
    let mut inventory = inventory(owner);
    let target = CategoryKey::new(owner, FolderId::new(uuid(2)));
    let moved = CategoryKey::new(owner, FolderId::new(uuid(4)));
    let retained_item = ItemKey::new(owner, ItemId::new(uuid(5)));
    inventory.upsert_category(category(owner, 2, Some(1), "target")).expect("add target folder");
    inventory.upsert_category(category(owner, 3, Some(2), "stale")).expect("add stale folder");
    inventory.upsert_category(category(owner, 4, Some(3), "moved")).expect("add nested incoming folder");
    inventory.upsert_item(item(owner, 5, 4, AssetType::Notecard, 50)).expect("add descendant item");

    inventory
        .apply_folder_fetch(
            target,
            ServerVersion::from_raw(1),
            Some(1),
            vec![InventoryChild::Category(category(owner, 4, Some(2), "moved"))],
        )
        .expect("move incoming folder out before removing the stale subtree");

    let snapshot = inventory.snapshot();
    assert!(snapshot.category(CategoryKey::new(owner, FolderId::new(uuid(3)))).is_none());
    assert_eq!(snapshot.category(moved).and_then(|category| category.parent), Some(target));
    assert_eq!(snapshot.item(retained_item).map(|item| item.parent), Some(moved));
    assert_eq!(invariant(&snapshot), None);
}

#[test]
fn snapshots_keep_prior_state() {
    let owner = owner(12);
    let mut inventory = inventory(owner);
    let old = inventory.snapshot();
    inventory.upsert_item(item(owner, 2, 1, AssetType::Notecard, 3)).expect("add item");
    let current = inventory.snapshot();

    assert_eq!(old.revision().get(), 0);
    assert_eq!(old.child_count(inventory.roots().personal()), 0);
    assert_eq!(current.child_count(inventory.roots().personal()), 1);
    assert_eq!(invariant(&old), None);
    assert_eq!(invariant(&current), None);
}

#[test]
fn links_distinguish_unloaded_and_broken() {
    let owner = owner(13);
    let mut inventory = inventory(owner);
    let root = inventory.roots().personal();
    let link = item(owner, 2, 1, AssetType::Link, 99);
    let link_key = link.key;
    inventory.upsert_item(link.clone()).expect("add link");
    assert_eq!(inventory.snapshot().link_resolution(link_key), Some(LinkResolution::NotLoaded(uuid(99))));

    inventory.apply_folder_fetch(root, ServerVersion::from_raw(1), Some(1), vec![InventoryChild::Item(link)]).expect("complete root fetch");

    assert_eq!(inventory.snapshot().link_resolution(link_key), Some(LinkResolution::Broken(uuid(99))));
}

#[test]
fn folder_types_preserve_unknowns() {
    assert_eq!(FolderType::from_raw(23), FolderType::Favorite);
    assert_eq!(FolderType::from_raw(33).raw(), 33);
    assert!(FolderType::from_raw(999).is_protected());
    assert_eq!(AssetType::from_raw(24), AssetType::Link);
    assert_eq!(AssetType::from_raw(999).raw(), 999);
    assert_eq!(InventoryType::from_raw(999).raw(), 999);
    assert_eq!(ServerVersion::from_raw(-1).raw(), -1);
    assert!(ServerVersion::known(-1).is_none());
    assert_eq!(SaleType::from_raw(999).raw(), 999);
}

#[test]
fn preferred_folder_type_index_tracks_updates_removal_and_id_reuse() {
    let owner = owner(15);
    let mut inventory = inventory(owner);
    let mut first = category(owner, 2, Some(1), "first");
    let mut second = category(owner, 3, Some(1), "second");
    first.preferred_folder_type = FolderType::Favorite;
    second.preferred_folder_type = FolderType::Favorite;
    let first_key = first.key;
    let second_key = second.key;
    inventory.upsert_category(first.clone()).expect("add first favorite folder");
    inventory.upsert_category(second.clone()).expect("add second favorite folder");
    assert_eq!(inventory.snapshot().category_with_type(owner, FolderType::Favorite).map(|category| category.key), Some(first_key));

    first.preferred_folder_type = FolderType::Unknown(71);
    inventory.upsert_category(first).expect("change the first folder type");
    assert_eq!(inventory.snapshot().category_with_type(owner, FolderType::Favorite).map(|category| category.key), Some(second_key));
    assert_eq!(inventory.snapshot().category_with_type(owner, FolderType::Unknown(71)).map(|category| category.key), Some(first_key));

    inventory.remove_subtree(owner, second_key.id()).expect("remove second folder");
    assert!(inventory.snapshot().category_with_type(owner, FolderType::Favorite).is_none());
    inventory.upsert_category(second).expect("reuse removed folder ID");
    assert_eq!(inventory.snapshot().category_with_type(owner, FolderType::Favorite).map(|category| category.key), Some(second_key));
    assert_eq!(invariant(&inventory.snapshot()), None);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 72, ..ProptestConfig::default() })]

    #[test]
    fn sequences_preserve_invariants(operations in prop::collection::vec((0_u8..8, 0_u8..8, 0_u8..8, 0_u8..8), 0..72)) {
        let owner = owner(14);
        let limits = InventoryLimits { max_categories: 4, max_items: 3, max_folder_fetch_children: 4 };
        let mut inventory = inventory_with_limits(owner, limits);
        for id in 1..=3 {
            inventory.upsert_category(category(owner, 100 + id, Some(1), "capacity-category")).expect("fill category capacity");
            prop_assert_eq!(invariant(&inventory.snapshot()), None);
            inventory
                .upsert_item(item(owner, 200 + id, 1, AssetType::Notecard, 300 + id))
                .expect("fill item capacity");
            prop_assert_eq!(invariant(&inventory.snapshot()), None);
        }
        prop_assert_eq!(inventory.snapshot().categories().count(), limits.max_categories);
        prop_assert_eq!(inventory.snapshot().items().count(), limits.max_items);
        let category_revision = inventory.revision();
        prop_assert_eq!(
            inventory.upsert_category(category(owner, 999, Some(1), "over capacity")),
            Err(InventoryError::TooManyCategories)
        );
        prop_assert_eq!(inventory.revision(), category_revision);
        prop_assert_eq!(invariant(&inventory.snapshot()), None);
        let item_revision = inventory.revision();
        prop_assert_eq!(
            inventory.upsert_item(item(owner, 999, 1, AssetType::Notecard, 999)),
            Err(InventoryError::TooManyItems)
        );
        prop_assert_eq!(inventory.revision(), item_revision);
        prop_assert_eq!(invariant(&inventory.snapshot()), None);
        for (operation, a, b, c) in operations {
            let before = inventory.snapshot();
            let before_revision = before.revision();
            let before_category_count = before.categories().count();
            let before_item_count = before.items().count();
            let before_root_child_count = before.child_count(before.roots().personal());
            let category_id = 100 + u128::from(a);
            let parent_id = 100 + u128::from(b);
            let item_id = 200 + u128::from(a);
            let result = match operation {
                0 => {
                    let parent = if c % 2 == 0 { 1 } else { parent_id };
                    let mut category = category(owner, category_id, Some(parent), &format!("folder-{a}-{c}"));
                    category.preferred_folder_type = FolderType::from_raw(i32::from(c % 4));
                    inventory.upsert_category(category)
                }
                1 => {
                    let parent = if c % 2 == 0 { 1 } else { parent_id };
                    inventory.upsert_item(item(owner, item_id, parent, AssetType::Notecard, 300 + u128::from(c)))
                }
                2 => {
                    let parent = if c % 2 == 0 { 1 } else { parent_id };
                    inventory.move_node(
                        InventoryNodeKey::new(owner, InventoryNodeId::Category(FolderId::new(uuid(category_id)))),
                        CategoryKey::new(owner, FolderId::new(uuid(parent))),
                    )
                }
                3 => {
                    let parent = if c % 2 == 0 { 1 } else { parent_id };
                    inventory.move_node(
                        InventoryNodeKey::new(owner, InventoryNodeId::Item(ItemId::new(uuid(item_id)))),
                        CategoryKey::new(owner, FolderId::new(uuid(parent))),
                    )
                }
                4 => inventory.remove_subtree(owner, FolderId::new(uuid(category_id))),
                5 => {
                    let folder = CategoryKey::new(owner, FolderId::new(uuid(category_id)));
                    inventory.apply_folder_fetch(
                        folder,
                        ServerVersion::from_raw(i32::from(c)),
                        Some(2),
                        vec![
                            InventoryChild::Category(category(owner, parent_id, Some(category_id), "fetch-child")),
                            InventoryChild::Item(item(owner, 400 + u128::from(c), category_id, AssetType::Notecard, 500 + u128::from(c))),
                        ],
                    )
                }
                _ => inventory.set_version(CategoryKey::new(owner, FolderId::new(uuid(category_id))), ServerVersion::UNKNOWN),
            };
            match result {
                Ok(changes) => prop_assert_eq!(changes.revision, inventory.revision()),
                Err(_) => prop_assert_eq!(before_revision, inventory.revision()),
            }
            prop_assert_eq!(before.revision(), before_revision);
            prop_assert_eq!(before.categories().count(), before_category_count);
            prop_assert_eq!(before.items().count(), before_item_count);
            prop_assert_eq!(before.child_count(before.roots().personal()), before_root_child_count);
            prop_assert_eq!(invariant(&before), None);
            prop_assert_eq!(invariant(&inventory.snapshot()), None);
        }
    }
}
