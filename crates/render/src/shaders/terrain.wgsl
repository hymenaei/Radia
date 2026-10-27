struct Camera {
    projection: mat4x4<f32>,
    view: mat4x4<f32>,
    light: vec4<f32>,
}
@group(0) @binding(0) var<uniform> camera: Camera;
@group(2) @binding(0) var detail_0: texture_2d<f32>;
@group(2) @binding(1) var detail_1: texture_2d<f32>;
@group(2) @binding(2) var detail_2: texture_2d<f32>;
@group(2) @binding(3) var detail_3: texture_2d<f32>;
@group(2) @binding(4) var detail_sampler: sampler;

struct Input {
    @location(0) position: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) weights: vec4<f32>,
    @location(4) x: vec4<f32>,
    @location(5) y: vec4<f32>,
    @location(6) z: vec4<f32>,
    @location(7) translation: vec4<f32>,
}

struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) weights: vec4<f32>,
}

@vertex fn vertex(v: Input) -> Output {
    var out: Output;
    let model = mat4x4<f32>(v.x, v.y, v.z, v.translation);
    out.position = camera.projection * model * vec4<f32>(v.position, 1.0);
    out.uv = v.uv;
    out.weights = v.weights;
    return out;
}

@fragment fn fragment(v: Output) -> @location(0) vec4<f32> {
    let color = textureSample(detail_0, detail_sampler, v.uv) * v.weights.x
        + textureSample(detail_1, detail_sampler, v.uv) * v.weights.y
        + textureSample(detail_2, detail_sampler, v.uv) * v.weights.z
        + textureSample(detail_3, detail_sampler, v.uv) * v.weights.w;
    return vec4<f32>(color.rgb, 1.0);
}
