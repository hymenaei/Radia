use std::{
    error::Error,
    fmt,
    mem::size_of,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};

use futures::{FutureExt, channel::oneshot, pin_mut};
use tokio::runtime::Handle;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};
use world::{
    Entity, EnvironmentRead, MAX_ENVIRONMENT_READ_BYTES, MAX_OBJECT_DETAIL_BATCH_BYTES, MAX_OBJECT_DETAIL_BATCH_ITEMS,
    MAX_OBJECT_PAGE_ITEMS, MAX_REGION_PAGE_BYTES, MAX_REGION_PAGE_ITEMS, MAX_TERRAIN_PAGE_PATCHES, MAX_WORLD_CHANGE_PAGE_ITEMS,
    ObjectDetailBatch, ObjectListItem, ObjectListPage, ObjectPageCursor, Region, RegionPage, RegionPageCursor, RegionSummary,
    TERRAIN_PATCH_SAMPLES, TerrainPage, TerrainPageCursor, TerrainPatch, World, WorldChangeCursor, WorldChangePage, WorldChangeRecord,
    WorldReadBarrier, WorldReadError, WorldRevision,
};

const MAX_OUTSTANDING_READS: usize = 8;
const MAX_RESERVED_REPLY_BYTES: usize = MAX_ENVIRONMENT_READ_BYTES;
const READ_TIMEOUT: Duration = Duration::from_secs(1);
pub(crate) const MAX_INTAKE_TURNS: usize = 8;
const ACTIVE: u8 = 0;
const ENDED: u8 = 1;
const STOPPED: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    Busy,
    StaleSession,
    Shutdown,
    World(WorldReadError),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("World reads are busy; retry later"),
            Self::StaleSession => f.write_str("the World session ended"),
            Self::Shutdown => f.write_str("the World reader stopped"),
            Self::World(error) => error.fmt(f),
        }
    }
}

impl Error for ReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::World(error) => Some(error),
            _ => None,
        }
    }
}

struct ReadAdmission {
    requests: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
    state: AtomicU8,
}

impl ReadAdmission {
    fn check(&self) -> Result<(), ReadError> {
        match self.state.load(Ordering::Acquire) {
            ACTIVE => Ok(()),
            ENDED => Err(ReadError::StaleSession),
            _ => Err(ReadError::Shutdown),
        }
    }

    fn reserve(&self, bytes: usize) -> Result<ReadPermit, ReadError> {
        self.check()?;
        let requests = Arc::clone(&self.requests).try_acquire_owned().map_err(|_| ReadError::Busy)?;
        let bytes = u32::try_from(bytes).map_err(|_| ReadError::Busy)?;
        let bytes = Arc::clone(&self.bytes).try_acquire_many_owned(bytes).map_err(|_| ReadError::Busy)?;
        Ok(ReadPermit { _requests: requests, _bytes: bytes })
    }
}

struct ReadPermit {
    _requests: OwnedSemaphorePermit,
    _bytes: OwnedSemaphorePermit,
}

struct ReadResponse<T> {
    result: Result<T, ReadError>,
    _permit: ReadPermit,
}

pub(crate) struct ReadResponder<T> {
    sender: oneshot::Sender<ReadResponse<T>>,
    permit: ReadPermit,
}

impl<T> ReadResponder<T> {
    fn cancelled(&self) -> bool {
        self.sender.is_canceled()
    }

    fn send(self, result: Result<T, ReadError>) {
        if self.sender.send(ReadResponse { result, _permit: self.permit }).is_err() {
            tracing::trace!("world read receiver closed");
        }
    }
}

pub(crate) enum ReadRequest {
    Barrier(ReadResponder<WorldReadBarrier>),
    Objects(Option<ObjectPageCursor>, usize, ReadResponder<ObjectListPage>),
    Regions(Option<RegionPageCursor>, usize, ReadResponder<RegionPage>),
    Details(Vec<Entity>, ReadResponder<ObjectDetailBatch>),
    Changes(WorldChangeCursor, usize, ReadResponder<WorldChangePage>),
    Terrain(Region, Option<TerrainPageCursor>, usize, ReadResponder<Option<TerrainPage>>),
    Environment(Region, ReadResponder<Option<EnvironmentRead>>),
}

