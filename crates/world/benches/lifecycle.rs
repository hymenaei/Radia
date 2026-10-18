use std::{hint::black_box, num::NonZeroU32};

use base::UUID;
use criterion::{BatchSize, BenchmarkGroup, BenchmarkId, Criterion, Throughput, measurement::WallTime};
use math::{Quat, Vec3};
use world::{
    AgentHandoff,
    ApplyOutcome::{AgentHandedOff, Introduced, Removed},
    ObjectPatch,
    ParentUpdate::{Local, Root},
    Pose, World,
};

use super::support::{
    INITIAL_ORDER, PER_REGION, Row, SCENE_SIZES, Scene, empty_scene, limits, observation, populated_scene, rows, size_name, source,
};

pub(super) fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("world/lifecycle");
    bench_spawn(&mut group);
    bench_delete(&mut group);
    bench_local_id_reuse(&mut group);
    bench_region_reset(&mut group);
    bench_linked_removal(&mut group);
    bench_agent_handoff(&mut group);
    group.finish();
}

fn bench_spawn(group: &mut BenchmarkGroup<'_, WallTime>) {
    for &count in SCENE_SIZES {
        group.throughput(Throughput::Elements(count as u64));
        let (verification_rows, _) = rows(count, 0);
        let mut verification = empty_scene(count, false);
        for row in &verification_rows {
            assert!(matches!(verification.world.observe_object(observation(*row)).expect("object introduction").outcome, Introduced(_)));
        }
        assert_eq!(verification.world.object_count(), count);

        group.bench_with_input(BenchmarkId::new("spawn", size_name(count)), &count, |b, &count| {
            let rows = rows(count, 0).0;
            b.iter_batched(
                || empty_scene(count, false),
                |mut scene| {
                    for row in &rows {
                        let _ = black_box(scene.world.observe_object(observation(*row)).expect("object introduction"));
                    }
                    black_box(scene.world.object_count());
                },
                BatchSize::LargeInput,
            );
        });
    }
}

fn bench_delete(group: &mut BenchmarkGroup<'_, WallTime>) {
    for &count in SCENE_SIZES {
        group.throughput(Throughput::Elements(count as u64));
        let verification = populated_scene(count, false, false);
        let mut verification_world = verification.world;
        for row in &verification.rows {
            assert_eq!(
                verification_world
                    .remove_object(source(row.region, INITIAL_ORDER + PER_REGION as u64 + u64::from(row.local_id.get())), row.local_id,)
                    .expect("object removal")
                    .outcome,
                Removed(row.id)
            );
        }
        assert_eq!(verification_world.object_count(), 0);

        group.bench_with_input(BenchmarkId::new("delete", size_name(count)), &count, |b, &count| {
            b.iter_batched(
                || populated_scene(count, false, false),
                |mut scene| {
                    for row in &scene.rows {
                        let _ = black_box(
                            scene
                                .world
                                .remove_object(
                                    source(row.region, INITIAL_ORDER + PER_REGION as u64 + u64::from(row.local_id.get())),
                                    row.local_id,
                                )
                                .expect("object removal"),
                        );
                    }
                    black_box(scene.world.object_count());
                },
                BatchSize::LargeInput,
            );
        });
    }
}

fn bench_local_id_reuse(group: &mut BenchmarkGroup<'_, WallTime>) {
    for &count in SCENE_SIZES {
        group.throughput(Throughput::Elements(2));
        let mut verification = populated_scene(count, false, false);
        let target = verification.rows[count / 2];
        let mut order = INITIAL_ORDER + 2 * PER_REGION as u64;
        let removed = verification.world.remove_object(source(target.region, order), target.local_id).expect("object removal");
        assert_eq!(removed.outcome, Removed(target.id));
        order += 1;
        let new_id = UUID::from_u128(count as u128 + 1);
        let introduced =
            verification.world.observe_object(observation(Row { id: new_id, order, ..target })).expect("local ID reintroduction");
        let Introduced(entity) = introduced.outcome else { panic!("expected local ID reintroduction") };
        assert_eq!(verification.world.entity(target.id), None);
        assert_eq!(verification.world.entity(new_id), Some(entity));
        assert_eq!(verification.world.object(entity).unwrap().address().local_id(), target.local_id);
        assert_eq!(verification.world.object_count(), count);

        group.bench_with_input(BenchmarkId::new("local_id_reuse", size_name(count)), &count, |b, &count| {
            let mut scene = populated_scene(count, false, false);
            let target = scene.rows[count / 2];
            let mut order = INITIAL_ORDER + 2 * PER_REGION as u64;
            let mut next_id = count as u128 + 1;
            b.iter(|| {
                let _ = black_box(scene.world.remove_object(source(target.region, order), target.local_id).expect("object removal"));
                order += 1;
                let id = UUID::from_u128(next_id);
                next_id += 1;
                let row = Row { id, order, ..target };
                let _ = black_box(scene.world.observe_object(observation(row)).expect("local ID reintroduction"));
                order += 1;
            });
        });
    }
}

