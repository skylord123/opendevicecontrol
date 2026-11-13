//! Motherboard Super I/O chips: decoding their hardware-monitor registers.
//!
//! Pure functions over a register reader, so they are tested on every
//! platform; the Windows sensor service reads the registers through
//! PawnIO's `LpcIO` module (`system::windows`). Linux uses the kernel's
//! drivers (`nct6775`, `it87`…) through hwmon instead.
//!
//! Register layouts from LibreHardwareMonitor's `Nct677X.cs` and
//! `IT87XX.cs` (MPL-2.0) and the Linux `nct6775` / `it87` drivers; pin
//! names as in the vendors' datasheets.

use super::external::{slug, ExtHardware, ExtSensor, ExtTree};
use crate::model::{HardwareKind, SensorKind};

/// Nuvoton's vendor id, in registers 0x804F (high) and 0x004F (low).
pub const NUVOTON_VENDOR_ID: u16 = 0x5CA3;
/// Logical device of the hardware monitor.
pub const NUVOTON_HWM_LDN: u8 = 0x0B;

/// How a Nuvoton chip's hardware monitor is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NctLayout {
    /// NCT6791D…NCT6799D: banked registers, the full temperature table.
    Banked,
    /// NCT6779D / NCT6792DA: banked, the short temperature table.
    BankedOld,
    /// NCT6683D / NCT6686D / NCT6687D(R): an embedded controller behind a
    /// page / index / data window; direct fan RPM registers.
    Ec { dr: bool },
}

/// A supported Nuvoton chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nuvoton {
    pub name: &'static str,
    pub layout: NctLayout,
    pub fans: usize,
    /// Has the TSENSOR input (NCT6798D / NCT6799D).
    pub tsensor: bool,
    /// The sixth PWM output is at 0xA09 (NCT6796D and later), not 0x017.
    pub new_pwm: bool,
    /// The hardware monitor's I/O space can be locked (register 0x28 bit 4).
    pub io_lock: bool,
}

/// Nuvoton chips by Super I/O chip id (0x20) and revision (0x21).
pub fn nuvoton_chip(id: u8, revision: u8) -> Option<Nuvoton> {
    use NctLayout::*;
    let (name, layout, fans, tsensor, new_pwm, io_lock) = match (id, revision) {
        (0xC5, r) if r & 0xF0 == 0x60 => ("Nuvoton NCT6779D", BankedOld, 5, false, false, false),
        (0xC8, 0x03) => ("Nuvoton NCT6791D", Banked, 6, false, false, true),
        (0xC9, 0x11) => ("Nuvoton NCT6792D", Banked, 6, false, false, true),
        (0xC9, 0x13) => ("Nuvoton NCT6792D-A", BankedOld, 6, false, false, true),
        (0xD1, 0x21) => ("Nuvoton NCT6793D", Banked, 6, false, false, true),
        (0xD3, 0x52) => ("Nuvoton NCT6795D", Banked, 6, false, false, true),
        (0xD4, 0x23) => ("Nuvoton NCT6796D", Banked, 6, false, true, true),
        (0xD4, 0x2A) => ("Nuvoton NCT6796D-R", Banked, 7, false, true, true),
        (0xD4, 0x51) => ("Nuvoton NCT6797D", Banked, 7, false, true, true),
        (0xD4, 0x2B) => ("Nuvoton NCT6798D", Banked, 7, true, true, true),
        (0xD8, 0x02) => ("Nuvoton NCT6799D", Banked, 7, true, true, true),
        (0xC7, 0x32) => ("Nuvoton NCT6683D", Ec { dr: false }, 0, false, false, false),
        (0xD4, 0x40 | 0x41) => ("Nuvoton NCT6686D", Ec { dr: false }, 0, false, false, false),
        (0xD5, 0x92) => ("Nuvoton NCT6687D", Ec { dr: false }, 0, false, false, false),
        _ => return None,
    };
    Some(Nuvoton { name, layout, fans, tsensor, new_pwm, io_lock })
}

/// A chip this decoder doesn't know yet, for the status line.
pub fn describe_unknown(id: u8, revision: u8) -> String {
    format!("Super I/O chip {id:02X}{revision:02X} is not supported yet")
}

