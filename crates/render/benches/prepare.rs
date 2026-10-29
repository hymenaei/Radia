use std::{
    f64::consts::PI,
    hint::black_box,
    time::{Duration, Instant},
};

use criterion::Criterion;
use futures::executor::block_on;
use math::{DVec3, Vec3};
use render::{
    GeometryPrepare,
    scene::{Camera, Reconciler, SceneView},
};
use wgpu::{DeviceDescriptor, PowerPreference, RequestAdapterOptions, TextureFormat};
use world::World;

use crate::support;

const OBJECTS: usize = 3_000;
const TEXTURES: usize = 512;
const ANIMATED_OBJECTS: usize = 500;
const SIZE: [u32; 2] = [1024, 1024];

struct Fixture {
    world: World,
    rows: Vec<support::Row>,
    textures: support::BenchmarkTextures,
    reconciler: Reconciler,
    view: SceneView,
    now: Instant,
    prepare: GeometryPrepare,
}

impl Fixture {
    fn settled(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        regions: usize,
        meshes: usize,
        animated: usize,
        start_x: f64,
        draw_distance: f64,
    ) -> Self {
        let (mut world, rows) = support::live_prepare_world_with_regions(OBJECTS, meshes, TEXTURES, regions);
        if animated > 0 {
            support::animate_objects(&mut world, &rows, animated, 4.0, true);
        }
        let textures = support::benchmark_textures(TEXTURES);
        let scene_camera = camera_at(start_x, 0.0, draw_distance);
        let (reconciler, view, now) = support::settle_with_textures(&world, OBJECTS, scene_camera, &textures.textures);
        let prepare = GeometryPrepare::new(device, queue, TextureFormat::Rgba8UnormSrgb);
        let mut fixture = Self { world, rows, textures, reconciler, view, now, prepare };
        let mut setup_cameras = Vec::with_capacity(regions * 2 + 1);
        for region in 0..regions {
            setup_cameras.push(camera_at(region as f64 * 256.0, 0.0, 1024.0));
            setup_cameras.push(camera_at(region as f64 * 256.0 + 128.0, 0.0, 1024.0));
        }
        setup_cameras.push(camera_at(start_x, 0.0, draw_distance));
        for camera in setup_cameras {
            for _ in 0..TEXTURES + 32 {
                fixture.sample(device, queue, camera);
                complete_texture_work(&mut fixture.prepare, TEXTURES);
                if fixture.prepare.stats().pending_meshes == 0 && fixture.prepare.stats().pending_textures == 0 {
                    break;
                }
            }
        }
        fixture.sample(device, queue, camera_at(start_x, 0.0, draw_distance));
        assert_eq!(fixture.prepare.stats().pending_meshes, 0, "fixture meshes must be resident before timing");
        assert_eq!(fixture.prepare.stats().pending_textures, 0, "fixture textures must be resident before timing");
        assert!(fixture.prepare.stats().instances > 0);
        assert_eq!(fixture.prepare.stats().omitted_faces, 0);
        fixture
    }

    fn sample(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, camera: Camera) -> Duration {
        prepare_sample(&mut self.prepare, device, queue, &self.view, camera)
    }
}

pub(crate) fn bench(c: &mut Criterion) {
    let instance = wgpu::Instance::default();
    let adapter = block_on(instance.request_adapter(&RequestAdapterOptions {
        power_preference: PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .expect("benchmark GPU adapter");
    eprintln!("render prepare benchmark GPU: {:?}", adapter.get_info());
    let (device, queue) = block_on(adapter.request_device(&DeviceDescriptor {
        required_features: adapter.features() & (wgpu::Features::TEXTURE_COMPRESSION_BC | wgpu::Features::INDIRECT_FIRST_INSTANCE),
        ..Default::default()
    }))
    .expect("benchmark GPU device");
    let mut group = c.benchmark_group("render/geometry_prepare");
    bench_unchanged(&mut group, &device, &queue);
    bench_camera_walk(&mut group, &device, &queue);
    bench_pose(&mut group, &device, &queue);
    bench_reprepare(&mut group, &device, &queue, 50, "stationary_one_reprepare_50_meshes");
    bench_live_mix(&mut group, &device, &queue);
    bench_animated(&mut group, &device, &queue);
    bench_reprepare(&mut group, &device, &queue, OBJECTS, "one_reprepare_3000_unique_meshes");
    group.finish();
}

fn bench_unchanged(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    let mut fixture = Fixture::settled(device, queue, 1, 50, 0, 50.0, 512.0);
    let camera = camera_at(50.0, 0.0, 512.0);
    fixture.sample(device, queue, camera);
    let mut latest = fixture.prepare.stats();
    group.bench_function("unchanged_static_camera", |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iters {
                elapsed += fixture.sample(device, queue, camera);
                latest = fixture.prepare.stats();
                assert_eq!(latest.pending_meshes, 0);
                assert_eq!(latest.pending_textures, 0);
                assert_eq!(latest.omitted_faces, 0);
                assert_eq!(latest.layouts, 0, "unchanged frames reuse the prepared draw list");
                assert_eq!(latest.items_updated, 0);
                assert_eq!(latest.faces_patched, 0);
            }
            elapsed
        });
    });
    report("unchanged_static_camera", latest);
}

