// The scene's lights beyond the sun, and its fog switch, as every lit material takes them.
//
// - Point lights: three.js r185's `PointLightNode` (`directPointLight`) with `getDistanceAttenuation`
//   (src/nodes/lighting/LightUtils.js, Frostbite's windowed inverse square). three hands every point light in the
//   scene to every lit material: the combat flashes (the web game's `CombatEffects.ts`, four), the ship fires'
//   (the web game's `LocalizedFireEffects.ts`, two) and the lightning stroke (`VisualEnvironment.ts` `boltLight`).
// - Fog: the air between the eye and a fragment is the aerial perspective (`naval::aerial`: the atmosphere and the
//   weather's haze, from the volume the sky fills each frame); `SceneFog` only switches it on for a layer.
//
// No bindings here: a layer keeps both structs in its own uniforms (Rust twins in naval_render::lights).
#define_import_path naval::lights

struct PointLight {
    // xyz world position (m), w cut-off distance (m; 0 reaches everywhere).
    position: vec4<f32>,
    // rgb linear colour × intensity (candela), w decay exponent.
    color: vec4<f32>,
    // xyz outward searchlight direction (zero for a point light), w outer half-angle cosine.
    cone: vec4<f32>,
}

struct PointLights {
    // x the lights in use (at most 8).
    count: vec4<u32>,
    lights: array<PointLight, 8>,
}

struct SceneFog {
    // rgb the authored fog colour, a the old ramp's power (no longer drawn).
    color: vec4<f32>,
    // x start, y end (the visibility, m), z sky-blend distance (m), w 1 while the aerial perspective is on: only w is
    // read here; the visibility reaches the sky's haze from the host.
    params: vec4<f32>,
}

// A point light at a surface: the unit direction toward the light and its colour × attenuation.
struct LightSample {
    direction: vec3<f32>,
    radiance: vec3<f32>,
}

// three's `getDistanceAttenuation`: 1 / max(d^decay, 0.01), windowed to 0 at the cut-off when there is one.
fn distance_attenuation(distance: f32, cutoff: f32, decay: f32) -> f32 {
    let falloff = 1.0 / max(pow(distance, decay), 0.01);
    if cutoff > 0.0 {
        let ratio = distance / cutoff;
        let window = clamp(1.0 - ratio * ratio * ratio * ratio, 0.0, 1.0);
        return falloff * window * window;
    }
    return falloff;
}

// `directPointLight`: toward the light from `world`, its colour times the attenuation over the distance.
fn point_light_at(light: PointLight, world: vec3<f32>) -> LightSample {
    let to_light = light.position.xyz - world;
    let distance = length(to_light);
    var out: LightSample;
    out.direction = to_light / max(distance, 1e-4);
    out.radiance = light.color.rgb * distance_attenuation(distance, light.position.w, light.color.w);
    if dot(light.cone.xyz, light.cone.xyz) > 0.5 {
        let inner = mix(light.cone.w, 1.0, 0.15);
        out.radiance *= smoothstep(light.cone.w, inner, dot(light.cone.xyz, -out.direction));
    }
    return out;
}

// The lights in use, never more than the array holds.
fn point_light_count(lights: PointLights) -> u32 {
    return min(lights.count.x, 8u);
}
