//! The game's sky with its clouds and weather, as the web game's `Sky.ts` composed them: `naval-sky`'s atmosphere,
//! bodies and environment, this crate's clouds (takram's, hooked into the dome and the environment bake), the weather,
//! and heavy rain's veil. One `naval_render::SceneLayer`: the weather, the sky's tables, the clouds' shadow maps and
//! ground shadow, and the environment in `prepare`; the clouds' march and reconstruction at render core II's opaque
//! copy point, over the opaque scene's depth (`after_copies(Opaque)`, `scene.opaque.depth`); the dome then the clouds'
//! composite in the `Sky` phase; splashes, the bolt and rain in `Transparent`. The veil runs after the scene pass
//! through [`GameSky::post_process`], or, with [`GameSky::post_after_copies`], inside the frame at the backdrop copy
//! point (see `native/docs/clouds.md`).

use std::cell::Cell;

use glam::{Mat4, Vec3};
use naval_render::{
    CopyPoint, DrawContext, Phase, PrepareContext, RenderContext, SCENE_BACKDROP_COLOR, SCENE_HDR,
    SCENE_OPAQUE_DEPTH, SceneCopies, SceneLayer, ShaderError, UploadSink,
};
use naval_sky::SkyPipelines;
use naval_sky::{CelestialLight, Sky, SkyQuality, SkyScene, SkyTargets, SkyView};

use crate::Targets;
use crate::aerial::{AerialPerspective, AerialPipelines};
use crate::layer::{CloudLayer, CloudPipelines, CloudView};
use crate::lightning::{StrikeKind, Thunder};
use crate::post::{PostPipelines, PostProcess};
use crate::weather::{Weather, WeatherPipelines};

/// Every pipeline of the game sky on the renderer's registry.
#[derive(Clone, Debug)]
struct Registered {
    sky: SkyPipelines,
    clouds: CloudPipelines,
    weather: WeatherPipelines,
    post: PostPipelines,
    aerial: AerialPipelines,
}

/// A camera that moves farther than this in one frame (m) or turns more (radians) has cut; so has a scene whose sun
/// moves more than 2° or whose cover changes more than 0.05 (`Sky.ts`).
const CUT_DISTANCE: f32 = 250.0;
const CUT_TURN: f32 = 0.5;
const CUT_SUN: f64 = 2.0 * std::f64::consts::PI / 180.0;
const CUT_COVER: f64 = 0.05;
/// A weather blend ([`GameSky::blend`]) applies the sky's light (the sun's strength, the environment's bake) in steps of
/// this share of the change: the clouds and the rain follow every frame, and the bake sweeps the clouds in between.
pub const BLEND_LIGHT_STEP: f64 = 1.0 / 48.0;

/// The sky facade. Reach the parts through `sky`, `clouds`, `weather` and `post`.
pub struct GameSky {
    pub sky: Sky,
    pub clouds: CloudLayer,
    pub weather: Weather,
    pub post: PostProcess,
    /// The aerial perspective every material, the dome and the clouds read: the atmosphere and the weather's haze
    /// (`crate::aerial`), filled each frame after the clouds' shadows.
    pub aerial: AerialPerspective,
    /// True while the camera looks into a hull (the port's cutaways, the damage X-ray): rain stays outside.
    pub sheltered: bool,
    /// Profiling: false skips the whole sky as a layer (no passes, no draws), as the web game's `benchmarkSky` did.
    pub enabled: bool,
    /// Run the veil inside the renderer core's frame: ask for the backdrop's colour copy while it shows and draw it into
    /// `scene.hdr` at the backdrop copy point, before the transparents. Off by default, for hosts that call
    /// [`GameSky::post_process`] themselves.
    pub post_after_copies: bool,
    /// Sky seconds per frame second (1 by default): the clouds' drift, their evolving shapes and the stars' twinkle.
    /// The theatre keeps this at 1 even when its ships and sea accelerate; the rain and the lightning also keep the
    /// frame's time.
    pub time_scale: f64,
    /// A visual battle's cloud layers and drift. Port and independent captures use their scene weather.
    pub authoritative_clouds: Option<naval_protocol::render::CloudRender>,
    /// Whether this frame asked for the backdrop copy (so a copy point another layer asked for is not read stale).
    asked_copies: Cell<bool>,
    quality: SkyQuality,
    scene: SkyScene,
    /// How far a weather blend ([`GameSky::blend`]) had come when the sky's light last took it; `None` outside a blend.
    blend_light: Option<f64>,
    cut: bool,
    last: Option<(Vec3, Vec3, f32)>,
    clock: Option<f64>,
    /// The first prepare is a frame of play, not a still picture ([`GameSky::start_in_play`]).
    starts_in_play: bool,
    /// The renderer core's frame index last prepared: a new one at the same time is a paused picture's.
    frame: Option<u64>,
    /// Whether the frame last prepared was a paused picture's.
    still: bool,
    view: Option<CloudView>,
    /// The sky's, the clouds', the weather's and the veil's pipelines on the renderer's registry
    /// ([`GameSky::register_pipelines`]), taken in at each prepare so a shader edit on disk shows at once.
    registry: Option<Registered>,
}

