use std::hint::black_box;

use base::UUID;
use criterion::{BatchSize, BenchmarkGroup, BenchmarkId, Criterion, Throughput, measurement::WallTime};
use world::{
    AgentHandoff,
    ApplyOutcome::{AgentHandedOff, RegionRetired},
    ParentUpdate::Root,
};

use super::support::{INITIAL_ORDER, PER_REGION, Row, SCENE_SIZES, empty_scene, observation, populated_scene, rows, size_name, source};

pub(super) fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("world/lifecycle");
    bench_spawn(&mut group);
    bench_delete(&mut group);
    bench_local_id_reuse(&mut group);
    bench_region_reset(&mut group);
    bench_agent_handoff(&mut group);
    group.finish();
}

fn bench_spawn(group: &mut BenchmarkGroup<'_, WallTime>) {
    for &count in SCENE_SIZES {
        group.throughput(Throughput::Elements(count as u64));
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
                        let RegionRetired { entities_removed } = &applied.outcome else { panic!("expected region retirement") };
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
