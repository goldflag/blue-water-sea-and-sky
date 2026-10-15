//! The sea hulls ride, drawn where they ride it: a port of the web game's `ocean/waves/hullSea.ts` (CPU half) and of
//! the web game's `hullSea.ts` (`hullFootprints`, `surfaceContact`).
//!
//! Combat poses every hull on its own long-wave sea (two long-crested sines, renderer-free); the wave field draws an
//! unrelated spectrum with the same height statistics. Near each hull the surface blends the field's long waves (the
//! first cascade low-passed by its mip chain) into the combat sea at the simulation's clock, keeping the field's
//! shorter waves, foam, wake and bow waves on top, and fades back to the field further out. Presentation only: nothing
//! here reaches combat. Only the realistic sea state couples (the art-directed sea is drawn as given).
//!
//! The game draws the combat sea everywhere ([`HullSeaUniforms::set_everywhere`], native only): the coupling's weight is
//! 1 over the whole surface, so the long swell the hulls ride (the first sine is four times the calibrated peak
//! wavelength, 128 m at 9 m/s) runs across the open sea too, every hull at any range sits in the water it rides, and
//! the first cascade's long waves, whose repeat on its tile was the sea's most visible one, are not drawn at all. Away
//! from the hulls each sine is one of a spread of swell components ([`open_sea_swell`]: other directions and lengths
//! about its own, sharing its energy), whose crests interfere into the short crests and uneven sets of a real swell, so
//! two sines do not rule the whole sea ([`swell_height`]); near the hulls it is combat's own, which is what
//! [`everywhere_height`] gives.
//!
//! The pure functions are what the shader (`shaders/waves.wgsl`) evaluates, for tests and CPU callers;
//! [`HullSeaUniforms`] is what it reads.

use bytemuck::{Pod, Zeroable};

use crate::spectrum::GRAVITY;

/// Hulls coupled at once: the nearest to the camera (`HULL_SEA_SLOTS`).
pub const HULL_SEA_SLOTS: usize = 16;
/// Components of the combat sea (`HULL_SEA_WAVES`).
pub const HULL_SEA_WAVES: usize = 2;
/// Components each combat sine adds to the open sea's swell ([`open_sea_swell`]): the first, with 0.84 of the
/// energy, the most, so its crests break up; the second is a fifth of the energy and needs fewer.
pub const SWELL_SPREAD: [usize; HULL_SEA_WAVES] = [12, 4];
/// All of them, as the shader reads them.
pub const SWELL_WAVES: usize = SWELL_SPREAD[0] + SWELL_SPREAD[1];
/// The spread's directions lie within this many radians of their sine's (about 46°), which keeps its crests to a
/// wavelength or two: ±29° still ruled the open sea in lines seen from a few hundred metres up.
pub const SWELL_TURN: f64 = 0.8;
/// The spread's wavelengths lie within e^±this of their sine's (0.70–1.42 times), so its sets build and fade within a
/// few wavelengths.
pub const SWELL_STRETCH: f64 = 0.35;
/// Full coupling out to this share of the hull length from its centre line, and at least this share of the longest
/// wave (`HULL_REACH`, `WAVE_REACH`).
pub const HULL_REACH: f64 = 0.3;
pub const WAVE_REACH: f64 = 0.25;
/// The coupling fades over at least a hull length and the longest wavelength (`FADE_HULLS`, `FADE_WAVES`).
pub const FADE_HULLS: f64 = 1.0;
pub const FADE_WAVES: f64 = 1.0;
/// Drawn waves at least this share of the longest combat wave long count as long (`LONG_WAVES`).
pub const LONG_WAVES: f64 = 0.5;
/// Long waves left beside a hull count this many times the shorter waves taken from around it (`SPLIT_LEAK_WEIGHT`).
pub const SPLIT_LEAK_WEIGHT: f64 = 2.0;
/// Split levels tried, in mip levels (`SPLIT_STEP`).
const SPLIT_STEP: f64 = 0.25;
/// Lattice cells per edge `cascade_modes` gathers a cascade's variance into (`MODE_BINS`).
const MODE_BINS: usize = 64;

/// One long-crested sine of the sea hulls ride (`HullSeaWave`): height a·sin(k·(x cos θ + z sin θ) − ω·t + φ),
/// k = 2π/λ, ω = √(g·k). `direction` is the direction of travel, radians from +X toward +Z.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HullSeaWave {
    pub amplitude: f64,
    pub wavelength: f64,
    pub direction: f64,
    pub phase: f64,
}

/// A drawn hull the surface around it agrees with (`HullFootprint`): the centre of its waterplane (world metres), its
/// bow direction (radians from +X toward +Z), length and beam, and contact (1 afloat, 0 for a hull none of which
/// reaches the surface).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HullFootprint {
    pub x: f64,
    pub z: f64,
    pub bearing: f64,
    pub length: f64,
    pub beam: f64,
    pub contact: f64,
}

/// One hull's reach as the shader reads it (`HullSeaSlot`): centre, bow axis, half length, full and zero coupling
/// radii from the centre line, and contact.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HullSeaSlot {
    pub x: f64,
    pub z: f64,
    pub axis_x: f64,
    pub axis_z: f64,
    pub half: f64,
    pub inner: f64,
    pub outer: f64,
    pub contact: f64,
}

