use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

use ::inventory::{Category, CategoryKey, FolderType, Inventory, InventoryChild, InventoryLimits, InventoryOwner, ServerVersion};
use utils::FolderId;
use uuid::Uuid;

use super::{BackgroundRequest, SourceKind, enqueue_background, next_background_job};

fn key(owner: InventoryOwner, id: u128) -> CategoryKey {
    CategoryKey::new(owner, FolderId::new(Uuid::from_u128(id)))
}

fn category(key: CategoryKey, parent: Option<CategoryKey>) -> Category {
    Category {
        key,
        parent,
        preferred_folder_type: FolderType::None,
        name: Arc::from("folder"),
        server_version: ServerVersion::UNKNOWN,
        server_descendent_count: None,
    }
}

fn inventory_with_children(count: u128) -> (Inventory, CategoryKey, Vec<CategoryKey>) {
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = key(owner, 1);
    let children = (2..count + 2).map(|id| key(owner, id)).collect::<Vec<_>>();
    let mut root_category = category(root, None);
    root_category.server_version = ServerVersion::from_raw(1);
    root_category.server_descendent_count = Some(u32::try_from(count).expect("small test folder"));
    let mut inventory = Inventory::new(root_category, None, InventoryLimits::default()).expect("valid inventory");
    for child in &children {
        inventory.upsert_category(category(*child, Some(root))).expect("valid child category");
    }
    (inventory, root, children)
}

#[test]
fn legacy_selection_caps_batch_at_ten() {
    let (inventory, _, children) = inventory_with_children(11);
    let snapshot = inventory.snapshot();
    let mut background = children.iter().map(|folder| BackgroundRequest { folder: *folder, depth: 0 }).collect::<VecDeque<_>>();
    let mut queued = children.iter().copied().collect::<HashSet<_>>();

    let job = next_background_job(&mut background, &mut queued, &snapshot, &HashSet::new(), &HashSet::new(), |_| SourceKind::Legacy)
        .expect("legacy batch is available");

    assert_eq!(job.folders, children[..10]);
    assert_eq!(background.len(), 1);
    assert_eq!(queued, HashSet::from([children[10]]));
}

#[test]
fn selection_skips_complete_in_flight_and_failed() {
    let (inventory, root, children) = inventory_with_children(3);
    let snapshot = inventory.snapshot();
    let requests = [root, children[0], children[1], children[2]];
    let mut background = requests.iter().map(|folder| BackgroundRequest { folder: *folder, depth: 0 }).collect::<VecDeque<_>>();
    let mut queued = requests.into_iter().collect::<HashSet<_>>();
    let in_flight = HashSet::from([children[0]]);
    let failed = HashSet::from([children[1]]);

    let job = next_background_job(&mut background, &mut queued, &snapshot, &in_flight, &failed, |_| SourceKind::Ais)
        .expect("eligible folder remains");

    assert_eq!(job.folders, [children[2]]);
    assert_eq!(background, VecDeque::from([BackgroundRequest { folder: children[0], depth: 0 }]));
    assert_eq!(queued, HashSet::from([children[0]]));
}

#[test]
fn latest_snapshot_requeues_child_made_incomplete_by_parent_metadata() {
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = key(owner, 1);
    let child = key(owner, 2);
    let mut root_category = category(root, None);
    root_category.server_version = ServerVersion::from_raw(1);
    root_category.server_descendent_count = Some(1);
    let mut child_category = category(child, Some(root));
    child_category.server_version = ServerVersion::from_raw(10);
    child_category.server_descendent_count = Some(0);
    let mut inventory = Inventory::new(root_category, None, InventoryLimits::default()).expect("valid inventory");
    inventory.upsert_category(child_category).expect("valid child category");
    let complete_snapshot = inventory.snapshot();
    assert!(complete_snapshot.is_complete(child));

    let mut background = VecDeque::new();
    let mut queued = HashSet::new();
    enqueue_background(BackgroundRequest { folder: child, depth: 0 }, &complete_snapshot, &HashSet::new(), &mut background, &mut queued);
    assert!(background.is_empty());

    let mut updated_child = category(child, Some(root));
    updated_child.server_version = ServerVersion::from_raw(11);
    updated_child.server_descendent_count = Some(1);
    inventory
        .apply_folder_fetch(root, ServerVersion::from_raw(2), Some(1), vec![InventoryChild::Category(updated_child)])
        .expect("accept newer parent metadata");
    let latest_snapshot = inventory.snapshot();
    assert!(!latest_snapshot.is_complete(child));

    enqueue_background(BackgroundRequest { folder: child, depth: 0 }, &latest_snapshot, &HashSet::new(), &mut background, &mut queued);
    assert_eq!(background.pop_front(), Some(BackgroundRequest { folder: child, depth: 0 }));
}

#[test]
fn in_flight_folder_stays_queued_until_its_request_finishes() {
    let (inventory, _, children) = inventory_with_children(1);
    let child = children[0];
    let snapshot = inventory.snapshot();
    let in_flight = HashSet::from([child]);
    let mut background = VecDeque::new();
    let mut queued = HashSet::new();

    enqueue_background(BackgroundRequest { folder: child, depth: 0 }, &snapshot, &HashSet::new(), &mut background, &mut queued);
    assert_eq!(background.front(), Some(&BackgroundRequest { folder: child, depth: 0 }));
    assert_eq!(queued, HashSet::from([child]));
    assert!(next_background_job(&mut background, &mut queued, &snapshot, &in_flight, &HashSet::new(), |_| SourceKind::Ais).is_none());

    let job = next_background_job(&mut background, &mut queued, &snapshot, &HashSet::new(), &HashSet::new(), |_| SourceKind::Ais)
        .expect("folder can be fetched after in-flight request completes");
    assert_eq!(job.folders, [child]);
    assert!(background.is_empty());
}
