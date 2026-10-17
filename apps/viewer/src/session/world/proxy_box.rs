use std::{cmp::Ordering, future::Future, mem::size_of, task::Poll};

use base::UUID;
use futures::{executor::block_on, future::poll_fn};
use math::{DVec3, Quat, Vec3};
use network::{SimulatorId, testing::simulator_id};
use world::{
    Entity, Kind, LocalAddress, MAX_OBJECT_DETAIL_BATCH_ITEMS, ObjectDetailBatch, ObjectDetailResult, ObjectListItem, ObjectObservation,
    ObjectPageCursor, ObjectPatch, OrderEvidence, ParentUpdate, Pose, PoseError, PoseState, Region, RegionInfo, UnresolvedReason,
    UpdateSource, WorldChange, WorldReadError, WorldTransformState,
};

use super::{SessionWorld, WorldReader, WorldReaderError, viewer_world_limits};
use crate::session::inbound::{ObjectBatch, ObjectOperation};

const OBJECT_PAGE_SIZE: usize = MAX_OBJECT_DETAIL_BATCH_ITEMS;
const CHANGE_PAGE_SIZE: usize = 16;
const FALLBACK_HALF_EXTENT: Vec3 = Vec3::splat(0.5);

#[derive(Clone, Copy, Debug, PartialEq)]
struct ProxyView {
    position: DVec3,
    forward: DVec3,
    radius: f64,
}

impl ProxyView {
    fn new(position: DVec3, forward: DVec3, radius: f64) -> Option<Self> {
        let length = forward.length();
        if !position.is_finite() || !forward.is_finite() || !length.is_finite() || length == 0.0 || !radius.is_finite() || radius < 0.0 {
            return None;
        }
        Some(Self { position, forward: forward / length, radius })
    }

