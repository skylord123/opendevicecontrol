//! Windows readers for [`super::external`]: OpenDeviceControl's sensor service (see
//! [`super::service`]), PawnIO in-process (opt-in, needs administrator),
//! HWiNFO shared memory and LibreHardwareMonitor (web server, or WMI for old
//! versions).
//!
//! Results are cached for a moment so This PC and the fan-curve "system"
//! source, which both poll, don't query the other tools twice.

use super::external::{self, ExtTree};
use crate::smbus::pawnio::{Module, NamedMutex};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How long a set of readings is reused.
const CACHE: Duration = Duration::from_millis(1500);
/// Retry a source that was not there after this long.
const RETRY: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Read the CPU's thermal registers through PawnIO (needs PawnIO,
    /// its modules and administrator rights).
    pub pawnio: bool,
    /// LibreHardwareMonitor web server port.
    pub lhm_port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { pawnio: false, lhm_port: 8085 }
    }
}

enum Pawn {
    Off,
    Ready(PawnCpu),
    Failed(String),
}

struct State {
    settings: Settings,
    at: Option<Instant>,
    trees: Vec<ExtTree>,
    pawn: Pawn,
    lhm_http_next: Instant,
    lhm_wmi_next: Instant,
    /// When to look for the sensor service again (it isn't installed).
    service_next: Instant,
    /// The sensor service's status lines, or why it can't be read.
    service: Result<Vec<String>, String>,
    /// Sources that answered on the last refresh.
    active: Vec<String>,
}

fn state() -> MutexGuard<'static, State> {
    static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
        let now = Instant::now();
        Mutex::new(State {
            settings: Settings::default(),
            at: None,
            trees: Vec::new(),
            pawn: Pawn::Off,
            lhm_http_next: now,
            lhm_wmi_next: now,
            service_next: now,
            service: Err("not installed".into()),
            active: Vec::new(),
        })
    });
    // A panic while refreshing leaves usable data behind; ignore poisoning.
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Apply settings (from the config file). Switching PawnIO on retries it.
pub fn configure(settings: Settings) {
    let mut s = state();
    if s.settings != settings {
        if s.settings.pawnio != settings.pawnio {
            s.pawn = Pawn::Off;
        }
        s.settings = settings;
        s.at = None;
        s.lhm_http_next = Instant::now();
    }
}

/// Look for every source again on the next read.
pub fn rescan() {
    let mut s = state();
    let now = Instant::now();
    s.at = None;
    s.service_next = now;
    s.lhm_http_next = now;
    s.lhm_wmi_next = now;
}

/// Hardware trees from the available sources, in priority order: PawnIO,
/// then HWiNFO or (when HWiNFO isn't sharing) LibreHardwareMonitor.
pub fn trees() -> Vec<ExtTree> {
    let mut s = state();
    if s.at.is_none_or(|t| t.elapsed() >= CACHE) {
        refresh(&mut s);
    }
    s.trees.clone()
}

/// One line per source, for Settings and `--probe`.
pub fn status() -> Vec<String> {
    let mut s = state();
    if s.at.is_none() {
        refresh(&mut s);
    }
    let mut v = Vec::new();
    match &s.service {
        Ok(lines) => v.extend(lines.iter().map(|l| format!("Sensor service: {l}"))),
        Err(e) => v.push(format!("Sensor service: {e}")),
    }
    match &s.pawn {
        Pawn::Off if s.settings.pawnio || s.service.is_ok() => {}
        Pawn::Off => v.push("PawnIO: off".to_string()),
        Pawn::Ready(c) => v.push(format!("PawnIO: reading {}", c.describe())),
        Pawn::Failed(e) => v.push(format!("PawnIO: {e}")),
    }
    let others: Vec<&String> = s.active.iter().filter(|a| *a != "PawnIO" && *a != "Sensor service").collect();
    if others.is_empty() {
        v.push("HWiNFO / LibreHardwareMonitor: not running or not sharing sensors".into());
    } else {
        v.extend(others.iter().map(|a| format!("{a}: connected")));
    }
    v
}

fn refresh(s: &mut State) {
    let mut out = Vec::new();
    let mut active = Vec::new();

    // The sensor service does the privileged reads (PawnIO) for us.
    let mut from_service = false;
    if Instant::now() >= s.service_next {
        match super::service::fetch() {
            Ok(snap) => {
                from_service = true;
                active.push("Sensor service".to_string());
                s.service = Ok(snap.status);
                out.extend(snap.trees.into_iter().map(|t| t.into_tree()).filter(|t| !t.hardware.is_empty()));
            }
            Err(super::service::FetchError::Absent) => {
                s.service = Err("not installed".into());
                s.service_next = Instant::now() + RETRY;
            }
            Err(super::service::FetchError::Failed(e)) => s.service = Err(e),
        }
    }

    if s.settings.pawnio && !from_service {
        if matches!(s.pawn, Pawn::Off) {
            s.pawn = match PawnCpu::open() {
                Ok(c) => {
                    log::info!("PawnIO: reading {}", c.describe());
                    Pawn::Ready(c)
                }
                Err(e) => {
                    log::warn!("PawnIO CPU temperature: {e}");
                    Pawn::Failed(e)
                }
            };
        }
        if let Pawn::Ready(c) = &s.pawn {
            let t = c.read();
            if !t.hardware.is_empty() {
                active.push("PawnIO".to_string());
                out.push(t);
            }
        }
    }

    // HWiNFO and LibreHardwareMonitor see the same hardware: use one.
    let hw = hwinfo();
    let have_hwinfo = hw.is_some();
    if let Some(t) = hw {
        active.push(format!("HWiNFO ({} sensors)", t.sensor_count()));
        out.push(t);
    }

    let now = Instant::now();
    let mut lhm = None;
    if !have_hwinfo && now >= s.lhm_http_next {
        lhm = lhm_http(s.settings.lhm_port);
        if let Some(t) = &lhm {
            active.push(format!("LibreHardwareMonitor web server ({} sensors)", t.sensor_count()));
        } else {
            s.lhm_http_next = now + RETRY;
        }
    }
    if !have_hwinfo && lhm.is_none() && now >= s.lhm_wmi_next {
        lhm = wmi::lhm();
        if let Some(t) = &lhm {
            active.push(format!("LibreHardwareMonitor WMI ({} sensors)", t.sensor_count()));
        } else {
            s.lhm_wmi_next = now + RETRY * 3;
        }
    }
    out.extend(lhm);

    s.trees = out;
    s.active = active;
    s.at = Some(Instant::now());
}

// ---------------------------------------------------------------------------
// HWiNFO

