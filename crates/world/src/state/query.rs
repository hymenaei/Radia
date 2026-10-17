use std::{
    collections::{Bound::Excluded, HashSet},
    time::Instant,
};

use base::UUID;
use math::DVec3;

use super::{
    AttachmentPoint, Entity, Kind, ObjectListItem, PoseState, Properties, PropertiesProvenance, RecentObject, Region, World, WorldId,
    WorldPose, WorldReadError, WorldRevision,
    spatial::{Cell, CellRange},
};

pub const MAX_WORLD_QUERY_WORK: usize = 65_536;
pub const MAX_WORLD_QUERY_RESULTS: usize = 256;
const DEFAULT_WORLD_QUERY_WORK: usize = 4096;
const DEFAULT_WORLD_QUERY_RESULTS: usize = 128;

/// Selects live objects, retained removals, or both.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryScope {
    Live,
    Recent,
    LiveAndRecent,
}

/// Matches a known value exactly or a known value different from the supplied value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueFilter<T> {
    Is(T),
    IsNot(T),
}

impl<T: Eq> ValueFilter<T> {
    fn matches(self, value: Option<T>) -> bool {
        match (self, value) {
            (Self::Is(expected), Some(value)) => value == expected,
            (Self::IsNot(expected), Some(value)) => value != expected,
            (Self::Is(_) | Self::IsNot(_), None) => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Sphere {
    center: DVec3,
    radius: f64,
}

/// A typed filter evaluated against one World revision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldQuery {
    scope: QueryScope,
    region: Option<Region>,
    kind: Option<ValueFilter<Kind>>,
    attachment: Option<ValueFilter<AttachmentPoint>>,
    owner: Option<ValueFilter<UUID>>,
    creator: Option<ValueFilter<UUID>>,
    sphere: Option<Sphere>,
}

/// Invalid input when constructing a spatial filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorldQueryBuildError {
    InvalidCenter,
    InvalidRadius,
    OutOfRange,
}

impl WorldQuery {
    pub const fn live() -> Self {
        Self::new(QueryScope::Live)
    }

    pub const fn recent() -> Self {
        Self::new(QueryScope::Recent)
    }

    pub const fn live_and_recent() -> Self {
        Self::new(QueryScope::LiveAndRecent)
    }

    const fn new(scope: QueryScope) -> Self {
        Self { scope, region: None, kind: None, attachment: None, owner: None, creator: None, sphere: None }
    }

    pub const fn in_region(mut self, region: Region) -> Self {
        self.region = Some(region);
        self
    }

    pub const fn kind(mut self, filter: ValueFilter<Kind>) -> Self {
        self.kind = Some(filter);
        self
    }

    pub const fn attachment(mut self, filter: ValueFilter<AttachmentPoint>) -> Self {
        self.attachment = Some(filter);
        self
    }

    pub const fn owner(mut self, filter: ValueFilter<UUID>) -> Self {
        self.owner = Some(filter);
        self
    }

    pub const fn creator(mut self, filter: ValueFilter<UUID>) -> Self {
        self.creator = Some(filter);
        self
    }

    /// Restricts results to resolved global positions within an exact Euclidean radius in meters.
    pub fn within(mut self, center: DVec3, radius: f64) -> Result<Self, WorldQueryBuildError> {
        if !center.is_finite() {
            return Err(WorldQueryBuildError::InvalidCenter);
        }
        if !radius.is_finite() || radius < 0.0 {
            return Err(WorldQueryBuildError::InvalidRadius);
        }
        if Cell::range(center, radius).is_none() {
            return Err(WorldQueryBuildError::OutOfRange);
        }
        self.sphere = Some(Sphere { center, radius });
        Ok(self)
    }

    const fn is_spatial(self) -> bool {
        self.sphere.is_some()
    }
}

/// Work and result bounds for one query call.
///
/// Work counts inspected candidates, spatial cells, and region entries scanned for dirty objects.
/// Filtering, pose resolution, and copying a candidate's result are charged to that candidate. A
/// full allowance can conservatively make item results incomplete when the scan cannot prove it
/// has exhausted its candidates. The unresolved count has separate completeness and can be
/// incomplete while item results remain exact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldQueryBudget {
    pub work: usize,
    pub results: usize,
}

impl Default for WorldQueryBudget {
    fn default() -> Self {
        Self { work: DEFAULT_WORLD_QUERY_WORK, results: DEFAULT_WORLD_QUERY_RESULTS }
    }
}

/// Whether a query exhausted every candidate needed for its answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryCompleteness {
    Exact,
    Incomplete { reason: QueryIncompleteReason },
}

/// Why the World stopped before the complete answer fit in one bounded call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryIncompleteReason {
    WorkLimit,
    ResultLimit,
}

/// Counts selected live or recent objects without resolved positions in a spatial query.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueryUnresolved {
    /// Number of eligible objects without a resolved pose.
    pub objects: usize,
    /// Whether the query checked every candidate that could affect this count.
    pub complete: bool,
}

/// One live match or one bounded record of a removed object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WorldQueryItem {
    Live { object: ObjectListItem, pose: Option<WorldPose>, properties: Properties, properties_provenance: PropertiesProvenance },
    Recent(RecentObject),
}

