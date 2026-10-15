// The sky backdrop: sky radiance plus the celestial bodies through the air, on one full-screen triangle at
// the far plane. Reversed depth clears to 0, so the dome sits at 0 and, tested `GreaterEqual` without
// writing depth, draws only where nothing else has. Ports `createDome` and `viewDirection` in
// the web game's `sky/dome.ts`, and the dome colour `sky/Sky.ts` composes; the sky itself is takram's atmosphere
// with the game's grade (`naval_sky::atmosphere::dome_sky`). With `CLOUDS` defined the clouds (`naval-clouds`)
// supply `naval_clouds::hooks` and their bind group at group 2, and the dome takes the clouds' light shafts at each
// pixel (`clouds_shadow_length`: km of the view ray in the clouds' shadow), as takram's sky reads its shadow-length
// buffer. The web's `dither` (±half an 8-bit level, interleaved gradient noise on the dome's radiance) is gone: the
// renderer dithers whatever it writes to an 8-bit output, the whole frame alike (naval-render's `naval::output`).

#import naval_sky::common::{sky, air, PI}
#import naval_sky::atmosphere::{dome_sky, transmittance_to_space, aerosol_phase}
#import naval_sky::bruneton::rayleigh_phase_function
#import naval_sky::celestial::{celestial_radiance}
#import naval_sky::aerial::aerial_over_sky
#ifdef CLOUDS
#import naval_clouds::hooks::{clouds_shadow_length}
#endif

struct DomeVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) view_ray: vec3<f32>,
}

// The world direction through a clip-space corner: the view ray is linear across the screen.
fn corner_ray(clip: vec2<f32>) -> vec3<f32> {
    let point = sky.inverse_projection * vec4<f32>(clip, 0.5, 1.0);
    return (sky.view_to_world * vec4<f32>(point.xyz / point.w, 0.0)).xyz;
}

@vertex
fn dome_vertex(@builtin(vertex_index) index: u32) -> DomeVertex {
    // One triangle covering the viewport: clip corners (−1, −1), (3, −1), (−1, 3).
    let clip = vec2<f32>(select(-1.0, 3.0, index == 1u), select(-1.0, 3.0, index == 2u));
    var out: DomeVertex;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.view_ray = corner_ray(clip);
    return out;
}

// The dome's radiance along a world direction, from the camera, with `shadow_length` km of light shafts.
fn dome_radiance(direction: vec3<f32>, shadow_length: f32) -> vec3<f32> {
    return dome_sky(direction, false, shadow_length) + transmittance_to_space(direction, false) * celestial_radiance(direction);
}

// Lightning in the air: the flash's colour (the stroke's light on meshes), the channel's core (m) inside which its
// falloff stops, and the scale heights (m) of the molecules and the aerosol (`model::RAYLEIGH_HEIGHT`, `MIE_HEIGHT`).
const FLASH_TINT: vec3<f32> = vec3<f32>(0.8, 0.87, 1.0);
const FLASH_CORE: f32 = 60.0;
const RAYLEIGH_HEIGHT_M: f32 = 8000.0;
const MIE_HEIGHT_M: f32 = 1200.0;

// A lightning flash in the air along a world direction from the camera: the air's single scattering of the channel's
// light (a point of the sky's `lightning_intensity` at `lightning_position`, an irradiance of `intensity × (1 km / r)²`
// in the sea's units), its inverse square integrated along the ray in closed form, the air as dense as where the ray
// passes the flash and the phases at the angle the flash makes with the ray from the eye (the aureole about a flash seen
// through haze), through the air between the eye and the flash.
fn flash_airlight(direction: vec3<f32>) -> vec3<f32> {
    let to_flash = sky.lightning_position - sky.camera_position;
    let reach = max(length(to_flash), 1.0);
    let along = dot(to_flash, direction);
    let offset = max(length(to_flash - direction * along), FLASH_CORE);
    // ∫ dt / r² from the eye along the ray (1/m).
    let path = (0.5 * PI + atan(along / offset)) / offset;
    let nu = dot(to_flash / reach, direction);
    let height = max(sky.camera_position.y + direction.y * max(along, 0.0), 0.0);
    // The air's coefficients are per kilometre.
    let scattering = air.rayleigh_scattering * (exp(-height / RAYLEIGH_HEIGHT_M) * rayleigh_phase_function(nu))
        + air.mie_scattering * (exp(-height / MIE_HEIGHT_M) * aerosol_phase(nu));
    let depth = (air.rayleigh_scattering + air.mie_extinction + air.absorption_extinction) * (reach * 0.001);
    return FLASH_TINT * (sky.lightning_intensity * 1e6 * path * 0.001) * scattering * exp(-depth);
}

@fragment
fn dome_fragment(in: DomeVertex) -> @location(0) vec4<f32> {
    let direction = normalize(in.view_ray);
    var shadow_length = 0.0;
#ifdef CLOUDS
    shadow_length = clouds_shadow_length(in.position.xy);
#endif
    // The weather's haze in front of the sky along this pixel's ray (the aerial-perspective volume's dome slice), so
    // the dome's horizon is the hazed sea's.
    var radiance = aerial_over_sky(dome_radiance(direction, shadow_length), in.position.xy * sky.viewport.zw);
    // A flash lights the air near it, short of the haze's far end (its own reach dims it), so it adds over the haze.
    if sky.lightning_intensity > 0.0 {
        radiance += flash_airlight(direction);
    }
    return vec4<f32>(radiance, 1.0);
}