impl ReadRequest {
    pub(crate) fn serve(self, world: &World) {
        match self {
            Self::Barrier(reply) if !reply.cancelled() => reply.send(Ok(world.read_barrier())),
            Self::Objects(cursor, limit, reply) if !reply.cancelled() => {
                reply.send(world.object_list_page(cursor, limit).map_err(ReadError::World));
            }
            Self::Regions(cursor, limit, reply) if !reply.cancelled() => {
                reply.send(world.region_page(cursor, limit).map_err(ReadError::World));
            }
            Self::Details(entities, reply) if !reply.cancelled() => {
                reply.send(world.object_detail_batch(&entities).map_err(ReadError::World));
            }
            Self::Changes(cursor, limit, reply) if !reply.cancelled() => {
                reply.send(world.change_page(cursor, limit).map_err(ReadError::World));
            }
            Self::Terrain(region, cursor, limit, reply) if !reply.cancelled() => {
                reply.send(world.terrain_page(region, cursor, limit).map_err(ReadError::World));
            }
            Self::Environment(region, reply) if !reply.cancelled() => reply.send(Ok(world.environment(region))),
            _ => {}
        }
    }
}

#[derive(Clone)]
pub struct WorldReader {
    sender: mpsc::Sender<ReadRequest>,
    revisions: watch::Receiver<Result<WorldRevision, ReadError>>,
    budget: Arc<ReadAdmission>,
    runtime: Handle,
}

impl WorldReader {
    pub fn revision(&self) -> Result<WorldRevision, ReadError> {
        self.budget.check()?;
        *self.revisions.borrow()
    }

    pub async fn changed(&mut self) -> Result<WorldRevision, ReadError> {
        self.budget.check()?;
        self.revisions.changed().await.map_err(|_| ReadError::Shutdown)?;
        self.budget.check()?;
        *self.revisions.borrow_and_update()
    }

    pub async fn barrier(&self) -> Result<WorldReadBarrier, ReadError> {
        self.request(size_of::<WorldReadBarrier>(), ReadRequest::Barrier).await
    }

    pub async fn objects(&self, cursor: Option<ObjectPageCursor>, limit: usize) -> Result<ObjectListPage, ReadError> {
        check_limit(limit, MAX_OBJECT_PAGE_ITEMS)?;
        self.request(size_of::<ObjectListPage>() + limit * size_of::<ObjectListItem>(), |reply| ReadRequest::Objects(cursor, limit, reply))
            .await
    }

    pub async fn regions(&self, cursor: Option<RegionPageCursor>, limit: usize) -> Result<RegionPage, ReadError> {
        check_limit(limit, MAX_REGION_PAGE_ITEMS)?;
        self.request(MAX_REGION_PAGE_BYTES + size_of::<RegionPage>() + limit * size_of::<RegionSummary>(), |reply| {
            ReadRequest::Regions(cursor, limit, reply)
        })
        .await
    }

    pub async fn terrain(&self, region: Region, cursor: Option<TerrainPageCursor>, limit: usize) -> Result<Option<TerrainPage>, ReadError> {
        check_limit(limit, MAX_TERRAIN_PAGE_PATCHES)?;
        let bytes = size_of::<Option<TerrainPage>>() + limit * (size_of::<TerrainPatch>() + TERRAIN_PATCH_SAMPLES * size_of::<f32>());
        self.request(bytes, |reply| ReadRequest::Terrain(region, cursor, limit, reply)).await
    }

    pub async fn details(&self, entities: &[Entity]) -> Result<ObjectDetailBatch, ReadError> {
        check_limit(entities.len(), MAX_OBJECT_DETAIL_BATCH_ITEMS)?;
        self.request(MAX_OBJECT_DETAIL_BATCH_BYTES, |reply| ReadRequest::Details(entities.to_vec(), reply)).await
    }

