// Precomputed atmospheric scattering: Eric Bruneton's model (2017, after Bruneton and Neyret 2008) as takram's
// three-geospatial atmosphere package ships it (`packages/atmosphere/src/shaders/bruneton/{definitions, common,
// precompute, runtime}.glsl`, three-geospatial at b012ad06), ported to WGSL. takram's changes are kept: transmittance
// read through a manual bilinear filter (its TRANSMITTANCE_PRECISION_LOG path, here over float32 texels that no
// sampler filters), a separate single-Mie table, a higher-order scattering table so light shafts occlude only single
// scattering, SafeSqrt toward the top of the air, rays clipped at the bottom of the air, the closest-point test and
// the horizon epsilon of GetSkyRadianceToPoint, and the guard on the Mie extrapolation. Every function takes the
// tables (and the sampler for the filtered ones) as arguments, so the precompute passes and the runtime
// (`naval_sky::atmosphere`, over the sky's group 1) share them. Lengths are kilometres, as takram's
// METER_TO_LENGTH_UNIT makes them; the RGB channels stand for 680, 550 and 440 nm. See THIRD_PARTY.md.
//
// Copyright (c) 2017 Eric Bruneton
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
// 1. Redistributions of source code must retain the above copyright
//    notice, this list of conditions and the following disclaimer.
// 2. Redistributions in binary form must reproduce the above copyright
//    notice, this list of conditions and the following disclaimer in the
//    documentation and/or other materials provided with the distribution.
// 3. Neither the name of the copyright holders nor the names of its
//    contributors may be used to endorse or promote products derived from
//    this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
// AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
// ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE
// LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
// CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
// SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
// INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
// CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
// ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF
// THE POSSIBILITY OF SUCH DAMAGE.
//
// Precomputed Atmospheric Scattering
// Copyright (c) 2008 INRIA
// All rights reserved.
// (Redistributed under the same three conditions and disclaimer as above.)
//
// takram's changes: Copyright (c) 2024 Shota Matsuda, MIT License.
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NON-INFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.
#define_import_path naval_sky::bruneton

const PI: f32 = 3.14159265358979323846;

// Table sizes (takram's constants.ts).
const TRANSMITTANCE_TEXTURE_WIDTH: i32 = 256;
const TRANSMITTANCE_TEXTURE_HEIGHT: i32 = 64;
const SCATTERING_TEXTURE_R_SIZE: i32 = 32;
const SCATTERING_TEXTURE_MU_SIZE: i32 = 128;
const SCATTERING_TEXTURE_MU_S_SIZE: i32 = 32;
const SCATTERING_TEXTURE_NU_SIZE: i32 = 8;
const IRRADIANCE_TEXTURE_WIDTH: i32 = 64;
const IRRADIANCE_TEXTURE_HEIGHT: i32 = 16;

// One layer of a density profile: exp_term * exp(exp_scale * h) + linear_term * h + constant_term, clamped to
// [0, 1], below `width` km (the upper layer runs to the top of the air). Padded to 32 bytes for uniform buffers.
struct DensityProfileLayer {
    width: f32,
    exp_term: f32,
    exp_scale: f32,
    linear_term: f32,
    constant_term: f32,
    padding_a: f32,
    padding_b: f32,
    padding_c: f32,
}

struct DensityProfile {
    lower: DensityProfileLayer,
    upper: DensityProfileLayer,
}

// `AtmosphereParameters` (definitions.glsl), laid out for a uniform buffer (`naval_sky::model::AtmosphereParameters`
// on the Rust side): every vec3 at a 16-byte offset, the profiles at 16-byte offsets.
struct AtmosphereParameters {
    solar_irradiance: vec3<f32>,
    sun_angular_radius: f32,
    bottom_radius: f32,
    top_radius: f32,
    mie_phase_function_g: f32,
    mu_s_min: f32,
    rayleigh_scattering: vec3<f32>,
    padding_a: f32,
    mie_scattering: vec3<f32>,
    padding_b: f32,
    mie_extinction: vec3<f32>,
    padding_c: f32,
    absorption_extinction: vec3<f32>,
    padding_d: f32,
    ground_albedo: vec3<f32>,
    padding_e: f32,
    rayleigh_density: DensityProfile,
    mie_density: DensityProfile,
    absorption_density: DensityProfile,
}

// --- common.glsl ---------------------------------------------------------------------------------------------

fn clamp_cosine(mu: f32) -> f32 {
    return clamp(mu, -1.0, 1.0);
}

fn clamp_distance(d: f32) -> f32 {
    return max(d, 0.0);
}

fn clamp_radius(atmosphere: AtmosphereParameters, r: f32) -> f32 {
    return clamp(r, atmosphere.bottom_radius, atmosphere.top_radius);
}

fn safe_sqrt(a: f32) -> f32 {
    return sqrt(max(a, 0.0));
}

fn distance_to_top_atmosphere_boundary(atmosphere: AtmosphereParameters, r: f32, mu: f32) -> f32 {
    let discriminant = r * r * (mu * mu - 1.0) + atmosphere.top_radius * atmosphere.top_radius;
    return clamp_distance(-r * mu + safe_sqrt(discriminant));
}

fn distance_to_bottom_atmosphere_boundary(atmosphere: AtmosphereParameters, r: f32, mu: f32) -> f32 {
    let discriminant = r * r * (mu * mu - 1.0) + atmosphere.bottom_radius * atmosphere.bottom_radius;
    return clamp_distance(-r * mu - safe_sqrt(discriminant));
}

fn ray_intersects_ground(atmosphere: AtmosphereParameters, r: f32, mu: f32) -> bool {
    return mu < 0.0 && r * r * (mu * mu - 1.0) + atmosphere.bottom_radius * atmosphere.bottom_radius >= 0.0;
}

fn get_texture_coord_from_unit_range(x: f32, texture_size: i32) -> f32 {
    return 0.5 / f32(texture_size) + x * (1.0 - 1.0 / f32(texture_size));
}

fn get_transmittance_texture_uv_from_r_mu(atmosphere: AtmosphereParameters, r: f32, mu: f32) -> vec2<f32> {
    // Distance to the top of the air for a horizontal ray at ground level.
    let h = sqrt(atmosphere.top_radius * atmosphere.top_radius - atmosphere.bottom_radius * atmosphere.bottom_radius);
    // Distance to the horizon.
    let rho = safe_sqrt(r * r - atmosphere.bottom_radius * atmosphere.bottom_radius);
    // Distance to the top of the air for the ray (r, mu), and its extremes over all mu.
    let d = distance_to_top_atmosphere_boundary(atmosphere, r, mu);
    let d_min = atmosphere.top_radius - r;
    let d_max = rho + h;
    let x_mu = (d - d_min) / (d_max - d_min);
    let x_r = rho / h;
    return vec2<f32>(
        get_texture_coord_from_unit_range(x_mu, TRANSMITTANCE_TEXTURE_WIDTH),
        get_texture_coord_from_unit_range(x_r, TRANSMITTANCE_TEXTURE_HEIGHT),
    );
}

