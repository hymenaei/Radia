use math::{Mat4, Vec4};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor, BindingResource, BindingType, Buffer, BufferBindingType,
    BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, CompareFunction, DepthStencilState, Device, FragmentState,
    MultisampleState, PipelineLayoutDescriptor, PrimitiveState, Queue, RenderPass, RenderPipeline, RenderPipelineDescriptor,
    ShaderModuleDescriptor, ShaderSource, ShaderStages, TextureFormat, VertexState,
};

use super::atmosphere::{ATMOSPHERE_FLOATS, atmosphere_values};
use super::surface::{CAMERA_NEAR_PLANE, camera_view};
use super::water::water_surface_values;
use crate::{
    environment::Evaluated,
    scene::{Camera, MAX_REGIONS},
    texture::{TextureViewState, Textures},
};

use super::TexturePair;

const VOID_WATER_REGION_FLOATS: usize = MAX_REGIONS * 4;
const VOID_WATER_UNIFORM_FLOATS: usize = 32 + 36 + ATMOSPHERE_FLOATS + VOID_WATER_REGION_FLOATS;
const VOID_WATER_UNIFORM_BYTES: usize = VOID_WATER_UNIFORM_FLOATS * 4;

pub(crate) struct VoidWater {
    pipeline: RenderPipeline,
    uniform: Buffer,
    binding: BindGroup,
    pub(crate) active: bool,
    normal_map: TexturePair,
    normal_map_binding: BindGroup,
    normal_map_view_states: [Option<TextureViewState>; 2],
}

impl VoidWater {
    pub(crate) fn new(device: &Device, format: TextureFormat, textures: &Textures) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Void water and atmosphere"),
            source: ShaderSource::Wgsl(
                concat!(
                    include_str!("../shaders/atmosphere.wgsl"),
                    include_str!("../shaders/water.wgsl"),
                    include_str!("../shaders/void_water.wgsl")
                )
                .into(),
            ),
        });
        let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Void water state"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Void water"),
            bind_group_layouts: &[Some(&layout), Some(&textures.pair_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Void water surface"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("void_water_vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
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
                entry_point: Some("void_water_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&BufferDescriptor {
            label: Some("Void water state"),
            size: VOID_WATER_UNIFORM_BYTES as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let binding = device.create_bind_group(&BindGroupDescriptor {
            label: Some("Void water state"),
            layout: &layout,
            entries: &[BindGroupEntry { binding: 0, resource: BindingResource::Buffer(uniform.as_entire_buffer_binding()) }],
        });
        let normal_map_binding = textures.water_pair_binding(device, None, None);
        Self {
            pipeline,
            uniform,
            binding,
            active: false,
            normal_map: TexturePair::default(),
            normal_map_binding,
            normal_map_view_states: [None, None],
        }
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
        normal_map: TexturePair,
        scroll: [f32; 4],
        region_water_bounds: &[Vec4; MAX_REGIONS],
        region_water_count: usize,
    ) {
        self.active = false;
        let keys = [normal_map.current, normal_map.next];
        let states = keys.map(|key| textures.view_state(key));
        if keys != [self.normal_map.current, self.normal_map.next] || states != self.normal_map_view_states {
            self.normal_map_binding = textures.water_pair_binding(device, normal_map.current, normal_map.next);
            self.normal_map_view_states = states;
        }
        self.normal_map = normal_map;
        let Some(relative_water_height) = relative_water_height(height, camera.position().z) else {
            return;
        };
        let count = region_water_count.min(MAX_REGIONS);
        let near = CAMERA_NEAR_PLANE;
        let normal_present = normal_map.current.is_some() || normal_map.next.is_some();
        let mut values = [0.0; VOID_WATER_UNIFORM_FLOATS];
        let mut offset = 0;
        append(&mut values, &mut offset, inverse.to_cols_array());
        append(&mut values, &mut offset, camera_view(camera).to_cols_array());
        append(&mut values, &mut offset, [relative_water_height, count as f32, near, 0.0]);
        append(
            &mut values,
            &mut offset,
            water_surface_values(environment, camera.position(), height, scroll, normal_map.factor, normal_present),
        );
        append(&mut values, &mut offset, atmosphere_values(environment));
        for region in region_water_bounds {
            append(&mut values, &mut offset, region.to_array());
        }
        let mut bytes = [0; VOID_WATER_UNIFORM_BYTES];
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
            pass.set_bind_group(1, &self.normal_map_binding, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

fn relative_water_height(height: Option<f64>, camera_z: f64) -> Option<f32> {
    let height = height? - camera_z;
    if !height.is_finite() || height == 0.0 || height.abs() >= f64::from(f32::MAX) / 16.0 {
        return None;
    }
    let height = height as f32;
    (height.is_finite() && height != 0.0).then_some(height)
}

fn append<const N: usize>(target: &mut [f32; VOID_WATER_UNIFORM_FLOATS], offset: &mut usize, values: [f32; N]) {
    if let Some(slice) = target.get_mut(*offset..(*offset).saturating_add(N)) {
        slice.copy_from_slice(&values);
        *offset += N;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use math::{DVec3, Vec3};

    use crate::pipeline::projection;

    #[test]
    fn relative_height_keeps_both_sides() {
        assert_eq!(relative_water_height(Some(25.0), 10.0), Some(15.0));
        assert_eq!(relative_water_height(Some(5.0), 10.0), Some(-5.0));
        assert_eq!(relative_water_height(Some(10.0), 10.0), None);
        assert_eq!(relative_water_height(None, 10.0), None);
    }

    #[test]
    fn inverse_view_projection_returns_world_space_water_ray() {
        let direction = Vec3::new(1.0, 0.25, -1.0).normalize();
        let camera = Camera::new(DVec3::new(128.0, 64.0, 60.0), direction, 1.0, [1280, 720]).unwrap();
        let inverse = projection(camera, [1280, 720]).inverse();
        let near = inverse * Vec4::new(0.0, 0.0, 1.0, 1.0);
        let world_direction = (near.truncate() / near.w).normalize();
        assert!((world_direction - camera.direction()).length() < 0.00001);
    }
}
