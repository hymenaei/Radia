@group(2) @binding(0) var detail_0: texture_2d<f32>;
@group(2) @binding(1) var detail_1: texture_2d<f32>;
@group(2) @binding(2) var detail_2: texture_2d<f32>;
@group(2) @binding(3) var detail_3: texture_2d<f32>;
@group(2) @binding(4) var detail_sampler: sampler;

struct TerrainOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) position_view: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) @interpolate(flat) params: vec4<f32>,
    @location(4) weights: vec4<f32>,
}

@vertex fn terrain_vertex(v: Input) -> TerrainOutput {
    let surface = transform(v);
    var out: TerrainOutput;
    out.position = surface.position;
    out.normal = surface.normal;
    out.position_view = surface.position_view;
    out.uv = v.uv;
    out.params = surface.params;
    out.weights = v.weights;
    return out;
}

@fragment fn terrain_fragment(v: TerrainOutput, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let albedo = textureSample(detail_0, detail_sampler, v.uv) * v.weights.x
        + textureSample(detail_1, detail_sampler, v.uv) * v.weights.y
        + textureSample(detail_2, detail_sampler, v.uv) * v.weights.z
        + textureSample(detail_3, detail_sampler, v.uv) * v.weights.w;
    return vec4<f32>(shade_surface(v.position_view, v.normal, albedo.rgb, v.params.x, front), 1.0);
}