/// Voltage inputs 0x480–0x48F: (pin, label, internal divider).
const VOLTAGES: [(&str, &str, f64); 16] = [
    ("vcore", "Vcore", 1.0),
    ("vin1", "VIN1", 1.0),
    ("avsb", "AVSB (+3.3V standby)", 2.0),
    ("3vcc", "3VCC (+3.3V)", 2.0),
    ("vin0", "VIN0", 1.0),
    ("vin8", "VIN8", 1.0),
    ("vin4", "VIN4", 1.0),
    ("3vsb", "3VSB (+3.3V standby)", 2.0),
    ("vbat", "VBAT (CMOS battery)", 2.0),
    ("vtt", "VTT", 1.0),
    ("vin5", "VIN5", 1.0),
    ("vin6", "VIN6", 1.0),
    ("vin2", "VIN2", 1.0),
    ("vin3", "VIN3", 1.0),
    ("vin7", "VIN7", 1.0),
    ("vin9", "VIN9", 1.0),
];
const VBAT_INDEX: usize = 8;
/// Bit 0: VBAT monitoring enabled.
const VBAT_MONITOR_CONTROL: u16 = 0x005D;

/// Temperature registers: (fixed source, value, half register, half bit,
/// source select register, alternate register). 0 = none, half bit < 0 =
/// none.
type TempReg = (u8, u16, u16, i8, u16, u16);
const TEMPERATURES: [TempReg; 24] = [
    (16, 0x073, 0x074, 7, 0x100, 0),
    (2, 0x075, 0x076, 7, 0x200, 0x491),
    (1, 0x077, 0x078, 7, 0x300, 0x490),
    (3, 0x079, 0x07A, 7, 0x800, 0x492),
    (4, 0x07B, 0x07C, 7, 0x900, 0x493),
    (5, 0x07D, 0x07E, 7, 0xA00, 0x494),
    (6, 0x4A0, 0x49E, 6, 0xB00, 0x495),
    (7, 0x027, 0, -1, 0x621, 0),
    (10, 0x4A2, 0x4A1, 7, 0xC00, 0x496),
    (8, 0x150, 0x151, 7, 0x622, 0),
    (9, 0x670, 0, -1, 0xC26, 0),
    (17, 0x672, 0, -1, 0xC27, 0),
    (18, 0x674, 0, -1, 0xC28, 0x400),
    (19, 0x676, 0, -1, 0xC29, 0x401),
    (20, 0x678, 0, -1, 0xC2A, 0x402),
    (21, 0x67A, 0, -1, 0xC2B, 0x404),
    (22, 0x405, 0, -1, 0, 0),
    (23, 0x406, 0, -1, 0, 0),
    (24, 0x407, 0, -1, 0, 0),
    (25, 0x408, 0, -1, 0, 0),
    (26, 0x419, 0, -1, 0, 0),
    (27, 0x41A, 0, -1, 0, 0),
    (28, 0x4F4, 0, -1, 0, 0),
    (29, 0x4F5, 0, -1, 0, 0),
];

/// Name of a temperature source (the chip's source select value).
fn source_name(src: u8) -> Option<(&'static str, &'static str)> {
    Some(match src {
        1 => ("systin", "System (SYSTIN)"),
        2 => ("cputin", "CPU socket (CPUTIN)"),
        3 => ("auxtin0", "AUXTIN0"),
        4 => ("auxtin1", "AUXTIN1"),
        5 => ("auxtin2", "AUXTIN2"),
        6 => ("auxtin3", "AUXTIN3"),
        7 => ("auxtin4", "AUXTIN4"),
        8 => ("smbusmaster0", "SMBus master 0"),
        9 => ("smbusmaster1", "SMBus master 1"),
        10 => ("tsensor", "TSENSOR"),
        16 => ("peci0", "CPU (PECI 0)"),
        17 => ("peci1", "CPU (PECI 1)"),
        18 => ("pch_cpu_max", "PCH CPU max"),
        19 => ("pch_chip", "PCH chip"),
        20 => ("pch_cpu", "PCH CPU"),
        21 => ("pch_mch", "PCH MCH"),
        22 => ("dimm0_0", "Agent 0 DIMM 0"),
        23 => ("dimm0_1", "Agent 0 DIMM 1"),
        24 => ("dimm1_0", "Agent 1 DIMM 0"),
        25 => ("dimm1_1", "Agent 1 DIMM 1"),
        26 => ("byte0", "Byte temperature 0"),
        27 => ("byte1", "Byte temperature 1"),
        28 => ("peci0_cal", "PECI 0 calibrated"),
        29 => ("peci1_cal", "PECI 1 calibrated"),
        _ => return None,
    })
}

