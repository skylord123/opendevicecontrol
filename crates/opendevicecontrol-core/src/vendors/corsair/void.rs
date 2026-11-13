//! Corsair's legacy (pre-Bragi) headsets: VOID, VOID PRO, VOID RGB ELITE and
//! HS70, wired and wireless. **Experimental: untested on real hardware.**
//!
//! Protocol (numbered HID reports, byte 0 = report ID, on the vendor
//! collection with usage page `0xFFC5`, interface 3; on wireless models the
//! receiver answers for the headset):
//!
//! | Report | Meaning |
//! |---|---|
//! | `C8 01 00` / `C8 00 00` | lighting owned by the host / by the firmware (the mic-mute LED only works automatically under firmware control) |
//! | `C9 64` | status request; answered by input report `64` (below) |
//! | `CA 02 n` | play notification sound `n` (0 or 1) |
//! | `CB n (reg, value)×n` | register writes: `1C/16/17` left R/G/B, `1D/18/19` right R/G/B, `1E` mic-mute LED |
//! | feature `FF 0B 00 FF 04 0E FF 05 01 04 00 v` | sidetone, `v` = 200..255, on the `0xFF00` collection (interface 0) |
//!
//! Status report `64 00 L B1 S`: `L` = battery percent, bit 7 set while the
//! microphone boom is flipped up (muted); `S` = 0 headset not connected to
//! the receiver, 1 on battery, 2 battery low, 4/5 charging. The receiver also
//! sends it unsolicited, and `xx xx 00 33` when the headset goes away.
//!
//! References (all GPL): HeadsetControl (`corsair_device.hpp`,
//! `corsair_void_rich.hpp`; battery, sidetone, sounds, lights), OpenRGB
//! (`CorsairVoidController`; lighting ownership and colour registers) and
//! OpenLinkHub (`voideliteW`, `voidelitedongle`; status events, mic-mute LED,
//! VOID ELITE LED brightness registers).
//!
//! Newer headsets (HS80, Virtuoso, VOID V2, HS55/65…) speak Bragi and are
//! not handled here.

use crate::model::*;
use crate::{Device, Error, Result};
use hidapi::HidDevice;
use std::ffi::CString;
use std::time::{Duration, Instant};

/// Vendor collection (usage 1) carrying lighting, status and sounds.
const USAGE_PAGE_MAIN: u16 = 0xffc5;
const MAIN_INTERFACE: i32 = 3;
/// Vendor collection (usage 1) that takes the sidetone feature report.
const USAGE_PAGE_SIDETONE: u16 = 0xff00;
const SIDETONE_INTERFACE: i32 = 0;

const REPORT_LIGHTING_OWNER: u8 = 0xc8;
const REPORT_REQUEST: u8 = 0xc9;
const REPORT_SOUND: u8 = 0xca;
const REPORT_REGISTERS: u8 = 0xcb;
const REPORT_STATUS: u8 = 0x64;

/// Register-write report length (OpenRGB `CORSAIR_VOID_REGISTER_REPORT_SIZE`).
const REGISTER_REPORT_LEN: usize = 20;

const REG_LEFT_RED: u8 = 0x1c;
const REG_LEFT_GREEN: u8 = 0x16;
const REG_LEFT_BLUE: u8 = 0x17;
const REG_RIGHT_RED: u8 = 0x1d;
const REG_RIGHT_GREEN: u8 = 0x18;
const REG_RIGHT_BLUE: u8 = 0x19;
const REG_MIC_LED: u8 = 0x1e;

/// Device sidetone range (HeadsetControl maps its 0..128 onto this).
const SIDETONE_MIN: u8 = 200;
const SIDETONE_MAX: u8 = 255;

/// How long to wait for the receiver to answer a status request.
const REPLY_TIMEOUT: Duration = Duration::from_millis(300);
/// Unanswered status requests before a wireless headset counts as away.
const MAX_MISSED: u32 = 3;
/// Wireless headsets fall back to firmware lighting when switched off and
/// on, so host control is re-requested this often (as OpenRGB does).
const OWNER_REFRESH: Duration = Duration::from_secs(1);

