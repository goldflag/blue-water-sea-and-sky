//! Bow waves on the displaced ocean, the CPU half of the web game's `BowWaves.ts`: a crest that climbs the hull's side
//! from the stem and peels away from it, the Kelvin V behind it and the white water on both, evaluated analytically in
//! each hull's frame by the surface shader (`naval_ocean::wake` in `shaders/wake.wgsl`), so crests stay sharp at any
//! range. Visual only.
//!
//! The crest's place (2026-10-12): the sea is undisturbed ahead of a fine stem. The water climbs the side from the
//! stem and stands highest a little aft of it ([`crest_peak`], Noblesse et al. 2008, "Simple analytical relations for
//! ship bow waves", J. Fluid Mech. 600), so nothing is raised or broken ahead of the bow.

use std::collections::VecDeque;

use crate::hull_profile::{PROFILE_LEVELS, PROFILE_STATIONS, WaterlineProfile};

const GRAVITY: f64 = 9.81;
const TAU: f64 = std::f64::consts::TAU;
/// Hulls drawn at once; the nearest to the camera keep their bow waves.
pub const BOW_WAVE_SLOTS: usize = 8;

/// Live-tunable shape of the bow wave (`BOW_WAVE_TUNING`). Visual choices, not a hydrodynamic model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BowWaveTuning {
    pub crest: f64,
    pub crest_angle: f64,
    /// The crest's half width where it leaves the stem, as a share of the beam (on top of 0.8 m): a thin sheet against
    /// the side, not a mound.
    pub crest_width: f64,
    /// How far aft of the stem the crest stands highest, in U²/g ÷ (1 + U/√(g·draft)) ([`crest_peak`]).
    pub crest_peak: f64,
    /// The water standing against the bow's sides about the crest, as a share of the crest's height.
    pub hump: f64,
    /// The Kelvin V's amplitude as a share of the stem crest ([`stem_crest`]), so it keeps the crest's proportion to the
    /// hull: from U²/2g alone, a destroyer at 36 kn raised a V of ±10 m about a 3.8 m draft.
    pub kelvin: f64,
    /// Share of the speed-derived wavelength used for the drawn V. Its shorter, feathered crests have their own
    /// spacing, independent of the stem crest's height and decay (which the bow spray also reads).
    pub kelvin_wavelength: f64,
    pub transverse: f64,
    pub divergent: f64,
    pub depth: f64,
    /// Upper bound on the analytic source depth (m). A fleet shown at one cruising speed can use its fine hulls'
    /// depth for the broad hulls too, retaining the same feathered crests. Infinite keeps the beam-derived depth.
    pub max_source_depth: f64,
    /// How far astern of the stem the V is drawn, in hull lengths, where her track has followed the arc it is laid on
    /// that far ([`BowWaves::update`]). From aloft a ship's Kelvin arms run for kilometres: the cusp's crests lose
    /// height only as the cube root of their range.
    pub reach: f64,
    /// The V's slopes inside its cusp lines, as a share of the lines' own (`shaders/wake.wgsl`, `CUSP_LINE`). The fringes
    /// within the lines are there, but from aloft the arms are two thin lines of short crests with quiet water between.
    pub kelvin_interior: f64,
    pub foam: f64,
    /// The crest's white water (the ribbon where its sheet plunges) fades astern over this many of the crest's decay
    /// lengths, and is gone by half the hull's length.
    pub foam_length: f64,
    /// How fast that ribbon widens as it runs aft (m per m).
    pub foam_spread: f64,
    /// The broken water pressed along the hull's sides behind the crest, and its width (m).
    pub wash: f64,
    pub wash_width: f64,
    pub kelvin_foam: f64,
    /// The drawn V's steepness (ak) at which its divergent crests start to break; they break fully at 3.3 times it.
    pub kelvin_steep: f64,
    /// How far the crests' white water lasts astern, in hull lengths.
    pub kelvin_foam_length: f64,
    /// The V's slopes as the surface shades them: this many × U²/2g × √(B/L) (the height it was drawn at before it took
    /// the crest's proportion). Long and low at its true height, the V would not read against the sea; only the drawn
    /// height, which the hull meets and whose steepness breaks its crests, keeps to [`BowWaveTuning::kelvin`].
    pub kelvin_shading: f64,
    pub breakup: f64,
    pub vertex_feature: f64,
    pub speed_scale: f64,
}

impl Default for BowWaveTuning {
    fn default() -> Self {
        Self {
            crest: 0.3,
            crest_angle: 0.18,
            crest_width: 0.05,
            crest_peak: 1.1,
            hump: 0.3,
            kelvin: 0.9,
            kelvin_wavelength: 0.2,
            transverse: 0.4,
            divergent: 1.0,
            depth: 0.12,
            max_source_depth: f64::INFINITY,
            reach: 12.0,
            kelvin_interior: 0.15,
            foam: 1.15,
            foam_length: 2.5,
            foam_spread: 0.025,
            wash: 0.7,
            wash_width: 3.5,
            kelvin_foam: 0.7,
            kelvin_steep: 0.06,
            kelvin_foam_length: 1.0,
            kelvin_shading: 0.7,
            breakup: 1.0,
            vertex_feature: 4.0,
            speed_scale: 1.0,
        }
    }
}

