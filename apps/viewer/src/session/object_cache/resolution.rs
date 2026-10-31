use std::collections::{HashMap, HashSet};

use network::SimulatorId;
use protocol::messages::{
    CacheMissType::{CrcMismatch, Full},
    ObjectUpdateCached,
};
use world::{LocalAddress, OrderEvidence, Region, UpdateSource};

use crate::session::inbound::{ObjectBatch, ObjectMediaRequest, ObjectOperation, decode_cached_object, object_media_requests};
use crate::session::object_cache::storage::make_access_submission;
use crate::session::object_cache::validation::{decode_entry, set_region_data, stored_entry};
use crate::session::object_cache::{
    CacheScope, EncodedEntry, LoadedRegion, MAX_REGION_BYTES, MAX_REGION_ENTRIES, MAX_TOTAL_BYTES, MAX_TOTAL_ENTRIES, ObjectCacheSession,
    StoredEntry, now_millis,
};
use crate::session::world::MAX_OBJECTS_PER_INTAKE_QUANTUM;

const FLAGS_USE_PHYSICS: u32 = 1;

impl ObjectCacheSession {
    pub(in crate::session) async fn resolve_cached(
        &mut self,
        simulator_id: SimulatorId,
        region: Region,
        sequence: u32,
        order: OrderEvidence,
        is_live: impl Fn(LocalAddress) -> bool,
        update: &ObjectUpdateCached,
        mut batch: ObjectBatch,
    ) -> (ObjectBatch, Vec<ObjectMediaRequest>) {
        let scope = self.identities.get(&simulator_id).cloned();
        let mut stale = HashSet::new();
        let mut media_requests = Vec::new();
        let mut invalid_fields = 0;
        for (index, operation) in batch.blocks.iter_mut().enumerate() {
            if index > 0 && index.is_multiple_of(MAX_OBJECTS_PER_INTAKE_QUANTUM) {
                tokio::task::yield_now().await;
            }
            let Some(ObjectOperation::Cached { source, local_id, .. }) = operation else { continue };
            let source = *source;
            let local_id = *local_id;
            if is_live(LocalAddress::new(region, local_id)) {
                *operation = None;
                continue;
            }
            let reference = update.objects.get(index).copied();
            let entry = self
                .loaded_regions
                .get(&simulator_id)
                .filter(|region| scope.as_ref() == Some(&region.scope))
                .and_then(|region| region.entries.get(&local_id.get()));
            let mut cache_miss_type = Full;
            let recovered = reference.zip(entry).and_then(|(reference, entry)| {
                let Some(crc) = u32::try_from(entry.crc).ok() else {
                    stale.insert(local_id.get());
                    return None;
                };
                if crc != reference.crc {
                    stale.insert(local_id.get());
                    cache_miss_type = CrcMismatch;
                    return None;
                }
                let Some(mut payload) = decode_entry(entry, local_id.get()) else {
                    stale.insert(local_id.get());
                    return None;
                };
                set_region_data(&mut payload, update.region_data);
                let ordinal = u32::try_from(index).ok()?;
                let mut operation = decode_cached_object(region, sequence, order, ordinal, reference, &payload, &mut invalid_fields);
                if operation.is_some() {
                    if let Some(operation) = &mut operation {
                        if let Some(received_at) = source.motion_received_at() {
                            operation.set_source(operation.source().with_motion_timing(received_at, source.motion_ping_delay()));
                        }
                        operation.set_is_physical(reference.update_flags & FLAGS_USE_PHYSICS != 0);
                    }
                    for mut request in object_media_requests(region, sequence, order, &payload) {
                        request.source = UpdateSource::new(region, sequence, order, ordinal);
                        media_requests.push(request);
                    }
                } else {
                    stale.insert(local_id.get());
                }
                operation
            });
            *operation = match recovered {
                Some(operation) => Some(operation),
                None => Some(ObjectOperation::Cached { source, local_id, cache_miss_type }),
            };
        }
        batch.invalid_fields = batch.invalid_fields.saturating_add(invalid_fields);
        if let Some(scope) = scope
            && self.loaded_regions.get(&simulator_id).is_some_and(|region| region.scope == scope)
        {
            self.touch_loaded_region(simulator_id, &scope);
            self.remove_loaded_entries(simulator_id, &stale);
            if !stale.is_empty() {
                self.enqueue(make_access_submission(scope, stale, now_millis()));
            }
        }
        (batch, media_requests)
    }

