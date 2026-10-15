// The cloud field: weather, shape and density at a point, a port of `clouds.glsl` of @takram/three-clouds. Where
// takram looks its textures up from ECEF positions and a cube-sphere UV of the globe, the game's are world-fixed and
// planar in the clouds' drifting frame (`noise_position`): the weather tiles every `1 / repeats.x` metres (100 km, the
// scale of takram's 100 repeats per cube face), the shape and detail volumes every `1 / repeats.y` and `1 / repeats.z`
// metres as takram's do, and the whole field rides the scene's wind.
//
// MIT License, Copyright (c) 2024 Shota Matsuda (https://github.com/takram-design-engineering/three-geospatial). The
// full notice is in `naval-clouds/THIRD_PARTY.md`.
#define_import_path naval_clouds::field

#import naval_clouds::common::{clouds, local_weather, shape_volume, detail_volume, turbulence_map, blue_noise, repeat_sampler, remap_clamped, remap_clamped4, saturate4, planet_to_world}

const GOLDEN_RATIO: f32 = 0.6180339887498949;

// takram's `getSTBN`: a blue-noise value in [0, 1) for a pixel (the pass's own pixel grid) on this update. The
// clouds' 128² mask, offset each frame by the golden ratio over a cycle of 64 frames, stands in for NVIDIA's
// spatiotemporal blue noise.
fn stbn(pixel: vec2<u32>) -> f32 {
    let level = textureLoad(blue_noise, vec2<i32>(pixel % vec2<u32>(128u)), 0).r;
    let frame = f32(u32(clouds.frame.x) % 64u);
    return fract((level * 255.0 + 0.5) / 256.0 + frame * GOLDEN_RATIO);
}

// The march's jitter for a pixel of the drawing buffer on its `sample`-th march (the update counter over 16: each pixel
// is marched once in 16 updates): the blue noise at the pixel, stepped by the golden ratio once per march of that
// pixel, so the samples a pixel's mean gathers spread evenly over [0, 1). `stbn` steps every update over a cycle of
// 64, which gave a pixel, marched every 16th update, only four jitters, all within a third of [0, 1): its mean never
// reached the clouds, and the grain stood in every still.
fn march_jitter(pixel: vec2<u32>, sample: u32) -> f32 {
    let level = textureLoad(blue_noise, vec2<i32>(pixel % vec2<u32>(128u)), 0).r;
    return fract((level * 255.0 + 0.5) / 256.0 + f32(sample % 256u) * GOLDEN_RATIO);
}

// A planet-frame position (m) in the clouds' drifting world frame: (east, height above the sea sphere, north), the
// coordinates the shape, detail and turbulence are looked up in.
fn noise_position(position: vec3<f32>) -> vec3<f32> {
    let world = planet_to_world(position);
    let height = length(position) - clouds.sun.w;
    return vec3<f32>(world.x - clouds.drift.x, height, world.z - clouds.drift.z);
}

// The weather's tile coordinates of a planet-frame position (takram's `globeUv × localWeatherRepeat`).
fn weather_uv(position: vec3<f32>) -> vec2<f32> {
    let world = planet_to_world(position);
    return (world.xz - clouds.drift.xz) * clouds.repeats.x;
}

// True between layers, where the march strides over empty air.
fn inside_layer_intervals(height: f32) -> bool {
    let gt = vec3<f32>(height) > clouds.min_interval_heights.xyz;
    let lt = vec3<f32>(height) < clouds.max_interval_heights.xyz;
    return any(gt & lt);
}

struct WeatherSample {
    // Each layer's normalised height.
    height_fraction: vec4<f32>,
    density: vec4<f32>,
}

// A semicircle that rounds the clouds toward their tops.
fn shape_altering(height_fraction: vec4<f32>, bias: vec4<f32>) -> vec4<f32> {
    let biased = pow(height_fraction, bias);
    let x = clamp(biased * 2.0 - 1.0, vec4<f32>(-1.0), vec4<f32>(1.0));
    return 1.0 - x * x;
}

