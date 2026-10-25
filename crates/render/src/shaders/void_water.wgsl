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
    if (void_water.plane.x < 0.0 && direction.z >= 0.0) || (void_water.plane.x > 0.0 && direction.z <= 0.0) {
        discard;
    }
    let distance = abs(void_water.plane.x) / max(abs(direction.z), 0.000001);
    let point = direction * min(distance, void_water.plane.z);
    if distance < void_water.plane.z {
        for (var index = 0u; index < u32(void_water.plane.y); index += 1u) {
            let region = void_water.regions[index];
            if all(point.xy >= region.xy) && all(point.xy < region.xy + region.zw) {
                discard;
            }
        }
    }
    let coordinates = water_coordinates(point, void_water.surface);
    let blend = void_water.surface.fresnel.z;
    let wave1 = select(
        blend_normal(
            linear_to_srgb(textureSample(void_water_normal_image, void_water_normal_sampler, coordinates.big).xyz),
            linear_to_srgb(textureSample(void_water_normal_next_image, void_water_normal_sampler, coordinates.big).xyz),
            blend,
        ),
        vec3<f32>(0.0, 0.0, 1.0),
        void_water.surface.fresnel.w < 0.5,
    );
    let wave2 = select(
        blend_normal(
            linear_to_srgb(textureSample(void_water_normal_image, void_water_normal_sampler, coordinates.first).xyz),
            linear_to_srgb(textureSample(void_water_normal_next_image, void_water_normal_sampler, coordinates.first).xyz),
            blend,
        ),
        vec3<f32>(0.0, 0.0, 1.0),
        void_water.surface.fresnel.w < 0.5,
    );
    let wave3 = select(
        blend_normal(
            linear_to_srgb(textureSample(void_water_normal_image, void_water_normal_sampler, coordinates.second).xyz),
            linear_to_srgb(textureSample(void_water_normal_next_image, void_water_normal_sampler, coordinates.second).xyz),
            blend,
        ),
        vec3<f32>(0.0, 0.0, 1.0),
        void_water.surface.fresnel.w < 0.5,
    );
    let position_eye = (void_water.view * vec4<f32>(point, 1.0)).xyz;
    let light_direction_eye = (void_water.view * vec4<f32>(void_water.atmosphere.light_direction.xyz, 0.0)).xyz;
    let light_norm_eye = (void_water.view * vec4<f32>(void_water.atmosphere.light_norm.xyz, 0.0)).xyz;
    let normal_eye = (void_water.view * vec4<f32>(0.0, 0.0, 1.0, 0.0)).xyz;
    let tangent_eye = (void_water.view * vec4<f32>(1.0, 0.0, 0.0, 0.0)).xyz;
    let color = shade_water(
        point,
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
    let finite_depth = void_water.plane.w * (void_water.plane.z / max(distance, void_water.plane.w) - 1.0) / (void_water.plane.z - void_water.plane.w);
    let depth = select(max(finite_depth, 0.00000001), 0.00000001, distance >= void_water.plane.z);
    var output: VoidWaterOutput;
    output.color = color;
    output.depth = depth;
    return output;
}
