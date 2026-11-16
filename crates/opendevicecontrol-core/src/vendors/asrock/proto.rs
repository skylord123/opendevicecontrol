//! ASRock Polychrome RGB over USB (`26ce:01a2`, the Nuvoton LED controller
//! on recent ASRock boards), as ASRock's Polychrome RGB utility drives it.
//!
//! Every message is a 64-byte output report (no report IDs, so 65 bytes
//! with the leading zero): `cmd 00 a b ...`. Only the read commands (0x11,
//! 0x14, 0xA6) answer, with a 64-byte input report `a6 00 00 a data...`.
//!
//! | Command | Request | Meaning |
//! |---|---|---|
//! | `10` | `ch mode r g b speed ff sync 00 [gradient]` | set a channel's effect |
//! | `10` | `ff e3 00 00 nlo nhi rgb...` then `ff e4 rgb...` | direct colours, all channels |
//! | `11` | `ch` | read a channel's effect |
//! | `12` | | save the current effects to flash |
//! | `14` | `01` / `02` / `03` / `0a ch` | read channel mask / LED counts / R-G swap / gradient |
//! | `15` | `02 n0..n7` / `03 mask` | write LED counts / R-G swap |
//! | `a6` | | firmware version |
//!
//! Command 0x30 and the ISP commands belong to the firmware updater and are
//! never sent.

use crate::model::Rgb;

pub const VID: u16 = 0x26ce;
pub const PID: u16 = 0x01a2;

/// Bytes per report, without the report ID.
pub const LEN: usize = 64;
/// Channels the controller has (not all exist on every board).
pub const CHANNELS: usize = 8;

pub const CMD_SET: u8 = 0x10;
pub const CMD_READ: u8 = 0x11;
pub const CMD_SAVE: u8 = 0x12;
pub const CMD_QUERY: u8 = 0x14;
pub const CMD_CONFIG: u8 = 0x15;
pub const CMD_VERSION: u8 = 0xa6;

pub const SUB_CHANNELS: u8 = 0x01;
pub const SUB_LED_COUNTS: u8 = 0x02;
pub const SUB_RG_SWAP: u8 = 0x03;
pub const SUB_GRADIENT: u8 = 0x0a;

/// Direct-colour modes: stream start, stream continuation. The firmware
/// also reports 0xE1 / 0xE2 for channels left in a direct state.
const DIRECT_FIRST: u8 = 0xe3;
const DIRECT_NEXT: u8 = 0xe4;
pub const DIRECT_STATES: [u8; 3] = [0xe1, 0xe2, 0xe3];
/// RGB triples in the first and following direct-colour packets.
const FIRST_LEDS: usize = 18;
const NEXT_LEDS: usize = 19;

/// The zones behind each channel, named as ASRock's utility names them.
pub const CHANNEL_NAMES: [&str; CHANNELS] = [
    "RGB LED 1",
    "RGB LED 2",
    "Addressable RGB LED 1",
    "Addressable RGB LED 2",
    "PCH heatsink",
    "I/O cover",
    "PCB",
    "Addressable RGB LED 3",
];

/// Whether a channel is an addressable header (its LED count and red/green
/// order are set by the user).
pub fn addressable(ch: usize) -> bool {
    matches!(ch, 2 | 3 | 7)
}

/// A built-in effect: (mode byte, name, uses the colour, uses the speed).
pub type Mode = (u8, &'static str, bool, bool);

pub const MODES: &[Mode] = &[
    (0x00, "Off", false, false),
    (0x01, "Static", true, false),
    (0x02, "Breathing", true, true),
    (0x03, "Strobe", true, true),
    (0x04, "Color cycle", false, true),
    (0x05, "Random", false, true),
    (0x06, "Music", false, false),
    (0x07, "Wave", false, true),
    (0x08, "Spring", true, true),
    (0x09, "Stack", true, true),
    (0x0a, "Cram", true, true),
    (0x0b, "Scan", true, true),
    (0x0c, "Neon", true, true),
    (0x0d, "Water", false, true),
    (0x0e, "Rainbow", false, true),
    (0x0f, "Rainbow 2", false, true),
    (0x10, "Glow stream", false, true),
    (0x11, "Luminous tide", false, true),
];

pub fn mode(byte: u8) -> Option<&'static Mode> {
    MODES.iter().find(|m| m.0 == byte)
}

/// Gradient modes carry a list of colour stops after the fixed fields.
pub fn has_gradient(mode: u8) -> bool {
    mode == 0x10 || mode == 0x11
}

/// One gradient stop: a colour at `pos` percent (0–100).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stop {
    pub color: Rgb,
    pub pos: u8,
}

/// A channel's built-in effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    pub mode: u8,
    pub color: Rgb,
    /// As sent: 0 is fastest, 255 slowest.
    pub speed: u8,
    /// Always 0xFF in ASRock's utility.
    pub level: u8,
    pub sync: bool,
    /// Gradient modes: the stops, without the closing copy of the first.
    pub stops: Vec<Stop>,
    /// Luminous tide: LEDs per cycle (non-zero).
    pub cycle: u8,
}

