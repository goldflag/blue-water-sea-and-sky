//! The GPU half of the wave field, ported from the web game's `ocean/waves/field.ts`: evolves the CPU spectrum to the current
//! time, inverse-transforms every cascade with Stockham compute passes, persists crest foam, builds the mip chains, and
//! holds the uniforms the surface's shaders read (`surface.wgsl`'s `WaveUniforms`).
//!
//! Per update, on High (256², three cascades): two row passes (the first evolves the spectrum), one column pass, the
//! resolve pass over every layer, and four mip levels.

use std::f64::consts::PI;

use bytemuck::{Pod, Zeroable};
use naval_render::{
    MakePipelines, PipelineId, PipelineRegistry, RenderContext, ShaderError, compute_recipe,
};

use crate::groups::{group_cut, group_layout, group_share};
use crate::gusts::{GUST_WAVELENGTH, gust_strength};
use crate::hull_sea::{
    HullFootprint, HullSeaUniforms, HullSeaWave, cascade_modes, hull_sea_wavelength, split_level,
};
use crate::sea_state::{drawn_sea, sea_state_cascades};
use crate::shaders;
use crate::spectrum::{
    FOLD_PERIOD, GRAVITY, WaveParameters, WaveSpectrum, build_spectrum, cascade_bands,
};
use crate::whitecaps::{
    CascadeBreaking, cascade_breaking, set_breaking_thresholds, whitecap_area, whitecap_depth,
};
use crate::{CascadeInfo, OceanParameters};

/// The field textures' format: half floats, as the web's HalfFloat render targets.
pub const FIELD_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Mip chains stop at this many texels per edge.
pub const COARSEST_TEXELS: u32 = 16;
/// Anisotropy of the field sampler.
pub const ANISOTROPY: u16 = 16;
/// A threshold standing for "never breaks".
const NEVER: f64 = 1e4;
/// Turbulent spreading of whitecap foam and bubbles: FOAM_SPREAD × λ·c, at most SPREAD_MOST m²/s, ACROSS_SHARE of it
/// across the wind, the explicit step's gain held below SPREAD_LIMIT. Kept small: a breaker lays its foam along its own
/// path, and old foam is drawn out along the wind rather than spread, so a thin crest stays thin instead of rounding
/// into a disc.
const FOAM_SPREAD: f64 = 0.015;
const SPREAD_MOST: f64 = 1.0;
const ACROSS_SHARE: f64 = 0.25;
const SPREAD_LIMIT: f64 = 0.24;
/// Bubbles persist for this share of the foam's lifetime.
const BUBBLE_LIFE: f64 = 0.5;
/// The surf's phase is kept within this many turns of the swell (`WaveUniforms::shore`): each breaker line keeps its own
/// tear for as many periods, and an f32 phase stays fine.
const SURF_TURNS: f64 = 64.0;
/// Gale foam streaks (native only; `resolve.wgsl`): the whitecaps' foam left on the water drifts downwind with the
/// surface's current and gathers into long lines along the wind, from Beaufort 7's first streaks to a strong gale's
/// dense ones. They run on the second cascade (181 m on High: texels of 0.7 m, lines tens of metres long).
const STREAK_CASCADE: usize = 1;
const STREAK_ONSET: f64 = 13.0;
const STREAK_FULL: f64 = 20.0;
/// The surface's drift per m/s of wind: the wind-driven current (about 3 %) and the waves' Stokes drift.
const STREAK_DRIFT: f64 = 0.035;
/// How fast the convergences gather old foam (m/s per m/s of wind), and how far apart they lie (m).
const STREAK_GATHER: f64 = 0.035;
const STREAK_SPACING: f64 = 18.0;
/// The lines' meander (radians of their phase), wandering over this period (s).
const STREAK_MEANDER: f64 = 0.8;
const STREAK_MEANDER_PERIOD: f64 = 90.0;
/// Old foam's e-folding life in the streaks (s), and how fast the whitecaps' foam feeds them at full strength (1/s).
const STREAK_LIFE: f64 = 30.0;
const STREAK_FEED: f64 = 0.03;
/// Share of the tail (Cox–Munk's slope variance no cascade draws) that roughens the surface.
const TAIL_ROUGHNESS: f64 = 0.25;
/// The three fields per cascade layer.
const FIELDS: [&str; 3] = ["displacement", "derivatives", "extras"];

/// Radices (each at most 16) that transform `n` points in the fewest passes (`fftRadices`).
pub fn fft_radices(n: u32) -> Vec<u32> {
    assert!(
        n.is_power_of_two() && n >= 2,
        "FFT size {n} is not a power of two"
    );
    let bits = n.ilog2();
    let passes = bits.div_ceil(4);
    (0..passes)
        .map(|i| 1 << (bits / passes + u32::from(i < bits % passes)))
        .collect()
}

/// A wake's slick damps waves shorter than this (m): turbulence and the films it brings up still the capillary and short
/// gravity waves, while the swell runs through (`SLICK_WAVELENGTH`).
pub const SLICK_WAVELENGTH: f64 = 25.0;

