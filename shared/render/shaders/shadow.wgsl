// The sun's shadow maps, sampled. Ports three r185's ShadowNode (receiver offset along the
// normal, shadow coordinates with the depth bias, frustum test, intensity), `FocusShadowNode.setup`'s
// blend of the near, wide and view maps, and the web game's `WaterShadows.ts` for the sea (with the
// view maps, which it left off). The soft filter is the client's own (the web's was three's PCFSoft,
// a 3x3 box of bilinear comparisons, whose edges stepped texel by texel as a hull moved): a cubic
// B-spline over 6x6 texels from nine gathers. On High and Ultra, three land maps (cascades fitted to the camera's view
// of the land out to 4 km) carry the land's, its trees' and its buildings' shadows beyond the near and wide maps. Aboard a
// hull (a walk, her bridge view) an aboard map holds her beyond the near map, between it and the wide map.
// Its Rust twin is `naval_render::shadows` (`ShadowUniforms`); keep the two in order.
//
// Soft occluders (smoke, which a depth map cannot hold) sit beside them at bindings 8 and 9: `sun_shadow` and
// `water_sun_shadow` multiply in what they let through; `sun_shadow_maps` is the depth maps alone.
//
// The bindings sit in group 0 beside the camera, so any scene or compute pipeline built with
// the renderer's camera layout can import this module. Caster pipelines cannot: their group 0
// is the shadow camera alone.
#define_import_path naval::shadow

struct ShadowMap {
    // World to (u, v, reversed depth), v down.
    matrix: mat4x4<f32>,
    // Texels per edge, 1 / that, normal bias (m), depth bias (added to the receiver's depth).
    params: vec4<f32>,
    // On (0 or 1), half width, half height (m), unused.
    state: vec4<f32>,
    // A view map's hulls in its reversed depth: where they begin (nearer the light) and end, and 1 when it has them
    // (0 for the near, aboard and wide maps), unused. The sea takes only their shadows from it (`water_view_shadow`).
    hulls: vec4<f32>,
}

// The maps' count (`naval_render::shadows::MAP_COUNT`), and the first of the land maps (`LAND_FIRST`).
const SHADOW_MAPS: u32 = 8u;
const SHADOW_LAND_FIRST: u32 = 5u;
// The square maps in the order a pixel takes them, each what those before it leave: near, aboard (`ABOARD`: the hull
// the camera stands aboard past the near map, off otherwise), wide, view 1, view 2. The first three hold the land's
// casters themselves; the land maps take over beyond them.
const SHADOW_ORDER = array<u32, 5>(0u, 2u, 1u, 3u, 4u);
const SHADOW_HELD: u32 = 3u;

struct Shadows {
    // Near, wide, aboard, view 1, view 2, land 1, land 2, land 3.
    maps: array<ShadowMap, 8>,
    // The near map's centre (xyz, unsnapped) and half-width (w).
    focus: vec4<f32>,
    // Intensity (0 when off), soft filter (0 or 1), enabled (0 or 1), any land map on (0 or 1).
    settings: vec4<f32>,
    // Toward the light (xyz).
    toward_light: vec4<f32>,
    // Aboard a hull, the band along the view over which the near map hands her to the aboard map: dot(seam.xyz, p) +
    // seam.w runs from 0 to 1 across it; (0, 0, 0, -1) otherwise (`ShadowUniforms::seam`).
    seam: vec4<f32>,
}

@group(0) @binding(1) var<uniform> shadows: Shadows;
// Layers 0, 1, 2, 3: the near, aboard and two view maps; 4, 5, 6: the land maps, when on (map i past the near map is
// layer i - 1).
@group(0) @binding(2) var shadow_cascades: texture_depth_2d_array;
@group(0) @binding(3) var shadow_wide: texture_depth_2d;
// GreaterEqual (reversed depth), linear: one comparison is a 2x2 bilinear one.
@group(0) @binding(4) var shadow_sampler: sampler_comparison;

