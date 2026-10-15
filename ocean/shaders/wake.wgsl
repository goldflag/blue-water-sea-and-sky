// What the ocean surface reads of the wakes (the web's `WakeSampler` as `ShipWake.ts` binds it with the
// realistic trail): the dispersive wake field's height, slope and breaking foam (the web game's `ocean/wake/WakeField.ts`,
// `createSampler`), the analytic bow waves (the web game's `BowWaves.ts`, `evaluate`), the fleet's trail foam, bubbles and slick
// (the web game's `FleetWakeFoam.ts`, `evaluate`), the torpedo tracks (`TorpedoTrackFoam.ts`, `sample`) and the hull
// contact foam (the web game's `HullContactFoam.ts`, `evaluate`).
//
// Its bindings join the surface's own group 1 (bindings 5–10): the surface already uses the four bind groups WebGPU
// guarantees (camera, surface, waves, sky). One uniform buffer packs every system's arrays (the removed web game's
// `PackedVec4Arrays` layout, which kept them under its per-stage uniform buffer limit).

#define_import_path naval_ocean::wake

#import naval::camera::camera
#import naval_ocean::mx_noise::mx_noise_float

const TILES: f32 = 8.0;
const TILE_COUNT: i32 = 64;
const WAKE_EXTENT: f32 = 1536.0;
const SLICK_EXTENT: f32 = 4096.0;
const EDGE_FADE: f32 = 0.03;
const BOW_SLOTS: i32 = 8;
const TAU: f32 = 6.283185307179586;
// Hull profiles (hull_profile.rs) and the contact foam's per-slot layout (hull_foam.rs).
const PROFILE_STATIONS: i32 = 32;
const PROFILE_LEVELS: i32 = 7;
const PROFILE_LEVEL_STEP: f32 = 2.0;
const PROFILE_LOWEST: f32 = -6.0;
const HULL_PROFILE: i32 = 7;
const HULL_STRIDE: i32 = 263;

struct Wake {
    // The wake field's display textures: origin of display 0 (xy) and 1 (zw); weights, world size; w: how fast the
    // water mesh's cells grow with distance from its centre (m per m).
    field_origins: vec4<f32>,
    field: vec4<f32>,
    // x: trail tiles in use, y: the trail's clock, z: the torpedo field's clock, w: 1 when torpedo foam is painted.
    trail: vec4<f32>,
    // Every trail tile's painted extent together (minX, minZ, maxX, maxZ).
    trail_reach: vec4<f32>,
    // xy: the torpedo field's centre, zw: the water mesh's centre (the eye, or what a magnified view is aimed at).
    torpedo: vec4<f32>,
    // The trail's shading knobs (trail.rs `WakeShading`); [3].w: 1 while the water mesh is a magnified view's fan.
    shading: array<vec4<f32>, 4>,
    // x: bow wave slots in use, y: their clock, z: radians per pixel, w: the cell of the water mesh's finest level
    // drawn; under a magnified view's fan, how fast its far rows' cells grow with the square of their range (1 / m).
    bow: vec4<f32>,
    bow_knobs: array<vec4<f32>, 4>,
    // x: contact foam slots in use, y: its clock, z: radians per pixel, w: the bow's extra foam length share.
    hull: vec4<f32>,
    // Trail tiles, section-major: centre and atlas tile, slick centre, churned-water box, slick box, both.
    tiles: array<vec4<f32>, 320>,
    // Bow wave slots, section-major: pose, wave, hull, bend, entrance, course.
    bow_slots: array<vec4<f32>, 48>,
    hull_data: array<vec4<f32>, 2104>,
}

@group(1) @binding(5) var<uniform> wake: Wake;

// Half the edge of the water mesh's outermost level, where its horizon ring takes over (`geometry::CLIPMAP_REACH`).
const MESH_REACH: f32 = 4096.0;

// The water mesh's vertex spacing at a world position (m), as `surface.wgsl`'s vertex stage has it: the cell of the
// level that holds the point (each level's half edge half the next's, its cell `wake.field.w` of it), morphed onto the
// next level's toward its edge; never finer than the finest level drawn; on the horizon ring beyond the lattice,
// `geometry.rs`'s 4 r / segments. A magnified view's fan (`geometry::Fan::spacing`, `wake.shading[3].w` set) has
// cells `wake.field.w` of their range from its centre, the eye, or `wake.bow.w` of its square where that is more. No
// TypeScript source.
fn mesh_spacing(xz: vec2<f32>) -> f32 {
    let off = xz - wake.torpedo.zw;
    if wake.shading[3].w > 0.5 {
        let range = length(off);
        return range * max(wake.field.w, wake.bow.w * range);
    }
    let reach = max(abs(off.x), abs(off.y));
    if reach >= MESH_REACH {
        return max(wake.bow.w, 2.0 * wake.field.w * length(off));
    }
    let half = MESH_REACH * exp2(-floor(log2(MESH_REACH / max(reach, 1e-3))));
    return max(wake.bow.w, wake.field.w * half * (1.0 + smoothstep(0.6, 0.85, reach / half)));
}
@group(1) @binding(6) var wake_display_a: texture_2d<f32>;
@group(1) @binding(7) var wake_display_b: texture_2d<f32>;
@group(1) @binding(8) var wake_sampler: sampler;
@group(1) @binding(9) var trail_atlas: texture_2d<f32>;
@group(1) @binding(10) var torpedo_field: texture_2d<f32>;

// ---- The wake field -------------------------------------------------------------------------------------------------

fn edge_fade(uv: vec2<f32>) -> f32 {
    return smoothstep(0.0, EDGE_FADE, uv.x) * smoothstep(0.0, EDGE_FADE, 1.0 - uv.x) * smoothstep(0.0, EDGE_FADE, uv.y)
        * smoothstep(0.0, EDGE_FADE, 1.0 - uv.y);
}

// Both displays at a world point, blended by the step fraction and the gain; calm outside the field.
// (h, ∂h/∂x, ∂h/∂z, foam).
fn wake_field(xz: vec2<f32>) -> vec4<f32> {
    let size = max(wake.field.z, 1.0);
    let uv_a = (xz - wake.field_origins.xy) / size + 0.5;
    let uv_b = (xz - wake.field_origins.zw) / size + 0.5;
    let a = textureSampleLevel(wake_display_a, wake_sampler, uv_a, 0.0) * edge_fade(uv_a);
    let b = textureSampleLevel(wake_display_b, wake_sampler, uv_b, 0.0) * edge_fade(uv_b);
    return a * wake.field.x + b * wake.field.y;
}

// ---- Bow waves ------------------------------------------------------------------------------------------------------

struct BowWaveSample {
    height: f32,
    slope: vec2<f32>,
    foam: f32,
    bubbles: f32,
    // How young the white water is: 1 where the crest breaks, less where it only left foam behind.
    young: f32,
    // The slope variance of the V's crests too short for the pixel to draw: from aloft and at range an arm still reads
    // as a line of rougher water after its crests have gone below the pixel.
    variance: f32,
}

