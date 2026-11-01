use std::{
    assert_matches,
    sync::{Arc, mpsc},
    time::Instant,
};

use assets::{Budget as AssetBudget, LoadJob, Priority, Request, Store};
use futures::executor::block_on;
use math::{DVec3, Vec3};
use utils::AssetId;
use uuid::Uuid;
use wgpu::util::{BufferInitDescriptor, DeviceExt};
use wgpu::{
    AddressMode, Backends, BindGroupDescriptor, BindGroupEntry, BindingResource, BufferDescriptor, BufferUsages, Color, DeviceDescriptor,
    ErrorFilter, Extent3d, FilterMode, InstanceDescriptor, LoadOp, MapMode, MipmapFilterMode, Operations, Origin3d, PollType, RenderPass,
    RenderPassColorAttachment, RenderPassDepthStencilAttachment, RenderPassDescriptor, SamplerDescriptor, StoreOp, TexelCopyBufferInfo,
    TexelCopyBufferLayout, TexelCopyTextureInfo, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};

use super::*;
use crate::geometry::{
    fixtures::{scene, scene_with_agent, scene_with_mapping, scene_with_scales, scene_with_world},
    instance::{instance_base, packed_instance},
};
use crate::material::FaceParams;
use crate::pipeline::{INSTANCE_BYTES, Pipeline, VERTEX_BYTES};
use crate::scene::{
    AgentPose, AssetChange, AssetRequest, Camera, DrawnMeshKey, Placement, ReadQuery, ReadReply, ReconcileStep, Reconciler, SceneView,
};
use world::{
    Appearance, FieldUpdate, Kind, ObjectObservation, OrderEvidence, ParentUpdate, Pose, Region, SculptFlags, Shape, SurfaceAppearance,
    UpdateSource, WorldPose,
};