    pub(super) fn remember_entries(&mut self, simulator_id: SimulatorId, scope: CacheScope, entries: &[EncodedEntry]) {
        self.ensure_loaded_region(simulator_id, scope);
        for entry in entries {
            let current_bytes = self
                .loaded_regions
                .get(&simulator_id)
                .and_then(|region| region.entries.get(&entry.local_id))
                .map_or(0, |old| usize::try_from(old.payload_bytes).unwrap_or(usize::MAX));
            let replacing = self.loaded_regions.get(&simulator_id).is_some_and(|region| region.entries.contains_key(&entry.local_id));
            let Some(bytes_after) = self
                .loaded_regions
                .get(&simulator_id)
                .and_then(|region| region.bytes.checked_sub(current_bytes))
                .and_then(|bytes| bytes.checked_add(entry.payload.len()))
            else {
                continue;
            };
            let region_entries = self.loaded_regions.get(&simulator_id).map_or(0, |region| region.entries.len());
            if (!replacing && region_entries >= MAX_REGION_ENTRIES) || bytes_after > MAX_REGION_BYTES {
                continue;
            }
            let row_increment = usize::from(!replacing);
            let byte_increment = entry.payload.len().saturating_sub(current_bytes);
            self.make_loaded_room(simulator_id, row_increment, byte_increment);
            let rows_after = self.loaded_entries.saturating_add(row_increment);
            let bytes_after_global = self.loaded_bytes.saturating_sub(current_bytes).saturating_add(entry.payload.len());
            if rows_after > MAX_TOTAL_ENTRIES || bytes_after_global > MAX_TOTAL_BYTES {
                continue;
            }
            let last_used = self.next_use();
            if let Some(region) = self.loaded_regions.get_mut(&simulator_id) {
                region.bytes = bytes_after;
                region.last_used = last_used;
                region.entries.insert(entry.local_id, stored_entry(entry));
            }
            self.loaded_entries = rows_after;
            self.loaded_bytes = bytes_after_global;
        }
    }

    fn ensure_loaded_region(&mut self, simulator_id: SimulatorId, scope: CacheScope) {
        if self.loaded_regions.get(&simulator_id).is_some_and(|region| region.scope == scope) {
            return;
        }
        self.remove_loaded_region(simulator_id);
        let last_used = self.next_use();
        self.loaded_regions.insert(simulator_id, LoadedRegion { scope, entries: HashMap::new(), bytes: 0, last_used, complete: false });
    }

    pub(super) fn merge_partial_region(&mut self, simulator_id: SimulatorId, scope: &CacheScope, entries: &mut HashMap<u32, StoredEntry>) {
        let Some(partial) = self.loaded_regions.remove(&simulator_id) else { return };
        self.loaded_entries = self.loaded_entries.saturating_sub(partial.entries.len());
        self.loaded_bytes = self.loaded_bytes.saturating_sub(partial.bytes);
        if partial.scope != *scope {
            return;
        }
        let mut bytes = entries
            .values()
            .map(|entry| usize::try_from(entry.payload_bytes).unwrap_or(usize::MAX))
            .try_fold(0_usize, usize::checked_add)
            .unwrap_or(usize::MAX);
        for (local_id, latest) in partial.entries {
            let previous = entries.remove(&local_id);
            let previous_bytes = previous.as_ref().map_or(0, |entry| usize::try_from(entry.payload_bytes).unwrap_or(usize::MAX));
            let latest_bytes = usize::try_from(latest.payload_bytes).unwrap_or(usize::MAX);
            let Some(bytes_after) = bytes.checked_sub(previous_bytes).and_then(|bytes| bytes.checked_add(latest_bytes)) else { continue };
            if (previous.is_some() || entries.len() < MAX_REGION_ENTRIES) && bytes_after <= MAX_REGION_BYTES {
                entries.insert(local_id, latest);
                bytes = bytes_after;
            } else if let Some(previous) = previous {
                entries.insert(local_id, previous);
            }
        }
    }

