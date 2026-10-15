// A stand-in sky behind the sea for the ocean lane's stills (the sky lane draws the real dome): the environment
// atlas's sharpest level along each pixel's view ray, as naval_sky::environment samples it. It has no sun disc, stars
// or clouds finer than the atlas's texels. Drawn in the renderer core's Sky phase: a full-screen triangle at depth 0,
// where the reversed GreaterEqual test passes only on pixels the sea left clear.

#import naval::camera::{camera, view_ray}
#import naval_ocean::sky_environment::{SkyEnvironment, env_radiance}

@group(1) @binding(0) var sky_atlas: texture_2d<f32>;
@group(1) @binding(1) var sky_sampler: sampler;
@group(1) @binding(2) var<uniform> sky_env: SkyEnvironment;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> VertexOut {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var output: VertexOut;
    output.ndc = uv * 2.0 - 1.0;
    output.clip = vec4<f32>(output.ndc, 0.0, 1.0);
    return output;
}

@fragment
fn fragment(input: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(env_radiance(sky_atlas, sky_sampler, sky_env, view_ray(input.ndc), 0.0), 1.0);
}
