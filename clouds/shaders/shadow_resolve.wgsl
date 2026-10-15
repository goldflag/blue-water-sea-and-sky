// The Beer shadow maps' temporal pass (`shadowResolve.frag` of @takram/three-clouds): each cascade's history is
// reprojected by the nearest texel's motion, clipped to the variance of this update's march about it (nine texels), and
// moved toward the march very slowly (takram's alpha 0.01: a single flickering texel shows in a shadow map). Without
// history (a cut, a new sun) the march is taken as it is. The cascades the shadow pass did not march this update (it
// marches one an update, round robin) are carried over unchanged.
//
// MIT License, Copyright (c) 2024 Shota Matsuda (https://github.com/takram-design-engineering/three-geospatial). The
// full notice is in `naval-clouds/THIRD_PARTY.md`.

#import naval_clouds::common::{clouds, clamp_sampler}

@group(3) @binding(0) var current_shadow: texture_2d_array<f32>;
@group(3) @binding(1) var current_depth_velocity: texture_2d_array<f32>;
@group(3) @binding(2) var history_shadow: texture_2d_array<f32>;
@group(3) @binding(3) var shadow_out: texture_storage_2d_array<rgba16float, write>;

const VARIANCE_GAMMA: f32 = 1.0;
const TEMPORAL_ALPHA: f32 = 0.01;

fn load(texture: texture_2d_array<f32>, coord: vec2<i32>, cascade: i32) -> vec4<f32> {
    let top = vec2<i32>(i32(clouds.shadow_map.y) - 1);
    return textureLoad(texture, clamp(coord, vec2<i32>(0), top), cascade, 0);
}

fn closest_fragment(coord: vec2<i32>, cascade: i32) -> vec4<f32> {
    var result = vec4<f32>(1e7, 0.0, 0.0, 0.0);
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let neighbor = load(current_depth_velocity, coord + vec2<i32>(x, y), cascade);
            if neighbor.r < result.r {
                result = neighbor;
            }
        }
    }
    return result;
}

fn clip_aabb(current: vec4<f32>, history: vec4<f32>, min_color: vec4<f32>, max_color: vec4<f32>) -> vec4<f32> {
    let p_clip = 0.5 * (max_color.rgb + min_color.rgb);
    let e_clip = 0.5 * (max_color.rgb - min_color.rgb) + 1e-7;
    let v_clip = history - vec4<f32>(p_clip, current.a);
    let v_unit = v_clip.xyz / e_clip;
    let a_unit = abs(v_unit);
    let ma_unit = max(a_unit.x, max(a_unit.y, a_unit.z));
    if ma_unit > 1.0 {
        return vec4<f32>(p_clip, current.a) + v_clip / ma_unit;
    }
    return history;
}

// Variance clipping over the nine texels about `coord`.
fn variance_clipping(coord: vec2<i32>, cascade: i32, current: vec4<f32>, history: vec4<f32>) -> vec4<f32> {
    var moment1 = current;
    var moment2 = current * current;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            if x == 0 && y == 0 {
                continue;
            }
            let neighbor = load(current_shadow, coord + vec2<i32>(x, y), cascade);
            moment1 += neighbor;
            moment2 += neighbor * neighbor;
        }
    }
    let n = 9.0;
    let mean = moment1 / n;
    let variance = sqrt(max(moment2 / n - mean * mean, vec4<f32>(0.0))) * VARIANCE_GAMMA;
    let min_color = mean - variance;
    let max_color = mean + variance;
    return clip_aabb(clamp(mean, min_color, max_color), history, min_color, max_color);
}

@compute @workgroup_size(8, 8, 1)
fn shadow_resolve_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = u32(clouds.shadow_map.y);
    let cascade = i32(id.z);
    if id.x >= size || id.y >= size || cascade >= i32(clouds.shadow_map.x) {
        return;
    }
    let coord = vec2<i32>(id.xy);
    // A cascade not marched this update keeps its history as it was (it is sampled with the matrix it was marched
    // with, so nothing moves).
    let marched = (u32(clouds.cascades.x) >> u32(cascade) & 1u) == 1u;
    if clouds.shadow_limits.w > 0.5 && !marched {
        textureStore(shadow_out, id.xy, cascade, textureLoad(history_shadow, coord, cascade, 0));
        return;
    }
    let current = textureLoad(current_shadow, coord, cascade, 0);
    if clouds.shadow_limits.w < 0.5 {
        textureStore(shadow_out, id.xy, cascade, current);
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / f32(size);
    let velocity = closest_fragment(coord, cascade).yz / f32(size);
    let previous_uv = uv - velocity;
    if any(previous_uv < vec2<f32>(0.0)) || any(previous_uv > vec2<f32>(1.0)) {
        textureStore(shadow_out, id.xy, cascade, current);
        return;
    }
    let history = textureSampleLevel(history_shadow, clamp_sampler, previous_uv, cascade, 0.0);
    let clipped = variance_clipping(coord, cascade, current, history);
    textureStore(shadow_out, id.xy, cascade, mix(clipped, current, TEMPORAL_ALPHA));
}