impl GameSky {
    /// A sky with clouds and weather whose draws fit `targets` (the renderer core's scene pass by default).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        quality: SkyQuality,
        targets: Targets,
    ) -> Self {
        Self::build(device, queue, quality, targets, None)
    }

    /// The game sky on the renderer's registry from the start: its pipelines are registered at once and compile on
    /// the registry's build threads beside every other layer's, and the first prepare takes them in (the parts hold
    /// the renderer's placeholders until then), so this compiles nothing on the calling thread. As [`GameSky::new`]
    /// followed by [`GameSky::register_pipelines`], without building every pipeline twice. Prepare once before
    /// [`GameSky::set_quality`].
    pub fn new_in(
        ctx: &mut RenderContext<'_>,
        quality: SkyQuality,
        targets: Targets,
    ) -> Result<Self, ShaderError> {
        let placeholders = ctx.placeholders().clone();
        let mut sky = Self::build(ctx.device, ctx.queue, quality, targets, Some(&placeholders));
        sky.register_pipelines(ctx)?;
        Ok(sky)
    }

    fn build(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        quality: SkyQuality,
        targets: Targets,
        placeholders: Option<&naval_render::PlaceholderPipelines>,
    ) -> Self {
        let sky_targets = SkyTargets {
            color: targets.color,
            depth: targets.depth,
        };
        let (mut sky, clouds, weather, post) = match placeholders {
            None => {
                let sky = Sky::new(device, queue, quality, sky_targets);
                let clouds = CloudLayer::new(device, queue, &sky, quality, targets);
                let weather = Weather::new(device, sky.bind_group_layout(), quality, targets);
                let post = PostProcess::new(device, sky.bind_group_layout(), targets.color);
                (sky, clouds, weather, post)
            }
            Some(placeholders) => {
                let sky = Sky::new_deferred(device, queue, quality, sky_targets, placeholders);
                let clouds =
                    CloudLayer::new_deferred(device, queue, &sky, quality, targets, placeholders);
                let weather = Weather::new_deferred(
                    device,
                    sky.bind_group_layout(),
                    quality,
                    targets,
                    placeholders,
                );
                let post = PostProcess::new_deferred(
                    device,
                    sky.bind_group_layout(),
                    targets.color,
                    placeholders,
                );
                (sky, clouds, weather, post)
            }
        };
        sky.attach_clouds(device, clouds.hooks());
        let aerial = AerialPerspective::new(
            device,
            &sky,
            clouds.shadow(),
            naval_render::MakePipelines::from_placeholders(placeholders),
        );
        let scene = *sky.scene();
        let mut game_sky = Self {
            sky,
            clouds,
            weather,
            post,
            aerial,
            sheltered: false,
            enabled: true,
            post_after_copies: false,
            time_scale: 1.0,
            asked_copies: Cell::new(false),
            quality,
            scene,
            blend_light: None,
            cut: true,
            last: None,
            clock: None,
            authoritative_clouds: None,
            starts_in_play: false,
            frame: None,
            still: false,
            view: None,
            registry: None,
        };
        game_sky.apply(queue, &scene);
        game_sky
    }

    /// Puts the sky's, the clouds', the weather's and the veil's pipelines on the renderer's registry, so they
    /// hot-reload with the renderer's `poll_shader_files`: registers both crates' WGSL with its library and builds the
    /// pipelines from it (on its build threads); [`SceneLayer::prepare`] then takes the registry's current ones each
    /// frame. A quality change ([`GameSky::set_quality`]) goes back to the crate's own pipelines until this is called
    /// again.
    pub fn register_pipelines(&mut self, ctx: &mut RenderContext<'_>) -> Result<(), ShaderError> {
        naval_sky::shaders::register(ctx)?;
        crate::shaders::register(ctx)?;
        self.registry = Some(Registered {
            sky: self.sky.register_pipelines(ctx)?,
            clouds: self.clouds.register_pipelines(ctx, &self.sky)?,
            weather: self.weather.register_pipelines(ctx)?,
            post: self.post.register_pipelines(ctx)?,
            aerial: self.aerial.register_pipelines(ctx)?,
        });
        Ok(())
    }

    /// Whether the pipelines come from the renderer's registry.
    pub fn uses_registry(&self) -> bool {
        self.registry.is_some()
    }

    pub fn scene(&self) -> &SkyScene {
        &self.scene
    }

    pub fn quality(&self) -> SkyQuality {
        self.quality
    }

    /// Apply a scene description at once (`Sky.apply`): the bodies, the air, the clouds and the weather. A sun that moves
    /// more than 2° or a cover that changes more than 0.05 cuts, as a new scene; [`GameSky::blend`] changes the weather
    /// without one.
    pub fn apply(&mut self, queue: &wgpu::Queue, scene: &SkyScene) {
        let before = self.scene;
        let sun_before = self.sky.sun_direction().as_dvec3();
        // The haze's boundary layer reaches up to the weather's cloud base; the sky takes the sun's light through it.
        self.set_haze_layer(scene);
        self.sky.apply(scene);
        if self
            .sky
            .sun_direction()
            .as_dvec3()
            .angle_between(sun_before)
            > CUT_SUN
            || (scene.clouds.coverage - before.clouds.coverage).abs() > CUT_COVER
        {
            self.cut = true;
        }
        self.clouds.apply(queue, &mut self.sky, scene);
        self.weather.apply(scene);
        self.scene = *scene;
        self.blend_light = None;
    }

    /// The weather `t` of the way from `from`'s to `to`'s (0 `from`, 1 `to`; a weather changing over minutes), drawn
    /// without a cut: the clouds blend the two weathers' layers on every call and keep their histories
    /// ([`CloudLayer::blend`]), the rain and lightning follow, and the sky's light (the sun's strength, the environment's
    /// bake) takes the blended scene in steps of [`BLEND_LIGHT_STEP`]. The air is `from`'s until halfway and `to`'s from
    /// there ([`SkyScene::lerp`]): its tables are computed once, not for every step. Call it every frame of the change,
    /// the last call at `t` = 1.
    pub fn blend(&mut self, from: &SkyScene, to: &SkyScene, t: f64) {
        self.blend_from(from, None, to, t);
    }

    /// [`GameSky::blend`] from clouds already drawn: `from_layers` (`self.clouds.layers()` when the change began) in
    /// place of `from`'s weather, so a change that interrupts another drifts on from the layers on screen.
    pub fn blend_from(
        &mut self,
        from: &SkyScene,
        from_layers: Option<&crate::layers::Layers>,
        to: &SkyScene,
        t: f64,
    ) {
        let t = t.clamp(0.0, 1.0);
        let scene = from.lerp(to, t);
        // The haze's layer follows the drifting cloud base.
        self.set_haze_layer(&scene);
        // The light in steps: a sun moving through the change (an hour changed with the weather) waits for the next.
        let light_due = match self.blend_light {
            None => true,
            Some(last) => {
                (t - last).abs() >= BLEND_LIGHT_STEP
                    || (t >= 1.0 && last < 1.0)
                    || (t <= 0.0 && last > 0.0)
                    || scene.atmosphere != self.sky.scene().atmosphere
            }
        };
        if light_due {
            self.sky.apply(&scene);
            self.blend_light = Some(t);
        }
        self.clouds.blend(from, from_layers, to, t as f32);
        self.weather.apply(&scene);
        self.scene = scene;
    }

    /// The weather's visibility (m), which sets the haze (the scene's fog end, which the simulation's lookouts
    /// see to; `naval_sky::haze`).
    pub fn set_visibility(&mut self, meters: f32) {
        let haze = naval_sky::haze::Haze {
            visibility: f64::from(meters),
            ..self.sky.weather_haze()
        };
        self.set_haze(haze);
    }

    /// The haze's boundary layer under the scene's cloud base (`apply`, and every step of a weather's drift).
    fn set_haze_layer(&mut self, scene: &SkyScene) {
        let haze =
            naval_sky::haze::Haze::new(self.sky.weather_haze().visibility, scene.clouds.altitude);
        self.set_haze(haze);
    }

    /// A new haze for the sky; a held or paused picture's converged clouds hold the old air in front of them
    /// (`naval_sky::aerial::aerial_along`), so they march again until they converge on the new.
    fn set_haze(&mut self, haze: naval_sky::haze::Haze) {
        if self.sky.set_weather_haze(haze) {
            self.clouds.air_changed();
        }
    }

    /// Graphics → Clouds, live: the sky's bake and stars, the clouds' preset, the rain's drops.
    pub fn set_quality(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, quality: SkyQuality) {
        self.quality = quality;
        // The rebuilt passes are the crate's own again (new layouts).
        self.registry = None;
        self.sky.set_quality(device, queue, quality);
        self.clouds.set_quality(device, quality);
        self.weather.set_quality(quality);
        let enabled = self.aerial.enabled;
        self.aerial = AerialPerspective::new(
            device,
            &self.sky,
            self.clouds.shadow(),
            naval_render::MakePipelines::Now,
        );
        self.aerial.enabled = enabled;
        let scene = self.scene;
        self.clouds.apply(queue, &mut self.sky, &scene);
        self.cut = true;
    }

    /// Development captures (`SkyApi.hold`): stand the sky at `time` seconds, its clouds drifted there from the start,
    /// and stop marching a still view once it has converged; `None` lets it run on.
    pub fn hold(&mut self, time: Option<f64>) {
        if let Some(time) = time {
            self.sky.seek(time);
            self.clock = None;
            self.starts_in_play = false;
        }
        self.clouds.hold(time.is_some());
        self.cut = true;
    }

    /// A sky put on screen in place of another one in play (the theatre's scene swaps): its first frame is a frame of
    /// play rather than a still picture, so its clouds fill in over the next updates, as after a cut in play, instead
    /// of marching every pixel in that one frame (a whole frame of GPU at 2880 × 1620, the first frames' longest). A
    /// [`GameSky::hold`] takes it back.
    pub fn start_in_play(&mut self) {
        self.starts_in_play = true;
    }

    /// A sky put on screen in place of `from`: its atmosphere tables copied from `from`'s when they hold the same air
    /// ([`Sky::take_tables_from`]), rather than computed again in its first frame. Submit `encoder` before this sky's
    /// next frame. Whether it copied them.
    pub fn take_tables_from(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        from: &GameSky,
    ) -> bool {
        self.sky.take_tables_from(queue, encoder, &from.sky)
    }

    /// Drop temporal history (a camera cut, or a scene change a capture must not smear).
    pub fn reset_history(&mut self) {
        self.cut = true;
    }

    /// Diagnostics: a seeded lightning flash `age` seconds into its first stroke (`WeatherSystem.forceStrike`).
    pub fn force_strike(
        &mut self,
        queue: &wgpu::Queue,
        position: Vec3,
        age: f64,
        kind: StrikeKind,
        seed: u32,
    ) {
        self.weather.force_strike(queue, position, age, kind, seed);
    }

    /// Thunder due since the last update, for the game's audio (`SkyApi.onThunder`).
    pub fn thunder(&self) -> &[Thunder] {
        &self.weather.thunder
    }

    /// Advance clouds, weather and bakes by `dt` (0 on a paused frame: nothing moves, the passes still run) for `view`,
    /// encoding the sky's and the clouds' passes that precede the scene pass. The clouds' march follows once the
    /// opaque scene is drawn: [`GameSky::march_clouds`] (which the scene layer runs at the opaque copy point).
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &CloudView,
        dt: f64,
    ) {
        self.update_timed(
            device,
            queue,
            &mut UploadSink::Queue(queue),
            encoder,
            view,
            dt,
            None,
        );
    }

    /// [`GameSky::update`], the sky's and the clouds' compute passes timed by the renderer core's timer (`sky tables`,
    /// `sky environment`, `cloud shadow maps`, `cloud shadow resolve`, `cloud ground shadow`; each only on frames it
    /// runs), the frame's uniforms written through `uploads` (the renderer core's staging belt in `prepare`; the sky's
    /// rare table precompute through `queue`).
    #[allow(clippy::too_many_arguments)]
    pub fn update_timed(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uploads: &mut UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        view: &CloudView,
        dt: f64,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        let forward = view
            .view
            .inverse()
            .transform_vector3(Vec3::NEG_Z)
            .normalize();
        let focal = view.projection.y_axis.y;
        let moved = match self.last {
            Some((position, last_forward, last_focal)) => {
                view.position.distance(position) > CUT_DISTANCE
                    || forward.angle_between(last_forward) > CUT_TURN
                    || (focal / last_focal - 1.0).abs() > 0.2
            }
            None => true,
        };
        let cut = self.cut || moved;
        self.cut = false;
        self.last = Some((view.position, forward, focal));
        // `Sky.update`: the weather first, whose strike lights the scene.
        self.weather.update(uploads, view, dt, cut, self.sheltered);
        let light = self.weather.light();
        self.sky
            .set_lightning(light.position, light.intensity, light.flash, light.bolt);
        let sky_view = SkyView {
            position: view.position,
            camera_to_world: view.view.inverse(),
            projection: view.projection,
            viewport: view.viewport,
        };
        self.sky.update_tables_timed(
            queue,
            uploads,
            encoder,
            &sky_view,
            dt * self.time_scale,
            timer,
        );
        if let Some(field) = self.authoritative_clouds {
            let drift = field.drift();
            self.sky
                .set_wind_offset(Vec3::new(drift[0] as f32, 0.0, drift[1] as f32));
            let mut layers = field.layers;
            // Rain shafts and sea-level fog remain the native weather's. Aircraft spotting shares the three cloud
            // layers; existing visibility already bounds every observer through rain and fog.
            layers.0[3] = self.clouds.layers().0[3];
            if *self.clouds.layers() != layers {
                self.clouds.set_layers(layers);
            }
        }
        self.clouds.prepare(device, uploads, view, &self.sky, cut);
        if let Some(group) = self.clouds.take_field_group() {
            self.sky.set_cloud_bind_group(group);
        }
        self.sky.update_environment_timed(uploads, encoder, timer);
        self.clouds.encode_shadows_timed(encoder, timer);
        self.post.update(view, self.weather.haze(), light);
        self.view = Some(*view);
    }

    /// The clouds' march and reconstruction over the opaque scene's reversed `depth` (with a key naming that texture,
    /// its registry version), or over no scene (`None`). After [`GameSky::update`], before [`GameSky::draw_sky`].
    pub fn march_clouds(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        depth: Option<(&wgpu::TextureView, u64)>,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        self.clouds
            .encode_march_timed(device, encoder, &self.sky, depth, timer);
    }

    /// The ocean's underwater probe, so the clouds draw nothing where a view ray starts under the waves
    /// ([`crate::CloudLayer::set_underwater_probe`]); `None` without an ocean.
    pub fn set_underwater_probe(&mut self, probe: Option<wgpu::TextureView>) {
        self.clouds.set_underwater_probe(probe);
    }

    /// The dome, then the clouds over it and over everything drawn (the `Sky` phase).
    pub fn draw_sky(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.sky.draw_dome(pass);
        self.clouds.draw_composite(pass);
    }

    /// Splashes, the bolt and the rain (the `Transparent` phase).
    pub fn draw_weather(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.weather.draw(pass, self.sky.bind_group());
    }

    /// Heavy rain's veil over the finished scene (`SkyApi.postProcess`), before the display grade: from `color` (a copy
    /// of the scene's colour) into `target` (the scene's colour). `depth` and `size` are no longer read (the sun shafts
    /// that did went with the old clouds). Returns false, touching nothing, when the veil does not show.
    #[allow(clippy::too_many_arguments)]
    pub fn post_process(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        _depth: &wgpu::TextureView,
        target: &wgpu::TextureView,
        _size: [u32; 2],
    ) -> bool {
        self.post
            .encode(device, queue, encoder, self.sky.bind_group(), color, target)
    }

    /// True when [`GameSky::post_process`] would draw.
    pub fn post_active(&self) -> bool {
        self.post.active()
    }

    /// The scene's celestial light: the sun or the moon.
    pub fn light(&self) -> CelestialLight {
        self.sky.light()
    }

    /// Lightning's light on the scene this frame (the web's `CelestialLight.flash` and `.bolt`): the flash lifts the
    /// hemisphere fill, the bolt is a shadowless point light.
    pub fn lightning(&self) -> naval_sky::LightningLight {
        self.sky.lightning()
    }
}

