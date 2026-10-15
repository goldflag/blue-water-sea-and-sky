// The Milky Way, rendered once into an equal-area band in galactic coordinates (longitude across,
// sin(latitude) down, 2048 × 512, sqrt(radiance / 2) in eight bits), and the box filter that builds its mips.
// Ports `milkyWay()` and `MilkyWay.bake` in the web game's `sky/celestial/milkyWay.ts`.

#import naval_sky::noise::{mx_noise_float3, mx_noise_vec3, mx_fractal_noise_float}

const PI: f32 = 3.141592653589793;
const DEGREES: f32 = 0.017453292519943295;
const ENCODING: f32 = 2.0;

@group(0) @binding(0) var band: texture_storage_2d<rgba8unorm, write>;

// A soft blob in (l, b) degrees, `size` its σ in degrees, stretched by `aspect` along `tilt` (degrees).
fn blob(l: f32, b: f32, centre: vec2<f32>, size: f32, aspect: f32, tilt: f32) -> f32 {
    let c = cos(tilt * DEGREES);
    let s = sin(tilt * DEGREES);
    let dl = l / DEGREES - centre.x;
    let db = b / DEGREES - centre.y;
    let along = (dl * c + db * s) / aspect;
    let across = db * c - dl * s;
    return exp((along * along + across * across) / (-2.0 * size * size));
}

fn ridges(p: vec3<f32>, sharpness: f32) -> f32 {
    return pow(max(1.0 - abs(mx_noise_float3(p)), 0.0), sharpness);
}

fn lane(b_degrees: f32, centre: f32, width: f32, strength: f32) -> f32 {
    let t = (b_degrees - centre) / width;
    return exp(-(t * t)) * strength;
}

