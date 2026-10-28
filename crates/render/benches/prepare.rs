use std::{
    collections::HashSet,
    hint::black_box,
    time::{Duration, Instant},
};

use criterion::Criterion;
use futures::executor::block_on;
use math::{DVec3, Vec3};
use render::{GeometryPrepare, scene::Camera};
use wgpu::{DeviceDescriptor, PowerPreference, RequestAdapterOptions, TextureFormat};

use crate::support;

const OBJECTS: usize = 3_000;
const DISTINCT_MESHES: [usize; 2] = [50, OBJECTS];
const LIVE_TEXTURES: usize = 512;
const OBJECT_TEXTURES: usize = LIVE_TEXTURES - 4;
const ANIMATED_OBJECTS: usize = 500;
const SIZE: [u32; 2] = [1024, 1024];

pub(crate) fn bench(c: &mut Criterion) {
    let instance = wgpu::Instance::default();
    let adapter = block_on(instance.request_adapter(&RequestAdapterOptions {
        power_preference: PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .expect("benchmark GPU adapter");
    eprintln!("render prepare benchmark GPU: {:?}", adapter.get_info());
    eprintln!("render prepare timestamp queries: {}", adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY));
    let (device, queue) = block_on(adapter.request_device(&DeviceDescriptor {
        required_features: adapter.features() & (wgpu::Features::TEXTURE_COMPRESSION_BC | wgpu::Features::INDIRECT_FIRST_INSTANCE),
        ..Default::default()
    }))
    .expect("benchmark GPU device");
    let mut group = c.benchmark_group("render/geometry_prepare");
    bench_animated_textures(&mut group, &device, &queue);
    for meshes in DISTINCT_MESHES {
        bench_scene(&mut group, &device, &queue, meshes);
    }
    bench_live_like(&mut group, &device, &queue, 50);
    bench_live_like(&mut group, &device, &queue, OBJECTS);
    group.finish();
}

fn bench_animated_textures(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    let (mut world, rows) = support::live_prepare_world(OBJECTS, 50, LIVE_TEXTURES);
    support::animate_objects(&mut world, &rows, ANIMATED_OBJECTS, 4.0, true);
    let textures = support::benchmark_textures(LIVE_TEXTURES);
    let camera = support::prepare_camera();
    let (_reconciler, view, _now) = support::settle_with_textures(&world, OBJECTS, camera, &textures.textures);
    let animated_objects = view.objects().filter(|object| object.detail().texture_animation.is_some()).count();
    assert_eq!(animated_objects, ANIMATED_OBJECTS);
    let mesh_count = view.objects().filter_map(|object| object.mesh().map(|(_, mesh)| mesh.id())).collect::<HashSet<_>>().len();
    let mut prepare = GeometryPrepare::new(device, queue, TextureFormat::Rgba8UnormSrgb);
    for _ in 0..LIVE_TEXTURES + 32 {
        prepare_sample(&mut prepare, device, queue, &view, camera);
        while let Some(job) = prepare.next_texture_job() {
            prepare.complete_texture(job.run(render::CompressionQuality::Fast));
        }
        if prepare.stats().pending_meshes == 0 && prepare.stats().pending_textures == 0 {
            break;
        }
    }
    assert_eq!(prepare.stats().pending_meshes, 0);
    assert_eq!(prepare.stats().pending_textures, 0);
    assert_eq!(prepare.stats().omitted_faces, 0);
    prepare_sample(&mut prepare, device, queue, &view, camera);
    assert!(prepare.stats().animated_faces_visited > 0, "visible animated texture faces must reach the prepare path");
    eprintln!(
        "animated-texture fixture: objects={OBJECTS}, meshes={mesh_count}, animated_objects={ANIMATED_OBJECTS}, blended_faces={}, textures={LIVE_TEXTURES}, draws={}, omitted_faces={}, smooth_rate_hz=4",
        prepare.stats().animated_faces_visited,
        prepare.stats().draw_calls,
        prepare.stats().omitted_faces
    );
    let mut latest = (0usize, 0usize, 0.0_f64);
    group.bench_function("animated_smooth_500_of_3000", |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::ZERO;
            let mut visited = 0usize;
            let mut changed = 0usize;
            let mut animation_update_ms = 0.0;
            for _ in 0..iters {
                elapsed += prepare_sample(&mut prepare, device, queue, &view, camera);
                visited += prepare.stats().animated_faces_visited;
                changed += prepare.stats().animated_faces_changed;
                animation_update_ms += prepare.stats().animation_update_ms;
            }
            latest = (visited / iters.max(1) as usize, changed / iters.max(1) as usize, animation_update_ms / iters.max(1) as f64);
            elapsed
        });
    });
    eprintln!("animated-texture faces per prepare: visited={}, changed={}, update_cpu={:.3} ms", latest.0, latest.1, latest.2);
}

