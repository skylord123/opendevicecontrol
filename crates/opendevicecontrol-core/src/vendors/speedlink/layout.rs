//! Where the ACCUSOR's keys are and which key-table entry each one is.
//!
//! Keys are found by their entry in the default table (each is unique), so
//! a key here can't drift from the table. Positions follow the keyboard:
//! M1..M6 on the left, the profile, app and media keys along the top, then
//! a full-size US layout (the key-table matrix also has ISO and Japanese
//! positions that this board doesn't have). Units are key widths.

use super::proto::{DEFAULT_TABLE, M_KEYS};
use crate::model::LedShape;

enum Pos {
    /// The key whose default entry is this.
    Entry([u8; 4]),
    /// Plain key with this HID usage.
    Usage(u8),
    /// A fixed table index (the M keys, whose default entries aren't unique).
    Index(usize),
}

struct Key {
    pos: Pos,
    name: &'static str,
    label: &'static str,
    rect: [f32; 4],
}

const X: f32 = 1.5; // the main block starts right of the M keys

const fn k(pos: Pos, name: &'static str, label: &'static str, x: f32, y: f32, w: f32, h: f32) -> Key {
    Key { pos, name, label, rect: [x, y, w, h] }
}

use Pos::{Entry, Index, Usage};

#[rustfmt::skip]
const KEYS: &[Key] = &[
    // Macro keys
    k(Index(M_KEYS[0]), "M1", "M1", 0.0, 1.0, 1.0, 1.0),
    k(Index(M_KEYS[1]), "M2", "M2", 0.0, 2.25, 1.0, 1.0),
    k(Index(M_KEYS[2]), "M3", "M3", 0.0, 3.25, 1.0, 1.0),
    k(Index(M_KEYS[3]), "M4", "M4", 0.0, 4.25, 1.0, 1.0),
    k(Index(M_KEYS[4]), "M5", "M5", 0.0, 5.25, 1.0, 1.0),
    k(Index(M_KEYS[5]), "M6", "M6", 0.0, 6.25, 1.0, 1.0),
    // Top strip: profiles, app keys, media
    k(Entry([5, 0, 3, 1]), "P1", "P1", X + 1.0, 0.0, 1.2, 0.75),
    k(Entry([5, 0, 3, 2]), "P2", "P2", X + 2.25, 0.0, 1.2, 0.75),
    k(Entry([5, 0, 3, 3]), "P3", "P3", X + 3.5, 0.0, 1.2, 0.75),
    k(Entry([3, 0, 0x94, 1]), "My Computer", "PC", X + 4.75, 0.0, 1.2, 0.75),
    k(Entry([3, 0, 0x8A, 1]), "Mail", "Mail", X + 6.0, 0.0, 1.2, 0.75),
    k(Entry([3, 0, 0x23, 2]), "Browser home", "Home", X + 7.25, 0.0, 1.2, 0.75),
    k(Entry([3, 0, 0xCD, 0]), "Play/Pause", "▶‖", X + 11.5, 0.0, 1.2, 0.75),
    k(Entry([3, 0, 0xB6, 0]), "Previous track", "⏮", X + 12.75, 0.0, 1.2, 0.75),
    k(Entry([3, 0, 0xB5, 0]), "Next track", "⏭", X + 14.0, 0.0, 1.2, 0.75),
    k(Entry([3, 0, 0xEA, 0]), "Volume down", "Vol−", X + 15.25, 0.0, 1.2, 0.75),
    k(Entry([3, 0, 0xE9, 0]), "Volume up", "Vol+", X + 16.5, 0.0, 1.2, 0.75),
    k(Entry([3, 0, 0xE2, 0]), "Mute", "Mute", X + 17.75, 0.0, 1.2, 0.75),
    // Function row
    k(Usage(0x29), "Esc", "Esc", X, 1.0, 1.0, 1.0),
    k(Entry([0x12, 1, 0x00, 0]), "F1", "F1", X + 2.0, 1.0, 1.0, 1.0),
    k(Entry([0x12, 1, 0x01, 0]), "F2", "F2", X + 3.0, 1.0, 1.0, 1.0),
    k(Entry([0x12, 1, 0x02, 0]), "F3", "F3", X + 4.0, 1.0, 1.0, 1.0),
    k(Entry([0x12, 1, 0x03, 0]), "F4", "F4", X + 5.0, 1.0, 1.0, 1.0),
    k(Entry([0x12, 1, 0x04, 0]), "F5", "F5", X + 6.5, 1.0, 1.0, 1.0),
    k(Entry([0x12, 1, 0x05, 0]), "F6", "F6", X + 7.5, 1.0, 1.0, 1.0),
    k(Entry([0x12, 1, 0x06, 0]), "F7", "F7", X + 8.5, 1.0, 1.0, 1.0),
    k(Entry([0x12, 1, 0x07, 0]), "F8", "F8", X + 9.5, 1.0, 1.0, 1.0),
    k(Entry([0x12, 1, 0x08, 0]), "F9", "F9", X + 11.0, 1.0, 1.0, 1.0),
    k(Entry([0x12, 1, 0x09, 0]), "F10", "F10", X + 12.0, 1.0, 1.0, 1.0),
    k(Entry([0x12, 1, 0x0A, 0]), "F11", "F11", X + 13.0, 1.0, 1.0, 1.0),
    k(Entry([0x12, 1, 0x0B, 0]), "F12", "F12", X + 14.0, 1.0, 1.0, 1.0),
    k(Entry([0x12, 1, 0x0E, 0]), "Print Screen", "PrtSc", X + 15.25, 1.0, 1.0, 1.0),
    k(Entry([0x12, 1, 0x0C, 0]), "Scroll Lock", "ScrLk", X + 16.25, 1.0, 1.0, 1.0),
    k(Entry([0x12, 1, 0x0F, 0]), "Pause", "Pause", X + 17.25, 1.0, 1.0, 1.0),
    // Number row
    k(Usage(0x35), "`", "`", X, 2.25, 1.0, 1.0),
    k(Usage(0x1E), "1", "1", X + 1.0, 2.25, 1.0, 1.0),
    k(Usage(0x1F), "2", "2", X + 2.0, 2.25, 1.0, 1.0),
    k(Usage(0x20), "3", "3", X + 3.0, 2.25, 1.0, 1.0),
    k(Usage(0x21), "4", "4", X + 4.0, 2.25, 1.0, 1.0),
    k(Usage(0x22), "5", "5", X + 5.0, 2.25, 1.0, 1.0),
    k(Usage(0x23), "6", "6", X + 6.0, 2.25, 1.0, 1.0),
    k(Usage(0x24), "7", "7", X + 7.0, 2.25, 1.0, 1.0),
    k(Usage(0x25), "8", "8", X + 8.0, 2.25, 1.0, 1.0),
    k(Usage(0x26), "9", "9", X + 9.0, 2.25, 1.0, 1.0),
    k(Usage(0x27), "0", "0", X + 10.0, 2.25, 1.0, 1.0),
    k(Usage(0x2D), "-", "-", X + 11.0, 2.25, 1.0, 1.0),
    k(Usage(0x2E), "=", "=", X + 12.0, 2.25, 1.0, 1.0),
    k(Usage(0x2A), "Backspace", "Bksp", X + 13.0, 2.25, 2.0, 1.0),
    k(Usage(0x49), "Insert", "Ins", X + 15.25, 2.25, 1.0, 1.0),
    k(Usage(0x4A), "Home", "Home", X + 16.25, 2.25, 1.0, 1.0),
    k(Entry([0x12, 1, 0x13, 0]), "Page Up", "PgUp", X + 17.25, 2.25, 1.0, 1.0),
    k(Usage(0x53), "Num Lock", "Num", X + 18.5, 2.25, 1.0, 1.0),
    k(Usage(0x54), "Keypad /", "/", X + 19.5, 2.25, 1.0, 1.0),
    k(Usage(0x55), "Keypad *", "*", X + 20.5, 2.25, 1.0, 1.0),
    k(Usage(0x56), "Keypad -", "-", X + 21.5, 2.25, 1.0, 1.0),
    // Tab row
    k(Usage(0x2B), "Tab", "Tab", X, 3.25, 1.5, 1.0),
    k(Usage(0x14), "Q", "Q", X + 1.5, 3.25, 1.0, 1.0),
    k(Entry([0x13, 3, 0x1A, 0x52]), "W", "W", X + 2.5, 3.25, 1.0, 1.0),
    k(Usage(0x08), "E", "E", X + 3.5, 3.25, 1.0, 1.0),
    k(Usage(0x15), "R", "R", X + 4.5, 3.25, 1.0, 1.0),
    k(Usage(0x17), "T", "T", X + 5.5, 3.25, 1.0, 1.0),
    k(Usage(0x1C), "Y", "Y", X + 6.5, 3.25, 1.0, 1.0),
    k(Usage(0x18), "U", "U", X + 7.5, 3.25, 1.0, 1.0),
    k(Usage(0x0C), "I", "I", X + 8.5, 3.25, 1.0, 1.0),
    k(Usage(0x12), "O", "O", X + 9.5, 3.25, 1.0, 1.0),
    k(Usage(0x13), "P", "P", X + 10.5, 3.25, 1.0, 1.0),
    k(Usage(0x2F), "[", "[", X + 11.5, 3.25, 1.0, 1.0),
    k(Usage(0x30), "]", "]", X + 12.5, 3.25, 1.0, 1.0),
    k(Usage(0x31), "\\", "\\", X + 13.5, 3.25, 1.5, 1.0),
    k(Usage(0x4C), "Delete", "Del", X + 15.25, 3.25, 1.0, 1.0),
    k(Usage(0x4D), "End", "End", X + 16.25, 3.25, 1.0, 1.0),
    k(Entry([0x12, 1, 0x14, 0]), "Page Down", "PgDn", X + 17.25, 3.25, 1.0, 1.0),
    k(Usage(0x5F), "Keypad 7", "7", X + 18.5, 3.25, 1.0, 1.0),
    k(Usage(0x60), "Keypad 8", "8", X + 19.5, 3.25, 1.0, 1.0),
    k(Usage(0x61), "Keypad 9", "9", X + 20.5, 3.25, 1.0, 1.0),
    k(Usage(0x57), "Keypad +", "+", X + 21.5, 3.25, 1.0, 2.0),
    // Caps row
    k(Usage(0x39), "Caps Lock", "Caps", X, 4.25, 1.75, 1.0),
    k(Entry([0x13, 3, 0x04, 0x50]), "A", "A", X + 1.75, 4.25, 1.0, 1.0),
    k(Entry([0x13, 3, 0x16, 0x51]), "S", "S", X + 2.75, 4.25, 1.0, 1.0),
    k(Entry([0x13, 3, 0x07, 0x4F]), "D", "D", X + 3.75, 4.25, 1.0, 1.0),
    k(Usage(0x09), "F", "F", X + 4.75, 4.25, 1.0, 1.0),
    k(Usage(0x0A), "G", "G", X + 5.75, 4.25, 1.0, 1.0),
    k(Usage(0x0B), "H", "H", X + 6.75, 4.25, 1.0, 1.0),
    k(Usage(0x0D), "J", "J", X + 7.75, 4.25, 1.0, 1.0),
    k(Usage(0x0E), "K", "K", X + 8.75, 4.25, 1.0, 1.0),
    k(Usage(0x0F), "L", "L", X + 9.75, 4.25, 1.0, 1.0),
    k(Usage(0x33), ";", ";", X + 10.75, 4.25, 1.0, 1.0),
    k(Usage(0x34), "'", "'", X + 11.75, 4.25, 1.0, 1.0),
    k(Usage(0x28), "Enter", "Enter", X + 12.75, 4.25, 2.25, 1.0),
    k(Usage(0x5C), "Keypad 4", "4", X + 18.5, 4.25, 1.0, 1.0),
    k(Usage(0x5D), "Keypad 5", "5", X + 19.5, 4.25, 1.0, 1.0),
    k(Usage(0x5E), "Keypad 6", "6", X + 20.5, 4.25, 1.0, 1.0),
    // Shift row
    k(Usage(0xE1), "Left Shift", "Shift", X, 5.25, 2.25, 1.0),
    k(Usage(0x1D), "Z", "Z", X + 2.25, 5.25, 1.0, 1.0),
    k(Usage(0x1B), "X", "X", X + 3.25, 5.25, 1.0, 1.0),
    k(Usage(0x06), "C", "C", X + 4.25, 5.25, 1.0, 1.0),
    k(Usage(0x19), "V", "V", X + 5.25, 5.25, 1.0, 1.0),
    k(Usage(0x05), "B", "B", X + 6.25, 5.25, 1.0, 1.0),
    k(Usage(0x11), "N", "N", X + 7.25, 5.25, 1.0, 1.0),
    k(Usage(0x10), "M", "M", X + 8.25, 5.25, 1.0, 1.0),
    k(Usage(0x36), ",", ",", X + 9.25, 5.25, 1.0, 1.0),
    k(Usage(0x37), ".", ".", X + 10.25, 5.25, 1.0, 1.0),
    k(Usage(0x38), "/", "/", X + 11.25, 5.25, 1.0, 1.0),
    k(Usage(0xE5), "Right Shift", "Shift", X + 12.25, 5.25, 2.75, 1.0),
    k(Entry([0x13, 3, 0x52, 0x1A]), "Up", "↑", X + 16.25, 5.25, 1.0, 1.0),
    k(Usage(0x59), "Keypad 1", "1", X + 18.5, 5.25, 1.0, 1.0),
    k(Usage(0x5A), "Keypad 2", "2", X + 19.5, 5.25, 1.0, 1.0),
    k(Usage(0x5B), "Keypad 3", "3", X + 20.5, 5.25, 1.0, 1.0),
    k(Usage(0x58), "Keypad Enter", "Ent", X + 21.5, 5.25, 1.0, 2.0),
    // Bottom row
    k(Usage(0xE0), "Left Ctrl", "Ctrl", X, 6.25, 1.25, 1.0),
    k(Entry([0x12, 1, 0x0D, 0]), "Left Windows", "Win", X + 1.25, 6.25, 1.25, 1.0),
    k(Usage(0xE2), "Left Alt", "Alt", X + 2.5, 6.25, 1.25, 1.0),
    k(Usage(0x2C), "Space", "", X + 3.75, 6.25, 6.25, 1.0),
    k(Usage(0xE6), "Right Alt", "AltGr", X + 10.0, 6.25, 1.25, 1.0),
    k(Entry([0x12, 0, 0, 0]), "Fn", "Fn", X + 11.25, 6.25, 1.25, 1.0),
    k(Usage(0x65), "Menu", "Menu", X + 12.5, 6.25, 1.25, 1.0),
    k(Usage(0xE4), "Right Ctrl", "Ctrl", X + 13.75, 6.25, 1.25, 1.0),
    k(Entry([0x13, 3, 0x50, 0x04]), "Left", "←", X + 15.25, 6.25, 1.0, 1.0),
    k(Entry([0x13, 3, 0x51, 0x16]), "Down", "↓", X + 16.25, 6.25, 1.0, 1.0),
    k(Entry([0x13, 3, 0x4F, 0x07]), "Right", "→", X + 17.25, 6.25, 1.0, 1.0),
    k(Usage(0x62), "Keypad 0", "0", X + 18.5, 6.25, 2.0, 1.0),
    k(Usage(0x63), "Keypad .", ".", X + 20.5, 6.25, 1.0, 1.0),
];

