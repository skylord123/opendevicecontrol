//! SMBus (I²C) access to the motherboard's chipset SMBus, used for RGB memory.
//!
//! The SMBus also carries the DIMMs' SPD EEPROMs, so every access goes
//! through [`Guarded`], which refuses writes to SPD space unconditionally
//! and to every other address until a driver has positively identified a
//! device there. See `docs/ram.md` for the full safety rules.
//!
//! Backends: Linux i2c-dev ([`linux`]), Windows PawnIO ([`pawnio`]).

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(windows)]
pub mod pawnio;

use crate::{Error, Result};

/// SMBus block transfers carry at most 32 bytes.
pub const BLOCK_MAX: usize = 32;

/// SMBus controllers that the RAM sits on, by PCI vendor/device ID (the list
/// of OpenRGB's `IF_DRAM_SMBUS`). Other buses (GPU, display DDC…) are never
/// touched.
pub const DRAM_SMBUS_PCI: &[(u16, u16)] = &[
    (0x1022, 0x790b), // AMD FCH (PIIX4)
    (0x8086, 0x3a30), // Intel ICH10
    (0x8086, 0xa123), // Intel Sunrise Point-H
    (0x8086, 0x2085), // Intel Skylake-X PCU
    (0x8086, 0xa2a3), // Intel Kaby Lake-H (200 series)
    (0x8086, 0xa323), // Intel Cannon Lake-H (300 series)
    (0x8086, 0x06a3), // Intel Comet Lake-H (400 series)
    (0x8086, 0xa3a3), // Intel Comet Lake-V
    (0x8086, 0x43a3), // Intel Tiger Lake-H (500 series)
    (0x8086, 0x7aa3), // Intel Alder Lake-S (600 series)
    (0x8086, 0x7a23), // Intel Raptor Lake-S (700 series)
    (0x8086, 0x7f23), // Intel Arrow Lake-S (800 series)
];

pub fn is_dram_smbus(vendor: u16, device: u16) -> bool {
    DRAM_SMBUS_PCI.contains(&(vendor, device))
}

/// Raw SMBus transfers on one bus. Addresses are 7-bit.
pub trait SmBus: Send {
    /// Human-readable bus name, e.g. `i2c-0 (SMBus I801 adapter at efa0)`.
    fn name(&self) -> &str;
    fn read_byte_data(&mut self, addr: u8, reg: u8) -> Result<u8>;
    /// SMBus word read: the first byte on the wire is the low byte.
    fn read_word_data(&mut self, addr: u8, reg: u8) -> Result<u16>;
    fn write_byte_data(&mut self, addr: u8, reg: u8, value: u8) -> Result<()>;
    fn write_word_data(&mut self, addr: u8, reg: u8, value: u16) -> Result<()>;
    /// SMBus block write (length byte + up to 32 data bytes).
    fn write_block_data(&mut self, addr: u8, reg: u8, data: &[u8]) -> Result<()>;
    /// Whether the controller can do SMBus block writes.
    fn supports_block_write(&self) -> bool {
        true
    }
    /// Name of an OS driver that owns `addr` (Linux: `ee1004`, `jc42`…),
    /// which makes the address unavailable to us.
    fn bound_driver(&self, _addr: u8) -> Option<String> {
        None
    }
    /// Temperature (°C) of the sensor at `addr` read through the OS driver
    /// that owns it (Linux hwmon).
    fn os_temperature(&self, _addr: u8) -> Option<f64> {
        None
    }
}

/// True for addresses opendevicecontrol never writes to, whatever a driver asks:
/// DDR4 SPD page/write-protect commands (0x30–0x37), SPD EEPROMs and DDR5
/// SPD hubs (0x50–0x57), and reserved addresses.
pub fn is_protected(addr: u8) -> bool {
    matches!(addr, 0x30..=0x37 | 0x50..=0x57) || !(0x08..=0x77).contains(&addr)
}

/// Safety wrapper around a bus: reads pass through, writes are refused
/// unless the address was explicitly allowed with [`Guarded::allow_writes`]
/// (after identification), and never for [`is_protected`] addresses.
pub struct Guarded {
    inner: Box<dyn SmBus>,
    writable: [bool; 128],
}

