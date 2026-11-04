use std::{
    assert_matches,
    sync::{Arc, mpsc},
    time::Instant,
};

use assets::{Budget as AssetBudget, Failure, FetchResponse, LoadJob, Priority, Request, Store};
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
        assert!(geometry.upload_mesh(device, shared, None, &mut 0, true));
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
            ReconcileStep::Asset(
                AssetChange::UpdateImportance { .. } | AssetChange::UpdatePriority { .. } | AssetChange::UpdateSize { .. },
            ) => {}
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
    mesh_scene_with_alpha(objects, textures, 128)
}

fn mesh_scene_with_alpha(
    objects: &[(u32, AssetId, Vec3)],
    textures: Option<(AssetId, AssetId)>,
    blended_alpha: u8,
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
                    vec![(1, world::FaceAppearance { texture: Some(green), color: [0, 255, 0, blended_alpha], ..Default::default() })],
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
    let input = match request {
        Request::MeshLod(block) => {
            let range = block.range();
            let start = usize::try_from(range.start).unwrap();
            let end = usize::try_from(range.end).unwrap();
            let bytes = bytes.get(start..end).expect("mesh fixture contains the requested LOD block").to_vec();
            job.complete_response(Ok(FetchResponse::Partial { bytes, range, total_length: Some(block.asset_length()) })).unwrap()
        }
        Request::MeshHeader(_) if bytes.len() > job.response_limit() => {
            let failure = job.complete_response(Err(Failure::FullAssetRequired)).expect_err("header fallback requires a full retry");
            store.complete(failure.into(), now);
            let LoadJob::Fetch(retry) = store.next_job(now).expect("full mesh retry is admitted") else {
                panic!("full mesh retry unexpectedly used a cached source");
            };
            assert!(retry.force_full_mesh());
            retry.complete(Ok(bytes)).unwrap()
        }
        _ => job.complete(Ok(bytes)).unwrap(),
    };
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
    prepare_frame_at(device, queue, geometry, view, camera, Instant::now());
}

fn prepare_frame_at(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    geometry: &mut GeometryRenderer,
    view: &SceneView,
    camera: Camera,
    now: Instant,
) {
    let frame = geometry.admit().unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare_at(device, &mut encoder, view, None, camera, [128, 128], crate::FrameInput::default(), now);
    queue.submit([encoder.finish()]);
    geometry.submitted(frame);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
}

fn ready_mesh_scene(objects: &[(u32, AssetId, Vec3)], bytes: Vec<u8>) -> (world::World, Reconciler, SceneView, Camera) {
    let (world, mut scene, mut view, camera, requests) = mesh_scene(objects, None);
    for request in requests {
        if matches!(request.request, Request::MeshHeader(_)) {
            scene.complete_asset(request.ticket, prepared_asset(request.request, bytes.clone()));
        }
    }
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    for request in requests {
        if matches!(request.request, Request::MeshLod(_)) {
            scene.complete_asset(request.ticket, prepared_asset(request.request, bytes.clone()));
        }
    }
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());
    (world, scene, view, camera)
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
#[ignore = "requires a Vulkan adapter; run explicitly with cargo nextest run -p render --run-ignored only -E 'test(gpu_replacement_visibility_tracks_its_own_bounds)'"]
fn gpu_replacement_visibility_tracks_its_own_bounds() {
    let first_asset = AssetId::new(Uuid::from_u128(801));
    let second_asset = AssetId::new(Uuid::from_u128(802));
    let first_bytes = test_support::mesh::asset_with_lod_faces(&[(3, 1)]);
    let second_bytes = test_support::mesh::asset_with_lod_bounds(3, [-0.01, 14.0, -1.0], [0.01, 16.0, 1.0]);
    let (mut world, mut scene, mut view, first_camera, requests) = mesh_scene(&[(1, first_asset, Vec3::new(2.0, 0.0, 0.0))], None);
    let first_header = requests
        .iter()
        .find(|request| matches!(request.request, Request::MeshHeader(asset) if asset == first_asset))
        .copied()
        .expect("held mesh header request");
    scene.complete_asset(first_header.ticket, prepared_asset(first_header.request, first_bytes.clone()));
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let first_high = requests
        .iter()
        .find(|request| {
            request.request.resource().id == first_asset
                && matches!(request.request, Request::MeshLod(lod) if lod.lod() == assets::mesh::MeshLod::High)
        })
        .copied()
        .expect("held high LOD request");
    scene.complete_asset(first_high.ticket, prepared_asset(first_high.request, first_bytes));
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());

    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    prepare_frame(&device, &queue, &mut geometry, &view, first_camera);
    let entity = view
        .objects()
        .find(|object| object.detail().summary.id == Uuid::from_u128(100_001))
        .expect("held mesh object")
        .detail()
        .summary
        .entity;
    let item_index = geometry.cache.objects[&entity];
    let held_mesh = geometry.cache.items.get(item_index).expect("held mesh item").mesh.id();
    assert!(geometry.meshes.contains_key(&held_mesh));
    assert!(geometry.cache.candidates.contains(&item_index));
    let initial_visible_union = geometry.cache.candidates.len() + geometry.cache.replacement_visible.len();
    assert_eq!(initial_visible_union, 1, "the held item is the only visible candidate");

    let fillers = fill_mesh_pages(&device, &mut geometry);
    assert!(!fillers.is_empty(), "fixture mesh page has capacity to reserve");
    let blocker_bytes = crate::geometry::MESH_BYTES - geometry.memory.mesh_state().bytes;
    let blocker = geometry.memory.reserve(blocker_bytes, crate::geometry::Resource::Mesh).expect("fill mesh budget");
    let mut patch = world::ObjectPatch::new(
        UpdateSource::new(Region::new(1), 2, OrderEvidence::Ordered(2), 0),
        std::num::NonZeroU32::new(1).expect("target local ID"),
    );
    patch.shape = FieldUpdate::Set(Shape::Mesh { asset_id: second_asset, flags: SculptFlags::default() });
    let _ = world.patch_object(patch).expect("replacement mesh applies");
    scene.notify_world_change();
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let second_header = requests
        .iter()
        .find(|request| matches!(request.request, Request::MeshHeader(asset) if asset == second_asset))
        .copied()
        .expect("replacement mesh header request");
    scene.complete_asset(second_header.ticket, prepared_asset(second_header.request, second_bytes.clone()));
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let second_high = requests
        .iter()
        .find(|request| {
            request.request.resource().id == second_asset
                && matches!(request.request, Request::MeshLod(lod) if lod.lod() == assets::mesh::MeshLod::High)
        })
        .copied()
        .expect("replacement high LOD request");
    scene.complete_asset(second_high.ticket, prepared_asset(second_high.request, second_bytes));
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let expected_replacement_id =
        view.object(entity).and_then(|object| object.mesh().map(|(_, mesh)| mesh.id())).expect("replacement mesh published");

    let second_camera = Camera::new(DVec3::ZERO, Vec3::new(2.0, 15.0, 0.0), 1.0, [128, 128]).expect("replacement camera");
    scene.set_camera(second_camera);
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());
    prepare_frame(&device, &queue, &mut geometry, &view, second_camera);
    let held = geometry.cache.items.get(item_index).expect("held item with replacement");
    let replacement_id = held.replacement.as_ref().expect("pending replacement").item.mesh.id();
    assert_eq!(replacement_id, expected_replacement_id);
    assert!(geometry.cache.candidates.is_empty(), "the held bounds are outside the new frustum");
    assert!(geometry.cache.replacement_visible.contains(&item_index), "the replacement bounds enter the new frustum");
    assert_eq!(
        geometry.cache.candidates.len() + geometry.cache.replacement_visible.len(),
        initial_visible_union,
        "moving visibility from held bounds to replacement bounds keeps the union size"
    );
    assert_eq!(geometry.stats.instances, 0, "an off-screen held mesh contributes no instances");
    assert_eq!(geometry.stats.pending_meshes, 1, "replacement demand survives the unchanged visible union");

    drop(blocker);
    prepare_frame(&device, &queue, &mut geometry, &view, second_camera);
    assert!(geometry.meshes.contains_key(&replacement_id), "the visible replacement uploads after budget is released");
    prepare_frame(&device, &queue, &mut geometry, &view, second_camera);
    let current = geometry.cache.items.get(item_index).expect("promoted replacement item");
    assert_eq!(current.mesh.id(), replacement_id);
    assert!(current.replacement.is_none());
    assert!(geometry.cache.visible.contains(&item_index));
    assert_eq!(geometry.stats.instances, 1);
    assert_visible_draw_coverage(&geometry);

    let memory = geometry.memory.clone();
    drop(geometry);
    drop(fillers);
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
        mesh_scene(&[(1, mesh_asset, Vec3::new(5.0, 0.0, 0.0)), (2, other_asset, Vec3::new(3.0, 0.5, 0.0))], Some((red, green)));
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
    for request in requests.iter().filter(|request| request.request.resource().kind == assets::Kind::Texture) {
        let texture = if request.request.resource().id == red { test_support::texture::RGB_J2C } else { test_support::texture::GRAY_J2C };
        let status = prepared_asset(request.request, texture.to_vec());
        assert_matches!(status, assets::Status::Ready(_));
        scene.complete_asset(request.ticket, status);
    }
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let bytes = test_support::mesh::asset_with_lod_faces(&[(2, 2), (3, 1)]);
    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let other_header = requests
        .iter()
        .find(|request| matches!(request.request, Request::MeshHeader(asset) if asset == other_asset))
        .copied()
        .unwrap_or(other_header);
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
        let status = prepared_asset(request.request, texture.to_vec());
        assert_matches!(status, assets::Status::Ready(_));
        scene.complete_asset(request.ticket, status);
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
    let first = Instant::now() - crate::geometry::first_appearance::ASSET_WAIT;
    let frame = geometry.admit().unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare_at(&device, &mut encoder, &view, None, camera, [128, 128], crate::FrameInput::default(), first);
    queue.submit([encoder.finish()]);
    geometry.submitted(frame);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    assert_eq!(geometry.stats.instances, 0, "first appearance waits for the matching textures to reach the GPU");
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    assert_eq!(geometry.stats.instances, 1, "the complete fallback appears by the optional asset deadline");

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
    let max_red = pixels.iter().filter(|pixel| pixel[0] > pixel[1].saturating_add(24)).map(|pixel| pixel[0]).max().unwrap_or_default();
    let max_green = pixels.iter().filter(|pixel| pixel[1] > pixel[0].saturating_add(24)).map(|pixel| pixel[1]).max().unwrap_or_default();
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
#[ignore = "requires a Vulkan adapter; run with cargo test -p render gpu_lod_keeps_fallback -- --ignored"]
fn gpu_lod_keeps_fallback() {
    let target = AssetId::new(Uuid::from_u128(820));
    let other_a = AssetId::new(Uuid::from_u128(821));
    let other_b = AssetId::new(Uuid::from_u128(822));
    let target_bytes = test_support::mesh::asset_with_lod_geometry(&[(2, 8, 65_535, 65_535), (3, 8, 65_535, 131_079)]);
    let other_a_bytes = test_support::mesh::asset_with_lod_geometry(&[(2, 8, 65_535, 196_605)]);
    let other_b_bytes = test_support::mesh::asset_with_lod_geometry(&[(2, 4, 65_535, 65_535)]);
    let (mut world, mut scene, mut view, camera, requests) = mesh_scene(
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
    let other_a_entity = view
        .objects()
        .find(|object| object.detail().summary.id == Uuid::from_u128(100_002))
        .expect("capacity freeing object")
        .detail()
        .summary
        .entity;
    let fallback_id = geometry.cache.items.get(geometry.cache.objects[&target_entity]).expect("fallback item").mesh.id();
    let other_a_id = geometry.cache.items.get(geometry.cache.objects[&other_a_entity]).expect("capacity freeing item").mesh.id();

    scene.complete_asset(high.ticket, prepared_asset(high.request, target_bytes));
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let replacement_id = view.object(target_entity).and_then(|object| object.mesh().map(|(_, mesh)| mesh.id())).expect("high LOD geometry");
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    let held = geometry.cache.items.get(geometry.cache.objects[&target_entity]).expect("fallback item");
    assert_eq!(held.mesh.id(), fallback_id);
    assert_eq!(held.replacement.as_ref().map(|replacement| replacement.item.mesh.id()), Some(replacement_id));
    assert_eq!(geometry.stats.mesh_budget_blocked, 1);
    assert_eq!(geometry.stats.pending_meshes, 1);
    assert_eq!(geometry.stats.instances, 3);
    assert_eq!(geometry.cache.visible.len(), 3);
    assert!(geometry.meshes.contains_key(&fallback_id), "the displayed fallback stays resident under pressure");
    assert!(geometry.memory.mesh_state().bytes <= crate::geometry::MESH_BYTES);
    assert!(!geometry.needs_frame(Instant::now()), "a blocked replacement does not schedule repeated frames");

    for _ in 0..3 {
        prepare_frame(&device, &queue, &mut geometry, &view, camera);
        let held = geometry.cache.items.get(geometry.cache.objects[&target_entity]).expect("fallback item");
        assert_eq!(held.mesh.id(), fallback_id);
        assert_eq!(held.replacement.as_ref().map(|replacement| replacement.item.mesh.id()), Some(replacement_id));
        assert_eq!(geometry.stats.mesh_budget_blocked, 1);
        assert_eq!(geometry.stats.pending_meshes, 1);
        assert_eq!(geometry.stats.instances, 3);
        assert_eq!(geometry.cache.visible.len(), 3);
        assert!(geometry.meshes.contains_key(&fallback_id), "the displayed fallback stays resident under pressure");
        assert!(geometry.memory.mesh_state().bytes <= crate::geometry::MESH_BYTES);
        assert!(!geometry.needs_frame(Instant::now()), "the blocked upload stays dormant until mesh capacity changes");
    }

    let _applied = world
        .remove_object(
            UpdateSource::new(Region::new(1), 30, OrderEvidence::Ordered(30), 0),
            std::num::NonZeroU32::new(2).expect("capacity freeing local id"),
        )
        .expect("capacity freeing object removal applies");
    scene.notify_world_change();
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());

    for _ in 0..4 {
        prepare_frame(&device, &queue, &mut geometry, &view, camera);
        if geometry.cache.items.get(geometry.cache.objects[&target_entity]).is_some_and(|item| item.replacement.is_none()) {
            break;
        }
    }
    let promoted = geometry.cache.items.get(geometry.cache.objects[&target_entity]).expect("promoted target");
    assert_eq!(promoted.mesh.id(), replacement_id);
    assert!(promoted.replacement.is_none(), "the resident replacement eventually becomes the displayed mesh");
    assert!(!geometry.cache.objects.contains_key(&other_a_entity));
    assert!(!geometry.meshes.contains_key(&other_a_id), "the removed object's allocation frees mesh capacity");
    assert_eq!(geometry.stats.mesh_budget_blocked, 0);
    assert_eq!(geometry.stats.pending_meshes, 0);
    assert_eq!(geometry.stats.instances, 2);
    assert!(geometry.memory.mesh_state().bytes <= crate::geometry::MESH_BYTES);

    assert!(!geometry.cache.mesh_ids.contains(&fallback_id), "promoted fallback no longer reserves upload demand");

    let memory = geometry.memory.clone();
    drop(geometry);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    assert_eq!(memory.mesh_state().bytes, 0, "renderer retirement releases every mesh page");
}

