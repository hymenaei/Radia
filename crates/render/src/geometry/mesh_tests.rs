use std::{
    assert_matches,
    sync::{Arc, mpsc},
    time::Instant,
};

use futures::executor::block_on;
use math::{DVec3, Vec3};
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
use crate::scene::{AgentPose, Camera, Placement, ReadQuery, ReadReply, ReconcileStep, Reconciler, SceneView};
use world::{
    Appearance, FieldUpdate, Kind, ObjectObservation, OrderEvidence, ParentUpdate, Pose, Region, SurfaceAppearance, UpdateSource, WorldPose,
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
                &instance_base(transform.pose.position, transform.pose.rotation, transform.scale, camera.position()),
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

fn settle_scene(world: &world::World, scene: &mut Reconciler, view: &mut SceneView, mut now: Instant) {
    loop {
        match scene.poll(now) {
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
            ReconcileStep::Idle => return,
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
            &instance_base(transform.pose.position, transform.pose.rotation, transform.scale, camera.position()),
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
