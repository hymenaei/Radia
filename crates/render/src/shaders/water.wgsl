struct WaterFog {
    color: vec4<f32>,
    parameters: vec4<f32>,
}

struct WaterSurface {
    fog: WaterFog,
    normal_scale: vec4<f32>,
    fresnel: vec4<f32>,
    wave_directions: vec4<f32>,
    animation: vec4<f32>,
    uv_origin0: vec4<f32>,
    uv_origin1: vec4<f32>,
}

struct WaterCoordinates {
    big: vec2<f32>,
    first: vec2<f32>,
    second: vec2<f32>,
}

struct PunctualTerms {
    nl: f32,
    diffuse: vec3<f32>,
    specular: vec3<f32>,
}

fn water_coordinates(position: vec3<f32>, water: WaterSurface) -> WaterCoordinates {
    let horizontal_distance = length(position.xy);
    let relative_position = position.xy * (min(horizontal_distance, 2560.0) / max(horizontal_distance, 0.000001));
    let sweep_x = relative_position.x + water.animation.y;
    let sweep_y = relative_position.y + water.animation.z;
    var point = relative_position;
    point.x += (cos(sweep_x * 0.08) + sin(sweep_y * 0.02)) * 6.0;
    let time = water.animation.x;
    var coordinates: WaterCoordinates;
    coordinates.big = point * 0.04 + water.wave_directions.xy * time * 0.055 + water.uv_origin0.xy;
    coordinates.first = point * vec2<f32>(0.45, 0.9) + water.wave_directions.zw * time * 0.13 + water.uv_origin0.zw;
    coordinates.second = point * vec2<f32>(0.1, 0.2) + water.wave_directions.xy * time * 0.1 + water.uv_origin1.xy;
    return coordinates;
}

fn blend_normal(first: vec3<f32>, second: vec3<f32>, factor: f32) -> vec3<f32> {
    return mix(first * 2.0 - vec3<f32>(1.0), second * 2.0 - vec3<f32>(1.0), factor);
}

fn calculate_fresnel(view: vec3<f32>, wave1: vec3<f32>, wave2: vec3<f32>, wave3: vec3<f32>, wavef: vec3<f32>, water: WaterSurface) -> vec2<f32> {
    var df3 = max(
        vec3<f32>(0.0),
        vec3<f32>(dot(view, wave1), dot(view, (wave2 + wave3) * 0.5), dot(view, wave3)) * water.fresnel.y + water.fresnel.x,
    );
    df3 *= df3;
    return max(vec2<f32>(0.0), vec2<f32>(df3.x + df3.y + df3.z, dot(view, wavef) * water.fresnel.y + water.fresnel.x));
}

fn water_fog_view(position: vec3<f32>, water: WaterFog) -> vec4<f32> {
    let view = safe_normalize(position);
    let es = -view.z;
    let e0 = max(-water.parameters.z, 0.0);
    var intersection = vec3<f32>(0.0);
    if water.parameters.z > 0.0 && abs(es) > 0.000001 {
        intersection = view * (water.parameters.z / es);
    }
    let length_in_water = max(length(position - intersection), 0.1);
    let density = water.parameters.x;
    let modifier = water.parameters.y;
    let t1 = -density * pow(0.98, modifier * e0);
    let t2 = density + modifier * es;
    let t3 = pow(0.98, t2 * length_in_water) - 1.0;
    var fog_amount = -density * pow(0.98, modifier * e0) * log(0.98) * length_in_water;
    if abs(t2) > 0.000001 {
        fog_amount = t1 / t2 * t3;
    }
    let fog_factor = pow(clamp(min(fog_amount, 1.0), 0.0, 1.0), 1.0 / 1.7);
    let transmission = pow(0.98, length_in_water * density);
    return vec4<f32>(srgb_to_linear(water.color.rgb) * fog_factor, transmission);
}

fn apply_water_fog(position: vec3<f32>, color: vec4<f32>, water: WaterFog, clip_to_surface: bool) -> vec4<f32> {
    if clip_to_surface && position.z + water.parameters.z > 0.0 {
        return color;
    }
    let fog = water_fog_view(position, water);
    return vec4<f32>(color.rgb * fog.a + fog.rgb, color.a);
}

fn apply_water_haze(position: vec3<f32>, color: vec4<f32>, water: WaterFog) -> vec4<f32> {
    if position.z + water.parameters.z > 0.0 {
        return color;
    }
    let fog = water_fog_view(position, water);
    return vec4<f32>(fog.rgb + color.rgb * pow(fog.a, 1.7), color.a);
}

