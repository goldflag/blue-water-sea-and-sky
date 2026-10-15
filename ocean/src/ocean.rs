//! The facade a renderer holds, after the web game's `ocean/Ocean.ts`: the wave field and the surface behind one object whose
//! parameters are live. It is a `naval_render::SceneLayer`: `prepare` advances the sea by the frame's `dt` (0 on a
//! paused frame) and encodes the field's compute passes on the frame's encoder, and `draw` draws the surface in the
//! water phase (after the opaque scene, before the sky; the camera and the sun's shadow maps at group 0, its own groups
//! 1–3). With [`Ocean::read_scene_copies`] it asks the renderer for copies of the opaque scene and binds them, so hulls
//! show through the water and draw their waterline foam. `draw_motion` writes the sea's temporal response (no history)
//! for temporal anti-aliasing. Without the renderer core, call [`Ocean::update`], [`Ocean::prepare_wake`],
//! [`OceanSurface::prepare`] and [`Ocean::draw`] with a camera bound at group 0.
//!
//! With [`Ocean::enable_wake`] it also holds the ship wakes ([`ShipWake`]): the host feeds them each frame through
//! [`ShipWake::update`] (`ocean.wake.as_mut()`), and `prepare` encodes their GPU work before the surface draws them.

use naval_render::{
    CopyPoint, DrawContext, MakePipelines, Phase, PipelineRegistry, PlaceholderPipelines,
    PrepareContext, RenderContext, SCENE_OPAQUE_COLOR, SCENE_OPAQUE_DEPTH, SceneCopies, SceneLayer,
    ShaderError,
};

use crate::spindrift::{Spindrift, spray_haze};

use crate::calibration::{NORTH_ATLANTIC, scene_parameters};
use crate::field::FieldPipelines;
use crate::ship_wake::{ShipWake, ShipWakePipelines};
use crate::surface::SurfacePipelines;

/// The ocean's pipelines on the renderer's registry ([`Ocean::register_pipelines`]).
#[derive(Clone, Copy, Debug)]
pub struct OceanPipelines {
    field: FieldPipelines,
    surface: SurfacePipelines,
    /// The wakes', once they are enabled through the registry ([`Ocean::enable_wake_in`]).
    wake: Option<ShipWakePipelines>,
}
use crate::{OceanParameters, OceanSurface, QualityTier, WaveField};

/// How long (s of the frames' clock) the sea takes to let the hulls go or take them back ([`Ocean::hull_rate`]): an
/// e-folding time, so a change of speed swaps the swell about them over a second or two rather than at once.
pub const RACING_EASE_S: f64 = 0.5;

