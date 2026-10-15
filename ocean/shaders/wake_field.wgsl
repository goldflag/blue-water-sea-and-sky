// The dispersive wake field's GPU passes: a port of the web game's `ocean/wake/WakeField.ts` (its reduce, collapse, step and
// display fragment passes, here compute on storage textures). A height field in the spirit of iWave on a movable
// anchor: ∂²h/∂t² + γ∂h/∂t = −g√(−∇²)(h + p/ρg), with √(−∇²) applied as the fitted kernels of kernel.rs on a
// reduce/collapse pyramid, leapfrog in time, velocity damping, a sponge at the edges, foam from breaking steepness and
// the hulls' pressure patches laid as swept Gaussian footprints. Native only: what strikes the sea (impulses.rs), laid
// as a crater or dome with no velocity of its own, a held pressure head and fresh foam, and the ring trains it starts.
//
// State texels: (h, h of the previous step, foam, hull pressure head for the next step). Display texels: (h, ∂h/∂x,
// ∂h/∂z, foam), half float and filterable, which the surface samples.

// A level of the pyramid: what a reduce or collapse pass reads and writes.
struct Level {
    // Edge of the grid written, of the grid read (the finer one for a reduce), and 1 when reading the state (its x + w).
    size: i32,
    source_size: i32,
    from_state: i32,
    // Edge of the coarser collapsed grid added in, 0 at the coarsest level.
    coarser_size: i32,
    // This level's kernel taps in `taps`: first and count.
    taps: vec2<u32>,
    padding: vec2<u32>,
}

