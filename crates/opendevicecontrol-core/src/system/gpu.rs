//! Graphics card statistics.
//!
//! * NVIDIA (Windows and Linux): NVML, loaded at runtime from the driver
//!   (`nvml.dll` / `libnvidia-ml.so`); no admin rights needed.
//! * AMD on Linux: the amdgpu driver's sysfs files.
//! * Other GPUs on Windows (AMD, Intel): load and memory use from Windows'
//!   GPU performance counters (`gpu_win`); AMD's temperatures, clocks,
//!   power and fan from the AMD Display Library (`gpu_adl`).

use super::tree::Tree;
use crate::model::{HardwareKind, Sensor, SensorKind};
use nvml_wrapper::enum_wrappers::device::{Clock, PcieUtilCounter, TemperatureSensor};
use nvml_wrapper::Nvml;

const GB: f64 = 1024.0 * 1024.0 * 1024.0;

pub struct Gpus {
    nvml: Option<Nvml>,
    #[cfg(target_os = "linux")]
    amd: Vec<std::path::PathBuf>,
    #[cfg(windows)]
    win: Option<super::gpu_win::WinGpus>,
    #[cfg(windows)]
    adl: Option<super::gpu_adl::AdlGpus>,
}

impl Default for Gpus {
    fn default() -> Self {
        Self::new()
    }
}

impl Gpus {
    pub fn new() -> Self {
        let nvml = match Nvml::init() {
            Ok(n) => {
                log::info!("NVIDIA GPU statistics via NVML ({} GPU(s))", n.device_count().unwrap_or(0));
                Some(n)
            }
            Err(e) => {
                log::debug!("NVML unavailable: {e}");
                None
            }
        };
        Gpus {
            nvml,
            #[cfg(target_os = "linux")]
            amd: linux_amd_cards(),
            #[cfg(windows)]
            win: super::gpu_win::WinGpus::open(),
            #[cfg(windows)]
            adl: super::gpu_adl::AdlGpus::open(),
        }
    }

    /// GPU names, for device information.
    pub fn names(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(n) = &self.nvml {
            for i in 0..n.device_count().unwrap_or(0) {
                if let Ok(name) = n.device_by_index(i).and_then(|d| d.name()) {
                    out.push(name);
                }
            }
        }
        out
    }