pub struct Ocean {
    pub quality: QualityTier,
    /// Every parameter the game sets per scene; changing the waves rebuilds the spectrum on the next update.
    pub parameters: OceanParameters,
    /// Wave time in seconds. Settable for fixed captures; a paused frame never advances it.
    pub time: f64,
    /// Sea seconds per frame second (1 by default): the waves' phases, their foam and the open sea's swell run this much
    /// faster than the frames (a theatre's running speed, held to its sea's presentation rate).
    pub time_scale: f64,
    /// Simulation seconds per frame second the hulls are drawn at (1 by default). Above `time_scale` the sines the
    /// hulls ride would race the rest of the swell, so the sea lets the hulls go (`racing` eases to 1 over
    /// [`RACING_EASE_S`]) and draws the swell on its own clock about them too ([`crate::hull_sea::HullSeaUniforms::set`]).
    pub hull_rate: f64,
    racing: f64,
    /// A coast to read the waves against, set with [`Ocean::set_shore`] and bound on the next prepare.
    pending_shore: Option<Option<std::sync::Arc<crate::shore::ShoreMap>>>,
    pub field: WaveField,
    pub surface: OceanSurface,
    /// The ship wakes the surface shades, when enabled.
    pub wake: Option<ShipWake>,
    /// The camera's height probe for the underwater view (`crate::underwater`), run while the near plane may reach
    /// the water.
    pub underwater: crate::underwater::UnderwaterProbe,
    /// The sea's height about each coupled hull for the hulls' wet band (`crate::hull_heights`), once enabled
    /// ([`Ocean::enable_hull_heights_in`]).
    pub hull_heights: Option<crate::hull_heights::HullHeights>,
    /// Spray torn off the crests in a gale ([`crate::spindrift`]), drawn in the transparent phase once the ocean is on the
    /// renderer's registry; the host sets its lightning `flash` each frame.
    pub spindrift: Spindrift,
    /// The sea's height about the camera for the rain's splashes ([`crate::rain_heights`]), made with the registry's
    /// pipelines and evaluated while it rains.
    rain_heights: Option<crate::rain_heights::RainHeights>,
    /// The sunlight the waves gather on what stands under the sea ([`crate::caustics`]), made with the registry's
    /// pipelines and evaluated while the camera is low enough to see below the surface.
    caustics: Option<crate::caustics::Caustics>,
    /// Which tiles of the opaque depth copy hold a surface, so the screen-space reflections trace only rays that may
    /// meet one (`crate::reflection_tiles`); built after each frame's copies while the ocean reads them.
    reflection_tiles: crate::reflection_tiles::ReflectionTiles,
    /// The wake version the surface is bound to (`Some(None)`: a calm wake); `None` forces a rebind.
    wake_bound: Option<Option<u64>>,
    /// The sky environment the sea reflects (naval-sky's bind group), and its layout.
    environment: wgpu::BindGroup,
    environment_layout: wgpu::BindGroupLayout,
    /// Ask the renderer for copies of the opaque scene each frame and bind them (`scene.opaque.color` and
    /// `scene.opaque.depth`): transmission onto submerged hulls and the waterline's foam. Off by default, for hosts
    /// that hand their own copies to [`Ocean::set_scene`].
    pub read_scene_copies: bool,
    /// The copies' versions the surface is bound to.
    copies_bound: Option<(u64, u64)>,
    /// The coupled hulls' centres (x, z), which a magnified view centres the clipmap on ([`crate::geometry::focus`]).
    focus_hulls: Vec<(f64, f64)>,
    /// The pipelines on the renderer's registry, taken in at each prepare ([`Ocean::new_in`],
    /// [`Ocean::register_pipelines`]).
    registry: Option<OceanPipelines>,
    /// The renderer's stand-ins an ocean built with [`Ocean::new_in`] holds until its first prepare.
    placeholders: Option<PlaceholderPipelines>,
}