// One simulation step.
struct Step {
    size: i32,
    // The finest collapsed level's edge (size / 2).
    top_size: i32,
    break_cells: i32,
    count: i32,
    // Whole-cell shift of the state's content since the previous step.
    shift: vec2<i32>,
    // Level 0's kernel in `taps`.
    taps: vec2<u32>,
    cell_size: f32,
    acceleration: f32,
    damping: f32,
    foam_decay: f32,
    foam_diffusion: f32,
    foam_strength: f32,
    foam_threshold: f32,
    min_radius: f32,
    // Per generator: swept segment (x0, z0, x1, z1) relative to the field centre, and (radius, depth, gate, 0).
    segments: array<vec4<f32>, 16>,
    shapes: array<vec4<f32>, 16>,
    // Per generator: the far end of the stretch its own waves break along (x, z relative to the field centre; from the
    // swept segment's end), and how far either side of it (m), 0, 0.
    breaks: array<vec4<f32>, 16>,
    // x: impulses this step, y: ring trains.
    disturbances: vec4<i32>,
    // Per impulse: its path (x0, z0, x1, z1) relative to the field centre, and (radius, height laid this step, head
    // this step, foam).
    impulse_paths: array<vec4<f32>, 16>,
    impulse_shapes: array<vec4<f32>, 16>,
    // Per ring train: its path, and (reach, soft margin beyond it, strength 0-1, 0).
    zone_paths: array<vec4<f32>, 16>,
    zone_shapes: array<vec4<f32>, 16>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var coarser: texture_2d<f32>;
@group(0) @binding(2) var out_level: texture_storage_2d<r32float, write>;
@group(0) @binding(3) var out_state: texture_storage_2d<rgba32float, write>;
@group(0) @binding(4) var out_display: texture_storage_2d<rgba16float, write>;
@group(0) @binding(5) var<uniform> step_params: Step;
@group(0) @binding(6) var<storage, read> taps: array<vec4<f32>>;
@group(0) @binding(7) var<uniform> level: Level;

const STEP: f32 = 1.0 / 30.0;
// The ocean's underwater bound relies on the wake never exceeding this (m).
const MAX_HEIGHT: f32 = 8.0;
// Share of the field at each edge where extra damping absorbs outgoing waves instead of reflecting them.
const SPONGE: f32 = 0.06;
const SPONGE_DAMPING: f32 = 1.5;
// Foam energy left along the hull path at full `foamStrength`.
const HULL_FOAM: f32 = 0.3;
// Foam per second added where the steepness is twice the break threshold, before `foamStrength`.
const BREAK_RATE: f32 = 1.0;
// A hull's breaking fades out from her stretch's reach to this many times it.
const BREAK_FADE: f32 = 1.5;

// Squared distance from `position` to the capsule spine `path` (x0, z0, x1, z1).
fn spine_distance2(position: vec2<f32>, path: vec4<f32>) -> f32 {
    let along = path.zw - path.xy;
    let offset = position - path.xy;
    let t = clamp(dot(offset, along) / max(dot(along, along), 1e-6), 0.0, 1.0);
    let miss = offset - along * t;
    return dot(miss, miss);
}

// Clamped texel reads: the pyramid treats the sea beyond the field as continuing the edge value.
fn load(tex: texture_2d<f32>, cell: vec2<i32>, size: i32) -> vec4<f32> {
    return textureLoad(tex, clamp(cell, vec2<i32>(0), vec2<i32>(size - 1)), 0);
}

// Bilinear 2× upsample of a level onto the next finer grid, texel centre aligned: fine texel p lies at coarse
// coordinate p/2 − 1/4. Matches the collapse the kernels were fitted for.
fn upsample(tex: texture_2d<f32>, fine: vec2<i32>, size: i32) -> f32 {
    let coarse = vec2<f32>(fine) * 0.5 - 0.25;
    let low = vec2<i32>(floor(coarse));
    let t = fract(coarse);
    let row0 = mix(load(tex, low, size).x, load(tex, low + vec2<i32>(1, 0), size).x, t.x);
    let row1 = mix(load(tex, low + vec2<i32>(0, 1), size).x, load(tex, low + vec2<i32>(1, 1), size).x, t.x);
    return mix(row0, row1, t.y);
}

// What the pyramid reduces from a source texel: the state's height plus the hull's pressure head, or a level's value.
fn reducible(cell: vec2<i32>) -> f32 {
    let texel = load(source, cell, level.source_size);
    return select(texel.x, texel.x + texel.w, level.from_state != 0);
}

// Reduce: [1 3 3 1]/8 per axis, from (h + pressure head) at the field's grid down to the coarsest level.
@compute @workgroup_size(8, 8)
fn reduce_pass(@builtin(global_invocation_id) id: vec3<u32>) {
    let cell = vec2<i32>(id.xy);
    if any(cell >= vec2<i32>(level.size)) {
        return;
    }
    var binomial = array<f32, 4>(1.0, 3.0, 3.0, 1.0);
    let base = cell * 2 - 1;
    var sum = 0.0;
    for (var b = 0; b < 4; b++) {
        for (var a = 0; a < 4; a++) {
            sum += reducible(base + vec2<i32>(a, b)) * (binomial[a] * binomial[b] / 64.0);
        }
    }
    textureStore(out_level, cell, vec4<f32>(sum, 0.0, 0.0, 1.0));
}

// Collapse, coarsest first: each level's kernel plus the upsampled result of the level below it.
@compute @workgroup_size(8, 8)
fn collapse_pass(@builtin(global_invocation_id) id: vec3<u32>) {
    let cell = vec2<i32>(id.xy);
    if any(cell >= vec2<i32>(level.size)) {
        return;
    }
    var sum = 0.0;
    for (var i = level.taps.x; i < level.taps.x + level.taps.y; i++) {
        let tap = taps[i];
        sum += load(source, cell + vec2<i32>(tap.xy), level.size).x * tap.z;
    }
    if level.coarser_size > 0 {
        sum += upsample(coarser, cell, level.coarser_size);
    }
    textureStore(out_level, cell, vec4<f32>(sum, 0.0, 0.0, 1.0));
}

// Step: leapfrog with velocity damping, then foam and this step's generator footprints. A hull is a moving pressure
// patch: at rest its head p/ρg = depth would settle into a depression of that depth, and in motion it radiates the
// Kelvin pattern. Its head is stored for the next step (one step late) so the operator can read it at every tap.
// Reads the previous state shifted by whole cells.
@compute @workgroup_size(8, 8)
fn step_pass(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = step_params.size;
    let cell = vec2<i32>(id.xy);
    if any(cell >= vec2<i32>(n)) {
        return;
    }
    let source_cell = cell + step_params.shift;
    let inside = all(source_cell >= vec2<i32>(0)) && all(source_cell < vec2<i32>(n));
    let center = load(source, source_cell, n);
    let h = center.x;
    let previous = center.y;
    let foam = center.z;
    var vertical = 0.0;
    for (var i = step_params.taps.x; i < step_params.taps.x + step_params.taps.y; i++) {
        let tap = taps[i];
        let texel = load(source, source_cell + vec2<i32>(tap.xy), n);
        vertical += (texel.x + texel.w) * tap.z;
    }
    vertical += upsample(coarser, source_cell, step_params.top_size);
    let texel = vec2<f32>(cell);
    let edge = min(min(texel.x, texel.y), f32(n - 1) - max(texel.x, texel.y));
    let sponge = 1.0 - smoothstep(0.0, f32(n) * SPONGE, edge);
    let a = step_params.damping + sponge * sponge * (SPONGE_DAMPING * STEP / 2.0);
    let height = clamp((h * 2.0 - (1.0 - a) * previous - step_params.acceleration * vertical) / (a + 1.0), -MAX_HEIGHT, MAX_HEIGHT);
    let reach = vec2<i32>(step_params.break_cells, 0);
    let up = vec2<i32>(0, step_params.break_cells);
    let slope = vec2<f32>(
        load(source, source_cell + reach, n).x - load(source, source_cell - reach, n).x,
        load(source, source_cell + up, n).x - load(source, source_cell - up, n).x,
    ) / (step_params.cell_size * (f32(step_params.break_cells) * 2.0));
    let spread = (load(source, source_cell + vec2<i32>(1, 0), n).z + load(source, source_cell + vec2<i32>(-1, 0), n).z
        + load(source, source_cell + vec2<i32>(0, 1), n).z + load(source, source_cell + vec2<i32>(0, -1), n).z - foam * 4.0)
        * step_params.foam_diffusion;
    // Footprints are laid on the new grid, relative to its centre.
    let position = (vec2<f32>(cell) + (0.5 - f32(n) / 2.0)) * step_params.cell_size;
    // Inside a strike's ring train the hulls' breaking threshold is not taken: its rings are not a hull's waves, and the
    // strike laid its own white water.
    var ringed = 0.0;
    for (var i = 0; i < step_params.disturbances.y; i++) {
        let zone = step_params.zone_shapes[i];
        let d = sqrt(spine_distance2(position, step_params.zone_paths[i]));
        ringed = max(ringed, zone.z * (1.0 - smoothstep(zone.x, zone.x + zone.y, d)));
    }
    var head = 0.0;
    var wash = 0.0;
    // How far this cell lies within a hull's own water: along her sides, from each generator to her middle.
    var own = 0.0;
    for (var i = 0; i < step_params.count; i++) {
        let segment = step_params.segments[i];
        let shape = step_params.shapes[i];
        let along = segment.zw - segment.xy;
        let offset = position - segment.xy;
        let t = clamp(dot(offset, along) / max(dot(along, along), 1e-6), 0.0, 1.0);
        let miss = offset - along * t;
        let radius = max(shape.x, step_params.min_radius);
        let footprint = exp(dot(miss, miss) / (radius * radius) * -2.0);
        head += shape.y * footprint;
        wash = max(wash, shape.z * footprint);
        let breaks = step_params.breaks[i];
        let side = sqrt(spine_distance2(position, vec4<f32>(segment.zw, breaks.xy)));
        own = max(own, 1.0 - smoothstep(breaks.z, breaks.z * BREAK_FADE + step_params.min_radius, side));
    }
    // The breaking threshold is tuned for the hulls' under-resolved waves, which steepen only along their sides: taken
    // there and nowhere else, so whatever else rings the field (a strike's rings, long after its train is forgotten)
    // never breaks into white water.
    let breaking = max(length(slope) - step_params.foam_threshold, 0.0) / step_params.foam_threshold
        * (step_params.foam_strength * BREAK_RATE * STEP) * (1.0 - ringed) * own;
    let hull = wash * step_params.foam_strength * HULL_FOAM;
    // What strikes the sea: its height raised in both steps (no velocity of its own), its head held, its foam laid.
    var lift = 0.0;
    var press = 0.0;
    var struck = 0.0;
    for (var i = 0; i < step_params.disturbances.x; i++) {
        let shape = step_params.impulse_shapes[i];
        let radius = max(shape.x, step_params.min_radius);
        let s = spine_distance2(position, step_params.impulse_paths[i]) / (radius * radius);
        if s < 12.0 {
            let bell = exp(-s);
            lift += shape.y * (1.0 - s) * bell;
            press += shape.z * bell;
            struck = max(struck, shape.w * bell);
        }
    }
    let laid = max(hull, struck);
    let updated = vec4<f32>(
        clamp(height + lift, -MAX_HEIGHT, MAX_HEIGHT),
        clamp(h + lift, -MAX_HEIGHT, MAX_HEIGHT),
        max((foam + spread) * step_params.foam_decay + breaking, laid),
        head + press,
    );
    textureStore(out_state, cell, select(vec4<f32>(lift, lift, laid, head + press), updated, inside));
}

// Display: height, central-difference slope and foam of the new state, for filtered sampling.
@compute @workgroup_size(8, 8)
fn display_pass(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = step_params.size;
    let cell = vec2<i32>(id.xy);
    if any(cell >= vec2<i32>(n)) {
        return;
    }
    let center = load(source, cell, n);
    let slope = vec2<f32>(
        load(source, cell + vec2<i32>(1, 0), n).x - load(source, cell + vec2<i32>(-1, 0), n).x,
        load(source, cell + vec2<i32>(0, 1), n).x - load(source, cell + vec2<i32>(0, -1), n).x,
    ) / (step_params.cell_size * 2.0);
    textureStore(out_display, cell, vec4<f32>(center.x, slope, center.z));
}
