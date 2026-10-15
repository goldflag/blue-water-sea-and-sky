//! Gusts, the CPU half of the web game's `ocean/waves/gusts.ts`: where the short waves are rougher or smoother than the sea's
//! mean. Three octaves of the world's value noise, stretched along the wind and drifting downwind, scale the slope
//! variance of the waves shorter than [`GUST_WAVELENGTH`] by a lognormal gain of mean 1 (`shaders/field_common.wgsl`).

/// Waves shorter than this (m) follow the gusts.
pub const GUST_WAVELENGTH: f64 = 8.0;
/// a at a moderate breeze.
pub const GUST_STRENGTH: f64 = 0.5;
const LIGHT_AIR: f64 = 0.3;
const LIGHT_AIR_WIND: f64 = 3.0;
const BREEZE_WIND: f64 = 8.0;
const GALE_WIND: f64 = 11.0;
const STORM_WIND: f64 = 20.0;
const STORM_LOSS: f64 = 0.6;

fn smooth(from: f64, to: f64, x: f64) -> f64 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// a for a wind of `wind_speed` m/s.
pub fn gust_strength(wind_speed: f64) -> f64 {
    GUST_STRENGTH
        * (1.0 + LIGHT_AIR * (1.0 - smooth(LIGHT_AIR_WIND, BREEZE_WIND, wind_speed))
            - STORM_LOSS * smooth(GALE_WIND, STORM_WIND, wind_speed))
}

/// `GustParameters`: every field live, absent fields take their defaults.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GustParameters {
    /// a; `None` follows the wind ([`gust_strength`]).
    pub strength: Option<f64>,
    /// Scale on the patches' size (1: about 4.2 km, 1.2 km and 400 m long).
    pub size: Option<f64>,
    /// Scale on their downwind drift (1: 0.3–0.6 of the wind speed).
    pub drift: Option<f64>,
    /// Waves shorter than this follow the gusts ([`GUST_WAVELENGTH`] when absent).
    pub wavelength: Option<f64>,
}