impl Ocean {
    /// An ocean for `quality` showing the port's sea (9 m/s on the North Atlantic's water). `camera` is the renderer's
    /// camera layout (`RenderContext::camera_layout`); `environment_layout` and `environment` are the sky's
    /// (`naval_sky::EnvironmentMap`, or [`crate::environment::StandInEnvironment`]). The horizon ring covers a far plane
    /// of `far` metres.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        quality: QualityTier,
        camera: &wgpu::BindGroupLayout,
        environment_layout: &wgpu::BindGroupLayout,
        environment: &wgpu::BindGroup,
        far: f64,
    ) -> Self {
        Self::build(
            device,
            queue,
            quality,
            camera,
            environment_layout,
            environment,
            far,
            None,
        )
    }

    /// An ocean on the renderer's registry from the start, as [`Ocean::new`] with the renderer's camera layout: its
    /// pipelines are registered at once and compile on the registry's build threads beside every other layer's, the
    /// first prepare takes them in, and their WGSL hot-reloads. Nothing compiles on the calling thread.
    pub fn new_in(
        ctx: &mut RenderContext<'_>,
        quality: QualityTier,
        environment_layout: &wgpu::BindGroupLayout,
        environment: &wgpu::BindGroup,
        far: f64,
    ) -> Result<Self, ShaderError> {
        let placeholders = ctx.placeholders().clone();
        let mut ocean = Self::build(
            ctx.device,
            ctx.queue,
            quality,
            ctx.camera_layout,
            environment_layout,
            environment,
            far,
            Some(&placeholders),
        );
        ocean.placeholders = Some(placeholders);
        ocean.register_pipelines(ctx)?;
        Ok(ocean)
    }

    /// Builds the field's, the surface's and (when enabled) the wakes' pipelines through the renderer's registry, as
    /// the ocean builds them, so their WGSL hot-reloads; each prepare then takes the registry's current ones.
    pub fn register_pipelines(&mut self, ctx: &mut RenderContext<'_>) -> Result<(), ShaderError> {
        crate::shaders::register(ctx)?;
        let waves = self.field.surface_layout().clone();
        self.underwater.register(ctx, &waves)?;
        if let Some(heights) = &mut self.hull_heights {
            heights.register(ctx, &waves)?;
        }
        let environment = self.environment_layout.clone();
        self.spindrift.register(ctx, &waves, &environment)?;
        if self.rain_heights.is_none() {
            let placeholders = ctx.placeholders().clone();
            let mut heights = crate::rain_heights::RainHeights::new(
                ctx.device,
                &waves,
                MakePipelines::Deferred(&placeholders),
            );
            heights.bind_wake(
                ctx.device,
                self.wake.as_ref().map(ShipWake::bindings).as_ref(),
            );
            self.rain_heights = Some(heights);
        }
        if let Some(heights) = &mut self.rain_heights {
            heights.register(ctx, &waves)?;
        }
        if self.caustics.is_none() {
            let placeholders = ctx.placeholders().clone();
            let mut caustics = crate::caustics::Caustics::new(
                ctx.device,
                &waves,
                self.quality.cascades().len(),
                MakePipelines::Deferred(&placeholders),
            );
            caustics.bind_wake(
                ctx.device,
                self.wake.as_ref().map(ShipWake::bindings).as_ref(),
            );
            self.caustics = Some(caustics);
        }
        if let Some(caustics) = &mut self.caustics {
            caustics.register(ctx, &waves)?;
        }
        self.reflection_tiles.register(ctx)?;
        self.registry = Some(OceanPipelines {
            field: self.field.register_pipelines(ctx)?,
            surface: self.surface.register_pipelines(ctx)?,
            wake: self
                .wake
                .as_ref()
                .map(|wake| wake.register_pipelines(ctx))
                .transpose()?,
        });
        Ok(())
    }

    /// Takes the registry's current pipelines in place of the ocean's own; whether any changed.
    pub fn adopt_pipelines(&mut self, registry: &PipelineRegistry) -> bool {
        let Some(ids) = self.registry else {
            return false;
        };
        let mut changed = self.field.adopt_pipelines(registry, &ids.field)
            | self.surface.adopt_pipelines(registry, &ids.surface)
            | self.underwater.adopt(registry)
            | self.reflection_tiles.adopt(registry)
            | self
                .rain_heights
                .as_mut()
                .is_some_and(|heights| heights.adopt(registry))
            | self
                .caustics
                .as_mut()
                .is_some_and(|caustics| caustics.adopt(registry))
            | self
                .hull_heights
                .as_mut()
                .is_some_and(|heights| heights.adopt(registry));
        if let (Some(wake), Some(wake_ids)) = (&mut self.wake, &ids.wake) {
            changed |= wake.adopt_pipelines(registry, wake_ids);
        }
        if self.wake.is_none() || ids.wake.is_some() {
            self.placeholders = None;
        }
        changed
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        quality: QualityTier,
        camera: &wgpu::BindGroupLayout,
        environment_layout: &wgpu::BindGroupLayout,
        environment: &wgpu::BindGroup,
        far: f64,
        placeholders: Option<&PlaceholderPipelines>,
    ) -> Self {
        let make = MakePipelines::from_placeholders(placeholders);
        let field = WaveField::build(device, &quality.cascades(), make);
        let surface = OceanSurface::build(
            device,
            queue,
            quality,
            &field,
            camera,
            environment_layout,
            far,
            make,
        );
        let underwater = crate::underwater::UnderwaterProbe::new(
            device,
            surface.probe_view(),
            field.surface_layout(),
            make,
        );
        let spindrift = Spindrift::new(
            device,
            crate::surface::placeholder_cloud_shadow(device),
            surface.foam(),
        );
        let reflection_tiles = crate::reflection_tiles::ReflectionTiles::new(device, make);
        Self {
            quality,
            parameters: scene_parameters(&NORTH_ATLANTIC, 9.0, true),
            time: 0.0,
            time_scale: 1.0,
            hull_rate: 1.0,
            racing: 0.0,
            pending_shore: None,
            field,
            surface,
            wake: None,
            underwater,
            hull_heights: None,
            spindrift,
            rain_heights: None,
            caustics: None,
            reflection_tiles,
            wake_bound: Some(None),
            environment: environment.clone(),
            environment_layout: environment_layout.clone(),
            read_scene_copies: false,
            copies_bound: None,
            focus_hulls: Vec::new(),
            registry: None,
            placeholders: None,
        }
    }

    /// Shade ship wakes: a wake field of the tier's resolution (`quality.ts`; none on Low) and the trail, torpedo, bow
    /// wave and contact foam systems.
    pub fn enable_wake(&mut self, device: &wgpu::Device) {
        assert!(
            self.placeholders.is_none(),
            "an ocean built with Ocean::new_in enables its wakes with Ocean::enable_wake_in"
        );
        let clipmap = self.quality.clipmap();
        let mut wake = ShipWake::new(
            device,
            crate::wake::resolution_for(self.quality),
            clipmap.cell(0),
        );
        wake.bow_waves.mesh_growth = clipmap.cell(0) / clipmap.half(0);
        self.wake = Some(wake);
        self.wake_bound = None;
    }

    /// [`Ocean::enable_wake`] on the renderer's registry: the wakes' pipelines compile on its build threads and hot-
    /// reload. An ocean not on the registry yet ([`Ocean::new`]) registers its own pipelines first.
    pub fn enable_wake_in(&mut self, ctx: &mut RenderContext<'_>) -> Result<(), ShaderError> {
        if self.registry.is_none() {
            self.register_pipelines(ctx)?;
        }
        let placeholders = ctx.placeholders().clone();
        let clipmap = self.quality.clipmap();
        let mut wake = ShipWake::build(
            ctx.device,
            crate::wake::resolution_for(self.quality),
            clipmap.cell(0),
            MakePipelines::Deferred(&placeholders),
        );
        wake.bow_waves.mesh_growth = clipmap.cell(0) / clipmap.half(0);
        crate::shaders::register(ctx)?;
        let ids = wake.register_pipelines(ctx)?;
        self.wake = Some(wake);
        self.wake_bound = None;
        if let Some(registry) = &mut self.registry {
            registry.wake = Some(ids);
        }
        if self.placeholders.is_none() {
            self.placeholders = Some(placeholders);
        }
        Ok(())
    }

    /// Evaluate the sea's height about each coupled hull every frame, for the hulls' wet band (`crate::hull_heights`):
    /// the hulls [`Ocean::set_hull_sea`] couples get a patch each, and [`Ocean::hull_heights`] is what the ships bind.
    pub fn enable_hull_heights(&mut self, device: &wgpu::Device) {
        assert!(
            self.registry.is_none(),
            "an ocean on the renderer's registry enables its hull heights with Ocean::enable_hull_heights_in"
        );
        self.hull_heights = Some(crate::hull_heights::HullHeights::new(
            device,
            self.field.surface_layout(),
            MakePipelines::Now,
        ));
        self.wake_bound = None;
    }

    /// [`Ocean::enable_hull_heights`] on the renderer's registry: the pass compiles on its build threads and hot-reloads.
    /// An ocean not on the registry yet ([`Ocean::new`]) registers its own pipelines first.
    pub fn enable_hull_heights_in(
        &mut self,
        ctx: &mut RenderContext<'_>,
    ) -> Result<(), ShaderError> {
        if self.registry.is_none() {
            self.register_pipelines(ctx)?;
        }
        let placeholders = ctx.placeholders().clone();
        let waves = self.field.surface_layout().clone();
        let mut heights = crate::hull_heights::HullHeights::new(
            ctx.device,
            &waves,
            MakePipelines::Deferred(&placeholders),
        );
        heights.register(ctx, &waves)?;
        self.hull_heights = Some(heights);
        self.wake_bound = None;
        if self.placeholders.is_none() {
            self.placeholders = Some(placeholders);
        }
        Ok(())
    }

    /// The sea's heights about the camera the rain's splashes stand on ([`crate::rain_heights`]: metres in red,
    /// [`crate::rain_heights::SPAN`] metres a repeat), once the ocean is on the renderer's registry.
    pub fn rain_heights(&self) -> Option<&wgpu::TextureView> {
        self.rain_heights
            .as_ref()
            .map(crate::rain_heights::RainHeights::view)
    }

    /// What the ships bind of the heights about the hulls, once enabled.
    pub fn hull_heights(&self) -> Option<crate::hull_heights::HullHeightsBindings<'_>> {
        self.hull_heights
            .as_ref()
            .map(crate::hull_heights::HullHeights::bindings)
    }

    /// Point the surface at copies of the opaque scene (see [`OceanSurface::set_scene`]), keeping the wakes bound.
    pub fn set_scene(
        &mut self,
        device: &wgpu::Device,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        self.surface.set_scene(device, color, depth);
        self.wake_bound = None;
        self.bind_wake(device);
    }

    /// Shade the clouds' shadow on the sea (`naval_clouds::CloudShadow`'s map, sampler and uniforms), or none, keeping
    /// the wakes bound. Call again when the clouds remake it (a quality change).
    pub fn set_cloud_shadow(
        &mut self,
        device: &wgpu::Device,
        shadow: Option<(&wgpu::TextureView, &wgpu::Sampler, &wgpu::Buffer)>,
    ) {
        self.surface.set_cloud_shadow(device, shadow);
        self.spindrift.set_cloud_shadow(match shadow {
            Some((map, sampler, uniforms)) => (map.clone(), sampler.clone(), uniforms.clone()),
            None => crate::surface::placeholder_cloud_shadow(device),
        });
        self.wake_bound = None;
        self.bind_wake(device);
    }

    /// Binds the renderer's opaque copies when they were made or remade (a resize).
    fn follow_scene_copies(&mut self, ctx: &PrepareContext<'_>) {
        let (Some(color), Some(depth)) = (
            ctx.resources.texture(SCENE_OPAQUE_COLOR),
            ctx.resources.texture(SCENE_OPAQUE_DEPTH),
        ) else {
            return;
        };
        let versions = (color.version, depth.version);
        if self.copies_bound != Some(versions) {
            let size = (depth.texture.width(), depth.texture.height());
            let (color, depth_view) = (color.view.clone(), depth.view.clone());
            self.reflection_tiles
                .follow(ctx.device, &depth_view, depth.version, size);
            if let Some(table) = self.reflection_tiles.table() {
                self.surface.set_reflection_table(table);
            }
            self.set_scene(ctx.device, &color, &depth_view);
            self.copies_bound = Some(versions);
        }
    }

    fn bind_wake(&mut self, device: &wgpu::Device) {
        let version = self.wake.as_ref().map(|wake| wake.version);
        if self.wake_bound == Some(version) {
            return;
        }
        let bindings = self.wake.as_ref().map(ShipWake::bindings);
        self.surface.set_wake(device, bindings.as_ref());
        self.underwater.bind_wake(device, bindings.as_ref());
        if let Some(heights) = &mut self.hull_heights {
            heights.bind_wake(device, bindings.as_ref());
        }
        if let Some(heights) = &mut self.rain_heights {
            heights.bind_wake(device, bindings.as_ref());
        }
        if let Some(caustics) = &mut self.caustics {
            caustics.bind_wake(device, bindings.as_ref());
        }
        self.wake_bound = Some(version);
    }

    /// Encode the wakes' GPU work for this frame (after [`ShipWake::update`]) and keep the surface bound to them.
    pub fn prepare_wake(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        self.prepare_wake_timed(
            device,
            &mut naval_render::UploadSink::Queue(queue),
            encoder,
            None,
        );
    }

    /// [`Ocean::prepare_wake`], the wake field's steps timed by the renderer core's timer, its writes through `uploads`
    /// (the renderer core's staging belt in `prepare`).
    pub fn prepare_wake_timed(
        &mut self,
        device: &wgpu::Device,
        uploads: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        if let Some(wake) = &mut self.wake {
            wake.encode_timed(device, uploads, encoder, timer);
        }
        self.bind_wake(device);
    }

    /// Around every drawn hull the water shows the sea it rides (`Ocean.setHullSea`, `waves/hullSea.ts`): combat's
    /// `waves` at `time` (the simulation's presentation clock), near the `hulls` (the nearest
    /// [`crate::hull_sea::HULL_SEA_SLOTS`] to the camera, [`crate::hull_sea::hull_footprints`]), about the camera's
    /// `origin` (x, z). No waves or no hulls: the sea as drawn. Only the realistic sea state couples. With
    /// [`Ocean::enable_hull_heights_in`], each of the `hulls` also gets a patch of the sea's heights for its wet band,
    /// coupled or not, since the band follows the drawn sea whatever it is.
    pub fn set_hull_sea(
        &mut self,
        waves: &[crate::hull_sea::HullSeaWave],
        time: f64,
        hulls: &[crate::hull_sea::HullFootprint],
        origin: (f64, f64),
    ) {
        let clock = crate::field::HullSeaClock {
            swell_time: self.time,
            racing: self.racing,
        };
        self.field
            .couple(waves, time, clock, hulls, origin, self.parameters.sea_state);
        self.focus_hulls.clear();
        self.focus_hulls
            .extend(hulls.iter().map(|hull| (hull.x, hull.z)));
        if let Some(heights) = &mut self.hull_heights {
            heights.place(hulls);
        }
    }

    /// Read the waves against a coast from the next frame ([`crate::shore`]): `map`'s land and fetch, in a chart placed
    /// by [`Ocean::set_shore_offset`]; none, the open sea everywhere.
    pub fn set_shore(&mut self, map: Option<std::sync::Arc<crate::shore::ShoreMap>>) {
        self.pending_shore = Some(map);
    }

    /// Where the coast's chart lies in the world this frame: world = chart + `offset` (its land window's offset).
    pub fn set_shore_offset(&mut self, offset: [f64; 2]) {
        self.field.set_shore_offset(offset);
    }

    /// Whether the waves are read against a coast (one set and bound).
    pub fn has_shore(&self) -> bool {
        self.field.has_shore()
    }

    /// How far the hulls are let go from the sea drawn about them (0 coupled … 1 let go, [`Ocean::hull_rate`]).
    pub fn racing(&self) -> f64 {
        self.racing
    }

    /// Runs on `old`'s clock: a scene put up in another's place keeps its sea's waves, the open sea's swell and how
    /// far the hulls are let go where they were, rather than starting them again.
    pub fn carry_clock(&mut self, old: &Ocean) {
        self.time = old.time;
        self.racing = old.racing;
    }

    /// The highest the drawn surface can reach this frame (`Ocean.update`'s crest and wake bounds), for the underwater
    /// view's uniform branch.
    pub fn surface_bound(&self) -> f64 {
        crate::underwater::surface_bound(
            self.field.max_height,
            self.parameters.waves.significant_height,
        )
    }

    /// The sunlight the waves gather under the sea ([`crate::caustics`]: the gain in red, [`crate::caustics::SPAN`]
    /// metres a repeat, a level per depth), once the ocean is on the renderer's registry. The ships read it on their
    /// submerged plating.
    pub fn caustics(&self) -> Option<&wgpu::TextureView> {
        self.caustics.as_ref().map(crate::caustics::Caustics::view)
    }

    /// The caustics' texture, for read-backs.
    pub fn caustics_texture(&self) -> Option<&wgpu::Texture> {
        self.caustics
            .as_ref()
            .map(crate::caustics::Caustics::texture)
    }

    /// The underwater probe's texture, for [`crate::underwater::UnderwaterLayer`].
    pub fn underwater_probe(&self) -> &wgpu::TextureView {
        self.surface.probe_view()
    }

    /// Reflect another environment (the sky rebuilt its atlas).
    pub fn set_environment(&mut self, environment: &wgpu::BindGroup) {
        self.environment = environment.clone();
    }

    /// Advance the sea by `dt` seconds (0 renders a paused frame: no waves, foam or time advance) and encode the wave
    /// field's compute passes.
    pub fn update(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, dt: f64) {
        self.update_timed(
            queue,
            &mut naval_render::UploadSink::Queue(queue),
            encoder,
            dt,
            None,
        );
    }

    /// [`Ocean::update`], the wave field's compute pass timed by the renderer core's timer (`ocean waves`), its
    /// uniforms through `uploads` (the renderer core's staging belt in `prepare`).
    pub fn update_timed(
        &mut self,
        queue: &wgpu::Queue,
        uploads: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        dt: f64,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        if dt > 0.0 {
            self.time += dt;
        }
        self.field.update_with_timer(
            queue,
            uploads,
            encoder,
            &self.parameters,
            self.time,
            dt.max(0.0),
            timer,
        );
    }

    /// Draw the surface inside a pass with an `HDR_FORMAT` target and a `DEPTH_FORMAT` attachment (reversed depth),
    /// the camera bound at group 0.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.surface.draw(pass, &self.field, &self.environment);
    }
}

