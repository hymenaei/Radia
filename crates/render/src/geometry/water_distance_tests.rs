use std::sync::mpsc;

use assets::EnvironmentDescription;
use futures::executor::block_on;
use math::{DVec3, Vec3};
use wgpu::{
    Backends, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, BindingType,
    BufferBindingType, BufferDescriptor, BufferUsages, Color, ComputePassDescriptor, ComputePipelineDescriptor, Device, DeviceDescriptor,
    Extent3d, InstanceDescriptor, LoadOp, MapMode, Operations, PipelineLayoutDescriptor, PollType, RenderPassColorAttachment,
    RenderPassDepthStencilAttachment, RenderPassDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages, StoreOp,
    TexelCopyBufferInfo, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    TextureViewDescriptor,
};

use super::*;
use crate::{
    FrameInput,
    environment::{Schedule, Sources, evaluate},
    pipeline::{PostProcess, projection},
    texture::CompressionQuality,
};

const SIZE: [u32; 2] = [320, 180];
const WATER_HEIGHT: f64 = 20.0;
const MAX_CHANNEL_DIFFERENCE: u8 = 2;

#[test]
#[ignore = "requires Vulkan; run with cargo test -p render --lib water_color_ignores_far_plane -- --ignored"]
fn water_color_ignores_far_plane() {
    let instance =
        wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
    let (device, queue) = block_on(async {
        let adapter = instance.request_adapter(&Default::default()).await.expect("Vulkan adapter is available");
        adapter.request_device(&DeviceDescriptor::default()).await.expect("Vulkan device is available")
    });
    let settings = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../assets/tests/fixtures/settings");
    let presets = ["Midday.dat", "Sunset.dat", "Midnight.dat"].map(|name| {
        let bytes = std::fs::read(settings.join(name)).expect("environment fixture is available");
        let description = EnvironmentDescription::decode(&bytes).expect("environment fixture is valid");
        evaluate(
            &Sources {
                base: Some(&description),
                schedule: Schedule::default(),
                experience_day: None,
                experience_sky: None,
                experience_water: None,
                sky_override: None,
                water_override: None,
                local: None,
            },
            0.0,
            26.0,
        )
    });
    let (_scene, view, _) = fixtures::water_scene(0);
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba16Float);

    let camera = Camera::new(DVec3::new(128.0, 128.0, WATER_HEIGHT + 2.0), Vec3::new(1.0, 0.0, -0.01), 1.0, SIZE).expect("camera is valid");
    let distance = (WATER_HEIGHT - camera.position().z).abs() / f64::from(camera.direction().z.abs());
    let intersection_x = camera.position().x + f64::from(camera.direction().x) * distance;
    assert!(intersection_x > 256.0, "center ray intersects outside the region water bounds");
    let with_void_water = render_frame(&device, &queue, &mut geometry, &view, camera, &presets[0], Some(WATER_HEIGHT));
    let without_water = render_frame(&device, &queue, &mut geometry, &view, camera, &presets[0], None);
    let center = ((SIZE[1] / 2 * SIZE[0] + SIZE[0] / 2) * 4) as usize;
    let visible_water = with_void_water[center..center + 3]
        .iter()
        .zip(&without_water[center..center + 3])
        .map(|(water, clear)| water.abs_diff(*clear))
        .max()
        .unwrap_or(0);
    assert!(visible_water > MAX_CHANNEL_DIFFERENCE, "center pixel outside region water shows void water");

    for height in [200.0, 1000.0] {
        let camera = Camera::new(DVec3::new(128.0, 128.0, WATER_HEIGHT + height), Vec3::Y, 1.0, SIZE).unwrap();
        let image = render_frame(&device, &queue, &mut geometry, &view, camera, &presets[0], Some(WATER_HEIGHT));
        let x = SIZE[0] as usize / 2;
        let mut maximum = 0;
        let mut maximum_row = 0;
        for y in (SIZE[1] as usize / 2 + 5)..SIZE[1] as usize - 1 {
            let a = (y * SIZE[0] as usize + x) * 4;
            let b = a + SIZE[0] as usize * 4;
            for channel in 0..3 {
                let difference = image[a + channel].abs_diff(image[b + channel]);
                if difference > maximum {
                    maximum = difference;
                    maximum_row = y;
                }
            }
        }
        assert!(maximum < 12, "height {height}: water has a sharp ring at row {maximum_row}, adjacent rows differ by {maximum}");
    }

    for position in [DVec3::new(128.0, 128.0, WATER_HEIGHT - 2.0), DVec3::new(512.0, 512.0, WATER_HEIGHT - 2.0)] {
        let camera = Camera::new(position, Vec3::X, 1.0, SIZE).unwrap();
        let image = render_frame(&device, &queue, &mut geometry, &view, camera, &presets[0], Some(WATER_HEIGHT));
        for row in [SIZE[1] / 4, SIZE[1] * 3 / 4] {
            let pixel = ((row * SIZE[0] + SIZE[0] / 4) * 4) as usize;
            assert!(
                image[pixel..pixel + 3].iter().all(|channel| *channel < 200),
                "underwater background at {position:?}, row {row}: {:?}",
                &image[pixel..pixel + 4]
            );
            assert!(
                image[pixel + 2] > 10 && image[pixel + 2] > image[pixel].saturating_add(10),
                "underwater background retains blue water fog: {:?}",
                &image[pixel..pixel + 4]
            );
        }
    }

    for (preset, environment) in ["midday", "sunset", "midnight"].into_iter().zip(&presets) {
        for height in [0.5, 2.0, 20.0, 200.0] {
            let mut render = |draw_distance| {
                let camera = Camera::new(DVec3::new(128.0, 128.0, WATER_HEIGHT + height), Vec3::X, 1.0, SIZE)
                    .expect("camera is valid")
                    .with_quality(draw_distance, 1.0)
                    .expect("draw distance is valid");
                render_frame(&device, &queue, &mut geometry, &view, camera, environment, Some(WATER_HEIGHT))
            };
            let near = render(64.0);
            let far = render(512.0);
            let mut row_maxima = Vec::with_capacity(SIZE[1] as usize / 2);
            for row in SIZE[1] as usize / 2..SIZE[1] as usize {
                let start = row * SIZE[0] as usize * 4;
                let end = start + SIZE[0] as usize * 4;
                let maximum = near[start..end]
                    .iter()
                    .zip(&far[start..end])
                    .enumerate()
                    .filter(|(channel, _)| channel % 4 < 3)
                    .map(|(_, (first, second))| first.abs_diff(*second))
                    .max()
                    .unwrap_or(0);
                assert!(
                    maximum <= MAX_CHANNEL_DIFFERENCE,
                    "{preset} height {height} row {row}: draw-distance water colors differ by {maximum}"
                );
                row_maxima.push(maximum);
            }
            eprintln!("{preset} height {height}: {}", row_profile(&row_maxima, SIZE[1] as usize / 2));
        }
    }
}

