use math::camera::rh::{proj::directx::perspective, view::look_to_mat4};
use math::{Mat4, Vec3};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource,
    BindingType, BlendState, Buffer, BufferBindingType, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, CompareFunction,
    DepthStencilState, Device, Face, FragmentState, FrontFace, MultisampleState, PipelineLayoutDescriptor, PrimitiveState, Queue,
    RenderPass, RenderPipeline, RenderPipelineDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages, TextureFormat,
    VertexAttribute, VertexBufferLayout, VertexFormat, VertexState, VertexStepMode,
};

use crate::scene::Camera;

pub(crate) const INSTANCE_BYTES: usize = 192;
pub(crate) const VERTEX_BYTES: usize = 48;
const CAMERA_BYTES: usize = 144;
const KEY_LIGHT: Vec3 = Vec3::new(-0.3, 0.5, 1.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Pipeline {
    Terrain,
    Surface,
    FlippedWinding,
    DoubleSided,
    Blend,
    BlendFlipped,
    BlendDoubleSided,
    Water,
}

pub(crate) struct SurfacePipelines {
    terrain: RenderPipeline,
    pipeline: RenderPipeline,
    flipped_winding: RenderPipeline,
    double_sided: RenderPipeline,
    blend: RenderPipeline,
    blend_flipped: RenderPipeline,
    blend_double_sided: RenderPipeline,
    water: RenderPipeline,
    camera: Buffer,
    binding: BindGroup,
}

impl SurfacePipelines {
    pub(crate) fn new(device: &Device, format: TextureFormat, textures: &BindGroupLayout, terrain_textures: &BindGroupLayout) -> Self {
        let surface_shader_source = include_str!("../shaders/surface.wgsl");
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Scene preview"),
            source: ShaderSource::Wgsl(surface_shader_source.into()),
        });
        let attributes = [
            VertexAttribute { format: VertexFormat::Float32x4, offset: 0, shader_location: 4 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 16, shader_location: 5 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 32, shader_location: 6 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 48, shader_location: 7 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 64, shader_location: 8 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 80, shader_location: 9 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 96, shader_location: 10 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 112, shader_location: 11 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 128, shader_location: 12 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 144, shader_location: 13 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 160, shader_location: 14 },
            VertexAttribute { format: VertexFormat::Float32x4, offset: 176, shader_location: 15 },
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
        let plain_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Surface pipelines"),
            bind_group_layouts: &[Some(&camera_layout), Some(textures)],
            immediate_size: 0,
        });
        let buffers = [
            Some(VertexBufferLayout {
                array_stride: VERTEX_BYTES as u64,
                step_mode: VertexStepMode::Vertex,
                attributes: &[
                    VertexAttribute { format: VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                    VertexAttribute { format: VertexFormat::Float32x3, offset: 12, shader_location: 1 },
                    VertexAttribute { format: VertexFormat::Float32x2, offset: 24, shader_location: 2 },
                    VertexAttribute { format: VertexFormat::Float32x4, offset: 32, shader_location: 3 },
                ],
            }),
            Some(VertexBufferLayout { array_stride: INSTANCE_BYTES as u64, step_mode: VertexStepMode::Instance, attributes: &attributes }),
        ];
        let targets = [Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })];
        let descriptor = RenderPipelineDescriptor {
            label: Some("Opaque scene preview"),
            layout: Some(&plain_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &buffers,
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
        let terrain_shader_source = format!("{surface_shader_source}\n{}", include_str!("../shaders/terrain.wgsl"));
        let terrain_shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Legacy terrain detail blend"),
            source: ShaderSource::Wgsl(terrain_shader_source.into()),
        });
        let terrain_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Legacy terrain pipeline"),
            bind_group_layouts: &[Some(&camera_layout), Some(textures), Some(terrain_textures)],
            immediate_size: 0,
        });
        let terrain = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Legacy terrain"),
            layout: Some(&terrain_layout),
            vertex: VertexState {
                module: &terrain_shader,
                entry_point: Some("terrain_vertex"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: descriptor.primitive,
            depth_stencil: descriptor.depth_stencil.clone(),
            multisample: descriptor.multisample,
            fragment: Some(FragmentState {
                module: &terrain_shader,
                entry_point: Some("terrain_fragment"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });
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
            vertex: VertexState { entry_point: Some("water_vertex"), ..descriptor.vertex.clone() },
            fragment: Some(FragmentState { entry_point: Some("water_fragment"), ..descriptor.fragment.clone().expect("scene fragment") }),
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

        Self { terrain, pipeline, flipped_winding, double_sided, blend, blend_flipped, blend_double_sided, water, camera, binding }
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
            Pipeline::Terrain => &self.terrain,
            Pipeline::Surface => &self.pipeline,
            Pipeline::FlippedWinding => &self.flipped_winding,
            Pipeline::DoubleSided => &self.double_sided,
            Pipeline::Blend => &self.blend,
            Pipeline::BlendFlipped => &self.blend_flipped,
            Pipeline::BlendDoubleSided => &self.blend_double_sided,
            Pipeline::Water => &self.water,
        });
        pass.set_bind_group(0, &self.binding, &[]);
    }
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
