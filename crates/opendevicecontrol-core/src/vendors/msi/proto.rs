//! MSI Mystic Light over USB: the LED controller on MSI motherboards
//! (`1462:7bxx`-`7exx`, or `0db0:0076` with the board's id in the serial).
//!
//! Everything is a HID feature report. Report 0x52 holds every zone's
//! built-in effect in fixed 10-byte records; its length tells the protocol
//! apart: 162 bytes (older boards) or 185 bytes (newer ones). Reading it
//! returns the effects running now; writing it applies them, and stores
//! them in flash only when its last byte (the save flag) is 1.
//!
//! A zone record: `effect r g b sb r2 g2 b2 flags pad`, where `sb` is
//! speed (bits 0-1, 0-2) | brightness (bits 2-6, 0-10) | JRGB sync (bit 7)
//! and `flags` bit 7 means "use the colour" (clear: the effect cycles
//! colours); bits 0-5 are sync flags (on-board, JRAINBOW1, JRAINBOW2,
//! JCORSAIR, JPIPE1, JPIPE2). On 185-byte boards the JRAINBOW records carry
//! an 11th byte, the LED count.
//!
//! Newer 185-byte boards also have a per-LED mode: a 0x52 report with effect
//! 0x25 switches it on, then 725-byte 0x53 reports carry 240 RGB triples.
//! The next ordinary 0x52 report goes back to the built-in effects.
//!
//! The 112-byte and 761-byte protocols are not handled (see the notes in
//! `docs/reverse-engineering/msi-mystic-light.md`). Reports must be sent at
//! exactly their length: a 184-byte 0x52 report is known to brick boards.

use crate::model::Rgb;

pub const VID: u16 = 0x1462;
/// The shared id of recent boards; the serial starts with the board's id.
pub const COMMON_VID: u16 = 0x0db0;
pub const COMMON_PID: u16 = 0x0076;

pub const REPORT_EFFECTS: u8 = 0x52;
pub const REPORT_PER_LED: u8 = 0x53;
/// Effect byte of the per-LED mode.
pub const PER_LED_EFFECT: u8 = 0x25;
/// Bytes of a 0x53 report: ID, 4 header bytes, 240 RGB triples.
pub const PER_LED_LEN: usize = 5 + 3 * PER_LED_LEDS;
pub const PER_LED_LEDS: usize = 240;

/// Board ids OpenRGB enables (those it lists as untested are left out).
pub const PIDS: &[u16] = &[
    0x1720, 0x7b12, 0x7b16, 0x7b17, 0x7b18, 0x7b50, 0x7b85, 0x7b92, 0x7b93, 0x7c02, 0x7c34, 0x7c35, 0x7c36, 0x7c37, 0x7c56, 0x7c59,
    0x7c60, 0x7c67, 0x7c71, 0x7c73, 0x7c75, 0x7c76, 0x7c77, 0x7c79, 0x7c80, 0x7c81, 0x7c82, 0x7c83, 0x7c84, 0x7c86, 0x7c87, 0x7c90,
    0x7c91, 0x7c92, 0x7c94, 0x7c95, 0x7c98, 0x7d03, 0x7d04, 0x7d06, 0x7d07, 0x7d08, 0x7d09, 0x7d13, 0x7d14, 0x7d15, 0x7d17, 0x7d18,
    0x7d19, 0x7d20, 0x7d25, 0x7d27, 0x7d28, 0x7d29, 0x7d30, 0x7d31, 0x7d32, 0x7d33, 0x7d36, 0x7d37, 0x7d38, 0x7d40, 0x7d41, 0x7d42,
    0x7d43, 0x7d46, 0x7d50, 0x7d51, 0x7d52, 0x7d53, 0x7d54, 0x7d59, 0x7d67, 0x7d69, 0x7d70, 0x7d73, 0x7d74, 0x7d75, 0x7d76, 0x7d77,
    0x7d78, 0x7d86, 0x7d88, 0x7d89, 0x7d90, 0x7d91, 0x7d93, 0x7d96, 0x7d97, 0x7d98, 0x7d99, 0x7e01, 0x7e03, 0x7e06, 0x7e07, 0x7e09,
    0x7e10, 0x7e12, 0x7e16, 0x7e20, 0x7e24, 0x7e26, 0x7e27, 0x7e28, 0x7e30, 0x7e32, 0x7e34, 0x7e49, 0x7e59, 0x7e66, 0x7e70, 0x7e76,
    0x7e80, 0x7e81, 0x7e86, 0xb926,
];

/// The two 0x52 layouts, named by their length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    V162,
    V185,
}

impl Variant {
    /// From the length a 0x52 read returned (some hosts count one more).
    pub fn from_len(n: usize) -> Option<Self> {
        match n {
            162 | 163 => Some(Variant::V162),
            185 | 186 => Some(Variant::V185),
            _ => None,
        }
    }

    pub fn report_len(self) -> usize {
        match self {
            Variant::V162 => 162,
            Variant::V185 => 185,
        }
    }

    /// Effects up to this byte play in one zone only; later ones are
    /// "multi zone" effects that the on-board zone leads.
    fn last_single(self) -> u8 {
        match self {
            Variant::V162 => 0x04,
            Variant::V185 => 0x05,
        }
    }
}

/// The zones a board may have, in the order they are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Jrgb1,
    Jrgb2,
    Jrainbow1,
    Jrainbow2,
    Jrainbow3,
    Jcorsair,
    Jpipe1,
    Jpipe2,
    Onboard,
}

use Zone::*;

pub const ZONES: [Zone; 9] = [Jrgb1, Jrgb2, Jrainbow1, Jrainbow2, Jrainbow3, Jcorsair, Jpipe1, Jpipe2, Onboard];

impl Zone {
    pub fn key(self) -> &'static str {
        match self {
            Jrgb1 => "jrgb1",
            Jrgb2 => "jrgb2",
            Jrainbow1 => "jrainbow1",
            Jrainbow2 => "jrainbow2",
            Jrainbow3 => "jrainbow3",
            Jcorsair => "jcorsair",
            Jpipe1 => "jpipe1",
            Jpipe2 => "jpipe2",
            Onboard => "onboard",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Jrgb1 => "JRGB 1",
            Jrgb2 => "JRGB 2",
            Jrainbow1 => "JRAINBOW 1",
            Jrainbow2 => "JRAINBOW 2",
            Jrainbow3 => "JRAINBOW 3",
            Jcorsair => "JCORSAIR",
            Jpipe1 => "Light pipe 1",
            Jpipe2 => "Light pipe 2",
            Onboard => "On-board LEDs",
        }
    }

    pub fn from_key(k: &str) -> Option<Zone> {
        ZONES.into_iter().find(|z| z.key() == k)
    }

    /// Addressable headers, whose LED count the user sets.
    pub fn addressable(self) -> bool {
        matches!(self, Jrainbow1 | Jrainbow2 | Jrainbow3 | Jcorsair)
    }
}

