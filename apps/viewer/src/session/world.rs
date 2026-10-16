use std::{collections::HashMap, sync::Arc};

use base::UUID;
use futures::{FutureExt, pin_mut, select_biased};
use network::SimulatorId;
use tokio::sync::{mpsc, oneshot, watch};
use world::{
    AgentHandoff, Appearance, Applied, ApplyError,
    ApplyOutcome::{AgentHandedOff, Introduced, TombstoneRecorded},
    Entity, Kind, LocalAddress, MAX_OBJECT_DETAIL_BATCH_ITEMS, ObjectDetailBatch, ObjectListItemRead, ObjectListPage, ObjectMediaUpdate,
    ObjectObservation, ObjectPageCursor, ObjectPatch, OrderEvidence, Region, RegionHandle, RegionInfo, RegionPage, RegionPageCursor,
    RegionSummaryRead, UpdateSource, World, WorldChangeCursor, WorldChangePage, WorldLimits, WorldReadError, WorldRevision,
};

use super::{
    coverage::{CoverageSnapshot, SessionCoverageState},
    inbound::{AvatarAppearanceMessage, ObjectBatch, ObjectMediaRequest, ObjectOperation, SitTargetUpdate},
    object_media::ObjectMediaTarget,
    recovery::MAX_RECOVERY_OBJECTS_PER_REQUEST,
};

pub(super) const MAX_WORLD_ENTITIES: usize = 65_536;
const MAX_WORLD_TOMBSTONES_PER_REGION: usize = 4_096;
const MAX_WORLD_TOMBSTONES: usize = 16_384;
const MAX_WORLD_JOURNAL_ENTRIES: usize = 4_096;
const MAX_REGION_NAME_BYTES: usize = 256;

const WORLD_READ_QUEUE_CAPACITY: usize = 1;
pub(super) const MAX_WORLD_EVENTS_BETWEEN_READS: usize = 16;
const MAX_OBJECTS_PER_INTAKE_QUANTUM: usize = 32;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WorldReaderError {
    Busy,
    Gone,
    Read(WorldReadError),
}

#[derive(Clone)]
pub(crate) struct WorldReader {
    renderer_requests: mpsc::Sender<WorldReadRequest>,
    inspector_requests: mpsc::Sender<WorldReadRequest>,
    session: Arc<()>,
    priority: WorldReadPriority,
    revisions: watch::Receiver<WorldRevision>,
    coverage: watch::Receiver<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionReadBarrier {
    pub(crate) changes: WorldChangeCursor,
    pub(crate) revision: WorldRevision,
    pub(crate) object_count: usize,
    pub(crate) coverage: CoverageSnapshot,
}

#[derive(Clone, Copy)]
enum WorldReadPriority {
    Renderer,
    Inspector,
}

impl WorldReader {
    pub(crate) fn same_session(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.session, &other.session)
    }

    pub(crate) fn inspector(&self) -> Self {
        Self { priority: WorldReadPriority::Inspector, ..self.clone() }
    }

    pub(crate) fn subscribe_revisions(&self) -> watch::Receiver<WorldRevision> {
        self.revisions.clone()
    }

    pub(crate) fn subscribe_coverage(&self) -> watch::Receiver<u64> {
        self.coverage.clone()
    }

    pub(crate) async fn barrier(&self) -> Result<SessionReadBarrier, WorldReaderError> {
        let (reply, response) = oneshot::channel();
        self.try_send(WorldReadRequest::Barrier(reply))?;
        response.await.map_err(|_| WorldReaderError::Gone)?.map_err(WorldReaderError::Read)
    }

    pub(crate) async fn object_list_page(
        &self,
        cursor: Option<ObjectPageCursor>,
        limit: usize,
    ) -> Result<ObjectListPage, WorldReaderError> {
        let (reply, response) = oneshot::channel();
        self.try_send(WorldReadRequest::ObjectListPage { cursor, limit, reply })?;
        response.await.map_err(|_| WorldReaderError::Gone)?.map_err(WorldReaderError::Read)
    }

    pub(crate) async fn object_list_item(&self, entity: Entity) -> Result<ObjectListItemRead, WorldReaderError> {
        let (reply, response) = oneshot::channel();
        self.try_send(WorldReadRequest::ObjectListItem { entity, reply })?;
        response.await.map_err(|_| WorldReaderError::Gone)?.map_err(WorldReaderError::Read)
    }

    pub(crate) async fn region_page(&self, cursor: Option<RegionPageCursor>, limit: usize) -> Result<RegionPage, WorldReaderError> {
        let (reply, response) = oneshot::channel();
        self.try_send(WorldReadRequest::RegionPage { cursor, limit, reply })?;
        response.await.map_err(|_| WorldReaderError::Gone)?.map_err(WorldReaderError::Read)
    }

    pub(crate) async fn region_summary(&self, region: Region) -> Result<Option<RegionSummaryRead>, WorldReaderError> {
        let (reply, response) = oneshot::channel();
        self.try_send(WorldReadRequest::RegionSummary { region, reply })?;
        response.await.map_err(|_| WorldReaderError::Gone)?.map_err(WorldReaderError::Read)
    }

