//! ASUS Aura over USB: the LED controller of ASUS motherboards ("AURA LED
//! Controller", `0b05:18f3` and later) and the older stand-alone addressable
//! header controller ("AURA MAINBOARD", `0b05:1867` and others).
//!
//! Every message is report 0xEC, 64 bytes after the ID: `ec cmd ...`. Only
//! the two reads answer, with an input report `ec reply ...`.
//!
//! | Command | Request | Meaning |
//! |---|---|---|
//! | `82` | | firmware version; reply `ec 02 ascii[16]` |
//! | `b0` | | config table; reply `ec 30 00 00 table[60]` |
//! | `35` | `ch 00 shutdown mode` | set an effect channel's mode (motherboard) |
//! | `36` | `maskHi maskLo shutdown rgb...` | the effect colour of the LED slots in the mask (motherboard) |
//! | `3b` | `ch 00 mode r g b` | set a header's mode and colour (addressable controller) |
//! | `3f` | `55` | save the effects to flash (motherboard) |
//! | `40` | `flags offset count rgb...` | direct colours, at most 20 LEDs per report |
//! | `52` | `53 00 01` | drive the addressable headers as Gen 1 (no LED-count detection) |
//!
//! Based on OpenRGB's and liquidctl's protocol work; not tested on hardware.
//! Firmware updates use other commands, which are never sent.

use crate::model::Rgb;

pub const VID: u16 = 0x0b05;

/// The motherboards' controller: effects with 0x35 / 0x36, saved with 0x3F.
pub const MAINBOARD_PIDS: &[u16] = &[0x18f3, 0x1939, 0x19af, 0x1aa6, 0x1bed];
/// The addressable header controller of earlier boards: effects with 0x3B.
pub const ADDRESSABLE_PIDS: &[u16] = &[0x1867, 0x1872, 0x18a3, 0x18a5];
/// The HID usage page of the lighting interface on boards that expose more
/// than one (`19af`).
pub const USAGE_PAGE: u16 = 0xff72;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Mainboard,
    Addressable,
}

pub fn kind(pid: u16) -> Option<Kind> {
    if MAINBOARD_PIDS.contains(&pid) {
        Some(Kind::Mainboard)
    } else if ADDRESSABLE_PIDS.contains(&pid) {
        Some(Kind::Addressable)
    } else {
        None
    }
}

pub const REPORT_ID: u8 = 0xec;
/// Bytes per report, with the report ID.
pub const LEN: usize = 65;

pub const CMD_FIRMWARE: u8 = 0x82;
pub const CMD_CONFIG: u8 = 0xb0;
pub const CMD_EFFECT: u8 = 0x35;
pub const CMD_EFFECT_COLOR: u8 = 0x36;
pub const CMD_ADDRESSABLE_EFFECT: u8 = 0x3b;
pub const CMD_COMMIT: u8 = 0x3f;
pub const CMD_DIRECT: u8 = 0x40;
pub const CMD_GEN1: u8 = 0x52;

pub const REPLY_FIRMWARE: u8 = 0x02;
pub const REPLY_CONFIG: u8 = 0x30;

/// The mode byte that hands a channel to the direct-colour stream.
pub const MODE_DIRECT: u8 = 0xff;
/// Direct-colour flags, OR-ed into the channel byte: the last report of a
/// frame (shows it), and an LED offset of 256 or more (offset byte wraps).
const DIRECT_APPLY: u8 = 0x80;
const DIRECT_OFFSET_HIGH: u8 = 0x10;
const DIRECT_LEDS: usize = 20;
/// The motherboard's own LEDs (and 12 V RGB headers) on the direct stream.
pub const MAINBOARD_DIRECT_CHANNEL: u8 = 0x04;

/// Addressable headers the protocol can address, and colour slots in the
/// 0x36 mask.
const MAX_HEADERS: usize = 8;
const MAX_SLOTS: usize = 16;