// Soft occluders in the sun's light (smoke), which the depth maps cannot hold: `naval_render::shadows::SunOccluders`.
struct SunOccluders {
    // World to the map's u and v, and to depth along the light (m): dot(axis.xyz, p) + axis.w.
    axis_u: vec4<f32>,
    axis_v: vec4<f32>,
    axis_d: vec4<f32>,
    // The depth the texels' fronts count back from, the depth their backs count on from, unused, on (0 or 1).
    params: vec4<f32>,
}
// Two planes of SUN_OCCLUDER_SIZE^2 texels, row by row, half floats: first one u32 a texel, how far before params.x
// the first occluder begins and how far after params.y the last ends; then two texels a u32, the optical depth
// between them.
@group(0) @binding(8) var<storage, read> sun_occluder_texels: array<u32>;
@group(0) @binding(9) var<uniform> sun_occluders: SunOccluders;
const SUN_OCCLUDER_SIZE: i32 = 256;

// FocusShadowNode's fade at a square map's edge (of its half-width).
const SHADOW_EDGE_START: f32 = 0.9;
const SHADOW_EDGE_END: f32 = 0.98;

fn shadows_enabled() -> bool {
    return shadows.settings.z > 0.5;
}

// Whether any land map samples this frame.
fn shadow_land_on() -> bool {
    return shadows.settings.w > 0.5;
}

// Toward the light the maps were drawn for.
fn shadow_light_direction() -> vec3<f32> {
    return shadows.toward_light.xyz;
}

// Shadow coordinates of a receiver in map `index`: three's `shadowMatrix * (position + normal *
// normalBias)`, then the depth bias toward the light.
fn shadow_coord(index: u32, position: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    let map = shadows.maps[index];
    let projected = map.matrix * vec4<f32>(position + normal * map.params.z, 1.0);
    let coord = projected.xyz / projected.w;
    return vec3<f32>(coord.xy, coord.z + map.params.w);
}

// The soft filter's knot spacing in texels: a cubic B-spline stretched this much spans six texels
// (standard deviation 0.87 texel), and its weights change smoothly as a receiver moves across the
// map, so a shadow edge glides where a box filter's would step.
const SHADOW_FILTER_SCALE: f32 = 1.5;

fn shadow_bspline(x: f32) -> f32 {
    let a = abs(x);
    if a < 1.0 {
        return (4.0 - 6.0 * a * a + 3.0 * a * a * a) / 6.0;
    }
    let b = max(2.0 - a, 0.0);
    return b * b * b / 6.0;
}

// The filter over nine gathers: 2x2 texel blocks at offsets -2, 0 and 2 in x and y (row by row,
// y from -2) about the texel below the sample, `f` the sample's place past that texel's centre.
fn shadow_filter_combine(gathers: array<vec4<f32>, 9>, f: vec2<f32>) -> f32 {
    var g = gathers;
    var wx: array<f32, 6>;
    var wy: array<f32, 6>;
    var sx = 0.0;
    var sy = 0.0;
    for (var k = 0; k < 6; k++) {
        wx[k] = shadow_bspline((f32(k) - 2.0 - f.x) / SHADOW_FILTER_SCALE);
        wy[k] = shadow_bspline((f32(k) - 2.0 - f.y) / SHADOW_FILTER_SCALE);
        sx += wx[k];
        sy += wy[k];
    }
    var sum = 0.0;
    for (var by = 0; by < 3; by++) {
        for (var bx = 0; bx < 3; bx++) {
            // A gather's texels: x (low u, high v), y (high u, high v), z (high u, low v), w (low u, low v).
            let c = g[by * 3 + bx];
            let x0 = wx[bx * 2];
            let x1 = wx[bx * 2 + 1];
            let y0 = wy[by * 2];
            let y1 = wy[by * 2 + 1];
            sum += (c.w * x0 + c.z * x1) * y0 + (c.x * x0 + c.y * x1) * y1;
        }
    }
    return sum / (sx * sy);
}