// The weather of every layer at a point; `shadow` keeps only the layers that cast shadows.
fn sample_weather(uv: vec2<f32>, height: f32, mip_level: f32, shadow: bool) -> WeatherSample {
    var weather: WeatherSample;
    weather.height_fraction = remap_clamped4(vec4<f32>(height), clouds.min_layer_heights, clouds.max_layer_heights);
    let texel = textureSampleLevel(local_weather, repeat_sampler, uv, mip_level);
    let local = pow(texel * clouds.channels, clouds.weather_exponents);
    let height_scale = shape_altering(weather.height_fraction, clouds.shape_altering_biases);
    // Modulation that lets the coverage control the weather (after Skybolt's `Clouds.h`), each layer by its own.
    let factor = 1.0 - clouds.layer_coverages * height_scale;
    weather.density = remap_clamped4(
        mix(local, vec4<f32>(1.0), clouds.coverage_filter_widths),
        factor,
        factor + clouds.coverage_filter_widths,
    );
    // The mask after the coverage (takram masks the weather before it, which a high coverage mixes back toward one, so
    // a layer that casts no shadow still shaded under it).
    if shadow {
        weather.density *= clouds.shadow_layer_mask;
    }
    return weather;
}

fn layer_density(height_fraction: vec4<f32>) -> vec4<f32> {
    return clouds.density_exp_terms * exp(clouds.density_exponents * height_fraction)
        + clouds.density_linear_terms * height_fraction
        + clouds.density_constant_terms;
}

struct MediaSample {
    density: f32,
    // Each layer's share of the density.
    weight: vec4<f32>,
    scattering: f32,
    extinction: f32,
}

// ---- The detail ---------------------------------------------------------------------------------------------------
//
// The clouds' structure from a few tens of metres down, carved out of their edges with the detail noise
// (`naval_clouds::detail_noise`: one 128³ volume tiling every `1 / repeats.z` metres, 100 m; r and g low- and
// high-frequency wisps, b and a low- and high-frequency billows, each stored bumps bright and creases dark). After the
// up-rez of Guerrilla's Nubis³ (Schneider, SIGGRAPH 2023): the frequency by the depth into the cloud, wisps or billows
// by the height in the layer. Every march reads it at mip 3 or coarser (texels of 6 m, about takram's 5 m; 16 KB, which
// stays in the texture cache as takram's 32³ did, where mip 2's 131 KB cost the march half as much again), carved to
// takram's statistics, so the clouds keep their size, their soft edge and their noise. A paused or held picture reads
// the same field as play: until 2026-10-08 it read a sharper near field close to a camera in the air (the carving at
// full contrast, the inside uneven, the edge sharpened, the finer mips), which drew a near cumulus as a mottled web of
// dark creases the moment the game paused.

// Texels per edge of the detail noise.
const DETAIL_SIZE: f32 = 128.0;
// The detail's mip never holds texels finer than a DETAIL_STEP_SHARE of the march's step (detail a march steps over is
// noise; the 8 to 16 jittered samples a pixel gathers resolve what lies within a step that far) nor than
// DETAIL_PIXEL_SHARE of a pixel of the drawing buffer at the sample.
const DETAIL_STEP_SHARE: f32 = 0.125;
const DETAIL_PIXEL_SHARE: f32 = 0.5;
// The finest mip any march reads: 16³ texels of 6 m (a march at Ultra's 10 m steps reading mips 0 and 1 paid their cache
// misses, 0.7 to 3.5 ms, on every sample).
const DETAIL_COARSE_LOD: f32 = 3.0;
// What the detail carves: the mean (takram's wisps carved 0.17 of the edge on average, its billows 0.29: the clouds
// keep their size and their depth) and the contrast about it (the spread takram's had). The last is the bumps' own mean.
const WISP_MEAN: f32 = 0.17;
const BILLOW_MEAN: f32 = 0.29;
const CARVE_CONTRAST: f32 = 0.47;
const BUMP_MEAN: f32 = 0.35;

