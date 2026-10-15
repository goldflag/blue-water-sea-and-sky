//! Camera-centred geometry clipmap plus a flat horizon ring, in one draw: ported from
//! the web game's `ocean/surface/OceanGeometry.ts`.
//!
//! Every level is a square of `segments` cells; each is twice the extent and cell size of the one inside it, and the
//! outermost reaches [`CLIPMAP_REACH`] from the centre on every tier. A level is drawn as a ring around the one inside
//! it, and the finest level drawn fills its own middle. Every level snaps its centre to twice its own cell, so vertices
//! sit on fixed world lattice points and never swim; each level geomorphs its odd vertices onto the coarser lattice
//! toward its outer edge (in `surface.wgsl`), so neighbouring levels share exact edge vertices. The horizon ring
//! continues from the outer lattice to 95% of the far plane in circles 3.5 km apart (close enough for the Earth's
//! curve, applied per vertex), and a skirt beyond it reaches the sea's horizon from an eye high enough to see past the
//! ring; the waves fade out every wave its spacing cannot carry.
//!
//! Which level is the finest drawn follows how far the view stands from the water ([`Clipmap::detail`]): a level
//! narrower than that distance holds cells smaller than the lens resolves, so a periscope draws them all (cells of a
//! quarter of a metre on High) and a view from a kilometre up starts five levels out. The finest level drawn morphs
//! onto the next before it is dropped, so the change never shows.
//!
//! A magnified view from well above the sea takes a second mesh in the clipmap's place ([`fan`]): a polar lattice
//! about the eye over the wedge of sea the lens shows, its cells as many pixels across at every range, square near
//! the eye and no shorter on screen than a share of a pixel beyond.

use bytemuck::{Pod, Zeroable};

/// Half the edge of the outermost level (m), where the horizon ring takes over; each level inside it is half the
/// one around it.
pub const CLIPMAP_REACH: f64 = 4096.0;
/// The most levels a tier's clipmap has (`Surface.frames` in `surface.wgsl`).
pub const MAX_CLIPMAP_LEVELS: usize = 10;
/// The share of the way to dropping the finest level drawn from which it morphs onto the next.
const DETAIL_MORPH_FROM: f64 = 0.5;
/// Share of the far plane the flat horizon ring reaches.
pub(crate) const HORIZON_REACH: f64 = 0.95;
/// Metres between the ring's circles. The sea is drawn curving over the Earth vertex by vertex
/// (`naval::camera::curved`), so the water between two circles is a chord under the curve: 3.5 km apart it sags a
/// quarter of a metre at most (Δ² / 8 R), where the doubling squares the ring had before sagged 30 m between 16 and
/// 57 km, a hull's height under a ship standing there.
const RING_STEP: f64 = 3_500.0;
/// As far out as the circles keep that spacing: the far plane of a battle's views of the sea.
const RING_CURVED: f64 = 60_000.0;
/// Beyond it, out to the far plane of a theatre view that draws the land beyond its window (450 km,
/// `naval_render::camera::HORIZON_FAR`), the circles stand [`RING_FAR_STEP`] apart: the water between two sags 4.4 m
/// under the curve at most, where doubling circles sagged 280 m at 180 km and the far land's sea floor stood above it.
/// A chart's ring (flat, to 950 km) doubles beyond.
const RING_FAR: f64 = 480_000.0;
const RING_FAR_STEP: f64 = 15_000.0;
/// Rows of the skirt beyond the ring, which `surface.wgsl` (its `SKIRT_ROWS`) spreads from the ring to the sea's
/// horizon when an eye stands high enough to see past the ring.
pub const SKIRT_ROWS: u32 = 6;

/// One vertex: lattice position (x, z) and (level, horizon spacing).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct OceanVertex {
    pub position: [f32; 2],
    pub level: [f32; 2],
}

impl OceanVertex {
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<OceanVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
    };
}

/// The mesh, built once per far plane.
pub struct ClipmapMesh {
    pub vertices: Vec<OceanVertex>,
    /// Every level's middle (the cells the level inside it covers), then every level's ring, then the horizon ring
    /// and its skirt.
    pub indices: Vec<u32>,
    /// Per level, where its middle's indices lie: drawn for the finest level drawn alone.
    pub fills: Vec<std::ops::Range<u32>>,
    /// Per level, where its ring's indices start: a draw from there to the end takes every coarser ring and the
    /// horizon.
    pub rings: Vec<u32>,
    /// Radius of the horizon ring (m).
    pub radius: f64,
}

/// A tier's clipmap: `segments` cells per level edge (a power of two, at least 8) and `levels` levels, the outermost
/// reaching [`CLIPMAP_REACH`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Clipmap {
    pub segments: u32,
    pub levels: usize,
}

/// The finest level drawn and how far it has morphed onto the next ([`Clipmap::detail`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Detail {
    pub first: usize,
    pub morph: f32,
}

impl Clipmap {
    pub const fn new(segments: u32, levels: usize) -> Self {
        assert!(segments >= 8 && segments.is_power_of_two());
        assert!(levels >= 1 && levels <= MAX_CLIPMAP_LEVELS);
        Self { segments, levels }
    }

    /// Half the edge of `level` in metres.
    pub fn half(&self, level: usize) -> f64 {
        CLIPMAP_REACH / f64::from(1u32 << (self.levels - 1 - level))
    }

    /// Cell size of `level` in metres: an exact power of two.
    pub fn cell(&self, level: usize) -> f64 {
        self.half(level) * 2.0 / f64::from(self.segments)
    }

    /// Each level's frame for a centre at (x, z): snapped centre, cell and its reciprocal.
    pub fn frames(&self, x: f64, z: f64) -> [[f32; 4]; MAX_CLIPMAP_LEVELS] {
        std::array::from_fn(|level| {
            let cell = self.cell(level.min(self.levels - 1));
            let step = cell * 2.0;
            // JavaScript's Math.round (halves up).
            let snap = |v: f64| (v / step + 0.5).floor() * step;
            [
                snap(x) as f32,
                snap(z) as f32,
                cell as f32,
                (1.0 / cell) as f32,
            ]
        })
    }

    /// The finest level worth drawing for a view standing `standoff` metres from the water at the clipmap's centre
    /// ([`standoff`]): the first whose half edge reaches that far. Cells grow with distance from the centre at two to
    /// four to a level's `segments`-th of it, so a level narrower than the standoff holds cells finer than every
    /// level beyond it shows on screen. Over the second half of the way to the next level the finest morphs onto it
    /// (`surface.wgsl`), so the level dropped has already become the middle of the one that takes over.
    pub fn detail(&self, standoff: f64) -> Detail {
        let level = (standoff.max(1e-6) / self.half(0))
            .log2()
            .min((self.levels - 1) as f64);
        let first = level.ceil().max(0.0);
        let share = ((level - (first - 1.0) - DETAIL_MORPH_FROM) / (1.0 - DETAIL_MORPH_FROM))
            .clamp(0.0, 1.0);
        Detail {
            first: first as usize,
            morph: (share * share * (3.0 - 2.0 * share)) as f32,
        }
    }
}