fn shadow_filter_wide(coord: vec3<f32>, size: f32) -> f32 {
    let p = coord.xy * size - 0.5;
    let base = floor(p);
    let uv = (base + 1.0) / size;
    let z = coord.z;
    return shadow_filter_combine(array<vec4<f32>, 9>(
        textureGatherCompare(shadow_wide, shadow_sampler, uv, z, vec2<i32>(-2, -2)),
        textureGatherCompare(shadow_wide, shadow_sampler, uv, z, vec2<i32>(0, -2)),
        textureGatherCompare(shadow_wide, shadow_sampler, uv, z, vec2<i32>(2, -2)),
        textureGatherCompare(shadow_wide, shadow_sampler, uv, z, vec2<i32>(-2, 0)),
        textureGatherCompare(shadow_wide, shadow_sampler, uv, z, vec2<i32>(0, 0)),
        textureGatherCompare(shadow_wide, shadow_sampler, uv, z, vec2<i32>(2, 0)),
        textureGatherCompare(shadow_wide, shadow_sampler, uv, z, vec2<i32>(-2, 2)),
        textureGatherCompare(shadow_wide, shadow_sampler, uv, z, vec2<i32>(0, 2)),
        textureGatherCompare(shadow_wide, shadow_sampler, uv, z, vec2<i32>(2, 2)),
    ), p - base);
}

fn shadow_filter_cascade(layer: i32, coord: vec3<f32>, size: f32) -> f32 {
    let p = coord.xy * size - 0.5;
    let base = floor(p);
    let uv = (base + 1.0) / size;
    let z = coord.z;
    return shadow_filter_combine(array<vec4<f32>, 9>(
        textureGatherCompare(shadow_cascades, shadow_sampler, uv, layer, z, vec2<i32>(-2, -2)),
        textureGatherCompare(shadow_cascades, shadow_sampler, uv, layer, z, vec2<i32>(0, -2)),
        textureGatherCompare(shadow_cascades, shadow_sampler, uv, layer, z, vec2<i32>(2, -2)),
        textureGatherCompare(shadow_cascades, shadow_sampler, uv, layer, z, vec2<i32>(-2, 0)),
        textureGatherCompare(shadow_cascades, shadow_sampler, uv, layer, z, vec2<i32>(0, 0)),
        textureGatherCompare(shadow_cascades, shadow_sampler, uv, layer, z, vec2<i32>(2, 0)),
        textureGatherCompare(shadow_cascades, shadow_sampler, uv, layer, z, vec2<i32>(-2, 2)),
        textureGatherCompare(shadow_cascades, shadow_sampler, uv, layer, z, vec2<i32>(0, 2)),
        textureGatherCompare(shadow_cascades, shadow_sampler, uv, layer, z, vec2<i32>(2, 2)),
    ), p - base);
}

// The unfiltered comparison of map `index` at shadow coordinates `coord`, through the linear
// comparison sampler (2x2 bilinear), or the soft filter when `soft`. 1 is lit.
fn shadow_compare(index: u32, coord: vec3<f32>, soft: bool) -> f32 {
    let size = shadows.maps[index].params.x;
    if index == 1u {
        if soft {
            return shadow_filter_wide(coord, size);
        }
        return textureSampleCompareLevel(shadow_wide, shadow_sampler, coord.xy, coord.z);
    }
    let layer = select(i32(index) - 1, 0, index == 0u);
    if soft {
        return shadow_filter_cascade(layer, coord, size);
    }
    return textureSampleCompareLevel(shadow_cascades, shadow_sampler, coord.xy, layer, coord.z);
}

// How much sun reaches `position` through map `index` alone: three's ShadowNode output,
// `mix(1, filter, intensity)`, 1 outside the map's frustum. `normal` is the receiver's world
// normal (unit), along which the map's normal bias offsets it.
fn shadow_map_visibility(index: u32, position: vec3<f32>, normal: vec3<f32>) -> f32 {
    let coord = shadow_coord(index, position, normal);
    if any(coord.xy < vec2<f32>(0.0)) || any(coord.xy > vec2<f32>(1.0)) || coord.z > 1.0 {
        return 1.0;
    }
    let filtered = shadow_compare(index, coord, shadows.settings.y > 0.5);
    return mix(1.0, filtered, shadows.settings.x);
}

