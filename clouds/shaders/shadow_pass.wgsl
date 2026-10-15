// The clouds' Beer shadow maps (`shadow.frag` of @takram/three-clouds): for each texel of each cascade, a march from
// the sun's side down through the shadow-casting layers along the sunlight, with structured volume sampling (Bowles &
// Zimmermann's volsample, after huwb) so the sample planes hold still from frame to frame. A texel keeps the front
// depth of what it crossed (km, transmittance-weighted), its mean extinction, its optical depth and the optical depth
// beyond the march's end (the tail); and, for the temporal pass, its front depth and motion in texels.
//
// One change from takram: its march starts at the light camera's near plane, clamped to it, so a cascade whose near
// plane falls inside the layers (a low frustum under a high sun) loses the cloud above it. Here every texel's march
// runs from where its ray enters the top of the shadow-casting layers, whichever side of the plane that is; the front
// depth then always counts from that top, which is what readers measure from (`distance_to_shadow_top`).
//
// MIT License, Copyright (c) 2024 Shota Matsuda (https://github.com/takram-design-engineering/three-geospatial). The
// full notice, and volsample's (MIT, Copyright (c) 2016 Huw Bowles & Daniel Zimmermann), are in
// `naval-clouds/THIRD_PARTY.md`.

#import naval_clouds::common::{clouds, world_to_planet, planet_to_world}
#import naval_clouds::field::{stbn, sample_weather, sample_media, weather_uv, inside_layer_intervals}

@group(3) @binding(0) var shadow_out: texture_storage_2d_array<rgba16float, write>;
@group(3) @binding(1) var depth_velocity_out: texture_storage_2d_array<rgba16float, write>;

// The vertices of the icosahedron whose face the direction crosses (Sloan, "Ambient Dice").
fn icosahedral_vertices(direction: vec3<f32>) -> array<vec3<f32>, 3> {
    // Scales that fit the dodecahedron to the unit sphere.
    let a = 0.85065080835204;
    let b = 0.5257311121191336;
    let k_t = 0.6180339887498948;
    let k_t2 = 0.38196601125010515;
    let abs_d = abs(direction);
    let selector1 = dot(abs_d, vec3<f32>(1.0, k_t2, -k_t));
    let selector2 = dot(abs_d, vec3<f32>(-k_t, 1.0, k_t2));
    let selector3 = dot(abs_d, vec3<f32>(k_t2, -k_t, 1.0));
    let octant = sign(direction);
    let v1 = select(vec3<f32>(-b, 0.0, a), vec3<f32>(a, b, 0.0), selector1 > 0.0) * octant;
    let v2 = select(vec3<f32>(a, -b, 0.0), vec3<f32>(0.0, a, b), selector2 > 0.0) * octant;
    let v3 = select(vec3<f32>(0.0, a, -b), vec3<f32>(b, 0.0, a), selector3 > 0.0) * octant;
    return array<vec3<f32>, 3>(v1, v2, v3);
}

fn pentagonal_weights(direction: vec3<f32>, v1: vec3<f32>, v2: vec3<f32>, v3: vec3<f32>) -> vec3<f32> {
    let w = exp(vec3<f32>(dot(v1, direction), dot(v2, direction), dot(v3, direction)) * 40.0);
    return w / (w.x + w.y + w.z);
}

// One of the three nearest structure normals, chosen by the jitter in proportion to their weights.
fn structure_normal(direction: vec3<f32>, jitter: f32) -> vec3<f32> {
    let vertices = icosahedral_vertices(direction);
    // Sorted by their dot with (0.5, 0.5, 1).
    let base = vec3<f32>(0.5, 0.5, 1.0);
    var a = vec4<f32>(vertices[0], dot(vertices[0], base));
    var b = vec4<f32>(vertices[1], dot(vertices[1], base));
    var c = vec4<f32>(vertices[2], dot(vertices[2], base));
    if a.w > b.w {
        let t = a;
        a = b;
        b = t;
    }
    if b.w > c.w {
        let t = b;
        b = c;
        c = t;
    }
    if a.w > b.w {
        let t = a;
        a = b;
        b = t;
    }
    let weights = pentagonal_weights(direction, a.xyz, b.xyz, c.xyz);
    return select(select(c.xyz, b.xyz, jitter < weights.x + weights.y), a.xyz, jitter < weights.x);
}

fn modulo(x: f32, y: f32) -> f32 {
    return x - y * floor(x / y);
}

