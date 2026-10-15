//! A hull's waterline profile sliced from its drawn triangles: a line-for-line port of
//! the web game's `hullWaterlineProfile.ts` (`hullWaterlineProfile`), which slices every opaque surface of a drawn hull at the
//! profile levels. Visual only: the contact foam line, the sea's shelter and the bow spray follow the hull the player
//! sees. The triangles are in the hull's own frame (metres, x to starboard, y up from the design waterline, z aft).

use glam::Vec3;

use crate::hull_profile::{
    PROFILE_LEVEL_STEP, PROFILE_LEVELS, PROFILE_LOWEST, PROFILE_STATIONS, WaterlineProfile,
};

/// Bins along the hull and across each side in which the height of the side is gathered before the stations are known.
const SIDE_BINS: usize = 128;
const SIDE_BANDS: usize = 16;
/// Each level is sliced this far over its height (m): a hull modelled with an edge loop exactly on a level, as the
/// design waterline often is, has no triangle crossing the level itself, only ones standing on it and hanging from
/// it, and the slice came back empty there (Yamato's waterline held 7 of 30 stations: no contact foam along her side).
const LEVEL_NUDGE: f32 = 1e-3;
/// Anything standing within this share of a station's half-breadth of the side counts as the side: the sheer strake, a
/// bulwark, the deck edge's fittings. Superstructure and turrets stand farther inboard.
const SIDE_SHARE: f32 = 0.8;

/// The profile of the hull `triangles` draw, or `None` when no triangle crosses a profile level.
pub fn profile_from_triangles(triangles: &[[Vec3; 3]]) -> Option<WaterlineProfile> {
    // The model's extent in its own frame lays out the bins the sides' heights gather in.
    let (mut low, mut high) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for corner in triangles.iter().flatten() {
        low = low.min(*corner);
        high = high.max(*corner);
    }
    if !low.is_finite() || !high.is_finite() {
        return None;
    }
    let bin_low = low.z;
    let bin_size = (high.z - low.z).max(1e-3) / SIDE_BINS as f32;
    let band_size = high.x.max(-low.x).max(1e-3) / SIDE_BANDS as f32;
    // Highest point above the waterline per side, bin along the hull and band out from the centreline.
    let mut tops = [
        vec![f32::NEG_INFINITY; SIDE_BINS * SIDE_BANDS],
        vec![f32::NEG_INFINITY; SIDE_BINS * SIDE_BANDS],
    ];
    let bin_of = |z: f32| (((z - bin_low) / bin_size).floor().max(0.0) as usize).min(SIDE_BINS - 1);
    let mut raise = |x: f32, y: f32, bin: usize| {
        let top = &mut tops[if x >= 0.0 { 0 } else { 1 }];
        let cell = bin * SIDE_BANDS + ((x.abs() / band_size).floor() as usize).min(SIDE_BANDS - 1);
        if y > top[cell] {
            top[cell] = y;
        }
    };
    // Crossing segments per level: x0, z0, x1, z1.
    let mut segments: Vec<Vec<[f32; 4]>> = vec![Vec::new(); PROFILE_LEVELS];
    for corner in triangles {
        let low = corner[0].y.min(corner[1].y).min(corner[2].y);
        let high = corner[0].y.max(corner[1].y).max(corner[2].y);
        // A triangle's highest point within a bin lies on its corners or where its edges cross the bin's ends: a long
        // strake of two triangles has no corner in the bins between its ends.
        if high > 0.0 {
            for k in 0..3 {
                let (a, b) = (corner[k], corner[(k + 1) % 3]);
                if a.y > 0.0 {
                    raise(a.x, a.y, bin_of(a.z));
                }
                let (first, last) = (bin_of(a.z.min(b.z)), bin_of(a.z.max(b.z)));
                for bin in first + 1..=last {
                    let t = (bin_low + bin as f32 * bin_size - a.z) / (b.z - a.z);
                    let y = a.y + (b.y - a.y) * t;
                    if y > 0.0 {
                        let x = a.x + (b.x - a.x) * t;
                        raise(x, y, bin - 1);
                        raise(x, y, bin);
                    }
                }
            }
        }
        if high < PROFILE_LOWEST || low > -PROFILE_LOWEST {
            continue;
        }
        for (level, list) in segments.iter_mut().enumerate() {
            let y = PROFILE_LOWEST + level as f32 * PROFILE_LEVEL_STEP + LEVEL_NUDGE;
            if !(low < y && high > y) {
                continue;
            }
            let mut points = [0.0f32; 4];
            let mut count = 0;
            for k in 0..3 {
                let (a, b) = (corner[k], corner[(k + 1) % 3]);
                if (a.y < y) == (b.y < y) {
                    continue;
                }
                let t = (y - a.y) / (b.y - a.y);
                if count < 4 {
                    points[count] = a.x + (b.x - a.x) * t;
                    points[count + 1] = a.z + (b.z - a.z) * t;
                }
                count += 2;
            }
            if count == 4 {
                list.push(points);
            }
        }
    }
    let middle = (PROFILE_LEVELS - 1) / 2;
    let (mut z_min, mut z_max) = (f32::INFINITY, f32::NEG_INFINITY);
    let mut stems = [f32::NAN; PROFILE_LEVELS];
    for (level, list) in segments.iter().enumerate() {
        for s in list {
            for z in [s[1], s[3]] {
                z_min = z_min.min(z);
                z_max = z_max.max(z);
                if stems[level].is_nan() || z < stems[level] {
                    stems[level] = z;
                }
            }
        }
    }
    if z_max.partial_cmp(&z_min) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    // One empty station beyond each end keeps the clamped texture free of foam past the hull.
    let spacing = (z_max - z_min) / (PROFILE_STATIONS as f32 - 3.0);
    let start = z_min - spacing;
    let size = PROFILE_LEVELS * PROFILE_STATIONS;
    let mut starboard = vec![f32::NAN; size];
    let mut port = vec![f32::NAN; size];
    let mut record = |level: usize, station: i64, x: f32| {
        if station < 0 || station >= PROFILE_STATIONS as i64 {
            return;
        }
        let i = level * PROFILE_STATIONS + station as usize;
        if starboard[i].is_nan() || starboard[i] < x {
            starboard[i] = x;
        }
        if port[i].is_nan() || port[i] < -x {
            port[i] = -x;
        }
    };
    for (level, list) in segments.iter().enumerate() {
        for &[x0, z0, x1, z1] in list {
            record(level, ((z0 - start) / spacing).round() as i64, x0);
            record(level, ((z1 - start) / spacing).round() as i64, x1);
            let first = ((z0.min(z1) - start) / spacing).ceil() as i64;
            let last = ((z0.max(z1) - start) / spacing).floor() as i64;
            for station in first..=last {
                let z = start + station as f32 * spacing;
                let run = if z1 - z0 == 0.0 { 1.0 } else { z1 - z0 };
                record(level, station, x0 + (x1 - x0) * (z - z0) / run);
            }
        }
    }
    let mut half_beam = 0.0f32;
    for i in 0..size {
        half_beam = half_beam
            .max(nan_to_zero(starboard[i]))
            .max(nan_to_zero(port[i]));
    }
    let (mut bow, mut stern) = (f32::INFINITY, f32::NEG_INFINITY);
    for station in 0..PROFILE_STATIONS {
        if !starboard[middle * PROFILE_STATIONS + station].is_nan() {
            let z = start + station as f32 * spacing;
            bow = bow.min(z);
            stern = stern.max(z);
        }
    }
    let sides = |top: &[f32], breadths: &[f32]| -> [f32; PROFILE_STATIONS] {
        side_heights(top, breadths, start, spacing, bin_low, bin_size, band_size)
    };
    Some(WaterlineProfile {
        start,
        spacing,
        bow: if bow.is_finite() { bow } else { z_min },
        stern: if stern.is_finite() { stern } else { z_max },
        stems,
        half_beam,
        starboard_side: sides(&tops[0], &starboard),
        port_side: sides(&tops[1], &port),
        starboard,
        port,
    })
}

