//! Spindrift and the spray haze (native only: the web's sea threw no spray of its own).
//!
//! From about Beaufort 7 the wind tears spray off breaking crests and drives it downwind in streaks and sheets; by a
//! strong gale it hangs low over the sea as a haze that whitens the far water and veils what stands in it. Both follow
//! the wind the ocean draws ([`spindrift_strength`], [`spray_haze`]), so calm weather draws exactly what it did.
//!
//! - **Spindrift** (`shaders/spindrift.wgsl`): wisps drawn by the ocean in the transparent phase, from the wave field's
//!   own breaking. No particle lives on the CPU: every wisp is a slot in a grid of cells laid on the world about the
//!   camera, [`LEVELS`] levels of [`GRID`]² cells, each level's cells and wisps twice the last's, over its own band of
//!   distance. Each life a slot takes a new point in its cell and throws spray only where the whitecaps' fresh foam
//!   shows there. The vertex stage reads the field and places the wisp; cells off screen, out of their band or on
//!   unbroken water are dropped before any fragment. Soft particles fade them into the sea and hulls (the backdrop's
//!   depth copy).
//! - **Haze** (`shaders/spray_haze.wgsl`): an extinction that falls off exponentially above the sea, integrated exactly
//!   along each view ray. The surface veils the water with it before its distance fog; the wisps take it too.

use bytemuck::{Pod, Zeroable};
use naval_render::{
    PipelineId, PipelineRegistry, PrepareContext, RenderContext, SCENE_BACKDROP_DEPTH,
    ScenePipeline, ShaderError,
};

use crate::{OceanParameters, shaders};

/// Cells per level edge (`GRID` in `spindrift.wgsl`).
pub const GRID: u32 = 48;
/// Wisps per cell (`SLOTS`).
pub const SLOTS: u32 = 3;
/// Levels of cells, each twice the last.
pub const LEVELS: usize = 4;
/// The finest level's cell (m) (`FINEST_CELL`).
pub const FINEST_CELL: f64 = 2.5;
/// A level draws out to this many of its cells from the camera (its grid reaches `GRID / 2`, less a cell of snapping).
const BAND_CELLS: f64 = 22.0;
/// Spindrift begins to be seen at Beaufort 7 (13.9–17.1 m/s) and is everywhere by a gale.
pub const SPINDRIFT_ONSET: f64 = 13.0;
const SPINDRIFT_ESTABLISHED: f64 = 17.5;
/// The wind (m/s) at which the spindrift has its reference strength 1; it grows with the square of the wind past it.
const SPINDRIFT_REFERENCE: f64 = 20.0;
/// The haze's extinction at the sea (1/m) at [`HAZE_REFERENCE`] m/s, doubling every [`HAZE_DOUBLING`] m/s: visibility in
/// it about 22 km at 20 m/s, 13 km at 22, 4 km at 28, 1.3 km at 32 (Beaufort 9 "spray may affect visibility", 10 and 11
/// "visibility affected", 12 "very seriously").
const HAZE_EXTINCTION: f64 = 1.8e-4;
const HAZE_REFERENCE: f64 = 20.0;
const HAZE_DOUBLING: f64 = 3.0;
/// The haze rises from nothing below this wind to its curve by [`HAZE_REFERENCE`].
const HAZE_ONSET: f64 = 17.0;
const HAZE_MOST: f64 = 0.01;
/// Its scale height: a few metres over the crests, thickening with the sea.
const HAZE_SCALE: f64 = 3.0;
const HAZE_SCALE_SEA: f64 = 0.4;

fn smoothstep(from: f64, to: f64, x: f64) -> f64 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The spindrift's strength at `wind` m/s: 0 below its onset, 1 at 20 m/s, growing with the square of the wind (the
/// opacity of a wisp takes it up to 1, the size of the wisps beyond).
pub fn spindrift_strength(wind: f64) -> f64 {
    if !wind.is_finite() {
        return 0.0;
    }
    smoothstep(SPINDRIFT_ONSET, SPINDRIFT_ESTABLISHED, wind) * (wind / SPINDRIFT_REFERENCE).powi(2)
}