impl BowWaveTuning {
    /// The knobs the shader reads, in the order `shaders/wake.wgsl` unpacks them (four vec4s).
    pub fn knobs(&self) -> [[f32; 4]; 4] {
        let f = |v: f64| v as f32;
        [
            [
                f(self.crest_angle),
                f(self.crest_width),
                f(self.hump),
                f(self.transverse),
            ],
            [
                f(self.divergent),
                f(self.kelvin_interior),
                f(self.foam),
                f(self.foam_length),
            ],
            [
                f(self.foam_spread),
                f(self.wash),
                f(self.wash_width),
                f(self.kelvin_foam),
            ],
            [
                f(self.kelvin_steep),
                f(self.kelvin_foam_length),
                f(self.breakup),
                f(self.vertex_feature),
            ],
        ]
    }
}

/// The stem crest a hull of `beam` and `length` metres raises at displayed `speed` (m/s): its height in metres above the
/// sea and the length (m) over which it decays astern (`stemCrest`).
pub fn stem_crest(speed: f64, beam: f64, length: f64, tuning: &BowWaveTuning) -> (f64, f64) {
    let head = speed * speed / (2.0 * GRAVITY);
    let wavelength_speed = (speed * tuning.speed_scale).max(1e-3);
    let k0 = GRAVITY / (wavelength_speed * wavelength_speed);
    let height = (tuning.crest * head * (beam / 32.0).clamp(0.45, 1.1)).min(4.0);
    (height, (0.25 * TAU / k0).min(0.3 * length))
}

/// How far aft of the stem (m) the crest stands highest against the side of a hull drawing `draft` metres at displayed
/// `speed`: 1.1 U²/g ÷ (1 + F), F = U/√(g·draft) the draft Froude number (Noblesse et al. 2008), within 2 m and 0.08 of
/// her length. A battleship at 25 kn and a destroyer at 30 both stand theirs 7 to 9 m aft of the stem.
pub fn crest_peak(speed: f64, draft: f64, length: f64, tuning: &BowWaveTuning) -> f64 {
    let speed = speed.max(0.0);
    let froude = speed / (GRAVITY * draft.max(0.5)).sqrt();
    (tuning.crest_peak * speed * speed / GRAVITY / (1.0 + froude))
        .clamp(2.0, (0.08 * length).max(2.0))
}

/// The crest's height on its own line `aft` metres astern of the stem, as a share of the stem crest's height
/// ([`stem_crest`]): nothing a little ahead of the stem, the climb to its `peak`, the decay over `decay` metres after
/// it, and the water standing against the bow's sides (`hump`, the tuning's share) under it. The surface shader's
/// crest and heap on the crest line (`shaders/wake.wgsl`, `bow_waves`) where the water mesh resolves the crest (at range
/// the vertex stage draws it broader and lower); the bow spray rises from it.
pub fn crest_profile(
    aft: f64,
    peak: f64,
    decay: f64,
    length: f64,
    beam: f64,
    tuning: &BowWaveTuning,
) -> f64 {
    let climb = smooth((aft + CLIMB_LEAD * peak) / ((1.0 + CLIMB_LEAD) * peak));
    let along = aft.max(0.0);
    let fall = (-(along - peak).max(0.0) / decay.max(1e-3)).exp();
    let heap_along = (along - 2.0 * peak).max(0.0) / (HEAP_REACH * length);
    let heap_across = along * tuning.crest_angle / (HEAP_WIDTH * beam + 1.0 + HEAP_SPREAD * along);
    // What the trough's skirt leaves of the crest on its own line.
    let on_line = 1.0 - CREST_TROUGH * (-(CREST_TROUGH_INBOARD / CREST_INNER).powi(2)).exp();
    climb
        * (fall * on_line
            + tuning.hump * (-(heap_along * heap_along + heap_across * heap_across)).exp())
}

/// The climb starts this share of the peak's distance ahead of the stem, so the surface meets the stem without a step.
const CLIMB_LEAD: f64 = 0.15;
/// The crest's cross-section as the shader draws it: its inboard side this many times as broad as its outer face, and
/// behind it a trough this share of its height, centred this many of the face's widths inboard of the crest line.
const CREST_INNER: f64 = 2.2;
const CREST_TROUGH: f64 = 0.25;
const CREST_TROUGH_INBOARD: f64 = 3.2;
/// The water standing against the bow's sides: it lasts this share of her length past twice the peak's distance, and
/// reaches this share of her beam (and 1 m) out from her side at the stem, widening this much per metre aft.
const HEAP_REACH: f64 = 0.07;
const HEAP_WIDTH: f64 = 0.2;
const HEAP_SPREAD: f64 = 0.12;
/// The entrance the crest follows: this share of the hull's length aft of the stem, read at an eighth, a quarter, a
/// half and the whole of it ([`entrance`]).
const ENTRANCE: f64 = 0.3;
const ENTRANCE_NODES: [f64; 4] = [0.125, 0.25, 0.5, 1.0];

