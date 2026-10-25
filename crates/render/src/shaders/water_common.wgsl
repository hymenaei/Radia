struct WaterSurface {
    fog_color: vec4<f32>,
    params: vec4<f32>,
    normal: vec4<f32>,
    waves: vec4<f32>,
    scroll: vec4<f32>,
    motion: vec4<f32>,
    origin_position: vec4<f32>,
    origin_uv: vec4<f32>,
}

struct WaterCoordinates {
    first: vec2<f32>,
    second: vec2<f32>,
}

fn water_coordinates(position: vec3<f32>, water: WaterSurface) -> WaterCoordinates {
    var point = position.xy;
    let world_point = position.xy + water.origin_position.xy;
    point.x += (cos(world_point.x * 0.08) + sin(world_point.y * 0.02)) * 6.0;
    let scale = clamp(water.normal.w / 0.0299, 0.0, 4.0);
    var coordinates: WaterCoordinates;
    coordinates.first = point * vec2<f32>(0.45, 0.9) * scale + water.scroll.xy + water.origin_uv.xy;
    coordinates.second = point * vec2<f32>(0.1, 0.2) * scale + water.scroll.zw + water.origin_uv.zw;
    return coordinates;
}

fn water_normal(first_sample: vec3<f32>, second_sample: vec3<f32>, water: WaterSurface) -> vec3<f32> {
    if water.params.w <= 0.5 {
        return vec3<f32>(0.0, 0.0, 1.0);
    }
    let first = first_sample * 2.0 - vec3<f32>(1.0);
    let second = second_sample * 2.0 - vec3<f32>(1.0);
    let normal_scale = clamp(water.normal.xyz, vec3<f32>(-16.0), vec3<f32>(16.0));
    let wave = first.xy * normal_scale.xy + second.xy * normal_scale.xy;
    let strength = clamp(water.normal.w / 0.0299, 0.0, 4.0);
    let vertical = 1.0 + (first.z + second.z) * normal_scale.z * 0.01;
    return safe_normalize(vec3<f32>(wave * strength * 0.035, max(vertical, 0.1)));
}

fn blended_water_normal(
    first_current: vec3<f32>,
    first_next: vec3<f32>,
    second_current: vec3<f32>,
    second_next: vec3<f32>,
    water: WaterSurface,
) -> vec3<f32> {
    let blend = clamp(water.motion.y, 0.0, 1.0);
    return water_normal(mix(first_current, first_next, blend), mix(second_current, second_next, blend), water);
}

fn water_surface_color(
    direction: vec3<f32>,
    normal: vec3<f32>,
    distance: f32,
    draw_distance: f32,
    water: WaterSurface,
    atmosphere: Atmosphere,
) -> vec3<f32> {
    let view = -safe_normalize(direction);
    let reflection = reflect(direction, normal);
    let reflected_sky = horizon_color(reflection, atmosphere);
    let cosine = max(dot(normal, view), 0.0);
    let fresnel = clamp(water.params.x + water.params.y * pow(1.0 - cosine, 5.0), 0.0, 1.0);
    let body = mix(water.fog_color.xyz, reflected_sky, fresnel);
    let sun = safe_normalize(atmosphere.light_direction.xyz);
    let halfway = safe_normalize(sun + view);
    let blur = clamp(water.motion.x * 2.0, 0.0, 1.0);
    let exponent = mix(256.0, 12.0, blur);
    let sun_up = select(0.0, 1.0, atmosphere.sunlight.w >= 0.5);
    let glint = pow(max(dot(normal, halfway), 0.0), exponent) * max(atmosphere.sunlight.xyz, vec3<f32>(0.0)) * sun_up * 0.25;
    return max(aerial_perspective(body + glint, direction, distance, draw_distance, water.params.z, atmosphere), vec3<f32>(0.0));
}

fn water_under_surface_color(
    direction: vec3<f32>,
    normal: vec3<f32>,
    distance: f32,
    draw_distance: f32,
    water: WaterSurface,
    atmosphere: Atmosphere,
) -> vec3<f32> {
    let horizon = horizon_color(vec3<f32>(direction.xy, 0.0), atmosphere);
    let cosine = max(dot(normal, -safe_normalize(direction)), 0.0);
    let fresnel = clamp(water.params.x + water.params.y * pow(1.0 - cosine, 5.0), 0.0, 1.0);
    return aerial_perspective(
        mix(water.fog_color.xyz, horizon, fresnel),
        direction,
        distance,
        draw_distance,
        water.params.z,
        atmosphere,
    );
}
