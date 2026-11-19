//! Speedlink ACCUSOR wire format. See docs/reverse-engineering/speedlink-accusor.md.
//!
//! Commands are 8-byte feature reports (no report ID) on the `0xFF01`
//! collection: seven command bytes and a checksum. Key tables and macros
//! follow as 64-byte output reports on the same collection.

use crate::keymap::{KeyBinding, Macro, MacroAction, MouseAction};
#[cfg(test)]
use crate::keymap::MacroStep;

/// Profiles stored in the keyboard (P1..P3).
pub const PROFILES: u8 = 3;
/// Entries in a key table (4 bytes each).
pub const TABLE_ENTRIES: usize = 160;
pub const TABLE_BYTES: usize = TABLE_ENTRIES * 4;
/// Bytes per output report.
pub const REPORT: usize = 64;
/// Bytes per macro slot.
pub const MACRO_BYTES: usize = 128;
/// Macro slots the official driver uses (`6 × profile + M-key`, 6..=23);
/// lower slots hold factory macros. Our macro `n` is device slot `6 + n`.
pub const MACRO_SLOTS: u8 = 18;
pub const FIRST_MACRO_SLOT: u8 = 6;
/// Most press/release events one macro slot holds (2 bytes each after the
/// 2-byte header).
pub const MACRO_MAX_STEPS: usize = (MACRO_BYTES - 2) / 2;

/// Last byte of every command: `(sum of bytes 0..7) ^ 0xAA`.
pub fn checksum(b: &[u8; 7]) -> u8 {
    (b.iter().map(|&x| x as u32).sum::<u32>() as u8) ^ 0xAA
}

/// A command as the feature report hidapi sends (report ID 0 first).
pub fn command(bytes: &[u8]) -> [u8; 9] {
    let mut b = [0u8; 7];
    b[..bytes.len().min(7)].copy_from_slice(&bytes[..bytes.len().min(7)]);
    let mut out = [0u8; 9];
    out[1..8].copy_from_slice(&b);
    out[8] = checksum(&b);
    out
}

/// Turns on event reports (`04 ..` on the `0xFF00` collection); the official
/// driver sends it at start-up and before every upload. Without it the
/// keyboard reports nothing.
pub fn hello() -> [u8; 9] {
    command(&[0x02, 0x09, 0x70, 0x07])
}

/// Ask for the active profile; the reply (a feature read) has it in byte 1
/// after the echoed `81`.
pub fn query_profile() -> [u8; 9] {
    command(&[0x81])
}

pub fn select_profile(p: u8) -> [u8; 9] {
    command(&[0x01, p])
}

/// Lighting of profile `p`: level 0 off, 1 25 %, 2 50 %, 3 100 %, 4
/// breathing; colour 0 red, 1 blue, 2 purple.
pub fn lighting(p: u8, level: u8, colour: u8) -> [u8; 9] {
    command(&[0x07, p, 0, level, 0, colour, 0])
}

/// Start of a key-table upload for profile `p`: 10 output reports follow.
pub fn key_table(p: u8) -> [u8; 9] {
    command(&[0x0C, p, 0, (TABLE_BYTES / REPORT) as u8])
}

/// Start of a macro upload to device slot `slot`: 2 output reports follow.
pub fn macro_slot(slot: u8) -> [u8; 9] {
    command(&[0x10, 0, slot, (MACRO_BYTES / REPORT) as u8])
}

pub const LEVELS: [&str; 5] = ["Off", "25%", "50%", "100%", "Breathing"];
pub const COLOURS: [&str; 3] = ["Red", "Blue", "Purple"];

/// Something the keyboard reported (input report `04 b1 b2 b3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Profile(u8),
    Level(u8),
    Colour(u8),
    /// Windows key locked (Fn+Win).
    WinLock(bool),
    /// A key set to "start application" `n` (a [`KeyBinding::Host`]).
    HostKey(u8),
    /// `04 00 00 00`: whatever was pressed was let go.
    Idle,
    Other([u8; 3]),
}

