// What a layer writes in the motion pass, for temporal anti-aliasing. Ports the
// `ObjectMotionNode` of the web game's `TemporalAntialiasing.ts`: a moving surface's own
// motion relative to the static world, in the previous frame's NDC (both positions
// projected with the previous view-projection), with its temporal response in z
// (0 takes full history, 1 none) and alpha 1, which replaces what is below. Camera
// motion is not written: the resolve reconstructs it from depth for every pixel.
// A surface that only asks for less history (the sea, tracers) writes
// `motion_response(r)`: alpha 0, so the pass's blend (One, OneMinusSrcAlpha) adds
// its response to what is below and keeps the motion there.
#define_import_path naval::motion

// Group 0 binding 5, beside the camera. Its Rust twin is `naval_render::CameraMotion`.
struct CameraMotion {
    // This frame's view-projection without temporal anti-aliasing's jitter.
    unjittered_view_projection: mat4x4<f32>,
    // The previous frame's unjittered view-projection (this frame's without history).
    previous_view_projection: mat4x4<f32>,
    // The sub-pixel offset the camera's projection carries, in pixels.
    jitter: vec2<f32>,
    padding: vec2<f32>,
}

@group(0) @binding(5) var<uniform> camera_motion: CameraMotion;

fn surface_motion(current_world: vec3<f32>, previous_world: vec3<f32>, response: f32) -> vec4<f32> {
    let now = camera_motion.previous_view_projection * vec4<f32>(current_world, 1.0);
    let before = camera_motion.previous_view_projection * vec4<f32>(previous_world, 1.0);
    return vec4<f32>(now.xy / now.w - before.xy / before.w, response, 1.0);
}

fn motion_response(response: f32) -> vec4<f32> {
    return vec4<f32>(0.0, 0.0, response, 0.0);
}
