//! CPU wave spectrum, ported from the web game's `ocean/waves/spectrum.ts`.
//!
//! Each cascade is a periodic tile whose wavevector lattice k = 2π·n/L holds seeded Gaussian amplitudes drawn from a
//! JONSWAP spectrum (Hasselmann et al. 1973), or Donelan et al.'s (1985) for the realistic sea state, whose short waves
//! are held to Phillips' saturation range, with frequency-dependent directional spreading (Hasselmann et al. 1980),
//! then scaled so the rendered surface has exactly the requested significant height. The GPU only evolves and
//! transforms what this builds. The arithmetic follows the TypeScript step for step (f64, amplitudes stored as f32,
//! integer hashes on u32), so the same seed draws the same sea on both clients.

use std::f64::consts::{LN_2, PI};

use crate::CascadeInfo;

pub const GRAVITY: f64 = 9.81;
/// Every angular frequency is a whole multiple of 2π/FOLD_PERIOD, so the sea repeats exactly after this many seconds
/// and the GPU can take time modulo it without float32 phase drift.
pub const FOLD_PERIOD: f64 = 4096.0;
/// The finer cascade takes over a band once its lattice has this many modes across the cut radius.
const FINE_MODES_AT_CUT: f64 = 8.0;
/// Breaking caps short waves at α·g²·ω⁻⁵ however high the sea (Phillips 1958), with a young sea's α.
const SATURATION: f64 = 0.02;
/// The cap blends in between these multiples of the peak frequency; the peak itself stays JONSWAP.
const SATURATION_ONSET: f64 = 1.5;
const SATURATION_FULL: f64 = 3.0;

/// Wavenumbers |k| (rad/m) a cascade holds: lo ≤ |k| < hi.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveBand {
    pub lo: f64,
    pub hi: f64,
}

/// One cascade's spectrum.
#[derive(Clone, Debug)]
pub struct CascadeSpectrum {
    pub size: f64,
    pub resolution: u32,
    pub band: WaveBand,
    /// Per texel, row z then column x, frequency n = i < N/2 ? i : i − N on each axis: (Re a, Im a, ±m, 0). The
    /// texel's wave is a·e^{∓iωt} with ω = 2π·m/FOLD_PERIOD.
    pub amplitudes: Vec<f32>,
    /// Time-averaged height variance (m²) this cascade contributes.
    pub variance: f64,
    /// Its time-averaged slope variance (∂y/∂x)² + (∂y/∂z)².
    pub slope_variance: f64,
    /// Twice the slope-weighted mean of 1/|k| (m): what a pixel's length along the view leaves resolved of its slopes.
    pub slope_length: f64,
}

#[derive(Clone, Debug)]
pub struct WaveSpectrum {
    pub cascades: Vec<CascadeSpectrum>,
    /// Strict bound on |height|: Σ|a| over every texel of every cascade.
    pub max_height: f64,
    /// Strict bound on the horizontal displacement's length: choppiness · Σ|a|.
    pub max_horizontal_displacement: f64,
    /// Slope variance the drawn waves fall short of Cox–Munk's total at this wind.
    pub tail_slope_variance: f64,
}

/// The sea state a spectrum is built for (`WaveParameters` in `contracts.ts`). Lengths are metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveParameters {
    /// Significant wave height Hm0 = 4σ of the rendered surface.
    pub significant_height: f64,
    /// Wind speed at 10 m (m/s).
    pub wind_speed: f64,
    /// Direction the wind blows toward, radians from +X toward +Z.
    pub wind_direction: f64,
    pub peak_wavelength: f64,
    /// Horizontal (choppy) displacement scale; 0 is a pure height field.
    pub choppiness: f64,
    /// JONSWAP peak enhancement γ.
    pub gamma: f64,
    /// Directional spreading; 1 is the measured spread, smaller is broader.
    pub directional_sharpness: f64,
    pub seed: f64,
    /// Donelan et al.'s ω⁻⁴ equilibrium range instead of JONSWAP's ω⁻⁵ (the realistic sea state sets it).
    pub equilibrium_range: bool,
}

impl Default for WaveParameters {
    /// `Ocean.ts`'s initial sea (seed 1941 as `Game.ts` creates it).
    fn default() -> Self {
        Self {
            significant_height: 1.8,
            wind_speed: 9.0,
            wind_direction: 35f64.to_radians(),
            peak_wavelength: 32.0,
            choppiness: 1.2,
            gamma: 2.6,
            directional_sharpness: 0.8,
            seed: 1941.0,
            equilibrium_range: false,
        }
    }
}

