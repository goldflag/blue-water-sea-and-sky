// Lakes and ponds at their own levels (native only, 2026-10-05; native/docs/ocean.md, "Lakes and ponds"): flat water
// over a theatre window's lakes (naval_ocean::lakes), shaded with the sea's optics: the mean Fresnel
// and sun glitter of naval_ocean::reflection, the water body of naval_ocean::water_body (with fresh water's absorption and
// silt, or a lagoon's or salt pond's, and where the window's satellite photograph covers a lake its own colour), small
// wind ripples in patches, the ships' and clouds' shadows, the scene's air. The shore is soft where the opaque scene's
// depth says the water thins to nothing; where the land is drawn coarser than the lakes' grid the lakes' own mask draws
// the outline (`mask_led`). The water decides what hides it from the opaque scene's depth (the land, at the pixel), not
// the depth test, which it passes in front of the sea's waves (they are not the lakes' to show) and of land drawn too
// coarsely to trust.
//
// Bind groups: 0 the camera (naval::camera) with the sun's shadow maps and the aerial volume, 1 the lakes' own, 2 the
// sky environment.

#import naval::camera::{camera, curved, view_depth}
#import naval::shadow::water_sun_shadow
#import naval::motion::motion_response
#import naval::aerial::aerial_apply
#import naval_clouds::shadow::{CloudShadow, cloud_shadow}
#import naval_ocean::noise::value_noise
#import naval_ocean::sky_environment::{SkyEnvironment, env_radiance, env_irradiance}
#import naval_ocean::reflection::{pmrem_roughness, wind_variances, mean_fresnel, sun_glitter, soft_knee}
#import naval_ocean::water_body::{REFRACTION_SQUARED, subsurface_reflectance, upwelling, column_transmittance, water_body}

// `naval_ocean::lakes::LakeUniforms`.
struct Lake {
    // xy: the chart's offset (world = chart + offset), z: 1 with the scene's air applied.
    offset: vec4<f32>,
    // xy: toward where the wind blows (unit, xz), z: its speed (m/s), w: the water shadows' quality.
    wind: vec4<f32>,
    // xy: chart metres at the mask's texture coordinate (0, 0), zw: texture coordinates a metre.
    mask: vec4<f32>,
    // x: the land's samples' spacing (m), y: its finest vertices' (m), z: the pixels its cells span, w: the lakes' cell.
    land: vec4<f32>,
    // x: how high the sea's crests reach (m), which the water stands in front of.
    sea: vec4<f32>,
    fresh_absorption: vec4<f32>,
    fresh_backscatter: vec4<f32>,
    salt_absorption: vec4<f32>,
    salt_backscatter: vec4<f32>,
    // The window's photograph: xy chart metres at its corner, z its texture coordinates a metre, w 1 while bound.
    photo: vec4<f32>,
    // rgb: the haze to take out of it (`naval_terrain::SurfaceMaps::statistics`), w: its texels a side.
    photo_haze: vec4<f32>,
}

@group(1) @binding(0) var<uniform> lake: Lake;
@group(1) @binding(1) var lake_mask: texture_2d<f32>;
@group(1) @binding(2) var mask_sampler: sampler;
// The opaque scene behind the water (colour and reversed depth): the bed through the water, and the shore.
@group(1) @binding(3) var scene_color: texture_2d<f32>;
@group(1) @binding(4) var scene_depth: texture_depth_2d;
// The clouds' shadow (naval_clouds::CloudShadow; a disabled placeholder until the host binds it).
@group(1) @binding(5) var cloud_map: texture_2d<f32>;
@group(1) @binding(6) var cloud_sampler: sampler;
@group(1) @binding(7) var<uniform> cloud_params: CloudShadow;
// The window's satellite photograph (the land layer's colour array: layer 0, sRGB, premultiplied by its coverage).
@group(1) @binding(8) var photo_map: texture_2d_array<f32>;
@group(1) @binding(9) var photo_sampler: sampler;

