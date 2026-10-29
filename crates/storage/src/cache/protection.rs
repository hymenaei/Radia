use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use super::{ENTRY_EVICTING, ENTRY_VALID, Entry, EvictionReason, Key, StorageError, UnavailableReason};

pub(super) const EVICTION_HISTORY_LEN: usize = 8;

pub struct Protection {
    pub(super) shared: Arc<ProtectionShared>,
    pub(super) key: Key,
}

impl Drop for Protection {
    fn drop(&mut self) {
        let mut state = lock_protection(&self.shared.state);
        state.release(self.key, &self.shared.bytes);
    }
}

#[derive(Default)]
struct ActiveProtection {
    count: usize,
    charged_size: Option<u64>,
}

#[derive(Default)]
pub(super) struct ProtectionRegistry {
    active: HashMap<Key, ActiveProtection>,
    pub(super) sizes: HashMap<Key, u64>,
    evicting: HashSet<Key>,
    pub(super) generation: u64,
}

impl ProtectionRegistry {
    pub(super) fn acquire(&mut self, key: Key, bytes: &AtomicU64) -> Result<(), StorageError> {
        if let Some(active) = self.active.get_mut(&key) {
            active.count = active.count.checked_add(1).ok_or(StorageError::TooLarge)?;
        } else {
            self.active.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
            let charged_size = if self.evicting.contains(&key) { None } else { self.sizes.get(&key).copied() };
            if let Some(size) = charged_size {
                bytes.fetch_add(size, Ordering::AcqRel);
            }
            self.active.insert(key, ActiveProtection { count: 1, charged_size });
        }
        self.generation = self.generation.saturating_add(1);
        Ok(())
    }

    pub(super) fn release(&mut self, key: Key, bytes: &AtomicU64) {
        let Some(active) = self.active.get_mut(&key) else {
            return;
        };
        if active.count > 1 {
            active.count -= 1;
        } else {
            if let Some(active) = self.active.remove(&key)
                && let Some(size) = active.charged_size
            {
                bytes.fetch_sub(size, Ordering::AcqRel);
            }
        }
        self.generation = self.generation.saturating_add(1);
    }

    pub(super) fn update_size(&mut self, key: Key, size: Option<u64>, bytes: &AtomicU64) {
        self.sizes.remove(&key);
        let is_evicting = self.evicting.contains(&key);
        if let Some(active) = self.active.get_mut(&key) {
            if let Some(charged_size) = active.charged_size.take() {
                bytes.fetch_sub(charged_size, Ordering::AcqRel);
            }
            let next_charge = if is_evicting { None } else { size };
            if let Some(size) = next_charge {
                bytes.fetch_add(size, Ordering::AcqRel);
            }
            active.charged_size = next_charge;
        }
        if let Some(size) = size {
            self.sizes.insert(key, size);
        }
        if size.is_none() {
            self.evicting.remove(&key);
        }
        self.generation = self.generation.saturating_add(1);
    }

    pub(super) fn is_active(&self, key: &Key) -> bool {
        self.active.contains_key(key)
    }

    pub(super) fn begin_eviction(&mut self, entry: &Entry) -> bool {
        if self.active.contains_key(&entry.key) || self.evicting.contains(&entry.key) || self.evicting.try_reserve(1).is_err() {
            return false;
        }
        if entry.state.compare_exchange(ENTRY_VALID, ENTRY_EVICTING, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return false;
        }
        self.evicting.insert(entry.key);
        self.generation = self.generation.saturating_add(1);
        true
    }

    pub(super) fn restore_eviction(&mut self, entry: &Entry, bytes: &AtomicU64) {
        if !self.evicting.remove(&entry.key) {
            return;
        }
        entry.state.store(ENTRY_VALID, Ordering::Release);
        if let Some(active) = self.active.get_mut(&entry.key)
            && active.charged_size.is_none()
            && let Some(size) = self.sizes.get(&entry.key).copied()
        {
            active.charged_size = Some(size);
            bytes.fetch_add(size, Ordering::AcqRel);
        }
        self.generation = self.generation.saturating_add(1);
    }
}

pub(super) struct ProtectionShared {
    pub(super) state: Mutex<ProtectionRegistry>,
    pub(super) bytes: AtomicU64,
}

pub(super) struct EvictionHistory {
    sequence: AtomicU64,
    reasons: [AtomicU64; EVICTION_HISTORY_LEN],
}

impl Default for EvictionHistory {
    fn default() -> Self {
        Self { sequence: AtomicU64::new(0), reasons: std::array::from_fn(|_| AtomicU64::new(0)) }
    }
}

impl EvictionHistory {
    pub(super) fn record(&self, reason: EvictionReason) {
        let sequence = self.sequence.fetch_add(1, Ordering::AcqRel).saturating_add(1);
        let slot = usize::try_from(sequence.saturating_sub(1) % EVICTION_HISTORY_LEN as u64).unwrap_or(0);
        let packed = sequence.saturating_mul(256).saturating_add(u64::from(reason as u8));
        if let Some(value) = self.reasons.get(slot) {
            value.store(packed, Ordering::Release);
        }
    }

    pub(super) fn snapshot(&self) -> [Option<EvictionReason>; EVICTION_HISTORY_LEN] {
        let latest = self.sequence.load(Ordering::Acquire);
        let first = latest.saturating_sub(EVICTION_HISTORY_LEN as u64 - 1).max(1);
        let mut reasons = [None; EVICTION_HISTORY_LEN];
        for sequence in first..=latest {
            let slot = usize::try_from(sequence.saturating_sub(1) % EVICTION_HISTORY_LEN as u64).unwrap_or(0);
            let packed = self.reasons.get(slot).map_or(0, |value| value.load(Ordering::Acquire));
            if packed / 256 == sequence {
                let index = usize::try_from(sequence - first).unwrap_or(0);
                if let Some(reason) = reasons.get_mut(index) {
                    *reason = EvictionReason::from_code((packed % 256) as u8);
                }
            }
        }
        reasons
    }
}

pub(super) fn lock_protection(state: &Mutex<ProtectionRegistry>) -> std::sync::MutexGuard<'_, ProtectionRegistry> {
    match state.lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    }
}