fn transmittance_texel(transmittance_texture: texture_2d<f32>, coord: vec2<i32>) -> vec3<f32> {
    let top = vec2<i32>(TRANSMITTANCE_TEXTURE_WIDTH - 1, TRANSMITTANCE_TEXTURE_HEIGHT - 1);
    return textureLoad(transmittance_texture, clamp(coord, vec2<i32>(0), top), 0).rgb;
}

// The transmittance table holds float32 optical depth (takram's TRANSMITTANCE_PRECISION_LOG idea, which keeps
// tiny transmittance from underflowing), which no WebGPU sampler need filter: filtered here by hand, clamped to the
// edge.
fn get_optical_depth_to_top_atmosphere_boundary(
    atmosphere: AtmosphereParameters,
    transmittance_texture: texture_2d<f32>,
    r: f32,
    mu: f32,
) -> vec3<f32> {
    let uv = get_transmittance_texture_uv_from_r_mu(atmosphere, r, mu);
    let size = vec2<f32>(f32(TRANSMITTANCE_TEXTURE_WIDTH), f32(TRANSMITTANCE_TEXTURE_HEIGHT));
    let coord = uv * size - 0.5;
    let base = floor(coord);
    let f = coord - base;
    let i = vec2<i32>(base);
    let t_a = transmittance_texel(transmittance_texture, i);
    let t_b = transmittance_texel(transmittance_texture, i + vec2<i32>(1, 0));
    let t_c = transmittance_texel(transmittance_texture, i + vec2<i32>(0, 1));
    let t_d = transmittance_texel(transmittance_texture, i + vec2<i32>(1, 1));
    return mix(mix(t_a, t_b, f.x), mix(t_c, t_d, f.x), f.y);
}

fn get_transmittance_to_top_atmosphere_boundary(
    atmosphere: AtmosphereParameters,
    transmittance_texture: texture_2d<f32>,
    r: f32,
    mu: f32,
) -> vec3<f32> {
    return exp(-get_optical_depth_to_top_atmosphere_boundary(atmosphere, transmittance_texture, r, mu));
}

// Bruneton's ratio of two transmittances to the top of the air, taken as a difference of optical depths so that
// long paths through a hazy map's air (optical depth past float32's exp range) neither underflow nor divide 0 by 0.
fn get_transmittance(
    atmosphere: AtmosphereParameters,
    transmittance_texture: texture_2d<f32>,
    r: f32,
    mu: f32,
    d: f32,
    ray_r_mu_intersects_ground: bool,
) -> vec3<f32> {
    let r_d = clamp_radius(atmosphere, sqrt(d * d + 2.0 * r * mu * d + r * r));
    let mu_d = clamp_cosine((r * mu + d) / r_d);
    if ray_r_mu_intersects_ground {
        return exp(-max(
            get_optical_depth_to_top_atmosphere_boundary(atmosphere, transmittance_texture, r_d, -mu_d)
                - get_optical_depth_to_top_atmosphere_boundary(atmosphere, transmittance_texture, r, -mu),
            vec3<f32>(0.0),
        ));
    }
    return exp(-max(
        get_optical_depth_to_top_atmosphere_boundary(atmosphere, transmittance_texture, r, mu)
            - get_optical_depth_to_top_atmosphere_boundary(atmosphere, transmittance_texture, r_d, mu_d),
        vec3<f32>(0.0),
    ));
}

fn get_transmittance_to_sun(
    atmosphere: AtmosphereParameters,
    transmittance_texture: texture_2d<f32>,
    r: f32,
    mu_s: f32,
) -> vec3<f32> {
    let sin_theta_h = atmosphere.bottom_radius / r;
    let cos_theta_h = -sqrt(max(1.0 - sin_theta_h * sin_theta_h, 0.0));
    return get_transmittance_to_top_atmosphere_boundary(atmosphere, transmittance_texture, r, mu_s)
        * smoothstep(
            -sin_theta_h * atmosphere.sun_angular_radius,
            sin_theta_h * atmosphere.sun_angular_radius,
            mu_s - cos_theta_h,
        );
}

fn rayleigh_phase_function(nu: f32) -> f32 {
    let k = 3.0 / (16.0 * PI);
    return k * (1.0 + nu * nu);
}

fn mie_phase_function(g: f32, nu: f32) -> f32 {
    let k = 3.0 / (8.0 * PI) * (1.0 - g * g) / (2.0 + g * g);
    return k * (1.0 + nu * nu) / pow(1.0 + g * g - 2.0 * g * nu, 1.5);
}

fn get_scattering_texture_uvwz_from_r_mu_mu_s_nu(
    atmosphere: AtmosphereParameters,
    r: f32,
    mu: f32,
    mu_s: f32,
    nu: f32,
    ray_r_mu_intersects_ground: bool,
) -> vec4<f32> {
    // Distance to the top of the air for a horizontal ray at ground level.
    let h = sqrt(atmosphere.top_radius * atmosphere.top_radius - atmosphere.bottom_radius * atmosphere.bottom_radius);
    // Distance to the horizon.
    let rho = safe_sqrt(r * r - atmosphere.bottom_radius * atmosphere.bottom_radius);
    let u_r = get_texture_coord_from_unit_range(rho / h, SCATTERING_TEXTURE_R_SIZE);

    // Discriminant of the ray (r, mu) meeting the ground.
    let r_mu = r * mu;
    let discriminant = r_mu * r_mu - r * r + atmosphere.bottom_radius * atmosphere.bottom_radius;
    var u_mu: f32;
    if ray_r_mu_intersects_ground {
        // Distance to the ground for the ray (r, mu), and its extremes over all mu.
        let d = -r_mu - safe_sqrt(discriminant);
        let d_min = r - atmosphere.bottom_radius;
        let d_max = rho;
        let x = select((d - d_min) / (d_max - d_min), 0.0, d_max == d_min);
        u_mu = 0.5 - 0.5 * get_texture_coord_from_unit_range(x, SCATTERING_TEXTURE_MU_SIZE / 2);
    } else {
        // Distance to the top of the air for the ray (r, mu), and its extremes over all mu.
        let d = -r_mu + safe_sqrt(discriminant + h * h);
        let d_min = atmosphere.top_radius - r;
        let d_max = rho + h;
        u_mu = 0.5 + 0.5 * get_texture_coord_from_unit_range((d - d_min) / (d_max - d_min), SCATTERING_TEXTURE_MU_SIZE / 2);
    }

    let d = distance_to_top_atmosphere_boundary(atmosphere, atmosphere.bottom_radius, mu_s);
    let d_min = atmosphere.top_radius - atmosphere.bottom_radius;
    let d_max = h;
    let a = (d - d_min) / (d_max - d_min);
    let big_d = distance_to_top_atmosphere_boundary(atmosphere, atmosphere.bottom_radius, atmosphere.mu_s_min);
    let big_a = (big_d - d_min) / (d_max - d_min);
    // An ad-hoc function equal to 0 for mu_s = mu_s_min, 1 for mu_s = 1, with a large slope around mu_s = 0.
    let u_mu_s = get_texture_coord_from_unit_range(max(1.0 - a / big_a, 0.0) / (1.0 + a), SCATTERING_TEXTURE_MU_S_SIZE);

    let u_nu = (nu + 1.0) / 2.0;
    return vec4<f32>(u_nu, u_mu_s, u_mu, u_r);
}

