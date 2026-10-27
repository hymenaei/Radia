struct VoidWaterUniform {
    inverse: mat4x4<f32>,
    view: mat4x4<f32>,
    plane: vec4<f32>,
    surface: WaterSurface,
    atmosphere: Atmosphere,
    regions: array<vec4<f32>, 33>,
}
@group(0) @binding(0) var<uniform> void_water: VoidWaterUniform;
@group(1) @binding(0) var void_water_normal_image: texture_2d<f32>;
@group(1) @binding(1) var void_water_normal_next_image: texture_2d<f32>;
@group(1) @binding(2) var void_water_normal_sampler: sampler;

struct VoidWaterRay {
    @builtin(position) position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex fn void_water_vertex(@builtin(vertex_index) index: u32) -> VoidWaterRay {
    let positions = array(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var ray: VoidWaterRay;
    ray.ndc = positions[index];
    ray.position = vec4<f32>(ray.ndc, 0.0, 1.0);
    return ray;
}

struct VoidWaterOutput {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@fragment fn void_water_fragment(ray: VoidWaterRay) -> VoidWaterOutput {
    let near = void_water.inverse * vec4<f32>(ray.ndc, 1.0, 1.0);
    let direction = safe_normalize(near.xyz / near.w);
    let distance = abs(void_water.plane.x) / max(abs(direction.z), 0.000001);
    let true_intersection = direction * distance;
    let direction_eye = (void_water.view * vec4<f32>(direction, 0.0)).xyz;
    let view_distance = distance * -direction_eye.z;
    let coordinates = water_coordinates(true_intersection, void_water.surface);
    let big_dx = dpdx(coordinates.big);
    let big_dy = dpdy(coordinates.big);
    let first_dx = dpdx(coordinates.first);
    let first_dy = dpdy(coordinates.first);
    let second_dx = dpdx(coordinates.second);
    let second_dy = dpdy(coordinates.second);
    if (void_water.plane.x < 0.0 && direction.z >= 0.0) || (void_water.plane.x > 0.0 && direction.z <= 0.0) {
        discard;
    }
    for (var index = 0u; index < u32(void_water.plane.y); index += 1u) {
        let region = void_water.regions[index];
        if all(true_intersection.xy >= region.xy) && all(true_intersection.xy < region.xy + region.zw) {
            discard;
        }
    }
    let blend = void_water.surface.fresnel.z;
    let wave1 = select(
        blend_normal(
            linear_to_srgb(textureSampleGrad(void_water_normal_image, void_water_normal_sampler, coordinates.big, big_dx, big_dy).xyz),
            linear_to_srgb(textureSampleGrad(void_water_normal_next_image, void_water_normal_sampler, coordinates.big, big_dx, big_dy).xyz),
            blend,
        ),
        vec3<f32>(0.0, 0.0, 1.0),
        void_water.surface.fresnel.w < 0.5,
    );
    let wave2 = select(
        blend_normal(
            linear_to_srgb(textureSampleGrad(void_water_normal_image, void_water_normal_sampler, coordinates.first, first_dx, first_dy).xyz),
            linear_to_srgb(textureSampleGrad(void_water_normal_next_image, void_water_normal_sampler, coordinates.first, first_dx, first_dy).xyz),
            blend,
        ),
        vec3<f32>(0.0, 0.0, 1.0),
        void_water.surface.fresnel.w < 0.5,
    );
    let wave3 = select(
        blend_normal(
            linear_to_srgb(textureSampleGrad(void_water_normal_image, void_water_normal_sampler, coordinates.second, second_dx, second_dy).xyz),
            linear_to_srgb(textureSampleGrad(void_water_normal_next_image, void_water_normal_sampler, coordinates.second, second_dx, second_dy).xyz),
            blend,
        ),
        vec3<f32>(0.0, 0.0, 1.0),
        void_water.surface.fresnel.w < 0.5,
    );
    let position_eye = (void_water.view * vec4<f32>(true_intersection, 1.0)).xyz;
    let light_direction_eye = (void_water.view * vec4<f32>(void_water.atmosphere.light_direction.xyz, 0.0)).xyz;
    let light_norm_eye = (void_water.view * vec4<f32>(void_water.atmosphere.light_norm.xyz, 0.0)).xyz;
    let normal_eye = (void_water.view * vec4<f32>(0.0, 0.0, 1.0, 0.0)).xyz;
    let tangent_eye = (void_water.view * vec4<f32>(1.0, 0.0, 0.0, 0.0)).xyz;
    let color = shade_water(
        true_intersection,
        position_eye,
        light_direction_eye,
        light_norm_eye,
        normal_eye,
        tangent_eye,
        void_water.view,
        wave1,
        wave2,
        wave3,
        void_water.surface,
        void_water.atmosphere,
    );
    let depth = max(void_water.plane.z / max(view_distance, void_water.plane.z), 0.00000001);
    var output: VoidWaterOutput;
    output.color = color;
    output.depth = depth;
    return output;
}
