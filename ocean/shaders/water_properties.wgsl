// Geographic optics shared by the surface, underwater view and motes. Layout twins: optics.rs.
#define_import_path naval_ocean::water_properties

struct WaterArea {
    shape: vec4<f32>,
    absorption: vec4<f32>,
    backscatter: vec4<f32>,
}
struct WaterField {
    absorption: vec4<f32>,
    backscatter: vec4<f32>,
    areas: array<WaterArea, 12>,
}
struct WaterOptics {
    absorption: vec3<f32>,
    backscatter: vec3<f32>,
}

fn sample_water(field: WaterField, xz: vec2<f32>, fallback: vec3<f32>) -> WaterOptics {
    var out = WaterOptics(fallback, vec3<f32>(0.00323, 0.00395, 0.00565));
    if field.absorption.w < 0.5 { return out; }
    out = WaterOptics(field.absorption.rgb, field.backscatter.rgb);
    for (var i = 0u; i < min(u32(field.backscatter.w), 12u); i++) {
        let area = field.areas[i];
        let p = (xz - area.shape.xy) * area.shape.zw;
        let share = (1.0 - smoothstep(0.15, 1.0, dot(p, p))) * area.absorption.w;
        out.absorption = mix(out.absorption, area.absorption.rgb, share);
        out.backscatter = mix(out.backscatter, area.backscatter.rgb, share);
    }
    return out;
}

// A first-order particle scattering/backscatter ratio, shared by surface transmission and underwater visibility.
fn water_beam(optics: WaterOptics) -> vec3<f32> {
    return optics.absorption + optics.backscatter * 8.0;
}

// Preserve the scene's daylight/night scaling, changing its underwater colour by the same reflectance ratio as the
// surface. The legacy underwater illumination is approximate; this does not introduce an independent water tint.
fn water_pigment(optics: WaterOptics, fallback: vec3<f32>, pigment: vec3<f32>) -> vec3<f32> {
    let old_scatter = vec3<f32>(0.00323, 0.00395, 0.00565);
    let old_ratio = old_scatter / max(fallback + old_scatter, vec3<f32>(1e-6));
    let ratio = optics.backscatter / max(optics.absorption + optics.backscatter, vec3<f32>(1e-6));
    let old_r = old_ratio * (old_ratio * 0.125 + 0.089);
    let r = ratio * (ratio * 0.125 + 0.089);
    return pigment * (r / (1.0 - r * 1.7)) / max(old_r / (1.0 - old_r * 1.7), vec3<f32>(1e-6));
}