fn hwinfo() -> Option<ExtTree> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Memory::{
        MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, VirtualQuery, FILE_MAP_READ, MEMORY_BASIC_INFORMATION,
        MEMORY_MAPPED_VIEW_ADDRESS,
    };
    let name: Vec<u16> = "Global\\HWiNFO_SENS_SM2".encode_utf16().chain([0]).collect();
    // SAFETY: plain Win32 calls; the view is only read within its region
    // size and unmapped before returning.
    unsafe {
        let map = OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr());
        if map.is_null() {
            return None;
        }
        let view = MapViewOfFile(map, FILE_MAP_READ, 0, 0, 0);
        CloseHandle(map);
        if view.Value.is_null() {
            return None;
        }
        let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
        let ok = VirtualQuery(view.Value, &mut mbi, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) != 0;
        let copy = ok.then(|| {
            let region = std::slice::from_raw_parts(view.Value as *const u8, mbi.RegionSize);
            // HWiNFO asks readers to hold its mutex while copying. An
            // unprivileged process may not be able to open it; reading
            // without it (what most clients do) at worst tears one sample.
            let mutex = NamedMutex::new("Global\\HWiNFO_SM2_MUTEX");
            let _guard = mutex.as_ref().and_then(|m| m.lock_for(50));
            let size = external::hwinfo_size(region).unwrap_or(region.len()).min(region.len());
            region[..size].to_vec()
        });
        UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: view.Value });
        external::parse_hwinfo(&copy?)
    }
}

// ---------------------------------------------------------------------------
// LibreHardwareMonitor web server ("Options → Remote Web Server → Run").

fn lhm_http(port: u16) -> Option<ExtTree> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(200)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    s.set_write_timeout(Some(Duration::from_secs(1))).ok()?;
    // HTTP/1.0: no chunked encoding, the server closes when done.
    s.write_all(b"GET /data.json HTTP/1.0\r\nHost: 127.0.0.1\r\nAccept: application/json\r\n\r\n").ok()?;
    let mut resp = Vec::new();
    s.take(16 << 20).read_to_end(&mut resp).ok()?;
    let body = external::http_body(&resp)?;
    external::parse_lhm_json(std::str::from_utf8(body).ok()?)
}

// ---------------------------------------------------------------------------
// LibreHardwareMonitor WMI (`root\LibreHardwareMonitor`, versions ≤ 0.9.4).

mod wmi {
    use super::external::{self, ExtTree, LhmHardware, LhmSensor};
    use std::cell::Cell;
    use windows::core::{w, BSTR, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, EOAC_NONE,
        RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
    };
    use windows::Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE};
    use windows::Win32::System::Variant::{VariantClear, VARIANT, VT_BSTR, VT_R4, VT_R8};
    use windows::Win32::System::Wmi::{
        IWbemClassObject, IWbemLocator, WbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
    };

    thread_local! {
        static COM: Cell<bool> = const { Cell::new(false) };
    }

    pub fn lhm() -> Option<ExtTree> {
        COM.with(|c| {
            if !c.get() {
                // SAFETY: once per thread. S_FALSE / RPC_E_CHANGED_MODE
                // (already initialised) still leave COM usable.
                let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                c.set(true);
            }
        });
        // SAFETY: COM calls on interfaces we own; every VARIANT is cleared.
        unsafe { query().ok() }
    }

    unsafe fn query() -> windows::core::Result<ExtTree> {
        let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
        let empty = BSTR::new();
        let svc = locator.ConnectServer(&BSTR::from("root\\LibreHardwareMonitor"), &empty, &empty, &empty, 0, &empty, None)?;
        CoSetProxyBlanket(&svc, RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE, None, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE)?;
        let rows = |q: &str| -> windows::core::Result<Vec<IWbemClassObject>> {
            let e = svc.ExecQuery(&BSTR::from("WQL"), &BSTR::from(q), WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, None)?;
            let mut out = Vec::new();
            for _ in 0..4096 {
                let mut obj = [None];
                let mut n = 0;
                let _ = e.Next(2000, &mut obj, &mut n);
                let [Some(o)] = obj else { break };
                if n == 0 {
                    break;
                }
                out.push(o);
            }
            Ok(out)
        };
        let hardware = rows("SELECT Identifier, Name, Parent FROM Hardware")?
            .iter()
            .filter_map(|o| {
                Some(LhmHardware {
                    id: string(o, w!("Identifier"))?,
                    name: string(o, w!("Name"))?,
                    parent: string(o, w!("Parent")).filter(|p| !p.is_empty()),
                })
            })
            .collect();
        let sensors = rows("SELECT Identifier, Name, SensorType, Value, Parent FROM Sensor")?
            .iter()
            .filter_map(|o| {
                Some(LhmSensor {
                    id: string(o, w!("Identifier"))?,
                    name: string(o, w!("Name"))?,
                    ty: string(o, w!("SensorType"))?,
                    value: number(o, w!("Value"))?,
                    unit: None,
                    parent: string(o, w!("Parent"))?,
                })
            })
            .collect();
        Ok(external::lhm_tree(hardware, sensors))
    }

    /// Installed memory modules ("A-DATA Technology AX5U6000C3016G-BLABBK"),
    /// in the BIOS's order.
    pub fn memory_modules() -> Vec<String> {
        COM.with(|c| {
            if !c.get() {
                // SAFETY: as in `lhm`.
                let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                c.set(true);
            }
        });
        // SAFETY: as in `query`.
        unsafe { modules().unwrap_or_default() }
    }

    unsafe fn modules() -> windows::core::Result<Vec<String>> {
        let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
        let empty = BSTR::new();
        let svc = locator.ConnectServer(&BSTR::from("root\\cimv2"), &empty, &empty, &empty, 0, &empty, None)?;
        CoSetProxyBlanket(&svc, RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE, None, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE)?;
        let e = svc.ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from("SELECT Manufacturer, PartNumber FROM Win32_PhysicalMemory"),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            None,
        )?;
        let mut out = Vec::new();
        for _ in 0..64 {
            let mut obj = [None];
            let mut n = 0;
            let _ = e.Next(2000, &mut obj, &mut n);
            let [Some(o)] = obj else { break };
            if n == 0 {
                break;
            }
            let maker = string(&o, w!("Manufacturer")).unwrap_or_default();
            let part = string(&o, w!("PartNumber")).unwrap_or_default();
            out.push(format!("{} {}", maker.trim(), part.trim()).trim().to_string());
        }
        Ok(out)
    }

    unsafe fn get(o: &IWbemClassObject, prop: PCWSTR) -> Option<VARIANT> {
        let mut v = VARIANT::default();
        o.Get(prop, 0, &mut v, None, None).ok()?;
        Some(v)
    }

    unsafe fn string(o: &IWbemClassObject, prop: PCWSTR) -> Option<String> {
        let mut v = get(o, prop)?;
        let s = (v.Anonymous.Anonymous.vt == VT_BSTR).then(|| v.Anonymous.Anonymous.Anonymous.bstrVal.to_string());
        let _ = VariantClear(&mut v);
        s
    }

    unsafe fn number(o: &IWbemClassObject, prop: PCWSTR) -> Option<f64> {
        let mut v = get(o, prop)?;
        let inner = &v.Anonymous.Anonymous;
        let n = match inner.vt {
            VT_R4 => Some(f64::from(inner.Anonymous.fltVal)),
            VT_R8 => Some(inner.Anonymous.dblVal),
            _ => None,
        };
        let _ = VariantClear(&mut v);
        n
    }
}

// ---------------------------------------------------------------------------
// The sensor service's readings (it runs as LocalSystem).

/// A reader (or why it failed to open) and when it was opened.
type Retrying<T> = (Option<Result<T, String>>, Option<Instant>);

