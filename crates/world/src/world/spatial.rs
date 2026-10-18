//! Derived spatial indexes with explicit, bounded maintenance.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ops::Bound::{Excluded, Unbounded};
use std::time::Instant;

#[cfg(feature = "invariants")]
use std::collections::HashSet;

use crate::PoseError;
use base::UUID;
use math::DVec3;

use super::{World, WorldChangeClasses, pose::MAX_PARENT_EDGES};
use crate::{Entity, Parent, PoseState, Region};

const CELL_SIZE: f64 = 32.0;
const MAX_DIRTY_ROOTS_PER_REGION: usize = 48;

pub const MAX_QUERY_STATE_MAINTENANCE_WORK: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct Cell {
    x: i64,
    y: i64,
    z: i64,
}

impl Cell {
    pub(super) fn for_position(position: DVec3) -> Option<Self> {
        Some(Self { x: cell_coordinate(position.x)?, y: cell_coordinate(position.y)?, z: cell_coordinate(position.z)? })
    }

    pub(super) fn range(center: DVec3, radius: f64) -> Option<CellRange> {
        let extent = DVec3::splat(radius);
        let min = Self::for_position(center - extent)?;
        let max = Self::for_position(center + extent)?;
        Some(CellRange { min, max, next: Some(min) })
    }
}

fn cell_coordinate(value: f64) -> Option<i64> {
    let cell = (value / CELL_SIZE).floor();
    const I64_UPPER_EXCLUSIVE: f64 = 9_223_372_036_854_775_808.0;
    if !cell.is_finite() || cell < i64::MIN as f64 || cell >= I64_UPPER_EXCLUSIVE {
        return None;
    }
    Some(cell as i64)
}

pub(super) struct CellRange {
    min: Cell,
    max: Cell,
    next: Option<Cell>,
}

impl CellRange {
    pub(super) fn contains_more_than(&self, limit: usize) -> bool {
        let limit = limit as u128;
        let dimension = |min: i64, max: i64| (i128::from(max) - i128::from(min) + 1) as u128;
        let mut count = 1_u128;
        for cells in [dimension(self.min.x, self.max.x), dimension(self.min.y, self.max.y), dimension(self.min.z, self.max.z)] {
            if count > limit / cells {
                return true;
            }
            count *= cells;
        }
        count > limit
    }
}

impl Iterator for CellRange {
    type Item = Cell;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.next?;
        self.next = if current.x < self.max.x {
            Some(Cell { x: current.x + 1, ..current })
        } else if current.y < self.max.y {
            Some(Cell { x: self.min.x, y: current.y + 1, z: current.z })
        } else if current.z < self.max.z {
            Some(Cell { x: self.min.x, y: self.min.y, z: current.z + 1 })
        } else {
            None
        };
        Some(current)
    }
}

#[derive(Default)]
pub(super) struct SpatialIndex {
    cells: HashMap<Cell, BTreeMap<UUID, Entity>>,
    cell_by_entity: HashMap<Entity, (UUID, Cell)>,
    unresolved: BTreeMap<Region, BTreeMap<UUID, Entity>>,
    dirty_roots: HashMap<Entity, DirtyRoot>,
    dirty_roots_by_region: BTreeMap<Region, VecDeque<Entity>>,
    last_dirty_root_region: Option<Region>,
    dirty_regions: BTreeMap<Region, DirtyScope>,
    last_dirty_region: Option<Region>,
    prefer_region: bool,
    unresolved_by_entity: HashMap<Entity, (UUID, Region)>,
}

struct DirtyRoot {
    region: Region,
    scope: DirtyScope<SubtreeScan>,
}

#[derive(Default)]
struct DirtyScope<C = Option<UUID>> {
    cursor: C,
    generation: u64,
    scan_generation: u64,
}