/// The renderer core's clock is absolute (frame N at N × dt): a frame drawn twice shows the same sky, and the sky
/// advances by the clock's steps.
impl SceneLayer for GameSky {
    fn label(&self) -> &str {
        "game sky"
    }

    fn prepare(&mut self, ctx: &mut PrepareContext<'_>) {
        let now = ctx.time.seconds;
        let dt = match self.clock {
            Some(last) => (now - last).max(0.0),
            // A sky started in play (`start_in_play`): its first frame steps by the frame's own time.
            None if std::mem::take(&mut self.starts_in_play) => Some(f64::from(ctx.time.dt))
                .filter(|&dt| dt > 0.0)
                .unwrap_or(1.0 / 60.0),
            None => 0.0,
        };
        self.clock = Some(now);
        // A new frame at the same time is a paused picture's: its clouds converge and then stand
        // (`CloudLayer::set_still`). A frame drawn again (a capture, keeping its index) is still as the frame it
        // repeats was, so a capture of a paused picture marches nothing either.
        let frame = ctx.time.frame;
        self.still = dt == 0.0
            && match self.frame {
                Some(last) if last == frame => self.still,
                Some(_) => true,
                None => false,
            };
        self.clouds.set_still(self.still);
        self.clouds.set_time_stands(dt == 0.0);
        self.frame = Some(frame);
        if !self.enabled {
            self.clouds.publish_cover(frame, false);
            return;
        }
        if let Some(registered) = &self.registry {
            // A rebuilt pass re-runs what it baked; a rebuilt cloud pass also drops the clouds' history.
            let sky_changed = self.sky.adopt_pipelines(ctx.pipelines, &registered.sky);
            if self
                .clouds
                .adopt_pipelines(ctx.pipelines, &registered.clouds)
                || sky_changed
            {
                self.cut = true;
            }
            self.weather
                .adopt_pipelines(ctx.pipelines, &registered.weather);
            self.post.adopt_pipelines(ctx.pipelines, &registered.post);
            // A rebuilt pass (a hot reload) changes the air the clouds' converged colours hold.
            if self
                .aerial
                .adopt_pipelines(ctx.pipelines, &registered.aerial)
            {
                self.cut = true;
            }
        }
        // The turbulence, when the registry's fill has not run yet (or was rebuilt).
        self.clouds.encode_noise_fill(ctx.encoder);
        let view = CloudView::from_camera(ctx.camera);
        // The compute passes in the frame's GPU timings, beside the renderer's own passes; the uniforms through the
        // frame's staging belt.
        let mut uploads = UploadSink::Belt(ctx.uploads);
        self.update_timed(
            ctx.device,
            ctx.queue,
            &mut uploads,
            ctx.encoder,
            &view,
            dt,
            Some(ctx.timer),
        );
        // The air between the eye and everything drawn this frame, once the sky and the clouds' shadow stand; the air
        // under the clouds that cast the shadow takes it.
        let heights = self.clouds.uniforms().heights;
        self.aerial.encode(
            ctx.device,
            &mut uploads,
            ctx.encoder,
            ctx.camera,
            ctx.aerial,
            &self.sky,
            self.clouds.shadow(),
            [heights[3], heights[2]],
            Some(ctx.timer),
        );
        self.clouds.publish_cover(frame, true);
    }

