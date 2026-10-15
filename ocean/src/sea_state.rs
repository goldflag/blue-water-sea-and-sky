//! The realistic wind sea behind `OceanRealism.seaState`, ported from the web game's `ocean/waves/seaState.ts`: the peak
//! wavelength a real wind sea has at the calibrated significant height and wind (JONSWAP's fetch-limited growth laws),
//! Donelan et al.'s spectrum at that wave age, crests sharpened toward Stokes' limit, and tiles large enough that the
//! longest waves do not repeat. The calibration table keeps its heights, which combat and hull motion share.

use std::f64::consts::PI;

use crate::CascadeInfo;
use crate::spectrum::{GRAVITY, WaveParameters};

/// JONSWAP's growth with dimensionless fetch χ = gX/U²: g²m0/U⁴ = 1.6e-7·χ, fp·U/g = 3.5·χ^-0.33.
const ENERGY_GROWTH: f64 = 1.6e-7;
const PEAK_FREQUENCY: f64 = 3.5;
const PEAK_DECAY: f64 = 0.33;
/// Stokes' highest wave is H/λ = 0.1412.
const STOKES_LIMIT: f64 = 0.1412;
/// At this choppiness a crest cusps exactly at Stokes' limit.
pub const BREAKING_CHOPPINESS: f64 = 1.0 / (PI * STOKES_LIMIT);
/// Tiles hold at least this many peak wavelengths.
pub const PEAK_WAVELENGTHS_PER_TILE: f64 = 8.0;
/// Tiles grow at most this much beyond the tier's layout.
pub const MAX_TILE_GROWTH: f64 = 4.0;

/// The fetch χ at which a wind sea of significant height `height` has grown under `wind_speed`.
pub fn wind_sea_fetch(height: f64, wind_speed: f64) -> f64 {
    (height / 4.0).powi(2) * GRAVITY.powi(2) / wind_speed.powi(4) / ENERGY_GROWTH
}

/// Peak wavelength (m) of a wind sea with this significant height under this wind (Toba's 3/2 law form). 0 for a flat
/// sea; `fallback` without wind.
pub fn wind_sea_peak_wavelength(height: f64, wind_speed: f64, fallback: f64) -> f64 {
    if height.is_nan() || height <= 0.0 || !height.is_finite() {
        return 0.0;
    }
    if wind_speed.is_nan() || wind_speed <= 0.0 || !wind_speed.is_finite() {
        return fallback;
    }
    let frequency = PEAK_FREQUENCY * GRAVITY / wind_speed
        * wind_sea_fetch(height, wind_speed).powf(-PEAK_DECAY);
    GRAVITY / (2.0 * PI * frequency * frequency)
}

/// Wave age U/cp.
pub fn wave_age(peak_wavelength: f64, wind_speed: f64) -> f64 {
    let age = wind_speed / (GRAVITY * peak_wavelength / (2.0 * PI)).sqrt();
    if age.is_finite() && age > 0.0 {
        age
    } else {
        0.0
    }
}

/// Donelan, Hamilton & Hui's (1985) peak enhancement at a wave age.
pub fn wind_sea_gamma(age: f64) -> f64 {
    1.7 + 6.0 * age.clamp(1.0, 5.0).log10()
}

/// The sea the wave field draws: `params` itself, or with `sea_state` on the wavelength of a real wind sea at the same
/// height and wind, Donelan's spectrum at its wave age, and crests sharpened toward the breaking limit.
pub fn drawn_sea(params: &WaveParameters, sea_state: bool) -> WaveParameters {
    if !sea_state {
        return *params;
    }
    let found = wind_sea_peak_wavelength(
        params.significant_height,
        params.wind_speed,
        params.peak_wavelength,
    );
    let peak_wavelength = if found != 0.0 && !found.is_nan() {
        found
    } else {
        params.peak_wavelength
    };
    WaveParameters {
        peak_wavelength,
        gamma: wind_sea_gamma(wave_age(peak_wavelength, params.wind_speed)),
        choppiness: BREAKING_CHOPPINESS,
        equilibrium_range: true,
        ..*params
    }
}

/// The tier's tiles grown by one factor until the largest holds PEAK_WAVELENGTHS_PER_TILE peak wavelengths.
pub fn sea_state_cascades(base: &[CascadeInfo], peak_wavelength: f64) -> Vec<CascadeInfo> {
    let growth =
        (PEAK_WAVELENGTHS_PER_TILE * peak_wavelength / base[0].size).clamp(1.0, MAX_TILE_GROWTH);
    let factor = if growth.is_finite() { growth } else { 1.0 };
    base.iter()
        .map(|c| CascadeInfo {
            size: c.size * factor,
            resolution: c.resolution,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn developed_sea_at_nine_metres_a_second() {
        // README: 70 m at 9 m/s (Pierson–Moskowitz's developed sea), 146 m at 15, 298 m at 25.
        assert!((wind_sea_peak_wavelength(1.8, 9.0, 0.0) - 70.0).abs() < 1.0);
        assert!((wind_sea_peak_wavelength(4.0, 15.0, 0.0) - 146.0).abs() < 2.0);
        assert!((wind_sea_peak_wavelength(8.8, 25.0, 0.0) - 298.0).abs() < 3.0);
    }
}
