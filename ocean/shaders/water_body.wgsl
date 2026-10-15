// Ports the web game's `ocean/surface/waterBody.ts` (`OceanRealism.waterColor`): the water body's colour from its inherent
// optical properties, light the sea scatters back out rather than an emitted pigment. Sunlight and skylight cross the
// surface, are absorbed and backscattered, and what comes back up crosses the surface again. Remote-sensing
// reflectance after Lee, Carder & Arnone (2002); optical constants after Morel (1974, 1988) and Mobley (1994).
#define_import_path naval_ocean::water_body

#import naval_ocean::reflection::schlick

// Backscattering (1/m) at about 620, 550 and 460 nm: sea water's own plus particles (Jerlov's oceanic II–III).
const BACKSCATTER: vec3<f32> = vec3<f32>(0.00323, 0.00395, 0.00565);
const LEE_G0: f32 = 0.089;
const LEE_G1: f32 = 0.125;
const INTERNAL_RETURN: f32 = 1.7;
// Fresnel reflectance of a uniform sky averaged over the hemisphere.
const SKY_FRESNEL: f32 = 0.066;
const REFRACTION_SQUARED: f32 = 1.776889;
// Mean cosine of daylight under the surface.
const MEAN_COSINE: f32 = 0.85;
// Share of the day's light that reaches a submerged object through the surface.
const SUBMERGED_LIGHT: f32 = 0.97;

// Subsurface remote-sensing reflectance r_rs (1/sr) of optically deep water, including the light the surface returns.
fn subsurface_reflectance(absorption: vec3<f32>, scatter: vec3<f32>) -> vec3<f32> {
    let ratio = scatter / max(absorption + scatter, vec3<f32>(1e-6));
    let reflectance = ratio * (ratio * LEE_G1 + LEE_G0);
    return reflectance / (1.0 - reflectance * INTERNAL_RETURN);
}

// Radiance just under the surface looking down into deep water: r_rs times the downwelling irradiance of the sun (less
// its Fresnel reflection, where the shadow `lit` lets it) and the sky (less its hemispherical Fresnel average).
fn upwelling(reflectance: vec3<f32>, sun: vec3<f32>, sun_irradiance: vec3<f32>, sky: vec3<f32>, lit: f32) -> vec3<f32> {
    let height = max(sun.y, 0.0);
    let sunlit = reflectance * sun_irradiance * (height * (1.0 - schlick(height)));
    return sunlit * lit + reflectance * sky * (1.0 - SKY_FRESNEL);
}

// Share of a submerged object's light that survives the column `column` (m, along the view ray) of water above it.
fn column_transmittance(absorption: vec3<f32>, scatter: vec3<f32>, column: f32, view: vec3<f32>) -> vec3<f32> {
    let attenuation = absorption + scatter;
    return exp(-(attenuation * (column * (1.0 + abs(view.y) / MEAN_COSINE))));
}

// The water body seen from above, before the surface's own Fresnel transmission: the opaque scene behind the column
// where it is thin, filling in with the deep water's upwelling, all crossing into air (÷ n²).
fn water_body(scene: vec3<f32>, deep: vec3<f32>, through: vec3<f32>) -> vec3<f32> {
    return (scene * through * SUBMERGED_LIGHT + deep * (1.0 - through)) / REFRACTION_SQUARED;
}
