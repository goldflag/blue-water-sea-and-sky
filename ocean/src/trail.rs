//! Wake trails as world-space foam stamps: a port of the web game's `WakeFoam.ts` (its realistic path, `OceanRealism.wake`,
//! on in the game: continuous churned water and a slick; the three translucent streams it replaced are not ported) and
//! the CPU half of the web game's `FleetWakeFoam.ts` (one atlas tile per hull, each with its painted extents).
//!
//! Samples remember the heading at emission, so old water never turns with the hull. Each rasterisation lays max-coverage
//! ellipses into a [`StampCollector`]; [`crate::stamps::StampPainter`] draws the collectors into the atlas on the GPU.
//! Channel 0 of a tile holds the churned water's turbulence on the [`WAKE_EXTENT`] square around the hull, channel 1 the
//! slick on the wider [`SLICK_EXTENT`] square centred on the trail itself. Each band's stamps also write where they lie
//! across it (the atlas's blue and alpha, [`Stamp::across`]), so the surface can lay foam in streaks along the track
//! however it turns.

/// Edge (m) of each trail's churned-water square.
pub const WAKE_EXTENT: f64 = 1536.0;
/// Edge (m) of each trail's slick square: wide enough to hold minutes of a straight run.
pub const SLICK_EXTENT: f64 = 4096.0;
const LIFETIME: f64 = 90.0;
const IMPACT_LIFETIME: f64 = 30.0;
/// Age (s) from which the churned water fades out by [`LIFETIME`].
const CHURN_FADE: f64 = 60.0;
const SAMPLE_DISTANCE: f64 = 3.0;
/// Every this many trail samples also records a slick sample.
const SLICK_EVERY: u64 = 6;
/// Seconds a slick sample lasts.
pub const SLICK_LIFETIME: f64 = 240.0;
/// Narrowest footprint, in cells of its channel.
const MIN_CELLS: f64 = 1.5;
/// Furthest (m, each axis) the slick's square may centre from the hull.
const SLICK_REACH: f64 = 0.42 * SLICK_EXTENT;
/// A box around nothing (finite: shaders compare it).
pub const EMPTY: [f64; 4] = [1e9, 1e9, -1e9, -1e9];
/// Atlas tiles per edge, and the trails it holds.
pub const TILES: usize = 8;
pub const ATLAS_CAPACITY: usize = TILES * TILES;

fn smooth(value: f64) -> f64 {
    let t = value.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Stamps laid per trail sample (`WAKE_TUNING`): visual values measured against aerial photographs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WakeTuning {
    pub churn_width: f64,
    pub churn_spread: f64,
    pub churn_life: f64,
    pub churn_power: f64,
    pub meander: f64,
    pub lobes: f64,
    pub turn_churn: f64,
    pub slick_width: f64,
    pub slick_spread: f64,
    pub slick_life: f64,
    pub slick_power: f64,
}

impl Default for WakeTuning {
    fn default() -> Self {
        Self {
            churn_width: 0.45,
            churn_spread: 0.13,
            churn_life: 30.0,
            churn_power: 1.3,
            meander: 0.6,
            lobes: 0.35,
            turn_churn: 0.6,
            slick_width: 0.55,
            slick_spread: 0.2,
            slick_life: 80.0,
            slick_power: 0.7,
        }
    }
}

/// How the trail's channels read on the water (`WAKE_SHADING`); the surface shader takes them as knobs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WakeShading {
    pub foam_start: f32,
    pub foam_full: f32,
    pub foam_cover: f32,
    pub tear: f32,
    pub bubble_start: f32,
    pub bubble_full: f32,
    pub bubbles: f32,
    pub bubble_tear: f32,
    pub lobes: f32,
    pub lobe_scale: f32,
    pub eddy_scale: f32,
    pub eddy_fine: f32,
    pub slick_full: f32,
    pub calm: f32,
    pub residue: f32,
}

impl Default for WakeShading {
    fn default() -> Self {
        Self {
            foam_start: 0.05,
            foam_full: 0.75,
            foam_cover: 0.88,
            tear: 0.1,
            bubble_start: 0.04,
            bubble_full: 0.7,
            bubbles: 0.6,
            bubble_tear: 0.18,
            lobes: 2.5,
            lobe_scale: 0.35,
            eddy_scale: 0.11,
            eddy_fine: 0.45,
            slick_full: 0.15,
            calm: 0.75,
            residue: 0.2,
        }
    }
}

/// A hull's wake dimensions (`wakeHull` + its full speed ahead).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WakeHull {
    pub length: f64,
    pub beam: f64,
    pub forward_speed: f64,
    pub center_x: f64,
    pub center_z: f64,
}

/// A hull's pose and speed as the trail reads them: world x, z, heading (clockwise from above, 0 toward −Z) and the
/// displayed speed (m/s) along the bow.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Motion {
    pub x: f64,
    pub z: f64,
    pub heading: f64,
    pub speed: f64,
}

#[derive(Clone, Copy, Debug)]
struct WakeSample {
    speed: f64,
    born: f64,
    turn: f64,
    forward_x: f64,
    forward_z: f64,
    trail_x: f64,
    trail_z: f64,
    x: f64,
    z: f64,
    wander: f64,
    port: f64,
    starboard: f64,
    churn: f64,
    churn_power: f64,
    turn_churn: f64,
    slick: f64,
    slick_power: f64,
}

#[derive(Clone, Copy, Debug)]
struct Impact {
    x: f64,
    z: f64,
    born: f64,
    scale: f64,
    right_x: f64,
    right_z: f64,
    aspect: f64,
}