// The Milky Way's radiance (relative: about 1 in the bright bulge) toward a direction in the galactic frame.
fn milky_way(g: vec3<f32>) -> vec3<f32> {
    let l = atan2(g.y, g.x);
    let b = asin(clamp(g.z, -1.0, 1.0));
    let height = abs(b);
    let l_degrees = l / DEGREES;
    let b_degrees = b / DEGREES;
    let toward = pow(cos(l) * 0.5 + 0.5, 1.6);
    let thickness = (toward * 4.2 + 2.4) * DEGREES;
    let disc = (toward * 0.62 + 0.14) * exp(-(height / thickness));
    let thick = (toward * 0.7 + 0.3) * 0.07 * exp(-(height / (15.0 * DEGREES)));
    let bulge = blob(l, b, vec2<f32>(0.0, -1.0), 5.5, 1.45, 0.0) * 1.05 + blob(l, b, vec2<f32>(0.0, -1.0), 12.0, 1.3, 0.0) * 0.3;
    let star_clouds = blob(l, b, vec2<f32>(3.0, -4.5), 3.2, 1.3, 0.0) * 0.55
        + blob(l, b, vec2<f32>(27.0, -2.0), 2.6, 1.5, 0.0) * 0.4
        + blob(l, b, vec2<f32>(75.0, 1.0), 5.0, 2.2, 0.0) * 0.28
        + blob(l, b, vec2<f32>(-73.0, -1.0), 4.0, 2.0, 0.0) * 0.3
        + blob(l, b, vec2<f32>(-30.0, -1.0), 4.0, 2.5, 0.0) * 0.22;
    let lumps = smoothstep(-0.5, 0.8, mx_fractal_noise_float(g * 5.5, 5, 2.1, 0.55)) * 0.8 + 0.5;
    let fine = mx_fractal_noise_float(g * 30.0 + 7.3, 4, 2.2, 0.55) * 0.3 + 1.0;
    let grain = mx_noise_float3(g * 430.0) * 0.25 + 1.0;
    let light = ((disc * lumps + star_clouds * (lumps * 0.5 + 0.6)) * fine + bulge * (lumps * 0.25 + 0.8)) * grain + thick;
    let warped = g + mx_noise_vec3(g * 3.3) * 0.065 + mx_noise_vec3(g * 9.1 + 3.0) * 0.018;
    let along = warped * vec3<f32>(1.0, 1.0, 4.0);
    let filaments = ridges(along * 10.0, 4.0) * 0.6 + ridges(along * 21.0 + 1.9, 4.0) * 0.4;
    let mottled = smoothstep(-0.2, 0.7, mx_fractal_noise_float(along * 7.0 + 2.7, 4, 2.1, 0.55));
    let wander = mx_fractal_noise_float(vec3<f32>(l * 2.2, 3.1, 0.0), 3, 2.0, 0.5) * 1.3;
    let rift = lane(b_degrees, wander + smoothstep(80.0, 10.0, l_degrees) * -1.4 + 2.0, toward * 1.6 + 1.4,
        smoothstep(92.0, 78.0, l_degrees) * smoothstep(-22.0, -4.0, l_degrees) * 2.1);
    let central = lane(b_degrees, wander * 0.4 - 0.6, 1.1, smoothstep(-30.0, -10.0, l_degrees) * smoothstep(20.0, 8.0, l_degrees) * 1.6);
    let southern = lane(b_degrees, wander - 0.4, 1.4, smoothstep(-160.0, -120.0, l_degrees) * smoothstep(-20.0, -40.0, l_degrees) * 1.1);
    let breakup = smoothstep(-0.35, 0.45, mx_fractal_noise_float(vec3<f32>(l * 3.4, b_degrees * 0.05, 7.7), 3, 2.0, 0.5)) * 0.75 + 0.25;
    let lanes = (rift + central + southern) * (filaments * 0.8 + 0.35) * breakup;
    let clouds = blob(l, b, vec2<f32>(-57.0, -0.6), 1.9, 1.0, 0.0) * 1.8
        + blob(l, b, vec2<f32>(3.0, 6.5), 2.0, 3.4, 58.0) * 1.5
        + blob(l, b, vec2<f32>(-4.0, 16.0), 2.4, 2.4, 38.0) * 1.0
        + blob(l, b, vec2<f32>(173.0, -15.0), 4.5, 1.8, -25.0) * 0.8;
    let patches = mottled * filaments * 0.9 * exp(-(height / (7.0 * DEGREES))) * (toward * 0.7 + 0.3);
    let depth = lanes + clouds * (filaments * 0.7 + 0.5) + patches;
    let transmit = exp(-(vec3<f32>(1.0, 1.25, 1.6) * depth));
    let warm = clamp(max(bulge * 1.5, toward * toward * 0.5), 0.0, 1.0);
    let tint = mix(vec3<f32>(0.82, 0.89, 1.0), vec3<f32>(1.0, 0.82, 0.6), warm);
    let nebulae = blob(l, b, vec2<f32>(6.0, -1.2), 0.8, 1.0, 0.0) + blob(l, b, vec2<f32>(-72.4, -0.6), 1.1, 1.0, 0.0)
        + blob(l, b, vec2<f32>(85.0, -1.0), 1.5, 1.0, 0.0) * 0.7 + blob(l, b, vec2<f32>(-151.0, -19.0), 3.0, 1.6, 40.0) * 0.5;
    return light * tint * transmit + vec3<f32>(1.0, 0.3, 0.4) * nebulae * 0.06;
}

@compute @workgroup_size(8, 8)
fn bake_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(band);
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    let longitude = (uv.x - 0.5) * (2.0 * PI);
    let sine = uv.y * 2.0 - 1.0;
    let ring = sqrt(max(1.0 - sine * sine, 0.0));
    let radiance = milky_way(vec3<f32>(ring * cos(longitude), ring * sin(longitude), sine));
    textureStore(band, vec2<i32>(id.xy), vec4<f32>(sqrt(clamp(radiance / ENCODING, vec3<f32>(0.0), vec3<f32>(1.0))), 1.0));
}

@group(0) @binding(1) var finer: texture_2d<f32>;

// One mip from the level above: the mean of each 2 × 2 block, as a linear blit at the block's centre is.
@compute @workgroup_size(8, 8)
fn downsample_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(band);
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let source = vec2<i32>(textureDimensions(finer)) - 1;
    let p = vec2<i32>(id.xy) * 2;
    let a = textureLoad(finer, min(p, source), 0);
    let b = textureLoad(finer, min(p + vec2<i32>(1, 0), source), 0);
    let c = textureLoad(finer, min(p + vec2<i32>(0, 1), source), 0);
    let d = textureLoad(finer, min(p + vec2<i32>(1, 1), source), 0);
    textureStore(band, vec2<i32>(id.xy), (a + b + c + d) * 0.25);
}