fn bench_region_reset(group: &mut BenchmarkGroup<'_, criterion::measurement::WallTime>) {
    for &count in SCENE_SIZES {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::new("region_reset", size_name(count)), &count, |b, &count| {
            b.iter_batched(
                || populated_scene(count, false, false),
                |mut scene| {
                    let mut removed = 0;
                    for region in &scene.regions {
                        let applied = scene.world.retire_region(*region).expect("region retirement");
                        let world::RegionOutcome::Retired { entities_removed, .. } = &applied.outcome else {
                            panic!("expected region retirement")
                        };
                        removed += *entities_removed;
                        let _ = black_box(applied);
                    }
                    assert_eq!(removed, count);
                    black_box(scene.world.object_count());
                },
                BatchSize::LargeInput,
            );
        });
    }
}

fn bench_agent_handoff(group: &mut BenchmarkGroup<'_, WallTime>) {
    group.throughput(Throughput::Elements(1));
    for &count in SCENE_SIZES {
        group.bench_with_input(BenchmarkId::new("agent_handoff", size_name(count)), &count, |b, &count| {
            let mut scene = populated_scene(count, true, true);
            let agent = scene.rows[0];
            let destination = *scene.regions.last().expect("destination region");
            let mut in_source = true;
            let mut source_order = INITIAL_ORDER + PER_REGION as u64 + 1;
            b.iter(|| {
                let (from, to) = if in_source { (agent.region, destination) } else { (destination, agent.region) };
                let applied = scene
                    .world
                    .handoff_agent(
                        from,
                        AgentHandoff {
                            source: source(to, source_order),
                            local_id: agent.local_id,
                            parent: Root,
                            pose: None,
                            linear_motion: None,
                            angular_velocity: None,
                            scale: None,
                        },
                    )
                    .expect("agent handoff");
                assert!(matches!(&applied.outcome, AgentHandedOff(_)));
                in_source = !in_source;
                source_order += 1;
                let _ = black_box(applied);
            });
        });
    }
}

fn linked_scene(count: usize, linkset_size: usize) -> Scene {
    let (mut rows, mut regions) = rows(count, 0);
    regions.truncate(1);
    let mut world_limits = limits();
    world_limits.max_entities_per_region = count;
    let mut world = World::new(world_limits);
    let _ = world.adopt_region(regions[0]).expect("adopt linked region");
    for (index, row) in rows.iter_mut().enumerate() {
        row.region = regions[0];
        row.local_id = NonZeroU32::new(index as u32 + 1).unwrap();
        row.order = INITIAL_ORDER + u64::from(row.local_id.get());
        let applied = world.observe_object(observation(*row)).expect("introduce linked object");
        assert!(matches!(applied.outcome, Introduced(_)));
    }
    let mut scene = Scene { world, rows, regions };
    for (index, row) in scene.rows.iter().enumerate() {
        if index % linkset_size == 0 {
            continue;
        }
        let root = scene.rows[index / linkset_size * linkset_size];
        let mut patch = ObjectPatch::new(source(row.region, INITIAL_ORDER + count as u64 + u64::from(row.local_id.get())), row.local_id);
        patch.parent = Some(Local(root.local_id));
        patch.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
        let _ = scene.world.patch_object(patch).expect("link child");
    }
    scene
}

fn bench_linked_removal(group: &mut BenchmarkGroup<'_, WallTime>) {
    for (count, linkset_size) in [(10_000, 10_000), (65_536, 65_536), (65_536, 128)] {
        group.throughput(Throughput::Elements(count as u64));
        for retire in [false, true] {
            if !retire && linkset_size != count {
                continue;
            }
            let name = if !retire {
                "delete_linkset"
            } else if linkset_size == count {
                "retire_linkset"
            } else {
                "retire_linksets"
            };
            group.bench_function(BenchmarkId::new(name, size_name(count)), |b| {
                b.iter_batched_ref(
                    || linked_scene(count, linkset_size),
                    |scene| {
                        let root = scene.rows[0];
                        if retire {
                            let _ = black_box(scene.world.retire_region(root.region).expect("retire linked region"));
                        } else {
                            let _ = black_box(
                                scene
                                    .world
                                    .remove_object(source(root.region, INITIAL_ORDER + 3 * count as u64), root.local_id)
                                    .expect("remove linked root"),
                            );
                        }
                        assert_eq!(scene.world.object_count(), 0);
                    },
                    BatchSize::LargeInput,
                );
            });
        }
    }
}