fn assert_visible_assets_match_source(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    geometry: &GeometryRenderer,
    view: &SceneView,
    camera: Camera,
    depth: &wgpu::TextureView,
    visible: &std::collections::HashSet<world::Entity>,
    phase: &str,
) {
    let mut references = Vec::new();
    for object in view.objects().filter(|object| visible.contains(&object.detail().summary.entity)) {
        let (_, shared) = object.mesh().expect("visible mesh asset is ready");
        let mesh = shared.mesh();
        let mut vertices = Vec::with_capacity(mesh.vertices().len() * VERTEX_BYTES);
        for vertex in mesh.vertices() {
            floats(&mut vertices, vertex.position.to_array());
            floats(&mut vertices, vertex.normal.to_array());
            floats(&mut vertices, vertex.uv.to_array());
        }
        let indices: Vec<_> = mesh.indices().iter().flat_map(|index| index.to_le_bytes()).collect();
        let Placement::Ready(transform) = object.placement() else { panic!("mesh fixture object must be placed") };
        let material = object.materials().expect("mesh fixture materials").first().expect("mesh fixture face material");
        let params = FaceParams::new(Some(material));
        let instance = packed_instance(
            &instance_base(transform.pose.position, transform.pose.rotation, transform.scale, false, camera.position()),
            &params,
            transform.scale,
        );
        references.push((
            device.create_buffer_init(&BufferInitDescriptor { label: None, contents: &vertices, usage: BufferUsages::VERTEX }),
            device.create_buffer_init(&BufferInitDescriptor { label: None, contents: &indices, usage: BufferUsages::INDEX }),
            device.create_buffer_init(&BufferInitDescriptor { label: None, contents: &instance, usage: BufferUsages::VERTEX }),
            mesh.indices().len() as u32,
        ));
    }
    assert_eq!(references.len(), visible.len(), "{phase}: every visible asset has one independent reference draw");
    let actual = render_pixels(device, queue, [128, 128], depth, |pass| geometry.draw(pass));
    let expected = render_pixels(device, queue, [128, 128], depth, |pass| {
        geometry.pipelines.bind_pipeline(pass, Pipeline::Surface);
        pass.set_bind_group(1, geometry.textures.binding(None), &[]);
        for (vertices, indices, instance, count) in &references {
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_vertex_buffer(1, instance.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..*count, 0, 0..1);
        }
    });
    let lit = actual.iter().filter(|pixel| pixel[0] > 16 || pixel[1] > 16 || pixel[2] > 16).count();
    assert!(lit > 100, "{phase}: source triangles render, {lit} pixels");
    assert_eq!(actual, expected, "{phase}: shared-page draws match complete decoded meshes");
}