/// Longest wavelength (m) of the combat sea; 0 without components (`hullSeaWavelength`).
pub fn hull_sea_wavelength(waves: &[HullSeaWave]) -> f64 {
    waves
        .iter()
        .fold(0.0, |longest, wave| longest.max(wave.wavelength))
}

/// `hullSeaSlot`.
pub fn hull_sea_slot(hull: &HullFootprint, wavelength: f64) -> HullSeaSlot {
    let inner = (HULL_REACH * hull.length)
        .max(0.75 * hull.beam)
        .max(WAVE_REACH * wavelength);
    HullSeaSlot {
        x: hull.x,
        z: hull.z,
        axis_x: hull.bearing.cos(),
        axis_z: hull.bearing.sin(),
        half: hull.length / 2.0,
        inner,
        outer: inner
            + (FADE_HULLS * hull.length)
                .max(FADE_WAVES * wavelength)
                .max(1.0),
        contact: hull.contact.clamp(0.0, 1.0),
    }
}

/// Coupling weight at (x, z), 0–1, and its gradient (`hullSeaWeight`): each hull's smoothstep of the distance from its
/// centre line, joined as 1 − Π(1 − wᵢ).
pub fn hull_sea_weight(slots: &[HullSeaSlot], x: f64, z: f64) -> (f64, f64, f64) {
    let (mut keep, mut gx, mut gz) = (1.0, 0.0, 0.0);
    for slot in slots {
        let (rx, rz) = (x - slot.x, z - slot.z);
        let t = (rx * slot.axis_x + rz * slot.axis_z).clamp(-slot.half, slot.half);
        let (qx, qz) = (rx - slot.axis_x * t, rz - slot.axis_z * t);
        let d = qx.hypot(qz);
        let span = (slot.outer - slot.inner).max(1e-3);
        let s = ((d - slot.inner) / span).clamp(0.0, 1.0);
        let w = slot.contact * (1.0 - s * s * (3.0 - 2.0 * s));
        let rate = -slot.contact * 6.0 * s * (1.0 - s) / span / d.max(1e-3);
        gx = gx * (1.0 - w) - rate * qx * keep;
        gz = gz * (1.0 - w) - rate * qz * keep;
        keep *= 1.0 - w;
    }
    (1.0 - keep, -gx, -gz)
}

/// Coefficients of the blend at weight w (`hullSeaBlend`): height = field + lowpass·(field's long waves) + sea·(combat
/// sea), both scaled by 1/√((1 − w)² + w²); the last two are d/dw.
pub fn hull_sea_blend(w: f64) -> (f64, f64, f64, f64) {
    let n = 1.0 / (1.0 - 2.0 * w + 2.0 * w * w).sqrt();
    let dn = (1.0 - 2.0 * w) * n * n * n;
    (n * (1.0 - w) - 1.0, n * w, dn * (1.0 - w) - n, dn * w + n)
}

/// Height (m) of the combat sea at (x, z) and `time` (s) (`hullSeaHeight`).
pub fn hull_sea_height(waves: &[HullSeaWave], time: f64, x: f64, z: f64) -> f64 {
    waves
        .iter()
        .map(|wave| {
            let k = 2.0 * std::f64::consts::PI / wave.wavelength;
            wave.amplitude
                * (k * (x * wave.direction.cos() + z * wave.direction.sin())
                    - (GRAVITY * k).sqrt() * time
                    + wave.phase)
                    .sin()
        })
        .sum()
}

/// A component as the shader reads it (`packHullSeaWave`): wave vector, amplitude and phase at `time` about `origin`,
/// reduced to one turn in double precision so f32 keeps the phase at any battle time and range.
pub fn pack_hull_sea_wave(wave: &HullSeaWave, time: f64, origin_x: f64, origin_z: f64) -> [f64; 4] {
    if wave.wavelength.is_nan() || wave.wavelength <= 0.0 || !wave.wavelength.is_finite() {
        return [0.0; 4];
    }
    let k = 2.0 * std::f64::consts::PI / wave.wavelength;
    let (kx, kz) = (k * wave.direction.cos(), k * wave.direction.sin());
    let turn = 2.0 * std::f64::consts::PI;
    let phase = kx * origin_x + kz * origin_z - (GRAVITY * k).sqrt() * time + wave.phase;
    [kx, kz, wave.amplitude, (phase % turn + turn) % turn]
}

/// The open sea's swell beside combat's sines: for each, [`SWELL_SPREAD`] more components, directions spread evenly
/// within [`SWELL_TURN`] of its own and wavelengths within [`SWELL_STRETCH`] (a golden-ratio sequence, so no two match),
/// at scattered phases, each with an equal share of the sine's energy; and each sine's own share away from the hulls.
/// Their sum is a short-crested swell of the sines' height; at a hull the sines take their energy back
/// ([`swell_height`]).
pub fn open_sea_swell(
    waves: &[HullSeaWave],
) -> ([HullSeaWave; SWELL_WAVES], [f64; HULL_SEA_WAVES]) {
    const GOLDEN: f64 = 0.618_033_988_749_895;
    const PLASTIC: f64 = 0.754_877_666_246_693;
    let mut spread = [HullSeaWave {
        amplitude: 0.0,
        wavelength: 0.0,
        direction: 0.0,
        phase: 0.0,
    }; SWELL_WAVES];
    let mut shares = [1.0; HULL_SEA_WAVES];
    let mut next = 0;
    for (j, &count) in SWELL_SPREAD.iter().enumerate() {
        shares[j] = 1.0 / (count + 1) as f64;
        let Some(wave) = waves.get(j) else {
            next += count;
            continue;
        };
        for m in 0..count {
            let turn = ((m as f64 + 0.5) / count as f64) * 2.0 - 1.0;
            let stretch = ((0.5 + m as f64 * GOLDEN).fract()) * 2.0 - 1.0;
            let scatter = (m as f64 * PLASTIC + j as f64 * GOLDEN).fract();
            spread[next] = HullSeaWave {
                amplitude: wave.amplitude * shares[j].sqrt(),
                wavelength: wave.wavelength * (SWELL_STRETCH * stretch).exp(),
                direction: wave.direction + SWELL_TURN * turn,
                phase: wave.phase + 2.0 * std::f64::consts::PI * scatter,
            };
            next += 1;
        }
    }
    (spread, shares)
}

