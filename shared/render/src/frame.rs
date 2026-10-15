//! The renderer and the frame it records: explicit passes over owned targets.
//!
//! A frame is, in order:
//!
//! 1. **Prepare.** Each [`SceneLayer`]'s `prepare` runs with the frame's command
//!    encoder: uniform writes, compute work, its own offscreen passes.
//! 2. **Sun shadows.** When shadows are on and a [`Sun`] is set, each shadow map
//!    due this frame is a depth-only pass drawn by every layer's
//!    [`SceneLayer::draw_shadow`] (see [`crate::shadows`]).
//! 3. **Scene pass.** Render passes into `scene.hdr` (`Rgba16Float`, cleared
//!    to black) with `scene.depth` (`Depth32Float`, reversed, cleared to 0).
//!    The camera is bound at group 0, then every layer draws in each [`Phase`]:
//!    all layers' `Opaque`, then all `Water`, `Sky` and `Transparent`. Where a
//!    layer asked for scene copies ([`SceneLayer::scene_copies`]), the pass ends
//!    after `Opaque` or after `Sky`, the copies are taken, layers may record
//!    their own passes on them ([`SceneLayer::after_copies`]), and the scene
//!    carries on in a new pass that loads what was drawn (see [`crate::copies`]).
//! 4. **Post.** The eye adaptation meters `scene.hdr` against the sky's mean
//!    radiance and steps its exposure; bloom blurs the graded highlights (see
//!    [`crate::post`]).
//! 5. **Display pass.** `scene.hdr` through the look (with the eye's exposure),
//!    the bloom and AgX onto the output (see [`crate::display`]). An 8- or 10-bit
//!    output is dithered by whichever pass writes it (see [`crate::dither`]); a float
//!    output takes values above SDR white when the renderer has a headroom above 1
//!    ([`Renderer::set_headroom`]: HDR output).
//!
//! With temporal anti-aliasing on (see [`crate::taa`]), the camera is jittered,
//! a motion pass follows the scene passes ([`SceneLayer::draw_motion`]), the
//! display pass writes `display.frame`, and a resolve and an output pass put the
//! result on the output. FXAA and SMAA also read `display.frame`, and below full
//! render scale the whole frame is drawn smaller and upscaled at the end (see
//! [`crate::antialiasing`]): by temporal AA's resolve at the output's size (temporal
//! upscaling), or bilinearly after FXAA or SMAA. Overlays ([`SceneLayer::draw_overlay`])
//! go on last, at the output's full resolution.
//!
//! External systems (ships, the ocean, the sky) plug in as [`SceneLayer`]s:
//! they build their pipelines through a [`RenderContext`] (so hot reload
//! reaches them) and are handed to [`Renderer::render`] each frame.

use bytemuck::bytes_of;

use crate::aerial::AerialVolume;
use crate::antialiasing::{
    Antialiasing, OutputStage, POST_FRAME, PostChain, StageTarget, clamp_render_scale, scene_size,
};
use crate::camera::{Camera, CameraMotion, CameraUniforms, FrameTime};
use crate::copies::{CopyPoint, SceneCopies};
use crate::debug_view::{DebugView, DebugViews};
use crate::display::{DisplayInputs, DisplayPass, DisplaySettings};
use crate::dither::Dither;
use crate::pipelines::{
    Pipeline, PipelineId, PipelineRecipe, PipelineRegistry, PipelineStatus, PlaceholderPipelines,
    RebuildReport,
};
use crate::post::{
    Bloom, BloomSettings, EYE_STATE_BUFFER, ExposureControl, EyeAdaptation, EyeAdaptationSettings,
    SKY_MEAN_BUFFER, SkyMean,
};
use crate::resources::{Resources, TextureDesc};
use crate::shaders::{ShaderDefs, ShaderError, ShaderFile, ShaderLibrary};
use crate::shadows::{
    CasterPipeline, ShadowFit, ShadowMap, ShadowReport, ShadowSettings, ShadowTargets, Sun,
    caster_depth_state,
};
use crate::taa::{
    DISPLAY_FRAME, MotionPipeline, SCENE_MOTION, TAA_RESOLVED, Taa, TaaSettings,
    motion_depth_state, motion_target,
};
use crate::timing::GpuTimer;
use crate::upload::{UploadSink, UploadStats, Uploads};

/// The scene's color target: linear HDR radiance.
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The scene's depth target: reversed floating-point depth.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Reversed depth: nearer is greater.
pub const DEPTH_COMPARE: wgpu::CompareFunction = wgpu::CompareFunction::GreaterEqual;
/// Reversed depth: the far plane.
pub const DEPTH_CLEAR: f32 = 0.0;
/// Registry name of the scene's HDR color target.
pub const SCENE_HDR: &str = "scene.hdr";
/// Registry name of the scene's depth target.
pub const SCENE_DEPTH: &str = "scene.depth";

/// Whether an output of `format` holds values above SDR white: a float format, as an HDR surface
/// (`Rgba16Float` in extended sRGB) or the HDR capture is. Only such an output takes the
/// renderer's headroom ([`Renderer::set_headroom`]); any other is drawn for SDR.
pub fn extended_range(format: wgpu::TextureFormat) -> bool {
    matches!(
        format,
        wgpu::TextureFormat::Rgba16Float | wgpu::TextureFormat::Rgba32Float
    )
}

/// When in the scene pass a layer's draws run. Every layer's `Opaque` draws
/// run before any `Water` draw, and so on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Phase {
    /// Depth-writing geometry: hulls, terrain, islands.
    Opaque,
    /// Depth-writing surfaces that see the opaque scene through themselves: the
    /// sea. Drawn after the opaque copies ([`CopyPoint::Opaque`]), which it may
    /// sample, and before the sky, so the sky fills only what neither covered.
    /// The removed web game drew its sea first in the transparent queue for the same reason.
    Water,
    /// Backgrounds at the far plane (depth 0), drawn where opaque geometry and
    /// water left the clear: `GreaterEqual` against 0 passes only there.
    Sky,
    /// Blended geometry tested against, but not writing, depth: smoke, spray,
    /// tracers. Drawn after the backdrop copies ([`CopyPoint::Backdrop`]).
    Transparent,
}

impl Phase {
    pub const ALL: [Phase; 4] = [Phase::Opaque, Phase::Water, Phase::Sky, Phase::Transparent];

    pub fn name(self) -> &'static str {
        match self {
            Phase::Opaque => "opaque",
            Phase::Water => "water",
            Phase::Sky => "sky",
            Phase::Transparent => "transparent",
        }
    }

    /// The copy point that follows this phase, if any.
    pub fn copy_point_after(self) -> Option<CopyPoint> {
        match self {
            Phase::Opaque => Some(CopyPoint::Opaque),
            Phase::Sky => Some(CopyPoint::Backdrop),
            Phase::Water | Phase::Transparent => None,
        }
    }
}

/// What a layer's `prepare` gets.
pub struct PrepareContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    /// The frame's encoder, ahead of the scene pass: a layer may record its own
    /// compute or render passes here.
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub pipelines: &'a PipelineRegistry,
    /// Mutable so a layer can make [`crate::resources::Lifetime::Frame`] resources.
    pub resources: &'a mut Resources,
    pub camera: &'a CameraUniforms,
    /// Group 0 of the scene pass (the camera, the sun's shadow maps, the aerial volume), for a layer's own pass drawn
    /// with pipelines from [`RenderContext::add_scene_pipeline_into`]. Before the scene pass it holds this frame's
    /// camera, though the aerial volume is written only by the sky's `prepare`: read it at a copy point.
    pub camera_group: &'a wgpu::BindGroup,
    pub time: FrameTime,
    pub viewport: (u32, u32),
    /// Timestamps for a layer's own passes: put `timer.compute_pass("ocean field")`
    /// (or `render_pass`) in the pass descriptor's `timestamp_writes`, and the pass
    /// shows in the frame's GPU timings.
    pub timer: &'a GpuTimer,
    /// Buffer uploads through the renderer's staging belt: `queue.write_buffer`'s
    /// ordering (in place before the frame's passes) without a fresh staging buffer
    /// per call, so a frame's large uploads touch no fresh pages ([`crate::upload`]).
    pub uploads: &'a mut Uploads,
    /// The sun's shadow maps as fitted for this frame (near, wide, view 1, view 2: `on` where sampled, `drawn` where
    /// their casters are drawn after `prepare`), so a layer can place casters for them (the land's trees).
    pub shadow_maps: &'a [ShadowMap],
    /// The aerial-perspective volume every material reads at group 0 ([`crate::aerial`]): the sky's layer writes
    /// it in its `prepare`, for this frame's camera.
    pub aerial: &'a AerialVolume,
    /// Soft occluders in the sun's light (smoke): zeroed before the layers prepare, written by a layer that draws
    /// them ([`crate::shadows::SunOccluders`]).
    pub sun_occluders: &'a crate::shadows::SunOccluders,
}

/// What a layer's `draw` gets. The camera is already bound at group 0.
pub struct DrawContext<'a> {
    pub pipelines: &'a PipelineRegistry,
    pub resources: &'a Resources,
    pub camera: &'a CameraUniforms,
    pub time: FrameTime,
    pub viewport: (u32, u32),
}

/// What a layer's `draw_overlay` gets. Group 0 is bound as in the scene pass (the camera, and
/// `naval::motion::camera_motion` beside it: project with its `unjittered_view_projection`, since
/// the camera block carries temporal AA's jitter).
pub struct OverlayContext<'a> {
    pub pipelines: &'a PipelineRegistry,
    pub resources: &'a Resources,
    pub camera: &'a CameraUniforms,
    pub time: FrameTime,
    /// The output's size: overlays draw at full resolution whatever the render scale.
    pub viewport: (u32, u32),
    /// The scene's size (the output's times the render scale).
    pub scene_viewport: (u32, u32),
    /// The output's format.
    pub format: wgpu::TextureFormat,
    overlays: &'a [OverlayEntry],
}

impl OverlayContext<'_> {
    /// The overlay pipeline `id` built for this frame's output format.
    pub fn pipeline(&self, id: OverlayPipelineId) -> &wgpu::RenderPipeline {
        let built = self.overlays[id.0]
            .built
            .iter()
            .find(|(format, _)| *format == self.format)
            .expect("built for the output's format before the overlay pass")
            .1;
        self.pipelines.render(built)
    }
}

/// A pipeline for the overlay pass, built with [`RenderContext::add_overlay_pipeline`]: the
/// camera at group 0 and the layer's groups after it, no depth, onto the output. It is built for
/// each output format it meets; for a format that does not encode sRGB in hardware it gets the
/// `ENCODE_SRGB` define (apply `naval::color::srgb_oetf` to the display-space colour there).
#[derive(Clone, Debug)]
pub struct OverlayPipeline {
    pub vertex_entry: &'static str,
    pub fragment_entry: &'static str,
    pub vertex_buffers: Vec<VertexLayout>,
    /// Groups 1, 2, ...
    pub bind_group_layouts: Vec<wgpu::BindGroupLayout>,
    pub primitive: wgpu::PrimitiveState,
    /// Premultiplied alpha over the frame by default.
    pub blend: Option<wgpu::BlendState>,
    pub defs: ShaderDefs,
}

impl OverlayPipeline {
    /// A triangle list with no vertex buffers, blended premultiplied over the frame.
    pub fn new(vertex_entry: &'static str, fragment_entry: &'static str) -> Self {
        Self {
            vertex_entry,
            fragment_entry,
            vertex_buffers: Vec::new(),
            bind_group_layouts: Vec::new(),
            primitive: wgpu::PrimitiveState::default(),
            blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            defs: Vec::new(),
        }
    }
}

/// An overlay pipeline registered with [`RenderContext::add_overlay_pipeline`]; look it up with
/// [`OverlayContext::pipeline`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OverlayPipelineId(usize);

