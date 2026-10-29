use std::{
    collections::{BTreeSet, HashMap, btree_set::Range},
    ops::Bound::{Excluded, Unbounded},
};

use super::Key;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvictionCandidate {
    pub key: Key,
    pub size: u64,
    pub last_use: u64,
    pub use_count: u64,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvictionReason {
    Infrequent = 1,
    Stale = 2,
    LowReconstructionCost = 3,
    LeastRecentlyUsed = 4,
    LowUtility = 5,
}

impl EvictionReason {
    pub(super) fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Infrequent),
            2 => Some(Self::Stale),
            3 => Some(Self::LowReconstructionCost),
            4 => Some(Self::LeastRecentlyUsed),
            5 => Some(Self::LowUtility),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvictionScore {
    pub value: u128,
    pub reason: EvictionReason,
}

pub trait EvictionPolicy: Send + Sync {
    fn score(&self, candidate: EvictionCandidate, now: u64) -> EvictionScore;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LruPolicy;

impl EvictionPolicy for LruPolicy {
    fn score(&self, candidate: EvictionCandidate, _now: u64) -> EvictionScore {
        EvictionScore { value: u128::from(candidate.last_use), reason: EvictionReason::LeastRecentlyUsed }
    }
}

#[derive(Default)]
pub(super) struct Lru {
    order: BTreeSet<(u64, Key)>,
    stamps: HashMap<Key, u64>,
}

pub(super) struct OrderedScan<'a> {
    after: Range<'a, (u64, Key)>,
    before: Option<Range<'a, (u64, Key)>>,
}

impl<'a> OrderedScan<'a> {
    pub fn new(order: &'a BTreeSet<(u64, Key)>, cursor: Option<(u64, Key)>) -> Self {
        match cursor {
            Some(cursor) => Self { after: order.range((Excluded(cursor), Unbounded)), before: Some(order.range(..=cursor)) },
            None => Self { after: order.range(..), before: None },
        }
    }
}

impl Iterator for OrderedScan<'_> {
    type Item = (u64, Key);

    fn next(&mut self) -> Option<Self::Item> {
        self.after.next().or_else(|| self.before.as_mut().and_then(|before| before.next())).copied()
    }
}

impl Lru {
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn try_reserve(&mut self, additional: usize) -> Result<(), std::collections::TryReserveError> {
        self.stamps.try_reserve(additional)
    }

    pub fn insert(&mut self, key: Key, stamp: u64) {
        self.remove(&key);
        self.order.insert((stamp, key));
        self.stamps.insert(key, stamp);
    }

    pub fn remove(&mut self, key: &Key) {
        if let Some(stamp) = self.stamps.remove(key) {
            self.order.remove(&(stamp, *key));
        }
    }

    pub fn scan_after(&self, cursor: Option<(u64, Key)>) -> OrderedScan<'_> {
        OrderedScan::new(&self.order, cursor)
    }

    pub fn clear(&mut self) {
        self.order.clear();
        self.stamps.clear();
    }
}