impl<C: Default> DirtyScope<C> {
    fn changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    fn finish_pass(&mut self, cursor: C, complete: bool) -> bool {
        if !complete {
            self.cursor = cursor;
            return false;
        }
        if self.generation != self.scan_generation {
            self.cursor = C::default();
            self.scan_generation = self.generation;
            false
        } else {
            true
        }
    }
}

#[derive(Default)]
pub(super) struct SubtreeScan {
    stack: Vec<SubtreeFrame>,
    started: bool,
}

struct SubtreeFrame {
    entity: Entity,
    after_child: Option<UUID>,
}

impl SubtreeScan {
    pub(super) fn next(&mut self, world: &World, root: Entity) -> Option<Entity> {
        if !self.started {
            self.started = true;
            self.stack.push(SubtreeFrame { entity: root, after_child: None });
            return Some(root);
        }
        while !self.stack.is_empty() {
            let depth = self.stack.len() - 1;
            let frame = self.stack.last_mut().expect("traversal has a current frame");
            let next = if depth < MAX_PARENT_EDGES {
                world
                    .children
                    .get(&frame.entity)
                    .and_then(|children| children.range((frame.after_child.map_or(Unbounded, Excluded), Unbounded)).next())
            } else {
                None
            };
            if let Some((id, child)) = next {
                frame.after_child = Some(*id);
                self.stack.push(SubtreeFrame { entity: *child, after_child: None });
                return Some(*child);
            } else {
                self.stack.pop();
            }
        }
        None
    }
}

#[derive(Clone, Copy, Debug)]
enum DirtyTask {
    Root(Entity),
    Region(Region),
}

/// Progress made by one bounded query-state maintenance step.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueryStateMaintenance {
    /// Spatial candidates examined or expired recent-object records removed.
    pub work_used: usize,
    /// Dirty spatial scopes that still need repair.
    pub dirty_scopes: usize,
    /// Whether dirty work remains and should be scheduled again promptly.
    pub has_dirty_pressure: bool,
}

impl SpatialIndex {
    pub(super) fn candidate_ids(&self, cell: Cell) -> Option<&BTreeMap<UUID, Entity>> {
        self.cells.get(&cell)
    }

