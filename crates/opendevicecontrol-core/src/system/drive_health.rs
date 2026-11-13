//! Drive health: the NVMe SMART / health log page and ATA SMART
//! attributes, decoded into sensors. Pure parsing, tested on every
//! platform; the Windows sensor service reads the raw data through the
//! storage IOCTLs (`system::windows_drives`). Linux gets drive
//! temperatures from the `nvme` / `drivetemp` hwmon drivers.

use super::external::ExtSensor;
use crate::model::SensorKind;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

fn le(b: &[u8], at: usize, n: usize) -> u128 {
    b.get(at..at + n).map_or(0, |s| s.iter().rev().fold(0u128, |acc, &x| (acc << 8) | u128::from(x)))
}

/// NVMe log page 02h (512 bytes) → sensors. `key` prefixes their ids.
pub fn nvme_health(key: &str, log: &[u8]) -> Vec<ExtSensor> {
    let mut out = Vec::new();
    if log.len() < 512 {
        return out;
    }
    let id = |s: &str| format!("{key}_{s}");
    // Composite temperature, kelvin.
    let k = le(log, 1, 2) as f64;
    if k > 273.0 && k < 273.0 + 125.0 {
        out.push(ExtSensor::new(id("temp"), "Temperature", SensorKind::Temperature, k - 273.0));
    }
    // Further sensors (bytes 200–215), when the drive has them.
    for i in 0..8 {
        let k = le(log, 200 + i * 2, 2) as f64;
        if k > 273.0 && k < 273.0 + 125.0 {
            out.push(ExtSensor::new(id(&format!("temp{}", i + 2)), format!("Temperature #{}", i + 2), SensorKind::Temperature, k - 273.0));
        }
    }
    let spare = log[3];
    if spare <= 100 {
        let mut s = ExtSensor::new(id("spare"), "Available spare", SensorKind::Load, f64::from(spare));
        s.diagnostic = true;
        out.push(s);
    }
    let used = log[5];
    let mut life = ExtSensor::new(id("life"), "Life remaining", SensorKind::Load, f64::from(100u8.saturating_sub(used)));
    life.diagnostic = true;
    out.push(life);
    // Data units: thousands of 512-byte blocks.
    let units = |at: usize| le(log, at, 16) as f64 * 512_000.0 / GIB;
    let mut r = ExtSensor::new(id("data_read"), "Data read", SensorKind::DataSize, (units(32) * 10.0).round() / 10.0);
    r.diagnostic = true;
    out.push(r);
    let mut w = ExtSensor::new(id("data_written"), "Data written", SensorKind::DataSize, (units(48) * 10.0).round() / 10.0);
    w.diagnostic = true;
    out.push(w);
    let hours = le(log, 128, 16) as f64;
    let mut h = ExtSensor::new(id("power_on"), "Power-on time", SensorKind::Duration, hours * 3600.0);
    h.diagnostic = true;
    out.push(h);
    out
}

/// One ATA SMART attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attribute {
    pub id: u8,
    /// Normalised value (usually 100 or 200 = new).
    pub current: u8,
    pub raw: u64,
}

/// The 30 attribute entries of the 512-byte SMART data.
pub fn ata_attributes(data: &[u8]) -> Vec<Attribute> {
    let mut out = Vec::new();
    for i in 0..30 {
        let at = 2 + i * 12;
        let Some(e) = data.get(at..at + 12) else { break };
        if e[0] == 0 {
            continue;
        }
        out.push(Attribute { id: e[0], current: e[3], raw: le(e, 5, 6) as u64 });
    }
    out
}

/// ATA SMART attributes → sensors.
pub fn ata_health(key: &str, attrs: &[Attribute]) -> Vec<ExtSensor> {
    let mut out = Vec::new();
    let id = |s: &str| format!("{key}_{s}");
    let get = |n: u8| attrs.iter().find(|a| a.id == n);
    // 194: temperature (raw low byte); 190 on some drives.
    if let Some(t) = get(194).or_else(|| get(190)).map(|a| (a.raw & 0xFF) as f64).filter(|t| *t > 0.0 && *t < 125.0) {
        out.push(ExtSensor::new(id("temp"), "Temperature", SensorKind::Temperature, t));
    }
    // SSD life left: 231 (most), 177 (Samsung wear levelling), 202
    // (Micron / Crucial), 233 (Intel media wearout) — normalised value.
    if let Some(a) = [231u8, 177, 202, 233].iter().find_map(|&n| get(n)).filter(|a| a.current <= 100) {
        let mut s = ExtSensor::new(id("life"), "Life remaining", SensorKind::Load, f64::from(a.current));
        s.diagnostic = true;
        out.push(s);
    }
    if let Some(a) = get(9) {
        // Power-on hours: low 32 bits (some drives put more in the rest).
        let mut s = ExtSensor::new(id("power_on"), "Power-on time", SensorKind::Duration, (a.raw & 0xFFFF_FFFF) as f64 * 3600.0);
        s.diagnostic = true;
        out.push(s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvme_log() {
        let mut log = vec![0u8; 512];
        log[1..3].copy_from_slice(&(273u16 + 41).to_le_bytes());
        log[3] = 100;
        log[5] = 3;
        // 2,097,152 data units written = 1,000 GiB... per 512,000 bytes.
        log[48..56].copy_from_slice(&2_097_152u64.to_le_bytes());
        log[128..136].copy_from_slice(&1500u64.to_le_bytes());
        let s = nvme_health("nvme0", &log);
        let v = |id: &str| s.iter().find(|x| x.id == id).map(|x| x.value);
        assert_eq!(v("nvme0_temp"), Some(41.0));
        assert_eq!(v("nvme0_life"), Some(97.0));
        assert_eq!(v("nvme0_power_on"), Some(1500.0 * 3600.0));
        assert_eq!(v("nvme0_data_written"), Some(1000.0));
    }

    #[test]
    fn ata() {
        let mut data = vec![0u8; 512];
        let mut put = |slot: usize, id: u8, cur: u8, raw: u64| {
            let at = 2 + slot * 12;
            data[at] = id;
            data[at + 3] = cur;
            data[at + 5..at + 11].copy_from_slice(&raw.to_le_bytes()[..6]);
        };
        put(0, 9, 99, 12_345);
        put(1, 194, 100, 0x0028_0012_0023); // 35 °C, min/max in higher bytes
        put(2, 177, 95, 40);
        let attrs = ata_attributes(&data);
        let s = ata_health("sata1", &attrs);
        let v = |id: &str| s.iter().find(|x| x.id == id).map(|x| x.value);
        assert_eq!(v("sata1_temp"), Some(35.0));
        assert_eq!(v("sata1_life"), Some(95.0));
        assert_eq!(v("sata1_power_on"), Some(12_345.0 * 3600.0));
    }
}
