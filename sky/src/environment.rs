//! The environment lighting the sky publishes: a prefiltered radiance atlas in three's PMREM (CubeUV)
//! layout, the uniforms that go with it (atlas size, the active celestial light), and the WGSL that
//! samples it. Ships and the sea read only this; see `native/docs/sky.md`, "Environment lighting".
//!
//! Ports the output side of the web game's `sky/environment/Environment.ts`.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

/// WGSL of the consumer side (`shaders/environment.wgsl`), import path `naval_sky::environment`.
pub const ENVIRONMENT_WGSL: &str = include_str!("../shaders/environment.wgsl");

/// The atlas's texel format: linear HDR, filterable.
pub const ENVIRONMENT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Three's smallest sharp level (`LOD_MIN`): levels below it are extra blurs at this size.
pub const LOD_MIN: u32 = 4;
/// Standard deviations (radians) of three's six extra levels, the roughest last (`EXTRA_LOD_SIGMA`).
pub const EXTRA_LOD_SIGMA: [f32; 6] = [0.125, 0.215, 0.35, 0.446, 0.526, 0.582];

/// Adds `naval_sky::environment` to a naga_oil composer, so a shader can
/// `#import naval_sky::environment::{SkyEnvironment, env_radiance, env_irradiance}`.
pub fn add_environment_module(
    composer: &mut naga_oil::compose::Composer,
) -> Result<(), Box<naga_oil::compose::ComposerError>> {
    composer.add_composable_module(naga_oil::compose::ComposableModuleDescriptor {
        source: ENVIRONMENT_WGSL,
        file_path: "naval-sky/shaders/environment.wgsl",
        language: naga_oil::compose::ShaderLanguage::Wgsl,
        ..Default::default()
    })?;
    Ok(())
}

/// Where each level of the prefiltered radiance sits in the atlas (three's `PMREMGenerator` layout).
///
/// Level 0 is the unblurred sky at `face_size`² per face; each sharp level halves the face down to
/// 16²; then six extra levels at 16² hold wider Gaussian blurs ([`EXTRA_LOD_SIGMA`]). A level is six
/// faces, three across and two down (+X +Y +Z over −X −Y −Z), each with a one-texel border that holds
/// directions just past its edge, so bilinear filtering never bleeds between faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CubeUvLayout {
    /// Face size of level 0, a power of two (64 on Low/Medium, 128 on High/Ultra).
    pub face_size: u32,
    /// log2 of `face_size` (three's `CUBEUV_MAX_MIP`, `_lodMax`).
    pub max_mip: u32,
    pub width: u32,
    pub height: u32,
}

/// One level of the atlas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubeUvLevel {
    /// Face size in texels.
    pub size: u32,
    /// The level's total Gaussian blur (radians) over the unblurred sky: 1 / size for sharp levels
    /// (0 for level 0), [`EXTRA_LOD_SIGMA`] for the extra ones.
    pub sigma: f32,
    /// Top-left texel of the level's 3 × 2 block of faces.
    pub x: u32,
    pub y: u32,
}

impl CubeUvLayout {
    /// The layout three builds from an equirectangular bake `bake_width` wide: faces of
    /// `2^floor(log2(bake_width / 4))`.
    pub fn from_bake_width(bake_width: u32) -> Self {
        let max_mip = (bake_width / 4).max(16).ilog2();
        Self::from_max_mip(max_mip)
    }

    pub fn from_max_mip(max_mip: u32) -> Self {
        let face_size = 1 << max_mip;
        Self {
            face_size,
            max_mip,
            width: 3 * face_size.max(16 * 7),
            height: 4 * face_size,
        }
    }

    /// Every level, sharpest first (three's `_createPlanes`).
    pub fn levels(&self) -> Vec<CubeUvLevel> {
        let sharp = self.max_mip - LOD_MIN + 1;
        let total = sharp + EXTRA_LOD_SIGMA.len() as u32;
        let mut lod = self.max_mip;
        let mut levels = Vec::with_capacity(total as usize);
        for i in 0..total {
            let size = 1u32 << lod;
            let sigma = if i >= sharp {
                EXTRA_LOD_SIGMA[(i - sharp) as usize]
            } else if i == 0 {
                0.0
            } else {
                1.0 / size as f32
            };
            let x = if i >= sharp {
                3 * size * (i - sharp + 1)
            } else {
                0
            };
            let y = 4 * (self.face_size - size);
            levels.push(CubeUvLevel { size, sigma, x, y });
            if lod > LOD_MIN {
                lod -= 1;
            }
        }
        levels
    }
}

/// The light of the sun or moon on the scene this frame (`CelestialLight` in the web game's `sky/contracts.ts`):
/// what the scene's directional light, the sea's glints and effects share.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CelestialLight {
    /// Unit world vector toward the active body: the sun by day, the moon once it outshines the sun.
    pub direction: Vec3,
    /// Linear RGB tint at the sea, brightest channel 1.
    pub color: Vec3,
    /// Radiance scale in the sea's units (the brightest channel; about 6 at noon).
    pub intensity: f32,
    /// True when the moon is the active body.
    pub night: bool,
}

