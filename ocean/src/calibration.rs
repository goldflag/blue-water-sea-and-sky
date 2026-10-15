//! The sea the game sets for a scene: the calibrated wind sea (`src/maps/seaCalibration.ts` over
//! `assets/maps/battle-conditions.v1.json`), a map's water (`assets/maps/environments.v1.json`) and how
//! the web game's `VisualEnvironment.ts` (`applySea`, `applyFog`) turns them into the ocean's parameters for the port and for
//! a battle. The table is copied here (a test checks it against the JSON) so the crate reads no file at run time.

use crate::gusts::GustParameters;
use crate::spectrum::WaveParameters;
use crate::whitecaps::windrow_coverage;
use crate::{
    CrestFoam, FoamParameters, FogParameters, OceanParameters, ShorelineFoam, SurfaceFoam,
    WaterColors,
};

/// One row of the calibration table: wind (m/s), significant height (m), peak wavelength (m).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeaSample {
    pub wind_speed: f64,
    pub significant_height: f64,
    pub peak_wavelength: f64,
}

/// `seaCalibration.samples` in `assets/maps/battle-conditions.v1.json`.
pub const SEA_CALIBRATION: [SeaSample; 15] = [
    SeaSample {
        wind_speed: 0.0,
        significant_height: 0.0,
        peak_wavelength: 6.4,
    },
    SeaSample {
        wind_speed: 1.0,
        significant_height: 0.05,
        peak_wavelength: 6.4,
    },
    SeaSample {
        wind_speed: 2.0,
        significant_height: 0.1,
        peak_wavelength: 7.111111,
    },
    SeaSample {
        wind_speed: 3.0,
        significant_height: 0.15,
        peak_wavelength: 10.666667,
    },
    SeaSample {
        wind_speed: 4.0,
        significant_height: 0.4,
        peak_wavelength: 14.222222,
    },
    SeaSample {
        wind_speed: 5.0,
        significant_height: 0.6,
        peak_wavelength: 17.777778,
    },
    SeaSample {
        wind_speed: 6.0,
        significant_height: 0.9,
        peak_wavelength: 21.333333,
    },
    SeaSample {
        wind_speed: 7.0,
        significant_height: 1.2,
        peak_wavelength: 24.888889,
    },
    SeaSample {
        wind_speed: 9.0,
        significant_height: 1.8,
        peak_wavelength: 32.0,
    },
    SeaSample {
        wind_speed: 12.0,
        significant_height: 2.7,
        peak_wavelength: 42.666667,
    },
    SeaSample {
        wind_speed: 15.0,
        significant_height: 4.0,
        peak_wavelength: 53.333333,
    },
    SeaSample {
        wind_speed: 18.0,
        significant_height: 5.5,
        peak_wavelength: 59.8528,
    },
    SeaSample {
        wind_speed: 21.0,
        significant_height: 7.0,
        peak_wavelength: 60.151467,
    },
    SeaSample {
        wind_speed: 25.0,
        significant_height: 8.8,
        peak_wavelength: 62.222222,
    },
    SeaSample {
        wind_speed: 30.0,
        significant_height: 11.3,
        peak_wavelength: 74.666667,
    },
];

/// A map's water (`OceanMap.water`), colours as sRGB hex the way the JSON writes them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapWater {
    pub id: &'static str,
    pub water_color: u32,
    pub transmission_color: u32,
    pub absorption_color: u32,
    pub amplitude_scale: f64,
    pub wind_scale: f64,
    pub wavelength_scale: f64,
    /// Degrees in the chart frame, from +X toward +Z.
    pub wind_direction: f64,
    pub foam: f64,
    /// The map's fog: colour, start, end, sky blend (m).
    pub fog: (u32, f32, f32, f32),
}

/// The open-sea map every battle defaults to, and the water the port shows.
pub const NORTH_ATLANTIC: MapWater = MapWater {
    id: "north-atlantic",
    water_color: 0x2d373c,
    transmission_color: 0x49575e,
    absorption_color: 0x945b57,
    amplitude_scale: 1.0,
    wind_scale: 1.0,
    wavelength_scale: 1.0,
    wind_direction: 35.0,
    foam: 0.45,
    fog: (0x8b8f92, 2800.0, 25000.0, 9900.0),
};

