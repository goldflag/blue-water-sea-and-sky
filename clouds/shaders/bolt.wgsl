// A cloud-to-ground bolt, ported from the web game's `sky/weather/boltMesh.ts`: the channel's segments as camera-facing
// capsules of emissive light added over the scene, each a Gaussian core at least a pixel wide (a thinner channel
// keeps its light by dimming) and a faint halo of lit air. Its brightness is the flicker `lightning.rs` computes.

struct BoltUniforms {
    view: mat4x4<f32>,
    projection: mat4x4<f32>,
    // Where the bolt meets the sea, relative to the camera.
    origin: vec3<f32>,
    // Core radiance this frame on the main channel and on the branches.
    main: f32,
    screen: vec2<f32>,
    branches: f32,
    padding: f32,
}

@group(2) @binding(0) var<uniform> bolt: BoltUniforms;

const BOLT_COLOR: vec3<f32> = vec3<f32>(0.78, 0.85, 1.0);
const MIN_SIGMA: f32 = 0.55;
const HALO_SHARE: f32 = 0.15;
const HALO_SIGMA: f32 = 5.0;
const HALO_WIDTHS: f32 = 4.0;
const NEAR: f32 = 1.0;
const SQRT_PI: f32 = 1.7724538509055159;

struct BoltVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) across: f32,
    @location(1) along: f32,
    @location(2) pixels: f32,
    @location(3) sigma: f32,
    @location(4) halo_sigma: f32,
    @location(5) core: f32,
    @location(6) halo: f32,
}

@vertex
fn bolt_vertex(@builtin(vertex_index) index: u32, @location(0) start: vec4<f32>, @location(1) end: vec4<f32>) -> BoltVertex {
    var corners = array<vec2<f32>, 4>(vec2<f32>(-1.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, 1.0));
    var order = array<u32, 6>(0u, 1u, 2u, 2u, 1u, 3u);
    let corner = corners[order[index % 6u]];
    let a = (bolt.view * vec4<f32>(bolt.origin + start.xyz, 0.0)).xyz;
    let b = (bolt.view * vec4<f32>(bolt.origin + end.xyz, 0.0)).xyz;
    let visible = a.z < -NEAR && b.z < -NEAR;
    let clip_a = bolt.projection * vec4<f32>(a, 1.0);
    let clip_b = bolt.projection * vec4<f32>(b, 1.0);
    let half_screen = bolt.screen * 0.5;
    let pixel_a = clip_a.xy / clip_a.w * half_screen;
    let pixel_b = clip_b.xy / clip_b.w * half_screen;
    let delta = pixel_b - pixel_a;
    let pixels = length(delta);
    let direction = select(vec2<f32>(0.0, 1.0), delta / max(pixels, 1e-3), pixels > 1e-3);
    let normal = vec2<f32>(-direction.y, direction.x);
    let focal = bolt.projection[1][1] * bolt.screen.y * 0.5;
    // True width in pixels, the drawn core and the halo, all Gaussian radii.
    let width = start.w * focal / ((-a.z - b.z) * 0.5);
    let core_sigma = max(width * 0.5, MIN_SIGMA);
    let glow_sigma = max(HALO_SIGMA, width * HALO_WIDTHS);
    let reach = max(core_sigma, glow_sigma) * 2.5;
    let pixel = mix(pixel_a, pixel_b, corner.y) + normal * (corner.x * reach) + direction * ((corner.y * 2.0 - 1.0) * reach);
    let clip = mix(clip_a, clip_b, corner.y);
    // Every stroke lights the main channel; only the first lights the branches.
    let light = select(bolt.branches * -end.w, bolt.main * end.w, end.w > 0.0);
    var out: BoltVertex;
    out.core = light * width / (core_sigma * SQRT_PI);
    out.halo = light * width * HALO_SHARE / (glow_sigma * SQRT_PI);
    out.across = corner.x * reach;
    out.along = corner.y * pixels + (corner.y * 2.0 - 1.0) * reach;
    out.pixels = pixels;
    out.sigma = core_sigma;
    out.halo_sigma = glow_sigma;
    out.position = select(vec4<f32>(0.0, 0.0, 2.0, 1.0), vec4<f32>(pixel / half_screen * clip.w, clip.z, clip.w), visible);
    return out;
}

@fragment
fn bolt_fragment(in: BoltVertex) -> @location(0) vec4<f32> {
    // Distance to the segment (a capsule); past either end each cap carries half, so joints sum to one.
    let beyond = max(max(-in.along, in.along - in.pixels), 0.0);
    let distance2 = in.across * in.across + beyond * beyond;
    let cap = select(1.0, 0.5, beyond > 0.0);
    let radiance = in.core * exp(-distance2 / (in.sigma * in.sigma)) + in.halo * exp(-distance2 / (in.halo_sigma * in.halo_sigma));
    return vec4<f32>(BOLT_COLOR * (radiance * cap), 0.0);
}
