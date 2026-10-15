// Aerial perspective for every material: the air between the eye and a fragment, read from the volume the sky's layer
// fills each frame (naval_render::aerial, bound at group 0 beside the camera). One model for everything in the world:
// takram's atmosphere (the dome's and the clouds' tables) and the weather's haze, lit by the sun, the moon and the sky.
// Before the sky has filled it the volume holds clear air, and these leave a colour as it is.
//
//     #import naval::aerial::{aerial_apply, aerial_apply_additive}
//     return vec4<f32>(aerial_apply(lit, in.world), 1.0);                   // opaque, or blended over (straight alpha)
//     return vec4<f32>(aerial_apply_additive(glow, in.world), alpha);      // added to what is behind (no in-scatter)
//
// For straight alpha (src × a + dst × (1 − a)) `aerial_apply` on the fragment's colour is exact: what is behind took the
// air beyond the fragment already.
#define_import_path naval::aerial

#import naval::camera::{camera, world_to_clip}
#import naval::aerial_volume::{AerialSample, aerial_sample}

@group(0) @binding(6) var aerial_volume: texture_3d<f32>;
@group(0) @binding(7) var aerial_sampler: sampler;

// The air between the eye and the world point `world` (m).
fn aerial_at(world: vec3<f32>) -> AerialSample {
    let clip = world_to_clip(world);
    let ndc = clip.xy / max(clip.w, 1e-6);
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    return aerial_sample(aerial_volume, aerial_sampler, uv, distance(world, camera.position));
}

// A surface's radiance at `world` as the eye sees it through the air.
fn aerial_apply(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let air = aerial_at(world);
    return color * air.transmittance + air.inscatter;
}

// Light added to what is behind (additive blending: glows, flashes): the air dims it and adds nothing of its own, which
// what is behind already carries.
fn aerial_apply_additive(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    return color * aerial_at(world).transmittance;
}