/// PawnIO readings, for [`super::service::snapshot`]: status lines and
/// trees. A reader that failed to open is retried every minute.
pub(super) fn service_readings() -> (Vec<String>, Vec<ExtTree>) {
    static CPU: LazyLock<Mutex<(Pawn, Option<Instant>)>> = LazyLock::new(|| Mutex::new((Pawn::Off, None)));
    let mut g = CPU.lock().unwrap_or_else(|e| e.into_inner());
    let retry = match (&g.0, g.1) {
        (Pawn::Off, _) => true,
        (Pawn::Failed(_), Some(t)) => t.elapsed() >= Duration::from_secs(60),
        _ => false,
    };
    if retry {
        g.0 = match PawnCpu::open() {
            Ok(c) => {
                log::info!("PawnIO: reading {}", c.describe());
                Pawn::Ready(c)
            }
            Err(e) => {
                log::warn!("PawnIO CPU temperature: {e}");
                Pawn::Failed(e)
            }
        };
        g.1 = Some(Instant::now());
    }
    let (mut status, mut trees) = match &g.0 {
        Pawn::Ready(c) => (vec![format!("CPU: {}", c.describe())], vec![c.read()]),
        Pawn::Failed(e) => (vec![format!("CPU: {e}")], Vec::new()),
        Pawn::Off => (Vec::new(), Vec::new()),
    };
    drop(g);

    // Power, clocks and voltages per core.
    static MSR: LazyLock<Mutex<Option<Result<CpuMsr, String>>>> = LazyLock::new(|| Mutex::new(None));
    let mut m = MSR.lock().unwrap_or_else(|e| e.into_inner());
    if m.is_none() {
        let r = CpuMsr::open();
        match &r {
            Ok(c) => log::info!("PawnIO: per-core power and clocks of {} cores", c.cores.len()),
            Err(e) => log::warn!("PawnIO CPU power and clocks: {e}"),
        }
        *m = Some(r);
    }
    match m.as_mut() {
        Some(Ok(c)) => {
            let sensors = c.read();
            if !sensors.is_empty() {
                status.push(format!("CPU power and clocks: {} cores", c.cores.len()));
                match trees.iter_mut().find_map(|t| t.hardware.iter_mut().find(|h| h.kind == crate::model::HardwareKind::Cpu)) {
                    Some(h) => h.sensors.extend(sensors),
                    None => trees.push(ExtTree {
                        source: "PawnIO",
                        hardware: vec![external::ExtHardware {
                            key: "pawnio_cpu".into(),
                            name: c.brand.clone(),
                            kind: crate::model::HardwareKind::Cpu,
                            parent: None,
                            sensors,
                        }],
                    }),
                }
            }
        }
        Some(Err(e)) => status.push(format!("CPU power and clocks: {e}")),
        None => {}
    }
    drop(m);

    // Memory modules' temperature sensors (SMBus).
    static DIMMS: LazyLock<Mutex<Retrying<Dimms>>> = LazyLock::new(|| Mutex::new((None, None)));
    let mut d = DIMMS.lock().unwrap_or_else(|e| e.into_inner());
    let retry = match (&d.0, d.1) {
        (None, _) => true,
        (Some(Err(_)), Some(t)) => t.elapsed() >= Duration::from_secs(600),
        _ => false,
    };
    if retry {
        let r = Dimms::open();
        match &r {
            Ok(x) => log::info!("PawnIO: {} memory module temperature sensor(s)", x.count()),
            Err(e) => log::warn!("PawnIO memory temperatures: {e}"),
        }
        d.0 = Some(r);
        d.1 = Some(Instant::now());
    }
    match &mut d.0 {
        Some(Ok(x)) => {
            let t = x.read();
            status.push(format!("Memory: {} module temperature sensor(s)", x.count()));
            if !t.hardware.is_empty() {
                trees.push(t);
            }
        }
        Some(Err(e)) => status.push(format!("Memory: {e}")),
        None => {}
    }
    drop(d);

    // Drive health (SMART / NVMe log): every 30 s, which is plenty for
    // what changes slowly and keeps hard disks from being queried often.
    static DRIVES: LazyLock<Mutex<Option<(Instant, ExtTree)>>> = LazyLock::new(|| Mutex::new(None));
    let mut dr = DRIVES.lock().unwrap_or_else(|e| e.into_inner());
    if dr.as_ref().is_none_or(|(t, _)| t.elapsed() >= Duration::from_secs(30)) {
        *dr = Some((Instant::now(), super::windows_drives::read()));
    }
    if let Some((_, t)) = dr.as_ref() {
        status.push(format!("Drives: {} with health data", t.hardware.len()));
        if !t.hardware.is_empty() {
            trees.push(t.clone());
        }
    }
    drop(dr);

    // ASUS boards' embedded-controller sensors.
    static EC: LazyLock<Mutex<Retrying<AsusEc>>> = LazyLock::new(|| Mutex::new((None, None)));
    let mut e = EC.lock().unwrap_or_else(|x| x.into_inner());
    if e.0.is_none() {
        let r = AsusEc::open();
        if let Ok(x) = &r {
            log::info!("PawnIO: ASUS EC, {} register(s)", x.board.registers().len());
        }
        e.0 = Some(r);
        e.1 = Some(Instant::now());
    }
    if let Some(Ok(x)) = &mut e.0 {
        let t = x.read();
        let n = t.sensor_count();
        status.push(format!("ASUS EC: {n} sensors"));
        if n > 0 {
            trees.push(t);
        }
    }
    drop(e);

    // The motherboard's Super I/O chip.
    static SIO: LazyLock<Mutex<Retrying<SuperIo>>> = LazyLock::new(|| Mutex::new((None, None)));
    let mut g = SIO.lock().unwrap_or_else(|e| e.into_inner());
    let retry = match (&g.0, g.1) {
        (None, _) => true,
        (Some(Err(_)), Some(t)) => t.elapsed() >= Duration::from_secs(60),
        _ => false,
    };
    if retry {
        let r = SuperIo::open();
        if let Err(e) = &r {
            log::warn!("PawnIO motherboard sensors: {e}");
        }
        g.0 = Some(r);
        g.1 = Some(Instant::now());
    }
    match &mut g.0 {
        Some(Ok(sio)) => {
            let t = sio.read();
            let n = t.sensor_count();
            status.push(format!("Motherboard: {} ({n} sensors)", sio.names()));
            if n > 0 {
                trees.push(t);
            }
        }
        Some(Err(e)) => status.push(format!("Motherboard: {e}")),
        None => {}
    }
    (status, trees)
}

// ---------------------------------------------------------------------------
// PawnIO: CPU power, clocks and voltages (model-specific registers).

use super::cpu_msr::{self, CoreSample};
use windows_sys::Win32::System::SystemInformation::GROUP_AFFINITY;

