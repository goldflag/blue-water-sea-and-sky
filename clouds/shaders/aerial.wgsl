// The aerial perspective for everything in the world: fills the renderer's froxel volume (naval_render::aerial) for the
// frame's camera, after Hillaire 2020 ("A Scalable and Production Ready Sky and Atmosphere Rendering Technique", the
// camera-volume aerial perspective). Along each column's ray the air is two media:
//
// - the atmosphere the dome and the clouds take (takram's tables: molecules, the map's clear-air aerosol, ozone), whose
//   in-scatter up to a point is `sky_radiance_to_point` (graded as the clouds' aerial perspective is), seen at its
//   `aerial_scale` share (`naval_sky::model::aerial`: half on a fair day): its opacity that thin, its in-scatter the
//   tables' over their opacity times it, so its colour at a distance is the tables' and only the depth is thinner;
// - the weather's haze (naval_sky::haze): a marine boundary layer of aerosol, densest at the sea and thinning up to the
//   layer's top, then quickly above it, its extinction at sea level the rest of what the visibility asks
//   (`SIGHTING_DEPTH` over the visibility: `Haze::sighting_extinction`, less the atmosphere's own), with its own albedo
//   and Cornette–Shanks phase. It scatters the sun and the moon (through the atmosphere, its own haze above,
//   delta-Eddington scaled, and the clouds' ground shadow) and the sky's light (the environment, the clouds included,
//   weighted by the phase about the ray).
//
// The haze dims the atmosphere's in-scatter by the haze in front of where it scatters (its transmittance weighted by the
// atmosphere's scattering along the ray), and the clouds' shadow takes the sun's share of it where the air lies under
// them: the sky's light under the clouds (the environment, which holds them) lights that air instead, so the air under
// a closed deck is the deck's grey rather than a sunlit band at the horizon. Each thread integrates one slice interval
// of one column (4 steps); a scan down the column sums them. The last slice's thread goes on to the end of the air for the dome's slice: the haze's
// in-scatter along the ray and the share of the dome's light it lets through.

#import naval_sky::common::{sky, air, transmittance_table, PI}
#import naval_sky::atmosphere::{planet_km, sky_radiance_to_point, camera_km, held}
#import naval_sky::bruneton::{get_transmittance_to_sun, get_profile_density, mie_phase_function}
#import naval_sky::environment::{SkyEnvironment, env_radiance}
#import naval_clouds::shadow::{CloudShadow, shadow_uv, clear_berth, EDGE}
#import naval::aerial_volume::{AERIAL_SLICES, aerial_slice_distance}

