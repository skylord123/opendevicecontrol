//! The F99's key-assignment and macro storage, as its firmware reads them
//! (V14.2, static analysis; see the notes in `proto`).
//!
//! **Keymap layer**: one 512-byte flash sector, 128 entries of 4 bytes
//! `type b1 b2 b3` at LED index × 4. Layers: 0 base, 1 Fn, 2 Fn2, 3 tap
//! (tap/hold). Entry 127 must stay `00 00 5A A5`: at power-up the firmware
//! checks it in every layer and resets everything to factory otherwise.
//! An all-zero entry does nothing in the base layer and falls through to
//! the base layer in the Fn layers.
//!
//! | type | fields |
//! |---|---|
//! | 00 | keyboard: `00 mods usage2 usage` (HID modifier bits) |
//! | 01 | mouse: `01 code count 00` (1–5 buttons, 6/7 pan, 8/9 wheel, 10/11 X, 12/13 Y; count FF = while held) |
//! | 02 | consumer: `02 00 hi lo` |
//! | 03 | macro: `03 mode count index` (1 play count times, 2 toggle, 4 while held) |
//! | 04 | host key: `04 00 hi lo`, reported to the PC as input report `03 04 hi lo` (USB only) |
//! | 07 | system function `07 00 00 code` |
//! | 08 | lighting control `08 target dir zone` |
//! | 0D | Fn layer while held: `0D 00` Fn1, `0D 01` Fn2 |
//!
//! **Macros** (0xDC00, 4 KB = 8 pages of 512): a directory of `u16le
//! offset, u16le length` per macro index (offsets from the area start,
//! length = whole record), each record `n name[n] event[4]…`. Event `e0 e1
//! e2 e3`: e0 bit 7 = release, bits 6–4 kind (0 key usage e3, 1 modifier
//! bit number e3, 2 mouse buttons e3, 3 X, 4 Y, 5 wheel, 6 pan as i8 e3);
//! `e1:e2` big-endian delay after the event in ms (< 0x8000). A key bound to
//! a directory entry with no valid record plays garbage, so every entry is
//! always pointed at a record (unused ones at one shared empty record).

use crate::keymap::{FunctionInfo, KeyBinding, LayerInfo, Macro, MacroAction, MacroMode, MacroStep, MouseAction};

pub const SECTOR: usize = 512;
pub const LAYER_ENTRIES: usize = 128;
/// Entries the firmware scans (and the wireless read returns).
pub const SCANNED: usize = 126;
pub const MACRO_AREA: usize = 8 * SECTOR;
/// Macro indices the directory covers (all a key can name).
const DIRECTORY: usize = 256;
/// Macro slots offered in the editor.
pub const MACRO_SLOTS: u8 = 32;
pub const MACRO_NAME_MAX: usize = 16;
/// Delays are 15 bits.
const MAX_DELAY: u16 = 0x7fff;

pub fn layers() -> Vec<LayerInfo> {
    let l = |id: u8, name: &str, help: &str| LayerInfo { id, name: name.into(), help: help.into() };
    vec![
        l(0, "Base", "What each key does normally (Windows and media-first OS modes; the Mac modes use their own built-in layout)."),
        l(1, "Fn", "While Fn is held. Keys set to Nothing fall back to the base layer."),
        l(2, "Fn2", "A second Fn layer: give some key \"Layer → Fn2\" to reach it. Keys set to Nothing fall back to the base layer."),
        l(3, "Tap", "Dual-function keys: a short tap does what is set here, holding does the base-layer action. Set the hold time in Settings."),
    ]
}

