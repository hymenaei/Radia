use futures::executor::block_on;
use wgpu::{
    Backends, BufferDescriptor, BufferUsages, DeviceDescriptor, Extent3d, InstanceDescriptor, MapMode, Operations, PollType,
    RenderPassColorAttachment, RenderPassDescriptor, StoreOp, TexelCopyBufferInfo, TexelCopyBufferLayout, TexelCopyTextureInfo,
    TextureAspect, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
};

use super::*;
use crate::environment::{Schedule, Sources, evaluate};

#[test]
#[ignore = "requires Vulkan; run with cargo test -p render --lib sky_cloud_noise_and_moon_use_reference_samples -- --ignored"]
fn sky_cloud_noise_and_moon_use_reference_samples() {
    let instance =
        wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
    let (device, queue) = block_on(async {
        let adapter = instance.request_adapter(&Default::default()).await.expect("Vulkan adapter is available");
        adapter.request_device(&DeviceDescriptor::default()).await.expect("Vulkan device is available")
    });
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("Sky texture numerical regression"),
        source: ShaderSource::Wgsl(
            concat!(include_str!("../shaders/atmosphere.wgsl"), include_str!("../shaders/water.wgsl"), include_str!("../shaders/sky.wgsl"))
                .into(),
        ),
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let celestial_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let (fallback_texture, fallback_color, fallback_raw) = test_texture(&device, &queue, [128, 128, 128, 255]);
    let (cloud_texture, cloud_color, cloud_raw) = test_texture(&device, &queue, [192, 0, 0, 255]);
    let cloud_views = std::array::from_fn(|binding| {
        if matches!(binding, 4 | 5) {
            sky_texture_view(binding, &cloud_color, &cloud_raw)
        } else {
            sky_texture_view(binding, &fallback_color, &fallback_raw)
        }
    });
    let mut cloud_values = sky_uniform_values();
    cloud_values[94] = 0.5;
    cloud_values[98] = 0.0;
    cloud_values[100] = 0.5;
    let cloud = render_sky_fragment(&device, &queue, &shader, "cloud_fragment", &cloud_values, cloud_views, &sampler, &celestial_sampler);
    assert_eq!(cloud[3], 255, "raw R192/255 cloud noise reaches opaque cloud alpha");

    let (moon_texture, moon_color, moon_raw) = test_texture(&device, &queue, [255, 255, 255, 255]);
    let (moon_next_texture, moon_next_color, moon_next_raw) = test_texture(&device, &queue, [0, 0, 0, 0]);
    let moon_views = std::array::from_fn(|binding| match binding {
        2 => &moon_color,
        3 => &moon_next_color,
        4 | 5 => sky_texture_view(binding, &fallback_color, &fallback_raw),
        _ => sky_texture_view(binding, &fallback_color, &fallback_raw),
    });
    let mut moon_values = sky_uniform_values();
    moon_values[78] = 1.0;
    moon_values[79] = 1.0;
    moon_values[80] = 1.0;
    moon_values[85] = 1.0;
    moon_values[108] = 0.5;
    moon_values[114] = 1.0;
    moon_values[115] = 1.0;
    moon_values[116] = 0.5;
    let moon = render_sky_fragment(&device, &queue, &shader, "moon_fragment", &moon_values, moon_views, &sampler, &celestial_sampler);
    assert_eq!(moon, [127, 127, 127, 255]);
    drop((cloud_texture, moon_texture, moon_next_texture, fallback_texture, moon_raw, moon_next_raw));
}

