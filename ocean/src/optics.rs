//! Geographic water optics. Coefficients are per metre at roughly 620, 550 and 460 nm, in linear space.
//! These are representative visual profiles, not measurements of a particular sea or date. Soft elliptical areas
//! blend the coefficients, never the finished pixel colour. The host rebases their centres in f64 when a theatre
//! scene moves; the same field is then read by the surface and underwater view.

use bytemuck::{Pod, Zeroable};

pub const MAX_WATER_AREAS: usize = 12;
pub const LEGACY_BACKSCATTER: [f32; 3] = [0.00323, 0.00395, 0.00565];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterOptics {
    pub absorption: [f32; 3],
    pub backscatter: [f32; 3],
}

impl WaterOptics {
    pub const CLEAR: Self = Self {
        absorption: [0.32, 0.035, 0.014],
        backscatter: [0.0010, 0.0015, 0.0027],
    };
    pub const OCEANIC: Self = Self {
        absorption: [0.34, 0.055, 0.045],
        backscatter: [0.0030, 0.0040, 0.0055],
    };
    pub const PRODUCTIVE: Self = Self {
        absorption: [0.36, 0.070, 0.150],
        backscatter: [0.0070, 0.0090, 0.0100],
    };
    pub const SEDIMENT: Self = Self {
        absorption: [0.47, 0.36, 0.44],
        backscatter: [0.110, 0.090, 0.070],
    };
    pub const ORGANIC: Self = Self {
        absorption: [0.35, 0.16, 0.60],
        backscatter: [0.0060, 0.0070, 0.0080],
    };

    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "clear" => Self::CLEAR,
            "oceanic" => Self::OCEANIC,
            "productive" => Self::PRODUCTIVE,
            "sediment" => Self::SEDIMENT,
            "organic" => Self::ORGANIC,
            _ => return None,
        })
    }

    pub fn mix(self, other: Self, share: f32) -> Self {
        let blend =
            |a: [f32; 3], b: [f32; 3]| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * share);
        Self {
            absorption: blend(self.absorption, other.absorption),
            backscatter: blend(self.backscatter, other.backscatter),
        }
    }

    /// Backscatter is a small share of particle scattering; this fixed ratio keeps the first version inexpensive.
    pub fn beam(self) -> [f32; 3] {
        std::array::from_fn(|i| self.absorption[i] + 8.0 * self.backscatter[i])
    }

    fn valid(self) -> bool {
        self.absorption
            .into_iter()
            .chain(self.backscatter)
            .all(|v| v.is_finite() && v > 0.0)
    }
}

/// Layout twins of `WaterArea` and `WaterField` in water_properties.wgsl. All members are vec4-aligned.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
struct WaterArea {
    shape: [f32; 4],      // xy centre, zw inverse radii
    absorption: [f32; 4], // w strength
    backscatter: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct WaterField {
    absorption: [f32; 4],  // w enabled
    backscatter: [f32; 4], // w area count
    areas: [WaterArea; MAX_WATER_AREAS],
}

impl WaterField {
    pub fn new(base: WaterOptics) -> Self {
        assert!(
            base.valid(),
            "water coefficients must be finite and positive"
        );
        Self {
            absorption: [
                base.absorption[0],
                base.absorption[1],
                base.absorption[2],
                1.0,
            ],
            backscatter: [
                base.backscatter[0],
                base.backscatter[1],
                base.backscatter[2],
                0.0,
            ],
            ..Self::default()
        }
    }

    /// Areas are applied in authored order, broad waters first, local plumes last. Centres are scene-local metres.
    pub fn add_area(
        &mut self,
        centre: [f64; 2],
        radii: [f64; 2],
        optics: WaterOptics,
        strength: f32,
    ) -> Result<(), &'static str> {
        let count = self.backscatter[3] as usize;
        if count >= MAX_WATER_AREAS {
            return Err("too many water areas");
        }
        if !centre
            .into_iter()
            .all(|v| v.is_finite() && (v as f32).is_finite())
            || !radii.into_iter().all(|v| v.is_finite() && v >= 1.0)
            || !optics.valid()
            || !strength.is_finite()
            || !(0.0..=1.0).contains(&strength)
        {
            return Err("invalid water area");
        }
        self.areas[count] = WaterArea {
            shape: [
                centre[0] as f32,
                centre[1] as f32,
                radii[0].recip() as f32,
                radii[1].recip() as f32,
            ],
            absorption: [
                optics.absorption[0],
                optics.absorption[1],
                optics.absorption[2],
                strength,
            ],
            backscatter: [
                optics.backscatter[0],
                optics.backscatter[1],
                optics.backscatter[2],
                0.0,
            ],
        };
        self.backscatter[3] += 1.0;
        Ok(())
    }