/// Fan count registers (high byte; the low 5 bits follow) and names.
const FANS: [(u16, &str, &str); 7] = [
    (0x4B0, "sysfan", "System fan (SYSFAN)"),
    (0x4B2, "cpufan", "CPU fan (CPUFAN)"),
    (0x4B4, "auxfan0", "AUX fan 0"),
    (0x4B6, "auxfan1", "AUX fan 1"),
    (0x4B8, "auxfan2", "AUX fan 2"),
    (0x4BA, "auxfan3", "AUX fan 3"),
    (0x4CC, "auxfan4", "AUX fan 4"),
];
/// Fan PWM output registers, same order as [`FANS`].
const PWM_OUT: [u16; 7] = [0x001, 0x003, 0x011, 0x013, 0x015, 0xA09, 0xB09];
/// …on chips before the NCT6796D.
const PWM_OUT_OLD: [u16; 7] = [0x001, 0x003, 0x011, 0x013, 0x015, 0x017, 0x029];
/// NCT6779D / NCT6792DA temperatures (same tuple layout as [`TEMPERATURES`]).
const TEMPERATURES_OLD: [TempReg; 7] = [
    (16, 0x027, 0, -1, 0x621, 0),
    (2, 0x073, 0x074, 7, 0x100, 0x491),
    (1, 0x075, 0x076, 7, 0x200, 0x490),
    (3, 0x077, 0x078, 7, 0x300, 0x492),
    (4, 0x079, 0x07A, 7, 0x800, 0x493),
    (5, 0x07B, 0x07C, 7, 0x900, 0x494),
    (6, 0x150, 0x151, 7, 0x622, 0x495),
];

/// Fan speed from a 13-bit count (1.35 MHz / count); `None` while the
/// count is meaningless, 0 when the fan is stopped.
pub fn fan_rpm(high: u8, low: u8) -> Option<f64> {
    let count = (u32::from(high) << 5) | (u32::from(low) & 0x1F);
    if count >= 0x1FFF {
        Some(0.0)
    } else if count < 0x15 {
        None
    } else {
        Some(1.35e6 / f64::from(count))
    }
}

/// Fans that have turned at least once since start (bit per fan): headers
/// with nothing connected stay hidden.
#[derive(Debug, Default, Clone, Copy)]
pub struct FansSeen(pub u8);

/// Read one Nuvoton chip. `read` returns the byte at a register (bank or
/// page in the high byte), `None` if the bus failed.
pub fn nct679x(chip: Nuvoton, parent: Option<&str>, seen: &mut FansSeen, read: &mut dyn FnMut(u16) -> Option<u8>) -> ExtHardware {
    if let NctLayout::Ec { dr } = chip.layout {
        return nct668x(chip, dr, parent, seen, read);
    }
    let key = format!("sio_{}", slug(chip.name.trim_start_matches("Nuvoton ")));
    let id = |s: &str| format!("{key}_{s}");
    let mut sensors = Vec::new();

    for (i, (pin, label, scale)) in VOLTAGES.iter().enumerate() {
        let Some(raw) = read(0x480 + i as u16) else { continue };
        if i == VBAT_INDEX && read(VBAT_MONITOR_CONTROL).is_none_or(|c| c & 1 == 0) {
            continue;
        }
        let v = f64::from(raw) * 0.008 * scale;
        if v > 0.0 {
            sensors.push(ExtSensor::new(id(pin), *label, SensorKind::Voltage, (v * 1000.0).round() / 1000.0));
        }
    }

    // Each source once: the first register that reports it wins.
    let mut done = [false; 32];
    let table: &[TempReg] = if chip.layout == NctLayout::BankedOld { &TEMPERATURES_OLD } else { &TEMPERATURES };
    for &(fixed, reg, half_reg, half_bit, src_reg, _) in table {
        if fixed == 10 && !chip.tsensor {
            continue;
        }
        let src = if src_reg > 0 {
            match read(src_reg) {
                Some(s) => s & 0x1F,
                None => continue,
            }
        } else {
            fixed
        };
        if src == 0 || done[src as usize] {
            continue;
        }
        let Some(raw) = read(reg) else { continue };
        let mut half = i32::from(raw as i8) << 1;
        if half_bit >= 0 {
            if let Some(h) = read(half_reg) {
                half |= i32::from((h >> half_bit) & 1);
            }
        }
        let t = f64::from(half) * 0.5;
        // Exactly 0: an input nothing drives (unused DIMM, PCH and
        // calibration registers read 0 on boards without them).
        if !(-55.0..=125.0).contains(&t) || t == 0.0 {
            continue;
        }
        let Some((name, label)) = source_name(src) else { continue };
        done[src as usize] = true;
        sensors.push(ExtSensor::new(id(name), label, SensorKind::Temperature, t));
    }
    // Sources without a monitored register: their alternate (whole degrees).
    for &(fixed, _, _, _, _, alt) in table {
        if alt == 0 || done[fixed as usize] {
            continue;
        }
        let Some(raw) = read(alt) else { continue };
        let t = f64::from(raw as i8);
        if t > 0.0 && t <= 125.0 {
            if let Some((name, label)) = source_name(fixed) {
                done[fixed as usize] = true;
                sensors.push(ExtSensor::new(id(name), label, SensorKind::Temperature, t));
            }
        }
    }

    for (i, (reg, name, label)) in FANS.iter().enumerate().take(chip.fans) {
        let (Some(hi), Some(lo)) = (read(*reg), read(reg + 1)) else { continue };
        let Some(rpm) = fan_rpm(hi, lo) else { continue };
        if rpm > 0.0 {
            seen.0 |= 1 << i;
        }
        if seen.0 & (1 << i) != 0 {
            sensors.push(ExtSensor::new(id(name), *label, SensorKind::FanSpeed, rpm.round()));
            let pwm_reg = if chip.new_pwm { PWM_OUT[i] } else { PWM_OUT_OLD[i] };
            if let Some(pwm) = read(pwm_reg) {
                let duty = (f64::from(pwm) / 2.55).round();
                sensors.push(ExtSensor::new(id(&format!("{name}_duty")), format!("{label} control"), SensorKind::Duty, duty));
            }
        }
    }

    ExtHardware { key, name: chip.name.to_string(), kind: HardwareKind::Chip, parent: parent.map(str::to_string), sensors }
}

