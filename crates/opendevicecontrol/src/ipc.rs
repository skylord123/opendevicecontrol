//! Talking to an already running instance:
//!
//! * launching opendevicecontrol a second time shows the running instance's window
//!   (opening it when OpenDeviceControl is in the tray);
//! * `opendevicecontrol --quit` asks the running instance to exit cleanly (installers
//!   use it so the PSU fan is handed back to hardware control before files
//!   are replaced) and waits for it to confirm that its devices were released.

use crate::tray::ShowFn;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Set by the GUI when it starts.
static SHOW: OnceLock<ShowFn> = OnceLock::new();

pub fn set_show(f: ShowFn) {
    let _ = SHOW.set(f);
}

/// How `opendevicecontrol --quit` ended.
#[derive(Debug, Clone, PartialEq)]
pub enum QuitOutcome {
    /// No instance was running.
    NotRunning,
    /// It exited and handed every device back to hardware control.
    Stopped,
    /// It stopped, but reported a failure (or exited without confirming).
    Failed(String),
    /// It did not finish within the timeout.
    TimedOut,
}

impl QuitOutcome {
    /// Process exit status for `--quit` (see the help text).
    pub fn exit_code(&self) -> i32 {
        match self {
            QuitOutcome::Stopped => 0,
            QuitOutcome::NotRunning => 1,
            QuitOutcome::TimedOut => 2,
            QuitOutcome::Failed(_) => 3,
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE, SYNCHRONIZATION_SYNCHRONIZE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, FindWindowW, SetForegroundWindow, ShowWindow, ASFW_ANY, SW_RESTORE, SW_SHOW,
    };

    /// This instance's answer events (ok, failed), created when it starts
    /// serving quit requests. Created here rather than by the requester, so
    /// an elevated installer can still wait on them.
    static ANSWERS: OnceLock<(usize, usize)> = OnceLock::new();

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn quit_event_name() -> Vec<u16> {
        wide("Local\\opendevicecontrol-quit")
    }

    /// Set by the instance when it stopped and released every device.
    fn quit_ok_name() -> Vec<u16> {
        wide("Local\\opendevicecontrol-quit-ok")
    }

    /// Set by the instance when it stopped but a device was not released.
    fn quit_failed_name() -> Vec<u16> {
        wide("Local\\opendevicecontrol-quit-failed")
    }

    fn show_event_name() -> Vec<u16> {
        wide("Local\\opendevicecontrol-show")
    }

    pub fn notify_existing() -> bool {
        let name = show_event_name();
        let ev = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
        if !ev.is_null() {
            unsafe {
                // This process was just started by the user, so it may hand
                // the foreground to the running instance's window.
                AllowSetForegroundWindow(ASFW_ANY);
                SetEvent(ev);
                CloseHandle(ev);
            }
            return true;
        }
        // An older version without the event: find its window.
        // Match winit's window class as well as the title, so an Explorer
        // window that happens to be named "opendevicecontrol" is not picked up.
        let title = wide("OpenDeviceControl");
        let class = wide("Window Class");
        unsafe {
            let hwnd = FindWindowW(class.as_ptr(), title.as_ptr());
            if hwnd.is_null() {
                return false;
            }
            ShowWindow(hwnd, SW_SHOW);
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
        true
    }

    pub fn listen(quit: ShowFn) {
        let name = show_event_name();
        let ev = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        if ev.is_null() {
            log::warn!("could not create show event");
        } else {
            let ev = ev as usize;
            std::thread::Builder::new()
                .name("opendevicecontrol-show".into())
                .spawn(move || {
                    while unsafe { WaitForSingleObject(ev as _, INFINITE) } == WAIT_OBJECT_0 {
                        if let Some(show) = SHOW.get() {
                            show();
                        }
                    }
                })
                .expect("spawn ipc");
        }
        let name = quit_event_name();
        let ev = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        if ev.is_null() {
            return log::warn!("could not create quit event");
        }
        // Manual reset: stay set for a requester that looks late.
        let (ok, failed) = (quit_ok_name(), quit_failed_name());
        let ok = unsafe { CreateEventW(std::ptr::null(), 1, 0, ok.as_ptr()) };
        let failed = unsafe { CreateEventW(std::ptr::null(), 1, 0, failed.as_ptr()) };
        if ok.is_null() || failed.is_null() {
            log::warn!("could not create quit answer events");
        } else {
            let _ = ANSWERS.set((ok as usize, failed as usize));
        }
        let ev = ev as usize;
        std::thread::Builder::new()
            .name("opendevicecontrol-ipc".into())
            .spawn(move || {
                if unsafe { WaitForSingleObject(ev as _, INFINITE) } == WAIT_OBJECT_0 {
                    quit();
                }
            })
            .expect("spawn ipc");
    }

    /// Answer a waiting `--quit` (harmless when nobody waits).
    pub fn report_shutdown(r: &Result<(), String>) {
        if let Some(&(ok, failed)) = ANSWERS.get() {
            unsafe { SetEvent((if r.is_ok() { ok } else { failed }) as HANDLE) };
        }
    }

    fn signalled(h: HANDLE) -> bool {
        unsafe { WaitForSingleObject(h, 0) == WAIT_OBJECT_0 }
    }

    pub fn request_quit(timeout: Duration) -> QuitOutcome {
        let name = quit_event_name();
        let ev = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
        if ev.is_null() {
            return QuitOutcome::NotRunning;
        }
        // Opened before asking, so they outlive the instance. Missing in
        // versions before them, which are then trusted once they exit.
        let (ok_name, failed_name) = (quit_ok_name(), quit_failed_name());
        let ok = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, ok_name.as_ptr()) };
        let failed = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, failed_name.as_ptr()) };
        let legacy = ok.is_null() || failed.is_null();
        unsafe {
            SetEvent(ev);
            CloseHandle(ev);
        }
        let answer = || {
            if !ok.is_null() && signalled(ok) {
                Some(QuitOutcome::Stopped)
            } else if !failed.is_null() && signalled(failed) {
                Some(QuitOutcome::Failed("a device was not handed back to hardware control (see its log)".into()))
            } else {
                None
            }
        };
        let deadline = Instant::now() + timeout;
        let outcome = loop {
            if let Some(o) = answer() {
                break o;
            }
            // The quit event disappears when the process holding it exits.
            let h = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
            if h.is_null() {
                // It may have answered just before exiting.
                break answer().unwrap_or_else(|| {
                    if legacy {
                        QuitOutcome::Stopped
                    } else {
                        QuitOutcome::Failed("it exited without confirming that its devices were released".into())
                    }
                });
            }
            unsafe { CloseHandle(h) };
            if Instant::now() >= deadline {
                break QuitOutcome::TimedOut;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        unsafe {
            for h in [ok, failed] {
                if !h.is_null() {
                    CloseHandle(h);
                }
            }
        }
        outcome
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    /// Deadline for reading a command or writing a reply.
    const IO_TIMEOUT: Duration = Duration::from_secs(2);
    /// Longest command line accepted.
    const MAX_COMMAND: usize = 16;

    /// The socket path while this process serves requests.
    static LISTENING: OnceLock<PathBuf> = OnceLock::new();
    /// `--quit` connections waiting for the shutdown result.
    static PENDING: Mutex<Vec<UnixStream>> = Mutex::new(Vec::new());

    fn uid() -> u32 {
        extern "C" {
            fn getuid() -> u32;
        }
        unsafe { getuid() }
    }

    /// A directory only this user can enter, so only this user (and root)
    /// can reach the socket. `$XDG_RUNTIME_DIR` is already private; the temp
    /// directory fallback is shared, hence the subdirectory and its checks.
    fn socket_dir() -> PathBuf {
        let base = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
        base.join(format!("opendevicecontrol-{}", uid()))
    }

    fn socket_path() -> PathBuf {
        socket_dir().join("ipc.sock")
    }

    /// Where versions before the private directory listened.
    fn legacy_socket_path() -> Option<PathBuf> {
        dirs::runtime_dir().map(|d| d.join(format!("opendevicecontrol-{}.sock", uid())))
    }

    /// Create `dir` (mode 0700) or accept an existing one only if it is a
    /// real directory owned by `owner`; tightens its mode if needed.
    pub(super) fn private_dir(dir: &Path, owner: u32) -> std::io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
            _ => {}
        }
        let m = std::fs::symlink_metadata(dir)?;
        if !m.file_type().is_dir() || m.uid() != owner {
            return Err(std::io::Error::other("exists but is not a directory owned by this user"));
        }
        if m.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    /// The connecting process's user, where the OS tells us.
    #[cfg(target_os = "linux")]
    fn peer_uid(s: &UnixStream) -> Option<u32> {
        use std::os::fd::AsRawFd;
        let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let r = unsafe {
            libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len)
        };
        (r == 0).then_some(cred.uid)
    }

    /// Elsewhere the private directory is the only check.
    #[cfg(not(target_os = "linux"))]
    fn peer_uid(_: &UnixStream) -> Option<u32> {
        None
    }

    /// Read one line (without the newline), at most `max` bytes, within
    /// `timeout` in total. A peer that closes early yields what it sent.
    pub(super) fn read_line(s: &mut UnixStream, max: usize, timeout: Duration) -> std::io::Result<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 64];
        while !buf.contains(&b'\n') && buf.len() < max {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            s.set_read_timeout(Some(left))?;
            let n = s.read(&mut chunk[..(max - buf.len()).min(chunk.len())])?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        if let Some(i) = buf.iter().position(|&b| b == b'\n') {
            buf.truncate(i);
        }
        Ok(buf)
    }

    #[derive(Debug, PartialEq)]
    pub(super) enum Request {
        Show,
        Quit,
    }

    pub(super) fn parse_request(line: &[u8]) -> Option<Request> {
        match line.trim_ascii() {
            b"show" => Some(Request::Show),
            b"quit" => Some(Request::Quit),
            _ => None,
        }
    }

    pub(super) fn reply(r: &Result<(), String>) -> String {
        match r {
            Ok(()) => "ok\n".into(),
            Err(e) => format!("error {}\n", e.replace(['\n', '\r'], " ")),
        }
    }

    pub(super) fn parse_reply(line: &[u8]) -> QuitOutcome {
        let line = String::from_utf8_lossy(line);
        let line = line.trim();
        if line == "ok" {
            QuitOutcome::Stopped
        } else if let Some(e) = line.strip_prefix("error") {
            QuitOutcome::Failed(e.trim().to_string())
        } else {
            // Closed without answering: crashed, or an older version.
            QuitOutcome::Failed("it exited without confirming that its devices were released".into())
        }
    }

    fn connect() -> Option<UnixStream> {
        let s = UnixStream::connect(socket_path()).ok().or_else(|| UnixStream::connect(legacy_socket_path()?).ok())?;
        let _ = s.set_write_timeout(Some(IO_TIMEOUT));
        Some(s)
    }

    pub fn notify_existing() -> bool {
        connect().is_some_and(|mut s| s.write_all(b"show\n").is_ok())
    }

    pub fn listen(quit: ShowFn) {
        let dir = socket_dir();
        if let Err(e) = private_dir(&dir, uid()) {
            return log::warn!("IPC directory {}: {e}", dir.display());
        }
        let path = socket_path();
        let _ = std::fs::remove_file(&path);
        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => return log::warn!("IPC socket {}: {e}", path.display()),
        };
        let _ = LISTENING.set(path);
        std::thread::Builder::new()
            .name("opendevicecontrol-ipc".into())
            .spawn(move || {
                for mut conn in listener.incoming().flatten() {
                    if let Some(peer) = peer_uid(&conn).filter(|&p| p != uid()) {
                        log::warn!("IPC: ignoring a connection from user {peer}");
                        continue;
                    }
                    let _ = conn.set_write_timeout(Some(IO_TIMEOUT));
                    // A silent or slow client only delays the next request.
                    let Ok(line) = read_line(&mut conn, MAX_COMMAND, IO_TIMEOUT) else { continue };
                    match parse_request(&line) {
                        Some(Request::Show) => {
                            if let Some(show) = SHOW.get() {
                                show();
                            }
                        }
                        Some(Request::Quit) => {
                            // Answered by `report_shutdown` once devices are released.
                            PENDING.lock().unwrap_or_else(|e| e.into_inner()).push(conn);
                            quit();
                        }
                        None => {}
                    }
                }
            })
            .expect("spawn ipc");
    }

    /// Answer every waiting `--quit` (no-op when nobody waits) and stop
    /// accepting new requests.
    pub fn report_shutdown(r: &Result<(), String>) {
        let Some(path) = LISTENING.get() else { return };
        let _ = std::fs::remove_file(path);
        let msg = reply(r);
        for mut s in PENDING.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            let _ = s.write_all(msg.as_bytes());
        }
    }

    pub fn request_quit(timeout: Duration) -> QuitOutcome {
        let Some(mut s) = connect() else { return QuitOutcome::NotRunning };
        if let Err(e) = s.write_all(b"quit\n") {
            return QuitOutcome::Failed(format!("could not send the request: {e}"));
        }
        match read_line(&mut s, 512, timeout) {
            Ok(line) => parse_reply(&line),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => QuitOutcome::TimedOut,
            Err(e) => QuitOutcome::Failed(format!("no answer: {e}")),
        }
    }
}

