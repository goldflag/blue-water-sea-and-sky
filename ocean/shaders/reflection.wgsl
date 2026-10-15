// Ports the web game's `ocean/surface/physicalReflection.ts` (`OceanRealism.reflections`, after Bruneton, Neyret & Holzschuch
// 2010): the pixel's normal is the mean of the waves it resolves; every finer facet is a Gaussian slope distribution
// around it (Cox & Munk 1954). The sky is mirrored through the facets the viewer sees, blurred by their spread, with
// the Fresnel reflectance averaged over them; the sun is reflected by the same distribution as a glitter BRDF with
// Smith masking. Also `fresnel`, `smithLambda` and `softKnee` of the web game's `ocean/surface/OceanSurfaceMaterial.ts`.
#define_import_path naval_ocean::reflection

#import naval_ocean::sky_environment::{SkyEnvironment, env_level, env_radiance_level}

const WATER_F0: f32 = 0.02;
const INV_SQRT_2PI: f32 = 0.3989422804014327;
const PI: f32 = 3.141592653589793;
// Cox & Munk (1954), clean sea surface: mean square slope along the wind 3.16e-3·U and across it 0.003 + 1.92e-3·U.
const COX_MUNK_ALONG: f32 = 3.16e-3;
const COX_MUNK_ACROSS: f32 = 1.92e-3;
const COX_MUNK_ACROSS_CALM: f32 = 3e-3;
// Share of the squared change of slope between neighbouring pixels that counts as slope variance inside a pixel.
const FOOTPRINT: f32 = 0.25;
const FOOTPRINT_LIMIT: f32 = 0.1;
// Per-axis slope variance of the facets that mirror some part of the game's 1.4° sun disc.
const SUN_DISC_VARIANCE: f32 = 9.37890625e-6;
// Each of the three sky taps keeps at least this share of the lobe's long axis as its own blur.
const TAP_BLUR: f32 = 0.5;
const TAP_SPREAD: f32 = 0.3;
// In-plane deviations below the lobe's centre at which a screen-space ray is traced.
const TRACE_BELOW: f32 = 1.0;

// Schlick's approximation with the surface's tiny grazing guard.
fn schlick(cosine: f32) -> f32 {
    return WATER_F0 + (1.0 - WATER_F0) * pow(1.0 - clamp(cosine, 1e-4, 1.0), 5.0);
}

fn along_wind_share(wind_speed: f32) -> f32 {
    let u = max(wind_speed, 0.0);
    let along = u * COX_MUNK_ALONG;
    let across = u * COX_MUNK_ACROSS + COX_MUNK_ACROSS_CALM;
    return along / (along + across);
}

// Slope variance the pixel spans that the wave field's filtering cannot see (Kaplanyan et al. 2016). Needs uniform
// control flow.
fn footprint_variance(normal: vec3<f32>) -> f32 {
    let slope = normal.xz / max(normal.y, 0.05);
    let across = dpdx(slope);
    let down = dpdy(slope);
    return min((dot(across, across) + dot(down, down)) * FOOTPRINT, FOOTPRINT_LIMIT);
}

// Per-axis unresolved slope variances (along, across the wind) from their total.
fn wind_variances(total: f32, wind_speed: f32) -> vec2<f32> {
    let along = along_wind_share(wind_speed);
    return vec2<f32>(total * along, total * (1.0 - along));
}

fn azimuth_variance(variances: vec2<f32>, wind: vec2<f32>, azimuth: vec2<f32>) -> f32 {
    let along = dot(azimuth, wind);
    return mix(variances.y, variances.x, along * along);
}

// φ(t)/Φ(t) of the standard normal at any t (the inverse Mills ratio), with Φ's tail from Bruneton et al.'s erfc fit.
fn inverse_mills(t: f32) -> f32 {
    let bound = abs(t) * (2.319 / 1.4142135623730951) + sqrt(t * t * (1.52 / 2.0) + 4.0);
    let density = exp(t * t * -0.5);
    return select(bound * INV_SQRT_2PI, density * INV_SQRT_2PI / (1.0 - density / bound), t >= 0.0);
}

struct VisibleSlopes {
    mean: f32,
    variance: f32,
}

// What a viewer sees of a Gaussian slope distribution around a mean surface seen at cot θ (Smith 1967; Heitz 2014).
fn visible_slopes(cotangent: f32, variance: f32) -> VisibleSlopes {
    let sigma = sqrt(max(variance, 1e-8));
    let t = max(cotangent / sigma, -2.0);
    let mills = inverse_mills(t);
    let total = max(mills + t, 1e-3);
    let mean = max(sigma / total, -cotangent + sigma * 0.05);
    let spread = variance * (1.0 + mills / total - 1.0 / (total * total));
    return VisibleSlopes(mean, min(max(spread, 0.0), variance));
}

