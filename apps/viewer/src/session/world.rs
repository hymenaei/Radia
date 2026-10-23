use std::collections::HashMap;

use network::SimulatorId;
use uuid::Uuid;
use world::{
    Appearance, Applied, ApplyError,
    ApplyOutcome::{AgentHandedOff, Introduced, TombstoneRecorded},
    EnvironmentAssignment, EnvironmentTarget, FieldRejectionReason, Kind, LocalAddress, MAX_QUERY_STATE_MAINTENANCE_WORK,
    ObjectMediaUpdate, ObjectObservation, ObjectPatch, OrderEvidence, ParcelLocalId, Region, RegionApplied, RegionHandle, RegionInfo,
    RegionOutcome, UpdateSource, World, WorldLimits,
};

use super::{
    inbound::{AvatarAppearanceMessage, ObjectBatch, ObjectMediaRequest, ObjectOperation, SitTargetUpdate},
    object_media::ObjectMediaTarget,
    recovery::MAX_RECOVERY_OBJECTS_PER_REQUEST,
    terrain::DecodedTerrain,
};

pub(super) const MAX_WORLD_ENTITIES: usize = 65_536;
const MAX_WORLD_TOMBSTONES_PER_REGION: usize = 4_096;
const MAX_WORLD_TOMBSTONES: usize = 16_384;
const MAX_WORLD_JOURNAL_ENTRIES: usize = 4_096;
const MAX_REGION_NAME_BYTES: usize = 256;
const MAX_INGEST_DIAGNOSTIC_ADDRESSES: usize = 8;
const MAX_OBJECTS_PER_INTAKE_QUANTUM: usize = 32;
const QUERY_STATE_MAINTENANCE_WORK_PER_QUANTUM: usize = MAX_QUERY_STATE_MAINTENANCE_WORK / 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IngestStatus {
    Applied,
    RegionHandleMismatch,
    RegionContextMismatch,
    RegionMetadataRejected(ApplyError),
}

#[derive(Debug)]
pub(super) enum RegistrationError {
    HandleChanged,
    Apply(ApplyError),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum BlockOutcome {
    Invalid,
    RecoveryRequired(LocalAddress),
    Applied { address: Option<LocalAddress>, applied: Applied, recovered: Option<LocalAddress> },
    Rejected { address: Option<LocalAddress>, error: ApplyError },
}

impl BlockOutcome {
    pub(super) fn applied(&self) -> Option<&Applied> {
        match self {
            Self::Applied { applied, .. } => Some(applied),
            _ => None,
        }
    }

    fn address(&self) -> Option<LocalAddress> {
        match self {
            Self::RecoveryRequired(address) => Some(*address),
            Self::Applied { address, .. } | Self::Rejected { address, .. } => *address,
            Self::Invalid => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct IngestSummary {
    pub(super) status: IngestStatus,
    pub(super) accepted_region_handle: Option<RegionHandle>,
    pub(super) blocks: Vec<BlockOutcome>,
    pub(super) invalid_blocks: usize,
    pub(super) invalid_fields: usize,
}

impl IngestSummary {
    pub(super) fn new(status: IngestStatus) -> Self {
        Self { status, accepted_region_handle: None, blocks: Vec::new(), invalid_blocks: 0, invalid_fields: 0 }
    }

    pub(super) fn push_applied(&mut self, applied: Applied, address: Option<LocalAddress>) {
        self.blocks.push(BlockOutcome::Applied { address, applied, recovered: None });
    }

    pub(super) fn push_error(&mut self, error: ApplyError, address: Option<LocalAddress>) {
        self.blocks.push(BlockOutcome::Rejected { address, error });
    }

    pub(super) fn append(&mut self, other: Self) {
        self.blocks.extend(other.blocks);
        self.invalid_blocks = self.invalid_blocks.saturating_add(other.invalid_blocks);
        self.invalid_fields = self.invalid_fields.saturating_add(other.invalid_fields);
        self.accepted_region_handle = self.accepted_region_handle.or(other.accepted_region_handle);
        if other.status != IngestStatus::Applied {
            self.status = other.status;
        }
    }

    pub(super) fn recovery_targets(&self) -> impl Iterator<Item = LocalAddress> + '_ {
        self.blocks.iter().flat_map(|block| {
            let required = match block {
                BlockOutcome::RecoveryRequired(address) => Some(*address),
                _ => None,
            };
            required.into_iter().chain(block.applied().into_iter().flat_map(|applied| applied.recovery.requests.iter().copied()))
        })
    }

    pub(super) fn recovery_cancellation_addresses(&self) -> impl Iterator<Item = LocalAddress> + '_ {
        self.blocks
            .iter()
            .filter_map(|block| {
                let BlockOutcome::Applied { applied, recovered, .. } = block else { return None };
                let address = match &applied.outcome {
                    TombstoneRecorded(address) => Some(*address),
                    _ => *recovered,
                };
                Some((applied, address))
            })
            .flat_map(|(result, address)| result.recovery_cancellations.iter().copied().chain(address))
    }

    pub(super) fn log_incomplete_data(&self, simulator_id: SimulatorId, region: Option<Region>) {
        use ApplyError::{
            AddressGenerationExhausted, EntityBudgetExhausted, EnvironmentBudgetExhausted, FrameGenerationExhausted, InvalidValue,
            KindMismatch, RegionRetired, RegistrationBudgetExhausted, RevisionExhausted, StaleRegion, TerrainBudgetExhausted,
        };
        use FieldRejectionReason::{
            InvalidRotation, InvalidValue as RejectedValue, ParentCycle, ParentDependsOnRejectedParent, ResourceLimit, StaleFrame,
            StaleVersion, UntrustedSource,
        };

        let simulator = simulator_id.get();
        let region = region.map(|region| region.get());
        if let IngestStatus::RegionMetadataRejected(error) = self.status {
            tracing::warn!(simulator, region, %error, "could not apply object batch region metadata");
            return;
        }
        let region_mismatch = matches!(self.status, IngestStatus::RegionHandleMismatch | IngestStatus::RegionContextMismatch);
        let mut malformed = (self.invalid_blocks as u64).saturating_add(self.invalid_fields as u64);
        let mut resource_limits = 0_u64;
        let mut rejected_addresses = [None; MAX_INGEST_DIAGNOSTIC_ADDRESSES];
        let mut rejected_address_count = 0;
        for block in &self.blocks {
            let mut rejected = false;
            match block {
                BlockOutcome::Rejected { error: InvalidValue { .. } | KindMismatch { .. }, .. } => {
                    malformed = malformed.saturating_add(1);
                    rejected = true;
                }
                BlockOutcome::Rejected {
                    error:
                        EntityBudgetExhausted
                        | TerrainBudgetExhausted
                        | EnvironmentBudgetExhausted
                        | RegistrationBudgetExhausted
                        | RevisionExhausted
                        | FrameGenerationExhausted
                        | AddressGenerationExhausted,
                    ..
                } => {
                    resource_limits = resource_limits.saturating_add(1);
                    rejected = true;
                }
                BlockOutcome::Rejected { error: StaleRegion | RegionRetired, .. } => {}
                BlockOutcome::Applied { applied, .. } => {
                    for rejection in applied.rejections.iter() {
                        match rejection.reason {
                            RejectedValue { .. } | InvalidRotation | ParentCycle | ParentDependsOnRejectedParent => {
                                malformed = malformed.saturating_add(1);
                                rejected = true;
                            }
                            ResourceLimit { .. } => {
                                resource_limits = resource_limits.saturating_add(1);
                                rejected = true;
                            }
                            StaleFrame | StaleVersion | UntrustedSource => {}
                        }
                    }
                }
                BlockOutcome::Invalid | BlockOutcome::RecoveryRequired(_) => {}
            }
            if rejected
                && rejected_address_count < MAX_INGEST_DIAGNOSTIC_ADDRESSES
                && let Some(address) = block.address()
            {
                rejected_addresses[rejected_address_count] = Some(address);
                rejected_address_count += 1;
            }
        }
        if region_mismatch || malformed > 0 || resource_limits > 0 {
            let local_ids =
                rejected_addresses[..rejected_address_count].iter().flatten().map(|address| address.local_id().get()).collect::<Vec<_>>();
            if region_mismatch || resource_limits > 0 {
                tracing::warn!(
                    simulator,
                    region,
                    region_mismatch,
                    malformed,
                    resource_limits,
                    local_ids = ?local_ids,
                    "world update was partially rejected; world data may be incomplete"
                );
            } else {
                tracing::debug!(
                    simulator,
                    region,
                    malformed,
                    local_ids = ?local_ids,
                    "world update contained rejected values"
                );
            }
        }
    }
}

pub(super) struct SessionWorld {
    world: World,
    regions: HashMap<SimulatorId, Region>,
}

impl SessionWorld {
    pub(super) fn new(limits: WorldLimits) -> Self {
        let world = World::new(limits);
        Self { world, regions: HashMap::new() }
    }