    pub(super) fn unresolved_ids(&self) -> impl Iterator<Item = (Region, UUID, Entity)> + '_ {
        self.unresolved.iter().flat_map(|(region, ids)| ids.iter().map(|(id, entity)| (*region, *id, *entity)))
    }

    pub(super) fn unresolved_ids_in(&self, region: Region) -> impl Iterator<Item = (Region, UUID, Entity)> + '_ {
        self.unresolved.get(&region).into_iter().flat_map(move |ids| ids.iter().map(move |(id, entity)| (region, *id, *entity)))
    }

    pub(super) fn dirty_roots_in(&self, region: Region) -> impl Iterator<Item = Entity> + '_ {
        self.dirty_roots_by_region.get(&region).into_iter().flat_map(|roots| roots.iter().copied())
    }

    pub(super) fn dirty_scope_regions(&self) -> impl Iterator<Item = Region> + '_ {
        self.dirty_regions
            .keys()
            .copied()
            .chain(self.dirty_roots_by_region.keys().filter(|region| !self.dirty_regions.contains_key(region)).copied())
    }

    fn dirty_root_region(&self, root: Entity) -> Option<Region> {
        self.dirty_roots.get(&root).map(|dirty| dirty.region)
    }

    fn remove_dirty_root(&mut self, root: Entity) {
        let Some(dirty) = self.dirty_roots.remove(&root) else { return };
        if let Some(queue) = self.dirty_roots_by_region.get_mut(&dirty.region) {
            queue.retain(|queued| *queued != root);
            if queue.is_empty() {
                self.dirty_roots_by_region.remove(&dirty.region);
            }
        }
    }

    fn clear_dirty_roots(&mut self, region: Region) {
        let Some(roots) = self.dirty_roots_by_region.remove(&region) else { return };
        for root in roots {
            self.dirty_roots.remove(&root);
        }
    }

    fn mark_subtree(
        &mut self,
        entity: Entity,
        region: Region,
        covered_by_root: bool,
        covered_roots: &[Option<Entity>],
        covering_roots: &[Option<Entity>],
    ) {
        for root in covered_roots.iter().flatten() {
            self.remove_dirty_root(*root);
        }
        for root in covering_roots.iter().flatten() {
            if let Some(dirty) = self.dirty_roots.get_mut(root) {
                dirty.scope.changed();
            }
        }
        if covered_by_root {
            return;
        }

        let roots_in_region = self.dirty_roots_by_region.get(&region).map_or(0, VecDeque::len);
        if roots_in_region >= MAX_DIRTY_ROOTS_PER_REGION {
            self.mark_region(region);
            return;
        }
        self.dirty_roots
            .insert(entity, DirtyRoot { region, scope: DirtyScope { generation: 1, scan_generation: 1, ..DirtyScope::default() } });
        self.dirty_roots_by_region.entry(region).or_default().push_back(entity);
    }

    fn mark_region(&mut self, region: Region) {
        self.clear_dirty_roots(region);
        if let Some(scope) = self.dirty_regions.get_mut(&region) {
            scope.changed();
            return;
        }
        self.dirty_regions.insert(region, DirtyScope { generation: 1, scan_generation: 1, ..DirtyScope::default() });
    }

    fn remove(&mut self, entity: Entity) {
        self.remove_cached_position(entity);
        self.remove_dirty_root(entity);
    }

    fn clear_region(&mut self, region: Region) {
        self.dirty_regions.remove(&region);
        self.clear_dirty_roots(region);
        if let Some(entities) = self.unresolved.remove(&region) {
            for entity in entities.into_values() {
                self.unresolved_by_entity.remove(&entity);
            }
        }
    }

    fn refresh(&mut self, entity: Entity, id: UUID, region: Region, pose: Result<PoseState, PoseError>) {
        self.remove_cached_position(entity);
        match pose {
            Ok(PoseState::Resolved(pose)) if pose.position.is_finite() => {
                if let Some(cell) = Cell::for_position(pose.position) {
                    self.cells.entry(cell).or_default().insert(id, entity);
                    self.cell_by_entity.insert(entity, (id, cell));
                }
            }
            Ok(PoseState::Resolved(_)) | Ok(PoseState::Unresolved(_)) | Ok(PoseState::Provisional { .. }) | Err(_) => {
                self.unresolved.entry(region).or_default().insert(id, entity);
                self.unresolved_by_entity.insert(entity, (id, region));
            }
        }
    }

    fn remove_cached_position(&mut self, entity: Entity) {
        if let Some((id, cell)) = self.cell_by_entity.remove(&entity) {
            let cells = self.cells.get_mut(&cell).expect("reverse spatial index points to an occupied cell");
            assert!(cells.remove(&id).is_some(), "reverse spatial index points to a missing entity");
            if cells.is_empty() {
                self.cells.remove(&cell);
            }
        }
        if let Some((id, region)) = self.unresolved_by_entity.remove(&entity)
            && let Some(entries) = self.unresolved.get_mut(&region)
        {
            entries.remove(&id);
            if entries.is_empty() {
                self.unresolved.remove(&region);
            }
        }
    }

    fn has_dirty_scope(&self) -> bool {
        !self.dirty_regions.is_empty() || !self.dirty_roots.is_empty()
    }

    pub(super) fn has_dirty_scope_in(&self, region: Option<Region>) -> bool {
        match region {
            Some(region) => self.dirty_regions.contains_key(&region) || self.dirty_roots_by_region.contains_key(&region),
            None => self.has_dirty_scope(),
        }
    }

    pub(super) fn is_region_dirty(&self, region: Region) -> bool {
        self.dirty_regions.contains_key(&region)
    }

    fn next_task(&self) -> Option<DirtyTask> {
        let region = self
            .last_dirty_region
            .and_then(|last| self.dirty_regions.range((Excluded(last), Unbounded)).next())
            .or_else(|| self.dirty_regions.first_key_value())
            .map(|(region, _)| DirtyTask::Region(*region));
        let root = self
            .last_dirty_root_region
            .and_then(|last| self.dirty_roots_by_region.range((Excluded(last), Unbounded)).next())
            .or_else(|| self.dirty_roots_by_region.first_key_value())
            .and_then(|(_, roots)| roots.front().copied())
            .map(DirtyTask::Root);
        match (self.prefer_region, region, root) {
            (true, Some(task), _) | (false, _, Some(task)) => Some(task),
            (_, Some(task), None) | (_, None, Some(task)) => Some(task),
            (_, None, None) => None,
        }
    }

    fn finish_root(&mut self, root: Entity) {
        if let Some(region) = self.dirty_root_region(root) {
            self.last_dirty_root_region = Some(region);
        }
        self.remove_dirty_root(root);
        self.prefer_region = true;
    }

    fn finish_subtree_batch(&mut self, root: Entity, cursor: SubtreeScan, complete: bool) {
        let region = self.dirty_root_region(root);
        let done = self.dirty_roots.get_mut(&root).is_none_or(|dirty| dirty.scope.finish_pass(cursor, complete));
        if done {
            self.remove_dirty_root(root);
        } else if let Some(queue) = region.and_then(|region| self.dirty_roots_by_region.get_mut(&region))
            && queue.front() == Some(&root)
        {
            queue.rotate_left(1);
        }
        if let Some(region) = region {
            self.last_dirty_root_region = Some(region);
        }
        self.prefer_region = true;
    }

    fn finish_region_batch(&mut self, region: Region, cursor: Option<UUID>, complete: bool) {
        let done = self.dirty_regions.get_mut(&region).is_none_or(|scope| scope.finish_pass(cursor, complete));
        if done {
            self.dirty_regions.remove(&region);
        }
        self.last_dirty_region = Some(region);
        self.prefer_region = false;
    }

    fn dirty_scope_count(&self) -> usize {
        self.dirty_regions.len().saturating_add(self.dirty_roots.len())
    }

    #[cfg(feature = "invariants")]
    pub(super) fn invariant_error(&self, world: &World) -> Option<&'static str> {
        let mut cached = HashSet::new();
        for (cell, ids) in &self.cells {
            for (id, entity) in ids {
                if !cached.insert(*entity)
                    || world.object(*entity).is_none_or(|object| object.id() != *id)
                    || self.cell_by_entity.get(entity) != Some(&(*id, *cell))
                {
                    return Some("cell index and reverse entity index disagree");
                }
            }
        }
        if self.cell_by_entity.iter().any(|(entity, (id, cell))| !self.cells.get(cell).is_some_and(|ids| ids.get(id) == Some(entity))) {
            return Some("reverse cell index points to a missing entity");
        }

        for (region, ids) in &self.unresolved {
            for (id, entity) in ids {
                if !cached.insert(*entity)
                    || world.object(*entity).is_none_or(|object| {
                        object.id() != *id || (!world.spatial_is_dirty(*entity) && object.address().region() != *region)
                    })
                    || self.unresolved_by_entity.get(entity) != Some(&(*id, *region))
                {
                    return Some("unresolved spatial index and reverse entity index disagree");
                }
            }
        }
        if self
            .unresolved_by_entity
            .iter()
            .any(|(entity, (id, region))| !self.unresolved.get(region).is_some_and(|ids| ids.get(id) == Some(entity)))
        {
            return Some("reverse unresolved index points to a missing entity");
        }

        for object in world.objects().iter() {
            let entity = object.entity();
            if world.spatial_is_dirty(entity) {
                continue;
            }
            match object.world_pose() {
                Ok(PoseState::Resolved(pose)) if pose.position.is_finite() => {
                    if let Some(cell) = Cell::for_position(pose.position) {
                        if self.cell_by_entity.get(&entity) != Some(&(object.id(), cell)) {
                            return Some("clean spatial position does not match the current pose");
                        }
                    } else if cached.contains(&entity) {
                        return Some("unrepresentable position is present in the spatial index");
                    }
                }
                _ => {
                    if self.unresolved_by_entity.get(&entity) != Some(&(object.id(), object.address().region())) {
                        return Some("unresolved spatial index does not match the current pose");
                    }
                }
            }
        }
        if self.dirty_regions.keys().any(|region| world.region(*region).is_none()) {
            return Some("dirty spatial scope references an inactive region");
        }

        let mut queued_roots = HashSet::with_capacity(self.dirty_roots.len());
        for (region, roots) in &self.dirty_roots_by_region {
            if roots.is_empty() || self.dirty_regions.contains_key(region) {
                return Some("dirty root queue overlaps a whole-region scope or is empty");
            }
            for root in roots {
                if !queued_roots.insert(*root)
                    || self.dirty_roots.get(root).is_none_or(|dirty| dirty.region != *region)
                    || world.object(*root).is_none_or(|object| object.address().region() != *region)
                {
                    return Some("dirty root queue and root index disagree");
                }
            }
        }
        if queued_roots.len() != self.dirty_roots.len()
            || !self.dirty_roots.iter().all(|(root, dirty)| {
                self.dirty_roots_by_region.get(&dirty.region).is_some_and(|roots| roots.contains(root))
                    && world.object(*root).is_some_and(|object| object.address().region() == dirty.region)
            })
        {
            return Some("dirty root index points to a missing queue entry");
        }
        None
    }
}