// How fully square map `index` holds `position`: 1 inside, fading to 0 at its edge and outside
// its depth range. Aboard a hull the near map also fades along the view, across the band where the aboard map takes
// her over (`seam`), so her shadows soften gradually there, with no line where the two maps meet.
fn shadow_cover(index: u32, position: vec3<f32>) -> f32 {
    let projected = shadows.maps[index].matrix * vec4<f32>(position, 1.0);
    let coord = projected.xyz / projected.w;
    let edge = max(abs(coord.x - 0.5), abs(coord.y - 0.5)) * 2.0;
    let depth = select(0.0, 1.0, coord.z >= 0.0 && coord.z <= 1.0);
    var cover = (1.0 - smoothstep(SHADOW_EDGE_START, SHADOW_EDGE_END, edge)) * depth;
    if index == 0u {
        cover *= 1.0 - smoothstep(0.0, 1.0, dot(shadows.seam.xyz, position) + shadows.seam.w);
    }
    return cover;
}

// The near map's share of the shadow at `position`: 1 inside its square, fading to the wide map
// toward its edge as every square map does (the web's `FocusShadowNode.nearShare` faded over a
// sphere inside the square, leaving its corners and a fifth of its width unused).
fn shadow_near_share(position: vec3<f32>) -> f32 {
    return shadow_cover(0u, position) * shadows.maps[0].state.x;
}

// A non-negative half float from its bits (subnormals read as 0), without `unpack2x16float` (which naga allows only
// with a 16-bit float capability).
fn occluder_half(bits: u32) -> f32 {
    let magnitude = bits & 0x7fffu;
    return select(bitcast<f32>((magnitude + 0x1c000u) << 13u), 0.0, magnitude < 0x400u);
}

// What the soft occluders let through to texel (x, y) at `depth` along the light: their optical depth spread evenly
// from the first one's front to the last one's back.
fn occluder_texel(x: i32, y: i32, depth: f32) -> f32 {
    let index = clamp(y, 0, SUN_OCCLUDER_SIZE - 1) * SUN_OCCLUDER_SIZE + clamp(x, 0, SUN_OCCLUDER_SIZE - 1);
    let pair = sun_occluder_texels[SUN_OCCLUDER_SIZE * SUN_OCCLUDER_SIZE + index / 2];
    let optical = occluder_half(select(pair & 0xffffu, pair >> 16u, (index & 1) == 1));
    // Clear air (most texels): the span is not read.
    if optical <= 0.0 {
        return 1.0;
    }
    let span = sun_occluder_texels[index];
    let front = sun_occluders.params.x - occluder_half(span & 0xffffu);
    let back = sun_occluders.params.y + occluder_half(span >> 16u);
    let share = clamp((depth - front) / max(back - front, 1.0), 0.0, 1.0);
    return exp(-optical * share);
}

// The share of the sun's light the soft occluders (smoke) let through to `position`, 0 to 1: four texels filtered
// bilinearly, fading to 1 at the map's rim; 1 with none drawn this frame.
fn occluder_transmittance(position: vec3<f32>) -> f32 {
    if sun_occluders.params.w < 0.5 {
        return 1.0;
    }
    let uv = vec2<f32>(dot(position, sun_occluders.axis_u.xyz) + sun_occluders.axis_u.w,
        dot(position, sun_occluders.axis_v.xyz) + sun_occluders.axis_v.w);
    let edge = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
    if edge <= 0.0 {
        return 1.0;
    }
    let depth = dot(position, sun_occluders.axis_d.xyz) + sun_occluders.axis_d.w;
    let p = uv * f32(SUN_OCCLUDER_SIZE) - 0.5;
    let base = vec2<i32>(floor(p));
    let f = p - floor(p);
    let top = mix(occluder_texel(base.x, base.y, depth), occluder_texel(base.x + 1, base.y, depth), f.x);
    let bottom = mix(occluder_texel(base.x, base.y + 1, depth), occluder_texel(base.x + 1, base.y + 1, depth), f.x);
    // At the shadows' intensity, as the depth maps' shadows are taken.
    return mix(1.0, mix(top, bottom, f.y), clamp(edge * 32.0, 0.0, 1.0) * shadows.settings.x);
}

