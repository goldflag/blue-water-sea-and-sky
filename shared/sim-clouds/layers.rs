//! The four cloud layers of @takram/three-clouds (`CloudLayer.ts`, `CloudLayers.ts`, `DensityProfile.ts`) and how
//! they pack into the shaders' vec4s (`updateCloudLayerUniforms` in `uniforms.ts`): each layer reads the local weather
//! texture between its altitude and altitude + height, shaped by its density scale, shape and detail amounts, weather
//! exponent, shape-altering bias, coverage filter width and density profile; the march strides over the gaps between
//! layers. Fair weather draws takram's default layers ([`Layers::takram`]); other weathers draw their own
//! (`crate::weathers`).
//!
//! Three departures from takram, none of which changes its default layers: a layer reads a weighted mix of the weather
//! texture's channels (takram: one channel), has a coverage of its own (takram: one coverage for all four), and casts
//! shadows by a weight (takram: a flag), so that two sets of layers blend continuously while the weather changes
//! ([`Layers::lerp`]).

/// A layer's density by its normalised height h: `exp_term · exp(exponent · h) + linear_term · h + constant_term`.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DensityProfile {
    pub exp_term: f32,
    pub exponent: f32,
    pub linear_term: f32,
    pub constant_term: f32,
}

impl DensityProfile {
    /// takram's default: `0.75 h + 0.25`.
    pub const TAKRAM: Self = Self {
        exp_term: 0.0,
        exponent: 0.0,
        linear_term: 0.75,
        constant_term: 0.25,
    };

    fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            exp_term: lerp(self.exp_term, other.exp_term, t),
            exponent: lerp(self.exponent, other.exponent, t),
            linear_term: lerp(self.linear_term, other.linear_term, t),
            constant_term: lerp(self.constant_term, other.constant_term, t),
        }
    }
}

/// A channel of the local weather texture: takram's layers read one each (R, G, B, and A for the empty fourth).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    R,
    G,
    B,
    A,
}

