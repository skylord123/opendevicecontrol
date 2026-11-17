//! "This PC": the computer OpenDeviceControl runs on, as a device. Every sensor the
//! machine has — CPU, memory, GPUs, drives, network, motherboard chips,
//! batteries — as a tree of hardware nodes (like HWMonitor), plus presence
//! (locked, idle time, in use) for Home Assistant automations.
//!
//! Sources: sysinfo and NVML everywhere; on Linux the kernel's hwmon chips
//! and batteries (`hwmon.rs`); on Windows HWiNFO, LibreHardwareMonitor and
//! PawnIO (`windows.rs`), because Windows itself exposes almost nothing.
//!
//! Its id is always `this_pc`, so on a dual-boot machine Windows and Linux
//! publish to the same Home Assistant device. Sensors OpenDeviceControl published
//! before the tree existed keep their ids and go to Home Assistant by
//! default; the many detail sensors are marked [`Sensor::extra`].

pub mod external;
pub mod gpu;
pub mod hwmon;
pub mod presence;
pub mod asus_ec;
pub mod battery;
pub mod cpu_msr;
pub mod dimm;
pub mod drive_health;
pub mod superio;
pub mod tree;
#[cfg(windows)]
pub mod service;
#[cfg(windows)]
mod windows_drives;
#[cfg(windows)]
mod gpu_win;
#[cfg(windows)]
mod gpu_adl;
#[cfg(windows)]
mod windows;

use crate::devices::Device;
use crate::model::*;
use crate::{Error, Result};
#[cfg_attr(target_os = "linux", allow(unused_imports))]
use external::{slug, ExtHardware, ExtSensor, ExtTree};
#[cfg(target_os = "linux")]
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use sysinfo::{Components, Disks, Networks, System};
use tree::Tree;

pub const ID: &str = "this_pc";
/// Pseudo path used to tell discovery the device is already open.
pub const PATH: &str = "system:this_pc";

pub const CTRL_ACTIVE_TIMEOUT: &str = "active_timeout";
const DEFAULT_ACTIVE_MINUTES: f64 = 5.0;
const GB: f64 = 1024.0 * 1024.0 * 1024.0;
const MB: f64 = 1024.0 * 1024.0;
/// Shortest window for byte and CPU rates. Counters read closer together
/// give noise (a few ms of traffic scaled up to a second), so polls inside
/// the window repeat the last rates.
const MIN_RATE_WINDOW: Duration = Duration::from_secs(1);

/// Source tag of the OS's own sensors (sysinfo; Linux uses hwmon's).
#[cfg(not(target_os = "linux"))]
const OS: &str = "OS";

pub struct ThisPc {
    info: DeviceInfo,
    sys: System,
    networks: Networks,
    disks: Disks,
    components: Components,
    gpus: gpu::Gpus,
    presence: presence::Monitor,
    active_minutes: f64,
    /// When the rate counters were last refreshed.
    last: Instant,
    /// Length in seconds of the window the current rates cover; 0 until
    /// the first full window.
    rate_dt: f64,
    hardware: Vec<Hardware>,
    /// Unix time of the last input, kept steady across polls (see
    /// [`steady_since`]).
    last_input: Option<i64>,
}

/// Unix time now, seconds.
fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// The moment `elapsed` seconds ago, rounded to the second. Clock jitter
/// between the two time sources would otherwise move it by a second now
/// and then; a value within 2 s of the previous one keeps the previous.
fn steady_since(prev: Option<i64>, now: f64, elapsed: f64) -> i64 {
    let at = (now - elapsed).round() as i64;
    match prev {
        Some(p) if (p - at).abs() <= 2 => p,
        _ => at,
    }
}

