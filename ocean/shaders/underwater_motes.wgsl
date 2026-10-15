// Motes in the water about a submerged camera (native only): open water gives the eye nothing to tell its own motion
// by, so a boat's speed and turn cannot be read from under the sea. A sparse cloud of the small suspended matter every
// sea carries stands still in the world within a few metres of the lens; as the camera moves it streams past. Each is a
// dot a pixel or two across, lit as the water about it, hidden by whatever stands nearer (the scene's depth), and drawn
// only where its pixel's ray starts under the waves (the underwater view's probe). No particle lives on the CPU: a mote
// is an instance index hashed to a point of a cube that repeats over the world.

#import naval::camera::{camera, world_to_clip}
#import naval_ocean::underwater_ray::{water_light, diffuse_attenuation}
#import naval_ocean::water_properties::{WaterField, sample_water, water_beam, water_pigment}

// `Underwater` in underwater.wgsl.
struct Underwater {
    pigment: vec4<f32>,
    absorption: vec4<f32>,
    transmission: vec4<f32>,
    sun_direction: vec4<f32>,
    sun_color: vec4<f32>,
    water: WaterField,
}

@group(1) @binding(0) var<uniform> water: Underwater;
@group(1) @binding(2) var depth_probe: texture_2d<f32>;
@group(1) @binding(3) var probe_sampler: sampler;

// The cube the motes repeat over (m): `naval_ocean::underwater::MOTES` of them in it.
const CUBE: f32 = 18.0;
// A mote's radius (m), and the smallest and largest it is drawn (pixels).
const RADIUS: f32 = 0.012;
const SMALLEST: f32 = 1.2;
const LARGEST: f32 = 3.0;
// Ranges (m) over which a mote fades in before the lens and out into the water.
const NEAR: vec2<f32> = vec2<f32>(0.7, 1.8);
const FAR: vec2<f32> = vec2<f32>(6.0, 9.0);
// A mote's light, as a multiple of what the open water shows along its ray, and its opacity at its centre.
const LIGHT: f32 = 2.5;
const OPACITY: f32 = 0.6;

struct MoteOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) offset: vec2<f32>,
    // rgb the mote's light, a its opacity.
    @location(1) light: vec4<f32>,
}

// PCG, three draws in [0, 1).
fn hash3(index: u32) -> vec3<f32> {
    var state = index * 747796405u + 2891336453u;
    var out = vec3<f32>(0.0);
    for (var i = 0; i < 3; i++) {
        state = state * 747796405u + 2891336453u;
        let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
        out[i] = f32((word >> 22u) ^ word) / 4294967296.0;
    }
    return out;
}

@vertex
fn mote_vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> MoteOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let corner = corners[vertex];
    let eye = camera.position;
    // Fixed in the world: of every copy of the mote, the one nearest the eye.
    let home = hash3(instance) * CUBE;
    let world = eye + (fract((home - eye) / CUBE + 0.5) - 0.5) * CUBE;
    let range = length(world - eye);
    let centre = world_to_clip(world);
    let pixels = RADIUS * camera.viewport.y / (2.0 * camera.tan_half_fov_y * max(range, 0.1));
    let radius = clamp(pixels, SMALLEST, LARGEST);
    // Its pixel's ray starts under the waves, or it is not drawn.
    let uv = centre.xy / max(centre.w, 1e-4) * vec2<f32>(0.5, -0.5) + 0.5;
    let under = smoothstep(0.0, 0.4, textureSampleLevel(depth_probe, probe_sampler, uv, 0.0).r);
    let shown = smoothstep(NEAR.x, NEAR.y, range) * (1.0 - smoothstep(FAR.x, FAR.y, range)) * under * min(pixels / SMALLEST, 1.0);
    // Lit as the water about the camera: the daylight left at its depth, the sun's forward lobe included.
    let depth = max(textureLoad(depth_probe, vec2<i32>(0, 0), 0).g, 0.0) * water.absorption.w;
    let view = (world - eye) / max(range, 1e-3);
    let optics = sample_water(water.water, world.xz, water.absorption.rgb);
    let beam = select(water.absorption.rgb, water_beam(optics), water.water.absorption.w > 0.5);
    let pigment = select(water.pigment.rgb, water_pigment(optics, water.absorption.rgb, water.pigment.rgb), water.water.absorption.w > 0.5);
    let glow = water_light(view, pigment, water.sun_direction.xyz, water.sun_color.rgb)
        * exp(-diffuse_attenuation(beam) * depth);
    var out: MoteOut;
    out.clip = centre + vec4<f32>(corner * (radius * 2.0) * camera.viewport.zw * centre.w, 0.0, 0.0);
    out.offset = corner;
    out.light = vec4<f32>(glow * LIGHT, shown * OPACITY * step(1e-3, centre.w));
    return out;
}

// Added to the scene (blended: one, one).
@fragment
fn mote_fragment(in: MoteOut) -> @location(0) vec4<f32> {
    let soft = 1.0 - smoothstep(0.35, 1.0, length(in.offset));
    return vec4<f32>(in.light.rgb * (in.light.a * soft), 0.0);
}
