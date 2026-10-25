use math::{DVec3, Mat4, Vec4};
use std::f64::consts::TAU;
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor, BindingResource, BindingType, Buffer,
    BufferBindingType, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, CompareFunction, DepthStencilState, Device,
    FragmentState, MultisampleState, PipelineLayoutDescriptor, PrimitiveState, Queue, RenderPass, RenderPipeline, RenderPipelineDescriptor,
    ShaderModuleDescriptor, ShaderSource, ShaderStages, TextureFormat, VertexState,
};

use super::atmosphere::{ATMOSPHERE_FLOATS, atmosphere_values};
use crate::{
    environment::Evaluated,
    scene::{Camera, MAX_REGIONS},
    texture::Textures,
};

use super::TexturePair;

pub(crate) const COLOR: Vec4 = Vec4::new(0.025, 0.09, 0.14, 1.0);
pub(crate) const DEFAULT_FRESNEL: [f32; 4] = [0.5, 0.3999, 0.0, 0.0];
pub(crate) const DEFAULT_NORMAL_SCALE: f32 = 2.0;
pub(crate) const DEFAULT_SCALE_ABOVE: f32 = 0.0299;
pub(crate) const DEFAULT_BLUR: f32 = 0.04;
pub(crate) const DEFAULT_WAVES: [f32; 4] = [1.04999, -0.42, 1.10999, -1.16];
const REGION_FLOATS: usize = MAX_REGIONS * 4;
const UNIFORM_FLOATS: usize = 16 + 36 + ATMOSPHERE_FLOATS + REGION_FLOATS;
const UNIFORM_BYTES: usize = UNIFORM_FLOATS * 4;

pub(crate) struct Water {
    pipeline: RenderPipeline,
    uniform: Buffer,
    binding: BindGroup,
    pub(crate) active: bool,
    normal: TexturePair,
}

impl Water {
    pub(crate) fn new(device: &Device, format: TextureFormat, textures: &BindGroupLayout) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Far water and atmosphere"),
            source: ShaderSource::Wgsl(
                concat!(
                    include_str!("../shaders/atmosphere.wgsl"),
                    include_str!("../shaders/water_common.wgsl"),
                    include_str!("../shaders/water.wgsl")
                )
                .into(),
            ),
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
            bind_group_layouts: &[Some(&layout), Some(textures), Some(textures)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Infinite far water"),
            layout: Some(&pipeline_layout),
            vertex: VertexState { module: &shader, entry_point: Some("vertex"), compilation_options: Default::default(), buffers: &[] },
            primitive: PrimitiveState::default(),
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
        Self { pipeline, uniform, binding, active: false, normal: TexturePair::default() }
    }

    pub(crate) fn update(
        &mut self,
        queue: &Queue,
        camera: Camera,
        inverse: Mat4,
        height: Option<f64>,
        environment: Option<&Evaluated>,
        normal: TexturePair,
        scroll: [f32; 4],
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
        let normal_present = normal.current.is_some() || normal.next.is_some();
        let scale_above = water.map_or(DEFAULT_SCALE_ABOVE, |water| water.scale_above);
        let origin = water_origin(camera.position(), scale_above);
        let parameters = water.map_or(DEFAULT_FRESNEL, |water| {
            [water.fresnel_offset, water.fresnel_scale, water.fog_density, if normal_present { 1.0 } else { 0.0 }]
        });
        let normal_scale = water.map_or([DEFAULT_NORMAL_SCALE; 3], |water| water.normal_scale);
        let waves = water.map_or(DEFAULT_WAVES, |water| [water.waves[0][0], water.waves[0][1], water.waves[1][0], water.waves[1][1]]);
        let mut values = [0.0; UNIFORM_FLOATS];
        let mut offset = 0;
        append(&mut values, &mut offset, inverse.to_cols_array());
        append(&mut values, &mut offset, [height as f32, count as f32, far, near]);
        append(&mut values, &mut offset, fog_color);
        append(&mut values, &mut offset, parameters);
        append(
            &mut values,
            &mut offset,
            [normal_scale[0], normal_scale[1], normal_scale[2], water.map_or(DEFAULT_SCALE_ABOVE, |water| water.scale_above)],
        );
        append(&mut values, &mut offset, waves);
        append(&mut values, &mut offset, scroll);
        append(&mut values, &mut offset, [water.map_or(DEFAULT_BLUR, |water| water.blur), normal.factor, 0.0, 0.0]);
        append(&mut values, &mut offset, [origin[0], origin[1], origin[2], origin[3]]);
        append(&mut values, &mut offset, [origin[4], origin[5], origin[6], origin[7]]);
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
            pass.set_bind_group(1, textures.binding(self.normal.current), &[]);
            pass.set_bind_group(2, textures.binding(self.normal.next), &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

pub(crate) fn water_origin(position: DVec3, scale_above: f32) -> [f32; 8] {
    let scale = if scale_above.is_finite() { (scale_above / DEFAULT_SCALE_ABOVE).clamp(0.0, 4.0) } else { 1.0 };
    let position_period = TAU / 0.02;
    let phase = |coordinate: f64, frequency: f64| {
        if frequency <= 0.000_001 { 0.0 } else { (coordinate.rem_euclid(1.0 / frequency) * frequency).rem_euclid(1.0) as f32 }
    };
    [
        position.x.rem_euclid(position_period) as f32,
        position.y.rem_euclid(position_period) as f32,
        0.0,
        0.0,
        phase(position.x, 0.45 * f64::from(scale)),
        phase(position.y, 0.9 * f64::from(scale)),
        phase(position.x, 0.1 * f64::from(scale)),
        phase(position.y, 0.2 * f64::from(scale)),
    ]
}

fn append<const N: usize>(target: &mut [f32; UNIFORM_FLOATS], offset: &mut usize, values: [f32; N]) {
    if let Some(slice) = target.get_mut(*offset..(*offset).saturating_add(N)) {
        slice.copy_from_slice(&values);
        *offset += N;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn water_texture_phase_survives_camera_rebasing() {
        let world = DVec3::new(31_004.25, -58_113.75, 0.0);
        let first_camera = DVec3::new(31_000.0, -58_100.0, 20.0);
        let second_camera = DVec3::new(31_120.0, -58_040.0, 20.0);
        let first_origin = water_origin(first_camera, DEFAULT_SCALE_ABOVE);
        let second_origin = water_origin(second_camera, DEFAULT_SCALE_ABOVE);
        let first = [
            ((world.x - first_camera.x) * 0.45 + f64::from(first_origin[4])).rem_euclid(1.0),
            ((world.y - first_camera.y) * 0.9 + f64::from(first_origin[5])).rem_euclid(1.0),
        ];
        let second = [
            ((world.x - second_camera.x) * 0.45 + f64::from(second_origin[4])).rem_euclid(1.0),
            ((world.y - second_camera.y) * 0.9 + f64::from(second_origin[5])).rem_euclid(1.0),
        ];
        for (first, second) in first.into_iter().zip(second) {
            let distance = (first - second).abs().min(1.0 - (first - second).abs());
            assert!(distance < 0.00001);
        }
    }
}
