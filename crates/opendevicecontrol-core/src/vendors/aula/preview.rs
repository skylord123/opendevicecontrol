//! On-screen preview of the keyboard's built-in effects. The keyboard
//! doesn't report its LEDs, so the preview simulates its firmware (AULA F99,
//! BY916 V14.2, from static analysis and emulation of the decrypted image):
//! the same effect state, per-step logic, colour tables, random generator
//! and LED maths, run on a clock that matches the keyboard's real step times.
//!
//! The firmware keeps an effect layer per LED — a base colour and an
//! intensity 0–32 — and the LED output. Some effects draw the output
//! directly every step; the others "hit" keys (base colour, intensity 32,
//! shown at once) and a separate compose pass fades them
//! (`out = base · intensity / 32`, intensity − 1 per pass). Every colour is
//! scaled by the brightness level (0/20/40/60/80 ÷ 80) on its way out.
//!
//! LEDs are numbered `column · 6 + row` (19 columns × 6 rows used). Effects
//! work on a matrix position (column, row) and map it to an LED through the
//! firmware's tables, which shift columns 1–12 of row 4 for the ISO key.

use std::collections::VecDeque;

use super::layout::Led;
use crate::model::Rgb;

/// The keyboard's default colour palette (slots 0–6): the ROM default at
/// firmware 0x2599, copied into every effect's block of the flash palette
/// (0xC800).
pub const PALETTE: [Rgb; 7] = [
    Rgb::new(255, 0, 0),
    Rgb::new(0, 255, 0),
    Rgb::new(0, 0, 255),
    Rgb::new(255, 255, 0),
    Rgb::new(255, 0, 255),
    Rgb::new(0, 255, 255),
    Rgb::new(255, 255, 255),
];

/// Brightness levels 0–4 in 80ths (firmware 0x2594).
const BRIGHTNESS: [u32; 5] = [0, 20, 40, 60, 80];

/// LED slots: 21 columns × 6 rows (the firmware's buffers; columns 19 and
/// 20 have no LEDs on the F99).
const LEDS: usize = 21 * 6;
/// Matrix columns the effects walk.
const COLS: usize = 19;

