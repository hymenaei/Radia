use std::num::NonZeroU64;

use crate::environment::Evaluated;
use math::{DVec3, Mat4};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource,
    BindingType, Buffer, BufferBindingType, BufferDescriptor, BufferUsages, Color, ColorTargetState, ColorWrites, CommandEncoderDescriptor,
    Device, Extent3d, FragmentState, LoadOp, Operations, PipelineLayoutDescriptor, PrimitiveState, Queue, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipeline, RenderPipelineDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages, StoreOp, Texture,
    TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureView, TextureViewDescriptor, VertexState,
};

use super::{atmosphere::sky_hdr_scale, water::water_fog_values};

const HDR_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
const EXPOSURE_FLOATS: usize = 32;
const EXPOSURE_BYTES: usize = EXPOSURE_FLOATS * 4;
const TONE_FLOATS: usize = 28;
const TONE_BYTES: usize = TONE_FLOATS * 4;

pub(crate) struct PostProcess {
    exposure_pipeline: RenderPipeline,
    tone_pipeline: RenderPipeline,
    exposure_uniform: Buffer,
    tone_uniform: Buffer,
    source_layout: BindGroupLayout,
    exposure_uniform_binding: BindGroup,
    tone_uniform_binding: BindGroup,
    size: [u32; 2],
    _scene_texture: Option<Texture>,
    scene_view: Option<TextureView>,
    _exposure_textures: [Option<Texture>; 2],
    exposure_views: [Option<TextureView>; 2],
    exposure_bindings: [Option<BindGroup>; 2],
    tone_bindings: [Option<BindGroup>; 2],
    _dummy_depth: Texture,
    dummy_depth_view: TextureView,
    current_exposure: usize,
    has_history: bool,
}

