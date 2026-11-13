//! Synthesised keyboard input and program launching for key actions (macros).
//!
//! * Windows: `SendInput` (text as Unicode, so any characters work).
//! * Linux: a uinput virtual keyboard (works on X11 and Wayland). Needs write
//!   access to `/dev/uinput`, granted by opendevicecontrol's udev rule. Text is typed
//!   with a US keyboard layout.

use crate::config::KeyAction;
use crossbeam_channel::Sender;
use std::sync::OnceLock;

/// (name, Windows virtual-key code, extended-key flag, Linux evdev code)
const KEYS: &[(&str, u16, bool, u16)] = &[
    ("esc", 0x1b, false, 1),
    ("escape", 0x1b, false, 1),
    ("tab", 0x09, false, 15),
    ("enter", 0x0d, false, 28),
    ("return", 0x0d, false, 28),
    ("space", 0x20, false, 57),
    ("backspace", 0x08, false, 14),
    ("delete", 0x2e, true, 111),
    ("del", 0x2e, true, 111),
    ("insert", 0x2d, true, 110),
    ("home", 0x24, true, 102),
    ("end", 0x23, true, 107),
    ("pageup", 0x21, true, 104),
    ("pagedown", 0x22, true, 109),
    ("up", 0x26, true, 103),
    ("down", 0x28, true, 108),
    ("left", 0x25, true, 105),
    ("right", 0x27, true, 106),
    ("printscreen", 0x2c, true, 99),
    ("pause", 0x13, false, 119),
    ("capslock", 0x14, false, 58),
    ("ctrl", 0xa2, false, 29),
    ("control", 0xa2, false, 29),
    ("shift", 0xa0, false, 42),
    ("alt", 0xa4, false, 56),
    ("win", 0x5b, true, 125),
    ("super", 0x5b, true, 125),
    ("meta", 0x5b, true, 125),
    ("cmd", 0x5b, true, 125),
    ("minus", 0xbd, false, 12),
    ("-", 0xbd, false, 12),
    ("equal", 0xbb, false, 13),
    ("=", 0xbb, false, 13),
    ("comma", 0xbc, false, 51),
    (",", 0xbc, false, 51),
    ("period", 0xbe, false, 52),
    (".", 0xbe, false, 52),
    ("slash", 0xbf, false, 53),
    ("/", 0xbf, false, 53),
    ("semicolon", 0xba, false, 39),
    (";", 0xba, false, 39),
    ("quote", 0xde, false, 40),
    ("'", 0xde, false, 40),
    ("grave", 0xc0, false, 41),
    ("`", 0xc0, false, 41),
    ("[", 0xdb, false, 26),
    ("]", 0xdd, false, 27),
    ("backslash", 0xdc, false, 43),
    ("\\", 0xdc, false, 43),
    // Media keys.
    ("play_pause", 0xb3, true, 164),
    ("next", 0xb0, true, 163),
    ("previous", 0xb1, true, 165),
    ("stop", 0xb2, true, 166),
    ("mute", 0xad, true, 113),
    ("volume_up", 0xaf, true, 115),
    ("volume_down", 0xae, true, 114),
];

pub const MEDIA_KEYS: [(&str, &str); 7] = [
    ("play_pause", "Play / pause"),
    ("next", "Next track"),
    ("previous", "Previous track"),
    ("stop", "Stop"),
    ("mute", "Mute"),
    ("volume_up", "Volume up"),
    ("volume_down", "Volume down"),
];

/// Linux evdev codes for a–z (QWERTY order of the alphabet).
const LETTER_CODES: [u16; 26] = [30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44];
/// Linux evdev codes for 0–9.
const DIGIT_CODES: [u16; 10] = [11, 2, 3, 4, 5, 6, 7, 8, 9, 10];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub vk: u16,
    pub extended: bool,
    pub evdev: u16,
}