// The march over the shadow-casting layers (`marchClouds` of `shadow.frag`): front depth (m), mean extinction, most
// optical depth and its tail. `fixed_origin` is the ray's start in a frame that does not move with the camera, so the
// structure's planes stay put.
fn march_shadow(ray_origin: vec3<f32>, ray_direction: vec3<f32>, max_ray_distance: f32, jitter: f32, mip_level: f32, fixed_origin: vec3<f32>) -> vec4<f32> {
    let max_iterations = i32(clouds.shadow_march.x);
    let normal = structure_normal(ray_direction, jitter);
    let sample_period = clamp(max_ray_distance / f32(max_iterations), clouds.shadow_march.y, clouds.shadow_march.z);
    let n_dot_d = dot(ray_direction, normal);
    let step_size = sample_period / abs(n_dot_d);
    // Skip to the first of the structure's planes in front of the origin.
    var ray_distance = -modulo(dot(fixed_origin, normal), sample_period) / n_dot_d;
    if ray_distance < 0.0 {
        ray_distance += step_size;
    }
    // The temporal jitter.
    ray_distance -= step_size * jitter;

    var extinction_sum = 0.0;
    var max_optical_depth = 0.0;
    var max_optical_depth_tail = 0.0;
    var transmittance_integral = 1.0;
    var weighted_distance_sum = 0.0;
    var transmittance_sum = 0.0;
    var sample_count = 0;
    for (var i = 0; i < max_iterations; i++) {
        if ray_distance > max_ray_distance {
            break;
        }
        let position = ray_distance * ray_direction + ray_origin;
        let height = length(position) - clouds.sun.w;
        if inside_layer_intervals(height) {
            ray_distance += step_size;
            continue;
        }
        let uv = weather_uv(position);
        let weather = sample_weather(uv, height, mip_level, true);
        if any(weather.density > vec4<f32>(clouds.shadow_march.w)) {
            // A homogeneous medium between samples.
            let media = sample_media(weather, position, uv, mip_level, jitter);
            if media.extinction > clouds.shadow_limits.x {
                extinction_sum += media.extinction;
                max_optical_depth += media.extinction * step_size;
                transmittance_integral *= exp(-media.extinction * step_size);
                weighted_distance_sum += ray_distance * transmittance_integral;
                transmittance_sum += transmittance_integral;
                sample_count += 1;
            }
        }
        if transmittance_integral <= clouds.shadow_limits.y {
            // Much optical depth lies past the least transmittance, falling off with the samples taken before it.
            max_optical_depth_tail = min(clouds.shadow_limits.z * step_size * exp(f32(1 - sample_count)), step_size * 0.5);
            break;
        }
        ray_distance += step_size;
    }
    if sample_count == 0 {
        return vec4<f32>(max_ray_distance, 0.0, 0.0, 0.0);
    }
    let front_depth = min(weighted_distance_sum / transmittance_sum, max_ray_distance);
    let mean_extinction = extinction_sum / f32(sample_count);
    return vec4<f32>(front_depth, mean_extinction, max_optical_depth, max_optical_depth_tail);
}

fn clip_to_uv(clip: vec4<f32>) -> vec2<f32> {
    let ndc = clip.xy / clip.w;
    return vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
}

@compute @workgroup_size(8, 8, 1)
fn shadow_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = u32(clouds.shadow_map.y);
    // The dispatch covers the cascades this update marches: layer `id.z` is the mask's `id.z`-th set bit
    // (`clouds.cascades.x`: the round robin's and any the view has left; all after a cut).
    let mask = u32(clouds.cascades.x);
    var cascade = -1;
    var seen = 0u;
    for (var bit = 0u; bit < 4u; bit++) {
        if (mask >> bit & 1u) == 1u {
            if seen == id.z {
                cascade = i32(bit);
            }
            seen += 1u;
        }
    }
    if id.x >= size || id.y >= size || cascade < 0 || cascade >= i32(clouds.shadow_map.x) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / f32(size);
    let clip = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    // The texel's point on the light camera's near plane.
    let near = clouds.inverse_shadow_matrices[cascade] * vec4<f32>(clip, 0.0, 1.0);
    let light_world = near.xyz / near.w;
    let light_position = world_to_planet(light_world);
    let ray_direction = -clouds.sun.xyz;

    // Where the sunlight through the texel enters the top of the shadow-casting layers and leaves their bottom (or the
    // top again when it misses the bottom).
    let top = clouds.sun.w + clouds.heights.z;
    let bottom = clouds.sun.w + clouds.heights.w;
    let b = 2.0 * dot(ray_direction, light_position);
    let top_discriminant = b * b - 4.0 * (dot(light_position, light_position) - top * top);
    let bottom_discriminant = b * b - 4.0 * (dot(light_position, light_position) - bottom * bottom);
    var color = vec4<f32>(0.0);
    var ray_origin = light_position;
    if top_discriminant > 0.0 {
        let ray_near = (-b - sqrt(top_discriminant)) * 0.5;
        let ray_far = select((-b + sqrt(top_discriminant)) * 0.5, (-b - sqrt(bottom_discriminant)) * 0.5, bottom_discriminant > 0.0);
        ray_origin = light_position + ray_direction * ray_near;
        if ray_far > ray_near {
            let mip_levels = array<f32, 4>(0.0, 0.5, 1.0, 2.0);
            let jitter = stbn(id.xy);
            let fixed_origin = planet_to_world(ray_origin) - clouds.drift.xyz;
            color = march_shadow(ray_origin, ray_direction, ray_far - ray_near, jitter, mip_levels[cascade], fixed_origin);
        }
    }
    // The front depth in kilometres, which half floats hold at any sun.
    textureStore(shadow_out, id.xy, cascade, vec4<f32>(color.x * 0.001, color.yzw));

    // Motion for the temporal pass, in texels.
    let front = planet_to_world(ray_origin + ray_direction * color.x);
    let previous = clip_to_uv(clouds.shadow_reprojection[cascade] * vec4<f32>(front, 1.0));
    textureStore(depth_velocity_out, id.xy, cascade, vec4<f32>(color.x * 0.001, (uv - previous) * f32(size), 0.0));
}
