@group(0) @binding(0) var source: texture_2d<f32>;

@vertex fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

@fragment fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = textureDimensions(source);
    let next = max(size / 2u, vec2<u32>(1u));
    let scale = vec2<f32>(size) / vec2<f32>(next);
    let lo = floor(position.xy) * scale;
    let hi = lo + scale;
    var sum = vec4<f32>(0.0);
    for (var y = u32(floor(lo.y)); y < min(u32(ceil(hi.y)), size.y); y++) {
        for (var x = u32(floor(lo.x)); x < min(u32(ceil(hi.x)), size.x); x++) {
            let weight = max(min(hi, vec2<f32>(f32(x), f32(y)) + 1.0) - max(lo, vec2<f32>(f32(x), f32(y))), vec2<f32>(0.0));
            sum += textureLoad(source, vec2<i32>(i32(x), i32(y)), 0) * weight.x * weight.y;
        }
    }
    return sum / (scale.x * scale.y);
}
