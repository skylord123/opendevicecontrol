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
//!
//! Both ends use overlapped I/O with deadlines: a client that stops reading
//! holds the service for at most [`SERVE_TIMEOUT`], and a service that stops
//! writing holds a reader for at most [`CLIENT_TIMEOUT`].

use super::external::{ExtHardware, ExtTree};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, ERROR_TIMEOUT, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

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

/// How long a reader waits for the whole reply.
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the service gives one client to take its reply.
pub const SERVE_TIMEOUT: Duration = Duration::from_secs(3);
/// Replies larger than this are refused.
const MAX_REPLY: usize = 8 << 20;

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
    match fetch_current() {
        Err(FetchError::Absent) => fetch_from(LEGACY_PIPE),
        r => r,
    }
}

/// Read the snapshot from the service under its current name only.
pub fn fetch_current() -> Result<Snapshot, FetchError> {
    fetch_from(PIPE)
}

fn fetch_from(pipe: &str) -> Result<Snapshot, FetchError> {
    let buf = read_pipe(pipe, CLIENT_TIMEOUT)?;
    serde_json::from_slice(&buf).map_err(|e| FetchError::Failed(format!("unreadable reply: {e}")))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn win_err(code: u32) -> String {
    std::io::Error::from_raw_os_error(code as i32).to_string()
}

/// A handle closed on drop.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: a handle this module opened, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// A manual-reset event for overlapped I/O.
fn event() -> std::io::Result<Owned> {
    // SAFETY: plain Win32 call.
    let e = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if e.is_null() {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(Owned(e))
    }
}

fn ms_until(deadline: Instant) -> u32 {
    deadline.saturating_duration_since(Instant::now()).as_millis().min(u32::MAX as u128 - 1) as u32
}

/// Finish an overlapped operation on `h` that was just started (`started`
/// is the call's return value): wait for it until `deadline`, cancelling it
/// if it isn't done by then. Returns the bytes moved, or the Win32 error
/// (`ERROR_TIMEOUT` on the deadline).
///
/// # Safety
/// `h` is a valid overlapped handle and `ov` the OVERLAPPED the operation
/// was started with, with a valid event.
unsafe fn finish(h: HANDLE, ov: &OVERLAPPED, started: i32, deadline: Instant) -> Result<u32, u32> {
    let mut n = 0u32;
    if started == 0 {
        let e = GetLastError();
        if e != ERROR_IO_PENDING {
            return Err(e);
        }
        if WaitForSingleObject(ov.hEvent, ms_until(deadline)) != WAIT_OBJECT_0 {
            CancelIoEx(h, ov);
            // Wait for the cancellation, so the buffer and OVERLAPPED are
            // no longer in use when we return.
            GetOverlappedResult(h, ov, &mut n, 1);
            return Err(ERROR_TIMEOUT);
        }
    }
    if GetOverlappedResult(h, ov, &mut n, 0) == 0 {
        return Err(GetLastError());
    }
    Ok(n)
}

/// Read a pipe to its end, giving up after `timeout`.
fn read_pipe(pipe: &str, timeout: Duration) -> Result<Vec<u8>, FetchError> {
    use windows_sys::Win32::Foundation::GENERIC_READ;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
    };
    use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

    let deadline = Instant::now() + timeout;
    let name = wide(pipe);
    let mut was_busy = false;
    let pipe = loop {
        // SAFETY: plain Win32 call with a NUL-terminated name. The server
        // may only identify us, not impersonate us.
        let h = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                std::ptr::null_mut(),
            )
        };
        if h != INVALID_HANDLE_VALUE {
            break Owned(h);
        }
        // SAFETY: plain Win32 call.
        match unsafe { GetLastError() } {
            // Between two clients the pipe briefly has no instance: only
            // absent if it wasn't just there.
            ERROR_FILE_NOT_FOUND if !was_busy => return Err(FetchError::Absent),
            ERROR_FILE_NOT_FOUND | ERROR_PIPE_BUSY => {
                was_busy = true;
                let ms = ms_until(deadline);
                if ms == 0 {
                    return Err(FetchError::Failed("the sensor service is busy".into()));
                }
                // SAFETY: as above.
                if unsafe { WaitNamedPipeW(name.as_ptr(), ms.min(500)) } == 0 {
                    std::thread::sleep(Duration::from_millis(10).min(Duration::from_millis(ms as u64)));
                }
            }
            e => return Err(FetchError::Failed(win_err(e))),
        }
    };
    let ev = event().map_err(|e| FetchError::Failed(e.to_string()))?;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        if buf.len() >= MAX_REPLY {
            return Err(FetchError::Failed("reply too large".into()));
        }
        buf.reserve(1 << 16);
        let room = (buf.capacity() - buf.len()).min(1 << 16).min(MAX_REPLY - buf.len());
        // SAFETY: zeroed OVERLAPPED with our event; the read goes into the
        // vector's spare capacity, and `finish` does not return while the
        // read is still in flight.
        let r = unsafe {
            let mut ov: OVERLAPPED = std::mem::zeroed();
            ov.hEvent = ev.0;
            let spare = buf.as_mut_ptr().add(buf.len());
            let started = ReadFile(pipe.0, spare, room as u32, std::ptr::null_mut(), &mut ov);
            finish(pipe.0, &ov, started, deadline)
        };
        match r {
            // SAFETY: the read initialised `n` bytes of spare capacity.
            Ok(n) if n > 0 => unsafe { buf.set_len(buf.len() + n as usize) },
            Ok(_) => {}
            Err(ERROR_BROKEN_PIPE) => break,
            // The server hung up after the data.
            Err(ERROR_PIPE_NOT_CONNECTED) if !buf.is_empty() => break,
            Err(ERROR_TIMEOUT) => return Err(FetchError::Failed("the sensor service did not answer in time".into())),
            Err(e) => return Err(FetchError::Failed(win_err(e))),
        }
    }
    Ok(buf)
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

