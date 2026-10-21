use std::{
    fmt,
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use tokio::sync::watch;

pub const ASSET_WORK_BYTES: usize = 512 * 1024 * 1024;
pub(crate) const WORK_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const TEXTURE_WORK_BYTES: usize = 192 * 1024 * 1024;

struct Accounting {
    charged: AtomicUsize,
    working: AtomicUsize,
    resident: AtomicUsize,
    high_water: AtomicUsize,
    wake: watch::Sender<()>,
}

impl Default for Accounting {
    fn default() -> Self {
        let (wake, _) = watch::channel(());
        Self {
            charged: AtomicUsize::new(0),
            working: AtomicUsize::new(0),
            resident: AtomicUsize::new(0),
            high_water: AtomicUsize::new(0),
            wake,
        }
    }
}

#[derive(Clone, Default)]
pub struct Budget(Arc<Accounting>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryStats {
    pub charged_bytes: usize,
    pub resident_bytes: usize,
    pub high_water_bytes: usize,
}

impl Budget {
    pub fn stats(&self) -> MemoryStats {
        MemoryStats {
            charged_bytes: self.0.charged.load(Ordering::Acquire),
            resident_bytes: self.0.resident.load(Ordering::Acquire),
            high_water_bytes: self.0.high_water.load(Ordering::Acquire),
        }
    }

    pub(crate) fn wake(&self) -> watch::Sender<()> {
        self.0.wake.clone()
    }

    pub(crate) fn reserve(&self, bytes: usize) -> Option<Reservation> {
        self.reserve_with_headroom(bytes, 0)
    }

    pub(crate) fn reserve_with_headroom(&self, bytes: usize, headroom: usize) -> Option<Reservation> {
        self.0
            .working
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|next| next.checked_add(headroom).is_some_and(|total| total <= ASSET_WORK_BYTES))
            })
            .ok()?;
        let used = self.0.charged.fetch_add(bytes, Ordering::AcqRel);
        self.0.high_water.fetch_max(used + bytes, Ordering::AcqRel);
        Some(Reservation { budget: self.clone(), bytes, resident: false })
    }
}

pub(crate) struct Reservation {
    budget: Budget,
    bytes: usize,
    resident: bool,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.resident {
            self.budget.0.resident.fetch_sub(self.bytes, Ordering::AcqRel);
        } else {
            self.budget.0.working.fetch_sub(self.bytes, Ordering::AcqRel);
        }
        self.budget.0.charged.fetch_sub(self.bytes, Ordering::AcqRel);
        self.budget.0.wake.send_replace(());
    }
}

impl Reservation {
    pub(crate) fn resize(&mut self, bytes: usize) -> bool {
        if self.resident {
            return false;
        }
        if bytes > self.bytes {
            let Some(mut extra) = self.budget.reserve(bytes - self.bytes) else { return false };
            extra.bytes = 0;
        } else {
            self.budget.0.charged.fetch_sub(self.bytes - bytes, Ordering::AcqRel);
            self.budget.0.working.fetch_sub(self.bytes - bytes, Ordering::AcqRel);
            if bytes < self.bytes {
                self.budget.0.wake.send_replace(());
            }
        }
        self.bytes = bytes;
        true
    }

    pub(crate) fn split(&mut self, bytes: usize) -> Result<Self, ()> {
        if self.resident || bytes > self.bytes {
            return Err(());
        }
        self.bytes -= bytes;
        Ok(Self { budget: self.budget.clone(), bytes, resident: false })
    }
    pub(crate) fn retain(mut self, capacity: usize) -> Result<Self, ()> {
        if capacity > self.bytes {
            return Err(());
        }
        self.budget.0.charged.fetch_sub(self.bytes - capacity, Ordering::AcqRel);
        self.budget.0.working.fetch_sub(self.bytes, Ordering::AcqRel);
        self.bytes = capacity;
        self.resident = true;
        self.budget.0.resident.fetch_add(capacity, Ordering::AcqRel);
        self.budget.0.wake.send_replace(());
        Ok(self)
    }

    pub(crate) fn finish(self, bytes: Vec<u8>) -> Result<Blob, ()> {
        let reservation = self.retain(bytes.capacity())?;
        Ok(Blob(Arc::new(BlobInner { bytes, _reservation: reservation })))
    }
}

struct BlobInner {
    bytes: Vec<u8>,
    _reservation: Reservation,
}

#[derive(Clone)]
pub(crate) struct Blob(Arc<BlobInner>);

impl Deref for Blob {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0.bytes
    }
}

impl fmt::Debug for Blob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Blob").field("bytes", &self.len()).finish()
    }
}
