//! Bounded snapshots and revision-checked pages for external consumers.

use std::{
    collections::Bound::{Excluded, Unbounded},
    mem::size_of,
};

use base::UUID;

use super::{Object, World, WorldChange, WorldId};
use crate::{
    Appearance, AssetId, AttachmentPoint, AvatarAppearance, AvatarTextureSlots, AvatarVisualParamIndex, FaceAppearance,
    GltfMaterialOverride, LinearMotion, ObjectMedia, PropertiesProvenance, RenderMaterials, SitTarget, SurfaceAppearance, Transform,
};
use crate::{
    Entity, GltfMaterialOverrides, Kind, LocalAddress, Parent, PoseError, PoseState, Properties, Region, RegionInfo, Shape, WorldRevision,
    WorldTransformState,
};

/// Maximum number of objects returned in one page.
pub const MAX_OBJECT_PAGE_ITEMS: usize = 256;
/// Maximum number of regions returned in one page.
pub const MAX_REGION_PAGE_ITEMS: usize = 64;
/// Maximum encoded size charged to one region page.
pub const MAX_REGION_PAGE_BYTES: usize = 1024 * 1024;
/// Maximum number of journal changes returned in one page.
pub const MAX_WORLD_CHANGE_PAGE_ITEMS: usize = 256;
/// Maximum estimated owned data returned by one detail query.
pub const MAX_OBJECT_DETAIL_BYTES: usize = 64 * 1024;
/// Maximum number of object details copied by one request.
pub const MAX_OBJECT_DETAIL_BATCH_ITEMS: usize = 8;

/// A point-in-time count and journal barrier for starting a World mirror.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldReadBarrier {
    pub revision: WorldRevision,
    pub object_count: usize,
    pub region_count: usize,
    pub changes: WorldChangeCursor,
}

/// Stable object information used to populate a consumer-side summary mirror.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectSummary {
    pub entity: Entity,
    pub id: UUID,
    pub kind: Kind,
    pub address: LocalAddress,
    pub parent: Parent,
    pub local_transform: Transform,
    pub pose: Result<PoseState, PoseError>,
}

/// An object summary copied at one World revision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectSummaryRead {
    /// Revision at which `summary` was copied.
    pub hydration_revision: WorldRevision,
    pub summary: ObjectSummary,
}

/// Lightweight current object data for identity tables and indexes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectListItem {
    pub entity: Entity,
    pub id: UUID,
    pub kind: Kind,
    pub address: LocalAddress,
}

/// An object list item copied at one World revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectListItemRead {
    /// Revision at which `item` was copied.
    pub hydration_revision: WorldRevision,
    pub item: ObjectListItem,
}

/// A bounded page of lightweight object list items.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectListPage {
    /// Revision at which all values in this page were read.
    pub hydration_revision: WorldRevision,
    /// Items in UUID order.
    pub objects: Vec<ObjectListItem>,
    /// Resume after the last returned object, if more objects were available.
    pub next: Option<ObjectPageCursor>,
}

/// A bounded page of object summaries, hydrated at one World revision.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectPage {
    /// Revision at which all values in this page were read.
    pub hydration_revision: WorldRevision,
    /// Summaries in UUID order.
    pub objects: Vec<ObjectSummary>,
    /// Resume after the last returned object, if more objects were available.
    pub next: Option<ObjectPageCursor>,
}

/// Opaque cursor for stable UUID-ordered object pages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectPageCursor {
    world: WorldId,
    after: UUID,
}

/// A bounded region summary with owned metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionSummary {
    pub region: Region,
    pub info: RegionInfo,
}

/// A region summary copied at one World revision.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionSummaryRead {
    /// Revision at which `summary` was copied.
    pub hydration_revision: WorldRevision,
    pub summary: RegionSummary,
}

/// A bounded page of region summaries, hydrated at one World revision.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionPage {
    /// Revision at which all values in this page were read.
    pub hydration_revision: WorldRevision,
    /// Summaries in Region order.
    pub regions: Vec<RegionSummary>,
    /// Resume after the last returned region, if more regions were available.
    pub next: Option<RegionPageCursor>,
}

/// Opaque cursor for stable region-ordered pages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegionPageCursor {
    world: WorldId,
    after: Region,
}

