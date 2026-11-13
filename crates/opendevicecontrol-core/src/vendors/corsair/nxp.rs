//! Corsair peripherals using the legacy NXP ("CUE") protocol: the K55 RGB
//! (hardware-verified), plus — experimental, untested on hardware — the
//! per-key K65/K68/K70/K95/STRAFE RGB keyboards, the M65, SABRE, SCIMITAR,
//! HARPOON, GLAIVE, IRONCLAW, NIGHTSWORD and wired DARK CORE mice, the MM800
//! POLARIS mousepad and the ST100 headset stand. The model table is in
//! `nxp/models.rs`; per-key keyboard layouts are generated into
//! `nxp/layouts.rs` by `tools/gen_nxp_layouts.py`.
//!
//! Protocol (64-byte HID reports, no report IDs, first byte = opcode):
//! * commands go to the vendor interface (usage page 0xFFC2), interface 1 on
//!   keyboards and mice, interface 0 on the MM800/ST100; replies to `0E`
//!   queries come back on the same handle (as feature reports on Windows);
//! * key/button input arrives on interface 0 as report `03` + bitmap.
//!
//! | Bytes | Meaning |
//! |---|---|
//! | `0E 01` | device info; reply `[8..9]` firmware (LE), `[16]` poll interval ms, `[20]` type, `[23]` layout (0 ANSI, 1 ISO, 2 ABNT, 3 JIS) |
//! | `07 04 02` / `07 04 01` | software / hardware mode |
//! | `07 05 02 00 x` | software lighting (x: 3 keyboards/stand, 1 mice, 4 mousepads) |
//! | `07 05 08 00 n` | K55/K68/K70 MK.2/K95 Platinum: needed for colour packets; STRAFE: side lights on/off |
//! | `07 40 n 00 (key, mode)×n` | key input routing: `0x80` HID, `0x40` report to host only, `0xC0` both |
//! | `07 05 02 00 03` | commit key routing |
//! | `07 25 00 00 R G B ×3` | K55: colours of the 3 lighting zones |
//! | `7F i len 00 data`, `07 28 ch 03 f` | per-key colour, 24-bit: 168 bytes per channel in 60/60/48-byte packets, commit per channel (f = 2 after blue) |
//! | `7F i len 00 data`, `07 27 00 00 D8` | per-key colour, 9-bit (K65/K70/K95 RGB): 3-bit inverted nibbles, R/G/B planes of 72 bytes, 60/60/60/36 |
//! | `07 22 n 01 (id R G B)×n` | mouse zone colours (ids: 1 front, 2 logo, 3 DPI, 4 wheel, 5 side keys, 6 side) |
//! | `07 22 n 00 (R G B)×n` | mousepad / stand zone colours |
//! | `07 AA 00 00 mask 07 00 00 64 RGB 000000 5-z` | DARK CORE: static colour of hardware zone z |
//! | `07 13 D0+s 00 xy Xlo Xhi Ylo Yhi [RGB]` | mouse DPI of stage s (0 = sniper) |
//! | `07 13 05 00 mask` / `07 13 02 00 s` | enabled DPI stages / select stage |
//! | `07 13 04 00 on 05` | angle snapping |
//! | `07 0A 00 00 ms` | poll interval (1, 2, 4, 8 ms); the device reconnects |
//! | `07 48 A0|flags` | key locks: +1 Win, +2 Alt+Tab, +4 Alt+F4, +8 Shift+Tab |
//! | `07 05 09 00 0|1` | K55 Win-lock indicator LED |
//!
//! References (all GPL): ckb-next (`usb_nxp.c`, `usb.h`, `device_keyboard.c`,
//! `device_mouse.c`, `led_keyboard.c`, `led_mouse.c`, `led_mousepad.c`,
//! `led_wireless.c`, `dpi.c`, `keymap.c`, GUI `keymap.cpp`), OpenRGB
//! (`CorsairPeripheralController`), OpenLinkHub (`devices/k55`, `k70mk2`,
//! `k95platinum`, `m65prorgb`, `scimitarprorgb`, `mm800`, `st100`…).

mod layouts;
mod models;

pub use models::MODELS;

use crate::model::*;
use crate::{Device, Error, Result};
use hidapi::HidDevice;
use std::time::{Duration, Instant};

const PACKET: usize = 64;

/// Key routing modes (ckb-next `IN_HID` / `IN_CORSAIR`).
pub const ROUTE_HID: u8 = 0x80;
pub const ROUTE_HOST: u8 = 0x40;
pub const ROUTE_BOTH: u8 = ROUTE_HID | ROUTE_HOST;

/// Hardware-mode actions for keys with a built-in function (ckb-next).
pub const ACT_LIGHT: u8 = 0x01;
pub const ACT_LOCK: u8 = 0x08;
pub const ACT_MR: u8 = 0x09;
pub const ACT_M1: u8 = 0x0a;
pub const ACT_M2: u8 = 0x0b;
pub const ACT_M3: u8 = 0x0c;

