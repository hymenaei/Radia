use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use storage::{EvictionCandidate, EvictionPolicy, EvictionReason, EvictionScore, NamespaceId};

use crate::Kind;

const RECENCY_SCALE: u128 = 1_000_000;
const RECENCY_BUCKET: u64 = 64;
const FREQUENCY_CAP: u64 = 15;
const DECODE_BYTES_PER_MICROSECOND: u64 = 1024;
const DECODE_DENSITY_CAP: u128 = 4096;
const MAX_OBSERVED_DECODE_MICROS: u64 = 30_000_000;

pub(crate) struct DecodeCosts {
    micros: [AtomicU64; 5],
}

impl Default for DecodeCosts {
    fn default() -> Self {
        Self { micros: std::array::from_fn(|_| AtomicU64::new(0)) }
    }
}

impl DecodeCosts {
    pub(crate) fn observe(&self, kind: Kind, micros: u64) {
        let Some(cost) = self.micros.get(kind_index(kind)) else {
            return;
        };
        let sample = micros.min(MAX_OBSERVED_DECODE_MICROS);
        match cost.try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(if current == 0 { sample } else { current - current / 4 + sample / 4 })
        }) {
            Ok(_) | Err(_) => {}
        }
    }

    fn for_namespace(&self, namespace: NamespaceId) -> u64 {
        let index = if namespace == NamespaceId::new(0) {
            0
        } else if namespace == NamespaceId::new(1) {
            1
        } else if namespace == NamespaceId::new(2) {
            2
        } else if namespace == NamespaceId::new(3) {
            3
        } else if namespace == NamespaceId::new(4) {
            4
        } else {
            return 0;
        };
        self.micros.get(index).map_or(0, |cost| cost.load(Ordering::Relaxed))
    }
}

pub(crate) struct AssetEvictionPolicy {
    costs: Arc<DecodeCosts>,
}

impl AssetEvictionPolicy {
    pub(crate) fn new(costs: Arc<DecodeCosts>) -> Self {
        Self { costs }
    }
}

impl EvictionPolicy for AssetEvictionPolicy {
    fn score(&self, candidate: EvictionCandidate, now: u64) -> EvictionScore {
        score_candidate(candidate, now, self.costs.for_namespace(candidate.key.namespace()))
    }
}

fn score_candidate(candidate: EvictionCandidate, now: u64, decode_micros: u64) -> EvictionScore {
    let age = now.saturating_sub(candidate.last_use);
    let recency = RECENCY_SCALE / u128::from(1u64.saturating_add(age / RECENCY_BUCKET));
    let frequency = 1 + candidate.use_count.min(FREQUENCY_CAP);
    let decode_bytes = decode_micros.saturating_mul(DECODE_BYTES_PER_MICROSECOND);
    let footprint = candidate.size.max(1);
    let decode_density =
        u128::from(decode_bytes).saturating_mul(1024).checked_div(u128::from(footprint)).unwrap_or(0).min(DECODE_DENSITY_CAP);
    let value_density = 1024 + decode_density;
    let value = recency.saturating_mul(u128::from(frequency)).saturating_mul(value_density) / 1024;
    let reason = if age > RECENCY_BUCKET.saturating_mul(8) {
        EvictionReason::Stale
    } else if candidate.use_count <= 1 {
        EvictionReason::Infrequent
    } else if decode_density <= 128 {
        EvictionReason::LowReconstructionCost
    } else {
        EvictionReason::LowUtility
    };
    EvictionScore { value, reason }
}

