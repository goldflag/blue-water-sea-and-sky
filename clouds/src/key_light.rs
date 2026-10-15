//! The clouds' key light: the body whose direct beam the march, the Beer shadow maps, the light shafts and the
//! environment's bake follow. By day it is the sun, as in takram's clouds; once the moon's direct light at the top of the
//! clouds outshines the sun's (the sun gone below the clouds' horizon, its last light reddened through the whole air) it
//! is the moon, at the irradiance the world takes from it (the scene's moonlight, `SkyUniforms::moon_irradiance`, on the
//! dome's scale `sky_grade::GAIN`, as the sun's `direct_scale` and the world's `SeaLevel` have it), through the same
//! tables. At the switch the two beams are equal, so the clouds' direct light passes from one body to the other without a
//! step; the sky's light on them (the sun's twilight, the moonlit sky at the dome's grade, the night floor) is the
//! march's own sum (`shaders/march.wgsl`, `key_scalar_irradiance`).

use glam::Vec3;
use naval_sky::Sky;
use naval_sky::model::{self, AirCoefficients, PLANET_RADIUS, SOLAR_IRRADIANCE, sky_grade};

/// Samples of the CPU's optical depth toward each body (`model::optical_depth`, quadratic spacing).
const DEPTH_SAMPLES: u32 = 64;

/// What the key light is chosen from: the sky's two bodies as the clouds take them.
#[derive(Clone, Copy, Debug)]
pub struct Bodies<'a> {
    pub air: &'a AirCoefficients,
    /// Toward the sun and the moon (world, unit).
    pub sun: Vec3,
    pub moon: Vec3,
    /// The sun's luminance above the air (the scene's `sun_intensity`) and the sky's twilight lift on it.
    pub sun_intensity: f64,
    pub sun_lift: f64,
    /// The moon above the air, colour × intensity (`SkyUniforms::moon_irradiance`), and whether the sky shows it.
    pub moon_irradiance: [f64; 3],
    pub moon_active: bool,
}

/// The body lighting the clouds this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyLight {
    /// Toward the key body (world, unit).
    pub direction: Vec3,
    /// Whether the moon is the key light.
    pub moon: bool,
    /// The moon's direct scale: game units per unit of the tables' solar irradiance, per channel (the moon above the
    /// air over the sun above the air, times the dome's gain).
    pub moon_scale: [f32; 3],
}

impl KeyLight {
    /// The key light for the clouds over the camera, `top` metres above the sea the highest the clouds reach (where the
    /// sun lingers longest).
    pub fn of(sky: &Sky, top: f64) -> Self {
        let u = sky.uniforms();
        Self::choose(
            &Bodies {
                air: sky.air(),
                sun: sky.sun_direction(),
                moon: sky.moon_direction(),
                sun_intensity: sky.scene().sun_intensity,
                sun_lift: f64::from(u.sun_lift),
                moon_irradiance: u.moon_irradiance.map(f64::from),
                moon_active: u.moon_active > 0.5,
            },
            top,
        )
    }

    /// The key light of `bodies` for clouds reaching `top` metres.
    pub fn choose(bodies: &Bodies<'_>, top: f64) -> Self {
        let sun = bodies.sun.normalize_or(Vec3::Y);
        let moon = bodies.moon.normalize_or(Vec3::NEG_Y);
        let (solar, lunar) = beams(bodies, top);
        let night = bodies.moon_active && lunar > solar;
        Self {
            direction: if night { moon } else { sun },
            moon: night,
            moon_scale: std::array::from_fn(|c| {
                (sky_grade::GAIN * bodies.moon_irradiance[c] / SOLAR_IRRADIANCE[c]) as f32
            }),
        }
    }

    /// `CloudUniforms::key_light`: xyz the moon's direct scale, w 1 while the moon is the key light.
    pub fn uniform(&self) -> [f32; 4] {
        [
            self.moon_scale[0],
            self.moon_scale[1],
            self.moon_scale[2],
            if self.moon { 1.0 } else { 0.0 },
        ]
    }
}

