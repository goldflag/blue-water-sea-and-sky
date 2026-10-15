// The underwater view: ports the web game's `ocean/screen/underwater.ts` (`createUnderwaterPass`), composed over the scene by two
// blended full-screen draws in the transparent phase (`underwater_dim`: the scene times the water's transmittance;
// `underwater_light`: the light the water scatters toward the camera and the film of water where the lens meets the
// surface). The depth below the surface of each pixel's near-plane point comes from the probe (`underwater_probe.wgsl`).

#import naval::camera::camera
#import naval_ocean::underwater_ray::{screen_ray, water_path_light}
#import naval_ocean::water_properties::{WaterField, sample_water, water_beam, water_pigment}

struct Underwater {
    // rgb the water's pigment (scaled for night by the scene), its absorption with, in w, how many metres of it each
    // metre under the camera counts as (`naval_ocean::underwater::UNDERWATER_CLARITY`), the transmission tint; the
    // sun's direction and its colour (brightest channel 1).
    pigment: vec4<f32>,
    absorption: vec4<f32>,
    transmission: vec4<f32>,
    sun_direction: vec4<f32>,
    sun_color: vec4<f32>,
    water: WaterField,
}

@group(1) @binding(0) var<uniform> water: Underwater;
// The scene's depth with the sea's surface in it (the backdrop copy), and the probe above, read bilinearly.
@group(1) @binding(1) var scene_depth: texture_depth_2d;
@group(1) @binding(2) var depth_probe: texture_2d<f32>;
@group(1) @binding(3) var probe_sampler: sampler;

// Waterline width in pixels, and how much the film of water on the lens takes the transmission tint.
const MENISCUS_WIDTH: f32 = 1.6;
const MENISCUS_TINT: f32 = 0.85;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// One triangle over the screen at the near plane (reversed depth 1), so the scene's depth test passes everywhere.
@vertex
fn vertex(@builtin(vertex_index) index: u32) -> VertexOut {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VertexOut;
    out.clip = vec4<f32>(corner * 2.0 - 1.0, 1.0, 1.0);
    out.uv = vec2<f32>(corner.x, 1.0 - corner.y);
    return out;
}

fn view_z(depth: f32) -> f32 {
    let near = camera.near;
    let far = camera.far;
    return -near * far / (depth * (far - near) + near);
}

// What the water does to this pixel: x (rgb) multiplies the scene, y (rgb) adds to it. Above the water both leave it be,
// but for the waterline's film.
struct Water {
    dim: vec3<f32>,
    light: vec3<f32>,
}

fn water_at(position: vec4<f32>, uv: vec2<f32>) -> Water {
    let ray = screen_ray(uv, camera.inverse_projection, camera.inverse_view, camera.near);
    // Metres below the surface where this pixel's ray starts; negative above it.
    let depth = textureSampleLevel(depth_probe, probe_sampler, uv, 0.0).r;
    // Derivatives before the per-pixel branch: the waterline keeps a constant width in pixels.
    let waterline = 1.0 - smoothstep(MENISCUS_WIDTH * 0.5, MENISCUS_WIDTH, abs(depth) / max(fwidth(depth), 1e-6));
    var dim = vec3<f32>(1.0);
    var light = vec3<f32>(0.0);
    if depth > 0.0 {
        // The farthest depth of this pixel and its neighbours: an edge's colour holds some of what lies behind it.
        let pixel = vec2<i32>(position.xy);
        let last = vec2<i32>(textureDimensions(scene_depth)) - 1;
        var farthest = view_z(textureLoad(scene_depth, pixel, 0));
        let offsets = array<vec2<i32>, 4>(vec2<i32>(1, 0), vec2<i32>(-1, 0), vec2<i32>(0, 1), vec2<i32>(0, -1));
        for (var i = 0; i < 4; i++) {
            farthest = min(farthest, view_z(textureLoad(scene_depth, clamp(pixel + offsets[i], vec2<i32>(0), last), 0)));
        }
        let clarity = water.absorption.w;
        let distance = max(farthest / ray.view.z - ray.to_near, 0.0) * clarity;
        // Visible underwater paths are much shorter than geographic blend widths. A midpoint sample follows a
        // nearby area's transition without integrating a region-scale path through already opaque water.
        let point = ray.start.xz + ray.world.xz * min(distance * 0.5, 100.0);
        let optics = sample_water(water.water, point, water.absorption.rgb);
        let absorption = select(water.absorption.rgb, water_beam(optics), water.water.absorption.w > 0.5);
        let pigment = select(water.pigment.rgb, water_pigment(optics, water.absorption.rgb, water.pigment.rgb), water.water.absorption.w > 0.5);
        dim = exp(-absorption * distance);
        light = water_path_light(ray.world, depth * clarity, distance, pigment, absorption, water.sun_direction.xyz, water.sun_color.rgb);
    }
    // The film of water where the lens meets the surface reads against both the sky and the deep.
    let film = waterline * MENISCUS_TINT;
    return Water(dim * (1.0 - film), light * (1.0 - film) + water.transmission.rgb * film);
}

// The scene times the water's transmittance (blended: zero, source).
@fragment
fn underwater_dim(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(water_at(in.clip, in.uv).dim, 1.0);
}

// The light the water scatters toward the camera, and the waterline's film (blended: one, one).
@fragment
fn underwater_light(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(water_at(in.clip, in.uv).light, 0.0);
}