fn complete_mesh_requests(
    world: &world::World,
    scene: &mut Reconciler,
    view: &mut SceneView,
    assets: &std::collections::HashMap<AssetId, Vec<u8>>,
    mut requests: Vec<AssetRequest>,
) {
    for _ in 0..8 {
        if requests.is_empty() {
            return;
        }
        for request in requests.drain(..) {
            if matches!(request.request, Request::MeshHeader(_) | Request::MeshLod(_)) {
                let bytes = assets.get(&request.request.resource().id).expect("fixture asset bytes").clone();
                scene.complete_asset(request.ticket, prepared_asset(request.request, bytes));
            }
        }
        requests = settle_scene(world, scene, view, Instant::now());
    }
    assert!(requests.is_empty(), "mesh fixture requests settle within the bounded chain");
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo test -p render gpu_asset_reuse_preserves_triangles -- --ignored"]
fn gpu_asset_reuse_preserves_triangles() {
    let (device, queue) = vulkan_device();
    let a = AssetId::new(Uuid::from_u128(840));
    let b = AssetId::new(Uuid::from_u128(841));
    let c = AssetId::new(Uuid::from_u128(842));
    let point = |degrees: f32| {
        let radians = degrees.to_radians();
        Vec3::new(3.0 * radians.cos(), 3.0 * radians.sin(), 0.0)
    };
    let assets = std::collections::HashMap::from([
        (a, test_support::mesh::asset_with_lod_bounds(3, [-0.01, -0.8, -0.5], [0.01, 0.1, 0.5])),
        (b, test_support::mesh::asset_with_lod_bounds(3, [-0.01, 0.0, -0.5], [0.01, 0.8, 0.5])),
        (c, test_support::mesh::asset_with_lod_bounds(3, [-0.01, -0.5, -0.5], [0.01, 0.5, 0.5])),
    ]);
    let (world, mut scene, mut view, _, mut requests) = mesh_scene(&[(1, a, point(-30.0)), (2, b, point(0.0)), (3, c, point(30.0))], None);
    let first_camera =
        Camera::new(DVec3::ZERO, Vec3::new(15_f32.to_radians().cos(), -15_f32.to_radians().sin(), 0.0), 40_f64.to_radians(), [128, 128])
            .expect("first fixture camera");
    let second_camera =
        Camera::new(DVec3::ZERO, Vec3::new(15_f32.to_radians().cos(), 15_f32.to_radians().sin(), 0.0), 40_f64.to_radians(), [128, 128])
            .expect("second fixture camera");
    scene.set_camera(first_camera);
    requests.extend(settle_scene(&world, &mut scene, &mut view, Instant::now()));
    complete_mesh_requests(&world, &mut scene, &mut view, &assets, requests);
    let a_entity =
        view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_001)).expect("asset A").detail().summary.entity;
    let b_entity =
        view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_002)).expect("asset B").detail().summary.entity;
    let c_entity =
        view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_003)).expect("asset C").detail().summary.entity;
    assert!(view.object(a_entity).and_then(|object| object.mesh()).is_some());
    assert!(view.object(b_entity).and_then(|object| object.mesh()).is_some());

    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    let depth = geometry.depth(&device, [128, 128]).expect("depth target");
    prepare_frame(&device, &queue, &mut geometry, &view, first_camera);
    let first_visible = std::collections::HashSet::from([a_entity, b_entity]);
    let actual_visible: std::collections::HashSet<_> =
        geometry.cache.visible.iter().filter_map(|index| geometry.cache.items.get(*index).and_then(|item| item.entity)).collect();
    assert_eq!(actual_visible, first_visible, "the initial camera admits the two intended assets");
    let a_id = geometry.cache.items.get(geometry.cache.objects[&a_entity]).expect("asset A item").mesh.id();
    let b_id = geometry.cache.items.get(geometry.cache.objects[&b_entity]).expect("asset B item").mesh.id();
    assert!(Arc::ptr_eq(geometry.meshes[&a_id].allocation.buffers(), geometry.meshes[&b_id].allocation.buffers()));
    assert_visible_assets_match_source(&device, &queue, &geometry, &view, first_camera, &depth, &first_visible, "initial camera");

    let fillers = fill_mesh_pages(&device, &mut geometry);
    assert_eq!(fillers.len(), 1, "the two source assets share one saturated page");
    let blocker_bytes = crate::geometry::MESH_BYTES - geometry.memory.mesh_state().bytes;
    let blocker = geometry.memory.reserve(blocker_bytes, crate::geometry::Resource::Mesh).expect("hold page-growth budget");

    scene.set_camera(second_camera);
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    complete_mesh_requests(&world, &mut scene, &mut view, &assets, requests);
    assert!(view.object(c_entity).and_then(|object| object.mesh()).is_some(), "the revealed asset has decoded geometry");
    prepare_frame(&device, &queue, &mut geometry, &view, second_camera);
    let second_visible = std::collections::HashSet::from([b_entity, c_entity]);
    let c_index = geometry.cache.objects[&c_entity];
    assert!(geometry.cache.candidates.contains(&c_index), "the revealed asset bounds enter the camera frustum");
    let actual_visible: std::collections::HashSet<_> =
        geometry.cache.visible.iter().filter_map(|index| geometry.cache.items.get(*index).and_then(|item| item.entity)).collect();
    assert_eq!(
        actual_visible,
        std::collections::HashSet::from([b_entity]),
        "the revealed asset stays whole while its page range is retired"
    );
    let c_id = view.object(c_entity).and_then(|object| object.mesh().map(|(_, mesh)| mesh.id())).expect("asset C mesh");
    assert!(!geometry.meshes.contains_key(&a_id), "the hidden allocation is retired to admit the revealed mesh");
    assert!(geometry.meshes.contains_key(&b_id));
    assert!(!geometry.meshes.contains_key(&c_id), "the first pressure frame waits for safe range reuse");
    assert_eq!(geometry.stats.pending_meshes, 1);
    assert_eq!(geometry.stats.instances, 1);
    assert!(geometry.needs_frame(Instant::now()), "retirement schedules a follow-up upload");
    assert_visible_assets_match_source(
        &device,
        &queue,
        &geometry,
        &view,
        second_camera,
        &depth,
        &std::collections::HashSet::from([b_entity]),
        "revealed asset pending",
    );
    prepare_frame(&device, &queue, &mut geometry, &view, second_camera);
    assert!(geometry.meshes.contains_key(&b_id));
    assert!(geometry.meshes.contains_key(&c_id));
    assert!(Arc::ptr_eq(geometry.meshes[&b_id].allocation.buffers(), geometry.meshes[&c_id].allocation.buffers()));
    assert_eq!(geometry.stats.pending_meshes, 0);
    assert_eq!(geometry.stats.instances, 2);
    let actual_visible: std::collections::HashSet<_> =
        geometry.cache.visible.iter().filter_map(|index| geometry.cache.items.get(*index).and_then(|item| item.entity)).collect();
    assert_eq!(actual_visible, second_visible, "camera movement reveals the adjacent asset after whole upload");
    assert_visible_assets_match_source(&device, &queue, &geometry, &view, second_camera, &depth, &second_visible, "first reuse");

    scene.set_camera(first_camera);
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    complete_mesh_requests(&world, &mut scene, &mut view, &assets, requests);
    prepare_frame(&device, &queue, &mut geometry, &view, first_camera);
    assert!(!geometry.meshes.contains_key(&c_id), "the second hidden allocation is retired on the reverse camera move");
    assert!(geometry.meshes.contains_key(&b_id));
    assert_eq!(geometry.stats.pending_meshes, 1);
    assert_eq!(geometry.stats.instances, 1);
    assert!(geometry.needs_frame(Instant::now()));
    assert_visible_assets_match_source(
        &device,
        &queue,
        &geometry,
        &view,
        first_camera,
        &depth,
        &std::collections::HashSet::from([b_entity]),
        "reverse asset pending",
    );
    prepare_frame(&device, &queue, &mut geometry, &view, first_camera);
    assert!(geometry.meshes.contains_key(&a_id));
    assert!(geometry.meshes.contains_key(&b_id));
    assert!(Arc::ptr_eq(geometry.meshes[&a_id].allocation.buffers(), geometry.meshes[&b_id].allocation.buffers()));
    assert_eq!(geometry.stats.pending_meshes, 0);
    assert_eq!(geometry.stats.instances, 2);
    assert_visible_assets_match_source(&device, &queue, &geometry, &view, first_camera, &depth, &first_visible, "reverse reuse");

    drop(blocker);
    drop(fillers);
    let memory = geometry.memory.clone();
    drop(geometry);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    assert_eq!(memory.mesh_state().bytes, 0);
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo test -p render --lib gpu_invisible_face_allows_mesh -- --ignored"]
fn gpu_invisible_face_allows_mesh() {
    let asset = AssetId::new(Uuid::from_u128(864));
    let visible_texture = AssetId::new(Uuid::from_u128(865));
    let hidden_texture = AssetId::new(Uuid::from_u128(866));
    let bytes = test_support::mesh::asset_with_lod_faces(&[(2, 2), (3, 2)]);
    let (world, mut scene, mut view, _, mut requests) =
        mesh_scene_with_alpha(&[(1, asset, Vec3::new(5.0, 0.0, 0.0))], Some((visible_texture, hidden_texture)), 0);
    let camera = Camera::new(DVec3::ZERO, Vec3::X, 25_f64.to_radians(), [128, 128]).expect("fixture camera");
    scene.set_camera(camera);
    requests.extend(settle_scene(&world, &mut scene, &mut view, Instant::now()));
    let visible_texture_requests: Vec<_> = requests
        .iter()
        .filter(|request| request.request.resource().id == visible_texture && request.request.resource().kind == assets::Kind::Texture)
        .copied()
        .collect();
    assert!(!visible_texture_requests.is_empty(), "the opaque face requests its texture");
    let mesh_assets = std::collections::HashMap::from([(asset, bytes)]);
    complete_mesh_requests(&world, &mut scene, &mut view, &mesh_assets, requests);
    for request in visible_texture_requests {
        scene.complete_asset(request.ticket, prepared_asset(request.request, test_support::texture::RGB_J2C.to_vec()));
    }
    let _requests = settle_scene(&world, &mut scene, &mut view, Instant::now());

    let object = view.objects().next().expect("mesh object");
    let entity = object.detail().summary.entity;
    let materials = object.materials().expect("mesh face materials");
    let opaque = materials.iter().find(|face| face.slot == 0).expect("opaque face material");
    let invisible = materials.iter().find(|face| face.slot == 1).expect("invisible face material");
    assert_eq!(opaque.alpha, crate::material::AlphaBehavior::Opaque);
    assert_eq!(invisible.alpha, crate::material::AlphaBehavior::Blend);
    assert_eq!(invisible.base_color_factor()[3], 0.0);
    assert!(matches!(
        invisible.base_color_texture.as_ref().expect("invisible face texture").state().resolution,
        crate::material::Resolution::Pending
    ));
    let (_, shared) = object.mesh().expect("uploaded mesh geometry");
    assert_eq!(shared.mesh().faces().len(), 2, "both authored face slots remain in the selected LOD");

    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    let depth = geometry.depth(&device, [128, 128]).expect("depth target");
    let started = Instant::now();
    prepare_frame_at(&device, &queue, &mut geometry, &view, camera, started);
    assert!(geometry.meshes.contains_key(&shared.id()), "the two-face LOD has been uploaded to the GPU");
    assert_eq!(geometry.stats.instances, 0, "the opaque texture must reach GPU residency before it can appear");
    let job = geometry.next_texture_job().expect("opaque face texture upload job");
    geometry.complete_texture(job.run(crate::texture::CompressionQuality::Fast));
    let early = started + std::time::Duration::from_millis(10);
    for _ in 0..3 {
        prepare_frame_at(&device, &queue, &mut geometry, &view, camera, early);
        if geometry.stats.instances == 1 {
            break;
        }
    }
    assert_eq!(geometry.stats.instances, 1, "the opaque face appears once its own texture is resident, before 250 ms");
    let opaque_image = opaque.base_color_texture.as_ref().and_then(|binding| binding.image()).expect("resident opaque texture");
    assert!(geometry.textures.is_resident(opaque_image.id()));
    assert!(
        invisible.base_color_texture.as_ref().is_some_and(|binding| binding.image().is_none()),
        "the invisible face texture remains pending"
    );

    let item = geometry.cache.items.get(geometry.cache.objects[&entity]).expect("visible mesh item");
    assert_eq!(item.faces.iter().map(|face| face.order.3).collect::<Vec<_>>(), [0]);
    assert_eq!(geometry.draw_list.draws().len(), 1, "only the opaque face has a draw command");
    assert_eq!(geometry.draw_list.draws()[0].indices, shared.mesh().faces()[0].indices);
    assert!(geometry.cache.visible.contains(&geometry.cache.objects[&entity]), "the whole mesh object became visible");

    let local_center = |face_index: usize| {
        let mesh = shared.mesh();
        let face = mesh.faces().get(face_index).expect("fixture face slot");
        let start = usize::try_from(face.indices.start).expect("fixture face index start");
        let end = usize::try_from(face.indices.end).expect("fixture face index end");
        let indices = mesh.indices().get(start..end).expect("fixture face index range");
        let mut center = Vec3::ZERO;
        for index in indices {
            center += mesh.vertices().get(*index as usize).expect("fixture face vertex").position;
        }
        center / indices.len() as f32
    };
    let ray_to_face = |center: Vec3| {
        let target = item.position + (item.rotation * (item.scale * center)).as_dvec3();
        (target - camera.position()).normalize()
    };
    assert!(geometry.pick(camera, ray_to_face(local_center(0))).unwrap().is_some(), "the opaque face remains pickable");
    assert_eq!(geometry.pick(camera, ray_to_face(local_center(1))).unwrap(), None, "the invisible face cannot be picked");

    let pixels = render_pixels(&device, &queue, [128, 128], &depth, |pass| geometry.draw(pass));
    let red_pixels = pixels.iter().filter(|pixel| pixel[0] > pixel[1].saturating_add(24)).count();
    assert!(red_pixels > 50, "the opaque face is rendered while the hidden texture is still pending: {red_pixels} pixels");

    let late = started + crate::geometry::first_appearance::ASSET_WAIT + std::time::Duration::from_millis(1);
    prepare_frame_at(&device, &queue, &mut geometry, &view, camera, late);
    assert_eq!(geometry.stats.instances, 1, "the visible object remains available after the optional asset deadline");
}