impl Default for Effect {
    fn default() -> Self {
        Effect { mode: 1, color: Rgb::new(255, 255, 255), speed: 0xe3, level: 0xff, sync: false, stops: default_stops(), cycle: 10 }
    }
}

/// The default gradient ASRock's utility starts from: red, green, blue.
pub fn default_stops() -> Vec<Stop> {
    vec![
        Stop { color: Rgb::new(255, 0, 0), pos: 0 },
        Stop { color: Rgb::new(0, 255, 0), pos: 0x21 },
        Stop { color: Rgb::new(0, 0, 255), pos: 0x42 },
    ]
}

fn report(cmd: u8) -> [u8; LEN + 1] {
    let mut b = [0u8; LEN + 1];
    b[1] = cmd;
    b
}

/// `10`: set channel `ch` to `e`.
pub fn set_effect(ch: u8, e: &Effect) -> [u8; LEN + 1] {
    let mut b = report(CMD_SET);
    b[3] = ch;
    b[4] = e.mode;
    b[5] = e.color.r;
    b[6] = e.color.g;
    b[7] = e.color.b;
    b[8] = e.speed;
    b[9] = e.level;
    b[10] = e.sync as u8;
    if has_gradient(e.mode) && !e.stops.is_empty() {
        // At most 11 stops plus the closing one fit after byte 16.
        let stops: Vec<Stop> = e.stops.iter().copied().take(11).collect();
        b[12] = stops.len() as u8 + 1;
        b[13] = e.cycle.max(1);
        let closing = Stop { color: stops[0].color, pos: 100 };
        for (i, s) in stops.iter().chain(std::iter::once(&closing)).enumerate() {
            let o = 16 + i * 4;
            b[o..o + 4].copy_from_slice(&[s.color.r, s.color.g, s.color.b, s.pos]);
        }
    }
    b
}

/// A request with no parameters, or with sub-command and channel bytes.
pub fn request(cmd: u8, a: u8, b: u8) -> [u8; LEN + 1] {
    let mut r = report(cmd);
    r[3] = a;
    r[4] = b;
    r
}

/// `15 02`: LED counts of all channels.
pub fn write_led_counts(counts: &[u8; CHANNELS]) -> [u8; LEN + 1] {
    let mut b = report(CMD_CONFIG);
    b[3] = SUB_LED_COUNTS;
    b[4..4 + CHANNELS].copy_from_slice(counts);
    b
}

/// `15 03`: which channels have red and green swapped.
pub fn write_rg_swap(mask: u8) -> [u8; LEN + 1] {
    let mut b = report(CMD_CONFIG);
    b[3] = SUB_RG_SWAP;
    b[4] = mask;
    b
}

/// Direct colours for every LED of every channel (channel order, each
/// channel's LEDs in a row), as one or more reports.
pub fn direct(colors: &[Rgb]) -> Vec<[u8; LEN + 1]> {
    let total = colors.len().min(u16::MAX as usize);
    let mut out = Vec::new();
    let mut first = report(CMD_SET);
    first[3] = 0xff;
    first[4] = DIRECT_FIRST;
    first[7..9].copy_from_slice(&(total as u16).to_le_bytes());
    let mut rest = colors[..total].iter();
    for (i, c) in rest.by_ref().take(FIRST_LEDS).enumerate() {
        first[9 + i * 3..12 + i * 3].copy_from_slice(&[c.r, c.g, c.b]);
    }
    out.push(first);
    loop {
        let chunk: Vec<&Rgb> = rest.by_ref().take(NEXT_LEDS).collect();
        if chunk.is_empty() {
            break;
        }
        let mut b = report(CMD_SET);
        b[3] = 0xff;
        b[4] = DIRECT_NEXT;
        for (i, c) in chunk.iter().enumerate() {
            b[5 + i * 3..8 + i * 3].copy_from_slice(&[c.r, c.g, c.b]);
        }
        out.push(b);
    }
    out
}

/// The data of a reply: `a6 00 00 x data...`, where `x` repeats the
/// request's sub-command or channel (stale for `a6`). `expect`: that byte,
/// when the reply must match it. The rest of a reply holds whatever an
/// earlier one left there.
pub fn reply_data(r: &[u8], expect: Option<u8>) -> Option<&[u8]> {
    (r.len() > 4 && expect.is_none_or(|x| r[3] == x)).then(|| &r[4..])
}

/// Reply to `11`: the channel's effect (gradient read separately).
pub fn parse_effect(data: &[u8]) -> Option<Effect> {
    let d = data.get(..7)?;
    Some(Effect { mode: d[0], color: Rgb::new(d[1], d[2], d[3]), speed: d[4], level: d[5], sync: d[6] != 0, ..Effect::default() })
}

/// Reply to `14 0a`: the stops (without the closing one) and the cycle.
pub fn parse_gradient(data: &[u8]) -> Option<(Vec<Stop>, u8)> {
    let n = (*data.first()? as usize).saturating_sub(1).min(11);
    let cycle = *data.get(1)?;
    let stops = (0..n)
        .map(|i| {
            let s = data.get(4 + i * 4..8 + i * 4)?;
            Some(Stop { color: Rgb::new(s[0], s[1], s[2]), pos: s[3] })
        })
        .collect::<Option<Vec<_>>>()?;
    Some((stops, cycle))
}

