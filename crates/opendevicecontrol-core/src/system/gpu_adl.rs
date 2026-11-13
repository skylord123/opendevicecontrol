//! AMD GPUs on Windows through the AMD Display Library (`atiadlxx.dll`,
//! installed with every AMD graphics driver), loaded at runtime like
//! NVIDIA's NVML. No administrator rights needed.
//!
//! Overdrive 8 GPUs (Vega 20, RDNA and the RDNA iGPUs) report everything
//! through `ADL2_New_QueryPMLogData_Get`: clocks, temperatures, fan, power,
//! voltages and activity. Older Overdrive N GPUs give their temperature
//! and performance status. Layouts as in AMD's ADL SDK and
//! LibreHardwareMonitor's `AtiAdlxx.cs` (MPL-2.0).

use crate::model::SensorKind;
use std::ffi::c_void;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

const ADL_MAX_PATH: usize = 256;
const ADL_PMLOG_MAX_SENSORS: usize = 256;
const AMD_VENDOR: i32 = 0x1002;
/// `sizeof(AdapterInfo)`.
const ADAPTER_INFO_SIZE: usize = 4 + 4 + ADL_MAX_PATH + 16 + ADL_MAX_PATH * 2 + 8 + ADL_MAX_PATH * 3 + 4;

type Alloc = unsafe extern "C" fn(i32) -> *mut c_void;
type Create = unsafe extern "C" fn(Alloc, i32, *mut *mut c_void) -> i32;
type NumAdapters = unsafe extern "C" fn(*mut c_void, *mut i32) -> i32;
type AdapterInfoGet = unsafe extern "C" fn(*mut c_void, *mut u8, i32) -> i32;
type AdapterInfoX2 = unsafe extern "C" fn(*mut c_void, *mut *mut u8) -> i32;
type Caps = unsafe extern "C" fn(*mut c_void, i32, *mut i32, *mut i32, *mut i32) -> i32;
type PmLog = unsafe extern "C" fn(*mut c_void, i32, *mut PmLogOutput) -> i32;
type OdnTemp = unsafe extern "C" fn(*mut c_void, i32, i32, *mut i32) -> i32;
type OdnPerf = unsafe extern "C" fn(*mut c_void, i32, *mut OdnPerformance) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
struct SensorData {
    supported: i32,
    value: i32,
}

#[repr(C)]
struct PmLogOutput {
    size: i32,
    sensors: [SensorData; ADL_PMLOG_MAX_SENSORS],
}

#[repr(C)]
#[derive(Default)]
struct OdnPerformance {
    core_clock: i32,
    memory_clock: i32,
    dcef_clock: i32,
    gfx_clock: i32,
    uvd_clock: i32,
    vce_clock: i32,
    gpu_activity: i32,
    levels: [i32; 6],
    bus_speed: i32,
    bus_lanes: i32,
    max_bus_lanes: i32,
    vddc: i32,
    vddci: i32,
}

/// ADL allocates through the caller; the few small buffers it asks for
/// (on create) live as long as the process.
unsafe extern "C" fn alloc(size: i32) -> *mut c_void {
    let layout = std::alloc::Layout::from_size_align(size.max(1) as usize, 8).expect("layout");
    // SAFETY: non-zero size.
    unsafe { std::alloc::alloc_zeroed(layout).cast() }
}

