//! The game's clouds and weather on wgpu. The clouds are a port of @takram/three-clouds
//! (https://github.com/takram-design-engineering/three-geospatial, MIT): four layers over the curved planet with
//! takram's weather, shape and detail textures, a ray march with Beer shadow maps, temporal upscaling, light shafts and
//! haze, lit through the sky's atmosphere. The weather is the game's own, ported from `src/game/sky/weather`: near
//! rain, splashes, lightning and heavy rain's veil.
//!
//! [`GameSky`] is the removed web game's `Sky.ts` facade: the sky, the clouds and the weather together, as one
//! `naval_render::SceneLayer`. See `native/docs/clouds.md` for what the clouds are, what they cost and the interfaces
//! other lanes read, and `THIRD_PARTY.md` for the notices.

pub mod aerial;
pub mod bolt;
pub mod cascades;
pub mod detail_noise;
pub mod game_sky;
mod gpu;
pub mod key_light;
pub mod layer;
pub mod layers;
pub mod lightning;
pub mod post;
pub mod quality;
pub mod shaders;
pub mod textures;
pub mod weather;
pub mod weathers;

pub use game_sky::GameSky;
pub use layer::{
    CloudClearing, CloudLayer, CloudPipelines, CloudShadow, CloudShadowUniforms, CloudUniforms,
    CloudView, HOLD_UPDATES,
};

/// The formats of the scene pass the clouds and weather draw into.
#[derive(Clone, Copy, Debug)]
pub struct Targets {
    pub color: wgpu::TextureFormat,
    /// The scene's reversed-Z depth, or none for a pass without depth.
    pub depth: Option<wgpu::TextureFormat>,
}

impl Default for Targets {
    /// The renderer core's scene pass: `HDR_FORMAT` with reversed `DEPTH_FORMAT`.
    fn default() -> Self {
        Self {
            color: naval_render::HDR_FORMAT,
            depth: Some(naval_render::DEPTH_FORMAT),
        }
    }
}

/// GLSL's `smoothstep(edge0, edge1, x)`.
pub fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
