struct ExposureSettings {
    values: vec4<f32>,
    timing: vec4<f32>,
    fog: WaterFog,
    inverse_view_projection: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> settings: ExposureSettings;
@group(1) @binding(0) var scene_image: texture_2d<f32>;
@group(1) @binding(1) var previous_exposure: texture_2d<f32>;
@group(1) @binding(2) var depth_image: texture_depth_2d;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
}

@vertex fn vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    let positions = array(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var output: VertexOutput;
    output.position = vec4<f32>(positions[index], 0.0, 1.0);
    return output;
}

fn scene_color(coordinate: vec2<i32>, size: vec2<u32>) -> vec3<f32> {
    var color = textureLoad(scene_image, coordinate, 0).rgb;
    let depth = textureLoad(depth_image, coordinate, 0);
    if depth == 0.0 {
        color = srgb_to_linear(color) * settings.timing.w;
    }
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
    return apply_water_haze(position, vec4<f32>(color, 1.0), settings.fog).rgb;
}

@fragment fn fragment() -> @location(0) vec4<f32> {
    let size = textureDimensions(scene_image, 0);
    let max_coordinate = vec2<i32>(size) - vec2<i32>(1);
    var luminance = 0.0;
    for (var y = 0u; y < 16u; y += 1u) {
        for (var x = 0u; x < 16u; x += 1u) {
            let uv = (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) / 16.0;
            let coordinate = clamp(vec2<i32>(uv * vec2<f32>(size)), vec2<i32>(0), max_coordinate);
            let color = scene_color(coordinate, size);
            luminance += max(dot(color, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0);
        }
    }
    luminance = clamp(luminance / 256.0, 0.0, settings.values.x);
    luminance = pow(luminance / settings.values.x, 2.0);
    let desired_exposure = mix(settings.values.z, settings.values.y, luminance);
    var exposure = desired_exposure;
    if settings.timing.z > 0.5 {
        let previous = textureLoad(previous_exposure, vec2<i32>(0), 0).r;
        let speed = -log(settings.values.w) / settings.timing.y;
        exposure = mix(previous, desired_exposure, 1.0 - exp(-speed * settings.timing.x));
    }
    return vec4<f32>(max(vec3<f32>(exposure), vec3<f32>(0.0)), 1.0);
}
