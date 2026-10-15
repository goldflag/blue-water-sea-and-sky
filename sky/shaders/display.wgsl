// A display transform for sky captures: the game's look
// ahead of AgX (`displayGrade` in the web game's `DisplayTransform.ts`: exposure 0.9, contrast 1.25 about middle
// grey, saturation 1 since 2026-10-04), then three r185's `agxToneMapping` (`src/nodes/display/ToneMappingFunctions.js`).
// Writes linear values to an sRGB target, whose hardware encoding stands in for three's sRGB output. No bloom.

struct Display {
    // The sky's exposure times any eye adaptation (1 neutral).
    exposure: f32,
    // 1 applies the game's look ahead of AgX; 0 when the input is already graded.
    grade: f32,
    padding_b: f32,
    padding_c: f32,
}

@group(0) @binding(0) var radiance_map: texture_2d<f32>;
@group(0) @binding(1) var<uniform> display: Display;

const LOOK_EXPOSURE: f32 = 0.9;
const LOOK_CONTRAST: f32 = 1.25;
const LOOK_SATURATION: f32 = 1.0;
const MIDDLE_GRAY: f32 = 0.18;

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn display_grade(radiance: vec3<f32>, exposure: f32) -> vec3<f32> {
    let exposed = radiance * (exposure * LOOK_EXPOSURE);
    let lum = max(luminance(exposed), 1e-5);
    return max(mix(vec3<f32>(lum), exposed, LOOK_SATURATION), vec3<f32>(0.0)) * pow(lum / MIDDLE_GRAY, LOOK_CONTRAST - 1.0);
}

fn agx_contrast(x: vec3<f32>) -> vec3<f32> {
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x - 0.00232;
}

fn agx(color: vec3<f32>) -> vec3<f32> {
    let srgb_to_rec2020 = mat3x3<f32>(
        vec3<f32>(0.6274, 0.0691, 0.0164), vec3<f32>(0.3293, 0.9195, 0.0880), vec3<f32>(0.0433, 0.0113, 0.8956));
    let rec2020_to_srgb = mat3x3<f32>(
        vec3<f32>(1.6605, -0.1246, -0.0182), vec3<f32>(-0.5876, 1.1329, -0.1006), vec3<f32>(-0.0728, -0.0083, 1.1187));
    let inset = mat3x3<f32>(
        vec3<f32>(0.856627153315983, 0.137318972929847, 0.11189821299995),
        vec3<f32>(0.0951212405381588, 0.761241990602591, 0.0767994186031903),
        vec3<f32>(0.0482516061458583, 0.101439036467562, 0.811302368396859));
    let outset = mat3x3<f32>(
        vec3<f32>(1.1271005818144368, -0.1413297634984383, -0.14132976349843826),
        vec3<f32>(-0.11060664309660323, 1.157823702216272, -0.11060664309660294),
        vec3<f32>(-0.016493938717834573, -0.016493938717834257, 1.2519364065950405));
    let min_ev = -12.47393;
    let max_ev = 4.026069;
    var c = srgb_to_rec2020 * color;
    c = inset * c;
    c = max(c, vec3<f32>(1e-10));
    c = log2(c);
    c = clamp((c - min_ev) / (max_ev - min_ev), vec3<f32>(0.0), vec3<f32>(1.0));
    c = agx_contrast(c);
    c = outset * c;
    c = pow(max(vec3<f32>(0.0), c), vec3<f32>(2.2));
    c = rec2020_to_srgb * c;
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

@vertex
fn display_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let clip = vec2<f32>(select(-1.0, 3.0, index == 1u), select(-1.0, 3.0, index == 2u));
    return vec4<f32>(clip, 0.0, 1.0);
}

@fragment
fn display_fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let radiance = textureLoad(radiance_map, vec2<i32>(position.xy), 0).rgb;
    let graded = select(radiance, display_grade(radiance, display.exposure), display.grade > 0.5);
    return vec4<f32>(agx(graded), 1.0);
}