/// Serve snapshots on the pipe until `stop` is set.
pub fn serve(stop: &std::sync::atomic::AtomicBool) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

    // SYSTEM and administrators: full access; signed-in users: read only.
    let sddl = wide("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GR;;;AU)");
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: plain Win32 call; `sd` is freed with LocalFree below.
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
    let result = serve_on(PIPE, &sa, stop, SERVE_TIMEOUT, || serde_json::to_vec(&snapshot()).unwrap_or_default());
    // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
    unsafe { LocalFree(sd as _) };
    result
}

/// Serve `reply()` to each client of pipe `name` until `stop` is set. Each
/// client gets `per_client` to take its reply; after that it is dropped.
fn serve_on(
    name: &str,
    sa: *const windows_sys::Win32::Security::SECURITY_ATTRIBUTES,
    stop: &std::sync::atomic::AtomicBool,
    per_client: Duration,
    mut reply: impl FnMut() -> Vec<u8>,
) -> std::io::Result<()> {
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::Storage::FileSystem::{WriteFile, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_OUTBOUND};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    let name = wide(name);
    let ev = event()?;
    while !stop.load(Ordering::Relaxed) {
        // SAFETY: plain Win32 call; the handle is owned (closed on drop).
        let h = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_OUTBOUND | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                1 << 16,
                0,
                0,
                sa,
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let h = Owned(h);

        // Wait for a client, checking `stop` a few times a second.
        // SAFETY: zeroed OVERLAPPED with our event; the connect is
        // cancelled and waited for before `ov` goes away.
        let connected = unsafe {
            let mut ov: OVERLAPPED = std::mem::zeroed();
            ov.hEvent = ev.0;
            if ConnectNamedPipe(h.0, &mut ov) != 0 {
                true
            } else {
                match GetLastError() {
                    ERROR_PIPE_CONNECTED => true,
                    ERROR_IO_PENDING => loop {
                        if WaitForSingleObject(ev.0, 250) == WAIT_OBJECT_0 {
                            let mut n = 0;
                            break GetOverlappedResult(h.0, &ov, &mut n, 0) != 0;
                        }
                        if stop.load(Ordering::Relaxed) {
                            let mut n = 0;
                            CancelIoEx(h.0, &ov);
                            GetOverlappedResult(h.0, &ov, &mut n, 1);
                            return Ok(());
                        }
                    },
                    _ => false,
                }
            }
        };
        if !connected || stop.load(Ordering::Relaxed) {
            continue;
        }

        let body = reply();
        let deadline = Instant::now() + per_client;
        let mut done = 0usize;
        while done < body.len() {
            let chunk = &body[done..];
            // SAFETY: as for the connect; `finish` waits out (or cancels)
            // the write before `chunk` or `ov` can go away.
            let r = unsafe {
                let mut ov: OVERLAPPED = std::mem::zeroed();
                ov.hEvent = ev.0;
                let started = WriteFile(h.0, chunk.as_ptr(), chunk.len().min(1 << 16) as u32, std::ptr::null_mut(), &mut ov);
                finish(h.0, &ov, started, deadline)
            };
            match r {
                Ok(n) if n > 0 => done += n as usize,
                // A stalled or vanished client: drop it.
                _ => break,
            }
        }
        // Closing (not disconnecting) leaves what the client hasn't read
        // yet in the pipe, followed by a clean end of data.
        drop(h);
    }
    Ok(())
}