fn get_irradiance_texture_uv_from_r_mu_s(atmosphere: AtmosphereParameters, r: f32, mu_s: f32) -> vec2<f32> {
    let x_r = (r - atmosphere.bottom_radius) / (atmosphere.top_radius - atmosphere.bottom_radius);
    let x_mu_s = mu_s * 0.5 + 0.5;
    return vec2<f32>(
        get_texture_coord_from_unit_range(x_mu_s, IRRADIANCE_TEXTURE_WIDTH),
        get_texture_coord_from_unit_range(x_r, IRRADIANCE_TEXTURE_HEIGHT),
    );
}

fn get_irradiance(
    atmosphere: AtmosphereParameters,
    irradiance_texture: texture_2d<f32>,
    linear_sampler: sampler,
    r: f32,
    mu_s: f32,
) -> vec3<f32> {
    let uv = get_irradiance_texture_uv_from_r_mu_s(atmosphere, r, mu_s);
    return textureSampleLevel(irradiance_texture, linear_sampler, uv, 0.0).rgb;
}

// A 4D scattering table read as two trilinear reads along nu (`GetScattering`, the AbstractScatteringTexture form).
fn get_scattering(
    atmosphere: AtmosphereParameters,
    scattering_texture: texture_3d<f32>,
    linear_sampler: sampler,
    r: f32,
    mu: f32,
    mu_s: f32,
    nu: f32,
    ray_r_mu_intersects_ground: bool,
) -> vec4<f32> {
    let uvwz = get_scattering_texture_uvwz_from_r_mu_mu_s_nu(atmosphere, r, mu, mu_s, nu, ray_r_mu_intersects_ground);
    let tex_coord_x = uvwz.x * f32(SCATTERING_TEXTURE_NU_SIZE - 1);
    let tex_x = floor(tex_coord_x);
    let lerp = tex_coord_x - tex_x;
    let uvw_a = vec3<f32>((tex_x + uvwz.y) / f32(SCATTERING_TEXTURE_NU_SIZE), uvwz.z, uvwz.w);
    let uvw_b = vec3<f32>((tex_x + 1.0 + uvwz.y) / f32(SCATTERING_TEXTURE_NU_SIZE), uvwz.z, uvwz.w);
    return textureSampleLevel(scattering_texture, linear_sampler, uvw_a, 0.0) * (1.0 - lerp)
        + textureSampleLevel(scattering_texture, linear_sampler, uvw_b, 0.0) * lerp;
}

// --- precompute.glsl ------------------------------------------------------------------------------------------

fn get_layer_density(layer: DensityProfileLayer, altitude: f32) -> f32 {
    let density = layer.exp_term * exp(layer.exp_scale * altitude) + layer.linear_term * altitude + layer.constant_term;
    return clamp(density, 0.0, 1.0);
}

fn get_profile_density(profile: DensityProfile, altitude_in: f32) -> f32 {
    // A sample at the bottom of the air can round a hair below it in float32, which would pick the lower layer
    // (all zero for the molecules and the aerosol) and lose the densest sample of a sea-level ray: held at 0.
    let altitude = max(altitude_in, 0.0);
    if altitude < profile.lower.width {
        return get_layer_density(profile.lower, altitude);
    }
    return get_layer_density(profile.upper, altitude);
}

fn compute_optical_length_to_top_atmosphere_boundary(
    atmosphere: AtmosphereParameters,
    profile: DensityProfile,
    r: f32,
    mu: f32,
) -> f32 {
    // Number of intervals of the numerical integration (trapezoidal rule).
    let sample_count = 500;
    let dx = distance_to_top_atmosphere_boundary(atmosphere, r, mu) / f32(sample_count);
    var result = 0.0;
    for (var i = 0; i <= sample_count; i++) {
        let d_i = f32(i) * dx;
        // Distance between the sample and the planet's centre.
        let r_i = sqrt(d_i * d_i + 2.0 * r * mu * d_i + r * r);
        let y_i = get_profile_density(profile, r_i - atmosphere.bottom_radius);
        let weight_i = select(1.0, 0.5, i == 0 || i == sample_count);
        result += y_i * weight_i * dx;
    }
    return result;
}

// The optical depth to the top of the air (the table stores it rather than its exponential; see
// `get_optical_depth_to_top_atmosphere_boundary`).
fn compute_optical_depth_to_top_atmosphere_boundary(atmosphere: AtmosphereParameters, r: f32, mu: f32) -> vec3<f32> {
    return atmosphere.rayleigh_scattering
        * compute_optical_length_to_top_atmosphere_boundary(atmosphere, atmosphere.rayleigh_density, r, mu)
        + atmosphere.mie_extinction
        * compute_optical_length_to_top_atmosphere_boundary(atmosphere, atmosphere.mie_density, r, mu)
        + atmosphere.absorption_extinction
        * compute_optical_length_to_top_atmosphere_boundary(atmosphere, atmosphere.absorption_density, r, mu);
}

fn get_unit_range_from_texture_coord(u: f32, texture_size: i32) -> f32 {
    return (u - 0.5 / f32(texture_size)) / (1.0 - 1.0 / f32(texture_size));
}

struct RMu {
    r: f32,
    mu: f32,
}

fn get_r_mu_from_transmittance_texture_uv(atmosphere: AtmosphereParameters, uv: vec2<f32>) -> RMu {
    let x_mu = get_unit_range_from_texture_coord(uv.x, TRANSMITTANCE_TEXTURE_WIDTH);
    let x_r = get_unit_range_from_texture_coord(uv.y, TRANSMITTANCE_TEXTURE_HEIGHT);
    // Distance to the top of the air for a horizontal ray at ground level.
    let h = sqrt(atmosphere.top_radius * atmosphere.top_radius - atmosphere.bottom_radius * atmosphere.bottom_radius);
    // Distance to the horizon, from which r follows.
    let rho = h * x_r;
    let r = sqrt(rho * rho + atmosphere.bottom_radius * atmosphere.bottom_radius);
    // Distance to the top of the air for the ray (r, mu) and its extremes, from which mu follows.
    let d_min = atmosphere.top_radius - r;
    let d_max = rho + h;
    let d = d_min + x_mu * (d_max - d_min);
    var mu = select((h * h - rho * rho - d * d) / (2.0 * r * d), 1.0, d == 0.0);
    mu = clamp_cosine(mu);
    return RMu(r, mu);
}

fn compute_optical_depth_to_top_atmosphere_boundary_texture(
    atmosphere: AtmosphereParameters,
    frag_coord: vec2<f32>,
) -> vec3<f32> {
    let size = vec2<f32>(f32(TRANSMITTANCE_TEXTURE_WIDTH), f32(TRANSMITTANCE_TEXTURE_HEIGHT));
    let r_mu = get_r_mu_from_transmittance_texture_uv(atmosphere, frag_coord / size);
    return compute_optical_depth_to_top_atmosphere_boundary(atmosphere, r_mu.r, r_mu.mu);
}

