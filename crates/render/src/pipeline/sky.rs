use assets::Sampler;
use math::Mat4;
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource,
    BindingType, BlendState, Buffer, BufferBindingType, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, CompareFunction,
    DepthStencilState, Device, FragmentState, MultisampleState, PipelineLayoutDescriptor, PrimitiveState, Queue, RenderPass,
    RenderPipeline, RenderPipelineDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages, TextureFormat, VertexState,
};

use super::atmosphere::{ATMOSPHERE_FLOATS, atmosphere_values};
use crate::{
    environment::{Evaluated, TextureBlend},
    scene::SceneView,
    texture::{TextureKey, Textures},
};

const UNIFORM_FLOATS: usize = 16 + ATMOSPHERE_FLOATS + 10 * 4;
const UNIFORM_BYTES: usize = UNIFORM_FLOATS * 4;

#[derive(Clone, Copy, Default)]
pub(crate) struct TexturePair {
    pub(crate) current: Option<TextureKey>,
    pub(crate) next: Option<TextureKey>,
    pub(crate) factor: f32,
}

pub(crate) struct Sky {
    sky: RenderPipeline,
    cloud: RenderPipeline,
    sun: RenderPipeline,
    moon: RenderPipeline,
    uniform: Buffer,
    binding: BindGroup,
    active: bool,
    sun_textures: TexturePair,
    moon_textures: TexturePair,
    cloud_textures: TexturePair,
}

impl Sky {
    pub(crate) fn new(device: &Device, format: TextureFormat, textures: &BindGroupLayout) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Sky and atmosphere"),
            source: ShaderSource::Wgsl(concat!(include_str!("../shaders/atmosphere.wgsl"), include_str!("../shaders/sky.wgsl")).into()),
        });
        let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Sky uniform"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX_FRAGMENT,
                ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Sky"),
            bind_group_layouts: &[Some(&layout), Some(textures)],
            immediate_size: 0,
        });
        let buffers = [];
        let targets = [Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })];
        let depth = Some(DepthStencilState {
            format: TextureFormat::Depth32Float,
            depth_write_enabled: Some(false),
            depth_compare: Some(CompareFunction::GreaterEqual),
            stencil: Default::default(),
            bias: Default::default(),
        });
        let descriptor = RenderPipelineDescriptor {
            label: Some("Sky background"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: PrimitiveState::default(),
            depth_stencil: depth,
            multisample: MultisampleState::default(),
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("sky_fragment"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        };
        let sky = device.create_render_pipeline(&descriptor);
        let translucent_targets =
            [Some(ColorTargetState { format, blend: Some(BlendState::ALPHA_BLENDING), write_mask: ColorWrites::ALL })];
        let translucent = RenderPipelineDescriptor {
            label: Some("Sky details"),
            fragment: Some(FragmentState { targets: &translucent_targets, ..descriptor.fragment.clone().expect("sky fragment") }),
            ..descriptor
        };
        let cloud = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Cloud layer"),
            fragment: Some(FragmentState { entry_point: Some("cloud_fragment"), ..translucent.fragment.clone().expect("sky fragment") }),
            ..translucent.clone()
        });
        let sun = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Sun disc"),
            fragment: Some(FragmentState { entry_point: Some("sun_fragment"), ..translucent.fragment.clone().expect("sky fragment") }),
            ..translucent.clone()
        });
        let moon = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Moon disc"),
            fragment: Some(FragmentState { entry_point: Some("moon_fragment"), ..translucent.fragment.clone().expect("sky fragment") }),
            ..translucent
        });
        let uniform = device.create_buffer(&BufferDescriptor {
            label: Some("Sky state"),
            size: UNIFORM_BYTES as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let binding = device.create_bind_group(&BindGroupDescriptor {
            label: Some("Sky state"),
            layout: &layout,
            entries: &[BindGroupEntry { binding: 0, resource: BindingResource::Buffer(uniform.as_entire_buffer_binding()) }],
        });
        Self {
            sky,
            cloud,
            sun,
            moon,
            uniform,
            binding,
            active: false,
            sun_textures: TexturePair::default(),
            moon_textures: TexturePair::default(),
            cloud_textures: TexturePair::default(),
        }
    }

    pub(crate) fn update(
        &mut self,
        queue: &Queue,
        inverse: Mat4,
        environment: Option<&Evaluated>,
        cloud_scroll: [f32; 2],
        sun_textures: TexturePair,
        moon_textures: TexturePair,
        cloud_textures: TexturePair,
    ) {
        self.active = false;
        self.sun_textures = sun_textures;
        self.moon_textures = moon_textures;
        self.cloud_textures = cloud_textures;
        let Some(environment) = environment else { return };
        let mut values = [0.0; UNIFORM_FLOATS];
        let mut offset = 0;
        append(&mut values, &mut offset, inverse.to_cols_array());
        append(&mut values, &mut offset, atmosphere_values(Some(environment)));
        append(&mut values, &mut offset, environment.sky.sun_direction);
        append(&mut values, &mut offset, [environment.sky.sun.scale]);
        append(&mut values, &mut offset, environment.sky.moon_direction);
        append(&mut values, &mut offset, [environment.sky.moon.scale]);
        append(
            &mut values,
            &mut offset,
            [environment.sky.clouds.color[0], environment.sky.clouds.color[1], environment.sky.clouds.color[2], 1.0],
        );
        append(
            &mut values,
            &mut offset,
            [environment.sky.clouds.density[0][0], environment.sky.clouds.density[0][1], environment.sky.clouds.density[0][2], 0.0],
        );
        append(
            &mut values,
            &mut offset,
            [environment.sky.clouds.density[1][0], environment.sky.clouds.density[1][1], environment.sky.clouds.density[1][2], 0.0],
        );
        append(&mut values, &mut offset, [environment.sky.clouds.scale, environment.sky.clouds.variance, 0.0, 0.0]);
        append(&mut values, &mut offset, [cloud_scroll[0], cloud_scroll[1], 0.0, 0.0]);
        append(&mut values, &mut offset, [sun_textures.factor, moon_textures.factor, cloud_textures.factor, environment.sky.gamma]);
        append(
            &mut values,
            &mut offset,
            [resident(sun_textures.current), resident(sun_textures.next), resident(moon_textures.current), resident(moon_textures.next)],
        );
        append(
            &mut values,
            &mut offset,
            [resident(cloud_textures.current), resident(cloud_textures.next), environment.sky.moon_brightness, environment.sky.gamma],
        );
        let mut bytes = [0; UNIFORM_BYTES];
        for (target, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
            *target = value.to_le_bytes();
        }
        queue.write_buffer(&self.uniform, 0, &bytes);
        self.active = true;
    }

    pub(crate) fn draw(&self, pass: &mut RenderPass<'_>, textures: &Textures) {
        if !self.active {
            return;
        }
        pass.set_bind_group(0, &self.binding, &[]);
        pass.set_pipeline(&self.sky);
        pass.set_bind_group(1, textures.binding(None), &[]);
        pass.draw(0..3, 0..1);
        draw_pair(pass, &self.cloud, textures, self.cloud_textures);
        draw_pair(pass, &self.sun, textures, self.sun_textures);
        draw_pair(pass, &self.moon, textures, self.moon_textures);
    }
}

