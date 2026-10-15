//! The game's ocean on wgpu, ported from `src/game/ocean` (see its README for the design every piece follows).
//!
//! - CPU: the seeded spectrum normalised to Hm0 ([`spectrum`]), the realistic sea state ([`sea_state`]), whitecap
//!   statistics and thresholds ([`whitecaps`]), breaking groups ([`groups`]), gusts ([`gusts`]), the calibrated seas
//!   and map water the game sets per scene ([`calibration`]), and the foam texture ([`foam_texture`]).
//! - GPU: spectrum evolution, inverse FFT, foam persistence and mip chains as WGSL compute ([`field`]); the clipmap and
//!   horizon ring ([`geometry`]); the surface's vertex displacement and shading ([`surface`]); the sky environment the
//!   sea reflects, in the sky lane's interface ([`environment`]); and a stand-in sky backdrop for stills
//!   ([`backdrop`]). [`Ocean`] holds the field and the surface behind one facade, as `Ocean.ts` does, and is a
//!   `naval_render::SceneLayer`: the renderer core prepares it (the field's compute) and draws it in its opaque phase.
//!
//! - Wakes, ported from the web game's `ShipWake.ts` and what it drives: the dispersive wake field ([`wake`]), the fleet's trail
//!   foam and slick ([`trail`]) and torpedo tracks ([`torpedo_tracks`]) painted on the GPU ([`stamps`]), the analytic bow
//!   waves ([`bow_waves`]) and the hull contact foam ([`hull_foam`], from a hull's [`hull_profile`]), behind
//!   [`ship_wake::ShipWake`], which the surface shades through `shaders/wake.wgsl`.
//!
//! - The sea's height about each coupled hull ([`hull_heights`]), evaluated once a frame for the hulls' wet band, which
//!   cannot bind the wave field itself.
//!
//! - Lakes and ponds at their own levels on a theatre window's land ([`lakes`]): flat water with the sea's optics, a
//!   layer of its own beside the sea in the `Water` phase.
//!
//! The GPU ocean is visual only: the simulation owns ship poses and never reads these waves.

pub mod backdrop;
pub mod bow_waves;
pub mod calibration;
pub mod caustics;
pub mod environment;
pub mod field;
pub mod foam_texture;
pub mod geometry;
pub mod groups;
pub mod gusts;
pub mod hull_foam;
pub mod hull_heights;
pub mod hull_profile;
pub mod hull_sea;
pub mod hull_slice;
pub mod lakes;
pub mod ocean;
pub mod optics;
pub mod rain_heights;
pub mod reflection_tiles;
mod sea_read;
pub mod sea_state;
pub mod shaders;
pub mod ship_wake;
pub mod shore;
pub mod spectrum;
pub mod spindrift;
pub mod stamps;
pub mod surface;
pub mod torpedo_tracks;
pub mod trail;
pub mod underwater;
pub mod wake;
pub mod whitecaps;

pub use field::WaveField;
pub use ocean::Ocean;
pub use spectrum::WaveParameters;
pub use surface::{OceanSurface, ScreenReflections};

/// The HDR scene format every pass here renders into (the renderer core's).
pub const HDR_FORMAT: wgpu::TextureFormat = naval_render::HDR_FORMAT;
/// Reversed floating-point depth: cleared to 0, compared `GreaterEqual` (the renderer core's).
pub const DEPTH_FORMAT: wgpu::TextureFormat = naval_render::DEPTH_FORMAT;

/// One wave cascade: a periodic tile `size` metres across, transformed at `resolution`² texels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CascadeInfo {
    pub size: f64,
    pub resolution: u32,
}

/// Graphics → Ocean (the web game's `ocean/quality.ts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QualityTier {
    Low,
    Medium,
    High,
    Ultra,
}