/// One stamp: a max-coverage ellipse (or ring) in a channel, world metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stamp {
    pub x: f64,
    pub z: f64,
    pub right_x: f64,
    pub right_z: f64,
    pub width: f64,
    pub length: f64,
    pub strength: f64,
    pub ring: bool,
    pub channel: u8,
    /// Where the stamp lies across the band it belongs to, in the band's half-widths: its centre's offset from the
    /// band's centreline and its own half-width. [`NO_BAND`] for stamps that belong to no band (splash rings, torpedo
    /// tracks).
    pub across: [f64; 2],
}

/// [`Stamp::across`] of a stamp outside any band.
pub const NO_BAND: [f64; 2] = [0.0, 0.0];

/// The last rasterisation of one tile (`WakeStampCollector`): its stamps and where each channel's square sits.
#[derive(Clone, Debug, Default)]
pub struct StampCollector {
    pub stamps: Vec<Stamp>,
    pub center: [f64; 2],
    /// Centre of the slick channel's wider square.
    pub slick_center: [f64; 2],
    /// Moves with every change to the stamps or their frame.
    pub version: u64,
}

impl StampCollector {
    pub fn begin(&mut self, center: [f64; 2], slick_center: [f64; 2]) {
        self.center = center;
        self.slick_center = slick_center;
        self.clear();
    }

    pub fn clear(&mut self) {
        self.stamps.clear();
        self.version += 1;
    }

    /// Where a channel lies in world space: its centre and the edge of the square a tile covers (`stampFrame`).
    pub fn frame(&self, channel: u8) -> ([f64; 2], f64) {
        if channel == 1 {
            (self.slick_center, SLICK_EXTENT)
        } else {
            (self.center, WAKE_EXTENT)
        }
    }
}

/// One hull's trail (`WakeFoam`, realistic).
pub struct WakeFoam {
    resolution: u32,
    hull: WakeHull,
    pub tuning: WakeTuning,
    pub collector: StampCollector,
    /// What the last rasterisation painted in each channel: (minX, minZ, maxX, maxZ), padded by a texel.
    pub painted: [[f64; 4]; 2],
    origin: [f64; 2],
    slick_origin: [f64; 2],
    time: f64,
    samples: std::collections::VecDeque<WakeSample>,
    slick_samples: std::collections::VecDeque<usize>,
    /// Every sample laid, by index (the slick keeps a sparser record for longer than the churn).
    history: Vec<WakeSample>,
    first: usize,
    impacts: std::collections::VecDeque<Impact>,
    previous: Option<Motion>,
    /// Where the hull stands while she lays her trail, and how fast she turns there (rad/s): the band's head, painted
    /// at every rasterisation so it keeps up with her stern between samples.
    head: Option<(Motion, f64)>,
    sample_distance: f64,
    sample_count: u64,
    elapsed: f64,
    dirty: bool,
    /// Moves on every rasterisation or reset (the web's `texture.version`).
    pub raster_version: u64,
    /// A rasterisation's impacts and churn stamps, kept as working room between frames.
    impact_room: Vec<Impact>,
    stamp_room: Vec<Stamp>,
}

impl WakeFoam {
    pub fn new(resolution: u32, hull: WakeHull) -> Self {
        Self {
            resolution,
            hull,
            tuning: WakeTuning::default(),
            collector: StampCollector::default(),
            painted: [EMPTY; 2],
            origin: [0.0; 2],
            slick_origin: [0.0; 2],
            time: 0.0,
            samples: Default::default(),
            slick_samples: Default::default(),
            history: Vec::new(),
            first: 0,
            impacts: Default::default(),
            previous: None,
            head: None,
            sample_distance: 0.0,
            sample_count: 0,
            elapsed: 0.0,
            dirty: false,
            raster_version: 0,
            impact_room: Vec::new(),
            stamp_room: Vec::new(),
        }
    }

    pub fn center(&self) -> [f64; 2] {
        self.origin
    }

    pub fn slick_center(&self) -> [f64; 2] {
        self.slick_origin
    }

