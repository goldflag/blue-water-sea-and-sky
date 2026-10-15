// The scene's point lights on the sea: the combat flashes, the burning ships' fires and lightning's stroke (the lights
// `naval::lights` hands every lit layer; the sea takes its own list, `naval_render::PointLights`, with fires out to the
// horizon). The web's sea took none of them (its material replaced three's lighting); here each lights the sea as the
// sun or moon does, through the same models: the facets' glitter (`naval_ocean::reflection::sun_glitter`, Cox–Munk's
// slope distribution of the pixel's unresolved facets around its resolved normal with Smith masking; the facets' frame
// taken once a pixel, `light_frame`, the rest only where a light can glint), the light crossing the mean surface into the
// water that the water scatters back out (the sunlit term of `naval_ocean::water_body::upwelling`), and the foam's
// wrapped diffuse light (`naval_ocean::foam::foam_radiance`'s). The surface sums the lights and puts their glints under
// the soft knee the sun's or moon's take, the sea's ceiling on its glints. A light is a point of three's intensity (its
// colour times candela in the sea's units): the irradiance on a surface facing it is that over the squared distance,
// with no cut-off (the reach three's lights carry is a culling convenience; the sea mirrors a light from wherever it
// stands). A light at or below a water point lights nothing there, and nor does one the opaque scene hides from it
// (`light_seen`: a hull, her upperworks, another ship or the land between the water and a fire, marched on screen).
#define_import_path naval_ocean::point_lights

#import naval::camera::{camera, curved}
#import naval::lights::PointLight
#import naval_ocean::reflection::{SUN_DISC_VARIANCE, PI, beckmann_lambda, azimuth_variance, schlick}
#import naval_ocean::foam::{FOAM_ALBEDO, FOAM_WRAP, FOAM_SUN}

const INV_PI: f32 = 0.3183098861837907;
// Closer than this (m) a light's falloff stops: a flash's fireball and a fire's flames are metres across.
const NEAREST: f32 = 1.0;
// Beyond this many squared deviations of the facets' slopes (twice the Gaussian's exponent) no facet mirrors the light
// toward the eye: e^-20 of the density's peak, under a millionth of a glint at any light the sea takes.
const NO_GLINT: f32 = 40.0;
// Below this irradiance (luminance) a light's diffuse share, in the water and on foam, is under a thousandth of the
// moonlit sea's: only its glints can show there.
const FAINT: f32 = 1e-3;
// The march toward a light over the opaque scene's depth (`light_seen`): its steps, packed toward the light, where the
// burning ship's own upperworks stand (each step's place goes as the square of its count from the light's end).
const SEEN_STEPS: i32 = 16;
const SEEN_BATCH: i32 = 4;
// The march goes toward this far (m) over the light: a fire's light stands about half way up its flames, and water
// that sees their tops mirrors them.
const SEEN_RISE: f32 = 5.0;
// The march stops this far (m) short of that point: flames stand metres over the roof or deck they burn on.
const SEEN_CLEARANCE: f32 = 4.0;
// A step lies behind a surface once it is this far (m) past it: the depth copy's precision, a deck's camber.
const SEEN_BIAS: f32 = 0.5;
// How deep (m) a surface is taken to be behind its face, the larger of the two: a hull's beam at hand, a twentieth
// of the distance far off, where a hull seen end-on is hundreds of metres deep. A step farther behind a surface than
// that passed behind something nearer the eye than the light's path, which hides none of it.
const SEEN_DEPTH: f32 = 30.0;
const SEEN_DEPTH_SHARE: f32 = 0.05;

// How a light's terms reach the pixel (`SeaLights`' in turn, as the surface sums them), and the least radiance
// (luminance) a light must add there for it to be marched toward (`light_seen`): less, and its shadow cannot show.
struct LightShares {
    glint: vec3<f32>,
    into_water: vec3<f32>,
    foam: vec3<f32>,
    least: f32,
}

// What point lights give one sea pixel.
struct SeaLights {
    // Radiance the facets mirror toward the viewer (the glitter).
    glint: vec3<f32>,
    // Irradiance crossing the mean surface into the water (Fresnel's transmitted share, the cosine of incidence), of
    // which the water scatters `subsurface_reflectance` back up.
    into_water: vec3<f32>,
    // The foam's radiance from the lights (its albedo included), as `foam_radiance` gives the sun's.
    foam: vec3<f32>,
}

