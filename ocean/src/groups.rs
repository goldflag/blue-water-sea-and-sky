//! Breaking groups, the CPU half of the web game's `ocean/waves/groups.ts`: which of a tile's breakers show where. A field of
//! wave groups laid on the world (value noise drifting downwind at the breaking waves' group velocity) decides it, so
//! each copy of a tile shows a different few; `shaders/field_common.wgsl` evaluates the field per pixel.

use std::f64::consts::PI;

use crate::spectrum::GRAVITY;

const GROUP_LEAST: f64 = 0.35;
const GROUP_MOST_SHARE: f64 = 0.8;
const GROUP_FILL: f64 = 0.3;
/// Half-width of a group's soft edge, in the noise's own units.
pub const GROUP_EDGE: f64 = 0.06;
/// The noise's quantiles: the level it passes over a share 0, 0.05, … 1 of the sea.
const QUANTILES: [f64; 21] = [
    0.9999, 0.8520, 0.7904, 0.7422, 0.7006, 0.6629, 0.6279, 0.5946, 0.5624, 0.5310, 0.5000, 0.4689,
    0.4375, 0.4054, 0.3720, 0.3369, 0.2994, 0.2577, 0.2094, 0.1479, 0.0002,
];
const GROUP_LENGTH: f64 = 2.5;
const GROUP_WIDTH: f64 = 4.0;
const GROUP_MOST: f64 = 0.3;

/// The share of the sea the groups cover when whitecaps cover `area` of it.
pub fn group_share(area: f64) -> f64 {
    let area = if area.is_nan() { 0.0 } else { area };
    (area / GROUP_FILL).clamp(GROUP_LEAST, GROUP_MOST_SHARE)
}

/// The noise level a group's edge lies at for groups covering `share` of the sea.
pub fn group_cut(share: f64) -> f64 {
    if share.is_nan() || share >= 1.0 {
        return -GROUP_EDGE;
    }
    let at = share.max(0.0) * (QUANTILES.len() - 1) as f64;
    let i = at.floor() as usize;
    QUANTILES[i] + (QUANTILES[i + 1] - QUANTILES[i]) * (at - i as f64)
}

/// A cascade's groups: cells (m) along and across the wind, and their downwind drift (m/s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroupLayout {
    pub length: f64,
    pub width: f64,
    pub drift: f64,
}

/// Group layout for breaking waves of `period` (s) in a tile `tile` metres wide.
pub fn group_layout(period: f64, tile: f64) -> GroupLayout {
    let wavelength = GRAVITY * period * period / (2.0 * PI);
    let most = GROUP_MOST * tile;
    if wavelength.is_nan() || wavelength <= 0.0 {
        return GroupLayout {
            length: most,
            width: most,
            drift: 0.0,
        };
    }
    GroupLayout {
        length: most.min(GROUP_LENGTH * wavelength),
        width: most.min(GROUP_WIDTH * wavelength),
        drift: GRAVITY * period / (4.0 * PI),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_interpolates_the_quantile_table() {
        assert_eq!(group_cut(0.5), 0.5);
        assert!((group_cut(0.35) - 0.5946).abs() < 1e-12);
        assert_eq!(group_cut(1.0), -GROUP_EDGE);
    }
}