    pub(super) fn world(&self) -> &World {
        &self.world
    }

    pub(super) fn observe_agent_parcel(
        &mut self,
        source: UpdateSource,
        parcel: Option<ParcelLocalId>,
    ) -> Result<RegionApplied, ApplyError> {
        self.world.observe_agent_parcel(source, parcel)
    }

    pub(super) fn observe_environment(
        &mut self,
        target: EnvironmentTarget,
        source: UpdateSource,
        assignment: EnvironmentAssignment,
    ) -> Result<RegionApplied, ApplyError> {
        self.world.observe_environment(target, source, assignment)
    }

    pub(super) fn maintain_query_state(&mut self) -> bool {
        match self.world.maintain_query_state(QUERY_STATE_MAINTENANCE_WORK_PER_QUANTUM) {
            Ok(progress) => progress.dirty_scopes > 0,
            Err(error) => {
                tracing::warn!(?error, "could not maintain world query state");
                false
            }
        }
    }

    pub(super) fn set_self_agent(&mut self, simulator_id: SimulatorId, agent_id: Uuid) {
        let region = self.region_for(simulator_id).map(|region| region.get());
        if let Err(error) = self.world.set_self_agent(agent_id) {
            tracing::warn!(simulator = simulator_id.get(), region, %error, "could not set authenticated world agent");
        }
    }

    pub(super) fn register_simulator(
        &mut self,
        simulator_id: SimulatorId,
        handle: Option<RegionHandle>,
    ) -> Result<Region, RegistrationError> {
        if let Some(region) = self.region_for(simulator_id) {
            let current = self.world.region(region).expect("registered simulator has a live World region").handle;
            resolve_region_handle(handle, current).map_err(|_| RegistrationError::HandleChanged)?;
            self.update_region_handle(region, handle).map_err(RegistrationError::Apply)?;
            return Ok(region);
        }

        let region = Region::new(simulator_id.get());
        self.world.adopt_region(region).map(|_| ()).map_err(RegistrationError::Apply)?;
        self.regions.insert(simulator_id, region);
        self.update_region_handle(region, handle).map_err(RegistrationError::Apply)?;
        Ok(region)
    }

    fn update_region_handle(&mut self, region: Region, handle: Option<RegionHandle>) -> Result<(), ApplyError> {
        if let Some(handle) = handle {
            self.update_region_metadata(region, RegionInfo { handle: Some(handle), ..RegionInfo::default() })?;
        }
        Ok(())
    }

    pub(super) fn region_for(&self, simulator_id: SimulatorId) -> Option<Region> {
        self.regions.get(&simulator_id).copied()
    }

    pub(super) fn region_name(&self, simulator_id: SimulatorId) -> Option<&str> {
        self.world.region(self.region_for(simulator_id)?).and_then(|info| info.name.as_deref())
    }

    pub(super) fn simulator_for_region(&self, region: Region) -> Option<SimulatorId> {
        self.regions.iter().find_map(|(simulator_id, known)| (*known == region).then_some(*simulator_id))
    }

    pub(super) fn update_region_metadata(&mut self, region: Region, info: RegionInfo) -> Result<(), ApplyError> {
        self.world.patch_region(region, info).map(|_| ())
    }

    pub(super) fn update_water_height(&mut self, source: UpdateSource, height: f32) -> Result<(), ApplyError> {
        self.world.observe_water_height(source, height).map(|_| ())
    }