fn smooth(value: f64) -> f64 {
    let t = value.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Model z of a hull's stem at her design waterline: the foremost point of that slice, which a raked or flared bow's
/// overhang and a bulb both stand ahead of ([`WaterlineProfile::bow`] is the foremost station any slice reaches).
pub fn waterline_stem(profile: &WaterlineProfile) -> f64 {
    let stem = profile.stems[(PROFILE_LEVELS - 1) / 2];
    f64::from(if stem.is_finite() { stem } else { profile.bow })
}

/// The waterline's half-breadths (m) along the entrance of a hull `length` long and `beam` wide, at
/// [`ENTRANCE_NODES`] of [`ENTRANCE`] aft of her stem: her own where she has a profile (the design waterline's slice,
/// the wider side), else a parabolic entrance's. The crest climbs her side, so it needs her side: a battleship's
/// entrance is half as full near the stem as the parabola.
pub fn entrance(length: f64, beam: f64, profile: Option<&WaterlineProfile>) -> [f32; 4] {
    let parabola = |share: f64| beam * 0.5 * (1.0 - (1.0 - share) * (1.0 - share));
    let Some(profile) = profile else {
        return ENTRANCE_NODES.map(|share| parabola(share) as f32);
    };
    let level = (PROFILE_LEVELS - 1) / 2;
    let stem = waterline_stem(profile);
    let at = |level: usize, station: usize| {
        let index = level * PROFILE_STATIONS + station;
        let wider = profile.starboard[index].max(profile.port[index]);
        (wider.is_finite() && wider > 0.0).then_some(f64::from(wider))
    };
    // A station the waterline's slice misses takes the slices 2 m under and over it.
    let breadth = |station: usize| {
        at(level, station).or_else(|| match (at(level - 1, station), at(level + 1, station)) {
            (Some(under), Some(over)) => Some((under + over) * 0.5),
            (under, over) => under.or(over),
        })
    };
    // The slice as points aft of the stem, the stem itself first.
    let mut points: Vec<(f64, f64)> = vec![(0.0, 0.0)];
    for station in 0..PROFILE_STATIONS {
        let aft = f64::from(profile.start) + station as f64 * f64::from(profile.spacing) - stem;
        if let Some(breadth) = breadth(station).filter(|_| aft > 1e-3) {
            points.push((aft, breadth));
        }
    }
    if points.len() < 2 {
        return ENTRANCE_NODES.map(|share| parabola(share) as f32);
    }
    ENTRANCE_NODES.map(|share| {
        let aft = share * ENTRANCE * length;
        let after = points.iter().position(|(z, _)| *z >= aft);
        let value = match after {
            Some(0) => 0.0,
            Some(i) => {
                let ((z0, b0), (z1, b1)) = (points[i - 1], points[i]);
                b0 + (b1 - b0) * (aft - z0) / (z1 - z0).max(1e-6)
            }
            None => points[points.len() - 1].1,
        };
        value as f32
    })
}

/// A hull as the bow waves read it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BowShip<'a> {
    pub id: &'a str,
    pub position: [f64; 3],
    pub heading: f64,
    /// Displayed speed (m/s).
    pub speed: f64,
    pub length: f64,
    pub beam: f64,
    pub draft: f64,
    pub center_x: f64,
    pub center_z: f64,
    pub forward_speed: f64,
    /// Her waterline's slice, where she has one: her stem at the waterline (a raked or flared bow's stands aft of her
    /// length's end) and her entrance's breadths. None places the stem by her length and gives her a parabolic entrance.
    pub profile: Option<&'a WaterlineProfile>,
}

/// Vec4s in a slot.
pub const BOW_SLOT_ROWS: usize = 6;
/// One slot's six vec4s: pose (stem x/z, sin and cos of heading); wave (k₀, Kelvin amplitude, crest height, crest decay
/// length); hull (length, beam, source depth, speed share of full ahead); bend (track curvature, 1/m; the Kelvin
/// amplitude the surface shades, `kelvin_shading`; how far aft of the stem the crest stands highest, m; how far astern
/// the V is drawn, m); entrance (the waterline's half-breadths, [`entrance`]); course (sin and cos of the course over
/// the ground the V is laid along).
pub type BowSlot = [[f32; 4]; BOW_SLOT_ROWS];