fn bench_camera_walk(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    let mut fixture = Fixture::settled(device, queue, 7, 50, 0, 896.0, 1024.0);
    let mut generation = 0usize;
    let mut layouts = 0usize;
    let mut latest = fixture.prepare.stats();
    group.bench_function("camera_walk_025_multi_region", |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iters {
                generation = generation.wrapping_add(1);
                let step = (generation % 2048) as f64 * 0.25;
                elapsed += fixture.sample(device, queue, camera_at(768.0 + step, step, 1024.0));
                latest = fixture.prepare.stats();
                layouts += latest.layouts;
                assert_eq!(latest.items_updated, 0, "camera movement does not update cached items");
                assert_eq!(latest.pending_meshes, 0, "camera walk must use resident meshes");
                assert_eq!(latest.pending_textures, 0, "camera walk must use resident textures");
                assert_eq!(latest.omitted_faces, 0);
            }
            assert!(layouts > 0, "multi-region camera walk must relayout when visible items change");
            elapsed
        });
    });
    report("camera_walk_025_multi_region", latest);
}

fn bench_pose(group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>, device: &wgpu::Device, queue: &wgpu::Queue) {
    let mut fixture = Fixture::settled(device, queue, 1, 50, 0, 50.0, 512.0);
    let camera = camera_at(50.0, 0.0, 512.0);
    let mut generation = 0;
    let mut order = 20_000_000;
    let mut latest = fixture.prepare.stats();
    group.bench_function("one_pose_change", |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iters {
                generation += 1;
                support::update_one(&mut fixture.world, &fixture.rows[0], &mut order, generation);
                support::first_publish(&mut fixture.reconciler, &mut fixture.view, &fixture.world, &mut fixture.now);
                elapsed += fixture.sample(device, queue, camera);
                latest = fixture.prepare.stats();
                assert!(latest.items_updated > 0, "pose update must reach the cache");
                assert!(latest.faces_patched > 0, "pose update must patch packed instance bytes");
                assert_eq!(latest.omitted_faces, 0);
            }
            elapsed
        });
    });
    report("one_pose_change", latest);
}

fn bench_reprepare(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mesh_count: usize,
    name: &'static str,
) {
    let mut fixture = Fixture::settled(device, queue, 1, mesh_count, 0, 50.0, 512.0);
    let camera = camera_at(50.0, 0.0, 512.0);
    let mut generation = 0;
    let mut order = 20_000_000;
    let mut latest = fixture.prepare.stats();
    group.bench_function(name, |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iters {
                generation += 1;
                let texture = support::update_texture(&mut fixture.world, &fixture.rows[0], &mut order, generation);
                support::publish_texture_update(
                    &mut fixture.reconciler,
                    &mut fixture.view,
                    &fixture.world,
                    &mut fixture.now,
                    &fixture.rows[0],
                    texture,
                );
                elapsed += fixture.sample(device, queue, camera);
                latest = fixture.prepare.stats();
                assert!(latest.items_updated > 0, "appearance update must refresh an object");
                assert_eq!(latest.layouts, 1, "appearance change must rebuild the draw list");
                assert_eq!(latest.omitted_faces, 0);
            }
            elapsed
        });
    });
    report(name, latest);
}

