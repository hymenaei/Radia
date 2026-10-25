use math::{DVec3, Mat4, Vec4};
use std::f64::consts::TAU;
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor, BindingResource, BindingType, Buffer, BufferBindingType,
    BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, CompareFunction, DepthStencilState, Device, FragmentState,
    MultisampleState, PipelineLayoutDescriptor, PrimitiveState, Queue, RenderPass, RenderPipeline, RenderPipelineDescriptor,
    ShaderModuleDescriptor, ShaderSource, ShaderStages, TextureFormat, VertexState,
};

use super::atmosphere::{ATMOSPHERE_FLOATS, atmosphere_values};
use super::surface::camera_view;
use crate::{
    environment::Evaluated,
    scene::{Camera, MAX_REGIONS},
    texture::{TextureViewState, Textures},
};

use super::TexturePair;

pub(crate) const COLOR: Vec4 = Vec4::new(0.0156, 0.149, 0.2509, 1.0);
pub(crate) const DEFAULT_FRESNEL: [f32; 4] = [0.5, 0.3999, 0.0, 0.0];
pub(crate) const DEFAULT_NORMAL_SCALE: f32 = 1.75;
pub(crate) const DEFAULT_BLUR: f32 = 0.04;
pub(crate) const DEFAULT_WAVES: [f32; 4] = [1.04999, -0.42, 1.10999, -1.16];
const REGION_FLOATS: usize = MAX_REGIONS * 4;
const UNIFORM_FLOATS: usize = 32 + 36 + ATMOSPHERE_FLOATS + REGION_FLOATS;
const UNIFORM_BYTES: usize = UNIFORM_FLOATS * 4;

pub(crate) struct Water {
    pipeline: RenderPipeline,
    uniform: Buffer,
    binding: BindGroup,
    pub(crate) active: bool,
    normal: TexturePair,
    normal_binding: BindGroup,
    normal_view_states: [Option<TextureViewState>; 2],
}

impl Water {
    pub(crate) fn new(device: &Device, format: TextureFormat, textures: &Textures) -> Self {
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
            bind_group_layouts: &[Some(&layout), Some(&textures.pair_layout)],
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
        let normal_binding = textures.pair_binding(device, None, None);
        Self { pipeline, uniform, binding, active: false, normal: TexturePair::default(), normal_binding, normal_view_states: [None, None] }
    }

