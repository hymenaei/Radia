use std::assert_matches;

use super::support::{
    BTreeMap, BTreeSet, Budget, DVec3, Detail, FieldUpdate, Instant, Kind, NonZeroU32, ObjectObservation, ObjectPatch, ParentUpdate,
    Placement, Pose, PreparedMesh, Quat, ReadFailure, ReadQuery, ReconcileStep, Reconciler, Region, RegionInfo, Shape, Status, Uuid, Vec3,
    add, camera, ids, move_root, positions, reply, settle, source, world,
};

#[test]
fn versions_fence_sessions_and_close() {
    let (mut scene, view) = Reconciler::new(Budget::default());
    let (_other_scene, other) = Reconciler::new(Budget::default());
    let version = view.version();
    assert_eq!(view.sequence(), other.sequence());
    assert_eq!(version, view.version(), "unchanged publication keeps its version");
    assert_ne!(version, other.version(), "matching sequence numbers cannot alias across sessions");
    scene.close();
    assert_ne!(version, view.version(), "closing invalidates cached content before another publication");
    assert_eq!(view.status(), Status::Closed);
}

#[test]
fn larger_visible_objects_build_first() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 100.0, 1);
    add(&mut world, 2, 2, ParentUpdate::Root, 10.0, 2);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(camera(0.0));
    let mut first = None;
    for _ in 0..2000 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Publish(batch) => {
                view.apply(batch).unwrap();
                first = view.objects().find(|object| object.mesh().is_some()).map(|object| object.detail().summary.id);
                if first.is_some() {
                    break;
                }
            }
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            _ => panic!("scene stopped before publishing geometry"),
        }
    }
    assert_eq!(first, Some(Uuid::from_u128(2)), "large on-screen geometry must overtake older, smaller requests");
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(view.objects().all(|object| object.mesh().is_some()), "smaller requests still complete");
}

#[test]
fn mesh_reuse_and_replacement() {
    let mut world = world();
    for id in 1..=2 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 100.0, id as u64);
    }
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();
    scene.set_camera(camera(-300.0));
    settle(&mut scene, &mut view, &world, &mut now);
    let meshes: Vec<_> = view.objects().map(|object| object.mesh().map(|(key, mesh)| (key, mesh.clone())).unwrap()).collect();
    assert_eq!(meshes[0].0.detail, Detail::Lowest);
    assert_eq!(meshes[0].1.id(), meshes[1].1.id());

    scene.set_camera(camera(99.0));
    let job = loop {
        match scene.poll(now) {
            ReconcileStep::Publish(batch) => {
                view.apply(batch).unwrap();
                assert!(view.objects().all(|object| object.mesh().unwrap().0.detail == Detail::Lowest));
            }
            ReconcileStep::Build(job) => break job,
            ReconcileStep::Continue => {}
            _ => panic!("expected new detail work"),
        }
    };
    scene.complete_mesh(job.run());
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(view.objects().all(|object| {
        let (key, mesh) = object.mesh().unwrap();
        key.detail == Detail::High && mesh.id() != meshes[0].1.id()
    }));
    let mut patch = ObjectPatch::new(source(1, 3), NonZeroU32::new(1).unwrap());
    patch.shape = FieldUpdate::Set(Shape::Prim(world::PrimShape { profile_curve: 15, ..Default::default() }));
    let _ = world.patch_object(patch).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let changed = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(1)).unwrap();
    assert!(changed.mesh().is_none());
    assert!(matches!(changed.mesh_state(), Some((_, PreparedMesh::Failed(render::scene::MeshBuildError::UnsupportedShape)))));
    scene.close();
    drop(scene);
    drop(view);
    assert!(budget.charged_bytes() > 0);
    drop(meshes);
    assert_eq!(budget.charged_bytes(), 0);
}