// Smith's Λ for a Beckmann slope distribution of per-axis variance σ² along the direction's azimuth.
fn beckmann_lambda(cosine: f32, variance: f32) -> f32 {
    let c = clamp(cosine, 1e-4, 1.0);
    let t = max(c / (sqrt(max(1.0 - c * c, 1e-8)) * sqrt(max(variance, 1e-8))), 1e-3);
    let bound = t * (2.319 / 1.4142135623730951) + sqrt(t * t * (1.52 / 2.0) + 4.0);
    return max(exp(t * t * -0.5) * (INV_SQRT_2PI - t / bound) / t, 0.0);
}

// Fresnel reflectance averaged over the facets a viewer sees on a sea of RMS slope σ along the view's azimuth.
fn mean_fresnel(cosine: f32, sigma: f32) -> f32 {
    let grazing = pow(1.0 - clamp(cosine, 1e-4, 1.0), exp(sigma * -2.69) * 5.0);
    return WATER_F0 + (1.0 - WATER_F0) * grazing / (pow(sigma, 1.5) * 22.7 + 1.0);
}

// The PMREM lookup roughness whose blur has angular standard deviation `angle` (radians): the web's measured table
// (`PMREM_BLUR`) for three's PMREM from the game's 384 px sky bake.
fn pmrem_roughness(angle: f32) -> f32 {
    var blur = array<vec2<f32>, 10>(
        vec2<f32>(0.0185, 0.1), vec2<f32>(0.0412, 0.15), vec2<f32>(0.0827, 0.2), vec2<f32>(0.1175, 0.3),
        vec2<f32>(0.1775, 0.4), vec2<f32>(0.2583, 0.5), vec2<f32>(0.356, 0.6), vec2<f32>(0.466, 0.7),
        vec2<f32>(0.579, 0.8), vec2<f32>(0.7208, 1.0),
    );
    var roughness = blur[0].y;
    for (var i = 1; i < 10; i++) {
        let lower = blur[i - 1];
        let upper = blur[i];
        roughness += clamp((angle - lower.x) / (upper.x - lower.x), 0.0, 1.0) * (upper.y - lower.y);
    }
    return roughness;
}

struct ReflectionLobe {
    // Mean normal of the facets that mirror light to the viewer.
    facet: vec3<f32>,
    // The mirrored ray off `facet`.
    direction: vec3<f32>,
    // Unit vector perpendicular to `direction` in the plane of incidence, away from the surface.
    up: vec3<f32>,
    // Angular standard deviations (radians) of the mirrored rays in the plane of incidence and across it.
    in_plane: f32,
    across: f32,
    // RMS unresolved slope along the view's azimuth, for the mean Fresnel.
    sigma_view: f32,
    // The ray a screen-space trace follows: TRACE_BELOW in-plane deviations below `direction`.
    traced: vec3<f32>,
}

// Schlick's reflectance falls with the cosine of incidence at the rate k = −d ln F/dc.
fn fresnel_falloff(cosine: f32) -> f32 {
    let c = clamp(cosine, 0.0, 1.0);
    return pow(1.0 - c, 4.0) * (5.0 * (1.0 - WATER_F0)) / schlick(c);
}

// The lobe of rays the unresolved facets mirror toward the viewer. `variances` are per axis, along and across the
// unit `wind` (xz).
fn reflection_lobe(view: vec3<f32>, normal: vec3<f32>, variances: vec2<f32>, wind: vec2<f32>) -> ReflectionLobe {
    let cosine = dot(normal, view);
    let tangent = view - normal * cosine;
    let sine = max(length(tangent), 1e-5);
    let toward = tangent / sine;
    let azimuth = normalize(vec2<f32>(toward.x, toward.z) + vec2<f32>(1e-6, 0.0));
    let in_plane_variance = azimuth_variance(variances, wind, azimuth);
    let across_variance = max(variances.x + variances.y - in_plane_variance, 0.0);
    let shift = fresnel_falloff(cosine) * in_plane_variance;
    let visible = visible_slopes(cosine / sine - shift, in_plane_variance);
    let facet = normalize(normal + toward * (visible.mean - shift));
    let direction = reflect(-view, facet);
    let up = normalize(facet - direction * dot(facet, direction) + vec3<f32>(0.0, 1e-6, 0.0));
    // A slope s turns a facet by atan s, and the mirrored ray by twice that.
    let mean = visible.mean - shift;
    let turn = 2.0 / (mean * mean + 1.0);
    let in_plane = sqrt(visible.variance) * turn;
    let below = in_plane * TRACE_BELOW;
    return ReflectionLobe(
        facet,
        direction,
        up,
        in_plane,
        sqrt(across_variance) * max(dot(facet, view), 0.0) * 2.0,
        sqrt(in_plane_variance),
        direction * cos(below) - up * sin(below),
    );
}

