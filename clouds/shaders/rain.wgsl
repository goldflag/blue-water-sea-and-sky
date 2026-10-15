// Near-camera rain, ported from the web game's `sky/weather/rain.ts` (`RainField.build`): instanced streaks in four
// nested boxes that wrap around the camera, each drop its own size, fall speed and brightness, drawn as the streak
// its motion relative to the camera paints over an exposure. A drop shows a blurred view of the sky and sea behind
// it, glints forward toward the celestial light and flares toward a lightning strike. Premultiplied alpha.

#import naval_sky::common::{sky}
#import naval_sky::atmosphere::{sky_radiance, ambient}
#import naval::motion::motion_response

struct RainUniforms {
    // Per class k: [3k] (phase x, y, z, box edge), [3k + 1] (box centre from the camera, fall speed), [3k + 2] (fall).
    boxes: array<vec4<f32>, 12>,
    view: mat4x4<f32>,
    projection: mat4x4<f32>,
    camera_velocity: vec3<f32>,
    opacity: f32,
    wind: vec3<f32>,
    density: f32,
    bolt_direction: vec3<f32>,
    bolt_irradiance: f32,
    screen: vec2<f32>,
    diameter: f32,
    direct_share: f32,
    grey: f32,
    height: f32,
    camera_altitude: f32,
    padding: f32,
}

@group(2) @binding(0) var<uniform> rain: RainUniforms;

const EDGE_FADE: f32 = 0.08;
const EXPOSURE: f32 = 0.016666668;
const MAX_STREAK: f32 = 1.2;
const NEAR_CULL: f32 = 0.6;
const NEAR_FULL: f32 = 1.6;
const SMALLEST: f32 = 0.45;
const SIZE_SPREAD: f32 = 1.0;
const DENSITY_GAIN: f32 = 500.0;
const MAX_ALPHA: f32 = 0.8;
const LONG: f32 = 24.0;
const LONG_POWER: f32 = 0.6;
const MIN_WIDTH: f32 = 1.0;
const MARGIN: f32 = 0.35;
const MAX_PIXELS: f32 = 110.0;
const ALPHA_CULL: f32 = 0.004;
const DOWN_FROM: f32 = 0.05;
const DOWN_TO: f32 = 0.8;
const DOWN_FADE: f32 = 0.35;
const DOWN_SHORTEN: f32 = 0.45;
const HIGH_FADE: f32 = 0.3;
const HIGH_SHORTEN: f32 = 0.5;
const SWAY: f32 = 0.07;
const SWAY_PER: f32 = 8.0;
const GLINT_G: f32 = 0.8;
const GLINT: f32 = 1.0;
const BOLT_G: f32 = 0.7;
const BOLT_FLOOR: f32 = 0.01;
const BOLT_GAIN: f32 = 0.08;
const SKYWARD_FROM: f32 = -0.4;
const SKYWARD_TO: f32 = 0.08;
const SEA_SHARE: f32 = 0.12;
const RIM_SKY: f32 = 0.25;
const RIM: f32 = 1.4;
const SKY_ELEVATION: f32 = 0.15;
const TAU: f32 = 6.283185307179586;

fn ramp(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// Henyey–Greenstein phase for a scattering angle's cosine.
fn phase(cosine: f32, g: f32) -> f32 {
    return (1.0 - g * g) / (4.0 * 3.141592653589793) / pow(max(1.0 + g * g - cosine * 2.0 * g, 1e-4), 1.5);
}

// The sky light around a drop seen along `view`: the dome toward it, at least 0.15 up, greyed under a deck.
fn sky_around(view: vec3<f32>, grey: f32) -> vec3<f32> {
    let color = sky_radiance(normalize(vec3<f32>(view.x, max(view.y, SKY_ELEVATION), view.z)), false);
    return mix(color, vec3<f32>(dot(color, vec3<f32>(0.2126, 0.7152, 0.0722))), grey);
}

struct RainVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) across: f32,
    @location(1) along: f32,
    @location(2) half_width: f32,
    @location(3) pixels: f32,
    @location(4) alpha: f32,
    @location(5) core: vec3<f32>,
    @location(6) rim: vec3<f32>,
    @location(7) glint: vec3<f32>,
}