/// Number of key slots in the routing table.
const KEY_SLOTS: usize = 152;
/// Bytes per colour channel in per-key packets (ckb-next sends 168).
const LED_SLOTS: usize = 168;
/// LEDs covered by the 9-bit format.
const LED_SLOTS_9: usize = 144;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecialKey {
    /// Bit in the Corsair input bitmap (= routing-table index on keyboards).
    pub bit: u8,
    pub id: &'static str,
    pub name: &'static str,
    pub assignable: bool,
    /// Action the firmware performs for this key in hardware mode.
    pub hw_action: Option<u8>,
    /// Routing while opendevicecontrol runs (keyboards).
    pub route: u8,
}

/// One LED of a per-key keyboard: lighting index, position in ckb-next's
/// GUI layout (x grows to the right), label, and layouts it exists in
/// (bit mask, see [`KbLayout`]).
#[derive(Debug, Clone, Copy)]
pub struct KbLed {
    pub led: u8,
    pub x: i16,
    pub y: i16,
    pub name: &'static str,
    pub layouts: u8,
}

/// A fixed lighting zone: device LED/zone indices in left-to-right order.
#[derive(Debug, Clone, Copy)]
pub struct ZoneDef {
    pub id: &'static str,
    pub name: &'static str,
    pub leds: &'static [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// 8 bits per channel (`07 28`).
    Full,
    /// 3 bits per channel (`07 27`), older K65/K70/K95 RGB.
    Nine,
}

#[derive(Debug, Clone, Copy)]
pub enum Lighting {
    /// K55: three zones of one colour each, `07 25`.
    ThreeZone,
    /// Per-key keyboard.
    PerKey { leds: &'static [KbLed], depth: ColorDepth },
    /// `07 22 n 01`, zones identified by id.
    Mouse,
    /// DARK CORE: per-zone static "hardware animation" packets.
    DarkCore,
    /// Mousepad / stand: `07 22 count 00`, `control` is the `07 05 02 00 x` byte.
    Pad { count: u8, control: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinLock {
    None,
    /// `07 48` key locks plus the K55's `07 05 09` indicator LED.
    Performance,
    /// `07 48` key locks only (the lock key's LED is a normal per-key LED).
    Lock48,
}

/// Physical keyboard layout reported in the device info.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KbLayout {
    Ansi = 1,
    Iso = 2,
    Abnt = 4,
    Jis = 8,
}

impl KbLayout {
    pub fn from_info(byte: u8) -> Self {
        match byte {
            1 => KbLayout::Iso,
            2 => KbLayout::Abnt,
            3 => KbLayout::Jis,
            _ => KbLayout::Ansi, // 0 ANSI, 4 Korean (ANSI-shaped)
        }
    }
}

pub struct NxpModel {
    pub pid: u16,
    pub name: &'static str,
    pub slug: &'static str,
    pub kind: DeviceKind,
    /// Interface carrying the 0xFFC2 command collection; -1 = any.
    pub command_interface: i32,
    /// Interface whose vendor collections carry the `03` input report.
    pub input_interface: Option<i32>,
    /// Fixed zones (all lighting types except per-key).
    pub zones: &'static [ZoneDef],
    pub lighting: Lighting,
    pub keys: &'static [SpecialKey],
    pub win_lock: WinLock,
    /// Send `07 05 08 00 01` after entering software mode (OpenRGB).
    pub lighting_08: bool,
    /// STRAFE side lights (`07 05 08 00 on`), exposed as a toggle.
    pub side_lights: bool,
    /// Poll rate adjustable with `07 0A` (ckb-next `SUPPORTS_ADJRATE`).
    pub poll_rate: bool,
    /// Mice: highest DPI; 0 = no DPI control.
    pub max_dpi: u16,
    /// The only DPI button cycles through the stages.
    pub dpi_cycle: bool,
    /// DPI packets carry the indicator colour (ckb-next `NXP_RGB_IN_DPI_PKT`).
    pub dpi_rgb: bool,
}

pub const KEY_BRIGHTNESS: &str = "brightness";
pub const KEY_WINLOCK: &str = "win_lock";
pub const KEY_DPI_UP: &str = "dpi_up";
pub const KEY_DPI_DOWN: &str = "dpi_down";
pub const KEY_SNIPER: &str = "sniper";

pub fn model_for_pid(pid: u16) -> Option<&'static NxpModel> {
    MODELS.iter().find(|m| m.pid == pid)
}

/// Whether `d` is the command collection of an NXP device (discovery).
pub fn is_command_interface(m: &NxpModel, d: &hidapi::DeviceInfo) -> bool {
    (m.command_interface < 0 || d.interface_number() == m.command_interface) && d.usage_page() == 0xffc2
}

/// Whether `i` is an input collection belonging to command collection `cmd`,
/// with `units` units of this model plugged in (see [`super::same_unit`]).
pub fn is_input_interface(m: &NxpModel, cmd: &hidapi::DeviceInfo, i: &hidapi::DeviceInfo, units: usize) -> bool {
    m.input_interface.is_some_and(|n| i.interface_number() == n)
        && i.product_id() == cmd.product_id()
        && i.usage_page() >= 0xff00
        && super::same_unit(super::serial_of(i), super::serial_of(cmd), units)
        && i.path() != cmd.path()
}

pub const CTRL_WIN_LOCK: &str = "win_lock";
pub const CTRL_LOCK_ALT_TAB: &str = "lock_alt_tab";
pub const CTRL_LOCK_ALT_F4: &str = "lock_alt_f4";
pub const CTRL_LOCK_SHIFT_TAB: &str = "lock_shift_tab";
pub const CTRL_SIDE_LIGHTS: &str = "side_lights";
pub const CTRL_POLL_RATE: &str = "poll_rate";
pub const CTRL_DPI_STAGE: &str = "dpi_stage";
pub const CTRL_SNIPER_DPI: &str = "dpi_sniper";
pub const CTRL_ANGLE_SNAP: &str = "angle_snap";

/// DPI stages besides the sniper stage 0 (ckb-next `DPI_COUNT` = 6).
const DPI_STAGES: usize = 5;
const DEFAULT_DPI: [u16; DPI_STAGES + 1] = [400, 800, 1500, 3000, 6000, 9000];
const POLL_RATES: [(u8, &str); 4] = [(1, "1000 Hz"), (2, "500 Hz"), (4, "250 Hz"), (8, "125 Hz")];

fn dpi_ctrl(stage: usize) -> String {
    format!("dpi_{stage}")
}

// ------------------------------------------------------------ packets
// Pure encoders: payloads without the hidapi report-id byte.

/// K55 zone colours.
pub(crate) fn three_zone_packet(colors: &[Rgb]) -> Vec<u8> {
    let mut p = vec![0x07, 0x25, 0x00, 0x00];
    for i in 0..3 {
        let c = colors.get(i).copied().unwrap_or_default();
        p.extend_from_slice(&[c.r, c.g, c.b]);
    }
    p
}

/// 24-bit per-key colours: per channel three `7F` stream packets (60/60/48
/// bytes, data from byte 4) and a `07 28` commit (ckb-next `makergb_full`).
pub(crate) fn per_key_24_packets(r: &[u8; LED_SLOTS], g: &[u8; LED_SLOTS], b: &[u8; LED_SLOTS]) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(12);
    for (ch, plane) in [r, g, b].into_iter().enumerate() {
        for (i, (start, len)) in [(0usize, 60usize), (60, 60), (120, 48)].into_iter().enumerate() {
            let mut p = vec![0x7f, i as u8 + 1, len as u8, 0x00];
            p.extend_from_slice(&plane[start..start + len]);
            out.push(p);
        }
        out.push(vec![0x07, 0x28, ch as u8 + 1, 0x03, if ch == 2 { 0x02 } else { 0x01 }]);
    }
    out
}

