// The sea's height about each hull, for the hulls' wet band: the web game's `HullWetBand.ts` reads, per hull fragment, the sea
// Game.ts hands it (`ocean.waveField.heightAt(xz)` with the sea around hulls of the web game's `ocean/waves/hullSea.ts`, plus
// `shipWake.surfaceHeight(x, z)`, the web game's `ShipWake.ts`: the wake field's height and the bow waves' at the vertex stage's
// feature size). The ship pipeline has no group free for the wave field (and the field's arrays change every frame), so
// the ocean evaluates the same heights here, once a frame, on a patch about each hull it couples and the camera sees:
// one layer per patch, its texels (as many as its distance needs) in the layer's corner from stern to stem (x) and port
// to starboard (y), the outer ones on the patch's edges. naval-ships' ship.wgsl reads them back bilinearly
// (`sea_height`).
//
// The heights are the sea as drawn against the hull (`drawn_height_at` at the water mesh's spacing there), not the wave
// field's full detail: a hull far from the mesh's centre (a ship on the horizon through binoculars) stands in water
// drawn flat, and plating the full waves would cover was lit as under water, black strips above the sea drawn.

#import naval_ocean::waves::drawn_height_at
#import naval_ocean::wake::{wake_height, mesh_spacing}

// `naval_ocean::hull_heights::HullHeightsUniforms`: per patch (centre x, z, bow axis x, z) and (half length, half
// breadth, texels along, texels across); then (patches in use, ·, ·, ·).
struct HullPatches {
    patches: array<vec4<f32>, 32>,
    count: vec4<f32>,
}

@group(3) @binding(0) var<uniform> hull_patches: HullPatches;
@group(3) @binding(1) var heights_out: texture_storage_2d_array<r32float, write>;

@compute @workgroup_size(8, 8)
fn heights_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let slot = i32(id.z);
    if slot >= i32(hull_patches.count.x) {
        return;
    }
    let centre = hull_patches.patches[slot * 2];
    let reach = hull_patches.patches[slot * 2 + 1];
    let size = min(vec2<u32>(reach.zw), textureDimensions(heights_out));
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let along = (f32(id.x) / f32(size.x - 1u) * 2.0 - 1.0) * reach.x;
    let across = (f32(id.y) / f32(size.y - 1u) * 2.0 - 1.0) * reach.y;
    let xz = centre.xy + centre.zw * along + vec2<f32>(-centre.w, centre.z) * across;
    // A calm wake (none bound yet) names no mesh: the finest detail, as near the mesh's centre.
    let height = drawn_height_at(xz, max(mesh_spacing(xz), 0.01)) + wake_height(xz);
    textureStore(heights_out, vec2<i32>(id.xy), slot, vec4<f32>(height, 0.0, 0.0, 1.0));
}
