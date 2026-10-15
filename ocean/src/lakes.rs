//! Lakes and ponds as water at their own levels (native only, 2026-10-05; native/docs/ocean.md, "Lakes and ponds"):
//! a theatre window's lakes (their levels on the region's 70 m grid, [`LakeGrid`]) drawn as flat water in the `Water`
//! phase beside the sea, shaded with the sea's optics (`shaders/lake.wgsl`: the reflection lobe, Fresnel and glitter of
//! `naval_ocean::reflection`, the water body of `naval_ocean::water_body`) with fresh water's colour, small wind ripples
//! and a soft shore read from the opaque scene's depth.
//!
//! The mesh ([`LakeMesh::build`]) covers each lake's samples and the squares between them and their neighbours (whose
//! drawn land the region bake raised above the level), merged into quads of at most [`QUAD_CELLS`] cells on a side and
//! grouped into chunks of [`CHUNK_CELLS`] for culling. The land crossing the level draws the outline near the camera;
//! where the land is drawn coarser than the lakes' grid (an air window's 140 m samples, or far off) the lakes' own mask
//! draws it ([`LakeMask`], the samples' bilinear contour), the water pulled toward the camera past the coarse land within
//! it. Placed as the land is: world = chart + [`LakeLayer::set_offset`]'s offset.

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use naval_render::copies::{color_copy_layout_entry, depth_copy_layout_entry};
use naval_render::taa::MotionPipeline;
use naval_render::{
    DrawContext, Phase, PipelineId, PrepareContext, RenderContext, SCENE_OPAQUE_COLOR,
    SCENE_OPAQUE_DEPTH, SceneCopies, SceneLayer, ScenePipeline, ShaderDefValue, ShaderError,
    VertexLayout, shader_file,
};

/// Cells a merged quad spans at most along either axis: 16 of the region's 70 m cells (1.1 km), so the Earth's curve,
/// applied per vertex, sags under a quad by at most its curvature times (560 m)², 2.5 cm.
pub const QUAD_CELLS: usize = 16;
/// Cells a chunk, the unit the layer culls, spans on a side (2.2 km of 70 m cells).
pub const CHUNK_CELLS: usize = 32;
/// A lake at or below this level (m) is taken for a coastal lagoon or salt pond, brackish or saline (the bay's salt ponds
/// stand at the bake's 1.25 m floor to 1.5 m), and drawn with that water's colour.
pub const SALT_LEVEL_M: f32 = 2.0;
/// The water's ripples are as rough as the open sea's (Cox and Munk's) over this fetch, calmer over less.
pub const FULL_FETCH_M: f32 = 10_000.0;

/// A window's lakes as the host hands them in: each sample's level on a regular grid, in the land layer's chart metres
/// (before its offset: world = chart + offset).
#[derive(Clone, Debug, PartialEq)]
pub struct LakeGrid {
    /// Chart metres of sample (0, 0).
    pub origin: [f64; 2],
    /// Metres between samples.
    pub cell: f64,
    pub columns: usize,
    pub rows: usize,
    /// Each sample's level (m), NaN where there is no lake; row by row, `columns` a row.
    pub levels: Vec<f32>,
}

impl LakeGrid {
    fn level(&self, c: i64, r: i64) -> Option<f32> {
        if c < 0 || r < 0 || c as usize >= self.columns || r as usize >= self.rows {
            return None;
        }
        let level = self.levels[r as usize * self.columns + c as usize];
        level.is_finite().then_some(level)
    }
}

/// One vertex of the water: its chart position at the lake's level, and the lake's own properties.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct LakeVertex {
    /// Chart x, the level, chart z (m).
    pub position: [f32; 3],
    /// x: the lake's fetch (m, the side of a square of its area), y: 1 for a lagoon or salt pond, z: a number in 0..1
    /// of its own (its colour's variation), w: unused.
    pub lake: [f32; 4],
}

