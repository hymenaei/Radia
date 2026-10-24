use std::assert_matches;

use super::support::{
    BTreeMap, BTreeSet, Budget, Duration, Instant, NonZeroU32, ParentUpdate, Placement, ProptestConfig, ReadFailure, ReadQuery, ReadReply,
    ReconcileStep, Reconciler, RecoveryReason, Region, Status, Uuid, WorldReadError, add, ids, move_root, positions, prop, prop_assert_eq,
    proptest, reply, settle, source, world,
};

#[test]
fn atomic_region_retirement() {
    let mut world = world();
    for id in 1..301 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 0.0, id as u64);
    }
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let entities: Vec<_> = view.objects().map(|object| object.detail().summary.entity).collect();
    let _ = world.retire_region(Region::new(1)).unwrap();
    scene.notify_world_change();
    let mut partial = false;
    loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                if let Ok(ReadReply::Changes(page)) = &result {
                    partial |= page.records.last().is_some_and(|record| record.revision > page.completed_revision);
                }
                scene.complete_read(job, result, now);
                assert_eq!(view.objects().count(), 300);
            }
            ReconcileStep::Publish(batch) => {
                view.apply(batch).unwrap();
                break;
            }
            ReconcileStep::Continue => {}
            _ => panic!("expected retirement continuation"),
        }
    }
    assert!(partial);
    assert!(entities.into_iter().all(|entity| view.object(entity).is_none()), "retirement removes direct object lookups atomically");
    assert!(view.region(Region::new(1)).is_none(), "retirement removes the region lookup with its objects");
    assert_eq!(view.objects().count(), 0);
    assert_eq!(view.regions().count(), 0);
    assert_eq!(view.revision(), Some(world.revision()));
}

#[test]
fn history_recovery_backoff() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 10.0, 1);
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let retained = budget.charged_bytes();
    assert!(retained > 0);
    for attempt in 1..=4 {
        scene.notify_world_change();
        let ReconcileStep::Read(job) = scene.poll(now) else {
            panic!("expected recovery read");
        };
        scene.complete_read(job, Err(ReadFailure::World(WorldReadError::ResyncRequired)), now);
        let ReconcileStep::Publish(batch) = scene.poll(now) else {
            panic!("expected recovery status");
        };
        view.apply(batch).unwrap();
        assert_eq!(positions(&view), BTreeMap::from([(1, 10.0)]));
        assert_eq!(budget.charged_bytes(), retained);
        let expected = if attempt <= 3 {
            Status::Recovering { reason: RecoveryReason::HistoryLost, attempt }
        } else {
            Status::Degraded { reason: RecoveryReason::HistoryLost }
        };
        assert_eq!(view.status(), expected);
        let ReconcileStep::Wait(until) = scene.poll(now) else {
            panic!("expected backoff");
        };
        assert_eq!(until.duration_since(now), Duration::from_secs(if attempt <= 3 { 1 << (attempt - 1) } else { 30 }));
        now = until;
    }
    move_root(&mut world, 20.0, 2);
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(positions(&view), BTreeMap::from([(1, 20.0)]));
    assert_eq!(view.status(), Status::Ready);
}

#[test]
fn publication_backpressure_and_close() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 10.0, 1);
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    move_root(&mut world, 20.0, 2);
    scene.notify_world_change();
    let batch = loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Publish(batch) => break batch,
            ReconcileStep::Continue => {}
            _ => panic!("expected bootstrap"),
        }
    };
    let held = budget.charged_bytes();
    for _ in 0..1000 {
        assert!(matches!(scene.poll(now), ReconcileStep::Wait(_)), "held publication keeps reconciliation backpressured");
    }
    assert_eq!(budget.charged_bytes(), held);
    view.apply(batch).unwrap();
    let retained = view.objects().next().unwrap().clone();
    scene.notify_world_change();
    let ReconcileStep::Read(job) = scene.poll(now) else {
        panic!("expected journal read");
    };
    let result = reply(&world, &job);
    scene.close();
    scene.complete_read(job, result, now);
    assert!(matches!(scene.poll(now), ReconcileStep::Closed), "closed reconciler remains closed");
    assert_eq!(view.objects().count(), 0);
    assert_eq!(view.status(), Status::Closed);
    assert!(view.object(retained.detail().summary.entity).is_none(), "retained data cannot bypass the session lifetime");
    assert!(view.region(Region::new(1)).is_none(), "closed sessions hide retained region data");
    drop(scene);
    drop(view);
    assert!(budget.charged_bytes() > 0);
    drop(retained);
    assert_eq!(budget.charged_bytes(), 0);
}

#[test]
fn hydration_identity_conflicts() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 1.0, 1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    add(&mut world, 2, 2, ParentUpdate::Root, 2.0, 2);
    scene.notify_world_change();
    let ReconcileStep::Read(job) = scene.poll(now) else {
        panic!("expected journal read");
    };
    let old_page = reply(&world, &job);
    add(&mut world, 2, 1, ParentUpdate::Root, 3.0, 3);
    scene.complete_read(job, old_page, now);
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(ids(&view), BTreeSet::from([2]));
    assert_eq!(positions(&view), BTreeMap::from([(2, 3.0)]));
    assert!(view.revision().unwrap() < world.revision());
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(ids(&view), BTreeSet::from([2]));
}