    pub async fn environment(&self, region: Region) -> Result<Option<EnvironmentRead>, ReadError> {
        self.request(MAX_ENVIRONMENT_READ_BYTES, |reply| ReadRequest::Environment(region, reply)).await
    }

    pub async fn changes(&self, cursor: WorldChangeCursor, limit: usize) -> Result<WorldChangePage, ReadError> {
        check_limit(limit, MAX_WORLD_CHANGE_PAGE_ITEMS)?;
        self.request(size_of::<WorldChangePage>() + limit * size_of::<WorldChangeRecord>(), |reply| {
            ReadRequest::Changes(cursor, limit, reply)
        })
        .await
    }

    async fn request<T>(&self, bytes: usize, build: impl FnOnce(ReadResponder<T>) -> ReadRequest) -> Result<T, ReadError> {
        let permit = self.budget.reserve(bytes)?;
        let (sender, response) = oneshot::channel();
        self.sender.try_send(build(ReadResponder { sender, permit })).map_err(|error| {
            self.budget.check().err().unwrap_or(match error {
                TrySendError::Full(_) => ReadError::Busy,
                TrySendError::Closed(_) => ReadError::Shutdown,
            })
        })?;
        let response = response.fuse();
        let deadline = {
            let _entered = self.runtime.enter();
            tokio::time::sleep(READ_TIMEOUT)
        }
        .fuse();
        pin_mut!(response, deadline);
        let delivered = futures::select_biased! {
            reply = response => reply.map_err(|_| self.budget.check().err().unwrap_or(ReadError::Shutdown))?,
            _ = deadline => return Err(self.budget.check().err().unwrap_or(ReadError::Busy)),
        };
        self.budget.check()?;
        delivered.result
    }
}

pub(crate) struct WorldReadHost {
    receiver: mpsc::Receiver<ReadRequest>,
    revisions: watch::Sender<Result<WorldRevision, ReadError>>,
    reader: WorldReader,
    consecutive_intake: usize,
}

impl WorldReadHost {
    pub(crate) fn new(world: &World, runtime: Handle) -> Self {
        let (sender, receiver) = mpsc::channel(MAX_OUTSTANDING_READS);
        let (revisions, observed) = watch::channel(Ok(world.revision()));
        let budget = Arc::new(ReadAdmission {
            requests: Arc::new(Semaphore::new(MAX_OUTSTANDING_READS)),
            bytes: Arc::new(Semaphore::new(MAX_RESERVED_REPLY_BYTES)),
            state: AtomicU8::new(ACTIVE),
        });
        Self { receiver, revisions, reader: WorldReader { sender, revisions: observed, budget, runtime }, consecutive_intake: 0 }
    }

    pub(crate) fn reader(&self) -> WorldReader {
        self.reader.clone()
    }

    pub(crate) fn publish(&self, world: &World) {
        let revision = Ok(world.revision());
        self.revisions.send_if_modified(|current| {
            if *current == revision {
                false
            } else {
                *current = revision;
                true
            }
        });
    }

    fn reserved(&mut self) -> Option<ReadRequest> {
        if self.consecutive_intake < MAX_INTAKE_TURNS {
            return None;
        }
        let request = self.receiver.try_recv().ok()?;
        self.consecutive_intake = 0;
        Some(request)
    }

    async fn next(&mut self) -> ReadRequest {
        let request = self.receiver.recv().await.expect("the service retains a request sender");
        self.consecutive_intake = 0;
        request
    }

    pub(crate) async fn select<T>(&mut self, intake: impl Future<Output = T>) -> Result<T, ReadRequest> {
        if let Some(request) = self.reserved() {
            return Err(request);
        }
        let result = {
            let intake = intake.fuse();
            let request = self.next().fuse();
            pin_mut!(intake, request);
            futures::select_biased! {
                input = intake => Ok(input),
                request = request => Err(request),
            }
        };
        if result.is_ok() {
            self.consecutive_intake = (self.consecutive_intake + 1).min(MAX_INTAKE_TURNS);
        }
        result
    }