/// The spray haze at `wind` m/s over a sea of `significant_height` m: extinction at its base (1/m), scale height (m)
/// and the base's height (the mean sea), as `spray_haze.wgsl` takes them. No haze below 17 m/s.
pub fn spray_haze(wind: f64, significant_height: f64) -> [f32; 4] {
    if !wind.is_finite() || wind <= HAZE_ONSET {
        return [0.0; 4];
    }
    let extinction = (HAZE_EXTINCTION
        * 2f64.powf((wind - HAZE_REFERENCE) / HAZE_DOUBLING)
        * smoothstep(HAZE_ONSET, HAZE_REFERENCE, wind))
    .min(HAZE_MOST);
    let scale = HAZE_SCALE + HAZE_SCALE_SEA * significant_height.max(0.0);
    [extinction as f32, scale as f32, 0.0, 0.0]
}

/// `Spray` in `spindrift.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct SprayUniforms {
    /// xy: unit vector the wind blows toward, z: wind speed (m/s), w: the sea's clock (s).
    pub wind: [f32; 4],
    /// x: the spindrift's strength, y: lightning's flash, z: the sea's significant height (m), w: the sea's water
    /// shadows (0 off … 3 high, as the surface's), by which the wisps take the ships' shadows.
    pub spray: [f32; 4],
    /// x: the haze's extinction at its base (1/m), y: scale height (m), z: base height (m).
    pub haze: [f32; 4],
    /// Per level: the corner of its first cell (x, z), the cell (m), the farthest distance it draws (m).
    pub levels: [[f32; 4]; LEVELS],
    /// The sea's fog: rgb colour, a power; then start, end, sky blend, on.
    pub fog_color: [f32; 4],
    pub fog: [f32; 4],
}

/// Each level's first cell, cell and band about a camera at `eye` (world x, z).
pub fn level_frames(eye: [f64; 2]) -> [[f32; 4]; LEVELS] {
    std::array::from_fn(|level| {
        let cell = FINEST_CELL * f64::from(1u32 << level);
        let half = f64::from(GRID / 2);
        let corner = eye.map(|v| ((v / cell).floor() - half) * cell);
        [
            corner[0] as f32,
            corner[1] as f32,
            cell as f32,
            (BAND_CELLS * cell) as f32,
        ]
    })
}

/// The wisps and their pipeline. The ocean holds one ([`crate::Ocean::spindrift`]), prepares it after the field and
/// draws it in the transparent phase.
pub struct Spindrift {
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    /// The clouds' shadow: map, sampler and uniforms (a disabled placeholder until set).
    cloud: (wgpu::TextureView, wgpu::Sampler, wgpu::Buffer),
    /// The ocean's foam texture and its repeating sampler, which shape the wisps.
    foam: (wgpu::TextureView, wgpu::Sampler),
    /// The bind group and the backdrop depth version it holds.
    group: Option<(u64, wgpu::BindGroup)>,
    pipeline: Option<PipelineId>,
    /// Lightning's flash this frame, a multiple of the sky's light (the effects' `flash`); the host sets it.
    pub flash: f32,
    /// This frame's strength (0: nothing drawn).
    strength: f32,
}

