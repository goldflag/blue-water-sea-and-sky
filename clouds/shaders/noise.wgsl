// The clouds' procedural textures, filled by compute once at startup: the base shape (128³ Perlin–Worley), the
// shape detail (32³ Worley fBm), the local weather (512², one channel per layer) and the turbulence (128², curl of
// Perlin noise). Ports `cloudShape.frag`, `cloudShapeDetail.frag`, `localWeather.frag`, `turbulence.frag`,
// `perlin.glsl` and `tileableNoise.glsl` of @takram/three-clouds
// (https://github.com/takram-design-engineering/three-geospatial, packages/clouds/src/shaders), and adds a box
// filter for the 2D textures' mips (three's `generateMipmaps`).
//
// MIT License
//
// Copyright (c) 2024 Shota Matsuda
//
// Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and
// to permit persons to whom the Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all copies or substantial portions of
// the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO
// THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF
// CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
// IN THE SOFTWARE.
//
// The tileable Worley and Perlin fBm are based on Sébastien Hillaire's TileableVolumeNoise
// (https://github.com/sebh/TileableVolumeNoise), under the same MIT terms, Copyright (c) 2017 Sébastien Hillaire.
// The periodic Perlin noise is ported from GLM (https://github.com/g-truc/glm, gtc/noise.inl), MIT licence.

@group(0) @binding(0) var shape_out: texture_storage_3d<rgba8unorm, write>;
@group(0) @binding(1) var detail_out: texture_storage_3d<rgba8unorm, write>;
@group(0) @binding(2) var weather_out: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(3) var turbulence_out: texture_storage_2d<rgba8unorm, write>;

// GLSL's `mod`: x − y·floor(x / y).
fn modulo4(x: vec4<f32>, y: vec4<f32>) -> vec4<f32> {
    return x - y * floor(x / y);
}

fn modulo3(x: vec3<f32>, y: vec3<f32>) -> vec3<f32> {
    return x - y * floor(x / y);
}

fn mod289(x: vec4<f32>) -> vec4<f32> {
    return x - floor(x * (1.0 / 289.0)) * 289.0;
}

fn permute(v: vec4<f32>) -> vec4<f32> {
    return mod289((v * 34.0 + 1.0) * v);
}

fn taylor_inv_sqrt(r: vec4<f32>) -> vec4<f32> {
    return 1.79284291400159 - 0.85373472095314 * r;
}

fn fade(v: vec4<f32>) -> vec4<f32> {
    return v * v * v * (v * (v * 6.0 - 15.0) + 10.0);
}

struct Gradients {
    x: vec4<f32>,
    y: vec4<f32>,
    z: vec4<f32>,
    w: vec4<f32>,
}

fn gradients(ixy: vec4<f32>) -> Gradients {
    var gx = ixy / 7.0;
    var gy = floor(gx) / 7.0;
    var gz = floor(gy) / 6.0;
    gx = fract(gx) - 0.5;
    gy = fract(gy) - 0.5;
    gz = fract(gz) - 0.5;
    let gw = vec4<f32>(0.75) - abs(gx) - abs(gy) - abs(gz);
    let sw = step(gw, vec4<f32>(0.0));
    gx -= sw * (step(vec4<f32>(0.0), gx) - 0.5);
    gy -= sw * (step(vec4<f32>(0.0), gy) - 0.5);
    return Gradients(gx, gy, gz, gw);
}

fn corner(g: Gradients, index: u32) -> vec4<f32> {
    return vec4<f32>(g.x[index], g.y[index], g.z[index], g.w[index]);
}