pub(crate) struct OverlayEntry {
    label: String,
    shader: String,
    desc: OverlayPipeline,
    built: Vec<(wgpu::TextureFormat, PipelineId)>,
}

impl OverlayEntry {
    fn ensure(
        &mut self,
        device: &wgpu::Device,
        library: &mut ShaderLibrary,
        pipelines: &mut PipelineRegistry,
        camera_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
    ) -> Result<PipelineId, ShaderError> {
        if let Some((_, id)) = self.built.iter().find(|(built, _)| *built == format) {
            return Ok(*id);
        }
        let mut defs = self.desc.defs.clone();
        if !format.is_srgb() {
            defs.push(("ENCODE_SRGB".to_owned(), crate::ShaderDefValue::Bool(true)));
        }
        let desc = self.desc.clone();
        let camera_layout = camera_layout.clone();
        let recipe: PipelineRecipe =
            Box::new(move |build| {
                let mut groups: Vec<Option<&wgpu::BindGroupLayout>> = vec![Some(&camera_layout)];
                groups.extend(desc.bind_group_layouts.iter().map(Some));
                let layout = build
                    .device
                    .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some(build.label),
                        bind_group_layouts: &groups,
                        immediate_size: 0,
                    });
                let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> = desc
                    .vertex_buffers
                    .iter()
                    .map(|buffer| {
                        Some(wgpu::VertexBufferLayout {
                            array_stride: buffer.stride,
                            step_mode: buffer.step_mode,
                            attributes: &buffer.attributes,
                        })
                    })
                    .collect();
                Pipeline::Render(build.device.create_render_pipeline(
                    &wgpu::RenderPipelineDescriptor {
                        label: Some(build.label),
                        layout: Some(&layout),
                        vertex: wgpu::VertexState {
                            module: build.module,
                            entry_point: Some(desc.vertex_entry),
                            compilation_options: Default::default(),
                            buffers: &buffers,
                        },
                        primitive: desc.primitive,
                        depth_stencil: None,
                        multisample: wgpu::MultisampleState::default(),
                        fragment: Some(wgpu::FragmentState {
                            module: build.module,
                            entry_point: Some(desc.fragment_entry),
                            compilation_options: Default::default(),
                            targets: &[Some(wgpu::ColorTargetState {
                                format,
                                blend: desc.blend,
                                write_mask: wgpu::ColorWrites::ALL,
                            })],
                        }),
                        multiview_mask: None,
                        cache: build.cache,
                    },
                ))
            });
        let id = pipelines.add(
            device,
            library,
            format!("{} to {format:?}", self.label),
            &self.shader,
            defs,
            recipe,
        )?;
        self.built.push((format, id));
        Ok(id)
    }
}

/// Something that draws into the HDR scene with depth.
///
/// Rules for a layer:
/// - Build pipelines with [`RenderContext::add_scene_pipeline`] (or match
///   [`scene_depth_state`], [`HDR_FORMAT`] and the camera at group 0 in a
///   custom recipe), so they fit the scene pass and hot reload rebuilds them.
/// - Set bind groups from 1 up; group 0 is the camera and is rebound before
///   each layer draws.
/// - Write radiance in linear HDR; never tone map. The display pass does that.
pub trait SceneLayer {
    /// A short name for GPU debug groups and telemetry.
    fn label(&self) -> &str;

    /// Work before the scene pass. The default does nothing.
    fn prepare(&mut self, ctx: &mut PrepareContext<'_>) {
        let _ = ctx;
    }

    /// Records this layer's draws for `phase` into the scene pass.
    fn draw(&self, phase: Phase, pass: &mut wgpu::RenderPass<'_>, ctx: &DrawContext<'_>);

    /// The scene copies this layer reads this frame (see [`crate::copies`]). Asked
    /// every frame before `prepare`; a copy is taken when any layer asks for it.
    /// The default asks for none.
    fn scene_copies(&self) -> SceneCopies {
        SceneCopies::NONE
    }

    /// Records this layer's shadow casters into one sun shadow map's depth-only
    /// pass. Group 0 is bound to that map's shadow camera (`naval::camera::camera`
    /// holds its orthographic matrices; `ctx.camera` is the same block), so a
    /// layer's usual vertex code places casters. Use pipelines from
    /// [`RenderContext::add_caster_pipeline`]. Called for every map drawn this
    /// frame (`map.kind` says which; `map.view` and `map.projection` bound it for
    /// culling). The default casts nothing.
    fn draw_shadow(&self, map: &ShadowMap, pass: &mut wgpu::RenderPass<'_>, ctx: &DrawContext<'_>) {
        let _ = (map, pass, ctx);
    }

    /// Records this layer's moving surfaces into the motion pass, drawn after the
    /// scene when temporal anti-aliasing is on (see [`crate::taa`]): object motion
    /// of what moves relative to the world (`naval::motion::surface_motion`), or a
    /// response for surfaces whose pixels should take less history or none
    /// (`motion_response(1.0)` for the sea and tracers). Use pipelines from
    /// [`RenderContext::add_motion_pipeline`]. Static geometry draws nothing here:
    /// the resolve reconstructs camera motion from depth. The default draws nothing.
    fn draw_motion(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &DrawContext<'_>) {
        let _ = (pass, ctx);
    }

    /// Records responses into the motion pass after every layer's [`SceneLayer::draw_motion`]:
    /// for a layer drawn early whose transparent surfaces lie in front of later layers' (the
    /// sky's rain in front of a hull, whose motion replaces what is below it). Use
    /// `motion_response`, which adds. The default draws nothing.
    fn draw_motion_over(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &DrawContext<'_>) {
        let _ = (pass, ctx);
    }

    /// Work at a copy point, after that point's copies are taken and before the
    /// scene pass resumes: a layer's own passes on the copies (a depth pyramid for
    /// screen-space reflections, a downsampled depth for half-resolution
    /// particles). Runs only at points where copies were taken this frame. The
    /// default does nothing.
    fn after_copies(&mut self, point: CopyPoint, ctx: &mut PrepareContext<'_>) {
        let _ = (point, ctx);
    }

    /// Whether this layer draws in the overlay pass this frame. The pass is recorded only when
    /// some layer says so. The default is false.
    fn has_overlay(&self) -> bool {
        false
    }

    /// Records this layer's overlay: drawn after the post chain onto the output, at its full
    /// resolution whatever the render scale, without depth, temporal AA or edge smoothing, as
    /// the page sits over the scene. Colours are display space (after tone
    /// mapping). Use pipelines from [`RenderContext::add_overlay_pipeline`], looked up with
    /// [`OverlayContext::pipeline`]. Called only when [`SceneLayer::has_overlay`] is true.
    fn draw_overlay(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &OverlayContext<'_>) {
        let _ = (pass, ctx);
    }
}

/// A vertex buffer layout that owns its attributes, for [`ScenePipeline`].
#[derive(Clone, Debug)]
pub struct VertexLayout {
    pub stride: u64,
    pub step_mode: wgpu::VertexStepMode,
    pub attributes: Vec<wgpu::VertexAttribute>,
}

/// A pipeline drawn in the scene pass: HDR color, reversed depth, the camera
/// at group 0 and the layer's own bind groups after it.
#[derive(Clone, Debug)]
pub struct ScenePipeline {
    pub vertex_entry: &'static str,
    pub fragment_entry: &'static str,
    pub vertex_buffers: Vec<VertexLayout>,
    /// Groups 1, 2, …; group 0 is always the camera.
    pub bind_group_layouts: Vec<wgpu::BindGroupLayout>,
    pub primitive: wgpu::PrimitiveState,
    /// `true` for opaque geometry, `false` for the sky and transparents.
    pub depth_write: bool,
    pub blend: Option<wgpu::BlendState>,
    pub defs: ShaderDefs,
}

impl ScenePipeline {
    /// An opaque, back-face-culled triangle list with no vertex buffers.
    pub fn new(vertex_entry: &'static str, fragment_entry: &'static str) -> Self {
        Self {
            vertex_entry,
            fragment_entry,
            vertex_buffers: Vec::new(),
            bind_group_layouts: Vec::new(),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_write: true,
            blend: None,
            defs: Vec::new(),
        }
    }
}

/// The scene pass's depth state: reversed depth, `GreaterEqual`.
pub fn scene_depth_state(write: bool) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(write),
        depth_compare: Some(DEPTH_COMPARE),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

/// Group 0: the camera's uniform buffer, and the sun's shadow maps beside it.
struct CameraBinding {
    /// Group 0 of scene pipelines: the camera (binding 0) and the shadow maps (1 to 4).
    layout: wgpu::BindGroupLayout,
    /// Group 0 of caster pipelines: a camera block alone (the map's shadow camera).
    caster_layout: wgpu::BindGroupLayout,
    buffer: wgpu::Buffer,
    /// `CameraMotion` at binding 5.
    motion: wgpu::Buffer,
    /// The aerial-perspective volume at bindings 6 and 7.
    aerial: AerialVolume,
    group: wgpu::BindGroup,
    /// The shadow targets' version `group` was built on.
    shadows_version: u64,
}

fn camera_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<CameraUniforms>() as u64),
        },
        count: None,
    }
}

impl CameraBinding {
    fn new(device: &wgpu::Device) -> (Self, ShadowTargets) {
        let mut entries = vec![camera_entry()];
        entries.extend(crate::shadows::view_layout_entries());
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 5,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<CameraMotion>() as u64),
            },
            count: None,
        });
        entries.extend(AerialVolume::layout_entries());
        entries.extend(crate::shadows::occluder_layout_entries());
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera and shadows"),
            entries: &entries,
        });
        let caster_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow caster camera"),
            entries: &[camera_entry()],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera"),
            size: std::mem::size_of::<CameraUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let motion = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera motion"),
            size: std::mem::size_of::<CameraMotion>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadows = ShadowTargets::new(device, &caster_layout);
        let aerial = AerialVolume::new(device);
        let group = Self::make_group(device, &layout, &buffer, &motion, &shadows, &aerial);
        (
            Self {
                layout,
                caster_layout,
                buffer,
                motion,
                aerial,
                group,
                shadows_version: shadows.version,
            },
            shadows,
        )
    }

    fn make_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        buffer: &wgpu::Buffer,
        motion: &wgpu::Buffer,
        shadows: &ShadowTargets,
        aerial: &AerialVolume,
    ) -> wgpu::BindGroup {
        let [aerial_volume, aerial_sampler] = aerial.group_entries();
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera and shadows"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: shadows.uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&shadows.cascades_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&shadows.wide_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&shadows.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: motion.as_entire_binding(),
                },
                aerial_volume,
                aerial_sampler,
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: shadows.occluders.texels.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: shadows.occluders.uniforms.as_entire_binding(),
                },
            ],
        })
    }

    /// Rebuilds the group when the shadow maps were remade.
    fn follow(&mut self, device: &wgpu::Device, shadows: &ShadowTargets) {
        if self.shadows_version != shadows.version {
            self.group = Self::make_group(
                device,
                &self.layout,
                &self.buffer,
                &self.motion,
                shadows,
                &self.aerial,
            );
            self.shadows_version = shadows.version;
        }
    }
}

/// What a layer is built with: the device, the shader library and the
/// pipeline registry, the resource registry and the camera's layout.
pub struct RenderContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub library: &'a mut ShaderLibrary,
    pub pipelines: &'a mut PipelineRegistry,
    pub resources: &'a mut Resources,
    /// Group 0 of every scene pipeline: the camera at binding 0 and the sun's
    /// shadow maps at bindings 1 to 4, their soft occluders at 8 and 9
    /// (`naval::shadow`). Also usable in compute pipelines.
    pub camera_layout: &'a wgpu::BindGroupLayout,
    /// Group 0 of shadow caster pipelines: a camera block alone, holding the
    /// map's shadow camera.
    pub caster_layout: &'a wgpu::BindGroupLayout,
    /// The output format overlay pipelines are first built for.
    pub output_format: wgpu::TextureFormat,
    /// The aerial-perspective volume group 0 binds (bindings 6 and 7, [`crate::aerial`]).
    pub aerial: &'a AerialVolume,
    overlays: &'a mut Vec<OverlayEntry>,
    placeholders: &'a std::sync::OnceLock<PlaceholderPipelines>,
}

