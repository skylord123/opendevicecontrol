//! This PC's sensor tree: OpenDeviceControl's own nodes (computer, CPU, memory, GPUs,
//! drives, network) with the hardware trees of other sources merged in, so
//! a CPU seen by sysinfo, k10temp and LibreHardwareMonitor is one node.

use super::external::ExtTree;
use crate::model::{DeviceInfo, DeviceKind, Hardware, HardwareKind, Sensor};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// Devices OpenDeviceControl drives itself: (kind, manufacturer, model).
static OWNED: Mutex<Vec<(DeviceKind, String, String)>> = Mutex::new(Vec::new());

/// Note a device OpenDeviceControl drives itself. Other sources' copy of it
/// (LibreHardwareMonitor's, HWiNFO's, a Linux hwmon driver's) is then left
/// out of This PC: it has its own page, and the copy uses other names.
pub fn own(info: &DeviceInfo) {
    let mut o = OWNED.lock().unwrap_or_else(|e| e.into_inner());
    let entry = (info.kind, info.manufacturer.clone(), info.model.clone());
    if !o.contains(&entry) {
        o.push(entry);
    }
}

/// Is this hardware from another source one of OpenDeviceControl's own devices?
fn is_owned(name: &str, kind: HardwareKind) -> bool {
    let o = OWNED.lock().unwrap_or_else(|e| e.into_inner());
    let lower = name.to_lowercase();
    o.iter().any(|(dk, maker, model)| {
        let full = if maker.is_empty() { model.clone() } else { format!("{maker} {model}") };
        let same_kind = match dk {
            DeviceKind::PowerSupply => kind == HardwareKind::PowerSupply,
            DeviceKind::Cooler | DeviceKind::FanHub => kind == HardwareKind::Cooler,
            _ => false,
        };
        same_name(name, &full)
            || same_name(name, model)
            // Kernel drivers name the chip after the driver ("corsairpsu").
            || (same_kind && !maker.is_empty() && lower.contains(&maker.to_lowercase()))
    })
}

#[derive(Debug, Default, Clone)]
pub struct Tree {
    pub hardware: Vec<Hardware>,
    pub sensors: Vec<Sensor>,
    /// Nodes a merged source created (not OpenDeviceControl's own).
    sourced: HashSet<String>,
}

/// "AMD Ryzen 7 7800X3D 8-Core Processor" and "AMD Ryzen 7 7800X3D" name
/// the same part; so do "NVIDIA GeForce RTX 4080" and itself.
pub fn same_name(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let s = s.to_lowercase().replace("(r)", "").replace("(tm)", "").replace(['™', '®'], "");
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let (a, b) = (norm(a), norm(b));
    let (short, long) = if a.len() <= b.len() { (&a, &b) } else { (&b, &a) };
    if long == short {
        return true;
    }
    // A longer name may only add a description ("8-Core Processor"), not a
    // model suffix ("SUPER", "Ti", "2").
    let rest = long.strip_prefix(&format!("{short} ")).unwrap_or("");
    short.len() >= 5 && (rest.contains("processor") || rest.contains("-core") || rest.starts_with('('))
}

impl Tree {
    /// Add a node (no-op if the id exists).
    pub fn node(&mut self, id: &str, name: &str, kind: HardwareKind, parent: Option<&str>) {
        if self.hardware.iter().all(|h| h.id != id) {
            self.hardware.push(Hardware { id: id.into(), name: name.into(), kind, parent: parent.map(Into::into) });
        }
    }

    /// Add a sensor to node `hw` (dropped if its id is taken).
    pub fn add(&mut self, hw: &str, s: Sensor) {
        if self.sensors.iter().all(|x| x.id != s.id) {
            self.sensors.push(s.on(hw));
        }
    }

    pub fn extend(&mut self, hw: &str, list: impl IntoIterator<Item = Sensor>) {
        for s in list {
            self.add(hw, s);
        }
    }

