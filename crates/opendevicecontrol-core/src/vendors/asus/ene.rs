//! The ENE RGB controller ASUS puts on its graphics cards (and memory and
//! motherboards), as ASUS Aura software drives it. Register layout as
//! documented by OpenRGB's `ENESMBusController`, confirmed on hardware.
//!
//! The controller sits at one I²C address and has a 16-bit register space
//! reached through an address pointer:
//!
//! | SMBus command | Meaning |
//! |---|---|
//! | write word `00` | set the pointer (register address, big-endian) |
//! | read byte `81` | read the register at the pointer |
//! | write byte `01` | write the register at the pointer |
//! | write block `03` | write consecutive registers from the pointer |
//!
//! | Register | Meaning |
//! |---|---|
//! | `1000` | device name, 16 bytes of ASCII (`AUMA0-E6K5-…`) |
//! | `1C00` | configuration table, 64 bytes |
//! | `8020` | 1: direct colours, 0: built-in effect |
//! | `8021` / `8022` / `8023` | effect mode / speed / direction |
//! | `80A0` | write `01` to apply an effect (`AA` would also save it to flash) |
//! | `8100` | direct colours, 3 bytes per LED |
//! | `8160` | effect colours, 3 bytes per LED |
//!
//! Colours are stored red, blue, green. Before anything else the
//! controller is identified by registers `A0`–`AF` reading back 0–15, read
//! directly (no pointer).

use crate::model::Rgb;
use crate::smbus::SmBus;
use crate::{Error, Result};

pub const REG_NAME: u16 = 0x1000;
pub const REG_CONFIG: u16 = 0x1C00;
pub const REG_DIRECT: u16 = 0x8020;
pub const REG_MODE: u16 = 0x8021;
pub const REG_SPEED: u16 = 0x8022;
pub const REG_DIRECTION: u16 = 0x8023;
pub const REG_APPLY: u16 = 0x80A0;
pub const REG_COLORS_DIRECT: u16 = 0x8100;
pub const REG_COLORS_EFFECT: u16 = 0x8160;
/// Written to [`REG_APPLY`]: use the effect registers now, without saving.
pub const APPLY: u8 = 0x01;

/// Byte of the configuration table that holds the LED count on the
/// AUMA0-E6K5 controllers ASUS fits to graphics cards.
pub const CONFIG_LED_COUNT: usize = 0x03;
/// Most LEDs the driver accepts from the configuration table.
pub const MAX_LEDS: usize = 64;

/// Speed register values: 0 is the fastest, 4 the slowest.
pub const SPEED_SLOWEST: u8 = 4;

/// Built-in effects: (mode register value, name, uses a colour, animates,
/// has a direction).
pub const MODES: &[(u8, &str, bool, bool, bool)] = &[
    (0x00, "Off", false, false, false),
    (0x01, "Static", true, false, false),
    (0x02, "Breathing", true, true, false),
    (0x03, "Flashing", true, true, false),
    (0x04, "Spectrum cycle", false, true, false),
    (0x05, "Rainbow", false, true, true),
    (0x06, "Spectrum cycle breathing", false, true, false),
    (0x07, "Chase fade", true, true, true),
    (0x08, "Spectrum cycle chase fade", false, true, true),
    (0x09, "Chase", true, true, true),
    (0x0a, "Spectrum cycle chase", false, true, true),
    (0x0b, "Spectrum cycle wave", false, true, true),
    (0x0c, "Chase rainbow pulse", false, true, true),
    (0x0d, "Random flicker", false, true, false),
];

pub fn mode(value: u8) -> Option<&'static (u8, &'static str, bool, bool, bool)> {
    MODES.iter().find(|m| m.0 == value)
}

/// Transfers on the GPU's I²C bus fail now and then; retry each a few times.
const TRIES: usize = 4;

fn retry<T>(mut f: impl FnMut() -> Result<T>) -> Result<T> {
    let mut last = None;
    for _ in 0..TRIES {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| Error::Bus("no attempt".into())))
}

/// True if an ENE controller answers at `addr`: registers `A0`–`AF` read
/// back 0–15. Reads only.
pub fn identify(bus: &mut dyn SmBus, addr: u8) -> bool {
    (0xA0u8..=0xAF).all(|reg| retry(|| bus.read_byte_data(addr, reg)).is_ok_and(|v| v == reg - 0xA0))
}

