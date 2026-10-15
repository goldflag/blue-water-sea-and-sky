//! The ocean surface: the clipmap mesh ([`crate::geometry`]) drawn with `shaders/surface.wgsl`, which displaces it by
//! the wave field and shades it (the web game's `ocean/surface/OceanSurfaceMaterial.ts` with physical reflections and water
//! colour, the game's defaults). It draws inside the scene's HDR pass with reversed depth; the opaque scene's colour
//! and depth copies ([`OceanSurface::set_scene`]) give it straight-through transmission and shoreline foam.
//!
//! Bind groups follow the renderer core's scene pipelines (`native/docs/render-core.md`): 0 the camera
//! (`naval_render::CameraUniforms`, bound by the renderer), 1 the surface's own (uniforms, foam texture, scene copies,
//! and the wakes' uniforms and textures at bindings 5–10, [`crate::ship_wake::ShipWake::bindings`]), 2 the wave field's
//! ([`crate::WaveField::surface_bind_group`]), 3 the sky environment (naval-sky's layout). A GPU guarantees four bind
//! groups, so the wakes join group 1 rather than taking a fifth.

use bytemuck::{Pod, Zeroable};
use naval_render::{
    MakePipelines, Pipeline, PipelineId, PipelineRegistry, RenderContext, ShaderError,
};

use crate::field::WaveField;
use crate::foam_texture::{FOAM_ANISOTROPY, FoamTexture};
use crate::geometry::{
    self, Clipmap, ClipmapMesh, Detail, FAN_BLOCK_ROWS, Fan, MAX_CLIPMAP_LEVELS, OceanVertex,
};
use crate::ship_wake::{WAKE_VEC4S, WakeBindings};
use crate::{DEPTH_FORMAT, HDR_FORMAT, OceanParameters, QualityTier, shaders};

/// `Surface` in `surface.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct SurfaceUniforms {
    frames: [[f32; 4]; MAX_CLIPMAP_LEVELS],
    clipmap: [f32; 4],
    /// x the finest level drawn, y how far it has morphed onto the next ([`Clipmap::detail`]); a magnified view's fan
    /// ([`Fan`]): z its first row, w its squeeze.
    detail: [f32; 4],
    /// The fan: its first column's bearing, its step, its knee's range, its last row's range.
    fan: [f32; 4],
    absorption: [f32; 4],
    transmission: [f32; 4],
    crest_foam: [f32; 4],
    surface_foam: [f32; 4],
    shoreline_foam: [f32; 4],
    foam: [f32; 4],
    fog_color: [f32; 4],
    fog: [f32; 4],
    shadows: [f32; 4],
    reflections: [f32; 4],
    /// rgb the water's pigment (the underside's deep), w 1 while the underwater probe is this frame's.
    pigment: [f32; 4],
    /// x: rain (0 dry … 1 a downpour), yzw: the spray haze (extinction at its base, scale height, base height:
    /// `crate::spindrift::spray_haze`).
    weather: [f32; 4],
    /// The scene's point lights on the sea (`shaders/point_lights.wgsl`).
    lights: naval_render::lights::PointLightsUniform,
    water: crate::optics::WaterField,
}

/// Screen-space reflections of the opaque scene (`OceanApi.reflections`): ships and land mirrored where the traced ray
/// meets them on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenReflections {
    /// Graphics → Reflections `scene` (the High and Ultra presets). Live; nothing is traced on a tier without steps.
    pub screen_space: bool,
    /// The longest ray (m): 400, stretched under binoculars to twice the range to the examined hull
    /// (`WaterViewFocus`).
    pub max_distance: f32,
}

impl ScreenReflections {
    /// A tier's defaults: on wherever the tier traces, 400 m (kept from the web game; not a product decision).
    pub fn for_tier(tier: QualityTier) -> Self {
        Self {
            screen_space: tier.reflection_steps() > 0,
            max_distance: 400.0,
        }
    }
}

fn rgba(rgb: [f32; 3], a: f32) -> [f32; 4] {
    [rgb[0], rgb[1], rgb[2], a]
}

