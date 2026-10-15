//! The weather's haze: the marine boundary layer's aerosol the visibility asks for, over the air the tables hold.
//!
//! The haze is a second aerosol beside the tables' own (whose turbidity sets the dome's): densest at the sea, thinning
//! with height (`scale_height`) up to the boundary layer's top (`top`, the clouds' base: a marine boundary layer's
//! clouds form at its top), then quickly above it (`taper`). The visibility V is the game's (the weather's fog end,
//! which the simulation's lookouts also see to); one constant, [`SIGHTING_DEPTH`], turns it into the air's extinction
//! at sea level (550 nm), the atmosphere and the haze together, and the haze is what that asks less the tables' own as
//! a view sees it (`AirCoefficients::aerial_scale` of it: half on a fair day, `model::aerial`).
//! It scatters nearly all it stops (`albedo`) with a strongly forward Cornette–Shanks phase (`asymmetry`).
//!
//! The aerial-perspective pass (naval-clouds' `aerial.wgsl`) integrates it along every view ray; the sun's and the
//! moon's light at the sea pass through it here ([`Haze::beam_transmittance`]), as they do to light the haze itself
//! there, so a low sun dims on the hulls as it does in the haze around them. Lengths are metres.

use crate::model::{self, AirCoefficients, PLANET_RADIUS};

/// The one calibration between the game's visibility V and the air: the optical depth (550 nm) of the air at sea level,
/// the atmosphere and the haze together, over V, so the extinction at sea is `SIGHTING_DEPTH / V`. Where the tables'
/// own air already stops that much, the weather adds no haze. Bill picked A (2026-10-03; `native/docs/haze.md`,
/// "Calibration", keeps B beside it):
///
/// - **A, 2.42** = ln(0.45 / 0.04): a ship goes at about V, where the simulation's lookouts lose her. Her darker parts
///   (0.45 contrast against the horizon close up) fall to 4 % there, what a target a fraction of a degree high needs
///   (twice Koschmieder's 2 %, the eye's on a large one: Blackwell 1946). A black object's 2 % (the meteorological
///   visibility) lies at 1.62 V.
/// - **B, 1.2** (not taken): the old visual range: clearer air for the same V. A broadside Bismarck at noon in the North Atlantic's
///   17 km keeps a good part of her contrast to 10 km and fades out past 20 km; the meteorological visibility is
///   3.3 V, and in the weathers whose own air is thicker than that (overcast, storm, fog) the tables' air alone sets it.
pub const SIGHTING_DEPTH: f64 = 2.42;

/// Koschmieder's constant: a black object falls to 2 % contrast against the horizon at an optical depth of ln(1 / 0.02).
pub const KOSCHMIEDER: f64 = 3.912;

/// The weather's haze.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Haze {
    /// The game's visibility (m): the weather's fog end, which the simulation's lookouts see to ([`SIGHTING_DEPTH`]).
    pub visibility: f64,
    /// Top of the boundary layer (m above the sea).
    pub top: f64,
    /// Scale height (m) of the aerosol within the layer.
    pub scale_height: f64,
    /// Scale height (m) of its taper above the layer's top.
    pub taper: f64,
    /// Single-scattering albedo (sea salt and sulphate droplets absorb little of what they stop).
    pub albedo: f64,
    /// Asymmetry of its Cornette–Shanks phase (marine aerosol scatters strongly forward).
    pub asymmetry: f64,
}

impl Haze {
    /// The layer's top as a share of the clouds' base, held within these heights (m).
    pub const TOP: (f64, f64) = (400.0, 1500.0);

    /// The haze's scale height within the layer (m): a surface layer. It was 1 km until 2026-10-09, when the land
    /// seen from the air was to keep its colour (`native/docs/haze.md`, "Clear air aloft"): the visibility at the sea
    /// is the same, the air above the lowest few hundred metres clearer.
    pub const SURFACE: f64 = 250.0;

    /// The haze of `visibility` metres under clouds based at `cloud_base` metres.
    pub fn new(visibility: f64, cloud_base: f64) -> Self {
        Self {
            visibility,
            top: cloud_base.clamp(Self::TOP.0, Self::TOP.1),
            ..Self::default()
        }
    }

    /// Extinction (per m, at 550 nm) at the sea of the air and the haze together: [`SIGHTING_DEPTH`] over the
    /// visibility.
    pub fn sighting_extinction(&self) -> f64 {
        SIGHTING_DEPTH / self.visibility.max(1.0)
    }

    /// Extinction (per m, at 550 nm) of the haze at the sea: what the visibility asks less the tables' own air there,
    /// the share of it a view sees (`aerial_scale`).
    pub fn extinction_at_sea(&self, air: &AirCoefficients) -> f64 {
        let atmosphere = air.aerial_scale * model::extinction(air, 0.0)[1] / 1000.0;
        (self.sighting_extinction() - atmosphere).max(0.0)
    }

    /// The haze's density at `height` (m above the sea) over its density at the sea.
    pub fn density(&self, height: f64) -> f64 {
        let height = height.max(0.0);
        if height <= self.top {
            return (-height / self.scale_height).exp();
        }
        (-self.top / self.scale_height).exp() * (-(height - self.top) / self.taper).exp()
    }

