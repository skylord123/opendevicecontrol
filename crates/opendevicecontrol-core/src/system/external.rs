//! Sensors from outside sysinfo: hardware trees read from other tools or
//! the kernel, and the CPU's own thermal registers.
//!
//! Sources (see `docs/windows-sensors.md` and `docs/sensors.md`):
//!
//! * **PawnIO** (Windows, opt-in, administrator): OpenDeviceControl reads the CPU's
//!   thermal registers through the signed PawnIO driver — AMD family
//!   17h–1Ah Tctl/CCD temperatures over SMN, Intel package temperature from
//!   `IA32_PACKAGE_THERM_STATUS`.
//! * **HWiNFO** shared memory ("Shared Memory Support" enabled in HWiNFO).
//! * **LibreHardwareMonitor**: its web server's `data.json` (current
//!   versions, "Remote Web Server" enabled), or its WMI namespace
//!   `root\LibreHardwareMonitor` (versions up to 0.9.4).
//! * **hwmon** (Linux): every chip in `/sys/class/hwmon` (see `hwmon.rs`).
//!
//! Each source yields an [`ExtTree`]: hardware nodes with all their
//! sensors (temperatures, voltages, fans, clocks, loads…). The decoders and
//! parsers here are platform independent so they are tested everywhere.

use crate::model::{HardwareKind, SensorKind};

/// Which part of the machine a temperature belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    Cpu,
    /// Motherboard Super I/O / embedded controller.
    Board,
    /// Anything else (drives, GPU, ACPI zones…).
    Other,
}

/// A temperature, for picking `cpu_temp`.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// Where it came from (`PawnIO`, `HWiNFO`, `LibreHardwareMonitor`…).
    pub source: &'static str,
    pub area: Area,
    pub label: String,
    pub celsius: f64,
}

impl Reading {
    pub fn new(source: &'static str, area: Area, label: impl Into<String>, celsius: f64) -> Self {
        Reading { source, area, label: label.into(), celsius }
    }

    /// Counts towards `cpu_temp` (the hottest CPU temperature).
    pub fn is_cpu_temperature(&self) -> bool {
        let l = self.label.to_lowercase();
        self.area == Area::Cpu && !["distance", "tjmax", "limit", "throttl", "critical"].iter().any(|k| l.contains(k))
    }
}

/// Plausible temperature in °C.
pub fn plausible(t: f64) -> bool {
    t.is_finite() && t > 0.0 && t < 150.0
}

/// Lower-case `[a-z0-9_]` id fragment.
pub fn slug(s: &str) -> String {
    let id: String = s.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let id = id.split('_').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("_");
    if id.is_empty() { "x".into() } else { id }
}

// ---------------------------------------------------------------------------
// Hardware trees.

/// One sensor of an [`ExtHardware`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExtSensor {
    /// Unique sensor id (`[a-z0-9_]`, prefixed by the source).
    pub id: String,
    pub label: String,
    pub kind: SensorKind,
    /// In OpenDeviceControl's unit for `kind` (°C, V, MHz, GB, MB/s…).
    pub value: f64,
    /// Published to Home Assistant by default (legacy sensors).
    pub default_on: bool,
    /// Diagnostic entity in Home Assistant.
    pub diagnostic: bool,
}

impl ExtSensor {
    pub fn new(id: impl Into<String>, label: impl Into<String>, kind: SensorKind, value: f64) -> Self {
        ExtSensor { id: id.into(), label: label.into(), kind, value, default_on: false, diagnostic: false }
    }
}

/// A piece of hardware with its sensors, as one source sees it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExtHardware {
    /// Unique node id within the source (`[a-z0-9_]`, prefixed by the source).
    pub key: String,
    pub name: String,
    pub kind: HardwareKind,
    /// Key of the parent node in the same tree.
    pub parent: Option<String>,
    pub sensors: Vec<ExtSensor>,
}

/// Everything one source reports.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtTree {
    pub source: &'static str,
    pub hardware: Vec<ExtHardware>,
}

impl ExtTree {
    /// Temperatures, for `cpu_temp`.
    pub fn readings(&self) -> Vec<Reading> {
        let mut out = Vec::new();
        for h in &self.hardware {
            let area = match h.kind {
                HardwareKind::Cpu => Area::Cpu,
                HardwareKind::Chip | HardwareKind::Motherboard => Area::Board,
                _ => Area::Other,
            };
            for s in h.sensors.iter().filter(|s| s.kind == SensorKind::Temperature && plausible(s.value)) {
                out.push(Reading::new(self.source, area, s.label.clone(), s.value));
            }
        }
        out
    }

    pub fn sensor_count(&self) -> usize {
        self.hardware.iter().map(|h| h.sensors.len()).sum()
    }
}