/// Owned details for one object, copied from a single World revision.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectDetail {
    /// Current object identity and simulator address.
    pub summary: ObjectSummary,
    /// Revision at which all values in this detail were read.
    pub hydration_revision: WorldRevision,
    /// Resolved-capable world transform, or the reason it is unavailable.
    pub world_transform: Result<WorldTransformState, PoseError>,
    pub parent_id: Option<UUID>,
    pub linear_motion: LinearMotion,
    pub angular_velocity: Option<math::Vec3>,
    pub shape: Option<Shape>,
    pub appearance: Option<Appearance>,
    /// Current TextureEntry face data.
    pub surface_appearance: Option<SurfaceAppearance>,
    pub render_materials: Option<RenderMaterials>,
    pub gltf_material_overrides: Option<GltfMaterialOverrides>,
    pub avatar_appearance: Option<AvatarAppearance>,
    pub attachment_point: Option<AttachmentPoint>,
    pub sit_target: Option<SitTarget>,
    /// Current owner and creator UUIDs.
    pub properties: Properties,
    /// Sources that supplied the current owner and creator values.
    pub properties_provenance: PropertiesProvenance,
    /// Current per-face media values.
    pub object_media: Option<ObjectMedia>,
}

/// Outcome for one entity in an [`ObjectDetailBatch`].
#[derive(Clone, Debug, PartialEq)]
pub enum ObjectDetailResult {
    Found(ObjectDetail),
    Gone(Entity),
    TooLarge(Entity),
}

/// Owned details hydrated at one World revision, in request order.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectDetailBatch {
    /// Revision at which all outcomes were evaluated.
    pub hydration_revision: WorldRevision,
    /// One outcome for each requested entity, in the same order.
    pub details: Vec<ObjectDetailResult>,
}

/// Maximum estimated owned data returned by one detail batch.
pub const MAX_OBJECT_DETAIL_BATCH_BYTES: usize = MAX_OBJECT_DETAIL_BATCH_ITEMS * MAX_OBJECT_DETAIL_BYTES
    + size_of::<ObjectDetailBatch>()
    + MAX_OBJECT_DETAIL_BATCH_ITEMS * size_of::<ObjectDetailResult>();

/// Position in a revisioned journal. `next_change` is present only while resuming inside that revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldChangeCursor {
    world: WorldId,
    /// All changes through this revision have been consumed when `next_change` is `None`.
    through_revision: WorldRevision,
    /// Index of the next change when a page stopped partway through `through_revision`.
    next_change: Option<usize>,
}

/// One ordered World invalidation or lifecycle fact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldChangeRecord {
    /// Revision that produced this change.
    pub revision: WorldRevision,
    /// Ordered invalidation or lifecycle fact.
    pub change: WorldChange,
}

/// A bounded page of journal changes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldChangePage {
    pub observed_revision: WorldRevision,
    pub records: Vec<WorldChangeRecord>,
    pub cursor: WorldChangeCursor,
    pub has_more: bool,
}

/// Failure to hydrate or continue a bounded World read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorldReadError {
    /// The cursor or entity belongs to a different World session.
    StaleSession,
    /// The entity incarnation is no longer live in this World.
    StaleIdentity,
    /// The requested journal history has been evicted.
    ResyncRequired,
    /// A revision-bound query cursor no longer matches the current World revision.
    StaleRevision,
    /// The cursor is ahead of the World or does not identify a valid continuation.
    InvalidCursor,
    /// The requested item count is zero or exceeds the operation's hard maximum.
    InvalidLimit,
    /// A copied result exceeds the operation's hard byte maximum.
    TooLarge,
}

impl World {
    /// Captures counts and a cursor that can replay all later mutations.
    pub fn read_barrier(&self) -> WorldReadBarrier {
        let revision = self.revision();
        WorldReadBarrier {
            revision,
            object_count: self.object_count(),
            region_count: self.regions.len(),
            changes: WorldChangeCursor { world: self.id, through_revision: revision, next_change: None },
        }
    }