/// A run of the index buffer the layer culls as one: its quads' bounds in chart metres (y the levels).
#[derive(Clone, Debug, PartialEq)]
pub struct LakeChunk {
    pub indices: Range<u32>,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Where the lakes are, for the shader to cut the water at their outline: 255 at a lake sample, the land beside them
/// a 3 × 3 binomial blur of the samples (64 beside a straight shore), 0 farther, read filtered, one texel of nothing
/// about the grid so it reads 0 beyond it.
#[derive(Clone, Debug, PartialEq)]
pub struct LakeMask {
    pub width: u32,
    pub height: u32,
    pub texels: Vec<u8>,
    /// Chart metres of texel (0, 0)'s centre (a cell before the grid's sample (0, 0)).
    pub origin: [f64; 2],
    pub cell: f64,
}

/// The water's triangles over a window's lakes.
#[derive(Clone, Debug, PartialEq)]
pub struct LakeMesh {
    pub vertices: Vec<LakeVertex>,
    pub indices: Vec<u32>,
    pub chunks: Vec<LakeChunk>,
    /// Lakes (samples 8-connected at one level).
    pub lakes: usize,
    pub mask: LakeMask,
}

/// What [`LakeMesh::build`] knows of each lake.
#[derive(Clone, Copy, Debug)]
struct LakeInfo {
    level: f32,
    samples: usize,
    seed: usize,
}

/// A 32-bit integer hash (Chris Wellons' lowbias32), to 0..1.
fn unit_hash(value: usize) -> f32 {
    let value = value as u64;
    let mut x = value as u32 ^ ((value >> 32) as u32).wrapping_mul(0x9e37_79b9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

impl LakeMesh {
    /// The water over `grid`'s lakes: each lake's samples labelled (8-connected at one level), every square between
    /// four samples with a corner in a lake taken for that lake's water (the highest's, where two lakes share one), the
    /// squares merged into quads within each chunk, and the mask.
    pub fn build(grid: &LakeGrid) -> Self {
        let (columns, rows) = (grid.columns, grid.rows);
        assert_eq!(grid.levels.len(), columns * rows, "a level a sample");
        // Each lake's samples, labelled from 1.
        let mut label = vec![0u32; columns * rows];
        let mut lakes: Vec<LakeInfo> = Vec::new();
        let mut stack = Vec::new();
        for start in 0..label.len() {
            let level = grid.levels[start];
            if !level.is_finite() || label[start] != 0 {
                continue;
            }
            lakes.push(LakeInfo {
                level,
                samples: 0,
                seed: start,
            });
            let id = lakes.len() as u32;
            label[start] = id;
            stack.push(start);
            while let Some(k) = stack.pop() {
                lakes[id as usize - 1].samples += 1;
                let (c, r) = ((k % columns) as i64, (k / columns) as i64);
                for dr in -1..=1 {
                    for dc in -1..=1 {
                        let (cc, rr) = (c + dc, r + dr);
                        if grid.level(cc, rr) == Some(level) {
                            let n = rr as usize * columns + cc as usize;
                            if label[n] == 0 {
                                label[n] = id;
                                stack.push(n);
                            }
                        }
                    }
                }
            }
        }
        // Square (c, r) lies between samples c..c + 1 and r..r + 1, c and r from -1; stored at (c + 1, r + 1).
        let (sw, sh) = (columns + 1, rows + 1);
        let mut square = vec![0u32; sw * sh];
        for r in -1..rows as i64 {
            for c in -1..columns as i64 {
                let mut best = 0u32;
                for (dc, dr) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (cc, rr) = (c + dc, r + dr);
                    if cc < 0 || rr < 0 || cc as usize >= columns || rr as usize >= rows {
                        continue;
                    }
                    let id = label[rr as usize * columns + cc as usize];
                    if id == 0 {
                        continue;
                    }
                    let higher = best == 0 || {
                        let (a, b) = (lakes[id as usize - 1].level, lakes[best as usize - 1].level);
                        a > b || (a == b && id < best)
                    };
                    if higher {
                        best = id;
                    }
                }
                square[(r + 1) as usize * sw + (c + 1) as usize] = best;
            }
        }
        // Quads within each chunk: runs along a row, then runs of equal span stacked down the rows.
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut chunks = Vec::new();
        let cell = grid.cell;
        let (tiles_x, tiles_z) = (sw.div_ceil(CHUNK_CELLS), sh.div_ceil(CHUNK_CELLS));
        for tz in 0..tiles_z {
            for tx in 0..tiles_x {
                let (x0, x1) = (tx * CHUNK_CELLS, ((tx + 1) * CHUNK_CELLS).min(sw));
                let (z0, z1) = (tz * CHUNK_CELLS, ((tz + 1) * CHUNK_CELLS).min(sh));
                let first = indices.len() as u32;
                let mut min = [f32::MAX; 3];
                let mut max = [f32::MIN; 3];
                // Open quads: (first column, end column, lake, first row, rows).
                let mut open: Vec<(usize, usize, u32, usize, usize)> = Vec::new();
                let mut emit = |q: (usize, usize, u32, usize, usize)| {
                    let (a, b, id, top, height) = q;
                    let info = lakes[id as usize - 1];
                    // Square column s spans chart x from origin + (s - 1) · cell.
                    let xa = (grid.origin[0] + (a as f64 - 1.0) * cell) as f32;
                    let xb = (grid.origin[0] + (b as f64 - 1.0) * cell) as f32;
                    let za = (grid.origin[1] + (top as f64 - 1.0) * cell) as f32;
                    let zb = (grid.origin[1] + ((top + height) as f64 - 1.0) * cell) as f32;
                    let lake = [
                        (info.samples as f64).sqrt() as f32 * cell as f32,
                        if info.level <= SALT_LEVEL_M { 1.0 } else { 0.0 },
                        unit_hash(info.seed),
                        0.0,
                    ];
                    let base = vertices.len() as u32;
                    for (x, z) in [(xa, za), (xa, zb), (xb, zb), (xb, za)] {
                        vertices.push(LakeVertex {
                            position: [x, info.level, z],
                            lake,
                        });
                    }
                    // Counter-clockwise seen from above (x right, z toward the viewer's bottom).
                    indices.extend_from_slice(&[
                        base,
                        base + 1,
                        base + 2,
                        base,
                        base + 2,
                        base + 3,
                    ]);
                    min = [min[0].min(xa), min[1].min(info.level), min[2].min(za)];
                    max = [max[0].max(xb), max[1].max(info.level), max[2].max(zb)];
                };
                for z in z0..z1 {
                    // This row's runs.
                    let mut runs: Vec<(usize, usize, u32)> = Vec::new();
                    let mut x = x0;
                    while x < x1 {
                        let id = square[z * sw + x];
                        if id == 0 {
                            x += 1;
                            continue;
                        }
                        let start = x;
                        while x < x1 && square[z * sw + x] == id && x - start < QUAD_CELLS {
                            x += 1;
                        }
                        runs.push((start, x, id));
                    }
                    // Extend the open quads this row continues; close the rest.
                    let mut next = Vec::with_capacity(runs.len());
                    for quad in open.drain(..) {
                        let continued = runs
                            .iter()
                            .position(|r| (r.0, r.1, r.2) == (quad.0, quad.1, quad.2))
                            .filter(|_| quad.4 < QUAD_CELLS);
                        match continued {
                            Some(k) => {
                                runs.swap_remove(k);
                                next.push((quad.0, quad.1, quad.2, quad.3, quad.4 + 1));
                            }
                            None => emit(quad),
                        }
                    }
                    next.extend(runs.into_iter().map(|(a, b, id)| (a, b, id, z, 1)));
                    open = next;
                }
                for quad in open.drain(..) {
                    emit(quad);
                }
                let last = indices.len() as u32;
                if last > first {
                    chunks.push(LakeChunk {
                        indices: first..last,
                        min,
                        max,
                    });
                }
            }
        }
        // The mask, a texel of nothing about the grid: the lakes' samples at 1, blurred once (a 3 × 3 binomial) about
        // them so the filtered contour rounds the grid's corners instead of stepping round them, a lake's own samples
        // kept at 1 (a one-sample pond keeps its middle).
        let (mw, mh) = (columns + 2, rows + 2);
        let lake = |c: i64, r: i64| -> u32 {
            u32::from(
                c >= 0
                    && r >= 0
                    && (c as usize) < columns
                    && (r as usize) < rows
                    && label[r as usize * columns + c as usize] != 0,
            )
        };
        let mut texels = vec![0u8; mw * mh];
        for r in -1..=rows as i64 {
            for c in -1..=columns as i64 {
                let value = if lake(c, r) == 1 {
                    255
                } else {
                    let mut sum = 0;
                    for (dr, wr) in [(-1i64, 1u32), (0, 2), (1, 1)] {
                        for (dc, wc) in [(-1i64, 1u32), (0, 2), (1, 1)] {
                            sum += lake(c + dc, r + dr) * wr * wc;
                        }
                    }
                    (sum * 255 / 16) as u8
                };
                texels[(r + 1) as usize * mw + (c + 1) as usize] = value;
            }
        }
        Self {
            vertices,
            indices,
            chunks,
            lakes: lakes.len(),
            mask: LakeMask {
                width: mw as u32,
                height: mh as u32,
                texels,
                origin: [grid.origin[0] - cell, grid.origin[1] - cell],
                cell,
            },
        }
    }

    /// The chart area (m²) the quads cover.
    pub fn area(&self) -> f64 {
        self.indices
            .chunks(6)
            .map(|q| {
                let (a, c) = (
                    self.vertices[q[0] as usize].position,
                    self.vertices[q[2] as usize].position,
                );
                f64::from((c[0] - a[0]).abs()) * f64::from((c[2] - a[2]).abs())
            })
            .sum()
    }
}

/// Which chunks of `chunks` (chart bounds) are drawn for a camera at `eye` with clip planes `planes` (from its flat
/// view-projection: inside where n · p + w ≥ 0), the lakes placed at `offset`: those in the view, allowing for the
/// Earth's curve (`curvature`, which lowers a point by curvature × d² where the camera draws it), and nearer than `range`.
/// Returns the chunks' indices, nearest first.
pub fn visible_chunks(
    chunks: &[LakeChunk],
    offset: [f64; 2],
    eye: Vec3,
    planes: &[Vec4],
    curvature: f32,
    range: f32,
) -> Vec<usize> {
    let mut out: Vec<(f32, usize)> = chunks
        .iter()
        .enumerate()
        .filter_map(|(k, chunk)| {
            let lo = Vec3::new(
                (f64::from(chunk.min[0]) + offset[0]) as f32,
                chunk.min[1],
                (f64::from(chunk.min[2]) + offset[1]) as f32,
            );
            let hi = Vec3::new(
                (f64::from(chunk.max[0]) + offset[0]) as f32,
                chunk.max[1],
                (f64::from(chunk.max[2]) + offset[1]) as f32,
            );
            // The box's nearest point to the eye, and its farthest corner across (how far the curve lowers it).
            let distance = eye.clamp(lo, hi).distance(eye);
            if distance > range {
                return None;
            }
            let far_x = (eye.x - lo.x).abs().max((eye.x - hi.x).abs());
            let far_z = (eye.z - lo.z).abs().max((eye.z - hi.z).abs());
            let drop = curvature * (far_x * far_x + far_z * far_z);
            let lo = Vec3::new(lo.x, lo.y - drop, lo.z);
            let inside = planes.iter().all(|p| {
                let x = if p.x >= 0.0 { hi.x } else { lo.x };
                let y = if p.y >= 0.0 { hi.y } else { lo.y };
                let z = if p.z >= 0.0 { hi.z } else { lo.z };
                p.x * x + p.y * y + p.z * z + p.w >= 0.0
            });
            inside.then_some((distance, k))
        })
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.into_iter().map(|(_, k)| k).collect()
}

/// A view-projection's clip planes (x, y in −w … w, reversed z in 0 … w), inside positive, normalised.
pub fn frustum_planes(view_projection: Mat4) -> Vec<Vec4> {
    let row = |i: usize| view_projection.row(i);
    let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2]
        .into_iter()
        .filter_map(|p| {
            let length = p.truncate().length();
            (length > 1e-12).then(|| p / length)
        })
        .collect()
}

/// How finely the lakes are shaded: High's ripples, three-tap sky lobe and shore, or Medium's cheaper path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LakeQuality {
    High,
    Medium,
}

