//! Every wake the surface shades, behind one facade: a port of the web game's `ShipWake.ts` with the realistic trail
//! (`OceanRealism.wake`, on in the game). It drives the wake field's bow and stern generators for the nearest hulls, the
//! fleet's trail foam and slick ([`crate::trail`]), the torpedo tracks ([`crate::torpedo_tracks`]), the analytic bow waves
//! ([`crate::bow_waves`]) and the hull contact foam ([`crate::hull_foam`]); paints the trail and torpedo atlases on the
//! GPU ([`crate::stamps`]); and packs every system's arrays into the one uniform buffer `shaders/wake.wgsl` reads.
//!
//! Per frame: [`ShipWake::update`] with the drawn hulls, this frame's water events and torpedoes and the camera (CPU),
//! then [`ShipWake::encode`] on the frame's encoder (the field's steps, atlas repaints, the uniform upload). The ocean
//! does the second from its `prepare`.

use crate::bow_waves::{BowShip, BowWaves};
use crate::hull_foam::{ContactShip, HULL_FOAM_SLOTS, HullContactFoam, STRIDE};
use crate::hull_profile::WaterlineProfile;
use crate::stamps::StampPainter;
use crate::torpedo_tracks::{TorpedoTracks, TrackTorpedo};
use crate::trail::{ATLAS_CAPACITY, FleetTrails, TILES, TrailShip, WakeHull};
use crate::wake::WakeField;
use crate::wake::WakeFieldPipelines;
use crate::wake::generators::GeneratorOptions;
use naval_render::{MakePipelines, PipelineId, PipelineRegistry, RenderContext, ShaderError};

/// Header vec4s of the `Wake` uniform (`shaders/wake.wgsl`), then its arrays.
const HEADER: usize = 15;
const TILE_ROWS: usize = 5 * ATLAS_CAPACITY;
const BOW_ROWS: usize = crate::bow_waves::BOW_SLOT_ROWS * crate::bow_waves::BOW_WAVE_SLOTS;
const HULL_ROWS: usize = HULL_FOAM_SLOTS * STRIDE;
/// The `Wake` uniform's size in vec4s.
pub const WAKE_VEC4S: usize = HEADER + TILE_ROWS + BOW_ROWS + HULL_ROWS;
/// The wake field spans this many metres around the focus hull, keeping a 250 m hull's whole trail.
pub const WORLD_SIZE: f64 = 1536.0;
/// Hulls within this distance (each axis) of the focus get the field's generators, the nearest eight of them.
const GENERATOR_REACH: f64 = 900.0;
const GENERATOR_HULLS: usize = 8;
/// A hull's own waves break in the field along her sides: within this many beams of the line from her bow and stern
/// generators to her middle (a Bismarck's, at full speed, all within 1.1 beams; a destroyer's not at all).
pub const BREAK_REACH: f64 = 1.25;

/// A drawn hull as the wakes read it.
#[derive(Clone, Copy, Debug)]
pub struct WakeShip<'a> {
    pub id: &'a str,
    /// World position of the hull's origin (y below 0: a diving submarine).
    pub position: [f64; 3],
    pub heading: f64,
    pub pitch: f64,
    pub roll: f64,
    /// Displayed speed along the bow (m/s).
    pub speed: f64,
    pub length: f64,
    pub beam: f64,
    pub draft: f64,
    pub center_x: f64,
    pub center_z: f64,
    pub forward_speed: f64,
    pub reverse_speed: f64,
    /// The hull's waterline profile, for its contact foam; none draws no line.
    pub profile: Option<&'a WaterlineProfile>,
}

/// A splash (or aircraft crash) that rings the trails with foam.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WakeSplash {
    pub sequence: u64,
    pub x: f64,
    pub z: f64,
    pub caliber_m: f64,
}

/// The camera the wakes read: where it is, which way it looks, its vertical field of view (radians), the projection's
/// vertical scale (`projectionMatrix[5]`, which grows with magnification) and the drawing buffer's height in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WakeCamera {
    pub eye: [f64; 3],
    pub forward: [f64; 3],
    pub fov_y: f64,
    pub zoom: f64,
    pub drawing_height: f64,
}

pub struct WakeInputs<'a> {
    /// The focus hull first.
    pub ships: &'a [WakeShip<'a>],
    pub dt: f64,
    pub splashes: &'a [WakeSplash],
    pub torpedoes: &'a [TrackTorpedo],
    pub camera: WakeCamera,
    /// Significant wave height of the sea on show (m).
    pub sea_height: f64,
}