    fn draw(&self, phase: Phase, pass: &mut wgpu::RenderPass<'_>, _ctx: &DrawContext<'_>) {
        if !self.enabled {
            return;
        }
        match phase {
            Phase::Sky => self.draw_sky(pass),
            Phase::Transparent => self.draw_weather(pass),
            _ => {}
        }
    }

    /// Temporal AA: the rain's streaks take no history where they fall (`Weather::draw_motion`), over the motion of
    /// the hulls behind them too (the sky's layer draws before the ships').
    fn draw_motion_over(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &DrawContext<'_>) {
        if let (true, Some(registered)) = (self.enabled, &self.registry) {
            let pipeline = ctx.pipelines.render(registered.weather.rain_motion);
            self.weather
                .draw_motion(pass, pipeline, self.sky.bind_group());
        }
    }

    /// The opaque scene's depth for the clouds' march, and the backdrop's colour while the veil shows (as the last
    /// update left it: a frame late when the veil first appears).
    fn scene_copies(&self) -> SceneCopies {
        if !self.enabled {
            self.asked_copies.set(false);
            return SceneCopies::NONE;
        }
        let asked = self.post_after_copies && self.post_active();
        self.asked_copies.set(asked);
        let mut copies = SceneCopies::OPAQUE_DEPTH;
        if asked {
            copies |= SceneCopies::BACKDROP_COLOR;
        }
        copies
    }