/// The surface mesh, pipeline and its own bindings.
pub struct OceanSurface {
    clipmap: Clipmap,
    /// The finest level this frame draws ([`OceanSurface::prepare`]).
    detail: Detail,
    pipeline: wgpu::RenderPipeline,
    /// The same with the scene's point lights (`POINT_LIGHTS`), drawn on frames that have any: the lights' block costs
    /// the sea's shader registers even where it runs nothing, so a frame without lights draws without it.
    lit_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    foam_view: wgpu::TextureView,
    foam_sampler: wgpu::Sampler,
    streak_mean: f32,
    scene: (wgpu::TextureView, wgpu::TextureView),
    /// The clouds' shadow: map, sampler and uniforms (a disabled placeholder until [`OceanSurface::set_cloud_shadow`]).
    cloud: (wgpu::TextureView, wgpu::Sampler, wgpu::Buffer),
    /// The motion pass's pipeline (temporal anti-aliasing): the surface's vertices, no history.
    motion_pipeline: wgpu::RenderPipeline,
    calm: CalmWake,
    wake_sampler: wgpu::Sampler,
    /// The underwater probe's texture (written by `crate::underwater::UnderwaterProbe`).
    probe: (wgpu::Texture, wgpu::TextureView),
    /// The depth copy's tiles holding a surface (`crate::reflection_tiles`), a placeholder that traces every ray until
    /// [`OceanSurface::set_reflection_table`].
    reflection_table: wgpu::Buffer,
    group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    /// Where each level's middle and ring lie in the indices ([`ClipmapMesh`]).
    fills: Vec<std::ops::Range<u32>>,
    rings: Vec<u32>,
    /// The fan's block ([`geometry::build_fan`]), and the fan this frame draws in the clipmap's place, if any.
    fan_vertices: wgpu::Buffer,
    fan_indices: wgpu::Buffer,
    fan: Option<Fan>,
    /// Radius of the horizon ring (m).
    pub horizon: f64,
    /// Whether fog is applied in the surface's shader (off when the renderer fogs every material itself).
    pub fog: bool,
    /// Ship shadows on the sea from the renderer core's sun maps (`naval::shadow::water_sun_shadow`): the Graphics
    /// menu's water shadows, 0 off, 1 low, 2 medium, 3 high (the High preset). Off by default.
    pub water_shadows: u32,
    /// Rain on the sea: the weather's precipitation, 0 dry … 1 a downpour. Its drops ring the water with capillary waves,
    /// drawn into the normal near the camera and spread as unresolved slope beyond, which dulls the glitter and blurs
    /// the mirrored sky. Live; 0 by default.
    pub rain: f32,
    /// The gale's spray haze over the sea (`crate::spindrift::spray_haze`): the ocean sets it each frame from its wind.
    pub haze: [f32; 4],
    /// Ships and land mirrored in the sea (they need the opaque scene's copies: [`OceanSurface::set_scene`]). On for
    /// the tiers that trace them.
    pub reflections: ScreenReflections,
    /// The scene's point lights the sea takes this frame (`shaders/point_lights.wgsl`): the combat flashes, the fires
    /// and lightning's stroke, as the host lists them for the sea (none by default).
    pub point_lights: naval_render::PointLights,
    /// Steps along the clipped ray, fixed by the tier (the shader is built with them).
    reflection_steps: u32,
    /// Groups 0–3 of both pipelines and the shader's defs, for the registry's recipes.
    groups: [wgpu::BindGroupLayout; 4],
    defs: [(&'static str, u32); 2],
}

/// The shader def that compiles the scene's point lights into the surface (`#ifdef POINT_LIGHTS`).
const POINT_LIGHTS: (&str, u32) = ("POINT_LIGHTS", 1);

/// The surface's pipelines on the renderer's registry ([`OceanSurface::register_pipelines`]).
#[derive(Clone, Copy, Debug)]
pub struct SurfacePipelines {
    surface: PipelineId,
    lit: PipelineId,
    motion: PipelineId,
}

/// The surface's pipeline over `groups` (`motion` false), or its twin in the renderer's motion pass (`scene.motion`,
/// `scene.depth` read-only).
fn surface_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    groups: &[wgpu::BindGroupLayout; 4],
    motion: bool,
    cache: Option<&wgpu::PipelineCache>,
) -> wgpu::RenderPipeline {
    let label = if motion {
        "ocean surface motion"
    } else {
        "ocean surface"
    };
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ocean surface"),
        bind_group_layouts: &[
            Some(&groups[0]),
            Some(&groups[1]),
            Some(&groups[2]),
            Some(&groups[3]),
        ],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vertex"),
            compilation_options: Default::default(),
            buffers: &[Some(OceanVertex::LAYOUT)],
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(if motion { "motion" } else { "fragment" }),
            compilation_options: Default::default(),
            targets: &[Some(if motion {
                naval_render::taa::motion_target()
            } else {
                HDR_FORMAT.into()
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            // Double-sided: a folding storm crest shows its back face.
            cull_mode: None,
            ..Default::default()
        },
        // The scene pass's reversed depth, written: the sea is opaque.
        depth_stencil: Some(if motion {
            naval_render::taa::motion_depth_state()
        } else {
            naval_render::scene_depth_state(true)
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache,
    })
}

impl OceanSurface {
    /// Builds both pipelines through the renderer's registry, as [`OceanSurface::new`] builds them. The ocean's files
    /// must be registered first ([`crate::shaders::register`]).
    pub fn register_pipelines(
        &self,
        ctx: &mut RenderContext<'_>,
    ) -> Result<SurfacePipelines, ShaderError> {
        let defs = shaders::library_defs(&self.defs);
        let lit_defs = shaders::library_defs(&[self.defs[0], self.defs[1], POINT_LIGHTS]);
        let mut add = |label: &str, defs: &naval_render::ShaderDefs, motion: bool| {
            let groups = self.groups.clone();
            ctx.add_pipeline(
                label,
                shaders::SURFACE.path,
                defs.clone(),
                Box::new(move |build| {
                    Pipeline::Render(surface_pipeline(
                        build.device,
                        build.module,
                        &groups,
                        motion,
                        build.cache,
                    ))
                }),
            )
        };
        Ok(SurfacePipelines {
            surface: add("ocean surface", &defs, false)?,
            lit: add("ocean surface lit", &lit_defs, false)?,
            motion: add("ocean surface motion", &defs, true)?,
        })
    }

    /// Takes the registry's current pipelines in place of the surface's own; whether any changed.
    pub fn adopt_pipelines(&mut self, registry: &PipelineRegistry, ids: &SurfacePipelines) -> bool {
        let mut changed = false;
        for (slot, id) in [
            (&mut self.pipeline, ids.surface),
            (&mut self.lit_pipeline, ids.lit),
            (&mut self.motion_pipeline, ids.motion),
        ] {
            let new = registry.render(id);
            if slot != new {
                *slot = new.clone();
                changed = true;
            }
        }
        changed
    }

    /// The surface for a quality tier and the field it reads, with the renderer's camera layout (group 0,
    /// `RenderContext::camera_layout`) and the sky environment's (group 3), covering a far plane of `far` metres.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tier: QualityTier,
        field: &WaveField,
        camera: &wgpu::BindGroupLayout,
        environment: &wgpu::BindGroupLayout,
        far: f64,
    ) -> Self {
        Self::build(
            device,
            queue,
            tier,
            field,
            camera,
            environment,
            far,
            MakePipelines::Now,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tier: QualityTier,
        field: &WaveField,
        camera: &wgpu::BindGroupLayout,
        environment: &wgpu::BindGroupLayout,
        far: f64,
        make: MakePipelines<'_>,
    ) -> Self {
        let clipmap = tier.clipmap();
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let visibility = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean surface"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility,
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
                // The opaque scene's colour, filtered for the screen-space reflections' smeared reads.
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // The wakes (shaders/wake.wgsl): their packed uniforms, the wake field's two display textures and their
                // sampler (read by the vertex stage too), the trail atlas and the torpedo field.
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new((WAKE_VEC4S * 16) as u64),
                    },
                    count: None,
                },
                filtered_texture(6, visibility),
                filtered_texture(7, visibility),
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                filtered_texture(9, fragment),
                filtered_texture(10, fragment),
                // The clouds' shadow (`naval_clouds::CloudShadow`): its map, linear repeat sampler and uniforms.
                filtered_texture(11, fragment),
                wgpu::BindGroupLayoutEntry {
                    binding: 12,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 13,
                    visibility: fragment,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // The underwater probe (`crate::underwater`): whether the camera is under the waves.
                filtered_texture(14, fragment),
                // The depth copy's tiles holding a surface, summed (`crate::reflection_tiles`).
                wgpu::BindGroupLayoutEntry {
                    binding: 15,
                    visibility: fragment,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            crate::reflection_tiles::table_bytes((0, 0)),
                        ),
                    },
                    count: None,
                },
            ],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean surface"),
            size: std::mem::size_of::<SurfaceUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // The foam texture with its generated mip chain.
        let foam = FoamTexture::generate();
        let foam_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ocean foam"),
            size: wgpu::Extent3d {
                width: foam.levels[0].0,
                height: foam.levels[0].0,
                depth_or_array_layers: 1,
            },
            mip_level_count: foam.levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, (size, pixels)) in foam.levels.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &foam_texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size * 4),
                    rows_per_image: Some(*size),
                },
                wgpu::Extent3d {
                    width: *size,
                    height: *size,
                    depth_or_array_layers: 1,
                },
            );
        }
        let foam_view = foam_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let foam_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ocean foam"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: FOAM_ANISOTROPY,
            ..Default::default()
        });

        // Until the renderer hands over its opaque copies: a black scene at the far plane.
        let scene = placeholder_scene(device);
        let probe = crate::underwater::probe_texture(device);
        let reflection_table = crate::reflection_tiles::placeholder_table(device);
        // Until the clouds hand theirs over: no cloud shadow (a zeroed, disabled uniform).
        let cloud = placeholder_cloud_shadow(device);
        // Until a ship wake is attached: a calm one.
        let calm = CalmWake::new(device);
        let wake_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ocean wakes"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let group = Self::scene_group(
            device,
            &layout,
            &uniforms,
            &foam_view,
            &foam_sampler,
            (&scene.0, &scene.1),
            &calm.bindings(),
            &wake_sampler,
            (&cloud.0, &cloud.1, &cloud.2),
            (&probe.1, &reflection_table),
        );

        let defs = [
            ("CASCADES", tier.cascades().len() as u32),
            ("REFLECTION_STEPS", tier.reflection_steps()),
        ];
        let groups = [
            camera.clone(),
            layout.clone(),
            field.surface_layout().clone(),
            environment.clone(),
        ];
        let module = make.module(|| shaders::shader_module(device, shaders::SURFACE, &defs));
        let pipeline = make.render(|| {
            surface_pipeline(
                device,
                module.as_ref().expect("built now"),
                &groups,
                false,
                None,
            )
        });
        let lit_module = make.module(|| {
            shaders::shader_module(device, shaders::SURFACE, &[defs[0], defs[1], POINT_LIGHTS])
        });
        let lit_pipeline = make.render(|| {
            surface_pipeline(
                device,
                lit_module.as_ref().expect("built now"),
                &groups,
                false,
                None,
            )
        });
        let motion_pipeline = make.render(|| {
            surface_pipeline(
                device,
                module.as_ref().expect("built now"),
                &groups,
                true,
                None,
            )
        });

        let mesh = geometry::build(clipmap, far);
        let vertices = buffer_with(
            device,
            "ocean vertices",
            bytemuck::cast_slice(&mesh.vertices),
            wgpu::BufferUsages::VERTEX,
        );
        let indices = buffer_with(
            device,
            "ocean indices",
            bytemuck::cast_slice(&mesh.indices),
            wgpu::BufferUsages::INDEX,
        );
        let fan = geometry::build_fan();
        let fan_vertices = buffer_with(
            device,
            "ocean fan vertices",
            bytemuck::cast_slice(&fan.vertices),
            wgpu::BufferUsages::VERTEX,
        );
        let fan_indices = buffer_with(
            device,
            "ocean fan indices",
            bytemuck::cast_slice(&fan.indices),
            wgpu::BufferUsages::INDEX,
        );
        Self {
            clipmap,
            detail: Detail::default(),
            pipeline,
            lit_pipeline,
            layout,
            uniforms,
            foam_view,
            foam_sampler,
            streak_mean: foam.streak_mean,
            scene,
            cloud,
            motion_pipeline,
            calm,
            wake_sampler,
            probe,
            reflection_table,
            group,
            vertices,
            indices,
            index_count: mesh.indices.len() as u32,
            fills: mesh.fills,
            rings: mesh.rings,
            fan_vertices,
            fan_indices,
            fan: None,
            horizon: mesh.radius,
            fog: true,
            water_shadows: 0,
            rain: 0.0,
            haze: [0.0; 4],
            reflections: ScreenReflections::for_tier(tier),
            point_lights: naval_render::PointLights::NONE,
            reflection_steps: tier.reflection_steps(),
            groups,
            defs,
        }
    }

    /// Grows the horizon ring to cover a far plane of `far` metres when it does not already (`ensureHorizon` in
    /// `src/game/ocean`): a view that raises the camera's far plane (the air map, from tens of kilometres up) would
    /// otherwise see the sea end at the ring built for the scene's own. The ring grows once and keeps that reach.
    /// Returns whether it grew.
    pub fn ensure_horizon(&mut self, device: &wgpu::Device, far: f64) -> bool {
        if !far.is_finite() || far * geometry::HORIZON_REACH <= self.horizon {
            return false;
        }
        let mesh = geometry::build(self.clipmap, far);
        self.vertices = buffer_with(
            device,
            "ocean vertices",
            bytemuck::cast_slice(&mesh.vertices),
            wgpu::BufferUsages::VERTEX,
        );
        self.indices = buffer_with(
            device,
            "ocean indices",
            bytemuck::cast_slice(&mesh.indices),
            wgpu::BufferUsages::INDEX,
        );
        self.index_count = mesh.indices.len() as u32;
        let ClipmapMesh {
            fills,
            rings,
            radius,
            ..
        } = mesh;
        (self.fills, self.rings, self.horizon) = (fills, rings, radius);
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn scene_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniforms: &wgpu::Buffer,
        foam: &wgpu::TextureView,
        foam_sampler: &wgpu::Sampler,
        (color, depth): (&wgpu::TextureView, &wgpu::TextureView),
        wake: &WakeBindings<'_>,
        wake_sampler: &wgpu::Sampler,
        (cloud_map, cloud_sampler, cloud_uniforms): (
            &wgpu::TextureView,
            &wgpu::Sampler,
            &wgpu::Buffer,
        ),
        (probe, reflection_table): (&wgpu::TextureView, &wgpu::Buffer),
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ocean surface"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(foam),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(foam_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(color),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wake.uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(wake.displays[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(wake.displays[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(wake_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(wake.trail_atlas),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::TextureView(wake.torpedo_field),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::TextureView(cloud_map),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: wgpu::BindingResource::Sampler(cloud_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 13,
                    resource: cloud_uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 14,
                    resource: wgpu::BindingResource::TextureView(probe),
                },
                wgpu::BindGroupEntry {
                    binding: 15,
                    resource: reflection_table.as_entire_binding(),
                },
            ],
        })
    }

    /// Point the surface at copies of the opaque scene drawn before it: HDR colour (`Rgba16Float`, filterable) and
    /// reversed depth (`Depth32Float`), both at the render target's size.
    pub fn set_scene(
        &mut self,
        device: &wgpu::Device,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        self.scene = (color.clone(), depth.clone());
        self.rebind(device, None);
    }

    /// Read the depth copy's tiles (`crate::reflection_tiles`) from the next [`OceanSurface::set_scene`] on: the table
    /// built for the copy that call binds.
    pub fn set_reflection_table(&mut self, table: &wgpu::Buffer) {
        self.reflection_table = table.clone();
    }

    /// Shade the clouds' shadow (`naval_clouds::CloudShadow`: its map, linear repeat sampler and 64-byte uniforms), or
    /// none with `None`. Like `set_scene`, this binds a calm wake: rebind the wake after it (`Ocean::set_cloud_shadow`
    /// does).
    pub fn set_cloud_shadow(
        &mut self,
        device: &wgpu::Device,
        shadow: Option<(&wgpu::TextureView, &wgpu::Sampler, &wgpu::Buffer)>,
    ) {
        self.cloud = match shadow {
            Some((map, sampler, uniforms)) => (map.clone(), sampler.clone(), uniforms.clone()),
            None => placeholder_cloud_shadow(device),
        };
        self.rebind(device, None);
    }

    /// Shade the wakes `wake` holds (call again whenever its `version` moves, and after `set_scene`); `None` shows a calm
    /// sea.
    pub fn set_wake(&mut self, device: &wgpu::Device, wake: Option<&WakeBindings<'_>>) {
        self.rebind(device, wake);
    }

    fn rebind(&mut self, device: &wgpu::Device, wake: Option<&WakeBindings<'_>>) {
        let calm = self.calm.bindings();
        self.group = Self::scene_group(
            device,
            &self.layout,
            &self.uniforms,
            &self.foam_view,
            &self.foam_sampler,
            (&self.scene.0, &self.scene.1),
            wake.unwrap_or(&calm),
            &self.wake_sampler,
            (&self.cloud.0, &self.cloud.1, &self.cloud.2),
            (&self.probe.1, &self.reflection_table),
        );
    }

    /// The foam texture and its repeating, anisotropic sampler.
    pub(crate) fn foam(&self) -> (wgpu::TextureView, wgpu::Sampler) {
        (self.foam_view.clone(), self.foam_sampler.clone())
    }

    /// The underwater probe's texture ([`crate::underwater::PROBE_SIZE`]), for the composite.
    pub fn probe_view(&self) -> &wgpu::TextureView {
        &self.probe.1
    }

    /// Write this frame's clipmap snap (around `centre`, world metres (x, z): the eye, or what a magnified view is aimed
    /// at, [`geometry::focus`]), the finest level the view's `standoff` from the water there calls for
    /// ([`geometry::standoff`], [`Clipmap::detail`]) and parameters; `probed` says the underwater probe ran this frame
    /// (the near plane may reach the water). A magnified view's `fan` ([`geometry::fan`]) is drawn in the clipmap's
    /// place.
    pub fn prepare(
        &mut self,
        queue: &mut naval_render::UploadSink<'_>,
        centre: [f64; 2],
        standoff: f64,
        fan: Option<Fan>,
        parameters: &OceanParameters,
        probed: bool,
    ) {
        self.detail = self.clipmap.detail(standoff);
        self.fan = fan;
        let foam = &parameters.foam;
        let fog = &parameters.fog;
        let uniforms = SurfaceUniforms {
            frames: self.clipmap.frames(centre[0], centre[1]),
            clipmap: [
                self.clipmap.segments as f32,
                self.clipmap.levels as f32,
                centre[0] as f32,
                centre[1] as f32,
            ],
            detail: [
                self.detail.first as f32,
                self.detail.morph,
                fan.map_or(0.0, |fan| fan.first as f32),
                fan.map_or(0.0, |fan| fan.squeeze as f32),
            ],
            fan: fan.map_or([0.0; 4], |fan| {
                // The bearing within a turn of zero, where a 32-bit float still holds a centimetre at the horizon.
                let turn = std::f64::consts::TAU;
                [
                    ((fan.bearing + turn / 2.0).rem_euclid(turn) - turn / 2.0) as f32,
                    fan.step as f32,
                    fan.knee as f32,
                    fan.far as f32,
                ]
            }),
            absorption: rgba(parameters.colors.absorption, 0.0),
            transmission: rgba(parameters.colors.transmission, 0.0),
            crest_foam: rgba(foam.crest.color, foam.crest.opacity),
            surface_foam: rgba(foam.surface.color, foam.surface.opacity),
            shoreline_foam: rgba(foam.shoreline.color, foam.shoreline.opacity),
            foam: [
                foam.crest.wind_stretch,
                foam.surface.coverage as f32,
                self.streak_mean,
                0.0,
            ],
            fog_color: rgba(fog.color, fog.power),
            fog: [
                fog.start,
                fog.end,
                fog.sky_blend_distance,
                if self.fog { 1.0 } else { 0.0 },
            ],
            shadows: [self.water_shadows.min(3) as f32, 0.0, 0.0, 0.0],
            reflections: [
                f32::from(u8::from(
                    self.reflections.screen_space && self.reflection_steps > 0,
                )),
                self.reflections.max_distance,
                0.0,
                0.0,
            ],
            pigment: rgba(parameters.colors.water, if probed { 1.0 } else { 0.0 }),
            weather: [
                self.rain.clamp(0.0, 1.0),
                self.haze[0],
                self.haze[1],
                self.haze[2],
            ],
            lights: self.point_lights.uniform(),
            water: parameters.water,
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
    }

    /// The water mesh's layout.
    pub fn clipmap(&self) -> Clipmap {
        self.clipmap
    }

    /// The fan this frame draws in the clipmap's place ([`geometry::fan`]): a magnified view's.
    pub fn fan(&self) -> Option<Fan> {
        self.fan
    }

    /// The finest level drawn, its middle filled, then every ring from it out and the horizon; or a magnified view's
    /// fan, a block of rows to an instance.
    fn draw_mesh(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(fan) = self.fan {
            pass.set_vertex_buffer(0, self.fan_vertices.slice(..));
            pass.set_index_buffer(self.fan_indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..fan.columns * FAN_BLOCK_ROWS * 6, 0, 0..fan.blocks());
            return;
        }
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(self.fills[self.detail.first].clone(), 0, 0..1);
        pass.draw_indexed(self.rings[self.detail.first]..self.index_count, 0, 0..1);
    }

    /// Draw inside a pass with an [`HDR_FORMAT`] target and a [`DEPTH_FORMAT`] attachment, the camera already bound at
    /// group 0 (the renderer core does it before each layer draws).
    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        field: &WaveField,
        environment: &wgpu::BindGroup,
    ) {
        pass.set_pipeline(if self.point_lights.is_empty() {
            &self.pipeline
        } else {
            &self.lit_pipeline
        });
        pass.set_bind_group(1, &self.group, &[]);
        pass.set_bind_group(2, field.surface_bind_group(), &[]);
        pass.set_bind_group(3, environment, &[]);
        self.draw_mesh(pass);
    }

    /// Draw the surface into the renderer's motion pass (`SceneLayer::draw_motion`): the sea takes no history.
    pub fn draw_motion(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        field: &WaveField,
        environment: &wgpu::BindGroup,
    ) {
        pass.set_pipeline(&self.motion_pipeline);
        pass.set_bind_group(1, &self.group, &[]);
        pass.set_bind_group(2, field.surface_bind_group(), &[]);
        pass.set_bind_group(3, environment, &[]);
        self.draw_mesh(pass);
    }

    /// The screen-space reflections' steps the tier built the surface with (0: none traced).
    pub fn reflection_steps(&self) -> u32 {
        self.reflection_steps
    }

    /// Triangles this frame's draw holds: the finest level drawn's middle, the rings from it out and the horizon, or
    /// the fan's.
    pub fn triangles(&self) -> u32 {
        if let Some(fan) = self.fan {
            return fan.columns * FAN_BLOCK_ROWS * 2 * fan.blocks();
        }
        let first = self.detail.first;
        (self.fills[first].len() as u32 + self.index_count - self.rings[first]) / 3
    }
}

