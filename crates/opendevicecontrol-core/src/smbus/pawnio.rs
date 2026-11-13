//! Windows SMBus backend through PawnIO (<https://pawnio.eu>), the signed
//! kernel driver used by current OpenRGB, LibreHardwareMonitor and
//! FanControl. Modelled on OpenRGB's `i2c_smbus_pawnio.cpp`.
//!
//! Nothing here is linked at build time: `PawnIOLib.dll` (installed with
//! PawnIO) is loaded when RAM support (or PawnIO CPU temperatures, see
//! `system::windows`) is enabled, and the signed modules (`SmbusI801.bin`,
//! `SmbusPIIX4.bin`, `AMDFamily17.bin`, `IntelMSR.bin`, from the
//! PawnIO.Modules release) are read from `<exe dir>\pawnio\`, `<exe dir>` or
//! `%APPDATA%\opendevicecontrol\pawnio\`. PawnIO only lets administrators open it.
//!
//! Every transfer holds the system-wide `Global\Access_SMBUS.HTP.Method`
//! mutex, the convention shared by HWiNFO, AIDA64, LibreHardwareMonitor and
//! OpenRGB, so monitoring tools and opendevicecontrol don't interleave transfers.

#![allow(clippy::upper_case_acronyms)]

use super::SmBus;
use crate::{Error, Result};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::OnceLock;

type HANDLE = *mut c_void;
type HRESULT = i32;

const E_ACCESSDENIED: HRESULT = 0x8007_0005_u32 as i32;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_ABANDONED: u32 = 0x80;
const MUTEX_NAME: &str = "Global\\Access_SMBUS.HTP.Method";
/// OpenRGB's default (`PAWNIO_SLEEPMODE_ALWAYSSLEEP`).
const SLEEP_MODE_ALWAYS_SLEEP: u64 = 2;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn CreateMutexW(attrs: *mut c_void, initial_owner: i32, name: *const u16) -> HANDLE;
    fn WaitForSingleObject(handle: HANDLE, ms: u32) -> u32;
    fn ReleaseMutex(handle: HANDLE) -> i32;
    fn CloseHandle(handle: HANDLE) -> i32;
}

type FnVersion = unsafe extern "system" fn(*mut u32) -> HRESULT;
type FnOpen = unsafe extern "system" fn(*mut HANDLE) -> HRESULT;
type FnLoad = unsafe extern "system" fn(HANDLE, *const u8, usize) -> HRESULT;
type FnExecute = unsafe extern "system" fn(HANDLE, *const u8, *const u64, usize, *mut u64, usize, *mut usize) -> HRESULT;
type FnClose = unsafe extern "system" fn(HANDLE) -> HRESULT;

struct Lib {
    version: FnVersion,
    open: FnOpen,
    load: FnLoad,
    execute: FnExecute,
    close: FnClose,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn lib() -> std::result::Result<&'static Lib, String> {
    static LIB: OnceLock<std::result::Result<Lib, String>> = OnceLock::new();
    LIB.get_or_init(|| {
        let mut candidates = Vec::new();
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            candidates.push(PathBuf::from(pf).join("PawnIO").join("PawnIOLib.dll"));
        }
        candidates.push(PathBuf::from("PawnIOLib.dll"));
        // SAFETY: plain Win32 calls; the module is never unloaded, so the
        // function pointers stay valid for the life of the process.
        unsafe {
            let module = candidates
                .iter()
                .map(|p| LoadLibraryW(wide(&p.to_string_lossy()).as_ptr()))
                .find(|m| !m.is_null())
                .ok_or_else(|| "PawnIO is not installed (PawnIOLib.dll not found). Install it from https://pawnio.eu".to_string())?;
            let sym = |name: &str| {
                let n: Vec<u8> = name.bytes().chain(std::iter::once(0)).collect();
                let p = GetProcAddress(module, n.as_ptr());
                if p.is_null() {
                    Err(format!("PawnIOLib.dll lacks {name}"))
                } else {
                    Ok(p)
                }
            };
            Ok(Lib {
                version: std::mem::transmute::<*mut c_void, FnVersion>(sym("pawnio_version")?),
                open: std::mem::transmute::<*mut c_void, FnOpen>(sym("pawnio_open")?),
                load: std::mem::transmute::<*mut c_void, FnLoad>(sym("pawnio_load")?),
                execute: std::mem::transmute::<*mut c_void, FnExecute>(sym("pawnio_execute")?),
                close: std::mem::transmute::<*mut c_void, FnClose>(sym("pawnio_close")?),
            })
        }
    })
    .as_ref()
    .map_err(Clone::clone)
}

