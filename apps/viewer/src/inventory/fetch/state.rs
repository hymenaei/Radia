use std::collections::{HashMap, HashSet, VecDeque};

use ::inventory::{CategoryKey, InventoryNodeId, InventorySnapshot, ServerVersion};

use super::{FolderFetch, MAX_FALLBACK_FOLDERS};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct BackgroundRequest {
    pub(super) folder: CategoryKey,
    pub(super) depth: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SourceKind {
    Ais,
    Legacy,
    Unavailable,
}

pub(super) struct BackgroundPlan {
    pub(super) folders: Vec<CategoryKey>,
    pub(super) depth: u8,
}

struct FolderState {
    version: ServerVersion,
    descendent_count: Option<u32>,
    child_count: usize,
    categories: HashSet<CategoryKey>,
}

impl FolderState {
    fn is_complete(&self) -> bool {
        self.version.is_known() && self.descendent_count.and_then(|count| usize::try_from(count).ok()) == Some(self.child_count)
    }
}

pub(super) struct TreeKnowledge {
    folders: HashMap<CategoryKey, FolderState>,
}

impl TreeKnowledge {
    pub(super) fn new(snapshot: &InventorySnapshot) -> Self {
        let mut folders = HashMap::new();
        for category in snapshot.categories() {
            let key = category.key;
            let categories = snapshot
                .children(key)
                .filter_map(|node| match node.id() {
                    InventoryNodeId::Category(id) => Some(CategoryKey::new(key.owner(), id)),
                    InventoryNodeId::Item(_) => None,
                })
                .collect();
            folders.insert(
                key,
                FolderState {
                    version: category.server_version,
                    descendent_count: category.server_descendent_count,
                    child_count: snapshot.child_count(key),
                    categories,
                },
            );
        }
        Self { folders }
    }

    pub(super) fn is_complete(&self, key: CategoryKey) -> bool {
        self.folders.get(&key).is_some_and(FolderState::is_complete)
    }

    pub(super) fn is_stale(&self, key: CategoryKey, version: ServerVersion) -> bool {
        self.folders.get(&key).is_some_and(|current| current.is_complete() && version.is_known() && current.version.raw() > version.raw())
    }

    pub(super) fn apply(&mut self, fetch: &FolderFetch) -> HashSet<CategoryKey> {
        let mut stale_children = HashSet::new();
        let mut category_children = HashSet::new();
        for child in &fetch.children {
            let ::inventory::InventoryChild::Category(category) = child else { continue };
            let key = category.key;
            category_children.insert(key);
            if let Some(current) = self.folders.get(&key)
                && current.is_complete()
                && category.server_version.is_known()
                && current.version.raw() > category.server_version.raw()
            {
                stale_children.insert(key);
                continue;
            }
            let existing_child_count = self.folders.get(&key).map_or(0, |state| state.child_count);
            let existing_categories = self.folders.get(&key).map_or_else(HashSet::new, |state| state.categories.clone());
            self.folders.insert(
                key,
                FolderState {
                    version: category.server_version,
                    descendent_count: category.server_descendent_count,
                    child_count: existing_child_count,
                    categories: existing_categories,
                },
            );
        }
        self.folders.insert(
            fetch.folder,
            FolderState {
                version: fetch.version,
                descendent_count: fetch.descendent_count,
                child_count: fetch.children.len(),
                categories: category_children,
            },
        );
        stale_children
    }
}

pub(super) fn next_background_job(
    background: &mut VecDeque<BackgroundRequest>,
    queued: &mut HashSet<CategoryKey>,
    tree: &TreeKnowledge,
    in_flight: &HashSet<CategoryKey>,
    failed: &HashSet<CategoryKey>,
    source_kind: impl Fn(::inventory::InventoryOwner) -> SourceKind,
) -> Option<BackgroundPlan> {
    while let Some(request) = background.pop_front() {
        queued.remove(&request.folder);
        if tree.is_complete(request.folder) || in_flight.contains(&request.folder) || failed.contains(&request.folder) {
            continue;
        }
        match source_kind(request.folder.owner()) {
            SourceKind::Ais => return Some(BackgroundPlan { folders: vec![request.folder], depth: request.depth }),
            SourceKind::Legacy => {
                let mut folders = vec![request.folder];
                while folders.len() < MAX_FALLBACK_FOLDERS {
                    let Some(next) = background.front().copied() else { break };
                    if next.folder.owner() != request.folder.owner() {
                        break;
                    }
                    background.pop_front();
                    queued.remove(&next.folder);
                    if tree.is_complete(next.folder) || in_flight.contains(&next.folder) || failed.contains(&next.folder) {
                        continue;
                    }
                    folders.push(next.folder);
                }
                return Some(BackgroundPlan { folders, depth: 0 });
            }
            SourceKind::Unavailable => return Some(BackgroundPlan { folders: vec![request.folder], depth: 0 }),
        }
    }
    None
}

pub(super) fn enqueue_background(
    request: BackgroundRequest,
    tree: &TreeKnowledge,
    in_flight: &HashSet<CategoryKey>,
    failed: &HashSet<CategoryKey>,
    background: &mut VecDeque<BackgroundRequest>,
    queued: &mut HashSet<CategoryKey>,
) {
    if tree.is_complete(request.folder)
        || in_flight.contains(&request.folder)
        || failed.contains(&request.folder)
        || !queued.insert(request.folder)
    {
        return;
    }
    background.push_back(request);
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