#[test]
fn mesh_progress_during_read() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 100.0, 1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let revision = view.revision();
    move_root(&mut world, 150.0, 2);
    scene.notify_world_change();
    let ReconcileStep::Read(held) = scene.poll(now) else {
        panic!("expected World read");
    };
    scene.set_camera(camera(99.0));
    let mut built = 0;
    for _ in 0..20 {
        match scene.poll(now) {
            ReconcileStep::Build(job) => {
                built += 1;
                scene.complete_mesh(job.run());
            }
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(_) => break,
            _ => panic!("mesh work must not start another World read"),
        }
    }
    assert_eq!(built, 1);
    assert!(view.objects().next().unwrap().mesh().is_some());
    assert_eq!(positions(&view)[&1], 100.0);
    assert_eq!(view.revision(), revision);
    let result = reply(&world, &held);
    scene.complete_read(held, result, now);
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(positions(&view)[&1], 150.0);
    assert_eq!(view.revision(), Some(world.revision()));
    assert!(view.objects().next().unwrap().mesh().is_some());

    let acknowledged = view.revision();
    move_root(&mut world, 175.0, 3);
    scene.notify_world_change();
    let ReconcileStep::Read(changes) = scene.poll(now) else {
        panic!("expected journal read");
    };
    let result = reply(&world, &changes);
    scene.complete_read(changes, result, now);
    let ReconcileStep::Read(details) = scene.poll(now) else {
        panic!("expected hydration");
    };
    scene.complete_read(details, Err(ReadFailure::Busy), now);
    let ReconcileStep::Publish(status) = scene.poll(now) else {
        panic!("expected recovery status");
    };
    view.apply(status).unwrap();
    assert_eq!(view.revision(), acknowledged, "status-only recovery cannot acknowledge unpublished World changes");
    assert_eq!(positions(&view)[&1], 150.0);
}

#[test]
fn camera_updates_preserve_progress() {
    let mut world = world();
    for id in 1..=300 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 100.0, id as u64);
    }
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    for turn in 0..30 {
        scene.set_camera(camera(-300.0 - f64::from(turn)));
        match scene.poll(now) {
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Continue | ReconcileStep::Idle => {}
            ReconcileStep::Wait(until) => now = until,
            _ => panic!("no World invalidation expected"),
        }
    }
    assert_eq!(view.objects().filter(|object| object.mesh().is_some()).count(), 300);
    settle(&mut scene, &mut view, &world, &mut now);
}

#[test]
fn mesh_admission_recovery() {
    let mut world = world();
    for id in 1..=1025 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 100.0, id as u64 * 2);
        let mut patch = ObjectPatch::new(source(1, id as u64 * 2 + 1), NonZeroU32::new(id as u32).unwrap());
        patch.shape = FieldUpdate::Set(Shape::Prim(world::PrimShape { profile_begin: id as u16, ..Default::default() }));
        let _ = world.patch_object(patch).unwrap();
    }
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(camera(-300.0));
    for _ in 0..30_000 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            _ => panic!("expected bounded preparation work"),
        }
        if view.objects().filter(|object| object.mesh().is_some()).count() == 1024
            && view.revision() == Some(world.revision())
            && view.objects().any(|object| matches!(object.mesh_state(), Some((_, PreparedMesh::Capacity))))
            && view.objects().all(|object| object.mesh().is_none() || object.materials().is_some())
        {
            break;
        }
    }
    assert_eq!(view.objects().filter(|object| matches!(object.mesh_state(), Some((_, PreparedMesh::Capacity)))).count(), 1);
    for _ in 0..32 {
        match scene.poll(now) {
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            _ => panic!("unchanged capacity cannot republish or regenerate"),
        }
    }
    let _ = world.remove_object(source(1, 10_000), NonZeroU32::new(1).unwrap()).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.object_count(), 1024);
    assert!(view.objects().all(|object| object.mesh().is_some()));
}

#[test]
fn quality_updates() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 100.0, 1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(camera(0.0));
    settle(&mut scene, &mut view, &world, &mut now);
    let revision = view.revision();
    assert_eq!(view.objects().next().unwrap().mesh().unwrap().0.detail, Detail::Low);

    scene.set_camera(camera(0.0).with_quality(32.0, 1.0).unwrap());
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(view.objects().next().unwrap().mesh().is_none());

    scene.set_camera(camera(0.0).with_quality(512.0, 4.0).unwrap());
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.objects().next().unwrap().mesh().unwrap().0.detail, Detail::Medium);
    scene.set_camera(camera(0.0).with_quality(512.0, 0.25).unwrap());
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.objects().next().unwrap().mesh().unwrap().0.detail, Detail::Lowest);
    assert_eq!(view.revision(), revision);
}