/// How OpenDeviceControl lighting reaches the LEDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direct {
    /// Not known to work on this board: built-in effects only.
    None,
    /// Static colours written as 0x52 reports: one colour per zone, one per
    /// on-board LED.
    Zone,
    /// 0x53 per-LED reports.
    PerLed,
}

/// What a board has, from OpenRGB's per-board table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Board {
    pub variant: Variant,
    pub pid: u16,
    pub name: Option<&'static str>,
    pub zones: &'static [Zone],
    /// On-board LEDs (each has its own record after the on-board one).
    pub onboard: usize,
    /// Per-LED mode only: LEDs of the light pipes, JRGB headers.
    pub pipe1: usize,
    pub pipe2: usize,
    pub jrgbs: usize,
    pub direct: Direct,
    /// Least firmware version (APROM byte) the board's entry needs.
    pub min_fw: u8,
}

const R1W12: &[Zone] = &[Jrgb1, Jrainbow1, Jrainbow2];
const R1W12O: &[Zone] = &[Jrgb1, Jrainbow1, Jrainbow2, Onboard];
const R12W12: &[Zone] = &[Jrgb1, Jrgb2, Jrainbow1, Jrainbow2];
const R12W12O: &[Zone] = &[Jrgb1, Jrgb2, Jrainbow1, Jrainbow2, Onboard];
const R12W12P1: &[Zone] = &[Jrgb1, Jrgb2, Jrainbow1, Jrainbow2, Jpipe1];
const R12W12P1O: &[Zone] = &[Jrgb1, Jrgb2, Jrainbow1, Jrainbow2, Jpipe1, Onboard];
const R1W12C: &[Zone] = &[Jrgb1, Jrainbow1, Jrainbow2, Jcorsair];
const R1W12CP1: &[Zone] = &[Jrgb1, Jrainbow1, Jrainbow2, Jcorsair, Jpipe1];
const R1W12CP1O: &[Zone] = &[Jrgb1, Jrainbow1, Jrainbow2, Jcorsair, Jpipe1, Onboard];
const R1W12CP12: &[Zone] = &[Jrgb1, Jrainbow1, Jrainbow2, Jcorsair, Jpipe1, Jpipe2];
const R1W123: &[Zone] = &[Jrgb1, Jrainbow1, Jrainbow2, Jrainbow3];
const R1W123P1: &[Zone] = &[Jrgb1, Jrainbow1, Jrainbow2, Jrainbow3, Jpipe1];
const R1W123P1O: &[Zone] = &[Jrgb1, Jrainbow1, Jrainbow2, Jrainbow3, Jpipe1, Onboard];
const R1W123P12: &[Zone] = &[Jrgb1, Jrainbow1, Jrainbow2, Jrainbow3, Jpipe1, Jpipe2];
const R1W123P12O: &[Zone] = &[Jrgb1, Jrainbow1, Jrainbow2, Jrainbow3, Jpipe1, Jpipe2, Onboard];
const R1W1: &[Zone] = &[Jrgb1, Jrainbow1];
const R1: &[Zone] = &[Jrgb1];
const W1: &[Zone] = &[Jrainbow1];
const W12: &[Zone] = &[Jrainbow1, Jrainbow2];
const W123: &[Zone] = &[Jrainbow1, Jrainbow2, Jrainbow3];
const ALL_185: &[Zone] = &[Jrgb1, Jrgb2, Jrainbow1, Jrainbow2, Jcorsair, Jpipe1, Jpipe2, Onboard];
const R12W1CO: &[Zone] = &[Jrgb1, Jrgb2, Jrainbow1, Jcorsair, Onboard];
const R12W1O: &[Zone] = &[Jrgb1, Jrgb2, Jrainbow1, Onboard];
const W1O: &[Zone] = &[Jrainbow1, Onboard];

use Direct::{None as N, PerLed as P, Zone as Z};

/// 185-byte boards: (id, on-board LEDs, pipe 1 LEDs, pipe 2 LEDs, JRGB
/// headers, zones, direct mode, least firmware, name).
type Row185 = (u16, usize, usize, usize, usize, &'static [Zone], Direct, u8, &'static str);

