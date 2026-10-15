//! The underwater view (the web game's `ocean/screen/underwater.ts` and the camera probe of `Ocean.update`).
//!
//! - [`UnderwaterProbe`] (the ocean's, run in its prepare while the near plane may reach the water): on a 128 × 72 grid
//!   over the screen, the depth below the waves and the wake of each pixel's near-plane point, and the camera's own depth
//!   below the waves. The surface reads the camera's to shade the back faces it sees from below as the sea's underside;
//!   the composite reads the grid for each pixel.
//! - [`UnderwaterLayer`] (a scene layer of its own, after every transparent but the aim overlay): two blended full-
//!   screen draws over the scene, the scene times the water's transmittance to the first surface behind each pixel (the
//!   renderer's backdrop depth copy, which holds the sea's surface), then the light the water scatters toward the camera
//!   and the film of water where the lens meets the surface; then the motes (`shaders/underwater_motes.wgsl`): a
//!   sparse cloud of suspended matter standing still in the world within a few metres of the lens, which streams past
//!   as the camera moves, so a boat's speed and turn read from under the sea.
//!
//! The water under the camera is the water the sea shows from above: the maps' absorption swatch is its beam
//! attenuation, stretched by `UNDERWATER_CLARITY` in `underwater.wgsl` so a hull reads from 100–150 m; the daylight
//! fades with depth by the diffuse attenuation, less the forward scattering the beam loses (`underwater_ray.wgsl`). The
//! web eased the absorption to a twentieth for a camera going under, which left the water clear for a kilometre and its
//! colour unchanged with depth.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use naval_render::{
    CameraUniforms, CopyPoint, DrawContext, Phase, PipelineId, PipelineRegistry, PrepareContext,
    RenderContext, SCENE_BACKDROP_DEPTH, SceneCopies, SceneLayer, ScenePipeline, ShaderError,
    compute_recipe,
};

use crate::sea_read::SeaRead;
use crate::ship_wake::WakeBindings;
use crate::{WaterColors, shaders};

/// The probe's grid over the screen.
pub const PROBE_SIZE: (u32, u32) = (128, 72);
/// The probe's texel format: depth below the surface in r, the camera's in g.
pub const PROBE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Wake heights are bounded to ±8 m; 10 cm more covers interpolation (`WAKE_BOUND`).
pub const WAKE_BOUND: f64 = 8.1;
/// Crests above twice the significant height do not occur in practice (`CREST_BOUND`).
pub const CREST_BOUND: f64 = 2.0;
/// How many metres of the map's water each metre under the camera counts as. The maps' absorption swatch is the water
/// as the sea shows it from above (visibility about 30 m in blue, 10 m in red on the North Atlantic); under the waves
/// the game wants a hull's bottom readable from 100–150 m, so the water there is clearer than the swatch: the same
/// colour, red still going first, so a distant hull turns blue-green rather than reading through air. The composite
/// dims and lights by it (`underwater.wgsl`), and the hulls light their submerged plating through the same water.
pub const UNDERWATER_CLARITY: f32 = 0.15;
/// Motes in the cube that repeats about the camera (`underwater_motes.wgsl`, its `CUBE`): about sixty in a view,
/// most of them faint.
pub const MOTES: u32 = 768;
/// Metres more than the highest crest at which the composite asks for its depth copy a frame ahead: the copies are
/// chosen before a frame's prepare, so the margin covers a frame of the camera's travel.
const COPY_MARGIN: f64 = 25.0;

/// The highest the drawn surface can reach this frame (`Ocean.update`): the field's bound, at most twice the
/// significant height, plus the wake's.
pub fn surface_bound(max_height: f64, significant_height: f64) -> f64 {
    max_height.min(CREST_BOUND * significant_height) + WAKE_BOUND
}