/// Share of each cascade's slope in waves shorter than `wavelength` (m) (`shortShares`).
pub fn short_shares(cascades: &[CascadeInfo], wavelength: f64) -> Vec<f64> {
    cascade_bands(cascades)
        .iter()
        .enumerate()
        .map(|(i, band)| {
            let lo = if i > 0 {
                band.lo
            } else {
                2.0 * PI / cascades[0].size
            };
            let cut = lo.max(2.0 * PI / wavelength);
            ((band.hi / cut).ln() / (band.hi / lo).ln()).max(0.0)
        })
        .collect()
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct CascadeParams {
    a: [f32; 4],
    b: [f32; 4],
    /// Streaks: drift (texels/s along x and z), e-folding life (s, 0 none), feed (1/s).
    c: [f32; 4],
    /// Streaks' lines: cycles per tile along x and z, gathering speed (texels/s), meander (radians).
    d: [f32; 4],
}

/// The gale streaks' parameters on a cascade `size` metres across at `n` texels, under `wind` m/s toward the unit
/// vector (`wx`, `wz`): `c` and `d` of [`CascadeParams`], zero below the streaks' onset.
fn streak_params(size: f64, n: u32, wind: f64, wx: f64, wz: f64) -> ([f32; 4], [f32; 4]) {
    let t = ((wind - STREAK_ONSET) / (STREAK_FULL - STREAK_ONSET)).clamp(0.0, 1.0);
    let strength = t * t * (3.0 - 2.0 * t);
    if strength.is_nan() || strength <= 0.0 || size <= 0.0 {
        return ([0.0; 4], [0.0; 4]);
    }
    let texel = size / f64::from(n);
    let drift = STREAK_DRIFT * wind / texel;
    // The lattice wave nearest a spacing of STREAK_SPACING across the wind: periodic on the tile.
    let cycles = size / STREAK_SPACING;
    let (m, k) = ((-wz * cycles).round(), (wx * cycles).round());
    let lines = if m == 0.0 && k == 0.0 {
        [0.0, 1.0]
    } else {
        [m as f32, k as f32]
    };
    (
        [
            (drift * wx) as f32,
            (drift * wz) as f32,
            STREAK_LIFE as f32,
            (STREAK_FEED * strength) as f32,
        ],
        [
            lines[0],
            lines[1],
            (STREAK_GATHER * wind / texel) as f32,
            STREAK_MEANDER as f32,
        ],
    )
}

/// `FieldParams` in `fft.wgsl` and `resolve.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct FieldParams {
    time: [f32; 4],
    wind: [f32; 4],
    cascades: [CascadeParams; 3],
}

/// `PassParams` in `fft.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct PassParams {
    radix: u32,
    span: u32,
    horizontal: u32,
    evolve: u32,
}

/// `WaveUniforms` in `surface.wgsl`: what the surface reads of the waves.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct WaveUniforms {
    /// Per cascade: tile edge (m).
    pub tile: [f32; 4],
    /// Per cascade: texels per metre.
    pub texels: [f32; 4],
    /// Per cascade: longest wavelength held (m).
    pub longest: [f32; 4],
    /// Per cascade: whole slope variance.
    pub slopes: [f32; 4],
    /// Per cascade: `slopeLength` (m).
    pub slope_lengths: [f32; 4],
    /// Per cascade: 1 / the standard deviation of the coarser cascades' compression.
    pub gates: [f32; 4],
    /// Per cascade: share of its slope in waves that follow the gusts.
    pub gust_shares: [f32; 4],
    /// Per cascade: breaking groups: 1/cell length and width (1/m), drift (m/s), seconds of age per unit of
    /// −ln(bubbles/foam).
    pub groups: [[f32; 4]; 3],
    /// x: tail roughness, y: the whole tail, z: ripple tile (m), w: ripple texels per metre.
    pub tail: [f32; 4],
    /// x: index of the coarsest mip, y: cascade count, z: N, w: the wind's whitecap share.
    pub layout: [f32; 4],
    /// xy: unit vector the wind blows toward, z: the field's clock (s), w: the groups' edge level.
    pub wind: [f32; 4],
    /// x: gust strength a, y: wind speed, z: size scale, w: drift scale.
    pub gusts: [f32; 4],
    /// The game's own sea (`params`): x significant height, y wind speed, z wind direction (rad).
    pub sea: [f32; 4],
    /// Per cascade: share of its slope in the short waves a wake's slick stills (`slickShares`, waves under 25 m).
    pub slick_shares: [f32; 4],
    /// The coast ([`crate::shore`]): the heights map's frame (x, z of its first texel's corner, texture coordinate per
    /// metre on each axis, [`crate::shore::GridPlace::frame`]).
    pub shore_heights: [f32; 4],
    /// The exposure map's frame, the same way.
    pub shore_exposure: [f32; 4],
    /// Per cascade, and (w) the swell: significant height drawn on the open sea (m), which the depth breaks.
    pub shore_waves: [f32; 4],
    /// x: 1 with a coast, y: the swell's frequency (rad/s), z: 1 / the swell's shelter length (1/km), w: the surf's
    /// phase (radians: that frequency times the field's clock, within [`SURF_TURNS`] turns).
    pub shore: [f32; 4],
    /// Per fetch of [`crate::shore::table_fetch`]: each cascade's share of its amplitude kept there
    /// ([`crate::shore::band_fetch_table`]; w unused).
    pub shore_fetch: [[f32; 4]; crate::shore::FETCH_TABLE],
}

struct FieldSet {
    textures: [wgpu::Texture; 3],
    /// Full mip chains, as the surface samples them.
    sampled: [wgpu::TextureView; 3],
}

