//! The sky environment the sea reflects, in the sky lane's published interface (`native/docs/sky.md` on
//! `goldflag/native-sky`): one `Rgba16Float` texture in three's PMREM (CubeUV) atlas layout, sampled through
//! `naval_sky::environment` (`env_radiance`, `env_irradiance`), bound as texture, sampler and the 80-byte
//! `SkyEnvironment` uniform at bindings 0–2 of one group. The ocean reads the active celestial light from that uniform.
//!
//! naval-sky is not merged, so this module mirrors its types and bind group layout and adds a stand-in filler: an
//! equirectangular radiance map (the web game's own sky bake, captured by `web/ocean-stills.ts`, or a gradient) is
//! prefiltered into the atlas by `shaders/environment_fill.wgsl`, one Gaussian per level at the sigmas three's
//! PMREMGenerator uses. The vertical slice replaces [`StandInEnvironment`] with `naval_sky::EnvironmentMap`, whose
//! bind group has the same layout.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

use crate::shaders;

/// The atlas's texel format.
pub const ENVIRONMENT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Three's smallest sharp level: levels below it are extra blurs at 16².
pub const LOD_MIN: u32 = 4;
/// Standard deviations (radians) of three's six extra levels.
pub const EXTRA_LOD_SIGMA: [f32; 6] = [0.125, 0.215, 0.35, 0.446, 0.526, 0.582];

/// `CubeUvLayout` of naval-sky: where each level of the prefiltered radiance sits in the atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CubeUvLayout {
    pub face_size: u32,
    pub max_mip: u32,
    pub width: u32,
    pub height: u32,
}

/// One level of the atlas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubeUvLevel {
    pub size: u32,
    pub sigma: f32,
    pub x: u32,
    pub y: u32,
}

impl CubeUvLayout {
    /// The layout three builds from an equirectangular bake `bake_width` wide.
    pub fn from_bake_width(bake_width: u32) -> Self {
        Self::from_max_mip((bake_width / 4).max(16).ilog2())
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

    /// Every level, sharpest first.
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
            levels.push(CubeUvLevel {
                size,
                sigma,
                x,
                y: 4 * (self.face_size - size),
            });
            if lod > LOD_MIN {
                lod -= 1;
            }
        }
        levels
    }
}

/// The light of the sun or moon on the scene (`CelestialLight`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CelestialLight {
    /// Unit world vector toward the active body.
    pub direction: Vec3,
    /// Linear RGB tint at the sea, brightest channel 1.
    pub color: Vec3,
    /// Radiance scale in the sea's units (about 6 at noon).
    pub intensity: f32,
    pub night: bool,
}

/// `SkyEnvironment` in `sky_environment.wgsl` (naval-sky's `EnvironmentUniforms`): 80 bytes.
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
    pub fn new(layout: &CubeUvLayout, light: &CelestialLight) -> Self {
        Self {
            texel_size: [1.0 / layout.width as f32, 1.0 / layout.height as f32],
            max_mip: layout.max_mip as f32,
            generation: 0.0,
            light_direction: light.direction.to_array(),
            light_intensity: light.intensity,
            light_color: light.color.to_array(),
            night: if light.night { 1.0 } else { 0.0 },
            sun_direction: light.direction.to_array(),
            moon_phase: 0.5,
            moon_direction: [0.0, -1.0, 0.0],
            moon_illumination: 1.0,
        }
    }
}

/// naval-sky's `environment_bind_group_layout`: 0 the atlas, 1 its sampler, 2 the uniforms.
pub fn environment_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let visibility =
        wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE;
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sky environment"),
        entries: &[
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
                        std::mem::size_of::<EnvironmentUniforms>() as u64,
                    ),
                },
                count: None,
            },
        ],
    })
}

impl Equirect<'_> {
    /// Bilinear radiance along a world direction (azimuth wraps, elevation clamps).
    pub fn sample(&self, direction: Vec3) -> Vec3 {
        use std::f32::consts::PI;
        let d = direction.normalize();
        let u = (d.z.atan2(d.x) / (2.0 * PI) + 0.5) * self.width as f32 - 0.5;
        let v = ((d.y.clamp(-1.0, 1.0).asin() / PI + 0.5) * self.height as f32 - 0.5)
            .clamp(0.0, self.height as f32 - 1.0);
        let (x0, y0) = (u.floor(), v.floor());
        let (fx, fy) = (u - x0, v - y0);
        let texel = |x: f32, y: f32| {
            let x = (x as i64).rem_euclid(i64::from(self.width)) as usize;
            let y = (y as i64).clamp(0, i64::from(self.height) - 1) as usize;
            let i = (y * self.width as usize + x) * 4;
            Vec3::new(self.rgba[i], self.rgba[i + 1], self.rgba[i + 2])
        };
        let top = texel(x0, y0).lerp(texel(x0 + 1.0, y0), fx);
        let bottom = texel(x0, y0 + 1.0).lerp(texel(x0 + 1.0, y0 + 1.0), fx);
        top.lerp(bottom, fy)
    }
}