struct AerialParams {
    // The frame camera's clip → world (its jitter included: the materials project with the same camera).
    inverse_view_projection: mat4x4<f32>,
    // xyz the eye (world m), w 1 to fill the volume (0: clear air).
    eye: vec4<f32>,
    // x the extinction at sea (per m, 550 nm) the visibility asks of the air and the haze together
    // (`Haze::sighting_extinction`), y the boundary layer's top (m), z the aerosol's scale height within it (m), w its
    // taper's above the top (m).
    haze: vec4<f32>,
    // x the haze's asymmetry (Cornette–Shanks g), y its single-scattering albedo, z and w the bottom and the top (m) of
    // the clouds that cast the ground shadow: the air under their bottom takes the shadow map's shade, over their top
    // none.
    phase: vec4<f32>,
    // x the share of the atmosphere a view sees (`AirCoefficients::aerial_scale`).
    view: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: AerialParams;
// The volume: in-scatter in its first `SLICES` layers, opacity (1 − transmittance) in the next; the dome slice's alpha
// is its opacity too. Zeros are clear air.
@group(0) @binding(1) var volume_out: texture_storage_3d<rgba16float, write>;
@group(0) @binding(2) var sky_out: texture_storage_2d<rgba16float, write>;
// Group 1 is the sky's (naval_sky::common): the tables, their air and the sky's uniforms.
@group(2) @binding(0) var env_atlas: texture_2d<f32>;
@group(2) @binding(1) var env_sampler: sampler;
@group(2) @binding(2) var<uniform> env: SkyEnvironment;
@group(3) @binding(0) var shadow_map: texture_2d<f32>;
@group(3) @binding(1) var shadow_sampler: sampler;
@group(3) @binding(2) var<uniform> shadow: CloudShadow;

const METER_TO_KM: f32 = 0.001;
// Columns a workgroup takes (2 × 2), slices down each, steps through each slice's interval.
const COLUMNS: u32 = 4u;
const SLICES: u32 = 32u;
const STEPS: u32 = 4u;
// Steps from the last slice to the end of the air (the dome's slice), spaced exponentially.
const TAIL_STEPS: u32 = 24u;
// The tail ends where the ray stands this far above the sea (km): the atmosphere's scattering above it is negligible.
const TAIL_TOP: f32 = 40.0;
// The first slice's distance (m), where a march from the eye's own slice starts.
const AERIAL_NEAR_M: f32 = 20.0;

// One slice interval of a column, from its start: the haze's in-scatter (rgb), the transmittance over it (rgb, both
// media), and greys for the scan: the haze's transmittance, the atmosphere's (550 nm), and the atmosphere's scattering
// weighted by the haze in front of it (x) and alone (y).
var<workgroup> interval_scatter: array<vec3<f32>, 128>;
var<workgroup> interval_transmittance: array<vec3<f32>, 128>;
var<workgroup> interval_haze: array<f32, 128>;
var<workgroup> interval_atmosphere: array<f32, 128>;
var<workgroup> interval_weights: array<vec2<f32>, 128>;
// The clouds' shade over the ground shadow map.
var<workgroup> shade_taps: array<f32, 128>;

// The haze's extinction at sea level (per m): what the visibility asks, less the atmosphere's own there as a view sees
// it (`Haze::extinction_at_sea`).
fn haze_extinction() -> f32 {
    let atmosphere = air.rayleigh_scattering.y * get_profile_density(air.rayleigh_density, 0.0)
        + air.mie_extinction.y * get_profile_density(air.mie_density, 0.0)
        + air.absorption_extinction.y * get_profile_density(air.absorption_density, 0.0);
    return max(params.haze.x - params.view.x * atmosphere * METER_TO_KM, 0.0);
}

// The tables' in-scatter over a path whose transmittance through their whole air is `transmittance`, as the view's share
// of that air scatters it: their opacity at that share over their own. An optically thin path takes the share itself.
fn thinned(transmittance: vec3<f32>) -> vec3<f32> {
    let k = params.view.x;
    let opacity = 1.0 - clamp(transmittance, vec3<f32>(0.0), vec3<f32>(1.0));
    let thin = 1.0 - pow(clamp(transmittance, vec3<f32>(1e-6), vec3<f32>(1.0)), vec3<f32>(k));
    return select(thin / max(opacity, vec3<f32>(1e-6)), vec3<f32>(k), opacity < vec3<f32>(1e-4));
}

// The haze's density at `height` (m above the sea) over its density at the sea (`Haze::density`).
fn haze_density(height: f32) -> f32 {
    let h = max(height, 0.0);
    let top = params.haze.y;
    if h <= top {
        return exp(-h / params.haze.z);
    }
    return exp(-top / params.haze.z) * exp(-(h - top) / params.haze.w);
}

// Metres of sea-level haze along a ray from `height` (m) whose zenith cosine there is `mu`, out of the air: the layer's
// exponential to its top, then the taper, each held at its grazing length (Chapman's limit) on the planet
// (`Haze::path`).
fn haze_path(height: f32, mu: f32) -> f32 {
    let bottom = air.bottom_radius * 1000.0;
    let h = max(height, 0.0);
    let r = bottom + h;
    let top = params.haze.y;
    let scale = params.haze.z;
    let taper = params.haze.w;
    if h < top {
        let r_top = bottom + top;
        let lower = scale * (exp(-h / scale) - exp(-top / scale)) / max(mu, sqrt(2.0 * scale / (PI * r)));
        let across = r / r_top * sqrt(max(1.0 - mu * mu, 0.0));
        let mu_top = select(0.0, sqrt(max(1.0 - across * across, 0.0)), mu > 0.0);
        return lower + exp(-top / scale) * taper / max(mu_top, sqrt(2.0 * taper / (PI * r_top)));
    }
    return haze_density(h) * taper / max(mu, sqrt(2.0 * taper / (PI * r)));
}

// Henyey–Greenstein's share of its scattering at angles whose cosine exceeds `mu`, for asymmetry `g`.
fn lobe_share(g: f32, mu: f32) -> f32 {
    if abs(g) < 1e-3 {
        return 0.5 * (1.0 - mu);
    }
    return (1.0 - g * g) / (2.0 * g) * (1.0 / (1.0 - g) - inverseSqrt(1.0 + g * g - 2.0 * g * mu));
}

// The sky's light the haze scatters toward the eye along `direction`: the environment (the sky above, the sea below,
// the clouds included) weighted by the haze's phase. A quadrature over bands of angle from the ray, each band's share of
// a Henyey–Greenstein lobe of the haze's asymmetry: the cone within 25° from a sharper level (a Gaussian of about 12°,
// so the bright band of sky right behind the haze is not averaged away), rings at 50° and 105° and the back from the
// roughest (about 33°). Forward scattering takes most of it from the sky behind the haze along the ray.
fn sky_on_haze(direction: vec3<f32>) -> vec3<f32> {
    let g = params.phase.x;
    // Band edges at 25°, 75° and 135°, and each band's share of the lobe.
    let within_25 = lobe_share(g, 0.9063078);
    let within_75 = lobe_share(g, 0.2588190);
    let within_135 = lobe_share(g, -0.7071068);
    let level = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(direction.y) > 0.999);
    let across = normalize(cross(direction, level));
    let up = cross(across, direction);
    // Ring taps at 50° (cosine 0.643, sine 0.766) and 105° (cosine −0.259, sine 0.966) from the ray.
    var near = vec3<f32>(0.0);
    var far = vec3<f32>(0.0);
    for (var i = 0u; i < 4u; i++) {
        let angle = f32(i) * 1.5707963;
        let side = across * cos(angle) + up * sin(angle);
        near += env_radiance(env_atlas, env_sampler, env, direction * 0.6427876 + side * 0.7660444, 1.0);
        far += env_radiance(env_atlas, env_sampler, env, direction * -0.2588190 + side * 0.9659258, 1.0);
    }
    return within_25 * env_radiance(env_atlas, env_sampler, env, direction, 0.4)
        + (within_75 - within_25) * 0.25 * near
        + (within_135 - within_75) * 0.25 * far
        + (1.0 - within_135) * env_radiance(env_atlas, env_sampler, env, -direction, 1.0);
}

