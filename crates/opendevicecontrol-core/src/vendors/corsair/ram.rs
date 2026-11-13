//! Corsair RGB memory (Vengeance RGB / RGB PRO / PRO SL / RS / RGB DDR5,
//! Dominator Platinum and Titanium RGB) on the motherboard SMBus, plus the
//! DIMM temperature sensors. **Experimental, untested on hardware.**
//!
//! Protocol and detection follow OpenRGB's `CorsairDRAMController` and
//! `CorsairVengeanceController` (GPL-2.0-or-later, Adam Honse, Erik Gilling)
//! and OpenLinkHub's memory driver (GPL-3.0-or-later, Nikola Jurkovic);
//! temperature formats follow the Linux `jc42` and `spd5118` drivers.
//! `docs/ram.md` explains the safety rules and every deviation.
//!
//! All DIMMs of one SMBus form one [`CorsairMemory`] device with a light
//! zone and a temperature sensor per DIMM.

use super::Device;
use crate::model::*;
use crate::smbus::{Guarded, SmBus};
use crate::{Error, Result, CORSAIR_VID};
use std::ffi::CString;
use std::time::Duration;

// Corsair DRAM controller registers (OpenRGB CorsairDRAMController.h).
const REG_BINARY_START: u8 = 0x21;
const REG_COLOR_BLOCK_1: u8 = 0x31;
const REG_COLOR_BLOCK_2: u8 = 0x32;
const REG_GET_BINARY_DATA: u8 = 0x40;
const REG_GET_CHECKSUM: u8 = 0x42;
const REG_ID_1: u8 = 0x43;
const REG_ID_2: u8 = 0x44;
const REG_GET_DEVICE_INFO: u8 = 0x61;

// Original Vengeance RGB registers (OpenRGB CorsairVengeanceController.h).
const VENGEANCE_FADE_TIME: u8 = 0xA4;
const VENGEANCE_MODE: u8 = 0xA6;
const VENGEANCE_RED: u8 = 0xB0;
const VENGEANCE_GREEN: u8 = 0xB1;
const VENGEANCE_BLUE: u8 = 0xB2;
const VENGEANCE_MODE_SINGLE: u8 = 0x00;

/// Delay between probed addresses during detection (as OpenRGB).
const PROBE_DELAY: Duration = Duration::from_millis(10);

/// A model of the Corsair DRAM controller family, by PID.
#[derive(Debug)]
pub struct DramModel {
    pub name: &'static str,
    pub pids: &'static [u16],
    pub leds: usize,
    /// The module takes its LEDs in reverse order.
    pub reverse: bool,
}

/// OpenRGB `CorsairDRAMDevices.cpp`.
pub const DRAM_MODELS: &[DramModel] = &[
    DramModel { name: "Vengeance RGB PRO DDR4", pids: &[0x0100, 0x0101], leds: 10, reverse: false },
    DramModel { name: "Dominator Platinum RGB DDR4", pids: &[0x0200, 0x0201], leds: 12, reverse: true },
    DramModel { name: "Vengeance RGB PRO SL DDR4", pids: &[0x0300, 0x0301], leds: 10, reverse: false },
    DramModel { name: "Vengeance RGB RS DDR4", pids: &[0x0400, 0x0401], leds: 6, reverse: false },
    DramModel { name: "Dominator Platinum RGB DDR5", pids: &[0x0600, 0x0601], leds: 12, reverse: true },
    DramModel { name: "Dominator Titanium RGB DDR5", pids: &[0x0800, 0x0801, 0x0810, 0x0811], leds: 12, reverse: true },
    DramModel {
        name: "Vengeance RGB DDR5",
        pids: &[0x0700, 0x0701, 0x0900, 0x0901, 0x0910, 0x0911],
        leds: 10,
        reverse: false,
    },
    DramModel { name: "Vengeance Shugo Series DDR5", pids: &[0x0A00, 0x0A01, 0x0A10, 0x0A11], leds: 10, reverse: false },
    DramModel { name: "Vengeance RGB RS DDR5", pids: &[0x0B00, 0x0B01], leds: 6, reverse: false },
];

pub fn model_for_pid(pid: u16) -> Option<&'static DramModel> {
    DRAM_MODELS.iter().find(|m| m.pids.contains(&pid))
}

/// CRC-8, polynomial 0x07, initial value 0 (CRCpp `CRC_8`).
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