/// Steady/Blossoming rings around matrix (8, 2) (firmware 0x25B1): 11 rings of up to
/// 13 matrix positions `col << 3 | row`, 0xFF unused.
#[rustfmt::skip]
const RINGS: [[u8; 13]; 11] = [
    [0x42, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
    [0x41, 0x49, 0x3A, 0x4A, 0x3B, 0x43, 0x4B, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
    [0x38, 0x40, 0x39, 0x51, 0x32, 0x52, 0x33, 0xFF, 0x44, 0x3C, 0x4C, 0x48, 0xFF],
    [0x30, 0x31, 0x2A, 0x2B, 0x34, 0x2D, 0x54, 0x53, 0x5A, 0x59, 0x50, 0x4D, 0x5B],
    [0x28, 0x29, 0x22, 0x23, 0x2C, 0xFF, 0x58, 0x61, 0x62, 0x63, 0x5C, 0x45, 0xFF],
    [0x21, 0x1A, 0x1B, 0x24, 0x60, 0x69, 0x6A, 0x55, 0x25, 0x64, 0x6B, 0x6C, 0x68],
    [0x20, 0x19, 0x12, 0x13, 0x1C, 0x78, 0x71, 0x72, 0xFF, 0x5D, 0xFF, 0x15, 0xFF],
    [0x18, 0x11, 0x0A, 0x0B, 0x14, 0x70, 0x79, 0x7A, 0x65, 0xFF, 0xFF, 0xFF, 0xFF],
    [0x10, 0x09, 0xFF, 0x7B, 0x7C, 0x0D, 0xFF, 0x6D, 0x73, 0x74, 0x75, 0x08, 0x7D], // slot 6: 0x0C in V14.2, unused in 0x1400
    [0x01, 0x85, 0x83, 0x81, 0x05, 0x04, 0x02, 0xFF, 0xFF, 0xFF, 0x82, 0x84, 0x80],
    [0x00, 0x88, 0x89, 0x90, 0x91, 0x92, 0x94, 0x03, 0xFF, 0x8A, 0x8B, 0x8C, 0x8D],
];

/// Breathing-shaped ramp 22→255→22 (firmware 0x2640, 96 bytes) and the grey-triplet table
/// that follows it (0x26A0): Neon stream reads the first 96, Blossoming in a single colour
/// reads all 192 (see `rings`).
#[rustfmt::skip]
const RAMP: [u8; 192] = [
    0x16, 0x18, 0x1B, 0x1E, 0x21, 0x24, 0x27, 0x2A, 0x2D, 0x31, 0x35, 0x39, 0x3D, 0x41, 0x45, 0x49, 0x4D, 0x51, 0x55, 0x59, 0x5E, 0x63, 0x68, 0x6D,
    0x72, 0x77, 0x7C, 0x81, 0x86, 0x8C, 0x92, 0x98, 0x9E, 0xA4, 0xAA, 0xB0, 0xB6, 0xBC, 0xC2, 0xC8, 0xCE, 0xD5, 0xDC, 0xE3, 0xEA, 0xF1, 0xF8, 0xFF,
    0xFF, 0xF8, 0xF1, 0xEA, 0xE3, 0xDC, 0xD5, 0xCE, 0xC8, 0xC2, 0xBC, 0xB6, 0xB0, 0xAA, 0xA4, 0x9E, 0x98, 0x92, 0x8C, 0x86, 0x81, 0x7C, 0x77, 0x72,
    0x6D, 0x68, 0x63, 0x5E, 0x59, 0x55, 0x51, 0x4D, 0x49, 0x45, 0x41, 0x3D, 0x39, 0x35, 0x31, 0x2D, 0x2A, 0x27, 0x24, 0x21, 0x1E, 0x1B, 0x18, 0x16,
    0x01, 0x01, 0x01, 0xBF, 0xBF, 0xBF, 0x06, 0x06, 0x06, 0xBE, 0xBE, 0xBE, 0x0B, 0x0B, 0x0B, 0xBC, 0xBC, 0xBC, 0x10, 0x10, 0x10, 0xB9, 0xB9, 0xB9,
    0x15, 0x15, 0x15, 0xB4, 0xB4, 0xB4, 0x1A, 0x1A, 0x1A, 0xAF, 0xAF, 0xAF, 0x1F, 0x1F, 0x1F, 0xAA, 0xAA, 0xAA, 0x24, 0x24, 0x24, 0xA5, 0xA5, 0xA5,
    0x29, 0x29, 0x29, 0xA0, 0xA0, 0xA0, 0x2E, 0x2E, 0x2E, 0x9B, 0x9B, 0x9B, 0x33, 0x33, 0x33, 0x96, 0x96, 0x96, 0x38, 0x38, 0x38, 0x91, 0x91, 0x91,
    0x3D, 0x3D, 0x3D, 0x8C, 0x8C, 0x8C, 0x42, 0x42, 0x42, 0x87, 0x87, 0x87, 0x47, 0x47, 0x47, 0x82, 0x82, 0x82, 0x4C, 0x4C, 0x4C, 0x7D, 0x7D, 0x7D,
];

/// 192-step colour wheel R→Y→G→C→B→M→R (firmware 0x279C).
#[rustfmt::skip]
const W192: [[u8; 3]; 192] = [
    [0xFF, 0x01, 0x00], [0xFF, 0x03, 0x00], [0xFF, 0x05, 0x00], [0xFF, 0x07, 0x00], [0xFF, 0x0A, 0x00], [0xFF, 0x0E, 0x00], [0xFF, 0x12, 0x00], [0xFF, 0x16, 0x00],
    [0xFF, 0x1A, 0x00], [0xFF, 0x20, 0x00], [0xFF, 0x26, 0x00], [0xFF, 0x2C, 0x00], [0xFF, 0x32, 0x00], [0xFF, 0x39, 0x00], [0xFF, 0x41, 0x00], [0xFF, 0x49, 0x00],
    [0xFF, 0x51, 0x00], [0xFF, 0x59, 0x00], [0xFF, 0x63, 0x00], [0xFF, 0x6D, 0x00], [0xFF, 0x77, 0x00], [0xFF, 0x81, 0x00], [0xFF, 0x8C, 0x00], [0xFF, 0x98, 0x00],
    [0xFF, 0xA4, 0x00], [0xFF, 0xB0, 0x00], [0xFF, 0xBC, 0x00], [0xFF, 0xC8, 0x00], [0xFF, 0xD5, 0x00], [0xFF, 0xE3, 0x00], [0xFF, 0xF1, 0x00], [0xFF, 0xFF, 0x00],
    [0xF8, 0xFF, 0x00], [0xEA, 0xFF, 0x00], [0xDC, 0xFF, 0x00], [0xCE, 0xFF, 0x00], [0xC2, 0xFF, 0x00], [0xB6, 0xFF, 0x00], [0xAA, 0xFF, 0x00], [0x9E, 0xFF, 0x00],
    [0x92, 0xFF, 0x00], [0x86, 0xFF, 0x00], [0x7C, 0xFF, 0x00], [0x72, 0xFF, 0x00], [0x68, 0xFF, 0x00], [0x5E, 0xFF, 0x00], [0x55, 0xFF, 0x00], [0x4D, 0xFF, 0x00],
    [0x45, 0xFF, 0x00], [0x3D, 0xFF, 0x00], [0x35, 0xFF, 0x00], [0x2F, 0xFF, 0x00], [0x29, 0xFF, 0x00], [0x23, 0xFF, 0x00], [0x1D, 0xFF, 0x00], [0x18, 0xFF, 0x00],
    [0x14, 0xFF, 0x00], [0x10, 0xFF, 0x00], [0x0C, 0xFF, 0x00], [0x08, 0xFF, 0x00], [0x06, 0xFF, 0x00], [0x04, 0xFF, 0x00], [0x02, 0xFF, 0x00], [0x00, 0xFF, 0x00],
    [0x00, 0xFF, 0x01], [0x00, 0xFF, 0x03], [0x00, 0xFF, 0x05], [0x00, 0xFF, 0x07], [0x00, 0xFF, 0x0A], [0x00, 0xFF, 0x0E], [0x00, 0xFF, 0x12], [0x00, 0xFF, 0x16],
    [0x00, 0xFF, 0x1A], [0x00, 0xFF, 0x20], [0x00, 0xFF, 0x26], [0x00, 0xFF, 0x2C], [0x00, 0xFF, 0x32], [0x00, 0xFF, 0x39], [0x00, 0xFF, 0x41], [0x00, 0xFF, 0x49],
    [0x00, 0xFF, 0x51], [0x00, 0xFF, 0x59], [0x00, 0xFF, 0x63], [0x00, 0xFF, 0x6D], [0x00, 0xFF, 0x77], [0x00, 0xFF, 0x81], [0x00, 0xFF, 0x8C], [0x00, 0xFF, 0x98],
    [0x00, 0xFF, 0xA4], [0x00, 0xFF, 0xB0], [0x00, 0xFF, 0xBC], [0x00, 0xFF, 0xC8], [0x00, 0xFF, 0xD5], [0x00, 0xFF, 0xE3], [0x00, 0xFF, 0xF1], [0x00, 0xFF, 0xFF],
    [0x00, 0xF8, 0xFF], [0x00, 0xEA, 0xFF], [0x00, 0xDC, 0xFF], [0x00, 0xCE, 0xFF], [0x00, 0xC2, 0xFF], [0x00, 0xB6, 0xFF], [0x00, 0xAA, 0xFF], [0x00, 0x9E, 0xFF],
    [0x00, 0x92, 0xFF], [0x00, 0x86, 0xFF], [0x00, 0x7C, 0xFF], [0x00, 0x72, 0xFF], [0x00, 0x68, 0xFF], [0x00, 0x5E, 0xFF], [0x00, 0x55, 0xFF], [0x00, 0x4D, 0xFF],
    [0x00, 0x45, 0xFF], [0x00, 0x3D, 0xFF], [0x00, 0x35, 0xFF], [0x00, 0x2F, 0xFF], [0x00, 0x29, 0xFF], [0x00, 0x23, 0xFF], [0x00, 0x1D, 0xFF], [0x00, 0x18, 0xFF],
    [0x00, 0x14, 0xFF], [0x00, 0x10, 0xFF], [0x00, 0x0C, 0xFF], [0x00, 0x08, 0xFF], [0x00, 0x06, 0xFF], [0x00, 0x04, 0xFF], [0x00, 0x02, 0xFF], [0x00, 0x00, 0xFF],
    [0x01, 0x00, 0xFF], [0x03, 0x00, 0xFF], [0x05, 0x00, 0xFF], [0x07, 0x00, 0xFF], [0x0A, 0x00, 0xFF], [0x0E, 0x00, 0xFF], [0x12, 0x00, 0xFF], [0x16, 0x00, 0xFF],
    [0x1A, 0x00, 0xFF], [0x20, 0x00, 0xFF], [0x26, 0x00, 0xFF], [0x2C, 0x00, 0xFF], [0x32, 0x00, 0xFF], [0x39, 0x00, 0xFF], [0x41, 0x00, 0xFF], [0x49, 0x00, 0xFF],
    [0x51, 0x00, 0xFF], [0x59, 0x00, 0xFF], [0x63, 0x00, 0xFF], [0x6D, 0x00, 0xFF], [0x77, 0x00, 0xFF], [0x81, 0x00, 0xFF], [0x8C, 0x00, 0xFF], [0x98, 0x00, 0xFF],
    [0xA4, 0x00, 0xFF], [0xB0, 0x00, 0xFF], [0xBC, 0x00, 0xFF], [0xC8, 0x00, 0xFF], [0xD5, 0x00, 0xFF], [0xE3, 0x00, 0xFF], [0xF1, 0x00, 0xFF], [0xFF, 0x00, 0xFF],
    [0xFF, 0x00, 0xF8], [0xFF, 0x00, 0xEA], [0xFF, 0x00, 0xDC], [0xFF, 0x00, 0xCE], [0xFF, 0x00, 0xC2], [0xFF, 0x00, 0xB6], [0xFF, 0x00, 0xAA], [0xFF, 0x00, 0x9E],
    [0xFF, 0x00, 0x92], [0xFF, 0x00, 0x86], [0xFF, 0x00, 0x7C], [0xFF, 0x00, 0x72], [0xFF, 0x00, 0x68], [0xFF, 0x00, 0x5E], [0xFF, 0x00, 0x55], [0xFF, 0x00, 0x4D],
    [0xFF, 0x00, 0x45], [0xFF, 0x00, 0x3D], [0xFF, 0x00, 0x35], [0xFF, 0x00, 0x2F], [0xFF, 0x00, 0x29], [0xFF, 0x00, 0x23], [0xFF, 0x00, 0x1D], [0xFF, 0x00, 0x18],
    [0xFF, 0x00, 0x14], [0xFF, 0x00, 0x10], [0xFF, 0x00, 0x0C], [0xFF, 0x00, 0x08], [0xFF, 0x00, 0x06], [0xFF, 0x00, 0x04], [0xFF, 0x00, 0x02], [0xFF, 0x00, 0x00],
];

/// 128-step colour wheel B→C→G→Y→R→M→B used by the windmill (firmware 0x29DF).
#[rustfmt::skip]
const W128: [[u8; 3]; 128] = [
    [0x00, 0x05, 0xFF], [0x00, 0x11, 0xFF], [0x00, 0x1D, 0xFF], [0x00, 0x30, 0xFF], [0x00, 0x41, 0xFF], [0x00, 0x55, 0xFF], [0x00, 0x69, 0xFF], [0x00, 0x7F, 0xFF],
    [0x00, 0x89, 0xFF], [0x00, 0x94, 0xFF], [0x00, 0xA1, 0xFF], [0x00, 0xAB, 0xFF], [0x00, 0xB6, 0xFF], [0x00, 0xC0, 0xFF], [0x00, 0xCA, 0xFF], [0x00, 0xD3, 0xFF],
    [0x00, 0xDC, 0xFF], [0x00, 0xE5, 0xFF], [0x00, 0xED, 0xFF], [0x00, 0xF3, 0xFF], [0x00, 0xFA, 0xFF], [0x00, 0xFF, 0xFF], [0x00, 0xFF, 0xFA], [0x00, 0xFF, 0xF6],
    [0x00, 0xFF, 0xEE], [0x00, 0xFF, 0xE7], [0x00, 0xFF, 0xDF], [0x00, 0xFF, 0xD6], [0x00, 0xFF, 0xCD], [0x00, 0xFF, 0xC3], [0x00, 0xFF, 0xBA], [0x00, 0xFF, 0xAF],
    [0x00, 0xFF, 0xA5], [0x00, 0xFF, 0x99], [0x00, 0xFF, 0x8F], [0x00, 0xFF, 0x82], [0x00, 0xFF, 0x72], [0x00, 0xFF, 0x5D], [0x00, 0xFF, 0x49], [0x00, 0xFF, 0x36],
    [0x00, 0xFF, 0x25], [0x00, 0xFF, 0x15], [0x00, 0xFF, 0x08], [0x00, 0xFF, 0x00], [0x0B, 0xFF, 0x00], [0x16, 0xFF, 0x00], [0x24, 0xFF, 0x00], [0x34, 0xFF, 0x00],
    [0x46, 0xFF, 0x00], [0x57, 0xFF, 0x00], [0x6A, 0xFF, 0x00], [0x7E, 0xFF, 0x00], [0x88, 0xFF, 0x00], [0x90, 0xFF, 0x00], [0x98, 0xFF, 0x00], [0x9F, 0xFF, 0x00],
    [0xA8, 0xFF, 0x00], [0xB0, 0xFF, 0x00], [0xB7, 0xFF, 0x00], [0xC0, 0xFF, 0x00], [0xC6, 0xFF, 0x00], [0xCE, 0xFF, 0x00], [0xD4, 0xFF, 0x00], [0xDC, 0xFF, 0x00],
    [0xE1, 0xFF, 0x00], [0xE7, 0xFF, 0x00], [0xED, 0xFF, 0x00], [0xF3, 0xFF, 0x00], [0xF7, 0xFF, 0x00], [0xFB, 0xFF, 0x00], [0xFF, 0xFF, 0x00], [0xFF, 0xF9, 0x00],
    [0xFF, 0xF2, 0x00], [0xFF, 0xEC, 0x00], [0xFF, 0xE1, 0x00], [0xFF, 0xD7, 0x00], [0xFF, 0xCE, 0x00], [0xFF, 0xC3, 0x00], [0xFF, 0xB7, 0x00], [0xFF, 0xAA, 0x00],
    [0xFF, 0x9F, 0x00], [0xFF, 0x91, 0x00], [0xFF, 0x84, 0x00], [0xFF, 0x69, 0x00], [0xFF, 0x44, 0x00], [0xFF, 0x22, 0x00], [0xFF, 0x0A, 0x00], [0xFF, 0x00, 0x05],
    [0xFF, 0x00, 0x11], [0xFF, 0x00, 0x1E], [0xFF, 0x00, 0x32], [0xFF, 0x00, 0x45], [0xFF, 0x00, 0x5B], [0xFF, 0x00, 0x71], [0xFF, 0x00, 0x83], [0xFF, 0x00, 0x8E],
    [0xFF, 0x00, 0x9B], [0xFF, 0x00, 0xA5], [0xFF, 0x00, 0xB1], [0xFF, 0x00, 0xBD], [0xFF, 0x00, 0xC7], [0xFF, 0x00, 0xD1], [0xFF, 0x00, 0xDB], [0xFF, 0x00, 0xE5],
    [0xFF, 0x00, 0xEC], [0xFF, 0x00, 0xF3], [0xFF, 0x00, 0xFA], [0xFF, 0x00, 0xFF], [0xFA, 0x00, 0xFF], [0xF3, 0x00, 0xFF], [0xED, 0x00, 0xFF], [0xE4, 0x00, 0xFF],
    [0xDC, 0x00, 0xFF], [0xD1, 0x00, 0xFF], [0xC8, 0x00, 0xFF], [0xBD, 0x00, 0xFF], [0xB2, 0x00, 0xFF], [0xA8, 0x00, 0xFF], [0x9E, 0x00, 0xFF], [0x8F, 0x00, 0xFF],
    [0x83, 0x00, 0xFF], [0x6F, 0x00, 0xFF], [0x5A, 0x00, 0xFF], [0x45, 0x00, 0xFF], [0x32, 0x00, 0xFF], [0x1F, 0x00, 0xFF], [0x10, 0x00, 0xFF], [0x05, 0x00, 0xFF],
];

/// Matrix position (row·21 + col) → LED column (firmware 0x2B5F) and, 126 bytes on, LED row
/// (0x2BDD); identity except row 4, where the ISO key shifts columns 1–12. Kept with the
/// bytes that follow (speed tables …) because Stars twinkle reads up to 255 entries past
/// each start.
#[rustfmt::skip]
const MAP: [u8; 382] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x14,
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x14,
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x14,
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x14,
    0x00, 0x0C, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x14,
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x14,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
    0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02,
    0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03,
    0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04,
    0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05,
    0x78, 0x64, 0x50, 0x32, 0x14, 0x3C, 0x32, 0x28, 0x1E, 0x14, 0x2D, 0x23, 0x19, 0x0F, 0x05, 0x2D, 0x23, 0x19, 0x0F, 0x06, 0x50,
    0x3C, 0x28, 0x14, 0x08, 0x14, 0x0F, 0x0A, 0x05, 0x01, 0x58, 0x44, 0x30, 0x1C, 0x08, 0x73, 0x64, 0x55, 0x3C, 0x1E, 0x78, 0x5A,
    0x46, 0x2D, 0x01, 0x1E, 0x18, 0x12, 0x0C, 0x06, 0x20, 0x18, 0x12, 0x10, 0x06, 0x73, 0x5F, 0x4B, 0x37, 0x1E, 0x20, 0x18, 0x10,
    0x08, 0x01, 0x2E, 0x24, 0x1A, 0x10, 0x06, 0x32, 0x28, 0x1E, 0x14, 0x08, 0x11, 0x01, 0x11, 0x02, 0x11, 0x03, 0x11, 0x04, 0x11,
    0x05, 0x1E, 0x0F, 0x04, 0x00, 0x1E, 0x0F, 0x04, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x12, 0x03, 0x0E, 0x03, 0x0E, 0x02, 0x0E,
    0x01, 0x0E, 0x00, 0x20, 0x54, 0x09, 0x20, 0x38, 0x06, 0x20, 0x56, 0x03, 0x02, 0x2D, 0x73, 0x90, 0x03, 0x06, 0xE0, 0xFF, 0x75,
    0xF0, 0x1C, 0xA4, 0x24,
];

/// Breathing envelope, 128 steps 0→255→0 (firmware 0xC480).
#[rustfmt::skip]
const ENVELOPE: [u8; 128] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0C, 0x0E, 0x10, 0x12, 0x14, 0x16, 0x18, 0x1B, 0x1E, 0x21, 0x24, 0x27, 0x2A, 0x2D, 0x31, 0x35, 0x39, 0x3D, 0x41, 0x45, 0x49,
    0x4D, 0x51, 0x55, 0x59, 0x5E, 0x63, 0x68, 0x6D, 0x72, 0x77, 0x7C, 0x81, 0x86, 0x8C, 0x92, 0x98, 0x9E, 0xA4, 0xAA, 0xB0, 0xB6, 0xBC, 0xC2, 0xC8, 0xCE, 0xD5, 0xDC, 0xE3, 0xEA, 0xF1, 0xF8, 0xFF,
    0xFF, 0xF8, 0xF1, 0xEA, 0xE3, 0xDC, 0xD5, 0xCE, 0xC8, 0xC2, 0xBC, 0xB6, 0xB0, 0xAA, 0xA4, 0x9E, 0x98, 0x92, 0x8C, 0x86, 0x81, 0x7C, 0x77, 0x72, 0x6D, 0x68, 0x63, 0x5E, 0x59, 0x55, 0x51, 0x4D,
    0x49, 0x45, 0x41, 0x3D, 0x39, 0x35, 0x31, 0x2D, 0x2A, 0x27, 0x24, 0x21, 0x1E, 0x1B, 0x18, 0x16, 0x14, 0x12, 0x10, 0x0E, 0x0C, 0x0A, 0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0x00,
];

