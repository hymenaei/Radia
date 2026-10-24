use std::sync::atomic::{AtomicU64, Ordering};

use network::SimulatorId;

static NEXT_SIMULATOR_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn simulator_id() -> SimulatorId {
    SimulatorId::new(NEXT_SIMULATOR_ID.fetch_add(1, Ordering::Relaxed))
}