/// An equirectangular radiance map: rows from the nadir up, `width` × `height` RGBA floats, azimuth
/// (u − ½)·2π from +X toward +Z, elevation (v − ½)·π (the web sky's bake layout).
pub struct Equirect<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [f32],
}

/// A gradient sky for tests and stills without a captured bake: zenith and horizon colours above, the sea's dark
/// mix below (the web bake's layout).
pub fn gradient_sky(width: u32, height: u32, zenith: Vec3, horizon: Vec3, below: Vec3) -> Vec<f32> {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let elevation = ((row as f32 + 0.5) / height as f32 - 0.5) * std::f32::consts::PI;
        let up = elevation.sin();
        let color = if up >= 0.0 {
            horizon.lerp(zenith, up.powf(0.45))
        } else {
            below.lerp(horizon, (1.0 + up * 12.0).max(0.0))
        };
        for _ in 0..width {
            rgba.extend([color.x, color.y, color.z, 1.0]);
        }
    }
    rgba
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct FillParams {
    origin: [u32; 2],
    size: u32,
    samples: u32,
    sigma: f32,
    _pad: [f32; 3],
}

/// The stand-in environment: the sky lane's atlas and bind group, filled from an equirectangular map.
pub struct StandInEnvironment {
    pub layout: CubeUvLayout,
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    pub uniforms: wgpu::Buffer,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

fn f16_bits(value: f32) -> u16 {
    half::f16::from_f32(value).to_bits()
}

impl StandInEnvironment {
    /// Fill an atlas of `layout` from `sky`, lit by `light`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: CubeUvLayout,
        sky: &Equirect,
        light: &CelestialLight,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stand-in sky environment atlas"),
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
        queue.write_buffer(
            &uniforms,
            0,
            bytemuck::bytes_of(&EnvironmentUniforms::new(&layout, light)),
        );
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
        let environment = Self {
            layout,
            texture,
            view,
            sampler,
            uniforms,
            bind_group_layout,
            bind_group,
        };
        environment.fill(device, queue, sky);
        environment
    }

    /// Update the celestial light the uniforms carry.
    pub fn set_light(&self, queue: &wgpu::Queue, light: &CelestialLight) {
        queue.write_buffer(
            &self.uniforms,
            0,
            bytemuck::bytes_of(&EnvironmentUniforms::new(&self.layout, light)),
        );
    }

    /// Prefilter `sky` into every level of the atlas.
    pub fn fill(&self, device: &wgpu::Device, queue: &wgpu::Queue, sky: &Equirect) {
        assert_eq!(
            sky.rgba.len(),
            (sky.width * sky.height * 4) as usize,
            "equirect size"
        );
        let source = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stand-in sky equirect"),
            size: wgpu::Extent3d {
                width: sky.width,
                height: sky.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let halves: Vec<u16> = sky.rgba.iter().map(|&v| f16_bits(v)).collect();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &source,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&halves),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(sky.width * 8),
                rows_per_image: Some(sky.height),
            },
            wgpu::Extent3d {
                width: sky.width,
                height: sky.height,
                depth_or_array_layers: 1,
            },
        );
        let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("stand-in sky equirect"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("environment fill"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: ENVIRONMENT_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let module = shaders::shader_module(device, shaders::ENVIRONMENT_FILL, &[]);
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("environment fill"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("environment fill"),
                    bind_group_layouts: &[Some(&layout)],
                    immediate_size: 0,
                }),
            ),
            module: &module,
            entry_point: Some("fill"),
            compilation_options: Default::default(),
            cache: None,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("environment fill"),
        });
        let mut keep = Vec::new();
        for level in self.layout.levels() {
            let params = FillParams {
                origin: [level.x, level.y],
                size: level.size,
                samples: if level.sigma == 0.0 {
                    1
                } else if level.sigma < 0.1 {
                    64
                } else {
                    1024
                },
                sigma: level.sigma,
                _pad: [0.0; 3],
            };
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("environment fill level"),
                size: std::mem::size_of::<FillParams>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&params));
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("environment fill level"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&source_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&self.view),
                    },
                ],
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("environment fill level"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(
                (level.size * 3).div_ceil(8),
                (level.size * 2).div_ceil(8),
                1,
            );
            drop(pass);
            keep.push((buffer, group));
        }
        queue.submit([encoder.finish()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_the_sky_lane() {
        let high = CubeUvLayout::from_bake_width(512);
        assert_eq!((high.width, high.height), (384, 512));
        let levels = high.levels();
        assert_eq!(levels.len(), 10);
        assert_eq!((levels[4].x, levels[4].y), (48, 448));
        // The web's High bake is 384 wide: faces of 64.
        assert_eq!(CubeUvLayout::from_bake_width(384).face_size, 64);
        assert_eq!(std::mem::size_of::<EnvironmentUniforms>(), 80);
    }

    #[test]
    fn half_floats_round_trip() {
        assert_eq!(f16_bits(1.0), 0x3c00);
        assert_eq!(f16_bits(-2.0), 0xc000);
        assert_eq!(f16_bits(0.0), 0);
        assert_eq!(f16_bits(65504.0), 0x7bff);
        assert_eq!(f16_bits(1e6), 0x7c00);
    }
}
