//! An Aula keyboard, over its 2.4 GHz receiver or a USB cable (see
//! [`super::proto`]).
//!
//! Two lighting sources: OpenDeviceControl's software effects (live per-key colours,
//! refreshed continuously because the keyboard drops back to its own effect
//! within moments), or the keyboard's built-in effects, whose settings live
//! in its persistent settings block and are exposed as controls.

use super::keys;
use super::layout::{self, Led};
use crate::keymap::{FixedKey, KeyBinding, KeymapInfo, Macro};
use super::proto::{self, Config, COLOURFUL};
use crate::devices::Device;
use crate::model::*;
use crate::{Error, Result};
use std::time::{Duration, Instant};

/// Software lighting: resend the current frame at least this often (the
/// keyboard returns to its own effect about 2 s after the last frame).
const REFRESH: Duration = Duration::from_millis(1000);
/// How often to look for a better link: the cable while on the receiver,
/// and whether the cable is still there while on it.
const LINK_CHECK: Duration = Duration::from_secs(3);

/// A 512-byte flash sector of key settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sector {
    /// Keymap layer 0–3.
    Layer(u8),
    /// Macro page 0–7.
    Macros(u8),
}

/// Host keys offered as OpenDeviceControl input keys (`host_key_1`…).
const HOST_KEYS: u16 = 16;

/// How OpenDeviceControl talks to the keyboard: the 2.4 GHz receiver or the cable.
pub trait Link: Send {
    /// Housekeeping between frames (events, online checks).
    fn poll(&mut self) -> Result<()> {
        Ok(())
    }
    /// False while a wireless keyboard is asleep or away.
    fn online(&self) -> bool {
        true
    }
    /// True once after the keyboard came back online.
    fn take_reconnected(&mut self) -> bool {
        false
    }
    /// Battery (percent, flags: see [`proto::parse_battery`]).
    fn battery(&self) -> Option<(u8, u8)> {
        None
    }
    /// True once after the keyboard said its settings changed.
    fn take_settings_changed(&mut self) -> bool {
        false
    }
    /// Read a keymap layer or macro page (512 bytes; the 2.4 GHz layer read
    /// returns the 504 the firmware scans).
    fn read_sector(&mut self, s: Sector) -> Result<Vec<u8>>;
    /// Write a keymap layer or macro page (512 bytes; flash).
    fn write_sector(&mut self, s: Sector, data: &[u8]) -> Result<()>;
    /// Whether host-key presses (keymap action 04) reach OpenDeviceControl.
    fn host_keys(&self) -> bool {
        false
    }
    /// Host-key presses since the last call: `Some(code)` pressed, `None`
    /// released.
    fn poll_host_keys(&mut self) -> Vec<Option<u16>> {
        Vec::new()
    }
    fn read_config(&mut self) -> Result<Option<Config>>;
    fn write_config(&mut self, c: &Config) -> Result<()>;
    /// Live colours by LED index; `uniform` when every LED has that colour.
    /// Returns false if the frame was skipped (link still busy).
    fn show(&mut self, leds: &[(u8, Rgb)], uniform: Option<Rgb>) -> Result<bool>;
}

pub struct Model {
    pub id: u8,
    pub name: &'static str,
    pub slug: &'static str,
    /// Per-key LED map; `None` = whole keyboard in one colour.
    pub layout: Option<&'static [Led]>,
}

pub const MODELS: &[Model] = &[
    Model { id: proto::MODEL_F99, name: "F99", slug: "aula-f99", layout: Some(layout::F99) },
    Model { id: proto::MODEL_F99_PRO, name: "F99 Pro", slug: "aula-f99-pro", layout: None },
    Model { id: proto::MODEL_F75, name: "F75", slug: "aula-f75", layout: None },
];

/// Used when the keyboard is asleep while OpenDeviceControl starts (no model answer).
pub const FALLBACK: &Model = &MODELS[0];

/// The model with id `m`, or [`FALLBACK`].
pub fn model(m: Option<u8>) -> &'static Model {
    m.and_then(|m| MODELS.iter().find(|x| x.id == m)).unwrap_or(FALLBACK)
}

/// Ids a link had of its own before both links became one device (the
/// receiver kept the plain slug).
pub const RETIRED_IDS: &[&str] = &["aula-f99-usb", "aula-f99-pro-usb", "aula-f75-usb"];