const BOARDS_185: &[Row185] = &[
    (0x7b93, 6, 1, 0, 1, R1W12CP1O, Z, 0, "MPG X570 GAMING PRO CARBON WIFI"),
    (0x7c34, 0, 1, 1, 1, R1W12CP12, Z, 0, "MEG X570 GODLIKE"),
    (0x7c35, 0, 1, 0, 1, R1W12CP1, Z, 0, "MEG X570 ACE"),
    (0x7c36, 6, 1, 0, 1, R1W12CP1O, Z, 0, "PRESTIGE X570 CREATION"),
    (0x7c37, 6, 0, 0, 2, R12W12O, Z, 0, "MPG X570 GAMING PLUS"),
    (0x7c56, 6, 0, 0, 1, R1W12O, P, 6, "MPG B550 GAMING PLUS"),
    (0x7c59, 0, 8, 0, 1, R1W12CP1, N, 0, "Creator TRX40"),
    (0x7c60, 6, 0, 0, 1, R1W12C, P, 0, "TRX40-A PRO"),
    (0x7c67, 6, 0, 0, 1, R1W1, P, 0, "MAG B365M MORTAR"),
    (0x7c71, 6, 6, 0, 1, R1W12CP1O, P, 0, "MEG Z490 ACE"),
    (0x7c73, 6, 4, 0, 1, R1W12CP1O, P, 0, "MPG Z490 GAMING CARBON WIFI"),
    (0x7c75, 6, 0, 0, 1, R1W12O, P, 0, "MPG Z490 GAMING PLUS"),
    (0x7c76, 6, 0, 0, 2, R12W12O, N, 0, "MPG Z490M GAMING EDGE WIFI"),
    (0x7c77, 0, 0, 0, 0, W12, P, 0, "MEG Z490I UNIFY"),
    (0x7c79, 6, 0, 0, 2, R12W12O, P, 0, "MPG Z490 GAMING EDGE WIFI"),
    (0x7c80, 6, 0, 0, 2, R12W12O, P, 0, "MAG Z490 TOMAHAWK"),
    (0x7c02, 6, 0, 0, 2, R12W12O, P, 0, "MAG B450 TOMAHAWK MAX"),
    (0x7c81, 6, 0, 0, 2, R12W12O, P, 0, "MAG B460 TOMAHAWK"),
    (0x7c82, 6, 0, 0, 1, R1W1, P, 0, "MAG B460M MORTAR WIFI"),
    (0x7c83, 6, 0, 0, 1, R1W1, N, 0, "B460M PRO-VDH WIFI"),
    (0x7c84, 6, 0, 0, 2, R12W12O, Z, 0, "MAG X570 TOMAHAWK WIFI"),
    (0x7c86, 6, 0, 0, 1, R1W1, N, 0, "MPG B460I GAMING EDGE"),
    (0x7c87, 6, 0, 0, 2, R12W12, P, 0, "B450M BAZOOKA MAX WIFI"),
    (0x7c90, 6, 4, 0, 1, R1W12CP1O, P, 0, "MPG B550 GAMING CARBON WIFI"),
    (0x7c91, 6, 0, 0, 2, R12W12O, P, 0, "MAG B550 TOMAHAWK"),
    (0x7c92, 6, 0, 0, 0, W1, P, 0, "MPG B550I GAMING EDGE WIFI"),
    (0x7c94, 6, 0, 0, 1, R1W12, P, 0, "MAG B550M MORTAR"),
    (0x7c95, 6, 0, 0, 2, R12W12O, P, 0, "B550M PRO-VDH WIFI"),
    (0x7c98, 6, 0, 0, 1, R1W1, P, 0, "Z490 PLUS"),
    (0x7d03, 0, 15, 18, 1, R1W12CP12, P, 0, "MPG Z590 GODLIKE"),
    (0x7d06, 4, 4, 0, 1, R1W12CP1O, P, 0, "MPG Z590 GAMING FORCE"),
    (0x7d07, 4, 5, 0, 2, R12W12P1O, P, 0, "MPG Z590 GAMING EDGE WIFI"),
    (0x7d08, 6, 0, 0, 2, R12W12O, P, 0, "MAG Z590 TOMAHAWK"),
    (0x7d09, 6, 0, 0, 2, R1W12, P, 0, "Z590-A PRO WIFI"),
    (0x7d13, 6, 0, 0, 1, R1W12C, P, 0, "MEG B550 UNIFY"),
    (0x7d14, 6, 0, 0, 1, R1W1, P, 0, "A520M PRO"),
    (0x7d15, 6, 0, 0, 2, R12W12O, P, 0, "MAG B560 TOMAHAWK WIFI"),
    (0x7d17, 6, 0, 0, 2, R1W12, P, 0, "MAG B560M MORTAR"),
    (0x7d18, 6, 0, 0, 2, R1W1, P, 0, "MAG B560M PRO-VDH"),
    (0x7d19, 6, 0, 0, 2, R1W1, P, 0, "MPG B560I GAMING EDGE WIFI"),
    (0x7d20, 6, 0, 0, 2, R1W1, P, 0, "MAG B560M PRO"),
    (0x7d25, 6, 0, 0, 2, R1W12, P, 0, "PRO Z690-A WIFI DDR4"),
    (0x7d27, 6, 0, 0, 2, R1W12C, P, 0, "MEG Z690 ACE"),
    (0x7d28, 6, 0, 0, 1, R1W12C, P, 0, "MEG Z690 UNIFY-X"),
    (0x7d29, 6, 0, 0, 0, W1, P, 0, "MEG Z690I UNIFY"),
    (0x7d30, 6, 6, 0, 2, R1W12CP1O, P, 0, "MPG Z690 CARBON WIFI"),
    (0x7d31, 4, 8, 0, 2, R1W123P1O, P, 0, "MPG Z690 EDGE WIFI DDR4"),
    (0x7d32, 1, 0, 0, 1, R1W123, P, 0, "MAG Z690 TOMAHAWK WIFI DDR4"),
    (0x7d33, 6, 0, 0, 2, R1W1, P, 0, "PRO Z790-VC WIFI"),
    (0x7d36, 6, 0, 0, 2, R1W1, P, 0, "PRO Z690-P DDR4"),
    (0x7d37, 6, 0, 0, 2, R1W12, P, 0, "PRO B760M-VC WIFI"),
    (0x7d38, 0, 0, 0, 1, R1W12C, P, 0, "MEG Z590 UNIFY-X"),
    (0x7d40, 0, 0, 0, 1, R1W1, P, 0, "MPG B760I EDGE WIFI DDR4"),
    (0x7d41, 6, 0, 0, 2, R12W12, P, 0, "MAG B660M TOMAHAWK WIFI DDR4"),
    (0x7d42, 6, 0, 0, 2, R1W12, P, 0, "MAG B660 MORTAR WIFI DDR4"),
    (0x7d43, 0, 0, 0, 2, R1W12, P, 0, "PRO B660M-A WIFI DDR4"),
    (0x7d46, 0, 1, 1, 0, R1W12, P, 0, "PRO H610M-G DDR4"),
    (0x7d50, 6, 12, 0, 1, R1W12CP1O, P, 0, "MEG X570S ACE MAX"),
    (0x7d51, 6, 0, 0, 2, R1W12C, P, 0, "MEG X570S UNIFY-X MAX"),
    (0x7d52, 6, 14, 0, 1, R1W12CP1O, P, 0, "MPG X570S CARBON EK X"),
    (0x7d53, 6, 0, 0, 2, R12W12O, P, 0, "MPG X570S EDGE MAX WIFI"),
    (0x7d54, 6, 0, 0, 2, R12W12O, P, 0, "MAG X570S TOMAHAWK MAX WIFI"),
    (0x7d59, 0, 0, 0, 2, R1W12, P, 0, "PRO B660-A DDR4"),
    (0x7d67, 0, 0, 0, 1, R1W12, P, 0, "PRO X670-P WIFI"),
    (0x7d69, 9, 2, 4, 1, R1W123P12O, P, 0, "MEG X670E ACE"),
    (0x7d70, 0, 6, 0, 1, R1W12CP1, P, 0, "MPG X670E CARBON WIFI"),
    (0x7d73, 1, 0, 0, 0, W1, P, 0, "MPG B650I EDGE WIFI"),
    (0x7d74, 0, 6, 0, 1, R1W123P1, P, 0, "MPG B650 CARBON WIFI"),
    (0x7d75, 0, 0, 0, 2, R12W12, P, 0, "MAG B650 TOMAHAWK WIFI"),
    (0x7d76, 0, 0, 0, 2, R12W12, P, 0, "MAG B650M MORTAR WIFI"),
    (0x7d77, 0, 0, 0, 2, R12W12, P, 0, "PRO B650M-A WIFI"),
    (0x7d78, 0, 0, 0, 2, R12W12, P, 0, "PRO B650-P WIFI"),
    (0x7d86, 0, 18, 4, 1, R1W123P12, P, 0, "MEG Z790 ACE"),
    (0x7d88, 0, 0, 0, 1, R1, P, 0, "MPG Z790-S WIFI"),
    (0x7d89, 0, 6, 0, 1, R1W123P1, P, 0, "MPG Z790 CARBON WIFI"),
    (0x7d90, 0, 0, 0, 1, R1W12, P, 0, "B760M BOMBER DDR4"),
    (0x7d91, 1, 0, 0, 1, R1W123, P, 0, "MAG Z790 TOMAHAWK WIFI"),
    (0x7d93, 6, 0, 0, 1, R1W12O, P, 0, "Z790 GAMING PRO WIFI"),
    (0x7d96, 0, 0, 0, 2, R12W12, P, 0, "MAG B760 TOMAHAWK WIFI"),
    (0x7d97, 6, 0, 0, 2, R1W12, P, 0, "MAG B660 MORTAR MAX WIFI DDR4"),
    (0x7d98, 0, 0, 0, 1, R1W12, P, 0, "PRO B760-P WIFI DDR4"),
    (0x7d99, 6, 0, 0, 1, R1W12, P, 0, "PRO B760M-A WIFI DDR4"),
    (0x7e01, 0, 0, 0, 1, R1W12, P, 0, "MAG B760M MORTAR MAX"),
    (0x7e03, 6, 0, 0, 0, W1, P, 0, "MPG Z790I EDGE WIFI"),
    (0x7e06, 0, 0, 0, 2, R1W12, P, 0, "PRO Z790-P WIFI DDR4"),
    (0x7e07, 0, 0, 0, 2, R1W123, P, 0, "PRO Z790-A WIFI DDR4"),
    (0x7e09, 0, 0, 0, 0, W123, P, 0, "B650M PROJECT ZERO"),
    (0x7e10, 0, 6, 0, 2, R12W12P1, P, 0, "MPG B650 EDGE WIFI"),
    (0x7e12, 0, 0, 0, 2, R12W12, P, 0, "MAG X670E TOMAHAWK WIFI"),
    (COMMON_PID, 0, 0, 0, 2, R12W12, P, 0, "MAG X670E TOMAHAWK WIFI"),
    (0x7e16, 0, 0, 0, 2, R12W12, P, 0, "X670E GAMING PLUS WIFI"),
    (0x7e24, 0, 0, 0, 2, R12W12, P, 0, "B650M GAMING PLUS WIFI"),
    (0x7e26, 0, 0, 0, 2, R12W12, P, 0, "B650 GAMING PLUS WIFI"),
    (0x7e27, 0, 0, 0, 2, R12W12, P, 0, "PRO B650M-P"),
    (0x7e28, 6, 0, 0, 2, ALL_185, Z, 0, "PRO A620M-B"),
    (0x7e30, 0, 0, 0, 2, R12W12, P, 0, "B650M GAMING WIFI"),
];

