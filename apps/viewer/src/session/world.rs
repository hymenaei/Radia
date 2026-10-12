use std::{
    collections::HashMap,
    sync::{Arc, PoisonError, RwLock},
};

use base::UUID;
use network::SimulatorId;
use world::{
    Applied, ApplyError,
    ApplyOutcome::{Introduced, TombstoneRecorded},
    LocalAddress, Region, RegionHandle, RegionInfo, UpdateSource, World, WorldLimits,
};

use super::{
    inbound::{ObjectBatch, ObjectOperation, SitTargetUpdate},
    recovery::MAX_RECOVERY_OBJECTS_PER_REQUEST,
};

const MAX_WORLD_ENTITIES: usize = 65_536;
const MAX_WORLD_ENTITIES_PER_REGION: usize = 10_000;
const MAX_WORLD_TOMBSTONES_PER_REGION: usize = 4_096;
const MAX_WORLD_TOMBSTONES: usize = 16_384;
const MAX_WORLD_JOURNAL_ENTRIES: usize = 4_096;
const MAX_REGION_NAME_BYTES: usize = 256;

pub(crate) type SharedWorldState = Arc<RwLock<World>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IngestStatus {
    Applied,
    RegionHandleMismatch,
    RegionContextMismatch,
}

#[derive(Debug)]
pub(super) enum RegistrationError {
    HandleChanged,
    Apply(ApplyError),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct BlockResult {
    pub(super) index: usize,
    pub(super) address: Option<LocalAddress>,
    pub(super) result: Option<Result<Applied, ApplyError>>,
    pub(super) recovery_target: Option<LocalAddress>,
    recovery_satisfied: bool,
    pub(super) valid: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct IngestSummary {
    pub(super) status: IngestStatus,
    pub(super) accepted_region_handle: Option<RegionHandle>,
    pub(super) blocks: Vec<BlockResult>,
    pub(super) invalid_blocks: usize,
}

impl IngestSummary {
    fn new(status: IngestStatus) -> Self {
        Self { status, accepted_region_handle: None, blocks: Vec::new(), invalid_blocks: 0 }
    }

    pub(super) fn recovery_targets(&self) -> impl Iterator<Item = LocalAddress> + '_ {
        self.blocks.iter().flat_map(|block| {
            block.recovery_target.into_iter().chain(
                block
                    .result
                    .iter()
                    .filter_map(|result| result.as_ref().ok()?.recovery.as_ref())
                    .flat_map(|recovery| recovery.targets.iter().copied()),
            )
        })
    }

    pub(super) fn recovery_cancellation_addresses(&self) -> impl Iterator<Item = LocalAddress> + '_ {
        self.blocks
            .iter()
            .filter_map(|block| {
                let result = block.result.as_ref()?.as_ref().ok()?;
                let address = match &result.outcome {
                    TombstoneRecorded(address) => Some(*address),
                    _ if block.recovery_satisfied => block.address,
                    _ => None,
                };
                Some((result, address))
            })
            .flat_map(|(result, address)| result.recovery_cancellations.iter().copied().chain(address))
    }
}

#[derive(Clone, Copy)]
struct SimulatorRegion {
    region: Region,
    pinned_handle: Option<RegionHandle>,
    observed_handle: Option<RegionHandle>,
}

/// Owns the observed World and the simulator connection lifetimes for one session.
pub(super) struct SessionWorld {
    state: SharedWorldState,
    regions: HashMap<SimulatorId, SimulatorRegion>,
}

impl SessionWorld {
    pub(super) fn new(limits: WorldLimits) -> Self {
        Self { state: Arc::new(RwLock::new(World::new(limits))), regions: HashMap::new() }
    }

    pub(super) fn shared_state(&self) -> SharedWorldState {
        Arc::clone(&self.state)
    }

    pub(super) fn set_self_agent(&mut self, agent_id: UUID) {
        if let Err(error) = self.state.write().unwrap_or_else(PoisonError::into_inner).set_self_agent(agent_id) {
            tracing::warn!(?error, "could not set authenticated world agent");
        }
    }