impl ThisPc {
    pub fn open() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_all();
        sys.refresh_memory();
        let gpus = gpu::Gpus::new();
        let cpu = sys.cpus().first().map(|c| c.brand().trim().to_string()).filter(|s| !s.is_empty());
        let mut product = Vec::new();
        if let Some(c) = cpu {
            product.push(c);
        }
        product.extend(gpus.names());
        ThisPc {
            info: DeviceInfo {
                id: ID.into(),
                kind: DeviceKind::Computer,
                manufacturer: String::new(),
                model: "This PC".into(),
                vendor_string: System::long_os_version(),
                product_string: (!product.is_empty()).then(|| product.join(" · ")),
                usb_vid: 0,
                usb_pid: 0,
                serial: System::host_name(),
                firmware: None,
            },
            sys,
            networks: Networks::new_with_refreshed_list(),
            disks: {
                // The first listing counts I/O since boot; a second read
                // makes it the baseline.
                let mut d = Disks::new_with_refreshed_list();
                d.refresh(true);
                d
            },
            components: Components::new_with_refreshed_list(),
            gpus,
            presence: presence::Monitor::new(),
            active_minutes: DEFAULT_ACTIVE_MINUTES,
            last: Instant::now(),
            rate_dt: 0.0,
            hardware: Vec::new(),
            last_input: None,
        }
    }

    fn presence_sensors(&mut self, t: &mut Tree) {
        let p = self.presence.read();
        if let Some(locked) = p.locked {
            t.add("computer", Sensor::new("locked", "Locked", SensorKind::State, locked as u8 as f64).class("lock"));
        }
        if let Some(idle) = p.idle_secs {
            let at = steady_since(self.last_input, unix_now(), idle.max(0.0));
            self.last_input = Some(at);
            t.add(
                "computer",
                Sensor::new("idle_time", "Idle time", SensorKind::Duration, idle.max(0.0).round())
                    .icon("mdi:gesture-tap")
                    .since(at, "Last input"),
            );
        }
        if p.locked.is_some() || p.idle_secs.is_some() {
            let active = !p.locked.unwrap_or(false) && p.idle_secs.is_none_or(|s| s < self.active_minutes * 60.0);
            t.add(
                "computer",
                Sensor::new("active", "In use", SensorKind::State, active as u8 as f64).class("occupancy"),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Temperature sources.

/// The OS's own sensors as a tree: Linux hwmon (and batteries), elsewhere
/// whatever sysinfo's components report (ACPI zones on Windows, SMC on
/// macOS). Temperatures keep the `temp_<label>` ids of earlier versions.
fn os_tree(components: &Components) -> ExtTree {
    #[cfg(target_os = "linux")]
    {
        let _ = components;
        static READER: LazyLock<Mutex<hwmon::Reader>> = LazyLock::new(Default::default);
        let mut r = READER.lock().unwrap_or_else(|e| e.into_inner());
        r.read(&hwmon::Roots::default())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut cpu = ExtHardware { key: "os_cpu".into(), name: "CPU".into(), kind: HardwareKind::Cpu, parent: None, sensors: Vec::new() };
        let mut other =
            ExtHardware { key: "os_thermal".into(), name: "Thermal zones".into(), kind: HardwareKind::Other, parent: None, sensors: Vec::new() };
        for (i, c) in components.list().iter().enumerate() {
            let Some(t) = c.temperature().map(f64::from).filter(|t| external::plausible(*t)) else { continue };
            let mut s = ExtSensor::new(format!("temp_{}", slug(c.label())), c.label(), SensorKind::Temperature, t);
            s.diagnostic = true;
            s.default_on = i < 32;
            if is_cpu_label(c.label()) { cpu.sensors.push(s) } else { other.sensors.push(s) }
        }
        let mut hardware: Vec<ExtHardware> = [cpu, other].into_iter().filter(|h| !h.sensors.is_empty()).collect();
        // Laptop batteries (Windows' battery class driver).
        hardware.extend(battery::read().into_iter().filter(|h| !h.sensors.is_empty()));
        ExtTree { source: OS, hardware }
    }
}

/// CPU temperature labels as OpenDeviceControl has always recognised them.
fn is_cpu_label(label: &str) -> bool {
    let l = label.to_lowercase();
    ["cpu", "tctl", "tdie", "package", "k10temp", "coretemp", "zenpower"].iter().any(|k| l.contains(k))
}

/// All sources' trees in priority order: the OS, then (Windows) PawnIO,
/// HWiNFO or LibreHardwareMonitor.
fn source_trees(components: &Components) -> Vec<ExtTree> {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut v = vec![os_tree(components)];
    #[cfg(windows)]
    v.extend(windows::trees());
    v
}

/// `cpu_temp`: the hottest CPU temperature of the first source that has any
/// (sources measure the same thing, but their sensors differ slightly).
fn cpu_temp(trees: &[ExtTree]) -> Option<f64> {
    for t in trees {
        let mut best: Option<f64> = None;
        for h in &t.hardware {
            for s in h.sensors.iter().filter(|s| s.kind == SensorKind::Temperature && external::plausible(s.value)) {
                let r = external::Reading::new(t.source, external::Area::Other, s.label.clone(), s.value);
                let cpu_node = h.kind == HardwareKind::Cpu;
                let os_label = t.source != "PawnIO" && t.source != "HWiNFO" && t.source != "LibreHardwareMonitor";
                let cpu = (cpu_node || (os_label && is_cpu_label(&format!("{} {}", h.name, s.label))))
                    && external::Reading { area: external::Area::Cpu, ..r }.is_cpu_temperature();
                if cpu {
                    best = Some(best.map_or(s.value, |b: f64| b.max(s.value)));
                }
            }
        }
        if best.is_some() {
            return best;
        }
    }
    None
}

/// Host temperature sensors: `cpu_temp` and every temperature of every
/// source (fan-curve inputs, as `system/<id>`).
pub fn host_temperatures(components: &Components) -> Vec<Sensor> {
    let trees = source_trees(components);
    let mut t = Tree::default();
    t.node("cpu", "CPU", HardwareKind::Cpu, None);
    if let Some(c) = cpu_temp(&trees) {
        t.add("cpu", Sensor::new("cpu_temp", "CPU temperature", SensorKind::Temperature, c));
    }
    for tr in &trees {
        t.merge(tr);
    }
    t.sensors.retain(|s| s.kind == SensorKind::Temperature);
    t.sensors
}

/// Settings for the extra Windows sources (no-op elsewhere).
/// Look for the sources again now (after the sensor service was installed
/// or removed) instead of at the next retry.
pub fn sensor_sources_changed() {
    #[cfg(windows)]
    windows::rescan();
}

pub fn configure_sensors(pawnio: bool, lhm_port: u16) {
    #[cfg(windows)]
    windows::configure(windows::Settings { pawnio, lhm_port });
    #[cfg(not(windows))]
    let _ = (pawnio, lhm_port);
}

/// Status of the extra sensor sources, one line each.
pub fn sensor_sources() -> Vec<String> {
    #[cfg(windows)]
    return windows::status();
    #[cfg(not(windows))]
    {
        let t = os_tree(&Components::new());
        let n = t.sensor_count();
        vec![if n > 0 {
            format!("Linux hwmon: {} chip(s), {n} sensors", t.hardware.len())
        } else {
            "Linux hwmon: no sensors (is lm-sensors' sensors-detect needed for your Super I/O chip?)".into()
        }]
    }
}

impl Device for ThisPc {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll_interval(&self) -> Option<Duration> {
        Some(Duration::from_secs(2))
    }

    fn hardware(&self) -> Vec<Hardware> {
        self.hardware.clone()
    }

    fn poll(&mut self) -> Result<Snapshot> {
        let window = self.last.elapsed();
        let fresh = window >= MIN_RATE_WINDOW;
        if fresh {
            self.rate_dt = window.as_secs_f64();
            self.last = Instant::now();
        }
        let dt = self.rate_dt;
        let rate = |bytes: u64| if dt > 0.0 { bytes as f64 / MB / dt } else { 0.0 };
        let mut t = Tree::default();
        t.node("computer", "System", HardwareKind::Computer, None);

        // Presence first: it is what HA automations react to.
        self.presence_sensors(&mut t);

        // CPU.
        if fresh {
            self.sys.refresh_cpu_all();
        }
        let cpus = self.sys.cpus();
        let brand = cpus.first().map(|c| c.brand().trim().to_string()).filter(|b| !b.is_empty()).unwrap_or_else(|| "CPU".into());
        t.node("cpu", &brand, HardwareKind::Cpu, None);
        let mut c = vec![Sensor::new("cpu_load", "CPU load", SensorKind::Load, self.sys.global_cpu_usage() as f64)];
        if let Some(max) = cpus.iter().map(|c| c.cpu_usage() as f64).reduce(f64::max) {
            c.push(Sensor::new("cpu_load_max", "CPU busiest core", SensorKind::Load, max).diagnostic());
        }
        if !cpus.is_empty() {
            let f = cpus.iter().map(|c| c.frequency() as f64).sum::<f64>() / cpus.len() as f64;
            if f > 0.0 {
                c.push(Sensor::new("cpu_clock", "CPU clock", SensorKind::Frequency, f).diagnostic());
            }
        }
        for (i, cpu) in cpus.iter().enumerate() {
            let n = i + 1;
            c.push(Sensor::new(&format!("cpu_thread{n}_load"), &format!("Thread #{n} load"), SensorKind::Load, cpu.cpu_usage() as f64).extra());
        }
        for (i, cpu) in cpus.iter().enumerate().filter(|(_, c)| c.frequency() > 0) {
            let n = i + 1;
            c.push(Sensor::new(&format!("cpu_thread{n}_clock"), &format!("Thread #{n} clock"), SensorKind::Frequency, cpu.frequency() as f64).extra());
        }
        t.extend("cpu", c.into_iter().map(|s| s.from_source("sysinfo")));
        self.components.refresh(true);
        let trees = source_trees(&self.components);
        if let Some(temp) = cpu_temp(&trees) {
            t.add("cpu", Sensor::new("cpu_temp", "CPU temperature", SensorKind::Temperature, temp));
        }

        // Memory.
        self.sys.refresh_memory();
        t.node("memory", "Memory", HardwareKind::Memory, None);
        let (used, total) = (self.sys.used_memory() as f64, self.sys.total_memory() as f64);
        let mut m = Vec::new();
        if total > 0.0 {
            m.push(Sensor::new("memory_load", "Memory usage", SensorKind::Load, used / total * 100.0));
            m.push(Sensor::new("memory_used", "Memory used", SensorKind::DataSize, used / GB));
            m.push(Sensor::new("memory_available", "Memory available", SensorKind::DataSize, self.sys.available_memory() as f64 / GB).extra());
            m.push(Sensor::new("memory_total", "Memory total", SensorKind::DataSize, total / GB).diagnostic());
        }
        let (sw_used, sw_total) = (self.sys.used_swap() as f64, self.sys.total_swap() as f64);
        if sw_total > 0.0 {
            m.push(Sensor::new("swap_load", "Swap usage", SensorKind::Load, sw_used / sw_total * 100.0).diagnostic());
            m.push(Sensor::new("swap_used", "Swap used", SensorKind::DataSize, sw_used / GB).extra());
        }
        t.extend("memory", m.into_iter().map(|s| s.from_source("sysinfo")));

        // GPUs.
        self.gpus.read(&mut t);

        // Disks: space per volume, throughput overall.
        if fresh {
            self.disks.refresh(true);
        }
        t.node("drives", "Drives", HardwareKind::Storage, None);
        let (mut rd, mut wr) = (0u64, 0u64);
        let mut d = Vec::new();
        for disk in self.disks.list() {
            let u = disk.usage();
            rd += u.read_bytes;
            wr += u.written_bytes;
            let total = disk.total_space() as f64;
            if total <= 0.0 {
                continue;
            }
            let mount = disk.mount_point().display().to_string();
            let label = mount.trim_end_matches(['\\', '/']).to_string();
            let label = if label.is_empty() { "/".to_string() } else { label };
            let key = slug(&label);
            let key = key.trim_matches('_');
            let free = disk.available_space() as f64;
            let used = (total - free) / total * 100.0;
            d.push(Sensor::new(&format!("disk_{key}_load"), &format!("Disk {label} usage"), SensorKind::Load, used).icon("mdi:harddisk"));
            d.push(Sensor::new(&format!("disk_{key}_free"), &format!("Disk {label} free"), SensorKind::DataSize, free / GB).extra());
            d.push(Sensor::new(&format!("disk_{key}_read"), &format!("Disk {label} read"), SensorKind::DataRate, rate(u.read_bytes)).extra());
            d.push(Sensor::new(&format!("disk_{key}_write"), &format!("Disk {label} write"), SensorKind::DataRate, rate(u.written_bytes)).extra());
        }
        d.push(Sensor::new("disk_read", "Disk read", SensorKind::DataRate, rate(rd)));
        d.push(Sensor::new("disk_write", "Disk write", SensorKind::DataRate, rate(wr)));
        t.extend("drives", d.into_iter().map(|s| s.from_source("sysinfo")));

        // Network: all interfaces except loopback, and each one that has
        // carried traffic.
        if fresh {
            self.networks.refresh(true);
        }
        t.node("network", "Network", HardwareKind::Network, None);
        let (mut rx, mut tx) = (0u64, 0u64);
        let mut per = Vec::new();
        for (name, n) in self.networks.list() {
            let l = name.to_lowercase();
            if l == "lo" || l.contains("loopback") {
                continue;
            }
            rx += n.received();
            tx += n.transmitted();
            if n.total_received() + n.total_transmitted() > 0 {
                per.push((name.clone(), n.received(), n.transmitted(), n.total_received(), n.total_transmitted()));
            }
        }
        t.extend(
            "network",
            [
                Sensor::new("net_down", "Network download", SensorKind::DataRate, rate(rx)).icon("mdi:download-network"),
                Sensor::new("net_up", "Network upload", SensorKind::DataRate, rate(tx)).icon("mdi:upload-network"),
            ],
        );
        per.sort();
        for (name, r, x, tr, tt) in per {
            let id = format!("net_{}", slug(&name));
            t.node(&id, &name, HardwareKind::Network, Some("network"));
            t.extend(
                &id,
                [
                    Sensor::new(&format!("{id}_down"), "Download", SensorKind::DataRate, rate(r)).extra(),
                    Sensor::new(&format!("{id}_up"), "Upload", SensorKind::DataRate, rate(x)).extra(),
                    Sensor::new(&format!("{id}_downloaded"), "Data downloaded", SensorKind::DataSize, tr as f64 / GB).extra(),
                    Sensor::new(&format!("{id}_uploaded"), "Data uploaded", SensorKind::DataSize, tt as f64 / GB).extra(),
                ]
                .into_iter()
                .map(|s| s.from_source("sysinfo")),
            );
        }

        t.add(
            "computer",
            Sensor::new("uptime", "Uptime", SensorKind::Duration, System::uptime() as f64)
                .diagnostic()
                .icon("mdi:clock-start")
                .since(System::boot_time() as i64, "Last boot"),
        );

        // Everything else: the OS's chips, then the other tools.
        for tr in &trees {
            t.merge(tr);
        }
        t.prune();
        self.hardware = t.hardware;

        let controls = vec![Control {
            id: CTRL_ACTIVE_TIMEOUT.into(),
            name: "Count as in use for".into(),
            kind: ControlKind::Number { min: 1.0, max: 120.0, step: 1.0, unit: "min".into() },
            value: ControlValue::Number(self.active_minutes),
            help: Some("\"In use\" stays on while unlocked and there was keyboard or mouse input within this many minutes.".into()),
        }];
        Ok(Snapshot { sensors: t.sensors, controls })
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        match (id, value) {
            (CTRL_ACTIVE_TIMEOUT, ControlValue::Number(m)) if m.is_finite() => {
                self.active_minutes = m.clamp(1.0, 120.0);
                Ok(())
            }
            (CTRL_ACTIVE_TIMEOUT, _) => Err(Error::InvalidValue(id.into())),
            _ => Err(Error::UnknownControl(id.into())),
        }
    }

    fn remembered_controls(&self) -> Vec<String> {
        vec![CTRL_ACTIVE_TIMEOUT.into()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_timestamps() {
        assert_eq!(steady_since(None, 1000.4, 10.0), 990);
        // Jitter of a second keeps the previous value; new input moves it.
        assert_eq!(steady_since(Some(990), 1001.6, 10.0), 990);
        assert_eq!(steady_since(Some(990), 1100.0, 0.5), 1100);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("C:\\"), "c");
        assert_eq!(slug("/home"), "home");
        assert_eq!(slug("Tctl (k10temp)"), "tctl_k10temp");
    }

    type Node<'a> = (HardwareKind, &'a str, Vec<(&'a str, f64)>);

    fn tree(source: &'static str, nodes: Vec<Node>) -> ExtTree {
        ExtTree {
            source,
            hardware: nodes
                .into_iter()
                .enumerate()
                .map(|(i, (kind, name, s))| ExtHardware {
                    key: format!("{source}{i}"),
                    name: name.into(),
                    kind,
                    parent: None,
                    sensors: s.into_iter().map(|(l, v)| ExtSensor::new(slug(l), l, SensorKind::Temperature, v)).collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn cpu_temp_from_first_source() {
        use HardwareKind::*;
        // Linux-like: CPU chips, plus a board chip's "CPU" input (as before).
        let os = tree("hwmon", vec![(Cpu, "CPU (k10temp)", vec![("Tctl", 55.0)]), (Chip, "NCT6799", vec![("CPUTIN", 58.0), ("SYSTIN", 70.0)])]);
        assert_eq!(cpu_temp(&[os]), Some(58.0));
        // Windows-like: nothing from the OS; HWiNFO first; TjMax distances don't count.
        let os = tree("OS", vec![]);
        let hw = tree("HWiNFO", vec![(Cpu, "Ryzen", vec![("CPU (Tctl/Tdie)", 61.0), ("Core 3", 63.0), ("Core #1 Distance to TjMax", 90.0)]), (Chip, "Nuvoton", vec![("CPU", 80.0)])]);
        let lhm = tree("LibreHardwareMonitor", vec![(Cpu, "Ryzen", vec![("Core (Tctl/Tdie)", 70.0)])]);
        assert_eq!(cpu_temp(&[os.clone(), hw, lhm.clone()]), Some(63.0));
        assert_eq!(cpu_temp(&[os.clone(), lhm]), Some(70.0));
        assert_eq!(cpu_temp(&[os]), None);
    }

    #[test]
    fn this_pc_polls() {
        let mut pc = ThisPc::open();
        let snap = pc.poll().unwrap();
        for id in ["cpu_load", "memory_load", "net_down", "uptime"] {
            assert!(snap.sensor(id).is_some(), "missing {id}");
        }
        assert!(snap.sensors.iter().all(|s| s.value.is_finite()));
        // Every sensor sits on a known node; ids are unique.
        let hw = pc.hardware();
        for s in &snap.sensors {
            let h = s.hardware.as_deref().unwrap_or_else(|| panic!("{} has no node", s.id));
            assert!(hw.iter().any(|n| n.id == h), "{} on unknown node {h}", s.id);
        }
        let mut ids: Vec<&str> = snap.sensors.iter().map(|s| s.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), snap.sensors.len());
        assert!(!snap.sensor("cpu_load").is_none() && !snap.sensors.iter().find(|s| s.id == "cpu_load").unwrap().extra);
        pc.set_control(CTRL_ACTIVE_TIMEOUT, &ControlValue::Number(500.0)).unwrap();
        assert_eq!(pc.active_minutes, 120.0);
    }
}
