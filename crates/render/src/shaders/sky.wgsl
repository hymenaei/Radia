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
    texture_flags2: vec4<f32>,
    optics: vec4<f32>,
}

@group(0) @binding(0) var<uniform> sky: SkyUniform;
@group(1) @binding(0) var sun_image: texture_2d<f32>;
@group(1) @binding(1) var sun_next_image: texture_2d<f32>;
@group(1) @binding(2) var moon_image: texture_2d<f32>;
@group(1) @binding(3) var moon_next_image: texture_2d<f32>;
@group(1) @binding(4) var cloud_image: texture_2d<f32>;
@group(1) @binding(5) var cloud_next_image: texture_2d<f32>;
@group(1) @binding(6) var bloom_image: texture_2d<f32>;
@group(1) @binding(7) var bloom_next_image: texture_2d<f32>;
@group(1) @binding(8) var rainbow_image: texture_2d<f32>;
@group(1) @binding(9) var rainbow_next_image: texture_2d<f32>;
@group(1) @binding(10) var halo_image: texture_2d<f32>;
@group(1) @binding(11) var halo_next_image: texture_2d<f32>;
@group(1) @binding(12) var environment_sampler: sampler;

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
    if dot(right, right) < 0.000001 {
        right = vec3<f32>(1.0, 0.0, 0.0);
    } else {
        right = safe_normalize(right);
    }
    let up = safe_normalize(cross(body_direction, right));
    return vec2<f32>(0.5) + vec2<f32>(dot(direction, right), dot(direction, up)) / (2.0 * radius);
}

fn cloud_noise(uv: vec2<f32>) -> vec4<f32> {
    let gradient_x = dpdx(uv);
    let gradient_y = dpdy(uv);
    return mix(
        textureSampleGrad(cloud_image, environment_sampler, uv, gradient_x, gradient_y),
        textureSampleGrad(cloud_next_image, environment_sampler, uv, gradient_x, gradient_y),
        sky.blends.z,
    );
}

fn rainbow_color(optical_dot: f32) -> vec3<f32> {
    if sky.optics.x <= 0.0 || max(sky.texture_flags2.x, sky.texture_flags2.y) < 0.5 {
        return vec3<f32>(0.0);
    }
    var vertical = clamp(-0.575 - optical_dot, 0.0, 1.0);
    let interior = max(0.0, vertical - 0.25) * 4.2857;
    vertical = clamp(vertical, 0.0, 0.25) + interior;
    let horizontal = (sky.optics.y - 5.0) / 1024.0 + 0.5;
    let uv = vec2<f32>(horizontal, vertical);
    let first = textureSample(rainbow_image, environment_sampler, uv);
    let second = textureSample(rainbow_next_image, environment_sampler, uv);
    return pow(max(mix(first.rgb, second.rgb, sky.cloud_flags.z), vec3<f32>(0.0)), vec3<f32>(1.8)) * sky.optics.x;
}

fn halo_color(optical_dot: f32) -> vec3<f32> {
    if sky.optics.z <= 0.0 || max(sky.texture_flags2.z, sky.texture_flags2.w) < 0.5 {
        return vec3<f32>(0.0);
    }
    let dot_value = clamp(optical_dot, 0.1, 1.0);
    let vertical = sqrt(clamp(1.0 - dot_value * dot_value, 0.0, 1.0));
    let uv = vec2<f32>(0.0, vertical);
    let first = textureSample(halo_image, environment_sampler, uv);
    let second = textureSample(halo_next_image, environment_sampler, uv);
    return mix(first.rgb, second.rgb, sky.cloud_flags.w) * sky.optics.z;
}

@fragment fn sky_fragment(ray: SkyRay) -> @location(0) vec4<f32> {
    let direction = ray_direction(ray.ndc);
    let terms = atmosphere_terms(direction, sky.atmosphere);
    let optical_dot = dot(terms.ray, terms.light);
    let color = windlight_soft_clip(windlight_haze(direction, sky.atmosphere) + rainbow_color(optical_dot) + halo_color(optical_dot));
    return vec4<f32>(color, 1.0);
}

