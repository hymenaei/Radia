struct SkyUniform {
    inverse: mat4x4<f32>,
    atmosphere: Atmosphere,
    sun: vec4<f32>,
    moon: vec4<f32>,
    cloud_color: vec4<f32>,
    cloud_density1: vec4<f32>,
    cloud_density2: vec4<f32>,
    cloud: vec4<f32>,
    scroll: vec4<f32>,
    blends: vec4<f32>,
    texture_flags: vec4<f32>,
    cloud_flags: vec4<f32>,
}
@group(0) @binding(0) var<uniform> sky: SkyUniform;
@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;
@group(2) @binding(0) var image_next: texture_2d<f32>;
@group(2) @binding(1) var image_next_sampler: sampler;

struct SkyRay {
    @builtin(position) position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
    @location(1) @interpolate(flat) sample_index: u32,
}

@vertex fn vertex(@builtin(vertex_index) index: u32, @builtin(instance_index) instance: u32) -> SkyRay {
    let positions = array(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var ray: SkyRay;
    ray.ndc = positions[index];
    ray.position = vec4<f32>(ray.ndc, 0.0, 1.0);
    ray.sample_index = instance;
    return ray;
}

fn ray_direction(ndc: vec2<f32>) -> vec3<f32> {
    let near = sky.inverse * vec4<f32>(ndc, 1.0, 1.0);
    return safe_normalize(near.xyz / near.w);
}

fn sphere_uv(direction: vec3<f32>, body_direction: vec3<f32>, radius: f32) -> vec2<f32> {
    var right = cross(vec3<f32>(0.0, 0.0, 1.0), body_direction);
    right = safe_normalize(right);
    let up = safe_normalize(cross(body_direction, right));
    return vec2<f32>(0.5) + vec2<f32>(dot(direction, right), dot(direction, up)) / (2.0 * radius);
}

fn cloud_noise(uv: vec2<f32>) -> vec4<f32> {
    return mix(
        textureSample(image, image_sampler, uv),
        textureSample(image_next, image_next_sampler, uv),
        sky.blends.z,
    );
}

@fragment fn sky_fragment(ray: SkyRay) -> @location(0) vec4<f32> {
    let direction = ray_direction(ray.ndc);
    let color = atmosphere_color(direction, sky.atmosphere);
    return vec4<f32>(color, 1.0);
}

@fragment fn cloud_fragment(ray: SkyRay) -> @location(0) vec4<f32> {
    let direction = ray_direction(ray.ndc);
    if sky.cloud.x < 0.001 {
        discard;
    }
    var uv1 = vec2<f32>((direction.x - 1.0) * 0.5, (1.0 - direction.y) * 0.5);
    uv1 = (uv1 - vec2<f32>(0.5)) / sky.cloud.x + vec2<f32>(0.5);
    let light_world = safe_normalize(sky.atmosphere.light_direction.xyz);
    let light = vec3<f32>(light_world.x, light_world.z, light_world.y);
    let uv2 = uv1 + light.xz * 0.0125;
    let uv3 = uv1 * 16.0;
    let uv4 = uv2 * 16.0;
    let disturbance_scale = sky.cloud.y * (1.0 - sky.cloud.x * 0.25);
    let disturbance = vec2<f32>(cloud_noise(uv1 / 8.0).x, cloud_noise((uv3 + uv1) / 16.0).x)
        * disturbance_scale;
    let disturbance2 = vec2<f32>(cloud_noise((uv1 + uv3) / 4.0).x, cloud_noise((uv4 + uv2) / 8.0).x)
        * disturbance_scale;
    let scroll_offset = vec2<f32>(-sky.scroll.x, sky.scroll.y);
    let large_offset = sky.cloud_density1.xy + scroll_offset;
    let small_offset = sky.cloud_density2.xy;
    let visible_large = uv1 + large_offset + disturbance * 0.2;
    let shadow_large = uv2 + large_offset;
    let visible_small = uv3 + small_offset;
    let shadow_small = uv4 + small_offset;
    let density_variance = min(1.0, max(0.0, (disturbance.x * 2.0 + disturbance.y * 2.0 + disturbance2.x + disturbance2.y) * 4.0));
    let cloud_density = 2.0 * (sky.atmosphere.glow.w - 0.25) * (1.0 - density_variance * density_variance);
    var alpha1 = (cloud_noise(visible_large).x - 0.5)
        + (cloud_noise(visible_small).x - 0.5) * sky.cloud_density2.z;
    alpha1 = min(max(alpha1 + cloud_density, 0.0) * 10.0 * sky.cloud_density1.z, 1.0);
    alpha1 = 1.0 - alpha1 * alpha1;
    alpha1 = 1.0 - alpha1 * alpha1;
    let relative_altitude = direction.z * max(sky.atmosphere.haze.w - sky.atmosphere.distance.y, 0.0);
    let altitude_blend = clamp((relative_altitude + 512.0) / max(sky.atmosphere.haze.w, 1.0), 0.0, 1.0);
    alpha1 *= altitude_blend * smoothstep(-0.025, 0.075, direction.z);
    alpha1 = clamp(alpha1, 0.0, 1.0);
    var alpha2 = cloud_noise(shadow_large).x - 0.5;
    alpha2 = min(max(alpha2 + cloud_density, 0.0) * 2.5 * sky.cloud_density1.z, 1.0);
    alpha2 = 1.0 - alpha2;
    alpha2 = 1.0 - alpha2 * alpha2;
    let terms = atmosphere_terms(direction, sky.atmosphere);
    let sun_up = sky.atmosphere.sunlight.w >= 0.5;
    let cloud_light_factor = select(0.7, 1.0, sun_up);
    let cloud_off_axis = 1.0 / max(0.000001, terms.light.y * 2.0);
    let cloud_sunlight = max(sky.atmosphere.sunlight.xyz, vec3<f32>(0.0))
        * cloud_light_factor
        * exp(-terms.light_atten * cloud_off_axis);
    let haze = sqrt(terms.transparency);
    let cloud_sun = cloud_sunlight * terms.glow_term * sky.cloud_color.xyz * haze;
    let cloud_ambient = terms.ambient_below * sky.cloud_color.xyz * haze
        + terms.additive_below * (vec3<f32>(1.0) - haze);
    let color = clamp(cloud_sun * (1.0 - alpha2) + cloud_ambient, vec3<f32>(0.0), vec3<f32>(1.0)) * 2.0;
    return vec4<f32>(color, alpha1);
}

fn body_fragment_color(direction: vec3<f32>, body: vec4<f32>, sample_index: u32, is_sun: bool) -> vec4<f32> {
    let body_direction = safe_normalize(body.xyz);
    let radius = clamp(body.w * 0.009, 0.002, 0.06);
    let separation = sqrt(max(1.0 - pow(dot(direction, body_direction), 2.0), 0.0));
    let disc = 1.0 - smoothstep(radius * 0.88, radius, separation);
    let altitude = smoothstep(-0.04, 0.1, body_direction.z);
    let weights = select(vec2<f32>(sky.texture_flags.z, sky.texture_flags.w), vec2<f32>(sky.texture_flags.x, sky.texture_flags.y), is_sun);
    let texture_present = select(weights.x, weights.y, sample_index == 1u) > 0.5;
    let blend = select(sky.blends.y, sky.blends.x, is_sun);
    let weight = select(1.0 - blend, blend, sample_index == 1u);
    let uv = sphere_uv(direction, body_direction, radius);
    var color = vec3<f32>(0.74, 0.78, 0.9);
    var alpha = disc;
    if texture_present {
        let sampled = textureSample(image, image_sampler, uv);
        color = sampled.rgb;
        alpha *= sampled.a;
    }
    if is_sun {
        color *= max(sky.atmosphere.sunlight.xyz, vec3<f32>(0.0));
    } else {
        color *= sky.cloud_flags.z;
    }
    return vec4<f32>(color, alpha * altitude * weight);
}

@fragment fn sun_fragment(ray: SkyRay) -> @location(0) vec4<f32> {
    return body_fragment_color(ray_direction(ray.ndc), sky.sun, ray.sample_index, true);
}

@fragment fn moon_fragment(ray: SkyRay) -> @location(0) vec4<f32> {
    return body_fragment_color(ray_direction(ray.ndc), sky.moon, ray.sample_index, false);
}