/// Every battle map's water (`assets/maps/environments.v1.json`, in its order; a test checks it against the JSON),
/// the North Atlantic first. The terrain lane's map selection (`naval_terrain::catalog`) resolves a map's sea through
/// [`map_water`] and [`scene_parameters`].
pub const MAP_WATERS: [MapWater; 8] = [
    NORTH_ATLANTIC,
    MapWater {
        id: "iron-bottom-sound",
        water_color: 0x2d373c,
        transmission_color: 0x49575e,
        absorption_color: 0xb67b45,
        amplitude_scale: 0.65,
        wind_scale: 0.7,
        wavelength_scale: 0.8,
        wind_direction: 270.0,
        foam: 0.25,
        fog: (0x8fc4cf, 3300.0, 20000.0, 10500.0),
    },
    MapWater {
        id: "vestfjord",
        water_color: 0x2d373c,
        transmission_color: 0x49575e,
        absorption_color: 0x916d59,
        amplitude_scale: 0.6,
        wind_scale: 0.65,
        wavelength_scale: 0.8,
        wind_direction: 240.0,
        foam: 0.3,
        fog: (0xafc4ca, 2500.0, 35000.0, 9200.0),
    },
    MapWater {
        id: "sunda-strait",
        water_color: 0x2d373c,
        transmission_color: 0x49575e,
        absorption_color: 0xa97851,
        amplitude_scale: 0.8,
        wind_scale: 0.9,
        wavelength_scale: 0.9,
        wind_direction: 105.0,
        foam: 0.35,
        fog: (0xb3a895, 2600.0, 15000.0, 9700.0),
    },
    MapWater {
        id: "strait-of-dover",
        water_color: 0x2d373c,
        transmission_color: 0x49575e,
        absorption_color: 0x8a6a55,
        amplitude_scale: 0.9,
        wind_scale: 1.0,
        wavelength_scale: 0.85,
        wind_direction: 270.0,
        foam: 0.4,
        fog: (0x9aa4a8, 2400.0, 12000.0, 9000.0),
    },
    MapWater {
        id: "ravnsund",
        water_color: 0x2d373c,
        transmission_color: 0x49575e,
        absorption_color: 0x916d59,
        amplitude_scale: 0.6,
        wind_scale: 0.65,
        wavelength_scale: 0.8,
        wind_direction: 250.0,
        foam: 0.3,
        fog: (0xaec3c9, 2500.0, 35000.0, 9200.0),
    },
    MapWater {
        id: "kawah-lagoon",
        water_color: 0x2d373c,
        transmission_color: 0x49575e,
        absorption_color: 0xb07a48,
        amplitude_scale: 0.65,
        wind_scale: 0.7,
        wavelength_scale: 0.8,
        wind_direction: 110.0,
        foam: 0.25,
        fog: (0x94c3cc, 3200.0, 20000.0, 10200.0),
    },
    MapWater {
        id: "galeb-channel",
        water_color: 0x2d373c,
        transmission_color: 0x49575e,
        absorption_color: 0x8a6a55,
        amplitude_scale: 0.85,
        wind_scale: 0.95,
        wavelength_scale: 0.85,
        wind_direction: 250.0,
        foam: 0.4,
        fog: (0x9ba6aa, 2400.0, 20000.0, 9000.0),
    },
];

/// A map's water by its id (`"vestfjord"`, …); `None` for an unknown map.
pub fn map_water(id: &str) -> Option<&'static MapWater> {
    MAP_WATERS.iter().find(|map| map.id == id)
}

