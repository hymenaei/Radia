struct Atmosphere {
    blue_horizon: vec4<f32>,
    blue_density: vec4<f32>,
    haze: vec4<f32>,
    sunlight: vec4<f32>,
    light_direction: vec4<f32>,
    ambient: vec4<f32>,
    glow: vec4<f32>,
    distance: vec4<f32>,
    light_norm: vec4<f32>,
    water_specular: vec4<f32>,
    dome: vec4<f32>,
}

struct AtmosphereTerms {
    ray: vec3<f32>,
    light: vec3<f32>,
    light_attenuation: vec3<f32>,
    blue_weight: vec3<f32>,
    haze_weight: vec3<f32>,
    sunlight: vec3<f32>,
    transparency: vec3<f32>,
    glow: f32,
    ambient_below: vec3<f32>,
    additive_below: vec3<f32>,
}

struct ObjectAtmosphereTerms {
    sunlight: vec3<f32>,
    additive: vec3<f32>,
    attenuation: vec3<f32>,
}

fn safe_normalize(value: vec3<f32>) -> vec3<f32> {
    let squared = dot(value, value);
    if squared > 0.000001 {
        return value * inverseSqrt(squared);
    }
    return vec3<f32>(0.0, 0.0, 1.0);
}

fn world_to_dome(value: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(value.y, value.z, value.x);
}

