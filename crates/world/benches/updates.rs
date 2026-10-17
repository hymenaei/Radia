use std::hint::black_box;

use base::UUID;
use criterion::{BenchmarkGroup, BenchmarkId, Criterion, Throughput, measurement::WallTime};
use math::{Quat, Vec3};
use world::{ApplyOutcome::Unchanged, MotionPatch, ObjectPatch, ParentUpdate::Root, Pose, PropertiesPatch};

use super::support::{INITIAL_ORDER, PER_REGION, Row, SCENE_SIZES, observation, observation_with_pose, populated_scene, size_name, source};

pub(super) fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("world/updates");
    bench_transform(&mut group);
    bench_duplicate(&mut group);
    bench_stale(&mut group);
    bench_mixed(&mut group);
    group.finish();
}

fn bench_transform(group: &mut BenchmarkGroup<'_, WallTime>) {
    for &count in SCENE_SIZES {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::new("transform", size_name(count)), &count, |b, &count| {
            let mut scene = populated_scene(count, false, false);
            let mut order = INITIAL_ORDER + PER_REGION as u64 + 1;
            let mut alternate = false;
            b.iter(|| {
                let pose = if alternate {
                    Pose { position: Some(Vec3::splat(2.0)), rotation: Some(Quat::IDENTITY) }
                } else {
                    Pose { position: Some(Vec3::ONE), rotation: Some(Quat::from_rotation_y(0.25)) }
                };
                alternate = !alternate;
                for row in &scene.rows {
                    let mut patch = ObjectPatch::new(source(row.region, order), row.local_id);
                    order += 1;
                    patch.pose = Some(pose);
                    let _ = black_box(scene.world.patch_object(patch).expect("transform update"));
                }
            });
        });
    }
}

fn bench_duplicate(group: &mut BenchmarkGroup<'_, WallTime>) {
    for &count in SCENE_SIZES {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::new("duplicate", size_name(count)), &count, |b, &count| {
            let mut scene = populated_scene(count, false, false);
            for row in &scene.rows {
                let applied = scene.world.observe_object(observation(*row)).expect("duplicate observation");
                assert_eq!(applied.outcome, Unchanged);
            }
            b.iter(|| {
                for row in &scene.rows {
                    let _ = black_box(scene.world.observe_object(observation(*row)).expect("duplicate observation"));
                }
            });
        });
    }
}

fn bench_stale(group: &mut BenchmarkGroup<'_, WallTime>) {
    for &count in SCENE_SIZES {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::new("stale", size_name(count)), &count, |b, &count| {
            let mut scene = populated_scene(count, false, false);
            for row in &scene.rows {
                let mut patch = ObjectPatch::new(source(row.region, row.order - 1), row.local_id);
                patch.pose = Some(Pose { position: Some(Vec3::ONE), rotation: Some(Quat::IDENTITY) });
                let applied = scene.world.patch_object(patch).expect("stale update");
                assert_eq!(applied.outcome, Unchanged);
            }
            b.iter(|| {
                for row in &scene.rows {
                    let mut patch = ObjectPatch::new(source(row.region, row.order - 1), row.local_id);
                    patch.pose = Some(Pose { position: Some(Vec3::ONE), rotation: Some(Quat::IDENTITY) });
                    let _ = black_box(scene.world.patch_object(patch).expect("stale update"));
                }
            });
        });
    }
}

fn bench_mixed(group: &mut BenchmarkGroup<'_, WallTime>) {
    for &count in SCENE_SIZES {
        let recycle_count = count.div_ceil(16);
        group.throughput(Throughput::Elements((count * 5 + recycle_count * 2) as u64));
        group.bench_with_input(BenchmarkId::new("mixed", size_name(count)), &count, |b, &count| {
            let mut scene = populated_scene(count, false, false);
            let mut base_order = 200_000_u64;
            let mut next_id = count as u128 + 1;
            let mut round = 0_u64;
            let mut duplicate_pose = Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) };
            for row in &scene.rows {
                let applied = scene.world.observe_object(observation(*row)).expect("duplicate observation");
                assert_eq!(applied.outcome, Unchanged);
            }
            b.iter(|| {
                let alternate = round % 2 == 1;
                let position = if alternate { Vec3::splat(2.0) } else { Vec3::ONE };
                let rotation = if alternate { Quat::IDENTITY } else { Quat::from_rotation_y(0.25) };
                let pose = Pose { position: Some(position), rotation: Some(rotation) };
                let (velocity, acceleration) = if alternate { (Vec3::Y, Vec3::Z) } else { (Vec3::X, Vec3::Y) };
                let owner = UUID::from_u128(u128::MAX - 2 - u128::from(round));
                let creator = UUID::from_u128(u128::MAX - 3 - u128::from(round));

                for row in &scene.rows {
                    let _ =
                        black_box(scene.world.observe_object(observation_with_pose(*row, duplicate_pose)).expect("duplicate observation"));
                }
                for row in &mut scene.rows {
                    let order = base_order + u64::from(row.local_id.get());
                    let mut patch = ObjectPatch::new(source(row.region, order), row.local_id);
                    patch.parent = Some(Root);
                    patch.pose = Some(pose);
                    patch.scale = Some(Vec3::ONE);
                    let _ = black_box(scene.world.patch_object(patch).expect("transform update"));
                    row.order = order;
                }
                for row in &scene.rows {
                    let mut patch = ObjectPatch::new(source(row.region, row.order), row.local_id);
                    patch.linear_motion = MotionPatch { velocity: Some(velocity), acceleration: Some(acceleration) };
                    let _ = black_box(scene.world.patch_object(patch).expect("motion update"));
                }
                for row in &scene.rows {
                    let entity = scene.world.entity(row.id).expect("object identity");
                    let target = scene.world.properties_target(entity).expect("current object binding");
                    let _ = black_box(
                        scene
                            .world
                            .patch_properties(
                                source(row.region, row.order),
                                target,
                                row.id,
                                PropertiesPatch { owner_id: Some(owner), creator_id: Some(creator) },
                            )
                            .expect("property update"),
                    );
                }
                for row in &scene.rows {
                    let mut patch = ObjectPatch::new(source(row.region, row.order - 1), row.local_id);
                    patch.pose = Some(Pose { position: Some(Vec3::splat(3.0)), rotation: Some(Quat::IDENTITY) });
                    let _ = black_box(scene.world.patch_object(patch).expect("stale update"));
                }
                for index in (0..scene.rows.len()).step_by(16) {
                    let row = scene.rows[index];
                    let remove_order = base_order + 300_000 + index as u64 * 2;
                    let _ = black_box(scene.world.remove_object(source(row.region, remove_order), row.local_id).expect("object removal"));
                    let replacement = Row { id: UUID::from_u128(next_id), order: remove_order + 1, ..row };
                    next_id += 1;
                    let _ = black_box(scene.world.observe_object(observation_with_pose(replacement, pose)).expect("local ID reuse"));
                    scene.rows[index] = replacement;
                }
                duplicate_pose = pose;
                base_order += 500_000;
                round += 1;
            });
        });
    }
}
