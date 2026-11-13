//! Linux SMBus backend: `/dev/i2c-N` (i2c-dev) with the `I2C_SMBUS` ioctl.
//!
//! Adapters are found in `/sys/bus/i2c/devices` and kept only if their PCI
//! parent is a chipset SMBus controller ([`super::DRAM_SMBUS_PCI`]), like
//! OpenRGB's `i2c_smbus_linux_detect`.

use super::SmBus;
use crate::{Error, Result};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

const I2C_SLAVE: u64 = 0x0703;
const I2C_FUNCS: u64 = 0x0705;
const I2C_SMBUS: u64 = 0x0720;
const I2C_SMBUS_READ: u8 = 1;
const I2C_SMBUS_WRITE: u8 = 0;
const I2C_SMBUS_BYTE_DATA: u32 = 2;
const I2C_SMBUS_WORD_DATA: u32 = 3;
const I2C_SMBUS_BLOCK_DATA: u32 = 5;
const I2C_FUNC_SMBUS_WRITE_BLOCK_DATA: u64 = 0x0200_0000;

extern "C" {
    fn ioctl(fd: std::ffi::c_int, request: std::ffi::c_ulong, ...) -> std::ffi::c_int;
}

/// `union i2c_smbus_data`: byte, word, or block[0] = length + 32 bytes + PEC.
#[repr(C)]
struct SmbusData {
    block: [u8; super::BLOCK_MAX + 2],
}

/// `struct i2c_smbus_ioctl_data`.
#[repr(C)]
struct SmbusIoctl {
    read_write: u8,
    command: u8,
    size: u32,
    data: *mut SmbusData,
}

pub struct LinuxBus {
    file: File,
    number: u32,
    name: String,
    slave: Option<u8>,
    block_write: bool,
}

impl LinuxBus {
    pub fn open(number: u32, adapter_name: &str) -> std::io::Result<Self> {
        let path = format!("/dev/i2c-{number}");
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        let mut funcs: std::ffi::c_ulong = 0;
        // SAFETY: I2C_FUNCS writes one unsigned long to the pointer.
        let rc = unsafe { ioctl(file.as_raw_fd(), I2C_FUNCS as _, &mut funcs as *mut std::ffi::c_ulong) };
        let block_write = rc >= 0 && (funcs as u64 & I2C_FUNC_SMBUS_WRITE_BLOCK_DATA) != 0;
        Ok(Self { file, number, name: format!("i2c-{number} ({adapter_name})"), slave: None, block_write })
    }

    fn select(&mut self, addr: u8) -> Result<()> {
        if self.slave == Some(addr) {
            return Ok(());
        }
        // SAFETY: I2C_SLAVE takes the address as the integer argument.
        // (Not I2C_SLAVE_FORCE: addresses owned by a kernel driver fail with EBUSY.)
        let rc = unsafe { ioctl(self.file.as_raw_fd(), I2C_SLAVE as _, addr as std::ffi::c_ulong) };
        if rc < 0 {
            self.slave = None;
            return Err(os_err(&format!("select {addr:#04x}")));
        }
        self.slave = Some(addr);
        Ok(())
    }

    fn xfer(&mut self, addr: u8, read_write: u8, command: u8, size: u32, data: &mut SmbusData) -> Result<()> {
        self.select(addr)?;
        let mut args = SmbusIoctl { read_write, command, size, data };
        // SAFETY: args and data are valid for the duration of the call.
        let rc = unsafe { ioctl(self.file.as_raw_fd(), I2C_SMBUS as _, &mut args as *mut SmbusIoctl) };
        if rc < 0 {
            return Err(os_err(&format!("transfer {addr:#04x}/{command:#04x}")));
        }
        Ok(())
    }

    fn client_dir(&self, addr: u8) -> PathBuf {
        PathBuf::from(format!("/sys/bus/i2c/devices/{}-{:04x}", self.number, addr))
    }
}

fn os_err(what: &str) -> Error {
    Error::Bus(format!("{what}: {}", std::io::Error::last_os_error()))
}

impl SmBus for LinuxBus {
    fn name(&self) -> &str {
        &self.name
    }

    fn read_byte_data(&mut self, addr: u8, reg: u8) -> Result<u8> {
        let mut d = SmbusData { block: [0; 34] };
        self.xfer(addr, I2C_SMBUS_READ, reg, I2C_SMBUS_BYTE_DATA, &mut d)?;
        Ok(d.block[0])
    }

    fn read_word_data(&mut self, addr: u8, reg: u8) -> Result<u16> {
        let mut d = SmbusData { block: [0; 34] };
        self.xfer(addr, I2C_SMBUS_READ, reg, I2C_SMBUS_WORD_DATA, &mut d)?;
        Ok(u16::from_le_bytes([d.block[0], d.block[1]]))
    }