/// Magnifications (against the normal 52° lens) over which the clipmap leaves the eye for what the view is aimed at.
const FOCUS_FROM: f64 = 1.25;
const FOCUS_BY: f64 = 2.0;
/// The farthest the clipmap's centre goes from the eye (m).
pub const FOCUS_REACH: f64 = 12_000.0;
/// A hull counts as aimed at while it lies within this share of the view's half height of the view's centre.
const FOCUS_CONE: f64 = 1.2;
/// Heights of the eye above the highest the surface reaches (m) over which the centre may leave it: an eye near the
/// water (a periscope, a low free camera) keeps the finest cells, so the water it sees close by is the sea the
/// underwater probe reads.
const CLEAR_FROM: f64 = 3.0;
const CLEAR_BY: f64 = 10.0;

/// The view the clipmap centres for: the eye, where it looks, the lens, and the hulls (x, z) the sea knows of.
pub struct FocusView<'a> {
    pub eye: [f64; 3],
    pub forward: [f64; 3],
    pub tan_half_fov_y: f64,
    pub hulls: &'a [(f64, f64)],
    /// The highest the drawn surface can reach (m), [`crate::Ocean::surface_bound`].
    pub surface: f64,
}

/// How far the view stands from the water at the clipmap's `centre`, in metres at the normal lens: the eye's
/// distance from the sea there, less the `swell` the surface may rise (or, for an eye under it, fall) toward it, over
/// the lens's magnification. Binoculars bring far water as near as that.
pub fn standoff(view: &FocusView<'_>, centre: [f64; 2], swell: f64) -> f64 {
    let [ex, ey, ez] = view.eye;
    let clear = (ey.abs() - swell.max(0.0)).max(0.0);
    let distance = (centre[0] - ex).hypot(centre[1] - ez).hypot(clear);
    let normal = (f64::from(naval_render::camera::DEFAULT_FOV_Y_DEGREES).to_radians() / 2.0).tan();
    distance * view.tan_half_fov_y.max(1e-6) / normal
}