/// Look up a key by name: `a`–`z`, `0`–`9`, `f1`–`f24`, or a name in `KEYS`.
pub fn key(name: &str) -> Option<Key> {
    let n = name.trim().to_lowercase();
    if let Some((_, vk, ext, ev)) = KEYS.iter().find(|(k, ..)| *k == n) {
        return Some(Key { vk: *vk, extended: *ext, evdev: *ev });
    }
    let mut chars = n.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_lowercase() {
            let i = (c as u8 - b'a') as usize;
            return Some(Key { vk: c.to_ascii_uppercase() as u16, extended: false, evdev: LETTER_CODES[i] });
        }
        if c.is_ascii_digit() {
            let i = (c as u8 - b'0') as usize;
            return Some(Key { vk: c as u16, extended: false, evdev: DIGIT_CODES[i] });
        }
    }
    if let Some(num) = n.strip_prefix('f').and_then(|s| s.parse::<u16>().ok()) {
        if (1..=24).contains(&num) {
            let evdev = match num {
                1..=10 => 58 + num,
                11 => 87,
                12 => 88,
                _ => 170 + num, // F13 = 183
            };
            return Some(Key { vk: 0x6f + num, extended: false, evdev });
        }
    }
    None
}

/// Parse `ctrl+shift+esc` into keys. Errors name the unknown part.
pub fn parse_hotkey(s: &str) -> Result<Vec<Key>, String> {
    let parts: Vec<&str> = s.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err("empty hotkey".into());
    }
    parts.iter().map(|p| key(p).ok_or_else(|| format!("unknown key '{p}'"))).collect()
}

enum Job {
    Action(KeyAction),
}

static WORKER: OnceLock<Sender<Job>> = OnceLock::new();

/// Run a key action in the background (actions never block device I/O).
pub fn run(action: KeyAction) {
    if matches!(action, KeyAction::None) {
        return;
    }
    let tx = WORKER.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::unbounded::<Job>();
        std::thread::Builder::new()
            .name("opendevicecontrol-macros".into())
            .spawn(move || {
                // A unit struct on Windows, not on Linux.
                #[allow(clippy::default_constructed_unit_structs)]
                let mut backend = imp::Backend::default();
                for Job::Action(a) in rx {
                    if let Err(e) = execute(&mut backend, &a) {
                        log::warn!("key action {a:?} failed: {e}");
                    }
                }
            })
            .expect("spawn macro thread");
        tx
    });
    let _ = tx.send(Job::Action(action));
}

fn execute(b: &mut imp::Backend, a: &KeyAction) -> Result<(), String> {
    match a {
        KeyAction::None => Ok(()),
        KeyAction::Hotkey { keys } => {
            let keys = parse_hotkey(keys)?;
            b.chord(&keys)
        }
        KeyAction::Media { key: k } => b.chord(&[key(k).ok_or_else(|| format!("unknown media key '{k}'"))?]),
        KeyAction::Text { text } => b.text(text),
        KeyAction::Launch { command } => launch(command),
    }
}

fn launch(command: &str) -> Result<(), String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("no command".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // `start` resolves programs, documents and URLs like the Run dialog.
        std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .raw_arg(command)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("sh").args(["-c", command]).spawn().map(|_| ()).map_err(|e| e.to_string())
    }
}