    /// Add one node per GPU to `tree`. The first sensors of each keep their
    /// historic ids (`gpu_load`, `gpu_temp`…); the rest are detail sensors.
    pub fn read(&mut self, tree: &mut Tree) {
        let total = self.nvml.as_ref().map_or(0, |n| n.device_count().unwrap_or(0) as usize);
        #[cfg(target_os = "linux")]
        let total = total + self.amd.len();
        let mut index = 0;
        if let Some(n) = &self.nvml {
            for i in 0..n.device_count().unwrap_or(0) {
                let Ok(d) = n.device_by_index(i) else { continue };
                let (id, name) = prefix(index, total);
                index += 1;
                let model = d.name().unwrap_or_else(|_| name.clone());
                tree.node(&id, &model, HardwareKind::Gpu, None);
                let mut out = Vec::new();
                let s = |suffix: &str, label: &str, kind: SensorKind, v: f64| {
                    Sensor::new(&format!("{id}_{suffix}"), &format!("{name} {label}"), kind, v)
                };
                if let Ok(u) = d.utilization_rates() {
                    out.push(s("load", "load", SensorKind::Load, u.gpu as f64));
                    out.push(s("memory_load", "memory controller load", SensorKind::Load, u.memory as f64).diagnostic());
                }
                if let Ok(t) = d.temperature(TemperatureSensor::Gpu) {
                    out.push(s("temp", "temperature", SensorKind::Temperature, t as f64));
                }
                if let Ok(m) = d.memory_info() {
                    out.push(s("vram_used", "VRAM used", SensorKind::DataSize, m.used as f64 / GB));
                    if m.total > 0 {
                        out.push(s("vram_load", "VRAM usage", SensorKind::Load, m.used as f64 / m.total as f64 * 100.0));
                    }
                    out.push(s("vram_total", "VRAM total", SensorKind::DataSize, m.total as f64 / GB).diagnostic());
                    out.push(s("vram_free", "VRAM free", SensorKind::DataSize, m.free as f64 / GB).extra());
                }
                if let Ok(mw) = d.power_usage() {
                    out.push(s("power", "power", SensorKind::Power, mw as f64 / 1000.0));
                }
                if let Ok(mw) = d.enforced_power_limit() {
                    out.push(s("power_limit", "power limit", SensorKind::Power, mw as f64 / 1000.0).extra());
                }
                if let Ok(f) = d.fan_speed(0) {
                    out.push(s("fan", "fan", SensorKind::Duty, f as f64));
                }
                for f in 1..d.num_fans().unwrap_or(0).min(8) {
                    if let Ok(v) = d.fan_speed(f) {
                        out.push(s(&format!("fan{}", f + 1), &format!("fan {}", f + 1), SensorKind::Duty, v as f64).extra());
                    }
                }
                if let Ok(c) = d.clock_info(Clock::Graphics) {
                    out.push(s("clock", "clock", SensorKind::Frequency, c as f64).diagnostic());
                }
                if let Ok(c) = d.clock_info(Clock::Memory) {
                    out.push(s("memory_clock", "memory clock", SensorKind::Frequency, c as f64).extra());
                }
                if let Ok(c) = d.clock_info(Clock::Video) {
                    out.push(s("video_clock", "video clock", SensorKind::Frequency, c as f64).extra());
                }
                if let Ok(u) = d.encoder_utilization() {
                    out.push(s("encoder_load", "video encoder load", SensorKind::Load, u.utilization as f64).extra());
                }
                if let Ok(u) = d.decoder_utilization() {
                    out.push(s("decoder_load", "video decoder load", SensorKind::Load, u.utilization as f64).extra());
                }
                // KB/s.
                if let Ok(v) = d.pcie_throughput(PcieUtilCounter::Receive) {
                    out.push(s("pcie_rx", "PCIe receive", SensorKind::DataRate, v as f64 / 1024.0).extra());
                }
                if let Ok(v) = d.pcie_throughput(PcieUtilCounter::Send) {
                    out.push(s("pcie_tx", "PCIe send", SensorKind::DataRate, v as f64 / 1024.0).extra());
                }
                tree.extend(&id, out.into_iter().map(|x| x.from_source("NVML")));
            }
        }
        #[cfg(target_os = "linux")]
        for card in &self.amd {
            let (id, name) = prefix(index, total);
            index += 1;
            tree.node(&id, &linux_amd_name(card).unwrap_or_else(|| format!("AMD {name}")), HardwareKind::Gpu, None);
            tree.extend(&id, linux_amd_read(card, &id, &name).into_iter().map(|x| x.from_source("amdgpu")));
        }
        let _ = index;
        // Their own ids, so adding one never renames NVIDIA's `gpu_*`.
        #[cfg(windows)]
        let mut adl = self.adl.as_mut().map(|a| a.read()).unwrap_or_default();
        #[cfg(windows)]
        if let Some(w) = &mut self.win {
            for r in w.read() {
                let id = format!("gpu_win{}", r.index + 1);
                tree.node(&id, &r.name, HardwareKind::Gpu, None);
                // AMD's own readings first (their load wins over Windows').
                if let Some(i) = adl.iter().position(|a| super::tree::same_name(&a.name, &r.name)) {
                    let a = adl.remove(i);
                    tree.extend(
                        &id,
                        a.sensors.into_iter().map(|(suffix, label, kind, v)| {
                            Sensor::new(&format!("{id}_{suffix}"), label, kind, v).extra().from_source("ADL")
                        }),
                    );
                }
                let mut out = Vec::new();
                if let Some(l) = r.load {
                    out.push(Sensor::new(&format!("{id}_load"), "GPU load", SensorKind::Load, l.round()).extra());
                }
                if let Some(m) = r.dedicated_used.filter(|_| r.dedicated_total > 0.0) {
                    out.push(Sensor::new(&format!("{id}_vram_used"), "Dedicated memory used", SensorKind::DataSize, m).extra());
                    out.push(Sensor::new(&format!("{id}_vram_total"), "Dedicated memory", SensorKind::DataSize, r.dedicated_total).extra().diagnostic());
                }
                if let Some(m) = r.shared_used {
                    out.push(Sensor::new(&format!("{id}_shared_used"), "Shared memory used", SensorKind::DataSize, m).extra());
                }
                let _ = r.vendor;
                tree.extend(&id, out.into_iter().map(|x| x.from_source("Windows")));
            }
        }
        // AMD GPUs Windows' counters didn't list.
        #[cfg(windows)]
        for (n, a) in adl.into_iter().enumerate() {
            let id = format!("gpu_amd{}", n + 1);
            tree.node(&id, &a.name, HardwareKind::Gpu, None);
            tree.extend(
                &id,
                a.sensors.into_iter().map(|(suffix, label, kind, v)| Sensor::new(&format!("{id}_{suffix}"), label, kind, v).extra().from_source("ADL")),
            );
        }
    }
}

