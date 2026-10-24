use std::time::Instant;

use math::DVec3;
use render::scene::{Budget, ReadFailure, ReadJob, ReadQuery, ReadReply, ReconcileStep, Reconciler, RecoveryReason, Status};
use world::{Region, RegionInfo, World, WorldLimits};

fn world() -> World {
    let mut world = World::new(WorldLimits {
        max_region_registrations: 2,
        max_entities: 1,
        max_entities_per_region: 1,
        max_tombstones_per_region: 1,
        max_tombstones: 1,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
        max_journal_entries: 16,
        max_journal_changes: 32,
        max_region_name_bytes: 64,
        max_recovery_targets: 1,
    });
    let region = Region::new(1);
    let _ = world.adopt_region(region).expect("region admission");
    let _ = world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).expect("region update");
    world
}

fn reply(world: &World, job: &ReadJob) -> Result<ReadReply, ReadFailure> {
    match job.query() {
        ReadQuery::Barrier => Ok(ReadReply::Barrier(world.read_barrier())),
        ReadQuery::Objects(cursor) => world.object_list_page(*cursor, 256).map(ReadReply::Objects),
        ReadQuery::Regions(cursor) => world.region_page(*cursor, 64).map(ReadReply::Regions),
        ReadQuery::Details(entities) => world.object_detail_batch(entities).map(ReadReply::Details),
        ReadQuery::Changes(cursor) => world.change_page(*cursor, 256).map(ReadReply::Changes),
        ReadQuery::Terrain(region, cursor) => world.terrain_page(*region, *cursor, 8).map(ReadReply::Terrain),
        ReadQuery::Environment(region) => Ok(ReadReply::Environment(world.environment(*region))),
    }
    .map_err(ReadFailure::World)
}

#[test]
fn abandoned_read_recovers() {
    let world = world();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();

    let ReconcileStep::Read(job) = scene.poll(now) else {
        panic!("expected initial read");
    };
    assert!(matches!(job.query(), ReadQuery::Barrier));
    drop(job);

    let ReconcileStep::Publish(status) = scene.poll(now) else {
        panic!("expected read failure status");
    };
    view.apply(status).unwrap();
    assert!(matches!(view.status(), Status::Recovering { reason: RecoveryReason::ReadFailed, attempt: 1 }));

    let ReconcileStep::Wait(until) = scene.poll(now) else {
        panic!("expected recovery backoff");
    };
    now = until;
    let ReconcileStep::Read(barrier) = scene.poll(now) else {
        panic!("expected recovery barrier");
    };
    assert!(matches!(barrier.query(), ReadQuery::Barrier));
    let result = reply(&world, &barrier);
    scene.complete_read(barrier, result, now);

    for _ in 0..32 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            ReconcileStep::Idle => break,
            ReconcileStep::Asset(_) => panic!("empty world must not request assets"),
            ReconcileStep::Closed => panic!("scene must remain open"),
        }
        if view.status() == Status::Ready {
            break;
        }
    }

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
    assert!(matches!(next.query(), ReadQuery::Regions(None)));
}