/// A built-in effect: (mode byte, name, uses the colour). The firmware has
/// no speed or direction setting.
pub type Mode = (u8, &'static str, bool);

pub const MODES: &[Mode] = &[
    (0x00, "Off", false),
    (0x01, "Static", true),
    (0x02, "Breathing", true),
    (0x03, "Flashing", true),
    (0x04, "Spectrum cycle", false),
    (0x05, "Rainbow", false),
    (0x06, "Spectrum cycle breathing", false),
    (0x07, "Chase fade", true),
    (0x08, "Spectrum cycle chase fade", false),
    (0x09, "Chase", true),
    (0x0a, "Spectrum cycle chase", false),
    (0x0b, "Spectrum cycle wave", false),
    (0x0c, "Chase rainbow pulse", false),
    (0x0d, "Random flicker", false),
    (0x10, "Gentle transition", false),
    (0x11, "Wave propagation", false),
    (0x12, "Wave propagation pause", false),
    (0x13, "Red pulse", false),
];

pub fn mode(byte: u8) -> Option<&'static Mode> {
    MODES.iter().find(|m| m.0 == byte)
}

/// A zone's built-in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effect {
    pub mode: u8,
    pub color: Rgb,
}

impl Default for Effect {
    /// The controller can't report its effects; Rainbow is what ASUS boards
    /// run out of the box.
    fn default() -> Self {
        Effect { mode: 0x05, color: Rgb::new(255, 255, 255) }
    }
}

fn report(cmd: u8) -> [u8; LEN] {
    let mut b = [0u8; LEN];
    b[0] = REPORT_ID;
    b[1] = cmd;
    b
}

/// A request without parameters (`82`, `b0`).
pub fn request(cmd: u8) -> [u8; LEN] {
    report(cmd)
}

/// `3f 55`: save the current effects.
pub fn commit() -> [u8; LEN] {
    let mut b = report(CMD_COMMIT);
    b[2] = 0x55;
    b
}

/// `52 53 00 01`: Gen 1 addressable headers. Gen 2 headers size their
/// strips themselves and ignore direct colours past what they detected.
pub fn gen1() -> [u8; LEN] {
    let mut b = report(CMD_GEN1);
    b[2] = 0x53;
    b[4] = 0x01;
    b
}

/// `35`: the mode of motherboard effect channel `ch`.
pub fn mainboard_effect(ch: u8, mode: u8) -> [u8; LEN] {
    let mut b = report(CMD_EFFECT);
    b[2] = ch;
    b[5] = mode;
    b
}

/// `36`: the effect colour of the LED slots from `start`, one per colour.
/// None if they don't fit the 16-bit mask.
pub fn mainboard_color(start: usize, colors: &[Rgb]) -> Option<[u8; LEN]> {
    if colors.is_empty() || start + colors.len() > MAX_SLOTS {
        return None;
    }
    let mask = (((1u32 << colors.len()) - 1) << start) as u16;
    let mut b = report(CMD_EFFECT_COLOR);
    b[2..4].copy_from_slice(&mask.to_be_bytes());
    for (i, c) in colors.iter().enumerate() {
        let o = 5 + (start + i) * 3;
        b[o..o + 3].copy_from_slice(&[c.r, c.g, c.b]);
    }
    Some(b)
}

/// `3b`: the mode and colour of header `ch` (addressable controller).
pub fn addressable_effect(ch: u8, mode: u8, c: Rgb) -> [u8; LEN] {
    let mut b = report(CMD_ADDRESSABLE_EFFECT);
    b[2] = ch;
    b[4] = mode;
    b[5..8].copy_from_slice(&[c.r, c.g, c.b]);
    b
}

/// `40`: the colours of every LED of direct channel `ch`, as reports of up
/// to 20 LEDs; the last one carries the apply flag.
pub fn direct(ch: u8, colors: &[Rgb]) -> Vec<[u8; LEN]> {
    let colors = &colors[..colors.len().min(511)];
    let n = colors.len().div_ceil(DIRECT_LEDS);
    colors
        .chunks(DIRECT_LEDS)
        .enumerate()
        .map(|(i, chunk)| {
            let start = i * DIRECT_LEDS;
            let mut b = report(CMD_DIRECT);
            b[2] = ch;
            if i + 1 == n {
                b[2] |= DIRECT_APPLY;
            }
            if start > 255 {
                b[2] |= DIRECT_OFFSET_HIGH;
            }
            b[3] = start as u8;
            b[4] = chunk.len() as u8;
            for (j, c) in chunk.iter().enumerate() {
                b[5 + j * 3..8 + j * 3].copy_from_slice(&[c.r, c.g, c.b]);
            }
            b
        })
        .collect()
}

