use std::collections::HashMap;

use base::UUID;
use network::SimulatorId;
use world::{
    AgentHandoff, Appearance, Applied, ApplyError,
    ApplyOutcome::{AgentHandedOff, Introduced, TombstoneRecorded},
    FieldRejectionReason, Kind, LocalAddress, MAX_QUERY_STATE_MAINTENANCE_WORK, ObjectMediaUpdate, ObjectObservation, ObjectPatch,
    OrderEvidence, Region, RegionHandle, RegionInfo, UpdateSource, World, WorldLimits,
};

use super::{
    inbound::{AvatarAppearanceMessage, ObjectBatch, ObjectMediaRequest, ObjectOperation, SitTargetUpdate},
    object_media::ObjectMediaTarget,
    recovery::MAX_RECOVERY_OBJECTS_PER_REQUEST,
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
    pub(super) invalid_fields: usize,
}

impl IngestSummary {
    pub(super) fn new(status: IngestStatus) -> Self {
        Self { status, accepted_region_handle: None, blocks: Vec::new(), invalid_blocks: 0, invalid_fields: 0 }
    }

    pub(super) fn push_applied(&mut self, applied: Applied, address: Option<LocalAddress>) {
        let index = self.blocks.len();
        self.blocks.push(BlockResult {
            index,
            address,
            result: Some(Ok(applied)),
            recovery_target: None,
            recovery_satisfied: false,
            valid: true,
        });
    }

    pub(super) fn push_error(&mut self, error: ApplyError, address: Option<LocalAddress>) {
        let index = self.blocks.len();
        self.blocks.push(BlockResult {
            index,
            address,
            result: Some(Err(error)),
            recovery_target: None,
            recovery_satisfied: false,
            valid: true,
        });
    }