/// How the lakes draw, fixed when the layer is made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LakeSettings {
    pub quality: LakeQuality,
    /// The spacing of the land's samples (m: a sea window's 40, an air window's 140). Where the land is drawn coarser
    /// than the lakes' own grid, their mask draws the outline.
    pub land_cell: f32,
    /// The land's finest vertices (m: its level 0, `naval_terrain::lod::LodSettings::spacing`) and the pixels a cell
    /// spans at the level it is drawn at (`target_pixels`: 5 on High, 9 on Medium), from which how coarse the land is
    /// drawn far off follows, as its level-of-detail ranges are laid out (`naval_terrain::lod::base_range`).
    pub land_spacing: f32,
    pub land_pixels: f32,
}

impl Default for LakeSettings {
    fn default() -> Self {
        Self {
            quality: LakeQuality::High,
            land_cell: 40.0,
            land_spacing: 20.0,
            land_pixels: 5.0,
        }
    }
}

/// Fresh water's absorption at 620, 550 and 460 nm (1/m): pure water's (Pope and Fry 1997) with a lake's dissolved
/// organic matter (a CDOM absorption of 0.6 /m at 440 nm falling 0.015 a nanometre) and a little chlorophyll: brown-green.
pub const FRESH_ABSORPTION: [f32; 3] = [0.31, 0.17, 0.52];
/// Fresh water's backscatter (1/m): a lake's suspended silt and plankton, many times the open ocean's.
pub const FRESH_BACKSCATTER: [f32; 3] = [0.010, 0.012, 0.013];
/// A lagoon's or salt pond's absorption (1/m): clearer of organic matter, with the algae of a salt pond's brine.
pub const SALT_ABSORPTION: [f32; 3] = [0.30, 0.09, 0.12];
/// A lagoon's or salt pond's backscatter (1/m): milky with fine sediment and salt crystals.
pub const SALT_BACKSCATTER: [f32; 3] = [0.032, 0.040, 0.042];

