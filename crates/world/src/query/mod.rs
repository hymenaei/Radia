pub(crate) mod read;
pub(crate) mod spatial;

use std::{
    collections::{
        BTreeSet,
        Bound::{Excluded, Unbounded},
        HashSet,
    },
    error::Error,
    fmt::{self, Display, Formatter},
    time::Instant,
};

use math::DVec3;
use uuid::Uuid;

use crate::{AttachmentPoint, Entity, Kind, PoseState, Properties, PropertiesProvenance, Region, WorldPose, WorldRevision};
use crate::{
    Object, ObjectListItem, RecentObject, World, WorldId, WorldReadError,
    query::spatial::{Cell, CellRange, SubtreeScan},
};

pub const MAX_WORLD_QUERY_WORK: usize = 65_536;
pub const MAX_WORLD_QUERY_RESULTS: usize = 256;
const DEFAULT_WORLD_QUERY_WORK: usize = 4096;
const DEFAULT_WORLD_QUERY_RESULTS: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryScope {
    Live,
    Recent,
    LiveAndRecent,
}

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldQuery {
    scope: QueryScope,
    region: Option<Region>,
    kind: Option<ValueFilter<Kind>>,
    attachment: Option<ValueFilter<AttachmentPoint>>,
    owner: Option<ValueFilter<Uuid>>,
    creator: Option<ValueFilter<Uuid>>,
    sphere: Option<Sphere>,
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

    pub const fn owner(mut self, filter: ValueFilter<Uuid>) -> Self {
        self.owner = Some(filter);
        self
    }

    pub const fn creator(mut self, filter: ValueFilter<Uuid>) -> Self {
        self.creator = Some(filter);
        self
    }

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorldQueryBuildError {
    InvalidCenter,
    InvalidRadius,
    OutOfRange,
}

impl Display for WorldQueryBuildError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCenter => f.write_str("spatial query center must be finite"),
            Self::InvalidRadius => f.write_str("spatial query radius must be finite and nonnegative"),
            Self::OutOfRange => f.write_str("spatial query exceeds the supported coordinate range"),
        }
    }
}