impl World {
    pub(super) fn update_spatial_state(&mut self, changes: &[super::WorldChange]) {
        use super::WorldChange::{
            EntityAdded, EntityRebound, EntityRemoved, EntityTransferred, EntityUpdated, RegionRetired, SubtreeInvalidated,
        };

        for change in changes {
            match *change {
                EntityAdded { entity, .. } | EntityRebound { entity, .. } | EntityTransferred { entity, .. } => {
                    self.mark_spatial_entity(entity);
                }
                EntityUpdated { entity, classes, .. } if classes.contains(WorldChangeClasses::POSE) => {
                    self.mark_spatial_entity(entity);
                }
                SubtreeInvalidated { entity, .. } => self.mark_spatial_entity(entity),
                EntityRemoved { entity, .. } => self.spatial.remove(entity),
                RegionRetired { region, .. } => self.spatial.clear_region(region),
                _ => {}
            }
        }
    }

    pub(super) fn mark_spatial_region(&mut self, region: Region) {
        self.spatial.mark_region(region);
    }

    fn mark_spatial_entity(&mut self, entity: Entity) {
        let Some(region) = self.object(entity).map(|object| object.address().region()) else { return };
        if self.spatial.dirty_root_region(entity).is_some_and(|previous| previous != region) {
            self.spatial.remove_dirty_root(entity);
        }
        if let Some(scope) = self.spatial.dirty_regions.get_mut(&region) {
            scope.changed();
            return;
        }
        let ancestors = self.ancestor_chain(entity);
        if ancestors.truncated {
            self.spatial.mark_region(region);
            return;
        }
        let mut covered_by_root = false;
        let mut covered_roots = [None; MAX_DIRTY_ROOTS_PER_REGION];
        let mut covered_count = 0;
        let mut covering_roots = [None; MAX_DIRTY_ROOTS_PER_REGION];
        let mut covering_count = 0;
        for root in self.spatial.dirty_roots_in(region) {
            if ancestors.contains(root) {
                covered_by_root = true;
                if covering_count < covering_roots.len() {
                    covering_roots[covering_count] = Some(root);
                    covering_count += 1;
                }
            }
            if root != entity && self.ancestor_chain(root).contains(entity) && covered_count < covered_roots.len() {
                covered_roots[covered_count] = Some(root);
                covered_count += 1;
            }
        }
        self.spatial.mark_subtree(entity, region, covered_by_root, &covered_roots[..covered_count], &covering_roots[..covering_count]);
    }

