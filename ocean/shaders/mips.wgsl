// The wave fields' mip chains, which the web game's `ocean/waves/field.ts` has three build (`generateMipmaps` on the
// resolve targets): each level the 2×2 box average of the one above, for displacement, derivatives and extras alike,
// down to 16² texels (`COARSEST_TEXELS`). Averaging the squared slope with the slope is what lets a pixel read the
// slope variance inside its footprint.

@group(0) @binding(0) var source_displacement: texture_2d_array<f32>;
@group(0) @binding(1) var source_derivatives: texture_2d_array<f32>;
@group(0) @binding(2) var source_extras: texture_2d_array<f32>;
@group(0) @binding(3) var displacement: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(4) var derivatives: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(5) var extras: texture_storage_2d_array<rgba16float, write>;

fn box(source: texture_2d_array<f32>, at: vec2<i32>, layer: i32) -> vec4<f32> {
    return (textureLoad(source, at, layer, 0) + textureLoad(source, at + vec2<i32>(1, 0), layer, 0)
        + textureLoad(source, at + vec2<i32>(0, 1), layer, 0) + textureLoad(source, at + vec2<i32>(1, 1), layer, 0)) * 0.25;
}

@compute @workgroup_size(8, 8, 1)
fn downsample(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<i32>(textureDimensions(displacement));
    let pixel = vec2<i32>(id.xy);
    let layer = i32(id.z);
    if pixel.x >= size.x || pixel.y >= size.y || layer >= i32(textureNumLayers(displacement)) {
        return;
    }
    let at = pixel * 2;
    textureStore(displacement, pixel, layer, box(source_displacement, at, layer));
    textureStore(derivatives, pixel, layer, box(source_derivatives, at, layer));
    textureStore(extras, pixel, layer, box(source_extras, at, layer));
}
