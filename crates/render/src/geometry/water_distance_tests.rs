use std::sync::mpsc;

use assets::EnvironmentDescription;
use futures::executor::block_on;
use math::{DVec3, Vec3};
use wgpu::{
    Backends, BufferDescriptor, BufferUsages, Color, Device, DeviceDescriptor, Extent3d, InstanceDescriptor, LoadOp, MapMode, Operations,
    PollType, RenderPassColorAttachment, RenderPassDepthStencilAttachment, RenderPassDescriptor, StoreOp, TexelCopyBufferInfo,
    TexelCopyBufferLayout, TexelCopyTextureInfo, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
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

    for (preset, environment) in ["midday", "sunset", "midnight"].into_iter().zip(&presets) {
        for height in [0.5, 2.0, 20.0, 200.0] {
            let mut render = |draw_distance| {
                let camera = Camera::new(DVec3::new(128.0, 128.0, WATER_HEIGHT + height), Vec3::X, 1.0, SIZE)
                    .expect("camera is valid")
                    .with_quality(draw_distance, 1.0)
                    .expect("draw distance is valid");
                render_frame(&device, &queue, &mut geometry, &view, camera, environment)
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
) -> Vec<u8> {
    let mut postprocess = PostProcess::new(device, queue, TextureFormat::Rgba8UnormSrgb);
    postprocess.resize(device, SIZE);
    let depth = geometry.depth(device, SIZE).expect("depth target is admitted");
    postprocess.set_depth_view(device, Some(&depth));
    let frame = FrameInput {
        water_height: Some(WATER_HEIGHT),
        environment: Some(environment),
        cloud_scroll: [0.0; 2],
        animation_time: 1.0,
        water_scroll: [0.0; 4],
    };
    let mut encoder = device.create_command_encoder(&Default::default());
    let admission = geometry.admit().expect("frame is admitted");
    geometry.prepare(device, &mut encoder, view, camera, SIZE, frame);
    while let Some(job) = geometry.next_texture_job() {
        geometry.complete_texture(job.run(CompressionQuality::Fast));
    }
    geometry.prepare(device, &mut encoder, view, camera, SIZE, frame);
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
        Some(WATER_HEIGHT),
        projection(camera, SIZE).inverse(),
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
        let _ = send.send(result);
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