    pub(super) fn apply_terrain(&mut self, decoded: DecodedTerrain) -> usize {
        let region = decoded.source.region().get();
        let Some(simulator) = self.simulator_for_region(decoded.source.region()).map(|simulator| simulator.get()) else { return 0 };
        let mut rejected = decoded.rejected;
        let mut applied = 0;
        for (ordinal, patch) in decoded.patches {
            let source = UpdateSource::new(decoded.source.region(), decoded.source.sequence(), decoded.source.order(), ordinal)
                .with_authority(decoded.source.authority());
            match self.world.observe_terrain_patch(source, patch) {
                Ok(result) if matches!(result.outcome, RegionOutcome::Patched { .. }) => applied += 1,
                Ok(_) => {}
                Err(_) => rejected += 1,
            }
        }
        if rejected > 0 {
            tracing::debug!(simulator, region, rejected, "terrain packet was partially rejected");
        }
        applied
    }

    pub(super) fn update_handshake_metadata(
        &mut self,
        source: UpdateSource,
        region_id: Option<Uuid>,
        name: String,
    ) -> Result<(), ApplyError> {
        self.world.observe_region_metadata(source, RegionInfo { region_id, name: Some(name), ..RegionInfo::default() }).map(|_| ())
    }

    pub(super) fn apply_object_batch(&mut self, simulator_id: SimulatorId, batch: ObjectBatch) -> Option<IngestSummary> {
        let region = self.region_for(simulator_id)?;
        if region != batch.region {
            return Some(IngestSummary::new(IngestStatus::RegionContextMismatch));
        }
        let current_handle = self.world.region(region).expect("registered simulator has a live World region").handle;
        let accepted_handle = match resolve_region_handle(batch.region_handle, current_handle) {
            Ok(handle) => handle,
            Err(status) => return Some(IngestSummary::new(status)),
        };

        let mut summary = IngestSummary::new(IngestStatus::Applied);
        if batch.blocks.is_empty() || batch.blocks.iter().any(Option::is_some) {
            if let Err(error) = self.update_region_handle(region, accepted_handle) {
                summary.status = IngestStatus::RegionMetadataRejected(error);
                return Some(summary);
            }
            summary.accepted_region_handle = accepted_handle;
        }
        summary.invalid_blocks = batch.invalid_blocks;
        summary.invalid_fields = batch.invalid_fields;
        for operation in batch.blocks {
            let outcome = match operation {
                Some(ObjectOperation::Cached { source, local_id }) => {
                    let address = LocalAddress::new(source.region(), local_id);
                    BlockOutcome::RecoveryRequired(address)
                }
                Some(operation) => {
                    let address = operation.address();
                    match apply_operation(&mut self.world, operation) {
                        Ok(applied) => {
                            let recovered = applied.recovery.satisfied.then_some(address);
                            BlockOutcome::Applied { address: Some(address), applied, recovered }
                        }
                        Err(error) => BlockOutcome::Rejected { address: Some(address), error },
                    }
                }
                None => BlockOutcome::Invalid,
            };
            summary.blocks.push(outcome);
        }
        Some(summary)
    }

    pub(super) async fn apply_object_batch_cooperatively(
        &mut self,
        simulator_id: SimulatorId,
        batch: ObjectBatch,
        mut on_chunk: impl FnMut(&IngestSummary, Vec<(Option<SimulatorId>, LocalAddress)>, Vec<LocalAddress>),
    ) -> Option<IngestSummary> {
        let ObjectBatch { region, region_handle, blocks, invalid_blocks, invalid_fields } = batch;
        let mut remaining = blocks.into_iter().peekable();
        let mut summary: Option<IngestSummary> = None;
        loop {
            let operations = remaining.by_ref().take(MAX_OBJECTS_PER_INTAKE_QUANTUM).collect::<Vec<_>>();
            let empty = operations.is_empty();
            if empty && summary.is_some() {
                break;
            }
            let has_more = remaining.peek().is_some();
            let part = self.apply_object_batch(
                simulator_id,
                ObjectBatch {
                    region,
                    region_handle,
                    blocks: operations,
                    invalid_blocks: if summary.is_none() { invalid_blocks } else { 0 },
                    invalid_fields: if summary.is_none() { invalid_fields } else { 0 },
                },
            )?;
            let recovery_targets = part.recovery_targets().map(|address| (self.simulator_for_region(address.region()), address)).collect();
            let recovery_cancellations = part.recovery_cancellation_addresses().collect();
            on_chunk(&part, recovery_targets, recovery_cancellations);
            match &mut summary {
                Some(summary) => summary.append(part),
                None => summary = Some(part),
            }
            if summary.as_ref().is_some_and(|summary| summary.status != IngestStatus::Applied) {
                break;
            }
            if empty {
                break;
            }
            self.maintain_query_state();
            if has_more {
                tokio::task::yield_now().await;
            }
        }
        summary
    }

    pub(super) fn prepare_object_media(&self, simulator_id: SimulatorId, request: ObjectMediaRequest) -> Option<ObjectMediaTarget> {
        let region = self.region_for(simulator_id)?;
        if region != request.source.region() || !matches!(request.source.order(), OrderEvidence::Ordered(_)) {
            return None;
        }
        let address = LocalAddress::new(region, request.local_id);
        let entity = self.world.entity_at(address)?;
        let object = self.world.object(entity)?;
        if object.id() != request.object_id
            || object.kind() != Kind::Object
            || object.appearance() != Some(Appearance::Surface)
            || object.object_media().is_some_and(|media| media.version() >= request.version)
        {
            return None;
        }
        let object_binding_target = self.world.object_binding_target(entity)?;
        Some(ObjectMediaTarget { simulator_id, binding: object_binding_target, source: request.source, version: request.version })
    }

    pub(super) fn object_media_target_is_current(&self, target: ObjectMediaTarget) -> bool {
        let Some(region) = self.region_for(target.simulator_id) else { return false };
        if region != target.source.region() || !matches!(target.source.order(), OrderEvidence::Ordered(_)) {
            return false;
        }
        if target.binding.address().region() != region || !self.world.object_binding_target_is_current(target.binding) {
            return false;
        }
        self.world.object(target.binding.entity()).is_some_and(|object| {
            object.kind() == Kind::Object
                && object.appearance() == Some(Appearance::Surface)
                && object.object_media().is_none_or(|media| media.version() < target.version)
        })
    }

    pub(super) fn apply_object_media(
        &mut self,
        target: ObjectMediaTarget,
        update: ObjectMediaUpdate,
    ) -> Option<Result<Applied, ApplyError>> {
        if update.version() < target.version || !self.object_media_target_is_current(target) {
            return None;
        }
        let mut patch = ObjectPatch::new(target.source, target.binding.address().local_id());
        patch.object_media = Some(update);
        let result = self.world.patch_object(patch);
        Some(result)
    }

