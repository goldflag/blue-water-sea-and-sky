// Screen-space reflection of the opaque scene for one water fragment: ports `screenSpaceReflection` of
// the web game's `ocean/screen/reflections.ts` (McGuire & Mara, "Efficient GPU Screen-Space Ray Tracing", JCGT 2014), line
// for line. The ray is clipped to the near plane and then to the viewport before its step budget is spread along it;
// depth is interpolated as 1/w; the step that first passes behind the depth buffer (or over an object's edge) is
// refined by bisection and accepted within a depth-proportional thickness; sky and far-plane pixels never occlude.
//
// The copies hold only the opaque scene (black and far where the sky will be, and the hull below the waterline), so
// every read of the image counts only where the copy holds a surface above the water point the ray left and above the
// sea drawn where the ray struck (a hull's red bottom is under it there: the ray would have met the water first); the
// rest of the image is sky, and its share goes back to the caller's sky reflection. Everything is in view space (the
// camera looking down −Z), screen positions in pixels with y down, as three's screen UV.
#define_import_path naval_ocean::screen_reflection

#import naval::camera::camera
#import naval_ocean::waves::displacement
#import naval_ocean::wake::wake_height

// The depth copy's tiles holding a surface, summed (`crate::reflection_tiles`): the table's width and height in tiles + 1,
// then its rows. The surface's pipeline binds it in its own group.
@group(1) @binding(15) var<storage, read> reflection_tiles: array<u32>;

// Bisections of the step that crossed the scene: 1/256 of a step.
const REFINEMENTS: i32 = 8;
// Crossings refined per ray at most.
const SEARCHES: i32 = 3;
// Steps whose depths are read at once (the four lanes of a vec4).
const MARCH_BATCH: i32 = 4;
// Pixels per tile edge of the depth copy's tile table (`crate::reflection_tiles::TILE`).
const REFLECTION_TILE: u32 = 16u;
// A refined hit may lie behind the scene by this share of its depth (plus the ray's own depth across the bracket).
const THICKNESS: f32 = 0.02;
// Metres below the water point over which a read of the image fades out of the reflection.
const SUBMERGED: f32 = 0.5;
// Metres about the sea drawn where the ray struck over which a read of the image fades in: what lies below that water
// (a hull's boot-topping and bottom, which the copy holds) is hidden from the ray by it.
const WATERLINE_BELOW: f32 = 0.05;
const WATERLINE_ABOVE: f32 = 0.35;
// The vertex spacing (m) the sea's height is read at there: every cascade, its short waves included.
const WATERLINE_SPACING: f32 = 0.5;
// Screen-edge fade, as a share of the viewport.
const EDGE: f32 = 0.06;
// Longest smear per standard deviation, as a share of the viewport.
const SMEAR_LIMIT: f32 = 0.04;
// A smeared read counts where the copy's surface there lies within `SMEAR_SLACK` metres, `SMEAR_TILT` of the smear's
// rise and `SMEAR_DEPTH` of its depth of where the turned ray meets the struck wall, fading out over as much again:
// behind it the turned ray passes over the surface, in front of it the surface hides the wall. Above a hull side's image
// the screen holds her deck, seen from above, which no ray rising off the water meets.
const SMEAR_SLACK: f32 = 0.5;
const SMEAR_TILT: f32 = 0.1;
const SMEAR_DEPTH: f32 = 0.005;
// The turned rays' elevations are kept within these (radians).
const SMEAR_LOWEST: f32 = -0.2;
const SMEAR_HIGHEST: f32 = 1.3;

// What the trace returns: the scene's colour where the ray hit (rgb, linear HDR; black on a miss) and the trust in
// it (a: 0 on a miss, fading at screen edges, near the longest ray and for rays heading back toward the camera,
// times the share of the smeared image that sees a surface).
struct ScreenReflection {
    color: vec3<f32>,
    confidence: f32,
}