/// Functions keys can be bound to: id = the 4 action bytes as a u32.
pub fn functions() -> Vec<FunctionInfo> {
    let f = |b: [u8; 4], name: &str, short: &str, group: &str| FunctionInfo {
        id: u32::from_be_bytes(b),
        name: name.into(),
        short: short.into(),
        group: group.into(),
    };
    vec![
        f([8, 0, 1, 0], "Next effect", "Fx+", "Lighting"),
        f([8, 0, 2, 0], "Previous effect", "Fx−", "Lighting"),
        f([8, 2, 0, 0], "Next colour", "Col", "Lighting"),
        f([8, 3, 1, 0], "Brightness up", "Bri+", "Lighting"),
        f([8, 3, 2, 0], "Brightness down", "Bri−", "Lighting"),
        f([8, 4, 1, 0], "Speed up", "Spd+", "Lighting"),
        f([8, 4, 2, 0], "Speed down", "Spd−", "Lighting"),
        f([7, 0, 0, 0x0b], "Lights on/off", "Lit", "Lighting"),
        f([7, 0, 0, 0x01], "Win key lock", "WinL", "Keyboard"),
        f([7, 0, 0, 0x18], "OS mode: Windows", "Win", "Keyboard"),
        f([7, 0, 0, 0x19], "OS mode: Windows, media keys first", "WinM", "Keyboard"),
        f([7, 0, 0, 0x1a], "OS mode: Mac", "Mac", "Keyboard"),
        f([7, 0, 0, 0x1b], "OS mode: Mac (alternate)", "Mac2", "Keyboard"),
        f([7, 0, 0, 0x0c], "Show battery level", "Bat", "Keyboard"),
        f([7, 0, 0, 0x05], "Bluetooth 1 (hold 3 s: pair)", "BT1", "Connection"),
        f([7, 0, 0, 0x06], "Bluetooth 2 (hold 3 s: pair)", "BT2", "Connection"),
        f([7, 0, 0, 0x07], "Bluetooth 3 (hold 3 s: pair)", "BT3", "Connection"),
        f([7, 0, 0, 0x08], "Pair 2.4 GHz receiver (hold 3 s)", "2.4G", "Connection"),
        f([7, 0, 0, 0x0a], "Power off radio (hold 8 s)", "Off", "Connection"),
        f([7, 0, 0, 0x04], "Factory reset (hold 3 s)", "Rst", "Keyboard"),
    ]
}

/// Decode one entry.
pub fn decode(e: [u8; 4]) -> KeyBinding {
    let [t, b1, b2, b3] = e;
    let id = u32::from_be_bytes(e);
    if functions().iter().any(|f| f.id == id) {
        return KeyBinding::Function(id);
    }
    match t {
        0 if e == [0; 4] => KeyBinding::None,
        0 if b2 == 0 => KeyBinding::Key { modifiers: b1, usage: b3 },
        1 if b3 == 0 => match (b1, b2) {
            (1..=5, _) => KeyBinding::Mouse(MouseAction::Button(b1)),
            (6, _) => KeyBinding::Mouse(MouseAction::Pan(1)),
            (7, _) => KeyBinding::Mouse(MouseAction::Pan(-1)),
            (8, _) => KeyBinding::Mouse(MouseAction::Wheel(1)),
            (9, _) => KeyBinding::Mouse(MouseAction::Wheel(-1)),
            (10, _) => KeyBinding::Mouse(MouseAction::MoveX(-1)),
            (11, _) => KeyBinding::Mouse(MouseAction::MoveX(1)),
            (12, _) => KeyBinding::Mouse(MouseAction::MoveY(-1)),
            (13, _) => KeyBinding::Mouse(MouseAction::MoveY(1)),
            _ => KeyBinding::Raw(e),
        },
        2 if b1 == 0 => KeyBinding::Media(u16::from_be_bytes([b2, b3])),
        3 => match b1 {
            1 => KeyBinding::Macro { slot: b3, mode: MacroMode::Times(b2.max(1)) },
            2 => KeyBinding::Macro { slot: b3, mode: MacroMode::Toggle },
            4 => KeyBinding::Macro { slot: b3, mode: MacroMode::WhileHeld },
            _ => KeyBinding::Raw(e),
        },
        4 if b1 == 0 && (b2, b3) != (0, 0) => KeyBinding::Host(u16::from_be_bytes([b2, b3])),
        0x0d if b2 == 0 && b3 == 0 => KeyBinding::Layer(if b1 == 0 { 1 } else { 2 }),
        _ => KeyBinding::Raw(e),
    }
}

