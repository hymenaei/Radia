use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::{Arc, atomic::Ordering},
};

use super::support::{account_size, effective_entry, lock, read_map, remove_new_candidate, sort_changes, write_map};
use super::{
    DirectoryId, Entry, EvictionReason, Key, Lru, PlannedChange, ProtectionRegistry, StorageError, UnavailableReason, WorkerState,
};
use crate::cache::MAX_VALUE_BYTES;
use crate::cache::eviction::{EvictionCandidate, EvictionPolicy, OrderedScan};

const MAX_EVICTIONS_PER_BATCH: usize = 32;
const MAX_EVICTION_CANDIDATES: usize = 128;

pub(super) struct Plan {
    changes: HashMap<Key, Option<Arc<Entry>>>,
    directory_deltas: HashMap<DirectoryId, i64>,
    eviction_reasons: HashMap<Key, EvictionReason>,
}

enum VictimSearch {
    Found(Key, EvictionReason),
    Exhausted,
}

enum ScanResult {
    Found(Key, EvictionReason),
    Exhausted,
    Deferred,
}

impl WorkerState {
    pub(super) fn accounted_size(&self) -> u64 {
        let index_bytes = self.index.estimated_bytes(self.entry_count).unwrap_or(u64::MAX);
        account_size(index_bytes, self.data_bytes, self.entry_count, self.directory_count).unwrap_or(u64::MAX)
    }

    pub(super) fn single_put_fits_empty(&self, key: Key, size: usize) -> Result<bool, StorageError> {
        let Some((prefixes, _)) = self.read_state.namespaces.directories(key) else {
            return Err(StorageError::UnknownNamespace);
        };
        let directory_count = prefixes.len().checked_add(1).and_then(|count| u64::try_from(count).ok()).ok_or(StorageError::TooLarge)?;
        let index_bytes = self.index.estimated_bytes(1)?;
        let data_bytes = u64::try_from(size).map_err(|_| StorageError::TooLarge)?;
        let accounted = account_size(index_bytes, data_bytes, 1, directory_count).ok_or(StorageError::TooLarge)?;
        Ok(accounted <= self.size_limit)
    }