fn srgb_to_linear(color: vec3<f32>) -> vec3<f32> {
    let low = color / 12.92;
    let high = pow((color + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(high, low, color <= vec3<f32>(0.04045));
}

fn linear_to_srgb(color: vec3<f32>) -> vec3<f32> {
    let low = color * 12.92;
    let high = 1.055 * pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(high, low, color <= vec3<f32>(0.0031308));
}

fn sky_dome_position(direction: vec3<f32>, atmosphere: Atmosphere) -> vec3<f32> {
    let dome_direction = world_to_dome(safe_normalize(direction));
    let radius = atmosphere.dome.x;
    let camera_height = atmosphere.dome.y;
    let height_along_ray = dome_direction.y * camera_height;
    let distance = -height_along_ray + sqrt(max(height_along_ray * height_along_ray + radius * radius - camera_height * camera_height, 0.0));
    return dome_direction * distance + vec3<f32>(0.0, 50.0, 0.0);
}

fn clamp_sky_altitude(position: vec3<f32>, max_y: f32) -> vec3<f32> {
    var relative_position = position;
    if relative_position.y > 0.0 {
        relative_position *= max_y / relative_position.y;
    }
    if relative_position.y < 0.0 {
        relative_position *= -32000.0 / relative_position.y;
    }
    return relative_position;
}

fn sky_atmosphere_terms(relative_position: vec3<f32>, atmosphere: Atmosphere) -> AtmosphereTerms {
    var terms: AtmosphereTerms;
    terms.ray = safe_normalize(relative_position);
    terms.light = world_to_dome(atmosphere.light_norm.xyz);
    terms.light_attenuation = (atmosphere.blue_density.xyz + vec3<f32>(atmosphere.haze.y * 0.25))
        * (atmosphere.haze.z * atmosphere.haze.w);
    let combined_density = max(abs(atmosphere.blue_density.xyz) + vec3<f32>(abs(atmosphere.haze.y)), vec3<f32>(0.000001));
    terms.blue_weight = atmosphere.blue_density.xyz / combined_density;
    terms.haze_weight = vec3<f32>(atmosphere.haze.y) / combined_density;
    let sun_light = select(atmosphere.sunlight.xyz * 0.7, atmosphere.sunlight.xyz, atmosphere.sunlight.w >= 0.5);
    let off_axis = 1.0 / max(0.000001, max(0.0, terms.ray.y) + terms.light.y);
    terms.sunlight = sun_light * exp(-terms.light_attenuation * off_axis);
    let density_distance = length(relative_position) * atmosphere.haze.z;
    terms.transparency = exp(-combined_density * density_distance);
    let glow_angle = max(1.0 - dot(terms.ray, terms.light), 0.001) * atmosphere.glow.x;
    let glow_value = clamp(pow(glow_angle, atmosphere.glow.z), -100000.0, 100000.0);
    terms.glow = select(0.0, atmosphere.distance.w * (glow_value + 0.25), atmosphere.distance.w >= 1.0);
    let cloud_shadow = atmosphere.glow.w;
    terms.ambient_below = atmosphere.ambient.xyz
        + max(vec3<f32>(0.0), vec3<f32>(1.0) - atmosphere.ambient.xyz) * cloud_shadow * 0.5;
    let sunlight_below = terms.sunlight * max(1.0 - cloud_shadow, 0.0);
    let blue_haze = atmosphere.blue_horizon.xyz * terms.blue_weight;
    let haze_color = vec3<f32>(atmosphere.haze.x) * terms.haze_weight;
    terms.additive_below = blue_haze * (sunlight_below + terms.ambient_below)
        + haze_color * (sunlight_below * terms.glow + terms.ambient_below);
    return terms;
}

fn windlight_sky_base(relative_position: vec3<f32>, atmosphere: Atmosphere) -> vec3<f32> {
    let terms = sky_atmosphere_terms(relative_position, atmosphere);
    let blue_haze = atmosphere.blue_horizon.xyz * terms.blue_weight;
    let haze_color = vec3<f32>(atmosphere.haze.x) * terms.haze_weight;
    var color = (blue_haze * (terms.sunlight + atmosphere.ambient.xyz)
        + haze_color * (terms.sunlight * terms.glow + atmosphere.ambient.xyz)) * (vec3<f32>(1.0) - terms.transparency);
    color += (terms.additive_below - color) * (vec3<f32>(1.0) - sqrt(sqrt(terms.transparency)));
    return color;
}

fn sky_linear_color(direction: vec3<f32>, atmosphere: Atmosphere) -> vec3<f32> {
    let relative_position = clamp_sky_altitude(sky_dome_position(direction, atmosphere), atmosphere.haze.w);
    let color = clamp(windlight_sky_base(relative_position, atmosphere) * 2.0, vec3<f32>(0.0), vec3<f32>(5.0));
    return srgb_to_linear(color) * atmosphere.distance.z;
}

fn object_atmosphere_terms(
    position_eye: vec3<f32>,
    light_direction_eye: vec3<f32>,
    light_norm_eye: vec3<f32>,
    atmosphere: Atmosphere,
) -> ObjectAtmosphereTerms {
    var relative_position = position_eye;
    if abs(relative_position.y) > atmosphere.haze.w {
        relative_position *= atmosphere.haze.w / relative_position.y;
    }
    let relative_position_direction = safe_normalize(relative_position);
    let combined_density = max(atmosphere.blue_density.xyz + vec3<f32>(atmosphere.haze.y), vec3<f32>(0.000001));
    let blue_weight = atmosphere.blue_density.xyz / combined_density;
    let haze_weight = vec3<f32>(atmosphere.haze.y) / combined_density;
    let light_attenuation = (atmosphere.blue_density.xyz + vec3<f32>(atmosphere.haze.y * 0.25))
        * (atmosphere.haze.z * atmosphere.haze.w);
    let above_horizon_factor = 1.0 / max(0.000001, light_norm_eye.y);
    var terms: ObjectAtmosphereTerms;
    terms.sunlight = atmosphere.sunlight.xyz * exp(-light_attenuation * above_horizon_factor);
    terms.attenuation = exp(-combined_density * length(relative_position) * atmosphere.haze.z * atmosphere.distance.x);
    var glow = dot(relative_position_direction, light_norm_eye) * max(0.0, dot(light_direction_eye, relative_position_direction));
    glow = max(1.0 - glow, 0.001) * atmosphere.glow.x;
    glow = clamp(pow(glow, atmosphere.glow.z), -100000.0, 100000.0) + 0.25;
    glow *= atmosphere.distance.w;
    let cloud_shadow = atmosphere.glow.w;
    let ambient = atmosphere.ambient.xyz + (vec3<f32>(1.0) - atmosphere.ambient.xyz) * cloud_shadow * 0.5;
    let sunlight_below = terms.sunlight * (1.0 - cloud_shadow);
    terms.additive = atmosphere.blue_horizon.xyz * blue_weight * (sunlight_below + ambient)
        + vec3<f32>(atmosphere.haze.x) * haze_weight * (sunlight_below * glow + ambient);
    terms.additive = min(terms.additive * (vec3<f32>(1.0) - terms.attenuation), vec3<f32>(10.0));
    return terms;
}

fn atmospheric_surface(
    color: vec3<f32>,
    position_eye: vec3<f32>,
    light_direction_eye: vec3<f32>,
    light_norm_eye: vec3<f32>,
    atmosphere: Atmosphere,
) -> vec3<f32> {
    let terms = object_atmosphere_terms(position_eye, light_direction_eye, light_norm_eye, atmosphere);
    return color * terms.attenuation.x + srgb_to_linear(terms.additive * 2.0) * atmosphere.distance.z;
}