/// Encode one binding (unknown ones as nothing).
pub fn encode(b: &KeyBinding) -> [u8; 4] {
    match b {
        KeyBinding::None => [0; 4],
        KeyBinding::Key { modifiers, usage } => [0, *modifiers, 0, *usage],
        KeyBinding::Media(u) => {
            let [hi, lo] = u.to_be_bytes();
            [2, 0, hi, lo]
        }
        KeyBinding::Mouse(m) => {
            // Buttons and pointer movement act while held; wheel and pan
            // step once per press.
            let (code, count) = match *m {
                MouseAction::Button(n) => (n.clamp(1, 5), 0xff),
                MouseAction::Pan(v) => (if v >= 0 { 6 } else { 7 }, 1),
                MouseAction::Wheel(v) => (if v >= 0 { 8 } else { 9 }, 1),
                MouseAction::MoveX(v) => (if v < 0 { 10 } else { 11 }, 0xff),
                MouseAction::MoveY(v) => (if v < 0 { 12 } else { 13 }, 0xff),
            };
            [1, code, count, 0]
        }
        KeyBinding::Macro { slot, mode } => match mode {
            MacroMode::Times(n) => [3, 1, (*n).max(1), *slot],
            MacroMode::Toggle => [3, 2, 0, *slot],
            MacroMode::WhileHeld => [3, 4, 0, *slot],
        },
        KeyBinding::Host(n) => {
            let [hi, lo] = n.to_be_bytes();
            [4, 0, hi, lo]
        }
        KeyBinding::Function(id) => id.to_be_bytes(),
        KeyBinding::Layer(l) => [0x0d, (*l >= 2) as u8, 0, 0],
        KeyBinding::Raw(e) => *e,
    }
}

/// A layer sector as the keyboard stores it, from the bindings by LED
/// index (`None` entries for LEDs without keys). Entry 127 = `00 00 5A A5`.
pub fn layer_sector(by_led: &[KeyBinding]) -> [u8; SECTOR] {
    let mut s = [0u8; SECTOR];
    for (i, b) in by_led.iter().take(SCANNED).enumerate() {
        s[i * 4..i * 4 + 4].copy_from_slice(&encode(b));
    }
    s[SECTOR - 4..].copy_from_slice(&[0, 0, 0x5a, 0xa5]);
    s
}

/// Bindings by LED index from a layer as read (504 or 512 bytes).
pub fn layer_bindings(data: &[u8]) -> Vec<KeyBinding> {
    data.as_chunks::<4>().0.iter().take(SCANNED).map(|c| decode(*c)).collect()
}

fn event(a: &MacroAction, delay: u16) -> [u8; 4] {
    let (release, kind, v) = match *a {
        MacroAction::KeyDown(u) => (false, 0, u),
        MacroAction::KeyUp(u) => (true, 0, u),
        MacroAction::ModifierDown(m) => (false, 1, m.trailing_zeros().min(7) as u8),
        MacroAction::ModifierUp(m) => (true, 1, m.trailing_zeros().min(7) as u8),
        MacroAction::MouseDown(b) => (false, 2, 1 << (b.clamp(1, 5) - 1)),
        MacroAction::MouseUp(b) => (true, 2, 1 << (b.clamp(1, 5) - 1)),
        MacroAction::MoveX(v) => (false, 3, v as u8),
        MacroAction::MoveY(v) => (false, 4, v as u8),
        MacroAction::Wheel(v) => (false, 5, v as u8),
        MacroAction::Pan(v) => (false, 6, v as u8),
    };
    let [d1, d2] = delay.min(MAX_DELAY).to_be_bytes();
    [(release as u8) << 7 | kind << 4, d1, d2, v]
}

