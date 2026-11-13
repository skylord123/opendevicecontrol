//! Linux kernel sensors: every chip in `/sys/class/hwmon` (CPU `k10temp` /
//! `coretemp`, Super I/O `nct6775` / `it87`, drives, GPUs, DIMMs…) with all
//! its inputs, the motherboard name from DMI, and batteries from
//! `/sys/class/power_supply`. No privileges needed.
//!
//! The reader takes the sysfs roots as arguments, so it is tested on every
//! platform with a fake tree.

use super::external::{slug, ExtHardware, ExtSensor, ExtTree};
use crate::model::{HardwareKind, SensorKind};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub const SOURCE: &str = "hwmon";

/// Where to read from.
pub struct Roots {
    pub hwmon: PathBuf,
    pub dmi: PathBuf,
    pub power_supply: PathBuf,
}

impl Default for Roots {
    fn default() -> Self {
        Roots {
            hwmon: "/sys/class/hwmon".into(),
            dmi: "/sys/class/dmi/id".into(),
            power_supply: "/sys/class/power_supply".into(),
        }
    }
}

/// Fans that read 0 RPM are hidden until they have turned once (Super I/O
/// chips have headers for fans that aren't connected).
#[derive(Default)]
pub struct Reader {
    fans_seen: HashSet<String>,
}

fn line(p: &Path) -> Option<String> {
    let s = fs::read_to_string(p).ok()?;
    Some(s.trim_end().to_string())
}

