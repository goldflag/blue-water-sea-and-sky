//! The star catalogue and the grids the dome finds its stars in, and the stars' colour ramp.
//!
//! The stars are the real ones: the Yale Bright Star Catalogue (`data/bright_stars.tsv`, written by `data/build.ts`
//! from CDS V/50), every star the eye can see and a few hundred past it, at their J2000 places. Until 2026-10-12 the
//! sky drew a seeded random field (the web game's `sky/celestial/stars.ts`); the grid, its warp and the colour ramp
//! (`starField.ts`) are still that port's.
//!
//! The dome finds a star by its cell: each cube face of the galactic sphere is cut into `STAR_GRID_CELLS` squared
//! cells, and a cell holds at most one star, whose drawn disc (`STAR_GRID_MARGIN` across its radius) must lie inside
//! it. One grid cannot hold a star near a cell's edge, so there are four, each shifted half a cell along one, both or
//! neither axis: a star then fits whichever leaves it clear, at its true place. A star whose disc crosses a face's edge
//! is written on both faces, each drawing its own part. Stars too close to have a cell each (the Pleiades, Mizar and
//! Alcor) share one: a cell is `STAR_LAYERS` texels deep, each star in it flagging the next.

use crate::half::f16_bits;
use crate::state::EQUATORIAL_TO_GALACTIC;

/// The catalogue (`data/build.ts`): HR number, right ascension and declination (degrees, J2000), V magnitude, B−V.
const BRIGHT_STARS: &str = include_str!("../data/bright_stars.tsv");
pub const COLOR_INDEX_RANGE: [f64; 2] = [-0.3, 2.0];

/// Cells per cube-face edge of the star grid.
pub const STAR_GRID_CELLS: u32 = 80;
/// Least clearance (radians) of a star from its cell's edges: the radius it is drawn within.
pub const STAR_GRID_MARGIN: f64 = 0.19 * std::f64::consts::PI / 180.0;
pub const EMPTY_MAGNITUDE: f64 = 40.0;
/// Each grid's shift along a face's two axes, in cells.
pub const STAR_GRIDS: [[f64; 2]; 4] = [[0.0, 0.0], [0.5, 0.5], [0.5, 0.0], [0.0, 0.5]];
/// Stars a cell holds: the first in the grid's own rows, each further one a layer down.
pub const STAR_LAYERS: u32 = 4;
/// Added to the B−V of a star whose cell holds another a layer down.
pub const STAR_MORE: f64 = 10.0;
/// The packed grids: six faces side by side, layer `l` of grid `g` in rows `(4 l + g) * (cells + 1)` on.
pub const STAR_TEXTURE_WIDTH: u32 = 6 * (STAR_GRID_CELLS + 1);
pub const STAR_TEXTURE_HEIGHT: u32 = STAR_LAYERS * STAR_GRIDS.len() as u32 * (STAR_GRID_CELLS + 1);
pub const WARP: [f64; 2] = [0.31157, 0.08442];
/// Colours sampled across B−V.
pub const RAMP_SIZE: u32 = 64;
const COLOR_SATURATION: f64 = 1.25;

/// A star's cell: its grid and face, the cell, its layer in it, and the star's place in the cell (in cells from the
/// cell's corner; outside 0–1 for a star written from the neighbouring face).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StarSlot {
    pub layer: u32,
    pub grid: u32,
    pub face: u32,
    pub i: u32,
    pub j: u32,
    pub a: f64,
    pub b: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Star {
    /// Harvard Revised (Bright Star) number.
    pub hr: u32,
    /// Its direction in the galactic frame.
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub magnitude: f64,
    pub color_index: f64,
    /// Its cell on its own face, then on each face its disc reaches.
    pub slots: Vec<StarSlot>,
}

/// Effective temperature (K) of a star of colour index B−V.
pub fn color_temperature(color_index: f64) -> f64 {
    4600.0 * (1.0 / (0.92 * color_index + 1.7) + 1.0 / (0.92 * color_index + 0.62))
}

/// Cube face (0–5: ±x, ±y, ±z) of a direction and its gnomonic coordinates on that face.
pub fn cube_face(x: f64, y: f64, z: f64) -> (u32, f64, f64) {
    let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
    if ax >= ay && ax >= az {
        return (if x >= 0.0 { 0 } else { 1 }, y / ax, z / ax);
    }
    if ay >= az {
        return (if y >= 0.0 { 2 } else { 3 }, z / ay, x / ay);
    }
    (if z >= 0.0 { 4 } else { 5 }, x / az, y / az)
}