pub(crate) fn module_blob(file: &str) -> std::result::Result<Vec<u8>, String> {
    let mut dirs = Vec::new();
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)) {
        dirs.push(exe_dir.join("pawnio"));
        dirs.push(exe_dir);
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("opendevicecontrol").join("pawnio"));
    }
    for d in &dirs {
        if let Ok(b) = std::fs::read(d.join(file)) {
            return Ok(b);
        }
    }
    Err(format!(
        "PawnIO module {file} not found (put the signed module from the PawnIO.Modules release in {})",
        dirs.first().map(|d| d.display().to_string()).unwrap_or_default()
    ))
}

/// A system-wide named mutex shared with other hardware tools
/// (`Global\Access_SMBUS.HTP.Method`, `Global\Access_PCI`, …).
pub(crate) struct NamedMutex(HANDLE);

// SAFETY: a kernel handle, usable from any thread.
unsafe impl Send for NamedMutex {}

impl NamedMutex {
    pub(crate) fn new(name: &str) -> Option<Self> {
        // SAFETY: plain Win32 call.
        let h = unsafe { CreateMutexW(std::ptr::null_mut(), 0, wide(name).as_ptr()) };
        (!h.is_null()).then_some(Self(h))
    }

    /// `None` if another program held it for longer than `ms`.
    pub(crate) fn lock_for(&self, ms: u32) -> Option<MutexGuard<'_>> {
        // SAFETY: valid mutex handle.
        let r = unsafe { WaitForSingleObject(self.0, ms) };
        (r == WAIT_OBJECT_0 || r == WAIT_ABANDONED).then_some(MutexGuard(self))
    }

    fn lock(&self) -> Result<MutexGuard<'_>> {
        self.lock_for(1000).ok_or_else(|| Error::Bus("SMBus is busy (another program holds the shared SMBus lock)".into()))
    }
}

pub(crate) struct MutexGuard<'a>(&'a NamedMutex);

impl Drop for MutexGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: we own the mutex.
        unsafe { ReleaseMutex(self.0 .0) };
    }
}