    /// Advance by `dt` (0 does nothing), laying samples along the swept path and rasterising at most every
    /// `update_interval` seconds.
    pub fn update(&mut self, state: Motion, dt: f64, update_interval: f64) {
        if dt <= 0.0 {
            return;
        }
        self.time += dt;
        self.elapsed += dt;
        let previous = self.previous;
        let translation = previous.map_or(0.0, |p| (state.x - p.x).hypot(state.z - p.z));
        let heading_delta = previous.map_or(0.0, |p| {
            let d = state.heading - p.heading;
            d.sin().atan2(d.cos())
        });
        // Bound travel at both hull ends: a stern can sweep far through the water while the centre barely moves.
        let end_radius = self
            .hull
            .center_x
            .hypot(self.hull.center_z.abs() + self.hull.length * 0.468);
        let distance = translation + heading_delta.abs() * end_radius;
        let expected_travel =
            previous.map_or(state.speed.abs(), |p| p.speed.abs().max(state.speed.abs())) * dt * 3.0;
        if translation > 100.0 + expected_travel {
            self.reset();
        } else if let Some(previous) = previous
            && distance > 0.0001
            && state.speed.abs() > 0.15
        {
            let mut along = SAMPLE_DISTANCE - self.sample_distance;
            while along <= distance {
                let fraction = along / distance;
                let speed = previous.speed + (state.speed - previous.speed) * fraction;
                let sample = self.lay(
                    Motion {
                        x: previous.x + (state.x - previous.x) * fraction,
                        z: previous.z + (state.z - previous.z) * fraction,
                        heading: previous.heading + heading_delta * fraction,
                        speed,
                    },
                    self.time - dt * (1.0 - fraction),
                    heading_delta.abs() / dt,
                );
                let index = self.history.len();
                self.history.push(sample);
                self.samples.push_back(sample);
                if self.sample_count.is_multiple_of(SLICK_EVERY) {
                    self.slick_samples.push_back(index);
                }
                self.sample_count += 1;
                self.dirty = true;
                along += SAMPLE_DISTANCE;
            }
            self.sample_distance = (self.sample_distance + distance) % SAMPLE_DISTANCE;
        }
        self.head = (translation <= 100.0
            && previous.is_some()
            && state.speed.abs() > 0.15
            && !self.samples.is_empty())
        .then(|| (state, heading_delta.abs() / dt));
        self.previous = Some(state);
        while self
            .samples
            .front()
            .is_some_and(|s| self.time - s.born > LIFETIME)
        {
            self.samples.pop_front();
        }
        while self
            .slick_samples
            .front()
            .is_some_and(|&i| self.time - self.history[i].born > SLICK_LIFETIME)
        {
            self.slick_samples.pop_front();
        }
        while self
            .impacts
            .front()
            .is_some_and(|i| self.time - i.born > IMPACT_LIFETIME)
        {
            self.impacts.pop_front();
        }
        self.compact();
        let slick = !self.slick_samples.is_empty();
        if self.elapsed < update_interval
            || (self.samples.is_empty() && self.impacts.is_empty() && !slick && !self.dirty)
        {
            return;
        }
        self.elapsed = if self.elapsed.is_finite() {
            self.elapsed % update_interval
        } else {
            0.0
        };
        self.rasterize(state);
        self.dirty = !self.samples.is_empty() || !self.impacts.is_empty() || slick;
    }

    /// Drop history no slick sample refers to any more.
    fn compact(&mut self) {
        let oldest = self
            .slick_samples
            .front()
            .copied()
            .unwrap_or(self.history.len());
        if oldest > self.first + 4096 {
            let drop = oldest - self.first;
            self.history.drain(..drop.min(self.history.len()));
            for index in &mut self.slick_samples {
                *index -= drop;
            }
            self.first = 0;
        }
    }

    /// A new sample with everything that stays fixed for its life worked out.
    fn lay(&self, motion: Motion, born: f64, turn: f64) -> WakeSample {
        let hull = &self.hull;
        let forward_x = motion.heading.sin();
        let forward_z = -motion.heading.cos();
        let right_x = -forward_z;
        let right_z = forward_x;
        let center_x = motion.x + right_x * hull.center_x - forward_x * hull.center_z;
        let center_z = motion.z + right_z * hull.center_x - forward_z * hull.center_z;
        let aft = if motion.speed >= 0.0 { 1.0 } else { -1.0 } * hull.length * 0.468;
        let along = self.sample_count as f64 * SAMPLE_DISTANCE;
        let laps = along / hull.beam;
        let lobe =
            |phase: f64| (laps * 4.8 + phase).sin() * 0.6 + (laps * 2.1 + phase * 1.7).sin() * 0.4;
        WakeSample {
            speed: motion.speed,
            born,
            turn,
            forward_x,
            forward_z,
            trail_x: center_x - forward_x * aft,
            trail_z: center_z - forward_z * aft,
            x: motion.x,
            z: motion.z,
            wander: (along / 37.0).sin() * 0.6 + (along / 83.0 + 1.7).sin() * 0.4,
            port: lobe(2.3),
            starboard: lobe(0.0),
            churn: 0.0,
            churn_power: f64::NAN,
            turn_churn: f64::NAN,
            slick: 0.0,
            slick_power: f64::NAN,
        }
    }

    /// A shell splash's patch of foam and aerated water at world `x`, `z` for a shell of `caliber_m`.
    pub fn splash(&mut self, x: f64, z: f64, caliber_m: f64) {
        if self.impacts.len() >= 32 {
            self.impacts.pop_front();
        }
        // A hash of the impact point, not a random stream: every trail paints the same patch for one splash.
        let hash = ((x * 12.9898 + z * 78.233).sin() * 43758.5453).abs() % 1.0;
        let turn = hash * std::f64::consts::TAU;
        self.impacts.push_back(Impact {
            x,
            z,
            born: self.time,
            scale: (caliber_m.max(0.05) / 0.38).powf(0.65),
            right_x: turn.cos(),
            right_z: turn.sin(),
            aspect: 1.0 + (hash * 7.31 % 1.0) * 0.22,
        });
        self.dirty = true;
    }

    pub fn reset_impacts(&mut self) {
        self.impacts.clear();
        self.dirty = true;
    }

    fn translate(&mut self, offset: [f64; 2]) {
        for sample in &mut self.samples {
            sample.x += offset[0];
            sample.z += offset[1];
        }
        for sample in &mut self.history {
            sample.x += offset[0];
            sample.z += offset[1];
        }
        for impact in &mut self.impacts {
            impact.x += offset[0];
            impact.z += offset[1];
        }
        if let Some(previous) = &mut self.previous {
            previous.x += offset[0];
            previous.z += offset[1];
        }
        if let Some((head, _)) = &mut self.head {
            head.x += offset[0];
            head.z += offset[1];
        }
        if let Some((head, _)) = self.head {
            self.rasterize(head);
        }
    }

