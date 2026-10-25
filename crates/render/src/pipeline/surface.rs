use math::camera::rh::{proj::directx::perspective, view::look_to_mat4};
use math::{DVec3, Mat4, Vec3};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource,
    BindingType, BlendState, Buffer, BufferBindingType, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, CompareFunction,
    DepthStencilState, Device, Face, FragmentState, FrontFace, MultisampleState, PipelineLayoutDescriptor, PrimitiveState, Queue,
    RenderPass, RenderPipeline, RenderPipelineDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages, TextureFormat,
    VertexAttribute, VertexBufferLayout, VertexFormat, VertexState, VertexStepMode,
};

use crate::scene::Camera;

use crate::{environment::Evaluated, texture::Textures};

use super::TexturePair;

use super::{
    atmosphere::{ATMOSPHERE_FLOATS, atmosphere_values},
    water::{COLOR, DEFAULT_BLUR, DEFAULT_FRESNEL, DEFAULT_NORMAL_SCALE, DEFAULT_SCALE_ABOVE, DEFAULT_WAVES, water_origin},
};

pub(crate) const INSTANCE_BYTES: usize = 192;
pub(crate) const VERTEX_BYTES: usize = 32;
const CAMERA_BYTES: usize = 144;
const ATMOSPHERE_BYTES: usize = (ATMOSPHERE_FLOATS + 4) * 4;
const WATER_BYTES: usize = 32 * 4;
const KEY_LIGHT: Vec3 = Vec3::new(-0.3, 0.5, 1.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Pipeline {
    Surface,
    FlippedWinding,
    DoubleSided,
    Blend,
    BlendFlipped,
    BlendDoubleSided,
    Water,
}

pub(crate) struct SurfacePipelines {
    pipeline: RenderPipeline,
    flipped_winding: RenderPipeline,
    double_sided: RenderPipeline,
    blend: RenderPipeline,
    blend_flipped: RenderPipeline,
    blend_double_sided: RenderPipeline,
    water: RenderPipeline,
    camera: Buffer,
    binding: BindGroup,
    atmosphere: Buffer,
    atmosphere_binding: BindGroup,
    water_uniform: Buffer,
    water_binding: BindGroup,
}

impl SurfacePipelines {
    pub(crate) fn new(device: &Device, format: TextureFormat, textures: &BindGroupLayout) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Scene preview"),
            source: ShaderSource::Wgsl(
                concat!(
                    include_str!("../shaders/atmosphere.wgsl"),
                    include_str!("../shaders/water_common.wgsl"),
                    include_str!("../shaders/surface.wgsl")
                )
                .into(),
            ),
        });
        let attributes = [
            VertexAttribute { format: VertexFormat::Float32x4, offset: 0, shader_location: 3 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 16, shader_location: 4 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 32, shader_location: 5 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 48, shader_location: 6 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 64, shader_location: 7 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 80, shader_location: 8 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 96, shader_location: 9 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 112, shader_location: 10 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 128, shader_location: 11 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 144, shader_location: 12 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 160, shader_location: 13 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 176, shader_location: 14 },
        ];
        let camera_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Surface camera"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX_FRAGMENT,
                ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let atmosphere_layout = uniform_layout(device, "Surface atmosphere");
        let water_layout = uniform_layout(device, "Surface water");
        let plain_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Surface pipelines"),
            bind_group_layouts: &[Some(&camera_layout), Some(textures), Some(&atmosphere_layout), Some(&water_layout)],
            immediate_size: 0,
        });
        let region_water_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Region water pipeline"),
            bind_group_layouts: &[
                Some(&camera_layout),
                Some(textures),
                Some(&atmosphere_layout),
                Some(&water_layout),
                Some(textures),
                Some(textures),
            ],
            immediate_size: 0,
        });
        let buffers = [
            VertexBufferLayout {
                array_stride: VERTEX_BYTES as u64,
                step_mode: VertexStepMode::Vertex,
                attributes: &[
                    VertexAttribute { format: VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                    VertexAttribute { format: VertexFormat::Float32x3, offset: 12, shader_location: 1 },
                    VertexAttribute { format: VertexFormat::Float32x2, offset: 24, shader_location: 2 },
                ],
            },
            VertexBufferLayout { array_stride: INSTANCE_BYTES as u64, step_mode: VertexStepMode::Instance, attributes: &attributes },
        ];
        let targets = [Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })];
        let descriptor = RenderPipelineDescriptor {
            label: Some("Opaque scene preview"),
            layout: Some(&plain_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &buffers.map(Some),
            },
            primitive: PrimitiveState { cull_mode: Some(Face::Back), ..Default::default() },
            depth_stencil: Some(DepthStencilState {
                format: TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(CompareFunction::Greater),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: MultisampleState::default(),
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        };
        let pipeline = device.create_render_pipeline(&descriptor);
        let flipped_winding = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Flipped winding scene preview"),
            primitive: PrimitiveState { front_face: FrontFace::Cw, ..descriptor.primitive },
            ..descriptor.clone()
        });
        let double_sided = device.create_render_pipeline(&RenderPipelineDescriptor {
            primitive: PrimitiveState { cull_mode: None, ..descriptor.primitive },
            ..descriptor.clone()
        });
        let blended_targets = [Some(ColorTargetState { format, blend: Some(BlendState::ALPHA_BLENDING), write_mask: ColorWrites::ALL })];
        let blended = RenderPipelineDescriptor {
            label: Some("Blended scene faces"),
            depth_stencil: Some(DepthStencilState {
                depth_write_enabled: Some(false),
                ..descriptor.depth_stencil.clone().expect("scene depth")
            }),
            fragment: Some(FragmentState { targets: &blended_targets, ..descriptor.fragment.clone().expect("scene fragment") }),
            ..descriptor.clone()
        };
        let blend = device.create_render_pipeline(&blended);
        let blend_flipped = device.create_render_pipeline(&RenderPipelineDescriptor {
            primitive: PrimitiveState { front_face: FrontFace::Cw, ..blended.primitive },
            ..blended.clone()
        });
        let blend_double_sided = device.create_render_pipeline(&RenderPipelineDescriptor {
            primitive: PrimitiveState { cull_mode: None, ..blended.primitive },
            ..blended
        });
        let water = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Region water"),
            layout: Some(&region_water_layout),
            vertex: VertexState { entry_point: Some("vertex"), ..descriptor.vertex.clone() },
            fragment: Some(FragmentState { entry_point: Some("water_fragment"), ..descriptor.fragment.clone().expect("scene fragment") }),
            primitive: PrimitiveState { cull_mode: None, ..descriptor.primitive },
            depth_stencil: Some(DepthStencilState {
                depth_compare: Some(CompareFunction::GreaterEqual),
                ..descriptor.depth_stencil.clone().expect("scene depth")
            }),
            ..descriptor
        });
        let camera = device.create_buffer(&BufferDescriptor {
            label: Some("Camera"),
            size: CAMERA_BYTES as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let binding = device.create_bind_group(&BindGroupDescriptor {
            label: Some("Camera"),
            layout: &camera_layout,
            entries: &[BindGroupEntry { binding: 0, resource: BindingResource::Buffer(camera.as_entire_buffer_binding()) }],
        });
        let atmosphere = uniform_buffer(device, "Surface atmosphere", ATMOSPHERE_BYTES);
        let atmosphere_binding = uniform_binding(device, "Surface atmosphere", &atmosphere_layout, &atmosphere);
        let water_uniform = uniform_buffer(device, "Surface water", WATER_BYTES);
        let water_binding = uniform_binding(device, "Surface water", &water_layout, &water_uniform);

        Self {
            pipeline,
            flipped_winding,
            double_sided,
            blend,
            blend_flipped,
            blend_double_sided,
            water,
            camera,
            binding,
            atmosphere,
            atmosphere_binding,
            water_uniform,
            water_binding,
        }
    }
    pub(crate) fn update_camera_uniform(&self, queue: &Queue, camera: Camera, size: [u32; 2]) -> Mat4 {
        let projection = projection(camera, size);
        let values = projection
            .to_cols_array()
            .into_iter()
            .chain(camera_view(camera).to_cols_array())
            .chain(KEY_LIGHT.normalize().extend(0.0).to_array());
        let mut bytes = [0; CAMERA_BYTES];
        for (dest, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
            *dest = value.to_le_bytes();
        }
        queue.write_buffer(&self.camera, 0, &bytes);
        projection
    }
    pub(crate) fn bind_pipeline(&self, pass: &mut RenderPass<'_>, pipeline: Pipeline) {
        pass.set_pipeline(match pipeline {
            Pipeline::Surface => &self.pipeline,
            Pipeline::FlippedWinding => &self.flipped_winding,
            Pipeline::DoubleSided => &self.double_sided,
            Pipeline::Blend => &self.blend,
            Pipeline::BlendFlipped => &self.blend_flipped,
            Pipeline::BlendDoubleSided => &self.blend_double_sided,
            Pipeline::Water => &self.water,
        });
        pass.set_bind_group(0, &self.binding, &[]);
        pass.set_bind_group(2, &self.atmosphere_binding, &[]);
        pass.set_bind_group(3, &self.water_binding, &[]);
    }

    pub(crate) fn bind_water_texture(&self, pass: &mut RenderPass<'_>, textures: &Textures, pair: TexturePair) {
        pass.set_bind_group(4, textures.binding(pair.current), &[]);
        pass.set_bind_group(5, textures.binding(pair.next), &[]);
    }

    pub(crate) fn update_environment(
        &self,
        queue: &Queue,
        draw_distance: f32,
        altitude: f32,
        camera_position: DVec3,
        environment: Option<&Evaluated>,
        normal_map: TexturePair,
        scroll: [f32; 4],
    ) {
        let mut atmosphere = [0.0; ATMOSPHERE_FLOATS + 4];
        atmosphere[..ATMOSPHERE_FLOATS].copy_from_slice(&atmosphere_values(environment, altitude));
        if let Some(distance) = atmosphere.get_mut(ATMOSPHERE_FLOATS) {
            *distance = draw_distance;
        }
        let mut atmosphere_bytes = [0; ATMOSPHERE_BYTES];
        write_floats(queue, &self.atmosphere, &mut atmosphere_bytes, &atmosphere);

        let Some(environment) = environment else {
            let origin = water_origin(camera_position, DEFAULT_SCALE_ABOVE);
            let mut values = [0.0; 32];
            values[..4].copy_from_slice(&COLOR.to_array());
            values[4..8].copy_from_slice(&DEFAULT_FRESNEL);
            values[8..12].copy_from_slice(&[DEFAULT_NORMAL_SCALE, DEFAULT_NORMAL_SCALE, DEFAULT_NORMAL_SCALE, DEFAULT_SCALE_ABOVE]);
            values[12..16].copy_from_slice(&DEFAULT_WAVES);
            values[16..20].copy_from_slice(&scroll);
            values[20..24].copy_from_slice(&[0.0, DEFAULT_BLUR, 0.0, 0.0]);
            values[24..28].copy_from_slice(&origin[..4]);
            values[28..32].copy_from_slice(&origin[4..]);
            let mut water_bytes = [0; WATER_BYTES];
            write_floats(queue, &self.water_uniform, &mut water_bytes, &values);
            return;
        };
        let water = &environment.water;
        let origin = water_origin(camera_position, water.scale_above);
        let mut values = [0.0; 32];
        values[..24].copy_from_slice(&[
            water.fog_color[0],
            water.fog_color[1],
            water.fog_color[2],
            1.0,
            water.fresnel_offset,
            water.fresnel_scale,
            water.fog_density,
            if normal_map.current.is_some() || normal_map.next.is_some() { 1.0 } else { 0.0 },
            water.normal_scale[0],
            water.normal_scale[1],
            water.normal_scale[2],
            water.scale_above,
            water.waves[0][0],
            water.waves[0][1],
            water.waves[1][0],
            water.waves[1][1],
            scroll[0],
            scroll[1],
            scroll[2],
            scroll[3],
            0.0,
            water.blur,
            normal_map.factor,
            0.0,
        ]);
        values[24..28].copy_from_slice(&origin[..4]);
        values[28..32].copy_from_slice(&origin[4..]);
        let mut water_bytes = [0; WATER_BYTES];
        write_floats(queue, &self.water_uniform, &mut water_bytes, &values);
    }
}

