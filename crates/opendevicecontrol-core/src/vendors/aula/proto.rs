//! Packet encoding for the CX/Sinowealth 2.4 GHz keyboard receiver
//! (`3554:fa09`, vendor collection usage page 0xFF02, report 0x13).
//!
//! Every packet is 20 bytes including the report id:
//!
//! | byte | meaning |
//! |---|---|
//! | 0 | report id `0x13` |
//! | 1 | command |
//! | 2 | packets in this transfer (replies: bit 7 = checksum error) |
//! | 3 | index of this packet (0-based) |
//! | 4 | sub-command (high nibble) + payload length (low nibble, ≤ 14) |
//! | 5..19 | payload |
//! | 19 | checksum: sum of bytes 0..19, mod 256 |
//!
//! | Command | Meaning |
//! |---|---|
//! | `07` | is the keyboard linked and awake? reply `[5]` = 1/0 |
//! | `05` | model query, reply `[10]` = model id |
//! | `44` / `04` | read / write the 128-byte settings block (see [`Config`]) |
//! | `88` | live lighting, sub 1 = per-key, sub 2 = solid (not acknowledged) |
//! | `4A` | battery: reply `[5]` = percent, `[6]` = flags (bit 0 not full, bit 4 on USB power) |
//! | `0A` (input) | event: `[5]=2` → `[6]` = keyboard online; `[5]=5` → `[6..8]` battery; `[5]=7` settings reloaded (e.g. changed with Fn keys) |
//!
//! Writes other than `0x88` are acknowledged packet by packet with a reply
//! carrying the same command and index. Read requests are one packet with no
//! payload; the answer comes back in as many packets as needed.
//!
//! Per-key lighting is a run of groups `R G B n led1..ledn` split across
//! packets; LEDs not named in a transfer are switched off. The keyboard falls
//! back to its own effect unless live lighting is refreshed regularly.
//!
//! Sources: OpenRGB issue #5166 / draft MR !3027 (live lighting) and static
//! analysis of the official AULA F99 driver (settings block, acks, events),
//! checked on an F99.

use crate::model::Rgb;

pub const REPORT_ID: u8 = 0x13;
pub const PACKET_LEN: usize = 20;
pub const PAYLOAD_LEN: usize = 14;

pub const CMD_ONLINE: u8 = 0x07;
pub const CMD_MODEL: u8 = 0x05;
pub const CMD_READ_CONFIG: u8 = 0x44;
pub const CMD_WRITE_CONFIG: u8 = 0x04;
pub const CMD_LIGHTS: u8 = 0x88;
pub const EVT_DEVICE: u8 = 0x0a;
/// Battery read (from the keyboard firmware; the official driver doesn't use it).
pub const CMD_BATTERY: u8 = 0x4a;
/// Most payload bytes the keyboard accepts in one live per-key frame (its
/// buffer holds 199; the official driver's 200-byte cap overruns it).
pub const MAX_LIVE_PAYLOAD: usize = 196;

const SUB_PER_KEY: u8 = 1;
const SUB_SOLID: u8 = 2;

/// Model ids answered by the keyboard to [`CMD_MODEL`] (reply byte 10).
pub const MODEL_F99: u8 = 0xa4;
pub const MODEL_F99_PRO: u8 = 0x58;
pub const MODEL_F75: u8 = 0xcd;

pub type Packet = [u8; PACKET_LEN];

/// Sum of all bytes but the last, mod 256.
pub fn checksum(p: &[u8]) -> u8 {
    p[..p.len() - 1].iter().fold(0u8, |a, b| a.wrapping_add(*b))
}

fn packet(cmd: u8, total: u8, index: u8, b4: u8, payload: &[u8]) -> Packet {
    let mut p = [0u8; PACKET_LEN];
    p[0] = REPORT_ID;
    p[1] = cmd;
    p[2] = total;
    p[3] = index;
    p[4] = b4;
    p[5..5 + payload.len()].copy_from_slice(payload);
    p[PACKET_LEN - 1] = checksum(&p);
    p
}

