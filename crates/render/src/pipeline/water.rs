use math::{Mat4, Vec4};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, BindingType, Buffer,
    BufferBindingType, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, CompareFunction, DepthStencilState, Device, Face,
    FragmentState, MultisampleState, PipelineLayoutDescriptor, PrimitiveState, Queue, RenderPass, RenderPipeline, RenderPipelineDescriptor,
    ShaderModuleDescriptor, ShaderSource, ShaderStages, TextureFormat, VertexState,
};

use crate::scene::{Camera, MAX_REGIONS, SceneView};

pub(crate) const COLOR: Vec4 = Vec4::new(0.025, 0.09, 0.14, 1.0);
const UNIFORM_BYTES: usize = 160 + MAX_REGIONS * 16;

pub(crate) struct Water {
    pipeline: RenderPipeline,
    uniform: Buffer,
    binding: BindGroup,
    pub(crate) active: bool,
}

impl Water {
    pub(crate) fn new(device: &Device, format: TextureFormat) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("water"),
            source: ShaderSource::Wgsl(include_str!("../shaders/water.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("water"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("water"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("water"),
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
            label: Some("water"),
            size: UNIFORM_BYTES as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let binding = device.create_bind_group(&BindGroupDescriptor {
            label: Some("water"),
            layout: &layout,
            entries: &[BindGroupEntry { binding: 0, resource: BindingResource::Buffer(uniform.as_entire_buffer_binding()) }],
        });
        Self { pipeline, uniform, binding, active: false }
    }

    pub(crate) fn update(&mut self, queue: &Queue, view: &SceneView, camera: Camera, projection: Mat4, height: Option<f64>) {
        self.active = false;
        let Some(height) = height
            .map(|height| height - camera.position().z)
            .filter(|height| height.is_finite() && *height < 0.0 && height.abs() < f64::from(f32::MAX) / 16.0)
        else {
            return;
        };
        let mut regions = [Vec4::ZERO; MAX_REGIONS];
        let mut count = 0;
        for region in view.regions() {
            let Some(origin) = region.summary().info.resolved_origin() else { return };
            let relative = (origin - camera.position()).as_vec3();
            if !relative.is_finite() || count == MAX_REGIONS {
                return;
            }
            regions[count] = Vec4::new(relative.x, relative.y, 256.0, 256.0);
            count += 1;
        }
        if count == 0 {
            return;
        }
        let values = projection
            .to_cols_array()
            .into_iter()
            .chain(projection.inverse().to_cols_array())
            .chain([height as f32, count as f32, 0.0, 0.0])
            .chain(COLOR.to_array())
            .chain(regions.into_iter().flat_map(|region| region.to_array()));
        let mut bytes = [0; UNIFORM_BYTES];
        for (dest, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
            *dest = value.to_le_bytes();
        }
        queue.write_buffer(&self.uniform, 0, &bytes);
        self.active = true;
    }

    pub(crate) fn draw(&self, pass: &mut RenderPass<'_>) {
        if self.active {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.binding, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