fn uniform_layout(device: &Device, label: &str) -> BindGroupLayout {
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        }],
    })
}

fn uniform_buffer(device: &Device, label: &str, bytes: usize) -> Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: bytes as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn uniform_binding(device: &Device, label: &str, layout: &BindGroupLayout, buffer: &Buffer) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[BindGroupEntry { binding: 0, resource: BindingResource::Buffer(buffer.as_entire_buffer_binding()) }],
    })
}

fn write_floats(queue: &Queue, buffer: &Buffer, bytes: &mut [u8], values: &[f32]) {
    for (target, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        target.copy_from_slice(&value.to_le_bytes());
    }
    queue.write_buffer(buffer, 0, bytes);
}

pub(crate) fn projection(camera: Camera, size: [u32; 2]) -> Mat4 {
    perspective(camera.vertical_fov() as f32, size[0] as f32 / size[1] as f32, camera.draw_distance() as f32, 0.05) * camera_view(camera)
}

fn camera_view(camera: Camera) -> Mat4 {
    let direction = camera.direction();
    let up = if direction.dot(Vec3::Z).abs() > 0.999 { Vec3::Y } else { Vec3::Z };
    look_to_mat4(Vec3::ZERO, direction, up)
}

#[cfg(test)]
mod tests {
    use super::*;
    use math::DVec3;

