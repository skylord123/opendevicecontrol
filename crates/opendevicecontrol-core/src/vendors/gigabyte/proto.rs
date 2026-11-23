//! Gigabyte RGB Fusion 2 over USB: the ITE LED controller (`048d:8297`,
//! `048d:5702`, `048d:5711`, `048d:8950`) on Gigabyte motherboards.
//!
//! Everything is a 64-byte feature report with report ID 0xCC on the
//! vendor collection (usage page 0xFF89, usage 0xCC): `cc cmd args...`.
//! Only the info commands answer, through a feature-report read.
//!
//! | Command | Request | Meaning |
//! |---|---|---|
//! | `20`–`27`, `90`–`92` | effect block (see [`effect`]) | set one LED zone's built-in effect |
//! | `28` | `mask32` | apply the effects set since the last apply |
//! | `31` | `0`/`1` | audio "beat" mode off/on |
//! | `32` | `mask` | addressable headers whose built-in effect is off (direct colours) |
//! | `34` | `n1n0 n3n2 n5n4` | addressable header lengths (one nibble each) |
//! | `48` | `0`/`1` | Windows Dynamic Lighting (lamp array) off/on |
//! | `58`, `59`, `62`, `63` | `off16 bytes rgb...` | direct colours for one header |
//! | `60` | | info report: firmware, name, lengths, colour order |
//! | `61` | | IT5711 only: colour order of headers 3–6 |
//!
//! Never sent: `33` (write the colour-order calibration), `47` / `5e`
//! (lighting persistence, save to flash), `38`–`3d` (Gen2 strip scan) and
//! anything belonging to the firmware updater.

use crate::model::Rgb;

pub const VID: u16 = 0x048d;
/// IT8297 (X570, Z390), IT5702 (Z490, B550), IT5711 (Z890, X870), IT82950 (H810).
pub const PIDS: [u16; 4] = [0x8297, 0x5702, 0x5711, 0x8950];
pub const USAGE_PAGE: u16 = 0xff89;
pub const USAGE: u16 = 0xcc;

pub const REPORT_ID: u8 = 0xcc;
/// Bytes per feature report, report ID included.
pub const LEN: usize = 64;

pub const CMD_APPLY: u8 = 0x28;
pub const CMD_BEAT: u8 = 0x31;
pub const CMD_DIRECT_MASK: u8 = 0x32;
pub const CMD_LED_COUNTS: u8 = 0x34;
pub const CMD_LAMP_ARRAY: u8 = 0x48;
pub const CMD_INFO: u8 = 0x60;
pub const CMD_INFO2: u8 = 0x61;

/// Info flag: the controller has a lamp array (Dynamic Lighting) mode.
pub const FLAG_LAMP_ARRAY: u8 = 0x02;

/// RGB triples in one direct-colour report.
const DIRECT_LEDS: usize = 19;

/// An addressable (D_LED / ARGB) header.
#[derive(Debug, PartialEq, Eq)]
pub struct Header {
    pub name: &'static str,
    /// LED index its built-in effect is set on.
    pub led: u8,
    /// Command its direct colours are sent with.
    pub direct: u8,
    /// Its bit in the `32` mask.
    pub bit: u8,
    /// Its slot in the `34` lengths and in the calibration.
    pub slot: usize,
}

const fn hdr(name: &'static str, led: u8, direct: u8, bit: u8, slot: usize) -> Header {
    Header { name, led, direct, bit, slot }
}

const IT8297_HEADERS: &[Header] = &[hdr("D_LED1", 5, 0x58, 0x01, 0), hdr("D_LED2", 6, 0x59, 0x02, 1)];
const IT5711_HEADERS: &[Header] = &[
    hdr("ARGB_V2_1", 5, 0x58, 0x01, 0),
    hdr("ARGB_V2_2", 6, 0x59, 0x02, 1),
    hdr("ARGB_V2_3", 7, 0x62, 0x08, 2),
    hdr("ARGB_V2_4", 8, 0x63, 0x10, 3),
];
/// The IT82950 drives its two headers from LED indices 3 and 4.
const IT8950_HEADERS: &[Header] = &[hdr("ARGB_V2_1", 2, 0x58, 0x01, 0), hdr("ARGB_V2_2", 3, 0x59, 0x02, 1)];

/// What a controller drives: its addressable headers, the single-colour
/// LED indices (0-based; "LED 1" is index 0), the longest strip a header
/// takes and the built-in effects it has.
#[derive(Debug, PartialEq, Eq)]
pub struct Layout {
    pub headers: &'static [Header],
    pub singles: &'static [u8],
    pub max_leds: u16,
    pub modes: &'static [Kind],
}