/// The window's satellite photograph, as the land layer holds it (`naval_terrain::TerrainLayer::surface_photo`): where it
/// covers a lake's middle the water takes the lake's own colour from it (a salt pond's red or green, a reservoir's silt).
#[derive(Clone, Debug)]
pub struct LakePhoto {
    /// The colour array (layer 0 the window's maps: sRGB, premultiplied by coverage, mipped).
    pub view: wgpu::TextureView,
    /// Chart metres of its corner, its side (m) and its texels a side.
    pub origin: [f64; 2],
    pub extent_m: f64,
    pub texels: u32,
    /// The haze to take out (linear reflectance a channel).
    pub haze: [f32; 3],
    /// Changes whenever the land's photograph does.
    pub version: u64,
}

/// What the layer draws, for the host's status and the tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LakeCounts {
    pub lakes: usize,
    pub chunks: usize,
    pub triangles: usize,
    pub drawn_chunks: usize,
    pub drawn_triangles: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct LakeUniforms {
    /// xy: the chart's offset (world = chart + offset), z: 1 with the scene's air applied, w: unused.
    offset: [f32; 4],
    /// xy: toward where the wind blows (unit, chart xz), z: its speed (m/s), w: the water shadows' quality.
    wind: [f32; 4],
    /// xy: chart metres at the mask's texture coordinate (0, 0), zw: texture coordinates a metre.
    mask: [f32; 4],
    /// x: the land's samples' spacing (m), y: its finest vertices' (m), z: the pixels its cells span, w: the lakes'
    /// cell (m).
    land: [f32; 4],
    /// x: how high the sea's crests reach (m).
    sea: [f32; 4],
    fresh_absorption: [f32; 4],
    fresh_backscatter: [f32; 4],
    salt_absorption: [f32; 4],
    salt_backscatter: [f32; 4],
    /// xy: chart metres at the photograph's corner, z: its texture coordinates a metre, w: 1 while bound.
    photo: [f32; 4],
    /// rgb: the haze to take out of it, w: its texels a side.
    photo_haze: [f32; 4],
}