// The entrance the crest follows: this share of the hull's length aft of the stem (`bow_waves.rs`, `ENTRANCE`).
const BOW_ENTRANCE: f32 = 0.3;
// The crest's climb starts this share of its peak's distance ahead of the stem (`CLIMB_LEAD`); the water standing
// against the bow's sides (`HEAP_REACH`, `HEAP_WIDTH`, `HEAP_SPREAD`).
const CLIMB_LEAD: f32 = 0.15;
const HEAP_REACH: f32 = 0.07;
const HEAP_WIDTH: f32 = 0.2;
const HEAP_SPREAD: f32 = 0.12;
// The crest's inboard side is this many times as broad as its outer face: a breaking front outboard, the water it has
// passed falling away gently toward the hull.
const CREST_INNER: f32 = 2.2;
// The trough behind the crest: this share of its height, centred this many of the outer face's widths inboard of the
// crest line (`bow_waves.rs` takes what it leaves of the crest on its own line).
const CREST_TROUGH: f32 = 0.25;
const CREST_TROUGH_INBOARD: f32 = 3.2;
// What the crest's breaker leaves on the water it has passed lasts this share of the hull's length downstream, and the
// white water the surface draws from it is this young (lace, not a breaker's dense white).
const BOW_RESIDUE_LENGTH: f32 = 0.16;
const BOW_RESIDUE: f32 = 0.75;
const BOW_RESIDUE_YOUNG: f32 = 0.7;
// How hard the streaks tear the white water: a breaker's dense core least, what it left behind most.
const BOW_TEAR: f32 = 0.45;
const BOW_LACE_TEAR: f32 = 1.0;
// The Kelvin V's cusp line, where its two wave systems meet at 1/√8 (19.47°) off the track. How far inside it a point
// lies is the Airy function's argument, CUSP_ZETA (k₀·run)^(2/3) (1/√8 − t), t the point's offset over its run (the
// systems' phase difference, linearised about the cusp): outside the line no wave stands and the sea settles within a
// few metres at any range; inside it the first fringe, the line itself, keeps its full slope (the Airy function peaks
// at 1.02 and first falls to nothing at 2.34) and the fringes within it `kelvin_interior` of theirs. From aloft the V
// reads as two thin lines of short crests in echelon, a band some 20 m wide 300 m astern of a hull at 15 m/s and 40 m
// wide 2 km astern, not a fan of ripples (`bow_waves.rs`, `kelvin_interior`).
const CUSP_T: f32 = 0.35355339;
const CUSP_ZETA: f32 = 1.9645;
const CUSP_LINE: f32 = 1.0;
const CUSP_LINE_WIDTH: f32 = 1.2;
// The line's slopes a little steeper than the V's elsewhere: its crests are where the two systems pile up.
const CUSP_LINE_GAIN: f32 = 1.15;
// The crests' slope variance the pixel cannot draw, as a share of what it would shade, and how patchy it is along the
// arm (the sea's own waves break the line up): a far arm reads as a line of rougher water, not a stroke of paint.
const KELVIN_ROUGHNESS: f32 = 0.12;
// Past a few hull lengths astern only the cusp lines are drawn: the water between them, quiet there and low, fades out
// over these Airy arguments inside the lines, from two to four hull lengths astern. A long V then costs two narrow bands
// of sea, not the whole fan.
const CUSP_INTERIOR_FROM: f32 = 6.0;
const CUSP_INTERIOR_TO: f32 = 9.0;
// The V's crests are drawn down to this share of the five pixels the crest needs (`finest`): they stand still in a view
// that follows the hull, and a far arm keeps its dashes.
const KELVIN_FINEST: f32 = 0.6;
const KELVIN_ROUGHNESS_PATCH: f32 = 180.0;

// The waterline's half-breadth `along` metres astern of the stem and how fast it grows (m per m): through the hull's own
// breadths an eighth, a quarter, a half and the whole of the `entrance` aft of it (`bow_waves.rs`, `entrance`), then
// out to her `full` half-breadth. No TypeScript source: `BowWaves.ts` assumed a parabola.
fn entrance_breadth(along: f32, entrance: f32, nodes: vec4<f32>, full: f32) -> vec2<f32> {
    let u = along / entrance;
    if u < 0.125 {
        return vec2<f32>(nodes.x * u * 8.0, nodes.x * 8.0 / entrance);
    }
    if u < 0.25 {
        return vec2<f32>(mix(nodes.x, nodes.y, (u - 0.125) * 8.0), (nodes.y - nodes.x) * 8.0 / entrance);
    }
    if u < 0.5 {
        return vec2<f32>(mix(nodes.y, nodes.z, (u - 0.25) * 4.0), (nodes.z - nodes.y) * 4.0 / entrance);
    }
    if u < 1.0 {
        return vec2<f32>(mix(nodes.z, nodes.w, (u - 0.5) * 2.0), (nodes.w - nodes.z) * 2.0 / entrance);
    }
    // Aft of the entrance she fills out to her full breadth by 0.6 of her length.
    let t = clamp(u - 1.0, 0.0, 1.0);
    let more = max(full, nodes.w) - nodes.w;
    return vec2<f32>(nodes.w + more * t * t * (3.0 - 2.0 * t), more * 6.0 * t * (1.0 - t) / entrance);
}

