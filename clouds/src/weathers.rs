//! The layers a weather draws, within takram's march: fair weather keeps takram's layers ([`Layers::takram`]); a
//! closing sky lowers and thickens them into a stratocumulus deck with structure under it (rolls, thick and thin
//! patches, gaps), a storm into a dark nimbostratus with a low ragged base, towers and rain shafts under its cells; the
//! fog weather lays fog banks on the sea; a genus the scene names draws its own layer at the base and depth it asks.
//!
//! Every weather fills the same four slots, so two weathers blend parameter by parameter while the weather changes
//! ([`Layers::lerp`]):
//!
//! | Slot | Fair (takram) | Deck | Storm |
//! | --- | --- | --- | --- |
//! | 0 | low cumulus, 750–1400 m | the stratocumulus deck | the nimbostratus, ragged to its base |
//! | 1 | middle cumulus, 1–2.2 km | a thin altostratus over it | towers through it |
//! | 2 | the high layer, 7.5–8 km | as fair, or the high sheet the scene names | a thicker sheet (the anvils) |
//! | 3 | empty | rain under the deck | rain shafts under the cells |
//!
//! The fog weather's banks take slot 3 (fog and rain do not come together). The weather texture's channels: R and G are
//! takram's cumulus cells, B its broad streaky field (the high layer), A a finer field takram leaves unused.

use naval_sky::{CloudGenus, CloudScene, HighClouds, SkyScene};

use crate::layers::Layers;

/// What the clouds read of a scene ([`CloudWeather::of`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloudWeather {
    /// 0 clear … 1 overcast (takram's coverage).
    pub coverage: f32,
    /// The base (m) and depth (m) the scene chose (a genus, the battle rail, the console); `None` lets the weather
    /// put its clouds where they stand.
    pub base: Option<f32>,
    pub depth: Option<f32>,
    pub genus: Option<CloudGenus>,
    pub high: Option<HighClouds>,
    /// Fog banks on the sea, 0–1.
    pub fog: f32,
    /// Rain, 0–1.
    pub precipitation: f32,
    /// Whether rain shafts are drawn (the tier's: `quality::CloudTier::rain_shafts`).
    pub rain_shafts: bool,
}

impl CloudWeather {
    pub fn simulation(&self) -> naval_sim::clouds::CloudWeather {
        naval_sim::clouds::CloudWeather {
            coverage: self.coverage,
            base: self.base,
            depth: self.depth,
            genus: self.genus,
            high: self.high,
            fog: self.fog,
            precipitation: self.precipitation,
            rain_shafts: self.rain_shafts,
        }
    }
    /// What a scene asks of the clouds, rain shafts drawn or not.
    pub fn of(scene: &SkyScene, rain_shafts: bool) -> Self {
        let clouds: &CloudScene = &scene.clouds;
        Self {
            coverage: clouds.coverage.clamp(0.0, 1.0) as f32,
            base: clouds.base_chosen.then_some(clouds.altitude as f32),
            depth: clouds.depth_chosen.then_some(clouds.thickness as f32),
            genus: clouds.genus,
            high: clouds.high,
            fog: clouds.fog.clamp(0.0, 1.0) as f32,
            precipitation: scene.weather.precipitation.clamp(0.0, 1.0) as f32,
            rain_shafts,
        }
    }
}

/// The layers of a scene's weather, rain shafts drawn or not.
pub fn layers(scene: &SkyScene, rain_shafts: bool) -> Layers {
    layers_for(&CloudWeather::of(scene, rain_shafts))
}

/// The common layers, also used by aircraft spotting in the simulation.
pub fn layers_for(weather: &CloudWeather) -> Layers {
    naval_sim::clouds::weathers::layers_for(&weather.simulation())
}

#[cfg(test)]
mod tests {
    use super::*;
    use naval_sky::DEFAULT_SKY_SCENE;

    fn weather(coverage: f32) -> CloudWeather {
        CloudWeather {
            coverage,
            base: None,
            depth: None,
            genus: None,
            high: None,
            fog: 0.0,
            precipitation: 0.0,
            rain_shafts: true,
        }
    }

    #[test]
    fn fair_weather_is_takrams_layers() {
        for cover in [0.0, 0.02, 0.38, 0.4, 0.62] {
            assert_eq!(
                layers_for(&weather(cover)),
                Layers::takram(cover),
                "cover {cover}"
            );
        }
        assert_eq!(layers(&DEFAULT_SKY_SCENE, true), Layers::takram(0.38));
    }

    #[test]
    fn a_closing_sky_is_a_deck_and_a_storm_has_a_low_ragged_base() {
        let overcast = layers_for(&weather(0.82));
        assert_eq!(overcast.0[0].altitude, 800.0);
        // Thinner than takram's cumulus, a slab, and not saturated: its cover leaves its own structure.
        assert!(overcast.0[0].density_scale < Layers::takram(0.82).0[0].density_scale);
        assert_eq!(overcast.0[0].slab, 1.0);
        assert!(overcast.0[0].coverage < 0.82);
        let storm = layers_for(&CloudWeather {
            precipitation: 0.9,
            ..weather(0.94)
        });
        assert!(storm.0[0].altitude <= 300.0);
        // Rain from under the sea to the base.
        assert!(storm.0[3].altitude < 0.0);
        assert_eq!(storm.0[3].altitude + storm.0[3].height, storm.0[0].altitude);
        // The change from one to the other is continuous in the cover.
        let a = layers_for(&weather(0.86));
        let b = layers_for(&weather(0.8601));
        assert!((a.0[0].altitude - b.0[0].altitude).abs() < 1.0);
    }

    #[test]
    fn a_chosen_genus_and_base_are_honoured() {
        let cumulus = layers_for(&CloudWeather {
            genus: Some(CloudGenus::Cumulus),
            base: Some(1500.0),
            depth: Some(2900.0),
            ..weather(0.35)
        });
        assert_eq!(cumulus.bottom(), 1500.0);
        assert!((cumulus.0[1].altitude + cumulus.0[1].height - 4400.0).abs() < 1.0);
        let stratus = layers_for(&CloudWeather {
            genus: Some(CloudGenus::Stratus),
            base: Some(250.0),
            depth: Some(450.0),
            ..weather(0.95)
        });
        assert_eq!((stratus.0[0].altitude, stratus.0[0].height), (250.0, 450.0));
        // A chosen base moves takram's fair layers too.
        let fair = layers_for(&CloudWeather {
            base: Some(1200.0),
            ..weather(0.4)
        });
        assert_eq!(fair.bottom(), 1200.0);
    }

    #[test]
    fn fog_banks_lie_on_the_sea() {
        let fog = layers_for(&CloudWeather {
            fog: 1.0,
            ..weather(0.65)
        });
        let banks = fog.0[3];
        assert!(banks.altitude < 0.0 && banks.altitude + banks.height > 200.0);
        // takram's shape-altering function peaks at the surface.
        let at_sea = -banks.altitude / banks.height;
        assert!((at_sea.powf(banks.shape_altering_bias) - 0.5).abs() < 1e-4);
    }
}