#[test]
fn abandoned_publication_recovery() {
    for reject_foreign in [false, true] {
        let mut world = world();
        add(&mut world, 1, 1, ParentUpdate::Root, 1.0, 1);
        let (mut scene, mut view) = Reconciler::new(Budget::default());
        let mut now = Instant::now();
        settle(&mut scene, &mut view, &world, &mut now);
        let (mut other_scene, mut other_view) = Reconciler::new(Budget::default());
        settle(&mut other_scene, &mut other_view, &world, &mut now);
        move_root(&mut world, 2.0, 2);
        scene.notify_world_change();
        loop {
            match scene.poll(now) {
                ReconcileStep::Read(job) => {
                    let result = reply(&world, &job);
                    scene.complete_read(job, result, now);
                }
                ReconcileStep::Publish(batch) => {
                    if reject_foreign {
                        let version = other_view.version();
                        let revision = other_view.revision();
                        assert_eq!(other_view.apply(batch), Err(render::scene::ApplyError::StaleSession));
                        assert_eq!(other_view.version(), version);
                        assert_eq!(other_view.revision(), revision);
                        assert_eq!(positions(&other_view), BTreeMap::from([(1, 1.0)]));
                    } else {
                        drop(batch);
                    }
                    break;
                }
                ReconcileStep::Continue => {}
                _ => panic!("expected publication"),
            }
        }
        let ReconcileStep::Publish(status) = scene.poll(now) else {
            panic!("expected abandoned delivery recovery");
        };
        view.apply(status).unwrap();
        assert_eq!(positions(&view), BTreeMap::from([(1, 1.0)]));
        assert_matches!(view.status(), Status::Recovering { reason: RecoveryReason::InvalidReply, .. });
        settle(&mut scene, &mut view, &world, &mut now);
        assert_eq!(positions(&view), BTreeMap::from([(1, 2.0)]));
    }
}

#[test]
fn bounded_reconciliation() {
    let mut world = world();
    for id in 1..=2048 {
        let parent = match id {
            2 => ParentUpdate::Local(NonZeroU32::new(1).unwrap()),
            3 => ParentUpdate::Local(NonZeroU32::new(2).unwrap()),
            _ => ParentUpdate::Root,
        };
        add(&mut world, id, id as u32, parent, 1.0, id as u64);
    }
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();
    let start = Instant::now();
    let bootstrap_reads = settle(&mut scene, &mut view, &world, &mut now);
    let bootstrap_time = start.elapsed();
    assert_eq!(view.object_count(), 2048);
    assert!(bootstrap_reads <= 300);
    let unchanged = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(2048)).unwrap().clone();
    move_root(&mut world, 10.0, 3000);
    scene.notify_world_change();
    let start = Instant::now();
    let movement_reads = settle(&mut scene, &mut view, &world, &mut now);
    let movement_time = start.elapsed();
    assert!(movement_reads <= 3, "local movement should only hydrate the affected subtree");
    let after = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(2048)).unwrap();
    assert!(std::ptr::eq(unchanged.detail(), after.detail()));
    let child = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(3)).unwrap();
    assert_matches!(child.placement(), Placement::Ready(transform) if transform.pose.position.x == 12.0);
    eprintln!(
        "2048 objects: bootstrap {:?}, {} reads; three-object movement {:?}, {} reads; admission peak {} bytes",
        bootstrap_time,
        bootstrap_reads,
        movement_time,
        movement_reads,
        budget.high_water_bytes()
    );
}

#[test]
fn recovery_memory_admission() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 1.0, 1);
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let retained = budget.charged_bytes();
    scene.notify_world_change();
    let ReconcileStep::Read(job) = scene.poll(now) else {
        panic!("expected journal read");
    };
    scene.complete_read(job, Err(ReadFailure::World(WorldReadError::ResyncRequired)), now);
    let ReconcileStep::Publish(status) = scene.poll(now) else {
        panic!("expected recovery status");
    };
    view.apply(status).unwrap();
    let ReconcileStep::Wait(until) = scene.poll(now) else {
        panic!("expected backoff");
    };
    now = until;
    let ReconcileStep::Read(job) = scene.poll(now) else {
        panic!("expected new barrier");
    };
    let mut oversized = world.read_barrier();
    oversized.object_count = render::scene::MAX_OBJECTS + 1;
    scene.complete_read(job, Ok(ReadReply::Barrier(oversized)), now);
    let ReconcileStep::Publish(status) = scene.poll(now) else {
        panic!("expected capacity status");
    };
    view.apply(status).unwrap();
    assert_matches!(view.status(), Status::Recovering { reason: RecoveryReason::Capacity, .. });
    assert_eq!(ids(&view), BTreeSet::from([1]));
    let ReconcileStep::Wait(until) = scene.poll(now) else {
        panic!("expected backoff");
    };
    now = until;
    loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let details = matches!(job.query(), ReadQuery::Details(_));
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
                if details {
                    break;
                }
            }
            ReconcileStep::Continue => {}
            _ => panic!("expected staging hydration"),
        }
    }
    assert!(budget.charged_bytes() >= 2 * retained);
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(budget.charged_bytes(), retained);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn incarnation_churn(ops in prop::collection::vec((0u8..3, 1u32..17), 1..100)) {
        let mut world = world();
        let (mut scene, mut view) = Reconciler::new(Budget::default());
        let mut now = Instant::now();
        settle(&mut scene, &mut view, &world, &mut now);
        let mut expected = BTreeMap::new();
        for (index, (op, address)) in ops.into_iter().enumerate() {
            let order = index as u64 + 1;
            if op == 0 {
                let _ = world.remove_object(source(1, order), NonZeroU32::new(address).unwrap()).unwrap();
                expected.remove(&address);
            } else {
                let id = if op == 1 { u128::from(address) } else { 1000 + index as u128 };
                add(&mut world, id, address, ParentUpdate::Root, order as f32, order);
                expected.insert(address, id);
            }
            scene.notify_world_change(); settle(&mut scene, &mut view, &world, &mut now);
            prop_assert_eq!(ids(&view), expected.values().copied().collect());
            prop_assert_eq!(view.objects().count(), expected.len());
        }
    }
}