impl Drop for NamedMutex {
    fn drop(&mut self) {
        // SAFETY: valid handle, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// PawnIO with one signed module loaded.
pub(crate) struct Module {
    lib: &'static Lib,
    handle: HANDLE,
}

// SAFETY: the PawnIO handle is a plain kernel handle, usable from any thread.
unsafe impl Send for Module {}

impl Module {
    /// Open PawnIO and load the signed module `file` (see [`module_blob`]).
    /// `Ok(None)` if the module does not apply to this machine.
    pub(crate) fn open(file: &str) -> std::result::Result<Option<Module>, String> {
        let lib = installed()?;
        let blob = module_blob(file)?;
        Ok(start(lib, file, &blob)?.map(|handle| Module { lib, handle }))
    }

    pub(crate) fn execute(&self, func: &str, input: &[u64], out: &mut [u64]) -> std::result::Result<usize, HRESULT> {
        let name: Vec<u8> = func.bytes().chain(std::iter::once(0)).collect();
        let mut returned = 0usize;
        // SAFETY: buffers are valid for the given lengths.
        let hr = unsafe {
            (self.lib.execute)(
                self.handle,
                name.as_ptr(),
                input.as_ptr(),
                input.len(),
                out.as_mut_ptr(),
                out.len(),
                &mut returned,
            )
        };
        if hr == 0 {
            Ok(returned)
        } else {
            Err(hr)
        }
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        // SAFETY: handle came from pawnio_open and is closed once.
        unsafe { (self.lib.close)(self.handle) };
    }
}

/// PawnIOLib.dll, with its driver running.
fn installed() -> std::result::Result<&'static Lib, String> {
    let lib = lib()?;
    let mut v = 0u32;
    // SAFETY: out-pointer is valid.
    if unsafe { (lib.version)(&mut v) } != 0 {
        return Err("PawnIO is installed but its driver is not running (reinstall PawnIO)".into());
    }
    Ok(lib)
}

/// Open PawnIO and load `module`. `None` if the module does not apply to
/// this machine (PawnIO reports "not supported").
fn start(lib: &'static Lib, module: &str, blob: &[u8]) -> std::result::Result<Option<HANDLE>, String> {
    let mut handle: HANDLE = std::ptr::null_mut();
    // SAFETY: out-pointer is valid.
    let hr = unsafe { (lib.open)(&mut handle) };
    if hr == E_ACCESSDENIED {
        return Err("PawnIO refused access: OpenDeviceControl must run as administrator to use PawnIO".into());
    }
    if hr != 0 {
        return Err(format!("could not open PawnIO ({hr:#010x})"));
    }
    // SAFETY: blob is valid for its length.
    let hr = unsafe { (lib.load)(handle, blob.as_ptr(), blob.len()) };
    if hr != 0 {
        // SAFETY: handle came from pawnio_open.
        unsafe { (lib.close)(handle) };
        // HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED): this hardware isn't present.
        if hr == 0x8007_0032_u32 as i32 {
            return Ok(None);
        }
        return Err(format!("PawnIO could not load {module} ({hr:#010x})"));
    }
    Ok(Some(handle))
}

pub struct PawnIoBus {
    module: Module,
    name: String,
    mutex: Option<NamedMutex>,
}

impl PawnIoBus {
    fn execute(&self, func: &str, input: &[u64], out: &mut [u64]) -> std::result::Result<usize, HRESULT> {
        self.module.execute(func, input, out)
    }