    pub(super) fn apply_sit_target_update(
        &mut self,
        simulator_id: SimulatorId,
        source: UpdateSource,
        update: SitTargetUpdate,
    ) -> Option<Result<Applied, ApplyError>> {
        let region = self.region_for(simulator_id)?;
        if region != source.region() {
            return None;
        }
        let result = self.world.patch_sit_target(source, update.object_id, Some(update.target));
        Some(result)
    }

    pub(super) fn apply_avatar_appearance(
        &mut self,
        simulator_id: SimulatorId,
        source: UpdateSource,
        update: AvatarAppearanceMessage,
    ) -> Option<Result<Applied, ApplyError>> {
        let region = self.region_for(simulator_id)?;
        if region != source.region() {
            return None;
        }
        let entity = self.world.entity(update.object_id)?;
        let object = self.world.object(entity)?;
        if object.id() != update.object_id || object.kind() != Kind::Avatar || object.address().region() != region {
            return None;
        }
        let local_id = object.address().local_id();
        let mut patch = ObjectPatch::new(source, local_id);
        patch.avatar_appearance = Some(update.update);
        let result = self.world.patch_object(patch);
        Some(result)
    }

    pub(super) fn object_address(&self, simulator_id: SimulatorId, object_id: Uuid) -> Option<LocalAddress> {
        let region = self.region_for(simulator_id)?;
        let entity = self.world.entity(object_id)?;
        self.world.object(entity).filter(|object| object.address().region() == region).map(|object| object.address())
    }

    pub(super) fn complete_agent_handoff(
        &mut self,
        from_simulator: SimulatorId,
        to_simulator: SimulatorId,
        observation: ObjectObservation,
    ) -> (bool, Option<Applied>) {
        let Some(to) = self.region_for(to_simulator) else { return (false, None) };
        if observation.source.region() != to || observation.kind != Kind::Avatar {
            return (false, None);
        }
        let Some(entity) = self.world.entity(observation.id) else {
            let applied = match self.world.observe_object(observation) {
                Ok(applied) => applied,
                Err(_) => return (false, None),
            };
            let introduced = matches!(
                applied.outcome,
                Introduced(entity) if self.world.object(entity).is_some_and(|object| object.address().region() == to)
            );
            return (introduced, Some(applied));
        };
        let Some(current_region) = self.world.object(entity).map(|object| object.address().region()) else { return (false, None) };
        if current_region == to {
            return (true, None);
        }
        let Some(from) = self.region_for(from_simulator).filter(|region| *region == current_region) else { return (false, None) };
        let applied = match self.world.handoff_agent(from, observation) {
            Ok(applied) => applied,
            Err(error) => {
                tracing::debug!(simulator = to_simulator.get(), region = to.get(), %error, "agent region handoff is waiting for valid source state");
                return (false, None);
            }
        };
        let completed = matches!(
            applied.outcome,
            AgentHandedOff(entity) if self.world.object(entity).is_some_and(|object| object.address().region() == to)
        );
        (completed, Some(applied))
    }