    pub(super) fn append(&mut self, mut other: Self) {
        let offset = self.blocks.len();
        for block in &mut other.blocks {
            block.index += offset;
        }
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
            block.recovery_target.into_iter().chain(
                block.result.iter().filter_map(|result| result.as_ref().ok()).flat_map(|applied| applied.recovery.requests.iter().copied()),
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

    pub(super) fn log_incomplete_data(&self, simulator_id: SimulatorId) {
        use ApplyError::{
            AddressGenerationExhausted, EntityBudgetExhausted, FrameGenerationExhausted, InvalidValue, KindMismatch, RegionRetired,
            RegistrationBudgetExhausted, RevisionExhausted, StaleRegion,
        };
        use FieldRejectionReason::{
            InvalidRotation, InvalidValue as RejectedValue, ParentCycle, ParentDependsOnRejectedParent, ResourceLimit, StaleFrame,
            StaleVersion, UntrustedSource,
        };

        let region_mismatch = matches!(self.status, IngestStatus::RegionHandleMismatch | IngestStatus::RegionContextMismatch);
        let mut malformed = (self.invalid_blocks as u64).saturating_add(self.invalid_fields as u64);
        let mut resource_limits = 0_u64;
        let mut rejected_addresses = [None; MAX_INGEST_DIAGNOSTIC_ADDRESSES];
        let mut rejected_address_count = 0;
        for block in &self.blocks {
            let Some(result) = &block.result else { continue };
            let mut rejected = false;
            match result {
                Err(InvalidValue { .. } | KindMismatch { .. }) => {
                    malformed = malformed.saturating_add(1);
                    rejected = true;
                }
                Err(
                    EntityBudgetExhausted
                    | RegistrationBudgetExhausted
                    | RevisionExhausted
                    | FrameGenerationExhausted
                    | AddressGenerationExhausted,
                ) => {
                    resource_limits = resource_limits.saturating_add(1);
                    rejected = true;
                }
                Err(StaleRegion | RegionRetired) => {}
                Ok(applied) => {
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
            }
            if rejected
                && rejected_address_count < MAX_INGEST_DIAGNOSTIC_ADDRESSES
                && let Some(address) = block.address
            {
                rejected_addresses[rejected_address_count] = Some(address);
                rejected_address_count += 1;
            }
        }
        if region_mismatch || malformed > 0 || resource_limits > 0 {
            let rejected_addresses = rejected_addresses[..rejected_address_count].iter().flatten().collect::<Vec<_>>();
            tracing::warn!(
                ?simulator_id,
                region_mismatch,
                malformed,
                resource_limits,
                rejected_addresses = ?rejected_addresses,
                "World update was partially rejected; world data may be incomplete"
            );
        }
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
    world: World,
    regions: HashMap<SimulatorId, SimulatorRegion>,
}

impl SessionWorld {
    pub(super) fn new(limits: WorldLimits) -> Self {
        let world = World::new(limits);
        Self { world, regions: HashMap::new() }
    }

    #[cfg(test)]
    pub(super) fn world(&self) -> &World {
        &self.world
    }

    pub(super) fn maintain_query_state(&mut self) -> bool {
        match self.world.maintain_query_state(QUERY_STATE_MAINTENANCE_WORK_PER_QUANTUM) {
            Ok(progress) => progress.has_dirty_pressure,
            Err(error) => {
                tracing::warn!(?error, "could not maintain World query state");
                false
            }
        }
    }

    pub(super) fn set_self_agent(&mut self, agent_id: UUID) {
        if let Err(error) = self.world.set_self_agent(agent_id) {
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
        self.world.adopt_region(region).map(|_| ()).map_err(RegistrationError::Apply)?;
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

    pub(super) fn simulator_for_region(&self, region: Region, preferred: SimulatorId) -> Option<SimulatorId> {
        if self.region_for(preferred) == Some(region) {
            return Some(preferred);
        }
        self.regions
            .iter()
            .filter(|(_, source)| source.region == region)
            .map(|(simulator_id, _)| *simulator_id)
            .min_by_key(|simulator_id| simulator_id.get())
    }

    pub(super) fn update_region_metadata(&mut self, region: Region, info: RegionInfo) -> Result<(), ApplyError> {
        self.world.patch_region(region, info).map(|_| ())
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
        summary.invalid_fields = batch.invalid_fields;
        for (index, operation) in batch.blocks.into_iter().enumerate() {
            let (address, result, recovery_target, recovery_satisfied, valid) = match operation {
                Some(ObjectOperation::Cached { source, local_id }) => {
                    let address = LocalAddress::new(source.region(), local_id);
                    (Some(address), None, Some(address), false, true)
                }
                Some(operation) => {
                    let address = operation.address();
                    let result = apply_operation(&mut self.world, operation);
                    let recovery_satisfied = result.as_ref().is_ok_and(|applied| applied.recovery.satisfied);
                    (Some(address), Some(result), None, recovery_satisfied, true)
                }
                None => (None, None, None, false, false),
            };
            summary.blocks.push(BlockResult { index, address, result, recovery_target, recovery_satisfied, valid });
        }
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
            let recovery_targets =
                part.recovery_targets().map(|address| (self.simulator_for_region(address.region(), simulator_id), address)).collect();
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
        let simulator = self.regions.get(&simulator_id)?;
        if simulator.region != request.source.region() || !matches!(request.source.order(), OrderEvidence::Ordered(_)) {
            return None;
        }
        let address = LocalAddress::new(simulator.region, request.local_id);
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
        let Some(simulator) = self.regions.get(&target.simulator_id) else { return false };
        if simulator.region != target.source.region() || !matches!(target.source.order(), OrderEvidence::Ordered(_)) {
            return false;
        }
        if target.binding.address().region() != simulator.region || !self.world.object_binding_target_is_current(target.binding) {
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
        let simulator = self.regions.get(&simulator_id)?;
        if simulator.region != source.region() {
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
        let simulator = self.regions.get(&simulator_id)?;
        if simulator.region != source.region() {
            return None;
        }
        let entity = self.world.entity(update.object_id)?;
        let object = self.world.object(entity)?;
        if object.id() != update.object_id || object.kind() != Kind::Avatar || object.address().region() != simulator.region {
            return None;
        }
        let local_id = object.address().local_id();
        drop(object);

        let mut patch = ObjectPatch::new(source, local_id);
        patch.avatar_appearance = Some(update.update);
        let result = self.world.patch_object(patch);
        Some(result)
    }

    pub(super) fn object_address(&self, simulator_id: SimulatorId, object_id: UUID) -> Option<LocalAddress> {
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
        let handoff = AgentHandoff {
            source: observation.source,
            local_id: observation.local_id,
            parent: observation.parent,
            pose: observation.pose,
            linear_motion: observation.linear_motion,
            angular_velocity: observation.angular_velocity,
            scale: observation.scale,
        };
        let applied = match self.world.handoff_agent(from, handoff) {
            Ok(applied) => applied,
            Err(error) => {
                tracing::debug!(?error, "agent region handoff is waiting for valid source state");
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
        let Some(source) = self.regions.remove(&simulator_id) else { return Ok(()) };
        self.world.retire_region(source.region).map(|_| ())
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
        ObjectOperation::RenderMaterialsPatch(mut patch) => {
            let address = LocalAddress::new(patch.source.region(), patch.local_id);
            let allows_render_materials = world.entity_at(address).is_none_or(|entity| {
                world.object(entity).is_some_and(|object| object.kind() == Kind::Object && object.appearance() == Some(Appearance::Surface))
            });
            if !allows_render_materials {
                patch.render_materials = None;
            }
            world.patch_object(patch)
        }
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

    use base::UUID;
    use math::Vec3;
    use network::{SimulatorId, testing::simulator_id};
    use tokio::sync::oneshot::channel;
    use world::{
        Appearance, AssetId, AvatarAppearanceUpdate, AvatarTextureSlot, AvatarTextureSlots, AvatarVisualParamIndex, FieldUpdate, Kind,
        LocalAddress, MAX_AVATAR_TEXTURE_SLOTS, MediaEntry, ObjectMediaUpdate, ObjectObservation, ObjectPatch, OrderEvidence, Parent,
        ParentUpdate, Pose, Region, RenderMaterials, Shape, UpdateSource,
    };

    use crate::session::inbound::{AvatarAppearanceMessage, ObjectBatch, ObjectMediaRequest, ObjectOperation};

    use super::{IngestSummary, MAX_OBJECTS_PER_INTAKE_QUANTUM, MAX_WORLD_ENTITIES, SessionWorld, viewer_world_limits};

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
            attachment_point: FieldUpdate::Keep,
            shape: FieldUpdate::Keep,
            appearance: FieldUpdate::Keep,
            surface_appearance: None,
            render_materials: None,
        }
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

        assert!(summary.blocks.iter().all(|block| block.result.as_ref().is_some_and(|result| result.is_ok())));
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
        assert_eq!(summary.invalid_blocks, 1);
        assert_eq!(summary.invalid_fields, 2);
        assert!(
            summary.blocks[..MAX_OBJECTS_PER_INTAKE_QUANTUM].iter().all(|block| block.result.as_ref().is_some_and(|result| result.is_ok()))
        );
    }

    #[test]
    fn recovery_routes_follow_the_address_region() {
        let mut session = SessionWorld::new(viewer_world_limits());
        let source_simulator = simulator_id();
        let destination_simulator = simulator_id();
        let source_region = session.register_simulator(source_simulator, None).expect("source simulator");
        let destination_region = session.register_simulator(destination_simulator, None).expect("destination simulator");

        assert_eq!(session.simulator_for_region(source_region, destination_simulator), Some(source_simulator));
        assert_eq!(session.simulator_for_region(destination_region, destination_simulator), Some(destination_simulator));
        assert_eq!(session.simulator_for_region(Region::new(u64::MAX), destination_simulator), None);
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
        assert!(matches!(rejected.blocks[0].result, Some(Err(world::ApplyError::EntityBudgetExhausted))));
        assert!(rejected.recovery_targets().next().is_none());

        let success = apply_one(
            &mut session,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 3, 0x1001, local_id, ParentUpdate::Root)),
        );
        assert!(success.blocks[0].result.as_ref().is_some_and(|result| result.is_ok()));
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
        assert!(introduced.blocks[0].result.as_ref().unwrap().as_ref().unwrap().recovery.satisfied);

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
    fn render_materials_require_volume_and_region() {
        let first_simulator = simulator_id();
        let second_simulator = simulator_id();
        let mut session = SessionWorld::new(viewer_world_limits());
        let first_region = session.register_simulator(first_simulator, None).expect("register first simulator");
        let second_region = session.register_simulator(second_simulator, None).expect("register second simulator");
        let local_id = NonZeroU32::new(7).expect("nonzero local ID");

        let non_volume = observation(first_region, 1, 0x701, local_id, ParentUpdate::Root);
        apply_one(&mut session, first_simulator, first_region, ObjectOperation::Observe(non_volume));

        let mut volume = observation(second_region, 1, 0x702, local_id, ParentUpdate::Root);
        volume.shape = FieldUpdate::Set(Shape::Prim(Default::default()));
        volume.appearance = FieldUpdate::Set(Appearance::Surface);
        volume.render_materials = Some(RenderMaterials::new(vec![(0, AssetId::new(UUID::from_u128(0x720)))]).unwrap());
        apply_one(&mut session, second_simulator, second_region, ObjectOperation::Observe(volume));

        let incremental_patch = |region, order, material_id| {
            let mut patch = ObjectPatch::new(UpdateSource::new(region, order, OrderEvidence::Ordered(u64::from(order)), 0), local_id);
            patch.render_materials = Some(RenderMaterials::new(vec![(0, AssetId::new(UUID::from_u128(material_id)))]).unwrap());
            ObjectOperation::RenderMaterialsPatch(patch)
        };

        apply_one(&mut session, first_simulator, first_region, incremental_patch(first_region, 2, 0x730));
        {
            let state = session.world();
            let first = state.object(state.entity_at(LocalAddress::new(first_region, local_id)).unwrap()).unwrap();
            let second = state.object(state.entity_at(LocalAddress::new(second_region, local_id)).unwrap()).unwrap();
            assert_eq!(first.render_materials(), None);
            assert_eq!(second.render_materials().unwrap().material(0), Some(AssetId::new(UUID::from_u128(0x720))));
        }

        apply_one(&mut session, second_simulator, second_region, incremental_patch(second_region, 2, 0x731));
        let state = session.world();
        let first = state.object(state.entity_at(LocalAddress::new(first_region, local_id)).unwrap()).unwrap();
        let second = state.object(state.entity_at(LocalAddress::new(second_region, local_id)).unwrap()).unwrap();
        assert_eq!(first.render_materials(), None);
        assert_eq!(second.render_materials().unwrap().material(0), Some(AssetId::new(UUID::from_u128(0x731))));
    }

    #[test]
    fn avatar_targets_sender() {
        let first_simulator = simulator_id();
        let other_simulator = simulator_id();
        let mut session = SessionWorld::new(viewer_world_limits());
        let region = session.register_simulator(first_simulator, None).expect("register simulator");
        let other_region = session.register_simulator(other_simulator, None).expect("register other simulator");
        let local_id = NonZeroU32::new(7).expect("nonzero local ID");
        let avatar_id = UUID::from_u128(0x801);
        let mut avatar = observation(region, 1, 0x801, local_id, ParentUpdate::Root);
        avatar.kind = Kind::Avatar;
        avatar.appearance = FieldUpdate::Set(Appearance::Avatar);
        apply_one(&mut session, first_simulator, region, ObjectOperation::Observe(avatar));
        let other = observation(other_region, 1, 0x802, local_id, ParentUpdate::Root);
        apply_one(&mut session, other_simulator, other_region, ObjectOperation::Observe(other));

        let mut texture_values = [None; MAX_AVATAR_TEXTURE_SLOTS];
        texture_values[usize::from(AvatarTextureSlot::HEAD_BAKED.index())] = Some(AssetId::new(UUID::from_u128(0x803)));
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
            Some(AssetId::new(UUID::from_u128(0x803)))
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
                    AvatarAppearanceMessage { object_id: UUID::from_u128(0x811), update: update.clone(), invalid_fields: 0 }
                )
                .is_none()
        );
        assert!(
            session
                .apply_avatar_appearance(
                    first_simulator,
                    source,
                    AvatarAppearanceMessage { object_id: UUID::from_u128(0x812), update: update.clone(), invalid_fields: 0 }
                )
                .is_none()
        );
        assert!(
            session
                .apply_avatar_appearance(
                    other_simulator,
                    source,
                    AvatarAppearanceMessage { object_id: UUID::from_u128(0x813), update, invalid_fields: 0 },
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
        let object_id = UUID::from_u128(0x703);
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
        let object_id = UUID::from_u128(0x704);
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
        assert!(partial.blocks[0].result.as_ref().unwrap().as_ref().unwrap().revision.is_some());
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
        assert!(matches!(full.blocks[0].result.as_ref().unwrap(), Ok(applied) if applied.outcome == world::ApplyOutcome::Unchanged));
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
        assert!(matches!(stale.blocks[0].result.as_ref().unwrap(), Ok(applied) if applied.outcome == world::ApplyOutcome::IgnoredStale));
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