impl RenderContext<'_> {
    /// Stand-ins for pipelines a layer registered and takes from the registry at its first prepare
    /// (see [`PlaceholderPipelines`]); made on first use, once per renderer.
    pub fn placeholders(&self) -> &PlaceholderPipelines {
        self.placeholders
            .get_or_init(|| PlaceholderPipelines::new(self.device))
    }

    /// Waits for every pipeline still compiling on a build thread (see
    /// [`PipelineRegistry::finish`]).
    pub fn finish_pipelines(&mut self) -> Result<(), ShaderError> {
        self.pipelines.finish()
    }

    /// Registers a WGSL file (a module to import or an entry shader).
    pub fn add_shader(&mut self, file: ShaderFile) -> Result<(), ShaderError> {
        self.library.add(file)
    }

    /// Builds a pipeline for the scene pass from the entry shader at `shader`.
    pub fn add_scene_pipeline(
        &mut self,
        label: &str,
        shader: &str,
        desc: ScenePipeline,
    ) -> Result<PipelineId, ShaderError> {
        let target = wgpu::ColorTargetState {
            format: HDR_FORMAT,
            blend: desc.blend,
            write_mask: wgpu::ColorWrites::ALL,
        };
        self.add_scene_pipeline_into(label, shader, desc, vec![Some(target)])
    }

    /// As [`RenderContext::add_scene_pipeline`], drawing into `targets` instead of the scene's color (`desc.blend` is
    /// then unused): for a layer's own pass with the scene's camera and depth, recorded at a copy point
    /// ([`SceneLayer::after_copies`], binding [`PrepareContext::camera_group`] at group 0).
    pub fn add_scene_pipeline_into(
        &mut self,
        label: &str,
        shader: &str,
        desc: ScenePipeline,
        targets: Vec<Option<wgpu::ColorTargetState>>,
    ) -> Result<PipelineId, ShaderError> {
        let camera_layout = self.camera_layout.clone();
        let defs = desc.defs.clone();
        // Opaque triangles may be drawn as lines by the wireframe debug view.
        let wireframe = desc.depth_write
            && desc.primitive.polygon_mode == wgpu::PolygonMode::Fill
            && matches!(
                desc.primitive.topology,
                wgpu::PrimitiveTopology::TriangleList | wgpu::PrimitiveTopology::TriangleStrip
            );
        let recipe: PipelineRecipe =
            Box::new(move |build| {
                let mut groups: Vec<Option<&wgpu::BindGroupLayout>> = vec![Some(&camera_layout)];
                groups.extend(desc.bind_group_layouts.iter().map(Some));
                let layout = build
                    .device
                    .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some(build.label),
                        bind_group_layouts: &groups,
                        immediate_size: 0,
                    });
                let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> = desc
                    .vertex_buffers
                    .iter()
                    .map(|buffer| {
                        Some(wgpu::VertexBufferLayout {
                            array_stride: buffer.stride,
                            step_mode: buffer.step_mode,
                            attributes: &buffer.attributes,
                        })
                    })
                    .collect();
                let mut primitive = desc.primitive;
                if build.polygon_mode != wgpu::PolygonMode::Fill {
                    primitive.polygon_mode = build.polygon_mode;
                }
                Pipeline::Render(build.device.create_render_pipeline(
                    &wgpu::RenderPipelineDescriptor {
                        label: Some(build.label),
                        layout: Some(&layout),
                        vertex: wgpu::VertexState {
                            module: build.module,
                            entry_point: Some(desc.vertex_entry),
                            compilation_options: Default::default(),
                            buffers: &buffers,
                        },
                        primitive,
                        depth_stencil: Some(scene_depth_state(desc.depth_write)),
                        multisample: wgpu::MultisampleState::default(),
                        fragment: Some(wgpu::FragmentState {
                            module: build.module,
                            entry_point: Some(desc.fragment_entry),
                            compilation_options: Default::default(),
                            targets: &targets,
                        }),
                        multiview_mask: None,
                        cache: build.cache,
                    },
                ))
            });
        let id = self
            .pipelines
            .add(self.device, self.library, label, shader, defs, recipe)?;
        if wireframe {
            self.pipelines
                .allow_wireframe(self.device, self.library, id);
        }
        Ok(id)
    }

    /// Lets the wireframe debug view draw pipeline `id` as lines (see
    /// [`PipelineRegistry::allow_wireframe`]): for a scene pipeline of opaque triangles built with a recipe of its
    /// own, which puts [`crate::PipelineBuild::polygon_mode`] in its primitive state.
    /// [`RenderContext::add_scene_pipeline`] allows it by itself for opaque triangles.
    pub fn allow_wireframe(&mut self, id: PipelineId) {
        self.pipelines
            .allow_wireframe(self.device, self.library, id);
    }

    /// Builds a pipeline that draws into the sun's shadow maps from the entry
    /// shader at `shader` (see [`crate::shadows::CasterPipeline`]).
    pub fn add_caster_pipeline(
        &mut self,
        label: &str,
        shader: &str,
        desc: CasterPipeline,
    ) -> Result<PipelineId, ShaderError> {
        let caster_layout = self.caster_layout.clone();
        let defs = desc.defs.clone();
        let recipe: PipelineRecipe =
            Box::new(move |build| {
                let mut groups: Vec<Option<&wgpu::BindGroupLayout>> = vec![Some(&caster_layout)];
                groups.extend(desc.bind_group_layouts.iter().map(Some));
                let layout = build
                    .device
                    .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some(build.label),
                        bind_group_layouts: &groups,
                        immediate_size: 0,
                    });
                let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> = desc
                    .vertex_buffers
                    .iter()
                    .map(|buffer| {
                        Some(wgpu::VertexBufferLayout {
                            array_stride: buffer.stride,
                            step_mode: buffer.step_mode,
                            attributes: &buffer.attributes,
                        })
                    })
                    .collect();
                Pipeline::Render(build.device.create_render_pipeline(
                    &wgpu::RenderPipelineDescriptor {
                        label: Some(build.label),
                        layout: Some(&layout),
                        vertex: wgpu::VertexState {
                            module: build.module,
                            entry_point: Some(desc.vertex_entry),
                            compilation_options: Default::default(),
                            buffers: &buffers,
                        },
                        primitive: desc.primitive,
                        depth_stencil: Some(caster_depth_state()),
                        multisample: wgpu::MultisampleState::default(),
                        fragment: desc.fragment_entry.map(|entry| wgpu::FragmentState {
                            module: build.module,
                            entry_point: Some(entry),
                            compilation_options: Default::default(),
                            targets: &[],
                        }),
                        multiview_mask: None,
                        cache: build.cache,
                    },
                ))
            });
        self.pipelines
            .add(self.device, self.library, label, shader, defs, recipe)
    }

    /// Builds a pipeline for the motion pass from the entry shader at `shader`
    /// (see [`crate::taa::MotionPipeline`]): the camera at group 0, the motion
    /// target with its blend ([`crate::taa::motion_target`]), `scene.depth` read-only.
    pub fn add_motion_pipeline(
        &mut self,
        label: &str,
        shader: &str,
        desc: MotionPipeline,
    ) -> Result<PipelineId, ShaderError> {
        let camera_layout = self.camera_layout.clone();
        let defs = desc.defs.clone();
        let recipe: PipelineRecipe =
            Box::new(move |build| {
                let mut groups: Vec<Option<&wgpu::BindGroupLayout>> = vec![Some(&camera_layout)];
                groups.extend(desc.bind_group_layouts.iter().map(Some));
                let layout = build
                    .device
                    .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some(build.label),
                        bind_group_layouts: &groups,
                        immediate_size: 0,
                    });
                let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> = desc
                    .vertex_buffers
                    .iter()
                    .map(|buffer| {
                        Some(wgpu::VertexBufferLayout {
                            array_stride: buffer.stride,
                            step_mode: buffer.step_mode,
                            attributes: &buffer.attributes,
                        })
                    })
                    .collect();
                Pipeline::Render(build.device.create_render_pipeline(
                    &wgpu::RenderPipelineDescriptor {
                        label: Some(build.label),
                        layout: Some(&layout),
                        vertex: wgpu::VertexState {
                            module: build.module,
                            entry_point: Some(desc.vertex_entry),
                            compilation_options: Default::default(),
                            buffers: &buffers,
                        },
                        primitive: desc.primitive,
                        depth_stencil: Some(motion_depth_state()),
                        multisample: wgpu::MultisampleState::default(),
                        fragment: Some(wgpu::FragmentState {
                            module: build.module,
                            entry_point: Some(desc.fragment_entry),
                            compilation_options: Default::default(),
                            targets: &[Some(motion_target())],
                        }),
                        multiview_mask: None,
                        cache: build.cache,
                    },
                ))
            });
        self.pipelines
            .add(self.device, self.library, label, shader, defs, recipe)
    }

    /// Builds a pipeline for the overlay pass from the entry shader at `shader` (see
    /// [`OverlayPipeline`]), for the renderer's output format now and for any other format an
    /// output brings later.
    pub fn add_overlay_pipeline(
        &mut self,
        label: &str,
        shader: &str,
        desc: OverlayPipeline,
    ) -> Result<OverlayPipelineId, ShaderError> {
        let mut entry = OverlayEntry {
            label: label.to_owned(),
            shader: shader.to_owned(),
            desc,
            built: Vec::new(),
        };
        entry.ensure(
            self.device,
            self.library,
            self.pipelines,
            self.camera_layout,
            self.output_format,
        )?;
        self.overlays.push(entry);
        Ok(OverlayPipelineId(self.overlays.len() - 1))
    }

    /// Makes the textures for `copies` now, so a layer can build bind groups on
    /// them when it is created (they are made on first request anyway). They are
    /// viewport textures: check their `version` before reusing a bind group.
    pub fn request_scene_copies(&mut self, copies: SceneCopies) {
        ensure_scene_copies(self.device, self.resources, copies);
    }

    /// Builds any other pipeline (compute, shadow maps, offscreen targets) from
    /// the entry shader at `shader` with a recipe of the caller's own.
    pub fn add_pipeline(
        &mut self,
        label: &str,
        shader: &str,
        defs: ShaderDefs,
        recipe: PipelineRecipe,
    ) -> Result<PipelineId, ShaderError> {
        self.pipelines
            .add(self.device, self.library, label, shader, defs, recipe)
    }
}

/// The camera and clock of one frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameInput<'a> {
    pub camera: &'a Camera,
    pub time: FrameTime,
}

/// Where the display pass writes: a surface texture or a capture target.
#[derive(Clone, Copy, Debug)]
pub struct RenderOutput<'a> {
    pub view: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub width: u32,
    pub height: u32,
}

/// The scene pass's groups: each group's phases, the copy point after it, and its pass's label.
type SceneGroups = Vec<(Vec<Phase>, Option<CopyPoint>, String)>;

/// What a frame did, for telemetry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameReport {
    pub frame: u64,
    pub time: f64,
    pub viewport: (u32, u32),
    /// Layer labels in draw order.
    pub layers: Vec<String>,
    /// The scene copies taken.
    pub copies: SceneCopies,
    /// The scene's render passes, by the phases each drew.
    pub scene_passes: Vec<String>,
    /// The sun shadow maps drawn this frame.
    pub shadow_maps: Vec<&'static str>,
}