/// The port's standing wind (`PORT_WIND`): 9 m/s toward 35°.
pub const PORT_WIND: (f64, f64) = (9.0, 35.0);
/// The map foam value whose whitecaps cover what the wind calls for (the Atlantic's).
const REFERENCE_FOAM: f64 = 0.45;
const WINDROW_OPACITY: f32 = 0.5;
/// e-folding lifetime of whitecap foam in periods of the breaking waves.
pub const WHITECAP_LIFETIME: f64 = 1.0;
/// The ocean's seed as `Game.ts` creates it.
pub const OCEAN_SEED: f64 = 1941.0;

/// The calibrated wind sea at `speed` on `map` (`windSea`): significant height, peak wavelength and choppiness.
pub fn wind_sea(map: &MapWater, speed: f64) -> (f64, f64, f64, f64) {
    let wind = speed.clamp(0.0, 30.0);
    let upper = SEA_CALIBRATION
        .iter()
        .position(|s| s.wind_speed >= wind)
        .unwrap_or(SEA_CALIBRATION.len() - 1);
    let a = SEA_CALIBRATION[upper.saturating_sub(1)];
    let b = SEA_CALIBRATION[upper];
    let t = if upper == 0 {
        0.0
    } else {
        (wind - a.wind_speed) / (b.wind_speed - a.wind_speed)
    };
    let mix = |x: f64, y: f64| x + (y - x) * t;
    (
        wind,
        mix(a.significant_height, b.significant_height) * map.amplitude_scale,
        mix(a.peak_wavelength, b.peak_wavelength) * map.wavelength_scale,
        0.65 + 0.9 * ((wind - 3.0) / 12.0).clamp(0.0, 1.0),
    )
}

/// three's `Color.set(hex)`: sRGB channels decoded to linear.
pub fn srgb_hex(hex: u32) -> [f32; 3] {
    let decode = |c: u32| {
        let c = f64::from(c) / 255.0;
        (if c < 0.04045 {
            c * 0.077_399_380_804_953_56
        } else {
            (c * 0.947_867_298_578_199_1 + 0.052_132_701_421_800_95).powf(2.4)
        }) as f32
    };
    [
        decode(hex >> 16 & 255),
        decode(hex >> 8 & 255),
        decode(hex & 255),
    ]
}

/// The port's haze: start, end and sky blend (m), the ramp's power and its colour. A fair day at sea, 22 km visibility
/// ("very good"; the battle maps' close at 15–18 km): the ramp `((d − 0.4 km) / 15.8 km)^0.48` follows Koschmieder's
/// `1 − e^(−3.9 d / 22 km)` within 0.055 from the berth out to 20 km (none at 0.4 km, 0.42 at 3 km, 0.79 at 10, closed at
/// 16.2 where the curve is at 0.94), so hills a few kilometres off take a little haze and the horizon goes into the sky.
/// The web's showroom fog closed at 5.6 km.
pub const PORT_FOG: (f32, f32, f32) = (400.0, 16200.0, 9400.0);
pub const PORT_FOG_POWER: f32 = 0.48;
pub const PORT_FOG_COLOR: u32 = 0x819aa5;