/// `("gpu", "GPU")` for a single GPU, `("gpu2", "GPU 2")` etc. otherwise.
fn prefix(index: usize, total: usize) -> (String, String) {
    if total <= 1 { ("gpu".into(), "GPU".into()) } else { (format!("gpu{}", index + 1), format!("GPU {}", index + 1)) }
}

#[cfg(target_os = "linux")]
fn linux_amd_cards() -> Vec<std::path::PathBuf> {
    let mut cards: Vec<std::path::PathBuf> = std::fs::read_dir("/sys/class/drm")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            n.starts_with("card") && n[4..].chars().all(|c| c.is_ascii_digit())
        })
        .map(|p| p.join("device"))
        .filter(|d| std::fs::read_to_string(d.join("vendor")).is_ok_and(|v| v.trim() == "0x1002"))
        .filter(|d| d.join("gpu_busy_percent").exists())
        .collect();
    cards.sort();
    cards
}

/// Marketing name from the kernel's `product_name` (some cards) or a
/// generic name with the PCI device id.
#[cfg(target_os = "linux")]
fn linux_amd_name(dev: &std::path::Path) -> Option<String> {
    let read = |f: &str| std::fs::read_to_string(dev.join(f)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    read("product_name").or_else(|| read("device").map(|d| format!("AMD Radeon GPU ({d})")))
}

#[cfg(target_os = "linux")]
fn linux_amd_read(dev: &std::path::Path, id: &str, name: &str) -> Vec<Sensor> {
    let num = |p: std::path::PathBuf| std::fs::read_to_string(p).ok().and_then(|s| s.trim().parse::<f64>().ok());
    let mut out = Vec::new();
    if let Some(v) = num(dev.join("gpu_busy_percent")) {
        out.push(Sensor::new(&format!("{id}_load"), &format!("{name} load"), SensorKind::Load, v));
    }
    let used = num(dev.join("mem_info_vram_used"));
    let total = num(dev.join("mem_info_vram_total"));
    if let Some(u) = used {
        out.push(Sensor::new(&format!("{id}_vram_used"), &format!("{name} VRAM used"), SensorKind::DataSize, u / GB));
        if let Some(t) = total.filter(|t| *t > 0.0) {
            out.push(Sensor::new(&format!("{id}_vram_load"), &format!("{name} VRAM usage"), SensorKind::Load, u / t * 100.0));
            out.push(Sensor::new(&format!("{id}_vram_total"), &format!("{name} VRAM total"), SensorKind::DataSize, t / GB).diagnostic());
        }
    }
    if let Some(v) = num(dev.join("mem_busy_percent")) {
        out.push(Sensor::new(&format!("{id}_memory_load"), &format!("{name} memory controller load"), SensorKind::Load, v).diagnostic());
    }
    let hwmon = std::fs::read_dir(dev.join("hwmon")).ok().and_then(|mut r| r.next()).and_then(|e| e.ok()).map(|e| e.path());
    if let Some(h) = hwmon {
        if let Some(t) = num(h.join("temp1_input")) {
            out.push(Sensor::new(&format!("{id}_temp"), &format!("{name} temperature"), SensorKind::Temperature, t / 1000.0));
        }
        if let Some(p) = num(h.join("power1_average")).or_else(|| num(h.join("power1_input"))) {
            out.push(Sensor::new(&format!("{id}_power"), &format!("{name} power"), SensorKind::Power, p / 1_000_000.0));
        }
        if let Some(f) = num(h.join("fan1_input")) {
            out.push(Sensor::new(&format!("{id}_fan_rpm"), &format!("{name} fan"), SensorKind::FanSpeed, f).extra());
        }
    }
    out
}
