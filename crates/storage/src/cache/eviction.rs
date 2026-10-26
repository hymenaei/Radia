use std::collections::{BTreeSet, HashMap};

use super::Key;

#[derive(Default)]
pub(super) struct Lru {
    order: BTreeSet<(u64, Key)>,
    stamps: HashMap<Key, u64>,
}

impl Lru {
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

    pub fn oldest(&self) -> impl Iterator<Item = (u64, Key)> + '_ {
        self.order.iter().cloned()
    }

    pub fn clear(&mut self) {
        self.order.clear();
        self.stamps.clear();
    }
}