pub struct VoidModel {
    pub pid: u16,
    pub name: &'static str,
    pub slug: &'static str,
    pub wireless: bool,
    /// Left/right logo LEDs.
    pub rgb: bool,
    /// VOID ELITE: set the LED brightness registers when taking over
    /// lighting (OpenLinkHub `setDeviceBrightness`).
    pub elite_init: bool,
}

const fn m(pid: u16, name: &'static str, slug: &'static str, wireless: bool, rgb: bool, elite_init: bool) -> VoidModel {
    VoidModel { pid, name, slug, wireless, rgb, elite_init }
}

/// Product IDs and names from HeadsetControl (`corsair_void.c`,
/// `corsair_void_rich.hpp`), OpenRGB and OpenLinkHub.
pub static MODELS: &[VoidModel] = &[
    m(0x1b27, "VOID Wireless", "void-wireless", true, true, false),
    m(0x1b2a, "VOID RGB USB", "void-usb", false, true, false),
    m(0x1b23, "VOID RGB USB", "void-usb", false, true, false),
    m(0x1b29, "VOID RGB USB", "void-usb", false, true, false),
    m(0x1b1c, "VOID RGB", "void", false, true, false),
    m(0x0a2b, "VOID RGB Wireless", "void-wireless", true, true, false),
    m(0x0a14, "VOID PRO Wireless", "void-pro-wireless", true, true, false),
    m(0x0a16, "VOID PRO Wireless", "void-pro-wireless", true, true, false),
    m(0x0a1a, "VOID PRO Wireless", "void-pro-wireless", true, true, false),
    m(0x0a17, "VOID PRO USB", "void-pro-usb", false, true, false),
    m(0x0a1d, "VOID PRO USB", "void-pro-usb", false, true, false),
    m(0x0a51, "VOID RGB ELITE Wireless", "void-elite-wireless", true, true, true),
    m(0x0a55, "VOID ELITE Wireless", "void-elite-wireless", true, true, true),
    m(0x0a75, "VOID ELITE Wireless", "void-elite-wireless", true, true, true),
    m(0x0a52, "VOID ELITE USB", "void-elite-usb", false, true, true),
    m(0x0a56, "VOID series headset", "void-headset", false, true, false),
    m(0x0a38, "HS70 Wireless", "hs70-wireless", true, false, false),
    m(0x0a4f, "HS70 PRO Wireless", "hs70-pro-wireless", true, false, false),
];

pub fn model_for_pid(pid: u16) -> Option<&'static VoidModel> {
    MODELS.iter().find(|m| m.pid == pid)
}

pub const CTRL_SIDETONE: &str = "sidetone";
pub const CTRL_MIC_LED: &str = "mic_mute_led";
pub const CTRL_SOUND: &str = "notification_sound";
pub const SOUND_NONE: &str = "—";
pub const SOUNDS: [&str; 2] = ["Sound 1", "Sound 2"];

// ------------------------------------------------------------ packets

/// `C8 01 00` (host) / `C8 00 00` (firmware).
pub fn lighting_owner_packet(host: bool) -> [u8; 3] {
    [REPORT_LIGHTING_OWNER, host as u8, 0x00]
}

/// `C9 64`: ask for the status report.
pub fn status_request_packet() -> [u8; 2] {
    [REPORT_REQUEST, REPORT_STATUS]
}

/// `CA 02 n`: play notification sound `n` (0 or 1).
pub fn sound_packet(n: u8) -> [u8; 3] {
    [REPORT_SOUND, 0x02, n.min(1)]
}