    pub(super) fn retire_simulator(&mut self, simulator_id: SimulatorId) -> Result<(), ApplyError> {
        let Some(region) = self.region_for(simulator_id) else { return Ok(()) };
        let _ = self.world.retire_region(region)?;
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

fn resolve_region_handle(
    message_handle: Option<RegionHandle>,
    current_handle: Option<RegionHandle>,
) -> Result<Option<RegionHandle>, IngestStatus> {
    if let Some(handle) = message_handle {
        if current_handle.is_some_and(|current| current != handle) {
            return Err(IngestStatus::RegionHandleMismatch);
        }
        return Ok(Some(handle));
    }
    Ok(current_handle)
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
        max_entities_per_region: MAX_WORLD_ENTITIES,
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
    use std::{
        num::NonZeroU32,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    use math::{DVec3, Vec3};
    use network::{SimulatorId, testing::simulator_id};
    use tokio::sync::oneshot::channel;
    use utils::AssetId;
    use uuid::Uuid;
    use world::{
        Appearance, AvatarAppearanceUpdate, AvatarTextureSlot, AvatarTextureSlots, AvatarVisualParamIndex, FieldUpdate, Kind, LocalAddress,
        MAX_AVATAR_TEXTURE_SLOTS, MediaEntry, ObjectMediaUpdate, ObjectObservation, ObjectPatch, OrderEvidence, Parent, ParentUpdate,
        PbrMaterialAssignments, Pose, Region, RegionHandle, RegionInfo, Shape, UpdateSource,
    };

    use crate::session::inbound::{AvatarAppearanceMessage, ObjectBatch, ObjectMediaRequest, ObjectOperation};

    use super::{
        BlockOutcome, IngestStatus, IngestSummary, MAX_OBJECTS_PER_INTAKE_QUANTUM, MAX_WORLD_ENTITIES, RegistrationError, SessionWorld,
        viewer_world_limits,
    };
    use world::ApplyError::InvalidValue;

    #[test]
    fn packet_handle_is_checked_against_committed_world_metadata() {
        let simulator = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let region = world.register_simulator(simulator, None).unwrap();
        let handle = RegionHandle::new(1 << 32);
        let _ = world.world.patch_region(region, RegionInfo { handle: Some(handle), ..RegionInfo::default() }).unwrap();
        let revision = world.world.revision();
        let summary = world
            .apply_object_batch(
                simulator,
                ObjectBatch {
                    region,
                    region_handle: Some(RegionHandle::new(0)),
                    blocks: vec![Some(ObjectOperation::Observe(observation(
                        region,
                        1,
                        1,
                        NonZeroU32::new(1).unwrap(),
                        ParentUpdate::Root,
                    )))],
                    invalid_blocks: 0,
                    invalid_fields: 0,
                },
            )
            .unwrap();

        assert_eq!(summary.status, IngestStatus::RegionHandleMismatch);
        assert_eq!(summary.accepted_region_handle, None);
        assert_eq!(world.world.object_count(), 0);
        assert_eq!(world.world.revision(), revision);
    }

    #[test]
    fn rejected_handle_metadata_does_not_admit_object_blocks() {
        let simulator = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let region = world.register_simulator(simulator, None).unwrap();
        let _ = world.world.patch_region(region, RegionInfo { origin: Some(DVec3::X), ..RegionInfo::default() }).unwrap();
        let revision = world.world.revision();
        assert!(matches!(
            world.register_simulator(simulator, Some(RegionHandle::new(0))),
            Err(RegistrationError::Apply(InvalidValue { field: "region_origin" }))
        ));
        let summary = world
            .apply_object_batch(
                simulator,
                ObjectBatch {
                    region,
                    region_handle: Some(RegionHandle::new(0)),
                    blocks: vec![Some(ObjectOperation::Observe(observation(
                        region,
                        1,
                        1,
                        NonZeroU32::new(1).unwrap(),
                        ParentUpdate::Root,
                    )))],
                    invalid_blocks: 0,
                    invalid_fields: 0,
                },
            )
            .unwrap();

        assert_eq!(summary.status, IngestStatus::RegionMetadataRejected(InvalidValue { field: "region_origin" }));
        assert_eq!(summary.accepted_region_handle, None);
        assert_eq!(world.world.object_count(), 0);
        assert_eq!(world.world.revision(), revision);
        assert_eq!(world.world.region(region).unwrap().handle, None);
        assert_eq!(world.region_for(simulator), Some(region));
    }

    fn observation(region: Region, order: u32, id: u128, local_id: NonZeroU32, parent: ParentUpdate) -> ObjectObservation {
        ObjectObservation::new(
            UpdateSource::new(region, order, OrderEvidence::Ordered(u64::from(order)), 0),
            Uuid::from_u128(id),
            local_id,
            Kind::Object,
            parent,
        )
    }

    fn apply_one(world: &mut SessionWorld, simulator_id: SimulatorId, region: Region, operation: ObjectOperation) -> IngestSummary {
        world
            .apply_object_batch(
                simulator_id,
                ObjectBatch { region, region_handle: None, blocks: vec![Some(operation)], invalid_blocks: 0, invalid_fields: 0 },
            )
            .expect("registered simulator")
    }

    #[test]
    fn dense_region_fits_world_budget() {
        let simulator_id = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let region = world.register_simulator(simulator_id, None).expect("register simulator");
        let blocks = (1..=10_001u32)
            .map(|local_id| {
                Some(ObjectOperation::Observe(observation(
                    region,
                    local_id,
                    u128::from(local_id),
                    NonZeroU32::new(local_id).expect("nonzero local ID"),
                    ParentUpdate::Root,
                )))
            })
            .collect();

        let summary = world
            .apply_object_batch(simulator_id, ObjectBatch { region, region_handle: None, blocks, invalid_blocks: 0, invalid_fields: 0 })
            .expect("registered simulator");

        assert!(summary.blocks.iter().all(|block| block.applied().is_some()));
        assert_eq!(world.world().objects().iter().count(), 10_001);
        assert_eq!(viewer_world_limits().max_entities_per_region, MAX_WORLD_ENTITIES);
    }

    #[tokio::test]
    async fn batch_yields_between_chunks() {
        let simulator_id = simulator_id();
        let mut session = SessionWorld::new(viewer_world_limits());
        let region = session.register_simulator(simulator_id, None).expect("register simulator");
        let before_revision = session.world().revision().get();
        let mut blocks = (1..=MAX_OBJECTS_PER_INTAKE_QUANTUM as u32)
            .map(|local_id| {
                Some(ObjectOperation::Observe(observation(
                    region,
                    local_id,
                    u128::from(local_id),
                    NonZeroU32::new(local_id).expect("nonzero local ID"),
                    ParentUpdate::Root,
                )))
            })
            .collect::<Vec<_>>();
        blocks.push(None);

        let (first_chunk_tx, first_chunk_rx) = channel();
        let observed_yield = Arc::new(AtomicBool::new(false));
        let observer_state = Arc::clone(&observed_yield);
        let observer = tokio::spawn(async move {
            first_chunk_rx.await.expect("first batch chunk is observed");
            observer_state.store(true, Ordering::Relaxed);
        });
        let mut first_chunk_tx = Some(first_chunk_tx);
        let worker = tokio::spawn(async move {
            let summary = session
                .apply_object_batch_cooperatively(
                    simulator_id,
                    ObjectBatch { region, region_handle: None, blocks, invalid_blocks: 1, invalid_fields: 2 },
                    |_, _, _| {
                        if let Some(sender) = first_chunk_tx.take() {
                            sender.send(()).expect("observer waits for the first chunk");
                        } else {
                            assert!(observed_yield.load(Ordering::Relaxed), "the batch yields before processing another chunk");
                        }
                    },
                )
                .await
                .expect("registered simulator");
            (session, summary)
        });
        let (session, summary) = worker.await.expect("World worker");
        observer.await.expect("observer runs after the cooperative yield");
        assert_eq!(session.world().objects().iter().count(), MAX_OBJECTS_PER_INTAKE_QUANTUM);
        assert_eq!(session.world().revision().get(), before_revision + MAX_OBJECTS_PER_INTAKE_QUANTUM as u64);
        assert_eq!(summary.blocks.len(), MAX_OBJECTS_PER_INTAKE_QUANTUM + 1);
        assert_eq!(summary.blocks.last(), Some(&BlockOutcome::Invalid));
        assert_eq!(summary.invalid_blocks, 1);
        assert_eq!(summary.invalid_fields, 2);
        assert!(summary.blocks[..MAX_OBJECTS_PER_INTAKE_QUANTUM].iter().all(|block| block.applied().is_some()));
    }

    #[test]
    fn recovery_routes_follow_active_region_lifetimes() {
        let mut session = SessionWorld::new(viewer_world_limits());
        let source_simulator = simulator_id();
        let destination_simulator = simulator_id();
        let source_region = session.register_simulator(source_simulator, None).expect("source simulator");
        let destination_region = session.register_simulator(destination_simulator, None).expect("destination simulator");

        assert_eq!(session.simulator_for_region(source_region), Some(source_simulator));
        assert_eq!(session.simulator_for_region(destination_region), Some(destination_simulator));
        assert_eq!(session.simulator_for_region(Region::new(u64::MAX)), None);

        session.retire_simulator(source_simulator).expect("retire source simulator");
        assert_eq!(session.region_for(source_simulator), None);
        assert_eq!(session.simulator_for_region(source_region), None);
        assert_eq!(session.simulator_for_region(destination_region), Some(destination_simulator));
        let revision = session.world().revision();
        session.retire_simulator(source_simulator).expect("repeated retirement");
        assert_eq!(session.world().revision(), revision);
    }

    #[test]
    fn local_intake_limits_preserve_existing_objects() {
        let simulator_id = simulator_id();
        let mut limits = viewer_world_limits();
        limits.max_entities = 1;
        limits.max_entities_per_region = 1;
        let mut session = SessionWorld::new(limits);
        let region = session.register_simulator(simulator_id, None).expect("register simulator");
        let local_id = NonZeroU32::new(1).expect("nonzero local ID");
        let summary = session
            .apply_object_batch(
                simulator_id,
                ObjectBatch {
                    region,
                    region_handle: None,
                    blocks: vec![Some(ObjectOperation::Observe(observation(region, 1, 0x1001, local_id, ParentUpdate::Root))), None],
                    invalid_blocks: 1,
                    invalid_fields: 2,
                },
            )
            .expect("registered simulator");
        assert_eq!(session.world().objects().iter().count(), 1);
        assert_eq!(summary.invalid_blocks, 1);
        assert_eq!(summary.invalid_fields, 2);

        let rejected = session
            .apply_object_batch(
                simulator_id,
                ObjectBatch {
                    region,
                    region_handle: None,
                    blocks: vec![Some(ObjectOperation::Observe(observation(
                        region,
                        2,
                        0x1002,
                        NonZeroU32::new(2).expect("nonzero local ID"),
                        ParentUpdate::Root,
                    )))],
                    invalid_blocks: 0,
                    invalid_fields: 0,
                },
            )
            .expect("registered simulator");
        assert!(matches!(rejected.blocks[0], BlockOutcome::Rejected { error: world::ApplyError::EntityBudgetExhausted, .. }));
        assert!(rejected.recovery_targets().next().is_none());

        let success = apply_one(
            &mut session,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 3, 0x1001, local_id, ParentUpdate::Root)),
        );
        assert!(success.blocks[0].applied().is_some());
        assert_eq!(session.world().objects().iter().count(), 1);
    }

    #[test]
    fn stale_full_observation_does_not_cancel_pending_parent_recovery() {
        let simulator_id = simulator_id();
        let mut session = SessionWorld::new(viewer_world_limits());
        let region = session.register_simulator(simulator_id, None).expect("register simulator");
        let parent_local_id = NonZeroU32::new(2).expect("parent local ID is nonzero");
        let child_local_id = NonZeroU32::new(1).expect("child local ID is nonzero");
        let child_address = LocalAddress::new(region, child_local_id);

        apply_one(
            &mut session,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 1, 0x1002, parent_local_id, ParentUpdate::Root)),
        );
        let mut child = observation(region, 2, 0x1001, child_local_id, ParentUpdate::Local(parent_local_id));
        child.kind = Kind::Avatar;
        let introduced = apply_one(&mut session, simulator_id, region, ObjectOperation::Observe(child.clone()));
        assert!(introduced.blocks[0].applied().unwrap().recovery.satisfied);