fn bench_live_mix(group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>, device: &wgpu::Device, queue: &wgpu::Queue) {
    let mut fixture = Fixture::settled(device, queue, 7, 50, 0, 896.0, 1024.0);
    let mut generation = 0u64;
    let mut order = 20_000_000;
    let mut latest = fixture.prepare.stats();
    group.bench_function("live_walk_reprepare_pose4_texture_arrivals", |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iters {
                generation += 1;
                let texture = support::update_texture(&mut fixture.world, &fixture.rows[0], &mut order, generation);
                support::update_many(&mut fixture.world, &fixture.rows[1..5], 4, &mut order, generation);
                support::publish_texture_update(
                    &mut fixture.reconciler,
                    &mut fixture.view,
                    &fixture.world,
                    &mut fixture.now,
                    &fixture.rows[0],
                    texture,
                );
                if generation.is_multiple_of(20) {
                    let texture_indexes = [generation as usize % 508, (generation as usize + 1) % 508];
                    let refinement_size = 128 << ((generation as usize / 20).min(4));
                    support::complete_texture_refinements(&mut fixture.textures, texture_indexes, refinement_size, Instant::now());
                }
                let step = (generation % 2048) as f64 * 0.25;
                elapsed += fixture.sample(device, queue, camera_at(768.0 + step, step, 1024.0));
                latest = fixture.prepare.stats();
                assert!(latest.items_updated >= 5, "live mix must include one appearance update and four pose updates");
                assert_eq!(latest.layouts, 1, "appearance change must rebuild the draw list");
                assert_eq!(latest.omitted_faces, 0);
                complete_texture_work(&mut fixture.prepare, 5);
            }
            elapsed
        });
    });
    report("live_walk_reprepare_pose4_texture_arrivals", latest);
}

fn bench_animated(group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>, device: &wgpu::Device, queue: &wgpu::Queue) {
    let mut fixture = Fixture::settled(device, queue, 1, 50, ANIMATED_OBJECTS, 50.0, 512.0);
    let camera = camera_at(50.0, 0.0, 512.0);
    let mut visited = 0usize;
    let mut changed = 0usize;
    let mut latest = fixture.prepare.stats();
    group.bench_function("animated_500_objects", |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iters {
                elapsed += fixture.sample(device, queue, camera);
                latest = fixture.prepare.stats();
                visited += latest.animated_faces_visited;
                changed += latest.animated_faces_changed;
                assert_eq!(latest.omitted_faces, 0);
                assert_eq!(latest.blended_sorts, 0, "parameter animation must not resort blended faces");
            }
            assert!(visited > 0, "animated scene must visit due faces");
            assert!(changed > 0, "animated scene must patch changed parameters");
            elapsed
        });
    });
    report("animated_500_objects", latest);
}

fn complete_texture_work(prepare: &mut GeometryPrepare, limit: usize) -> usize {
    let mut completed = 0;
    while completed < limit {
        let Some(job) = prepare.next_texture_job() else { break };
        prepare.complete_texture(job.run(render::CompressionQuality::Fast));
        completed += 1;
    }
    completed
}

fn prepare_sample(prepare: &mut GeometryPrepare, device: &wgpu::Device, queue: &wgpu::Queue, view: &SceneView, camera: Camera) -> Duration {
    let mut encoder = device.create_command_encoder(&Default::default());
    let started = Instant::now();
    prepare.prepare(device, &mut encoder, view, camera, SIZE, None);
    let elapsed = started.elapsed();
    let stats = prepare.stats();
    black_box((stats.instances, stats.draw_calls, stats.draw_commands, stats.omitted_faces, stats.pending_meshes, stats.pending_textures));
    queue.submit([encoder.finish()]);
    prepare.submitted();
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("benchmark GPU completion");
    elapsed
}

fn report(name: &str, stats: render::Stats) {
    eprintln!(
        "{name}: draw_submissions={}, draw_commands={}, omitted_faces={}",
        stats.draw_calls, stats.draw_commands, stats.omitted_faces
    );
}

fn camera_at(x: f64, rotation: f64, draw_distance: f64) -> Camera {
    let angle = rotation * 0.00005;
    let direction = Vec3::new(angle.sin() as f32, 0.0, -angle.cos() as f32);
    Camera::new(DVec3::new(x, 15.0, 120.0), direction, PI / 2.0, SIZE)
        .expect("benchmark camera")
        .with_quality(draw_distance, 1.0)
        .expect("benchmark camera quality")
}