// Every hull in reach. The vertex stage takes only the height; the fragment stage the slope, breaking white water
// and the aerated water underneath it. `BowWaves.evaluate`.
fn bow_waves(x: f32, z: f32, vertex_stage: bool) -> BowWaveSample {
    var height = 0.0;
    var slope_x = 0.0;
    var slope_z = 0.0;
    var foam = 0.0;
    var bubbles = 0.0;
    var young = 1.0;
    var variance = 0.0;
    let crest_angle = wake.bow_knobs[0].x;
    let crest_width = wake.bow_knobs[0].y;
    let hump_share = wake.bow_knobs[0].z;
    let transverse = wake.bow_knobs[0].w;
    let divergent_gain = wake.bow_knobs[1].x;
    let kelvin_interior = wake.bow_knobs[1].y;
    let foam_gain = wake.bow_knobs[1].z;
    let foam_length = wake.bow_knobs[1].w;
    let foam_spread = wake.bow_knobs[2].x;
    let wash_gain = wake.bow_knobs[2].y;
    let wash_width = wake.bow_knobs[2].z;
    let kelvin_foam = wake.bow_knobs[2].w;
    let kelvin_steep = wake.bow_knobs[3].x;
    let kelvin_foam_length = wake.bow_knobs[3].y;
    let breakup = wake.bow_knobs[3].z;
    let vertex_feature = wake.bow_knobs[3].w;
    let eye = camera.position.xyz;
    let range = max(length(vec3<f32>(eye.x - x, eye.y, eye.z - z)), 1.0);
    // The mesh coarsens with distance from its own centre, which leaves the eye in a magnified view.
    let mesh = wake.torpedo.zw;
    let mesh_range = max(length(vec3<f32>(mesh.x - x, eye.y, mesh.y - z)), 1.0);
    // The shortest feature each stage can hold without shimmering: a few vertices of the water mesh, or five pixels
    // stretched by the grazing view.
    // A magnified view's fan (`mesh_spacing`): its cells grow with their range along the sea.
    let fan_range = length(mesh - vec2<f32>(x, z));
    let cell = select(wake.bow.w + mesh_range * wake.field.w,
        fan_range * max(wake.field.w, wake.bow.w * fan_range), wake.shading[3].w > 0.5);
    let finest = select(range * wake.bow.z / max(abs(eye.y) / range, 0.04) * 5.0, cell * vertex_feature, vertex_stage);
    let count = i32(wake.bow.x);
    for (var i = 0; i < count; i++) {
        let pose = wake.bow_slots[i];
        let wave = wake.bow_slots[BOW_SLOTS + i];
        let hull = wake.bow_slots[2 * BOW_SLOTS + i];
        let bend = wake.bow_slots[3 * BOW_SLOTS + i].x;
        // The V's slopes are shaded at its own amplitude (`kelvin_shading`), not the drawn height's; its white water
        // breaks from the drawn wave's steepness.
        let shading_share = wake.bow_slots[3 * BOW_SLOTS + i].y / max(wave.y, 1e-6);
        let dx = x - pose.x;
        let dz = z - pose.y;
        let hull_length = hull.x;
        let beam = hull.y;
        // How far astern the V is drawn: as far as the arc it is laid on still follows her track (`bow_waves.rs`).
        let reach = max(wake.bow_slots[3 * BOW_SLOTS + i].w, hull_length);
        // Nothing of hers reaches past the V's far corners.
        let span = reach * 1.42 + beam * 1.5;
        if dx * dx + dz * dz > span * span {
            continue;
        }
        // Ship frame: `a` metres astern of the stem, `y` to starboard.
        let a = dz * pose.w - dx * pose.z;
        let y = dx * pose.w + dz * pose.z;
        let ay = abs(y);
        let side = sign(y);
        var slope_a = 0.0;
        var slope_y = 0.0;
        var white = 0.0;
        var aerated = 0.0;
        var fresh = 1.0;
        let along = max(a, 0.0);
        // How far aft of the stem the crest stands highest against the side.
        let peak_at = max(wake.bow_slots[3 * BOW_SLOTS + i].z, 0.5);
        // The crest, the water against the bow's sides and the hull-side wash live close to the hull; the V covers far
        // more water. Nothing is raised or broken ahead of the stem: the sea meets a fine bow undisturbed.
        if a > -CLIMB_LEAD * peak_at && a < hull_length * 0.8 && ay < beam * 0.9 + along * 0.3 {
            let crest_height = wave.z;
            let crest_length = wave.w;
            // The hull's own waterline along her entrance.
            let breadth = entrance_breadth(along, hull_length * BOW_ENTRANCE, wake.bow_slots[4 * BOW_SLOTS + i], beam * 0.5);
            let half_beam = breadth.x;
            let half_beam_rate = breadth.y;
            // The crest climbs the side from the stem, stands highest `peak_at` aft of it, then leaves the side at the
            // crest angle and decays: a steep outer face where it breaks, a gentle fall toward the hull behind it and
            // a trough inboard of that.
            let offset = ay - (half_beam + along * crest_angle);
            let width = beam * crest_width + 0.8 + along * 0.05;
            let blurred = max(width, finest * select(0.2, 0.4, vertex_stage));
            let rise = clamp((a + CLIMB_LEAD * peak_at) / ((1.0 + CLIMB_LEAD) * peak_at), 0.0, 1.0);
            let climb = rise * rise * (3.0 - 2.0 * rise);
            let climb_rate = 6.0 * rise * (1.0 - rise) / ((1.0 + CLIMB_LEAD) * peak_at);
            let fall = exp(-max(along - peak_at, 0.0) / crest_length);
            let fall_rate = -fall * step(peak_at, along) / crest_length;
            let scale = crest_height * (width / blurred);
            let crest = scale * climb * fall;
            let face = select(blurred * CREST_INNER, blurred, offset > 0.0);
            let across = offset / face;
            let inboard = (offset + blurred * CREST_TROUGH_INBOARD) / (blurred * CREST_INNER);
            let peak = exp(-(across * across));
            let trough = exp(-(inboard * inboard)) * CREST_TROUGH;
            let profile = peak - trough;
            // The water stands higher against the bow's sides as a whole, not only along the crest line.
            let outboard = max(ay - half_beam, 0.0);
            let heap_reach = hull_length * HEAP_REACH;
            let heap_width = beam * HEAP_WIDTH + 1.0 + along * HEAP_SPREAD;
            let heap_along = max(along - 2.0 * peak_at, 0.0) / heap_reach;
            let heap_across = outboard / heap_width;
            let heap = crest_height * hump_share * exp(-(heap_along * heap_along + heap_across * heap_across));
            if vertex_stage {
                height += crest * profile + heap * climb;
            } else {
                let rate = across * -2.0 / face * peak + inboard * 2.0 / (blurred * CREST_INNER) * trough;
                let heap_rate = heap * -2.0 * heap_across / heap_width * step(half_beam, ay);
                slope_a += scale * (climb_rate * fall + climb * fall_rate) * profile
                    + crest * rate * -(half_beam_rate + crest_angle)
                    + heap * climb_rate - heap * climb * 2.0 * heap_along / heap_reach - heap_rate * climb * half_beam_rate;
                slope_y += (crest * rate + heap_rate * climb) * side;
                // A fine bow's lower crest can still break: use its steepness against the breadth of the mound it
                // stood on before, with a height gate that keeps slow hulls quiet.
                let breaking = smoothstep(0.03, 0.18, crest_height / (beam * 0.12 + 1.5))
                    * smoothstep(0.12, 0.6, crest_height);
                // The sheet the bow throws up overturns and plunges along the crest's outer face: a narrow white
                // ribbon from just aft of the stem, a little wider and thinner as the water passes aft, gone by half
                // her length. Nothing breaks ahead of the stem.
                let run = breaking * smoothstep(0.0, peak_at * 0.6, a)
                    * exp(-max(along - peak_at, 0.0) / (crest_length * foam_length))
                    * (1.0 - smoothstep(hull_length * 0.25, hull_length * 0.5, along));
                let narrow = max(beam * 0.03 + 1.0, finest * 0.2);
                let spread = narrow + along * foam_spread;
                let band = (offset - blurred * 0.35) / spread;
                let ribbon = run * sqrt(narrow / spread) * exp(-(band * band) * 1.6);
                // What the breaker leaves lies on the water the crest has passed, between its line and the hull:
                // thinner the longer ago the crest crossed it, which is the farther inboard it lies.
                let passed = max(-offset, 0.0) / max(half_beam_rate + crest_angle, 0.05);
                let residue = run * BOW_RESIDUE * exp(-passed / (hull_length * BOW_RESIDUE_LENGTH))
                    * (1.0 - smoothstep(0.0, blurred * 0.6, offset));
                // The sheet's root: white water climbing the side from the stem to the crest's peak.
                let root = breaking * smoothstep(0.0, 1.0, a) * exp(-along / (peak_at * 1.5)) * exp(-(outboard * outboard) / 2.5);
                // Broken water pressed along the hull side behind the crest.
                let edge = (outboard - wash_width * 0.5) / wash_width;
                let wash = exp(-(edge * edge)) * smoothstep(hull_length * 0.02, hull_length * 0.1, a)
                    * (1.0 - smoothstep(hull_length * 0.45, hull_length * 0.75, a)) * hull.w * wash_gain;
                let breaker = max(ribbon, root);
                white = max(breaker, max(residue, wash));
                // Dense white where the crest breaks; what it leaves, and the wash along the side, are lace.
                fresh = mix(BOW_RESIDUE_YOUNG, 1.0, clamp(breaker / max(white, 1e-3), 0.0, 1.0));
                // The bubble cloud lies under the white water, no wider (a wider cloud paled the sea a beam out from
                // the bow); faint, so it reads as churned water.
                let fringe = 1.0 - smoothstep((beam * 0.9 + along * 0.3) * 0.7, beam * 0.9 + along * 0.3, ay);
                aerated = white * 0.22 * fringe;
                white *= fringe;
            }
        }
        // Kelvin V behind the stem, inside the 19.47° cusp, laid along the arc the stem has run from her course over the
        // ground (`v_a` astern, `v_y` to starboard of it): `run` metres back along the track and `off` metres to
        // starboard of it (v_a and v_y when straight).
        let course = wake.bow_slots[5 * BOW_SLOTS + i];
        let v_a = dz * course.y - dx * course.x;
        let v_y = dx * course.y + dz * course.x;
        let cross_term = 1.0 - bend * v_y;
        let off = (v_y * 2.0 - bend * (v_y * v_y + v_a * v_a))
            / (1.0 + sqrt(bend * bend * v_a * v_a + cross_term * cross_term));
        let run = select(v_a, atan2(abs(bend) * v_a, cross_term) / max(abs(bend), 1e-6), abs(bend) > 1e-6);
        let aoff = abs(off);
        let off_side = sign(off);
        let k_zero = wave.x;
        let t_off = max(aoff / max(run, 0.5), 1e-3);
        var zeta = -4.0;
        var interior = 0.0;
        // Within the V's own bounds along its arc (a curved arm leaves any box drawn about her heading).
        if run > 1.0 && run < reach && aoff < run * 0.42 + beam * 1.5 {
            zeta = CUSP_ZETA * pow(k_zero * run, 2.0 / 3.0) * (CUSP_T - t_off);
            interior = 1.0 - smoothstep(CUSP_INTERIOR_FROM, CUSP_INTERIOR_TO, zeta)
                * smoothstep(hull_length * 2.0, hull_length * 4.0, run);
        }
        if zeta > -3.0 && interior > 0.0 {
            var along_slope = 0.0;
            var off_slope = 0.0;
            // Outside the cusp line the systems meet in one crest that dies away as the Airy function does,
            // Ai(x)/Ai(0) ≈ exp(-(2/3)x^(3/2) - x/4); inside it the line keeps its slope and the fringes within it less.
            let beyond = max(-zeta, 0.0);
            let settle = exp(-beyond * (sqrt(beyond) * 0.6667 + 0.25));
            let inner = max(zeta - CUSP_LINE, 0.0) / CUSP_LINE_WIDTH;
            let line = mix(kelvin_interior, CUSP_LINE_GAIN, exp(-(inner * inner)));
            let t = min(t_off, CUSP_T);
            let t_sq = t * t;
            let r = max(max(sqrt(run * run + off * off), beam), TAU * 0.3 / k_zero);
            let root = sqrt(max(1.0 - t_sq * 8.0, 0.0));
            let cusp = pow(k_zero * r, -1.0 / 3.0) * 1.5;
            // The far end fades over half the reach, or three hull lengths of a long one.
            let fade = min(reach * 0.5, hull_length * 3.0);
            let envelope = smoothstep(0.0, hull_length * 0.2, run) * (1.0 - smoothstep(reach - fade, reach, run))
                * settle * interior * wave.y;
            let secant = sqrt(t_sq + 1.0);
            var unresolved = 0.0;
            for (var system = 0; system < 2; system++) {
                let divergent = system == 1;
                let T = select(t * 2.0 / (root + 1.0), (root + 1.0) / (t * 4.0), divergent);
                let shift = select(0.7853981633974483, -0.7853981633974483, divergent);
                let gain = select(transverse, divergent_gain, divergent);
                let big_t_sq = T * T;
                let s_sq = big_t_sq + 1.0;
                let S = sqrt(s_sq);
                let k = k_zero * s_sq;
                let phase = k_zero * S * (run - aoff * T) + shift;
                let cos_ta = max((1.0 - t * T) / (S * secant), 0.05);
                // |∂²φ/∂θ²| vanishes on the cusp; the Airy limit keeps the amplitude finite there.
                let phase_bend = max(abs(1.0 - big_t_sq * 2.0), cusp);
                let shortest = select(finest * KELVIN_FINEST, finest, vertex_stage);
                let resolved = smoothstep(shortest * 0.5, shortest, TAU / k);
                let wave_amp = envelope * gain * exp(-k * hull.z) * sqrt(TAU / (k_zero * r * s_sq * cos_ta * phase_bend));
                let amp = wave_amp * resolved;
                if vertex_stage {
                    height += amp * cos(phase);
                    continue;
                }
                // What the pixel cannot draw of the crests' slope (k times their amplitude) roughens it instead.
                let unresolved_slope = wave_amp * shading_share * line * k;
                unresolved += 0.5 * unresolved_slope * unresolved_slope * (1.0 - resolved);
                let swing = sin(phase) * amp * shading_share * line * k_zero * S;
                along_slope -= swing;
                off_slope += swing * T * off_side;
                if divergent {
                    // Divergent crests steepen and break where they meet the transverse waves at the cusp, if the
                    // drawn wave is steep enough (ak), not the steeper one the slopes are shaded at: a fast destroyer's
                    // first crests break, a battleship's barely, and the arms astern carry none.
                    let crest_foam = smoothstep(0.55, 0.98, cos(phase)) * smoothstep(0.06, 0.11, t_sq)
                        * smoothstep(kelvin_steep, kelvin_steep * 3.3, amp * k)
                        * exp(-run / (hull_length * kelvin_foam_length)) * kelvin_foam;
                    // A crest breaking is fresh white water, whatever the bow left under it.
                    if crest_foam > white {
                        white = crest_foam;
                        fresh = 1.0;
                    }
                }
            }
            if unresolved > 0.0 {
                variance += unresolved * KELVIN_ROUGHNESS * clamp(0.9 + mx_noise_float(vec3<f32>(run / KELVIN_ROUGHNESS_PATCH,
                    off_side * 5.3 + f32(i) * 11.7, wake.bow.y * 0.02)) * 1.2, 0.15, 1.6);
            }
            if !vertex_stage {
                // The arc's frame is turned by her change of course since the stem passed this point, then laid back
                // onto world x/z from her course.
                let turn = bend * run;
                let turn_cos = cos(turn);
                let turn_sin = sin(turn);
                let v_slope_a = along_slope * turn_cos - off_slope * turn_sin;
                let v_slope_y = along_slope * turn_sin + off_slope * turn_cos;
                slope_x += v_slope_y * course.y - v_slope_a * course.x;
                slope_z += v_slope_a * course.y + v_slope_y * course.x;
            }
        }
        if !vertex_stage {
            // Rotate the ship-frame slope back to world x/z.
            slope_x += slope_y * pose.w - slope_a * pose.z;
            slope_z += slope_a * pose.w + slope_y * pose.z;
            if max(white, aerated) > 0.02 {
                // World-anchored streaks along the heading: the white water flows aft past the hull and draws out
                // into strands and lace instead of riding with the ship as a painted band. Each fades before a pixel
                // holds more than one of it.
                let downstream = x * pose.z - z * pose.w;
                let abeam = x * pose.w + z * pose.z;
                let time = wake.bow.y;
                let strands = mx_noise_float(vec3<f32>(downstream * 0.02, abeam * 0.3, time * 0.25))
                    * (1.0 - smoothstep(4.0, 10.0, finest));
                let lace = mx_noise_float(vec3<f32>(downstream * 0.06 + 7.0, abeam * 0.55, time * 0.4))
                    * (1.0 - smoothstep(2.0, 5.5, finest));
                let fleck = mx_noise_float(vec3<f32>(downstream * 0.35 + 3.0, abeam * 0.9, time * 0.9))
                    * (1.0 - smoothstep(1.2, 3.5, finest));
                let noise = (strands * 0.5 + lace * 0.4 + fleck * 0.2) * breakup;
                // A noise-dithered threshold: dense at the core, torn at the edges, strands where it thins.
                let tear = mix(BOW_LACE_TEAR, BOW_TEAR, smoothstep(BOW_RESIDUE_YOUNG, 1.0, fresh));
                let coverage = smoothstep(0.18, 0.58, white + noise * tear);
                let covered = coverage * min(white, 0.85) * foam_gain;
                if covered > foam {
                    foam = covered;
                    young = fresh;
                }
                bubbles = max(bubbles, aerated * clamp(0.8 + noise * 0.4, 0.45, 1.0));
            }
        }
    }
    return BowWaveSample(height, vec2<f32>(slope_x, slope_z), min(foam, 1.0), bubbles, young, variance);
}