fn filtered_texture(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
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

/// The wake bindings of a sea with no wakes: zeroed uniforms (no hulls, no weights) and black 1×1 textures.
struct CalmWake {
    uniforms: wgpu::Buffer,
    display: wgpu::TextureView,
}

impl CalmWake {
    fn new(device: &wgpu::Device) -> Self {
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("calm wake"),
            size: (WAKE_VEC4S * 16) as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let display = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("calm wake"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        Self { uniforms, display }
    }

    fn bindings(&self) -> WakeBindings<'_> {
        WakeBindings {
            uniforms: &self.uniforms,
            displays: [&self.display, &self.display],
            trail_atlas: &self.display,
            torpedo_field: &self.display,
        }
    }
}

fn buffer_with(
    device: &wgpu::Device,
    label: &str,
    contents: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents,
        usage,
    })
}

/// A 1×1 black colour and a 1×1 depth at the far plane (textures start zeroed; reversed depth 0 is far).
fn placeholder_scene(device: &wgpu::Device) -> (wgpu::TextureView, wgpu::TextureView) {
    let texture = |label: &str, format: wgpu::TextureFormat| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    };
    (
        texture("ocean placeholder scene colour", HDR_FORMAT),
        texture("ocean placeholder scene depth", DEPTH_FORMAT),
    )
}

/// No cloud shadow: a 1×1 map, a sampler and zeroed uniforms, whose `enabled` 0 leaves the sun whole.
pub(crate) fn placeholder_cloud_shadow(
    device: &wgpu::Device,
) -> (wgpu::TextureView, wgpu::Sampler, wgpu::Buffer) {
    let map = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("ocean placeholder cloud shadow"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("ocean placeholder cloud shadow"),
        ..Default::default()
    });
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ocean placeholder cloud shadow"),
        size: std::mem::size_of::<naval_clouds::CloudShadowUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM,
        mapped_at_creation: false,
    });
    (map, sampler, uniforms)
}
