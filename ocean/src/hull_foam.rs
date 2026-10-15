//! The foam line where each hull meets the water, the CPU half of the web game's `HullContactFoam.ts`: per slot, a bounding
//! circle, the world-to-hull transform of the drawn pose, the hull's waterline profile ([`crate::hull_profile`]) and
//! what drives the line (rest, sea, speed, the bow). The surface shader reads it (`hull_foam` in `shaders/wake.wgsl`) and
//! finds the hull's breadth where the water actually stands against it, so the line hugs the drawn hull at any heave,
//! pitch and roll. The same slots carry each side's height for the shelter the sides give the water beside them (`hull_shelter`).

use glam::{DMat4, DQuat, DVec3, EulerRot};

use crate::hull_profile::{PROFILE_LEVELS, PROFILE_STATIONS, WaterlineProfile};

/// Hulls with a foam line at once; the nearest to the camera keep theirs.
pub const HULL_FOAM_SLOTS: usize = 8;
/// Beyond this range the line is narrower than a pixel and fades out anyway.
const REACH: f64 = 4000.0;
/// Empty stations read as this share of the half beam inside the centreline.
const EMPTY: f32 = 0.25;
/// Per slot: a bounding circle, three rows of the world-to-hull matrix, the shape, the drive, the shelter, one vec4 per
/// level and station (that station's and the next one's starboard and port half-breadths), one per station (its and the
/// next one's side heights).
pub const PROFILE: usize = 7;
pub const SIDES: usize = PROFILE + PROFILE_LEVELS * PROFILE_STATIONS;
pub const STRIDE: usize = SIDES + PROFILE_STATIONS;
/// Distances from a hull's side, in heights of the side above the water, over which its shelter fades out.
const SHELTER_REACH: f64 = 5.0;
/// Share of `REACH` beyond which the shelter fades out with the camera's range.
const SHELTER_RANGE_FADE: f64 = 0.75;
/// Heels (radians, from upright) over which the shelter fades out: past them a side is no longer a wall standing over
/// the water but lies along it, and a capsized hull's sides are under it.
const SHELTER_HEEL: [f64; 2] = [
    35.0 * std::f64::consts::PI / 180.0,
    55.0 * std::f64::consts::PI / 180.0,
];

/// Live-tunable look of the foam that hugs a hull (`HULL_FOAM_TUNING`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HullFoamTuning {
    pub rest: f64,
    pub sea: f64,
    pub underway: f64,
    pub bow: f64,
    pub width: f64,
    pub sea_width: f64,
    pub speed_width: f64,
    pub bow_length: f64,
}

impl Default for HullFoamTuning {
    fn default() -> Self {
        Self {
            rest: 0.12,
            sea: 0.025,
            underway: 0.3,
            bow: 0.4,
            width: 0.9,
            sea_width: 0.1,
            speed_width: 1.2,
            bow_length: 0.3,
        }
    }
}

fn smoothstep(low: f64, high: f64, x: f64) -> f64 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A hull at the water: its drawn pose, displayed speed, dimensions and profile.
#[derive(Clone, Copy, Debug)]
pub struct ContactShip<'a> {
    pub id: &'a str,
    pub position: [f64; 3],
    pub heading: f64,
    pub pitch: f64,
    pub roll: f64,
    pub speed: f64,
    pub beam: f64,
    pub draft: f64,
    pub forward_speed: f64,
    pub profile: &'a WaterlineProfile,
}

/// The contact foam's slots (`HullContactFoam`).
pub struct HullContactFoam {
    pub tuning: HullFoamTuning,
    pub enabled: bool,
    /// Scale of every hull's shelter of the water beside it.
    pub shelter_strength: f64,
    /// Significant wave height of the sea on show (m).
    pub sea_height: f64,
    pub time: f64,
    pub pixel_angle: f64,
    /// `HULL_FOAM_SLOTS × STRIDE` vec4s, `count` of them in use.
    pub values: Vec<[f32; 4]>,
    pub count: usize,
    speeds: Vec<(String, f64)>,
}

