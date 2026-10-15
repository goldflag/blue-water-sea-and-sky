// MaterialX Perlin noise as three.js r185 ships it (`src/nodes/materialx/lib/mx_noise.js`, transpiled from
// MaterialX `libraries/stdlib/genglsl/lib/mx_noise.glsl`): the `mx_noise_float` (2D and 3D), `mx_noise_vec3`
// (3D) and `mx_fractal_noise_float` (3D) the celestial bodies use (the web game's `sky/celestial/discs.ts`,
// `milkyWay.ts`), with the same Bob Jenkins hash, so the moon's maria and the Milky Way's dust match the web.
#define_import_path naval_sky::noise

fn mx_rotl32(x: u32, k: u32) -> u32 {
    return (x << k) | (x >> (32u - k));
}

fn mx_bjfinal(a_in: u32, b_in: u32, c_in: u32) -> u32 {
    var a = a_in;
    var b = b_in;
    var c = c_in;
    c ^= b; c -= mx_rotl32(b, 14u);
    a ^= c; a -= mx_rotl32(c, 11u);
    b ^= a; b -= mx_rotl32(a, 25u);
    c ^= b; c -= mx_rotl32(b, 16u);
    a ^= c; a -= mx_rotl32(c, 4u);
    b ^= a; b -= mx_rotl32(a, 14u);
    c ^= b; c -= mx_rotl32(b, 24u);
    return c;
}

fn mx_hash_int2(x: i32, y: i32) -> u32 {
    let start = 0xdeadbeefu + (2u << 2u) + 13u;
    return mx_bjfinal(start + u32(x), start + u32(y), start);
}

fn mx_hash_int3(x: i32, y: i32, z: i32) -> u32 {
    let start = 0xdeadbeefu + (3u << 2u) + 13u;
    return mx_bjfinal(start + u32(x), start + u32(y), start + u32(z));
}

fn mx_hash_vec3(x: i32, y: i32, z: i32) -> vec3<u32> {
    let h = mx_hash_int3(x, y, z);
    return vec3<u32>(h & 0xffu, (h >> 8u) & 0xffu, (h >> 16u) & 0xffu);
}