/// Every physical core's first logical processor, as an affinity.
fn physical_cores() -> Vec<GROUP_AFFINITY> {
    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformationEx, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
    };
    let mut len = 0u32;
    // SAFETY: first call sizes the buffer, the second fills it; records are
    // walked within the returned length.
    unsafe {
        GetLogicalProcessorInformationEx(RelationProcessorCore, std::ptr::null_mut(), &mut len);
        let mut buf = vec![0u8; len as usize];
        if GetLogicalProcessorInformationEx(RelationProcessorCore, buf.as_mut_ptr().cast(), &mut len) == 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut at = 0usize;
        // Records are as long as their `Size` (a core's record is shorter
        // than the whole union): walk by the 8-byte header.
        while at + 8 <= len as usize {
            let rec = &*(buf.as_ptr().add(at) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX);
            if rec.Size == 0 || at + rec.Size as usize > len as usize {
                break;
            }
            if rec.Relationship == RelationProcessorCore && rec.Anonymous.Processor.GroupCount > 0 {
                let g = rec.Anonymous.Processor.GroupMask[0];
                if g.Mask != 0 {
                    // The core's first logical processor.
                    let first = g.Mask & g.Mask.wrapping_neg();
                    out.push(GROUP_AFFINITY { Mask: first, Group: g.Group, Reserved: [0; 3] });
                }
            }
            if rec.Size == 0 {
                break;
            }
            at += rec.Size as usize;
        }
        out
    }
}

/// Runs the calling thread on `core` until dropped.
struct Pinned(Option<GROUP_AFFINITY>);

impl Pinned {
    fn to(core: &GROUP_AFFINITY) -> Pinned {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadGroupAffinity};
        let mut prev: GROUP_AFFINITY = unsafe { std::mem::zeroed() };
        // SAFETY: valid pseudo-handle and affinity structs.
        let ok = unsafe { SetThreadGroupAffinity(GetCurrentThread(), core, &mut prev) } != 0;
        Pinned(ok.then_some(prev))
    }
}

impl Drop for Pinned {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadGroupAffinity};
        if let Some(prev) = &self.0 {
            // SAFETY: as above.
            unsafe { SetThreadGroupAffinity(GetCurrentThread(), prev, std::ptr::null_mut()) };
        }
    }
}

struct CpuMsr {
    module: Module,
    amd: bool,
    brand: String,
    cores: Vec<GROUP_AFFINITY>,
    /// Joules per energy count.
    unit: f64,
    /// Base (P0) clock, MHz: MPERF ticks at this rate.
    base_mhz: f64,
    /// Intel TjMax, °C.
    tjmax: f64,
    /// The P-state VID means a voltage (not on Zen 4 / Zen 5, whose SVI3
    /// encoding differs: the old formula reads ~0.4 V there).
    vid: bool,
    prev: Option<MsrSample>,
}

#[derive(Clone)]
struct MsrSample {
    at: Instant,
    cores: Vec<Option<CoreSample>>,
    package: Option<u64>,
    pp0: Option<u64>,
    pp1: Option<u64>,
    dram: Option<u64>,
}

impl CpuMsr {
    fn open() -> Result<CpuMsr, String> {
        let id = cpuid().ok_or("needs an x86-64 CPU")?;
        let amd = &id.vendor == b"AuthenticAMD";
        let module = match &id.vendor {
            b"AuthenticAMD" => Module::open("AMDFamily17.bin")?.ok_or("this AMD CPU is not supported by the AMDFamily17 module")?,
            b"GenuineIntel" => Module::open("IntelMSR.bin")?.ok_or("the IntelMSR module does not support this CPU")?,
            v => return Err(format!("unsupported CPU vendor {}", String::from_utf8_lossy(v))),
        };
        let cores = physical_cores();
        if cores.is_empty() {
            return Err("could not list the CPU cores".into());
        }
        let read = |msr: u32| -> Option<u64> {
            let mut out = [0u64; 1];
            module.execute("ioctl_read_msr", &[u64::from(msr)], &mut out).ok().map(|_| out[0])
        };
        let (unit, base_mhz, tjmax) = {
            let _p = Pinned::to(&cores[0]);
            if amd {
                let unit = read(cpu_msr::amd::PWR_UNIT).map(cpu_msr::energy_unit).ok_or("reading the power unit failed")?;
                let base = read(cpu_msr::amd::PSTATE_0).and_then(|m| cpu_msr::amd::pstate_mhz(id.family, m)).unwrap_or(0.0);
                (unit, base, 0.0)
            } else {
                let unit = read(cpu_msr::intel::RAPL_POWER_UNIT).map(cpu_msr::energy_unit).unwrap_or(0.0);
                let base = read(cpu_msr::intel::PLATFORM_INFO).and_then(cpu_msr::intel::base_mhz).unwrap_or(0.0);
                let tjmax = match read(external::MSR_IA32_TEMPERATURE_TARGET).map(|t| (t >> 16) & 0xFF) {
                    Some(t) if t > 0 => t as f64,
                    _ => 100.0,
                };
                (unit, base, tjmax)
            }
        };
        let zen4_or_later = amd && (id.family >= 0x1A || (id.family == 0x19 && matches!(id.model, 0x10..=0x1F | 0x60..=0xAF)));
        Ok(CpuMsr { module, amd, brand: id.brand, cores, unit, base_mhz, tjmax, vid: !zen4_or_later, prev: None })
    }

    fn msr(&self, msr: u32) -> Option<u64> {
        let mut out = [0u64; 1];
        self.module.execute("ioctl_read_msr", &[u64::from(msr)], &mut out).ok().map(|_| out[0])
    }