/// Files changed by hot reload, and what rebuilding their pipelines did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReloadReport {
    /// Library paths whose text changed and composed.
    pub changed: Vec<String>,
    /// Modules that no longer compose; the library keeps their last good build.
    pub library_errors: Vec<(String, String)>,
    pub pipelines: RebuildReport,
}

impl ReloadReport {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.library_errors.is_empty()
    }
}

/// The renderer: shader library, pipelines, resources, camera, and the scene
/// and display passes.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    library: ShaderLibrary,
    pipelines: PipelineRegistry,
    resources: Resources,
    camera: CameraBinding,
    shadow_fit: ShadowFit,
    shadow_targets: ShadowTargets,
    sun: Option<Sun>,
    timer: GpuTimer,
    /// The frame's buffer uploads, submitted ahead of it.
    uploads: Uploads,
    /// The scene pass's groups of phases and their labels for the copies last asked for (worked out again only
    /// when the copies change), and a report handed back for reuse ([`Renderer::recycle_report`]).
    scene_groups: (SceneCopies, SceneGroups),
    spare_report: Option<FrameReport>,
    /// What the last frame uploaded through them.
    upload_stats: UploadStats,
    /// Copies taken whatever the layers ask for (measurement and debugging).
    extra_copies: SceneCopies,
    /// Frames submitted, for the timer (a capture renders one clock frame twice).
    submitted: u64,
    display: DisplayPass,
    eye: EyeAdaptation,
    bloom: Bloom,
    bloom_on: bool,
    /// The sun's glare in the eye, which the bloom's composite adds (`crate::sun_glare`).
    sun_glare: crate::sun_glare::SunGlare,
    taa: Taa,
    antialiasing: Antialiasing,
    /// The scene's size over the output's (see [`crate::antialiasing`]).
    render_scale: f32,
    chain: PostChain,
    overlays: Vec<OverlayEntry>,
    /// The latest output's format, which new overlay pipelines are built for.
    output_format: wgpu::TextureFormat,
    /// A black texture the display reads without bloom.
    black: (wgpu::Texture, wgpu::TextureView),
    /// The host's underwater probe and how often it has changed ([`Renderer::set_underwater`]).
    underwater: (Option<wgpu::TextureView>, u64),
    /// The output dither's mask and phase.
    dither: Dither,
    /// The display's headroom over SDR white, which a float output is drawn for (1: SDR).
    headroom: f32,
    #[cfg(feature = "hot-reload")]
    watcher: crate::hot_reload::ShaderWatcher,
    placeholders: std::sync::OnceLock<PlaceholderPipelines>,
    /// The developer panel's debug view.
    debug: DebugViews,
    /// Photo mode's post (`crate::photo`): its depth of field's passes, and the finish the display gives the frame.
    dof: crate::photo::DepthOfFieldPass,
    /// Photo mode's lens flare and glare (`crate::photo_optics`).
    optics: crate::photo_optics::OpticsPass,
    photo: Option<crate::photo::PhotoPost>,
    /// Photo mode's shutter (`crate::shutter`) and the moving bodies it blurs.
    shutter: crate::shutter::ShutterPass,
    photo_bodies: Vec<crate::shutter::MovingBody>,
    /// The game's look (the Graphics menu's Look); photo mode's replaces it while photo mode is up.
    look: crate::photo::Look,
    /// The look the last frame was graded with: temporal AA's history holds its colours.
    drawn_look: crate::photo::Look,
    /// The last frame's camera planes, near and far, for the Look tab's previews of it (`crate::look_preview`).
    clip_planes: [f32; 2],
    /// The look's film after edge smoothing (`crate::film`).
    film: crate::film::FilmPass,
}