/// 162-byte boards: (id, on-board LEDs, zones, name).
const BOARDS_162: &[(u16, usize, &[Zone], &str)] = &[
    (0x1720, 10, R12W1CO, "MPG Z390 GAMING EDGE AC"),
    (0x7b12, 10, R12W1CO, "MEG Z390 ACE"),
    (0x7b17, 10, R12W1CO, "MPG Z390 GAMING PRO CARBON"),
    (0x7b18, 6, R12W1O, "MAG Z390 TOMAHAWK"),
    (0x7b50, 6, R12W12O, "MPG Z390M GAMING EDGE AC"),
    (0x7b85, 7, R12W1CO, "B450 GAMING PRO CARBON"),
    (0x7b92, 10, R12W1CO, "MEG X399 CREATION"),
    (0xb926, 3, W1O, "MPG B460 TRIDENT AS"),
];

/// The board behind `pid`. Boards missing from the tables get every zone
/// and no OpenDeviceControl lighting on 185-byte boards, as in OpenRGB.
pub fn board(variant: Variant, pid: u16) -> Board {
    let base = Board { variant, pid, name: None, zones: ALL_185, onboard: 6, pipe1: 1, pipe2: 1, jrgbs: 2, direct: Direct::None, min_fw: 0 };
    match variant {
        Variant::V185 => BOARDS_185.iter().find(|r| r.0 == pid).map_or(base, |&(_, onboard, pipe1, pipe2, jrgbs, zones, direct, min_fw, name)| Board {
            name: Some(name),
            zones,
            onboard,
            pipe1,
            pipe2,
            jrgbs,
            direct,
            min_fw,
            ..base
        }),
        Variant::V162 => {
            let row = BOARDS_162.iter().find(|r| r.0 == pid);
            Board {
                name: row.map(|r| r.3),
                zones: row.map_or(R12W1CO, |r| r.2),
                onboard: row.map_or(10, |r| r.1),
                pipe1: 0,
                pipe2: 0,
                jrgbs: 0,
                direct: Direct::Zone,
                ..base
            }
        }
    }
}

/// `0db0:0076` boards: the board's id from the first four hex digits of
/// the serial.
pub fn virtual_pid(serial: &str) -> Option<u16> {
    u16::from_str_radix(serial.get(..4)?, 16).ok()
}

impl Board {
    pub fn has(&self, z: Zone) -> bool {
        self.zones.contains(&z)
    }

    /// 185-byte boards from 0x7D03 on moved JRGB2 into on-board LED 6's
    /// record, and that LED into the JCORSAIR outer record.
    fn mixed(&self) -> bool {
        self.variant == Variant::V185 && self.pid >= 0x7d03
    }

    /// Offset of a zone's record in the 0x52 report.
    pub fn offset(&self, z: Zone) -> Option<usize> {
        match self.variant {
            Variant::V185 => Some(match z {
                Jrgb1 => 1,
                Jpipe1 => 11,
                Jpipe2 => 21,
                Jrainbow1 => 31,
                Jrainbow2 => 42,
                Jrainbow3 | Jcorsair => 53,
                Onboard => 74,
                Jrgb2 if self.mixed() => 134,
                Jrgb2 => 174,
            }),
            Variant::V162 => match z {
                Jrgb1 => Some(1),
                Jrainbow1 => Some(11),
                Jcorsair => Some(21),
                Onboard => Some(41),
                // Shares on-board LED 10's record.
                Jrainbow2 => Some(141),
                Jrgb2 => Some(151),
                _ => None,
            },
        }
    }

    /// Records of the on-board LEDs (after the on-board zone's own one).
    fn onboard_leds(&self) -> Vec<usize> {
        let (max, first) = match self.variant {
            Variant::V185 => (9, 84),
            Variant::V162 => (if self.has(Jrainbow2) { 9 } else { 10 }, 51),
        };
        (0..self.onboard.min(max)).map(|i| if self.mixed() && i == 5 { 64 } else { first + 10 * i }).collect()
    }

