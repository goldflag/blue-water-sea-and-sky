//! The wake's vertical-derivative operator √(−∇²), the core of iWave (Tessendorf, *Interactive Water Surfaces*, 2004),
//! as radial convolution kernels on a reduce/collapse pyramid: a port of the web game's `ocean/wake/kernel.ts`, f64 step for
//! step, so the fitted weights match the web's.
//!
//! A single truncated kernel of radius P reproduces |k| only for wavelengths shorter than about P cells. Following
//! Canabal et al., *Dispersion Kernels for Water Wave Simulation* (2016), each level applies a small kernel to a
//! binomially reduced copy of the field and the results are collapsed by bilinear upsampling; the weights are fitted
//! jointly so the total response stays within ~3% of |k| from 6 cells up to 12 × 2^levels cells. Grid units throughout;
//! the field divides by its cell size.

/// One tap on a level's own grid: offset in cells and weight in 1/cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KernelTap {
    pub x: i32,
    pub y: i32,
    pub weight: f64,
}

/// Kernel radius (cells) on every level but the coarsest; the coarsest also carries the long-range tail.
const RADIUS: i32 = 3;
const COARSEST_RADIUS: i32 = 6;

/// Response of the [1 3 3 1]/8 reduce, and equally of the 2× bilinear upsample, per axis.
fn binomial(k: f64) -> f64 {
    ((1.5 * k).cos() + 3.0 * (0.5 * k).cos()) / 4.0
}

/// The target: |k| with a smooth roll-off before Nyquist, where a grid cannot carry a wave anyway.
fn target(q: f64) -> f64 {
    q * (-(q / 2.6).powi(4)).exp()
}

/// Tap offsets grouped by 8-fold symmetry: (a, b), a ≥ b ≥ 0, a > 0, inside `radius`, in the web's insertion order.
fn symmetry_classes(radius: i32) -> Vec<Vec<(i32, i32)>> {
    let mut classes = Vec::new();
    for a in 1..=radius {
        for b in 0..=a {
            if a * a + b * b > radius * radius {
                continue;
            }
            let mut taps: Vec<(i32, i32)> = Vec::new();
            for (x, y) in [(a, b), (b, a)] {
                for sx in [1, -1] {
                    for sy in [1, -1] {
                        let tap = (sx * x, sy * y);
                        if !taps.contains(&tap) {
                            taps.push(tap);
                        }
                    }
                }
            }
            classes.push(taps);
        }
    }
    classes
}

struct Basis {
    level: usize,
    taps: Vec<(i32, i32)>,
}

fn basis_response(entry: &Basis, kx: f64, ky: f64) -> f64 {
    let mut filter = 1.0;
    let mut scale = 1.0;
    while scale < f64::from(1u32 << entry.level) {
        filter *= (binomial(scale * kx) * binomial(scale * ky)).powi(2);
        scale *= 2.0;
    }
    let spread = f64::from(1u32 << entry.level);
    let mut sum = 0.0;
    for &(x, y) in &entry.taps {
        sum += (spread * (kx * f64::from(x) + ky * f64::from(y))).cos() - 1.0;
    }
    filter * sum
}

/// The fitted pyramid: `levels[0]` runs on the field's grid, `levels[j]` on the grid reduced 2^j times. Each sums to 0.
pub struct DispersionPyramid {
    pub levels: Vec<Vec<KernelTap>>,
    basis: Vec<Basis>,
    weights: Vec<f64>,
}

impl DispersionPyramid {
    /// Base-band response to a plane wave (kx, ky) in radians per fine cell; ≈ |k| up to the roll-off.
    pub fn response(&self, kx: f64, ky: f64) -> f64 {
        self.basis
            .iter()
            .zip(&self.weights)
            .map(|(entry, weight)| weight * basis_response(entry, kx, ky))
            .sum()
    }
}