// The clipped ray's state, shared by the probes.
struct Ray {
    s_start: vec2<f32>,
    s_stop: vec2<f32>,
    q_start: vec3<f32>,
    q_stop: vec3<f32>,
    k_start: f32,
    k_stop: f32,
}

struct Probe {
    along: f32,
    scene: f32,
    sky: bool,
    behind: bool,
}

// A depth copy read texel-exact at screen `uv` (the web's nearest-filtered depth texture), clamped to the viewport.
fn depth_at(depth: texture_depth_2d, uv: vec2<f32>) -> f32 {
    let size = vec2<i32>(textureDimensions(depth));
    let texel = clamp(vec2<i32>(floor(uv * vec2<f32>(size))), vec2<i32>(0), size - vec2<i32>(1));
    return textureLoad(depth, texel, 0);
}

// Metres in front of the camera of a reversed depth (three's `perspectiveDepthToViewZ`, negated).
fn view_distance(depth: f32) -> f32 {
    return camera.near * camera.far / (depth * (camera.far - camera.near) + camera.near);
}

// The ray's depth (metres in front of the camera) at screen fraction `t` of the clipped ray.
fn along_at(ray: Ray, t: f32) -> f32 {
    return -mix(ray.q_start.z, ray.q_stop.z, t) / mix(ray.k_start, ray.k_stop, t);
}

// The depth copy under screen fraction `t` of the ray.
fn depth_along(ray: Ray, depth: texture_depth_2d, t: f32) -> f32 {
    return depth_at(depth, mix(ray.s_start, ray.s_stop, t) * camera.viewport.zw);
}

// The ray at screen fraction `t` against `depth`, the copy read there ([`depth_along`]). Reading apart from judging
// lets a caller issue several reads before it waits on any.
fn probe_at(ray: Ray, t: f32, depth: f32) -> Probe {
    var out: Probe;
    out.along = along_at(ray, t);
    out.scene = view_distance(depth);
    // Sky and far-plane pixels never occlude, even behind the longest ray.
    out.sky = out.scene >= camera.far * 0.999;
    out.behind = out.along > out.scene && !out.sky;
    return out;
}

// Whether a probe inside a refined bracket lies past the crossing: behind the scene, or past the edge being searched.
fn crossed(inner: Probe, edge: bool, front_scene: f32) -> bool {
    return inner.behind || (edge && inner.along > front_scene);
}

// A view-space point's height in the world (the drawn curve taken off, `naval::camera::curved`).
fn world_height(point: vec3<f32>) -> f32 {
    let world = (camera.inverse_view * vec4<f32>(point, 1.0)).xyz;
    let across = world.xz - camera.position.xz;
    return world.y + camera.curvature * dot(across, across);
}

// Whether the copies hold a surface above the water point `origin` at screen `uv`, where a ray turned up the struck wall
// by `rise` metres meets it (`expected`, view space): not sky or the far plane, not below the water point (by up to
// SUBMERGED), not under `water`, the sea's height where the ray struck, and on the wall where the turned ray meets it.
fn seen(depth: texture_depth_2d, uv: vec2<f32>, origin: vec3<f32>, up: vec3<f32>, water: f32, expected: vec3<f32>,
    rise: f32) -> f32 {
    let unprojected = camera.inverse_projection * vec4<f32>(uv.x * 2.0 - 1.0, uv.y * -2.0 + 1.0, depth_at(depth, uv), 1.0);
    let point = unprojected.xyz / unprojected.w;
    let above = smoothstep(water - WATERLINE_BELOW, water + WATERLINE_ABOVE, world_height(point));
    let slack = SMEAR_SLACK + abs(rise) * SMEAR_TILT - expected.z * SMEAR_DEPTH;
    let met = 1.0 - smoothstep(slack, slack * 2.0, distance(expected, point));
    return select(0.0, smoothstep(-SUBMERGED, 0.0, dot(point - origin, up)) * above * met, -point.z < camera.far * 0.999);
}