fn parse_event(e: &[u8]) -> Option<MacroStep> {
    let release = e[0] & 0x80 != 0;
    let delay_ms = u16::from_be_bytes([e[1] & 0x7f, e[2]]);
    let v = e[3];
    let action = match (e[0] >> 4) & 7 {
        0 => {
            if release {
                MacroAction::KeyUp(v)
            } else {
                MacroAction::KeyDown(v)
            }
        }
        1 => {
            if release {
                MacroAction::ModifierUp(1 << (v & 7))
            } else {
                MacroAction::ModifierDown(1 << (v & 7))
            }
        }
        2 => {
            let b = (v.trailing_zeros() + 1).min(5) as u8;
            if release {
                MacroAction::MouseUp(b)
            } else {
                MacroAction::MouseDown(b)
            }
        }
        3 => MacroAction::MoveX(v as i8),
        4 => MacroAction::MoveY(v as i8),
        5 => MacroAction::Wheel(v as i8),
        6 => MacroAction::Pan(v as i8),
        _ => return None,
    };
    Some(MacroStep { action, delay_ms })
}

/// The whole macro area for `macros` (slot = index). Errors if it doesn't
/// fit in 4 KB.
pub fn macro_area(macros: &[Macro]) -> Result<Vec<u8>, String> {
    let mut area = vec![0u8; MACRO_AREA];
    let mut at = DIRECTORY * 4;
    // One shared empty record for every unused index.
    let empty = at;
    area[at] = 0;
    at += 1;
    for i in 0..DIRECTORY {
        let (off, len) = match macros.get(i).filter(|m| !m.steps.is_empty()) {
            Some(m) => {
                let name: Vec<u8> = m.name.bytes().filter(u8::is_ascii).take(MACRO_NAME_MAX).collect();
                let len = 1 + name.len() + 4 * m.steps.len();
                if at + len > MACRO_AREA {
                    return Err("the macros don't fit in the keyboard's 4 KB macro memory".into());
                }
                area[at] = name.len() as u8;
                area[at + 1..at + 1 + name.len()].copy_from_slice(&name);
                for (k, s) in m.steps.iter().enumerate() {
                    let p = at + 1 + name.len() + 4 * k;
                    area[p..p + 4].copy_from_slice(&event(&s.action, s.delay_ms));
                }
                let r = (at, len);
                at += len;
                r
            }
            None => (empty, 1),
        };
        area[i * 4..i * 4 + 4].copy_from_slice(&[(off & 0xff) as u8, (off >> 8) as u8, (len & 0xff) as u8, (len >> 8) as u8]);
    }
    Ok(area)
}

