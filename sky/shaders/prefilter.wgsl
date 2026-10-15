// The environment's prefilter: one half of three's separable Gaussian cube blur per dispatch, reading a
// level of one atlas and writing a level of the other. The chain blurs each level from the one before by the
// Gaussian that widens it to the level's own width, latitudinally about a pole and then longitudinally,
// turning the pole from level to level. Ports `blur`/`getSample` in three r185
// `src/nodes/pmrem/PMREMUtils.js`, `_halfBlur` in `src/renderers/common/extras/PMREMGenerator.js`, and the
// Gaussian chain `SkyPMREMGenerator._applyPMREM` in the web game's `sky/environment/Environment.ts` uses in place of
// three's GGX importance sampling.

#import naval_sky::environment::{cube_uv_direction, cube_uv_bilinear_rgba}

const MAX_SAMPLES: u32 = 20u;

struct Blur {
    // Normalised Gaussian weights, four to a vec4 (uniform arrays keep a 16-byte stride).
    weights: array<vec4<f32>, 5>,
    pole_axis: vec3<f32>,
    // Radians between samples.
    d_theta: f32,
    texel_size: vec2<f32>,
    max_mip: f32,
    // The level read (as `env_radiance` numbers them).
    mip_int: f32,
    samples: u32,
    latitudinal: u32,
    // Face size of the level written, and where its block of faces starts.
    out_size: u32,
    padding: u32,
    out_origin: vec2<u32>,
    padding_tail: vec2<u32>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> blur: Blur;
@group(1) @binding(0) var destination: texture_storage_2d<rgba16float, write>;

fn weight(i: u32) -> f32 {
    return blur.weights[i / 4u][i % 4u];
}

fn get_sample(theta: f32, axis: vec3<f32>, output_direction: vec3<f32>) -> vec4<f32> {
    let cos_theta = cos(theta);
    // Rodrigues' axis-angle rotation.
    let direction = output_direction * cos_theta + cross(axis, output_direction) * sin(theta)
        + axis * (dot(axis, output_direction) * (1.0 - cos_theta));
    return cube_uv_bilinear_rgba(source, source_sampler, blur.texel_size, blur.max_mip, direction, blur.mip_int);
}

@compute @workgroup_size(8, 8)
fn blur_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let s = blur.out_size;
    if id.x >= 3u * s || id.y >= 2u * s {
        return;
    }
    let face = f32(id.x / s + 3u * (id.y / s));
    let local = vec2<f32>(f32(id.x % s), f32(id.y % s));
    let output_direction = normalize(cube_uv_direction((local - 0.5) / f32(s - 2u), face));
    var axis = select(cross(blur.pole_axis, output_direction), blur.pole_axis, blur.latitudinal == 1u);
    if all(axis == vec3<f32>(0.0)) {
        axis = vec3<f32>(output_direction.z, 0.0, -output_direction.x);
    }
    axis = normalize(axis);
    var color = weight(0u) * get_sample(0.0, axis, output_direction);
    for (var i = 1u; i < MAX_SAMPLES; i++) {
        if i >= blur.samples {
            break;
        }
        let theta = blur.d_theta * f32(i);
        color += weight(i) * (get_sample(-theta, axis, output_direction) + get_sample(theta, axis, output_direction));
    }
    textureStore(destination, blur.out_origin + id.xy, color);
}
