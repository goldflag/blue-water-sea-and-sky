// The sky's mean radiance for the renderer's eye adaptation (naval_sky::SkyMeanLayer; moved here from
// naval-harness): the environment's roughest
// level straight up, as the web game's `EyeAdaptation.ts` reads `pmremTexture(environment,
// vec3(0, 1, 0), 1)` into a 1x1 target each frame. Written into the renderer's
// `exposure.sky_mean` buffer (`naval_render::SKY_MEAN_BUFFER`) with the renderer's sky
// mean set to `SkyMean::Gpu`. In w, the luminance the same mean would have without clouds (the atlas's alpha), which
// the eye meters the view against: it makes up the light clouds take from the scene, not the light the sun's fall takes.
#import naval_sky::environment::{SkyEnvironment, env_radiance_and_clear}

@group(0) @binding(0) var sky_atlas: texture_2d<f32>;
@group(0) @binding(1) var sky_sampler: sampler;
@group(0) @binding(2) var<uniform> sky_env: SkyEnvironment;

@group(1) @binding(0) var<storage, read_write> sky_mean: vec4<f32>;

@compute @workgroup_size(1)
fn main() {
    sky_mean = env_radiance_and_clear(sky_atlas, sky_sampler, sky_env, vec3<f32>(0.0, 1.0, 0.0), 1.0);
}
