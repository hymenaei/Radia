use uuid::Uuid;

use super::Effects;
use super::fields::{Draft, Fields, Origin, Settlement, normalized_rotation, validate_uuid, validate_vec3};
use crate::history::WorldChange::EntityUpdated;
use crate::history::WorldChangeClasses;
use crate::object::relationships::ParentResolution;
use crate::update::FieldStamp;
use crate::{
    Applied, ApplyError, ApplyOutcome, AvatarAppearance, DeferredReason, FieldGroup, FieldRejectionReason, LocalAddress,
    ObjectBindingTarget, ObjectPatch, PropertiesPatch, PropertyProvenance, Recovery, Rejections, SitTarget, UpdateAuthority, UpdateSource,
};
use crate::{ObjectContent, RegionalStamp, World, newer_field, newer_region_field};
use FieldRejectionReason::UntrustedSource;
use UpdateAuthority::{Tracked, Untracked};

const UNKNOWN_COF_VERSION: i32 = -1;

impl World {
    pub fn patch_object(&mut self, patch: ObjectPatch) -> Result<Applied, ApplyError> {
        let source = patch.source;
        self.require_active_region(source.region)?;
        let address = LocalAddress::new(source.region, patch.local_id);
        let Some(stamp) = source.stamp() else {
            return Ok(self.defer(DeferredReason::AmbiguousOrder, address));
        };
        let Some(entity) = self.entity_at(address) else {
            return Ok(self.unknown_address(address, stamp));
        };
        let origin = Origin { source, stamp };
        let (parent_update, mut fields) = Fields::patched(patch);
        let advance_binding = parent_update.is_some() || fields.orders_binding();
        let mut read = self.store.read(entity.0);
        let (record, content, ordering) = read.get().expect("address index contains a complete entity");
        let content = content.unwrap_or(&ObjectContent::EMPTY);
        let (id, kind, before) = (record.id, record.kind, record.state);

        let mut rejections = fields.validate(before.transform.rotation());
        let appearance = fields.next_appearance(content.appearance, &ordering.fences, origin);
        fields.reject_unsupported(kind, appearance, &mut rejections);
        if self.self_agent == Some(id)
            && let Some(update) = &fields.avatar_appearance
        {
            let current = content.avatar_appearance.as_deref().and_then(AvatarAppearance::cof_version).unwrap_or(UNKNOWN_COF_VERSION);
            if update.cof_version().unwrap_or(UNKNOWN_COF_VERSION) <= current {
                rejections.reject(FieldGroup::AvatarAppearance, FieldRejectionReason::StaleVersion);
                fields.avatar_appearance = None;
            }
        }
        let parent_admitted = newer_field(origin.field(), ordering.fences.parent) && ordering.admits_parent(source, stamp);
        let (parent, parent_cycle) = match parent_update.filter(|_| parent_admitted) {
            Some(update) => match self.resolve_parent(source.region, address.local_id, update, origin.field(), Some(entity)) {
                ParentResolution::Accepted(parent) => (Some(parent), false),
                ParentResolution::Cycle => (None, true),
            },
            None => (None, false),
        };
        let untrusted_parent = source.authority() == Untracked && parent_update.is_some() && ordering.protects_parent();
        fields.reject_parent(parent_cycle, untrusted_parent, &mut rejections);
        if source.authority() == Untracked {
            fields.reject_untrusted(&ordering.fences, &mut rejections);
        }
        let next_parent = parent.unwrap_or(before.parent);
        let next_attachment = fields.next_attachment_point(before.attachment_point, &ordering.fences, origin);
        let frame_changed = self.relationship_frame_changed(before.parent, next_parent, before.attachment_point, next_attachment);
        let detach_snapshot = parent.and_then(|next| self.detach_snapshot(entity, kind, before.parent, next));

        let mut draft = Draft::new(before, *ordering, content);
        let frame = draft.plan_frame(frame_changed, origin)?;
        let pose_accepted = draft.accept_pose(&fields, origin, frame, &mut rejections);
        let pose_affects_descendants = pose_accepted && draft.pose_changes(fields.pose);
        let settlement =
            Settlement { frame, pose_accepted, detach_snapshot, parent_applied: parent.is_some(), parent_cycle, advance_binding };
        draft.update(fields, kind, origin, settlement, self.object_media_bytes, &mut rejections);
        if draft.is_unchanged(&before, ordering) {
            return Ok(Applied::unchanged().with_rejections(rejections));
        }
        let change_classes = draft.change_classes(&before, next_parent);
        if !change_classes.is_empty() {
            self.ensure_revision_available()?;
        }
        let media_bytes = draft.media_bytes();
        let (state, content, ordering) = draft.into_parts();
        drop(read);

        self.store.commit(entity, state, content, ordering);
        if let Some((old, new)) = media_bytes {
            self.object_media_bytes = self.object_media_bytes.saturating_sub(old).saturating_add(new);
        }
        let mut effects = Effects::default();
        if let Some(parent) = parent {
            self.apply_parent(entity, parent, &mut effects);
        }
        if !change_classes.is_empty() {
            effects.changes.push(EntityUpdated { id, entity, classes: change_classes });
        }
        if frame_changed || parent.is_some_and(|parent| parent != before.parent) || pose_affects_descendants {
            self.invalidate_subtree(entity, &mut effects);
        }
        let outcome = if change_classes.is_empty() { ApplyOutcome::Unchanged } else { ApplyOutcome::Updated(entity) };
        Ok(self.finish(outcome, effects).with_rejections(rejections).with_recovery_satisfied(parent.is_some()))
    }

