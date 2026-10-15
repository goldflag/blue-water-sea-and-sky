// The atmosphere's tables, precomputed on the GPU as compute passes: takram's PrecomputedTexturesGenerator
// (`packages/atmosphere/src/PrecomputedTexturesGenerator.ts` and `src/shaders/precompute/*.frag`, three-geospatial
// at b012ad06) over Bruneton's functions (`naval_sky::bruneton`), in radiance (its luminanceFromRadiance is the
// identity): transmittance; direct irradiance; single Rayleigh and Mie scattering; then for scattering orders 2, 3
// and 4 the scattering density, the indirect irradiance and the multiple scattering, accumulated into the
// scattering, higher-order and irradiance tables; and a resolve that writes the accumulated values into the tables
// the runtime samples, so a recompute swaps the sky's tables in one go. Group 0 is left empty (as the sky's other
// passes leave it); group 1 holds the parameters, group 2 what a pass reads, group 3 what it writes. The textures
// the passes both write and later sample are half floats (takram's half-float path); accumulation is float32 in
// storage buffers, and transmittance float32 throughout. See THIRD_PARTY.md for the notices.

#import naval_sky::bruneton::{AtmosphereParameters, TRANSMITTANCE_TEXTURE_WIDTH, TRANSMITTANCE_TEXTURE_HEIGHT, SCATTERING_TEXTURE_R_SIZE, SCATTERING_TEXTURE_MU_SIZE, SCATTERING_TEXTURE_MU_S_SIZE, SCATTERING_TEXTURE_NU_SIZE, IRRADIANCE_TEXTURE_WIDTH, IRRADIANCE_TEXTURE_HEIGHT, compute_optical_depth_to_top_atmosphere_boundary_texture, compute_direct_irradiance_texture, compute_single_scattering_texture, get_r_mu_mu_s_nu_from_scattering_texture_frag_coord, compute_scattering_density, compute_indirect_irradiance_texture, compute_multiple_scattering, rayleigh_phase_function}

struct Order {
    // The scattering order this pass computes (2, 3 or 4).
    value: i32,
    padding_a: i32,
    padding_b: i32,
    padding_c: i32,
}

@group(1) @binding(0) var<uniform> atmosphere: AtmosphereParameters;
@group(1) @binding(1) var<uniform> order: Order;

@group(2) @binding(0) var transmittance_texture: texture_2d<f32>;
// Single Rayleigh scattering for order 2, the previous order's multiple scattering after it (takram shares one
// texture between them, as Bruneton's demo does).
@group(2) @binding(1) var rayleigh_or_multiple_texture: texture_3d<f32>;
@group(2) @binding(2) var single_mie_texture: texture_3d<f32>;
@group(2) @binding(3) var scattering_density_texture: texture_3d<f32>;
@group(2) @binding(4) var delta_irradiance_texture: texture_2d<f32>;
@group(2) @binding(5) var linear_sampler: sampler;

@group(3) @binding(0) var transmittance_out: texture_storage_2d<rgba32float, write>;
@group(3) @binding(1) var delta_irradiance_out: texture_storage_2d<rgba16float, write>;
@group(3) @binding(2) var rayleigh_or_multiple_out: texture_storage_3d<rgba16float, write>;
@group(3) @binding(3) var single_mie_out: texture_storage_3d<rgba16float, write>;
@group(3) @binding(4) var scattering_density_out: texture_storage_3d<rgba16float, write>;
@group(3) @binding(5) var<storage, read_write> scattering_sum: array<vec4<f32>>;
@group(3) @binding(6) var<storage, read_write> higher_order_sum: array<vec4<f32>>;
@group(3) @binding(7) var<storage, read_write> single_mie_sum: array<vec4<f32>>;
@group(3) @binding(8) var<storage, read_write> irradiance_sum: array<vec4<f32>>;
// The runtime's tables, written by the resolve.
@group(3) @binding(9) var scattering_final: texture_storage_3d<rgba16float, write>;
@group(3) @binding(10) var single_mie_final: texture_storage_3d<rgba16float, write>;
@group(3) @binding(11) var higher_order_final: texture_storage_3d<rgba16float, write>;
@group(3) @binding(12) var irradiance_final: texture_storage_2d<rgba16float, write>;

const SCATTERING_WIDTH: u32 = u32(SCATTERING_TEXTURE_NU_SIZE * SCATTERING_TEXTURE_MU_S_SIZE);
const SCATTERING_HEIGHT: u32 = u32(SCATTERING_TEXTURE_MU_SIZE);
const SCATTERING_DEPTH: u32 = u32(SCATTERING_TEXTURE_R_SIZE);

fn scattering_index(id: vec3<u32>) -> u32 {
    return id.x + SCATTERING_WIDTH * (id.y + SCATTERING_HEIGHT * id.z);
}

fn outside_scattering(id: vec3<u32>) -> bool {
    return id.x >= SCATTERING_WIDTH || id.y >= SCATTERING_HEIGHT || id.z >= SCATTERING_DEPTH;
}

fn outside_irradiance(id: vec3<u32>) -> bool {
    return id.x >= u32(IRRADIANCE_TEXTURE_WIDTH) || id.y >= u32(IRRADIANCE_TEXTURE_HEIGHT);
}