fn vertex_layout() -> VertexLayout {
    VertexLayout {
        stride: std::mem::size_of::<LakeVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: vec![
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 12,
                shader_location: 1,
            },
        ],
    }
}

/// The lake shader's path in the renderer's library.
pub const LAKE_SHADER: &str = "naval-ocean/shaders/lake.wgsl";

/// A window's lakes as a renderer-core layer.
pub struct LakeLayer {
    settings: LakeSettings,
    offset: [f64; 2],
    chunks: Vec<LakeChunk>,
    lakes: usize,
    triangles: usize,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    uniforms: LakeUniforms,
    uniform_buffer: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    group: wgpu::BindGroup,
    environment: wgpu::BindGroup,
    mask_view: wgpu::TextureView,
    mask_sampler: wgpu::Sampler,
    cloud: (wgpu::TextureView, wgpu::Sampler, wgpu::Buffer),
    photo: Option<LakePhoto>,
    photo_placeholder: wgpu::TextureView,
    photo_sampler: wgpu::Sampler,
    copies_bound: Option<(u64, u64)>,
    pipeline: PipelineId,
    motion: PipelineId,
    /// Index ranges drawn this frame (adjacent chunks merged), and how many chunks they hold.
    drawn: Vec<Range<u32>>,
    drawn_chunks: usize,
    drawn_triangles: usize,
    range: f32,
    shown: bool,
}

impl LakeLayer {
    /// The water over `grid`'s lakes, lit by the sky's environment (its layout and bind group, as the ocean takes them),
    /// its pipelines on the renderer's registry. Draws nothing until placed ([`LakeLayer::set_offset`]: at the chart's
    /// origin until then).
    pub fn new(
        ctx: &mut RenderContext<'_>,
        grid: &LakeGrid,
        settings: LakeSettings,
        environment_layout: &wgpu::BindGroupLayout,
        environment: &wgpu::BindGroup,
    ) -> Result<Self, ShaderError> {
        Self::from_mesh(
            ctx,
            &LakeMesh::build(grid),
            settings,
            environment_layout,
            environment,
        )
    }