impl QualityTier {
    /// Largest tile first: 1,024 / 181 / 31 m (Low takes the first, Medium two).
    pub fn cascades(self) -> Vec<CascadeInfo> {
        let (count, resolution) = match self {
            Self::Low => (1, 256),
            Self::Medium => (2, 256),
            Self::High => (3, 256),
            Self::Ultra => (3, 512),
        };
        [1024.0, 181.0, 31.0][..count]
            .iter()
            .map(|&size| CascadeInfo { size, resolution })
            .collect()
    }

    /// Screen-space reflection ray steps (`reflectionSteps` in `quality.ts`): none on Low and Medium, where scene
    /// reflections are unavailable.
    pub fn reflection_steps(self) -> u32 {
        match self {
            Self::Low | Self::Medium => 0,
            Self::High => 16,
            Self::Ultra => 32,
        }
    }

    /// The water mesh ([`geometry::Clipmap`]): cells per level edge and levels, so the finest cells are 8 m, 2 m,
    /// 25 cm and 12.5 cm (the web's tiers drew 16, 8, 4 and 2 m). An eye near the water draws them; from higher the
    /// mesh starts further out ([`geometry::Clipmap::detail`]).
    pub fn clipmap(self) -> geometry::Clipmap {
        match self {
            Self::Low => geometry::Clipmap::new(32, 6),
            Self::Medium => geometry::Clipmap::new(64, 7),
            Self::High => geometry::Clipmap::new(128, 9),
            Self::Ultra => geometry::Clipmap::new(256, 9),
        }
    }
}

/// Custom water colours, linear RGB (`WaterColors`). The physical water body reads only the absorption.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterColors {
    /// Colour of deep water looked into (the pigment, used without physical water colour and by the underside).
    pub water: [f32; 3],
    /// Tint of light passing through thin water (wave crests).
    pub transmission: [f32; 3],
    /// Absorption coefficients per metre.
    pub absorption: [f32; 3],
}

/// Whitecaps and wake foam (`CrestFoamParameters`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CrestFoam {
    /// Scale on the coverage the wind calls for: 1 is calibrated.
    pub coverage_scale: f64,
    /// e-folding lifetime of whitecap foam in periods of the waves that broke.
    pub lifetime: f64,
    pub color: [f32; 3],
    pub opacity: f32,
    /// 0 round patches … 1 streaks drawn out along the wind.
    pub wind_stretch: f32,
}

/// Windrows (`SurfaceFoamParameters`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceFoam {
    pub color: [f32; 3],
    pub opacity: f32,
    /// Share of the sea covered.
    pub coverage: f64,
}

/// Foam where the water column thins (`ShorelineFoamParameters`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShorelineFoam {
    pub color: [f32; 3],
    pub opacity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FoamParameters {
    pub crest: CrestFoam,
    pub surface: SurfaceFoam,
    pub shoreline: ShorelineFoam,
}

/// Distance fog (`OceanFogParameters`, the gameplay ramp; real extinction is off in the game by default).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FogParameters {
    /// Linear RGB.
    pub color: [f32; 3],
    pub start: f32,
    pub end: f32,
    pub power: f32,
    /// Distance over which the fog colour turns into the sky behind it.
    pub sky_blend_distance: f32,
}

impl From<FogParameters> for naval_render::SceneFog {
    /// The sea's fog as every other material takes it (`scene.fogNode`): ships, aircraft and effects fog with the
    /// same ramp and colour, on.
    fn from(fog: FogParameters) -> Self {
        Self {
            color: fog.color,
            start: fog.start,
            end: fog.end,
            power: fog.power,
            sky_blend_distance: fog.sky_blend_distance,
            enabled: true,
        }
    }
}

/// Everything the game sets on the ocean for a scene, live.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OceanParameters {
    pub waves: WaveParameters,
    pub gusts: gusts::GustParameters,
    pub colors: WaterColors,
    /// Geographic optical coefficients. Disabled by default for the authored developer map fixtures.
    pub water: optics::WaterField,
    pub foam: FoamParameters,
    pub fog: FogParameters,
    /// `realism.seaState`: wavelengths follow a real wind sea's steepness.
    pub sea_state: bool,
    /// `realism.gusts`.
    pub gusts_enabled: bool,
}