impl Channel {
    /// The channel alone, as a layer's weights.
    pub const fn weights(self) -> [f32; 4] {
        match self {
            Channel::R => [1.0, 0.0, 0.0, 0.0],
            Channel::G => [0.0, 1.0, 0.0, 0.0],
            Channel::B => [0.0, 0.0, 1.0, 0.0],
            Channel::A => [0.0, 0.0, 0.0, 1.0],
        }
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// One layer (takram's `CloudLayer`). Heights in metres above the sea. A layer may start below the sea: takram's
/// shape-altering function thins every layer to nothing at its bottom, so fog and rain that reach the sea start lower.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Layer {
    /// The weather texture's channels the layer reads, weighted (takram's one channel: [`Channel::weights`]).
    pub channels: [f32; 4],
    pub altitude: f32,
    pub height: f32,
    pub density_scale: f32,
    pub shape_amount: f32,
    pub shape_detail_amount: f32,
    pub weather_exponent: f32,
    pub shape_altering_bias: f32,
    pub coverage_filter_width: f32,
    pub density_profile: DensityProfile,
    /// The share of the sky the layer covers, 0–1 (takram's `coverage`, one for all four layers there).
    pub coverage: f32,
    /// Whether the layer casts shadows (into the Beer shadow maps and onto the sea): 1 or 0, between while two sets of
    /// layers blend.
    pub shadow: f32,
    /// Whether the layer is a closed slab (a deck, fog), 1, or broken cloud (takram's cumulus, a high sheet), 0: under
    /// and inside a slab the light from above is the light diffused down through it (`march::deck_skylight`), not
    /// takram's open sky. Between while two sets of layers blend.
    pub slab: f32,
}

impl Default for Layer {
    /// takram's `CloudLayer.DEFAULT`: no height, so it draws nothing.
    fn default() -> Self {
        Self {
            channels: Channel::R.weights(),
            altitude: 0.0,
            height: 0.0,
            density_scale: 0.2,
            shape_amount: 1.0,
            shape_detail_amount: 1.0,
            weather_exponent: 1.0,
            shape_altering_bias: 0.35,
            coverage_filter_width: 0.6,
            density_profile: DensityProfile::TAKRAM,
            coverage: 0.0,
            shadow: 0.0,
            slab: 0.0,
        }
    }
}

impl Layer {
    /// `self` at `t` = 0, `other` at 1, every parameter between. An empty layer (no height) takes the other's place and
    /// shape and grows from nothing there, rather than sweeping up from the sea.
    pub fn lerp(&self, other: &Layer, t: f32) -> Layer {
        if t <= 0.0 {
            return *self;
        }
        if t >= 1.0 {
            return *other;
        }
        let empty_at = |layer: &Layer| Layer {
            height: 0.0,
            coverage: 0.0,
            ..*layer
        };
        let (from, to) = match (self.height > 0.0, other.height > 0.0) {
            (false, true) => (empty_at(other), *other),
            (true, false) => (*self, empty_at(self)),
            _ => (*self, *other),
        };
        Layer {
            channels: std::array::from_fn(|i| lerp(from.channels[i], to.channels[i], t)),
            altitude: lerp(from.altitude, to.altitude, t),
            height: lerp(from.height, to.height, t),
            density_scale: lerp(from.density_scale, to.density_scale, t),
            shape_amount: lerp(from.shape_amount, to.shape_amount, t),
            shape_detail_amount: lerp(from.shape_detail_amount, to.shape_detail_amount, t),
            weather_exponent: lerp(from.weather_exponent, to.weather_exponent, t),
            shape_altering_bias: lerp(from.shape_altering_bias, to.shape_altering_bias, t),
            coverage_filter_width: lerp(from.coverage_filter_width, to.coverage_filter_width, t),
            density_profile: from.density_profile.lerp(to.density_profile, t),
            coverage: lerp(from.coverage, to.coverage, t),
            shadow: lerp(from.shadow, to.shadow, t),
            slab: lerp(from.slab, to.slab, t),
        }
    }
}

/// The four layers (takram's `CloudLayers`).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Layers(pub [Layer; 4]);

/// The layers as the shaders read them (`CloudLayerUniforms`), one layer per component.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Packed {
    pub min_layer_heights: [f32; 4],
    pub max_layer_heights: [f32; 4],
    /// The gaps between layers, up to three.
    pub min_interval_heights: [f32; 3],
    pub max_interval_heights: [f32; 3],
    pub density_scales: [f32; 4],
    pub shape_amounts: [f32; 4],
    pub shape_detail_amounts: [f32; 4],
    pub weather_exponents: [f32; 4],
    pub shape_altering_biases: [f32; 4],
    pub coverage_filter_widths: [f32; 4],
    pub coverages: [f32; 4],
    pub slabs: [f32; 4],
    pub exp_terms: [f32; 4],
    pub exponents: [f32; 4],
    pub linear_terms: [f32; 4],
    pub constant_terms: [f32; 4],
    /// The lowest layer's bottom and the highest layer's top (m).
    pub min_height: f32,
    pub max_height: f32,
    /// The shadow-casting layers' top and bottom (m).
    pub shadow_top_height: f32,
    pub shadow_bottom_height: f32,
    pub shadow_layer_mask: [f32; 4],
    /// Column i weighs the weather's channels for layer i (`LOCAL_WEATHER_CHANNELS`, one-hot in takram's).
    pub channels: [[f32; 4]; 4],
}

impl Layers {
    /// takram's `CloudLayers.DEFAULT`: low clouds at 750 m (650 m deep) and middle ones at 1 km (1.2 km deep), both
    /// casting shadows, a thin high layer at 7.5 km, and an empty fourth; every layer at takram's one `coverage`.
    pub fn takram(coverage: f32) -> Self {
        let base = Layer {
            coverage,
            ..Layer::default()
        };
        Self([
            Layer {
                channels: Channel::R.weights(),
                altitude: 750.0,
                height: 650.0,
                density_scale: 0.2,
                shape_amount: 1.0,
                shape_detail_amount: 1.0,
                weather_exponent: 1.0,
                shape_altering_bias: 0.35,
                coverage_filter_width: 0.6,
                shadow: 1.0,
                ..base
            },
            Layer {
                channels: Channel::G.weights(),
                altitude: 1000.0,
                height: 1200.0,
                density_scale: 0.2,
                shape_amount: 1.0,
                shape_detail_amount: 1.0,
                weather_exponent: 1.0,
                shape_altering_bias: 0.35,
                coverage_filter_width: 0.6,
                shadow: 1.0,
                ..base
            },
            Layer {
                channels: Channel::B.weights(),
                altitude: 7500.0,
                height: 500.0,
                density_scale: 0.003,
                shape_amount: 0.4,
                shape_detail_amount: 0.0,
                weather_exponent: 1.0,
                shape_altering_bias: 0.35,
                coverage_filter_width: 0.5,
                ..base
            },
            Layer {
                channels: Channel::A.weights(),
                ..base
            },
        ])
    }

