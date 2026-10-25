struct WaterUniform {
    inverse: mat4x4<f32>,
    view: mat4x4<f32>,
    plane: vec4<f32>,
    surface: WaterSurface,
    atmosphere: Atmosphere,
    regions: array<vec4<f32>, 33>,
}
@group(0) @binding(0) var<uniform> water: WaterUniform;
@group(1) @binding(0) var normal_image: texture_2d<f32>;
@group(1) @binding(1) var normal_next_image: texture_2d<f32>;
@group(1) @binding(2) var normal_sampler: sampler;

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
    if (water.plane.x < 0.0 && direction.z >= 0.0) || (water.plane.x > 0.0 && direction.z <= 0.0) {
        discard;
    }
    let distance = abs(water.plane.x) / max(abs(direction.z), 0.000001);
    let point = direction * min(distance, water.plane.z);
    if distance < water.plane.z {
        for (var index = 0u; index < u32(water.plane.y); index += 1u) {
            let region = water.regions[index];
            if all(point.xy >= region.xy) && all(point.xy < region.xy + region.zw) {
                discard;
            }
        }
    }
    let coordinates = water_coordinates(point, water.surface);
    let blend = water.surface.fresnel.z;
    let wave1 = select(
        blend_normal(
            linear_to_srgb(textureSample(normal_image, normal_sampler, coordinates.big).xyz),
            linear_to_srgb(textureSample(normal_next_image, normal_sampler, coordinates.big).xyz),
            blend,
        ),
        vec3<f32>(0.0, 0.0, 1.0),
        water.surface.fresnel.w < 0.5,
    );
    let wave2 = select(
        blend_normal(
            linear_to_srgb(textureSample(normal_image, normal_sampler, coordinates.first).xyz),
            linear_to_srgb(textureSample(normal_next_image, normal_sampler, coordinates.first).xyz),
            blend,
        ),
        vec3<f32>(0.0, 0.0, 1.0),
        water.surface.fresnel.w < 0.5,
    );
    let wave3 = select(
        blend_normal(
            linear_to_srgb(textureSample(normal_image, normal_sampler, coordinates.second).xyz),
            linear_to_srgb(textureSample(normal_next_image, normal_sampler, coordinates.second).xyz),
            blend,
        ),
        vec3<f32>(0.0, 0.0, 1.0),
        water.surface.fresnel.w < 0.5,
    );
    let position_eye = (water.view * vec4<f32>(point, 1.0)).xyz;
    let light_direction_eye = (water.view * vec4<f32>(water.atmosphere.light_direction.xyz, 0.0)).xyz;
    let light_norm_eye = (water.view * vec4<f32>(water.atmosphere.light_norm.xyz, 0.0)).xyz;
    let normal_eye = (water.view * vec4<f32>(0.0, 0.0, 1.0, 0.0)).xyz;
    let tangent_eye = (water.view * vec4<f32>(1.0, 0.0, 0.0, 0.0)).xyz;
    let color = shade_water(
        point,
        position_eye,
        light_direction_eye,
        light_norm_eye,
        normal_eye,
        tangent_eye,
        water.view,
        wave1,
        wave2,
        wave3,
        water.surface,
        water.atmosphere,
    );
    let finite_depth = water.plane.w * (water.plane.z / max(distance, water.plane.w) - 1.0) / (water.plane.z - water.plane.w);
    let depth = select(max(finite_depth, 0.00000001), 0.00000001, distance >= water.plane.z);
    var output: WaterOutput;
    output.color = color;
    output.depth = depth;
    return output;
}