    /// Carries the trail into a scene laid on another chart (a theatre's re-lay), each value by its kind: its points
    /// (where a sample was laid, its trail point, a splash's patch, the hull's last and head poses) through their places,
    /// its directions (a sample's forward, a patch's right) through the two charts' Jacobians where they are held, and
    /// the hull's headings through the turn between the two grids there.
    fn relay(&mut self, relay: &dyn naval_render::relay::SceneRelay) {
        let sample = |s: &mut WakeSample| {
            let at = [s.x, 0.0, s.z];
            [s.forward_x, s.forward_z] = relay.direction(at, [s.forward_x, s.forward_z]);
            [s.trail_x, s.trail_z] = relay.point2([s.trail_x, s.trail_z]);
            [s.x, s.z] = relay.point2([s.x, s.z]);
        };
        self.samples.iter_mut().for_each(sample);
        self.history.iter_mut().for_each(sample);
        for impact in &mut self.impacts {
            let at = [impact.x, 0.0, impact.z];
            [impact.right_x, impact.right_z] =
                relay.direction(at, [impact.right_x, impact.right_z]);
            [impact.x, impact.z] = relay.point2([impact.x, impact.z]);
        }
        let motion = |m: &mut Motion| {
            let at = [m.x, 0.0, m.z];
            m.heading = relay.heading(at, m.heading);
            [m.x, m.z] = relay.point2([m.x, m.z]);
        };
        if let Some(previous) = &mut self.previous {
            motion(previous);
        }
        if let Some((head, _)) = &mut self.head {
            motion(head);
        }
        if let Some((head, _)) = self.head {
            self.rasterize(head);
        }
    }

    pub fn reset(&mut self) {
        self.samples.clear();
        self.slick_samples.clear();
        self.history.clear();
        self.first = 0;
        self.impacts.clear();
        self.previous = None;
        self.head = None;
        self.sample_distance = 0.0;
        self.sample_count = 0;
        self.collector.clear();
        self.painted = [EMPTY; 2];
        self.raster_version += 1;
        self.dirty = false;
    }

    fn rasterize(&mut self, state: Motion) {
        let cell = WAKE_EXTENT / f64::from(self.resolution);
        self.origin = [
            (state.x / cell).round() * cell,
            (state.z / cell).round() * cell,
        ];
        self.frame_slick(state);
        self.collector.begin(self.origin, self.slick_origin);
        self.painted = [EMPTY; 2];
        let now = self.time;
        let mut impacts = std::mem::take(&mut self.impact_room);
        impacts.clear();
        impacts.extend(self.impacts.iter().copied());
        for &impact in &impacts {
            let age = now - impact.born;
            let scale = impact.scale;
            // Where the column stood the sea boils white, then spreads into a wide patch of aerated water and foam that
            // thins as it goes (the surface reads it as lace and pale water, never a cut outline).
            let patch = (8.0 + 26.0 * (1.0 - (-age / 3.0).exp()) + 0.4 * age) * scale;
            let patching = smooth(age / 0.3)
                * (0.45 + 0.5 * (-age / 2.5).exp())
                * (1.0 - smooth((age - 6.0) / (IMPACT_LIFETIME - 6.0)));
            // The base surge pushes a brighter front of foam out ahead of it for the first few seconds.
            let front = (10.0 + 30.0 * (1.0 - (-age / 2.5).exp())) * scale;
            let fronting = smooth(age / 0.3) * 0.7 * (1.0 - smooth((age - 2.0) / 8.0));
            let (rx, rz, aspect) = (impact.right_x, impact.right_z, impact.aspect);
            self.stamp(
                impact.x,
                impact.z,
                rx,
                rz,
                patch * aspect,
                patch,
                patching,
                false,
                0,
                NO_BAND,
            );
            self.stamp(
                impact.x,
                impact.z,
                rx,
                rz,
                front * aspect,
                front,
                fronting,
                true,
                0,
                NO_BAND,
            );
        }
        self.impact_room = impacts;
        self.churn();
        self.raster_version += 1;
    }

    /// Centre the slick's square on the trail it holds, not on the hull, keeping the hull inside with a margin.
    fn frame_slick(&mut self, state: Motion) {
        let cell = SLICK_EXTENT / f64::from(self.resolution);
        let reach = SLICK_REACH;
        let (mut min_x, mut max_x, mut min_z, mut max_z) = (state.x, state.x, state.z, state.z);
        for &i in &self.slick_samples {
            let sample = &self.history[i];
            if (sample.x - state.x).abs() > reach * 2.0 || (sample.z - state.z).abs() > reach * 2.0
            {
                continue;
            }
            min_x = min_x.min(sample.x);
            max_x = max_x.max(sample.x);
            min_z = min_z.min(sample.z);
            max_z = max_z.max(sample.z);
        }
        let x = state.x + ((min_x + max_x) / 2.0 - state.x).clamp(-reach, reach);
        let z = state.z + ((min_z + max_z) / 2.0 - state.z).clamp(-reach, reach);
        self.slick_origin = [(x / cell).round() * cell, (z / cell).round() * cell];
    }