// The share of the scene's light (the sun's, or the moon's once it outshines the sun) the clouds let through to a world
// point (m): the clouds' ground shadow map down the light's rays, as `cloud_shadow` reads it, but toward the map's mean
// beyond its reach rather than toward full light, so the far haze under a deck stays shaded.
fn cloud_light(position: vec3<f32>, mean: f32) -> f32 {
    let at = shadow_uv(shadow, position);
    let t = clamp((at.z - 1.0) / -EDGE, 0.0, 1.0);
    let inside = t * t * (3.0 - 2.0 * t);
    let raw = textureSampleLevel(shadow_map, shadow_sampler, at.xy, 0.0).r;
    let shaded = mix(1.0, mix(mean, raw, inside), shadow.strength * shadow.enabled);
    return clear_berth(shadow, position, shaded);
}

// The share of the scene's light the clouds leave the air at a world point (m) `height` metres up: the ground shadow's
// under the shadowing clouds' bottom, all of it over their top, in proportion between.
fn air_light(position: vec3<f32>, height: f32, mean: f32) -> f32 {
    let top = params.phase.w;
    if height >= top {
        return 1.0;
    }
    let bottom = params.phase.z;
    return mix(cloud_light(position, mean), 1.0, clamp((height - bottom) / max(top - bottom, 1.0), 0.0, 1.0));
}

// The light of a body (`scale` its game units per unit of the tables: the sun's or the moon's) on the haze at a point,
// through the atmosphere and the haze above it, times the haze's phase toward the eye along `direction`.
fn body_light(
    point: vec3<f32>,
    height: f32,
    direction: vec3<f32>,
    body: vec3<f32>,
    scale: vec3<f32>,
    beam_extinction: f32,
) -> vec3<f32> {
    let r = max(length(point), air.bottom_radius + 1e-6);
    let mu = dot(point, body) / length(point);
    let through_air = air.solar_irradiance * get_transmittance_to_sun(air, transmittance_table, r, mu) * scale;
    return through_air * exp(-beam_extinction * haze_path(height, mu)) * mie_phase_function(params.phase.x, dot(direction, body));
}

