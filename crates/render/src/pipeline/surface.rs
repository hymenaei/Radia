use math::camera::rh::view::look_to_mat4;
use math::{DVec3, Mat4, Vec3, Vec4};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource,
    BindingType, BlendState, Buffer, BufferBindingType, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, CompareFunction,
    DepthStencilState, Device, Face, FragmentState, FrontFace, MultisampleState, PipelineLayoutDescriptor, PrimitiveState, Queue,
    RenderPass, RenderPipeline, RenderPipelineDescriptor, SamplerBindingType, ShaderModuleDescriptor, ShaderSource, ShaderStages,
    TextureFormat, TextureSampleType, TextureViewDimension, VertexAttribute, VertexBufferLayout, VertexFormat, VertexState, VertexStepMode,
};

use crate::scene::Camera;

use crate::{
    environment::Evaluated,
    texture::{TextureKey, TextureViewState, Textures},
};

use super::TexturePair;

use super::{
    atmosphere::{ATMOSPHERE_FLOATS, atmosphere_values},
    water::water_surface_values,
};

pub(crate) const INSTANCE_BYTES: usize = 192;
pub(crate) const VERTEX_BYTES: usize = 32;
const CAMERA_BYTES: usize = 128;
const ATMOSPHERE_BYTES: usize = ATMOSPHERE_FLOATS * 4;
const WATER_BYTES: usize = 32 * 4;
pub(crate) const CAMERA_NEAR_PLANE: f32 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Pipeline {
    Surface,
    FlippedWinding,
    DoubleSided,
    Blend,
    BlendFlipped,
    BlendDoubleSided,
    RegionWater,
}

pub(crate) struct SurfacePipelines {
    pipeline: RenderPipeline,
    flipped_winding: RenderPipeline,
    double_sided: RenderPipeline,
    blend: RenderPipeline,
    blend_flipped: RenderPipeline,
    blend_double_sided: RenderPipeline,
    region_water: RenderPipeline,
    frame_environment_layout: BindGroupLayout,
    camera: Buffer,
    binding: BindGroup,
    atmosphere: Buffer,
    region_water_uniform: Buffer,
    region_water_normal_keys: [Option<TextureKey>; 2],
    region_water_normal_states: [Option<TextureViewState>; 2],
}