pub fn parse_event(r: &[u8]) -> Option<Event> {
    if r.len() < 4 || r[0] != 0x04 {
        return None;
    }
    Some(match (r[1], r[2], r[3]) {
        (0x05, 0x03, p) => Event::Profile(p),
        (0x06, 0x03, l) => Event::Level(l),
        (0x08, 0x02, c) => Event::Colour(c),
        (0x13, 0x00, v) => Event::WinLock(v != 0),
        (0x00, 0x0A, n) if n > 0 => Event::HostKey(n),
        (0, 0, 0) => Event::Idle,
        (a, b, c) => Event::Other([a, b, c]),
    })
}

/// The official driver's key table (`FUN_00405b50` in Monitor.EXE), which
/// matches what the keyboard does out of the box.
#[rustfmt::skip]
pub const DEFAULT_TABLE: [[u8; 4]; 144] = [
    [0,0,0x1D,0],[0,0,0x1E,0],[2,0,1,0],[3,0,0xB6,0],[0,0,0xE6,0],[3,0,0xB5,0],[2,0,2,0],[2,0,4,0],
    [0,0,0x06,0],[0,0,0x20,0],[0x0F,0x80,0,0],[3,0,0xE9,0],[3,0,0xEA,0],[3,0,0x27,2],[0,0,0xE4,0],[3,0,0x2A,2],
    [0,0,0x09,0],[0x12,1,0,0],[0x12,1,4,0],[3,0,0x24,2],[0,0,0xE2,0],[3,0,0x25,2],[0x12,3,1,0],[3,0,0x21,2],
    [0,0,0x28,0],[0x12,1,1,0],[0x12,1,5,0],[0x13,3,0x1A,0x52],[0x13,3,0x04,0x50],[0,0,0xE1,0],[0x13,3,0x16,0x51],[0x13,3,0x07,0x4F],
    [0,0,0x29,0],[0,0,0x0E,0],[0,0,0x13,0],[0,0,0x65,0],[0,0,0x0A,0],[0,0,0x39,0],[0,0,0xE0,0],[3,0,0x8A,1],
    [0,0,0x2C,0],[0,0,0x21,0],[0,0,0x87,0],[0x12,1,0x0D,0],[3,0,0x23,2],[0,0,0x8B,0],[0,0,0x8A,0],[0,0,0x88,0],
    [0x13,3,0x50,0x04],[0x12,1,3,0],[0,0,0x12,0],[3,0,0x92,1],[0,0,0x64,0],[0,0,0x91,0],[0,0,0x90,0],[0,0,0xE7,0],
    [0,0,0x2B,0],[0,0,0x35,0],[0x12,1,6,0],[0,0,0x1C,0],[0,0,0x4D,0],[0,0,0x24,0],[0,0,0x33,0],[0x12,3,3,0],
    [0x13,3,0x51,0x16],[0,0,0x2D,0],[0x12,1,7,0],[0,0,0x18,0],[0,0,0x2F,0],[0,0,0x25,0],[0,0,0x49,0],[0,0,0x11,0],
    [0x13,3,0x4F,0x07],[0,0,0x2E,0],[0x12,1,8,0],[0,0,0x0C,0],[0,0,0x30,0],[0,0,0x26,0],[0,0,0x34,0],[0x12,0,0,0],
    [0x13,3,0x52,0x1A],[0,0,0x2A,0],[0x12,1,0x0A,0],[0x12,1,0x0E,0],[0,0,0x4A,0],[0,0,0x4C,0],[0,0,0x0B,0],[0,0,0x37,0],
    [0,0,0x08,0],[0,0,0x0F,0],[0x12,1,0x0B,0],[0x12,1,0x0C,0],[0x12,1,0x0F,0],[0x12,1,0x13,0],[0,0,0x32,0],[0,0,0x38,0],
    [0,0,0x14,0],[0,0,0x0D,0],[0x12,1,9,0],[0,0,0x89,0],[0,0,0x31,0],[0,0,0x27,0],[0x12,1,0x14,0],[0,0,0x36,0],
    [0,0,0x05,0],[0,0,0x10,0],[0,0,0,0],[0,0,0x53,0],[0,0,0x5F,0],[0,0,0x5C,0],[0,0,0x59,0],[0x12,3,4,0],
    [0,0,0x19,0],[0,0,0x22,0],[5,0,3,1],[0,0,0x54,0],[0,0,0x60,0],[0,0,0x5D,0],[0,0,0x5A,0],[0,0,0x62,0],
    [0,0,0x15,0],[0,0,0x23,0],[5,0,3,2],[0,0,0x55,0],[0,0,0x61,0],[0,0,0x5E,0],[0,0,0x5B,0],[0,0,0x63,0],
    [0,0,0x17,0],[0x12,1,2,0],[5,0,3,3],[0,0,0x56,0],[0,0,0x57,0],[0,0,0x85,0],[0,0,0x58,0],[0x12,3,5,0],
    [0,0,0x1B,0],[0,0,0x1F,0],[3,0,0x94,1],[3,0,0xCD,0],[3,0,0x83,1],[0,0,0xE5,0],[3,0,0xE2,0],[3,0,0xB7,0],
];

