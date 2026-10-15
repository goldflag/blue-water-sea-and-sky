//! Renderer-free shape of a cloud-to-ground channel, ported from the web game's `sky/weather/bolt.ts`: a tortuous main
//! channel from the cloud base to the sea (a directed random walk refined by midpoint displacement) with branches
//! forking down from its upper part, as segments `shaders/bolt.wgsl` draws.

use crate::lightning::Random;

/// Segments one channel may hold, and the longest segment it keeps (m).
pub const MAX_BOLT_SEGMENTS: usize = 480;
const MAX_SEGMENT: f64 = 40.0;
const WIDTH_MAIN: f64 = 2.2;
const WIDTH_BRANCH: f64 = 1.4;
const WIDTH_FORK: f64 = 0.9;

/// Per segment: start (x, y, z relative to where the bolt meets the sea, width) and end (x, y, z, brightness:
/// positive on the main channel, negative on branches, which only the first stroke lights), as `Float32Array`s.
#[derive(Clone, Debug, Default)]
pub struct BoltChannel {
    pub start: Vec<[f32; 4]>,
    pub end: Vec<[f32; 4]>,
}

type Point = [f64; 3];

fn unit(random: &mut Random) -> Point {
    let z = random.draw() * 2.0 - 1.0;
    let angle = random.draw() * 2.0 * std::f64::consts::PI;
    let r = (1.0 - z * z).sqrt();
    [r * angle.cos(), z, r * angle.sin()]
}

fn wander(random: &mut Random, from: Point, to: Point, legs: usize, roughness: f64) -> Vec<Point> {
    let mut points = vec![from];
    let [mut x, mut y, mut z] = from;
    let (mut dx, mut dy, mut dz) = (0.0, 0.0, 0.0);
    let mut left = legs;
    while left > 1 {
        let n = left as f64;
        let (lx, ly, lz) = ((to[0] - x) / n, (to[1] - y) / n, (to[2] - z) / n);
        let length = (lx * lx + ly * ly + lz * lz).sqrt();
        let [ux, uy, uz] = unit(random);
        dx = dx * 0.35 + ux * length * roughness;
        dy = dy * 0.35 + uy * length * roughness * 0.4;
        dz = dz * 0.35 + uz * length * roughness;
        x += lx + dx;
        y += ly + dy;
        z += lz + dz;
        points.push([x, y, z]);
        left -= 1;
    }
    points.push(to);
    points
}

fn refine(random: &mut Random, mut points: Vec<Point>, levels: usize, amount: f64) -> Vec<Point> {
    for level in 0..levels + 4 {
        let mut next = vec![points[0]];
        let mut split = false;
        for i in 1..points.len() {
            let (a, b) = (points[i - 1], points[i]);
            let length =
                ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt();
            if level >= levels && length <= MAX_SEGMENT {
                next.push(b);
                continue;
            }
            let [ux, uy, uz] = unit(random);
            next.push([
                (a[0] + b[0]) / 2.0 + ux * length * amount,
                (a[1] + b[1]) / 2.0 + uy * length * amount * 0.5,
                (a[2] + b[2]) / 2.0 + uz * length * amount,
            ]);
            next.push(b);
            split = true;
        }
        points = next;
        if !split {
            break;
        }
    }
    points
}

fn emit(
    channel: &mut BoltChannel,
    random: &mut Random,
    points: &[Point],
    width: f64,
    from: f64,
    to: f64,
) {
    let last = points.len() - 1;
    let mut i = 1;
    while i <= last && channel.start.len() < MAX_BOLT_SEGMENTS {
        let (a, b) = (points[i - 1], points[i]);
        let brightness =
            (from + (to - from) * i as f64 / last as f64) * (1.0 - 0.2 * random.draw());
        channel
            .start
            .push([a[0] as f32, a[1] as f32, a[2] as f32, width as f32]);
        channel
            .end
            .push([b[0] as f32, b[1] as f32, b[2] as f32, brightness as f32]);
        i += 1;
    }
}