// ---- The fleet's trails ----------------------------------------------------------------------------------------------

fn inside_square(uv: vec2<f32>) -> bool {
    return uv.x > 0.025 && uv.x < 0.975 && uv.y > 0.025 && uv.y < 0.975;
}

fn within(p: vec2<f32>, b: vec4<f32>) -> bool {
    return p.x > b.x && p.y > b.y && p.x < b.z && p.y < b.w;
}

fn square_edge(uv: vec2<f32>) -> f32 {
    return smoothstep(0.025, 0.05, uv.x) * (1.0 - smoothstep(0.95, 0.975, uv.x)) * smoothstep(0.025, 0.05, uv.y)
        * (1.0 - smoothstep(0.95, 0.975, uv.y));
}

fn sway(p: vec2<f32>, f: f32, phase: f32) -> f32 {
    return sin(p.x * f + sin(p.y * (f * 0.61) + phase) * 1.7);
}

// Where a texel lies across its band, in the band's half-widths from its centreline, from the atlas's blue or alpha
// (shaders/stamps.wgsl's encoding of how central it is); -1 where no band's stamp reached it.
const TRACK_RANGE: f32 = 1.3;
const TRACK_ZERO: f32 = 0.1;
fn track_coordinate(encoded: f32) -> f32 {
    return select(-1.0, (1.0 - (encoded - TRACK_ZERO) / (1.0 - TRACK_ZERO)) * TRACK_RANGE, encoded > TRACK_ZERO * 0.5);
}

