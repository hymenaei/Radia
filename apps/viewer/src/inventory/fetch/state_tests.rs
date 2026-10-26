use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

use ::inventory::{Category, CategoryKey, FolderType, Inventory, InventoryChild, InventoryLimits, InventoryOwner, ServerVersion};
use utils::FolderId;
use uuid::Uuid;

use super::{BackgroundRequest, SourceKind, TreeKnowledge, enqueue_background, next_background_job};
use crate::inventory::fetch::FolderFetch;

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

fn snapshot_with_children(count: u128) -> (Inventory, CategoryKey, Vec<CategoryKey>) {
    let owner = InventoryOwner::new(Uuid::from_u128(1));
    let root = key(owner, 1);
    let children = (2..count + 2).map(|id| key(owner, id)).collect::<Vec<_>>();
    let mut inventory = Inventory::new(category(root, None), None, InventoryLimits::default()).expect("valid inventory");
    for child in &children {
        inventory.upsert_category(category(*child, Some(root))).expect("valid child category");
    }
    (inventory, root, children)
}

#[test]
fn legacy_selection_caps_batch_at_ten() {
    let (inventory, _, children) = snapshot_with_children(11);
    let tree = TreeKnowledge::new(&inventory.snapshot());
    let mut background = children.iter().map(|folder| BackgroundRequest { folder: *folder, depth: 0 }).collect::<VecDeque<_>>();
    let mut queued = children.iter().copied().collect::<HashSet<_>>();

    let job = next_background_job(&mut background, &mut queued, &tree, &HashSet::new(), &HashSet::new(), |_| SourceKind::Legacy)
        .expect("legacy batch is available");

    assert_eq!(job.folders, children[..10]);
    assert_eq!(background.len(), 1);
    assert_eq!(queued, HashSet::from([children[10]]));
}

#[test]
fn selection_skips_complete_in_flight_and_failed() {
    let (inventory, root, children) = snapshot_with_children(3);
    let snapshot = inventory.snapshot();
    let mut tree = TreeKnowledge::new(&snapshot);
    let root_children = children.iter().map(|child| InventoryChild::Category(category(*child, Some(root)))).collect();
    tree.apply(&FolderFetch { folder: root, version: ServerVersion::from_raw(4), descendent_count: Some(3), children: root_children });
    let requests = [root, children[0], children[1], children[2]];
    let mut background = requests.iter().map(|folder| BackgroundRequest { folder: *folder, depth: 0 }).collect::<VecDeque<_>>();
    let mut queued = requests.into_iter().collect::<HashSet<_>>();
    let in_flight = HashSet::from([children[0]]);
    let failed = HashSet::from([children[1]]);

    let job = next_background_job(&mut background, &mut queued, &tree, &in_flight, &failed, |_| SourceKind::Ais)
        .expect("eligible folder remains");

    assert_eq!(job.folders, [children[2]]);
    assert!(background.is_empty());
}

#[test]
fn complete_version_marks_older_fetch_stale() {
    let (inventory, root, _) = snapshot_with_children(0);
    let mut tree = TreeKnowledge::new(&inventory.snapshot());
    tree.apply(&FolderFetch { folder: root, version: ServerVersion::from_raw(8), descendent_count: Some(0), children: Vec::new() });

    assert!(tree.is_complete(root));
    assert!(tree.is_stale(root, ServerVersion::from_raw(7)));
    assert!(!tree.is_stale(root, ServerVersion::from_raw(9)));

    let mut background = VecDeque::new();
    let mut queued = HashSet::new();
    enqueue_background(BackgroundRequest { folder: root, depth: 0 }, &tree, &HashSet::new(), &HashSet::new(), &mut background, &mut queued);
    assert!(background.is_empty());
}