/// A full key table, the default one padded with disabled entries.
pub fn default_table() -> Vec<[u8; 4]> {
    let mut t = DEFAULT_TABLE.to_vec();
    t.resize(TABLE_ENTRIES, [0; 4]);
    t
}

/// Table entries for the M keys (M1..M6), from Monitor.EXE.
pub const M_KEYS: [usize; 6] = [10, 22, 63, 106, 111, 135];

/// Firmware functions a key can be set to: the exact entry, its name and a
/// short label. The entry is the [`KeyBinding::Function`] id (big-endian).
pub const FUNCTIONS: &[([u8; 4], &str, &str, &str)] = &[
    ([0x05, 0, 3, 1], "Profile 1", "P1", "Profiles"),
    ([0x05, 0, 3, 2], "Profile 2", "P2", "Profiles"),
    ([0x05, 0, 3, 3], "Profile 3", "P3", "Profiles"),
    ([0x12, 0, 0, 0], "Fn", "Fn", "Keyboard"),
    ([0x12, 1, 0x00, 0], "F1 (Fn: slower key response)", "F1", "Fn keys"),
    ([0x12, 1, 0x01, 0], "F2 (Fn: faster key response)", "F2", "Fn keys"),
    ([0x12, 1, 0x02, 0], "F3 (Fn: lower polling rate)", "F3", "Fn keys"),
    ([0x12, 1, 0x03, 0], "F4 (Fn: higher polling rate)", "F4", "Fn keys"),
    ([0x12, 1, 0x04, 0], "F5 (Fn: calculator)", "F5", "Fn keys"),
    ([0x12, 1, 0x05, 0], "F6 (Fn: refresh)", "F6", "Fn keys"),
    ([0x12, 1, 0x06, 0], "F7 (Fn: back)", "F7", "Fn keys"),
    ([0x12, 1, 0x07, 0], "F8 (Fn: forward)", "F8", "Fn keys"),
    ([0x12, 1, 0x08, 0], "F9 (Fn: low brightness)", "F9", "Fn keys"),
    ([0x12, 1, 0x09, 0], "F10 (Fn: medium brightness)", "F10", "Fn keys"),
    ([0x12, 1, 0x0A, 0], "F11 (Fn: high brightness)", "F11", "Fn keys"),
    ([0x12, 1, 0x0B, 0], "F12 (Fn: breathing)", "F12", "Fn keys"),
    ([0x12, 1, 0x0E, 0], "Print Screen (Fn: gaming mode)", "PrtSc", "Fn keys"),
    ([0x12, 1, 0x0C, 0], "Scroll Lock (Fn: lighting on/off)", "ScrLk", "Fn keys"),
    ([0x12, 1, 0x0F, 0], "Pause (Fn: next colour)", "Pause", "Fn keys"),
    ([0x12, 1, 0x13, 0], "Page Up", "PgUp", "Fn keys"),
    ([0x12, 1, 0x14, 0], "Page Down", "PgDn", "Fn keys"),
    ([0x12, 1, 0x0D, 0], "Left Windows (Fn: lock)", "Win", "Fn keys"),
    ([0x13, 3, 0x1A, 0x52], "W (Up when swapped)", "W", "Gaming mode"),
    ([0x13, 3, 0x04, 0x50], "A (Left when swapped)", "A", "Gaming mode"),
    ([0x13, 3, 0x16, 0x51], "S (Down when swapped)", "S", "Gaming mode"),
    ([0x13, 3, 0x07, 0x4F], "D (Right when swapped)", "D", "Gaming mode"),
    ([0x13, 3, 0x52, 0x1A], "Up (W when swapped)", "↑", "Gaming mode"),
    ([0x13, 3, 0x50, 0x04], "Left (A when swapped)", "←", "Gaming mode"),
    ([0x13, 3, 0x51, 0x16], "Down (S when swapped)", "↓", "Gaming mode"),
    ([0x13, 3, 0x4F, 0x07], "Right (D when swapped)", "→", "Gaming mode"),
    ([0x13, 0xF0, 0x32, 2], "Double click", "2×Click", "Mouse"),
    ([0x13, 0xF0, 0x32, 0], "Fire (repeated left click)", "Fire", "Mouse"),
    ([0x02, 0, 1, 0], "System power", "Power", "System"),
    ([0x02, 0, 2, 0], "System sleep", "Sleep", "System"),
    ([0x02, 0, 4, 0], "System wake", "Wake", "System"),
];