impl Guarded {
    pub fn new(inner: Box<dyn SmBus>) -> Self {
        Self { inner, writable: [false; 128] }
    }

    /// Permit writes to `addr`. Fails for protected addresses.
    pub fn allow_writes(&mut self, addr: u8) -> Result<()> {
        if is_protected(addr) {
            return Err(Error::Bus(format!("refusing to enable writes to protected address {addr:#04x}")));
        }
        self.writable[addr as usize] = true;
        Ok(())
    }

    pub fn revoke_writes(&mut self, addr: u8) {
        if let Some(w) = self.writable.get_mut(addr as usize) {
            *w = false;
        }
    }

    pub fn is_writable(&self, addr: u8) -> bool {
        !is_protected(addr) && self.writable.get(addr as usize).copied().unwrap_or(false)
    }

    fn check(&self, addr: u8) -> Result<()> {
        if self.is_writable(addr) {
            Ok(())
        } else {
            Err(Error::Bus(format!("write to unidentified/protected address {addr:#04x} refused")))
        }
    }
}

impl SmBus for Guarded {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn read_byte_data(&mut self, addr: u8, reg: u8) -> Result<u8> {
        self.inner.read_byte_data(addr, reg)
    }
    fn read_word_data(&mut self, addr: u8, reg: u8) -> Result<u16> {
        self.inner.read_word_data(addr, reg)
    }
    fn write_byte_data(&mut self, addr: u8, reg: u8, value: u8) -> Result<()> {
        self.check(addr)?;
        self.inner.write_byte_data(addr, reg, value)
    }
    fn write_word_data(&mut self, addr: u8, reg: u8, value: u16) -> Result<()> {
        self.check(addr)?;
        self.inner.write_word_data(addr, reg, value)
    }
    fn write_block_data(&mut self, addr: u8, reg: u8, data: &[u8]) -> Result<()> {
        self.check(addr)?;
        if data.is_empty() || data.len() > BLOCK_MAX {
            return Err(Error::Bus(format!("invalid block length {}", data.len())));
        }
        self.inner.write_block_data(addr, reg, data)
    }
    fn supports_block_write(&self) -> bool {
        self.inner.supports_block_write()
    }
    fn bound_driver(&self, addr: u8) -> Option<String> {
        self.inner.bound_driver(addr)
    }
    fn os_temperature(&self, addr: u8) -> Option<f64> {
        self.inner.os_temperature(addr)
    }
}

/// Open every chipset SMBus that may carry DIMMs. Errors explain what the
/// user has to do (load i2c-dev, install PawnIO, run elevated…).
pub fn open_dram_buses() -> Result<Vec<Box<dyn SmBus>>> {
    #[cfg(target_os = "linux")]
    {
        linux::open_dram_buses()
    }
    #[cfg(windows)]
    {
        pawnio::open_dram_buses()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Err(Error::Bus("SMBus access is not supported on this operating system".into()))
    }
}