fn kind_index(kind: Kind) -> usize {
    match kind {
        Kind::Texture => 0,
        Kind::PbrMaterial => 1,
        Kind::LegacyMaterial => 2,
        Kind::Environment => 3,
        Kind::Mesh => 4,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Clone, Copy)]
    struct TraceAsset {
        id: u32,
        size: u64,
        kind: Kind,
        decode_micros: u64,
    }

    #[derive(Clone, Copy)]
    struct TraceEntry {
        asset: TraceAsset,
        last_use: u64,
        use_count: u64,
    }

    fn candidate(id: u32, size: u64, last_use: u64, use_count: u64, kind: Kind) -> EvictionCandidate {
        let mut name = [0; 16];
        name[..4].copy_from_slice(&id.to_le_bytes());
        let namespace = u8::try_from(kind_index(kind)).expect("supported kind namespace");
        EvictionCandidate { key: storage::Key::new(NamespaceId::new(namespace), name), size, last_use, use_count }
    }

    #[test]
    fn frequency_preserves_reused_assets() {
        let frequent = score_candidate(candidate(1, 256 * 1024, 900, 12, Kind::Texture), 1000, 0);
        let one_time = score_candidate(candidate(2, 256 * 1024, 900, 0, Kind::Texture), 1000, 0);

        assert!(frequent.value > one_time.value);
    }

    #[test]
    fn decode_cost_increases_value() {
        let entry = candidate(3, 512 * 1024, 900, 2, Kind::Texture);
        let cheap = score_candidate(entry, 1000, 0);
        let expensive = score_candidate(entry, 1000, 120_000);

        assert!(expensive.value > cheap.value);
    }

    #[test]
    fn mesh_decode_cost_is_scored() {
        let costs = Arc::new(DecodeCosts::default());
        let policy = AssetEvictionPolicy::new(Arc::clone(&costs));
        let entry = candidate(4, 512 * 1024, 900, 2, Kind::Mesh);
        let cheap = policy.score(entry, 1000);
        costs.observe(Kind::Mesh, 120_000);
        let expensive = policy.score(entry, 1000);

        assert!(expensive.value > cheap.value);
    }

    #[test]
    fn trace_beats_lru() {
        let usefulness = replay_trace(true);
        let lru = replay_trace(false);

        println!(
            "synthetic multi-session byte hit rate: usefulness {:.1}% ({}/{}) ; LRU {:.1}% ({}/{})",
            percentage(usefulness.hit_bytes, usefulness.requested_bytes),
            usefulness.hit_bytes,
            usefulness.requested_bytes,
            percentage(lru.hit_bytes, lru.requested_bytes),
            lru.hit_bytes,
            lru.requested_bytes
        );
        assert!(usefulness.hit_bytes > lru.hit_bytes);
    }

    #[derive(Default)]
    struct TraceResult {
        hit_bytes: u64,
        requested_bytes: u64,
    }

    fn percentage(hit_bytes: u64, requested_bytes: u64) -> f64 {
        hit_bytes as f64 * 100.0 / requested_bytes as f64
    }

    fn replay_trace(usefulness: bool) -> TraceResult {
        const LIMIT: u64 = 2 * 1024 * 1024;
        let costs = Arc::new(DecodeCosts::default());
        let policy = AssetEvictionPolicy::new(Arc::clone(&costs));
        let home: Vec<_> = (0..8).map(|id| TraceAsset { id, size: 96 * 1024, kind: Kind::Texture, decode_micros: 400 }).collect();
        let meshes: Vec<_> = (8..10).map(|id| TraceAsset { id, size: 512 * 1024, kind: Kind::Texture, decode_micros: 120_000 }).collect();
        let mut entries = HashMap::<u32, TraceEntry>::new();
        let mut bytes = 0u64;
        let mut now = 0u64;
        let mut result = TraceResult::default();

        for session in 0..8 {
            for asset in home.iter().chain(&meshes) {
                for _ in 0..if session == 0 { 3 } else { 1 } {
                    request_trace_asset(*asset, usefulness, &policy, &costs, LIMIT, &mut entries, &mut bytes, &mut now, &mut result);
                }
            }
            for offset in 0..12 {
                let asset = TraceAsset { id: 1000 + session * 12 + offset, size: 128 * 1024, kind: Kind::Texture, decode_micros: 400 };
                request_trace_asset(asset, usefulness, &policy, &costs, LIMIT, &mut entries, &mut bytes, &mut now, &mut result);
            }
        }
        result
    }

    fn request_trace_asset(
        asset: TraceAsset,
        usefulness: bool,
        policy: &AssetEvictionPolicy,
        costs: &DecodeCosts,
        limit: u64,
        entries: &mut HashMap<u32, TraceEntry>,
        bytes: &mut u64,
        now: &mut u64,
        result: &mut TraceResult,
    ) {
        *now = now.saturating_add(1);
        result.requested_bytes = result.requested_bytes.saturating_add(asset.size);
        if let Some(entry) = entries.get_mut(&asset.id) {
            result.hit_bytes = result.hit_bytes.saturating_add(asset.size);
            entry.last_use = *now;
            entry.use_count = entry.use_count.saturating_add(1);
            return;
        }

        costs.observe(asset.kind, asset.decode_micros);

        while bytes.saturating_add(asset.size) > limit {
            let victim = entries
                .iter()
                .map(|(id, entry)| {
                    let score = if usefulness {
                        policy.score(candidate(*id, entry.asset.size, entry.last_use, entry.use_count, entry.asset.kind), *now).value
                    } else {
                        u128::from(entry.last_use)
                    };
                    (score, *id)
                })
                .min();
            let Some((_, id)) = victim else {
                return;
            };
            if let Some(entry) = entries.remove(&id) {
                *bytes = bytes.saturating_sub(entry.asset.size);
            }
        }
        *bytes = bytes.saturating_add(asset.size);
        entries.insert(asset.id, TraceEntry { asset, last_use: *now, use_count: 0 });
    }
}