fn draw_pair(pass: &mut RenderPass<'_>, pipeline: &RenderPipeline, textures: &Textures, pair: TexturePair) {
    pass.set_pipeline(pipeline);
    pass.set_bind_group(1, textures.binding(pair.current), &[]);
    pass.draw(0..3, 0..1);
    if let Some(next) = pair.next.filter(|next| Some(*next) != pair.current) {
        pass.set_bind_group(1, textures.binding(Some(next)), &[]);
        pass.draw(0..3, 1..2);
    }
}

fn resident(key: Option<TextureKey>) -> f32 {
    if key.is_some() { 1.0 } else { 0.0 }
}

pub(crate) fn blend_keys(device: &Device, blend: TextureBlend, view: &SceneView, textures: &mut Textures) -> TexturePair {
    let current = resident_key(device, blend.current, view, textures);
    let next = resident_key(device, blend.next, view, textures);
    TexturePair { current, next, factor: if current == next { 0.0 } else { blend.factor } }
}

fn resident_key(device: &Device, id: Option<utils::AssetId>, view: &SceneView, textures: &mut Textures) -> Option<TextureKey> {
    let id = id?;
    let handle = view.environment_texture(id)?;
    let key = TextureKey { id: handle.id(), sampler: Sampler::default() };
    textures.resident(device, key)
}

fn append<const N: usize>(target: &mut [f32; UNIFORM_FLOATS], offset: &mut usize, values: [f32; N]) {
    if let Some(slice) = target.get_mut(*offset..offset.saturating_add(N)) {
        slice.copy_from_slice(&values);
        *offset += N;
    }
}
