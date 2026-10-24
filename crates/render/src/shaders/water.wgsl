struct WaterUniform {
    projection: mat4x4<f32>,
    inverse: mat4x4<f32>,
    plane: vec4<f32>,
    color: vec4<f32>,
    regions: array<vec4<f32>, 33>,
}
@group(0) @binding(0) var<uniform> water: WaterUniform;

struct Ray {
    @builtin(position) position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex fn vertex(@builtin(vertex_index) index: u32) -> Ray {
    let positions = array(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var ray: Ray;
    ray.ndc = positions[index];
    ray.position = vec4<f32>(ray.ndc, 0.0, 1.0);
    return ray;
}

struct Water {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@fragment fn fragment(ray: Ray) -> Water {
    let near = water.inverse * vec4<f32>(ray.ndc, 1.0, 1.0);
    let direction = normalize(near.xyz / near.w);
    if direction.z >= 0.0 { discard; }
    let point = vec4<f32>(direction * -water.plane.x, -direction.z);
    for (var i = 0u; i < u32(water.plane.y); i += 1u) {
        let region = water.regions[i];
        if all(point.xy >= region.xy * point.w) && all(point.xy < (region.xy + region.zw) * point.w) { discard; }
    }
    let clip = water.projection * point;
    if clip.w < 0.05 * point.w { discard; }
    var output: Water;
    output.color = water.color;
    output.depth = clamp(clip.z / clip.w, 0.0, 1.0);
    return output;
}
