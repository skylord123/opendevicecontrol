//! Start opendevicecontrol when the user logs in.
//!
//! * Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\opendevicecontrol`
//! * Linux:   `$XDG_CONFIG_HOME/autostart/opendevicecontrol.desktop` (XDG autostart spec;
//!   honoured by GNOME, COSMIC, KDE, XFCE…)

use std::io;
use std::path::PathBuf;

const NAME: &str = "opendevicecontrol";

fn exe() -> io::Result<PathBuf> {
    // An AppImage's exe path is a temporary mount; use the image itself.
    if let Some(p) = std::env::var_os("APPIMAGE") {
        return Ok(PathBuf::from(p));
    }
    std::env::current_exe()
}

#[cfg(windows)]
mod imp {
    use super::*;
    use winreg::enums::*;
    use winreg::RegKey;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn command() -> io::Result<String> {
        Ok(format!("\"{}\" --minimized", exe()?.display()))
    }

    pub fn is_enabled() -> bool {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(RUN)
            .and_then(|k| k.get_value::<String, _>(NAME))
            .is_ok()
    }

    pub fn set_enabled(on: bool) -> io::Result<()> {
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN)?;
        if on {
            key.set_value(NAME, &command()?)
        } else {
            match key.delete_value(NAME) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                r => r,
            }
        }
    }

    pub fn location() -> String {
        format!(r"HKCU\{RUN}\{NAME}")
    }

    /// Remove the previous name's entry; true if one existed.
    pub fn remove_legacy() -> bool {
        let Ok(k) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN, KEY_READ | KEY_WRITE) else { return false };
        // Every previous name: `|` so all are removed.
        super::LEGACY_NAMES.iter().fold(false, |had, n| k.delete_value(n).is_ok() | had)
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    fn path() -> PathBuf {
        dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config")).join("autostart").join(format!("{NAME}.desktop"))
    }

    fn quote(p: &std::path::Path) -> String {
        // Desktop Entry spec quoting.
        let s = p.display().to_string();
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"").replace('`', "\\`").replace('$', "\\$");
        format!("\"{escaped}\"")
    }

    pub fn is_enabled() -> bool {
        std::fs::read_to_string(path()).map(|s| !s.contains("Hidden=true")).unwrap_or(false)
    }

    pub fn set_enabled(on: bool) -> io::Result<()> {
        let p = path();
        if on {
            std::fs::create_dir_all(p.parent().unwrap())?;
            let body = format!(
                "[Desktop Entry]\n\
                 Type=Application\n\
                 Name=OpenDeviceControl\n\
                 Comment=PC hardware monitor and Home Assistant bridge\n\
                 Exec={} --minimized\n\
                 Icon=opendevicecontrol\n\
                 Terminal=false\n\
                 Categories=System;Monitor;\n\
                 X-GNOME-Autostart-enabled=true\n\
                 X-GNOME-Autostart-Delay=5\n",
                quote(&exe()?)
            );
            std::fs::write(p, body)
        } else {
            match std::fs::remove_file(p) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                r => r,
            }
        }
    }

    pub fn location() -> String {
        path().display().to_string()
    }

    /// Remove the previous name's entry; true if one existed.
    pub fn remove_legacy() -> bool {
        let dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config")).join("autostart");
        super::LEGACY_NAMES.iter().fold(false, |had, n| std::fs::remove_file(dir.join(format!("{n}.desktop"))).is_ok() | had)
    }
}

pub use imp::{is_enabled, location, set_enabled};

use crate::config::LEGACY_NAMES;

/// Replace a start-at-login entry left by a previous name with ours.
/// Returns true if the user had autostart enabled under the old name.
pub fn migrate_legacy() -> bool {
    let had = imp::remove_legacy();
    if had {
        log::info!("autostart: replaced the previous name's login entry");
        if let Err(e) = set_enabled(true) {
            log::warn!("autostart: {e}");
        }
    }
    had
}

/// Make the OS entry match the config (and refresh the exe path, in case the
/// binary moved since autostart was enabled).
pub fn sync(want: bool) {
    let result = if want { set_enabled(true) } else if is_enabled() { set_enabled(false) } else { Ok(()) };
    if let Err(e) = result {
        log::warn!("autostart: {e}");
    }
}