/// Reply to `82`: the version string, e.g. `AULA3-AR32-0207`.
pub fn parse_firmware(r: &[u8]) -> Option<String> {
    if r.len() < 18 || r[1] != REPLY_FIRMWARE {
        return None;
    }
    let s: String = r[2..18].iter().take_while(|&&b| b != 0).map(|&b| b as char).collect();
    let s = s.trim();
    (!s.is_empty() && s.chars().all(|c| c.is_ascii_graphic())).then(|| s.to_string())
}

/// The parts of the 60-byte config table that are understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Addressable (5 V) headers.
    pub headers: usize,
    /// The motherboard's own LEDs, including one per 12 V RGB header.
    pub mainboard_leds: usize,
    /// 12 V RGB headers, the last of the motherboard LEDs.
    pub rgb_headers: usize,
    pub raw: [u8; 60],
}

/// Reply to `b0`: `ec 30 00 00 table[60]`. Table byte 0x02 is the number
/// of addressable headers, 0x1B the motherboard LEDs, 0x1D the RGB headers.
pub fn parse_config(r: &[u8]) -> Option<Config> {
    if r.len() < 64 || r[1] != REPLY_CONFIG {
        return None;
    }
    let mut raw = [0u8; 60];
    raw.copy_from_slice(&r[4..64]);
    let mainboard_leds = raw[0x1b] as usize;
    // More RGB headers than LEDs: the field means something else here.
    let rgb_headers = Some(raw[0x1d] as usize).filter(|&n| n <= mainboard_leds).unwrap_or(0);
    Some(Config { headers: raw[0x02] as usize, mainboard_leds, rgb_headers, raw })
}

/// One effect zone: the motherboard's LEDs, or an addressable header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    /// For control ids: `mb`, `argb1`...
    pub key: String,
    pub name: String,
    /// The channel byte of `35` / `3b`.
    pub effect_ch: u8,
    /// The channel byte of `40`.
    pub direct_ch: u8,
    /// First `36` colour slot and how many (the motherboard has one per
    /// LED, a header one for the whole strip).
    pub slot: usize,
    pub slots: usize,
    /// Addressable header number (0-based), whose LED count the user sets.
    pub header: Option<usize>,
}

/// The zones a controller has, from its config table. Firmware
/// `AULA3-AR32*` numbers the header effect channels from 0 and puts the
/// motherboard after them; others give the motherboard 0 and the headers 1
/// up. Err: a table this driver doesn't understand.
pub fn zones(kind: Kind, cfg: &Config, firmware: &str) -> Result<Vec<Zone>, &'static str> {
    if cfg.headers > MAX_HEADERS {
        return Err("more addressable headers than the protocol knows");
    }
    let header = |i: usize, effect_ch: usize, slot: usize| Zone {
        key: format!("argb{}", i + 1),
        name: format!("Addressable header {}", i + 1),
        effect_ch: effect_ch as u8,
        direct_ch: i as u8,
        slot,
        slots: 1,
        header: Some(i),
    };
    match kind {
        Kind::Addressable => Ok((0..cfg.headers).map(|i| header(i, i, 0)).collect()),
        Kind::Mainboard => {
            let m = cfg.mainboard_leds;
            if m + cfg.headers > MAX_SLOTS {
                return Err("more LEDs than the effect colour command addresses");
            }
            let ar32 = firmware.starts_with("AULA3-AR32");
            let mut v = Vec::new();
            if m > 0 {
                v.push(Zone {
                    key: "mb".into(),
                    name: if cfg.rgb_headers > 0 { "Motherboard and RGB headers".into() } else { "Motherboard".into() },
                    effect_ch: if ar32 { cfg.headers as u8 } else { 0 },
                    direct_ch: MAINBOARD_DIRECT_CHANNEL,
                    slot: 0,
                    slots: m,
                    header: None,
                });
            }
            let first = if m > 0 && !ar32 { 1 } else { 0 };
            v.extend((0..cfg.headers).map(|i| header(i, first + i, m + i)));
            Ok(v)
        }
    }
}