/// What the surface binds of the wakes (group 1, bindings 5–10).
pub struct WakeBindings<'a> {
    pub uniforms: &'a wgpu::Buffer,
    pub displays: [&'a wgpu::TextureView; 2],
    pub trail_atlas: &'a wgpu::TextureView,
    pub torpedo_field: &'a wgpu::TextureView,
}

pub struct ShipWake {
    pub field: Option<WakeField>,
    pub trails: FleetTrails,
    pub torpedoes: TorpedoTracks,
    pub bow_waves: BowWaves,
    pub hull_foam: HullContactFoam,
    trail_painter: StampPainter,
    torpedo_painter: StampPainter,
    uniforms: wgpu::Buffer,
    calm_displays: [wgpu::TextureView; 2],
    generators: Vec<(String, u32, u32)>,
    event_sequence: u64,
    center: [f64; 2],
    dt: f64,
    /// Moves when the textures the surface binds are remade (a field reset), so it knows to rebind.
    pub version: u64,
}

/// The wakes' pipelines on the renderer's registry ([`ShipWake::register_pipelines`]).
#[derive(Clone, Copy, Debug)]
pub struct ShipWakePipelines {
    field: Option<WakeFieldPipelines>,
    trail: PipelineId,
    torpedo: PipelineId,
}

impl ShipWake {
    /// Wakes for a wake field of `resolution`² cells (0: no field, a calm wake, as the Low tier) whose finest water-mesh
    /// vertices are `mesh_spacing` metres apart.
    pub fn new(device: &wgpu::Device, resolution: u32, mesh_spacing: f64) -> Self {
        Self::build(device, resolution, mesh_spacing, MakePipelines::Now)
    }

    /// Builds the field's and both painters' pipelines through the renderer's registry, as [`ShipWake::new`] builds
    /// them. The ocean's files must be registered first (`crate::shaders::register`).
    pub fn register_pipelines(
        &self,
        ctx: &mut RenderContext<'_>,
    ) -> Result<ShipWakePipelines, ShaderError> {
        Ok(ShipWakePipelines {
            field: self
                .field
                .as_ref()
                .map(|field| field.register_pipelines(ctx))
                .transpose()?,
            trail: self.trail_painter.register_pipeline(ctx)?,
            torpedo: self.torpedo_painter.register_pipeline(ctx)?,
        })
    }

    /// Takes the registry's current pipelines in place of the wakes' own; whether any changed.
    pub fn adopt_pipelines(
        &mut self,
        registry: &PipelineRegistry,
        ids: &ShipWakePipelines,
    ) -> bool {
        let mut changed = false;
        if let (Some(field), Some(ids)) = (&mut self.field, &ids.field) {
            changed |= field.adopt_pipelines(registry, ids);
        }
        changed |= self.trail_painter.adopt_pipeline(registry, ids.trail);
        changed |= self.torpedo_painter.adopt_pipeline(registry, ids.torpedo);
        changed
    }

    pub(crate) fn build(
        device: &wgpu::Device,
        resolution: u32,
        mesh_spacing: f64,
        make: MakePipelines<'_>,
    ) -> Self {
        let field = (resolution > 0).then(|| {
            let mut field = WakeField::build(device, resolution, WORLD_SIZE, make);
            field.friction = 0.065;
            // Hull waves at full speed reach slopes of about 0.02 over the field's 12 m breaking baseline; this lets the
            // bow and stern shoulders break without foaming the whole wedge.
            field.foam_break_threshold = 0.015;
            field.foam_strength = 1.2;
            field.foam_lifetime = 9.0;
            field
        });
        // Trail foam is the game's own and stays on when the tier runs no wake field.
        let trail_resolution = if resolution > 0 {
            resolution.min(256)
        } else {
            128
        };
        let trails = FleetTrails::new(trail_resolution);
        let trail_painter = StampPainter::build(device, trail_resolution, TILES as u32, true, make);
        // A bubble track is a few metres wide: finer than any fleet wake cell.
        let torpedoes = TorpedoTracks::new(1024);
        let torpedo_painter = StampPainter::build(device, 1024, 1, false, make);
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ship wake"),
            size: (WAKE_VEC4S * 16) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let calm = |label| {
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
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let mut bow_waves = BowWaves::default();
        bow_waves.vertex_spacing = mesh_spacing;
        Self {
            field,
            trails,
            torpedoes,
            bow_waves,
            hull_foam: HullContactFoam::default(),
            trail_painter,
            torpedo_painter,
            uniforms,
            calm_displays: [calm("calm wake 0"), calm("calm wake 1")],
            generators: Vec::new(),
            event_sequence: 0,
            center: [0.0; 2],
            dt: 0.0,
            version: 0,
        }
    }

    /// What the surface binds.
    pub fn bindings(&self) -> WakeBindings<'_> {
        let displays = match &self.field {
            Some(field) => field.display_views(),
            None => [&self.calm_displays[0], &self.calm_displays[1]],
        };
        WakeBindings {
            uniforms: &self.uniforms,
            displays,
            trail_atlas: self.trail_painter.view(),
            torpedo_field: self.torpedo_painter.view(),
        }
    }