impl WorldQueryItem {
    pub const fn id(self) -> UUID {
        match self {
            Self::Live { object, .. } => object.id,
            Self::Recent(object) => object.id,
        }
    }
}

/// Which ordered portion of a combined query remains to be scanned.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum QueryCursorPhase {
    Live,
    Recent,
}

/// Continuation for one non-spatial query result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldQueryCursor {
    world: WorldId,
    query: WorldQuery,
    revision: WorldRevision,
    recent_expires_at: Option<Instant>,
    phase: QueryCursorPhase,
    after: Option<UUID>,
}

/// Bounded matches from one query call.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldQueryPage {
    pub observed_revision: WorldRevision,
    pub items: Vec<WorldQueryItem>,
    pub completeness: QueryCompleteness,
    pub unresolved: QueryUnresolved,
    pub next: Option<WorldQueryCursor>,
}

struct QueryRun {
    remaining: usize,
    result_limit: usize,
    incomplete: Option<QueryIncompleteReason>,
    items: Vec<WorldQueryItem>,
    seen_entities: HashSet<Entity>,
    unresolved_ids: HashSet<UUID>,
    unresolved_complete: bool,
}

impl QueryRun {
    fn new(budget: WorldQueryBudget) -> Self {
        Self {
            remaining: budget.work,
            result_limit: budget.results,
            incomplete: None,
            items: Vec::with_capacity(budget.results),
            seen_entities: HashSet::new(),
            unresolved_ids: HashSet::new(),
            unresolved_complete: true,
        }
    }

    fn spend(&mut self, amount: usize) -> bool {
        if self.remaining < amount {
            self.incomplete = Some(QueryIncompleteReason::WorkLimit);
            return false;
        }
        self.remaining -= amount;
        true
    }

    fn exhaust_work(&mut self) -> bool {
        if self.remaining == 0 {
            self.incomplete = Some(QueryIncompleteReason::WorkLimit);
            true
        } else {
            false
        }
    }

    fn spend_unresolved(&mut self) -> bool {
        if self.remaining == 0 {
            self.unresolved_complete = false;
            return false;
        }
        self.remaining -= 1;
        true
    }

    fn unresolved(&mut self, id: UUID) {
        self.unresolved_ids.insert(id);
    }

    fn finish(self, revision: WorldRevision, next: Option<WorldQueryCursor>) -> WorldQueryPage {
        let mut items = self.items;
        items.sort_by_key(|item| match item {
            WorldQueryItem::Live { object, .. } => (QueryCursorPhase::Live, object.id),
            WorldQueryItem::Recent(object) => (QueryCursorPhase::Recent, object.id),
        });
        WorldQueryPage {
            observed_revision: revision,
            items,
            completeness: self.incomplete.map_or(QueryCompleteness::Exact, |reason| QueryCompleteness::Incomplete { reason }),
            unresolved: QueryUnresolved { objects: self.unresolved_ids.len(), complete: self.unresolved_complete },
            next,
        }
    }
}

impl World {
    /// Returns a bounded query result copied from one World revision.
    ///
    /// Spatial matches are checked against current resolved poses. If spatial repair is pending,
    /// the query scans those scopes from the authoritative entity set under the same work bound.
    /// Unresolved poses are counted separately and do not by themselves make located results
    /// incomplete. Non-spatial queries can be continued with a cursor. World mutations invalidate
    /// every cursor, and retained-record expiry invalidates recent-scope cursors. Combined queries
    /// return live matches before retained records.
    pub fn query(
        &self,
        query: WorldQuery,
        cursor: Option<WorldQueryCursor>,
        budget: WorldQueryBudget,
    ) -> Result<WorldQueryPage, WorldReadError> {
        self.query_at(query, cursor, budget, Instant::now())
    }

    fn query_at(
        &self,
        query: WorldQuery,
        cursor: Option<WorldQueryCursor>,
        budget: WorldQueryBudget,
        now: Instant,
    ) -> Result<WorldQueryPage, WorldReadError> {
        validate_budget(budget)?;
        validate_cursor(self, query, cursor, now)?;

        let revision = self.revision();
        let mut run = QueryRun::new(budget);
        let mut next = None;
        let has_live = matches!(query.scope, QueryScope::Live | QueryScope::LiveAndRecent);
        let has_recent = matches!(query.scope, QueryScope::Recent | QueryScope::LiveAndRecent);
        let recent_expires_at =
            cursor.map_or_else(|| if has_recent { self.recent.next_expiry_after(now) } else { None }, |cursor| cursor.recent_expires_at);
        let phase = cursor.map_or_else(
            || if query.scope == QueryScope::Recent { QueryCursorPhase::Recent } else { QueryCursorPhase::Live },
            |cursor| cursor.phase,
        );
        let mut live_scope_scanned = false;

        if has_live && phase == QueryCursorPhase::Live {
            if query.is_spatial() {
                live_scope_scanned = self.query_spatial_live(query, &mut run);
            } else {
                next = self.query_live_page(query, cursor.and_then(|cursor| cursor.after), revision, recent_expires_at, &mut run);
            }
        }
        if has_recent && run.incomplete.is_none() {
            let after = if phase == QueryCursorPhase::Recent { cursor.and_then(|cursor| cursor.after) } else { None };
            next = self.query_recent(query, after, revision, recent_expires_at, now, &mut run);
        }
        if query.is_spatial() && run.unresolved_complete {
            if run.incomplete.is_some() {
                run.unresolved_complete = false;
            } else if has_live && !live_scope_scanned {
                self.query_unresolved(query, &mut run);
            }
        }

        Ok(run.finish(revision, next))
    }