    pub(crate) async fn object_details(&self, entities: &[Entity]) -> Result<ObjectDetailBatch, WorldReaderError> {
        if entities.is_empty() || entities.len() > MAX_OBJECT_DETAIL_BATCH_ITEMS {
            return Err(WorldReaderError::Read(WorldReadError::InvalidLimit));
        }
        let (reply, response) = oneshot::channel();
        self.try_send(WorldReadRequest::ObjectDetails { entities: entities.to_vec(), reply })?;
        response.await.map_err(|_| WorldReaderError::Gone)?.map_err(WorldReaderError::Read)
    }

    pub(crate) async fn changes(&self, cursor: WorldChangeCursor, limit: usize) -> Result<WorldChangePage, WorldReaderError> {
        let (reply, response) = oneshot::channel();
        self.try_send(WorldReadRequest::Changes { cursor, limit, reply })?;
        response.await.map_err(|_| WorldReaderError::Gone)?.map_err(WorldReaderError::Read)
    }

    fn try_send(&self, request: WorldReadRequest) -> Result<(), WorldReaderError> {
        let requests = match self.priority {
            WorldReadPriority::Renderer => &self.renderer_requests,
            WorldReadPriority::Inspector => &self.inspector_requests,
        };
        requests.try_send(request).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => WorldReaderError::Busy,
            mpsc::error::TrySendError::Closed(_) => WorldReaderError::Gone,
        })
    }
}

pub(super) enum WorldReadRequest {
    Barrier(oneshot::Sender<Result<SessionReadBarrier, WorldReadError>>),
    ObjectListPage { cursor: Option<ObjectPageCursor>, limit: usize, reply: oneshot::Sender<Result<ObjectListPage, WorldReadError>> },
    ObjectListItem { entity: Entity, reply: oneshot::Sender<Result<ObjectListItemRead, WorldReadError>> },
    RegionPage { cursor: Option<RegionPageCursor>, limit: usize, reply: oneshot::Sender<Result<RegionPage, WorldReadError>> },
    RegionSummary { region: Region, reply: oneshot::Sender<Result<Option<RegionSummaryRead>, WorldReadError>> },
    ObjectDetails { entities: Vec<Entity>, reply: oneshot::Sender<Result<ObjectDetailBatch, WorldReadError>> },
    Changes { cursor: WorldChangeCursor, limit: usize, reply: oneshot::Sender<Result<WorldChangePage, WorldReadError>> },
}

#[derive(Default)]
pub(super) struct WorldReadBatch {
    renderer: Option<WorldReadRequest>,
    inspector: Option<WorldReadRequest>,
}

impl WorldReadBatch {
    pub(super) fn is_empty(&self) -> bool {
        self.renderer.is_none() && self.inspector.is_none()
    }