@group(2) @binding(0) var sky_atlas: texture_2d<f32>;
@group(2) @binding(1) var sky_sampler: sampler;
@group(2) @binding(2) var<uniform> sky_env: SkyEnvironment;

const PI: f32 = 3.141592653589793;
// The sea's ceiling on its glints (a share of the light's intensity).
const GLITTER_KNEE: f32 = 0.25;
// The facets' squared slope, in shares of their variance, past which the sun's glitter is not evaluated (e^-20).
const GLINT_REACH: f32 = 40.0;
// The mask's contour that cuts the water (the mask is 1 at a lake's samples and 0.25 at the land beside a straight
// shore, `LakeMesh::build`): near the camera the land crossing the level draws the outline and the contour, past it
// (about 65 m from the last sample), only stops water spilling over land lower than the lake beyond its raised shore;
// far off, where the mask draws the outline, the contour where the land would cross for a bed 1 m down and a shore
// 0.3 m up (0.77 of the way to the next sample).
const MASK_NEAR: f32 = 0.30;
const MASK_FAR: f32 = 0.42;
// The contour's soft width, as a share of the mask.
const MASK_SOFT: f32 = 0.06;
// Where the mask draws the outline the water is taken this deep (m) at a lake's samples, for its colour: the land
// beneath is too coarse to say.
const MASK_DEPTH: f32 = 3.0;
// Where the mask draws the outline the water stands over coarse land up to this far above it (m), along the view ray
// at most this share of the distance (a hill in front still hides it).
const COARSE_RISE: f32 = 10.0;
const COARSE_SHARE: f32 = 0.03;
// The water thins to nothing over this depth (m) at the shore.
const SHORE_FADE: f32 = 0.25;
// Land standing this little (m) above the water at a pixel does not hide it: the land's own level of detail moves its
// crossing by about as much.
const LAND_SLACK: f32 = 0.05;
// Where the opaque scene behind the water lies below this height (m) it is the sea's floor, not a lake's bed (the bake
// keeps every lake's bed at least 0.25 m above the sea): no lake there. Where the mask draws the outline the coarse land
// may dip lower at a lake's edge.
const SEA_BED: f32 = 0.1;
const SEA_BED_COARSE: f32 = -3.0;
// How much of a lake's colour the photograph gives where it covers the lake's middle, and the sky's glare it holds on
// water (reflectance at 620, 550 and 460 nm), taken out with the haze.
const PHOTO_TRUST: f32 = 0.85;
const PHOTO_BIAS: f32 = 2.0;
const PHOTO_GLARE: vec3<f32> = vec3<f32>(0.010, 0.013, 0.020);
// Cox and Munk's clean sea's slope variance is 0.003 + 0.00512 U; a lake's surface films and sheltering banks calm it.
const RIPPLE_CALM: f32 = 0.6;
// The share of the slope variance the ripples carry where a pixel resolves them all (the rest stays unresolved).
const RIPPLE_RESOLVED: f32 = 0.6;
// Gusts in patches (cat's paws): metres along and across the wind, and how far the patches drift (a share of the wind).
const GUST_ALONG: f32 = 170.0;
const GUST_ACROSS: f32 = 90.0;
const GUST_DRIFT: f32 = 0.6;
// The water shadows' quality the lakes take at most (one comparison a map: the shadows on a lake are mostly the land's
// and trees', soft at the maps' texels anyway).
const SHADOW_QUALITY: u32 = 1u;

struct VertexIn {
    // Chart x, the level, chart z.
    @location(0) position: vec3<f32>,
    // x: fetch (m), y: 1 for a lagoon or salt pond, z: the lake's own number in 0..1.
    @location(1) lake: vec4<f32>,
}

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    // World position (the plane's: shading, shadows, the air).
    @location(0) world: vec3<f32>,
    // x: the water's view depth (m) where it is drawn, before it is pulled toward the eye; y: the mask-led share.
    @location(1) depth: vec2<f32>,
    @location(2) lake: vec4<f32>,
}