/// `x || 0` for a half-breadth: NaN (no crossing) counts as nothing.
fn nan_to_zero(x: f32) -> f32 {
    if x.is_nan() { 0.0 } else { x }
}

/// Per station, the highest point standing at one side: within `SIDE_SHARE` of the station's widest half-breadth, over
/// the bins the station spans. Never below the highest level the side itself crosses; a median of three stations drops
/// a lone bridge wing or boom, which would otherwise stand in for a whole stretch of side.
fn side_heights(
    top: &[f32],
    breadths: &[f32],
    start: f32,
    spacing: f32,
    bin_low: f32,
    bin_size: f32,
    band_size: f32,
) -> [f32; PROFILE_STATIONS] {
    let mut heights = [0.0f32; PROFILE_STATIONS];
    for (station, height_out) in heights.iter_mut().enumerate() {
        let mut breadth = f32::NEG_INFINITY;
        for level in 0..PROFILE_LEVELS {
            let b = breadths[level * PROFILE_STATIONS + station];
            breadth = breadth.max(if b.is_nan() || b == 0.0 {
                f32::NEG_INFINITY
            } else {
                b
            });
        }
        if breadth.is_nan() || breadth <= 0.0 {
            continue;
        }
        let mut crossed = 0.0f32;
        for level in 0..PROFILE_LEVELS {
            if breadths[level * PROFILE_STATIONS + station] >= breadth * SIDE_SHARE {
                crossed = crossed.max(PROFILE_LOWEST + level as f32 * PROFILE_LEVEL_STEP);
            }
        }
        let z = start + station as f32 * spacing;
        let first = ((z - spacing / 2.0 - bin_low) / bin_size).floor().max(0.0) as usize;
        let last = (((z + spacing / 2.0 - bin_low) / bin_size).floor().max(0.0) as usize)
            .min(SIDE_BINS - 1);
        let band = ((breadth * SIDE_SHARE / band_size).floor() as usize).min(SIDE_BANDS - 1);
        let mut height = crossed;
        for bin in first..=last {
            for b in band..SIDE_BANDS {
                height = height.max(top[bin * SIDE_BANDS + b]);
            }
        }
        *height_out = height;
    }
    let mut median = heights;
    for station in 1..PROFILE_STATIONS - 1 {
        let (a, b, c) = (heights[station - 1], heights[station], heights[station + 1]);
        median[station] = a.min(b).max(a.max(b).min(c));
    }
    median
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hull_profile::SimpleHull;

    /// A box hull, 100 m long, 20 m wide, 6 m deep and 8 m of freeboard, as two triangles a face.
    fn box_hull() -> Vec<[Vec3; 3]> {
        let (l, b, d, f) = (50.0, 10.0, -6.0, 8.0);
        let c = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        let quad = |a: Vec3, b: Vec3, cc: Vec3, dd: Vec3| [[a, b, cc], [a, cc, dd]];
        let mut t = Vec::new();
        t.extend(quad(c(b, d, -l), c(b, d, l), c(b, f, l), c(b, f, -l)));
        t.extend(quad(c(-b, d, -l), c(-b, f, -l), c(-b, f, l), c(-b, d, l)));
        t.extend(quad(c(-b, d, -l), c(b, d, -l), c(b, f, -l), c(-b, f, -l)));
        t.extend(quad(c(-b, d, l), c(-b, f, l), c(b, f, l), c(b, d, l)));
        t
    }

    #[test]
    fn a_box_slices_to_its_breadth_ends_and_freeboard() {
        let profile = profile_from_triangles(&box_hull()).expect("a profile");
        let middle = (PROFILE_LEVELS - 1) / 2;
        let amidships = profile.starboard[middle * PROFILE_STATIONS + PROFILE_STATIONS / 2];
        assert!((amidships - 10.0).abs() < 1e-3, "{amidships}");
        assert!(
            (profile.port[middle * PROFILE_STATIONS + PROFILE_STATIONS / 2] - 10.0).abs() < 1e-3
        );
        assert!((profile.half_beam - 10.0).abs() < 1e-3);
        assert!((profile.stems[middle] + 50.0).abs() < 1e-3);
        // One empty station beyond each end.
        assert!(profile.starboard[middle * PROFILE_STATIONS].is_nan());
        assert!(profile.starboard[middle * PROFILE_STATIONS + PROFILE_STATIONS - 1].is_nan());
        assert!((profile.bow + 50.0).abs() < 4.0 && (profile.stern - 50.0).abs() < 4.0);
        let side = profile.starboard_side[PROFILE_STATIONS / 2];
        assert!((side - 8.0).abs() < 1e-3, "{side}");
    }

    #[test]
    fn an_edge_loop_on_the_waterline_still_slices() {
        // The same box with its sides split along the design waterline: no triangle crosses y = 0.
        let (l, b, d, f) = (50.0, 10.0, -6.0, 8.0);
        let c = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        let quad = |a: Vec3, b: Vec3, cc: Vec3, dd: Vec3| [[a, b, cc], [a, cc, dd]];
        let mut t = Vec::new();
        for (low, high) in [(d, 0.0), (0.0, f)] {
            t.extend(quad(
                c(b, low, -l),
                c(b, low, l),
                c(b, high, l),
                c(b, high, -l),
            ));
            t.extend(quad(
                c(-b, low, -l),
                c(-b, high, -l),
                c(-b, high, l),
                c(-b, low, l),
            ));
            t.extend(quad(
                c(-b, low, -l),
                c(b, low, -l),
                c(b, high, -l),
                c(-b, high, -l),
            ));
            t.extend(quad(
                c(-b, low, l),
                c(-b, high, l),
                c(b, high, l),
                c(b, low, l),
            ));
        }
        let profile = profile_from_triangles(&t).expect("a profile");
        let middle = (PROFILE_LEVELS - 1) / 2;
        for station in 1..PROFILE_STATIONS - 1 {
            let i = middle * PROFILE_STATIONS + station;
            assert!(
                (profile.starboard[i] - 10.0).abs() < 1e-3,
                "{station}: {}",
                profile.starboard[i]
            );
            assert!(
                (profile.port[i] - 10.0).abs() < 1e-3,
                "{station}: {}",
                profile.port[i]
            );
        }
        assert!((profile.stems[middle] + 50.0).abs() < 1e-3);
    }

    #[test]
    fn nothing_at_the_waterline_slices_to_none() {
        let high = [[
            Vec3::new(0.0, 20.0, 0.0),
            Vec3::new(1.0, 21.0, 0.0),
            Vec3::new(0.0, 22.0, 1.0),
        ]];
        assert!(profile_from_triangles(&high).is_none());
        assert!(profile_from_triangles(&[]).is_none());
        // The proxy hull's analytic profile is the same layout.
        assert_eq!(
            SimpleHull::warship(100.0, 20.0, 6.0)
                .profile()
                .starboard
                .len(),
            PROFILE_LEVELS * PROFILE_STATIONS
        );
    }
}