/// Fit the pyramid with `coarse_levels` reduced levels below the field's own grid (`dispersionPyramid`).
pub fn dispersion_pyramid(coarse_levels: usize) -> DispersionPyramid {
    let radii: Vec<i32> = (0..=coarse_levels)
        .map(|level| {
            if level == coarse_levels {
                COARSEST_RADIUS
            } else {
                RADIUS
            }
        })
        .collect();
    // Each unknown is one symmetry class minus as much at the centre, so every level sums to zero and a uniform raise
    // of the sea never accelerates.
    let basis: Vec<Basis> = radii
        .iter()
        .enumerate()
        .flat_map(|(level, &radius)| {
            symmetry_classes(radius)
                .into_iter()
                .map(move |taps| Basis { level, taps })
        })
        .collect();
    // Weighted least squares on relative error, sampled log-uniformly in |k| over the whole Nyquist square: the
    // diagonal corner must stay positive or its modes grow instead of oscillating.
    let n = basis.len();
    let mut normal = vec![vec![0.0_f64; n]; n];
    let mut rhs = vec![0.0_f64; n];
    let k_min = 2.0 * std::f64::consts::PI / (12.0 * f64::from(1u32 << coarse_levels));
    let k_max = std::f64::consts::PI * std::f64::consts::SQRT_2;
    for i in 0..90 {
        let q = k_min * (k_max / k_min).powf(f64::from(i) / 89.0);
        for a in 0..6 {
            let angle = f64::from(a) / 5.0 * std::f64::consts::PI / 4.0;
            let kx = q * angle.cos();
            let ky = q * angle.sin();
            if kx > std::f64::consts::PI {
                continue;
            }
            let t = target(q);
            let weight = (if q < 2.2 { 1.0 } else { 0.2 }) / t.max(1e-4).powi(2);
            let row: Vec<f64> = basis
                .iter()
                .map(|entry| basis_response(entry, kx, ky))
                .collect();
            for r in 0..n {
                rhs[r] += weight * row[r] * t;
                for c in 0..n {
                    normal[r][c] += weight * row[r] * row[c];
                }
            }
        }
    }
    let ridge = (0..n).map(|i| normal[i][i]).sum::<f64>() / n as f64 * 1e-6;
    for (i, row) in normal.iter_mut().enumerate() {
        row[i] += ridge;
    }
    let weights = solve(normal, rhs);
    let levels = (0..radii.len())
        .map(|level| {
            let mut taps = Vec::new();
            let mut center = 0.0;
            for (entry, &weight) in basis.iter().zip(&weights) {
                if entry.level != level {
                    continue;
                }
                for &(x, y) in &entry.taps {
                    taps.push(KernelTap { x, y, weight });
                }
                center -= weight * entry.taps.len() as f64;
            }
            let mut level_taps = vec![KernelTap {
                x: 0,
                y: 0,
                weight: center,
            }];
            level_taps.extend(taps);
            level_taps
        })
        .collect();
    DispersionPyramid {
        levels,
        basis,
        weights,
    }
}