/// Direct-mode colour packet: `[led count, R, G, B, …, crc8]`.
pub fn direct_packet(colors: &[Rgb], reverse: bool) -> Vec<u8> {
    let mut p = Vec::with_capacity(colors.len() * 3 + 2);
    p.push(colors.len() as u8);
    let mut push = |c: &Rgb| p.extend_from_slice(&[c.r, c.g, c.b]);
    if reverse {
        colors.iter().rev().for_each(&mut push);
    } else {
        colors.iter().for_each(&mut push);
    }
    p.push(crc8(&p));
    p
}

/// The packet as SMBus block writes: first 32 bytes to 0x31, the rest to 0x32.
pub fn block_writes(packet: &[u8]) -> Vec<(u8, &[u8])> {
    let first = packet.len().min(crate::smbus::BLOCK_MAX);
    let mut v = vec![(REG_COLOR_BLOCK_1, &packet[..first])];
    if packet.len() > first {
        v.push((REG_COLOR_BLOCK_2, &packet[first..]));
    }
    v
}

/// The packet as word writes, for controllers without SMBus block writes
/// (OpenRGB `SetColorsPerLED` fallback): every 5 bytes become two words,
/// the middle byte's nibbles go into the low nibble of the register numbers.
pub fn word_writes(packet: &[u8]) -> Vec<(u8, u16)> {
    let n = packet.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let mut reg0: u8 = if i == 0 { 0x90 } else { 0xA0 };
        let mut reg1: u8 = 0;
        let mut w0 = packet[i] as u16;
        let mut w1 = 0u16;
        i += 1;
        if i < n {
            w0 |= (packet[i] as u16) << 8;
            i += 1;
        }
        if i < n {
            reg1 = 0xA0 | ((packet[i] & 0xF0) >> 4);
            reg0 |= packet[i] & 0x0F;
            i += 1;
        }
        if i < n {
            w1 = packet[i] as u16;
            i += 1;
        }
        if i < n {
            w1 |= (packet[i] as u16) << 8;
            i += 1;
        }
        out.push((reg0, w0));
        if reg1 > 0 {
            out.push((reg1, w1));
        }
    }
    out
}

/// Parsed 32-byte device information buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DramInfo {
    pub vid: u16,
    pub pid: u16,
    pub firmware: String,
    pub protocol: u8,
}

pub fn parse_info(b: &[u8; 32]) -> DramInfo {
    DramInfo {
        vid: u16::from_le_bytes([b[0], b[1]]),
        pid: u16::from_le_bytes([b[2], b[3]]),
        firmware: format!("{}.{}.{}", b[9], b[8], u16::from_le_bytes([b[10], b[11]])),
        protocol: b[28],
    }
}

/// DDR5 hub temperature, MR49/MR50 (kernel `spd5118_temp_from_reg`).
pub fn ddr5_temp(raw: u16) -> f64 {
    let v = ((raw >> 2) & 0x7ff) as i16;
    let v = (v << 5) >> 5; // sign-extend 11 bits
    v as f64 * 0.25
}