/// 9-bit per-key colours (ckb-next `makergb_512` with `quantize8to3`).
pub(crate) fn per_key_9_packets(r: &[u8; LED_SLOTS], g: &[u8; LED_SLOTS], b: &[u8; LED_SLOTS]) -> Vec<Vec<u8>> {
    let mut data = Vec::with_capacity(216);
    for plane in [r, g, b] {
        for i in (0..LED_SLOTS_9).step_by(2) {
            let lo = 7 - (plane[i] >> 5);
            let hi = 7 - (plane[i + 1] >> 5);
            data.push(hi << 4 | lo);
        }
    }
    let mut out = Vec::with_capacity(5);
    for (i, (start, len)) in [(0usize, 60usize), (60, 60), (120, 60), (180, 36)].into_iter().enumerate() {
        let mut p = vec![0x7f, i as u8 + 1, len as u8, 0x00];
        p.extend_from_slice(&data[start..start + len]);
        out.push(p);
    }
    out.push(vec![0x07, 0x27, 0x00, 0x00, 0xd8]);
    out
}

/// Mouse zone colours: `(zone id, colour)` pairs.
pub(crate) fn mouse_packet(zones: &[(u8, Rgb)]) -> Vec<u8> {
    let mut p = vec![0x07, 0x22, zones.len() as u8, 0x01];
    for (id, c) in zones {
        p.extend_from_slice(&[*id, c.r, c.g, c.b]);
    }
    p
}

/// Mousepad / stand colours, device zone order.
pub(crate) fn pad_packet(colors: &[Rgb]) -> Vec<u8> {
    let mut p = vec![0x07, 0x22, colors.len() as u8, 0x00];
    for c in colors {
        p.extend_from_slice(&[c.r, c.g, c.b]);
    }
    p
}

/// DARK CORE static colour for hardware zone 0..=2 (ckb-next `apply_hwanim`).
pub(crate) fn dark_core_packet(zone: u8, c: Rgb) -> Vec<u8> {
    vec![0x07, 0xaa, 0x00, 0x00, 1 << zone, 0x07, 0x00, 0x00, 100, c.r, c.g, c.b, 0, 0, 0, 5 - zone]
}

/// Key routing table, 30 keys per packet.
pub(crate) fn key_table_packets(table: &[u8; KEY_SLOTS]) -> Vec<Vec<u8>> {
    table
        .chunks(30)
        .enumerate()
        .map(|(chunk_no, chunk)| {
            let mut p = vec![0x07, 0x40, chunk.len() as u8, 0x00];
            for (i, mode) in chunk.iter().enumerate() {
                p.push((chunk_no * 30 + i) as u8);
                p.push(*mode);
            }
            p
        })
        .collect()
}

