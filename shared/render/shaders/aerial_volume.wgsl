// The aerial-perspective volume's layout and its read, binding-free (naval_render::aerial): the screen in columns, each
// column's ray in `AERIAL_SLICES` slices of radial distance from the eye, slice k at AERIAL_NEAR × (FAR / NEAR)^(k / 31).
// One 3D texture holds the air's in-scattered radiance (rgb) up to each cell's distance in its first `AERIAL_SLICES`
// layers and its opacity (rgb, 1 − transmittance) in the next: zeros are clear air. The renderer's group 0 binds it for
// materials (`naval::aerial`); the sky reads a buffer copy of it (`naval_sky::aerial`). `naval_render::aerial` holds
// the same numbers.
#define_import_path naval::aerial_volume

const AERIAL_SLICES: f32 = 32.0;
const AERIAL_NEAR: f32 = 20.0;
const AERIAL_FAR: f32 = 450000.0;

// What the air does between the eye and a point: seen through it, a surface's radiance becomes
// `radiance × transmittance + inscatter`.
struct AerialSample {
    inscatter: vec3<f32>,
    transmittance: vec3<f32>,
}

// The radial distance (m) of slice `k` (fractional between slices).
fn aerial_slice_distance(k: f32) -> f32 {
    return AERIAL_NEAR * pow(AERIAL_FAR / AERIAL_NEAR, k / (AERIAL_SLICES - 1.0));
}

// The fractional slice of a radial distance (m): 0 at AERIAL_NEAR or nearer, AERIAL_SLICES - 1 at AERIAL_FAR or
// beyond.
fn aerial_slice_of(distance: f32) -> f32 {
    let k = log(max(distance, AERIAL_NEAR) / AERIAL_NEAR) / log(AERIAL_FAR / AERIAL_NEAR) * (AERIAL_SLICES - 1.0);
    return clamp(k, 0.0, AERIAL_SLICES - 1.0);
}

// Nearer than the first slice the air is that slice's share of it: none at the eye.
fn aerial_near_share(distance: f32) -> f32 {
    return clamp(distance / AERIAL_NEAR, 0.0, 1.0);
}

// The volume at screen `uv` (0–1, y down) and radial `distance` (m), trilinear within each half.
fn aerial_sample(volume: texture_3d<f32>, volume_sampler: sampler, uv: vec2<f32>, distance: f32) -> AerialSample {
    let at = clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0));
    let k = aerial_slice_of(distance);
    let layers = 2.0 * AERIAL_SLICES;
    let share = aerial_near_share(distance);
    var air: AerialSample;
    air.inscatter = textureSampleLevel(volume, volume_sampler, vec3<f32>(at, (k + 0.5) / layers), 0.0).rgb * share;
    let opacity = textureSampleLevel(volume, volume_sampler, vec3<f32>(at, (AERIAL_SLICES + k + 0.5) / layers), 0.0).rgb;
    air.transmittance = 1.0 - opacity * share;
    return air;
}
