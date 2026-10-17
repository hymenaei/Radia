#[cfg(test)]
use std::num::NonZeroU32;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use base::UUID;
#[cfg(test)]
use math::Vec3;
use world::{
    MAX_OBJECT_PAGE_ITEMS, MAX_REGION_PAGE_ITEMS, MAX_WORLD_CHANGE_PAGE_ITEMS, ObjectDetailResult, ObjectPageCursor, Region,
    RegionPageCursor, RegionSummary, WorldChange, WorldChangeCursor, WorldReadError, WorldRevision,
};

use crate::session::{CoverageSnapshot, SessionReadBarrier, WorldReader, WorldReaderError};

use super::{ObjectDetails, ObjectRow, ObjectSelection, RegionLabel, WorldSnapshot, object_details, object_row};

const MAX_BOOTSTRAP_PAGES_PER_SYNC: usize = 16;
const MAX_BOOTSTRAP_RESTARTS: usize = 3;
const MAX_CHANGE_PAGES_PER_SYNC: usize = 4;
const MAX_SUMMARY_HYDRATIONS_PER_SYNC: usize = 64;
const MAX_REGION_HYDRATIONS_PER_SYNC: usize = 32;

pub(super) enum Refresh {
    Busy,
    Cancelled,
    Disconnected,
    Resync(WorldReader),
    ResyncFailed(WorldReader),
    Failed(WorldReader, String),
    Captured(WorldReader, WorldSnapshot, Duration),
}

pub(super) enum ResyncOutcome {
    Retry,
    Failed,
}

#[derive(Default)]
pub(super) struct WorldMirror {
    reader: Option<WorldReader>,
    selection_invalidated: bool,
    resync_restarts: usize,
    change_cursor: Option<WorldChangeCursor>,
    object_cursor: Option<ObjectPageCursor>,
    region_cursor: Option<RegionPageCursor>,
    objects_done: bool,
    regions_done: bool,
    bootstrapping: bool,
    has_more_changes: bool,
    revision: Option<WorldRevision>,
    journal_revision: Option<WorldRevision>,
    object_count: usize,
    coverage: Option<CoverageSnapshot>,
    objects: BTreeMap<UUID, ObjectRow>,
    regions: BTreeMap<Region, RegionSummary>,
    pending_objects: BTreeMap<UUID, world::Entity>,
    pending_regions: BTreeSet<Region>,
    selection: Option<ObjectSelection>,
    details: Option<ObjectDetails>,
    details_error: Option<String>,
    details_dirty: bool,
    snapshot: WorldSnapshot,
    snapshot_dirty: bool,
}

impl WorldMirror {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn is_for_different_session(&self, reader: &WorldReader) -> bool {
        self.reader.as_ref().is_some_and(|current| !current.same_session(reader))
    }

    pub(super) fn take_selection_invalidated(&mut self) -> bool {
        std::mem::take(&mut self.selection_invalidated)
    }

    fn restart_after_history_loss(&mut self) -> bool {
        if self.resync_restarts == MAX_BOOTSTRAP_RESTARTS {
            return false;
        }
        let reader = self.reader.clone();
        let selection_invalidated = self.selection_invalidated;
        let resync_restarts = self.resync_restarts + 1;
        *self = Self::default();
        self.reader = reader;
        self.selection_invalidated = selection_invalidated;
        self.resync_restarts = resync_restarts;
        true
    }

    fn begin(&mut self, reader: &WorldReader, barrier: SessionReadBarrier) {
        self.reader = Some(reader.clone());
        self.change_cursor = Some(barrier.changes);
        self.object_count = barrier.object_count;
        self.revision = Some(barrier.revision);
        self.journal_revision = Some(barrier.revision);
        self.bootstrapping = true;
        self.details_dirty = true;
        self.details_error = None;
        self.snapshot_dirty = true;
    }