// How much the lakes' mask, not the land, draws the outline `distance` metres off: as far as the land is drawn coarser
// there than the lakes' grid (its window's samples near the camera, its patches' level of detail far off: a level's
// vertices twice the last's over a range twice as far, from `naval_terrain::lod::base_range`, morphing onto the next).
fn mask_led(distance: f32) -> f32 {
    let pixels_per_radian = camera.viewport.y / (2.0 * camera.tan_half_fov_y);
    let spacing = lake.land.y;
    let range = max(2.0 * spacing * pixels_per_radian / lake.land.z, 8.0 * spacing * 32.0);
    let land = max(lake.land.x, spacing * max(1.0, 2.0 * distance / range));
    return smoothstep(0.75, 1.5, land / lake.land.w);
}

@vertex
fn vertex(input: VertexIn) -> VertexOut {
    let world = vec3<f32>(input.position.x + lake.offset.x, input.position.y, input.position.z + lake.offset.y);
    let drawn = curved(world);
    let to = drawn - camera.position;
    let distance = max(length(to), 1e-3);
    let share = mask_led(distance);
    // The water is pulled toward the eye along its own ray (it covers the same pixels) past the sea's crests under a lake
    // near the sea's level and, where the mask draws the outline, past the coarse land within it: the fragment stage
    // decides what hides it from the opaque scene's depth.
    let rise = max(abs(to.y) / distance, 0.02);
    let over = max(share * COARSE_RISE, lake.sea.x - input.position.y);
    let pull = min(over / rise, select(0.5, COARSE_SHARE, lake.sea.x <= input.position.y) * distance);
    let pulled = camera.position + to * (1.0 - pull / distance);
    var output: VertexOut;
    output.clip = camera.view_projection * vec4<f32>(pulled, 1.0);
    output.world = world;
    output.depth = vec2<f32>(-(camera.view * vec4<f32>(drawn, 1.0)).z, share);
    output.lake = input.lake;
    return output;
}

// The lakes' mask at a world point: 1 at a lake's samples, 0 at the land's, blended between.
fn mask_at(world: vec3<f32>) -> f32 {
    let chart = world.xz - lake.offset.xy;
    return textureSampleLevel(lake_mask, mask_sampler, (chart - lake.mask.xy) * lake.mask.zw, 0.0).r;
}

// The contour the water is cut at for a mask-led share.
fn contour(share: f32) -> f32 {
    return mix(MASK_NEAR, MASK_FAR, share);
}

struct Ripples {
    // The resolved slope (∂y/∂x, ∂y/∂z).
    slope: vec2<f32>,
    // The slope variance left to the unresolved facets.
    unresolved: f32,
}

// One gravity-capillary wavelet (surface tension over density 7.3e-5 m³/s² in its frequency) about the wind's heading:
// `turn` its heading's cosine and sine from the wind's, with its wavenumber and angular frequency; `shape` its
// wavelength (m), phase and share of the resolved variance. It shows where a pixel holds a fifth of its wavelength or
// less (finer, it aliases into stripes), and its share of the variance goes to the unresolved facets beyond.
fn wavelet(r: ptr<function, Ripples>, xz: vec2<f32>, footprint: f32, wind: vec2<f32>, variance: f32, turn: vec4<f32>,
    shape: vec3<f32>) {
    let heading = vec2<f32>(wind.x * turn.x - wind.y * turn.y, wind.x * turn.y + wind.y * turn.x);
    let phase = turn.z * dot(heading, xz) - turn.w * camera.time + shape.y;
    let shown = 1.0 - smoothstep(0.1, 0.2, footprint / shape.x);
    let share = variance * RIPPLE_RESOLVED * shape.z;
    // A wavelet of slope amplitude a has slope variance a² / 2.
    (*r).slope += heading * (sqrt(2.0 * share) * cos(phase) * shown);
    (*r).unresolved -= share * shown;
}