    /// `ioctl_smbus_xfer`: in = [addr, read_write, command, size, data (34 bytes in 5 u64)],
    /// out = data.
    fn xfer(&mut self, addr: u8, read_write: u8, command: u8, size: u32, data: &mut [u8; 34]) -> Result<()> {
        let mut input = [0u64; 9];
        input[0] = addr as u64;
        input[1] = read_write as u64;
        input[2] = command as u64;
        input[3] = size as u64;
        let mut raw = [0u8; 40];
        raw[..34].copy_from_slice(data);
        for (i, chunk) in raw.chunks(8).enumerate() {
            input[4 + i] = u64::from_le_bytes(chunk.try_into().unwrap());
        }
        let mut out = [0u64; 5];
        let r = {
            let _guard = self.mutex.as_ref().map(|m| m.lock()).transpose()?;
            self.execute("ioctl_smbus_xfer", &input, &mut out)
        };
        r.map_err(|hr| Error::Bus(format!("{}: transfer {addr:#04x}/{command:#04x} failed ({hr:#010x})", self.name)))?;
        let mut back = [0u8; 40];
        for (i, v) in out.iter().enumerate() {
            back[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
        }
        data.copy_from_slice(&back[..34]);
        Ok(())
    }

    fn new(module: Module, label: String) -> Self {
        Self { module, name: label, mutex: NamedMutex::new(MUTEX_NAME) }
    }

    /// PCI vendor/device of the controller the module drives.
    fn identity(&self) -> Option<(u16, u16)> {
        let mut out = [0u64; 3];
        self.execute("ioctl_identity", &[0], &mut out).ok()?;
        Some(((out[2] & 0xffff) as u16, ((out[2] >> 16) & 0xffff) as u16))
    }
}

impl SmBus for PawnIoBus {
    fn name(&self) -> &str {
        &self.name
    }
    fn read_byte_data(&mut self, addr: u8, reg: u8) -> Result<u8> {
        let mut d = [0u8; 34];
        self.xfer(addr, 1, reg, 2, &mut d)?;
        Ok(d[0])
    }
    fn read_word_data(&mut self, addr: u8, reg: u8) -> Result<u16> {
        let mut d = [0u8; 34];
        self.xfer(addr, 1, reg, 3, &mut d)?;
        Ok(u16::from_le_bytes([d[0], d[1]]))
    }
    fn write_byte_data(&mut self, addr: u8, reg: u8, value: u8) -> Result<()> {
        let mut d = [0u8; 34];
        d[0] = value;
        self.xfer(addr, 0, reg, 2, &mut d)
    }
    fn write_word_data(&mut self, addr: u8, reg: u8, value: u16) -> Result<()> {
        let mut d = [0u8; 34];
        d[..2].copy_from_slice(&value.to_le_bytes());
        self.xfer(addr, 0, reg, 3, &mut d)
    }
    fn write_block_data(&mut self, addr: u8, reg: u8, data: &[u8]) -> Result<()> {
        if data.is_empty() || data.len() > super::BLOCK_MAX {
            return Err(Error::Bus(format!("invalid block length {}", data.len())));
        }
        let mut d = [0u8; 34];
        d[0] = data.len() as u8;
        d[1..=data.len()].copy_from_slice(data);
        self.xfer(addr, 0, reg, 5, &mut d)
    }
}

pub fn open_dram_buses() -> Result<Vec<Box<dyn SmBus>>> {
    let lib = installed().map_err(Error::Bus)?;
    let mut buses: Vec<Box<dyn SmBus>> = Vec::new();
    let mut problems = Vec::new();

    // (module file, PIIX4 ports to open)
    for (file, ports) in [("SmbusI801.bin", &[None][..]), ("SmbusPIIX4.bin", &[Some(0u64), Some(1)][..])] {
        let blob = match module_blob(file) {
            Ok(b) => b,
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        for &port in ports {
            let handle = match start(lib, file, &blob) {
                Ok(Some(h)) => h,
                Ok(None) => break, // controller not present
                Err(e) => return Err(Error::Bus(e)), // access denied etc.: same for every module
            };
            let label = match port {
                Some(p) => format!("PawnIO PIIX4 port {p}"),
                None => "PawnIO i801".to_string(),
            };
            let bus = PawnIoBus::new(Module { lib, handle }, label);
            if let Some(p) = port {
                let guard = bus.mutex.as_ref().map(|m| m.lock()).transpose();
                let ok = guard.is_ok() && bus.execute("ioctl_piix4_port_sel", &[p], &mut [0u64; 1]).is_ok();
                drop(guard);
                if !ok {
                    log::warn!("{}: selecting the port failed", bus.name);
                    continue;
                }
            }
            let _ = bus.execute("ioctl_set_sleep_mode", &[SLEEP_MODE_ALWAYS_SLEEP], &mut []);
            match bus.identity() {
                Some((ven, dev)) if super::is_dram_smbus(ven, dev) => {
                    log::info!("SMBus {}: {ven:04x}:{dev:04x}", bus.name);
                    buses.push(Box::new(bus));
                }
                Some((ven, dev)) => log::info!("{}: controller {ven:04x}:{dev:04x} is not a known DRAM SMBus", bus.name),
                None => log::warn!("{}: identity query failed", bus.name),
            }
        }
    }
    if buses.is_empty() {
        if problems.is_empty() {
            problems.push("PawnIO found no supported chipset SMBus controller".into());
        }
        return Err(Error::Bus(problems.join("; ")));
    }
    Ok(buses)
}
