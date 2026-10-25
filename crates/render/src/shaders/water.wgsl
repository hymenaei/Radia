struct WaterUniform {
    inverse: mat4x4<f32>,
    plane: vec4<f32>,
    fog_color: vec4<f32>,
    params: vec4<f32>,
    normal: vec4<f32>,
    waves: vec4<f32>,
    atmosphere: Atmosphere,
    regions: array<vec4<f32>, 33>,
}
@group(0) @binding(0) var<uniform> water: WaterUniform;
@group(1) @binding(0) var normal_image: texture_2d<f32>;
@group(1) @binding(1) var normal_sampler: sampler;

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

struct WaterOutput {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@fragment fn fragment(ray: Ray) -> WaterOutput {
    let near = water.inverse * vec4<f32>(ray.ndc, 1.0, 1.0);
    let direction = safe_normalize(near.xyz / near.w);
    if direction.z >= 0.0 { discard; }
    let distance = abs(water.plane.x) / max(-direction.z, 0.000001);
    let point = direction * distance;
    if distance < water.plane.z {
        for (var i = 0u; i < u32(water.plane.y); i += 1u) {
            let region = water.regions[i];
            if all(point.xy >= region.xy) && all(point.xy < (region.xy + region.zw)) { discard; }
        }
    }
    let horizon = horizon_color(vec3<f32>(direction.xy, 0.0), water.atmosphere);
    var normal = vec3<f32>(0.0, 0.0, 1.0);
    if water.params.w > 0.5 {
        let uv = point.xy * water.normal.x * 0.015 + water.waves.xy * 0.03 + water.waves.zw * -0.02;
        let sampled = textureSample(normal_image, normal_sampler, uv).xyz * 2.0 - vec3<f32>(1.0);
        normal = safe_normalize(mix(normal, safe_normalize(sampled), 0.12));
    }
    let view = -direction;
    let cosine = max(dot(normal, view), 0.0);
    let fresnel = clamp(water.params.x + water.params.y * pow(1.0 - cosine, 5.0), 0.0, 1.0);
    let body = mix(water.fog_color.xyz, horizon, fresnel);
    let color = aerial_perspective(body, direction, distance, water.plane.z, water.params.z, water.atmosphere);
    let finite_depth = water.plane.w * (water.plane.z / max(distance, water.plane.w) - 1.0) / (water.plane.z - water.plane.w);
    let infinite_tail = 0.00000001 * water.plane.z / max(distance, water.plane.z);
    let depth = select(max(clamp(finite_depth, 0.0, 1.0), 0.00000001), infinite_tail, distance >= water.plane.z);
    var output: WaterOutput;
    output.color = vec4<f32>(color, 1.0);
    output.depth = depth;
    return output;
}