        apply_one(
            &mut session,
            simulator_id,
            region,
            ObjectOperation::Remove { source: UpdateSource::new(region, 3, OrderEvidence::Ordered(3), 0), local_id: parent_local_id },
        );
        let child_entity = session.world().entity(child.id).expect("child remains after parent removal");
        assert!(matches!(session.world().object(child_entity).unwrap().parent(), Parent::Pending { .. }));

        let stale = apply_one(&mut session, simulator_id, region, ObjectOperation::Observe(child));
        assert!(!stale.recovery_cancellation_addresses().any(|address| address == child_address));
        assert!(matches!(session.world().object(child_entity).unwrap().parent(), Parent::Pending { .. }));

        let mut repaired = observation(region, 4, 0x1001, child_local_id, ParentUpdate::Root);
        repaired.kind = Kind::Avatar;
        let repaired = apply_one(&mut session, simulator_id, region, ObjectOperation::Observe(repaired));
        assert!(repaired.recovery_cancellation_addresses().any(|address| address == child_address));
    }

    #[test]
    fn pbr_assignments_preserve_partial_region_state() {
        let first_simulator = simulator_id();
        let second_simulator = simulator_id();
        let mut session = SessionWorld::new(viewer_world_limits());
        let first_region = session.register_simulator(first_simulator, None).expect("register first simulator");
        let second_region = session.register_simulator(second_simulator, None).expect("register second simulator");
        let local_id = NonZeroU32::new(7).expect("nonzero local ID");

        let partial = observation(first_region, 1, 0x701, local_id, ParentUpdate::Root);
        apply_one(&mut session, first_simulator, first_region, ObjectOperation::Observe(partial));

        let mut volume = observation(second_region, 1, 0x702, local_id, ParentUpdate::Root);
        volume.shape = FieldUpdate::Set(Shape::Prim(Default::default()));
        volume.appearance = FieldUpdate::Set(Appearance::Surface);
        volume.pbr_materials = Some(PbrMaterialAssignments::new(vec![(0, AssetId::new(Uuid::from_u128(0x720)))]).unwrap());
        apply_one(&mut session, second_simulator, second_region, ObjectOperation::Observe(volume));

        let incremental_patch = |region, order, material_id| {
            let mut patch = ObjectPatch::new(UpdateSource::new(region, order, OrderEvidence::Ordered(u64::from(order)), 0), local_id);
            patch.pbr_materials = Some(PbrMaterialAssignments::new(vec![(0, AssetId::new(Uuid::from_u128(material_id)))]).unwrap());
            ObjectOperation::Patch(patch)
        };

        apply_one(&mut session, first_simulator, first_region, incremental_patch(first_region, 2, 0x730));
        {
            let state = session.world();
            let first = state.object(state.entity_at(LocalAddress::new(first_region, local_id)).unwrap()).unwrap();
            let second = state.object(state.entity_at(LocalAddress::new(second_region, local_id)).unwrap()).unwrap();
            assert_eq!(first.pbr_materials().unwrap().material(0), Some(AssetId::new(Uuid::from_u128(0x730))));
            assert_eq!(second.pbr_materials().unwrap().material(0), Some(AssetId::new(Uuid::from_u128(0x720))));
        }

        let mut classify = ObjectPatch::new(UpdateSource::new(first_region, 3, OrderEvidence::Ordered(3), 0), local_id);
        classify.appearance = FieldUpdate::Set(Appearance::Surface);
        apply_one(&mut session, first_simulator, first_region, ObjectOperation::Patch(classify));

        apply_one(&mut session, second_simulator, second_region, incremental_patch(second_region, 2, 0x731));
        let state = session.world();
        let first = state.object(state.entity_at(LocalAddress::new(first_region, local_id)).unwrap()).unwrap();
        let second = state.object(state.entity_at(LocalAddress::new(second_region, local_id)).unwrap()).unwrap();
        assert_eq!(first.pbr_materials().unwrap().material(0), Some(AssetId::new(Uuid::from_u128(0x730))));
        assert_eq!(second.pbr_materials().unwrap().material(0), Some(AssetId::new(Uuid::from_u128(0x731))));
    }