impl Renderer {
    /// A renderer whose scene targets start at `width` × `height`; they follow
    /// the output's size after that. Builds the display pipeline for
    /// `output_format` up front.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        output_format: wgpu::TextureFormat,
    ) -> Result<Self, ShaderError> {
        Self::with_pipeline_cache(device, queue, width, height, output_format, None)
    }

    /// [`Renderer::new`] with a pipeline cache every pipeline it and its layers build is handed (Vulkan's
    /// `Features::PIPELINE_CACHE`; see [`crate::pipelines::PipelineBuild::cache`]), its own post passes included.
    pub fn with_pipeline_cache(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        output_format: wgpu::TextureFormat,
        cache: Option<wgpu::PipelineCache>,
    ) -> Result<Self, ShaderError> {
        let mut resources = Resources::new(width, height);
        resources.create_texture(
            device,
            SCENE_HDR,
            TextureDesc::viewport(
                HDR_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                1.0,
            ),
        );
        resources.create_texture(
            device,
            SCENE_DEPTH,
            TextureDesc::viewport(
                DEPTH_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                1.0,
            ),
        );
        let (camera, shadow_targets) = CameraBinding::new(device);
        let mut library = ShaderLibrary::new();
        let mut pipelines = PipelineRegistry::default();
        // Every pipeline from here on compiles on build threads (native), so a load's pipelines
        // compile together; lookups wait for theirs (see `crate::pipelines`).
        pipelines.set_build_threads(crate::pipelines::default_build_threads());
        pipelines.set_cache(cache);
        let eye = EyeAdaptation::new(device, &mut library, &mut pipelines, &mut resources)?;
        let sun_glare =
            crate::sun_glare::SunGlare::new(device, &mut library, &mut pipelines, &mut resources)?;
        let bloom = Bloom::new(device, &mut library, &mut pipelines)?;
        let taa = Taa::new(device, &mut library, &mut pipelines)?;
        let chain = PostChain::new(device, queue, &mut library, &mut pipelines)?;
        let dof = crate::photo::DepthOfFieldPass::new(device, &mut library, &mut pipelines)?;
        let optics = crate::photo_optics::OpticsPass::new(device, &mut library, &mut pipelines)?;
        let shutter = crate::shutter::ShutterPass::new(device, &mut library, &mut pipelines)?;
        let film = crate::film::FilmPass::new(device, &mut library)?;
        let black = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("no bloom"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let black_view = black.create_view(&Default::default());
        let mut renderer = Self {
            device: device.clone(),
            queue: queue.clone(),
            library,
            pipelines,
            resources,
            camera,
            shadow_fit: ShadowFit::default(),
            shadow_targets,
            sun: None,
            timer: GpuTimer::new(device, queue),
            uploads: Uploads::new(device),
            upload_stats: UploadStats::default(),
            scene_groups: (SceneCopies::NONE, Vec::new()),
            spare_report: None,
            extra_copies: SceneCopies::NONE,
            submitted: 0,
            display: DisplayPass::new(device),
            eye,
            bloom,
            bloom_on: false,
            sun_glare,
            taa,
            antialiasing: Antialiasing::None,
            render_scale: 1.0,
            chain,
            overlays: Vec::new(),
            output_format,
            black: (black, black_view),
            underwater: (None, 0),
            dither: Dither::new(device, queue),
            headroom: 1.0,
            #[cfg(feature = "hot-reload")]
            watcher: crate::hot_reload::ShaderWatcher::default(),
            placeholders: std::sync::OnceLock::new(),
            debug: DebugViews::default(),
            dof,
            optics,
            photo: None,
            shutter,
            photo_bodies: Vec::new(),
            look: crate::photo::Look::Natural,
            drawn_look: crate::photo::Look::Natural,
            clip_planes: [0.5, 60_000.0],
            film,
        };
        renderer.display.ensure_format(
            &renderer.device,
            &mut renderer.library,
            &mut renderer.pipelines,
            output_format,
        )?;
        renderer.display.ensure_cover(
            &renderer.device,
            &mut renderer.library,
            &mut renderer.pipelines,
        )?;
        Ok(renderer)
    }

    /// Borrows what a layer is built with.
    pub fn context(&mut self) -> RenderContext<'_> {
        RenderContext {
            device: &self.device,
            queue: &self.queue,
            library: &mut self.library,
            pipelines: &mut self.pipelines,
            resources: &mut self.resources,
            camera_layout: &self.camera.layout,
            caster_layout: &self.camera.caster_layout,
            output_format: self.output_format,
            aerial: &self.camera.aerial,
            overlays: &mut self.overlays,
            placeholders: &self.placeholders,
        }
    }

    /// Records and submits one frame: prepare, the sun's shadow maps, the scene
    /// pass, the display pass.
    /// The scene targets are resized to the output first.
    pub fn render(
        &mut self,
        input: FrameInput<'_>,
        layers: &mut [&mut dyn SceneLayer],
        output: RenderOutput<'_>,
    ) -> Result<FrameReport, ShaderError> {
        // The scene is drawn at the render scale's size. Without edge smoothing at full
        // scale the display pass writes the output; otherwise it writes `display.frame`
        // for the chain's last stage (FXAA, SMAA's blend, temporal AA's output), which
        // writes the output, or `post.frame` for the bilinear upscale below full scale
        // (temporal AA resolves at the output's size instead).
        let taa_on = self.antialiasing == Antialiasing::Taa;
        let scene = scene_size((output.width, output.height), self.render_scale);
        let output_size = (output.width.max(1), output.height.max(1));
        // Below full scale temporal AA upscales the frame itself (its resolve and output pass are at
        // the output's size); FXAA and SMAA are stretched onto the output bilinearly.
        let temporal_upscale = taa_on && scene != output_size;
        let upscale = scene != output_size && !temporal_upscale;
        let stage = match self.antialiasing {
            Antialiasing::None => None,
            Antialiasing::Fxaa => Some(OutputStage::Fxaa),
            Antialiasing::Smaa => Some(OutputStage::SmaaBlend),
            Antialiasing::Taa => Some(OutputStage::Taa),
        };
        let display_direct = stage.is_none() && !upscale;
        // A look's film goes on last, at the output's size (`crate::film`): whichever pass would write the output
        // writes `film.frame` instead, and the film pass writes the output.
        let look = self.photo.as_ref().map_or(self.look, |photo| photo.look);
        // The grade comes before temporal AA, whose history (kept while time is held) would blend the last look's
        // colours into the new one's.
        if look != self.drawn_look {
            self.drawn_look = look;
            self.taa.reset();
        }
        let film = crate::film::FilmFinish::new(look, self.photo.as_ref(), input.time.seconds);
        let film_on = film.is_on();
        let last_format = if film_on {
            crate::taa::FRAME_FORMAT
        } else {
            output.format
        };
        let display_format = if display_direct {
            last_format
        } else {
            crate::taa::FRAME_FORMAT
        };
        let display = self.display.ensure_format(
            &self.device,
            &mut self.library,
            &mut self.pipelines,
            display_format,
        )?;
        let stage_format = if upscale {
            crate::taa::FRAME_FORMAT
        } else {
            last_format
        };
        let stage_pipeline = match stage {
            Some(stage) => Some(self.chain.ensure_stage(
                &self.device,
                &mut self.library,
                &mut self.pipelines,
                stage,
                stage_format,
            )?),
            None => None,
        };
        let upscale_pipeline = if upscale {
            Some(self.chain.ensure_stage(
                &self.device,
                &mut self.library,
                &mut self.pipelines,
                OutputStage::Upscale,
                last_format,
            )?)
        } else {
            None
        };
        let film_pipeline = if film_on {
            Some(self.film.ensure_format(
                &self.device,
                &mut self.library,
                &mut self.pipelines,
                output.format,
            )?)
        } else {
            None
        };
        self.output_format = output.format;
        let overlay = layers.iter().any(|layer| layer.has_overlay());
        let debug_pass = self.debug.pipeline();
        if overlay || debug_pass.is_some() {
            for entry in &mut self.overlays {
                entry.ensure(
                    &self.device,
                    &mut self.library,
                    &mut self.pipelines,
                    &self.camera.layout,
                    output.format,
                )?;
            }
        }
        // Pipelines still compiling on build threads are waited for here, before any is drawn.
        self.pipelines.finish()?;
        self.resources.resize(&self.device, scene.0, scene.1);
        let chain_dropped = PostChain::ensure_targets(
            &self.device,
            &mut self.resources,
            !display_direct,
            upscale && stage.is_some(),
            self.antialiasing == Antialiasing::Smaa,
        );
        if chain_dropped {
            // Its cached groups bind the dropped targets; they would keep them alive.
            self.chain.forget_groups();
        }
        self.film
            .ensure_target(&self.device, &mut self.resources, output_size, film_on);
        let viewport = self.resources.viewport();
        let unjittered = input
            .camera
            .projection(viewport.0 as f32 / viewport.1.max(1) as f32)
            * input.camera.view();
        let (uniforms, taa_frame) = if taa_on {
            Taa::ensure_targets(&self.device, &mut self.resources, output_size);
            // No time passed (the presentation held: a pause): a still camera repeats the last
            // frame, so the picture holds.
            let held = input.time.dt <= 0.0;
            // A scene drawn smaller than the output takes more jitter phases to cover it.
            let phases = crate::taa::jitter_phases(viewport.0 as f32 / output_size.0 as f32);
            let frame = self.taa.begin(
                input.time.frame,
                input.camera,
                unjittered,
                viewport,
                held,
                phases,
            );
            let uniforms =
                input
                    .camera
                    .uniforms_jittered(viewport.0, viewport.1, input.time, frame.jitter);
            (uniforms, Some(frame))
        } else {
            // The post chain's cached groups bind `taa.resolved`; they would keep it alive.
            if self.taa.release_targets(&mut self.resources) {
                self.chain.forget_groups();
            }
            (
                input.camera.uniforms(viewport.0, viewport.1, input.time),
                None,
            )
        };
        let motion = CameraMotion {
            unjittered_view_projection: unjittered.to_cols_array(),
            previous_view_projection: taa_frame
                .map_or(unjittered, |frame| frame.previous_view_projection)
                .to_cols_array(),
            jitter: taa_frame.map_or([0.0; 2], |frame| frame.jitter.to_array()),
            padding: [0.0; 2],
        };
        // The frame's own uniforms through its staging belt too: in place before its passes, as the layers' are.
        self.uploads
            .write_buffer(&self.camera.motion, 0, bytes_of(&motion));
        self.uploads
            .write_buffer(&self.camera.buffer, 0, bytes_of(&uniforms));

        // Fit the sun's maps to this frame's camera, and remake them if their size changed.
        let aspect = viewport.0 as f32 / viewport.1.max(1) as f32;
        self.shadow_fit
            .update(input.camera, aspect, self.sun.as_ref());
        if self
            .shadow_targets
            .ensure(&self.device, &self.shadow_fit.settings)
        {
            // New textures hold nothing: draw every map sampled this frame.
            for map in &mut self.shadow_fit.maps {
                map.drawn |= map.on;
            }
        }
        self.camera.follow(&self.device, &self.shadow_targets);
        let toward_light = self
            .sun
            .as_ref()
            .map_or(glam::Vec3::Y, |sun| sun.toward_light);
        self.uploads.write_buffer(
            &self.shadow_targets.uniforms,
            0,
            bytes_of(&self.shadow_fit.uniforms(toward_light)),
        );
        // No soft occluders until a layer draws them this frame (a layer's own write, through the same belt, follows).
        self.uploads.write_buffer(
            &self.shadow_targets.occluders.uniforms,
            0,
            bytes_of(&crate::shadows::SunOccluderUniform::default()),
        );

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        self.timer
            .begin_frame(&self.device, &mut encoder, self.submitted);
        self.eye.poll_reading(&self.device);
        self.eye.write_sky_mean(&mut self.uploads, &self.resources);
        self.dither.write(&mut self.uploads, input.time.seconds);
        let copies = layers
            .iter()
            .fold(self.extra_copies, |set, layer| set | layer.scene_copies());
        ensure_scene_copies(&self.device, &mut self.resources, copies);

        for layer in layers.iter_mut() {
            encoder.push_debug_group(layer.label());
            layer.prepare(&mut PrepareContext {
                device: &self.device,
                queue: &self.queue,
                encoder: &mut encoder,
                pipelines: &self.pipelines,
                resources: &mut self.resources,
                camera: &uniforms,
                camera_group: &self.camera.group,
                time: input.time,
                viewport,
                timer: &self.timer,
                uploads: &mut self.uploads,
                shadow_maps: &self.shadow_fit.maps,
                aerial: &self.camera.aerial,
                sun_occluders: &self.shadow_targets.occluders,
            });
            encoder.pop_debug_group();
        }

        // The frame's report, written over one handed back (its lists keep their room).
        let mut report = self.spare_report.take().unwrap_or_default();
        // The sun's shadow maps due this frame.
        let shadow_maps = &mut report.shadow_maps;
        shadow_maps.clear();
        for map in self.shadow_fit.maps.iter().filter(|map| map.drawn) {
            let (group, light) =
                self.shadow_targets
                    .caster_group(&mut self.uploads, map, input.time);
            let label = shadow_label(map.kind);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                timestamp_writes: self.timer.render_pass(label),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: self.shadow_targets.target(map.kind),
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            let ctx = DrawContext {
                pipelines: &self.pipelines,
                resources: &self.resources,
                camera: &light,
                time: input.time,
                viewport: (map.size, map.size),
            };
            for layer in layers.iter() {
                pass.push_debug_group(layer.label());
                pass.set_bind_group(0, group, &[]);
                layer.draw_shadow(map, &mut pass, &ctx);
                pass.pop_debug_group();
            }
            shadow_maps.push(map.kind.name());
        }

        // The scene pass, ended at each copy point a layer asked for.
        if self.scene_groups.1.is_empty() || self.scene_groups.0 != copies {
            self.scene_groups = (
                copies,
                scene_pass_groups(copies)
                    .into_iter()
                    .map(|(phases, point)| {
                        let names: Vec<&str> = phases.iter().map(|phase| phase.name()).collect();
                        let label = format!("scene {}", names.join(", "));
                        (phases, point, label)
                    })
                    .collect(),
            );
        }
        let scene_groups = std::mem::take(&mut self.scene_groups.1);
        let night_readability = self.eye.sky_mean != SkyMean::None;
        let scene_passes = &mut report.scene_passes;
        scene_passes.truncate(scene_groups.len());
        for (index, (phases, point, label)) in scene_groups.iter().enumerate() {
            let point = *point;
            {
                let mut pass = begin_scene_pass(
                    &mut encoder,
                    &self.resources,
                    label,
                    index > 0,
                    self.timer.render_pass(label),
                );
                let ctx = DrawContext {
                    pipelines: &self.pipelines,
                    resources: &self.resources,
                    camera: &uniforms,
                    time: input.time,
                    viewport,
                };
                for &phase in phases {
                    // The night's grade reads the transparents' cover from the scene's alpha (`transparent_cover.wgsl`).
                    if phase == Phase::Transparent && night_readability {
                        self.display.draw_cover(&mut pass, &self.pipelines);
                    }
                    for layer in layers.iter() {
                        pass.push_debug_group(layer.label());
                        pass.set_bind_group(0, &self.camera.group, &[]);
                        layer.draw(phase, &mut pass, &ctx);
                        pass.pop_debug_group();
                    }
                }
            }
            match scene_passes.get_mut(index) {
                Some(held) => held.clone_from(label),
                None => scene_passes.push(label.clone()),
            }
            if let Some(point) = point {
                copy_scene(&mut encoder, &self.resources, copies & point.copies());
                for layer in layers.iter_mut() {
                    encoder.push_debug_group(layer.label());
                    layer.after_copies(
                        point,
                        &mut PrepareContext {
                            device: &self.device,
                            queue: &self.queue,
                            encoder: &mut encoder,
                            pipelines: &self.pipelines,
                            resources: &mut self.resources,
                            camera: &uniforms,
                            camera_group: &self.camera.group,
                            time: input.time,
                            viewport,
                            timer: &self.timer,
                            uploads: &mut self.uploads,
                            shadow_maps: &self.shadow_fit.maps,
                            aerial: &self.camera.aerial,
                            sun_occluders: &self.shadow_targets.occluders,
                        },
                    );
                    encoder.pop_debug_group();
                }
            }
        }
        self.scene_groups.1 = scene_groups;

        // Temporal AA: moving surfaces' motion, where each is the nearest thing drawn.
        if taa_frame.is_some() {
            let motion = self.resources.texture(SCENE_MOTION).expect("made above");
            let depth = self
                .resources
                .texture(SCENE_DEPTH)
                .expect("registered in new");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("motion"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &motion.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth.view,
                    depth_ops: None,
                    stencil_ops: None,
                }),
                timestamp_writes: self.timer.render_pass("motion"),
                ..Default::default()
            });
            let ctx = DrawContext {
                pipelines: &self.pipelines,
                resources: &self.resources,
                camera: &uniforms,
                time: input.time,
                viewport,
            };
            for layer in layers.iter() {
                pass.push_debug_group(layer.label());
                pass.set_bind_group(0, &self.camera.group, &[]);
                layer.draw_motion(&mut pass, &ctx);
                pass.pop_debug_group();
            }
            // Then what lies in front of every surface (the rain), over all their motion.
            for layer in layers.iter() {
                pass.set_bind_group(0, &self.camera.group, &[]);
                layer.draw_motion_over(&mut pass, &ctx);
            }
        }

        // Photo mode's shutter: the scene blurred in place by the frozen moment's velocities, before the eye, the bloom,
        // the depth of field and the finish read it.
        if let Some(shutter) = self.photo.as_ref().and_then(|photo| photo.shutter) {
            let hdr = self
                .resources
                .texture(SCENE_HDR)
                .expect("registered in new");
            let depth = self
                .resources
                .texture(SCENE_DEPTH)
                .expect("registered in new");
            self.shutter.encode(
                &self.device,
                &self.queue,
                &mut encoder,
                &self.pipelines,
                &self.timer,
                (&hdr.texture, &hdr.view, hdr.version),
                (&depth.view, depth.version),
                viewport,
                input.camera,
                taa_frame.map_or(glam::Vec2::ZERO, |frame| frame.jitter),
                &shutter,
                &self.photo_bodies,
            );
        }

        // Post: the eye's exposure for this frame, then the bloom on it.
        self.eye.encode(
            &self.device,
            &mut self.uploads,
            &mut encoder,
            &self.pipelines,
            &self.resources,
            &self.timer,
            input.time.frame,
            input.time.dt,
        );
        self.eye
            .copy_reading(&self.device, &mut encoder, &self.resources);
        if self.bloom_on {
            // What the frame shows of the sun, for its glare in the eye.
            let sun = self.sun_glare.encode(
                &self.device,
                &mut self.uploads,
                &mut encoder,
                &self.pipelines,
                &self.resources,
                &self.timer,
                input.camera,
                input.time.dt,
            );
            self.bloom.encode(
                &self.device,
                &mut self.uploads,
                &mut encoder,
                &self.pipelines,
                &self.resources,
                &self.timer,
                // Photo mode's compensation too, so the glow follows the exposure.
                self.display.settings.exposure * self.photo.map_or(1.0, |photo| photo.ev.exp2()),
                sun,
            );
        } else {
            self.bloom.release();
        }

        let hdr = self
            .resources
            .texture(SCENE_HDR)
            .expect("registered in new");
        // Photo mode's depth of field: the display reads its result in place of the scene's radiance.
        let source = match self.photo.as_ref().and_then(|photo| photo.dof) {
            Some(dof) => {
                let depth = self
                    .resources
                    .texture(SCENE_DEPTH)
                    .expect("registered in new");
                self.dof.encode(
                    &self.device,
                    &self.queue,
                    &mut encoder,
                    &self.pipelines,
                    &self.timer,
                    ((&hdr.view, hdr.version), (&depth.view, depth.version)),
                    viewport,
                    (input.camera.near, input.camera.far),
                    &dof,
                    crate::photo_optics::aperture_shape(
                        self.photo.as_ref().and_then(|photo| photo.optics.as_ref()),
                        self.photo.as_ref().map_or(0.0, |photo| photo.roll),
                    ),
                )
            }
            None => (&hdr.view, hdr.version),
        };
        // Photo mode's lens: its flare and glare over that, from the scene's radiance.
        let source = match self
            .photo
            .as_ref()
            .and_then(|photo| photo.optics.map(|o| (o, photo)))
        {
            Some((optics, photo)) => {
                let depth = self
                    .resources
                    .texture(SCENE_DEPTH)
                    .expect("registered in new");
                let eye = &self
                    .resources
                    .buffer(EYE_STATE_BUFFER)
                    .expect("registered by the eye adaptation")
                    .buffer;
                self.optics.encode(
                    &self.device,
                    &self.queue,
                    &mut encoder,
                    &self.pipelines,
                    &self.timer,
                    &crate::photo_optics::OpticsInputs {
                        scene: &hdr.view,
                        depth: &depth.view,
                        source: source.0,
                        eye,
                        viewport,
                        exposure: self.display.settings.exposure * photo.ev.exp2(),
                        frame_scale: photo.roll_scale,
                        roll: photo.roll,
                        camera: input.camera,
                        dof: photo.dof,
                    },
                    &optics,
                )
            }
            None => source,
        };
        let bloom = match self.bloom.output().filter(|_| self.bloom_on) {
            Some(view) => (view, self.bloom.version),
            None => (&self.black.1, 0),
        };
        let inputs = DisplayInputs {
            source,
            depth: {
                let depth = self
                    .resources
                    .texture(SCENE_DEPTH)
                    .expect("registered in new");
                (&depth.view, depth.version)
            },
            clip_planes: [input.camera.near, input.camera.far],
            bloom,
            bloom_on: self.bloom_on,
            sampler: self.bloom.sampler(),
            eye: &self
                .resources
                .buffer(EYE_STATE_BUFFER)
                .expect("registered by the eye adaptation")
                .buffer,
            sky_mean: &self
                .resources
                .buffer(SKY_MEAN_BUFFER)
                .expect("registered by the eye adaptation")
                .buffer,
            night_readability: self.eye.sky_mean != SkyMean::None,
            underwater: (
                self.underwater.0.as_ref().unwrap_or(&self.black.1),
                self.underwater.1,
            ),
            headroom: if extended_range(output.format) {
                self.headroom
            } else {
                1.0
            },
            dither: &self.dither,
            photo: self.photo.as_ref(),
            look,
            preview: 0.0,
        };
        self.clip_planes = inputs.clip_planes;
        let film_frame = film_on.then(|| {
            &self
                .resources
                .texture(crate::film::FILM_FRAME)
                .expect("made above")
                .view
        });
        let display_target = if display_direct {
            film_frame.unwrap_or(output.view)
        } else {
            &self
                .resources
                .texture(DISPLAY_FRAME)
                .expect("made above")
                .view
        };
        self.display.encode(
            &self.device,
            &mut UploadSink::Belt(&mut self.uploads),
            &mut encoder,
            self.pipelines.render(display),
            &inputs,
            display_target,
            self.timer.render_pass("display"),
        );
        if let Some(frame) = taa_frame {
            self.taa.resolve(
                &self.device,
                &mut self.uploads,
                &mut encoder,
                &self.pipelines,
                &self.resources,
                &self.timer,
                &frame,
                &uniforms,
                &motion,
            );
        }
        if self.antialiasing == Antialiasing::Smaa {
            self.chain.encode_smaa(
                &self.device,
                &mut encoder,
                &self.pipelines,
                &self.resources,
                &self.timer,
                &self.dither,
                DISPLAY_FRAME,
            );
        }
        // The last stage onto the output, or into `post.frame` for the upscale.
        let mut last = DISPLAY_FRAME;
        if let (Some(stage), Some(pipeline)) = (stage, stage_pipeline) {
            let source = if stage == OutputStage::Taa {
                TAA_RESOLVED
            } else {
                DISPLAY_FRAME
            };
            let target = if upscale {
                StageTarget::Texture(POST_FRAME)
            } else if film_on {
                StageTarget::Texture(crate::film::FILM_FRAME)
            } else {
                StageTarget::Output(output.view)
            };
            self.chain.encode_stage(
                &self.device,
                &mut encoder,
                &self.pipelines,
                &self.resources,
                &self.timer,
                &self.dither,
                stage,
                pipeline,
                source,
                target,
            );
            last = POST_FRAME;
        }
        if let Some(pipeline) = upscale_pipeline {
            self.chain.encode_stage(
                &self.device,
                &mut encoder,
                &self.pipelines,
                &self.resources,
                &self.timer,
                &self.dither,
                OutputStage::Upscale,
                pipeline,
                last,
                if film_on {
                    StageTarget::Texture(crate::film::FILM_FRAME)
                } else {
                    StageTarget::Output(output.view)
                },
            );
        }
        if let Some(pipeline) = film_pipeline {
            self.film.encode(
                &self.device,
                &mut self.uploads,
                &mut encoder,
                self.pipelines.render(pipeline),
                &self.resources,
                &self.timer,
                &self.dither,
                &film,
                output.view,
            );
        }
        // The developer panel's full-screen debug view, over the finished frame and under the overlays.
        if let Some(id) = debug_pass {
            let depth = self
                .resources
                .texture(SCENE_DEPTH)
                .expect("registered in new");
            let pipeline = self.overlays[id.0]
                .built
                .iter()
                .find(|(format, _)| *format == output.format)
                .map(|(_, built)| *built);
            if let (Some(group), Some(pipeline)) = (self.debug.group(&self.device, depth), pipeline)
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("debug view"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: output.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    timestamp_writes: self.timer.render_pass("debug view"),
                    ..Default::default()
                });
                pass.set_pipeline(self.pipelines.render(pipeline));
                pass.set_bind_group(0, &self.camera.group, &[]);
                pass.set_bind_group(1, group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        if overlay {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                timestamp_writes: self.timer.render_pass("overlay"),
                ..Default::default()
            });
            let ctx = OverlayContext {
                pipelines: &self.pipelines,
                resources: &self.resources,
                camera: &uniforms,
                time: input.time,
                viewport: (output.width.max(1), output.height.max(1)),
                scene_viewport: viewport,
                format: output.format,
                overlays: &self.overlays,
            };
            for layer in layers.iter().filter(|layer| layer.has_overlay()) {
                pass.push_debug_group(layer.label());
                pass.set_bind_group(0, &self.camera.group, &[]);
                layer.draw_overlay(&mut pass, &ctx);
                pass.pop_debug_group();
            }
        }
        // The frame's uploads first, so they are in place before its passes (as `write_buffer`'s are).
        self.upload_stats = self.uploads.take_stats();
        let uploads = self.uploads.finish();
        self.queue
            .submit(uploads.into_iter().chain([encoder.finish()]));
        self.uploads.recall();
        self.eye.map_reading();
        self.timer.end_frame(&self.queue);
        self.submitted += 1;
        self.resources.end_frame();
        report.frame = input.time.frame;
        report.time = input.time.seconds;
        report.viewport = viewport;
        report.copies = copies;
        report.layers.truncate(layers.len());
        for (index, layer) in layers.iter().enumerate() {
            match report.layers.get_mut(index) {
                Some(held) => {
                    held.clear();
                    held.push_str(layer.label());
                }
                None => report.layers.push(layer.label().to_owned()),
            }
        }
        Ok(report)
    }

    /// Hands back a report [`Renderer::render`] returned, for the next frame's to be written over (its lists keep
    /// their room): a host that keeps only the latest report passes the one it replaces.
    pub fn recycle_report(&mut self, report: FrameReport) {
        self.spare_report = Some(report);
    }

    /// Builds, ahead of the first frame, every pipeline the output passes can need for an output
    /// of `format`: the display pass onto it and onto the post frame, each edge-smoothing stage
    /// (FXAA, SMAA's blend, temporal AA's present) onto either, the render scale's upscale onto
    /// either, the look's film pass and the overlays. Otherwise each is built by the first frame
    /// that uses it, and a change of antialiasing, render scale or look mid-battle would stall a
    /// frame while it compiles. With build
    /// threads this returns at once; the frame that first needs a pipeline waits for it.
    pub fn prepare_output(&mut self, format: wgpu::TextureFormat) -> Result<(), ShaderError> {
        for display_format in [format, crate::taa::FRAME_FORMAT] {
            self.display.ensure_format(
                &self.device,
                &mut self.library,
                &mut self.pipelines,
                display_format,
            )?;
        }
        for stage_format in [format, crate::taa::FRAME_FORMAT] {
            for stage in [OutputStage::Fxaa, OutputStage::SmaaBlend, OutputStage::Taa] {
                self.chain.ensure_stage(
                    &self.device,
                    &mut self.library,
                    &mut self.pipelines,
                    stage,
                    stage_format,
                )?;
            }
        }
        // The upscale writes the output, or the film's frame under a look's film, which the film pass puts on the
        // output (`crate::film`).
        for upscale_format in [format, crate::taa::FRAME_FORMAT] {
            self.chain.ensure_stage(
                &self.device,
                &mut self.library,
                &mut self.pipelines,
                OutputStage::Upscale,
                upscale_format,
            )?;
        }
        // The film onto the output, and onto the Look tab's previews (`crate::look_preview`).
        for film_format in [format, crate::look_preview::PREVIEW_FORMAT] {
            self.film.ensure_format(
                &self.device,
                &mut self.library,
                &mut self.pipelines,
                film_format,
            )?;
        }
        for entry in &mut self.overlays {
            entry.ensure(
                &self.device,
                &mut self.library,
                &mut self.pipelines,
                &self.camera.layout,
                format,
            )?;
        }
        Ok(())
    }

    /// Waits for every pipeline still compiling on a build thread, returning the first one wgpu
    /// rejected. `render` does this itself; a host calls it to end a load with every pipeline
    /// compiled, or to see a first build's error where it registered it.
    pub fn finish_pipelines(&mut self) -> Result<(), ShaderError> {
        self.pipelines.finish()
    }

    /// Pipelines still compiling on build threads.
    pub fn pending_pipelines(&self) -> usize {
        self.pipelines.pending()
    }

    /// Build threads for pipelines registered from now on (native; 0 builds each on the calling
    /// thread). `Renderer::new` starts [`crate::pipelines::default_build_threads`].
    pub fn set_pipeline_build_threads(&mut self, threads: usize) {
        self.pipelines.set_build_threads(threads);
    }

    /// The pipeline cache handed to every pipeline registered from now on (Vulkan's
    /// `Features::PIPELINE_CACHE`; see `crate::pipelines::PipelineBuild::cache`).
    pub fn set_pipeline_cache(&mut self, cache: Option<wgpu::PipelineCache>) {
        self.pipelines.set_cache(cache);
    }

    /// The pipeline cache, whose `get_data` a host persists.
    pub fn pipeline_cache(&self) -> Option<&wgpu::PipelineCache> {
        self.pipelines.cache()
    }

    /// Applies new text for library files and rebuilds the pipelines that
    /// import them, keeping the last good pipeline where a rebuild fails. The
    /// `hot-reload` feature feeds this from disk; any host can call it.
    pub fn apply_shader_changes(&mut self, changes: Vec<(String, String)>) -> ReloadReport {
        let mut report = ReloadReport::default();
        let mut changed = std::collections::BTreeSet::new();
        for (path, text) in changes {
            match self.library.update(&path, text) {
                // The file itself, plus module edits an earlier broken module held back.
                Ok(live) => {
                    for path in live {
                        if changed.insert(path.clone()) {
                            report.changed.push(path);
                        }
                    }
                }
                Err(error) => report.library_errors.push((path, error.message)),
            }
        }
        if !changed.is_empty() {
            report.pipelines =
                self.pipelines
                    .rebuild_affected(&self.device, &mut self.library, &changed);
        }
        report
    }

    /// Checks registered WGSL files on disk (at most every quarter second) and
    /// applies any that changed. `None` when nothing changed.
    #[cfg(feature = "hot-reload")]
    pub fn poll_shader_files(&mut self) -> Option<ReloadReport> {
        let changes = self.watcher.poll(&self.library);
        if changes.is_empty() {
            return None;
        }
        Some(self.apply_shader_changes(changes))
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn resources(&self) -> &Resources {
        &self.resources
    }

    /// The aerial-perspective volume group 0 binds ([`crate::aerial`]).
    pub fn aerial(&self) -> &AerialVolume {
        &self.camera.aerial
    }

    pub fn library(&self) -> &ShaderLibrary {
        &self.library
    }

    /// The pipeline registry, for a host's own passes outside the layers (labels drawn onto the output) to look up
    /// what it registered through [`Renderer::context`].
    pub fn pipelines(&self) -> &PipelineRegistry {
        &self.pipelines
    }

    pub fn pipeline_status(&self) -> Vec<PipelineStatus> {
        self.pipelines.status()
    }

    /// Photo mode's post (`crate::photo`): its depth of field, exposure compensation, look (in place of the game's,
    /// [`Renderer::set_look`]), grain, vignette and roll; `None` draws the frame with the game's look, with nothing of
    /// photo mode's allocated.
    pub fn set_photo(&mut self, photo: Option<crate::photo::PhotoPost>) {
        if photo.and_then(|p| p.dof).is_none() {
            self.dof.release();
        }
        if photo.and_then(|p| p.optics).is_none() {
            self.optics.release();
        }
        if photo.and_then(|p| p.shutter).is_none() {
            self.shutter.release();
            self.photo_bodies = Vec::new();
        }
        self.photo = photo;
    }

    pub fn photo(&self) -> Option<crate::photo::PhotoPost> {
        self.photo
    }

    /// The game's look (`crate::photo::Look`, the Graphics menu's Look): its grade, its halation and diffusion, and
    /// its film after edge smoothing (`crate::film`). Natural draws the frame as it was, with no film pass.
    pub fn set_look(&mut self, look: crate::photo::Look) {
        self.look = look;
    }

    pub fn look(&self) -> crate::photo::Look {
        self.look
    }

    /// The Look tab's previews (`crate::look_preview`): each of `looks` on the frame last drawn (its radiance, bloom
    /// and eye), about `width` pixels across at its aspect, through the display pass's grade and the film pass's
    /// film. Each look is a submit of its own, as both passes write their uniforms per draw; the tiles land one after
    /// another in a buffer the host maps. `None` before the first frame, or with a pipeline that failed to build.
    pub fn look_previews(
        &mut self,
        looks: &[crate::photo::Look],
        width: u32,
    ) -> Option<crate::look_preview::LookPreviews> {
        use crate::look_preview::{PREVIEW_FORMAT, preview_film, tile_size};
        if looks.is_empty() || self.submitted == 0 {
            return None;
        }
        let display_id = self
            .display
            .ensure_format(
                &self.device,
                &mut self.library,
                &mut self.pipelines,
                crate::taa::FRAME_FORMAT,
            )
            .ok()?;
        let film_id = self
            .film
            .ensure_format(
                &self.device,
                &mut self.library,
                &mut self.pipelines,
                PREVIEW_FORMAT,
            )
            .ok()?;
        self.pipelines.finish().ok()?;
        let hdr = self.resources.texture(SCENE_HDR)?;
        let frame = (hdr.texture.width(), hdr.texture.height());
        let (width, height) = tile_size(width, frame);
        let target = |label, format, usage| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | usage,
                view_formats: &[],
            })
        };
        let graded = target(
            "look preview graded",
            crate::taa::FRAME_FORMAT,
            wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let graded_view = graded.create_view(&wgpu::TextureViewDescriptor::default());
        let tile = target(
            "look preview tile",
            PREVIEW_FORMAT,
            wgpu::TextureUsages::COPY_SRC,
        );
        let tile_view = tile.create_view(&wgpu::TextureViewDescriptor::default());
        let row_bytes = width * 4;
        let tile_bytes = u64::from(row_bytes) * u64::from(height);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("look previews"),
            size: tile_bytes * looks.len() as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let depth = self
            .resources
            .texture(SCENE_DEPTH)
            .expect("registered in new");
        let bloom = match self.bloom.output().filter(|_| self.bloom_on) {
            Some(view) => (view, self.bloom.version),
            None => (&self.black.1, 0),
        };
        let eye = &self
            .resources
            .buffer(EYE_STATE_BUFFER)
            .expect("registered by the eye adaptation")
            .buffer;
        let sky_mean = &self
            .resources
            .buffer(SKY_MEAN_BUFFER)
            .expect("registered by the eye adaptation")
            .buffer;
        for (index, look) in looks.iter().enumerate() {
            let inputs = DisplayInputs {
                source: (&hdr.view, hdr.version),
                depth: (&depth.view, depth.version),
                clip_planes: self.clip_planes,
                bloom,
                bloom_on: self.bloom_on,
                sampler: self.bloom.sampler(),
                eye,
                sky_mean,
                night_readability: self.eye.sky_mean != SkyMean::None,
                underwater: (
                    self.underwater.0.as_ref().unwrap_or(&self.black.1),
                    self.underwater.1,
                ),
                headroom: 1.0,
                dither: &self.dither,
                photo: None,
                look: *look,
                preview: frame.0 as f32 / width as f32,
            };
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("look preview"),
                });
            self.display.encode(
                &self.device,
                &mut UploadSink::Queue(&self.queue),
                &mut encoder,
                self.pipelines.render(display_id),
                &inputs,
                &graded_view,
                None,
            );
            self.film.encode_preview(
                &self.device,
                &self.queue,
                &mut encoder,
                self.pipelines.render(film_id),
                &self.dither,
                &preview_film(*look, height),
                &graded_view,
                &tile_view,
            );
            encoder.copy_texture_to_buffer(
                tile.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: tile_bytes * index as u64,
                        bytes_per_row: Some(row_bytes),
                        rows_per_image: Some(height),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            self.queue.submit([encoder.finish()]);
        }
        Some(crate::look_preview::LookPreviews {
            buffer,
            looks: looks.to_vec(),
            width,
            height,
            row_bytes,
        })
    }

    /// Photo mode's moving bodies at the frozen moment, which its shutter blurs by their velocities (world space; the
    /// renderer keeps those in view). Read only while [`crate::photo::PhotoPost::shutter`] is set.
    pub fn set_photo_bodies(&mut self, bodies: &[crate::shutter::MovingBody]) {
        self.photo_bodies.clear();
        self.photo_bodies.extend_from_slice(bodies);
    }

    /// Photo mode's focus probe: the reversed depth the last frame drew at scene texel `at`, copied into a new 16-byte
    /// read-back buffer (the depth first) in a submission of its own, for the caller to map without waiting. `None`
    /// before a frame has been drawn.
    pub fn probe_depth(&mut self, at: (u32, u32)) -> Option<wgpu::Buffer> {
        let depth = self.resources.texture(SCENE_DEPTH)?;
        Some(
            self.dof
                .probe_depth(&self.device, &self.queue, &self.pipelines, &depth.view, at),
        )
    }

    pub fn display_settings(&self) -> DisplaySettings {
        self.display.settings
    }

    pub fn set_display_settings(&mut self, settings: DisplaySettings) {
        self.display.settings = settings;
    }

    /// The display's headroom: how much brighter than SDR white it can show now (Apple's EDR
    /// headroom, or a Windows display's peak over its SDR white), 1 for SDR (the default). Only a
    /// float output ([`extended_range`]) is drawn for it, with AgX's top raised to it (see
    /// [`crate::display`]); 8- and 10-bit outputs are always drawn for SDR. Anything below 1 or
    /// not finite is 1.
    pub fn set_headroom(&mut self, headroom: f32) {
        self.headroom = if headroom.is_finite() {
            headroom.max(1.0)
        } else {
            1.0
        };
    }

    pub fn headroom(&self) -> f32 {
        self.headroom
    }

    /// GPU timings per pass (see [`crate::timing`]): `latest()` and `history()`
    /// hold frames whose timestamps have arrived, two or three frames late.
    pub fn gpu_timer(&self) -> &GpuTimer {
        &self.timer
    }

    pub fn gpu_timer_mut(&mut self) -> &mut GpuTimer {
        &mut self.timer
    }

    /// The renderer's uploads, for a host writing a layer's buffers outside its `prepare`: they go with the next frame's
    /// submission, ahead of its passes.
    pub fn uploads(&mut self) -> &mut Uploads {
        &mut self.uploads
    }

    /// What the last frame uploaded through the renderer's staging belt.
    pub fn upload_stats(&self) -> UploadStats {
        self.upload_stats
    }

    /// Frames submitted so far: the `frame` numbers of [`crate::FrameTimings`].
    pub fn frames_submitted(&self) -> u64 {
        self.submitted
    }

    /// Reads back what timings it can without rendering a frame. Call it after
    /// waiting for the GPU (native: `device.poll(wait)`) to collect the last
    /// frames' timings.
    pub fn drain_gpu_timings(&mut self) {
        self.timer.drain(&self.device, &self.queue);
    }

    /// Bloom on HDR highlights (`BLOOM` in `DisplayTransform.ts`), or `None` for
    /// none (the default, and the Low preset's).
    pub fn set_bloom(&mut self, bloom: Option<BloomSettings>) {
        self.bloom_on = bloom.is_some();
        if let Some(settings) = bloom {
            self.bloom.settings = settings;
        }
    }

    /// The flashes on the screen this frame, about which the bloom glares ([`crate::post::BloomGlare`]); the first
    /// [`crate::post::BLOOM_GLARES`] are taken. They stand until set again: set an empty list when none burns.
    pub fn set_bloom_glares(&mut self, glares: &[crate::post::BloomGlare]) {
        self.bloom.glares.clear();
        self.bloom.glares.extend_from_slice(glares);
    }

    /// The sun the eye glares at (`crate::sun_glare`: a veil about it by what the frame shows of its disc, added by
    /// the bloom, so it follows the bloom's setting); none once it has set, or where no sky is drawn.
    pub fn set_sun_glare(&mut self, sun: Option<crate::photo_optics::FlareSun>) {
        self.sun_glare.sun = sun;
    }

    pub fn bloom(&self) -> Option<BloomSettings> {
        self.bloom_on.then_some(self.bloom.settings)
    }

    /// The bloom's composite (half resolution) once a frame has drawn it.
    pub fn bloom_texture(&self) -> Option<&wgpu::Texture> {
        self.bloom.output_texture()
    }

    /// The eye adaptation's settings. Off by default; turning it off eases the
    /// eye back to the fixed exposure.
    pub fn set_eye_adaptation(&mut self, settings: EyeAdaptationSettings) {
        self.eye.settings = settings;
    }

    pub fn eye_adaptation(&self) -> EyeAdaptationSettings {
        self.eye.settings
    }

    /// Where the eye adaptation reads the sky's mean radiance (the web's
    /// `pmremTexture(environment, up, 1)`). Until one is set the eye holds.
    pub fn set_sky_mean(&mut self, sky_mean: SkyMean) {
        self.eye.sky_mean = sky_mean;
    }

    pub fn sky_mean(&self) -> SkyMean {
        self.eye.sky_mean
    }

    /// Where the night grade reads which pixels look through water: a texture over the screen, r the metres under
    /// water where each pixel's view ray starts (positive under), as the ocean's underwater probe writes it. Those
    /// pixels take the grade of the world near the eye whatever their depth: the open deep has none and is not sky.
    /// `None` (the default) for a scene without a sea.
    pub fn set_underwater(&mut self, probe: Option<&wgpu::TextureView>) {
        if self.underwater.0.as_ref() != probe {
            self.underwater = (probe.cloned(), self.underwater.1 + 1);
        }
    }

    /// The next frame's eye settles at once on its target (the web game's
    /// `EyeAdaptation.snap()`): a capture must not depend on the frames before it.
    /// Who sets the exposure over the fixed one: the eye ([`ExposureControl::Auto`]), held where it is, or set.
    pub fn set_exposure_control(&mut self, control: ExposureControl) {
        self.eye.control = control;
    }

    pub fn exposure_control(&self) -> ExposureControl {
        self.eye.control
    }

    /// Reads the eye's state back after the next frame, without waiting; [`Renderer::eye_reading`] has it a few
    /// frames later.
    pub fn request_eye_reading(&mut self) {
        self.eye.request_reading();
    }

    /// The eye's state as last read back: stops open (the display's exposure is the fixed one × `exp2` of them),
    /// metered, target, brightest tenth.
    pub fn eye_reading(&self) -> Option<[f32; 4]> {
        self.eye.reading()
    }

    /// The eye readings arrived so far, and whether another is in flight (see `EyeAdaptation::reading_serial`).
    pub fn eye_reading_serial(&self) -> (u64, bool) {
        self.eye.reading_serial()
    }

    /// The debug views this device can show (the wireframe needs `Features::POLYGON_MODE_LINE`).
    pub fn debug_views(&self) -> Vec<DebugView> {
        DebugView::offered(self.device.features())
    }

    pub fn debug_view(&self) -> DebugView {
        self.debug.view
    }

    /// Shows a debug view (see [`crate::debug_view`]); `Off` is the frame as the game draws it. Refused, with the
    /// reason, for a view this device cannot show. A view's pipelines are made when it is first chosen.
    pub fn set_debug_view(&mut self, view: DebugView) -> Result<(), String> {
        if !self.debug_views().contains(&view) {
            return Err(format!(
                "this GPU cannot draw the {} view (it lacks line polygons, wgpu's POLYGON_MODE_LINE)",
                view.name()
            ));
        }
        if view != DebugView::Off && view != DebugView::Wireframe {
            let mut debug = std::mem::take(&mut self.debug);
            let built = debug.build(&mut self.context());
            self.debug = debug;
            built.map_err(|error| format!("the {} view's shader: {error}", view.name()))?;
        }
        let wireframe = view == DebugView::Wireframe;
        if self.pipelines.wireframe() != wireframe {
            self.pipelines
                .set_wireframe(&self.device, &mut self.library, wireframe);
        }
        self.debug.view = view;
        Ok(())
    }

    pub fn snap_eye_adaptation(&mut self) {
        self.eye.snap();
        self.sun_glare.snap();
    }

    /// Back to the fixed exposure (a new scene).
    pub fn reset_eye_adaptation(&mut self) {
        self.eye.reset(&self.queue, &self.resources);
    }

    /// Edge smoothing by the Graphics setting's names (see [`crate::antialiasing`]): none (the default),
    /// FXAA, SMAA, or temporal AA with FXAA where it takes no history. Turning temporal AA on
    /// starts without history.
    pub fn set_antialiasing(&mut self, antialiasing: Antialiasing) {
        if antialiasing == Antialiasing::Taa && self.antialiasing != Antialiasing::Taa {
            self.taa.reset();
        }
        self.antialiasing = antialiasing;
    }

    pub fn antialiasing(&self) -> Antialiasing {
        self.antialiasing
    }

    /// The scene's size over the output's: the Graphics setting's `renderScale` / 100, clamped to 0.5–1.
    /// Below 1 the scene and the post chain draw smaller and an upscale stretches the frame onto
    /// the output; overlays stay at full resolution (see [`crate::antialiasing`]).
    pub fn set_render_scale(&mut self, scale: f32) {
        self.render_scale = clamp_render_scale(scale);
    }

    pub fn render_scale(&self) -> f32 {
        self.render_scale
    }

    /// Temporal anti-aliasing (`TemporalAntialiasing.ts`) with these settings, or `None`
    /// to turn it off (edge smoothing then goes to none; FXAA and SMAA are left as they
    /// are). The same as [`Renderer::set_antialiasing`] with [`Antialiasing::Taa`], plus
    /// its tunables.
    pub fn set_taa(&mut self, taa: Option<TaaSettings>) {
        match taa {
            Some(settings) => {
                self.set_antialiasing(Antialiasing::Taa);
                self.taa.settings = settings;
            }
            None if self.antialiasing == Antialiasing::Taa => {
                self.antialiasing = Antialiasing::None;
            }
            None => {}
        }
    }

    pub fn taa(&self) -> Option<TaaSettings> {
        (self.antialiasing == Antialiasing::Taa).then_some(self.taa.settings)
    }

    /// Whether the latest frame blended history (false on the first frame, after a
    /// reset or a resize, and without temporal AA).
    pub fn taa_history_valid(&self) -> bool {
        self.antialiasing == Antialiasing::Taa && self.taa.history_valid()
    }

    /// Drops temporal history: after a camera cut or a scene change a capture must
    /// not smear.
    pub fn reset_temporal(&mut self) {
        self.taa.reset();
    }

    /// Scene copies to take every frame whatever the layers ask for: for
    /// measuring what copies cost, and for looking at them.
    pub fn set_extra_scene_copies(&mut self, copies: SceneCopies) {
        self.extra_copies = copies;
    }

    /// The shadow maps' graphics settings (off by default).
    pub fn shadow_settings(&self) -> ShadowSettings {
        self.shadow_fit.settings
    }

    /// Applies shadow settings from the next frame; a new map size remakes the maps.
    pub fn set_shadow_settings(&mut self, settings: ShadowSettings) {
        self.shadow_fit.settings = settings;
    }

    /// The light the shadow maps are drawn for, and what they follow. `None` draws
    /// no shadows. Set it every frame the sun, the subject or the ships in view move.
    pub fn set_sun(&mut self, sun: Option<Sun>) {
        self.sun = sun;
    }

    pub fn sun(&self) -> Option<&Sun> {
        self.sun.as_ref()
    }

    /// The maps as fitted for the latest frame.
    pub fn shadow_fit(&self) -> &ShadowFit {
        &self.shadow_fit
    }

    /// The shadow maps' textures: the near, view and land maps' array (layers 0, 1, 2,
    /// then 3, 4, 5 with the land maps on) and the wide map, for diagnostics.
    pub fn shadow_textures(&self) -> (&wgpu::Texture, &wgpu::Texture) {
        (&self.shadow_targets.cascades, &self.shadow_targets.wide)
    }

    pub fn shadow_report(&self) -> ShadowReport {
        let fit = &self.shadow_fit;
        ShadowReport {
            enabled: fit.settings.enabled() && self.sun.is_some(),
            map_size: fit.settings.map_size,
            soft: fit.settings.soft,
            maps: fit
                .maps
                .iter()
                .map(|map| (map.kind, map.size, map.on, map.drawn, map.half_width))
                .collect(),
        }
    }
}