    /// Merge another source's tree. Its nodes join an existing node that is
    /// the same hardware (same kind and name, or the only CPU / GPU on both
    /// sides); the rest are added under their own ids. A child node that an
    /// earlier (preferred) source already added under the same parent, such
    /// as the motherboard chip read by the sensor service, is left out with
    /// everything below it: it would only repeat the same readings under
    /// other names.
    pub fn merge(&mut self, t: &ExtTree) {
        let before: Vec<Hardware> = self.hardware.clone();
        let earlier = self.sourced.clone();
        let count = |kind: HardwareKind, list: &[Hardware]| list.iter().filter(|h| h.kind == kind && h.parent.is_none()).count();
        let ext_top = |kind: HardwareKind| t.hardware.iter().filter(|h| h.kind == kind && h.parent.is_none()).count();
        let mut map: HashMap<&str, String> = HashMap::new();
        let mut skipped: HashSet<&str> = HashSet::new();
        for h in &t.hardware {
            let parent_id = h.parent.as_deref().and_then(|p| map.get(p));
            // (A top-level node joins its match below instead.)
            let seen_before = parent_id.is_some()
                && before.iter().any(|n| {
                    earlier.contains(&n.id) && n.kind == h.kind && same_name(&n.name, &h.name) && n.parent.as_ref() == parent_id
                });
            let skip = seen_before
                || match h.parent.as_deref() {
                    None => is_owned(&h.name, h.kind),
                    Some(p) => skipped.contains(p),
                };
            if skip {
                skipped.insert(&h.key);
                continue;
            }
            let parent = h.parent.as_deref().and_then(|p| map.get(p).cloned());
            let target = if h.parent.is_none() {
                let by_name = before.iter().find(|n| n.kind == h.kind && same_name(&n.name, &h.name));
                let single = matches!(h.kind, HardwareKind::Cpu | HardwareKind::Gpu)
                    && count(h.kind, &before) == 1
                    && ext_top(h.kind) == 1;
                let memory = h.kind == HardwareKind::Memory && (h.key == "lhm_ram" || h.name.eq_ignore_ascii_case("total memory"));
                by_name
                    .or_else(|| single.then(|| before.iter().find(|n| n.kind == h.kind && n.parent.is_none())).flatten())
                    .or_else(|| memory.then(|| before.iter().find(|n| n.id == "memory")).flatten())
                    .map(|n| n.id.clone())
            } else {
                None
            };
            let id = match target {
                Some(id) => id,
                None => {
                    self.node(&h.key, &h.name, h.kind, parent.as_deref());
                    self.sourced.insert(h.key.clone());
                    h.key.clone()
                }
            };
            for s in &h.sensors {
                let mut x = Sensor::new(&s.id, &s.label, s.kind, s.value).from_source(t.source);
                if !s.default_on {
                    x = x.extra();
                }
                if s.diagnostic {
                    x = x.diagnostic();
                }
                self.add(&id, x);
            }
            map.insert(&h.key, id);
        }
        // Nodes that ended up without sensors or children are noise.
        self.prune();
    }