    /// On-board LEDs with their own record (lit one by one in zone-based
    /// direct mode).
    pub fn onboard_led_count(&self) -> usize {
        self.onboard_leds().len()
    }

    pub fn save_offset(&self) -> usize {
        self.variant.report_len() - 1
    }

    /// LED count byte of a JRAINBOW record (185-byte boards).
    fn count_offset(&self, z: Zone) -> Option<usize> {
        (self.variant == Variant::V185 && matches!(z, Jrainbow1 | Jrainbow2 | Jrainbow3)).then(|| self.offset(z).unwrap() + 10)
    }

    /// How many LEDs a header drives, as set on the board.
    pub fn led_count(&self, report: &[u8], z: Zone) -> Option<u8> {
        report.get(self.count_offset(z)?).copied()
    }

    pub fn set_led_count(&self, report: &mut [u8], z: Zone, n: u8) {
        if let Some(b) = self.count_offset(z).and_then(|o| report.get_mut(o)) {
            *b = n;
        }
    }
}

/// How a built-in effect uses the colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colour {
    /// Always the chosen colour.
    Fixed,
    /// The chosen colour, or cycling colours.
    Optional,
    /// Its own colours.
    None,
}

/// A built-in effect: (effect byte, name, colour use, has a speed).
pub type Mode = (u8, &'static str, Colour, bool);

/// The effects OpenRGB offers (those known to work across boards).
const MODES_185: &[Mode] = &[
    (0x01, "Static", Colour::Fixed, false),
    (0x02, "Breathing", Colour::Fixed, true),
    (0x03, "Flashing", Colour::Optional, true),
    (0x04, "Double flashing", Colour::Optional, true),
    (0x05, "Lightning", Colour::Fixed, true),
    (0x07, "Meteor", Colour::Optional, true),
    (0x0f, "Color ring", Colour::None, true),
    (0x10, "Planetary", Colour::None, true),
    (0x11, "Double meteor", Colour::None, true),
    (0x12, "Energy", Colour::None, true),
    (0x13, "Blink", Colour::Optional, true),
    (0x14, "Clock", Colour::None, true),
    (0x15, "Color pulse", Colour::Optional, true),
    (0x16, "Color shift", Colour::None, true),
    (0x17, "Color wave", Colour::Optional, true),
    (0x18, "Marquee", Colour::Fixed, true),
    (0x1a, "Rainbow wave", Colour::None, true),
    (0x1b, "Visor", Colour::Optional, true),
    (0x1d, "Rainbow flashing", Colour::None, true),
    (0x23, "Color ring double flashing", Colour::None, true),
    (0x24, "Stack", Colour::Optional, true),
];

const MODES_162: &[Mode] = &[
    (0x01, "Static", Colour::Fixed, false),
    (0x02, "Breathing", Colour::Fixed, true),
    (0x03, "Flashing", Colour::Optional, true),
    (0x04, "Double flashing", Colour::Optional, true),
    (0x05, "Lightning", Colour::Fixed, true),
    (0x07, "Meteor", Colour::Optional, true),
    (0x08, "Stack", Colour::Optional, true),
    (0x0f, "Rainbow", Colour::Optional, true),
    (0x10, "Planetary", Colour::None, true),
    (0x11, "Double meteor", Colour::None, true),
    (0x12, "Energy", Colour::None, true),
    (0x13, "Blink", Colour::Optional, true),
    (0x14, "Clock", Colour::None, true),
    (0x15, "Color pulse", Colour::Optional, true),
    (0x16, "Color shift", Colour::None, true),
    (0x17, "Color wave", Colour::Optional, true),
    (0x18, "Marquee", Colour::Fixed, true),
    (0x1a, "Rainbow wave", Colour::None, true),
    (0x1b, "Visor", Colour::Optional, true),
    (0x1d, "Rainbow flashing", Colour::None, true),
    (0x1e, "Rainbow double flashing", Colour::None, true),
];

pub fn modes(v: Variant) -> &'static [Mode] {
    match v {
        Variant::V162 => MODES_162,
        Variant::V185 => MODES_185,
    }
}

pub fn mode(v: Variant, byte: u8) -> Option<&'static Mode> {
    modes(v).iter().find(|m| m.0 == byte)
}

pub const STATIC: u8 = 0x01;
/// Speed values: slow, medium, fast.
pub const MAX_SPEED: u8 = 2;
/// Brightness steps (10 % each).
pub const MAX_BRIGHTNESS: u8 = 10;

/// A zone's built-in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effect {
    pub mode: u8,
    pub color: Rgb,
    pub speed: u8,
    pub brightness: u8,
    /// Use `color` (false: the effect cycles colours).
    pub custom: bool,
}

impl Default for Effect {
    fn default() -> Self {
        Effect { mode: STATIC, color: Rgb::new(255, 255, 255), speed: 1, brightness: MAX_BRIGHTNESS, custom: true }
    }
}

const SYNC_JRGB: u8 = 0x80;
const COLOUR_FLAG: u8 = 0x80;
const SYNC_ONBOARD: u8 = 0x01;
const SYNC_PIPES: u8 = 0x30;

/// A zone's effect from its record.
pub fn parse_effect(rec: &[u8]) -> Option<Effect> {
    let r = rec.get(..10)?;
    Some(Effect {
        mode: r[0],
        color: Rgb::new(r[1], r[2], r[3]),
        speed: (r[4] & 0x03).min(MAX_SPEED),
        brightness: ((r[4] >> 2) & 0x1f).min(MAX_BRIGHTNESS),
        custom: r[8] & COLOUR_FLAG != 0,
    })
}

/// Effect, both colours, speed and brightness (JRGB sync cleared) and the
/// colour flag; other flag bits are kept.
fn write_record(rec: &mut [u8], e: &Effect) {
    let c = [e.color.r, e.color.g, e.color.b];
    rec[0] = e.mode;
    rec[1..4].copy_from_slice(&c);
    rec[4] = e.speed.min(MAX_SPEED) | (e.brightness.min(MAX_BRIGHTNESS) << 2);
    rec[5..8].copy_from_slice(&c);
    rec[8] = if e.custom { rec[8] | COLOUR_FLAG } else { rec[8] & !COLOUR_FLAG };
}

/// Only the two colours.
fn write_colours(rec: &mut [u8], c: Rgb) {
    rec[1..4].copy_from_slice(&[c.r, c.g, c.b]);
    rec[5..8].copy_from_slice(&[c.r, c.g, c.b]);
}

