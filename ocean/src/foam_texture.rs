//! The ocean's one foam texture, ported from the web game's `ocean/surface/foamTexture.ts`: generated at startup, tiling in
//! both axes, 256² RGBA8 with every mip level given explicitly.
//!
//! Red: lace, the structure of thinning white water. Green: broad patches (fbm). Alpha: churn, the heaped billows of
//! fresh and churned white water. These three are equalised, and each mip level down to 16² averages the raw fields and
//! is equalised again, so thresholding a channel at 1 − coverage keeps that share at any distance. Blue: the band mask
//! (soft lines along u that meander and break, where the wind gathers old foam into windrows); it is scaled, not
//! thresholded, so its levels are plain averages whose mean ([`FoamTexture::streak_mean`]) holds at every distance.

/// Texels per edge.
pub const FOAM_TEXELS: u32 = 256;
/// Most taps the sampler averages along a grazing pixel's length.
pub const FOAM_ANISOTROPY: u16 = 8;

/// Integer hash to [0, 1): seeds and lattice indices only (`hash` in the TypeScript).
pub fn hash(x: i32, y: i32, seed: i32) -> f64 {
    let mut h = (x.wrapping_mul(0x27d4_eb2d_u32 as i32))
        ^ (y.wrapping_mul(0x1656_67b1))
        ^ seed.wrapping_mul(0x9e37_79b9_u32 as i32);
    h = (h ^ ((h as u32) >> 15) as i32).wrapping_mul(0x85eb_ca6b_u32 as i32);
    h = (h ^ ((h as u32) >> 13) as i32).wrapping_mul(0xc2b2_ae35_u32 as i32);
    f64::from((h ^ ((h as u32) >> 16) as i32) as u32) / 4_294_967_296.0
}

fn wrap(i: i32, n: i32) -> i32 {
    ((i % n) + n) % n
}

/// Distances to the nearest and second-nearest feature points of a periodic `cells`² jittered grid, in cell units.
fn worley(u: f64, v: f64, cells: i32, seed: i32) -> (f64, f64) {
    let x = u * f64::from(cells);
    let y = v * f64::from(cells);
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let (mut first, mut second) = (9.0_f64, 9.0_f64);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let cx = ix + dx;
            let cy = iy + dy;
            let wx = wrap(cx, cells);
            let wy = wrap(cy, cells);
            let d = (f64::from(cx) + hash(wx, wy, seed) - x)
                .hypot(f64::from(cy) + hash(wx, wy, seed + 1) - y);
            if d < first {
                second = first;
                first = d;
            } else if d < second {
                second = d;
            }
        }
    }
    (first, second)
}

/// Smooth periodic value noise in [0, 1) on a `cells_u` × `cells_v` lattice.
fn value(u: f64, v: f64, cells_u: i32, cells_v: i32, seed: i32) -> f64 {
    let x = u * f64::from(cells_u);
    let y = v * f64::from(cells_v);
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let fx = x - f64::from(ix);
    let fy = y - f64::from(iy);
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let at = |i: i32, j: i32| hash(wrap(i, cells_u), wrap(j, cells_v), seed);
    let top = at(ix, iy) + (at(ix + 1, iy) - at(ix, iy)) * sx;
    let bottom = at(ix, iy + 1) + (at(ix + 1, iy + 1) - at(ix, iy + 1)) * sx;
    top + (bottom - top) * sy
}