/// Key index (scan col·6 + row) → LED position `col << 3 | row`, 0xFF = no key (firmware
/// 0xC500). Breathing, Rainbow and Waterfall only light positions that have a key.
#[rustfmt::skip]
const KEYS: [u8; 126] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05,
    0x08, 0x09, 0x0A, 0x0B, 0x14, 0x0D,
    0x10, 0x11, 0x12, 0x13, 0x1C, 0x15,
    0x18, 0x19, 0x1A, 0x1B, 0x24, 0xFF,
    0x20, 0x21, 0x22, 0x23, 0x2C, 0xFF,
    0x28, 0x29, 0x2A, 0x2B, 0x34, 0x2D,
    0x30, 0x31, 0x32, 0x33, 0x3C, 0xFF,
    0x38, 0x39, 0x3A, 0x3B, 0x44, 0xFF,
    0x40, 0x41, 0x42, 0x43, 0x4C, 0x45,
    0x48, 0x49, 0x4A, 0x4B, 0x54, 0x4D,
    0x50, 0x51, 0x52, 0x53, 0x5C, 0x55,
    0x58, 0x59, 0x5A, 0x5B, 0x64, 0xFF,
    0x60, 0x61, 0x62, 0x63, 0x0C, 0x65,
    0x68, 0x69, 0x6A, 0x6B, 0x6C, 0x6D,
    0x70, 0x71, 0x72, 0x73, 0x74, 0x75,
    0x78, 0x79, 0x7A, 0x7B, 0x7C, 0x7D,
    0x80, 0x81, 0x82, 0x83, 0x84, 0x85,
    0x88, 0x89, 0x8A, 0x8B, 0x8C, 0x8D,
    0x90, 0x91, 0x92, 0xFF, 0x94, 0xFF,
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
];