/// The CPU side of the pass's uniform branch (`nearPlaneMayBeSubmerged`): whether any near-plane corner (or the camera)
/// may lie below `max_height`. False: the whole view starts above the water.
pub fn near_plane_may_be_submerged(camera: &CameraUniforms, max_height: f64) -> bool {
    if !max_height.is_finite() {
        return true;
    }
    let world = Mat4::from_cols_array(&camera.inverse_view);
    let inverse_projection = Mat4::from_cols_array(&camera.inverse_projection);
    let eye = world.w_axis.truncate();
    let forward = -world.z_axis.truncate().normalize_or_zero();
    let mut lowest = f64::from(eye.y);
    for (x, y) in [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)] {
        // Any depth inside the frustum lies on the corner's ray, which then runs to the near plane.
        let on_ray = inverse_projection * Vec4::new(x, y, 0.5, 1.0);
        let corner = (world * (on_ray / on_ray.w)).truncate() - eye;
        let along = corner.dot(forward);
        if along > 0.0 {
            lowest = lowest.min(f64::from(eye.y + corner.y * camera.near / along));
        }
    }
    lowest <= max_height
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ProbeParams {
    inverse_projection: [f32; 16],
    camera_world: [f32; 16],
    camera: [f32; 4],
}

/// The camera's height probe (see the module docs). Owned by the ocean, which runs it in its prepare.
pub struct UnderwaterProbe {
    /// Groups 0–2: the camera, the wake and the wave field `height_at` and `wake_height` read.
    sea: SeaRead,
    probe_layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    probe_group: wgpu::BindGroup,
    pipeline: wgpu::ComputePipeline,
    id: Option<PipelineId>,
    /// Whether the near plane may reach the water this frame (the probe ran and its texels are this frame's).
    pub near_surface: bool,
}

impl UnderwaterProbe {
    /// The probe writing into `probe` (the surface's, [`PROBE_SIZE`] in [`PROBE_FORMAT`]); `waves` is the wave field's
    /// group layout.
    pub(crate) fn new(
        device: &wgpu::Device,
        probe: &wgpu::TextureView,
        waves: &wgpu::BindGroupLayout,
        make: naval_render::MakePipelines<'_>,
    ) -> Self {
        let compute = wgpu::ShaderStages::COMPUTE;
        let sea = SeaRead::new(device, "ocean underwater probe");
        let probe_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean underwater probe"),
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
                        format: PROBE_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean underwater probe"),
            size: std::mem::size_of::<ProbeParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let probe_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ocean underwater probe"),
            layout: &probe_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(probe),
                },
            ],
        });
        let layouts = sea.layouts(waves, &probe_layout);
        let pipeline = make.compute(|| {
            let module = shaders::shader_module(device, shaders::UNDERWATER_PROBE, &[]);
            let groups: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(Some).collect();
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("ocean underwater probe"),
                layout: Some(
                    &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("ocean underwater probe"),
                        bind_group_layouts: &groups,
                        immediate_size: 0,
                    }),
                ),
                module: &module,
                entry_point: Some("probe_main"),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        Self {
            sea,
            probe_layout,
            params,
            probe_group,
            pipeline,
            id: None,
            near_surface: false,
        }
    }

    /// Builds the probe's pipeline through the renderer's registry (hot reload).
    pub(crate) fn register(
        &mut self,
        ctx: &mut RenderContext<'_>,
        waves: &wgpu::BindGroupLayout,
    ) -> Result<(), ShaderError> {
        self.id = Some(ctx.add_pipeline(
            "ocean underwater probe",
            shaders::UNDERWATER_PROBE.path,
            Vec::new(),
            compute_recipe(self.sea.layouts(waves, &self.probe_layout), "probe_main"),
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

    /// Reads `wake` (or a calm sea's) from now on.
    pub(crate) fn bind_wake(&mut self, device: &wgpu::Device, wake: Option<&WakeBindings<'_>>) {
        self.sea.bind_wake(device, wake);
    }

    /// Probes the surface under the `camera` when the near plane may reach it (below `max_height`); the field's group
    /// is this frame's. Whether it ran. The frame the near plane leaves the water's reach the probe is cleared to above
    /// the water instead, so readers that do not know whether it ran (the clouds) can always trust it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        camera: &CameraUniforms,
        field: &wgpu::BindGroup,
        max_height: f64,
        timer: Option<&naval_render::GpuTimer>,
    ) -> bool {
        let was_near = self.near_surface;
        self.near_surface = near_plane_may_be_submerged(camera, max_height);
        if !self.near_surface && !was_near {
            return false;
        }
        // A near plane of 0 asks the shader to clear the probe.
        let near = if self.near_surface { camera.near } else { 0.0 };
        let params = ProbeParams {
            inverse_projection: camera.inverse_projection,
            camera_world: camera.inverse_view,
            camera: [
                camera.position[0],
                camera.position[1],
                camera.position[2],
                near,
            ],
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ocean underwater probe"),
            timestamp_writes: timer.and_then(|timer| timer.compute_pass("ocean underwater probe")),
        });
        pass.set_pipeline(&self.pipeline);
        self.sea.bind(device, queue, &mut pass, camera, field);
        pass.set_bind_group(3, &self.probe_group, &[]);
        pass.dispatch_workgroups(PROBE_SIZE.0.div_ceil(8), PROBE_SIZE.1.div_ceil(8), 1);
        self.near_surface
    }
}