/// A number with an optional unit suffix: "45.3 °C", "45,3 °C",
/// "4749312.0 B/s", "1.2 GB". `None` for "-" or empty values.
pub fn number_and_unit(v: &str) -> Option<(f64, String)> {
    let v = v.trim();
    let end = v.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+' | 'e' | 'E'))).unwrap_or(v.len());
    let (num, unit) = v.split_at(end);
    let num = num.replace(',', ".");
    let n: f64 = num.parse().ok()?;
    n.is_finite().then(|| (n, unit.trim().to_string()))
}

/// Convert a value in `unit` to OpenDeviceControl's unit for `kind`.
pub fn convert(kind: SensorKind, value: f64, unit: &str) -> f64 {
    let u = unit.trim();
    match kind {
        SensorKind::Temperature if u.ends_with('F') => (value - 32.0) / 1.8,
        SensorKind::DataSize => match u {
            "B" => value / (1u64 << 30) as f64,
            "KB" => value / (1u64 << 20) as f64,
            "MB" => value / 1024.0,
            "TB" => value * 1024.0,
            _ => value,
        },
        SensorKind::DataRate => match u {
            "B/s" => value / (1u64 << 20) as f64,
            "KB/s" => value / 1024.0,
            "GB/s" => value * 1024.0,
            _ => value,
        },
        SensorKind::Frequency => match u {
            "Hz" => value / 1e6,
            "kHz" => value / 1e3,
            "GHz" => value * 1e3,
            _ => value,
        },
        SensorKind::Voltage if u == "mV" => value / 1000.0,
        SensorKind::Power if u == "mW" => value / 1000.0,
        SensorKind::Current if u == "mA" => value / 1000.0,
        _ => value,
    }
}

// ---------------------------------------------------------------------------
// AMD family 17h–1Ah (Zen) through SMN, as Linux k10temp.

/// SMN address of the reported temperature control register (`THM_TCON_CUR_TMP`).
pub const AMD_SMN_TEMP: u32 = 0x0005_9800;

/// Tctl in °C from `THM_TCON_CUR_TMP` (k10temp `read_tempreg_nb_zen`).
pub fn amd_tctl(reg: u32) -> f64 {
    let mut t = f64::from(reg >> 21) * 0.125;
    // "Range select" or both TJ_SEL bits: the reading is offset by 49 °C.
    if reg & (1 << 19) != 0 || reg & (0b11 << 16) == (0b11 << 16) {
        t -= 49.0;
    }
    t
}

/// CCD temperature register: `None` if the CCD is absent / invalid.
pub fn amd_ccd(reg: u32) -> Option<f64> {
    (reg & (1 << 11) != 0).then(|| f64::from(reg & 0x7ff) * 0.125 - 49.0)
}

/// SMN offset (from [`AMD_SMN_TEMP`]) of CCD 0's temperature register and
/// the number of CCD registers, per family/model (k10temp `k10temp_probe`).
pub fn amd_ccd_layout(family: u32, model: u32) -> Option<(u32, u32)> {
    match (family, model) {
        (0x17, 0x31 | 0x60 | 0x68 | 0x71) => Some((0x154, 4)),
        (0x19, 0x00..=0x01 | 0x08 | 0x21 | 0x50..=0x5f) => Some((0x154, 8)),
        (0x19, 0x40..=0x4f) => Some((0x300, 8)),
        (0x19, 0x60..=0x7f) => Some((0x308, 8)),
        (0x19, 0x10..=0x1f | 0xa0..=0xaf) => Some((0x300, 12)),
        (0x1a, 0x00..=0x2f | 0x40..=0x4f) => Some((0x308, 8)),
        _ => None,
    }
}

/// Tctl reads higher than the die on some first/second generation Ryzen and
/// Threadripper parts (k10temp `tctl_offset_table`). Returns the offset.
pub fn amd_tctl_offset(brand: &str) -> f64 {
    const TABLE: &[(&str, f64)] = &[
        ("AMD Ryzen 5 1600X", 20.0),
        ("AMD Ryzen 7 1700X", 20.0),
        ("AMD Ryzen 7 1800X", 20.0),
        ("AMD Ryzen 7 2700X", 10.0),
        ("AMD Ryzen Threadripper 19", 27.0),
        ("AMD Ryzen Threadripper 29", 27.0),
    ];
    TABLE.iter().find(|(k, _)| brand.contains(k)).map_or(0.0, |(_, o)| *o)
}

/// PawnIO's CPU node from the AMD registers: `tctl` and `(index, reg)` per CCD.
pub fn amd_tree(brand: &str, tctl: u32, offset: f64, ccds: &[(u32, u32)]) -> ExtTree {
    let mut sensors = Vec::new();
    let t = amd_tctl(tctl) - offset;
    if plausible(t) {
        sensors.push(ExtSensor::new("pawnio_cpu_tctl", "CPU Tctl", SensorKind::Temperature, t));
    }
    for &(i, reg) in ccds {
        if let Some(t) = amd_ccd(reg).filter(|t| plausible(*t)) {
            let n = i + 1;
            sensors.push(ExtSensor::new(format!("pawnio_cpu_ccd{n}"), format!("CPU CCD{n} (Tdie)"), SensorKind::Temperature, t));
        }
    }
    pawnio_tree(brand, sensors)
}