fn irradiance_index(id: vec3<u32>) -> u32 {
    return id.x + u32(IRRADIANCE_TEXTURE_WIDTH) * id.y;
}

// transmittance.frag
@compute @workgroup_size(8, 8)
fn transmittance_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= u32(TRANSMITTANCE_TEXTURE_WIDTH) || id.y >= u32(TRANSMITTANCE_TEXTURE_HEIGHT) {
        return;
    }
    let optical_depth = compute_optical_depth_to_top_atmosphere_boundary_texture(atmosphere, vec2<f32>(id.xy) + 0.5);
    textureStore(transmittance_out, id.xy, vec4<f32>(optical_depth, 1.0));
}

// directIrradiance.frag: the direct irradiance into the delta, and the irradiance table zeroed (it holds only the
// sky's light).
@compute @workgroup_size(8, 8)
fn direct_irradiance_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside_irradiance(id) {
        return;
    }
    let delta = compute_direct_irradiance_texture(atmosphere, transmittance_texture, vec2<f32>(id.xy) + 0.5);
    textureStore(delta_irradiance_out, id.xy, vec4<f32>(delta, 1.0));
    irradiance_sum[irradiance_index(id)] = vec4<f32>(0.0, 0.0, 0.0, 1.0);
}

// singleScattering.frag: the deltas, the scattering table's first values (Rayleigh in rgb, Mie's red in alpha) and
// the single-Mie table.
@compute @workgroup_size(8, 8, 1)
fn single_scattering_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside_scattering(id) {
        return;
    }
    let single = compute_single_scattering_texture(atmosphere, transmittance_texture, vec3<f32>(id) + 0.5);
    textureStore(rayleigh_or_multiple_out, id, vec4<f32>(single.rayleigh, 1.0));
    textureStore(single_mie_out, id, vec4<f32>(single.mie, 1.0));
    let index = scattering_index(id);
    scattering_sum[index] = vec4<f32>(single.rayleigh, single.mie.r);
    single_mie_sum[index] = vec4<f32>(single.mie, 1.0);
    higher_order_sum[index] = vec4<f32>(0.0);
}

// scatteringDensity.frag
@compute @workgroup_size(8, 8, 1)
fn scattering_density_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside_scattering(id) {
        return;
    }
    let c = get_r_mu_mu_s_nu_from_scattering_texture_frag_coord(atmosphere, vec3<f32>(id) + 0.5);
    let density = compute_scattering_density(
        atmosphere,
        transmittance_texture,
        rayleigh_or_multiple_texture,
        single_mie_texture,
        rayleigh_or_multiple_texture,
        delta_irradiance_texture,
        linear_sampler,
        c.r,
        c.mu,
        c.mu_s,
        c.nu,
        order.value,
    );
    textureStore(scattering_density_out, id, vec4<f32>(density, 1.0));
}

// indirectIrradiance.frag: from the previous order's scattering, into the delta and added to the table.
@compute @workgroup_size(8, 8)
fn indirect_irradiance_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside_irradiance(id) {
        return;
    }
    let delta = compute_indirect_irradiance_texture(
        atmosphere,
        rayleigh_or_multiple_texture,
        single_mie_texture,
        rayleigh_or_multiple_texture,
        linear_sampler,
        vec2<f32>(id.xy) + 0.5,
        order.value - 1,
    );
    textureStore(delta_irradiance_out, id.xy, vec4<f32>(delta, 1.0));
    let index = irradiance_index(id);
    irradiance_sum[index] = irradiance_sum[index] + vec4<f32>(delta, 0.0);
}

// multipleScattering.frag: into the delta, and added (over the Rayleigh phase) to the scattering and higher-order
// tables.
@compute @workgroup_size(8, 8, 1)
fn multiple_scattering_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside_scattering(id) {
        return;
    }
    let c = get_r_mu_mu_s_nu_from_scattering_texture_frag_coord(atmosphere, vec3<f32>(id) + 0.5);
    let delta = compute_multiple_scattering(
        atmosphere,
        transmittance_texture,
        scattering_density_texture,
        linear_sampler,
        c.r,
        c.mu,
        c.mu_s,
        c.nu,
        c.ray_r_mu_intersects_ground,
    );
    textureStore(rayleigh_or_multiple_out, id, vec4<f32>(delta, 1.0));
    let added = vec4<f32>(delta / rayleigh_phase_function(c.nu), 0.0);
    let index = scattering_index(id);
    scattering_sum[index] = scattering_sum[index] + added;
    higher_order_sum[index] = higher_order_sum[index] + added;
}

// The accumulated scattering, single Mie and higher orders into the runtime's tables.
@compute @workgroup_size(8, 8, 1)
fn resolve_scattering_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside_scattering(id) {
        return;
    }
    let index = scattering_index(id);
    textureStore(scattering_final, id, scattering_sum[index]);
    textureStore(single_mie_final, id, single_mie_sum[index]);
    textureStore(higher_order_final, id, vec4<f32>(higher_order_sum[index].rgb, 1.0));
}

@compute @workgroup_size(8, 8)
fn resolve_irradiance_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside_irradiance(id) {
        return;
    }
    textureStore(irradiance_final, id.xy, vec4<f32>(irradiance_sum[irradiance_index(id)].rgb, 1.0));
}