    fn read(&mut self) -> Vec<external::ExtSensor> {
        use crate::model::SensorKind;
        use external::ExtSensor;
        let mut out = Vec::new();
        let mut cores = Vec::with_capacity(self.cores.len());
        let mut vids = Vec::new();
        let mut temps = Vec::new();
        let (mut package, mut pp0, mut pp1, mut dram) = (None, None, None, None);
        for (i, core) in self.cores.iter().enumerate() {
            let _p = Pinned::to(core);
            if self.amd {
                let (a, m) = (self.msr(cpu_msr::amd::APERF_RO), self.msr(cpu_msr::amd::MPERF_RO));
                let energy = self.msr(cpu_msr::amd::CORE_ENERGY_STAT);
                cores.push(a.zip(m).map(|(aperf, mperf)| CoreSample { aperf, mperf, energy }));
                vids.push(self.msr(cpu_msr::amd::HW_PSTATE_STATUS).filter(|_| self.vid).map(cpu_msr::amd::vid_volts));
                if i == 0 {
                    package = self.msr(cpu_msr::amd::PKG_ENERGY_STAT);
                }
            } else {
                let (a, m) = (self.msr(cpu_msr::intel::APERF), self.msr(cpu_msr::intel::MPERF));
                cores.push(a.zip(m).map(|(aperf, mperf)| CoreSample { aperf, mperf, energy: None }));
                vids.push(self.msr(cpu_msr::intel::PERF_STATUS).and_then(cpu_msr::intel::vid_volts));
                temps.push(self.msr(cpu_msr::intel::THERM_STATUS).and_then(|t| cpu_msr::intel::core_temp(t, self.tjmax)));
                if i == 0 {
                    package = self.msr(cpu_msr::intel::PKG_ENERGY_STATUS);
                    pp0 = self.msr(cpu_msr::intel::PP0_ENERGY_STATUS);
                    pp1 = self.msr(cpu_msr::intel::PP1_ENERGY_STATUS);
                    dram = self.msr(cpu_msr::intel::DRAM_ENERGY_STATUS);
                }
            }
        }
        let now = MsrSample { at: Instant::now(), cores, package, pp0, pp1, dram };

        for (i, t) in temps.iter().enumerate() {
            if let Some(t) = t.filter(|t| external::plausible(*t)) {
                out.push(ExtSensor::new(format!("pawnio_cpu_core{}_temp", i + 1), format!("Core #{}", i + 1), SensorKind::Temperature, t));
            }
        }
        if let Some(prev) = &self.prev {
            let secs = now.at.duration_since(prev.at).as_secs_f64();
            let w = |p: Option<u64>, c: Option<u64>| p.zip(c).and_then(|(p, c)| cpu_msr::watts(p, c, self.unit, secs));
            let round = |x: f64| (x * 100.0).round() / 100.0;
            if let Some(p) = w(prev.package, now.package) {
                out.push(ExtSensor::new("pawnio_cpu_package_power", "Package power", SensorKind::Power, round(p)));
            }
            if let Some(p) = w(prev.pp0, now.pp0) {
                out.push(ExtSensor::new("pawnio_cpu_cores_power", "Cores power", SensorKind::Power, round(p)));
            }
            if let Some(p) = w(prev.pp1, now.pp1) {
                out.push(ExtSensor::new("pawnio_cpu_igpu_power", "Integrated graphics power", SensorKind::Power, round(p)));
            }
            if let Some(p) = w(prev.dram, now.dram) {
                out.push(ExtSensor::new("pawnio_cpu_dram_power", "Memory power", SensorKind::Power, round(p)));
            }
            for (i, (p, c)) in prev.cores.iter().zip(&now.cores).enumerate() {
                let (Some(p), Some(c)) = (p, c) else { continue };
                let n = i + 1;
                if let Some(watts) = w(p.energy, c.energy) {
                    out.push(ExtSensor::new(format!("pawnio_cpu_core{n}_power"), format!("Core #{n} power"), SensorKind::Power, round(watts)));
                }
                if let Some((run, eff)) = cpu_msr::core_clocks(*p, *c, self.base_mhz, secs) {
                    if self.base_mhz > 0.0 {
                        out.push(ExtSensor::new(format!("pawnio_cpu_core{n}_clock"), format!("Core #{n} clock"), SensorKind::Frequency, run.round()));
                    }
                    out.push(ExtSensor::new(
                        format!("pawnio_cpu_core{n}_effective"),
                        format!("Core #{n} effective clock"),
                        SensorKind::Frequency,
                        eff.round(),
                    ));
                }
            }
        }
        for (i, v) in vids.iter().enumerate() {
            if let Some(v) = v.filter(|v| *v > 0.0 && *v < 2.0) {
                let n = i + 1;
                out.push(ExtSensor::new(format!("pawnio_cpu_core{n}_vid"), format!("Core #{n} VID"), SensorKind::Voltage, (v * 1000.0).round() / 1000.0));
            }
        }
        self.prev = Some(now);
        out
    }
}

// ---------------------------------------------------------------------------
// PawnIO: memory module temperatures (SMBus).

struct Dimms {
    buses: Vec<(Box<dyn crate::smbus::SmBus>, Vec<super::dimm::Sensor>)>,
    /// Module names by sensor address, from the BIOS's memory table.
    names: std::collections::HashMap<u8, String>,
}

impl Dimms {
    fn open() -> Result<Dimms, String> {
        let buses = crate::smbus::pawnio::open_dram_buses().map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for mut bus in buses {
            let found = super::dimm::scan(bus.as_mut());
            if !found.is_empty() {
                out.push((bus, found));
            }
        }
        if out.is_empty() {
            return Err("no memory module temperature sensors found".into());
        }
        // The BIOS lists the installed modules in slot order, like the SPD
        // addresses: pair them up when the counts agree.
        let mut addrs: Vec<u8> = out.iter().flat_map(|(_, s)| s.iter().map(|x| x.addr())).collect();
        addrs.sort_unstable();
        let modules = wmi::memory_modules();
        let names = if modules.len() == addrs.len() { addrs.into_iter().zip(modules).collect() } else { Default::default() };
        Ok(Dimms { buses: out, names })
    }

    fn count(&self) -> usize {
        self.buses.iter().map(|(_, s)| s.len()).sum()
    }

    fn read(&mut self) -> ExtTree {
        use crate::model::{HardwareKind, SensorKind};
        let mut hardware = Vec::new();
        for (bus, sensors) in &mut self.buses {
            for s in sensors.iter() {
                let Some(t) = super::dimm::read(bus.as_mut(), *s) else { continue };
                let slot = s.slot();
                let name = match self.names.get(&s.addr()) {
                    Some(n) => format!("{n} (DIMM #{slot})"),
                    None => format!("DIMM #{slot}"),
                };
                hardware.push(external::ExtHardware {
                    key: format!("spd_dimm{slot}"),
                    name,
                    kind: HardwareKind::Memory,
                    parent: None,
                    sensors: vec![external::ExtSensor::new(format!("spd_dimm{slot}_temp"), "Temperature", SensorKind::Temperature, t)],
                });
            }
        }
        ExtTree { source: "PawnIO", hardware }
    }
}

// ---------------------------------------------------------------------------
// PawnIO: ASUS embedded-controller sensors (LpcACPIEC module, ACPI EC
// ports 0x62 / 0x66).

const EC_DATA: u64 = 0x62;
const EC_CMD: u64 = 0x66;
const EC_OBF: u8 = 0x01;
const EC_IBF: u8 = 0x02;

struct AsusEc {
    module: Module,
    mutex: Option<NamedMutex>,
    board: super::asus_ec::Board,
    name: Option<String>,
    /// Consecutive reads that timed out: after many, stop waiting so a
    /// stuck EC can't slow every poll down.
    failures: std::cell::Cell<u32>,
}

impl AsusEc {
    fn open() -> Result<AsusEc, String> {
        let (maker, product) = bios_board();
        let board = super::asus_ec::Board::find(maker.as_deref().unwrap_or(""), product.as_deref().unwrap_or(""))
            .ok_or("not an ASUS board with known EC sensors")?;
        let module = Module::open("LpcACPIEC.bin")?.ok_or("the LpcACPIEC module does not support this machine")?;
        Ok(AsusEc { module, mutex: NamedMutex::new("Global\\Access_EC"), board, name: board_name(), failures: Default::default() })
    }

    fn inb(&self, port: u64) -> Option<u8> {
        lpc(&self.module, "ioctl_pio_read", &[port]).map(|v| v as u8)
    }

    fn outb(&self, port: u64, v: u8) -> Option<()> {
        lpc(&self.module, "ioctl_pio_write", &[port, u64::from(v)]).map(|_| ())
    }