@vertex
fn rain_vertex(@builtin(vertex_index) index: u32, @location(0) drop: vec4<f32>) -> RainVertex {
    // Corners: x across the streak (−1, 1), y along it (0 tail, 1 head); two triangles (0 1 2, 2 1 3).
    var corners = array<vec2<f32>, 4>(vec2<f32>(-1.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, 1.0));
    var order = array<u32, 6>(0u, 1u, 2u, 2u, 1u, 3u);
    let corner = corners[order[index % 6u]];
    let k = i32(drop.w);
    let seed = fract(drop.w);
    let box = rain.boxes[k * 3];
    let place = rain.boxes[k * 3 + 1];
    let fall = rain.boxes[k * 3 + 2].x;
    // The drop's own size, its speed in whole tenths of the class's, and its brightness.
    let grade = fract(seed * 7.919);
    let tenths = floor(grade * 5.999) + 5.0;
    let size = grade * grade * SIZE_SPREAD + SMALLEST;
    let speed = place.w * tenths * 0.1;
    let q = fract(drop.xyz + box.xyz - vec3<f32>(0.0, fall * tenths * 0.1, 0.0)) - 0.5;
    let edge = max(abs(q.x), max(abs(q.y), abs(q.z)));
    let cycles = max(1.0, round(box.w / SWAY_PER));
    let turn = (q.y * cycles + seed) * TAU;
    let turn_rate = speed / box.w * cycles * -TAU;
    let sway = vec3<f32>(sin(turn), 0.0, cos(turn * 2.0 + seed * 5.0)) * SWAY;
    let sway_velocity = vec3<f32>(cos(turn), 0.0, sin(turn * 2.0 + seed * 5.0) * -2.0) * (turn_rate * SWAY);
    let head = place.xyz + q * box.w + sway;
    let view = normalize(head);
    let down = ramp(DOWN_FROM, DOWN_TO, -view.y);
    let velocity = vec3<f32>(rain.wind.x, -speed, rain.wind.z) + sway_velocity - rain.camera_velocity;
    let shorten = (1.0 - down * (1.0 - DOWN_SHORTEN)) * (1.0 - rain.height * (1.0 - HIGH_SHORTEN));
    let streak = velocity * (shorten * EXPOSURE);
    let streak_length = length(streak);
    let tail = head - streak * min(1.0, MAX_STREAK / max(streak_length, 1e-5));
    let view_head = (rain.view * vec4<f32>(head, 0.0)).xyz;
    let view_tail = (rain.view * vec4<f32>(tail, 0.0)).xyz;
    let depth = -view_head.z;
    let clip_head = rain.projection * vec4<f32>(view_head, 1.0);
    let clip_full = rain.projection * vec4<f32>(view_tail, 1.0);
    let half_screen = rain.screen * 0.5;
    let pixel_head = clip_head.xy / clip_head.w * half_screen;
    let cut = min(1.0, MAX_PIXELS / max(length(pixel_head - clip_full.xy / clip_full.w * half_screen), 1e-3));
    let clip_tail = mix(clip_head, clip_full, cut);
    let tail_depth = clip_tail.w;
    let pixel_tail = clip_tail.xy / clip_tail.w * half_screen;
    let delta = pixel_head - pixel_tail;
    let pixels = length(delta);
    let direction = select(vec2<f32>(0.0, 1.0), delta / max(pixels, 1e-3), pixels > 1e-3);
    let normal = vec2<f32>(-direction.y, direction.x);
    let focal = rain.projection[1][1] * rain.screen.y * 0.5;
    let width = rain.diameter * size * focal / max(depth, NEAR_CULL);
    let drawn = max(width, MIN_WIDTH);
    let half_width = drawn * 0.5 + MARGIN;
    let pixel = mix(pixel_tail, pixel_head, corner.y) + normal * (corner.x * half_width) + direction * ((corner.y * 2.0 - 1.0) * half_width);
    let end = mix(clip_tail, clip_head, corner.y);
    // Time-averaged coverage: a drop `width` wide smeared over `pixels` covers width² / (pixels + width) of a line.
    let coverage = width * width / ((pixels + width) * drawn);
    let longest = pow(LONG / max(pixels, LONG), LONG_POWER);
    let faded = min(coverage * (rain.density * DENSITY_GAIN), longest * (grade * 0.35 + 0.65) * MAX_ALPHA)
        * ramp(NEAR_CULL, NEAR_FULL, depth) * (1.0 - ramp(0.5 - EDGE_FADE / 2.0, 0.5, edge))
        * (1.0 - down * (1.0 - DOWN_FADE)) * (1.0 - rain.height * (1.0 - HIGH_FADE)) * rain.opacity;
    let visible = depth > NEAR_CULL && tail_depth > NEAR_CULL * 0.5 && faded > ALPHA_CULL;
    var out: RainVertex;
    out.across = corner.x * half_width;
    out.along = corner.y * pixels + (corner.y * 2.0 - 1.0) * half_width;
    out.half_width = drawn * 0.5;
    out.pixels = pixels;
    out.alpha = select(0.0, faded, visible);
    // Light: the sky and sea behind the drop, a forward glint of the celestial light, and the lightning.
    let sky_share = ramp(SKYWARD_FROM, SKYWARD_TO, view.y) * (1.0 - SEA_SHARE) + SEA_SHARE;
    let around = sky_around(view, rain.grey);
    let sea = ambient(max(rain.camera_altitude, 0.0)).below;
    let lift = sky.flash + 1.0;
    out.core = mix(sea, around, sky_share) * lift;
    out.rim = mix(sea, around, min(sky_share + RIM_SKY, 1.0)) * RIM * lift;
    out.glint = sky.light_color * rain.direct_share * phase(dot(view, sky.light_direction), GLINT_G) * GLINT
        + vec3<f32>(0.8, 0.87, 1.0) * (rain.bolt_irradiance * (phase(dot(view, rain.bolt_direction), BOLT_G) + BOLT_FLOOR) * BOLT_GAIN);
    out.position = select(vec4<f32>(0.0, 0.0, 2.0, 1.0), vec4<f32>(pixel / half_screen * end.w, end.z, end.w), visible);
    return out;
}

@fragment
fn rain_fragment(in: RainVertex) -> @location(0) vec4<f32> {
    return streak(in);
}

// Temporal AA's motion pass: a streak falls somewhere new every frame, so where one shows the frame takes no history
// (the web's rain writes a response of 1), or the history averages the streaks away. Its faintest edge keeps some.
@fragment
fn rain_motion(in: RainVertex) -> @location(0) vec4<f32> {
    return motion_response(min(streak(in).a * 50.0, 1.0));
}

// A flat-topped line with antialiased edges and soft ends; its edges catch the wider field.
fn streak(in: RainVertex) -> vec4<f32> {
    let offset = abs(in.across);
    let profile = 1.0 - ramp(in.half_width - MARGIN, in.half_width + MARGIN, offset);
    let ends = ramp(-in.half_width, in.half_width * 0.5, in.along) * (1.0 - ramp(in.pixels - in.half_width * 0.5, in.pixels + in.half_width, in.along));
    let edge_light = min(offset / max(in.half_width, 0.5), 1.0);
    let radiance = mix(in.core, in.rim, edge_light * edge_light) + in.glint;
    let alpha = in.alpha * profile * ends;
    return vec4<f32>(radiance * alpha, alpha);
}