// Classic Perlin noise, periodic version (GLM's `perlin(vec4, vec4)`).
fn perlin(position: vec4<f32>, rep: vec4<f32>) -> f32 {
    let pi0 = modulo4(floor(position), rep);
    let pi1 = modulo4(pi0 + 1.0, rep);
    let pf0 = fract(position);
    let pf1 = pf0 - 1.0;
    let ix = vec4<f32>(pi0.x, pi1.x, pi0.x, pi1.x);
    let iy = vec4<f32>(pi0.y, pi0.y, pi1.y, pi1.y);
    let iz0 = vec4<f32>(pi0.z);
    let iz1 = vec4<f32>(pi1.z);
    let iw0 = vec4<f32>(pi0.w);
    let iw1 = vec4<f32>(pi1.w);

    let ixy = permute(permute(ix) + iy);
    let ixy0 = permute(ixy + iz0);
    let ixy1 = permute(ixy + iz1);
    let g00 = gradients(permute(ixy0 + iw0));
    let g01 = gradients(permute(ixy0 + iw1));
    let g10 = gradients(permute(ixy1 + iw0));
    let g11 = gradients(permute(ixy1 + iw1));

    var g0000 = corner(g00, 0u);
    var g1000 = corner(g00, 1u);
    var g0100 = corner(g00, 2u);
    var g1100 = corner(g00, 3u);
    var g0010 = corner(g10, 0u);
    var g1010 = corner(g10, 1u);
    var g0110 = corner(g10, 2u);
    var g1110 = corner(g10, 3u);
    var g0001 = corner(g01, 0u);
    var g1001 = corner(g01, 1u);
    var g0101 = corner(g01, 2u);
    var g1101 = corner(g01, 3u);
    var g0011 = corner(g11, 0u);
    var g1011 = corner(g11, 1u);
    var g0111 = corner(g11, 2u);
    var g1111 = corner(g11, 3u);

    let norm00 = taylor_inv_sqrt(vec4<f32>(dot(g0000, g0000), dot(g0100, g0100), dot(g1000, g1000), dot(g1100, g1100)));
    g0000 *= norm00.x;
    g0100 *= norm00.y;
    g1000 *= norm00.z;
    g1100 *= norm00.w;
    let norm01 = taylor_inv_sqrt(vec4<f32>(dot(g0001, g0001), dot(g0101, g0101), dot(g1001, g1001), dot(g1101, g1101)));
    g0001 *= norm01.x;
    g0101 *= norm01.y;
    g1001 *= norm01.z;
    g1101 *= norm01.w;
    let norm10 = taylor_inv_sqrt(vec4<f32>(dot(g0010, g0010), dot(g0110, g0110), dot(g1010, g1010), dot(g1110, g1110)));
    g0010 *= norm10.x;
    g0110 *= norm10.y;
    g1010 *= norm10.z;
    g1110 *= norm10.w;
    let norm11 = taylor_inv_sqrt(vec4<f32>(dot(g0011, g0011), dot(g0111, g0111), dot(g1011, g1011), dot(g1111, g1111)));
    g0011 *= norm11.x;
    g0111 *= norm11.y;
    g1011 *= norm11.z;
    g1111 *= norm11.w;

    let n0000 = dot(g0000, pf0);
    let n1000 = dot(g1000, vec4<f32>(pf1.x, pf0.y, pf0.z, pf0.w));
    let n0100 = dot(g0100, vec4<f32>(pf0.x, pf1.y, pf0.z, pf0.w));
    let n1100 = dot(g1100, vec4<f32>(pf1.x, pf1.y, pf0.z, pf0.w));
    let n0010 = dot(g0010, vec4<f32>(pf0.x, pf0.y, pf1.z, pf0.w));
    let n1010 = dot(g1010, vec4<f32>(pf1.x, pf0.y, pf1.z, pf0.w));
    let n0110 = dot(g0110, vec4<f32>(pf0.x, pf1.y, pf1.z, pf0.w));
    let n1110 = dot(g1110, vec4<f32>(pf1.x, pf1.y, pf1.z, pf0.w));
    let n0001 = dot(g0001, vec4<f32>(pf0.x, pf0.y, pf0.z, pf1.w));
    let n1001 = dot(g1001, vec4<f32>(pf1.x, pf0.y, pf0.z, pf1.w));
    let n0101 = dot(g0101, vec4<f32>(pf0.x, pf1.y, pf0.z, pf1.w));
    let n1101 = dot(g1101, vec4<f32>(pf1.x, pf1.y, pf0.z, pf1.w));
    let n0011 = dot(g0011, vec4<f32>(pf0.x, pf0.y, pf1.z, pf1.w));
    let n1011 = dot(g1011, vec4<f32>(pf1.x, pf0.y, pf1.z, pf1.w));
    let n0111 = dot(g0111, vec4<f32>(pf0.x, pf1.y, pf1.z, pf1.w));
    let n1111 = dot(g1111, pf1);

    let fade_xyzw = fade(pf0);
    let n_0w = mix(vec4<f32>(n0000, n1000, n0100, n1100), vec4<f32>(n0001, n1001, n0101, n1101), fade_xyzw.w);
    let n_1w = mix(vec4<f32>(n0010, n1010, n0110, n1110), vec4<f32>(n0011, n1011, n0111, n1111), fade_xyzw.w);
    let n_zw = mix(n_0w, n_1w, fade_xyzw.z);
    let n_yzw = mix(n_zw.xy, n_zw.zw, fade_xyzw.y);
    let n_xyzw = mix(n_yzw.x, n_yzw.y, fade_xyzw.x);
    return 2.2 * n_xyzw;
}

