//! Shared cloud profiles; the native march and simulation use the same layers.
use super::layers::{Channel, DensityProfile, Layer, Layers};
use super::{CloudGenus, HighCloudGenus, HighClouds};

/// Renderer-free weather inputs shared by spotting and the native scene.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
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

/// Rain this light or lighter draws no shafts (the overcast's drizzle, 0.15).
const SHAFT_RAIN: f32 = 0.3;

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A closing sky turns to a deck from this cover to the next (takram's fair layers below; the overcast weather's 0.82
/// is a deck).
const DECK_COVER: [f32; 2] = [0.66, 0.8];
/// And to a storm from this cover to the next (the storm weather's 0.94).
const STORM_COVER: [f32; 2] = [0.87, 0.94];

/// The layers of a weather.
pub fn layers_for(weather: &CloudWeather) -> Layers {
    let c = weather.coverage;
    let mut layers = match weather.genus {
        None => {
            // The cover decides: takram's fair layers, closing into a deck, darkening into a storm.
            let fair = fair(c, weather);
            let deck = deck(c, weather);
            let storm = storm(c, weather);
            fair.lerp(&deck, smoothstep(DECK_COVER[0], DECK_COVER[1], c))
                .lerp(&storm, smoothstep(STORM_COVER[0], STORM_COVER[1], c))
        }
        Some(genus) => genus_layers(genus, weather),
    };
    // The high sheet the scene names, in place of the weather's own.
    if let Some(high) = weather.high {
        layers.0[2] = high_sheet(&high);
    }
    // Rain under the clouds, or fog banks on the sea in its place; over the banks, unless a genus is named, a broken
    // stratus in place of the weather's cumulus.
    if let Some(rain) = rain(weather, &layers) {
        layers.0[3] = rain;
    }
    if weather.fog > 0.0 {
        let fog = weather.fog;
        if weather.genus.is_none() {
            let (base, depth) = heights(weather, 600.0, 350.0);
            let stratus = Layer {
                shape_amount: 0.35,
                shape_detail_amount: 0.4,
                density_scale: 0.04,
                ..deck_layer(base, depth, 0.5)
            };
            layers.0[0] = layers.0[0].lerp(&stratus, fog);
            layers.0[1] = layers.0[1].lerp(&Layer::default(), fog);
        }
        layers.0[3] = fog_banks(fog);
    }
    // A chosen genus describes shape and height; an explicitly clear layer still has no low/middle cloud.
    if weather.coverage <= 0.0 {
        layers.0[0].coverage = 0.0;
        layers.0[1].coverage = 0.0;
    }
    layers
}

/// The base and depth the scene chose, or the weather's own.
fn heights(weather: &CloudWeather, base: f32, depth: f32) -> (f32, f32) {
    (
        weather.base.unwrap_or(base),
        weather.depth.unwrap_or(depth).max(50.0),
    )
}

/// takram's layers, or with a base or depth chosen the same two cumulus layers stretched over it.
fn fair(coverage: f32, weather: &CloudWeather) -> Layers {
    let mut layers = Layers::takram(coverage);
    // takram's cumulus span 750–2200 m: the low layer the bottom 45 %, the middle one from 17 % up.
    let (base, depth) = heights(weather, 750.0, 1450.0);
    if (base, depth) != (750.0, 1450.0) {
        let scale = depth / 1450.0;
        for layer in &mut layers.0[..2] {
            layer.altitude = base + (layer.altitude - 750.0) * scale;
            layer.height *= scale;
        }
    }
    layers
}

/// A stratocumulus deck: lumpy cells of the finer field under the broad one's rolls, thick and thin patches and a few
/// gaps (its cover at 0.55 with a narrow filter is not saturated, where takram's layers at the overcast's 0.82 are one
/// opaque sheet), half a cumulus's density, and a base that undulates with the cells. `coverage` is the layer's
/// (`Layer::coverage`), not the sky's.
fn deck_layer(base: f32, depth: f32, coverage: f32) -> Layer {
    Layer {
        channels: [0.0, 0.0, 0.35, 0.65],
        altitude: base,
        height: depth,
        density_scale: 0.1,
        shape_amount: 0.8,
        shape_detail_amount: 0.6,
        weather_exponent: 1.0,
        shape_altering_bias: 0.6,
        coverage_filter_width: 0.35,
        density_profile: DensityProfile {
            exp_term: 0.0,
            exponent: 0.0,
            linear_term: 0.5,
            constant_term: 0.5,
        },
        coverage,
        shadow: 1.0,
        slab: 1.0,
    }
}

/// A thin, broad altostratus: shades the deck under it in wide patches.
fn altostratus(base: f32, depth: f32, coverage: f32) -> Layer {
    Layer {
        channels: Channel::B.weights(),
        altitude: base,
        height: depth,
        density_scale: 0.008,
        shape_amount: 0.5,
        shape_detail_amount: 0.3,
        weather_exponent: 1.0,
        shape_altering_bias: 0.5,
        coverage_filter_width: 0.5,
        density_profile: DensityProfile::TAKRAM,
        coverage,
        shadow: 1.0,
        slab: 1.0,
    }
}

/// The overcast: a stratocumulus deck at 800 m (700 m deep) under a thin altostratus, takram's high layer over both.
fn deck(coverage: f32, weather: &CloudWeather) -> Layers {
    let (base, depth) = heights(weather, 800.0, 700.0);
    let mut layers = Layers::takram(coverage);
    layers.0[0] = deck_layer(base, depth, 0.55);
    layers.0[1] = altostratus(base + depth + 2000.0, 600.0, 0.55);
    layers
}

/// A storm: nimbostratus from a low base (the deck solid from about 450 m, its ragged scud down to 250 m), towers of
/// the cumulus cells through it, and a thicker high sheet over it all.
fn storm(coverage: f32, weather: &CloudWeather) -> Layers {
    let (base, depth) = heights(weather, 250.0, 2000.0);
    let mut layers = Layers::takram(coverage);
    // Thick and dense (an optical depth in the hundreds where the cells are deepest) over a base the widest at mid
    // height, so the cells' bottoms hang ragged from the solid base, about 600 m, down to the layer's 250 m.
    layers.0[0] = Layer {
        channels: [0.0, 0.0, 0.5, 0.5],
        density_scale: 0.12,
        shape_amount: 0.8,
        shape_detail_amount: 1.0,
        shape_altering_bias: 1.0,
        coverage_filter_width: 0.4,
        density_profile: DensityProfile {
            exp_term: 0.0,
            exponent: 0.0,
            linear_term: 0.4,
            constant_term: 0.6,
        },
        ..deck_layer(base, depth, 0.62)
    };
    // Cumulus heads of the cells, towering a few kilometres out of the deck.
    layers.0[1] = Layer {
        channels: Channel::G.weights(),
        altitude: base + 1000.0,
        height: 4500.0,
        density_scale: 0.12,
        shape_amount: 1.0,
        shape_detail_amount: 1.0,
        weather_exponent: 1.0,
        shape_altering_bias: 0.5,
        coverage_filter_width: 0.6,
        density_profile: DensityProfile::TAKRAM,
        coverage: 0.3,
        shadow: 1.0,
        slab: 0.0,
    };
    layers.0[2] = Layer {
        altitude: 8500.0,
        height: 800.0,
        density_scale: 0.004,
        coverage: 0.85,
        ..layers.0[2]
    };
    layers
}

/// A genus the scene names, at the base and depth it asks (each genus's own by default, `CLOUD_GENERA`).
fn genus_layers(genus: CloudGenus, weather: &CloudWeather) -> Layers {
    let c = weather.coverage;
    let empty = Layer::default();
    match genus {
        CloudGenus::Cumulus => {
            let (base, depth) = heights(weather, 1100.0, 1900.0);
            fair(
                c,
                &CloudWeather {
                    base: Some(base),
                    depth: Some(depth),
                    ..*weather
                },
            )
        }
        CloudGenus::Cumulonimbus => {
            // Towers from a low base over scattered cumulus.
            let (base, depth) = heights(weather, 800.0, 10_000.0);
            let mut layers = Layers::takram(c);
            layers.0[0] = Layer {
                altitude: base,
                ..layers.0[0]
            };
            layers.0[1] = Layer {
                channels: Channel::G.weights(),
                altitude: base,
                height: depth,
                density_scale: 0.12,
                coverage: c.max(0.3),
                shadow: 1.0,
                ..Layers::takram(c).0[1]
            };
            layers
        }
        CloudGenus::Stratocumulus => {
            let (base, depth) = heights(weather, 800.0, 900.0);
            let mut layers = deck(c, weather);
            layers.0[0] = deck_layer(base, depth, c.max(0.5));
            layers.0[1] = empty;
            layers
        }
        CloudGenus::Stratus => {
            // A smooth, low, grey sheet: little shape, a thin layer.
            let (base, depth) = heights(weather, 250.0, 450.0);
            let mut layers = deck(c, weather);
            layers.0[0] = Layer {
                shape_amount: 0.35,
                shape_detail_amount: 0.4,
                density_scale: 0.04,
                ..deck_layer(base, depth, c.max(0.5))
            };
            layers.0[1] = empty;
            layers
        }
        CloudGenus::Nimbostratus => {
            let (base, depth) = heights(weather, 600.0, 3500.0);
            let mut layers = storm(c, weather);
            layers.0[0] = Layer {
                altitude: base,
                height: depth,
                coverage: c.max(0.8),
                ..layers.0[0]
            };
            layers.0[1] = empty;
            layers
        }
        CloudGenus::Altocumulus => {
            // Small cells in a mid-level sheet.
            let (base, depth) = heights(weather, 3500.0, 700.0);
            let mut layers = Layers::takram(c);
            layers.0[0] = Layer {
                channels: Channel::R.weights(),
                altitude: base,
                height: depth,
                density_scale: 0.06,
                shape_amount: 1.0,
                shape_detail_amount: 0.8,
                coverage: c,
                shadow: 1.0,
                ..Layer::default()
            };
            layers.0[1] = empty;
            layers
        }
        CloudGenus::Altostratus => {
            let (base, depth) = heights(weather, 4000.0, 1500.0);
            let mut layers = Layers::takram(c);
            layers.0[0] = altostratus(base, depth, c.max(0.6));
            layers.0[1] = empty;
            layers
        }
    }
}

/// The high sheet a scene names: cirrus streaks (takram's high layer), a smooth cirrostratus veil, or cirrocumulus
/// grains, at its altitude and amount.
fn high_sheet(high: &HighClouds) -> Layer {
    let takram = Layers::takram(0.0).0[2];
    let coverage = high.amount.clamp(0.0, 1.0) as f32;
    let altitude = high.altitude as f32;
    match high.genus {
        HighCloudGenus::Cirrus => Layer {
            altitude,
            coverage,
            ..takram
        },
        HighCloudGenus::Cirrostratus => Layer {
            altitude,
            height: 700.0,
            density_scale: 0.002,
            shape_amount: 0.15,
            coverage_filter_width: 0.8,
            coverage,
            ..takram
        },
        HighCloudGenus::Cirrocumulus => Layer {
            channels: Channel::A.weights(),
            altitude,
            height: 300.0,
            density_scale: 0.006,
            shape_amount: 1.0,
            shape_detail_amount: 0.6,
            coverage,
            ..takram
        },
    }
}

/// Rain shafts from the cloud base to the sea under the wettest cells (the broad field), denser and wider as it rains
/// harder; `None` without rain.
fn rain(weather: &CloudWeather, layers: &Layers) -> Option<Layer> {
    let p = weather.precipitation;
    if !weather.rain_shafts || p <= SHAFT_RAIN {
        return None;
    }
    // From below the sea (takram's shape thins a layer to nothing at its bottom) to the lowest cloud's base.
    let base = layers.0[..2]
        .iter()
        .filter(|layer| layer.height > 0.0)
        .map(|layer| layer.altitude)
        .fold(f32::INFINITY, f32::min);
    let top = if base.is_finite() { base } else { 750.0 };
    Some(Layer {
        // The deck's own channels, so the shafts fall under its thickest cells.
        channels: [0.0, 0.0, 0.5, 0.5],
        altitude: -100.0,
        height: top + 100.0,
        // A downpour's kilometre or two of visibility in a shaft; a drizzle's shafts barely show.
        density_scale: 0.003 * p * p,
        shape_amount: 0.0,
        shape_detail_amount: 0.0,
        weather_exponent: 1.0,
        shape_altering_bias: 0.5,
        coverage_filter_width: 0.3,
        density_profile: DensityProfile {
            exp_term: 0.0,
            exponent: 0.0,
            linear_term: 0.0,
            constant_term: 1.0,
        },
        // A fifth of the sea under shafts in a downpour, a few in a shower.
        coverage: 0.22 + 0.08 * p,
        shadow: 1.0,
        slab: 0.0,
    })
}

/// Fog banks lying on the sea: the finer field patched by the broad one, densest at the surface and thinning to their
/// tops about 250 m up, ragged at the edges; `amount` thickens them and closes the gaps between.
fn fog_banks(amount: f32) -> Layer {
    // The layer starts under the sea so its densest height (where takram's shape-altering function peaks) is the
    // surface: with the bias b the peak stands at 0.5^(1/b) of the depth.
    let (bottom, depth): (f32, f32) = (-50.0, 300.0);
    let bias = 0.5f32.ln() / (-bottom / depth).ln();
    Layer {
        // Banks kilometres across from the broad field, their edges broken by the finer one.
        channels: [0.0, 0.0, 0.7, 0.3],
        altitude: bottom,
        height: depth,
        // A few hundred metres of visibility in the thick of a bank.
        density_scale: 0.02,
        shape_amount: 0.8,
        shape_detail_amount: 0.8,
        weather_exponent: 1.0,
        shape_altering_bias: bias,
        coverage_filter_width: 0.35,
        density_profile: DensityProfile {
            exp_term: 0.0,
            exponent: 0.0,
            linear_term: -0.6,
            constant_term: 1.0,
        },
        // At the surface, about half the sea lies in banks.
        coverage: 0.3 + 0.06 * amount,
        shadow: 1.0,
        slab: 1.0,
    }
}
