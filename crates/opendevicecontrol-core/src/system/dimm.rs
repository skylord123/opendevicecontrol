//! Memory module (DIMM) temperatures over SMBus.
//!
//! * DDR5: every module's SPD hub (SPD5118, addresses 0x50–0x57) has a
//!   temperature sensor: device type in MR0/MR1 (0x51 0x18), temperature in
//!   MR49/MR50 (bits 15:2, two's complement, 0.25 °C).
//! * DDR4: modules with a thermal sensor have a JEDEC JC-42.4 TSOD at
//!   0x18–0x1F: temperature register 5 (big-endian, bits 12:0 signed,
//!   0.0625 °C), manufacturer id in register 6.
//!
//! Only reads. Linux has the `spd5118` and `jc42` drivers for the same
//! (hwmon); the Windows sensor service reads them through PawnIO's SMBus
//! modules.

use crate::smbus::SmBus;

/// SPD5118 device type, MR0 / MR1.
const SPD5118_TYPE: (u8, u8) = (0x51, 0x18);
const SPD5118_TEMP: u8 = 0x31;
const JC42_TEMP: u8 = 0x05;
const JC42_MANUFACTURER: u8 = 0x06;

/// A module's temperature sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensor {
    Spd5118(u8),
    Jc42(u8),
}

impl Sensor {
    pub fn addr(self) -> u8 {
        match self {
            Sensor::Spd5118(a) | Sensor::Jc42(a) => a,
        }
    }

    /// Slot number from the address (0x50 / 0x18 → 1).
    pub fn slot(self) -> u8 {
        match self {
            Sensor::Spd5118(a) => a - 0x50 + 1,
            Sensor::Jc42(a) => a - 0x18 + 1,
        }
    }
}

/// SPD5118 MR49/MR50 → °C.
pub fn spd5118_celsius(lo: u8, hi: u8) -> f64 {
    let raw = (u16::from_le_bytes([lo, hi]) >> 2) & 0x7FF;
    let v = if raw & 0x400 != 0 { raw as i32 - 0x800 } else { raw as i32 };
    f64::from(v) * 0.25
}

/// JC-42.4 temperature register (as an SMBus word: low byte first on the
/// wire, but the sensor sends the high byte first) → °C.
pub fn jc42_celsius(word: u16) -> f64 {
    let be = word.swap_bytes() & 0x1FFF;
    let v = if be & 0x1000 != 0 { be as i32 - 0x2000 } else { be as i32 };
    f64::from(v) * 0.0625
}

fn plausible(t: f64) -> bool {
    t > 0.0 && t < 125.0
}

/// Find the modules' temperature sensors on a bus.
pub fn scan(bus: &mut dyn SmBus) -> Vec<Sensor> {
    let mut found = Vec::new();
    for addr in 0x50..=0x57u8 {
        let ty = (bus.read_byte_data(addr, 0x00), bus.read_byte_data(addr, 0x01));
        if let (Ok(a), Ok(b)) = ty {
            if (a, b) == SPD5118_TYPE && read(bus, Sensor::Spd5118(addr)).is_some() {
                found.push(Sensor::Spd5118(addr));
            }
        }
    }
    if found.is_empty() {
        for addr in 0x18..=0x1Fu8 {
            let maker = bus.read_word_data(addr, JC42_MANUFACTURER).map(u16::swap_bytes);
            if matches!(maker, Ok(m) if m != 0 && m != 0xFFFF) && read(bus, Sensor::Jc42(addr)).is_some() {
                found.push(Sensor::Jc42(addr));
            }
        }
    }
    found
}

/// One module's temperature, °C.
pub fn read(bus: &mut dyn SmBus, s: Sensor) -> Option<f64> {
    let t = match s {
        Sensor::Spd5118(a) => {
            let lo = bus.read_byte_data(a, SPD5118_TEMP).ok()?;
            let hi = bus.read_byte_data(a, SPD5118_TEMP + 1).ok()?;
            spd5118_celsius(lo, hi)
        }
        Sensor::Jc42(a) => jc42_celsius(bus.read_word_data(a, JC42_TEMP).ok()?),
    };
    plausible(t).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes() {
        // 41.25 °C = 165 quarter degrees, in bits 15:2.
        let raw = 165u16 << 2;
        assert_eq!(spd5118_celsius(raw as u8, (raw >> 8) as u8), 41.25);
        // Negative: -0.25 °C.
        let neg = (0x7FFu16) << 2;
        assert_eq!(spd5118_celsius(neg as u8, (neg >> 8) as u8), -0.25);
        // JC42: 0x0194 = 25.25 °C, sent high byte first.
        assert_eq!(jc42_celsius(0x9401), 25.25);
        assert_eq!(Sensor::Spd5118(0x52).slot(), 3);
    }
}