fn hash(n: f32) -> f32 {
    return fract(sin(n + 1.951) * 43758.5453);
}

fn value_noise(x: vec3<f32>) -> f32 {
    let p = floor(x);
    var f = fract(x);
    f = f * f * (3.0 - 2.0 * f);
    let n = p.x + p.y * 57.0 + 113.0 * p.z;
    return mix(
        mix(mix(hash(n + 0.0), hash(n + 1.0), f.x), mix(hash(n + 57.0), hash(n + 58.0), f.x), f.y),
        mix(mix(hash(n + 113.0), hash(n + 114.0), f.x), mix(hash(n + 170.0), hash(n + 171.0), f.x), f.y),
        f.z,
    );
}

fn worley(p: vec3<f32>, cell_count: f32) -> f32 {
    let cell = p * cell_count;
    var d = 1.0e10;
    for (var x = -1; x <= 1; x++) {
        for (var y = -1; y <= 1; y++) {
            for (var z = -1; z <= 1; z++) {
                var tp = floor(cell) + vec3<f32>(f32(x), f32(y), f32(z));
                tp = cell - tp - value_noise(modulo3(tp, vec3<f32>(cell_count / 1.0)));
                d = min(d, dot(tp, tp));
            }
        }
    }
    return clamp(d, 0.0, 1.0);
}

fn perlin_fbm(point: vec3<f32>, frequency: vec3<f32>, octave_count: i32) -> f32 {
    // Noise frequency factor between octaves, forced to 2.
    let octave_frequency_factor = 2.0;
    var sum = 0.0;
    let roughness = 0.5;
    var weight_sum = 0.0;
    var weight = 1.0;
    var next_frequency = frequency;
    for (var i = 0; i < octave_count; i++) {
        let p = vec4<f32>(point, 0.0) * vec4<f32>(next_frequency, 1.0);
        let value = perlin(p, vec4<f32>(next_frequency, 1.0));
        sum += value * weight;
        weight_sum += weight;
        weight *= roughness;
        next_frequency *= octave_frequency_factor;
    }
    // Intentionally skip clamping.
    return sum / weight_sum;
}

fn remap(x: f32, min1: f32, max1: f32, min2: f32, max2: f32) -> f32 {
    return min2 + (x - min1) / (max1 - min1) * (max2 - min2);
}

// `cloudShape.frag`.
fn perlin_worley(point: vec3<f32>) -> f32 {
    let octave_count = 3;
    let frequency = 8.0;
    var noise = perlin_fbm(point, vec3<f32>(frequency), octave_count);
    noise = clamp(noise, 0.0, 1.0);
    let cell_count = 4.0;
    let w = vec3<f32>(
        1.0 - worley(point, cell_count * 2.0),
        1.0 - worley(point, cell_count * 8.0),
        1.0 - worley(point, cell_count * 14.0),
    );
    let fbm = dot(w, vec3<f32>(0.625, 0.25, 0.125));
    return remap(noise, 0.0, 1.0, fbm, 1.0);
}

fn worley_fbm_shape(point: vec3<f32>) -> f32 {
    let cell_count = 4.0;
    let w = vec4<f32>(
        1.0 - worley(point, cell_count * 2.0),
        1.0 - worley(point, cell_count * 4.0),
        1.0 - worley(point, cell_count * 8.0),
        1.0 - worley(point, cell_count * 16.0),
    );
    let fbm = vec3<f32>(
        dot(w.xyz, vec3<f32>(0.625, 0.25, 0.125)),
        dot(w.yzw, vec3<f32>(0.625, 0.25, 0.125)),
        dot(w.zw, vec2<f32>(0.75, 0.25)),
    );
    return dot(fbm, vec3<f32>(0.625, 0.25, 0.125));
}

// A texel's point: (x + 0.5) / size on the plane (the full-screen quad's uv), z / size along the layers (three's
// `layer / size`).
fn volume_point(id: vec3<u32>, size: f32) -> vec3<f32> {
    return vec3<f32>((f32(id.x) + 0.5) / size, (f32(id.y) + 0.5) / size, f32(id.z) / size);
}

@compute @workgroup_size(4, 4, 4)
fn shape_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = 128.0;
    if any(id >= vec3<u32>(128u)) {
        return;
    }
    let point = volume_point(id, size);
    let value = remap(perlin_worley(point), worley_fbm_shape(point) - 1.0, 1.0, 0.0, 1.0);
    textureStore(shape_out, id, vec4<f32>(value, 0.0, 0.0, 1.0));
}