    fn ancestor_chain(&self, mut entity: Entity) -> AncestorChain {
        let mut ancestors = AncestorChain::default();
        for _ in 0..=MAX_PARENT_EDGES {
            if ancestors.contains(entity) {
                break;
            }
            ancestors.push(entity);
            let Some(object) = self.object(entity) else { break };
            let Parent::Resolved(parent) = object.parent() else { break };
            if ancestors.len == ancestors.entities.len() {
                ancestors.truncated = true;
                break;
            }
            entity = parent;
        }
        ancestors
    }

    pub(super) fn spatial_is_dirty(&self, entity: Entity) -> bool {
        let Some(object) = self.object(entity) else { return false };
        let region = object.address().region();
        if self.spatial.is_region_dirty(region) {
            return true;
        }
        let ancestors = self.ancestor_chain(entity);
        ancestors.iter().any(|ancestor| self.spatial.dirty_roots.contains_key(ancestor))
    }

    pub(super) fn refresh_spatial_entity(&mut self, entity: Entity) {
        let Some((id, region)) = self.object(entity).map(|object| (object.id(), object.address().region())) else { return };
        let pose = self.resolve_pose(entity);
        self.spatial.refresh(entity, id, region, pose);
    }

    fn repair_subtree(&mut self, root: Entity, limit: usize) -> usize {
        if !self.children.contains_key(&root) {
            self.refresh_spatial_entity(root);
            self.spatial.finish_root(root);
            return 1;
        }
        let scope = &mut self.spatial.dirty_roots.get_mut(&root).expect("scheduled root exists").scope;
        if !scope.cursor.started {
            scope.scan_generation = scope.generation;
        }
        let mut scan = std::mem::take(&mut scope.cursor);
        let mut work = 0;
        let mut complete = false;
        while work < limit {
            let Some(entity) = scan.next(self, root) else {
                complete = true;
                break;
            };
            self.refresh_spatial_entity(entity);
            work += 1;
        }
        self.spatial.finish_subtree_batch(root, scan, complete);
        work
    }

