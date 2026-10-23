use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

pub const MEMORY_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct Budget(Arc<Accounting>);

impl Budget {
    pub fn charged_bytes(&self) -> usize {
        self.0.used.load(Ordering::Acquire)
    }

    pub fn high_water_bytes(&self) -> usize {
        self.0.peak.load(Ordering::Acquire)
    }

    pub(in crate::scene) fn reserve(&self, bytes: usize) -> Option<Lease> {
        let before = self
            .0
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| used.checked_add(bytes).filter(|total| *total <= MEMORY_BYTES))
            .ok()?;
        self.0.peak.fetch_max(before + bytes, Ordering::AcqRel);
        Some(Lease { budget: self.clone(), bytes })
    }

    pub(in crate::scene) fn mesh_work(&self) -> Option<MeshWork> {
        self.0.mesh_jobs.try_update(Ordering::AcqRel, Ordering::Acquire, |jobs| (jobs < 2).then_some(jobs + 1)).ok()?;
        let Some(lease) = self.reserve(4 * 1024 * 1024) else {
            self.0.mesh_jobs.fetch_sub(1, Ordering::AcqRel);
            return None;
        };
        Some(MeshWork { lease, budget: self.clone() })
    }
}

#[derive(Default)]
struct Accounting {
    used: AtomicUsize,
    peak: AtomicUsize,
    mesh_jobs: AtomicUsize,
}

pub(in crate::scene) struct MeshWork {
    pub(in crate::scene) lease: Lease,
    budget: Budget,
}

impl Drop for MeshWork {
    fn drop(&mut self) {
        self.budget.0.mesh_jobs.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(in crate::scene) struct Lease {
    budget: Budget,
    bytes: usize,
}

impl Lease {
    pub(in crate::scene) fn split(&mut self, bytes: usize) -> Option<Self> {
        self.bytes = self.bytes.checked_sub(bytes)?;
        Some(Self { budget: self.budget.clone(), bytes })
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.budget.0.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