    fn query_live_page(
        &self,
        query: WorldQuery,
        after: Option<UUID>,
        revision: WorldRevision,
        recent_expires_at: Option<Instant>,
        run: &mut QueryRun,
    ) -> Option<WorldQueryCursor> {
        let start = after;
        let ids = match query.region {
            Some(region) => self.region_entities.get(&region)?,
            None => &self.object_ids,
        };
        let mut ids = match start {
            Some(id) => ids.range((Excluded(id), std::ops::Bound::Unbounded)),
            None => ids.range(..),
        };
        let mut last_evaluated = start;
        let mut page_continuation = None;
        loop {
            if run.exhaust_work() {
                break;
            }
            let Some(id) = ids.next() else { break };
            if !run.spend(1) {
                break;
            }
            let entity = self.entities[id];
            let Some(item) = self.live_item(entity, query, None, run) else {
                if run.incomplete.is_some() {
                    break;
                }
                last_evaluated = Some(*id);
                if run.exhaust_work() {
                    break;
                }
                continue;
            };
            if run.items.len() == run.result_limit {
                let after = run.items.last().map(|item| item.id()).expect("a full result page contains an ID");
                page_continuation = Some(WorldQueryCursor {
                    world: self.id,
                    query,
                    revision,
                    recent_expires_at,
                    phase: QueryCursorPhase::Live,
                    after: Some(after),
                });
                run.incomplete = Some(QueryIncompleteReason::ResultLimit);
                break;
            }
            run.items.push(item);
            last_evaluated = Some(*id);
            if run.exhaust_work() {
                break;
            }
        }

        if run.incomplete == Some(QueryIncompleteReason::WorkLimit)
            && let Some(after) = last_evaluated
        {
            return Some(WorldQueryCursor {
                world: self.id,
                query,
                revision,
                recent_expires_at,
                phase: QueryCursorPhase::Live,
                after: Some(after),
            });
        }
        page_continuation
    }

    fn query_spatial_live(&self, query: WorldQuery, run: &mut QueryRun) -> bool {
        let sphere = query.sphere.expect("spatial query has a sphere");
        let cells = Cell::range(sphere.center, sphere.radius).expect("query construction validated the sphere bounds");
        let candidate_count =
            query.region.map_or_else(|| self.object_ids.len(), |region| self.region_entities.get(&region).map_or(0, |ids| ids.len()));
        if cells.contains_more_than(candidate_count) {
            return self.query_spatial_entities(query, sphere, run);
        }
        self.query_spatial_cells(query, sphere, cells, run);
        if run.incomplete.is_some() {
            return false;
        }
        if self.spatial_has_dirty_scope(query.region) {
            self.query_dirty_live(query, sphere, run);
        }
        false
    }

    fn query_spatial_entities(&self, query: WorldQuery, sphere: Sphere, run: &mut QueryRun) -> bool {
        if let Some(region) = query.region {
            let Some(ids) = self.region_entities.get(&region) else { return true };
            self.query_spatial_entity_ids(ids.iter().copied(), query, sphere, run)
        } else {
            self.query_spatial_entity_ids(self.object_ids.iter().copied(), query, sphere, run)
        }
    }

    fn query_spatial_entity_ids(&self, ids: impl Iterator<Item = UUID>, query: WorldQuery, sphere: Sphere, run: &mut QueryRun) -> bool {
        for id in ids {
            if !run.spend(1) {
                return false;
            }
            let entity = *self.entities.get(&id).expect("object ordering index contains only live entities");
            if let Some(item) = self.live_item(entity, query, Some(sphere), run) {
                if run.items.len() == run.result_limit {
                    run.incomplete = Some(QueryIncompleteReason::ResultLimit);
                    return false;
                }
                run.items.push(item);
            }
            if run.incomplete.is_some() {
                return false;
            }
        }
        true
    }

    fn query_spatial_cells(&self, query: WorldQuery, sphere: Sphere, cells: CellRange, run: &mut QueryRun) {
        let mut cells = cells;
        loop {
            if run.exhaust_work() {
                return;
            }
            let Some(cell) = cells.next() else { break };
            if !run.spend(1) {
                return;
            }
            let Some(candidates) = self.spatial.candidate_ids(cell) else { continue };
            let mut candidates = candidates.values();
            loop {
                if run.exhaust_work() {
                    return;
                }
                let Some(entity) = candidates.next() else { break };
                if !run.spend(1) {
                    return;
                }
                run.seen_entities.insert(*entity);
                if let Some(item) = self.live_item(*entity, query, Some(sphere), run) {
                    if run.items.len() == run.result_limit {
                        run.incomplete = Some(QueryIncompleteReason::ResultLimit);
                        return;
                    }
                    run.items.push(item);
                }
                if run.incomplete.is_some() {
                    return;
                }
            }
        }
    }