#[test]
#[ignore = "requires a Vulkan adapter"]
fn gpu_ready_texture_wakes_appearance() {
    let asset = AssetId::new(Uuid::from_u128(862));
    let texture = AssetId::new(Uuid::from_u128(863));
    let data = test_support::mesh::asset_with_lod_bounds(3, [-0.5; 3], [0.5; 3]);
    let (world, mut scene, mut view, camera, requests) = mesh_scene(&[(1, asset, Vec3::new(2.0, 0.0, 0.0))], Some((texture, texture)));
    let texture_requests: Vec<_> =
        requests.iter().filter(|request| request.request.resource().kind == assets::Kind::Texture).copied().collect();
    assert!(!texture_requests.is_empty());
    complete_mesh_requests(&world, &mut scene, &mut view, &std::collections::HashMap::from([(asset, data)]), requests);
    for request in texture_requests {
        scene.complete_asset(request.ticket, prepared_asset(request.request, test_support::texture::GRAY_4096_J2C.to_vec()));
    }
    let _requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    let entity = view.objects().next().expect("gated object").detail().summary.entity;
    geometry.request_frame_textures(&view, &[entity], &[entity]);
    let job = geometry.next_texture_job().expect("gated visible demand must override the shared background demand");
    let now = Instant::now();
    prepare_frame_at(&device, &queue, &mut geometry, &view, camera, now);
    assert_eq!(geometry.stats.instances, 0);
    geometry.complete_texture(job.run(crate::texture::CompressionQuality::Fast));
    let moved = Camera::new(camera.position(), Vec3::new(1.0, 0.01, 0.0), camera.vertical_fov(), [128, 128]).unwrap();
    let early = now + std::time::Duration::from_millis(1);
    let frame = geometry.admit().unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare_at(&device, &mut encoder, &view, None, moved, [128, 128], crate::FrameInput::default(), early);
    assert_eq!(geometry.stats.instances, 0, "residency is established after this frame's visibility decision");
    assert!(geometry.needs_frame(early), "new texture residency schedules eligibility before the timeout");
    queue.submit([encoder.finish()]);
    geometry.submitted(frame);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    prepare_frame_at(&device, &queue, &mut geometry, &view, moved, early);
    assert_eq!(geometry.stats.instances, 1, "ready assets release the gate before 250 ms");
}

#[test]
#[ignore = "requires a Vulkan adapter"]
fn gpu_first_appearance_wait_is_bounded() {
    let asset = AssetId::new(Uuid::from_u128(860));
    let texture = AssetId::new(Uuid::from_u128(861));
    let data = test_support::mesh::asset_with_lod_bounds(3, [-0.5; 3], [0.5; 3]);
    let (mut world, mut scene, mut view, camera, requests) = mesh_scene(&[(1, asset, Vec3::new(2.0, 0.0, 0.0))], Some((texture, texture)));
    complete_mesh_requests(&world, &mut scene, &mut view, &std::collections::HashMap::from([(asset, data)]), requests);
    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    let now = Instant::now();
    let wait = crate::geometry::first_appearance::ASSET_WAIT;
    let away = Camera::new(camera.position(), -camera.direction(), camera.vertical_fov(), [128, 128]).unwrap();
    for (elapsed, current, expected) in [
        (std::time::Duration::ZERO, camera, 0),
        (wait / 2, away, 0),
        (wait - std::time::Duration::from_nanos(1), camera, 0),
        (wait, camera, 1),
    ] {
        let frame = geometry.admit().unwrap();
        let mut encoder = device.create_command_encoder(&Default::default());
        geometry.prepare_at(&device, &mut encoder, &view, None, current, [128, 128], crate::FrameInput::default(), now + elapsed);
        queue.submit([encoder.finish()]);
        geometry.submitted(frame);
        device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
        assert_eq!(geometry.stats.instances, expected, "appearance at {elapsed:?}");
    }
    assert_eq!(geometry.meshes.len(), 1, "complete geometry remains resident throughout the optional texture wait");
    assert_eq!(geometry.redraw_deadline(), None, "fallback appearance settles without an idle redraw loop");
    let entity = view.objects().next().unwrap().detail().summary.entity;
    let key = crate::geometry::first_appearance::AppearanceKey { entity, asset };
    assert!(geometry.first_appearances.shown(key));
    let _removed = world
        .remove_object(UpdateSource::new(Region::new(1), 2, OrderEvidence::Ordered(2), 0), std::num::NonZeroU32::new(1).unwrap())
        .unwrap();
    scene.notify_world_change();
    let _requests = settle_scene(&world, &mut scene, &mut view, now + wait);
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    assert!(!geometry.first_appearances.shown(key), "production scene cleanup forgets removed appearance history");
}

