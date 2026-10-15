// The environment bake, evaluated straight into level 0 of the PMREM (CubeUV) atlas: from sea level under the
// camera, the dome (sky, moon and Milky Way; no sun disc, no stars) above the horizon, and below it the sea as
// ships see it from above. Ports the bake material of the web game's `sky/environment/Environment.ts` and the
// `dome` source the web game's `sky/Sky.ts` hands it, written per cube texel as three's `_textureToCubeUV` lays level 0
// out, without the equirectangular step. With `CLOUDS` defined the clouds (`naval-clouds`) supply
// `naval_clouds::hooks` and their bind group at group 2: above the horizon, the clouds' short march over the dome
// (`clouds_bake(origin, direction)`: premultiplied radiance, transmittance in a), and below it the share of the scene's
// light they let through to the sea (`clouds_bake_light`).
//
// The sea (`sea`) is Cox and Munk's wind-roughened sea, lit as the scene is: the sky mirrored in it, the scene's light
// (the sun, or the moon once it outshines the sun: `SkyUniforms.light_direction` and `light_color`, the light the ships
// and airframes take) mirrored off the waves' facets into a glitter lobe about its mirror direction, and the light the
// water body sends back up out of what the light and the sky put into it. Until 2026-10-09 it held the mirrored sky
// over a body lit by the zenith's radiance alone: no sunlight came off the sea, which returned about 1% of the light on
// it, so an airframe's underside, a gallery's ceiling or a hull's flare took a fifth of the light from below they take
// over a real sea.

#import naval_sky::common::{sky, PI}
#import naval_sky::atmosphere::{sky_radiance, transmittance_to_space}
#import naval_sky::celestial::{celestial_diffuse_radiance}
#import naval_sky::environment::cube_uv_direction
#ifdef CLOUDS
#import naval_clouds::hooks::{clouds_bake, clouds_bake_light}
#endif

struct BakeParams {
    face_size: u32,
    first_row: u32,
    last_row: u32,
    // Radians a level-0 texel spans at a face's centre.
    texel_angle: f32,
}

@group(3) @binding(0) var atlas: texture_storage_2d<rgba16float, write>;
@group(3) @binding(1) var<uniform> bake: BakeParams;

const REC709: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// Water's reflectance looking straight down.
const WATER_F0: f32 = 0.02;
// The waves' mean square slope over all directions (Cox and Munk 1954: 0.003 + 0.00512 U) for a moderate breeze,
// U = 7 m/s: the glitter spreads about 20° about the light's mirror direction. The bake does not read the sea's wind.
const SEA_SLOPE_VARIANCE: f32 = 0.039;
// The water body's irradiance reflectance just above the surface (the light it sends up over the light on it), per
// channel: open ocean of low chlorophyll, about 3.5% in the blue, 1.5% in the green and 0.3% in the red (Morel and
// Maritorena 2001), 1.4% in luminance. With the glitter (2% of a high sun) and the sky it mirrors (6.6% of the sky's
// light), the sea returns about 5% of the light on it under a high sun, more as the sun sinks and its Fresnel rises
// (Payne 1972), while the light itself falls.
const WATER_BODY: vec3<f32> = vec3<f32>(0.003, 0.015, 0.035);
// How far about the camera (m) the sea points reach whose share of the light through the clouds the sea takes,
// averaged: the glitter an airframe a few hundred metres up sees comes from about that far.
const SEA_LIGHT_RADIUS: f32 = 1500.0;
// The glitter fades in over this far below the horizon (sine: 15 degrees). The sea's own reflection reads the horizon
// through the atlas's blurred levels (naval-ocean clamps its reflected rays to it), so a low sun's or moon's glitter
// just under the horizon would come back in its glitter path a second time, over its own.
const GLITTER_HORIZON: f32 = 0.26;
// Invocations per workgroup, each adding one sample of what the sea takes (`sea_light`).
const SEA_SAMPLES: u32 = 64u;

// What the sea takes, shared by every texel of a workgroup: in rgb the sky's radiance over SEA_SAMPLES cosine-weighted
// directions (the clear dome without the bodies' discs), whose mean times π is its irradiance on the sea; in a the
// share of the scene's light the clouds let through to SEA_SAMPLES sea points about the camera. The samples are the
// same for every workgroup, so every texel takes the same light.
var<workgroup> sea_light: array<vec4<f32>, SEA_SAMPLES>;

// Dome radiance from sea level along a direction: sky, the moon and Milky Way through the air.
fn dome(direction: vec3<f32>) -> vec3<f32> {
    return sky_radiance(direction, true) + transmittance_to_space(direction, true) * celestial_diffuse_radiance(direction);
}

fn schlick(cosine: f32) -> f32 {
    let grazing = 1.0 - clamp(cosine, 0.0, 1.0);
    let grazing2 = grazing * grazing;
    return WATER_F0 + grazing2 * grazing2 * grazing * (1.0 - WATER_F0);
}

