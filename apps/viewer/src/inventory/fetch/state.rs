use std::collections::{HashSet, VecDeque};

use ::inventory::{CategoryKey, InventorySnapshot};

use super::MAX_FALLBACK_FOLDERS;

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

pub(super) struct TreeKnowledge {
    complete: HashSet<CategoryKey>,
}

impl TreeKnowledge {
    pub(super) fn new(snapshot: &InventorySnapshot) -> Self {
        Self {
            complete: snapshot.categories().filter(|category| snapshot.is_complete(category.key)).map(|category| category.key).collect(),
        }
    }

    pub(super) fn is_complete(&self, key: CategoryKey) -> bool {
        self.complete.contains(&key)
    }

    pub(super) fn record(&mut self, key: CategoryKey, complete: bool) {
        if complete {
            self.complete.insert(key);
        } else {
            self.complete.remove(&key);
        }
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