fn render_frame(
    device: &Device,
    queue: &wgpu::Queue,
    geometry: &mut GeometryRenderer,
    view: &crate::scene::SceneView,
    camera: Camera,
    environment: &crate::environment::Evaluated,
    water_height: Option<f64>,
) -> Vec<u8> {
    let mut postprocess = PostProcess::new(device, queue, TextureFormat::Rgba8UnormSrgb);
    postprocess.resize(device, SIZE);
    let depth = geometry.depth(device, SIZE).expect("depth target is admitted");
    postprocess.set_depth_view(device, Some(&depth));
    let frame =
        FrameInput { water_height, environment: Some(environment), cloud_scroll: [0.0; 2], animation_time: 1.0, water_scroll: [0.0; 4] };
    let mut encoder = device.create_command_encoder(&Default::default());
    let admission = geometry.admit().expect("frame is admitted");
    geometry.prepare_with_environment(device, &mut encoder, view, None, camera, SIZE, frame);
    while let Some(job) = geometry.next_texture_job() {
        geometry.complete_texture(job.run(CompressionQuality::Fast));
    }
    geometry.prepare_with_environment(device, &mut encoder, view, None, camera, SIZE, frame);
    let output = device.create_texture(&TextureDescriptor {
        label: Some("Water distance comparison"),
        size: Extent3d { width: SIZE[0], height: SIZE[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8UnormSrgb,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let output_view = output.create_view(&TextureViewDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Water distance scene"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: postprocess.scene_view(),
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: LoadOp::Clear(Color { r: 0.012, g: 0.012, b: 0.012, a: 1.0 }), store: StoreOp::Store },
            })],
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(Operations { load: LoadOp::Clear(0.0), store: StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        geometry.draw(&mut pass);
    }
    postprocess.render(
        queue,
        &mut encoder,
        &output_view,
        Some(environment),
        1.0 / 60.0,
        Some(camera.position()),
        water_height,
        projection(camera, SIZE).inverse(),
        crate::pipeline::camera_view(camera),
        0,
        0,
    );
    let bytes_per_row = SIZE[0] * 4;
    let padded = bytes_per_row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("Water distance readback"),
        size: u64::from(padded) * u64::from(SIZE[1]),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo { texture: &output, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(SIZE[1]) },
        },
        Extent3d { width: SIZE[0], height: SIZE[1], depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    geometry.submitted(admission);
    let (send, receive) = mpsc::channel();
    readback.slice(..).map_async(MapMode::Read, move |result| {
        send.send(result).expect("readback receiver remains alive until completion");
    });
    device.poll(PollType::Wait { submission_index: None, timeout: None }).expect("render completes");
    receive.recv().expect("readback result arrives").expect("readback maps");
    let mapped = readback.slice(..).get_mapped_range().expect("readback range is mapped");
    let mut pixels = Vec::with_capacity(SIZE[0] as usize * SIZE[1] as usize * 4);
    for row in mapped.chunks_exact(padded as usize) {
        pixels.extend_from_slice(&row[..bytes_per_row as usize]);
    }
    drop(mapped);
    readback.unmap();
    pixels
}

fn row_profile(maxima: &[u8], first_row: usize) -> String {
    let mut profile = Vec::new();
    let mut start = first_row;
    let mut current = maxima[0];
    for (offset, value) in maxima.iter().copied().enumerate().skip(1) {
        if value != current {
            profile.push(format!("{start}-{}:{current}", first_row + offset - 1));
            start = first_row + offset;
            current = value;
        }
    }
    profile.push(format!("{start}-{}:{current}", first_row + maxima.len() - 1));
    profile.join(",")
}

#[test]
#[ignore = "requires Vulkan; run with cargo test -p render --lib water_uvs_continue_past_limit -- --ignored"]
fn water_uvs_continue_past_limit() {
    let instance =
        wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
    let (device, queue) = block_on(async {
        let adapter = instance.request_adapter(&Default::default()).await.expect("Vulkan adapter is available");
        adapter.request_device(&DeviceDescriptor::default()).await.expect("Vulkan device is available")
    });
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("Water UV regression"),
        source: ShaderSource::Wgsl(
            format!(
                "{}{}{}",
                include_str!("../shaders/atmosphere.wgsl"),
                include_str!("../shaders/water.wgsl"),
                r#"
@group(0) @binding(0) var<storage, read_write> output: array<vec4<f32>, 3>;

@compute @workgroup_size(1)
fn water_uv_probe() {
    var water: WaterSurface;
    water.animation = vec4<f32>(0.0);
    water.wave_directions = vec4<f32>(1.0);
    let near = water_coordinates_at(vec3<f32>(3000.0, 0.0, 0.0), water);
    let far = water_coordinates_at(vec3<f32>(3001.0, 0.0, 0.0), water);
    output[0] = vec4<f32>(near.big, far.big);
    output[1] = vec4<f32>(near.first, far.first);
    output[2] = vec4<f32>(near.second, far.second);
}
"#,
            )
            .into(),
        ),
    });
    let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("Water UV regression"),
        entries: &[BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::COMPUTE,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("Water UV regression"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&ComputePipelineDescriptor {
        label: Some("Water UV regression"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("water_uv_probe"),
        compilation_options: Default::default(),
        cache: None,
    });
    let output = device.create_buffer(&BufferDescriptor {
        label: Some("Water UV output"),
        size: 3 * 16,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let binding = device.create_bind_group(&BindGroupDescriptor {
        label: Some("Water UV output"),
        layout: &layout,
        entries: &[BindGroupEntry { binding: 0, resource: BindingResource::Buffer(output.as_entire_buffer_binding()) }],
    });
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("Water UV readback"),
        size: 3 * 16,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &binding, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 3 * 16);
    queue.submit([encoder.finish()]);
    let (send, receive) = mpsc::channel();
    readback.slice(..).map_async(MapMode::Read, move |result| {
        send.send(result).expect("readback receiver remains alive until completion");
    });
    device.poll(PollType::Wait { submission_index: None, timeout: None }).expect("water UV probe completes");
    receive.recv().expect("water UV readback result arrives").expect("water UV readback maps");
    let mapped = readback.slice(..).get_mapped_range().expect("water UV readback range is mapped");
    let values = mapped.as_chunks::<4>().0.iter().map(|bytes| f32::from_le_bytes(*bytes)).collect::<Vec<_>>();
    for (coordinates, index) in [("big", 0), ("first", 4), ("second", 8)] {
        let difference = (values[index] - values[index + 2]).abs().max((values[index + 1] - values[index + 3]).abs());
        assert!(difference > 0.01, "{coordinates} water UVs collapse beyond 2560 m");
    }
}

#[test]
#[ignore = "requires Vulkan; run with cargo test -p render --lib water_base_color_skips_extra_atmosphere_haze -- --ignored"]
fn water_base_color_skips_extra_atmosphere_haze() {
    let instance =
        wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
    let (device, queue) = block_on(async {
        let adapter = instance.request_adapter(&Default::default()).await.expect("Vulkan adapter is available");
        adapter.request_device(&DeviceDescriptor::default()).await.expect("Vulkan device is available")
    });
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("Water atmosphere numerical regression"),
        source: ShaderSource::Wgsl(
            format!(
                "{}{}{}",
                include_str!("../shaders/atmosphere.wgsl"),
                include_str!("../shaders/water.wgsl"),
                r#"
@group(0) @binding(0) var<storage, read_write> output: array<vec4<f32>, 2>;

fn shade_probe(density: f32) -> vec4<f32> {
    var water: WaterSurface;
    water.fog.color = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    water.fog.parameters = vec4<f32>(0.0);
    water.normal_scale = vec4<f32>(1.0, 1.0, 1.0, 1.0);
    water.fresnel = vec4<f32>(0.2, 0.4, 0.0, 0.0);
    water.wave_directions = vec4<f32>(0.0);
    water.animation = vec4<f32>(0.0);
    water.uv_origin0 = vec4<f32>(0.0);
    water.uv_origin1 = vec4<f32>(0.0);
    water.camera_relative = vec4<f32>(0.0);
    var atmosphere: Atmosphere;
    atmosphere.blue_horizon = vec4<f32>(0.0);
    atmosphere.blue_density = vec4<f32>(density, density, density, 0.0);
    atmosphere.haze = vec4<f32>(0.0, 0.0, 1.0, 1.0);
    atmosphere.sunlight = vec4<f32>(1.0, 1.0, 1.0, 0.0);
    atmosphere.light_direction = vec4<f32>(0.0, 0.0, 1.0, 0.0);
    atmosphere.ambient = vec4<f32>(0.0);
    atmosphere.glow = vec4<f32>(0.0);
    atmosphere.distance = vec4<f32>(1.0, 1.0, 1.0, 0.0);
    atmosphere.light_norm = vec4<f32>(0.0, 0.0, 1.0, 1.0);
    atmosphere.water_specular = vec4<f32>(1.0, 1.0, 1.0, 0.0);
    atmosphere.dome = vec4<f32>(15000.0, 14400.0, 0.0, 0.0);
    atmosphere.legacy_lighting = vec4<f32>(0.0);
    let identity = mat4x4<f32>(
        vec4<f32>(1.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 1.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 1.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 1.0),
    );
    let wave = vec3<f32>(1.0, 0.0, 0.0);
    return shade_water(
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(1.0, 0.0, 0.0),
        identity,
        wave,
        wave,
        wave,
        water,
        atmosphere,
    );
}

@compute @workgroup_size(1)
fn run_water_probe() {
    output[0] = shade_probe(0.0);
    output[1] = shade_probe(0.1);
}
"#,
            )
            .into(),
        ),
    });
    let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("Water atmosphere regression"),
        entries: &[BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::COMPUTE,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("Water atmosphere regression"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&ComputePipelineDescriptor {
        label: Some("Water atmosphere regression"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("run_water_probe"),
        compilation_options: Default::default(),
        cache: None,
    });
    let output = device.create_buffer(&BufferDescriptor {
        label: Some("Water atmosphere regression output"),
        size: 8 * 4,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("Water atmosphere regression readback"),
        size: 8 * 4,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let binding = device.create_bind_group(&BindGroupDescriptor {
        label: Some("Water atmosphere regression"),
        layout: &layout,
        entries: &[BindGroupEntry { binding: 0, resource: BindingResource::Buffer(output.as_entire_buffer_binding()) }],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &binding, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 8 * 4);
    queue.submit([encoder.finish()]);
    let (send, receive) = mpsc::channel();
    readback.slice(..).map_async(MapMode::Read, move |result| {
        send.send(result).expect("readback receiver remains alive until completion");
    });
    device.poll(PollType::Wait { submission_index: None, timeout: None }).expect("water regression completes");
    receive.recv().expect("readback result arrives").expect("readback maps");
    let mapped = readback.slice(..).get_mapped_range().expect("readback range is mapped");
    let values = mapped.as_chunks::<4>().0.iter().map(|bytes| f32::from_le_bytes(*bytes)).collect::<Vec<_>>();
    assert!(values[0] > 0.88, "water keeps the punctual specular term: {values:?}");
    let attenuation = (-0.2_f32).exp();
    let base_color = (values[4] - attenuation * values[0]) / (1.0 - attenuation);
    assert!((base_color - 0.88).abs() < 0.001, "atmosphere attenuation applies to punctual light only: {values:?}");
    drop(mapped);
    readback.unmap();
}