/// Reply to `a6`: the version, data bytes 4..8 (e.g. `01 10 00 01`).
pub fn parse_version(data: &[u8]) -> Option<String> {
    let v = data.get(4..8)?;
    (v.iter().any(|&b| b != 0)).then(|| format!("{:x}.{:02x}.{:02x}.{:02x}", v[0], v[1], v[2], v[3]))
}

/// `speed` as sent (0 fastest) to a 0–100 % slider (100 fastest).
pub fn speed_percent(speed: u8) -> f64 {
    ((255.0 - speed as f64) / 2.55).round()
}

pub fn speed_byte(percent: f64) -> u8 {
    (255.0 - percent.clamp(0.0, 100.0) * 2.55).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_red_matches_the_utility() {
        let e = Effect { mode: 1, color: Rgb::new(255, 0, 0), speed: 0xe3, ..Effect::default() };
        let b = set_effect(2, &e);
        assert_eq!(&b[..12], &[0, 0x10, 0, 2, 1, 0xff, 0, 0, 0xe3, 0xff, 0, 0]);
        assert!(b[12..].iter().all(|&x| x == 0));
    }

    #[test]
    fn glow_stream_matches_the_utility() {
        // Captured from ASRock Polychrome RGB.
        let want = "00 10 00 02 10 00 00 ff e3 ff 00 00 04 0a 00 00 ff 00 00 00 00 ff 00 21 00 00 ff 42 ff 00 00 64";
        let want: Vec<u8> = want.split(' ').map(|x| u8::from_str_radix(x, 16).unwrap()).collect();
        let e = Effect { mode: 0x10, color: Rgb::new(0, 0, 255), speed: 0xe3, cycle: 10, ..Effect::default() };
        let b = set_effect(2, &e);
        assert_eq!(&b[..want.len()], &want[..]);
        assert!(b[want.len()..].iter().all(|&x| x == 0));
    }

    #[test]
    fn direct_frames_split_into_reports() {
        let colors: Vec<Rgb> = (0..40).map(|i| Rgb::new(i, 0, 0)).collect();
        let r = direct(&colors);
        assert_eq!(r.len(), 3); // 18 + 19 + 3
        assert_eq!(&r[0][..9], &[0, 0x10, 0, 0xff, 0xe3, 0, 0, 40, 0]);
        assert_eq!(r[0][9 + 17 * 3], 17);
        assert_eq!(&r[1][..5], &[0, 0x10, 0, 0xff, 0xe4]);
        assert_eq!(r[1][5], 18);
        assert_eq!(r[2][5 + 2 * 3], 39);
        assert_eq!(direct(&[]).len(), 1);
    }

    fn bytes(s: &str) -> Vec<u8> {
        s.split(' ').map(|x| u8::from_str_radix(x, 16).unwrap()).collect()
    }

    #[test]
    fn replies_from_a_b650e() {
        // Read from an ASRock B650E board.
        let counts = bytes("a6 00 00 02 01 01 50 50 06 07 06 50 00 00 00 00 00 00 ff 42 ff 00 00 64");
        assert_eq!(&reply_data(&counts, Some(SUB_LED_COUNTS)).unwrap()[..8], &[1, 1, 80, 80, 6, 7, 6, 80]);
        assert!(reply_data(&counts, Some(SUB_CHANNELS)).is_none());
        let mask = bytes("a6 00 00 01 9d 00 00 00");
        assert_eq!(reply_data(&mask, Some(SUB_CHANNELS)).unwrap()[0], 0x9d);
        let ch2 = bytes("a6 00 00 02 0e 00 00 ff e3 ff 00 00 00");
        let e = parse_effect(reply_data(&ch2, Some(2)).unwrap()).unwrap();
        assert_eq!((e.mode, e.color, e.speed, e.sync), (0x0e, Rgb::new(0, 0, 255), 0xe3, false));
        let ver = bytes("a6 00 00 03 06 00 00 00 01 10 00 01 00");
        assert_eq!(parse_version(reply_data(&ver, None).unwrap()).as_deref(), Some("1.10.00.01"));
    }

    #[test]
    fn gradient_round_trip() {
        let e = Effect { mode: 0x11, stops: default_stops(), cycle: 7, ..Effect::default() };
        let b = set_effect(3, &e);
        // The read reply lays the same fields out from data byte 0.
        let mut data = vec![b[12], b[13], 0, 0];
        data.extend(&b[16..64]);
        assert_eq!(parse_gradient(&data), Some((default_stops(), 7)));
    }

    #[test]
    fn speed_scale() {
        assert_eq!(speed_byte(100.0), 0);
        assert_eq!(speed_byte(0.0), 255);
        assert_eq!(speed_percent(speed_byte(50.0)), 50.0);
    }

    #[test]
    fn modes_are_unique() {
        let mut v: Vec<u8> = MODES.iter().map(|m| m.0).collect();
        v.dedup();
        assert_eq!(v.len(), MODES.len());
    }
}