impl PostProcess {
    pub(crate) fn new(device: &Device, queue: &Queue, surface_format: TextureFormat) -> Self {
        let exposure_shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Environment exposure"),
            source: ShaderSource::Wgsl(
                concat!(
                    include_str!("../shaders/atmosphere.wgsl"),
                    include_str!("../shaders/water.wgsl"),
                    include_str!("../shaders/exposure.wgsl")
                )
                .into(),
            ),
        });
        let tone_shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Environment tonemap"),
            source: ShaderSource::Wgsl(
                concat!(
                    include_str!("../shaders/atmosphere.wgsl"),
                    include_str!("../shaders/water.wgsl"),
                    include_str!("../shaders/tonemap.wgsl")
                )
                .into(),
            ),
        });
        let exposure_layout = uniform_layout(device, "Exposure settings", EXPOSURE_BYTES);
        let tone_layout = uniform_layout(device, "Tonemap settings", TONE_BYTES);
        let source_layout = source_layout(device);
        let exposure_pipeline =
            fullscreen_pipeline(device, "Environment exposure", &exposure_shader, &exposure_layout, &source_layout, HDR_FORMAT);
        let tone_pipeline = fullscreen_pipeline(device, "Environment tonemap", &tone_shader, &tone_layout, &source_layout, surface_format);
        let exposure_uniform = uniform_buffer(device, "Exposure settings", EXPOSURE_BYTES);
        let tone_uniform = uniform_buffer(device, "Tonemap settings", TONE_BYTES);
        let exposure_uniform_binding = uniform_binding(device, "Exposure settings", &exposure_layout, &exposure_uniform);
        let tone_uniform_binding = uniform_binding(device, "Tonemap settings", &tone_layout, &tone_uniform);
        let dummy_depth = device.create_texture(&TextureDescriptor {
            label: Some("Empty scene depth"),
            size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Depth32Float,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let dummy_depth_view = dummy_depth.create_view(&TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("Initialize empty scene depth") });
        {
            let _pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("Initialize empty scene depth"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &dummy_depth_view,
                    depth_ops: Some(Operations { load: LoadOp::Clear(0.0), store: StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
        }
        queue.submit([encoder.finish()]);
        let mut postprocess = Self {
            exposure_pipeline,
            tone_pipeline,
            exposure_uniform,
            tone_uniform,
            source_layout,
            exposure_uniform_binding,
            tone_uniform_binding,
            size: [0, 0],
            _scene_texture: None,
            scene_view: None,
            _exposure_textures: [None, None],
            exposure_views: [None, None],
            exposure_bindings: [None, None],
            tone_bindings: [None, None],
            _dummy_depth: dummy_depth,
            dummy_depth_view,
            current_exposure: 0,
            has_history: false,
        };
        postprocess.resize(device, [1, 1]);
        postprocess
    }

    pub(crate) fn scene_view(&self) -> &TextureView {
        self.scene_view.as_ref().expect("post-process targets are initialized before rendering")
    }

    pub(crate) fn resize(&mut self, device: &Device, size: [u32; 2]) {
        if size.contains(&0) || self.size == size {
            return;
        }
        let scene_texture = render_texture(device, "Linear scene color", size, HDR_FORMAT);
        let scene_view = scene_texture.create_view(&TextureViewDescriptor::default());
        let exposure_textures = [
            render_texture(device, "Exposure history A", [1, 1], HDR_FORMAT),
            render_texture(device, "Exposure history B", [1, 1], HDR_FORMAT),
        ];
        let exposure_views = exposure_textures.each_ref().map(|texture| texture.create_view(&TextureViewDescriptor::default()));
        let exposure_bindings = std::array::from_fn(|index| {
            Some(source_binding(
                device,
                "Exposure inputs",
                &self.source_layout,
                &scene_view,
                exposure_views.get(1 - index).expect("two exposure histories are present"),
                &self.dummy_depth_view,
            ))
        });
        let tone_bindings = std::array::from_fn(|index| {
            Some(source_binding(
                device,
                "Tonemap inputs",
                &self.source_layout,
                &scene_view,
                exposure_views.get(index).expect("two exposure histories are present"),
                &self.dummy_depth_view,
            ))
        });
        self._scene_texture = Some(scene_texture);
        self.scene_view = Some(scene_view);
        self._exposure_textures = exposure_textures.map(Some);
        self.exposure_views = exposure_views.map(Some);
        self.exposure_bindings = exposure_bindings;
        self.tone_bindings = tone_bindings;
        self.current_exposure = 0;
        self.has_history = false;
        self.size = size;
    }

    pub(crate) fn set_depth_view(&mut self, device: &Device, depth: Option<&TextureView>) {
        let depth = depth.unwrap_or(&self.dummy_depth_view);
        let Some(scene_view) = self.scene_view.as_ref() else { return };
        self.exposure_bindings = std::array::from_fn(|index| {
            Some(source_binding(
                device,
                "Exposure inputs",
                &self.source_layout,
                scene_view,
                self.exposure_views[1 - index].as_ref().expect("exposure history is initialized"),
                depth,
            ))
        });
        self.tone_bindings = std::array::from_fn(|index| {
            Some(source_binding(
                device,
                "Tonemap inputs",
                &self.source_layout,
                scene_view,
                self.exposure_views[index].as_ref().expect("exposure history is initialized"),
                depth,
            ))
        });
    }

    pub(crate) fn render(
        &mut self,
        queue: &Queue,
        encoder: &mut wgpu::CommandEncoder,
        output: &TextureView,
        environment: Option<&Evaluated>,
        delta_seconds: f32,
        camera_position: Option<DVec3>,
        water_height: Option<f64>,
        inverse_view_projection: Mat4,
    ) {
        let (minimum_exposure, maximum_exposure, tonemap_mix) = environment_values(environment);
        let fog = camera_position.map_or([0.0; 8], |position| water_fog_values(environment, position, water_height));
        let mut exposure = [0.0; EXPOSURE_FLOATS];
        exposure[..8].copy_from_slice(&[
            0.5,
            minimum_exposure,
            maximum_exposure,
            0.1,
            delta_seconds,
            2.0,
            if self.has_history { 1.0 } else { 0.0 },
            sky_hdr_scale(environment),
        ]);
        exposure[8..16].copy_from_slice(&fog);
        exposure[16..].copy_from_slice(&inverse_view_projection.to_cols_array());
        let mut tone = [0.0; TONE_FLOATS];
        tone[..4].copy_from_slice(&[1.0, tonemap_mix, sky_hdr_scale(environment), 0.0]);
        tone[4..12].copy_from_slice(&fog);
        tone[12..].copy_from_slice(&inverse_view_projection.to_cols_array());
        let mut exposure_bytes = [0; EXPOSURE_BYTES];
        let mut tone_bytes = [0; TONE_BYTES];
        write_values(queue, &self.exposure_uniform, &exposure, &mut exposure_bytes);
        write_values(queue, &self.tone_uniform, &tone, &mut tone_bytes);

        let destination = 1 - self.current_exposure;
        {
            let target = self.exposure_views[destination].as_ref().expect("exposure targets are initialized before rendering");
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("Environment exposure"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations { load: LoadOp::Clear(Color::TRANSPARENT), store: StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.exposure_pipeline);
            pass.set_bind_group(0, &self.exposure_uniform_binding, &[]);
            pass.set_bind_group(
                1,
                self.exposure_bindings[destination].as_ref().expect("exposure inputs are initialized before rendering"),
                &[],
            );
            pass.draw(0..3, 0..1);
        }
        self.current_exposure = destination;
        self.has_history = true;
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("Environment tonemap"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: output,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations { load: LoadOp::Clear(Color::BLACK), store: StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.tone_pipeline);
            pass.set_bind_group(0, &self.tone_uniform_binding, &[]);
            pass.set_bind_group(
                1,
                self.tone_bindings[self.current_exposure].as_ref().expect("tonemap inputs are initialized before rendering"),
                &[],
            );
            pass.draw(0..3, 0..1);
        }
    }
}

fn environment_values(environment: Option<&Evaluated>) -> (f32, f32, f32) {
    let hdr = environment.is_some_and(|environment| environment.sky.reflection_probe_ambiance.is_some_and(|ambiance| ambiance > 0.0));
    if !hdr {
        return (1.0, 1.0, 0.0);
    }
    let gamma = environment.map_or(1.0, |environment| environment.sky.gamma);
    let hdr_scale = gamma.max(0.0).sqrt() * 2.0;
    if hdr_scale > 1.0 { (1.0 / hdr_scale, hdr_scale, 1.0) } else { (1.0, 1.0, 1.0) }
}

fn fullscreen_pipeline(
    device: &Device,
    label: &str,
    shader: &wgpu::ShaderModule,
    settings: &BindGroupLayout,
    source: &BindGroupLayout,
    format: TextureFormat,
) -> RenderPipeline {
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(settings), Some(source)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: VertexState { module: shader, entry_point: Some("vertex"), compilation_options: Default::default(), buffers: &[] },
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(FragmentState {
            module: shader,
            entry_point: Some("fragment"),
            compilation_options: Default::default(),
            targets: &[Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn uniform_layout(device: &Device, label: &str, size: usize) -> BindGroupLayout {
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(size as u64),
            },
            count: None,
        }],
    })
}

fn source_layout(device: &Device) -> BindGroupLayout {
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("Post-process images"),
        entries: &[
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 2,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    })
}

fn source_binding(
    device: &Device,
    label: &str,
    layout: &BindGroupLayout,
    scene: &TextureView,
    exposure: &TextureView,
    depth: &TextureView,
) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            BindGroupEntry { binding: 0, resource: BindingResource::TextureView(scene) },
            BindGroupEntry { binding: 1, resource: BindingResource::TextureView(exposure) },
            BindGroupEntry { binding: 2, resource: BindingResource::TextureView(depth) },
        ],
    })
}

fn uniform_binding(device: &Device, label: &str, layout: &BindGroupLayout, buffer: &Buffer) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[BindGroupEntry { binding: 0, resource: BindingResource::Buffer(buffer.as_entire_buffer_binding()) }],
    })
}

fn uniform_buffer(device: &Device, label: &str, size: usize) -> Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: size as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn render_texture(device: &Device, label: &str, size: [u32; 2], format: TextureFormat) -> Texture {
    device.create_texture(&TextureDescriptor {
        label: Some(label),
        size: Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn write_values<const N: usize>(queue: &Queue, buffer: &Buffer, values: &[f32; N], bytes: &mut [u8]) {
    for (target, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        *target = value.to_le_bytes();
    }
    queue.write_buffer(buffer, 0, bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::{Schedule, Sources, evaluate};

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
        assert_eq!(mix, 1.0);
    }
}
