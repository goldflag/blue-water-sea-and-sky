//! The objects that disturb the wake, turned into per-step swept segments: a port of
//! the web game's `ocean/wake/generators.ts`. A frame's steps interpolate each generator between its previous and current
//! frame positions, so a slow frame that runs several steps still lays a continuous path.

/// Generators the step shader loops over.
pub const MAX_GENERATORS: usize = 16;
/// Swept speed (m/s) at which a generator reaches full amplitude; slower motion fades it to nothing.
const FULL_SPEED: f64 = 1.0;
/// Time constant (s) of the emitted depth: a hull that appears, jumps or starts at speed presses the sea in over about
/// a second instead of in one step.
const RESPONSE: f64 = 0.8;

/// What one generator puts into one simulation step: the path swept since its previous step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WakeEmission {
    pub x0: f64,
    pub z0: f64,
    pub x1: f64,
    pub z1: f64,
    pub radius: f64,
    /// Hull depth after the speed gate (m).
    pub depth: f64,
    /// Speed gate 0–1 for foam along the hull path.
    pub gate: f64,
    /// World x, z of the far end of the stretch where its own waves break (from the swept path's end), and how far
    /// either side of that stretch they break (m).
    pub breaks_to: [f64; 2],
    pub break_reach: f64,
}

/// A generator's settings (`WakeGeneratorOptions`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeneratorOptions {
    pub active: bool,
    pub depth: f64,
    pub radius: f64,
    pub teleport_threshold: f64,
    /// Offset from the object's origin in its own frame (x starboard, z aft), turned by its yaw only.
    pub offset: [f64; 2],
    /// Where the stretch of water its own waves break along ends, in the object's frame as `offset` (a hull's middle
    /// for her bow and stern generators: her waves break along her sides), and how far either side of that stretch
    /// (m). Only there does the field take its breaking threshold, which is tuned for the hulls' waves.
    pub breaks_to: [f64; 2],
    pub break_reach: f64,
}

