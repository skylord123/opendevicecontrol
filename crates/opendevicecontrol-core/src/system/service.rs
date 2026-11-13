//! OpenDeviceControl's sensor service (Windows).
//!
//! Reading the CPU's thermal registers, the motherboard's Super I/O chip and
//! the like needs kernel access (the PawnIO driver), and PawnIO only lets
//! administrators in. OpenDeviceControl itself runs unprivileged in the tray, so a
//! small service, `opendevicecontrol-sensors.exe` running as LocalSystem, does those
//! reads and hands the results to the app over a local named pipe.
//!
//! The pipe is read-only: a client connects, the service writes one JSON
//! [`Snapshot`] and closes. It accepts no commands, so it gives a local user
//! nothing beyond the readings. Remote clients are rejected.

use super::external::{ExtHardware, ExtTree};
use serde::{Deserialize, Serialize};
use std::io::Read;

/// The service's name in the Service Control Manager.
pub const SERVICE_NAME: &str = "OpenDeviceControlSensors";
pub const SERVICE_DISPLAY_NAME: &str = "OpenDeviceControl hardware sensors";
pub const PIPE: &str = r"\\.\pipe\opendevicecontrol-sensors";
/// The service under the project's previous name, still read until the new
/// one is installed (installing it removes the old one).
pub const LEGACY_SERVICE_NAME: &str = "RigMeldSensors";
pub const LEGACY_PIPE: &str = r"\\.\pipe\rigmeld-sensors";
/// Where the previous name installed the service (under `%ProgramFiles%`).
pub const LEGACY_INSTALL_DIR: &str = r"RigMeld\Sensors";

/// What the service sends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// The service's version.
    pub version: String,
    /// One line per reader, for Settings and `--probe`.
    pub status: Vec<String>,
    pub trees: Vec<WireTree>,
}

/// An [`ExtTree`] on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireTree {
    pub source: String,
    pub hardware: Vec<ExtHardware>,
}

impl WireTree {
    pub fn from_tree(t: &ExtTree) -> Self {
        WireTree { source: t.source.to_string(), hardware: t.hardware.clone() }
    }

    pub fn into_tree(self) -> ExtTree {
        // Sources are a fixed set; anything else is labelled as the service.
        let source = match self.source.as_str() {
            "PawnIO" => "PawnIO",
            _ => "OpenDeviceControl sensor service",
        };
        ExtTree { source, hardware: self.hardware }
    }
}

/// Why the service could not be read.
#[derive(Debug)]
pub enum FetchError {
    /// Not installed or not running.
    Absent,
    /// Running but busy, or it sent something unreadable.
    Failed(String),
}

/// Read the service's current snapshot (from the service installed under the
/// previous name if the new one isn't there yet).
pub fn fetch() -> Result<Snapshot, FetchError> {
    match fetch_from(PIPE) {
        Err(FetchError::Absent) => fetch_from(LEGACY_PIPE),
        r => r,
    }
}

fn fetch_from(pipe: &str) -> Result<Snapshot, FetchError> {
    let mut f = match std::fs::File::open(pipe) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(FetchError::Absent),
        Err(e) => return Err(FetchError::Failed(e.to_string())),
    };
    let mut buf = Vec::new();
    match f.by_ref().take(8 << 20).read_to_end(&mut buf) {
        Ok(_) => {}
        // ERROR_PIPE_NOT_CONNECTED after the data: the server hung up.
        Err(e) if e.raw_os_error() == Some(233) && !buf.is_empty() => {}
        Err(e) => return Err(FetchError::Failed(e.to_string())),
    }
    serde_json::from_slice(&buf).map_err(|e| FetchError::Failed(format!("unreadable reply: {e}")))
}

/// What the service reads right now.
pub fn snapshot() -> Snapshot {
    let (status, trees) = super::windows::service_readings();
    Snapshot {
        version: env!("CARGO_PKG_VERSION").to_string(),
        status,
        trees: trees.iter().map(WireTree::from_tree).collect(),
    }
}

/// Serve snapshots on the pipe until `stop` is set (then call [`wake`]).
pub fn serve(stop: &std::sync::atomic::AtomicBool) -> std::io::Result<()> {
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, GetLastError, ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::{FlushFileBuffers, WriteFile, PIPE_ACCESS_OUTBOUND};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    // SYSTEM and administrators: full access; signed-in users: read only.
    let sddl = wide("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GR;;;AU)");
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: plain Win32 calls; `sd` is freed with LocalFree on return.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) == 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };
    let name = wide(PIPE);
    let result = loop {
        // SAFETY: as above; every handle is closed before the next loop.
        unsafe {
            let h = CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_OUTBOUND,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                1 << 16,
                0,
                0,
                &sa,
            );
            if h == INVALID_HANDLE_VALUE {
                break Err(std::io::Error::last_os_error());
            }
            let connected = ConnectNamedPipe(h, std::ptr::null_mut()) != 0 || GetLastError() == ERROR_PIPE_CONNECTED;
            if stop.load(Ordering::Relaxed) {
                CloseHandle(h);
                break Ok(());
            }
            if connected {
                let body = serde_json::to_vec(&snapshot()).unwrap_or_default();
                let mut done = 0usize;
                while done < body.len() {
                    let mut n = 0u32;
                    let chunk = &body[done..];
                    if WriteFile(h, chunk.as_ptr(), chunk.len().min(1 << 16) as u32, &mut n, std::ptr::null_mut()) == 0 || n == 0 {
                        break;
                    }
                    done += n as usize;
                }
                // Wait until the client has read it all; closing (not
                // disconnecting) then gives the client a clean end of data.
                FlushFileBuffers(h);
            }
            CloseHandle(h);
        }
    };
    // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
    unsafe { LocalFree(sd as _) };
    result
}

/// Unblock [`serve`] waiting for a client (after setting its stop flag).
pub fn wake() {
    let _ = std::fs::File::open(PIPE);
}