    pub(super) fn into_iter(self) -> impl Iterator<Item = WorldReadRequest> {
        self.renderer.into_iter().chain(self.inspector)
    }
}

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
    pub(super) fn new(status: IngestStatus) -> Self {
        Self { status, accepted_region_handle: None, blocks: Vec::new(), invalid_blocks: 0 }
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

    fn append(&mut self, mut other: Self) {
        let offset = self.blocks.len();
        for block in &mut other.blocks {
            block.index += offset;
        }
        self.blocks.extend(other.blocks);
        self.invalid_blocks = self.invalid_blocks.saturating_add(other.invalid_blocks);
        self.accepted_region_handle = self.accepted_region_handle.or(other.accepted_region_handle);
        if other.status != IngestStatus::Applied {
            self.status = other.status;
        }
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
    world: World,
    renderer_read_requests: mpsc::Receiver<WorldReadRequest>,
    inspector_read_requests: mpsc::Receiver<WorldReadRequest>,
    renderer_reader_requests: mpsc::Sender<WorldReadRequest>,
    inspector_reader_requests: mpsc::Sender<WorldReadRequest>,
    session: Arc<()>,
    revision_sender: watch::Sender<WorldRevision>,
    coverage: SessionCoverageState,
    regions: HashMap<SimulatorId, SimulatorRegion>,
}

impl SessionWorld {
    #[cfg(test)]
    pub(super) fn new(limits: WorldLimits) -> Self {
        Self::with_coverage(limits, SessionCoverageState::new())
    }

    pub(super) fn with_coverage(limits: WorldLimits, coverage: SessionCoverageState) -> Self {
        let world = World::new(limits);
        let (renderer_reader_requests, renderer_read_requests) = mpsc::channel(WORLD_READ_QUEUE_CAPACITY);
        let (inspector_reader_requests, inspector_read_requests) = mpsc::channel(WORLD_READ_QUEUE_CAPACITY);
        let (revision_sender, _) = watch::channel(world.revision());
        Self {
            world,
            renderer_read_requests,
            inspector_read_requests,
            renderer_reader_requests,
            inspector_reader_requests,
            session: Arc::new(()),
            revision_sender,
            coverage,
            regions: HashMap::new(),
        }
    }

    pub(super) fn reader(&self) -> WorldReader {
        WorldReader {
            renderer_requests: self.renderer_reader_requests.clone(),
            inspector_requests: self.inspector_reader_requests.clone(),
            session: Arc::clone(&self.session),
            priority: WorldReadPriority::Renderer,
            revisions: self.revision_sender.subscribe(),
            coverage: self.coverage.subscribe(),
        }
    }

    #[cfg(test)]
    pub(super) fn world(&self) -> &World {
        &self.world
    }

    pub(super) async fn next_read_requests(&mut self) -> Option<WorldReadBatch> {
        let (priority, request) = {
            let renderer = self.renderer_read_requests.recv().fuse();
            let inspector = self.inspector_read_requests.recv().fuse();
            pin_mut!(renderer, inspector);
            select_biased! {
                request = renderer => (WorldReadPriority::Renderer, request),
                request = inspector => (WorldReadPriority::Inspector, request),
            }
        };
        let request = request?;
        let mut batch = WorldReadBatch::default();
        match priority {
            WorldReadPriority::Renderer => {
                batch.renderer = Some(request);
                batch.inspector = self.inspector_read_requests.try_recv().ok();
            }
            WorldReadPriority::Inspector => {
                batch.renderer = self.renderer_read_requests.try_recv().ok();
                batch.inspector = Some(request);
            }
        }
        Some(batch)
    }

    pub(super) fn try_read_requests(&mut self) -> WorldReadBatch {
        WorldReadBatch { renderer: self.renderer_read_requests.try_recv().ok(), inspector: self.inspector_read_requests.try_recv().ok() }
    }

    pub(super) fn service_read_request(&self, request: WorldReadRequest) {
        match request {
            WorldReadRequest::Barrier(reply) if !reply.is_closed() => {
                let barrier = self.world.read_barrier();
                let coverage = self.coverage.snapshot(barrier.revision);
                let _ = reply.send(Ok(SessionReadBarrier {
                    changes: barrier.changes,
                    revision: barrier.revision,
                    object_count: barrier.object_count,
                    coverage,
                }));
            }
            WorldReadRequest::ObjectListPage { cursor, limit, reply } if !reply.is_closed() => {
                let _ = reply.send(self.world.object_list_page(cursor, limit));
            }
            WorldReadRequest::ObjectListItem { entity, reply } if !reply.is_closed() => {
                let _ = reply.send(self.world.object_list_item(entity));
            }
            WorldReadRequest::RegionPage { cursor, limit, reply } if !reply.is_closed() => {
                let _ = reply.send(self.world.region_page(cursor, limit));
            }
            WorldReadRequest::RegionSummary { region, reply } if !reply.is_closed() => {
                let _ = reply.send(self.world.region_summary(region));
            }
            WorldReadRequest::ObjectDetails { entities, reply } if !reply.is_closed() => {
                let _ = reply.send(self.world.object_detail_batch(&entities));
            }
            WorldReadRequest::Changes { cursor, limit, reply } if !reply.is_closed() => {
                let _ = reply.send(self.world.change_page(cursor, limit));
            }
            WorldReadRequest::Barrier(_)
            | WorldReadRequest::ObjectListPage { .. }
            | WorldReadRequest::ObjectListItem { .. }
            | WorldReadRequest::RegionPage { .. }
            | WorldReadRequest::RegionSummary { .. }
            | WorldReadRequest::ObjectDetails { .. }
            | WorldReadRequest::Changes { .. } => {}
        }
    }

    pub(super) fn publish_revision(&self) {
        let revision = self.world.revision();
        if *self.revision_sender.borrow() != revision {
            self.revision_sender.send_replace(revision);
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
        self.publish_revision();
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
        let result = self.world.patch_region(region, info).map(|_| ());
        self.publish_revision();
        result
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
        for (index, operation) in batch.blocks.into_iter().enumerate() {
            let (address, result, recovery_target, recovery_satisfied, valid) = match operation {
                Some(ObjectOperation::Cached { source, local_id }) => {
                    let address = LocalAddress::new(source.region(), local_id);
                    (Some(address), None, Some(address), false, true)
                }
                Some(operation) => {
                    let address = operation.address();
                    let prior_parent =
                        self.world.entity_at(address).and_then(|entity| self.world.object(entity)).map(|object| object.parent());
                    let has_parent_evidence = match &operation {
                        ObjectOperation::Observe(_) => true,
                        ObjectOperation::Patch(patch) => patch.parent.is_some(),
                        ObjectOperation::RenderMaterialsPatch(_) => false,
                        ObjectOperation::Remove { .. } | ObjectOperation::Cached { .. } => false,
                    };
                    let result = apply_operation(&mut self.world, operation);
                    let current_parent =
                        self.world.entity_at(address).and_then(|entity| self.world.object(entity)).map(|object| object.parent());
                    let recovery_satisfied = result.as_ref().is_ok_and(|applied| {
                        matches!(applied.outcome, Introduced(_)) || (has_parent_evidence && prior_parent != current_parent)
                    });
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
        self.publish_revision();
        Some(summary)
    }

    pub(super) async fn apply_object_batch_cooperatively(
        &mut self,
        simulator_id: SimulatorId,
        batch: ObjectBatch,
    ) -> Option<IngestSummary> {
        let ObjectBatch { region, region_handle, blocks, invalid_blocks } = batch;
        let mut remaining = blocks.into_iter().peekable();
        let mut summary: Option<IngestSummary> = None;
        let mut first = true;
        loop {
            let operations = remaining.by_ref().take(MAX_OBJECTS_PER_INTAKE_QUANTUM).collect::<Vec<_>>();
            if operations.is_empty() {
                break;
            }
            let has_more = remaining.peek().is_some();
            let part = self.apply_object_batch(
                simulator_id,
                ObjectBatch { region, region_handle, blocks: operations, invalid_blocks: if first { invalid_blocks } else { 0 } },
            )?;
            first = false;
            match &mut summary {
                Some(summary) => summary.append(part),
                None => summary = Some(part),
            }
            if summary.as_ref().is_some_and(|summary| summary.status != IngestStatus::Applied) {
                break;
            }
            if has_more {
                for request in self.try_read_requests().into_iter() {
                    self.service_read_request(request);
                }
                tokio::task::yield_now().await;
            }
        }
        if first {
            self.apply_object_batch(simulator_id, ObjectBatch { region, region_handle, blocks: Vec::new(), invalid_blocks })
        } else {
            summary
        }
    }

    pub(super) fn prepare_object_media(&self, simulator_id: SimulatorId, request: ObjectMediaRequest) -> Option<ObjectMediaTarget> {
        let simulator = self.regions.get(&simulator_id)?;
        if simulator.region != request.source.region() || !matches!(request.source.order(), OrderEvidence::Ordered(_)) {
            return None;
        }
        let address = LocalAddress::new(simulator.region, request.local_id);
        let entity = self.world.entity_at(address)?;
        let object = self.world.object(entity)?;
        if object.identity().id() != request.object_id
            || object.kind() != Kind::Object
            || object.appearance() != Some(Appearance::Surface)
            || object.object_media().is_some_and(|media| media.version() >= request.version)
        {
            return None;
        }
        Some(ObjectMediaTarget {
            simulator_id,
            entity,
            source: request.source,
            object_id: request.object_id,
            local_id: request.local_id,
            version: request.version,
        })
    }

    pub(super) fn object_media_target_is_current(&self, target: ObjectMediaTarget) -> bool {
        let Some(simulator) = self.regions.get(&target.simulator_id) else { return false };
        if simulator.region != target.source.region() || !matches!(target.source.order(), OrderEvidence::Ordered(_)) {
            return false;
        }
        let address = LocalAddress::new(simulator.region, target.local_id);
        let Some(entity) = self.world.entity_at(address) else { return false };
        if entity != target.entity {
            return false;
        }
        self.world.object(entity).is_some_and(|object| {
            object.identity().id() == target.object_id
                && object.kind() == Kind::Object
                && object.appearance() == Some(Appearance::Surface)
                && object.object_media().is_none_or(|media| media.version() < target.version)
        })
    }

    pub(super) fn apply_object_media(
        &mut self,
        target: ObjectMediaTarget,
        update: ObjectMediaUpdate,
    ) -> Option<Result<Applied, ApplyError>> {
        if update.version() < target.version {
            return None;
        }
        let simulator = self.regions.get(&target.simulator_id)?;
        let address = LocalAddress::new(simulator.region, target.local_id);
        if simulator.region != target.source.region() || !matches!(target.source.order(), OrderEvidence::Ordered(_)) {
            return None;
        }
        let entity = self.world.entity_at(address)?;
        if entity != target.entity {
            return None;
        }
        let object = self.world.object(entity)?;
        if object.identity().id() != target.object_id || object.kind() != Kind::Object || object.appearance() != Some(Appearance::Surface) {
            return None;
        }
        drop(object);
        let mut patch = ObjectPatch::new(target.source, target.local_id);
        patch.object_media = Some(update);
        let result = self.world.patch_object(patch);
        self.publish_revision();
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
        self.publish_revision();
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
        if object.identity().id() != update.object_id || object.kind() != Kind::Avatar || object.address().region() != simulator.region {
            return None;
        }
        let local_id = object.address().local_id();
        drop(object);

        let mut patch = ObjectPatch::new(source, local_id);
        patch.avatar_appearance = Some(update.update);
        let result = self.world.patch_object(patch);
        self.publish_revision();
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
                Err(_) => {
                    self.publish_revision();
                    return (false, None);
                }
            };
            let introduced = matches!(
                applied.outcome,
                Introduced(entity) if self.world.object(entity).is_some_and(|object| object.address().region() == to)
            );
            self.publish_revision();
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
        self.publish_revision();
        (completed, Some(applied))
    }

    pub(super) fn retire_simulator(&mut self, simulator_id: SimulatorId) -> Result<(), ApplyError> {
        let Some(source) = self.regions.remove(&simulator_id) else { return Ok(()) };
        let result = self.world.retire_region(source.region).map(|_| ());
        self.publish_revision();
        result
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
    use std::{future::Future as _, num::NonZeroU32, task::Poll};

    use base::UUID;
    use futures::{executor::block_on, future::poll_fn, pin_mut};
    use math::Vec3;
    use network::{CoverageLossReason, CoverageSource, CoverageWatermark, SimulatorId, testing::simulator_id};
    use tokio::sync::oneshot::channel;
    use world::{
        Appearance, AssetId, AvatarAppearanceUpdate, AvatarTextureSlot, AvatarTextureSlots, AvatarVisualParamIndex, Kind, LocalAddress,
        MAX_AVATAR_TEXTURE_SLOTS, MAX_OBJECT_PAGE_ITEMS, MediaEntry, ObjectMediaUpdate, ObjectObservation, ObjectPatch, OrderEvidence,
        Parent, ParentUpdate, Pose, Region, RegionHandle, RegionInfo, RenderMaterials, Shape, UpdateSource,
    };

    use crate::session::{
        CoverageHealth,
        coverage::SessionCoverageState,
        inbound::{AvatarAppearanceMessage, ObjectBatch, ObjectMediaRequest, ObjectOperation},
    };

    use super::{
        IngestSummary, MAX_OBJECTS_PER_INTAKE_QUANTUM, MAX_WORLD_ENTITIES, SessionWorld, WorldReadError, WorldReadRequest, WorldReader,
        WorldReaderError, viewer_world_limits,
    };

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
            shape: None,
            appearance: None,
            surface_appearance: None,
            render_materials: None,
        }
    }

    fn apply_one(world: &mut SessionWorld, simulator_id: SimulatorId, region: Region, operation: ObjectOperation) -> IngestSummary {
        world
            .apply_object_batch(simulator_id, ObjectBatch { region, region_handle: None, blocks: vec![Some(operation)], invalid_blocks: 0 })
            .expect("registered simulator")
    }

    fn read_barrier(session: &mut SessionWorld, reader: &WorldReader) -> super::SessionReadBarrier {
        let (reply, response) = channel();
        reader.try_send(WorldReadRequest::Barrier(reply)).expect("enqueue barrier");
        for request in session.try_read_requests().into_iter() {
            session.service_read_request(request);
        }
        block_on(response).expect("World reader response").expect("World read barrier")
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
            .apply_object_batch(simulator_id, ObjectBatch { region, region_handle: None, blocks, invalid_blocks: 0 })
            .expect("registered simulator");

        assert!(summary.blocks.iter().all(|block| block.result.as_ref().is_some_and(|result| result.is_ok())));
        assert_eq!(world.world().objects().count(), 10_001);
        assert_eq!(viewer_world_limits().max_entities_per_region, MAX_WORLD_ENTITIES);
    }

    #[tokio::test]
    async fn batch_yields_between_chunks() {
        let simulator_id = simulator_id();
        let mut session = SessionWorld::new(viewer_world_limits());
        let region = session.register_simulator(simulator_id, None).expect("register simulator");
        let before_revision = session.world().revision().get();
        let reader = session.reader();
        let (reply, response) = channel();
        reader.try_send(WorldReadRequest::Barrier(reply)).expect("queue barrier");
        let blocks = (1..=MAX_OBJECTS_PER_INTAKE_QUANTUM as u32 + 1)
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

        let worker = tokio::spawn(async move {
            let summary = session
                .apply_object_batch_cooperatively(simulator_id, ObjectBatch { region, region_handle: None, blocks, invalid_blocks: 0 })
                .await
                .expect("registered simulator");
            (session, summary)
        });
        let barrier = response.await.expect("World read response").expect("World read barrier");
        assert_eq!(barrier.object_count, MAX_OBJECTS_PER_INTAKE_QUANTUM);
        assert_eq!(barrier.revision.get(), before_revision + MAX_OBJECTS_PER_INTAKE_QUANTUM as u64);

        let (session, summary) = worker.await.expect("World worker");
        assert_eq!(session.world().objects().count(), MAX_OBJECTS_PER_INTAKE_QUANTUM + 1);
        assert_eq!(summary.blocks.len(), MAX_OBJECTS_PER_INTAKE_QUANTUM + 1);
        assert!(summary.blocks.iter().all(|block| block.result.as_ref().is_some_and(|result| result.is_ok())));
    }

    #[test]
    fn reader_lanes_bound_and_prioritize() {
        let mut world = SessionWorld::new(viewer_world_limits());
        let reader = world.reader();
        let inspector = reader.inspector();
        let (inspector_reply, inspector_response) = channel();
        inspector.try_send(WorldReadRequest::Barrier(inspector_reply)).unwrap();
        let (busy_inspector_reply, _busy_inspector_response) = channel();
        assert_eq!(inspector.try_send(WorldReadRequest::Barrier(busy_inspector_reply)), Err(WorldReaderError::Busy));

        let (renderer_reply, renderer_response) = channel();
        reader.try_send(WorldReadRequest::ObjectListPage { cursor: None, limit: 1, reply: renderer_reply }).unwrap();
        let (busy_renderer_reply, _busy_renderer_response) = channel();
        assert_eq!(reader.try_send(WorldReadRequest::Barrier(busy_renderer_reply)), Err(WorldReaderError::Busy));

        let mut requests = world.try_read_requests().into_iter();
        let request = requests.next().expect("renderer request first");
        assert!(matches!(&request, WorldReadRequest::ObjectListPage { .. }));
        world.service_read_request(request);
        let request = requests.next().expect("inspector request follows renderer");
        assert!(matches!(&request, WorldReadRequest::Barrier(_)));
        world.service_read_request(request);
        assert!(requests.next().is_none());

        let _page = block_on(renderer_response).unwrap().unwrap();
        let barrier = block_on(inspector_response).unwrap().unwrap();
        assert_eq!(barrier.revision, world.world().revision());
        assert_eq!(barrier.coverage.world_revision, barrier.revision);
        assert_eq!(barrier.coverage.health, CoverageHealth::NoKnownLoss);
        drop(world);

        assert_eq!(block_on(reader.barrier()), Err(WorldReaderError::Gone));
        assert_eq!(block_on(inspector.barrier()), Err(WorldReaderError::Gone));
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
    fn coverage_changes_keep_world_revision() {
        let coverage = SessionCoverageState::new();
        let mut session = SessionWorld::with_coverage(viewer_world_limits(), coverage.clone());
        let reader = session.reader();
        let mut changes = reader.subscribe_coverage();
        let before = read_barrier(&mut session, &reader);
        assert_eq!(before.coverage.health, CoverageHealth::NoKnownLoss);

        coverage.record_network_overflow(1);
        assert!(changes.has_changed().expect("coverage watch remains open"));
        assert_eq!(*changes.borrow_and_update(), 1);
        let after = read_barrier(&mut session, &reader);

        assert_eq!(after.revision, before.revision);
        assert_eq!(after.coverage.world_revision, after.revision);
        assert!(after.coverage.generation > before.coverage.generation);
        assert_eq!(after.coverage.health, CoverageHealth::Degraded);
    }

    #[test]
    fn loss_is_visible_before_dependent_world_update() {
        let simulator_id = simulator_id();
        let coverage = SessionCoverageState::new();
        let mut session = SessionWorld::with_coverage(viewer_world_limits(), coverage.clone());
        let region = session.register_simulator(simulator_id, None).expect("register simulator");
        let reader = session.reader();
        let loss = CoverageWatermark { simulator_id, source: CoverageSource::Lludp, generation: 1, overflow_generation: 0 };
        coverage.record_source_loss(loss, CoverageLossReason::DecodeRejected, false);
        let lost = read_barrier(&mut session, &reader);
        assert_eq!(lost.coverage.health, CoverageHealth::Degraded);
        assert_eq!(lost.coverage.losses[0].watermark, loss);

        apply_one(
            &mut session,
            simulator_id,
            region,
            ObjectOperation::Observe(observation(region, 1, 0x1001, NonZeroU32::new(1).expect("nonzero local ID"), ParentUpdate::Root)),
        );
        let applied = read_barrier(&mut session, &reader);
        assert!(applied.revision > lost.revision);
        assert_eq!(applied.coverage.health, CoverageHealth::Degraded);
        assert_eq!(applied.coverage.losses[0].watermark.generation, 1);
    }

    #[test]
    fn waiting_reads_prioritize_renderer() {
        let mut world = SessionWorld::new(viewer_world_limits());
        let reader = world.reader();
        let inspector = reader.inspector();
        let (inspector_reply, _inspector_response) = channel();
        inspector.try_send(WorldReadRequest::Barrier(inspector_reply)).unwrap();
        let (renderer_reply, _renderer_response) = channel();
        reader.try_send(WorldReadRequest::ObjectListPage { cursor: None, limit: 1, reply: renderer_reply }).unwrap();

        let mut requests = block_on(world.next_read_requests()).expect("queued read batch").into_iter();
        assert!(matches!(requests.next(), Some(WorldReadRequest::ObjectListPage { .. })));
        assert!(matches!(requests.next(), Some(WorldReadRequest::Barrier(_))));
        assert!(requests.next().is_none());
    }

    #[test]
    fn bootstrap_replays_changes() {
        let simulator_id = simulator_id();
        let mut session = SessionWorld::new(viewer_world_limits());
        let region = session.register_simulator(simulator_id, None).expect("register simulator");
        let blocks = (0..=MAX_OBJECT_PAGE_ITEMS)
            .map(|index| {
                let local_id = NonZeroU32::new(index as u32 + 1).expect("nonzero local ID");
                Some(ObjectOperation::Observe(observation(
                    region,
                    index as u32 + 1,
                    u128::from(1_000 + index as u32),
                    local_id,
                    ParentUpdate::Root,
                )))
            })
            .collect();
        session
            .apply_object_batch(simulator_id, ObjectBatch { region, region_handle: None, blocks, invalid_blocks: 0 })
            .expect("seed World");

        let selected_id = UUID::from_u128(1_001);
        let selected_entity = session.world().entity(selected_id).expect("selected object");
        let removed_id = UUID::from_u128(1_000);
        let removed_entity = session.world().entity(removed_id).expect("object to replace");
        let revision_hint = session.world().revision();
        let reader = session.reader().inspector();
        let capture = crate::devtools::capture_snapshot_for_test(&reader, Some((selected_id, selected_entity)), revision_hint);
        pin_mut!(capture);
        let mut changed_during_paging = false;

        let snapshot = block_on(poll_fn(|cx| {
            if let Poll::Ready(result) = capture.as_mut().poll(cx) {
                return Poll::Ready(result);
            }

            let requests = session.try_read_requests().into_iter().collect::<Vec<_>>();
            assert!(!requests.is_empty(), "pending capture should have a queued read");
            for request in requests {
                let change_now = !changed_during_paging && matches!(&request, WorldReadRequest::ObjectListPage { cursor: Some(_), .. });
                if change_now {
                    changed_during_paging = true;
                    apply_one(
                        &mut session,
                        simulator_id,
                        region,
                        ObjectOperation::Observe(observation(
                            region,
                            258,
                            5,
                            NonZeroU32::new(258).expect("nonzero local ID"),
                            ParentUpdate::Root,
                        )),
                    );
                    apply_one(
                        &mut session,
                        simulator_id,
                        region,
                        ObjectOperation::Observe(observation(
                            region,
                            259,
                            2_000,
                            NonZeroU32::new(259).expect("nonzero local ID"),
                            ParentUpdate::Root,
                        )),
                    );
                    apply_one(
                        &mut session,
                        simulator_id,
                        region,
                        ObjectOperation::Remove {
                            source: UpdateSource::new(region, 260, OrderEvidence::Ordered(260), 0),
                            local_id: NonZeroU32::new(1).expect("nonzero local ID"),
                        },
                    );
                    apply_one(
                        &mut session,
                        simulator_id,
                        region,
                        ObjectOperation::Observe(observation(
                            region,
                            261,
                            1_000,
                            NonZeroU32::new(300).expect("nonzero local ID"),
                            ParentUpdate::Root,
                        )),
                    );
                    let mut patch = ObjectPatch::new(
                        UpdateSource::new(region, 262, OrderEvidence::Ordered(262), 0),
                        NonZeroU32::new(2).expect("nonzero local ID"),
                    );
                    patch.parent = Some(ParentUpdate::Local(NonZeroU32::new(3).expect("nonzero parent local ID")));
                    patch.pose = Some(Pose { position: Some(Vec3::new(9.0, 8.0, 7.0)), rotation: None });
                    apply_one(&mut session, simulator_id, region, ObjectOperation::Patch(patch));
                    let handle = RegionHandle::new((512_u64 << 32) | 768);
                    session
                        .update_region_metadata(
                            region,
                            RegionInfo { handle: Some(handle), origin: Some(handle.origin()), ..RegionInfo::default() },
                        )
                        .expect("update region origin");
                    session.publish_revision();
                }
                session.service_read_request(request);
            }
            Poll::Pending
        }))
        .expect("capture and replay changes");

        let inserted_entity = session.world().entity(UUID::from_u128(5)).expect("concurrent insertion");
        let ahead_entity = session.world().entity(UUID::from_u128(2_000)).expect("ahead-of-cursor insertion");
        let replacement_entity = session.world().entity(removed_id).expect("replacement incarnation");
        let selected_parent = session.world().object(selected_entity).expect("selected object").parent();
        let Parent::Resolved(parent_entity) = selected_parent else { panic!("selected object has a resolved parent") };
        let parent_id = session.world().object(parent_entity).expect("parent object").identity().id();
        let expected_parent = parent_id.to_string();
        let expected_region = session.world().region(region).expect("current region metadata");
        assert!(changed_during_paging);
        assert_eq!(snapshot.object_count, MAX_OBJECT_PAGE_ITEMS + 3);
        assert_eq!(snapshot.objects.len(), snapshot.object_count);
        assert_eq!(snapshot.revision, Some(session.world().revision()));
        assert_eq!(snapshot.selected_position, Some(Vec3::new(9.0, 8.0, 7.0)));
        assert_eq!(snapshot.selected_parent, Some(expected_parent));
        assert_eq!(snapshot.regions.iter().find(|summary| summary.region == region).map(|summary| &summary.info), Some(expected_region));
        assert!(snapshot.objects.iter().any(|(id, entity, _, _)| *id == UUID::from_u128(5) && *entity == inserted_entity));
        assert!(snapshot.objects.iter().any(|(id, entity, _, _)| *id == UUID::from_u128(2_000) && *entity == ahead_entity));
        assert!(
            snapshot
                .objects
                .iter()
                .any(|(id, entity, _, local_id)| { *id == removed_id && *entity == replacement_entity && local_id.get() == 300 })
        );
        assert!(!snapshot.objects.iter().any(|(id, entity, _, _)| *id == removed_id && *entity == removed_entity));
    }

    #[test]
    fn stale_selection_is_cleared_on_session_change() {
        let old_simulator = simulator_id();
        let mut old_session = SessionWorld::new(viewer_world_limits());
        let old_region = old_session.register_simulator(old_simulator, None).expect("register old simulator");
        let old_id = UUID::from_u128(10);
        apply_one(
            &mut old_session,
            old_simulator,
            old_region,
            ObjectOperation::Observe(observation(old_region, 1, 10, NonZeroU32::new(1).expect("nonzero local ID"), ParentUpdate::Root)),
        );
        let old_reader = old_session.reader().inspector();
        let old_entity = old_session.world().entity(old_id).expect("old entity");

        let new_simulator = simulator_id();
        let mut new_session = SessionWorld::new(viewer_world_limits());
        let new_region = new_session.register_simulator(new_simulator, None).expect("register new simulator");
        let new_id = UUID::from_u128(20);
        apply_one(
            &mut new_session,
            new_simulator,
            new_region,
            ObjectOperation::Observe(observation(new_region, 1, 20, NonZeroU32::new(1).expect("nonzero local ID"), ParentUpdate::Root)),
        );
        let reader = new_session.reader().inspector();
        let revision_hint = new_session.world().revision();
        let capture =
            crate::devtools::capture_snapshot_after_session_change_for_test(&old_reader, &reader, (old_id, old_entity), revision_hint);
        pin_mut!(capture);

        let (snapshot, selection_invalidated) = block_on(poll_fn(|cx| {
            if let Poll::Ready(result) = capture.as_mut().poll(cx) {
                return Poll::Ready(result);
            }
            let requests = new_session.try_read_requests().into_iter().collect::<Vec<_>>();
            assert!(!requests.is_empty(), "pending capture should have a queued read");
            for request in requests {
                new_session.service_read_request(request);
            }
            Poll::Pending
        }));
        let snapshot = snapshot.expect("capture new session");

        assert!(selection_invalidated);
        assert_eq!(snapshot.selected_id, None);
        assert_eq!(snapshot.object_count, 1);
        assert!(snapshot.objects.iter().any(|(id, _, _, _)| *id == new_id));
    }

    #[test]
    fn busy_after_switch_clears_selection() {
        let old_simulator = simulator_id();
        let mut old_session = SessionWorld::new(viewer_world_limits());
        let old_region = old_session.register_simulator(old_simulator, None).expect("register old simulator");
        let old_id = UUID::from_u128(10);
        apply_one(
            &mut old_session,
            old_simulator,
            old_region,
            ObjectOperation::Observe(observation(old_region, 1, 10, NonZeroU32::new(1).expect("nonzero local ID"), ParentUpdate::Root)),
        );
        let old_reader = old_session.reader().inspector();
        let old_entity = old_session.world().entity(old_id).expect("old entity");

        let mut new_session = SessionWorld::new(viewer_world_limits());
        new_session.register_simulator(simulator_id(), None).expect("register new simulator");
        let reader = new_session.reader().inspector();
        let (reply, _response) = channel();
        reader.try_send(WorldReadRequest::Barrier(reply)).expect("fill inspector queue");

        let (result, selection_invalidated) = block_on(crate::devtools::capture_snapshot_after_session_change_for_test(
            &old_reader,
            &reader,
            (old_id, old_entity),
            new_session.world().revision(),
        ));

        assert!(matches!(result, Err(WorldReaderError::Busy)));
        assert!(selection_invalidated);
    }

    #[test]
    fn resync_after_switch_clears_selection() {
        let old_simulator = simulator_id();
        let mut old_session = SessionWorld::new(viewer_world_limits());
        let old_region = old_session.register_simulator(old_simulator, None).expect("register old simulator");
        let old_id = UUID::from_u128(10);
        apply_one(
            &mut old_session,
            old_simulator,
            old_region,
            ObjectOperation::Observe(observation(old_region, 1, 10, NonZeroU32::new(1).expect("nonzero local ID"), ParentUpdate::Root)),
        );
        let old_reader = old_session.reader().inspector();
        let old_entity = old_session.world().entity(old_id).expect("old entity");

        let mut limits = viewer_world_limits();
        limits.max_journal_entries = 1;
        let mut new_session = SessionWorld::new(limits);
        let new_simulator = simulator_id();
        let new_region = new_session.register_simulator(new_simulator, None).expect("register new simulator");
        let reader = new_session.reader().inspector();
        let revision_hint = new_session.world().revision();
        let capture =
            crate::devtools::capture_snapshot_after_session_change_for_test(&old_reader, &reader, (old_id, old_entity), revision_hint);
        pin_mut!(capture);
        let mut history_advanced = false;

        let (result, selection_invalidated) = block_on(poll_fn(|cx| {
            if let Poll::Ready(result) = capture.as_mut().poll(cx) {
                return Poll::Ready(result);
            }

            let requests = new_session.try_read_requests().into_iter().collect::<Vec<_>>();
            assert!(!requests.is_empty(), "pending capture should have a queued read");
            for request in requests {
                if !history_advanced && matches!(&request, WorldReadRequest::RegionPage { .. }) {
                    for (order, id) in [(2, 21), (3, 22)] {
                        apply_one(
                            &mut new_session,
                            new_simulator,
                            new_region,
                            ObjectOperation::Observe(observation(
                                new_region,
                                order,
                                id,
                                NonZeroU32::new(order).expect("nonzero local ID"),
                                ParentUpdate::Root,
                            )),
                        );
                    }
                    history_advanced = true;
                }
                new_session.service_read_request(request);
            }
            Poll::Pending
        }));

        assert!(history_advanced);
        assert!(matches!(result, Err(WorldReaderError::Read(WorldReadError::ResyncRequired))));
        assert!(selection_invalidated);
    }

    #[test]
    fn revision_watch_coalesces_world_changes() {
        let mut world = SessionWorld::new(viewer_world_limits());
        let reader = world.reader();
        assert!(reader.same_session(&world.reader()));
        assert!(reader.same_session(&reader.inspector()));
        let another = SessionWorld::new(viewer_world_limits()).reader();
        assert!(!reader.same_session(&another));
        let mut revisions = reader.subscribe_revisions();
        let simulator_id = simulator_id();
        let region = world.register_simulator(simulator_id, None).expect("register simulator");
        world.update_region_metadata(region, RegionInfo { name: Some("region".to_owned()), ..RegionInfo::default() }).unwrap();

        block_on(revisions.changed()).expect("revision notification");
        assert_eq!(*revisions.borrow_and_update(), world.world().revision());
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
        volume.shape = Some(Some(Shape::Prim));
        volume.appearance = Some(Some(Appearance::Surface));
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
        avatar.appearance = Some(Some(Appearance::Avatar));
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
        let message = AvatarAppearanceMessage { object_id: avatar_id, update };
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
                    AvatarAppearanceMessage { object_id: UUID::from_u128(0x811), update: update.clone() }
                )
                .is_none()
        );
        assert!(
            session
                .apply_avatar_appearance(
                    first_simulator,
                    source,
                    AvatarAppearanceMessage { object_id: UUID::from_u128(0x812), update: update.clone() }
                )
                .is_none()
        );
        assert!(
            session
                .apply_avatar_appearance(other_simulator, source, AvatarAppearanceMessage { object_id: UUID::from_u128(0x813), update })
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
        volume.shape = Some(Some(Shape::Prim));
        volume.appearance = Some(Some(Appearance::Surface));
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
        replacement.shape = Some(Some(Shape::Prim));
        replacement.appearance = Some(Some(Appearance::Surface));
        apply_one(&mut session, sim_id, region, ObjectOperation::Observe(replacement));

        let late = ObjectMediaUpdate::new(6, vec![Some(MediaEntry::default())]).expect("valid late response");
        assert!(session.apply_object_media(target, late).is_none());
        let state = session.world();
        assert_eq!(state.entity_at(LocalAddress::new(other_region, local_id)), None);
        let current = state.object(state.entity_at(LocalAddress::new(region, local_id)).unwrap()).unwrap();
        assert_eq!(current.object_media(), None);
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