    fn query_dirty_live(&self, query: WorldQuery, sphere: Sphere, run: &mut QueryRun) {
        if let Some(region) = query.region {
            self.query_dirty_region(region, query, sphere, run);
        } else {
            let mut regions = self.spatial.dirty_scope_regions();
            loop {
                if run.exhaust_work() {
                    return;
                }
                let Some(region) = regions.next() else { break };
                if !self.query_dirty_region(region, query, sphere, run) {
                    return;
                }
            }
        }
    }

    fn query_dirty_region(&self, region: Region, query: WorldQuery, sphere: Sphere, run: &mut QueryRun) -> bool {
        if run.exhaust_work() {
            return false;
        }
        if !run.spend(1) {
            return false;
        }
        let Some(ids) = self.region_entities.get(&region) else { return true };
        let mut ids = ids.iter();
        loop {
            if run.exhaust_work() {
                return false;
            }
            let Some(id) = ids.next() else { break };
            if !run.spend(1) {
                return false;
            }
            let entity = *self.entities.get(id).expect("region index contains only live objects");
            if self.spatial_is_dirty(entity) {
                self.check_dirty_entity(entity, query, sphere, run);
            }
            if run.incomplete.is_some() {
                return false;
            }
        }
        true
    }

    fn check_dirty_entity(&self, entity: Entity, query: WorldQuery, sphere: Sphere, run: &mut QueryRun) {
        if run.seen_entities.contains(&entity) {
            return;
        }
        if let Some(item) = self.live_item(entity, query, Some(sphere), run) {
            if run.items.len() == run.result_limit {
                run.incomplete = Some(QueryIncompleteReason::ResultLimit);
                return;
            }
            run.items.push(item);
        }
    }

    fn query_unresolved(&self, query: WorldQuery, run: &mut QueryRun) {
        if let Some(region) = query.region {
            self.query_unresolved_ids(self.spatial.unresolved_ids_in(region), query, run);
        } else {
            self.query_unresolved_ids(self.spatial.unresolved_ids(), query, run);
        }
    }

    fn query_unresolved_ids(&self, mut unresolved: impl Iterator<Item = (Region, UUID, Entity)>, query: WorldQuery, run: &mut QueryRun) {
        loop {
            if run.remaining == 0 {
                run.unresolved_complete = false;
                return;
            }
            let Some((region, _id, entity)) = unresolved.next() else { break };
            if !run.spend_unresolved() {
                run.unresolved_complete = false;
                return;
            }
            if query.region.is_some_and(|filter| filter != region) || self.spatial_is_dirty(entity) {
                continue;
            }
            if let Some(object) = self.object(entity)
                && self.live_matches_without_pose(entity, query)
            {
                run.unresolved(object.identity().id());
            }
            if run.remaining == 0 {
                run.unresolved_complete = false;
                return;
            }
        }
    }

    fn query_recent(
        &self,
        query: WorldQuery,
        after: Option<UUID>,
        revision: WorldRevision,
        recent_expires_at: Option<Instant>,
        now: Instant,
        run: &mut QueryRun,
    ) -> Option<WorldQueryCursor> {
        let mut recent = self.recent.iter_after(after);
        let mut last_evaluated = after;
        loop {
            if run.exhaust_work() {
                break;
            }
            let (id, entry) = recent.next()?;
            if !run.spend(1) {
                break;
            }
            if self.recent.is_expired(entry, now) {
                last_evaluated = Some(id);
                continue;
            }
            if query.scope == QueryScope::LiveAndRecent && self.entities.contains_key(&id) {
                last_evaluated = Some(id);
                continue;
            }
            if !recent_matches(entry.object, query) {
                if query.sphere.is_some() && entry.object.last_pose.is_none() && recent_matches_without_radius(entry.object, query) {
                    run.unresolved(id);
                }
                last_evaluated = Some(id);
                continue;
            }
            if run.items.len() == run.result_limit {
                run.incomplete = Some(QueryIncompleteReason::ResultLimit);
                return (!query.is_spatial()).then_some(WorldQueryCursor {
                    world: self.id,
                    query,
                    revision,
                    recent_expires_at,
                    phase: QueryCursorPhase::Recent,
                    after: last_evaluated,
                });
            }
            run.items.push(super::WorldQueryItem::Recent(entry.object));
            last_evaluated = Some(id);
            if run.exhaust_work() {
                break;
            }
        }
        (run.incomplete == Some(QueryIncompleteReason::WorkLimit) && !query.is_spatial()).then_some(WorldQueryCursor {
            world: self.id,
            query,
            revision,
            recent_expires_at,
            phase: QueryCursorPhase::Recent,
            after: last_evaluated,
        })
    }