fn pawnio_tree(brand: &str, sensors: Vec<ExtSensor>) -> ExtTree {
    let hardware = if sensors.is_empty() {
        Vec::new()
    } else {
        vec![ExtHardware { key: "pawnio_cpu".into(), name: brand.to_string(), kind: HardwareKind::Cpu, parent: None, sensors }]
    };
    ExtTree { source: "PawnIO", hardware }
}

// ---------------------------------------------------------------------------
// Intel, digital thermal sensor MSRs.

pub const MSR_IA32_PACKAGE_THERM_STATUS: u32 = 0x1b1;
pub const MSR_IA32_TEMPERATURE_TARGET: u32 = 0x1a2;

/// Package temperature from `IA32_PACKAGE_THERM_STATUS` and
/// `MSR_TEMPERATURE_TARGET` (TjMax in bits 23:16, 100 °C if unset).
pub fn intel_package(therm_status: u64, temperature_target: u64) -> Option<f64> {
    // Bit 31: reading valid (older parts) — package status always sets it.
    if therm_status & (1 << 31) == 0 {
        return None;
    }
    let tjmax = match (temperature_target >> 16) & 0xff {
        0 => 100.0,
        t => t as f64,
    };
    let t = tjmax - ((therm_status >> 16) & 0x7f) as f64;
    plausible(t).then_some(t)
}

/// PawnIO's CPU node for an Intel package temperature.
pub fn intel_tree(brand: &str, package: Option<f64>) -> ExtTree {
    let sensors = package
        .map(|t| ExtSensor::new("pawnio_cpu_package", "CPU Package", SensorKind::Temperature, t))
        .into_iter()
        .collect();
    pawnio_tree(brand, sensors)
}

// ---------------------------------------------------------------------------
// HWiNFO shared memory (`hwisenssm2.h`, `#pragma pack(1)`).

pub const HWINFO_SIGNATURE: u32 = u32::from_le_bytes(*b"HWiS");
const STR_LEN: usize = 128;