// Sunlight reaching a surface at `position` with unit world normal `normal`, 0 to 1: each pixel
// samples the first map that covers it (near, aboard, wide, view 1, view 2) and fades to the next
// at that map's edge; beyond the near, aboard and wide maps the land maps too, the darker of theirs and the
// view maps' taken; what no map covers is lit; then what the soft occluders (smoke) let through.
// `FocusShadowNode.setup` without the clouds' transmittance, which the caller multiplies in.
fn sun_shadow(position: vec3<f32>, normal: vec3<f32>) -> f32 {
    if !shadows_enabled() {
        return 1.0;
    }
    return sun_shadow_maps(position, normal) * occluder_transmittance(position);
}

// `sun_shadow` from the depth maps alone, without the soft occluders: for a receiver that is itself one (smoke, which
// shades itself from its own map).
fn sun_shadow_maps(position: vec3<f32>, normal: vec3<f32>) -> f32 {
    if !shadows_enabled() {
        return 1.0;
    }
    // Each map's weight in `SHADOW_ORDER`, and what the near, aboard and wide maps leave.
    var weights = array<f32, 5>(0.0, 0.0, 0.0, 0.0, 0.0);
    var rest = 1.0;
    var beyond = 1.0;
    for (var k = 0u; k < 5u; k++) {
        let i = SHADOW_ORDER[k];
        let share = shadow_cover(i, position) * shadows.maps[i].state.x;
        weights[k] = rest * share;
        rest *= 1.0 - share;
        if k + 1u == SHADOW_HELD {
            beyond = rest;
        }
    }
    // The near, aboard and wide maps' part, then the view maps' and the lit rest beyond them. Each map is read only
    // where it counts.
    var held = 0.0;
    var outside = rest;
    for (var k = 0u; k < 5u; k++) {
        if weights[k] > 0.0 {
            let part = weights[k] * shadow_map_visibility(SHADOW_ORDER[k], position, normal);
            if k < SHADOW_HELD {
                held += part;
            } else {
                outside += part;
            }
        }
    }
    // Beyond the near, aboard and wide maps (which hold the land's casters themselves), the land maps: the darker of theirs and
    // the view maps'. Both may hold the land, so the two are never multiplied.
    if beyond > 0.0 && shadow_land_on() {
        outside = min(outside, beyond * shadow_land(position, normal));
    }
    return held + outside;
}

// A land map's filter with the soft filter on: four bilinear comparisons half a texel either side of the sample, a
// kernel three texels across whose weights change linearly as a receiver moves (an edge glides, never steps), at about
// a third of the soft filter's nine gathers. The land maps hold what stands still in the world (the land, its trees and
// buildings) on texel grids fixed in the world, so their edges have no motion of their own to crawl; and they fill
// every pixel of land beyond the near and wide maps, so their filter is paid across most of a view from the air.
fn shadow_land_compare(index: u32, coord: vec3<f32>) -> f32 {
    let layer = i32(index) - 1;
    let d = 0.5 * shadows.maps[index].params.y;
    let z = coord.z;
    return 0.25 * (textureSampleCompareLevel(shadow_cascades, shadow_sampler, coord.xy + vec2<f32>(-d, -d), layer, z)
        + textureSampleCompareLevel(shadow_cascades, shadow_sampler, coord.xy + vec2<f32>(d, -d), layer, z)
        + textureSampleCompareLevel(shadow_cascades, shadow_sampler, coord.xy + vec2<f32>(-d, d), layer, z)
        + textureSampleCompareLevel(shadow_cascades, shadow_sampler, coord.xy + vec2<f32>(d, d), layer, z));
}

