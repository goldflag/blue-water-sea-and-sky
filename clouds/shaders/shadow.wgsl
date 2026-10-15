// Cloud shadows for ships, islands and the sea (`SkyApi.cloudShadow`, `CloudLayer.shadow` in
// the web game's `sky/clouds/CloudLayer.ts`): the celestial light's transmittance through the clouds at a world position
// (the sun's, or the moon's once it outshines the sun), 1 in full light and `CLOUD_SHADOW_FLOOR` (0: none of the
// beam) under the thickest deck. Multiply it into the direct celestial light only: the light under a cloud is the
// sky's, which the environment gives.
// Every function takes the map, its sampler and the uniforms as arguments, so a consumer binds them in any group:
//
//     #import naval_clouds::shadow::{CloudShadow, cloud_shadow}
//     @group(3) @binding(4) var shadow_map: texture_2d<f32>;
//     @group(3) @binding(5) var shadow_sampler: sampler;       // linear, repeat
//     @group(3) @binding(6) var<uniform> shadow: CloudShadow;
//     let lit = cloud_shadow(shadow_map, shadow_sampler, shadow, world_position);
//
// It reads level 0 explicitly, so it works in any stage.
#define_import_path naval_clouds::shadow

// `naval_clouds::CloudShadowUniforms` on the Rust side, 64 bytes.
struct CloudShadow {
    // Toward the body whose light the map stops (the sun, or the moon once it outshines the sun), and how strongly
    // clouds shade (0 with that body on the horizon, 1 from about 7° up).
    sun_direction: vec3<f32>,
    strength: f32,
    // The clouds' drift (m) and the camera's position (m), world XZ.
    wind_offset: vec2<f32>,
    camera: vec2<f32>,
    // Texels per edge of the map, and 1 while clouds are drawn.
    size: f32,
    enabled: f32,
    padding: vec2<f32>,
    // The port's berth clearing (`PORT_CLEARING` and `clearBerth` in the web game's `VisualEnvironment.ts`): its centre (m,
    // world XZ), no cloud shade within `z` metres of it, the shadow back in full by `w`; `w` 0 clears nothing.
    clearing: vec4<f32>,
}

const EXTENT: f32 = 40000.0;
const EDGE: f32 = 0.15;
// `CLOUD_SHADOW_FLOOR`: the beam a deck lets through where the sun's transmittance is 0 (none).
const FLOOR: f32 = 0.0;

// Down the sun's rays to the sea, then into the clouds' drifting frame the map is laid out in.
fn shadow_uv(params: CloudShadow, position: vec3<f32>) -> vec3<f32> {
    let sun = params.sun_direction;
    let ground = position.xz - sun.xz * (position.y / max(sun.y, 0.02));
    let edge = max(abs(ground.x - params.camera.x), abs(ground.y - params.camera.y)) / (EXTENT / 2.0);
    return vec3<f32>((ground - params.wind_offset) / EXTENT, edge);
}

fn shade(params: CloudShadow, value: f32, edge: f32) -> f32 {
    let t = clamp((edge - 1.0) / (1.0 - EDGE - 1.0), 0.0, 1.0);
    let fade = t * t * (3.0 - 2.0 * t);
    return mix(1.0, mix(FLOOR, 1.0, value), params.strength * fade * params.enabled);
}

// `clearBerth`: no cloud shades the berth within the clearing's reach, the shadow returning by its fade.
fn clear_berth(params: CloudShadow, position: vec3<f32>, shadow: f32) -> f32 {
    if params.clearing.w <= 0.0 {
        return shadow;
    }
    let clear = 1.0 - smoothstep(params.clearing.z, params.clearing.w, distance(position.xz, params.clearing.xy));
    return mix(shadow, 1.0, clear);
}

// The shadow through a cubic B-spline (four bilinear reads, Sigg & Hadwiger 2005): the map's texels are 40 m, and a
// bilinear read would draw their grid into a shadow's edge.
fn cloud_shadow(map: texture_2d<f32>, map_sampler: sampler, params: CloudShadow, position: vec3<f32>) -> f32 {
    let at = shadow_uv(params, position);
    let size = params.size;
    let coord = at.xy * size - 0.5;
    let base = floor(coord);
    let f = coord - base;
    let f2 = f * f;
    let f3 = f2 * f;
    let one = 1.0 - f;
    let w0 = one * one * one / 6.0;
    let w1 = (f3 * 3.0 - f2 * 6.0 + 4.0) / 6.0;
    let w2 = (f3 * -3.0 + f2 * 3.0 + f * 3.0 + 1.0) / 6.0;
    let w3 = f3 / 6.0;
    let g0 = w0 + w1;
    let g1 = w2 + w3;
    let p0 = (base - 0.5 + w1 / g0) / size;
    let p1 = (base + 1.5 + w3 / g1) / size;
    let a = textureSampleLevel(map, map_sampler, vec2<f32>(p1.x, p1.y), 0.0).r;
    let b = textureSampleLevel(map, map_sampler, vec2<f32>(p0.x, p1.y), 0.0).r;
    let c = textureSampleLevel(map, map_sampler, vec2<f32>(p1.x, p0.y), 0.0).r;
    let d = textureSampleLevel(map, map_sampler, vec2<f32>(p0.x, p0.y), 0.0).r;
    return clear_berth(params, position, shade(params, mix(mix(a, b, g0.x), mix(c, d, g0.x), g0.y), at.z));
}

// One bilinear read, for passes where four are too many.
fn cloud_shadow_fast(map: texture_2d<f32>, map_sampler: sampler, params: CloudShadow, position: vec3<f32>) -> f32 {
    let at = shadow_uv(params, position);
    return clear_berth(params, position, shade(params, textureSampleLevel(map, map_sampler, at.xy, 0.0).r, at.z));
}