// The churned water's foam, as it ages: dense white water whipped into eddies behind the stern, drawn out along the track
// into streaks and braids that thin as the wake spreads, densest down the middle and along both edges.
const YOUNG_START: f32 = 0.45;
const YOUNG_FULL: f32 = 0.85;
// How much the young water's eddies are drawn out along the track.
const YOUNG_STREAKS: f32 = 0.35;
// Streaks across a band's half-width, and their length along it (in beams).
const STREAKS_ACROSS: f32 = 9.0;
const STREAK_LENGTH: f32 = 0.45;
// Old foam's cover: a braided rope of streaks down the middle of the band (out to CORE_WIDTH of its half-width), sparse
// lace across the rest, and a line along each edge, where the wake's spreading front gathers it.
const CORE_COVER: f32 = 0.6;
const CORE_WIDTH: f32 = 0.32;
const LACE_COVER: f32 = 0.1;
const EDGE_AT: f32 = 0.9;
const EDGE_WIDTH: f32 = 0.09;
const EDGE_COVER: f32 = 0.3;
// The young water's white thins toward the band's sides, where turquoise water shows through its lace.
const YOUNG_SIDES: f32 = 0.5;
// The faint line of foam and bubbles the slick keeps down its middle long after the churned water is gone.
const RESIDUAL_COVER: f32 = 0.16;
// How far the band's edges wander along it (a share of its half-width).
const EDGE_WANDER: f32 = 0.4;
const RESIDUAL_WIDTH: f32 = 0.22;
// The churned water's edge is torn over at least this much of its pattern: lace that thins into the sea around it, not
// a mask cut along one level of the eddies.
const WAKE_TEAR: f32 = 0.3;
// The bubble cloud thins from the band's middle (this share of its half-width) to its edge, and keeps this share of
// itself once the water is old: the pale turquoise stays close astern.
const BUBBLES_INSIDE: f32 = 0.3;
const OLD_BUBBLES: f32 = 0.4;
// The turbulence below which a splash's foam (no band) is lace rather than dense white water.
const SPLASH_YOUNG: f32 = 0.7;
// A splash's patch is mottled over this many eddy lengths, and keeps at least this share of its cover where the mottling
// is thinnest: broad soft patches, never an outline cut along one level of the eddies.
const SPLASH_MOTTLE: f32 = 2.8;
const SPLASH_THINNEST: f32 = 0.3;

// Streaks along a band at `band` half-widths from its centreline: fine across it, drawn out along it (world-anchored, in
// beams), braided by warps that wander at two scales, evolving at `time`. 0–1, about uniform.
fn wake_streaks(band: f32, world: vec2<f32>, beam: f32, time: f32) -> f32 {
    let along = world / (beam * STREAK_LENGTH);
    let braid = mx_noise_float(vec3<f32>(along * 0.31 + 11.0, time * 0.02)) * 1.4
        + mx_noise_float(vec3<f32>(along * 1.3 + 29.0, time * 0.04)) * 0.5;
    let across = band * STREAKS_ACROSS + braid;
    let coarse = mx_noise_float(vec3<f32>(across, along.x, along.y + time * 0.015));
    let fine = mx_noise_float(vec3<f32>(across * 2.3 + 17.0, along.x * 2.6 + 5.0, along.y * 2.6 + time * 0.03));
    let flecks = mx_noise_float(vec3<f32>(across * 4.1 + 41.0, along * 6.0 + time * 0.05));
    return clamp((coarse * 0.55 + fine * 0.35 + flecks * 0.25) * 1.35 + 0.5, 0.0, 1.0);
}

// Foam covering `cover` of a 0–1 `pattern`, its edge torn over `tear`, or the cover itself where the pattern is finer than
// the pixel (`resolved` 0); none at all where there is no cover, however high the pattern peaks.
fn torn_foam(cover: f32, pattern: f32, tear: f32, resolved: f32) -> f32 {
    let share = clamp(cover, 0.0, 1.0);
    let threshold = 1.0 - share;
    let width = max(min(tear, min(share, threshold)), 1e-4);
    return mix(smoothstep(0.0, 1.0, share), smoothstep(threshold - width, threshold + width, pattern), resolved)
        * smoothstep(0.0, 1e-3, share);
}

struct TrailSample {
    // Foam cover 0–1, the bubble cloud's density, how much of the short waves the slick stills.
    foam: f32,
    bubbles: f32,
    slick: f32,
    // How young the churned water is: 1 its dense, billowing white water, 0 the lace of old streaks.
    young: f32,
}