// The summed tile table at (`x`, `y`) of a table `width` wide.
fn tiles_at(x: i32, y: i32, width: i32) -> u32 {
    return reflection_tiles[2 + y * width + x];
}

// Whether the depth copy holds any surface in the screen box between pixel positions `a` and `b`, from its summed tile
// table: the box of the texels `depth_at` reads between them, a texel wider each way for the rounding along the way. A
// table that is not this copy's (the placeholder) says yes.
fn surface_between(depth: texture_depth_2d, a: vec2<f32>, b: vec2<f32>) -> bool {
    let size = vec2<i32>(textureDimensions(depth));
    let tile = i32(REFLECTION_TILE);
    let counts = (size + vec2<i32>(tile - 1)) / tile;
    let width = i32(reflection_tiles[0]);
    if width != counts.x + 1 || i32(reflection_tiles[1]) != counts.y + 1 {
        return true;
    }
    let scale = camera.viewport.zw * vec2<f32>(size);
    let low = clamp(vec2<i32>(floor(min(a, b) * scale)) - 1, vec2<i32>(0), size - 1);
    let high = clamp(vec2<i32>(floor(max(a, b) * scale)) + 1, vec2<i32>(0), size - 1);
    let first = low / tile;
    let last = high / tile + 1;
    // Wrapping arithmetic: the four corners give the tiles inside the box.
    let marked = tiles_at(last.x, last.y, width) - tiles_at(first.x, last.y, width) - tiles_at(last.x, first.y, width)
        + tiles_at(first.x, first.y, width);
    return marked != 0u;
}

fn pixel_of(clip: vec4<f32>, k: f32) -> vec2<f32> {
    return vec2<f32>(clip.x * k * 0.5 + 0.5, clip.y * k * -0.5 + 0.5) * camera.viewport.xy;
}

fn exit_axis(d: f32, start: f32, stop: f32, extent: f32) -> f32 {
    return select((clamp(stop, 0.5, extent - 0.5) - start) / d, 1.0, abs(d) < 1e-3);
}