/// Windmill start hue (0–127) of each matrix position, rows of 21 (firmware 0x9A9A, the data
/// of the C init record at 0x9A96 that fills XDATA 0x0E40): an angle around the board centre.
#[rustfmt::skip]
const WINDMILL: [u8; 126] = [
    0x51, 0x52, 0x53, 0x54, 0x55, 0x57, 0x59, 0x5C, 0x62, 0x69, 0x73, 0x78, 0x7C, 0x7F, 0x01, 0x03, 0x05, 0x06, 0x07, 0x08, 0x51,
    0x4F, 0x50, 0x50, 0x51, 0x52, 0x53, 0x55, 0x56, 0x5F, 0x6C, 0x78, 0x7F, 0x01, 0x03, 0x06, 0x07, 0x08, 0x09, 0x09, 0x0A, 0x4F,
    0x4D, 0x4D, 0x4E, 0x4E, 0x4F, 0x4F, 0x50, 0x52, 0x5A, 0x7D, 0x06, 0x08, 0x09, 0x09, 0x0A, 0x0B, 0x0B, 0x0B, 0x0C, 0x0C, 0x4D,
    0x49, 0x48, 0x47, 0x46, 0x45, 0x44, 0x43, 0x42, 0x34, 0x17, 0x11, 0x10, 0x0F, 0x0F, 0x0F, 0x0E, 0x0E, 0x0E, 0x0D, 0x0D, 0x4A,
    0x46, 0x45, 0x44, 0x43, 0x42, 0x40, 0x3D, 0x34, 0x27, 0x1B, 0x18, 0x16, 0x13, 0x13, 0x12, 0x12, 0x12, 0x11, 0x10, 0x0F, 0x47,
    0x45, 0x44, 0x42, 0x40, 0x3E, 0x36, 0x30, 0x29, 0x1F, 0x19, 0x17, 0x16, 0x15, 0x14, 0x14, 0x13, 0x12, 0x11, 0x11, 0x10, 0x46,
];

/// LED (column, row) that matrix position (`col`, `row`) drives.
fn led_at(row: usize, col: usize) -> (usize, usize) {
    let i = row * 21 + col;
    (MAP[i] as usize, MAP[0x7E + i] as usize)
}

/// Per-channel `c · num / den`, in integers as the firmware does (0x9B24).
fn scale(c: Rgb, num: u32, den: u32) -> Rgb {
    let s = |x: u8| (x as u32 * num / den) as u8;
    Rgb::new(s(c.r), s(c.g), s(c.b))
}

fn wheel(w: [u8; 3]) -> Rgb {
    Rgb::new(w[0], w[1], w[2])
}

/// Built-in effect settings as stored in the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub effect: u8,
    /// 0–4; 0 is dark.
    pub brightness: u8,
    /// 0 (slowest) – 4.
    pub speed: u8,
    /// 0–6 palette slot, 7 colourful.
    pub colour: u8,
}

impl Settings {
    fn colourful(&self) -> bool {
        self.colour >= 7
    }
    fn fixed(&self) -> Rgb {
        PALETTE[(self.colour as usize).min(6)]
    }
}

/// The firmware's `rand()` (0xA3F2): a 32-bit shift register stepped 16
/// times per call. Nothing seeds it; a zero state starts at 0xA5A50000.
struct Rand(u32);

impl Rand {
    fn next(&mut self) -> u16 {
        if self.0 == 0 {
            self.0 = 0xA5A5_0000;
        }
        for _ in 0..16 {
            let lsb = self.0 & 1;
            self.0 >>= 1;
            if lsb != 0 {
                self.0 ^= 0xCC4C_4ECE;
            }
        }
        (self.0 & 0x7FFF) as u16
    }
}

/// Real time per effect step in ms at speed 0–4. The firmware's period
/// table (in ms on its 1 ms tick) plus what the keyboard spends rendering:
/// most effects restart the period after drawing, and a full-board draw or
/// compose pass takes ≈25 ms on its 8051, so fast speeds bottom out at
/// whole main-loop passes. Fitted to stopwatch measurements of snake,
/// windmill and blossoming.
fn step_ms(effect: u8, speed: u8) -> f64 {
    // Computed from the user's 0x1400 firmware (identical lighting engine
    // to V14.2): table period plus render time, or whole ~33 ms compose
    // passes for the fading effects; checked against stopwatch timings.
    let t: [f64; 5] = match effect {
        1 => [58.0; 5],                          // fixed 40 + draw
        2 => [63.0, 53.0, 43.0, 33.0, 23.0],     // 0x2C65: 45 35 25 15 5
        3 => [59.0, 49.0, 39.0, 29.0, 20.0],     // 0x2C6A: 45 35 25 15 6
        4 => [101.0, 68.0, 68.0, 35.0, 35.0],    // 0x2C6F: 80 60 40 20 8
        5 => [134.0, 134.0, 101.0, 68.0, 35.0],  // 0x2C5B: 120 100 80 50 20
        7 => [101.0, 101.0, 68.0, 36.0, 36.0],   // 0x2C79: 88 68 48 28 8
        8 => [129.0, 129.0, 98.0, 66.0, 34.0],   // 0x2C5B
        10 => [132.0, 99.0, 99.0, 66.0, 33.0],   // 0x2C83: 120 90 70 45 1
        11 => [30.0, 24.0, 18.5, 18.5, 18.5],    // 0x2C88: 30 24 18 12 6 (counter reset before drawing)
        12 => [64.0, 56.0, 50.0, 48.0, 38.0],    // 0x2C8D: 32 24 18 16 6
        13 => [140.0, 107.0, 107.0, 74.0, 41.0], // 0x2C92: 115 95 75 55 30
        15 => [58.0, 50.0, 42.0, 34.0, 27.0],    // 0x2C97: 32 24 16 8 1
        16 => [46.0, 36.0, 26.0, 16.0, 15.3],    // 0x2C9C: 46 36 26 16 6 (reset before drawing)
        17 => [50.0, 40.0, 30.0, 20.0, 18.3],    // 0x2CA1: 50 40 30 20 8 (reset before drawing)
        _ => [100.0; 5],
    };
    t[speed.min(4) as usize]
}