/// Her track is kept a point every this many metres, as far back as the V's cusp lines were raised.
const HISTORY_STEP: f64 = 20.0;
/// The track's curvature is the circle's through where she is and the points this share of her length (and at least
/// [`FIT_CHORD_MIN`] m) and twice as far back along it: her heading swings with her yaw in a seaway, which bent the
/// far arms by tens of metres a second; the track she makes good does not.
const FIT_CHORD: f64 = 0.4;
const FIT_CHORD_MIN: f64 = 30.0;
/// A point of the cusp line `run` metres astern of the stem left her track this many times as far back: its waves run
/// out 35.26° off her heading at their group velocity, half her speed's cos 35.26°, so she gains 1 − 0.408 × 0.8165 =
/// 2/3 of her run on them.
const CUSP_EMITTED: f64 = 1.5;
/// How far her kept track may stray from the arc the V is laid on, as a share of how far back she ran it (and half her
/// beam), before the V stops short of it: past a turn the arms would swing with her helm.
const TRACK_STRAY: f64 = 0.04;
/// How fast the V's reach follows the track (s).
const REACH_EASE: f64 = 1.0;

struct Track {
    x: f64,
    z: f64,
    curvature: f64,
    /// Her track, newest last, a point every [`HISTORY_STEP`] metres.
    history: VecDeque<[f64; 2]>,
    /// How far astern of the stem the V is drawn (m), eased.
    reach: f64,
}

impl Track {
    fn new(x: f64, z: f64, reach: f64) -> Self {
        Self {
            x,
            z,
            curvature: 0.0,
            history: VecDeque::from([[x, z]]),
            reach,
        }
    }
}

/// The point `back` metres back along her kept `history` (newest last) from where she is, `at`.
fn back_along(history: &VecDeque<[f64; 2]>, at: [f64; 2], back: f64) -> Option<[f64; 2]> {
    let mut run = 0.0;
    let mut previous = at;
    for point in history.iter().rev() {
        let step = (point[0] - previous[0]).hypot(point[1] - previous[1]);
        if run + step >= back && step > 0.0 {
            let share = (back - run) / step;
            return Some([
                previous[0] + (point[0] - previous[0]) * share,
                previous[1] + (point[1] - previous[1]) * share,
            ]);
        }
        run += step;
        previous = *point;
    }
    None
}

/// Her track's curvature (1/m; positive where it bends to starboard astern of her, as `shaders/wake.wgsl` lays the V)
/// through where she is, `at`, and the points `chord` and twice `chord` back along her kept `history`: the circle's
/// through the three. None until she has run that far.
fn fitted_bend(history: &VecDeque<[f64; 2]>, at: [f64; 2], chord: f64) -> Option<f64> {
    let near = back_along(history, at, chord)?;
    let far = back_along(history, at, 2.0 * chord)?;
    let (ax, az) = (near[0] - at[0], near[1] - at[1]);
    let (bx, bz) = (far[0] - near[0], far[1] - near[1]);
    let sides = ax.hypot(az) * bx.hypot(bz) * (far[0] - at[0]).hypot(far[1] - at[1]);
    // Astern is (−sin h, cos h) and starboard (cos h, sin h): turning from the one toward the other is a negative cross.
    (sides > 1e-6).then(|| -2.0 * (ax * bz - az * bx) / sides)
}

/// Her course over the ground at `at` (rad, as a heading: forward is (sin, −cos)): the chord from the newest kept point
/// at least half a step back, turned by half the arc's bend (`bend`, 1/m) over it onto the tangent. None until she
/// has run that far. The V is laid along it, not along her heading, which yaws and drifts in a turn.
fn course(history: &VecDeque<[f64; 2]>, at: [f64; 2], bend: f64) -> Option<f64> {
    let gap = |p: &[f64; 2]| (at[0] - p[0]).hypot(at[1] - p[1]);
    let back = history
        .iter()
        .rev()
        .find(|p| gap(p) >= HISTORY_STEP * 0.5)?;
    let (dx, dz) = (at[0] - back[0], at[1] - back[1]);
    Some(dx.atan2(-dz) + bend * gap(back) * 0.5)
}

/// How far back along her kept `history` (m; newest last) her track follows the arc of curvature `bend` (1/m) the V
/// is laid on from where she is (`at`) along her `course`: the run back to the first point that strays from it by
/// more than [`TRACK_STRAY`] of that run and half her `beam`. Unbounded when all of it does: a track not kept is taken
/// to.
fn steady_run(
    history: &VecDeque<[f64; 2]>,
    at: [f64; 2],
    course: f64,
    bend: f64,
    beam: f64,
) -> f64 {
    let (sin, cos) = course.sin_cos();
    let mut run = 0.0;
    let mut previous = at;
    for point in history.iter().rev() {
        run += (point[0] - previous[0]).hypot(point[1] - previous[1]);
        previous = *point;
        let (dx, dz) = (point[0] - at[0], point[1] - at[1]);
        // `shaders/wake.wgsl`'s frame (a astern, y to starboard) and the arc's offset.
        let (a, y) = (dz * cos - dx * sin, dx * cos + dz * sin);
        let cross = 1.0 - bend * y;
        let off = (2.0 * y - bend * (y * y + a * a))
            / (1.0 + (bend * bend * a * a + cross * cross).sqrt());
        if off.abs() > TRACK_STRAY * run + beam * 0.5 {
            return run;
        }
    }
    f64::INFINITY
}