    /// Wait until the status port has `bit` set (`set`) or clear.
    fn wait(&self, bit: u8, set: bool, spins: u32) -> bool {
        for _ in 0..spins {
            if self.inb(EC_CMD).is_some_and(|s| (s & bit != 0) == set) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        false
    }

    fn read_byte(&self, reg: u8) -> Option<u8> {
        if !self.wait(EC_IBF, false, 50) {
            return None;
        }
        self.outb(EC_CMD, 0x80)?;
        if !self.wait(EC_IBF, false, 50) {
            return None;
        }
        self.outb(EC_DATA, reg)?;
        // ASUS boards sometimes never raise OBF; then IBF clearing means
        // the byte is there (LibreHardwareMonitor's workaround).
        let ok = self.failures.get() > 20 || self.wait(EC_OBF, true, 5) || self.wait(EC_IBF, false, 50);
        if !ok {
            self.failures.set(self.failures.get() + 1);
            return None;
        }
        self.failures.set(0);
        self.inb(EC_DATA)
    }

    fn write_byte(&self, reg: u8, v: u8) -> Option<()> {
        if !self.wait(EC_IBF, false, 50) {
            return None;
        }
        self.outb(EC_CMD, 0x81)?;
        if !self.wait(EC_IBF, false, 50) {
            return None;
        }
        self.outb(EC_DATA, reg)?;
        if !self.wait(EC_IBF, false, 50) {
            return None;
        }
        self.outb(EC_DATA, v)?;
        self.wait(EC_IBF, false, 50).then_some(())
    }

    fn read(&mut self) -> ExtTree {
        let mut values = std::collections::HashMap::new();
        {
            let _g = match self.mutex.as_ref().map(|m| m.lock_for(100)) {
                Some(None) => return ExtTree { source: "PawnIO", hardware: Vec::new() },
                Some(Some(g)) => Some(g),
                None => None,
            };
            // Registers are sorted by bank; EC register 0xFF selects it.
            let previous = self.read_byte(0xFF);
            let mut bank = None;
            for reg in self.board.registers() {
                let b = (reg >> 8) as u8;
                if bank != Some(b) {
                    if self.write_byte(0xFF, b).is_none() {
                        break;
                    }
                    bank = Some(b);
                }
                if let Some(v) = self.read_byte((reg & 0xFF) as u8) {
                    values.insert(reg, v);
                }
            }
            if let (Some(p), Some(b)) = (previous, bank) {
                if p != b {
                    self.write_byte(0xFF, p);
                }
            }
        }
        let hw = self.board.decode(&|r| values.get(&r).copied());
        super::superio::board_tree("PawnIO", self.name.as_deref(), vec![hw])
    }
}

// ---------------------------------------------------------------------------
// PawnIO: the motherboard's Super I/O chip (LpcIO module).

/// Shared with other tools that use the ISA bus (LibreHardwareMonitor…).
const ISA_MUTEX: &str = "Global\\Access_ISABUS.HTP.Method";
/// Super I/O config ports of slot 0 and 1.
const SIO_PORTS: [u16; 2] = [0x2E, 0x4E];

/// One detected chip. Each has its own LpcIO handle: the module allows
/// the I/O ranges of the slot it was set up for.
struct Chip {
    module: Module,
    kind: ChipKind,
    /// Hardware monitor I/O base (index at +5, data at +6).
    base: u16,
    seen: super::superio::FansSeen,
}

#[derive(Clone, Copy)]
enum ChipKind {
    Nuvoton(super::superio::Nuvoton),
    Ite(super::superio::Ite),
    Fintek(super::superio::Fintek),
}

impl ChipKind {
    fn name(&self) -> &'static str {
        match self {
            ChipKind::Nuvoton(c) => c.name,
            ChipKind::Ite(c) => c.name,
            ChipKind::Fintek(c) => c.name,
        }
    }
}

struct SuperIo {
    isa: Option<NamedMutex>,
    chips: Vec<Chip>,
    board: Option<String>,
}

/// Run an LpcIO function with up to two arguments and one result.
fn lpc(module: &Module, func: &str, args: &[u64]) -> Option<u64> {
    let mut out = [0u64; 1];
    module.execute(func, args, &mut out).ok().map(|_| out[0])
}

/// The hardware monitor's base address of the selected logical device, read
/// twice and checked (as LibreHardwareMonitor does).
fn hwm_base(module: &Module, ldn: u8) -> Option<u16> {
    lpc(module, "ioctl_superio_outb", &[0x07, u64::from(ldn)])?;
    let word = || lpc(module, "ioctl_superio_inw", &[0x60]).map(|v| v as u16);
    let base = word()?;
    std::thread::sleep(Duration::from_millis(1));
    (word()? == base && base >= 0x100 && base & 0xF007 == 0).then_some(base)
}

impl SuperIo {
    fn open() -> Result<SuperIo, String> {
        let isa = NamedMutex::new(ISA_MUTEX);
        let mut chips = Vec::new();
        let mut problems = Vec::new();
        for (slot, port) in SIO_PORTS.iter().enumerate() {
            let module = Module::open("LpcIO.bin")?.ok_or("the LpcIO module does not support this machine")?;
            let _guard = match &isa {
                Some(m) => Some(m.lock_for(500).ok_or("the ISA bus is busy (another tool holds its lock)")?),
                None => None,
            };
            lpc(&module, "ioctl_select_slot", &[slot as u64]).ok_or("selecting the Super I/O port failed")?;
            let port = u64::from(*port);
            let outb = |v: u64| lpc(&module, "ioctl_pio_outb", &[port, v]);
            let inb = |r: u64| lpc(&module, "ioctl_superio_inb", &[r]).map(|v| v as u8);
            let id = || Some((inb(0x20)?, inb(0x21)?));

            // Winbond / Nuvoton: 0x87 0x87 enters, 0xAA leaves.
            outb(0x87);
            outb(0x87);
            let found = match id() {
                Some((i, r)) if i != 0 && i != 0xFF => match (super::superio::nuvoton_chip(i, r), super::superio::fintek_chip(i, r)) {
                    (Some(c), _) => {
                        lpc(&module, "ioctl_find_bars", &[]);
                        let base = hwm_base(&module, super::superio::NUVOTON_HWM_LDN);
                        if base.is_some() && c.io_lock {
                            // Some boards lock the hardware monitor's I/O space.
                            if let Some(opt) = inb(0x28).filter(|o| o & 0x10 != 0) {
                                lpc(&module, "ioctl_superio_outb", &[0x28, u64::from(opt & !0x10)]);
                            }
                        }
                        Some((ChipKind::Nuvoton(c), base))
                    }
                    (None, Some(c)) => {
                        let vendor = lpc(&module, "ioctl_superio_inw", &[0x23]).map(|v| v as u16);
                        if vendor == Some(super::superio::FINTEK_VENDOR_ID) {
                            lpc(&module, "ioctl_find_bars", &[]);
                            lpc(&module, "ioctl_superio_outb", &[0x07, u64::from(c.ldn)]);
                            let word = || lpc(&module, "ioctl_superio_inw", &[0x60]).map(|v| v as u16);
                            let a = word();
                            std::thread::sleep(Duration::from_millis(1));
                            let b = word();
                            // Fintek reports the base + 5 on some chips.
                            let base = a.filter(|x| Some(*x) == b).map(|x| if x & 0x07 == 0x05 { x & 0xFFF8 } else { x }).filter(|x| *x >= 0x100);
                            Some((ChipKind::Fintek(c), base))
                        } else {
                            None
                        }
                    }
                    (None, None) => {
                        problems.push(super::superio::describe_unknown(i, r));
                        None
                    }
                },
                _ => None,
            };
            outb(0xAA);

            // ITE: 0x87 0x01 0x55 0x55 (0x2E) / 0xAA (0x4E) enters, writing
            // 0x02 to register 0x02 leaves.
            let found = found.or_else(|| {
                outb(0x87);
                outb(0x01);
                outb(0x55);
                outb(if port == 0x2E { 0x55 } else { 0xAA });
                let r = match id() {
                    Some((hi, lo)) if hi != 0 && hi != 0xFF => {
                        let code = u16::from(hi) << 8 | u16::from(lo);
                        match super::superio::ite_chip(code) {
                            Some(c) => {
                                lpc(&module, "ioctl_find_bars", &[]);
                                Some((ChipKind::Ite(c), hwm_base(&module, super::superio::ITE_EC_LDN)))
                            }
                            None => {
                                if hi == 0x86 || hi == 0x87 || hi == 0x88 {
                                    problems.push(format!("ITE chip {code:04X} is not supported yet"));
                                }
                                None
                            }
                        }
                    }
                    _ => None,
                };
                lpc(&module, "ioctl_superio_outb", &[0x02, 0x02]);
                r
            });
            drop(_guard);

            let Some((kind, base)) = found else { continue };
            let Some(base) = base else {
                problems.push(format!("{}: its hardware monitor has no valid address", kind.name()));
                continue;
            };
            let chip = Chip { module, kind, base, seen: Default::default() };
            // Check that the hardware monitor answers as that vendor.
            let ok = {
                let _g = isa.as_ref().map(|m| m.lock_for(100));
                match kind {
                    // The NCT668x embedded controllers have no vendor id.
                    ChipKind::Nuvoton(c) if matches!(c.layout, super::superio::NctLayout::Ec { .. }) => chip.nct(c, 0x100).is_some(),
                    ChipKind::Nuvoton(c) => {
                        let v = (u16::from(chip.nct(c, 0x804F).unwrap_or(0)) << 8) | u16::from(chip.nct(c, 0x004F).unwrap_or(0));
                        v == super::superio::NUVOTON_VENDOR_ID
                    }
                    ChipKind::Fintek(_) => chip.plain(0x20).is_some(),
                    ChipKind::Ite(c) => {
                        chip.ite_reg(c, 0x58).is_some_and(|v| super::superio::ITE_VENDOR_IDS.contains(&v))
                            && (!c.check_config || chip.ite_reg(c, 0x00).is_some_and(|v| v & 0x10 != 0))
                    }
                }
            };
            if ok {
                log::info!("PawnIO: {} at {:#x}", kind.name(), base);
                chips.push(chip);
            } else {
                problems.push(format!("{}: the hardware monitor did not answer", kind.name()));
            }
        }
        if chips.is_empty() {
            return Err(problems.into_iter().next().unwrap_or_else(|| "no supported Super I/O chip found".into()));
        }
        Ok(SuperIo { isa, chips, board: board_name() })
    }

