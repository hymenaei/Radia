use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Instant;

use base::UUID;
use math::DVec3;

use super::{Entity, Parent, PoseState, Region, World, pose::MAX_PARENT_EDGES};

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

#[derive(Clone, Copy)]
struct DirtyRoot {
    region: Region,
    scope: DirtyScope,
}

#[derive(Clone, Copy, Default)]
struct DirtyScope {
    cursor: Option<UUID>,
    generation: u64,
    scan_generation: u64,
}

impl DirtyScope {
    fn changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    fn finish_pass(&mut self, cursor: Option<UUID>, complete: bool) -> bool {
        if !complete {
            self.cursor = cursor;
            return false;
        }
        if self.generation != self.scan_generation {
            self.cursor = None;
            self.scan_generation = self.generation;
            false
        } else {
            true
        }
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

    fn mark_entity(
        &mut self,
        entity: Entity,
        region: Region,
        covered_by_root: bool,
        covered_roots: &[Option<Entity>],
        covering_roots: &[Option<Entity>],
    ) {
        if self.dirty_roots.get(&entity).is_some_and(|dirty| dirty.region != region) {
            self.remove_dirty_root(entity);
        }
        if let Some(scope) = self.dirty_regions.get_mut(&region) {
            scope.changed();
            return;
        }

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
            self.promote_region(region);
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

    fn promote_region(&mut self, region: Region) {
        self.clear_dirty_roots(region);
        if self.dirty_regions.contains_key(&region) {
            self.dirty_regions.get_mut(&region).expect("dirty region remains indexed").changed();
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

    fn refresh(&mut self, entity: Entity, id: UUID, region: Region, pose: Result<PoseState, super::PoseError>) {
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

    fn is_region_dirty(&self, region: Region) -> bool {
        self.dirty_regions.contains_key(&region)
    }

    fn next_task(&self) -> Option<DirtyTask> {
        let region = self
            .last_dirty_region
            .and_then(|last| self.dirty_regions.range((std::ops::Bound::Excluded(last), std::ops::Bound::Unbounded)).next())
            .or_else(|| self.dirty_regions.first_key_value())
            .map(|(region, _)| DirtyTask::Region(*region));
        let root = self
            .last_dirty_root_region
            .and_then(|last| self.dirty_roots_by_region.range((std::ops::Bound::Excluded(last), std::ops::Bound::Unbounded)).next())
            .or_else(|| self.dirty_roots_by_region.first_key_value())
            .and_then(|(_, roots)| roots.front().copied())
            .map(DirtyTask::Root);
        match (self.prefer_region, region, root) {
            (true, Some(task), _) | (false, _, Some(task)) => Some(task),
            (_, Some(task), None) | (_, None, Some(task)) => Some(task),
            (_, None, None) => None,
        }
    }

    fn cursor(&self, task: DirtyTask) -> Option<UUID> {
        match task {
            DirtyTask::Root(root) => self.dirty_roots.get(&root).and_then(|dirty| dirty.scope.cursor),
            DirtyTask::Region(region) => self.dirty_regions.get(&region).and_then(|scope| scope.cursor),
        }
    }

    fn finish_task_batch(&mut self, task: DirtyTask, cursor: Option<UUID>, complete: bool) {
        match task {
            DirtyTask::Root(root) => {
                let region = self.dirty_root_region(root);
                if complete {
                    let done = self.dirty_roots.get_mut(&root).is_none_or(|dirty| dirty.scope.finish_pass(None, true));
                    if done {
                        self.remove_dirty_root(root);
                    } else if let Some(queue) = region.and_then(|region| self.dirty_roots_by_region.get_mut(&region))
                        && queue.front() == Some(&root)
                    {
                        queue.rotate_left(1);
                    }
                } else if let Some(dirty) = self.dirty_roots.get_mut(&root) {
                    dirty.scope.finish_pass(cursor, false);
                    if let Some(queue) = region.and_then(|region| self.dirty_roots_by_region.get_mut(&region))
                        && queue.front() == Some(&root)
                    {
                        queue.rotate_left(1);
                    }
                }
                if let Some(region) = region {
                    self.last_dirty_root_region = Some(region);
                }
                self.prefer_region = true;
            }
            DirtyTask::Region(region) => {
                if complete {
                    let done = self.dirty_regions.get_mut(&region).is_none_or(|scope| scope.finish_pass(None, true));
                    if done {
                        self.dirty_regions.remove(&region);
                    }
                } else if self.dirty_regions.contains_key(&region) {
                    self.dirty_regions.get_mut(&region).expect("dirty region remains indexed").finish_pass(cursor, false);
                }
                self.last_dirty_region = Some(region);
                self.prefer_region = false;
            }
        }
    }

    fn dirty_scope_count(&self) -> usize {
        self.dirty_regions.len().saturating_add(self.dirty_roots.len())
    }
}

impl World {
    pub(super) fn update_spatial_state(&mut self, changes: &[super::Change]) {
        use super::Change::{
            EntityAdded, EntityRebound, EntityRemoved, EntityTransferred, EntityUpdated, RegionRetired, SubtreeInvalidated,
        };

        for change in changes {
            match *change {
                EntityAdded { entity, .. } | EntityRebound { entity, .. } | EntityTransferred { entity, .. } => {
                    self.mark_spatial_entity(entity);
                }
                EntityUpdated { entity, classes, .. }
                    if classes.contains(super::ChangeClasses::TRANSFORM_MOTION) || classes.contains(super::ChangeClasses::RELATIONSHIP) =>
                {
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
        self.spatial.mark_entity(entity, region, covered_by_root, &covered_roots[..covered_count], &covering_roots[..covering_count]);
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
        if ancestors.truncated && self.spatial.dirty_roots_in(region).next().is_some() {
            return true;
        }
        ancestors.iter().any(|ancestor| self.spatial.dirty_roots.contains_key(ancestor))
    }

    pub(super) fn refresh_spatial_entity(&mut self, entity: Entity) {
        let Some((id, region)) = self.object(entity).map(|object| (object.identity().id(), object.address().region())) else { return };
        let pose = self.resolve_pose(entity);
        self.spatial.refresh(entity, id, region, pose);
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
        let mut ids = Vec::with_capacity(work_limit);
        while work_used < work_limit && scopes_visited < work_limit {
            let Some(task) = self.spatial.next_task() else { break };
            scopes_visited += 1;
            ids.clear();
            let after = self.spatial.cursor(task);
            let root_region = match task {
                DirtyTask::Root(root) => self.spatial.dirty_root_region(root),
                DirtyTask::Region(region) => Some(region),
            };
            let candidates = match task {
                DirtyTask::Root(_) | DirtyTask::Region(_) => root_region.and_then(|region| self.region_entities.get(&region)),
            };
            let remaining = work_limit - work_used;
            let has_more = if let Some(candidates) = candidates {
                let lower = after.map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded);
                ids.extend(candidates.range((lower, std::ops::Bound::Unbounded)).take(remaining).copied());
                ids.len() == remaining
            } else {
                false
            };
            let cursor = ids.last().copied().or(after);
            work_used = work_used.saturating_add(ids.len());

            for id in &ids {
                let Some(entity) = self.entities.get(id).copied() else { continue };
                let region = self.object(entity).expect("UUID index contains only live entities").address().region();
                let refresh = match task {
                    DirtyTask::Root(root) => {
                        if root_region != Some(region) {
                            false
                        } else {
                            let ancestors = self.ancestor_chain(entity);
                            ancestors.truncated || ancestors.contains(root)
                        }
                    }
                    DirtyTask::Region(target) => region == target,
                };
                if refresh {
                    self.refresh_spatial_entity(entity);
                }
            }
            self.spatial.finish_task_batch(task, cursor, !has_more);
            if !has_more {
                continue;
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
    use crate::{ObjectObservation, OrderEvidence::Ordered, Pose, UpdateSource, WorldLimits};

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
                attachment_point: None,
                shape: None,
                appearance: None,
                surface_appearance: None,
                render_materials: None,
            })
            .expect("add object");

        let pressure = world.maintain_query_state(1).expect("bounded maintenance");
        assert_eq!(pressure.dirty_scopes, 1);
        assert!(pressure.has_dirty_pressure);

        let repaired = world.maintain_query_state(1).expect("finish spatial repair");
        assert_eq!(repaired.dirty_scopes, 0);
        assert!(!repaired.has_dirty_pressure);
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
