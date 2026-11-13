//! Bragi keyboards.
//!
//! In software mode the keyboard keeps sending normal HID reports to the OS
//! and additionally reports every key on its vendor event interface as
//! `?? 02 <bitmap>` (bit n = key index n, Corsair's Bragi key map: 4 = A,
//! 102 mute, 113 brightness, 114 lock, 122 Fn, 127 MR, 128 profile,
//! 131..=136 G1..G6). The iCUE control dial sends `?? 05 ?? ?? <01|ff>`.
//!
//! Keyboards of the "assignment" family (K55 PRO/XT, K57, K60, K70 TKL,
//! K95 PLATINUM XT, K100) take a key-assignment table on handle 1 /
//! resource 0x02, one byte per key index: 1 = native function, 0 = report to
//! software only. opendevicecontrol sets the special keys (G-keys, MR, profile,
//! brightness, lock) to 0 so they no longer trigger on-board actions.
//!
//! LED index = key index, so per-key effects run in key-code order rather
//! than physical position (no physical layout tables yet).
//!
//! Sources: OpenLinkHub `devices/k55pro`, `k55proXT`, `k57rgbWU`,
//! `k60rgbpro`, `k65rm`, `k70pro`, `k70rgbtklcs`, `k70core`, `k70coretkl`,
//! `k95platinumXT`, `k100`, `k100airWU`; OpenRGB `CorsairPeripheralV2Devices.cpp`,
//! `CorsairK55RGBPROXTController.cpp`; ckb-next `led_bragi.c`, `keymap.c`.

use super::models::{Lighting, Model};
use crate::model::DeviceKind;

#[derive(Debug, Clone, Copy)]
pub struct SpecialKey {
    pub index: u16,
    pub id: &'static str,
    pub name: &'static str,
    pub assignable: bool,
}

const fn k(index: u16, id: &'static str, name: &'static str, assignable: bool) -> SpecialKey {
    SpecialKey { index, id, name, assignable }
}

pub const KEY_BRIGHTNESS: &str = "brightness";
pub const KEY_WINLOCK: &str = "win_lock";
pub const KEY_DIAL_CW: &str = "dial_cw";
pub const KEY_DIAL_CCW: &str = "dial_ccw";

pub struct Keyboard {
    pub keys: &'static [SpecialKey],
    /// Length of the key-assignment table, if the keyboard takes one.
    pub assignment_len: Option<usize>,
    pub key_locks: bool,
    pub dial: bool,
    /// Lighting can be streamed while connected through a receiver (K57
    /// and K100 AIR only take uploaded hardware effects wirelessly).
    pub wireless_lighting: bool,
}

const G_KEYS: [SpecialKey; 6] = [
    k(131, "g1", "G1", true),
    k(132, "g2", "G2", true),
    k(133, "g3", "G3", true),
    k(134, "g4", "G4", true),
    k(135, "g5", "G5", true),
    k(136, "g6", "G6", true),
];
const BRIGHTNESS: SpecialKey = k(113, KEY_BRIGHTNESS, "Brightness", false);
const LOCK: SpecialKey = k(114, KEY_WINLOCK, "Win lock", false);
const MR: SpecialKey = k(127, "mr", "MR (macro record)", true);
const PROFILE: SpecialKey = k(128, "profile", "Profile", true);

const KEYS_G: &[SpecialKey] =
    &[G_KEYS[0], G_KEYS[1], G_KEYS[2], G_KEYS[3], G_KEYS[4], G_KEYS[5], MR, BRIGHTNESS, LOCK];
const KEYS_G_PROFILE: &[SpecialKey] =
    &[G_KEYS[0], G_KEYS[1], G_KEYS[2], G_KEYS[3], G_KEYS[4], G_KEYS[5], MR, PROFILE, BRIGHTNESS, LOCK];
const KEYS_PLAIN: &[SpecialKey] = &[BRIGHTNESS, LOCK];
const KEYS_PROFILE: &[SpecialKey] = &[PROFILE, BRIGHTNESS, LOCK];

const fn kb(keys: &'static [SpecialKey], assignment_len: Option<usize>, dial: bool) -> Keyboard {
    Keyboard { keys, assignment_len, key_locks: true, dial, wireless_lighting: true }
}