/// A direction's gnomonic coordinates on `face` (beyond ±1 off the face), and the radians it lies outside that face
/// (0 on it); none for a direction behind the face.
fn on_face(face: u32, x: f64, y: f64, z: f64) -> Option<(f64, f64, f64)> {
    let sign = if face.is_multiple_of(2) { 1.0 } else { -1.0 };
    let (major, u, v) = match face / 2 {
        0 => (x * sign, y, z),
        1 => (y * sign, z, x),
        _ => (z * sign, x, y),
    };
    if major <= 0.0 {
        return None;
    }
    // The face ends where another axis overtakes its own: the planes `major = ±u` and `major = ±v`.
    let outside = ((u.abs().max(v.abs()) - major) / std::f64::consts::SQRT_2)
        .clamp(0.0, 1.0)
        .asin();
    Some((u / major, v / major, outside))
}

pub fn warp(u: f64) -> f64 {
    let a = u.abs();
    u * (1.0 + (1.0 - a) * (WARP[0] + WARP[1] * a))
}

pub fn warp_slope(u: f64) -> f64 {
    let a = u.abs();
    1.0 + WARP[0] + 2.0 * (WARP[1] - WARP[0]) * a - 3.0 * WARP[1] * a * a
}

fn across(u: f64, v: f64) -> (f64, f64) {
    let c = 1.0 + u * u + v * v;
    (
        1.0 / (warp_slope(u) * (c * (1.0 + u * u)).sqrt()),
        1.0 / (warp_slope(v) * (c * (1.0 + v * v)).sqrt()),
    )
}

/// The cell of `grid` on `face` holding the gnomonic point `(u, v)`, with the point's clearance (radians) from the
/// cell's edges inside the face. A face's own edge is no edge here: the neighbouring face draws what crosses it.
fn face_cell(grid: u32, face: u32, u: f64, v: f64) -> (StarSlot, f64) {
    let n = STAR_GRID_CELLS as f64;
    let shift = STAR_GRIDS[grid as usize];
    let (scale_s, scale_t) = across(u.clamp(-1.0, 1.0), v.clamp(-1.0, 1.0));
    let cell = 2.0 / n;
    // One axis: the cell, the place in it, and the clearance in cells (a shifted grid has a half cell at each end).
    let axis = |w: f64, shift: f64| {
        let last = n - 1.0 + 2.0 * shift;
        let g = (warp(w) + 1.0) * n / 2.0 + shift;
        let k = g.floor().clamp(0.0, last);
        let a = g - k;
        let below = if k > 0.0 { a } else { f64::INFINITY };
        let above = if k < last { 1.0 - a } else { f64::INFINITY };
        (k, a, below.min(above))
    };
    let (i, a, clear_s) = axis(u, shift[0]);
    let (j, b, clear_t) = axis(v, shift[1]);
    (
        StarSlot {
            layer: 0,
            grid,
            face,
            i: i as u32,
            j: j as u32,
            a,
            b,
        },
        (clear_s * cell * scale_s).min(clear_t * cell * scale_t),
    )
}

/// The cell of `grid` holding a direction on its own face, with the direction's clearance (radians) from the cell's
/// edges inside the face.
pub fn grid_cell(grid: u32, x: f64, y: f64, z: f64) -> (StarSlot, f64) {
    let (face, u, v) = cube_face(x, y, z);
    face_cell(grid, face, u, v)
}

pub fn slot_texel(slot: &StarSlot) -> usize {
    let row = STAR_GRID_CELLS + 1;
    let grid = slot.layer * STAR_GRIDS.len() as u32 + slot.grid;
    (slot.face * row + slot.i + (grid * row + slot.j) * STAR_TEXTURE_WIDTH) as usize
}

/// The direction a slot's star is drawn in: the inverse of [`grid_cell`], as the dome reads the grid.
pub fn slot_direction(slot: &StarSlot) -> [f64; 3] {
    let n = STAR_GRID_CELLS as f64;
    let shift = STAR_GRIDS[slot.grid as usize];
    // The warp has no closed inverse: Newton's steps from the unwarped place.
    let unwarp = |g: f64| {
        let target = g * 2.0 / n - 1.0;
        let mut w = target;
        for _ in 0..8 {
            w -= (warp(w) - target) / warp_slope(w);
        }
        w
    };
    let u = unwarp(slot.i as f64 + slot.a - shift[0]);
    let v = unwarp(slot.j as f64 + slot.b - shift[1]);
    let sign = if slot.face.is_multiple_of(2) {
        1.0
    } else {
        -1.0
    };
    let length = (1.0 + u * u + v * v).sqrt();
    let (major, u, v) = (sign / length, u / length, v / length);
    match slot.face / 2 {
        0 => [major, u, v],
        1 => [v, major, u],
        _ => [u, v, major],
    }
}