// Wind ripples on the water at `xz`, with `variance` the water's whole slope variance: a few wavelets (High four, 0.37
// to 2.4 m; Medium two), each fading into the unresolved roughness as the pixel (`footprint` m) outgrows it. Their
// constants are worked out ahead (`turn`), so a pixel pays a cosine a wavelet.
fn ripples(xz: vec2<f32>, footprint: f32, wind: vec2<f32>, variance: f32) -> Ripples {
    var r = Ripples(vec2<f32>(0.0), variance);
#ifdef LAKE_HIGH
    wavelet(&r, xz, footprint, wind, variance, vec4<f32>(0.92866, 0.37092, 16.9816, 12.9208), vec3<f32>(0.37, 0.0, 0.22));
    wavelet(&r, xz, footprint, wind, variance, vec4<f32>(0.86782, -0.49688, 8.8496, 9.3201), vec3<f32>(0.71, 1.3, 0.24));
    wavelet(&r, xz, footprint, wind, variance, vec4<f32>(0.99281, 0.11971, 4.7963, 6.8600), vec3<f32>(1.31, 2.9, 0.27));
    wavelet(&r, xz, footprint, wind, variance, vec4<f32>(0.89157, -0.45289, 2.5857, 5.0365), vec3<f32>(2.43, 4.1, 0.27));
#else
    wavelet(&r, xz, footprint, wind, variance, vec4<f32>(0.95233, 0.30506, 10.1342, 9.9746), vec3<f32>(0.62, 0.0, 0.5));
    wavelet(&r, xz, footprint, wind, variance, vec4<f32>(0.93590, -0.35227, 3.6744, 6.0041), vec3<f32>(1.71, 2.1, 0.5));
#endif
    r.unresolved = max(r.unresolved, variance * 0.05);
    return r;
}

// The wind's patches on the water (cat's paws drifting downwind): a gain on its slope variance, glassy to rough.
fn gusts(xz: vec2<f32>, wind: vec2<f32>, speed: f32) -> f32 {
    let along = dot(xz, wind) - speed * GUST_DRIFT * camera.time;
    let across = dot(xz, vec2<f32>(-wind.y, wind.x));
    // One octave, its lattice turned off the wind's axes and its contrast soft, so no cell of it shows as a block.
    let turned = vec2<f32>(along * 0.8 - across * 0.6, along * 0.6 + across * 0.8);
    let n = value_noise(turned / vec2<f32>(GUST_ALONG, GUST_ACROSS), 911u);
    return mix(0.45, 1.4, smoothstep(0.1, 0.9, n));
}

// What the water stands over at a pixel: the opaque surface behind it along the ray, from the opaque depth.
struct Behind {
    // Along the ray from the water to the opaque surface (below zero where it stands in front), metres.
    in_front: f32,
    // The surface's height (the Earth's drop taken back out), metres.
    height: f32,
    // Nothing opaque behind (the clear depth).
    open: bool,
}

fn behind_at(pixel: vec2<i32>, world: vec3<f32>, view_z: f32) -> Behind {
    let depth = textureLoad(scene_depth, pixel, 0);
    let ray = normalize(curved(world) - camera.position);
    let axis = max(-(camera.view * vec4<f32>(ray, 0.0)).z, 1e-3);
    let behind_view = view_depth(depth);
    let drawn = camera.position + ray * (behind_view / axis);
    let across = drawn.xz - camera.position.xz;
    return Behind((behind_view - view_z) / axis, drawn.y + camera.curvature * dot(across, across), depth <= 0.0);
}

// No lake beyond the mask's contour, over the sea's floor (the bake keeps every lake's bed above `SEA_BED`), or where the
// land stands in front of the water (by more than the coarse land's tolerance where the mask leads).
fn cut_away(mask: f32, share: f32, behind: Behind, steep: f32) -> bool {
    let sea = !behind.open && behind.height < mix(SEA_BED, SEA_BED_COARSE, share);
    let hidden = !behind.open && behind.in_front * steep < -(share * COARSE_RISE + LAND_SLACK);
    return mask <= contour(share) || sea || hidden;
}