// The radiance the haze scatters toward the eye per unit of its scattering coefficient at a world point (m), the
// clouds leaving `lit` of the bodies' light there.
fn haze_source(position: vec3<f32>, direction: vec3<f32>, extinction: f32, ambient: vec3<f32>, lit: f32) -> vec3<f32> {
    let point = planet_km(position);
    let height = (length(point) - air.bottom_radius) * 1000.0;
    // Delta-Eddington: what the haze scatters forward stays in the beam it lights the haze below with.
    let beam = (1.0 - params.phase.y * params.phase.x) * extinction;
    var light = vec3<f32>(0.0);
    if sky.sun_active > 0.5 {
        light += body_light(point, height, direction, sky.sun_direction, sky.direct_scale, beam);
    }
    if sky.moon_active > 0.5 {
        light += body_light(point, height, direction, sky.moon_direction, sky.moon_scale, beam);
    }
    return light * lit + ambient;
}

// The atmosphere's extinction and scattering (rgb), and its scattering at 550 nm (per m), at `height_km` above the sea:
// the share a view sees.
struct AtmosphereAt {
    extinction: vec3<f32>,
    scattering_rgb: vec3<f32>,
    scattering: f32,
}

fn atmosphere_at(height_km: f32) -> AtmosphereAt {
    let rayleigh = get_profile_density(air.rayleigh_density, height_km);
    let mie = get_profile_density(air.mie_density, height_km);
    let ozone = get_profile_density(air.absorption_density, height_km);
    let extinction = air.rayleigh_scattering * rayleigh + air.mie_extinction * mie + air.absorption_extinction * ozone;
    let scattering = air.rayleigh_scattering * rayleigh + air.mie_scattering * mie;
    let seen = params.view.x * METER_TO_KM;
    return AtmosphereAt(extinction * seen, scattering * seen, scattering.y * seen);
}

// A stretch of a ray integrated from its start (see the workgroup arrays).
struct Stretch {
    scatter: vec3<f32>,
    transmittance: vec3<f32>,
    haze: f32,
    atmosphere: f32,
    weights: vec2<f32>,
}

fn empty_stretch() -> Stretch {
    return Stretch(vec3<f32>(0.0), vec3<f32>(1.0), 1.0, 1.0, vec2<f32>(0.0));
}

// One step of `span` metres centred `distance` along the ray, onto `stretch`.
fn step_stretch(
    stretch: Stretch,
    direction: vec3<f32>,
    distance: f32,
    span: f32,
    extinction_at_sea: f32,
    ambient: vec3<f32>,
    shade_mean: f32,
) -> Stretch {
    let position = params.eye.xyz + direction * distance;
    let height_km = length(planet_km(position)) - air.bottom_radius;
    let haze = extinction_at_sea * haze_density(height_km * 1000.0);
    let atmosphere = atmosphere_at(height_km);
    let extinction = atmosphere.extinction + vec3<f32>(haze);
    let lit = air_light(position, height_km * 1000.0, shade_mean);
    // The haze's light, and the sky's light on the atmosphere's share the clouds shade (the tables light it all with
    // the sun), over the step.
    var source = atmosphere.scattering_rgb * (1.0 - lit) * ambient;
    // Above the layer the haze's in-scatter is nothing worth its lights' reads.
    if haze * span > 1e-6 {
        source += haze * params.phase.y * haze_source(position, direction, extinction_at_sea, ambient, lit);
    }
    var out = stretch;
    out.scatter += stretch.transmittance * source * (1.0 - exp(-extinction * span)) / max(extinction, vec3<f32>(1e-12));
    // The atmosphere's sunlit scattering here, and the haze in front of it (to the middle of the step).
    let weight = atmosphere.scattering * stretch.atmosphere * span;
    out.weights += vec2<f32>(weight * stretch.haze * exp(-0.5 * haze * span) * lit, weight);
    out.transmittance = stretch.transmittance * exp(-extinction * span);
    out.haze = stretch.haze * exp(-haze * span);
    out.atmosphere = stretch.atmosphere * exp(-atmosphere.extinction.y * span);
    return out;
}

// `later` (integrated from its own start) after `earlier`.
fn join(earlier: Stretch, later: Stretch) -> Stretch {
    return Stretch(
        earlier.scatter + earlier.transmittance * later.scatter,
        earlier.transmittance * later.transmittance,
        earlier.haze * later.haze,
        earlier.atmosphere * later.atmosphere,
        earlier.weights + earlier.atmosphere * vec2<f32>(earlier.haze * later.weights.x, later.weights.y),
    );
}