/// The phases each render pass of the scene draws, and the copy point that ends
/// it: a pass ends only where `copies` asks for a copy.
pub(crate) fn scene_pass_groups(copies: SceneCopies) -> Vec<(Vec<Phase>, Option<CopyPoint>)> {
    let mut groups = Vec::new();
    let mut phases = Vec::new();
    for phase in Phase::ALL {
        phases.push(phase);
        if let Some(point) = phase
            .copy_point_after()
            .filter(|point| copies.intersects(point.copies()))
        {
            groups.push((std::mem::take(&mut phases), Some(point)));
        }
    }
    if !phases.is_empty() {
        groups.push((phases, None));
    }
    groups
}

/// Registers the textures for `copies` that do not exist yet: full-size viewport
/// textures in the scene targets' formats.
fn ensure_scene_copies(device: &wgpu::Device, resources: &mut Resources, copies: SceneCopies) {
    for (_, name, depth) in copies.entries() {
        if resources.texture(name).is_some() {
            continue;
        }
        let format = if depth { DEPTH_FORMAT } else { HDR_FORMAT };
        resources.create_texture(
            device,
            name,
            TextureDesc::viewport(
                format,
                // COPY_SRC for captures and tests that read a copy back.
                wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                1.0,
            ),
        );
    }
}