impl Error for WorldQueryBuildError {}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryCompleteness {
    Exact,
    Incomplete { reason: QueryIncompleteReason },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryIncompleteReason {
    WorkLimit,
    ResultLimit,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueryUnresolved {
    pub objects: usize,
    pub complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WorldQueryItem {
    Live { object: ObjectListItem, pose: Option<WorldPose>, properties: Properties, properties_provenance: PropertiesProvenance },
    Recent(RecentObject),
}

impl WorldQueryItem {
    pub const fn id(self) -> Uuid {
        match self {
            Self::Live { object, .. } => object.id,
            Self::Recent(object) => object.id,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum QueryCursorPhase {
    Live,
    Recent,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldQueryCursor {
    world: WorldId,
    query: WorldQuery,
    revision: WorldRevision,
    recent_expires_at: Option<Instant>,
    phase: QueryCursorPhase,
    after: Option<Uuid>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorldQueryPage {
    pub observed_revision: WorldRevision,
    pub items: Vec<WorldQueryItem>,
    pub completeness: QueryCompleteness,
    pub unresolved: QueryUnresolved,
    pub next: Option<WorldQueryCursor>,
}

struct QueryRun {
    world: WorldId,
    query: WorldQuery,
    revision: WorldRevision,
    recent_expires_at: Option<Instant>,
    remaining: usize,
    result_limit: usize,
    incomplete: Option<QueryIncompleteReason>,
    items: Vec<WorldQueryItem>,
    seen_entities: HashSet<Entity>,
    unresolved_ids: HashSet<Uuid>,
    unresolved_complete: bool,
}

impl QueryRun {
    fn object<'b>(&self, world: &'b World, entity: Entity) -> Option<Object<'b>> {
        if entity.1 != world.id {
            return None;
        }
        world.object(entity)
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

    fn unresolved(&mut self, id: Uuid) {
        self.unresolved_ids.insert(id);
    }

    fn push(&mut self, item: WorldQueryItem) -> bool {
        if self.items.len() == self.result_limit {
            self.incomplete = Some(QueryIncompleteReason::ResultLimit);
            return false;
        }
        self.items.push(item);
        true
    }

    fn cursor(&self, phase: QueryCursorPhase, after: Option<Uuid>) -> WorldQueryCursor {
        WorldQueryCursor {
            world: self.world,
            query: self.query,
            revision: self.revision,
            recent_expires_at: self.recent_expires_at,
            phase,
            after,
        }
    }

    fn finish(self, next: Option<WorldQueryCursor>) -> WorldQueryPage {
        let mut items = self.items;
        items.sort_by_key(|item| match item {
            WorldQueryItem::Live { object, .. } => (QueryCursorPhase::Live, object.id),
            WorldQueryItem::Recent(object) => (QueryCursorPhase::Recent, object.id),
        });
        WorldQueryPage {
            observed_revision: self.revision,
            items,
            completeness: self.incomplete.map_or(QueryCompleteness::Exact, |reason| QueryCompleteness::Incomplete { reason }),
            unresolved: QueryUnresolved { objects: self.unresolved_ids.len(), complete: self.unresolved_complete },
            next,
        }
    }
}

impl World {
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

        let has_live = matches!(query.scope, QueryScope::Live | QueryScope::LiveAndRecent);
        let has_recent = matches!(query.scope, QueryScope::Recent | QueryScope::LiveAndRecent);
        let phase = cursor.map_or_else(
            || if query.scope == QueryScope::Recent { QueryCursorPhase::Recent } else { QueryCursorPhase::Live },
            |cursor| cursor.phase,
        );
        let live = has_live && phase == QueryCursorPhase::Live;
        let mut run = QueryRun {
            world: self.id,
            query,
            revision: self.revision(),
            recent_expires_at: cursor
                .map_or_else(|| if has_recent { self.recent.next_expiry_after(now) } else { None }, |cursor| cursor.recent_expires_at),
            remaining: budget.work,
            result_limit: budget.results,
            incomplete: None,
            items: Vec::with_capacity(budget.results),
            seen_entities: HashSet::new(),
            unresolved_ids: HashSet::new(),
            unresolved_complete: true,
        };
        let mut next = None;
        let mut live_scope_scanned = false;

        if live {
            if query.is_spatial() {
                live_scope_scanned = self.query_spatial_live(&mut run);
            } else {
                next = self.query_live_page(cursor.and_then(|cursor| cursor.after), &mut run);
            }
        }
        if has_recent && run.incomplete.is_none() {
            let after = if phase == QueryCursorPhase::Recent { cursor.and_then(|cursor| cursor.after) } else { None };
            next = self.query_recent(after, now, &mut run);
        }
        if query.is_spatial() && run.unresolved_complete {
            if run.incomplete.is_some() {
                run.unresolved_complete = false;
            } else if has_live && !live_scope_scanned {
                self.query_unresolved(&mut run);
            }
        }

        Ok(run.finish(next))
    }

    fn query_live_page(&self, after: Option<Uuid>, run: &mut QueryRun) -> Option<WorldQueryCursor> {
        let ids = self.live_ids(run.query)?;
        let mut ids = ids.range((after.map_or(Unbounded, Excluded), Unbounded));
        let mut last_evaluated = after;
        loop {
            if run.exhaust_work() {
                break;
            }
            let Some(id) = ids.next() else { break };
            if !run.spend(1) {
                break;
            }
            let entity = self.entity(*id).expect("object index contains only live entities");
            let Some(item) = self.live_item(entity, None, run) else {
                if run.incomplete.is_some() {
                    break;
                }
                last_evaluated = Some(*id);
                if run.exhaust_work() {
                    break;
                }
                continue;
            };
            if !run.push(item) {
                let after = run.items.last().map(|item| item.id()).expect("a full result page contains an ID");
                return Some(run.cursor(QueryCursorPhase::Live, Some(after)));
            }
            last_evaluated = Some(*id);
            if run.exhaust_work() {
                break;
            }
        }
        let after = last_evaluated.filter(|_| run.incomplete == Some(QueryIncompleteReason::WorkLimit))?;
        Some(run.cursor(QueryCursorPhase::Live, Some(after)))
    }

    fn query_spatial_live(&self, run: &mut QueryRun) -> bool {
        let sphere = run.query.sphere.expect("spatial query has a sphere");
        let cells = Cell::range(sphere.center, sphere.radius).expect("query construction validated the sphere bounds");
        let candidates = self.live_ids(run.query);
        if cells.contains_more_than(candidates.map_or(0, BTreeSet::len)) {
            let Some(ids) = candidates else { return true };
            return self.query_spatial_entity_ids(ids.iter().copied(), sphere, run);
        }
        self.query_spatial_cells(sphere, cells, run);
        if run.incomplete.is_none() && self.spatial.has_dirty_scope_in(run.query.region) {
            self.query_dirty_live(sphere, run);
        }
        false
    }

    fn live_ids(&self, query: WorldQuery) -> Option<&BTreeSet<Uuid>> {
        let ids = match query.region {
            Some(region) => self.store.region_ids(region)?,
            None => self.store.ids(),
        };
        Some(if query.kind == Some(ValueFilter::Is(Kind::Avatar)) && self.store.avatar_ids().len() < ids.len() {
            self.store.avatar_ids()
        } else {
            ids
        })
    }

    fn query_spatial_entity_ids(&self, ids: impl Iterator<Item = Uuid>, sphere: Sphere, run: &mut QueryRun) -> bool {
        for id in ids {
            if !run.spend(1) {
                return false;
            }
            let entity = self.entity(id).expect("object ordering index contains only live entities");
            if !self.collect_spatial(entity, sphere, run) {
                return false;
            }
        }
        true
    }

    fn query_spatial_cells(&self, sphere: Sphere, mut cells: CellRange, run: &mut QueryRun) {
        let track_seen = self.spatial.has_dirty_scope_in(run.query.region);
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
                if track_seen {
                    run.seen_entities.insert(*entity);
                }
                if !self.collect_spatial(*entity, sphere, run) {
                    return;
                }
            }
        }
    }

    fn query_dirty_live(&self, sphere: Sphere, run: &mut QueryRun) {
        if let Some(region) = run.query.region {
            self.query_dirty_region(region, sphere, run);
            return;
        }
        let mut regions = self.spatial.dirty_scope_regions();
        loop {
            if run.exhaust_work() {
                return;
            }
            let Some(region) = regions.next() else { break };
            if !self.query_dirty_region(region, sphere, run) {
                return;
            }
        }
    }

    fn query_dirty_region(&self, region: Region, sphere: Sphere, run: &mut QueryRun) -> bool {
        if run.exhaust_work() || !run.spend(1) {
            return false;
        }
        if !self.spatial.is_region_dirty(region) {
            for root in self.spatial.dirty_roots_in(region) {
                let mut scan = SubtreeScan::default();
                loop {
                    if run.exhaust_work() {
                        return false;
                    }
                    let Some(entity) = scan.next(self, root) else { break };
                    if !run.spend(1) || !self.collect_dirty(entity, sphere, run) {
                        return false;
                    }
                }
            }
            return true;
        }
        let Some(ids) = self.store.region_ids(region) else { return true };
        let mut ids = ids.iter();
        loop {
            if run.exhaust_work() {
                return false;
            }
            let Some(id) = ids.next() else { break };
            if !run.spend(1) {
                return false;
            }
            let entity = self.entity(*id).expect("region index contains only live objects");
            if !self.collect_dirty(entity, sphere, run) {
                return false;
            }
        }
        true
    }

    fn collect_dirty(&self, entity: Entity, sphere: Sphere, run: &mut QueryRun) -> bool {
        !run.seen_entities.insert(entity) || self.collect_spatial(entity, sphere, run)
    }

    fn collect_spatial(&self, entity: Entity, sphere: Sphere, run: &mut QueryRun) -> bool {
        if let Some(item) = self.live_item(entity, Some(sphere), run)
            && !run.push(item)
        {
            return false;
        }
        run.incomplete.is_none()
    }

    fn query_unresolved(&self, run: &mut QueryRun) {
        match run.query.region {
            Some(region) => self.query_unresolved_ids(self.spatial.unresolved_ids_in(region), run),
            None => self.query_unresolved_ids(self.spatial.unresolved_ids(), run),
        }
    }

    fn query_unresolved_ids(&self, mut unresolved: impl Iterator<Item = (Region, Uuid, Entity)>, run: &mut QueryRun) {
        loop {
            if run.remaining == 0 {
                run.unresolved_complete = false;
                return;
            }
            let Some((region, _, entity)) = unresolved.next() else { break };
            if !run.spend_unresolved() {
                return;
            }
            if run.query.region.is_some_and(|filter| filter != region) || self.spatial_is_dirty(entity) {
                continue;
            }
            let unresolved_id = run.object(self, entity).and_then(|object| {
                let (properties, provenance) = object.properties_data();
                live_matches_without_pose(&object, run.query, properties, provenance).then(|| object.id())
            });
            if let Some(id) = unresolved_id {
                run.unresolved(id);
            }
            if run.remaining == 0 {
                run.unresolved_complete = false;
                return;
            }
        }
    }

    fn query_recent(&self, after: Option<Uuid>, now: Instant, run: &mut QueryRun) -> Option<WorldQueryCursor> {
        let query = run.query;
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
            if !recent_matches(entry.object, query) {
                if query.sphere.is_some() && entry.object.last_pose.is_none() && recent_matches_without_radius(entry.object, query) {
                    run.unresolved(id);
                }
                last_evaluated = Some(id);
                continue;
            }
            if !run.push(WorldQueryItem::Recent(entry.object)) {
                return (!query.is_spatial()).then(|| run.cursor(QueryCursorPhase::Recent, last_evaluated));
            }
            last_evaluated = Some(id);
            if run.exhaust_work() {
                break;
            }
        }
        (run.incomplete == Some(QueryIncompleteReason::WorkLimit) && !query.is_spatial())
            .then(|| run.cursor(QueryCursorPhase::Recent, last_evaluated))
    }

    fn live_item(&self, entity: Entity, sphere: Option<Sphere>, run: &mut QueryRun) -> Option<WorldQueryItem> {
        let object = run.object(self, entity)?;
        let id = object.id();
        let (properties, provenance) = object.properties_data();
        if !live_matches_without_pose(&object, run.query, properties, provenance) {
            return None;
        }

        let address = object.address();

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
}

fn live_matches_without_pose(object: &Object<'_>, query: WorldQuery, properties: Properties, provenance: PropertiesProvenance) -> bool {
    matches_query_fields(query, object.address().region(), object.kind(), object.attachment_point(), properties, provenance)
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
mod tests;