const MODES_ALL: &[Kind] = &[Kind::Off, Kind::Static, Kind::Breathing, Kind::Flash, Kind::DoubleFlash, Kind::ColorCycle, Kind::Random];
const MODES_NO_RANDOM: &[Kind] = &[Kind::Off, Kind::Static, Kind::Breathing, Kind::Flash, Kind::DoubleFlash, Kind::ColorCycle];

/// The generic layout of controller `pid`. `device_num` is 1 for the second
/// controller of boards that have two, which drives single-colour LEDs only.
/// Which LED index lights what differs from board to board.
pub fn layout(pid: u16, device_num: u8) -> Layout {
    let second = device_num == 1;
    match pid {
        0x5711 if second => Layout { headers: &[], singles: &[0, 1, 2, 3, 4, 8, 9, 10], max_leds: 256, modes: MODES_ALL },
        // Indices 9 and 10 are onboard addressable strips of board-specific
        // length; they are left alone.
        0x5711 => Layout { headers: IT5711_HEADERS, singles: &[0, 1, 2, 3, 4], max_leds: 256, modes: MODES_ALL },
        _ if second => Layout { headers: &[], singles: &[0, 1, 2, 3, 4, 5, 6, 7], max_leds: 256, modes: MODES_ALL },
        0x8950 => Layout { headers: IT8950_HEADERS, singles: &[], max_leds: 256, modes: MODES_NO_RANDOM },
        0x8297 => Layout { headers: IT8297_HEADERS, singles: &[0, 1, 2, 3, 4, 7], max_leds: 1024, modes: MODES_ALL },
        _ => Layout { headers: IT8297_HEADERS, singles: &[0, 1, 2, 3, 4, 7], max_leds: 256, modes: MODES_ALL },
    }
}

/// Byte positions of red, green and blue within one LED of a direct-colour
/// report.
pub type Order = [usize; 3];

/// RGB on the wire; used when a header's calibration is blank or invalid.
pub const DEFAULT_ORDER: Order = [0, 1, 2];

/// A calibration word (`b g r 00`: the position of each colour) as an
/// [`Order`]; None when blank or not a permutation.
pub fn decode_order(cal: u32) -> Option<Order> {
    let [b, g, r, reserved] = cal.to_le_bytes();
    let (r, g, b) = (r as usize, g as usize, b as usize);
    (reserved == 0 && r < 3 && g < 3 && b < 3 && r != g && r != b && g != b).then_some([r, g, b])
}

/// "GRB" and so on: the colours in wire order.
pub fn order_name(o: Order) -> String {
    let mut s = [b'?'; 3];
    for (pos, c) in o.iter().zip(*b"RGB") {
        s[*pos] = c;
    }
    String::from_utf8_lossy(&s).into_owned()
}

/// The info report (`60`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub product: u8,
    /// 0 for the first controller, 1 for the second on dual-controller boards.
    pub device_num: u8,
    pub strip_detect: u8,
    pub firmware: [u8; 4],
    /// Length class of each header slot (see [`count_class`]).
    pub counts: [u8; 6],
    pub flags: u8,
    /// e.g. `IT5702-GIGABYTE V1.0.10.0`.
    pub name: String,
    /// Colour-order calibration of header slots 0–5 (2–5 from `61`).
    pub cal: [u32; 6],
    pub chip_id: u32,
}

fn u32_at(r: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]])
}

/// The reply to `60`. The IT8297 answers with 0 in place of the report ID.
pub fn parse_info(r: &[u8]) -> Option<Info> {
    if r.len() < LEN || !matches!(r[0], REPORT_ID | 0) {
        return None;
    }
    let name = &r[12..40];
    let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
    Some(Info {
        product: r[1],
        device_num: r[2],
        strip_detect: r[3],
        firmware: [r[4], r[5], r[6], r[7]],
        counts: [r[8] & 0x0f, r[8] >> 4, r[9] & 0x0f, r[9] >> 4, r[10] & 0x0f, r[10] >> 4],
        flags: r[11],
        name: String::from_utf8_lossy(name).trim().to_string(),
        cal: [u32_at(r, 44), u32_at(r, 48), 0, 0, 0, 0],
        chip_id: u32_at(r, 56),
    })
}