    /// As [`LakeLayer::new`], from a mesh built off the render thread.
    pub fn from_mesh(
        ctx: &mut RenderContext<'_>,
        mesh: &LakeMesh,
        settings: LakeSettings,
        environment_layout: &wgpu::BindGroupLayout,
        environment: &wgpu::BindGroup,
    ) -> Result<Self, ShaderError> {
        register(ctx)?;
        ctx.request_scene_copies(SceneCopies::OPAQUE_COLOR | SceneCopies::OPAQUE_DEPTH);
        let device = ctx.device.clone();
        let both = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lake water"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: both,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                color_copy_layout_entry(3, fragment),
                depth_copy_layout_entry(4, fragment),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: fragment,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let make = |label: &str, bytes: &[u8], usage: wgpu::BufferUsages| {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (bytes.len() as u64).max(16).next_multiple_of(4),
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            if !bytes.is_empty() {
                ctx.queue.write_buffer(&buffer, 0, bytes);
            }
            buffer
        };
        let vertices = make(
            "lake vertices",
            bytemuck::cast_slice(&mesh.vertices),
            wgpu::BufferUsages::VERTEX,
        );
        let indices = make(
            "lake indices",
            bytemuck::cast_slice(&mesh.indices),
            wgpu::BufferUsages::INDEX,
        );
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lake uniforms"),
            size: std::mem::size_of::<LakeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mask = &mesh.mask;
        let mask_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lake mask"),
            size: wgpu::Extent3d {
                width: mask.width,
                height: mask.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &mask_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &mask.texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(mask.width),
                rows_per_image: Some(mask.height),
            },
            wgpu::Extent3d {
                width: mask.width,
                height: mask.height,
                depth_or_array_layers: 1,
            },
        );
        let mask_view = mask_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mask_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lake mask"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let cloud = crate::surface::placeholder_cloud_shadow(&device);
        // No photograph: a texel of nothing (wgpu clears a texture never written to zeros).
        let photo_placeholder = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("lake photo placeholder"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
        let photo_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lake photo"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let (color, depth) = copies(ctx.resources);
        let group = Self::make_group(
            &device,
            &layout,
            &uniform_buffer,
            (&mask_view, &mask_sampler),
            (&color.0, &depth.0),
            &cloud,
            (&photo_placeholder, &photo_sampler),
        );
        let copies_bound = Some((color.1, depth.1));
        let defs = match settings.quality {
            LakeQuality::High => vec![("LAKE_HIGH".to_owned(), ShaderDefValue::Bool(true))],
            LakeQuality::Medium => vec![],
        };
        let pipeline = ctx.add_scene_pipeline(
            "lake water",
            LAKE_SHADER,
            ScenePipeline {
                vertex_buffers: vec![vertex_layout()],
                bind_group_layouts: vec![layout.clone(), environment_layout.clone()],
                defs: defs.clone(),
                ..ScenePipeline::new("vertex", "fragment")
            },
        )?;
        let motion = ctx.add_motion_pipeline(
            "lake water motion",
            LAKE_SHADER,
            MotionPipeline {
                vertex_buffers: vec![vertex_layout()],
                bind_group_layouts: vec![layout.clone(), environment_layout.clone()],
                defs,
                ..MotionPipeline::new("vertex", "motion")
            },
        )?;
        let uniforms = LakeUniforms {
            offset: [0.0, 0.0, 1.0, 0.0],
            wind: [1.0, 0.0, 4.0, 0.0],
            mask: [
                (mask.origin[0] - mask.cell / 2.0) as f32,
                (mask.origin[1] - mask.cell / 2.0) as f32,
                (1.0 / (mask.cell * f64::from(mask.width))) as f32,
                (1.0 / (mask.cell * f64::from(mask.height))) as f32,
            ],
            land: [
                settings.land_cell,
                settings.land_spacing,
                settings.land_pixels,
                mask.cell as f32,
            ],
            sea: [0.0; 4],
            fresh_absorption: [
                FRESH_ABSORPTION[0],
                FRESH_ABSORPTION[1],
                FRESH_ABSORPTION[2],
                0.0,
            ],
            fresh_backscatter: [
                FRESH_BACKSCATTER[0],
                FRESH_BACKSCATTER[1],
                FRESH_BACKSCATTER[2],
                0.0,
            ],
            salt_absorption: [
                SALT_ABSORPTION[0],
                SALT_ABSORPTION[1],
                SALT_ABSORPTION[2],
                0.0,
            ],
            salt_backscatter: [
                SALT_BACKSCATTER[0],
                SALT_BACKSCATTER[1],
                SALT_BACKSCATTER[2],
                0.0,
            ],
            photo: [0.0; 4],
            photo_haze: [0.0; 4],
        };
        Ok(Self {
            settings,
            offset: [0.0, 0.0],
            chunks: mesh.chunks.clone(),
            lakes: mesh.lakes,
            triangles: mesh.indices.len() / 3,
            vertices,
            indices,
            uniforms,
            uniform_buffer,
            layout,
            group,
            environment: environment.clone(),
            mask_view,
            mask_sampler,
            cloud,
            photo: None,
            photo_placeholder,
            photo_sampler,
            copies_bound,
            pipeline,
            motion,
            drawn: Vec::new(),
            drawn_chunks: 0,
            drawn_triangles: 0,
            range: 60_000.0,
            shown: true,
        })
    }