    pub(super) fn install_loaded_region(&mut self, simulator_id: SimulatorId, scope: CacheScope, entries: HashMap<u32, StoredEntry>) {
        if entries.len() > MAX_REGION_ENTRIES {
            return;
        }
        let bytes =
            entries.values().map(|entry| usize::try_from(entry.payload_bytes).unwrap_or(usize::MAX)).try_fold(0_usize, usize::checked_add);
        let Some(bytes) = bytes.filter(|bytes| *bytes <= MAX_REGION_BYTES) else { return };
        self.make_loaded_room(simulator_id, entries.len(), bytes);
        if entries.len() > MAX_TOTAL_ENTRIES
            || bytes > MAX_TOTAL_BYTES
            || self.loaded_entries.saturating_add(entries.len()) > MAX_TOTAL_ENTRIES
            || self.loaded_bytes.saturating_add(bytes) > MAX_TOTAL_BYTES
        {
            return;
        }
        let last_used = self.next_use();
        self.loaded_entries += entries.len();
        self.loaded_bytes += bytes;
        self.loaded_regions.insert(simulator_id, LoadedRegion { scope, entries, bytes, last_used, complete: true });
    }

    pub(super) fn remove_loaded_region(&mut self, simulator_id: SimulatorId) {
        if let Some(region) = self.loaded_regions.remove(&simulator_id) {
            self.loaded_entries = self.loaded_entries.saturating_sub(region.entries.len());
            self.loaded_bytes = self.loaded_bytes.saturating_sub(region.bytes);
        }
    }

    pub(super) fn remove_loaded_entries(&mut self, simulator_id: SimulatorId, local_ids: &HashSet<u32>) {
        let Some(region) = self.loaded_regions.get_mut(&simulator_id) else { return };
        for local_id in local_ids {
            if let Some(entry) = region.entries.remove(local_id) {
                let bytes = usize::try_from(entry.payload_bytes).unwrap_or(usize::MAX);
                region.bytes = region.bytes.saturating_sub(bytes);
                self.loaded_entries = self.loaded_entries.saturating_sub(1);
                self.loaded_bytes = self.loaded_bytes.saturating_sub(bytes);
            }
        }
    }

    fn touch_loaded_region(&mut self, simulator_id: SimulatorId, scope: &CacheScope) {
        let last_used = self.next_use();
        if let Some(region) = self.loaded_regions.get_mut(&simulator_id).filter(|region| &region.scope == scope) {
            region.last_used = last_used;
        }
    }

    fn make_loaded_room(&mut self, current: SimulatorId, rows: usize, bytes: usize) {
        while (self.loaded_entries.saturating_add(rows) > MAX_TOTAL_ENTRIES || self.loaded_bytes.saturating_add(bytes) > MAX_TOTAL_BYTES)
            && let Some(oldest) = self
                .loaded_regions
                .iter()
                .filter(|(simulator_id, _)| **simulator_id != current)
                .min_by_key(|(_, region)| region.last_used)
                .map(|(simulator_id, _)| *simulator_id)
        {
            self.remove_loaded_region(oldest);
        }
    }

