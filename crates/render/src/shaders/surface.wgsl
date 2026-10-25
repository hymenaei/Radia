struct Camera {
    projection: mat4x4<f32>,
    view: mat4x4<f32>,
    light: vec4<f32>,
}
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;
struct SurfaceAtmosphere {
    atmosphere: Atmosphere,
    distance: vec4<f32>,
}
@group(2) @binding(0) var<uniform> surface_atmosphere: SurfaceAtmosphere;
@group(3) @binding(0) var<uniform> surface_water: WaterSurface;
@group(4) @binding(0) var water_normal_image: texture_2d<f32>;
@group(4) @binding(1) var water_normal_sampler: sampler;
@group(5) @binding(0) var water_normal_next_image: texture_2d<f32>;
@group(5) @binding(1) var water_normal_next_sampler: sampler;

struct Input {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) x: vec4<f32>,
    @location(4) y: vec4<f32>,
    @location(5) z: vec4<f32>,
    @location(6) translation: vec4<f32>,
    @location(7) nx: vec4<f32>,
    @location(8) ny: vec4<f32>,
    @location(9) nz: vec4<f32>,
    @location(10) color: vec4<f32>,
    @location(11) u: vec4<f32>,
    @location(12) v: vec4<f32>,
    @location(13) params: vec4<f32>,
    @location(14) scale: vec4<f32>,
}
struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) position_view: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) relative_position: vec3<f32>,
    @location(4) uv: vec2<f32>,
    @location(5) @interpolate(flat) params: vec4<f32>,
}
fn transform(v: Input) -> Output {
    var out: Output;
    let model = mat4x4<f32>(v.x, v.y, v.z, v.translation);
    let position = model * vec4<f32>(v.position, 1.0);
    out.position = camera.projection * position;
    let normal = mat3x3<f32>(v.nx.xyz, v.ny.xyz, v.nz.xyz) * v.normal;
    out.normal = (camera.view * vec4<f32>(normal, 0.0)).xyz;
    out.position_view = (camera.view * position).xyz;
    out.relative_position = position.xyz;
    out.color = v.color;
    var uv = v.uv;
    if v.params.w == 1.0 {
        var binormal = vec3<f32>(1.0, 0.0, 0.0);
        if abs(v.normal.x) >= 0.5 {
            binormal = vec3<f32>(0.0, select(1.0, -1.0, v.normal.x < 0.0), 0.0);
        } else if v.normal.y > 0.0 {
            binormal = vec3<f32>(-1.0, 0.0, 0.0);
        }
        let tangent = cross(binormal, v.normal);
        let point = v.position * v.scale.xyz;
        uv = vec2<f32>(0.5 + 2.0 * dot(binormal, point), 0.5 - 2.0 * dot(tangent, point));
    }
    out.uv = vec2<f32>(dot(v.u.xyz, vec3<f32>(uv, 1.0)), dot(v.v.xyz, vec3<f32>(uv, 1.0)));
    out.params = v.params;
    return out;
}

@vertex fn vertex(v: Input) -> Output {
    return transform(v);
}

struct Water {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

struct Surface {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@fragment fn water_fragment(v: Output) -> Water {
    let clip = camera.projection * vec4<f32>(v.relative_position, 1.0);
    let direction = safe_normalize(v.relative_position);
    var color: vec3<f32>;
    if v.relative_position.z > 0.0 {
        let horizon = horizon_color(vec3<f32>(direction.xy, 0.0), surface_atmosphere.atmosphere);
        var normal = vec3<f32>(0.0, 0.0, 1.0);
        if surface_water.params.w > 0.5 {
            let uv = v.relative_position.xy * surface_water.normal.x * 0.015
                + surface_water.scroll.xy * 0.03
                + surface_water.scroll.zw * -0.02;
            let sampled = textureSample(water_normal_image, water_normal_sampler, uv).xyz * 2.0 - vec3<f32>(1.0);
            normal = safe_normalize(mix(normal, safe_normalize(sampled), 0.12));
        }
        let cosine = max(dot(normal, safe_normalize(-direction)), 0.0);
        let fresnel = clamp(surface_water.params.x + surface_water.params.y * pow(1.0 - cosine, 5.0), 0.0, 1.0);
        color = aerial_perspective(
            mix(surface_water.fog_color.xyz, horizon, fresnel),
            direction,
            length(v.position_view),
            surface_atmosphere.distance.x,
            surface_water.params.z,
            surface_atmosphere.atmosphere,
        );
    } else {
        let coordinates = water_coordinates(v.relative_position, surface_water);
        let normal = blended_water_normal(
            textureSample(water_normal_image, water_normal_sampler, coordinates.first).xyz,
            textureSample(water_normal_next_image, water_normal_next_sampler, coordinates.first).xyz,
            textureSample(water_normal_image, water_normal_sampler, coordinates.second).xyz,
            textureSample(water_normal_next_image, water_normal_next_sampler, coordinates.second).xyz,
            surface_water,
        );
        color = water_surface_color(
            direction,
            normal,
            length(v.position_view),
            surface_atmosphere.distance.x,
            surface_water,
            surface_atmosphere.atmosphere,
        );
    }
    var water: Water;
    water.color = vec4<f32>(color, 1.0);
    water.depth = max(clamp(clip.z / clip.w, 0.0, 1.0), 0.00000001);
    return water;
}
fn studio(v: Output, color: vec3<f32>, front: bool) -> vec3<f32> {
    let eye = normalize(-v.position_view);
    var normal = normalize(v.normal);
    if !front {
        normal = -normal;
    }
    let key = max(dot(normal, camera.light.xyz), 0.0);
    let halfway = normalize(camera.light.xyz + eye);
    let specular = pow(max(dot(normal, halfway), 0.0), 48.0) * 0.25;
    return color * (0.15 + 0.85 * key) + vec3<f32>(specular);
}

@fragment fn fragment(v: Output, @builtin(front_facing) front: bool) -> Surface {
    let color = textureSample(image, image_sampler, v.uv) * v.color;
    if v.params.y == 1.0 && color.a < v.params.z { discard; }
    let alpha = select(1.0, clamp(color.a, 0.0, 1.0), v.params.y == 2.0);
    let surface = mix(studio(v, color.rgb, front), color.rgb, v.params.x);
    let direction = safe_normalize(v.relative_position);
    let fogged = aerial_perspective(
        surface,
        direction,
        length(v.position_view),
        surface_atmosphere.distance.x,
        surface_atmosphere.atmosphere.haze.y,
        surface_atmosphere.atmosphere,
    );
    var output: Surface;
    output.color = vec4<f32>(fogged, alpha);
    output.depth = max(v.position.z, 0.00000001);
    return output;
}
