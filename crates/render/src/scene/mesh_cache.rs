use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashMap},
    mem::size_of,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use assets::mesh::Mesh;
use utils::AssetId;
use world::{PrimShape, SculptFlags, SculptType};

use crate::scene::{
    Budget, Detail, MeshBuildError, SharedMesh,
    budget::{MEMORY_BYTES, MeshWork},
    mesh::retained_bytes,
    prim::tessellate,
};

const CACHE_BYTES: usize = MEMORY_BYTES;
const HASH_ENTRY_OVERHEAD_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshKey {
    pub shape: PrimShape,
    pub detail: Detail,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SculptMeshKey {
    pub asset_id: AssetId,
    pub sculpt_type: SculptType,
    pub flags: SculptFlags,
    pub detail: Detail,
    pub map_size: [u32; 2],
    pub discard_level: u8,
}

#[derive(Clone)]
pub enum PreparedMesh {
    Pending,
    Ready(SharedMesh),
    Failed(MeshBuildError),
    Capacity,
}

pub(in crate::scene) struct PrimJob {
    key: MeshKey,
    work: MeshWork,
    cancelled: Arc<AtomicBool>,
}

impl PrimJob {
    pub fn run(self) -> PrimCompletion {
        let result = tessellate(self.key.shape, self.key.detail, &self.cancelled);
        PrimCompletion { key: self.key, result, work: self.work, cancelled: self.cancelled }
    }
}

pub(in crate::scene) struct PrimCompletion {
    key: MeshKey,
    result: Result<Mesh, MeshBuildError>,
    work: MeshWork,
    cancelled: Arc<AtomicBool>,
}

enum State {
    Queued,
    Blocked(usize),
    Running(Weak<AtomicBool>),
    Ready(SharedMesh),
    Failed(MeshBuildError),
}

struct Entry {
    wanted: usize,
    touched: u64,
    priority: u32,
    state: State,
    queued: Option<(Reverse<u32>, u64)>,
    blocked: Option<(Reverse<u32>, u64)>,
    ready: Option<(u32, u64)>,
    unused: Option<u64>,
}

const ENTRY_BYTES: usize = size_of::<MeshKey>() + size_of::<Entry>() + HASH_ENTRY_OVERHEAD_BYTES;

pub(in crate::scene) struct MeshCache {
    budget: Budget,
    entries: HashMap<MeshKey, Entry>,
    queued: BTreeMap<(Reverse<u32>, u64), MeshKey>,
    blocked: BTreeMap<(Reverse<u32>, u64), MeshKey>,
    ready: BTreeMap<(u32, u64), MeshKey>,
    unused: BTreeMap<u64, MeshKey>,
    running: Vec<MeshKey>,
    evicted: Vec<MeshKey>,
    updated: Vec<MeshKey>,
    bytes: usize,
    clock: u64,
}

impl MeshCache {
    pub(in crate::scene) fn new(budget: Budget) -> Self {
        Self {
            budget,
            entries: HashMap::new(),
            queued: BTreeMap::new(),
            blocked: BTreeMap::new(),
            ready: BTreeMap::new(),
            unused: BTreeMap::new(),
            running: Vec::new(),
            evicted: Vec::new(),
            updated: Vec::new(),
            bytes: 0,
            clock: 0,
        }
    }

    pub(in crate::scene) fn request(&mut self, key: MeshKey, priority: u32) -> bool {
        self.clock = self.clock.checked_add(1).expect("mesh cache clock exhausted");
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.wanted += 1;
            entry.touched = self.clock;
            entry.priority = entry.priority.max(priority);
            self.reindex(key);
            return true;
        }
        while !self.has_room(ENTRY_BYTES) && self.evict(priority) {}
        if !self.has_room(ENTRY_BYTES) {
            return false;
        }
        self.entries.insert(
            key,
            Entry {
                wanted: 1,
                touched: self.clock,
                priority,
                state: State::Queued,
                queued: None,
                blocked: None,
                ready: None,
                unused: None,
            },
        );
        self.bytes += ENTRY_BYTES;
        self.reindex(key);
        true
    }

    pub(in crate::scene) fn reset_priorities(&mut self) {
        self.queued.clear();
        self.blocked.clear();
        self.ready.clear();
        self.unused.clear();
        let mut keys = Vec::new();
        for entry in self.entries.values_mut() {
            entry.priority = 0;
            entry.queued = None;
            entry.blocked = None;
            entry.ready = None;
            entry.unused = None;
        }
        for (key, entry) in &self.entries {
            if matches!(&entry.state, State::Queued | State::Blocked(_) | State::Ready(_)) {
                keys.push(*key);
            }
        }
        for key in keys {
            self.reindex(key);
        }
    }

    pub(in crate::scene) fn prioritize(&mut self, key: MeshKey, priority: u32) {
        if let Some(entry) = self.entries.get_mut(&key)
            && priority > entry.priority
        {
            entry.priority = priority;
            self.reindex(key);
        }
    }

    fn reindex(&mut self, key: MeshKey) {
        let Some(entry) = self.entries.get_mut(&key) else { return };
        let previous_queued = entry.queued.take();
        let previous_blocked = entry.blocked.take();
        let previous_ready = entry.ready.take();
        let previous_unused = entry.unused.take();
        let priority = entry.priority;
        let touched = entry.touched;
        let queued = matches!(&entry.state, State::Queued);
        let blocked = matches!(&entry.state, State::Blocked(_));
        let ready = matches!(&entry.state, State::Ready(_));
        if let Some(index) = previous_queued {
            self.queued.remove(&index);
        }
        if let Some(index) = previous_blocked {
            self.blocked.remove(&index);
        }
        if let Some(index) = previous_ready {
            self.ready.remove(&index);
        }
        if let Some(index) = previous_unused {
            self.unused.remove(&index);
        }
        if queued {
            let index = (Reverse(priority), touched);
            self.queued.insert(index, key);
            self.entries.get_mut(&key).expect("indexed mesh remains cached").queued = Some(index);
        } else if blocked {
            let index = (Reverse(priority), touched);
            self.blocked.insert(index, key);
            self.entries.get_mut(&key).expect("indexed mesh remains cached").blocked = Some(index);
        } else if ready {
            let index = (priority, touched);
            self.ready.insert(index, key);
            let entry = self.entries.get_mut(&key).expect("indexed mesh remains cached");
            entry.ready = Some(index);
            if entry.wanted == 0 {
                self.unused.insert(touched, key);
                self.entries.get_mut(&key).expect("indexed mesh remains cached").unused = Some(touched);
            }
        }
    }

    fn remove_indexes(&mut self, key: MeshKey) {
        let Some(entry) = self.entries.get_mut(&key) else { return };
        if let Some(index) = entry.queued.take() {
            self.queued.remove(&index);
        }
        if let Some(index) = entry.blocked.take() {
            self.blocked.remove(&index);
        }
        if let Some(index) = entry.ready.take() {
            self.ready.remove(&index);
        }
        if let Some(index) = entry.unused.take() {
            self.unused.remove(&index);
        }
    }

    pub(in crate::scene) fn release(&mut self, key: MeshKey) {
        let Some(entry) = self.entries.get_mut(&key) else {
            return;
        };
        debug_assert!(entry.wanted > 0);
        entry.wanted -= 1;
        if entry.wanted == 0 {
            entry.priority = 0;
        }
        let remove = entry.wanted == 0 && matches!(entry.state, State::Queued | State::Running(_) | State::Blocked(_));
        if remove {
            if let Some(entry) = self.entries.get(&key)
                && let State::Running(cancelled) = &entry.state
                && let Some(cancelled) = cancelled.upgrade()
            {
                cancelled.store(true, Ordering::Release);
            }
            self.remove_indexes(key);
            self.running.retain(|running| *running != key);
            self.entries.remove(&key);
            self.bytes -= ENTRY_BYTES;
            self.shrink_if_sparse();
        } else {
            self.reindex(key);
        }
    }

    pub(in crate::scene) fn get(&self, key: MeshKey) -> PreparedMesh {
        match self.entries.get(&key).map(|entry| &entry.state) {
            Some(State::Ready(mesh)) => PreparedMesh::Ready(mesh.clone()),
            Some(State::Failed(error)) => PreparedMesh::Failed(*error),
            Some(State::Queued | State::Running(_)) => PreparedMesh::Pending,
            Some(State::Blocked(_)) => PreparedMesh::Capacity,
            None => PreparedMesh::Capacity,
        }
    }

    pub(in crate::scene) fn start(&mut self) -> Option<PrimJob> {
        let mut running = Vec::with_capacity(self.running.len());
        for key in std::mem::take(&mut self.running) {
            let Some(entry) = self.entries.get_mut(&key) else { continue };
            if let State::Running(cancelled) = &entry.state {
                if cancelled.upgrade().is_some() {
                    running.push(key);
                } else {
                    entry.state = State::Queued;
                    self.reindex(key);
                }
            }
        }
        self.running = running;
        if self.queued.is_empty()
            && let Some((_, key)) = self.blocked.first_key_value().map(|(index, key)| (*index, *key))
        {
            let Some(State::Blocked(bytes)) = self.entries.get(&key).map(|entry| &entry.state) else { return None };
            let bytes = *bytes;
            let priority = self.entries.get(&key).map_or(0, |entry| entry.priority);
            while !self.has_room(bytes) && self.evict(priority) {}
            if self.has_room(bytes) {
                self.entries.get_mut(&key).expect("blocked mesh remains cached").state = State::Queued;
                self.reindex(key);
            } else {
                return None;
            }
        }
        if self.queued.is_empty() {
            return None;
        }
        let work = self.budget.mesh_work()?;
        let (_, key) = self.queued.pop_first()?;
        self.entries.get_mut(&key).expect("selected queue entry").queued = None;
        let cancelled = Arc::new(AtomicBool::new(false));
        self.entries.get_mut(&key).expect("selected queue entry").state = State::Running(Arc::downgrade(&cancelled));
        self.running.push(key);
        Some(PrimJob { key, work, cancelled })
    }

    pub(in crate::scene) fn waiting(&self) -> bool {
        !self.queued.is_empty() || !self.running.is_empty()
    }

    pub(in crate::scene) fn has_updates(&self) -> bool {
        !self.evicted.is_empty() || !self.updated.is_empty()
    }

    pub(in crate::scene) fn take_evicted(&mut self) -> Vec<MeshKey> {
        std::mem::take(&mut self.evicted)
    }

    pub(in crate::scene) fn take_updated(&mut self) -> Vec<MeshKey> {
        std::mem::take(&mut self.updated)
    }

    pub(in crate::scene) fn complete(&mut self, mut completion: PrimCompletion) {
        let Some(entry) = self.entries.get(&completion.key) else {
            return;
        };
        if !matches!(
            &entry.state,
            State::Running(cancelled) if Weak::ptr_eq(cancelled, &Arc::downgrade(&completion.cancelled))
        ) {
            return;
        }
        self.running.retain(|key| *key != completion.key);
        let state = match completion.result {
            Ok(mesh) => {
                let bytes = retained_bytes(&mesh);
                while !self.has_room(bytes) {
                    if !self.evict(self.entries.get(&completion.key).map_or(0, |entry| entry.priority)) {
                        self.entries.get_mut(&completion.key).expect("matched completion").state = State::Blocked(bytes);
                        self.reindex(completion.key);
                        self.evicted.push(completion.key);
                        return;
                    }
                }
                let lease = completion.work.lease.split(bytes).expect("bounded mesh fits admitted work");
                self.bytes += bytes;
                State::Ready(SharedMesh::new(mesh, lease))
            }
            Err(MeshBuildError::Cancelled) => State::Queued,
            Err(error) => State::Failed(error),
        };
        self.entries.get_mut(&completion.key).expect("matched completion").state = state;
        self.reindex(completion.key);
        self.updated.push(completion.key);
    }

    fn evict(&mut self, incoming_priority: u32) -> bool {
        let lower_priority = self.ready.first_key_value().and_then(|((priority, _), key)| (*priority < incoming_priority).then_some(*key));
        let idle = self.unused.first_key_value().map(|(_, key)| *key);
        let key = match (lower_priority, idle) {
            (Some(a), Some(b)) => {
                let a_entry = self.entries.get(&a).expect("ready entry remains cached");
                let b_entry = self.entries.get(&b).expect("idle entry remains cached");
                if (a_entry.priority, a_entry.touched) <= (b_entry.priority, b_entry.touched) { a } else { b }
            }
            (Some(key), None) | (None, Some(key)) => key,
            (None, None) => return false,
        };
        let Some(entry) = self.entries.get(&key) else {
            return false;
        };
        let wanted = entry.wanted;
        let retained = match &entry.state {
            State::Ready(mesh) => retained_bytes(mesh.mesh()),
            _ => return false,
        };
        if wanted == 0 {
            self.remove_indexes(key);
            self.entries.remove(&key);
            self.bytes -= ENTRY_BYTES + retained;
        } else {
            self.entries.get_mut(&key).expect("selected eviction remains cached").state = State::Blocked(retained);
            self.bytes -= retained;
            self.reindex(key);
            self.evicted.push(key);
        }
        self.shrink_if_sparse();
        true
    }

    fn has_room(&self, bytes: usize) -> bool {
        self.bytes.checked_add(bytes).is_some_and(|used| used <= CACHE_BYTES)
    }

    fn shrink_if_sparse(&mut self) {
        if self.entries.capacity() > self.entries.len().saturating_mul(2) {
            self.entries.shrink_to_fit();
        }
    }
}

