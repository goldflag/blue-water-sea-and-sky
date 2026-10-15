//! Whitecaps, ported from the web game's `ocean/waves/whitecaps.ts`: how much of the sea the wind turns white, and where each
//! cascade's crests break to make it so.
//!
//! A crest breaks where it is both compressed along the wind and steep on its forward face. Both are linear in the wave
//! amplitudes and a quarter period apart for every travelling mode, so over the tile they are uncorrelated Gaussian
//! fields whose variances the spectrum gives exactly; their normalised blend is a standard normal breaking indicator,
//! and a crest breaks where it passes the quantile the wind's coverage calls for.

use std::f64::consts::PI;

use crate::groups::group_share;
use crate::spectrum::{CascadeSpectrum, GRAVITY, WaveSpectrum};

/// Monahan & O'Muircheartaigh (1980): whitecaps cover 3.84e-6·U^3.41 of the sea.
const MONAHAN_SCALE: f64 = 3.84e-6;
const MONAHAN_EXPONENT: f64 = 3.41;
const ONSET_START: f64 = 3.5;
const ONSET_FULL: f64 = 5.0;
const WINDROW_START: f64 = 13.0;
const WINDROW_FULL: f64 = 25.0;
const WINDROW_SHARE: f64 = 0.25;
const WINDROW_MOST: f64 = 0.04;
/// Forward shift of the breaking indicator from the crest (radians of wave phase).
const FACE_SHIFT: f64 = 0.9;
/// Waves break across a window of wavelengths relative to the sea's mean wavelength: fully from BREAKING_SHORT to
/// BREAKING_LONG of it, fading out by BREAKING_SHORTEST and BREAKING_LONGEST. Breaking crest length peaks well below the
/// peak (Phillips' Λ(c) at a third to half of its phase speed, Kleiss & Melville 2010), so the dominant waves
/// themselves rarely break and whitecaps are small and many.
const BREAKING_LONGEST: f64 = 0.6;
const BREAKING_LONG: f64 = 0.3;
const BREAKING_SHORT: f64 = 1.0 / 16.0;
const BREAKING_SHORTEST: f64 = 1.0 / 32.0;
const RIPPLE_START: f64 = 3.0;
const RIPPLE_END: f64 = 5.0;
const MIN_ACTIVE: f64 = 1e-7;
const PERSISTENCE: f64 = 19.5;
const GATED_SHOWING: f64 = 0.55;
const SATURATION: f64 = 0.5;
const MAX_BOOST: f64 = 4.0;
const MAX_BREAKING: f64 = 0.5;