#[test]
#[ignore = "requires a Vulkan adapter"]
fn gpu_blended_rotation_uploads_instances() {
    let (device, queue) = vulkan_device();
    let (_scene, view, _) = scene(128);
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.instances = device.create_buffer(&BufferDescriptor {
        label: None,
        size: crate::geometry::INSTANCE_BUFFER_BYTES as u64,
        usage: BufferUsages::VERTEX | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let camera = |x| Camera::new(DVec3::new(x, -30.0, 0.0), Vec3::Y, 1.0, [128, 128]).unwrap();
    prepare_frame(&device, &queue, &mut geometry, &view, camera(0.0));
    assert!(geometry.draw_list.has_blended());
    let instances = geometry.stats.instances;
    prepare_frame(&device, &queue, &mut geometry, &view, camera(26.0));
    assert_eq!(geometry.stats.instances, instances);
    assert_eq!(geometry.stats.layouts, 0);
    assert_eq!(geometry.stats.blended_sorts, 1);
    let slots = geometry.draw_list.draws().iter().map(|draw| draw.first + draw.count).max().unwrap();
    let bytes = u64::from(slots) * crate::pipeline::INSTANCE_BYTES as u64;
    let reference = device.create_buffer(&BufferDescriptor {
        label: None,
        size: bytes,
        usage: BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    geometry.draw_list.upload_layout(&queue, &reference);
    let readback = device.create_buffer(&BufferDescriptor {
        label: None,
        size: bytes * 2,
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(&geometry.instances, 0, &readback, 0, bytes);
    encoder.copy_buffer_to_buffer(&reference, 0, &readback, bytes, bytes);
    queue.submit([encoder.finish()]);
    let (sent, received) = std::sync::mpsc::channel();
    readback.slice(..).map_async(MapMode::Read, move |result| sent.send(result).unwrap());
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    received.recv().unwrap().unwrap();
    let mapped = readback.get_mapped_range(..).unwrap();
    let (actual, expected) = mapped.split_at(bytes as usize);
    assert!(actual == expected, "camera sorting must upload the newly ordered transforms and materials");
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo test -p render gpu_warmed_rotation_preserves_triangles -- --ignored"]
fn gpu_warmed_rotation_preserves_triangles() {
    let (device, queue, indirect) = block_on(async {
        let instance =
            wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let indirect = adapter.features() & wgpu::Features::INDIRECT_FIRST_INSTANCE;
        let (device, queue) =
            adapter.request_device(&DeviceDescriptor { required_features: indirect, ..Default::default() }).await.unwrap();
        (device, queue, indirect.contains(wgpu::Features::INDIRECT_FIRST_INSTANCE))
    });
    let scope = device.push_error_scope(ErrorFilter::Validation);
    let a = AssetId::new(Uuid::from_u128(850));
    let b = AssetId::new(Uuid::from_u128(851));
    let c = AssetId::new(Uuid::from_u128(852));
    let point = |degrees: f32| {
        let radians = degrees.to_radians();
        Vec3::new(3.0 * radians.cos(), 3.0 * radians.sin(), 0.0)
    };
    let assets = std::collections::HashMap::from([
        (a, test_support::mesh::asset_with_lod_bounds(3, [-0.01, -0.8, -0.5], [0.01, 0.1, 0.5])),
        (b, test_support::mesh::asset_with_lod_bounds(3, [-0.01, -0.4, -0.4], [0.01, 0.4, 0.4])),
        (c, test_support::mesh::asset_with_lod_bounds(3, [-0.01, -0.5, -0.8], [0.01, 0.5, 0.2])),
    ]);
    let (world, mut scene, mut view, _, mut requests) = mesh_scene(&[(1, a, point(0.0)), (2, b, point(31.0)), (3, c, point(-15.0))], None);
    let load_camera = Camera::new(DVec3::ZERO, Vec3::X, 120_f64.to_radians(), [128, 128])
        .expect("wide loading camera")
        .with_quality(512.0, 4.0)
        .expect("high detail loading camera");
    let away_camera =
        Camera::new(DVec3::ZERO, Vec3::new(5_f32.to_radians().cos(), -5_f32.to_radians().sin(), 0.0), 40_f64.to_radians(), [128, 128])
            .expect("away camera");
    let back_camera =
        Camera::new(DVec3::ZERO, Vec3::new(5_f32.to_radians().cos(), 5_f32.to_radians().sin(), 0.0), 40_f64.to_radians(), [128, 128])
            .expect("back camera");
    scene.set_camera(load_camera);
    requests.extend(settle_scene(&world, &mut scene, &mut view, Instant::now()));
    complete_mesh_requests(&world, &mut scene, &mut view, &assets, requests);
    let entities: Vec<_> = view.objects().map(|object| object.detail().summary.entity).collect();
    assert_eq!(entities.len(), 3);
    let a_entity =
        view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_001)).expect("asset A").detail().summary.entity;
    let b_entity =
        view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_002)).expect("asset B").detail().summary.entity;
    let c_entity =
        view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_003)).expect("asset C").detail().summary.entity;
    for (entity, asset) in [(a_entity, a), (b_entity, b), (c_entity, c)] {
        assert_eq!(
            view.object(entity).and_then(|object| object.mesh().map(|(key, _)| key)),
            Some(DrawnMeshKey::Asset { asset, lod: assets::mesh::MeshLod::High }),
            "all assets are fully loaded at high LOD before camera rotation"
        );
    }

    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    assert_eq!(geometry.indirect_first_instance, indirect);
    let depth = geometry.depth(&device, [128, 128]).expect("depth target");
    prepare_frame(&device, &queue, &mut geometry, &view, load_camera);
    let all_visible = std::collections::HashSet::from([a_entity, b_entity, c_entity]);
    let actual_visible: std::collections::HashSet<_> =
        geometry.cache.visible.iter().filter_map(|index| geometry.cache.items.get(*index).and_then(|item| item.entity)).collect();
    assert_eq!(actual_visible, all_visible, "all meshes are resident and drawn before movement");
    assert_eq!(geometry.meshes.len(), 3);
    assert!(geometry.stats.uploaded_bytes > 0, "the warm-up frame uploads the complete mesh set");
    let ids = [a_entity, b_entity, c_entity].map(|entity| geometry.cache.items.get(geometry.cache.objects[&entity]).unwrap().mesh.id());
    let ranges = ids.map(|id| {
        let allocation = &geometry.meshes[&id].allocation;
        (allocation.vertices(), allocation.indices())
    });
    let page = Arc::as_ptr(geometry.meshes[&ids[0]].allocation.buffers()) as usize;
    assert!(ids.iter().all(|id| Arc::as_ptr(geometry.meshes[id].allocation.buffers()) as usize == page));
    let memory = geometry.memory.mesh_state();
    assert_eq!(geometry.draw_list.draw_batches().len(), 1, "the warm meshes use one shared opaque page batch");
    assert_eq!(geometry.draw_list.draw_batches()[0].count, 3);
    assert_eq!(geometry.stats.draw_commands, 3);
    assert_eq!(geometry.stats.draw_calls, if indirect { 1 } else { 3 });
    assert_visible_assets_match_source(&device, &queue, &geometry, &view, load_camera, &depth, &all_visible, "fully warm scene");

    scene.set_camera(away_camera);
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    complete_mesh_requests(&world, &mut scene, &mut view, &assets, requests);
    prepare_frame(&device, &queue, &mut geometry, &view, away_camera);
    let away_visible = std::collections::HashSet::from([a_entity, c_entity]);
    let actual_visible: std::collections::HashSet<_> =
        geometry.cache.visible.iter().filter_map(|index| geometry.cache.items.get(*index).and_then(|item| item.entity)).collect();
    assert_eq!(actual_visible, away_visible, "the small turn culls the resident edge mesh");
    assert_eq!(geometry.stats.layouts, 1, "visibility reentry rebuilds the draw layout");
    assert_eq!(geometry.draw_list.draw_batches().len(), 1);
    assert_eq!(geometry.draw_list.draw_batches()[0].count, 2, "the opaque page batch drops the hidden command");
    assert_eq!(geometry.stats.draw_commands, 2);
    assert_eq!(geometry.stats.draw_calls, if indirect { 1 } else { 2 });
    assert_eq!(geometry.stats.uploaded_bytes, 0);
    assert_eq!(geometry.stats.pending_meshes, 0);
    assert_eq!(geometry.stats.mesh_budget_blocked, 0);
    let away_memory = geometry.memory.mesh_state();
    assert_eq!(away_memory.bytes, memory.bytes, "camera-only visibility changes allocate no page memory");
    assert_eq!(away_memory.revision, memory.revision, "camera-only visibility changes do not retire a mesh");
    assert_eq!(geometry.meshes.len(), 3, "all GPU meshes remain resident while visibility changes");
    assert!(geometry.retired.is_empty(), "camera-only visibility changes do not evict a mesh");
    for (id, (vertices, indices)) in ids.iter().zip(ranges.iter()) {
        let allocation = &geometry.meshes[id].allocation;
        assert_eq!((allocation.vertices(), allocation.indices()), (vertices.clone(), indices.clone()));
        assert_eq!(Arc::as_ptr(allocation.buffers()) as usize, page);
    }
    assert_visible_assets_match_source(&device, &queue, &geometry, &view, away_camera, &depth, &away_visible, "resident edge hidden");

    scene.set_camera(back_camera);
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    complete_mesh_requests(&world, &mut scene, &mut view, &assets, requests);
    prepare_frame(&device, &queue, &mut geometry, &view, back_camera);
    let back_visible = all_visible;
    let actual_visible: std::collections::HashSet<_> =
        geometry.cache.visible.iter().filter_map(|index| geometry.cache.items.get(*index).and_then(|item| item.entity)).collect();
    assert_eq!(actual_visible, back_visible, "turning back makes the same resident mesh visible again");
    assert_eq!(geometry.stats.layouts, 1);
    assert_eq!(geometry.draw_list.draw_batches().len(), 1);
    assert_eq!(geometry.draw_list.draw_batches()[0].count, 3, "the reentered command returns to the page batch");
    assert_eq!(geometry.stats.draw_commands, 3);
    assert_eq!(geometry.stats.draw_calls, if indirect { 1 } else { 3 });
    assert_eq!(geometry.stats.uploaded_bytes, 0);
    assert_eq!(geometry.stats.pending_meshes, 0);
    assert_eq!(geometry.stats.mesh_budget_blocked, 0);
    let back_memory = geometry.memory.mesh_state();
    assert_eq!(back_memory.bytes, memory.bytes);
    assert_eq!(back_memory.revision, memory.revision);
    assert_eq!(geometry.meshes.len(), 3);
    assert!(geometry.retired.is_empty());
    for (id, (vertices, indices)) in ids.iter().zip(ranges.iter()) {
        let allocation = &geometry.meshes[id].allocation;
        assert_eq!((allocation.vertices(), allocation.indices()), (vertices.clone(), indices.clone()));
        assert_eq!(Arc::as_ptr(allocation.buffers()) as usize, page);
    }
    assert_visible_assets_match_source(&device, &queue, &geometry, &view, back_camera, &depth, &back_visible, "resident edge reentered");

    let memory = geometry.memory.clone();
    drop(geometry);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    assert_eq!(memory.mesh_state().bytes, 0);
    assert!(block_on(scope.pop()).is_none(), "warmed page batching and camera reentry satisfy GPU validation");
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo nextest run -p render --run-ignored only -E 'test(gpu_side_jitter_keeps_layout)'"]
fn gpu_side_jitter_keeps_layout() {
    use crate::geometry::visibility::{VisibilityBands, WorldSphere};

    const SIZE: [u32; 2] = [128, 128];
    let (_scene, view, _) = scene(255);
    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    geometry.update_cache(&view);
    let target = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(1)).expect("edge target");
    let target_entity = target.detail().summary.entity;
    let target_index = geometry.cache.objects[&target_entity];
    let other = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(2)).expect("hidden object");
    let other_index = geometry.cache.objects[&other.detail().summary.entity];
    geometry.cache.items.get_mut(other_index).unwrap().position = DVec3::new(10.0, 100.0, 0.0);
    let item = geometry.cache.items.get(target_index).expect("cached edge target");
    let (position, rotation, scale, bounds, shading) = (item.position, item.rotation, item.scale, item.info.bounds, item.shading);
    let mut last_strict = None;
    let mut edge = None;
    for step in 0..3000 {
        let y = f64::from(step) * 0.01;
        let camera = Camera::new(DVec3::new(0.0, y, 0.0), Vec3::X, 1.0, SIZE).expect("edge camera");
        let bands = VisibilityBands::new(crate::pipeline::projection(camera, SIZE), SIZE);
        let sphere = WorldSphere::new(position, rotation, scale, bounds, camera).expect("finite target bounds");
        if sphere.visible(shading, camera, &bands.strict) {
            last_strict = Some(camera);
        } else if let Some(visible_camera) = last_strict
            && sphere.visible(shading, camera, &bands.retained)
        {
            edge = Some((visible_camera, camera));
            break;
        }
    }
    let (strict_camera, exited_camera) = edge.expect("sphere crosses the side plane inside the retention band");

    prepare_frame(&device, &queue, &mut geometry, &view, strict_camera);
    assert!(geometry.cache.candidates.contains(&target_index));
    assert!(geometry.cache.visible.contains(&target_index));
    assert_eq!(geometry.stats.instances, 1);

    for camera in [exited_camera, strict_camera, exited_camera] {
        prepare_frame(&device, &queue, &mut geometry, &view, camera);
        assert!(geometry.cache.candidates.contains(&target_index), "the prior candidate stays admitted through side-plane jitter");
        assert!(geometry.cache.visible.contains(&target_index));
        assert_eq!(geometry.stats.layouts, 0, "membership and draw layout stay stable across the margin");
        assert_eq!(geometry.stats.instances, 1);
    }
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo nextest run -p render --run-ignored only -E 'test(gpu_prep_resident_before_entry)'"]
fn gpu_prep_resident_before_entry() {
    use crate::geometry::first_appearance::AppearanceKey;

    let asset = AssetId::new(Uuid::from_u128(9201));
    let bytes = test_support::mesh::asset(&[3], 1);
    let (_world, _scene, view, camera) = ready_mesh_scene(&[(1, asset, Vec3::new(10.0, 0.0, 0.0))], bytes);
    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    geometry.update_cache(&view);
    let object = view.objects().next().expect("target object");
    let entity = object.detail().summary.entity;
    let index = geometry.cache.objects[&entity];
    geometry.cache.items.get_mut(index).expect("cached target").position = DVec3::new(10.0, -9.0, 0.0);
    let mesh_id = geometry.cache.items.get(index).expect("cached target").mesh.id();

    prepare_frame(&device, &queue, &mut geometry, &view, camera);

    assert!(geometry.cache.preparation_candidates.contains(&index));
    assert!(!geometry.cache.candidates.contains(&index));
    assert!(!geometry.cache.visible.contains(&index));
    assert!(geometry.cache.preparation_mesh_ids.contains(&mesh_id));
    assert!(!geometry.cache.mesh_ids.contains(&mesh_id));
    assert!(geometry.meshes.contains_key(&mesh_id));
    assert!(!geometry.cache.mesh_ready.contains(&mesh_id));
    assert_eq!(geometry.stats.instances, 0);
    assert_eq!(geometry.stats.draw_commands, 0);
    assert_eq!(geometry.pick(camera, DVec3::new(10.0, -9.0, 0.0).normalize()).expect("prep-only pick"), None);
    assert!(!geometry.first_appearances.shown(AppearanceKey { entity, asset }));
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo nextest run -p render --run-ignored only -E 'test(gpu_pending_priority_tracks_camera)'"]
fn gpu_pending_priority_tracks_camera() {
    let first_asset = AssetId::new(Uuid::from_u128(9301));
    let second_asset = AssetId::new(Uuid::from_u128(9302));
    let bytes = test_support::mesh::asset(&[3], 1);
    let (world, mut scene, mut view, _, mut requests) =
        mesh_scene(&[(1, first_asset, Vec3::new(10.0, 100.0, 0.0)), (2, second_asset, Vec3::new(10.0, 108.0, 0.0))], None);
    let load_camera = Camera::new(DVec3::new(0.0, 104.0, 0.0), Vec3::X, std::f64::consts::FRAC_PI_3, [128, 128]).unwrap();
    scene.set_camera(load_camera);
    requests.extend(settle_scene(&world, &mut scene, &mut view, Instant::now()));
    let assets = std::collections::HashMap::from([(first_asset, bytes.clone()), (second_asset, bytes)]);
    complete_mesh_requests(&world, &mut scene, &mut view, &assets, requests);
    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    geometry.update_cache(&view);
    let first_entity =
        view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_001)).unwrap().detail().summary.entity;
    let second_entity =
        view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_002)).unwrap().detail().summary.entity;
    let first_mesh = geometry.cache.items.get(geometry.cache.objects[&first_entity]).unwrap().mesh.id();
    let second_mesh = geometry.cache.items.get(geometry.cache.objects[&second_entity]).unwrap().mesh.id();
    assert_ne!(first_mesh, second_mesh);
    let blocker = geometry.memory.reserve(crate::geometry::MESH_BYTES, crate::geometry::Resource::Mesh).expect("fill mesh budget");
    let left_camera = Camera::new(DVec3::new(0.0, 102.0, 0.0), Vec3::X, std::f64::consts::FRAC_PI_3, [128, 128]).unwrap();
    let right_camera = Camera::new(DVec3::new(0.0, 106.0, 0.0), Vec3::X, std::f64::consts::FRAC_PI_3, [128, 128]).unwrap();

    prepare_frame(&device, &queue, &mut geometry, &view, left_camera);
    assert_eq!(geometry.cache.mesh_ids, vec![first_mesh, second_mesh]);
    assert_eq!(geometry.stats.pending_meshes, 2);
    prepare_frame(&device, &queue, &mut geometry, &view, left_camera);
    assert!(!geometry.needs_frame(Instant::now()), "a stalled visible upload stops requesting frames without progress");
    let candidates = geometry.cache.candidates.clone();

    prepare_frame(&device, &queue, &mut geometry, &view, right_camera);

    assert_eq!(geometry.cache.candidates, candidates, "both draw memberships stay unchanged");
    assert_eq!(geometry.cache.mesh_ids, vec![second_mesh, first_mesh], "pending order follows the moved camera");
    assert_eq!(geometry.stats.pending_meshes, 2);
    assert_eq!(geometry.stats.layouts, 0);
    assert!(!geometry.needs_frame(Instant::now()));
    drop(blocker);
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo nextest run -p render --run-ignored only -E 'test(gpu_prep_pressure_keeps_residents)'"]
fn gpu_prep_pressure_keeps_residents() {
    use crate::geometry::first_appearance::AppearanceKey;

    let display_asset = AssetId::new(Uuid::from_u128(9401));
    let hidden_asset = AssetId::new(Uuid::from_u128(9402));
    let target_asset = AssetId::new(Uuid::from_u128(9403));
    let bytes = test_support::mesh::asset_with_lod_geometry(&[(3, 1, 20_000, 20_000)]);
    let (_world, _scene, view, camera) = ready_mesh_scene(
        &[
            (1, display_asset, Vec3::new(10.0, 0.0, 0.0)),
            (2, hidden_asset, Vec3::new(10.0, 3.0, 0.0)),
            (3, target_asset, Vec3::new(10.0, 0.0, 0.0)),
        ],
        bytes,
    );
    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    geometry.update_cache(&view);
    let display = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_001)).unwrap();
    let hidden = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_002)).unwrap();
    let target = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_003)).unwrap();
    let display_index = geometry.cache.objects[&display.detail().summary.entity];
    let hidden_index = geometry.cache.objects[&hidden.detail().summary.entity];
    let target_index = geometry.cache.objects[&target.detail().summary.entity];
    let display_id = geometry.cache.items.get(display_index).unwrap().mesh.id();
    let hidden_id = geometry.cache.items.get(hidden_index).unwrap().mesh.id();
    let target_id = geometry.cache.items.get(target_index).unwrap().mesh.id();
    geometry.cache.items.get_mut(target_index).unwrap().position = DVec3::new(10.0, -30.0, 0.0);
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    assert!(geometry.meshes.contains_key(&display_id));
    assert!(geometry.meshes.contains_key(&hidden_id));
    assert!(!geometry.meshes.contains_key(&target_id));

    geometry.cache.items.get_mut(hidden_index).unwrap().position = DVec3::new(10.0, 30.0, 0.0);
    geometry.cache.items.get_mut(target_index).unwrap().position = DVec3::new(10.0, -9.0, 0.0);
    let remaining = crate::geometry::MESH_BYTES - geometry.memory.mesh_state().bytes;
    let blocker = geometry.memory.reserve(remaining, crate::geometry::Resource::Mesh).expect("fill mesh budget");
    let moved_camera = Camera::new(DVec3::new(0.0, 0.1, 0.0), Vec3::X, std::f64::consts::FRAC_PI_3, [128, 128]).unwrap();

    prepare_frame(&device, &queue, &mut geometry, &view, moved_camera);

    assert!(geometry.cache.preparation_candidates.contains(&target_index));
    assert!(!geometry.cache.candidates.contains(&target_index));
    assert!(geometry.cache.preparation_mesh_ids.contains(&target_id));
    assert!(!geometry.cache.mesh_ids.contains(&target_id));
    assert!(geometry.meshes.contains_key(&display_id), "the displayed mesh stays resident");
    assert!(geometry.meshes.contains_key(&hidden_id), "preparation pressure does not evict an unused mesh");
    assert!(!geometry.meshes.contains_key(&target_id));
    assert!(geometry.retired.is_empty());
    assert_eq!(geometry.stats.instances, 1);
    assert_eq!(geometry.stats.pending_meshes, 0);
    assert_eq!(geometry.stats.mesh_bytes, crate::geometry::MESH_BYTES);
    assert!(geometry.needs_frame(Instant::now()), "the changed memory state permits one follow-up");

    prepare_frame(&device, &queue, &mut geometry, &view, moved_camera);
    assert!(!geometry.needs_frame(Instant::now()), "a capacity-blocked prep does not keep waking the renderer");
    assert!(geometry.meshes.contains_key(&hidden_id));

    drop(blocker);
    assert!(geometry.needs_frame(Instant::now()), "released memory schedules preparation again");
    prepare_frame(&device, &queue, &mut geometry, &view, moved_camera);
    assert!(geometry.meshes.contains_key(&target_id));
    assert_eq!(geometry.stats.instances, 1);
    assert!(!geometry.first_appearances.shown(AppearanceKey { entity: target.detail().summary.entity, asset: target_asset }));
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo nextest run -p render --run-ignored only -E 'test(gpu_prep_cleanup_on_lifecycle)'"]
fn gpu_prep_cleanup_on_lifecycle() {
    let first_asset = AssetId::new(Uuid::from_u128(9501));
    let second_asset = AssetId::new(Uuid::from_u128(9502));
    let bytes = test_support::mesh::asset(&[3], 1);
    let (mut world, mut scene, mut view, camera) =
        ready_mesh_scene(&[(1, first_asset, Vec3::new(10.0, 0.0, 0.0)), (2, second_asset, Vec3::new(10.0, 0.0, 0.0))], bytes);
    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    geometry.update_cache(&view);
    let target = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_002)).unwrap();
    let target_entity = target.detail().summary.entity;
    let target_index = geometry.cache.objects[&target_entity];
    let target_mesh = geometry.cache.items.get(target_index).unwrap().mesh.id();
    geometry.cache.items.get_mut(target_index).unwrap().position = DVec3::new(10.0, -9.0, 0.0);
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    assert!(geometry.cache.preparation_mesh_ids.contains(&target_mesh));

    let _ = world
        .remove_object(UpdateSource::new(Region::new(1), 10, OrderEvidence::Ordered(10), 0), std::num::NonZeroU32::new(2).unwrap())
        .expect("target removal applies");
    scene.notify_world_change();
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    assert!(!geometry.cache.objects.contains_key(&target_entity));
    assert!(!geometry.cache.preparation_candidates.contains(&target_index));
    assert!(!geometry.cache.preparation_mesh_ids.contains(&target_mesh));

    let remaining = view.objects().next().expect("remaining region object");
    let remaining_entity = remaining.detail().summary.entity;
    let remaining_index = geometry.cache.objects[&remaining_entity];
    let remaining_mesh = geometry.cache.items.get(remaining_index).unwrap().mesh.id();
    geometry.cache.items.get_mut(remaining_index).unwrap().position = DVec3::new(10.0, -9.0, 0.0);
    let moved_camera = Camera::new(DVec3::new(0.0, 0.1, 0.0), Vec3::X, std::f64::consts::FRAC_PI_3, [128, 128]).unwrap();
    prepare_frame(&device, &queue, &mut geometry, &view, moved_camera);
    assert!(geometry.cache.candidates.is_empty());
    assert!(geometry.cache.preparation_candidates.contains(&remaining_index));
    assert!(geometry.cache.preparation_mesh_ids.contains(&remaining_mesh));

    let _ = world.retire_region(Region::new(1)).expect("region retirement applies");
    scene.notify_world_change();
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());
    prepare_frame(&device, &queue, &mut geometry, &view, moved_camera);
    assert!(geometry.cache.items.iter().next().is_none());
    assert!(geometry.cache.objects.is_empty());
    assert!(geometry.cache.candidates.is_empty());
    assert!(geometry.cache.preparation_candidates.is_empty());
    assert!(geometry.cache.preparation_replacements.is_empty());
    assert!(geometry.cache.mesh_ids.is_empty());
    assert!(geometry.cache.preparation_mesh_ids.is_empty());
    assert_eq!(geometry.stats.instances, 0);
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo nextest run -p render --run-ignored only -E 'test(gpu_large_prep_respects_upload_limit)'"]
fn gpu_large_prep_respects_upload_limit() {
    let asset = AssetId::new(Uuid::from_u128(9601));
    let bytes = test_support::mesh::asset_with_lod_geometry(&[(3, 3, 65_535, 65_535)]);
    let (_world, _scene, view, camera) = ready_mesh_scene(&[(1, asset, Vec3::new(10.0, 0.0, 0.0))], bytes);
    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    geometry.update_cache(&view);
    let object = view.objects().next().expect("target object");
    let entity = object.detail().summary.entity;
    let index = geometry.cache.objects[&entity];
    geometry.cache.items.get_mut(index).unwrap().position = DVec3::new(10.0, -9.0, 0.0);
    let mesh_id = geometry.cache.items.get(index).unwrap().mesh.id();

    prepare_frame(&device, &queue, &mut geometry, &view, camera);

    assert!(geometry.cache.preparation_mesh_ids.contains(&mesh_id));
    assert!(!geometry.cache.mesh_ids.contains(&mesh_id));
    assert!(!geometry.meshes.contains_key(&mesh_id), "preparation cannot use the large first-upload exception");
    assert_eq!(geometry.stats.uploaded_bytes, 0);
    assert_eq!(geometry.stats.pending_meshes, 0);
    assert!(!geometry.needs_frame(Instant::now()), "an over-budget prep does not schedule an empty retry");
}

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo nextest run -p render --run-ignored only -E 'test(gpu_pending_visible_mesh_precedes_prep)'"]
fn gpu_pending_visible_mesh_precedes_prep() {
    let visible_asset = AssetId::new(Uuid::from_u128(9701));
    let preparation_asset = AssetId::new(Uuid::from_u128(9702));
    let visible_bytes = test_support::mesh::asset_with_lod_geometry(&[(3, 3, 65_535, 65_535)]);
    let preparation_bytes = test_support::mesh::asset(&[3], 1);
    let (world, mut scene, mut view, camera, requests) =
        mesh_scene(&[(1, visible_asset, Vec3::new(10.0, 0.0, 0.0)), (2, preparation_asset, Vec3::new(10.0, 0.0, 0.0))], None);
    let assets = std::collections::HashMap::from([(visible_asset, visible_bytes), (preparation_asset, preparation_bytes)]);
    complete_mesh_requests(&world, &mut scene, &mut view, &assets, requests);

    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    geometry.memory = Arc::new(crate::geometry::Accounting::default());
    geometry.update_cache(&view);
    let visible_entity =
        view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_001)).unwrap().detail().summary.entity;
    let preparation_entity =
        view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_002)).unwrap().detail().summary.entity;
    let visible_index = geometry.cache.objects[&visible_entity];
    let preparation_index = geometry.cache.objects[&preparation_entity];
    geometry.cache.items.get_mut(preparation_index).unwrap().position = DVec3::new(10.0, -9.0, 0.0);
    let visible_mesh = geometry.cache.items.get(visible_index).unwrap().mesh.id();
    let preparation_mesh = geometry.cache.items.get(preparation_index).unwrap().mesh.id();
    let preparation_page = view.object(preparation_entity).unwrap().mesh().unwrap().1.clone();
    let available = mesh_page_bytes(&preparation_page);
    let blocker = geometry
        .memory
        .reserve(crate::geometry::MESH_BYTES - available, crate::geometry::Resource::Mesh)
        .expect("leave one preparation page available");

    prepare_frame(&device, &queue, &mut geometry, &view, camera);

    assert!(geometry.cache.candidates.contains(&visible_index));
    assert!(!geometry.cache.candidates.contains(&preparation_index));
    assert!(geometry.cache.preparation_candidates.contains(&preparation_index));
    assert_eq!(geometry.cache.mesh_ids, vec![visible_mesh]);
    assert!(geometry.cache.preparation_mesh_ids.contains(&preparation_mesh));
    assert_eq!(geometry.stats.pending_meshes, 1);
    assert!(!geometry.meshes.contains_key(&visible_mesh));
    assert!(!geometry.meshes.contains_key(&preparation_mesh), "available prep capacity stays behind pending visible demand");
    assert_eq!(geometry.stats.uploaded_bytes, 0);
    assert_eq!(geometry.stats.mesh_bytes, crate::geometry::MESH_BYTES - available);
    assert!(geometry.needs_frame(Instant::now()), "the memory change schedules one follow-up");
    prepare_frame(&device, &queue, &mut geometry, &view, camera);
    assert!(!geometry.meshes.contains_key(&preparation_mesh));
    assert!(!geometry.needs_frame(Instant::now()), "blocked visible demand and deferred preparation do not spin");
    drop(blocker);
}

