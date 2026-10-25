struct Atmosphere {
    blue_horizon: vec4<f32>,
    blue_density: vec4<f32>,
    haze: vec4<f32>,
    sunlight: vec4<f32>,
    light_direction: vec4<f32>,
    ambient: vec4<f32>,
    glow: vec4<f32>,
    distance: vec4<f32>,
}

struct AtmosphereTerms {
    ray: vec3<f32>,
    light: vec3<f32>,
    light_atten: vec3<f32>,
    blue_weight: vec3<f32>,
    haze_weight: vec3<f32>,
    sunlight: vec3<f32>,
    transparency: vec3<f32>,
    glow_term: f32,
    ambient_below: vec3<f32>,
    additive_below: vec3<f32>,
}

fn safe_normalize(value: vec3<f32>) -> vec3<f32> {
    let squared = dot(value, value);
    if squared > 0.000001 {
        return value * inverseSqrt(squared);
    }
    return vec3<f32>(1.0, 0.0, 0.0);
}

fn smooth_positive(value: f32) -> f32 {
    return 0.5 * (value + sqrt(value * value + 0.000001));
}

fn atmosphere_terms(direction: vec3<f32>, atmosphere: Atmosphere) -> AtmosphereTerms {
    var terms: AtmosphereTerms;
    let max_y = max(atmosphere.haze.w, 0.0);
    let altitude = clamp(atmosphere.distance.y, -32000.0, max_y);
    var relative_position = safe_normalize(vec3<f32>(direction.x, direction.z, direction.y)) * 15000.0;
    relative_position.y += 50.0 - altitude;
    terms.ray = safe_normalize(relative_position);
    let light_world = safe_normalize(atmosphere.light_direction.xyz);
    terms.light = vec3<f32>(light_world.x, light_world.z, light_world.y);
    let up = terms.ray.y;
    let below = 1.0 - smoothstep(-0.05, 0.05, up);
    let upper_span = max_y;
    let lower_span = 32000.0;
    let span = mix(upper_span, lower_span, below);
    let rel_pos_len = span * inverseSqrt(up * up + 0.000001);
    terms.light_atten = (atmosphere.blue_density.xyz + vec3<f32>(atmosphere.haze.y * 0.25))
        * (atmosphere.haze.z * max_y);
    let combined_density = max(abs(atmosphere.blue_density.xyz) + vec3<f32>(abs(atmosphere.haze.y)), vec3<f32>(0.000001));
    terms.blue_weight = atmosphere.blue_density.xyz / combined_density;
    terms.haze_weight = vec3<f32>(atmosphere.haze.y) / combined_density;
    let sunlight_factor = select(0.7, 1.0, atmosphere.sunlight.w >= 0.5);
    let sunlight = max(atmosphere.sunlight.xyz, vec3<f32>(0.0)) * sunlight_factor;
    let off_axis = 1.0 / max(0.000001, smooth_positive(up) + terms.light.y);
    terms.sunlight = sunlight * exp(-terms.light_atten * off_axis);
    terms.transparency = exp(-combined_density * (rel_pos_len * atmosphere.haze.z));
    let glow_angle = max(1.0 - dot(terms.ray, terms.light), 0.001) * atmosphere.glow.x;
    let glow_factor = atmosphere.distance.w;
    terms.glow_term = select(0.0, glow_factor * (pow(glow_angle, atmosphere.glow.z) + 0.25), glow_factor >= 1.0);
    terms.ambient_below = atmosphere.ambient.xyz
        + max(vec3<f32>(0.0), vec3<f32>(1.0) - atmosphere.ambient.xyz) * atmosphere.glow.w * 0.5;
    let sunlight_below = terms.sunlight * max(1.0 - atmosphere.glow.w, 0.0);
    let blue_haze = atmosphere.blue_horizon.xyz * terms.blue_weight;
    let haze_color = vec3<f32>(atmosphere.haze.x) * terms.haze_weight;
    terms.additive_below = blue_haze * (sunlight_below + terms.ambient_below)
        + haze_color * (sunlight_below * terms.glow_term + terms.ambient_below);
    return terms;
}

fn windlight_haze(direction: vec3<f32>, atmosphere: Atmosphere) -> vec3<f32> {
    if atmosphere.ambient.w <= 0.0 {
        return vec3<f32>(0.012);
    }
    let terms = atmosphere_terms(direction, atmosphere);
    let blue_haze = atmosphere.blue_horizon.xyz * terms.blue_weight;
    let haze_color = vec3<f32>(atmosphere.haze.x) * terms.haze_weight;
    let above_cloud = (blue_haze * (terms.sunlight + atmosphere.ambient.xyz)
        + haze_color * (terms.sunlight * terms.glow_term + atmosphere.ambient.xyz))
        * (vec3<f32>(1.0) - terms.transparency);
    let below_cloud_blend = vec3<f32>(1.0) - sqrt(terms.transparency);
    return above_cloud + (terms.additive_below - above_cloud) * below_cloud_blend;
}

fn windlight_soft_clip(color: vec3<f32>) -> vec3<f32> {
    return clamp(color * 2.0, vec3<f32>(0.0), vec3<f32>(5.0));
}

fn horizon_color(direction: vec3<f32>, atmosphere: Atmosphere) -> vec3<f32> {
    return windlight_soft_clip(windlight_haze(direction, atmosphere));
}

fn atmosphere_color(direction: vec3<f32>, atmosphere: Atmosphere) -> vec3<f32> {
    return horizon_color(direction, atmosphere);
}

fn aerial_perspective(
    color: vec3<f32>,
    direction: vec3<f32>,
    distance: f32,
    draw_distance: f32,
    density: f32,
    atmosphere: Atmosphere,
) -> vec3<f32> {
    if atmosphere.ambient.w <= 0.0 {
        return color;
    }
    let horizon = horizon_color(vec3<f32>(direction.xy, 0.0), atmosphere);
    let safe_draw_distance = max(draw_distance, 0.1);
    let distance_haze = 1.0 - exp(-max(distance, 0.0) * max(density, 0.0) * max(atmosphere.distance.x, 0.0) * 0.001);
    let draw_edge = smoothstep(safe_draw_distance * 0.84, safe_draw_distance, distance);
    return mix(color, horizon, max(distance_haze, draw_edge));
}