// The noise's mip for a sample `distance` metres from the camera on a march stepping `step` metres: by the step and the
// pixel, never finer than DETAIL_COARSE_LOD.
fn detail_lod(step: f32, distance: f32) -> f32 {
    let texel = 1.0 / (clouds.repeats.z * DETAIL_SIZE);
    let pixel = distance * 2.0 / (clouds.forward.w * clouds.screen.y);
    let lod = log2(max(max(step * DETAIL_STEP_SHARE, pixel * DETAIL_PIXEL_SHARE) / texel, 1.0));
    return max(lod, DETAIL_COARSE_LOD);
}

// What the detail carves of each layer from the wisps' and the billows' bumps there (0 a crease to 1 a bump's top).
fn detail_erosion(wisp_bumps: vec4<f32>, billow_bumps: vec4<f32>, billowy: vec4<f32>) -> vec4<f32> {
    let wisps = WISP_MEAN + (wisp_bumps - BUMP_MEAN) * CARVE_CONTRAST;
    let billows = BILLOW_MEAN + (BUMP_MEAN - billow_bumps) * CARVE_CONTRAST;
    return saturate4(mix(wisps, billows, billowy));
}

// Each layer's density (0 at the edge to 1 in the core, the weather carved by the shape) with the detail carved out of
// it, at `at` (the clouds' drifting frame, m), the noise read at mip `lod` (`detail_lod`).
fn carve_detail(density: vec4<f32>, height_fraction: vec4<f32>, at: vec3<f32>, lod: f32) -> vec4<f32> {
    let noise = textureSampleLevel(detail_volume, repeat_sampler, at * clouds.repeats.z, lod);
    let depth = saturate4(density);
    // Wisps at the bottom of a layer, billows at its top (takram's "fluffy at the top and whippy at the bottom").
    let billowy = remap_clamped4(height_fraction, vec4<f32>(0.2), vec4<f32>(0.4));
    // Wisps: the bumps' tops carved away, the web of creases between them the filaments that stay. Billows: the creases
    // carved away, the bumps the lobes that stay. Each the low frequency at the edge, with more of the high frequency
    // toward the core (the direction Nubis³ prints; the other way round the edge is all fine grain and no lobe shows).
    let erosion = detail_erosion(
        mix(vec4<f32>(noise.r), vec4<f32>(noise.g), depth * 0.5),
        mix(vec4<f32>(noise.b), vec4<f32>(noise.a), depth * 0.6),
        billowy,
    ) * clouds.shape_detail_amounts;
    return remap_clamped4(density * 2.0, erosion * 0.5, vec4<f32>(1.0));
}

// The participating medium at a point (`sampleMedia`), its detail read at mip `detail_lod`.
fn sample_media_at(weather: WeatherSample, position: vec3<f32>, uv: vec2<f32>, mip_level: f32, jitter: f32, detail_lod: f32) -> MediaSample {
    var density = weather.density;
    let at = noise_position(position);
    // The shape evolves as the weather moves: takram shifts it along the surface normal by the weather's offset.
    let evolution = vec3<f32>(0.0, -clouds.drift.w, 0.0);

    var turbulence = vec3<f32>(0.0);
    if clouds.switches.y > 0.5 {
        let turbulence_uv = uv * clouds.repeats.w;
        turbulence = clouds.medium.w
            * (textureSampleLevel(turbulence_map, repeat_sampler, turbulence_uv, 0.0).rgb * 2.0 - 1.0)
            * dot(density, remap_clamped4(weather.height_fraction, vec4<f32>(0.3), vec4<f32>(0.0)));
    }

    let shape_position = (at + evolution + turbulence) * clouds.repeats.y;
    let shape = textureSampleLevel(shape_volume, repeat_sampler, shape_position, 0.0).r;
    density = remap_clamped4(density, vec4<f32>(1.0 - shape) * clouds.shape_amounts, vec4<f32>(1.0));

    if clouds.switches.x > 0.5 && mip_level * 0.5 + (jitter - 0.5) * 0.5 < 0.5 {
        density = carve_detail(density, weather.height_fraction, at + turbulence, detail_lod);
    }

    // The density profiles.
    density = saturate4(density * clouds.density_scales * layer_density(weather.height_fraction));

    var media: MediaSample;
    let sum = density.x + density.y + density.z + density.w;
    media.density = sum;
    media.weight = density / max(sum, 1e-12);
    media.scattering = sum * clouds.medium.x;
    media.extinction = sum * clouds.medium.y + media.scattering;
    return media;
}