    fn is_current(&self, revision_hint: WorldRevision) -> bool {
        !self.bootstrapping
            && !self.has_more_changes
            && self.pending_objects.is_empty()
            && self.pending_regions.is_empty()
            && !self.details_dirty
            && self.journal_revision == Some(revision_hint)
    }

    pub(super) fn has_more_work(&self) -> bool {
        self.bootstrapping
            || self.has_more_changes
            || !self.pending_objects.is_empty()
            || !self.pending_regions.is_empty()
            || self.details_dirty
    }

    fn apply_change(&mut self, change: WorldChange) {
        match change {
            WorldChange::EntityAdded { id, entity, .. } => {
                self.object_count += 1;
                self.pending_objects.insert(id, entity);
                if self.selection.is_some_and(|selection| selection.id == id && selection.entity == entity) {
                    self.details_dirty = true;
                }
            }
            WorldChange::EntityRebound { id, entity, .. } | WorldChange::EntityTransferred { id, entity, .. } => {
                self.pending_objects.insert(id, entity);
                if self.selection.is_some_and(|selection| selection.id == id && selection.entity == entity) {
                    self.details_dirty = true;
                }
            }
            WorldChange::EntityUpdated { id, entity, .. } => {
                if self.selection.is_some_and(|selection| selection.id == id && selection.entity == entity) {
                    self.details_dirty = true;
                }
            }
            WorldChange::EntityRemoved { id, entity, .. } => {
                self.object_count -= 1;
                if self.objects.get(&id).is_some_and(|row| row.selection.entity == entity) {
                    self.objects.remove(&id);
                    self.snapshot_dirty = true;
                }
                if self.pending_objects.get(&id).is_some_and(|pending| *pending == entity) {
                    self.pending_objects.remove(&id);
                }
                if self.selection.is_some_and(|selection| selection.id == id && selection.entity == entity) {
                    self.details_dirty = true;
                }
            }
            WorldChange::SubtreeInvalidated { .. } => {
                self.details_dirty |= self.selection.is_some();
            }
            WorldChange::RegionAdopted(region) => {
                self.pending_regions.insert(region);
            }
            WorldChange::RegionUpdated(region) => {
                self.pending_regions.insert(region);
                self.details_dirty |=
                    self.selection.and_then(|selection| self.objects.get(&selection.id)).is_some_and(|row| row.region == region);
            }
            WorldChange::RegionRetired { region, .. } => {
                self.snapshot_dirty |= self.regions.remove(&region).is_some();
                self.pending_regions.remove(&region);
            }
        }
    }

    fn snapshot(&mut self) -> WorldSnapshot {
        if self.snapshot_dirty {
            self.snapshot.region_labels = self
                .regions
                .values()
                .map(|region| RegionLabel {
                    region: region.region,
                    label: region.info.name.as_deref().map_or_else(|| format!("Region {}", region.region.get()), ToOwned::to_owned),
                })
                .collect::<Vec<_>>()
                .into();
            self.snapshot.objects = self.objects.values().cloned().collect::<Vec<_>>().into();
            self.snapshot_dirty = false;
        }
        self.snapshot.revision = self.revision;
        self.snapshot.coverage = self.coverage.clone();
        self.snapshot.object_count = self.object_count;
        self.snapshot.details = self.details.clone();
        self.snapshot.details_error = self.details_error.clone();
        self.snapshot.clone()
    }
}

pub(super) fn refresh_after_resync(mirror: &mut WorldMirror) -> ResyncOutcome {
    if mirror.restart_after_history_loss() { ResyncOutcome::Retry } else { ResyncOutcome::Failed }
}

#[cfg(test)]
pub(super) async fn capture_snapshot(
    reader: &WorldReader,
    mirror: &mut WorldMirror,
    selected: Option<ObjectSelection>,
) -> Result<WorldSnapshot, WorldReaderError> {
    Ok(capture_snapshot_cancellable(reader, mirror, selected, || false).await?.expect("capture without a cancellation source completes"))
}