    fn names(&self) -> String {
        self.chips.iter().map(|c| c.kind.name()).collect::<Vec<_>>().join(", ")
    }

    fn read(&mut self) -> ExtTree {
        let mut hardware = Vec::new();
        {
            let _g = match &self.isa {
                Some(m) => match m.lock_for(100) {
                    Some(g) => Some(g),
                    None => return ExtTree { source: "PawnIO", hardware },
                },
                None => None,
            };
            for chip in &mut self.chips {
                let mut seen = chip.seen;
                let hw = match chip.kind {
                    ChipKind::Nuvoton(c) => super::superio::nct679x(c, None, &mut seen, &mut |r| chip.nct(c, r)),
                    ChipKind::Fintek(c) => super::superio::fintek(c, None, &mut seen, &mut |r| chip.plain(r)),
                    ChipKind::Ite(c) => {
                        if c.bank0 {
                            chip.ite_bank0();
                        }
                        super::superio::ite(c, None, &mut seen, &mut |r| chip.ite_reg(c, r))
                    }
                };
                chip.seen = seen;
                hardware.push(hw);
            }
        }
        super::superio::board_tree("PawnIO", self.board.as_deref(), hardware)
    }
}

impl Chip {
    fn port(&self, port: u16, v: u8) -> Option<()> {
        lpc(&self.module, "ioctl_pio_outb", &[u64::from(port), u64::from(v)]).map(|_| ())
    }

    fn inp(&self, port: u16) -> Option<u8> {
        lpc(&self.module, "ioctl_pio_inb", &[u64::from(port)]).map(|v| v as u8)
    }