struct RayleighMie {
    rayleigh: vec3<f32>,
    mie: vec3<f32>,
}

fn compute_single_scattering_integrand(
    atmosphere: AtmosphereParameters,
    transmittance_texture: texture_2d<f32>,
    r: f32,
    mu: f32,
    mu_s: f32,
    nu: f32,
    d: f32,
    ray_r_mu_intersects_ground: bool,
) -> RayleighMie {
    let r_d = clamp_radius(atmosphere, sqrt(d * d + 2.0 * r * mu * d + r * r));
    let mu_s_d = clamp_cosine((r * mu_s + d * nu) / r_d);
    let transmittance = get_transmittance(atmosphere, transmittance_texture, r, mu, d, ray_r_mu_intersects_ground)
        * get_transmittance_to_sun(atmosphere, transmittance_texture, r_d, mu_s_d);
    return RayleighMie(
        transmittance * get_profile_density(atmosphere.rayleigh_density, r_d - atmosphere.bottom_radius),
        transmittance * get_profile_density(atmosphere.mie_density, r_d - atmosphere.bottom_radius),
    );
}

fn distance_to_nearest_atmosphere_boundary(
    atmosphere: AtmosphereParameters,
    r: f32,
    mu: f32,
    ray_r_mu_intersects_ground: bool,
) -> f32 {
    if ray_r_mu_intersects_ground {
        return distance_to_bottom_atmosphere_boundary(atmosphere, r, mu);
    }
    return distance_to_top_atmosphere_boundary(atmosphere, r, mu);
}

fn compute_single_scattering(
    atmosphere: AtmosphereParameters,
    transmittance_texture: texture_2d<f32>,
    r: f32,
    mu: f32,
    mu_s: f32,
    nu: f32,
    ray_r_mu_intersects_ground: bool,
) -> RayleighMie {
    // The trapezoidal rule over `sample_count` intervals that grow quadratically from the viewpoint (Bruneton's are
    // 50 equal ones): along a horizon ray a hazy map's air is opaque within a few kilometres of a path hundreds
    // long, which equal intervals of 15-20 km overstate several times; takram's thin default air agrees either way.
    let sample_count = INTEGRATION_INTERVALS;
    let length = distance_to_nearest_atmosphere_boundary(atmosphere, r, mu, ray_r_mu_intersects_ground);
    var rayleigh_sum = vec3<f32>(0.0);
    var mie_sum = vec3<f32>(0.0);
    var previous = compute_single_scattering_integrand(
        atmosphere, transmittance_texture, r, mu, mu_s, nu, 0.0, ray_r_mu_intersects_ground,
    );
    var previous_d = 0.0;
    for (var i = 1; i <= sample_count; i++) {
        let t = f32(i) / f32(sample_count);
        let d_i = length * t * t;
        let sample = compute_single_scattering_integrand(
            atmosphere, transmittance_texture, r, mu, mu_s, nu, d_i, ray_r_mu_intersects_ground,
        );
        let dx = d_i - previous_d;
        rayleigh_sum += (previous.rayleigh + sample.rayleigh) * (0.5 * dx);
        mie_sum += (previous.mie + sample.mie) * (0.5 * dx);
        previous = sample;
        previous_d = d_i;
    }
    return RayleighMie(
        rayleigh_sum * atmosphere.solar_irradiance * atmosphere.rayleigh_scattering,
        mie_sum * atmosphere.solar_irradiance * atmosphere.mie_scattering,
    );
}

// Intervals of the single- and multiple-scattering integrals (Bruneton: 50 equal ones).
const INTEGRATION_INTERVALS: i32 = 96;

struct ScatteringCoordinates {
    r: f32,
    mu: f32,
    mu_s: f32,
    nu: f32,
    ray_r_mu_intersects_ground: bool,
}

fn get_r_mu_mu_s_nu_from_scattering_texture_uvwz(atmosphere: AtmosphereParameters, uvwz: vec4<f32>) -> ScatteringCoordinates {
    // Distance to the top of the air for a horizontal ray at ground level.
    let h = sqrt(atmosphere.top_radius * atmosphere.top_radius - atmosphere.bottom_radius * atmosphere.bottom_radius);
    // Distance to the horizon.
    let rho = h * get_unit_range_from_texture_coord(uvwz.w, SCATTERING_TEXTURE_R_SIZE);
    let r = sqrt(rho * rho + atmosphere.bottom_radius * atmosphere.bottom_radius);

    var mu: f32;
    var ray_r_mu_intersects_ground: bool;
    if uvwz.z < 0.5 {
        // Distance to the ground for the ray (r, mu) and its extremes over all mu, from which mu follows.
        let d_min = r - atmosphere.bottom_radius;
        let d_max = rho;
        let d = d_min + (d_max - d_min) * get_unit_range_from_texture_coord(1.0 - 2.0 * uvwz.z, SCATTERING_TEXTURE_MU_SIZE / 2);
        mu = select(clamp_cosine(-(rho * rho + d * d) / (2.0 * r * d)), -1.0, d == 0.0);
        ray_r_mu_intersects_ground = true;
    } else {
        // Distance to the top of the air for the ray (r, mu) and its extremes over all mu, from which mu follows.
        let d_min = atmosphere.top_radius - r;
        let d_max = rho + h;
        let d = d_min + (d_max - d_min) * get_unit_range_from_texture_coord(2.0 * uvwz.z - 1.0, SCATTERING_TEXTURE_MU_SIZE / 2);
        mu = select(clamp_cosine((h * h - rho * rho - d * d) / (2.0 * r * d)), 1.0, d == 0.0);
        ray_r_mu_intersects_ground = false;
    }

    let x_mu_s = get_unit_range_from_texture_coord(uvwz.y, SCATTERING_TEXTURE_MU_S_SIZE);
    let d_min = atmosphere.top_radius - atmosphere.bottom_radius;
    let d_max = h;
    let big_d = distance_to_top_atmosphere_boundary(atmosphere, atmosphere.bottom_radius, atmosphere.mu_s_min);
    let big_a = (big_d - d_min) / (d_max - d_min);
    let a = (big_a - x_mu_s * big_a) / (1.0 + x_mu_s * big_a);
    let d = d_min + min(a, big_a) * (d_max - d_min);
    let mu_s = select(clamp_cosine((h * h - d * d) / (2.0 * atmosphere.bottom_radius * d)), 1.0, d == 0.0);

    let nu = clamp_cosine(uvwz.x * 2.0 - 1.0);
    return ScatteringCoordinates(r, mu, mu_s, nu, ray_r_mu_intersects_ground);
}