/// NCT6683D / NCT6686D / NCT6687D: voltages as 12-bit millivolts, direct
/// fan RPM, temperatures in half degrees. Read-only (LibreHardwareMonitor
/// also sets up the chip's fan control, which OpenDeviceControl doesn't touch).
fn nct668x(chip: Nuvoton, dr: bool, parent: Option<&str>, seen: &mut FansSeen, read: &mut dyn FnMut(u16) -> Option<u8>) -> ExtHardware {
    const VOLTS: [u16; 14] = [0x120, 0x122, 0x124, 0x126, 0x128, 0x12A, 0x12C, 0x12E, 0x130, 0x13A, 0x13E, 0x136, 0x138, 0x13C];
    let key = format!("sio_{}", slug(chip.name.trim_start_matches("Nuvoton ")));
    let id = |s: &str| format!("{key}_{s}");
    let mut sensors = Vec::new();
    for (i, reg) in VOLTS.iter().enumerate() {
        let (Some(hi), Some(lo)) = (read(*reg), read(reg + 1)) else { continue };
        let scale = match i {
            0 => 12.0,
            1 => 5.0,
            4 => 2.0,
            _ => 1.0,
        };
        let v = 0.001 * (16.0 * f64::from(hi) + f64::from(lo >> 4)) * scale;
        let (name, label) = match i {
            0 => ("12v".to_string(), "+12V".to_string()),
            1 => ("5v".to_string(), "+5V".to_string()),
            _ => (format!("vin{i}"), format!("VIN{i}")),
        };
        if v > 0.0 {
            sensors.push(ExtSensor::new(id(&name), label, SensorKind::Voltage, (v * 1000.0).round() / 1000.0));
        }
    }
    let temps = if dr { 7 } else { 11 };
    for i in 0..temps {
        let reg = 0x100 + 2 * i as u16;
        let (Some(t), Some(h)) = (read(reg), read(reg + 1)) else { continue };
        let v = f64::from(t as i8) + 0.5 * f64::from((h >> 7) & 1);
        if v > 0.0 && v < 125.0 {
            sensors.push(ExtSensor::new(id(&format!("temp{}", i + 1)), format!("Temperature #{}", i + 1), SensorKind::Temperature, v));
        }
    }
    const FANS_DR: [u16; 16] = [0x140, 0x142, 0x144, 0x146, 0x148, 0x14A, 0x14C, 0x14E, 0x150, 0x152, 0x15E, 0x15C, 0x15A, 0x158, 0x156, 0x154];
    let fans: Vec<u16> = if dr { FANS_DR.to_vec() } else { (0..16).map(|i| 0x140 + 2 * i).collect() };
    for (i, reg) in fans.iter().enumerate() {
        let (Some(hi), Some(lo)) = (read(*reg), read(reg + 1)) else { continue };
        let rpm = f64::from(u16::from(hi) << 8 | u16::from(lo));
        if rpm > 0.0 && rpm < 30_000.0 {
            seen.0 |= 1 << i.min(7);
        }
        if seen.0 & (1 << i.min(7)) == 0 || rpm >= 30_000.0 {
            continue;
        }
        let n = i + 1;
        sensors.push(ExtSensor::new(id(&format!("fan{n}")), format!("Fan #{n}"), SensorKind::FanSpeed, rpm));
        if i < 8 {
            if let Some(d) = read(0x160 + i as u16) {
                sensors.push(ExtSensor::new(id(&format!("fan{n}_duty")), format!("Fan #{n} control"), SensorKind::Duty, (f64::from(d) / 2.55).round()));
            }
        }
    }
    ExtHardware { key, name: chip.name.to_string(), kind: HardwareKind::Chip, parent: parent.map(str::to_string), sensors }
}