static K55_PRO: Keyboard = kb(KEYS_G, Some(137), false);
static K57: Keyboard = Keyboard { wireless_lighting: false, ..kb(KEYS_G, Some(137), false) };
static K60: Keyboard = kb(KEYS_PLAIN, Some(123), false);
static K65_MINI: Keyboard = kb(KEYS_PLAIN, None, false);
static K70_PRO: Keyboard = kb(KEYS_PROFILE, None, false);
static K70_TKL: Keyboard = kb(KEYS_PROFILE, Some(129), false);
static K70_CORE: Keyboard = kb(KEYS_PLAIN, None, true);
static K95_XT: Keyboard = kb(KEYS_G_PROFILE, Some(137), false);
static K100: Keyboard = kb(KEYS_G_PROFILE, Some(138), true);
static K100_AIR: Keyboard = Keyboard { wireless_lighting: false, ..kb(KEYS_G_PROFILE, None, false) };

const K55_PRO_ZONES: &[(&str, &str)] = &[
    ("indicator", "Lock indicator"),
    ("zone1", "Zone 1"),
    ("zone2", "Zone 2"),
    ("zone3", "Zone 3"),
    ("zone4", "Zone 4"),
    ("zone5", "Zone 5"),
];

const fn keyboard(
    pid: u16,
    name: &'static str,
    slug: &'static str,
    packet: usize,
    lighting: Lighting,
    led_count: usize,
    spec: &'static Keyboard,
) -> Model {
    Model {
        pid,
        name,
        slug,
        kind: DeviceKind::Keyboard,
        packet,
        lighting,
        led_count,
        leds: &[],
        buttons: &[],
        dpi: None,
        max_poll: 4,
        wireless: false,
        keyboard: Some(spec),
        headset: None,
    }
}

const fn wireless(mut m: Model) -> Model {
    m.wireless = true;
    m
}

const fn zones(mut m: Model, leds: &'static [(&'static str, &'static str)]) -> Model {
    m.leds = leds;
    m
}

const fn max_poll(mut m: Model, code: u8) -> Model {
    m.max_poll = code;
    m
}

use Lighting::{Planar, Triplets};

pub static KEYBOARDS: &[Model] = &[
    zones(keyboard(0x1ba4, "K55 RGB PRO", "k55-rgb-pro", 64, Planar, 6, &K55_PRO), K55_PRO_ZONES),
    keyboard(0x1ba1, "K55 RGB PRO XT", "k55-rgb-pro-xt", 64, Planar, 137, &K55_PRO),
    wireless(keyboard(0x1b6e, "K57 RGB Wireless", "k57-rgb-wireless", 64, Planar, 137, &K57)),
    keyboard(0x1ba0, "K60 RGB PRO", "k60-rgb-pro", 64, Planar, 123, &K60),
    keyboard(0x1bad, "K60 RGB PRO Low Profile", "k60-rgb-pro-lp", 64, Planar, 123, &K60),
    keyboard(0x1b8d, "K60 RGB PRO SE", "k60-rgb-pro-se", 64, Planar, 123, &K60),
    keyboard(0x1bc7, "K60 RGB PRO TKL", "k60-rgb-pro-tkl", 64, Planar, 123, &K60),
    keyboard(0x1bed, "K60 RGB PRO TKL", "k60-rgb-pro-tkl", 64, Planar, 123, &K60),
    max_poll(keyboard(0x1baf, "K65 RGB MINI", "k65-rgb-mini", 1024, Triplets, 123, &K65_MINI), 7),
    max_poll(keyboard(0x1bc3, "K65 RGB MINI", "k65-rgb-mini", 1024, Triplets, 123, &K65_MINI), 7),
    max_poll(keyboard(0x1bcf, "K65 RGB MINI", "k65-rgb-mini", 1024, Triplets, 123, &K65_MINI), 7),
    max_poll(keyboard(0x1bc4, "K70 RGB PRO", "k70-rgb-pro", 1024, Triplets, 142, &K70_PRO), 7),
    max_poll(keyboard(0x1bc6, "K70 RGB PRO", "k70-rgb-pro", 1024, Triplets, 142, &K70_PRO), 7),
    max_poll(keyboard(0x1bb3, "K70 RGB PRO", "k70-rgb-pro", 1024, Triplets, 142, &K70_PRO), 7),
    max_poll(keyboard(0x1bd4, "K70 RGB PRO", "k70-rgb-pro", 1024, Triplets, 142, &K70_PRO), 7),
    max_poll(keyboard(0x1b73, "K70 RGB TKL", "k70-rgb-tkl", 1024, Triplets, 129, &K70_TKL), 7),
    max_poll(keyboard(0x1bb9, "K70 RGB TKL Champion", "k70-rgb-tkl-cs", 1024, Triplets, 129, &K70_TKL), 7),
    keyboard(0x1bfd, "K70 CORE RGB", "k70-core-rgb", 64, Triplets, 123, &K70_CORE),
    keyboard(0x1bff, "K70 CORE RGB", "k70-core-rgb", 64, Triplets, 123, &K70_CORE),
    keyboard(0x2b0a, "K70 CORE RGB", "k70-core-rgb", 64, Triplets, 123, &K70_CORE),
    keyboard(0x2b01, "K70 CORE RGB TKL", "k70-core-rgb-tkl", 64, Triplets, 125, &K70_CORE),
    keyboard(0x1b89, "K95 RGB PLATINUM XT", "k95-rgb-platinum-xt", 64, Planar, 156, &K95_XT),
    max_poll(keyboard(0x1b7c, "K100 RGB", "k100-rgb", 1024, Triplets, 193, &K100), 7),
    max_poll(keyboard(0x1bc5, "K100 RGB", "k100-rgb", 1024, Triplets, 193, &K100), 7),
    max_poll(keyboard(0x1b7d, "K100 RGB", "k100-rgb", 1024, Triplets, 193, &K100), 7),
    wireless(keyboard(0x1bab, "K100 AIR Wireless", "k100-air-wireless", 1024, Triplets, 137, &K100_AIR)),
];