    /// The CPU half of a frame (`ShipWake.update`).
    pub fn update(&mut self, inputs: &WakeInputs<'_>) {
        let dt = inputs.dt.max(0.0);
        self.dt = dt;
        let fresh: Vec<&WakeSplash> = inputs
            .splashes
            .iter()
            .filter(|s| s.sequence > self.event_sequence)
            .collect();
        for splash in &fresh {
            self.event_sequence = self.event_sequence.max(splash.sequence);
        }
        if let Some(focus) = inputs.ships.first() {
            self.center = [focus.position[0], focus.position[2]];
            if let Some(field) = &mut self.field {
                field.set_center(focus.position[0], focus.position[2]);
            }
        }
        // Drawing favours the focus and nearest hulls; every hull retains its trail history independently of slots.
        let span = |ship: &WakeShip<'_>| {
            (ship.position[0] - self.center[0]).hypot(ship.position[2] - self.center[1])
        };
        let mut ships: Vec<WakeShip<'_>> = inputs.ships.to_vec();
        if ships.len() > ATLAS_CAPACITY {
            let mut rest = ships.split_off(1);
            rest.sort_by(|a, b| span(a).total_cmp(&span(b)));
            ships.extend(rest);
        }
        // The wake field takes 16 generators: its local swell goes to the nearest eight hulls.
        let mut nearby: Vec<&WakeShip<'_>> = ships
            .iter()
            .filter(|s| {
                (s.position[0] - self.center[0]).abs() < GENERATOR_REACH
                    && (s.position[2] - self.center[1]).abs() < GENERATOR_REACH
            })
            .collect();
        nearby.sort_by(|a, b| span(a).total_cmp(&span(b)));
        nearby.truncate(GENERATOR_HULLS);
        if let Some(field) = &mut self.field {
            let (keep, gone): (Vec<_>, Vec<_>) = self
                .generators
                .drain(..)
                .partition(|(id, ..)| nearby.iter().any(|s| s.id == id));
            for (_, bow, stern) in gone {
                field.generators.remove(bow);
                field.generators.remove(stern);
            }
            self.generators = keep;
            let mut strength: f64 = 0.0;
            for ship in &nearby {
                if !self.generators.iter().any(|(id, ..)| id == ship.id) {
                    let options = |depth: f64, radius: f64, offset_z: f64| GeneratorOptions {
                        active: false,
                        depth,
                        radius,
                        teleport_threshold: 100.0,
                        offset: [ship.center_x, offset_z],
                        breaks_to: [ship.center_x, ship.center_z],
                        break_reach: ship.beam * BREAK_REACH,
                    };
                    let bow = field.generators.add(options(
                        0.32,
                        ship.beam * 0.28,
                        ship.center_z - ship.length * 0.448,
                    ));
                    let stern = field.generators.add(options(
                        0.18,
                        ship.beam * 0.39,
                        ship.center_z + ship.length * 0.448,
                    ));
                    if let (Some(bow), Some(stern)) = (bow, stern) {
                        self.generators.push((ship.id.to_owned(), bow, stern));
                    }
                }
                let Some(&(_, bow, stern)) = self.generators.iter().find(|(id, ..)| id == ship.id)
                else {
                    continue;
                };
                let surface = (1.0 + ship.position[1] / 3.0).clamp(0.0, 1.0);
                let speed = ship.speed.abs() * surface;
                let ratio = (speed / ship.forward_speed.max(1e-3)).min(1.0);
                field.generators.update(bow, |g| {
                    g.active = speed > 0.1;
                    g.depth = 0.32 * ratio * ratio;
                });
                field.generators.update(stern, |g| {
                    g.active = speed > 0.1;
                    g.depth = 0.18 * ratio * ratio;
                });
                for id in [bow, stern] {
                    field
                        .generators
                        .set_pose(id, ship.position[0], ship.position[2], ship.heading);
                }
                strength = strength.max(ratio);
            }
            field.foam_strength = 1.2 * strength;
        }
        let camera = inputs.camera;
        let splashes: Vec<(f64, f64, f64)> =
            fresh.iter().map(|s| (s.x, s.z, s.caliber_m)).collect();
        let trail_ships: Vec<TrailShip<'_>> = ships
            .iter()
            .map(|s| TrailShip {
                id: s.id,
                position: s.position,
                heading: s.heading,
                speed: s.speed,
                hull: WakeHull {
                    length: s.length,
                    beam: s.beam,
                    forward_speed: s.forward_speed,
                    center_x: s.center_x,
                    center_z: s.center_z,
                },
                reverse_speed: s.reverse_speed,
            })
            .collect();
        self.trails
            .update(&trail_ships, dt, &splashes, Some((camera.eye, camera.zoom)));
        let torpedo_camera = Some((camera.eye, camera.forward, camera.zoom));
        self.torpedoes
            .update(inputs.torpedoes, dt, self.center, torpedo_camera);
        let bow_ships: Vec<BowShip<'_>> = ships
            .iter()
            .map(|s| BowShip {
                id: s.id,
                position: s.position,
                heading: s.heading,
                speed: s.speed,
                length: s.length,
                beam: s.beam,
                draft: s.draft,
                center_x: s.center_x,
                center_z: s.center_z,
                forward_speed: s.forward_speed,
                profile: s.profile,
            })
            .collect();
        self.bow_waves.update(
            &bow_ships,
            dt,
            Some((camera.eye, camera.fov_y)),
            camera.drawing_height,
        );
        self.hull_foam.sea_height = inputs.sea_height;
        let contact: Vec<ContactShip<'_>> = ships
            .iter()
            .filter_map(|s| {
                Some(ContactShip {
                    id: s.id,
                    position: s.position,
                    heading: s.heading,
                    pitch: s.pitch,
                    roll: s.roll,
                    speed: s.speed,
                    beam: s.beam,
                    draft: s.draft,
                    forward_speed: s.forward_speed,
                    profile: s.profile?,
                })
            })
            .collect();
        self.hull_foam.update(
            &contact,
            dt,
            Some((camera.eye, camera.fov_y)),
            camera.drawing_height,
        );
    }

    /// Ships that come aboard underway (a theatre scene, cruise or battle) lay the trails they have been leaving
    /// (`ShipWake.layTrails`): `seconds` of trail along each hull's present course and speed at `pace`, a step at a
    /// time (2.5 s, or 90 m of the fastest hull's run), from one pose a step before the history opens. Only the trails;
    /// the wake field's swell and the bow waves build as they would.
    pub fn lay_trails(
        &mut self,
        ships: &[WakeShip<'_>],
        seconds: f64,
        pace: f64,
        camera: &WakeCamera,
    ) {
        let fastest = ships
            .iter()
            .map(|s| s.speed.abs() * pace)
            .fold(0.0, f64::max);
        if seconds <= 0.0 || fastest <= 0.15 || !seconds.is_finite() {
            return;
        }
        let step = 2.5f64.min(90.0 / fastest);
        let steps = (seconds / step).ceil() as u64;
        for n in (0..=steps + 1).rev() {
            let ago = n as f64 * step;
            let past: Vec<TrailShip<'_>> = ships
                .iter()
                .map(|s| {
                    let run = s.speed * pace * ago;
                    TrailShip {
                        id: s.id,
                        position: [
                            s.position[0] - s.heading.sin() * run,
                            s.position[1],
                            s.position[2] + s.heading.cos() * run,
                        ],
                        heading: s.heading,
                        speed: s.speed,
                        hull: WakeHull {
                            length: s.length,
                            beam: s.beam,
                            forward_speed: s.forward_speed,
                            center_x: s.center_x,
                            center_z: s.center_z,
                        },
                        reverse_speed: s.reverse_speed,
                    }
                })
                .collect();
            self.trails
                .update(&past, step, &[], Some((camera.eye, camera.zoom)));
        }
    }

    /// The GPU half of a frame: the wake field's steps, atlas repaints and the uniform upload, on `encoder`.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        self.encode_timed(
            device,
            &mut naval_render::UploadSink::Queue(queue),
            encoder,
            None,
        );
    }

    /// [`ShipWake::encode`], the wake field's steps timed by the renderer core's timer, every write through `queue`
    /// (the renderer core's staging belt in a layer's `prepare`).
    pub fn encode_timed(
        &mut self,
        device: &wgpu::Device,
        queue: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        if let Some(field) = &mut self.field {
            field.update_timed(queue, encoder, self.dt, timer);
        }
        if let Some(collectors) = self.trails.take_repaint() {
            self.trail_painter
                .repaint(device, queue, encoder, &collectors);
        }
        if self.torpedoes.dirty {
            self.torpedoes.dirty = false;
            self.torpedo_painter
                .repaint(device, queue, encoder, &[&self.torpedoes.collector]);
        }
        queue.write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&self.pack()));
        self.dt = 0.0;
    }

    /// The `Wake` uniform (`shaders/wake.wgsl`).
    fn pack(&self) -> Vec<[f32; 4]> {
        let mut data = vec![[0.0_f32; 4]; WAKE_VEC4S];
        if let Some(field) = &self.field {
            let sampling = field.sampling();
            data[0] = [
                sampling.origins[0][0],
                sampling.origins[0][1],
                sampling.origins[1][0],
                sampling.origins[1][1],
            ];
            data[1] = [
                sampling.weights[0],
                sampling.weights[1],
                sampling.world_size,
                0.0,
            ];
        } else {
            data[1] = [0.0, 0.0, WORLD_SIZE as f32, 0.0];
        }
        data[1][3] = self.bow_waves.mesh_growth as f32;
        let torpedo = self.torpedoes.center();
        data[2] = [
            self.trails.rows.len() as f32,
            self.trails.time as f32,
            self.torpedoes.time as f32,
            if self.torpedoes.painted() { 1.0 } else { 0.0 },
        ];
        data[3] = self.trails.reach.map(|v| v as f32);
        let mesh = self.bow_waves.mesh_centre;
        data[4] = [
            torpedo[0] as f32,
            torpedo[1] as f32,
            mesh[0] as f32,
            mesh[1] as f32,
        ];
        let s = &self.trails.shading;
        data[5] = [s.foam_start, s.foam_full, s.foam_cover, s.tear];
        data[6] = [s.bubble_start, s.bubble_full, s.bubbles, s.bubble_tear];
        data[7] = [s.lobes, s.lobe_scale, s.eddy_scale, s.eddy_fine];
        data[8] = [
            s.slick_full,
            s.calm,
            s.residue,
            f32::from(u8::from(self.bow_waves.mesh_fan)),
        ];
        let bow = &self.bow_waves;
        data[9] = [
            bow.slots.len() as f32,
            bow.time as f32,
            bow.pixel_angle as f32,
            bow.vertex_spacing as f32,
        ];
        data[10..14].copy_from_slice(&bow.tuning.knobs());
        let hull = &self.hull_foam;
        data[14] = [
            hull.count as f32,
            hull.time as f32,
            hull.pixel_angle as f32,
            hull.tuning.bow_length as f32,
        ];
        for (i, row) in self.trails.rows.iter().enumerate() {
            data[HEADER + i] = row.center;
            data[HEADER + ATLAS_CAPACITY + i] = row.slick_center;
            data[HEADER + 2 * ATLAS_CAPACITY + i] = row.foam_box;
            data[HEADER + 3 * ATLAS_CAPACITY + i] = row.slick_box;
            data[HEADER + 4 * ATLAS_CAPACITY + i] = row.box_;
        }
        let slots = crate::bow_waves::BOW_WAVE_SLOTS;
        for (i, slot) in bow.slots.iter().enumerate() {
            for (section, value) in slot.iter().enumerate() {
                data[HEADER + TILE_ROWS + section * slots + i] = *value;
            }
        }
        let hull_start = HEADER + TILE_ROWS + BOW_ROWS;
        data[hull_start..hull_start + hull.count * STRIDE]
            .copy_from_slice(&hull.values[..hull.count * STRIDE]);
        data
    }

    /// Clear every trail and the field (returning to port).
    pub fn reset(&mut self, device: &wgpu::Device) {
        self.trails.reset();
        self.torpedoes.reset();
        self.bow_waves.reset();
        self.hull_foam.reset();
        self.event_sequence = 0;
        if let Some(field) = &mut self.field {
            field.reset(device);
            self.version += 1;
        }
    }

    /// Repaint the trail atlas from the current stamps whether or not they changed (for measurements).
    pub fn repaint_trails(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let collectors = self.trails.collectors();
        self.trail_painter.repaint(
            device,
            &mut naval_render::UploadSink::Queue(queue),
            encoder,
            &collectors,
        );
    }

    /// Stamps in the last repaint of the trail atlas and of the torpedo field, and wake field steps so far.
    pub fn diagnostics(&self) -> (u32, u32, u64) {
        (
            self.trail_painter.stamps(),
            self.torpedo_painter.stamps(),
            self.field.as_ref().map_or(0, |f| f.steps_run),
        )
    }

    /// The trail atlas texture, for inspection.
    pub fn trail_atlas(&self) -> &wgpu::Texture {
        self.trail_painter.texture()
    }
}