fn bench_live_like(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mesh_count: usize,
) {
    let (mut world, rows) = support::live_prepare_world(OBJECTS, mesh_count, LIVE_TEXTURES);
    let mut textures = support::benchmark_textures(LIVE_TEXTURES);
    let camera = support::prepare_camera();
    let (mut reconciler, mut view, mut now) = support::settle_with_textures(&world, OBJECTS, camera, &textures.textures);
    let mut prepare = GeometryPrepare::new(device, queue, TextureFormat::Rgba8UnormSrgb);
    for _ in 0..mesh_count.div_ceil(16) + 8 {
        prepare_sample(&mut prepare, device, queue, &view, camera);
        if prepare.stats().pending_meshes == 0 {
            break;
        }
    }
    assert_eq!(prepare.stats().pending_meshes, 0, "live-like fixture meshes must be resident before timing");
    assert!(prepare.stats().pending_textures > 0, "live-like fixture keeps texture arrivals in flight");
    assert!(prepare.stats().instances > OBJECTS / 2, "live-like camera should retain most scene instances");
    assert_eq!(prepare.stats().omitted_faces, 0);
    eprintln!(
        "live-like fixture: objects={OBJECTS}, meshes={mesh_count}, textures={LIVE_TEXTURES}, faces~20000, terrain_patches=36, draws={}, omitted_faces={}",
        prepare.stats().draw_calls,
        prepare.stats().omitted_faces
    );

    let mut order = 20_000_000;
    let mut generation = 0u64;
    let mut counters = [0usize; 9];
    let mut frames = 0usize;
    let mut content_frame_samples = Vec::new();
    let mut texture_update_samples = Vec::new();
    let mut slowest_content_frame = (Duration::ZERO, [0usize; 8]);
    let live_like = format!("live_like_3000_{mesh_count}_meshes_pose50_camera_texture_arrivals");
    group.bench_function(&live_like, |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iters {
                generation += 1;
                support::update_many(&mut world, &rows, 50, &mut order, generation);
                let first_texture = ((generation as usize - 1) * 2) % OBJECT_TEXTURES;
                let texture_indexes = [first_texture, (first_texture + 1) % OBJECT_TEXTURES];
                support::first_publish(&mut reconciler, &mut view, &world, &mut now);
                let sample =
                    prepare_sample(&mut prepare, device, queue, &view, camera_at(if generation.is_multiple_of(2) { 49.0 } else { 51.0 }));
                elapsed += sample;
                content_frame_samples.push(sample);
                let terrain_completions = if generation <= 200 && generation.is_multiple_of(20) {
                    let pair = if (generation / 20).is_multiple_of(2) {
                        [OBJECT_TEXTURES, OBJECT_TEXTURES + 1]
                    } else {
                        [OBJECT_TEXTURES + 2, OBJECT_TEXTURES + 3]
                    };
                    let size = 128u32 << (((generation - 20) / 40).min(4) as u32);
                    support::complete_texture_refinements(&mut textures, pair, size, Instant::now())
                } else {
                    0
                };
                let stats = prepare.stats();
                if stats.texture_state_changes > 0 {
                    texture_update_samples.push(sample);
                }
                if sample > slowest_content_frame.0 {
                    slowest_content_frame = (
                        sample,
                        [
                            stats.full_rebuilds,
                            stats.region_invalidations,
                            stats.texture_state_changes,
                            stats.texture_state_reresolves,
                            stats.objects_reprepared,
                            stats.dirtied_items,
                            stats.draw_calls,
                            stats.omitted_faces,
                        ],
                    );
                }
                counters[0] += stats.full_rebuilds;
                counters[1] += stats.region_invalidations;
                counters[2] += stats.texture_state_reresolves;
                counters[3] += stats.objects_reprepared;
                counters[4] += stats.dirtied_items;
                counters[5] += complete_texture_work(&mut prepare, 5);
                counters[6] += stats.texture_state_changes;
                let refinement_size = 128u32 << (((generation as usize - 1) / (LIVE_TEXTURES / 2)).min(4));
                counters[7] += support::complete_texture_refinements(&mut textures, texture_indexes, refinement_size, Instant::now());
                counters[8] += terrain_completions;
                frames += 1;
            }
            elapsed
        });
    });
    if frames > 0 {
        eprintln!(
            "live-like prepare reasons per frame: full_rebuilds={:.2}, region_invalidations={:.2}, texture_state_changes={:.2}, texture_state_reresolves={:.2}, objects_reprepared={:.2}, dirtied_items={:.2}; renderer texture jobs/frame={:.2}, asset texture completions/frame={:.2}, terrain texture completions/frame={:.2}",
            counters[0] as f64 / frames as f64,
            counters[1] as f64 / frames as f64,
            counters[6] as f64 / frames as f64,
            counters[2] as f64 / frames as f64,
            counters[3] as f64 / frames as f64,
            counters[4] as f64 / frames as f64,
            counters[5] as f64 / frames as f64,
            counters[7] as f64 / frames as f64,
            counters[8] as f64 / frames as f64
        );
    }
    if !content_frame_samples.is_empty() {
        content_frame_samples.sort_unstable();
        let percentile = |rank: usize| content_frame_samples[(content_frame_samples.len() * rank).div_ceil(100).saturating_sub(1)];
        eprintln!(
            "live-like content-frame prepare: p50={:.2} ms, p95={:.2} ms, max={:.2} ms; slowest counters: full_rebuilds={}, region_invalidations={}, texture_state_changes={}, texture_state_reresolves={}, objects_reprepared={}, dirtied_items={}, draws={}, omitted_faces={}",
            percentile(50).as_secs_f64() * 1000.0,
            percentile(95).as_secs_f64() * 1000.0,
            content_frame_samples.last().expect("content frame sample").as_secs_f64() * 1000.0,
            slowest_content_frame.1[0],
            slowest_content_frame.1[1],
            slowest_content_frame.1[2],
            slowest_content_frame.1[3],
            slowest_content_frame.1[4],
            slowest_content_frame.1[5],
            slowest_content_frame.1[6],
            slowest_content_frame.1[7]
        );
    }
    if !texture_update_samples.is_empty() {
        texture_update_samples.sort_unstable();
        let percentile = |rank: usize| texture_update_samples[(texture_update_samples.len() * rank).div_ceil(100).saturating_sub(1)];
        eprintln!(
            "live-like texture-update frames: count={}, p50={:.2} ms, p95={:.2} ms, max={:.2} ms",
            texture_update_samples.len(),
            percentile(50).as_secs_f64() * 1000.0,
            percentile(95).as_secs_f64() * 1000.0,
            texture_update_samples.last().expect("texture update sample").as_secs_f64() * 1000.0
        );
    }
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