fn index(p: &Pos) -> Option<usize> {
    match *p {
        Index(i) => Some(i),
        Entry(e) => DEFAULT_TABLE.iter().position(|&x| x == e),
        Usage(u) => DEFAULT_TABLE.iter().position(|&x| x == [0, 0, u, 0]),
    }
}

/// Key-table index of each key, in keymap order.
pub fn table_indices() -> Vec<usize> {
    KEYS.iter().map(|k| index(&k.pos).expect("every key is in the default table")).collect()
}

pub fn shapes() -> Vec<LedShape> {
    KEYS.iter()
        .enumerate()
        .map(|(led, k)| LedShape { led, name: k.name.into(), label: k.label.into(), rect: k.rect })
        .collect()
}

/// Keymap positions of the profile keys (P1..P3).
pub fn profile_keys() -> Vec<usize> {
    KEYS.iter().enumerate().filter(|(_, k)| matches!(k.pos, Entry([5, 0, 3, _]))).map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_in_the_table_once() {
        let idx = table_indices();
        let mut seen = std::collections::HashSet::new();
        for (i, k) in KEYS.iter().enumerate() {
            assert!(seen.insert(idx[i]), "{} shares table index {}", k.name, idx[i]);
        }
        assert_eq!(idx.len(), KEYS.len());
    }

    #[test]
    fn hardware_mapped_keys() {
        // From the key test on the real keyboard (see the RE notes).
        let at = |name: &str| table_indices()[KEYS.iter().position(|k| k.name == name).unwrap()];
        assert_eq!(at("F1"), 17);
        assert_eq!(at("F12"), 90);
        assert_eq!(at("Print Screen"), 83);
        assert_eq!(at("Scroll Lock"), 91);
        assert_eq!(at("Pause"), 92);
        assert_eq!(at("Page Up"), 93);
        assert_eq!(at("Page Down"), 102);
        assert_eq!(at("Left Windows"), 43);
        assert_eq!(at("My Computer"), 138);
        assert_eq!(at("M1"), 10);
        assert_eq!(at("P1"), 114);
    }
}