    /// The realistic trail: one churned band per sample in channel 0 and, sparser, the slick in channel 1.
    fn churn(&mut self) {
        let tuning = self.tuning;
        let beam = self.hull.beam;
        let now = self.time;
        let churn_cell = MIN_CELLS * WAKE_EXTENT / f64::from(self.resolution);
        let slick_cell = MIN_CELLS * SLICK_EXTENT / f64::from(self.resolution);
        let forward_speed = self.hull.forward_speed;
        let length = self.hull.length;
        let mut stamps = std::mem::take(&mut self.stamp_room);
        stamps.clear();
        // Samples are laid every few metres and the trail painted a few times a second: without a head where the hull
        // stands, the band's end fell behind her stern and jumped after her with each sample.
        let mut head = self.head.map(|(motion, turn)| self.lay(motion, now, turn));
        for sample in self.samples.iter_mut().chain(head.iter_mut()) {
            let age = now - sample.born;
            if sample.churn_power != tuning.churn_power || sample.turn_churn != tuning.turn_churn {
                let ratio = (sample.speed.abs() / forward_speed).min(1.0);
                let slide = (sample.turn * length / sample.speed.abs().max(1.0)).min(1.0);
                sample.churn = (smooth(ratio).powf(tuning.churn_power)
                    * (1.0 + tuning.turn_churn * slide))
                    .min(1.0);
                sample.churn_power = tuning.churn_power;
                sample.turn_churn = tuning.turn_churn;
            }
            let strength = sample.churn
                * (-age / tuning.churn_life).exp()
                * (1.0 - smooth((age - CHURN_FADE) / (LIFETIME - CHURN_FADE)));
            if strength < 0.015 {
                continue;
            }
            let right_x = -sample.forward_z;
            let right_z = sample.forward_x;
            let run = age * sample.speed.abs();
            let spread = (run / beam).sqrt();
            let width =
                (beam * (tuning.churn_width + tuning.churn_spread * spread)).max(churn_cell);
            // The band wanders a little along the track as its eddies grow.
            let meander = sample.wander * (tuning.meander * age.sqrt()).min(beam / 6.0);
            let center_x = sample.trail_x + right_x * meander;
            let center_z = sample.trail_z + right_z * meander;
            // Each side is its own stamp, bulging and drawing in on its own.
            for side in [-1.0, 1.0] {
                let lobe = if side > 0.0 {
                    sample.starboard
                } else {
                    sample.port
                };
                let half = (width * 0.65 * (1.0 + tuning.lobes * lobe)).max(churn_cell);
                stamps.push(Stamp {
                    x: center_x + right_x * side * width * 0.35,
                    z: center_z + right_z * side * width * 0.35,
                    right_x,
                    right_z,
                    width: half,
                    length: (width * 0.35).max(4.0),
                    strength,
                    ring: false,
                    channel: 0,
                    across: [side * 0.35, half / width],
                });
            }
        }
        let slick = |sample: &mut WakeSample| {
            let age = now - sample.born;
            if sample.slick_power != tuning.slick_power {
                sample.slick =
                    smooth((sample.speed.abs() / forward_speed).min(1.0)).powf(tuning.slick_power);
                sample.slick_power = tuning.slick_power;
            }
            let strength = sample.slick
                * (-age / tuning.slick_life).exp()
                * (1.0 - smooth((age - SLICK_LIFETIME * 0.75) / (SLICK_LIFETIME * 0.25)));
            if strength < 0.015 {
                return None;
            }
            let right_x = -sample.forward_z;
            let right_z = sample.forward_x;
            let run = age * sample.speed.abs();
            let spread = (run / beam).sqrt();
            let width =
                (beam * (tuning.slick_width + tuning.slick_spread * spread)).max(slick_cell);
            Some(Stamp {
                x: sample.trail_x,
                z: sample.trail_z,
                right_x,
                right_z,
                width,
                length: (SLICK_EVERY as f64 * SAMPLE_DISTANCE).max(width * 0.5),
                strength,
                ring: false,
                channel: 1,
                across: [0.0, 1.0],
            })
        };
        for &i in &self.slick_samples {
            stamps.extend(slick(&mut self.history[i]));
        }
        // The slick's samples lie further apart still: its head keeps up with the hull too.
        if let Some(head) = head.as_mut() {
            stamps.extend(slick(head));
        }
        for s in stamps.drain(..) {
            self.stamp(
                s.x, s.z, s.right_x, s.right_z, s.width, s.length, s.strength, s.ring, s.channel,
                s.across,
            );
        }
        self.stamp_room = stamps;
    }

    #[allow(clippy::too_many_arguments)]
    fn stamp(
        &mut self,
        x: f64,
        z: f64,
        right_x: f64,
        right_z: f64,
        width: f64,
        length: f64,
        strength: f64,
        ring: bool,
        channel: u8,
        across: [f64; 2],
    ) {
        if strength < 0.015 {
            return;
        }
        // The stamp's axis-aligned reach, plus a texel of its channel for filtering.
        let pad = if channel == 1 {
            SLICK_EXTENT
        } else {
            WAKE_EXTENT
        } / f64::from(self.resolution);
        let reach_x = right_x.abs() * width + right_z.abs() * length + pad;
        let reach_z = right_z.abs() * width + right_x.abs() * length + pad;
        let b = &mut self.painted[channel as usize];
        *b = [
            b[0].min(x - reach_x),
            b[1].min(z - reach_z),
            b[2].max(x + reach_x),
            b[3].max(z + reach_z),
        ];
        self.collector.stamps.push(Stamp {
            x,
            z,
            right_x,
            right_z,
            width,
            length,
            strength,
            ring,
            channel,
            across,
        });
        self.collector.version += 1;
    }
}

/// A drawn hull whose trail the fleet keeps: its identity between frames, pose, displayed speed and dimensions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrailShip<'a> {
    pub id: &'a str,
    /// World position (y is the hull's height: a diving submarine leaves less on the surface).
    pub position: [f64; 3],
    pub heading: f64,
    pub speed: f64,
    pub hull: WakeHull,
    pub reverse_speed: f64,
}

