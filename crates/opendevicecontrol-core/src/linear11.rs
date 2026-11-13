//! PMBus LINEAR11 number format used by Corsair HXi/RMi power supplies.
//!
//! A 16-bit word holds a 5-bit two's complement exponent (bits 15..11) and an
//! 11-bit two's complement mantissa (bits 10..0): `value = mantissa * 2^exponent`.

pub fn decode(raw: u16) -> f64 {
    let exp = (raw as i16) >> 11;
    let mant = (((raw & 0x07ff) as i16) << 5) >> 5;
    mant as f64 * 2f64.powi(exp as i32)
}

/// Encode a value as LINEAR11, picking the exponent that keeps the most precision.
pub fn encode(value: f64) -> u16 {
    if value == 0.0 {
        return 0;
    }
    for exp in -16i32..=15 {
        let mant = value / 2f64.powi(exp);
        if (-1024.0..=1023.0).contains(&mant) {
            let m = mant.round() as i16 as u16 & 0x07ff;
            let e = (exp as i16 as u16 & 0x1f) << 11;
            return e | m;
        }
    }
    // Out of range: saturate.
    if value > 0.0 { 0x7bff } else { 0x7c00 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_known_values() {
        // 0xF8C8: exp=-1, mant=200 -> 100.0
        assert_eq!(decode(0xF8C8), 100.0);
        // 0xD34E: exp=-6, mant=0x34E=846 -> 13.21875
        assert!((decode(0xD34E) - 13.21875).abs() < 1e-9);
        // negative mantissa
        assert_eq!(decode(0x07FF), -1.0);
        assert_eq!(decode(0x0000), 0.0);
    }

    #[test]
    fn roundtrip() {
        for v in [0.5, 1.0, 12.03, 33.3, 120.0, 845.0, 1500.0, 0.0125] {
            let back = decode(encode(v));
            assert!((back - v).abs() / v < 0.002, "{v} -> {back}");
        }
    }
}
