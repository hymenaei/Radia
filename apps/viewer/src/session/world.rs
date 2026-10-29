use std::collections::HashMap;

use network::SimulatorId;
use uuid::Uuid;
use world::{
    Appearance, Applied, ApplyError,
    ApplyOutcome::{AgentHandedOff, Introduced, TombstoneRecorded},
    EnvironmentAssignment, EnvironmentTarget, FieldRejectionReason, Kind, LocalAddress, ObjectMediaUpdate, ObjectObservation, ObjectPatch,
    OrderEvidence, ParcelLocalId, ParcelOverlayObservation, ParcelPropertiesObservation, Pose, Region, RegionApplied, RegionHandle,
    RegionInfo, RegionOutcome, TerrainSettings, UpdateSource, World, WorldLimits,
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
const PARCEL_REJECTION_LOG_INTERVAL: u64 = 64;

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
    IdentityExhausted,
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
                        | world::ApplyError::ParcelBudgetExhausted
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
    parcel_rejections: u64,
    next_region_id: Option<u64>,
}

impl SessionWorld {
    pub(super) fn new(limits: WorldLimits) -> Self {
        let world = World::new(limits);
        Self { world, regions: HashMap::new(), parcel_rejections: 0, next_region_id: Some(1) }
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

    pub(super) fn clear_agent_parcel(&mut self, region: Region) -> Result<RegionApplied, ApplyError> {
        self.world.clear_agent_parcel(region)
    }

    pub(super) fn observe_parcel_overlay(
        &mut self,
        source: UpdateSource,
        observation: ParcelOverlayObservation,
    ) -> Result<RegionApplied, ApplyError> {
        self.world.observe_parcel_overlay(source, observation)
    }

    pub(super) fn observe_parcel_properties(
        &mut self,
        source: UpdateSource,
        observation: ParcelPropertiesObservation,
    ) -> Result<RegionApplied, ApplyError> {
        self.world.observe_parcel_properties(source, observation)
    }

    pub(super) fn observe_region_flags(
        &mut self,
        source: UpdateSource,
        low: Option<u32>,
        extended: Option<u64>,
    ) -> Result<RegionApplied, ApplyError> {
        self.world.observe_region_flags(source, low, extended)
    }

    pub(super) fn record_parcel_rejection(&mut self) -> Option<u64> {
        self.parcel_rejections = self.parcel_rejections.saturating_add(1);
        (self.parcel_rejections != u64::MAX && self.parcel_rejections.is_multiple_of(PARCEL_REJECTION_LOG_INTERVAL))
            .then_some(self.parcel_rejections)
    }

    pub(super) fn observe_environment(
        &mut self,
        target: EnvironmentTarget,
        source: UpdateSource,
        assignment: EnvironmentAssignment,
    ) -> Result<RegionApplied, ApplyError> {
        self.world.observe_environment(target, source, assignment)
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

        let region_id = self.next_region_id.ok_or(RegistrationError::IdentityExhausted)?;
        let region = Region::new(region_id);
        self.world.adopt_region(region).map(|_| ()).map_err(RegistrationError::Apply)?;
        self.regions.insert(simulator_id, region);
        self.next_region_id = region_id.checked_add(1);
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

    pub(super) fn simulator_ids(&self) -> impl Iterator<Item = SimulatorId> + '_ {
        self.regions.keys().copied()
    }

    pub(super) fn update_region_metadata(&mut self, region: Region, info: RegionInfo) -> Result<(), ApplyError> {
        self.world.patch_region(region, info).map(|_| ())
    }

    pub(super) fn update_water_height(&mut self, source: UpdateSource, height: f32) -> Result<(), ApplyError> {
        self.world.observe_water_height(source, height).map(|_| ())
    }

    pub(super) fn update_terrain_settings(
        &mut self,
        source: UpdateSource,
        detail_textures: [Uuid; 4],
        start_heights: [f32; 4],
        height_ranges: [f32; 4],
    ) -> Result<(), ApplyError> {
        let settings = TerrainSettings::new(detail_textures, start_heights, height_ranges)
            .map_err(|_| ApplyError::InvalidValue { field: "terrain_settings" })?;
        self.world.observe_terrain_settings(source, settings).map(|_| ())
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

    pub(super) fn apply_teleport_local(
        &mut self,
        simulator_id: SimulatorId,
        agent_id: Uuid,
        source: UpdateSource,
        position: math::Vec3,
    ) -> Option<Result<(), ApplyError>> {
        let address = self.object_address(simulator_id, agent_id)?;
        if source.region() != address.region() || !position.is_finite() {
            return Some(Err(ApplyError::InvalidValue { field: "teleport_position" }));
        }
        let mut patch = ObjectPatch::new(source, address.local_id());
        patch.pose = Some(Pose { position: Some(position), rotation: None });
        Some(self.world.patch_object(patch).map(|_| ()))
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

    pub(super) fn retire_all_except(&mut self, keep: SimulatorId) -> Vec<(SimulatorId, ApplyError)> {
        let simulator_ids = self.regions.keys().copied().filter(|simulator_id| *simulator_id != keep).collect::<Vec<_>>();
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
#[path = "world_tests.rs"]
mod tests;