/// The probe's texture (the surface owns it and binds it).
pub(crate) fn probe_texture(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ocean underwater probe"),
        size: wgpu::Extent3d {
            width: PROBE_SIZE.0,
            height: PROBE_SIZE.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: PROBE_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// `Underwater` in `underwater.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct UnderwaterUniforms {
    pigment: [f32; 4],
    absorption: [f32; 4],
    transmission: [f32; 4],
    sun_direction: [f32; 4],
    sun_color: [f32; 4],
    water: crate::optics::WaterField,
}

/// The underwater composite (see the module docs): a scene layer drawn in the transparent phase, after every effect.
pub struct UnderwaterLayer {
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    probe: wgpu::TextureView,
    sampler: wgpu::Sampler,
    group: Option<wgpu::BindGroup>,
    depth_version: Option<u64>,
    dim: PipelineId,
    light: PipelineId,
    motes: PipelineId,
    /// Whether the motes draw (on by default).
    pub show_motes: bool,
    /// The water's colours (`OceanParameters::colors`, the absorption as the map sets it: the layer eases it for a
    /// camera under water itself) and the celestial light's direction and colour (brightest channel 1). Set them each
    /// frame.
    pub colors: WaterColors,
    pub water: crate::optics::WaterField,
    pub sun_direction: Vec3,
    pub sun_color: Vec3,
    /// The highest the drawn surface can reach this frame ([`surface_bound`]; the ocean's
    /// [`crate::Ocean::surface_bound`]). Set it each frame.
    pub max_height: f64,
    /// Whether the composite draws this frame, and asked for the depth copy for the next.
    active: bool,
    wants_copy: bool,
}

impl UnderwaterLayer {
    /// The composite over the ocean's probe (`Ocean::underwater_probe`).
    pub fn new(
        ctx: &mut RenderContext<'_>,
        probe: &wgpu::TextureView,
    ) -> Result<Self, ShaderError> {
        crate::shaders::register(ctx)?;
        let fragment = wgpu::ShaderStages::FRAGMENT;
        // The motes read the uniforms and the probe in their vertex stage.
        let both = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let layout = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ocean underwater"),
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
                    naval_render::copies::depth_copy_layout_entry(1, fragment),
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: both,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: both,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let pipeline = |blend: wgpu::BlendState| ScenePipeline {
            bind_group_layouts: vec![layout.clone()],
            primitive: wgpu::PrimitiveState::default(),
            depth_write: false,
            blend: Some(blend),
            ..ScenePipeline::new("vertex", "underwater_dim")
        };
        let dim = ctx.add_scene_pipeline(
            "ocean underwater dim",
            shaders::UNDERWATER.path,
            ScenePipeline {
                fragment_entry: "underwater_dim",
                ..pipeline(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Zero,
                        dst_factor: wgpu::BlendFactor::Src,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Zero,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                })
            },
        )?;
        let light = ctx.add_scene_pipeline(
            "ocean underwater light",
            shaders::UNDERWATER.path,
            ScenePipeline {
                fragment_entry: "underwater_light",
                ..pipeline(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Zero,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                })
            },
        )?;
        // The motes: dots added to the scene, hidden by what stands nearer.
        let motes = ctx.add_scene_pipeline(
            "ocean underwater motes",
            shaders::UNDERWATER_MOTES.path,
            ScenePipeline {
                bind_group_layouts: vec![layout.clone()],
                primitive: wgpu::PrimitiveState::default(),
                depth_write: false,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Zero,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
                ..ScenePipeline::new("mote_vertex", "mote_fragment")
            },
        )?;
        let uniforms = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean underwater"),
            size: std::mem::size_of::<UnderwaterUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = ctx.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ocean underwater probe"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Ok(Self {
            layout,
            uniforms,
            probe: probe.clone(),
            sampler,
            group: None,
            depth_version: None,
            dim,
            light,
            motes,
            show_motes: true,
            water: crate::optics::WaterField::default(),
            colors: crate::calibration::scene_parameters(
                &crate::calibration::NORTH_ATLANTIC,
                9.0,
                true,
            )
            .colors,
            sun_direction: Vec3::Y,
            sun_color: Vec3::ONE,
            max_height: f64::INFINITY,
            active: false,
            wants_copy: false,
        })
    }

    /// Whether the composite drew in the last frame.
    pub fn active(&self) -> bool {
        self.active
    }
}