    /// Nuvoton: one register, bank (or EC page) in the high byte.
    fn nct(&self, chip: super::superio::Nuvoton, reg: u16) -> Option<u8> {
        let (page, index, data) = (self.base + 4, self.base + 5, self.base + 6);
        if let super::superio::NctLayout::Ec { .. } = chip.layout {
            // The EC window is free when the page port reads 0xFF.
            let mut free = false;
            for _ in 0..50 {
                if self.inp(page)? == 0xFF {
                    free = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            if !free {
                self.port(page, 0xFF)?;
            }
            self.port(page, (reg >> 8) as u8)?;
            self.port(index, (reg & 0xFF) as u8)?;
            let v = self.inp(data);
            self.port(page, 0xFF)?;
            return v;
        }
        self.port(index, 0x4E)?;
        self.port(data, (reg >> 8) as u8)?;
        self.port(index, (reg & 0xFF) as u8)?;
        self.inp(data)
    }

    /// Fintek: one register through index / data.
    fn plain(&self, reg: u8) -> Option<u8> {
        self.port(self.base + 5, reg)?;
        self.inp(self.base + 6)
    }

    /// ITE: one register; the address must read back (except IT8688E).
    fn ite_reg(&self, chip: super::superio::Ite, reg: u8) -> Option<u8> {
        let (index, data) = (self.base + 5, self.base + 6);
        self.port(index, reg)?;
        if !chip.no_readback && self.inp(index)? != reg {
            return None;
        }
        self.inp(data)
    }

    /// ITE: select bank 0 (bits 6:5 of register 0x06).
    fn ite_bank0(&self) {
        let (index, data) = (self.base + 5, self.base + 6);
        if self.port(index, 0x06).is_some() {
            if let Some(v) = self.inp(data) {
                self.port(index, 0x06);
                self.port(data, v & 0x9F);
            }
        }
    }
}

/// (manufacturer, product) from the BIOS's baseboard strings.
fn bios_board() -> (Option<String>, Option<String>) {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let get = |name: &str| -> Option<String> {
        let key: Vec<u16> = "HARDWARE\\DESCRIPTION\\System\\BIOS".encode_utf16().chain([0]).collect();
        let value: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let mut buf = [0u16; 256];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: buffers are valid for the sizes given.
        let r = unsafe {
            RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size)
        };
        if r != 0 {
            return None;
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let s = String::from_utf16_lossy(&buf[..n]).trim().to_string();
        (!s.is_empty()).then_some(s)
    };
    (get("BaseBoardManufacturer"), get("BaseBoardProduct"))
}

/// "ASRock B650E PG Riptide WiFi", from the BIOS's baseboard strings.
fn board_name() -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let get = |name: &str| -> Option<String> {
        let key: Vec<u16> = "HARDWARE\\DESCRIPTION\\System\\BIOS".encode_utf16().chain([0]).collect();
        let value: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let mut buf = [0u16; 256];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: buffers are valid for the sizes given.
        let r = unsafe {
            RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size)
        };
        if r != 0 {
            return None;
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let s = String::from_utf16_lossy(&buf[..n]).trim().to_string();
        (!s.is_empty() && !s.eq_ignore_ascii_case("to be filled by o.e.m.")).then_some(s)
    };
    let product = get("BaseBoardProduct")?;
    Some(match get("BaseBoardManufacturer") {
        Some(m) if !product.to_lowercase().starts_with(&m.to_lowercase()) => format!("{m} {product}"),
        _ => product,
    })
}

// ---------------------------------------------------------------------------
// PawnIO: the CPU's own thermal registers.

enum PawnCpu {
    Amd {
        brand: String,
        module: Module,
        /// `\BaseNamedObjects\Access_PCI`: SMN goes through PCI config
        /// space of the root complex, shared with other tools.
        pci: Option<NamedMutex>,
        offset: f64,
        /// (CCD index, SMN address) of the CCDs present.
        ccds: Vec<(u32, u32)>,
    },
    Intel {
        brand: String,
        module: Module,
        target: u64,
    },
}

struct Cpuid {
    vendor: [u8; 12],
    family: u32,
    model: u32,
    brand: String,
}

/// One SMN register (`ioctl_read_smn`), holding the shared PCI mutex as
/// the module asks. `None` if busy or failed.
fn smn(module: &Module, pci: Option<&NamedMutex>, addr: u32) -> Option<u32> {
    let _guard = match pci {
        Some(m) => Some(m.lock_for(100)?),
        None => None,
    };
    let mut out = [0u64; 1];
    module.execute("ioctl_read_smn", &[addr as u64], &mut out).ok()?;
    Some(out[0] as u32)
}

#[cfg(target_arch = "x86_64")]
fn cpuid() -> Option<Cpuid> {
    use std::arch::x86_64::__cpuid;
    #[allow(unused_unsafe)]
    // SAFETY: CPUID is always available on x86_64.
    let (l0, l1) = unsafe { (__cpuid(0), __cpuid(1)) };
    let mut vendor = [0u8; 12];
    vendor[0..4].copy_from_slice(&l0.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&l0.edx.to_le_bytes());
    vendor[8..12].copy_from_slice(&l0.ecx.to_le_bytes());
    let base_family = (l1.eax >> 8) & 0xf;
    let family = if base_family == 0xf { base_family + ((l1.eax >> 20) & 0xff) } else { base_family };
    let model = if base_family == 0xf || base_family == 6 {
        ((l1.eax >> 4) & 0xf) | (((l1.eax >> 16) & 0xf) << 4)
    } else {
        (l1.eax >> 4) & 0xf
    };
    let mut brand = Vec::new();
    #[allow(unused_unsafe)]
    // SAFETY: as above; leaves 0x8000_0002..4 exist on every x86_64 CPU.
    for leaf in 0x8000_0002u32..=0x8000_0004 {
        let r = unsafe { __cpuid(leaf) };
        for x in [r.eax, r.ebx, r.ecx, r.edx] {
            brand.extend_from_slice(&x.to_le_bytes());
        }
    }
    let brand = String::from_utf8_lossy(&brand).trim_matches(['\0', ' ']).to_string();
    Some(Cpuid { vendor, family, model, brand })
}

#[cfg(not(target_arch = "x86_64"))]
fn cpuid() -> Option<Cpuid> {
    None
}

impl PawnCpu {
    fn open() -> Result<PawnCpu, String> {
        let id = cpuid().ok_or("PawnIO CPU temperatures need an x86-64 CPU")?;
        match &id.vendor {
            b"AuthenticAMD" => {
                let module = Module::open("AMDFamily17.bin")?
                    .ok_or_else(|| format!("CPU family {:#x} is not supported by the AMDFamily17 module", id.family))?;
                let pci = NamedMutex::new("Global\\Access_PCI");
                smn(&module, pci.as_ref(), external::AMD_SMN_TEMP).ok_or("reading the CPU temperature over SMN failed")?;
                // Keep the CCDs whose register reads as valid (as k10temp).
                let ccds = external::amd_ccd_layout(id.family, id.model)
                    .map(|(first, n)| (0..n).map(|i| (i, external::AMD_SMN_TEMP + first + i * 4)).collect::<Vec<_>>())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|&(_, addr)| smn(&module, pci.as_ref(), addr).and_then(external::amd_ccd).is_some())
                    .collect();
                Ok(PawnCpu::Amd { offset: external::amd_tctl_offset(&id.brand), brand: id.brand, module, pci, ccds })
            }
            b"GenuineIntel" => {
                let module = Module::open("IntelMSR.bin")?.ok_or("the IntelMSR module does not support this CPU")?;
                let mut out = [0u64; 1];
                let target = module
                    .execute("ioctl_read_msr", &[external::MSR_IA32_TEMPERATURE_TARGET as u64], &mut out)
                    .map(|_| out[0])
                    .unwrap_or(0);
                let cpu = PawnCpu::Intel { brand: id.brand, module, target };
                if cpu.read().hardware.is_empty() {
                    return Err("this CPU does not report a package temperature".into());
                }
                Ok(cpu)
            }
            v => Err(format!("unsupported CPU vendor {}", String::from_utf8_lossy(v))),
        }
    }

    fn read(&self) -> ExtTree {
        match self {
            PawnCpu::Amd { brand, module, pci, offset, ccds } => {
                let Some(tctl) = smn(module, pci.as_ref(), external::AMD_SMN_TEMP) else {
                    return ExtTree { source: "PawnIO", hardware: Vec::new() };
                };
                let regs: Vec<(u32, u32)> =
                    ccds.iter().filter_map(|&(i, a)| Some((i, smn(module, pci.as_ref(), a)?))).collect();
                external::amd_tree(brand, tctl, *offset, &regs)
            }
            PawnCpu::Intel { brand, module, target } => {
                let mut out = [0u64; 1];
                let pkg = module
                    .execute("ioctl_read_msr", &[external::MSR_IA32_PACKAGE_THERM_STATUS as u64], &mut out)
                    .ok()
                    .and_then(|_| external::intel_package(out[0], *target));
                external::intel_tree(brand, pkg)
            }
        }
    }

    fn describe(&self) -> String {
        match self {
            PawnCpu::Amd { ccds, .. } if ccds.is_empty() => "AMD Tctl".into(),
            PawnCpu::Amd { ccds, .. } => format!("AMD Tctl and {} CCD temperature(s)", ccds.len()),
            PawnCpu::Intel { .. } => "Intel package temperature".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever this machine has installed, enabling PawnIO reports a
    /// status (working, or why not) and never panics. With the sensor
    /// service running, the service does the PawnIO reads instead.
    #[test]
    fn pawnio_opt_in_reports_status() {
        configure(Settings { pawnio: true, ..Settings::default() });
        let t = trees();
        let st = status();
        configure(Settings::default());
        let service = st.iter().any(|s| s.starts_with("Sensor service: ") && !s.ends_with("not installed"));
        assert!(service || st.iter().any(|s| s.starts_with("PawnIO: ")), "{st:?}");
        // `cargo test pawnio_opt_in -- --nocapture`, elevated, shows what PawnIO reads.
        eprintln!("{st:?}");
        for x in t.iter().filter(|x| x.source == "PawnIO") {
            eprintln!("{x:?}");
        }
    }
}