/// The swell drawn at a point `free` (0–1) away from the hulls (the shader's `swell_freedom`): combat's sines, and
/// with distance the spread ([`open_sea_swell`]) taking an even share of each sine's energy, so the variance holds at
/// every point. At 0, [`hull_sea_height`].
pub fn swell_height(waves: &[HullSeaWave], free: f64, time: f64, x: f64, z: f64) -> f64 {
    let (spread, shares) = open_sea_swell(waves);
    let sines: f64 = waves
        .iter()
        .zip(shares)
        .map(|(wave, share)| {
            swell_keep(share, free) * hull_sea_height(std::slice::from_ref(wave), time, x, z)
        })
        .sum();
    let spread: f64 = spread
        .iter()
        .filter(|wave| wave.wavelength > 0.0)
        .map(|wave| hull_sea_height(std::slice::from_ref(wave), time, x, z))
        .sum();
    sines + free * spread
}

/// A combat sine's amplitude scale at `free` away from the hulls: all of it at a hull, its `share` of the spread's
/// energy on the open sea (the shader's `swell_keep`).
pub fn swell_keep(share: f64, free: f64) -> f64 {
    (1.0 - free * free * (1.0 - share)).max(0.0).sqrt()
}

/// A sine's share of the open sea's swell while the hulls are let go by `racing` (0 coupled … 1 let go,
/// [`HullSeaUniforms::set`]): none once they are, its energy all the spread's.
pub fn racing_share(share: f64, racing: f64) -> f64 {
    share * (1.0 - racing.clamp(0.0, 1.0))
}

/// The gain on a sine's spread while the hulls are let go by `racing`: the energy the sine gives up
/// ([`racing_share`]) shared among its spread, whose amplitudes [`open_sea_swell`] sized for its `share`.
pub fn racing_spread_gain(share: f64, racing: f64) -> f64 {
    ((1.0 - racing_share(share, racing)) / (1.0 - share).max(1e-9)).sqrt()
}

/// (kx, kz, variance) triples of a cascade's spectrum gathered onto at most 64² cells, each at its variance-weighted
/// wave vector (`cascadeModes`). `amplitudes` is the cascade's packed spectrum (re, im, frequency multiple, ·) on an
/// n×n lattice of a `size` m tile.
pub fn cascade_modes(amplitudes: &[f32], n: usize, size: f64) -> Vec<f32> {
    let bin = n.div_ceil(MODE_BINS).max(1);
    let cells = n.div_ceil(bin);
    let dk = 2.0 * std::f64::consts::PI / size;
    let mut sums = vec![0f64; cells * cells * 3];
    for (texel, value) in amplitudes.as_chunks::<4>().0.iter().enumerate() {
        if value[2] == 0.0 {
            continue;
        }
        let (x, z) = (texel % n, texel / n);
        let variance = f64::from(value[0]).powi(2) + f64::from(value[1]).powi(2);
        let half = (n / 2) as i64;
        let nx = if x < n / 2 {
            x as i64
        } else {
            x as i64 - n as i64
        };
        let nz = if z < n / 2 {
            z as i64
        } else {
            z as i64 - n as i64
        };
        let cell = ((((nz + half) as usize) / bin) * cells + ((nx + half) as usize) / bin) * 3;
        sums[cell] += nx as f64 * dk * variance;
        sums[cell + 1] += nz as f64 * dk * variance;
        sums[cell + 2] += variance;
    }
    let mut modes = Vec::new();
    for cell in sums.as_chunks::<3>().0 {
        if cell[2] > 0.0 {
            modes.extend([
                (cell[0] / cell[2]) as f32,
                (cell[1] / cell[2]) as f32,
                cell[2] as f32,
            ]);
        }
    }
    modes
}

/// Transfer of a mip box `width` metres wide, read bilinearly, at wave vector (kx, kz): per axis sinc³(k·width/2)
/// (`boxTransfer`).
pub fn box_transfer(kx: f64, kz: f64, width: f64) -> f64 {
    let sinc = |x: f64| if x.abs() < 1e-6 { 1.0 } else { x.sin() / x };
    (sinc(kx * width / 2.0) * sinc(kz * width / 2.0)).powi(3)
}

