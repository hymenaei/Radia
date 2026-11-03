use super::*;
use crate::environment::{Schedule, Sources, evaluate};
use futures::executor::block_on;
use wgpu::{
    Backends, DeviceDescriptor, Extent3d, InstanceDescriptor, MapMode, PollType, RenderPassDepthStencilAttachment, TexelCopyBufferInfo,
    TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect,
};

fn assert_rgba_near(actual: [u8; 4], expected: [u8; 4], tolerance: u8) {
    for (index, (actual, expected)) in actual.into_iter().zip(expected).enumerate() {
        assert!(actual.abs_diff(expected) <= tolerance, "channel {index}: {actual} != {expected}");
    }
}

#[test]
fn exposure_and_tonemap_defaults_follow_sky_ambiance() {
    let sources = Sources {
        base: None,
        schedule: Schedule::default(),
        experience_day: None,
        experience_sky: None,
        experience_water: None,
        sky_override: None,
        water_override: None,
        local: None,
    };
    let mut environment = evaluate(&sources, 0.0, 0.0);
    assert_eq!(environment_values(Some(&environment)), (1.0, 1.0, 0.0));
    environment.sky.reflection_probe_ambiance = Some(0.5);
    environment.sky.gamma = 1.44;
    let (minimum, maximum, mix) = environment_values(Some(&environment));
    assert!((minimum - (1.0 / 2.4)).abs() < 0.000001);
    assert!((maximum - 2.4).abs() < 0.000001);
    assert_eq!(mix, 0.7);
}

#[test]
fn exposure_requests_frames_only_while_settling() {
    let now = Instant::now();
    assert!(exposure_needs_frame(Some(now + EXPOSURE_SETTLE_TIME), now));
    assert!(!exposure_needs_frame(Some(now), now));
    assert!(!exposure_needs_frame(None, now));
}

#[test]
#[ignore = "requires Vulkan; run with cargo test -p render --lib tonemap_matches_aces_and_keeps_depth_zero_scene_linear -- --ignored"]
fn tonemap_matches_aces_and_keeps_depth_zero_scene_linear() {
    let instance =
        wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
    let (device, queue) = block_on(async {
        let adapter = instance.request_adapter(&Default::default()).await.expect("Vulkan adapter is available");
        adapter.request_device(&DeviceDescriptor::default()).await.expect("Vulkan device is available")
    });
    let aces = render_tonemap_vector(&device, &queue, [0.75, 0.6, 0.35, 1.0], 1.0, 0.0, 0.7);
    assert_rgba_near(aces, [202, 186, 150, 255], 1);
    let sky = render_tonemap_vector(&device, &queue, [0.5, 0.25, 0.1, 1.0], 1.0, 0.0, 0.0);
    assert_rgba_near(sky, [188, 137, 89, 255], 1);
}

#[test]
#[ignore = "requires Vulkan; run with cargo test -p render --lib exposure_uses_diffuse_scale_and_centered_crop -- --ignored"]
fn exposure_uses_diffuse_scale_and_centered_crop() {
    let instance =
        wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
    let (device, queue) = block_on(async {
        let adapter = instance.request_adapter(&Default::default()).await.expect("Vulkan adapter is available");
        adapter.request_device(&DeviceDescriptor::default()).await.expect("Vulkan device is available")
    });
    let baseline = render_exposure_scene(&device, &queue, None, [0.25, 0.25, 0.25, 1.0], 0.5);
    let outside_crop = render_exposure_scene(&device, &queue, Some((8, 8, 64, 64)), [0.25, 0.25, 0.25, 1.0], 0.5);
    let inside_crop = render_exposure_scene(&device, &queue, Some((64, 96, 64, 64)), [0.25, 0.25, 0.25, 1.0], 0.5);
    assert_rgba_near(baseline, [168, 168, 168, 255], 1);
    assert_rgba_near(outside_crop, baseline, 1);
    assert_rgba_near(inside_crop, [80, 80, 80, 255], 1);
}

#[test]
#[ignore = "requires Vulkan; run with cargo test -p render --lib exposure_keeps_depth_zero_luminance_linear -- --ignored"]
fn exposure_keeps_depth_zero_luminance_linear() {
    let instance =
        wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
    let (device, queue) = block_on(async {
        let adapter = instance.request_adapter(&Default::default()).await.expect("Vulkan adapter is available");
        adapter.request_device(&DeviceDescriptor::default()).await.expect("Vulkan device is available")
    });
    let sky = render_exposure_scene(&device, &queue, None, [0.5, 0.25, 0.1, 1.0], 0.0);
    assert_rgba_near(sky, [202, 151, 97, 255], 1);
}