#[test]
#[ignore = "requires Vulkan; run with cargo test -p render --lib stars_sample_the_current_bloom_texture -- --ignored"]
fn stars_sample_the_current_bloom_texture() {
    let instance =
        wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
    let (device, queue) = block_on(async {
        let adapter = instance.request_adapter(&Default::default()).await.expect("Vulkan adapter is available");
        adapter.request_device(&DeviceDescriptor::default()).await.expect("Vulkan device is available")
    });
    let source = format!(
        "{}{}",
        include_str!("../shaders/stars.wgsl"),
        r#"
@vertex fn probe_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let positions = array(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(positions[index], 0.0, 1.0);
}
@fragment fn probe_fragment() -> @location(0) vec4<f32> {
    var input: StarVertex;
    input.position = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    input.uv = vec2<f32>(0.5, 0.5);
    input.color = vec3<f32>(1.0);
    input.alpha = 0.9;
    input.screen_position = vec2<f32>(0.999999, 0.0);
    let color = star_color(input);
    return vec4<f32>(color.a / 32.0, 0.0, 0.0, 1.0);
}
"#
    );
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("Star bloom numerical regression"),
        source: ShaderSource::Wgsl(source.into()),
    });
    let (current_texture, current_color, _) = test_texture(&device, &queue, [255, 255, 255, 255]);
    let (next_texture, next_color, _) = test_texture(&device, &queue, [0, 0, 0, 0]);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let pair_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("Star bloom pair"),
        entries: &[
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 2,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let pair_binding = texture_pair_binding_for_views(&device, &pair_layout, &current_color, &next_color, &sampler);
    let state_layout = uniform_layout(&device, "Star regression state", ShaderStages::VERTEX_FRAGMENT);
    let star_data_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("Star regression field"),
        entries: &[BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::VERTEX,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("Star bloom numerical regression"),
        bind_group_layouts: &[Some(&state_layout), Some(&star_data_layout), Some(&pair_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("Star bloom numerical regression"),
        layout: Some(&pipeline_layout),
        vertex: VertexState { module: &shader, entry_point: Some("probe_vertex"), compilation_options: Default::default(), buffers: &[] },
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(FragmentState {
            module: &shader,
            entry_point: Some("probe_fragment"),
            compilation_options: Default::default(),
            targets: &[Some(ColorTargetState { format: TextureFormat::Rgba8Unorm, blend: None, write_mask: ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let state = device.create_buffer(&BufferDescriptor {
        label: Some("Star regression state"),
        size: STAR_UNIFORM_BYTES as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let state_binding = uniform_binding(&device, "Star regression state", &state_layout, &state);
    let field = device.create_buffer(&BufferDescriptor {
        label: Some("Star regression field"),
        size: 32,
        usage: BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let field_binding = device.create_bind_group(&BindGroupDescriptor {
        label: Some("Star regression field"),
        layout: &star_data_layout,
        entries: &[BindGroupEntry { binding: 0, resource: BindingResource::Buffer(field.as_entire_buffer_binding()) }],
    });
    let output = device.create_texture(&TextureDescriptor {
        label: Some("Star bloom regression output"),
        size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let output_view = output.create_view(&TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Star bloom numerical regression"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &output_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &state_binding, &[]);
        pass.set_bind_group(1, &field_binding, &[]);
        pass.set_bind_group(2, &pair_binding, &[]);
        pass.draw(0..3, 0..1);
    }
    let padded = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("Star bloom regression readback"),
        size: u64::from(padded),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo { texture: &output, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: TextureAspect::All },
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(1) },
        },
        Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::channel();
    readback.slice(..).map_async(MapMode::Read, move |result| {
        send.send(result).expect("readback receiver remains alive until completion");
    });
    device.poll(PollType::Wait { submission_index: None, timeout: None }).expect("star regression completes");
    receive.recv().expect("readback result arrives").expect("readback maps");
    let mapped = readback.slice(..).get_mapped_range().expect("readback range is mapped");
    let pixel: [u8; 4] = mapped[..4].try_into().expect("RGBA8 pixel has four channels");
    assert!(pixel[0] >= 254, "current alpha=1 produces a star mask near 32/32: {pixel:?}");
    drop(mapped);
    readback.unmap();
    drop((current_texture, next_texture));
}

fn sky_uniform_values() -> [f32; UNIFORM_FLOATS] {
    let environment = evaluate(
        &Sources {
            base: None,
            schedule: Schedule::default(),
            experience_day: None,
            experience_sky: None,
            experience_water: None,
            sky_override: None,
            water_override: None,
            local: None,
        },
        0.0,
        0.0,
    );
    let mut values = [0.0; UNIFORM_FLOATS];
    values[..16].copy_from_slice(&math::Mat4::IDENTITY.to_cols_array());
    let mut atmosphere = atmosphere_values(Some(&environment));
    atmosphere[24 + 3] = 0.25;
    atmosphere[40 + 1] = 0.0;
    values[16..64].copy_from_slice(&atmosphere);
    values
}

fn test_texture(device: &Device, queue: &Queue, pixel: [u8; 4]) -> (wgpu::Texture, TextureView, TextureView) {
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("Sky regression texture"),
        size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8UnormSrgb,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[TextureFormat::Rgba8Unorm],
    });
    queue.write_texture(
        texture.as_image_copy(),
        &pixel,
        TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
        Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    let color = texture.create_view(&TextureViewDescriptor::default());
    let raw = texture.create_view(&TextureViewDescriptor { format: Some(TextureFormat::Rgba8Unorm), ..Default::default() });
    (texture, color, raw)
}

fn render_sky_fragment(
    device: &Device,
    queue: &Queue,
    shader: &wgpu::ShaderModule,
    entry: &str,
    values: &[f32; UNIFORM_FLOATS],
    views: [&TextureView; 12],
    sampler: &wgpu::Sampler,
    celestial_sampler: &wgpu::Sampler,
) -> [u8; 4] {
    let uniform_layout = uniform_layout(device, "Sky regression uniform", ShaderStages::VERTEX_FRAGMENT);
    let texture_layout = sky_texture_layout(device);
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("Sky texture numerical regression"),
        bind_group_layouts: &[Some(&uniform_layout), Some(&texture_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("Sky texture numerical regression"),
        layout: Some(&pipeline_layout),
        vertex: VertexState { module: shader, entry_point: Some("vertex"), compilation_options: Default::default(), buffers: &[] },
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(FragmentState {
            module: shader,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            targets: &[Some(ColorTargetState { format: TextureFormat::Rgba8Unorm, blend: None, write_mask: ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let uniform = device.create_buffer(&BufferDescriptor {
        label: Some("Sky regression uniform"),
        size: UNIFORM_BYTES as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut bytes = [0; UNIFORM_BYTES];
    for (target, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        *target = value.to_le_bytes();
    }
    queue.write_buffer(&uniform, 0, &bytes);
    let uniform_binding = uniform_binding(device, "Sky regression uniform", &uniform_layout, &uniform);
    let texture_binding = sky_texture_binding_for_views(device, &texture_layout, views, sampler, celestial_sampler);
    let output = device.create_texture(&TextureDescriptor {
        label: Some("Sky regression output"),
        size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let output_view = output.create_view(&TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Sky texture numerical regression"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &output_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &uniform_binding, &[]);
        pass.set_bind_group(1, &texture_binding, &[]);
        pass.draw(0..3, 0..1);
    }
    let padded = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("Sky regression readback"),
        size: u64::from(padded),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo { texture: &output, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: TextureAspect::All },
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(1) },
        },
        Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::channel();
    readback.slice(..).map_async(MapMode::Read, move |result| {
        send.send(result).expect("readback receiver remains alive until completion");
    });
    device.poll(PollType::Wait { submission_index: None, timeout: None }).expect("sky regression completes");
    receive.recv().expect("readback result arrives").expect("readback maps");
    let mapped = readback.slice(..).get_mapped_range().expect("readback range is mapped");
    let pixel = mapped[..4].try_into().expect("RGBA8 pixel has four channels");
    drop(mapped);
    readback.unmap();
    pixel
}