fn check_pages(device: &wgpu::Device, queue: &wgpu::Queue, alpha: u8, separate_pages: bool) {
    let scope = device.push_error_scope(ErrorFilter::Validation);
    let (_scene, view, camera) = scene(alpha);
    let mut geometry = GeometryRenderer::new(device, queue, TextureFormat::Rgba8UnormSrgb);
    let filler = separate_pages.then(|| {
        let (_, shared) = view.objects().next().unwrap().mesh().unwrap();
        assert!(geometry.upload_mesh(device, shared, None, &mut 0));
        let allocation = &geometry.meshes[&shared.id()].allocation;
        let vertices = allocation.buffers().vertices.size() as usize / VERTEX_BYTES - allocation.vertices().end as usize;
        let indices = allocation.buffers().indices.size() as usize / size_of::<u32>() - allocation.indices().end as usize;
        geometry.storage.allocate(device, vertices, indices, false, &geometry.memory).unwrap()
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    let depth = geometry.depth(device, [128, 128]).unwrap();
    geometry.prepare(device, &mut encoder, &view, None, camera, [128, 128], None);
    let mut meshes: Vec<_> = geometry.meshes.values().collect();
    meshes.sort_by_key(|mesh| mesh.allocation.vertices().start);
    assert_eq!(meshes.len(), 2);
    assert_eq!(Arc::ptr_eq(meshes[0].allocation.buffers(), meshes[1].allocation.buffers()), !separate_pages);
    if !separate_pages {
        assert_eq!(meshes[0].allocation.vertices().end, meshes[1].allocation.vertices().start);
        assert_eq!(meshes[0].allocation.indices().end, meshes[1].allocation.indices().start);
    }
    if alpha == 255 {
        let grouped_calls = if separate_pages { 2 } else { 1 };
        let expected_calls = if geometry.indirect_first_instance { grouped_calls } else { geometry.stats.draw_commands };
        assert_eq!(geometry.stats.draw_calls, expected_calls, "draw submissions match the available indirect features");
        geometry.draw_list.build_draw_batches(&geometry.meshes, geometry.indirect_first_instance);
        assert_eq!(geometry.draw_list.draw_batches().len(), grouped_calls, "rebuilding retained indirect groups keeps submissions");
    }
    let mut objects: Vec<_> = view.objects().collect();
    objects.sort_by_key(|object| object.detail().summary.id);
    if alpha != 255 {
        objects.reverse();
    }
    let reference: Vec<_> = objects
        .into_iter()
        .map(|object| {
            let (_, shared) = object.mesh().unwrap();
            let mesh = shared.mesh();
            let mut vertices = Vec::new();
            for vertex in mesh.vertices() {
                floats(&mut vertices, vertex.position.to_array());
                floats(&mut vertices, vertex.normal.to_array());
                floats(&mut vertices, vertex.uv.to_array());
            }
            let indices: Vec<_> = mesh.indices().iter().flat_map(|index| index.to_le_bytes()).collect();
            let Placement::Ready(transform) = object.placement() else { panic!("fixture object must be placed") };
            let params = FaceParams::new(Some(&object.materials().unwrap()[0]));
            let packed = packed_instance(
                &instance_base(transform.pose.position, transform.pose.rotation, transform.scale, false, camera.position()),
                &params,
                transform.scale,
            );
            (
                device.create_buffer_init(&BufferInitDescriptor { label: None, contents: &vertices, usage: BufferUsages::VERTEX }),
                device.create_buffer_init(&BufferInitDescriptor { label: None, contents: &indices, usage: BufferUsages::INDEX }),
                device.create_buffer_init(&BufferInitDescriptor { label: None, contents: &packed, usage: BufferUsages::VERTEX }),
                mesh.indices().len() as u32,
            )
        })
        .collect();
    let color = device.create_texture(&TextureDescriptor {
        label: None,
        size: Extent3d { width: 128, height: 128, depth_or_array_layers: 2 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8UnormSrgb,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    for layer in 0..2 {
        let color_view = color.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_array_layer: layer,
            array_layer_count: Some(1),
            ..Default::default()
        });
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: LoadOp::Clear(Color::BLACK), store: StoreOp::Store },
            })],
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(Operations { load: LoadOp::Clear(0.0), store: StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        if layer == 0 {
            geometry.draw(&mut pass);
        } else {
            geometry.pipelines.bind_pipeline(&mut pass, if alpha == 255 { Pipeline::Surface } else { Pipeline::Blend });
            pass.set_bind_group(1, geometry.textures.binding(None), &[]);
            for (vertices, indices, instance, count) in &reference {
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_vertex_buffer(1, instance.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..*count, 0, 0..1);
            }
        }
    }
    let readback = device.create_buffer(&BufferDescriptor {
        label: None,
        size: 2 * 128 * 128 * 4,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for layer in 0..2 {
        encoder.copy_texture_to_buffer(
            TexelCopyTextureInfo { origin: Origin3d { x: 0, y: 0, z: layer }, ..color.as_image_copy() },
            TexelCopyBufferInfo {
                buffer: &readback,
                layout: TexelCopyBufferLayout {
                    offset: u64::from(layer) * 128 * 128 * 4,
                    bytes_per_row: Some(512),
                    rows_per_image: Some(128),
                },
            },
            Extent3d { width: 128, height: 128, depth_or_array_layers: 1 },
        );
    }
    let frame = geometry.admit().unwrap();
    queue.submit([encoder.finish()]);
    geometry.submitted(frame);
    let (mapped, complete) = mpsc::channel();
    readback.slice(..).map_async(MapMode::Read, move |result| mapped.send(result).unwrap());
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    complete.recv().unwrap().unwrap();
    let pixels = readback.get_mapped_range(..).unwrap();
    let (shared_pixels, reference_pixels) = pixels.split_at(128 * 128 * 4);
    let mismatched_pixels = shared_pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(reference_pixels.as_chunks::<4>().0)
        .filter(|(shared, reference)| shared != reference)
        .count();
    assert_eq!(mismatched_pixels, 0, "batched pages must match independently drawn complete source meshes");
    let mut red = 0;
    let mut green = 0;
    for pixel in shared_pixels.as_chunks::<4>().0 {
        red += usize::from(pixel[0] > pixel[1].saturating_add(32));
        green += usize::from(pixel[1] > pixel[0].saturating_add(32));
    }
    assert!(red > 100 && green > 100, "both meshes retain distinct geometry and material colors: {red} red, {green} green pixels");
    drop(pixels);
    readback.unmap();
    let (id, old) = geometry
        .meshes
        .iter()
        .filter(|(_, mesh)| filler.as_ref().is_none_or(|filler| !Arc::ptr_eq(mesh.allocation.buffers(), filler.buffers())))
        .min_by_key(|(_, mesh)| mesh.allocation.vertices().start)
        .unwrap();
    let id = *id;
    let old_vertices = old.allocation.vertices();
    let old_indices = old.allocation.indices();
    let old_buffers = old.allocation.buffers().clone();
    let charged = geometry.memory.mesh_state().bytes;
    geometry.retired.push(geometry.meshes.remove(&id).unwrap());
    let frame = geometry.admit().unwrap();
    queue.submit([]);
    geometry.submitted(frame);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    assert_eq!(geometry.memory.mesh_state().bytes, charged);
    assert!(geometry.needs_frame(Instant::now()), "retirement must unblock preparation without freeing a whole page");
    let reused = geometry.storage.allocate(device, old_vertices.len(), old_indices.len(), false, &geometry.memory).unwrap();
    assert_eq!((reused.vertices(), reused.indices()), (old_vertices, old_indices));
    if separate_pages {
        assert!(!Arc::ptr_eq(reused.buffers(), &old_buffers), "empty pages are released before a new page allocation");
    } else {
        assert!(Arc::ptr_eq(reused.buffers(), &old_buffers), "ranges on a page with live allocations are reused");
    }
    drop(reused);
    drop(old_buffers);
    drop(filler);
    assert!(block_on(scope.pop()).is_none(), "shared ranges and draw offsets must satisfy GPU validation");
    let memory = geometry.memory.clone();
    drop(geometry);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    assert_eq!(memory.mesh_state().bytes, 0, "renderer retirement releases page accounting after GPU completion");
}

fn mesh_page_bytes(shared: &crate::scene::SharedMesh) -> usize {
    let mesh = shared.mesh();
    let bytes = mesh.vertices().len() * VERTEX_BYTES + std::mem::size_of_val(mesh.indices());
    bytes * (1024 * 1024 / bytes).max(1)
}

fn settle_scene(world: &world::World, scene: &mut Reconciler, view: &mut SceneView, mut now: Instant) -> Vec<AssetRequest> {
    let mut requests = Vec::new();
    loop {
        match scene.poll(now) {
            ReconcileStep::Asset(AssetChange::Request(request)) => requests.push(request),
            ReconcileStep::Asset(AssetChange::Release(_)) => {}
            ReconcileStep::Read(job) => {
                let result = match job.query() {
                    ReadQuery::Barrier => Ok(ReadReply::Barrier(world.read_barrier())),
                    ReadQuery::Objects(cursor) => world.object_list_page(*cursor, 256).map(ReadReply::Objects),
                    ReadQuery::Regions(cursor) => world.region_page(*cursor, 64).map(ReadReply::Regions),
                    ReadQuery::Details(entities) => world.object_detail_batch(entities).map(ReadReply::Details),
                    ReadQuery::Changes(cursor) => world.change_page(*cursor, 256).map(ReadReply::Changes),
                    ReadQuery::Terrain(region, cursor) => world.terrain_page(*region, *cursor, 8).map(ReadReply::Terrain),
                    ReadQuery::Environment(region) => Ok(ReadReply::Environment(world.environment(*region))),
                };
                scene.complete_read(job, result.map_err(crate::scene::ReadFailure::World), now);
            }
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Publish(batch) => {
                view.apply(batch).unwrap();
            }
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            ReconcileStep::Idle => return requests,
            _ => panic!("fixture stopped before publishing geometry"),
        }
    }
}

fn move_object(world: &mut world::World, id: u32, order: u64, position: Vec3, color: [u8; 4]) {
    let mut observation = ObjectObservation::new(
        UpdateSource::new(Region::new(1), u32::try_from(order).expect("fixture sequence"), OrderEvidence::Ordered(order), 0),
        uuid::Uuid::from_u128(u128::from(id)),
        std::num::NonZeroU32::new(id).expect("fixture local ID"),
        Kind::Object,
        ParentUpdate::Root,
    );
    observation.pose = Some(Pose { position: Some(position), rotation: Some(math::Quat::IDENTITY) });
    observation.appearance = FieldUpdate::Set(Appearance::Surface);
    observation.surface_appearance =
        Some(SurfaceAppearance::new(world::FaceAppearance { color, ..Default::default() }, Vec::new()).expect("fixture appearance"));
    assert_matches!(world.observe_object(observation).unwrap().outcome, world::ApplyOutcome::Updated(_));
}

fn mesh_scene(
    objects: &[(u32, AssetId, Vec3)],
    textures: Option<(AssetId, AssetId)>,
) -> (world::World, Reconciler, SceneView, Camera, Vec<AssetRequest>) {
    let count = objects.len();
    let mut world = world::World::new(world::WorldLimits {
        max_region_registrations: 1,
        max_entities: count,
        max_entities_per_region: count,
        max_tombstones_per_region: 1,
        max_tombstones: 1,
        max_tombstone_bytes_per_region: world::WorldLimits::tombstone_bytes_for_count(1),
        max_tombstone_bytes: world::WorldLimits::tombstone_bytes_for_count(1),
        max_journal_entries: 16,
        max_journal_changes: 64,
        max_region_name_bytes: 64,
        max_recovery_targets: count,
    });
    let region = Region::new(1);
    let _adopted = world.adopt_region(region).unwrap();
    let _metadata = world.patch_region(region, world::RegionInfo { origin: Some(DVec3::ZERO), ..Default::default() }).unwrap();
    for (id, asset, position) in objects {
        let mut observation = ObjectObservation::new(
            UpdateSource::new(region, *id, OrderEvidence::Ordered(u64::from(*id)), 0),
            Uuid::from_u128(100_000 + u128::from(*id)),
            std::num::NonZeroU32::new(*id).unwrap(),
            Kind::Object,
            ParentUpdate::Root,
        );
        observation.pose = Some(Pose { position: Some(*position), rotation: Some(math::Quat::IDENTITY) });
        observation.scale = Some(Vec3::ONE);
        observation.shape = FieldUpdate::Set(Shape::Mesh { asset_id: *asset, flags: SculptFlags::default() });
        if *id == 1
            && let Some((red, green)) = textures
        {
            observation.appearance = FieldUpdate::Set(Appearance::Surface);
            observation.surface_appearance = Some(
                SurfaceAppearance::new(
                    world::FaceAppearance { texture: Some(red), color: [255, 0, 0, 255], ..Default::default() },
                    vec![(1, world::FaceAppearance { texture: Some(green), color: [0, 255, 0, 128], ..Default::default() })],
                )
                .unwrap(),
            );
        }
        let _applied = world.observe_object(observation).unwrap();
    }
    let camera = Camera::new(DVec3::ZERO, Vec3::X, std::f64::consts::FRAC_PI_3, [128, 128]).unwrap();
    let (mut scene, mut view) = Reconciler::new(crate::scene::Budget::default());
    scene.set_camera(camera);
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    (world, scene, view, camera, requests)
}

fn avatar_mesh_scene() -> (world::World, Reconciler, SceneView, Camera) {
    let mut world = world::World::new(world::WorldLimits {
        max_region_registrations: 1,
        max_entities: 1,
        max_entities_per_region: 1,
        max_tombstones_per_region: 1,
        max_tombstones: 1,
        max_tombstone_bytes_per_region: world::WorldLimits::tombstone_bytes_for_count(1),
        max_tombstone_bytes: world::WorldLimits::tombstone_bytes_for_count(1),
        max_journal_entries: 16,
        max_journal_changes: 64,
        max_region_name_bytes: 64,
        max_recovery_targets: 1,
    });
    let region = Region::new(1);
    let _ = world.adopt_region(region).expect("avatar region");
    let _ =
        world.patch_region(region, world::RegionInfo { origin: Some(DVec3::ZERO), ..Default::default() }).expect("avatar region origin");
    let mut observation = ObjectObservation::new(
        UpdateSource::new(region, 1, OrderEvidence::Ordered(1), 0),
        Uuid::from_u128(1),
        std::num::NonZeroU32::new(1).expect("avatar local id"),
        Kind::Avatar,
        ParentUpdate::Root,
    );
    observation.scale = Some(Vec3::ONE);
    observation.shape = FieldUpdate::Set(Shape::Prim(world::PrimShape::default()));
    observation.appearance = FieldUpdate::Set(Appearance::Surface);
    observation.surface_appearance = Some(SurfaceAppearance::new(world::FaceAppearance::default(), Vec::new()).expect("avatar appearance"));
    let _ = world.observe_object(observation).expect("avatar introduction");
    let (mut scene, mut view) = Reconciler::new(crate::scene::Budget::default());
    let camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [128, 128]).expect("avatar camera");
    scene.set_camera(camera);
    settle_scene(&world, &mut scene, &mut view, Instant::now());
    (world, scene, view, camera)
}

fn prepared_asset(request: Request, bytes: Vec<u8>) -> assets::Status {
    let now = Instant::now();
    let mut store = Store::new(AssetBudget::default());
    let route = store.register_route().unwrap();
    let demand = store.request(route, request, Priority::Visible, now).unwrap();
    let LoadJob::Fetch(job) = store.next_job(now).unwrap() else { panic!("fixture request unexpectedly used a cached source") };
    let input = job.complete(Ok(bytes)).unwrap();
    let completion = store.start_decode(input).unwrap().run();
    store.complete(completion, now);
    demand.status()
}

fn vulkan_device() -> (wgpu::Device, wgpu::Queue) {
    block_on(async {
        let instance =
            wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        adapter.request_device(&DeviceDescriptor::default()).await.unwrap()
    })
}

fn prepare_frame(device: &wgpu::Device, queue: &wgpu::Queue, geometry: &mut GeometryRenderer, view: &SceneView, camera: Camera) {
    let frame = geometry.admit().unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(device, &mut encoder, view, None, camera, [128, 128], None);
    queue.submit([encoder.finish()]);
    geometry.submitted(frame);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
}

fn prepare_agent_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    geometry: &mut GeometryRenderer,
    view: &SceneView,
    camera: Camera,
    agent_pose: Option<AgentPose>,
) {
    let frame = geometry.admit().expect("geometry frame lease");
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(device, &mut encoder, view, agent_pose, camera, [128, 128], None);
    queue.submit([encoder.finish()]);
    geometry.submitted(frame);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).expect("submitted frame completes");
}

