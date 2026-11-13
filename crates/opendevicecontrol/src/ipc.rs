//! Talking to an already running instance:
//!
//! * launching opendevicecontrol a second time shows the running instance's window
//!   (opening it when OpenDeviceControl is in the tray);
//! * `opendevicecontrol --quit` asks the running instance to exit cleanly (installers
//!   use it so the PSU fan is handed back to hardware control before files
//!   are replaced).

use crate::tray::ShowFn;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Set by the GUI when it starts.
static SHOW: OnceLock<ShowFn> = OnceLock::new();

pub fn set_show(f: ShowFn) {
    let _ = SHOW.set(f);
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, FindWindowW, SetForegroundWindow, ShowWindow, ASFW_ANY, SW_RESTORE, SW_SHOW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn quit_event_name() -> Vec<u16> {
        wide("Local\\opendevicecontrol-quit")
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

    pub fn request_quit(timeout: Duration) -> bool {
        let name = quit_event_name();
        let ev = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
        if ev.is_null() {
            return false;
        }
        unsafe {
            SetEvent(ev);
            CloseHandle(ev);
        }
        // The event disappears when the process holding it exits.
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let h = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
            if h.is_null() {
                return true;
            }
            unsafe { CloseHandle(h) };
            std::thread::sleep(Duration::from_millis(100));
        }
        true
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};

    fn socket_path() -> std::path::PathBuf {
        extern "C" {
            fn getuid() -> u32;
        }
        let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
        dir.join(format!("opendevicecontrol-{}.sock", unsafe { getuid() }))
    }

    fn send(msg: &[u8]) -> bool {
        match UnixStream::connect(socket_path()) {
            Ok(mut s) => s.write_all(msg).is_ok(),
            Err(_) => false,
        }
    }

    pub fn notify_existing() -> bool {
        send(b"show\n")
    }

    pub fn listen(quit: ShowFn) {
        let path = socket_path();
        let _ = std::fs::remove_file(&path);
        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => return log::warn!("IPC socket {}: {e}", path.display()),
        };
        std::thread::Builder::new()
            .name("opendevicecontrol-ipc".into())
            .spawn(move || {
                for mut conn in listener.incoming().flatten() {
                    let mut buf = [0u8; 16];
                    let n = conn.read(&mut buf).unwrap_or(0);
                    match &buf[..n] {
                        b if b.starts_with(b"show") => {
                            if let Some(show) = SHOW.get() {
                                show();
                            }
                        }
                        b if b.starts_with(b"quit") => {
                            let _ = std::fs::remove_file(socket_path());
                            quit();
                        }
                        _ => {}
                    }
                }
            })
            .expect("spawn ipc");
    }

    pub fn request_quit(timeout: Duration) -> bool {
        if !send(b"quit\n") {
            return false;
        }
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline && UnixStream::connect(socket_path()).is_ok() {
            std::thread::sleep(Duration::from_millis(100));
        }
        true
    }
}

pub use imp::{listen, notify_existing, request_quit};