#[test]
#[ignore = "requires a Vulkan adapter"]
fn gpu_visible_upload_wins() {
    let mesh_asset = AssetId::new(Uuid::from_u128(9801));
    let old_texture = AssetId::new(Uuid::from_u128(9802));
    let new_texture = AssetId::new(Uuid::from_u128(9803));
    let mesh = test_support::mesh::asset_with_lod_faces(&[(2, 1), (3, 1)]);
    let (mut world, mut scene, mut view, _, requests) =
        mesh_scene(&[(1, mesh_asset, Vec3::new(5.0, 0.0, 0.0)), (2, mesh_asset, Vec3::new(-5.0, 0.0, 0.0))], None);
    let old_camera = Camera::new(DVec3::ZERO, Vec3::X, 10_f64.to_radians(), [2048, 2048]).expect("fixture camera");
    let new_camera = Camera::new(DVec3::ZERO, -Vec3::X, 10_f64.to_radians(), [2048, 2048]).expect("fixture camera");
    scene.set_camera(new_camera);
    let mut requests = requests;
    requests.extend(settle_scene(&world, &mut scene, &mut view, Instant::now()));
    complete_mesh_requests(&world, &mut scene, &mut view, &std::collections::HashMap::from([(mesh_asset, mesh)]), requests);
    assert!(
        view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(100_002)).and_then(|object| object.mesh()).is_some(),
        "the return camera object has prepared mesh geometry"
    );

    let set_texture = |world: &mut world::World, id: u32, sequence: u32, texture: AssetId| {
        let mut observation = ObjectObservation::new(
            UpdateSource::new(Region::new(1), sequence, OrderEvidence::Ordered(u64::from(sequence)), 0),
            Uuid::from_u128(100_000 + u128::from(id)),
            std::num::NonZeroU32::new(id).expect("fixture local id"),
            Kind::Object,
            ParentUpdate::Root,
        );
        observation.appearance = FieldUpdate::Set(Appearance::Surface);
        observation.surface_appearance = Some(
            SurfaceAppearance::new(world::FaceAppearance { texture: Some(texture), ..Default::default() }, Vec::new())
                .expect("fixture appearance"),
        );
        assert_matches!(world.observe_object(observation).unwrap().outcome, world::ApplyOutcome::Updated(_));
    };

    set_texture(&mut world, 1, 10, old_texture);
    scene.notify_world_change();
    scene.set_camera(old_camera);
    let old_requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let old_texture_requests: Vec<_> = old_requests
        .into_iter()
        .filter(|request| request.request.resource().kind == assets::Kind::Texture && request.request.resource().id == old_texture)
        .collect();
    assert!(!old_texture_requests.is_empty(), "the initial camera requests the old visible texture");
    for request in old_texture_requests {
        scene.complete_asset(request.ticket, prepared_asset(request.request, test_support::texture::GRAY_4096_J2C.to_vec()));
    }
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());
    scene.set_camera(new_camera);
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());

    let texture_id = |view: &SceneView, object_id| {
        view.objects()
            .find(|object| object.detail().summary.id == Uuid::from_u128(object_id))
            .and_then(|object| object.materials())
            .and_then(|faces| {
                faces
                    .iter()
                    .find_map(|face| face.base_color_texture.as_ref().and_then(|binding| binding.image()).map(|texture| texture.id()))
            })
            .expect("fixture object texture is ready")
    };
    let old_id = texture_id(&view, 100_001);

    let (device, queue) = vulkan_device();
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    prepare_frame_at(&device, &queue, &mut geometry, &view, new_camera, Instant::now());
    assert_eq!(geometry.stats.instances, 1, "the new camera shows its mesh before the texture changes");

    set_texture(&mut world, 2, 11, new_texture);
    scene.notify_world_change();
    let requests = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let new_texture_requests: Vec<_> = requests
        .into_iter()
        .filter(|request| request.request.resource().kind == assets::Kind::Texture && request.request.resource().id == new_texture)
        .collect();
    assert!(!new_texture_requests.is_empty(), "the visible mesh requests its new texture");
    for request in new_texture_requests {
        scene.complete_asset(request.ticket, prepared_asset(request.request, test_support::texture::GRAY_4096_J2C.to_vec()));
    }
    let _ = settle_scene(&world, &mut scene, &mut view, Instant::now());
    let new_id = texture_id(&view, 100_002);
    assert!(old_id < new_id, "the older texture ID sorts before the newly visible texture");

    prepare_frame_at(&device, &queue, &mut geometry, &view, new_camera, Instant::now());
    let job = geometry.next_texture_job().expect("new visible texture upload job");
    geometry.complete_texture(job.run(crate::texture::CompressionQuality::Fast));
    assert!(geometry.next_texture_job().is_none());
    prepare_frame_at(&device, &queue, &mut geometry, &view, new_camera, Instant::now());
    assert!(!geometry.textures.is_resident(new_id), "the large new texture upload is still in flight");

    prepare_frame_at(&device, &queue, &mut geometry, &view, old_camera, Instant::now());
    let job = geometry.next_texture_job().expect("old visible texture upload job");
    geometry.complete_texture(job.run(crate::texture::CompressionQuality::Fast));
    assert!(geometry.next_texture_job().is_none());
    prepare_frame_at(&device, &queue, &mut geometry, &view, old_camera, Instant::now());
    assert!(!geometry.textures.is_resident(old_id), "the large old texture remains in flight");
    assert!(!geometry.textures.is_resident(new_id), "the offscreen upload remains incomplete");

    prepare_frame_at(&device, &queue, &mut geometry, &view, new_camera, Instant::now());
    assert!(!geometry.textures.is_resident(new_id), "the returned texture still needs another upload frame");
    prepare_frame_at(&device, &queue, &mut geometry, &view, new_camera, Instant::now());
    assert!(geometry.textures.is_resident(new_id), "the newly visible texture completes ahead of the offscreen upload");
    assert!(!geometry.textures.is_resident(old_id), "the large offscreen upload remains incomplete");
}