fn fill_mesh_pages(device: &wgpu::Device, geometry: &mut GeometryRenderer) -> Vec<crate::geometry::storage::Allocation> {
    let mut pages: Vec<(Arc<crate::geometry::storage::Buffers>, u32, u32)> = Vec::new();
    for mesh in geometry.meshes.values() {
        let buffers = mesh.allocation.buffers().clone();
        if let Some((_, vertices, indices)) = pages.iter_mut().find(|(page, _, _)| Arc::ptr_eq(page, &buffers)) {
            *vertices = (*vertices).max(mesh.allocation.vertices().end);
            *indices = (*indices).max(mesh.allocation.indices().end);
        } else {
            pages.push((buffers, mesh.allocation.vertices().end, mesh.allocation.indices().end));
        }
    }
    let mut tails: Vec<_> = pages
        .iter()
        .map(|(buffers, vertices, indices)| {
            let vertex_capacity = u32::try_from(buffers.vertices.size() / VERTEX_BYTES as u64).expect("fixture page vertices");
            let index_capacity = u32::try_from(buffers.indices.size() / size_of::<u32>() as u64).expect("fixture page indices");
            (buffers.clone(), vertex_capacity - *vertices, index_capacity - *indices)
        })
        .collect();
    tails.sort_unstable_by_key(|(_, vertices, indices)| std::cmp::Reverse(vertices.saturating_add(*indices)));
    let mut allocations = Vec::new();
    for (buffers, vertices, indices) in tails {
        if vertices == 0 || indices == 0 {
            continue;
        }
        if let Some(allocation) = geometry.storage.allocate(device, vertices as usize, indices as usize, false, &geometry.memory) {
            assert!(Arc::ptr_eq(allocation.buffers(), &buffers), "the page tail reservation matches its page");
            allocations.push(allocation);
        }
    }
    allocations
}

fn render_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: [u32; 2],
    depth: &wgpu::TextureView,
    draw: impl FnOnce(&mut RenderPass<'_>),
) -> Vec<[u8; 4]> {
    let color = device.create_texture(&TextureDescriptor {
        label: None,
        size: Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8UnormSrgb,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: LoadOp::Clear(Color::BLACK), store: StoreOp::Store },
            })],
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(Operations { load: LoadOp::Clear(0.0), store: StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        draw(&mut pass);
    }
    let bytes_per_row = size[0] * 4;
    let readback = device.create_buffer(&BufferDescriptor {
        label: None,
        size: u64::from(bytes_per_row) * u64::from(size[1]),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo { origin: Origin3d::ZERO, ..color.as_image_copy() },
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(size[1]) },
        },
        Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    let (mapped, complete) = mpsc::channel();
    readback.slice(..).map_async(MapMode::Read, move |result| mapped.send(result).unwrap());
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    complete.recv().unwrap().unwrap();
    let pixels = readback.get_mapped_range(..).unwrap().as_chunks::<4>().0.to_vec();
    readback.unmap();
    pixels
}

fn check_mirrored_render(device: &wgpu::Device, queue: &wgpu::Queue) {
    let scope = device.push_error_scope(ErrorFilter::Validation);
    let (_scene, view, camera) = scene_with_scales(255, [Vec3::splat(2.0), Vec3::new(-2.0, 2.0, 2.0)]);
    let size = [256, 256];
    let mut geometry = GeometryRenderer::new(device, queue, TextureFormat::Rgba8UnormSrgb);
    let mut prepare = device.create_command_encoder(&Default::default());
    let depth = geometry.depth(device, size).unwrap();
    geometry.prepare(device, &mut prepare, &view, None, camera, size, None);
    queue.submit([prepare.finish()]);
    assert_eq!(geometry.stats.instances, 2, "the public scene view submits both visible prims");
    let pixels = render_pixels(device, queue, size, &depth, |pass| geometry.draw(pass));
    let mut red = Vec::new();
    let mut green = Vec::new();
    for pixel in pixels {
        if pixel[0] > pixel[1].saturating_add(32) {
            red.push(pixel[0]);
        }
        if pixel[1] > pixel[0].saturating_add(32) {
            green.push(pixel[1]);
        }
    }
    assert!(red.len() > 100, "unmirrored prim remains visible after back-face culling: {} pixels", red.len());
    assert!(green.len() > 100, "mirrored prim remains visible after back-face culling: {} pixels", green.len());
    let average = |values: &[u8]| values.iter().map(|value| u32::from(*value)).sum::<u32>() / values.len() as u32;
    assert!(average(&red).abs_diff(average(&green)) < 30, "mirroring keeps front-face normals outward for studio lighting");
    drop(geometry);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    assert!(block_on(scope.pop()).is_none(), "production scene preparation and mirrored draw pass satisfy GPU validation");
}