    pub fn patch_properties(
        &mut self,
        source: UpdateSource,
        target: ObjectBindingTarget,
        response_id: Uuid,
        patch: PropertiesPatch,
    ) -> Result<Applied, ApplyError> {
        self.require_active_region(source.region)?;
        validate_uuid(response_id, "object_id")?;
        if response_id != target.id || target.address.region != source.region || !self.object_binding_target_is_current(target) {
            return Ok(Applied::new(ApplyOutcome::IgnoredStale));
        }
        let Some(stamp) = source.stamp() else {
            return Ok(self.defer(DeferredReason::AmbiguousOrder, target.address));
        };
        let entity = target.entity;
        let (mut state, mut content, ordering) = {
            let object = self.object(entity).expect("current properties target names a live entity");
            (object.record.state, object.content().clone(), *object.ordering())
        };
        let properties = content.properties;
        let provenance = content.properties_provenance;
        let field_stamp = FieldStamp { authority: source.authority(), stamp };
        let mut rejections = Rejections::default();
        let untrusted = |value: Option<Uuid>, fence: Option<PropertyProvenance>| {
            value.is_some() && source.authority() == Untracked && fence.is_some_and(|fence| fence.authority == Tracked)
        };
        let owner_untrusted = untrusted(patch.owner_id, provenance.owner);
        let creator_untrusted = untrusted(patch.creator_id, provenance.creator);
        if owner_untrusted || creator_untrusted {
            rejections.reject(FieldGroup::Properties, UntrustedSource);
        }
        let newer = |value: Option<Uuid>, untrusted: bool, fence: Option<PropertyProvenance>| {
            value.is_some() && !untrusted && newer_region_field(source.region, field_stamp, fence.map(RegionalStamp::from))
        };
        let owner_is_newer = newer(patch.owner_id, owner_untrusted, provenance.owner);
        let creator_is_newer = newer(patch.creator_id, creator_untrusted, provenance.creator);
        let next_provenance = PropertyProvenance {
            region: source.region,
            authority: source.authority(),
            order: stamp.order,
            block_ordinal: stamp.block_ordinal,
            source_sequence: source.sequence(),
        };
        let observable_change = (owner_is_newer && (properties.owner_id != patch.owner_id || provenance.owner != Some(next_provenance)))
            || (creator_is_newer && (properties.creator_id != patch.creator_id || provenance.creator != Some(next_provenance)));
        if observable_change {
            self.ensure_revision_available()?;
        }
        if owner_is_newer {
            content.properties.owner_id = patch.owner_id;
            content.properties_provenance.owner = Some(next_provenance);
        }
        if creator_is_newer {
            content.properties.creator_id = patch.creator_id;
            content.properties_provenance.creator = Some(next_provenance);
        }
        if patch.owner_id.is_some() || patch.creator_id.is_some() {
            state.binding.advance_fence(source, stamp);
        }
        self.store.commit(entity, state, Some(content), ordering);
        let (outcome, changes) = if observable_change {
            (ApplyOutcome::Updated(entity), vec![EntityUpdated { id: response_id, entity, classes: WorldChangeClasses::PROPERTIES }])
        } else {
            (ApplyOutcome::Unchanged, Vec::new())
        };
        Ok(self.finish(outcome, Effects::from_changes(changes)).with_rejections(rejections))
    }