    fn live_item(&self, entity: Entity, query: WorldQuery, sphere: Option<Sphere>, run: &mut QueryRun) -> Option<WorldQueryItem> {
        let object = self.object(entity)?;
        let id = object.identity().id();
        if !self.live_matches_without_pose(entity, query) {
            return None;
        }

        let address = object.address();
        let properties = object.properties();
        let provenance = object.properties_provenance();

        let pose = if let Some(sphere) = sphere {
            match object.world_pose() {
                Ok(PoseState::Resolved(pose)) if within_radius(pose.position, sphere) => Some(pose),
                Ok(PoseState::Resolved(_)) => return None,
                Ok(PoseState::Unresolved(_)) | Ok(PoseState::Provisional { .. }) | Err(_) => {
                    run.unresolved(id);
                    return None;
                }
            }
        } else {
            None
        };
        let item = ObjectListItem { entity, id, kind: object.kind(), address };
        Some(WorldQueryItem::Live { object: item, pose, properties, properties_provenance: provenance })
    }

    fn live_matches_without_pose(&self, entity: Entity, query: WorldQuery) -> bool {
        let Some(object) = self.object(entity) else { return false };
        matches_query_fields(
            query,
            object.address().region(),
            object.kind(),
            object.attachment_point(),
            object.properties(),
            object.properties_provenance(),
        )
    }

    fn spatial_has_dirty_scope(&self, region: Option<Region>) -> bool {
        self.spatial.has_dirty_scope_in(region)
    }
}

fn validate_budget(budget: WorldQueryBudget) -> Result<(), WorldReadError> {
    if budget.work == 0 || budget.work > MAX_WORLD_QUERY_WORK || budget.results == 0 || budget.results > MAX_WORLD_QUERY_RESULTS {
        Err(WorldReadError::InvalidLimit)
    } else {
        Ok(())
    }
}

fn validate_cursor(world: &World, query: WorldQuery, cursor: Option<WorldQueryCursor>, now: Instant) -> Result<(), WorldReadError> {
    let Some(cursor) = cursor else { return Ok(()) };
    let phase_matches_scope = match query.scope {
        QueryScope::Live => cursor.phase == QueryCursorPhase::Live,
        QueryScope::Recent => cursor.phase == QueryCursorPhase::Recent,
        QueryScope::LiveAndRecent => true,
    };
    if !phase_matches_scope || query.is_spatial() {
        return Err(WorldReadError::InvalidCursor);
    }
    if cursor.world != world.id {
        return Err(WorldReadError::StaleSession);
    }
    if cursor.query != query {
        return Err(WorldReadError::InvalidCursor);
    }
    if cursor.revision != world.revision() {
        return Err(WorldReadError::StaleRevision);
    }
    if cursor.recent_expires_at.is_some_and(|expires_at| expires_at <= now) {
        return Err(WorldReadError::InvalidCursor);
    }
    Ok(())
}

fn within_radius(position: DVec3, sphere: Sphere) -> bool {
    let delta = position - sphere.center;
    delta.x.hypot(delta.y).hypot(delta.z) <= sphere.radius
}

fn matches_query_fields(
    query: WorldQuery,
    region: Region,
    kind: Kind,
    attachment: Option<AttachmentPoint>,
    properties: Properties,
    provenance: PropertiesProvenance,
) -> bool {
    if query.region.is_some_and(|expected| expected != region)
        || !query.kind.is_none_or(|filter| filter.matches(Some(kind)))
        || !query.attachment.is_none_or(|filter| filter.matches(attachment))
    {
        return false;
    }
    let owner = provenance.owner.and(properties.owner_id);
    let creator = provenance.creator.and(properties.creator_id);
    query.owner.is_none_or(|filter| filter.matches(owner)) && query.creator.is_none_or(|filter| filter.matches(creator))
}

fn recent_matches(object: RecentObject, query: WorldQuery) -> bool {
    if !matches_query_fields(
        query,
        object.address.region(),
        object.kind,
        object.attachment_point,
        object.properties,
        object.properties_provenance,
    ) {
        return false;
    }
    if let Some(sphere) = query.sphere {
        return object.last_pose.is_some_and(|pose| within_radius(pose.position, sphere));
    }
    true
}