/// A one-packet request with no payload (online, model, read settings).
pub fn request(cmd: u8) -> Packet {
    packet(cmd, 1, 0, 0, &[])
}

/// A read request for sub-item `sub` (keymap layer, macro page).
pub fn request_sub(cmd: u8, sub: u8) -> Packet {
    packet(cmd, 1, 0, sub << 4, &[])
}

/// Keymap layer read / write (sub = layer 0–3).
pub const CMD_READ_LAYER: u8 = 0x41;
pub const CMD_WRITE_LAYER: u8 = 0x01;
/// Macro page read / write (sub = page 0–7).
pub const CMD_READ_MACROS: u8 = 0x43;
pub const CMD_WRITE_MACROS: u8 = 0x03;

/// `data` split into the 14-byte packets of one transfer.
pub fn transfer(cmd: u8, sub: u8, data: &[u8]) -> Vec<Packet> {
    if data.is_empty() {
        return vec![packet(cmd, 1, 0, sub << 4, &[])];
    }
    let chunks: Vec<&[u8]> = data.chunks(PAYLOAD_LEN).take(u8::MAX as usize).collect();
    let total = chunks.len() as u8;
    chunks.iter().enumerate().map(|(i, c)| packet(cmd, total, i as u8, (sub << 4) | c.len() as u8, c)).collect()
}

/// Whole keyboard in one colour.
pub fn solid(c: Rgb) -> Packet {
    transfer(CMD_LIGHTS, SUB_SOLID, &[c.r, c.g, c.b])[0]
}

/// A received report with the right id, length and checksum.
pub fn valid(reply: &[u8]) -> bool {
    reply.len() >= PACKET_LEN && reply[0] == REPORT_ID && checksum(&reply[..PACKET_LEN]) == reply[PACKET_LEN - 1]
}

/// The model id in a reply to [`CMD_MODEL`], if `reply` is one.
pub fn parse_model(reply: &[u8]) -> Option<u8> {
    (valid(reply) && reply[1] == CMD_MODEL).then(|| reply[10])
}

/// The keyboard's online state from a reply to [`CMD_ONLINE`] or an
/// unsolicited device event, if `reply` is one of those.
pub fn parse_online(reply: &[u8]) -> Option<bool> {
    if !valid(reply) {
        return None;
    }
    match reply[1] {
        CMD_ONLINE => Some(reply[5] != 0),
        EVT_DEVICE if reply[5] == 2 => Some(reply[6] != 0),
        _ => None,
    }
}

/// Battery state from a reply to [`CMD_BATTERY`] or a battery event:
/// (percent, flags). Flags: bit 0 = not fully charged, bit 4 = on USB
/// power (so 0x11 charging, 0x10 full on the cable, 0x01 on battery).
pub fn parse_battery(reply: &[u8]) -> Option<(u8, u8)> {
    if !valid(reply) {
        return None;
    }
    match reply[1] {
        CMD_BATTERY if reply[4] & 0x0f >= 2 => Some((reply[5].min(100), reply[6])),
        EVT_DEVICE if reply[5] == 5 => Some((reply[6].min(100), reply[7])),
        _ => None,
    }
}

/// Whether battery `flags` mean charging (on USB power, not full yet).
pub fn charging(flags: u8) -> bool {
    flags & 0x11 == 0x11
}

/// Whether `reply` says the keyboard reloaded its settings (they may have
/// been changed on the keyboard, e.g. lighting with Fn keys).
pub fn parse_settings_changed(reply: &[u8]) -> bool {
    valid(reply) && reply[1] == EVT_DEVICE && reply[5] == 7
}

/// Collects the packets of a multi-packet reply to `cmd`.
pub struct Reassembly {
    cmd: u8,
    parts: Vec<Option<Vec<u8>>>,
}

impl Reassembly {
    pub fn new(cmd: u8) -> Self {
        Self { cmd, parts: Vec::new() }
    }

