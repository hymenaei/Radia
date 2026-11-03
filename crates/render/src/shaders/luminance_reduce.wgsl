@group(0) @binding(0) var source_image: texture_2d<f32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
}

@vertex fn vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    let positions = array(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var output: VertexOutput;
    output.position = vec4<f32>(positions[index], 0.0, 1.0);
    return output;
}

@fragment fn fragment(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let origin = vec2<i32>(pixel.xy) * 2;
    let top_left = textureLoad(source_image, origin, 0).r;
    let top_right = textureLoad(source_image, origin + vec2<i32>(1, 0), 0).r;
    let bottom_left = textureLoad(source_image, origin + vec2<i32>(0, 1), 0).r;
    let bottom_right = textureLoad(source_image, origin + vec2<i32>(1, 1), 0).r;
    return vec4<f32>((top_left + top_right + bottom_left + bottom_right) * 0.25, 0.0, 0.0, 1.0);
}