#[test]
fn bootstrap_identity_changes() {
    let mut world = world();
    for id in 2..302 {
        add(&mut world, id, id as u32, ParentUpdate::Root, id as f32, id as u64);
    }
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    loop {
        let job = match scene.poll(now) {
            ReconcileStep::Read(job) => job,
            ReconcileStep::Publish(batch) => {
                view.apply(batch).unwrap();
                continue;
            }
            ReconcileStep::Continue => continue,
            _ => panic!("expected bootstrap progress"),
        };
        let objects = matches!(job.query(), ReadQuery::Objects(None));
        let result = reply(&world, &job);
        scene.complete_read(job, result, now);
        if objects {
            break;
        }
    }
    add(&mut world, 1, 1000, ParentUpdate::Root, 1.0, 1000);
    add(&mut world, 900, 50, ParentUpdate::Root, 900.0, 1001);
    settle(&mut scene, &mut view, &world, &mut now);
    let mut expected: BTreeSet<_> = (2..302).collect();
    expected.remove(&50);
    expected.insert(1);
    expected.insert(900);
    assert_eq!(ids(&view), expected);
    assert_eq!(view.revision(), Some(world.revision()));
    assert_eq!(view.status(), Status::Ready);
}

#[test]
fn descendant_pose_updates() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 10.0, 1);
    add(&mut world, 2, 2, ParentUpdate::Local(NonZeroU32::new(1).unwrap()), 2.0, 2);
    add(&mut world, 3, 3, ParentUpdate::Local(NonZeroU32::new(2).unwrap()), 3.0, 3);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(positions(&view), BTreeMap::from([(1, 10.0), (2, 12.0), (3, 15.0)]));
    move_root(&mut world, 20.0, 4);
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(positions(&view), BTreeMap::from([(1, 20.0), (2, 22.0), (3, 25.0)]));
    let mut patch = ObjectPatch::new(source(1, 5), NonZeroU32::new(2).unwrap());
    patch.parent = Some(ParentUpdate::Root);
    patch.pose = Some(Pose { position: Some(Vec3::new(7.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
    let _ = world.patch_object(patch).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(positions(&view), BTreeMap::from([(1, 20.0), (2, 7.0), (3, 10.0)]));
    let _ = world.adopt_region(Region::new(2)).unwrap();
    let mut transfer =
        ObjectObservation::new(source(2, 1), Uuid::from_u128(2), NonZeroU32::new(9).unwrap(), Kind::Object, ParentUpdate::Root);
    transfer.pose = Some(Pose { position: Some(Vec3::new(8.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
    transfer.scale = Some(Vec3::ONE);
    let _ = world.observe_object(transfer).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    for obj in view.objects().filter(|obj| obj.detail().summary.id != Uuid::from_u128(1)) {
        assert_matches!(obj.placement(), Placement::Hidden(_));
    }
    let _ = world.patch_region(Region::new(2), RegionInfo { origin: Some(DVec3::new(100.0, 0.0, 0.0)), ..RegionInfo::default() }).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let root = view.objects().find(|obj| obj.detail().summary.id == Uuid::from_u128(2)).unwrap();
    assert_matches!(root.placement(), Placement::Ready(transform) if transform.pose.position.x == 108.0);
    let child = view.objects().find(|obj| obj.detail().summary.id == Uuid::from_u128(3)).unwrap();
    assert_matches!(child.placement(), Placement::Hidden(_));
    let mut child = ObjectObservation::new(
        source(2, 2),
        Uuid::from_u128(3),
        NonZeroU32::new(10).unwrap(),
        Kind::Object,
        ParentUpdate::Local(NonZeroU32::new(9).unwrap()),
    );
    child.pose = Some(Pose { position: Some(Vec3::new(3.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
    child.scale = Some(Vec3::ONE);
    let _ = world.observe_object(child).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(positions(&view), BTreeMap::from([(1, 20.0), (2, 108.0), (3, 111.0)]));
}

#[test]
fn pending_parent_arrival() {
    let mut world = world();
    add(&mut world, 2, 2, ParentUpdate::Local(NonZeroU32::new(1).unwrap()), 2.0, 1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_matches!(view.objects().next().unwrap().placement(), Placement::Hidden(_));
    add(&mut world, 1, 1, ParentUpdate::Root, 10.0, 2);
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(positions(&view), BTreeMap::from([(1, 10.0), (2, 12.0)]));
}

#[test]
fn own_signed_dimensions() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 10.0, 1);
    add(&mut world, 2, 2, ParentUpdate::Local(NonZeroU32::new(1).unwrap()), 2.0, 2);
    let mut root = ObjectPatch::new(source(1, 3), NonZeroU32::new(1).unwrap());
    root.scale = Some(Vec3::new(100.0, 200.0, 300.0));
    let _ = world.patch_object(root).unwrap();
    let mut child = ObjectPatch::new(source(1, 4), NonZeroU32::new(2).unwrap());
    child.scale = Some(Vec3::new(-2.0, 3.0, 4.0));
    let _ = world.patch_object(child).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let child = view.objects().find(|obj| obj.detail().summary.id == Uuid::from_u128(2)).unwrap();
    let Placement::Ready(transform) = child.placement() else {
        panic!("signed dimensions are retained by World");
    };
    assert_eq!(transform.pose.position.x, 12.0);
    assert_eq!(transform.scale, Vec3::new(-2.0, 3.0, 4.0));
    let mut zero = ObjectPatch::new(source(1, 5), NonZeroU32::new(2).unwrap());
    zero.scale = Some(Vec3::new(0.0, 3.0, 4.0));
    let _ = world.patch_object(zero).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let child = view.objects().find(|obj| obj.detail().summary.id == Uuid::from_u128(2)).unwrap();
    assert_eq!(child.placement(), Placement::Hidden(render::scene::HiddenReason::InvalidDimensions));
}

#[test]
fn abandoned_read_recovers() {
    let world = world();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();

    let ReconcileStep::Read(job) = scene.poll(now) else {
        panic!("expected initial read");
    };
    assert_matches!(job.query(), ReadQuery::Barrier, "initial reads establish a barrier");
    drop(job);

    let ReconcileStep::Publish(status) = scene.poll(now) else {
        panic!("expected read failure status");
    };
    view.apply(status).unwrap();
    assert_matches!(view.status(), Status::Recovering { reason: RecoveryReason::ReadFailed, attempt: 1 });

    let ReconcileStep::Wait(until) = scene.poll(now) else {
        panic!("expected recovery backoff");
    };
    now = until;
    let ReconcileStep::Read(barrier) = scene.poll(now) else {
        panic!("expected recovery barrier");
    };
    assert_matches!(barrier.query(), ReadQuery::Barrier, "recovery starts with a fresh barrier");
    let result = reply(&world, &barrier);
    scene.complete_read(barrier, result, now);
    settle(&mut scene, &mut view, &world, &mut now);

    assert_eq!(view.status(), Status::Ready);
    let retained = view.region(Region::new(1)).expect("recovery restores region");
    assert_eq!(retained.summary().region, Region::new(1));
    assert_eq!(retained.summary().info.resolved_origin(), Some(DVec3::ZERO));
}

#[test]
fn foreign_read_is_ignored() {
    let world = world();
    let (mut owner, _) = Reconciler::new(Budget::default());
    let (mut receiver, _) = Reconciler::new(Budget::default());
    let now = Instant::now();

    let ReconcileStep::Read(foreign) = owner.poll(now) else {
        panic!("expected owner read");
    };
    let ReconcileStep::Read(local) = receiver.poll(now) else {
        panic!("expected receiver read");
    };
    let result = reply(&world, &local);

    receiver.complete_read(foreign, Err(ReadFailure::Busy), now);
    assert_eq!(receiver.status(), Status::Loading);
    receiver.complete_read(local, result, now);

    let ReconcileStep::Read(next) = receiver.poll(now) else {
        panic!("foreign job must not replace the local read");
    };
    assert_matches!(next.query(), ReadQuery::Regions(None), "the receiver keeps its own read sequence");
}