/// The catalogue's rows: (HR, galactic direction, magnitude, B−V), brightest first.
fn rows() -> impl Iterator<Item = (u32, [f64; 3], f64, f64)> {
    BRIGHT_STARS
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| {
            let mut fields = line.split('\t');
            let mut next = || fields.next().expect("a star's five columns");
            let hr = next().parse().expect("an HR number");
            let mut number = || next().parse::<f64>().expect("a star's number");
            let (ra, dec) = (number().to_radians(), number().to_radians());
            let (magnitude, color_index) = (number(), number());
            let equatorial = [dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin()];
            let galactic = EQUATORIAL_TO_GALACTIC.map(|row| {
                row[0] * equatorial[0] + row[1] * equatorial[1] + row[2] * equatorial[2]
            });
            (hr, galactic, magnitude, color_index)
        })
}

/// The catalogue, brightest first, every star in its cells. A star with no cell left (a cluster crowded past
/// `STAR_LAYERS` deep: none of this catalogue's) is left out.
pub fn star_catalog() -> Vec<Star> {
    let mut occupied = vec![false; (STAR_TEXTURE_WIDTH * STAR_TEXTURE_HEIGHT) as usize];
    let mut stars = Vec::new();
    for (hr, [x, y, z], magnitude, color_index) in rows() {
        let (own, ..) = cube_face(x, y, z);
        // Its own face first, then each face its disc reaches.
        let faces = std::iter::once(own).chain((0..6).filter(|&face| face != own));
        let mut slots = Vec::new();
        let mut placed = true;
        for face in faces {
            let Some((u, v, outside)) = on_face(face, x, y, z) else {
                continue;
            };
            if outside >= STAR_GRID_MARGIN {
                continue;
            }
            // The shallowest free layer of each grid that leaves it clear; of those the shallowest, then the clearest.
            let best = (0..STAR_GRIDS.len() as u32)
                .map(|grid| face_cell(grid, face, u, v))
                .filter(|(_, clearance)| *clearance >= STAR_GRID_MARGIN)
                .filter_map(|(slot, clearance)| {
                    (0..STAR_LAYERS)
                        .map(|layer| StarSlot { layer, ..slot })
                        .find(|slot| !occupied[slot_texel(slot)])
                        .map(|slot| (slot, clearance))
                })
                .min_by(|a, b| a.0.layer.cmp(&b.0.layer).then(b.1.total_cmp(&a.1)));
            match best {
                Some((slot, _)) => slots.push(slot),
                None => placed = false,
            }
        }
        if !placed || slots.is_empty() {
            continue;
        }
        for slot in &slots {
            occupied[slot_texel(slot)] = true;
        }
        stars.push(Star {
            hr,
            x,
            y,
            z,
            magnitude,
            color_index: color_index.clamp(COLOR_INDEX_RANGE[0], COLOR_INDEX_RANGE[1]),
            slots,
        });
    }
    stars
}

/// The grid texture's RGBA half floats for the brightest `count` stars: (a, b) inside the cell,
/// magnitude, B−V (`starTexels`), the B−V raised by [`STAR_MORE`] where the cell's next layer holds a star too.
pub fn star_texels(stars: &[Star], count: usize) -> Vec<u16> {
    let texels = (STAR_TEXTURE_WIDTH * STAR_TEXTURE_HEIGHT) as usize;
    let mut target = vec![0u16; texels * 4];
    let empty = f16_bits(EMPTY_MAGNITUDE as f32);
    for texel in 0..texels {
        target[texel * 4 + 2] = empty;
    }
    let slots = || {
        stars
            .iter()
            .take(count)
            .flat_map(|star| star.slots.iter().map(move |slot| (star, slot)))
    };
    // A star a layer down flags the one above it, which is brighter: placed first, so among the `count` if it is.
    let mut more = vec![false; texels];
    for (_, slot) in slots().filter(|(_, slot)| slot.layer > 0) {
        more[slot_texel(&StarSlot {
            layer: slot.layer - 1,
            ..*slot
        })] = true;
    }
    for (star, slot) in slots() {
        let texel = slot_texel(slot);
        let color = star.color_index + if more[texel] { STAR_MORE } else { 0.0 };
        target[texel * 4] = f16_bits(slot.a as f32);
        target[texel * 4 + 1] = f16_bits(slot.b as f32);
        target[texel * 4 + 2] = f16_bits(star.magnitude as f32);
        target[texel * 4 + 3] = f16_bits(color as f32);
    }
    target
}