/// Highest "start application" number the firmware reports.
pub const HOST_KEYS: u16 = 15;

/// What a table entry does, for a keymap editor.
pub fn binding(e: [u8; 4]) -> KeyBinding {
    match e {
        [0, 0, 0, 0] => KeyBinding::None,
        [0, 0, u @ 0xE0..=0xE7, _] => KeyBinding::Key { modifiers: 1 << (u - 0xE0), usage: 0 },
        [0, 0, u, _] => KeyBinding::Key { modifiers: 0, usage: u },
        [3, 0, lo, hi] => KeyBinding::Media(u16::from_le_bytes([lo, hi])),
        [1, 0, b @ 0xF0..=0xF4, _] => KeyBinding::Mouse(MouseAction::Button(b - 0xF0 + 1)),
        [0x0F, 0x80, s, _] if (FIRST_MACRO_SLOT..FIRST_MACRO_SLOT + MACRO_SLOTS).contains(&s) => {
            KeyBinding::Macro { slot: s - FIRST_MACRO_SLOT, mode: crate::keymap::MacroMode::Times(1) }
        }
        [0x11, 0, 0x0A, n] if n >= 1 && (n as u16) <= HOST_KEYS => KeyBinding::Host(n as u16),
        e if FUNCTIONS.iter().any(|f| f.0 == e) => KeyBinding::Function(u32::from_be_bytes(e)),
        e => KeyBinding::Raw(e),
    }
}

/// The table entry for a binding.
pub fn entry(b: &KeyBinding) -> Result<[u8; 4], String> {
    Ok(match *b {
        KeyBinding::None => [0; 4],
        KeyBinding::Key { modifiers: 0, usage } if usage != 0 => [0, 0, usage, 0],
        KeyBinding::Key { modifiers, usage: 0 } if modifiers.count_ones() == 1 => [0, 0, 0xE0 + modifiers.trailing_zeros() as u8, 0],
        KeyBinding::Key { .. } => return Err("this keyboard can't send a key combination from one key; record it as a macro instead".into()),
        KeyBinding::Media(u) => {
            let [lo, hi] = u.to_le_bytes();
            [3, 0, lo, hi]
        }
        KeyBinding::Mouse(MouseAction::Button(n @ 1..=5)) => [1, 0, 0xF0 + n - 1, 0],
        KeyBinding::Mouse(_) => return Err("this keyboard only does mouse buttons, not movement or the wheel".into()),
        KeyBinding::Macro { slot, mode } => {
            if slot >= MACRO_SLOTS {
                return Err(format!("there is no macro {}", slot + 1));
            }
            if mode != crate::keymap::MacroMode::Times(1) {
                return Err("this keyboard plays a macro once per press".into());
            }
            [0x0F, 0x80, FIRST_MACRO_SLOT + slot, 0]
        }
        KeyBinding::Host(n) if (1..=HOST_KEYS).contains(&n) => [0x11, 0, 0x0A, n as u8],
        KeyBinding::Host(_) => return Err(format!("only OpenDeviceControl keys 1 to {HOST_KEYS} exist")),
        KeyBinding::Function(id) => {
            let e = id.to_be_bytes();
            if !FUNCTIONS.iter().any(|f| f.0 == e) {
                return Err(format!("unknown keyboard function {id:#010x}"));
            }
            e
        }
        KeyBinding::Layer(_) => return Err("this keyboard has no layers".into()),
        KeyBinding::Raw(e) => e,
    })
}