/// Software-mode routing: normal keys to the OS, special keys as the model says.
pub(crate) fn software_key_table(model: &NxpModel) -> [u8; KEY_SLOTS] {
    let mut table = [ROUTE_HID; KEY_SLOTS];
    for k in model.keys {
        table[k.bit as usize] = k.route;
    }
    table
}

/// Hardware-mode routing: special keys get their built-in functions back.
pub(crate) fn hardware_key_table(model: &NxpModel) -> [u8; KEY_SLOTS] {
    let mut table = [ROUTE_HID; KEY_SLOTS];
    for k in model.keys {
        if let Some(a) = k.hw_action {
            table[k.bit as usize] = a;
        }
    }
    table
}

/// `07 48` key-lock flags.
pub(crate) fn key_lock_flags(win: bool, alt_tab: bool, alt_f4: bool, shift_tab: bool) -> u8 {
    let mut flags = 0xa0u8;
    if win {
        flags |= 0x01;
        if alt_tab {
            flags |= 0x02;
        }
        if alt_f4 {
            flags |= 0x04;
        }
        if shift_tab {
            flags |= 0x08;
        }
    }
    flags
}

/// DPI of one stage (ckb-next `updatedpi`); `rgb` for `dpi_rgb` models.
pub(crate) fn dpi_stage_packet(stage: u8, dpi: u16, rgb: Option<Rgb>) -> Vec<u8> {
    let [lo, hi] = dpi.to_le_bytes();
    let mut p = vec![0x07, 0x13, 0xd0 | stage, 0x00, 0x00, lo, hi, lo, hi];
    if let Some(c) = rgb {
        p.extend_from_slice(&[c.r, c.g, c.b]);
    }
    p
}