    pub(super) fn plan_puts(&mut self, staged: &[Arc<Entry>], protection: &ProtectionRegistry) -> Result<Plan, StorageError> {
        let initial_scan_cursor = self.eviction_scan_cursor;
        let entries = read_map(&self.read_state.entries);
        let mut changes = HashMap::new();
        changes.try_reserve(staged.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let mut eviction_reasons = HashMap::new();
        eviction_reasons.try_reserve(MAX_EVICTIONS_PER_BATCH).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let mut directory_deltas = HashMap::<DirectoryId, i64>::new();
        directory_deltas
            .try_reserve(staged.len().saturating_mul(3))
            .map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let mut new_order = BTreeSet::<(u64, Key)>::new();
        let mut new_stamps = HashMap::<Key, u64>::new();
        let mut data_bytes = self.data_bytes;
        let mut directory_count = self.directory_count;
        let mut entry_count = self.entry_count;
        let now = self.read_state.usage_clock.load(Ordering::Relaxed);
        let remaining_evictions = MAX_EVICTIONS_PER_BATCH.saturating_sub(self.evictions_this_batch);

        for entry in staged {
            let key = entry.key;
            if let Some(previous) = effective_entry(&key, &changes, &entries) {
                self.adjust_directories(&key, -1, &mut directory_deltas, &mut directory_count)?;
                data_bytes = data_bytes.checked_sub(previous.size).ok_or(StorageError::TooLarge)?;
                entry_count = entry_count.saturating_sub(1);
            }
            remove_new_candidate(&key, &mut new_order, &mut new_stamps);
            self.adjust_directories(&key, 1, &mut directory_deltas, &mut directory_count)?;
            data_bytes = data_bytes.checked_add(entry.size).ok_or(StorageError::TooLarge)?;
            entry_count = entry_count.checked_add(1).ok_or(StorageError::TooLarge)?;
            changes.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
            changes.insert(key, Some(Arc::clone(entry)));
            eviction_reasons.remove(&key);
            let stamp = entry.last_use.load(Ordering::Relaxed);
            new_order.insert((stamp, key));
            new_stamps.insert(key, stamp);
            while !self.fits(data_bytes, entry_count, directory_count, self.size_limit)? {
                if eviction_reasons.len() >= remaining_evictions {
                    drop(entries);
                    self.eviction_scan_cursor = initial_scan_cursor;
                    return Err(StorageError::TooLarge);
                }
                let (victim, reason) = match find_victim(
                    &self.lru,
                    &mut new_order,
                    &mut new_stamps,
                    Some(key),
                    &changes,
                    &entries,
                    protection,
                    self.read_state.policy.as_ref(),
                    now,
                    &mut self.eviction_scan_cursor,
                ) {
                    VictimSearch::Found(victim, reason) => (victim, reason),
                    VictimSearch::Exhausted => return Err(StorageError::TooLarge),
                };
                let previous = effective_entry(&victim, &changes, &entries).ok_or(StorageError::TooLarge)?;
                self.adjust_directories(&victim, -1, &mut directory_deltas, &mut directory_count)?;
                data_bytes = data_bytes.checked_sub(previous.size).ok_or(StorageError::TooLarge)?;
                entry_count = entry_count.saturating_sub(1);
                changes.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
                changes.insert(victim, None);
                eviction_reasons.insert(victim, reason);
            }
        }
        self.evictions_this_batch = self.evictions_this_batch.saturating_add(eviction_reasons.len());
        Ok(Plan { changes, directory_deltas, eviction_reasons })
    }

    fn adjust_directories(
        &self,
        key: &Key,
        amount: i64,
        deltas: &mut HashMap<DirectoryId, i64>,
        directory_count: &mut u64,
    ) -> Result<(), StorageError> {
        let Some((prefixes, shard)) = self.read_state.namespaces.directories(*key) else {
            return Err(StorageError::UnknownNamespace);
        };
        for directory in prefixes.iter().copied().chain(std::iter::once(shard)) {
            let current_delta = deltas.get(&directory).copied().unwrap_or(0);
            let existing = self.directory_counts.get(&directory).copied().unwrap_or(0);
            let current =
                i64::try_from(existing).map_err(|_| StorageError::TooLarge)?.checked_add(current_delta).ok_or(StorageError::TooLarge)?;
            let next = current.checked_add(amount).ok_or(StorageError::TooLarge)?;
            if next < 0 {
                return Err(StorageError::TooLarge);
            }
            if current == 0 && next > 0 {
                *directory_count = directory_count.checked_add(1).ok_or(StorageError::TooLarge)?;
            } else if current > 0 && next == 0 {
                *directory_count = directory_count.checked_sub(1).ok_or(StorageError::TooLarge)?;
            }
            deltas.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
            deltas.insert(directory, current_delta.checked_add(amount).ok_or(StorageError::TooLarge)?);
        }
        Ok(())
    }

    pub(super) fn prepare_changes(&mut self, plan: &Plan, protection: &mut ProtectionRegistry) -> Result<Vec<PlannedChange>, StorageError> {
        let mut changes = Vec::new();
        changes.try_reserve_exact(plan.changes.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let entries = read_map(&self.read_state.entries);
        for (key, next) in &plan.changes {
            changes.push(PlannedChange {
                key: *key,
                previous: entries.get(key).cloned(),
                next: next.clone(),
                eviction_reason: plan.eviction_reasons.get(key).copied(),
            });
        }
        drop(entries);
        sort_changes(&mut changes);
        let additional = changes.iter().filter(|change| change.next.is_some()).count();
        let mut entries = write_map(&self.read_state.entries);
        entries.try_reserve(additional).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        drop(entries);
        self.referenced_keys.try_reserve(additional).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        self.directory_counts
            .try_reserve(plan.directory_deltas.len())
            .map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        self.lru.try_reserve(additional).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        protection.sizes.try_reserve(additional).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        Ok(changes)
    }

    pub(super) fn rollback_evictions(&self, eviction_marks: &[Arc<Entry>], committed: &HashSet<Key>) {
        let protection_shared = Arc::clone(&self.read_state.protection);
        let mut protection = lock(&protection_shared.state);
        for entry in eviction_marks.iter().filter(|entry| !committed.contains(&entry.key)) {
            protection.restore_eviction(entry, &protection_shared.bytes);
        }
    }

    pub(super) fn mark_evictions(
        &self,
        changes: &[PlannedChange],
        protection: &mut ProtectionRegistry,
    ) -> Result<Vec<Arc<Entry>>, StorageError> {
        let count = changes.iter().filter(|change| change.eviction_reason.is_some()).count();
        let mut marked: Vec<Arc<Entry>> = Vec::new();
        marked.try_reserve_exact(count).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        for change in changes.iter().filter(|change| change.eviction_reason.is_some()) {
            let Some(entry) = &change.previous else {
                continue;
            };
            if !protection.begin_eviction(entry) {
                for entry in &marked {
                    protection.restore_eviction(entry, &self.read_state.protection.bytes);
                }
                return Err(StorageError::Unavailable(UnavailableReason::Io));
            }
            marked.push(Arc::clone(entry));
        }
        Ok(marked)
    }

    pub(super) fn set_limit(&mut self, limit: u64) -> Result<(), StorageError> {
        if limit < self.read_state.minimum_size_limit {
            return Err(StorageError::LimitTooSmall);
        }
        self.drain_touches_best_effort();
        self.blocked_protection_generation = None;
        self.enforce_limit(limit)?;
        self.size_limit = limit;
        self.read_state.size_limit.store(limit, Ordering::Relaxed);
        Ok(())
    }

    pub(super) fn enforce_limit(&mut self, limit: u64) -> Result<(), StorageError> {
        self.drain_touches_best_effort();
        if self.accounted_size() <= limit {
            self.blocked_protection_generation = None;
            return Ok(());
        }
        let prepared = {
            let protection_shared = Arc::clone(&self.read_state.protection);
            let mut protection = lock(&protection_shared.state);
            if self.blocked_protection_generation == Some(protection.generation) {
                return Ok(());
            }
            let entries = read_map(&self.read_state.entries);
            let mut changes = HashMap::<Key, Option<Arc<Entry>>>::new();
            let mut eviction_reasons = HashMap::new();
            eviction_reasons.try_reserve(MAX_EVICTIONS_PER_BATCH).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
            let mut directory_deltas = HashMap::<DirectoryId, i64>::new();
            let mut data_bytes = self.data_bytes;
            let mut directory_count = self.directory_count;
            let mut entry_count = self.entry_count;
            let mut no_new_order = BTreeSet::new();
            let mut no_new_stamps = HashMap::new();
            let now = self.read_state.usage_clock.load(Ordering::Relaxed);
            let remaining_evictions = MAX_EVICTIONS_PER_BATCH.saturating_sub(self.evictions_this_batch);
            let mut scan_exhausted = false;
            while !self.fits(data_bytes, entry_count, directory_count, limit)? && eviction_reasons.len() < remaining_evictions {
                let (key, reason) = match find_victim(
                    &self.lru,
                    &mut no_new_order,
                    &mut no_new_stamps,
                    None,
                    &changes,
                    &entries,
                    &protection,
                    self.read_state.policy.as_ref(),
                    now,
                    &mut self.eviction_scan_cursor,
                ) {
                    VictimSearch::Found(key, reason) => (key, reason),
                    VictimSearch::Exhausted => {
                        scan_exhausted = true;
                        break;
                    }
                };
                let entry = effective_entry(&key, &changes, &entries).ok_or(StorageError::TooLarge)?;
                self.adjust_directories(&key, -1, &mut directory_deltas, &mut directory_count)?;
                data_bytes = data_bytes.checked_sub(entry.size).ok_or(StorageError::TooLarge)?;
                entry_count = entry_count.saturating_sub(1);
                changes.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
                changes.insert(key, None);
                eviction_reasons.insert(key, reason);
            }
            drop(entries);
            if changes.is_empty() {
                self.blocked_protection_generation = (self.accounted_size() > limit && scan_exhausted).then_some(protection.generation);
                return Ok(());
            }
            let planned_evictions = eviction_reasons.len();
            let plan = Plan { changes, directory_deltas, eviction_reasons };
            let changes = self.prepare_changes(&plan, &mut protection)?;
            let eviction_marks = self.mark_evictions(&changes, &mut protection)?;
            self.evictions_this_batch = self.evictions_this_batch.saturating_add(planned_evictions);
            self.blocked_protection_generation = None;
            Some((changes, eviction_marks))
        };
        if let Some((changes, eviction_marks)) = prepared { self.commit_changes(&changes, &eviction_marks) } else { Ok(()) }
    }

    fn fits(&self, data_bytes: u64, entry_count: usize, directory_count: u64, limit: u64) -> Result<bool, StorageError> {
        if entry_count > crate::cache::index::MAX_ENTRIES {
            return Ok(false);
        }
        let index_bytes = self.index.estimated_bytes(entry_count)?;
        Ok(account_size(index_bytes, data_bytes, entry_count, directory_count).ok_or(StorageError::TooLarge)? <= limit)
    }

    pub(super) fn update_accounted_size(&self) {
        self.read_state.accounted_size.store(self.accounted_size(), Ordering::Relaxed);
    }
}

pub(super) fn low_watermark(limit: u64) -> u64 {
    let headroom = (MAX_VALUE_BYTES as u64 + 64 * 1024).min(limit / 8);
    limit.saturating_sub(headroom)
}

fn find_victim(
    lru: &Lru,
    new_order: &mut BTreeSet<(u64, Key)>,
    new_stamps: &mut HashMap<Key, u64>,
    excluded: Option<Key>,
    changes: &HashMap<Key, Option<Arc<Entry>>>,
    entries: &HashMap<Key, Arc<Entry>>,
    protection: &ProtectionRegistry,
    policy: &dyn EvictionPolicy,
    now: u64,
    scan_cursor: &mut Option<(u64, Key)>,
) -> VictimSearch {
    let candidate_count = lru.len().saturating_add(new_order.len());
    let mut inspected = 0usize;
    loop {
        match scan_victim(
            lru,
            new_order,
            new_stamps,
            excluded,
            changes,
            entries,
            protection,
            policy,
            now,
            scan_cursor,
            &mut inspected,
            candidate_count,
        ) {
            ScanResult::Found(key, reason) => return VictimSearch::Found(key, reason),
            ScanResult::Exhausted => {
                *scan_cursor = None;
                return VictimSearch::Exhausted;
            }
            ScanResult::Deferred if inspected < candidate_count => {}
            ScanResult::Deferred => {
                *scan_cursor = None;
                return VictimSearch::Exhausted;
            }
        }
    }
}

fn scan_victim(
    lru: &Lru,
    new_order: &mut BTreeSet<(u64, Key)>,
    new_stamps: &mut HashMap<Key, u64>,
    excluded: Option<Key>,
    changes: &HashMap<Key, Option<Arc<Entry>>>,
    entries: &HashMap<Key, Arc<Entry>>,
    protection: &ProtectionRegistry,
    policy: &dyn EvictionPolicy,
    now: u64,
    scan_cursor: &mut Option<(u64, Key)>,
    search_inspected: &mut usize,
    candidate_count: usize,
) -> ScanResult {
    let mut base_scan = lru.scan_after(*scan_cursor).peekable();
    let mut new_scan = OrderedScan::new(new_order, *scan_cursor).peekable();
    let mut selected: Option<(u128, u64, Key, EvictionReason, bool)> = None;
    let mut turn_inspected = 0usize;
    while turn_inspected < MAX_EVICTION_CANDIDATES && *search_inspected < candidate_count {
        let next_base = base_scan.peek().copied();
        let next_new = new_scan.peek().copied();
        let (stamp, key, is_new) = match (next_base, next_new) {
            (Some(base_candidate), Some(new_candidate)) if base_candidate <= new_candidate => {
                base_scan.next();
                (base_candidate.0, base_candidate.1, false)
            }
            (_, Some(new_candidate)) => {
                new_scan.next();
                (new_candidate.0, new_candidate.1, true)
            }
            (Some(base_candidate), None) => {
                base_scan.next();
                (base_candidate.0, base_candidate.1, false)
            }
            (None, None) => break,
        };
        turn_inspected += 1;
        *search_inspected += 1;
        *scan_cursor = Some((stamp, key));
        let stale = if is_new {
            changes.get(&key).is_some_and(Option::is_none) || effective_entry(&key, changes, entries).is_none()
        } else {
            changes.contains_key(&key) || effective_entry(&key, changes, entries).is_none()
        };
        if stale || Some(key) == excluded || protection.is_active(&key) {
            continue;
        }
        let Some(entry) = effective_entry(&key, changes, entries) else {
            continue;
        };
        let candidate = EvictionCandidate {
            key,
            size: entry.size,
            last_use: entry.last_use.load(Ordering::Relaxed),
            use_count: entry.use_count.load(Ordering::Relaxed),
        };
        let score = policy.score(candidate, now);
        let rank = (score.value, stamp, key, score.reason, is_new);
        if selected.is_none_or(|current| (rank.0, rank.1, rank.2) < (current.0, current.1, current.2)) {
            selected = Some(rank);
        }
    }
    let has_more = base_scan.peek().is_some() || new_scan.peek().is_some();
    let Some((_, stamp, key, reason, is_new)) = selected else {
        return if has_more && *search_inspected < candidate_count { ScanResult::Deferred } else { ScanResult::Exhausted };
    };
    drop(new_scan);
    drop(base_scan);
    if is_new {
        remove_new_candidate(&key, new_order, new_stamps);
    }
    *scan_cursor = Some((stamp, key));
    ScanResult::Found(key, reason)
}