@fragment fn cloud_fragment(ray: SkyRay) -> @location(0) vec4<f32> {
    let direction = ray_direction(ray.ndc);
    if sky.cloud.x < 0.001 {
        discard;
    }
    let projected_up = max(direction.z, sky.cloud.w);
    let uv_h = direction.xy * ((1.0 + sky.cloud.z) / (projected_up + sky.cloud.z));
    let planar = vec2<f32>((-uv_h.y + 1.0) * 0.5, (-uv_h.x + 1.0) * 0.5);
    var uv1 = vec2<f32>(-planar.x, planar.y);
    uv1 = (uv1 - vec2<f32>(0.5)) / sky.cloud.x + vec2<f32>(0.5);
    let light_world = safe_normalize(sky.atmosphere.light_direction.xyz);
    let light = vec3<f32>(light_world.x, light_world.z, light_world.y);
    let uv2 = uv1 + vec2<f32>(light.x, light.z) * 0.0125;
    var uv3 = uv1 * 16.0;
    let uv4 = uv2 * 16.0;
    let disturbance_scale = sky.cloud.y * (1.0 - sky.cloud.x * 0.25);
    let disturbance = vec2<f32>(cloud_noise(uv1 / 8.0).x, cloud_noise((uv3 + uv1) / 16.0).x) * disturbance_scale;
    let disturbance2 = vec2<f32>(cloud_noise((uv1 + uv3) / 4.0).x, cloud_noise((uv4 + uv2) / 8.0).x) * disturbance_scale;
    let scroll = vec2<f32>(-sky.scroll.x, sky.scroll.y);
    uv1 += sky.cloud_density1.xy + scroll + disturbance * 0.2;
    let shadow_large = uv2 + sky.cloud_density1.xy + scroll;
    uv3 += sky.cloud_density2.xy;
    let density_variance = min(1.0, max(0.0, (disturbance.x * 2.0 + disturbance.y * 2.0 + disturbance2.x + disturbance2.y) * 4.0));
    let cloud_density = 2.0 * (sky.atmosphere.glow.w - 0.25) * (1.0 - density_variance * density_variance);
    var alpha1 = (cloud_noise(uv1).x - 0.5) + (cloud_noise(uv3).x - 0.5) * sky.cloud_density2.z;
    alpha1 = min(max(alpha1 + cloud_density, 0.0) * 10.0 * sky.cloud_density1.z, 1.0);
    alpha1 = 1.0 - alpha1 * alpha1;
    alpha1 = 1.0 - alpha1 * alpha1;
    alpha1 *= smoothstep(sky.cloud.w, sky.cloud_density2.w, direction.z);
    alpha1 = clamp(alpha1, 0.0, 1.0);
    var alpha2 = cloud_noise(shadow_large).x - 0.5;
    alpha2 = min(max(alpha2 + cloud_density, 0.0) * 2.5 * sky.cloud_density1.z, 1.0);
    alpha2 = 1.0 - alpha2;
    alpha2 = 1.0 - alpha2 * alpha2;
    let terms = atmosphere_terms(direction, sky.atmosphere);
    let cloud_shadow = sky.atmosphere.glow.w;
    let ambient = sky.atmosphere.ambient.xyz
        + (vec3<f32>(1.0) - sky.atmosphere.ambient.xyz) * cloud_shadow * 0.5;
    let sunlight = terms.sunlight * max(1.0 - cloud_shadow, 0.0);
    let below_cloud = sky.atmosphere.blue_horizon.xyz * terms.blue_weight * (sunlight + ambient)
        + vec3<f32>(sky.atmosphere.haze.x) * terms.haze_weight * (sunlight * terms.glow_term + ambient);
    let cloud_sunlight = max(sky.atmosphere.sunlight.xyz, vec3<f32>(0.0))
        * exp(-terms.light_atten / max(terms.light.y * 2.0, 0.000001));
    let transparency = sqrt(terms.transparency);
    let sun_color = cloud_sunlight * terms.glow_term * sky.cloud_color.xyz * transparency;
    let ambient_color = ambient * sky.cloud_color.xyz * transparency
        + below_cloud * (vec3<f32>(1.0) - transparency);
    let color = clamp(sun_color * (1.0 - alpha2) + ambient_color, vec3<f32>(0.0), vec3<f32>(1.0)) * 2.0;
    return vec4<f32>(color, alpha1);
}

fn body_fragment_color(direction: vec3<f32>, body: vec4<f32>, sample_index: u32, is_sun: bool) -> vec4<f32> {
    let body_direction = safe_normalize(body.xyz);
    let radius = clamp(body.w * 0.009, 0.0, 0.06);
    if radius <= 0.000001 {
        discard;
    }
    let separation = sqrt(max(1.0 - pow(dot(direction, body_direction), 2.0), 0.0));
    let disc = 1.0 - smoothstep(radius * 0.88, radius, separation);
    let uv = sphere_uv(direction, body_direction, radius);
    var current = textureSample(sun_image, environment_sampler, uv);
    var next = textureSample(sun_next_image, environment_sampler, uv);
    if !is_sun {
        current = textureSample(moon_image, environment_sampler, uv);
        next = textureSample(moon_next_image, environment_sampler, uv);
    }
    if is_sun {
        let textured = max(sky.texture_flags.x, sky.texture_flags.y) > 0.5;
        let texture_color = mix(current, next, sky.blends.x);
        let solar_color = select(max(sky.atmosphere.sunlight.xyz, vec3<f32>(0.0)), texture_color.rgb, textured);
        let texture_alpha = select(1.0, texture_color.a, textured);
        let fade = smoothstep(-0.02, 0.04, body_direction.z);
        let alpha = disc * texture_alpha * fade;
        return vec4<f32>(solar_color, alpha);
    }
    let textured = max(sky.texture_flags.z, sky.texture_flags.w) > 0.5;
    if !textured {
        discard;
    }
    let texture_color = mix(current, next, sky.blends.y);
    if texture_color.a <= 2.0 / 255.0 {
        discard;
    }
    let moon_up_fade = select(1.0, clamp(body_direction.z * body_direction.z * 4.0, 0.0, 1.0), body_direction.z > 0.0);
    let alpha = disc * texture_color.a * moon_up_fade;
    return vec4<f32>(texture_color.rgb * sky.cloud_flags.x, alpha);
}

@fragment fn sun_fragment(ray: SkyRay) -> @location(0) vec4<f32> {
    return body_fragment_color(ray_direction(ray.ndc), sky.sun, ray.sample_index, true);
}

@fragment fn moon_fragment(ray: SkyRay) -> @location(0) vec4<f32> {
    return body_fragment_color(ray_direction(ray.ndc), sky.moon, ray.sample_index, false);
}