/// A bolt from `top` (on the cloud base) down to `bottom`, both relative to the ground point (`buildBolt`).
pub fn build_bolt(random: &mut Random, top: Point, bottom: Point) -> BoltChannel {
    let mut channel = BoltChannel::default();
    let height = (top[1] - bottom[1]).max(1.0);
    let legs = ((height / 100.0).round() as usize).clamp(5, 20);
    let walk = wander(random, top, bottom, legs, 0.9);
    let main = refine(random, walk, 2, 0.22);
    emit(&mut channel, random, &main, WIDTH_MAIN, 1.0, 1.0);
    let branches = 4 + (random.draw() * 6.0).floor() as usize;
    for _ in 0..branches {
        let root = main[((0.02 + random.draw() * 0.6) * (main.len() - 1) as f64).floor() as usize];
        let drop = root[1] - bottom[1];
        let length = drop * (0.2 + random.draw() * 0.4);
        if length < 20.0 {
            continue;
        }
        let heading = random.draw() * 2.0 * std::f64::consts::PI;
        let spread = 0.9 + random.draw() * 1.6;
        let norm = spread.hypot(1.0);
        let end: Point = [
            root[0] + heading.sin() * spread / norm * length,
            (bottom[1] + 0.1 * height).max(root[1] - length / norm),
            root[2] + heading.cos() * spread / norm * length,
        ];
        let walk = wander(
            random,
            root,
            end,
            ((length / 45.0).round() as usize).max(3),
            0.6,
        );
        let branch = refine(random, walk, 1, 0.2);
        let brightness = 0.6 + random.draw() * 0.3;
        emit(
            &mut channel,
            random,
            &branch,
            WIDTH_BRANCH,
            -brightness,
            -0.15,
        );
        if random.draw() < 0.4 && branch.len() > 4 {
            let fork = branch[(branch.len() as f64 * (0.3 + random.draw() * 0.3)).floor() as usize];
            let reach = (fork[1] - end[1]) * 0.8 + length * 0.15;
            let side = if random.draw() < 0.5 { -1.0 } else { 1.0 };
            let turn = heading + side * (0.5 + random.draw() * 0.8);
            let tip: Point = [
                fork[0] + turn.sin() * reach * 0.7,
                (bottom[1] + 0.1 * height).max(fork[1] - reach * 0.7),
                fork[2] + turn.cos() * reach * 0.7,
            ];
            let walk = wander(
                random,
                fork,
                tip,
                ((reach / 45.0).round() as usize).max(2),
                0.6,
            );
            let refined = refine(random, walk, 1, 0.2);
            emit(
                &mut channel,
                random,
                &refined,
                WIDTH_FORK,
                -brightness * 0.65,
                -0.1,
            );
        }
    }
    channel
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The recorded game's forced strike (seed 7 at (2600, 0, 380) under a 650 m base): Bun gives 91 segments.
    #[test]
    fn the_bolt_matches_the_web() {
        let mut random = Random::new(3_134_562_008);
        let top = [
            2645.9231536439847 - 2600.0,
            650.0,
            410.8109231691393 - 380.0,
        ];
        let channel = build_bolt(&mut random, top, [0.0, -3.0, 0.0]);
        assert_eq!(channel.start.len(), 91);
        let first = [
            45.923_15, 650.0, 30.810_92, 2.2, 32.276_88, 617.042_2, 28.083_51, 2.2,
        ];
        let got = [channel.start[0], channel.start[1]].concat();
        for (a, b) in got.iter().zip(first.iter()) {
            assert!((a - b).abs() < 1e-3, "{got:?}");
        }
        let last = channel.end[90];
        for (a, b) in last
            .iter()
            .zip([-68.468_57, 326.752_56, 112.009_17, -0.081_501].iter())
        {
            assert!((a - b).abs() < 1e-3, "{last:?}");
        }
    }
}
