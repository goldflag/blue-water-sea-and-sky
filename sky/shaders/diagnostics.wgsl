// Development views of the sky, for captures and tests:
// - a panorama tile of the atmosphere alone with plain sun and moon discs, graded as the display grades the
//   game, exactly as `scripts/diagnostics/sky-atmosphere.html` draws one;
// - radiance probes along fixed directions (the same page's readings);
// - the environment through the consumer's own lookup (`naval_sky::environment::env_radiance`), unwrapped.

#import naval_sky::common::{sky, air, transmittance_table, luminance}
#import naval_sky::bruneton::{get_transmittance_to_top_atmosphere_boundary}
#import naval_sky::atmosphere::{sky_radiance, transmittance_to_space, sun_sky_scalar_irradiance, sun_sky_irradiance}
#import naval_sky::environment::{SkyEnvironment, env_radiance}

struct Diagnostics {
    // Top-left pixel and size of the tile being drawn.
    origin: vec2<f32>,
    tile: vec2<f32>,
    // Azimuth (radians, compass) at the tile's middle; the panorama's lowest and highest elevation (degrees).
    centre_azimuth: f32,
    low: f32,
    high: f32,
    // 1 for the sea-level view the environment bakes.
    from_sea: f32,
    // GGX roughness of the environment view.
    roughness: f32,
    padding_a: f32,
    padding_b: f32,
    padding_c: f32,
}

@group(2) @binding(0) var<uniform> diagnostics: Diagnostics;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn fullscreen_vertex(@builtin(vertex_index) index: u32) -> VertexOut {
    let clip = vec2<f32>(select(-1.0, 3.0, index == 1u), select(-1.0, 3.0, index == 2u));
    return VertexOut(vec4<f32>(clip, 0.0, 1.0));
}

const RADIANS: f32 = 0.017453292519943295;

fn panorama_direction(pixel: vec2<f32>) -> vec3<f32> {
    let local = pixel - diagnostics.origin;
    let azimuth = (local.x / diagnostics.tile.x - 0.5) * 6.283185307179586 + diagnostics.centre_azimuth;
    let elevation = (diagnostics.high - local.y / diagnostics.tile.y * (diagnostics.high - diagnostics.low)) * RADIANS;
    return vec3<f32>(sin(azimuth) * cos(elevation), sin(elevation), cos(azimuth) * cos(elevation));
}

fn plain_disc(direction: vec3<f32>, toward: vec3<f32>, radius: f32) -> f32 {
    return smoothstep(1.0 - radius * 1.15, 1.0 - radius * 0.85, dot(direction, toward));
}

// Already graded (`DISPLAY_LOOK`); the display pass adds only AgX, as the page's renderer does.
@fragment
fn panorama_fragment(in: VertexOut) -> @location(0) vec4<f32> {
    let direction = panorama_direction(in.position.xy);
    let from_sea = diagnostics.from_sea > 0.5;
    let bodies = sky.sun_irradiance * plain_disc(direction, sky.sun_direction, 7.5e-5) * 6.0
        + sky.moon_irradiance * plain_disc(direction, sky.moon_direction, 7.5e-5) * 12.0;
    let radiance = sky_radiance(direction, from_sea) + transmittance_to_space(direction, from_sea) * bodies;
    let exposed = radiance * 0.9;
    let lum = max(luminance(exposed), 1e-5);
    let graded = max(mix(vec3<f32>(lum), exposed, 1.25), vec3<f32>(0.0)) * pow(lum / 0.18, 0.25);
    let horizon_line = smoothstep(0.0015, 0.0, abs(direction.y)) * 0.25;
    return vec4<f32>(graded + horizon_line * (1.0 - min(lum, 1.0)), 1.0);
}

@group(2) @binding(1) var<storage, read> probe_directions: array<vec4<f32>>;
@group(2) @binding(2) var<storage, read_write> probe_radiance: array<vec4<f32>>;

// One reading per probe; w picks it: 0 the sky's radiance along xyz from the camera, 1 from sea level, 2 the
// transmittance to space along xyz from sea level, 3 from the camera, 4 and 5 the sun's and the sky's scalar
// irradiance (`sun_sky_scalar_irradiance`) at the point xyz of the planet frame (km), 6 the transmittance table at
// radius x (km) and zenith cosine y, 7 and 8 the sun's and the sky's irradiance on a level surface
// (`sun_sky_irradiance`, facing up) at the point xyz.
@compute @workgroup_size(64)
fn probe_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= arrayLength(&probe_directions) {
        return;
    }
    let probe = probe_directions[id.x];
    let direction = normalize(probe.xyz);
    var value: vec3<f32>;
    switch u32(probe.w + 0.5) {
        case 0u: {
            value = sky_radiance(direction, false);
        }
        case 1u: {
            value = sky_radiance(direction, true);
        }
        case 2u: {
            value = transmittance_to_space(direction, true);
        }
        case 3u: {
            value = transmittance_to_space(direction, false);
        }
        case 4u: {
            value = sun_sky_scalar_irradiance(probe.xyz).sun;
        }
        case 5u: {
            value = sun_sky_scalar_irradiance(probe.xyz).skylight;
        }
        case 7u: {
            value = sun_sky_irradiance(probe.xyz, vec3<f32>(0.0, 1.0, 0.0)).sun;
        }
        case 8u: {
            value = sun_sky_irradiance(probe.xyz, vec3<f32>(0.0, 1.0, 0.0)).skylight;
        }
        default: {
            // The transmittance table read at radius x (km) and zenith cosine y.
            value = get_transmittance_to_top_atmosphere_boundary(air, transmittance_table, probe.x, probe.y);
        }
    }
    probe_radiance[id.x] = vec4<f32>(value, 1.0);
}

@group(3) @binding(0) var environment_atlas: texture_2d<f32>;
@group(3) @binding(1) var environment_sampler: sampler;
@group(3) @binding(2) var<uniform> environment_uniforms: SkyEnvironment;

// The environment at one roughness, unwrapped: azimuth across (compass 0 at the middle), elevation down.
@fragment
fn environment_fragment(in: VertexOut) -> @location(0) vec4<f32> {
    let local = (in.position.xy - diagnostics.origin) / diagnostics.tile;
    let azimuth = (local.x - 0.5) * 6.283185307179586 + diagnostics.centre_azimuth;
    let elevation = (0.5 - local.y) * 3.141592653589793;
    let direction = vec3<f32>(sin(azimuth) * cos(elevation), sin(elevation), cos(azimuth) * cos(elevation));
    return vec4<f32>(env_radiance(environment_atlas, environment_sampler, environment_uniforms, direction, diagnostics.roughness), 1.0);
}

// The atlas itself, as it sits in the texture.
@fragment
fn atlas_fragment(in: VertexOut) -> @location(0) vec4<f32> {
    let local = (in.position.xy - diagnostics.origin) / diagnostics.tile;
    return vec4<f32>(textureSampleLevel(environment_atlas, environment_sampler, local, 0.0).rgb, 1.0);
}