fn get_r_mu_mu_s_nu_from_scattering_texture_frag_coord(
    atmosphere: AtmosphereParameters,
    frag_coord: vec3<f32>,
) -> ScatteringCoordinates {
    let size = vec4<f32>(
        f32(SCATTERING_TEXTURE_NU_SIZE - 1),
        f32(SCATTERING_TEXTURE_MU_S_SIZE),
        f32(SCATTERING_TEXTURE_MU_SIZE),
        f32(SCATTERING_TEXTURE_R_SIZE),
    );
    let frag_coord_nu = floor(frag_coord.x / f32(SCATTERING_TEXTURE_MU_S_SIZE));
    let frag_coord_mu_s = frag_coord.x - f32(SCATTERING_TEXTURE_MU_S_SIZE) * floor(frag_coord.x / f32(SCATTERING_TEXTURE_MU_S_SIZE));
    let uvwz = vec4<f32>(frag_coord_nu, frag_coord_mu_s, frag_coord.y, frag_coord.z) / size;
    var coordinates = get_r_mu_mu_s_nu_from_scattering_texture_uvwz(atmosphere, uvwz);
    // Clamp nu to its valid range of values, given mu and mu_s.
    let mu = coordinates.mu;
    let mu_s = coordinates.mu_s;
    coordinates.nu = clamp(
        coordinates.nu,
        mu * mu_s - sqrt((1.0 - mu * mu) * (1.0 - mu_s * mu_s)),
        mu * mu_s + sqrt((1.0 - mu * mu) * (1.0 - mu_s * mu_s)),
    );
    return coordinates;
}

fn compute_single_scattering_texture(
    atmosphere: AtmosphereParameters,
    transmittance_texture: texture_2d<f32>,
    frag_coord: vec3<f32>,
) -> RayleighMie {
    let c = get_r_mu_mu_s_nu_from_scattering_texture_frag_coord(atmosphere, frag_coord);
    return compute_single_scattering(atmosphere, transmittance_texture, c.r, c.mu, c.mu_s, c.nu, c.ray_r_mu_intersects_ground);
}

// The light arriving after `scattering_order` - 1 bounces (`GetScattering` with the order): single Rayleigh and
// Mie with their phases for the first order, the multiple-scattering table after it.
fn get_scattering_of_order(
    atmosphere: AtmosphereParameters,
    single_rayleigh_scattering_texture: texture_3d<f32>,
    single_mie_scattering_texture: texture_3d<f32>,
    multiple_scattering_texture: texture_3d<f32>,
    linear_sampler: sampler,
    r: f32,
    mu: f32,
    mu_s: f32,
    nu: f32,
    ray_r_mu_intersects_ground: bool,
    scattering_order: i32,
) -> vec3<f32> {
    if scattering_order == 1 {
        let rayleigh = get_scattering(
            atmosphere, single_rayleigh_scattering_texture, linear_sampler, r, mu, mu_s, nu, ray_r_mu_intersects_ground,
        ).rgb;
        let mie = get_scattering(
            atmosphere, single_mie_scattering_texture, linear_sampler, r, mu, mu_s, nu, ray_r_mu_intersects_ground,
        ).rgb;
        return rayleigh * rayleigh_phase_function(nu) + mie * mie_phase_function(atmosphere.mie_phase_function_g, nu);
    }
    return get_scattering(
        atmosphere, multiple_scattering_texture, linear_sampler, r, mu, mu_s, nu, ray_r_mu_intersects_ground,
    ).rgb;
}

fn compute_scattering_density(
    atmosphere: AtmosphereParameters,
    transmittance_texture: texture_2d<f32>,
    single_rayleigh_scattering_texture: texture_3d<f32>,
    single_mie_scattering_texture: texture_3d<f32>,
    multiple_scattering_texture: texture_3d<f32>,
    irradiance_texture: texture_2d<f32>,
    linear_sampler: sampler,
    r: f32,
    mu: f32,
    mu_s: f32,
    nu: f32,
    scattering_order: i32,
) -> vec3<f32> {
    // Unit vectors for the zenith, the view direction omega and the sun direction omega_s, such that the cosine of
    // the view-zenith angle is mu, of the sun-zenith angle mu_s, and of the view-sun angle nu.
    let zenith_direction = vec3<f32>(0.0, 0.0, 1.0);
    let omega = vec3<f32>(sqrt(1.0 - mu * mu), 0.0, mu);
    let sun_dir_x = select((nu - mu * mu_s) / omega.x, 0.0, omega.x == 0.0);
    let sun_dir_y = sqrt(max(1.0 - sun_dir_x * sun_dir_x - mu_s * mu_s, 0.0));
    let omega_s = vec3<f32>(sun_dir_x, sun_dir_y, mu_s);

    let sample_count = 16;
    let dphi = PI / f32(sample_count);
    let dtheta = PI / f32(sample_count);
    var rayleigh_mie = vec3<f32>(0.0);

    let rayleigh_density = get_profile_density(atmosphere.rayleigh_density, r - atmosphere.bottom_radius);
    let mie_density = get_profile_density(atmosphere.mie_density, r - atmosphere.bottom_radius);

    // Nested loops for the integral over all the incident directions omega_i.
    for (var l = 0; l < sample_count; l++) {
        let theta = (f32(l) + 0.5) * dtheta;
        let cos_theta = cos(theta);
        let sin_theta = sin(theta);
        let ray_r_theta_intersects_ground = ray_intersects_ground(atmosphere, r, cos_theta);

        // The distance and transmittance to the ground depend on theta only.
        var distance_to_ground = 0.0;
        var transmittance_to_ground = vec3<f32>(0.0);
        var ground_albedo = vec3<f32>(0.0);
        if ray_r_theta_intersects_ground {
            distance_to_ground = distance_to_bottom_atmosphere_boundary(atmosphere, r, cos_theta);
            transmittance_to_ground = get_transmittance(atmosphere, transmittance_texture, r, cos_theta, distance_to_ground, true);
            ground_albedo = atmosphere.ground_albedo;
        }

        for (var m = 0; m < 2 * sample_count; m++) {
            let phi = (f32(m) + 0.5) * dphi;
            let omega_i = vec3<f32>(cos(phi) * sin_theta, sin(phi) * sin_theta, cos_theta);
            let domega_i = dtheta * dphi * sin(theta);

            // The radiance arriving from omega_i after n - 1 bounces: the table of the (n - 1)th order...
            let nu_a = dot(omega_s, omega_i);
            var incident_radiance = get_scattering_of_order(
                atmosphere,
                single_rayleigh_scattering_texture,
                single_mie_scattering_texture,
                multiple_scattering_texture,
                linear_sampler,
                r,
                omega_i.z,
                mu_s,
                nu_a,
                ray_r_theta_intersects_ground,
                scattering_order - 1,
            );

            // ...and the paths whose last bounce is on the ground: transmittance to it, its albedo, its BRDF and
            // the irradiance it received after n - 2 bounces.
            let ground_normal = normalize(zenith_direction * r + omega_i * distance_to_ground);
            let ground_irradiance = get_irradiance(
                atmosphere, irradiance_texture, linear_sampler, atmosphere.bottom_radius, dot(ground_normal, omega_s),
            );
            incident_radiance += transmittance_to_ground * ground_albedo * (1.0 / PI) * ground_irradiance;

            // Scattered from omega_i toward -omega: the incident radiance, the scattering coefficient and the phase
            // function for omega and omega_i, summed over Rayleigh and Mie.
            let nu_b = dot(omega, omega_i);
            rayleigh_mie += incident_radiance
                * (atmosphere.rayleigh_scattering * rayleigh_density * rayleigh_phase_function(nu_b)
                    + atmosphere.mie_scattering * mie_density * mie_phase_function(atmosphere.mie_phase_function_g, nu_b))
                * domega_i;
        }
    }
    return rayleigh_mie;
}