fn bench_scene(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    meshes: usize,
) {
    let (mut world, rows) = support::prepare_world(OBJECTS, meshes);
    let camera = support::prepare_camera();
    let (mut reconciler, mut view, mut now) = support::settle_with_camera(&world, OBJECTS, camera);
    let mesh_count = view.objects().filter_map(|object| object.mesh().map(|(_, mesh)| mesh.id())).collect::<HashSet<_>>().len();
    let terrain_count = view.surfaces().filter(|surface| matches!(surface.kind, render::scene::SurfaceKind::Terrain(_))).count();
    let object_faces = view.objects().filter_map(|object| object.mesh().map(|(_, mesh)| mesh.mesh().faces().len())).sum::<usize>();
    let terrain_faces = view
        .surfaces()
        .filter_map(|surface| match &surface.mesh {
            render::scene::PreparedMesh::Ready(mesh) => Some(mesh.mesh().faces().len()),
            _ => None,
        })
        .sum::<usize>();
    let face_count = object_faces + terrain_faces;
    assert_eq!(view.object_count(), OBJECTS);
    assert_eq!(mesh_count, meshes);
    assert!(terrain_count > 0);
    assert!(face_count >= 18_000, "benchmark scene should contain about 20,000 textured faces, found {face_count}");

    let mut prepare = GeometryPrepare::new(device, queue, TextureFormat::Rgba8UnormSrgb);
    for _ in 0..meshes.div_ceil(16) + 8 {
        prepare_sample(&mut prepare, device, queue, &view, camera);
        while let Some(job) = prepare.next_texture_job() {
            prepare.complete_texture(job.run(render::CompressionQuality::Fast));
        }
        if prepare.stats().pending_meshes == 0 && prepare.stats().pending_textures == 0 {
            break;
        }
    }
    assert_eq!(prepare.stats().pending_meshes, 0);
    assert_eq!(prepare.stats().pending_textures, 0);
    assert!(prepare.stats().texture_count > 0);
    assert_eq!(prepare.stats().instances, OBJECTS + terrain_count);
    assert_eq!(prepare.stats().omitted_faces, 0, "busy-region prepare must draw every supported face");
    eprintln!(
        "prepare scene: objects={OBJECTS}, meshes={mesh_count}, faces={face_count}, terrain_patches={terrain_count}, draw_submissions={}, indexed_commands={}, omitted_faces={}",
        prepare.stats().draw_calls,
        prepare.stats().draw_commands,
        prepare.stats().omitted_faces
    );

    let workload = format!("3000_{meshes}_36_terrain");
    let unchanged = format!("unchanged_static_camera/{workload}");
    group.bench_function(unchanged, |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iters {
                elapsed += prepare_sample(&mut prepare, device, queue, &view, camera);
            }
            elapsed
        });
    });
    let camera_a = camera_at(49.0);
    let camera_b = camera_at(51.0);
    prepare_sample(&mut prepare, device, queue, &view, camera);
    let mut moved = false;
    let camera_only = format!("camera_only/{workload}");
    group.bench_function(camera_only, |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iters {
                moved = !moved;
                elapsed += prepare_sample(&mut prepare, device, queue, &view, if moved { camera_a } else { camera_b });
            }
            elapsed
        });
    });
    prepare_sample(&mut prepare, device, queue, &view, camera);
    let mut order = 10_000_000;
    let mut generation = 1;
    let changed = &rows[0];
    let one_changed = format!("one_object_changed/{workload}");
    group.bench_function(one_changed, |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iters {
                support::update_one(&mut world, changed, &mut order, generation);
                generation += 1;
                support::first_publish(&mut reconciler, &mut view, &world, &mut now);
                elapsed += prepare_sample(&mut prepare, device, queue, &view, camera);
            }
            elapsed
        });
    });
    black_box((mesh_count, terrain_count));
}

fn prepare_sample(
    prepare: &mut GeometryPrepare,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    view: &render::scene::SceneView,
    camera: Camera,
) -> Duration {
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

fn camera_at(x: f64) -> Camera {
    Camera::new(DVec3::new(x, 15.0, 120.0), -Vec3::Z, std::f64::consts::FRAC_PI_2, SIZE)
        .expect("benchmark camera")
        .with_quality(512.0, 1.0)
        .expect("benchmark camera quality")
}
