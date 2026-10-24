use std::{
    collections::Bound::{Excluded, Unbounded},
    error::Error,
    fmt::{self, Display, Formatter},
    mem::size_of,
};

use uuid::Uuid;

use crate::{
    Appearance, AttachmentPoint, AvatarAppearance, LinearMotion, ObjectMedia, PbrMaterialAssignments, PropertiesProvenance, SitTarget,
    SurfaceAppearance, Transform,
};
use crate::{
    Entity, Kind, LocalAddress, Parent, PbrMaterialOverrides, PoseError, PoseState, Properties, Region, RegionInfo, Shape, WorldRevision,
    WorldTransformState,
};
use crate::{Object, World, WorldChange, WorldId};

pub const MAX_OBJECT_PAGE_ITEMS: usize = 256;
pub const MAX_REGION_PAGE_ITEMS: usize = 64;
pub const MAX_REGION_PAGE_BYTES: usize = 1024 * 1024;
pub const MAX_WORLD_CHANGE_PAGE_ITEMS: usize = 256;
pub const MAX_OBJECT_DETAIL_BYTES: usize = 64 * 1024;
pub const MAX_OBJECT_MEDIA_READ_BYTES: usize = 256 * 1024;
pub const MAX_OBJECT_DETAIL_BATCH_ITEMS: usize = 8;
pub const MAX_OBJECT_DETAIL_BATCH_BYTES: usize = MAX_OBJECT_DETAIL_BATCH_ITEMS * MAX_OBJECT_DETAIL_BYTES
    + size_of::<ObjectDetailBatch>()
    + MAX_OBJECT_DETAIL_BATCH_ITEMS * size_of::<ObjectDetailResult>();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldReadBarrier {
    pub revision: WorldRevision,
    pub object_count: usize,
    pub region_count: usize,
    pub changes: WorldChangeCursor,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectSummary {
    pub entity: Entity,
    pub id: Uuid,
    pub kind: Kind,
    pub address: LocalAddress,
    pub parent: Parent,
    pub local_transform: Transform,
    pub pose: Result<PoseState, PoseError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectListItem {
    pub entity: Entity,
    pub id: Uuid,
    pub kind: Kind,
    pub address: LocalAddress,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectListPage {
    pub hydration_revision: WorldRevision,
    pub objects: Vec<ObjectListItem>,
    pub next: Option<ObjectPageCursor>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectPageCursor {
    world: WorldId,
    after: Uuid,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegionSummary {
    pub region: Region,
    pub info: RegionInfo,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegionPage {
    pub hydration_revision: WorldRevision,
    pub regions: Vec<RegionSummary>,
    pub next: Option<RegionPageCursor>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegionPageCursor {
    world: WorldId,
    after: Region,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectDetail {
    pub summary: ObjectSummary,
    pub hydration_revision: WorldRevision,
    pub world_transform: Result<WorldTransformState, PoseError>,
    pub parent_id: Option<Uuid>,
    pub linear_motion: LinearMotion,
    pub angular_velocity: Option<math::Vec3>,
    pub shape: Option<Shape>,
    pub appearance: Option<Appearance>,
    pub surface_appearance: Option<SurfaceAppearance>,
    pub pbr_materials: Option<PbrMaterialAssignments>,
    pub pbr_material_overrides: Option<PbrMaterialOverrides>,
    pub avatar_appearance: Option<AvatarAppearance>,
    pub attachment_point: Option<AttachmentPoint>,
    pub sit_target: Option<SitTarget>,
    pub properties: Properties,
    pub properties_provenance: PropertiesProvenance,
}

impl ObjectDetail {
    pub fn owned_bytes(&self) -> usize {
        detail_bytes(
            self.surface_appearance.as_ref(),
            self.pbr_materials.as_ref(),
            self.pbr_material_overrides.as_ref(),
            self.avatar_appearance.as_ref(),
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectMediaRead {
    pub entity: Entity,
    pub hydration_revision: WorldRevision,
    pub media: Option<ObjectMedia>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ObjectDetailResult {
    Found(ObjectDetail),
    Gone(Entity),
    TooLarge(Entity),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectDetailBatch {
    pub hydration_revision: WorldRevision,
    pub details: Vec<ObjectDetailResult>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldChangeCursor {
    world: WorldId,
    through_revision: WorldRevision,
    next_change: Option<usize>,
}

impl WorldChangeCursor {
    fn completed_revision(self) -> WorldRevision {
        match self.next_change {
            Some(_) => WorldRevision(self.through_revision.0.checked_sub(1).expect("a partial revision is nonzero")),
            None => self.through_revision,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldChangeRecord {
    pub revision: WorldRevision,
    pub change: WorldChange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldChangePage {
    pub observed_revision: WorldRevision,
    pub completed_revision: WorldRevision,
    pub records: Vec<WorldChangeRecord>,
    pub cursor: WorldChangeCursor,
    pub has_more: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorldReadError {
    StaleSession,
    StaleIdentity,
    ResyncRequired,
    InvalidCursor,
    InvalidLimit,
    TooLarge,
}

impl Display for WorldReadError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleSession => f.write_str("cursor or entity belongs to another World session"),
            Self::StaleIdentity => f.write_str("object incarnation is no longer live"),
            Self::ResyncRequired => f.write_str("journal history was evicted; rebuild from a fresh read barrier"),
            Self::InvalidCursor => f.write_str("invalid World read cursor"),
            Self::InvalidLimit => f.write_str("read limit is zero or exceeds the operation maximum"),
            Self::TooLarge => f.write_str("read result exceeds the byte limit"),
        }
    }
}

impl Error for WorldReadError {}

impl World {
    pub fn read_barrier(&self) -> WorldReadBarrier {
        let revision = self.revision();
        WorldReadBarrier {
            revision,
            object_count: self.object_count(),
            region_count: self.regions.len(),
            changes: WorldChangeCursor { world: self.id, through_revision: revision, next_change: None },
        }
    }

    pub fn object_list_page(&self, cursor: Option<ObjectPageCursor>, limit: usize) -> Result<ObjectListPage, WorldReadError> {
        if limit == 0 || limit > MAX_OBJECT_PAGE_ITEMS {
            return Err(WorldReadError::InvalidLimit);
        }
        if cursor.is_some_and(|cursor| cursor.world != self.id) {
            return Err(WorldReadError::StaleSession);
        }

        let lower = cursor.map_or(Unbounded, |cursor| Excluded(cursor.after));
        let mut ids = self.store.ids().range((lower, Unbounded));
        let objects = ids
            .by_ref()
            .take(limit)
            .map(|id| {
                let entity = self.entity(*id).expect("object ordering index contains only live object IDs");
                let record = self.store.object_record(entity).expect("UUID index contains only live entities");
                ObjectListItem { entity, id: record.id, kind: record.kind, address: record.state.binding.address }
            })
            .collect::<Vec<_>>();
        let next = ids.next().map(|_| ObjectPageCursor { world: self.id, after: objects.last().expect("nonzero page limit").id });
        Ok(ObjectListPage { hydration_revision: self.revision(), objects, next })
    }

    pub fn region_page(&self, cursor: Option<RegionPageCursor>, limit: usize) -> Result<RegionPage, WorldReadError> {
        if limit == 0 || limit > MAX_REGION_PAGE_ITEMS {
            return Err(WorldReadError::InvalidLimit);
        }
        if cursor.is_some_and(|cursor| cursor.world != self.id) {
            return Err(WorldReadError::StaleSession);
        }

        let hydration_revision = self.revision();
        let mut regions = Vec::with_capacity(limit);
        let mut after = cursor.map(|cursor| cursor.after);
        let mut iter = match after {
            Some(region) => self.regions.range((Excluded(region), Unbounded)),
            None => self.regions.range(..),
        };
        let mut bytes = 0usize;
        let mut oversized_item = false;
        for (region, state) in iter.by_ref().take(limit) {
            let info = &state.info;
            let charge = region_summary_bytes(info);
            if bytes.saturating_add(charge) > MAX_REGION_PAGE_BYTES {
                if regions.is_empty() {
                    return Err(WorldReadError::TooLarge);
                }
                oversized_item = true;
                break;
            }
            bytes = bytes.saturating_add(charge);
            after = Some(*region);
            regions.push(RegionSummary { region: *region, info: info.clone() });
        }
        let has_more = oversized_item || iter.next().is_some();
        let next = has_more.then(|| RegionPageCursor { world: self.id, after: after.expect("nonzero page limit") });
        Ok(RegionPage { hydration_revision, regions, next })
    }

    pub fn object_detail(&self, entity: Entity) -> Result<ObjectDetail, WorldReadError> {
        self.object_detail_at(entity, self.revision())
    }

    pub fn object_detail_batch(&self, entities: &[Entity]) -> Result<ObjectDetailBatch, WorldReadError> {
        if entities.is_empty() || entities.len() > MAX_OBJECT_DETAIL_BATCH_ITEMS {
            return Err(WorldReadError::InvalidLimit);
        }
        if entities.iter().any(|entity| entity.1 != self.id) {
            return Err(WorldReadError::StaleSession);
        }

        let hydration_revision = self.revision();
        let mut details = Vec::with_capacity(entities.len());
        for entity in entities {
            let detail = match self.object_detail_at(*entity, hydration_revision) {
                Ok(detail) => ObjectDetailResult::Found(detail),
                Err(WorldReadError::StaleIdentity) => ObjectDetailResult::Gone(*entity),
                Err(WorldReadError::TooLarge) => ObjectDetailResult::TooLarge(*entity),
                Err(error) => return Err(error),
            };
            details.push(detail);
        }

        Ok(ObjectDetailBatch { hydration_revision, details })
    }

    pub fn object_media(&self, entity: Entity) -> Result<ObjectMediaRead, WorldReadError> {
        if entity.1 != self.id {
            return Err(WorldReadError::StaleSession);
        }
        let object = self.object(entity).ok_or(WorldReadError::StaleIdentity)?;
        let media = object.object_media();
        let bytes = size_of::<ObjectMediaRead>()
            .saturating_add(media.map_or(0, |media| media.retained_bytes().saturating_sub(size_of::<ObjectMedia>())));
        if bytes > MAX_OBJECT_MEDIA_READ_BYTES {
            return Err(WorldReadError::TooLarge);
        }
        Ok(ObjectMediaRead { entity, hydration_revision: self.revision(), media: media.cloned() })
    }

    fn object_detail_at(&self, entity: Entity, hydration_revision: WorldRevision) -> Result<ObjectDetail, WorldReadError> {
        if entity.1 != self.id {
            return Err(WorldReadError::StaleSession);
        }
        let object = self.object(entity).ok_or(WorldReadError::StaleIdentity)?;
        let world_transform = object.world_transform();
        let pose = match world_transform {
            Ok(WorldTransformState::Resolved(transform)) => Ok(PoseState::Resolved(transform.pose)),
            Ok(WorldTransformState::Provisional { pose, reason }) => Ok(PoseState::Provisional { pose, reason }),
            Ok(WorldTransformState::Unresolved(reason)) => Ok(PoseState::Unresolved(reason)),
            Ok(WorldTransformState::MissingScale(pose)) => Ok(PoseState::Resolved(pose)),
            Err(error) => Err(error),
        };
        let summary = ObjectSummary {
            entity,
            id: object.id(),
            kind: object.kind(),
            address: object.address(),
            parent: object.parent(),
            local_transform: object.transform(),
            pose,
        };
        let parent_id = match summary.parent {
            Parent::Resolved(parent) => self.object(parent).map(|object| object.id()),
            Parent::Unknown | Parent::Root | Parent::Pending { .. } => None,
        };
        if object_detail_bytes(&object) > MAX_OBJECT_DETAIL_BYTES {
            return Err(WorldReadError::TooLarge);
        }
        let detail = ObjectDetail {
            summary,
            hydration_revision,
            world_transform,
            parent_id,
            linear_motion: object.linear_motion(),
            angular_velocity: object.angular_velocity(),
            shape: object.shape(),
            appearance: object.appearance(),
            surface_appearance: object.surface_appearance().cloned(),
            pbr_materials: object.pbr_materials().cloned(),
            pbr_material_overrides: object.pbr_material_overrides().cloned(),
            avatar_appearance: object.avatar_appearance().cloned(),
            attachment_point: object.attachment_point(),
            sit_target: object.sit_target(),
            properties: object.properties(),
            properties_provenance: object.properties_provenance(),
        };
        Ok(detail)
    }

    pub fn change_page(&self, cursor: WorldChangeCursor, limit: usize) -> Result<WorldChangePage, WorldReadError> {
        if limit == 0 || limit > MAX_WORLD_CHANGE_PAGE_ITEMS {
            return Err(WorldReadError::InvalidLimit);
        }
        if cursor.world != self.id {
            return Err(WorldReadError::StaleSession);
        }
        if cursor.through_revision > self.revision() {
            return Err(WorldReadError::InvalidCursor);
        }
        if let Some(lost_through) = self.journal.lost_through {
            let loss_affects_cursor = match cursor.next_change {
                Some(_) => lost_through >= cursor.through_revision,
                None => cursor.through_revision < lost_through,
            };
            if loss_affects_cursor {
                return Err(WorldReadError::ResyncRequired);
            }
        }

        let (mut entry_index, mut change_index) = if let Some(next_change) = cursor.next_change {
            let entry_index = self.journal.entry_index(cursor.through_revision);
            let Some(entry) = self.journal.entries.get(entry_index).filter(|entry| entry.revision == cursor.through_revision) else {
                return Err(WorldReadError::ResyncRequired);
            };
            if next_change >= entry.changes.len() {
                return Err(WorldReadError::InvalidCursor);
            }
            (entry_index, next_change)
        } else {
            let entry_index = self.journal.entry_index(cursor.through_revision);
            let entry_index = if self.journal.entries.get(entry_index).is_some_and(|entry| entry.revision == cursor.through_revision) {
                entry_index + 1
            } else {
                entry_index
            };
            (entry_index, 0)
        };

        let mut records = Vec::with_capacity(limit);
        let mut next_cursor = cursor;
        while let Some(entry) = self.journal.entries.get(entry_index) {
            for (index, change) in entry.changes.iter().copied().enumerate().skip(change_index) {
                records.push(WorldChangeRecord { revision: entry.revision, change });
                let next_index = index + 1;
                if records.len() == limit {
                    let has_more_in_entry = next_index < entry.changes.len();
                    let has_more = has_more_in_entry || entry_index + 1 < self.journal.entries.len();
                    next_cursor = if has_more_in_entry {
                        WorldChangeCursor { world: self.id, through_revision: entry.revision, next_change: Some(next_index) }
                    } else {
                        WorldChangeCursor { world: self.id, through_revision: entry.revision, next_change: None }
                    };
                    return Ok(WorldChangePage {
                        observed_revision: self.revision(),
                        completed_revision: next_cursor.completed_revision(),
                        records,
                        cursor: next_cursor,
                        has_more,
                    });
                }
            }
            next_cursor = WorldChangeCursor { world: self.id, through_revision: entry.revision, next_change: None };
            entry_index += 1;
            change_index = 0;
        }

        Ok(WorldChangePage {
            observed_revision: self.revision(),
            completed_revision: next_cursor.completed_revision(),
            records,
            cursor: next_cursor,
            has_more: false,
        })
    }
}

fn region_summary_bytes(info: &RegionInfo) -> usize {
    size_of::<RegionSummary>().saturating_add(info.name.as_ref().map_or(0, String::len))
}

fn object_detail_bytes(object: &Object<'_>) -> usize {
    detail_bytes(object.surface_appearance(), object.pbr_materials(), object.pbr_material_overrides(), object.avatar_appearance())
}

fn detail_bytes(
    surface_appearance: Option<&SurfaceAppearance>,
    pbr_materials: Option<&PbrMaterialAssignments>,
    pbr_material_overrides: Option<&PbrMaterialOverrides>,
    avatar_appearance: Option<&AvatarAppearance>,
) -> usize {
    size_of::<ObjectDetail>()
        .saturating_add(surface_appearance.map_or(0, SurfaceAppearance::heap_bytes))
        .saturating_add(pbr_materials.map_or(0, PbrMaterialAssignments::heap_bytes))
        .saturating_add(pbr_material_overrides.map_or(0, PbrMaterialOverrides::heap_bytes))
        .saturating_add(avatar_appearance.map_or(0, AvatarAppearance::heap_bytes))
}
