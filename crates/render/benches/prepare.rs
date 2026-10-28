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
const DISTINCT_MESHES: [usize; 1] = [50];
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
    let (device, queue) = block_on(adapter.request_device(&DeviceDescriptor {
        required_features: adapter.features() & (wgpu::Features::TEXTURE_COMPRESSION_BC | wgpu::Features::INDIRECT_FIRST_INSTANCE),
        ..Default::default()
    }))
    .expect("benchmark GPU device");
    let mut group = c.benchmark_group("render/geometry_prepare");
    for meshes in DISTINCT_MESHES {
        bench_scene(&mut group, &device, &queue, meshes);
    }
    group.finish();
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
    elapsed
}

fn camera_at(x: f64) -> Camera {
    Camera::new(DVec3::new(x, 15.0, 120.0), -Vec3::Z, std::f64::consts::FRAC_PI_2, SIZE)
        .expect("benchmark camera")
        .with_quality(512.0, 1.0)
        .expect("benchmark camera quality")
}