impl SurfacePipelines {
    pub(crate) fn new(device: &Device, format: TextureFormat, textures: &BindGroupLayout, texture_store: &Textures) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Scene preview"),
            source: ShaderSource::Wgsl(
                concat!(
                    include_str!("../shaders/atmosphere.wgsl"),
                    include_str!("../shaders/water.wgsl"),
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
        let frame_environment_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Frame environment"),
            entries: &[
                uniform_entry(0, ShaderStages::VERTEX_FRAGMENT),
                uniform_entry(1, ShaderStages::FRAGMENT),
                uniform_entry(2, ShaderStages::FRAGMENT),
                texture_entry(3),
                texture_entry(4),
                BindGroupLayoutEntry {
                    binding: 5,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let plain_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Surface pipelines"),
            bind_group_layouts: &[Some(&frame_environment_layout), Some(textures)],
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
        let region_water = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Region water"),
            layout: Some(&plain_layout),
            vertex: VertexState { entry_point: Some("vertex"), ..descriptor.vertex.clone() },
            fragment: Some(FragmentState {
                entry_point: Some("region_water_fragment"),
                ..descriptor.fragment.clone().expect("scene fragment")
            }),
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
        let atmosphere = uniform_buffer(device, "Surface atmosphere", ATMOSPHERE_BYTES);
        let region_water_uniform = uniform_buffer(device, "Region water", WATER_BYTES);
        let binding = device.create_bind_group(&BindGroupDescriptor {
            label: Some("Frame environment"),
            layout: &frame_environment_layout,
            entries: &[
                BindGroupEntry { binding: 0, resource: BindingResource::Buffer(camera.as_entire_buffer_binding()) },
                BindGroupEntry { binding: 1, resource: BindingResource::Buffer(atmosphere.as_entire_buffer_binding()) },
                BindGroupEntry { binding: 2, resource: BindingResource::Buffer(region_water_uniform.as_entire_buffer_binding()) },
                BindGroupEntry { binding: 3, resource: BindingResource::TextureView(texture_store.view(None)) },
                BindGroupEntry { binding: 4, resource: BindingResource::TextureView(texture_store.view(None)) },
                BindGroupEntry { binding: 5, resource: BindingResource::Sampler(&texture_store.default_sampler) },
            ],
        });
        Self {
            pipeline,
            flipped_winding,
            double_sided,
            blend,
            blend_flipped,
            blend_double_sided,
            region_water,
            frame_environment_layout,
            camera,
            binding,
            atmosphere,
            region_water_uniform,
            region_water_normal_keys: [None, None],
            region_water_normal_states: [None, None],
        }
    }
    pub(crate) fn update_camera_uniform(&self, queue: &Queue, camera: Camera, size: [u32; 2]) -> Mat4 {
        let projection = projection(camera, size);
        let values = projection.to_cols_array().into_iter().chain(camera_view(camera).to_cols_array());
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
            Pipeline::RegionWater => &self.region_water,
        });
        pass.set_bind_group(0, &self.binding, &[]);
    }

    pub(crate) fn update_environment(
        &mut self,
        device: &Device,
        textures: &Textures,
        queue: &Queue,
        camera_position: DVec3,
        environment: Option<&Evaluated>,
        normal_map: TexturePair,
        water_height: Option<f64>,
        scroll: [f32; 4],
    ) {
        let atmosphere = atmosphere_values(environment);
        let mut atmosphere_bytes = [0; ATMOSPHERE_BYTES];
        write_floats(queue, &self.atmosphere, &mut atmosphere_bytes, &atmosphere);
        let region_water_values = water_surface_values(
            environment,
            camera_position,
            water_height,
            scroll,
            normal_map.factor,
            normal_map.current.is_some() || normal_map.next.is_some(),
        );
        let mut region_water_bytes = [0; WATER_BYTES];
        write_floats(queue, &self.region_water_uniform, &mut region_water_bytes, &region_water_values);
        let keys = [normal_map.current, normal_map.next];
        let states = keys.map(|key| textures.view_state(key));
        if keys != self.region_water_normal_keys || states != self.region_water_normal_states {
            self.binding = device.create_bind_group(&BindGroupDescriptor {
                label: Some("Frame environment"),
                layout: &self.frame_environment_layout,
                entries: &[
                    BindGroupEntry { binding: 0, resource: BindingResource::Buffer(self.camera.as_entire_buffer_binding()) },
                    BindGroupEntry { binding: 1, resource: BindingResource::Buffer(self.atmosphere.as_entire_buffer_binding()) },
                    BindGroupEntry { binding: 2, resource: BindingResource::Buffer(self.region_water_uniform.as_entire_buffer_binding()) },
                    BindGroupEntry { binding: 3, resource: BindingResource::TextureView(textures.water_view(normal_map.current)) },
                    BindGroupEntry { binding: 4, resource: BindingResource::TextureView(textures.water_view(normal_map.next)) },
                    BindGroupEntry { binding: 5, resource: BindingResource::Sampler(&textures.water_sampler) },
                ],
            });
            self.region_water_normal_keys = keys;
            self.region_water_normal_states = states;
        }
    }
}

fn uniform_entry(binding: u32, visibility: ShaderStages) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility,
        ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

fn texture_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Texture {
            sample_type: TextureSampleType::Float { filterable: true },
            view_dimension: TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn uniform_buffer(device: &Device, label: &str, bytes: usize) -> Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: bytes as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn write_floats(queue: &Queue, buffer: &Buffer, bytes: &mut [u8], values: &[f32]) {
    for (target, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        target.copy_from_slice(&value.to_le_bytes());
    }
    queue.write_buffer(buffer, 0, bytes);
}

pub(crate) fn projection(camera: Camera, size: [u32; 2]) -> Mat4 {
    let focal = 1.0 / (camera.vertical_fov() as f32 * 0.5).tan();
    let aspect = size[0] as f32 / size[1] as f32;
    Mat4::from_cols(
        Vec4::new(focal / aspect, 0.0, 0.0, 0.0),
        Vec4::new(0.0, focal, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, -1.0),
        Vec4::new(0.0, 0.0, CAMERA_NEAR_PLANE, 0.0),
    ) * camera_view(camera)
}

pub(crate) fn camera_view(camera: Camera) -> Mat4 {
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
        let distant = projection(camera.with_quality(1024.0, 1.0).unwrap(), [1000, 1000]);
        assert_eq!(close, distant);
        assert!(close.project_point3(Vec3::new(32.0, 0.0, 0.0)).z > 0.0);
        assert!(close.project_point3(Vec3::new(1_000_000.0, 0.0, 0.0)).z > 0.0);
        assert!(math::Frustum::from_clip_matrix(close).intersects_sphere(DVec3::new(1.0e9, 0.0, 0.0), 0.0));
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