fn registers_packet(pairs: &[(u8, u8)]) -> [u8; REGISTER_REPORT_LEN] {
    let mut p = [0u8; REGISTER_REPORT_LEN];
    p[0] = REPORT_REGISTERS;
    p[1] = pairs.len() as u8;
    for (i, (reg, val)) in pairs.iter().enumerate() {
        p[2 + 2 * i] = *reg;
        p[3 + 2 * i] = *val;
    }
    p
}

/// Logo colours, register order as OpenRGB writes them.
pub fn colors_packet(left: Rgb, right: Rgb) -> [u8; REGISTER_REPORT_LEN] {
    registers_packet(&[
        (REG_LEFT_RED, left.r),
        (REG_LEFT_GREEN, left.g),
        (REG_LEFT_BLUE, left.b),
        (REG_RIGHT_RED, right.r),
        (REG_RIGHT_GREEN, right.g),
        (REG_RIGHT_BLUE, right.b),
    ])
}

/// Mic-mute LED on/off (only effective under host lighting control).
pub fn mic_led_packet(on: bool) -> [u8; REGISTER_REPORT_LEN] {
    registers_packet(&[(REG_MIC_LED, if on { 0xff } else { 0x00 })])
}

/// VOID ELITE LED brightness registers, as OpenLinkHub sends them.
pub fn elite_brightness_packet() -> [u8; REGISTER_REPORT_LEN] {
    registers_packet(&[(0x2c, 0xaf), (0x26, 0xaf), (0x27, 0xaf), (0x2d, 0xaf), (0x28, 0xaf), (0x29, 0xaf)])
}

/// Sidetone `percent` (0..=100) mapped onto the device's 200..=255.
pub fn sidetone_level(percent: u8) -> u8 {
    let p = percent.min(100) as u32;
    (SIDETONE_MIN as u32 + (p * (SIDETONE_MAX - SIDETONE_MIN) as u32 + 50) / 100) as u8
}

/// 64-byte sidetone feature report (report ID `FF`).
pub fn sidetone_report(percent: u8) -> [u8; 64] {
    let mut r = [0u8; 64];
    r[..11].copy_from_slice(&[0xff, 0x0b, 0x00, 0xff, 0x04, 0x0e, 0xff, 0x05, 0x01, 0x04, 0x00]);
    r[11] = sidetone_level(percent);
    r
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The receiver reports no headset.
    Disconnected,
    Connected {
        battery: u8,
        charging: bool,
        low: bool,
        /// Microphone boom flipped up (muted).
        mic_up: bool,
    },
}

/// Decode a status report (`64 00 L B1 S`, report ID first). Returns `None`
/// for other reports.
pub fn parse_status(buf: &[u8]) -> Option<Status> {
    if buf.len() < 5 || buf[0] != REPORT_STATUS {
        return None;
    }
    // `64 02 …` is a mute-button event (OpenLinkHub), not a status report.
    if buf[1] == 0x02 {
        return None;
    }
    if buf[2] == 0x00 && buf[3] == 0x33 {
        return Some(Status::Disconnected);
    }
    let (charging, low) = match buf[4] {
        0 => return Some(Status::Disconnected),
        1 => (false, false),
        2 => (false, true),
        4 | 5 => (true, false),
        _ => return None,
    };
    Some(Status::Connected { battery: (buf[2] & 0x7f).min(100), charging, low, mic_up: buf[2] & 0x80 != 0 })
}

// ------------------------------------------------------------- device

pub struct VoidHeadset {
    dev: HidDevice,
    /// Interface taking the sidetone feature report, if found.
    sidetone_dev: Option<HidDevice>,
    info: DeviceInfo,
    model: &'static VoidModel,
    connected: bool,
    missed: u32,
    battery: Option<u8>,
    charging: bool,
    mic_up: bool,
    mic_led: bool,
    /// Last sidetone we set (the device cannot be read back).
    sidetone: Option<u8>,
    /// Host owns lighting; time of the last ownership request.
    owner_requested: Option<Instant>,
    /// Re-send colours and mic LED (after (re)connecting).
    resync: bool,
    /// Re-send the sidetone (after (re)connecting).
    resync_sidetone: bool,
    last_colors: Vec<Rgb>,
    last_light_write: Option<Instant>,
    last_mic_led: Option<bool>,
}