/// One tile's rows for the surface (`FleetWakeFoam`'s packed sections): trail centre and atlas tile, slick centre and the
/// hull's beam, the painted boxes of churned water, of slick and of both.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TileRows {
    pub center: [f32; 4],
    pub slick_center: [f32; 4],
    pub foam_box: [f32; 4],
    pub slick_box: [f32; 4],
    pub box_: [f32; 4],
}

/// Every hull's trail in one atlas (`FleetWakeFoam`, CPU half).
pub struct FleetTrails {
    resolution: u32,
    entries: Vec<(String, WakeFoam, i64, u64)>,
    /// Last frame's list of entries, kept as room for the next frame's.
    ordered_room: Vec<(String, WakeFoam, i64, u64)>,
    pub tuning: WakeTuning,
    pub shading: WakeShading,
    pub time: f64,
    pub rows: Vec<TileRows>,
    /// Every tile's painted extent together.
    pub reach: [f64; 4],
    dirty: bool,
}

impl FleetTrails {
    pub fn new(resolution: u32) -> Self {
        Self {
            resolution,
            entries: Vec::new(),
            ordered_room: Vec::new(),
            tuning: WakeTuning::default(),
            shading: WakeShading::default(),
            time: 0.0,
            rows: Vec::new(),
            reach: EMPTY,
            dirty: true,
        }
    }

    pub fn resolution(&self) -> u32 {
        self.resolution
    }

    /// Every tile's collector in slot order.
    pub fn collectors(&self) -> Vec<&StampCollector> {
        self.entries
            .iter()
            .take(ATLAS_CAPACITY)
            .map(|(_, foam, _, _)| &foam.collector)
            .collect()
    }