// Trace `direction` (view space, normalised here; below the horizon it is mirrored about the mean surface) from the
// water fragment at view-space `position` over `color_copy` and `depth_copy` (whose surfaces `reflection_tiles` sums by
// tile), at most `max_distance` metres in `steps` steps. Rough water smears the image: the ray turned up or down in its
// vertical plane by deviations of `spread` (radians) meets the struck wall, taken upright, higher or lower, read over four
// taps centred `blur_centre` deviations up from the hit; `spread` 0 reads the hit alone. (`blur_up`, the lobe's own up
// in view space, smeared along the screen before: the screen above a hull side holds her deck, which reflected.)
fn screen_space_reflection(
    color_copy: texture_2d<f32>,
    color_sampler: sampler,
    depth_copy: texture_depth_2d,
    position: vec3<f32>,
    direction: vec3<f32>,
    enabled: bool,
    max_distance: f32,
    steps: i32,
    blur_up: vec3<f32>,
    spread: f32,
    blur_centre: f32,
) -> ScreenReflection {
    var result: ScreenReflection;
    result.color = vec3<f32>(0.0);
    result.confidence = 0.0;
    let up = (camera.view * vec4<f32>(0.0, 1.0, 0.0, 0.0)).xyz;
    // Below the horizon: mirrored about the mean surface, as the sea it strikes sends it back up.
    let reflected = normalize(direction);
    let mirrored = reflected - up * (min(dot(reflected, up), 0.0) * 2.0);
    // Depth holds only camera-facing surfaces; rays coming back toward the camera would see their backs. A ray this
    // trust leaves nothing to would add nothing, wherever it hit: it is not traced.
    let facing = 1.0 - smoothstep(0.0, 0.5, mirrored.z);
    if !enabled || dot(mirrored, up) <= 0.0 || steps <= 0 || facing <= 0.0 {
        return result;
    }
    let origin = position;
    // A ray heading toward the camera ends just in front of the near plane (w stays positive).
    let to_near = (camera.near * -1.001 - origin.z) / mirrored.z;
    let length_along = select(max_distance, min(to_near, max_distance), mirrored.z > 1e-6);
    let end = origin + mirrored * length_along;
    let clip_start = camera.projection * vec4<f32>(origin, 1.0);
    let clip_end = camera.projection * vec4<f32>(end, 1.0);
    let k_start = 1.0 / clip_start.w;
    let k_end = 1.0 / clip_end.w;
    let s_start = pixel_of(clip_start, k_start);
    let s_end = pixel_of(clip_end, k_end);
    // Clip to the viewport: pixel position, 1/w and P/w are linear on screen and clip by the same fraction.
    let delta = s_end - s_start;
    let size = camera.viewport.xy;
    let exit = clamp(min(exit_axis(delta.x, s_start.x, s_end.x, size.x), exit_axis(delta.y, s_start.y, s_end.y, size.y)), 0.0, 1.0);
    var ray: Ray;
    ray.s_start = s_start;
    ray.s_stop = s_start + delta * exit;
    ray.q_start = origin * k_start;
    ray.q_stop = mix(ray.q_start, end * k_end, exit);
    ray.k_start = k_start;
    ray.k_stop = mix(k_start, k_end, exit);
    // Every step and bisection reads the copy on the segment between the clipped ray's ends: where no surface lies in
    // its box, each would read sky and the trace would find nothing.
    if !surface_between(depth_copy, ray.s_start, ray.s_stop) {
        return result;
    }

    // The last sample in front of the scene and the depth it saw there (the far plane over sky).
    var hit = -1.0;
    var front = true;
    var last_front = 0.0;
    var front_scene = camera.far;
    var searches = 0;
    var traced = false;
    // The steps go in batches whose depths are read together, so the reads' latencies overlap; each batch is then
    // judged step by step as one march would.
    for (var first = 0; first < steps && !traced; first += MARCH_BATCH) {
        var depths = vec4<f32>(
            depth_along(ray, depth_copy, f32(first + 1) / f32(steps)),
            depth_along(ray, depth_copy, f32(first + 2) / f32(steps)),
            depth_along(ray, depth_copy, f32(first + 3) / f32(steps)),
            depth_along(ray, depth_copy, f32(first + 4) / f32(steps)),
        );
        for (var k = 0; k < MARCH_BATCH; k += 1) {
            let i = first + k;
            if i >= steps {
                break;
            }
            let t = f32(i + 1) / f32(steps);
            let sample = probe_at(ray, t, depths[k]);
            // A step that lands on a much farther surface (or sky) while deeper than the surface it left passed over
            // that surface's edge and may have been behind it on the way: search the edge as well.
            let edge = sample.along > front_scene && sample.scene > front_scene * (1.0 + THICKNESS);
            if front && (sample.behind || edge) && searches < SEARCHES {
                searches += 1;
                var low = last_front;
                var high = t;
                // The probe at `high`: the step's own until a bisection moves it.
                var refined = sample;
                // Two bisections a round: the middle and both quarter points are read together, then the middle
                // chooses its half and that half's quarter point the half of it, as two bisections one after the
                // other would (the same points, so the same bracket).
                for (var r = 0; r < REFINEMENTS; r += 2) {
                    let middle = (low + high) * 0.5;
                    let lower = (low + middle) * 0.5;
                    let upper = (middle + high) * 0.5;
                    let at_middle = depth_along(ray, depth_copy, middle);
                    let at_lower = depth_along(ray, depth_copy, lower);
                    let at_upper = depth_along(ray, depth_copy, upper);
                    let centre = probe_at(ray, middle, at_middle);
                    if crossed(centre, edge, front_scene) {
                        high = middle;
                        refined = centre;
                        let quarter = probe_at(ray, lower, at_lower);
                        if crossed(quarter, edge, front_scene) {
                            high = lower;
                            refined = quarter;
                        } else {
                            low = lower;
                        }
                    } else {
                        low = middle;
                        let quarter = probe_at(ray, upper, at_upper);
                        if crossed(quarter, edge, front_scene) {
                            high = upper;
                            refined = quarter;
                        } else {
                            low = upper;
                        }
                    }
                }
                // Accept a surface within the thickness plus the depth the ray covers across the final bracket.
                let span = refined.along - along_at(ray, low);
                if refined.behind && refined.along - refined.scene < refined.scene * THICKNESS + span {
                    hit = high;
                    traced = true;
                    break;
                }
            }
            front = !sample.behind;
            if front {
                last_front = t;
                front_scene = select(sample.scene, camera.far, sample.sky);
            }
        }
    }
    if hit < 0.0 {
        return result;
    }
    let uv = mix(ray.s_start, ray.s_stop, hit) / size;
    let point = mix(ray.q_start, ray.q_stop, hit) / mix(ray.k_start, ray.k_stop, hit);
    // The surface the copy holds there: the ray's own point may lie behind it by the thickness accepted.
    let unprojected = camera.inverse_projection * vec4<f32>(uv.x * 2.0 - 1.0, uv.y * -2.0 + 1.0, depth_at(depth_copy, uv), 1.0);
    let wall = unprojected.xyz / unprojected.w;
    // The sea where the ray struck (the waves and the hulls' sea at the grid point under it, and the wake's crests).
    let struck = (camera.inverse_view * vec4<f32>(wall, 1.0)).xz;
    let water = displacement(struck, WATERLINE_SPACING).y + wake_height(struck);
    let edges = smoothstep(0.0, EDGE, min(uv.x, 1.0 - uv.x)) * smoothstep(0.0, EDGE, min(uv.y, 1.0 - uv.y));
    let range = 1.0 - smoothstep(0.7, 1.0, distance(point, origin) / max_distance);
    // The image over the smear rough water gives it: the ray turned up or down in its vertical plane by the lobe's
    // deviations meets the struck wall (taken upright) higher or lower, at most SMEAR_LIMIT of the viewport a deviation.
    let reach = wall - origin;
    let climb = dot(reach, up);
    let across = max(length(reach - up * climb), 1e-3);
    let elevation = atan2(climb, across);
    let deviation = across * tan(clamp(elevation + spread, SMEAR_LOWEST, SMEAR_HIGHEST)) - climb;
    let lifted = camera.projection * vec4<f32>(wall + up * deviation, 1.0);
    let smear = vec2<f32>(lifted.x / lifted.w * 0.5 + 0.5, lifted.y / lifted.w * -0.5 + 0.5) - uv;
    let shortened = min(1.0, SMEAR_LIMIT / max(length(smear), 1e-6));
    // ±0.5 and ±1.5 deviations of the smear with the normal density's weights.
    let taps = array<vec2<f32>, 4>(vec2<f32>(-1.5, 0.1344), vec2<f32>(-0.5, 0.3656), vec2<f32>(0.5, 0.3656), vec2<f32>(1.5, 0.1344));
    var cover = 0.0;
    var color = vec3<f32>(0.0);
    for (var j = 0; j < 4; j += 1) {
        let turn = (taps[j].x + blur_centre) * shortened;
        let rise = across * tan(clamp(elevation + spread * turn, SMEAR_LOWEST, SMEAR_HIGHEST)) - climb;
        let expected = wall + up * rise;
        let clip = camera.projection * vec4<f32>(expected, 1.0);
        let at = clamp(vec2<f32>(clip.x / clip.w * 0.5 + 0.5, clip.y / clip.w * -0.5 + 0.5), vec2<f32>(0.0), vec2<f32>(1.0));
        let share = seen(depth_copy, at, origin, up, water, expected, rise) * taps[j].y;
        cover += share;
        color += textureSampleLevel(color_copy, color_sampler, at, 0.0).rgb * share;
    }
    result.color = color / max(cover, 1e-4);
    result.confidence = edges * range * facing * cover;
    return result;
}