// The realistic wake at a world point. `footprint` is the pixel's metres, max(fwidth(x), fwidth(z)), taken by the caller
// in uniform control flow. `FleetWakeFoam.evaluate`, with the churned water streaked along the track.
fn trail_read(at: vec2<f32>, footprint: f32) -> TrailSample {
    // No trails (a calm wake binds zeroed uniforms, whose knobs would put equal edges in the smoothsteps below).
    if wake.trail.x < 0.5 {
        return TrailSample(0.0, 0.0, 0.0, 0.0);
    }
    let foam_start = wake.shading[0].x;
    let foam_full = wake.shading[0].y;
    let foam_cover = wake.shading[0].z;
    let tear = wake.shading[0].w;
    let bubble_start = wake.shading[1].x;
    let bubble_full = wake.shading[1].y;
    let bubble_share = wake.shading[1].z;
    let bubble_tear = wake.shading[1].w;
    let lobes = wake.shading[2].x;
    let k = wake.shading[2].y;
    let f = wake.shading[2].z;
    let fine = wake.shading[2].w;
    let slick_full = wake.shading[3].x;
    let calm_share = wake.shading[3].y;
    let residue = wake.shading[3].z;
    var turbulence = 0.0;
    var slick = 0.0;
    // Where the point lies across the strongest band here and across the strongest slick, and that hull's beam.
    var band = -1.0;
    var slick_band = -1.0;
    var beam = 10.0;
    var slick_beam = 10.0;
    var foam = 0.0;
    var bubbles = 0.0;
    var young = 0.0;
    var world = at;
    var strongest = -1;
    // How far inside the slick's wandering edge the point lies (1 well inside it).
    var slick_edge = 1.0;
    // Most of the sea lies outside every trail: one test against their union spares it the tiles.
    if within(at, wake.trail_reach) {
        // The trail is read through a gentle warp, so its outline frays at a few metres.
        world = at + vec2<f32>(sway(at, k, 1.3) + sway(at.yx, k * 1.83, 4.1) * 0.5,
            sway(at.yx, k * 1.19, 2.9) + sway(at, k * 2.07, 0.7) * 0.5) * lobes;
        let count = i32(wake.trail.x);
        for (var i = 0; i < count; i++) {
            if !within(world, wake.tiles[4 * TILE_COUNT + i]) {
                continue;
            }
            let bounds = wake.tiles[i];
            let slick_row = wake.tiles[TILE_COUNT + i];
            let slick_uv = (world - slick_row.xy) / SLICK_EXTENT + 0.5;
            if within(world, wake.tiles[3 * TILE_COUNT + i]) && inside_square(slick_uv) {
                let wide = textureSampleLevel(trail_atlas, wake_sampler, (slick_uv + bounds.zw) / TILES, 0.0);
                let value = wide.g * square_edge(slick_uv);
                if value > slick {
                    slick = value;
                    slick_band = track_coordinate(wide.a);
                    slick_beam = max(slick_row.z, 1.0);
                }
            }
            let uv = (world - bounds.xy) / WAKE_EXTENT + 0.5;
            if within(world, wake.tiles[2 * TILE_COUNT + i]) && inside_square(uv) {
                let tile = textureSampleLevel(trail_atlas, wake_sampler, (uv + bounds.zw) / TILES, 0.0);
                let value = tile.r * square_edge(uv);
                if value > turbulence {
                    turbulence = value;
                    strongest = i;
                    beam = max(slick_row.z, 1.0);
                }
            }
        }
        // The streaks follow the track itself, not the warp that frays the outline: where the point lies across the
        // strongest band is read where it is.
        if strongest >= 0 {
            let bounds = wake.tiles[strongest];
            let uv = (at - bounds.xy) / WAKE_EXTENT + 0.5;
            band = track_coordinate(textureSampleLevel(trail_atlas, wake_sampler, (uv + bounds.zw) / TILES, 0.0).b);
            if band < 0.0 {
                // Coverage the warp brought from a band, read past where the band says where it lies: outside it, not
                // a splash's dense white water (whose own stamps carry no band where the coverage was read).
                let source = (world - bounds.xy) / WAKE_EXTENT + 0.5;
                let there = textureSampleLevel(trail_atlas, wake_sampler, (source + bounds.zw) / TILES, 0.0).b;
                band = select(-1.0, TRACK_RANGE, track_coordinate(there) >= 0.0);
            }
        }
        let time = wake.trail.y;
        // Where the eddies and streaks fall below a pixel, it takes their mean cover instead of shimmering.
        let resolved = 1.0 - smoothstep(0.15, 0.6, footprint * f);
        if turbulence > 0.01 {
            // World-anchored eddies from ten metres down to a metre and a half, evolving slowly: the young water's churn.
            let coarse = 1.0 - fine;
            let eddies = mx_noise_float(vec3<f32>(world * f, time * 0.05)) * (coarse * 0.56)
                + mx_noise_float(vec3<f32>(world * (f * 2.4) + 17.0, time * 0.09)) * (coarse * 0.44)
                + mx_noise_float(vec3<f32>(world * (f * 6.0) + 41.0, time * 0.16)) * fine;
            var pattern = clamp(eddies * 1.3 + 0.5, 0.0, 1.0);
            young = smoothstep(YOUNG_START, YOUNG_FULL, turbulence);
            if band < 0.0 {
                // A splash's boil is dense white water only at its height; its spreading ring is lace from the first.
                young = smoothstep(SPLASH_YOUNG, 1.0, turbulence);
            }
            var cover = smoothstep(foam_start, foam_full, turbulence) * foam_cover;
            var aerated = 1.0;
            if band >= 0.0 {
                // The band's edges wander a little along it, as a spreading wake's do.
                let wander = 1.0 + (mx_noise_float(vec3<f32>(world / (beam * 2.2), time * 0.01)) * 0.7
                    + mx_noise_float(vec3<f32>(world / (beam * 0.6) + 7.0, time * 0.02)) * 0.3) * EDGE_WANDER;
                let across = band * wander;
                // Away from the stern the foam lies in streaks along the track, down the middle and along the edges.
                let streaks = wake_streaks(band, world, beam, time);
                let middle = 1.0 - smoothstep(CORE_WIDTH * 0.35, CORE_WIDTH, across);
                let inside = 1.0 - smoothstep(0.75, 1.0, across);
                let edge = exp(-pow((across - EDGE_AT) / EDGE_WIDTH, 2.0));
                let old_cover = smoothstep(0.0, foam_full, turbulence)
                    * max(max(middle * CORE_COVER, inside * LACE_COVER), edge * EDGE_COVER);
                // Even the young water is drawn out along the track a little, thins toward the sides and ends at the
                // band's edge.
                pattern = mix(streaks, mix(pattern, streaks, YOUNG_STREAKS), young);
                let young_cover = cover * (1.0 - smoothstep(0.35, 0.9, across) * YOUNG_SIDES) * (1.0 - smoothstep(0.8, 1.1, across));
                cover = mix(old_cover, max(young_cover, edge * EDGE_COVER), young);
                // The bubble cloud fills the band, densest down the middle, patchy and streaked like the foam over it,
                // fading out across the outer part of the band.
                let patches = smoothstep(0.2, 0.8, streaks * 0.5 + (eddies * 0.5 + 0.5) * 0.5);
                aerated = (1.0 - smoothstep(BUBBLES_INSIDE, 1.1, across)) * mix(1.0, mix(0.35, 1.0, patches), 1.0 - young)
                    * mix(OLD_BUBBLES, 1.0, young);
            }
            if band < 0.0 {
                // A splash's patch keeps its energy smooth: the surface's own foam texture tears it into lace
                // (`churned_water`), thinning as the patch spreads and fades, and the broad mottling here keeps it from
                // reading as a disc.
                let mottle = mx_noise_float(vec3<f32>(world * (f / SPLASH_MOTTLE) + 53.0, time * 0.03)) * 0.9
                    + eddies * 0.35;
                foam = cover * mix(SPLASH_THINNEST, 1.0, clamp(mottle + 0.6, 0.0, 1.0));
                pattern = clamp(mottle * 0.8 + 0.5, 0.0, 1.0);
            } else {
                foam = torn_foam(cover, pattern, max(tear, WAKE_TEAR), resolved);
            }
            bubbles = smoothstep(bubble_start, bubble_full, turbulence + eddies * bubble_tear) * (pattern * 0.4 + 0.6)
                * bubble_share * aerated;
        }
        // A slick read where its band was never written lies past its edge (stilled water all the way out to a texel
        // boundary drew straight-edged calm patches).
        if slick > 0.01 && slick_band < 0.0 {
            slick_edge = 0.0;
        }
        if slick > 0.01 && slick_band >= 0.0 {
            // The slick's edges wander and fade over its outer part instead of stopping at a line.
            let wander = 1.0 + mx_noise_float(vec3<f32>(world / (slick_beam * 3.1) + 23.0, time * 0.01)) * EDGE_WANDER;
            let across = slick_band * wander;
            slick_edge = 1.0 - smoothstep(0.45, 1.0, across);
            // The old wake's middle: a few thin streaks of foam over a faint bubble cloud, as long as the slick lasts.
            let strength = smoothstep(0.0, 0.6, slick);
            let middle = 1.0 - smoothstep(RESIDUAL_WIDTH * 0.4, RESIDUAL_WIDTH, across);
            let streaks = wake_streaks(slick_band * 0.5, world, slick_beam, time);
            let residual = torn_foam(strength * middle * RESIDUAL_COVER, streaks, tear, resolved);
            foam = max(foam, residual * (1.0 - young));
            let patches = smoothstep(0.25, 0.75, streaks);
            bubbles = max(bubbles, strength * (1.0 - smoothstep(0.2, 0.75, across)) * mix(0.5, 1.0, patches) * residue);
        }
    }
    let calm = smoothstep(0.0, max(slick_full, 1e-6), slick) * slick_edge;
    return TrailSample(foam, bubbles, calm * calm_share, young);
}