    /// Feed one report; returns the whole payload once every packet is in.
    pub fn push(&mut self, r: &[u8]) -> Option<Vec<u8>> {
        if !valid(r) || r[1] != self.cmd || r[2] & 0x80 != 0 {
            return None;
        }
        let (total, index, len) = ((r[2] & 0x7f) as usize, r[3] as usize, (r[4] & 0x0f) as usize);
        if total == 0 || index >= total || len > PAYLOAD_LEN {
            return None;
        }
        if self.parts.len() != total {
            self.parts = vec![None; total];
        }
        self.parts[index] = Some(r[5..5 + len].to_vec());
        self.parts.iter().all(Option::is_some).then(|| self.parts.iter().flatten().flatten().copied().collect())
    }
}

/// Whether `reply` acknowledges packet `index` of a `cmd` write.
#[derive(Debug, PartialEq, Eq)]
pub enum Ack {
    No,
    Ok,
    /// The keyboard rejected the packet's checksum.
    Bad,
}

pub fn ack(reply: &[u8], cmd: u8, index: u8) -> Ack {
    if !valid(reply) || reply[1] != cmd || reply[3] != index {
        Ack::No
    } else if reply[2] & 0x80 != 0 {
        Ack::Bad
    } else {
        Ack::Ok
    }
}

pub const CONFIG_LEN: usize = 128;

/// Low-nibble colour value meaning "colourful" (random / rainbow colours).
pub const COLOURFUL: u8 = 7;

/// The keyboard's persistent settings block (read `0x44`, written `0x04`).
///
/// | Offset | Meaning |
/// |---|---|
/// | `0x03` | debounce |
/// | `0x09` | 1 = custom per-key effect |
/// | `0x0A` | hardware effect id |
/// | `0x18` | sleep after idle, units of 30 s (0 = never) |
/// | `0x38 + 2e` | effect `e`: brightness 0–4 |
/// | `0x39 + 2e` | effect `e`: speed 0–4 (high nibble), colour (low nibble; 0–6 palette, 7 colourful) |
/// | `0x7E..0x80` | `5A A5` |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config(pub [u8; CONFIG_LEN]);

impl Config {
    /// A block read from the keyboard, if it has the right length and magic.
    pub fn parse(data: &[u8]) -> Option<Self> {
        let b: [u8; CONFIG_LEN] = data.get(..CONFIG_LEN)?.try_into().ok()?;
        (b[0x7e] == 0x5a && b[0x7f] == 0xa5).then_some(Self(b))
    }
    pub fn effect(&self) -> u8 {
        self.0[0x0a]
    }
    pub fn set_effect(&mut self, e: u8) {
        self.0[0x0a] = e;
        self.0[0x09] = 0;
    }
    fn slot(e: u8) -> Option<usize> {
        let i = 0x38 + 2 * e as usize;
        (i + 1 < 0x7e).then_some(i)
    }
    /// (brightness, speed, colour) of effect `e`.
    pub fn effect_settings(&self, e: u8) -> (u8, u8, u8) {
        match Self::slot(e) {
            Some(i) => (self.0[i], self.0[i + 1] >> 4, self.0[i + 1] & 0x0f),
            None => (0, 0, 0),
        }
    }
    pub fn set_effect_settings(&mut self, e: u8, brightness: u8, speed: u8, colour: u8) {
        if let Some(i) = Self::slot(e) {
            self.0[i] = brightness.min(4);
            self.0[i + 1] = (speed.min(4) << 4) | (colour & 0x0f);
        }
    }
    pub fn sleep(&self) -> u8 {
        self.0[0x18]
    }
    pub fn set_sleep(&mut self, v: u8) {
        self.0[0x18] = v;
    }
    pub fn debounce(&self) -> u8 {
        self.0[0x03]
    }
    /// OS mode (Fn+W/Q/E/R): 0 Windows, 1 Windows with media keys first on
    /// the F-row, 2 and 3 Mac layouts.
    pub fn os_mode(&self) -> u8 {
        self.0[0x1b].min(3)
    }
    pub fn set_os_mode(&mut self, v: u8) {
        self.0[0x1b] = v.min(3);
    }
    /// Tap/hold: a key with a tap-layer action sends it when released
    /// within this many ~21 ms units (0 = tap layer off).
    pub fn tap_hold(&self) -> u8 {
        self.0[0x16]
    }
    pub fn set_tap_hold(&mut self, v: u8) {
        self.0[0x16] = v;
    }
    /// Windows key disabled (only in OS mode 0).
    pub fn win_lock(&self) -> bool {
        self.0[0x0f] != 0
    }
    pub fn set_win_lock(&mut self, on: bool) {
        self.0[0x0f] = on as u8;
    }
    pub fn set_debounce(&mut self, v: u8) {
        self.0[0x03] = v;
    }
    /// Packets writing this block back.
    pub fn packets(&self) -> Vec<Packet> {
        transfer(CMD_WRITE_CONFIG, 0, &self.0)
    }
}

