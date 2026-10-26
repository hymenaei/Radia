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

pub(super) fn next_background_job(
    background: &mut VecDeque<BackgroundRequest>,
    queued: &mut HashSet<CategoryKey>,
    snapshot: &InventorySnapshot,
    in_flight: &HashSet<CategoryKey>,
    failed: &HashSet<CategoryKey>,
    source_kind: impl Fn(::inventory::InventoryOwner) -> SourceKind,
) -> Option<BackgroundPlan> {
    let mut index = 0;
    while index < background.len() {
        let request = background[index];
        if snapshot.is_complete(request.folder) || failed.contains(&request.folder) {
            background.remove(index);
            queued.remove(&request.folder);
            continue;
        }
        if in_flight.contains(&request.folder) {
            index += 1;
            continue;
        }
        let request = background.remove(index)?;
        queued.remove(&request.folder);
        match source_kind(request.folder.owner()) {
            SourceKind::Ais => return Some(BackgroundPlan { folders: vec![request.folder], depth: request.depth }),
            SourceKind::Legacy => {
                let mut folders = vec![request.folder];
                let mut next_index = index;
                while folders.len() < MAX_FALLBACK_FOLDERS {
                    let Some(next) = background.get(next_index).copied() else { break };
                    if next.folder.owner() != request.folder.owner() {
                        break;
                    }
                    if snapshot.is_complete(next.folder) || failed.contains(&next.folder) {
                        background.remove(next_index);
                        queued.remove(&next.folder);
                        continue;
                    }
                    if in_flight.contains(&next.folder) {
                        next_index += 1;
                        continue;
                    }
                    background.remove(next_index);
                    queued.remove(&next.folder);
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
    snapshot: &InventorySnapshot,
    failed: &HashSet<CategoryKey>,
    background: &mut VecDeque<BackgroundRequest>,
    queued: &mut HashSet<CategoryKey>,
) {
    if snapshot.is_complete(request.folder) || failed.contains(&request.folder) || !queued.insert(request.folder) {
        return;
    }
    background.push_back(request);
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
