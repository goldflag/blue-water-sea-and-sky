// Rain on the sea (native only: the web's sea took none). Every drop sends a ring of capillary waves a few centimetres
// long across the water. Where a pixel resolves them the rings are drawn into the surface's normal, from a jittered
// grid of drops in two sizes, each landing somewhere new every life; where it does not they are slope variance (Cox &
// Munk's unresolved roughness, which rain raises most on a calm sea), so the mirrored sky blurs and the sun's glitter
// spreads and dims as the rain thickens.
#define_import_path naval_ocean::rain

const TAU: f32 = 6.283185307179586;
// Mean square slope of the rings at the heaviest rain (0 dry … 1 a downpour).
const RAIN_SLOPE_VARIANCE: f32 = 0.02;
// The share of it the drawn rings carry where a pixel resolves them; the rest is the ripples between and under them.
const RESOLVED_SHARE: f32 = 0.6;
// How fast a ring's front runs out (m/s): the group speed of centimetre capillary waves.
const RING_SPEED: f32 = 0.24;

struct RainSurface {
    // Slope (∂h/∂x, ∂h/∂z) of the rings the pixel resolves.
    slope: vec2<f32>,
    // Slope variance of the rings it does not.
    variance: f32,
}

// Three uniform hashes of a cell and its drop's life (PCG3D, Jarzynski & Olano 2020).
fn rain_hash(cell: vec2<f32>, life: f32, seed: u32) -> vec3<f32> {
    var v = vec3<u32>(bitcast<u32>(i32(cell.x)), bitcast<u32>(i32(cell.y)), bitcast<u32>(i32(life)) ^ seed);
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

// The slope of one family of drops at `xz` (m) and `time` (s): one drop per `cell` metres a side lands in a share
// `landing` of the cells every `life` seconds, its ring `wavelength` long and `amplitude` high where it starts.
fn rain_rings(xz: vec2<f32>, time: f32, landing: f32, cell: f32, life: f32, wavelength: f32, amplitude: f32, seed: u32)
    -> vec2<f32> {
    let p = xz / cell;
    let base = floor(p);
    let k = TAU / wavelength;
    // A packet of a crest or two.
    let width = wavelength * 0.7;
    var slope = vec2<f32>(0.0);
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let c = base + vec2<f32>(f32(i), f32(j));
            let age = time / life + rain_hash(c, 0.0, seed).x;
            let lived = floor(age);
            let h = rain_hash(c, lived + 1.0, seed);
            if h.z >= landing {
                continue;
            }
            let t = age - lived;
            let d = xz - (c + 0.2 + 0.6 * h.xy) * cell;
            let r = length(d);
            let x = r - RING_SPEED * life * t;
            let fade = (1.0 - t) * sqrt(1.0 - t);
            let envelope = exp(-(x * x) / (width * width)) * fade;
            let rise = amplitude * envelope * (k * cos(k * x) - 2.0 * x / (width * width) * sin(k * x));
            slope += d / max(r, 1e-4) * rise;
        }
    }
    return slope;
}

// Rain's share of the surface at grid point `xz`, `time` seconds on the sea's clock, for a pixel `footprint` metres
// across (the geometric mean of its extents on the water), under `rain` (0 dry … 1 a downpour).
fn rain_surface(xz: vec2<f32>, time: f32, rain: f32, footprint: f32) -> RainSurface {
    let amount = clamp(rain, 0.0, 1.0);
    let variance = RAIN_SLOPE_VARIANCE * amount;
    // Rings show while a pixel spans under half their wavelength (the large family's, 8 cm).
    let shown = 1.0 - smoothstep(0.02, 0.04, footprint);
    var slope = vec2<f32>(0.0);
    if amount > 0.0 && shown > 0.0 {
        // Heavier rain lands more and larger drops.
        let landing = min(0.15 + 0.7 * sqrt(amount), 0.85);
        let size = 0.7 + 0.3 * amount;
        // Large drops' rings, 8 cm crests running out to 0.3 m in 1.2 s, and smaller drops' between them.
        slope = rain_rings(xz, time, landing, 0.55, 1.2, 0.08, 0.0045 * size, 0x5eedu)
            + rain_rings(xz, time + 0.37, landing, 0.32, 0.8, 0.04, 0.0022 * size, 0xd809u);
        slope *= shown;
    }
    return RainSurface(slope, variance * (1.0 - shown * RESOLVED_SHARE));
}