    fn after_copies(&mut self, point: CopyPoint, ctx: &mut PrepareContext<'_>) {
        if !self.enabled {
            return;
        }
        match point {
            CopyPoint::Opaque => {
                let depth = ctx
                    .resources
                    .texture(SCENE_OPAQUE_DEPTH)
                    .map(|entry| (entry.view.clone(), entry.version));
                self.march_clouds(
                    ctx.device,
                    ctx.encoder,
                    depth.as_ref().map(|(view, version)| (view, *version)),
                    Some(ctx.timer),
                );
            }
            CopyPoint::Backdrop => {
                if !self.asked_copies.get() {
                    return;
                }
                let (Some(color), Some(target)) = (
                    ctx.resources.texture(SCENE_BACKDROP_COLOR),
                    ctx.resources.texture(SCENE_HDR),
                ) else {
                    return;
                };
                let (color, target) = (color.view.clone(), target.view.clone());
                self.post.encode(
                    ctx.device,
                    ctx.queue,
                    ctx.encoder,
                    self.sky.bind_group(),
                    &color,
                    &target,
                );
            }
        }
    }
}

/// A camera at `eye` looking at `target` (+Y up) with the game's reversed-Z projection.
pub fn view_looking_at(
    eye: Vec3,
    target: Vec3,
    fov_y_degrees: f32,
    near: f32,
    far: f32,
    viewport: [u32; 2],
) -> CloudView {
    let camera = naval_render::Camera {
        eye,
        target,
        fov_y_degrees,
        near,
        far,
        curvature: naval_render::camera::FLAT,
    };
    let aspect = viewport[0] as f32 / viewport[1].max(1) as f32;
    CloudView {
        position: eye,
        view: camera.view(),
        projection: camera.projection(aspect),
        near,
        far,
        viewport,
    }
}

/// A view from a world matrix and projection read back from three (column-major arrays).
pub fn view_from_three(
    matrix_world: &[f32; 16],
    projection: &[f32; 16],
    near: f32,
    far: f32,
    viewport: [u32; 2],
) -> CloudView {
    let world = Mat4::from_cols_array(matrix_world);
    CloudView {
        position: world.w_axis.truncate(),
        view: world.inverse(),
        projection: Mat4::from_cols_array(projection),
        near,
        far,
        viewport,
    }
}