fn check_planar_projection(device: &wgpu::Device, queue: &wgpu::Queue) {
    let scope = device.push_error_scope(ErrorFilter::Validation);
    let (_scene, view, camera) = scene_with_mapping(255, [Vec3::splat(2.0), Vec3::new(2.0, -2.0, 2.0)], world::TextureMapping::Planar);
    let size = [256, 256];
    let mut geometry = GeometryRenderer::new(device, queue, TextureFormat::Rgba8UnormSrgb);
    let depth = geometry.depth(device, size).unwrap();
    let projection = geometry.pipelines.update_camera_uniform(queue, camera, size, camera.position());
    let mut objects: Vec<_> = view.objects().collect();
    objects.sort_by_key(|object| object.detail().summary.id);
    let mut instances = Vec::new();
    for object in &objects {
        let Placement::Ready(transform) = object.placement() else { panic!("fixture prim must be placed") };
        let face = object.materials().unwrap().iter().find(|material| material.slot == 0).expect("fixture has face zero");
        let mut params = FaceParams::new(Some(face));
        assert_eq!(params.params[3], 1.0, "scene planar mapping reaches the production surface parameters");
        params.color = [1.0; 4];
        params.params[0] = 1.0;
        instances.extend_from_slice(&packed_instance(
            &instance_base(transform.pose.position, transform.pose.rotation, transform.scale, false, camera.position()),
            &params,
            transform.scale,
        ));
    }
    let vertices: Vec<u8> = [Vec3::new(0.0, -0.5, -0.5), Vec3::new(0.0, 0.5, -0.5), Vec3::new(0.0, 0.5, 0.5), Vec3::new(0.0, -0.5, 0.5)]
        .into_iter()
        .flat_map(|position| position.to_array().into_iter().chain([-1.0, 0.0, 0.0]).chain([0.0, 0.0]).flat_map(f32::to_le_bytes))
        .collect();
    let indices = [0_u32, 2, 1, 0, 3, 2];
    let index_bytes: Vec<_> = indices.into_iter().flat_map(u32::to_le_bytes).collect();
    let vertex_buffer = device.create_buffer_init(&BufferInitDescriptor { label: None, contents: &vertices, usage: BufferUsages::VERTEX });
    let index_buffer = device.create_buffer_init(&BufferInitDescriptor { label: None, contents: &index_bytes, usage: BufferUsages::INDEX });
    let instance_buffer =
        device.create_buffer_init(&BufferInitDescriptor { label: None, contents: &instances, usage: BufferUsages::VERTEX });
    let texture_pixels: Vec<_> =
        (0..8).flat_map(|_| (0..8).flat_map(|x| if x < 4 { [255, 0, 0, 255] } else { [0, 0, 255, 255] })).collect();
    let texture = device.create_texture(&TextureDescriptor {
        label: None,
        size: Extent3d { width: 8, height: 8, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8UnormSrgb,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        &texture_pixels,
        TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(32), rows_per_image: Some(8) },
        Extent3d { width: 8, height: 8, depth_or_array_layers: 1 },
    );
    let texture_view = texture.create_view(&Default::default());
    let sampler = device.create_sampler(&SamplerDescriptor {
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        mag_filter: FilterMode::Nearest,
        min_filter: FilterMode::Nearest,
        mipmap_filter: MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let texture_binding = device.create_bind_group(&BindGroupDescriptor {
        label: None,
        layout: &geometry.textures.layout,
        entries: &[
            BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&texture_view) },
            BindGroupEntry { binding: 1, resource: BindingResource::Sampler(&sampler) },
        ],
    });
    let pixels = render_pixels(device, queue, size, &depth, |pass| {
        geometry.pipelines.bind_pipeline(pass, Pipeline::DoubleSided);
        pass.set_bind_group(1, &texture_binding, &[]);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        for index in 0..objects.len() {
            let start = index * INSTANCE_BYTES;
            pass.set_vertex_buffer(1, instance_buffer.slice(start as u64..(start + INSTANCE_BYTES) as u64));
            pass.draw_indexed(0..indices.len() as u32, 0, 0..1);
        }
    });
    for object in objects {
        let Placement::Ready(transform) = object.placement() else { panic!("fixture prim must be placed") };
        for (delta, red_expected) in [(0.2, true), (-0.2, false)] {
            let point = (transform.pose.position + DVec3::new(0.0, delta, 0.0)).as_vec3();
            let clip = projection.project_point3(point);
            let x = ((clip.x * 0.5 + 0.5) * size[0] as f32).floor() as usize;
            let y = ((1.0 - (clip.y * 0.5 + 0.5)) * size[1] as f32).floor() as usize;
            let pixel = pixels[y * size[0] as usize + x];
            assert_eq!(pixel[0] > pixel[2].saturating_add(32), red_expected, "planar texels follow signed object scale after projection");
        }
    }
    drop(geometry);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    assert!(block_on(scope.pop()).is_none(), "planar projection through the surface shader satisfies GPU validation");
}

fn assert_visible_draw_coverage(geometry: &GeometryRenderer) {
    let visible: std::collections::HashSet<_> = geometry.cache.visible.iter().copied().collect();
    let mut face_slots = std::collections::HashSet::new();
    for (index, item) in geometry.cache.items.iter() {
        for face in &item.faces {
            match (visible.contains(&index), face.slot) {
                (true, Some(slot)) => assert!(face_slots.insert(slot), "each visible face owns one instance slot"),
                (false, None) | (true, None) => {}
                (false, Some(_)) => panic!("hidden items cannot retain instance slots"),
            }
        }
    }
    let draw_slots: std::collections::HashSet<_> =
        geometry.draw_list.draws().iter().flat_map(|draw| draw.first..draw.first.saturating_add(draw.count)).collect();
    assert_eq!(draw_slots, face_slots, "draw commands cover every packed visible face and no hidden face");
}

#[test]
#[ignore = "requires a Vulkan adapter; run explicitly with cargo nextest run -p render --run-ignored only -E 'test(gpu_retired_hidden_page_releases_for_visible_mesh)'"]
fn gpu_retired_hidden_page_releases_for_visible_mesh() {
    let (device, queue) = block_on(async {
        let instance =
            wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        adapter.request_device(&DeviceDescriptor::default()).await.unwrap()
    });
    let (_scene, view, _) = scene(128);
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    let retired_camera = Camera::new(DVec3::new(13.0, 0.0, 0.0), Vec3::X, 1.0, [128, 128]).unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(&device, &mut encoder, &view, None, retired_camera, [128, 128], None);
    assert_eq!(geometry.stats.instances, 1);
    assert_eq!(geometry.meshes.len(), 1);
    assert!(!geometry.draw_list.draw_batches().is_empty());

    let retired_id = *geometry.meshes.keys().next().expect("initial visible mesh");
    let retired_object = view
        .objects()
        .find(|object| object.mesh().is_some_and(|(_, mesh)| mesh.id() == retired_id))
        .expect("retired mesh object remains in the scene");
    let retired_entity = retired_object.detail().summary.entity;
    let target_object =
        view.objects().find(|object| object.mesh().is_some_and(|(_, mesh)| mesh.id() != retired_id)).expect("another cached mesh");
    let (_, target_mesh) = target_object.mesh().expect("target mesh");
    let target_id = target_mesh.id();
    let target_entity = target_object.detail().summary.entity;
    let retired_index = geometry.cache.objects[&retired_entity];
    let target_index = geometry.cache.objects[&target_entity];
    let old_page_bytes = geometry.memory.mesh_state().bytes;
    let target_page_bytes = mesh_page_bytes(target_mesh);
    let headroom = target_page_bytes.saturating_sub(old_page_bytes).saturating_add(1);
    let blocker_bytes = crate::geometry::MESH_BYTES
        .checked_sub(old_page_bytes.checked_add(headroom).expect("fixture page budget"))
        .expect("fixture meshes fit the renderer budget");
    let blocker = geometry.memory.reserve(blocker_bytes, crate::geometry::Resource::Mesh).expect("fill the unused mesh budget");

    drop(geometry.meshes.remove(&retired_id).expect("retired GPU mesh"));
    geometry.cache.items.get_mut(retired_index).expect("cached retired item").position = DVec3::new(-100.0, 0.0, 0.0);
    geometry.cache.items.get_mut(target_index).expect("cached target item").position = DVec3::new(14.0, 0.0, 0.0);
    geometry.cache.camera = None;
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(&device, &mut encoder, &view, None, retired_camera, [128, 128], None);
    if !geometry.meshes.contains_key(&target_id) {
        assert!(geometry.needs_frame(Instant::now()), "a freed mesh page must schedule another preparation");
        let mut encoder = device.create_command_encoder(&Default::default());
        geometry.prepare(&device, &mut encoder, &view, None, retired_camera, [128, 128], None);
    }

    assert!(geometry.meshes.contains_key(&target_id), "the newly visible mesh uploads after the hidden page is released");
    assert!(geometry.pending_meshes.is_empty());
    assert!(geometry.cache.items.get(retired_index).is_some(), "hidden face groups remain cached");
    assert!(!geometry.cache.candidates.contains(&retired_index));
    assert!(!geometry.cache.visible.contains(&retired_index));
    assert!(geometry.cache.candidates.contains(&target_index));
    assert!(geometry.cache.visible.contains(&target_index));

    let memory = geometry.memory.clone();
    drop(blocker);
    drop(geometry);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    assert_eq!(memory.mesh_state().bytes, 0);
}

#[test]
#[ignore = "requires a Vulkan adapter; run explicitly with cargo nextest run -p render --run-ignored only -E 'test(gpu_appearance_replacement_rechecks_visibility)'"]
fn gpu_appearance_replacement_rechecks_visibility() {
    let (device, queue) = block_on(async {
        let instance =
            wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        adapter.request_device(&DeviceDescriptor::default()).await.unwrap()
    });
    let (mut world, mut scene, mut view, camera) = scene_with_world(255, [Vec3::splat(2.0); 2], Default::default());
    let entity = view
        .objects()
        .find(|object| object.detail().summary.id == uuid::Uuid::from_u128(1))
        .expect("fixture object")
        .detail()
        .summary
        .entity;
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(&device, &mut encoder, &view, None, camera, [128, 128], None);
    let index = geometry.cache.objects[&entity];
    let item_id = geometry.cache.items.get(index).expect("cached fixture object").id;
    assert!(geometry.cache.candidates.contains(&index));
    assert!(geometry.cache.visible.contains(&index));

    move_object(&mut world, 1, 100, Vec3::new(10.0, 100.0, 0.0), [0, 0, 255, 255]);
    scene.notify_world_change();
    settle_scene(&world, &mut scene, &mut view, Instant::now());
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(&device, &mut encoder, &view, None, camera, [128, 128], None);
    let index = geometry.cache.objects[&entity];
    assert_eq!(geometry.cache.items.get(index).map(|item| item.id), Some(item_id));
    assert!(!geometry.cache.candidates.contains(&index));
    assert!(!geometry.cache.visible.contains(&index));
    assert_visible_draw_coverage(&geometry);

    move_object(&mut world, 1, 200, Vec3::new(10.0, -2.0, 0.0), [255, 255, 0, 255]);
    scene.notify_world_change();
    settle_scene(&world, &mut scene, &mut view, Instant::now());
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(&device, &mut encoder, &view, None, camera, [128, 128], None);
    let index = geometry.cache.objects[&entity];
    assert_eq!(geometry.cache.items.get(index).map(|item| item.id), Some(item_id));
    assert!(geometry.cache.candidates.contains(&index));
    assert!(geometry.cache.visible.contains(&index));
    assert_visible_draw_coverage(&geometry);
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo test -p render gpu_same_mesh_appearance_rebuilds_after_eviction -- --ignored"]
fn gpu_same_mesh_appearance_rebuilds_after_eviction() {
    let (device, queue) = vulkan_device();
    let (mut world, mut scene, mut view, camera) = scene_with_world(255, [Vec3::splat(2.0); 2], Default::default());
    let entity =
        view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(1)).expect("fixture object").detail().summary.entity;
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    let index = geometry.cache.objects[&entity];
    let mesh_id = geometry.cache.items.get(index).expect("cached object").mesh.id();
    drop(geometry.meshes.remove(&mesh_id).expect("resident fixture mesh"));

    let color = [24, 96, 224, 255];
    move_object(&mut world, 1, 100, Vec3::new(10.0, -2.0, 0.0), color);
    scene.notify_world_change();
    settle_scene(&world, &mut scene, &mut view, Instant::now());
    prepare_frame(&device, &queue, &mut geometry, &view, camera);

    let item = geometry.cache.items.get(index).expect("rebuilt object");
    let object = view.object(entity).expect("updated scene object");
    let expected = FaceParams::new(Some(&object.materials().expect("updated material")[0]));
    assert_eq!(item.mesh.id(), mesh_id);
    assert!(item.replacement.is_none(), "same-mesh appearance updates rebuild immediately");
    assert!(geometry.meshes.contains_key(&mesh_id), "the evicted mesh is uploaded again");
    assert_eq!(item.faces.first().map(|face| face.base_params.color), Some(expected.color));
    assert_eq!(geometry.stats.pending_meshes, 0);
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo test -p render gpu_removed_pending_replacement_reconciles_snapshot -- --ignored"]
fn gpu_removed_pending_replacement_reconciles_snapshot() {
    let (device, queue) = vulkan_device();
    let (mut world, mut scene, mut view, camera) = scene_with_world(255, [Vec3::splat(2.0); 2], Default::default());
    let entity =
        view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(1)).expect("replacement target").detail().summary.entity;
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    let original_mesh = geometry.cache.items.get(geometry.cache.objects[&entity]).expect("original item").mesh.id();
    move_object(&mut world, 2, 10, Vec3::new(1000.0, 0.0, 0.0), [0, 255, 0, 255]);
    scene.notify_world_change();
    settle_scene(&world, &mut scene, &mut view, Instant::now());
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    let stale_version = geometry.cache.version.clone().expect("prepared view version");
    let fillers = fill_mesh_pages(&device, &mut geometry);
    assert!(!fillers.is_empty(), "fixture mesh pages have spare capacity to reserve");
    let blocker_bytes = crate::geometry::MESH_BYTES - geometry.memory.mesh_state().bytes;
    let blocker = geometry.memory.reserve(blocker_bytes, crate::geometry::Resource::Mesh).expect("fill mesh budget");

    let mut patch = world::ObjectPatch::new(
        UpdateSource::new(Region::new(1), 20, OrderEvidence::Ordered(20), 0),
        std::num::NonZeroU32::new(1).expect("target local id"),
    );
    patch.shape =
        FieldUpdate::Set(Shape::Prim(world::PrimShape { path_scale_x: 40, path_scale_y: 40, profile_hollow: 16, ..Default::default() }));
    let _ = world.patch_object(patch).expect("replacement shape applies");
    scene.notify_world_change();
    settle_scene(&world, &mut scene, &mut view, Instant::now());
    let replacement_mesh =
        view.object(entity).and_then(|object| object.mesh().map(|(_, mesh)| mesh.id())).expect("new mesh geometry published");
    assert_ne!(replacement_mesh, original_mesh);
    assert!(!geometry.meshes.contains_key(&replacement_mesh));
    assert_eq!(geometry.cache.items.get(geometry.cache.objects[&entity]).map(|item| item.mesh.id()), Some(original_mesh));
    let view_changes = view.changes_since(&stale_version).expect("same session changes").collect::<Vec<_>>();
    assert!(view_changes.iter().all(|change| !change.full), "the mesh update remains incremental");
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    let held = geometry.cache.items.get(geometry.cache.objects[&entity]).expect("held item");
    assert_eq!(held.mesh.id(), original_mesh);
    assert_eq!(held.replacement.as_ref().map(|replacement| replacement.item.mesh.id()), Some(replacement_mesh));
    assert_eq!(geometry.stats.instances, 1, "the old item remains drawn while allocation is blocked");

    let _ = world
        .remove_object(
            UpdateSource::new(Region::new(1), 30, OrderEvidence::Ordered(30), 0),
            std::num::NonZeroU32::new(1).expect("target local id"),
        )
        .expect("target removal applies");
    scene.notify_world_change();
    settle_scene(&world, &mut scene, &mut view, Instant::now());
    for order in 31..=95 {
        move_object(&mut world, 2, order, Vec3::new(1000.0, 0.0, 0.0), [order as u8, 96, 224, 255]);
        scene.notify_world_change();
        settle_scene(&world, &mut scene, &mut view, Instant::now());
    }
    let changes = view.changes_since(&stale_version).expect("same session changes").collect::<Vec<_>>();
    assert!(changes.iter().any(|change| change.reconcile_all), "evicted removal history requests snapshot reconciliation");
    assert!(changes.iter().flat_map(|change| &change.objects).all(|changed| *changed != entity), "the removal event aged out of history");
    assert_eq!(view.object_count(), 1, "the surviving scene snapshot contains the unrelated object");

    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    assert!(!geometry.cache.objects.contains_key(&entity));
    assert!(geometry.cache.objects.is_empty(), "the only remaining object is outside the view");
    assert!(geometry.cache.items.iter().all(|(_, item)| item.entity != Some(entity)));
    assert!(geometry.cache.items.iter().all(|(_, item)| item.replacement.is_none()));
    assert!(!geometry.cache.mesh_info.contains_key(&replacement_mesh));
    assert_eq!(geometry.stats.instances, 0);
    assert_visible_draw_coverage(&geometry);

    drop(blocker);
    drop(fillers);
    let memory = geometry.memory.clone();
    drop(geometry);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    assert_eq!(memory.mesh_state().bytes, 0);
}

#[test]
#[ignore = "requires a Vulkan adapter; run explicitly with cargo nextest run -p render --run-ignored ignored-only gpu_mesh_pages"]
fn gpu_mesh_pages() {
    let (device, queue) = block_on(async {
        let instance =
            wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        adapter
            .request_device(&DeviceDescriptor {
                required_features: adapter.features() & wgpu::Features::INDIRECT_FIRST_INSTANCE,
                ..Default::default()
            })
            .await
            .unwrap()
    });
    for (alpha, separate_pages) in [(255, false), (128, true)] {
        check_pages(&device, &queue, alpha, separate_pages);
    }
}

#[test]
#[ignore = "requires a Vulkan adapter; run explicitly with cargo test -p render gpu_prim_transforms -- --ignored"]
fn gpu_prim_transforms() {
    let (device, queue) = block_on(async {
        let instance =
            wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        adapter.request_device(&DeviceDescriptor::default()).await.unwrap()
    });
    check_mirrored_render(&device, &queue);
    check_planar_projection(&device, &queue);
}

#[test]
#[ignore = "requires a Vulkan adapter; run explicitly with cargo test -p render gpu_skip_rebuilds_scene -- --ignored"]
fn gpu_skip_rebuilds_scene() {
    let (device, queue) = block_on(async {
        let instance =
            wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        adapter.request_device(&DeviceDescriptor::default()).await.unwrap()
    });
    let (_scene, view, camera) = scene(255);
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(&device, &mut encoder, &view, None, camera, [128, 128], None);
    assert!(!geometry.draw_list.draws().is_empty());
    geometry.pending_meshes.insert(*geometry.cache.mesh_ids.first().expect("visible mesh demand"));
    assert!(!geometry.cache.mesh_representatives.is_empty());
    assert!(!geometry.cache.mesh_compositions.is_empty());
    assert!(!geometry.cache.mesh_priorities.is_empty());
    assert!(!geometry.cache.mesh_ready.is_empty());
    geometry.skip();
    assert!(geometry.pending_meshes.is_empty());
    assert!(geometry.cache.mesh_ids.is_empty());
    assert!(geometry.cache.mesh_set.is_empty());
    assert!(geometry.cache.mesh_representatives.is_empty());
    assert!(geometry.cache.mesh_compositions.is_empty());
    assert!(geometry.cache.mesh_priorities.is_empty());
    assert!(geometry.cache.mesh_ready.is_empty());
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(&device, &mut encoder, &view, None, camera, [128, 128], None);
    assert!(!geometry.draw_list.draws().is_empty());
}

#[test]
#[ignore = "requires a Vulkan adapter; run explicitly with cargo test -p render gpu_hidden_items_leave_no_instances_in_draws -- --ignored"]
fn gpu_hidden_items_leave_no_instances_in_draws() {
    let (device, queue) = block_on(async {
        let instance =
            wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        adapter.request_device(&DeviceDescriptor::default()).await.unwrap()
    });
    let (_scene, view, _) = scene(128);
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    let hidden = Camera::new(DVec3::new(13.0, 0.0, 0.0), Vec3::X, 1.0, [128, 128]).unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(&device, &mut encoder, &view, None, hidden, [128, 128], None);
    assert_eq!(geometry.stats.instances, 1);
    assert!(geometry.draw_list.has_blended());

    let visible = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [128, 128]).unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(&device, &mut encoder, &view, None, visible, [128, 128], None);
    assert_eq!(geometry.stats.instances, 2);
    assert_visible_draw_coverage(&geometry);

    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(&device, &mut encoder, &view, None, hidden, [128, 128], None);
    assert_eq!(geometry.stats.instances, 1);
    assert_visible_draw_coverage(&geometry);
}

