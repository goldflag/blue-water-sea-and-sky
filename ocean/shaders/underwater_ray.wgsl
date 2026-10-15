// The view ray through a screen point, as the web game's `ocean/screen/underwater.ts` builds it, shared by the probe and the
// composite; and the light the water itself scatters toward a viewer in it, shared by the composite (the deep around
// the camera) and the sea's underside (the deep it mirrors outside Snell's window).
#define_import_path naval_ocean::underwater_ray

// Water's refractive index: sunlight enters the sea bent toward the vertical.
const REFRACTION: f32 = 1.333;
// Forward-scattering asymmetry of the sunlit water (Henyey–Greenstein g).
const FORWARD: f32 = 0.55;
// Sunlight scattered forward, relative to the pigment.
const SUNLIT: f32 = 0.45;

// What the water scatters toward a viewer looking along `view` (world), per metre of absorption, just under the
// surface: the pigment (the water's own light, scaled for night by the scene) plus the sun's forward lobe once it is
// up, the sun refracted toward the vertical. `sun_color` is the celestial light's colour (any scale).
fn water_light(view: vec3<f32>, pigment: vec3<f32>, sun_direction: vec3<f32>, sun_color: vec3<f32>) -> vec3<f32> {
    let bent = vec3<f32>(sun_direction.x, 0.0, sun_direction.z) / REFRACTION;
    let sun_in_water = vec3<f32>(bent.x, sqrt(max(1.0 - dot(bent, bent), 0.0)), bent.z);
    let g2 = FORWARD * FORWARD;
    let lobe = (1.0 - g2) / pow(1.0 + g2 - dot(view, sun_in_water) * 2.0 * FORWARD, 1.5);
    let tint = sun_color / max(max(sun_color.r, max(sun_color.g, sun_color.b)), 1e-3);
    let daylight = smoothstep(-0.05, 0.25, sun_direction.y) * SUNLIT;
    return pigment * (tint * lobe * daylight + 1.0);
}

// Clear ocean water's scattering (per metre, nearly flat across the spectrum, Jerlov oceanic I–II), almost all of it
// through small angles: an image through the water loses it, the daylight going down keeps it.
const WATER_SCATTERING: f32 = 0.07;

// How fast the daylight fades with depth (the diffuse attenuation K_d) for water of beam attenuation `beam` (the maps'
// absorption swatch, which sets how far one sees): the beam less the scattering the light keeps, and at least a fifth
// of the beam. In blue oceanic water K_d is a fifth to a third of the beam's, in red nearly all of it.
fn diffuse_attenuation(beam: vec3<f32>) -> vec3<f32> {
    return max(beam - vec3<f32>(WATER_SCATTERING), beam * 0.2);
}

// The light the water scatters toward a viewer `depth` metres under the surface looking along `view` (world) over
// `distance` metres of water (any large distance for the open deep), in water of beam attenuation c (`beam`): the
// daylight at depth z falls as exp(−k·z) (k the diffuse attenuation); every metre scatters c of it toward the viewer,
// normalised so the open deep seen level just under the surface shows `water_light`; the beam keeps exp(−c·s) of it.
// Along the ray z = depth − s·view.y, so the integral over [0, d] is c·exp(−k·depth)·(1 − exp(−(c − k·view.y)·d)) /
// (c − k·view.y).
fn water_path_light(
    view: vec3<f32>,
    depth: f32,
    distance: f32,
    pigment: vec3<f32>,
    beam: vec3<f32>,
    sun_direction: vec3<f32>,
    sun_color: vec3<f32>,
) -> vec3<f32> {
    let diffuse = diffuse_attenuation(beam);
    let rate = max(beam - diffuse * view.y, vec3<f32>(1e-4));
    let optical = max(rate * distance, vec3<f32>(1e-4));
    let path = (1.0 - exp(-optical)) / optical * beam * distance;
    return water_light(view, pigment, sun_direction, sun_color) * exp(-diffuse * max(depth, 0.0)) * path;
}

// The view ray through screen point `uv` (0–1, y down) in view space, and its world direction and near-plane point.
struct ScreenRay {
    view: vec3<f32>,
    world: vec3<f32>,
    start: vec3<f32>,
    to_near: f32,
}

fn screen_ray(uv: vec2<f32>, inverse_projection: mat4x4<f32>, camera_world: mat4x4<f32>, near: f32) -> ScreenRay {
    // Any NDC depth inside the frustum lies on this pixel's ray, whichever depth convention is active.
    let on_ray = inverse_projection * vec4<f32>(uv.x * 2.0 - 1.0, uv.y * -2.0 + 1.0, 0.5, 1.0);
    let view_ray = normalize(on_ray.xyz / on_ray.w);
    let world = normalize((camera_world * vec4<f32>(view_ray, 0.0)).xyz);
    let to_near = near / -view_ray.z;
    let start = (camera_world * vec4<f32>(view_ray * to_near, 1.0)).xyz;
    return ScreenRay(view_ray, world, start, to_near);
}