fn recent_matches_without_radius(object: RecentObject, query: WorldQuery) -> bool {
    let mut query = query;
    query.sphere = None;
    recent_matches(object, query)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        num::{NonZeroU8, NonZeroU32},
        time::{Duration, Instant},
    };

    use math::{Quat, Vec3};

    use super::*;
    use crate::{
        AttachmentPoint, MAX_QUERY_STATE_MAINTENANCE_WORK, ObjectObservation, OrderEvidence::Ordered, ParentUpdate, ParentUpdate::Root,
        Pose, PropertiesPatch, RECENT_OBJECT_TTL, RegionInfo, UpdateSource, WorldEntityRemovalReason, WorldLimits,
    };

    fn limits() -> WorldLimits {
        WorldLimits {
            max_region_registrations: 4,
            max_entities: 1024,
            max_entities_per_region: 1024,
            max_tombstones_per_region: 8,
            max_tombstones: 16,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(8),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(16),
            max_journal_entries: 128,
            max_journal_changes: 4096,
            max_region_name_bytes: 32,
            max_recovery_targets: 16,
        }
    }

    fn add(world: &mut World, region: Region, id: u128, local_id: u32, position: Vec3, kind: Kind) -> Entity {
        let observation = ObjectObservation {
            source: UpdateSource::new(region, 1, Ordered(id as u64 + 1), 0),
            id: UUID::from_u128(id),
            local_id: NonZeroU32::new(local_id).unwrap(),
            kind,
            parent: Root,
            pose: Some(Pose { position: Some(position), rotation: Some(Quat::IDENTITY) }),
            linear_motion: None,
            angular_velocity: None,
            scale: None,
            attachment_point: None,
            shape: None,
            appearance: None,
            surface_appearance: None,
            render_materials: None,
        };
        let result = world.observe_object(observation).expect("object observation");
        match result.outcome {
            crate::ApplyOutcome::Introduced(entity) => entity,
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }
    }

    fn add_with_query_metadata(
        world: &mut World,
        region: Region,
        id: u128,
        local_id: u32,
        attachment_point: Option<AttachmentPoint>,
        owner_id: Option<UUID>,
        creator_id: Option<UUID>,
    ) -> Entity {
        let id = UUID::from_u128(id);
        let observation = ObjectObservation {
            source: UpdateSource::new(region, local_id, Ordered(u64::from(local_id)), 0),
            id,
            local_id: NonZeroU32::new(local_id).expect("local ID is nonzero"),
            kind: Kind::Object,
            parent: Root,
            pose: None,
            linear_motion: None,
            angular_velocity: None,
            scale: None,
            attachment_point: Some(attachment_point),
            shape: None,
            appearance: None,
            surface_appearance: None,
            render_materials: None,
        };
        let result = world.observe_object(observation).expect("query fixture object");
        let crate::ApplyOutcome::Introduced(entity) = result.outcome else { panic!("query fixture object is introduced") };

        if owner_id.is_some() || creator_id.is_some() {
            let target = world.properties_target(entity).expect("query fixture properties target");
            let order = u64::from(local_id) + 100;
            let applied = world
                .patch_properties(
                    UpdateSource::new(region, order as u32, Ordered(order), 0),
                    target,
                    id,
                    PropertiesPatch { owner_id, creator_id },
                )
                .expect("query fixture properties");
            assert_eq!(applied.outcome, crate::ApplyOutcome::Updated(entity));
        }

        entity
    }

    fn settle(world: &mut World) {
        loop {
            let step = world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).unwrap();
            if step.dirty_scopes == 0 {
                break;
            }
        }
    }

    fn budget(results: usize) -> WorldQueryBudget {
        WorldQueryBudget { work: MAX_WORLD_QUERY_WORK, results }
    }

    fn spatial_page_at(world: &World, region: Region, x: f64) -> WorldQueryPage {
        let query = WorldQuery::live().in_region(region).within(DVec3::new(x, 0.0, 0.0), 0.25).unwrap();
        world.query(query, None, budget(64)).expect("spatial query")
    }

    fn add_region(world: &mut World, region: Region) {
        let _ = world.adopt_region(region).unwrap();
    }

    fn set_origin(world: &mut World, region: Region) {
        let _ = world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).unwrap();
    }

    #[test]
    fn radius_includes_boundary_only() {
        let region = Region::new(1);
        let mut world = World::new(limits());
        add_region(&mut world, region);
        set_origin(&mut world, region);
        add(&mut world, region, 1, 1, Vec3::new(3.0, 4.0, 0.0), Kind::Object);
        add(&mut world, region, 2, 2, Vec3::new(3.01, 4.0, 0.0), Kind::Object);
        settle(&mut world);

        let query = WorldQuery::live().within(DVec3::ZERO, 5.0).unwrap();
        let result = world.query(query, None, budget(8)).unwrap();

        assert_eq!(result.completeness, QueryCompleteness::Exact);
        assert_eq!(result.items.iter().map(|item| item.id()).collect::<Vec<_>>(), [UUID::from_u128(1)]);
    }

    #[test]
    fn metadata_filters_require_known_values() {
        let region = Region::new(2);
        let mut world = World::new(limits());
        add_region(&mut world, region);
        set_origin(&mut world, region);
        let known = UUID::from_u128(11);
        let other = UUID::from_u128(12);
        let owner = UUID::from_u128(91);
        let other_owner = UUID::from_u128(92);
        let creator = UUID::from_u128(93);
        let other_creator = UUID::from_u128(94);
        let attachment = AttachmentPoint::new(NonZeroU8::new(5).expect("attachment point is nonzero"));
        let other_attachment = AttachmentPoint::new(NonZeroU8::new(6).expect("attachment point is nonzero"));

        add_with_query_metadata(&mut world, region, 11, 1, Some(attachment), Some(owner), Some(creator));
        add_with_query_metadata(&mut world, region, 12, 2, Some(other_attachment), Some(other_owner), Some(other_creator));
        add_with_query_metadata(&mut world, region, 13, 3, None, None, None);

        assert_eq!(query_ids(&world, WorldQuery::live().owner(ValueFilter::Is(owner))), HashSet::from([known]), "known owner matches Is",);
        assert_eq!(
            query_ids(&world, WorldQuery::live().owner(ValueFilter::IsNot(owner))),
            HashSet::from([other]),
            "known different owner matches IsNot; missing owner matches neither",
        );
        assert_eq!(
            query_ids(&world, WorldQuery::live().creator(ValueFilter::Is(creator))),
            HashSet::from([known]),
            "known creator matches Is",
        );
        assert_eq!(
            query_ids(&world, WorldQuery::live().creator(ValueFilter::IsNot(creator))),
            HashSet::from([other]),
            "known different creator matches IsNot; missing creator matches neither",
        );
        assert_eq!(
            query_ids(&world, WorldQuery::live().attachment(ValueFilter::Is(attachment))),
            HashSet::from([known]),
            "known attachment point matches Is",
        );
        assert_eq!(
            query_ids(&world, WorldQuery::live().attachment(ValueFilter::IsNot(attachment))),
            HashSet::from([other]),
            "known different attachment point matches IsNot; missing point matches neither",
        );
    }

    #[test]
    fn region_filter_does_not_spend_work_on_other_regions() {
        let first = Region::new(6);
        let second = Region::new(7);
        let mut world = World::new(limits());
        add_region(&mut world, first);
        add_region(&mut world, second);
        for id in 1..=16 {
            add(&mut world, first, id, id as u32, Vec3::ZERO, Kind::Object);
        }
        add(&mut world, second, 100, 100, Vec3::ZERO, Kind::Object);

        let page = world
            .query(WorldQuery::live().in_region(second), None, WorldQueryBudget { work: 1, results: 1 })
            .expect("region-filtered query");

        assert_eq!(page.items.iter().copied().map(WorldQueryItem::id).collect::<Vec<_>>(), [UUID::from_u128(100)]);
    }

    fn query_ids(world: &World, query: WorldQuery) -> HashSet<UUID> {
        let page = world.query(query, None, budget(8)).expect("metadata query");
        assert_eq!(page.completeness, QueryCompleteness::Exact);
        page.items.into_iter().map(WorldQueryItem::id).collect()
    }

    #[test]
    fn unresolved_poses_keep_results_exact() {
        let region = Region::new(3);
        let mut world = World::new(limits());
        add_region(&mut world, region);
        add(&mut world, region, 21, 1, Vec3::ZERO, Kind::Object);
        let query = WorldQuery::live().within(DVec3::ZERO, 10.0).unwrap();
        let result = world.query(query, None, WorldQueryBudget { work: 32, results: 8 }).unwrap();

        assert_eq!(result.completeness, QueryCompleteness::Exact);
        assert_eq!(result.unresolved.objects, 1);
        assert!(result.unresolved.complete);
    }

    #[test]
    fn cursor_rejects_query_or_revision_change() {
        let region = Region::new(4);
        let mut world = World::new(limits());
        add_region(&mut world, region);
        set_origin(&mut world, region);
        for id in 31..35 {
            add(&mut world, region, id, id as u32, Vec3::ZERO, Kind::Object);
        }
        let query = WorldQuery::live();
        let first = world.query(query, None, budget(2)).unwrap();

        assert!(first.next.is_some());
        let second = world.query(query, first.next, budget(2)).unwrap();
        assert_ne!(first.items[0].id(), second.items[0].id());

        let changed = WorldQuery::live().in_region(region);
        assert_eq!(world.query(changed, first.next, budget(2)), Err(WorldReadError::InvalidCursor));
        add(&mut world, region, 99, 99, Vec3::ZERO, Kind::Object);
        assert_eq!(world.query(query, first.next, budget(2)), Err(WorldReadError::StaleRevision));
    }

    #[test]
    fn recent_cursor_expires_after_ttl() {
        let now = Instant::now();
        let region = Region::new(4);
        let mut world = World::new(limits());
        for id in [31_u32, 32] {
            world.recent.insert(
                RecentObject {
                    id: UUID::from_u128(u128::from(id)),
                    kind: Kind::Object,
                    address: super::super::LocalAddress::new(region, NonZeroU32::new(id).unwrap()),
                    attachment_point: None,
                    properties: Properties::default(),
                    properties_provenance: PropertiesProvenance::default(),
                    last_pose: None,
                    removal_reason: WorldEntityRemovalReason::Killed,
                },
                now,
            );
        }
        let query = WorldQuery::recent();
        let revision = world.revision();
        let first = world.query_at(query, None, budget(1), now).unwrap();

        assert_eq!(first.items.iter().map(|item| item.id()).collect::<Vec<_>>(), [UUID::from_u128(31)]);
        assert!(first.next.is_some());
        assert_eq!(world.revision(), revision);
        assert_eq!(world.query_at(query, first.next, budget(1), now + RECENT_OBJECT_TTL), Err(WorldReadError::InvalidCursor));
    }

    #[test]
    fn dirty_fallback_is_exact() {
        let region = Region::new(5);
        let mut world = World::new(limits());
        add_region(&mut world, region);
        set_origin(&mut world, region);
        add(&mut world, region, 41, 1, Vec3::ZERO, Kind::Object);
        settle(&mut world);
        let moved = ObjectObservation {
            source: UpdateSource::new(region, 2, Ordered(90), 0),
            id: UUID::from_u128(41),
            local_id: NonZeroU32::new(1).unwrap(),
            kind: Kind::Object,
            parent: Root,
            pose: Some(Pose { position: Some(Vec3::new(128.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) }),
            linear_motion: None,
            angular_velocity: None,
            scale: None,
            attachment_point: None,
            shape: None,
            appearance: None,
            surface_appearance: None,
            render_materials: None,
        };
        let _ = world.observe_object(moved).unwrap();
        let query = WorldQuery::live().within(DVec3::new(128.0, 0.0, 0.0), 1.0).unwrap();
        let result = world.query(query, None, WorldQueryBudget { work: 32, results: 8 }).unwrap();

        assert_eq!(result.completeness, QueryCompleteness::Exact);
        assert_eq!(result.items.iter().map(|item| item.id()).collect::<Vec<_>>(), [UUID::from_u128(41)]);
    }

    #[test]
    fn deep_ancestry_uses_region_fallback() {
        const CHAIN_LENGTH: u32 = 20;

        let region = Region::new(5);
        let mut world = World::new(limits());
        add_region(&mut world, region);
        set_origin(&mut world, region);
        for local_id in 1..=CHAIN_LENGTH {
            let parent = if local_id == 1 { Root } else { ParentUpdate::Local(NonZeroU32::new(local_id - 1).unwrap()) };
            let observation = ObjectObservation {
                source: UpdateSource::new(region, local_id, Ordered(u64::from(local_id)), 0),
                id: UUID::from_u128(5_000 + u128::from(local_id)),
                local_id: NonZeroU32::new(local_id).unwrap(),
                kind: Kind::Object,
                parent,
                pose: Some(Pose { position: Some(Vec3::X), rotation: Some(Quat::IDENTITY) }),
                linear_motion: None,
                angular_velocity: None,
                scale: None,
                attachment_point: None,
                shape: None,
                appearance: None,
                surface_appearance: None,
                render_materials: None,
            };
            let result = world.observe_object(observation).expect("introduce linkset object");
            assert!(matches!(result.outcome, crate::ApplyOutcome::Introduced(_)));
        }

        let candidate_id = UUID::from_u128(5_000 + u128::from(CHAIN_LENGTH + 1));
        let candidate_local_id = NonZeroU32::new(CHAIN_LENGTH + 1).unwrap();
        let candidate = world
            .observe_object(ObjectObservation {
                source: UpdateSource::new(region, CHAIN_LENGTH + 1, Ordered(u64::from(CHAIN_LENGTH + 1)), 0),
                id: candidate_id,
                local_id: candidate_local_id,
                kind: Kind::Object,
                parent: Root,
                pose: Some(Pose { position: Some(Vec3::new(30.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) }),
                linear_motion: None,
                angular_velocity: None,
                scale: None,
                attachment_point: None,
                shape: None,
                appearance: None,
                surface_appearance: None,
                render_materials: None,
            })
            .expect("introduce queryable object");
        let crate::ApplyOutcome::Introduced(candidate) = candidate.outcome else { panic!("object should be introduced") };
        settle(&mut world);

        let before = spatial_page_at(&world, region, 30.0);
        assert_eq!(before.completeness, QueryCompleteness::Exact);
        assert!(before.unresolved.complete);
        assert_eq!(before.items.iter().map(|item| item.id()).collect::<Vec<_>>(), [candidate_id]);

        let reparented = world
            .observe_object(ObjectObservation {
                source: UpdateSource::new(region, CHAIN_LENGTH + 2, Ordered(u64::from(CHAIN_LENGTH + 2)), 0),
                id: candidate_id,
                local_id: candidate_local_id,
                kind: Kind::Object,
                parent: ParentUpdate::Local(NonZeroU32::new(CHAIN_LENGTH).unwrap()),
                pose: Some(Pose { position: Some(Vec3::X), rotation: Some(Quat::IDENTITY) }),
                linear_motion: None,
                angular_velocity: None,
                scale: None,
                attachment_point: None,
                shape: None,
                appearance: None,
                surface_appearance: None,
                render_materials: None,
            })
            .expect("reparent object below deep linkset");
        assert!(matches!(reparented.outcome, crate::ApplyOutcome::Updated(_)));
        assert!(world.spatial_is_dirty(candidate));

        settle(&mut world);
        assert!(!world.spatial_is_dirty(candidate));

        let after = spatial_page_at(&world, region, 30.0);
        assert_eq!(after.completeness, QueryCompleteness::Exact);
        assert!(after.items.is_empty());
        assert_eq!(after.unresolved.objects, before.unresolved.objects + 1);
        assert!(after.unresolved.complete);
    }

    #[test]
    fn expired_recent_records_are_not_returned() {
        let now = Instant::now();
        let region = Region::new(6);
        let mut world = World::new(limits());
        world.recent.insert(
            RecentObject {
                id: UUID::from_u128(61),
                kind: Kind::Object,
                address: super::super::LocalAddress::new(region, NonZeroU32::new(1).unwrap()),
                attachment_point: None,
                properties: Properties::default(),
                properties_provenance: PropertiesProvenance::default(),
                last_pose: None,
                removal_reason: WorldEntityRemovalReason::Killed,
            },
            now - RECENT_OBJECT_TTL - Duration::from_secs(1),
        );

        let result = world.query(WorldQuery::recent(), None, budget(8)).unwrap();

        assert!(result.items.is_empty());
        assert_eq!(result.completeness, QueryCompleteness::Exact);
    }
}