// `sun_glitter`'s frame of the facets for a pixel, which every light's test needs.
struct LightFrame {
    // Along the wind and across it, in the resolved surface.
    along: vec3<f32>,
    across: vec3<f32>,
    // The reciprocals of the per-axis slope variances (the sun disc's spread included, as `sun_glitter` has them).
    inverse_spread: vec2<f32>,
}

// The frame for the resolved normal `normal`, with the unresolved slope variances along and across the unit `wind` (xz).
fn light_frame(normal: vec3<f32>, variances: vec2<f32>, wind: vec2<f32>) -> LightFrame {
    let flat_wind = vec3<f32>(wind.x, 0.0, wind.y);
    let along = normalize(flat_wind - normal * dot(normal, flat_wind));
    return LightFrame(along, cross(normal, along), 1.0 / (variances + vec2<f32>(SUN_DISC_VARIANCE)));
}

// A pixel's shift of the march's steps toward the lights (`light_seen`), 0-1: interleaved gradient noise (Jimenez 2014),
// moved each frame so the sea's temporal history blends the steps between frames.
fn light_dither(pixel: vec2<f32>) -> f32 {
    let f = f32(camera.frame % 64u) * 5.588238;
    let p = floor(pixel) + vec2<f32>(f, f);
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// Whether the light at `light` reaches the water point `world` (world positions) past the opaque scene (`depth`, the
// copy of its reversed depth behind the sea), marched on screen from the light's end toward the water, the steps
// shifted by `dither` (0-1) of a step. A step is hidden where a surface on screen lies in front of it by no more than
// a hull is deep; the light is seen past what the screen does not show (off its edges, behind the eye).
fn light_seen(depth: texture_depth_2d, world: vec3<f32>, light: vec3<f32>, dither: f32) -> bool {
    let origin = (camera.view * vec4<f32>(curved(world), 1.0)).xyz;
    let span = (camera.view * vec4<f32>(curved(light + vec3<f32>(0.0, SEEN_RISE, 0.0)), 1.0)).xyz - origin;
    let length_total = length(span);
    if length_total <= SEEN_CLEARANCE {
        return true;
    }
    // The far end: the clearance short of the light, and in front of the eye.
    var reach = 1.0 - SEEN_CLEARANCE / length_total;
    let near = -camera.near * 1.001;
    if origin.z + span.z * reach > near {
        reach = (near - origin.z) / span.z;
    }
    // Clip space is linear along the segment.
    let clip_origin = camera.projection * vec4<f32>(origin, 1.0);
    let clip_span = camera.projection * vec4<f32>(span, 0.0) * reach;
    let size = vec2<f32>(textureDimensions(depth));
    // The steps go in batches whose depths are read together, so the reads' latencies overlap.
    for (var first = 0; first < SEEN_STEPS; first += SEEN_BATCH) {
        var alongs = vec4<f32>(0.0);
        var depths = vec4<f32>(0.0);
        for (var k = 0; k < SEEN_BATCH; k++) {
            let place = (f32(first + k) + dither) / f32(SEEN_STEPS);
            let t = 1.0 - place * place;
            let clip = clip_origin + clip_span * t;
            let ndc = clip.xy / clip.w;
            // Off the screen a step reads nothing and is judged seen (no depth that far in front of it).
            alongs[k] = select(-(origin.z + span.z * reach * t), -1.0, abs(ndc.x) >= 1.0 || abs(ndc.y) >= 1.0);
            let texel = clamp(vec2<i32>(vec2<f32>(ndc.x * 0.5 + 0.5, ndc.y * -0.5 + 0.5) * size), vec2<i32>(0),
                vec2<i32>(size) - vec2<i32>(1));
            depths[k] = textureLoad(depth, texel, 0);
        }
        for (var k = 0; k < SEEN_BATCH; k++) {
            let scene = camera.near * camera.far / (depths[k] * (camera.far - camera.near) + camera.near);
            let behind = alongs[k] - scene;
            if scene < camera.far * 0.999 && behind > SEEN_BIAS && behind < max(SEEN_DEPTH, alongs[k] * SEEN_DEPTH_SHARE) {
                return false;
            }
        }
    }
    return true;
}

// One light at the water point `world` with resolved normal `normal`, seen along `view` (toward the eye) in `frame`,
// with the pixel's unresolved slope variances along and across the unit `wind` (xz). Where it adds enough to the pixel
// (`shares`), the march toward it over `depth` (`light_seen`, shifted by `dither`) says whether anything hides it.
fn sea_point_light(
    light: PointLight,
    frame: LightFrame,
    world: vec3<f32>,
    normal: vec3<f32>,
    view: vec3<f32>,
    variances: vec2<f32>,
    wind: vec2<f32>,
    depth: texture_depth_2d,
    dither: f32,
    shares: LightShares,
) -> SeaLights {
    var out = SeaLights(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0));
    let to_light = light.position.xyz - world;
    let squared = max(dot(to_light, to_light), NEAREST * NEAREST);
    let inverse_distance = inverseSqrt(squared);
    let direction = to_light * inverse_distance;
    if direction.y <= 0.0 {
        return out;
    }
    var irradiance = light.color.rgb * (inverse_distance * inverse_distance);
    if dot(light.cone.xyz, light.cone.xyz) > 0.5 {
        let inner = mix(light.cone.w, 1.0, 0.15);
        irradiance *= smoothstep(light.cone.w, inner, dot(light.cone.xyz, -direction));
        if light.position.w > 0.0 {
            let ratio = sqrt(squared) / light.position.w;
            let window = clamp(1.0 - ratio * ratio * ratio * ratio, 0.0, 1.0);
            irradiance *= window * window;
        }
    }
    if dot(irradiance, vec3<f32>(0.2126, 0.7152, 0.0722)) > FAINT {
        out.into_water = irradiance * (direction.y * (1.0 - schlick(direction.y)));
        let facing = max((dot(normal, direction) + FOAM_WRAP) / (1.0 + FOAM_WRAP), 0.0);
        out.foam = irradiance * (facing * (FOAM_SUN * INV_PI) * FOAM_ALBEDO);
    }
    // The glitter (`sun_glitter`), where any facet can mirror the light toward the eye: the facet slopes the half vector
    // needs (its length cancels in them) against their spread first.
    let halfway = direction + view;
    let rise = max(dot(normal, halfway), 1e-6);
    let slope_along = dot(halfway, frame.along) / rise;
    let slope_across = dot(halfway, frame.across) / rise;
    let deviations = slope_along * slope_along * frame.inverse_spread.x
        + slope_across * slope_across * frame.inverse_spread.y;
    if deviations <= NO_GLINT {
        let spread = variances + vec2<f32>(SUN_DISC_VARIANCE);
        let half_vector = normalize(halfway);
        let cosine = max(dot(normal, half_vector), 1e-3);
        let to_view = dot(normal, view);
        let to_light_facing = dot(normal, direction);
        let view_heading = normalize(vec2<f32>(view.x, view.z) + vec2<f32>(1e-6, 0.0));
        let light_heading = normalize(vec2<f32>(direction.x, direction.z) + vec2<f32>(1e-6, 0.0));
        let masking = 1.0 / (1.0 + beckmann_lambda(to_view, azimuth_variance(spread, wind, view_heading))
            + beckmann_lambda(to_light_facing, azimuth_variance(spread, wind, light_heading)));
        let above = smoothstep(0.0, 0.02, to_light_facing) * smoothstep(-0.02, 0.03, direction.y);
        let cosine2 = cosine * cosine;
        let density = exp(deviations * -0.5) / (sqrt(spread.x * spread.y) * 2.0 * PI);
        out.glint = irradiance * (schlick(dot(view, half_vector)) * density * masking * above
            / (max(to_view, 0.01) * (cosine2 * cosine2) * 4.0));
    }
    let shown = out.glint * shares.glint + out.into_water * shares.into_water + out.foam * shares.foam;
    if dot(shown, vec3<f32>(0.2126, 0.7152, 0.0722)) > shares.least && !light_seen(depth, world, light.position.xyz, dither) {
        return SeaLights(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0));
    }
    return out;
}
