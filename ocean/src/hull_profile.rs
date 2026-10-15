//! A hull's waterline profile, as the web game's `hullWaterlineProfile.ts` slices it from the drawn model: where the hull meets
//! horizontal planes around its waterline (half-breadths per level and station, each side) and how tall each side stands
//! above the water. The hull contact foam ([`crate::hull_foam`]) and the bow spray (naval-water-effects) read it.
//!
//! [`SimpleHull`] describes a proxy hull analytically (a fine entrance, a parallel middle body, a run aft to a transom,
//! a raked stem, sheer toward the bow) and [`SimpleHull::profile`] samples it into the same layout.

/// Stations along the hull, including one empty station beyond each end.
pub const PROFILE_STATIONS: usize = 32;
/// Horizontal slices through the hull, `PROFILE_LEVEL_STEP` metres apart, centred on the design waterline.
pub const PROFILE_LEVELS: usize = 7;
pub const PROFILE_LEVEL_STEP: f32 = 2.0;
pub const PROFILE_LOWEST: f32 = -PROFILE_LEVEL_STEP * (PROFILE_LEVELS as f32 - 1.0) / 2.0;

/// Where the hull meets horizontal planes around its waterline, in the hull's own frame (x to starboard, z aft; the bow at
/// the lowest z). `starboard` and `port` hold the outboard half-breadths per level and station, level-major; NaN where no
/// part of the hull crosses that plane (`WaterlineProfile` in the web).
#[derive(Clone, Debug, PartialEq)]
pub struct WaterlineProfile {
    /// Model z of the first station's centre and the spacing between stations.
    pub start: f32,
    pub spacing: f32,
    /// Model z of the stem and the sternmost crossing at the design waterline.
    pub bow: f32,
    pub stern: f32,
    /// Per level, the model z of the foremost point where the hull crosses it; NaN where the level misses the hull.
    pub stems: [f32; PROFILE_LEVELS],
    /// Widest half-breadth at any level.
    pub half_beam: f32,
    pub starboard: Vec<f32>,
    pub port: Vec<f32>,
    /// Height above the design waterline of the top of each side per station, 0 where the hull has no side.
    pub starboard_side: [f32; PROFILE_STATIONS],
    pub port_side: [f32; PROFILE_STATIONS],
}

impl WaterlineProfile {
    /// The tallest side, in metres above the design waterline.
    pub fn tallest_side(&self) -> f32 {
        self.starboard_side
            .iter()
            .chain(self.port_side.iter())
            .fold(0.0_f32, |a, &b| a.max(b))
    }
}

/// A proxy hull. Metres, the hull's own
/// frame (x starboard, y up from the design waterline, z aft, bow at `-length / 2`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimpleHull {
    /// Waterline length and beam.
    pub length: f32,
    pub beam: f32,
    /// Keel below the design waterline.
    pub draft: f32,
    /// Deck edge above the design waterline amidships; the bow rises `sheer` more.
    pub freeboard: f32,
    pub sheer: f32,
}

/// Share of the length the entrance takes from the stem to full beam (as `BowWaves` assumes: 0.3), and where the run aft
/// starts.
const ENTRANCE: f32 = 0.3;
const RUN: f32 = 0.78;
/// Half-breadth left at the transom, as a share of the half beam.
const TRANSOM: f32 = 0.45;
/// Stem rake: metres forward per metre above the waterline.
const RAKE: f32 = 0.35;

impl SimpleHull {
    /// A hull of the given waterline length and beam with proportions of a WWII warship (draft and freeboard scale with
    /// the beam and length).
    pub fn warship(length: f32, beam: f32, draft: f32) -> Self {
        Self {
            length,
            beam,
            draft,
            freeboard: 1.5 + 0.03 * length,
            sheer: 0.012 * length,
        }
    }

    /// The share `u` (0 at the stem, 1 at the stern) of the waterline a model z is at.
    fn along(&self, z: f32) -> f32 {
        (z + self.length / 2.0) / self.length
    }

    /// Model z of the stem at height `y` above the design waterline (raked forward above it, cut back below).
    pub fn stem(&self, y: f32) -> f32 {
        -self.length / 2.0 - RAKE * y.max(0.0) + 0.6 * (-y).max(0.0)
    }

    /// Height of the deck edge above the design waterline at model z.
    pub fn side_height(&self, z: f32) -> f32 {
        let u = self.along(z).clamp(0.0, 1.0);
        self.freeboard + self.sheer * (1.0 - u / 0.4).max(0.0).powi(2)
    }

