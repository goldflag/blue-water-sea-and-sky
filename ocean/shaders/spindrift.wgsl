// Spindrift (native only: the web's sea threw no spray of its own). From about Beaufort 7 the wind tears spray off
// breaking crests and drives it downwind in streaks and sheets that settle back within a couple of seconds; at a full
// gale it hangs over the sea as a low haze (naval_ocean::spray_haze, which the surface and these sprites both take).
//
// No particle lives on the CPU. Every sprite is a slot in a grid of cells laid on the world about the camera, four
// levels of them, each level's cells twice the last's and its sprites twice as large, each level drawn over its own band
// of distance and crossfaded into the next. Each life a slot picks a new point in its cell; where the wave field breaks
// there (the whitecaps' fresh foam, shown through the same breaking groups and crest gates the surface draws them with)
// a wisp of spray rises off the crest and drives downwind with the wind, growing and thinning, and is gone at the end
// of its life. Lit as the splashes' and bow spray's white water is (naval-water-effects' `spray_radiance`: the sky's
// light from above, the sun's or moon's beam through the clouds' shadow and the ships' shadows on the sea, a glow looking
// into it, lightning's flash),
// taking the air between the eye and it as the sea does (the aerial perspective, `naval::aerial`), and faded into the
// water and hulls behind it (soft particles on the backdrop's depth).
//
// A wisp is small beside its distance, so it is lit, hazed and fogged once, at its centre, in the vertex stage; the
// fragment stage only shapes it (the ocean's foam texture, drawn out along the wind) and fades it into what lies behind.
//
// Bind groups: 0 the camera, 1 the spray's own (uniforms, the backdrop's depth, the clouds' shadow, the foam texture),
// 2 the wave field (naval_ocean::waves), 3 the sky's environment.

#import naval::camera::{camera, world_to_clip, curved}
#import naval::aerial::aerial_apply
#import naval::shadow::water_sun_shadow
#import naval_clouds::shadow::{CloudShadow, cloud_shadow_fast}
#import naval_ocean::sky_environment::{SkyEnvironment, env_radiance}
#import naval_ocean::waves::{waves, field_derivatives, field_extras, scatter, copies_level, mix_zero, mix_mean, displacement, breaking_group}
#import naval_ocean::spray_haze::{spray_depth, spray_light}

// `naval_ocean::spindrift::SprayUniforms`.
struct Spray {
    // xy: unit vector the wind blows toward, z: wind speed (m/s), w: the sea's clock (s).
    wind: vec4<f32>,
    // x: the spindrift's strength (0 below a gale's onset, growing with the wind), y: lightning's flash (a multiple of the
    // sky's light), z: the sea's significant height (m), w: the sea's water shadows (the surface's `shadows.x`: 0 off,
    // 1 low, 2 medium, 3 high).
    spray: vec4<f32>,
    // The low haze: x extinction at its base (1/m), y scale height (m), z the base's height (m), w unused.
    haze: vec4<f32>,
    // Per level: xy the corner of its first cell (world m), z the cell (m), w the farthest distance it draws (m).
    levels: array<vec4<f32>, 4>,
    // The sea's distance fog, as the surface's: rgb colour, a power; x start, y end, z sky blend, w 1 with fog. Only w
    // is read: it switches the aerial perspective on.
    fog_color: vec4<f32>,
    fog: vec4<f32>,
}

@group(1) @binding(0) var<uniform> spray: Spray;
// The backdrop's reversed depth (opaque, water): the spray fades where it meets the sea or a hull.
@group(1) @binding(1) var scene_depth: texture_depth_2d;
@group(1) @binding(2) var cloud_map: texture_2d<f32>;
@group(1) @binding(3) var cloud_sampler: sampler;
@group(1) @binding(4) var<uniform> cloud_params: CloudShadow;
// The ocean's foam texture (naval_ocean::foam_texture): lace in red, churned billows in alpha.
@group(1) @binding(5) var foam_map: texture_2d<f32>;
@group(1) @binding(6) var foam_sampler: sampler;

@group(3) @binding(0) var sky_atlas: texture_2d<f32>;
@group(3) @binding(1) var sky_sampler: sampler;
@group(3) @binding(2) var<uniform> sky_env: SkyEnvironment;

