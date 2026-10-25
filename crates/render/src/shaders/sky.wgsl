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

fn star_noise(uv: vec2<f32>) -> f32 {
    return fract(sin(dot(uv, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

@fragment fn sky_fragment(ray: SkyRay) -> @location(0) vec4<f32> {
    let direction = ray_direction(ray.ndc);
    let color = atmosphere_color(direction, sky.atmosphere);
    return vec4<f32>(color, 1.0);
}

@fragment fn cloud_fragment(ray: SkyRay) -> @location(0) vec4<f32> {
    let direction = ray_direction(ray.ndc);
    let longitude = atan2(direction.y, direction.x) / 6.28318530718 + 0.5;
    let latitude = 0.5 - asin(clamp(direction.z, -1.0, 1.0)) / 3.14159265359;
    var uv = vec2<f32>(-longitude, latitude);
    uv = (uv - vec2<f32>(0.5)) * sky.cloud.x + vec2<f32>(0.5);
    uv += vec2<f32>(-sky.scroll.x, sky.scroll.y);
    let resident = select(sky.cloud_flags.x, sky.cloud_flags.y, ray.sample_index == 1u) > 0.5;
    var noise = star_noise(uv * 128.0);
    if resident {
        noise = textureSample(image, image_sampler, uv).r;
    }
    let density = clamp((sky.cloud_density1.z + sky.cloud_density2.z) * 0.5, 0.0, 1.0);
    let threshold = 0.56 - density * 0.28;
    let edge = 0.06 + clamp(sky.cloud.y, 0.0, 1.0) * 0.15;
    let coverage = smoothstep(threshold - edge, threshold + edge, noise);
    let fade = smoothstep(0.0, 0.16, direction.z);
    let weight = select(1.0 - sky.blends.z, sky.blends.z, ray.sample_index == 1u);
    let alpha = coverage * fade * density * weight;
    return vec4<f32>(sky.cloud_color.xyz, alpha);
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