fn num(p: &Path) -> Option<f64> {
    line(p)?.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// What a hwmon chip is, from its driver name.
pub fn chip_kind(name: &str) -> HardwareKind {
    let n = name.to_lowercase();
    let starts = |p: &[&str]| p.iter().any(|x| n.starts_with(x));
    if starts(&["k10temp", "coretemp", "zenpower", "k8temp", "fam15h_power", "cpu_thermal", "cpu-thermal"]) {
        HardwareKind::Cpu
    } else if starts(&["nct6", "nct7", "it8", "it87", "f71", "f81", "w83", "asus", "gigabyte", "dell_smm", "thinkpad", "applesmc", "sch5", "nct"]) {
        HardwareKind::Chip
    } else if starts(&["amdgpu", "radeon", "nouveau", "i915", "xe"]) {
        HardwareKind::Gpu
    } else if starts(&["nvme", "drivetemp"]) {
        HardwareKind::Storage
    } else if starts(&["spd5118", "jc42", "ee1004"]) {
        HardwareKind::Memory
    } else if starts(&["iwlwifi", "mt79", "ath1", "r8169", "igc", "ixgbe", "bnxt", "atlantic"]) {
        HardwareKind::Network
    } else if starts(&["corsairpsu"]) {
        HardwareKind::PowerSupply
    } else if starts(&["nzxt", "corsair", "aquacomputer", "kraken", "d5next", "quadro", "octo", "highflow"]) {
        HardwareKind::Cooler
    } else if starts(&["bat", "battery"]) {
        HardwareKind::Battery
    } else {
        HardwareKind::Other
    }
}

/// The label sysinfo gives a temperature (`"k10temp Tctl"`): the id of the
/// `temp_*` sensors OpenDeviceControl has always published, kept for Home Assistant.
fn sysinfo_label(name: &str, label: Option<&str>, model: Option<&str>, n: u32) -> String {
    match (label.filter(|l| !l.is_empty()), model) {
        (Some(l), Some(m)) => format!("{name} {l} {m}"),
        (Some(l), None) => format!("{name} {l}"),
        (None, Some(m)) => format!("{name} {m}"),
        (None, None) => format!("{name} temp{n}"),
    }
}

/// `temp3_input` → ("temp", 3).
fn split_input(file: &str) -> Option<(&str, u32)> {
    let (head, item) = match file.split_once('_') {
        Some((h, i)) => (h, i),
        None => (file, ""),
    };
    let pos = head.find(|c: char| c.is_ascii_digit())?;
    let (class, n) = head.split_at(pos);
    let n: u32 = n.parse().ok()?;
    match (class, item) {
        ("temp" | "in" | "fan" | "power" | "curr" | "freq", "input") => Some((class, n)),
        ("power", "average") => Some(("power_avg", n)),
        ("pwm", "") => Some(("pwm", n)),
        _ => None,
    }
}

impl Reader {
    pub fn read(&mut self, roots: &Roots) -> ExtTree {
        let mut hardware = Vec::new();
        let board = board(&roots.dmi);
        let mut chips: Vec<PathBuf> = fs::read_dir(&roots.hwmon).into_iter().flatten().flatten().map(|e| e.path()).collect();
        chips.sort_by_key(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            (n.len(), n)
        });
        let mut temps_on = 0;
        let mut seen_keys = HashSet::new();
        for dir in chips {
            if let Some(h) = self.chip(&dir, board.is_some(), &mut temps_on) {
                if seen_keys.insert(h.key.clone()) && !h.sensors.is_empty() {
                    hardware.push(h);
                }
            }
        }
        if let Some(b) = board {
            if hardware.iter().any(|h| h.parent.is_some()) {
                hardware.insert(0, b);
            } else {
                for h in &mut hardware {
                    h.parent = None;
                }
            }
        }
        hardware.extend(batteries(&roots.power_supply));
        ExtTree { source: SOURCE, hardware }
    }

    fn chip(&mut self, dir: &Path, have_board: bool, temps_on: &mut usize) -> Option<ExtHardware> {
        let name = line(&dir.join("name"))?;
        let model = line(&dir.join("device/model")).map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
        let dev = fs::read_link(dir.join("device"))
            .ok()
            .and_then(|l| l.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        let kind = chip_kind(&name);
        let key = format!("hwmon_{}", slug(&format!("{name}_{dev}")));
        let display = match kind {
            HardwareKind::Cpu => format!("CPU ({name})"),
            HardwareKind::Gpu => format!("GPU ({name})"),
            HardwareKind::Storage | HardwareKind::Memory => model.clone().unwrap_or_else(|| format!("{name} {dev}")),
            HardwareKind::Chip => name.to_uppercase(),
            _ => model.clone().map(|m| format!("{m} ({name})")).unwrap_or_else(|| name.clone()),
        };

        let mut inputs: Vec<(&'static str, u32, PathBuf)> = Vec::new();
        for e in fs::read_dir(dir).ok()?.flatten() {
            let file = e.file_name().to_string_lossy().into_owned();
            if let Some((class, n)) = split_input(&file) {
                let class: &'static str = match class {
                    "temp" => "temp",
                    "in" => "in",
                    "fan" => "fan",
                    "power" => "power",
                    "power_avg" => "power_avg",
                    "curr" => "curr",
                    "freq" => "freq",
                    _ => "pwm",
                };
                inputs.push((class, n, e.path()));
            }
        }
        let order = |c: &str| ["temp", "in", "power", "power_avg", "curr", "fan", "pwm", "freq"].iter().position(|x| *x == c).unwrap_or(9);
        inputs.sort_by_key(|(c, n, _)| (order(c), *n));

        let mut sensors = Vec::new();
        for (class, n, path) in inputs {
            let label_of = |prefix: &str| line(&dir.join(format!("{prefix}{n}_label"))).filter(|l| !l.trim().is_empty());
            let Some(raw) = num(&path) else { continue };
            let s = match class {
                "temp" => {
                    let t = raw / 1000.0;
                    if !super::external::plausible(t) {
                        continue;
                    }
                    let label = label_of("temp");
                    let legacy = sysinfo_label(&name, label.as_deref(), model.as_deref(), n);
                    let mut s = ExtSensor::new(
                        format!("temp_{}", slug(&legacy)),
                        label.unwrap_or_else(|| format!("Temperature #{n}")),
                        SensorKind::Temperature,
                        t,
                    );
                    // As before: up to 32 temperatures go to Home Assistant.
                    s.diagnostic = true;
                    s.default_on = *temps_on < 32;
                    *temps_on += 1;
                    s
                }
                "in" => ExtSensor::new(
                    format!("{key}_in{n}"),
                    label_of("in").unwrap_or_else(|| format!("Voltage #{n}")),
                    SensorKind::Voltage,
                    raw / 1000.0,
                ),
                "power" | "power_avg" => {
                    if class == "power_avg" && dir.join(format!("power{n}_input")).exists() {
                        continue;
                    }
                    ExtSensor::new(
                        format!("{key}_power{n}"),
                        label_of("power").unwrap_or_else(|| format!("Power #{n}")),
                        SensorKind::Power,
                        raw / 1e6,
                    )
                }
                "curr" => ExtSensor::new(
                    format!("{key}_curr{n}"),
                    label_of("curr").unwrap_or_else(|| format!("Current #{n}")),
                    SensorKind::Current,
                    raw / 1000.0,
                ),
                "fan" => {
                    let id = format!("{key}_fan{n}");
                    let label = label_of("fan");
                    if raw > 0.0 {
                        self.fans_seen.insert(id.clone());
                    }
                    if label.is_none() && !self.fans_seen.contains(&id) {
                        continue;
                    }
                    ExtSensor::new(id, label.unwrap_or_else(|| format!("Fan #{n}")), SensorKind::FanSpeed, raw)
                }
                "pwm" => {
                    // Only fan outputs that report a mode (`pwmN_enable`).
                    if !dir.join(format!("pwm{n}_enable")).exists() {
                        continue;
                    }
                    ExtSensor::new(format!("{key}_pwm{n}"), format!("Fan control #{n}"), SensorKind::Duty, raw / 255.0 * 100.0)
                }
                _ => ExtSensor::new(
                    format!("{key}_freq{n}"),
                    label_of("freq").unwrap_or_else(|| format!("Clock #{n}")),
                    SensorKind::Frequency,
                    raw / 1e6,
                ),
            };
            sensors.push(s);
        }
        let parent = (have_board && kind == HardwareKind::Chip).then(|| "hwmon_board".to_string());
        Some(ExtHardware { key, name: display, kind, parent, sensors })
    }
}

/// The motherboard, from DMI ("ASRock B650E PG Riptide WiFi").
fn board(dmi: &Path) -> Option<ExtHardware> {
    let vendor = line(&dmi.join("board_vendor")).unwrap_or_default();
    let name = line(&dmi.join("board_name"))?;
    let full = format!("{} {}", vendor.trim(), name.trim()).trim().to_string();
    (!full.is_empty()).then(|| ExtHardware {
        key: "hwmon_board".into(),
        name: full,
        kind: HardwareKind::Motherboard,
        parent: None,
        sensors: Vec::new(),
    })
}

/// Laptop and UPS batteries (`type` = Battery).
fn batteries(root: &Path) -> Vec<ExtHardware> {
    let mut out = Vec::new();
    let mut dirs: Vec<PathBuf> = fs::read_dir(root).into_iter().flatten().flatten().map(|e| e.path()).collect();
    dirs.sort();
    for d in dirs {
        if line(&d.join("type")).as_deref() != Some("Battery") || line(&d.join("scope")).as_deref() == Some("Device") {
            continue;
        }
        let dname = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let key = format!("battery_{}", slug(&dname));
        let mut sensors = Vec::new();
        if let Some(c) = num(&d.join("capacity")) {
            let mut s = ExtSensor::new(format!("{key}_level"), "Charge level", SensorKind::Battery, c);
            s.default_on = true;
            sensors.push(s);
        }
        if let Some(st) = line(&d.join("status")) {
            let mut s = ExtSensor::new(format!("{key}_charging"), "Charging", SensorKind::Charging, (st == "Charging") as u8 as f64);
            s.default_on = true;
            sensors.push(s);
        }
        let volts = num(&d.join("voltage_now")).map(|v| v / 1e6);
        let power = num(&d.join("power_now"))
            .map(|p| p / 1e6)
            .or_else(|| Some(num(&d.join("current_now"))? / 1e6 * volts?));
        if let Some(p) = power {
            sensors.push(ExtSensor::new(format!("{key}_power"), "Charge / discharge rate", SensorKind::Power, p));
        }
        if let Some(v) = volts {
            sensors.push(ExtSensor::new(format!("{key}_voltage"), "Voltage", SensorKind::Voltage, v));
        }
        let wear = match (num(&d.join("energy_full")), num(&d.join("energy_full_design"))) {
            (Some(f), Some(dsg)) if dsg > 0.0 => Some(f / dsg * 100.0),
            _ => match (num(&d.join("charge_full")), num(&d.join("charge_full_design"))) {
                (Some(f), Some(dsg)) if dsg > 0.0 => Some(f / dsg * 100.0),
                _ => None,
            },
        };
        if let Some(w) = wear {
            let mut s = ExtSensor::new(format!("{key}_health"), "Wear level (capacity left)", SensorKind::Load, w);
            s.diagnostic = true;
            sensors.push(s);
        }
        if sensors.is_empty() {
            continue;
        }
        let model = line(&d.join("model_name")).filter(|m| !m.trim().is_empty());
        let name = match model {
            Some(m) => format!("{} ({dname})", m.trim()),
            None => format!("Battery {dname}"),
        };
        out.push(ExtHardware { key, name, kind: HardwareKind::Battery, parent: None, sensors });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    #[test]
    fn reads_a_sysfs_tree() {
        let root = std::env::temp_dir().join(format!("opendevicecontrol-hwmon-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let hw = root.join("hwmon");
        write(&hw, "hwmon0/name", "k10temp\n");
        write(&hw, "hwmon0/temp1_input", "61500\n");
        write(&hw, "hwmon0/temp1_label", "Tctl\n");
        write(&hw, "hwmon0/temp3_input", "58000\n");
        write(&hw, "hwmon0/temp3_label", "Tccd1\n");
        write(&hw, "hwmon1/name", "nct6799\n");
        write(&hw, "hwmon1/in0_input", "1032\n");
        write(&hw, "hwmon1/in0_label", "Vcore\n");
        write(&hw, "hwmon1/fan1_input", "1545\n");
        write(&hw, "hwmon1/fan2_input", "0\n");
        write(&hw, "hwmon1/pwm1", "128\n");
        write(&hw, "hwmon1/pwm1_enable", "5\n");
        write(&hw, "hwmon1/temp1_input", "35000\n");
        write(&hw, "hwmon1/temp7_input", "-62000\n");
        write(&hw, "hwmon2/name", "nvme\n");
        write(&hw, "hwmon2/device/model", "WD_BLACK SN850X 2000GB                  \n");
        write(&hw, "hwmon2/temp1_input", "40850\n");
        write(&hw, "hwmon2/temp1_label", "Composite\n");
        write(&hw, "hwmon3/name", "amdgpu\n");
        write(&hw, "hwmon3/power1_average", "45000000\n");
        write(&hw, "hwmon3/freq1_input", "2400000000\n");
        write(&hw, "hwmon3/freq1_label", "sclk\n");
        write(&root, "dmi/board_vendor", "ASRock\n");
        write(&root, "dmi/board_name", "B650E PG Riptide WiFi\n");
        write(&root, "ps/BAT0/type", "Battery\n");
        write(&root, "ps/BAT0/capacity", "87\n");
        write(&root, "ps/BAT0/status", "Discharging\n");
        write(&root, "ps/BAT0/power_now", "9500000\n");
        write(&root, "ps/AC/type", "Mains\n");

        let roots = Roots { hwmon: hw.clone(), dmi: root.join("dmi"), power_supply: root.join("ps") };
        let mut r = Reader::default();
        let t = r.read(&roots);
        let nodes: Vec<(&str, &str, HardwareKind, Option<&str>)> =
            t.hardware.iter().map(|h| (h.key.as_str(), h.name.as_str(), h.kind, h.parent.as_deref())).collect();
        assert_eq!(
            nodes,
            vec![
                ("hwmon_board", "ASRock B650E PG Riptide WiFi", HardwareKind::Motherboard, None),
                ("hwmon_k10temp_hwmon0", "CPU (k10temp)", HardwareKind::Cpu, None),
                ("hwmon_nct6799_hwmon1", "NCT6799", HardwareKind::Chip, Some("hwmon_board")),
                ("hwmon_nvme_hwmon2", "WD_BLACK SN850X 2000GB", HardwareKind::Storage, None),
                ("hwmon_amdgpu_hwmon3", "GPU (amdgpu)", HardwareKind::Gpu, None),
                ("battery_bat0", "Battery BAT0", HardwareKind::Battery, None),
            ]
        );
        let ids = |i: usize| t.hardware[i].sensors.iter().map(|s| (s.id.clone(), s.label.clone(), s.value)).collect::<Vec<_>>();
        // sysinfo-compatible temperature ids.
        assert_eq!(
            ids(1),
            vec![("temp_k10temp_tctl".into(), "Tctl".into(), 61.5), ("temp_k10temp_tccd1".into(), "Tccd1".into(), 58.0)]
        );
        assert_eq!(ids(3)[0].0, "temp_nvme_composite_wd_black_sn850x_2000gb");
        // Board chip: the implausible temperature and the unconnected fan are skipped.
        let chip: Vec<String> = ids(2).into_iter().map(|x| x.0).collect();
        assert_eq!(chip, ["temp_nct6799_temp1", "hwmon_nct6799_hwmon1_in0", "hwmon_nct6799_hwmon1_fan1", "hwmon_nct6799_hwmon1_pwm1"]);
        assert!((t.hardware[2].sensors[3].value - 50.2).abs() < 0.1);
        assert_eq!(ids(4), vec![("hwmon_amdgpu_hwmon3_power1".into(), "Power #1".into(), 45.0), ("hwmon_amdgpu_hwmon3_freq1".into(), "sclk".into(), 2400.0)]);
        let bat: Vec<(String, f64)> = t.hardware[5].sensors.iter().map(|s| (s.id.clone(), s.value)).collect();
        assert_eq!(bat, vec![("battery_bat0_level".into(), 87.0), ("battery_bat0_charging".into(), 0.0), ("battery_bat0_power".into(), 9.5)]);
        assert!(t.hardware[1].sensors[0].default_on && t.hardware[1].sensors[0].diagnostic);
        assert!(!t.hardware[2].sensors[1].default_on);

        // Once a fan has turned, it stays listed when it stops.
        fs::write(hw.join("hwmon1/fan2_input"), "800\n").unwrap();
        r.read(&roots);
        fs::write(hw.join("hwmon1/fan2_input"), "0\n").unwrap();
        let t = r.read(&roots);
        assert!(t.hardware[2].sensors.iter().any(|s| s.id.ends_with("_fan2")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn input_names() {
        assert_eq!(split_input("temp12_input"), Some(("temp", 12)));
        assert_eq!(split_input("pwm2"), Some(("pwm", 2)));
        assert_eq!(split_input("power1_average"), Some(("power_avg", 1)));
        assert_eq!(split_input("pwm2_enable"), None);
        assert_eq!(split_input("temp1_label"), None);
        assert_eq!(split_input("name"), None);
    }
}