/// LEDs of a per-key model in the given layout, left to right.
pub(crate) fn layout_leds(leds: &'static [KbLed], layout: KbLayout) -> impl Iterator<Item = &'static KbLed> {
    leds.iter().filter(move |l| l.layouts & layout as u8 != 0)
}

/// Lighting zones and, per frame position, the device LED index.
fn build_zones(model: &NxpModel, layout: KbLayout) -> (Vec<LightZone>, Vec<u8>) {
    match model.lighting {
        Lighting::PerKey { leds, .. } => {
            // Light-bar LEDs (K95 Platinum, index >= 0x90) form their own zone.
            let (keys, bar): (Vec<&KbLed>, Vec<&KbLed>) = layout_leds(leds, layout).partition(|l| l.led < 0x90);
            let mut zones = vec![LightZone { id: "keys".into(), name: "Keys".into(), leds: keys.len() }];
            let mut map: Vec<u8> = keys.iter().map(|l| l.led).collect();
            if !bar.is_empty() {
                zones.push(LightZone { id: "light_bar".into(), name: "Light bar".into(), leds: bar.len() });
                map.extend(bar.iter().map(|l| l.led));
            }
            (zones, map)
        }
        _ => {
            let zones = model
                .zones
                .iter()
                .map(|z| LightZone { id: z.id.into(), name: z.name.into(), leds: z.leds.len() })
                .collect();
            let map = model.zones.iter().flat_map(|z| z.leds.iter().copied()).collect();
            (zones, map)
        }
    }
}

// ------------------------------------------------------------ device

pub struct NxpDevice {
    cmd: HidDevice,
    inputs: Vec<HidDevice>,
    info: DeviceInfo,
    model: &'static NxpModel,
    layout: KbLayout,
    zones: Vec<LightZone>,
    /// Device LED / zone index for each LED of a frame.
    led_map: Vec<u8>,
    pressed: Vec<bool>,
    win_lock: bool,
    lock_alt_tab: bool,
    lock_alt_f4: bool,
    lock_shift_tab: bool,
    side_lights: bool,
    /// Poll interval in ms, if known.
    poll_ms: Option<u8>,
    dpi: [u16; DPI_STAGES + 1],
    dpi_rgb: [Rgb; DPI_STAGES + 1],
    /// Selected DPI stage 1..=5.
    dpi_stage: u8,
    sniper_held: bool,
    angle_snap: bool,
    last_colors: Vec<Rgb>,
    last_light_write: Option<Instant>,
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

impl NxpDevice {
    pub fn open(
        cmd: HidDevice,
        inputs: Vec<HidDevice>,
        model: &'static NxpModel,
        id: String,
        serial: Option<String>,
    ) -> Result<Self> {
        let mut d = Self {
            cmd,
            inputs,
            info: DeviceInfo {
                id,
                kind: model.kind,
                manufacturer: "Corsair".into(),
                model: model.name.into(),
                vendor_string: None,
                product_string: None,
                usb_vid: crate::CORSAIR_VID,
                usb_pid: model.pid,
                serial,
                firmware: None,
            },
            model,
            layout: KbLayout::Ansi,
            zones: Vec::new(),
            led_map: Vec::new(),
            pressed: vec![false; model.keys.len()],
            win_lock: false,
            lock_alt_tab: true,
            lock_alt_f4: true,
            lock_shift_tab: false,
            side_lights: true,
            poll_ms: None,
            dpi: DEFAULT_DPI,
            dpi_rgb: [Rgb::WHITE; DPI_STAGES + 1],
            dpi_stage: 1,
            sniper_held: false,
            angle_snap: false,
            last_colors: Vec::new(),
            last_light_write: None,
        };
        if let Some(info) = d.read_info() {
            let fw = u16::from_le_bytes([info[8], info[9]]);
            d.info.firmware = Some(format!("{}.{:02x}", fw >> 12, fw & 0xff));
            if matches!(info[16], 1 | 2 | 4 | 8) {
                d.poll_ms = Some(info[16]);
            }
            d.layout = KbLayout::from_info(info[23]);
        }
        (d.zones, d.led_map) = build_zones(model, d.layout);
        if model.max_dpi > 0 {
            d.read_dpi();
        }
        d.enter_software_mode()?;
        Ok(d)
    }

    fn write(&self, payload: &[u8]) -> Result<()> {
        let mut out = [0u8; PACKET + 1];
        out[1..1 + payload.len()].copy_from_slice(payload);
        self.cmd.write(&out)?;
        Ok(())
    }

    /// `0E …` query. Returns the 64 reply bytes once the reply echoes the
    /// request's first `req.len().min(4)` bytes and is at least `min_len` long.
    ///
    /// On Windows the K55 answers only through HID feature reports (verified
    /// on hardware); elsewhere replies may also arrive as input reports.
    fn query(&self, req: &[u8], min_len: usize) -> Option<[u8; PACKET]> {
        let echo = req.len().min(4);
        let mut fr = [0u8; PACKET + 1];
        fr[1..1 + req.len()].copy_from_slice(req);
        if self.cmd.send_feature_report(&fr).is_ok() {
            let mut buf = [0u8; PACKET + 1];
            if let Ok(n) = self.cmd.get_feature_report(&mut buf) {
                // Byte 0 is the report ID.
                if n > echo && buf[1..=echo] == req[..echo] {
                    let mut out = [0u8; PACKET];
                    out.copy_from_slice(&buf[1..=PACKET]);
                    return Some(out);
                }
            }
        }
        self.write(req).ok()?;
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            let mut buf = [0u8; PACKET];
            match self.cmd.read_timeout(&mut buf, 50) {
                Ok(n) if n >= min_len && buf[..echo] == req[..echo] => return Some(buf),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
        None
    }

    fn read_info(&self) -> Option<[u8; PACKET]> {
        // Some keyboards answer with zeros right after a cold boot; retry.
        for _ in 0..5 {
            if let Some(r) = self.query(&[0x0e, 0x01], 24) {
                // [8..9] firmware version, little endian; K55 3.09 = 0x3009.
                if u16::from_le_bytes([r[8], r[9]]) != 0 {
                    return Some(r);
                }
            }
            pause(50);
        }
        None
    }

    /// Load the stored DPI stages (ckb-next `loaddpi`); keep defaults if the
    /// mouse doesn't answer.
    fn read_dpi(&mut self) {
        for s in 0..=DPI_STAGES {
            if let Some(r) = self.query(&[0x0e, 0x13, 0xd0 | s as u8, 0x01], 12) {
                let x = u16::from_le_bytes([r[5], r[6]]);
                if (50..=self.model.max_dpi).contains(&x) {
                    self.dpi[s] = x;
                    self.dpi_rgb[s] = Rgb::new(r[9], r[10], r[11]);
                }
            }
        }
        if let Some(r) = self.query(&[0x0e, 0x13, 0x02, 0x01], 5) {
            if (1..=DPI_STAGES as u8).contains(&r[4]) {
                self.dpi_stage = r[4];
            }
        }
        if let Some(r) = self.query(&[0x0e, 0x13, 0x04, 0x01], 5) {
            self.angle_snap = r[4] != 0;
        }
    }

    fn is_keyboard(&self) -> bool {
        self.model.kind == DeviceKind::Keyboard
    }

    /// Take over lighting and route keys: everything to the OS as normal,
    /// special keys only to us. Sequence as OpenRGB uses for the K55/K95
    /// Platinum family: special-function software mode, lighting control
    /// software mode (`07 05 02 00 03`; byte 5 = 3 for keyboards), then
    /// `07 05 08 00 01` — without it the firmware keeps running its own
    /// effect and ignores colour packets.
    fn enter_software_mode(&mut self) -> Result<()> {
        self.write(&[0x07, 0x04, 0x02])?;
        pause(30);
        if !self.is_keyboard() {
            return self.enter_software_mode_peripheral();
        }
        self.write(&[0x07, 0x05, 0x02, 0x00, 0x03])?;
        pause(30);
        if self.model.lighting_08 || self.model.side_lights {
            let on = if self.model.side_lights { self.side_lights as u8 } else { 1 };
            self.write(&[0x07, 0x05, 0x08, 0x00, on])?;
            pause(30);
        }
        self.write_key_table(&software_key_table(self.model))?;
        self.apply_key_locks()?;
        self.last_colors.clear();
        Ok(())
    }

    /// Mice, mousepad, stand (after `07 04 02`).
    fn enter_software_mode_peripheral(&mut self) -> Result<()> {
        if self.model.kind == DeviceKind::Mouse {
            // Button input selection exactly as ckb-next `setactive_mouse`
            // sends it (the pair list is filled only after sending, so the
            // body is empty).
            self.write(&[0x07, 0x40, 20, 0x00])?;
            pause(5);
        }
        let control = match self.model.lighting {
            Lighting::Pad { control, .. } => control,
            _ => 0x01,
        };
        self.write(&[0x07, 0x05, 0x02, 0x00, control])?;
        pause(30);
        if self.model.max_dpi > 0 {
            self.apply_dpi()?;
        }
        self.last_colors.clear();
        Ok(())
    }

    fn write_key_table(&self, table: &[u8; KEY_SLOTS]) -> Result<()> {
        for p in key_table_packets(table) {
            self.write(&p)?;
            pause(5);
        }
        self.write(&[0x07, 0x05, 0x02, 0x00, 0x03])?;
        pause(30);
        Ok(())
    }

    fn apply_key_locks(&self) -> Result<()> {
        if self.model.win_lock == WinLock::None {
            return Ok(());
        }
        let flags = key_lock_flags(self.win_lock, self.lock_alt_tab, self.lock_alt_f4, self.lock_shift_tab);
        self.write(&[0x07, 0x48, flags])?;
        pause(5);
        if self.model.win_lock == WinLock::Performance {
            self.write(&[0x07, 0x05, 0x09, 0x00, self.win_lock as u8, 0x00])?;
        }
        Ok(())
    }

    fn dpi_color(&self, stage: usize) -> Option<Rgb> {
        self.model.dpi_rgb.then_some(self.dpi_rgb[stage])
    }

    /// Write all stages, enable them all and select the current one.
    fn apply_dpi(&self) -> Result<()> {
        for s in 0..=DPI_STAGES {
            self.write(&dpi_stage_packet(s as u8, self.dpi[s], self.dpi_color(s)))?;
            pause(5);
        }
        self.write(&[0x07, 0x13, 0x05, 0x00, (1 << (DPI_STAGES + 1)) - 1])?;
        pause(5);
        self.select_dpi_stage()
    }

    fn select_dpi_stage(&self) -> Result<()> {
        let stage = if self.sniper_held { 0 } else { self.dpi_stage };
        self.write(&[0x07, 0x13, 0x02, 0x00, stage])
    }

    fn step_dpi(&mut self, up: bool) {
        let n = DPI_STAGES as u8;
        let stage = match (up, self.model.dpi_cycle) {
            (true, true) => self.dpi_stage % n + 1,
            (true, false) => (self.dpi_stage + 1).min(n),
            (false, _) => self.dpi_stage.saturating_sub(1).max(1),
        };
        if let Err(e) = set_if_applied(self, |d| &mut d.dpi_stage, stage, |d| d.select_dpi_stage()) {
            log::warn!("{}: DPI: {e}", self.info.id);
        }
    }

    fn toggle(&self, id: &str, name: &str, value: bool, help: &str) -> Control {
        Control {
            id: id.into(),
            name: name.into(),
            kind: ControlKind::Toggle,
            value: ControlValue::Bool(value),
            help: Some(help.into()),
        }
    }

    fn colors_for<'a>(&'a self, colors: &'a [Rgb]) -> impl Iterator<Item = (u8, Rgb)> + 'a {
        self.led_map.iter().enumerate().map(|(i, &led)| (led, colors.get(i).copied().unwrap_or_default()))
    }

    fn send_lights(&self, colors: &[Rgb]) -> Result<()> {
        match self.model.lighting {
            Lighting::ThreeZone => self.write(&three_zone_packet(colors)),
            Lighting::PerKey { depth, .. } => {
                let (mut r, mut g, mut b) = ([0u8; LED_SLOTS], [0u8; LED_SLOTS], [0u8; LED_SLOTS]);
                for (led, c) in self.colors_for(colors) {
                    let i = led as usize;
                    if i < LED_SLOTS {
                        (r[i], g[i], b[i]) = (c.r, c.g, c.b);
                    }
                }
                // Inter-packet delays after ckb-next `nxp_kb_setfps`.
                let (packets, gap) = match depth {
                    ColorDepth::Full => (per_key_24_packets(&r, &g, &b), 1),
                    ColorDepth::Nine => (per_key_9_packets(&r, &g, &b), 3),
                };
                for p in packets {
                    self.write(&p)?;
                    pause(gap);
                }
                Ok(())
            }
            Lighting::Mouse => {
                let zones: Vec<(u8, Rgb)> = self.colors_for(colors).collect();
                self.write(&mouse_packet(&zones))
            }
            Lighting::DarkCore => {
                for (i, (zone, c)) in self.colors_for(colors).enumerate() {
                    if self.last_colors.get(i) != Some(&c) || self.last_light_write.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
                        self.write(&dark_core_packet(zone, c))?;
                    }
                }
                Ok(())
            }
            Lighting::Pad { count, .. } => {
                let mut dev = vec![Rgb::BLACK; count as usize];
                for (led, c) in self.colors_for(colors) {
                    if let Some(slot) = dev.get_mut(led as usize) {
                        *slot = c;
                    }
                }
                self.write(&pad_packet(&dev))
            }
        }
    }
}

/// Set the cached `field` of `d` to `v` and run `apply`, which sends the
/// cached state to the device; if that fails, the old value is put back so
/// the cache (and what is reported and saved from it) stays what the device
/// confirmed.
fn set_if_applied<S, T>(d: &mut S, field: fn(&mut S) -> &mut T, v: T, apply: impl FnOnce(&S) -> Result<()>) -> Result<()> {
    let old = std::mem::replace(field(d), v);
    let r = apply(d);
    if r.is_err() {
        *field(d) = old;
    }
    r
}

impl Device for NxpDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        let mut controls = Vec::new();
        if self.model.win_lock != WinLock::None {
            controls.push(self.toggle(
                CTRL_WIN_LOCK,
                "Win lock",
                self.win_lock,
                "Disables the Windows key (and the shortcuts below). Also toggled by the keyboard's lock key.",
            ));
            controls.push(self.toggle(CTRL_LOCK_ALT_TAB, "Win lock also blocks Alt+Tab", self.lock_alt_tab, ""));
            controls.push(self.toggle(CTRL_LOCK_ALT_F4, "Win lock also blocks Alt+F4", self.lock_alt_f4, ""));
            controls.push(self.toggle(CTRL_LOCK_SHIFT_TAB, "Win lock also blocks Shift+Tab", self.lock_shift_tab, ""));
        }
        if self.model.side_lights {
            controls.push(self.toggle(CTRL_SIDE_LIGHTS, "Side lights", self.side_lights, "The light strips on the sides."));
        }
        if self.model.max_dpi > 0 {
            let dpi = |id: String, name: String, v: u16| Control {
                id,
                name,
                kind: ControlKind::Number { min: 100.0, max: self.model.max_dpi as f64, step: 50.0, unit: "DPI".into() },
                value: ControlValue::Number(v as f64),
                help: None,
            };
            controls.push(Control {
                id: CTRL_DPI_STAGE.into(),
                name: "DPI stage".into(),
                kind: ControlKind::Choice { options: (1..=DPI_STAGES).map(|s| s.to_string()).collect() },
                value: ControlValue::Choice(self.dpi_stage.to_string()),
                help: Some("Also changed by the mouse's DPI buttons.".into()),
            });
            for s in 1..=DPI_STAGES {
                controls.push(dpi(dpi_ctrl(s), format!("DPI stage {s}"), self.dpi[s]));
            }
            controls.push(dpi(CTRL_SNIPER_DPI.into(), "Sniper DPI".into(), self.dpi[0]));
            controls.push(self.toggle(CTRL_ANGLE_SNAP, "Angle snapping", self.angle_snap, "Straightens nearly straight movements."));
        }
        if self.model.poll_rate {
            controls.push(Control {
                id: CTRL_POLL_RATE.into(),
                name: "Polling rate".into(),
                kind: ControlKind::Choice { options: POLL_RATES.iter().map(|(_, n)| (*n).to_string()).collect() },
                value: ControlValue::Choice(
                    POLL_RATES.iter().find(|(ms, _)| Some(*ms) == self.poll_ms).map_or("1000 Hz", |(_, n)| n).to_string(),
                ),
                help: Some("The device briefly reconnects after a change.".into()),
            });
        }
        Ok(Snapshot { sensors: Vec::new(), controls })
    }

    fn poll_interval(&self) -> Option<Duration> {
        Some(Duration::from_secs(5))
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        let invalid = || Error::InvalidValue(id.into());
        match (id, value) {
            (CTRL_WIN_LOCK | CTRL_LOCK_ALT_TAB | CTRL_LOCK_ALT_F4 | CTRL_LOCK_SHIFT_TAB, ControlValue::Bool(v))
                if self.model.win_lock != WinLock::None =>
            {
                let field: fn(&mut Self) -> &mut bool = match id {
                    CTRL_WIN_LOCK => |d| &mut d.win_lock,
                    CTRL_LOCK_ALT_TAB => |d| &mut d.lock_alt_tab,
                    CTRL_LOCK_ALT_F4 => |d| &mut d.lock_alt_f4,
                    _ => |d| &mut d.lock_shift_tab,
                };
                let r = set_if_applied(self, field, *v, |d| d.apply_key_locks());
                if r.is_err() {
                    // The lock flags are two writes: put back what we
                    // still believe is set (best effort).
                    let _ = self.apply_key_locks();
                }
                r
            }
            // The cached value changes only once the device took it.
            (CTRL_SIDE_LIGHTS, ControlValue::Bool(v)) if self.model.side_lights => {
                self.write(&[0x07, 0x05, 0x08, 0x00, *v as u8])?;
                self.side_lights = *v;
                Ok(())
            }
            (CTRL_ANGLE_SNAP, ControlValue::Bool(v)) if self.model.max_dpi > 0 => {
                self.write(&[0x07, 0x13, 0x04, 0x00, *v as u8, 0x05])?;
                self.angle_snap = *v;
                Ok(())
            }
            (CTRL_DPI_STAGE, ControlValue::Choice(s)) if self.model.max_dpi > 0 => {
                let s: u8 = s.parse().map_err(|_| invalid())?;
                if !(1..=DPI_STAGES as u8).contains(&s) {
                    return Err(invalid());
                }
                set_if_applied(self, |d| &mut d.dpi_stage, s, |d| d.select_dpi_stage())
            }
            (_, ControlValue::Number(v)) if self.model.max_dpi > 0 => {
                let stage = if id == CTRL_SNIPER_DPI {
                    0
                } else {
                    (1..=DPI_STAGES).find(|s| dpi_ctrl(*s) == id).ok_or_else(|| Error::UnknownControl(id.into()))?
                };
                let dpi = ((v / 50.0).round() * 50.0).clamp(100.0, self.model.max_dpi as f64) as u16;
                self.write(&dpi_stage_packet(stage as u8, dpi, self.dpi_color(stage)))?;
                self.dpi[stage] = dpi;
                Ok(())
            }
            (CTRL_POLL_RATE, ControlValue::Choice(c)) if self.model.poll_rate => {
                let (ms, _) = POLL_RATES.iter().find(|(_, n)| n == c).ok_or_else(invalid)?;
                self.write(&[0x07, 0x0a, 0x00, 0x00, *ms])?;
                self.poll_ms = Some(*ms);
                Ok(())
            }
            // A known control with a value of the wrong type.
            (
                CTRL_WIN_LOCK | CTRL_LOCK_ALT_TAB | CTRL_LOCK_ALT_F4 | CTRL_LOCK_SHIFT_TAB | CTRL_SIDE_LIGHTS | CTRL_ANGLE_SNAP
                | CTRL_DPI_STAGE | CTRL_POLL_RATE,
                _,
            ) => Err(invalid()),
            _ => Err(Error::UnknownControl(id.into())),
        }
    }

    fn light_zones(&self) -> Vec<LightZone> {
        self.zones.clone()
    }

    fn set_lights(&mut self, colors: &[Rgb]) -> Result<()> {
        // Skip identical frames, but refresh once a second in case another
        // program (or a replug) changed the LEDs.
        let fresh = self.last_light_write.is_some_and(|t| t.elapsed() < Duration::from_secs(1));
        if colors == self.last_colors.as_slice() && fresh {
            return Ok(());
        }
        self.send_lights(colors)?;
        self.last_colors = colors.to_vec();
        self.last_light_write = Some(Instant::now());
        Ok(())
    }

    fn input_keys(&self) -> Vec<InputKey> {
        self.model
            .keys
            .iter()
            .map(|k| InputKey { id: k.id.into(), name: k.name.into(), assignable: k.assignable })
            .collect()
    }

    fn poll_input(&mut self) -> Vec<InputEvent> {
        let mut events = Vec::new();
        let mut buf = [0u8; PACKET];
        let min_len = if self.is_keyboard() { 20 } else { 4 };
        for i in 0..self.inputs.len() {
            for _ in 0..32 {
                let n = match self.inputs[i].read_timeout(&mut buf, 0) {
                    Ok(n) if n > 0 => n,
                    _ => break,
                };
                // Report 03 = Corsair key bitmap, bit k at byte 1 + k/8.
                if buf[0] != 0x03 || n < min_len {
                    continue;
                }
                for (ki, k) in self.model.keys.iter().enumerate() {
                    let byte = 1 + k.bit as usize / 8;
                    let down = byte < n && buf[byte] & (1 << (k.bit % 8)) != 0;
                    if down == self.pressed[ki] {
                        continue;
                    }
                    self.pressed[ki] = down;
                    if down {
                        events.push(InputEvent::Pressed(k.id.into()));
                    } else {
                        events.push(InputEvent::Released(k.id.into()));
                    }
                    // Built-in functions work like they do under iCUE.
                    match (k.id, down) {
                        (KEY_WINLOCK, true) => {
                            let v = !self.win_lock;
                            if let Err(e) = set_if_applied(self, |d| &mut d.win_lock, v, |d| d.apply_key_locks()) {
                                log::warn!("win lock: {e}");
                            }
                        }
                        (KEY_DPI_UP, true) => self.step_dpi(true),
                        (KEY_DPI_DOWN, true) => self.step_dpi(false),
                        (KEY_SNIPER, held) => {
                            self.sniper_held = held;
                            if let Err(e) = self.select_dpi_stage() {
                                log::warn!("{}: sniper: {e}", self.info.id);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        events
    }

    fn release(&mut self) -> Result<()> {
        // Hardware mode (sent twice: some firmware ignores the first), then
        // give the special keys their built-in functions back (ckb-next:
        // otherwise the G-keys can freeze the keyboard).
        let first = self.write(&[0x07, 0x04, 0x01]);
        pause(30);
        let mode = self.write(&[0x07, 0x04, 0x01]).or(first);
        pause(30);
        if !self.is_keyboard() {
            return mode;
        }
        mode.and(self.write_key_table(&hardware_key_table(self.model)))
    }
}

#[cfg(test)]
mod tests;