#[test]
#[ignore = "requires a Vulkan adapter; run explicitly with cargo nextest run -p render --run-ignored only -E 'test(gpu_agent_pose_override_patches_draw_and_pick)'"]
fn gpu_agent_pose_override_patches_draw_and_pick() {
    let (device, queue) = block_on(async {
        let instance =
            wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        adapter.request_device(&DeviceDescriptor::default()).await.unwrap()
    });
    let (_scene, view, _) = scene_with_agent();
    let size = [128, 128];
    let agent_id = uuid::Uuid::from_u128(1);
    let agent = view.objects().find(|object| object.detail().summary.id == agent_id).expect("fixture avatar");
    let entity = agent.detail().summary.entity;
    let Placement::Ready(simulator_transform) = agent.placement() else { panic!("fixture avatar must have a pose") };
    let camera = Camera::new(DVec3::new(0.0, -2.0, 0.0), Vec3::X, 1.0, size).unwrap();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(&device, &mut encoder, &view, None, camera, size, None);
    assert_eq!(geometry.stats.layouts, 1);
    let old_ray = camera.ray([64.0, 64.0], size);
    assert!(geometry.pick(camera, old_ray).unwrap().is_some(), "the simulator pose is pickable");

    let predicted_pose = WorldPose { position: simulator_transform.pose.position + DVec3::Y * 2.0, ..simulator_transform.pose };
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(&device, &mut encoder, &view, Some(AgentPose::new(agent_id, predicted_pose)), camera, size, None);
    assert_eq!(geometry.stats.layouts, 0, "a pose-only update does not rebuild the draw-list layout");
    assert!(geometry.stats.faces_patched > 0, "visible avatar faces use the pose-patch path");
    let index = geometry.cache.objects[&entity];
    let item = geometry.cache.items.get(index).expect("cached avatar");
    assert_eq!(item.position, predicted_pose.position);
    let slot = item.faces.iter().find_map(|face| face.slot).expect("visible avatar face slot");
    assert_eq!(geometry.draw_list.instance_transform(slot), Some(item.transform.as_slice()));
    assert_eq!(geometry.pick(camera, old_ray).unwrap(), None, "picking no longer sees the old avatar position");
    let moved_ray = camera.ray([41.0, 64.0], size);
    let moved_hit = geometry.pick(camera, moved_ray).unwrap().expect("predicted avatar position is pickable");
    assert!(moved_hit.distance(predicted_pose.position) < 1.0, "picked point follows the predicted pose: {moved_hit:?}");

    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(&device, &mut encoder, &view, None, camera, size, None);
    assert_eq!(geometry.stats.layouts, 0, "removing an override restores the item through a pose patch");
    assert!(geometry.stats.faces_patched > 0);
    let item = geometry.cache.items.get(index).expect("cached avatar");
    assert_eq!(item.position, simulator_transform.pose.position);
    let slot = item.faces.iter().find_map(|face| face.slot).expect("visible avatar face slot");
    assert_eq!(geometry.draw_list.instance_transform(slot), Some(item.transform.as_slice()));
    assert!(geometry.pick(camera, old_ray).unwrap().is_some(), "picking returns to the simulator pose");
    assert_eq!(geometry.pick(camera, moved_ray).unwrap(), None, "the restored avatar is absent from the predicted ray");
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo test -p render gpu_avatar_mesh_ready_late_uses_agent_pose -- --ignored"]
fn gpu_avatar_mesh_ready_late_uses_agent_pose() {
    let (device, queue) = vulkan_device();
    let (mut world, mut scene, mut view, camera) = avatar_mesh_scene();
    let agent_id = Uuid::from_u128(1);
    let agent = view.objects().next().expect("avatar");
    let entity = agent.detail().summary.entity;
    let predicted_pose = WorldPose { position: DVec3::new(10.0, 0.0, 0.0), rotation: math::Quat::IDENTITY };
    let agent_pose = Some(AgentPose::new(agent_id, predicted_pose));
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    prepare_agent_frame(&device, &queue, &mut geometry, &view, camera, agent_pose);
    assert!(!geometry.cache.objects.contains_key(&entity), "the unresolved avatar has no render item");

    let mut observation = ObjectObservation::new(
        UpdateSource::new(Region::new(1), 10, OrderEvidence::Ordered(10), 0),
        agent_id,
        std::num::NonZeroU32::new(1).expect("avatar local id"),
        Kind::Avatar,
        ParentUpdate::Root,
    );
    observation.pose = Some(Pose { position: Some(Vec3::new(10.0, -2.0, 0.0)), rotation: Some(math::Quat::IDENTITY) });
    let applied = world.observe_object(observation).expect("avatar pose update");
    assert_matches!(applied.outcome, world::ApplyOutcome::Updated(_));
    scene.notify_world_change();
    settle_scene(&world, &mut scene, &mut view, Instant::now());

    prepare_agent_frame(&device, &queue, &mut geometry, &view, camera, agent_pose);
    assert_eq!(geometry.stats.instances, 1);
    let index = geometry.cache.objects[&entity];
    let item = geometry.cache.items.get(index).expect("late avatar render item");
    assert!(item.replacement.is_none());
    assert_eq!(item.position, predicted_pose.position);
    let slot = item.faces.iter().find_map(|face| face.slot).expect("visible avatar face slot");
    assert_eq!(geometry.draw_list.instance_transform(slot), Some(item.transform.as_slice()));
    assert_visible_draw_coverage(&geometry);
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo test -p render gpu_mesh_faces_and_alpha -- --ignored"]
fn gpu_mesh_faces_and_alpha() {
    let mesh_asset = AssetId::new(Uuid::from_u128(801));
    let other_asset = AssetId::new(Uuid::from_u128(804));
    let red = AssetId::new(Uuid::from_u128(802));
    let green = AssetId::new(Uuid::from_u128(803));
    let (mut world, mut scene, mut view, _, requests) =
        mesh_scene(&[(1, mesh_asset, Vec3::new(5.0, 0.0, 0.0)), (2, other_asset, Vec3::new(1.2, 1.4, 0.0))], Some((red, green)));
    let camera = Camera::new(DVec3::ZERO, Vec3::X, 25_f64.to_radians(), [128, 128]).expect("fixture camera");
    scene.set_camera(camera);
    let header = requests
        .iter()
        .find(|request| matches!(request.request, Request::MeshHeader(asset) if asset == mesh_asset))
        .copied()
        .expect("mesh header request");
    let other_header = requests
        .iter()
        .find(|request| matches!(request.request, Request::MeshHeader(asset) if asset == other_asset))
        .copied()
        .expect("other mesh header request");
    let bytes = test_support::mesh::asset_with_lod_faces(&[(2, 2), (3, 1)]);
    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let high = requests
        .iter()
        .find(|request| matches!(request.request, Request::MeshLod(lod) if lod.lod() == assets::mesh::MeshLod::High))
        .copied()
        .expect("selected high LOD request");
    let medium = requests
        .iter()
        .find(|request| matches!(request.request, Request::MeshLod(lod) if lod.lod() == assets::mesh::MeshLod::Medium))
        .copied()
        .expect("fallback medium LOD request");
    scene.complete_asset(medium.ticket, prepared_asset(medium.request, bytes.clone()));
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    for request in requests.into_iter().filter(|request| request.request.resource().kind == assets::Kind::Texture) {
        let texture = if request.request.resource().id == red { test_support::texture::RGB_J2C } else { test_support::texture::GRAY_J2C };
        scene.complete_asset(request.ticket, prepared_asset(request.request, texture.to_vec()));
    }
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let object = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_001)).expect("mesh object");
    assert_eq!(object.mesh().map(|(key, _)| key), Some(DrawnMeshKey::Asset { asset: mesh_asset, lod: assets::mesh::MeshLod::Medium }));
    let faces = object.materials().expect("fallback materials");
    let red_face = faces.iter().find(|face| face.slot == 0).expect("first fallback face material");
    let green_face = faces.iter().find(|face| face.slot == 1).expect("second fallback face material");
    assert_eq!(red_face.base_color_texture.as_ref().expect("red texture").state().asset, red);
    assert_eq!(green_face.base_color_texture.as_ref().expect("green texture").state().asset, green);
    assert_eq!(red_face.alpha, crate::material::AlphaBehavior::Opaque);
    assert_eq!(green_face.alpha, crate::material::AlphaBehavior::Blend);

    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    let depth = geometry.depth(&device, [128, 128]).expect("depth target");
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    assert_eq!(geometry.stats.instances, 1, "only the target fallback is ready");

    scene.complete_asset(high.ticket, prepared_asset(high.request, bytes));
    let other_bytes = test_support::mesh::asset_with_lod_geometry(&[(3, 3, 65_535, 65_535)]);
    scene.complete_asset(other_header.ticket, prepared_asset(other_header.request, other_bytes.clone()));
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let other_high = requests
        .iter()
        .find(|request| {
            request.request.resource().id == other_asset
                && matches!(request.request, Request::MeshLod(lod) if lod.lod() == assets::mesh::MeshLod::High)
        })
        .copied()
        .expect("other selected high LOD request");
    scene.complete_asset(other_high.ticket, prepared_asset(other_high.request, other_bytes));
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let target = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_001)).expect("replacement target");
    assert_eq!(target.mesh().map(|(key, _)| key), Some(DrawnMeshKey::Asset { asset: mesh_asset, lod: assets::mesh::MeshLod::High }));
    assert_eq!(target.materials().expect("selected high material").len(), 1, "new mesh materials use only the published face slots");

    let frame = geometry.admit().unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(&device, &mut encoder, &view, None, camera, [128, 128], None);
    assert_eq!(
        geometry.stats.pending_meshes, 1,
        "the larger new mesh uses this frame's upload budget: instances={}, uploaded={}",
        geometry.stats.instances, geometry.stats.uploaded_bytes
    );
    assert_eq!(geometry.stats.instances, 2, "the old multi-face item stays drawn while the replacement waits");
    queue.submit([encoder.finish()]);
    let pixels = render_pixels(&device, &queue, [128, 128], &depth, |pass| geometry.draw(pass));
    geometry.submitted(frame);
    let red_pixels = pixels.iter().filter(|pixel| pixel[0] > pixel[1].saturating_add(24)).count();
    let green_pixels = pixels.iter().filter(|pixel| pixel[1] > pixel[0].saturating_add(24)).count();
    let max_red = pixels.iter().map(|pixel| pixel[0]).max().unwrap_or_default();
    let max_green = pixels.iter().map(|pixel| pixel[1]).max().unwrap_or_default();
    assert!(red_pixels > 50, "fallback opaque face renders with red material: {red_pixels} pixels");
    assert!(green_pixels > 50, "fallback blended face renders with green material: {green_pixels} pixels");
    assert!(max_green < max_red, "fallback face alpha blends against the black clear color: green={max_green}, red={max_red}");

    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    assert_eq!(geometry.stats.pending_meshes, 0, "the replacement becomes resident on the next frame");
    assert_eq!(geometry.stats.instances, 2);

    let _removed = world
        .remove_object(
            UpdateSource::new(Region::new(1), 10, OrderEvidence::Ordered(10), 0),
            std::num::NonZeroU32::new(2).expect("fixture local ID"),
        )
        .expect("competing object removal applies");
    scene.notify_world_change();
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    assert_eq!(geometry.stats.instances, 1, "only the mesh target remains visible");

    let mut patch = world::ObjectPatch::new(
        UpdateSource::new(Region::new(1), 11, OrderEvidence::Ordered(11), 0),
        std::num::NonZeroU32::new(1).expect("fixture local ID"),
    );
    patch.shape = FieldUpdate::Set(Shape::Mesh { asset_id: mesh_asset, flags: SculptFlags { invert: true, mirror: true } });
    let _applied = world.patch_object(patch).expect("mesh flags update applies");
    scene.notify_world_change();
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let object = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_001)).expect("flagged mesh target");
    assert_eq!(
        object
            .materials()
            .and_then(|materials| materials.iter().find(|face| face.slot == 0))
            .and_then(|face| face.base_color_texture.as_ref())
            .map(|texture| texture.state().asset),
        Some(red),
        "mesh flags preserve the object's existing face material"
    );
    let frame = geometry.admit().expect("geometry frame lease");
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(&device, &mut encoder, &view, None, camera, [128, 128], None);
    assert!(geometry.stats.layouts > 0, "mirror and invert flag changes rebuild the draw layout");
    assert_eq!(geometry.stats.instances, 1);
    queue.submit([encoder.finish()]);
    let hit = (0..128).step_by(8).any(|y| {
        (0..128).step_by(8).any(|x| geometry.pick(camera, camera.ray([f64::from(x), f64::from(y)], [128, 128])).unwrap().is_some())
    });
    geometry.submitted(frame);
    assert!(hit, "a mirrored and inverted mesh remains pickable after winding and normal updates");
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo test -p render gpu_large_mesh_first_upload_exceeds_upload_target -- --ignored"]
fn gpu_large_mesh_first_upload_exceeds_upload_target() {
    let asset = AssetId::new(Uuid::from_u128(810));
    let (world, mut scene, mut view, camera, requests) = mesh_scene(&[(1, asset, Vec3::new(2.0, 0.0, 0.0))], None);
    let bytes = test_support::mesh::asset_with_lod_geometry(&[(3, 3, 65_535, 65_535)]);
    let header = requests
        .iter()
        .find(|request| matches!(request.request, Request::MeshHeader(id) if id == asset))
        .copied()
        .expect("mesh header request");
    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let high = requests
        .iter()
        .find(|request| matches!(request.request, Request::MeshLod(lod) if lod.lod() == assets::mesh::MeshLod::High))
        .copied()
        .expect("selected high LOD request");
    scene.complete_asset(high.ticket, prepared_asset(high.request, bytes));
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());

    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    assert!(geometry.stats.uploaded_bytes > 4 * 1024 * 1024);
    assert_eq!(geometry.stats.pending_meshes, 0);
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo test -p render gpu_lod_keeps_fallback_during_unrelated_retirement -- --ignored"]
fn gpu_lod_keeps_fallback_during_unrelated_retirement() {
    let target = AssetId::new(Uuid::from_u128(830));
    let unrelated = AssetId::new(Uuid::from_u128(831));
    let target_bytes = test_support::mesh::asset_with_lod_geometry(&[(2, 8, 65_535, 65_535), (3, 8, 65_535, 131_079)]);
    let unrelated_bytes = test_support::mesh::asset_with_lod_geometry(&[(2, 8, 65_535, 196_605)]);
    let (mut world, mut scene, mut view, camera, requests) =
        mesh_scene(&[(1, target, Vec3::new(2.0, 0.0, 0.0)), (2, unrelated, Vec3::new(4.0, 0.0, 0.0))], None);
    for request in requests {
        let Request::MeshHeader(asset) = request.request else { continue };
        let bytes = if asset == target { target_bytes.clone() } else { unrelated_bytes.clone() };
        scene.complete_asset(request.ticket, prepared_asset(request.request, bytes));
    }
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let mut high = None;
    for request in requests {
        let Request::MeshLod(lod) = request.request else { continue };
        let bytes = if request.request.resource().id == target { target_bytes.clone() } else { unrelated_bytes.clone() };
        if request.request.resource().id == target && lod.lod() == assets::mesh::MeshLod::High {
            high = Some(request);
        } else if lod.lod() == assets::mesh::MeshLod::Medium {
            scene.complete_asset(request.ticket, prepared_asset(request.request, bytes));
        }
    }
    let high = high.expect("selected high LOD request remains pending");
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());

    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    for _ in 0..8 {
        prepare_frame(&device, &queue, &mut geometry, &view, camera);
        if geometry.stats.pending_meshes == 0 {
            break;
        }
    }
    let target_entity = view
        .objects()
        .find(|object| object.detail().summary.id == Uuid::from_u128(100_001))
        .expect("replacement target")
        .detail()
        .summary
        .entity;
    let unrelated_entity = view
        .objects()
        .find(|object| object.detail().summary.id == Uuid::from_u128(100_002))
        .expect("unrelated mesh object")
        .detail()
        .summary
        .entity;
    let fallback_id = geometry.cache.items.get(geometry.cache.objects[&target_entity]).expect("fallback item").mesh.id();
    let unrelated_id = geometry.cache.items.get(geometry.cache.objects[&unrelated_entity]).expect("unrelated item").mesh.id();
    assert!(geometry.meshes.contains_key(&unrelated_id));

    let _ = world
        .remove_object(
            UpdateSource::new(Region::new(1), 30, OrderEvidence::Ordered(30), 0),
            std::num::NonZeroU32::new(2).expect("unrelated local id"),
        )
        .expect("unrelated object removal applies");
    scene.notify_world_change();
    settle_scene(&world, &mut scene, &mut view, Instant::now());
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    assert!(!geometry.cache.objects.contains_key(&unrelated_entity));
    assert!(geometry.meshes.contains_key(&unrelated_id), "unrelated GPU allocation remains available for eviction");
    let blocker_bytes = crate::geometry::MESH_BYTES - geometry.memory.mesh_state().bytes;
    let blocker = geometry.memory.reserve(blocker_bytes, crate::geometry::Resource::Mesh).expect("fill mesh budget");

    scene.complete_asset(high.ticket, prepared_asset(high.request, target_bytes));
    settle_scene(&world, &mut scene, &mut view, Instant::now());
    let replacement_id = view.object(target_entity).and_then(|object| object.mesh().map(|(_, mesh)| mesh.id())).expect("high LOD geometry");
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    let held = geometry.cache.items.get(geometry.cache.objects[&target_entity]).expect("fallback item");
    assert_eq!(held.mesh.id(), fallback_id);
    assert_eq!(held.replacement.as_ref().map(|replacement| replacement.item.mesh.id()), Some(replacement_id));
    assert!(geometry.meshes.contains_key(&fallback_id), "the visible fallback stays resident");
    assert!(!geometry.meshes.contains_key(&unrelated_id), "the unrelated allocation was retired to make room");
    assert_eq!(geometry.stats.mesh_budget_blocked, 1);
    assert_eq!(geometry.stats.instances, 1);

    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    let current = geometry.cache.items.get(geometry.cache.objects[&target_entity]).expect("replacement item");
    assert_eq!(current.mesh.id(), replacement_id);
    assert!(current.replacement.is_none());
    assert_eq!(geometry.stats.mesh_budget_blocked, 0);
    assert_eq!(geometry.stats.instances, 1);
    assert!(geometry.stats.mesh_bytes <= crate::geometry::MESH_BYTES);

    let memory = geometry.memory.clone();
    drop(blocker);
    drop(geometry);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    assert_eq!(memory.mesh_state().bytes, 0);
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo test -p render gpu_lod_releases_fallback_when_replacement_exceeds_remaining_mesh_budget -- --ignored"]
fn gpu_lod_releases_fallback_when_replacement_exceeds_remaining_mesh_budget() {
    let target = AssetId::new(Uuid::from_u128(820));
    let other_a = AssetId::new(Uuid::from_u128(821));
    let other_b = AssetId::new(Uuid::from_u128(822));
    let target_bytes = test_support::mesh::asset_with_lod_geometry(&[(2, 8, 65_535, 65_535), (3, 8, 65_535, 131_079)]);
    let other_a_bytes = test_support::mesh::asset_with_lod_geometry(&[(2, 8, 65_535, 196_605)]);
    let other_b_bytes = test_support::mesh::asset_with_lod_geometry(&[(2, 4, 65_535, 65_535)]);
    let (world, mut scene, mut view, camera, requests) = mesh_scene(
        &[(1, target, Vec3::new(2.0, 0.0, 0.0)), (2, other_a, Vec3::new(4.0, 0.0, 0.0)), (3, other_b, Vec3::new(6.0, 0.0, 0.0))],
        None,
    );
    for request in requests {
        let Request::MeshHeader(asset) = request.request else { continue };
        let bytes = if asset == target {
            target_bytes.clone()
        } else if asset == other_a {
            other_a_bytes.clone()
        } else {
            other_b_bytes.clone()
        };
        scene.complete_asset(request.ticket, prepared_asset(request.request, bytes));
    }
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let mut high = None;
    for request in requests {
        let Request::MeshLod(lod) = request.request else { continue };
        let asset = request.request.resource().id;
        if asset == target && lod.lod() == assets::mesh::MeshLod::High {
            high = Some(request);
            continue;
        }
        if lod.lod() == assets::mesh::MeshLod::Medium {
            let bytes = if asset == target {
                target_bytes.clone()
            } else if asset == other_a {
                other_a_bytes.clone()
            } else {
                other_b_bytes.clone()
            };
            scene.complete_asset(request.ticket, prepared_asset(request.request, bytes));
        }
    }
    let high = high.expect("selected high LOD request remains pending");
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());
    assert_eq!(
        view.objects()
            .find(|object| object.detail().summary.id == Uuid::from_u128(100_001))
            .and_then(|object| object.mesh().map(|(key, _)| key)),
        Some(DrawnMeshKey::Asset { asset: target, lod: assets::mesh::MeshLod::Medium })
    );

    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    for _ in 0..8 {
        prepare_frame(&device, &queue, &mut geometry, &view, camera);
        if geometry.stats.pending_meshes == 0 {
            break;
        }
    }
    assert_eq!(geometry.stats.pending_meshes, 0);
    assert!(geometry.stats.mesh_bytes > 48 * 1024 * 1024);
    assert!(geometry.stats.mesh_bytes < 49 * 1024 * 1024);
    assert_eq!(geometry.stats.instances, 3);
    let target_entity = view
        .objects()
        .find(|object| object.detail().summary.id == Uuid::from_u128(100_001))
        .expect("replacement target")
        .detail()
        .summary
        .entity;
    let fallback_id = geometry.cache.items.get(geometry.cache.objects[&target_entity]).expect("fallback item").mesh.id();

    scene.complete_asset(high.ticket, prepared_asset(high.request, target_bytes));
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    assert_eq!(geometry.stats.mesh_budget_blocked, 1);
    assert_eq!(geometry.stats.pending_meshes, 1);
    assert_eq!(geometry.stats.instances, 2);
    assert!(!geometry.meshes.contains_key(&fallback_id), "the held fallback is retired when it is the allocation victim");

    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    assert_eq!(geometry.stats.mesh_budget_blocked, 0);
    assert_eq!(
        geometry.stats.pending_meshes,
        0,
        "the replacement uploads after the fallback hold is released: bytes={}, meshes={}",
        geometry.stats.mesh_bytes,
        geometry.meshes.len()
    );
    assert_eq!(geometry.stats.instances, 3);
    assert!(geometry.stats.mesh_bytes <= 64 * 1024 * 1024);
}