    #[test]
    fn avatar_targets_sender() {
        let first_simulator = simulator_id();
        let other_simulator = simulator_id();
        let mut session = SessionWorld::new(viewer_world_limits());
        let region = session.register_simulator(first_simulator, None).expect("register simulator");
        let other_region = session.register_simulator(other_simulator, None).expect("register other simulator");
        let local_id = NonZeroU32::new(7).expect("nonzero local ID");
        let avatar_id = Uuid::from_u128(0x801);
        let mut avatar = observation(region, 1, 0x801, local_id, ParentUpdate::Root);
        avatar.kind = Kind::Avatar;
        avatar.appearance = FieldUpdate::Set(Appearance::Avatar);
        apply_one(&mut session, first_simulator, region, ObjectOperation::Observe(avatar));
        let other = observation(other_region, 1, 0x802, local_id, ParentUpdate::Root);
        apply_one(&mut session, other_simulator, other_region, ObjectOperation::Observe(other));

        let mut texture_values = [None; MAX_AVATAR_TEXTURE_SLOTS];
        texture_values[usize::from(AvatarTextureSlot::HEAD_BAKED.index())] = Some(AssetId::new(Uuid::from_u128(0x803)));
        let update = AvatarAppearanceUpdate::new(
            Some(1),
            None,
            vec![(AvatarVisualParamIndex::new(0).unwrap(), 17)],
            Some(AvatarTextureSlots::new(texture_values).unwrap()),
        )
        .unwrap();
        let message = AvatarAppearanceMessage { object_id: avatar_id, update, invalid_fields: 0 };
        let source = UpdateSource::new(region, 2, OrderEvidence::Ordered(2), 0);

        assert!(session.apply_avatar_appearance(other_simulator, source, message.clone()).is_none());
        assert!(session.apply_avatar_appearance(first_simulator, source, message).is_some_and(|result| result.is_ok()));

        let state = session.world();
        let avatar = state.object(state.entity_at(LocalAddress::new(region, local_id)).unwrap()).unwrap();
        let other = state.object(state.entity_at(LocalAddress::new(other_region, local_id)).unwrap()).unwrap();
        let appearance = avatar.avatar_appearance().unwrap();
        assert_eq!(appearance.visual_param(AvatarVisualParamIndex::new(0).unwrap()), Some(17));
        assert_eq!(
            appearance.texture_slots().and_then(|slots| slots.texture(AvatarTextureSlot::HEAD_BAKED)),
            Some(AssetId::new(Uuid::from_u128(0x803)))
        );
        assert_eq!(avatar.surface_appearance(), None);
        assert_eq!(other.avatar_appearance(), None);
    }

    #[test]
    fn avatar_targets_local_avatar() {
        let first_simulator = simulator_id();
        let other_simulator = simulator_id();
        let mut session = SessionWorld::new(viewer_world_limits());
        let region = session.register_simulator(first_simulator, None).expect("register simulator");
        let _other_region = session.register_simulator(other_simulator, None).expect("register other simulator");
        let local_id = NonZeroU32::new(7).expect("nonzero local ID");
        let object = observation(region, 1, 0x811, local_id, ParentUpdate::Root);
        apply_one(&mut session, first_simulator, region, ObjectOperation::Observe(object));
        let update = AvatarAppearanceUpdate::default();
        let source = UpdateSource::new(region, 2, OrderEvidence::Ordered(2), 0);

        assert!(
            session
                .apply_avatar_appearance(
                    first_simulator,
                    source,
                    AvatarAppearanceMessage { object_id: Uuid::from_u128(0x811), update: update.clone(), invalid_fields: 0 }
                )
                .is_none()
        );
        assert!(
            session
                .apply_avatar_appearance(
                    first_simulator,
                    source,
                    AvatarAppearanceMessage { object_id: Uuid::from_u128(0x812), update: update.clone(), invalid_fields: 0 }
                )
                .is_none()
        );
        assert!(
            session
                .apply_avatar_appearance(
                    other_simulator,
                    source,
                    AvatarAppearanceMessage { object_id: Uuid::from_u128(0x813), update, invalid_fields: 0 },
                )
                .is_none()
        );
    }

    #[test]
    fn media_results_reject_stale_targets() {
        let sim_id = simulator_id();
        let other_simulator = simulator_id();
        let mut session = SessionWorld::new(viewer_world_limits());
        let region = session.register_simulator(sim_id, None).expect("register simulator");
        let other_region = session.register_simulator(other_simulator, None).expect("register other simulator");
        let local_id = NonZeroU32::new(7).expect("nonzero local ID");
        let object_id = Uuid::from_u128(0x703);
        let mut volume = observation(region, 1, 0x703, local_id, ParentUpdate::Root);
        volume.shape = FieldUpdate::Set(Shape::Prim(Default::default()));
        volume.appearance = FieldUpdate::Set(Appearance::Surface);
        apply_one(&mut session, sim_id, region, ObjectOperation::Observe(volume));

        let request =
            ObjectMediaRequest { source: UpdateSource::new(region, 2, OrderEvidence::Ordered(2), 0), object_id, local_id, version: 5 };
        assert!(session.prepare_object_media(other_simulator, request).is_none());
        let target = session.prepare_object_media(sim_id, request).expect("matching object target");
        let stale = ObjectMediaUpdate::new(4, vec![Some(MediaEntry::default())]).expect("valid media response");
        assert!(session.apply_object_media(target, stale).is_none());
        let update = ObjectMediaUpdate::new(5, vec![Some(MediaEntry::default())]).expect("valid media response");
        assert!(matches!(session.apply_object_media(target, update), Some(Ok(_))));

        let remove = UpdateSource::new(region, 3, OrderEvidence::Ordered(3), 0);
        apply_one(&mut session, sim_id, region, ObjectOperation::Remove { source: remove, local_id });
        let mut replacement = observation(region, 4, 0x703, local_id, ParentUpdate::Root);
        replacement.shape = FieldUpdate::Set(Shape::Prim(Default::default()));
        replacement.appearance = FieldUpdate::Set(Appearance::Surface);
        apply_one(&mut session, sim_id, region, ObjectOperation::Observe(replacement));

        let late = ObjectMediaUpdate::new(6, vec![Some(MediaEntry::default())]).expect("valid late response");
        assert!(session.apply_object_media(target, late).is_none());
        let state = session.world();
        assert_eq!(state.entity_at(LocalAddress::new(other_region, local_id)), None);
        let current = state.object(state.entity_at(LocalAddress::new(region, local_id)).unwrap()).unwrap();
        assert_eq!(current.object_media(), None);
    }

