//! GPUs that NVML doesn't cover (AMD, Intel) on Windows: load and memory
//! use from Windows' own GPU performance counters, as Task Manager shows
//! them. No driver library and no administrator rights needed.
//!
//! * Adapters (name, vendor, LUID) from DXGI.
//! * `\GPU Engine(*)\Utilization Percentage`: one instance per process and
//!   engine (`pid_…_luid_0x…_0x…_phys_0_eng_3_engtype_3D`); an adapter's load
//!   is its busiest engine type, summed over processes.
//! * `\GPU Adapter Memory(*)\Dedicated Usage` / `Shared Usage`, bytes.

use std::collections::HashMap;
use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
};

const NVIDIA: u32 = 0x10DE;

/// One adapter's readings.
pub struct GpuReading {
    pub name: String,
    pub vendor: u32,
    /// Stable key (the adapter's LUID changes on reboot; the name doesn't).
    pub index: usize,
    pub load: Option<f64>,
    pub dedicated_used: Option<f64>,
    pub shared_used: Option<f64>,
    pub dedicated_total: f64,
}

struct Adapter {
    luid: String,
    name: String,
    vendor: u32,
    dedicated_total: u64,
}

pub struct WinGpus {
    query: isize,
    engine: isize,
    dedicated: isize,
    shared: isize,
    adapters: Vec<Adapter>,
}

// SAFETY: PDH handles are plain handles, used from one thread at a time
// (the owner is behind a mutex).
unsafe impl Send for WinGpus {}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn adapters() -> Vec<Adapter> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
    let mut out = Vec::new();
    // SAFETY: COM calls on interfaces we own.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
        for i in 0.. {
            let Ok(a) = factory.EnumAdapters1(i) else { break };
            let Ok(d) = a.GetDesc1() else { continue };
            if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let n = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
            let name = String::from_utf16_lossy(&d.Description[..n]).trim().to_string();
            let luid = format!("luid_0x{:08x}_0x{:08x}", d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart);
            // The same adapter can be listed once per output; keep one.
            if out.iter().any(|x: &Adapter| x.luid == luid) {
                continue;
            }
            out.push(Adapter { luid, name, vendor: d.VendorId, dedicated_total: d.DedicatedVideoMemory as u64 });
        }
    }
    out
}

impl WinGpus {
    /// Adapters other than NVIDIA's (NVML reads those), or `None` if there
    /// are none.
    pub fn open() -> Option<WinGpus> {
        let adapters: Vec<Adapter> = adapters().into_iter().filter(|a| a.vendor != NVIDIA && a.vendor != 0x1414).collect();
        if adapters.is_empty() {
            return None;
        }
        let mut q = 0isize;
        // SAFETY: plain PDH calls with valid out-pointers.
        unsafe {
            if PdhOpenQueryW(std::ptr::null(), 0, &mut q as *mut isize as *mut _) != 0 {
                return None;
            }
            let add = |path: &str| {
                let mut c = 0isize;
                let p = wide(path);
                (PdhAddEnglishCounterW(q as _, p.as_ptr(), 0, &mut c as *mut isize as *mut _) == 0).then_some(c)
            };
            let (Some(engine), Some(dedicated), Some(shared)) = (
                add(r"\GPU Engine(*)\Utilization Percentage"),
                add(r"\GPU Adapter Memory(*)\Dedicated Usage"),
                add(r"\GPU Adapter Memory(*)\Shared Usage"),
            ) else {
                PdhCloseQuery(q as _);
                return None;
            };
            // Rates need a first sample.
            PdhCollectQueryData(q as _);
            Some(WinGpus { query: q, engine, dedicated, shared, adapters })
        }
    }

    fn values(counter: isize) -> Vec<(String, f64)> {
        let mut out = Vec::new();
        let (mut size, mut count) = (0u32, 0u32);
        // SAFETY: sized first, then filled; items are read within `count`.
        unsafe {
            let r = PdhGetFormattedCounterArrayW(counter as _, PDH_FMT_DOUBLE, &mut size, &mut count, std::ptr::null_mut());
            if r != PDH_MORE_DATA || size == 0 {
                return out;
            }
            // u64 storage: the items hold f64s and pointers, so the buffer
            // must be 8-byte aligned (a Vec<u8> only promises 1).
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(counter as _, PDH_FMT_DOUBLE, &mut size, &mut count, items) != 0 {
                return out;
            }
            let fits = buf.len() * 8 / std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            for i in 0..(count as usize).min(fits) {
                let it = &*items.add(i);
                if it.FmtValue.CStatus != 0 {
                    continue;
                }
                let mut n = 0;
                while *it.szName.add(n) != 0 {
                    n += 1;
                }
                let name = String::from_utf16_lossy(std::slice::from_raw_parts(it.szName, n)).to_lowercase();
                out.push((name, it.FmtValue.Anonymous.doubleValue));
            }
        }
        out
    }

    pub fn read(&mut self) -> Vec<GpuReading> {
        // SAFETY: valid query handle.
        if unsafe { PdhCollectQueryData(self.query as _) } != 0 {
            return Vec::new();
        }
        // luid → engine type → summed utilisation.
        let mut load: HashMap<&str, HashMap<String, f64>> = HashMap::new();
        let engine = Self::values(self.engine);
        for a in &self.adapters {
            let e = load.entry(a.luid.as_str()).or_default();
            for (inst, v) in engine.iter().filter(|(i, _)| i.contains(&a.luid)) {
                let ty = inst.split("engtype_").nth(1).unwrap_or("").to_string();
                *e.entry(ty).or_default() += v;
            }
        }
        let mem = |counter: isize| -> HashMap<String, f64> {
            let mut m: HashMap<String, f64> = HashMap::new();
            for (inst, v) in Self::values(counter) {
                if let Some(a) = self.adapters.iter().find(|a| inst.contains(&a.luid)) {
                    *m.entry(a.luid.clone()).or_default() += v;
                }
            }
            m
        };
        let (dedicated, shared) = (mem(self.dedicated), mem(self.shared));
        const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
        self.adapters
            .iter()
            .enumerate()
            .map(|(index, a)| GpuReading {
                name: a.name.clone(),
                vendor: a.vendor,
                index,
                load: load.get(a.luid.as_str()).and_then(|e| e.values().copied().reduce(f64::max)).map(|l| l.clamp(0.0, 100.0)),
                dedicated_used: dedicated.get(&a.luid).map(|b| b / GIB),
                shared_used: shared.get(&a.luid).map(|b| b / GIB),
                dedicated_total: a.dedicated_total as f64 / GIB,
            })
            .collect()
    }
}

impl Drop for WinGpus {
    fn drop(&mut self) {
        // SAFETY: valid query handle, closed once.
        unsafe { PdhCloseQuery(self.query as _) };
    }
}