/// The sun's and the moon's direct light (luminance, game units) `top` metres above the sea under them: the clouds'
/// sunlight as the march takes it (`direct_scale`: the sun's luminance above the air, the twilight lift, the dome's gain)
/// and the world's moonlight, each through the air.
pub fn beams(bodies: &Bodies<'_>, top: f64) -> (f64, f64) {
    let radius = PLANET_RADIUS + top.max(0.0) / 1000.0;
    let transmittance = |toward: Vec3| -> [f64; 3] {
        let toward = toward.normalize_or(Vec3::Y);
        model::optical_depth(bodies.air, radius, f64::from(toward.y), DEPTH_SAMPLES).map(|depth| {
            if depth.is_finite() {
                (-depth).exp()
            } else {
                0.0
            }
        })
    };
    let gain = sky_grade::GAIN;
    let solar =
        model::luminance(transmittance(bodies.sun)) * bodies.sun_intensity * bodies.sun_lift * gain;
    let toward_moon = transmittance(bodies.moon);
    let lunar = model::luminance(std::array::from_fn(|c| {
        bodies.moon_irradiance[c] * toward_moon[c]
    })) * gain;
    (solar, lunar)
}

#[cfg(test)]
mod tests {
    use super::*;
    use naval_sky::state::{self, CelestialModel};

    fn bodies(air: &AirCoefficients, sun_elevation: f64) -> Bodies<'_> {
        let mut model = CelestialModel::default();
        model.set(sun_elevation, 270.0, 0.5);
        let moon_elevation = model.moon.y.asin().to_degrees();
        Bodies {
            air,
            sun: model.sun.as_vec3(),
            moon: model.moon.as_vec3(),
            sun_intensity: 6.6,
            sun_lift: model::twilight_lift(sun_elevation),
            moon_irradiance: (state::moon_tint() * state::MOONLIGHT).to_array(),
            moon_active: moon_elevation > -6.0,
        }
    }

    fn north_atlantic() -> AirCoefficients {
        model::air_coefficients(&naval_sky::state::SkyScene::north_atlantic(30.0, 0.0).atmosphere)
    }

    #[test]
    fn the_sun_by_day_and_the_moon_by_night() {
        let air = north_atlantic();
        for elevation in [70.0, 20.0, 3.0, 0.0, -1.0] {
            assert!(
                !KeyLight::choose(&bodies(&air, elevation), 8000.0).moon,
                "{elevation}"
            );
        }
        for elevation in [-4.0, -10.0, -28.0, -60.0] {
            let key = KeyLight::choose(&bodies(&air, elevation), 8000.0);
            assert!(key.moon, "{elevation}");
        }
    }

    /// The switch falls where the two beams are equal: the moon takes over from a sun whose last light at the top of
    /// the clouds is no brighter than its own.
    #[test]
    fn the_beam_passes_from_the_sun_to_the_moon_without_a_step() {
        let air = north_atlantic();
        let mut previous: Option<(bool, f64, f64)> = None;
        let mut switches = 0;
        for step in 0..=400 {
            let elevation = -1.0 - f64::from(step) * 0.01;
            let (solar, lunar) = beams(&bodies(&air, elevation), 8000.0);
            let moon = KeyLight::choose(&bodies(&air, elevation), 8000.0).moon;
            if let Some((was_moon, was_solar, was_lunar)) = previous
                && was_moon != moon
            {
                switches += 1;
                // Either side of the switch the key beam is the brighter one, and the two are close.
                let before = if was_moon { was_lunar } else { was_solar };
                let after = if moon { lunar } else { solar };
                assert!(
                    (after - before).abs() < 0.1 * before.max(after),
                    "{before} → {after} at {elevation}°"
                );
                // The sun sets for the top of the clouds 2.9° below the sea's horizon (8 km up).
                assert!(
                    (-3.0..-2.0).contains(&elevation),
                    "the switch at {elevation}°"
                );
            }
            previous = Some((moon, solar, lunar));
        }
        assert_eq!(switches, 1);
    }
}