    #[test]
    fn clip_conventions() {
        let camera = Camera::new(DVec3::splat(1e9), Vec3::X, std::f64::consts::FRAC_PI_2, [1000, 1000]).unwrap();
        let matrix = projection(camera, [1000, 1000]);
        let center = matrix.project_point3(Vec3::new(10.0, 0.0, 0.0));
        assert!(center.x.abs() < 1e-6 && center.y.abs() < 1e-6 && (0.0..1.0).contains(&center.z));
        assert!(matrix.project_point3(Vec3::new(10.0, 0.0, 1.0)).y > 0.0);
        assert!(matrix.project_point3(Vec3::new(10.0, -1.0, 0.0)).x > 0.0);
        assert!((matrix.project_point3(Vec3::new(0.05, 0.0, 0.0)).z - 1.0).abs() < 1e-5);
        let close = projection(camera.with_quality(32.0, 1.0).unwrap(), [1000, 1000]);
        assert!(close.project_point3(Vec3::new(32.0, 0.0, 0.0)).z.abs() < 1e-6);
        assert!(close.project_point3(Vec3::new(40.0, 0.0, 0.0)).z < 0.0);
    }

    #[test]
    fn far_depth_separation() {
        let camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [1000, 1000]).unwrap();
        let matrix = projection(camera, [1000, 1000]);
        let front = matrix.project_point3(Vec3::new(200.0, 0.0, 0.0)).z;
        let back = matrix.project_point3(Vec3::new(200.001, 0.0, 0.0)).z;
        assert_ne!(front, back, "depth must distinguish surfaces one millimeter apart at 200 meters");
        assert!(front > back);
    }
}
