//! What each Graphics → Clouds tier spends: takram's quality presets (`qualityPresets.ts` of @takram/three-clouds:
//! Low, Medium, High and Ultra onto the tiers of the same names) for the clouds, less three trims that do not show
//! (see `HIGH` and Ultra) and with the game's own octaves of multiple scattering, and the game's budgets for the rest
//! (the ground shadow map, the environment bake's march, the rain). The sky's own share (bake size, stars) is `naval_sky::SkyQuality`'s. MIT License, Copyright (c) 2024 Shota
//! Matsuda; the full notice is in `THIRD_PARTY.md`.

use naval_sky::SkyQuality;

/// The primary march's settings (`clouds` in takram's presets).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarchSettings {
    /// Octaves of multiple scattering in broken cloud, four at most (slabs and lightning keep takram's eight).
    pub multi_scattering_octaves: u32,
    pub accurate_sun_sky_light: bool,
    pub max_iteration_count: u32,
    pub min_step_size: f32,
    pub max_step_size: f32,
    pub max_ray_distance: f32,
    pub perspective_step_scale: f32,
    pub min_density: f32,
    pub min_extinction: f32,
    pub min_transmittance: f32,
    /// Steps marched toward the ground for the cloud under a sample (the sea's bounce); 0 takes it from the weather.
    pub max_iteration_count_to_ground: u32,
    pub max_iteration_count_to_sun: u32,
    pub min_secondary_step_size: f32,
    pub secondary_step_scale: f32,
    pub max_shadow_length_iteration_count: u32,
    pub min_shadow_length_step_size: f32,
    pub max_shadow_length_ray_distance: f32,
}

/// The Beer shadow maps' settings (`shadow` in takram's presets).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowSettings {
    pub cascade_count: usize,
    pub map_size: u32,
    pub max_iteration_count: u32,
    pub min_step_size: f32,
    pub max_step_size: f32,
    pub min_density: f32,
    pub min_extinction: f32,
    pub min_transmittance: f32,
}

/// One tier's budget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloudTier {
    /// takram's switches.
    pub light_shafts: bool,
    pub shape_detail: bool,
    pub turbulence: bool,
    pub haze: bool,
    pub clouds: MarchSettings,
    pub shadow: ShadowSettings,
    /// Texels per edge of the ground shadow map other layers read, and its march's steps.
    pub ground_size: u32,
    pub ground_steps: u32,
    /// The environment bake's cloud march steps.
    pub environment_steps: u32,
    /// Rain streaks around the camera at full precipitation.
    pub rain_drops: u32,
    /// Rain shafts under a downpour's clouds (`crate::weathers`): a layer marched from the camera, about half a
    /// millisecond on Medium at 2400 × 1350, so from High up.
    pub rain_shafts: bool,
}

/// takram's High preset, its defaults but two, which Ultra inherits: the light shafts' march starts from 200 m steps
/// (takram's 50 m; it still reaches its 200 km), and no step is marched toward the ground for the light the sea bounces
/// onto the clouds (takram's three): every tier takes the bounce through the cloud its weather puts under a sample
/// (`shaders/march.wgsl`, `radiance_from_ground`; from 2026-10-02 to 2026-10-04 High and Ultra took none, and Medium
/// one marched step). Neither shows beside takram's own (`docs/clouds.md`, "GPU cost").
const HIGH: CloudTier = CloudTier {
    light_shafts: true,
    shape_detail: true,
    turbulence: true,
    // takram's thin haze stays off: the scene's haze is the aerial perspective's (`crate::aerial`), which the clouds'
    // march takes at their front like every material.
    haze: false,
    clouds: MarchSettings {
        // Broken cloud's octaves on every tier (takram's presets have eight; `shaders/march.wgsl`, `sun_scattering`):
        // a ray takes their phases once, so two cost what three do, and two leave the clouds 4 to 7% darker.
        multi_scattering_octaves: 3,
        accurate_sun_sky_light: true,
        max_iteration_count: 500,
        min_step_size: 50.0,
        max_step_size: 1000.0,
        max_ray_distance: 2e5,
        perspective_step_scale: 1.01,
        min_density: 1e-5,
        min_extinction: 1e-5,
        min_transmittance: 1e-2,
        max_iteration_count_to_ground: 0,
        max_iteration_count_to_sun: 2,
        min_secondary_step_size: 100.0,
        secondary_step_scale: 2.0,
        max_shadow_length_iteration_count: 500,
        min_shadow_length_step_size: 200.0,
        max_shadow_length_ray_distance: 2e5,
    },
    shadow: ShadowSettings {
        cascade_count: 3,
        map_size: 512,
        max_iteration_count: 50,
        min_step_size: 100.0,
        max_step_size: 1000.0,
        min_density: 1e-5,
        min_extinction: 1e-5,
        min_transmittance: 1e-4,
    },
    ground_size: 1024,
    ground_steps: 24,
    environment_steps: 24,
    rain_drops: 12000,
    rain_shafts: true,
};