/// The mip level of the first cascade whose box low-pass best splits its waves into the long ones the coupling replaces
/// and the rest, or `None` when leaving every drawn wave in place does better (`splitLevel`). `modes` holds (kx, kz,
/// variance) triples, `texel` the cascade's texel in metres, `top` its coarsest mip level.
pub fn split_level(modes: &[f32], texel: f64, top: f64, wavelength: f64) -> Option<f64> {
    let long = 2.0 * std::f64::consts::PI / (LONG_WAVES * wavelength);
    let mode = |m: &[f32; 3]| (f64::from(m[0]), f64::from(m[1]), f64::from(m[2]));
    let long_variance: f64 = modes
        .as_chunks::<3>()
        .0
        .iter()
        .map(mode)
        .filter(|(kx, kz, _)| kx.hypot(*kz) <= long)
        .map(|(_, _, v)| v)
        .sum();
    let mut best = None;
    let mut cost = SPLIT_LEAK_WEIGHT * long_variance;
    let mut level = 0.0;
    while level <= top + 1e-9 {
        let width = texel * 2f64.powf(level);
        let mut total = 0.0;
        for m in modes.as_chunks::<3>().0 {
            if total >= cost {
                break;
            }
            let (kx, kz, v) = mode(m);
            let t = box_transfer(kx, kz, width);
            total += if kx.hypot(kz) <= long {
                SPLIT_LEAK_WEIGHT * v * (1.0 - t).powi(2)
            } else {
                v * t * t
            };
        }
        if total < cost {
            cost = total;
            best = Some(level);
        }
        level += SPLIT_STEP;
    }
    best
}

/// The drawn height near hulls (`coupledHeight`): `field` (all cascades) with its long waves `lowpass` blended into
/// the combat sea. With the combat sea drawn everywhere, [`everywhere_height`].
pub fn coupled_height(
    field: f64,
    lowpass: f64,
    slots: &[HullSeaSlot],
    waves: &[HullSeaWave],
    time: f64,
    x: f64,
    z: f64,
) -> f64 {
    let (lp, sea, _, _) = hull_sea_blend(hull_sea_weight(slots, x, z).0);
    field + lp * lowpass + sea * hull_sea_height(waves, time, x, z)
}

/// The drawn height with the combat sea drawn everywhere ([`HullSeaUniforms::set_everywhere`]): at every point what
/// [`coupled_height`] gives at a hull, `field` with its long waves `lowpass` given way to the combat sea.
pub fn everywhere_height(
    field: f64,
    lowpass: f64,
    waves: &[HullSeaWave],
    time: f64,
    x: f64,
    z: f64,
) -> f64 {
    field - lowpass + hull_sea_height(waves, time, x, z)
}