// ---- Torpedo tracks ------------------------------------------------------------------------------------------------

fn torpedo_foam(at: vec2<f32>) -> f32 {
    if wake.trail.w < 0.5 {
        return 0.0;
    }
    let uv = (at - wake.torpedo.xy) / WAKE_EXTENT + 0.5;
    var energy = 0.0;
    if uv.x > 0.0 && uv.x < 1.0 && uv.y > 0.0 && uv.y < 1.0 {
        let coverage = textureSampleLevel(torpedo_field, wake_sampler, uv, 0.0).r;
        if coverage > 0.01 {
            // Metre-scale clumps: a track is a string of bubble patches, not a painted line.
            let time = wake.trail.z;
            let clumps = mx_noise_float(vec3<f32>(at * 0.32, time * 0.12));
            let patches = mx_noise_float(vec3<f32>(at * 0.09 + 31.0, time * 0.05));
            let edge = smoothstep(0.0, 0.03, uv.x) * (1.0 - smoothstep(0.97, 1.0, uv.x)) * smoothstep(0.0, 0.03, uv.y)
                * (1.0 - smoothstep(0.97, 1.0, uv.y));
            energy = coverage * clamp(clumps * 0.55 + patches * 0.5 + 0.74, 0.3, 1.0) * edge;
        }
    }
    return energy;
}

// ---- Hull contact foam ---------------------------------------------------------------------------------------------

// Foam coverage on the water where hulls meet it. `x`, `z` anchor the noise (the grid point); the hull test reads the
// fragment's drawn position `world`. `HullContactFoam.evaluate`.
fn hull_foam(x: f32, z: f32, world: vec3<f32>) -> f32 {
    var foam = 0.0;
    let point = vec4<f32>(world, 1.0);
    let eye = camera.position.xyz;
    let range = max(length(vec3<f32>(eye.x - x, eye.y, eye.z - z)), 1.0);
    // Three pixels stretched by the grazing view: a line narrower than that fades instead of shimmering.
    let finest = range * wake.hull.z / max(abs(eye.y) / range, 0.04) * 3.0;
    let count = i32(wake.hull.x);
    for (var i = 0; i < count; i++) {
        let base = i * HULL_STRIDE;
        let bound = wake.hull_data[base];
        let dx = world.x - bound.x;
        let dz = world.z - bound.y;
        if dx * dx + dz * dz >= bound.z {
            continue;
        }
        let shape = wake.hull_data[base + 4];
        let drive = wake.hull_data[base + 5];
        let lx = dot(wake.hull_data[base + 1], point);
        let lz = dot(wake.hull_data[base + 3], point);
        let width = drive.z;
        let hull_length = drive.w;
        let half = shape.z;
        if !(abs(lx) < half + width * 3.0 && lz > shape.w - width * 3.0 - shape.y
            && lz < shape.w + hull_length + width * 3.0 + shape.y) {
            continue;
        }
        // Bilinear in station and level: the hull's breadth where the water actually stands against it.
        let ly = dot(wake.hull_data[base + 2], point);
        let level = clamp((ly - PROFILE_LOWEST) / PROFILE_LEVEL_STEP, 0.0, f32(PROFILE_LEVELS - 1));
        let lower = min(floor(level), f32(PROFILE_LEVELS - 2));
        let rise = level - lower;
        let station = clamp((lz - shape.x) / shape.y, 0.0, f32(PROFILE_STATIONS - 1));
        let first = min(floor(station), f32(PROFILE_STATIONS - 2));
        let along = station - first;
        let cell = base + HULL_PROFILE + i32(lower) * PROFILE_STATIONS + i32(first);
        let below = wake.hull_data[cell];
        let above = wake.hull_data[cell + PROFILE_STATIONS];
        let breadth = mix(mix(below.xy, below.zw, along), mix(above.xy, above.zw, along), rise);
        // Metres outboard of the hull's side on this side of the centreline.
        let edge = select(-lx - breadth.y, lx - breadth.x, lx >= 0.0);
        let blurred = max(width, finest);
        let across = (edge - blurred * 0.3) / blurred;
        let line = exp(-(across * across)) * smoothstep(-0.6, 0.0, edge) * (width / blurred);
        let from_bow = max((lz - shape.w) / hull_length, 0.0);
        let strength = drive.x + drive.y * (1.0 - smoothstep(0.0, wake.hull.w, from_bow));
        let white = line * strength;
        if white > 0.01 {
            // World-anchored eddies: the water streams aft past a moving hull and churns slowly at rest.
            let time = wake.hull.y;
            let strands = mx_noise_float(vec3<f32>(x * 0.21, z * 0.21, time * 0.35));
            let flecks = mx_noise_float(vec3<f32>(x * 0.63 + 5.0, z * 0.63, time * 0.8));
            // Distant water keeps the mean coverage instead of sub-pixel noise.
            let detail = 1.0 - smoothstep(1.5, 6.0, finest);
            let noise = (strands * 0.6 + flecks * 0.4) * detail;
            foam = max(foam, smoothstep(0.1, 0.7, white + noise * 0.6) * min(white, 0.55));
        }
    }
    return foam;
}

// ---- The sea beside hulls (shelter) ---------------------------------------------------------------------------------

const HULL_SIDES: i32 = HULL_PROFILE + PROFILE_LEVELS * PROFILE_STATIONS;
const PI: f32 = 3.141592653589793;
// Distances from a hull's side, in heights of the side, over which its shelter fades out.
const SHELTER_FADE: f32 = 3.0;
const SHELTER_REACH: f32 = 5.0;
// Softest edge (radians) of the side's top in a mirrored lobe.
const MIRROR_EDGE: f32 = 0.02;

// Height of the side's top at `station` (clamped to the hull's ends) on the starboard or port side, in hull coordinates.
fn side_top(base: i32, station: f32, starboard: bool) -> f32 {
    let first = min(floor(station), f32(PROFILE_STATIONS - 2));
    let along = station - first;
    let cell = wake.hull_data[base + HULL_SIDES + i32(first)];
    let height = mix(cell.xy, cell.zw, along);
    return select(height.y, height.x, starboard);
}

// Height of the side standing at `station` above the water at hull coordinates `local`, on the starboard or port side,
// `breadth` out from the centre line, in the world (`up`, the world's up in hull coordinates): a heeled hull's high side
// does not stand taller the farther out the water lies.
fn side_height(base: i32, station: f32, starboard: bool, local: vec3<f32>, breadth: f32, up: vec3<f32>) -> f32 {
    let top = vec3<f32>(select(-breadth, breadth, starboard), side_top(base, station, starboard), local.z);
    return max(dot(up, top - local), 0.0);
}

// A wall's view factor term for one end at `x` (the end's distance along the side over the distance out), `k` =
// 1/√(1 + Y²).
fn wall_end(x: f32, k: f32) -> f32 {
    return atan(x) - atan(x * k) * k;
}