    #[test]
    fn media_results_reject_rebound_address_aba_targets() {
        let simulator_id = simulator_id();
        let mut session = SessionWorld::new(viewer_world_limits());
        let region = session.register_simulator(simulator_id, None).expect("register simulator");
        let local_id = NonZeroU32::new(7).expect("nonzero local ID");
        let object_id = Uuid::from_u128(0x704);
        let mut volume = observation(region, 1, 0x704, local_id, ParentUpdate::Root);
        volume.shape = FieldUpdate::Set(Shape::Prim(Default::default()));
        volume.appearance = FieldUpdate::Set(Appearance::Surface);
        apply_one(&mut session, simulator_id, region, ObjectOperation::Observe(volume));

        let request =
            ObjectMediaRequest { source: UpdateSource::new(region, 2, OrderEvidence::Ordered(2), 0), object_id, local_id, version: 5 };
        let stale_target = session.prepare_object_media(simulator_id, request).expect("initial media target");

        let other_local_id = NonZeroU32::new(8).expect("nonzero replacement local ID");
        apply_one(
            &mut session,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 3, 0x704, other_local_id, ParentUpdate::Root)),
        );
        apply_one(
            &mut session,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 4, 0x704, local_id, ParentUpdate::Root)),
        );

        let rebound_target = session
            .prepare_object_media(
                simulator_id,
                ObjectMediaRequest { source: UpdateSource::new(region, 5, OrderEvidence::Ordered(5), 0), ..request },
            )
            .expect("target after address is rebound");
        assert_ne!(stale_target.binding, rebound_target.binding);

        let late = ObjectMediaUpdate::new(5, vec![Some(MediaEntry::default())]).expect("valid media response");
        assert!(session.apply_object_media(stale_target, late).is_none());
        let current = ObjectMediaUpdate::new(5, vec![Some(MediaEntry::default())]).expect("valid media response");
        assert!(matches!(session.apply_object_media(rebound_target, current), Some(Ok(_))));

        let entity = session.world().entity_at(LocalAddress::new(region, local_id)).expect("rebound object");
        let object = session.world().object(entity).expect("object is retained");
        assert_eq!(object.id(), object_id);
        assert_eq!(object.object_media().map(|media| media.version()), Some(5));
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
        assert!(partial.blocks[0].applied().unwrap().revision.is_some());
        assert_eq!(partial.recovery_cancellation_addresses().count(), 0);

        let mut parent = ObjectPatch::new(UpdateSource::new(region, 6, OrderEvidence::Ordered(6), 0), child_local_id);
        parent.parent = Some(ParentUpdate::Root);
        let refreshed = apply_one(&mut world, simulator_id, region, ObjectOperation::Patch(parent));
        assert_eq!(refreshed.recovery_cancellation_addresses().collect::<Vec<_>>(), [child_address]);
        let state = world.world();
        let child = state.object(state.entity_at(child_address).unwrap()).unwrap();
        assert_eq!(child.parent(), Parent::Root);
        assert!(state.entity_at(parent_address).is_none());
    }

    #[test]
    fn fresh_full_observation_satisfies_cached_recovery_but_stale_and_partial_updates_do_not() {
        let simulator_id = simulator_id();
        let mut session = SessionWorld::new(viewer_world_limits());
        let region = session.register_simulator(simulator_id, None).expect("register simulator");
        let local_id = NonZeroU32::new(2).expect("nonzero local ID");
        let address = LocalAddress::new(region, local_id);
        apply_one(
            &mut session,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 1, 0x1201, local_id, ParentUpdate::Root)),
        );

        let cached = apply_one(
            &mut session,
            simulator_id,
            region,
            ObjectOperation::Cached { source: UpdateSource::new(region, 2, OrderEvidence::Ordered(2), 0), local_id },
        );
        assert_eq!(cached.recovery_targets().collect::<Vec<_>>(), [address]);

        let full = apply_one(
            &mut session,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 3, 0x1201, local_id, ParentUpdate::Root)),
        );
        assert_eq!(full.blocks[0].applied().unwrap().outcome, world::ApplyOutcome::Unchanged);
        assert_eq!(full.recovery_cancellation_addresses().collect::<Vec<_>>(), [address]);

        let cached = apply_one(
            &mut session,
            simulator_id,
            region,
            ObjectOperation::Cached { source: UpdateSource::new(region, 4, OrderEvidence::Ordered(4), 0), local_id },
        );
        assert_eq!(cached.recovery_targets().collect::<Vec<_>>(), [address]);
        let stale = apply_one(
            &mut session,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 3, 0x1201, local_id, ParentUpdate::Root)),
        );
        assert_eq!(stale.blocks[0].applied().unwrap().outcome, world::ApplyOutcome::IgnoredStale);
        assert_eq!(stale.recovery_cancellation_addresses().count(), 0);

        let mut partial = ObjectPatch::new(UpdateSource::new(region, 5, OrderEvidence::Ordered(5), 0), local_id);
        partial.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
        let partial = apply_one(&mut session, simulator_id, region, ObjectOperation::Patch(partial));
        assert_eq!(partial.recovery_cancellation_addresses().count(), 0);
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

        let state = world.world();
        let child = state.object(state.entity_at(child_address).expect("child remains live")).expect("child object");
        let parent = state.entity_at(parent_address).expect("parent reintroduced");
        assert_eq!(child.parent(), Parent::Resolved(parent));
    }
}