impl SceneLayer for UnderwaterLayer {
    fn label(&self) -> &str {
        "underwater"
    }

    fn scene_copies(&self) -> SceneCopies {
        if self.wants_copy {
            SceneCopies::BACKDROP_DEPTH
        } else {
            SceneCopies::NONE
        }
    }

    fn prepare(&mut self, ctx: &mut PrepareContext<'_>) {
        let camera = ctx.camera;
        self.wants_copy = near_plane_may_be_submerged(camera, self.max_height + COPY_MARGIN);
        let depth = ctx.resources.texture(SCENE_BACKDROP_DEPTH);
        self.active = near_plane_may_be_submerged(camera, self.max_height) && depth.is_some();
        let Some(depth) = depth else {
            return;
        };
        if self.depth_version != Some(depth.version) || self.group.is_none() {
            self.group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ocean underwater"),
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
                        resource: wgpu::BindingResource::TextureView(&self.probe),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            }));
            self.depth_version = Some(depth.version);
        }
        if !self.active {
            return;
        }
        let colors = &self.colors;
        let rgba = |c: [f32; 3]| [c[0], c[1], c[2], 0.0];
        let uniforms = UnderwaterUniforms {
            pigment: rgba(colors.water),
            absorption: [
                colors.absorption[0],
                colors.absorption[1],
                colors.absorption[2],
                UNDERWATER_CLARITY,
            ],
            transmission: rgba(colors.transmission),
            sun_direction: self.sun_direction.extend(0.0).to_array(),
            sun_color: self.sun_color.extend(0.0).to_array(),
            water: self.water,
        };
        ctx.queue
            .write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
    }

    fn after_copies(&mut self, _point: CopyPoint, _ctx: &mut PrepareContext<'_>) {}

    fn draw(&self, phase: Phase, pass: &mut wgpu::RenderPass<'_>, ctx: &DrawContext<'_>) {
        if phase != Phase::Transparent || !self.active {
            return;
        }
        let Some(group) = &self.group else {
            return;
        };
        pass.set_bind_group(1, group, &[]);
        pass.set_pipeline(ctx.pipelines.render(self.dim));
        pass.draw(0..3, 0..1);
        pass.set_pipeline(ctx.pipelines.render(self.light));
        pass.draw(0..3, 0..1);
        if self.show_motes {
            pass.set_pipeline(ctx.pipelines.render(self.motes));
            pass.draw(0..6, 0..MOTES);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_near_plane_reaches_the_water_from_a_low_camera_only() {
        let camera = |y: f32, pitch: f32| {
            let eye = Vec3::new(0.0, y, 0.0);
            let target = eye + Vec3::new(0.0, pitch.sin(), -pitch.cos());
            naval_render::Camera {
                eye,
                target,
                fov_y_degrees: 55.0,
                near: 1.0,
                far: 30_000.0,
                curvature: naval_render::camera::EARTH_CURVATURE,
            }
            .uniforms(1600, 1000, naval_render::FrameTime::default())
        };
        assert!(!near_plane_may_be_submerged(&camera(30.0, 0.0), 10.0));
        assert!(near_plane_may_be_submerged(&camera(9.5, 0.0), 10.0));
        assert!(near_plane_may_be_submerged(&camera(-3.0, 0.3), 10.0));
        assert!(near_plane_may_be_submerged(
            &camera(30.0, 0.0),
            f64::INFINITY
        ));
    }
}