// `shadow_map_visibility` for land map `index`, through its own filter (`shadow_land_compare`), or one bilinear
// comparison with the soft filter off.
fn shadow_land_visibility(index: u32, position: vec3<f32>, normal: vec3<f32>) -> f32 {
    let coord = shadow_coord(index, position, normal);
    if any(coord.xy < vec2<f32>(0.0)) || any(coord.xy > vec2<f32>(1.0)) || coord.z > 1.0 {
        return 1.0;
    }
    var filtered: f32;
    if shadows.settings.y > 0.5 {
        filtered = shadow_land_compare(index, coord);
    } else {
        filtered = textureSampleCompareLevel(shadow_cascades, shadow_sampler, coord.xy, i32(index) - 1, coord.z);
    }
    return mix(1.0, filtered, shadows.settings.x);
}

// How much sun reaches `position` through the land maps alone: each covering what those nearer the camera (finer) leave,
// fading to the next at its edge; 1 where none covers.
fn shadow_land(position: vec3<f32>, normal: vec3<f32>) -> f32 {
    var rest = 1.0;
    var result = 0.0;
    for (var i = SHADOW_LAND_FIRST; i < SHADOW_MAPS; i++) {
        if shadows.maps[i].state.x < 0.5 {
            continue;
        }
        let share = shadow_cover(i, position);
        if share > 0.0 {
            result += rest * share * shadow_land_visibility(i, position, normal);
            rest *= 1.0 - share;
            if rest <= 0.0 {
                break;
            }
        }
    }
    return result + rest;
}

// The share of `position`'s shadow the land maps hold in `sun_shadow`'s weights: beyond the near, aboard and wide maps,
// where a land map covers. A stand-in for the casters the land maps carry (the forest's shade, `naval_terrain::forest`)
// steps aside there.
fn shadow_land_share(position: vec3<f32>) -> f32 {
    if !shadows_enabled() || !shadow_land_on() {
        return 0.0;
    }
    var beyond = 1.0;
    for (var k = 0u; k < SHADOW_HELD; k++) {
        let i = SHADOW_ORDER[k];
        beyond *= 1.0 - shadow_cover(i, position) * shadows.maps[i].state.x;
    }
    var rest = 1.0;
    for (var i = SHADOW_LAND_FIRST; i < SHADOW_MAPS; i++) {
        if shadows.maps[i].state.x > 0.5 {
            rest *= 1.0 - shadow_cover(i, position);
        }
    }
    return beyond * (1.0 - rest);
}

// One map's visibility for the sea (`WaterShadows.receiver`): offset straight up by the map's
// normal bias, lit outside its frustum, filtered as the ships are when `soft`; a view map with
// hulls takes only their shadows (`water_view_shadow`), whatever `soft` asks.
fn water_shadow_map(index: u32, position: vec3<f32>, soft: bool) -> f32 {
    let coord = shadow_coord(index, position, vec3<f32>(0.0, 1.0, 0.0));
    if any(coord < vec3<f32>(0.0)) || any(coord > vec3<f32>(1.0)) {
        return 1.0;
    }
    if shadows.maps[index].hulls.z > 0.5 {
        return mix(1.0, water_view_shadow(index, coord), shadows.settings.x);
    }
    return mix(1.0, shadow_compare(index, coord, soft), shadows.settings.x);
}

// A view map's shadow on the sea at shadow coordinates `coord`, 1 lit: a 2x2 bilinear filter whose
// texels count an occluder only where it lies among the hulls the map was fitted to (between
// `hulls.x` and `hulls.y` in its depth). The land draws itself into the view maps too, clamped
// onto their near plane when it stands sunward, so a distant hull under a fjord wall is shaded;
// read on the water, that shadow would lie only in her square and end at its straight edges
// across the land's shadowless sea beyond the wide map. Three gathers, fewer than the soft
// filter's nine: on the water at a view map's distance a texel is well under a pixel.
fn water_view_shadow(index: u32, coord: vec3<f32>) -> f32 {
    let map = shadows.maps[index];
    let layer = i32(index) - 1;
    let p = coord.xy * map.params.x - 0.5;
    let base = floor(p);
    let f = p - base;
    let uv = (base + 1.0) / map.params.x;
    let lit = textureGatherCompare(shadow_cascades, shadow_sampler, uv, layer, coord.z);
    let past_first = textureGatherCompare(shadow_cascades, shadow_sampler, uv, layer, map.hulls.x);
    let past_last = textureGatherCompare(shadow_cascades, shadow_sampler, uv, layer, map.hulls.y);
    // A texel shades when its occluder is in front of the receiver, no nearer the light than the hulls' first point and
    // no farther than their last.
    let visible = vec4<f32>(1.0) - (vec4<f32>(1.0) - lit) * past_first * (vec4<f32>(1.0) - past_last);
    // A gather's texels: x (low u, high v), y (high u, high v), z (high u, low v), w (low u, low v).
    return mix(mix(visible.w, visible.z, f.x), mix(visible.x, visible.y, f.x), f.y);
}