// How the hulls near the displaced point `world` stand over the water there (`HullContactFoam.evaluateShelter`): the
// share of the sky's light their sides hide, the share of the mirrored lobe along `direction` (spread `spread` radians)
// that meets a side instead of the sky, and that side's horizontal outward normal (zw).
fn hull_shelter(world: vec3<f32>, direction: vec3<f32>, spread: f32) -> vec4<f32> {
    var open = 1.0;
    var mirror = 0.0;
    var outward = vec2<f32>(0.0);
    var dominant = 0.0;
    let point = vec4<f32>(world, 1.0);
    let ray = vec4<f32>(direction, 0.0);
    let edge_softness = max(spread, MIRROR_EDGE);
    let count = i32(wake.hull.x);
    for (var i = 0; i < count; i++) {
        let base = i * HULL_STRIDE;
        let bound = wake.hull_data[base];
        let dx = world.x - bound.x;
        let dz = world.z - bound.y;
        if dx * dx + dz * dz >= bound.w {
            continue;
        }
        let across = wake.hull_data[base + 1];
        let row_y = wake.hull_data[base + 2];
        let row_z = wake.hull_data[base + 3];
        let shape = wake.hull_data[base + 4];
        let drive = wake.hull_data[base + 5];
        let shelter = wake.hull_data[base + 6];
        let lx = dot(across, point);
        let ly = dot(row_y, point);
        let lz = dot(row_z, point);
        let bow = shape.w;
        let stern = shape.w + drive.w;
        let starboard = lx >= 0.0;
        let strength = shelter.x;
        let reach = shelter.y;
        // Water farther out than the tallest side's reach takes no shelter from this hull.
        if !(abs(lx) < shape.z + reach && lz > bow - reach && lz < stern + reach) {
            continue;
        }
        let first_station = (bow - shape.x) / shape.y;
        let last_station = (stern - shape.x) / shape.y;
        // Breadth at the water's level, as the foam reads it, at the nearest station of the hull.
        let station = clamp((lz - shape.x) / shape.y, first_station, last_station);
        let first = min(floor(station), f32(PROFILE_STATIONS - 2));
        let along = station - first;
        let level = clamp((ly - PROFILE_LOWEST) / PROFILE_LEVEL_STEP, 0.0, f32(PROFILE_LEVELS - 1));
        let lower = min(floor(level), f32(PROFILE_LEVELS - 2));
        let rise = level - lower;
        let cell = base + HULL_PROFILE + i32(lower) * PROFILE_STATIONS + i32(first);
        let below = wake.hull_data[cell];
        let above = wake.hull_data[cell + PROFILE_STATIONS];
        let breadth = mix(mix(below.xy, below.zw, along), mix(above.xy, above.zw, along), rise);
        let side = select(breadth.y, breadth.x, starboard);
        let out_dist = max(abs(lx) - side, 0.05);
        let up = vec3<f32>(across.y, row_y.y, row_z.y);
        let wall = side_height(base, station, starboard, vec3<f32>(lx, ly, lz), side, up);
        let fade = (1.0 - smoothstep(wall * SHELTER_FADE, wall * SHELTER_REACH + 1e-3, out_dist)) * strength;
        // The view factor of the wall, both ends.
        let ratio = wall / out_dist;
        let k = 1.0 / sqrt(ratio * ratio + 1.0);
        let hidden = clamp((wall_end((lz - bow) / out_dist, k) + wall_end((stern - lz) / out_dist, k)) / (2.0 * PI), 0.0, 0.5);
        let hides = hidden * fade;
        open *= 1.0 - hides;
        // The mirrored ray: where it reaches the wall's plane, and whether it passes below the wall's top there.
        let local = vec3<f32>(dot(across, ray), dot(row_y, ray), dot(row_z, ray));
        let toward = select(local.x, -local.x, starboard);
        let flat_length = max(length(local.xz), 1e-4);
        let travel = min(out_dist / max(toward, 1e-3), 4000.0);
        let hit = lz + local.z * travel;
        let hit_station = clamp((hit - shape.x) / shape.y, first_station, last_station);
        // In the hull's frame, as the ray is: the side's plane and its top there are hers.
        let top = atan2(max(side_top(base, hit_station, starboard) - ly, 0.0), travel * flat_length) - atan2(max(local.y, 0.0), flat_length);
        let ends = smoothstep(bow - shape.y, bow + shape.y, hit) * (1.0 - smoothstep(stern - shape.y, stern + shape.y, hit));
        let met = smoothstep(-edge_softness, edge_softness, top) * smoothstep(0.0, 0.05, toward) * ends * fade;
        mirror = max(mirror, met);
        // The side that shelters this water most names the light the water sees on it.
        if max(met, hides) > dominant {
            dominant = max(met, hides);
            outward = normalize(across.xz * select(-1.0, 1.0, starboard) + vec2<f32>(1e-6, 0.0));
        }
    }
    return vec4<f32>(1.0 - open, mirror, outward);
}

// The water's light beside hulls (`surface/shelter.ts`): the side is lit by half the sky and by the sun where it faces
// it, and grey paint returns a share of that.
const HULL_ALBEDO: f32 = 0.3;
const SIDE_SKY: f32 = 0.5;
const SIDE_SUN: f32 = 1.0;

struct HullShelter {
    // The sky's irradiance on the water with what the sides hide replaced by the light they scatter.
    irradiance: vec3<f32>,
    // The share of the sky's light that still reaches the water.
    sky_share: f32,
    // The sides' exitance divided by π (their radiance), and the share of the mirrored lobe they fill.
    side_radiance: vec3<f32>,
    mirrored: f32,
}

// `shelter` from [`hull_shelter`]; `irradiance` the sky's on the horizontal; `sun_irradiance` the sun's on a surface
// facing it, from `sun`.
fn shelter_light(shelter: vec4<f32>, irradiance: vec3<f32>, sun_irradiance: vec3<f32>, sun: vec3<f32>) -> HullShelter {
    let hidden = shelter.x;
    let outward = vec3<f32>(shelter.z, 0.0, shelter.w);
    let sunlit = sun_irradiance * (max(dot(outward, sun), 0.0) * smoothstep(-0.1, 0.05, sun.y) * SIDE_SUN);
    let side = (irradiance * SIDE_SKY + sunlit) * HULL_ALBEDO;
    return HullShelter(mix(irradiance, side, hidden), 1.0 - hidden * (1.0 - HULL_ALBEDO * SIDE_SKY), side / PI, shelter.y);
}

// `sky`, mirrored by the water, where the mirrored lobe meets a side: never brighter than the sky it hides.
fn shelter_reflect(contact: HullShelter, sky: vec3<f32>) -> vec3<f32> {
    return mix(sky, min(sky, contact.side_radiance), contact.mirrored);
}

// ---- Together (`ShipWake`'s sampler) ------------------------------------------------------------------------------

// Height the wakes add to the sea at a grid point (vertex stage).
fn wake_height(xz: vec2<f32>) -> f32 {
    return wake_field(xz).x + bow_waves(xz.x, xz.y, true).height;
}

struct WakeSurface {
    // The wake's upward normal (the field's slope with the bow waves' added).
    normal: vec3<f32>,
    // The greatest of every foam: field breaking, trail, torpedo, bow waves, contact.
    foam: f32,
    bubbles: f32,
    // How much of the short waves the slick stills.
    slick: f32,
    // How young that foam is: 1 dense white water, 0 the lace of an old wake's streaks.
    young: f32,
    // The slope variance of the bow waves' crests too short to draw (`BowWaveSample::variance`).
    variance: f32,
}

// Everything the fragment stage reads at grid point `xz` (the displaced point `world`); `footprint` from uniform flow.
fn wake_surface(xz: vec2<f32>, world: vec3<f32>, footprint: f32) -> WakeSurface {
    let field = wake_field(xz);
    let base = normalize(vec3<f32>(-field.y, 1.0, -field.z));
    let bow = bow_waves(xz.x, xz.y, false);
    let normal = normalize(vec3<f32>(base.x - bow.slope.x * base.y, base.y, base.z - bow.slope.y * base.y));
    let trail = trail_read(xz, footprint);
    let fresh = max(max(max(field.w, 0.0), torpedo_foam(xz)), hull_foam(xz.x, xz.y, world));
    let other = max(fresh, bow.foam);
    // The trail's own foam keeps its age, and so does what a bow's breaker left behind it; every other foam here is
    // fresh white water.
    let young = select(select(1.0, bow.young, bow.foam > fresh), trail.young, trail.foam > other);
    return WakeSurface(normal, max(other, trail.foam), max(trail.bubbles, bow.bubbles), trail.slick, young, bow.variance);
}