    /// Metres of sea-level haze along a ray from `height` (m) whose zenith cosine there is `mu`, out of the air: the
    /// layer's exponential to its top, then the taper, each held at its grazing length (Chapman's limit) on the planet.
    /// The WGSL twin is `haze_path` in naval-clouds' `aerial.wgsl`.
    pub fn path(&self, height: f64, mu: f64) -> f64 {
        let bottom = PLANET_RADIUS * 1000.0;
        let height = height.max(0.0);
        let r = bottom + height;
        let grazing =
            |scale: f64, radius: f64| (2.0 * scale / (std::f64::consts::PI * radius)).sqrt();
        let (h, t) = (self.scale_height, self.taper);
        if height < self.top {
            let r_top = bottom + self.top;
            let lower = h * ((-height / h).exp() - (-self.top / h).exp()) / mu.max(grazing(h, r));
            let across = r / r_top * (1.0 - mu * mu).max(0.0).sqrt();
            let mu_top = if mu > 0.0 {
                (1.0 - across * across).max(0.0).sqrt()
            } else {
                0.0
            };
            let upper = (-self.top / h).exp() * t / mu_top.max(grazing(t, r_top));
            return lower + upper;
        }
        self.density(height) * t / mu.max(grazing(t, r))
    }

    /// The share of a body's light reaching `height` from zenith cosine `mu` through the haze above, delta-Eddington
    /// scaled: what the haze scatters forward stays in the beam.
    pub fn beam_transmittance(&self, air: &AirCoefficients, height: f64, mu: f64) -> f64 {
        let scaled = (1.0 - self.albedo * self.asymmetry) * self.extinction_at_sea(air);
        (-scaled * self.path(height, mu)).exp()
    }
}

impl Default for Haze {
    /// A fair day's: 20 km of visibility in a 1 km boundary layer, densest in its lowest few hundred metres (the sea
    /// salt and the swollen droplets the visibility at the sea is made of: [`Haze::SURFACE`]).
    fn default() -> Self {
        Self {
            visibility: 20_000.0,
            top: 1000.0,
            scale_height: Self::SURFACE,
            taper: 250.0,
            albedo: 0.95,
            asymmetry: 0.75,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AuthoredAtmosphere, air_coefficients};

    fn air(turbidity: f64) -> AirCoefficients {
        air_coefficients(&AuthoredAtmosphere {
            rayleigh: 0.4,
            turbidity,
            mie: 0.35,
            mie_g: 0.7,
            multiple: 1.0,
        })
    }

    #[test]
    fn the_visibility_sets_the_extinction_at_sea() {
        let air = air(2.2);
        let haze = Haze::new(with_haze(&air, 1e-4), 1700.0);
        let total = haze.extinction_at_sea(&air)
            + air.aerial_scale * model::extinction(&air, 0.0)[1] / 1000.0;
        // The air a view sees at sea stops `SIGHTING_DEPTH` over the visibility; a black object's 2 % lies beyond it by
        // their ratio.
        assert!((total * haze.visibility - SIGHTING_DEPTH).abs() < 1e-9);
        let meteorological = KOSCHMIEDER / total;
        assert!((meteorological / haze.visibility - KOSCHMIEDER / SIGHTING_DEPTH).abs() < 1e-9);
        // Air thicker than the visibility asks adds no haze.
        assert_eq!(Haze::new(5e5, 1000.0).extinction_at_sea(&air), 0.0);
    }

    #[test]
    fn a_vertical_path_crosses_the_layer() {
        let haze = Haze::default();
        // Straight up from the sea: the layer's exponential to its top, then the taper.
        let h = haze.scale_height;
        let expected = h * (1.0 - (-haze.top / h).exp()) + (-haze.top / h).exp() * haze.taper;
        assert!((haze.path(0.0, 1.0) / expected - 1.0).abs() < 1e-6);
        // Grazing paths stay finite, longer the lower the sun.
        let (low, lower) = (haze.path(0.0, 0.05), haze.path(0.0, 0.01));
        assert!(lower > low && lower.is_finite() && low > haze.path(0.0, 0.5));
        // Above the layer only the taper remains.
        assert!(haze.path(3000.0, 1.0) < 1.0);
    }

    /// The visibility at which the haze at sea stands `haze` per metre over the tables' air (the share a view sees),
    /// whatever the calibration.
    fn with_haze(air: &AirCoefficients, haze: f64) -> f64 {
        SIGHTING_DEPTH / (air.aerial_scale * model::extinction(air, 0.0)[1] / 1000.0 + haze)
    }

    #[test]
    fn a_low_sun_dims_in_the_haze() {
        let air = air(2.2);
        let haze = Haze::new(with_haze(&air, 1.4e-4), 1700.0);
        let noon = haze.beam_transmittance(&air, 0.0, 0.94);
        let dusk = haze.beam_transmittance(&air, 0.0, 3f64.to_radians().sin());
        assert!(noon > 0.97 && dusk < 0.9 && dusk > 0.2, "{noon} {dusk}");
    }
}