    /// Half-breadth at model z and height y, or `None` where the hull does not reach (beyond its ends, below the keel,
    /// above the deck).
    pub fn half_breadth(&self, z: f32, y: f32) -> Option<f32> {
        if y < -self.draft || y > self.side_height(z) {
            return None;
        }
        let stem = self.stem(y);
        let stern = self.length / 2.0;
        if z < stem || z > stern {
            return None;
        }
        // The entrance measured from the stem at this height, so a raked stem keeps its fine lines.
        let span = stern - stem;
        let u = (z - stem) / span;
        let half = self.beam / 2.0;
        let entrance = if u < ENTRANCE {
            let t = u / ENTRANCE;
            1.0 - (1.0 - t) * (1.0 - t)
        } else {
            1.0
        };
        let run = if u > RUN {
            let t = (u - RUN) / (1.0 - RUN);
            1.0 - (1.0 - TRANSOM) * t * t
        } else {
            1.0
        };
        // The bilge turns in over the lowest third of the draft; the sides flare a little above the water.
        let depth = (-y / self.draft).clamp(0.0, 1.0);
        let bilge = if depth > 0.66 {
            let t = (depth - 0.66) / 0.34;
            (1.0 - t * t).max(0.0).sqrt().max(0.15)
        } else {
            1.0
        };
        let flare = 1.0 + 0.004 * y.max(0.0);
        Some(half * entrance * run * bilge * flare)
    }

    /// The waterline profile this hull slices into (`hullWaterlineProfile` sampled analytically: the same stations,
    /// levels, stems and side heights, with one empty station beyond each end).
    pub fn profile(&self) -> WaterlineProfile {
        let middle = (PROFILE_LEVELS - 1) / 2;
        let mut stems = [f32::NAN; PROFILE_LEVELS];
        let mut z_min = f32::INFINITY;
        let mut z_max = f32::NEG_INFINITY;
        for (level, stem) in stems.iter_mut().enumerate() {
            let y = PROFILE_LOWEST + level as f32 * PROFILE_LEVEL_STEP;
            if y < -self.draft || y > self.side_height(self.stem(y)) {
                continue;
            }
            *stem = self.stem(y);
            z_min = z_min.min(*stem);
            z_max = z_max.max(self.length / 2.0);
        }
        let spacing = (z_max - z_min) / (PROFILE_STATIONS as f32 - 3.0);
        let start = z_min - spacing;
        let size = PROFILE_LEVELS * PROFILE_STATIONS;
        let mut starboard = vec![f32::NAN; size];
        for level in 0..PROFILE_LEVELS {
            let y = PROFILE_LOWEST + level as f32 * PROFILE_LEVEL_STEP;
            for station in 0..PROFILE_STATIONS {
                let z = start + station as f32 * spacing;
                // A station just aft of the stem still records the stem's crossing, as the slice rounds it there.
                let z = if station > 0
                    && !stems[level].is_nan()
                    && (z - stems[level]).abs() < spacing * 0.5
                {
                    z.max(stems[level])
                } else {
                    z
                };
                if let Some(b) = self.half_breadth(z, y) {
                    starboard[level * PROFILE_STATIONS + station] = b;
                }
            }
        }
        let port = starboard.clone();
        let half_beam = starboard
            .iter()
            .filter(|b| !b.is_nan())
            .fold(0.0_f32, |a, &b| a.max(b));
        let mut sides = [0.0_f32; PROFILE_STATIONS];
        for (station, side) in sides.iter_mut().enumerate() {
            let crossed = (0..PROFILE_LEVELS)
                .any(|level| !starboard[level * PROFILE_STATIONS + station].is_nan());
            if crossed {
                *side = self.side_height(start + station as f32 * spacing);
            }
        }
        let (mut bow, mut stern) = (f32::INFINITY, f32::NEG_INFINITY);
        for station in 0..PROFILE_STATIONS {
            if !starboard[middle * PROFILE_STATIONS + station].is_nan() {
                let z = start + station as f32 * spacing;
                bow = bow.min(z);
                stern = stern.max(z);
            }
        }
        WaterlineProfile {
            start,
            spacing,
            bow: if bow.is_finite() { bow } else { z_min },
            stern: if stern.is_finite() { stern } else { z_max },
            stems,
            half_beam,
            starboard,
            port,
            starboard_side: sides,
            port_side: sides,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_hull_slices_like_a_model() {
        let hull = SimpleHull::warship(241.0, 36.0, 9.3);
        let profile = hull.profile();
        let middle = (PROFILE_LEVELS - 1) / 2;
        // One empty station beyond each end at the waterline.
        assert!(profile.starboard[middle * PROFILE_STATIONS].is_nan());
        assert!(profile.starboard[middle * PROFILE_STATIONS + PROFILE_STATIONS - 1].is_nan());
        // Full beam amidships, symmetric sides, a fine bow.
        let amidships = profile.starboard[middle * PROFILE_STATIONS + PROFILE_STATIONS / 2];
        assert!((amidships - 18.0).abs() < 0.5, "{amidships}");
        let bits = |values: &[f32]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&profile.starboard), bits(&profile.port));
        assert!((profile.half_beam - 18.0).abs() < 1.0);
        assert!((profile.stems[middle] + 120.5).abs() < 1e-3);
        // A raked stem stands farther forward above the water.
        assert!(profile.stems[middle + 2] < profile.stems[middle]);
        assert!(profile.bow < -110.0 && profile.stern > 110.0);
        assert!(profile.tallest_side() > hull.freeboard);
    }
}