/// Macros (one per slot, empty ones included) from the macro area.
pub fn macros_from_area(area: &[u8]) -> Vec<Macro> {
    (0..MACRO_SLOTS as usize)
        .map(|i| {
            let d = &area[i * 4..i * 4 + 4];
            let (off, len) = (u16::from_le_bytes([d[0], d[1]]) as usize, u16::from_le_bytes([d[2], d[3]]) as usize);
            if len < 1 || off + len > area.len() {
                return Macro::default();
            }
            let rec = &area[off..off + len];
            let n = (rec[0] as usize).min(rec.len() - 1);
            let name = String::from_utf8_lossy(&rec[1..1 + n]).into_owned();
            let steps = rec[1 + n..].as_chunks::<4>().0.iter().filter_map(|e| parse_event(e)).collect();
            Macro { name, steps }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::modifier;

    #[test]
    fn bindings_round_trip() {
        let all = [
            KeyBinding::None,
            KeyBinding::Key { modifiers: modifier::LCTRL | modifier::LSHIFT, usage: 0x29 },
            KeyBinding::Media(0x00e9),
            KeyBinding::Mouse(MouseAction::Button(2)),
            KeyBinding::Mouse(MouseAction::Wheel(-1)),
            KeyBinding::Mouse(MouseAction::MoveY(1)),
            KeyBinding::Macro { slot: 3, mode: MacroMode::Times(2) },
            KeyBinding::Macro { slot: 0, mode: MacroMode::Toggle },
            KeyBinding::Macro { slot: 7, mode: MacroMode::WhileHeld },
            KeyBinding::Host(5),
            KeyBinding::Layer(1),
            KeyBinding::Layer(2),
            KeyBinding::Function(u32::from_be_bytes([7, 0, 0, 0x01])),
            KeyBinding::Raw([0x09, 1, 0, 0]),
        ];
        for b in all {
            assert_eq!(decode(encode(&b)), b, "{b:?}");
        }
        // KB.ini examples: Esc = 00 00 00 29; macro modes per the firmware.
        assert_eq!(encode(&KeyBinding::Key { modifiers: 0, usage: 0x29 }), [0, 0, 0, 0x29]);
        assert_eq!(encode(&KeyBinding::Macro { slot: 0, mode: MacroMode::Times(1) }), [3, 1, 1, 0]);
        assert_eq!(encode(&KeyBinding::Macro { slot: 0, mode: MacroMode::WhileHeld }), [3, 4, 0, 0]);
    }

    #[test]
    fn layer_sector_keeps_the_magic() {
        let s = layer_sector(&[KeyBinding::Key { modifiers: 0, usage: 0x29 }]);
        assert_eq!(&s[..4], &[0, 0, 0, 0x29]);
        assert_eq!(&s[SECTOR - 4..], &[0, 0, 0x5a, 0xa5]);
        // Bindings past the scanned range never overwrite it.
        let many = vec![KeyBinding::Media(1); 200];
        assert_eq!(&layer_sector(&many)[SECTOR - 4..], &[0, 0, 0x5a, 0xa5]);
        assert_eq!(layer_bindings(&s[..504]).len(), SCANNED);
    }

    #[test]
    fn macro_example_from_the_firmware() {
        // "A, wait 100 ms, B" as the firmware notes give it.
        let m = Macro {
            name: String::new(),
            steps: vec![
                MacroStep { action: MacroAction::KeyDown(4), delay_ms: 10 },
                MacroStep { action: MacroAction::KeyUp(4), delay_ms: 100 },
                MacroStep { action: MacroAction::KeyDown(5), delay_ms: 10 },
                MacroStep { action: MacroAction::KeyUp(5), delay_ms: 1 },
            ],
        };
        let area = macro_area(std::slice::from_ref(&m)).unwrap();
        let off = u16::from_le_bytes([area[0], area[1]]) as usize;
        assert_eq!(&area[2..4], &[17, 0]);
        assert_eq!(&area[off..off + 17], &[0, 0, 0, 0x0a, 4, 0x80, 0, 0x64, 4, 0, 0, 0x0a, 5, 0x80, 0, 1, 5]);
        // Every directory entry points at a valid record.
        for i in 0..DIRECTORY {
            let len = u16::from_le_bytes([area[i * 4 + 2], area[i * 4 + 3]]);
            assert!(len >= 1);
        }
        let back = macros_from_area(&area);
        assert_eq!(back.len(), MACRO_SLOTS as usize);
        assert_eq!(back[0], m);
        assert!(back[1].steps.is_empty());
    }

    #[test]
    fn macro_names_modifiers_and_overflow() {
        let m = Macro {
            name: "copy".into(),
            steps: vec![
                MacroStep { action: MacroAction::ModifierDown(modifier::LCTRL), delay_ms: 5 },
                MacroStep { action: MacroAction::KeyDown(6), delay_ms: 5 },
                MacroStep { action: MacroAction::KeyUp(6), delay_ms: 5 },
                MacroStep { action: MacroAction::ModifierUp(modifier::LCTRL), delay_ms: 0 },
                MacroStep { action: MacroAction::MouseDown(2), delay_ms: 0 },
                MacroStep { action: MacroAction::Wheel(-3), delay_ms: 0 },
            ],
        };
        let back = macros_from_area(&macro_area(std::slice::from_ref(&m)).unwrap());
        assert_eq!(back[0], m);
        let huge = vec![Macro { name: "x".into(), steps: vec![MacroStep { action: MacroAction::KeyDown(4), delay_ms: 0 }; 200] }; 10];
        assert!(macro_area(&huge).is_err());
    }
}