/// The key-assignment table: native (1) for every key except the special
/// keys opendevicecontrol handles, which report to software only (0). With
/// `restore`, everything native again.
pub fn assignment_table(spec: &Keyboard, restore: bool) -> Option<Vec<u8>> {
    let len = spec.assignment_len?;
    let mut t = vec![1u8; len];
    if !restore {
        for key in spec.keys {
            if let Some(b) = t.get_mut(key.index as usize) {
                *b = 0;
            }
        }
    }
    Some(t)
}

/// Is key `index` down in a `?? 02 <bitmap>` event packet?
pub fn key_down(packet: &[u8], index: u16) -> bool {
    let byte = 2 + index as usize / 8;
    packet.get(byte).is_some_and(|b| b & (1 << (index % 8)) != 0)
}

/// Direction of a control-dial event (`?? 05 …`): +1 clockwise, -1 counter.
pub fn dial_step(packet: &[u8]) -> Option<i8> {
    if packet.get(1) != Some(&0x05) {
        return None;
    }
    match packet.get(4) {
        Some(0x01) => Some(1),
        Some(0xff) => Some(-1),
        _ => None,
    }
}

/// Value of property 0x4A ("performance" key locks): bit 0 Win, 1 Alt+Tab,
/// 2 Alt+F4, 3 Shift+Tab; 0 while Win lock is off.
pub fn key_lock_mask(win: bool, alt_tab: bool, alt_f4: bool, shift_tab: bool) -> u32 {
    if !win {
        return 0;
    }
    1 | (alt_tab as u32) << 1 | (alt_f4 as u32) << 2 | (shift_tab as u32) << 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g_keys_in_bitmap() {
        // OpenLinkHub: G1..G6 = indices 131..136 = data[18] bits 3..7, data[19] bit 0.
        let mut p = [0u8; 64];
        p[1] = 0x02;
        p[18] = 0b0000_1000;
        assert!(key_down(&p, 131));
        assert!(!key_down(&p, 132));
        p[19] = 1;
        assert!(key_down(&p, 136));
        assert!(!key_down(&p[..10], 136));
    }

    #[test]
    fn dial() {
        assert_eq!(dial_step(&[0x00, 0x05, 0x60, 0x00, 0x01]), Some(1));
        assert_eq!(dial_step(&[0x00, 0x05, 0x60, 0x00, 0xff]), Some(-1));
        assert_eq!(dial_step(&[0x00, 0x02, 0x00, 0x00, 0x01]), None);
    }

    #[test]
    fn locks() {
        assert_eq!(key_lock_mask(false, true, true, true), 0);
        assert_eq!(key_lock_mask(true, true, true, false), 0b0111);
        assert_eq!(key_lock_mask(true, false, false, true), 0b1001);
    }

    #[test]
    fn assignment() {
        let t = assignment_table(&K100, false).unwrap();
        assert_eq!(t.len(), 138);
        assert_eq!(t[131], 0);
        assert_eq!(t[113], 0);
        assert_eq!(t[4], 1);
        assert!(assignment_table(&K100, true).unwrap().iter().all(|&b| b == 1));
        assert!(assignment_table(&K70_PRO, false).is_none());
    }

    #[test]
    fn table_sanity() {
        for m in KEYBOARDS {
            let spec = m.keyboard.unwrap();
            if let Some(n) = spec.assignment_len {
                assert!(spec.keys.iter().all(|k| (k.index as usize) < n), "{}", m.name);
            }
            assert!(m.led_count > 0 && m.led_count * 3 + 2 < 1021, "{}", m.name);
        }
    }
}