pub(super) async fn capture_snapshot_cancellable(
    reader: &WorldReader,
    mirror: &mut WorldMirror,
    selected: Option<ObjectSelection>,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Option<WorldSnapshot>, WorldReaderError> {
    if cancelled() {
        return Ok(None);
    }
    let reader_changed = mirror.reader.as_ref().is_some_and(|current| !current.same_session(reader));
    if reader_changed {
        mirror.reset();
        mirror.selection_invalidated = selected.is_some();
    }
    let selected = if reader_changed { None } else { selected };
    let barrier = reader.barrier().await;
    if cancelled() {
        return Ok(None);
    }
    let barrier = barrier?;
    let coverage = barrier.coverage.clone();
    let target_revision = barrier.revision;
    if mirror.reader.is_none() || mirror.change_cursor.is_none() {
        mirror.begin(reader, barrier);
    }
    mirror.coverage = Some(coverage);
    if mirror.selection != selected {
        mirror.selection = selected;
        mirror.details = None;
        mirror.details_dirty = true;
    }

    if mirror.is_current(target_revision) {
        return Ok(Some(mirror.snapshot()));
    }

    for _ in 0..MAX_BOOTSTRAP_PAGES_PER_SYNC {
        if !mirror.objects_done {
            if cancelled() {
                return Ok(None);
            }
            let page = reader.object_list_page(mirror.object_cursor, MAX_OBJECT_PAGE_ITEMS).await;
            if cancelled() {
                return Ok(None);
            }
            let page = page?;
            mirror.objects.extend(page.objects.into_iter().map(|item| (item.id, object_row(item))));
            mirror.snapshot_dirty = true;
            mirror.object_cursor = page.next;
            mirror.objects_done = mirror.object_cursor.is_none();
        } else if !mirror.regions_done {
            if cancelled() {
                return Ok(None);
            }
            let page = reader.region_page(mirror.region_cursor, MAX_REGION_PAGE_ITEMS).await;
            if cancelled() {
                return Ok(None);
            }
            let page = page?;
            mirror.regions.extend(page.regions.into_iter().map(|summary| (summary.region, summary)));
            mirror.snapshot_dirty = true;
            mirror.region_cursor = page.next;
            mirror.regions_done = mirror.region_cursor.is_none();
        }
        if mirror.objects_done && mirror.regions_done {
            mirror.bootstrapping = false;
            break;
        }
    }

    if !mirror.bootstrapping {
        for _ in 0..MAX_CHANGE_PAGES_PER_SYNC {
            let cursor = mirror.change_cursor.expect("bootstrap establishes a journal cursor");
            if cancelled() {
                return Ok(None);
            }
            let page = reader.changes(cursor, MAX_WORLD_CHANGE_PAGE_ITEMS).await;
            if cancelled() {
                return Ok(None);
            }
            let page = page?;
            for record in page.records {
                mirror.apply_change(record.change);
            }
            mirror.change_cursor = Some(page.cursor);
            mirror.has_more_changes = page.has_more;
            mirror.revision = Some(page.observed_revision);
            if !page.has_more {
                mirror.journal_revision = Some(page.observed_revision);
            }
            if !page.has_more {
                break;
            }
        }
    }

    for _ in 0..MAX_SUMMARY_HYDRATIONS_PER_SYNC {
        let Some((id, entity)) = mirror.pending_objects.first_key_value().map(|(id, entity)| (*id, *entity)) else { break };
        if cancelled() {
            return Ok(None);
        }
        let item = reader.object_list_item(entity).await;
        if cancelled() {
            return Ok(None);
        }
        match item {
            Ok(read) if read.item.id == id => {
                mirror.pending_objects.remove(&id);
                if mirror.objects.insert(id, object_row(read.item)) != Some(object_row(read.item)) {
                    mirror.snapshot_dirty = true;
                }
                mirror.revision = Some(read.hydration_revision);
            }
            Err(WorldReaderError::Read(WorldReadError::StaleIdentity)) => {
                mirror.pending_objects.remove(&id);
                if mirror.objects.get(&id).is_some_and(|row| row.selection.entity == entity) {
                    mirror.objects.remove(&id);
                    mirror.snapshot_dirty = true;
                }
            }
            Err(error) => return Err(error),
            Ok(_) => {
                mirror.pending_objects.remove(&id);
            }
        }
    }

    for _ in 0..MAX_REGION_HYDRATIONS_PER_SYNC {
        let Some(region) = mirror.pending_regions.first().copied() else { break };
        if cancelled() {
            return Ok(None);
        }
        let summary = reader.region_summary(region).await;
        if cancelled() {
            return Ok(None);
        }
        let summary = summary?;
        mirror.pending_regions.remove(&region);
        match summary {
            Some(read) => {
                if mirror.regions.insert(region, read.summary.clone()) != Some(read.summary) {
                    mirror.snapshot_dirty = true;
                }
                mirror.revision = Some(read.hydration_revision);
            }
            None => {
                mirror.snapshot_dirty |= mirror.regions.remove(&region).is_some();
            }
        }
    }

    if mirror.details_dirty {
        mirror.details = match selected {
            Some(selection) => {
                if cancelled() {
                    return Ok(None);
                }
                let batch = reader.object_details(&[selection.entity]).await;
                if cancelled() {
                    return Ok(None);
                }
                let batch = batch?;
                mirror.revision = Some(batch.hydration_revision);
                match batch.details.into_iter().next() {
                    Some(ObjectDetailResult::Found(detail)) if detail.summary.id == selection.id => {
                        mirror.details_error = None;
                        Some(object_details(selection, detail))
                    }
                    Some(ObjectDetailResult::TooLarge(_)) => {
                        mirror.details_error = Some("Object details exceed the reader's size limit.".into());
                        None
                    }
                    Some(ObjectDetailResult::Gone(_) | ObjectDetailResult::Found(_)) | None => {
                        mirror.details_error = None;
                        None
                    }
                }
            }
            None => {
                mirror.details_error = None;
                None
            }
        };
        mirror.details_dirty = false;
    }

    if cancelled() {
        return Ok(None);
    }

    let snapshot = mirror.snapshot();
    if mirror.is_current(target_revision) {
        mirror.resync_restarts = 0;
    }
    Ok(Some(snapshot))
}

#[cfg(test)]
pub(crate) struct WorldSnapshotForTest {
    pub(crate) objects: Vec<(UUID, world::Entity, Region, NonZeroU32)>,
    pub(crate) regions: Vec<RegionSummary>,
    pub(crate) object_count: usize,
    pub(crate) revision: Option<WorldRevision>,
    pub(crate) selected_position: Option<Vec3>,
    pub(crate) selected_parent: Option<String>,
    pub(crate) selected_id: Option<UUID>,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct WorldMirrorForTest(WorldMirror);

#[cfg(test)]
impl WorldMirrorForTest {
    pub(crate) fn captured_object_count(&self) -> usize {
        self.0.objects.len()
    }

    pub(crate) fn object_page_started(&self) -> bool {
        self.0.object_cursor.is_some() || self.0.objects_done
    }
}

#[cfg(test)]
fn snapshot_for_test(mirror: &WorldMirror, snapshot: WorldSnapshot) -> WorldSnapshotForTest {
    let objects = mirror.objects.iter().map(|(id, row)| (*id, row.selection.entity, row.region, row.local_id)).collect();
    let regions = mirror.regions.values().cloned().collect();
    WorldSnapshotForTest {
        objects,
        regions,
        object_count: snapshot.object_count,
        revision: snapshot.revision,
        selected_position: snapshot.details.as_ref().and_then(|details| details.position),
        selected_parent: snapshot.details.as_ref().map(|details| details.parent.clone()),
        selected_id: mirror.selection.map(|selection| selection.id),
    }
}

#[cfg(test)]
pub(crate) async fn capture_snapshot_for_test(
    reader: &WorldReader,
    selected: Option<(UUID, world::Entity)>,
) -> Result<WorldSnapshotForTest, WorldReaderError> {
    let mut mirror = WorldMirror { reader: Some(reader.clone()), ..WorldMirror::default() };
    assert!(mirror.restart_after_history_loss());
    let snapshot = capture_snapshot(reader, &mut mirror, selected.map(|(id, entity)| ObjectSelection { id, entity })).await?;
    Ok(snapshot_for_test(&mirror, snapshot))
}

#[cfg(test)]
pub(crate) async fn capture_snapshot_cancellable_for_test(
    reader: &WorldReader,
    mirror: &mut WorldMirrorForTest,
    cancelled: impl FnMut() -> bool,
) -> Result<Option<WorldSnapshotForTest>, WorldReaderError> {
    let snapshot = capture_snapshot_cancellable(reader, &mut mirror.0, None, cancelled).await?;
    Ok(snapshot.map(|snapshot| snapshot_for_test(&mirror.0, snapshot)))
}

#[cfg(test)]
pub(crate) async fn capture_snapshot_after_session_change_for_test(
    previous_reader: &WorldReader,
    reader: &WorldReader,
    selected: (UUID, world::Entity),
) -> (Result<WorldSnapshotForTest, WorldReaderError>, bool) {
    let (id, entity) = selected;
    let selection = ObjectSelection { id, entity };
    let mut mirror = WorldMirror { reader: Some(previous_reader.clone()), selection: Some(selection), ..WorldMirror::default() };
    let capture = capture_snapshot(reader, &mut mirror, Some(selection)).await;
    if matches!(
        &capture,
        Err(WorldReaderError::Read(WorldReadError::ResyncRequired | WorldReadError::InvalidCursor | WorldReadError::StaleSession))
    ) {
        let _ = refresh_after_resync(&mut mirror);
    }
    let result = capture.map(|snapshot| snapshot_for_test(&mirror, snapshot));
    (result, mirror.selection_invalidated)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use base::UUID;
    use world::{
        ApplyOutcome::{Introduced, RegionAdopted},
        Kind, LocalAddress, MAX_OBJECT_PAGE_ITEMS, MAX_REGION_PAGE_ITEMS, ObjectObservation,
        OrderEvidence::Ordered,
        ParentUpdate::Root,
        Region, UpdateSource, World, WorldChange, WorldEntityRemovalReason, WorldLimits,
    };

    use crate::devtools::{InspectorState, ObjectFilter, ObjectSelection, inspector_state, object_details, object_table};

    use super::{MAX_BOOTSTRAP_RESTARTS, ResyncOutcome, WorldMirror};

    const SNAPSHOT_OBJECTS: usize = 300;

    fn limits() -> WorldLimits {
        WorldLimits {
            max_region_registrations: 1,
            max_entities: SNAPSHOT_OBJECTS,
            max_entities_per_region: SNAPSHOT_OBJECTS,
            max_tombstones_per_region: 1,
            max_tombstones: 1,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
            max_journal_entries: SNAPSHOT_OBJECTS + 2,
            max_journal_changes: SNAPSHOT_OBJECTS + 2,
            max_region_name_bytes: 32,
            max_recovery_targets: 1,
        }
    }

    fn selection_fixture() -> ObjectSelection {
        let region = Region::new(1);
        let id = UUID::from_u128(7);
        let local_id = NonZeroU32::new(41).expect("nonzero local ID");
        let mut world = World::new(limits());
        assert_eq!(world.adopt_region(region).expect("adopt region").outcome, RegionAdopted);
        assert!(matches!(
            world
                .observe_object(ObjectObservation {
                    source: UpdateSource::new(region, 1, Ordered(1), 0),
                    id,
                    local_id,
                    kind: Kind::Object,
                    parent: Root,
                    pose: None,
                    linear_motion: None,
                    angular_velocity: None,
                    scale: None,
                    attachment_point: None,
                    shape: None,
                    appearance: None,
                    surface_appearance: None,
                    render_materials: None,
                })
                .expect("introduce object")
                .outcome,
            Introduced(_)
        ));
        ObjectSelection { entity: world.entity(id).expect("object entity"), id }
    }

    #[test]
    fn mirror_builds_a_sorted_snapshot() {
        let region = Region::new(1);
        let mut world = World::new(limits());
        assert_eq!(world.adopt_region(region).expect("adopt region").outcome, RegionAdopted);
        let selected = UUID::from_u128(1);
        for id in 1..=SNAPSHOT_OBJECTS as u128 {
            assert!(matches!(
                world
                    .observe_object(ObjectObservation {
                        source: UpdateSource::new(region, id as u32, Ordered(id as u64), 0),
                        id: UUID::from_u128(id),
                        local_id: NonZeroU32::new(id as u32).expect("nonzero local ID"),
                        kind: Kind::Object,
                        parent: Root,
                        pose: None,
                        linear_motion: None,
                        angular_velocity: None,
                        scale: None,
                        attachment_point: None,
                        shape: None,
                        appearance: None,
                        surface_appearance: None,
                        render_materials: None,
                    })
                    .expect("introduce object")
                    .outcome,
                Introduced(_)
            ));
        }

        let barrier = world.read_barrier();
        let selection = ObjectSelection { entity: world.entity(selected).expect("selected object"), id: selected };
        let detail = world.object_detail(selection.entity).expect("selected detail");
        let mut mirror = WorldMirror {
            object_count: barrier.object_count,
            revision: Some(barrier.revision),
            selection: Some(selection),
            details: Some(object_details(selection, detail)),
            ..WorldMirror::default()
        };

        let mut object_cursor = None;
        loop {
            let page = world.object_list_page(object_cursor, MAX_OBJECT_PAGE_ITEMS).expect("object list page");
            mirror.objects.extend(page.objects.into_iter().map(|item| (item.id, super::super::object_row(item))));
            object_cursor = page.next;
            if object_cursor.is_none() {
                break;
            }
        }
        let mut region_cursor = None;
        loop {
            let page = world.region_page(region_cursor, MAX_REGION_PAGE_ITEMS).expect("region summary page");
            mirror.regions.extend(page.regions.into_iter().map(|summary| (summary.region, summary)));
            region_cursor = page.next;
            if region_cursor.is_none() {
                break;
            }
        }
        mirror.snapshot_dirty = true;
        let snapshot = mirror.snapshot();
        assert_eq!(snapshot.objects.len(), SNAPSHOT_OBJECTS);
        assert_eq!(snapshot.object_count, SNAPSHOT_OBJECTS);
        assert_eq!(snapshot.details.as_ref().map(|details| details.selection.id), Some(selected));
        assert!(snapshot.objects.iter().all(|object| object.region == region));
    }

    #[test]
    fn old_removal_keeps_reused_object() {
        let region = Region::new(1);
        let id = UUID::from_u128(42);
        let local_id = NonZeroU32::new(9).unwrap();
        let mut world = World::new(limits());
        assert_eq!(world.adopt_region(region).expect("adopt region").outcome, RegionAdopted);
        let mut observation = ObjectObservation {
            source: UpdateSource::new(region, 1, Ordered(1), 0),
            id,
            local_id,
            kind: Kind::Object,
            parent: Root,
            pose: None,
            linear_motion: None,
            angular_velocity: None,
            scale: None,
            attachment_point: None,
            shape: None,
            appearance: None,
            surface_appearance: None,
            render_materials: None,
        };
        assert!(matches!(world.observe_object(observation.clone()).unwrap().outcome, Introduced(_)));
        let old_entity = world.entity(id).unwrap();
        let old_item = world.object_list_item(old_entity).unwrap().item;
        let selection = ObjectSelection { entity: old_entity, id };

        let _ = world.remove_object(UpdateSource::new(region, 2, Ordered(2), 0), local_id).expect("remove selected entity");
        observation.source = UpdateSource::new(region, 3, Ordered(3), 0);
        assert!(matches!(world.observe_object(observation).unwrap().outcome, Introduced(_)));
        let replacement = ObjectSelection { entity: world.entity(id).unwrap(), id };
        assert_ne!(selection.entity, replacement.entity);

        let item = world.object_list_item(replacement.entity).unwrap().item;
        let mut mirror = WorldMirror {
            object_count: 1,
            objects: std::collections::BTreeMap::from([(id, super::super::object_row(old_item))]),
            selection: Some(selection),
            ..WorldMirror::default()
        };
        mirror.apply_change(WorldChange::EntityRemoved {
            id,
            entity: old_entity,
            address: LocalAddress::new(region, local_id),
            reason: WorldEntityRemovalReason::Killed,
        });
        assert_eq!(mirror.object_count, 0);
        mirror.apply_change(WorldChange::EntityAdded { id, entity: replacement.entity, address: item.address });
        mirror.objects.insert(id, super::super::object_row(item));
        mirror.snapshot_dirty = true;
        let snapshot = mirror.snapshot();
        let table = object_table(&snapshot, &ObjectFilter::default());

        assert_eq!(replacement.id, selection.id);
        assert_eq!(snapshot.object_count, 1);
        assert!(snapshot.details.is_none());
        assert!(matches!(inspector_state(&snapshot, Some(selection)), InspectorState::Removed));
        assert_eq!(table.row_for_object(Some(selection)), None);
        assert_eq!(table.row_for_object(Some(replacement)), Some(0));
    }

    #[test]
    fn updates_refresh_selected_details() {
        let selection = selection_fixture();
        let mut mirror = WorldMirror { selection: Some(selection), ..WorldMirror::default() };

        mirror.apply_change(WorldChange::EntityUpdated { id: selection.id, entity: selection.entity, classes: Default::default() });

        assert!(mirror.pending_objects.is_empty());
        assert!(mirror.details_dirty);
    }

    #[test]
    fn repeated_resync_requires_refresh() {
        let mut mirror = WorldMirror { object_count: 5, ..WorldMirror::default() };

        for attempt in 1..=MAX_BOOTSTRAP_RESTARTS {
            assert!(matches!(super::refresh_after_resync(&mut mirror), ResyncOutcome::Retry));
            assert_eq!(mirror.resync_restarts, attempt);
            assert_eq!(mirror.object_count, 0);
        }

        mirror.object_count = 7;
        assert!(matches!(super::refresh_after_resync(&mut mirror), ResyncOutcome::Failed));
        assert_eq!(mirror.resync_restarts, MAX_BOOTSTRAP_RESTARTS);
        assert_eq!(mirror.object_count, 7);
    }

    #[test]
    fn hydration_does_not_advance_journal() {
        let region = Region::new(1);
        let mut world = World::new(limits());
        let start = world.read_barrier().revision;
        let mut mirror = WorldMirror { journal_revision: Some(start), revision: Some(start), ..WorldMirror::default() };
        assert!(mirror.is_current(start));

        assert_eq!(world.adopt_region(region).expect("adopt region").outcome, RegionAdopted);
        let hydrated = world.region_summary(region).unwrap().expect("region summary").hydration_revision;
        mirror.revision = Some(hydrated);

        assert!(!mirror.is_current(hydrated));
        mirror.journal_revision = Some(hydrated);
        assert!(mirror.is_current(hydrated));
    }
}