fn pbr_punctual(
    diffuse_color: vec3<f32>,
    specular_color: vec3<f32>,
    perceptual_roughness: f32,
    normal: vec3<f32>,
    view: vec3<f32>,
    light: vec3<f32>,
) -> PunctualTerms {
    let roughness = max(perceptual_roughness, 8.0 / 255.0);
    let alpha_roughness = roughness * roughness;
    let reflectance = max(max(specular_color.r, specular_color.g), specular_color.b);
    let reflectance90 = clamp(reflectance * 25.0, 0.0, 1.0);
    let half_vector = safe_normalize(light + view);
    let n_dot_l = clamp(dot(normal, light), 0.001, 1.0);
    let n_dot_v = clamp(abs(dot(normal, view)), 0.001, 1.0);
    let n_dot_h = clamp(dot(normal, half_vector), 0.0, 1.0);
    let v_dot_h = clamp(dot(view, half_vector), 0.0, 1.0);
    let fresnel = specular_color + (vec3<f32>(reflectance90) - specular_color) * pow(clamp(1.0 - v_dot_h, 0.0, 1.0), 5.0);
    let roughness_squared = alpha_roughness * alpha_roughness;
    let distribution_factor = (n_dot_h * roughness_squared - n_dot_h) * n_dot_h + 1.0;
    let distribution = roughness_squared / (3.14159265 * distribution_factor * distribution_factor);
    let attenuation_l = 2.0 * n_dot_l / (n_dot_l + sqrt(alpha_roughness * alpha_roughness + (1.0 - alpha_roughness * alpha_roughness) * n_dot_l * n_dot_l));
    let attenuation_v = 2.0 * n_dot_v / (n_dot_v + sqrt(alpha_roughness * alpha_roughness + (1.0 - alpha_roughness * alpha_roughness) * n_dot_v * n_dot_v));
    let geometry = attenuation_l * attenuation_v;
    var result: PunctualTerms;
    result.nl = n_dot_l;
    result.diffuse = (vec3<f32>(1.0) - fresnel) * diffuse_color / 3.14159265;
    result.specular = fresnel * geometry * distribution / (4.0 * n_dot_l * n_dot_v);
    return result;
}

fn water_radiance_from_sky_without_probes(direction: vec3<f32>, atmosphere: Atmosphere) -> vec3<f32> {
    let sky_direction = safe_normalize(vec3<f32>(direction.xy, abs(direction.z)));
    return sky_linear_color(sky_direction, atmosphere);
}

fn transform_water_normal(normal: vec3<f32>, tangent: vec3<f32>, bitangent: vec3<f32>, sample: vec3<f32>) -> vec3<f32> {
    return safe_normalize(sample.x * tangent + sample.y * bitangent + sample.z * normal);
}

fn shade_water(
    position: vec3<f32>,
    position_eye: vec3<f32>,
    light_direction_eye: vec3<f32>,
    light_norm_eye: vec3<f32>,
    normal_eye: vec3<f32>,
    tangent_eye: vec3<f32>,
    world_to_eye: mat4x4<f32>,
    wave1_sample: vec3<f32>,
    wave2_sample: vec3<f32>,
    wave3_sample: vec3<f32>,
    water: WaterSurface,
    atmosphere: Atmosphere,
) -> vec4<f32> {
    if water.fog.parameters.w > 0.5 {
        let underwater_surface = vec4<f32>(srgb_to_linear(water.fog.color.rgb), 0.0);
        let fogged = max(apply_water_fog(position, underwater_surface, water.fog, false), vec4<f32>(0.0));
        return fogged;
    }
    let wave1 = wave1_sample;
    let wave2 = wave2_sample;
    let wave3 = wave3_sample;
    let view_direction = safe_normalize(position);
    let wavef = (wave1 + wave2 * 0.4 + wave3 * 0.6) * 0.5;
    let fresnel = calculate_fresnel(view_direction, wave1, wave2, wave3, wavef, water);
    let view_eye = safe_normalize(position_eye);
    let bitangent_eye = cross(normal_eye, tangent_eye);
    let view_dot_up = clamp(-dot(view_eye, normal_eye) * 2.0, 0.0, 1.0);
    var wave_ibl = wavef * water.normal_scale.xyz;
    wave_ibl.z *= 2.0;
    wave_ibl = transform_water_normal(normal_eye, tangent_eye, bitangent_eye, safe_normalize(wave_ibl));
    let reflected_eye = reflect(view_eye, wave_ibl);
    let view_rotation = mat3x3<f32>(world_to_eye[0].xyz, world_to_eye[1].xyz, world_to_eye[2].xyz);
    let reflected_direction = safe_normalize(transpose(view_rotation) * reflected_eye);
    let distance = length(position);
    let fog_sample = position + view_direction * 2048.0;
    let fb = apply_water_fog(fog_sample, vec4<f32>(1.0), water.fog, true);
    let wavef_eye = transform_water_normal(normal_eye, tangent_eye, bitangent_eye, safe_normalize(wavef));
    let punctual_normal = safe_normalize(wavef_eye + normal_eye * (max(distance, 32.0) / 32.0 * (1.0 - view_dot_up)));
    let punctual = pbr_punctual(
        vec3<f32>(0.0),
        srgb_to_linear(atmosphere.water_specular.xyz),
        water.normal_scale.w,
        punctual_normal,
        -view_eye,
        safe_normalize(light_direction_eye),
    );
    let atmosphere_terms = object_atmosphere_terms(position_eye, light_direction_eye, light_norm_eye, atmosphere);
    let punctual_color = clamp(punctual.nl * (punctual.diffuse + punctual.specular), vec3<f32>(0.0), vec3<f32>(10.0))
        * atmosphere_terms.sunlight * atmosphere_terms.attenuation;
    let radiance = water_radiance_from_sky_without_probes(reflected_direction, atmosphere) * fresnel.y;
    let base_color = mix(fb.rgb, radiance, min(1.0, fresnel.x));
    let atmospheric_color = base_color * atmosphere_terms.attenuation.x
        + srgb_to_linear(atmosphere_terms.additive * 2.0) * atmosphere.distance.z;
    let color = atmospheric_color + punctual_color;
    let output = min(vec4<f32>(1.0), max(vec4<f32>(color, 0.0), vec4<f32>(0.0)));
    return output;
}
