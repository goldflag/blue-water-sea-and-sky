// Rain landing on the sea near the camera, ported from the web game's `sky/weather/splashes.ts` (`SplashField.build`):
// short-lived ripple rings and crowns scattered afresh every life over a square of sea that wraps around the camera.
// The web sets them on the drawn waves (`seaHeight`); here they stand on the ocean's heights about the camera
// (`naval_ocean::rain_heights`, a square of `sea_span` metres repeating over the world), or on the mean sea without
// them. Premultiplied alpha.

#import naval_sky::atmosphere::{sky_radiance}
#import naval_sky::common::{sky}

struct SplashUniforms {
    view: mat4x4<f32>,
    projection: mat4x4<f32>,
    // Per class: its clock in lifetimes (wrapped with the 60 s clock).
    cycles: vec4<f32>,
    // Region phase (x, z) in region units and the region centre relative to the camera (x, z).
    region: vec4<f32>,
    camera_xz: vec2<f32>,
    camera_height: f32,
    opacity: f32,
    size: f32,
    direct_share: f32,
    grey: f32,
    // The span (m) of the sea's heights about the camera, 0 for the flat mean sea.
    sea_span: f32,
}

@group(2) @binding(0) var<uniform> splashes: SplashUniforms;
@group(2) @binding(1) var sea_heights: texture_2d<f32>;
@group(2) @binding(2) var sea_sampler: sampler;

const REGION: f32 = 64.0;
const FAR_FROM: f32 = 100.0;
const FAR_TO: f32 = 140.0;
const RING: f32 = 0.45;
const CROWN_HEIGHT: f32 = 0.26;
const CROWN_WIDTH: f32 = 0.12;
const CROWN_LIFE: f32 = 0.25;
const LIFT: f32 = 0.2;
const RING_OPACITY: f32 = 0.5;
const CROWN_OPACITY: f32 = 0.9;
const RING_LIGHT: f32 = 1.15;
const CROWN_LIGHT: f32 = 1.9;
const CROWN_GLINT: f32 = 0.15;
const SKY_ELEVATION: f32 = 0.15;

fn ramp(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// three's TSL `hash` (PCG).
fn pcg(seed: f32) -> f32 {
    let state = u32(seed) * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return f32((word >> 22u) ^ word) * (1.0 / 4294967296.0);
}

fn sky_around(view: vec3<f32>, grey: f32) -> vec3<f32> {
    let color = sky_radiance(normalize(vec3<f32>(view.x, max(view.y, SKY_ELEVATION), view.z)), false);
    return mix(color, vec3<f32>(dot(color, vec3<f32>(0.2126, 0.7152, 0.0722))), grey);
}

struct SplashVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) part: f32,
    @location(2) age: f32,
    @location(3) fade: f32,
    @location(4) water: vec3<f32>,
    @location(5) spray: vec3<f32>,
}

@vertex
fn splash_vertex(@builtin(vertex_index) index: u32, @location(0) splash: vec4<f32>) -> SplashVertex {
    // Corners (x, y, part): part 0 is the ring lying on the water, part 1 the crown standing on it.
    var corners = array<vec3<f32>, 8>(
        vec3<f32>(-1.0, -1.0, 0.0), vec3<f32>(1.0, -1.0, 0.0), vec3<f32>(-1.0, 1.0, 0.0), vec3<f32>(1.0, 1.0, 0.0),
        vec3<f32>(-1.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 1.0), vec3<f32>(-1.0, 1.0, 1.0), vec3<f32>(1.0, 1.0, 1.0));
    var order = array<u32, 12>(0u, 1u, 2u, 2u, 1u, 3u, 4u, 5u, 6u, 6u, 5u, 7u);
    var lives = array<f32, 4>(120.0, 100.0, 80.0, 60.0);
    let corner = corners[order[index % 12u]];
    let k = i32(splash.w);
    let seed = fract(splash.w);
    let cycle = splashes.cycles[k] + splash.z;
    let progress = fract(cycle);
    // Every life lands somewhere new, counted within the clock's cycle so a splash keeps its place at the wrap.
    let life = floor(cycle) % lives[k];
    let key = life * 7919.0 + floor(seed * 65536.0);
    let scatter = vec2<f32>(pcg(key), pcg(key + 104729.0));
    let q = fract(splash.xy + scatter + splashes.region.xy) - 0.5;
    let edge = max(abs(q.x), abs(q.y));
    let xz = splashes.region.zw + q * REGION;
    // The drawn sea under the splash, where the ocean gives its heights.
    var sea = 0.0;
    if splashes.sea_span > 0.0 {
        sea = textureSampleLevel(sea_heights, sea_sampler, (splashes.camera_xz + xz) / splashes.sea_span, 0.0).r;
    }
    let base = vec3<f32>(xz.x, sea + LIFT - splashes.camera_height, xz.y);
    let crown = corner.z > 0.5;
    let radius = (sqrt(progress) * 0.85 + 0.15) * RING * splashes.size;
    let view = normalize(base);
    let side = normalize(vec3<f32>(-view.z, 0.0, view.x) + vec3<f32>(1e-5, 0.0, 0.0));
    let rise = sin(min(progress / CROWN_LIFE, 1.0) * 3.141592653589793) * CROWN_HEIGHT * splashes.size;
    let ring = base + vec3<f32>(corner.x * radius, 0.0, corner.y * radius);
    let standing = base + side * (corner.x * CROWN_WIDTH * splashes.size) + vec3<f32>(0.0, corner.y * rise, 0.0);
    let position = select(ring, standing, crown);
    var out: SplashVertex;
    out.local = corner.xy;
    out.part = corner.z;
    out.age = progress;
    out.fade = (1.0 - ramp(0.36, 0.5, edge)) * (1.0 - ramp(FAR_FROM, FAR_TO, length(base))) * splashes.opacity;
    // Rings mirror the sky at a grazing angle; crowns are white water lit by it and by any direct light.
    let around = sky_around(view, splashes.grey) * (sky.flash + 1.0);
    out.water = around * RING_LIGHT;
    out.spray = around * CROWN_LIGHT + sky.light_color * splashes.direct_share * CROWN_GLINT;
    out.position = splashes.projection * vec4<f32>((splashes.view * vec4<f32>(position, 0.0)).xyz, 1.0);
    return out;
}

@fragment
fn splash_fragment(in: SplashVertex) -> @location(0) vec4<f32> {
    let ring_off = (length(in.local) - 0.8) / 0.14;
    let ring_shape = exp(-(ring_off * ring_off)) * (1.0 - in.age) * (1.0 - in.age) * RING_OPACITY;
    let crown_age = in.age / CROWN_LIFE;
    let across = 1.0 - in.local.x * in.local.x;
    let crown_shape = across * across * ramp(0.0, 0.15, in.local.y) * sqrt(max(1.0 - in.local.y, 0.0))
        * (1.0 - ramp(0.6, 1.0, crown_age)) * select(0.0, CROWN_OPACITY, crown_age < 1.0);
    let is_crown = in.part > 0.5;
    let alpha = select(ring_shape, crown_shape, is_crown) * in.fade;
    return vec4<f32>(select(in.water, in.spray, is_crown) * alpha, alpha);
}