fn compute_multiple_scattering(
    atmosphere: AtmosphereParameters,
    transmittance_texture: texture_2d<f32>,
    scattering_density_texture: texture_3d<f32>,
    linear_sampler: sampler,
    r: f32,
    mu: f32,
    mu_s: f32,
    nu: f32,
    ray_r_mu_intersects_ground: bool,
) -> vec3<f32> {
    // The trapezoidal rule over intervals growing quadratically from the viewpoint (see compute_single_scattering).
    let sample_count = INTEGRATION_INTERVALS;
    let length = distance_to_nearest_atmosphere_boundary(atmosphere, r, mu, ray_r_mu_intersects_ground);
    var rayleigh_mie_sum = vec3<f32>(0.0);
    var previous = vec3<f32>(0.0);
    var previous_d = 0.0;
    for (var i = 0; i <= sample_count; i++) {
        let t = f32(i) / f32(sample_count);
        let d_i = length * t * t;
        // r, mu and mu_s at the integration point.
        let r_i = clamp_radius(atmosphere, sqrt(d_i * d_i + 2.0 * r * mu * d_i + r * r));
        let mu_i = clamp_cosine((r * mu + d_i) / r_i);
        let mu_s_i = clamp_cosine((r * mu_s + d_i * nu) / r_i);
        let rayleigh_mie_i = get_scattering(
            atmosphere, scattering_density_texture, linear_sampler, r_i, mu_i, mu_s_i, nu, ray_r_mu_intersects_ground,
        ).rgb
            * get_transmittance(atmosphere, transmittance_texture, r, mu, d_i, ray_r_mu_intersects_ground);
        if i > 0 {
            rayleigh_mie_sum += (previous + rayleigh_mie_i) * (0.5 * (d_i - previous_d));
        }
        previous = rayleigh_mie_i;
        previous_d = d_i;
    }
    return rayleigh_mie_sum;
}

fn compute_direct_irradiance(
    atmosphere: AtmosphereParameters,
    transmittance_texture: texture_2d<f32>,
    r: f32,
    mu_s: f32,
) -> vec3<f32> {
    let alpha_s = atmosphere.sun_angular_radius;
    // Approximate average of the cosine factor mu_s over the visible fraction of the sun's disc.
    var average_cosine_factor: f32;
    if mu_s < -alpha_s {
        average_cosine_factor = 0.0;
    } else if mu_s > alpha_s {
        average_cosine_factor = mu_s;
    } else {
        average_cosine_factor = (mu_s + alpha_s) * (mu_s + alpha_s) / (4.0 * alpha_s);
    }
    return atmosphere.solar_irradiance
        * get_transmittance_to_top_atmosphere_boundary(atmosphere, transmittance_texture, r, mu_s)
        * average_cosine_factor;
}

fn compute_indirect_irradiance(
    atmosphere: AtmosphereParameters,
    single_rayleigh_scattering_texture: texture_3d<f32>,
    single_mie_scattering_texture: texture_3d<f32>,
    multiple_scattering_texture: texture_3d<f32>,
    linear_sampler: sampler,
    r: f32,
    mu_s: f32,
    scattering_order: i32,
) -> vec3<f32> {
    let sample_count = 32;
    let dphi = PI / f32(sample_count);
    let dtheta = PI / f32(sample_count);

    var result = vec3<f32>(0.0);
    let omega_s = vec3<f32>(sqrt(1.0 - mu_s * mu_s), 0.0, mu_s);
    for (var j = 0; j < sample_count / 2; j++) {
        let theta = (f32(j) + 0.5) * dtheta;
        for (var i = 0; i < 2 * sample_count; i++) {
            let phi = (f32(i) + 0.5) * dphi;
            let omega = vec3<f32>(cos(phi) * sin(theta), sin(phi) * sin(theta), cos(theta));
            let domega = dtheta * dphi * sin(theta);
            let nu = dot(omega, omega_s);
            result += get_scattering_of_order(
                atmosphere,
                single_rayleigh_scattering_texture,
                single_mie_scattering_texture,
                multiple_scattering_texture,
                linear_sampler,
                r,
                omega.z,
                mu_s,
                nu,
                false,
                scattering_order,
            ) * omega.z * domega;
        }
    }
    return result;
}

struct RMuS {
    r: f32,
    mu_s: f32,
}

fn get_r_mu_s_from_irradiance_texture_uv(atmosphere: AtmosphereParameters, uv: vec2<f32>) -> RMuS {
    let x_mu_s = get_unit_range_from_texture_coord(uv.x, IRRADIANCE_TEXTURE_WIDTH);
    let x_r = get_unit_range_from_texture_coord(uv.y, IRRADIANCE_TEXTURE_HEIGHT);
    return RMuS(
        atmosphere.bottom_radius + x_r * (atmosphere.top_radius - atmosphere.bottom_radius),
        clamp_cosine(2.0 * x_mu_s - 1.0),
    );
}

fn irradiance_texture_size() -> vec2<f32> {
    return vec2<f32>(f32(IRRADIANCE_TEXTURE_WIDTH), f32(IRRADIANCE_TEXTURE_HEIGHT));
}

fn compute_direct_irradiance_texture(
    atmosphere: AtmosphereParameters,
    transmittance_texture: texture_2d<f32>,
    frag_coord: vec2<f32>,
) -> vec3<f32> {
    let c = get_r_mu_s_from_irradiance_texture_uv(atmosphere, frag_coord / irradiance_texture_size());
    return compute_direct_irradiance(atmosphere, transmittance_texture, c.r, c.mu_s);
}

fn compute_indirect_irradiance_texture(
    atmosphere: AtmosphereParameters,
    single_rayleigh_scattering_texture: texture_3d<f32>,
    single_mie_scattering_texture: texture_3d<f32>,
    multiple_scattering_texture: texture_3d<f32>,
    linear_sampler: sampler,
    frag_coord: vec2<f32>,
    scattering_order: i32,
) -> vec3<f32> {
    let c = get_r_mu_s_from_irradiance_texture_uv(atmosphere, frag_coord / irradiance_texture_size());
    return compute_indirect_irradiance(
        atmosphere,
        single_rayleigh_scattering_texture,
        single_mie_scattering_texture,
        multiple_scattering_texture,
        linear_sampler,
        c.r,
        c.mu_s,
        scattering_order,
    );
}

// --- runtime.glsl ---------------------------------------------------------------------------------------------