/// Where the clipmap centres (x, z). The levels coarsen with distance from their centre, which matches what a pixel
/// covers at the normal lens when the centre is the eye; a magnified view shows far water as large as near water, so
/// through binoculars the centre moves to what the view is aimed at: the hull nearest the middle of the view, or else
/// where the view meets the sea, at most [`FOCUS_REACH`] off. The water between the eye and the focus, at the bottom
/// of a narrow view, takes the coarser cells.
pub fn focus(view: &FocusView<'_>) -> [f64; 2] {
    let [ex, ey, ez] = view.eye;
    let normal = (f64::from(naval_render::camera::DEFAULT_FOV_Y_DEGREES).to_radians() / 2.0).tan();
    let magnification = normal / view.tan_half_fov_y.max(1e-6);
    let smooth = |from: f64, by: f64, x: f64| {
        let t = ((x - from) / (by - from)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let clear = ey - view.surface.max(0.0);
    let share = smooth(FOCUS_FROM, FOCUS_BY, magnification) * smooth(CLEAR_FROM, CLEAR_BY, clear);
    if share <= 0.0 {
        return [ex, ez];
    }
    let [fx, fy, fz] = view.forward;
    let length = (fx * fx + fy * fy + fz * fz).sqrt().max(1e-9);
    let (fx, fy, fz) = (fx / length, fy / length, fz / length);
    // The hull nearest the middle of the view, measured at her waterline.
    let cone = (view.tan_half_fov_y * FOCUS_CONE).atan();
    let aimed = view
        .hulls
        .iter()
        .filter_map(|&(x, z)| {
            let (dx, dy, dz) = (x - ex, -ey, z - ez);
            let range = (dx * dx + dy * dy + dz * dz).sqrt();
            let angle = ((dx * fx + dy * fy + dz * fz) / range.max(1e-9))
                .clamp(-1.0, 1.0)
                .acos();
            (angle < cone && dx.hypot(dz) < FOCUS_REACH).then_some((angle, x, z))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let (x, z) = match aimed {
        Some((_, x, z)) => (x, z),
        None => {
            let across = fx.hypot(fz);
            if across < 1e-6 {
                return [ex, ez];
            }
            let reach = if fy < -1e-6 {
                (ey * across / -fy).min(FOCUS_REACH)
            } else {
                FOCUS_REACH
            };
            (ex + fx / across * reach, ez + fz / across * reach)
        }
    };
    [ex + (x - ex) * share, ez + (z - ez) * share]
}

/// Magnification from which a view standing clear of the sea takes the fan ([`fan`]) in place of the clipmap: where
/// [`focus`] would start to leave the eye.
pub const FAN_FROM: f64 = FOCUS_FROM;
/// Rows of cells in the fan's block ([`build_fan`], `FAN_BLOCK_ROWS` in `surface.wgsl`): the draw stacks one instance
/// of it outward per block.
pub const FAN_BLOCK_ROWS: u32 = 32;
/// Columns of cells the fan's block holds, and the most blocks a draw stacks.
pub const FAN_COLUMNS: u32 = 512;
const FAN_BLOCKS: u32 = 256;
/// The fan's cell at the normal lens over the least a clipmap cell is of its distance from the centre (two over the
/// cells a level edge holds; a level's cells are one to two of it), so a magnified view's cells cover the pixels the
/// normal view's do.
const FAN_CELL: f64 = 1.25;
/// Steps of the fan's cell to a doubling: the lens's magnification and the cells' budget move it a step at a time, so
/// the lattice holds still while a view turns or tilts.
const FAN_NOTCHES: f64 = 8.0;
/// The most cells a fan draws, over the cells of the tier's whole lattice.
const FAN_BUDGET: f64 = 2.0;
/// The least of a view's forward that lies along the sea for the fan: a view looking more steeply down sees water all
/// round the point below the eye, which a wedge cannot hold, and keeps the clipmap.
const FAN_LEVEL: f64 = 0.5;
/// The steepest any line of sight in the frame may look down for the fan, as a tangent (70 degrees): nearer the point
/// under the eye the wedge opens toward a full turn.
const FAN_STEEP: f64 = 2.75;
/// The least a row of the fan stands on screen, in pixels of the drawing's height over the clipmap's least cell (as
/// [`FAN_CELL`]): a pixel and a half on High, three on Medium. The sea seen at a grazing angle is foreshortened, a
/// square cell 10 km off through an 8x lens a twelfth of a pixel tall, and triangles thinner than a pixel cost their
/// pixels several times over (each shades the whole of every two-by-two block it touches): square to the horizon the
/// fan added 3.6 ms to a High frame at 8x, and with rows a pixel and a half tall 1.5 ms, the frame then costing what
/// the normal lens's does (`native/docs/ocean.md`, "A magnified view draws a fan"). Where a square cell would be
/// shorter the rows stand this far apart instead, and carry the waves their spacing can.
const FAN_ROW_PIXELS: f64 = 96.0;
/// How much nearer and farther than the lens's edges meet the sea the fan reaches.
const FAN_MARGIN: f64 = 0.03;
/// Samples along each edge of the frame the fan is fitted to.
const FAN_EDGE_SAMPLES: usize = 32;

/// The view the fan is fitted to: the camera's eye, axes and lens, how high the drawn surface can reach
/// ([`crate::Ocean::surface_bound`]), the Earth's curve (`naval_render::camera::Camera::curvature`, 0 for a flat sea)
/// and the far plane.
pub struct FanView {
    pub eye: [f64; 3],
    pub forward: [f64; 3],
    pub right: [f64; 3],
    pub up: [f64; 3],
    pub tan_half_fov_y: f64,
    pub aspect: f64,
    /// The drawing's height in pixels.
    pub pixels: f64,
    pub surface: f64,
    pub curvature: f64,
    pub far: f64,
}

/// A magnified view's water mesh ([`fan`]): a polar lattice about the eye. Columns stand `step` radians apart from
/// `bearing`. Rows are counted from the `knee`: within it each is e^`step` times the range of the one before, so a
/// cell is square and `step` times its range across; beyond it each row's line of sight dips `squeeze` times the
/// eye's height less than the one before (one over a row's range falls by `squeeze` a row), so every row stands as
/// tall on screen as the square cell at the knee does. The draw's first row is row `first`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fan {
    pub step: f64,
    /// The first column's bearing (radians from +z toward +x).
    pub bearing: f64,
    pub columns: u32,
    /// The range where rows turn from square cells to rows a set height on screen (m).
    pub knee: f64,
    /// What one over a row's range falls by each row beyond the knee (1 / m): `step` over the knee.
    pub squeeze: f64,
    /// The draw's first row, counted from the knee (negative within it).
    pub first: f64,
    /// Rows of cells; none when the lens shows no sea.
    pub rows: u32,
    /// As far as the fan reaches (m): its last row stands here, however far its place in the lattice lies beyond.
    pub far: f64,
}

impl Fan {
    /// Blocks of [`FAN_BLOCK_ROWS`] rows the draw stacks.
    pub fn blocks(&self) -> u32 {
        self.rows.div_ceil(FAN_BLOCK_ROWS)
    }

    /// The range from the eye of the draw's row `row` (m), as `surface.wgsl`'s vertex stage places it.
    pub fn range(&self, row: u32) -> f64 {
        let row = self.first + f64::from(row);
        let range = if row <= 0.0 {
            self.knee * (row * self.step).exp()
        } else {
            // A row past where a level sea would end has no range of its own.
            1.0 / (1.0 / self.knee - self.squeeze * row).max(1e-9)
        };
        range.min(self.far)
    }

    /// A range's row, counted from the knee (the inverse of [`Fan::range`], unrounded).
    fn row(&self, range: f64) -> f64 {
        if range <= self.knee {
            (range / self.knee).ln() / self.step
        } else {
            (1.0 / self.knee - 1.0 / range) / self.squeeze
        }
    }

    /// The vertex spacing the waves are read at `range` metres from the eye (m): the longer of a cell's sides.
    pub fn spacing(&self, range: f64) -> f64 {
        range * self.step.max(self.squeeze * range)
    }
}

/// The mesh a magnified view draws the sea with, or none for a view that keeps the clipmap.
///
/// The clipmap is fine about one point, and a lens high over the sea shows a strip of it kilometres long: at 8x from
/// a foretop, from 600 m off to the horizon. The fan covers that strip alone, the wedge of sea between the frame's
/// edges from where its lowest line of sight meets the water to where its highest does (the sea's horizon and a crest
/// beyond it, for a view of the sky), with cells [`FAN_CELL`] of the clipmap's least at the normal lens, over the
/// magnification; past the range where such a cell stands less than [`FAN_ROW_PIXELS`] of a pixel tall, rows that
/// height apart, which carry the waves no shorter than they are deep. Its columns stand on fixed bearings and its
/// rows on fixed ranges from the eye, so turning or tilting the view adds and drops cells at the frame's edges and
/// moves none; the lattice moves with the eye, and changes a notch at a time when the lens does, when the eye's
/// height does, or when the cells in view would pass the tier's budget.
///
/// An eye within [`CLEAR_BY`] of the highest the surface reaches keeps the clipmap (the water at the lens is the sea
/// the underwater probe reads), as does a view looking steeply down ([`FAN_LEVEL`], [`FAN_STEEP`]) or wider than a
/// half turn.
pub fn fan(view: &FanView, clipmap: Clipmap) -> Option<Fan> {
    if view.tan_half_fov_y <= 0.0 {
        return None;
    }
    let normal = (f64::from(naval_render::camera::DEFAULT_FOV_Y_DEGREES).to_radians() / 2.0).tan();
    let magnification = normal / view.tan_half_fov_y;
    let bound = view.surface.max(0.0);
    let height = view.eye[1];
    if magnification < FAN_FROM || height - bound < CLEAR_BY {
        return None;
    }
    let [fx, fy, fz] = view.forward;
    let length = (fx * fx + fy * fy + fz * fz).sqrt().max(1e-9);
    if fx.hypot(fz) / length < FAN_LEVEL {
        return None;
    }
    let heading = fx.atan2(fz);
    // The frame's edges: the steepest and shallowest lines of sight (the tangent of each one's depression) and the
    // bearings they span.
    let (wide, tall) = (view.tan_half_fov_y * view.aspect, view.tan_half_fov_y);
    let (mut steepest, mut shallowest) = (f64::NEG_INFINITY, f64::INFINITY);
    let (mut left, mut right) = (f64::INFINITY, f64::NEG_INFINITY);
    for k in 0..4 * FAN_EDGE_SAMPLES {
        let along = (k % FAN_EDGE_SAMPLES) as f64 / FAN_EDGE_SAMPLES as f64 * 2.0 - 1.0;
        let (sx, sy) = match k / FAN_EDGE_SAMPLES {
            0 => (along, -1.0),
            1 => (1.0, along),
            2 => (-along, 1.0),
            _ => (-1.0, -along),
        };
        let line: [f64; 3] = std::array::from_fn(|axis| {
            view.forward[axis] / length + view.right[axis] * sx * wide + view.up[axis] * sy * tall
        });
        let across = line[0].hypot(line[2]);
        if across < 1e-6 {
            return None;
        }
        let turn = (line[0].atan2(line[2]) - heading + std::f64::consts::PI)
            .rem_euclid(std::f64::consts::TAU)
            - std::f64::consts::PI;
        if turn.abs() > std::f64::consts::FRAC_PI_2 {
            return None;
        }
        let slope = -line[1] / across;
        (steepest, shallowest) = (steepest.max(slope), shallowest.min(slope));
        (left, right) = (left.min(turn), right.max(turn));
    }
    if steepest > FAN_STEEP {
        return None;
    }
    // As far as the sea is drawn: its horizon and a crest that still shows beyond it, or the far plane's share of a
    // flat sea.
    let reach = if view.curvature > 0.0 {
        (height / view.curvature).sqrt() + ((bound + 1.0) / view.curvature).sqrt()
    } else {
        view.far * HORIZON_REACH
    };
    // The waves move the water sideways as far as they raise it, about.
    let shift = 2.0 * bound;
    // Nearest, the steepest line meets the highest the surface reaches; farthest, the shallowest meets the lowest it
    // falls, which the Earth's curve drops further, or passes over the horizon.
    let near = if steepest > 0.0 {
        ((height - bound) / steepest * (1.0 - FAN_MARGIN) - shift).max(1.0)
    } else {
        reach
    };
    let drop = shallowest * shallowest - 4.0 * view.curvature * (height + bound);
    let far = if shallowest > 0.0 && drop > 0.0 {
        (2.0 * (height + bound) / (shallowest + drop.sqrt()) * (1.0 + FAN_MARGIN) + shift)
            .min(reach)
    } else {
        reach
    };
    let pad = shift / near;
    let (left, right) = (heading + left - pad, heading + right + pad);
    let growth = clipmap.cell(0) / clipmap.half(0);
    let budget = FAN_BUDGET * clipmap.levels as f64 * f64::from(clipmap.segments).powi(2) * 0.75;
    // The lens and the eye's height to the nearest notch, so neither's drift moves the lattice.
    let notched = |v: f64| ((v.log2() * FAN_NOTCHES).round() / FAN_NOTCHES).exp2();
    let zoom = (magnification.log2() * FAN_NOTCHES).round();
    // A row's line of sight dips this much less than the one before, as a tangent: the pixels' share of the lens.
    let lens = normal / (zoom / FAN_NOTCHES).exp2();
    let dip = FAN_ROW_PIXELS * growth * 2.0 * lens / view.pixels.max(1.0);
    let squeeze = dip / notched(height);
    let mut notch = -zoom;
    loop {
        // A whole number of columns to the turn, so the bearings' lattice has no seam where the heading wraps.
        let step = FAN_CELL * growth * (notch / FAN_NOTCHES).exp2();
        let step = std::f64::consts::TAU / (std::f64::consts::TAU / step).round().max(4.0);
        let mut fan = Fan {
            step,
            bearing: (left / step).floor() * step,
            columns: 0,
            knee: step / squeeze,
            squeeze,
            first: 0.0,
            rows: 0,
            far,
        };
        let columns = (right / step).ceil() - (left / step).floor();
        fan.first = fan.row(near).floor();
        let rows = if far > near {
            fan.row(far).ceil() - fan.first
        } else {
            0.0
        };
        if columns <= f64::from(FAN_COLUMNS)
            && rows <= f64::from(FAN_BLOCKS * FAN_BLOCK_ROWS)
            && columns * rows <= budget
        {
            (fan.columns, fan.rows) = (columns as u32, rows as u32);
            return Some(fan);
        }
        notch += 1.0;
    }
}

/// The fan's block: [`FAN_COLUMNS`] columns of [`FAN_BLOCK_ROWS`] cells, a vertex's position its (column, row) and its
/// level -1, which `surface.wgsl`'s vertex stage places by [`Fan`]. The indices run column by column, so a draw of
/// the first `columns * FAN_BLOCK_ROWS * 6` takes the columns a view needs.
pub struct FanMesh {
    pub vertices: Vec<OceanVertex>,
    pub indices: Vec<u32>,
}

pub fn build_fan() -> FanMesh {
    let rows = FAN_BLOCK_ROWS + 1;
    let vertices = (0..=FAN_COLUMNS)
        .flat_map(|column| {
            (0..rows).map(move |row| OceanVertex {
                position: [column as f32, row as f32],
                level: [-1.0, 0.0],
            })
        })
        .collect();
    // Bearings turn from +z toward +x and rows run outward: counter-clockwise seen from above, as the clipmap's.
    let indices = (0..FAN_COLUMNS)
        .flat_map(|column| {
            (0..FAN_BLOCK_ROWS).flat_map(move |row| {
                let a = column * rows + row;
                let (b, c, d) = (a + rows, a + rows + 1, a + 1);
                [a, c, b, a, d, c]
            })
        })
        .collect();
    FanMesh { vertices, indices }
}

/// Build the clipmap and horizon ring for a far plane of `far` metres.
pub fn build(clipmap: Clipmap, far: f64) -> ClipmapMesh {
    let Clipmap { segments, levels } = clipmap;
    assert!(
        segments >= 8 && segments.is_power_of_two(),
        "clipmap segments must be a power of two, at least 8, not {segments}"
    );
    let mut vertices: Vec<OceanVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let half = (segments / 2) as i32;
    let hole = (segments / 4) as i32;
    let vertex = |vertices: &mut Vec<OceanVertex>, x: f64, z: f64, level: f64, spacing: f64| {
        vertices.push(OceanVertex {
            position: [x as f32, z as f32],
            level: [level as f32, spacing as f32],
        });
        (vertices.len() - 1) as u32
    };
    // Counter-clockwise seen from above, so the front face looks up.
    let triangle = |vertices: &[OceanVertex], indices: &mut Vec<u32>, a: u32, b: u32, c: u32| {
        let [ax, az] = vertices[a as usize].position;
        let [bx, bz] = vertices[b as usize].position;
        let [cx, cz] = vertices[c as usize].position;
        let cross = (bx - ax) * (cz - az) - (bz - az) * (cx - ax);
        if cross > 0.0 {
            indices.extend([a, c, b]);
        } else {
            indices.extend([a, b, c]);
        }
    };
    let firsts: Vec<u32> = (0..levels)
        .map(|level| {
            let size = clipmap.cell(level);
            let first = vertices.len() as u32;
            for j in -half..=half {
                for i in -half..=half {
                    vertex(
                        &mut vertices,
                        f64::from(i) * size,
                        f64::from(j) * size,
                        level as f64,
                        0.0,
                    );
                }
            }
            first
        })
        .collect();
    // A level's cells: its middle (the cells the level inside it covers) or the ring around it.
    let cells = |vertices: &[OceanVertex], indices: &mut Vec<u32>, level: usize, middle: bool| {
        let first = firsts[level];
        let at = |i: i32, j: i32| first + ((j + half) * (segments as i32 + 1) + i + half) as u32;
        for j in -half..half {
            for i in -half..half {
                if (i >= -hole && i < hole && j >= -hole && j < hole) != middle {
                    continue;
                }
                // One diagonal direction everywhere: fully morphed cells collapse onto the coarser triangles.
                triangle(vertices, indices, at(i, j), at(i, j + 1), at(i + 1, j + 1));
                triangle(vertices, indices, at(i, j), at(i + 1, j + 1), at(i + 1, j));
            }
        }
    };
    let fills: Vec<std::ops::Range<u32>> = (0..levels)
        .map(|level| {
            let start = indices.len() as u32;
            cells(&vertices, &mut indices, level, true);
            start..indices.len() as u32
        })
        .collect();
    let rings: Vec<u32> = (0..levels)
        .map(|level| {
            let start = indices.len() as u32;
            cells(&vertices, &mut indices, level, false);
            start
        })
        .collect();
    // Horizon rows: the outer lattice (every other outermost vertex), then circles out to the ring's radius.
    let outer = clipmap.cell(levels - 1);
    let edge = f64::from(half) * outer;
    let step = 2.0 * outer;
    let per_side = half as usize;
    let count = (segments * 2) as usize;
    let square: Vec<(f64, f64)> = (0..count)
        .map(|k| {
            let side = k / per_side;
            let t = (k % per_side) as f64 * step - edge;
            match side {
                0 => (t, -edge),
                1 => (edge, t),
                2 => (-t, edge),
                _ => (-edge, -t),
            }
        })
        .collect();
    // The circle stays outside the last square's corners however small the far plane.
    let radius = (far * HORIZON_REACH).max(edge * 3.0);
    // The first circle clears the lattice's corners; the rest stand [`RING_STEP`] apart as far as a battle's view of
    // the sea reaches ([`RING_CURVED`]), [`RING_FAR_STEP`] apart to a theatre view's far land ([`RING_FAR`]), and
    // double beyond it (a chart's far plane, drawn flat).
    let mut radii = vec![edge * std::f64::consts::SQRT_2 * 1.05];
    loop {
        let last = *radii.last().unwrap();
        let next = if last < RING_CURVED {
            last + RING_STEP
        } else if last < RING_FAR {
            last + RING_FAR_STEP
        } else {
            last * 2.0
        };
        if next >= radius - RING_STEP * 0.5 {
            break;
        }
        radii.push(next);
    }
    radii.push(radius);
    let horizon = levels as f64;
    let mut previous: Vec<u32> = square
        .iter()
        .map(|&(x, z)| vertex(&mut vertices, x, z, horizon, step))
        .collect();
    let mut join = |vertices: &[OceanVertex], previous: &[u32], current: &[u32]| {
        for k in 0..count {
            let n = (k + 1) % count;
            triangle(vertices, &mut indices, previous[k], current[k], current[n]);
            triangle(vertices, &mut indices, previous[k], current[n], previous[n]);
        }
    };
    for &r in &radii {
        let current: Vec<u32> = square
            .iter()
            .map(|&(x, z)| {
                let scale = r / x.hypot(z);
                vertex(
                    &mut vertices,
                    x * scale,
                    z * scale,
                    horizon,
                    step * r / edge,
                )
            })
            .collect();
        join(&vertices, &previous, &current);
        previous = current;
    }
    // The skirt: rows that stand on the ring's circle in the mesh, which the vertex stage carries out to the sea's
    // horizon when it lies beyond the ring (`surface.wgsl`: an eye high enough sees past the far plane).
    for row in 1..=SKIRT_ROWS {
        let current: Vec<u32> = square
            .iter()
            .map(|&(x, z)| {
                let scale = radius / x.hypot(z);
                vertex(
                    &mut vertices,
                    x * scale,
                    z * scale,
                    horizon + f64::from(row),
                    step * radius / edge,
                )
            })
            .collect();
        join(&vertices, &previous, &current);
        previous = current;
    }
    ClipmapMesh {
        vertices,
        indices,
        fills,
        rings,
        radius,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clipmap the web's High tier drew: 4 m cells out to 128 m, six levels.
    const WEB_HIGH: Clipmap = Clipmap::new(64, 6);

    #[test]
    fn a_mesh_has_every_level_s_middle_and_ring_and_the_horizon() {
        let mesh = build(WEB_HIGH, 60000.0);
        let level_vertices = WEB_HIGH.levels * 65 * 65;
        assert!(mesh.vertices.len() > level_vertices);
        assert!((mesh.radius - 57000.0).abs() < 1e-9);
        // Every level's middle (the quarter the level inside it covers), then every level's ring.
        let (middle, ring) = (32 * 32 * 2 * 3, (64 * 64 - 32 * 32) * 2 * 3);
        for level in 0..WEB_HIGH.levels {
            assert_eq!(
                mesh.fills[level],
                (level * middle) as u32..((level + 1) * middle) as u32
            );
            assert_eq!(
                mesh.rings[level],
                (WEB_HIGH.levels * middle + level * ring) as u32
            );
        }
        assert!(mesh.indices.len() > WEB_HIGH.levels * (middle + ring));
        assert!(
            mesh.indices
                .iter()
                .all(|&i| (i as usize) < mesh.vertices.len())
        );
    }

    #[test]
    fn every_tier_reaches_the_same_lattice_edge() {
        for clipmap in [
            Clipmap::new(32, 6),
            WEB_HIGH,
            Clipmap::new(128, 9),
            Clipmap::new(256, 9),
        ] {
            assert_eq!(clipmap.half(clipmap.levels - 1), CLIPMAP_REACH);
            let mesh = build(clipmap, 60000.0);
            let edge = mesh.vertices[..clipmap.levels * (clipmap.segments as usize + 1).pow(2)]
                .iter()
                .map(|v| f64::from(v.position[0].abs().max(v.position[1].abs())))
                .fold(0.0, f64::max);
            assert_eq!(edge, CLIPMAP_REACH);
        }
        let high = Clipmap::new(128, 9);
        assert_eq!(
            (high.half(0), high.cell(0), high.cell(8)),
            (16.0, 0.25, 64.0)
        );
    }

    #[test]
    fn the_finest_level_drawn_follows_the_standoff() {
        let high = Clipmap::new(128, 9);
        // A periscope: every level. A bridge 25 m up: from the level 32 m wide. A kilometre up: from the 1,024 m one.
        assert_eq!(
            high.detail(1.0),
            Detail {
                first: 0,
                morph: 0.0
            }
        );
        assert_eq!(high.detail(25.0).first, 1);
        assert_eq!(high.detail(1000.0).first, 6);
        // From tens of kilometres up the outermost level alone, morphed onto the horizon ring's spacing.
        assert_eq!(
            high.detail(40_000.0),
            Detail {
                first: 8,
                morph: 1.0
            }
        );
        // A level has morphed onto the next by the standoff that drops it, and the next starts unmorphed.
        let before = high.detail(64.0 - 1e-9);
        let after = high.detail(64.0 + 1e-9);
        assert_eq!((before.first, after.first), (2, 3));
        assert!(before.morph > 0.999 && after.morph == 0.0);
        // No morph over the first half of a level's span, rising monotonically over the second.
        assert_eq!(high.detail(44.0).morph, 0.0);
        let morphs: Vec<f32> = (0..=20)
            .map(|i| high.detail(46.0 + f64::from(i) * 0.9).morph)
            .collect();
        assert!(morphs.windows(2).all(|w| w[1] >= w[0]) && morphs[20] > 0.99);
    }

    #[test]
    fn the_standoff_is_the_eye_s_distance_at_the_normal_lens() {
        let view = |eye: [f64; 3], m: f64| FocusView {
            eye,
            forward: [0.0, 0.0, -1.0],
            tan_half_fov_y: lens(m),
            hulls: &[],
            surface: 0.0,
        };
        // Centred on the eye: its height above the swell, above or below the water.
        assert!((standoff(&view([5.0, 30.0, 7.0], 1.0), [5.0, 7.0], 2.0) - 28.0).abs() < 1e-9);
        assert!((standoff(&view([5.0, -30.0, 7.0], 1.0), [5.0, 7.0], 2.0) - 28.0).abs() < 1e-9);
        assert_eq!(standoff(&view([0.0, 1.2, 0.0], 1.0), [0.0, 0.0], 2.0), 0.0);
        // Binoculars on a hull 2 km off bring her water as near as 250 m.
        let far = standoff(&view([0.0, 18.0, 0.0], 8.0), [0.0, -2000.0], 2.0);
        assert!((far - 250.0).abs() < 0.1, "{far}");
    }

    /// The air map's far plane (1,000 km): the same lattice with more horizon rows, the ring at 95% of it.
    #[test]
    fn a_chart_s_far_plane_grows_the_ring_alone() {
        let (scene, chart) = (build(WEB_HIGH, 60000.0), build(WEB_HIGH, 1_000_000.0));
        assert!((chart.radius - 950_000.0).abs() < 1e-6);
        let lattice = WEB_HIGH.levels * 65 * 65;
        assert_eq!(scene.vertices[..lattice], chart.vertices[..lattice]);
        assert!(chart.vertices.len() > scene.vertices.len());
        let reach = |mesh: &ClipmapMesh| {
            mesh.vertices
                .iter()
                .map(|v| f64::from(v.position[0]).hypot(f64::from(v.position[1])))
                .fold(0.0, f64::max)
        };
        assert!((reach(&scene) - 57_000.0).abs() < 1.0 && (reach(&chart) - 950_000.0).abs() < 1.0);
        // A view of the sea's ring: circles close enough for the Earth's curve (a chord 3.5 km long sags 0.24 m),
        // then the skirt's rows on the ring's circle. The chart's stand 15 km apart to 480 km (a theatre view of the
        // land beyond its window: a chord sags 4.4 m) and double beyond.
        let circles = |mesh: &ClipmapMesh| {
            let mut radii: Vec<f64> = mesh.vertices[lattice..]
                .iter()
                .filter(|v| v.level[0] == WEB_HIGH.levels as f32 && v.position[1] == 0.0)
                .filter(|v| v.position[0] > 0.0)
                .map(|v| f64::from(v.position[0]))
                .collect();
            radii.sort_by(f64::total_cmp);
            radii
        };
        let scene_circles = circles(&scene);
        assert!(scene_circles.windows(2).all(|w| w[1] - w[0] < 5300.0));
        let chart_circles = circles(&chart);
        assert!(
            chart_circles
                .windows(2)
                .filter(|w| w[1] < 470_000.0)
                .all(|w| w[1] - w[0] < 15_100.0)
        );
        assert!(chart_circles.windows(2).any(|w| w[1] > w[0] * 1.9));
        let theatre = circles(&build(WEB_HIGH, 450_000.0));
        assert!(theatre.windows(2).all(|w| w[1] - w[0] < 15_100.0));
        assert!((theatre.last().unwrap() - 427_500.0).abs() < 1.0);
        for mesh in [&scene, &chart] {
            let skirt = mesh
                .vertices
                .iter()
                .filter(|v| v.level[0] > WEB_HIGH.levels as f32)
                .count();
            assert_eq!(skirt, SKIRT_ROWS as usize * 128);
        }
        assert!(
            chart
                .indices
                .iter()
                .all(|&i| (i as usize) < chart.vertices.len())
        );
    }

    #[test]
    fn frames_snap_to_twice_the_cell() {
        let frames = WEB_HIGH.frames(3.0, -5.0);
        assert_eq!(frames[0], [0.0, -8.0, 4.0, 0.25]);
        assert_eq!(frames[5][2], 128.0);
        // Levels the tier lacks repeat its outermost.
        assert_eq!(frames[9], frames[5]);
    }

    /// The normal lens's half height, magnified `m` times.
    fn lens(m: f64) -> f64 {
        (26.0_f64).to_radians().tan() / m
    }

    const HIGH: Clipmap = Clipmap::new(128, 9);
    /// The Earth's curve as the camera carries it: one over twice its radius.
    const CURVE: f64 = 1.0 / (2.0 * 6_371_000.0);

    /// A foretop's view 35 m up through a lens magnifying `m` times: turned `yaw` from +z toward +x, raised `pitch`
    /// and rolled `roll` (radians), 16:9, over a sea that reaches 2 m.
    fn scope(m: f64, yaw: f64, pitch: f64, roll: f64) -> FanView {
        let ((sy, cy), (sp, cp), (sr, cr)) = (yaw.sin_cos(), pitch.sin_cos(), roll.sin_cos());
        let forward = [sy * cp, sp, cy * cp];
        let (level, raised) = ([-cy, 0.0, sy], [-sy * sp, cp, -cy * sp]);
        FanView {
            eye: [120.0, 35.0, -40.0],
            forward,
            right: std::array::from_fn(|i| level[i] * cr + raised[i] * sr),
            up: std::array::from_fn(|i| raised[i] * cr - level[i] * sr),
            tan_half_fov_y: lens(m),
            aspect: 16.0 / 9.0,
            pixels: 1080.0,
            surface: 2.0,
            curvature: CURVE,
            far: 60_000.0,
        }
    }

    #[test]
    fn the_normal_lens_a_low_eye_and_a_steep_view_keep_the_clipmap() {
        assert!(fan(&scope(1.0, 0.3, -0.01, 0.0), HIGH).is_none());
        assert!(fan(&scope(1.2, 0.3, -0.01, 0.0), HIGH).is_none());
        assert!(fan(&scope(2.0, 0.3, -0.01, 0.0), HIGH).is_some());
        // A periscope, or a low free camera: within 10 m of the highest the surface reaches.
        let low = FanView {
            eye: [0.0, 9.0, 0.0],
            ..scope(8.0, 0.3, -0.01, 0.0)
        };
        assert!(fan(&low, HIGH).is_none());
        // Looking down past 60 degrees, or with the frame's lower edge past 70, the sea lies all round the point under
        // the eye.
        assert!(fan(&scope(2.0, 0.3, -1.2, 0.0), HIGH).is_none());
        assert!(fan(&scope(1.3, 0.3, -1.0, 0.0), HIGH).is_none());
        assert!(fan(&scope(2.0, 0.3, -0.9, 0.0), HIGH).is_some());
    }

    #[test]
    fn a_fan_s_cells_cover_the_pixels_the_clipmap_s_do() {
        // At 8x a cell is 1.25 / 64 / 8 of its range across: 24 m at 10 km, where the clipmap about the eye has 312 m,
        // and 1.3 m where the lens's lower edge meets the sea.
        let view = scope(8.0, 0.3, -0.004, 0.0);
        let eight = fan(&view, HIGH).unwrap();
        assert!((eight.step * 8.0 * 64.0 / 1.25 - 1.0).abs() < 1e-3);
        assert!(
            (std::f64::consts::TAU / eight.step)
                .fract()
                .min(1.0 - (std::f64::consts::TAU / eight.step).fract())
                < 1e-6
        );
        assert!(
            eight.range(0) > 400.0 && eight.range(0) < 540.0,
            "{eight:?}"
        );
        // To the sea's horizon (21 km from 35 m) and a crest beyond.
        let reach = eight.range(eight.rows);
        assert!(reach > 27_000.0 && reach < 32_000.0, "{reach} {eight:?}");
        // Square cells out to the knee, where one stands a pixel and a half tall on a 1080-line drawing; from there
        // every row does, and the waves are read at the rows' spacing.
        let pixel = 2.0 * lens(8.0) / 1080.0;
        let tall = |range: f64, deep: f64| deep * 35.0 / (range * range) / pixel;
        assert!(
            (tall(eight.knee, eight.knee * eight.step) - 1.5).abs() < 0.1,
            "{eight:?}"
        );
        assert!(eight.knee > 450.0 && eight.knee < 560.0, "{eight:?}");
        // (All but the last row, which stands where the fan ends.)
        let rows: Vec<f64> = (0..eight.rows).map(|row| eight.range(row)).collect();
        assert_eq!(eight.range(eight.rows), eight.far);
        for pair in rows.windows(2) {
            let (range, deep) = (pair[0], pair[1] - pair[0]);
            assert!(deep > 0.0 && tall(range, deep) > 1.4, "{range} {deep}");
            if pair[1] < eight.knee {
                assert!((deep / range - eight.step).abs() < 1e-4, "{range} {deep}");
                assert!((eight.spacing(range) - range * eight.step).abs() < 1e-9);
            } else if range > eight.knee && range < 10_000.0 {
                assert!(
                    (eight.spacing(range) / deep - 1.0).abs() < 0.1,
                    "{range} {deep}"
                );
            }
        }
        assert!(eight.rows < 500, "{eight:?}");
        // The frame's width in cells, a few to spare.
        let across = f64::from(eight.columns) * eight.step;
        let seen = 2.0 * (view.tan_half_fov_y * view.aspect).atan();
        assert!(
            across > seen && across < seen + 10.0 * eight.step,
            "{eight:?}"
        );
        // Never more cells than twice the tier's lattice, and with rows no thinner than a pixel and a half, far fewer
        // at any lens: no more rows than the drawing has lines.
        let budget = 2.0 * 9.0 * 128.0 * 128.0 * 0.75;
        for m in [1.5, 2.0, 4.0, 8.0, 16.0, 32.0] {
            let fan = fan(&scope(m, -2.0, -0.2 / m, 0.02), HIGH).unwrap();
            assert!(f64::from(fan.columns * fan.rows) <= budget, "{m}: {fan:?}");
            assert!(
                fan.columns <= FAN_COLUMNS && fan.blocks() <= FAN_BLOCKS,
                "{m}: {fan:?}"
            );
            assert!(
                (fan.step * m * 64.0 / 1.25 - 1.0).abs() < 0.05,
                "{m}: {fan:?}"
            );
            assert!(fan.rows < 1080, "{m}: {fan:?}");
        }
        // A coarser tier's cells are as much larger as its lattice's.
        let low = fan(&scope(2.0, 0.3, -0.05, 0.0), Clipmap::new(32, 6)).unwrap();
        assert!(low.step >= 1.25 / 16.0 / 2.0 * 0.99);
    }

    #[test]
    fn a_fan_covers_the_sea_the_lens_shows() {
        // Every line of sight in the frame, where it meets the highest and the lowest the flat sea's surface reaches,
        // lies between the fan's first row and its last and between its first column and its last.
        // The last three: a coarse tier's rows, deeper than the sea's horizon dips, still reach it; a high eye rolled,
        // the steepest line of sight between the frame's corners.
        let low = Clipmap::new(32, 6);
        for (m, yaw, pitch, roll, height, pixels, clipmap) in [
            (2.0, 0.0, -0.05, 0.0, 35.0, 1080.0, HIGH),
            (2.0, 3.1, -0.6, 0.1, 35.0, 1080.0, HIGH),
            (4.0, -1.3, 0.03, -0.15, 35.0, 1080.0, HIGH),
            (8.0, 2.2, -0.004, 0.05, 35.0, 1080.0, HIGH),
            (8.0, -3.0, -0.3, 0.0, 35.0, 1080.0, HIGH),
            (32.0, 0.7, -0.01, 0.2, 35.0, 1080.0, HIGH),
            (1.26, 0.4, 0.001, 0.0, 35.0, 180.0, low),
            (1.3, -2.0, -0.02, 0.0, 13.0, 720.0, low),
            (1.25, 1.0, -0.75, 0.14, 10_000.0, 1080.0, HIGH),
        ] {
            let view = FanView {
                eye: [120.0, height, -40.0],
                pixels,
                curvature: 0.0,
                far: 400_000.0,
                ..scope(m, yaw, pitch, roll)
            };
            let fan = fan(&view, clipmap).unwrap();
            let mut seen = 0;
            for i in 0..=40 {
                for j in 0..=200 {
                    let (sx, sy) = (f64::from(i) / 20.0 - 1.0, f64::from(j) / 100.0 - 1.0);
                    let line: [f64; 3] = std::array::from_fn(|axis| {
                        view.forward[axis]
                            + view.right[axis] * sx * view.tan_half_fov_y * view.aspect
                            + view.up[axis] * sy * view.tan_half_fov_y
                    });
                    for surface in [-view.surface, view.surface] {
                        let along = (surface - view.eye[1]) / line[1];
                        let range = along * line[0].hypot(line[2]);
                        if along <= 0.0 || range > view.far * HORIZON_REACH {
                            continue;
                        }
                        seen += 1;
                        let (first, last) = (fan.range(0), fan.range(fan.rows));
                        assert!(range >= first && range <= last, "{m} {range} {fan:?}");
                        let turn = (line[0].atan2(line[2]) - fan.bearing)
                            .rem_euclid(std::f64::consts::TAU);
                        assert!(
                            turn <= f64::from(fan.columns) * fan.step,
                            "{m} {turn} {fan:?}"
                        );
                    }
                }
            }
            assert!(seen > 400, "{m}: {seen}");
        }
        // A view of the sky alone draws no sea.
        assert_eq!(fan(&scope(8.0, 0.3, 0.3, 0.0), HIGH).unwrap().rows, 0);
    }

    #[test]
    fn turning_and_tilting_a_view_keep_the_fan_s_lattice() {
        // Columns on whole steps of bearing, rows on whole steps of the log of their range: a turn or a tilt adds and
        // drops cells at the frame's edges and moves none.
        let whole = |v: f64| (v - v.round()).abs() < 1e-6;
        let a = fan(&scope(8.0, 0.3, -0.004, 0.0), HIGH).unwrap();
        // (The last looks across the bearing where the heading wraps.)
        for (yaw, pitch) in [
            (0.3137, -0.004),
            (0.3, -0.021),
            (-1.9, -0.05),
            (3.13, -0.004),
            (-3.13, -0.004),
        ] {
            let b = fan(&scope(8.0, yaw, pitch, 0.0), HIGH).unwrap();
            assert_eq!((a.step, a.knee, a.squeeze), (b.step, b.knee, b.squeeze));
            let turns = (a.bearing - b.bearing) / a.step;
            assert!(whole(turns), "{a:?} {b:?}");
            assert!(whole(a.first) && whole(b.first), "{a:?} {b:?}");
        }
        // A lens a hair stronger or weaker is the same notch, as is an eye a metre higher as the ship rolls.
        assert_eq!(
            a.step,
            fan(&scope(8.2, 0.3, -0.004, 0.0), HIGH).unwrap().step
        );
        let raised = FanView {
            eye: [120.0, 36.0, -40.0],
            ..scope(8.0, 0.3, -0.004, 0.0)
        };
        let b = fan(&raised, HIGH).unwrap();
        assert_eq!((a.step, a.knee, a.squeeze), (b.step, b.knee, b.squeeze));
        assert!(fan(&scope(4.0, 0.3, -0.004, 0.0), HIGH).unwrap().step > a.step * 1.9);
    }

    #[test]
    fn the_fan_s_block_winds_as_the_clipmap_does() {
        let block = build_fan();
        assert_eq!(
            block.vertices.len(),
            ((FAN_COLUMNS + 1) * (FAN_BLOCK_ROWS + 1)) as usize
        );
        assert_eq!(
            block.indices.len(),
            (FAN_COLUMNS * FAN_BLOCK_ROWS * 6) as usize
        );
        // Placed as the vertex stage places them: bearing from +z toward +x, range growing with the row.
        let fan = Fan {
            step: 0.01,
            bearing: 0.4,
            columns: 8,
            knee: 120.0,
            squeeze: 0.01 / 120.0,
            first: -16.0,
            rows: 32,
            far: 1e9,
        };
        let place = |v: &OceanVertex| {
            let bearing = fan.bearing + f64::from(v.position[0]) * fan.step;
            let range = fan.range(v.position[1] as u32);
            (range * bearing.sin(), range * bearing.cos())
        };
        let turn = |vertices: &[(f64, f64)], triangle: &[u32]| {
            let [a, b, c] = [0, 1, 2].map(|i| vertices[triangle[i] as usize]);
            ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)).signum()
        };
        let clipmap = build(WEB_HIGH, 60000.0);
        let lattice: Vec<(f64, f64)> = clipmap
            .vertices
            .iter()
            .map(|v| (f64::from(v.position[0]), f64::from(v.position[1])))
            .collect();
        let way = turn(&lattice, &clipmap.indices[..3]);
        let placed: Vec<(f64, f64)> = block.vertices.iter().map(place).collect();
        // The columns a draw takes are the first of the indices.
        let drawn = (fan.columns * FAN_BLOCK_ROWS * 6) as usize;
        assert!(
            block.indices[..drawn]
                .chunks(3)
                .all(|t| turn(&placed, t) == way)
        );
        let last = block.indices[..drawn].iter().max().unwrap();
        assert_eq!(*last, (fan.columns + 1) * (FAN_BLOCK_ROWS + 1) - 1);
    }

    #[test]
    fn the_normal_lens_centres_on_the_eye() {
        let hulls = [(0.0, -2000.0)];
        let view = FocusView {
            eye: [10.0, 35.0, 20.0],
            forward: [0.0, -0.02, -1.0],
            tan_half_fov_y: lens(1.0),
            hulls: &hulls,
            surface: 2.0,
        };
        assert_eq!(focus(&view), [10.0, 20.0]);
    }

    #[test]
    fn binoculars_centre_on_the_hull_in_the_sight() {
        // Aimed over her deck: the view meets the sea far beyond her, but the clipmap centres on her.
        let hulls = [(0.0, -2000.0), (600.0, -2000.0), (0.0, 30.0)];
        let view = FocusView {
            eye: [0.0, 35.0, 0.0],
            forward: [0.0, -0.005, -1.0],
            tan_half_fov_y: lens(8.0),
            hulls: &hulls,
            surface: 2.0,
        };
        assert_eq!(focus(&view), [0.0, -2000.0]);
    }

    #[test]
    fn binoculars_on_open_sea_centre_where_the_view_meets_it() {
        let view = FocusView {
            eye: [0.0, 30.0, 0.0],
            forward: [0.6, -0.01, -0.8],
            tan_half_fov_y: lens(4.0),
            hulls: &[],
            surface: 2.0,
        };
        let [x, z] = focus(&view);
        let reach = 30.0 / 0.01;
        assert!((x - 0.6 * reach).abs() < 1e-6 && (z + 0.8 * reach).abs() < 1e-6);
        // Level or above the horizon: as far as the centre goes.
        let level = FocusView {
            forward: [0.0, 0.1, -1.0],
            ..view
        };
        assert!((focus(&level)[1] + FOCUS_REACH).abs() < 1e-6);
    }

    #[test]
    fn the_centre_eases_out_with_the_lens() {
        let at = |m: f64| {
            focus(&FocusView {
                eye: [0.0, 30.0, 0.0],
                forward: [0.0, -0.01, -1.0],
                tan_half_fov_y: lens(m),
                hulls: &[],
                surface: 2.0,
            })[1]
        };
        assert_eq!(at(1.2), 0.0);
        assert!(at(1.5) < 0.0 && at(1.5) > at(2.0));
        assert!((at(2.0) + 3000.0).abs() < 1e-6);
    }

    #[test]
    fn an_eye_near_the_water_keeps_the_clipmap() {
        // A periscope at 8x a metre over a 2 m sea: the probe reads the sea about the eye at full detail.
        let view = FocusView {
            eye: [5.0, 3.0, 7.0],
            forward: [0.0, -0.001, -1.0],
            tan_half_fov_y: lens(8.0),
            hulls: &[],
            surface: 2.0,
        };
        assert_eq!(focus(&view), [5.0, 7.0]);
    }
}