fn mx_fade(t: f32) -> f32 {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

fn mx_negate_if(value: f32, b: bool) -> f32 {
    return select(value, -value, b);
}

fn mx_gradient_float2(hash: u32, x: f32, y: f32) -> f32 {
    let h = hash & 7u;
    let u = select(y, x, h < 4u);
    let v = 2.0 * select(x, y, h < 4u);
    return mx_negate_if(u, (h & 1u) != 0u) + mx_negate_if(v, (h & 2u) != 0u);
}

fn mx_gradient_float3(hash: u32, x: f32, y: f32, z: f32) -> f32 {
    let h = hash & 15u;
    let u = select(y, x, h < 8u);
    let v = select(select(z, x, h == 12u || h == 14u), y, h < 4u);
    return mx_negate_if(u, (h & 1u) != 0u) + mx_negate_if(v, (h & 2u) != 0u);
}

fn mx_gradient_vec3(hash: vec3<u32>, x: f32, y: f32, z: f32) -> vec3<f32> {
    return vec3<f32>(mx_gradient_float3(hash.x, x, y, z), mx_gradient_float3(hash.y, x, y, z), mx_gradient_float3(hash.z, x, y, z));
}

fn mx_bilerp(v0: f32, v1: f32, v2: f32, v3: f32, s: f32, t: f32) -> f32 {
    let s1 = 1.0 - s;
    return (1.0 - t) * (v0 * s1 + v1 * s) + t * (v2 * s1 + v3 * s);
}

fn mx_trilerp(v0: f32, v1: f32, v2: f32, v3: f32, v4: f32, v5: f32, v6: f32, v7: f32, s: f32, t: f32, r: f32) -> f32 {
    let s1 = 1.0 - s;
    let t1 = 1.0 - t;
    let r1 = 1.0 - r;
    return r1 * (t1 * (v0 * s1 + v1 * s) + t * (v2 * s1 + v3 * s)) + r * (t1 * (v4 * s1 + v5 * s) + t * (v6 * s1 + v7 * s));
}

fn mx_trilerp3(v0: vec3<f32>, v1: vec3<f32>, v2: vec3<f32>, v3: vec3<f32>, v4: vec3<f32>, v5: vec3<f32>, v6: vec3<f32>, v7: vec3<f32>, s: f32, t: f32, r: f32) -> vec3<f32> {
    let s1 = 1.0 - s;
    let t1 = 1.0 - t;
    let r1 = 1.0 - r;
    return r1 * (t1 * (v0 * s1 + v1 * s) + t * (v2 * s1 + v3 * s)) + r * (t1 * (v4 * s1 + v5 * s) + t * (v6 * s1 + v7 * s));
}

// 2D Perlin noise, about −1 … 1 (`mx_noise_float(vec2)`).
fn mx_noise_float2(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let X = i32(cell.x);
    let Y = i32(cell.y);
    let fx = p.x - cell.x;
    let fy = p.y - cell.y;
    let u = mx_fade(fx);
    let v = mx_fade(fy);
    let result = mx_bilerp(
        mx_gradient_float2(mx_hash_int2(X, Y), fx, fy),
        mx_gradient_float2(mx_hash_int2(X + 1, Y), fx - 1.0, fy),
        mx_gradient_float2(mx_hash_int2(X, Y + 1), fx, fy - 1.0),
        mx_gradient_float2(mx_hash_int2(X + 1, Y + 1), fx - 1.0, fy - 1.0),
        u, v);
    return 0.6616 * result;
}

// 3D Perlin noise (`mx_noise_float(vec3)`).
fn mx_noise_float3(p: vec3<f32>) -> f32 {
    let cell = floor(p);
    let X = i32(cell.x);
    let Y = i32(cell.y);
    let Z = i32(cell.z);
    let f = p - cell;
    let u = mx_fade(f.x);
    let v = mx_fade(f.y);
    let w = mx_fade(f.z);
    let result = mx_trilerp(
        mx_gradient_float3(mx_hash_int3(X, Y, Z), f.x, f.y, f.z),
        mx_gradient_float3(mx_hash_int3(X + 1, Y, Z), f.x - 1.0, f.y, f.z),
        mx_gradient_float3(mx_hash_int3(X, Y + 1, Z), f.x, f.y - 1.0, f.z),
        mx_gradient_float3(mx_hash_int3(X + 1, Y + 1, Z), f.x - 1.0, f.y - 1.0, f.z),
        mx_gradient_float3(mx_hash_int3(X, Y, Z + 1), f.x, f.y, f.z - 1.0),
        mx_gradient_float3(mx_hash_int3(X + 1, Y, Z + 1), f.x - 1.0, f.y, f.z - 1.0),
        mx_gradient_float3(mx_hash_int3(X, Y + 1, Z + 1), f.x, f.y - 1.0, f.z - 1.0),
        mx_gradient_float3(mx_hash_int3(X + 1, Y + 1, Z + 1), f.x - 1.0, f.y - 1.0, f.z - 1.0),
        u, v, w);
    return 0.982 * result;
}

// 3D Perlin noise, three channels (`mx_noise_vec3(vec3)`).
fn mx_noise_vec3(p: vec3<f32>) -> vec3<f32> {
    let cell = floor(p);
    let X = i32(cell.x);
    let Y = i32(cell.y);
    let Z = i32(cell.z);
    let f = p - cell;
    let u = mx_fade(f.x);
    let v = mx_fade(f.y);
    let w = mx_fade(f.z);
    let result = mx_trilerp3(
        mx_gradient_vec3(mx_hash_vec3(X, Y, Z), f.x, f.y, f.z),
        mx_gradient_vec3(mx_hash_vec3(X + 1, Y, Z), f.x - 1.0, f.y, f.z),
        mx_gradient_vec3(mx_hash_vec3(X, Y + 1, Z), f.x, f.y - 1.0, f.z),
        mx_gradient_vec3(mx_hash_vec3(X + 1, Y + 1, Z), f.x - 1.0, f.y - 1.0, f.z),
        mx_gradient_vec3(mx_hash_vec3(X, Y, Z + 1), f.x, f.y, f.z - 1.0),
        mx_gradient_vec3(mx_hash_vec3(X + 1, Y, Z + 1), f.x - 1.0, f.y, f.z - 1.0),
        mx_gradient_vec3(mx_hash_vec3(X, Y + 1, Z + 1), f.x, f.y - 1.0, f.z - 1.0),
        mx_gradient_vec3(mx_hash_vec3(X + 1, Y + 1, Z + 1), f.x - 1.0, f.y - 1.0, f.z - 1.0),
        u, v, w);
    return 0.982 * result;
}

// Fractal sum of 3D Perlin octaves (`mx_fractal_noise_float(vec3, octaves, lacunarity, diminish)`).
fn mx_fractal_noise_float(p_in: vec3<f32>, octaves: i32, lacunarity: f32, diminish: f32) -> f32 {
    var p = p_in;
    var result = 0.0;
    var amplitude = 1.0;
    for (var i = 0; i < octaves; i++) {
        result += amplitude * mx_noise_float3(p);
        amplitude *= diminish;
        p *= lacunarity;
    }
    return result;
}
