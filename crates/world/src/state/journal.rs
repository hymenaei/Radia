use super::{Applied, ApplyError, ApplyOutcome, Entity, LocalAddress, Recovery, Region, Rejections, World, WorldRevision};
use base::UUID;
use std::{
    collections::VecDeque,
    ops::{BitOr, BitOrAssign},
};

#[cfg(test)]
use super::{OrderEvidence, ParentUpdate};

/// Why an entity left the current World.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntityRemovalReason {
    Killed,
    AddressReplaced,
    ParentCascade,
    RegionRetired,
}

/// Coarse categories used to decide which consumer-side values need hydration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ChangeClasses(u8);

impl ChangeClasses {
    pub const TRANSFORM_MOTION: Self = Self(1 << 0);
    pub const RELATIONSHIP: Self = Self(1 << 1);
    pub const SHAPE: Self = Self(1 << 2);
    pub const APPEARANCE: Self = Self(1 << 3);
    pub const PROPERTIES: Self = Self(1 << 4);

    /// Returns whether no categories are present.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns whether every category in `classes` is present.
    pub const fn contains(self, classes: Self) -> bool {
        self.0 & classes.0 == classes.0
    }
}

impl BitOr for ChangeClasses {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for ChangeClasses {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Ordered World invalidations and lifecycle facts retained by the journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Change {
    EntityAdded {
        id: UUID,
        entity: Entity,
        address: LocalAddress,
    },
    EntityUpdated {
        id: UUID,
        entity: Entity,
        classes: ChangeClasses,
    },
    SubtreeInvalidated {
        id: UUID,
        entity: Entity,
    },
    EntityRemoved {
        id: UUID,
        entity: Entity,
        address: LocalAddress,
        reason: EntityRemovalReason,
    },
    /// An entity kept its UUID and moved to a different simulator-local address.
    EntityRebound {
        id: UUID,
        entity: Entity,
        from: LocalAddress,
        to: LocalAddress,
    },
    EntityTransferred {
        id: UUID,
        entity: Entity,
        from: LocalAddress,
        to: LocalAddress,
    },
    RegionAdopted(Region),
    RegionUpdated(Region),
    RegionRetired {
        region: Region,
        entities_removed: usize,
    },
}

#[derive(Debug)]
#[allow(dead_code)]
pub(super) struct JournalEntry {
    pub(super) revision: WorldRevision,
    pub(super) changes: Box<[Change]>,
}

#[derive(Debug)]
pub(super) struct Journal {
    pub(super) revision: WorldRevision,
    pub(super) entries: VecDeque<JournalEntry>,
    retained_changes: usize,
    pub(super) lost_through: Option<WorldRevision>,
}

impl Journal {
    pub(super) fn new() -> Self {
        Self { revision: WorldRevision(0), entries: VecDeque::new(), retained_changes: 0, lost_through: None }
    }

    fn note_loss_through(&mut self, revision: WorldRevision) {
        self.lost_through = Some(self.lost_through.map_or(revision, |lost| lost.max(revision)));
    }
}

impl World {
    pub(super) fn ensure_revision_available(&self) -> Result<(), ApplyError> {
        self.journal.revision.0.checked_add(1).map(|_| ()).ok_or(ApplyError::RevisionExhausted)
    }

