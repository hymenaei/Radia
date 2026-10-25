use assets::Sampler;
use math::Mat4;
use wgpu::util::{BufferInitDescriptor, DeviceExt};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource,
    BindingType, BlendComponent, BlendFactor, BlendOperation, BlendState, Buffer, BufferBindingType, BufferDescriptor, BufferUsages,
    ColorTargetState, ColorWrites, CompareFunction, DepthStencilState, Device, FragmentState, MultisampleState, PipelineLayoutDescriptor,
    PrimitiveState, Queue, RenderPass, RenderPipeline, RenderPipelineDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages,
    TextureFormat, VertexState,
};

use super::atmosphere::{ATMOSPHERE_FLOATS, atmosphere_values};
use crate::{
    environment::{Evaluated, STAR_COUNT, TextureBlend, star_brightness, stars},
    scene::SceneView,
    texture::{TextureKey, Textures},
};

const UNIFORM_FLOATS: usize = 16 + ATMOSPHERE_FLOATS + 12 * 4;
const UNIFORM_BYTES: usize = UNIFORM_FLOATS * 4;
const STAR_UNIFORM_FLOATS: usize = 24;
const STAR_UNIFORM_BYTES: usize = STAR_UNIFORM_FLOATS * 4;

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
    stars: RenderPipeline,
    uniform: Buffer,
    binding: BindGroup,
    star_uniform: Buffer,
    star_binding: BindGroup,
    star_buffer_binding: BindGroup,
    active: bool,
    sun_textures: TexturePair,
    moon_textures: TexturePair,
    cloud_textures: TexturePair,
    bloom_textures: TexturePair,
    rainbow_textures: TexturePair,
    halo_textures: TexturePair,
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
            bind_group_layouts: &[
                Some(&layout),
                Some(textures),
                Some(textures),
                Some(textures),
                Some(textures),
                Some(textures),
                Some(textures),
            ],
            immediate_size: 0,
        });
        let cloud_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Cloud layers"),
            bind_group_layouts: &[Some(&layout), Some(textures), Some(textures)],
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
            layout: Some(&cloud_pipeline_layout),
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
            ..translucent.clone()
        });
        let star_shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("WindLight stars"),
            source: ShaderSource::Wgsl(include_str!("../shaders/stars.wgsl").into()),
        });
        let star_layout = uniform_layout(device, "Star state", ShaderStages::VERTEX_FRAGMENT);
        let star_data_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Fixed star field"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let star_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("WindLight stars"),
            bind_group_layouts: &[Some(&star_layout), Some(&star_data_layout), Some(textures), Some(textures)],
            immediate_size: 0,
        });
        let star_targets = [Some(ColorTargetState {
            format,
            blend: Some(BlendState {
                color: BlendComponent { src_factor: BlendFactor::SrcAlpha, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
                alpha: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
            }),
            write_mask: ColorWrites::ALL,
        })];
        let stars_pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Star field"),
            layout: Some(&star_pipeline_layout),
            vertex: VertexState {
                module: &star_shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: PrimitiveState::default(),
            depth_stencil: Some(DepthStencilState {
                format: TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: MultisampleState::default(),
            fragment: Some(FragmentState {
                module: &star_shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &star_targets,
            }),
            multiview_mask: None,
            cache: None,
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
        let star_uniform = device.create_buffer(&BufferDescriptor {
            label: Some("Star state"),
            size: STAR_UNIFORM_BYTES as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let star_binding = uniform_binding(device, "Star state", &star_layout, &star_uniform);
        let star_values = stars();
        let mut star_bytes = Vec::with_capacity(STAR_COUNT * 8 * 4);
        for star in star_values {
            for value in star.direction_intensity.into_iter().chain(star.color) {
                star_bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let star_buffer = device.create_buffer_init(&BufferInitDescriptor {
            label: Some("Fixed star field"),
            contents: &star_bytes,
            usage: BufferUsages::STORAGE,
        });
        let star_buffer_binding = device.create_bind_group(&BindGroupDescriptor {
            label: Some("Fixed star field"),
            layout: &star_data_layout,
            entries: &[BindGroupEntry { binding: 0, resource: star_buffer.as_entire_binding() }],
        });
        Self {
            sky,
            cloud,
            sun,
            moon,
            stars: stars_pipeline,
            uniform,
            binding,
            star_uniform,
            star_binding,
            star_buffer_binding,
            active: false,
            sun_textures: TexturePair::default(),
            moon_textures: TexturePair::default(),
            cloud_textures: TexturePair::default(),
            bloom_textures: TexturePair::default(),
            rainbow_textures: TexturePair::default(),
            halo_textures: TexturePair::default(),
        }
    }

    pub(crate) fn update(
        &mut self,
        queue: &Queue,
        projection: Mat4,
        inverse: Mat4,
        altitude: f32,
        viewport: [u32; 2],
        environment: Option<&Evaluated>,
        cloud_scroll: [f32; 2],
        animation_time: f32,
        sun_textures: TexturePair,
        moon_textures: TexturePair,
        cloud_textures: TexturePair,
        bloom_textures: TexturePair,
        rainbow_textures: TexturePair,
        halo_textures: TexturePair,
    ) {
        self.active = false;
        self.sun_textures = sun_textures;
        self.moon_textures = moon_textures;
        self.cloud_textures = cloud_textures;
        self.bloom_textures = bloom_textures;
        self.rainbow_textures = rainbow_textures;
        self.halo_textures = halo_textures;
        let Some(environment) = environment else { return };
        let mut values = [0.0; UNIFORM_FLOATS];
        let mut offset = 0;
        append(&mut values, &mut offset, inverse.to_cols_array());
        append(&mut values, &mut offset, atmosphere_values(Some(environment), altitude));
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
        append(&mut values, &mut offset, [sun_textures.factor, moon_textures.factor, cloud_textures.factor, rainbow_textures.factor]);
        append(
            &mut values,
            &mut offset,
            [resident(sun_textures.current), resident(sun_textures.next), resident(moon_textures.current), resident(moon_textures.next)],
        );
        append(
            &mut values,
            &mut offset,
            [environment.sky.moon_brightness, environment.sky.star_brightness, rainbow_textures.factor, halo_textures.factor],
        );
        append(
            &mut values,
            &mut offset,
            [
                resident(rainbow_textures.current),
                resident(rainbow_textures.next),
                resident(halo_textures.current),
                resident(halo_textures.next),
            ],
        );
        append(&mut values, &mut offset, [environment.sky.moisture, environment.sky.droplet_radius, environment.sky.ice, 0.0]);
        let mut bytes = [0; UNIFORM_BYTES];
        for (target, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
            *target = value.to_le_bytes();
        }
        queue.write_buffer(&self.uniform, 0, &bytes);
        let mut star_values = [0.0; STAR_UNIFORM_FLOATS];
        star_values[..16].copy_from_slice(&projection.to_cols_array());
        star_values[16] = viewport[0].max(1) as f32;
        star_values[17] = viewport[1].max(1) as f32;
        star_values[18] = star_brightness(environment.sky.star_brightness, environment.sky.sun_direction[2]);
        star_values[19] = bloom_textures.factor;
        star_values[20] = (animation_time * 0.5).rem_euclid(1.25);
        let mut star_bytes = [0; STAR_UNIFORM_BYTES];
        for (target, value) in star_bytes.as_chunks_mut::<4>().0.iter_mut().zip(star_values) {
            *target = value.to_le_bytes();
        }
        queue.write_buffer(&self.star_uniform, 0, &star_bytes);
        self.active = true;
    }

    pub(crate) fn draw(&self, pass: &mut RenderPass<'_>, textures: &Textures) {
        if !self.active {
            return;
        }
        pass.set_bind_group(0, &self.binding, &[]);
        pass.set_pipeline(&self.sky);
        pass.set_bind_group(1, textures.binding(None), &[]);
        bind_pair(pass, textures, 3, self.rainbow_textures);
        bind_pair(pass, textures, 5, self.halo_textures);
        pass.draw(0..3, 0..1);
        draw_pair(pass, &self.sun, textures, self.sun_textures);
        if self.moon_textures.current.is_some() || self.moon_textures.next.is_some() {
            draw_pair(pass, &self.moon, textures, self.moon_textures);
        }
        if self.bloom_textures.current.is_some() || self.bloom_textures.next.is_some() {
            pass.set_pipeline(&self.stars);
            pass.set_bind_group(0, &self.star_binding, &[]);
            pass.set_bind_group(1, &self.star_buffer_binding, &[]);
            bind_pair(pass, textures, 2, self.bloom_textures);
            pass.draw(0..6, 0..STAR_COUNT as u32);
        }
        draw_cloud(pass, &self.cloud, textures, self.cloud_textures);
    }
}

fn draw_pair(pass: &mut RenderPass<'_>, pipeline: &RenderPipeline, textures: &Textures, pair: TexturePair) {
    pass.set_pipeline(pipeline);
    bind_pair(pass, textures, 1, pair);
    pass.draw(0..3, 0..1);
}

fn bind_pair(pass: &mut RenderPass<'_>, textures: &Textures, group: u32, pair: TexturePair) {
    let current = pair.current.or(pair.next);
    let next = pair.next.or(pair.current);
    pass.set_bind_group(group, textures.binding(current), &[]);
    pass.set_bind_group(group + 1, textures.binding(next), &[]);
}

fn draw_cloud(pass: &mut RenderPass<'_>, pipeline: &RenderPipeline, textures: &Textures, pair: TexturePair) {
    if pair.current.is_none() && pair.next.is_none() {
        return;
    }
    let current = pair.current.or(pair.next);
    let next = pair.next.or(pair.current);
    pass.set_pipeline(pipeline);
    pass.set_bind_group(1, textures.binding(current), &[]);
    pass.set_bind_group(2, textures.binding(next), &[]);
    pass.draw(0..3, 0..1);
}

fn resident(key: Option<TextureKey>) -> f32 {
    if key.is_some() { 1.0 } else { 0.0 }
}

fn uniform_layout(device: &Device, label: &str, visibility: ShaderStages) -> BindGroupLayout {
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[BindGroupLayoutEntry {
            binding: 0,
            visibility,
            ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        }],
    })
}

fn uniform_binding(device: &Device, label: &str, layout: &BindGroupLayout, uniform: &Buffer) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[BindGroupEntry { binding: 0, resource: BindingResource::Buffer(uniform.as_entire_buffer_binding()) }],
    })
}

pub(crate) fn blend_keys(device: &Device, blend: TextureBlend, view: &SceneView, textures: &mut Textures) -> TexturePair {
    let mut current = resident_key(device, blend.current, view, textures);
    let mut next = resident_key(device, blend.next, view, textures);
    if current.is_none() {
        current = next.take();
    }
    if next.is_none() {
        next = current;
    }
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