pub fn tier(quality: SkyQuality) -> CloudTier {
    match quality {
        // No light shafts, shape detail or turbulence, a coarser march, the light from the layer's ends.
        SkyQuality::Low => CloudTier {
            light_shafts: false,
            shape_detail: false,
            turbulence: false,
            clouds: MarchSettings {
                accurate_sun_sky_light: false,
                max_iteration_count: 200,
                min_step_size: 100.0,
                max_ray_distance: 1e5,
                min_density: 1e-4,
                min_extinction: 1e-4,
                min_transmittance: 1e-1,
                max_iteration_count_to_sun: 1,
                ..HIGH.clouds
            },
            shadow: ShadowSettings {
                cascade_count: 2,
                map_size: 256,
                max_iteration_count: 25,
                min_density: 1e-4,
                min_extinction: 1e-4,
                min_transmittance: 1e-2,
                ..HIGH.shadow
            },
            ground_size: 512,
            ground_steps: 12,
            environment_steps: 12,
            rain_drops: 3000,
            rain_shafts: false,
            ..HIGH
        },
        // No light shafts or turbulence.
        SkyQuality::Medium => CloudTier {
            light_shafts: false,
            turbulence: false,
            clouds: MarchSettings {
                min_density: 1e-4,
                min_extinction: 1e-4,
                accurate_sun_sky_light: false,
                max_iteration_count_to_sun: 2,
                ..HIGH.clouds
            },
            shadow: ShadowSettings {
                min_density: 1e-4,
                min_extinction: 1e-4,
                map_size: 256,
                ..HIGH.shadow
            },
            ground_size: 1024,
            ground_steps: 16,
            environment_steps: 16,
            rain_drops: 8000,
            rain_shafts: false,
            ..HIGH
        },
        SkyQuality::High => HIGH,
        // Finer primary steps and larger shadow maps: 768² (takram's 1024²) keep the dusk light shafts that 512² lose,
        // for about 1 ms less.
        SkyQuality::Ultra => CloudTier {
            clouds: MarchSettings {
                min_step_size: 10.0,
                ..HIGH.clouds
            },
            shadow: ShadowSettings {
                map_size: 768,
                ..HIGH.shadow
            },
            ground_steps: 32,
            environment_steps: 32,
            rain_drops: 20000,
            ..HIGH
        },
    }
}

/// The most rain drops any tier draws (the rain's capacity).
pub const MOST_RAIN_DROPS: u32 = 20000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tiers_are_takrams_presets_less_three_trims() {
        let low = tier(SkyQuality::Low);
        assert!(!low.light_shafts && !low.shape_detail && !low.turbulence && !low.haze);
        assert_eq!(
            (
                low.shadow.cascade_count,
                low.shadow.map_size,
                low.clouds.max_iteration_count_to_sun
            ),
            (2, 256, 1)
        );
        let medium = tier(SkyQuality::Medium);
        assert!(!medium.light_shafts && medium.shape_detail && !medium.turbulence);
        let high = tier(SkyQuality::High);
        assert!(
            high.light_shafts
                && high.shape_detail
                && high.turbulence
                && high.clouds.accurate_sun_sky_light
        );
        // The trims: the shafts from 200 m steps, no step toward the ground on any tier, and Ultra's maps at 768².
        assert_eq!(
            (
                high.clouds.min_shadow_length_step_size,
                high.clouds.max_iteration_count_to_ground,
                low.clouds.max_iteration_count_to_ground
            ),
            (200.0, 0, 0)
        );
        // Broken cloud's octaves are the game's, the same on every tier.
        assert_eq!(
            (
                low.clouds.multi_scattering_octaves,
                high.clouds.multi_scattering_octaves
            ),
            (3, 3)
        );
        let ultra = tier(SkyQuality::Ultra);
        assert_eq!(
            (ultra.clouds.min_step_size, ultra.shadow.map_size),
            (10.0, 768)
        );
        assert!(tier(SkyQuality::Ultra).rain_drops <= MOST_RAIN_DROPS);
    }
}