impl Drop for MeshCache {
    fn drop(&mut self) {
        for entry in self.entries.values() {
            if let State::Running(cancelled) = &entry.state
                && let Some(cancelled) = cancelled.upgrade()
            {
                cancelled.store(true, Ordering::Release);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(detail: Detail) -> MeshKey {
        MeshKey { shape: PrimShape::default(), detail }
    }

    #[test]
    fn visible_work_overtakes_background() {
        let mut meshes = MeshCache::new(Budget::default());
        for detail in [Detail::Lowest, Detail::Low, Detail::Medium, Detail::High] {
            assert!(meshes.request(key(detail), 0));
        }
        meshes.prioritize(key(Detail::Low), 20);
        meshes.prioritize(key(Detail::High), 200);
        meshes.prioritize(key(Detail::High), 5);
        let first = meshes.start().unwrap();
        assert_eq!(first.key, key(Detail::High), "shared geometry keeps its greatest on-screen demand");
        meshes.complete(first.run());
        meshes.reset_priorities();
        meshes.prioritize(key(Detail::Medium), 100);
        let second = meshes.start().unwrap();
        assert_eq!(second.key, key(Detail::Medium), "camera changes replace previous priorities");
        meshes.complete(second.run());
        let third = meshes.start().unwrap();
        assert_eq!(third.key, key(Detail::Lowest), "equal priorities retain request order");
        meshes.complete(third.run());
        let last = meshes.start().unwrap();
        assert_eq!(last.key, key(Detail::Low));
        meshes.complete(last.run());
        assert!(!meshes.waiting(), "background geometry still completes");
    }

    #[test]
    fn shared_admission_and_retry() {
        let budget = Budget::default();
        let mut a = MeshCache::new(budget.clone());
        let mut b = MeshCache::new(budget.clone());
        for detail in [Detail::Low, Detail::High] {
            assert!(a.request(key(detail), 0));
        }
        assert!(b.request(key(Detail::Medium), 0));
        let first = a.start().unwrap();
        let second = a.start().unwrap();
        assert!(b.start().is_none());
        assert_eq!(budget.charged_bytes(), 8 * 1024 * 1024);
        drop(first);
        let other = b.start().unwrap();
        assert!(a.start().is_none());
        drop(second);
        let retried = a.start().unwrap();
        a.complete(retried.run());
        assert!(matches!(a.get(key(Detail::Low)), PreparedMesh::Ready(_)));
        drop(other);
        let retained = budget.charged_bytes();
        let completion = a.start().unwrap().run();
        let completion_charge = budget.charged_bytes();
        assert!(completion_charge > retained, "an undelivered completion keeps its reservation");
        assert!(a.start().is_none(), "a live completion must keep its cache entry running");
        drop(completion);
        assert_eq!(budget.charged_bytes(), retained, "dropping an undelivered completion releases its reservation");
        let retried = a.start().expect("dropping a completion makes its work retryable");
        a.complete(retried.run());
        assert!(matches!(a.get(key(Detail::High)), PreparedMesh::Ready(_)));
        drop(a);
        drop(b);
        assert_eq!(budget.charged_bytes(), 0);
    }

    #[test]
    fn stale_jobs_and_retained_charge() {
        let budget = Budget::default();
        let mut meshes = MeshCache::new(budget.clone());
        let key = key(Detail::High);
        assert!(meshes.request(key, 0));
        let old = meshes.start().unwrap().run();
        meshes.release(key);
        assert!(meshes.request(key, 0));
        let new = meshes.start().unwrap();
        meshes.complete(old);
        assert!(matches!(meshes.get(key), PreparedMesh::Pending));
        meshes.complete(new.run());
        let PreparedMesh::Ready(retained) = meshes.get(key) else {
            panic!("expected ready mesh");
        };
        let charge = budget.charged_bytes();
        assert!(charge > retained.mesh().owned_bytes() && charge < 4 * 1024 * 1024);
        meshes.release(key);
        drop(meshes);
        assert_eq!(budget.charged_bytes(), charge);
        let mut replacement = MeshCache::new(budget.clone());
        assert!(replacement.request(key, 0));
        let job = replacement.start().unwrap();
        replacement.complete(job.run());
        let PreparedMesh::Ready(rebuilt) = replacement.get(key) else {
            panic!("expected rebuilt mesh");
        };
        assert_ne!(retained.id(), rebuilt.id(), "rebuilding a source key must not alias a retained GPU resource");
        drop(replacement);
        drop(rebuilt);
        assert_eq!(budget.charged_bytes(), charge);
        drop(retained);
        assert_eq!(budget.charged_bytes(), 0);
    }

    #[test]
    fn negative_cache_is_reused() {
        let mut meshes = MeshCache::new(Budget::default());
        let extra = MeshKey { shape: PrimShape { profile_curve: 15, ..PrimShape::default() }, detail: Detail::High };
        assert!(meshes.request(extra, 0));
        let job = meshes.start().expect("unsupported mesh work starts");
        meshes.complete(job.run());
        assert!(matches!(meshes.get(extra), PreparedMesh::Failed(MeshBuildError::UnsupportedShape)));
        assert!(meshes.request(extra, 0));
        assert!(meshes.start().is_none());
    }

    #[test]
    fn higher_priority_displaces_lower_ready_mesh() {
        let mut meshes = MeshCache::new(Budget::default());
        let background = key(Detail::Lowest);
        let foreground = key(Detail::High);
        assert!(meshes.request(background, 5));
        let job = meshes.start().unwrap();
        meshes.complete(job.run());
        assert!(matches!(meshes.get(background), PreparedMesh::Ready(_)));
        assert!(meshes.request(foreground, 500));

        assert!(meshes.evict(500));
        assert!(matches!(meshes.get(background), PreparedMesh::Capacity));
        assert_eq!(meshes.take_evicted(), vec![background]);
        assert!(matches!(meshes.get(foreground), PreparedMesh::Pending));
    }

    #[test]
    fn cache_keeps_many_unique_meshes_resident_within_memory_budget() {
        let budget = Budget::default();
        let mut meshes = MeshCache::new(budget.clone());
        let keys: Vec<_> = (0..128)
            .map(|profile_begin| MeshKey {
                shape: PrimShape {
                    path_curve: 0x20,
                    profile_curve: 0,
                    path_scale_x: 150,
                    path_scale_y: 150,
                    path_twist_begin: -100,
                    path_twist: 100,
                    path_revolutions: 200,
                    path_skew: 95,
                    profile_begin,
                    ..PrimShape::default()
                },
                detail: Detail::High,
            })
            .collect();
        for key in &keys {
            assert!(meshes.request(*key, 0));
        }
        for _ in 0..keys.len() {
            let Some(job) = meshes.start() else {
                break;
            };
            meshes.complete(job.run());
        }
        assert!(keys.iter().all(|key| matches!(meshes.get(*key), PreparedMesh::Ready(_))));
        assert!(meshes.start().is_none(), "resident meshes do not repeatedly decode");
        assert!(meshes.bytes <= CACHE_BYTES);
        let key = *keys.iter().find(|key| matches!(meshes.get(**key), PreparedMesh::Ready(_))).unwrap();
        meshes.release(key);
        assert!(meshes.start().is_none(), "releasing a resident mesh within budget does not evict it");
        assert!(meshes.bytes <= CACHE_BYTES);
        drop(meshes);
        assert_eq!(budget.charged_bytes(), 0);
    }
}