/// Gaussian elimination with partial pivoting; the systems here are ~50 unknowns.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Vec<f64> {
    let n = b.len();
    for i in 0..n {
        let mut pivot = i;
        for r in i + 1..n {
            if a[r][i].abs() > a[pivot][i].abs() {
                pivot = r;
            }
        }
        a.swap(i, pivot);
        b.swap(i, pivot);
        let (upper, lower) = a.split_at_mut(i + 1);
        let pivot_row = &upper[i];
        for (offset, row) in lower.iter_mut().enumerate() {
            let f = row[i] / pivot_row[i];
            for (value, pivot_value) in row[i..].iter_mut().zip(&pivot_row[i..]) {
                *value -= f * pivot_value;
            }
            b[i + 1 + offset] -= f * b[i];
        }
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut s = b[i];
        for c in i + 1..n {
            s -= a[i][c] * x[c];
        }
        x[i] = s / a[i][i];
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The GPU passes on a periodic CPU grid: [1 3 3 1] reduce, per-level kernels, bilinear collapse (kernel.test.ts).
    fn apply_pyramid(levels: &[Vec<KernelTap>], field: &[f64], size: usize) -> Vec<f64> {
        let wrap = |i: i64, n: usize| i.rem_euclid(n as i64) as usize;
        let reduce = |u: &[f64], n: usize| {
            let half = n / 2;
            let mut out = vec![0.0; half * half];
            let w = [1.0, 3.0, 3.0, 1.0];
            for j in 0..half {
                for i in 0..half {
                    let mut sum = 0.0;
                    for (b, wb) in w.iter().enumerate() {
                        for (a, wa) in w.iter().enumerate() {
                            sum += wa
                                * wb
                                * u[wrap(2 * j as i64 + b as i64 - 1, n) * n
                                    + wrap(2 * i as i64 + a as i64 - 1, n)];
                        }
                    }
                    out[j * half + i] = sum / 64.0;
                }
            }
            out
        };
        let upsample = |c: &[f64], n: usize| {
            let m = 2 * n;
            let mut out = vec![0.0; m * m];
            let taps = |f: usize| -> [(i64, f64); 2] {
                if f % 2 == 1 {
                    [(((f - 1) / 2) as i64, 0.75), (f.div_ceil(2) as i64, 0.25)]
                } else {
                    [((f / 2) as i64 - 1, 0.25), ((f / 2) as i64, 0.75)]
                }
            };
            for j in 0..m {
                for i in 0..m {
                    let mut sum = 0.0;
                    for (cj, wj) in taps(j) {
                        for (ci, wi) in taps(i) {
                            sum += wi * wj * c[wrap(cj, n) * n + wrap(ci, n)];
                        }
                    }
                    out[j * m + i] = sum;
                }
            }
            out
        };
        let convolve = |u: &[f64], n: usize, kernel: &[KernelTap]| {
            let mut out = vec![0.0; n * n];
            for j in 0..n {
                for i in 0..n {
                    let mut sum = 0.0;
                    for tap in kernel {
                        sum += tap.weight
                            * u[wrap(j as i64 + i64::from(tap.y), n) * n
                                + wrap(i as i64 + i64::from(tap.x), n)];
                    }
                    out[j * n + i] = sum;
                }
            }
            out
        };
        let mut pyramid = vec![field.to_vec()];
        for level in 1..levels.len() {
            let next = reduce(&pyramid[level - 1], size >> (level - 1));
            pyramid.push(next);
        }
        let mut collapsed: Option<Vec<f64>> = None;
        for level in (0..levels.len()).rev() {
            let n = size >> level;
            let mut own = convolve(&pyramid[level], n, &levels[level]);
            if let Some(coarser) = &collapsed {
                let up = upsample(coarser, n / 2);
                for (value, up) in own.iter_mut().zip(up) {
                    *value += up;
                }
            }
            collapsed = Some(own);
        }
        collapsed.unwrap()
    }

    #[test]
    fn every_level_sums_to_zero() {
        for coarse in [3, 4, 5] {
            for level in dispersion_pyramid(coarse).levels {
                assert!(level.iter().map(|t| t.weight).sum::<f64>().abs() < 1e-9);
            }
        }
    }

    #[test]
    fn the_pyramid_follows_deep_water_dispersion() {
        for coarse in [3, 4, 5] {
            let pyramid = dispersion_pyramid(coarse);
            let mut wavelength = 6.0;
            while wavelength <= 12.0 * f64::from(1u32 << coarse) {
                for angle in [0.0, 0.2, 0.4, 0.6, std::f64::consts::FRAC_PI_4] {
                    let k = 2.0 * std::f64::consts::PI / wavelength;
                    let r = pyramid.response(k * f64::cos(angle), k * f64::sin(angle)) / k;
                    assert!((r - 1.0).abs() < 0.04, "λ {wavelength} angle {angle}: {r}");
                }
                wavelength *= 1.25;
            }
        }
    }

    #[test]
    fn every_mode_oscillates() {
        for coarse in [3, 4, 5] {
            let pyramid = dispersion_pyramid(coarse);
            let (mut smallest, mut largest) = (f64::INFINITY, 0.0_f64);
            for x in 0..=48 {
                for y in 0..=48 {
                    if x == 0 && y == 0 {
                        continue;
                    }
                    let value = pyramid.response(
                        f64::from(x) / 48.0 * std::f64::consts::PI,
                        f64::from(y) / 48.0 * std::f64::consts::PI,
                    );
                    smallest = smallest.min(value);
                    largest = largest.max(value);
                }
            }
            assert!(smallest > 0.0);
            assert!(largest < 1.6);
        }
    }

    #[test]
    fn reduce_and_collapse_match_the_fit() {
        let size = 128;
        let pyramid = dispersion_pyramid(3);
        for (p, q) in [
            (2, 0),
            (2, 1),
            (4, 0),
            (3, 3),
            (8, 2),
            (12, 5),
            (16, 0),
            (20, 6),
        ] {
            let kx = 2.0 * std::f64::consts::PI * f64::from(p) / size as f64;
            let ky = 2.0 * std::f64::consts::PI * f64::from(q) / size as f64;
            let k = kx.hypot(ky);
            let field: Vec<f64> = (0..size * size)
                .map(|index| (kx * (index % size) as f64 + ky * (index / size) as f64).cos())
                .collect();
            let result = apply_pyramid(&pyramid.levels, &field, size);
            let projection: f64 = field.iter().zip(&result).map(|(a, b)| a * b).sum();
            let energy: f64 = field.iter().map(|a| a * a).sum();
            let response = projection / energy;
            let residual: f64 = result
                .iter()
                .zip(&field)
                .map(|(r, f)| (r - response * f).powi(2))
                .sum();
            let total: f64 = result.iter().map(|r| r * r).sum();
            assert!((response / pyramid.response(kx, ky) - 1.0).abs() < 0.03);
            assert!((response / k - 1.0).abs() < 0.05);
            assert!((residual / total).sqrt() < 0.06);
        }
    }
}