impl VoidHeadset {
    pub fn open(
        dev: HidDevice,
        sidetone_dev: Option<HidDevice>,
        model: &'static VoidModel,
        id: String,
        serial: Option<String>,
        firmware: Option<String>,
    ) -> Result<Self> {
        let mut d = Self {
            dev,
            sidetone_dev,
            info: DeviceInfo {
                id,
                kind: DeviceKind::Headset,
                manufacturer: "Corsair".into(),
                model: model.name.into(),
                vendor_string: None,
                product_string: None,
                usb_vid: crate::CORSAIR_VID,
                usb_pid: model.pid,
                serial,
                firmware,
            },
            model,
            connected: !model.wireless,
            missed: 0,
            battery: None,
            charging: false,
            mic_up: false,
            mic_led: true,
            sidetone: None,
            owner_requested: None,
            resync: true,
            resync_sidetone: false,
            last_colors: Vec::new(),
            last_light_write: None,
            last_mic_led: None,
        };
        d.info.product_string = d.dev.get_product_string().ok().flatten();
        d.info.vendor_string = d.dev.get_manufacturer_string().ok().flatten();
        if model.wireless {
            // Learn whether the headset is on; HID errors mean the receiver
            // itself is unusable.
            d.refresh_status()?;
        }
        Ok(d)
    }

    fn write(&self, p: &[u8]) -> Result<()> {
        // Numbered reports: byte 0 is the report ID itself.
        self.dev.write(p)?;
        Ok(())
    }

    fn apply(&mut self, s: Status) {
        match s {
            Status::Disconnected => {
                if self.connected && self.model.wireless {
                    log::info!("{}: headset disconnected from receiver", self.info.id);
                }
                if self.model.wireless {
                    self.connected = false;
                }
            }
            Status::Connected { battery, charging, mic_up, .. } => {
                if !self.connected {
                    log::info!("{}: headset connected", self.info.id);
                    self.resync = true;
                    self.resync_sidetone = true;
                    self.owner_requested = None;
                }
                self.connected = true;
                self.missed = 0;
                self.battery = Some(battery);
                self.charging = charging;
                self.mic_up = mic_up;
            }
        }
    }