/// The ocean's parameters for a scene with `wind` (m/s) on `map` (`VisualEnvironment.applySea` and `applyFog`), by
/// day. `port` takes the port's wind direction and its haze ([`PORT_FOG`]).
pub fn scene_parameters(map: &MapWater, wind: f64, port: bool) -> OceanParameters {
    let (wind_speed, significant_height, peak_wavelength, choppiness) = wind_sea(map, wind);
    let direction = if port {
        PORT_WIND.1
    } else {
        map.wind_direction
    };
    let coverage_scale = map.foam / REFERENCE_FOAM;
    let waves = WaveParameters {
        significant_height,
        wind_speed,
        wind_direction: direction.to_radians(),
        peak_wavelength,
        choppiness,
        gamma: 2.6,
        directional_sharpness: 0.8,
        seed: OCEAN_SEED,
        equilibrium_range: false,
    };
    let fog = if port {
        FogParameters {
            color: srgb_hex(PORT_FOG_COLOR),
            start: PORT_FOG.0,
            end: PORT_FOG.1,
            power: PORT_FOG_POWER,
            sky_blend_distance: PORT_FOG.2,
        }
    } else {
        FogParameters {
            color: srgb_hex(map.fog.0),
            start: map.fog.1,
            end: map.fog.2,
            power: 1.4,
            sky_blend_distance: map.fog.3,
        }
    };
    OceanParameters {
        waves,
        water: crate::optics::WaterField::default(),
        gusts: GustParameters::default(),
        colors: WaterColors {
            water: srgb_hex(map.water_color),
            transmission: srgb_hex(map.transmission_color),
            absorption: srgb_hex(map.absorption_color),
        },
        foam: FoamParameters {
            crest: CrestFoam {
                coverage_scale,
                lifetime: WHITECAP_LIFETIME,
                color: [1.0; 3],
                opacity: 1.0,
                wind_stretch: 0.5,
            },
            surface: SurfaceFoam {
                color: [1.0; 3],
                opacity: WINDROW_OPACITY,
                coverage: windrow_coverage(wind_speed, coverage_scale),
            },
            shoreline: ShorelineFoam {
                color: srgb_hex(0xedf9fd),
                opacity: 0.8,
            },
        },
        fog,
        sea_state: true,
        gusts_enabled: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_the_game_json() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../assets/maps/battle-conditions.v1.json"
        );
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let samples = json["seaCalibration"]["samples"].as_array().unwrap();
        assert_eq!(samples.len(), SEA_CALIBRATION.len());
        for (sample, ours) in samples.iter().zip(SEA_CALIBRATION) {
            assert_eq!(sample["windSpeed"].as_f64().unwrap(), ours.wind_speed);
            assert_eq!(
                sample["significantHeightM"].as_f64().unwrap(),
                ours.significant_height
            );
            assert_eq!(
                sample["peakWavelengthM"].as_f64().unwrap(),
                ours.peak_wavelength
            );
        }
    }

    #[test]
    fn every_map_water_matches_the_game_json() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../assets/maps/environments.v1.json"
        );
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let maps = json["maps"].as_array().unwrap();
        assert_eq!(maps.len(), MAP_WATERS.len());
        let hex =
            |s: &serde_json::Value| u32::from_str_radix(&s.as_str().unwrap()[1..], 16).unwrap();
        for (map, ours) in maps.iter().zip(MAP_WATERS) {
            assert_eq!(map["id"].as_str().unwrap(), ours.id);
            assert_eq!(map_water(ours.id), Some(&ours));
            let water = &map["water"];
            assert_eq!(hex(&water["waterColor"]), ours.water_color);
            assert_eq!(hex(&water["transmissionColor"]), ours.transmission_color);
            assert_eq!(hex(&water["absorptionColor"]), ours.absorption_color);
            assert_eq!(
                water["amplitudeScale"].as_f64().unwrap(),
                ours.amplitude_scale
            );
            assert_eq!(water["windScale"].as_f64().unwrap(), ours.wind_scale);
            assert_eq!(
                water["wavelengthScale"].as_f64().unwrap(),
                ours.wavelength_scale
            );
            assert_eq!(
                water["windDirection"].as_f64().unwrap(),
                ours.wind_direction
            );
            assert_eq!(water["foam"].as_f64().unwrap(), ours.foam);
            let fog = &map["fog"];
            assert_eq!(hex(&fog["color"]), ours.fog.0);
            assert_eq!(fog["start"].as_f64().unwrap() as f32, ours.fog.1);
            assert_eq!(fog["end"].as_f64().unwrap() as f32, ours.fog.2);
            assert_eq!(fog["skyBlend"].as_f64().unwrap() as f32, ours.fog.3);
        }
        assert_eq!(map_water("nowhere"), None);
    }

    #[test]
    fn port_sea_is_the_nine_metre_a_second_row() {
        let (wind, height, peak, chop) = wind_sea(&NORTH_ATLANTIC, 9.0);
        assert_eq!((wind, height, peak), (9.0, 1.8, 32.0));
        assert!((chop - 1.1).abs() < 1e-12);
        let fog = srgb_hex(0x8b8f92);
        assert!((fog[0] - 0.2582).abs() < 1e-4);
    }
}