/// Put zone `z`'s effect into `report`, as OpenRGB does: single-zone
/// effects go to each record, multi-zone ones make the on-board zone lead
/// the JRGB and pipe zones (185) or set the zone's own sync flags (162).
pub fn write_effect(b: &Board, report: &mut [u8], z: Zone, e: &Effect) {
    let Some(o) = b.offset(z) else { return };
    if report.len() != b.variant.report_len() {
        return;
    }
    let single = e.mode <= b.variant.last_single();
    write_record(&mut report[o..o + 10], e);
    match b.variant {
        Variant::V185 => {
            // JRAINBOW3 lives in the JCORSAIR record, marked by its pad byte.
            report[o + 9] = if z == Jrainbow3 { 4 } else { 0 };
            let on = b.offset(Onboard).unwrap();
            let headers = matches!(z, Jrgb1 | Jrgb2 | Jpipe1 | Jpipe2);
            let lead = !b.has(Onboard) && headers;
            if lead {
                // Without an on-board zone its record still leads.
                report[on] = e.mode;
                write_colours(&mut report[on..on + 10], e.color);
                report[on + 8] |= SYNC_ONBOARD;
                report[on + 9] = 0;
            }
            if z == Onboard || lead {
                if single {
                    report[on + 4] &= !SYNC_JRGB;
                    report[on + 8] &= !SYNC_PIPES;
                } else {
                    report[on + 4] |= SYNC_JRGB;
                    report[on + 8] |= SYNC_PIPES;
                }
            }
        }
        Variant::V162 => {
            report[o + 9] = 0;
            if single {
                report[o + 4] &= !SYNC_JRGB;
                report[o + 8] &= !SYNC_ONBOARD;
            } else {
                report[o + 4] |= SYNC_JRGB;
                report[o + 8] |= SYNC_ONBOARD;
            }
        }
    }
    if z == Onboard {
        for r in b.onboard_leds() {
            if single {
                write_record(&mut report[r..r + 10], e);
                report[r + 9] = 0;
            } else {
                write_colours(&mut report[r..r + 10], e.color);
            }
        }
    }
}

/// Static, full brightness, the given colour, no sync.
fn write_static(rec: &mut [u8], c: Rgb) {
    write_record(rec, &Effect { mode: STATIC, color: c, speed: 0, brightness: MAX_BRIGHTNESS, custom: true });
    rec[8] &= !0x3f;
    rec[9] = 0;
}

/// Zone-based direct colours: every listed zone static in its colour (the
/// first of `colors`), each on-board LED in its own. `base`: the last
/// report, for the records left alone.
pub fn zone_frame(b: &Board, base: &[u8], zones: &[(Zone, &[Rgb])]) -> Vec<u8> {
    let mut r = base.to_vec();
    r.resize(b.variant.report_len(), 0);
    r[0] = REPORT_EFFECTS;
    for &(z, colors) in zones {
        let first = colors.first().copied().unwrap_or_default();
        let Some(o) = b.offset(z) else { continue };
        if z == Onboard {
            // 162-byte boards light the on-board record too; 185-byte
            // boards keep it dark.
            let lead = if b.variant == Variant::V162 { first } else { Rgb::BLACK };
            write_static(&mut r[o..o + 10], lead);
            for (i, rec) in b.onboard_leds().into_iter().enumerate() {
                write_static(&mut r[rec..rec + 10], colors.get(i).copied().unwrap_or_default());
            }
        } else {
            write_static(&mut r[o..o + 10], first);
            if z == Jrainbow3 {
                r[o + 9] = 4;
            }
        }
    }
    let s = b.save_offset();
    r[s] = 0;
    r
}

/// The effects report with the save flag set or cleared.
pub fn effects_report(b: &Board, report: &[u8], save: bool) -> Vec<u8> {
    let mut r = report.to_vec();
    r.resize(b.variant.report_len(), 0);
    r[0] = REPORT_EFFECTS;
    r[b.save_offset()] = save as u8;
    r
}

/// LEDs a header takes in the single ("synchronised") per-LED report, and
/// at most in its own report.
pub fn per_led_max(z: Zone, sync: bool) -> usize {
    match (z, sync) {
        (Jrainbow1 | Jrainbow2, true) => 40,
        (Jrainbow3 | Jcorsair, true) => 120,
        (Jrainbow1, false) => 200,
        (Jrainbow3 | Jcorsair | Jrainbow2, false) => 240,
        _ => 0,
    }
}

impl Board {
    /// LEDs before the headers in a per-LED report: on-board, pipes, JRGBs.
    fn per_led_base(&self) -> usize {
        self.onboard + self.pipe1 + self.pipe2 + self.jrgbs
    }

    /// Where zone `z`'s LEDs start in the synchronised per-LED report, or
    /// in the on-board report of the unsynchronised one (for the headers:
    /// their own report).
    fn per_led_start(&self, z: Zone, sync: bool) -> Option<usize> {
        let jrgb = self.onboard + self.pipe1 + self.pipe2;
        let base = self.per_led_base();
        match z {
            Onboard => Some(0),
            Jpipe1 => Some(self.onboard),
            Jpipe2 => Some(self.onboard + self.pipe1),
            Jrgb1 => (self.jrgbs >= 1).then_some(jrgb),
            Jrgb2 => (self.jrgbs >= 2).then_some(jrgb + 1),
            Jrainbow1 if sync => Some(base),
            Jrainbow2 if sync => Some(base + 40),
            Jrainbow3 | Jcorsair if sync => Some(base + 80),
            _ => Some(0),
        }
    }

    /// LEDs of zone `z` in per-LED mode, given the header LED counts.
    pub fn per_led_leds(&self, z: Zone, header: usize) -> usize {
        if !self.has(z) || self.per_led_start(z, true).is_none() {
            return 0;
        }
        match z {
            Onboard => self.onboard,
            Jpipe1 => self.pipe1,
            Jpipe2 => self.pipe2,
            Jrgb1 | Jrgb2 => 1,
            _ => header.min(per_led_max(z, false)),
        }
    }

    /// Whether the synchronised per-LED report fits these headers.
    pub fn per_led_sync(&self, headers: &[(Zone, usize)]) -> bool {
        self.per_led_base() + 200 <= PER_LED_LEDS && headers.iter().all(|&(z, n)| n <= per_led_max(z, true))
    }
}