pub fn table_bytes(t: &[[u8; 4]]) -> Vec<u8> {
    let mut b: Vec<u8> = t.iter().flatten().copied().collect();
    b.resize(TABLE_BYTES, 0);
    b
}

/// One press or release in a macro slot. Two bytes: `(delay / 20) | 0x01`
/// with bit 7 set for a release, then the HID usage; delays of more than
/// 127 units take four bytes, `[0x01 | 0x80 on release, usage, delay / 50,
/// 0x02]`. Encoded exactly as the official driver does.
fn event(out: &mut Vec<u8>, usage: u8, release: bool, delay_ms: u16) {
    let rel = if release { 0x80 } else { 0 };
    let units = delay_ms / 20;
    if units <= 0x7F {
        out.extend([(units as u8 & 0x7F) | 0x01 | rel, usage]);
    } else {
        out.extend([0x01 | rel, usage, (delay_ms / 50).min(255) as u8, 0x02]);
    }
}

/// A macro as the keyboard stores it (128 bytes). Each event carries the
/// pause before it, so a step's pause goes on the next event.
pub fn macro_bytes(m: &Macro) -> Result<Vec<u8>, String> {
    let mut out = vec![0x00, 0x01];
    let mut pause = 0u16;
    for s in &m.steps {
        let (usage, release) = match s.action {
            MacroAction::KeyDown(u) => (u, false),
            MacroAction::KeyUp(u) => (u, true),
            MacroAction::ModifierDown(bit) if bit.count_ones() == 1 => (0xE0 + bit.trailing_zeros() as u8, false),
            MacroAction::ModifierUp(bit) if bit.count_ones() == 1 => (0xE0 + bit.trailing_zeros() as u8, true),
            _ => return Err("this keyboard's macros can only press and release keys".into()),
        };
        event(&mut out, usage, release, pause);
        pause = s.delay_ms;
    }
    if out.len() > MACRO_BYTES {
        return Err(format!("macro '{}' is too long for the keyboard ({} bytes, at most {MACRO_BYTES})", m.name, out.len()));
    }
    out.resize(MACRO_BYTES, 0);
    Ok(out)
}

