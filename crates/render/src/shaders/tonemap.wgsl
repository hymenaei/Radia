struct ToneSettings {
    values: vec4<f32>,
    fog: WaterFog,
    inverse_view_projection: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> settings: ToneSettings;
@group(1) @binding(0) var scene_image: texture_2d<f32>;
@group(1) @binding(1) var exposure_image: texture_2d<f32>;
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

fn pbr_neutral_tonemap(input: vec3<f32>) -> vec3<f32> {
    let start_compression = 0.76;
    let desaturation = 0.15;
    let minimum = min(input.r, min(input.g, input.b));
    let offset = select(0.04, minimum - 6.25 * minimum * minimum, minimum < 0.08);
    var color = input - vec3<f32>(offset);
    let peak = max(color.r, max(color.g, color.b));
    if peak < start_compression {
        return color;
    }
    let distance = 1.0 - start_compression;
    let new_peak = 1.0 - distance * distance / (peak + distance - start_compression);
    color *= new_peak / peak;
    let gray = 1.0 - 1.0 / (desaturation * (peak - new_peak) + 1.0);
    return mix(color, vec3<f32>(new_peak), gray);
}

@fragment fn fragment(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let coordinate = vec2<i32>(pixel.xy);
    let size = textureDimensions(scene_image, 0);
    let scene = textureLoad(scene_image, coordinate, 0);
    var color = scene.rgb;
    let depth = textureLoad(depth_image, coordinate, 0);
    let is_water_surface = scene.a < 0.5;
    if depth == 0.0 {
        color = srgb_to_linear(color) * settings.values.z;
    }
    if settings.fog.color.w > 0.5 && !is_water_surface {
        let uv = (pixel.xy + vec2<f32>(0.5)) / vec2<f32>(size);
        let clip = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0);
        let projected = settings.inverse_view_projection * clip;
        let position = projected.xyz / projected.w;
        if abs(position.z + settings.fog.parameters.z) >= 0.01 {
            color = apply_water_haze(position, vec4<f32>(color, 1.0), settings.fog).rgb;
        }
    }
    let exposure = textureLoad(exposure_image, vec2<i32>(0), 0).r;
    let exposed = color * settings.values.x * exposure;
    let mapped = pbr_neutral_tonemap(exposed);
    return vec4<f32>(clamp(mix(exposed, mapped, settings.values.y), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