    pub(super) fn register_simulator(
        &mut self,
        simulator_id: SimulatorId,
        handle: Option<RegionHandle>,
    ) -> Result<Region, RegistrationError> {
        if let Some(source) = self.regions.get_mut(&simulator_id) {
            if handle.is_some_and(|handle| source.pinned_handle.is_some_and(|pinned| pinned != handle))
                || handle.is_some_and(|handle| source.observed_handle.is_some_and(|observed| observed != handle))
            {
                return Err(RegistrationError::HandleChanged);
            }
            source.pinned_handle = source.pinned_handle.or(handle);
            let region = source.region;
            let current_handle = source.pinned_handle.or(source.observed_handle);
            if let Some(handle) = current_handle
                && let Err(error) = self.update_region_metadata(
                    region,
                    RegionInfo { handle: Some(handle), origin: Some(handle.origin()), ..RegionInfo::default() },
                )
            {
                tracing::warn!(?error, "could not update registered simulator region metadata");
            }
            return Ok(region);
        }

        let region = Region::new(simulator_id.get());
        self.state.write().unwrap_or_else(PoisonError::into_inner).adopt_region(region).map(|_| ()).map_err(RegistrationError::Apply)?;
        self.regions.insert(simulator_id, SimulatorRegion { region, pinned_handle: handle, observed_handle: None });
        if let Some(handle) = handle
            && let Err(error) = self
                .update_region_metadata(region, RegionInfo { handle: Some(handle), origin: Some(handle.origin()), ..RegionInfo::default() })
        {
            tracing::warn!(?error, "could not update registered simulator region metadata");
        }
        Ok(region)
    }

    pub(super) fn region_for(&self, simulator_id: SimulatorId) -> Option<Region> {
        self.regions.get(&simulator_id).map(|source| source.region)
    }

    pub(super) fn update_region_metadata(&mut self, region: Region, info: RegionInfo) -> Result<(), ApplyError> {
        self.state.write().unwrap_or_else(PoisonError::into_inner).patch_region(region, info).map(|_| ())
    }

    pub(super) fn update_handshake_metadata(
        &mut self,
        simulator_id: SimulatorId,
        region_id: Option<UUID>,
        name: String,
    ) -> Result<(), ApplyError> {
        let Some(source) = self.regions.get(&simulator_id).copied() else {
            return Ok(());
        };
        let handle = source.pinned_handle.or(source.observed_handle);
        self.update_region_metadata(
            source.region,
            RegionInfo { region_id, handle, name: Some(name), origin: handle.map(RegionHandle::origin) },
        )
    }

    pub(super) fn apply_object_batch(&mut self, simulator_id: SimulatorId, batch: ObjectBatch) -> Option<IngestSummary> {
        let source = *self.regions.get(&simulator_id)?;
        if source.region != batch.region {
            return Some(IngestSummary::new(IngestStatus::RegionContextMismatch));
        }
        let accepted_handle = match resolve_region_handle(batch.region_handle, source) {
            Ok(handle) => handle,
            Err(status) => return Some(IngestSummary::new(status)),
        };

        let mut summary = IngestSummary::new(IngestStatus::Applied);
        summary.accepted_region_handle = accepted_handle;
        summary.invalid_blocks = batch.invalid_blocks;
        let mut state = None;
        for (index, operation) in batch.blocks.into_iter().enumerate() {
            let (address, result, recovery_target, recovery_satisfied, valid) = match operation {
                Some(ObjectOperation::Cached { source, local_id }) => {
                    let address = LocalAddress::new(source.region(), local_id);
                    (Some(address), None, Some(address), false, true)
                }
                Some(operation) => {
                    let address = operation.address();
                    let world = state.get_or_insert_with(|| self.state.write().unwrap_or_else(PoisonError::into_inner));
                    let prior_parent = world.entity_at(address).and_then(|entity| world.object(entity)).map(|object| object.parent());
                    let has_parent_evidence = match operation {
                        ObjectOperation::Observe(_) => true,
                        ObjectOperation::Patch(patch) => patch.parent.is_some(),
                        ObjectOperation::Remove { .. } | ObjectOperation::Cached { .. } => false,
                    };
                    let result = apply_operation(world, operation);
                    let current_parent = world.entity_at(address).and_then(|entity| world.object(entity)).map(|object| object.parent());
                    let recovery_satisfied = result.as_ref().is_ok_and(|applied| {
                        matches!(applied.outcome, Introduced(_)) || (has_parent_evidence && prior_parent != current_parent)
                    });
                    (Some(address), Some(result), None, recovery_satisfied, true)
                }
                None => (None, None, None, false, false),
            };
            summary.blocks.push(BlockResult { index, address, result, recovery_target, recovery_satisfied, valid });
        }
        drop(state);

        if !summary.blocks.is_empty() && summary.blocks.iter().all(|block| !block.valid) {
            summary.accepted_region_handle = None;
        }
        if let Some(handle) = summary.accepted_region_handle {
            let metadata = self.regions.get_mut(&simulator_id).and_then(|source| {
                let previous_handle = source.pinned_handle.or(source.observed_handle);
                source.observed_handle = Some(handle);
                let current_handle = source.pinned_handle.or(source.observed_handle);
                (previous_handle != current_handle).then_some((
                    source.region,
                    RegionInfo { handle: current_handle, origin: current_handle.map(RegionHandle::origin), ..RegionInfo::default() },
                ))
            });
            if let Some((region, info)) = metadata
                && let Err(error) = self.update_region_metadata(region, info)
            {
                tracing::warn!(?error, "could not update object region handle");
            }
        }
        Some(summary)
    }

