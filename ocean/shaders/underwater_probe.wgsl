// The camera's height probe for the underwater view (the GPU half of `Ocean.update`'s camera probe in
// the web game's `ocean/Ocean.ts` and of the per-pixel `heightAt` in `ocean/screen/underwater.ts`): on a grid over the
// screen, the depth below the waves and the wake of each near-plane point, and the camera's own depth below the waves.
// The underwater composite (`underwater.wgsl`), the surface's underside (`surface.wgsl`) and the clouds (which draw
// nothing where a ray starts under the waves) read it. The frame the near plane leaves the water's reach it is written
// once more, all above water, so a reader that does not know whether it ran can always trust it.

#import naval_ocean::waves::height_at
#import naval_ocean::wake::wake_height
#import naval_ocean::underwater_ray::screen_ray

struct ProbeParams {
    inverse_projection: mat4x4<f32>,
    camera_world: mat4x4<f32>,
    // xyz the camera, w the near plane (m); w 0 to clear the probe (every point above the water).
    camera: vec4<f32>,
}

@group(3) @binding(0) var<uniform> probe: ProbeParams;
@group(3) @binding(1) var probe_out: texture_storage_2d<rgba16float, write>;

// r: metres below the surface (waves and wake) where the ray through this texel's screen point starts, negative above it;
// g: the camera's own depth below the waves (`Ocean.update`'s probe reads the wave field alone).
@compute @workgroup_size(8, 8)
fn probe_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(probe_out);
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    if probe.camera.w <= 0.0 {
        textureStore(probe_out, id.xy, vec4<f32>(-1.0, -1.0, 0.0, 1.0));
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    let ray = screen_ray(uv, probe.inverse_projection, probe.camera_world, probe.camera.w);
    let depth = height_at(ray.start.xz) + wake_height(ray.start.xz) - ray.start.y;
    let eye = probe.camera.xyz;
    textureStore(probe_out, id.xy, vec4<f32>(depth, height_at(eye.xz) - eye.y, 0.0, 1.0));
}

