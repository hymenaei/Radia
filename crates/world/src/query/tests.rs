use std::{
    collections::HashSet,
    num::{NonZeroU8, NonZeroU32},
    time::{Duration, Instant},
};

use math::{Quat, Vec3};

use super::*;
use crate::{
    AttachmentPoint, FieldUpdate, MAX_QUERY_STATE_MAINTENANCE_WORK, ObjectObservation, OrderEvidence::Ordered, ParentUpdate,
    ParentUpdate::Root, Pose, PropertiesPatch, RECENT_OBJECT_TTL, RegionInfo, UpdateSource, WorldEntityRemovalReason, WorldLimits,
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
        pose: Some(Pose { position: Some(position), rotation: Some(Quat::IDENTITY) }),
        ..ObjectObservation::new(
            UpdateSource::new(region, 1, Ordered(id as u64 + 1), 0),
            Uuid::from_u128(id),
            NonZeroU32::new(local_id).unwrap(),
            kind,
            Root,
        )
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
    owner_id: Option<Uuid>,
    creator_id: Option<Uuid>,
) -> Entity {
    let id = Uuid::from_u128(id);
    let observation = ObjectObservation {
        attachment_point: attachment_point.map_or(FieldUpdate::Clear, FieldUpdate::Set),
        ..ObjectObservation::new(
            UpdateSource::new(region, local_id, Ordered(u64::from(local_id)), 0),
            id,
            NonZeroU32::new(local_id).expect("local ID is nonzero"),
            Kind::Object,
            Root,
        )
    };
    let result = world.observe_object(observation).expect("query fixture object");
    let crate::ApplyOutcome::Introduced(entity) = result.outcome else { panic!("query fixture object is introduced") };

    if owner_id.is_some() || creator_id.is_some() {
        let target = world.object_binding_target(entity).expect("query fixture properties target");
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
    assert_eq!(result.items.iter().map(|item| item.id()).collect::<Vec<_>>(), [Uuid::from_u128(1)]);
}

#[test]
fn metadata_filters_require_known_values() {
    let region = Region::new(2);
    let mut world = World::new(limits());
    add_region(&mut world, region);
    set_origin(&mut world, region);
    let known = Uuid::from_u128(11);
    let other = Uuid::from_u128(12);
    let owner = Uuid::from_u128(91);
    let other_owner = Uuid::from_u128(92);
    let creator = Uuid::from_u128(93);
    let other_creator = Uuid::from_u128(94);
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
    assert_eq!(query_ids(&world, WorldQuery::live().creator(ValueFilter::Is(creator))), HashSet::from([known]), "known creator matches Is",);
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

    let page =
        world.query(WorldQuery::live().in_region(second), None, WorldQueryBudget { work: 1, results: 1 }).expect("region-filtered query");

    assert_eq!(page.items.iter().copied().map(WorldQueryItem::id).collect::<Vec<_>>(), [Uuid::from_u128(100)]);
}

fn query_ids(world: &World, query: WorldQuery) -> HashSet<Uuid> {
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
                id: Uuid::from_u128(u128::from(id)),
                kind: Kind::Object,
                address: crate::LocalAddress::new(region, NonZeroU32::new(id).unwrap()),
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

    assert_eq!(first.items.iter().map(|item| item.id()).collect::<Vec<_>>(), [Uuid::from_u128(31)]);
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
        pose: Some(Pose { position: Some(Vec3::new(128.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) }),
        ..ObjectObservation::new(
            UpdateSource::new(region, 2, Ordered(90), 0),
            Uuid::from_u128(41),
            NonZeroU32::new(1).unwrap(),
            Kind::Object,
            Root,
        )
    };
    let _ = world.observe_object(moved).unwrap();
    let query = WorldQuery::live().within(DVec3::new(128.0, 0.0, 0.0), 1.0).unwrap();
    let result = world.query(query, None, WorldQueryBudget { work: 32, results: 8 }).unwrap();

    assert_eq!(result.completeness, QueryCompleteness::Exact);
    assert_eq!(result.items.iter().map(|item| item.id()).collect::<Vec<_>>(), [Uuid::from_u128(41)]);
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
            pose: Some(Pose { position: Some(Vec3::X), rotation: Some(Quat::IDENTITY) }),
            ..ObjectObservation::new(
                UpdateSource::new(region, local_id, Ordered(u64::from(local_id)), 0),
                Uuid::from_u128(5_000 + u128::from(local_id)),
                NonZeroU32::new(local_id).unwrap(),
                Kind::Object,
                parent,
            )
        };
        let result = world.observe_object(observation).expect("introduce linkset object");
        assert!(matches!(result.outcome, crate::ApplyOutcome::Introduced(_)));
    }

    let candidate_id = Uuid::from_u128(5_000 + u128::from(CHAIN_LENGTH + 1));
    let candidate_local_id = NonZeroU32::new(CHAIN_LENGTH + 1).unwrap();
    let candidate = world
        .observe_object(ObjectObservation {
            pose: Some(Pose { position: Some(Vec3::new(30.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) }),
            ..ObjectObservation::new(
                UpdateSource::new(region, CHAIN_LENGTH + 1, Ordered(u64::from(CHAIN_LENGTH + 1)), 0),
                candidate_id,
                candidate_local_id,
                Kind::Object,
                Root,
            )
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
            pose: Some(Pose { position: Some(Vec3::X), rotation: Some(Quat::IDENTITY) }),
            ..ObjectObservation::new(
                UpdateSource::new(region, CHAIN_LENGTH + 2, Ordered(u64::from(CHAIN_LENGTH + 2)), 0),
                candidate_id,
                candidate_local_id,
                Kind::Object,
                ParentUpdate::Local(NonZeroU32::new(CHAIN_LENGTH).unwrap()),
            )
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
            id: Uuid::from_u128(61),
            kind: Kind::Object,
            address: crate::LocalAddress::new(region, NonZeroU32::new(1).unwrap()),
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