    pub fn patch_sit_target(&mut self, source: UpdateSource, id: Uuid, target: Option<SitTarget>) -> Result<Applied, ApplyError> {
        self.require_active_region(source.region)?;
        validate_uuid(id, "object_id")?;
        let entity = self.entity(id);
        let Some(stamp) = source.stamp() else {
            let address = entity.and_then(|entity| self.object(entity)).map(|object| object.address());
            return Ok(match address.filter(|address| address.region == source.region) {
                Some(address) => self.defer(DeferredReason::AmbiguousOrder, address),
                None => Applied::deferred(DeferredReason::AmbiguousOrder, Recovery::default()),
            });
        };
        let Some(entity) = entity else {
            return Ok(Applied::deferred(DeferredReason::UnknownObject, Recovery::default()));
        };
        let (mut state, mut content, mut ordering) = {
            let object = self.object(entity).expect("UUID index contains a live entity");
            if object.address().region != source.region {
                return Ok(Applied::unchanged());
            }
            (object.record.state, object.content().clone(), *object.ordering())
        };

        let mut rejections = Rejections::default();
        let target = match target {
            Some(mut target) => {
                let rotation = normalized_rotation(target.rotation);
                if let Some(reason) = validate_vec3(Some(target.position), "sit_position").err().or_else(|| rotation.err()) {
                    rejections.reject(FieldGroup::SitTarget, reason);
                    return Ok(Applied::unchanged().with_rejections(rejections));
                }
                target.rotation = rotation.expect("sit-target rotation was validated");
                Some(target)
            }
            None => None,
        };

        let fence = ordering.fences.sit_target;
        let field_stamp = FieldStamp { authority: source.authority(), stamp };
        let protected = source.authority() == Untracked && fence.is_some_and(|fence| fence.stamp.authority == Tracked);
        if protected {
            rejections.reject(FieldGroup::SitTarget, UntrustedSource);
        }
        if protected || !newer_region_field(source.region, field_stamp, fence) {
            return Ok(Applied::unchanged().with_rejections(rejections));
        }
        let observable_change = content.sit_target != target;
        if observable_change {
            self.ensure_revision_available()?;
        }
        content.sit_target = target;
        ordering.fences.sit_target = Some(RegionalStamp { region: source.region, stamp: field_stamp });
        state.binding.advance_fence(source, stamp);
        self.store.commit(entity, state, Some(content), ordering);
        let (outcome, changes) = if observable_change {
            (ApplyOutcome::Updated(entity), vec![EntityUpdated { id, entity, classes: WorldChangeClasses::RELATIONSHIP }])
        } else {
            (ApplyOutcome::Unchanged, Vec::new())
        };
        Ok(self.finish(outcome, Effects::from_changes(changes)).with_rejections(rejections))
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use math::Vec3;
    use uuid::Uuid;

    use crate::{
        Appearance, ApplyOutcome, ApplyOutcome::Introduced, FieldGroup, FieldRejection, FieldRejectionReason, FieldUpdate, Kind,
        LinearMotion, MediaEntry, ObjectMediaUpdate, ObjectObservation, ObjectPatch, OrderEvidence::Ordered, ParentUpdate::Root, Pose,
        Region, UpdateSource,
    };

    use crate::MAX_OBJECT_MEDIA_BYTES;
    use crate::mutation::tests::test_world;

    #[test]
    fn invalid_patch_keeps_valid_groups() {
        let region = Region::new(23);
        let mut world = test_world(&[region]);
        let source = UpdateSource::new(region, 1, Ordered(1), 0);
        let observation = ObjectObservation {
            pose: Some(Pose { position: Some(Vec3::X), rotation: None }),
            linear_motion: Some(LinearMotion::default()),
            scale: Some(Vec3::splat(2.0)),
            ..ObjectObservation::new(source, Uuid::from_u128(23), NonZeroU32::new(1).unwrap(), Kind::Object, Root)
        };
        let introduced = world.observe_object(observation).unwrap();
        let ApplyOutcome::Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };
        let revision = world.revision();

        let patch = ObjectPatch {
            pose: Some(Pose { position: Some(Vec3::Y), rotation: None }),
            scale: Some(Vec3::splat(f32::NAN)),
            ..ObjectPatch::new(UpdateSource::new(region, 2, Ordered(2), 0), NonZeroU32::new(1).unwrap())
        };

        let applied = world.patch_object(patch).unwrap();
        assert_eq!(applied.outcome, ApplyOutcome::Updated(entity));
        assert_eq!(applied.revision.unwrap().get(), revision.get() + 1);
        assert_eq!(
            applied.rejections.iter().copied().collect::<Vec<_>>(),
            [FieldRejection { group: FieldGroup::Scale, reason: FieldRejectionReason::InvalidValue { field: "scale" } }]
        );
        assert_eq!(world.revision().get(), revision.get() + 1);
        let object = world.object(entity).unwrap();
        assert_eq!(object.position(), Some(Vec3::Y));
        assert_eq!(object.scale(), Some(Vec3::splat(2.0)));
    }

    #[test]
    fn object_media_respects_retained_budget() {
        let region = Region::new(24);
        let mut world = test_world(&[region]);
        let local_id = NonZeroU32::new(1).unwrap();
        let object = ObjectObservation {
            appearance: FieldUpdate::Set(Appearance::Surface),
            ..ObjectObservation::new(UpdateSource::new(region, 1, Ordered(1), 0), Uuid::from_u128(24), local_id, Kind::Object, Root)
        };
        let introduced = world.observe_object(object).unwrap();
        let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };
        world.object_media_bytes = MAX_OBJECT_MEDIA_BYTES;

        let update = ObjectMediaUpdate::new(1, vec![Some(MediaEntry::default())]).unwrap();
        let result = world
            .patch_object(ObjectPatch {
                object_media: Some(update),
                ..ObjectPatch::new(UpdateSource::new(region, 3, Ordered(3), 0), local_id)
            })
            .unwrap();

        assert!(result.rejections.iter().any(|rejection| *rejection
            == FieldRejection { group: FieldGroup::ObjectMedia, reason: FieldRejectionReason::ResourceLimit { field: "object_media" } }));
        assert_eq!(world.object(entity).unwrap().object_media(), None);
        assert_eq!(world.object_media_bytes, MAX_OBJECT_MEDIA_BYTES);
    }
}
