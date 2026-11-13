//! When a key was last pressed, for devices whose live lighting gets in the
//! way of their own key handling (the Aula F99 over its cable).
//!
//! * Windows: a background thread receives Raw Input from every keyboard
//!   (a message-only window registered with `RIDEV_INPUTSINK`). Raw Input
//!   only reports keys; unlike a keyboard hook it can't slow typing down.
//! * Elsewhere: drivers that see their own keyboard's reports call
//!   [`note_key`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

static EPOCH: OnceLock<Instant> = OnceLock::new();
/// Milliseconds after [`EPOCH`] of the last key event, plus one (0 = none).
static LAST: AtomicU64 = AtomicU64::new(0);

fn epoch() -> Instant {
    *EPOCH.get_or_init(Instant::now)
}

/// Record a key event now.
pub fn note_key() {
    let ms = epoch().elapsed().as_millis() as u64 + 1;
    LAST.fetch_max(ms, Ordering::Relaxed);
}

/// Time since the last key event seen, if any.
pub fn since_last_key() -> Option<Duration> {
    imp::start();
    match LAST.load(Ordering::Relaxed) {
        0 => None,
        ms => Some(epoch().elapsed().saturating_sub(Duration::from_millis(ms - 1))),
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::Once;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::{RegisterRawInputDevices, RAWINPUTDEVICE, RIDEV_INPUTSINK};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW, HWND_MESSAGE, MSG, WM_INPUT,
        WNDCLASSW,
    };

    static START: Once = Once::new();

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_INPUT {
            // Only keyboards are registered: any input is a key event.
            super::note_key();
        }
        // SAFETY: forwarding the message unchanged (WM_INPUT needs it for cleanup).
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }

    pub fn start() {
        START.call_once(|| {
            let spawned = std::thread::Builder::new().name("raw-input".into()).spawn(|| {
                if let Err(e) = run() {
                    log::warn!("key activity: {e}");
                }
            });
            if let Err(e) = spawned {
                log::warn!("key activity thread: {e}");
            }
        });
    }

    fn run() -> Result<(), String> {
        let class: Vec<u16> = "OpenDeviceControlRawInput\0".encode_utf16().collect();
        // SAFETY: plain Win32 calls; `class` outlives the window, and the
        // message loop runs on the thread that owns the window.
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
            if RegisterClassW(&wc) == 0 {
                return Err("RegisterClassW failed".into());
            }
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return Err("CreateWindowExW failed".into());
            }
            // Generic desktop page, keyboard usage; delivered in the background.
            let dev = RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: 0x06, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd };
            if RegisterRawInputDevices(&dev, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32) == 0 {
                return Err("RegisterRawInputDevices failed".into());
            }
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                DispatchMessageW(&msg);
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn start() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noted_keys_are_recent() {
        note_key();
        let d = since_last_key().expect("a key was noted");
        assert!(d < Duration::from_secs(1));
    }
}