fn render_exposure_scene(
    device: &Device,
    queue: &Queue,
    patch: Option<(u32, u32, u32, u32)>,
    background: [f64; 4],
    depth_value: f32,
) -> [u8; 4] {
    use wgpu::{PipelineLayoutDescriptor, PrimitiveState, VertexState};

    let environment = {
        let mut environment = evaluate(
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
        environment.sky.reflection_probe_ambiance = Some(0.5);
        environment
    };
    let mut postprocess = PostProcess::new(device, queue, TextureFormat::Rgba8UnormSrgb);
    postprocess.resize(device, [LUMINANCE_SIZE, LUMINANCE_SIZE]);
    let depth = device.create_texture(&TextureDescriptor {
        label: Some("Exposure regression depth"),
        size: Extent3d { width: LUMINANCE_SIZE, height: LUMINANCE_SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Depth32Float,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&TextureViewDescriptor::default());
    postprocess.set_depth_view(device, Some(&depth_view));
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("Exposure regression") });
    {
        let _pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Exposure regression depth"),
            color_attachments: &[],
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(Operations { load: LoadOp::Clear(depth_value), store: StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
    }
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Exposure regression background"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: postprocess.scene_view(),
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(Color { r: background[0], g: background[1], b: background[2], a: background[3] }),
                    store: StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        if let Some((x, y, width, height)) = patch {
            let shader = device.create_shader_module(ShaderModuleDescriptor {
                label: Some("Exposure regression patch"),
                source: ShaderSource::Wgsl(
                    r#"
struct VertexOutput { @builtin(position) position: vec4<f32>, }
@vertex fn vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    let positions = array(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var output: VertexOutput;
    output.position = vec4<f32>(positions[index], 0.0, 1.0);
    return output;
}
@fragment fn fragment() -> @location(0) vec4<f32> { return vec4<f32>(5.0, 5.0, 5.0, 1.0); }
"#
                    .into(),
                ),
            });
            let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
                label: Some("Exposure regression patch"),
                bind_group_layouts: &[],
                immediate_size: 0,
            });
            let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
                label: Some("Exposure regression patch"),
                layout: Some(&layout),
                vertex: VertexState { module: &shader, entry_point: Some("vertex"), compilation_options: Default::default(), buffers: &[] },
                primitive: PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(FragmentState {
                    module: &shader,
                    entry_point: Some("fragment"),
                    compilation_options: Default::default(),
                    targets: &[Some(ColorTargetState { format: HDR_FORMAT, blend: None, write_mask: ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_viewport(x as f32, y as f32, width as f32, height as f32, 0.0, 1.0);
            pass.draw(0..3, 0..1);
        }
    }
    let output = device.create_texture(&TextureDescriptor {
        label: Some("Exposure regression output"),
        size: Extent3d { width: LUMINANCE_SIZE, height: LUMINANCE_SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8UnormSrgb,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let output_view = output.create_view(&TextureViewDescriptor::default());
    postprocess.render(queue, &mut encoder, &output_view, Some(&environment), 1.0 / 60.0, None, None, Mat4::IDENTITY, Mat4::IDENTITY, 0, 0);
    let padded = LUMINANCE_SIZE * 4;
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("Exposure regression readback"),
        size: u64::from(padded) * u64::from(LUMINANCE_SIZE),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo { texture: &output, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: TextureAspect::All },
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(LUMINANCE_SIZE) },
        },
        Extent3d { width: LUMINANCE_SIZE, height: LUMINANCE_SIZE, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::channel();
    readback.slice(..).map_async(MapMode::Read, move |result| {
        send.send(result).expect("readback receiver remains alive until completion");
    });
    device.poll(PollType::Wait { submission_index: None, timeout: None }).expect("exposure completes");
    receive.recv().expect("readback result arrives").expect("readback maps");
    let mapped = readback.slice(..).get_mapped_range().expect("readback range is mapped");
    let offset = ((30 * LUMINANCE_SIZE + 220) * 4) as usize;
    let pixel = mapped[offset..offset + 4].try_into().expect("RGBA8 pixel has four channels");
    drop(mapped);
    readback.unmap();
    pixel
}

fn render_tonemap_vector(device: &Device, queue: &Queue, color: [f64; 4], exposure: f64, depth: f32, mix: f32) -> [u8; 4] {
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("Tonemap numerical regression"),
        source: ShaderSource::Wgsl(
            concat!(
                include_str!("../shaders/atmosphere.wgsl"),
                include_str!("../shaders/water.wgsl"),
                include_str!("../shaders/tonemap.wgsl")
            )
            .into(),
        ),
    });
    let tone_layout = uniform_layout(device, "Tonemap regression settings", TONE_BYTES);
    let source_layout = source_layout(device);
    let pipeline = fullscreen_pipeline(
        device,
        "Tonemap numerical regression",
        &shader,
        &tone_layout,
        &source_layout,
        TextureFormat::Rgba8UnormSrgb,
        "fragment",
    );
    let tone_buffer = uniform_buffer(device, "Tonemap regression settings", TONE_BYTES);
    let tone_binding = uniform_binding(device, "Tonemap regression settings", &tone_layout, &tone_buffer);
    let scene = render_texture(device, "Tonemap regression scene", [1, 1], HDR_FORMAT);
    let scene_view = scene.create_view(&TextureViewDescriptor::default());
    let exposure_texture = render_texture(device, "Tonemap regression exposure", [1, 1], HDR_FORMAT);
    let exposure_view = exposure_texture.create_view(&TextureViewDescriptor::default());
    let depth_texture = device.create_texture(&TextureDescriptor {
        label: Some("Tonemap regression depth"),
        size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Depth32Float,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let depth_view = depth_texture.create_view(&TextureViewDescriptor::default());
    let target = device.create_texture(&TextureDescriptor {
        label: Some("Tonemap regression output"),
        size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8UnormSrgb,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&TextureViewDescriptor::default());
    let mut values = [0.0; TONE_FLOATS];
    values[..4].copy_from_slice(&[1.0, mix, 1.0, 0.0]);
    values[12..28].copy_from_slice(&Mat4::IDENTITY.to_cols_array());
    values[28..44].copy_from_slice(&Mat4::IDENTITY.to_cols_array());
    let mut uniform_bytes = [0; TONE_BYTES];
    write_values(queue, &tone_buffer, &values, &mut uniform_bytes);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let sources =
        source_binding(device, "Tonemap numerical regression", &source_layout, &sampler, &scene_view, &exposure_view, &depth_view);
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("Tonemap numerical regression") });
    for (view, clear) in [
        (&scene_view, Color { r: color[0], g: color[1], b: color[2], a: color[3] }),
        (&exposure_view, Color { r: exposure, g: exposure, b: exposure, a: exposure }),
    ] {
        let _pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Tonemap regression inputs"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: LoadOp::Clear(clear), store: StoreOp::Store },
            })],
            ..Default::default()
        });
    }
    {
        let _pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Tonemap regression depth"),
            color_attachments: &[],
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(Operations { load: LoadOp::Clear(depth), store: StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
    }
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Tonemap numerical regression"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: LoadOp::Clear(Color::BLACK), store: StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &tone_binding, &[]);
        pass.set_bind_group(1, &sources, &[]);
        pass.draw(0..3, 0..1);
    }
    let padded = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("Tonemap regression readback"),
        size: u64::from(padded),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: TextureAspect::All },
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
    device.poll(PollType::Wait { submission_index: None, timeout: None }).expect("tonemap completes");
    receive.recv().expect("readback result arrives").expect("readback maps");
    let mapped = readback.slice(..).get_mapped_range().expect("readback range is mapped");
    let pixel = mapped[..4].try_into().expect("RGBA8 pixel has four channels");
    drop(mapped);
    readback.unmap();
    pixel
}

#[test]
#[ignore = "requires Vulkan; run with cargo nextest run -p render --lib exposure_interpolates_subpixel_samples --run-ignored all"]
fn exposure_interpolates_subpixel_samples() {
    let instance =
        wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
    let (device, queue) = block_on(async {
        let adapter = instance.request_adapter(&Default::default()).await.expect("Vulkan adapter is available");
        adapter.request_device(&DeviceDescriptor::default()).await.expect("Vulkan device is available")
    });
    let actual = render_luminance_sample(&device, &queue);
    assert!((actual - 0.12756).abs() < 0.001, "{actual} != 0.12756");
}

fn render_luminance_sample(device: &Device, queue: &Queue) -> f32 {
    use wgpu::{PipelineLayoutDescriptor, PrimitiveState, VertexState};

    let mut postprocess = PostProcess::new(device, queue, TextureFormat::Rgba8UnormSrgb);
    postprocess.resize(device, [LUMINANCE_SIZE, LUMINANCE_SIZE]);
    queue.write_buffer(&postprocess.exposure_uniform, 0, &[0; EXPOSURE_BYTES]);
    let depth = device.create_texture(&TextureDescriptor {
        label: Some("Subpixel exposure depth"),
        size: Extent3d { width: LUMINANCE_SIZE, height: LUMINANCE_SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Depth32Float,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&TextureViewDescriptor::default());
    postprocess.set_depth_view(device, Some(&depth_view));
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("Subpixel exposure sample") });
    {
        let _pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Subpixel exposure depth"),
            color_attachments: &[],
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(Operations { load: LoadOp::Clear(0.0), store: StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
    }
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("Subpixel exposure stripe"),
        source: ShaderSource::Wgsl(
            r#"
struct VertexOutput { @builtin(position) position: vec4<f32>, }
@vertex fn vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    let positions = array(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var output: VertexOutput;
    output.position = vec4<f32>(positions[index], 0.0, 1.0);
    return output;
}
@fragment fn fragment() -> @location(0) vec4<f32> { return vec4<f32>(1.0, 0.0, 0.0, 1.0); }
"#
            .into(),
        ),
    });
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("Subpixel exposure stripe"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("Subpixel exposure stripe"),
        layout: Some(&layout),
        vertex: VertexState { module: &shader, entry_point: Some("vertex"), compilation_options: Default::default(), buffers: &[] },
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(FragmentState {
            module: &shader,
            entry_point: Some("fragment"),
            compilation_options: Default::default(),
            targets: &[Some(ColorTargetState { format: HDR_FORMAT, blend: None, write_mask: ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    });
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Subpixel exposure stripe"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: postprocess.scene_view(),
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: LoadOp::Clear(Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }), store: StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_viewport(52.0, 0.0, 1.0, LUMINANCE_SIZE as f32, 0.0, 1.0);
        pass.draw(0..3, 0..1);
    }
    let target = device.create_texture(&TextureDescriptor {
        label: Some("Subpixel exposure output"),
        size: Extent3d { width: LUMINANCE_SIZE, height: LUMINANCE_SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: HDR_FORMAT,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Subpixel luminance crop"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &target.create_view(&TextureViewDescriptor::default()),
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: LoadOp::Clear(Color::TRANSPARENT), store: StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&postprocess.luminance_pipeline);
        pass.set_bind_group(0, &postprocess.exposure_uniform_binding, &[]);
        pass.set_bind_group(
            1,
            postprocess.scene_bindings[1 - postprocess.current_exposure].as_ref().expect("scene luminance inputs are initialized"),
            &[],
        );
        pass.draw(0..3, 0..1);
    }
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("Subpixel exposure readback"),
        size: u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d { x: 1, y: 128, z: 0 }, aspect: TextureAspect::All },
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT), rows_per_image: Some(1) },
        },
        Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::channel();
    readback.slice(..).map_async(MapMode::Read, move |result| {
        send.send(result).expect("readback receiver remains alive until completion");
    });
    device.poll(PollType::Wait { submission_index: None, timeout: None }).expect("subpixel exposure completes");
    receive.recv().expect("readback result arrives").expect("readback maps");
    let mapped = readback.slice(..).get_mapped_range().expect("readback range is mapped");
    let value = f16_to_f32(u16::from_le_bytes([mapped[0], mapped[1]]));
    drop(mapped);
    readback.unmap();
    value
}
fn f16_to_f32(value: u16) -> f32 {
    let sign = u32::from(value & 0x8000) << 16;
    let exponent = u32::from((value >> 10) & 0x1f);
    let fraction = u32::from(value & 0x03ff);
    let bits = match exponent {
        0 if fraction == 0 => sign,
        0 => {
            let mut fraction = fraction;
            let mut exponent = 113;
            while fraction & 0x0400 == 0 {
                fraction <<= 1;
                exponent -= 1;
            }
            sign | (exponent << 23) | ((fraction & 0x03ff) << 13)
        }
        31 => sign | 0x7f80_0000 | (fraction << 13),
        _ => sign | ((exponent + 112) << 23) | (fraction << 13),
    };
    f32::from_bits(bits)
}