/// Deep-water angular frequency ω = √(g|k|), quantised to a whole multiple m of 2π/FOLD_PERIOD.
pub fn frequency_multiple(k: f64) -> f64 {
    ((GRAVITY * k).sqrt() * FOLD_PERIOD / (2.0 * PI)).round()
}

/// Where each cascade's band starts and ends; bands are disjoint, so no wavevector is drawn twice.
pub fn cascade_bands(cascades: &[CascadeInfo]) -> Vec<WaveBand> {
    let nyquist = |c: &CascadeInfo| PI * f64::from(c.resolution) / c.size;
    let cuts: Vec<f64> = cascades
        .iter()
        .skip(1)
        .enumerate()
        .map(|(i, fine)| {
            (FINE_MODES_AT_CUT * 2.0 * PI / fine.size).min(nyquist(&cascades[i]) / 2.0)
        })
        .collect();
    cascades
        .iter()
        .enumerate()
        .map(|(i, cascade)| WaveBand {
            lo: if i > 0 { cuts[i - 1] } else { 0.0 },
            hi: if i < cuts.len() {
                cuts[i]
            } else {
                nyquist(cascade)
            },
        })
        .collect()
}

/// JONSWAP energy per unit angular frequency, without α.
pub fn jonswap(omega: f64, peak: f64, gamma: f64) -> f64 {
    if omega.is_nan() || omega <= 0.0 {
        return 0.0;
    }
    let sigma = if omega <= peak { 0.07 } else { 0.09 };
    let r = (-((omega - peak).powi(2)) / (2.0 * sigma * sigma * peak * peak)).exp();
    GRAVITY * GRAVITY / omega.powi(5) * (-1.25 * (peak / omega).powi(4)).exp() * gamma.powf(r)
}

/// Donelan, Hamilton & Hui (1985) wind-sea energy per unit ω, without its level.
pub fn equilibrium(omega: f64, peak: f64, gamma: f64, age: f64) -> f64 {
    if omega.is_nan() || omega <= 0.0 {
        return 0.0;
    }
    let sigma = 0.08 * (1.0 + 4.0 / age.clamp(0.83, 5.0).powi(3));
    let r = (-((omega - peak).powi(2)) / (2.0 * sigma * sigma * peak * peak)).exp();
    GRAVITY * GRAVITY / (omega.powi(4) * peak) * (-((peak / omega).powi(4))).exp() * gamma.powf(r)
}

/// Energy per unit ω of the drawn sea: JONSWAP at `level`, or with a wave age Donelan's equilibrium range, held at or
/// below the saturation range above the peak.
pub fn sea_spectrum(omega: f64, peak: f64, gamma: f64, level: f64, age: Option<f64>) -> f64 {
    let free = level
        * match age {
            None => jonswap(omega, peak, gamma),
            Some(age) => equilibrium(omega, peak, gamma, age),
        };
    let t =
        ((omega / peak - SATURATION_ONSET) / (SATURATION_FULL - SATURATION_ONSET)).clamp(0.0, 1.0);
    free + (free.min(SATURATION * GRAVITY.powi(2) / omega.powi(5)) - free) * t * t * (3.0 - 2.0 * t)
}

/// The level α at which `sea_spectrum` holds `variance` (m²) in total.
pub fn spectrum_level(peak: f64, gamma: f64, variance: f64, age: Option<f64>) -> f64 {
    let total = |level: f64| {
        let mut sum = 0.0;
        for i in 0..=600 {
            let omega = peak * 0.3 * 100f64.powf(f64::from(i) / 600.0);
            let weight = if i == 0 || i == 600 { 0.5 } else { 1.0 };
            sum += sea_spectrum(omega, peak, gamma, level, age) * omega * weight;
        }
        sum * 100f64.ln() / 600.0
    };
    let (mut low, mut high) = (0.0, 1e-3);
    while total(high) < variance {
        high *= 2.0;
    }
    for _ in 0..60 {
        let middle = (low + high) / 2.0;
        if total(middle) < variance {
            low = middle;
        } else {
            high = middle;
        }
    }
    (low + high) / 2.0
}

