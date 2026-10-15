// Stand-in for the sky lane's environment prefilter (its port of three's PMREMGenerator, which fills the atlas
// the web game's `sky/environment/Environment.ts` makes): fills one level of a PMREM (CubeUV) atlas from an equirectangular
// radiance map, Gaussian-filtered at the level's total sigma in one pass (three reaches the same sigma through a chain
// of separable blurs). The texel → direction mapping inverts `cube_uv_face_uv` and `cube_uv_texel` of
// naval_sky::environment, border texels included. Replaced by naval-sky's own bake in the vertical slice.

struct FillParams {
    // Top-left texel of the level's 3 × 2 block of faces, and its face size.
    origin: vec2<u32>,
    size: u32,
    samples: u32,
    // Total Gaussian blur (radians).
    sigma: f32,
}

@group(0) @binding(0) var equirect: texture_2d<f32>;
@group(0) @binding(1) var equirect_sampler: sampler;
@group(0) @binding(2) var<uniform> params: FillParams;
@group(0) @binding(3) var atlas: texture_storage_2d<rgba16float, write>;

const PI: f32 = 3.141592653589793;
const GOLDEN_ANGLE: f32 = 2.399963229728653;

// The equirectangular map along a world direction: rows from the nadir up, azimuth from +X toward +Z.
fn radiance(direction: vec3<f32>) -> vec3<f32> {
    let d = normalize(direction);
    let u = atan2(d.z, d.x) / (2.0 * PI) + 0.5;
    let v = asin(clamp(d.y, -1.0, 1.0)) / PI + 0.5;
    return textureSampleLevel(equirect, equirect_sampler, vec2<f32>(u, v), 0.0).rgb;
}

// Face-local coordinates (a, b) in [−1, 1] back to a direction: the inverse of `cube_uv_face_uv`.
fn face_direction(face: u32, a: f32, b: f32) -> vec3<f32> {
    switch face {
        case 0u: { return vec3<f32>(1.0, b, a); }
        case 1u: { return vec3<f32>(-a, 1.0, -b); }
        case 2u: { return vec3<f32>(-a, b, 1.0); }
        case 3u: { return vec3<f32>(-1.0, b, -a); }
        case 4u: { return vec3<f32>(-a, -1.0, b); }
        default: { return vec3<f32>(a, b, -1.0); }
    }
}

@compute @workgroup_size(8, 8, 1)
fn fill(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = params.size;
    if id.x >= size * 3u || id.y >= size * 2u {
        return;
    }
    let column = id.x / size;
    let row = id.y / size;
    let face = column + 3u * row;
    let local = vec2<f32>(f32(id.x - column * size), f32(id.y - row * size)) + 0.5;
    let uv = (local - 1.0) / (f32(size) - 2.0);
    let direction = normalize(face_direction(face, uv.x * 2.0 - 1.0, uv.y * 2.0 - 1.0));
    var color = vec3<f32>(0.0);
    if params.samples <= 1u || params.sigma <= 0.0 {
        color = radiance(direction);
    } else {
        // A 2D Gaussian of angular deviation sigma about the direction, sampled on a Fibonacci spiral.
        let helper = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(direction.y) > 0.9);
        let tangent = normalize(cross(helper, direction));
        let bitangent = cross(direction, tangent);
        let count = params.samples;
        for (var k = 0u; k < count; k++) {
            let u = (f32(k) + 0.5) / f32(count);
            let angle = min(params.sigma * sqrt(-2.0 * log(1.0 - u)), PI);
            let phi = f32(k) * GOLDEN_ANGLE;
            let offset = tangent * cos(phi) + bitangent * sin(phi);
            color += radiance(direction * cos(angle) + offset * sin(angle));
        }
        color /= f32(count);
    }
    // Alpha: the clear sky's luminance, as naval-sky's bake writes it (this stand-in has no clouds).
    textureStore(atlas, vec2<u32>(params.origin) + id.xy, vec4<f32>(color, dot(color, vec3<f32>(0.2126, 0.7152, 0.0722))));
}