impl Default for HullContactFoam {
    fn default() -> Self {
        Self {
            tuning: HullFoamTuning::default(),
            enabled: true,
            shelter_strength: 1.0,
            sea_height: 0.0,
            time: 0.0,
            pixel_angle: 0.001,
            values: vec![[0.0; 4]; HULL_FOAM_SLOTS * STRIDE],
            count: 0,
            speeds: Vec::new(),
        }
    }
}

impl HullContactFoam {
    /// `camera`: the eye and its vertical field of view (radians); `drawing_height` in pixels.
    pub fn update(
        &mut self,
        ships: &[ContactShip<'_>],
        dt: f64,
        camera: Option<([f64; 3], f64)>,
        drawing_height: f64,
    ) {
        let tuning = self.tuning;
        if dt > 0.0 {
            self.time += dt;
        }
        if let Some((_, fov)) = camera {
            self.pixel_angle = 2.0 * (fov / 2.0).tan() / drawing_height.max(1.0);
        }
        self.speeds
            .retain(|(id, _)| ships.iter().any(|s| s.id == id));
        let mut candidates = Vec::new();
        if self.enabled {
            for ship in ships {
                // A laden surface hull may ride metres below its design waterline; a diving submarine leaves the surface.
                let surface = (1.0 + (ship.position[1] + ship.draft * 0.6) / 3.0).clamp(0.0, 1.0);
                let target = (ship.speed.abs() / ship.forward_speed.max(1.0)).min(1.0) * surface;
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
                if distance < REACH && surface > 0.2 {
                    candidates.push((ship, speed, distance, surface));
                }
            }
        }
        candidates.sort_by(|a, b| a.2.total_cmp(&b.2));
        let mut slot = 0;
        for (ship, speed, _, surface) in candidates {
            if slot >= HULL_FOAM_SLOTS {
                break;
            }
            let sea = self.sea_height.min(8.0);
            let scale = (ship.beam / 32.0).clamp(0.6, 1.3);
            let width =
                (tuning.width + tuning.sea_width * sea + tuning.speed_width * speed) * scale;
            if let Some((eye, _)) = camera
                && self.sub_pixel(width.max(ship.beam * 0.3), ship, eye)
            {
                continue;
            }
            let profile = ship.profile;
            let rotation = DQuat::from_euler(EulerRot::YXZ, -ship.heading, ship.pitch, ship.roll);
            let matrix = DMat4::from_rotation_translation(rotation, DVec3::from(ship.position));
            let inverse = matrix.inverse();
            let base = slot * STRIDE;
            for row in 0..3 {
                let r = inverse.row(row);
                self.values[base + 1 + row] = [r.x as f32, r.y as f32, r.z as f32, r.w as f32];
            }
            self.write_profile(slot, profile);
            let length = f64::from(profile.stern - profile.bow);
            self.values[base + 4] = [
                profile.start,
                profile.spacing,
                profile.half_beam,
                profile.bow,
            ];
            self.values[base + 5] = [
                ((tuning.rest + tuning.sea * sea + tuning.underway * speed) * surface) as f32,
                (tuning.bow * speed * surface) as f32,
                width as f32,
                length.max(1.0) as f32,
            ];
            let range = camera.map_or(0.0, |(eye, _)| {
                (ship.position[0] - eye[0]).hypot(ship.position[2] - eye[2])
            });
            let shelter_reach = SHELTER_REACH * f64::from(profile.tallest_side()).max(1.0);
            let upright = (rotation * DVec3::Y).y;
            self.values[base + 6] = [
                (self.shelter_strength
                    * surface
                    * (1.0 - smoothstep(REACH * SHELTER_RANGE_FADE, REACH, range))
                    * smoothstep(SHELTER_HEEL[1].cos(), SHELTER_HEEL[0].cos(), upright))
                    as f32,
                shelter_reach as f32,
                0.0,
                0.0,
            ];
            // Every open-water pixel tests these circles alone; only water near a hull reads the rest.
            let mid = f64::from(profile.start)
                + f64::from(profile.spacing) * (PROFILE_STATIONS as f64 - 1.0) / 2.0;
            let hull = (f64::from(profile.spacing) * PROFILE_STATIONS as f64 / 2.0)
                .hypot(f64::from(profile.half_beam));
            let reach = hull + 3.0 * width;
            let shelter = hull + shelter_reach;
            let axis = matrix.z_axis;
            self.values[base] = [
                (matrix.w_axis.x + axis.x * mid) as f32,
                (matrix.w_axis.z + axis.z * mid) as f32,
                (reach * reach) as f32,
                (shelter * shelter) as f32,
            ];
            slot += 1;
        }
        self.count = slot;
    }

    /// A line this far off spans a small share of the three pixels the fragment stage needs.
    fn sub_pixel(&self, width: f64, ship: &ContactShip<'_>, eye: [f64; 3]) -> bool {
        let range = ((ship.position[0] - eye[0]).powi(2)
            + eye[1].powi(2)
            + (ship.position[2] - eye[2]).powi(2))
        .sqrt()
        .max(1.0);
        let finest = range * self.pixel_angle / (eye[1].abs() / range).max(0.04) * 3.0;
        width / finest < 0.05
    }

    fn write_profile(&mut self, slot: usize, profile: &WaterlineProfile) {
        let empty = -EMPTY * profile.half_beam;
        let read = |values: &[f32], i: usize| if values[i].is_nan() { empty } else { values[i] };
        for level in 0..PROFILE_LEVELS {
            for station in 0..PROFILE_STATIONS {
                let i = level * PROFILE_STATIONS + station;
                let next = level * PROFILE_STATIONS + (station + 1).min(PROFILE_STATIONS - 1);
                self.values[slot * STRIDE + PROFILE + i] = [
                    read(&profile.starboard, i),
                    read(&profile.port, i),
                    read(&profile.starboard, next),
                    read(&profile.port, next),
                ];
            }
        }
        for station in 0..PROFILE_STATIONS {
            let next = (station + 1).min(PROFILE_STATIONS - 1);
            self.values[slot * STRIDE + SIDES + station] = [
                profile.starboard_side[station],
                profile.port_side[station],
                profile.starboard_side[next],
                profile.port_side[next],
            ];
        }
    }

    pub fn reset(&mut self) {
        self.speeds.clear();
        self.count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hull_profile::SimpleHull;

    /// The shelter a hull at `roll` gives the water beside her.
    fn shelter_at(roll: f64) -> f32 {
        let profile = SimpleHull::warship(115.0, 12.0, 4.0).profile();
        let mut foam = HullContactFoam::default();
        let ship = ContactShip {
            id: "fletcher",
            position: [0.0, -0.5, 0.0],
            heading: 0.0,
            pitch: 0.0,
            roll,
            speed: 0.0,
            beam: 12.0,
            draft: 4.0,
            forward_speed: 18.0,
            profile: &profile,
        };
        foam.update(&[ship], 0.0, Some(([0.0, 20.0, 300.0], 0.8)), 1000.0);
        assert_eq!(foam.count, 1);
        foam.values[6][0]
    }

    #[test]
    fn a_capsized_hull_shelters_no_water() {
        assert_eq!(shelter_at(0.0), 1.0);
        assert_eq!(shelter_at(20f64.to_radians()), 1.0);
        assert!(shelter_at(45f64.to_radians()) < 1.0);
        assert_eq!(shelter_at(150f64.to_radians()), 0.0);
        assert_eq!(shelter_at(-90f64.to_radians()), 0.0);
    }
}
