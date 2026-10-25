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

fn safe_normalize(value: vec3<f32>) -> vec3<f32> {
    let squared = dot(value, value);
    if squared > 0.000001 {
        return value * inverseSqrt(squared);
    }
    return vec3<f32>(1.0, 0.0, 0.0);
}

fn horizon_color(direction: vec3<f32>, atmosphere: Atmosphere) -> vec3<f32> {
    if atmosphere.ambient.w <= 0.0 {
        return vec3<f32>(0.012);
    }
    let ray = safe_normalize(direction);
    let light = safe_normalize(atmosphere.light_direction.xyz);
    let alignment = dot(ray, light);
    let lobe = exp(-sqrt(alignment * alignment + 0.0004) * 24.0);
    let optical = clamp(
        (atmosphere.blue_density.xyz + vec3<f32>(atmosphere.haze.y * 0.25)) * atmosphere.haze.z * atmosphere.haze.w,
        vec3<f32>(0.0),
        vec3<f32>(20.0),
    );
    let transmittance = exp(-optical);
    let sunlight = max(atmosphere.sunlight.xyz, vec3<f32>(0.0)) * (1.0 - clamp(atmosphere.glow.w, 0.0, 1.0));
    let ambient = max(atmosphere.ambient.xyz, vec3<f32>(0.0));
    let blue = max(atmosphere.blue_horizon.xyz * atmosphere.blue_density.xyz, vec3<f32>(0.0));
    let haze = vec3<f32>(max(atmosphere.haze.x, 0.0) * max(atmosphere.haze.y, 0.0));
    let glow = max(atmosphere.glow.xyz, vec3<f32>(0.0)) * lobe;
    return max((blue + haze) * (sunlight + ambient) * transmittance + glow * sunlight, vec3<f32>(0.0));
}

fn display_color(color: vec3<f32>, atmosphere: Atmosphere) -> vec3<f32> {
    if atmosphere.ambient.w <= 0.0 {
        return color;
    }
    return pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / max(atmosphere.ambient.w, 0.05)));
}

fn atmosphere_color(direction: vec3<f32>, atmosphere: Atmosphere) -> vec3<f32> {
    let ray = safe_normalize(direction);
    let horizon = horizon_color(vec3<f32>(ray.xy, 0.0), atmosphere);
    let vertical = ray.z;
    if vertical >= 0.0 {
        let zenith = max(atmosphere.blue_horizon.xyz * (atmosphere.sunlight.xyz + atmosphere.ambient.xyz), horizon);
        return display_color(mix(horizon, zenith, smoothstep(0.0, 0.72, vertical)), atmosphere);
    }
    let lower = mix(horizon, max(atmosphere.ambient.xyz, vec3<f32>(0.012)), 0.42);
    return display_color(mix(horizon, lower, smoothstep(0.0, 0.72, -vertical)), atmosphere);
}

fn aerial_perspective(color: vec3<f32>, direction: vec3<f32>, distance: f32, draw_distance: f32, density: f32, atmosphere: Atmosphere) -> vec3<f32> {
    let effective_density = density * max(atmosphere.distance.x, 0.0);
    let start = draw_distance * clamp(0.84 - effective_density * 0.025, 0.5, 0.84);
    return display_color(
        mix(color, horizon_color(vec3<f32>(direction.xy, 0.0), atmosphere), smoothstep(start, draw_distance, distance)),
        atmosphere,
    );
}