impl Spindrift {
    pub(crate) fn new(
        device: &wgpu::Device,
        cloud: (wgpu::TextureView, wgpu::Sampler, wgpu::Buffer),
        foam: (wgpu::TextureView, wgpu::Sampler),
    ) -> Self {
        let visibility = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean spindrift"),
            entries: &[
                uniform(0),
                naval_render::copies::depth_copy_layout_entry(1, wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform(4),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean spindrift"),
            size: std::mem::size_of::<SprayUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            layout,
            uniforms,
            cloud,
            foam,
            group: None,
            pipeline: None,
            flash: 0.0,
            strength: 0.0,
        }
    }

    /// Builds the pipeline on the renderer's registry (it hot-reloads): `waves` is the field's group layout, `environment`
    /// the sky's. The ocean's files must be registered first.
    pub(crate) fn register(
        &mut self,
        ctx: &mut RenderContext<'_>,
        waves: &wgpu::BindGroupLayout,
        environment: &wgpu::BindGroupLayout,
    ) -> Result<(), ShaderError> {
        self.pipeline = Some(ctx.add_scene_pipeline(
            "ocean spindrift",
            shaders::SPINDRIFT.path,
            ScenePipeline {
                bind_group_layouts: vec![self.layout.clone(), waves.clone(), environment.clone()],
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_write: false,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                ..ScenePipeline::new("spindrift_vertex", "spindrift_fragment")
            },
        )?);
        Ok(())
    }

    /// Shade the wisps' sunlight with the clouds' shadow (`naval_clouds::CloudShadow`), or none.
    pub(crate) fn set_cloud_shadow(
        &mut self,
        cloud: (wgpu::TextureView, wgpu::Sampler, wgpu::Buffer),
    ) {
        self.cloud = cloud;
        self.group = None;
    }

    /// Whether the wisps draw at `wind` m/s (and so need the backdrop's depth).
    pub fn active(&self, wind: f64) -> bool {
        self.pipeline.is_some() && spindrift_strength(wind) > 0.0
    }

    /// This frame's uniforms for the ocean's `parameters` at `time` (the field's clock) over a sea of
    /// `significant_height` with the surface's `water_shadows` (0 off … 3 high), and the bind group on the backdrop's
    /// depth.
    pub(crate) fn prepare(
        &mut self,
        ctx: &mut PrepareContext<'_>,
        parameters: &OceanParameters,
        time: f64,
        significant_height: f64,
        water_shadows: u32,
    ) {
        let waves = &parameters.waves;
        self.strength = if self.pipeline.is_some() {
            spindrift_strength(waves.wind_speed) as f32
        } else {
            0.0
        };
        if self.strength <= 0.0 {
            return;
        }
        let Some(depth) = ctx.resources.texture(SCENE_BACKDROP_DEPTH) else {
            self.strength = 0.0;
            return;
        };
        if self.group.as_ref().map(|(version, _)| *version) != Some(depth.version) {
            let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ocean spindrift"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&depth.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&self.cloud.0),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.cloud.1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: self.cloud.2.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&self.foam.0),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::Sampler(&self.foam.1),
                    },
                ],
            });
            self.group = Some((depth.version, group));
        }
        let (toward_z, toward_x) = waves.wind_direction.sin_cos();
        let eye = ctx.camera.position;
        let fog = &parameters.fog;
        let uniforms = SprayUniforms {
            wind: [
                toward_x as f32,
                toward_z as f32,
                waves.wind_speed as f32,
                time as f32,
            ],
            spray: [
                self.strength,
                self.flash,
                significant_height as f32,
                water_shadows.min(3) as f32,
            ],
            haze: spray_haze(waves.wind_speed, significant_height),
            levels: level_frames([f64::from(eye[0]), f64::from(eye[2])]),
            fog_color: [fog.color[0], fog.color[1], fog.color[2], fog.power],
            fog: [fog.start, fog.end, fog.sky_blend_distance, 1.0],
        };
        ctx.uploads
            .write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Draws the wisps in the transparent phase over the field's group (`field`) and the sky's (`environment`).
    pub(crate) fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        pipelines: &PipelineRegistry,
        field: &wgpu::BindGroup,
        environment: &wgpu::BindGroup,
    ) {
        let (Some(pipeline), Some((_, group))) = (self.pipeline, &self.group) else {
            return;
        };
        if self.strength <= 0.0 {
            return;
        }
        pass.set_pipeline(pipelines.render(pipeline));
        pass.set_bind_group(1, group, &[]);
        pass.set_bind_group(2, field, &[]);
        pass.set_bind_group(3, environment, &[]);
        pass.draw(0..4, 0..LEVELS as u32 * GRID * GRID * SLOTS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spray_starts_with_a_gale_and_grows_with_the_wind() {
        assert_eq!(spindrift_strength(9.0), 0.0);
        assert_eq!(spindrift_strength(13.0), 0.0);
        assert!(spindrift_strength(15.0) > 0.0 && spindrift_strength(15.0) < 0.5);
        assert!((spindrift_strength(20.0) - 1.0).abs() < 1e-12);
        assert!(spindrift_strength(28.0) > 1.9);
        assert_eq!(spray_haze(16.0, 5.0), [0.0; 4]);
        // Visibility (3.912 / extinction) in the haze at the sea.
        let visibility = |wind: f64| 3.912 / f64::from(spray_haze(wind, 7.0)[0]);
        assert!((10_000.0..16_000.0).contains(&visibility(22.0)));
        assert!((3_000.0..5_000.0).contains(&visibility(28.0)));
        assert!(visibility(32.0) < 1_500.0);
    }

    #[test]
    fn levels_cover_their_bands_about_the_camera() {
        let eye = [1234.5, -987.6];
        for (level, frame) in level_frames(eye).iter().enumerate() {
            let cell = f64::from(frame[2]);
            assert_eq!(cell, FINEST_CELL * f64::from(1u32 << level));
            // The grid's far edges lie beyond the band on every side.
            let reach = f64::from(GRID) * cell;
            for (axis, &start) in [frame[0], frame[1]].iter().enumerate() {
                let start = f64::from(start);
                assert!(eye[axis] - start >= f64::from(frame[3]));
                assert!(start + reach - eye[axis] >= f64::from(frame[3]));
            }
        }
    }
}
