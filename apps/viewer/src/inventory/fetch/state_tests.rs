use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

use ::inventory::{Category, CategoryKey, FolderType, Inventory, InventoryLimits, InventoryOwner, ServerVersion};
use utils::FolderId;
use uuid::Uuid;

use super::{BackgroundRequest, SourceKind, TreeKnowledge, enqueue_background, next_background_job};

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
    tree.record(root, true);
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
fn rejected_apply_keeps_folder_fetchable() {
    let (inventory, root, _) = snapshot_with_children(0);
    let mut tree = TreeKnowledge::new(&inventory.snapshot());
    tree.record(root, true);
    assert!(tree.is_complete(root));
    tree.record(root, false);
    assert!(!tree.is_complete(root));

    let mut background = VecDeque::new();
    let mut queued = HashSet::new();
    enqueue_background(BackgroundRequest { folder: root, depth: 0 }, &tree, &HashSet::new(), &HashSet::new(), &mut background, &mut queued);
    assert_eq!(background.pop_front(), Some(BackgroundRequest { folder: root, depth: 0 }));
}