impl SceneLayer for Ocean {
    fn label(&self) -> &str {
        "ocean"
    }

    fn scene_copies(&self) -> SceneCopies {
        let mut copies = if self.read_scene_copies {
            SceneCopies::OPAQUE_COLOR | SceneCopies::OPAQUE_DEPTH
        } else {
            SceneCopies::NONE
        };
        // The spindrift fades into the sea and hulls behind it.
        if self.spindrift.active(self.parameters.waves.wind_speed) {
            copies |= SceneCopies::BACKDROP_DEPTH;
        }
        copies
    }

    fn prepare(&mut self, ctx: &mut PrepareContext<'_>) {
        // The registry's pipelines (rebuilt after a shader edit, or first built on its threads) in place of the own.
        self.adopt_pipelines(ctx.pipelines);
        if self.read_scene_copies {
            self.follow_scene_copies(ctx);
        }
        // A coast set since the last frame.
        if let Some(map) = self.pending_shore.take() {
            self.field.set_shore(ctx.device, ctx.queue, map);
        }
        // The hulls are let go while the simulation runs faster than the sea, and taken back once it does not.
        let wall = f64::from(ctx.time.dt);
        if wall > 0.0 {
            let target = if self.hull_rate > self.time_scale * (1.0 + 1e-3) {
                1.0
            } else {
                0.0
            };
            self.racing += (target - self.racing) * (1.0 - (-wall / RACING_EASE_S).exp());
            if (target - self.racing).abs() < 1e-3 {
                self.racing = target;
            }
        }
        // The layer's compute passes in the frame's GPU timings, beside the renderer's own passes; their uniforms
        // through the frame's staging belt.
        let timer = Some(ctx.timer);
        let mut uploads = naval_render::UploadSink::Belt(ctx.uploads);
        self.update_timed(
            ctx.queue,
            &mut uploads,
            ctx.encoder,
            f64::from(ctx.time.dt) * self.time_scale,
            timer,
        );
        // The clipmap centres on the eye, or on what a magnified view is aimed at, and starts at the level the view's
        // standoff from the water there calls for; a magnified view from well above the sea draws a fan over what
        // its lens shows in the clipmap's place. The bow waves' crests flatten by the same mesh.
        let camera = ctx.camera;
        let axis = |column: usize| -> [f64; 3] {
            std::array::from_fn(|row| f64::from(camera.inverse_view[column * 4 + row]))
        };
        let eye = camera.position.map(f64::from);
        let forward = axis(2).map(|v| -v);
        let clipmap = self.surface.clipmap();
        let fan = crate::geometry::fan(
            &crate::geometry::FanView {
                eye,
                forward,
                right: axis(0),
                up: axis(1),
                tan_half_fov_y: f64::from(camera.tan_half_fov_y),
                aspect: f64::from(camera.aspect),
                pixels: f64::from(camera.viewport[1]),
                surface: self.surface_bound(),
                curvature: f64::from(camera.curvature),
                far: f64::from(camera.far),
            },
            clipmap,
        );
        let view = crate::geometry::FocusView {
            eye,
            forward,
            tan_half_fov_y: f64::from(camera.tan_half_fov_y),
            hulls: &self.focus_hulls,
            surface: self.surface_bound(),
        };
        let centre = if fan.is_some() {
            [eye[0], eye[2]]
        } else {
            crate::geometry::focus(&view)
        };
        let standoff = crate::geometry::standoff(&view, centre, self.field.sea.significant_height);
        if let Some(wake) = &mut self.wake {
            let detail = clipmap.detail(standoff);
            let bow = &mut wake.bow_waves;
            bow.mesh_centre = centre;
            bow.mesh_fan = fan.is_some();
            (bow.vertex_spacing, bow.mesh_growth) = match fan {
                Some(fan) => (fan.squeeze, fan.step),
                None => (
                    clipmap.cell(detail.first) * (1.0 + f64::from(detail.morph)),
                    clipmap.cell(0) / clipmap.half(0),
                ),
            };
        }
        self.prepare_wake_timed(ctx.device, &mut uploads, ctx.encoder, timer);
        // The sea about each coupled hull, for the hulls' wet band (drawn after this, in the opaque phase).
        if let Some(heights) = &mut self.hull_heights {
            heights.encode(
                ctx.device,
                &mut uploads,
                ctx.encoder,
                ctx.camera,
                self.field.surface_bind_group(),
                timer,
            );
        }
        // While it rains, the sea about the camera for the splashes.
        if self.surface.rain > 0.0
            && let Some(heights) = &mut self.rain_heights
        {
            heights.encode(
                ctx.device,
                &mut uploads,
                ctx.encoder,
                ctx.camera,
                self.field.surface_bind_group(),
                timer,
            );
        }
        // While the camera is under the sea or low over it, the sunlight the waves gather on what stands in it.
        if ctx.camera.position[1] < crate::caustics::CEILING
            && let Some(caustics) = &mut self.caustics
        {
            caustics.encode(
                ctx.device,
                &mut uploads,
                ctx.encoder,
                ctx.camera,
                self.field.surface_bind_group(),
                timer,
            );
        }
        // While the near plane may reach the water, probe the surface under the camera and the screen.
        let bound = self.surface_bound();
        let probed = self.underwater.encode(
            ctx.device,
            &mut uploads,
            ctx.encoder,
            ctx.camera,
            self.field.surface_bind_group(),
            bound,
            timer,
        );
        // A gale's spray: the haze the surface veils the water with, and the wisps.
        let sea_height = self.field.sea.significant_height;
        self.surface.haze = spray_haze(self.parameters.waves.wind_speed, sea_height);
        self.surface.prepare(
            &mut uploads,
            centre,
            standoff,
            fan,
            &self.parameters,
            probed,
        );
        self.spindrift.prepare(
            ctx,
            &self.parameters,
            self.time,
            sea_height,
            self.surface.water_shadows,
        );
    }

    fn draw(&self, phase: Phase, pass: &mut wgpu::RenderPass<'_>, ctx: &DrawContext<'_>) {
        match phase {
            Phase::Water => Ocean::draw(self, pass),
            Phase::Transparent => self.spindrift.draw(
                pass,
                ctx.pipelines,
                self.field.surface_bind_group(),
                &self.environment,
            ),
            Phase::Opaque | Phase::Sky => {}
        }
    }

    fn after_copies(&mut self, point: CopyPoint, ctx: &mut PrepareContext<'_>) {
        // The tiles of this frame's depth copy, before the surface reads them (the copy the surface is bound to).
        if point == CopyPoint::Opaque && self.read_scene_copies && self.copies_bound.is_some() {
            self.reflection_tiles.set_reach(
                ctx.uploads,
                ctx.camera,
                self.surface.reflections.max_distance,
            );
            self.reflection_tiles.encode(ctx.encoder, Some(ctx.timer));
        }
    }

    fn draw_motion(&self, pass: &mut wgpu::RenderPass<'_>, _ctx: &DrawContext<'_>) {
        self.surface
            .draw_motion(pass, &self.field, &self.environment);
    }
}
