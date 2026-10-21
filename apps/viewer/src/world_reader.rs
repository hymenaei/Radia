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

#[cfg(test)]
#[path = "world_reader_tests.rs"]
mod tests;

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
        let _ = self.sender.send(ReadResponse { result, _permit: self.permit });
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

fn check_limit(limit: usize, maximum: usize) -> Result<(), ReadError> {
    if limit == 0 || limit > maximum { Err(ReadError::World(WorldReadError::InvalidLimit)) } else { Ok(()) }
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