// ---------------------------------------------------------------------------
// Fintek

/// Fintek's vendor id, Super I/O register 0x23 (word).
pub const FINTEK_VENDOR_ID: u16 = 0x1934;

/// A supported Fintek chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fintek {
    pub name: &'static str,
    pub ldn: u8,
    pub voltages: usize,
    pub temps: usize,
    pub fans: usize,
    /// F71858: temperatures in a different format.
    pub f71858: bool,
}

/// Fintek chips by chip id (0x20) and revision (0x21).
pub fn fintek_chip(id: u8, revision: u8) -> Option<Fintek> {
    let c = |name, voltages, temps, fans| Fintek { name, ldn: 0x04, voltages, temps, fans, f71858: false };
    Some(match (id, revision) {
        (0x05, 0x07) => Fintek { ldn: 0x02, f71858: true, ..c("Fintek F71858", 3, 3, 4) },
        (0x05, 0x41) => c("Fintek F71882", 9, 3, 4),
        (0x06, 0x01) => c("Fintek F71862", 9, 3, 3),
        (0x07, 0x23) => c("Fintek F71889F", 9, 3, 3),
        (0x08, 0x14) => c("Fintek F71869", 9, 3, 3),
        (0x09, 0x01) => c("Fintek F71808E", 9, 2, 3),
        (0x09, 0x09) => c("Fintek F71889ED", 9, 3, 3),
        (0x10, 0x05) => c("Fintek F71889AD", 9, 3, 3),
        (0x10, 0x07) => c("Fintek F71869A", 9, 3, 3),
        (0x11, 0x06) => c("Fintek F71878AD", 9, 3, 3),
        _ => return None,
    })
}

/// Read one Fintek chip. `read` returns a register byte.
pub fn fintek(chip: Fintek, parent: Option<&str>, seen: &mut FansSeen, read: &mut dyn FnMut(u8) -> Option<u8>) -> ExtHardware {
    let key = format!("sio_{}", slug(chip.name.trim_start_matches("Fintek ")));
    let id = |s: &str| format!("{key}_{s}");
    let mut sensors = Vec::new();
    for i in 0..chip.voltages {
        if chip.name == "Fintek F71808E" && i == 6 {
            continue;
        }
        let Some(raw) = read(0x20 + i as u8) else { continue };
        let v = 0.008 * f64::from(raw);
        if v > 0.0 {
            sensors.push(ExtSensor::new(id(&format!("vin{i}")), format!("VIN{i}"), SensorKind::Voltage, (v * 1000.0).round() / 1000.0));
        }
    }
    for i in 0..chip.temps {
        let t = if chip.f71858 {
            let mode = read(0x69).unwrap_or(0) & 3;
            let (Some(hi), Some(lo)) = (read(0x70 + 2 * i as u8), read(0x71 + 2 * i as u8)) else { continue };
            if hi == 0xBB || hi == 0xCC {
                continue;
            }
            let mut bits: i32 = match mode {
                2 => (i32::from(hi) & 0x80) << 8,
                3 => (i32::from(lo) & 0x01) << 15,
                _ => 0,
            };
            bits |= i32::from(hi) << 7;
            bits |= (i32::from(lo) & 0xE0) >> 1;
            f64::from((bits & 0xFFF0) as i16) / 128.0
        } else {
            let Some(raw) = read(0x72 + 2 * i as u8) else { continue };
            f64::from(raw as i8)
        };
        if t > 0.0 && t < 127.0 {
            sensors.push(ExtSensor::new(id(&format!("temp{}", i + 1)), format!("Temperature #{}", i + 1), SensorKind::Temperature, t));
        }
    }
    for i in 0..chip.fans.min(4) {
        let reg = 0xA0 + 0x10 * i as u8;
        let (Some(hi), Some(lo)) = (read(reg), read(reg + 1)) else { continue };
        let count = u32::from(hi) << 8 | u32::from(lo);
        if count == 0 {
            continue;
        }
        let rpm = if count < 0x0FFF { 1.5e6 / f64::from(count) } else { 0.0 };
        if rpm > 0.0 {
            seen.0 |= 1 << i;
        }
        if seen.0 & (1 << i) == 0 {
            continue;
        }
        let n = i + 1;
        sensors.push(ExtSensor::new(id(&format!("fan{n}")), format!("Fan #{n}"), SensorKind::FanSpeed, rpm.round()));
    }
    ExtHardware { key, name: chip.name.to_string(), kind: HardwareKind::Chip, parent: parent.map(str::to_string), sensors }
}

