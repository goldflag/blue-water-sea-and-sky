// MaterialX's 3D Perlin noise, as three's TSL `mx_noise_float(vec3)` computes it: a port of
// node_modules/three/src/nodes/materialx/lib/mx_noise.js (`mx_perlin_noise_float_1` with its Bob Jenkins lookup3 hash,
// gradient, trilerp and fade), itself transpiled from MaterialX's genglsl mx_noise.glsl. The wake's eddies, the bow wave's
// white-water streaks, the contact foam and the torpedo tracks all read it.

#define_import_path naval_ocean::mx_noise

fn mx_rotl(x: u32, k: u32) -> u32 {
    return (x << k) | (x >> (32u - k));
}

fn mx_bjfinal(a_in: u32, b_in: u32, c_in: u32) -> u32 {
    var a = a_in;
    var b = b_in;
    var c = c_in;
    c ^= b;
    c -= mx_rotl(b, 14u);
    a ^= c;
    a -= mx_rotl(c, 11u);
    b ^= a;
    b -= mx_rotl(a, 25u);
    c ^= b;
    c -= mx_rotl(b, 16u);
    a ^= c;
    a -= mx_rotl(c, 4u);
    b ^= a;
    b -= mx_rotl(a, 14u);
    c ^= b;
    c -= mx_rotl(b, 24u);
    return c;
}

fn mx_hash_cell(x: i32, y: i32, z: i32) -> u32 {
    // 0xdeadbeef + (3 << 2) + 13.
    let seed = 0xdeadbeefu + (3u << 2u) + 13u;
    return mx_bjfinal(seed + u32(x), seed + u32(y), seed + u32(z));
}

fn mx_negate_if(value: f32, b: bool) -> f32 {
    return select(value, -value, b);
}

fn mx_gradient_cell(hash: u32, x: f32, y: f32, z: f32) -> f32 {
    let h = hash & 15u;
    let u = select(y, x, h < 8u);
    let v = select(select(z, x, h == 12u || h == 14u), y, h < 4u);
    return mx_negate_if(u, (h & 1u) != 0u) + mx_negate_if(v, (h & 2u) != 0u);
}

fn mx_fade(t: f32) -> f32 {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

fn mx_trilerp(va: f32, vb: f32, vc: f32, vd: f32, ve: f32, vf: f32, vg: f32, vh: f32, s: f32, t: f32, r: f32) -> f32 {
    let si = 1.0 - s;
    let ti = 1.0 - t;
    let ri = 1.0 - r;
    return ri * (ti * (va * si + vb * s) + t * (vc * si + vd * s)) + r * (ti * (ve * si + vf * s) + t * (vg * si + vh * s));
}

// Signed Perlin noise of a 3D point, about −1 … 1 (`mx_noise_float(p)` with amplitude 1 and pivot 0).
fn mx_noise_float(p: vec3<f32>) -> f32 {
    let cell = floor(p);
    let X = i32(cell.x);
    let Y = i32(cell.y);
    let Z = i32(cell.z);
    let fx = p.x - cell.x;
    let fy = p.y - cell.y;
    let fz = p.z - cell.z;
    let u = mx_fade(fx);
    let v = mx_fade(fy);
    let w = mx_fade(fz);
    let result = mx_trilerp(
        mx_gradient_cell(mx_hash_cell(X, Y, Z), fx, fy, fz),
        mx_gradient_cell(mx_hash_cell(X + 1, Y, Z), fx - 1.0, fy, fz),
        mx_gradient_cell(mx_hash_cell(X, Y + 1, Z), fx, fy - 1.0, fz),
        mx_gradient_cell(mx_hash_cell(X + 1, Y + 1, Z), fx - 1.0, fy - 1.0, fz),
        mx_gradient_cell(mx_hash_cell(X, Y, Z + 1), fx, fy, fz - 1.0),
        mx_gradient_cell(mx_hash_cell(X + 1, Y, Z + 1), fx - 1.0, fy, fz - 1.0),
        mx_gradient_cell(mx_hash_cell(X, Y + 1, Z + 1), fx, fy - 1.0, fz - 1.0),
        mx_gradient_cell(mx_hash_cell(X + 1, Y + 1, Z + 1), fx - 1.0, fy - 1.0, fz - 1.0),
        u, v, w,
    );
    return 0.9820 * result;
}