fn le_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le_f64(b: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// NUL-terminated string field. UTF-8 when valid, else Latin-1 (the ANSI
/// fields are in the system code page, which is Latin-1 for the labels
/// HWiNFO uses).
fn c_str(b: &[u8], at: usize, len: usize) -> Option<String> {
    let f = b.get(at..at + len)?;
    let f = &f[..f.iter().position(|&c| c == 0).unwrap_or(len)];
    Some(match std::str::from_utf8(f) {
        Ok(s) => s.trim().to_string(),
        Err(_) => f.iter().map(|&c| c as char).collect::<String>().trim().to_string(),
    })
}

/// Bytes HWiNFO's mapping must have for its header to be complete:
/// `Some(total size)` once the 44-byte header is readable.
pub fn hwinfo_size(b: &[u8]) -> Option<usize> {
    let section = |at: usize| -> Option<usize> {
        let (off, size, num) = (le_u32(b, at)? as usize, le_u32(b, at + 4)? as usize, le_u32(b, at + 8)? as usize);
        off.checked_add(size.checked_mul(num)?)
    };
    Some(section(20)?.max(section(32)?))
}

/// Super I/O / EC sensor names ("ASUS ROG STRIX X670E-E (Nuvoton NCT6799D)").
fn is_board_chip(name: &str) -> bool {
    let n = name.to_lowercase();
    ["nuvoton", "nct6", "ite it", "it86", "it87", "fintek", "embedded controller", "smsc", "winbond"]
        .iter()
        .any(|k| n.contains(k))
}

/// Kind of an HWiNFO sensor from its original (English) name.
fn hwinfo_kind(orig: &str) -> HardwareKind {
    let n = orig.to_lowercase();
    if n.starts_with("cpu [") {
        HardwareKind::Cpu
    } else if n.starts_with("gpu [") {
        HardwareKind::Gpu
    } else if is_board_chip(&n) {
        HardwareKind::Chip
    } else if n.starts_with("s.m.a.r.t") || n.starts_with("drive") || n.contains("nvme") {
        HardwareKind::Storage
    } else if n.starts_with("dimm") || n.starts_with("memory") || n.contains("spd hub") {
        HardwareKind::Memory
    } else if n.starts_with("network") {
        HardwareKind::Network
    } else if n.starts_with("battery") {
        HardwareKind::Battery
    } else if n.starts_with("system") {
        HardwareKind::Computer
    } else {
        HardwareKind::Other
    }
}

/// (kind, unit) of an HWiNFO reading type; "other" readings by unit.
fn hwinfo_sensor_kind(ty: u32, unit: &str) -> Option<SensorKind> {
    Some(match ty {
        1 => SensorKind::Temperature,
        2 => SensorKind::Voltage,
        3 => SensorKind::FanSpeed,
        4 => SensorKind::Current,
        5 => SensorKind::Power,
        6 => SensorKind::Frequency,
        7 => SensorKind::Load,
        8 => match unit {
            "%" => SensorKind::Load,
            "B" | "KB" | "MB" | "GB" | "TB" => SensorKind::DataSize,
            "B/s" | "KB/s" | "MB/s" | "GB/s" => SensorKind::DataRate,
            "W" => SensorKind::Power,
            "RPM" => SensorKind::FanSpeed,
            "l/h" | "L/h" => SensorKind::Flow,
            _ => return None,
        },
        _ => return None,
    })
}

/// "CPU [#0]: AMD Ryzen 7 7800X3D: Enhanced" → ("CPU [#0]: AMD Ryzen 7 7800X3D", "Enhanced").
fn hwinfo_split(orig: &str) -> (String, Option<String>) {
    let mut parts = orig.splitn(3, ": ");
    let (a, b, c) = (parts.next().unwrap_or(""), parts.next(), parts.next());
    match (b, c) {
        (Some(b), Some(c)) if a.ends_with(']') => (format!("{a}: {b}"), Some(c.to_string())),
        _ => (orig.to_string(), None),
    }
}

/// Every sensor HWiNFO publishes, from a copy of its shared memory. `None`
/// if HWiNFO is not publishing (missing / `DEAD` signature, truncated).
///
/// HWiNFO lists several "sensors" per CPU or GPU ("CPU [#0]: Ryzen 7",
/// "CPU [#0]: Ryzen 7: Enhanced"…); the extra ones become children of the
/// first, so the tree has one CPU node.
pub fn parse_hwinfo(b: &[u8]) -> Option<ExtTree> {
    if le_u32(b, 0)? != HWINFO_SIGNATURE || b.len() < hwinfo_size(b)? {
        return None;
    }
    let (s_off, s_size, s_num) = (le_u32(b, 20)? as usize, le_u32(b, 24)? as usize, le_u32(b, 28)? as usize);
    let (r_off, r_size, r_num) = (le_u32(b, 32)? as usize, le_u32(b, 36)? as usize, le_u32(b, 40)? as usize);
    // Version 1 layouts; version 2 elements are larger (UTF-8 strings appended).
    if s_size < 8 + 2 * STR_LEN || r_size < 12 + 2 * STR_LEN + 16 + 32 {
        return None;
    }
    let s_utf8 = s_size >= 8 + 3 * STR_LEN;
    let r_utf8 = r_size >= 12 + 2 * STR_LEN + 16 + 32 + STR_LEN + 16;

    let mut hardware: Vec<ExtHardware> = Vec::new();
    // Per sensor index: the node it went to.
    let mut node_of: Vec<usize> = Vec::new();
    // Base name ("CPU [#0]: Ryzen 7") → node index.
    let mut bases: Vec<(String, usize)> = Vec::new();
    for i in 0..s_num.min(1024) {
        let at = s_off + i * s_size;
        let (id, inst) = (le_u32(b, at)?, le_u32(b, at + 4)?);
        // The original English name classifies, the user's name labels.
        let orig = c_str(b, at + 8, STR_LEN)?;
        let user = if s_utf8 { c_str(b, at + 264, STR_LEN)? } else { c_str(b, at + 136, STR_LEN)? };
        let user = if user.is_empty() { orig.clone() } else { user };
        let kind = hwinfo_kind(&orig);
        let key = format!("hwinfo_{id:x}_{inst}");
        let (base, sub) = hwinfo_split(&orig);
        let parent = bases.iter().find(|(b, _)| *b == base).map(|(_, n)| *n);
        let (name, parent) = match (sub, parent) {
            (Some(sub), Some(p)) => (sub, Some(hardware[p].key.clone())),
            _ => (strip_hwinfo_prefix(&user), None),
        };
        if parent.is_none() {
            bases.push((base, hardware.len()));
        }
        node_of.push(hardware.len());
        hardware.push(ExtHardware { key, name, kind, parent, sensors: Vec::new() });
    }
    for i in 0..r_num.min(8192) {
        let at = r_off + i * r_size;
        let ty = le_u32(b, at)?;
        let Some(&node) = node_of.get(le_u32(b, at + 4)? as usize) else { continue };
        let reading_id = le_u32(b, at + 8)?;
        let label = if r_utf8 { c_str(b, at + 316, STR_LEN)? } else { c_str(b, at + 140, STR_LEN)? };
        let label = if label.is_empty() { c_str(b, at + 12, STR_LEN)? } else { label };
        let unit = if r_utf8 { c_str(b, at + 444, 16)? } else { c_str(b, at + 268, 16)? };
        let Some(kind) = hwinfo_sensor_kind(ty, &unit) else { continue };
        let value = convert(kind, le_f64(b, at + 284)?, &unit);
        if !value.is_finite() || label.is_empty() {
            continue;
        }
        let h = &mut hardware[node];
        let id = format!("{}_{reading_id:x}", h.key);
        if h.sensors.iter().all(|s| s.id != id) {
            h.sensors.push(ExtSensor::new(id, label, kind, value));
        }
    }
    // Drop empty nodes that have no children with sensors either.
    let keep: Vec<bool> = (0..hardware.len())
        .map(|i| !hardware[i].sensors.is_empty() || hardware.iter().any(|c| c.parent.as_ref() == Some(&hardware[i].key) && !c.sensors.is_empty()))
        .collect();
    let mut k = keep.into_iter();
    hardware.retain(|_| k.next().unwrap_or(false));
    Some(ExtTree { source: "HWiNFO", hardware })
}

/// "CPU [#0]: AMD Ryzen 7 7800X3D" → "AMD Ryzen 7 7800X3D".
fn strip_hwinfo_prefix(name: &str) -> String {
    match name.split_once("]: ") {
        Some((head, rest)) if head.len() < 12 && head.contains('[') => rest.to_string(),
        _ => name.to_string(),
    }
}

// ---------------------------------------------------------------------------
// LibreHardwareMonitor.

/// Hardware kind from an LHM hardware identifier.
pub fn lhm_kind(identifier: &str) -> HardwareKind {
    let first = identifier.trim_start_matches('/').split('/').next().unwrap_or("");
    match first {
        "motherboard" => HardwareKind::Motherboard,
        "lpc" | "ec" => HardwareKind::Chip,
        "amdcpu" | "intelcpu" | "cpu" => HardwareKind::Cpu,
        "ram" | "vram" | "memory" => HardwareKind::Memory,
        "nvme" | "ssd" | "hdd" | "storage" => HardwareKind::Storage,
        "nic" => HardwareKind::Network,
        "battery" => HardwareKind::Battery,
        "psu" => HardwareKind::PowerSupply,
        f if f.starts_with("gpu") => HardwareKind::Gpu,
        "aquacomputer" | "nzxt" | "aeroCool" | "heatmaster" | "tbalancer" => HardwareKind::Cooler,
        _ => HardwareKind::Other,
    }
}

/// OpenDeviceControl kind and default unit of an LHM `SensorType`.
fn lhm_sensor_kind(ty: &str) -> Option<(SensorKind, &'static str)> {
    Some(match ty {
        "Temperature" => (SensorKind::Temperature, "°C"),
        "Voltage" => (SensorKind::Voltage, "V"),
        "Current" => (SensorKind::Current, "A"),
        "Power" => (SensorKind::Power, "W"),
        "Clock" => (SensorKind::Frequency, "MHz"),
        "Frequency" => (SensorKind::Frequency, "Hz"),
        "Load" | "Level" => (SensorKind::Load, "%"),
        "Control" => (SensorKind::Duty, "%"),
        "Fan" => (SensorKind::FanSpeed, "RPM"),
        "Flow" => (SensorKind::Flow, "L/h"),
        "Data" => (SensorKind::DataSize, "GB"),
        "SmallData" => (SensorKind::DataSize, "MB"),
        "Throughput" => (SensorKind::DataRate, "B/s"),
        "TimeSpan" => (SensorKind::Duration, "s"),
        _ => return None,
    })
}

/// A hardware row (WMI `Hardware`, or a `data.json` node with `HardwareId`).
pub struct LhmHardware {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
}

/// A sensor row (WMI `Sensor`, or a `data.json` node with `SensorId`).
pub struct LhmSensor {
    pub id: String,
    pub name: String,
    pub ty: String,
    pub value: f64,
    /// Unit when the source states it (`data.json`); else the type's default.
    pub unit: Option<String>,
    pub parent: String,
}

/// Tree from LHM's hardware and sensor rows.
pub fn lhm_tree(hardware: Vec<LhmHardware>, sensors: Vec<LhmSensor>) -> ExtTree {
    let key = |id: &str| format!("lhm_{}", slug(id));
    let mut out: Vec<ExtHardware> = hardware
        .iter()
        .map(|h| ExtHardware {
            key: key(&h.id),
            name: h.name.trim().to_string(),
            kind: lhm_kind(&h.id),
            parent: h.parent.as_deref().filter(|p| hardware.iter().any(|x| x.id == *p)).map(key),
            sensors: Vec::new(),
        })
        .collect();
    for s in sensors {
        let Some((kind, unit)) = lhm_sensor_kind(&s.ty) else { continue };
        let value = convert(kind, s.value, s.unit.as_deref().filter(|u| !u.is_empty()).unwrap_or(unit));
        let parent = key(&s.parent);
        let Some(h) = out.iter_mut().find(|h| h.key == parent) else { continue };
        if value.is_finite() && !s.name.trim().is_empty() {
            h.sensors.push(ExtSensor::new(key(&s.id), s.name.trim(), kind, value));
        }
    }
    ExtTree { source: "LibreHardwareMonitor", hardware: out }
}

/// Everything from LibreHardwareMonitor's web server `data.json`.
pub fn parse_lhm_json(json: &str) -> Option<ExtTree> {
    let root: serde_json::Value = serde_json::from_str(json).ok()?;
    let (mut hardware, mut sensors) = (Vec::new(), Vec::new());
    // (node, nearest hardware ancestor id)
    let mut stack: Vec<(&serde_json::Value, Option<String>)> = vec![(&root, None)];
    while let Some((n, parent)) = stack.pop() {
        let mut here = parent.clone();
        let text = n.get("Text").and_then(|v| v.as_str()).unwrap_or_default();
        if let Some(id) = n.get("HardwareId").and_then(|v| v.as_str()) {
            hardware.push(LhmHardware { id: id.into(), name: text.into(), parent: parent.clone() });
            here = Some(id.to_string());
        } else if let (Some(id), Some(ty), Some(p)) =
            (n.get("SensorId").and_then(|v| v.as_str()), n.get("Type").and_then(|v| v.as_str()), &parent)
        {
            // `RawValue` (base unit, e.g. "4749312.0 B/s" or a number) since
            // 0.9.5; older versions only have the formatted `Value`.
            let parsed = match n.get("RawValue") {
                Some(serde_json::Value::Number(x)) => x.as_f64().map(|v| (v, String::new())),
                Some(serde_json::Value::String(s)) => number_and_unit(s),
                _ => None,
            }
            .or_else(|| n.get("Value").and_then(|v| v.as_str()).and_then(number_and_unit));
            if let Some((value, unit)) = parsed {
                sensors.push(LhmSensor {
                    id: id.into(),
                    name: text.into(),
                    ty: ty.into(),
                    value,
                    unit: Some(unit),
                    parent: p.clone(),
                });
            }
        }
        if let Some(children) = n.get("Children").and_then(|c| c.as_array()) {
            stack.extend(children.iter().rev().map(|c| (c, here.clone())));
        }
    }
    Some(lhm_tree(hardware, sensors))
}

/// Body of an HTTP/1.x response, if its status is 200.
pub fn http_body(resp: &[u8]) -> Option<&[u8]> {
    let head_end = resp.windows(4).position(|w| w == b"\r\n\r\n")?;
    let status = std::str::from_utf8(&resp[..head_end]).ok()?.lines().next()?;
    (status.split_whitespace().nth(1) == Some("200")).then(|| &resp[head_end + 4..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amd_decoding() {
        // 45.0 °C, no range select: 45 / 0.125 = 360.
        assert_eq!(amd_tctl(360 << 21), 45.0);
        // Range select set: raw 94 °C - 49.
        assert_eq!(amd_tctl((752 << 21) | (1 << 19)), 45.0);
        assert_eq!(amd_tctl((752 << 21) | (0b11 << 16)), 45.0);
        assert_eq!(amd_ccd((1 << 11) | 752), Some(45.0));
        assert_eq!(amd_ccd(752), None);
        assert_eq!(amd_ccd_layout(0x19, 0x61), Some((0x308, 8))); // Ryzen 7000 (Raphael)
        assert_eq!(amd_ccd_layout(0x19, 0x21), Some((0x154, 8))); // Ryzen 5000 (Vermeer)
        assert_eq!(amd_ccd_layout(0x17, 0x08), None);
        assert_eq!(amd_tctl_offset("AMD Ryzen 7 1800X Eight-Core Processor"), 20.0);
        assert_eq!(amd_tctl_offset("AMD Ryzen 7 7800X3D 8-Core Processor"), 0.0);
        let t = amd_tree("Ryzen", 360 << 21, 0.0, &[(0, (1 << 11) | 760), (1, 0)]);
        let s = &t.hardware[0].sensors;
        assert_eq!(s.len(), 2);
        assert_eq!((s[1].id.as_str(), s[1].label.as_str(), s[1].value), ("pawnio_cpu_ccd1", "CPU CCD1 (Tdie)", 46.0));
        assert_eq!(t.readings().len(), 2);
        assert!(t.readings()[0].is_cpu_temperature());
        // Garbage (all ones: 255.875 - 49 °C) is rejected.
        assert!(amd_tree("Ryzen", u32::MAX, 0.0, &[]).hardware.is_empty());
    }

    #[test]
    fn intel_decoding() {
        // TjMax 100, 38 below → 62 °C.
        assert_eq!(intel_package((1 << 31) | (38 << 16), 100 << 16), Some(62.0));
        assert_eq!(intel_package((1 << 31) | (38 << 16), 0), Some(62.0));
        assert_eq!(intel_package(38 << 16, 100 << 16), None);
        assert_eq!(intel_tree("i7", Some(62.0)).readings()[0].celsius, 62.0);
    }

    #[test]
    fn numbers_and_units() {
        assert_eq!(number_and_unit("45,3 °C"), Some((45.3, "°C".into())));
        assert_eq!(number_and_unit("4749312.0 B/s"), Some((4749312.0, "B/s".into())));
        assert_eq!(number_and_unit("1545 RPM"), Some((1545.0, "RPM".into())));
        assert_eq!(number_and_unit("-"), None);
        assert_eq!(number_and_unit(" A"), None);
        assert_eq!(convert(SensorKind::DataRate, 1048576.0, "B/s"), 1.0);
        assert_eq!(convert(SensorKind::DataSize, 2048.0, "MB"), 2.0);
        assert_eq!(convert(SensorKind::Temperature, 212.0, "°F"), 100.0);
        assert_eq!(slug("/lpc/nct6799d/0"), "lpc_nct6799d_0");
    }

    #[test]
    fn labels() {
        let r = |l: &str| Reading::new("x", Area::Cpu, l, 50.0);
        assert!(!r("Core #1 Distance to TjMax").is_cpu_temperature());
        assert!(r("CPU Package").is_cpu_temperature());
        assert!(!Reading::new("x", Area::Board, "CPU", 50.0).is_cpu_temperature());
    }

    fn put_str(b: &mut [u8], at: usize, s: &str) {
        b[at..at + s.len()].copy_from_slice(s.as_bytes());
    }

    /// A version-2 HWiNFO mapping: sensors (name), readings (type, sensor, label, unit, value).
    fn hwinfo_blob(sensors: &[&str], readings: &[(u32, u32, &str, &str, f64)]) -> Vec<u8> {
        let (s_size, r_size) = (392usize, 460usize);
        let s_off = 48;
        let r_off = s_off + s_size * sensors.len();
        let mut b = vec![0u8; r_off + r_size * readings.len()];
        b[0..4].copy_from_slice(b"HWiS");
        b[4..8].copy_from_slice(&2u32.to_le_bytes());
        for (at, v) in [(20, s_off), (24, s_size), (28, sensors.len()), (32, r_off), (36, r_size), (40, readings.len())] {
            b[at..at + 4].copy_from_slice(&(v as u32).to_le_bytes());
        }
        for (i, name) in sensors.iter().enumerate() {
            let at = s_off + i * s_size;
            b[at..at + 4].copy_from_slice(&(0xf000 + i as u32).to_le_bytes());
            put_str(&mut b, at + 8, name);
            put_str(&mut b, at + 136, name);
            put_str(&mut b, at + 264, name);
        }
        for (i, (ty, sensor, label, unit, value)) in readings.iter().enumerate() {
            let at = r_off + i * r_size;
            b[at..at + 4].copy_from_slice(&ty.to_le_bytes());
            b[at + 4..at + 8].copy_from_slice(&sensor.to_le_bytes());
            b[at + 8..at + 12].copy_from_slice(&(0x100 + i as u32).to_le_bytes());
            put_str(&mut b, at + 12, label);
            put_str(&mut b, at + 316, label);
            put_str(&mut b, at + 444, unit);
            b[at + 284..at + 292].copy_from_slice(&value.to_le_bytes());
        }
        b
    }

    #[test]
    fn hwinfo_parsing() {
        let b = hwinfo_blob(
            &[
                "CPU [#0]: AMD Ryzen 7 7800X3D",
                "CPU [#0]: AMD Ryzen 7 7800X3D: Enhanced",
                "ASUS ROG STRIX B650E-F (Nuvoton NCT6799D)",
                "GPU [#0]: NVIDIA GeForce RTX 4080",
                "Windows Hardware Errors (WHEA)",
            ],
            &[
                (6, 0, "Core 0 Clock", "MHz", 5000.0),
                (1, 1, "CPU (Tctl/Tdie)", "°C", 61.5),
                (2, 1, "CPU Core Voltage", "V", 1.1),
                (1, 2, "Motherboard", "°F", 104.0),
                (3, 2, "CPU Fan", "RPM", 900.0),
                (1, 3, "GPU Temperature", "°C", 50.0),
                (8, 3, "GPU D3D Memory Dynamic", "MB", 2048.0),
                (8, 3, "GPU Performance Limit", "Yes/No", 1.0),
                (1, 7, "Bad sensor index", "°C", 50.0),
            ],
        );
        assert_eq!(hwinfo_size(&b), Some(b.len()));
        let t = parse_hwinfo(&b).unwrap();
        let nodes: Vec<(&str, HardwareKind, Option<&str>, usize)> =
            t.hardware.iter().map(|h| (h.name.as_str(), h.kind, h.parent.as_deref(), h.sensors.len())).collect();
        assert_eq!(
            nodes,
            vec![
                ("AMD Ryzen 7 7800X3D", HardwareKind::Cpu, None, 1),
                ("Enhanced", HardwareKind::Cpu, Some("hwinfo_f000_0"), 2),
                ("ASUS ROG STRIX B650E-F (Nuvoton NCT6799D)", HardwareKind::Chip, None, 2),
                ("NVIDIA GeForce RTX 4080", HardwareKind::Gpu, None, 2),
            ]
        );
        let board = &t.hardware[2].sensors[0];
        assert!((board.value - 40.0).abs() < 1e-9);
        assert_eq!(t.hardware[3].sensors[1].value, 2.0); // MB → GB
        let r = t.readings();
        assert_eq!(r.iter().filter(|r| r.is_cpu_temperature()).count(), 1);
        assert_eq!(r.iter().find(|r| r.area == Area::Board).unwrap().label, "Motherboard");
        // Inactive or truncated memory.
        let mut dead = b.clone();
        dead[0..4].copy_from_slice(b"DEAD");
        assert_eq!(parse_hwinfo(&dead), None);
        assert_eq!(parse_hwinfo(&b[..b.len() - 10]), None);
        assert_eq!(parse_hwinfo(&[]), None);
    }

    #[test]
    fn lhm_parsing() {
        let json = r#"{"id":0,"Text":"Sensor","Children":[{"id":1,"Text":"PC","Children":[
            {"id":11,"Text":"ASRock B650E","HardwareId":"/motherboard","Children":[
              {"id":7,"Text":"Nuvoton NCT6799D","HardwareId":"/lpc/nct6799d/0","Children":[
                {"id":12,"Text":"Temperatures","Children":[
                  {"id":8,"Text":"System","SensorId":"/lpc/nct6799d/0/temperature/0","Type":"Temperature","Value":"35.0 °C","Children":[]}]},
                {"id":13,"Text":"Fans","Children":[
                  {"id":14,"Text":"Fan #1","SensorId":"/lpc/nct6799d/0/fan/0","Type":"Fan","Value":"1545 RPM","RawValue":"1545 RPM","Children":[]}]}]}]},
            {"id":2,"Text":"AMD Ryzen 7 7800X3D","HardwareId":"/amdcpu/0","Children":[
              {"id":3,"Text":"Temperatures","Children":[
                {"id":4,"Text":"Core (Tctl/Tdie)","SensorId":"/amdcpu/0/temperature/2","Type":"Temperature","Value":"61,5 °C","RawValue":61.5,"Children":[]},
                {"id":5,"Text":"CCD1 (Tdie)","SensorId":"/amdcpu/0/temperature/3","Type":"Temperature","Value":"130.1 °F","Children":[]},
                {"id":6,"Text":"Package","SensorId":"/amdcpu/0/power/0","Type":"Power","Value":"40 W","RawValue":"40.0 W","Children":[]},
                {"id":15,"Text":"Core #1","SensorId":"/amdcpu/0/factor/0","Type":"Factor","Value":"49.5","Children":[]}]}]},
            {"id":9,"Text":"RTX 4080","HardwareId":"/gpu-nvidia/0","Children":[
                {"id":10,"Text":"GPU PCIe Rx","SensorId":"/gpu-nvidia/0/throughput/0","Type":"Throughput","Value":"4.5 MB/s","RawValue":"4718592.0 B/s","Children":[]},
                {"id":16,"Text":"GPU Memory Used","SensorId":"/gpu-nvidia/0/smalldata/1","Type":"SmallData","Value":"1579.0 MB","RawValue":"1536.0 MB","Children":[]},
                {"id":17,"Text":"+12V","SensorId":"/gpu-nvidia/0/current/7","Type":"Current","Value":"-","RawValue":" A","Children":[]}]}
        ]}]}"#;
        let t = parse_lhm_json(json).unwrap();
        let nodes: Vec<(&str, HardwareKind, Option<&str>)> =
            t.hardware.iter().map(|h| (h.key.as_str(), h.kind, h.parent.as_deref())).collect();
        assert_eq!(
            nodes,
            vec![
                ("lhm_motherboard", HardwareKind::Motherboard, None),
                ("lhm_lpc_nct6799d_0", HardwareKind::Chip, Some("lhm_motherboard")),
                ("lhm_amdcpu_0", HardwareKind::Cpu, None),
                ("lhm_gpu_nvidia_0", HardwareKind::Gpu, None),
            ]
        );
        let cpu: Vec<(&str, f64)> = t.hardware[2].sensors.iter().map(|s| (s.label.as_str(), (s.value * 10.0).round() / 10.0)).collect();
        assert_eq!(cpu, vec![("Core (Tctl/Tdie)", 61.5), ("CCD1 (Tdie)", 54.5), ("Package", 40.0)]);
        assert_eq!(t.hardware[1].sensors[1].kind, SensorKind::FanSpeed);
        let gpu: Vec<f64> = t.hardware[3].sensors.iter().map(|s| s.value).collect();
        assert_eq!(gpu, vec![4.5, 1.5]);
        assert_eq!(t.hardware[2].sensors[0].id, "lhm_amdcpu_0_temperature_2");
        let r = t.readings();
        assert_eq!(r.iter().filter(|r| r.is_cpu_temperature()).count(), 2);
        assert_eq!(parse_lhm_json("not json"), None);
    }

    #[test]
    fn http_bodies() {
        assert_eq!(http_body(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"), Some(&b"{}"[..]));
        assert_eq!(http_body(b"HTTP/1.1 401 Unauthorized\r\n\r\nno"), None);
        assert_eq!(http_body(b"garbage"), None);
    }
}
