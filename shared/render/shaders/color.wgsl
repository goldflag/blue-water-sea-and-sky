// Color helpers shared by every crate. Ports three.js r185
// src/nodes/display/ColorSpaceFunctions.js and the Rec. 709 luminance of
// src/math/ColorManagement.js (the working space is linear sRGB).
#define_import_path naval::color

const REC709_LUMINANCE: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

fn luminance(color: vec3<f32>) -> f32 {
    return dot(color, REC709_LUMINANCE);
}

// three's sRGBTransferOETF: linear to sRGB-encoded, for a target that does not
// encode in hardware.
fn srgb_oetf(color: vec3<f32>) -> vec3<f32> {
    let a = pow(max(color, vec3<f32>(0.0)), vec3<f32>(0.41666)) * 1.055 - 0.055;
    let b = color * 12.92;
    return select(a, b, color <= vec3<f32>(0.0031308));
}

// three's sRGBTransferEOTF: sRGB-encoded to linear.
fn srgb_eotf(color: vec3<f32>) -> vec3<f32> {
    let a = pow(max(color * 0.9478672986 + 0.0521327014, vec3<f32>(0.0)), vec3<f32>(2.4));
    let b = color * 0.0773993808;
    return select(a, b, color <= vec3<f32>(0.04045));
}