/// The reply to `61` (IT5711): calibration of header slots 2–5.
pub fn parse_info2(r: &[u8]) -> Option<[u32; 4]> {
    (r.len() >= LEN && matches!(r[0], REPORT_ID | 0)).then(|| [u32_at(r, 4), u32_at(r, 8), u32_at(r, 12), u32_at(r, 16)])
}

impl Info {
    pub fn firmware_string(&self) -> String {
        let [a, b, c, d] = self.firmware;
        format!("{a}.{b}.{c}.{d}")
    }
}

/// The length class the controller stores for a header of `n` LEDs.
pub fn count_class(n: u16) -> u8 {
    match n {
        0..=32 => 0,
        33..=64 => 1,
        65..=256 => 2,
        257..=512 => 3,
        _ => 4,
    }
}

/// A command with up to three argument bytes.
pub fn command(cmd: u8, args: &[u8]) -> [u8; LEN] {
    let mut b = [0u8; LEN];
    b[0] = REPORT_ID;
    b[1] = cmd;
    b[2..2 + args.len()].copy_from_slice(args);
    b
}

/// `34`: the length class of all six header slots.
pub fn led_counts(classes: &[u8; 6]) -> [u8; LEN] {
    let [a, b, c, d, e, f] = classes.map(|n| n & 0x0f);
    command(CMD_LED_COUNTS, &[(b << 4) | a, (d << 4) | c, (f << 4) | e])
}

/// `28`: apply the effects of the LED indices in `mask`.
pub fn apply(mask: u32) -> [u8; LEN] {
    command(CMD_APPLY, &mask.to_le_bytes())
}

/// Direct colours for one header, as reports of up to 19 LEDs.
pub fn direct(cmd: u8, order: Order, colors: &[Rgb]) -> Vec<[u8; LEN]> {
    let (chunks, rest) = colors.as_chunks::<DIRECT_LEDS>();
    chunks
        .iter()
        .map(|c| c.as_slice())
        .chain((!rest.is_empty()).then_some(rest))
        .enumerate()
        .map(|(i, chunk)| {
            let mut b = command(cmd, &((i * DIRECT_LEDS * 3) as u16).to_le_bytes());
            b[4] = (chunk.len() * 3) as u8;
            for (j, c) in chunk.iter().enumerate() {
                let o = 5 + j * 3;
                b[o + order[0]] = c.r;
                b[o + order[1]] = c.g;
                b[o + order[2]] = c.b;
            }
            b
        })
        .collect()
}

/// The built-in effects (as Gigabyte's RGB Fusion names them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Off,
    Static,
    Breathing,
    Flash,
    DoubleFlash,
    ColorCycle,
    Random,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Off => "Off",
            Kind::Static => "Static",
            Kind::Breathing => "Breathing",
            Kind::Flash => "Flash",
            Kind::DoubleFlash => "Double flash",
            Kind::ColorCycle => "Color cycle",
            Kind::Random => "Random",
        }
    }

    pub fn uses_color(self) -> bool {
        matches!(self, Kind::Static | Kind::Breathing | Kind::Flash | Kind::DoubleFlash)
    }

    pub fn uses_speed(self) -> bool {
        matches!(self, Kind::Breathing | Kind::Flash | Kind::DoubleFlash | Kind::ColorCycle)
    }
}

/// Speed steps as Gigabyte's software has them: 0 is fastest, 9 slowest.
pub const SPEED_STEPS: u8 = 9;

/// One zone's built-in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effect {
    pub kind: Kind,
    pub color: Rgb,
    /// 0 (fastest) to [`SPEED_STEPS`].
    pub speed: u8,
    /// 0–100 %.
    pub brightness: u8,
}

impl Default for Effect {
    fn default() -> Self {
        Effect { kind: Kind::Static, color: Rgb::WHITE, speed: 4, brightness: 100 }
    }
}

impl Effect {
    /// A static colour, for direct colours on single-colour LEDs.
    pub fn fixed(color: Rgb) -> Self {
        Effect { kind: Kind::Static, color, speed: 0, brightness: 100 }
    }