/// Effects with a fade/compose pass of their own, and its real cadence in
/// ms. The firmware gates it at 16 ms (11 for raindrops, 3 for sine) but a
/// pass takes longer than that (≈33 ms on the 0x1400 firmware, from its
/// cycle count), so it runs on every main-loop pass; those effects' step
/// times are whole multiples of it.
fn compose_ms(effect: u8) -> Option<f64> {
    match effect {
        8 => Some(32.0),
        4 | 5 | 7 | 10 => Some(33.0),
        13 => Some(35.0),
        _ => None,
    }
}

/// A running preview.
pub struct Preview {
    settings: Settings,
    /// LED index and virtual-key code of each layout entry.
    leds: Vec<(u8, u8)>,
    /// Effect base colour per LED (XDATA 0x0424).
    base: [Rgb; LEDS],
    /// Effect intensity 0–32 per LED (0x0017); the windmill keeps its hues
    /// here.
    alpha: [u8; LEDS],
    /// What the LEDs show, brightness applied (0x0152).
    out: [Rgb; LEDS],
    /// Shared phase / step counter (0x08BF).
    phase: u8,
    /// Row bitmaps per matrix column moving up, down, left and right
    /// (0x08C2, 0x02E8, 0x013B, 0x0001): Flash away, Ripples, Sine wave.
    up: [u8; 21],
    down: [u8; 21],
    left: [u8; 21],
    right: [u8; 21],
    /// Sine wave: calls since the last reseed (0x08BA) and which edge comes
    /// next (b24.7).
    sine: (u8, bool),
    /// Stars: "rising" bit per row (0x0CF8) and the step counter (0x0EBF).
    rising: [u8; 21],
    stars: u8,
    /// Raindrops: row of the drop in each column, 0xFF none (0x0E1D).
    drops: [u8; 21],
    /// Snake head: row, column, moving right, moving down (0x0EC1–0x0EC4).
    snake: (i8, i8, bool, bool),
    /// Key presses waiting for the LED task, as LED (column, row); the
    /// firmware's queue holds 10.
    keys: VecDeque<(usize, usize)>,
    rand: Rand,
    /// Simulation clock and the times of the next step and compose pass, ms.
    now: f64,
    next_step: f64,
    next_compose: f64,
    /// Wall-clock time of the last frame.
    last_t: f64,
}

impl Preview {
    /// A preview of `settings` for the keys of `layout`.
    pub fn new(settings: Settings, layout: &[Led], t: f64) -> Self {
        let mut p = Self {
            settings,
            leds: layout.iter().map(|k| (k.0, k.4)).collect(),
            base: [Rgb::BLACK; LEDS],
            alpha: [0; LEDS],
            out: [Rgb::BLACK; LEDS],
            phase: 0,
            up: [0; 21],
            down: [0; 21],
            left: [0; 21],
            right: [0; 21],
            sine: (0, false),
            rising: [0; 21],
            stars: 0,
            drops: [0xFF; 21],
            snake: (0, 0, true, true),
            keys: VecDeque::new(),
            rand: Rand(0),
            now: 0.0,
            next_step: 0.0,
            next_compose: 0.0,
            last_t: t,
        };
        match settings.effect {
            // Sine wave (0xA2F8): one dot at matrix column 6, row 0.
            13 => {
                p.up[6] = 1;
                p.down[6] = 1;
                p.left[6] = 1;
                p.right[6] = 1;
            }
            // Windmill (0xA6FE): each position starts at its angle.
            15 => {
                for c in 0..21 {
                    for r in 0..6 {
                        p.alpha[c * 6 + r] = WINDMILL[r * 21 + c];
                    }
                }
            }
            _ => {}
        }
        p
    }

    pub fn settings(&self) -> Settings {
        self.settings
    }

    /// A key pressed while the preview is shown (by Windows virtual-key
    /// code). Only the reactive effects queue presses, as on the keyboard.
    pub fn key(&mut self, vk: u8) {
        if !reacts_to_keys(self.settings.effect) || self.keys.len() >= 10 {
            return;
        }
        let Some(&(led, _)) = self.leds.iter().find(|k| k.1 == vk) else { return };
        self.keys.push_back((led as usize / 6, led as usize % 6));
    }

    /// Advance to `t` seconds and return each layout key's colour, in
    /// layout order.
    pub fn frame(&mut self, t: f64) -> Vec<Rgb> {
        // Gaps (e.g. a hidden window) don't replay seconds of steps.
        let end = self.now + (t - self.last_t).clamp(0.0, 0.5) * 1000.0;
        self.last_t = t;
        let compose = compose_ms(self.settings.effect);
        loop {
            // The LED task takes at most one queued key per pass.
            self.take_key();
            let at_compose = if compose.is_some() { self.next_compose } else { f64::INFINITY };
            let at = self.next_step.min(at_compose);
            if at > end {
                break;
            }
            self.now = at;
            // The compose pass runs before the step in the same pass.
            if at_compose <= self.next_step {
                self.compose_pass();
                self.next_compose = at + compose.unwrap_or(f64::INFINITY);
            } else {
                self.step();
                self.next_step = at + step_ms(self.settings.effect, self.settings.speed);
            }
        }
        self.now = end;
        self.take_key();
        self.leds.iter().map(|&(i, _)| self.out.get(i as usize).copied().unwrap_or_default()).collect()
    }

    /// Change speed or brightness without restarting the animation (other
    /// changes need a new preview). Like the keyboard, the new brightness
    /// shows as the effect next draws each LED.
    pub fn adjust(&mut self, speed: u8, brightness: u8) {
        self.settings.speed = speed;
        self.settings.brightness = brightness;
    }

    // ---- firmware primitives --------------------------------------------

    fn bright(&self, c: Rgb) -> Rgb {
        scale(c, BRIGHTNESS[self.settings.brightness.min(4) as usize], 80)
    }

    fn set_led(&mut self, col: usize, row: usize, c: Rgb) {
        if col < 21 && row < 6 {
            self.out[col * 6 + row] = self.bright(c);
        }
    }

    /// Key hit (0x462B): a palette colour, or a random wheel colour when
    /// colourful, at full intensity, shown at once.
    fn hit(&mut self, row: usize, col: usize) {
        if col >= 21 || row >= 6 {
            return;
        }
        let c = if self.settings.colourful() { wheel(W192[self.rand.next() as usize % 192]) } else { self.settings.fixed() };
        let i = col * 6 + row;
        self.base[i] = c;
        self.alpha[i] = 32;
        self.set_led(col, row, c);
    }

    /// Compose (0x5F65): show base × intensity / 32 and fade by `fade`.
    fn compose(&mut self, fade: u8) {
        for col in 0..COLS {
            for row in 0..6 {
                let i = col * 6 + row;
                self.set_led(col, row, scale(self.base[i], self.alpha[i] as u32, 32));
                self.alpha[i] = self.alpha[i].saturating_sub(fade);
            }
        }
    }

    /// Phase stepper (0x9E98): count `phase` down by 1 (2 for neon and
    /// blossoming), wrapping to `n − 1`.
    fn phase_step(&mut self, n: u8) -> u8 {
        let d = if matches!(self.settings.effect, 11 | 17) { 2 } else { 1 };
        let p = self.phase.wrapping_sub(d);
        self.phase = if p >= n { n - 1 } else { p };
        self.phase
    }