// The same for a march toward the light: the detail at its coarsest mip, so the light stands still as the camera moves.
fn sample_media(weather: WeatherSample, position: vec3<f32>, uv: vec2<f32>, mip_level: f32, jitter: f32) -> MediaSample {
    return sample_media_at(weather, position, uv, mip_level, jitter, DETAIL_COARSE_LOD);
}

// The share of the coverage-filtered weather's density the shape and detail noise leave on average, for estimates that
// read the weather alone (`column_above`).
const SHAPE_FILL: f32 = 0.5;

// The clouds straight above a point: the optical depth of every layer over it, from the point (or the layer's bottom)
// to the layer's top, its weather read at the point's column at a quarter and three quarters of that span; and how much
// of a closed slab lies over it, 0 none to 1 a deck or fog (the largest slab weight of the layers above,
// `naval_clouds::layers::Layer::slab`). For the light that diffuses down through a deck
// (`naval_clouds::march::deck_skylight`).
struct ColumnAbove {
    optical_depth: f32,
    closed: f32,
}

// Whether any layer is a slab: fair weather (takram's layers) skips `column_above`.
fn any_deck() -> bool {
    return any(clouds.layer_slabs > vec4<f32>(0.0));
}

fn column_above(uv: vec2<f32>, height: f32, mip_level: f32) -> ColumnAbove {
    let texel = textureSampleLevel(local_weather, repeat_sampler, uv, mip_level);
    let local = pow(texel * clouds.channels, clouds.weather_exponents);
    let filtered = mix(local, vec4<f32>(1.0), clouds.coverage_filter_widths);
    let bottom = max(vec4<f32>(height), clouds.min_layer_heights);
    let top = clouds.max_layer_heights;
    let extent = max(top - bottom, vec4<f32>(0.0));
    var density = vec4<f32>(0.0);
    for (var i = 0; i < 2; i++) {
        let at = bottom + extent * (0.25 + 0.5 * f32(i));
        let fraction = remap_clamped4(at, clouds.min_layer_heights, clouds.max_layer_heights);
        let factor = 1.0 - clouds.layer_coverages * shape_altering(fraction, clouds.shape_altering_biases);
        let weather = remap_clamped4(filtered, factor, factor + clouds.coverage_filter_widths);
        density += 0.5 * saturate4(weather * clouds.density_scales * layer_density(fraction));
    }
    let extinction = density * SHAPE_FILL * (clouds.medium.x + clouds.medium.y);
    let closure = select(vec4<f32>(0.0), clouds.layer_slabs, extent > vec4<f32>(0.0));
    return ColumnAbove(dot(extinction, extent), max(max(closure.x, closure.y), max(closure.z, closure.w)));
}

// The weather texture's mip for a ray's start (takram's `getMipLevel` of the globe UV's screen derivatives, taken here
// from the footprint of one pixel there): `distance` (m) along a ray at `slant` (|cos| between the ray and the vertical)
// from a camera whose pixels each span `pixel_angle` radians.
fn weather_mip_level(distance: f32, slant: f32, pixel_angle: f32) -> f32 {
    let footprint = distance * pixel_angle / max(slant, 0.05);
    // Weather tiles per pixel of the march buffer (a quarter of the drawing buffer's), in units of the buffer.
    let delta = footprint * clouds.repeats.x * clouds.screen.y * 4.0;
    let delta_max_sqr = delta * delta * 0.1;
    return max(0.0, 0.5 * log2(max(1.0, delta_max_sqr)));
}