    /// The (fade in, fade out, hold, extra) periods in ms, and the four
    /// effect parameters.
    pub fn timing(&self) -> ([u16; 4], [u8; 4]) {
        let s = self.speed.min(SPEED_STEPS) as u16;
        match self.kind {
            Kind::Off | Kind::Static => ([0; 4], [0; 4]),
            Kind::Breathing => {
                let p = if s <= 6 { 400 + s * 100 } else { 1000 + (s - 6) * 200 };
                ([p, p, 200, 0], [0; 4])
            }
            // Parameter 1 turns on pulsing, parameter 2 is the flashes per cycle.
            Kind::Flash => ([100, 100, 700 + s * 200, 0], [0, 1, 1, 0]),
            Kind::DoubleFlash => ([100, 100, 800 + s * 200, 0], [0, 1, 2, 0]),
            Kind::ColorCycle => {
                let p = 300 + s * 100 + if s > 8 { 1300 * (s - 8) } else { 0 };
                ([p, p - 200, 0, 0], [7, 0, 0, 0])
            }
            Kind::Random => ([100, 0, 0, 0], [1, 5, 0, 0]),
        }
    }

    /// The maximum-brightness byte. Breathing misbehaves above 100.
    pub fn brightness_byte(&self) -> u8 {
        let pct = self.brightness.min(100) as u16;
        match self.kind {
            Kind::Off => 0,
            Kind::Breathing => pct as u8,
            _ => (pct * 255 / 100) as u8,
        }
    }
}

/// The command that sets the effect of LED index `led` (0–10).
pub fn effect_cmd(led: u8) -> u8 {
    if led < 8 { 0x20 + led } else { 0x90 + led - 8 }
}

/// `20`+: set the built-in effect of LED index `led`:
/// `cc cmd zone0:u32 zone1:u32 00 type max min color0:u32 color1:u32
/// period0..3:u16 param0..3`, colours as `b g r 00`.
pub fn effect(led: u8, e: &Effect) -> [u8; LEN] {
    let mut b = command(effect_cmd(led), &[]);
    b[2..6].copy_from_slice(&(1u32 << led).to_le_bytes());
    b[11] = match e.kind {
        Kind::Off | Kind::Static => 1,
        Kind::Breathing => 2,
        Kind::Flash | Kind::DoubleFlash => 3,
        Kind::ColorCycle => 4,
        Kind::Random => 8,
    };
    b[12] = e.brightness_byte();
    if e.kind != Kind::Off {
        b[14..17].copy_from_slice(&[e.color.b, e.color.g, e.color.r]);
    }
    let (periods, params) = e.timing();
    for (i, p) in periods.iter().enumerate() {
        b[22 + i * 2..24 + i * 2].copy_from_slice(&p.to_le_bytes());
    }
    b[30..34].copy_from_slice(&params);
    b
}

/// A 0–100 % speed slider (100 fastest) to a speed step, and back.
pub fn speed_step(percent: f64) -> u8 {
    ((100.0 - percent.clamp(0.0, 100.0)) / 100.0 * SPEED_STEPS as f64).round() as u8
}

