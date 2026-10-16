use std::{
    collections::HashSet,
    num::{NonZeroU8, NonZeroU32},
    ptr::eq,
};

use base::UUID;
use math::{DVec3, Quat, Vec3};
use world::{
    Appearance, Applied,
    ApplyOutcome::{Introduced, RegionAdopted, RegionPatched, RegionRetired, Removed, Updated},
    AttachmentPoint, CoordinateFrame, Entity, Kind,
    Kind::{Avatar, Object},
    LinearMotion, LocalAddress, MAX_MEDIA_URL_BYTES, MAX_MEDIA_WHITELIST_BYTES, MAX_OBJECT_DETAIL_BATCH_ITEMS, MAX_REGION_PAGE_BYTES,
    MediaEntry, ObjectDetailResult, ObjectMediaUpdate, ObjectObservation, ObjectPatch,
    OrderEvidence::Ordered,
    Parent, ParentUpdate,
    ParentUpdate::{Local, Root},
    Pose, PoseState, PropertiesPatch, Region, RegionHandle, RegionInfo, UnresolvedReason, UpdateSource, World, WorldChange, WorldLimits,
    WorldPose, WorldReadError, WorldTransform, WorldTransformState,
};

fn world_limits() -> WorldLimits {
    WorldLimits {
        max_region_registrations: 2,
        max_entities: 3,
        max_entities_per_region: 2,
        max_tombstones_per_region: 2,
        max_tombstones: 4,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(2),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(4),
        max_journal_entries: 12,
        max_journal_changes: 24,
        max_region_name_bytes: 32,
        max_recovery_targets: 2,
    }
}

fn uuid(value: u128) -> UUID {
    UUID::from_u128(value)
}

fn source(region: Region, order: u64) -> UpdateSource {
    UpdateSource::new(region, order as u32, Ordered(order), 0)
}

fn local_id(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("local ID is nonzero")
}

fn object(region: Region, order: u64, id: u128, local: u32, kind: Kind, parent: ParentUpdate) -> ObjectObservation {
    ObjectObservation {
        source: source(region, order),
        id: uuid(id),
        local_id: local_id(local),
        kind,
        parent,
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

fn entity(outcome: Applied) -> Entity {
    let Introduced(entity) = outcome.outcome else { panic!("expected object introduction") };
    entity
}

#[test]
fn borrowed_queries_expose_current_state() {
    let first = Region::new(1);
    let second = Region::new(2);
    let mut world = World::new(world_limits());
    assert_eq!(world.adopt_region(first).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(second).unwrap().outcome, RegionAdopted);
    let first_info = RegionInfo {
        region_id: Some(uuid(10)),
        handle: Some(RegionHandle::new((256_u64 << 32) | 512)),
        name: Some("First".to_owned()),
        origin: None,
    };
    assert_eq!(world.patch_region(first, first_info.clone()).unwrap().outcome, RegionPatched);
    assert_eq!(world.region(first), Some(&first_info));
    assert_eq!(first.get(), 1);
    assert_eq!(first_info.handle.unwrap().get(), (256_u64 << 32) | 512);
    assert_eq!(first_info.handle.unwrap().origin(), DVec3::new(256.0, 512.0, 0.0));
    assert_eq!(world.region(Region::new(99)), None);

    let mut root = object(first, 1, 20, 1, Object, Root);
    root.pose = Some(Pose { position: Some(Vec3::new(3.0, 4.0, 5.0)), rotation: Some(Quat::IDENTITY) });
    root.linear_motion = Some(LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::Y) });
    root.angular_velocity = Some(Vec3::Z);
    root.scale = Some(Vec3::splat(2.0));
    root.attachment_point = Some(NonZeroU8::new(7).map(AttachmentPoint::new));
    let root = entity(world.observe_object(root).unwrap());
    let child = entity(world.observe_object(object(first, 2, 21, 2, Object, Local(local_id(1)))).unwrap());
    let avatar = entity(world.observe_object(object(second, 1, 22, 1, Avatar, Root)).unwrap());

    assert_eq!(world.entity(uuid(20)), Some(root));
    assert_eq!(world.entity(uuid(99)), None);
    assert_eq!(world.entity_at(LocalAddress::new(first, local_id(1))), Some(root));
    assert_eq!(world.object_count(), 3);
    assert!(world.revision().get() > 0);
    assert_eq!(world.objects().count(), 3);
    assert_eq!(world.objects_in_region(first).count(), 2);
    assert_eq!(world.objects_in_region(second).count(), 1);
    assert_eq!(world.regions().count(), 2);
    assert_eq!(world.children(root).count(), 1);
    assert_eq!(world.avatars().map(|object| object.entity()).collect::<Vec<_>>(), vec![avatar]);

    let view = world.object(root).expect("root exists");
    assert_eq!(view.entity(), root);
    assert_eq!(view.identity().id(), uuid(20));
    assert_eq!(view.kind(), Object);
    assert_eq!(view.address(), LocalAddress::new(first, local_id(1)));
    assert_eq!(view.address().local_id(), local_id(1));
    assert_eq!(view.parent(), Parent::Root);
    assert_eq!(view.position(), Some(Vec3::new(3.0, 4.0, 5.0)));
    assert_eq!(view.rotation(), Some(Quat::IDENTITY));
    assert_eq!(view.scale(), Some(Vec3::splat(2.0)));
    assert_eq!(view.local_transform().unwrap().scale, Vec3::splat(2.0));
    assert_eq!(view.linear_motion(), LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::Y) });
    assert_eq!(view.angular_velocity(), Some(Vec3::Z));
    assert_eq!(view.shape(), None);
    assert_eq!(view.appearance(), None);
    assert_eq!(view.attachment_point(), NonZeroU8::new(7).map(AttachmentPoint::new));
    assert_eq!(view.attachment_point().unwrap().get(), 7);
    assert_eq!(view.sit_target(), None);
    assert_eq!(view.properties().owner_id, None);
    assert_eq!(view.coordinate_frame().unwrap(), CoordinateFrame::RegionRelative { region: first });
    assert_eq!(
        view.world_transform().unwrap(),
        WorldTransformState::Resolved(WorldTransform {
            pose: WorldPose { position: DVec3::new(259.0, 516.0, 5.0), rotation: Quat::IDENTITY },
            scale: Vec3::splat(2.0),
        })
    );
    assert_eq!(world.object(child).unwrap().coordinate_frame().unwrap(), CoordinateFrame::LinksetRoot { root });
    assert_eq!(world.object(child).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::MissingLocalPose));
    drop(view);

    assert_eq!(
        world
            .patch_properties(source(first, 3), uuid(20), PropertiesPatch { owner_id: Some(uuid(30)), creator_id: Some(uuid(31)) })
            .unwrap()
            .outcome,
        Updated(root)
    );
    let properties = world.object(root).unwrap().properties();
    assert_eq!(properties.owner_id, Some(uuid(30)));
    assert_eq!(properties.creator_id, Some(uuid(31)));
}