/// Copies the scene targets into each of `copies` (whole textures, as a depth
/// copy must be).
fn copy_scene(encoder: &mut wgpu::CommandEncoder, resources: &Resources, copies: SceneCopies) {
    for (_, name, depth) in copies.entries() {
        let source = resources
            .texture(if depth { SCENE_DEPTH } else { SCENE_HDR })
            .expect("registered in new");
        let target = resources
            .texture(name)
            .expect("made by ensure_scene_copies");
        encoder.copy_texture_to_texture(
            source.texture.as_image_copy(),
            target.texture.as_image_copy(),
            source.texture.size(),
        );
    }
}

/// One render pass of the scene: clears the targets for the first, loads them after.
fn begin_scene_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    resources: &Resources,
    label: &str,
    load: bool,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
) -> wgpu::RenderPass<'e> {
    let hdr = resources.texture(SCENE_HDR).expect("registered in new");
    let depth = resources.texture(SCENE_DEPTH).expect("registered in new");
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &hdr.view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if load {
                    wgpu::LoadOp::Load
                } else {
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                },
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &depth.view,
            depth_ops: Some(wgpu::Operations {
                load: if load {
                    wgpu::LoadOp::Load
                } else {
                    wgpu::LoadOp::Clear(DEPTH_CLEAR)
                },
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes,
        ..Default::default()
    })
}