// The haze's mean transmittance over the atmosphere's in-scatter, and the clouds' light on it: what the haze and the
// clouds' shadow leave of the sky's sunlit air along the ray.
fn sky_share(stretch: Stretch) -> f32 {
    return select(stretch.haze, stretch.weights.x / stretch.weights.y, stretch.weights.y > 1e-12);
}

// Metres along a ray from the eye (planet frame, km) to where it stands `TAIL_TOP` km up.
fn tail_top(eye: vec3<f32>, direction: vec3<f32>) -> f32 {
    let b = dot(eye, direction);
    let top = air.bottom_radius + TAIL_TOP;
    return (-b + sqrt(max(b * b - dot(eye, eye) + top * top, 0.0))) * 1000.0;
}

// Metres along a ray from the eye (planet frame, km) to the sea, or -1 when it misses the planet.
fn sea_hit(eye: vec3<f32>, direction: vec3<f32>) -> f32 {
    let b = dot(eye, direction);
    let ground = b * b - dot(eye, eye) + air.bottom_radius * air.bottom_radius;
    if b < 0.0 && ground > 0.0 {
        return (-b - sqrt(ground)) * 1000.0;
    }
    return -1.0;
}

// Slice `slice`'s interval of the ray along `direction` (4 steps).
fn slice_interval(direction: vec3<f32>, slice: u32, extinction_at_sea: f32, ambient: vec3<f32>, shade_mean: f32) -> Stretch {
    let start = select(0.0, aerial_slice_distance(f32(slice) - 1.0), slice > 0u);
    let end = aerial_slice_distance(f32(slice));
    let span = (end - start) / f32(STEPS);
    var stretch = empty_stretch();
    for (var s = 0u; s < STEPS; s++) {
        stretch = step_stretch(stretch, direction, start + (f32(s) + 0.5) * span, span, extinction_at_sea, ambient, shade_mean);
    }
    return stretch;
}

fn store_interval(cell: u32, stretch: Stretch) {
    interval_scatter[cell] = stretch.scatter;
    interval_transmittance[cell] = stretch.transmittance;
    interval_haze[cell] = stretch.haze;
    interval_atmosphere[cell] = stretch.atmosphere;
    interval_weights[cell] = stretch.weights;
}