    /// Feed one queued key press to the effect (LED task 0x165C). The
    /// firmware restarts the step and compose timers whenever it takes a
    /// key, so fast typing slows the animation down.
    fn take_key(&mut self) {
        let Some((col, row)) = self.keys.pop_front() else { return };
        let bit = 1u8 << row;
        match self.settings.effect {
            // The LED position is used as a matrix position and mapped
            // again when drawn, so on row 4 the light starts one key off,
            // as on the keyboard.
            4 => {
                self.left[col] |= bit;
                self.right[col] |= bit;
            }
            7 => {
                self.up[col] |= bit;
                self.down[col] |= bit;
                self.left[col] |= bit;
                self.right[col] |= bit;
            }
            // Reaction lights the key at once. The firmware also stops
            // fading a key while it is held (it masks it until release);
            // the preview only sees presses.
            //
            // Reading the LED task alone suggests the last key is re-hit on
            // every pass (nothing seems to clear the "key pending" flag
            // 0x0ED8), but the task's prologue (0x1108–0x111A) sets it to
            // 1 on every pass, so each press lights the key once and it
            // fades.
            12 => {
                let (lc, lr) = led_at(row, col);
                self.hit(lr, lc);
            }
            _ => {}
        }
        self.next_step = self.now + step_ms(self.settings.effect, self.settings.speed);
        if let Some(ms) = compose_ms(self.settings.effect) {
            self.next_compose = self.now + ms;
        }
    }

    fn compose_pass(&mut self) {
        match self.settings.effect {
            8 => self.compose_stars(),
            // Sine wave fades twice as fast (0x60A4).
            13 => self.compose(2),
            _ => self.compose(1),
        }
    }

    fn step(&mut self) {
        match self.settings.effect {
            1 => {
                self.phase = (self.phase + 1).min(11);
                self.rings(self.phase as usize, 0, false);
            }
            2 => self.breathe(),
            3 => self.rainbow(),
            4 | 7 | 13 => self.spread(),
            5 => self.rain(),
            8 => self.spawn_stars(),
            10 => self.snake(),
            11 => self.neon(),
            // Reaction's step is its fade.
            12 => self.compose(1),
            15 => self.windmill(),
            16 => self.waterfall(),
            17 => {
                let p = self.phase_step(192);
                self.rings(11, p, true);
            }
            // 6, 9, 14 and 18 have no firmware code: the keyboard is dark.
            _ => {}
        }
    }

    // ---- effects ----------------------------------------------------------

    /// Steady (n = 1…11 rings, growing every step: a bloom from the centre
    /// that then stays) and Blossoming (all 11, hue moving outward) —
    /// 0x6E62. Colourful rings are 13 wheel steps apart. With a fixed
    /// colour, Blossoming scales it by the ramp at 0x2640, but indexes it
    /// with the 0–191 wheel phase although the ramp is 96 bytes long, so
    /// half the cycle reads the grey-triplet table after it and the rings
    /// flick between near-black and ≈75 % (firmware bug, reproduced).
    fn rings(&mut self, n: usize, hue: u8, scaled: bool) {
        let mut h = hue as usize;
        for ring in RINGS.iter().take(n) {
            let c = if self.settings.colourful() {
                wheel(W192[h])
            } else if scaled {
                scale(self.settings.fixed(), RAMP[h] as u32, 255)
            } else {
                self.settings.fixed()
            };
            for &p in ring.iter().filter(|&&p| p != 0xFF) {
                let (lc, lr) = led_at((p & 7) as usize, (p >> 3) as usize);
                self.set_led(lc, lr, c);
            }
            h += 13;
            if h >= 192 {
                h -= 192;
            }
        }
    }

    /// Breathing (0x75CC): the 128-step envelope over the whole board.
    /// Colourful is a per-column gradient over wheel 0–108 with red and
    /// blue swapped: blue on the left through green to orange on the right,
    /// never red.
    fn breathe(&mut self) {
        let v = ENVELOPE[self.phase_step(128) as usize] as u32;
        for c in 0..COLS {
            let rgb = if self.settings.colourful() {
                let w = W192[6 * c];
                Rgb::new(w[2], w[1], w[0])
            } else {
                self.settings.fixed()
            };
            let rgb = scale(rgb, v, 255);
            for r in 0..6 {
                if KEYS[c * 6 + r] != 0xFF {
                    self.set_led(MAP[r * 21 + c] as usize, r, rgb);
                }
            }
        }
    }

    /// Rainbow (0x9854): the whole board one wheel colour, stepping
    /// R→M→B→C→G→Y; ignores the colour setting.
    fn rainbow(&mut self) {
        let rgb = wheel(W192[self.phase_step(192) as usize]);
        for &m in KEYS.iter().take(COLS * 6).filter(|&&m| m != 0xFF) {
            self.set_led((m >> 3) as usize, (m & 7) as usize, rgb);
        }
    }

    /// Flash away, Ripples and Sine wave (0x5622): hit every bitmap bit,
    /// move the bitmaps one key, and let vertical movers spawn horizontal
    /// ones. A Ripples press grows a one-key diamond front whose trail
    /// stays lit and fades (filled look); Flash away only runs left and
    /// right. Sine wave reseeds matrix column 6 every 6 steps, alternating
    /// row 0 (moving down) and row 5 (moving up): a triangle wave spreading
    /// sideways.
    fn spread(&mut self) {
        for c in 0..COLS {
            let m = self.up[c] | self.down[c] | self.left[c] | self.right[c];
            for r in 0..6 {
                if m & (1 << r) != 0 {
                    let (lc, lr) = led_at(r, c);
                    self.hit(lr, lc);
                }
            }
        }
        for c in 0..COLS {
            self.up[c] >>= 1;
            self.down[c] <<= 1;
            if c >= 1 {
                self.left[c - 1] = self.left[c];
            }
            if c < COLS - 1 {
                self.right[COLS - 1 - c] = self.right[COLS - 2 - c];
            }
        }
        self.left[COLS - 1] = 0;
        self.right[0] = 0;
        for c in 0..COLS {
            let v = self.up[c] | self.down[c];
            self.left[c] |= v;
            self.right[c] |= v;
        }
        if self.settings.effect == 13 {
            self.sine.0 += 1;
            if self.sine.0 >= 6 {
                self.sine.0 = 0;
                let bit = if self.sine.1 { 0x01 } else { 0x20 };
                self.sine.1 = !self.sine.1;
                self.up[6] |= bit;
                self.down[6] |= bit;
                self.left[6] |= bit;
                self.right[6] |= bit;
            }
        }
    }

    /// Raindrops (0x95DB): maybe start a drop in one random idle column,
    /// then move every drop down a row; drops leave fading tails.
    fn rain(&mut self) {
        let c0 = self.rand.next() as usize % COLS;
        if self.drops[c0] == 0xFF {
            self.drops[c0] = 0;
        }
        for c in 0..COLS {
            let r = self.drops[c];
            if r >= 6 {
                self.drops[c] = 0xFF;
            } else {
                let (lc, lr) = led_at(r as usize, c);
                self.hit(lr, lc);
                self.drops[c] = r + 1;
            }
        }
    }

    /// Stars twinkle spawn (0x5B02): five tries at a random dark position.
    /// The firmware draws the position modulo 132 from a 126-entry map, so
    /// some tries read past it into other tables; those name rows past 5,
    /// which only touch memory outside the LEDs (no visible effect), but
    /// they still use up random numbers.
    fn spawn_stars(&mut self) {
        self.stars = self.stars.wrapping_add(1);
        for _ in 0..5 {
            // Keil's 16-bit signed arithmetic: the sum can wrap negative.
            let x = self.rand.next().wrapping_add(self.stars as u16) as i16;
            let pos = (x % 132) as u8 as usize;
            let c = MAP[pos] as usize;
            if c >= COLS {
                continue;
            }
            let r = MAP[0x7E + pos] as usize;
            let h = self.rand.next() as usize % 192;
            if r >= 6 {
                continue;
            }
            let i = c * 6 + r;
            if self.alpha[i] != 0 || self.rising[c] & (1 << r) != 0 {
                continue;
            }
            self.rising[c] |= 1 << r;
            self.base[i] = if self.settings.colourful() { wheel(W192[h]) } else { self.settings.fixed() };
        }
    }