/// The GPU wave field. Call [`WaveField::update`] once per frame, then bind [`WaveField::surface_bind_group`].
pub struct WaveField {
    tier: Vec<CascadeInfo>,
    n: u32,
    count: u32,
    top: u32,
    /// The sea as drawn: the game's parameters with the realism switch applied.
    pub sea: WaveParameters,
    /// The current tiles: the tier's, grown with the sea's peak while the sea state is realistic.
    pub cascades: Vec<CascadeInfo>,
    /// Upper bounds (m) on height and horizontal displacement for the current spectrum.
    pub max_height: f64,
    pub max_horizontal_displacement: f64,
    /// The share of the sea's height drawn, 1 normally: the waves' amplitudes scale by it while their wavelengths,
    /// directions, phases and tiles stay those of the full sea (the realistic sea state resolves its wavelength from the
    /// full height). The native client draws a sea still building or subsiding toward a developer wind this way
    /// (`naval-client`'s `weather::follow_sim_sea`), so a change of share moves no crest.
    pub height_scale: f64,
    built_for: Option<(WaveParameters, bool, f64)>,
    /// A spectrum built since the last update's compute pass ([`WaveField::rebuild_spectrum`]): that pass runs even
    /// on a paused frame.
    spectrum_fresh: bool,
    breaking: Vec<CascadeBreaking>,
    uniforms: WaveUniforms,
    gust_cut: f64,
    last_phase: f32,
    current: usize,
    spectrum: wgpu::Buffer,
    field_params: wgpu::Buffer,
    wave_uniforms: wgpu::Buffer,
    /// The sea around hulls (`HullSea`): binding 5 of the surface's group.
    hull_sea: HullSeaUniforms,
    hull_sea_buffer: wgpu::Buffer,
    /// The first cascade's spectrum as `split_level` weighs it, and the combat wavelength its split was last chosen
    /// for (NaN: choose again).
    first_modes: Vec<f32>,
    combat_wavelength: f64,
    split_for: f64,
    sets: [FieldSet; 2],
    transform: wgpu::ComputePipeline,
    resolve: wgpu::ComputePipeline,
    mips: wgpu::ComputePipeline,
    /// The transform's, the resolve's and the mips' group layouts, for the registry's recipes.
    layouts: [wgpu::BindGroupLayout; 3],
    /// Every transform pass but the last column pass, in order.
    passes: Vec<wgpu::BindGroup>,
    /// The resolve pass writing set `i`, reading the other's extras.
    resolves: [wgpu::BindGroup; 2],
    /// Mip levels 1..=top of set `i`.
    mip_groups: [Vec<wgpu::BindGroup>; 2],
    surface_layout: wgpu::BindGroupLayout,
    surface_groups: [wgpu::BindGroup; 2],
    /// The field's own sets' views and sampler, to bind again with another coast.
    surface_sampler: wgpu::Sampler,
    shore_sampler: wgpu::Sampler,
    /// The coast the waves are read against ([`WaveField::set_shore`]): its map's grids and textures, and where its
    /// chart lies in the world ([`WaveField::set_shore_offset`]).
    shore: Option<Shore>,
    shore_textures: crate::shore::ShoreTextures,
    shore_offset: [f64; 2],
}

/// The clock the open sea's swell runs on beside the hulls' sines ([`HullSeaUniforms::set`]): the sea's own `swell_time`
/// (s), and how far the hulls are let go (`racing`, 0 … 1) while the simulation runs faster than the sea is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HullSeaClock {
    pub swell_time: f64,
    pub racing: f64,
}

/// A coast as the field binds it: its map (on the GPU too), and each cascade's table of what it keeps at each fetch.
struct Shore {
    map: std::sync::Arc<crate::shore::ShoreMap>,
}

/// A plain filterable 2D texture of the surface's group.
fn plain_texture_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// The surface's group for each of the field's two sets, with the coast's maps.
#[allow(clippy::too_many_arguments)]
fn surface_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sets: &[FieldSet; 2],
    sampler: &wgpu::Sampler,
    wave_uniforms: &wgpu::Buffer,
    hull_sea: &wgpu::Buffer,
    shore: &crate::shore::ShoreTextures,
    shore_sampler: &wgpu::Sampler,
) -> [wgpu::BindGroup; 2] {
    std::array::from_fn(|set| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ocean waves"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&sets[set].sampled[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&sets[set].sampled[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&sets[set].sampled[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wave_uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: hull_sea.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&shore.heights),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&shore.exposure),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(shore_sampler),
                },
            ],
        })
    })
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_buffer_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn array_texture_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    filterable: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: FIELD_FORMAT,
            view_dimension: wgpu::TextureViewDimension::D2Array,
        },
        count: None,
    }
}

fn level_view(texture: &wgpu::Texture, level: u32) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        base_mip_level: level,
        mip_level_count: Some(1),
        ..Default::default()
    })
}

/// The field's pipelines on the renderer's registry ([`WaveField::register_pipelines`]).
#[derive(Clone, Copy, Debug)]
pub struct FieldPipelines {
    transform: PipelineId,
    resolve: PipelineId,
    mips: PipelineId,
}

/// A compute pipeline over one group, as each of the field's passes is.
fn compute_pipeline(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    layout: &wgpu::BindGroupLayout,
    entry: &str,
) -> wgpu::ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(
            &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            }),
        ),
        module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}

impl WaveField {
    /// A field for the quality tier's cascades (all of one resolution).
    pub fn new(device: &wgpu::Device, tier: &[CascadeInfo]) -> Self {
        Self::build(device, tier, MakePipelines::Now)
    }

    /// Builds the field's three passes through the renderer's registry, as [`WaveField::new`] builds them. The
    /// ocean's files must be registered first ([`crate::shaders::register`]).
    pub fn register_pipelines(
        &self,
        ctx: &mut RenderContext<'_>,
    ) -> Result<FieldPipelines, ShaderError> {
        let [transform, resolve, mips] = &self.layouts;
        let mut add =
            |label: &str, shader: shaders::Entry, layout: &wgpu::BindGroupLayout, entry| {
                ctx.add_pipeline(
                    label,
                    shader.path,
                    Vec::new(),
                    compute_recipe(vec![layout.clone()], entry),
                )
            };
        Ok(FieldPipelines {
            transform: add("ocean transform", shaders::FFT, transform, "transform")?,
            resolve: add("ocean resolve", shaders::RESOLVE, resolve, "resolve")?,
            mips: add("ocean mips", shaders::MIPS, mips, "downsample")?,
        })
    }