    fn make_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniforms: &wgpu::Buffer,
        (mask, mask_sampler): (&wgpu::TextureView, &wgpu::Sampler),
        copies: (&wgpu::TextureView, &wgpu::TextureView),
        cloud: &(wgpu::TextureView, wgpu::Sampler, wgpu::Buffer),
        (photo, photo_sampler): (&wgpu::TextureView, &wgpu::Sampler),
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lake water"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(mask),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(mask_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(copies.0),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(copies.1),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&cloud.0),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(&cloud.1),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: cloud.2.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(photo),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::Sampler(photo_sampler),
                },
            ],
        })
    }

    fn rebind(&mut self, device: &wgpu::Device, resources: &naval_render::Resources) {
        let (color, depth) = copies(resources);
        let photo = self
            .photo
            .as_ref()
            .map_or(&self.photo_placeholder, |p| &p.view);
        self.group = Self::make_group(
            device,
            &self.layout,
            &self.uniform_buffer,
            (&self.mask_view, &self.mask_sampler),
            (&color.0, &depth.0),
            &self.cloud,
            (photo, &self.photo_sampler),
        );
        self.copies_bound = Some((color.1, depth.1));
    }

    pub fn settings(&self) -> LakeSettings {
        self.settings
    }

    /// Where the lakes' chart stands in the world (world x, z = chart x, z + `offset`), as the land layer's
    /// (`naval_terrain::TerrainLayer::set_offset`): from the next frame.
    pub fn set_offset(&mut self, offset: [f64; 2]) {
        self.offset = offset;
    }

    pub fn offset(&self) -> [f64; 2] {
        self.offset
    }

    /// The clouds' shadow on the water (naval-clouds' map, sampler and uniforms, as [`crate::Ocean::set_cloud_shadow`]
    /// takes them), or none.
    pub fn set_cloud_shadow(
        &mut self,
        device: &wgpu::Device,
        shadow: Option<(&wgpu::TextureView, &wgpu::Sampler, &wgpu::Buffer)>,
    ) {
        self.cloud = match shadow {
            Some((map, sampler, uniforms)) => (map.clone(), sampler.clone(), uniforms.clone()),
            None => crate::surface::placeholder_cloud_shadow(device),
        };
        self.copies_bound = None;
    }

    /// The wind over the water: its speed (m/s) and the direction it blows toward (radians from +x toward +z, as
    /// [`crate::WaveParameters::wind_direction`]).
    pub fn set_wind(&mut self, speed: f32, direction: f32) {
        self.uniforms.wind[0] = direction.cos();
        self.uniforms.wind[1] = direction.sin();
        self.uniforms.wind[2] = speed.max(0.0);
    }

    /// The window's satellite photograph, or none: the lakes' own colours where it covers them, from the next frame.
    pub fn set_photo(&mut self, photo: Option<LakePhoto>) {
        self.uniforms.photo = match &photo {
            Some(p) => [
                p.origin[0] as f32,
                p.origin[1] as f32,
                (1.0 / p.extent_m.max(1.0)) as f32,
                1.0,
            ],
            None => [0.0; 4],
        };
        self.uniforms.photo_haze = match &photo {
            Some(p) => [p.haze[0], p.haze[1], p.haze[2], p.texels as f32],
            None => [0.0; 4],
        };
        self.photo = photo;
        self.copies_bound = None;
    }

    /// The version of the photograph bound (none without one).
    pub fn photo_version(&self) -> Option<u64> {
        self.photo.as_ref().map(|p| p.version)
    }

    /// How high the sea's crests reach (m): a lake lower than that is drawn in front of them (the sea's waves are drawn
    /// everywhere, under the land too, and would show through a lake near the sea's level).
    pub fn set_sea_crest(&mut self, metres: f32) {
        self.uniforms.sea[0] = metres.max(0.0);
    }

    /// The ships' shadows on the water at the Graphics menu's water-shadow quality (0 off … 3 high), as the sea's.
    pub fn set_water_shadows(&mut self, quality: u32) {
        self.uniforms.wind[3] = quality as f32;
    }

    /// Whether the scene's air (the aerial perspective every material takes) veils the water.
    pub fn set_fog(&mut self, on: bool) {
        self.uniforms.offset[2] = if on { 1.0 } else { 0.0 };
    }

    /// Chunks wholly farther than `metres` from the camera are not drawn: beyond the haze.
    pub fn set_range(&mut self, metres: f32) {
        self.range = metres.max(1.0);
    }

    /// Draws the water or not (A/B and timing).
    pub fn set_shown(&mut self, shown: bool) {
        self.shown = shown;
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    pub fn counts(&self) -> LakeCounts {
        LakeCounts {
            lakes: self.lakes,
            chunks: self.chunks.len(),
            triangles: self.triangles,
            drawn_chunks: self.drawn_chunks,
            drawn_triangles: self.drawn_triangles,
        }
    }

    fn drawing(&self) -> bool {
        self.shown && !self.drawn.is_empty()
    }
}

