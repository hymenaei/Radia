struct ExposureSettings {
    values: vec4<f32>,
    timing: vec4<f32>,
    fog: WaterFog,
    inverse_view_projection: mat4x4<f32>,
    view: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> settings: ExposureSettings;
@group(1) @binding(0) var scene_image: texture_2d<f32>;
@group(1) @binding(1) var previous_exposure: texture_2d<f32>;
@group(1) @binding(2) var depth_image: texture_depth_2d;
@group(1) @binding(3) var scene_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
}

@vertex fn vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    let positions = array(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var output: VertexOutput;
    output.position = vec4<f32>(positions[index], 0.0, 1.0);
    return output;
}

fn scene_color(color_uv: vec2<f32>, coordinate: vec2<i32>, size: vec2<u32>) -> vec3<f32> {
    let scene = textureSample(scene_image, scene_sampler, color_uv);
    let nearest_scene = textureLoad(scene_image, coordinate, 0);
    var color = scene.rgb;
    let depth = textureLoad(depth_image, coordinate, 0);
    let is_water_surface = nearest_scene.a < 0.5;
    if depth == 0.0 {
        if settings.fog.color.w > 0.5 {
            let clip = vec4<f32>(color_uv.x * 2.0 - 1.0, 1.0 - color_uv.y * 2.0, 1.0, 1.0);
            let projected = settings.inverse_view_projection * clip;
            let direction_eye = (settings.view * vec4<f32>(projected.xyz / projected.w, 0.0)).xyz;
            let plane_normal_eye = (settings.view * vec4<f32>(0.0, 0.0, 1.0, 0.0)).xyz;
            return apply_water_background(direction_eye, plane_normal_eye, color, settings.fog);
        }
        return color;
    }
    if is_water_surface {
        return color;
    }
    color *= settings.timing.w;
    if settings.fog.color.w < 0.5 {
        return color;
    }
    let uv = (vec2<f32>(coordinate) + vec2<f32>(0.5)) / vec2<f32>(size);
    let clip = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0);
    let projected = settings.inverse_view_projection * clip;
    let position = projected.xyz / projected.w;
    if abs(position.z + settings.fog.parameters.z) < 0.01 {
        return color;
    }
    let position_eye = (settings.view * vec4<f32>(position, 1.0)).xyz;
    let plane_normal_eye = (settings.view * vec4<f32>(0.0, 0.0, 1.0, 0.0)).xyz;
    return apply_water_haze(position_eye, plane_normal_eye, vec4<f32>(color, 1.0), settings.fog).rgb;
}

@fragment fn luminance_fragment(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let size = textureDimensions(scene_image, 0);
    let sample_uv = pixel.xy / 256.0;
    let uv = vec2<f32>(sample_uv.x * 0.6 + 0.2, sample_uv.y * 0.6 + 0.3);
    let coordinate = min(vec2<u32>(uv * vec2<f32>(size)), size - vec2<u32>(1));
    let color = scene_color(uv, vec2<i32>(coordinate), size);
    let luminance = max(dot(color, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0);
    return vec4<f32>(luminance, 0.0, 0.0, 1.0);
}

@fragment fn fragment() -> @location(0) vec4<f32> {
    let luminance_sample = textureLoad(scene_image, vec2<i32>(0), 0).r;
    let luminance = pow(clamp(luminance_sample / settings.values.x, 0.0, 1.0), 2.0);
    let desired_exposure = mix(settings.values.z, settings.values.y, luminance);
    var exposure = desired_exposure;
    if settings.timing.z > 0.5 {
        let previous = textureLoad(previous_exposure, vec2<i32>(0), 0).r;
        let speed = -log(settings.values.w) / settings.timing.y;
        exposure = mix(previous, desired_exposure, 1.0 - exp(-speed * settings.timing.x));
    }
    return vec4<f32>(max(vec3<f32>(exposure), vec3<f32>(0.0)), 1.0);
}