// The scattered light along a view ray split as the game recombines it (`naval_sky::atmosphere`): single Rayleigh
// (no phase), the higher orders (divided by the Rayleigh phase, as the tables store them), single Mie (no phase),
// and the transmittance. takram's GetSkyRadiance returns (rayleigh + multiple) * RayleighPhaseFunction(nu) +
// mie * MiePhaseFunction(g, nu); [`combine`] does exactly that.
struct Scattered {
    rayleigh: vec3<f32>,
    multiple: vec3<f32>,
    mie: vec3<f32>,
    transmittance: vec3<f32>,
}

fn combine(atmosphere: AtmosphereParameters, scattered: Scattered, nu: f32) -> vec3<f32> {
    return (scattered.rayleigh + scattered.multiple) * rayleigh_phase_function(nu)
        + scattered.mie * mie_phase_function(atmosphere.mie_phase_function_g, nu);
}

struct Combined {
    // Rayleigh single and multiple scattering, as the scattering table holds them.
    scattering: vec3<f32>,
    single_mie: vec3<f32>,
    higher_order: vec3<f32>,
}

// `GetCombinedScattering` (separate single-Mie table) and the higher-order table at the same coordinates.
fn get_combined_scattering(
    atmosphere: AtmosphereParameters,
    scattering_texture: texture_3d<f32>,
    single_mie_scattering_texture: texture_3d<f32>,
    higher_order_scattering_texture: texture_3d<f32>,
    linear_sampler: sampler,
    r: f32,
    mu: f32,
    mu_s: f32,
    nu: f32,
    ray_r_mu_intersects_ground: bool,
) -> Combined {
    let uvwz = get_scattering_texture_uvwz_from_r_mu_mu_s_nu(atmosphere, r, mu, mu_s, nu, ray_r_mu_intersects_ground);
    let tex_coord_x = uvwz.x * f32(SCATTERING_TEXTURE_NU_SIZE - 1);
    let tex_x = floor(tex_coord_x);
    let lerp = tex_coord_x - tex_x;
    let uvw_a = vec3<f32>((tex_x + uvwz.y) / f32(SCATTERING_TEXTURE_NU_SIZE), uvwz.z, uvwz.w);
    let uvw_b = vec3<f32>((tex_x + 1.0 + uvwz.y) / f32(SCATTERING_TEXTURE_NU_SIZE), uvwz.z, uvwz.w);
    let scattering = textureSampleLevel(scattering_texture, linear_sampler, uvw_a, 0.0).rgb * (1.0 - lerp)
        + textureSampleLevel(scattering_texture, linear_sampler, uvw_b, 0.0).rgb * lerp;
    let single_mie = textureSampleLevel(single_mie_scattering_texture, linear_sampler, uvw_a, 0.0).rgb * (1.0 - lerp)
        + textureSampleLevel(single_mie_scattering_texture, linear_sampler, uvw_b, 0.0).rgb * lerp;
    let higher_order = textureSampleLevel(higher_order_scattering_texture, linear_sampler, uvw_a, 0.0).rgb * (1.0 - lerp)
        + textureSampleLevel(higher_order_scattering_texture, linear_sampler, uvw_b, 0.0).rgb * lerp;
    return Combined(scattering, single_mie, higher_order);
}