/// The bow waves' per-hull state and this frame's slots (`BowWaves`).
pub struct BowWaves {
    pub tuning: BowWaveTuning,
    pub enabled: bool,
    pub time: f64,
    /// Radians per drawing-buffer pixel, for the fragment's anti-aliasing.
    pub pixel_angle: f64,
    /// The cell of the finest water-mesh level drawn (m), doubling as that level morphs onto the next: the ocean sets
    /// it each frame with the centre ([`crate::geometry::Clipmap::detail`]). Under a magnified view's fan, whose cells
    /// start from nothing at the eye, its squeeze ([`crate::geometry::Fan::spacing`]).
    pub vertex_spacing: f64,
    /// How fast the mesh's cells grow with distance from its centre (m per m): a level's cell over half its edge,
    /// two over the cells a level edge holds, whichever level is the finest drawn; a fan's step
    /// ([`crate::geometry::Fan`]).
    pub mesh_growth: f64,
    /// The water mesh's centre (x, z), from which its rings coarsen: the eye from [`BowWaves::update`] until the ocean
    /// sets where it centres the clipmap ([`crate::geometry::focus`]).
    pub mesh_centre: [f64; 2],
    /// Whether the water mesh is a magnified view's fan ([`crate::geometry::fan`]) this frame.
    pub mesh_fan: bool,
    pub slots: Vec<BowSlot>,
    speeds: Vec<(String, f64)>,
    tracks: Vec<(String, Track)>,
}

impl Default for BowWaves {
    fn default() -> Self {
        Self {
            tuning: BowWaveTuning::default(),
            enabled: true,
            time: 0.0,
            pixel_angle: 0.001,
            vertex_spacing: 4.0,
            mesh_growth: 4.0 / 128.0,
            mesh_centre: [0.0; 2],
            mesh_fan: false,
            slots: Vec::new(),
            speeds: Vec::new(),
            tracks: Vec::new(),
        }
    }
}