@compute @workgroup_size(2, 2, 32)
fn aerial_main(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_id) local: vec3<u32>,
    @builtin(local_invocation_index) index: u32,
) {
    let column = local.x + local.y * 2u;
    let slice = local.z;
    let cell = column * SLICES + slice;

    // The clouds' mean shade over the ground shadow map, for the haze beyond its reach.
    let grid = vec2<f32>(f32(index % 16u) + 0.5, f32(index / 16u) + 0.5) / vec2<f32>(16.0, 8.0);
    shade_taps[index] = textureSampleLevel(shadow_map, shadow_sampler, grid, 0.0).r;
    workgroupBarrier();
    var shade_mean = 0.0;
    for (var i = 0u; i < 128u; i++) {
        shade_mean += shade_taps[i];
    }
    shade_mean /= 128.0;

    // The column's ray through the middle of its cell of the screen.
    let size = vec2<f32>(textureDimensions(sky_out));
    let uv = (vec2<f32>(id.xy) + 0.5) / size;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let near = params.inverse_view_projection * vec4<f32>(ndc, 1.0, 1.0);
    let direction = normalize(near.xyz / near.w - params.eye.xyz);
    let on = params.eye.w > 0.5;
    let ambient = sky_on_haze(direction);

    // This slice's interval of the ray.
    let end = aerial_slice_distance(f32(slice));
    let extinction_at_sea = haze_extinction();
    var stretch = empty_stretch();
    if on {
        stretch = slice_interval(direction, slice, extinction_at_sea, ambient, shade_mean);
    }
    store_interval(cell, stretch);
    workgroupBarrier();

    // Down the column to this slice.
    var total = empty_stretch();
    let first = column * SLICES;
    for (var j = 0u; j <= slice; j++) {
        let k = first + j;
        total = join(total, Stretch(interval_scatter[k], interval_transmittance[k], interval_haze[k], interval_atmosphere[k], interval_weights[k]));
    }

    let texel = vec3<u32>(id.xy, slice);
    let through = vec3<u32>(id.xy, SLICES + slice);
    if !on {
        textureStore(volume_out, texel, vec4<f32>(0.0));
        textureStore(volume_out, through, vec4<f32>(0.0));
        if slice == SLICES - 1u {
            textureStore(sky_out, id.xy, vec4<f32>(0.0));
        }
        return;
    }

    // The rays held at the horizon (`held`, as the dome's own sky is) for a column whose ray meets the sea: its slices
    // past the sea and the dome's slice take the air along the horizon's ray instead (below).
    let eye = planet_km(params.eye.xyz);
    let eye_km = vec3<f32>(0.0, max(eye.y, air.bottom_radius + 0.002), 0.0);
    workgroupBarrier();
    let dome_direction = held(direction, camera_km().y);
    var dome_ambient = ambient;
    if any(dome_direction != direction) {
        dome_ambient = sky_on_haze(dome_direction);
        store_interval(cell, slice_interval(dome_direction, slice, extinction_at_sea, dome_ambient, shade_mean));
    }
    workgroupBarrier();

    // A slice wholly past where the column's ray meets the sea holds the air along the horizon's ray to its distance.
    // Nothing in the world is drawn there along this ray (the sea and the hulls stand nearer), but the pixels above the
    // horizon read this column too: the clouds just over it, 20 to 100 km off, take their air at their own distance.
    // Followed under the sea the ray met the haze at its densest all the way, so those clouds took the sea's air in the
    // share the horizon stood within the columns' rows, and pumped as the view pitched and it crossed them.
    let sea = sea_hit(eye_km, direction);
    let start = select(0.0, aerial_slice_distance(f32(slice) - 1.0), slice > 0u);
    let beyond = sea > 0.0 && start >= sea && any(dome_direction != direction);
    var ray = direction;
    if beyond {
        ray = dome_direction;
        total = empty_stretch();
        for (var j = 0u; j <= slice; j++) {
            let k = first + j;
            total = join(total, Stretch(interval_scatter[k], interval_transmittance[k], interval_haze[k], interval_atmosphere[k], interval_weights[k]));
        }
    }

    // The atmosphere's own light up to the slice (as the clouds take it) at the share a view sees, dimmed by the haze in
    // front of it.
    let point_km = planet_km(params.eye.xyz + ray * end);
    let lifted = point_km * max(1.0, (air.bottom_radius + 1e-3) / max(length(point_km), 1e-3));
    let tables = sky_radiance_to_point(eye_km, lifted, 0.0);
    let atmosphere = max(tables.radiance, vec3<f32>(0.0)) * thinned(tables.transmittance);
    textureStore(volume_out, texel, vec4<f32>(atmosphere * sky_share(total) + total.scatter, 1.0));
    textureStore(volume_out, through, vec4<f32>(1.0 - total.transmittance, 1.0));

    // The dome's slice: the haze along the column's ray to the end of the air, the ray held at the horizon: a column
    // whose ray meets the sea takes the air along the horizon's ray. The dome shows only above the drawn sea, so its
    // pixels just above the horizon read the columns below it too: holding them keeps the slice running on smoothly
    // across the horizon. A column that ended at the sea instead (opaque, the far sea's colour) bled a band of it into
    // the sky over the horizon, as deep as the columns are tall, which pumped as the view pitched and the horizon
    // crossed the columns' rows.
    if slice == SLICES - 1u {
        let last = tail_top(eye_km, dome_direction);
        var dome = empty_stretch();
        var reached = 0.0;
        for (var j = 0u; j < SLICES; j++) {
            let distance = aerial_slice_distance(f32(j));
            if distance > last {
                break;
            }
            let k = first + j;
            dome = join(dome, Stretch(interval_scatter[k], interval_transmittance[k], interval_haze[k], interval_atmosphere[k], interval_weights[k]));
            reached = distance;
        }
        if last > reached {
            let start_tail = max(reached, AERIAL_NEAR_M);
            let ratio = pow(last / start_tail, 1.0 / f32(TAIL_STEPS));
            var near = start_tail;
            for (var s = 0u; s < TAIL_STEPS; s++) {
                let far = near * ratio;
                dome = step_stretch(dome, dome_direction, 0.5 * (near + far), far - near, extinction_at_sea, dome_ambient, shade_mean);
                near = far;
            }
        }
        textureStore(sky_out, id.xy, vec4<f32>(dome.scatter, 1.0 - sky_share(dome)));
    }
}