/// PMLog sensors: (index, id suffix, label, kind, scale).
const PMLOG: &[(usize, &str, &str, SensorKind, f64)] = &[
    (19, "load", "GPU load", SensorKind::Load, 1.0),
    (20, "memory_load", "Memory controller load", SensorKind::Load, 1.0),
    (8, "temp", "Temperature", SensorKind::Temperature, 1.0),
    (27, "hotspot", "Hot spot temperature", SensorKind::Temperature, 1.0),
    (9, "mem_temp", "Memory temperature", SensorKind::Temperature, 1.0),
    (28, "gfx_temp", "Graphics temperature", SensorKind::Temperature, 1.0),
    (29, "soc_temp", "SoC temperature", SensorKind::Temperature, 1.0),
    (10, "vr_core_temp", "VRM core temperature", SensorKind::Temperature, 1.0),
    (11, "vr_mem_temp", "VRM memory temperature", SensorKind::Temperature, 1.0),
    (24, "vr_soc_temp", "VRM SoC temperature", SensorKind::Temperature, 1.0),
    (12, "liquid_temp", "Liquid temperature", SensorKind::Temperature, 1.0),
    (73, "board_power", "Board power", SensorKind::Power, 1.0),
    (23, "power", "Package power", SensorKind::Power, 1.0),
    (30, "gfx_power", "Graphics power", SensorKind::Power, 1.0),
    (17, "soc_power", "SoC power", SensorKind::Power, 1.0),
    (1, "clock", "Core clock", SensorKind::Frequency, 1.0),
    (2, "memory_clock", "Memory clock", SensorKind::Frequency, 1.0),
    (3, "soc_clock", "SoC clock", SensorKind::Frequency, 1.0),
    (14, "fan_rpm", "Fan", SensorKind::FanSpeed, 1.0),
    (15, "fan", "Fan duty", SensorKind::Duty, 1.0),
    (21, "core_voltage", "Core voltage", SensorKind::Voltage, 0.001),
    (22, "mem_voltage", "Memory voltage", SensorKind::Voltage, 0.001),
    (16, "soc_voltage", "SoC voltage", SensorKind::Voltage, 0.001),
];

struct Adapter {
    index: i32,
    name: String,
    od_version: i32,
}

pub struct AdlGpus {
    ctx: *mut c_void,
    adapters: Vec<Adapter>,
    pmlog: Option<PmLog>,
    odn_temp: Option<OdnTemp>,
    odn_perf: Option<OdnPerf>,
}

// SAFETY: the ADL context is used from one thread at a time (the owner is
// behind a mutex).
unsafe impl Send for AdlGpus {}

/// One GPU's readings: (id suffix, label, kind, value).
pub struct AdlReading {
    pub name: String,
    pub sensors: Vec<(&'static str, &'static str, SensorKind, f64)>,
}

impl AdlGpus {
    pub fn open() -> Option<AdlGpus> {
        let dll: Vec<u16> = "atiadlxx.dll".encode_utf16().chain([0]).collect();
        // SAFETY: loading a system DLL by name and looking up its exports;
        // each is transmuted to its documented signature.
        unsafe {
            let lib = LoadLibraryW(dll.as_ptr());
            if lib.is_null() {
                return None;
            }
            let sym = |name: &str| {
                let n: Vec<u8> = name.bytes().chain([0]).collect();
                GetProcAddress(lib, n.as_ptr())
            };
            let create: Create = std::mem::transmute(sym("ADL2_Main_Control_Create")?);
            let count: NumAdapters = std::mem::transmute(sym("ADL2_Adapter_NumberOfAdapters_Get")?);
            let info: AdapterInfoGet = std::mem::transmute(sym("ADL2_Adapter_AdapterInfo_Get")?);
            let caps: Option<Caps> = sym("ADL2_Overdrive_Caps").map(|f| std::mem::transmute(f));
            let mut ctx: *mut c_void = std::ptr::null_mut();
            if create(alloc, 1, &mut ctx) != 0 || ctx.is_null() {
                return None;
            }
            let mut n = 0;
            if count(ctx, &mut n) != 0 || n <= 0 {
                return None;
            }
            let mut buf = vec![0u8; ADAPTER_INFO_SIZE * n as usize];
            // The X2 call allocates the list itself (through `alloc`); the
            // older one fills a buffer, but some drivers leave it empty.
            let x2: Option<AdapterInfoX2> = sym("ADL2_Adapter_AdapterInfoX2_Get").map(|f| std::mem::transmute(f));
            let mut list: *mut u8 = std::ptr::null_mut();
            if let Some(f) = x2.filter(|f| f(ctx, &mut list) == 0 && !list.is_null()) {
                let _ = f;
                std::ptr::copy_nonoverlapping(list, buf.as_mut_ptr(), buf.len());
            } else if info(ctx, buf.as_mut_ptr(), buf.len() as i32) != 0 {
                return None;
            }
            let int = |b: &[u8], at: usize| i32::from_le_bytes(b[at..at + 4].try_into().unwrap_or_default());
            let text = |b: &[u8], at: usize| {
                let s = &b[at..at + ADL_MAX_PATH];
                let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
                String::from_utf8_lossy(&s[..end]).trim().to_string()
            };
            let mut adapters = Vec::new();
            let mut seen = Vec::new();
            for rec in buf.as_chunks::<ADAPTER_INFO_SIZE>().0 {
                let index = int(rec, 4);
                let (bus, dev, func, vendor) = (int(rec, 264), int(rec, 268), int(rec, 272), int(rec, 276));
                // One entry per display output: keep one per PCI device.
                // ADL writes AMD's vendor id as the decimal digits "1002".
                if (vendor != AMD_VENDOR && vendor != 1002) || seen.contains(&(bus, dev, func)) {
                    continue;
                }
                seen.push((bus, dev, func));
                let name = text(rec, 280);
                let (mut sup, mut en, mut ver) = (0, 0, 0);
                if let Some(c) = caps {
                    c(ctx, index, &mut sup, &mut en, &mut ver);
                }
                adapters.push(Adapter { index, name, od_version: ver });
            }
            if adapters.is_empty() {
                return None;
            }
            log::info!(
                "AMD GPU statistics via ADL: {}",
                adapters.iter().map(|a| format!("{} (Overdrive {})", a.name, a.od_version)).collect::<Vec<_>>().join(", ")
            );
            Some(AdlGpus {
                ctx,
                adapters,
                pmlog: sym("ADL2_New_QueryPMLogData_Get").map(|f| std::mem::transmute(f)),
                odn_temp: sym("ADL2_OverdriveN_Temperature_Get").map(|f| std::mem::transmute(f)),
                odn_perf: sym("ADL2_OverdriveN_PerformanceStatus_Get").map(|f| std::mem::transmute(f)),
            })
        }
    }

