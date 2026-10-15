// The sky's environment lighting as ships and the sea read it.
//
// Ports three.js r185 `src/nodes/pmrem/PMREMUtils.js` (`textureCubeUV`, `bilinearCubeUV`, `getFace`,
// `getUV`, `roughnessToMip`): the sampling side of the PMREM (CubeUV) atlas that
// the web game's `sky/environment/Environment.ts` fills and every `pmremTexture(env, direction, roughness)`
// in the web game reads, and three's `EnvironmentNode` convention for diffuse (irradiance is the
// roughest level along the normal, times π).
//
// Compose with naga_oil (`naval_sky::environment_module`) and import:
//
//     #import naval_sky::environment::{SkyEnvironment, env_radiance, env_irradiance}
//
// The functions take the atlas, its sampler and the uniforms as arguments, so a consumer binds them
// in whatever group it likes (`naval_sky::EnvironmentBindings` gives a ready layout).
#define_import_path naval_sky::environment

// `naval_sky::EnvironmentUniforms` on the Rust side. 80 bytes, std140-compatible.
struct SkyEnvironment {
    // 1 / atlas width, 1 / atlas height (three's CUBEUV_TEXEL_WIDTH / HEIGHT).
    texel_size: vec2<f32>,
    // log2 of the atlas's largest face (three's CUBEUV_MAX_MIP): 6 on Low/Medium, 7 on High/Ultra.
    max_mip: f32,
    // Bumps every time the atlas is refiltered (wraps at 2^24), for consumers that cache.
    generation: f32,
    // Unit world vector toward the active celestial light: the sun by day, the moon once it outshines it.
    light_direction: vec3<f32>,
    // Radiance scale of that light at the sea, in the sea's units (the brightest channel).
    light_intensity: f32,
    // Linear RGB tint of that light at the sea, brightest channel 1.
    light_color: vec3<f32>,
    // 1.0 when the moon is the active light, else 0.0.
    night: f32,
    // Unit world vectors toward the sun and the moon, whichever is active.
    sun_direction: vec3<f32>,
    // 0 new, 0.5 full, 1 new again.
    moon_phase: f32,
    moon_direction: vec3<f32>,
    // Lit share of the moon's disc.
    moon_illumination: f32,
}

const PI: f32 = 3.141592653589793;

// These must match the prefilter (`prefilter.wgsl`, three's PMREMGenerator).
const CUBEUV_R0: f32 = 1.0;
const CUBEUV_M0: f32 = -2.0;
const CUBEUV_R1: f32 = 0.8;
const CUBEUV_M1: f32 = -1.0;
const CUBEUV_R4: f32 = 0.4;
const CUBEUV_M4: f32 = 2.0;
const CUBEUV_R5: f32 = 0.305;
const CUBEUV_M5: f32 = 3.0;
const CUBEUV_R6: f32 = 0.21;
const CUBEUV_M6: f32 = 4.0;
const CUBEUV_MIN_MIP_LEVEL: f32 = 4.0;
const CUBEUV_MIN_TILE_SIZE: f32 = 16.0;

// Cube face of a direction: 0 +X, 1 +Y, 2 +Z, 3 −X, 4 −Y, 5 −Z.
fn cube_uv_face(direction: vec3<f32>) -> f32 {
    let a = abs(direction);
    if a.x > a.z {
        if a.x > a.y {
            return select(3.0, 0.0, direction.x > 0.0);
        }
        return select(4.0, 1.0, direction.y > 0.0);
    }
    if a.z > a.y {
        return select(5.0, 2.0, direction.z > 0.0);
    }
    return select(4.0, 1.0, direction.y > 0.0);
}

// Face-local coordinates in [0, 1] (right-handed, three's PMREM face convention).
fn cube_uv_face_uv(direction: vec3<f32>, face: f32) -> vec2<f32> {
    var uv: vec2<f32>;
    if face == 0.0 {
        uv = vec2<f32>(direction.z, direction.y) / abs(direction.x);
    } else if face == 1.0 {
        uv = vec2<f32>(-direction.x, -direction.z) / abs(direction.y);
    } else if face == 2.0 {
        uv = vec2<f32>(-direction.x, direction.y) / abs(direction.z);
    } else if face == 3.0 {
        uv = vec2<f32>(-direction.z, direction.y) / abs(direction.x);
    } else if face == 4.0 {
        uv = vec2<f32>(-direction.x, direction.z) / abs(direction.y);
    } else {
        uv = vec2<f32>(direction.x, direction.y) / abs(direction.z);
    }
    return 0.5 * (uv + 1.0);
}

// The direction of face-local coordinates `uv` (0–1, may run past the edges into the border) on a face
// (three's `getDirection`); the inverse of `cube_uv_face_uv`. Not normalised.
fn cube_uv_direction(uv_in: vec2<f32>, face: f32) -> vec3<f32> {
    let uv = uv_in * 2.0 - 1.0;
    if face == 0.0 {
        return vec3<f32>(1.0, uv.y, uv.x);
    } else if face == 1.0 {
        return vec3<f32>(-uv.x, 1.0, -uv.y);
    } else if face == 2.0 {
        return vec3<f32>(-uv.x, uv.y, 1.0);
    } else if face == 3.0 {
        return vec3<f32>(-1.0, uv.y, -uv.x);
    } else if face == 4.0 {
        return vec3<f32>(-uv.x, -1.0, uv.y);
    }
    return vec3<f32>(uv.x, uv.y, -1.0);
}

