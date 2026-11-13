//! ASUS motherboards' embedded-controller sensors (chipset, VRM, T_Sensor
//! and water temperatures, extra fans, water flow, CPU current…), which
//! the Super I/O chip doesn't see. Each board model has its own set; the
//! table is LibreHardwareMonitor's (`EmbeddedController.cs`, MPL-2.0),
//! which the Linux `asus-ec-sensors` driver also follows.
//!
//! Registers are 16-bit: EC bank in the high byte (selected through EC
//! register 0xFF), offset in the low byte. Values are 1 byte (temperatures
//! signed) or 2 bytes (big-endian unless noted, signed).

use super::external::{ExtHardware, ExtSensor};
use crate::model::{HardwareKind, SensorKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum S {
    TempChipset,
    TempCpu,
    TempCpuPackage,
    TempMb,
    TempTSensor,
    TempTSensorAlt,
    TempTSensor2,
    TempVrm,
    TempVrmSoc,
    TempDimm,
    TempWaterIn,
    TempWaterOut,
    TempWaterBlockIn,
    VoltageCpu,
    VoltageVrmCore,
    VoltageVrmSoc,
    PowerVrmCore,
    PowerVrmSoc,
    FanCpuOpt,
    FanVrmHs,
    FanChipset,
    FanWaterPump,
    FanWaterFlow,
    CurrCpu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Amd400,
    Amd500,
    Amd600,
    Amd800,
    Intel100,
    Intel300,
    Intel370,
    Intel400,
    Intel600,
    Intel700,
    Intel800,
}

/// One sensor's register: (address, size, little-endian).
fn register(f: Family, s: S) -> Option<(u16, u8, bool)> {
    use Family::*;
    use S::*;
    let one = |a| Some((a, 1, false));
    let two = |a| Some((a, 2, false));
    match (f, s) {
        (Amd400 | Amd500 | Intel100 | Intel300 | Intel370 | Intel400, TempChipset) => one(0x3A),
        (Amd400 | Amd500, TempCpu) => one(0x3B),
        (Amd400 | Amd500, TempMb) => one(0x3C),
        (Amd400 | Amd500 | Intel100 | Intel300 | Intel370 | Intel400 | Intel600, TempTSensor) => one(0x3D),
        (Amd400 | Amd500 | Intel100 | Intel300 | Intel400 | Intel600, TempVrm) => one(0x3E),
        (Amd400 | Amd500 | Intel100, VoltageCpu) => two(0xA2),
        (Amd400, FanCpuOpt) | (Intel370, FanCpuOpt) => two(0xBC),
        (Amd500 | Amd600 | Amd800 | Intel300 | Intel400 | Intel700, FanCpuOpt) => two(0xB0),
        (Amd400 | Amd500, FanVrmHs) => two(0xB2),
        (Amd400, FanWaterFlow) => two(0xB4),
        (Amd500, FanChipset) => two(0xB4),
        (Amd500 | Intel300 | Intel700, FanWaterFlow) => two(0xBC),
        (Intel400 | Intel600, FanWaterFlow) => two(0xBE),
        (Intel100, FanWaterPump) => two(0xBC),
        (Intel370, FanWaterPump) => two(0xBE),
        (Amd400 | Amd500 | Intel100 | Intel400, CurrCpu) => one(0xF4),
        (Amd400, TempWaterIn) => one(0x10D),
        (Amd400, TempWaterOut) => one(0x10B),
        (Amd500 | Amd600 | Intel300 | Intel400 | Intel600 | Intel700, TempWaterIn) => one(0x100),
        (Amd500 | Amd600 | Intel300 | Intel400 | Intel600 | Intel700, TempWaterOut) => one(0x101),
        (Intel600, TempWaterBlockIn) => one(0x102),
        (Amd800, TempCpu) => one(0x30),
        (Amd800, TempCpuPackage) => one(0x31),
        (Amd800, TempMb) => one(0x32),
        (Amd800 | Intel700 | Intel800, TempVrm) => one(0x33),
        (Amd800, TempDimm) => one(0x55),
        (Amd800, TempVrmSoc) => one(0x418),
        (Amd800, VoltageVrmCore) => two(0x15A),
        (Amd800, VoltageVrmSoc) => Some((0x41A, 2, true)),
        (Amd800, PowerVrmCore) => two(0x15C),
        (Amd800, PowerVrmSoc) => Some((0x41E, 2, true)),
        (Amd800, TempTSensor) => one(0x36),
        (Amd800, TempTSensorAlt) => one(0x35),
        (Intel700, TempTSensor) => one(0x109),
        (Intel700, TempTSensor2) => one(0x105),
        (Intel800, TempTSensor) => one(0x10C),
        _ => None,
    }
}

/// (id, label, kind, factor, blank raw value).
fn describe(f: Family, s: S) -> (&'static str, &'static str, SensorKind, f64, Option<i32>) {
    use SensorKind::*;
    use S::*;
    let t = |id, label| (id, label, Temperature, 1.0, Some(-40));
    match s {
        TempChipset => ("chipset", "Chipset", Temperature, 1.0, None),
        TempCpu => ("cpu", "CPU", Temperature, 1.0, None),
        TempCpuPackage => ("cpu_package", "CPU package", Temperature, 1.0, None),
        TempMb => ("motherboard", "Motherboard", Temperature, 1.0, None),
        TempTSensor => t("t_sensor", "T_Sensor"),
        TempTSensorAlt => t("t_sensor_alt", "T_Sensor (alternate)"),
        TempTSensor2 => t("t_sensor2", "T_Sensor 2"),
        TempVrm => ("vrm", "VRM", Temperature, 1.0, None),
        TempVrmSoc => ("vrm_soc", "VRM SoC", Temperature, 1.0, None),
        TempDimm => ("dimm", "DIMM", Temperature, 1.0, None),
        TempWaterIn => t("water_in", "Water in"),
        TempWaterOut => t("water_out", "Water out"),
        TempWaterBlockIn => t("water_block_in", "Water block in"),
        VoltageCpu => ("cpu_voltage", "CPU core", Voltage, 1e-3, None),
        VoltageVrmCore => ("vrm_core_voltage", "VRM core", Voltage, 0.000125, None),
        VoltageVrmSoc => ("vrm_soc_voltage", "VRM SoC", Voltage, 0.001, None),
        PowerVrmCore => ("vrm_core_power", "VRM core", Power, 0.125, None),
        PowerVrmSoc => ("vrm_soc_power", "VRM SoC", Power, 1.0, None),
        FanCpuOpt => ("cpu_opt", "CPU_OPT fan", FanSpeed, 1.0, None),
        FanVrmHs => ("vrm_fan", "VRM heatsink fan", FanSpeed, 1.0, None),
        FanChipset => ("chipset_fan", "Chipset fan", FanSpeed, 1.0, None),
        FanWaterPump => ("water_pump", "Water pump", FanSpeed, 1.0, None),
        // Pulses → L/h (Intel 700 boards report per minute already).
        FanWaterFlow => ("water_flow", "Water flow", Flow, if f == Family::Intel700 { 1.0 / 42.0 } else { 60.0 / 42.0 }, None),
        CurrCpu => ("cpu_current", "CPU", Current, 1.0, None),
    }
}

/// The board's sensors, by its normalised product name (see [`board_key`]).
fn board(key: &str) -> Option<(Family, &'static [S])> {
    use Family::*;
    use S::*;
    const C8_HERO: &[S] =
        &[TempChipset, TempCpu, TempMb, TempTSensor, TempVrm, TempWaterIn, TempWaterOut, FanCpuOpt, FanChipset, FanWaterFlow, CurrCpu, VoltageCpu];
    const X870_FULL: &[S] = &[TempCpu, TempCpuPackage, TempMb, TempVrm, TempTSensor, FanCpuOpt];
    const X670E: &[S] = &[TempWaterIn, TempWaterOut, FanCpuOpt];
    const XI_FORMULA: &[S] = &[TempVrm, TempChipset, TempWaterIn, TempWaterOut, TempTSensor, FanCpuOpt];
    Some(match key {
        "ROG_STRIX_B850_E_GAMING_WIFI" => (Amd800, &[TempCpuPackage, TempVrm, TempTSensorAlt]),
        "ROG_CROSSHAIR_X870E_APEX" => (Amd800, &[TempVrm, TempVrmSoc, TempDimm, VoltageVrmCore, VoltageVrmSoc, PowerVrmCore, PowerVrmSoc, FanCpuOpt]),
        "ROG_CROSSHAIR_X870E_DARK_HERO" => (Amd800, &[TempTSensor]),
        "ROG_CROSSHAIR_X870E_HERO" | "ROG_CROSSHAIR_X870E_HERO_BTF" => (Amd800, X870_FULL),
        "TUF_GAMING_X870_PLUS_WIFI" => (Amd800, &[TempVrm, FanCpuOpt]),
        "ROG_STRIX_X870E_E_GAMING_WIFI" | "ROG_STRIX_X870E_H_GAMING_WIFI7" => (Amd800, X870_FULL),
        "ROG_STRIX_X870_F_GAMING_WIFI" => (Amd800, &[TempCpu, TempCpuPackage, TempMb, TempVrm, TempTSensor]),
        "ROG_STRIX_X870_I_GAMING_WIFI" => (Amd800, &[TempCpu, TempCpuPackage, TempMb, TempVrm]),
        "PRIME_X470_PRO" => (Amd400, &[TempChipset, TempCpu, TempMb, TempVrm, FanCpuOpt, CurrCpu, VoltageCpu]),
        "PRIME_X570_PRO" => (Amd500, &[TempChipset, TempCpu, TempMb, TempTSensor, TempVrm, FanChipset]),
        "PROART_X570_CREATOR_WIFI" => (Amd500, &[TempChipset, TempCpu, TempMb, TempTSensor, TempVrm, FanCpuOpt, CurrCpu, VoltageCpu]),
        "PRO_WS_X570_ACE" => (Amd500, &[TempChipset, TempCpu, TempMb, TempVrm, FanChipset, CurrCpu, VoltageCpu]),
        "ROG_CROSSHAIR_VIII_HERO" | "ROG_CROSSHAIR_VIII_HERO_WIFI" | "ROG_CROSSHAIR_VIII_FORMULA" => (Amd500, C8_HERO),
        "ROG_CROSSHAIR_X670E_EXTREME" | "ROG_CROSSHAIR_X670E_HERO" | "ROG_CROSSHAIR_X670E_GENE" => (Amd600, X670E),
        "ROG_STRIX_X670E_E_GAMING_WIFI" | "ROG_STRIX_X670E_F_GAMING_WIFI" => (Amd600, X670E),
        "ROG_CROSSHAIR_VIII_DARK_HERO" => {
            (Amd500, &[TempChipset, TempCpu, TempMb, TempTSensor, TempVrm, TempWaterIn, TempWaterOut, FanCpuOpt, FanWaterFlow, CurrCpu, VoltageCpu])
        }
        "ROG_CROSSHAIR_VIII_IMPACT" => (Amd500, &[TempChipset, TempCpu, TempMb, TempTSensor, TempVrm, FanChipset, CurrCpu, VoltageCpu]),
        "ROG_STRIX_B550_E_GAMING" => (Amd500, &[TempChipset, TempCpu, TempMb, TempTSensor, TempVrm, FanCpuOpt]),
        "ROG_STRIX_B550_I_GAMING" => (Amd500, &[TempChipset, TempCpu, TempMb, TempTSensor, TempVrm, FanVrmHs, CurrCpu, VoltageCpu]),
        "ROG_STRIX_X570_E_GAMING" => (Amd500, &[TempChipset, TempCpu, TempMb, TempTSensor, TempVrm, FanChipset, CurrCpu, VoltageCpu]),
        "ROG_STRIX_X570_E_GAMING_WIFI_II" => (Amd500, &[TempChipset, TempCpu, TempMb, TempTSensor, TempVrm]),
        "ROG_STRIX_X570_F_GAMING" => (Amd500, &[TempChipset, TempCpu, TempMb, TempTSensor, FanChipset]),
        "ROG_STRIX_X570_I_GAMING" => (Amd500, &[TempTSensor, FanVrmHs, FanChipset, CurrCpu, VoltageCpu, TempChipset, TempVrm]),
        "ROG_STRIX_Z370_G_GAMING" => (Intel370, &[TempChipset, TempTSensor, FanCpuOpt, FanWaterPump]),
        "ROG_STRIX_Z390_E_GAMING" | "ROG_STRIX_Z390_F_GAMING" => (Intel300, &[TempVrm, TempChipset, TempTSensor, FanCpuOpt]),
        "ROG_STRIX_Z390_I_GAMING" => (Intel300, &[TempVrm, TempChipset, TempTSensor]),
        "ROG_MAXIMUS_XI_FORMULA" => (Intel300, XI_FORMULA),
        "ROG_MAXIMUS_XI_HERO" | "ROG_MAXIMUS_XI_HERO_WIFI" => {
            (Intel300, &[TempVrm, TempChipset, TempWaterIn, TempWaterOut, TempTSensor, FanCpuOpt, FanWaterFlow])
        }
        "ROG_MAXIMUS_XII_Z490_FORMULA" | "ROG_MAXIMUS_XII_FORMULA" => (Intel400, &[TempTSensor, TempVrm, TempWaterIn, TempWaterOut, FanWaterFlow]),
        "ROG_MAXIMUS_XII_HERO_WIFI" => {
            (Intel400, &[TempTSensor, TempChipset, TempVrm, TempWaterIn, TempWaterOut, CurrCpu, FanCpuOpt, FanWaterFlow])
        }
        "ROG_STRIX_Z690_A_GAMING_WIFI_D4" | "ROG_STRIX_Z690_G_GAMING_WIFI" | "PRIME_Z690_A" => (Intel600, &[TempTSensor, TempVrm]),
        "ROG_MAXIMUS_Z690_HERO" => (Intel600, &[TempTSensor, TempWaterIn, TempWaterOut, FanWaterFlow]),
        "ROG_MAXIMUS_Z690_FORMULA" => (Intel600, &[TempTSensor, TempVrm, TempWaterIn, TempWaterOut, TempWaterBlockIn, FanWaterFlow]),
        "ROG_MAXIMUS_Z690_EXTREME_GLACIAL" => (Intel600, &[TempVrm, TempWaterIn, TempWaterOut, TempWaterBlockIn, FanWaterFlow]),
        "ROG_MAXIMUS_Z790_HERO" | "ROG_MAXIMUS_Z790_DARK_HERO" => {
            (Intel700, &[TempVrm, TempTSensor, TempWaterIn, TempWaterOut, FanWaterFlow, FanCpuOpt])
        }
        "ROG_MAXIMUS_Z790_FORMULA" => (Intel700, &[TempWaterIn, TempWaterOut]),
        "ROG_STRIX_Z790_I_GAMING_WIFI" => (Intel700, &[TempTSensor, TempTSensor2]),
        "ROG_STRIX_Z790_E_GAMING_WIFI" => (Intel700, &[TempWaterIn]),
        "ROG_STRIX_Z790_E_GAMING_WIFI_II" => (Intel700, &[TempTSensor, TempVrm, FanCpuOpt]),
        "ROG_STRIX_Z890_E_GAMING_WIFI" => (Intel800, &[TempTSensor, TempVrm]),
        "Z170_A" => (Intel100, &[TempTSensor, TempChipset, FanWaterPump, CurrCpu, VoltageCpu]),
        "Z170_PRO_GAMING" => (Intel100, &[TempChipset, TempVrm, TempTSensor]),
        _ => return None,
    })
}

/// "ROG CROSSHAIR VIII HERO (WI-FI)" → "ROG_CROSSHAIR_VIII_HERO_WIFI".
pub fn board_key(product: &str) -> String {
    let up = product.to_uppercase().replace("WI-FI", "WIFI");
    up.split(|c: char| !c.is_ascii_alphanumeric()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("_")
}

/// The EC registers to read for an ASUS board, or `None` if it has none
/// OpenDeviceControl knows. Sorted by address (so by bank).
pub struct Board {
    family: Family,
    sensors: &'static [S],
}

impl Board {
    pub fn find(manufacturer: &str, product: &str) -> Option<Board> {
        if !manufacturer.to_uppercase().contains("ASUS") {
            return None;
        }
        let (family, sensors) = board(&board_key(product))?;
        Some(Board { family, sensors })
    }

    /// Every register byte to read, sorted.
    pub fn registers(&self) -> Vec<u16> {
        let mut out: Vec<u16> = self
            .sensors
            .iter()
            .filter_map(|s| register(self.family, *s))
            .flat_map(|(a, n, _)| (0..u16::from(n)).map(move |i| a + i))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Decode the bytes read (by register) into sensors.
    pub fn decode(&self, read: &dyn Fn(u16) -> Option<u8>) -> ExtHardware {
        let mut sensors = Vec::new();
        for s in self.sensors {
            let Some((addr, size, le)) = register(self.family, *s) else { continue };
            let (id, label, kind, factor, blank) = describe(self.family, *s);
            let raw = match size {
                1 => {
                    let Some(b) = read(addr) else { continue };
                    if kind == SensorKind::Temperature { i32::from(b as i8) } else { i32::from(b) }
                }
                _ => {
                    let (Some(a), Some(b)) = (read(addr), read(addr + 1)) else { continue };
                    let (hi, lo) = if le { (b, a) } else { (a, b) };
                    i32::from(i16::from_be_bytes([hi, lo]))
                }
            };
            if blank == Some(raw) {
                continue;
            }
            let v = f64::from(raw) * factor;
            sensors.push(ExtSensor::new(format!("asusec_{id}"), label, kind, (v * 1000.0).round() / 1000.0));
        }
        ExtHardware { key: "asus_ec".into(), name: "ASUS EC".into(), kind: HardwareKind::Chip, parent: None, sensors }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn crosshair_viii() {
        assert_eq!(board_key("ROG CROSSHAIR VIII HERO (WI-FI)"), "ROG_CROSSHAIR_VIII_HERO_WIFI");
        let b = Board::find("ASUSTeK COMPUTER INC.", "ROG CROSSHAIR VIII HERO (WI-FI)").unwrap();
        assert!(Board::find("ASRock", "ROG CROSSHAIR VIII HERO").is_none());
        let mut regs: HashMap<u16, u8> = HashMap::new();
        regs.insert(0x3A, 52); // chipset
        regs.insert(0x3D, (-40i8) as u8); // T_Sensor not connected
        regs.insert(0xA2, 0x04); // CPU core 1.250 V
        regs.insert(0xA3, 0xE2);
        regs.insert(0xBC, 0x00); // 84 pulses → 120 L/h
        regs.insert(0xBD, 84);
        let hw = b.decode(&|r| Some(*regs.get(&r).unwrap_or(&0)));
        let v = |id: &str| hw.sensors.iter().find(|s| s.id == id).map(|s| s.value);
        assert_eq!(v("asusec_chipset"), Some(52.0));
        assert_eq!(v("asusec_t_sensor"), None);
        assert_eq!(v("asusec_cpu_voltage"), Some(1.25));
        assert_eq!(v("asusec_water_flow"), Some(120.0));
        assert!(b.registers().windows(2).all(|w| w[0] < w[1]));
    }
}