    /// Returns a stable UUID-ordered page of summaries.
    ///
    /// Each page is hydrated independently. Consumers must pull changes from a barrier to reconcile
    /// inserts and removals that occur while they enumerate pages.
    pub fn object_page(&self, cursor: Option<ObjectPageCursor>, limit: usize) -> Result<ObjectPage, WorldReadError> {
        if limit == 0 || limit > MAX_OBJECT_PAGE_ITEMS {
            return Err(WorldReadError::InvalidLimit);
        }
        if cursor.is_some_and(|cursor| cursor.world != self.id) {
            return Err(WorldReadError::StaleSession);
        }

        let hydration_revision = self.revision();
        let mut ids = match cursor {
            Some(cursor) => self.object_ids.range((Excluded(cursor.after), Unbounded)),
            None => self.object_ids.range(..),
        };
        let mut objects = Vec::with_capacity(limit);
        for id in ids.by_ref().take(limit) {
            objects.push(self.summary_for_id(*id));
        }
        let has_more = ids.next().is_some();
        let next = has_more.then(|| ObjectPageCursor { world: self.id, after: objects.last().expect("nonzero page limit").id });
        Ok(ObjectPage { hydration_revision, objects, next })
    }

    /// Returns a stable UUID-ordered page without resolving object poses.
    ///
    /// # Errors
    ///
    /// Returns [`WorldReadError::InvalidLimit`] for a zero or oversized page and
    /// [`WorldReadError::StaleSession`] for a cursor from another World.
    pub fn object_list_page(&self, cursor: Option<ObjectPageCursor>, limit: usize) -> Result<ObjectListPage, WorldReadError> {
        if limit == 0 || limit > MAX_OBJECT_PAGE_ITEMS {
            return Err(WorldReadError::InvalidLimit);
        }
        if cursor.is_some_and(|cursor| cursor.world != self.id) {
            return Err(WorldReadError::StaleSession);
        }

        let hydration_revision = self.revision();
        let mut ids = match cursor {
            Some(cursor) => self.object_ids.range((Excluded(cursor.after), Unbounded)),
            None => self.object_ids.range(..),
        };
        let mut objects = Vec::with_capacity(limit);
        for id in ids.by_ref().take(limit) {
            objects.push(self.list_item_for_id(*id));
        }
        let has_more = ids.next().is_some();
        let next = has_more.then(|| ObjectPageCursor { world: self.id, after: objects.last().expect("nonzero page limit").id });
        Ok(ObjectListPage { hydration_revision, objects, next })
    }

    /// Copies one current object list item without resolving its pose.
    ///
    /// # Errors
    ///
    /// Returns [`WorldReadError::StaleSession`] for an entity from another World and
    /// [`WorldReadError::StaleIdentity`] when that incarnation is no longer live.
    pub fn object_list_item(&self, entity: Entity) -> Result<ObjectListItemRead, WorldReadError> {
        let item = self.list_item_for_entity(entity)?;
        Ok(ObjectListItemRead { hydration_revision: self.revision(), item })
    }

    /// Copies one current summary for `entity`.
    pub fn object_summary(&self, entity: Entity) -> Result<ObjectSummaryRead, WorldReadError> {
        let summary = self.summary_for_entity(entity)?;
        Ok(ObjectSummaryRead { hydration_revision: self.revision(), summary })
    }