/// The 0x52 report that switches per-LED mode on: every record static
/// with fixed flags, the on-board record leading with effect 0x25. `sync`:
/// one 0x53 report for all LEDs (headers up to 40/40/120 LEDs); otherwise
/// each header gets its own.
pub fn per_led_enable(b: &Board, sync: bool) -> Vec<u8> {
    let mut r = vec![0u8; Variant::V185.report_len()];
    r[0] = REPORT_EFFECTS;
    let mut rec = |o: usize, effect: u8, sb: u8, flags: u8| {
        r[o] = effect;
        r[o + 4] = sb;
        r[o + 8] = flags;
    };
    rec(1, STATIC, 0x08, 0x80); // JRGB1
    rec(11, STATIC, 0x2a, 0x80); // JPIPE1
    rec(21, STATIC, 0x2a, 0x80); // JPIPE2
    let header = |z: Zone| if !sync && b.has(z) { PER_LED_EFFECT } else { STATIC };
    rec(31, header(Jrainbow1), 0x29, 0x80);
    rec(42, header(Jrainbow2), 0x29, 0x80);
    let corsair = if b.has(Jcorsair) || b.has(Jrainbow3) { header(Jcorsair) } else { STATIC };
    rec(53, corsair, 0x29, 0x82);
    rec(64, STATIC, 0x28, 0x80); // JCORSAIR outer
    // On-board leads, synced with everything (or the on-board zones only).
    rec(74, PER_LED_EFFECT, 0xa9, if sync { 0xbf } else { 0xb1 });
    for o in (84..=164).step_by(10) {
        rec(o, STATIC, 0x28, 0x80);
    }
    rec(174, STATIC, 0x2a, 0x80); // JRGB2
    r[41] = if sync { 40 } else { 200 };
    r[52] = if sync { 40 } else { 240 };
    r[63] = if sync { 120 } else { 240 };
    r
}

/// A 0x53 report: `53 25 area index 00` and up to 240 colours.
pub fn per_led_report(area: u8, index: u8, colors: &[Rgb]) -> Vec<u8> {
    let mut r = vec![0u8; PER_LED_LEN];
    r[..5].copy_from_slice(&[REPORT_PER_LED, PER_LED_EFFECT, area, index, 0]);
    for (c, slot) in colors.iter().zip(r[5..].as_chunks_mut::<3>().0) {
        *slot = [c.r, c.g, c.b];
    }
    r
}

/// Report areas: JRAINBOW (index 0 or 1), JCORSAIR / JRAINBOW3, on-board.
const AREA_RAINBOW: u8 = 0x04;
const AREA_CORSAIR: u8 = 0x05;
const AREA_ONBOARD: u8 = 0x06;

/// The 0x53 reports for one frame: one when `sync`, otherwise one per
/// header the board has plus the on-board one (sent last).
pub fn per_led_frame(b: &Board, zones: &[(Zone, &[Rgb])], sync: bool) -> Vec<Vec<u8>> {
    let mut main = vec![Rgb::BLACK; PER_LED_LEDS];
    let mut own: Vec<Vec<u8>> = Vec::new();
    for &(z, colors) in zones {
        let n = b.per_led_leds(z, colors.len()).min(colors.len());
        let Some(start) = b.per_led_start(z, sync) else { continue };
        let header = z.addressable();
        if header && !sync {
            let (area, index) = match z {
                Jrainbow1 => (AREA_RAINBOW, 0),
                Jrainbow2 => (AREA_RAINBOW, 1),
                _ => (AREA_CORSAIR, 0),
            };
            own.push(per_led_report(area, index, &colors[..n]));
        } else {
            for (slot, c) in main.iter_mut().skip(start).zip(&colors[..n]) {
                *slot = *c;
            }
        }
    }
    own.push(per_led_report(AREA_ONBOARD, 0, &main));
    own
}

/// Firmware version request: a 64-byte output report `01 sub cc...`
/// (`b0` the application, `b6` the loader). The reply's byte 2 is the
/// version, high nibble major.
pub fn version_request(sub: u8) -> [u8; 64] {
    let mut r = [0xccu8; 64];
    r[0] = 0x01;
    r[1] = sub;
    r
}

pub const VERSION_APROM: u8 = 0xb0;
pub const VERSION_LDROM: u8 = 0xb6;

pub fn parse_version(reply: &[u8], sub: u8) -> Option<u8> {
    (reply.len() > 2 && reply[0] == 0x01 && reply[1] == sub).then(|| reply[2])
}