// Cells per level edge, sprites per cell, levels: `naval_ocean::spindrift`'s `GRID`, `SLOTS`, `LEVELS`.
const GRID: u32 = 48u;
const SLOTS: u32 = 3u;
// The finest level's cell (m): sprites there have their base size.
const FINEST_CELL: f32 = 2.5;
// A wisp's mean life (s), and the share of it each life varies.
const LIFE: f32 = 1.8;
// Spray drives downwind at this share of the wind speed: spume drops a metre or two up lag the 10 m wind.
const DRIFT_SHARE: f32 = 0.5;
// How high a wisp is lifted off the crest (m), plus this share of the significant height, and how fast (s).
const LIFT: f32 = 0.5;
const LIFT_SEA: f32 = 0.08;
const LIFT_TIME: f32 = 0.3;
// A wisp's height at birth and at the end of its life (m, the finest level's), its length over its height, and the
// seconds of its drift it is smeared along (a sheet streaming off the crest).
const WIDTH_BORN: f32 = 0.5;
const WIDTH_GONE: f32 = 1.8;
const STRETCH: f32 = 2.0;
const STREAK_TIME: f32 = 0.35;
// Each level's wisps are this power of its cell's ratio to the finest larger: fewer and larger with distance, thinning
// into the haze.
const LEVEL_GROWTH: f32 = 0.75;
// Fresh foam over which the crest throws spray, from none to all it can.
const SOURCE_FROM: f32 = 0.12;
const SOURCE_FULL: f32 = 0.55;
// A coarser cascade's compression over which a finer cascade's crests show (the surface's `GATE_NONE`, `GATE_FULL`).
const GATE_NONE: f32 = -1.0;
const GATE_FULL: f32 = 0.75;
// Opacity of a wisp's core at full strength.
const OPACITY: f32 = 0.42;
// Metres over which a wisp fades into what lies behind it, per metre of its width.
const SOFT_EDGE: f32 = 0.6;
// The sky's light on spray as naval-water-effects' `spray.wgsl` scales it: `SKY_ALBEDO`, `SKY_GAIN`, and the direct
// radiance it scatters, 1.25 at a clear noon's light of 5.8 (`EffectLighting::set_illumination`).
const SKY_ALBEDO: f32 = 0.677;
const SKY_GAIN: f32 = 2.2;
const DIRECT_SCALE: f32 = 1.25 / 5.8;