// The sample `index` of `sea_light`: a Hammersley point taken as a cosine-weighted direction and as a point of the
// disc.
fn sea_light_sample(index: u32) -> vec4<f32> {
    let u = vec2<f32>((f32(index) + 0.5) / f32(SEA_SAMPLES), f32(reverseBits(index)) * 2.3283064365386963e-10);
    let angle = 2.0 * PI * u.y;
    let across = vec2<f32>(cos(angle), sin(angle));
    let sine = sqrt(u.x);
    let sky_light = sky_radiance(vec3<f32>(across.x * sine, sqrt(1.0 - u.x), across.y * sine), true);
    var through = 1.0;
#ifdef CLOUDS
    let at = sky.camera_position.xz + across * (SEA_LIGHT_RADIUS * sine);
    through = clouds_bake_light(vec3<f32>(at.x, 0.0, at.y), normalize(sky.light_direction));
#endif
    return vec4<f32>(sky_light, through);
}

// Smith's masking for the waves' slopes (Beckmann's, Walter et al. 2007's fit), seen at `cosine` from the zenith.
fn smith_lambda(cosine: f32) -> f32 {
    let c = clamp(cosine, 1e-4, 1.0);
    let a = c / sqrt(SEA_SLOPE_VARIANCE * max(1.0 - c * c, 1e-8));
    if a >= 1.6 {
        return 0.0;
    }
    return (1.0 - 1.259 * a + 0.396 * a * a) / (3.535 * a + 2.181 * a * a);
}

// The glitter: the light's irradiance (across its beam) to radiance toward `view` off the facets that mirror it, Cox
// and Munk's slope distribution as a microfacet distribution, F D G / (4 cos θv) (the light's cosine cancels).
fn glitter(view: vec3<f32>, toward: vec3<f32>) -> f32 {
    let half = normalize(view + toward);
    let c = max(half.y, 1e-4);
    let c2 = c * c;
    let d = exp(-(1.0 - c2) / (c2 * SEA_SLOPE_VARIANCE)) / (PI * SEA_SLOPE_VARIANCE * c2 * c2);
    let g = 1.0 / (1.0 + smith_lambda(view.y) + smith_lambda(toward.y));
    return schlick(dot(view, half)) * d * g / (4.0 * max(view.y, 1e-4));
}

// The sea's radiance seen looking down along `direction`, under the sky's irradiance `sky_irradiance`, the scene's
// light reaching it at the share `through`.
fn sea(direction: vec3<f32>, sky_irradiance: vec3<f32>, through: f32) -> vec3<f32> {
    let view = -direction;
    let fresnel = schlick(view.y);
    let toward = normalize(sky.light_direction);
    let beam = sky.light_color * (through * select(0.0, 1.0, toward.y > 0.0));
    // The sky in the sea, as a level surface mirrors it.
    var color = dome(vec3<f32>(direction.x, view.y, direction.z)) * fresnel;
    // The water body: what the light and the sky put into it, sent back up through the surface.
    let down = beam * max(toward.y, 0.0) + sky_irradiance;
    color += down * WATER_BODY * ((1.0 - fresnel) / ((1.0 - WATER_F0) * PI));
    // The light off the waves.
    color += beam * (glitter(view, toward) * smoothstep(0.0, GLITTER_HORIZON, view.y));
    return color;
}

@compute @workgroup_size(8, 8)
fn bake_main(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) index: u32) {
    sea_light[index] = sea_light_sample(index);
    workgroupBarrier();
    let s = bake.face_size;
    let row = id.y + bake.first_row;
    if id.x >= 3u * s || row >= min(2u * s, bake.last_row) {
        return;
    }
    let face = f32(id.x / s + 3u * (row / s));
    let local = vec2<f32>(f32(id.x % s), f32(row % s));
    // Texel centres span the face and its one-texel border (three's `_createPlanes` uv range).
    let uv = (local - 0.5) / f32(s - 2u);
    let direction = normalize(cube_uv_direction(uv, face));
    var color: vec3<f32>;
    // Alpha: the luminance the texel would have without clouds, which the eye meters the scene against.
    var clear: f32;
    if direction.y >= 0.0 {
        let sky_light = dome(direction);
        clear = dot(sky_light, REC709);
#ifdef CLOUDS
        let origin = vec3<f32>(sky.camera_position.x, 0.0, sky.camera_position.z);
        let clouds = clouds_bake(origin, direction);
        color = sky_light * clouds.a + clouds.rgb;
#else
        color = sky_light;
#endif
    } else {
        var mean = vec4<f32>(0.0);
        for (var i = 0u; i < SEA_SAMPLES; i++) {
            mean += sea_light[i];
        }
        mean /= f32(SEA_SAMPLES);
        let sky_irradiance = mean.rgb * PI;
        color = sea(direction, sky_irradiance, mean.a);
        clear = dot(sea(direction, sky_irradiance, 1.0), REC709);
    }
    textureStore(atlas, vec2<u32>(id.x, row), vec4<f32>(color, clear));
}