    fn next_use(&mut self) -> u64 {
        self.use_clock = self.use_clock.saturating_add(1);
        self.use_clock
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use network::Grid;
    use protocol::{lludp::Message, messages::CachedObjectReference};
    use rusqlite::params;
    use storage::sqlite::SqliteBatch;
    use tokio::runtime::Builder;
    use uuid::Uuid;
    use world::CLICK_ACTION_SIT;

    use super::*;
    use crate::session::inbound::decode_object_message;
    use crate::session::object_cache::ObjectCacheEvent;
    use crate::session::object_cache::test_support::*;
    #[tokio::test]
    async fn full_and_compressed_payloads_survive_restart_and_resolve_as_hits() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let path = directory.path().join("objects.sqlite");
        let mut cache = open_cache(&path);
        assert_eq!(load_region(&mut cache).await, 7);
        assert_eq!(cache.region_handshake_flags(SIMULATOR), 7);

        for fixture in [vectors::OBJECT_UPDATE, vectors::OBJECT_UPDATE_COMPRESSED_TYPED] {
            let mut update = fixture_message(fixture);
            match &mut update {
                Message::ObjectUpdate(update) => {
                    for object in &mut update.objects {
                        object.click_action = CLICK_ACTION_SIT;
                    }
                }
                Message::ObjectUpdateCompressed(update) => {
                    for object in &mut update.objects {
                        object.local_id = object.local_id.saturating_add(10_000);
                        object.click_action = CLICK_ACTION_SIT;
                    }
                }
                _ => panic!("fixture should be a full or compressed update"),
            }
            let (_, references) = cached_references(&update);
            cache.record_update(SIMULATOR, update.clone());
            flush(&mut cache).await;
            assert_eq!(cache.region_handshake_flags(SIMULATOR), 5);
            let cached = cached_message(cached_references(&update).0, references);
            let resolved = resolve(&mut cache, &cached, 40).await;
            assert_eq!(resolved.blocks.len(), cached.objects.len());
            assert!(
                resolved
                    .blocks
                    .iter()
                    .all(|operation| matches!(operation, Some(super::ObjectOperation::Observe(_) | super::ObjectOperation::Patch(_))))
            );
        }
        flush(&mut cache).await;
        cache.close().await;

        let mut reopened = open_cache(&path);
        assert_eq!(reopened.region_handshake_flags(SIMULATOR), 7);
        assert_eq!(load_region(&mut reopened).await, 5);
        assert_eq!(reopened.region_handshake_flags(SIMULATOR), 5);
        for fixture in [vectors::OBJECT_UPDATE, vectors::OBJECT_UPDATE_COMPRESSED_TYPED] {
            let mut update = fixture_message(fixture);
            if let Message::ObjectUpdateCompressed(update) = &mut update {
                for object in &mut update.objects {
                    object.local_id = object.local_id.saturating_add(10_000);
                }
            }
            let (region_data, references) = cached_references(&update);
            let resolved = resolve(&mut reopened, &cached_message(region_data, references), 41).await;
            assert!(
                resolved
                    .blocks
                    .iter()
                    .all(|operation| { matches!(operation, Some(super::ObjectOperation::Observe(_) | super::ObjectOperation::Patch(_))) })
            );
            for operation in &resolved.blocks {
                std::assert_matches!(
                    operation,
                    Some(super::ObjectOperation::Observe(observation)) if observation.click_action == Some(CLICK_ACTION_SIT)
                );
            }
        }
        reopened.close().await;
    }

    #[tokio::test]
    async fn cached_full_and_compressed_hits_restore_media_requests() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let mut cache = open_cache(directory.path().join("objects.sqlite"));
        assert_eq!(load_region(&mut cache).await, 7);
        let object_id = Uuid::from_u128(0xabcde);