    fn contains(self, geometry: ProxyGeometry) -> bool {
        let ProxyGeometry::Box { center, .. } = geometry else { return true };
        let offset = center - self.position;
        let distance = offset.length();
        distance.is_finite() && distance <= self.radius && offset.dot(self.forward) >= 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ProxyGeometry {
    Box { center: DVec3, half_extents: DVec3, quality: BoxQuality },
    Unresolved(ProxyIssue),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BoxQuality {
    Resolved,
    Provisional(UnresolvedReason),
    MissingScale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProxyIssue {
    Pose(UnresolvedReason),
    PoseError(PoseError),
    InvalidTransform,
    DetailsTooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ProxyRecord {
    entity: Entity,
    id: UUID,
    address: LocalAddress,
    geometry: ProxyGeometry,
}

impl ProxyRecord {
    fn from_detail(detail: &world::ObjectDetail) -> Self {
        let geometry = match detail.world_transform {
            Ok(WorldTransformState::Resolved(transform)) => box_at(transform.pose, transform.scale, BoxQuality::Resolved),
            Ok(WorldTransformState::MissingScale(pose)) => box_at(pose, FALLBACK_HALF_EXTENT * 2.0, BoxQuality::MissingScale),
            Ok(WorldTransformState::Unresolved(PoseState::Provisional { pose, reason })) => {
                box_at(pose, FALLBACK_HALF_EXTENT * 2.0, BoxQuality::Provisional(reason))
            }
            Ok(WorldTransformState::Unresolved(PoseState::Unresolved(reason))) => ProxyGeometry::Unresolved(ProxyIssue::Pose(reason)),
            Ok(WorldTransformState::Unresolved(PoseState::Resolved(_))) => ProxyGeometry::Unresolved(ProxyIssue::InvalidTransform),
            Err(error) => ProxyGeometry::Unresolved(ProxyIssue::PoseError(error)),
        };
        Self { entity: detail.summary.entity, id: detail.summary.id, address: detail.summary.address, geometry }
    }

    fn too_large(item: ObjectListItem) -> Self {
        Self { entity: item.entity, id: item.id, address: item.address, geometry: ProxyGeometry::Unresolved(ProxyIssue::DetailsTooLarge) }
    }
}

fn box_at(pose: world::WorldPose, scale: Vec3, quality: BoxQuality) -> ProxyGeometry {
    if !pose.position.is_finite() || !pose.rotation.is_finite() || !scale.is_finite() {
        return ProxyGeometry::Unresolved(ProxyIssue::InvalidTransform);
    }

    let half = scale.abs() * 0.5;
    let x = pose.rotation * (Vec3::X * half.x);
    let y = pose.rotation * (Vec3::Y * half.y);
    let z = pose.rotation * (Vec3::Z * half.z);
    let half_extents = DVec3::new(
        f64::from(x.x.abs() + y.x.abs() + z.x.abs()),
        f64::from(x.y.abs() + y.y.abs() + z.y.abs()),
        f64::from(x.z.abs() + y.z.abs() + z.z.abs()),
    );
    if !half_extents.is_finite() {
        return ProxyGeometry::Unresolved(ProxyIssue::InvalidTransform);
    }
    ProxyGeometry::Box { center: pose.position, half_extents, quality }
}

#[derive(Clone, Copy, Debug)]
struct ProxyLimits {
    soft_target_items: usize,
    hard_item_cap: usize,
    hard_record_bytes: usize,
    max_work_per_step: usize,
}

impl ProxyLimits {
    fn capacity(self) -> usize {
        self.soft_target_items.min(self.hard_item_cap).min(self.hard_record_bytes / size_of::<Option<ProxyRecord>>())
    }

    fn check(self) {
        assert!((1..=MAX_OBJECT_DETAIL_BATCH_ITEMS).contains(&self.max_work_per_step));
    }
}

struct ProxySet {
    slots: Box<[Option<ProxyRecord>]>,
    len: usize,
}

impl ProxySet {
    fn new(capacity: usize) -> Self {
        Self { slots: std::iter::repeat_n(None, capacity).collect(), len: 0 }
    }

    fn insert(&mut self, candidate: ProxyRecord, view: ProxyView) -> bool {
        let existing = self.index_of(candidate.entity).is_some();
        let previous_worst = self.len.checked_sub(1).and_then(|index| self.slots.get(index)).copied().flatten();
        if existing {
            self.remove(candidate.entity);
        }
        if !view.contains(candidate.geometry) || self.slots.is_empty() {
            return false;
        }
        if existing
            && self.len == self.slots.len() - 1
            && previous_worst.is_some_and(|worst| compare(candidate, worst, view) == Ordering::Greater)
        {
            return false;
        }
        if self.len < self.slots.len() {
            self.slots[self.len] = Some(candidate);
            self.len += 1;
        } else {
            let worst = self.slots[self.len - 1].expect("active proxy slots are populated");
            if compare(candidate, worst, view) != Ordering::Less {
                return false;
            }
            self.slots[self.len - 1] = Some(candidate);
        }
        self.sort(view);
        self.index_of(candidate.entity).is_some()
    }

    fn remove(&mut self, entity: Entity) -> bool {
        let Some(index) = self.index_of(entity) else { return false };
        for slot in index..self.len - 1 {
            self.slots[slot] = self.slots[slot + 1];
        }
        self.len -= 1;
        self.slots[self.len] = None;
        true
    }

    fn remove_region(&mut self, region: Region) -> bool {
        let initial_len = self.len;
        let mut kept = 0;
        for index in 0..initial_len {
            let record = self.slots[index].expect("active proxy slots are populated");
            if record.address.region() != region {
                self.slots[kept] = Some(record);
                kept += 1;
            }
        }
        self.slots[kept..initial_len].fill(None);
        self.len = kept;
        kept != initial_len
    }

    fn clear(&mut self) {
        self.slots[..self.len].fill(None);
        self.len = 0;
    }

    fn index_of(&self, entity: Entity) -> Option<usize> {
        self.slots[..self.len].iter().position(|record| record.is_some_and(|record| record.entity == entity))
    }

    fn sort(&mut self, view: ProxyView) {
        self.slots[..self.len].sort_unstable_by(|left, right| {
            compare(left.expect("active proxy slots are populated"), right.expect("active proxy slots are populated"), view)
        });
    }

    fn iter(&self) -> impl Iterator<Item = &ProxyRecord> {
        self.slots[..self.len].iter().map(|record| record.as_ref().expect("active proxy slots are populated"))
    }

    fn len(&self) -> usize {
        self.len
    }

    fn storage_bytes(&self) -> usize {
        self.slots.len() * size_of::<Option<ProxyRecord>>()
    }
}

fn compare(left: ProxyRecord, right: ProxyRecord, view: ProxyView) -> Ordering {
    let left_distance = distance(left.geometry, view);
    let right_distance = distance(right.geometry, view);
    left_distance.total_cmp(&right_distance).then_with(|| left.id.cmp(&right.id))
}

fn distance(geometry: ProxyGeometry, view: ProxyView) -> f64 {
    match geometry {
        ProxyGeometry::Box { center, .. } => center.distance(view.position),
        ProxyGeometry::Unresolved(_) => f64::INFINITY,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Snapshot,
    Scan(Option<ObjectPageCursor>),
    Reconcile,
    Live,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Progress {
    Began,
    Scanned(usize),
    Restarted,
    Ready,
    Idle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RefreshResult {
    Current,
    Refill,
    Restart,
}

enum DetailAction {
    Replace(ProxyRecord),
    Remove,
    TooLarge,
}

struct ProxyBoxConsumer {
    reader: WorldReader,
    limits: ProxyLimits,
    view: ProxyView,
    records: ProxySet,
    phase: Phase,
    change_cursor: Option<world::WorldChangeCursor>,
    restarts: usize,
}

impl ProxyBoxConsumer {
    fn new(reader: WorldReader, view: ProxyView, limits: ProxyLimits) -> Self {
        limits.check();
        Self { reader, limits, view, records: ProxySet::new(limits.capacity()), phase: Phase::Snapshot, change_cursor: None, restarts: 0 }
    }

    fn set_view(&mut self, view: ProxyView) {
        self.view = view;
        self.restart();
    }

    fn replace_reader(&mut self, reader: WorldReader) {
        if !self.reader.same_session(&reader) {
            self.reader = reader;
            self.restart();
        }
    }

    fn restart(&mut self) {
        self.records.clear();
        self.phase = Phase::Snapshot;
        self.change_cursor = None;
        self.restarts += 1;
    }

    fn is_live(&self) -> bool {
        self.phase == Phase::Live
    }

    fn records(&self) -> impl Iterator<Item = &ProxyRecord> {
        self.records.iter()
    }

    async fn advance(&mut self) -> Result<Progress, WorldReaderError> {
        match self.phase {
            Phase::Snapshot => {
                let barrier = self.reader.barrier().await?;
                self.change_cursor = Some(barrier.changes);
                self.phase = Phase::Scan(None);
                Ok(Progress::Began)
            }
            Phase::Scan(cursor) => self.scan(cursor).await,
            Phase::Reconcile | Phase::Live => self.read_changes().await,
        }
    }

    async fn scan(&mut self, cursor: Option<ObjectPageCursor>) -> Result<Progress, WorldReaderError> {
        let limit = self.limits.max_work_per_step.min(OBJECT_PAGE_SIZE);
        let page = match self.reader.object_list_page(cursor, limit).await {
            Ok(page) => page,
            Err(WorldReaderError::Read(WorldReadError::StaleSession | WorldReadError::ResyncRequired | WorldReadError::InvalidCursor)) => {
                self.restart();
                return Ok(Progress::Restarted);
            }
            Err(error) => return Err(error),
        };
        let scanned = page.objects.len();
        let next = page.next;
        let items: Vec<_> = page.objects.into_iter().filter(|item| item.kind == Kind::Object).collect();
        if !items.is_empty() {
            let entities: Vec<_> = items.iter().map(|item| item.entity).collect();
            let source = self.reader.clone();
            let batch = match source.object_details(&entities).await {
                Ok(batch) => batch,
                Err(WorldReaderError::Read(WorldReadError::StaleSession | WorldReadError::InvalidCursor)) => {
                    self.restart();
                    return Ok(Progress::Restarted);
                }
                Err(error) => return Err(error),
            };
            if !self.accept_details_from(&source, &items, batch) {
                self.restart();
                return Ok(Progress::Restarted);
            }
        }

        match next {
            None => self.phase = Phase::Reconcile,
            Some(next) => self.phase = Phase::Scan(Some(next)),
        }
        Ok(Progress::Scanned(scanned))
    }

    fn accept_details_from(&mut self, source: &WorldReader, items: &[ObjectListItem], batch: ObjectDetailBatch) -> bool {
        if !self.reader.same_session(source) || batch.details.len() != items.len() {
            return false;
        }

        for (item, result) in items.iter().copied().zip(batch.details) {
            match detail_action(item.entity, result) {
                Some(DetailAction::Replace(record)) => {
                    self.upsert_record(record);
                }
                Some(DetailAction::Remove) => {
                    self.remove_record(item.entity);
                }
                Some(DetailAction::TooLarge) => {
                    self.upsert_record(ProxyRecord::too_large(item));
                }
                None => return false,
            }
        }
        true
    }

    fn upsert_record(&mut self, record: ProxyRecord) -> bool {
        let was_selected = self.records.index_of(record.entity).is_some();
        let was_full = !self.records.slots.is_empty() && self.records.len() == self.records.slots.len();
        let retained = self.records.insert(record, self.view);
        was_selected && was_full && !retained
    }

    fn remove_record(&mut self, entity: Entity) -> bool {
        let was_full = !self.records.slots.is_empty() && self.records.len() == self.records.slots.len();
        was_full && self.records.remove(entity)
    }

    async fn read_changes(&mut self) -> Result<Progress, WorldReaderError> {
        let cursor = self.change_cursor.expect("a snapshot starts with a change cursor");
        let page = match self.reader.changes(cursor, CHANGE_PAGE_SIZE).await {
            Ok(page) => page,
            Err(WorldReaderError::Read(WorldReadError::ResyncRequired | WorldReadError::StaleSession | WorldReadError::InvalidCursor)) => {
                self.restart();
                return Ok(Progress::Restarted);
            }
            Err(error) => return Err(error),
        };

        let was_reconciling = self.phase == Phase::Reconcile;
        let mut changed_entities = Vec::new();
        let mut refill = false;
        for record in page.records {
            match record.change {
                WorldChange::EntityRemoved { entity, .. } => {
                    let was_full = !self.records.slots.is_empty() && self.records.len() == self.records.slots.len();
                    let removed = self.records.remove(entity);
                    refill |= was_full && removed;
                }
                WorldChange::RegionRetired { region, .. } => {
                    let was_full = !self.records.slots.is_empty() && self.records.len() == self.records.slots.len();
                    let removed = self.records.remove_region(region);
                    refill |= was_full && removed;
                }
                WorldChange::EntityAdded { entity, .. }
                | WorldChange::EntityUpdated { entity, .. }
                | WorldChange::SubtreeInvalidated { entity, .. }
                | WorldChange::EntityRebound { entity, .. }
                | WorldChange::EntityTransferred { entity, .. } => {
                    if !changed_entities.contains(&entity) {
                        changed_entities.push(entity);
                    }
                }
                WorldChange::RegionAdopted(_) | WorldChange::RegionUpdated(_) => {
                    self.restart();
                    return Ok(Progress::Restarted);
                }
            }
        }
        self.change_cursor = Some(page.cursor);
        match self.refresh_entities(&changed_entities).await? {
            RefreshResult::Current => {}
            RefreshResult::Refill => refill = true,
            RefreshResult::Restart => {
                self.restart();
                return Ok(Progress::Restarted);
            }
        }
        if refill {
            self.phase = Phase::Scan(None);
            return Ok(Progress::Scanned(0));
        }
        if page.has_more {
            return Ok(Progress::Scanned(0));
        }
        if was_reconciling {
            self.phase = Phase::Live;
            Ok(Progress::Ready)
        } else {
            Ok(Progress::Idle)
        }
    }

    async fn refresh_entities(&mut self, entities: &[Entity]) -> Result<RefreshResult, WorldReaderError> {
        if entities.is_empty() {
            return Ok(RefreshResult::Current);
        }
        let source = self.reader.clone();
        let batch = match source.object_details(entities).await {
            Ok(batch) => batch,
            Err(WorldReaderError::Read(WorldReadError::StaleSession | WorldReadError::InvalidCursor)) => return Ok(RefreshResult::Restart),
            Err(error) => return Err(error),
        };
        if !self.reader.same_session(&source) || batch.details.len() != entities.len() {
            return Ok(RefreshResult::Restart);
        }

        let mut refill = false;
        for (entity, result) in entities.iter().copied().zip(batch.details) {
            match detail_action(entity, result) {
                Some(DetailAction::Replace(record)) => refill |= self.upsert_record(record),
                Some(DetailAction::Remove) => refill |= self.remove_record(entity),
                Some(DetailAction::TooLarge) => match source.object_list_item(entity).await {
                    Ok(item) if item.item.kind == Kind::Object => {
                        refill |= self.upsert_record(ProxyRecord::too_large(item.item));
                    }
                    Ok(_) | Err(WorldReaderError::Read(WorldReadError::StaleIdentity)) => {
                        refill |= self.remove_record(entity);
                    }
                    Err(WorldReaderError::Read(WorldReadError::StaleSession)) => return Ok(RefreshResult::Restart),
                    Err(error) => return Err(error),
                },
                None => return Ok(RefreshResult::Restart),
            }
        }
        Ok(if refill { RefreshResult::Refill } else { RefreshResult::Current })
    }
}

fn detail_action(entity: Entity, result: ObjectDetailResult) -> Option<DetailAction> {
    match result {
        ObjectDetailResult::Found(detail) if detail.summary.entity == entity && detail.summary.kind == Kind::Object => {
            Some(DetailAction::Replace(ProxyRecord::from_detail(&detail)))
        }
        ObjectDetailResult::Found(detail) if detail.summary.entity == entity => Some(DetailAction::Remove),
        ObjectDetailResult::Gone(gone) if gone == entity => Some(DetailAction::Remove),
        ObjectDetailResult::TooLarge(large) if large == entity => Some(DetailAction::TooLarge),
        _ => None,
    }
}

fn drive<T>(session: &mut SessionWorld, future: impl Future<Output = T>) -> T {
    let mut future = Box::pin(future);
    block_on(poll_fn(|cx| match future.as_mut().poll(cx) {
        Poll::Ready(result) => Poll::Ready(result),
        Poll::Pending => {
            for request in session.try_read_requests().into_iter() {
                session.service_read_request(request);
            }
            cx.waker().wake_by_ref();
            future.as_mut().poll(cx)
        }
    }))
}

fn limits(soft_target_items: usize, hard_item_cap: usize, hard_record_bytes: usize, max_work_per_step: usize) -> ProxyLimits {
    ProxyLimits { soft_target_items, hard_item_cap, hard_record_bytes, max_work_per_step }
}

fn default_limits() -> ProxyLimits {
    limits(16, 64, size_of::<Option<ProxyRecord>>() * 64, OBJECT_PAGE_SIZE)
}

fn test_view(position: DVec3, forward: DVec3, radius: f64) -> ProxyView {
    ProxyView::new(position, forward, radius).expect("valid proxy view")
}

pub(crate) async fn capture_proxy_objects_for_test(reader: WorldReader) -> Result<Vec<(UUID, LocalAddress)>, WorldReaderError> {
    let view = test_view(DVec3::new(-1.0e12, 0.0, 0.0), DVec3::X, 2.0e12);
    let mut consumer = ProxyBoxConsumer::new(reader, view, default_limits());
    for _ in 0..4096 {
        consumer.advance().await?;
        if consumer.is_live() {
            return Ok(consumer.records().map(|record| (record.id, record.address)).collect());
        }
    }
    panic!("proxy consumer did not converge on a stable snapshot");
}

fn settle(consumer: &mut ProxyBoxConsumer, session: &mut SessionWorld) {
    for _ in 0..4096 {
        let _ = drive(session, consumer.advance()).expect("World reader remains available");
        if consumer.is_live() {
            return;
        }
    }
    panic!("proxy consumer did not converge");
}

fn read<T>(session: &mut SessionWorld, future: impl Future<Output = T>) -> T {
    drive(session, future)
}

fn observe(
    session: &mut SessionWorld,
    simulator: SimulatorId,
    region: Region,
    order: u32,
    id: u128,
    local_id: u32,
    position: Vec3,
    scale: Option<Vec3>,
    parent: ParentUpdate,
    kind: Kind,
) -> Entity {
    let object = ObjectObservation {
        source: source(region, order),
        id: UUID::from_u128(id),
        local_id: nz(local_id),
        kind,
        parent,
        pose: Some(Pose { position: Some(position), rotation: Some(Quat::IDENTITY) }),
        linear_motion: None,
        angular_velocity: None,
        scale,
        attachment_point: None,
        shape: None,
        appearance: None,
        surface_appearance: None,
        render_materials: None,
    };
    let _ = session
        .apply_object_batch(
            simulator,
            ObjectBatch {
                region,
                region_handle: None,
                blocks: vec![Some(ObjectOperation::Observe(object))],
                invalid_blocks: 0,
                invalid_fields: 0,
            },
        )
        .expect("registered simulator");
    session.world().entity(UUID::from_u128(id)).expect("observation creates object")
}

fn patch_parent(
    session: &mut SessionWorld,
    simulator: SimulatorId,
    region: Region,
    order: u32,
    local_id: u32,
    parent: ParentUpdate,
    position: Vec3,
) {
    let mut patch = ObjectPatch::new(source(region, order), nz(local_id));
    patch.parent = Some(parent);
    patch.pose = Some(Pose { position: Some(position), rotation: Some(Quat::IDENTITY) });
    let _ = session
        .apply_object_batch(
            simulator,
            ObjectBatch {
                region,
                region_handle: None,
                blocks: vec![Some(ObjectOperation::Patch(patch))],
                invalid_blocks: 0,
                invalid_fields: 0,
            },
        )
        .expect("registered simulator");
}

fn patch_position(session: &mut SessionWorld, simulator: SimulatorId, region: Region, order: u32, local_id: u32, position: Vec3) {
    let mut patch = ObjectPatch::new(source(region, order), nz(local_id));
    patch.pose = Some(Pose { position: Some(position), rotation: None });
    let _ = session
        .apply_object_batch(
            simulator,
            ObjectBatch {
                region,
                region_handle: None,
                blocks: vec![Some(ObjectOperation::Patch(patch))],
                invalid_blocks: 0,
                invalid_fields: 0,
            },
        )
        .expect("registered simulator");
}

fn remove(session: &mut SessionWorld, simulator: SimulatorId, region: Region, order: u32, local_id: u32) {
    let _ = session
        .apply_object_batch(
            simulator,
            ObjectBatch {
                region,
                region_handle: None,
                blocks: vec![Some(ObjectOperation::Remove { source: source(region, order), local_id: nz(local_id) })],
                invalid_blocks: 0,
                invalid_fields: 0,
            },
        )
        .expect("registered simulator");
}

fn source(region: Region, order: u32) -> UpdateSource {
    UpdateSource::new(region, order, OrderEvidence::Ordered(u64::from(order)), 0)
}

fn nz(value: u32) -> std::num::NonZeroU32 {
    std::num::NonZeroU32::new(value).expect("test local IDs are nonzero")
}

fn new_session() -> (SessionWorld, SimulatorId, Region) {
    let simulator = simulator_id();
    let mut session = SessionWorld::new(viewer_world_limits());
    let region = session.register_simulator(simulator, None).expect("register simulator");
    (session, simulator, region)
}

fn set_origin(session: &mut SessionWorld, region: Region, origin: DVec3) {
    session.update_region_metadata(region, RegionInfo { origin: Some(origin), ..RegionInfo::default() }).expect("valid region origin");
}

fn ids(consumer: &ProxyBoxConsumer) -> Vec<u128> {
    consumer.records().map(|record| record.id.as_u128()).collect()
}

#[test]
fn selects_visible_objects() {
    let (mut session, simulator, region) = new_session();
    set_origin(&mut session, region, DVec3::ZERO);
    observe(&mut session, simulator, region, 1, 10, 1, Vec3::new(5.0, 0.0, 0.0), Some(Vec3::splat(2.0)), ParentUpdate::Root, Kind::Object);
    observe(&mut session, simulator, region, 2, 20, 2, Vec3::new(6.0, 0.0, 0.0), None, ParentUpdate::Root, Kind::Object);
    observe(&mut session, simulator, region, 3, 30, 3, Vec3::new(-2.0, 0.0, 0.0), Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    observe(&mut session, simulator, region, 4, 40, 4, Vec3::new(0.0, 8.0, 0.0), Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    observe(&mut session, simulator, region, 5, 50, 5, Vec3::new(100.0, 0.0, 0.0), Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    observe(&mut session, simulator, region, 6, 60, 6, Vec3::new(1.0, 0.0, 0.0), Some(Vec3::ONE), ParentUpdate::Root, Kind::Avatar);

    let mut consumer = ProxyBoxConsumer::new(
        session.reader(),
        test_view(DVec3::ZERO, DVec3::X, 20.0),
        limits(8, 8, size_of::<Option<ProxyRecord>>() * 8, 2),
    );
    settle(&mut consumer, &mut session);
    assert_eq!(ids(&consumer), [10, 20, 40]);
    let first = consumer.records().find(|record| record.id == UUID::from_u128(10)).unwrap();
    assert_eq!(
        first.geometry,
        ProxyGeometry::Box { center: DVec3::new(5.0, 0.0, 0.0), half_extents: DVec3::ONE, quality: BoxQuality::Resolved }
    );
    let missing_scale = consumer.records().find(|record| record.id == UUID::from_u128(20)).unwrap();
    assert_eq!(
        missing_scale.geometry,
        ProxyGeometry::Box { center: DVec3::new(6.0, 0.0, 0.0), half_extents: DVec3::splat(0.5), quality: BoxQuality::MissingScale }
    );

    consumer.set_view(test_view(DVec3::ZERO, -DVec3::X, 20.0));
    settle(&mut consumer, &mut session);
    assert_eq!(ids(&consumer), [30, 40]);

    consumer.set_view(test_view(DVec3::new(2.0, 0.0, 0.0), DVec3::X, 20.0));
    settle(&mut consumer, &mut session);
    assert_eq!(ids(&consumer), [10, 20]);

    consumer.set_view(test_view(DVec3::new(100.0, 0.0, 0.0), DVec3::X, 10.0));
    settle(&mut consumer, &mut session);
    assert_eq!(ids(&consumer), [50]);
}

#[test]
fn caps_work_and_bytes() {
    let (mut session, simulator, region) = new_session();
    set_origin(&mut session, region, DVec3::ZERO);
    for index in 0..6 {
        observe(
            &mut session,
            simulator,
            region,
            index + 1,
            100 + u128::from(index),
            index + 1,
            Vec3::new((index + 1) as f32, 0.0, 0.0),
            Some(Vec3::ONE),
            ParentUpdate::Root,
            Kind::Object,
        );
    }

    let byte_cap = size_of::<Option<ProxyRecord>>() * 2;
    let byte_limits = limits(5, 5, byte_cap, 2);
    let mut consumer = ProxyBoxConsumer::new(session.reader(), test_view(DVec3::ZERO, DVec3::X, 20.0), byte_limits);
    let mut max_scanned = 0;
    for _ in 0..4096 {
        let progress = drive(&mut session, consumer.advance()).expect("World reader remains available");
        if let Progress::Scanned(scanned) = progress {
            max_scanned = max_scanned.max(scanned);
        }
        assert!(consumer.records.len() <= byte_limits.capacity());
        assert!(consumer.records.storage_bytes() <= byte_cap);
        if consumer.is_live() {
            break;
        }
    }
    assert!(consumer.is_live());
    assert_eq!(max_scanned, 2);
    assert_eq!(ids(&consumer), [100, 101]);

    let record_bytes = size_of::<Option<ProxyRecord>>();
    let soft_limits = limits(1, 6, record_bytes * 6, 6);
    assert_eq!(soft_limits.capacity(), 1);
    let mut soft_limited = ProxyBoxConsumer::new(session.reader(), test_view(DVec3::ZERO, DVec3::X, 20.0), soft_limits);
    settle(&mut soft_limited, &mut session);
    assert_eq!(ids(&soft_limited), [100]);

    let hard_limits = limits(6, 1, record_bytes * 6, 6);
    assert_eq!(hard_limits.capacity(), 1);
    let mut hard_limited = ProxyBoxConsumer::new(session.reader(), test_view(DVec3::ZERO, DVec3::X, 20.0), hard_limits);
    settle(&mut hard_limited, &mut session);
    assert_eq!(ids(&hard_limited), [100]);
}

#[test]
fn breaks_distance_ties_by_id() {
    let (mut session, simulator, region) = new_session();
    set_origin(&mut session, region, DVec3::ZERO);
    let higher =
        observe(&mut session, simulator, region, 1, 300, 1, Vec3::new(3.0, 0.0, 0.0), Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    let middle =
        observe(&mut session, simulator, region, 2, 200, 2, Vec3::new(3.0, 0.0, 0.0), Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    let lower =
        observe(&mut session, simulator, region, 3, 100, 3, Vec3::new(3.0, 0.0, 0.0), Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    let reader = session.reader();
    let view = test_view(DVec3::ZERO, DVec3::X, 20.0);
    let capacity = size_of::<Option<ProxyRecord>>() * 2;

    for order in [[higher, middle, lower], [lower, higher, middle]] {
        let items: Vec<_> = order.iter().map(|entity| session.world().object_list_item(*entity).unwrap().item).collect();
        let batch = read(&mut session, reader.object_details(&order)).unwrap();
        let mut consumer = ProxyBoxConsumer::new(reader.clone(), view, limits(2, 2, capacity, 3));

        assert!(consumer.accept_details_from(&reader, &items, batch));
        assert_eq!(ids(&consumer), [100, 200]);
    }
}

#[test]
fn resolves_late_origins_and_parents() {
    let (mut session, simulator, region) = new_session();
    let root =
        observe(&mut session, simulator, region, 1, 200, 3, Vec3::new(3.0, 0.0, 0.0), Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    let child = observe(&mut session, simulator, region, 2, 201, 2, Vec3::X, Some(Vec3::ONE), ParentUpdate::Local(nz(1)), Kind::Object);

    let mut consumer = ProxyBoxConsumer::new(session.reader(), test_view(DVec3::ZERO, DVec3::X, 200.0), default_limits());
    settle(&mut consumer, &mut session);
    let child_record = consumer.records().find(|record| record.entity == child).unwrap();
    assert_eq!(child_record.geometry, ProxyGeometry::Unresolved(ProxyIssue::Pose(UnresolvedReason::PendingParent)));
    let root_record = consumer.records().find(|record| record.entity == root).unwrap();
    assert_eq!(root_record.geometry, ProxyGeometry::Unresolved(ProxyIssue::Pose(UnresolvedReason::MissingRegionOrigin)));

    set_origin(&mut session, region, DVec3::new(100.0, 0.0, 0.0));
    settle(&mut consumer, &mut session);
    let root_record = consumer.records().find(|record| record.entity == root).unwrap();
    assert!(matches!(root_record.geometry, ProxyGeometry::Box { center, .. } if center == DVec3::new(103.0, 0.0, 0.0)));
    assert!(matches!(
        consumer.records().find(|record| record.entity == child).unwrap().geometry,
        ProxyGeometry::Unresolved(ProxyIssue::Pose(UnresolvedReason::PendingParent))
    ));

    observe(&mut session, simulator, region, 3, 202, 1, Vec3::new(3.0, 0.0, 0.0), Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    settle(&mut consumer, &mut session);
    let child_record = consumer.records().find(|record| record.entity == child).unwrap();
    assert!(matches!(child_record.geometry, ProxyGeometry::Box { center, .. } if center == DVec3::new(104.0, 0.0, 0.0)));
}

#[test]
fn refreshes_reparented_boxes() {
    let (mut session, simulator, region) = new_session();
    set_origin(&mut session, region, DVec3::ZERO);
    observe(&mut session, simulator, region, 1, 300, 1, Vec3::new(10.0, 0.0, 0.0), Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    observe(&mut session, simulator, region, 2, 301, 2, Vec3::new(30.0, 0.0, 0.0), Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    let child = observe(&mut session, simulator, region, 3, 302, 3, Vec3::X, Some(Vec3::ONE), ParentUpdate::Local(nz(1)), Kind::Object);

    let mut consumer = ProxyBoxConsumer::new(session.reader(), test_view(DVec3::ZERO, DVec3::X, 100.0), default_limits());
    settle(&mut consumer, &mut session);
    let record = consumer.records().find(|record| record.entity == child).unwrap();
    assert!(matches!(record.geometry, ProxyGeometry::Box { center, .. } if center == DVec3::new(11.0, 0.0, 0.0)));

    patch_parent(&mut session, simulator, region, 4, 3, ParentUpdate::Local(nz(2)), Vec3::new(2.0, 0.0, 0.0));
    settle(&mut consumer, &mut session);
    let record = consumer.records().find(|record| record.entity == child).unwrap();
    assert!(matches!(record.geometry, ProxyGeometry::Box { center, .. } if center == DVec3::new(32.0, 0.0, 0.0)), "{:?}", record.geometry);
}

#[test]
fn reconciles_live_mutations_without_restarting() {
    let (mut session, simulator, region) = new_session();
    set_origin(&mut session, region, DVec3::ZERO);
    for id in 1..=20 {
        observe(
            &mut session,
            simulator,
            region,
            id as u32,
            id,
            id as u32,
            Vec3::new(id as f32, 0.0, 0.0),
            Some(Vec3::ONE),
            ParentUpdate::Root,
            Kind::Object,
        );
    }

    let capacity = 20;
    let mut consumer = ProxyBoxConsumer::new(
        session.reader(),
        test_view(DVec3::ZERO, DVec3::X, 1_000.0),
        limits(capacity, capacity, size_of::<Option<ProxyRecord>>() * capacity, 2),
    );
    assert_eq!(drive(&mut session, consumer.advance()).unwrap(), Progress::Began);
    assert_eq!(drive(&mut session, consumer.advance()).unwrap(), Progress::Scanned(2));

    patch_position(&mut session, simulator, region, 30, 1, Vec3::new(50.0, 0.0, 0.0));
    observe(&mut session, simulator, region, 31, 21, 21, Vec3::splat(0.5), Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    settle(&mut consumer, &mut session);
    assert_eq!(consumer.restarts, 0);
    assert_eq!(consumer.records.len(), capacity);
    assert!(!ids(&consumer).contains(&1));
    assert!(ids(&consumer).contains(&21));

    patch_position(&mut session, simulator, region, 32, 2, Vec3::new(90.0, 0.0, 0.0));
    observe(&mut session, simulator, region, 33, 22, 22, Vec3::splat(0.25), Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    settle(&mut consumer, &mut session);
    assert_eq!(consumer.restarts, 0);
    assert_eq!(consumer.records.len(), capacity);
    assert!(!ids(&consumer).contains(&2));
    assert!(ids(&consumer).contains(&22));
}

#[test]
fn replaces_deleted_slot_incarnations() {
    let (mut session, simulator, region) = new_session();
    set_origin(&mut session, region, DVec3::ZERO);
    let old = observe(&mut session, simulator, region, 1, 400, 7, Vec3::X, Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    let mut consumer = ProxyBoxConsumer::new(session.reader(), test_view(DVec3::ZERO, DVec3::X, 10.0), default_limits());
    settle(&mut consumer, &mut session);
    assert_eq!(consumer.records().next().unwrap().entity, old);

    remove(&mut session, simulator, region, 2, 7);
    assert_eq!(session.world().entity(UUID::from_u128(400)), None);
    let replacement =
        observe(&mut session, simulator, region, 3, 401, 7, Vec3::new(2.0, 0.0, 0.0), Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    assert_ne!(old, replacement);
    settle(&mut consumer, &mut session);
    let record = consumer.records().next().unwrap();
    assert_eq!(record.entity, replacement);
    assert_eq!(record.id, UUID::from_u128(401));
    assert_eq!(record.address.local_id(), nz(7));
}

#[test]
fn reboots_after_journal_loss() {
    let simulator = simulator_id();
    let mut limits = viewer_world_limits();
    limits.max_journal_entries = 1;
    let mut session = SessionWorld::new(limits);
    let region = session.register_simulator(simulator, None).expect("register simulator");
    set_origin(&mut session, region, DVec3::ZERO);
    observe(&mut session, simulator, region, 1, 500, 1, Vec3::X, Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    let reader = session.reader();
    let mut consumer = ProxyBoxConsumer::new(reader, test_view(DVec3::ZERO, DVec3::X, 100.0), default_limits());
    assert_eq!(drive(&mut session, consumer.advance()).unwrap(), Progress::Began);
    assert!(matches!(drive(&mut session, consumer.advance()).unwrap(), Progress::Scanned(_)));
    assert_eq!(consumer.phase, Phase::Reconcile);

    patch_position(&mut session, simulator, region, 2, 1, Vec3::new(2.0, 0.0, 0.0));
    patch_position(&mut session, simulator, region, 3, 1, Vec3::new(3.0, 0.0, 0.0));
    let before = consumer.restarts;
    assert_eq!(drive(&mut session, consumer.advance()).unwrap(), Progress::Restarted);
    assert!(consumer.restarts > before);
    settle(&mut consumer, &mut session);
    let record = consumer.records().next().unwrap();
    assert!(matches!(record.geometry, ProxyGeometry::Box { center, .. } if center == DVec3::new(3.0, 0.0, 0.0)));
}

#[test]
fn ignores_old_session_results() {
    let (mut old_session, old_simulator, old_region) = new_session();
    set_origin(&mut old_session, old_region, DVec3::ZERO);
    let old_entity =
        observe(&mut old_session, old_simulator, old_region, 1, 600, 1, Vec3::X, Some(Vec3::ONE), ParentUpdate::Root, Kind::Object);
    let old_reader = old_session.reader();
    let old_item = old_session.world().object_list_item(old_entity).unwrap().item;
    let old_batch = read(&mut old_session, old_reader.object_details(&[old_entity])).unwrap();

    let (mut new_session, new_simulator, new_region) = new_session();
    set_origin(&mut new_session, new_region, DVec3::ZERO);
    observe(
        &mut new_session,
        new_simulator,
        new_region,
        1,
        600,
        1,
        Vec3::new(9.0, 0.0, 0.0),
        Some(Vec3::ONE),
        ParentUpdate::Root,
        Kind::Object,
    );

    let mut consumer = ProxyBoxConsumer::new(old_reader.clone(), test_view(DVec3::ZERO, DVec3::X, 20.0), default_limits());
    consumer.replace_reader(new_session.reader());
    assert_eq!(drive(&mut new_session, consumer.advance()).unwrap(), Progress::Began);
    assert!(!consumer.accept_details_from(&old_reader, &[old_item], old_batch));
    assert_eq!(consumer.records.len(), 0);
    settle(&mut consumer, &mut new_session);
    assert_eq!(ids(&consumer), [600]);
    assert!(
        matches!(consumer.records().next().unwrap().geometry, ProxyGeometry::Box { center, .. } if center == DVec3::new(9.0, 0.0, 0.0))
    );
}