/// Unblock [`serve`] waiting for a client (after setting its stop flag).
/// `serve` also notices the flag on its own within a fraction of a second.
pub fn wake() {
    let _ = std::fs::File::open(PIPE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn name(tag: &str) -> String {
        format!(r"\\.\pipe\opendevicecontrol-test-{tag}-{}", std::process::id())
    }

    /// A server on its own thread; stops when dropped.
    struct Server {
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<std::io::Result<()>>>,
    }

    impl Server {
        fn start(pipe: &str, per_client: Duration, body: Vec<u8>) -> Server {
            let stop = Arc::new(AtomicBool::new(false));
            let (s, p) = (stop.clone(), pipe.to_string());
            let thread = std::thread::spawn(move || serve_on(&p, std::ptr::null(), &s, per_client, || body.clone()));
            // Wait until the pipe exists.
            let until = Instant::now() + Duration::from_secs(5);
            while read_pipe(pipe, Duration::from_secs(2)).is_err() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
            Server { stop, thread: Some(thread) }
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let t = self.thread.take().unwrap();
            let start = Instant::now();
            assert!(t.join().unwrap().is_ok());
            assert!(start.elapsed() < Duration::from_secs(2), "stop took {:?}", start.elapsed());
        }
    }

    fn body(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 % 251) as u8).collect()
    }

    #[test]
    fn absent_pipe_is_absent() {
        assert!(matches!(read_pipe(&name("absent"), Duration::from_secs(1)), Err(FetchError::Absent)));
    }

    #[test]
    fn large_reply_arrives_whole() {
        let pipe = name("large");
        let want = body(300_000);
        let _s = Server::start(&pipe, Duration::from_secs(5), want.clone());
        assert_eq!(read_pipe(&pipe, Duration::from_secs(5)).ok(), Some(want));
    }

    /// A client that connects and never reads holds the server for at most
    /// its per-client time; the next client is served and stop stays quick.
    #[test]
    fn stalled_client_does_not_wedge_the_server() {
        let pipe = name("stall");
        let want = body(300_000);
        let s = Server::start(&pipe, Duration::from_millis(500), want.clone());
        // Retry: between clients the pipe briefly has no instance.
        let connect = || {
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                match std::fs::File::open(&pipe) {
                    Ok(f) => break f,
                    Err(e) if Instant::now() > until => panic!("{e}"),
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        };
        let stalled = connect();
        let start = Instant::now();
        let got = read_pipe(&pipe, Duration::from_secs(5));
        assert!(matches!(&got, Ok(b) if *b == want), "{:?}", got.map(|b| b.len()));
        assert!(start.elapsed() < Duration::from_secs(3), "{:?}", start.elapsed());
        drop(stalled);
        // Stop while a stalled client is connected.
        let _stalled = connect();
        drop(s);
    }

    /// A server that never finishes its reply costs a reader at most its
    /// deadline.
    #[test]
    fn reader_gives_up_on_a_stalled_server() {
        use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_OUTBOUND;
        use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_WAIT};
        let pipe = name("silent");
        let w = wide(&pipe);
        // SAFETY: plain Win32 calls in a test.
        let h = unsafe { CreateNamedPipeW(w.as_ptr(), PIPE_ACCESS_OUTBOUND, PIPE_TYPE_BYTE | PIPE_WAIT, 1, 1 << 16, 0, 0, std::ptr::null()) };
        assert_ne!(h, INVALID_HANDLE_VALUE);
        let h = Owned(h);
        let raw = h.0 as usize;
        // Accept the client but never write.
        let t = std::thread::spawn(move || unsafe { ConnectNamedPipe(raw as HANDLE, std::ptr::null_mut()) });
        let start = Instant::now();
        let r = read_pipe(&pipe, Duration::from_millis(400));
        assert!(matches!(r, Err(FetchError::Failed(_))), "{r:?}");
        assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
        t.join().unwrap();
        drop(h);
    }
}
