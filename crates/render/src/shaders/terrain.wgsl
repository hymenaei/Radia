@group(2) @binding(0) var detail_0: texture_2d<f32>;
@group(2) @binding(1) var detail_1: texture_2d<f32>;
@group(2) @binding(2) var detail_2: texture_2d<f32>;
@group(2) @binding(3) var detail_3: texture_2d<f32>;
@group(2) @binding(4) var detail_sampler: sampler;
@group(2) @binding(5) var alpha_ramp: texture_2d<f32>;
@group(2) @binding(6) var alpha_sampler: sampler;

struct TerrainOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) position_view: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) @interpolate(flat) params: vec4<f32>,
    @location(4) composition: f32,
}

@vertex fn terrain_vertex(v: Input) -> TerrainOutput {
    let surface = transform(v);
    var out: TerrainOutput;
    out.position = surface.position;
    out.normal = surface.normal;
    out.position_view = surface.position_view;
    out.uv = v.uv;
    out.params = surface.params;
    out.composition = v.composition;
    return out;
}

@fragment fn terrain_fragment(v: TerrainOutput, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let c0 = textureSample(detail_0, detail_sampler, v.uv);
    let c1 = textureSample(detail_1, detail_sampler, v.uv);
    let c2 = textureSample(detail_2, detail_sampler, v.uv);
    let c3 = textureSample(detail_3, detail_sampler, v.uv);
    let alpha1 = textureSample(alpha_ramp, alpha_sampler, vec2<f32>(v.composition, 0.0)).r;
    let alpha2 = textureSample(alpha_ramp, alpha_sampler, vec2<f32>(v.composition - 2.0, 0.0)).r;
    let alpha_final = textureSample(alpha_ramp, alpha_sampler, vec2<f32>(v.composition - 1.0, 0.0)).r;
    let albedo = mix(mix(c3, c2, alpha2), mix(c1, c0, alpha1), alpha_final);
    return vec4<f32>(shade_surface(v.position_view, v.normal, albedo.rgb, v.params.x, front), 1.0);
}