// ---------------------------------------------------------------------------
// ITE

/// ITE's vendor ids, register 0x58.
pub const ITE_VENDOR_IDS: [u8; 2] = [0x90, 0x7F];
/// Logical device of the environment controller.
pub const ITE_EC_LDN: u8 = 0x04;

/// A supported ITE chip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ite {
    pub name: &'static str,
    pub voltages: usize,
    /// Volts per step.
    pub gain: f64,
    pub temps: usize,
    pub fans: usize,
    pub controls: usize,
    /// IT8665E / IT8625E: the 6th fan and PWM registers differ.
    pub alt6: bool,
    /// IT8665E / IT8655E: bank 0 must be selected before reading.
    pub bank0: bool,
    /// IT8688E: its address register doesn't read back.
    pub no_readback: bool,
    /// Configuration register 0x00 bit 4 must be set (all but IT8655E /
    /// IT8665E).
    pub check_config: bool,
}

/// ITE chips by their 16-bit chip id (0x20, 0x21).
pub fn ite_chip(id: u16) -> Option<Ite> {
    let c = |name, voltages, gain, temps, fans, controls| Ite {
        name,
        voltages,
        gain,
        temps,
        fans,
        controls,
        alt6: false,
        bank0: false,
        no_readback: false,
        check_config: true,
    };
    Some(match id {
        0x8686 => c("ITE IT8686E", 10, 0.012, 7, 6, 5),
        0x8688 => Ite { no_readback: true, ..c("ITE IT8688E", 11, 0.012, 6, 6, 5) },
        0x8689 => c("ITE IT8689E", 10, 0.012, 6, 6, 6),
        0x8628 => c("ITE IT8628E", 10, 0.012, 6, 6, 6),
        0x8665 => Ite { alt6: true, bank0: true, check_config: false, ..c("ITE IT8665E", 9, 0.0109, 6, 6, 6) },
        0x8655 => Ite { bank0: true, check_config: false, ..c("ITE IT8655E", 9, 0.0109, 6, 3, 3) },
        0x8695 => c("ITE IT87952E", 9, 0.011, 3, 3, 3),
        0x8613 => c("ITE IT8613E", 10, 0.012, 4, 5, 4),
        0x8625 => Ite { alt6: true, ..c("ITE IT8625E", 7, 0.011, 3, 6, 6) },
        0x8620 => c("ITE IT8620E", 9, 0.012, 3, 5, 5),
        0x8728 => c("ITE IT8728F", 9, 0.012, 3, 5, 3),
        _ => return None,
    })
}

const ITE_VIN: [u8; 13] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x2F, 0x2C, 0x2D, 0x2E];
const ITE_TEMP: u8 = 0x29;
const ITE_TACH: [u8; 6] = [0x0D, 0x0E, 0x0F, 0x80, 0x82, 0x4C];
const ITE_TACH_EXT: [u8; 6] = [0x18, 0x19, 0x1A, 0x81, 0x83, 0x4D];
const ITE_TACH_ALT: [u8; 6] = [0x0D, 0x0E, 0x0F, 0x80, 0x82, 0x93];
const ITE_TACH_EXT_ALT: [u8; 6] = [0x18, 0x19, 0x1A, 0x81, 0x83, 0x94];
const ITE_PWM: [u8; 6] = [0x15, 0x16, 0x17, 0x7F, 0xA7, 0xAF];
const ITE_PWM_ALT: [u8; 6] = [0x15, 0x16, 0x17, 0x1E, 0x1F, 0x92];
const ITE_PWM_EXT: [u8; 6] = [0x63, 0x6B, 0x73, 0x7B, 0xA3, 0xAB];