fn matching(nm: f64) -> [f64; 3] {
    let lobe = |mean: f64, low: f64, high: f64| {
        let t = (nm - mean) / if nm < mean { low } else { high };
        (-0.5 * t * t).exp()
    };
    [
        1.056 * lobe(599.8, 37.9, 31.0) + 0.362 * lobe(442.0, 16.0, 26.7)
            - 0.065 * lobe(501.1, 20.4, 26.2),
        0.821 * lobe(568.8, 46.9, 40.5) + 0.286 * lobe(530.9, 16.3, 31.1),
        1.217 * lobe(437.0, 11.8, 36.0) + 0.681 * lobe(459.0, 26.0, 13.8),
    ]
}

/// Linear sRGB of a blackbody at `kelvin`, scaled to unit luminance and clipped to the gamut.
pub fn blackbody_color(kelvin: f64) -> [f64; 3] {
    let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);
    let mut nm: f64 = 380.0;
    while nm <= 780.0 {
        let metres = nm * 1e-9;
        let radiance = 1.0 / (metres.powi(5) * ((1.4388e-2 / (metres * kelvin)).exp() - 1.0));
        let [xb, yb, zb] = matching(nm);
        x += radiance * xb;
        y += radiance * yb;
        z += radiance * zb;
        nm += 5.0;
    }
    let rgb = [
        (3.2406 * x - 1.5372 * y - 0.4986 * z).max(0.0),
        (-0.9689 * x + 1.8758 * y + 0.0415 * z).max(0.0),
        (0.0557 * x - 0.204 * y + 1.057 * z).max(0.0),
    ];
    let luminance = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
    rgb.map(|channel| channel / luminance)
}