    /// Repairs the spatial query index and removes expired recent-object metadata.
    ///
    /// The shared work limit counts examined spatial candidates and removed recent records. Each
    /// call also visits at most `work_limit` dirty scopes, including scopes with no candidates.
    pub fn maintain_query_state(&mut self, work_limit: usize) -> Result<QueryStateMaintenance, super::WorldReadError> {
        if work_limit == 0 || work_limit > MAX_QUERY_STATE_MAINTENANCE_WORK {
            return Err(super::WorldReadError::InvalidLimit);
        }

        let mut work_used = 0;
        let mut scopes_visited = 0;
        let mut ids = Vec::new();
        while work_used < work_limit && scopes_visited < work_limit {
            let Some(task) = self.spatial.next_task() else { break };
            scopes_visited += 1;
            let remaining = work_limit - work_used;
            match task {
                DirtyTask::Root(root) => work_used += self.repair_subtree(root, remaining),
                DirtyTask::Region(region) => {
                    ids.clear();
                    let scope = self.spatial.dirty_regions.get_mut(&region).expect("scheduled region exists");
                    if scope.cursor.is_none() {
                        scope.scan_generation = scope.generation;
                    }
                    let after = scope.cursor;
                    if let Some(candidates) = self.region_entities.get(&region) {
                        let lower = after.map_or(Unbounded, Excluded);
                        ids.extend(candidates.range((lower, Unbounded)).take(remaining).copied());
                    }
                    let complete = ids.len() < remaining;
                    let cursor = ids.last().copied().or(after);
                    work_used += ids.len();
                    for id in &ids {
                        let entity = self.entities[id];
                        self.refresh_spatial_entity(entity);
                    }
                    self.spatial.finish_region_batch(region, cursor, complete);
                }
            }
        }

        let expired = self.recent.purge_expired(Instant::now(), work_limit - work_used);
        work_used = work_used.saturating_add(expired);

        let dirty_scopes = self.spatial.dirty_scope_count();
        Ok(QueryStateMaintenance { work_used, dirty_scopes, has_dirty_pressure: dirty_scopes > 0 })
    }
}