/// How much of a hull reaches the surface, 0–1 (`surfaceContact` in the web game's `hullSea.ts`): all of it while its highest
/// point (a ship's deck edge, `top` metres above her waterline; a submarine's periscope head) stands above the
/// still-water line, none once it is deeper than the sea's `amplitude` and a metre more. `mean` is the hull origin's
/// height less its wave heave.
pub fn surface_contact(mean: f64, top: f64, amplitude: f64) -> f64 {
    let edge0 = -(amplitude.max(0.0) + 1.0);
    let t = ((mean + top - edge0) / (0.0 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A drawn hull as [`hull_footprints`] reads it: her pose (world metres, heading clockwise from −Z), her waterplane's
/// extents about her origin (`wakeHull`), and her contact with the surface ([`surface_contact`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeaHull {
    pub x: f64,
    pub z: f64,
    pub heading: f64,
    pub length: f64,
    pub beam: f64,
    pub center_x: f64,
    pub center_z: f64,
    pub contact: f64,
}

/// The drawn hulls the sea around them follows (`hullFootprints`): the nearest `slots` to the camera, their waterplanes'
/// centres and bow bearings in the ocean's frame. With more hulls than slots, those near the cut fade out.
pub fn hull_footprints(hulls: &[SeaHull], camera: (f64, f64), slots: usize) -> Vec<HullFootprint> {
    let mut out = Vec::new();
    hull_footprints_into(hulls, camera, slots, &mut out, &mut Vec::new());
    out
}

/// [`hull_footprints`] into `out`, ranking in `order` (both kept between frames, so a frame allocates nothing).
pub fn hull_footprints_into(
    hulls: &[SeaHull],
    camera: (f64, f64),
    slots: usize,
    out: &mut Vec<HullFootprint>,
    order: &mut Vec<(f64, usize)>,
) {
    let smooth = |edge0: f64, edge1: f64, x: f64| {
        let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    order.clear();
    order.extend(
        hulls
            .iter()
            .enumerate()
            .filter(|(_, hull)| hull.x.is_finite() && hull.z.is_finite())
            .map(|(i, hull)| ((hull.x - camera.0).hypot(hull.z - camera.1), i)),
    );
    // A stable sort, as JavaScript's: equally distant hulls keep their order.
    order.sort_by(|a, b| a.0.total_cmp(&b.0));
    let cut = order
        .get(slots)
        .map_or(f64::INFINITY, |&(distance, _)| distance);
    out.clear();
    out.extend(order.iter().take(slots).map(|&(distance, i)| {
        let hull = &hulls[i];
        let (sin, cos) = hull.heading.sin_cos();
        let rank = if cut.is_infinite() {
            1.0
        } else {
            1.0 - smooth(0.75 * cut, cut, distance)
        };
        HullFootprint {
            x: hull.x + cos * hull.center_x - sin * hull.center_z,
            z: hull.z + sin * hull.center_x + cos * hull.center_z,
            bearing: (-cos).atan2(sin),
            length: hull.length,
            beam: hull.beam,
            contact: hull.contact * rank,
        }
    }));
}

/// `HullSeaUniforms` in `shaders/waves.wgsl` (the web's `HullSea` uniforms in one block): per hull (centre x, z, bow
/// axis x, z) and (half length, inner, outer, contact); per component (kx, kz, amplitude, phase about `origin`), combat's
/// then the open sea's spread ([`open_sea_swell`]); then (origin x, z, the split mip level, 1 while the first cascade's
/// long waves are replaced), (hulls coupled, 1 while the combat sea is drawn everywhere, each sine's share of the
/// open sea's swell) and (how far the hulls are let go, [`HullSeaUniforms::set`]'s `racing`; yzw unused).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct HullSeaUniforms {
    pub slots: [[f32; 4]; HULL_SEA_SLOTS * 2],
    pub waves: [[f32; 4]; HULL_SEA_WAVES],
    pub swell: [[f32; 4]; SWELL_WAVES],
    pub origin: [f32; 4],
    pub count: [f32; 4],
    pub clock: [f32; 4],
}

impl Default for HullSeaUniforms {
    fn default() -> Self {
        Self::zeroed()
    }
}

impl HullSeaUniforms {
    /// Couple `hulls` (at most [`HULL_SEA_SLOTS`]) to the sea `waves` at `time` about `origin` (`HullSea.set`), keeping
    /// the split. No components or no hulls: no coupling. The open sea's spread runs on `swell_time`, the sea's own
    /// clock: its components are independent of the sines, so any clock draws the same sea, and one held to the sea's
    /// presentation rate keeps the swell from racing while the simulation's runs fast. There the sines would race at
    /// the hulls, so `racing` (0 … 1) lets the hulls go: the sines give their energy to the spread everywhere.
    pub fn set(
        &mut self,
        waves: &[HullSeaWave],
        time: f64,
        swell_time: f64,
        racing: f64,
        hulls: &[HullFootprint],
        origin: (f64, f64),
    ) {
        assert!(
            waves.len() <= HULL_SEA_WAVES,
            "at most {HULL_SEA_WAVES} hull sea components"
        );
        let wavelength = hull_sea_wavelength(waves);
        let count = if wavelength > 0.0 {
            hulls.len().min(HULL_SEA_SLOTS)
        } else {
            0
        };
        self.origin[0] = origin.0 as f32;
        self.origin[1] = origin.1 as f32;
        for (i, value) in self.waves.iter_mut().enumerate() {
            *value = waves
                .get(i)
                .map_or([0.0; 4], |wave| {
                    pack_hull_sea_wave(wave, time, origin.0, origin.1)
                })
                .map(|v| v as f32);
        }
        let racing = racing.clamp(0.0, 1.0);
        let (spread, shares) = open_sea_swell(waves);
        let gains = shares.map(|share| racing_spread_gain(share, racing));
        let mut sine = SWELL_SPREAD
            .iter()
            .enumerate()
            .flat_map(|(j, &count)| std::iter::repeat_n(j, count));
        for (value, wave) in self.swell.iter_mut().zip(&spread) {
            let gain = sine.next().map_or(1.0, |j| gains[j]);
            let mut packed = pack_hull_sea_wave(wave, swell_time, origin.0, origin.1);
            packed[2] *= gain;
            *value = packed.map(|v| v as f32);
        }
        self.count[2] = shares[0] as f32;
        self.count[3] = shares[1] as f32;
        self.clock[0] = racing as f32;
        for (i, hull) in hulls.iter().take(count).enumerate() {
            let slot = hull_sea_slot(hull, wavelength);
            self.slots[i * 2] = [slot.x, slot.z, slot.axis_x, slot.axis_z].map(|v| v as f32);
            self.slots[i * 2 + 1] =
                [slot.half, slot.inner, slot.outer, slot.contact].map(|v| v as f32);
        }
        self.count[0] = count as f32;
    }

    /// The split chosen for the first cascade (`chooseSplit`): its mip level, or none replaced (the coarsest level,
    /// and the combat sea adds to every drawn wave).
    pub fn set_split(&mut self, level: Option<f64>, top: f64) {
        self.origin[2] = level.unwrap_or(top) as f32;
        self.origin[3] = if level.is_some() { 1.0 } else { 0.0 };
    }

    /// Draw the combat sea across the whole surface (native only): the coupling's weight is 1 wherever the waves are
    /// read, hulls or none, so the long swell the hulls ride is the open sea's too ([`everywhere_height`] at the hulls,
    /// [`swell_height`] away from them). Only while
    /// the last [`HullSeaUniforms::set`] gave the sea components; call it after each.
    pub fn set_everywhere(&mut self, on: bool) {
        let waves = self.waves.iter().any(|wave| wave[2] != 0.0);
        self.count[1] = if on && waves { 1.0 } else { 0.0 };
    }

    /// Whether the combat sea is drawn everywhere.
    pub fn everywhere(&self) -> bool {
        self.count[1] > 0.5
    }

    /// Drop every hull (`clear`).
    pub fn clear(&mut self) {
        self.count[0] = 0.0;
    }

    /// Hulls coupled.
    pub fn count(&self) -> usize {
        self.count[0] as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waves() -> [HullSeaWave; 2] {
        [
            HullSeaWave {
                amplitude: 2.86,
                wavelength: 249.0,
                direction: 0.6,
                phase: 1.3,
            },
            HullSeaWave {
                amplitude: 1.23,
                wavelength: 142.0,
                direction: 1.4,
                phase: 3.3,
            },
        ]
    }

    const BATTLESHIP: HullFootprint = HullFootprint {
        x: 120.0,
        z: -40.0,
        bearing: 0.3,
        length: 250.0,
        beam: 36.0,
        contact: 1.0,
    };

    fn slot() -> HullSeaSlot {
        hull_sea_slot(&BATTLESHIP, hull_sea_wavelength(&waves()))
    }

    fn at(slot: &HullSeaSlot, along: f64, across: f64) -> (f64, f64) {
        (
            slot.x + slot.axis_x * along - slot.axis_z * across,
            slot.z + slot.axis_z * along + slot.axis_x * across,
        )
    }

    fn close(a: f64, b: f64, digits: i32) -> bool {
        (a - b).abs() < 0.5 * 10f64.powi(-digits)
    }

    #[test]
    fn the_open_sea_swell_keeps_each_sines_energy_and_gives_it_back_at_a_hull() {
        let waves = waves();
        let (spread, shares) = open_sea_swell(&waves);
        for free in [0.0, 0.3, 0.7, 1.0] {
            let mut next = 0;
            for (j, &count) in SWELL_SPREAD.iter().enumerate() {
                let kept = swell_keep(shares[j], free) * waves[j].amplitude;
                let added: f64 = spread[next..next + count]
                    .iter()
                    .map(|wave| (free * wave.amplitude).powi(2))
                    .sum();
                assert!(close(kept * kept + added, waves[j].amplitude.powi(2), 9));
                next += count;
            }
        }
        for (x, z, time) in [
            (0.0, 0.0, 0.0),
            (310.0, -45.0, 12.5),
            (-1200.0, 800.0, 301.0),
        ] {
            assert!(close(
                swell_height(&waves, 0.0, time, x, z),
                hull_sea_height(&waves, time, x, z),
                12
            ));
        }
    }

    #[test]
    fn the_open_sea_swell_spreads_about_each_sine_and_its_crests_are_short() {
        let waves = waves();
        let (spread, shares) = open_sea_swell(&waves);
        let mut next = 0;
        for (j, &count) in SWELL_SPREAD.iter().enumerate() {
            let members = &spread[next..next + count];
            for wave in members {
                assert!((wave.direction - waves[j].direction).abs() <= SWELL_TURN + 1e-12);
                assert!(
                    (wave.wavelength / waves[j].wavelength).ln().abs() <= SWELL_STRETCH + 1e-12
                );
            }
            for (a, first) in members.iter().enumerate() {
                for second in &members[a + 1..] {
                    assert!(
                        (first.wavelength - second.wavelength).abs() > 1e-3 * waves[j].wavelength
                    );
                }
            }
            next += count;
        }
        // The correlation three wavelengths along the first sine's crests (Σa²cos(k·d)/Σa²): 1 for the sine alone,
        // under a half on the open sea.
        let correlation = |components: &[(f64, &HullSeaWave)]| {
            let along = waves[0].direction + std::f64::consts::FRAC_PI_2;
            let d = 3.0 * waves[0].wavelength;
            let (mut sum, mut total) = (0.0, 0.0);
            for &(scale, wave) in components {
                let k = 2.0 * std::f64::consts::PI / wave.wavelength;
                let power = (scale * wave.amplitude).powi(2);
                sum += power * (k * d * (wave.direction - along).cos()).cos();
                total += power;
            }
            sum / total
        };
        assert!(close(correlation(&[(1.0, &waves[0])]), 1.0, 12));
        let open: Vec<(f64, &HullSeaWave)> = waves
            .iter()
            .zip(shares)
            .map(|(wave, share)| (swell_keep(share, 1.0), wave))
            .chain(spread.iter().map(|wave| (1.0, wave)))
            .collect();
        assert!(correlation(&open).abs() < 0.5, "{}", correlation(&open));
    }

    #[test]
    fn a_hull_is_fully_coupled_along_its_waterline_and_not_at_all_beyond_the_fade() {
        let s = slot();
        assert!(close(
            s.inner,
            (HULL_REACH * 250.0).max(WAVE_REACH * 249.0),
            9
        ));
        assert!(close(
            s.outer - s.inner,
            (FADE_HULLS * 250.0).max(FADE_WAVES * 249.0),
            9
        ));
        for (along, across) in [
            (0.0, 0.0),
            (125.0, 0.0),
            (-125.0, 0.0),
            (0.0, 18.0),
            (125.0 + s.inner, 0.0),
            (-60.0, -s.inner),
        ] {
            let (x, z) = at(&s, along, across);
            assert!(close(hull_sea_weight(&[s], x, z).0, 1.0, 12));
        }
        for (along, across) in [
            (125.0 + s.outer, 0.0),
            (0.0, s.outer),
            (-125.0 - s.outer - 1.0, 30.0),
            (900.0, 900.0),
        ] {
            let (x, z) = at(&s, along, across);
            assert_eq!(hull_sea_weight(&[s], x, z).0, 0.0);
        }
    }

    #[test]
    fn the_weight_gradient_matches_finite_differences() {
        let second = hull_sea_slot(
            &HullFootprint {
                x: 420.0,
                z: 60.0,
                bearing: 2.1,
                length: 110.0,
                beam: 11.0,
                contact: 0.8,
            },
            hull_sea_wavelength(&waves()),
        );
        for slots in [vec![slot()], vec![slot(), second]] {
            for i in 0..400 {
                let x = -300.0 + ((i * 97) % 1100) as f64;
                let z = -500.0 + ((i * 61) % 1000) as f64;
                let h = 1e-3;
                let (w, dx, dz) = hull_sea_weight(&slots, x, z);
                assert!((0.0..=1.0).contains(&w));
                let fx = (hull_sea_weight(&slots, x + h, z).0
                    - hull_sea_weight(&slots, x - h, z).0)
                    / (2.0 * h);
                let fz = (hull_sea_weight(&slots, x, z + h).0
                    - hull_sea_weight(&slots, x, z - h).0)
                    / (2.0 * h);
                assert!(close(dx, fx, 6) && close(dz, fz, 6));
            }
        }
    }

    #[test]
    fn the_blend_keeps_the_variance_of_seas_of_equal_height() {
        let (lp, sea, _, _) = hull_sea_blend(0.0);
        assert_eq!((lp, sea), (0.0, 0.0));
        let (lp, sea, _, _) = hull_sea_blend(1.0);
        assert!(close(lp, -1.0, 12) && close(sea, 1.0, 12));
        for i in 0..=20 {
            let w = f64::from(i) * 0.05;
            let (lp, sea, dlp, dsea) = hull_sea_blend(w);
            assert!(close((1.0 + lp).powi(2) + sea * sea, 1.0, 12));
            let h = 1e-6;
            assert!(close(
                dlp,
                (hull_sea_blend(w + h).0 - hull_sea_blend(w - h).0) / (2.0 * h),
                6
            ));
            assert!(close(
                dsea,
                (hull_sea_blend(w + h).1 - hull_sea_blend(w - h).1) / (2.0 * h),
                6
            ));
        }
    }

    #[test]
    fn packed_components_reproduce_the_sea_in_f32() {
        let origin = (15_000.0, -22_000.0);
        for time in [0.0, 61.25, 1800.5, 3600.0] {
            for (dx, dz) in [(0.0f64, 0.0f64), (310.0, -470.0), (-900.0, 250.0)] {
                let shader: f32 = waves()
                    .iter()
                    .map(|wave| {
                        let [kx, kz, a, p] =
                            pack_hull_sea_wave(wave, time, origin.0, origin.1).map(|v| v as f32);
                        a * (kx * dx as f32 + kz * dz as f32 + p).sin()
                    })
                    .sum();
                let exact = hull_sea_height(&waves(), time, origin.0 + dx, origin.1 + dz);
                assert!((f64::from(shader) - exact).abs() < 2e-4, "{shader} {exact}");
            }
        }
        let flat = HullSeaWave {
            amplitude: 1.0,
            wavelength: 0.0,
            direction: 0.0,
            phase: 0.0,
        };
        assert_eq!(pack_hull_sea_wave(&flat, 5.0, 0.0, 0.0), [0.0; 4]);
    }

    #[test]
    fn at_the_hull_the_drawn_height_is_the_combat_sea_plus_the_shorter_waves() {
        let s = slot();
        let time = 734.5;
        for (along, across) in [(0.0, 0.0), (110.0, 10.0), (-120.0, -15.0), (60.0, s.inner)] {
            let (x, z) = at(&s, along, across);
            let sea = hull_sea_height(&waves(), time, x, z);
            assert!(close(
                coupled_height(3.2, 3.2, &[s], &waves(), time, x, z),
                sea,
                12
            ));
            assert!(close(
                coupled_height(3.6, 3.2, &[s], &waves(), time, x, z),
                sea + 0.4,
                12
            ));
        }
        let (x, z) = at(&s, 0.0, s.outer + 10.0);
        assert_eq!(coupled_height(-1.7, -1.5, &[s], &waves(), time, x, z), -1.7);
        // Drawn everywhere, the sea far from any hull is what the hull's own is.
        let sea = hull_sea_height(&waves(), time, x, z);
        assert!(close(
            everywhere_height(-1.7, -1.5, &waves(), time, x, z),
            sea - 0.2,
            12
        ));
    }

    #[test]
    fn hulls_let_go_give_the_sines_energy_to_the_spread_on_the_seas_own_clock() {
        let waves = waves();
        let (spread, shares) = open_sea_swell(&waves);
        for racing in [0.0, 0.4, 1.0] {
            let mut next = 0;
            for (j, &count) in SWELL_SPREAD.iter().enumerate() {
                // On the open sea (free 1): the sine's kept share and its spread, gained, hold its energy.
                let share = racing_share(shares[j], racing);
                let kept = swell_keep(share, 1.0) * waves[j].amplitude;
                let gain = racing_spread_gain(shares[j], racing);
                let added: f64 = spread[next..next + count]
                    .iter()
                    .map(|wave| (gain * wave.amplitude).powi(2))
                    .sum();
                assert!(close(kept * kept + added, waves[j].amplitude.powi(2), 9));
                next += count;
            }
        }
        // Let go, a sine keeps nothing; coupled, its share.
        assert_eq!(racing_share(shares[0], 1.0), 0.0);
        assert_eq!(racing_share(shares[0], 0.0), shares[0]);
        // The spread is packed at the sea's own clock, the sines at the simulation's, the spread gained.
        let mut uniforms = HullSeaUniforms::default();
        uniforms.set(&waves, 30.0, 12.0, 1.0, &[], (0.0, 0.0));
        let sine = pack_hull_sea_wave(&waves[0], 30.0, 0.0, 0.0);
        assert!(close(f64::from(uniforms.waves[0][3]), sine[3], 5));
        let first = pack_hull_sea_wave(&spread[0], 12.0, 0.0, 0.0);
        assert!(close(f64::from(uniforms.swell[0][3]), first[3], 5));
        let gain = racing_spread_gain(shares[0], 1.0);
        assert!(close(f64::from(uniforms.swell[0][2]), first[2] * gain, 5));
        assert_eq!(uniforms.clock[0], 1.0);
    }

    #[test]
    fn the_sea_is_drawn_everywhere_only_with_components() {
        let mut uniforms = HullSeaUniforms::default();
        uniforms.set(&waves(), 12.0, 12.0, 0.0, &[], (0.0, 0.0));
        assert_eq!(uniforms.count(), 0);
        uniforms.set_everywhere(true);
        assert!(uniforms.everywhere());
        uniforms.set_everywhere(false);
        assert!(!uniforms.everywhere());
        uniforms.set(&[], 12.0, 12.0, 0.0, &[BATTLESHIP], (0.0, 0.0));
        uniforms.set_everywhere(true);
        assert!(!uniforms.everywhere());
    }

    #[test]
    fn the_split_takes_the_long_waves_and_leaves_a_sea_of_shorter_waves_whole() {
        let k = |wavelength: f64| (2.0 * std::f64::consts::PI / wavelength) as f32;
        assert_eq!(
            split_level(&[k(400.0), 0.0, 1.0, 0.0, k(300.0), 2.0], 4.0, 4.0, 250.0),
            Some(0.0)
        );
        assert_eq!(
            split_level(&[k(40.0), 0.0, 1.0, k(30.0), k(30.0), 1.0], 4.0, 4.0, 250.0),
            None
        );
    }

    #[test]
    fn the_split_of_the_drawn_seas_matches_the_web() {
        use crate::QualityTier;
        use crate::sea_state::{drawn_sea, sea_state_cascades};
        use crate::spectrum::{WaveParameters, build_spectrum};
        // Bun on hullSea.ts (the storm seas of hullSea.test.ts at High): 439 modes each; the realistic seas split at
        // level 1.75, the art-directed 25 m/s sea at the coarsest level 4, its 30 m/s sea not at all.
        for (wind, height, peak, realism, level) in [
            (25.0, 8.8, 62.2, true, Some(1.75)),
            (25.0, 8.8, 62.2, false, Some(4.0)),
            (30.0, 11.3, 74.7, true, Some(1.75)),
            (30.0, 11.3, 74.7, false, None),
        ] {
            let params = WaveParameters {
                significant_height: height,
                wind_speed: wind,
                wind_direction: 0.6,
                peak_wavelength: peak,
                choppiness: 1.55,
                gamma: 2.6,
                directional_sharpness: 0.8,
                seed: 7.0,
                equilibrium_range: false,
            };
            let drawn = drawn_sea(&params, realism);
            let tier = QualityTier::High.cascades();
            let cascades = if realism {
                sea_state_cascades(&tier, drawn.peak_wavelength)
            } else {
                tier
            };
            let first = &build_spectrum(&cascades, &drawn).cascades[0];
            let n = first.resolution as usize;
            let modes = cascade_modes(&first.amplitudes, n, first.size);
            assert_eq!(modes.len() / 3, 439);
            let total: f64 = modes
                .as_chunks::<3>()
                .0
                .iter()
                .map(|m| f64::from(m[2]))
                .sum();
            assert!((total - first.variance).abs() < 1e-6 * first.variance.max(1.0));
            assert_eq!(
                split_level(&modes, first.size / n as f64, 4.0, 4.0 * peak),
                level
            );
        }
    }

    #[test]
    fn footprints_take_the_nearest_hulls_and_fade_those_at_the_cut() {
        let hull = |x: f64| SeaHull {
            x,
            z: 0.0,
            heading: 0.0,
            length: 100.0,
            beam: 10.0,
            center_x: 0.0,
            center_z: 5.0,
            contact: 1.0,
        };
        let hulls: Vec<SeaHull> = (0..20).map(|i| hull(f64::from(i) * 100.0)).collect();
        let feet = hull_footprints(&hulls, (0.0, 0.0), HULL_SEA_SLOTS);
        assert_eq!(feet.len(), HULL_SEA_SLOTS);
        // Heading 0 points the bow toward −Z: bearing −π/2; the waterplane's centre 5 m astern of the origin lies at +Z.
        assert!(close(feet[0].bearing, -std::f64::consts::FRAC_PI_2, 12));
        assert!(close(feet[0].z, 5.0, 12));
        assert_eq!(feet[0].contact, 1.0);
        // The 17th hull is the cut at 1600 m: the 16th at 1500 m fades within 1200–1600 m.
        assert!(feet[15].contact > 0.0 && feet[15].contact < 1.0);
        assert!(close(surface_contact(0.0, 1.0, 2.0), 1.0, 12));
        assert_eq!(surface_contact(-10.0, 1.0, 2.0), 0.0);
    }
}
