//! Is the user at the computer? Session lock state and time since the last
//! keyboard/mouse input.
//!
//! * Windows: lock state from `WTSQuerySessionInformation(WTSSessionInfoEx)`
//!   (the session's real lock flag, so a UAC prompt is not "locked"); idle
//!   time from `GetLastInputInfo`.
//! * Linux: logind (`LockedHint`, `IdleHint`/`IdleSinceHint`) over D-Bus;
//!   on GNOME, Mutter's idle monitor gives the exact time since last input.
//!   How promptly `IdleHint` is set depends on the desktop.

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Presence {
    /// `None` when the OS doesn't tell us.
    pub locked: Option<bool>,
    /// Seconds since the last keyboard/mouse input.
    pub idle_secs: Option<f64>,
}

pub use imp::Monitor;

#[cfg(windows)]
mod imp {
    use super::Presence;
    use windows_sys::Win32::System::RemoteDesktop::{
        WTSFreeMemory, WTSQuerySessionInformationW, WTSSessionInfoEx, WTSINFOEXW, WTS_CURRENT_SERVER_HANDLE,
        WTS_CURRENT_SESSION,
    };
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

    /// `WTSINFOEX_LEVEL1.SessionFlags` values (Windows 8 and later).
    const WTS_SESSIONSTATE_LOCK: i32 = 0;
    const WTS_SESSIONSTATE_UNLOCK: i32 = 1;

    #[derive(Default)]
    pub struct Monitor;

    impl Monitor {
        pub fn new() -> Self {
            Monitor
        }

        pub fn read(&mut self) -> Presence {
            Presence { locked: locked(), idle_secs: idle_secs() }
        }
    }

    fn locked() -> Option<bool> {
        unsafe {
            let mut buf: windows_sys::core::PWSTR = std::ptr::null_mut();
            let mut len = 0u32;
            if WTSQuerySessionInformationW(WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTSSessionInfoEx, &mut buf, &mut len) == 0
                || buf.is_null()
            {
                return None;
            }
            let info = &*(buf as *const WTSINFOEXW);
            let state = if info.Level == 1 {
                match info.Data.WTSInfoExLevel1.SessionFlags {
                    WTS_SESSIONSTATE_LOCK => Some(true),
                    WTS_SESSIONSTATE_UNLOCK => Some(false),
                    _ => None,
                }
            } else {
                None
            };
            WTSFreeMemory(buf as _);
            state
        }
    }

    fn idle_secs() -> Option<f64> {
        let mut lii = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        unsafe {
            if GetLastInputInfo(&mut lii) == 0 {
                return None;
            }
            // Both are 32-bit millisecond tick counts; wrapping_sub copes
            // with the 49.7-day wrap.
            Some(GetTickCount().wrapping_sub(lii.dwTime) as f64 / 1000.0)
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::Presence;
    use std::time::{SystemTime, UNIX_EPOCH};
    use zbus::blocking::{Connection, Proxy};

    const LOGIN1: &str = "org.freedesktop.login1";

    #[derive(Default)]
    pub struct Monitor {
        system: Option<Connection>,
        session: Option<Connection>,
        session_path: Option<String>,
        /// Mutter's idle monitor answered at least once (GNOME).
        mutter: Option<bool>,
    }

    impl Monitor {
        pub fn new() -> Self {
            let mut m = Monitor { system: Connection::system().ok(), session: Connection::session().ok(), ..Default::default() };
            m.session_path = m.find_session();
            if m.session_path.is_none() {
                log::info!("presence: no logind session found; lock/idle state unavailable");
            }
            m
        }

        /// Our logind session: `session/auto` when OpenDeviceControl runs inside the
        /// graphical session, else the user's display session (systemd
        /// user services run outside any session).
        fn find_session(&self) -> Option<String> {
            let conn = self.system.as_ref()?;
            let auto = "/org/freedesktop/login1/session/auto";
            if let Ok(p) = Proxy::new(conn, LOGIN1, auto, "org.freedesktop.login1.Session") {
                if p.get_property::<String>("Id").is_ok() {
                    return Some(auto.into());
                }
            }
            let mgr = Proxy::new(conn, LOGIN1, "/org/freedesktop/login1", "org.freedesktop.login1.Manager").ok()?;
            let uid = unsafe { getuid() };
            let user: zbus::zvariant::OwnedObjectPath = mgr.call("GetUser", &(uid,)).ok()?;
            let up = Proxy::new(conn, LOGIN1, user.as_str(), "org.freedesktop.login1.User").ok()?;
            let (_, path): (String, zbus::zvariant::OwnedObjectPath) = up.get_property("Display").ok()?;
            (path.as_str() != "/").then(|| path.as_str().to_string())
        }

        fn session_proxy(&self) -> Option<Proxy<'_>> {
            Proxy::new(self.system.as_ref()?, LOGIN1, self.session_path.clone()?, "org.freedesktop.login1.Session").ok()
        }

        fn mutter_idle(&mut self) -> Option<f64> {
            if self.mutter == Some(false) {
                return None;
            }
            let conn = self.session.as_ref()?;
            let r = conn
                .call_method(
                    Some("org.gnome.Mutter.IdleMonitor"),
                    "/org/gnome/Mutter/IdleMonitor/Core",
                    Some("org.gnome.Mutter.IdleMonitor"),
                    "GetIdletime",
                    &(),
                )
                .and_then(|m| m.body().deserialize::<u64>());
            match r {
                Ok(ms) => {
                    self.mutter = Some(true);
                    Some(ms as f64 / 1000.0)
                }
                Err(_) => {
                    if self.mutter.is_none() {
                        self.mutter = Some(false);
                    }
                    None
                }
            }
        }

        pub fn read(&mut self) -> Presence {
            let locked = self.session_proxy().and_then(|p| p.get_property::<bool>("LockedHint").ok());
            let idle_secs = self.mutter_idle().or_else(|| {
                let p = self.session_proxy()?;
                if !p.get_property::<bool>("IdleHint").ok()? {
                    return Some(0.0);
                }
                // IdleSinceHint: CLOCK_REALTIME microseconds.
                let since: u64 = p.get_property("IdleSinceHint").ok()?;
                let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_micros() as u64;
                Some(now.saturating_sub(since) as f64 / 1_000_000.0)
            });
            Presence { locked, idle_secs }
        }
    }

    extern "C" {
        fn getuid() -> u32;
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::Presence;
    #[derive(Default)]
    pub struct Monitor;
    impl Monitor {
        pub fn new() -> Self {
            Monitor
        }
        pub fn read(&mut self) -> Presence {
            Presence::default()
        }
    }
}