// One tap of the sky lobe: the ray turned `turn` radians in the plane of incidence, the horizon's below it.
fn lobe_tap(atlas: texture_2d<f32>, atlas_sampler: sampler, env: SkyEnvironment, lobe: ReflectionLobe, turn: f32,
    level: vec2<f32>) -> vec3<f32> {
    let ray = lobe.direction * cos(turn) + lobe.up * sin(turn);
    return env_radiance_level(atlas, atlas_sampler, env, normalize(vec3<f32>(ray.x, max(ray.y, 0.0), ray.z)), level);
}

// Sky radiance over the reflected lobe: three taps along the plane of incidence for an elongated lobe, one for a
// round one, all at one roughness. Rays below the horizon read the horizon.
fn sky_lobe(atlas: texture_2d<f32>, atlas_sampler: sampler, env: SkyEnvironment, lobe: ReflectionLobe) -> vec3<f32> {
    let blur = max(lobe.across, lobe.in_plane * TAP_BLUR);
    let tap_step = sqrt(max((lobe.in_plane * lobe.in_plane - blur * blur) * 2.0, 0.0));
    let elongated = tap_step > blur * TAP_SPREAD;
    let level = env_level(env, pmrem_roughness(select(max(lobe.in_plane, lobe.across), blur, elongated)));
    if !elongated {
        return lobe_tap(atlas, atlas_sampler, env, lobe, 0.0, level);
    }
    var sky = lobe_tap(atlas, atlas_sampler, env, lobe, -tap_step, level) * 0.25;
    sky += lobe_tap(atlas, atlas_sampler, env, lobe, 0.0, level) * 0.5;
    sky += lobe_tap(atlas, atlas_sampler, env, lobe, tap_step, level) * 0.25;
    return sky;
}

// The density, per unit of slope area, of the facets about `normal` that turn the sun to the viewer: Cox–Munk's
// anisotropic Gaussian (plus the sun disc's own spread) at the half vector's slope.
fn glitter_density(normal: vec3<f32>, view: vec3<f32>, sun: vec3<f32>, variances: vec2<f32>, wind: vec2<f32>) -> f32 {
    let half_vector = normalize(sun + view);
    let flat_wind = vec3<f32>(wind.x, 0.0, wind.y);
    let along = normalize(flat_wind - normal * dot(normal, flat_wind));
    let across = cross(normal, along);
    let cosine = max(dot(normal, half_vector), 1e-3);
    let slope_along = dot(half_vector, along) / cosine;
    let slope_across = dot(half_vector, across) / cosine;
    let along_variance = variances.x + SUN_DISC_VARIANCE;
    let across_variance = variances.y + SUN_DISC_VARIANCE;
    return exp((slope_along * slope_along / along_variance + slope_across * slope_across / across_variance) * -0.5)
        / (sqrt(along_variance * across_variance) * 2.0 * PI);
}

// The sun's (or moon's) reflection by every facet: Cox–Munk's anisotropic Gaussian slope distribution (plus the sun
// disc's own spread) around the resolved normal, with Smith masking and shadowing. `irradiance` is the sun's on a
// surface facing it.
fn sun_glitter(normal: vec3<f32>, view: vec3<f32>, sun: vec3<f32>, irradiance: vec3<f32>, variances: vec2<f32>, wind: vec2<f32>) -> vec3<f32> {
    let half_vector = normalize(sun + view);
    let cosine = max(dot(normal, half_vector), 1e-3);
    let density = glitter_density(normal, view, sun, variances, wind);
    let to_view = dot(normal, view);
    let to_sun = dot(normal, sun);
    let spread = variances + vec2<f32>(SUN_DISC_VARIANCE);
    let view_heading = normalize(vec2<f32>(view.x, view.z) + vec2<f32>(1e-6, 0.0));
    let sun_heading = normalize(vec2<f32>(sun.x, sun.z) + vec2<f32>(1e-6, 0.0));
    let masking = 1.0 / (1.0 + beckmann_lambda(to_view, azimuth_variance(spread, wind, view_heading))
        + beckmann_lambda(to_sun, azimuth_variance(spread, wind, sun_heading)));
    let above = smoothstep(0.0, 0.02, to_sun) * smoothstep(-0.02, 0.03, sun.y);
    let cosine2 = cosine * cosine;
    return irradiance * (schlick(dot(view, half_vector)) * density * masking * above / (max(to_view, 0.01) * (cosine2 * cosine2) * 4.0));
}