    /// Stars compose (0x4ECB): rising stars brighten to 32, then fade.
    fn compose_stars(&mut self) {
        for col in 0..COLS {
            for row in 0..6 {
                let i = col * 6 + row;
                if self.rising[col] & (1 << row) != 0 {
                    self.alpha[i] += 1;
                    if self.alpha[i] > 32 {
                        self.alpha[i] = 32;
                        self.rising[col] &= !(1 << row);
                    }
                } else if self.alpha[i] > 0 {
                    self.alpha[i] -= 1;
                }
                self.set_led(col, row, scale(self.base[i], self.alpha[i] as u32, 32));
            }
        }
    }

    /// Retro snake (0x8976): hit the head, then step along the row and at
    /// either end move one row down (or up), bouncing between rows 0 and 5:
    /// 19 positions × 10 rows = 190 steps per cycle.
    fn snake(&mut self) {
        let (row, col, right, down) = self.snake;
        let (lc, lr) = led_at(row as usize, col as usize);
        self.hit(lr, lc);
        let (mut row, mut col, mut right, mut down) = (row, col, right, down);
        let dy = if down { 1 } else { -1 };
        if right {
            col += 1;
            if col > 18 {
                col = 18;
                right = false;
                row += dy;
            }
        } else {
            col -= 1;
            if col < 0 {
                col = 0;
                right = true;
                row += dy;
            }
        }
        if row == 6 {
            down = !down;
            row = 4;
        }
        if row == -1 {
            down = !down;
            row = 1;
        }
        self.snake = (row, col, right, down);
    }

    /// Neon stream (0x6911): colourful is a rainbow scrolling left to right
    /// (7 wheel steps per column); a fixed colour is two brightness bands
    /// from the 96-step ramp (10 steps per column).
    fn neon(&mut self) {
        let colourful = self.settings.colourful();
        let mut p = self.phase_step(if colourful { 192 } else { 96 }) as usize;
        for c in 0..COLS {
            let rgb = if colourful {
                let w = wheel(W192[p]);
                p = (p + 7) % 192;
                w
            } else {
                let w = scale(self.settings.fixed(), RAMP[p] as u32, 255);
                p = (p + 10) % 96;
                w
            };
            for r in 0..6 {
                let (lc, lr) = led_at(r, c);
                if lc < COLS {
                    self.set_led(lc, lr, rgb);
                }
            }
        }
    }

    /// Rotating windmill (0x7F12): every position steps its hue on the
    /// 128-colour wheel, so the angular rainbow turns once per 128 steps;
    /// ignores the colour setting.
    fn windmill(&mut self) {
        for c in 0..COLS {
            for r in 0..6 {
                let i = c * 6 + r;
                // 0xAD91: one step back, wrapping to 127.
                let h = self.alpha[i].wrapping_sub(1);
                let h = if h >= 128 { 127 } else { h };
                self.alpha[i] = h;
                let (lc, lr) = led_at(r, c);
                if lc < COLS {
                    self.set_led(lc, lr, wheel(W128[h as usize]));
                }
            }
        }
    }

    /// Colorful waterfall (0x91BD): one wheel colour per row, 25 steps
    /// apart, flowing downward; always rainbow.
    fn waterfall(&mut self) {
        let mut p = self.phase_step(192) as usize;
        for row in 0..6 {
            let rgb = wheel(W192[p]);
            p = (p + 25) % 192;
            for col in 0..COLS {
                let m = KEYS[col * 6 + row];
                if m != 0xFF {
                    self.set_led((m >> 3) as usize, (m & 7) as usize, rgb);
                }
            }
        }
    }
}