fn device_info(info: &hidapi::DeviceInfo, model: &Model, wired: bool, id: String) -> DeviceInfo {
    let rel = info.release_number();
    DeviceInfo {
        id,
        kind: DeviceKind::Keyboard,
        manufacturer: "Aula".into(),
        model: model.name.into(),
        vendor_string: info.manufacturer_string().map(str::to_owned),
        product_string: info.product_string().map(str::to_owned).or_else(|| wired.then(|| "USB cable".into())),
        usb_vid: info.vendor_id(),
        usb_pid: info.product_id(),
        serial: info.serial_number().map(str::to_owned).filter(|s| !s.trim().is_empty()),
        firmware: Some(format!("{}.{:02x}", rel >> 8, rel & 0xff)),
    }
}

/// A built-in effect: (hardware id, name, has speed, has colour choice).
/// From the official driver's KB.ini `LedOpt` table; every effect has a
/// brightness.
pub type Effect = (u8, &'static str, bool, bool);

#[rustfmt::skip]
pub const EFFECTS: &[Effect] = &[
    (1, "Steady", false, true),
    (2, "Breathing", true, true),
    (3, "Rainbow (colour cycle)", true, false),
    (4, "Flash away", true, true),
    (5, "Raindrops", true, true),
    (7, "Ripples", true, true),
    (8, "Stars twinkle", true, true),
    // 6 Rainbow wheel, 9 Shadow disappear, 14 Retinue scanning and 18
    // Rotating storm are listed by the official driver, but the F99
    // firmware has no code for them (V14.2; dark on 0x1400 too), so they
    // aren't offered.
    (10, "Retro snake", true, true),
    (11, "Neon stream", true, true),
    (12, "Reaction", true, true),
    (13, "Sine wave", true, true),
    (15, "Rotating windmill", true, false),
    (16, "Colourful waterfall", true, false),
    (17, "Blossoming", true, false),
];

/// Colour choices: the keyboard's default palette slots 0–6, then colourful.
pub const COLOURS: &[&str] = &["Red", "Green", "Blue", "Yellow", "Magenta", "Cyan", "White", "Colourful"];

/// OS modes (settings byte 0x1B), as switched with Fn+W / Q / E / R.
pub const OS_MODES: [&str; 4] = ["Windows", "Windows, media keys first", "Mac", "Mac (alternate)"];

/// Sleep choices: (label, hardware value in 30 s units; 0 = never).
pub const SLEEP: &[(&str, u8)] = &[
    ("Never", 0),
    ("1 min", 2),
    ("1.5 min", 3),
    ("2 min", 4),
    ("3 min", 6),
    ("4 min", 8),
    ("5 min", 10),
    ("10 min", 20),
    ("15 min", 30),
    ("20 min", 40),
];

/// Windows virtual-key code for a front-end key name (see
/// `Device::preview_key`).
fn key_vk(key: &str) -> Option<u8> {
    let k = key.to_ascii_lowercase();
    let vk = match k.as_str() {
        "space" => 0x20,
        "enter" => 0x0D,
        "escape" => 0x1B,
        "tab" => 0x09,
        "backspace" => 0x08,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "insert" => 0x2D,
        "delete" => 0x2E,
        "minus" => 0xBD,
        "equals" => 0xBB,
        "comma" => 0xBC,
        "period" => 0xBE,
        "slash" => 0xBF,
        "semicolon" => 0xBA,
        "quote" => 0xDE,
        "backtick" => 0xC0,
        "leftbracket" => 0xDB,
        "rightbracket" => 0xDD,
        "backslash" => 0xDC,
        _ => 0,
    };
    if vk != 0 {
        return Some(vk);
    }
    if let Some(n) = k.strip_prefix('f').and_then(|d| d.parse::<u8>().ok()).filter(|n| (1..=12).contains(n)) {
        return Some(0x6F + n);
    }
    match k.as_bytes() {
        [c @ b'a'..=b'z'] => Some(c.to_ascii_uppercase()),
        [c @ b'0'..=b'9'] => Some(*c),
        _ => None,
    }
}

/// Key shapes for `layout`, indexed like the zone (`order`: LED index per
/// zone position).
fn shapes(layout: &[Led], order: &[u8]) -> Vec<LedShape> {
    order
        .iter()
        .enumerate()
        .filter_map(|(i, &led)| {
            let &(_, name, label, [l, t, r, b], _) = layout.iter().find(|k| k.0 == led)?;
            Some(LedShape { led: i, name: name.into(), label: label.into(), rect: [l as f32, t as f32, (r - l) as f32, (b - t) as f32] })
        })
        .collect()
}

/// One keyboard, whichever way it is connected: over the cable when it is
/// plugged in (smoother lighting, host keys), else over the receiver. It
/// moves between the two by itself.
pub struct AulaKeyboard {
    link: Box<dyn Link>,
    /// `link` is the cable.
    wired: bool,
    next_link_check: Instant,
    /// Cable interfaces that turned out not to be this keyboard (the cable
    /// VID/PID is a generic Sinowealth one).
    not_ours: Vec<std::ffi::CString>,
    /// For looking for the other link; refreshed per VID/PID (hidapi
    /// allows one kind of context per process, so no enumeration-free one).
    api: Option<hidapi::HidApi>,
    info: DeviceInfo,
    model: &'static Model,
    /// LED indices in zone order (left to right).
    order: Vec<u8>,
    config: Option<Config>,
    software: bool,
    frame: Vec<(u8, Rgb)>,
    uniform: Option<Rgb>,
    last_sent: Option<Instant>,
    /// Built-in effect preview while OpenDeviceControl lighting is off.
    preview: Option<super::preview::Preview>,
    /// Host key currently held (cable only).
    host_key_down: Option<u16>,
}

impl AulaKeyboard {
    pub(crate) fn open(o: super::Opened, id: String) -> Self {
        let model = model(o.model);
        let mut kb = Self {
            link: o.link,
            wired: o.wired,
            next_link_check: Instant::now() + LINK_CHECK,
            not_ours: Vec::new(),
            api: None,
            info: device_info(&o.info, model, o.wired, id),
            model,
            order: model.layout.map(layout::left_to_right).unwrap_or_default(),
            config: None,
            software: true,
            frame: Vec::new(),
            uniform: None,
            last_sent: None,
            preview: None,
            host_key_down: None,
        };
        if kb.link.online() {
            kb.read_config();
        }
        kb
    }

    fn api(&mut self) -> Option<&mut hidapi::HidApi> {
        if self.api.is_none() {
            match hidapi::HidApi::new() {
                Ok(a) => self.api = Some(a),
                Err(e) => log::warn!("{}: HID: {e}", self.info.id),
            }
        }
        self.api.as_mut()
    }

    /// Use `o` from now on, if it is the same model.
    fn switch_to(&mut self, o: super::Opened) -> bool {
        if o.model.is_some_and(|m| m != self.model.id) {
            log::info!("{}: the other link has a different Aula model ({:?}), not switching", self.info.id, o.model);
            if o.wired {
                self.not_ours.push(o.info.path().to_owned());
            }
            return false;
        }
        log::info!("{}: now over the {}", self.info.id, if o.wired { "USB cable" } else { "2.4 GHz receiver" });
        let id = std::mem::take(&mut self.info.id);
        self.info = device_info(&o.info, self.model, o.wired, id);
        self.link = o.link;
        self.wired = o.wired;
        // Re-read the settings and resend the lighting over the new link.
        self.config = None;
        self.last_sent = None;
        if let Some(down) = self.host_key_down.take() {
            log::debug!("{}: host key {down} released by the link change", self.info.id);
        }
        true
    }

    /// Prefer the cable: switch to it when it appears, and back to the
    /// receiver when it goes. Err when the keyboard has no link left.
    fn check_link(&mut self) -> Result<()> {
        if Instant::now() < self.next_link_check {
            return Ok(());
        }
        self.next_link_check = Instant::now() + LINK_CHECK;
        let Some(api) = self.api() else { return Ok(()) };
        let cables: Vec<_> = super::present(api, true).into_iter().filter(|p| !self.not_ours.contains(p)).collect();
        if self.wired {
            if cables.is_empty() {
                return self.fall_back();
            }
        } else if !cables.is_empty() {
            let skip = self.not_ours.clone();
            if let Some(o) = self.api().and_then(|api| super::find(api, true, &skip)) {
                self.switch_to(o);
            }
        }
        Ok(())
    }

    /// The current link failed or went away: use the other one if it is
    /// there.
    fn fall_back(&mut self) -> Result<()> {
        let skip = if self.wired { Vec::new() } else { self.not_ours.clone() };
        let wired = self.wired;
        match self.api().and_then(|api| super::find(api, !wired, &skip)).map(|o| self.switch_to(o)) {
            Some(true) => Ok(()),
            _ => {
                log::info!("{}: {} gone, no other link", self.info.id, if self.wired { "USB cable" } else { "receiver" });
                Err(Error::Offline)
            }
        }
    }

    /// Read the settings block; keeps the previous one on failure.
    fn read_config(&mut self) {
        match self.link.read_config() {
            Ok(Some(c)) => self.config = Some(c),
            Ok(None) => log::debug!("{}: no settings reply", self.info.id),
            Err(e) => log::warn!("{}: reading settings: {e}", self.info.id),
        }
    }

    fn update_config(&mut self, f: impl FnOnce(&mut Config)) -> Result<()> {
        self.read_config();
        let Some(mut c) = self.config.clone() else {
            return Err(Error::Offline);
        };
        f(&mut c);
        self.link.write_config(&c)?;
        self.config = Some(c);
        Ok(())
    }

    fn send_frame(&mut self) -> Result<()> {
        if self.link.show(&self.frame, self.uniform)? {
            self.last_sent = Some(Instant::now());
        }
        Ok(())
    }

    /// In Windows OS mode the firmware forces Fn+F1 / F2 / F4 (LED 6, 12,
    /// 24) to browser home, mail and Win+E, whatever the Fn layer says.
    fn fixed_keys(&self) -> Vec<FixedKey> {
        if self.config.as_ref().is_none_or(|c| c.os_mode() != 0) {
            return Vec::new();
        }
        let reason = "In Windows OS mode the keyboard's firmware always does this for Fn plus this key; the setting here only applies in the other OS modes.";
        [
            (6u8, KeyBinding::Media(0x0223)),
            (12, KeyBinding::Media(0x018a)),
            (24, KeyBinding::Key { modifiers: crate::keymap::modifier::LGUI, usage: 0x08 }),
        ]
        .into_iter()
        .filter_map(|(led, does)| {
            let key = self.order.iter().position(|&l| l == led)?;
            Some(FixedKey { layer: 1, key, does, reason: reason.into() })
        })
        .collect()
    }

    fn effect(&self) -> Option<&'static Effect> {
        let e = self.config.as_ref()?.effect();
        EFFECTS.iter().find(|x| x.0 == e)
    }

    fn controls(&self) -> Vec<Control> {
        let mut v = vec![Control {
            id: crate::devices::SOFTWARE_LIGHTING.into(),
            name: "OpenDeviceControl lighting".into(),
            kind: ControlKind::Toggle,
            value: ControlValue::Bool(self.software),
            help: Some("On: OpenDeviceControl's lighting effects drive the keys. Off: the keyboard runs its built-in effect.".into()),
        }];
        let Some(c) = &self.config else { return v };
        let current = self.effect();
        let choice = |id: &str, name: &str, options: Vec<String>, value: String, help: &str| Control {
            id: id.into(),
            name: name.into(),
            kind: ControlKind::Choice { options },
            value: ControlValue::Choice(value),
            help: Some(help.into()),
        };
        let number = |id: &str, name: &str, min: f64, max: f64, value: u8, help: &str| Control {
            id: id.into(),
            name: name.into(),
            kind: ControlKind::Number { min, max, step: 1.0, unit: String::new() },
            value: ControlValue::Number(value as f64),
            help: Some(help.into()),
        };
        let effect_name = current.map(|e| e.1.to_string()).unwrap_or_else(|| format!("Other ({})", c.effect()));
        v.push(choice(
            "hw_effect",
            "Built-in effect",
            EFFECTS.iter().map(|e| e.1.to_string()).collect(),
            effect_name,
            "Stored in the keyboard; shown when OpenDeviceControl lighting is off.",
        ));
        if let Some(&(e, _, has_speed, has_colour)) = current {
            let (brightness, speed, colour) = c.effect_settings(e);
            v.push(number("hw_brightness", "Effect brightness", 0.0, 4.0, brightness, "Built-in effect brightness (0 = off)."));
            if has_speed {
                v.push(number("hw_speed", "Effect speed", 0.0, 4.0, speed, "Built-in effect speed."));
            }
            if has_colour {
                let name = COLOURS.get(colour.min(COLOURFUL) as usize).copied().unwrap_or("Colourful");
                v.push(choice(
                    "hw_color",
                    "Effect colour",
                    COLOURS.iter().map(|s| s.to_string()).collect(),
                    name.into(),
                    "Built-in effect colour (the keyboard's palette).",
                ));
            }
        }
        let sleep = SLEEP.iter().find(|s| s.1 == c.sleep()).map(|s| s.0.to_string()).unwrap_or_else(|| format!("{} s", c.sleep() as u32 * 30));
        v.push(choice(
            "sleep",
            "Sleep after",
            SLEEP.iter().map(|s| s.0.to_string()).collect(),
            sleep,
            "In wireless mode the keyboard sleeps after this much idle time.",
        ));
        v.push(number("debounce", "Debounce", 1.0, 4.0, c.debounce(), "Key debounce level (1–4, as in the official driver)."));
        let os = c.os_mode() as usize;
        v.push(choice(
            "os_mode",
            "OS mode",
            OS_MODES.iter().map(|s| s.to_string()).collect(),
            OS_MODES[os.min(3)].into(),
            "Like Fn+W / Q / E / R on the keyboard.",
        ));
        v.push(Control {
            id: "tap_hold".into(),
            name: "Tap/hold time".into(),
            kind: ControlKind::Number { min: 0.0, max: 1000.0, step: 21.0, unit: "ms".into() },
            value: ControlValue::Number(c.tap_hold() as f64 * 21.0),
            help: Some("Keys with a Tap-layer action: released sooner than this, they do the tap action; held longer, the normal key. 0 turns the Tap layer off.".into()),
        });
        v.push(Control {
            id: "win_lock".into(),
            name: "Win key lock".into(),
            kind: ControlKind::Toggle,
            value: ControlValue::Bool(c.win_lock()),
            help: Some("Disables the Windows key (in Windows OS mode), like Fn+Win.".into()),
        });
        v
    }
}