// Three uniform hashes of an integer cell, a slot and a life (PCG3D, Jarzynski & Olano 2020).
fn spray_hash(cell: vec2<i32>, slot: u32, life: u32) -> vec3<f32> {
    var v = vec3<u32>(bitcast<u32>(cell.x), bitcast<u32>(cell.y), slot * 0x9e3779b9u ^ life);
    v = v * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> vec3<u32>(16u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return vec3<f32>(v >> vec3<u32>(8u)) * (1.0 / 16777216.0);
}

// How freshly the sea breaks at grid point `xz`, read over about `footprint` metres: each cascade's young foam where its
// groups show it and the coarser cascades' crests carry it, as the surface draws its whitecaps' cores.
fn breaking_at(xz: vec2<f32>, footprint: f32) -> f32 {
    var strain = 0.0;
    var fresh = 0.0;
    for (var i = 0; i < i32(waves.info.y); i++) {
        let copies = scatter(xz, i);
        let level = clamp(log2(footprint * waves.texels[i]), 0.0, waves.info.x);
        let derivatives = mix_zero(copies_level(field_derivatives, copies, i, level), copies);
        let extras = mix_mean(copies_level(field_extras, copies, i, level), copies);
        let crests = select(1.0, smoothstep(GATE_NONE, GATE_FULL, -strain * waves.gates[i]), i > 0);
        fresh = max(fresh, extras.w * crests * breaking_group(xz, extras.y, extras.w, i));
        strain += derivatives.z + derivatives.w;
    }
    return fresh;
}

struct SprayVertex {
    @builtin(position) clip: vec4<f32>,
    // x along the wisp (downwind), y across it, 0–1.
    @location(0) uv: vec2<f32>,
    // Per wisp: its radiance (lit, hazed and fogged at its centre), opacity, how thin it has grown (0–1), the metres
    // over which it fades into what lies behind, and where its shape is read in the foam texture.
    @location(1) @interpolate(flat) color: vec3<f32>,
    @location(2) @interpolate(flat) opacity: f32,
    @location(3) @interpolate(flat) thin: f32,
    @location(4) @interpolate(flat) soft: f32,
    @location(5) @interpolate(flat) seed: vec2<f32>,
    // The fragment's depth in view space (m, negative ahead).
    @location(6) view_z: f32,
}

fn culled() -> SprayVertex {
    var out: SprayVertex;
    out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    out.uv = vec2<f32>(0.0);
    out.color = vec3<f32>(0.0);
    out.opacity = 0.0;
    out.thin = 0.0;
    out.soft = 1.0;
    out.seed = vec2<f32>(0.0);
    out.view_z = 0.0;
    return out;
}

// The air between the eye and a wisp, as the surface's `apply_fog` takes it: the aerial perspective (the atmosphere and
// the weather's haze, `naval::aerial`), read once at its centre in the vertex stage.
fn spray_fog(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    if spray.fog.w < 0.5 {
        return color;
    }
    return aerial_apply(color, world);
}

// A wisp's radiance at `centre`, `thin` (0 dense … 1 a veil): lit as white spray (naval-water-effects' `spray_radiance`:
// the sky from above filling its shade, the sun's or moon's beam through the clouds and past the ships' shadows, the
// glow of thin spray seen toward it, lightning's flash), then the haze between the eye and it and the sea's fog.
fn wisp_radiance(centre: vec3<f32>, thin: f32) -> vec3<f32> {
    let to_camera = normalize(camera.position - centre);
    let normal = normalize(to_camera + vec3<f32>(0.0, 0.6, 0.0));
    let sun = sky_env.light_direction;
    let sky_mean = env_radiance(sky_atlas, sky_sampler, sky_env, vec3<f32>(0.0, 1.0, 0.0), 1.0);
    let fill = sky_mean * SKY_ALBEDO * (1.0 + spray.spray.y) * SKY_GAIN * (thin * 0.4 + 0.6) * (0.82 + normal.y * 0.18);
    // The ships' (and splashes') shadows as the sea beneath the wisp takes them (the surface's `water_sun_shadow`, faint
    // under the moon), so spray torn off a crest in a hull's shadow stays in it.
    let ships = mix(1.0, water_sun_shadow(centre, u32(spray.spray.w)), clamp(sky_env.light_intensity / 2.0, 0.0, 1.0));
    let sunlit = cloud_shadow_fast(cloud_map, cloud_sampler, cloud_params, centre) * smoothstep(-0.1, 0.05, sun.y) * ships;
    let beam = sky_env.light_color * sky_env.light_intensity * sunlit;
    let diffuse = clamp((dot(normal, sun) + 0.5) / 1.5, 0.0, 1.0);
    let toward = clamp(dot(-to_camera, sun), 0.0, 1.0);
    let glow = pow(toward, 4.0) * (thin * thin * 1.6 + 0.08);
    let lit = fill + beam * DIRECT_SCALE * (diffuse + glow);
    let veil = exp(-spray_depth(camera.position, centre, spray.haze));
    return spray_fog(lit * veil + spray_light(sky_mean, beam, to_camera, sun) * (1.0 - veil), centre);
}

@vertex
fn spindrift_vertex(@builtin(vertex_index) corner_index: u32, @builtin(instance_index) instance: u32) -> SprayVertex {
    let strength = spray.spray.x;
    let per_level = GRID * GRID * SLOTS;
    let level = min(instance / per_level, 3u);
    let rest = instance % per_level;
    let slot = rest % SLOTS;
    let cell_index = rest / SLOTS;
    let frame = spray.levels[level];
    let cell = frame.z;
    let corner = frame.xy + vec2<f32>(f32(cell_index % GRID), f32(cell_index / GRID)) * cell;
    // The band this level draws, crossfaded with its neighbours' over the inner fifth of each edge.
    let distance = length(vec3<f32>(corner.x + 0.5 * cell, 0.0, corner.y + 0.5 * cell) - camera.position);
    let inner = select(spray.levels[max(level, 1u) - 1u].w, 0.0, level == 0u);
    let outer = frame.w;
    let band = select(smoothstep(inner * 0.8, inner, distance), 1.0, level == 0u)
        * (1.0 - smoothstep(outer * 0.8, outer, distance));
    if strength <= 0.0 || band <= 0.0 {
        return culled();
    }
    // Cells off screen draw nothing (their wisps reach a few cells downwind and up).
    let scale = pow(cell / FINEST_CELL, LEVEL_GROWTH);
    let drift = spray.wind.z * DRIFT_SHARE;
    let reach = cell * 2.0 + (WIDTH_GONE * STRETCH + drift * (LIFE * 1.3 + STREAK_TIME * 1.6)) * scale;
    let probe = world_to_clip(vec3<f32>(corner.x + 0.5 * cell, 1.0, corner.y + 0.5 * cell));
    // The reach in clip units on the narrower (vertical) axis: conservative across.
    let slack = reach / camera.tan_half_fov_y;
    if probe.w < -reach || any(abs(probe.xy) > vec2<f32>(probe.w + slack)) {
        return culled();
    }
    // This slot's current life: a fresh point in the cell each time.
    let id = vec2<i32>(floor(corner / cell + 0.5));
    let key = spray_hash(id, slot + level * SLOTS, 0u);
    let life = LIFE * (0.7 + 0.6 * key.x);
    let cycles = spray.wind.w / life + key.y;
    let lived = floor(cycles);
    let t = cycles - lived;
    let h = spray_hash(id, slot + level * SLOTS, u32(i32(lived)) + 1u);
    let birth = corner + h.xy * cell;
    let source = smoothstep(SOURCE_FROM, SOURCE_FULL, breaking_at(birth, cell * 0.35));
    let envelope = smoothstep(0.0, 0.08, t) * pow(1.0 - t, 1.3);
    let opacity = OPACITY * min(strength, 1.0) * source * envelope * band;
    if opacity < 1e-3 {
        return culled();
    }
    // Off the crest it was torn from, downwind with the wind and a little across it, lifted and settling back.
    let age = t * life;
    let wind = spray.wind.xy;
    let gust = 0.75 + 0.5 * h.z;
    let crest = displacement(birth, cell * 0.5);
    let carried = birth + crest.xz + wind * (drift * gust * age)
        + vec2<f32>(-wind.y, wind.x) * ((h.z - 0.5) * cell * 0.4);
    let lift = (LIFT + LIFT_SEA * spray.spray.z) * (1.0 - exp(-age / LIFT_TIME)) * (1.0 - 0.6 * t);
    let width = mix(WIDTH_BORN, WIDTH_GONE, sqrt(t)) * scale * (0.8 + 0.4 * key.z) * mix(1.0, 1.6, saturate(strength - 1.0));
    let below = displacement(carried, cell * 0.5).y;
    let centre = vec3<f32>(carried.x, max(crest.y + lift, below + width * 0.35), carried.y);
    // Drawn out along the wind, turned to the camera; seen end-on it shortens toward round.
    let to_eye = normalize(camera.position - centre);
    let wind3 = vec3<f32>(wind.x, 0.0, wind.y);
    let flat = wind3 - to_eye * dot(wind3, to_eye);
    let shown = length(flat);
    let along = select(normalize(cross(to_eye, vec3<f32>(0.0, 1.0, 0.0)) + vec3<f32>(1e-5, 0.0, 0.0)), flat / max(shown, 1e-5),
        shown > 1e-3);
    let across = normalize(cross(to_eye, along));
    let streak = width * STRETCH * (0.8 + 0.4 * h.y) + drift * gust * STREAK_TIME * scale;
    let length_m = mix(width, streak, saturate(shown));
    let uv = vec2<f32>(f32(corner_index & 1u), f32(corner_index >> 1u));
    let world = centre + along * ((uv.x - 0.5) * length_m) + across * ((uv.y - 0.5) * width);
    var out: SprayVertex;
    out.clip = world_to_clip(world);
    out.uv = uv;
    out.color = wisp_radiance(centre, 0.3 + 0.6 * t);
    out.opacity = opacity;
    out.thin = t;
    out.soft = max(width * SOFT_EDGE, 0.2);
    out.seed = h.xy;
    out.view_z = (camera.view * vec4<f32>(curved(world), 1.0)).z;
    return out;
}

@fragment
fn spindrift_fragment(input: SprayVertex) -> @location(0) vec4<f32> {
    // A wisp: a sheet densest at its head off the crest, torn along the wind into streaks of the foam's lace that thin
    // out behind it.
    let across = (input.uv.y - 0.5) / 0.26;
    let core = exp(-across * across);
    let ends = smoothstep(0.0, 0.25, input.uv.x) * (1.0 - smoothstep(0.35, 1.0, input.uv.x));
    let shape = textureSample(foam_map, foam_sampler, input.seed + vec2<f32>(input.uv.x * 0.3, input.uv.y * 1.1));
    let torn = smoothstep(0.25, 0.8, shape.r * 0.65 + shape.a * 0.35 + 0.2 * (1.0 - input.uv.x) - 0.15 * input.thin);
    // Soft where it meets the water or a hull behind it.
    let depth = textureLoad(scene_depth, vec2<i32>(input.clip.xy), 0);
    let near = camera.near;
    let far = camera.far;
    let behind = -near * far / (depth * (far - near) + near);
    let soft = saturate((input.view_z - behind) / input.soft);
    return vec4<f32>(input.color, input.opacity * core * ends * torn * soft);
}