/// An in-memory SMBus for tests.
#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Op {
        Byte(u8, u8, u8),
        Word(u8, u8, u16),
        Block(u8, u8, Vec<u8>),
    }

    /// Present devices answer reads from `bytes` / `words` (0 when unset);
    /// absent addresses fail like a NACK. Writes are recorded in `log`.
    /// `info` emulates the Corsair DRAM binary buffer: writing 0x21 rewinds
    /// it, reading 0x40 returns its next byte, 0x42 its CRC-8.
    #[derive(Default)]
    pub struct MockBus {
        pub present: Vec<u8>,
        pub bytes: HashMap<(u8, u8), u8>,
        pub words: HashMap<(u8, u8), u16>,
        pub info: HashMap<u8, Vec<u8>>,
        pub info_pos: HashMap<u8, usize>,
        pub bound: HashMap<u8, String>,
        pub os_temps: HashMap<u8, f64>,
        pub no_block: bool,
        pub log: Arc<Mutex<Vec<Op>>>,
        pub reads: Arc<Mutex<Vec<(u8, u8)>>>,
    }

    impl MockBus {
        fn present(&self, addr: u8) -> Result<()> {
            if self.present.contains(&addr) && !self.bound.contains_key(&addr) {
                Ok(())
            } else {
                Err(Error::Bus(format!("no ACK from {addr:#04x}")))
            }
        }
    }

    impl SmBus for MockBus {
        fn name(&self) -> &str {
            "mock"
        }
        fn read_byte_data(&mut self, addr: u8, reg: u8) -> Result<u8> {
            self.present(addr)?;
            self.reads.lock().unwrap().push((addr, reg));
            if let Some(buf) = self.info.get(&addr) {
                if reg == 0x40 {
                    let p = self.info_pos.entry(addr).or_default();
                    let v = buf.get(*p).copied().unwrap_or(0);
                    *p += 1;
                    return Ok(v);
                }
                if reg == 0x42 && !self.bytes.contains_key(&(addr, reg)) {
                    return Ok(crate::devices::ram::crc8(buf));
                }
            }
            Ok(self.bytes.get(&(addr, reg)).copied().unwrap_or(0))
        }
        fn read_word_data(&mut self, addr: u8, reg: u8) -> Result<u16> {
            self.present(addr)?;
            self.reads.lock().unwrap().push((addr, reg));
            Ok(self.words.get(&(addr, reg)).copied().unwrap_or(0))
        }
        fn write_byte_data(&mut self, addr: u8, reg: u8, value: u8) -> Result<()> {
            self.present(addr)?;
            if reg == 0x21 {
                self.info_pos.insert(addr, 0);
            }
            self.log.lock().unwrap().push(Op::Byte(addr, reg, value));
            Ok(())
        }
        fn write_word_data(&mut self, addr: u8, reg: u8, value: u16) -> Result<()> {
            self.present(addr)?;
            self.log.lock().unwrap().push(Op::Word(addr, reg, value));
            Ok(())
        }
        fn write_block_data(&mut self, addr: u8, reg: u8, data: &[u8]) -> Result<()> {
            self.present(addr)?;
            if self.no_block {
                return Err(Error::Bus("block writes not supported".into()));
            }
            self.log.lock().unwrap().push(Op::Block(addr, reg, data.to_vec()));
            Ok(())
        }
        fn supports_block_write(&self) -> bool {
            !self.no_block
        }
        fn bound_driver(&self, addr: u8) -> Option<String> {
            self.bound.get(&addr).cloned()
        }
        fn os_temperature(&self, addr: u8) -> Option<f64> {
            self.os_temps.get(&addr).copied()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::*;
    use super::*;

    #[test]
    fn protected_ranges() {
        for a in 0x50..=0x57 {
            assert!(is_protected(a));
        }
        for a in 0x30..=0x37 {
            assert!(is_protected(a));
        }
        assert!(!is_protected(0x58));
        assert!(!is_protected(0x18));
        assert!(is_protected(0x78));
    }

    #[test]
    fn guarded_refuses_unidentified_and_spd_writes() {
        let bus = MockBus { present: vec![0x50, 0x58], ..Default::default() };
        let log = bus.log.clone();
        let mut g = Guarded::new(Box::new(bus));
        assert!(g.write_byte_data(0x58, 0x31, 1).is_err());
        assert!(g.allow_writes(0x50).is_err());
        assert!(g.write_byte_data(0x50, 0x00, 1).is_err());
        assert!(g.allow_writes(0x36).is_err());
        g.allow_writes(0x58).unwrap();
        g.write_byte_data(0x58, 0x31, 1).unwrap();
        assert!(g.write_block_data(0x58, 0x31, &[0; 33]).is_err());
        g.revoke_writes(0x58);
        assert!(g.write_word_data(0x58, 0x90, 1).is_err());
        assert_eq!(*log.lock().unwrap(), vec![Op::Byte(0x58, 0x31, 1)]);
        // Reads always pass through.
        assert!(g.read_byte_data(0x50, 0).is_ok());
    }
}
