use std::{sync::mpsc, time::Instant};

use assets::{Budget, Content, Kind, LoadJob, Priority, Request, Resource, Status};
use futures::executor::block_on;
use utils::AssetId;
use uuid::Uuid;
use wgpu::{
    Backends, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, BindingType,
    BufferBindingType, BufferDescriptor, BufferUsages, ComputePassDescriptor, ComputePipelineDescriptor, DeviceDescriptor,
    InstanceDescriptor, MapMode, PipelineLayoutDescriptor, PollType, ShaderModuleDescriptor, ShaderSource, ShaderStages, TextureSampleType,
    TextureViewDimension,
};

use crate::{texture::CompressionQuality, texture::TextureKey};

#[test]
#[ignore = "requires Vulkan; run with cargo test -p render --lib firestorm_water_waves_sample_the_interpolated_field -- --ignored"]
fn firestorm_water_waves_sample_the_interpolated_field() {
    let instance =
        wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
    let (device, queue) = block_on(async {
        let adapter = instance.request_adapter(&Default::default()).await.expect("Vulkan adapter is available");
        adapter.request_device(&DeviceDescriptor::default()).await.expect("Vulkan device is available")
    });
    let texture = loaded_fixture(0x4f6d_7192_3a4c_5e6f_8091_a2b3_c4d5_e6f7, test_support::texture::RGBA_J2C);
    let image = texture.image().expect("fixture image pixels remain available");
    let key = TextureKey { id: texture.id(), sampler: assets::Sampler::default() };
    let mut textures = crate::texture::Textures::new(&device, &queue);
    textures.request_environment(&texture, 1);
    let job = textures.next_conversion_job().expect("fixture texture upload is admitted");
    textures.complete(job.run(CompressionQuality::Fast));

    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("Firestorm water interpolation sample"),
        source: ShaderSource::Wgsl(
            format!(
                "{}{}{}",
                include_str!("../shaders/atmosphere.wgsl"),
                include_str!("../shaders/water.wgsl"),
                r#"
@group(0) @binding(0) var<storage, read_write> output: array<vec4<f32>, 6>;
@group(1) @binding(0) var normal_image: texture_2d<f32>;
@group(1) @binding(1) var normal_next_image: texture_2d<f32>;
@group(1) @binding(2) var normal_sampler: sampler;

fn sampled_normal(coordinates: WaterCoordinates) -> vec4<f32> {
    return vec4<f32>(textureSampleLevel(normal_image, normal_sampler, coordinates.first, 0.0).xyz * 2.0 - 1.0, 1.0);
}

@compute @workgroup_size(1)
fn probe() {
    var water: WaterSurface;
    water.animation = vec4<f32>(0.0, 128.0, 128.0, 0.0);
    water.wave_directions = vec4<f32>(0.0);
    water.uv_origin0 = vec4<f32>(0.0, 0.0, 0.6, 0.2);
    water.uv_origin1 = vec4<f32>(0.0, 0.0, 128.0, 128.0);
    let region_point = (
        water_sweep_point(vec2<f32>(-128.0, -128.0), water)
        + water_sweep_point(vec2<f32>(128.0, 128.0), water)
    ) * 0.5;
    output[0] = sampled_normal(water_coordinates_from_point(region_point, water));
    output[1] = sampled_normal(water_coordinates_from_point(water_sweep_point(vec2<f32>(0.0), water), water));
    output[2] = sampled_normal(water_coordinates_at(vec3<f32>(0.0), water));
    let boundary = water_coordinates_at(vec3<f32>(128.0, 0.0, 0.0), water);
    let edge_point = (
        water_sweep_point(vec2<f32>(128.0, -128.0), water)
        + water_sweep_point(vec2<f32>(128.0, 128.0), water)
    ) * 0.5;
    let region_edge = water_coordinates_from_point(edge_point, water);
    output[3] = vec4<f32>(boundary.first, region_edge.first);
    output[4] = vec4<f32>(water_coordinates_at(vec3<f32>(127.999, 0.0, 0.0), water).first, 0.0, 0.0);
    output[5] = vec4<f32>(water_coordinates_at(vec3<f32>(128.001, 0.0, 0.0), water).first, 0.0, 0.0);
}
"#,
            )
            .into(),
        ),
    });
    let output_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("Water interpolation output"),
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
    let texture_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("Water raw normal sample"),
        entries: &[
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 2,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("Water interpolation sample"),
        bind_group_layouts: &[Some(&output_layout), Some(&texture_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&ComputePipelineDescriptor {
        label: Some("Water interpolation sample"),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("probe"),
        compilation_options: Default::default(),
        cache: None,
    });
    let output = device.create_buffer(&BufferDescriptor {
        label: Some("Water interpolation output"),
        size: 6 * 16,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("Water interpolation readback"),
        size: 6 * 16,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let output_binding = device.create_bind_group(&BindGroupDescriptor {
        label: Some("Water interpolation output"),
        layout: &output_layout,
        entries: &[BindGroupEntry { binding: 0, resource: BindingResource::Buffer(output.as_entire_buffer_binding()) }],
    });

    let mut encoder = device.create_command_encoder(&Default::default());
    textures.upload(&device, &mut encoder, 1);
    let normal_binding = device.create_bind_group(&BindGroupDescriptor {
        label: Some("Water raw normal sample"),
        layout: &texture_layout,
        entries: &[
            BindGroupEntry { binding: 0, resource: BindingResource::TextureView(textures.raw_view(Some(key))) },
            BindGroupEntry { binding: 1, resource: BindingResource::TextureView(textures.raw_view(Some(key))) },
            BindGroupEntry { binding: 2, resource: BindingResource::Sampler(&textures.water_sampler) },
        ],
    });
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &output_binding, &[]);
        pass.set_bind_group(1, &normal_binding, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 6 * 16);
    queue.submit([encoder.finish()]);
    textures.submitted(&queue);
    let (send, receive) = mpsc::channel();
    readback.slice(..).map_async(MapMode::Read, move |result| {
        send.send(result).expect("water interpolation receiver remains alive until completion");
    });
    device.poll(PollType::Wait { submission_index: None, timeout: None }).expect("water interpolation sample completes");
    receive.recv().expect("water interpolation readback result arrives").expect("water interpolation readback maps");
    let mapped = readback.slice(..).get_mapped_range().expect("water interpolation readback range is mapped");
    let values = mapped.as_chunks::<4>().0.iter().map(|bytes| f32::from_le_bytes(*bytes)).collect::<Vec<_>>();

    let reference_sweep = |x: f64, y: f64| x + ((x * 0.08).cos() + (y * 0.02).sin()) * 6.0;
    let phase = |value: f64, frequency: f64| (value * frequency).rem_euclid(1.0);
    let region_uv =
        [((reference_sweep(0.0, 0.0) + reference_sweep(256.0, 256.0)) * 0.5 - 128.0) * 0.45 + phase(128.0, 0.45), phase(128.0, 0.9)];
    let direct_uv = [(reference_sweep(128.0, 128.0) - 128.0) * 0.45 + phase(128.0, 0.45), phase(128.0, 0.9)];
    let reference_sample = sample_normal(&image, region_uv);
    let direct_sample = sample_normal(&image, direct_uv);
    for channel in 0..3 {
        assert!(
            (values[channel] - reference_sample[channel]).abs() < 0.02,
            "region normal channel {channel}: {values:?} vs {reference_sample:?}"
        );
        assert!((values[8 + channel] - values[channel]).abs() < 0.0001, "void normal differs from region field: {values:?}");
    }
    assert!(
        (values[12] - values[14]).abs() < 0.0001 && (values[13] - values[15]).abs() < 0.0001,
        "region-to-void edge differs: {values:?}"
    );
    assert!(
        (values[16] - values[20]).abs() < 0.005 && (values[17] - values[21]).abs() < 0.005,
        "wave phase jumps at a region boundary: {values:?}"
    );
    let sample_difference = (0..3).map(|channel| (reference_sample[channel] - direct_sample[channel]).abs()).fold(0.0_f32, f32::max);
    assert!(sample_difference > 0.02, "the fixture must expose the nonlinear sweep mismatch: {reference_sample:?} vs {direct_sample:?}");
    drop(mapped);
    readback.unmap();
}

fn loaded_fixture(id: u128, bytes: &[u8]) -> assets::TextureHandle {
    let mut store = assets::Store::new(Budget::default());
    let route = store.register_route().expect("fixture route is admitted");
    let now = Instant::now();
    let resource = Resource { id: AssetId::new(Uuid::from_u128(id)), kind: Kind::Texture };
    let demand = store.request(route, Request::new(resource), Priority::Visible, now).expect("fixture demand is admitted");
    let LoadJob::Fetch(job) = store.next_job(now).expect("fixture fetch is admitted") else {
        panic!("fresh fixture request unexpectedly uses cached source");
    };
    let input = job.complete(Ok(bytes.to_vec())).expect("fixture response fits");
    let completion = store.start_decode(input).expect("fixture decode is admitted").run();
    store.complete(completion, now);
    let Status::Ready(Content::Texture(texture)) = demand.status() else { panic!("fixture texture decodes") };
    texture
}

fn sample_normal(image: &assets::Image, uv: [f64; 2]) -> [f32; 3] {
    let [width, height] = image.size();
    let coordinates = uv.map(|value| value.rem_euclid(1.0));
    let texel = [coordinates[0] * f64::from(width) - 0.5, coordinates[1] * f64::from(height) - 0.5];
    let base = texel.map(|value| value.floor() as i64);
    let fraction = [texel[0] - base[0] as f64, texel[1] - base[1] as f64];
    let sample = |x: i64, y: i64, channel: usize| {
        let x = x.rem_euclid(i64::from(width)) as usize;
        let y = y.rem_euclid(i64::from(height)) as usize;
        f64::from(image.pixels()[(y * width as usize + x) * 4 + channel]) / 255.0
    };
    std::array::from_fn(|channel| {
        let top = sample(base[0], base[1], channel) * (1.0 - fraction[0]) + sample(base[0] + 1, base[1], channel) * fraction[0];
        let bottom = sample(base[0], base[1] + 1, channel) * (1.0 - fraction[0]) + sample(base[0] + 1, base[1] + 1, channel) * fraction[0];
        ((top * (1.0 - fraction[1]) + bottom * fraction[1]) * 2.0 - 1.0) as f32
    })
}