/// A shadow map pass's label (`shadow <map>`), named once.
fn shadow_label(kind: crate::shadows::ShadowMapKind) -> &'static str {
    use crate::shadows::ShadowMapKind;
    match kind {
        ShadowMapKind::Near => "shadow near",
        ShadowMapKind::Wide => "shadow wide",
        ShadowMapKind::Aboard => "shadow aboard",
        ShadowMapKind::View(0) => "shadow view 1",
        ShadowMapKind::View(_) => "shadow view 2",
        ShadowMapKind::Land(0) => "shadow land 1",
        ShadowMapKind::Land(1) => "shadow land 2",
        ShadowMapKind::Land(_) => "shadow land 3",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scene_pass_ends_only_where_a_copy_is_asked_for() {
        let groups = scene_pass_groups(SceneCopies::NONE);
        assert_eq!(groups, [(Phase::ALL.to_vec(), None)]);
        let groups = scene_pass_groups(SceneCopies::OPAQUE_DEPTH);
        assert_eq!(
            groups,
            [
                (vec![Phase::Opaque], Some(CopyPoint::Opaque)),
                (vec![Phase::Water, Phase::Sky, Phase::Transparent], None),
            ]
        );
        let groups = scene_pass_groups(SceneCopies::ALL);
        assert_eq!(
            groups,
            [
                (vec![Phase::Opaque], Some(CopyPoint::Opaque)),
                (vec![Phase::Water, Phase::Sky], Some(CopyPoint::Backdrop)),
                (vec![Phase::Transparent], None),
            ]
        );
        let groups = scene_pass_groups(SceneCopies::BACKDROP_COLOR);
        assert_eq!(
            groups,
            [
                (
                    vec![Phase::Opaque, Phase::Water, Phase::Sky],
                    Some(CopyPoint::Backdrop)
                ),
                (vec![Phase::Transparent], None),
            ]
        );
    }
}
