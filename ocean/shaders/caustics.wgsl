// Caustics under the sea (native only; naval-ships' ship.wgsl reads them on what stands below the surface): how the
// sunlight that crosses the surface gathers and thins at a depth, from the wave field's own slopes. A ray entering the
// water where the surface slopes by ∇h leans by (1 − 1/n)·∇h toward the crests, so the light entering at x lands at
// x + s·∇h(x) at depth d (s = d·(1 − 1/n)), and a patch of the surface lights a patch |det(I + s·H)| as large below it
// (H the Hessian of h): the light there is 1 / |det| of what a flat sea gives, taken where the light landing on the
// point entered the sea (two fixed-point steps back up the ray: taken straight above the point instead, the gains
// average a fifth to a half over a flat sea's, and the net's threads lie beside where they fall). The slopes are the
// cascades' derivative maps, read at the mip that holds a difference `step` wide (the level's texel, or the blur the
// unresolved ripples and the water's scattering leave of a net at its depth, whichever is wider), on a
// square of `span` metres laid on the world and repeating over it, each texel holding the world point nearest the
// camera of all that fall on it. A layer per depth, each with a square twice the one before: the reader blends two
// layers for the depth between.

#import naval_ocean::waves::{waves, field_derivatives, field_sampler, field_uv}

// `naval_ocean::caustics::CausticsUniforms`: the camera (x, z), this level's span (m) and its texels per edge;
// then this level's depth (m), the step of its differences (m), the brightest a focus is drawn and the first cascade
// it reads (the longest waves bend the light of a shallow level by nothing that shows).
struct Caustics {
    centre: vec4<f32>,
    level: vec4<f32>,
}

@group(3) @binding(0) var<uniform> caustics: Caustics;
@group(3) @binding(1) var caustics_out: texture_storage_2d<rgba16float, write>;

// 1 − 1/n for water (n 1.333): the lean of a refracted ray per unit of the surface's slope.
const BEND: f32 = 0.25;

// The surface's slope (∂h/∂x, ∂h/∂z) at grid point `xz`, every cascade at the mip a difference `step` metres wide holds.
fn slope(xz: vec2<f32>, step: f32) -> vec2<f32> {
    var sum = vec2<f32>(0.0);
    for (var i = i32(caustics.level.w); i < i32(waves.info.y); i++) {
        let level = log2(step * waves.texels[i]);
        // A cascade whose every wave is shorter than the step has averaged away.
        if level < waves.info.x {
            sum += textureSampleLevel(field_derivatives, field_sampler, field_uv(xz, i), i, max(level, 0.0)).xy;
        }
    }
    return sum;
}

@compute @workgroup_size(8, 8)
fn caustics_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let texels = u32(caustics.centre.w);
    if id.x >= texels || id.y >= texels {
        return;
    }
    let span = caustics.centre.z;
    let held = (vec2<f32>(id.xy) + 0.5) * (span / f32(texels));
    let xz = caustics.centre.xy + (fract((held - caustics.centre.xy) / span + 0.5) - 0.5) * span;
    let step = caustics.level.y;
    let s = BEND * caustics.level.x;
    // Where the light landing under `xz` entered the sea: x with x + s·∇h(x) = xz.
    var entered = xz - s * slope(xz, step);
    entered = xz - s * slope(entered, step);
    let east = slope(entered + vec2<f32>(step, 0.0), step);
    let west = slope(entered - vec2<f32>(step, 0.0), step);
    let south = slope(entered + vec2<f32>(0.0, step), step);
    let north = slope(entered - vec2<f32>(0.0, step), step);
    let hxx = (east.x - west.x) / (2.0 * step);
    let hzz = (south.y - north.y) / (2.0 * step);
    let hxz = ((south.x - north.x) + (east.y - west.y)) / (4.0 * step);
    let spread = abs(1.0 + s * (hxx + hzz) + s * s * (hxx * hzz - hxz * hxz));
    let gain = 1.0 / max(spread, 1.0 / caustics.level.z);
    textureStore(caustics_out, vec2<i32>(id.xy), vec4<f32>(gain, 0.0, 0.0, 1.0));
}