/// Read one ITE chip. `read` returns a register byte (no banks).
pub fn ite(chip: Ite, parent: Option<&str>, seen: &mut FansSeen, read: &mut dyn FnMut(u8) -> Option<u8>) -> ExtHardware {
    let key = format!("sio_{}", slug(chip.name.trim_start_matches("ITE ")));
    let id = |s: &str| format!("{key}_{s}");
    let mut sensors = Vec::new();
    for (i, reg) in ITE_VIN.iter().enumerate().take(chip.voltages) {
        let Some(raw) = read(*reg) else { continue };
        let v = f64::from(raw) * chip.gain;
        if v > 0.0 {
            sensors.push(ExtSensor::new(id(&format!("vin{i}")), format!("VIN{i}"), SensorKind::Voltage, (v * 1000.0).round() / 1000.0));
        }
    }
    for i in 0..chip.temps {
        let Some(raw) = read(ITE_TEMP + i as u8) else { continue };
        let t = raw as i8;
        if t > 0 && t < i8::MAX {
            sensors.push(ExtSensor::new(id(&format!("temp{}", i + 1)), format!("Temperature #{}", i + 1), SensorKind::Temperature, f64::from(t)));
        }
    }
    let (tach, ext, pwm) = if chip.alt6 { (ITE_TACH_ALT, ITE_TACH_EXT_ALT, ITE_PWM_ALT) } else { (ITE_TACH, ITE_TACH_EXT, ITE_PWM) };
    let enable = read(0x0C).unwrap_or(0xFF);
    let enable_b = read(0x0B).unwrap_or(0xFF);
    for i in 0..chip.fans.min(6) {
        let on = match i {
            3 => enable & 0x10 != 0,
            4 => enable & 0x20 != 0,
            5 if chip.name == "ITE IT8665E" => enable_b & 0x08 != 0,
            5 => enable & 0x04 != 0,
            _ => true,
        };
        if !on {
            continue;
        }
        let (Some(lo), Some(hi)) = (read(tach[i]), read(ext[i])) else { continue };
        let count = u32::from(lo) | (u32::from(hi) << 8);
        if count <= 0x3F {
            continue;
        }
        let rpm = if count < 0xFFFF { 1.35e6 / (f64::from(count) * 2.0) } else { 0.0 };
        if rpm > 0.0 {
            seen.0 |= 1 << i;
        }
        if seen.0 & (1 << i) == 0 {
            continue;
        }
        let n = i + 1;
        sensors.push(ExtSensor::new(id(&format!("fan{n}")), format!("Fan #{n}"), SensorKind::FanSpeed, rpm.round()));
        if i < chip.controls {
            // Automatic mode (bit 7) has no fixed duty to show.
            if let Some(ctrl) = read(pwm[i]).filter(|c| c & 0x80 == 0) {
                let _ = ctrl;
                if let Some(v) = read(ITE_PWM_EXT[i]) {
                    let duty = (f64::from(v) * 100.0 / 255.0).round();
                    sensors.push(ExtSensor::new(id(&format!("fan{n}_duty")), format!("Fan #{n} control"), SensorKind::Duty, duty));
                }
            }
        }
    }
    ExtHardware { key, name: chip.name.to_string(), kind: HardwareKind::Chip, parent: parent.map(str::to_string), sensors }
}

