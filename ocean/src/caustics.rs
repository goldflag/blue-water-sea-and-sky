//! Caustics under the sea (native only): the sunlight the waves gather and thin on what stands below the surface.
//!
//! A hull below the waterline was lit as in air; under water the sun reaches it through the waves, which focus it into
//! the moving net of light every photograph of a hull from below shows, sharp and bright a couple of metres down and
//! soft and faint by thirty. It tells depth and shows a hull's form, so the ships read it on their submerged plating
//! (naval-ships' `ship.wgsl`). The ships' pipeline cannot read the wave field (see [`crate::hull_heights`]), so the
//! ocean evaluates the light here, once a frame while the camera is under the sea or within [`CEILING`] of it
//! (`shaders/caustics.wgsl`): the gain 1 / |det(I + s·H)| of the light a patch of sea sends to a depth, a layer per
//! depth ([`depth`]: 2, 5, 12.5 and 31 m), each a square about the camera laid on the world so it repeats
//! ([`span`]: 32 m for the first, each twice the one before), so the net two metres down is drawn from the shortest
//! waves the field holds and the one thirty metres down from the swell. The host hands [`Caustics::view`] to the ships
//! (`ShipRenderer::set_sea_caustics`), which read two layers at `world xz / span` with a repeating sampler and blend
//! them for the fragment's depth ([`level_of`]).

use bytemuck::{Pod, Zeroable};
use naval_render::{
    CameraUniforms, MakePipelines, PipelineId, PipelineRegistry, RenderContext, ShaderError,
    compute_recipe,
};

use crate::sea_read::SeaRead;
use crate::shaders;
use crate::ship_wake::WakeBindings;

/// The first level's square (m), beyond which its net repeats; each further level's is twice the one before.
pub const SPAN: f32 = 32.0;
/// Texels per edge of every level: an eighth of a metre each on the first, the net's threads a couple of metres down.
pub const TEXELS: u32 = 256;
/// Levels: one per depth, each a layer.
pub const LEVELS: u32 = 4;
/// The first level's depth (m), and the ratio of each level's to the one before.
pub const DEPTH_NEAR: f32 = 2.0;
pub const DEPTH_RATIO: f32 = 2.5;
/// The blur of a net of light per metre of depth (m): the ripples the field does not hold and the water's scattering
/// spread each ray, so a level's slopes are differenced no finer than this share of its depth.
pub const BLUR: f32 = 0.06;
/// Levels shallower than this (m) leave out the first cascade when there are others: its waves, tens of metres long,
/// curve the surface too little to gather light that near under it.
pub const LONG_WAVES_FROM: f32 = 10.0;
/// The brightest a focus is drawn, in flat-sea sunlight: where rays cross, the gain is unbounded.
pub const BRIGHTEST: f32 = 4.0;
/// Above this height over the mean sea (m) the pass does not run: from there the sea's own surface hides the net, and
/// most of a battle is watched from higher. The readers fade the net out between [`FADE_FROM`] and it.
pub const CEILING: f32 = 25.0;
pub const FADE_FROM: f32 = 12.0;
/// The caustics' format: the gain in red, filterable.
pub const CAUSTICS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The edge (m) of level `level`'s square.
pub fn span(level: u32) -> f32 {
    SPAN * (1u32 << level) as f32
}

/// The depth (m) level `level` is evaluated for.
pub fn depth(level: u32) -> f32 {
    DEPTH_NEAR * DEPTH_RATIO.powi(level as i32)
}

/// The level a surface `depth` metres down reads, as `ship.wgsl` computes it.
pub fn level_of(depth: f32) -> f32 {
    ((depth.max(DEPTH_NEAR) / DEPTH_NEAR).ln() / DEPTH_RATIO.ln()).min((LEVELS - 1) as f32)
}

/// `Caustics` in `caustics.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct CausticsUniforms {
    centre: [f32; 4],
    level: [f32; 4],
}

/// The pass and its texture.
pub struct Caustics {
    sea: SeaRead,
    layout: wgpu::BindGroupLayout,
    /// Per level: its uniforms and its group (the uniforms and the level as a storage texture).
    levels: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    pipeline: wgpu::ComputePipeline,
    id: Option<PipelineId>,
    /// The wave field's cascades.
    cascades: usize,
}