impl BowWaves {
    /// `camera`: the eye and its vertical field of view (radians); `drawing_height` in pixels.
    pub fn update(
        &mut self,
        ships: &[BowShip<'_>],
        dt: f64,
        camera: Option<([f64; 3], f64)>,
        drawing_height: f64,
    ) {
        let tuning = self.tuning;
        if dt > 0.0 {
            self.time += dt;
        }
        if let Some((eye, fov)) = camera {
            self.pixel_angle = 2.0 * (fov / 2.0).tan() / drawing_height.max(1.0);
            self.mesh_centre = [eye[0], eye[2]];
        }
        self.speeds
            .retain(|(id, _)| ships.iter().any(|s| s.id == id));
        self.tracks
            .retain(|(id, _)| ships.iter().any(|s| s.id == id));
        let mut candidates: Vec<(&BowShip<'_>, f64, f64)> = Vec::new();
        for ship in ships {
            let surface = (1.0 + ship.position[1] / 3.0).clamp(0.0, 1.0);
            let target = ship.speed.max(0.0) * surface;
            let previous = self
                .speeds
                .iter()
                .find(|(id, _)| id == ship.id)
                .map_or(target, |(_, s)| *s);
            let speed = if dt > 0.0 {
                previous + (target - previous) * (1.0 - (-dt / 2.5).exp())
            } else {
                previous
            };
            match self.speeds.iter_mut().find(|(id, _)| id == ship.id) {
                Some(entry) => entry.1 = speed,
                None => self.speeds.push((ship.id.to_owned(), speed)),
            }
            let distance = camera.map_or(0.0, |(eye, _)| {
                (ship.position[0] - eye[0]).hypot(ship.position[2] - eye[2])
            });
            candidates.push((ship, speed, distance));
        }
        candidates.retain(|(_, speed, _)| *speed > 0.5);
        candidates.sort_by(|a, b| a.2.total_cmp(&b.2));
        candidates.truncate(BOW_WAVE_SLOTS);
        self.slots.clear();
        for (ship, speed, _) in candidates {
            let (length, beam) = (ship.length, ship.beam);
            let (sin, cos) = ship.heading.sin_cos();
            // The stem at the waterline, from her own slice: a raked bow's overhang raises no water. Without one, from
            // the hull's extents (the authoring origin need not be amidships).
            let ahead = ship
                .profile
                .map(|profile| -waterline_stem(profile))
                .filter(|ahead| ahead.is_finite())
                .unwrap_or(length * 0.49 - ship.center_z);
            let stem_x = ship.position[0] + cos * ship.center_x + sin * ahead;
            let stem_z = ship.position[2] + sin * ship.center_x - cos * ahead;
            let (bend, reach, course) = self.track(ship, dt, length, tuning.reach * length);
            let head = speed * speed / (2.0 * GRAVITY);
            let wavelength_speed = speed * tuning.speed_scale;
            let k0 = GRAVITY
                / (wavelength_speed * wavelength_speed * tuning.kelvin_wavelength.max(1e-3));
            let (crest, decay) = stem_crest(speed, beam, length, &tuning);
            let peak = crest_peak(speed, ship.draft, length, &tuning);
            let f = |v: f64| v as f32;
            self.slots.push([
                [f(stem_x), f(stem_z), f(sin), f(cos)],
                [f(k0), f(tuning.kelvin * crest), f(crest), f(decay)],
                [
                    f(length),
                    f(beam),
                    f((tuning.depth * beam).min(tuning.max_source_depth)),
                    f(smooth(speed / ship.forward_speed.max(1.0))),
                ],
                [
                    f(bend),
                    f(tuning.kelvin_shading * head * (beam / length).sqrt()),
                    f(peak),
                    f(reach),
                ],
                entrance(length, beam, ship.profile),
                [f(course.sin()), f(course.cos()), 0.0, 0.0],
            ]);
        }
        if !self.enabled {
            self.slots.clear();
        }
    }

    /// Tallest stem crest with the heap around it, in metres above the sea.
    pub fn crest_height(&self) -> f64 {
        self.slots
            .iter()
            .map(|slot| f64::from(slot[1][2]) * (1.0 + self.tuning.hump))
            .fold(0.0, f64::max)
    }

    pub fn reset(&mut self) {
        self.speeds.clear();
        self.tracks.clear();
        self.slots.clear();
    }

    /// Heading change per metre run, smoothed: a steady turn bends the wake onto its circle.
    /// Her track's curvature (1/m), how far astern of the stem (m) the V is drawn and the course (rad) it is laid along:
    /// as far as her kept track has followed the arc it is laid on (the cusp `run` astern was raised [`CUSP_EMITTED`]
    /// times as far back), never short of her length nor past `reach`; her course over the ground, her heading until
    /// she has a track.
    fn track(&mut self, ship: &BowShip<'_>, dt: f64, length: f64, reach: f64) -> (f64, f64, f64) {
        let (x, z) = (ship.position[0], ship.position[2]);
        let Some((_, track)) = self.tracks.iter_mut().find(|(id, _)| id == ship.id) else {
            self.tracks
                .push((ship.id.to_owned(), Track::new(x, z, reach)));
            return (0.0, reach, ship.heading);
        };
        if (x - track.x).hypot(z - track.z) > 100.0 {
            *track = Track::new(x, z, reach);
        }
        track.x = x;
        track.z = z;
        let newest = track.history.back().copied().unwrap_or([x, z]);
        if (x - newest[0]).hypot(z - newest[1]) >= HISTORY_STEP {
            track.history.push_back([x, z]);
        }
        let kept = (reach * CUSP_EMITTED / HISTORY_STEP).ceil() as usize + 2;
        while track.history.len() > kept {
            track.history.pop_front();
        }
        if dt > 0.0 {
            let limit = 2.0 / length.max(1.0);
            let chord = (FIT_CHORD * length).max(FIT_CHORD_MIN);
            let fitted = fitted_bend(&track.history, [x, z], chord).unwrap_or(0.0);
            track.curvature +=
                (fitted.clamp(-limit, limit) - track.curvature) * (1.0 - (-dt / 1.2).exp());
        }
        let Some(course) = course(&track.history, [x, z], track.curvature) else {
            return (track.curvature, track.reach, ship.heading);
        };
        let steady = steady_run(&track.history, [x, z], course, track.curvature, ship.beam);
        let target = (steady / CUSP_EMITTED).clamp(length, reach.max(length));
        if dt > 0.0 {
            track.reach += (target - track.reach) * (1.0 - (-dt / REACH_EASE).exp());
        }
        (track.curvature, track.reach, course)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fast_destroyer_raises_a_metre_crest() {
        let (height, decay) = stem_crest(18.0, 12.0, 115.0, &BowWaveTuning::default());
        // U²/2g = 16.5 m, × 0.3 × max(12/32, 0.45) = 2.2 m; a quarter of the 207 m Kelvin wavelength, capped at 0.3 L.
        assert!((height - 16.513_761 * 0.3 * 0.45).abs() < 1e-3, "{height}");
        assert!((decay - 0.3 * 115.0).abs() < 1e-9, "{decay}");
    }

    #[test]
    fn the_kelvin_v_keeps_the_crest_proportion() {
        // A Fletcher at 36 kn: the V a share of her stem crest, not U²/2g × √(B/L) (9 m about a 3.8 m draft).
        let mut bow = BowWaves::default();
        let fletcher = BowShip {
            id: "fletcher",
            position: [0.0; 3],
            heading: 0.0,
            speed: 18.5,
            length: 114.7,
            beam: 12.02,
            draft: 3.8,
            center_x: 0.0,
            center_z: 0.0,
            forward_speed: 18.5,
            profile: None,
        };
        for _ in 0..200 {
            bow.update(&[fletcher], 0.1, None, 900.0);
        }
        let wave = bow.slots[0][1];
        assert!((wave[1] - 0.9 * wave[2]).abs() < 1e-4, "{wave:?}");
        assert!(wave[1] < 2.5, "{wave:?}");
    }

    #[test]
    fn the_crest_stands_aft_of_the_stem_and_nothing_ahead_of_it() {
        let tuning = BowWaveTuning::default();
        // A battleship at 25 kn and a destroyer at 30: both 7 to 9 m aft of the stem.
        let battleship = crest_peak(12.7, 10.4, 263.0, &tuning);
        let destroyer = crest_peak(15.2, 3.8, 114.7, &tuning);
        assert!((7.0..9.0).contains(&battleship), "{battleship}");
        assert!((7.0..9.0).contains(&destroyer), "{destroyer}");
        // Slow, it stands at the stem; never past 0.08 of her length.
        assert_eq!(crest_peak(2.0, 3.8, 114.7, &tuning), 2.0);
        assert!(crest_peak(40.0, 3.8, 114.7, &tuning) <= 0.08 * 114.7 + 1e-9);
        let (_, decay) = stem_crest(12.7, 38.9, 263.0, &tuning);
        let profile = |aft: f64| crest_profile(aft, battleship, decay, 263.0, 38.9, &tuning);
        // Nothing stands ahead of the stem; the crest and the water against the side at the peak; then it decays.
        assert_eq!(profile(-0.15 * battleship - 0.01), 0.0);
        assert!(profile(0.0) < 0.07, "{}", profile(0.0));
        // (The trough behind the crest takes 3 % of it on its own line.)
        assert!(
            (profile(battleship) - (0.97 + tuning.hump)).abs() < 0.02,
            "{}",
            profile(battleship)
        );
        assert!(profile(3.0 * battleship) < profile(battleship));
        assert!(profile(120.0) < 0.2, "{}", profile(120.0));
    }

    #[test]
    fn the_stem_and_the_entrance_are_the_hulls_own() {
        // A proxy hull whose stem at the waterline stands 4 m aft of her length's end, with a fine entrance.
        let hull = crate::hull_profile::SimpleHull::warship(200.0, 30.0, 8.0);
        let mut profile = hull.profile();
        let level = (PROFILE_LEVELS - 1) / 2;
        let own = entrance(200.0, 30.0, Some(&profile));
        let parabola = entrance(200.0, 30.0, None);
        // The proxy's entrance is the parabola the bow waves assumed: the slice returns it within a station's chord.
        for (a, b) in own.iter().zip(parabola) {
            assert!((a - b).abs() < 0.9, "{own:?} {parabola:?}");
        }
        assert!(
            own[0] > 0.0 && own.windows(2).all(|w| w[1] >= w[0]),
            "{own:?}"
        );
        // Halve the slice at the waterline: the entrance follows her.
        for station in 0..PROFILE_STATIONS {
            profile.starboard[level * PROFILE_STATIONS + station] *= 0.5;
            profile.port[level * PROFILE_STATIONS + station] *= 0.5;
        }
        let fine = entrance(200.0, 30.0, Some(&profile));
        for (a, b) in fine.iter().zip(own) {
            assert!((a - b * 0.5).abs() < 1e-4, "{fine:?} {own:?}");
        }
        // The stem is the waterline's, not 0.49 of her length ahead of her middle.
        profile.stems[level] = -94.0;
        let ship = BowShip {
            id: "proxy",
            position: [0.0; 3],
            heading: 0.0,
            speed: 12.0,
            length: 200.0,
            beam: 30.0,
            draft: 8.0,
            center_x: 0.0,
            center_z: 0.0,
            forward_speed: 14.0,
            profile: Some(&profile),
        };
        let mut bow = BowWaves::default();
        bow.update(&[ship], 0.1, None, 900.0);
        assert!(
            (bow.slots[0][0][1] + 94.0).abs() < 1e-3,
            "{:?}",
            bow.slots[0][0]
        );
        assert_eq!(bow.slots[0][4], entrance(200.0, 30.0, Some(&profile)));
        let peak = crest_peak(12.0, 8.0, 200.0, &bow.tuning);
        assert!((f64::from(bow.slots[0][3][2]) - peak).abs() < 1e-3);
    }

    #[test]
    fn the_nearest_moving_hulls_take_the_slots() {
        let mut bow = BowWaves::default();
        let ship = |id: &'static str, x: f64, speed: f64| BowShip {
            id,
            position: [x, 0.0, 0.0],
            heading: 0.0,
            speed,
            length: 250.0,
            beam: 36.0,
            draft: 9.0,
            center_x: 0.0,
            center_z: 0.0,
            forward_speed: 15.0,
            profile: None,
        };
        let ships = [
            ship("far", 5000.0, 15.0),
            ship("near", 100.0, 15.0),
            ship("stopped", 0.0, 0.0),
        ];
        bow.update(&ships, 0.1, Some(([0.0, 30.0, 0.0], 0.9)), 900.0);
        assert_eq!(bow.slots.len(), 2);
        // The nearer first; its stem 0.49 L ahead (toward −Z at heading 0).
        assert!(
            (bow.slots[0][0][0] - 100.0).abs() < 1e-3 && (bow.slots[0][0][1] + 122.5).abs() < 1e-3
        );
    }

    /// The stem's kept track behind a stem at the origin: `run` metres of it, a point every [`HISTORY_STEP`] (newest
    /// last, the stem itself not kept), her heading over the ground `heading(s)` s metres back (forward is
    /// (sin h, −cos h), as `shaders/wake.wgsl` has it).
    fn kept_track(run: f64, heading: impl Fn(f64) -> f64) -> VecDeque<[f64; 2]> {
        let mut points = Vec::new();
        let (mut p, mut s, mut next) = ([0.0f64; 2], 0.0, HISTORY_STEP);
        while s < run {
            let h = heading(s + 0.25);
            p = [p[0] - h.sin() * 0.5, p[1] + h.cos() * 0.5];
            s += 0.5;
            if s >= next {
                points.push(p);
                next += HISTORY_STEP;
            }
        }
        points.into_iter().rev().collect()
    }

    #[test]
    fn the_v_reaches_as_far_as_her_track_follows_its_arc() {
        let bend = 1.0 / 600.0;
        // On a steady turn the whole kept track lies on the arc, whichever way she heads.
        let circle = kept_track(2000.0, |s| 0.7 - bend * s);
        // Within the kept points' chords (a 50 m chord's point lies up to 6 cm inside the circle).
        let fitted = fitted_bend(&circle, [0.0, 0.0], 50.0).unwrap();
        assert!((fitted - bend).abs() < bend * 0.06, "{fitted}");
        // Her course over the ground where she is, from the chord to her track a step back.
        let heading = course(&circle, [0.0, 0.0], bend).unwrap();
        assert!((heading - 0.7).abs() < 1e-3, "{heading}");
        assert_eq!(
            steady_run(&circle, [0.0, 0.0], heading, bend, 12.0),
            f64::INFINITY
        );
        let straight = kept_track(2000.0, |_| 2.0);
        let heading = course(&straight, [0.0, 0.0], 0.0).unwrap();
        assert_eq!(
            steady_run(&straight, [0.0, 0.0], heading, 0.0, 12.0),
            f64::INFINITY
        );
        // Laid 5° off her course, the arc leaves her track within a few hundred metres; so does the other bend.
        assert!(steady_run(&straight, [0.0, 0.0], heading + 0.087, 0.0, 12.0) < 300.0);
        assert!(steady_run(&circle, [0.0, 0.0], 0.7, -bend, 12.0) < 400.0);
        // Straight for 400 m after a 60° turn: the arms stop short of the turn.
        let turned = kept_track(2000.0, |s| -bend * (s - 400.0).clamp(0.0, 630.0));
        let steady = steady_run(&turned, [0.0, 0.0], 0.0, 0.0, 12.0);
        assert!((400.0..700.0).contains(&steady), "{steady}");
    }

    #[test]
    fn the_arms_stop_short_after_a_turn_and_grow_again() {
        let mut bow = BowWaves::default();
        let full = bow.tuning.reach * 115.0;
        let (mut x, mut z, mut heading) = (0.0f64, 0.0f64, 0.0f64);
        let mut run = |bow: &mut BowWaves, seconds: f64, rate: f64| {
            for _ in 0..(seconds * 10.0) as usize {
                heading += rate * 0.1;
                x += heading.sin() * 1.5;
                z -= heading.cos() * 1.5;
                let ship = BowShip {
                    id: "dd",
                    position: [x, 0.0, z],
                    heading,
                    speed: 15.0,
                    length: 115.0,
                    beam: 12.0,
                    draft: 4.0,
                    center_x: 0.0,
                    center_z: 0.0,
                    forward_speed: 18.0,
                    profile: None,
                };
                bow.update(&[ship], 0.1, Some(([x, 30.0, z], 0.9)), 900.0);
            }
            bow.slots[0][3][3] as f64
        };
        assert!((run(&mut bow, 200.0, 0.0) - full).abs() < full * 0.01);
        // 26° to port over 15 s, then 10 s steady: the arcs she ran before the turn no longer carry the V.
        run(&mut bow, 15.0, -15.0 / 500.0);
        assert!(run(&mut bow, 10.0, 0.0) < full * 0.3);
        assert!(run(&mut bow, 100.0, 0.0) > full * 0.6);
    }
}