/// Replace every value by its rank, so the channel is uniform on [0, 1]. Ties keep their index order, as the
/// TypeScript's stable sort does.
fn equalize(field: &[f32]) -> Vec<f32> {
    let mut order: Vec<usize> = (0..field.len()).collect();
    order.sort_by(|&a, &b| {
        field[a]
            .partial_cmp(&field[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut ranks = vec![0f32; field.len()];
    let count = field.len() as f64;
    for (rank, &index) in order.iter().enumerate() {
        ranks[index] = ((rank as f64 + 0.5) / count) as f32;
    }
    ranks
}

const LACE_CELLS: [i32; 3] = [5, 12, 29];
const LACE_WEIGHTS: [f64; 3] = [0.45, 0.35, 0.2];
const LACE_WIDTH: f64 = 0.09;
const LACE_FILL: f64 = 0.35;
const LACE_WARP: f64 = 0.5;

fn lace(u: f64, v: f64) -> f64 {
    let warp = LACE_WARP / f64::from(LACE_CELLS[0]);
    let wu = u + warp * (value(u, v, 7, 7, 71) - 0.5);
    let wv = v + warp * (value(u, v, 7, 7, 73) - 0.5);
    let mut sum = 0.0;
    for (i, &cells) in LACE_CELLS.iter().enumerate() {
        let (first, second) = worley(wu, wv, cells, 3 + 2 * i as i32);
        sum += LACE_WEIGHTS[i]
            * ((1.0 - LACE_FILL) * (-(second - first) / LACE_WIDTH).exp()
                + LACE_FILL * (first / 0.7).min(1.0));
    }
    sum
}

const STREAK_ROWS: [i32; 2] = [5, 13];
const STREAK_WIDTH_MIN: f64 = 1.5;
const STREAK_WIDTH_MAX: f64 = 4.0;
const STREAK_MEANDER: f64 = 1.3;
const STREAK_BREAKS: i32 = 41;
const STREAK_WIGGLE: f64 = 0.3;
const STREAK_WIGGLES: i32 = 70;
const STREAK_SWELLS: i32 = 53;
const STREAK_GAP: f64 = 0.42;
const STREAK_GAP_EDGE: f64 = 0.25;

fn smooth(t: f64) -> f64 {
    let x = t.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn streak_lines(u: f64, v: f64, rows: i32, seed: i32) -> f64 {
    let y = v * f64::from(rows)
        + STREAK_MEANDER * (value(u, v, 3, 5, seed + 7) - 0.5)
        + STREAK_WIGGLE * (value(u, v, STREAK_WIGGLES, rows, seed + 9) - 0.5);
    let row = y.floor() as i32;
    let mut line = 0.0_f64;
    for offset in -1..=1 {
        let cell = row + offset;
        let id = wrap(cell, rows);
        let centre = f64::from(cell) + 0.2 + 0.6 * hash(id, 0, seed);
        let swell = 0.4 + 1.2 * value(u + hash(id, 4, seed), 0.0, STREAK_SWELLS, 1, seed * 37 + id);
        let width = (STREAK_WIDTH_MIN + (STREAK_WIDTH_MAX - STREAK_WIDTH_MIN) * hash(id, 1, seed))
            * swell
            * f64::from(rows)
            / f64::from(FOAM_TEXELS);
        let strength = 0.25 + 0.75 * hash(id, 3, seed);
        let along = value(
            u + hash(id, 2, seed),
            0.0,
            STREAK_BREAKS,
            1,
            seed * 131 + id,
        );
        let profile = (-((y - centre) / width).powi(2)).exp();
        line = line.max(profile * strength * smooth((along - STREAK_GAP) / STREAK_GAP_EDGE));
    }
    line
}

fn streaks(u: f64, v: f64) -> f64 {
    STREAK_ROWS
        .iter()
        .enumerate()
        .fold(0.0, |most, (i, &rows)| {
            most.max(streak_lines(u, v, rows, 23 + 10 * i as i32))
        })
}

const STREAK_CHANNEL: usize = 2;
const EQUALIZED_TEXELS: u32 = 16;
const CHURN_CELLS: [i32; 3] = [23, 53, 127];
const CHURN_WEIGHTS: [f64; 3] = [0.5, 0.32, 0.18];

fn churn(u: f64, v: f64) -> f64 {
    CHURN_CELLS
        .iter()
        .enumerate()
        .map(|(i, &cells)| {
            CHURN_WEIGHTS[i] * (1.0 - worley(u, v, cells, 51 + 2 * i as i32).0.min(1.0))
        })
        .sum()
}

fn halve(field: &[f32], size: usize) -> Vec<f32> {
    let half = size / 2;
    let mut out = vec![0f32; half * half];
    for y in 0..half {
        for x in 0..half {
            let i = 2 * y * size + 2 * x;
            let sum = f64::from(field[i])
                + f64::from(field[i + 1])
                + f64::from(field[i + size])
                + f64::from(field[i + size + 1]);
            out[y * half + x] = (sum / 4.0) as f32;
        }
    }
    out
}

fn level_pixels(fields: &[Vec<f32>; 4], size: u32) -> Vec<u8> {
    let count = fields[0].len();
    let mut pixels = vec![0u8; count * 4];
    for (channel, field) in fields.iter().enumerate() {
        let values = if channel == STREAK_CHANNEL || size < EQUALIZED_TEXELS {
            field.clone()
        } else {
            equalize(field)
        };
        for (i, level) in values.iter().enumerate() {
            pixels[i * 4 + channel] = (f64::from(level.clamp(0.0, 1.0)) * 255.0).round() as u8;
        }
    }
    pixels
}

/// The generated texture: RGBA8 levels, largest first, and the band mask's mean.
pub struct FoamTexture {
    /// `(size, pixels)` per mip level, 256² down to 1².
    pub levels: Vec<(u32, Vec<u8>)>,
    pub streak_mean: f32,
}

impl FoamTexture {
    pub fn generate() -> Self {
        let n = FOAM_TEXELS as usize;
        let mut fields: [Vec<f32>; 4] = std::array::from_fn(|_| vec![0f32; n * n]);
        for y in 0..n {
            for x in 0..n {
                let u = (x as f64 + 0.5) / n as f64;
                let v = (y as f64 + 0.5) / n as f64;
                let i = y * n + x;
                let mut broad = 0.0;
                let mut weight = 0.5;
                for octave in 0..4 {
                    broad += weight * value(u, v, 4 << octave, 4 << octave, 11 + octave);
                    weight *= 0.5;
                }
                fields[0][i] = lace(u, v) as f32;
                fields[1][i] = broad as f32;
                fields[2][i] = streaks(u, v) as f32;
                fields[3][i] = churn(u, v) as f32;
            }
        }
        let streak_mean = (fields[STREAK_CHANNEL]
            .iter()
            .map(|&v| f64::from(v))
            .sum::<f64>()
            / (n * n) as f64) as f32;
        let mut levels = Vec::new();
        let mut size = FOAM_TEXELS;
        loop {
            levels.push((size, level_pixels(&fields, size)));
            if size == 1 {
                break;
            }
            // Below the equalised levels the fields average the last equalised one, so they settle to its mean.
            if size == EQUALIZED_TEXELS {
                for (channel, field) in fields.iter_mut().enumerate() {
                    if channel != STREAK_CHANNEL {
                        *field = equalize(field);
                    }
                }
            }
            for field in fields.iter_mut() {
                *field = halve(field, size as usize);
            }
            size /= 2;
        }
        Self {
            levels,
            streak_mean,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_the_typescript() {
        // `hashAt` in the web game's `ocean/noise.ts` under Bun.
        assert_eq!(hash(0, 0, 0), 0.0);
        assert_eq!(hash(1, 2, 3), 0.871_194_091_159_850_4);
        assert_eq!(hash(-5, 7, 101), 0.373_585_304_711_014_03);
    }

    #[test]
    fn texture_matches_the_typescript() {
        // `foamTexture()` under Bun: the band mask's mean and a few texels of levels 0, 4 and 8.
        let texture = FoamTexture::generate();
        assert!((f64::from(texture.streak_mean) - 0.079_772_716_717_113_46).abs() < 1e-7);
        let level0 = &texture.levels[0].1;
        assert_eq!(&level0[..4], &[137, 9, 0, 16]);
        assert_eq!(&level0[4000..4004], &[150, 11, 63, 219]);
        assert_eq!(texture.levels[4].1[20], 217);
        assert_eq!(texture.levels[8].1[0], 128);
    }

    #[test]
    fn equalised_channels_are_uniform_at_every_equalised_level() {
        let texture = FoamTexture::generate();
        assert_eq!(texture.levels.len(), 9);
        for (size, pixels) in texture
            .levels
            .iter()
            .filter(|(size, _)| *size >= EQUALIZED_TEXELS)
        {
            let count = (*size * *size) as f64;
            let mean = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| f64::from(p[0]))
                .sum::<f64>()
                / count
                / 255.0;
            assert!((mean - 0.5).abs() < 0.01, "level {size} lace mean {mean}");
        }
        assert!(texture.streak_mean > 0.0 && texture.streak_mean < 0.5);
    }
}
