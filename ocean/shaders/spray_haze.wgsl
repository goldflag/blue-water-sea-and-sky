// The spray a gale tears off the crests hangs low over the sea as a haze (native only). Its drops thin out with height
// above the waves, exponentially over a scale height that grows with the sea, so a view across the water sees through a
// long path of it and a view from above through a short one. Large drops scatter almost every photon they meet (albedo
// 1), most of them forward (Henyey–Greenstein, g 0.75): the haze takes the sky's light from above and the sea's from
// below, and glows toward a low sun. `naval_ocean::spindrift` sets its strength from the wind (`spray_haze` in
// spindrift.rs).
#define_import_path naval_ocean::spray_haze

const PI: f32 = 3.141592653589793;
// Forward scattering of spume drops (tens to hundreds of micrometres).
const SPRAY_ASYMMETRY: f32 = 0.75;
// The share of the sky's mean radiance that reaches a drop from every side: the dome above it and the darker sea below.
const SPRAY_SKY_SHARE: f32 = 0.55;

// The haze's relative density integrated from its base up to height `y` above it (m), scale height `scale`: the drops
// fill the troughs below the base evenly.
fn spray_column(y: f32, scale: f32) -> f32 {
    return select(y, scale * (1.0 - exp(-y / scale)), y > 0.0);
}

// Optical depth of the spray between `eye` and `point` (world m). `haze`: x extinction at the base (1/m), y scale height
// (m), z the base's height (m); no haze below an extinction of 0.
fn spray_depth(eye: vec3<f32>, point: vec3<f32>, haze: vec4<f32>) -> f32 {
    if haze.x <= 0.0 {
        return 0.0;
    }
    let a = eye.y - haze.z;
    let b = point.y - haze.z;
    let span = distance(eye, point);
    let rise = b - a;
    if abs(rise) < 1e-3 {
        return haze.x * span * select(1.0, exp(-a / haze.y), a > 0.0);
    }
    return haze.x * span * (spray_column(b, haze.y) - spray_column(a, haze.y)) / rise;
}

// Radiance the haze scatters toward a viewer along `view` (unit, from the haze toward the eye): `sky` the sky's mean
// radiance from above (the environment's roughest level about +Y), `sun` the beam's irradiance on a surface facing it
// where the haze lies (cloud shadow included), `sun_direction` toward the sun.
fn spray_light(sky: vec3<f32>, sun: vec3<f32>, view: vec3<f32>, sun_direction: vec3<f32>) -> vec3<f32> {
    let g = SPRAY_ASYMMETRY;
    // The angle between the beam's travel and the light leaving toward the eye: forward scattering looks into the sun.
    let cosine = dot(-sun_direction, view);
    let phase = (1.0 - g * g) / (4.0 * PI * pow(1.0 + g * g - 2.0 * g * cosine, 1.5));
    return sky * SPRAY_SKY_SHARE + sun * phase;
}