// The sun's glitter as glints, not a sheen (native, 2026-10-08). `sun_glitter` is the mean over a pixel's facets, but
// the sun's reflection is the few wavelets turned just so, each a tiny image of the sun far brighter than white: from
// the air, where a pixel spans metres of sea, it holds none to a few of them, and the mean drew an even grey sheen like
// brushed metal. A pixel takes a Poisson count of glinting facets about the number expected from its area, the slope
// density at the half vector (`glitter_density`) and the slope area that mirrors part of the sun's disc, and the
// mean is scaled by the count over that number, so the mean is kept. Counts are drawn on cells laid on the world
// about a pixel across (the two power-of-two sizes about it, blended), each held for a quarter of a second on its own
// phase and faded into the next. Where a pixel spans only a few facets the waves it resolves draw the glints and the
// mean stands. Returns the multiple of the mean at grid point `xz` for a pixel of `area` m², `time` the waves' clock.
fn glitter_glints(xz: vec2<f32>, area: f32, density: f32, time: f32) -> f32 {
    let level = log2(sqrt(max(area, 1e-8)));
    let lower = floor(level);
    let size = exp2(lower);
    let glints = mix(glint_cell(xz, size, density, time), glint_cell(xz, size * 2.0, density, time), level - lower);
    return mix(1.0, glints, smoothstep(GLINT_FADE_START, GLINT_FADE_END, sqrt(area)));
}

// Independent glinting facets per square metre (wavelets about 5 cm apart).
const GLINT_FACETS: f32 = 400.0;
// The slope area whose facets mirror part of the game's 1.4° sun disc: π (0.7° / 2)².
const GLINT_DISC_SLOPES: f32 = 1.17e-4;
// Draws a second, the last share of each faded into the next.
const GLINT_TWINKLE: f32 = 4.0;
const GLINT_HOLD: f32 = 0.6;
// Above this many glints a cell draws the mean; below the smallest, too.
const GLINT_DENSE: f32 = 4096.0;
const GLINT_FEWEST: f32 = 1e-5;
// Pixel sizes (m) over which the glints take over from the mean.
const GLINT_FADE_START: f32 = 0.1;
const GLINT_FADE_END: f32 = 0.4;

// The multiple of the mean a cell `size` metres across draws.
fn glint_cell(xz: vec2<f32>, size: f32, density: f32, time: f32) -> f32 {
    let expected = density * GLINT_DISC_SLOPES * GLINT_FACETS * size * size;
    if expected > GLINT_DENSE || expected < GLINT_FEWEST {
        return 1.0;
    }
    let cell = floor(xz / size);
    let clock = time * GLINT_TWINKLE + glint_hash(cell, 17u);
    let slot = floor(clock);
    let now = glint_count(expected, cell, glint_salt(slot));
    let next = glint_count(expected, cell, glint_salt(slot + 1.0));
    return mix(now, next, smoothstep(GLINT_HOLD, 1.0, clock - slot)) / expected;
}

fn glint_salt(slot: f32) -> u32 {
    return u32(i32(slot) & 0xffff) * 977u + 3u;
}

// A uniform draw in (0, 1) for a cell and a salt (integer hash).
fn glint_hash(cell: vec2<f32>, salt: u32) -> f32 {
    var h = (bitcast<u32>(i32(cell.x)) * 0x8da6b343u) ^ (bitcast<u32>(i32(cell.y)) * 0xd8163841u) ^ (salt * 0xcb1ab31fu);
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return (f32(h >> 8u) + 0.5) / 16777216.0;
}

// A Poisson count about `expected` from the cell's draw: by its distribution up to ten, Gaussian above.
fn glint_count(expected: f32, cell: vec2<f32>, salt: u32) -> f32 {
    let u = glint_hash(cell, salt);
    if expected > 10.0 {
        let v = glint_hash(cell, salt + 7919u);
        return max(expected + sqrt(expected) * sqrt(-2.0 * log(u)) * cos(2.0 * PI * v), 0.0);
    }
    var p = exp(-expected);
    var cdf = p;
    var k = 0.0;
    for (var i = 1; i < 40; i++) {
        if u <= cdf {
            break;
        }
        p *= expected / f32(i);
        cdf += p;
        k = f32(i);
    }
    return k;
}

fn luminance(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// `radiance` unchanged well below `knee` (in luminance) and approaching it above.
fn soft_knee(radiance: vec3<f32>, knee: f32) -> vec3<f32> {
    return radiance / (1.0 + luminance(radiance) / max(knee, 1e-4));
}

// `radiance` unchanged up to `ceiling` (in luminance) and held there above: the cap on a glint, whose one pixel draws
// the glitter of many (`glitter_glints`). Uncapped, a lone glint near the ship came out a hundred times the glitter's
// knee and the bloom spread it into a round halo.
fn luminance_ceiling(radiance: vec3<f32>, ceiling: f32) -> vec3<f32> {
    return radiance * min(1.0, ceiling / max(luminance(radiance), 1e-6));
}
