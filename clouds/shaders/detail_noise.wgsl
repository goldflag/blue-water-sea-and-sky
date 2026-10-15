// The clouds' detail noise, filled by compute once at startup: one 128³ four-channel volume that tiles, holding the
// structure from a few tens of metres down to about a metre that `naval_clouds::field::sample_media` carves the
// clouds' edges with. r and g: low- and high-frequency wisps; b and a: low- and high-frequency billows.
//
// Every channel is layered "alligator" noise, the crate's own after the published description of the kind (a cellular
// noise: each lattice cell holds one bump of a random height at a random place, and the noise is the tallest bump at a
// point less the second tallest, so bumps stand rounded with creases between them; Schneider, "Nubis³", SIGGRAPH
// 2023, names it for cloud billows where inverted Worley noise reads as packed spheres). The wisps' lookups are
// displaced by curl noise first, which swirls the creases into filaments. Stored bumps bright, creases dark: the
// field carves the bumps away for wisps (the web of creases stays) and the creases away for billows (the bumps stay).
//
// The lattice is hashed with integers (`hash_u32`), so every GPU places the same bumps: the texels differ between
// GPUs by float rounding alone, under a level (`naval_clouds::detail_noise::reference_texel` is the same on the CPU,
// and a test holds the two together). No asset is shipped for it.

@group(0) @binding(0) var detail_out: texture_storage_3d<rgba8unorm, write>;
// The mip above the one `mip_main` writes.
@group(0) @binding(1) var mip_source: texture_3d<f32>;

// Texels per edge.
const SIZE: f32 = 128.0;

// A 32-bit integer hash (Chris Wellons's "lowbias32", public domain).
fn hash_u32(value: u32) -> u32 {
    var h = value;
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return h;
}

fn hash_cell(cell: vec3<u32>, seed: u32) -> u32 {
    return hash_u32(cell.x + hash_u32(cell.y + hash_u32(cell.z + seed)));
}

// A lattice coordinate wrapped into [0, cells): the noise tiles.
fn wrap_cell(cell: vec3<i32>, cells: i32) -> vec3<u32> {
    return vec3<u32>((cell % cells + cells) % cells);
}

// One octave of alligator noise at `point` (tile units, [0, 1)) over a lattice of `cells` per tile edge: in [0, 1).
fn alligator(point: vec3<f32>, cells: i32, seed: u32) -> f32 {
    let scaled = point * f32(cells);
    let base = floor(scaled);
    let within = scaled - base;
    let origin = vec3<i32>(base);
    var first = 0.0;
    var second = 0.0;
    for (var z = -1; z <= 1; z++) {
        for (var y = -1; y <= 1; y++) {
            for (var x = -1; x <= 1; x++) {
                let offset = vec3<i32>(x, y, z);
                let h = hash_cell(wrap_cell(origin + offset, cells), seed);
                // The bump's place in its cell (ten bits an axis) and its height, 0.5 to 1.
                let place = vec3<f32>(vec3<u32>(h, h >> 10u, h >> 20u) & vec3<u32>(1023u)) * (1.0 / 1024.0);
                let height = 0.5 + 0.5 * f32(hash_u32(h) >> 8u) * (1.0 / 16777216.0);
                let reach = clamp(1.0 - distance(vec3<f32>(offset) + place, within), 0.0, 1.0);
                let bump = reach * reach * (3.0 - 2.0 * reach) * height;
                if bump > first {
                    second = first;
                    first = bump;
                } else {
                    second = max(second, bump);
                }
            }
        }
    }
    return first - second;
}

// Three octaves, each twice the cells at half the weight, normalised.
fn alligator_layers(point: vec3<f32>, cells: i32, seed: u32) -> f32 {
    var total = 0.0;
    var weight = 1.0;
    for (var octave = 0u; octave < 3u; octave++) {
        total += weight * alligator(point, cells << octave, seed + 17u * octave);
        weight *= 0.5;
    }
    return total / 1.75;
}

// The gradient (per tile unit) of periodic value noise with a quintic fade.
fn value_gradient(point: vec3<f32>, cells: i32, seed: u32) -> vec3<f32> {
    let scaled = point * f32(cells);
    let base = floor(scaled);
    let f = scaled - base;
    let origin = vec3<i32>(base);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let du = 30.0 * f * f * (f * (f - 2.0) + 1.0);
    var corners: array<f32, 8>;
    for (var i = 0; i < 8; i++) {
        let offset = vec3<i32>(i & 1, (i >> 1) & 1, (i >> 2) & 1);
        corners[i] = f32(hash_cell(wrap_cell(origin + offset, cells), seed) >> 8u) * (1.0 / 16777216.0);
    }
    let a = corners[0];
    let b = corners[1];
    let c = corners[2];
    let d = corners[3];
    let e = corners[4];
    let g = corners[5];
    let h = corners[6];
    let k = corners[7];
    let k1 = b - a;
    let k2 = c - a;
    let k3 = e - a;
    let k4 = a - b - c + d;
    let k5 = a - c - e + h;
    let k6 = a - b - e + g;
    let k7 = -a + b + c - d + e - g - h + k;
    return f32(cells) * du * vec3<f32>(
        k1 + k4 * u.y + k6 * u.z + k7 * u.y * u.z,
        k2 + k5 * u.z + k4 * u.x + k7 * u.z * u.x,
        k3 + k6 * u.x + k5 * u.y + k7 * u.x * u.y,
    );
}

// Curl noise: the curl of a vector potential of three value noises, divergence-free, about 1 in size.
fn curl(point: vec3<f32>, cells: i32, seed: u32) -> vec3<f32> {
    let a = value_gradient(point, cells, seed);
    let b = value_gradient(point, cells, seed + 101u);
    let c = value_gradient(point, cells, seed + 202u);
    return vec3<f32>(c.y - b.z, a.z - c.x, b.x - a.y) / f32(cells);
}

// The four channels at a point of the tile. The scales bring each channel's 99th percentile to about 0.9.
fn detail_channels(point: vec3<f32>) -> vec4<f32> {
    let swirl_low = curl(point, 3, 900u) * 0.045 + curl(point, 7, 950u) * 0.018;
    let swirl_high = curl(point, 5, 990u) * 0.02 + curl(point, 13, 995u) * 0.008;
    return clamp(
        vec4<f32>(
            alligator_layers(fract(point + swirl_low), 4, 100u) * 1.55,
            alligator_layers(fract(point + swirl_high), 12, 200u) * 1.75,
            alligator_layers(point, 5, 300u) * 2.1,
            alligator_layers(point, 14, 400u) * 1.75,
        ),
        vec4<f32>(0.0),
        vec4<f32>(1.0),
    );
}

@compute @workgroup_size(4, 4, 4)
fn detail_main(@builtin(global_invocation_id) id: vec3<u32>) {
    textureStore(detail_out, id, detail_channels((vec3<f32>(id) + 0.5) / SIZE));
}

// A mip of the volume: each texel the mean of the eight under it.
@compute @workgroup_size(4, 4, 4)
fn mip_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id >= textureDimensions(detail_out)) {
        return;
    }
    var sum = vec4<f32>(0.0);
    for (var i = 0u; i < 8u; i++) {
        sum += textureLoad(mip_source, id * 2u + vec3<u32>(i & 1u, (i >> 1u) & 1u, (i >> 2u) & 1u), 0);
    }
    textureStore(detail_out, id, sum * 0.125);
}
