use std::sync::mpsc;

use futures::executor::block_on;
use math::{DVec3, Vec3};
use wgpu::{
    Backends, BufferDescriptor, BufferUsages, Color, DeviceDescriptor, Extent3d, InstanceDescriptor, LoadOp, MapMode, Operations, PollType,
    RenderPassColorAttachment, RenderPassDepthStencilAttachment, RenderPassDescriptor, StoreOp, TexelCopyBufferInfo, TexelCopyBufferLayout,
    TexelCopyTextureInfo, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};

use super::{GeometryRenderer, fixtures};
use crate::scene::Camera;

const SIZE: [u32; 2] = [128, 128];
const RGB_TOLERANCE: u8 = 2;

#[test]
#[ignore = "requires a Vulkan adapter; run with cargo test -p render --lib blend_preserves_scene_alpha -- --ignored"]
fn blend_preserves_scene_alpha() {
    let (device, queue) = vulkan_device();
    let water = [0, 125, 130, 255].map(|alpha| render_center(&device, &queue, alpha, true));
    let water_background = water[0];
    let water_opaque = water[3];

    for (pixel, alpha) in [(water[1], 125), (water[2], 130)] {
        assert_eq!(pixel[3], 0, "opacity {alpha}/255 preserves water classification");
        assert_blended_rgb(pixel, water_background, water_opaque, alpha);
    }

    let opaque_background = render_center(&device, &queue, 125, false);
    assert_eq!(opaque_background[3], 255, "transparent surface preserves opaque background classification");
}

fn vulkan_device() -> (wgpu::Device, wgpu::Queue) {
    block_on(async {
        let instance =
            wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let adapter = instance.request_adapter(&Default::default()).await.expect("Vulkan adapter is available");
        adapter.request_device(&DeviceDescriptor::default()).await.expect("Vulkan device is available")
    })
}

fn render_center(device: &wgpu::Device, queue: &wgpu::Queue, alpha: u8, with_water: bool) -> [u8; 4] {
    let (_scene, view, scene_camera) = if with_water { fixtures::water_scene(alpha) } else { fixtures::scene(alpha) };
    let camera = if with_water {
        Camera::new(DVec3::new(10.0, -1.35, -5.0), Vec3::Z, 1.0, SIZE).expect("water camera is valid")
    } else {
        scene_camera
    };
    let mut geometry = GeometryRenderer::new(device, queue, TextureFormat::Rgba8Unorm);
    let depth = geometry.depth(device, SIZE).expect("depth target is admitted");
    let frame = geometry.admit().expect("frame is admitted");
    let mut encoder = device.create_command_encoder(&Default::default());
    geometry.prepare(device, &mut encoder, &view, None, camera, SIZE, with_water.then_some(20.0));
    let color = device.create_texture(&TextureDescriptor {
        label: Some("Surface alpha regression"),
        size: Extent3d { width: SIZE[0], height: SIZE[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color.create_view(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Surface alpha regression"),
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
        geometry.draw(&mut pass);
    }
    let bytes_per_row = SIZE[0] * 4;
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("Surface alpha regression readback"),
        size: u64::from(bytes_per_row) * u64::from(SIZE[1]),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo { texture: &color, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(SIZE[1]) },
        },
        Extent3d { width: SIZE[0], height: SIZE[1], depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    geometry.submitted(frame);
    let (send, receive) = mpsc::channel();
    readback.slice(..).map_async(MapMode::Read, move |result| {
        send.send(result).expect("readback receiver remains alive until completion");
    });
    device.poll(PollType::Wait { submission_index: None, timeout: None }).expect("render completes");
    receive.recv().expect("readback result arrives").expect("readback maps");
    let mapped = readback.slice(..).get_mapped_range().expect("readback range is mapped");
    let center = ((SIZE[1] / 2 * SIZE[0] + SIZE[0] / 2) * 4) as usize;
    let pixel: [u8; 4] =
        mapped.get(center..center + 4).expect("center pixel is in the readback").try_into().expect("pixel has four channels");
    drop(mapped);
    readback.unmap();
    pixel
}

fn assert_blended_rgb(pixel: [u8; 4], background: [u8; 4], surface: [u8; 4], alpha: u8) {
    let opacity = f32::from(alpha) / f32::from(u8::MAX);
    for channel in 0..3 {
        let expected = (f32::from(background[channel]) * (1.0 - opacity) + f32::from(surface[channel]) * opacity).round() as u8;
        assert!(
            pixel[channel].abs_diff(expected) <= RGB_TOLERANCE,
            "opacity {alpha}/255 channel {channel}: expected {expected}, got {}",
            pixel[channel]
        );
    }
}