    pub(super) fn finish(&mut self, outcome: ApplyOutcome, changes: Vec<Change>, recovery: Option<Recovery>) -> Applied {
        let revision = if changes.is_empty() {
            None
        } else {
            let value = self.journal.revision.0.checked_add(1).expect("observable mutations preflight journal revision");
            let next = WorldRevision(value);
            self.journal.revision = next;
            let change_count = changes.len();
            if self.limits.max_journal_entries == 0 || change_count > self.limits.max_journal_changes {
                self.journal.note_loss_through(next);
            } else {
                while self.journal.entries.len() >= self.limits.max_journal_entries
                    || self.journal.retained_changes.saturating_add(change_count) > self.limits.max_journal_changes
                {
                    if let Some(old) = self.journal.entries.pop_front() {
                        self.journal.retained_changes -= old.changes.len();
                        self.journal.note_loss_through(old.revision);
                    } else {
                        break;
                    }
                }
                if self.journal.retained_changes.saturating_add(change_count) <= self.limits.max_journal_changes {
                    self.journal.retained_changes += change_count;
                    self.journal.entries.push_back(JournalEntry { revision: next, changes: changes.into_boxed_slice() });
                } else {
                    self.journal.note_loss_through(next);
                }
            }
            Some(next)
        };
        Applied { outcome, revision, recovery, recovery_cancellations: Vec::new(), rejections: Rejections::default() }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::Change::{
        EntityAdded, EntityRebound, EntityRemoved, EntityTransferred, EntityUpdated, RegionAdopted, RegionRetired, RegionUpdated,
        SubtreeInvalidated,
    };
    use OrderEvidence::Ordered;
    use ParentUpdate::{Local, Root};
    use base::UUID;
    use math::Vec3;

    use super::*;
    use crate::{
        Appearance, AssetId, FaceAppearance, Kind, LocalAddress, ObjectObservation, ObjectPatch, Pose, PropertiesPatch, RegionInfo,
        RenderMaterials, SculptFlags, Shape, SurfaceAppearance, UpdateSource, WorldLimits,
    };

    fn limits(max_journal_entries: usize) -> WorldLimits {
        WorldLimits {
            max_region_registrations: 4,
            max_entities: 8,
            max_entities_per_region: 8,
            max_tombstones_per_region: 8,
            max_tombstones: 8,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(8),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(8),
            max_journal_entries,
            max_journal_changes: 8,
            max_region_name_bytes: 8,
            max_recovery_targets: 1,
        }
    }

    fn observation(region: Region, order: u64, id: u128, local_id: u32) -> ObjectObservation {
        ObjectObservation {
            source: UpdateSource::new(region, order as u32, Ordered(order), 0),
            id: UUID::from_u128(id),
            local_id: NonZeroU32::new(local_id).unwrap(),
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
        }
    }

    fn last_changes(world: &World) -> &[Change] {
        &world.journal.entries.back().expect("observable changes are journaled").changes
    }

    fn assert_revision(applied: Applied) {
        assert!(applied.revision.is_some());
    }

    #[test]
    fn revision_exhaustion_rejects_update_atomically() {
        let region = Region::new(21);
        let mut world = World::new(limits(4));
        assert_eq!(world.adopt_region(region).unwrap().outcome, ApplyOutcome::RegionAdopted);
        world.journal.revision = WorldRevision(u64::MAX);

        let result = world
            .observe_object(ObjectObservation {
                source: UpdateSource::new(region, 1, Ordered(1), 0),
                id: UUID::from_u128(21),
                local_id: NonZeroU32::new(1).unwrap(),
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
            .unwrap_err();

        assert_eq!(result, ApplyError::RevisionExhausted);
        assert_eq!(world.entity(UUID::from_u128(21)), None);
        assert_eq!(world.entity_at(LocalAddress::new(region, NonZeroU32::new(1).unwrap())), None);
        assert!(world.objects().next().is_none());
        assert_eq!(world.revision(), WorldRevision(u64::MAX));
        assert_eq!(world.journal.entries.len(), 1);
        assert_eq!(world.journal.entries.front().unwrap().revision, WorldRevision(1));
        assert_eq!(world.journal.lost_through, None);
    }

    #[test]
    fn noop_updates_need_no_revision() {
        let region = Region::new(27);
        let mut world = World::new(limits(4));
        assert_revision(world.adopt_region(region).unwrap());
        let mut initial = observation(region, 1, 270, 1);
        initial.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
        assert_revision(world.observe_object(initial).unwrap());
        let owner_id = UUID::from_u128(271);
        assert_revision(
            world
                .patch_properties(
                    UpdateSource::new(region, 1, Ordered(1), 0),
                    UUID::from_u128(270),
                    PropertiesPatch { owner_id: Some(owner_id), creator_id: None },
                )
                .unwrap(),
        );
        assert!(matches!(
            last_changes(&world),
            [EntityUpdated { id, classes, .. }]
                if *id == UUID::from_u128(270) && classes.contains(ChangeClasses::PROPERTIES)
        ));

        world.journal.revision = WorldRevision(u64::MAX);
        let stale = ObjectPatch {
            pose: Some(Pose { position: Some(Vec3::Y), rotation: None }),
            ..ObjectPatch::new(UpdateSource::new(region, 1, Ordered(1), 0), NonZeroU32::new(1).unwrap())
        };
        assert_eq!(world.patch_object(stale).unwrap().outcome, ApplyOutcome::Unchanged);

        let same_pose = ObjectPatch {
            pose: Some(Pose { position: Some(Vec3::X), rotation: None }),
            ..ObjectPatch::new(UpdateSource::new(region, 2, Ordered(2), 0), NonZeroU32::new(1).unwrap())
        };
        assert_eq!(world.patch_object(same_pose).unwrap().outcome, ApplyOutcome::Unchanged);
        assert_eq!(
            world
                .patch_properties(
                    UpdateSource::new(region, 2, Ordered(2), 0),
                    UUID::from_u128(270),
                    PropertiesPatch { owner_id: Some(owner_id), creator_id: None },
                )
                .unwrap()
                .outcome,
            ApplyOutcome::Unchanged
        );
        assert_eq!(world.observe_object(observation(region, 3, 270, 1)).unwrap().outcome, ApplyOutcome::Unchanged);
        assert_eq!(world.revision(), WorldRevision(u64::MAX));
    }

    #[test]
    fn surface_changes_emit_appearance() {
        let region = Region::new(28);
        let mut world = World::new(limits(4));
        assert_revision(world.adopt_region(region).unwrap());
        let added = world.observe_object(observation(region, 1, 280, 1)).unwrap();
        let ApplyOutcome::Introduced(entity) = added.outcome else { panic!("expected object introduction") };
        let appearance =
            SurfaceAppearance::new(FaceAppearance { texture_id: UUID::from_u128(281), ..FaceAppearance::default() }, Vec::new()).unwrap();

        let applied = world
            .patch_object(ObjectPatch {
                surface_appearance: Some(appearance),
                ..ObjectPatch::new(UpdateSource::new(region, 2, Ordered(2), 0), NonZeroU32::new(1).unwrap())
            })
            .unwrap();

        assert_eq!(applied.outcome, ApplyOutcome::Updated(entity));
        assert!(matches!(
            last_changes(&world),
            [EntityUpdated { id, classes, .. }]
                if *id == UUID::from_u128(280)
                    && classes.contains(ChangeClasses::APPEARANCE)
                    && !classes.contains(ChangeClasses::SHAPE)
        ));
    }

    #[test]
    fn render_material_changes_emit_appearance() {
        let region = Region::new(28);
        let mut world = World::new(limits(4));
        assert_revision(world.adopt_region(region).unwrap());
        let added = world.observe_object(observation(region, 1, 282, 1)).unwrap();
        let ApplyOutcome::Introduced(entity) = added.outcome else { panic!("expected object introduction") };
        let materials = RenderMaterials::new(vec![(3, AssetId::new(UUID::from_u128(283)))]).unwrap();

        let applied = world
            .patch_object(ObjectPatch {
                render_materials: Some(materials),
                ..ObjectPatch::new(UpdateSource::new(region, 2, Ordered(2), 0), NonZeroU32::new(1).unwrap())
            })
            .unwrap();

        assert_eq!(applied.outcome, ApplyOutcome::Updated(entity));
        assert!(matches!(
            last_changes(&world),
            [EntityUpdated { id, classes, .. }]
                if *id == UUID::from_u128(282)
                    && classes.contains(ChangeClasses::APPEARANCE)
                    && !classes.contains(ChangeClasses::SHAPE)
        ));
    }

    #[test]
    fn bounded_journal_marks_the_last_lost_revision() {
        let region = Region::new(22);
        let mut world = World::new(limits(1));
        assert_eq!(world.adopt_region(region).unwrap().outcome, ApplyOutcome::RegionAdopted);
        let first_revision = world.revision();

        assert_eq!(
            world.patch_region(region, RegionInfo { name: Some("region".to_owned()), ..RegionInfo::default() }).unwrap().outcome,
            ApplyOutcome::RegionPatched
        );

        assert_eq!(world.journal.entries.len(), 1);
        assert_eq!(world.journal.entries.front().unwrap().revision, world.revision());
        assert_eq!(world.journal.lost_through, Some(first_revision));
    }

    #[test]
    fn journal_change_budget_evicts_oldest_entries() {
        let region = Region::new(28);
        let mut limits = limits(4);
        limits.max_journal_changes = 1;
        let mut world = World::new(limits);

        assert_revision(world.adopt_region(region).unwrap());
        let first_revision = world.revision();
        assert_revision(world.patch_region(region, RegionInfo { name: Some("region".to_owned()), ..RegionInfo::default() }).unwrap());

        assert_eq!(world.journal.entries.len(), 1);
        assert_eq!(world.journal.retained_changes, 1);
        assert_eq!(world.journal.entries.front().unwrap().revision, world.revision());
        assert_eq!(world.journal.lost_through, Some(first_revision));
    }

    #[test]
    fn oversized_journal_entry_still_applies_mutation() {
        let region = Region::new(29);
        let mut limits = limits(8);
        limits.max_journal_changes = 2;
        let mut world = World::new(limits);
        assert_revision(world.adopt_region(region).unwrap());
        assert_revision(world.observe_object(observation(region, 1, 290, 1)).unwrap());
        assert_revision(world.observe_object(observation(region, 2, 291, 2)).unwrap());
        let before_retirement = world.revision();

        let retired = world.retire_region(region).unwrap();

        assert_eq!(retired.revision, Some(WorldRevision(before_retirement.get() + 1)));
        assert_eq!(retired.outcome, ApplyOutcome::RegionRetired { entities_removed: 2 });
        assert_eq!(world.region(region), None);
        assert!(world.objects().next().is_none());
        assert!(world.journal.retained_changes <= 2);
        assert_eq!(world.journal.lost_through, retired.revision);
    }

    #[test]
    fn journal_records_add_rebind_transfer() {
        let first_region = Region::new(23);
        let second_region = Region::new(24);
        let mut world = World::new(limits(8));
        assert_revision(world.adopt_region(first_region).unwrap());
        assert_revision(world.adopt_region(second_region).unwrap());
        assert!(matches!(last_changes(&world), [RegionAdopted(region)] if *region == second_region));
        assert_revision(world.patch_region(first_region, RegionInfo { name: Some("first".to_owned()), ..RegionInfo::default() }).unwrap());
        assert!(matches!(last_changes(&world), [RegionUpdated(region)] if *region == first_region));

        let first_address = LocalAddress::new(first_region, NonZeroU32::new(1).unwrap());
        assert_revision(world.observe_object(observation(first_region, 1, 230, 1)).unwrap());
        assert!(
            matches!(last_changes(&world), [EntityAdded { id, address, .. }] if *id == UUID::from_u128(230) && *address == first_address)
        );

        let rebound_address = LocalAddress::new(first_region, NonZeroU32::new(2).unwrap());
        assert_revision(world.observe_object(observation(first_region, 2, 230, 2)).unwrap());
        assert!(last_changes(&world).iter().any(|change| {
            matches!(change, EntityRebound { id, from, to, .. } if *id == UUID::from_u128(230) && *from == first_address && *to == rebound_address)
        }));

        let transfer_address = LocalAddress::new(second_region, NonZeroU32::new(3).unwrap());
        assert_revision(world.observe_object(observation(second_region, 1, 230, 3)).unwrap());
        assert!(last_changes(&world).iter().any(|change| {
            matches!(change, EntityTransferred { id, from, to, .. } if *id == UUID::from_u128(230) && *from == rebound_address && *to == transfer_address)
        }));
    }

    #[test]
    fn journal_records_kill_and_cascade_reasons() {
        let region = Region::new(25);
        let mut world = World::new(limits(8));
        assert_revision(world.adopt_region(region).unwrap());
        assert_revision(world.observe_object(observation(region, 1, 250, 1)).unwrap());
        let mut child = observation(region, 2, 251, 2);
        child.parent = Local(NonZeroU32::new(1).unwrap());
        assert_revision(world.observe_object(child).unwrap());
        let mut avatar = observation(region, 3, 252, 3);
        avatar.kind = Kind::Avatar;
        avatar.parent = Local(NonZeroU32::new(1).unwrap());
        assert_revision(world.observe_object(avatar).unwrap());
        let mut attachment = observation(region, 4, 253, 4);
        attachment.parent = Local(NonZeroU32::new(3).unwrap());
        assert_revision(world.observe_object(attachment).unwrap());

        assert_revision(world.remove_object(UpdateSource::new(region, 5, Ordered(5), 0), NonZeroU32::new(1).unwrap()).unwrap());

        let changes = last_changes(&world);
        assert!(changes.iter().any(|change| {
            matches!(change, EntityRemoved { id, address, reason: EntityRemovalReason::Killed, .. }
                if *id == UUID::from_u128(250) && address.local_id().get() == 1)
        }));
        assert!(changes.iter().any(|change| {
            matches!(change, EntityRemoved { id, address, reason: EntityRemovalReason::ParentCascade, .. }
                if *id == UUID::from_u128(251) && address.local_id().get() == 2)
        }));
        assert!(changes.iter().any(|change| {
            matches!(change, EntityUpdated { id, classes, .. }
                if *id == UUID::from_u128(252)
                    && classes.contains(ChangeClasses::RELATIONSHIP | ChangeClasses::TRANSFORM_MOTION))
        }));
        assert!(changes.iter().any(|change| matches!(change, SubtreeInvalidated { id, .. } if *id == UUID::from_u128(252))));
    }

    #[test]
    fn relationships_invalidate_transform_motion() {
        let region = Region::new(32);
        let mut world = World::new(limits(8));
        assert_revision(world.adopt_region(region).unwrap());
        assert_revision(world.observe_object(observation(region, 1, 320, 1)).unwrap());
        assert_revision(world.observe_object(observation(region, 2, 321, 2)).unwrap());

        let mut link = ObjectPatch::new(UpdateSource::new(region, 3, Ordered(3), 0), NonZeroU32::new(2).unwrap());
        link.parent = Some(Local(NonZeroU32::new(1).unwrap()));
        assert_revision(world.patch_object(link).unwrap());

        assert!(last_changes(&world).iter().any(|change| {
            matches!(change, EntityUpdated { id, classes, .. }
                if *id == UUID::from_u128(321)
                    && classes.contains(ChangeClasses::RELATIONSHIP | ChangeClasses::TRANSFORM_MOTION))
        }));
    }

    #[test]
    fn shape_and_appearance_journal_separately() {
        let region = Region::new(34);
        let mut world = World::new(limits(8));
        assert_revision(world.adopt_region(region).unwrap());
        let mut object = observation(region, 1, 340, 1);
        object.shape = Some(Some(Shape::Prim));
        object.appearance = Some(Some(Appearance::Surface));
        assert_revision(world.observe_object(object).unwrap());

        let appearance = ObjectPatch {
            appearance: Some(Some(Appearance::Avatar)),
            ..ObjectPatch::new(UpdateSource::new(region, 3, Ordered(3), 0), NonZeroU32::new(1).unwrap())
        };
        assert_revision(world.patch_object(appearance).unwrap());
        assert!(matches!(
            last_changes(&world),
            [EntityUpdated { id, classes, .. }]
                if *id == UUID::from_u128(340) && classes.contains(ChangeClasses::APPEARANCE)
                    && !classes.contains(ChangeClasses::SHAPE)
        ));

        let mixed_freshness = ObjectPatch {
            shape: Some(Some(Shape::Mesh { asset_id: AssetId::new(UUID::from_u128(1)), flags: SculptFlags::default() })),
            appearance: Some(Some(Appearance::Surface)),
            ..ObjectPatch::new(UpdateSource::new(region, 2, Ordered(2), 0), NonZeroU32::new(1).unwrap())
        };
        assert_revision(world.patch_object(mixed_freshness).unwrap());
        assert!(matches!(
            last_changes(&world),
            [EntityUpdated { id, classes, .. }]
                if *id == UUID::from_u128(340) && classes.contains(ChangeClasses::SHAPE)
                    && !classes.contains(ChangeClasses::APPEARANCE)
        ));

        let asset_change = ObjectPatch {
            shape: Some(Some(Shape::Mesh { asset_id: AssetId::new(UUID::from_u128(2)), flags: SculptFlags::default() })),
            ..ObjectPatch::new(UpdateSource::new(region, 4, Ordered(4), 0), NonZeroU32::new(1).unwrap())
        };
        assert_revision(world.patch_object(asset_change).unwrap());
        assert!(matches!(
            last_changes(&world),
            [EntityUpdated { id, classes, .. }]
                if *id == UUID::from_u128(340) && classes.contains(ChangeClasses::SHAPE)
                    && !classes.contains(ChangeClasses::APPEARANCE)
        ));
    }

    #[test]
    fn parent_pose_change_invalidates_descendant_subtree() {
        let region = Region::new(31);
        let mut world = World::new(limits(8));
        assert_revision(world.adopt_region(region).unwrap());
        assert_revision(world.observe_object(observation(region, 1, 310, 1)).unwrap());

        let mut child = observation(region, 2, 311, 2);
        child.parent = Local(NonZeroU32::new(1).unwrap());
        assert_revision(world.observe_object(child).unwrap());

        let mut grandchild = observation(region, 3, 312, 3);
        grandchild.parent = Local(NonZeroU32::new(2).unwrap());
        assert_revision(world.observe_object(grandchild).unwrap());

        let mut movement = ObjectPatch::new(UpdateSource::new(region, 4, Ordered(4), 0), NonZeroU32::new(1).unwrap());
        movement.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
        assert_revision(world.patch_object(movement).unwrap());

        assert!(matches!(
            last_changes(&world),
            [EntityUpdated { id, classes, .. }, SubtreeInvalidated { id: root, .. }]
                if *id == UUID::from_u128(310)
                    && classes.contains(ChangeClasses::TRANSFORM_MOTION)
                    && *root == UUID::from_u128(310)
        ));
    }

    #[test]
    fn journal_records_replacement_and_retirement() {
        let region = Region::new(26);
        let mut world = World::new(limits(8));
        assert_revision(world.adopt_region(region).unwrap());
        assert_revision(world.observe_object(observation(region, 1, 260, 1)).unwrap());
        assert_revision(world.observe_object(observation(region, 2, 261, 1)).unwrap());
        assert!(last_changes(&world).iter().any(|change| {
            matches!(change, EntityRemoved { id, reason: EntityRemovalReason::AddressReplaced, .. } if *id == UUID::from_u128(260))
        }));

        let entity_address = LocalAddress::new(region, NonZeroU32::new(1).unwrap());
        let mut child = observation(region, 3, 262, 2);
        child.parent = Local(NonZeroU32::new(1).unwrap());
        assert_revision(world.observe_object(child).unwrap());
        assert_revision(world.retire_region(region).unwrap());
        let changes = last_changes(&world);
        assert!(changes.iter().any(|change| {
            matches!(change, EntityRemoved { id, address, reason: EntityRemovalReason::RegionRetired, .. }
                if *id == UUID::from_u128(261) && *address == entity_address)
        }));
        assert!(changes.iter().any(|change| {
            matches!(change, EntityRemoved { id, reason: EntityRemovalReason::RegionRetired, .. } if *id == UUID::from_u128(262))
        }));
        assert!(matches!(changes.last(), Some(RegionRetired { region: retired, entities_removed: 2 }) if *retired == region));
    }
}