#[cfg(windows)]
mod imp {
    use super::Key;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    };

    #[derive(Default)]
    pub struct Backend;

    fn kb(vk: u16, scan: u16, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
        }
    }

    fn send(inputs: &[INPUT]) -> Result<(), String> {
        let n = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
        if n as usize == inputs.len() { Ok(()) } else { Err("SendInput was blocked".into()) }
    }

    impl Backend {
        /// Press keys in order, release in reverse.
        pub fn chord(&mut self, keys: &[Key]) -> Result<(), String> {
            let ext = |k: &Key| if k.extended { KEYEVENTF_EXTENDEDKEY } else { 0 };
            let mut v: Vec<INPUT> = keys.iter().map(|k| kb(k.vk, 0, ext(k))).collect();
            v.extend(keys.iter().rev().map(|k| kb(k.vk, 0, ext(k) | KEYEVENTF_KEYUP)));
            send(&v)
        }

        pub fn text(&mut self, text: &str) -> Result<(), String> {
            let mut v = Vec::new();
            for unit in text.replace("\r\n", "\n").encode_utf16() {
                if unit == '\n' as u16 {
                    v.push(kb(0x0d, 0, 0));
                    v.push(kb(0x0d, 0, KEYEVENTF_KEYUP));
                } else {
                    v.push(kb(0, unit, KEYEVENTF_UNICODE));
                    v.push(kb(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
                }
            }
            send(&v)
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{Key, DIGIT_CODES, LETTER_CODES};
    use evdev::uinput::VirtualDevice;
    use evdev::{AttributeSet, EventType, InputEvent, KeyCode};
    use std::time::Duration;

    const SHIFT: u16 = 42;
    const SYN: u16 = 0; // EV_SYN / SYN_REPORT

    #[derive(Default)]
    pub struct Backend {
        dev: Option<VirtualDevice>,
    }

    impl Backend {
        fn dev(&mut self) -> Result<&mut VirtualDevice, String> {
            if self.dev.is_none() {
                let mut keys = AttributeSet::<KeyCode>::new();
                for code in 1..=248u16 {
                    keys.insert(KeyCode::new(code));
                }
                let dev = VirtualDevice::builder()
                    .and_then(|b| b.name("OpenDeviceControl virtual keyboard").with_keys(&keys))
                    .and_then(|b| b.build())
                    .map_err(|e| format!("cannot create virtual keyboard (/dev/uinput access?): {e}"))?;
                // Give the compositor/libinput time to pick the device up.
                std::thread::sleep(Duration::from_millis(300));
                self.dev = Some(dev);
            }
            Ok(self.dev.as_mut().unwrap())
        }

        fn key_events(&mut self, codes: &[(u16, i32)]) -> Result<(), String> {
            let dev = self.dev()?;
            for (code, value) in codes {
                let ev = [InputEvent::new(EventType::KEY.0, *code, *value), InputEvent::new(EventType::SYNCHRONIZATION.0, SYN, 0)];
                dev.emit(&ev).map_err(|e| e.to_string())?;
                std::thread::sleep(Duration::from_millis(4));
            }
            Ok(())
        }

        pub fn chord(&mut self, keys: &[Key]) -> Result<(), String> {
            let mut ev: Vec<(u16, i32)> = keys.iter().map(|k| (k.evdev, 1)).collect();
            ev.extend(keys.iter().rev().map(|k| (k.evdev, 0)));
            self.key_events(&ev)
        }

        pub fn text(&mut self, text: &str) -> Result<(), String> {
            for c in text.chars() {
                let Some((code, shift)) = us_key(c) else {
                    log::warn!("cannot type '{c}' (only US-layout ASCII is supported on Linux)");
                    continue;
                };
                let mut ev = Vec::new();
                if shift {
                    ev.push((SHIFT, 1));
                }
                ev.push((code, 1));
                ev.push((code, 0));
                if shift {
                    ev.push((SHIFT, 0));
                }
                self.key_events(&ev)?;
            }
            Ok(())
        }
    }

    /// evdev code and Shift state for a character on a US layout.
    fn us_key(c: char) -> Option<(u16, bool)> {
        if c.is_ascii_lowercase() {
            return Some((LETTER_CODES[(c as u8 - b'a') as usize], false));
        }
        if c.is_ascii_uppercase() {
            return Some((LETTER_CODES[(c as u8 - b'A') as usize], true));
        }
        if c.is_ascii_digit() {
            return Some((DIGIT_CODES[(c as u8 - b'0') as usize], false));
        }
        let shifted_digits = ")!@#$%^&*(";
        if let Some(i) = shifted_digits.find(c) {
            return Some((DIGIT_CODES[i], true));
        }
        Some(match c {
            ' ' => (57, false),
            '\n' => (28, false),
            '\t' => (15, false),
            '-' => (12, false),
            '_' => (12, true),
            '=' => (13, false),
            '+' => (13, true),
            '[' => (26, false),
            '{' => (26, true),
            ']' => (27, false),
            '}' => (27, true),
            '\\' => (43, false),
            '|' => (43, true),
            ';' => (39, false),
            ':' => (39, true),
            '\'' => (40, false),
            '"' => (40, true),
            '`' => (41, false),
            '~' => (41, true),
            ',' => (51, false),
            '<' => (51, true),
            '.' => (52, false),
            '>' => (52, true),
            '/' => (53, false),
            '?' => (53, true),
            _ => return None,
        })
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::Key;
    #[derive(Default)]
    pub struct Backend;
    impl Backend {
        pub fn chord(&mut self, _: &[Key]) -> Result<(), String> {
            Err("key actions are not supported on this OS".into())
        }
        pub fn text(&mut self, _: &str) -> Result<(), String> {
            Err("key actions are not supported on this OS".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_parse() {
        let k = parse_hotkey("Ctrl+Shift+Esc").unwrap();
        assert_eq!(k.len(), 3);
        assert_eq!(k[2].vk, 0x1b);
        assert_eq!(key("f13").unwrap().evdev, 183);
        assert_eq!(key("f1").unwrap().evdev, 59);
        assert_eq!(key("a").unwrap().evdev, 30);
        assert!(parse_hotkey("ctrl+nope").is_err());
    }
}