@fragment
fn fragment(input: VertexOut) -> @location(0) vec4<f32> {
    let world = input.world;
    let share = input.depth.y;
    let to_camera = camera.position - world;
    let distance = length(to_camera);
    let view = to_camera / distance;
    let steep = max(view.y, 0.02);
    // First what cuts the water away, before any shading (nothing below takes screen-space derivatives).
    let pixel = vec2<i32>(input.clip.xy);
    let behind = behind_at(pixel, world, input.depth.x);
    let mask = mask_at(world);
    if cut_away(mask, share, behind, steep) {
        discard;
    }
    let salt = input.lake.y;
    let wind = lake.wind.xy;
    let speed = lake.wind.z;
    // The pixel's size on the water (m): the mean of its extents across and along the view, as the sea takes it.
    let footprint = distance * 2.0 * camera.tan_half_fov_y * camera.viewport.w / sqrt(max(view.y, 0.05));
    // The water's slope variance: Cox and Munk's for the wind, calmed by a short fetch and the lake's surface, in patches.
    let fetch = clamp(sqrt(input.lake.x / 10000.0), 0.25, 1.0);
    let variance = (0.003 + 0.00512 * speed) * RIPPLE_CALM * fetch * gusts(world.xz, wind, speed);
    let rippled = ripples(world.xz, footprint, wind, variance);
    let up = normalize(vec3<f32>(-rippled.slope.x, 1.0, -rippled.slope.y));
    let variances = wind_variances(rippled.unresolved, speed);

    // The light: the sun by day, the moon once it outshines it, shaded by the land, the ships and the clouds.
    let sun_direction = sky_env.light_direction;
    let sun_intensity = sky_env.light_intensity;
    let sun_radiance = sky_env.light_color * sun_intensity;
    let shadow = water_sun_shadow(world, min(u32(lake.wind.w), SHADOW_QUALITY))
        * cloud_shadow(cloud_map, cloud_sampler, cloud_params, world);
    let lit = mix(1.0, shadow, clamp(sun_intensity / 2.0, 0.0, 1.0));
    let daylight = env_irradiance(sky_atlas, sky_sampler, sky_env, vec3<f32>(0.0, 1.0, 0.0));

    // How deep the water stands over the bed, straight down and along the ray. Where the mask draws the outline the land
    // beneath is too coarse to read: the water is taken deeper toward the middle of the lake, by the mask.
    let along = select(max(behind.in_front, 0.0), 1.0e4, behind.open);
    let cut = contour(share);
    let below = max(along * steep, share * MASK_DEPTH * smoothstep(cut, 1.0, mask));
    let column = below / steep;
    let bed = textureLoad(scene_color, pixel, 0).rgb;

    // The water body: fresh water's brown-green, or a lagoon's milky turquoise, over the bed where it is shallow.
    let absorption = mix(lake.fresh_absorption.rgb, lake.salt_absorption.rgb, salt);
    let backscatter = mix(lake.fresh_backscatter.rgb, lake.salt_backscatter.rgb, salt);
    var deep = upwelling(subsurface_reflectance(absorption, backscatter), sun_direction, sun_radiance, daylight, lit);
    // The photograph's own colour of the lake where it covers its middle (a salt pond's red or green, a reservoir's
    // silt): taken as the water's reflectance less the glare it held, lit by this sky. Its texels at the shore hold the
    // bank too, so the colour there stays the water's own. Read two levels coarser than the pixel asks (90 m on a sea
    // window's 23 m maps): the lake's colour, not its photograph's blotches.
    let chart = world.xz - lake.offset.xy;
    let photo_uv = (chart - lake.photo.xy) * lake.photo.z;
    let photo_inside = all(photo_uv > vec2<f32>(0.0)) && all(photo_uv < vec2<f32>(1.0));
    if photo_inside && mask > 0.55 && lake.photo.w > 0.5 {
        let photo_level = max(log2(footprint * lake.photo.z * lake.photo_haze.w), 0.0) + PHOTO_BIAS;
        let photo = textureSampleLevel(photo_map, photo_sampler, photo_uv, 0, photo_level);
        let photo_cover = photo.a * smoothstep(0.55, 1.0, mask) * PHOTO_TRUST;
        let reflectance_photo = max(photo.rgb / max(photo.a, 1e-3) - lake.photo_haze.rgb - PHOTO_GLARE, vec3<f32>(0.0));
        let irradiance = sun_radiance * max(sun_direction.y, 0.0) * lit + daylight;
        deep = mix(deep, reflectance_photo * irradiance * (REFRACTION_SQUARED / PI), photo_cover);
    }
    let body = water_body(bed, deep, column_transmittance(absorption, backscatter, column, view));

    // The sky mirrored about the resolved normal, blurred by the unresolved facets' spread (a slope's deviation turns the
    // mirrored ray twice as far; a lake's mirror is soft, and the sea's visible-facet lobe and three taps cost more than
    // they showed here), the mean Fresnel over those facets, and the sun's glitter.
    let mirrored = reflect(-view, up);
    let spread = sqrt(variances.x + variances.y);
    let roughness = pmrem_roughness(2.0 * spread);
    let reflectance = mean_fresnel(dot(up, view), spread);
    let sky = env_radiance(sky_atlas, sky_sampler, sky_env, normalize(vec3<f32>(mirrored.x, max(mirrored.y, 0.0), mirrored.z)),
        roughness);
    // The glitter only where the facets can turn the sun to the eye: past √40 deviations of slope (e^-20) it is nothing.
    let half_vector = normalize(sun_direction + view);
    let facing = max(dot(half_vector, up), 1e-3);
    let needed = (1.0 - facing * facing) / (facing * facing);
    var glint = vec3<f32>(0.0);
    if needed < GLINT_REACH * (variances.x + variances.y) && sun_direction.y > -0.02 {
        glint = soft_knee(sun_glitter(up, view, sun_direction, sun_radiance, variances, wind), sun_intensity * GLITTER_KNEE);
    }
    var water = mix(body, sky, reflectance) + glint * shadow;

    // A hint of foam where a breeze laps the shore.
#ifdef LAKE_HIGH
    if below < 0.09 && speed > 2.0 {
        let lapping = smoothstep(2.0, 6.0, speed) * smoothstep(0.0, 0.02, below) * (1.0 - smoothstep(0.02, 0.09, below))
            * smoothstep(0.55, 0.8, value_noise(world.xz * 0.45, 913u));
        let foam_light = (daylight * (1.0 / PI) + sun_radiance * max(sun_direction.y, 0.0) * lit / PI) * 0.55;
        water = mix(water, foam_light, lapping * 0.35);
    }
#endif

    if lake.offset.z > 0.5 {
        water = aerial_apply(water, world);
    }
    // The shore: the water thins to nothing where the bed meets the level (the opaque scene shows through, already in
    // its air); where the mask draws the outline the land's depth says nothing, and the water fades in from the mask's
    // contour, which also stops it beyond the raised shore wherever the land behind lies lower than the lake.
    let shore = mix(smoothstep(0.0, SHORE_FADE, along * steep), 1.0, share);
    let edge = smoothstep(cut, cut + MASK_SOFT, mask);
    return vec4<f32>(mix(bed, water, min(shore, edge)), 1.0);
}

// The motion pass for temporal anti-aliasing: the water takes 70 % history, as the sea does (without it its glints
// dance), and nothing where it is cut away.
@fragment
fn motion(input: VertexOut) -> @location(0) vec4<f32> {
    let world = input.world;
    let steep = max(normalize(camera.position - world).y, 0.02);
    let behind = behind_at(vec2<i32>(input.clip.xy), world, input.depth.x);
    if cut_away(mask_at(world), input.depth.y, behind, steep) {
        discard;
    }
    return motion_response(0.3);
}