    /// `self` at `t` = 0, `other` at 1, layer by layer ([`Layer::lerp`]).
    pub fn lerp(&self, other: &Layers, t: f32) -> Layers {
        Layers(std::array::from_fn(|i| self.0[i].lerp(&other.0[i], t)))
    }

    /// The highest layer's top (m), 0 without any layer.
    pub fn top(&self) -> f32 {
        self.0
            .iter()
            .filter(|layer| layer.height > 0.0)
            .map(|layer| layer.altitude + layer.height)
            .fold(0.0, f32::max)
    }

    /// The lowest layer's bottom (m), 0 without any layer.
    pub fn bottom(&self) -> f32 {
        let bottom = self
            .0
            .iter()
            .filter(|layer| layer.height > 0.0)
            .map(|layer| layer.altitude)
            .fold(f32::INFINITY, f32::min);
        if bottom.is_finite() { bottom } else { 0.0 }
    }

    /// The gaps between the layers' union (`packIntervalHeights`): sorted bottoms and tops, a gap wherever the count
    /// of open layers returns to zero before the next bottom. Unused slots are (0, 0).
    pub fn intervals(&self) -> ([f32; 3], [f32; 3]) {
        // (height, 0 for a bottom or 1 for a top), sorted by height then bottoms first.
        let mut entries: Vec<(f32, u8)> = self
            .0
            .iter()
            .flat_map(|layer| [(layer.altitude, 0), (layer.altitude + layer.height, 1)])
            .collect();
        entries.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut min = [0.0; 3];
        let mut max = [0.0; 3];
        let mut count = 0;
        let mut balance = 0i32;
        for (index, &(value, flag)) in entries.iter().enumerate() {
            if balance == 0 && index > 0 && count < 3 {
                min[count] = entries[index - 1].0;
                max[count] = value;
                count += 1;
            }
            balance += if flag == 0 { 1 } else { -1 };
        }
        (min, max)
    }

    pub fn pack(&self) -> Packed {
        let layers = &self.0;
        let each =
            |value: fn(&Layer) -> f32| -> [f32; 4] { std::array::from_fn(|i| value(&layers[i])) };
        let (min_interval_heights, max_interval_heights) = self.intervals();
        let mut min_height = f32::INFINITY;
        let mut max_height = 0.0f32;
        let mut shadow_bottom = f32::INFINITY;
        let mut shadow_top = 0.0f32;
        let mut shadow_layer_mask = [0.0; 4];
        let mut channels = [[0.0; 4]; 4];
        for (index, layer) in layers.iter().enumerate() {
            let top = layer.altitude + layer.height;
            if layer.height > 0.0 {
                min_height = min_height.min(layer.altitude);
                max_height = max_height.max(top);
                if layer.shadow > 0.0 {
                    shadow_bottom = shadow_bottom.min(layer.altitude);
                    shadow_top = shadow_top.max(top);
                }
            }
            shadow_layer_mask[index] = layer.shadow.clamp(0.0, 1.0);
            channels[index] = layer.channels;
        }
        Packed {
            min_layer_heights: each(|l| l.altitude),
            max_layer_heights: each(|l| l.altitude + l.height),
            min_interval_heights,
            max_interval_heights,
            density_scales: each(|l| l.density_scale),
            shape_amounts: each(|l| l.shape_amount),
            shape_detail_amounts: each(|l| l.shape_detail_amount),
            weather_exponents: each(|l| l.weather_exponent),
            shape_altering_biases: each(|l| l.shape_altering_bias),
            coverage_filter_widths: each(|l| l.coverage_filter_width),
            coverages: each(|l| l.coverage.clamp(0.0, 1.0)),
            slabs: each(|l| l.slab.clamp(0.0, 1.0)),
            exp_terms: each(|l| l.density_profile.exp_term),
            exponents: each(|l| l.density_profile.exponent),
            linear_terms: each(|l| l.density_profile.linear_term),
            constant_terms: each(|l| l.density_profile.constant_term),
            min_height: if min_height.is_finite() {
                min_height
            } else {
                0.0
            },
            max_height,
            shadow_top_height: shadow_top,
            shadow_bottom_height: if shadow_bottom.is_finite() {
                shadow_bottom
            } else {
                0.0
            },
            shadow_layer_mask,
            channels,
        }
    }
}