    /// Takes the registry's current pipelines in place of the field's own; whether any changed.
    pub fn adopt_pipelines(&mut self, registry: &PipelineRegistry, ids: &FieldPipelines) -> bool {
        let mut changed = false;
        for (slot, id) in [
            (&mut self.transform, ids.transform),
            (&mut self.resolve, ids.resolve),
            (&mut self.mips, ids.mips),
        ] {
            let new = registry.compute(id);
            if slot != new {
                *slot = new.clone();
                changed = true;
            }
        }
        changed
    }

    pub(crate) fn build(
        device: &wgpu::Device,
        tier: &[CascadeInfo],
        make: MakePipelines<'_>,
    ) -> Self {
        let n = tier[0].resolution;
        assert!(
            tier.iter().all(|c| c.resolution == n) && (1..=3).contains(&tier.len()),
            "one to three cascades of one resolution"
        );
        let count = tier.len() as u32;
        let top = (n / COARSEST_TEXELS).max(1).ilog2();
        let atlas = u64::from(n) * u64::from(n) * u64::from(count);
        let spectrum = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean spectrum"),
            size: atlas * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let spectra: [wgpu::Buffer; 2] = std::array::from_fn(|i| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(["ocean spectra 0", "ocean spectra 1"][i]),
                size: atlas * 32,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        });
        let field_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean field params"),
            size: std::mem::size_of::<FieldParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let wave_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean wave uniforms"),
            size: std::mem::size_of::<WaveUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Zeroed: no hull coupled, the split unset.
        let hull_sea_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean hull sea"),
            size: std::mem::size_of::<HullSeaUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sets: [FieldSet; 2] = std::array::from_fn(|set| {
            let textures: [wgpu::Texture; 3] = std::array::from_fn(|f| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(&format!("ocean {} {set}", FIELDS[f])),
                    size: wgpu::Extent3d {
                        width: n,
                        height: n,
                        depth_or_array_layers: count,
                    },
                    mip_level_count: top + 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FIELD_FORMAT,
                    usage: wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
            });
            let sampled = std::array::from_fn(|f| {
                textures[f].create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                })
            });
            FieldSet { textures, sampled }
        });

        // Transform passes.
        let transform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean transform"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::COMPUTE),
                uniform_entry(1, wgpu::ShaderStages::COMPUTE),
                storage_buffer_entry(2, true),
                storage_buffer_entry(3, true),
                storage_buffer_entry(4, false),
            ],
        });
        let transform = make.compute(|| {
            let module = shaders::shader_module(device, shaders::FFT, &[]);
            compute_pipeline(
                device,
                "ocean transform",
                &module,
                &transform_layout,
                "transform",
            )
        });
        let pass_buffer = |params: PassParams| {
            use wgpu::util::DeviceExt;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ocean transform pass"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        let radices = fft_radices(n);
        let mut passes = Vec::new();
        let mut make_pass = |index: usize, params: PassParams| {
            let buffer = pass_buffer(params);
            // The evolving pass reads no spectra; it binds the other buffer so nothing aliases its output.
            let source = &spectra[(index + 1) % 2];
            let destination = &spectra[index % 2];
            passes.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ocean transform pass"),
                layout: &transform_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: field_params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: spectrum.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: source.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: destination.as_entire_binding(),
                    },
                ],
            }));
        };
        let mut span = 1;
        let mut index = 0;
        for (i, &radix) in radices.iter().enumerate() {
            make_pass(
                index,
                PassParams {
                    radix,
                    span,
                    horizontal: 1,
                    evolve: u32::from(i == 0),
                },
            );
            span *= radix;
            index += 1;
        }
        span = 1;
        for &radix in &radices[..radices.len() - 1] {
            make_pass(
                index,
                PassParams {
                    radix,
                    span,
                    horizontal: 0,
                    evolve: 0,
                },
            );
            span *= radix;
            index += 1;
        }
        let final_source = &spectra[(index - 1) % 2];
        let final_pass = pass_buffer(PassParams {
            radix: *radices.last().unwrap(),
            span,
            horizontal: 0,
            evolve: 0,
        });

        // Resolve.
        let resolve_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean resolve"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::COMPUTE),
                uniform_entry(1, wgpu::ShaderStages::COMPUTE),
                storage_buffer_entry(2, true),
                array_texture_entry(3, wgpu::ShaderStages::COMPUTE, false),
                storage_texture_entry(4),
                storage_texture_entry(5),
                storage_texture_entry(6),
                array_texture_entry(7, wgpu::ShaderStages::COMPUTE, false),
            ],
        });
        let resolve = make.compute(|| {
            let module = shaders::shader_module(device, shaders::RESOLVE, &[]);
            compute_pipeline(device, "ocean resolve", &module, &resolve_layout, "resolve")
        });
        let resolves: [wgpu::BindGroup; 2] = std::array::from_fn(|set| {
            let previous = level_view(&sets[1 - set].textures[2], 0);
            let previous_displacement = level_view(&sets[1 - set].textures[0], 0);
            let outputs: [wgpu::TextureView; 3] =
                std::array::from_fn(|f| level_view(&sets[set].textures[f], 0));
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ocean resolve"),
                layout: &resolve_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: field_params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: final_pass.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: final_source.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&previous),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&outputs[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&outputs[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&outputs[2]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&previous_displacement),
                    },
                ],
            })
        });

        // Mip chains.
        let mips_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean mips"),
            entries: &[
                array_texture_entry(0, wgpu::ShaderStages::COMPUTE, false),
                array_texture_entry(1, wgpu::ShaderStages::COMPUTE, false),
                array_texture_entry(2, wgpu::ShaderStages::COMPUTE, false),
                storage_texture_entry(3),
                storage_texture_entry(4),
                storage_texture_entry(5),
            ],
        });
        let mips = make.compute(|| {
            let module = shaders::shader_module(device, shaders::MIPS, &[]);
            compute_pipeline(device, "ocean mips", &module, &mips_layout, "downsample")
        });
        let mip_groups: [Vec<wgpu::BindGroup>; 2] = std::array::from_fn(|set| {
            (1..=top)
                .map(|level| {
                    let sources: [wgpu::TextureView; 3] =
                        std::array::from_fn(|f| level_view(&sets[set].textures[f], level - 1));
                    let outputs: [wgpu::TextureView; 3] =
                        std::array::from_fn(|f| level_view(&sets[set].textures[f], level));
                    let mut entries = Vec::new();
                    for (f, view) in sources.iter().chain(outputs.iter()).enumerate() {
                        entries.push(wgpu::BindGroupEntry {
                            binding: f as u32,
                            resource: wgpu::BindingResource::TextureView(view),
                        });
                    }
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("ocean mip level"),
                        layout: &mips_layout,
                        entries: &entries,
                    })
                })
                .collect()
        });

        // What the surface reads (and the camera's height probe, a compute pass).
        let visibility =
            wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE;
        let surface_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean waves"),
            entries: &[
                array_texture_entry(0, visibility, true),
                array_texture_entry(1, visibility, true),
                array_texture_entry(2, visibility, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform_entry(4, visibility),
                uniform_entry(5, visibility),
                plain_texture_entry(6, visibility),
                plain_texture_entry(7, visibility),
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shore_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ocean shore"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shore_textures = crate::shore::ShoreTextures::open(device);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ocean waves"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: ANISOTROPY,
            ..Default::default()
        });
        let surface_groups = surface_groups(
            device,
            &surface_layout,
            &sets,
            &sampler,
            &wave_uniforms,
            &hull_sea_buffer,
            &shore_textures,
            &shore_sampler,
        );

        Self {
            tier: tier.to_vec(),
            n,
            count,
            top,
            sea: WaveParameters::default(),
            cascades: tier.to_vec(),
            max_height: 0.0,
            max_horizontal_displacement: 0.0,
            height_scale: 1.0,
            built_for: None,
            spectrum_fresh: false,
            breaking: Vec::new(),
            uniforms: WaveUniforms::default(),
            gust_cut: GUST_WAVELENGTH,
            last_phase: -1.0,
            current: 0,
            spectrum,
            field_params,
            wave_uniforms,
            hull_sea: HullSeaUniforms::default(),
            hull_sea_buffer,
            first_modes: Vec::new(),
            combat_wavelength: 0.0,
            split_for: f64::NAN,
            sets,
            transform,
            resolve,
            mips,
            layouts: [
                transform_layout.clone(),
                resolve_layout.clone(),
                mips_layout.clone(),
            ],
            passes,
            resolves,
            mip_groups,
            surface_layout,
            surface_groups,
            surface_sampler: sampler,
            shore_sampler,
            shore: None,
            shore_textures,
            shore_offset: [0.0, 0.0],
        }
    }

    /// Texels per cascade edge.
    pub fn resolution(&self) -> u32 {
        self.n
    }

    /// Index of the coarsest mip level.
    pub fn coarsest_level(&self) -> u32 {
        self.top
    }

    /// The latest displacement, derivatives and extras textures (one layer per cascade).
    pub fn textures(&self) -> &[wgpu::Texture; 3] {
        &self.sets[self.current].textures
    }

    /// The uniforms the surface reads, as last written.
    pub fn uniforms(&self) -> &WaveUniforms {
        &self.uniforms
    }

    /// Layout of [`WaveField::surface_bind_group`]: the three field arrays, their sampler, [`WaveUniforms`].
    pub fn surface_layout(&self) -> &wgpu::BindGroupLayout {
        &self.surface_layout
    }

    /// The current fields for the surface's shaders.
    pub fn surface_bind_group(&self) -> &wgpu::BindGroup {
        &self.surface_groups[self.current]
    }

    /// Near `hulls`, blend the drawn long waves into the sea they ride (`couple`): `waves` at `time`, the simulation's
    /// clock (not the field's), their open sea's spread on the `clock` given ([`HullSeaClock`]), about `origin` (a world
    /// point near the camera, keeping f32 phases small). Only the
    /// realistic sea state couples (`sea_state`), and it draws that sea everywhere, not only near the hulls
    /// ([`HullSeaUniforms::set_everywhere`]); the art-directed sea is drawn as given. Written with the next update.
    pub fn couple(
        &mut self,
        waves: &[HullSeaWave],
        time: f64,
        clock: HullSeaClock,
        hulls: &[HullFootprint],
        origin: (f64, f64),
        sea_state: bool,
    ) {
        self.hull_sea.set(
            waves,
            time,
            clock.swell_time,
            clock.racing,
            if sea_state { hulls } else { &[] },
            origin,
        );
        // The realistic sea draws the sea the hulls ride everywhere: its long swell crosses the open sea too.
        self.hull_sea.set_everywhere(sea_state);
        self.combat_wavelength = hull_sea_wavelength(waves);
        self.choose_split();
    }

    /// The sea around hulls as last coupled.
    pub fn hull_sea(&self) -> &HullSeaUniforms {
        &self.hull_sea
    }

    /// Read the waves against a coast ([`crate::shore`]): `map`'s land and fetch, its chart placed in the world by
    /// [`WaveField::set_shore_offset`]; none, the open sea everywhere.
    pub fn set_shore(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        map: Option<std::sync::Arc<crate::shore::ShoreMap>>,
    ) {
        self.shore_textures = match &map {
            Some(map) => crate::shore::ShoreTextures::new(device, queue, map),
            None => crate::shore::ShoreTextures::open(device),
        };
        self.shore = map.map(|map| Shore { map });
        self.surface_groups = surface_groups(
            device,
            &self.surface_layout,
            &self.sets,
            &self.surface_sampler,
            &self.wave_uniforms,
            &self.hull_sea_buffer,
            &self.shore_textures,
            &self.shore_sampler,
        );
    }

    /// Where the coast's chart lies in the world: world = chart + `offset` (a land window's offset).
    pub fn set_shore_offset(&mut self, offset: [f64; 2]) {
        self.shore_offset = offset;
    }

    /// Whether the waves are read against a coast.
    pub fn has_shore(&self) -> bool {
        self.shore.is_some()
    }

    /// What the coast leaves of the waves at world point (`x`, `z`), as the shader reads it (the uniforms as last
    /// written); none without a coast.
    pub fn shore_sample(&self, x: f64, z: f64) -> Option<crate::shore::ShoreSample> {
        let shore = self.shore.as_ref()?;
        let u = &self.uniforms;
        let tables: Vec<[f64; crate::shore::FETCH_TABLE]> = (0..self.count as usize)
            .map(|c| std::array::from_fn(|i| f64::from(u.shore_fetch[i][c])))
            .collect();
        Some(shore.map.sample(
            [x - self.shore_offset[0], z - self.shore_offset[1]],
            &tables,
            u.shore_waves.map(f64::from),
        ))
    }

    /// The coast's uniforms for this frame at the field's clock `time` (s): its maps' frames, each cascade's fetch law
    /// and open-sea height and the swell's, and the surf's phase on the swell's period.
    fn place_shore(&mut self, time: f64) {
        let u = &mut self.uniforms;
        let Some(shore) = &self.shore else {
            u.shore = [0.0; 4];
            return;
        };
        u.shore_heights = shore.map.heights.place().frame(self.shore_offset);
        u.shore_exposure = shore.map.exposure.place().frame(self.shore_offset);
        // The swell: combat's sines, as the hulls ride them (their spread shares their energy).
        let swell: f64 = self
            .hull_sea
            .waves
            .iter()
            .map(|w| f64::from(w[2]).powi(2) / 2.0)
            .sum();
        u.shore_waves[3] = (4.0 * swell.sqrt()) as f32;
        // The surf runs in on the swell's period, else on the sea's peak.
        let k = f64::from(self.hull_sea.waves[0][0]).hypot(f64::from(self.hull_sea.waves[0][1]));
        let k = if k > 0.0 {
            k
        } else {
            2.0 * PI / self.sea.peak_wavelength.max(10.0)
        };
        let frequency = (GRAVITY * k).sqrt();
        let phase = (frequency * time).rem_euclid(2.0 * PI * SURF_TURNS);
        u.shore = [
            1.0,
            frequency as f32,
            (1000.0 / crate::shore::SWELL_SHELTER_M) as f32,
            phase as f32,
        ];
    }

    /// Split the first cascade for the combat sea's wavelength, once per spectrum and wavelength (`chooseSplit`).
    fn choose_split(&mut self) {
        if self.split_for == self.combat_wavelength
            || self.combat_wavelength.is_nan()
            || self.combat_wavelength <= 0.0
            || self.built_for.is_none()
        {
            return;
        }
        let level = split_level(
            &self.first_modes,
            self.cascades[0].size / f64::from(self.n),
            f64::from(self.top),
            self.combat_wavelength,
        );
        self.hull_sea.set_split(level, f64::from(self.top));
        self.split_for = self.combat_wavelength;
    }

    /// Point every size-dependent read at a tile layout (`layOut`).
    fn lay_out(&mut self, cascades: Vec<CascadeInfo>) {
        let bands = cascade_bands(&cascades);
        let count = cascades.len();
        let finest = bands[count - 1];
        let gust_shares = short_shares(&cascades, self.gust_cut);
        let slick_shares = short_shares(&cascades, SLICK_WAVELENGTH);
        let u = &mut self.uniforms;
        for (i, cascade) in cascades.iter().enumerate() {
            u.gust_shares[i] = gust_shares[i] as f32;
            u.slick_shares[i] = slick_shares[i] as f32;
            u.tile[i] = cascade.size as f32;
            u.texels[i] = (f64::from(self.n) / cascade.size) as f32;
            u.longest[i] = if i > 0 {
                2.0 * PI / bands[i].lo
            } else {
                cascades[0].size
            } as f32;
        }
        let ripple = if count > 1 {
            cascades[count - 1].size * finest.lo / finest.hi
        } else {
            0.0
        };
        u.tail[2] = ripple as f32;
        u.tail[3] = if count > 1 {
            f64::from(self.n) / ripple
        } else {
            0.0
        } as f32;
        self.cascades = cascades;
    }

    /// Whitecap statistics of a rebuilt spectrum (`prepareWhitecaps`).
    fn prepare_whitecaps(&mut self, spectrum: &WaveSpectrum) {
        let choppiness = self.sea.choppiness.max(0.0);
        let (wz, wx) = self.sea.wind_direction.sin_cos();
        self.breaking = cascade_breaking(spectrum, wx, wz, choppiness);
        let mut coarser = 0.0;
        for (c, cascade) in spectrum.cascades.iter().enumerate() {
            self.uniforms.gates[c] = if coarser > 0.0 {
                1.0 / f64::sqrt(coarser)
            } else {
                0.0
            } as f32;
            coarser += choppiness.powi(2) * cascade.slope_variance;
        }
    }

    /// Rebuild the spectrum if the sea changed, then evolve and transform to `time` (s). `dt` advances foam
    /// persistence; 0 renders without advancing it. Returns whether any GPU work was encoded.
    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        parameters: &OceanParameters,
        time: f64,
        dt: f64,
    ) -> bool {
        self.update_timed(queue, encoder, parameters, time, dt, None)
    }

    /// [`WaveField::update`], writing GPU timestamps at the start and end of its compute pass into `timestamps` at
    /// the two given indices (needs `Features::TIMESTAMP_QUERY`).
    pub fn update_timed(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        parameters: &OceanParameters,
        time: f64,
        dt: f64,
        timestamps: Option<(&wgpu::QuerySet, u32, u32)>,
    ) -> bool {
        let uploads = &mut naval_render::UploadSink::Queue(queue);
        self.update_with(queue, uploads, encoder, parameters, time, dt, || {
            timestamps.map(|(query_set, begin, end)| wgpu::ComputePassTimestampWrites {
                query_set,
                beginning_of_pass_write_index: Some(begin),
                end_of_pass_write_index: Some(end),
            })
        })
    }

    /// [`WaveField::update`], its compute pass timed by the renderer core's timer as `ocean waves` (a paused frame
    /// that encodes nothing takes no timestamps), the frame's uniforms written through `uploads` (the renderer core's
    /// staging belt in a layer's `prepare`; a rebuilt spectrum through `queue`).
    #[allow(clippy::too_many_arguments)]
    pub fn update_with_timer(
        &mut self,
        queue: &wgpu::Queue,
        uploads: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        parameters: &OceanParameters,
        time: f64,
        dt: f64,
        timer: Option<&naval_render::GpuTimer>,
    ) -> bool {
        self.update_with(queue, uploads, encoder, parameters, time, dt, || {
            timer.and_then(|timer| timer.compute_pass("ocean waves"))
        })
    }

    /// Builds the spectrum for `parameters` when the sea changed since the last one (the CPU half of a rebuild, about
    /// 400k wave modes at Ultra, and its upload): the next update's compute pass takes it up. A theatre's scene calls
    /// it while it builds, off the frame loop, so the frame that swaps it in does not.
    pub fn rebuild_spectrum(&mut self, queue: &wgpu::Queue, parameters: &OceanParameters) {
        let params = &parameters.waves;
        if self.built_for == Some((*params, parameters.sea_state, self.height_scale)) {
            return;
        }
        self.sea = drawn_sea(params, parameters.sea_state);
        // Scaled after the realistic sea resolved its wavelength from the full height, so only the amplitudes move.
        self.sea.significant_height *= self.height_scale;
        let cascades = if parameters.sea_state {
            sea_state_cascades(&self.tier, self.sea.peak_wavelength)
        } else {
            self.tier.clone()
        };
        self.lay_out(cascades);
        let spectrum = build_spectrum(&self.cascades, &self.sea);
        let n = self.n as usize;
        let count = self.count as usize;
        let mut atlas = vec![0f32; n * n * count * 4];
        for (c, cascade) in spectrum.cascades.iter().enumerate() {
            for z in 0..n {
                let row = &cascade.amplitudes[z * n * 4..(z + 1) * n * 4];
                atlas[(z * count + c) * n * 4..(z * count + c + 1) * n * 4].copy_from_slice(row);
            }
            self.uniforms.slopes[c] = cascade.slope_variance as f32;
            self.uniforms.slope_lengths[c] = cascade.slope_length as f32;
        }
        queue.write_buffer(&self.spectrum, 0, bytemuck::cast_slice(&atlas));
        self.max_height = spectrum.max_height;
        self.max_horizontal_displacement = spectrum.max_horizontal_displacement;
        self.uniforms.tail[0] = (spectrum.tail_slope_variance * TAIL_ROUGHNESS) as f32;
        self.uniforms.tail[1] = spectrum.tail_slope_variance as f32;
        self.prepare_whitecaps(&spectrum);
        self.first_modes = cascade_modes(
            &spectrum.cascades[0].amplitudes,
            self.n as usize,
            self.cascades[0].size,
        );
        // What a coast leaves of each cascade at each fetch, and the height its depth breaks.
        for (c, cascade) in spectrum.cascades.iter().enumerate() {
            let table = if c == 0 {
                crate::shore::band_fetch_table(&self.first_modes, self.sea.wind_speed)
            } else {
                let modes = cascade_modes(&cascade.amplitudes, self.n as usize, cascade.size);
                crate::shore::band_fetch_table(&modes, self.sea.wind_speed)
            };
            for (entry, kept) in self.uniforms.shore_fetch.iter_mut().zip(table) {
                entry[c] = kept as f32;
            }
            self.uniforms.shore_waves[c] = (4.0 * cascade.variance.max(0.0).sqrt()) as f32;
        }
        self.built_for = Some((*params, parameters.sea_state, self.height_scale));
        self.split_for = f64::NAN;
        self.choose_split();
        self.spectrum_fresh = true;
    }

    /// The update, asking `timestamps` for the compute pass's writes only when it encodes the pass.
    #[allow(clippy::too_many_arguments)]
    fn update_with<'t>(
        &mut self,
        queue: &wgpu::Queue,
        uploads: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        parameters: &OceanParameters,
        time: f64,
        dt: f64,
        timestamps: impl FnOnce() -> Option<wgpu::ComputePassTimestampWrites<'t>>,
    ) -> bool {
        let params = &parameters.waves;
        self.rebuild_spectrum(queue, parameters);
        let rebuilt = std::mem::take(&mut self.spectrum_fresh);
        // Live, paused or not.
        let gusts = &parameters.gusts;
        let strength = if parameters.gusts_enabled {
            gusts
                .strength
                .unwrap_or_else(|| gust_strength(self.sea.wind_speed))
        } else {
            0.0
        };
        let cut = gusts.wavelength.unwrap_or(GUST_WAVELENGTH);
        if cut != self.gust_cut {
            self.gust_cut = cut;
            for (i, share) in short_shares(&self.cascades, cut).into_iter().enumerate() {
                self.uniforms.gust_shares[i] = share as f32;
            }
        }
        let (wz, wx) = self.sea.wind_direction.sin_cos();
        let u = &mut self.uniforms;
        u.gusts = [
            strength as f32,
            self.sea.wind_speed as f32,
            gusts.size.unwrap_or(1.0) as f32,
            gusts.drift.unwrap_or(1.0) as f32,
        ];
        u.sea = [
            self.sea.significant_height as f32,
            params.wind_speed as f32,
            params.wind_direction as f32,
            0.0,
        ];
        self.place_shore(time);
        let phase = ((time % FOLD_PERIOD + FOLD_PERIOD) % FOLD_PERIOD / FOLD_PERIOD) as f32;
        // A paused frame with nothing changed keeps last frame's fields, foam included.
        if !rebuilt && dt <= 0.0 && phase == self.last_phase {
            uploads.write_buffer(&self.wave_uniforms, 0, bytemuck::bytes_of(&self.uniforms));
            uploads.write_buffer(&self.hull_sea_buffer, 0, bytemuck::bytes_of(&self.hull_sea));
            return false;
        }
        self.last_phase = phase;
        let crest = &parameters.foam.crest;
        set_breaking_thresholds(
            &mut self.breaking,
            whitecap_depth(params.wind_speed, crest.coverage_scale),
        );
        let share = whitecap_area(params.wind_speed, crest.coverage_scale);
        let n = self.n;
        let meander = (std::f64::consts::TAU * (time / STREAK_MEANDER_PERIOD).fract()) as f32;
        let mut field = FieldParams {
            time: [
                phase,
                self.sea.choppiness as f32,
                dt.max(0.0) as f32,
                meander,
            ],
            wind: [wx as f32, wz as f32, n as f32, self.count as f32],
            ..Default::default()
        };
        let u = &mut self.uniforms;
        u.layout = [self.top as f32, self.count as f32, n as f32, share as f32];
        u.wind = [
            wx as f32,
            wz as f32,
            time as f32,
            group_cut(group_share(share)) as f32,
        ];
        for (c, breaking) in self.breaking.iter().enumerate() {
            let cascade = self.cascades[c];
            let decay = (crest.lifetime * breaking.period).max(1e-3);
            let layout = group_layout(breaking.period, cascade.size);
            u.groups[c] = [
                (1.0 / layout.length) as f32,
                (1.0 / layout.width) as f32,
                layout.drift as f32,
                (decay * BUBBLE_LIFE / (1.0 - BUBBLE_LIFE)) as f32,
            ];
            // Spreading along the texel axes from the wind-aligned rates: D·dt/Δx², held below the scheme's limit.
            let texel = cascade.size / f64::from(n);
            let gain = dt.max(0.0) / (texel * texel);
            let period = breaking.period;
            let wavelength = GRAVITY * period * period / (2.0 * PI);
            let along = if period > 0.0 {
                SPREAD_MOST.min(FOAM_SPREAD * wavelength * wavelength / period)
            } else {
                0.0
            };
            let across = along * ACROSS_SHARE;
            let (streaks, lines) = if c == STREAK_CASCADE {
                streak_params(cascade.size, n, params.wind_speed, wx, wz)
            } else {
                ([0.0; 4], [0.0; 4])
            };
            field.cascades[c] = CascadeParams {
                a: [
                    (2.0 * PI / cascade.size) as f32,
                    decay as f32,
                    breaking.threshold.min(NEVER) as f32,
                    (decay * BUBBLE_LIFE) as f32,
                ],
                b: [
                    breaking.compression as f32,
                    breaking.face as f32,
                    SPREAD_LIMIT.min(gain * (along * wx * wx + across * wz * wz)) as f32,
                    SPREAD_LIMIT.min(gain * (along * wz * wz + across * wx * wx)) as f32,
                ],
                c: streaks,
                d: lines,
            };
        }
        uploads.write_buffer(&self.field_params, 0, bytemuck::bytes_of(&field));
        uploads.write_buffer(&self.wave_uniforms, 0, bytemuck::bytes_of(&self.uniforms));
        uploads.write_buffer(&self.hull_sea_buffer, 0, bytemuck::bytes_of(&self.hull_sea));

        let next = 1 - self.current;
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ocean waves"),
            timestamp_writes: timestamps(),
        });
        pass.set_pipeline(&self.transform);
        for group in &self.passes {
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups((n * self.count).div_ceil(16), n.div_ceil(16), 1);
        }
        pass.set_pipeline(&self.resolve);
        pass.set_bind_group(0, &self.resolves[next], &[]);
        pass.dispatch_workgroups(n.div_ceil(8), n.div_ceil(8), self.count);
        pass.set_pipeline(&self.mips);
        for (level, group) in self.mip_groups[next].iter().enumerate() {
            let size = n >> (level + 1);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(size.div_ceil(8), size.div_ceil(8), self.count);
        }
        drop(pass);
        self.current = next;
        true
    }
}
