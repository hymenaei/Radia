use std::{
    cmp::Reverse,
    collections::HashMap,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use assets::mesh::Mesh;
use utils::AssetId;
use world::{PrimShape, SculptFlags, SculptType};

use crate::scene::{Budget, Detail, MeshBuildError, SharedMesh, budget::MeshWork, mesh::retained_bytes, prim::tessellate};

const MAX_ENTRIES: usize = 1024;
const CACHE_BYTES: usize = 32 * 1024 * 1024;

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
}

pub(in crate::scene) struct MeshCache {
    budget: Budget,
    entries: HashMap<MeshKey, Entry>,
    bytes: usize,
    clock: u64,
}

impl MeshCache {
    pub(in crate::scene) fn new(budget: Budget) -> Self {
        Self { budget, entries: HashMap::new(), bytes: 0, clock: 0 }
    }

    pub(in crate::scene) fn request(&mut self, key: MeshKey) -> bool {
        self.clock = self.clock.checked_add(1).expect("mesh cache clock exhausted");
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.wanted += 1;
            entry.touched = self.clock;
            return true;
        }
        if self.entries.len() == MAX_ENTRIES && !self.evict() {
            return false;
        }
        self.entries.insert(key, Entry { wanted: 1, touched: self.clock, priority: 0, state: State::Queued });
        true
    }

    pub(in crate::scene) fn reset_priorities(&mut self) {
        for entry in self.entries.values_mut() {
            entry.priority = 0;
        }
    }

    pub(in crate::scene) fn prioritize(&mut self, key: MeshKey, priority: u32) {
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.priority = entry.priority.max(priority);
        }
    }

    pub(in crate::scene) fn release(&mut self, key: MeshKey) {
        let Some(entry) = self.entries.get_mut(&key) else {
            return;
        };
        debug_assert!(entry.wanted > 0);
        entry.wanted -= 1;
        if entry.wanted == 0 && matches!(entry.state, State::Queued | State::Running(_) | State::Blocked(_)) {
            if let State::Running(cancelled) = &entry.state
                && let Some(cancelled) = cancelled.upgrade()
            {
                cancelled.store(true, Ordering::Release);
            }
            self.entries.remove(&key);
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
        for entry in self.entries.values_mut() {
            if let State::Running(cancelled) = &entry.state
                && cancelled.upgrade().is_none()
            {
                entry.state = State::Queued;
            }
        }
        let blocked: Vec<_> = self
            .entries
            .iter()
            .filter_map(|(key, entry)| if let State::Blocked(bytes) = entry.state { Some((*key, bytes)) } else { None })
            .collect();
        for (key, bytes) in blocked {
            while self.bytes + bytes > CACHE_BYTES && self.evict() {}
            if self.bytes + bytes <= CACHE_BYTES {
                self.entries.get_mut(&key).expect("blocked entry remains wanted").state = State::Queued;
            }
        }
        let key = self
            .entries
            .iter()
            .filter(|(_, e)| matches!(e.state, State::Queued))
            .min_by_key(|(_, e)| (Reverse(e.priority), e.touched))
            .map(|(key, _)| *key)?;
        let work = self.budget.mesh_work()?;
        let cancelled = Arc::new(AtomicBool::new(false));
        self.entries.get_mut(&key).expect("selected queue entry").state = State::Running(Arc::downgrade(&cancelled));
        Some(PrimJob { key, work, cancelled })
    }

    pub(in crate::scene) fn waiting(&self) -> bool {
        self.entries.values().any(|entry| matches!(entry.state, State::Queued | State::Running(_) | State::Blocked(_)))
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
        let state = match completion.result {
            Ok(mesh) => {
                let bytes = retained_bytes(&mesh);
                while self.bytes + bytes > CACHE_BYTES {
                    if !self.evict() {
                        self.entries.get_mut(&completion.key).expect("matched completion").state = State::Blocked(bytes);
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
    }

    fn evict(&mut self) -> bool {
        let Some(key) =
            self.entries.iter().filter(|(_, entry)| entry.wanted == 0).min_by_key(|(_, entry)| entry.touched).map(|(key, _)| *key)
        else {
            return false;
        };
        if let State::Ready(mesh) = self.entries.remove(&key).expect("selected eviction").state {
            self.bytes -= retained_bytes(mesh.mesh());
        }
        true
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
            assert!(meshes.request(key(detail)));
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
            assert!(a.request(key(detail)));
        }
        assert!(b.request(key(Detail::Medium)));
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
        assert!(meshes.request(key));
        let old = meshes.start().unwrap().run();
        meshes.release(key);
        assert!(meshes.request(key));
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
        assert!(replacement.request(key));
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
    fn entry_limit_and_negative_cache() {
        let mut meshes = MeshCache::new(Budget::default());
        for begin in 0..MAX_ENTRIES {
            assert!(
                meshes.request(MeshKey { shape: PrimShape { profile_begin: begin as u16, ..PrimShape::default() }, detail: Detail::Low })
            );
        }
        let extra = MeshKey { shape: PrimShape { profile_curve: 15, ..PrimShape::default() }, detail: Detail::High };
        assert!(!meshes.request(extra));
        assert!(matches!(meshes.get(extra), PreparedMesh::Capacity));
        meshes.release(key(Detail::Low));
        assert!(meshes.request(extra));
        while let Some(job) = meshes.start() {
            meshes.complete(job.run());
        }
        assert!(matches!(meshes.get(extra), PreparedMesh::Failed(MeshBuildError::UnsupportedShape)));
        assert!(meshes.request(extra));
        assert!(meshes.start().is_none());
    }

    #[test]
    fn cache_pressure_recovery() {
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
            assert!(meshes.request(*key));
        }
        for _ in 0..keys.len() {
            let Some(job) = meshes.start() else {
                break;
            };
            meshes.complete(job.run());
        }
        assert!(keys.iter().any(|key| matches!(meshes.get(*key), PreparedMesh::Capacity)));
        assert!(meshes.start().is_none(), "capacity cannot cause repeated decoding");
        assert!(meshes.bytes <= CACHE_BYTES);
        let key = *keys.iter().find(|key| matches!(meshes.get(**key), PreparedMesh::Ready(_))).unwrap();
        meshes.release(key);
        let job = meshes.start().expect("eviction makes blocked work eligible");
        meshes.complete(job.run());
        assert!(meshes.bytes <= CACHE_BYTES);
        drop(meshes);
        assert_eq!(budget.charged_bytes(), 0);
    }
}
