// The cloud shadow other layers read (ships, islands, the sea: `naval_clouds::shadow`): the transmittance of the
// scene's light (the sun's, or the moon's once it outshines the sun: `clouds.ground_light`) through the shadow-casting
// cloud layers above each point of the sea, over a map `EXTENT` metres across laid out in the clouds' drifting frame (so
// it follows the wind exactly) and repeating, each texel standing for its repeat nearest the camera. Marched toward that
// body through the layers with the same field as the clouds, a slice of the map per update. Takram's clouds shade a scene through the Beer shadow maps in a deferred pass; the game's forward-lit ships
// and sea read this map instead, through the interface they always had.

#import naval_clouds::common::{clouds, world_to_planet, ray_sphere_second}
#import naval_clouds::field::{sample_weather, sample_media, weather_uv}

@group(3) @binding(0) var ground_out: texture_storage_2d<rgba8unorm, write>;

// `EXTENT` in `shadow.wgsl`.
const EXTENT: f32 = 40000.0;

@compute @workgroup_size(64, 1, 1)
fn ground_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = u32(clouds.ground.x);
    let index = u32(clouds.ground.y) + id.x;
    if id.x >= u32(clouds.ground.z) || index >= size * size {
        return;
    }
    let texel = vec2<u32>(index % size, index / size);
    // The sea point the texel stands for: its place in the drifting frame, repeated every EXTENT, nearest the camera.
    let at = (vec2<f32>(texel) + 0.5) / f32(size) * EXTENT;
    let camera = clouds.camera.xz - clouds.drift.xz;
    let nearest = at + EXTENT * round((camera - at) / EXTENT);
    let world = nearest + clouds.drift.xz;
    let ground = world_to_planet(vec3<f32>(world.x, 0.0, world.y));
    let toward = clouds.ground_light.xyz;

    // Through the shadow-casting layers toward the light, from the sea when they start under it (fog, rain).
    let enter = max(ray_sphere_second(ground, toward, clouds.sun.w + clouds.heights.w), 0.0);
    let leave = ray_sphere_second(ground, toward, clouds.sun.w + clouds.heights.z);
    var optical_depth = 0.0;
    if toward.y > 0.0 && leave > enter && enter >= 0.0 {
        let steps = i32(clouds.ground.w);
        let step_size = (leave - enter) / f32(steps);
        for (var i = 0; i < steps; i++) {
            let position = ground + toward * (enter + (f32(i) + 0.5) * step_size);
            let height = length(position) - clouds.sun.w;
            let uv = weather_uv(position);
            let weather = sample_weather(uv, height, 0.0, true);
            if any(weather.density > vec4<f32>(clouds.shadow_march.w)) {
                let media = sample_media(weather, position, uv, 0.0, 0.5);
                optical_depth += media.extinction * step_size;
            }
        }
    }
    textureStore(ground_out, texel, vec4<f32>(exp(-optical_depth), 0.0, 0.0, 1.0));
}