/// The opaque copies' views and versions.
fn copies(
    resources: &naval_render::Resources,
) -> ((wgpu::TextureView, u64), (wgpu::TextureView, u64)) {
    let color = resources
        .texture(SCENE_OPAQUE_COLOR)
        .expect("the opaque colour copy (requested)");
    let depth = resources
        .texture(SCENE_OPAQUE_DEPTH)
        .expect("the opaque depth copy (requested)");
    (
        (color.view.clone(), color.version),
        (depth.view.clone(), depth.version),
    )
}

/// Registers the lake shader and the ocean's modules it imports (the first five of `shaders::FILES`: the water's
/// properties, the noise, the sky environment, reflection and the water body; and the clouds' shadow), each only where
/// no layer has.
pub fn register(ctx: &mut RenderContext<'_>) -> Result<(), ShaderError> {
    if ctx
        .library
        .text(naval_clouds::shaders::SHADOW_FILE.path)
        .is_none()
    {
        ctx.add_shader(naval_clouds::shaders::SHADOW_FILE)?;
    }
    for file in &crate::shaders::FILES[..5] {
        if ctx.library.text(file.path).is_none() {
            ctx.add_shader(*file)?;
        }
    }
    ctx.add_shader(shader_file!("shaders/lake.wgsl"))
}

impl SceneLayer for LakeLayer {
    fn label(&self) -> &str {
        "lakes"
    }

    fn scene_copies(&self) -> SceneCopies {
        // Asked whenever there is water to draw, in view or not (the sea asks for the same copies every frame).
        if self.shown && !self.chunks.is_empty() {
            SceneCopies::OPAQUE_COLOR | SceneCopies::OPAQUE_DEPTH
        } else {
            SceneCopies::NONE
        }
    }

    fn prepare(&mut self, ctx: &mut PrepareContext<'_>) {
        let versions = ctx
            .resources
            .texture(SCENE_OPAQUE_COLOR)
            .zip(ctx.resources.texture(SCENE_OPAQUE_DEPTH))
            .map(|(c, d)| (c.version, d.version));
        if versions.is_some() && versions != self.copies_bound {
            self.rebind(ctx.device, ctx.resources);
        }
        let camera = ctx.camera;
        let eye = Vec3::from_array(camera.position);
        let planes = frustum_planes(Mat4::from_cols_array(&camera.view_projection));
        let visible = if self.shown {
            visible_chunks(
                &self.chunks,
                self.offset,
                eye,
                &planes,
                camera.curvature,
                self.range,
            )
        } else {
            Vec::new()
        };
        let mut ranges: Vec<Range<u32>> = visible
            .iter()
            .map(|&k| self.chunks[k].indices.clone())
            .collect();
        ranges.sort_by_key(|r| r.start);
        self.drawn.clear();
        self.drawn_triangles = 0;
        for r in ranges {
            self.drawn_triangles += (r.end - r.start) as usize / 3;
            match self.drawn.last_mut() {
                Some(last) if last.end == r.start => last.end = r.end,
                _ => self.drawn.push(r),
            }
        }
        self.drawn_chunks = visible.len();
        self.uniforms.offset[0] = self.offset[0] as f32;
        self.uniforms.offset[1] = self.offset[1] as f32;
        ctx.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&self.uniforms));
    }

    fn draw(&self, phase: Phase, pass: &mut wgpu::RenderPass<'_>, ctx: &DrawContext<'_>) {
        if phase != Phase::Water || !self.drawing() {
            return;
        }
        pass.set_pipeline(ctx.pipelines.render(self.pipeline));
        pass.set_bind_group(1, &self.group, &[]);
        pass.set_bind_group(2, &self.environment, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        for range in &self.drawn {
            pass.draw_indexed(range.clone(), 0, 0..1);
        }
    }

    fn draw_motion(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &DrawContext<'_>) {
        if !self.drawing() {
            return;
        }
        pass.set_pipeline(ctx.pipelines.render(self.motion));
        pass.set_bind_group(1, &self.group, &[]);
        pass.set_bind_group(2, &self.environment, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        for range in &self.drawn {
            pass.draw_indexed(range.clone(), 0, 0..1);
        }
    }
}

#[cfg(test)]
#[path = "lakes_tests.rs"]
mod tests;