        for (index, fixture) in [vectors::OBJECT_UPDATE, vectors::OBJECT_UPDATE_COMPRESSED_TYPED].into_iter().enumerate() {
            let mut full = fixture_message(fixture);
            let local_id = 20_000 + u32::try_from(index).expect("two fixture indexes fit u32");
            match &mut full {
                Message::ObjectUpdate(update) => {
                    update.objects.truncate(1);
                    update.objects[0].pcode = 9;
                    update.objects[0].id = object_id;
                    update.objects[0].local_id = local_id;
                    update.objects[0].media_url = "x-mv:12/00000000-0000-0000-0000-000000000001".into();
                }
                Message::ObjectUpdateCompressed(update) => {
                    update.objects.truncate(1);
                    update.objects[0].pcode = 9;
                    update.objects[0].id = object_id;
                    update.objects[0].local_id = local_id;
                    update.objects[0].media_url = Some("x-mv:12/00000000-0000-0000-0000-000000000001".into());
                }
                _ => panic!("object update fixture should be full or compressed"),
            }
            let (region_data, references) = cached_references(&full);
            cache.record_update(SIMULATOR, full.clone());
            flush(&mut cache).await;

            let cached = cached_message(region_data, references);
            let sequence = 90 + u32::try_from(index).expect("two fixture indexes fit u32");
            let batch = decoded_cached_batch(&cached, sequence);
            let (resolved, requests) = cache
                .resolve_cached(SIMULATOR, REGION, sequence, OrderEvidence::Ordered(u64::from(sequence)), |_| false, &cached, batch)
                .await;

            assert_eq!(resolved.blocks.len(), 1);
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].object_id, object_id);
            assert_eq!(requests[0].local_id.get(), local_id);
            assert_eq!(requests[0].version, 12);
        }
        cache.close().await;
    }

    #[tokio::test]
    async fn cached_hit_uses_reference_physics_flag() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let mut cache = open_cache(directory.path().join("objects.sqlite"));
        assert_eq!(load_region(&mut cache).await, 7);
        let mut full = fixture_message(vectors::OBJECT_UPDATE);
        let Message::ObjectUpdate(update) = &mut full else { panic!("fixture should be a full object update") };
        update.objects.truncate(1);
        update.objects[0].update_flags |= FLAGS_USE_PHYSICS;
        update.region_data.region_handle = REGION_HANDLE;
        let object_id = update.objects[0].id;
        let (region_data, mut references) = cached_references(&full);
        references[0].update_flags &= !FLAGS_USE_PHYSICS;
        cache.record_update(SIMULATOR, full);
        flush(&mut cache).await;

        let cached = cached_message(region_data, references);
        let batch = decoded_cached_batch(&cached, 80);
        let (batch, _) = cache.resolve_cached(SIMULATOR, REGION, 80, OrderEvidence::Ordered(80), |_| false, &cached, batch).await;
        let mut world = region_world();
        let summary = world.apply_object_batch(SIMULATOR, batch).expect("registered simulator applies the cached hit");
        assert!(summary.blocks.iter().all(|block| block.applied().is_some()));
        let entity = world.world().entity(object_id).expect("cached object was restored");
        assert!(!world.world().object(entity).expect("restored object is live").is_physical());
        cache.close().await;
    }

    #[tokio::test]
    async fn crc_mismatch_requests_refresh_and_missing_objects_request_full_updates() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let mut cache = open_cache(directory.path().join("objects.sqlite"));
        assert_eq!(load_region(&mut cache).await, 7);
        let full = fixture_message(vectors::OBJECT_UPDATE);
        cache.record_update(SIMULATOR, full.clone());
        flush(&mut cache).await;
        let (region_data, mut references) = cached_references(&full);
        let expected_crc = references[0].crc;
        references[0].crc = expected_crc.wrapping_add(1);
        references.push(CachedObjectReference { local_id: 0x7fff_fffe, crc: 1, update_flags: 0 });

        let resolved = resolve(&mut cache, &cached_message(region_data, references), 51).await;
        std::assert_matches!(
            resolved.blocks[0],
            Some(super::ObjectOperation::Cached { cache_miss_type: protocol::messages::CacheMissType::CrcMismatch, .. })
        );
        std::assert_matches!(
            resolved.blocks.last(),
            Some(Some(super::ObjectOperation::Cached { cache_miss_type: protocol::messages::CacheMissType::Full, .. }))
        );

        flush(&mut cache).await;
        let (region_data, mut references) = cached_references(&full);
        references.truncate(1);
        let resolved = resolve(&mut cache, &cached_message(region_data, references), 52).await;
        std::assert_matches!(
            resolved.blocks[0],
            Some(super::ObjectOperation::Cached { cache_miss_type: protocol::messages::CacheMissType::Full, .. })
        );
        cache.close().await;
    }

    #[tokio::test]
    async fn cached_probes_before_region_load_are_misses() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let path = directory.path().join("objects.sqlite");
        let mut writer = open_cache(&path);
        let full = fixture_message(vectors::OBJECT_UPDATE);
        writer.record_update(SIMULATOR, full.clone());
        flush(&mut writer).await;
        writer.close().await;

        let mut cache = open_cache(&path);
        let (region_data, references) = cached_references(&full);
        let update = cached_message(region_data, references);
        let before_load = resolve(&mut cache, &update, 55).await;
        assert!(before_load.blocks.iter().all(|operation| {
            matches!(operation, Some(super::ObjectOperation::Cached { cache_miss_type: protocol::messages::CacheMissType::Full, .. }))
        }));

        assert_eq!(load_region(&mut cache).await, 5);
        let after_load = resolve(&mut cache, &update, 56).await;
        assert!(
            after_load
                .blocks
                .iter()
                .all(|operation| { matches!(operation, Some(super::ObjectOperation::Observe(_) | super::ObjectOperation::Patch(_))) })
        );
        cache.close().await;
    }

    #[tokio::test]
    async fn corrupt_entries_are_misses_and_removed() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let path = directory.path().join("objects.sqlite");
        let mut cache = open_cache(&path);
        let full = fixture_message(vectors::OBJECT_UPDATE);
        cache.record_update(SIMULATOR, full.clone());
        flush(&mut cache).await;
        cache.close().await;

        let mut cache = open_cache(&path);
        let (region_data, references) = cached_references(&full);
        let corrupt_local_id = references[0].local_id;
        let corrupt = SqliteBatch::new(1, 64, move |transaction| {
            transaction.execute(
                "UPDATE object_entries SET payload = zeroblob(length(payload))
             WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3 AND local_id = ?4",
                params![Grid::Agni.id(), REGION_ID.as_bytes().as_slice(), CACHE_ID.as_bytes().as_slice(), i64::from(corrupt_local_id)],
            )?;
            Ok(())
        })
        .expect("corruption batch fits SQLite limits");
        cache.db.as_ref().expect("cache database is open").submit_batches(vec![corrupt]).expect("queue corrupt fixture");
        flush(&mut cache).await;
        assert_eq!(load_region(&mut cache).await, 5);

        let first = cached_message(region_data, references[..1].to_vec());
        let resolved = resolve(&mut cache, &first, 60).await;
        std::assert_matches!(
            resolved.blocks[0],
            Some(super::ObjectOperation::Cached { cache_miss_type: protocol::messages::CacheMissType::Full, .. })
        );
        flush(&mut cache).await;
        assert_eq!(cache_count(cache.db.as_ref().expect("cache database is open")).await, references.len() - 1);
        cache.close().await;
    }

    #[tokio::test]
    async fn semantically_invalid_entries_are_removed() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let mut cache = open_cache(directory.path().join("objects.sqlite"));
        let mut full = fixture_message(vectors::OBJECT_UPDATE);
        let Message::ObjectUpdate(update) = &mut full else { panic!("object fixture should be a full update") };
        update.objects[0].id = Uuid::nil();
        let (region_data, references) = cached_references(&full);
        cache.record_update(SIMULATOR, full);
        flush(&mut cache).await;
        assert_eq!(load_region(&mut cache).await, 5);

        let resolved = resolve(&mut cache, &cached_message(region_data, references.clone()), 61).await;

        std::assert_matches!(
            resolved.blocks[0],
            Some(super::ObjectOperation::Cached { cache_miss_type: protocol::messages::CacheMissType::Full, .. })
        );
        flush(&mut cache).await;
        assert_eq!(cache_count(cache.db.as_ref().expect("cache database is open")).await, references.len() - 1);
        cache.close().await;
    }

    #[tokio::test]
    async fn terse_updates_remove_cached_entries_before_later_probes() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let mut cache = open_cache(directory.path().join("objects.sqlite"));
        let mut full = fixture_message(vectors::OBJECT_UPDATE);
        let Message::ObjectUpdate(update) = &mut full else { panic!("object fixture should be a full update") };
        update.objects.truncate(1);
        let local_id = update.objects[0].local_id;
        let (region_data, references) = cached_references(&full);
        cache.record_update(SIMULATOR, full);
        flush(&mut cache).await;
        assert_eq!(cache_count(cache.db.as_ref().expect("cache database is open")).await, 1);
        assert_eq!(load_region(&mut cache).await, 5);

        let mut terse = fixture_message(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
        let Message::ImprovedTerseObjectUpdate(update) = &mut terse else { panic!("terse fixture should be a terse update") };
        update.region_data.region_handle = region_data.region_handle;
        update.objects[0].as_decoded_mut().expect("terse fixture object is valid").local_id = local_id;
        cache.record_update(SIMULATOR, terse);
        flush(&mut cache).await;

        assert_eq!(cache_count(cache.db.as_ref().expect("cache database is open")).await, 0);
        let resolved = resolve(&mut cache, &cached_message(region_data, references), 62).await;
        std::assert_matches!(
            resolved.blocks[0],
            Some(super::ObjectOperation::Cached { cache_miss_type: protocol::messages::CacheMissType::Full, .. })
        );
        cache.close().await;
    }

    #[tokio::test]
    async fn cache_id_change_invalidates_the_old_region_scope() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let mut cache = open_cache(directory.path().join("objects.sqlite"));
        assert_eq!(load_region(&mut cache).await, 7);
        let full = fixture_message(vectors::OBJECT_UPDATE);
        cache.record_update(SIMULATOR, full.clone());
        flush(&mut cache).await;
        assert_eq!(cache.region_handshake_flags(SIMULATOR), 5);

        cache.set_region_identity(SIMULATOR, &region_handshake(NEXT_CACHE_ID));
        assert_eq!(cache.region_handshake_flags(SIMULATOR), 7);
        flush(&mut cache).await;
        cache.set_region_identity(SIMULATOR, &region_handshake(CACHE_ID));
        assert_eq!(load_region(&mut cache).await, 7);
        let (region_data, references) = cached_references(&full);
        let result = resolve(&mut cache, &cached_message(region_data, references[..1].to_vec()), 71).await;
        std::assert_matches!(
            result.blocks[0],
            Some(super::ObjectOperation::Cached { cache_miss_type: protocol::messages::CacheMissType::Full, .. })
        );
        cache.close().await;
    }

    #[test]
    #[ignore = "manual release-mode synthetic region intake measurement"]
    fn measure_warm_resolution_and_five_thousand_object_intake() {
        let runtime = Builder::new_current_thread().enable_all().build().expect("create benchmark runtime");
        runtime.block_on(async {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let (region_data, updates, references) = synthetic_region_updates();
        let mut warm_cache = open_cache(directory.path().join("warm.sqlite"));
        for update in &updates {
            warm_cache.record_update(SIMULATOR, update.clone());
            flush(&mut warm_cache).await;
        }
        let handshake_started = Instant::now();
        let warm_handshake_flags = load_region(&mut warm_cache).await;
        let warm_handshake_us = handshake_started.elapsed().as_micros();

        let mut warm_world = region_world();
        let mut decode_us = 0_u128;
        let mut resolve_us = 0_u128;
        let mut world_apply_us = 0_u128;
        let mut warm_misses = 0_usize;
        let mut warm_requests = 0_usize;
        for (index, chunk) in references.chunks(255).enumerate() {
            let sequence = 100 + u32::try_from(index).expect("synthetic sequence fits u32");
            let update = cached_message(region_data, chunk.to_vec());
            let message = Message::ObjectUpdateCached(update.clone());
            let started = Instant::now();
            let batch = decode_object_message(REGION, sequence, OrderEvidence::Ordered(u64::from(sequence)), &message)
                .expect("cached update decodes into World operations");
            decode_us += started.elapsed().as_micros();
            let started = Instant::now();
            let (batch, _) = warm_cache.resolve_cached(
                SIMULATOR,
                REGION,
                sequence,
                OrderEvidence::Ordered(u64::from(sequence)),
                |_| false,
                &update,
                batch,
            ).await;
            resolve_us += started.elapsed().as_micros();
            warm_misses += batch.blocks.iter().filter(|operation| matches!(operation, Some(super::ObjectOperation::Cached { .. }))).count();
            let started = Instant::now();
            let summary = warm_world
                .apply_object_batch_cooperatively(SIMULATOR, batch, |_, targets, _| warm_requests += targets.len())
                .await;
            world_apply_us += started.elapsed().as_micros();
            assert!(summary.is_some(), "cached objects apply to the registered region");
        }
        assert_eq!(warm_world.world().object_count(), 5_000);

        let mut full_world = region_world();
        let mut full_world_apply_us = 0_u128;
        let mut full_recovery_targets = 0_usize;
        for (index, message) in updates.iter().enumerate() {
            let sequence = u32::try_from(index + 1).expect("synthetic packet sequence fits u32");
            let batch = decode_object_message(REGION, sequence, OrderEvidence::Ordered(u64::from(sequence)), message)
                .expect("full update decodes into World operations");
            let started = Instant::now();
            let summary = full_world
                .apply_object_batch_cooperatively(SIMULATOR, batch, |_, targets, _| full_recovery_targets += targets.len())
                .await;
            full_world_apply_us += started.elapsed().as_micros();
            assert!(summary.is_some(), "synthetic update applies to the registered region");
        }
        assert_eq!(full_world.world().object_count(), 5_000);

        let mut cold_cache = open_cache(directory.path().join("cold.sqlite"));
        let handshake_started = Instant::now();
        let cold_handshake_flags = load_region(&mut cold_cache).await;
        let cold_handshake_us = handshake_started.elapsed().as_micros();
        let mut cold_misses = 0_usize;
        for (index, chunk) in references.chunks(255).enumerate() {
            let sequence = 200 + u32::try_from(index).expect("synthetic sequence fits u32");
            let resolved = resolve(&mut cold_cache, &cached_message(region_data, chunk.to_vec()), sequence).await;
            cold_misses += resolved.blocks.iter().filter(|operation| matches!(operation, Some(super::ObjectOperation::Cached { .. }))).count();
        }

        let mut disabled_times = Vec::with_capacity(3);
        let mut enabled_times = Vec::with_capacity(3);
        let mut dropped_writes = 0_u64;
        for trial in 0..3 {
            let mut disabled_cache = ObjectCacheSession::new(Grid::Agni);
            assert!(disabled_cache.handle_event(ObjectCacheEvent::OpenFailed("disabled for measurement".into())).is_none());
            disabled_cache.set_region_identity(SIMULATOR, &region_handshake(CACHE_ID));
            let mut enabled_cache = open_cache(directory.path().join(format!("enabled-{trial}.sqlite")));
            if trial % 2 == 0 {
                let (elapsed, _) = measure_full_update_intake(&mut disabled_cache, updates.clone()).await;
                disabled_times.push(elapsed);
                let (elapsed, _) = measure_full_update_intake(&mut enabled_cache, updates.clone()).await;
                enabled_times.push(elapsed);
            } else {
                let (elapsed, _) = measure_full_update_intake(&mut enabled_cache, updates.clone()).await;
                enabled_times.push(elapsed);
                let (elapsed, _) = measure_full_update_intake(&mut disabled_cache, updates.clone()).await;
                disabled_times.push(elapsed);
            }
            flush(&mut enabled_cache).await;
            dropped_writes = dropped_writes.saturating_add(enabled_cache.dropped_writes);
            disabled_cache.close().await;
            enabled_cache.close().await;
        }
        disabled_times.sort_unstable();
        enabled_times.sort_unstable();
        let disabled_median = disabled_times[1].as_micros();
        let enabled_median = enabled_times[1].as_micros();
        let intake_delta = (enabled_median as f64 - disabled_median as f64) * 100.0 / disabled_median.max(1) as f64;
        let cache_overhead_us = decode_us + resolve_us;
        let warm_total_us = cache_overhead_us + world_apply_us;
        let packet_count = references.len().div_ceil(255);
        println!(
            "warm 5,000-object arrival: handshake load {warm_handshake_us}us flags={warm_handshake_flags}; {warm_misses} cache misses, {} RequestMultipleObjects packets, {warm_requests} World recovery targets",
            warm_misses.div_ceil(255)
        );
        println!(
            "warm cached packets: decode {decode_per_packet}us, cache resolve {resolve_per_packet}us, World apply {world_per_packet}us; cache overhead {cache_share:.1}% / World {world_share:.1}% of {total_per_packet}us total",
            decode_per_packet = decode_us / u128::try_from(packet_count).unwrap_or(1),
            resolve_per_packet = resolve_us / u128::try_from(packet_count).unwrap_or(1),
            world_per_packet = world_apply_us / u128::try_from(packet_count).unwrap_or(1),
            total_per_packet = warm_total_us / u128::try_from(packet_count).unwrap_or(1),
            cache_share = (cache_overhead_us as f64 * 100.0) / warm_total_us.max(1) as f64,
            world_share = (world_apply_us as f64 * 100.0) / warm_total_us.max(1) as f64,
        );
        println!(
            "World apply per packet: cached {cached_world_per_packet}us ({warm_requests} recovery targets); normal full update {full_world_per_packet}us ({full_recovery_targets} recovery targets); signed delta {world_apply_delta:.1}%",
            cached_world_per_packet = world_apply_us / u128::try_from(packet_count).unwrap_or(1),
            full_world_per_packet = full_world_apply_us / u128::try_from(updates.len()).unwrap_or(1),
            world_apply_delta = (world_apply_us as f64 - full_world_apply_us as f64) * 100.0 / full_world_apply_us.max(1) as f64,
        );
        println!(
            "cold 5,000-object arrival: handshake load {cold_handshake_us}us flags={cold_handshake_flags}; {cold_misses} misses, {} RequestMultipleObjects packets",
            cold_misses.div_ceil(255)
        );
        println!(
            "full-update intake median over three 5,000-object bursts: cache disabled {disabled_median}us; cache enabled {enabled_median}us; signed delta {delta:.1}%; {} of {} cache write packets dropped",
            dropped_writes,
            updates.len() * 3,
            delta = intake_delta,
        );
        cold_cache.close().await;
        warm_cache.close().await;
    });
    }
}