    pub(crate) fn finish(&mut self) {
        self.reader.budget.state.store(ENDED, Ordering::Release);
        let _ = self.revisions.send_replace(Err(ReadError::StaleSession));
        self.receiver.close();
        while self.receiver.try_recv().is_ok() {}
    }
}

impl Drop for WorldReadHost {
    fn drop(&mut self) {
        if self.reader.budget.state.compare_exchange(ACTIVE, STOPPED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            let _ = self.revisions.send_replace(Err(ReadError::Shutdown));
        }
    }
}

fn check_limit(limit: usize, maximum: usize) -> Result<(), ReadError> {
    if limit == 0 || limit > maximum { Err(ReadError::World(WorldReadError::InvalidLimit)) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use std::{future::pending, num::NonZeroU32};

    use futures::{join, poll};
    use uuid::Uuid;
    use world::{Kind, ObjectDetailResult, ObjectObservation, OrderEvidence, ParentUpdate, Region, UpdateSource};

    use super::*;
    use world::{EnvironmentAssignment, EnvironmentScope, EnvironmentSource, MAX_ENVIRONMENT_SOURCE_BYTES, ParcelLocalId, WorldLimits};

    fn world() -> World {
        World::new(WorldLimits {
            max_region_registrations: 8,
            max_entities: 16,
            max_entities_per_region: 16,
            max_tombstones_per_region: 16,
            max_tombstones: 16,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(16),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(16),
            max_journal_entries: 32,
            max_journal_changes: 128,
            max_region_name_bytes: 256,
            max_recovery_targets: 8,
        })
    }

    async fn deliver<T>(
        read: impl Future<Output = Result<T, ReadError>>,
        service: &mut WorldReadHost,
        world: &World,
    ) -> Result<T, ReadError> {
        join!(read, async {
            let Err(request) = service.select(pending::<()>()).await else { unreachable!() };
            request.serve(world);
        })
        .0
    }

    fn insert(world: &mut World, region: Region, id: u128, local: u32, order: u64) -> Entity {
        let observation = ObjectObservation::new(
            UpdateSource::new(region, order as u32, OrderEvidence::Ordered(order), 0),
            Uuid::from_u128(id),
            NonZeroU32::new(local).unwrap(),
            Kind::Object,
            ParentUpdate::Root,
        );
        let _ = world.observe_object(observation).unwrap();
        world.entity(Uuid::from_u128(id)).unwrap()
    }

    #[tokio::test]
    async fn environment_reads_fit_admission() {
        let mut world = world();
        let region = Region::new(1);
        let _ = world.adopt_region(region).unwrap();
        let source = UpdateSource::new(region, 1, OrderEvidence::Ordered(1), 0);
        let parcel = ParcelLocalId::new(42).unwrap();
        let _ = world.observe_agent_parcel(source, Some(parcel)).unwrap();
        for scope in [EnvironmentScope::Region, EnvironmentScope::Parcel(parcel)] {
            let target = world.environment_target(region, scope).unwrap();
            let assignment = EnvironmentAssignment {
                source: EnvironmentSource::Inline(vec![0; MAX_ENVIRONMENT_SOURCE_BYTES].into()),
                version: Some(1),
                day_length_seconds: None,
                day_offset_seconds: None,
                altitudes: None,
            };
            let _ = world.observe_environment(target, source, assignment).unwrap();
        }
        let mut service = WorldReadHost::new(&world, Handle::current());
        let reader = service.reader();
        let read = deliver(reader.environment(region), &mut service, &world).await.unwrap().unwrap();
        assert_eq!(read.region, region);
        assert_eq!(read.hydration_revision, world.revision());
        assert_eq!(read, world.environment(region).unwrap());
        assert!(read.retained_bytes() <= MAX_ENVIRONMENT_READ_BYTES);
        let _ = world.retire_region(region).unwrap();
        assert_eq!(deliver(reader.environment(region), &mut service, &world).await.unwrap(), None);
        assert!(
            matches!(read.region_assignment.unwrap().source, EnvironmentSource::Inline(source) if source.len() == MAX_ENVIRONMENT_SOURCE_BYTES)
        );
    }

    #[tokio::test]
    async fn pages_preserve_identity_and_cursors() {
        let mut world = world();
        let region = Region::new(1);
        let _ = world.adopt_region(region).unwrap();
        let old = insert(&mut world, region, 20, 1, 1);
        insert(&mut world, region, 30, 2, 2);
        let mut service = WorldReadHost::new(&world, Handle::current());
        let reader = service.reader();
        let barrier = deliver(reader.barrier(), &mut service, &world).await.unwrap();
        assert_eq!(barrier.object_count, 2);
        let first = deliver(reader.objects(None, 1), &mut service, &world).await.unwrap();
        assert_eq!(first.objects[0].id, Uuid::from_u128(20));
        insert(&mut world, region, 10, 3, 3);
        let next = deliver(reader.objects(first.next, 1), &mut service, &world).await.unwrap();
        assert_eq!(next.objects[0].id, Uuid::from_u128(30));
        assert_eq!(next.hydration_revision, world.revision());
        assert!(next.next.is_none());
        let source = UpdateSource::new(region, 4, OrderEvidence::Ordered(4), 0);
        let _ = world.remove_object(source, NonZeroU32::new(1).unwrap()).unwrap();
        let replacement = insert(&mut world, region, 20, 1, 5);
        assert_ne!(old, replacement);
        let details = deliver(reader.details(&[old, replacement]), &mut service, &world).await.unwrap();
        assert_eq!(details.hydration_revision, world.revision());
        assert!(matches!(&details.details[..], [ObjectDetailResult::Gone(entity), ObjectDetailResult::Found(detail)]
                if *entity == old && detail.summary.entity == replacement));
        let regions = deliver(reader.regions(None, 64), &mut service, &world).await.unwrap();
        assert_eq!(regions.regions.len(), 1);
        assert_eq!(regions.regions[0].region, region);
        assert_eq!(regions.hydration_revision, world.revision());
        let source = UpdateSource::new(region, 10, OrderEvidence::Ordered(10), 0);
        let patch = world::TerrainPatch::new(0, 0, Box::new([0.0; TERRAIN_PATCH_SAMPLES])).unwrap();
        let _ = world.observe_terrain_patch(source, patch).unwrap();
        let _ = world.observe_water_height(source, 0.0).unwrap();
        let terrain = deliver(reader.terrain(region, None, 8), &mut service, &world).await.unwrap().unwrap();
        assert_eq!(terrain, world.terrain_page(region, None, 8).unwrap().unwrap());
        assert_eq!(terrain.patches[0].heights(), &[0.0; TERRAIN_PATCH_SAMPLES]);
        assert_eq!(terrain.water_height, Some(0.0));
        let barrier = deliver(reader.barrier(), &mut service, &world).await.unwrap();
        let _ = world.retire_region(region).unwrap();
        assert_eq!(deliver(reader.terrain(region, None, 8), &mut service, &world).await, Ok(None));
        assert_eq!(terrain.patches[0].heights()[0], 0.0, "delivered terrain remains owned after retirement");
        let mut cursor = barrier.changes;
        let mut partial = false;
        loop {
            let page = deliver(reader.changes(cursor, 1), &mut service, &world).await.unwrap();
            assert_eq!(page, world.change_page(cursor, 1).unwrap(), "bridge must preserve the complete journal contract");
            partial |= page.has_more && page.completed_revision < page.observed_revision;
            cursor = page.cursor;
            if !page.has_more {
                assert_eq!(page.completed_revision, world.revision());
                break;
            }
        }
        assert!(partial, "region retirement must span a logical revision across pages");
        let other = self::world();
        assert_eq!(
            deliver(reader.changes(other.read_barrier().changes, 1), &mut service, &world).await,
            Err(ReadError::World(WorldReadError::StaleSession))
        );
        assert_eq!(reader.objects(None, 257).await, Err(ReadError::World(WorldReadError::InvalidLimit)));
    }

    #[tokio::test]
    async fn delivery_releases_admission() {
        let world = world();
        let mut service = WorldReadHost::new(&world, Handle::current());
        let reader = service.reader();
        let mut pending = (0..MAX_OUTSTANDING_READS).map(|_| Box::pin(reader.barrier())).collect::<Vec<_>>();
        for read in &mut pending {
            assert!(poll!(read.as_mut()).is_pending());
        }
        assert_eq!(reader.barrier().await, Err(ReadError::Busy));
        let request = service.next().await;
        request.serve(&world);
        assert_eq!(reader.barrier().await, Err(ReadError::Busy), "a queued reply still owns admission");
        let delivered = pending.remove(0).await.unwrap();
        let mut admitted = Box::pin(reader.barrier());
        assert!(poll!(admitted.as_mut()).is_pending(), "delivery releases admission even while the value is retained");
        assert_eq!(delivered.object_count, 0);
        drop(pending);
        drop(admitted);
        while let Ok(request) = service.receiver.try_recv() {
            request.serve(&world);
        }
        assert!(deliver(reader.barrier(), &mut service, &world).await.is_ok());
    }

    #[tokio::test]
    async fn reply_bytes_limit_admission() {
        let world = world();
        let mut service = WorldReadHost::new(&world, Handle::current());
        let reader = service.reader();
        let mut pending = Vec::new();
        for _ in 0..3 {
            let mut page = Box::pin(reader.regions(None, MAX_REGION_PAGE_ITEMS));
            assert!(poll!(page.as_mut()).is_pending());
            service.next().await.serve(&world);
            pending.push(page);
        }
        assert_eq!(reader.regions(None, MAX_REGION_PAGE_ITEMS).await, Err(ReadError::Busy));
        let _ = pending.remove(0).await.unwrap();
        assert!(deliver(reader.regions(None, MAX_REGION_PAGE_ITEMS), &mut service, &world).await.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn deadlines_and_retirement_reject_stale() {
        let world = world();
        let mut service = WorldReadHost::new(&world, Handle::current());
        let reader = service.reader();
        let mut timed_out = Box::pin(reader.barrier());
        assert!(poll!(timed_out.as_mut()).is_pending());
        tokio::time::advance(READ_TIMEOUT).await;
        assert_eq!(timed_out.await, Err(ReadError::Busy));
        service.next().await.serve(&world);
        let mut late = Box::pin(reader.barrier());
        assert!(poll!(late.as_mut()).is_pending());
        service.next().await.serve(&world);
        let mut queued = Box::pin(reader.barrier());
        assert!(poll!(queued.as_mut()).is_pending());
        let mut observed = reader.clone();
        let mut changed = Box::pin(observed.changed());
        assert!(poll!(changed.as_mut()).is_pending());
        service.finish();
        assert_eq!(late.await, Err(ReadError::StaleSession), "a completed reply cannot escape after session retirement");
        assert_eq!(changed.await, Err(ReadError::StaleSession));
        assert_eq!(queued.await, Err(ReadError::StaleSession));
        assert_eq!(reader.barrier().await, Err(ReadError::StaleSession));
        let service = WorldReadHost::new(&world, Handle::current());
        let reader = service.reader();
        let mut interrupted = Box::pin(reader.barrier());
        assert!(poll!(interrupted.as_mut()).is_pending());
        drop(service);
        assert_eq!(interrupted.await, Err(ReadError::Shutdown));
    }

    #[tokio::test]
    async fn revisions_coalesce_updates() {
        let mut world = world();
        let service = WorldReadHost::new(&world, Handle::current());
        let mut reader = service.reader();
        for i in 1..=3 {
            let _ = world.adopt_region(Region::new(i)).unwrap();
            service.publish(&world);
        }
        assert_eq!(reader.changed().await.unwrap(), world.revision());
        assert!(poll!(Box::pin(reader.changed()).as_mut()).is_pending());
    }
}