    fn write_byte_data(&mut self, addr: u8, reg: u8, value: u8) -> Result<()> {
        let mut d = SmbusData { block: [0; 34] };
        d.block[0] = value;
        self.xfer(addr, I2C_SMBUS_WRITE, reg, I2C_SMBUS_BYTE_DATA, &mut d)
    }

    fn write_word_data(&mut self, addr: u8, reg: u8, value: u16) -> Result<()> {
        let mut d = SmbusData { block: [0; 34] };
        d.block[..2].copy_from_slice(&value.to_le_bytes());
        self.xfer(addr, I2C_SMBUS_WRITE, reg, I2C_SMBUS_WORD_DATA, &mut d)
    }

    fn write_block_data(&mut self, addr: u8, reg: u8, data: &[u8]) -> Result<()> {
        if data.is_empty() || data.len() > super::BLOCK_MAX {
            return Err(Error::Bus(format!("invalid block length {}", data.len())));
        }
        let mut d = SmbusData { block: [0; 34] };
        d.block[0] = data.len() as u8;
        d.block[1..=data.len()].copy_from_slice(data);
        self.xfer(addr, I2C_SMBUS_WRITE, reg, I2C_SMBUS_BLOCK_DATA, &mut d)
    }

    fn supports_block_write(&self) -> bool {
        self.block_write
    }

    fn bound_driver(&self, addr: u8) -> Option<String> {
        let dir = self.client_dir(addr);
        if !dir.join("driver").exists() {
            return None;
        }
        let name = std::fs::read_to_string(dir.join("name")).ok()?;
        Some(name.trim().to_string())
    }

    fn os_temperature(&self, addr: u8) -> Option<f64> {
        let hwmon = self.client_dir(addr).join("hwmon");
        for e in std::fs::read_dir(hwmon).ok()?.flatten() {
            if let Ok(s) = std::fs::read_to_string(e.path().join("temp1_input")) {
                if let Ok(milli) = s.trim().parse::<f64>() {
                    return Some(milli / 1000.0);
                }
            }
        }
        None
    }
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn read_hex(p: &Path) -> Option<u16> {
    u16::from_str_radix(read_trim(p)?.trim_start_matches("0x"), 16).ok()
}

/// PCI vendor/device of the nearest PCI ancestor of a sysfs device.
fn pci_ids(sysfs: &Path) -> Option<(u16, u16)> {
    let mut p = std::fs::canonicalize(sysfs).ok()?;
    loop {
        if let (Some(v), Some(d)) = (read_hex(&p.join("vendor")), read_hex(&p.join("device"))) {
            return Some((v, d));
        }
        if !p.pop() || p.as_os_str().len() <= "/sys".len() {
            return None;
        }
    }
}

/// An i2c adapter that is a chipset SMBus controller.
#[derive(Debug, Clone)]
pub struct Adapter {
    pub number: u32,
    pub name: String,
    pub pci: (u16, u16),
}

pub fn dram_adapters() -> Vec<Adapter> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/sys/bus/i2c/devices") else { return out };
    for e in dir.flatten() {
        let fname = e.file_name().to_string_lossy().to_string();
        let Some(number) = fname.strip_prefix("i2c-").and_then(|n| n.parse::<u32>().ok()) else { continue };
        let name = read_trim(&e.path().join("name")).unwrap_or_default();
        if let Some(pci) = pci_ids(&e.path()) {
            if super::is_dram_smbus(pci.0, pci.1) {
                out.push(Adapter { number, name, pci });
            }
        }
    }
    out.sort_by_key(|a| a.number);
    out
}

pub fn open_dram_buses() -> Result<Vec<Box<dyn SmBus>>> {
    let adapters = dram_adapters();
    if adapters.is_empty() {
        return Err(Error::Bus(
            "no chipset SMBus adapter found. Check that i2c-i801 (Intel) or i2c-piix4 (AMD) is loaded; if dmesg shows \
             an ACPI resource conflict, the BIOS reserves the SMBus (see docs/ram.md about acpi_enforce_resources=lax)"
                .into(),
        ));
    }
    let mut buses: Vec<Box<dyn SmBus>> = Vec::new();
    let mut problems = Vec::new();
    for a in &adapters {
        match LinuxBus::open(a.number, &a.name) {
            Ok(b) => {
                log::info!("SMBus {}: {:04x}:{:04x}, block writes {}", b.name, a.pci.0, a.pci.1, b.block_write);
                buses.push(Box::new(b));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                problems.push(format!("/dev/i2c-{} missing: load the i2c-dev module (sudo modprobe i2c-dev)", a.number))
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => problems.push(format!(
                "/dev/i2c-{}: permission denied (install opendevicecontrol's udev rule, then log out and in)",
                a.number
            )),
            Err(e) => problems.push(format!("/dev/i2c-{}: {e}", a.number)),
        }
    }
    if buses.is_empty() {
        return Err(Error::Bus(problems.join("; ")));
    }
    for p in problems {
        log::warn!("{p}");
    }
    Ok(buses)
}