impl Default for Layers {
    /// takram's layers at takram's default coverage (0.3).
    fn default() -> Self {
        Self::takram(0.3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heights(spans: [(f32, f32); 4]) -> Layers {
        Layers(spans.map(|(altitude, height)| Layer {
            altitude,
            height,
            ..Layer::default()
        }))
    }

    /// takram's `CloudLayers.test.ts`.
    #[test]
    fn intervals_are_the_gaps_between_layers() {
        let (min, max) = heights([(0.0, 1.0), (2.0, 1.0), (4.0, 1.0), (6.0, 1.0)]).intervals();
        assert_eq!((min, max), ([1.0, 3.0, 5.0], [2.0, 4.0, 6.0]));
        let (min, max) = heights([(0.0, 3.0), (2.0, 1.0), (4.0, 2.0), (6.0, 1.0)]).intervals();
        assert_eq!((min, max), ([3.0, 0.0, 0.0], [4.0, 0.0, 0.0]));
        let (min, max) = heights([(0.0, 3.0), (2.0, 0.0), (4.0, 4.0), (6.0, 1.0)]).intervals();
        assert_eq!((min, max), ([3.0, 0.0, 0.0], [4.0, 0.0, 0.0]));
    }

    #[test]
    fn the_default_layers_pack_as_takram_packs_them() {
        let packed = Layers::takram(0.4).pack();
        assert_eq!(packed.min_layer_heights, [750.0, 1000.0, 7500.0, 0.0]);
        assert_eq!(packed.max_layer_heights, [1400.0, 2200.0, 8000.0, 0.0]);
        // Below the clouds, and between the middle and the high layer.
        assert_eq!(packed.min_interval_heights, [0.0, 2200.0, 0.0]);
        assert_eq!(packed.max_interval_heights, [750.0, 7500.0, 0.0]);
        assert_eq!((packed.min_height, packed.max_height), (750.0, 8000.0));
        assert_eq!(
            (packed.shadow_bottom_height, packed.shadow_top_height),
            (750.0, 2200.0)
        );
        assert_eq!(packed.shadow_layer_mask, [1.0, 1.0, 0.0, 0.0]);
        assert_eq!(packed.density_scales, [0.2, 0.2, 0.003, 0.2]);
        assert_eq!(packed.linear_terms, [0.75; 4]);
        assert_eq!(packed.constant_terms, [0.25; 4]);
        assert_eq!(packed.coverages, [0.4; 4]);
        // Layer i reads channel i.
        for (index, column) in packed.channels.iter().enumerate() {
            let mut one_hot = [0.0; 4];
            one_hot[index] = 1.0;
            assert_eq!(*column, one_hot);
        }
        assert_eq!(Layers::takram(0.4).top(), 8000.0);
        assert_eq!(Layers::takram(0.4).bottom(), 750.0);
    }

    #[test]
    fn layers_blend_from_one_set_to_another() {
        let fair = Layers::takram(0.4);
        let mut deck = fair;
        deck.0[0] = Layer {
            channels: Channel::A.weights(),
            altitude: 400.0,
            height: 1800.0,
            coverage: 0.9,
            density_scale: 0.05,
            shadow: 1.0,
            ..Layer::default()
        };
        assert_eq!(fair.lerp(&deck, 0.0), fair);
        assert_eq!(fair.lerp(&deck, 1.0), deck);
        let half = fair.lerp(&deck, 0.5).0[0];
        assert_eq!(half.altitude, 575.0);
        assert_eq!(half.channels, [0.5, 0.0, 0.0, 0.5]);
        assert!((half.coverage - 0.65).abs() < 1e-6);
        // An empty layer grows where the other one is, from nothing.
        let mut rain = fair;
        rain.0[3] = Layer {
            altitude: -100.0,
            height: 500.0,
            coverage: 0.5,
            ..Layer::default()
        };
        let growing = fair.lerp(&rain, 0.25).0[3];
        assert_eq!((growing.altitude, growing.height), (-100.0, 125.0));
        assert!((growing.coverage - 0.125).abs() < 1e-6);
        // And goes back to nothing in place.
        let shrinking = rain.lerp(&fair, 0.75).0[3];
        assert_eq!((shrinking.altitude, shrinking.height), (-100.0, 125.0));
    }
}