// Ship shadows on the sea at `position` (`updateWaterShadows`): the maps by the ships' own weights,
// the near map where it covers the sea, then the aboard and wide maps, then the two view maps about the hulls
// beyond it, each fading to the next at its edge, so a distant ship's shadow lies on the water as
// her own deck takes it. (The web game left the view maps off the sea for want of a sampler; here
// they are layers of the near map's array, which the sea already binds.) The sea casts into no
// map, so it cannot shade itself: the receiver offsets only keep a hull's waterline from shading
// the water on her sunlit side. From the view maps the sea takes only the hulls' shadows, not the
// land's (`water_view_shadow`). `quality` is the Graphics menu's water shadows: 0 off, 1 low (one
// bilinear comparison per map), 2 medium (the near and aboard maps filtered as the ships' paint), 3 high
// (the wide map too); the view maps take their own 2x2 filter at each.
fn water_sun_shadow(position: vec3<f32>, quality: u32) -> f32 {
    if !shadows_enabled() || quality == 0u {
        return 1.0;
    }
    // The maps in turn, each taking by its cover what those before it leave (`rest`), as `sun_shadow_maps` weighs
    // them. A pixel the near, aboard or wide map holds whole looks no further, a map that is off is passed over, and
    // each map is read only where it covers: most of the distant sea reads none.
    var rest = 1.0;
    var result = 0.0;
    // The near, aboard and wide maps' part, and what they leave.
    var held = 0.0;
    var beyond = 1.0;
    for (var k = 0u; k < 5u; k++) {
        let i = SHADOW_ORDER[k];
        if rest > 0.0 && shadows.maps[i].state.x >= 0.5 {
            let share = shadow_cover(i, position);
            if share > 0.0 {
                result += rest * share * water_shadow_map(i, position, water_shadow_soft(i, quality));
                rest *= 1.0 - share;
            }
        }
        if k + 1u == SHADOW_HELD {
            held = result;
            beyond = rest;
        }
        if rest <= 0.0 && k + 1u >= SHADOW_HELD {
            break;
        }
    }
    // Beyond them the land maps: the shadows of the land, its trees and its buildings on the water off the shore, the
    // darker of theirs and the view maps' (one bilinear comparison each: there a texel is about a pixel).
    var outside = result - held + rest;
    if beyond > 0.0 && shadow_land_on() {
        outside = min(outside, beyond * water_land_shadow(position));
    }
    return (held + outside) * occluder_transmittance(position);
}

// The land maps' shadow on the sea at `position`, as `shadow_land` weighs them, each map one bilinear comparison.
fn water_land_shadow(position: vec3<f32>) -> f32 {
    var rest = 1.0;
    var result = 0.0;
    for (var i = SHADOW_LAND_FIRST; i < SHADOW_MAPS; i++) {
        if shadows.maps[i].state.x < 0.5 {
            continue;
        }
        let share = shadow_cover(i, position);
        if share > 0.0 {
            result += rest * share * water_shadow_map(i, position, false);
            rest *= 1.0 - share;
            if rest <= 0.0 {
                break;
            }
        }
    }
    return result + rest;
}

// Whether the sea filters map `index` at water-shadow `quality` (`WaterShadows.ts`): High the near,
// aboard and wide maps, Medium the near and aboard ones, Low none. The view maps take their own filter
// (`water_view_shadow`).
fn water_shadow_soft(index: u32, quality: u32) -> bool {
    return ((index == 0u || index == 2u) && quality >= 2u) || (index == 1u && quality >= 3u);
}