pub fn version_string(v: u8) -> String {
    format!("{}.{}", v >> 4, v & 0x0f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|x| u8::from_str_radix(x, 16).unwrap()).collect()
    }

    #[test]
    fn lengths() {
        assert_eq!(Variant::from_len(185), Some(Variant::V185));
        assert_eq!(Variant::from_len(186), Some(Variant::V185));
        assert_eq!(Variant::from_len(162), Some(Variant::V162));
        assert_eq!(Variant::from_len(184), None);
        assert_eq!(Variant::from_len(112), None);
        assert_eq!(Variant::from_len(761), None);
        let b = board(Variant::V185, 0x7c91);
        assert_eq!(effects_report(&b, &[0x52; 10], true).len(), 185);
        assert_eq!(effects_report(&b, &[0x52; 10], true)[184], 1);
        assert_eq!(per_led_enable(&b, true).len(), 185);
        assert_eq!(per_led_report(4, 0, &[]).len(), 725);
        let b = board(Variant::V162, 0x7b18);
        assert_eq!(effects_report(&b, &[0x52; 200], false).len(), 162);
        assert_eq!(zone_frame(&b, &[0x52; 162], &[]).len(), 162);
    }

    #[test]
    fn board_tables() {
        let b = board(Variant::V185, 0x7c91);
        assert_eq!((b.name, b.onboard, b.jrgbs, b.direct), (Some("MAG B550 TOMAHAWK"), 6, 2, Direct::PerLed));
        assert!(b.has(Jrgb2) && !b.has(Jcorsair));
        let unknown = board(Variant::V185, 0x7e20);
        assert_eq!((unknown.name, unknown.direct), (None, Direct::None));
        let b = board(Variant::V162, 0xb926);
        assert_eq!((b.onboard, b.zones), (3, W1O));
        assert_eq!(virtual_pid("7E12A1B2C3"), Some(0x7e12));
        assert_eq!(virtual_pid("x"), None);
        for r in BOARDS_185 {
            let b = board(Variant::V185, r.0);
            assert!(b.per_led_base() <= 40, "{:#06x}", r.0);
        }
    }

    #[test]
    fn record_offsets() {
        let old = board(Variant::V185, 0x7c91);
        assert_eq!(old.offset(Jrgb2), Some(174));
        let new = board(Variant::V185, 0x7d15);
        assert_eq!(new.offset(Jrgb2), Some(134));
        assert_eq!(new.onboard_leds(), vec![84, 94, 104, 114, 124, 64]);
        let v1 = board(Variant::V162, 0x7b50);
        assert_eq!(v1.offset(Jrainbow2), Some(141));
        assert_eq!(v1.onboard_leds(), vec![51, 61, 71, 81, 91, 101]);
        assert_eq!(v1.offset(Jpipe1), None);
    }

    #[test]
    fn effect_round_trip() {
        let b = board(Variant::V185, 0x7c91);
        let mut r = vec![0u8; 185];
        r[0] = 0x52;
        let e = Effect { mode: 0x02, color: Rgb::new(255, 0, 0), speed: 2, brightness: 7, custom: true };
        write_effect(&b, &mut r, Jrainbow1, &e);
        assert_eq!(&r[31..42], &bytes("02 ff 00 00 1e ff 00 00 80 00 00")[..]);
        assert_eq!(parse_effect(&r[31..]), Some(e));
        // A single-zone effect on the on-board zone reaches every LED.
        write_effect(&b, &mut r, Onboard, &e);
        assert_eq!(r[74], 0x02);
        assert_eq!(r[134], 0x02);
        assert_eq!(r[74 + 4] & 0x80, 0);
        // A multi-zone one leads JRGB and the pipes.
        let wave = Effect { mode: 0x1a, custom: false, ..e };
        write_effect(&b, &mut r, Onboard, &wave);
        assert_eq!((r[74], r[78] & 0x80, r[82] & 0x30, r[82] & 0x80), (0x1a, 0x80, 0x30, 0));
        assert_eq!(r[84], 0x02, "LED records keep their effect");
    }

    #[test]
    fn headers_lead_without_onboard_leds() {
        let b = board(Variant::V185, 0x7d75);
        let mut r = vec![0u8; 185];
        let e = Effect { mode: 0x15, ..Effect::default() };
        write_effect(&b, &mut r, Jrgb1, &e);
        assert_eq!((r[74], r[82] & 0x31, r[78] & 0x80), (0x15, 0x31, 0x80));
        write_effect(&b, &mut r, Jrainbow3, &e);
        assert_eq!(r[53 + 9], 4);
    }

    #[test]
    fn sync_flags_on_162() {
        let b = board(Variant::V162, 0x7b18);
        let mut r = vec![0u8; 162];
        write_effect(&b, &mut r, Jrgb1, &Effect { mode: 0x05, ..Effect::default() });
        assert_eq!((r[5] & 0x80, r[9] & 0x01), (0x80, 0x01), "Lightning is multi-zone on 162");
        write_effect(&b, &mut r, Jrgb1, &Effect { mode: 0x03, ..Effect::default() });
        assert_eq!((r[5] & 0x80, r[9] & 0x01), (0, 0));
    }

    #[test]
    fn per_led_enable_matches_capture() {
        // The synchronised switch-on report as captured from MSI's software
        // (JCORSAIR static, as OpenRGB sends it).
        let b = board(Variant::V185, 0x7c79);
        let r = per_led_enable(&b, true);
        assert_eq!(&r[1..11], &bytes("01 00 00 00 08 00 00 00 80 00")[..]);
        assert_eq!(&r[31..42], &bytes("01 00 00 00 29 00 00 00 80 00 28")[..]);
        assert_eq!(&r[53..64], &bytes("01 00 00 00 29 00 00 00 82 00 78")[..]);
        assert_eq!(&r[74..84], &bytes("25 00 00 00 a9 00 00 00 bf 00")[..]);
        assert_eq!(&r[174..185], &bytes("01 00 00 00 2a 00 00 00 80 00 00")[..]);
        let u = per_led_enable(&b, false);
        assert_eq!(&u[31..42], &bytes("25 00 00 00 29 00 00 00 80 00 c8")[..]);
        assert_eq!(u[42], 0x25);
        assert_eq!(u[53], 0x01, "no JCORSAIR on this board");
        assert_eq!(u[82], 0xb1);
    }

    #[test]
    fn per_led_layout() {
        // 7C79: 6 on-board LEDs, 2 JRGB, 2 JRAINBOW.
        let b = board(Variant::V185, 0x7c79);
        let on: Vec<Rgb> = (0..6).map(|i| Rgb::new(i, 0, 0)).collect();
        let w1: Vec<Rgb> = (0..3).map(|i| Rgb::new(0, i + 1, 0)).collect();
        let j1 = [Rgb::new(9, 9, 9)];
        let zones: Vec<(Zone, &[Rgb])> = vec![(Jrgb1, &j1), (Jrainbow1, &w1), (Onboard, &on)];
        assert!(b.per_led_sync(&[(Jrainbow1, 3)]));
        let f = per_led_frame(&b, &zones, true);
        assert_eq!(f.len(), 1);
        let r = &f[0];
        assert_eq!(&r[..5], &[0x53, 0x25, 0x06, 0, 0]);
        assert_eq!(r[5 + 5 * 3], 5);
        assert_eq!(&r[0x17..0x1a], &[9, 9, 9]);
        assert_eq!(r[0x1d + 1], 1);
        // Unsynchronised: the header gets its own report.
        let f = per_led_frame(&b, &zones, false);
        assert_eq!(f.len(), 2);
        assert_eq!(&f[0][..6], &[0x53, 0x25, 0x04, 0, 0, 0]);
        assert_eq!(f[0][6], 1);
        assert_eq!(f[1][2], 0x06);
        assert!(!b.per_led_sync(&[(Jrainbow1, 41)]));
    }

    #[test]
    fn zone_based_frame() {
        let b = board(Variant::V162, 0x7b18);
        let base = vec![0u8; 162];
        let on: Vec<Rgb> = (0..6).map(|i| Rgb::new(i * 10, 0, 0)).collect();
        let red = [Rgb::new(255, 0, 0)];
        let r = zone_frame(&b, &base, &[(Jrgb1, &red), (Onboard, &on)]);
        assert_eq!(&r[1..11], &bytes("01 ff 00 00 28 ff 00 00 80 00")[..]);
        assert_eq!((r[41], r[51 + 1], r[61 + 1], r[101 + 1]), (1, 0, 10, 50));
        assert_eq!(r[161], 0);
    }

    #[test]
    fn version() {
        let q = version_request(VERSION_APROM);
        assert_eq!(&q[..3], &[0x01, 0xb0, 0xcc]);
        assert_eq!(parse_version(&[0x01, 0xb0, 0x16], VERSION_APROM), Some(0x16));
        assert_eq!(parse_version(&[0x02, 0xb0, 0x16], VERSION_APROM), None);
        assert_eq!(version_string(0x16), "1.6");
    }

    #[test]
    fn mode_tables() {
        for v in [Variant::V162, Variant::V185] {
            let m = modes(v);
            let mut b: Vec<u8> = m.iter().map(|m| m.0).collect();
            b.dedup();
            assert_eq!(b.len(), m.len());
            assert!(m.iter().all(|m| m.0 != PER_LED_EFFECT));
        }
    }
}