impl Device for AulaKeyboard {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        self.check_link()?;
        if let Err(e) = self.link.poll() {
            log::warn!("{}: {e}", self.info.id);
            self.fall_back()?;
        }
        if self.link.take_reconnected() {
            // Settings may have been changed on the keyboard meanwhile, and
            // live lighting doesn't survive sleep: send the frame again now.
            self.config = None;
            self.last_sent = None;
        }
        if self.link.take_settings_changed() {
            self.config = None; // re-read below
        }
        if self.link.online() && self.config.is_none() {
            self.read_config();
        }
        // Keep software lighting alive between effect frames.
        if self.software && !self.frame.is_empty() && self.last_sent.is_none_or(|t| t.elapsed() >= REFRESH) {
            self.send_frame()?;
        }
        let mut sensors = Vec::new();
        if let Some((pct, flags)) = self.link.battery() {
            sensors.push(Sensor::new("battery", "Battery", SensorKind::Battery, pct as f64));
            sensors.push(Sensor::new("charging", "Charging", SensorKind::Charging, proto::charging(flags) as u8 as f64));
        }
        Ok(Snapshot { sensors, controls: self.controls() })
    }

    fn poll_interval(&self) -> Option<Duration> {
        Some(if self.software { REFRESH / 2 } else { Duration::from_secs(1) })
    }

    fn connected(&self) -> bool {
        self.link.online()
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        let bad = || Error::InvalidValue(id.to_string());
        let num = |v: &ControlValue| match v {
            ControlValue::Number(n) => Some(n.round().clamp(0.0, 255.0) as u8),
            _ => None,
        };
        let choice = |v: &ControlValue| match v {
            ControlValue::Choice(s) => Some(s.clone()),
            _ => None,
        };
        match id {
            crate::devices::SOFTWARE_LIGHTING => {
                let ControlValue::Bool(on) = value else { return Err(bad()) };
                self.software = *on;
                if *on {
                    self.last_sent = None;
                }
                Ok(())
            }
            "hw_effect" => {
                let name = choice(value).ok_or_else(bad)?;
                let e = EFFECTS.iter().find(|e| e.1 == name).ok_or_else(bad)?.0;
                self.update_config(|c| c.set_effect(e))
            }
            "hw_brightness" | "hw_speed" | "hw_color" => {
                let n = match id {
                    "hw_color" => {
                        let name = choice(value).ok_or_else(bad)?;
                        COLOURS.iter().position(|c| *c == name).ok_or_else(bad)? as u8
                    }
                    _ => num(value).ok_or_else(bad)?.min(4),
                };
                self.update_config(|c| {
                    let e = c.effect();
                    let (b, s, col) = c.effect_settings(e);
                    match id {
                        "hw_brightness" => c.set_effect_settings(e, n, s, col),
                        "hw_speed" => c.set_effect_settings(e, b, n, col),
                        _ => c.set_effect_settings(e, b, s, n),
                    }
                })
            }
            "sleep" => {
                let name = choice(value).ok_or_else(bad)?;
                let v = SLEEP.iter().find(|s| s.0 == name).ok_or_else(bad)?.1;
                self.update_config(|c| c.set_sleep(v))
            }
            "os_mode" => {
                let name = choice(value).ok_or_else(bad)?;
                let v = OS_MODES.iter().position(|m| *m == name).ok_or_else(bad)? as u8;
                self.update_config(|c| c.set_os_mode(v))
            }
            "tap_hold" => {
                let ControlValue::Number(ms) = value else { return Err(bad()) };
                let v = (ms / 21.0).round().clamp(0.0, 255.0) as u8;
                self.update_config(|c| c.set_tap_hold(v))
            }
            "win_lock" => {
                let ControlValue::Bool(on) = value else { return Err(bad()) };
                let on = *on;
                self.update_config(|c| c.set_win_lock(on))
            }
            "debounce" => {
                let v = num(value).ok_or_else(bad)?.clamp(1, 4);
                self.update_config(|c| c.set_debounce(v))
            }
            _ => Err(Error::UnknownControl(id.to_string())),
        }
    }

    fn keymap_info(&self) -> Option<KeymapInfo> {
        self.model.layout?;
        Some(KeymapInfo {
            layers: keys::layers(),
            functions: keys::functions(),
            macro_slots: keys::MACRO_SLOTS,
            macro_max_steps: 120,
            host_keys: self.link.host_keys(),
            fixed: self.fixed_keys(),
        })
    }

    fn read_keymap(&mut self, layer: u8) -> Result<Vec<KeyBinding>> {
        if layer > 3 || self.model.layout.is_none() {
            return Err(Error::NotSupported(": no such layer"));
        }
        let by_led = keys::layer_bindings(&self.link.read_sector(Sector::Layer(layer))?);
        Ok(self.order.iter().map(|&led| by_led.get(led as usize).cloned().unwrap_or(KeyBinding::None)).collect())
    }

    fn write_keymap(&mut self, layer: u8, zone: &[KeyBinding]) -> Result<()> {
        if layer > 3 || self.model.layout.is_none() {
            return Err(Error::NotSupported(": no such layer"));
        }
        // Read-modify-write: entries for LED slots without a key keep
        // whatever the keyboard has.
        let mut by_led = keys::layer_bindings(&self.link.read_sector(Sector::Layer(layer))?);
        by_led.resize(keys::SCANNED, KeyBinding::None);
        for (i, &led) in self.order.iter().enumerate() {
            if let (Some(b), Some(slot)) = (zone.get(i), by_led.get_mut(led as usize)) {
                *slot = b.clone();
            }
        }
        self.link.write_sector(Sector::Layer(layer), &keys::layer_sector(&by_led))
    }

    fn read_macros(&mut self) -> Result<Vec<Macro>> {
        let mut area = Vec::with_capacity(keys::MACRO_AREA);
        for page in 0..8 {
            let mut s = self.link.read_sector(Sector::Macros(page))?;
            s.resize(keys::SECTOR, 0);
            area.extend(s);
        }
        Ok(keys::macros_from_area(&area))
    }

    fn write_macros(&mut self, macros: &[Macro]) -> Result<()> {
        let area = keys::macro_area(macros).map_err(Error::Bus)?;
        for (page, chunk) in area.chunks(keys::SECTOR).enumerate() {
            self.link.write_sector(Sector::Macros(page as u8), chunk)?;
            // The official driver leaves 100 ms between pages.
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }

    fn input_keys(&self) -> Vec<InputKey> {
        // Listed whichever link is in use (front-ends read this once), but
        // presses only arrive over the cable (see `keymap_info`).
        if self.model.layout.is_none() {
            return Vec::new();
        }
        (1..=HOST_KEYS).map(|n| InputKey { id: format!("host_key_{n}"), name: format!("OpenDeviceControl key {n}"), assignable: true }).collect()
    }

    fn poll_input(&mut self) -> Vec<InputEvent> {
        let mut out = Vec::new();
        for e in self.link.poll_host_keys() {
            match e {
                Some(n) if (1..=HOST_KEYS).contains(&n) => {
                    if let Some(prev) = self.host_key_down.replace(n) {
                        out.push(InputEvent::Released(format!("host_key_{prev}")));
                    }
                    out.push(InputEvent::Pressed(format!("host_key_{n}")));
                }
                Some(_) => {}
                None => {
                    if let Some(prev) = self.host_key_down.take() {
                        out.push(InputEvent::Released(format!("host_key_{prev}")));
                    }
                }
            }
        }
        out
    }

    fn light_zones(&self) -> Vec<LightZone> {
        let leds = if self.model.layout.is_some() { self.order.len() } else { 1 };
        vec![LightZone { id: "keys".into(), name: "Keys".into(), leds }]
    }

    fn led_layout(&self) -> Vec<LedShape> {
        self.model.layout.map(|l| shapes(l, &self.order)).unwrap_or_default()
    }

    fn preview_lights(&mut self, t: f64) -> Option<Vec<Rgb>> {
        if self.software {
            self.preview = None;
            return None;
        }
        let c = self.config.as_ref()?;
        let layout = self.model.layout?;
        let effect = c.effect();
        let (brightness, speed, colour) = c.effect_settings(effect);
        let settings = super::preview::Settings { effect, brightness, speed, colour };
        match &mut self.preview {
            // Speed and brightness changes keep the animation running.
            Some(p) if p.settings().effect == effect && p.settings().colour == colour => p.adjust(speed, brightness),
            _ => self.preview = Some(super::preview::Preview::new(settings, layout, t)),
        }
        let cells = self.preview.as_mut()?.frame(t);
        // Driver key order → zone order.
        Some(
            self.order
                .iter()
                .map(|&led| layout.iter().position(|k| k.0 == led).and_then(|i| cells.get(i).copied()).unwrap_or_default())
                .collect(),
        )
    }

    fn preview_key(&mut self, key: &str) {
        if let (Some(p), Some(vk)) = (self.preview.as_mut(), key_vk(key)) {
            p.key(vk);
        }
    }

    fn set_lights(&mut self, colors: &[Rgb]) -> Result<()> {
        if !self.software {
            return Ok(());
        }
        let first = colors.first().copied().unwrap_or_default();
        let uniform = colors.iter().all(|&c| c == first).then_some(first);
        let frame: Vec<(u8, Rgb)> = if self.model.layout.is_some() {
            self.order.iter().enumerate().map(|(i, &led)| (led, colors.get(i).copied().unwrap_or_default())).collect()
        } else {
            (0..proto::wired::LED_SLOTS as u8).map(|led| (led, first)).collect()
        };
        let uniform = if self.model.layout.is_some() { uniform } else { Some(first) };
        let fresh = self.last_sent.is_some_and(|t| t.elapsed() < REFRESH);
        if frame == self.frame && fresh {
            return Ok(());
        }
        self.frame = frame;
        self.uniform = uniform;
        match self.send_frame() {
            Err(e) if self.wired => {
                // Most likely the cable was pulled.
                log::warn!("{}: {e}", self.info.id);
                self.fall_back()
            }
            r => r,
        }
    }

    fn release(&mut self) -> Result<()> {
        // Nothing to send: without refreshes the keyboard returns to its own
        // effect by itself.
        self.frame.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_consistent() {
        let mut ids: Vec<u8> = EFFECTS.iter().map(|e| e.0).collect();
        ids.dedup();
        assert_eq!(ids.len(), EFFECTS.len());
        assert_eq!(COLOURS.len() as u8, COLOURFUL + 1);
        assert!(SLEEP.windows(2).all(|w| w[0].1 < w[1].1));
        assert!(MODELS.iter().any(|m| m.id == proto::MODEL_F99 && m.layout.is_some()));
    }

    #[test]
    fn layout_shapes_follow_zone_order() {
        let order = layout::left_to_right(layout::F99);
        let shapes = shapes(layout::F99, &order);
        assert_eq!(shapes.len(), 99);
        assert!(shapes.iter().enumerate().all(|(i, s)| s.led == i));
        assert_eq!((shapes[0].name.as_str(), shapes[0].rect), ("Esc", [18.0, 20.0, 22.0, 27.0]));
        let space = shapes.iter().find(|s| s.name == "Space").unwrap();
        assert_eq!(order[space.led], 35);
        assert_eq!(space.rect, [138.0, 194.0, 196.0, 27.0]);
    }
}