/// The motherboard node with its Super I/O chips below it.
pub fn board_tree(source: &'static str, board: Option<&str>, chips: Vec<ExtHardware>) -> ExtTree {
    let mut hardware = Vec::new();
    let parent = board.map(|b| {
        let key = "sio_board".to_string();
        hardware.push(ExtHardware { key: key.clone(), name: b.to_string(), kind: HardwareKind::Motherboard, parent: None, sensors: Vec::new() });
        key
    });
    for mut c in chips {
        c.parent = parent.clone();
        hardware.push(c);
    }
    ExtTree { source, hardware }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn fan_counts() {
        assert_eq!(fan_rpm(0xFF, 0x1F), Some(0.0));
        assert_eq!(fan_rpm(0, 0x10), None);
        // 1000 counts: 1350 rpm.
        assert_eq!(fan_rpm((1000 >> 5) as u8, (1000 & 0x1F) as u8), Some(1350.0));
    }

    #[test]
    fn decodes_nct6799d() {
        let mut regs: HashMap<u16, u8> = HashMap::new();
        regs.insert(0x480, 150); // Vcore 1.2 V
        regs.insert(0x483, 206); // 3VCC 3.296 V
        regs.insert(0x488, 190); // VBAT 3.04 V
        regs.insert(0x005D, 1);
        // CPUTIN 45.5 °C, its source register says CPUTIN.
        regs.insert(0x200, 2);
        regs.insert(0x075, 45);
        regs.insert(0x076, 0x80);
        // SYSTIN 33 °C.
        regs.insert(0x300, 1);
        regs.insert(0x077, 33);
        // An input sourced to SYSTIN as well: ignored.
        regs.insert(0x800, 1);
        regs.insert(0x079, 90);
        // CPU fan at 1350 rpm with 40 % duty; system fan never turned.
        regs.insert(0x4B2, (1000 >> 5) as u8);
        regs.insert(0x4B3, (1000 & 0x1F) as u8);
        regs.insert(0x003, 102);
        regs.insert(0x4B0, 0xFF);
        regs.insert(0x4B1, 0x1F);
        let mut seen = FansSeen::default();
        let chip = nuvoton_chip(0xD8, 0x02).unwrap();
        let hw = nct679x(chip, Some("sio_board"), &mut seen, &mut |r| Some(*regs.get(&r).unwrap_or(&0)));
        let got: Vec<(&str, f64)> = hw.sensors.iter().map(|s| (s.id.as_str(), s.value)).collect();
        assert_eq!(
            got,
            vec![
                ("sio_nct6799d_vcore", 1.2),
                ("sio_nct6799d_3vcc", 3.296),
                ("sio_nct6799d_vbat", 3.04),
                ("sio_nct6799d_cputin", 45.5),
                ("sio_nct6799d_systin", 33.0),
                ("sio_nct6799d_cpufan", 1350.0),
                ("sio_nct6799d_cpufan_duty", 40.0),
            ]
        );
        assert_eq!(hw.parent.as_deref(), Some("sio_board"));
        assert_eq!(nuvoton_chip(0xD4, 0x23).map(|c| c.fans), Some(6));
    }

    #[test]
    fn decodes_nct6687d() {
        let mut regs: HashMap<u16, u8> = HashMap::new();
        // +12V: 1.0 V on the pin (16 × 62 + 8 = 1000 mV) × 12.
        regs.insert(0x120, 62);
        regs.insert(0x121, 8 << 4);
        // Temperature #1: 41.5 °C.
        regs.insert(0x100, 41);
        regs.insert(0x101, 0x80);
        // Fan #2: 1200 rpm, 45 % (115 / 2.55).
        regs.insert(0x142, (1200 >> 8) as u8);
        regs.insert(0x143, (1200 & 0xFF) as u8);
        regs.insert(0x161, 115);
        let chip = nuvoton_chip(0xD5, 0x92).unwrap();
        let mut seen = FansSeen::default();
        let hw = nct679x(chip, None, &mut seen, &mut |r| Some(*regs.get(&r).unwrap_or(&0)));
        let got: Vec<(&str, f64)> = hw.sensors.iter().map(|s| (s.id.as_str(), s.value)).collect();
        assert_eq!(
            got,
            vec![("sio_nct6687d_12v", 12.0), ("sio_nct6687d_temp1", 41.5), ("sio_nct6687d_fan2", 1200.0), ("sio_nct6687d_fan2_duty", 45.0)]
        );
    }

    #[test]
    fn decodes_fintek() {
        let mut regs: HashMap<u8, u8> = HashMap::new();
        regs.insert(0x20, 125); // VIN0 1.0 V
        regs.insert(0x72, 38); // Temperature #1
        // Fan #1: 1000 counts → 1500 rpm.
        regs.insert(0xA0, (1000 >> 8) as u8);
        regs.insert(0xA1, (1000 & 0xFF) as u8);
        let chip = fintek_chip(0x05, 0x41).unwrap();
        let mut seen = FansSeen::default();
        let hw = fintek(chip, None, &mut seen, &mut |r| Some(*regs.get(&r).unwrap_or(&0)));
        let got: Vec<(&str, f64)> = hw.sensors.iter().map(|s| (s.id.as_str(), s.value)).collect();
        assert_eq!(got, vec![("sio_f71882_vin0", 1.0), ("sio_f71882_temp1", 38.0), ("sio_f71882_fan1", 1500.0)]);
    }

    #[test]
    fn decodes_ite() {
        let mut regs: HashMap<u8, u8> = HashMap::new();
        regs.insert(0x20, 100); // VIN0 1.2 V
        regs.insert(0x29, 40); // Temperature #1
        regs.insert(0x2A, 0x80); // -128: not connected
        // Fan #1: count 0x0258 = 600 → 1125 rpm, manual 50 %.
        regs.insert(0x0D, 0x58);
        regs.insert(0x18, 0x02);
        regs.insert(0x15, 0x00);
        regs.insert(0x63, 128);
        regs.insert(0x0C, 0x00);
        let chip = ite_chip(0x8686).unwrap();
        let mut seen = FansSeen::default();
        let hw = ite(chip, None, &mut seen, &mut |r| Some(*regs.get(&r).unwrap_or(&0)));
        let got: Vec<(&str, f64)> = hw.sensors.iter().map(|s| (s.id.as_str(), s.value)).collect();
        assert_eq!(
            got,
            vec![("sio_it8686e_vin0", 1.2), ("sio_it8686e_temp1", 40.0), ("sio_it8686e_fan1", 1125.0), ("sio_it8686e_fan1_duty", 50.0)]
        );
    }
}