// Roughness → the atlas's level: log2 of the face size for sharp levels (the max mip is the base),
// 3 … −2 for the six extra blurred levels at 16² (three's `roughnessToMip`).
fn cube_uv_roughness_to_mip(roughness: f32) -> f32 {
    if roughness >= CUBEUV_R1 {
        return (CUBEUV_R0 - roughness) * (CUBEUV_M1 - CUBEUV_M0) / (CUBEUV_R0 - CUBEUV_R1) + CUBEUV_M0;
    } else if roughness >= CUBEUV_R4 {
        return (CUBEUV_R1 - roughness) * (CUBEUV_M4 - CUBEUV_M1) / (CUBEUV_R1 - CUBEUV_R4) + CUBEUV_M1;
    } else if roughness >= CUBEUV_R5 {
        return (CUBEUV_R4 - roughness) * (CUBEUV_M5 - CUBEUV_M4) / (CUBEUV_R4 - CUBEUV_R5) + CUBEUV_M4;
    } else if roughness >= CUBEUV_R6 {
        return (CUBEUV_R5 - roughness) * (CUBEUV_M6 - CUBEUV_M5) / (CUBEUV_R5 - CUBEUV_R6) + CUBEUV_M5;
    }
    return -2.0 * log2(1.16 * roughness);
}

// Atlas texel coordinates (not normalised) of a direction at an integer level `mip_int`.
fn cube_uv_texel(direction: vec3<f32>, mip_int_in: f32, max_mip: f32) -> vec2<f32> {
    var face = cube_uv_face(direction);
    let filter_int = max(CUBEUV_MIN_MIP_LEVEL - mip_int_in, 0.0);
    let mip_int = max(mip_int_in, CUBEUV_MIN_MIP_LEVEL);
    let face_size = exp2(mip_int);
    var uv = cube_uv_face_uv(direction, face) * (face_size - 2.0) + 1.0;
    if face > 2.0 {
        uv.y += face_size;
        face -= 3.0;
    }
    uv.x += face * face_size;
    uv.x += filter_int * 3.0 * CUBEUV_MIN_TILE_SIZE;
    uv.y += 4.0 * (exp2(max_mip) - face_size);
    return uv;
}

// One bilinear tap of the atlas at an integer level (three's `bilinearCubeUV`).
fn cube_uv_bilinear(
    atlas: texture_2d<f32>,
    atlas_sampler: sampler,
    texel_size: vec2<f32>,
    max_mip: f32,
    direction: vec3<f32>,
    mip_int: f32,
) -> vec3<f32> {
    return cube_uv_bilinear_rgba(atlas, atlas_sampler, texel_size, max_mip, direction, mip_int).rgb;
}

// The same tap with its alpha: the luminance the sky would have there without clouds (the bake's, prefiltered with
// the radiance).
fn cube_uv_bilinear_rgba(
    atlas: texture_2d<f32>,
    atlas_sampler: sampler,
    texel_size: vec2<f32>,
    max_mip: f32,
    direction: vec3<f32>,
    mip_int: f32,
) -> vec4<f32> {
    let uv = cube_uv_texel(direction, mip_int, max_mip) * texel_size;
    return textureSampleLevel(atlas, atlas_sampler, uv, 0.0);
}

// Prefiltered sky radiance along a world direction for a GGX roughness in [0, 1], linear HDR in the
// sea's units: `pmremTexture(env, direction, roughness)` in the web game. Safe in any shader stage
// (it samples level 0 of the atlas explicitly).
fn env_radiance(
    atlas: texture_2d<f32>,
    atlas_sampler: sampler,
    env: SkyEnvironment,
    direction: vec3<f32>,
    roughness: f32,
) -> vec3<f32> {
    return env_radiance_and_clear(atlas, atlas_sampler, env, direction, roughness).rgb;
}

// `env_radiance` with, in alpha, the luminance the sky would have along the direction without clouds, prefiltered
// alike: the eye meters the scene against it (`sky_mean.wgsl`).
fn env_radiance_and_clear(
    atlas: texture_2d<f32>,
    atlas_sampler: sampler,
    env: SkyEnvironment,
    direction: vec3<f32>,
    roughness: f32,
) -> vec4<f32> {
    let mip = clamp(cube_uv_roughness_to_mip(roughness), CUBEUV_M0, env.max_mip);
    let mip_f = fract(mip);
    let mip_int = floor(mip);
    let color0 = cube_uv_bilinear_rgba(atlas, atlas_sampler, env.texel_size, env.max_mip, direction, mip_int);
    if mip_f == 0.0 {
        return color0;
    }
    let color1 = cube_uv_bilinear_rgba(atlas, atlas_sampler, env.texel_size, env.max_mip, direction, mip_int + 1.0);
    return mix(color0, color1, mip_f);
}

// Diffuse sky light on a surface facing `normal`, as three's `EnvironmentNode` computes it: the roughest
// level along the normal times π. A Lambertian surface reflects `albedo / π` of it, so its diffuse
// image-based light is `albedo * env_radiance(normal, 1)`.
fn env_irradiance(
    atlas: texture_2d<f32>,
    atlas_sampler: sampler,
    env: SkyEnvironment,
    normal: vec3<f32>,
) -> vec3<f32> {
    return env_radiance(atlas, atlas_sampler, env, normal, 1.0) * PI;
}

// Three's specular lookup direction for a surface: the mirror direction bent toward the normal by
// roughness⁴, so rough surfaces do not gather light from behind their tangent plane
// (`EnvironmentNode`'s radiance context). `view` points from the surface toward the eye.
fn env_reflection_direction(normal: vec3<f32>, view: vec3<f32>, roughness: f32) -> vec3<f32> {
    let mirror = reflect(-view, normal);
    let r2 = roughness * roughness;
    return normalize(mix(mirror, normal, r2 * r2));
}