    /// The collectors in slot order, when any tile changed since the last call (then the atlas must be repainted).
    pub fn take_repaint(&mut self) -> Option<Vec<&StampCollector>> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        Some(
            self.entries
                .iter()
                .take(ATLAS_CAPACITY)
                .map(|(_, foam, _, _)| &foam.collector)
                .collect(),
        )
    }

    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// `ships` in slot order (the focus first); `splashes` are (x, z, caliber) of this frame's splashes and aircraft
    /// crashes; `camera` the eye and the projection's vertical scale (`projectionMatrix[5]`) for the refresh rate.
    pub fn update(
        &mut self,
        ships: &[TrailShip<'_>],
        dt: f64,
        splashes: &[(f64, f64, f64)],
        camera: Option<([f64; 3], f64)>,
    ) {
        let before = self.entries.len();
        self.entries
            .retain(|(id, ..)| ships.iter().any(|ship| ship.id == id));
        if self.entries.len() != before {
            self.dirty = true;
        }
        if dt > 0.0 {
            self.time += dt;
        }
        self.rows.clear();
        self.reach = EMPTY;
        // Atlas slots are a drawing budget. Other hulls still sample their course for a future view.
        // Keep entries in the ships' order, so a hull's slot is its index (into last frame's list, kept as room).
        let mut ordered = std::mem::take(&mut self.ordered_room);
        ordered.clear();
        for ship in ships {
            let entry = match self.entries.iter().position(|(id, ..)| id == ship.id) {
                Some(index) => self.entries.swap_remove(index),
                None => {
                    let mut foam = WakeFoam::new(self.resolution, ship.hull);
                    foam.tuning = self.tuning;
                    (ship.id.to_owned(), foam, -1, u64::MAX)
                }
            };
            ordered.push(entry);
        }
        std::mem::swap(&mut self.entries, &mut ordered);
        // The hulls no longer carried go with last frame's list; its room stays.
        ordered.clear();
        self.ordered_room = ordered;
        for (slot, ((_, foam, entry_slot, version), ship)) in
            self.entries.iter_mut().zip(ships).enumerate()
        {
            foam.tuning = self.tuning;
            for &(x, z, caliber) in splashes {
                foam.splash(x, z, caliber);
            }
            let surface = (1.0 + ship.position[1] / 3.0).clamp(0.0, 1.0);
            // Keep every emission sample, but refresh distant coverage at 5 Hz; magnification restores 20 Hz.
            let interval = match camera {
                Some((eye, zoom)) => {
                    let d = eye[0] - ship.position[0];
                    let e = eye[1] - ship.position[1];
                    let f = eye[2] - ship.position[2];
                    let apparent = (d * d + e * e + f * f).sqrt() * 2.05 / zoom;
                    if apparent > 2500.0 { 0.2 } else { 0.05 }
                }
                None => 0.05,
            };
            let motion = Motion {
                x: ship.position[0],
                z: ship.position[2],
                heading: ship.heading,
                speed: ship.speed * surface,
            };
            foam.update(motion, dt, interval);
            if slot >= ATLAS_CAPACITY {
                *entry_slot = -1;
                continue;
            }
            let (tx, ty) = ((slot % TILES) as f32, (slot / TILES) as f32);
            let center = foam.center();
            let slick_center = foam.slick_center();
            let [foam_box, slick_box] = foam.painted;
            let both = [
                foam_box[0].min(slick_box[0]),
                foam_box[1].min(slick_box[1]),
                foam_box[2].max(slick_box[2]),
                foam_box[3].max(slick_box[3]),
            ];
            self.reach = [
                self.reach[0].min(both[0]),
                self.reach[1].min(both[1]),
                self.reach[2].max(both[2]),
                self.reach[3].max(both[3]),
            ];
            let f = |b: [f64; 4]| b.map(|v| v as f32);
            self.rows.push(TileRows {
                center: [center[0] as f32, center[1] as f32, tx, ty],
                slick_center: [
                    slick_center[0] as f32,
                    slick_center[1] as f32,
                    ship.hull.beam as f32,
                    0.0,
                ],
                foam_box: f(foam_box),
                slick_box: f(slick_box),
                box_: f(both),
            });
            if *entry_slot == slot as i64 && *version == foam.raster_version {
                continue;
            }
            self.dirty = true;
            *entry_slot = slot as i64;
            *version = foam.raster_version;
        }
    }

    pub fn reset_impacts(&mut self) {
        for (_, foam, ..) in &mut self.entries {
            foam.reset_impacts();
        }
    }

    /// Takes `old`'s trails (a theatre scene's re-cut), carried by `shift` `[x, z]`: the old scene's origin less this
    /// one's (`naval_sim::earth::SceneShift::xz`), so each point `q` becomes `q + shift`.
    pub fn take_history_from(&mut self, old: &mut Self, shift: [f64; 2]) {
        self.entries = std::mem::take(&mut old.entries);
        self.time = old.time;
        for (_, foam, slot, version) in &mut self.entries {
            foam.resolution = self.resolution;
            foam.translate(shift);
            *slot = -1;
            *version = u64::MAX;
        }
        self.dirty = true;
    }

    /// Takes `old`'s trails into a scene laid on another chart (a theatre's re-lay), each value carried by its kind
    /// (`naval_render::relay::SceneRelay`), where [`FleetTrails::take_history_from`] translates them.
    pub fn relay_history_from(
        &mut self,
        old: &mut Self,
        relay: &dyn naval_render::relay::SceneRelay,
    ) {
        self.entries = std::mem::take(&mut old.entries);
        self.time = old.time;
        for (_, foam, slot, version) in &mut self.entries {
            foam.resolution = self.resolution;
            foam.relay(relay);
            *slot = -1;
            *version = u64::MAX;
        }
        self.dirty = true;
    }

    pub fn reset(&mut self) {
        self.entries.clear();
        self.rows.clear();
        self.time = 0.0;
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naval_render::relay::SceneRelay as _;

    fn bismarck() -> WakeHull {
        WakeHull {
            length: 250.0,
            beam: 36.0,
            forward_speed: 15.4,
            center_x: 0.0,
            center_z: 0.0,
        }
    }

    #[test]
    fn changing_atlas_slots_keeps_the_unwatched_ships_course_history() {
        let ids: Vec<_> = (0..ATLAS_CAPACITY + 1)
            .map(|i| format!("ship-{i}"))
            .collect();
        let mut trails = FleetTrails::new(64);
        for tick in 0..=600 {
            let ships: Vec<_> = ids
                .iter()
                .enumerate()
                .map(|(i, id)| TrailShip {
                    id,
                    position: [i as f64 * 1000.0, 0.0, -f64::from(tick) * 1.5],
                    heading: 0.0,
                    speed: 10.0,
                    hull: bismarck(),
                    reverse_speed: 6.0,
                })
                .collect();
            trails.update(&ships, 0.1, &[], None);
        }
        let mut ships: Vec<_> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| TrailShip {
                id,
                position: [i as f64 * 1000.0, 0.0, -901.5],
                heading: 0.0,
                speed: 10.0,
                hull: bismarck(),
                reverse_speed: 6.0,
            })
            .collect();
        ships.rotate_right(1);
        trails.update(&ships, 0.1, &[], None);
        let astern = trails.collectors()[0]
            .stamps
            .iter()
            .map(|s| s.z + 901.5)
            .fold(0.0, f64::max);
        assert!(
            astern > 500.0,
            "switching to a ship reset its long wake to {astern} m"
        );
        assert_eq!(trails.rows.len(), ATLAS_CAPACITY);
    }

    /// A re-lay as a turn by `t` about the old scene's zero and a shift: what a chart about another place does to a
    /// patch of sea, near enough for a test of who carries what.
    struct Turned {
        t: f64,
        shift: [f64; 2],
    }

    impl naval_render::relay::SceneRelay for Turned {
        fn point(&self, p: [f64; 3]) -> [f64; 3] {
            let [x, z] = self.direction(p, [p[0], p[2]]);
            [x + self.shift[0], p[1], z + self.shift[1]]
        }
        fn vector(&self, _: [f64; 3], v: [f64; 3]) -> [f64; 3] {
            let (sin, cos) = self.t.sin_cos();
            [v[0] * cos - v[2] * sin, v[1], v[2] * cos + v[0] * sin]
        }
        fn heading(&self, _: [f64; 3], heading: f64) -> f64 {
            heading + self.t
        }
    }

    /// A re-lay carries a trail by kind: where each sample lies and its trail point as points, its forward as a vector,
    /// the hull's last heading as a heading; the band then paints where the turned sea has it.
    #[test]
    fn a_re_lay_carries_a_trail_by_kind() {
        let mut foam = WakeFoam::new(256, bismarck());
        for i in 0..=300 {
            let t = f64::from(i) / 10.0;
            foam.update(
                Motion {
                    x: 0.0,
                    z: -t * 15.0,
                    heading: 0.0,
                    speed: 15.0,
                },
                0.1,
                0.05,
            );
        }
        let before = foam.samples.clone();
        let relay = Turned {
            t: 0.3,
            shift: [1_000.0, -2_000.0],
        };
        foam.relay(&relay);
        for (was, now) in before.iter().zip(&foam.samples) {
            let p = relay.point([was.x, 0.0, was.z]);
            assert!((now.x - p[0]).abs() < 1e-9 && (now.z - p[2]).abs() < 1e-9);
            let f = relay.direction([was.x, 0.0, was.z], [was.forward_x, was.forward_z]);
            assert!((now.forward_x - f[0]).abs() < 1e-12 && (now.forward_z - f[1]).abs() < 1e-12);
            let q = relay.point([was.trail_x, 0.0, was.trail_z]);
            assert!((now.trail_x - q[0]).abs() < 1e-9 && (now.trail_z - q[2]).abs() < 1e-9);
        }
        let previous = foam.previous.unwrap();
        assert!((previous.heading - 0.3).abs() < 1e-12);
        // The hull went on from where the turned sea has her: her heading there, not a turn of 0.3 rad in a tick.
        let at = relay.point([0.0, 0.0, -450.0]);
        assert!((previous.x - at[0]).abs() < 1e-6 && (previous.z - at[2]).abs() < 1e-6);
    }

    #[test]
    fn a_straight_run_lays_a_churned_band_and_a_slick() {
        let mut foam = WakeFoam::new(256, bismarck());
        let speed = 15.0;
        // Two minutes: longer than the churned water lasts, so the slick reaches farther astern.
        for i in 0..=1200 {
            let t = f64::from(i) / 10.0;
            foam.update(
                Motion {
                    x: 0.0,
                    z: -t * speed * 1.5,
                    heading: 0.0,
                    speed,
                },
                0.1,
                0.05,
            );
        }
        let churned = foam
            .collector
            .stamps
            .iter()
            .filter(|s| s.channel == 0)
            .count();
        let slick = foam
            .collector
            .stamps
            .iter()
            .filter(|s| s.channel == 1)
            .count();
        assert!(
            churned > 100 && slick > 20,
            "{churned} churned, {slick} slick stamps"
        );
        // The band lies astern of the hull (toward +z at heading 0) and the slick outlasts it farther astern.
        let hull_z = -120.0 * speed * 1.5;
        let churn_far = foam
            .collector
            .stamps
            .iter()
            .filter(|s| s.channel == 0)
            .map(|s| s.z)
            .fold(f64::MIN, f64::max);
        let slick_far = foam
            .collector
            .stamps
            .iter()
            .filter(|s| s.channel == 1)
            .map(|s| s.z)
            .fold(f64::MIN, f64::max);
        assert!(
            churn_far > hull_z + 500.0 && slick_far > churn_far,
            "{hull_z} {churn_far} {slick_far}"
        );
        assert!(foam.painted[0][3] >= churn_far && foam.painted[1][3] >= slick_far);
        // Every band stamp says where it lies across its band, for the streaks the surface draws along the track.
        assert!(foam.collector.stamps.iter().all(|s| s.across[1] > 0.0));
    }

    /// The churned water's coverage at a world point, as `stamps.wgsl` paints it (before the atlas's texels).
    fn churned_at(collector: &StampCollector, x: f64, z: f64) -> f64 {
        collector
            .stamps
            .iter()
            .filter(|s| s.channel == 0 && !s.ring)
            .map(|s| {
                let (dx, dz) = (x - s.x, z - s.z);
                let across = (dx * s.right_x + dz * s.right_z) / s.width;
                let along = (-dx * s.right_z + dz * s.right_x) / s.length;
                let radius = across * across + along * along;
                if radius < 1.0 {
                    (1.0 - smooth(radius)) * s.strength
                } else {
                    0.0
                }
            })
            .fold(0.0, f64::max)
    }

    #[test]
    fn the_band_keeps_up_with_the_stern() {
        // A destroyer at full speed, drawn at 120 Hz, her trail painted at 20 Hz: 25 m/s over the water (the battle's
        // pace) lays a sample every eighth of a second.
        let hull = WakeHull {
            length: 115.0,
            beam: 12.0,
            forward_speed: 18.0,
            center_x: 0.0,
            center_z: 0.0,
        };
        let mut foam = WakeFoam::new(256, hull);
        let (speed, pace, dt) = (17.0, 1.5, 1.0 / 120.0);
        let mut weakest = f64::MAX;
        let mut strongest: f64 = 0.0;
        for frame in 0..=2400 {
            let z = -f64::from(frame) * dt * speed * pace;
            foam.update(
                Motion {
                    x: 0.0,
                    z,
                    heading: 0.0,
                    speed,
                },
                dt,
                0.05,
            );
            if frame >= 1200 {
                // Where the hull's stern stands this frame (heading 0: astern is +z).
                let stern = churned_at(&foam.collector, 0.0, z + hull.length * 0.468);
                weakest = weakest.min(stern);
                strongest = strongest.max(stern);
            }
        }
        assert!(
            weakest > 0.75 * strongest,
            "the churned water at the stern swings from {weakest:.3} to {strongest:.3}"
        );
    }

    #[test]
    fn a_splash_rings_every_trail_alike() {
        let mut a = WakeFoam::new(256, bismarck());
        let mut b = WakeFoam::new(256, bismarck());
        for foam in [&mut a, &mut b] {
            foam.update(Motion::default(), 0.1, 0.05);
            foam.splash(40.0, -20.0, 0.38);
            foam.update(Motion::default(), 1.0, 0.05);
        }
        assert_eq!(a.collector.stamps, b.collector.stamps);
        assert_eq!(a.collector.stamps.iter().filter(|s| s.ring).count(), 1);
        assert!(a.collector.stamps.iter().all(|s| s.across == NO_BAND));
    }
}
