//! Generates `assets/blue_noise.png`, the clouds' own 128 × 128 blue-noise dither mask, with Ulichney's
//! void-and-cluster method ("The void-and-cluster method for dither array generation", SPIE 1993): a Gaussian energy
//! (σ 1.9 texels) on the torus, a seeded 10 % initial pattern relaxed until stable, then ranks by removing the
//! tightest clusters and filling the largest voids. Each of the 256 grey levels holds exactly 64 texels. The clouds
//! animate it over frames by golden-ratio offsets (`naval_clouds::field::stbn`), standing in for takram's NVIDIA STBN
//! texture, which is licensed for non-commercial use only and is not shipped.
//!
//! `cargo run -p naval-clouds --example blue_noise --release` rewrites the asset; the same seed gives the same bytes.

use std::path::PathBuf;

const N: usize = 128;
const SIGMA: f64 = 1.9;
const RADIUS: i64 = 12;
const SEED: u64 = 0x5eed_c10d;

struct Energy {
    values: Vec<f64>,
    kernel: Vec<f64>,
}

impl Energy {
    fn new() -> Self {
        let side = (2 * RADIUS + 1) as usize;
        let mut kernel = vec![0.0; side * side];
        for dy in -RADIUS..=RADIUS {
            for dx in -RADIUS..=RADIUS {
                let r2 = (dx * dx + dy * dy) as f64;
                kernel[((dy + RADIUS) as usize) * side + (dx + RADIUS) as usize] =
                    (-r2 / (2.0 * SIGMA * SIGMA)).exp();
            }
        }
        Self {
            values: vec![0.0; N * N],
            kernel,
        }
    }

    /// Adds (`sign` 1) or removes (−1) a point's energy at `index` everywhere it reaches, on the torus.
    fn splat(&mut self, index: usize, sign: f64) {
        let (x, y) = ((index % N) as i64, (index / N) as i64);
        let side = (2 * RADIUS + 1) as usize;
        for dy in -RADIUS..=RADIUS {
            let row = (y + dy).rem_euclid(N as i64) as usize * N;
            for dx in -RADIUS..=RADIUS {
                let column = (x + dx).rem_euclid(N as i64) as usize;
                self.values[row + column] +=
                    sign * self.kernel[((dy + RADIUS) as usize) * side + (dx + RADIUS) as usize];
            }
        }
    }

    fn from_pattern(pattern: &[bool]) -> Self {
        let mut energy = Self::new();
        for (index, &set) in pattern.iter().enumerate() {
            if set {
                energy.splat(index, 1.0);
            }
        }
        energy
    }

    /// The set point with the most energy (the tightest cluster).
    fn tightest(&self, pattern: &[bool]) -> usize {
        let mut best = (f64::MIN, 0);
        for (index, &set) in pattern.iter().enumerate() {
            if set && self.values[index] > best.0 {
                best = (self.values[index], index);
            }
        }
        best.1
    }

    /// The unset point with the least energy (the largest void).
    fn largest_void(&self, pattern: &[bool]) -> usize {
        let mut best = (f64::MAX, 0);
        for (index, &set) in pattern.iter().enumerate() {
            if !set && self.values[index] < best.0 {
                best = (self.values[index], index);
            }
        }
        best.1
    }
}

/// A small deterministic generator (SplitMix64).
fn random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn main() -> anyhow::Result<()> {
    let total = N * N;
    // The initial binary pattern: a tenth of the texels, then relaxed until no cluster moves.
    let mut state = SEED;
    let mut prototype = vec![false; total];
    let mut placed = 0;
    while placed < total / 10 {
        let index = (random(&mut state) % total as u64) as usize;
        if !prototype[index] {
            prototype[index] = true;
            placed += 1;
        }
    }
    let mut energy = Energy::from_pattern(&prototype);
    loop {
        let tightest = energy.tightest(&prototype);
        prototype[tightest] = false;
        energy.splat(tightest, -1.0);
        let void = energy.largest_void(&prototype);
        prototype[void] = true;
        energy.splat(void, 1.0);
        if void == tightest {
            break;
        }
    }
    let ones = prototype.iter().filter(|&&set| set).count();
    let mut ranks = vec![0usize; total];

    // Phase 1: remove the tightest clusters from the prototype, ranking down from `ones − 1`.
    let mut pattern = prototype.clone();
    let mut energy = Energy::from_pattern(&pattern);
    for rank in (0..ones).rev() {
        let tightest = energy.tightest(&pattern);
        pattern[tightest] = false;
        energy.splat(tightest, -1.0);
        ranks[tightest] = rank;
    }
    // Phase 2: fill the largest voids up to half the texels.
    let mut pattern = prototype;
    let mut energy = Energy::from_pattern(&pattern);
    for rank in ones..total / 2 {
        let void = energy.largest_void(&pattern);
        pattern[void] = true;
        energy.splat(void, 1.0);
        ranks[void] = rank;
    }
    // Phase 3: the rest, as the tightest clusters of the remaining unset texels.
    let inverted: Vec<bool> = pattern.iter().map(|&set| !set).collect();
    let mut minority = inverted;
    let mut energy = Energy::from_pattern(&minority);
    for rank in total / 2..total {
        let tightest = energy.tightest(&minority);
        minority[tightest] = false;
        energy.splat(tightest, -1.0);
        ranks[tightest] = rank;
    }

    let bytes: Vec<u8> = ranks
        .iter()
        .map(|&rank| (rank * 256 / total) as u8)
        .collect();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/blue_noise.png");
    image::GrayImage::from_raw(N as u32, N as u32, bytes)
        .expect("128 × 128 bytes")
        .save(&path)?;
    println!("wrote {}", path.display());
    Ok(())
}