    /// Read queued reports, then request a status report and wait for it.
    fn refresh_status(&mut self) -> Result<()> {
        let mut buf = [0u8; 64];
        for _ in 0..32 {
            match self.dev.read_timeout(&mut buf, 0)? {
                0 => break,
                n => {
                    if let Some(s) = parse_status(&buf[..n]) {
                        self.apply(s);
                    }
                }
            }
        }
        self.write(&status_request_packet())?;
        let deadline = Instant::now() + REPLY_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let n = self.dev.read_timeout(&mut buf, left.as_millis().max(1) as i32)?;
            if n == 0 {
                break;
            }
            if let Some(s) = parse_status(&buf[..n]) {
                self.apply(s);
                return Ok(());
            }
        }
        // No answer. Wired headsets may simply not support the request; a
        // receiver that stops answering counts as "headset away", which is
        // not an error (the worker would otherwise give up and reopen it).
        self.missed += 1;
        if self.model.wireless && self.missed >= MAX_MISSED && self.connected {
            log::info!("{}: no status from receiver; treating headset as off", self.info.id);
            self.connected = false;
        }
        Ok(())
    }

    fn take_lighting(&mut self) -> Result<()> {
        let refresh = self.model.wireless && self.owner_requested.is_some_and(|t| t.elapsed() >= OWNER_REFRESH);
        if self.owner_requested.is_none() || refresh {
            self.write(&lighting_owner_packet(true))?;
            if self.owner_requested.is_none() && self.model.elite_init {
                self.write(&elite_brightness_packet())?;
            }
            self.owner_requested = Some(Instant::now());
        }
        Ok(())
    }

    /// Under host lighting control the firmware leaves the mic-mute LED
    /// alone, so drive it from the boom position ourselves.
    fn update_mic_led(&mut self) -> Result<()> {
        if self.owner_requested.is_none() || !self.connected {
            return Ok(());
        }
        let want = self.mic_led && self.mic_up;
        if self.last_mic_led != Some(want) || self.resync {
            self.write(&mic_led_packet(want))?;
            self.last_mic_led = Some(want);
        }
        Ok(())
    }

    fn set_sidetone(&mut self, percent: u8) -> Result<()> {
        let Some(st) = &self.sidetone_dev else {
            return Err(Error::UnknownControl(CTRL_SIDETONE.into()));
        };
        st.send_feature_report(&sidetone_report(percent))?;
        self.sidetone = Some(percent);
        Ok(())
    }

    fn controls(&self) -> Vec<Control> {
        let mut c = Vec::new();
        if self.sidetone_dev.is_some() {
            c.push(Control {
                id: CTRL_SIDETONE.into(),
                name: "Sidetone".into(),
                kind: ControlKind::Number { min: 0.0, max: 100.0, step: 1.0, unit: "%".into() },
                value: ControlValue::Number(self.sidetone.unwrap_or(0) as f64),
                help: Some("How much of your own voice you hear. Not read back from the headset: shows the last value set here.".into()),
            });
        }
        if self.model.rgb {
            c.push(Control {
                id: CTRL_MIC_LED.into(),
                name: "Mic mute LED".into(),
                kind: ControlKind::Toggle,
                value: ControlValue::Bool(self.mic_led),
                help: Some("Light the microphone LED while the boom is flipped up (muted).".into()),
            });
        }
        let mut options = vec![SOUND_NONE.to_string()];
        options.extend(SOUNDS.iter().map(|s| s.to_string()));
        c.push(Control {
            id: CTRL_SOUND.into(),
            name: "Play notification sound".into(),
            kind: ControlKind::Choice { options },
            value: ControlValue::Choice(SOUND_NONE.into()),
            help: Some("Plays one of the headset's built-in sounds.".into()),
        });
        c
    }
}

impl Device for VoidHeadset {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        self.refresh_status()?;
        if self.connected {
            if std::mem::take(&mut self.resync_sidetone) {
                if let Some(p) = self.sidetone {
                    if let Err(e) = self.set_sidetone(p) {
                        log::warn!("{}: restoring sidetone: {e}", self.info.id);
                    }
                }
            }
            self.update_mic_led()?;
        }
        let mut sensors = Vec::new();
        if self.model.wireless {
            // Always listed (with the last known values) so front-ends see
            // the same entities whether or not the headset is on.
            sensors.push(Sensor::new("battery", "Battery", SensorKind::Battery, self.battery.unwrap_or(0) as f64));
            sensors.push(Sensor::new("charging", "Charging", SensorKind::Charging, self.charging as u8 as f64));
        }
        Ok(Snapshot { sensors, controls: self.controls() })
    }

    fn poll_interval(&self) -> Option<Duration> {
        Some(Duration::from_secs(5))
    }

    fn connected(&self) -> bool {
        self.connected
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        match (id, value) {
            (CTRL_SIDETONE, ControlValue::Number(v)) => self.set_sidetone(v.round().clamp(0.0, 100.0) as u8),
            (CTRL_MIC_LED, ControlValue::Bool(b)) if self.model.rgb => {
                self.mic_led = *b;
                self.update_mic_led()
            }
            (CTRL_SOUND, ControlValue::Choice(s)) => match SOUNDS.iter().position(|x| x == s) {
                Some(n) => self.write(&sound_packet(n as u8)),
                None if s == SOUND_NONE => Ok(()),
                None => Err(Error::InvalidValue(id.into())),
            },
            (CTRL_SIDETONE | CTRL_MIC_LED | CTRL_SOUND, _) => Err(Error::InvalidValue(id.into())),
            _ => Err(Error::UnknownControl(id.into())),
        }
    }

    fn light_zones(&self) -> Vec<LightZone> {
        if !self.model.rgb {
            return Vec::new();
        }
        vec![
            LightZone { id: "left".into(), name: "Left earcup".into(), leds: 1 },
            LightZone { id: "right".into(), name: "Right earcup".into(), leds: 1 },
        ]
    }

    fn set_lights(&mut self, colors: &[Rgb]) -> Result<()> {
        if !self.model.rgb || !self.connected {
            return Ok(());
        }
        self.take_lighting()?;
        let fresh = self.last_light_write.is_some_and(|t| t.elapsed() < Duration::from_secs(1));
        if colors == self.last_colors.as_slice() && fresh && !self.resync {
            return Ok(());
        }
        let left = colors.first().copied().unwrap_or_default();
        let right = colors.get(1).copied().unwrap_or(left);
        self.write(&colors_packet(left, right))?;
        if self.resync {
            self.update_mic_led()?;
            self.resync = false;
        }
        self.last_colors = colors.to_vec();
        self.last_light_write = Some(Instant::now());
        Ok(())
    }

    fn release(&mut self) {
        if self.owner_requested.take().is_some() {
            let _ = self.write(&lighting_owner_packet(false));
        }
    }
}