#[derive(Clone, Copy)]
struct AncestorChain {
    entities: [Option<Entity>; MAX_PARENT_EDGES + 1],
    len: usize,
    truncated: bool,
}

impl Default for AncestorChain {
    fn default() -> Self {
        Self { entities: [None; MAX_PARENT_EDGES + 1], len: 0, truncated: false }
    }
}

impl AncestorChain {
    fn contains(self, entity: Entity) -> bool {
        self.entities[..self.len].contains(&Some(entity))
    }

    fn push(&mut self, entity: Entity) {
        if self.len < self.entities.len() {
            self.entities[self.len] = Some(entity);
            self.len += 1;
        }
    }

    fn iter(&self) -> impl Iterator<Item = &Entity> {
        self.entities[..self.len].iter().filter_map(Option::as_ref)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use math::{Quat, Vec3};

    use super::*;
    use crate::{FieldUpdate, ObjectObservation, OrderEvidence::Ordered, Pose, UpdateSource, WorldLimits};

    fn limits() -> WorldLimits {
        WorldLimits {
            max_region_registrations: 2,
            max_entities: 4,
            max_entities_per_region: 4,
            max_tombstones_per_region: 2,
            max_tombstones: 2,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(2),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(2),
            max_journal_entries: 4,
            max_journal_changes: 16,
            max_region_name_bytes: 16,
            max_recovery_targets: 2,
        }
    }

    #[test]
    fn spatial_repair_clears_pressure() {
        let region = Region::new(1);
        let mut world = World::new(limits());
        let _ = world.adopt_region(region).expect("adopt region");
        let _ = world
            .observe_object(ObjectObservation {
                source: UpdateSource::new(region, 1, Ordered(1), 0),
                id: UUID::from_u128(1),
                local_id: NonZeroU32::new(1).expect("nonzero local ID"),
                kind: crate::Kind::Object,
                parent: crate::ParentUpdate::Root,
                pose: Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) }),
                linear_motion: None,
                angular_velocity: None,
                scale: None,
                attachment_point: FieldUpdate::Keep,
                shape: FieldUpdate::Keep,
                appearance: FieldUpdate::Keep,
                surface_appearance: None,
                render_materials: None,
            })
            .expect("add object");

        let repaired = world.maintain_query_state(1).expect("bounded maintenance");
        assert_eq!(repaired.work_used, 1);
        assert_eq!(repaired.dirty_scopes, 0);
        assert!(!repaired.has_dirty_pressure);

        let idle = world.maintain_query_state(1).expect("maintenance after repair");
        assert_eq!(idle.work_used, 0);
        assert!(!idle.has_dirty_pressure);
    }

    #[test]
    fn cells_cover_radius_bounds() {
        let center = DVec3::new(31.0, 64.0, -1.0);
        let mut cells = Cell::range(center, 2.0).unwrap().collect::<Vec<_>>();

        assert!(cells.contains(&Cell { x: 0, y: 1, z: -1 }));
        assert!(cells.contains(&Cell { x: 1, y: 2, z: 0 }));
        cells.sort_by_key(|cell| (cell.z, cell.y, cell.x));
        cells.dedup();
        assert_eq!(cells.len(), 8);
    }

    #[test]
    fn cell_rejects_unrepresentable_positions() {
        assert!(Cell::for_position(DVec3::splat(f64::MAX)).is_none());
    }
}