/// Groups of LEDs sharing a colour, black LEDs left out (they are switched
/// off anyway). When there are more than `max_groups` distinct colours,
/// colours are merged by dropping low bits until they fit; each merged group
/// gets the average of its members.
pub fn group_colors(leds: &[(u8, Rgb)], max_groups: usize) -> Vec<(Rgb, Vec<u8>)> {
    let lit: Vec<(u8, Rgb)> = leds.iter().copied().filter(|(_, c)| *c != Rgb::BLACK).collect();
    let mut bits = 8u32;
    loop {
        let shift = 8 - bits;
        let key = |c: Rgb| (c.r >> shift, c.g >> shift, c.b >> shift);
        // (quantised colour, channel sums, LEDs)
        type Group = ((u8, u8, u8), [u32; 3], Vec<u8>);
        let mut groups: Vec<Group> = Vec::new();
        for &(led, c) in &lit {
            let k = key(c);
            let i = match groups.iter().position(|g| g.0 == k) {
                Some(i) => i,
                None => {
                    groups.push((k, [0; 3], Vec::new()));
                    groups.len() - 1
                }
            };
            let g = &mut groups[i];
            g.1[0] += c.r as u32;
            g.1[1] += c.g as u32;
            g.1[2] += c.b as u32;
            g.2.push(led);
        }
        if groups.len() <= max_groups || bits == 1 {
            return groups
                .into_iter()
                .filter_map(|(_, sum, members)| {
                    let n = members.len() as u32;
                    let avg = |s: u32| ((s + n / 2) / n) as u8;
                    let c = Rgb::new(avg(sum[0]), avg(sum[1]), avg(sum[2]));
                    // Averaging very dark colours can round to black.
                    (c != Rgb::BLACK).then_some((c, members))
                })
                .collect();
        }
        bits -= 1;
    }
}

/// Packets for a per-key frame. All LEDs black still needs one (empty)
/// packet, which switches every key off.
pub fn per_key(groups: &[(Rgb, Vec<u8>)]) -> Vec<Packet> {
    let mut payload = Vec::new();
    for (c, leds) in groups {
        for chunk in leds.chunks(u8::MAX as usize) {
            payload.extend_from_slice(&[c.r, c.g, c.b, chunk.len() as u8]);
            payload.extend_from_slice(chunk);
        }
    }
    transfer(CMD_LIGHTS, SUB_PER_KEY, &payload)
}

/// Wired (USB cable, `258a:010c`) framing: one 520-byte feature report per
/// page, `06 cmd sub 00 pages page sizeLE data[512]`. Reads send `cmd | 0x80`
/// and fetch the answer with a feature GET (data again from byte 8).
///
/// | Command | Meaning | Receiver equivalent |
/// |---|---|---|
/// | `82` (sub 1, size 6) | model query, reply `[13]` = model id | `05` |
/// | `84` / `04` | read / write the settings block | `44` / `04` |
/// | `08` | live colours, RGB triplets at `8 + 3 × LED` | `88` |
pub mod wired {
    use super::{Config, CONFIG_LEN};
    use crate::model::Rgb;