/// Spreading exponent s of cos^{2s}(θ/2) (Hasselmann et al. 1980).
pub fn spreading_exponent(omega: f64, peak: f64, wind_speed: f64, sharpness: f64) -> f64 {
    let ratio = omega / peak;
    let age = (wind_speed * peak / GRAVITY).clamp(0.3, 3.0);
    let s = if ratio < 1.05 {
        6.97 * ratio.powf(4.06)
    } else {
        9.77 * ratio.powf(-2.33 - 1.45 * (age - 1.17))
    };
    (s * sharpness).max(0.0)
}

/// ln Γ(x) for x > 0 (Lanczos, g = 7).
fn log_gamma(x: f64) -> f64 {
    if x < 0.5 {
        return (PI / (PI * x).sin()).ln() - log_gamma(1.0 - x);
    }
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    let x = x - 1.0;
    let mut sum = C[0];
    for (i, c) in C.iter().enumerate().skip(1) {
        sum += c / (x + i as f64);
    }
    let t = x + 7.5;
    0.5 * (2.0 * PI).ln() + (x + 0.5) * t.ln() - t + sum.ln()
}

/// Normalisation of |cos(θ/2)|^{2s} over a full turn: 2^{2s−1}Γ(s+1)² / (πΓ(2s+1)).
pub fn spreading_normalization(s: f64) -> f64 {
    ((2.0 * s - 1.0) * LN_2 + 2.0 * log_gamma(s + 1.0) - log_gamma(2.0 * s + 1.0)).exp() / PI
}

/// PCG output permutation (Jarzynski & Olano 2020) on u32.
pub fn pcg(v: u32) -> u32 {
    let state = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    (word >> 22) ^ word
}

/// Counter-based random stream for one lattice pair: the seed's high and low words, the cascade and both signed
/// indices mixed as u32.
pub fn mode_hash(seed: f64, cascade: u32, nx: i32, nz: i32) -> u32 {
    let whole = seed.trunc();
    let high = to_uint32((whole / 4_294_967_296.0).floor());
    let low = to_uint32(whole);
    pcg((nz as u32).wrapping_add(pcg(
        (nx as u32).wrapping_add(pcg(cascade.wrapping_add(pcg(low.wrapping_add(pcg(high))))))
    )))
}

/// JavaScript's ToUint32 of an integral number.
fn to_uint32(value: f64) -> u32 {
    if !value.is_finite() {
        return 0;
    }
    value.rem_euclid(4_294_967_296.0) as u32
}

fn unit(bits: u32) -> f64 {
    (f64::from(bits) + 0.5) / 4_294_967_296.0
}

