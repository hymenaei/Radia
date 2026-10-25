use math::{Mat4, Vec4};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor, BindingResource, BindingType, Buffer,
    BufferBindingType, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, CompareFunction, DepthStencilState, Device, Face,
    FragmentState, MultisampleState, PipelineLayoutDescriptor, PrimitiveState, Queue, RenderPass, RenderPipeline, RenderPipelineDescriptor,
    ShaderModuleDescriptor, ShaderSource, ShaderStages, TextureFormat, VertexState,
};

use super::atmosphere::{ATMOSPHERE_FLOATS, atmosphere_values};
use crate::{
    environment::Evaluated,
    scene::{Camera, MAX_REGIONS},
    texture::{TextureKey, Textures},
};

pub(crate) const COLOR: Vec4 = Vec4::new(0.025, 0.09, 0.14, 1.0);
pub(crate) const DEFAULT_FRESNEL: [f32; 4] = [0.5, 0.3999, 0.0, 0.0];
pub(crate) const DEFAULT_NORMAL_SCALE: f32 = 2.0;
pub(crate) const DEFAULT_WAVES: [f32; 4] = [1.04999, -0.42, 1.10999, -1.16];
const REGION_FLOATS: usize = MAX_REGIONS * 4;
const UNIFORM_FLOATS: usize = 16 + 20 + ATMOSPHERE_FLOATS + REGION_FLOATS;
const UNIFORM_BYTES: usize = UNIFORM_FLOATS * 4;

pub(crate) struct Water {
    pipeline: RenderPipeline,
    uniform: Buffer,
    binding: BindGroup,
    pub(crate) active: bool,
    normal: Option<TextureKey>,
}

impl Water {
    pub(crate) fn new(device: &Device, format: TextureFormat, textures: &BindGroupLayout) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Far water and atmosphere"),
            source: ShaderSource::Wgsl(concat!(include_str!("../shaders/atmosphere.wgsl"), include_str!("../shaders/water.wgsl")).into()),
        });
        let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Far water state"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Far water"),
            bind_group_layouts: &[Some(&layout), Some(textures)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Infinite far water"),
            layout: Some(&pipeline_layout),
            vertex: VertexState { module: &shader, entry_point: Some("vertex"), compilation_options: Default::default(), buffers: &[] },
            primitive: PrimitiveState { cull_mode: Some(Face::Back), ..Default::default() },
            depth_stencil: Some(DepthStencilState {
                format: TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: MultisampleState::default(),
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&BufferDescriptor {
            label: Some("Far water state"),
            size: UNIFORM_BYTES as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let binding = device.create_bind_group(&BindGroupDescriptor {
            label: Some("Far water state"),
            layout: &layout,
            entries: &[BindGroupEntry { binding: 0, resource: BindingResource::Buffer(uniform.as_entire_buffer_binding()) }],
        });
        Self { pipeline, uniform, binding, active: false, normal: None }
    }

    pub(crate) fn update(
        &mut self,
        queue: &Queue,
        camera: Camera,
        inverse: Mat4,
        height: Option<f64>,
        environment: Option<&Evaluated>,
        normal: Option<TextureKey>,
        regions: &[Vec4; MAX_REGIONS],
        region_count: usize,
    ) {
        self.active = false;
        self.normal = normal;
        let Some(height) = height
            .map(|height| height - camera.position().z)
            .filter(|height| height.is_finite() && *height < 0.0 && height.abs() < f64::from(f32::MAX) / 16.0)
        else {
            return;
        };
        let count = region_count.min(MAX_REGIONS);
        let far = camera.draw_distance().clamp(0.1, f64::from(f32::MAX) / 16.0) as f32;
        let near = 0.05_f32;
        let water = environment.map(|environment| &environment.water);
        let fog_color = water.map_or(COLOR.to_array(), |water| [water.fog_color[0], water.fog_color[1], water.fog_color[2], 1.0]);
        let parameters = water.map_or(DEFAULT_FRESNEL, |water| {
            [water.fresnel_offset, water.fresnel_scale, water.fog_density, if normal.is_some() { 1.0 } else { 0.0 }]
        });
        let normal_scale = water.map_or(DEFAULT_NORMAL_SCALE, |water| water.normal_scale[0]);
        let waves = water.map_or(DEFAULT_WAVES, |water| [water.waves[0][0], water.waves[0][1], water.waves[1][0], water.waves[1][1]]);
        let mut values = [0.0; UNIFORM_FLOATS];
        let mut offset = 0;
        append(&mut values, &mut offset, inverse.to_cols_array());
        append(&mut values, &mut offset, [height as f32, count as f32, far, near]);
        append(&mut values, &mut offset, fog_color);
        append(&mut values, &mut offset, parameters);
        append(&mut values, &mut offset, [normal_scale, 0.0, 0.0, 0.0]);
        append(&mut values, &mut offset, waves);
        append(&mut values, &mut offset, atmosphere_values(environment, camera.position().z as f32));
        for region in regions {
            append(&mut values, &mut offset, region.to_array());
        }
        let mut bytes = [0; UNIFORM_BYTES];
        for (target, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
            *target = value.to_le_bytes();
        }
        queue.write_buffer(&self.uniform, 0, &bytes);
        self.active = true;
    }

    pub(crate) fn draw(&self, pass: &mut RenderPass<'_>, textures: &Textures) {
        if self.active {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.binding, &[]);
            pass.set_bind_group(1, textures.binding(self.normal), &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

fn append<const N: usize>(target: &mut [f32; UNIFORM_FLOATS], offset: &mut usize, values: [f32; N]) {
    if let Some(slice) = target.get_mut(*offset..(*offset).saturating_add(N)) {
        slice.copy_from_slice(&values);
        *offset += N;
    }
}