/// Lightning's light on the scene this frame (the web's `CelestialLight.flash` and `.bolt`, kept beside
/// [`CelestialLight`] so its four fields stay as other crates build them): the flash's extra diffuse light, a
/// multiple of the scene's ambient (0 = none), and its direct light, which lit meshes take from its bearing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LightningLight {
    pub flash: f32,
    pub bolt: BoltLight,
}

/// Lightning's direct light this frame (`BoltLight`): where it shines from and its radiant intensity, the air
/// between included: irradiance `intensity × (1 km / r)²` in the sea's units. 0 while dark.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoltLight {
    pub position: Vec3,
    pub intensity: f32,
}

impl CelestialLight {
    /// Colour × intensity: what a directional light's radiance is.
    pub fn radiance(&self) -> Vec3 {
        self.color * self.intensity
    }
}

/// `SkyEnvironment` in `environment.wgsl`: the atlas's size and the active celestial light.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct EnvironmentUniforms {
    pub texel_size: [f32; 2],
    pub max_mip: f32,
    pub generation: f32,
    pub light_direction: [f32; 3],
    pub light_intensity: f32,
    pub light_color: [f32; 3],
    pub night: f32,
    pub sun_direction: [f32; 3],
    pub moon_phase: f32,
    pub moon_direction: [f32; 3],
    pub moon_illumination: f32,
}

impl EnvironmentUniforms {
    pub fn new(
        layout: &CubeUvLayout,
        generation: u64,
        light: &CelestialLight,
        sun: Vec3,
        moon: Vec3,
        moon_phase: f32,
    ) -> Self {
        Self {
            texel_size: [1.0 / layout.width as f32, 1.0 / layout.height as f32],
            max_mip: layout.max_mip as f32,
            generation: (generation % (1 << 24)) as f32,
            light_direction: light.direction.to_array(),
            light_intensity: light.intensity,
            light_color: light.color.to_array(),
            night: if light.night { 1.0 } else { 0.0 },
            sun_direction: sun.to_array(),
            moon_phase,
            moon_direction: moon.to_array(),
            moon_illumination: (1.0 - (std::f32::consts::TAU * moon_phase).cos()) / 2.0,
        }
    }
}

/// A ready bind group layout for the environment: binding 0 the atlas (`texture_2d<f32>`), 1 its
/// linear clamp sampler, 2 the [`EnvironmentUniforms`] (`var<uniform> SkyEnvironment`). Visible to vertex, fragment
/// and compute stages. Put it at any group index. (Binding 3, the far fog's colour map, went with the fog ramp: the
/// air between the eye and a surface is the aerial perspective, `naval_render::aerial`.)
pub fn environment_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let visibility =
        wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE;
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sky environment"),
        entries: &atlas_entries(visibility),
    })
}

/// The environment's atlas, sampler and uniforms (bindings 0–2).
fn atlas_entries(visibility: wgpu::ShaderStages) -> [wgpu::BindGroupLayoutEntry; 3] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(
                    std::mem::size_of::<EnvironmentUniforms>() as u64
                ),
            },
            count: None,
        },
    ]
}

/// The environment the sky owns and refreshes in place: the prefiltered radiance atlas, its sampler,
/// the uniforms buffer and a bind group in [`environment_bind_group_layout`]. The texture object stays
/// the same until the quality tier changes the atlas size (then `generation` restarts nothing, but
/// `layout` changes and consumers must rebuild bind groups that hold the old view).
pub struct EnvironmentMap {
    pub layout: CubeUvLayout,
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    pub uniforms: wgpu::Buffer,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
    /// Counts refilters; bumps whenever the atlas's contents change.
    pub generation: u64,
}

impl EnvironmentMap {
    pub fn new(device: &wgpu::Device, layout: CubeUvLayout) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sky environment atlas"),
            size: wgpu::Extent3d {
                width: layout.width,
                height: layout.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ENVIRONMENT_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sky environment"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky environment uniforms"),
            size: std::mem::size_of::<EnvironmentUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = environment_bind_group_layout(device);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky environment"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniforms.as_entire_binding(),
                },
            ],
        });
        Self {
            layout,
            texture,
            view,
            sampler,
            uniforms,
            bind_group_layout,
            bind_group,
            generation: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_three_for_every_tier() {
        // Bake widths of Low, Medium, High, Ultra (the web game's `sky/quality.ts`).
        let faces: Vec<u32> = [256, 384, 512, 768]
            .iter()
            .map(|&w| CubeUvLayout::from_bake_width(w).face_size)
            .collect();
        assert_eq!(faces, [64, 64, 128, 128]);
        let high = CubeUvLayout::from_bake_width(512);
        assert_eq!((high.width, high.height), (384, 512));
        let levels = high.levels();
        assert_eq!(levels.len(), 4 + 6);
        assert_eq!(levels[0].sigma, 0.0);
        assert_eq!((levels[3].size, levels[3].y), (16, 448));
        assert_eq!((levels[4].x, levels[4].y), (48, 448));
        let last = levels.last().unwrap();
        assert!(last.x + 3 * last.size <= high.width);
        assert_eq!(last.sigma, 0.582);
    }

    #[test]
    fn uniforms_are_80_bytes() {
        assert_eq!(std::mem::size_of::<EnvironmentUniforms>(), 80);
    }
}