    pub(super) fn apply_sit_target_update(
        &self,
        simulator_id: SimulatorId,
        source: UpdateSource,
        update: SitTargetUpdate,
    ) -> Option<Result<Applied, ApplyError>> {
        let simulator = self.regions.get(&simulator_id)?;
        if simulator.region != source.region() {
            return None;
        }
        Some(self.state.write().unwrap_or_else(PoisonError::into_inner).patch_sit_target(source, update.object_id, Some(update.target)))
    }

    pub(super) fn retire_simulator(&mut self, simulator_id: SimulatorId) -> Result<(), ApplyError> {
        let Some(source) = self.regions.get(&simulator_id).copied() else { return Ok(()) };
        let _ = self.state.write().unwrap_or_else(PoisonError::into_inner).retire_region(source.region)?;
        self.regions.remove(&simulator_id);
        Ok(())
    }

    pub(super) fn retire_all(&mut self) -> Vec<(SimulatorId, ApplyError)> {
        let simulator_ids = self.regions.keys().copied().collect::<Vec<_>>();
        let mut failures = Vec::new();
        for simulator_id in simulator_ids {
            if let Err(error) = self.retire_simulator(simulator_id) {
                failures.push((simulator_id, error));
            }
        }
        failures
    }
}

fn resolve_region_handle(message_handle: Option<RegionHandle>, source: SimulatorRegion) -> Result<Option<RegionHandle>, IngestStatus> {
    if source.pinned_handle.is_some() && source.observed_handle.is_some() && source.pinned_handle != source.observed_handle {
        return Err(IngestStatus::RegionContextMismatch);
    }

    if let Some(handle) = message_handle {
        if source.pinned_handle.is_some_and(|pinned| pinned != handle) || source.observed_handle.is_some_and(|observed| observed != handle)
        {
            return Err(IngestStatus::RegionHandleMismatch);
        }
        return Ok(Some(handle));
    }

    Ok(source.pinned_handle.or(source.observed_handle))
}

fn apply_operation(world: &mut World, operation: ObjectOperation) -> Result<Applied, ApplyError> {
    match operation {
        ObjectOperation::Observe(observation) => world.observe_object(observation),
        ObjectOperation::Patch(patch) => world.patch_object(patch),
        ObjectOperation::Remove { source, local_id } => world.remove_object(source, local_id),
        ObjectOperation::Cached { .. } => unreachable!("cached operations are recovery intents"),
    }
}