/// A step list back from a slot (the inverse of [`macro_bytes`], for tests).
#[cfg(test)]
pub fn macro_steps(b: &[u8]) -> Vec<MacroStep> {
    let mut steps: Vec<MacroStep> = Vec::new();
    let mut i = 2;
    while i + 1 < b.len() && b[i] != 0 {
        let (head, usage) = (b[i], b[i + 1]);
        let release = head & 0x80 != 0;
        let (delay, len) = if i + 3 < b.len() && b[i + 3] == 0x02 && head & 0x7F == 0x01 && b[i + 2] != 0 {
            (b[i + 2] as u16 * 50, 4)
        } else {
            (((head & 0x7E) as u16) * 20, 2)
        };
        if let Some(prev) = steps.last_mut() {
            prev.delay_ms = delay;
        }
        let action = match (usage, release) {
            (0xE0..=0xE7, false) => MacroAction::ModifierDown(1 << (usage - 0xE0)),
            (0xE0..=0xE7, true) => MacroAction::ModifierUp(1 << (usage - 0xE0)),
            (u, false) => MacroAction::KeyDown(u),
            (u, true) => MacroAction::KeyUp(u),
        };
        steps.push(MacroStep { action, delay_ms: 0 });
        i += len;
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_match_the_vendor_and_the_hardware() {
        assert_eq!(hello()[1..], [0x02, 0x09, 0x70, 0x07, 0, 0, 0, 0x28]);
        assert_eq!(query_profile()[1..], [0x81, 0, 0, 0, 0, 0, 0, 0x2B]);
        assert_eq!(select_profile(3)[1..], [0x01, 3, 0, 0, 0, 0, 0, 0xAE]);
        assert_eq!(lighting(3, 3, 1)[1..], [0x07, 3, 0, 3, 0, 1, 0, 0xA4]);
        assert_eq!(key_table(3)[1..], [0x0C, 3, 0, 0x0A, 0, 0, 0, 0xB3]);
        assert_eq!(macro_slot(18)[1..], [0x10, 0, 0x12, 0x02, 0, 0, 0, 0x8E]);
        assert_eq!(command(&[0x11, 0, 0, 1])[1..], [0x11, 0, 0, 1, 0, 0, 0, 0xB8]);
    }

    #[test]
    fn known_positions() {
        assert_eq!(DEFAULT_TABLE[114], [5, 0, 3, 1]);
        assert_eq!(DEFAULT_TABLE[130], [5, 0, 3, 3]);
        assert_eq!(DEFAULT_TABLE[10], [0x0F, 0x80, 0, 0]);
        assert_eq!(DEFAULT_TABLE[139], [3, 0, 0xCD, 0]);
        assert_eq!(default_table().len(), TABLE_ENTRIES);
    }

    #[test]
    fn every_default_entry_round_trips() {
        for e in default_table() {
            assert_eq!(entry(&binding(e)).unwrap(), e, "{e:02x?}");
        }
    }

    #[test]
    fn macro_matches_the_hardware_test() {
        let m = Macro {
            name: "hi".into(),
            steps: vec![
                MacroStep { action: MacroAction::KeyDown(0x0B), delay_ms: 0 },
                MacroStep { action: MacroAction::KeyUp(0x0B), delay_ms: 0 },
                MacroStep { action: MacroAction::KeyDown(0x0C), delay_ms: 0 },
                MacroStep { action: MacroAction::KeyUp(0x0C), delay_ms: 0 },
            ],
        };
        let b = macro_bytes(&m).unwrap();
        assert_eq!(b[..10], [0x00, 0x01, 0x01, 0x0B, 0x81, 0x0B, 0x01, 0x0C, 0x81, 0x0C]);
        assert_eq!(b.len(), MACRO_BYTES);
        assert_eq!(b[10..], [0; MACRO_BYTES - 10]);
    }

    #[test]
    fn ctrl_c_like_the_vendor() {
        let m = Macro {
            name: "copy".into(),
            steps: vec![
                MacroStep { action: MacroAction::ModifierDown(crate::keymap::modifier::LCTRL), delay_ms: 0 },
                MacroStep { action: MacroAction::KeyDown(0x06), delay_ms: 0 },
                MacroStep { action: MacroAction::KeyUp(0x06), delay_ms: 0 },
                MacroStep { action: MacroAction::ModifierUp(crate::keymap::modifier::LCTRL), delay_ms: 0 },
            ],
        };
        assert_eq!(macro_bytes(&m).unwrap()[..10], [0x00, 0x01, 0x01, 0xE0, 0x01, 0x06, 0x81, 0x06, 0x81, 0xE0]);
        assert_eq!(macro_steps(&macro_bytes(&m).unwrap()), m.steps);
    }

    #[test]
    fn long_pauses_use_the_four_byte_form() {
        let mut out = Vec::new();
        event(&mut out, 0x04, false, 5000);
        assert_eq!(out, [0x01, 0x04, 100, 0x02]);
    }

    #[test]
    fn events() {
        assert_eq!(parse_event(&[4, 5, 3, 2]), Some(Event::Profile(2)));
        assert_eq!(parse_event(&[4, 6, 3, 4]), Some(Event::Level(4)));
        assert_eq!(parse_event(&[4, 0x13, 0, 1]), Some(Event::WinLock(true)));
        assert_eq!(parse_event(&[4, 0, 0x0A, 3]), Some(Event::HostKey(3)));
        assert_eq!(parse_event(&[4, 0, 0, 0]), Some(Event::Idle));
        assert_eq!(parse_event(&[3, 0xCD, 0]), None);
    }
}