/// Register access to one controller.
pub struct Ene<'a> {
    pub bus: &'a mut dyn SmBus,
    pub addr: u8,
}

impl Ene<'_> {
    fn point(&mut self, reg: u16) -> Result<()> {
        // The register address goes high byte first.
        retry(|| self.bus.write_word_data(self.addr, 0x00, reg.swap_bytes()))
    }

    pub fn read(&mut self, reg: u16) -> Result<u8> {
        retry(|| {
            self.point(reg)?;
            self.bus.read_byte_data(self.addr, 0x81)
        })
    }

    pub fn read_n(&mut self, reg: u16, n: usize) -> Result<Vec<u8>> {
        (0..n).map(|i| self.read(reg + i as u16)).collect()
    }

    pub fn write(&mut self, reg: u16, value: u8) -> Result<()> {
        retry(|| {
            self.point(reg)?;
            self.bus.write_byte_data(self.addr, 0x01, value)
        })
    }

    /// Write consecutive registers, in blocks the bus can carry.
    pub fn write_n(&mut self, reg: u16, data: &[u8]) -> Result<()> {
        // Whole LEDs per block, so a failed block leaves no LED half set.
        const CHUNK: usize = 30;
        for (i, part) in data.chunks(CHUNK).enumerate() {
            let at = reg + (i * CHUNK) as u16;
            retry(|| {
                self.point(at)?;
                self.bus.write_block_data(self.addr, 0x03, part)
            })?;
        }
        Ok(())
    }

    pub fn name(&mut self) -> Result<String> {
        let raw = self.read_n(REG_NAME, 16)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        Ok(raw[..end].iter().filter(|b| b.is_ascii_graphic()).map(|&b| b as char).collect())
    }
}

/// Colours as the controller stores them (red, blue, green).
pub fn to_rbg(colors: &[Rgb]) -> Vec<u8> {
    colors.iter().flat_map(|c| [c.r, c.b, c.g]).collect()
}

pub fn from_rbg(raw: &[u8]) -> Vec<Rgb> {
    raw.as_chunks::<3>().0.iter().map(|c| Rgb::new(c[0], c[2], c[1])).collect()
}

/// Speed register value <-> percent (100 % fastest).
pub fn speed_percent(v: u8) -> f64 {
    (SPEED_SLOWEST - v.min(SPEED_SLOWEST)) as f64 / SPEED_SLOWEST as f64 * 100.0
}

pub fn speed_value(percent: f64) -> u8 {
    SPEED_SLOWEST - (percent.clamp(0.0, 100.0) / 100.0 * SPEED_SLOWEST as f64).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smbus::mock::{MockBus, Op};

    #[test]
    fn identifies_by_counting_registers() {
        let mut bus = MockBus { present: vec![0x67], ..Default::default() };
        assert!(!identify(&mut bus, 0x67));
        for r in 0xA0u8..=0xAF {
            bus.bytes.insert((0x67, r), r - 0xA0);
        }
        assert!(identify(&mut bus, 0x67));
        assert!(!identify(&mut bus, 0x66));
    }

    #[test]
    fn pointer_is_big_endian_and_colours_red_blue_green() {
        let bus = MockBus { present: vec![0x67], ..Default::default() };
        let log = bus.log.clone();
        let mut bus = bus;
        let mut e = Ene { bus: &mut bus, addr: 0x67 };
        e.write(REG_MODE, 5).unwrap();
        e.write_n(REG_COLORS_DIRECT, &to_rbg(&[Rgb::new(1, 2, 3)])).unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                Op::Word(0x67, 0x00, 0x2180),
                Op::Byte(0x67, 0x01, 5),
                Op::Word(0x67, 0x00, 0x0081),
                Op::Block(0x67, 0x03, vec![1, 3, 2]),
            ]
        );
        assert_eq!(from_rbg(&[1, 3, 2]), vec![Rgb::new(1, 2, 3)]);
    }

    #[test]
    fn speed_mapping() {
        assert_eq!(speed_percent(0), 100.0);
        assert_eq!(speed_percent(4), 0.0);
        assert_eq!(speed_value(50.0), 2);
        assert_eq!(speed_value(100.0), 0);
        assert_eq!(speed_value(0.0), 4);
    }
}