// `GetSkyRadiance` (with GROUND: rays toward the ground read the ground half of the tables), split.
fn get_sky_radiance(
    atmosphere: AtmosphereParameters,
    transmittance_texture: texture_2d<f32>,
    scattering_texture: texture_3d<f32>,
    single_mie_scattering_texture: texture_3d<f32>,
    higher_order_scattering_texture: texture_3d<f32>,
    linear_sampler: sampler,
    camera_in: vec3<f32>,
    view_ray: vec3<f32>,
    shadow_length: f32,
    sun_direction: vec3<f32>,
) -> Scattered {
    var camera = camera_in;
    // Distance to the top of the air along the view ray, assuming the viewer is in space.
    var r = length(camera);
    var rmu = dot(camera, view_ray);
    // takram: SafeSqrt (three-geospatial PR 26).
    let distance_to_top = -rmu - safe_sqrt(rmu * rmu - r * r + atmosphere.top_radius * atmosphere.top_radius);
    // A viewer in space whose ray meets the air moves to the top of the air along it.
    if distance_to_top > 0.0 {
        camera = camera + view_ray * distance_to_top;
        r = atmosphere.top_radius;
        rmu += distance_to_top;
    } else if r > atmosphere.top_radius {
        // The ray misses the air.
        return Scattered(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    let mu = rmu / r;
    let mu_s = dot(camera, sun_direction) / r;
    let nu = dot(view_ray, sun_direction);
    let ray_r_mu_intersects_ground = ray_intersects_ground(atmosphere, r, mu);

    let transmittance = select(
        get_transmittance_to_top_atmosphere_boundary(atmosphere, transmittance_texture, r, mu),
        vec3<f32>(0.0),
        ray_r_mu_intersects_ground,
    );
    if shadow_length == 0.0 {
        let c = get_combined_scattering(
            atmosphere, scattering_texture, single_mie_scattering_texture, higher_order_scattering_texture,
            linear_sampler, r, mu, mu_s, nu, ray_r_mu_intersects_ground,
        );
        return Scattered(max(c.scattering - c.higher_order, vec3<f32>(0.0)), c.higher_order, c.single_mie, transmittance);
    }
    // Light shafts (shadow_length is the total length l in the paper): omit the scattering between the camera and
    // the point at distance l (Eq. 18), and (takram) occlude only single scattering.
    let d = shadow_length;
    let r_p = clamp_radius(atmosphere, sqrt(d * d + 2.0 * r * mu * d + r * r));
    let mu_p = (r * mu + d) / r_p;
    let mu_s_p = (r * mu_s + d * nu) / r_p;
    let c = get_combined_scattering(
        atmosphere, scattering_texture, single_mie_scattering_texture, higher_order_scattering_texture,
        linear_sampler, r_p, mu_p, mu_s_p, nu, ray_r_mu_intersects_ground,
    );
    let shadow_transmittance = get_transmittance(
        atmosphere, transmittance_texture, r, mu, shadow_length, ray_r_mu_intersects_ground,
    );
    let single = max(c.scattering - c.higher_order, vec3<f32>(0.0));
    return Scattered(single * shadow_transmittance, c.higher_order, c.single_mie * shadow_transmittance, transmittance);
}

// takram: the point on the segment from `camera` to `point` closest to the planet's centre.
fn closest_point_on_ray(camera: vec3<f32>, point: vec3<f32>) -> vec3<f32> {
    let ray = point - camera;
    let t = clamp(-dot(camera, ray) / dot(ray, ray), 0.0, 1.0);
    return camera + t * ray;
}

fn ray_sphere_intersections(camera: vec3<f32>, direction: vec3<f32>, radius: f32) -> vec2<f32> {
    let b = 2.0 * dot(direction, camera);
    let c = dot(camera, camera) - radius * radius;
    let discriminant = b * b - 4.0 * c;
    let q = sqrt(discriminant);
    return vec2<f32>(-b - q, -b + q) * 0.5;
}

struct Clipped {
    camera: vec3<f32>,
    point: vec3<f32>,
    // Both ends below the bottom of the air.
    below: bool,
}

// takram: clip the view segment at the bottom of the air.
fn clip_at_bottom_atmosphere(atmosphere: AtmosphereParameters, view_ray: vec3<f32>, camera: vec3<f32>, point: vec3<f32>) -> Clipped {
    let bottom_radius = atmosphere.bottom_radius;
    let camera_below = length(camera) < bottom_radius;
    let point_below = length(point) < bottom_radius;
    let t = ray_sphere_intersections(camera, view_ray, bottom_radius);
    let intersection = camera + view_ray * select(t.x, t.y, camera_below);
    return Clipped(
        select(camera, intersection, camera_below),
        select(point, intersection, point_below),
        camera_below && point_below,
    );
}

// `GetSkyRadianceToPoint` with light shafts and takram's changes, split.
fn get_sky_radiance_to_point(
    atmosphere: AtmosphereParameters,
    transmittance_texture: texture_2d<f32>,
    scattering_texture: texture_3d<f32>,
    single_mie_scattering_texture: texture_3d<f32>,
    higher_order_scattering_texture: texture_3d<f32>,
    linear_sampler: sampler,
    camera_in: vec3<f32>,
    point_in: vec3<f32>,
    shadow_length: f32,
    sun_direction: vec3<f32>,
) -> Scattered {
    let none = Scattered(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(1.0));
    // takram: no artefacts where the segment never meets the top of the air.
    if length(closest_point_on_ray(camera_in, point_in)) > atmosphere.top_radius {
        return none;
    }
    let view_ray = normalize(point_in - camera_in);
    let clipped = clip_at_bottom_atmosphere(atmosphere, view_ray, camera_in, point_in);
    if clipped.below {
        return none;
    }
    var camera = clipped.camera;
    let point = clipped.point;

    // Distance to the top of the air along the view ray, assuming the viewer is in space.
    var r = length(camera);
    var rmu = dot(camera, view_ray);
    let distance_to_top = -rmu - safe_sqrt(rmu * rmu - r * r + atmosphere.top_radius * atmosphere.top_radius);
    if distance_to_top > 0.0 {
        camera = camera + view_ray * distance_to_top;
        r = atmosphere.top_radius;
        rmu += distance_to_top;
    }

    // r, mu, mu_s and nu for the first lookup.
    var mu = rmu / r;
    let mu_s = dot(camera, sun_direction) / r;
    let nu = dot(view_ray, sun_direction);
    var d = length(point - camera);
    let ray_r_mu_intersects_ground = ray_intersects_ground(atmosphere, r, mu);

    // takram: avoid artefacts near the horizon from the tables' finite resolution and float precision
    // (precomputed_atmospheric_scattering PR 32).
    if !ray_r_mu_intersects_ground {
        let mu_horizon = -safe_sqrt(1.0 - (atmosphere.bottom_radius * atmosphere.bottom_radius) / (r * r));
        mu = max(mu, mu_horizon + 0.004);
    }

    let transmittance = get_transmittance(atmosphere, transmittance_texture, r, mu, d, ray_r_mu_intersects_ground);
    let c = get_combined_scattering(
        atmosphere, scattering_texture, single_mie_scattering_texture, higher_order_scattering_texture,
        linear_sampler, r, mu, mu_s, nu, ray_r_mu_intersects_ground,
    );

    // r, mu, mu_s and nu for the second lookup: with shafts, ignore the scattering along the last shadow_length of
    // the ray (Eq. 17).
    d = max(d - shadow_length, 0.0);
    let r_p = clamp_radius(atmosphere, sqrt(d * d + 2.0 * r * mu * d + r * r));
    let mu_p = (r * mu + d) / r_p;
    let mu_s_p = (r * mu_s + d * nu) / r_p;
    let c_p = get_combined_scattering(
        atmosphere, scattering_texture, single_mie_scattering_texture, higher_order_scattering_texture,
        linear_sampler, r_p, mu_p, mu_s_p, nu, ray_r_mu_intersects_ground,
    );

    // The scattering between the camera and the point.
    var shadow_transmittance = transmittance;
    if shadow_length > 0.0 {
        // T(x, x_s) of Eq. 17, for light shafts.
        shadow_transmittance = get_transmittance(atmosphere, transmittance_texture, r, mu, d, ray_r_mu_intersects_ground);
    }
    // takram: occlude only single Rayleigh scattering by the shadow.
    let single = max(c.scattering - c.higher_order, vec3<f32>(0.0));
    let single_p = max(c_p.scattering - c_p.higher_order, vec3<f32>(0.0));
    var single_mie = c.single_mie - shadow_transmittance * c_p.single_mie;
    // Hack against artefacts with the sun below the horizon.
    single_mie = single_mie * smoothstep(0.0, 0.01, mu_s);
    return Scattered(
        single - shadow_transmittance * single_p,
        c.higher_order - transmittance * c_p.higher_order,
        single_mie,
        transmittance,
    );
}

struct SunAndSky {
    sun: vec3<f32>,
    skylight: vec3<f32>,
}

// `GetSunAndSkyIrradiance`: on a surface with `normal` (the indirect part approximated off the horizontal).
fn get_sun_and_sky_irradiance(
    atmosphere: AtmosphereParameters,
    transmittance_texture: texture_2d<f32>,
    irradiance_texture: texture_2d<f32>,
    linear_sampler: sampler,
    point: vec3<f32>,
    normal: vec3<f32>,
    sun_direction: vec3<f32>,
) -> SunAndSky {
    let r = length(point);
    let mu_s = dot(point, sun_direction) / r;
    let sky = get_irradiance(atmosphere, irradiance_texture, linear_sampler, r, mu_s) * (1.0 + dot(normal, point) / r) * 0.5;
    let sun = atmosphere.solar_irradiance
        * get_transmittance_to_sun(atmosphere, transmittance_texture, r, mu_s)
        * max(dot(normal, sun_direction), 0.0);
    return SunAndSky(sun, sky);
}

// takram's `GetSunAndSkyScalarIrradiance` (for the clouds): the sky over the whole sphere (2π), the sun without its
// cosine term.
fn get_sun_and_sky_scalar_irradiance(
    atmosphere: AtmosphereParameters,
    transmittance_texture: texture_2d<f32>,
    irradiance_texture: texture_2d<f32>,
    linear_sampler: sampler,
    point: vec3<f32>,
    sun_direction: vec3<f32>,
) -> SunAndSky {
    let r = length(point);
    let mu_s = dot(point, sun_direction) / r;
    let sky = get_irradiance(atmosphere, irradiance_texture, linear_sampler, r, mu_s) * 2.0 * PI;
    let sun = atmosphere.solar_irradiance * get_transmittance_to_sun(atmosphere, transmittance_texture, r, mu_s);
    return SunAndSky(sun, sky);
}