/// The reports that set zone `z` to `e`.
pub fn set_effect(kind: Kind, z: &Zone, e: &Effect) -> Vec<[u8; LEN]> {
    match kind {
        Kind::Addressable => vec![addressable_effect(z.effect_ch, e.mode, e.color)],
        Kind::Mainboard => {
            let mut v = vec![mainboard_effect(z.effect_ch, e.mode)];
            v.extend(mainboard_color(z.slot, &vec![e.color; z.slots]));
            v
        }
    }
}

/// The report that hands zone `z` to the direct-colour stream.
pub fn enter_direct(kind: Kind, z: &Zone) -> [u8; LEN] {
    match kind {
        Kind::Addressable => addressable_effect(z.effect_ch, MODE_DIRECT, Rgb::BLACK),
        Kind::Mainboard => mainboard_effect(z.effect_ch, MODE_DIRECT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(headers: u8, leds: u8, rgb: u8) -> Config {
        let mut r = [0u8; 65];
        r[0] = REPORT_ID;
        r[1] = REPLY_CONFIG;
        r[4 + 0x02] = headers;
        r[4 + 0x1b] = leds;
        r[4 + 0x1d] = rgb;
        parse_config(&r).unwrap()
    }

    #[test]
    fn config_table() {
        let c = config(3, 5, 1);
        assert_eq!((c.headers, c.mainboard_leds, c.rgb_headers), (3, 5, 1));
        assert_eq!(config(1, 0, 2).rgb_headers, 0);
        let mut r = [0u8; 65];
        r[1] = 0x31;
        assert!(parse_config(&r).is_none());
        assert!(parse_config(&[0xec, 0x30, 0, 0]).is_none());
    }

    #[test]
    fn firmware_string() {
        let mut r = [0u8; 65];
        r[0] = REPORT_ID;
        r[1] = REPLY_FIRMWARE;
        r[2..17].copy_from_slice(b"AULA3-AR32-0207");
        assert_eq!(parse_firmware(&r).as_deref(), Some("AULA3-AR32-0207"));
        r[1] = 0;
        assert!(parse_firmware(&r).is_none());
        assert!(parse_firmware(&[0xec, 0x02, 0, 0]).is_none());
    }

    #[test]
    fn effect_channels_by_firmware() {
        let c = config(3, 4, 1);
        let z = zones(Kind::Mainboard, &c, "AULA3-6K75-0207").unwrap();
        let chans: Vec<(u8, u8, usize)> = z.iter().map(|z| (z.effect_ch, z.direct_ch, z.slot)).collect();
        assert_eq!(chans, [(0, 4, 0), (1, 0, 4), (2, 1, 5), (3, 2, 6)]);
        let z = zones(Kind::Mainboard, &c, "AULA3-AR32-0207").unwrap();
        let chans: Vec<u8> = z.iter().map(|z| z.effect_ch).collect();
        assert_eq!(chans, [3, 0, 1, 2]);
        // No motherboard LEDs: headers from 0 either way.
        let z = zones(Kind::Mainboard, &config(2, 0, 0), "AULA3-6K75").unwrap();
        assert_eq!(z.iter().map(|z| (z.effect_ch, z.slot)).collect::<Vec<_>>(), [(0, 0), (1, 1)]);
        assert!(zones(Kind::Mainboard, &config(9, 0, 0), "").is_err());
        assert!(zones(Kind::Mainboard, &config(3, 14, 0), "").is_err());
        let z = zones(Kind::Addressable, &config(2, 0, 0), "").unwrap();
        assert_eq!(z.iter().map(|z| (z.effect_ch, z.direct_ch)).collect::<Vec<_>>(), [(0, 0), (1, 1)]);
    }

    #[test]
    fn mainboard_effect_packets() {
        let z = &zones(Kind::Mainboard, &config(2, 3, 0), "").unwrap();
        let r = set_effect(Kind::Mainboard, &z[0], &Effect { mode: 1, color: Rgb::new(1, 2, 3) });
        assert_eq!(r.len(), 2);
        assert_eq!(&r[0][..6], &[0xec, 0x35, 0, 0, 0, 1]);
        assert_eq!(&r[1][..14], &[0xec, 0x36, 0x00, 0x07, 0, 1, 2, 3, 1, 2, 3, 1, 2, 3]);
        // The second header: slot 4 (after three LEDs and header 1).
        let r = set_effect(Kind::Mainboard, &z[2], &Effect { mode: 2, color: Rgb::new(9, 8, 7) });
        assert_eq!(&r[0][..6], &[0xec, 0x35, 2, 0, 0, 2]);
        assert_eq!(&r[1][..4], &[0xec, 0x36, 0x00, 0x10]);
        assert!(r[1][5..17].iter().all(|&b| b == 0));
        assert_eq!(&r[1][17..20], &[9, 8, 7]);
        assert_eq!(mainboard_color(15, &[Rgb::BLACK]).unwrap()[2..4], [0x80, 0x00]);
        assert!(mainboard_color(15, &[Rgb::BLACK; 2]).is_none());
        assert_eq!(&enter_direct(Kind::Mainboard, &z[1])[..6], &[0xec, 0x35, 1, 0, 0, 0xff]);
    }

    #[test]
    fn addressable_effect_packets() {
        let z = &zones(Kind::Addressable, &config(2, 0, 0), "").unwrap();
        let r = set_effect(Kind::Addressable, &z[1], &Effect { mode: 1, color: Rgb::new(255, 0, 16) });
        assert_eq!(r.len(), 1);
        assert_eq!(&r[0][..8], &[0xec, 0x3b, 1, 0, 1, 255, 0, 16]);
        assert!(r[0][8..].iter().all(|&b| b == 0));
        assert_eq!(&enter_direct(Kind::Addressable, &z[0])[..8], &[0xec, 0x3b, 0, 0, 0xff, 0, 0, 0]);
    }

    #[test]
    fn direct_frames_split_into_reports() {
        let colors: Vec<Rgb> = (0..45).map(|i| Rgb::new(i, 0, 0)).collect();
        let r = direct(1, &colors);
        assert_eq!(r.len(), 3);
        assert_eq!(&r[0][..8], &[0xec, 0x40, 0x01, 0, 20, 0, 0, 0]);
        assert_eq!(r[0][5 + 19 * 3], 19);
        assert_eq!(&r[1][..6], &[0xec, 0x40, 0x01, 20, 20, 20]);
        assert_eq!(&r[2][..6], &[0xec, 0x40, 0x81, 40, 5, 40]);
        let r = direct(MAINBOARD_DIRECT_CHANNEL, &[Rgb::new(1, 2, 3)]);
        assert_eq!(&r[0][..8], &[0xec, 0x40, 0x84, 0, 1, 1, 2, 3]);
        assert!(direct(0, &[]).is_empty());
        // Offsets past 255 set the high flag and wrap.
        let r = direct(2, &[Rgb::BLACK; 300]);
        assert_eq!(&r[12][2..5], &[0x02, 240, 20]);
        assert_eq!(&r[13][2..5], &[0x12, 4, 20]);
        assert_eq!(&r[14][2..5], &[0x92, 24, 20]);
    }

    #[test]
    fn fixed_packets() {
        assert_eq!(&commit()[..4], &[0xec, 0x3f, 0x55, 0]);
        assert_eq!(&gen1()[..6], &[0xec, 0x52, 0x53, 0x00, 0x01, 0]);
        assert_eq!(request(CMD_CONFIG).len(), LEN);
    }

    #[test]
    fn pids_and_modes() {
        assert_eq!(kind(0x19af), Some(Kind::Mainboard));
        assert_eq!(kind(0x1867), Some(Kind::Addressable));
        assert_eq!(kind(0x1889), None); // ROG Aura Terminal
        let mut v: Vec<u8> = MODES.iter().map(|m| m.0).collect();
        v.dedup();
        assert_eq!(v.len(), MODES.len());
        assert!(mode(MODE_DIRECT).is_none());
    }
}