    pub const REPORT_ID: u8 = 0x06;
    pub const LEN: usize = 520;
    const HEADER: usize = 8;
    pub const CMD_MODEL: u8 = 0x82;
    pub const CMD_CONFIG: u8 = 0x04;
    pub const CMD_LIGHTS: u8 = 0x08;
    /// Keymap layer (sub = layer 0–3) and macro page (page 0–7) commands.
    pub const CMD_LAYER: u8 = 0x03;
    pub const CMD_MACROS: u8 = 0x05;
    pub const DATA: usize = 8;
    /// LED slots in a live frame (126 × 3 = 0x17A bytes).
    pub const LED_SLOTS: usize = 126;

    /// One single-page report carrying `data` (≤ 512 bytes).
    pub fn report(cmd: u8, sub: u8, size: usize, data: &[u8]) -> Vec<u8> {
        report_page(cmd, sub, 0, size, data)
    }

    /// Like [`report`], for page `page` of a multi-page item (macros).
    pub fn report_page(cmd: u8, sub: u8, page: u8, size: usize, data: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; LEN];
        b[0] = REPORT_ID;
        b[1] = cmd;
        b[2] = sub;
        b[4] = 1; // pages
        b[5] = page;
        b[6..8].copy_from_slice(&(size as u16).to_le_bytes());
        let n = data.len().min(LEN - HEADER);
        b[HEADER..HEADER + n].copy_from_slice(&data[..n]);
        b
    }

    /// Request for a read of `size` bytes with command `cmd` (sent `| 0x80`).
    pub fn read_request(cmd: u8, sub: u8, size: usize) -> Vec<u8> {
        report(cmd | 0x80, sub, size, &[])
    }

    pub fn model_request() -> Vec<u8> {
        read_request(CMD_MODEL, 1, 6)
    }

    pub fn parse_model(reply: &[u8]) -> Option<u8> {
        (reply.len() > 13 && reply[0] == REPORT_ID && reply[1] == CMD_MODEL).then(|| reply[13])
    }

    pub fn config_request() -> Vec<u8> {
        read_request(CMD_CONFIG, 0, CONFIG_LEN)
    }

    pub fn parse_config(reply: &[u8]) -> Option<Config> {
        (reply.len() >= HEADER + CONFIG_LEN && reply[0] == REPORT_ID && reply[1] & 0x7f == CMD_CONFIG)
            .then(|| Config::parse(&reply[HEADER..HEADER + CONFIG_LEN]))
            .flatten()
    }

    /// Battery read (hidden command 0x87): reply data = percent, flags.
    pub fn battery_request() -> Vec<u8> {
        read_request(0x07, 0, 2)
    }

    pub fn parse_battery(reply: &[u8]) -> Option<(u8, u8)> {
        (reply.len() > HEADER + 1 && reply[0] == REPORT_ID && reply[1] & 0x7f == 0x07)
            .then(|| (reply[HEADER].min(100), reply[HEADER + 1]))
    }

    pub fn config_write(c: &Config) -> Vec<u8> {
        report(CMD_CONFIG, 0, CONFIG_LEN, &c.0)
    }

    /// Live colours, by LED index.
    pub fn lights(leds: &[(u8, Rgb)]) -> Vec<u8> {
        let mut rgb = [0u8; LED_SLOTS * 3];
        for &(led, c) in leds {
            let i = led as usize * 3;
            if i + 2 < rgb.len() {
                rgb[i..i + 3].copy_from_slice(&[c.r, c.g, c.b]);
            }
        }
        report(CMD_LIGHTS, 0, rgb.len(), &rgb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wired_reports_match_openrgb() {
        let m = wired::model_request();
        assert_eq!(m.len(), 520);
        assert_eq!(&m[..8], &[0x06, 0x82, 0x01, 0x00, 0x01, 0x00, 0x06, 0x00]);
        let l = wired::lights(&[(0, Rgb::new(1, 2, 3)), (112, Rgb::new(4, 5, 6))]);
        assert_eq!(&l[..11], &[0x06, 0x08, 0x00, 0x00, 0x01, 0x00, 0x7a, 0x01, 1, 2, 3]);
        assert_eq!(&l[8 + 336..8 + 339], &[4, 5, 6]);
        assert_eq!(&wired::config_request()[..8], &[0x06, 0x84, 0x00, 0x00, 0x01, 0x00, 0x80, 0x00]);
        assert_eq!(&wired::battery_request()[..8], &[0x06, 0x87, 0x00, 0x00, 0x01, 0x00, 0x02, 0x00]);
        // Firmware notes: layer read `06 83 sub 00 01 page 00 02`, macro write `06 05 00 00 01 page 00 02`.
        assert_eq!(&wired::read_request(wired::CMD_LAYER, 2, 512)[..8], &[0x06, 0x83, 0x02, 0x00, 0x01, 0x00, 0x00, 0x02]);
        assert_eq!(&wired::report_page(wired::CMD_MACROS, 0, 5, 512, &[1, 2])[..10], &[0x06, 0x05, 0x00, 0x00, 0x01, 0x05, 0x00, 0x02, 1, 2]);
        // Wireless: layer write is 37 packets (`13 01 25 idx …`), read request `13 41 01 00 L<<4`.
        let p = transfer(CMD_WRITE_LAYER, 3, &[0u8; 512]);
        assert_eq!(p.len(), 37);
        assert_eq!(&p[0][..5], &[0x13, 0x01, 0x25, 0x00, 0x3e]);
        assert_eq!(&p[36][..5], &[0x13, 0x01, 0x25, 0x24, 0x38]);
        assert_eq!(&request_sub(CMD_READ_LAYER, 2)[..5], &[0x13, 0x41, 0x01, 0x00, 0x20]);
        let mut reply = vec![0u8; 520];
        reply[0] = 0x06;
        reply[1] = 0x82;
        reply[13] = 0xa4;
        assert_eq!(wired::parse_model(&reply), Some(MODEL_F99));
    }

    #[test]
    fn wired_config_round_trips() {
        let c = f99_config();
        let w = wired::config_write(&c);
        assert_eq!(&w[..8], &[0x06, 0x04, 0x00, 0x00, 0x01, 0x00, 0x80, 0x00]);
        assert_eq!(wired::parse_config(&w), Some(c));
    }

    fn reply(bytes: &[u8]) -> Packet {
        let mut p = [0u8; PACKET_LEN];
        p[..bytes.len()].copy_from_slice(bytes);
        p[19] = checksum(&p);
        p
    }

    #[test]
    fn requests_match_capture() {
        let mut want = [0u8; 20];
        want[..3].copy_from_slice(&[0x13, 0x05, 0x01]);
        want[19] = 0x19;
        assert_eq!(request(CMD_MODEL), want);
        assert_eq!(request(CMD_ONLINE)[19], 0x1b);
        assert_eq!(request(CMD_READ_CONFIG)[19], 0x58);
    }

    #[test]
    fn solid_red_matches_capture() {
        let p = solid(Rgb::new(0xff, 0, 0));
        assert_eq!(&p[..8], &[0x13, 0x88, 0x01, 0x00, 0x23, 0xff, 0x00, 0x00]);
        assert!(p[8..19].iter().all(|&b| b == 0));
        assert_eq!(p[19], 0xbe);
    }

    #[test]
    fn single_key_matches_capture() {
        // From OpenRGB issue #5166: one key, colour 00 00 8b, LED 0x2c.
        let p = per_key(&group_colors(&[(0x2c, Rgb::new(0, 0, 0x8b))], 16));
        assert_eq!(p.len(), 1);
        assert_eq!(&p[0][..10], &[0x13, 0x88, 0x01, 0x00, 0x15, 0x00, 0x00, 0x8b, 0x01, 0x2c]);
        assert_eq!(p[0][19], 0x69);
    }

    #[test]
    fn parses_model_reply() {
        let r = [0x13, 0x05, 0x01, 0x00, 0x0a, 0x03, 0, 0, 0, 0, 0xa4, 0x01, 0, 0, 0, 0, 0, 0, 0, 0xcb];
        assert_eq!(parse_model(&r), Some(MODEL_F99));
        assert_eq!(parse_model(&request(CMD_ONLINE)), None);
    }

    #[test]
    fn parses_online_state() {
        // Captured: awake `13 07 01 00 01 01 …`, asleep `13 07 01 00 01 00 …`.
        assert_eq!(parse_online(&reply(&[0x13, 0x07, 0x01, 0x00, 0x01, 0x01])), Some(true));
        assert_eq!(parse_online(&reply(&[0x13, 0x07, 0x01, 0x00, 0x01, 0x00])), Some(false));
        assert_eq!(parse_online(&reply(&[0x13, 0x0a, 0x01, 0x00, 0x02, 0x02, 0x00])), Some(false));
        assert_eq!(parse_online(&reply(&[0x13, 0x0a, 0x01, 0x00, 0x02, 0x03, 0x01])), None);
        let mut bad = reply(&[0x13, 0x07, 0x01, 0x00, 0x01, 0x01]);
        bad[19] ^= 1;
        assert_eq!(parse_online(&bad), None);
    }

    #[test]
    fn groups_split_across_packets() {
        let leds: Vec<(u8, Rgb)> = (0..20).map(|i| (i, Rgb::new(10, 20, 30))).collect();
        let p = per_key(&group_colors(&leds, 16));
        // 4 header bytes + 20 LEDs = 24 bytes -> 14 + 10.
        assert_eq!(p.len(), 2);
        assert_eq!(&p[0][1..5], &[0x88, 2, 0, 0x1e]);
        assert_eq!(&p[0][5..10], &[10, 20, 30, 20, 0]);
        assert_eq!(&p[1][1..5], &[0x88, 2, 1, 0x1a]);
        assert_eq!(p[1][5 + 9], 19);
        for q in &p {
            assert!(valid(q));
        }
    }

    #[test]
    fn black_is_left_out_and_all_black_is_one_packet() {
        let g = group_colors(&[(0, Rgb::BLACK), (6, Rgb::new(1, 2, 3))], 16);
        assert_eq!(g, vec![(Rgb::new(1, 2, 3), vec![6])]);
        let p = per_key(&group_colors(&[(0, Rgb::BLACK)], 16));
        assert_eq!(p.len(), 1);
        assert_eq!(&p[0][1..5], &[0x88, 1, 0, 0x10]);
    }

    #[test]
    fn many_colours_are_merged_to_the_budget() {
        let leds: Vec<(u8, Rgb)> = (0..99u8).map(|i| (i, Rgb::from_hsv(i as f32 * 3.6, 1.0, 1.0))).collect();
        let g = group_colors(&leds, 12);
        assert!(g.len() <= 12);
        assert_eq!(g.iter().map(|(_, l)| l.len()).sum::<usize>(), 99);
    }

    /// The settings block read from the user's F99 (10 packets).
    const F99_CONFIG: [[u8; 14]; 10] = [
        [0x00, 0x03, 0x03, 0x01, 0x00, 0x00, 0x04, 0x04, 0x07, 0x00, 0x11, 0x20, 0x01, 0x00],
        [0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x02, 0x01, 0x00, 0xff, 0x0a, 0x00, 0x00, 0x00],
        [0x02, 0x02, 0x03, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        [0; 14],
        [0x80, 0x8f, 0x04, 0x47, 0x04, 0x47, 0x04, 0x37, 0x02, 0x37, 0x02, 0x07, 0x04, 0x47],
        [0x04, 0x07, 0x04, 0x47, 0x04, 0x47, 0x04, 0x47, 0x00, 0x47, 0x04, 0x47, 0x04, 0x47],
        [0x04, 0x47, 0x02, 0x07, 0x04, 0x47, 0x04, 0x17, 0x04, 0x37, 0x04, 0x37, 0x04, 0x37],
        [0x07, 0x47, 0x07, 0x47, 0x07, 0x44, 0x07, 0x44, 0x07, 0x44, 0x07, 0x44, 0x07, 0x44],
        [0x07, 0x44, 0x07, 0x44, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        [0x5a, 0xa5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    ];

    fn config_replies() -> Vec<Packet> {
        F99_CONFIG
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let len = if i == 9 { 2 } else { 14 };
                let mut h = vec![0x13, 0x44, 10, i as u8, len as u8];
                h.extend_from_slice(&d[..len]);
                reply(&h)
            })
            .collect()
    }

    fn f99_config() -> Config {
        let mut r = Reassembly::new(CMD_READ_CONFIG);
        Config::parse(&config_replies().iter().find_map(|p| r.push(p)).unwrap()).unwrap()
    }

    #[test]
    fn reassembles_and_decodes_settings() {
        let mut r = Reassembly::new(CMD_READ_CONFIG);
        let replies = config_replies();
        // Duplicates and unrelated reports are ignored.
        assert!(r.push(&replies[0]).is_none());
        assert!(r.push(&reply(&[0x13, 0x07, 0x01, 0x00, 0x01, 0x01])).is_none());
        let data = replies.iter().find_map(|p| r.push(p)).expect("complete");
        assert_eq!(data.len(), 128);
        assert!(Config::parse(&data[..127]).is_none());
        let c = Config::parse(&data).expect("magic");
        assert_eq!(c.effect(), 17);
        assert_eq!(c.sleep(), 10); // 300 s
        assert_eq!(c.debounce(), 1);
        assert_eq!(c.effect_settings(1), (4, 4, COLOURFUL));
        assert_eq!(c.effect_settings(17), (4, 1, COLOURFUL));
        assert_eq!(c.effect_settings(11), (0, 4, COLOURFUL));
    }

    #[test]
    fn settings_write_round_trips() {
        let mut c = f99_config();
        c.set_effect(2);
        c.set_effect_settings(2, 3, 1, 0);
        c.set_sleep(4);
        let p = c.packets();
        assert_eq!(p.len(), 10);
        assert_eq!(&p[0][..5], &[0x13, 0x04, 10, 0, 0x0e]);
        assert_eq!(&p[9][..8], &[0x13, 0x04, 10, 9, 0x02, 0x5a, 0xa5, 0]);
        let back: Vec<u8> = p.iter().flat_map(|q| q[5..5 + (q[4] & 0x0f) as usize].to_vec()).collect();
        let c2 = Config::parse(&back).unwrap();
        assert_eq!(c2, c);
        assert_eq!(c2.effect(), 2);
        assert_eq!(c2.0[0x3c..0x3e], [3, 0x10]);
        assert_eq!(c2.sleep(), 4);
    }

    #[test]
    fn parses_battery() {
        assert_eq!(parse_battery(&reply(&[0x13, 0x4a, 0x01, 0x00, 0x02, 87, 0x01])), Some((87, 0x01)));
        assert_eq!(parse_battery(&reply(&[0x13, 0x4a, 0x01, 0x00, 0x02, 100, 0x11])), Some((100, 0x11)));
        assert_eq!(parse_battery(&reply(&[0x13, 0x4a, 0x01, 0x00, 0x01, 87])), None);
        assert_eq!(parse_battery(&request(CMD_BATTERY)), None);
        // Unsolicited event `13 0A 01 00 04 05 <bat> <flags>`.
        assert_eq!(parse_battery(&reply(&[0x13, 0x0a, 0x01, 0x00, 0x04, 0x05, 64, 0x11])), Some((64, 0x11)));
        assert!(charging(0x11) && !charging(0x10) && !charging(0x01));
        assert!(parse_settings_changed(&reply(&[0x13, 0x0a, 0x01, 0x00, 0x02, 0x07, 0x00])));
        assert!(!parse_settings_changed(&reply(&[0x13, 0x0a, 0x01, 0x00, 0x02, 0x02, 0x01])));
    }

    #[test]
    fn acks() {
        assert_eq!(ack(&reply(&[0x13, 0x04, 0x0a, 0x03]), CMD_WRITE_CONFIG, 3), Ack::Ok);
        assert_eq!(ack(&reply(&[0x13, 0x04, 0x0a, 0x02]), CMD_WRITE_CONFIG, 3), Ack::No);
        assert_eq!(ack(&reply(&[0x13, 0x04, 0x8a, 0x03]), CMD_WRITE_CONFIG, 3), Ack::Bad);
    }
}