/// Build every cascade's amplitudes for `params`. Waves travel toward `wind_direction`.
pub fn build_spectrum(cascades: &[CascadeInfo], params: &WaveParameters) -> WaveSpectrum {
    let bands = cascade_bands(cascades);
    let height = params.significant_height;
    let lambda = params.peak_wavelength;
    let calm = height.is_nan()
        || height <= 0.0
        || lambda.is_nan()
        || lambda <= 0.0
        || !height.is_finite()
        || !lambda.is_finite();
    let peak = (GRAVITY * 2.0 * PI / lambda).sqrt();
    let gamma = params.gamma.max(1.0);
    let age = params
        .equilibrium_range
        .then(|| params.wind_speed.max(0.0) * peak / GRAVITY);
    let level = if calm {
        0.0
    } else {
        spectrum_level(peak, gamma, (height / 4.0).powi(2), age)
    };
    let (sin_wind, cos_wind) = params.wind_direction.sin_cos();
    let mut drawn = 0.0;
    let mut built: Vec<CascadeSpectrum> = cascades
        .iter()
        .enumerate()
        .map(|(index, cascade)| {
            let n = cascade.resolution as usize;
            let half = (n / 2) as i32;
            let band = bands[index];
            let mut amplitudes = vec![0f32; n * n * 4];
            let dk = 2.0 * PI / cascade.size;
            if !calm {
                for z in 0..n {
                    for x in 0..n {
                        let nx = if (x as i32) < half {
                            x as i32
                        } else {
                            x as i32 - n as i32
                        };
                        let nz = if (z as i32) < half {
                            z as i32
                        } else {
                            z as i32 - n as i32
                        };
                        // One draw per ± pair, from its canonical member (nz > 0, or nz = 0 and nx > 0).
                        if nz < 0 || (nz == 0 && nx <= 0) {
                            continue;
                        }
                        let kx = f64::from(nx) * dk;
                        let kz = f64::from(nz) * dk;
                        let k = kx.hypot(kz);
                        // The Nyquist column has no distinct twin; it stays empty.
                        if k < band.lo || k >= band.hi || nx == -half {
                            continue;
                        }
                        let omega = (GRAVITY * k).sqrt();
                        let s = spreading_exponent(
                            omega,
                            peak,
                            params.wind_speed,
                            params.directional_sharpness,
                        );
                        let cos_theta = (kx * cos_wind + kz * sin_wind) / k;
                        let toward = ((1.0 + cos_theta) / 2.0).max(0.0).sqrt().powf(2.0 * s);
                        let away = ((1.0 - cos_theta) / 2.0).max(0.0).sqrt().powf(2.0 * s);
                        let density = sea_spectrum(omega, peak, gamma, level, age) * GRAVITY
                            / (2.0 * omega)
                            * spreading_normalization(s)
                            / k
                            * dk
                            * dk;
                        let pair = density * (toward + away);
                        if pair.is_nan() || pair <= 0.0 {
                            continue;
                        }
                        let bits = mode_hash(params.seed, index as u32, nx, nz);
                        let radius = (-2.0 * unit(pcg(bits.wrapping_add(1))).ln()).sqrt();
                        let angle = 2.0 * PI * unit(pcg(bits.wrapping_add(2)));
                        let scale = (pair / 4.0).sqrt() * radius;
                        let re = scale * angle.cos();
                        let im = scale * angle.sin();
                        let forward = unit(pcg(bits.wrapping_add(3))) * (toward + away) < toward;
                        let m = frequency_multiple(k);
                        let here = (z * n + x) * 4;
                        let twin = (((n - z) % n) * n + (n - x) % n) * 4;
                        let (at, other) = if forward { (here, twin) } else { (twin, here) };
                        amplitudes[at] = re as f32;
                        amplitudes[at + 1] = im as f32;
                        amplitudes[at + 2] = m as f32;
                        amplitudes[other] = re as f32;
                        amplitudes[other + 1] = (-im) as f32;
                        amplitudes[other + 2] = (-m) as f32;
                        drawn += 2.0 * (re * re + im * im);
                    }
                }
            }
            CascadeSpectrum {
                size: cascade.size,
                resolution: cascade.resolution,
                band,
                amplitudes,
                variance: 0.0,
                slope_variance: 0.0,
                slope_length: 0.0,
            }
        })
        .collect();
    let scale = if drawn > 0.0 {
        height / (4.0 * drawn.sqrt())
    } else {
        0.0
    };
    // Statistics come from the stored float32 amplitudes: the GPU transforms exactly these.
    let mut bound = 0.0;
    for cascade in &mut built {
        let n = cascade.resolution as usize;
        let dk = 2.0 * PI / cascade.size;
        let mut reach = 0.0;
        let (mut variance, mut slope_variance) = (0.0, 0.0);
        for (texel, a) in cascade
            .amplitudes
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .enumerate()
        {
            if a[2] == 0.0 {
                continue;
            }
            a[0] = (f64::from(a[0]) * scale) as f32;
            a[1] = (f64::from(a[1]) * scale) as f32;
            let nx = texel % n;
            let nz = texel / n;
            let fx = if nx < n / 2 {
                nx as f64
            } else {
                nx as f64 - n as f64
            };
            let fz = if nz < n / 2 {
                nz as f64
            } else {
                nz as f64 - n as f64
            };
            let energy = f64::from(a[0]).powi(2) + f64::from(a[1]).powi(2);
            let k2 = (fx * fx + fz * fz) * dk * dk;
            bound += energy.sqrt();
            variance += energy;
            slope_variance += energy * k2;
            reach += energy * k2.sqrt();
        }
        cascade.variance = variance;
        cascade.slope_variance = slope_variance;
        cascade.slope_length = if slope_variance > 0.0 {
            2.0 * reach / slope_variance
        } else {
            0.0
        };
    }
    let slope_variance: f64 = built.iter().map(|c| c.slope_variance).sum();
    // Waves shorter than the finest tile still roughen the surface: Cox & Munk's total 0.003 + 0.00512·U.
    let tail_slope_variance = if calm {
        0.0
    } else {
        (0.003 + 0.00512 * params.wind_speed.max(0.0) - slope_variance).max(0.0)
    };
    WaveSpectrum {
        cascades: built,
        max_height: bound,
        max_horizontal_displacement: params.choppiness.max(0.0) * bound,
        tail_slope_variance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QualityTier;

    fn close(a: f64, b: f64, relative: f64) -> bool {
        (a - b).abs() <= relative * b.abs().max(1e-300)
    }

    #[test]
    fn mode_hash_matches_the_typescript() {
        // `modeHash` in the web game's `ocean/waves/spectrum.ts` under Bun.
        assert_eq!(mode_hash(1941.0, 0, 3, 5), 149_580_125);
        assert_eq!(mode_hash(1941.0, 2, -7, 1), 3_532_196_215);
        assert_eq!(mode_hash(-12_345_678_901.0, 1, 0, 4), 356_689_412);
    }

    #[test]
    fn spectrum_matches_the_typescript() {
        // `buildSpectrum` for the High tier at the port's calibrated 9 m/s sea, under Bun.
        let params = WaveParameters {
            choppiness: 1.1,
            ..WaveParameters::default()
        };
        let spectrum = build_spectrum(&QualityTier::High.cascades(), &params);
        assert!(close(spectrum.max_height, 37.473_194_871_775_306, 1e-9));
        let expected = [
            (
                0.136_027_731_943_548_42,
                0.005_409_077_956_823_776,
                9.823_781_451_144_322,
            ),
            (
                0.064_550_364_028_375_77,
                0.019_354_396_415_571_8,
                3.292_538_730_911_487,
            ),
            (
                0.001_921_904_097_288_625_6,
                0.027_400_847_541_977_014,
                0.420_360_393_582_294_27,
            ),
        ];
        for (cascade, (variance, slopes, length)) in spectrum.cascades.iter().zip(expected) {
            assert!(close(cascade.variance, variance, 1e-9));
            assert!(close(cascade.slope_variance, slopes, 1e-9));
            assert!(close(cascade.slope_length, length, 1e-9));
        }
        let a = &spectrum.cascades[0].amplitudes;
        let at = |i: usize| (a[i * 4], a[i * 4 + 1], a[i * 4 + 2]);
        assert_eq!(at(256 + 2), (0.0, 0.0, -239.0));
        assert_eq!(at(3 * 256 + 250).2, -414.0);
        assert!(close(
            f64::from(at(10 * 256 + 5).0),
            -0.000_121_729_070_087_894_8,
            1e-6
        ));
        assert!(close(
            f64::from(at(10 * 256 + 5).1),
            0.000_179_188_034_962_862_73,
            1e-6
        ));
    }

    #[test]
    fn realistic_spectrum_matches_the_typescript() {
        let params = crate::sea_state::drawn_sea(
            &WaveParameters {
                choppiness: 1.1,
                ..WaveParameters::default()
            },
            true,
        );
        assert!(close(params.peak_wavelength, 70.440_795_014_579_38, 1e-12));
        let cascades = crate::sea_state::sea_state_cascades(
            &QualityTier::High.cascades(),
            params.peak_wavelength,
        );
        let spectrum = build_spectrum(&cascades, &params);
        assert!(close(spectrum.max_height, 35.077_179_442_379_624, 1e-9));
        assert!(close(
            spectrum.tail_slope_variance,
            0.010_173_490_146_160_737,
            1e-8
        ));
        assert!(close(
            spectrum.cascades[1].slope_variance,
            0.008_769_615_380_747_73,
            1e-9
        ));
    }

    #[test]
    fn realised_height_is_exact() {
        let cascades = QualityTier::Low.cascades();
        let params = WaveParameters::default();
        let spectrum = build_spectrum(&cascades, &params);
        let variance: f64 = spectrum.cascades.iter().map(|c| c.variance).sum();
        assert!((4.0 * variance.sqrt() - params.significant_height).abs() < 1e-4);
    }

    #[test]
    fn bands_are_disjoint_and_cover_the_tier() {
        let bands = cascade_bands(&QualityTier::High.cascades());
        assert_eq!(bands[0].lo, 0.0);
        for pair in bands.windows(2) {
            assert_eq!(pair[0].hi, pair[1].lo);
        }
    }
}
