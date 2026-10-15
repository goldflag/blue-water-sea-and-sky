//! The game's sky on wgpu, ported from `src/game/sky` (the game's own sky, which replaced Sky Pro)
//! without clouds and weather: the atmosphere's scattering tables, the dome at the far plane, the
//! sun, moon and stars, and the environment lighting ships and the sea read.
//!
//! See `native/docs/sky.md` for the interface other lanes consume and what matches the web.

pub mod aerial;
mod cover;
pub mod diagnostics;
pub mod display;
pub mod environment;
mod gpu;
mod half;
pub mod haze;
pub mod layer;
pub mod model;
pub mod shaders;
mod sky;
pub mod sky_mean;
pub mod stars;
pub mod state;
mod tables;
mod uniforms;

pub use cover::{CloudCover, CloudCoverFrame};
pub use environment::{
    BoltLight, CelestialLight, CubeUvLayout, CubeUvLevel, ENVIRONMENT_FORMAT, ENVIRONMENT_WGSL,
    EnvironmentMap, EnvironmentUniforms, LightningLight, add_environment_module,
    environment_bind_group_layout,
};
pub use layer::SkyLayer;
pub use shaders::add_sky_modules;
pub use sky::{CloudHooks, SeaLevel, Sky, SkyPipelines, SkyQuality, SkyTargets, SkyView};
pub use sky_mean::SkyMeanLayer;
pub use state::{
    CloudGenus, CloudScene, DEFAULT_SKY_SCENE, HighCloudGenus, HighClouds, SkyScene, WeatherScene,
};
pub use uniforms::SkyUniforms;

#[cfg(test)]
mod tests {
    use naga_oil::compose::{Composer, NagaModuleDescriptor, ShaderType};

    /// A consumer shader imports the environment module and samples it, as the ships and ocean lanes will.
    #[test]
    fn environment_module_composes_into_a_consumer() {
        let mut composer = Composer::default();
        super::add_environment_module(&mut composer).expect("environment module");
        let consumer = r#"
#import naval_sky::environment::{SkyEnvironment, env_radiance, env_irradiance, env_reflection_direction}

@group(2) @binding(0) var sky_atlas: texture_2d<f32>;
@group(2) @binding(1) var sky_sampler: sampler;
@group(2) @binding(2) var<uniform> sky_env: SkyEnvironment;

@fragment
fn main(@location(0) normal: vec3<f32>, @location(1) view: vec3<f32>) -> @location(0) vec4<f32> {
    let n = normalize(normal);
    let r = env_reflection_direction(n, normalize(view), 0.4);
    let specular = env_radiance(sky_atlas, sky_sampler, sky_env, r, 0.4);
    let diffuse = env_irradiance(sky_atlas, sky_sampler, sky_env, n) / 3.14159265;
    return vec4<f32>(specular + diffuse * 0.5, 1.0);
}
"#;
        let module = composer
            .make_naga_module(NagaModuleDescriptor {
                source: consumer,
                file_path: "consumer.wgsl",
                shader_type: ShaderType::Wgsl,
                ..Default::default()
            })
            .expect("consumer composes");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("valid module");
    }
}