pub use imp::{listen, notify_existing, report_shutdown, request_quit};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_exit_codes_are_distinct() {
        let codes = [
            QuitOutcome::Stopped.exit_code(),
            QuitOutcome::NotRunning.exit_code(),
            QuitOutcome::TimedOut.exit_code(),
            QuitOutcome::Failed(String::new()).exit_code(),
        ];
        assert_eq!(codes, [0, 1, 2, 3]);
    }

    #[cfg(unix)]
    mod unix {
        use super::super::imp::*;
        use super::super::QuitOutcome;
        use std::io::Write;
        use std::os::unix::net::UnixStream;
        use std::time::Duration;

        #[test]
        fn requests_and_replies_round_trip() {
            assert_eq!(parse_request(b"show"), Some(Request::Show));
            assert_eq!(parse_request(b"quit\r"), Some(Request::Quit));
            assert_eq!(parse_request(b"quitx"), None);
            assert_eq!(parse_reply(reply(&Ok(())).trim_end().as_bytes()), QuitOutcome::Stopped);
            let failed = reply(&Err("hxi: no\nreply".into()));
            assert_eq!(failed.matches('\n').count(), 1);
            assert_eq!(parse_reply(failed.trim_end().as_bytes()), QuitOutcome::Failed("hxi: no reply".into()));
            assert!(matches!(parse_reply(b""), QuitOutcome::Failed(_)));
        }

        #[test]
        fn fragmented_command_is_reassembled() {
            let (mut a, mut b) = UnixStream::pair().unwrap();
            let t = std::thread::spawn(move || {
                a.write_all(b"qu").unwrap();
                std::thread::sleep(Duration::from_millis(50));
                a.write_all(b"it\nextra").unwrap();
                a
            });
            assert_eq!(read_line(&mut b, 16, Duration::from_secs(2)).unwrap(), b"quit");
            drop(t.join());
        }

        #[test]
        fn idle_client_times_out_and_long_lines_are_capped() {
            let (_a, mut b) = UnixStream::pair().unwrap();
            assert!(read_line(&mut b, 16, Duration::from_millis(100)).is_err());
            let (mut a, mut b) = UnixStream::pair().unwrap();
            a.write_all(&[b'x'; 100]).unwrap();
            assert_eq!(read_line(&mut b, 16, Duration::from_secs(1)).unwrap().len(), 16);
        }

        #[test]
        fn private_dir_is_created_0700_and_foreign_dirs_rejected() {
            use std::os::unix::fs::PermissionsExt;
            let base = std::env::temp_dir().join(format!("odc-ipc-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            let me = std::fs::metadata(&base).map(|m| std::os::unix::fs::MetadataExt::uid(&m)).unwrap();

            let dir = base.join("private");
            private_dir(&dir, me).unwrap();
            assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
            // An existing directory with loose permissions is tightened.
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            private_dir(&dir, me).unwrap();
            assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
            // Someone else's directory, a file, or a symlink is refused.
            assert!(private_dir(&dir, me.wrapping_add(1)).is_err());
            let file = base.join("file");
            std::fs::write(&file, b"").unwrap();
            assert!(private_dir(&file, me).is_err());
            let link = base.join("link");
            std::os::unix::fs::symlink(&dir, &link).unwrap();
            assert!(private_dir(&link, me).is_err());
            let _ = std::fs::remove_dir_all(&base);
        }
    }
}