// ---------------------------------------------------------- discovery

fn is_main(d: &hidapi::DeviceInfo) -> bool {
    (d.usage_page() == USAGE_PAGE_MAIN && d.usage() == 1) || (d.usage_page() == 0 && d.interface_number() == MAIN_INTERFACE)
}

fn is_sidetone(d: &hidapi::DeviceInfo) -> bool {
    (d.usage_page() == USAGE_PAGE_SIDETONE && d.usage() == 1) || (d.usage_page() == 0 && d.interface_number() == SIDETONE_INTERFACE)
}

/// Open every legacy headset in `list` (already filtered to Corsair).
pub(crate) fn discover(
    api: &hidapi::HidApi,
    list: &[&hidapi::DeviceInfo],
    ids: &mut super::Ids,
    skip_paths: &[CString],
    out: &mut Vec<(CString, Box<dyn Device>)>,
) {
    for d in list.iter().filter(|d| model_for_pid(d.product_id()).is_some() && is_main(d)) {
        let model = model_for_pid(d.product_id()).unwrap();
        let id = ids.next(model.slug);
        if skip_paths.iter().any(|p| p.as_c_str() == d.path()) {
            continue;
        }
        let sidetone = list
            .iter()
            .find(|s| s.product_id() == d.product_id() && s.serial_number() == d.serial_number() && is_sidetone(s))
            .and_then(|s| match api.open_path(s.path()) {
                Ok(h) => Some(h),
                Err(e) => {
                    log::warn!("{}: cannot open sidetone interface: {e}", model.name);
                    None
                }
            });
        let serial = d.serial_number().map(str::to_owned).filter(|s| !s.trim().is_empty());
        let rel = d.release_number();
        let firmware = (rel != 0).then(|| format!("{}.{:02x}", rel >> 8, rel & 0xff));
        match api
            .open_path(d.path())
            .map_err(crate::Error::from)
            .and_then(|h| VoidHeadset::open(h, sidetone, model, id.clone(), serial, firmware))
        {
            Ok(dev) => {
                log::info!(
                    "opened {} ({id}, experimental){}",
                    model.name,
                    if dev.connected { "" } else { ", headset currently off" }
                );
                out.push((d.path().to_owned(), Box::new(dev)));
            }
            Err(e) => log::warn!("failed to open {}: {e}", model.name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pids_unique_and_not_nxp() {
        for (i, a) in MODELS.iter().enumerate() {
            assert!(MODELS[i + 1..].iter().all(|b| b.pid != a.pid), "duplicate {:04x}", a.pid);
            assert!(crate::devices::nxp::model_for_pid(a.pid).is_none());
            assert!(crate::devices::hxi::model_for_pid(a.pid).is_none());
        }
    }

    #[test]
    fn simple_packets() {
        // HeadsetControl / OpenRGB byte layouts.
        assert_eq!(lighting_owner_packet(true), [0xc8, 0x01, 0x00]);
        assert_eq!(lighting_owner_packet(false), [0xc8, 0x00, 0x00]);
        assert_eq!(status_request_packet(), [0xc9, 0x64]);
        assert_eq!(sound_packet(0), [0xca, 0x02, 0x00]);
        assert_eq!(sound_packet(1), [0xca, 0x02, 0x01]);
        assert_eq!(sound_packet(7), [0xca, 0x02, 0x01]);
    }

    #[test]
    fn colors() {
        let p = colors_packet(Rgb::new(1, 2, 3), Rgb::new(4, 5, 6));
        assert_eq!(
            p[..14],
            [0xcb, 0x06, 0x1c, 1, 0x16, 2, 0x17, 3, 0x1d, 4, 0x18, 5, 0x19, 6]
        );
        assert!(p[14..].iter().all(|&b| b == 0));
        assert_eq!(p.len(), 20);
    }

    #[test]
    fn mic_led_and_elite() {
        assert_eq!(mic_led_packet(true)[..4], [0xcb, 0x01, 0x1e, 0xff]);
        assert_eq!(mic_led_packet(false)[..4], [0xcb, 0x01, 0x1e, 0x00]);
        assert_eq!(
            elite_brightness_packet()[..14],
            [0xcb, 0x06, 0x2c, 0xaf, 0x26, 0xaf, 0x27, 0xaf, 0x2d, 0xaf, 0x28, 0xaf, 0x29, 0xaf]
        );
    }

    #[test]
    fn sidetone() {
        assert_eq!(sidetone_level(0), 200);
        assert_eq!(sidetone_level(100), 255);
        assert_eq!(sidetone_level(200), 255);
        assert_eq!(sidetone_level(50), 228);
        let r = sidetone_report(100);
        assert_eq!(r[..12], [0xff, 0x0b, 0x00, 0xff, 0x04, 0x0e, 0xff, 0x05, 0x01, 0x04, 0x00, 0xff]);
        assert!(r[12..].iter().all(|&b| b == 0));
    }

    #[test]
    fn status_reports() {
        // On battery, 80 %, mic down.
        assert_eq!(
            parse_status(&[0x64, 0x00, 80, 0xb1, 0x01]),
            Some(Status::Connected { battery: 80, charging: false, low: false, mic_up: false })
        );
        // Low battery, mic boom up (bit 7).
        assert_eq!(
            parse_status(&[0x64, 0x00, 0x80 | 9, 0xb1, 0x02]),
            Some(Status::Connected { battery: 9, charging: false, low: true, mic_up: true })
        );
        // Charging (4 and 5).
        for s in [4, 5] {
            assert_eq!(
                parse_status(&[0x64, 0x00, 55, 0xb1, s]),
                Some(Status::Connected { battery: 55, charging: true, low: false, mic_up: false })
            );
        }
        // Headset not connected to the receiver.
        assert_eq!(parse_status(&[0x64, 0x00, 0, 0xb1, 0x00]), Some(Status::Disconnected));
        assert_eq!(parse_status(&[0x64, 0x00, 0x00, 0x33, 0x00]), Some(Status::Disconnected));
        // Other reports.
        assert_eq!(parse_status(&[0x64, 0x02, 0, 0, 0]), None);
        assert_eq!(parse_status(&[0x66, 0x00, 0, 1, 2]), None);
        assert_eq!(parse_status(&[0x64, 0x00]), None);
        assert_eq!(parse_status(&[0x64, 0x00, 50, 0xb1, 0x09]), None);
    }
}