/// DDR4 TSE2004av temperature register (already byte-swapped to MSB-first
/// order), kernel `jc42`.
pub fn ddr4_temp(raw: u16) -> f64 {
    let v = (raw & 0x1fff) as i16;
    let v = (v << 3) >> 3; // sign-extend 13 bits
    v as f64 * 0.0625
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemType {
    Ddr4,
    Ddr5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TempSource {
    /// DDR5 SPD hub address.
    Hub(u8),
    /// DDR4 thermal sensor address.
    Ts(u8),
    /// Read through the OS driver owning this address (Linux hwmon).
    Os(u8),
}

#[derive(Debug, Clone)]
enum Controller {
    /// Corsair DRAM controller; `direct` = protocol ≥ 4 (direct mode allowed).
    Dram { model: &'static DramModel, info: DramInfo, direct: bool },
    /// Original single-zone Vengeance RGB.
    Vengeance,
}

#[derive(Debug, Clone)]
struct Dimm {
    slot: u8,
    addr: u8,
    controller: Controller,
    temp: Option<TempSource>,
    /// Last colours sent, to skip unchanged frames.
    last: Option<Vec<Rgb>>,
}

impl Dimm {
    fn model_name(&self) -> &'static str {
        match &self.controller {
            Controller::Dram { model, .. } => model.name,
            Controller::Vengeance => "Vengeance RGB DDR4",
        }
    }

    /// LEDs opendevicecontrol drives (0 = no lighting, e.g. protocol < 4).
    fn leds(&self) -> usize {
        match &self.controller {
            Controller::Dram { model, direct: true, .. } => model.leds,
            Controller::Dram { .. } => 0,
            Controller::Vengeance => 1,
        }
    }
}

/// Memory type of slot `n` from its SPD (read-only), `None` if empty.
fn slot_type(bus: &mut Guarded, n: u8) -> Option<MemType> {
    let spd = 0x50 + n;
    if let Some(driver) = bus.bound_driver(spd) {
        return Some(if driver.starts_with("spd5118") { MemType::Ddr5 } else { MemType::Ddr4 });
    }
    let b0 = bus.read_byte_data(spd, 0x00).ok()?;
    let b1 = bus.read_byte_data(spd, 0x01).ok()?;
    Some(if b0 == 0x51 && (b1 & 0xef) == 0x08 { MemType::Ddr5 } else { MemType::Ddr4 })
}

/// Identify a Corsair DRAM controller at `addr`. Writes are only enabled
/// after the ID registers match and are revoked again if the device info
/// does not check out.
fn identify_dram(bus: &mut Guarded, addr: u8) -> Option<Controller> {
    let id1 = bus.read_byte_data(addr, REG_ID_1).ok()?;
    if !matches!(id1, 0x1a..=0x1c) {
        return None;
    }
    let id2 = bus.read_byte_data(addr, REG_ID_2).ok()?;
    if !matches!(id2, 0x01 | 0x03 | 0x04) {
        return None;
    }
    bus.allow_writes(addr).ok()?;
    let info = read_info(bus, addr);
    match info {
        Ok(info) if info.vid == CORSAIR_VID => match model_for_pid(info.pid) {
            Some(model) => {
                let direct = info.protocol >= 4;
                if !direct {
                    log::warn!(
                        "{addr:#04x}: {} protocol {} has no direct mode; lighting disabled, temperature only",
                        model.name,
                        info.protocol
                    );
                }
                Some(Controller::Dram { model, info, direct })
            }
            None => {
                log::warn!("{addr:#04x}: Corsair DRAM controller with unknown PID {:#06x}; ignored", info.pid);
                bus.revoke_writes(addr);
                None
            }
        },
        Ok(info) => {
            log::warn!("{addr:#04x}: device info has VID {:#06x}, not Corsair; ignored", info.vid);
            bus.revoke_writes(addr);
            None
        }
        Err(e) => {
            log::warn!("{addr:#04x}: {e}; ignored");
            bus.revoke_writes(addr);
            None
        }
    }
}

fn read_info(bus: &mut Guarded, addr: u8) -> Result<DramInfo> {
    bus.write_byte_data(addr, REG_GET_DEVICE_INFO, 0x00)?;
    bus.write_byte_data(addr, REG_BINARY_START, 0x00)?;
    let mut b = [0u8; 32];
    for x in b.iter_mut() {
        *x = bus.read_byte_data(addr, REG_GET_BINARY_DATA)?;
    }
    let crc = bus.read_byte_data(addr, REG_GET_CHECKSUM)?;
    if crc != crc8(&b) {
        return Err(Error::Bus(format!("device info CRC mismatch ({crc:#04x} != {:#04x})", crc8(&b))));
    }
    Ok(parse_info(&b))
}

/// Original Vengeance RGB: registers 0xA0..=0xAF all read 0xBA (read-only).
fn identify_vengeance(bus: &mut Guarded, addr: u8) -> bool {
    (0xa0..=0xaf).all(|r| bus.read_byte_data(addr, r).ok() == Some(0xba))
}

/// Find the temperature sensor of slot `n` (read-only).
fn find_temp(bus: &mut Guarded, n: u8, mem: MemType) -> Option<TempSource> {
    let addr = match mem {
        MemType::Ddr5 => 0x50 + n,
        MemType::Ddr4 => 0x18 + n,
    };
    if bus.bound_driver(addr).is_some() {
        return bus.os_temperature(addr).map(|_| TempSource::Os(addr));
    }
    match mem {
        MemType::Ddr5 => {
            // MR26 bit 0 set = sensor disabled; we leave it that way.
            let cfg = bus.read_byte_data(addr, 0x1a).ok()?;
            (cfg & 0x01 == 0).then_some(TempSource::Hub(addr))
        }
        MemType::Ddr4 => {
            let cap = bus.read_word_data(addr, 0x00).ok()?.swap_bytes();
            let cfg = bus.read_word_data(addr, 0x01).ok()?.swap_bytes();
            (cap & 0xff00 == 0 && cfg & 0xf800 == 0).then_some(TempSource::Ts(addr))
        }
    }
}

fn read_temp(bus: &mut Guarded, src: TempSource) -> Option<f64> {
    let t = match src {
        TempSource::Hub(a) => {
            let lo = bus.read_byte_data(a, 0x31).ok()?;
            let hi = bus.read_byte_data(a, 0x32).ok()?;
            ddr5_temp(u16::from_le_bytes([lo, hi]))
        }
        TempSource::Ts(a) => ddr4_temp(bus.read_word_data(a, 0x05).ok()?.swap_bytes()),
        TempSource::Os(a) => bus.os_temperature(a)?,
    };
    (-40.0..=150.0).contains(&t).then_some(t)
}

/// Scan one bus for Corsair RGB memory. Returns `None` if there is none.
fn scan(bus: Box<dyn SmBus>, id: String, delay: Duration) -> Option<CorsairMemory> {
    let mut bus = Guarded::new(bus);
    let mut dimms = Vec::new();
    for n in 0..8u8 {
        let Some(mem) = slot_type(&mut bus, n) else { continue };
        // DDR4 controllers at 0x58+n, DDR5 controllers at 0x18+n (where DDR4
        // has its thermal sensor, so that range is only probed on DDR5).
        let addr = match mem {
            MemType::Ddr4 => 0x58 + n,
            MemType::Ddr5 => 0x18 + n,
        };
        if let Some(d) = bus.bound_driver(addr) {
            log::info!("{}: {addr:#04x} is owned by the kernel driver {d}; skipped", bus.name());
            continue;
        }
        std::thread::sleep(delay);
        let controller = match identify_dram(&mut bus, addr) {
            Some(c) => Some(c),
            None if mem == MemType::Ddr4 && identify_vengeance(&mut bus, addr) => {
                bus.allow_writes(addr).ok().map(|_| Controller::Vengeance)
            }
            None => None,
        };
        let Some(controller) = controller else { continue };
        let temp = find_temp(&mut bus, n, mem);
        let dimm = Dimm { slot: n, addr, controller, temp, last: None };
        log::info!(
            "{}: DIMM {} at {addr:#04x}: Corsair {}{}, {} LEDs, temperature {}",
            bus.name(),
            n + 1,
            dimm.model_name(),
            match &dimm.controller {
                Controller::Dram { info, .. } => format!(" (PID {:#06x}, firmware {}, protocol {})", info.pid, info.firmware, info.protocol),
                Controller::Vengeance => String::new(),
            },
            dimm.leds(),
            if dimm.temp.is_some() { "yes" } else { "no" }
        );
        dimms.push(dimm);
    }
    if dimms.is_empty() {
        return None;
    }
    Some(CorsairMemory::new(bus, dimms, id))
}

/// One SMBus worth of Corsair RGB DIMMs.
pub struct CorsairMemory {
    bus: Guarded,
    dimms: Vec<Dimm>,
    info: DeviceInfo,
}

impl CorsairMemory {
    fn new(bus: Guarded, dimms: Vec<Dimm>, id: String) -> Self {
        let first = &dimms[0];
        let model = if dimms.iter().all(|d| d.model_name() == first.model_name()) {
            first.model_name().to_string()
        } else {
            "RGB memory".to_string()
        };
        let (pid, firmware) = match &first.controller {
            Controller::Dram { info, .. } => (info.pid, Some(info.firmware.clone())),
            Controller::Vengeance => (0, None),
        };
        let info = DeviceInfo {
            id,
            kind: DeviceKind::Memory,
            manufacturer: "Corsair".into(),
            model,
            vendor_string: None,
            product_string: Some(format!("{} DIMM(s) on {}", dimms.len(), bus.name())),
            usb_vid: CORSAIR_VID,
            usb_pid: pid,
            serial: None,
            firmware,
        };
        Self { bus, dimms, info }
    }

    fn write_dimm(bus: &mut Guarded, d: &Dimm, colors: &[Rgb]) -> Result<()> {
        match &d.controller {
            Controller::Dram { model, .. } => {
                let packet = direct_packet(colors, model.reverse);
                let blocks = block_writes(&packet);
                let first = if bus.supports_block_write() {
                    bus.write_block_data(d.addr, blocks[0].0, blocks[0].1)
                } else {
                    Err(Error::Bus("no block writes".into()))
                };
                match first {
                    Ok(()) => {
                        for (reg, data) in &blocks[1..] {
                            bus.write_block_data(d.addr, *reg, data)?;
                        }
                    }
                    // As OpenRGB: fall back to word writes if block writes fail.
                    Err(_) => {
                        for (reg, w) in word_writes(&packet) {
                            bus.write_word_data(d.addr, reg, w)?;
                        }
                    }
                }
                Ok(())
            }
            Controller::Vengeance => {
                let c = colors.first().copied().unwrap_or_default();
                bus.write_byte_data(d.addr, VENGEANCE_FADE_TIME, 0x00)?;
                bus.write_byte_data(d.addr, VENGEANCE_RED, c.r)?;
                bus.write_byte_data(d.addr, VENGEANCE_GREEN, c.g)?;
                bus.write_byte_data(d.addr, VENGEANCE_BLUE, c.b)?;
                bus.write_byte_data(d.addr, VENGEANCE_MODE, VENGEANCE_MODE_SINGLE)
            }
        }
    }
}

impl Device for CorsairMemory {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        let mut sensors = Vec::new();
        for d in &self.dimms {
            if let Some(src) = d.temp {
                if let Some(t) = read_temp(&mut self.bus, src) {
                    let n = d.slot + 1;
                    sensors.push(Sensor::new(
                        &format!("dimm{n}_temperature"),
                        &format!("DIMM {n} temperature"),
                        SensorKind::Temperature,
                        t,
                    ));
                }
            }
        }
        Ok(Snapshot { sensors, controls: Vec::new() })
    }

    fn set_control(&mut self, id: &str, _value: &ControlValue) -> Result<()> {
        Err(Error::UnknownControl(id.to_string()))
    }

    fn light_zones(&self) -> Vec<LightZone> {
        self.dimms
            .iter()
            .filter(|d| d.leds() > 0)
            .map(|d| LightZone {
                id: format!("dimm{}", d.slot + 1),
                name: format!("DIMM {} ({})", d.slot + 1, d.model_name()),
                leds: d.leds(),
            })
            .collect()
    }

    fn set_lights(&mut self, colors: &[Rgb]) -> Result<()> {
        let mut off = 0;
        for d in self.dimms.iter_mut().filter(|d| d.leds() > 0) {
            let n = d.leds();
            let Some(c) = colors.get(off..off + n) else { break };
            off += n;
            if d.last.as_deref() == Some(c) {
                continue;
            }
            d.last = None;
            Self::write_dimm(&mut self.bus, d, c)?;
            d.last = Some(c.to_vec());
        }
        Ok(())
    }

    // release(): nothing. The controllers keep the last frame until power
    // off and then run the effect stored in them; we never write that
    // stored (persistent) effect configuration. See docs/ram.md §3.6.
}

/// Result of a RAM scan.
pub struct Scan {
    pub devices: Vec<(CString, Box<dyn Device>)>,
    /// Names of the SMBus controllers that were scanned.
    pub buses: Vec<String>,
}

/// Find Corsair RGB memory on the chipset SMBus. Only called when the user
/// enabled RAM support. Buses whose path (`smbus:<name>`) is in
/// `skip_paths` are already in use and are not probed again.
pub fn discover(skip_paths: &[CString]) -> Result<Scan> {
    let buses = crate::smbus::open_dram_buses()?;
    let mut scan = Scan { devices: Vec::new(), buses: Vec::new() };
    let mut n = 0;
    for bus in buses {
        let name = bus.name().to_string();
        scan.buses.push(name.clone());
        let path = CString::new(format!("smbus:{name}")).unwrap_or_default();
        n += 1;
        let id = if n == 1 { "memory".to_string() } else { format!("memory-{n}") };
        if skip_paths.contains(&path) {
            continue;
        }
        if let Some(dev) = scan_bus(bus, id) {
            log::info!("opened Corsair {} ({}) on {name}", dev.info.model, dev.info.id);
            scan.devices.push((path, Box::new(dev)));
        }
    }
    Ok(scan)
}

fn scan_bus(bus: Box<dyn SmBus>, id: String) -> Option<CorsairMemory> {
    scan(bus, id, PROBE_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smbus::mock::{MockBus, Op};

    fn info_buf(pid: u16, protocol: u8) -> Vec<u8> {
        let mut b = vec![0u8; 32];
        b[..2].copy_from_slice(&CORSAIR_VID.to_le_bytes());
        b[2..4].copy_from_slice(&pid.to_le_bytes());
        b[8] = 2; // minor
        b[9] = 1; // major
        b[10..12].copy_from_slice(&300u16.to_le_bytes());
        b[28] = protocol;
        b
    }

    /// A DDR4 system with a DRAM controller in slot `n` (SPD at 0x50+n,
    /// controller at 0x58+n, thermal sensor at 0x18+n).
    fn ddr4_dimm(bus: &mut MockBus, n: u8, pid: u16) {
        bus.present.extend([0x50 + n, 0x58 + n, 0x18 + n]);
        bus.bytes.insert((0x50 + n, 0x00), 0x23);
        bus.bytes.insert((0x50 + n, 0x01), 0x11);
        bus.bytes.insert((0x58 + n, 0x43), 0x1c);
        bus.bytes.insert((0x58 + n, 0x44), 0x03);
        bus.info.insert(0x58 + n, info_buf(pid, 4));
        bus.words.insert((0x18 + n, 0x05), 0x4001); // wire order 01 40 -> 0x0140 = 20 °C
    }

    fn scan_mock(bus: MockBus) -> Option<CorsairMemory> {
        scan(Box::new(bus), "memory".into(), Duration::ZERO)
    }

    #[test]
    fn crc8_matches_crcpp() {
        // CRC-8/SMBUS check value.
        assert_eq!(crc8(b"123456789"), 0xf4);
        assert_eq!(crc8(&[]), 0);
    }

    #[test]
    fn packet_layout_and_reverse() {
        let c = [Rgb::new(1, 2, 3), Rgb::new(4, 5, 6)];
        let p = direct_packet(&c, false);
        assert_eq!(&p[..7], &[2, 1, 2, 3, 4, 5, 6]);
        assert_eq!(p[7], crc8(&p[..7]));
        let r = direct_packet(&c, true);
        assert_eq!(&r[..7], &[2, 4, 5, 6, 1, 2, 3]);
    }

    #[test]
    fn block_split() {
        let ten = direct_packet(&[Rgb::WHITE; 10], false);
        assert_eq!(ten.len(), 32);
        let b = block_writes(&ten);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].0, 0x31);

        let twelve = direct_packet(&[Rgb::WHITE; 12], false);
        assert_eq!(twelve.len(), 38);
        let b = block_writes(&twelve);
        assert_eq!((b[0].0, b[0].1.len()), (0x31, 32));
        assert_eq!((b[1].0, b[1].1.len()), (0x32, 6));

        // 6-LED modules: exactly 20 bytes (OpenRGB sends 32).
        let six = direct_packet(&[Rgb::WHITE; 6], false);
        assert_eq!(block_writes(&six)[0].1.len(), 20);
    }

    #[test]
    fn word_fallback_encoding() {
        // 8 bytes = a chunk of 5 and a chunk of 3. The third byte of each
        // chunk is split into the low nibbles of the two register numbers;
        // only the very first chunk uses 0x90.
        let w = word_writes(&[0x0a, 0x11, 0x3c, 0x22, 0x44, 0x55, 0x66, 0x7f]);
        assert_eq!(w, vec![(0x9c, 0x110a), (0xa3, 0x4422), (0xaf, 0x6655), (0xa7, 0x0000)]);
        // A trailing chunk of 1–2 bytes sends a single word.
        assert_eq!(word_writes(&[1, 2, 3, 4, 5, 6]), vec![(0x93, 0x0201), (0xa0, 0x0504), (0xa0, 0x0006)]);
    }

    #[test]
    fn temperature_formats() {
        // DDR5: 0.25 °C per LSB in bits 12:2.
        assert_eq!(ddr5_temp((40 * 4) << 2), 40.0);
        assert_eq!(ddr5_temp(0x1ffc), -0.25);
        // DDR4: 0.0625 °C per LSB, 13-bit two's complement.
        assert_eq!(ddr4_temp(0x0140), 20.0);
        assert_eq!(ddr4_temp(0x1ff0), -1.0);
        // Alarm flags in bits 15:13 are ignored.
        assert_eq!(ddr4_temp(0xe140), 20.0);
    }

    #[test]
    fn info_parsing() {
        let b: [u8; 32] = info_buf(0x0201, 4).try_into().unwrap();
        let i = parse_info(&b);
        assert_eq!(i, DramInfo { vid: 0x1b1c, pid: 0x0201, firmware: "1.2.300".into(), protocol: 4 });
        assert_eq!(model_for_pid(0x0201).unwrap().name, "Dominator Platinum RGB DDR4");
        assert!(model_for_pid(0x0500).is_none());
    }

    #[test]
    fn detects_ddr4_kit_and_writes_only_to_controllers() {
        let mut bus = MockBus::default();
        ddr4_dimm(&mut bus, 0, 0x0100);
        ddr4_dimm(&mut bus, 2, 0x0201);
        let log = bus.log.clone();
        let mut dev = scan_mock(bus).expect("found");
        assert_eq!(dev.info().kind, DeviceKind::Memory);
        assert_eq!(dev.info().model, "RGB memory");
        let zones = dev.light_zones();
        assert_eq!(zones.iter().map(|z| (z.id.as_str(), z.leds)).collect::<Vec<_>>(), vec![("dimm1", 10), ("dimm3", 12)]);

        // Detection wrote only the two device-info requests per controller.
        let writes = log.lock().unwrap().clone();
        assert_eq!(
            writes,
            vec![Op::Byte(0x58, 0x61, 0), Op::Byte(0x58, 0x21, 0), Op::Byte(0x5a, 0x61, 0), Op::Byte(0x5a, 0x21, 0)]
        );
        log.lock().unwrap().clear();

        let mut colors = vec![Rgb::new(255, 0, 0); 10];
        colors.extend(vec![Rgb::new(0, 0, 255); 12]);
        dev.set_lights(&colors).unwrap();
        let writes = log.lock().unwrap().clone();
        assert_eq!(writes.len(), 3);
        match &writes[0] {
            Op::Block(0x58, 0x31, d) => assert_eq!(d, &direct_packet(&colors[..10], false)),
            o => panic!("{o:?}"),
        }
        let dom = direct_packet(&colors[10..], true);
        assert_eq!(writes[1], Op::Block(0x5a, 0x31, dom[..32].to_vec()));
        assert_eq!(writes[2], Op::Block(0x5a, 0x32, dom[32..].to_vec()));
        for w in &writes {
            let a = match w {
                Op::Byte(a, ..) | Op::Word(a, ..) | Op::Block(a, ..) => *a,
            };
            assert!(a == 0x58 || a == 0x5a);
        }

        // Unchanged frame: nothing sent.
        log.lock().unwrap().clear();
        dev.set_lights(&colors).unwrap();
        assert!(log.lock().unwrap().is_empty());

        // Temperatures from the DDR4 thermal sensors.
        let snap = dev.poll().unwrap();
        assert_eq!(snap.sensor("dimm1_temperature"), Some(20.0));
        assert_eq!(snap.sensor("dimm3_temperature"), Some(20.0));
    }

    #[test]
    fn ddr5_uses_0x18_range_and_hub_temperature() {
        let mut bus = MockBus::default();
        bus.present.extend([0x51, 0x19]);
        bus.bytes.insert((0x51, 0x00), 0x51);
        bus.bytes.insert((0x51, 0x01), 0x18);
        bus.bytes.insert((0x51, 0x31), ((41 * 4) << 2) as u8);
        bus.bytes.insert((0x51, 0x32), (((41 * 4) << 2) >> 8) as u8);
        bus.bytes.insert((0x19, 0x43), 0x1a);
        bus.bytes.insert((0x19, 0x44), 0x04);
        bus.info.insert(0x19, info_buf(0x0900, 4));
        let mut dev = scan_mock(bus).expect("found");
        assert_eq!(dev.info().model, "Vengeance RGB DDR5");
        assert_eq!(dev.light_zones()[0].id, "dimm2");
        assert_eq!(dev.poll().unwrap().sensor("dimm2_temperature"), Some(41.0));
    }

    #[test]
    fn ddr4_system_never_probes_0x18_range_for_rgb() {
        let mut bus = MockBus::default();
        bus.present.extend([0x50, 0x18]);
        bus.bytes.insert((0x50, 0x00), 0x23);
        // Something at 0x18 that would pass the DRAM ID check.
        bus.bytes.insert((0x18, 0x43), 0x1c);
        bus.bytes.insert((0x18, 0x44), 0x03);
        let reads = bus.reads.clone();
        let log = bus.log.clone();
        assert!(scan_mock(bus).is_none());
        assert!(!reads.lock().unwrap().iter().any(|&(a, r)| a == 0x18 && r == 0x43));
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn empty_slots_are_not_probed() {
        let bus = MockBus { present: vec![0x58], ..Default::default() };
        let reads = bus.reads.clone();
        assert!(scan_mock(bus).is_none());
        assert!(reads.lock().unwrap().is_empty());
    }

    #[test]
    fn rejects_bad_crc_unknown_pid_and_foreign_vid() {
        for case in 0..3 {
            let mut bus = MockBus::default();
            ddr4_dimm(&mut bus, 0, 0x0100);
            match case {
                0 => {
                    bus.bytes.insert((0x58, 0x42), 0x00);
                }
                1 => {
                    bus.info.insert(0x58, info_buf(0x0500, 4));
                }
                _ => {
                    let mut b = info_buf(0x0100, 4);
                    b[0] = 0x34;
                    bus.info.insert(0x58, b);
                }
            }
            let log = bus.log.clone();
            assert!(scan_mock(bus).is_none(), "case {case}");
            // Only the two info-request writes happened.
            assert_eq!(log.lock().unwrap().len(), 2, "case {case}");
        }
    }

    #[test]
    fn id_mismatch_means_no_writes() {
        let mut bus = MockBus::default();
        ddr4_dimm(&mut bus, 0, 0x0100);
        bus.bytes.insert((0x58, 0x44), 0x02);
        let log = bus.log.clone();
        assert!(scan_mock(bus).is_none());
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn old_protocol_gets_temperature_but_no_lighting() {
        let mut bus = MockBus::default();
        ddr4_dimm(&mut bus, 1, 0x0100);
        bus.info.insert(0x59, info_buf(0x0100, 3));
        let mut dev = scan_mock(bus).expect("found");
        assert!(dev.light_zones().is_empty());
        assert_eq!(dev.poll().unwrap().sensor("dimm2_temperature"), Some(20.0));
    }

    #[test]
    fn word_fallback_when_block_writes_unsupported() {
        let mut bus = MockBus { no_block: true, ..Default::default() };
        ddr4_dimm(&mut bus, 0, 0x0100);
        let log = bus.log.clone();
        let mut dev = scan_mock(bus).unwrap();
        log.lock().unwrap().clear();
        let colors = vec![Rgb::new(9, 8, 7); 10];
        dev.set_lights(&colors).unwrap();
        let want: Vec<Op> = word_writes(&direct_packet(&colors, false)).into_iter().map(|(r, w)| Op::Word(0x58, r, w)).collect();
        assert_eq!(*log.lock().unwrap(), want);
    }

    #[test]
    fn original_vengeance() {
        let mut bus = MockBus::default();
        bus.present.extend([0x50, 0x58]);
        for r in 0xa0..=0xaf {
            bus.bytes.insert((0x58, r), 0xba);
        }
        let log = bus.log.clone();
        let mut dev = scan_mock(bus).expect("found");
        assert_eq!(dev.info().model, "Vengeance RGB DDR4");
        assert_eq!(dev.light_zones()[0].leds, 1);
        assert!(log.lock().unwrap().is_empty(), "identification is read-only");
        dev.set_lights(&[Rgb::new(10, 20, 30)]).unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                Op::Byte(0x58, 0xa4, 0),
                Op::Byte(0x58, 0xb0, 10),
                Op::Byte(0x58, 0xb1, 20),
                Op::Byte(0x58, 0xb2, 30),
                Op::Byte(0x58, 0xa6, 0),
            ]
        );
    }

    #[test]
    fn kernel_owned_addresses_use_hwmon_and_are_never_probed() {
        let mut bus = MockBus::default();
        ddr4_dimm(&mut bus, 0, 0x0100);
        bus.bound.insert(0x50, "ee1004".into());
        bus.bound.insert(0x18, "jc42".into());
        bus.os_temps.insert(0x18, 33.5);
        let reads = bus.reads.clone();
        let mut dev = scan_mock(bus).expect("found");
        assert_eq!(dev.poll().unwrap().sensor("dimm1_temperature"), Some(33.5));
        assert!(!reads.lock().unwrap().iter().any(|&(a, _)| a == 0x50 || a == 0x18));
    }

    #[test]
    fn detection_never_writes_spd_space() {
        // Even a (hypothetical) controller answering at an SPD address is never written.
        let mut g = Guarded::new(Box::new(MockBus { present: vec![0x52], ..Default::default() }));
        assert!(g.allow_writes(0x52).is_err());
        assert!(g.write_block_data(0x52, 0x31, &[1]).is_err());
    }
}