    pub(crate) fn update(
        &mut self,
        device: &Device,
        textures: &Textures,
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
        let keys = [normal.current, normal.next];
        let states = keys.map(|key| textures.view_state(key));
        if keys != [self.normal.current, self.normal.next] || states != self.normal_view_states {
            self.normal_binding = textures.pair_binding(device, normal.current, normal.next);
            self.normal_view_states = states;
        }
        self.normal = normal;
        let Some(relative_water_height) = relative_height(height, camera.position().z) else {
            return;
        };
        let count = region_count.min(MAX_REGIONS);
        let far = camera.draw_distance().clamp(0.1, f64::from(f32::MAX) / 16.0) as f32;
        let near = 0.05_f32;
        let normal_present = normal.current.is_some() || normal.next.is_some();
        let mut values = [0.0; UNIFORM_FLOATS];
        let mut offset = 0;
        append(&mut values, &mut offset, inverse.to_cols_array());
        append(&mut values, &mut offset, camera_view(camera).to_cols_array());
        append(&mut values, &mut offset, [relative_water_height, count as f32, far, near]);
        append(
            &mut values,
            &mut offset,
            water_surface_values(environment, camera.position(), height, scroll, normal.factor, normal_present),
        );
        append(&mut values, &mut offset, atmosphere_values(environment));
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

    pub(crate) fn draw(&self, pass: &mut RenderPass<'_>) {
        if self.active {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.binding, &[]);
            pass.set_bind_group(1, &self.normal_binding, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

#[derive(Clone, Copy)]
struct WaterOrigins {
    sweep: [f32; 2],
    big: [f32; 2],
    first: [f32; 2],
    second: [f32; 2],
}

fn water_origin(position: DVec3) -> WaterOrigins {
    let phase = |coordinate: f64, frequency: f64| (coordinate * frequency).rem_euclid(1.0) as f32;
    WaterOrigins {
        sweep: [(position.x.rem_euclid(TAU / 0.08)) as f32, (position.y.rem_euclid(TAU / 0.02)) as f32],
        big: [phase(position.x, 0.04), phase(position.y, 0.04)],
        first: [phase(position.x, 0.45), phase(position.y, 0.9)],
        second: [phase(position.x, 0.1), phase(position.y, 0.2)],
    }
}

pub(crate) fn water_surface_values(
    environment: Option<&Evaluated>,
    camera_position: DVec3,
    water_height: Option<f64>,
    time: [f32; 4],
    normal_blend: f32,
    normal_present: bool,
) -> [f32; 32] {
    let water = environment.map(|environment| &environment.water);
    let fog_color = water.map_or([COLOR.x, COLOR.y, COLOR.z], |water| water.fog_color);
    let density = water.map_or(2.0, |water| water.fog_density);
    let underwater_modifier = water.map_or(0.25, |water| water.underwater_fog);
    let underwater = water_height.is_some_and(|height| camera_position.z < height);
    let fog_density = modified_water_fog_density(density, underwater_modifier, underwater);
    let fog_ks = water_fog_ks(environment);
    let plane_w = water_height.map_or(0.0, |height| camera_position.z as f32 - height as f32);
    let normal_scale = water.map_or([DEFAULT_NORMAL_SCALE; 3], |water| water.normal_scale);
    let blur = water.map_or(DEFAULT_BLUR, |water| water.blur).max(0.0) * 2.0;
    let fresnel = water.map_or(DEFAULT_FRESNEL, |water| [water.fresnel_offset, water.fresnel_scale, 0.0, 0.0]);
    let waves = water.map_or(DEFAULT_WAVES, |water| [water.waves[0][0], water.waves[0][1], water.waves[1][0], water.waves[1][1]]);
    let origin = water_origin(camera_position);
    let mut values = [0.0; 32];
    values[..4].copy_from_slice(&[fog_color[0], fog_color[1], fog_color[2], if water_height.is_some() { 1.0 } else { 0.0 }]);
    values[4..8].copy_from_slice(&[fog_density, fog_ks, plane_w, if underwater { 1.0 } else { 0.0 }]);
    values[8..12].copy_from_slice(&[normal_scale[0], normal_scale[1], normal_scale[2], blur]);
    values[12..16].copy_from_slice(&[fresnel[0], fresnel[1], normal_blend, if normal_present { 1.0 } else { 0.0 }]);
    values[16..20].copy_from_slice(&waves);
    values[20..24].copy_from_slice(&[time[0], origin.sweep[0], origin.sweep[1], camera_position.z as f32]);
    values[24..28].copy_from_slice(&[origin.big[0], origin.big[1], origin.first[0], origin.first[1]]);
    values[28..32].copy_from_slice(&[origin.second[0], origin.second[1], 0.0, 0.0]);
    values
}

pub(crate) fn modified_water_fog_density(density: f32, modifier: f32, underwater: bool) -> f32 {
    if !underwater || modifier <= 0.0 {
        return density;
    }
    density.powf(modifier)
}

pub(crate) fn water_fog_values(environment: Option<&Evaluated>, camera_position: DVec3, water_height: Option<f64>) -> [f32; 8] {
    water_surface_values(environment, camera_position, water_height, [0.0; 4], 0.0, false)[..8]
        .try_into()
        .expect("water fog uses two four-component values")
}

fn water_fog_ks(environment: Option<&Evaluated>) -> f32 {
    let Some(environment) = environment else { return 1.0 / 0.3 };
    let sun_up = environment.sky.sun_direction[2] >= 0.0;
    let moon_up = environment.sky.moon_direction[2] >= 0.0;
    let light_direction = if sun_up {
        environment.sky.sun_direction
    } else if moon_up {
        environment.sky.moon_direction
    } else {
        [0.0, 0.0, -1.0]
    };
    1.0 / light_direction[2].max(-0.1).max(0.3)
}

fn relative_height(height: Option<f64>, camera_z: f64) -> Option<f32> {
    let height = height? - camera_z;
    if !height.is_finite() || height == 0.0 || height.abs() >= f64::from(f32::MAX) / 16.0 {
        return None;
    }
    let height = height as f32;
    (height.is_finite() && height != 0.0).then_some(height)
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
    use crate::environment::{Schedule, Sources, evaluate};

    #[test]
    fn water_plane_keeps_both_sides() {
        assert_eq!(relative_height(Some(25.0), 10.0), Some(15.0));
        assert_eq!(relative_height(Some(5.0), 10.0), Some(-5.0));
        assert_eq!(relative_height(Some(10.0), 10.0), None);
        assert_eq!(relative_height(None, 10.0), None);
    }

    #[test]
    fn water_uniforms_match_windlight_settings_and_fog_rules() {
        let environment = evaluate(
            &Sources {
                base: None,
                schedule: Schedule::default(),
                experience_day: None,
                experience_sky: None,
                experience_water: None,
                sky_override: None,
                water_override: None,
            },
            0.0,
            0.0,
        );
        let above = water_surface_values(Some(&environment), DVec3::new(25.0, 10.0, 20.0), Some(10.0), [5.0; 4], 0.25, true);
        assert_eq!(&above[4..8], &[2.0, 1.0 / environment.sky.sun_direction[2].max(0.3), 10.0, 0.0]);
        assert_eq!(&above[8..16], &[1.75, 1.75, 1.75, 0.08, 0.5, 0.3999, 0.25, 1.0]);
        assert_eq!(&above[16..24], &[1.04999, -0.42, 1.10999, -1.16, 5.0, 25.0, 10.0, 20.0]);
        assert_eq!(&above[24..32], &[0.0, 0.4, 0.25, 0.0, 0.5, 0.0, 0.0, 0.0]);
        let below = water_surface_values(Some(&environment), DVec3::new(25.0, 10.0, 0.0), Some(10.0), [0.0; 4], 0.0, false);
        assert!((below[4] - 2.0_f32.powf(0.25)).abs() < 0.000001);
        assert_eq!(below[6], -10.0);
        assert_eq!(below[7], 1.0);
        assert_eq!(modified_water_fog_density(2.0, 0.25, false), 2.0);
        assert_eq!(modified_water_fog_density(2.0, 0.5, true), 2.0_f32.sqrt());
        assert_eq!(modified_water_fog_density(2.0, 2.0, true), 4.0);
    }
}
