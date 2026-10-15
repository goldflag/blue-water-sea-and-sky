//! f32 → IEEE half-float bits, round to nearest even (three's `DataUtils.toHalfFloat` rounds the same way
//! for the values the sky uploads), for the half-float tables the sky fills from the CPU.

pub fn f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x007f_ffff;
    if exponent == 0xff {
        // Infinity or NaN.
        return sign | 0x7c00 | if mantissa != 0 { 0x200 } else { 0 };
    }
    let unbiased = exponent - 127;
    if unbiased > 15 {
        return sign | 0x7c00;
    }
    if unbiased >= -14 {
        // Normal half: round the 23-bit mantissa to 10 bits.
        let half_exponent = (unbiased + 15) as u32;
        let mut half = (half_exponent << 10) | (mantissa >> 13);
        let rest = mantissa & 0x1fff;
        if rest > 0x1000 || (rest == 0x1000 && (half & 1) == 1) {
            half += 1; // May carry into the exponent, which is still correct (up to infinity).
        }
        return sign | half as u16;
    }
    if unbiased < -25 {
        return sign;
    }
    // Subnormal half.
    let full = mantissa | 0x0080_0000;
    let shift = (-14 - unbiased + 13) as u32;
    let mut half = full >> shift;
    let rest = full & ((1 << shift) - 1);
    let halfway = 1 << (shift - 1);
    if rest > halfway || (rest == halfway && (half & 1) == 1) {
        half += 1;
    }
    sign | half as u16
}

#[cfg(test)]
mod tests {
    use super::f16_bits;

    #[test]
    fn known_values() {
        assert_eq!(f16_bits(0.0), 0x0000);
        assert_eq!(f16_bits(1.0), 0x3c00);
        assert_eq!(f16_bits(-2.0), 0xc000);
        assert_eq!(f16_bits(40.0), 0x5100);
        assert_eq!(f16_bits(65504.0), 0x7bff);
        assert_eq!(f16_bits(1e6), 0x7c00);
        assert_eq!(f16_bits(0.5), 0x3800);
        assert_eq!(f16_bits(6.1035156e-5), 0x0400);
        assert_eq!(f16_bits(5.9604645e-8), 0x0001);
    }
}