/// Built-in effects that react to key presses.
pub fn reacts_to_keys(effect: u8) -> bool {
    matches!(effect, 4 | 7 | 12)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vendors::aula::layout::F99;

    const G: u8 = b'G';

    fn preview(effect: u8, speed: u8, colour: u8) -> Preview {
        Preview::new(Settings { effect, brightness: 4, speed, colour }, F99, 0.0)
    }

    /// Frames every `dt` s from `from` for `secs` s.
    fn run(p: &mut Preview, from: f64, secs: f64, dt: f64) -> Vec<Vec<Rgb>> {
        (1..=(secs / dt).round() as usize).map(|i| p.frame(from + i as f64 * dt)).collect()
    }

    fn lit(f: &[Rgb]) -> usize {
        f.iter().filter(|c| **c != Rgb::BLACK).count()
    }

    /// Layout position of the key with LED index `led`.
    fn at(led: u8) -> usize {
        F99.iter().position(|k| k.0 == led).unwrap()
    }

    #[test]
    fn tables_match_the_firmware() {
        assert_eq!(W192[0], [0xFF, 0x01, 0x00]);
        assert_eq!(W192[191], [0xFF, 0x00, 0x00]);
        assert_eq!(W128[0], [0x00, 0x05, 0xFF]);
        assert_eq!(ENVELOPE[63], 0xFF);
        // Row 4 swaps in the ISO key.
        assert_eq!(led_at(4, 1), (12, 4));
        assert_eq!(led_at(4, 2), (1, 4));
        // The firmware's generator (checked against 0xA3F2 in an emulator).
        let mut r = Rand(0);
        assert_eq!([r.next(), r.next(), r.next(), r.next()], [0x25A5, 0x63F6, 0x2D7F, 0x5523]);
        // The Steady/Blossoming rings reach every key.
        let ringed: Vec<usize> = RINGS.iter().flatten().filter(|&&p| p != 0xFF).map(|&p| led_at((p & 7) as usize, (p >> 3) as usize)).map(|(c, r)| c * 6 + r).collect();
        assert!(F99.iter().all(|k| ringed.contains(&(k.0 as usize))));
    }

    #[test]
    fn missing_effects_are_dark() {
        for effect in [0, 6, 9, 14, 18, 19, 32, 255] {
            let mut p = preview(effect, 2, 7);
            p.key(G);
            assert!(run(&mut p, 0.0, 2.0, 0.05).iter().all(|f| lit(f) == 0), "effect {effect}");
        }
    }

    #[test]
    fn every_effect_lights_and_brightness_zero_is_dark() {
        for effect in [1, 2, 3, 4, 5, 7, 8, 10, 11, 12, 13, 15, 16, 17] {
            for colour in [0, 7] {
                let mut p = preview(effect, 2, colour);
                p.key(G);
                assert!(run(&mut p, 0.0, 3.0, 0.05).iter().any(|f| lit(f) > 0), "effect {effect} colour {colour}");
                let mut p = Preview::new(Settings { effect, brightness: 0, speed: 2, colour }, F99, 0.0);
                p.key(G);
                assert!(run(&mut p, 0.0, 3.0, 0.05).iter().all(|f| lit(f) == 0), "effect {effect} at brightness 0");
            }
        }
    }

    #[test]
    fn steady_blooms_then_holds_the_palette_colour() {
        let mut p = Preview::new(Settings { effect: 1, brightness: 2, speed: 0, colour: 2 }, F99, 0.0);
        let first = p.frame(0.01);
        assert_eq!(lit(&first), 1); // ring 0: I
        assert_eq!(first[at(50)], Rgb::new(0, 0, 127));
        let f = run(&mut p, 0.01, 1.0, 0.05);
        assert!(f.last().unwrap().iter().all(|c| *c == Rgb::new(0, 0, 127)));
    }

    #[test]
    fn breathing_colourful_runs_blue_to_orange() {
        let mut p = preview(2, 0, 7);
        while p.phase != 63 {
            p.step();
        }
        // Firmware output at full envelope (emulated): left blue, right orange.
        assert_eq!(p.out[1], Rgb::new(0, 1, 255));
        assert_eq!(p.out[9 * 6 + 1], Rgb::new(0, 255, 0x1D));
        assert_eq!(p.out[18 * 6 + 1], Rgb::new(255, 104, 0));
        // 128 steps per breath, dark at the bottom.
        for _ in 0..64 {
            p.step();
        }
        assert_eq!(p.phase, 127);
        assert_eq!(lit(&p.out), 0);
    }

    #[test]
    fn snake_zig_zags_190_steps_down_and_up() {
        let mut p = preview(10, 0, 0);
        let mut path = Vec::new();
        for _ in 0..380 {
            path.push((p.snake.0, p.snake.1));
            p.snake();
        }
        assert_eq!(path[..190], path[190..]);
        let rows: Vec<i8> = path[..190].chunks(19).map(|c| c[0].0).collect();
        assert_eq!(rows, [0, 1, 2, 3, 4, 5, 4, 3, 2, 1]);
        assert_eq!(path[18], (0, 18));
        assert_eq!(path[19], (1, 18));
        assert_eq!(path[37], (1, 0));
        assert_eq!(path[38], (2, 0));
    }

    #[test]
    fn snake_leaves_a_fading_trail() {
        let mut p = preview(10, 4, 1);
        let f = run(&mut p, 0.0, 2.0, 0.01);
        let last = f.last().unwrap();
        assert!((20..=40).contains(&lit(last)), "{}", lit(last));
        assert!(last.iter().all(|c| c.r == 0 && c.b == 0));
    }

    /// Time of the 128th windmill step after the first.
    fn windmill_turn(speed: u8) -> f64 {
        let mut p = preview(15, speed, 0);
        let (mut t, mut steps, mut first) = (0.0, 0, None);
        let mut h = p.alpha[0];
        while steps < 129 {
            t += 0.005;
            p.frame(t);
            if p.alpha[0] != h {
                h = p.alpha[0];
                steps += 1;
                first.get_or_insert(t);
            }
        }
        assert_eq!(p.alpha[0], WINDMILL[0] - 1);
        t - first.unwrap()
    }

    #[test]
    fn windmill_turns_in_128_steps() {
        let slow = windmill_turn(0);
        let fast = windmill_turn(4);
        // 128 steps × 58 / 27 ms; the keyboard measured ≈7.5 s and ≈4 s.
        assert!((slow - 7.42).abs() < 0.1, "{slow}");
        assert!((fast - 3.46).abs() < 0.1, "{fast}");
    }

    #[test]
    fn windmill_matches_the_firmware() {
        let mut p = preview(15, 0, 3);
        p.step();
        // Emulated firmware output, row 0 and row 4 (ISO shift).
        assert_eq!(p.out[0], Rgb::new(0xFF, 0x9F, 0x00));
        assert_eq!(p.out[9 * 6], Rgb::new(0xFF, 0x00, 0xEC));
        assert_eq!(p.out[13 * 6], Rgb::new(0x10, 0x00, 0xFF));
        assert_eq!(p.out[12 * 6 + 4], Rgb::new(0xF7, 0xFF, 0x00));
    }

    #[test]
    fn blossoming_reaches_the_edge_in_65_steps() {
        let mut p = preview(17, 0, 7);
        let f = p.frame(0.001);
        let centre = f[at(50)];
        let mut t = 0.001;
        loop {
            t += 0.002;
            if p.frame(t)[at(0)] == centre {
                break;
            }
        }
        // 65 steps × 50 ms; the keyboard measured ≈3 s.
        assert!((t - 3.25).abs() < 0.1, "{t}");
    }

    #[test]
    fn blossoming_single_colour_ramp_bug() {
        let mut p = preview(17, 0, 1);
        for _ in 0..3 {
            p.step();
        }
        // Emulated firmware: ring levels 4C 2D 63 AA FF B0 68 31 BF BC B4.
        let rings: Vec<u8> = [(8, 2), (8, 1), (7, 0), (6, 0), (5, 0), (3, 2), (4, 0), (3, 0), (2, 0), (0, 1), (0, 0)].iter().map(|&(c, r)| p.out[c * 6 + r].g).collect();
        assert_eq!(rings, [0x4C, 0x2D, 0x63, 0xAA, 0xFF, 0xB0, 0x68, 0x31, 0xBF, 0xBC, 0xB4]);
    }

    #[test]
    fn neon_and_waterfall_match_the_firmware() {
        let mut p = preview(11, 0, 3);
        p.step();
        p.step();
        assert_eq!(p.phase, 93);
        assert_eq!([p.out[0], p.out[6], p.out[12]], [Rgb::new(0x1B, 0x1B, 0), Rgb::new(0x2A, 0x2A, 0), Rgb::new(0x51, 0x51, 0)]);
        let mut p = preview(16, 0, 1);
        p.step();
        let col0: Vec<Rgb> = p.out[..6].to_vec();
        assert_eq!(col0[0], Rgb::new(0xFF, 0, 0));
        assert_eq!(col0[1], Rgb::new(0xFF, 0xA4, 0));
        assert_eq!(col0[5], Rgb::new(0, 0x06, 0xFF));
    }

    #[test]
    fn reactive_effects_wait_for_keys() {
        for effect in [4, 7, 12] {
            let mut p = preview(effect, 2, 0);
            assert!(run(&mut p, 0.0, 1.0, 0.05).iter().all(|f| lit(f) == 0), "effect {effect} lit without a key");
            p.key(G);
            assert!(run(&mut p, 1.0, 0.3, 0.05).iter().any(|f| lit(f) > 0), "effect {effect} ignored the key");
        }
    }

    #[test]
    fn reaction_lights_the_key_once_then_fades() {
        let mut p = preview(12, 0, 1);
        p.key(G);
        let f = p.frame(0.01);
        assert_eq!(lit(&f), 1);
        assert_eq!(f[at(33)], Rgb::new(0, 255, 0));
        // 32 fade steps of ≈60 ms.
        let f = run(&mut p, 0.01, 2.5, 0.05);
        assert!(lit(&f[20]) == 1 && f[20][at(33)].g < 255);
        assert_eq!(lit(f.last().unwrap()), 0);
    }

    #[test]
    fn ripple_fills_then_fades_out() {
        let mut p = preview(7, 4, 0);
        p.key(G);
        let counts: Vec<usize> = run(&mut p, 0.0, 3.0, 0.01).iter().map(|f| lit(f)).collect();
        assert!(counts.iter().any(|&n| n > 5 && n < 50), "never partly spread");
        assert!(counts.iter().any(|&n| n >= 95), "never filled: {:?}", counts.iter().max());
        assert_eq!(*counts.last().unwrap(), 0, "never faded out");
    }

    #[test]
    fn flash_away_stays_on_its_row() {
        let mut p = preview(4, 4, 0);
        p.key(G);
        let f = run(&mut p, 0.0, 0.6, 0.01);
        let rows: Vec<u8> = f.iter().flat_map(|f| F99.iter().zip(f).filter(|(_, c)| **c != Rgb::BLACK).map(|(k, _)| k.0 % 6)).collect();
        assert!(!rows.is_empty() && rows.iter().all(|&r| r == 3));
    }

    #[test]
    fn sine_traces_a_triangle_from_column_6() {
        let mut p = preview(13, 0, 0);
        let mut rows = Vec::new();
        for _ in 0..24 {
            rows.push(((p.up[6] | p.down[6]) & 0x3F).trailing_zeros());
            p.spread();
        }
        assert_eq!(rows, [0, 1, 2, 3, 4, 5, 5, 4, 3, 2, 1, 0, 0, 1, 2, 3, 4, 5, 5, 4, 3, 2, 1, 0]);
        // At speed 4 the fades overlap: most keys stay lit.
        let mut p = preview(13, 4, 0);
        let f = run(&mut p, 0.0, 3.0, 0.01);
        assert!(lit(f.last().unwrap()) > 60, "{}", lit(f.last().unwrap()));
    }

    #[test]
    fn stars_get_denser_with_speed() {
        let avg = |speed| {
            let mut p = preview(8, speed, 7);
            let f = run(&mut p, 0.0, 6.0, 0.05);
            f[80..].iter().map(|f| lit(f)).sum::<usize>() / f[80..].len()
        };
        let (slow, mid, fast) = (avg(0), avg(2), avg(4));
        // A try on a lit key is wasted, so the board settles at about a
        // third of the keys at speed 2 and two thirds at speed 4.
        assert!(mid > 30 && fast > 55, "{mid} {fast}");
        assert!(slow < mid && mid < fast, "{slow} {mid} {fast}");
    }

    #[test]
    fn raindrops_fall_down_columns() {
        let mut p = preview(5, 2, 0);
        let f = run(&mut p, 0.0, 3.0, 0.05);
        assert!(f[20..].iter().all(|f| lit(f) > 3));
    }
}