    /// Returns a stable Region-ordered page with bounded copied metadata.
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
        for (region, info) in iter.by_ref().take(limit) {
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

    /// Copies one current region summary when the region is active.
    ///
    /// # Errors
    ///
    /// Returns [`WorldReadError::TooLarge`] when the summary exceeds
    /// [`MAX_REGION_PAGE_BYTES`].
    pub fn region_summary(&self, region: Region) -> Result<Option<RegionSummaryRead>, WorldReadError> {
        let Some(info) = self.regions.get(&region) else { return Ok(None) };
        if region_summary_bytes(info) > MAX_REGION_PAGE_BYTES {
            return Err(WorldReadError::TooLarge);
        }
        Ok(Some(RegionSummaryRead { hydration_revision: self.revision(), summary: RegionSummary { region, info: info.clone() } }))
    }

    /// Copies a complete object detail value at the current revision.
    pub fn object_detail(&self, entity: Entity) -> Result<ObjectDetail, WorldReadError> {
        self.object_detail_at(entity, self.revision())
    }

    /// Hydrates a bounded set of current object incarnations at one revision.
    ///
    /// A missing or oversized object is reported in its corresponding batch entry. A foreign
    /// World identity rejects the entire request.
    ///
    /// # Errors
    ///
    /// Returns [`WorldReadError::InvalidLimit`] for an empty request or more than
    /// [`MAX_OBJECT_DETAIL_BATCH_ITEMS`] entities, and [`WorldReadError::StaleSession`] if any
    /// entity belongs to another World session.
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
            render_materials: object.render_materials().cloned(),
            gltf_material_overrides: object.gltf_material_overrides().cloned(),
            avatar_appearance: object.avatar_appearance().cloned(),
            attachment_point: object.attachment_point(),
            sit_target: object.sit_target(),
            properties: object.properties(),
            properties_provenance: object.properties_provenance(),
            object_media: object.object_media().cloned(),
        };
        Ok(detail)
    }

    /// Pulls a bounded ordered page of changes after `cursor`.
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
                    return Ok(WorldChangePage { observed_revision: self.revision(), records, cursor: next_cursor, has_more });
                }
            }
            next_cursor = WorldChangeCursor { world: self.id, through_revision: entry.revision, next_change: None };
            entry_index += 1;
            change_index = 0;
        }

        Ok(WorldChangePage { observed_revision: self.revision(), records, cursor: next_cursor, has_more: false })
    }

    fn summary_for_id(&self, id: UUID) -> ObjectSummary {
        let entity = self.entities.get(&id).copied().expect("object ordering index contains only live object IDs");
        self.summary_for_entity(entity).expect("UUID index contains only live entities")
    }

    fn list_item_for_id(&self, id: UUID) -> ObjectListItem {
        let entity = self.entities.get(&id).copied().expect("object ordering index contains only live object IDs");
        self.list_item_for_entity(entity).expect("UUID index contains only live entities")
    }

    fn list_item_for_entity(&self, entity: Entity) -> Result<ObjectListItem, WorldReadError> {
        if entity.1 != self.id {
            return Err(WorldReadError::StaleSession);
        }
        let object = self.record(entity).ok_or(WorldReadError::StaleIdentity)?;
        Ok(ObjectListItem { entity, id: object.id, kind: object.kind, address: object.state.binding.address })
    }

    fn summary_for_entity(&self, entity: Entity) -> Result<ObjectSummary, WorldReadError> {
        if entity.1 != self.id {
            return Err(WorldReadError::StaleSession);
        }
        let object = self.record(entity).ok_or(WorldReadError::StaleIdentity)?;
        Ok(ObjectSummary {
            entity,
            id: object.id,
            kind: object.kind,
            address: object.state.binding.address,
            parent: object.state.parent,
            local_transform: object.state.transform,
            pose: self.resolve_pose(entity),
        })
    }
}

fn region_summary_bytes(info: &RegionInfo) -> usize {
    size_of::<RegionSummary>().saturating_add(info.name.as_ref().map_or(0, String::len))
}

fn object_detail_bytes(object: &Object<'_>) -> usize {
    let surface_bytes = object.surface_appearance().map_or(0, |value| {
        size_of::<SurfaceAppearance>().saturating_add(value.overrides().count().saturating_mul(size_of::<(u8, FaceAppearance)>()))
    });
    let render_bytes = object.render_materials().map_or(0, |value| {
        size_of::<RenderMaterials>().saturating_add(value.bindings().count().saturating_mul(size_of::<(u8, AssetId)>()))
    });
    let gltf_bytes = object.gltf_material_overrides().map_or(0, |value| {
        size_of::<GltfMaterialOverrides>().saturating_add(value.faces().len().saturating_mul(size_of::<(u8, GltfMaterialOverride)>()))
    });
    let avatar_bytes = object.avatar_appearance().map_or(0, |value| {
        size_of::<AvatarAppearance>()
            .saturating_add(value.visual_params().count().saturating_mul(size_of::<(AvatarVisualParamIndex, u8)>()))
            .saturating_add(value.texture_slots().map_or(0, |_| size_of::<AvatarTextureSlots>()))
    });

    size_of::<ObjectDetail>()
        .saturating_add(surface_bytes)
        .saturating_add(render_bytes)
        .saturating_add(gltf_bytes)
        .saturating_add(avatar_bytes)
        .saturating_add(object.object_media().map_or(0, |media| media.retained_bytes().saturating_sub(size_of::<ObjectMedia>())))
}