#[test]
fn objects_returns_live_entities() {
    let region = Region::new(3);
    let mut world = World::new(world_limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);

    let obj = entity(world.observe_object(object(region, 1, 40, 1, Object, Root)).unwrap());
    let avatar = entity(world.observe_object(object(region, 2, 41, 2, Avatar, Root)).unwrap());
    assert_eq!(world.objects().map(|object| object.identity().id()).collect::<HashSet<_>>(), HashSet::from([uuid(40), uuid(41)]));
    assert_eq!(world.objects().count(), world.object_count());

    assert_eq!(world.remove_object(source(region, 3), local_id(2)).unwrap().outcome, Removed(uuid(41)));
    assert!(world.object(avatar).is_none());
    assert_eq!(world.objects().map(|object| object.identity().id()).collect::<HashSet<_>>(), HashSet::from([uuid(40)]));

    let replacement = entity(world.observe_object(object(region, 4, 41, 2, Avatar, Root)).unwrap());
    assert_ne!(avatar, replacement);
    assert_eq!(world.objects().map(|object| object.identity().id()).collect::<HashSet<_>>(), HashSet::from([uuid(40), uuid(41)]));
    assert_eq!(world.objects().count(), world.object_count());
    assert_eq!(world.object(obj).unwrap().identity().id(), uuid(40));
}

