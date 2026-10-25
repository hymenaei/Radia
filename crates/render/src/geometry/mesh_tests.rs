use std::sync::{Arc, mpsc};

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
    Instance, ShadingMode,
    fixtures::{scene, scene_with_mapping, scene_with_scales},
    instance::{instance_base, packed_instance},
};
use crate::material::FaceParams;
use crate::pipeline::{INSTANCE_BYTES, Pipeline};
use crate::scene::Placement;

fn check_pages(device: &wgpu::Device, queue: &wgpu::Queue, alpha: u8, separate_pages: bool) {
    let scope = device.push_error_scope(ErrorFilter::Validation);
    let (_scene, view, camera) = scene(alpha);
    let mut geometry = GeometryRenderer::new(device, queue, TextureFormat::Rgba8UnormSrgb);
    let filler = separate_pages.then(|| {
        let (_, shared) = view.objects().next().unwrap().mesh().unwrap();
        assert!(geometry.upload_mesh(device, shared, &mut 0));
        let allocation = &geometry.meshes[&shared.id()].allocation;
        let vertices = allocation.buffers().vertices.size() as usize / VERTEX_BYTES - allocation.vertices().end as usize;
        let indices = allocation.buffers().indices.size() as usize / size_of::<u32>() - allocation.indices().end as usize;
        geometry.storage.allocate(device, vertices, indices, &geometry.memory).unwrap()
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    let depth = geometry.depth(device, [128, 128]).unwrap();
    geometry.prepare(device, &mut encoder, &view, camera, [128, 128], crate::FrameInput::default());
    let mut meshes: Vec<_> = geometry.meshes.values().collect();
    meshes.sort_by_key(|mesh| mesh.allocation.vertices().start);
    assert_eq!(meshes.len(), 2);
    assert_eq!(Arc::ptr_eq(meshes[0].allocation.buffers(), meshes[1].allocation.buffers()), !separate_pages);
    if !separate_pages {
        assert_eq!(meshes[0].allocation.vertices().end, meshes[1].allocation.vertices().start);
        assert_eq!(meshes[0].allocation.indices().end, meshes[1].allocation.indices().start);
    }
    if alpha == 255 {
        assert_eq!(geometry.stats.draw_calls, 2, "identical material parameters combine adjacent faces within each mesh");
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
            let instance = Instance {
                mesh: shared.clone(),
                position: transform.pose.position,
                rotation: transform.pose.rotation,
                scale: transform.scale,
                pickable: true,
                faces: Vec::new(),
                shading: ShadingMode::Studio,
                water_region: None,
            };
            let params = FaceParams::new(Some(&object.materials().unwrap()[0]));
            let packed = packed_instance(&instance_base(&instance, camera.position()), &params, instance.scale);
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
    assert!(shared_pixels == reference_pixels, "batched pages must match independently drawn complete source meshes");
    let mut red = 0;
    let mut green = 0;
    for pixel in shared_pixels.as_chunks::<4>().0 {
        red += usize::from(pixel[0] > pixel[1].saturating_add(32));
        green += usize::from(pixel[1] > pixel[0].saturating_add(32));
    }
    assert!(red > 100 && green > 100, "both meshes retain distinct geometry and material colors: {red} red, {green} green pixels");
    drop(pixels);
    readback.unmap();
    let (id, old) = geometry.meshes.iter().next().unwrap();
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
    assert!(geometry.needs_frame(), "retirement must unblock preparation without freeing a whole page");
    let reused = geometry.storage.allocate(device, old_vertices.len(), old_indices.len(), &geometry.memory).unwrap();
    assert_eq!((reused.vertices(), reused.indices()), (old_vertices, old_indices));
    assert!(Arc::ptr_eq(reused.buffers(), &old_buffers));
    drop(reused);
    drop(old_buffers);
    drop(filler);
    assert!(block_on(scope.pop()).is_none(), "shared ranges and draw offsets must satisfy GPU validation");
    let memory = geometry.memory.clone();
    drop(geometry);
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    assert_eq!(memory.mesh_state().bytes, 0, "renderer retirement releases page accounting after GPU completion");
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
    geometry.prepare(device, &mut prepare, &view, camera, size, None);
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
    assert!(red.len() > 100, "unmirrored prim remains visible after back-face culling");
    assert!(green.len() > 100, "mirrored prim remains visible after back-face culling");
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
    let projection = geometry.pipelines.update_camera_uniform(queue, camera, size);
    let mut objects: Vec<_> = view.objects().collect();
    objects.sort_by_key(|object| object.detail().summary.id);
    let mut instances = Vec::new();
    for object in &objects {
        let (_, mesh) = object.mesh().unwrap();
        let Placement::Ready(transform) = object.placement() else { panic!("fixture prim must be placed") };
        let face = object.materials().unwrap().iter().find(|material| material.slot == 0).expect("fixture has face zero");
        let mut params = FaceParams::new(Some(face));
        assert_eq!(params.params[3], 1.0, "scene planar mapping reaches the production surface parameters");
        params.color = [1.0; 4];
        params.params[0] = 1.0;
        let instance = Instance {
            mesh: mesh.clone(),
            position: transform.pose.position,
            rotation: transform.pose.rotation,
            scale: transform.scale,
            pickable: true,
            faces: Vec::new(),
            shading: ShadingMode::Studio,
        };
        instances.extend_from_slice(&packed_instance(&instance_base(&instance, camera.position()), &params, instance.scale));
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

#[test]
#[ignore = "requires a Vulkan adapter; run explicitly with cargo nextest run -p render --run-ignored ignored-only gpu_mesh_pages"]
fn gpu_mesh_pages() {
    let (device, queue) = block_on(async {
        let instance =
            wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        adapter.request_device(&DeviceDescriptor::default()).await.unwrap()
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