/// The stars' colour ramp across B−V, RGBA half floats (`StarField`'s ramp).
pub fn star_ramp() -> Vec<u16> {
    let mut colors = vec![0u16; RAMP_SIZE as usize * 4];
    for i in 0..RAMP_SIZE as usize {
        let index = COLOR_INDEX_RANGE[0]
            + (COLOR_INDEX_RANGE[1] - COLOR_INDEX_RANGE[0]) * i as f64 / (RAMP_SIZE - 1) as f64;
        let rgb = blackbody_color(color_temperature(index));
        let luminance = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
        for c in 0..3 {
            colors[i * 4 + c] =
                f16_bits((luminance + (rgb[c] - luminance) * COLOR_SATURATION).max(0.0) as f32);
        }
        colors[i * 4 + 3] = f16_bits(1.0);
    }
    colors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn star(stars: &[Star], hr: u32) -> &Star {
        stars
            .iter()
            .find(|star| star.hr == hr)
            .unwrap_or_else(|| panic!("HR {hr} is in the sky"))
    }

    fn angle(a: [f64; 3], b: [f64; 3]) -> f64 {
        let cross = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2])
            .sqrt()
            .atan2(dot)
    }

    #[test]
    fn the_catalogue_is_the_real_sky_brightest_first() {
        let stars = star_catalog();
        assert!(
            stars
                .windows(2)
                .all(|w| w[0].magnitude <= w[1].magnitude + 1e-9)
        );
        // Sirius, then Canopus, then Alpha Centauri's two as one.
        assert_eq!(
            stars[..3].iter().map(|s| s.hr).collect::<Vec<_>>(),
            [2491, 2326, 5459]
        );
        assert!((stars[0].magnitude + 1.46).abs() < 1e-9);
        assert!((stars[2].magnitude + 0.29).abs() < 0.02, "{:?}", stars[2]);
        // Betelgeuse is red and Rigel blue-white; Orion's belt is three stars in a line 2.7° long.
        assert!(star(&stars, 2061).color_index > 1.5);
        assert!(star(&stars, 1713).color_index < 0.1);
        let at = |hr| {
            let s = star(&stars, hr);
            [s.x, s.y, s.z]
        };
        let (mintaka, alnilam, alnitak) = (at(1852), at(1903), at(1948));
        let belt = angle(mintaka, alnitak).to_degrees();
        assert!((belt - 2.74).abs() < 0.03, "{belt}");
        let bend = angle(mintaka, alnilam) + angle(alnilam, alnitak) - angle(mintaka, alnitak);
        assert!(bend.to_degrees() < 0.03, "{}", bend.to_degrees());
        // The galactic centre lies in Sagittarius, beside the Archer's arrowhead (Gamma Sagittarii).
        assert!(angle(at(6746), [1.0, 0.0, 0.0]).to_degrees() < 6.0);
    }

    /// Every star of the catalogue has a cell, the crowded ones a layer or more down.
    #[test]
    fn every_star_has_a_cell() {
        let stars = star_catalog();
        let placed: std::collections::HashSet<u32> = stars.iter().map(|s| s.hr).collect();
        let missing: Vec<_> = rows()
            .filter(|(hr, ..)| !placed.contains(hr))
            .map(|(hr, _, magnitude, _)| (hr, magnitude))
            .collect();
        assert!(missing.is_empty(), "{missing:?}");
        assert_eq!(stars.len(), 8952);
        let deep = |layer| stars.iter().filter(|s| s.slots[0].layer >= layer).count();
        assert!(deep(1) > 100 && deep(STAR_LAYERS - 1) < 10);
        // The Pleiades' nine and the Horse and Rider are all there.
        for hr in [
            1165, 1178, 1142, 1149, 1156, 1145, 1180, 1140, 1151, 5054, 5062,
        ] {
            assert!(placed.contains(&hr), "HR {hr}");
        }
    }

    /// The dome draws each star where the catalogue has it: every slot, read back as the shader reads it, is the star's
    /// direction, its disc clear of its cell's edges, and no two stars share a cell.
    #[test]
    fn every_slot_draws_its_star_in_place() {
        let stars = star_catalog();
        let mut seen = std::collections::HashSet::new();
        let mut copies = 0;
        for s in &stars {
            copies += s.slots.len() - 1;
            assert_eq!(s.slots[0].face, cube_face(s.x, s.y, s.z).0);
            for slot in &s.slots {
                assert!(seen.insert(slot_texel(slot)), "{s:?}");
                let error = angle(slot_direction(slot), [s.x, s.y, s.z]);
                assert!(error < 1e-9, "HR {}: {error} rad off in {slot:?}", s.hr);
                let (u, v, _) = on_face(slot.face, s.x, s.y, s.z).unwrap();
                let (again, clearance) = face_cell(slot.grid, slot.face, u, v);
                assert_eq!(
                    StarSlot {
                        layer: slot.layer,
                        ..again
                    },
                    *slot
                );
                assert!(clearance >= STAR_GRID_MARGIN);
            }
        }
        // About one star in a hundred lies across a face's edge.
        assert!((20..300).contains(&copies), "{copies}");
    }

    #[test]
    fn the_grid_texture_holds_the_brightest_stars() {
        let stars = star_catalog();
        let texels = star_texels(&stars, 100);
        assert_eq!(
            texels.len(),
            (STAR_TEXTURE_WIDTH * STAR_TEXTURE_HEIGHT * 4) as usize
        );
        let empty = f16_bits(EMPTY_MAGNITUDE as f32);
        let filled = texels.chunks(4).filter(|t| t[2] != empty).count();
        let slots: usize = stars[..100].iter().map(|s| s.slots.len()).sum();
        assert_eq!(filled, slots);
        // Every star a layer down is flagged by the star above it, and nothing else is flagged.
        let texels = star_texels(&stars, usize::MAX);
        let flag = f16_bits(STAR_MORE as f32 + COLOR_INDEX_RANGE[0] as f32 - 0.5);
        let flagged =
            |texel: usize| texels[texel * 4 + 3] >= flag && texels[texel * 4 + 3] < 0x8000;
        let mut below = 0;
        for slot in stars.iter().flat_map(|s| &s.slots) {
            let down = StarSlot {
                layer: slot.layer + 1,
                ..*slot
            };
            let has_below =
                slot.layer + 1 < STAR_LAYERS && texels[slot_texel(&down) * 4 + 2] != empty;
            assert_eq!(flagged(slot_texel(slot)), has_below, "{slot:?}");
            below += usize::from(has_below);
        }
        assert!(below > 0);
    }
}