pub(super) fn viewer_world_limits() -> WorldLimits {
    WorldLimits {
        max_region_registrations: MAX_WORLD_ENTITIES,
        max_entities: MAX_WORLD_ENTITIES,
        max_entities_per_region: MAX_WORLD_ENTITIES_PER_REGION,
        max_tombstones_per_region: MAX_WORLD_TOMBSTONES_PER_REGION,
        max_tombstones: MAX_WORLD_TOMBSTONES,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(MAX_WORLD_TOMBSTONES_PER_REGION),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(MAX_WORLD_TOMBSTONES),
        max_journal_entries: MAX_WORLD_JOURNAL_ENTRIES,
        max_journal_changes: MAX_WORLD_ENTITIES,
        max_region_name_bytes: MAX_REGION_NAME_BYTES,
        max_recovery_targets: MAX_RECOVERY_OBJECTS_PER_REQUEST,
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU32, sync::mpsc, time::Duration};

    use base::UUID;
    use math::Vec3;
    use network::{SimulatorId, test_support::simulator_id};
    use world::{
        Kind, LocalAddress, ObjectObservation, ObjectPatch, OrderEvidence, Parent, ParentUpdate, Pose, Region, RegionHandle, UpdateSource,
    };

    use crate::session::inbound::{ObjectBatch, ObjectOperation};

    use super::{SessionWorld, viewer_world_limits};

    fn observation(region: Region, order: u32, id: u128, local_id: NonZeroU32, parent: ParentUpdate) -> ObjectObservation {
        ObjectObservation {
            source: UpdateSource::new(region, order, OrderEvidence::Ordered(u64::from(order)), 0),
            id: UUID::from_u128(id),
            local_id,
            kind: Kind::Object,
            parent,
            pose: None,
            linear_motion: None,
            angular_velocity: None,
            scale: None,
            attachment_point: None,
        }
    }

    fn apply_one(world: &mut SessionWorld, simulator_id: SimulatorId, region: Region, operation: ObjectOperation) -> super::IngestSummary {
        world
            .apply_object_batch(simulator_id, ObjectBatch { region, region_handle: None, blocks: vec![Some(operation)], invalid_blocks: 0 })
            .expect("registered simulator")
    }

    #[test]
    fn cached_batch_skips_world_lock() {
        let simulator_id = simulator_id();
        let handle = RegionHandle::new(0x1111_0000_0000_0001);
        let mut world = SessionWorld::new(viewer_world_limits());
        let region = world.register_simulator(simulator_id, Some(handle)).expect("register simulator");
        let local_id = NonZeroU32::new(7).expect("nonzero local ID");
        let source = UpdateSource::new(region, 1, OrderEvidence::Ordered(1), 0);
        let batch = ObjectBatch {
            region,
            region_handle: Some(handle),
            blocks: vec![Some(ObjectOperation::Cached { source, local_id })],
            invalid_blocks: 0,
        };
        let shared_state = world.shared_state();
        let write_guard = shared_state.write().expect("lock world for contention test");
        let (result_sender, result_receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = world.apply_object_batch(simulator_id, batch).is_some_and(|summary| summary.recovery_targets().count() == 1);
            result_sender.send(result).expect("send batch result");
        });

        let completed_without_world_lock = result_receiver.recv_timeout(Duration::from_millis(100)).is_ok();
        drop(write_guard);
        worker.join().expect("batch worker");

        assert!(completed_without_world_lock);
    }

    #[test]
    fn partial_child_update_keeps_recovery_pending() {
        let simulator_id = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let region = world.register_simulator(simulator_id, None).expect("register simulator");
        let parent_local_id = NonZeroU32::new(1).expect("nonzero parent local ID");
        let child_local_id = NonZeroU32::new(2).expect("nonzero child local ID");
        let parent_address = LocalAddress::new(region, parent_local_id);
        let child_address = LocalAddress::new(region, child_local_id);
        apply_one(
            &mut world,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 1, 11, parent_local_id, ParentUpdate::Root)),
        );
        apply_one(
            &mut world,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 4, 12, child_local_id, ParentUpdate::Local(parent_local_id))),
        );
        let removed = apply_one(
            &mut world,
            simulator_id,
            region,
            ObjectOperation::Remove { source: UpdateSource::new(region, 3, OrderEvidence::Ordered(3), 0), local_id: parent_local_id },
        );
        assert_eq!(removed.recovery_targets().collect::<Vec<_>>(), [child_address]);

        let mut pose = ObjectPatch::new(UpdateSource::new(region, 5, OrderEvidence::Ordered(5), 0), child_local_id);
        pose.pose = Some(Pose { position: Some(Vec3::new(1.0, 2.0, 3.0)), rotation: None });
        let partial = apply_one(&mut world, simulator_id, region, ObjectOperation::Patch(pose));
        assert!(partial.blocks[0].result.as_ref().unwrap().as_ref().unwrap().revision.is_some());
        assert_eq!(partial.recovery_cancellation_addresses().count(), 0);

        let mut parent = ObjectPatch::new(UpdateSource::new(region, 6, OrderEvidence::Ordered(6), 0), child_local_id);
        parent.parent = Some(ParentUpdate::Root);
        let refreshed = apply_one(&mut world, simulator_id, region, ObjectOperation::Patch(parent));
        assert_eq!(refreshed.recovery_cancellation_addresses().collect::<Vec<_>>(), [child_address]);
        let state = world.shared_state();
        let state = state.read().unwrap();
        let child = state.object(state.entity_at(child_address).unwrap()).unwrap();
        assert_eq!(child.parent(), Parent::Root);
        assert!(state.entity_at(parent_address).is_none());
    }

    #[test]
    fn live_kill_cancels_removed_entities() {
        let simulator_id = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let region = world.register_simulator(simulator_id, None).expect("register simulator");
        let parent_local_id = NonZeroU32::new(1).expect("nonzero parent local ID");
        let child_local_id = NonZeroU32::new(2).expect("nonzero child local ID");
        let parent_address = LocalAddress::new(region, parent_local_id);
        let child_address = LocalAddress::new(region, child_local_id);

        apply_one(
            &mut world,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 1, 21, parent_local_id, ParentUpdate::Root)),
        );
        apply_one(
            &mut world,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 2, 22, child_local_id, ParentUpdate::Local(parent_local_id))),
        );

        let killed = apply_one(
            &mut world,
            simulator_id,
            region,
            ObjectOperation::Remove { source: UpdateSource::new(region, 3, OrderEvidence::Ordered(3), 0), local_id: parent_local_id },
        );
        let cancelled = killed.recovery_cancellation_addresses().collect::<Vec<_>>();

        assert_eq!(cancelled.len(), 2);
        assert!(cancelled.contains(&parent_address));
        assert!(cancelled.contains(&child_address));
        assert_eq!(killed.recovery_targets().count(), 0);
    }

    #[test]
    fn parent_reintroduction_cancels_child_retry() {
        let simulator_id = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let region = world.register_simulator(simulator_id, None).expect("register simulator");
        let parent_local_id = NonZeroU32::new(1).expect("nonzero parent local ID");
        let child_local_id = NonZeroU32::new(2).expect("nonzero child local ID");
        let parent_address = LocalAddress::new(region, parent_local_id);
        let child_address = LocalAddress::new(region, child_local_id);

        apply_one(
            &mut world,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 1, 31, parent_local_id, ParentUpdate::Root)),
        );
        apply_one(
            &mut world,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 4, 32, child_local_id, ParentUpdate::Local(parent_local_id))),
        );
        let removed = apply_one(
            &mut world,
            simulator_id,
            region,
            ObjectOperation::Remove { source: UpdateSource::new(region, 3, OrderEvidence::Ordered(3), 0), local_id: parent_local_id },
        );
        assert_eq!(removed.recovery_targets().collect::<Vec<_>>(), [child_address]);

        let restored = apply_one(
            &mut world,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 5, 31, parent_local_id, ParentUpdate::Root)),
        );
        assert!(restored.recovery_cancellation_addresses().any(|address| address == child_address));
        assert!(restored.recovery_cancellation_addresses().any(|address| address == parent_address));

        let state = world.shared_state();
        let state = state.read().unwrap();
        let child = state.object(state.entity_at(child_address).expect("child remains live")).expect("child object");
        let parent = state.entity_at(parent_address).expect("parent reintroduced");
        assert_eq!(child.parent(), Parent::Resolved(parent));
    }
}