pub fn speed_percent(step: u8) -> f64 {
    ((SPEED_STEPS - step.min(SPEED_STEPS)) as f64 / SPEED_STEPS as f64 * 100.0).round()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info_report() -> Vec<u8> {
        let mut r = vec![0u8; LEN];
        r[0] = REPORT_ID;
        r[1] = 1;
        r[4..8].copy_from_slice(&[1, 0, 10, 0]);
        r[8] = 0x21; // D_LED1: up to 64, D_LED2: up to 256
        r[11] = 0x01;
        let name = b"IT5702-GIGABYTE V1.0.10.0";
        r[12..12 + name.len()].copy_from_slice(name);
        r[44..48].copy_from_slice(&[0x02, 0x00, 0x01, 0x00]); // GRB
        r[48..52].copy_from_slice(&[0x02, 0x01, 0x00, 0x00]); // RGB
        r[56..60].copy_from_slice(&0x5702_0001u32.to_le_bytes());
        r
    }

    #[test]
    fn info_report_fields() {
        let i = parse_info(&info_report()).unwrap();
        assert_eq!(i.name, "IT5702-GIGABYTE V1.0.10.0");
        assert_eq!(i.firmware_string(), "1.0.10.0");
        assert_eq!((i.product, i.device_num, i.flags), (1, 0, 1));
        assert_eq!(i.counts, [1, 2, 0, 0, 0, 0]);
        assert_eq!(decode_order(i.cal[0]).map(order_name).as_deref(), Some("GRB"));
        assert_eq!(decode_order(i.cal[1]).map(order_name).as_deref(), Some("RGB"));
        assert_eq!(i.chip_id, 0x5702_0001);
        // The IT8297 puts 0 where the report ID goes.
        let mut r = info_report();
        r[0] = 0;
        assert!(parse_info(&r).is_some());
        r[0] = 0x5a;
        assert!(parse_info(&r).is_none());
        assert!(parse_info(&info_report()[..40]).is_none());
    }

    #[test]
    fn calibration_must_be_a_permutation() {
        assert_eq!(decode_order(0), None);
        assert_eq!(decode_order(0x0001_0101), None);
        assert_eq!(decode_order(0x0100_0102), None);
        // b g r 00: BGR is b at 0, g at 1, r at 2.
        assert_eq!(decode_order(0x0002_0100), Some([2, 1, 0]));
        assert_eq!(order_name([2, 1, 0]), "BGR");
    }

    #[test]
    fn static_effect_layout() {
        let e = Effect { kind: Kind::Static, color: Rgb::new(0x35, 0x00, 0x17), speed: 4, brightness: 100 };
        let b = effect(1, &e);
        assert_eq!(&b[..17], &[0xcc, 0x21, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0xff, 0, 0x17, 0x00, 0x35]);
        assert!(b[17..].iter().all(|&x| x == 0));
        // The fourth header of an IT5711 is LED index 8.
        let b = effect(8, &e);
        assert_eq!(&b[..6], &[0xcc, 0x90, 0, 1, 0, 0]);
    }

    #[test]
    fn timed_effects() {
        let e = Effect { kind: Kind::Breathing, speed: 4, brightness: 100, ..Effect::default() };
        let b = effect(0, &e);
        assert_eq!(b[12], 100);
        assert_eq!(&b[22..30], &[0x20, 0x03, 0x20, 0x03, 200, 0, 0, 0]); // 800, 800, 200
        let e = Effect { kind: Kind::DoubleFlash, speed: 0, ..Effect::default() };
        let b = effect(0, &e);
        assert_eq!((b[11], &b[30..34]), (3, &[0, 1, 2, 0][..]));
        let e = Effect { kind: Kind::ColorCycle, speed: 9, ..Effect::default() };
        assert_eq!(e.timing().0[..2], [2500, 2300]);
        let off = effect(3, &Effect { kind: Kind::Off, ..Effect::default() });
        assert_eq!((off[11], off[12], off[14]), (1, 0, 0));
    }

    #[test]
    fn direct_reports() {
        let colors: Vec<Rgb> = (0..40).map(|i| Rgb::new(i, 100, 200)).collect();
        let r = direct(0x58, [1, 0, 2], &colors);
        assert_eq!(r.len(), 3); // 19 + 19 + 2
        assert_eq!(&r[0][..8], &[0xcc, 0x58, 0, 0, 57, 100, 0, 200]);
        assert_eq!(&r[1][..5], &[0xcc, 0x58, 57, 0, 57]);
        assert_eq!(&r[2][..11], &[0xcc, 0x58, 114, 0, 6, 100, 38, 200, 100, 39, 200]);
        assert!(r[2][11..].iter().all(|&x| x == 0));
        assert!(direct(0x59, DEFAULT_ORDER, &[]).is_empty());
    }

    #[test]
    fn small_commands() {
        assert_eq!(&led_counts(&[1, 2, 0, 0, 4, 3])[..6], &[0xcc, 0x34, 0x21, 0x00, 0x34, 0]);
        assert_eq!(&apply(0xff)[..7], &[0xcc, 0x28, 0xff, 0, 0, 0, 0]);
        assert_eq!(&command(CMD_DIRECT_MASK, &[0x03])[..4], &[0xcc, 0x32, 0x03, 0]);
        assert_eq!(count_class(0), 0);
        assert_eq!(count_class(33), 1);
        assert_eq!(count_class(256), 2);
        assert_eq!(count_class(1024), 4);
    }

    #[test]
    fn speed_scale() {
        assert_eq!(speed_step(100.0), 0);
        assert_eq!(speed_step(0.0), 9);
        assert_eq!(speed_step(speed_percent(4)), 4);
    }

    #[test]
    fn layouts() {
        let l = layout(0x5702, 0);
        assert_eq!((l.headers.len(), l.singles.len(), l.max_leds), (2, 6, 256));
        assert_eq!(layout(0x8297, 0).max_leds, 1024);
        assert!(layout(0x8297, 1).headers.is_empty());
        assert_eq!(layout(0x5711, 0).headers.len(), 4);
        assert!(!layout(0x8950, 0).modes.contains(&Kind::Random));
        // No single LED shares an index with a header.
        for pid in PIDS {
            let l = layout(pid, 0);
            assert!(l.singles.iter().all(|s| l.headers.iter().all(|h| h.led != *s)));
        }
    }
}
