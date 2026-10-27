struct StarUniform {
    projection: mat4x4<f32>,
    viewport_alpha_blend: vec4<f32>,
    time: vec4<f32>,
}

struct Star {
    direction_intensity: vec4<f32>,
    color_size: vec4<f32>,
}

struct StarVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec3<f32>,
    @location(2) alpha: f32,
    @location(3) screen_position: vec2<f32>,
}

@group(0) @binding(0) var<uniform> state: StarUniform;
@group(1) @binding(0) var<storage, read> star_field: array<Star>;
@group(2) @binding(0) var bloom_image: texture_2d<f32>;
@group(2) @binding(1) var bloom_next_image: texture_2d<f32>;
@group(2) @binding(2) var bloom_sampler: sampler;

fn linear_to_srgb(color: vec3<f32>) -> vec3<f32> {
    let low = color * 12.92;
    let high = 1.055 * pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(high, low, color <= vec3<f32>(0.0031308));
}

@vertex fn vertex(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) star_index: u32) -> StarVertex {
    let star = star_field[star_index];
    let corners = array(
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 0.0),
    );
    let corner = corners[vertex_index];
    let direction = star.direction_intensity.xyz;
    let angle = state.time.y;
    let rotated = vec3<f32>(
        direction.x * cos(angle) - direction.y * sin(angle),
        direction.x * sin(angle) + direction.y * cos(angle),
        direction.z,
    );
    var left = cross(rotated, vec3<f32>(0.0, 0.0, 1.0));
    if dot(left, left) > 0.000001 {
        left = normalize(left);
    }
    var up = cross(rotated, left);
    if dot(up, up) > 0.000001 {
        up = normalize(up);
    }
    let position = rotated * 15000.0 + left * ((1.0 - corner.x) * star.color_size.w) + up * (corner.y * star.color_size.w);
    let clip = state.projection * vec4<f32>(position, 1.0);
    var output: StarVertex;
    output.position = vec4<f32>(clip.xy, 0.0, clip.w);
    output.uv = corner;
    output.color = star.color_size.xyz;
    output.alpha = state.viewport_alpha_blend.z * star.direction_intensity.w;
    output.screen_position = position.xy * state.time.x;
    return output;
}

@fragment fn fragment(input: StarVertex) -> @location(0) vec4<f32> {
    let sample_current = textureSample(bloom_image, bloom_sampler, input.uv);
    let sample_next = textureSample(bloom_next_image, bloom_sampler, input.uv);
    let current = vec4<f32>(linear_to_srgb(sample_current.rgb), sample_current.a);
    let next = vec4<f32>(linear_to_srgb(sample_next.rgb), sample_next.a);
    let bloom = mix(next, current, state.viewport_alpha_blend.w);
    let twinkle = abs(fract(input.screen_position.x + input.screen_position.y));
    let alpha = smoothstep(0.0, 0.9, input.alpha) * 32.0 * twinkle * bloom.a;
    return vec4<f32>(bloom.rgb * input.color, alpha);
}