#[test]
fn object_views_borrow_the_same_identity() {
    let region = Region::new(4);
    let mut world = World::new(world_limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let entity = entity(world.observe_object(object(region, 1, 50, 1, Object, Root)).unwrap());

    let first = world.object(entity).expect("object exists");
    let second = world.object(entity).expect("object exists");

    assert!(eq(first.identity(), second.identity()));
    assert_eq!(first.identity().id(), uuid(50));
    assert_eq!(second.kind(), Object);
    assert_eq!(first.address(), LocalAddress::new(region, local_id(1)));
}

#[test]
fn object_pages_reconcile_inserts_behind_the_cursor() {
    let region = Region::new(5);
    let mut limits = world_limits();
    limits.max_entities = 4;
    limits.max_entities_per_region = 4;
    let mut world = World::new(limits);
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    entity(world.observe_object(object(region, 1, 10, 1, Object, Root)).unwrap());
    entity(world.observe_object(object(region, 2, 20, 2, Object, Root)).unwrap());
    let barrier = world.read_barrier();

    let first = world.object_page(None, 1).unwrap();
    assert_eq!(first.hydration_revision, barrier.revision);
    assert_eq!(first.objects.iter().map(|item| item.id).collect::<Vec<_>>(), [uuid(10)]);
    let cursor = first.next.expect("second object remains");

    let inserted = entity(world.observe_object(object(region, 3, 5, 3, Object, Root)).unwrap());
    let second = world.object_page(Some(cursor), 1).unwrap();

    assert_eq!(second.objects.iter().map(|item| item.id).collect::<Vec<_>>(), [uuid(20)]);
    assert_eq!(second.next, None);
    let changes = world.change_page(barrier.changes, 8).unwrap();
    assert!(
        changes
            .records
            .iter()
            .any(|record| { matches!(record.change, WorldChange::EntityAdded { id, entity, .. } if id == uuid(5) && entity == inserted) })
    );
}

#[test]
fn list_pages_return_identity() {
    let region = Region::new(19);
    let mut world = World::new(world_limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let mut child = object(region, 1, 190, 1, Object, Local(local_id(9)));
    child.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
    let entity = entity(world.observe_object(child).unwrap());

    let page = world.object_list_page(None, 1).unwrap();
    let item = world.object_list_item(entity).unwrap();

    assert_eq!(page.hydration_revision, item.hydration_revision);
    assert_eq!(page.objects, [item.item]);
    assert_eq!(item.item.id, uuid(190));
    assert_eq!(item.item.kind, Object);
    assert_eq!(item.item.address, LocalAddress::new(region, local_id(1)));
    assert_eq!(world.object_summary(entity).unwrap().summary.pose, Ok(PoseState::Unresolved(UnresolvedReason::PendingParent)));
}

#[test]
fn detail_rejects_a_reused_entity() {
    let region = Region::new(6);
    let mut world = World::new(world_limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let original = entity(world.observe_object(object(region, 1, 60, 1, Object, Root)).unwrap());
    let barrier = world.read_barrier();

    assert_eq!(world.remove_object(source(region, 2), local_id(1)).unwrap().outcome, Removed(uuid(60)));
    let replacement = entity(world.observe_object(object(region, 3, 60, 2, Object, Root)).unwrap());

    assert_ne!(original, replacement);
    assert_eq!(world.object_detail(original), Err(WorldReadError::StaleIdentity));
    assert_eq!(world.object_detail(replacement).unwrap().summary.id, uuid(60));
    let changes = world.change_page(barrier.changes, 8).unwrap();
    assert!(
        changes.records.iter().any(|record| {
            matches!(record.change, WorldChange::EntityRemoved { id, entity, .. } if id == uuid(60) && entity == original)
        })
    );
    assert!(
        changes.records.iter().any(|record| {
            matches!(record.change, WorldChange::EntityAdded { id, entity, .. } if id == uuid(60) && entity == replacement)
        })
    );
}

#[test]
fn summaries_include_relationship_and_pose() {
    let region = Region::new(17);
    let mut world = World::new(world_limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let parent = entity(world.observe_object(object(region, 1, 170, 1, Object, Root)).unwrap());
    let mut child = object(region, 2, 171, 2, Object, Local(local_id(1)));
    child.pose = Some(Pose { position: Some(Vec3::new(2.0, 3.0, 4.0)), rotation: None });
    let child = entity(world.observe_object(child).unwrap());

    let summary = world.object_summary(child).unwrap().summary;

    assert_eq!(summary.parent, Parent::Resolved(parent));
    assert_eq!(summary.local_transform.position(), Some(Vec3::new(2.0, 3.0, 4.0)));
    assert_eq!(summary.pose, Ok(PoseState::Unresolved(UnresolvedReason::MissingLocalPose)));
}

#[test]
fn detail_batches_preserve_order() {
    let region = Region::new(18);
    let mut world = World::new(world_limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let original = entity(world.observe_object(object(region, 1, 180, 1, Object, Root)).unwrap());
    let other = entity(world.observe_object(object(region, 2, 181, 2, Object, Root)).unwrap());
    assert_eq!(world.remove_object(source(region, 3), local_id(1)).unwrap().outcome, Removed(uuid(180)));
    let replacement = entity(world.observe_object(object(region, 4, 180, 3, Object, Root)).unwrap());

    let batch = world.object_detail_batch(&[other, original, replacement]).unwrap();

    assert_eq!(batch.hydration_revision, world.revision());
    assert!(matches!(
        batch.details.as_slice(),
        [
            ObjectDetailResult::Found(first),
            ObjectDetailResult::Gone(gone),
            ObjectDetailResult::Found(last)
        ] if first.summary.entity == other && first.hydration_revision == batch.hydration_revision
            && *gone == original && last.summary.entity == replacement && last.hydration_revision == batch.hydration_revision
    ));
}

#[test]
fn details_include_bounded_media() {
    let region = Region::new(22);
    let mut world = World::new(world_limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let mut observation = object(region, 1, 220, 1, Object, Root);
    observation.appearance = Some(Some(Appearance::Surface));
    let entity = entity(world.observe_object(observation).unwrap());
    let media = MediaEntry { current_url: "https://media.test/item".to_owned(), ..MediaEntry::default() };
    let update = ObjectMediaUpdate::new(1, vec![Some(media)]).unwrap();
    assert!(
        world
            .patch_object(ObjectPatch { object_media: Some(update), ..ObjectPatch::new(source(region, 2), local_id(1)) })
            .unwrap()
            .revision
            .is_some()
    );

    let detail = world.object_detail(entity).unwrap();
    assert_eq!(detail.object_media.unwrap().face(0).unwrap().current_url, "https://media.test/item");

    let large_entry = MediaEntry {
        current_url: "x".repeat(MAX_MEDIA_URL_BYTES),
        home_url: "y".repeat(MAX_MEDIA_URL_BYTES),
        whitelist: vec!["z".repeat(MAX_MEDIA_WHITELIST_BYTES)],
        ..MediaEntry::default()
    };
    let large = ObjectMediaUpdate::new(2, vec![Some(large_entry); 23]).unwrap();
    assert!(
        world
            .patch_object(ObjectPatch { object_media: Some(large), ..ObjectPatch::new(source(region, 3), local_id(1)) })
            .unwrap()
            .revision
            .is_some()
    );

    assert_eq!(world.object_detail(entity), Err(WorldReadError::TooLarge));
}

#[test]
fn change_pages_resume_inside_a_revision() {
    let region = Region::new(7);
    let mut world = World::new(world_limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    entity(world.observe_object(object(region, 1, 70, 1, Object, Root)).unwrap());
    entity(world.observe_object(object(region, 2, 71, 2, Object, Root)).unwrap());
    let barrier = world.read_barrier();
    let retired = world.retire_region(region).unwrap();
    let revision = retired.revision.expect("retirement changes the World");

    let first = world.change_page(barrier.changes, 1).unwrap();
    let second = world.change_page(first.cursor, 1).unwrap();
    let third = world.change_page(second.cursor, 1).unwrap();

    assert!(first.has_more);
    assert!(second.has_more);
    assert!(!third.has_more);
    assert_eq!(first.records.len() + second.records.len() + third.records.len(), 3);
    assert!(first.records.iter().chain(&second.records).chain(&third.records).all(|record| record.revision == revision));
    assert!(matches!(third.records[0].change, WorldChange::RegionRetired { region: retired, .. } if retired == region));
}

#[test]
fn journal_eviction_requires_a_fresh_barrier() {
    let region = Region::new(8);
    let mut limits = world_limits();
    limits.max_journal_entries = 1;
    let mut world = World::new(limits);
    let before_adoption = world.read_barrier();
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let after_adoption = world.read_barrier();
    assert_eq!(
        world.patch_region(region, RegionInfo { name: Some("region".to_owned()), ..RegionInfo::default() }).unwrap().outcome,
        RegionPatched
    );

    assert_eq!(world.change_page(before_adoption.changes, 8), Err(WorldReadError::ResyncRequired));
    let changes = world.change_page(after_adoption.changes, 8).unwrap();
    assert!(matches!(changes.records.as_slice(), [record] if record.change == WorldChange::RegionUpdated(region)));
}

#[test]
fn foreign_handles_and_zero_limits_are_rejected() {
    let region = Region::new(9);
    let mut first = World::new(world_limits());
    let mut second = World::new(world_limits());
    assert_eq!(first.adopt_region(region).unwrap().outcome, RegionAdopted);
    assert_eq!(second.adopt_region(region).unwrap().outcome, RegionAdopted);
    let selected = entity(first.observe_object(object(region, 1, 90, 1, Object, Root)).unwrap());
    entity(first.observe_object(object(region, 2, 91, 2, Object, Root)).unwrap());
    let cursor = first.object_page(None, 1).unwrap().next.expect("second object remains");
    let list_cursor = first.object_list_page(None, 1).unwrap().next.expect("second list item remains");
    let barrier = first.read_barrier();

    assert_eq!(second.object_page(Some(cursor), 1), Err(WorldReadError::StaleSession));
    assert_eq!(second.object_list_page(Some(list_cursor), 1), Err(WorldReadError::StaleSession));
    assert_eq!(second.object_list_item(selected), Err(WorldReadError::StaleSession));
    assert_eq!(second.object_detail(selected), Err(WorldReadError::StaleSession));
    assert_eq!(second.object_detail_batch(&[selected]), Err(WorldReadError::StaleSession));
    assert_eq!(first.object_detail_batch(&[]), Err(WorldReadError::InvalidLimit));
    assert_eq!(first.object_detail_batch(&[selected; MAX_OBJECT_DETAIL_BATCH_ITEMS + 1]), Err(WorldReadError::InvalidLimit));
    assert_eq!(second.change_page(barrier.changes, 1), Err(WorldReadError::StaleSession));
    assert_eq!(first.object_page(None, 0), Err(WorldReadError::InvalidLimit));
    assert_eq!(first.change_page(barrier.changes, 0), Err(WorldReadError::InvalidLimit));
}

#[test]
fn region_pages_follow_active_regions() {
    let first = Region::new(10);
    let second = Region::new(11);
    let mut world = World::new(world_limits());
    assert_eq!(world.adopt_region(first).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(second).unwrap().outcome, RegionAdopted);
    assert_eq!(world.read_barrier().region_count, 2);

    let page = world.region_page(None, 1).unwrap();
    assert_eq!(page.regions.iter().map(|summary| summary.region).collect::<Vec<_>>(), [first]);
    let cursor = page.next.expect("second region remains");
    assert_eq!(world.retire_region(first).unwrap().outcome, RegionRetired { entities_removed: 0 });

    let page = world.region_page(Some(cursor), 1).unwrap();
    assert_eq!(page.regions.iter().map(|summary| summary.region).collect::<Vec<_>>(), [second]);
    assert_eq!(page.next, None);
    assert_eq!(world.region_summary(second).unwrap().unwrap().summary.region, second);
}

#[test]
fn region_page_rejects_a_large_name() {
    let region = Region::new(12);
    let mut limits = world_limits();
    limits.max_region_name_bytes = MAX_REGION_PAGE_BYTES + 1;
    let mut world = World::new(limits);
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let name = "r".repeat(MAX_REGION_PAGE_BYTES);
    assert_eq!(world.patch_region(region, RegionInfo { name: Some(name), ..RegionInfo::default() }).unwrap().outcome, RegionPatched);

    assert_eq!(world.region_page(None, 1), Err(WorldReadError::TooLarge));
    assert_eq!(world.region_summary(region), Err(WorldReadError::TooLarge));
}

#[test]
fn oversized_revision_requires_resync() {
    let region = Region::new(13);
    let mut limits = world_limits();
    limits.max_journal_changes = 1;
    let mut world = World::new(limits);
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    entity(world.observe_object(object(region, 1, 130, 1, Object, Root)).unwrap());
    let mut child = object(region, 2, 131, 2, Object, Local(local_id(1)));
    child.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: None });
    entity(world.observe_object(child).unwrap());
    let barrier = world.read_barrier();
    let mut movement = ObjectPatch::new(source(region, 3), local_id(1));
    movement.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
    assert!(world.patch_object(movement).unwrap().revision.is_some());

    assert_eq!(world.change_page(barrier.changes, 8), Err(WorldReadError::ResyncRequired));
}

#[test]
fn evicted_partial_revision_requires_resync() {
    let first = Region::new(14);
    let second = Region::new(15);
    let mut limits = world_limits();
    limits.max_journal_entries = 1;
    let mut world = World::new(limits);
    assert_eq!(world.adopt_region(first).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(second).unwrap().outcome, RegionAdopted);
    entity(world.observe_object(object(first, 1, 140, 1, Object, Root)).unwrap());
    entity(world.observe_object(object(first, 2, 141, 2, Object, Root)).unwrap());
    let barrier = world.read_barrier();
    assert_eq!(world.retire_region(first).unwrap().outcome, RegionRetired { entities_removed: 2 });
    let partial = world.change_page(barrier.changes, 1).unwrap();
    assert!(partial.has_more);
    assert_eq!(
        world.patch_region(second, RegionInfo { name: Some("second".to_owned()), ..RegionInfo::default() }).unwrap().outcome,
        RegionPatched
    );

    assert_eq!(world.change_page(partial.cursor, 1), Err(WorldReadError::ResyncRequired));
}