    pub fn read(&mut self) -> Vec<AdlReading> {
        let mut out = Vec::new();
        for a in &self.adapters {
            let mut sensors = Vec::new();
            // SAFETY: documented ADL calls with buffers of their sizes.
            unsafe {
                if a.od_version >= 8 {
                    if let Some(f) = self.pmlog {
                        let mut log: PmLogOutput = std::mem::zeroed();
                        log.size = std::mem::size_of::<PmLogOutput>() as i32;
                        if f(self.ctx, a.index, &mut log) == 0 {
                            for &(i, id, label, kind, scale) in PMLOG {
                                let s = log.sensors[i];
                                if s.supported != 0 {
                                    sensors.push((id, label, kind, f64::from(s.value) * scale));
                                }
                            }
                        }
                    }
                } else {
                    if let Some(f) = self.odn_temp {
                        let mut t = 0;
                        // Type 1: the GPU core; millidegrees.
                        if f(self.ctx, a.index, 1, &mut t) == 0 && t > 0 {
                            sensors.push(("temp", "Temperature", SensorKind::Temperature, f64::from(t) / 1000.0));
                        }
                    }
                    if let Some(f) = self.odn_perf {
                        let mut p = OdnPerformance::default();
                        if f(self.ctx, a.index, &mut p) == 0 {
                            sensors.push(("load", "GPU load", SensorKind::Load, f64::from(p.gpu_activity)));
                            sensors.push(("clock", "Core clock", SensorKind::Frequency, f64::from(p.core_clock) / 100.0));
                            sensors.push(("memory_clock", "Memory clock", SensorKind::Frequency, f64::from(p.memory_clock) / 100.0));
                            if p.vddc > 0 {
                                sensors.push(("core_voltage", "Core voltage", SensorKind::Voltage, f64::from(p.vddc) / 1000.0));
                            }
                        }
                    }
                }
            }
            // A sensor reading exactly 0 that can't be 0 isn't connected.
            sensors.retain(|(_, _, k, v)| !(matches!(k, SensorKind::Temperature | SensorKind::Voltage) && *v <= 0.0));
            out.push(AdlReading { name: a.name.clone(), sensors });
        }
        out
    }
}