    /// Remove nodes with no sensors anywhere below them.
    pub fn prune(&mut self) {
        loop {
            let empty: Vec<String> = self
                .hardware
                .iter()
                .filter(|h| {
                    !self.sensors.iter().any(|s| s.hardware.as_deref() == Some(&h.id))
                        && !self.hardware.iter().any(|c| c.parent.as_deref() == Some(&h.id))
                })
                .map(|h| h.id.clone())
                .collect();
            if empty.is_empty() {
                return;
            }
            self.hardware.retain(|h| !empty.contains(&h.id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SensorKind;
    use crate::system::external::{ExtHardware, ExtSensor};

    fn ext(key: &str, name: &str, kind: HardwareKind, parent: Option<&str>, sensors: &[&str]) -> ExtHardware {
        ExtHardware {
            key: key.into(),
            name: name.into(),
            kind,
            parent: parent.map(Into::into),
            sensors: sensors.iter().map(|s| ExtSensor::new(*s, *s, SensorKind::Temperature, 50.0)).collect(),
        }
    }

    #[test]
    fn names() {
        assert!(same_name("AMD Ryzen 7 7800X3D 8-Core Processor", "AMD Ryzen 7 7800X3D"));
        assert!(same_name("NVIDIA GeForce RTX 4080", "NVIDIA GeForce RTX 4080"));
        assert!(!same_name("NVIDIA GeForce RTX 4080", "NVIDIA GeForce RTX 4080 SUPER x"));
        assert!(same_name("Intel(R) Core(TM) i7", "Intel Core i7"));
        assert!(!same_name("Wi-Fi", "Wi-Fi 2"));
    }

    #[test]
    fn merging() {
        let mut t = Tree::default();
        t.node("cpu", "AMD Ryzen 7 7800X3D 8-Core Processor", HardwareKind::Cpu, None);
        t.add("cpu", Sensor::new("cpu_load", "CPU load", SensorKind::Load, 10.0));
        t.node("gpu", "NVIDIA GeForce RTX 4070 Ti SUPER", HardwareKind::Gpu, None);
        t.add("gpu", Sensor::new("gpu_load", "GPU load", SensorKind::Load, 10.0));
        t.node("memory", "Memory", HardwareKind::Memory, None);
        t.add("memory", Sensor::new("memory_load", "Memory usage", SensorKind::Load, 10.0));
        let lhm = ExtTree {
            source: "LibreHardwareMonitor",
            hardware: vec![
                ext("lhm_motherboard", "ASRock B650E", HardwareKind::Motherboard, None, &[]),
                ext("lhm_lpc", "Nuvoton NCT6799D", HardwareKind::Chip, Some("lhm_motherboard"), &["lhm_sys"]),
                ext("lhm_amdcpu_0", "AMD Ryzen 7 7800X3D", HardwareKind::Cpu, None, &["lhm_tctl"]),
                ext("lhm_ram", "Total Memory", HardwareKind::Memory, None, &["lhm_ram_load"]),
                ext("lhm_gpu_nvidia_0", "NVIDIA GeForce RTX 4070 Ti SUPER", HardwareKind::Gpu, None, &["lhm_gpu_hot"]),
                ext("lhm_gpu_amd_0", "AMD Radeon(TM) Graphics", HardwareKind::Gpu, None, &["lhm_igpu"]),
                ext("lhm_empty", "Nothing", HardwareKind::Other, None, &[]),
            ],
        };
        t.merge(&lhm);
        let ids: Vec<(&str, Option<&str>)> = t.hardware.iter().map(|h| (h.id.as_str(), h.parent.as_deref())).collect();
        assert_eq!(
            ids,
            vec![
                ("cpu", None),
                ("gpu", None),
                ("memory", None),
                ("lhm_motherboard", None),
                ("lhm_lpc", Some("lhm_motherboard")),
                ("lhm_gpu_amd_0", None),
            ]
        );
        let on = |id: &str| t.sensors.iter().find(|s| s.id == id).and_then(|s| s.hardware.clone());
        assert_eq!(on("lhm_tctl").as_deref(), Some("cpu"));
        assert_eq!(on("lhm_ram_load").as_deref(), Some("memory"));
        assert_eq!(on("lhm_gpu_hot").as_deref(), Some("gpu"));
        let s = t.sensors.iter().find(|s| s.id == "lhm_tctl").unwrap();
        assert!(s.extra && s.source.as_deref() == Some("LibreHardwareMonitor"));
    }

    #[test]
    fn earlier_source_wins() {
        let mut t = Tree::default();
        let service = ExtTree {
            source: "PawnIO",
            hardware: vec![
                ext("sio_board", "ASRock B650E PG Riptide WiFi", HardwareKind::Motherboard, None, &[]),
                ext("sio_nct6799d", "Nuvoton NCT6799D", HardwareKind::Chip, Some("sio_board"), &["sio_vcore"]),
            ],
        };
        let lhm = ExtTree {
            source: "LibreHardwareMonitor",
            hardware: vec![
                ext("lhm_motherboard", "ASRock B650E PG Riptide WiFi", HardwareKind::Motherboard, None, &[]),
                ext("lhm_lpc", "Nuvoton NCT6799D", HardwareKind::Chip, Some("lhm_motherboard"), &["lhm_v1"]),
                ext("lhm_ec", "Embedded controller", HardwareKind::Chip, Some("lhm_motherboard"), &["lhm_ec_t"]),
                ext("lhm_ssd", "Samsung 990 PRO", HardwareKind::Storage, None, &["lhm_ssd_t"]),
            ],
        };
        t.merge(&service);
        t.merge(&lhm);
        let ids: Vec<&str> = t.hardware.iter().map(|h| h.id.as_str()).collect();
        // The board and its Super I/O chip come from the service; LHM adds
        // what the service doesn't read.
        assert_eq!(ids, vec!["sio_board", "sio_nct6799d", "lhm_ec", "lhm_ssd"]);
        assert!(t.sensors.iter().all(|s| s.id != "lhm_v1"));
    }

    #[test]
    fn own_devices_left_out() {
        own(&DeviceInfo {
            id: "hx850i".into(),
            kind: DeviceKind::PowerSupply,
            manufacturer: "Corsair".into(),
            model: "HX850i".into(),
            vendor_string: None,
            product_string: None,
            usb_vid: 0,
            usb_pid: 0,
            serial: None,
            firmware: None,
        });
        let mut t = Tree::default();
        t.node("cpu", "CPU", HardwareKind::Cpu, None);
        t.add("cpu", Sensor::new("cpu_load", "CPU load", SensorKind::Load, 10.0));
        let ext_tree = ExtTree {
            source: "LibreHardwareMonitor",
            hardware: vec![
                ext("lhm_psu_corsair_0", "Corsair HX850i", HardwareKind::PowerSupply, None, &["lhm_psu_12v"]),
                ext("lhm_psu_rail", "Rail", HardwareKind::Other, Some("lhm_psu_corsair_0"), &["lhm_psu_rail_v"]),
                ext("hwmon_corsairpsu", "corsairpsu-hid-3-1", HardwareKind::PowerSupply, None, &["hwmon_psu_t"]),
                ext("lhm_ssd_0", "Corsair Force GT", HardwareKind::Storage, None, &["lhm_ssd_t"]),
            ],
        };
        t.merge(&ext_tree);
        let ids: Vec<&str> = t.hardware.iter().map(|h| h.id.as_str()).collect();
        // The PSU (and its children) has its own page; a Corsair SSD is not
        // the PSU.
        assert_eq!(ids, vec!["cpu", "lhm_ssd_0"]);
    }
}
