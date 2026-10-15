// Ports the web game's `ocean/noise.ts`: smooth value noise laid on the world, never repeating. An integer hash of lattice
// indices (no float hash inputs), interpolated with smoothstep weights. Every tiled pattern the ocean draws repeats;
// this is what varies them from one copy to the next (breaking groups, gusts, the windrows' layout).
#define_import_path naval_ocean::noise

// Integer hash of a lattice point to [0, 1). `seed_mix` is Math.imul(seed, 0x9e3779b9) >>> 0, precomputed by
// `noise_seed`.
fn noise_hash(x: i32, y: i32, seed_mix: u32) -> f32 {
    let h0 = (bitcast<u32>(x) * 0x27d4eb2du) ^ (bitcast<u32>(y) * 0x165667b1u) ^ seed_mix;
    let h1 = (h0 ^ (h0 >> 15u)) * 0x85ebca6bu;
    let h2 = (h1 ^ (h1 >> 13u)) * 0xc2b2ae35u;
    return f32(h2 ^ (h2 >> 16u)) * (1.0 / 4294967296.0);
}

fn noise_seed(seed: u32) -> u32 {
    return seed * 0x9e3779b9u;
}

// Value noise at `p`, in lattice cells, 0–1 (mean ½, standard deviation 0.214).
fn value_noise(p: vec2<f32>, seed: u32) -> f32 {
    let mixed = noise_seed(seed);
    let cell = floor(p);
    let f = p - cell;
    let s = f * f * (3.0 - 2.0 * f);
    let x = i32(cell.x);
    let y = i32(cell.y);
    let top = mix(noise_hash(x, y, mixed), noise_hash(x + 1, y, mixed), s.x);
    let bottom = mix(noise_hash(x, y + 1, mixed), noise_hash(x + 1, y + 1, mixed), s.x);
    return mix(top, bottom, s.y);
}
