use std::{
    collections::{BTreeSet, HashMap},
    io,
    path::Path,
    sync::{Arc, Mutex, RwLock, mpsc::SyncSender},
};

use super::{Entry, Key, PlannedChange, StorageError, UnavailableReason};
use crate::cache::index::Mutation;
use crate::cache::{DIRECTORY_OVERHEAD, FILE_OVERHEAD, INDEX_FILE, INDEX_FILE_OVERHEAD, JOURNAL_FILE, SNAPSHOT_FILE};

pub(super) fn mutations(changes: &[PlannedChange]) -> Result<Vec<Mutation>, StorageError> {
    let mut mutations = Vec::new();
    mutations.try_reserve_exact(changes.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
    for change in changes {
        mutations.push(match &change.next {
            Some(entry) => Mutation::Put(entry.record()),
            None => Mutation::Remove(change.key),
        });
    }
    Ok(mutations)
}

pub(super) fn sort_changes(changes: &mut [PlannedChange]) {
    changes.sort_unstable_by_key(|change| (change.next.is_some(), change.key));
}

pub(super) fn is_index_file(root: &Path, path: &Path) -> bool {
    if path.parent() != Some(root) {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == INDEX_FILE
        || name.strip_prefix(INDEX_FILE).is_some_and(|suffix| matches!(suffix, "-wal" | "-shm" | ".maintenance.lock"))
        || matches!(name, JOURNAL_FILE | SNAPSHOT_FILE | "lock")
}

pub(super) fn effective_entry(
    key: &Key,
    changes: &HashMap<Key, Option<Arc<Entry>>>,
    entries: &HashMap<Key, Arc<Entry>>,
) -> Option<Arc<Entry>> {
    match changes.get(key) {
        Some(entry) => entry.clone(),
        None => entries.get(key).cloned(),
    }
}

pub(super) fn remove_new_candidate(key: &Key, order: &mut BTreeSet<(u64, Key)>, stamps: &mut HashMap<Key, u64>) {
    if let Some(stamp) = stamps.remove(key) {
        order.remove(&(stamp, *key));
    }
}

pub(super) fn next_victim(
    base_order: &mut std::iter::Peekable<impl Iterator<Item = (u64, Key)>>,
    new_order: &mut BTreeSet<(u64, Key)>,
    new_stamps: &mut HashMap<Key, u64>,
    protected: Option<Key>,
    changes: &HashMap<Key, Option<Arc<Entry>>>,
    entries: &HashMap<Key, Arc<Entry>>,
) -> Option<Key> {
    loop {
        while let Some((_, key)) = base_order.peek().cloned() {
            if Some(&key) == protected.as_ref() || changes.contains_key(&key) || effective_entry(&key, changes, entries).is_none() {
                base_order.next();
            } else {
                break;
            }
        }
        let stale = new_order
            .iter()
            .find(|&(_, key)| changes.get(key).is_some_and(Option::is_none) || effective_entry(key, changes, entries).is_none())
            .cloned();
        if let Some(stale) = stale {
            new_order.remove(&stale);
            new_stamps.remove(&stale.1);
            continue;
        }
        let base = base_order.peek().cloned();
        let new = new_order.iter().find(|&(_, key)| Some(key) != protected.as_ref()).cloned();
        let selected = match (base, new) {
            (Some(base), Some(new)) if base <= new => {
                base_order.next();
                Some(base)
            }
            (_, Some(new)) => {
                new_order.remove(&new);
                new_stamps.remove(&new.1);
                Some(new)
            }
            (Some(base), None) => {
                base_order.next();
                Some(base)
            }
            (None, None) => None,
        };
        if let Some((_, key)) = selected {
            if effective_entry(&key, changes, entries).is_some() {
                return Some(key);
            }
        } else {
            return None;
        }
    }
}

pub(super) fn account_size(index_bytes: u64, data_bytes: u64, entry_count: usize, directory_count: u64) -> Option<u64> {
    let files = u64::try_from(entry_count).ok()?.checked_add(INDEX_FILE_OVERHEAD)?;
    let directories = directory_count.checked_add(1)?;
    DIRECTORY_OVERHEAD
        .checked_mul(directories)?
        .checked_add(index_bytes)?
        .checked_add(data_bytes)?
        .checked_add(FILE_OVERHEAD.checked_mul(files)?)
}

pub(super) fn read_map(entries: &RwLock<HashMap<Key, Arc<Entry>>>) -> std::sync::RwLockReadGuard<'_, HashMap<Key, Arc<Entry>>> {
    match entries.read() {
        Ok(entries) => entries,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(super) fn write_map(entries: &RwLock<HashMap<Key, Arc<Entry>>>) -> std::sync::RwLockWriteGuard<'_, HashMap<Key, Arc<Entry>>> {
    match entries.write() {
        Ok(entries) => entries,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(super) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(super) fn reply_result<T>(sender: SyncSender<Result<T, StorageError>>, value: Result<T, StorageError>) {
    match sender.send(value) {
        Ok(()) | Err(_) => {}
    }
}

pub(super) fn clone_error(error: &StorageError) -> StorageError {
    match error {
        StorageError::InUse => StorageError::InUse,
        StorageError::InvalidNamespaces => StorageError::InvalidNamespaces,
        StorageError::UnknownNamespace => StorageError::UnknownNamespace,
        StorageError::Unavailable(reason) => StorageError::Unavailable(*reason),
        StorageError::LimitTooSmall => StorageError::LimitTooSmall,
        StorageError::TooLarge => StorageError::TooLarge,
        StorageError::Io(error) => StorageError::Io(io::Error::new(error.kind(), error.to_string())),
        StorageError::WorkerPanicked => StorageError::WorkerPanicked,
    }
}

pub(super) fn simulated_crash_error() -> StorageError {
    StorageError::Io(io::Error::new(io::ErrorKind::Interrupted, "simulated cache crash"))
}

pub(super) fn is_simulated_crash(error: &StorageError) -> bool {
    matches!(error, StorageError::Io(error) if error.kind() == io::ErrorKind::Interrupted)
}