@compute @workgroup_size(4, 4, 4)
fn detail_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = 32.0;
    if any(id >= vec3<u32>(32u)) {
        return;
    }
    let point = volume_point(id, size);
    let cell_count = 2.0;
    let w = vec4<f32>(
        1.0 - worley(point, cell_count * 1.0),
        1.0 - worley(point, cell_count * 2.0),
        1.0 - worley(point, cell_count * 4.0),
        1.0 - worley(point, cell_count * 8.0),
    );
    let fbm = vec3<f32>(
        dot(w.xyz, vec3<f32>(0.625, 0.25, 0.125)),
        dot(w.yzw, vec3<f32>(0.625, 0.25, 0.125)),
        dot(w.zw, vec2<f32>(0.75, 0.25)),
    );
    textureStore(detail_out, id, vec4<f32>(dot(fbm, vec3<f32>(0.625, 0.25, 0.125)), 0.0, 0.0, 1.0));
}

fn weather_fbm(point: vec3<f32>, frequency_start: f32, amplitude_start: f32, lacunarity: f32, gain: f32, octave_count: i32) -> f32 {
    var noise = 0.0;
    var frequency = frequency_start;
    var amplitude = amplitude_start;
    for (var i = 0; i < octave_count; i++) {
        noise += amplitude * (1.0 - worley(point, frequency));
        frequency *= lacunarity;
        amplitude *= gain;
    }
    return noise;
}

// `localWeather.frag`: r low clouds, g mid clouds, b high clouds; a the extra Perlin layer (the shipped texture keeps
// it, while the generator overwrites it with 1).
@compute @workgroup_size(8, 8, 1)
fn weather_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = 512.0;
    if any(id.xy >= vec2<u32>(512u)) {
        return;
    }
    let point = vec3<f32>((f32(id.x) + 0.5) / size, (f32(id.y) + 0.5) / size, 0.0);
    var color = vec4<f32>(0.0);
    // Mid clouds.
    let mid = smoothstep(1.0, 1.4, weather_fbm(point + vec3<f32>(0.5), 8.0, 0.4, 2.0, 0.95, 4));
    color.g = mid;
    // Low clouds.
    let low = smoothstep(0.8, 1.4, weather_fbm(point, 16.0, 0.4, 2.0, 0.95, 4));
    color.r = clamp(low - color.g, 0.0, 1.0);
    // High clouds.
    color.b = smoothstep(-0.5, 0.5, perlin_fbm(point, vec3<f32>(6.0, 12.0, 1.0), 8));
    // Extra.
    color.a = smoothstep(-0.5, 0.5, perlin_fbm(point + vec3<f32>(-19.1, 33.4, 47.2), vec3<f32>(32.0), 4));
    textureStore(weather_out, id.xy, color);
}

fn turbulence_perlin(point: vec3<f32>) -> f32 {
    return perlin_fbm(point, vec3<f32>(12.0), 3);
}

fn perlin3d(point: vec3<f32>) -> vec3<f32> {
    let perlin1 = turbulence_perlin(point);
    let perlin2 = turbulence_perlin(point.yzx + vec3<f32>(-19.1, 33.4, 47.2));
    let perlin3 = turbulence_perlin(point.zxy + vec3<f32>(74.2, -124.5, 99.4));
    return vec3<f32>(perlin1, perlin2, perlin3);
}

fn curl(point: vec3<f32>) -> vec3<f32> {
    let delta = 0.1;
    let dx = vec3<f32>(delta, 0.0, 0.0);
    let dy = vec3<f32>(0.0, delta, 0.0);
    let dz = vec3<f32>(0.0, 0.0, delta);
    let px0 = perlin3d(point - dx);
    let px1 = perlin3d(point + dx);
    let py0 = perlin3d(point - dy);
    let py1 = perlin3d(point + dy);
    let pz0 = perlin3d(point - dz);
    let pz1 = perlin3d(point + dz);
    let x = py1.z - py0.z - pz1.y + pz0.y;
    let y = pz1.x - pz0.x - px1.z + px0.z;
    let z = px1.y - px0.y - py1.x + py0.x;
    let divisor = 1.0 / (2.0 * delta);
    return normalize(vec3<f32>(x, y, z) * divisor);
}

// `turbulence.frag`.
@compute @workgroup_size(8, 8, 1)
fn turbulence_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = 128.0;
    if any(id.xy >= vec2<u32>(128u)) {
        return;
    }
    let point = vec3<f32>((f32(id.x) + 0.5) / size, (f32(id.y) + 0.5) / size, 0.0);
    textureStore(turbulence_out, id.xy, vec4<f32>(0.5 * curl(point) + 0.5, 1.0));
}
