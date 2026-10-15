// The sea's height about the camera, for the rain's splashes (native only; naval-clouds' splashes.wgsl reads it): the
// drawn sea's `height_at` plus the wakes' `wake_height` on a square of `span` metres laid on the world and repeating
// over it, each texel holding the world point nearest the camera of all that fall on it.

#import naval_ocean::waves::height_at
#import naval_ocean::wake::wake_height

// `naval_ocean::rain_heights::RainHeightsUniforms`: the camera (x, z), the square's span (m) and its texels per edge.
struct RainPatch {
    centre: vec4<f32>,
}

@group(3) @binding(0) var<uniform> rain_patch: RainPatch;
@group(3) @binding(1) var heights_out: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn rain_heights_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let texels = u32(rain_patch.centre.w);
    if id.x >= texels || id.y >= texels {
        return;
    }
    let span = rain_patch.centre.z;
    let held = (vec2<f32>(id.xy) + 0.5) * (span / f32(texels));
    let xz = rain_patch.centre.xy + (fract((held - rain_patch.centre.xy) / span + 0.5) - 0.5) * span;
    textureStore(heights_out, vec2<i32>(id.xy), vec4<f32>(height_at(xz) + wake_height(xz), 0.0, 0.0, 1.0));
}