impl Default for GeneratorOptions {
    fn default() -> Self {
        Self {
            active: true,
            depth: 0.3,
            radius: 5.0,
            teleport_threshold: 50.0,
            offset: [0.0, 0.0],
            breaks_to: [0.0, 0.0],
            break_reach: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Generator {
    options: GeneratorOptions,
    /// The object's world x, z and heading, set each frame.
    pose: [f64; 3],
    previous: Option<[f64; 2]>,
    current: Option<[f64; 2]>,
    last: Option<[f64; 2]>,
    level: f64,
}

/// Tracks the generators and their history. Ids are stable handles.
#[derive(Default)]
pub struct WakeGenerators {
    generators: Vec<(u32, Generator)>,
    next_id: u32,
}

impl WakeGenerators {
    /// A generator following an object at world `x`, `z` on `heading` (clockwise from above, 0 toward −Z).
    pub fn add(&mut self, options: GeneratorOptions) -> Option<u32> {
        if self.generators.len() >= MAX_GENERATORS {
            return None;
        }
        self.next_id += 1;
        let id = self.next_id;
        self.generators.push((
            id,
            Generator {
                options,
                pose: [0.0; 3],
                previous: None,
                current: None,
                last: None,
                level: 0.0,
            },
        ));
        Some(id)
    }

    pub fn update(&mut self, id: u32, update: impl FnOnce(&mut GeneratorOptions)) -> bool {
        match self.generators.iter_mut().find(|(g, _)| *g == id) {
            Some((_, generator)) => {
                update(&mut generator.options);
                true
            }
            None => false,
        }
    }

    /// Where the object is this frame: world x, z and heading.
    pub fn set_pose(&mut self, id: u32, x: f64, z: f64, heading: f64) {
        if let Some((_, generator)) = self.generators.iter_mut().find(|(g, _)| *g == id) {
            generator.pose = [x, z, heading];
        }
    }

    pub fn remove(&mut self, id: u32) -> bool {
        let before = self.generators.len();
        self.generators.retain(|(g, _)| *g != id);
        self.generators.len() != before
    }

    pub fn len(&self) -> usize {
        self.generators.len()
    }

    pub fn is_empty(&self) -> bool {
        self.generators.is_empty()
    }

    /// Forget every position: the next step of each generator only records where it is.
    pub fn restart(&mut self) {
        for (_, generator) in &mut self.generators {
            generator.previous = None;
            generator.current = None;
            generator.last = None;
            generator.level = 0.0;
        }
    }

    /// Sample every object once per advancing frame. A frame move longer than the teleport threshold restarts that
    /// generator's history and freezes it at its new position for the frame's steps.
    pub fn begin_frame(&mut self) {
        for (_, generator) in &mut self.generators {
            let point = emission_point(generator);
            let from = generator.current;
            generator.previous = from;
            generator.current = Some(point);
            if let Some(from) = from
                && (point[0] - from[0]).hypot(point[1] - from[1])
                    > generator.options.teleport_threshold
            {
                generator.previous = Some(point);
                generator.last = None;
                generator.level = 0.0;
            }
        }
    }

    /// Emissions for a step that ends at `fraction` (0, 1] of the current frame and lasts `seconds`.
    pub fn step(&mut self, fraction: f64, seconds: f64, out: &mut Vec<WakeEmission>) {
        out.clear();
        for (_, generator) in &mut self.generators {
            let Some(current) = generator.current else {
                continue;
            };
            let previous = generator.previous.unwrap_or(current);
            let x = previous[0] + (current[0] - previous[0]) * fraction;
            let z = previous[1] + (current[1] - previous[1]) * fraction;
            let last = generator.last;
            generator.last = Some([x, z]);
            let Some(last) = last else { continue };
            let distance = (x - last[0]).hypot(z - last[1]);
            if distance > generator.options.teleport_threshold {
                generator.level = 0.0;
                continue;
            }
            let s = (distance / seconds / FULL_SPEED).min(1.0);
            let gate = if generator.options.active {
                s * s * (3.0 - 2.0 * s)
            } else {
                0.0
            };
            generator.level += (generator.options.depth * gate - generator.level)
                * (1.0 - (-seconds / RESPONSE).exp());
            if gate <= 0.0 {
                continue;
            }
            // The breaking stretch's far end, carried with the step's point.
            let end = turned(generator.pose[2], generator.options.breaks_to);
            let start = turned(generator.pose[2], generator.options.offset);
            out.push(WakeEmission {
                x0: last[0],
                z0: last[1],
                x1: x,
                z1: z,
                radius: generator.options.radius,
                depth: generator.level,
                gate,
                breaks_to: [x + end[0] - start[0], z + end[1] - start[1]],
                break_reach: generator.options.break_reach,
            });
        }
    }
}

/// World XZ of the object's origin plus its offset turned by the object's yaw only (the yaw of `Euler(pitch, −heading,
/// roll, 'YXZ')` is −heading).
fn emission_point(generator: &Generator) -> [f64; 2] {
    let [px, pz, heading] = generator.pose;
    let [ox, oz] = turned(heading, generator.options.offset);
    [px + ox, pz + oz]
}

/// An offset in the object's frame turned by its yaw (−`heading`) into the world's.
fn turned(heading: f64, [ox, oz]: [f64; 2]) -> [f64; 2] {
    let (sin, cos) = (-heading).sin_cos();
    [ox * cos + oz * sin, -ox * sin + oz * cos]
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: f64 = 1.0 / 30.0;

    fn frame(generators: &mut WakeGenerators, steps: usize) -> Vec<Vec<WakeEmission>> {
        generators.begin_frame();
        (0..steps)
            .map(|i| {
                let mut out = Vec::new();
                generators.step((i + 1) as f64 / steps as f64, STEP, &mut out);
                out
            })
            .collect()
    }

    #[test]
    fn the_swept_path_is_continuous() {
        let mut generators = WakeGenerators::default();
        let id = generators
            .add(GeneratorOptions {
                depth: 0.3,
                radius: 8.0,
                ..Default::default()
            })
            .unwrap();
        assert!(frame(&mut generators, 1)[0].is_empty());
        let mut emitted = Vec::new();
        for i in 1..=150 {
            generators.set_pose(id, f64::from(i) * 0.5, 0.0, 0.0);
            emitted.extend(frame(&mut generators, 1).remove(0));
        }
        assert_eq!(emitted.len(), 150);
        for (i, e) in emitted.iter().enumerate() {
            assert!(
                (e.x0 - i as f64 * 0.5).abs() < 1e-9 && (e.x1 - (i + 1) as f64 * 0.5).abs() < 1e-9
            );
            assert_eq!(e.gate, 1.0);
            if i > 0 {
                assert!(e.depth > emitted[i - 1].depth);
            }
        }
        assert!(emitted[0].depth < 0.3 * 0.05);
        assert!((emitted[149].depth - 0.3).abs() < 0.005);
    }

    #[test]
    fn the_offset_turns_with_the_heading() {
        let mut generators = WakeGenerators::default();
        let id = generators
            .add(GeneratorOptions {
                offset: [0.0, -100.0],
                ..Default::default()
            })
            .unwrap();
        // Heading 90° (toward +X): the bow offset (z −100) lies 100 m toward +X.
        generators.set_pose(id, 10.0, 20.0, std::f64::consts::FRAC_PI_2);
        generators.begin_frame();
        let point = generators.generators[0].1.current.unwrap();
        assert!(
            (point[0] - 110.0).abs() < 1e-9 && (point[1] - 20.0).abs() < 1e-9,
            "{point:?}"
        );
    }

    #[test]
    fn the_breaking_stretch_runs_from_each_step_to_the_hulls_middle() {
        let mut generators = WakeGenerators::default();
        let id = generators
            .add(GeneratorOptions {
                offset: [0.0, -100.0],
                breaks_to: [0.0, 0.0],
                break_reach: 40.0,
                ..Default::default()
            })
            .unwrap();
        // Toward +X at 30 m/s: a bow 100 m ahead of the middle.
        generators.set_pose(id, 0.0, 0.0, std::f64::consts::FRAC_PI_2);
        frame(&mut generators, 1);
        generators.set_pose(id, 0.5, 0.0, std::f64::consts::FRAC_PI_2);
        let steps = frame(&mut generators, 2);
        for (step, emissions) in steps.iter().enumerate() {
            let e = emissions[0];
            // Each step's end is its bow; the stretch runs 100 m aft of it, to the middle at that step.
            assert!((e.x1 - (100.0 + 0.25 * (step + 1) as f64)).abs() < 1e-9);
            assert!(
                (e.breaks_to[0] - (e.x1 - 100.0)).abs() < 1e-9 && e.breaks_to[1].abs() < 1e-9,
                "{e:?}"
            );
            assert_eq!(e.break_reach, 40.0);
        }
    }
}
