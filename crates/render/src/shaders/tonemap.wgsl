struct ToneSettings {
    values: vec4<f32>,
    fog: WaterFog,
    inverse_view_projection: mat4x4<f32>,
    view: mat4x4<f32>,
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

fn rrt_and_odt_fit(color: vec3<f32>) -> vec3<f32> {
    let a = color * (color + vec3<f32>(0.0245786)) - vec3<f32>(0.000090537);
    let b = color * (color * 0.983729 + vec3<f32>(0.4329510)) + vec3<f32>(0.238081);
    return a / b;
}

fn aces_hill(input: vec3<f32>) -> vec3<f32> {
    let input_matrix = mat3x3<f32>(
        vec3<f32>(0.59719, 0.07600, 0.02840),
        vec3<f32>(0.35458, 0.90834, 0.13383),
        vec3<f32>(0.04823, 0.01566, 0.83777),
    );
    let output_matrix = mat3x3<f32>(
        vec3<f32>(1.60475, -0.10208, -0.00327),
        vec3<f32>(-0.53108, 1.10813, -0.07276),
        vec3<f32>(-0.07367, -0.00605, 1.07602),
    );
    return clamp(output_matrix * rrt_and_odt_fit(input_matrix * input), vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment fn fragment(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let coordinate = vec2<i32>(pixel.xy);
    let size = textureDimensions(scene_image, 0);
    let scene = textureLoad(scene_image, coordinate, 0);
    var color = scene.rgb;
    let depth = textureLoad(depth_image, coordinate, 0);
    let is_water_surface = scene.a < 0.5;
    if settings.fog.color.w > 0.5 && !is_water_surface {
        let uv = pixel.xy / vec2<f32>(size);
        let clip = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, select(depth, 1.0, depth == 0.0), 1.0);
        let projected = settings.inverse_view_projection * clip;
        let position = projected.xyz / projected.w;
        if depth == 0.0 {
            let direction_eye = (settings.view * vec4<f32>(position, 0.0)).xyz;
            let plane_normal_eye = (settings.view * vec4<f32>(0.0, 0.0, 1.0, 0.0)).xyz;
            color = apply_water_background(direction_eye, plane_normal_eye, color, settings.fog);
        } else if abs(position.z + settings.fog.parameters.z) >= 0.01 {
            let position_eye = (settings.view * vec4<f32>(position, 1.0)).xyz;
            let plane_normal_eye = (settings.view * vec4<f32>(0.0, 0.0, 1.0, 0.0)).xyz;
            color = apply_water_haze(position_eye, plane_normal_eye, vec4<f32>(color, 1.0), settings.fog).rgb;
        }
    }
    let exposure = textureLoad(exposure_image, vec2<i32>(0), 0).r;
    let exposed = color * settings.values.x * exposure;
    let mapped = aces_hill(exposed);
    return vec4<f32>(clamp(mix(exposed, mapped, settings.values.y), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