impl Caustics {
    /// The pass over a wave field of `cascades` cascades whose group layout is `waves`.
    pub fn new(
        device: &wgpu::Device,
        waves: &wgpu::BindGroupLayout,
        cascades: usize,
        make: MakePipelines<'_>,
    ) -> Self {
        let compute = wgpu::ShaderStages::COMPUTE;
        let sea = SeaRead::new(device, "ocean caustics");
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean caustics"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: compute,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: compute,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: CAUSTICS_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ocean caustics"),
            size: wgpu::Extent3d {
                width: TEXELS,
                height: TEXELS,
                depth_or_array_layers: LEVELS,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CAUSTICS_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("ocean caustics"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let levels = (0..LEVELS)
            .map(|level| {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("ocean caustics"),
                    size: std::mem::size_of::<CausticsUniforms>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let target = texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("ocean caustics level"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: level,
                    array_layer_count: Some(1),
                    ..Default::default()
                });
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("ocean caustics"),
                    layout: &layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&target),
                        },
                    ],
                });
                (buffer, group)
            })
            .collect();
        let layouts = sea.layouts(waves, &layout);
        let pipeline = make.compute(|| {
            let module = shaders::shader_module(device, shaders::CAUSTICS, &[]);
            let groups: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(Some).collect();
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("ocean caustics"),
                layout: Some(
                    &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("ocean caustics"),
                        bind_group_layouts: &groups,
                        immediate_size: 0,
                    }),
                ),
                module: &module,
                entry_point: Some("caustics_main"),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        Self {
            sea,
            layout,
            levels,
            texture,
            view,
            pipeline,
            id: None,
            cascades,
        }
    }

    /// Builds the pass's pipeline through the renderer's registry (hot reload).
    pub(crate) fn register(
        &mut self,
        ctx: &mut RenderContext<'_>,
        waves: &wgpu::BindGroupLayout,
    ) -> Result<(), ShaderError> {
        self.id = Some(ctx.add_pipeline(
            "ocean caustics",
            shaders::CAUSTICS.path,
            Vec::new(),
            compute_recipe(self.sea.layouts(waves, &self.layout), "caustics_main"),
        )?);
        Ok(())
    }

    /// Takes the registry's current pipeline; whether it changed.
    pub(crate) fn adopt(&mut self, registry: &PipelineRegistry) -> bool {
        let Some(id) = self.id else {
            return false;
        };
        let pipeline = registry.compute(id);
        let changed = *pipeline != self.pipeline;
        if changed {
            self.pipeline = pipeline.clone();
        }
        changed
    }

    /// The wakes the pass's shared groups bind (it reads none of them).
    pub(crate) fn bind_wake(&mut self, device: &wgpu::Device, wake: Option<&WakeBindings<'_>>) {
        self.sea.bind_wake(device, wake);
    }

    /// The gains what stands under the sea reads: in red, a layer per depth, each [`span`] metres a repeat.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// The texture, for read-backs.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Evaluates the light under the sea about `camera` (the wave field's group `field` being this frame's).
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        camera: &CameraUniforms,
        field: &wgpu::BindGroup,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ocean caustics"),
            timestamp_writes: timer.and_then(|timer| timer.compute_pass("ocean caustics")),
        });
        pass.set_pipeline(&self.pipeline);
        self.sea.bind(device, queue, &mut pass, camera, field);
        for (level, (buffer, group)) in self.levels.iter().enumerate() {
            let span = span(level as u32);
            let uniforms = CausticsUniforms {
                centre: [camera.position[0], camera.position[2], span, TEXELS as f32],
                level: [
                    depth(level as u32),
                    (span / TEXELS as f32).max(BLUR * depth(level as u32)),
                    BRIGHTEST,
                    if depth(level as u32) < LONG_WAVES_FROM && self.cascades > 1 {
                        1.0
                    } else {
                        0.0
                    },
                ],
            };
            queue.write_buffer(buffer, 0, bytemuck::bytes_of(&uniforms));
            pass.set_bind_group(3, group, &[]);
            pass.dispatch_workgroups(TEXELS.div_ceil(8), TEXELS.div_ceil(8), 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_holds_its_depth() {
        assert_eq!(depth(0), 2.0);
        assert!((depth(3) - 31.25).abs() < 1e-4);
        for level in 0..LEVELS {
            assert!((level_of(depth(level)) - level as f32).abs() < 1e-4);
        }
        // Shallower than the first level and deeper than the last read the ends.
        assert_eq!(level_of(0.5), 0.0);
        assert_eq!(level_of(200.0), (LEVELS - 1) as f32);
    }
}