fn smooth(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The share of the sea whitecaps cover at `wind_speed`, active and residual foam together.
pub fn whitecap_coverage(wind_speed: f64) -> f64 {
    let wind = if wind_speed.is_nan() {
        0.0
    } else {
        wind_speed.clamp(0.0, 40.0)
    };
    let t = smooth((wind - ONSET_START) / (ONSET_FULL - ONSET_START));
    MONAHAN_SCALE * wind.powf(MONAHAN_EXPONENT) * t
}

/// The share of the sea old foam covers in windrows at `wind_speed`, times `scale`.
pub fn windrow_coverage(wind_speed: f64, scale: f64) -> f64 {
    let wind = if wind_speed.is_nan() { 0.0 } else { wind_speed };
    let t = smooth((wind - WINDROW_START) / (WINDROW_FULL - WINDROW_START));
    (whitecap_coverage(wind_speed) * WINDROW_SHARE * t).min(WINDROW_MOST) * scale.max(0.0)
}

/// Inverse of the standard normal CDF (Acklam's rational approximation).
pub fn normal_quantile(p: f64) -> f64 {
    if p.is_nan() || p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    const A: [f64; 6] = [
        -39.696_830_286_653_76,
        220.946_098_424_520_5,
        -275.928_510_446_968_7,
        138.357_751_867_269,
        -30.664_798_066_147_16,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -54.476_098_798_224_06,
        161.585_836_858_040_9,
        -155.698_979_859_886_6,
        66.801_311_887_719_72,
        -13.280_681_552_885_72,
    ];
    const C: [f64; 6] = [
        -0.007_784_894_002_430_293,
        -0.322_396_458_041_136_5,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        0.007_784_695_709_041_462,
        0.322_467_129_070_039_8,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    let tail = |q: f64| {
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    if p < 0.02425 {
        return tail((-2.0 * p.ln()).sqrt());
    }
    if p > 1.0 - 0.02425 {
        return -tail((-2.0 * (1.0 - p).ln()).sqrt());
    }
    let q = p - 0.5;
    let r = q * q;
    (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
        / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
}

/// How one cascade breaks: indicator weights, threshold, share and period.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CascadeBreaking {
    pub compression: f64,
    pub face: f64,
    /// Indicator where breaking starts; infinite where this cascade holds no breaking waves.
    pub threshold: f64,
    /// Share of the sea's breaking this cascade carries.
    pub share: f64,
    /// Period (s) of the waves that break in this cascade.
    pub period: f64,
}

fn log_ramp(x: f64, a: f64, b: f64) -> f64 {
    smooth((x / a).ln() / (b / a).ln())
}

fn breaking_window(wavelength: f64, mean: f64) -> f64 {
    let ratio = wavelength / mean;
    log_ramp(ratio, BREAKING_SHORTEST, BREAKING_SHORT)
        * (1.0 - log_ramp(ratio, BREAKING_LONG, BREAKING_LONGEST))
        * log_ramp(wavelength, RIPPLE_START, RIPPLE_END)
}

/// Calls `visit(energy, kx, kz)` for every stored texel of a cascade.
fn for_modes(cascade: &CascadeSpectrum, mut visit: impl FnMut(f64, f64, f64)) {
    let n = cascade.resolution as usize;
    let dk = 2.0 * PI / cascade.size;
    for (texel, a) in cascade.amplitudes.as_chunks::<4>().0.iter().enumerate() {
        if a[2] == 0.0 {
            continue;
        }
        let x = texel % n;
        let z = texel / n;
        let fx = if x < n / 2 {
            x as f64
        } else {
            x as f64 - n as f64
        };
        let fz = if z < n / 2 {
            z as f64
        } else {
            z as f64 - n as f64
        };
        let energy = f64::from(a[0]).powi(2) + f64::from(a[1]).powi(2);
        visit(energy, fx * dk, fz * dk);
    }
}

/// Each cascade's breaking statistics under a wind blowing toward the unit vector (wx, wz); thresholds are left
/// infinite for [`set_breaking_thresholds`].
pub fn cascade_breaking(
    spectrum: &WaveSpectrum,
    wx: f64,
    wz: f64,
    choppiness: f64,
) -> Vec<CascadeBreaking> {
    let (mut energy, mut inverse) = (0.0, 0.0);
    for cascade in &spectrum.cascades {
        for_modes(cascade, |e, kx, kz| {
            energy += e;
            inverse += e / kx.hypot(kz);
        });
    }
    let mean = if energy > 0.0 {
        2.0 * PI * inverse / energy
    } else {
        0.0
    };
    struct Stats {
        compression: f64,
        face: f64,
        weight: f64,
        period: f64,
    }
    let stats: Vec<Stats> = spectrum
        .cascades
        .iter()
        .map(|cascade| {
            let (mut squeeze, mut face, mut weight, mut weighted_inverse) = (0.0, 0.0, 0.0, 0.0);
            for_modes(cascade, |e, kx, kz| {
                let k2 = kx * kx + kz * kz;
                let k = k2.sqrt();
                let along = kx * wx + kz * wz;
                squeeze += e * along.powi(4) / k2;
                face += e * along * along;
                let w = e * k2 * breaking_window(2.0 * PI / k, mean);
                weight += w;
                weighted_inverse += w / k;
            });
            let compression = choppiness.max(0.0).powi(2) * squeeze;
            let a = if compression > 0.0 {
                FACE_SHIFT.cos() / compression.sqrt()
            } else {
                0.0
            };
            let b = if face > 0.0 {
                (if compression > 0.0 {
                    FACE_SHIFT.sin()
                } else {
                    1.0
                }) / face.sqrt()
            } else {
                0.0
            };
            let wavelength = if weight > 0.0 {
                2.0 * PI * weighted_inverse / weight
            } else {
                0.0
            };
            Stats {
                compression: a,
                face: b,
                weight,
                period: if wavelength > 0.0 {
                    (2.0 * PI * wavelength / GRAVITY).sqrt()
                } else {
                    0.0
                },
            }
        })
        .collect();
    let total: f64 = stats.iter().map(|s| s.weight).sum();
    stats
        .iter()
        .map(|s| CascadeBreaking {
            compression: s.compression,
            face: s.face,
            threshold: f64::INFINITY,
            share: if total > 0.0 { s.weight / total } else { 0.0 },
            period: s.period,
        })
        .collect()
}

/// The share of the sea whitecap patches cover at `wind_speed` with the map's `scale`.
pub fn whitecap_area(wind_speed: f64, scale: f64) -> f64 {
    let total = (whitecap_coverage(wind_speed) * scale.max(0.0)).min(0.95);
    let rows = total.min(windrow_coverage(wind_speed, scale));
    1.0 - (1.0 - total) / (1.0 - rows)
}

/// The whitecaps' optical depth to inject for `wind_speed`.
pub fn whitecap_depth(wind_speed: f64, scale: f64) -> f64 {
    let area = whitecap_area(wind_speed, scale);
    let depth = -(1.0 - area / group_share(area)).ln();
    depth / (1.0 / MAX_BOOST).max(1.0 - SATURATION * depth)
}

/// Place each cascade's threshold so that it breaks over the fraction of the sea that covers its share of `depth`.
pub fn set_breaking_thresholds(cascades: &mut [CascadeBreaking], depth: f64) {
    let capacity: Vec<f64> = (0..cascades.len())
        .map(|i| MAX_BREAKING * PERSISTENCE * if i > 0 { GATED_SHOWING } else { 1.0 })
        .collect();
    let mut carried: Vec<f64> = cascades.iter().map(|c| c.share * depth).collect();
    for _ in 0..cascades.len() {
        let excess: f64 = carried
            .iter()
            .zip(&capacity)
            .map(|(d, c)| (d - c).max(0.0))
            .sum();
        let open: f64 = cascades
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if carried[i] < capacity[i] {
                    c.share
                } else {
                    0.0
                }
            })
            .sum();
        if excess.is_nan() || excess <= 0.0 || open.is_nan() || open <= 0.0 {
            break;
        }
        for i in 0..carried.len() {
            carried[i] = if carried[i] >= capacity[i] {
                capacity[i]
            } else {
                carried[i] + excess * cascades[i].share / open
            };
        }
    }
    for (i, cascade) in cascades.iter_mut().enumerate() {
        let fraction = (carried[i] / (PERSISTENCE * if i > 0 { GATED_SHOWING } else { 1.0 }))
            .min(MAX_BREAKING);
        cascade.threshold =
            if fraction > MIN_ACTIVE && (cascade.compression != 0.0 || cascade.face != 0.0) {
                -normal_quantile(fraction)
            } else {
                f64::INFINITY
            };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantile_inverts_the_normal() {
        assert!(normal_quantile(0.5).abs() < 1e-12);
        assert!((normal_quantile(0.975) - 1.959_963_985).abs() < 1e-8);
        assert!((normal_quantile(0.01) + 2.326_347_874).abs() < 1e-8);
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1e-12)
    }

    #[test]
    fn statistics_match_the_typescript() {
        // `whitecaps.ts` and `groups.ts` under Bun.
        assert!(close(whitecap_area(9.0, 1.0), 0.006_891_252_793_797_764));
        assert!(close(whitecap_depth(9.0, 1.0), 0.020_085_416_707_101_324));
        assert!(close(whitecap_area(15.0, 1.0), 0.038_636_787_658_983_93));
        assert!(close(whitecap_depth(15.0, 1.0), 0.124_239_366_506_140_13));
        assert!(close(
            windrow_coverage(18.0, 1.0),
            0.006_888_427_532_993_986_5
        ));
        let layout = crate::groups::group_layout(2.6, 181.0);
        assert!(close(layout.length, 26.386_138_860_262_243));
        assert!(close(layout.width, 42.217_822_176_419_59));
        assert!(close(layout.drift, 2.029_702_989_250_941_5));
    }

    #[test]
    fn breaking_comes_from_waves_well_below_the_peak() {
        // `cascadeBreaking` + `setBreakingThresholds(whitecapDepth(9))` on the High tier's calibrated 9 m/s sea. The
        // indicator's weights are still the TypeScript's under Bun; the breaking window has since moved below the peak
        // (2026-10-03), so the longest cascade, which holds the peak, no longer breaks and the breakers are 4–8 m waves.
        let params = crate::WaveParameters {
            choppiness: 1.1,
            ..Default::default()
        };
        let spectrum =
            crate::spectrum::build_spectrum(&crate::QualityTier::High.cascades(), &params);
        let (wz, wx) = params.wind_direction.sin_cos();
        let mut breaking = cascade_breaking(&spectrum, wx, wz, params.choppiness);
        set_breaking_thresholds(&mut breaking, whitecap_depth(9.0, 1.0));
        let expected = [
            (
                9.636_181_763_900_801,
                12.278_987_243_054_248,
                f64::INFINITY,
                0.0,
                0.0,
            ),
            (
                6.110_728_863_497_924,
                7.469_585_546_192_103_5,
                2.914_990_642_088_906_5,
                0.949_662_650_351_259_4,
                2.190_121_338_193_977_3,
            ),
            (
                5.771_247_434_636_511_5,
                6.835_091_942_667_820_5,
                3.733_896_272_618_410_6,
                0.050_337_349_648_740_6,
                1.538_302_124_529_654_2,
            ),
        ];
        for (b, (compression, face, threshold, share, period)) in breaking.iter().zip(expected) {
            let near = |a: f64, b: f64| a == b || (a - b).abs() <= 1e-6 * b.abs();
            assert!(
                near(b.compression, compression) && near(b.face, face),
                "{b:?}"
            );
            assert!(
                near(b.threshold, threshold) && near(b.share, share) && near(b.period, period),
                "{b:?}"
            );
        }
    }

    #[test]
    fn coverage_follows_monahan() {
        assert_eq!(whitecap_coverage(3.0), 0.0);
        assert!((whitecap_coverage(9.0) - 3.84e-6 * 9f64.powf(3.41)).abs() < 1e-12);
        assert_eq!(windrow_coverage(9.0, 1.0), 0.0);
    }
}