    pub fn enabled(&self) -> bool {
        self.absorption[3] > 0.0
    }

    /// CPU twin of sample_water, for diagnostics and continuity checks.
    pub fn sample(&self, point: [f32; 2], fallback: [f32; 3]) -> WaterOptics {
        if !self.enabled() {
            return WaterOptics {
                absorption: fallback,
                backscatter: LEGACY_BACKSCATTER,
            };
        }
        let mut out = WaterOptics {
            absorption: self.absorption[..3].try_into().unwrap(),
            backscatter: self.backscatter[..3].try_into().unwrap(),
        };
        for area in &self.areas[..self.backscatter[3] as usize] {
            let x = (point[0] - area.shape[0]) * area.shape[2];
            let z = (point[1] - area.shape[1]) * area.shape[3];
            let t = ((x * x + z * z - 0.15) / 0.85).clamp(0.0, 1.0);
            let share = (1.0 - t * t * (3.0 - 2.0 * t)) * area.absorption[3];
            out = out.mix(
                WaterOptics {
                    absorption: area.absorption[..3].try_into().unwrap(),
                    backscatter: area.backscatter[..3].try_into().unwrap(),
                },
                share,
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plume_has_clear_outer_water_and_a_continuous_edge() {
        let mut water = WaterField::new(WaterOptics::CLEAR);
        water
            .add_area([0.0, 0.0], [1000.0, 2000.0], WaterOptics::SEDIMENT, 1.0)
            .unwrap();
        assert_eq!(water.sample([0.0, 0.0], [1.0; 3]), WaterOptics::SEDIMENT);
        assert_eq!(water.sample([1001.0, 0.0], [1.0; 3]), WaterOptics::CLEAR);
        let edge = water.sample([999.0, 0.0], [1.0; 3]);
        for (a, b) in edge
            .absorption
            .into_iter()
            .zip(WaterOptics::CLEAR.absorption)
        {
            assert!((a - b).abs() < 1e-5);
        }
        assert!(water.sample([500.0, 0.0], [1.0; 3]).beam()[1] > WaterOptics::CLEAR.beam()[1]);
    }

    #[test]
    fn disabled_field_preserves_existing_map_coefficients() {
        let legacy = WaterField::default().sample([123.0, -789.0], [0.3, 0.1, 0.09]);
        assert_eq!(legacy.absorption, [0.3, 0.1, 0.09]);
        assert_eq!(legacy.backscatter, LEGACY_BACKSCATTER);
        assert_eq!(std::mem::size_of::<WaterField>(), 32 + 48 * MAX_WATER_AREAS);
    }

    #[test]
    fn invalid_areas_do_not_mutate_the_field() {
        let mut water = WaterField::new(WaterOptics::CLEAR);
        let before = water;
        assert!(
            water
                .add_area([f64::NAN, 0.0], [100.0; 2], WaterOptics::SEDIMENT, 1.0)
                .is_err()
        );
        assert!(
            water
                .add_area([0.0; 2], [0.0; 2], WaterOptics::SEDIMENT, 1.0)
                .is_err()
        );
        assert_eq!(water, before);
        for _ in 0..MAX_WATER_AREAS {
            water
                .add_area([0.0; 2], [100.0; 2], WaterOptics::SEDIMENT, 1.0)
                .unwrap();
        }
        assert!(
            water
                .add_area([0.0; 2], [100.0; 2], WaterOptics::SEDIMENT, 1.0)
                .is_err()
        );
    }
}
