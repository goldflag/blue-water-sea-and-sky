//! Bubble tracks left on the displaced ocean surface by running torpedoes: the CPU half of
//! the web game's `TorpedoTrackFoam.ts`. Exhaust released at running depth takes seconds to surface, so the track begins well
//! astern of the round, stays where it was laid and outlives it. One fine field follows the view, since a track is
//! narrower than a fleet wake cell.

use crate::trail::{StampCollector, WAKE_EXTENT};

const SAMPLE_DISTANCE: f64 = 4.0;
const LIFETIME: f64 = 30.0;
const UPDATE_INTERVAL: f64 = 1.0 / 20.0;
/// Exhaust rises as a loose plume, faster than a lone bubble.
const RISE_SPEED: f64 = 1.5;
/// Seconds the shallowest exhaust still takes to surface.
const RISE_FLOOR: f64 = 0.6;

fn smooth(value: f64) -> f64 {
    let t = value.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A running torpedo: its id, world position and velocity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackTorpedo {
    pub id: i64,
    pub position: [f64; 3],
    pub velocity: [f64; 3],
}

#[derive(Clone, Copy, Debug)]
struct TrackSample {
    x: f64,
    z: f64,
    right_x: f64,
    right_z: f64,
    surfaced: f64,
    strength: f64,
    phase: f64,
    laid: f64,
}

/// The tracks (`TorpedoTrackFoam`, CPU half): samples laid along each round's path, rasterised into one collector.
pub struct TorpedoTracks {
    resolution: u32,
    pub collector: StampCollector,
    pub time: f64,
    origin: [f64; 2],
    running: Vec<(i64, [f64; 2], f64)>,
    samples: Vec<TrackSample>,
    elapsed: f64,
    painted: bool,
    /// Set when the collector changed and the painter must repaint.
    pub dirty: bool,
}

impl TorpedoTracks {
    pub fn new(resolution: u32) -> Self {
        Self {
            resolution,
            collector: StampCollector::default(),
            time: 0.0,
            origin: [0.0; 2],
            running: Vec::new(),
            samples: Vec::new(),
            elapsed: 0.0,
            painted: false,
            dirty: false,
        }
    }

    pub fn center(&self) -> [f64; 2] {
        self.origin
    }

    pub fn resolution(&self) -> u32 {
        self.resolution
    }

    /// Takes `old`'s tracks (a theatre scene's re-cut), carried by `shift` `[x, z]`: the old scene's origin less this
    /// one's (`naval_sim::earth::SceneShift::xz`), so each point `q` becomes `q + shift`.
    pub fn take_history_from(&mut self, old: &mut Self, shift: [f64; 2]) {
        self.running = std::mem::take(&mut old.running);
        self.samples = std::mem::take(&mut old.samples);
        self.time = old.time;
        self.origin = [old.origin[0] + shift[0], old.origin[1] + shift[1]];
        for (_, position, _) in &mut self.running {
            position[0] += shift[0];
            position[1] += shift[1];
        }
        for sample in &mut self.samples {
            sample.x += shift[0];
            sample.z += shift[1];
        }
        self.rasterize(self.time);
    }

    /// Takes `old`'s tracks into a scene laid on another chart (a theatre's re-lay), each value by its kind: the points
    /// (a round's last, each sample's, the field's middle) through their places, a sample's right through the two
    /// charts' Jacobians where it lies (`naval_render::relay::SceneRelay`).
    pub fn relay_history_from(
        &mut self,
        old: &mut Self,
        relay: &dyn naval_render::relay::SceneRelay,
    ) {
        self.running = std::mem::take(&mut old.running);
        self.samples = std::mem::take(&mut old.samples);
        self.time = old.time;
        self.origin = relay.point2(old.origin);
        for (_, position, _) in &mut self.running {
            *position = relay.point2(*position);
        }
        for sample in &mut self.samples {
            let at = [sample.x, 0.0, sample.z];
            [sample.right_x, sample.right_z] =
                relay.direction(at, [sample.right_x, sample.right_z]);
            [sample.x, sample.z] = relay.point2([sample.x, sample.z]);
        }
        self.rasterize(self.time);
    }

    /// Whether any stamp is painted (the surface skips the field when not).
    pub fn painted(&self) -> bool {
        self.painted
    }

    /// Advance by `dt` (0 does nothing). `focus` is the fleet's centre; `camera` the eye, its forward direction and the
    /// projection's vertical scale (`projectionMatrix[5]`), for where the field centres.
    pub fn update(
        &mut self,
        torpedoes: &[TrackTorpedo],
        dt: f64,
        focus: [f64; 2],
        camera: Option<([f64; 3], [f64; 3], f64)>,
    ) {
        if dt <= 0.0 {
            // Zooming or orbiting a paused battle still moves the field; only the water's age is held.
            let previous = self.origin;
            self.anchor(torpedoes, focus, camera);
            if self.origin != previous && !self.samples.is_empty() {
                self.rasterize(self.time);
            }
            return;
        }
        let laid = self.time;
        self.time += dt;
        self.elapsed += dt;
        let now = self.time;
        self.running
            .retain(|(id, ..)| torpedoes.iter().any(|t| t.id == *id));
        for t in torpedoes {
            let [x, y, z] = t.position;
            let Some(previous) = self.running.iter_mut().find(|(id, ..)| *id == t.id) else {
                self.running.push((t.id, [x, z], 0.0));
                continue;
            };
            let distance = (x - previous.1[0]).hypot(z - previous.1[1]);
            // Airborne rounds leave no exhaust in the sea; deep ones never show through it.
            let strength = if y > 0.0 {
                0.0
            } else {
                smooth((y + 8.0) / 4.0)
            };
            if distance > 200.0 || strength <= 0.0 {
                previous.1 = [x, z];
                previous.2 = 0.0;
                continue;
            }
            if distance < 0.0001 {
                continue;
            }
            let forward_x = (x - previous.1[0]) / distance;
            let forward_z = (z - previous.1[1]) / distance;
            let rise = RISE_FLOOR.max(-y / RISE_SPEED);
            let mut along = SAMPLE_DISTANCE - previous.2;
            while along <= distance {
                let fraction = along / distance;
                let born = now - dt * (1.0 - fraction);
                self.samples.push(TrackSample {
                    x: previous.1[0] + (x - previous.1[0]) * fraction,
                    z: previous.1[1] + (z - previous.1[1]) * fraction,
                    right_x: -forward_z,
                    right_z: forward_x,
                    surfaced: born + rise,
                    strength,
                    phase: t.id as f64 * 2.3 + born * 1.7,
                    laid,
                });
                along += SAMPLE_DISTANCE;
            }
            previous.2 = (previous.2 + distance) % SAMPLE_DISTANCE;
            previous.1 = [x, z];
        }
        if self
            .samples
            .first()
            .is_some_and(|s| now - s.surfaced > LIFETIME)
        {
            self.expire(now);
        }
        if self.elapsed < UPDATE_INTERVAL || (self.samples.is_empty() && !self.painted) {
            return;
        }
        self.elapsed %= UPDATE_INTERVAL;
        self.anchor(torpedoes, focus, camera);
        self.rasterize(now);
    }

    /// Drop every sample that surfaced more than LIFETIME ago, keeping the rest in order. In place: the list holds
    /// every round's last half minute (a megabyte with a destroyer flotilla's spread running) and this runs nearly
    /// every frame, and copying it out and back each time was a tenth of a big battle's frame in the heap.
    fn expire(&mut self, now: f64) {
        let horizon = now - LIFETIME + RISE_FLOOR - 0.1;
        let end = self.samples.iter().take_while(|s| s.laid < horizon).count();
        let mut index = 0;
        self.samples.retain(|s| {
            let laid_before = index < end;
            index += 1;
            !laid_before || now - s.surfaced <= LIFETIME
        });
    }

    /// Centre the field on the water being watched, or the round nearest the sight line. A wide lens must keep the
    /// same track as a magnified one; centring on the eye left its water outside the field when the orbit pulled back.
    fn anchor(
        &mut self,
        torpedoes: &[TrackTorpedo],
        focus: [f64; 2],
        camera: Option<([f64; 3], [f64; 3], f64)>,
    ) {
        let (mut x, mut z) = match camera {
            Some((eye, forward, _)) if eye[1] > 0.0 && forward[1] < -0.001 => {
                let range = -eye[1] / forward[1];
                (eye[0] + forward[0] * range, eye[2] + forward[2] * range)
            }
            Some((eye, ..)) => (eye[0], eye[2]),
            None => (focus[0], focus[1]),
        };
        if let Some((eye, forward, _)) = camera {
            let mut best = 0.985;
            for t in torpedoes {
                let d = [
                    t.position[0] - eye[0],
                    t.position[1] - eye[1],
                    t.position[2] - eye[2],
                ];
                let range = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if range < WAKE_EXTENT * 0.4 {
                    continue;
                }
                let aligned = (d[0] * forward[0] + d[1] * forward[1] + d[2] * forward[2]) / range;
                if aligned > best {
                    best = aligned;
                    x = t.position[0];
                    z = t.position[2];
                }
            }
        }
        let cell = WAKE_EXTENT / f64::from(self.resolution);
        self.origin = [(x / cell).round() * cell, (z / cell).round() * cell];
    }

    fn rasterize(&mut self, now: f64) {
        let [origin_x, origin_z] = self.origin;
        let reach = WAKE_EXTENT / 2.0;
        self.collector.begin(self.origin, self.origin);
        let mut count = 0;
        for sample in &self.samples {
            let age = now - sample.surfaced;
            if age < 0.0
                || (sample.x - origin_x).abs() > reach
                || (sample.z - origin_z).abs() > reach
            {
                continue;
            }
            // Bubbles break the surface over a second or so, then the patch spreads, thins and drifts off the line.
            let strength = sample.strength
                * smooth(age / 2.5)
                * (-age / 14.0).exp()
                * (1.0 - smooth((age - 20.0) / 10.0))
                * 0.88;
            if strength < 0.015 {
                continue;
            }
            let drift = (sample.phase + age * 0.21).sin() * (age * 0.09).min(1.3);
            let x = sample.x + sample.right_x * drift;
            let z = sample.z + sample.right_z * drift;
            let width = 1.3 + age.sqrt() * 0.75;
            let length = SAMPLE_DISTANCE * 1.6;
            self.collector.stamps.push(crate::trail::Stamp {
                x,
                z,
                right_x: sample.right_x,
                right_z: sample.right_z,
                width,
                length,
                strength,
                ring: false,
                channel: 0,
                across: crate::trail::NO_BAND,
            });
            count += 1;
        }
        self.painted = count > 0;
        self.dirty = true;
    }

    pub fn reset(&mut self) {
        self.samples.clear();
        self.running.clear();
        self.elapsed = 0.0;
        if self.painted {
            self.collector.clear();
            self.dirty = true;
        }
        self.painted = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zooming_out_keeps_the_track_under_the_view() {
        let mut tracks = TorpedoTracks::new(1024);
        let eye = [0.0, 1200.0, 1800.0];
        let range = 1200.0_f64.hypot(1800.0);
        let forward = [0.0, -1200.0 / range, -1800.0 / range];
        for i in 0..=400 {
            let t = f64::from(i) / 20.0;
            let torpedo = TrackTorpedo {
                id: 3,
                position: [t * 25.0, -2.0, 0.0],
                velocity: [25.0, 0.0, 0.0],
            };
            // Follow the same torpedo from a close lens, then an ordinary wide lens.
            let mut camera_eye = eye;
            camera_eye[0] = torpedo.position[0];
            tracks.update(
                &[torpedo],
                1.0 / 20.0,
                [0.0, 0.0],
                Some((camera_eye, forward, if i < 300 { 8.0 } else { 2.05 })),
            );
        }
        assert!(
            tracks.painted(),
            "a torpedo in the centre of the wide view lost its wake"
        );
        assert!(tracks.collector.stamps.len() > 20);
        // A frozen clock and a spent round keep the water visible while the eye moves farther back.
        let before = tracks.time;
        tracks.update(
            &[],
            0.0,
            [0.0, 0.0],
            Some(([500.0, 2400.0, 3600.0], forward, 2.05)),
        );
        assert_eq!(tracks.time, before);
        assert!(tracks.painted());
    }

    #[test]
    fn a_track_surfaces_astern_of_the_round() {
        let mut tracks = TorpedoTracks::new(1024);
        for i in 0..=400 {
            let t = f64::from(i) / 20.0;
            let torpedo = TrackTorpedo {
                id: 3,
                position: [t * 25.0, -4.0, 0.0],
                velocity: [25.0, 0.0, 0.0],
            };
            tracks.update(&[torpedo], 1.0 / 20.0, [0.0, 0.0], None);
        }
        assert!(tracks.painted());
        let xs: Vec<f64> = tracks.collector.stamps.iter().map(|s| s.x).collect();
        let front = xs.iter().copied().fold(f64::MIN, f64::max);
        // Exhaust from 4 m takes 2.7 s and more to show: the track ends well astern of the round (at 500 m).
        assert!(front < 500.0 - 25.0 * 2.5, "{front}");
        assert!(xs.iter().copied().fold(f64::MAX, f64::min) < 100.0);
    }

    #[test]
    fn spent_samples_go_in_place_and_in_order() {
        let mut tracks = TorpedoTracks::new(1024);
        let run = |tracks: &mut TorpedoTracks, from: u32, to: u32| {
            for i in from..to {
                let t = f64::from(i) / 60.0;
                let torpedo = |id: i64, z: f64| TrackTorpedo {
                    id,
                    position: [t * 25.0, -3.0, z],
                    velocity: [25.0, 0.0, 0.0],
                };
                tracks.update(
                    &[torpedo(1, 0.0), torpedo(2, 50.0)],
                    1.0 / 60.0,
                    [0.0, 0.0],
                    None,
                );
            }
        };
        // Past the lifetime, so the oldest samples are expiring frame by frame.
        run(&mut tracks, 0, 60 * 40);
        tracks.samples.reserve(4096);
        let (storage, before) = (tracks.samples.as_ptr(), tracks.samples.len());
        run(&mut tracks, 60 * 40, 60 * 42);
        let now = tracks.time;
        assert_eq!(tracks.samples.as_ptr(), storage, "expiring reallocated");
        // A steady run lays as many as expire: the list neither grows without bound nor empties.
        assert!(
            tracks.samples.len().abs_diff(before) < before / 10,
            "{before} -> {}",
            tracks.samples.len()
        );
        let horizon = now - LIFETIME + RISE_FLOOR - 0.1;
        assert!(
            tracks.samples.iter().all(
                |s| s.laid >= horizon - 1.0 / 60.0 || now - s.surfaced <= LIFETIME + 1.0 / 60.0
            ),
            "a spent sample stayed"
        );
        assert!(
            tracks.samples.windows(2).all(|w| w[0].laid <= w[1].laid),
            "the order changed"
        );
    }
}
