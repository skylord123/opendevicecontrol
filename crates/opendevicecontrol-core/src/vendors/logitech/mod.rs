//! Logitech mice, keyboards and headsets over HID++: through a Unifying,
//! Bolt, Nano or Lightspeed receiver, on a USB cable, or over Bluetooth.
//!
//! Everything Solaar offers is here: sensitivity, report rate, scroll wheel
//! and SmartShift, thumb wheel, button remapping and diversion (diverted
//! buttons, gestures and wheels become keys for actions), keyboard
//! backlight and Fn keys, Easy-Switch hosts, lighting, haptics, headset
//! sidetone and EQ, battery, and pairing on the receivers.
//!
//! | Module | Contents |
//! |---|---|
//! | `hidpp` | report framing, request/reply matching, errors |
//! | `features` | HID++ 2.0 feature ids and the feature table |
//! | `link` | a channel plus its features |
//! | `settings` | every setting, read and written over HID++ |
//! | `buttons` | remapping, diversion, gestures, wheel and G-key events |
//! | `receiver` | receivers: slots, pairing, unpairing |
//! | `devices` | names, kinds and HID++ 1.0 registers of known models |
//! | `keys` | control, task and key names |
//!
//! Protocol notes: docs/reverse-engineering/logitech-hidpp.md.

pub mod buttons;
pub mod devices;
pub mod features;
pub mod hidpp;
pub mod keys;
pub mod link;
pub mod receiver;
pub mod settings;

use self::buttons::{Buttons, Wheels};
use self::features::{self as f, Features};
use self::hidpp::{Channel, Report};
use self::link::{at, u16_at, Link};
use self::settings::{Dpi, Setting};
use crate::devices::{Device, Ids};
use crate::model::*;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::time::{Duration, Instant};

pub const VID: u16 = 0x046D;
/// HID++ collections: short/long reports on receivers and USB devices,
/// and the Bluetooth (BLE) one.
const PAGE_HIDPP: u16 = 0xFF00;
const PAGE_HIDPP_BLE: u16 = 0xFF43;

/// How often settings and battery are read again while connected.
const REFRESH: Duration = Duration::from_secs(60);
/// How often an offline device in a receiver slot is pinged.
const PING_OFFLINE: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq)]
struct Battery {
    level: Option<f64>,
    charging: bool,
    voltage: Option<u16>,
}

/// What the user set here: re-applied when the device connects (it forgets
/// most settings when it sleeps or switches off), kept in the state folder.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Saved {
    values: BTreeMap<String, ControlValue>,
}

impl Saved {
    fn path(id: &str) -> Option<std::path::PathBuf> {
        crate::state_dir().map(|d| d.join(format!("logitech-{id}.json")))
    }

    fn load(id: &str) -> Saved {
        Saved::path(id).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn store(&self, id: &str) {
        let Some(p) = Saved::path(id) else { return };
        match serde_json::to_string_pretty(self) {
            Ok(t) => {
                if let Err(e) = std::fs::write(&p, t) {
                    log::warn!("logitech: saving {}: {e}", p.display());
                }
            }
            Err(e) => log::warn!("logitech: {e}"),
        }
    }
}

/// Serial numbers of devices connected on a cable or Bluetooth: the same
/// device's receiver slot then stays out of the way.
static DIRECT: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn connected_directly(serial: Option<&String>) -> bool {
    serial.is_some_and(|s| DIRECT.lock().unwrap_or_else(|e| e.into_inner()).contains(s))
}

/// A mouse, keyboard or headset.
pub struct Peripheral {
    info: DeviceInfo,
    link: Link,
    /// Receiver slot, `None` on a cable or Bluetooth.
    slot: Option<u8>,
    registers: u32,
    online: bool,
    /// Features, settings and buttons have been read for this connection.
    ready: bool,
    settings: Vec<Box<dyn Setting>>,
    dpi: Option<Dpi>,
    buttons: Option<Buttons>,
    wheels: Wheels,
    battery: Option<Battery>,
    saved: Saved,
    /// Name of the saved-settings file: the serial number when known (the
    /// same device over Bluetooth and on a receiver shares it), else the id.
    saved_key: String,
    refreshed: Instant,
    pinged: Instant,
    /// Unpaired from its receiver: the worker should give up.
    gone: bool,
    events: Vec<InputEvent>,
}

impl Peripheral {
    #[allow(clippy::too_many_arguments)]
    fn open(ch: Channel, slot: Option<u8>, id: String, model: String, kind: DeviceKind, pid: u16, serial: Option<String>, registers: u32) -> Peripheral {
        let info = DeviceInfo {
            id: id.clone(),
            kind,
            manufacturer: "Logitech".into(),
            model,
            vendor_string: None,
            product_string: None,
            usb_vid: VID,
            usb_pid: pid,
            serial,
            firmware: None,
        };
        let mut p = Peripheral {
            info,
            link: Link { ch, features: Features::default(), protocol: (0, 0) },
            slot,
            registers,
            online: false,
            ready: false,
            settings: Vec::new(),
            dpi: None,
            buttons: None,
            wheels: Wheels::default(),
            battery: None,
            saved: Saved::default(),
            saved_key: String::new(),
            refreshed: Instant::now(),
            pinged: Instant::now(),
            gone: false,
            events: Vec::new(),
        };
        p.load_saved();
        p.connect();
        p
    }

    fn load_saved(&mut self) {
        let key = self.info.serial.clone().unwrap_or_else(|| self.info.id.clone());
        if key != self.saved_key {
            self.saved = Saved::load(&key);
            self.saved_key = key;
        }
    }

    /// Ping the device and, when it answers, read what it offers.
    fn connect(&mut self) {
        self.pinged = Instant::now();
        match self.link.ch.protocol() {
            Ok(v) => {
                self.link.protocol = v;
                self.online = true;
                if let Err(e) = self.setup() {
                    log::warn!("{}: reading the device: {e}", self.info.id);
                    self.online = !matches!(e, Error::Offline | Error::NoReply { .. });
                }
            }
            Err(Error::Offline) | Err(Error::NoReply { .. }) => self.online = false,
            Err(e) => {
                log::debug!("{}: ping: {e}", self.info.id);
                self.online = false;
            }
        }
    }

    /// Read features, identity, settings and buttons, then re-apply what
    /// the user set.
    fn setup(&mut self) -> Result<()> {
        let id = self.info.id.clone();
        self.ready = false;
        if !self.link.hidpp10() {
            self.link.features = Features::read(&mut self.link.ch)?;
            log::debug!(
                "{id}: HID++ {}.{}, features {}",
                self.link.protocol.0,
                self.link.protocol.1,
                self.link.features.all().iter().map(|x| format!("{:04x} {}", x.id, f::name(x.id))).collect::<Vec<_>>().join(", ")
            );
            if let Some(n) = name(&mut self.link) {
                self.info.model = n;
            }
            if let Some(k) = kind(&mut self.link) {
                self.info.kind = k;
            }
            if let Some(fw) = firmware(&mut self.link) {
                self.info.firmware = Some(fw);
            }
            if self.info.serial.is_none() {
                self.info.serial = unit_id(&mut self.link);
            }
            if self.slot.is_none() {
                if let Some(sn) = &self.info.serial {
                    let mut d = DIRECT.lock().unwrap_or_else(|e| e.into_inner());
                    if !d.contains(sn) {
                        d.push(sn.clone());
                    }
                }
            }
            self.load_saved();
        } else if let Ok(r) = self.link.read_register(0xF1, &[0x01]) {
            self.info.firmware = Some(format!("{:02X}.{:02X}", at(&r, 1), at(&r, 2)));
        }
        let list = settings::build(&mut self.link, self.registers);
        // DPI is kept apart: sliding DPI drives it.
        self.dpi = None;
        if !self.link.hidpp10() && self.link.has(f::ADJUSTABLE_DPI) {
            match settings::dpi_list(&mut self.link, f::ADJUSTABLE_DPI, 1, 1, 0) {
                Ok(l) if !l.is_empty() => self.dpi = Some(Dpi { list: l, value: None }),
                Ok(_) => {}
                Err(e) => log::debug!("{id}: DPI list: {e}"),
            }
        }
        self.settings = list;
        self.buttons = if self.link.hidpp10() { None } else { Buttons::read(&mut self.link)? };
        self.wheels = if self.link.hidpp10() { Wheels::default() } else { Wheels::read(&mut self.link) };
        self.restore();
        self.read_all();
        self.ready = true;
        Ok(())
    }

    /// Re-apply saved settings and button modes (after a reconnect).
    fn restore(&mut self) {
        let id = self.info.id.clone();
        let values: Vec<(String, ControlValue)> = self.saved.values.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (cid, v) in values {
            if let Err(e) = self.apply(&cid, &v) {
                log::debug!("{id}: restoring {cid}: {e}");
            }
        }
        if let Some(b) = &mut self.buttons {
            b.restore(&mut self.link);
        }
    }

    fn read_all(&mut self) {
        let id = self.info.id.clone();
        if let Some(d) = &mut self.dpi {
            if let Err(e) = d.read(&mut self.link) {
                self.note_error(&e);
            }
        }
        for i in 0..self.settings.len() {
            if let Err(e) = self.settings[i].read(&mut self.link) {
                log::debug!("{id}: reading a setting: {e}");
                if matches!(e, Error::Offline) {
                    self.online = false;
                    return;
                }
            }
        }
        if let Some(b) = &mut self.buttons {
            b.refresh(&mut self.link);
        }
        self.sync_wheels();
        self.battery = self.read_battery().or(self.battery);
        self.refreshed = Instant::now();
    }

    fn sync_wheels(&mut self) {
        let on = |s: &[Box<dyn Setting>], id: &str| {
            s.iter().find(|x| x.owns(id)).and_then(|x| x.controls().into_iter().find(|c| c.id == id)).is_some_and(|c| c.value == ControlValue::Bool(true))
        };
        self.wheels.thumb_diverted = on(&self.settings, buttons::THUMB_DIVERT);
        self.wheels.wheel_diverted = on(&self.settings, buttons::WHEEL_DIVERT);
        self.wheels.crown_diverted = on(&self.settings, buttons::CROWN_DIVERT);
        self.wheels.gkeys_diverted = on(&self.settings, buttons::GKEYS_DIVERT);
    }

    fn note_error(&mut self, e: &Error) {
        if matches!(e, Error::Offline) {
            self.online = false;
        }
    }

    fn read_battery(&mut self) -> Option<Battery> {
        let l = &mut self.link;
        if l.hidpp10() {
            if self.registers & settings::reg::BATTERY_CHARGE != 0 {
                let r = l.read_register(0x0D, &[]).ok()?;
                return Some(Battery { level: Some(at(&r, 0) as f64), charging: matches!(at(&r, 2) & 0xF0, 0x50), voltage: None });
            }
            let r = l.read_register(0x07, &[]).ok()?;
            let level = match at(&r, 0) {
                7 => 90.0,
                5 => 50.0,
                3 => 20.0,
                1 => 5.0,
                _ => 0.0,
            };
            let charging = at(&r, 1) & 0x21 == 0x21;
            return Some(Battery { level: (at(&r, 0) != 0 || !charging).then_some(level), charging, voltage: None });
        }
        if l.has(f::UNIFIED_BATTERY) {
            return l.call(f::UNIFIED_BATTERY, 1, &[]).ok().map(|r| unified_battery(&r));
        }
        if l.has(f::BATTERY_STATUS) {
            return l.call(f::BATTERY_STATUS, 0, &[]).ok().map(|r| battery_status(&r));
        }
        if l.has(f::BATTERY_VOLTAGE) {
            return l.call(f::BATTERY_VOLTAGE, 0, &[]).ok().map(|r| battery_voltage(&r));
        }
        if l.has(f::ADC_MEASUREMENT) {
            return l.call(f::ADC_MEASUREMENT, 0, &[]).ok().and_then(|r| adc_battery(&r));
        }
        None
    }

    /// Handle what the device (or its receiver) sent on its own.
    fn process_notifications(&mut self) {
        for r in self.link.ch.take_notifications() {
            self.notification(&r);
        }
    }

    fn notification(&mut self, r: &Report) {
        let id = self.info.id.clone();
        match r.sub {
            hidpp::DEVICE_CONNECTED if self.slot.is_some() => {
                let up = at(&r.data, 0) & 0x40 == 0;
                log::debug!("{id}: link {}", if up { "up" } else { "down" });
                if up && !self.online {
                    self.connect();
                } else if !up {
                    self.online = false;
                }
                return;
            }
            hidpp::DEVICE_DISCONNECTED if self.slot.is_some() && r.address == 0x02 => {
                log::info!("{id}: unpaired from the receiver");
                self.gone = true;
                return;
            }
            s if s >= 0x40 => return,
            _ => {}
        }
        if self.link.hidpp10() {
            return;
        }
        let Some(feature) = self.link.feature_of(r.sub) else { return };
        if !self.online {
            // A device that sends events is there.
            self.connect();
            return;
        }
        log::trace!("{id}: {} event {} {:02x?}", f::name(feature), r.function(), r.data);
        match feature {
            f::WIRELESS_DEVICE_STATUS if r.function() == 0 => {
                // Woke up or reconnected; asks for its settings again.
                if at(&r.data, 1) == 1 {
                    log::debug!("{id}: reconnected, restoring settings");
                    self.restore();
                    self.read_all();
                }
            }
            f::UNIFIED_BATTERY if r.function() == 0 => self.battery = Some(unified_battery(&r.data)),
            f::BATTERY_STATUS if r.function() == 0 => self.battery = Some(battery_status(&r.data)),
            f::BATTERY_VOLTAGE if r.function() == 0 => self.battery = Some(battery_voltage(&r.data)),
            f::ADC_MEASUREMENT if r.function() == 0 => self.battery = adc_battery(&r.data).or(self.battery),
            f::REPROG_CONTROLS_V4 => {
                if let Some(b) = &mut self.buttons {
                    b.notify(&mut self.link, r, self.dpi.as_mut());
                    self.events.extend(b.take_events());
                }
            }
            f::HIRES_WHEEL if r.function() == 1 => {
                // The ratchet switched (button behind the wheel): re-read soon.
                self.refreshed = Instant::now() - REFRESH;
            }
            _ => {
                self.wheels.notify(feature, r);
                self.events.extend(self.wheels.take_events());
            }
        }
    }

    /// Write a control to the device (no saving).
    fn apply(&mut self, id: &str, v: &ControlValue) -> Result<()> {
        if let Some(d) = &mut self.dpi {
            if d.owns(id) {
                return d.write(&mut self.link, id, v);
            }
        }
        let has_dpi = self.dpi.is_some();
        if let Some(b) = &mut self.buttons {
            if b.owns(id) {
                return b.write(&mut self.link, id, v, has_dpi);
            }
        }
        let s = self.settings.iter_mut().find(|s| s.owns(id)).ok_or_else(|| Error::UnknownControl(id.into()))?;
        s.write(&mut self.link, id, v)?;
        self.sync_wheels();
        Ok(())
    }

    fn persistent(&self, id: &str) -> bool {
        self.settings.iter().find(|s| s.owns(id)).is_none_or(|s| s.persist())
    }

    fn sensors(&self) -> Vec<Sensor> {
        let mut out = Vec::new();
        if let Some(b) = self.battery {
            if let Some(level) = b.level {
                out.push(Sensor::new("battery", "Battery", SensorKind::Battery, level));
            }
            out.push(Sensor::new("charging", "Charging", SensorKind::Charging, if b.charging { 1.0 } else { 0.0 }));
            if let Some(mv) = b.voltage {
                out.push(Sensor::new("battery_voltage", "Battery voltage", SensorKind::Voltage, mv as f64 / 1000.0).diagnostic());
            }
        }
        out
    }

    fn controls(&self) -> Vec<Control> {
        let mut out = Vec::new();
        if let Some(d) = &self.dpi {
            out.extend(d.controls());
        }
        if let Some(b) = &self.buttons {
            out.extend(b.controls(self.dpi.is_some()));
        }
        for s in &self.settings {
            out.extend(s.controls());
        }
        out
    }
}

/// UNIFIED_BATTERY status: percent, approximate level, charging status,
/// external power.
fn unified_battery(r: &[u8]) -> Battery {
    let pct = at(r, 0);
    let level = match at(r, 1) {
        8 => 90.0,
        4 => 50.0,
        2 => 20.0,
        1 => 5.0,
        _ => 0.0,
    };
    Battery { level: Some(if pct != 0 { pct as f64 } else { level }), charging: matches!(at(r, 2), 1 | 2 | 4), voltage: None }
}

/// BATTERY_STATUS: level, next level, status.
fn battery_status(r: &[u8]) -> Battery {
    Battery { level: (at(r, 0) != 0).then(|| at(r, 0) as f64), charging: matches!(at(r, 2), 1 | 2 | 4), voltage: None }
}

/// BATTERY_VOLTAGE: millivolts, flags (bit 7 charging).
fn battery_voltage(r: &[u8]) -> Battery {
    let mv = u16_at(r, 0);
    Battery { level: Some(voltage_percent(mv) as f64), charging: at(r, 2) & 0x80 != 0 && at(r, 2) & 0x03 != 1, voltage: Some(mv) }
}

/// ADC_MEASUREMENT: millivolts, flags (bit 0 valid, bit 1 charging).
fn adc_battery(r: &[u8]) -> Option<Battery> {
    let mv = u16_at(r, 0);
    (at(r, 2) & 0x01 != 0).then(|| Battery { level: Some(voltage_percent(mv) as f64), charging: at(r, 2) & 0x02 != 0, voltage: Some(mv) })
}

/// Charge estimate from a Li-ion cell voltage (Solaar's table).
pub fn voltage_percent(mv: u16) -> u8 {
    const T: [(u16, u8); 13] = [
        (4186, 100),
        (4067, 90),
        (3989, 80),
        (3922, 70),
        (3859, 60),
        (3811, 50),
        (3778, 40),
        (3751, 30),
        (3717, 20),
        (3671, 10),
        (3646, 5),
        (3579, 2),
        (3500, 0),
    ];
    if mv >= T[0].0 {
        return 100;
    }
    for w in T.windows(2) {
        let ((vh, ph), (vl, pl)) = (w[0], w[1]);
        if mv >= vl {
            return (pl as f64 + (ph - pl) as f64 * (mv - vl) as f64 / (vh - vl) as f64).round() as u8;
        }
    }
    0
}

fn read_string(l: &mut Link, feature: u16, skip: usize) -> Option<String> {
    let len = at(&l.call(feature, 0, &[]).ok()?, 0) as usize;
    let mut name = Vec::new();
    while name.len() < len {
        let r = l.call(feature, 1, &[name.len() as u8]).ok()?;
        let part = r.get(skip..).unwrap_or(&[]);
        let take = part.len().min(len - name.len());
        if take == 0 {
            break;
        }
        name.extend_from_slice(&part[..take]);
    }
    let s = String::from_utf8_lossy(&name).trim_end_matches('\0').trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn name(l: &mut Link) -> Option<String> {
    if l.has(f::DEVICE_NAME) { read_string(l, f::DEVICE_NAME, 0) } else { None }
}

fn kind(l: &mut Link) -> Option<DeviceKind> {
    if !l.has(f::DEVICE_NAME) {
        return None;
    }
    let k = at(&l.call(f::DEVICE_NAME, 2, &[]).ok()?, 0);
    Some(device_kind(devices::kind_from(k, true)))
}

fn device_kind(k: devices::Kind) -> DeviceKind {
    use devices::Kind as K;
    match k {
        K::Mouse | K::Trackball | K::Touchpad => DeviceKind::Mouse,
        K::Keyboard | K::Numpad => DeviceKind::Keyboard,
        K::Headset => DeviceKind::Headset,
        _ => DeviceKind::Other,
    }
}

/// The main firmware: "RBM 14.00.B0010".
fn firmware(l: &mut Link) -> Option<String> {
    let count = at(&l.call(f::DEVICE_FW_VERSION, 0, &[]).ok()?, 0);
    for i in 0..count {
        let r = l.call(f::DEVICE_FW_VERSION, 1, &[i]).ok()?;
        if at(&r, 0) & 0x0F == 0 {
            let prefix = String::from_utf8_lossy(r.get(1..4).unwrap_or(&[])).trim_end_matches('\0').to_string();
            let build = u16_at(&r, 6);
            let mut v = format!("{prefix} {:02X}.{:02X}", at(&r, 4), at(&r, 5));
            if build != 0 {
                v += &format!(".B{build:04X}");
            }
            return Some(v);
        }
    }
    None
}

fn unit_id(l: &mut Link) -> Option<String> {
    let r = l.call(f::DEVICE_FW_VERSION, 0, &[]).ok()?;
    let id = r.get(1..5)?;
    (id != [0; 4]).then(|| id.iter().map(|b| format!("{b:02X}")).collect())
}

impl Drop for Peripheral {
    fn drop(&mut self) {
        if let (None, Some(sn)) = (self.slot, &self.info.serial) {
            DIRECT.lock().unwrap_or_else(|e| e.into_inner()).retain(|s| s != sn);
        }
    }
}

impl Device for Peripheral {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        self.process_notifications();
        if self.gone {
            return Err(Error::Other("unpaired from its receiver".into()));
        }
        if self.slot.is_some() && !self.online && connected_directly(self.info.serial.as_ref()) {
            return Err(Error::Other("connected over Bluetooth or USB instead".into()));
        }
        if !self.online {
            if self.pinged.elapsed() >= PING_OFFLINE {
                self.connect();
            }
        } else if !self.ready {
            self.connect();
        } else if self.refreshed.elapsed() >= REFRESH {
            self.read_all();
        }
        // A direct link that fails is gone (unplugged, Bluetooth dropped):
        // let the worker rescan. A receiver keeps answering for its slots.
        if self.slot.is_none() && !self.online {
            return Err(Error::Offline);
        }
        Ok(Snapshot { sensors: self.sensors(), controls: if self.ready { self.controls() } else { Vec::new() } })
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        if self.persistent(id) {
            self.saved.values.insert(id.to_string(), value.clone());
            self.saved.store(&self.saved_key);
        }
        if !self.online || !self.ready {
            return Ok(()); // applied when it connects
        }
        let r = self.apply(id, value);
        if let Err(e) = &r {
            self.note_error(e);
        }
        r
    }

    fn restore_toggles(&self) -> bool {
        false
    }

    fn input_keys(&self) -> Vec<InputKey> {
        let mut out = self.buttons.as_ref().map(|b| b.input_keys()).unwrap_or_default();
        out.extend(self.wheels.input_keys());
        out
    }

    fn poll_input(&mut self) -> Vec<InputEvent> {
        self.process_notifications();
        std::mem::take(&mut self.events)
    }

    fn poll_interval(&self) -> Option<Duration> {
        Some(Duration::from_secs(1))
    }

    fn connected(&self) -> bool {
        self.online
    }

    fn release(&mut self) -> Result<()> {
        // Give diverted buttons and wheels back to the device.
        if !self.online || !self.ready {
            return Ok(());
        }
        let mut first: Option<Error> = None;
        if let Some(b) = &mut self.buttons {
            let diverted: Vec<u16> = b.keys.iter().filter(|k| k.reporting & buttons::report::DIVERTED != 0 && b.mode(k.cid) != buttons::Mode::Normal).map(|k| k.cid).collect();
            for cid in diverted {
                if let Err(e) = b.set_mode(&mut self.link, cid, buttons::Mode::Normal) {
                    first.get_or_insert(e);
                }
            }
        }
        for id in [buttons::THUMB_DIVERT, buttons::WHEEL_DIVERT, buttons::CROWN_DIVERT] {
            if let Some(s) = self.settings.iter_mut().find(|s| s.owns(id)) {
                if s.controls().iter().any(|c| c.value == ControlValue::Bool(true)) {
                    if let Err(e) = s.write(&mut self.link, id, &ControlValue::Bool(false)) {
                        first.get_or_insert(e);
                    }
                }
            }
        }
        first.map_or(Ok(()), Err)
    }
}

/// "MX Master 3S" → "mx-master-3s".
fn slug(name: &str) -> String {
    let s: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "logitech".into() } else { s }
}

/// Windows lists each HID collection as its own path,
/// `\\?\hid#vid_046d&pid_c52b&mi_02&col01#8&2a3b&0&0000#{guid}`: the short
/// and long report collections of one interface differ only in `&colNN`
/// and the last part of the instance id.
fn interface_key(path: &[u8]) -> String {
    let p = String::from_utf8_lossy(path).to_lowercase();
    let mut parts: Vec<String> = p.split('#').map(str::to_string).collect();
    if parts.len() < 3 {
        return p;
    }
    if let Some(i) = parts[1].find("&col") {
        let rest = parts[1][i + 4..].trim_start_matches(|c: char| c.is_ascii_hexdigit()).to_string();
        parts[1] = format!("{}{rest}", &parts[1][..i]);
    }
    if let Some((head, _)) = parts[2].rsplit_once('&') {
        parts[2] = head.to_string();
    }
    parts.join("#")
}

/// Linux: hidraw nodes the logitech-dj driver makes for each device of a
/// receiver sit under the receiver's own HID device. Those devices are
/// reached through the receiver instead.
#[cfg(target_os = "linux")]
fn receiver_child(path: &[u8]) -> bool {
    let p = String::from_utf8_lossy(path);
    let Some(node) = p.strip_prefix("/dev/") else { return false };
    let Ok(dev) = std::fs::canonicalize(format!("/sys/class/{}/device", node.replace("hidraw", "hidraw/hidraw"))) else { return false };
    let parent = dev.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    // A HID device is named "BBBB:VVVV:PPPP.NNNN".
    parent.len() >= 19 && parent.as_bytes()[4] == b':' && parent.as_bytes()[9] == b':' && parent.contains('.')
}

#[cfg(not(target_os = "linux"))]
fn receiver_child(_path: &[u8]) -> bool {
    false
}

/// The HID++ handles of one interface: the main one (short reports, or
/// everything on Linux) and, on Windows, the long-report collection.
struct Interface<'a> {
    main: &'a hidapi::DeviceInfo,
    long: Option<&'a hidapi::DeviceInfo>,
    ble: bool,
}

fn interfaces<'a>(all: &[&'a hidapi::DeviceInfo]) -> Vec<Interface<'a>> {
    let mut out: Vec<Interface<'a>> = Vec::new();
    for d in all.iter().copied().filter(|d| d.vendor_id() == VID && matches!(d.usage_page(), PAGE_HIDPP | PAGE_HIDPP_BLE)) {
        let key = interface_key(d.path().to_bytes());
        let ble = d.usage_page() == PAGE_HIDPP_BLE;
        let is_long = d.usage_page() == PAGE_HIDPP && d.usage() == 0x0002;
        match out.iter_mut().find(|i| interface_key(i.main.path().to_bytes()) == key) {
            Some(i) => {
                i.ble |= ble;
                if i.main.path() == d.path() {
                    continue; // Linux: one hidraw node for all collections
                }
                if is_long {
                    i.long = Some(d);
                } else {
                    // Short collection found after the long one.
                    i.long = Some(std::mem::replace(&mut i.main, d));
                }
            }
            None => out.push(Interface { main: d, long: None, ble }),
        }
    }
    out
}

fn open_channel(api: &hidapi::HidApi, i: &Interface, device: u8) -> Option<Channel> {
    let main = api.open_path(i.main.path()).map_err(|e| log::debug!("logitech: opening {:?}: {e}", i.main.path())).ok()?;
    let long = i.long.and_then(|l| api.open_path(l.path()).ok());
    // Bluetooth devices only have long reports.
    Some(Channel::new(main, long, device, !i.ble))
}

pub(crate) fn discover(api: &hidapi::HidApi, all: &[&hidapi::DeviceInfo], ids: &mut Ids, skip_paths: &[CString], out: &mut Vec<(CString, Box<dyn Device>)>) {
    let held = |p: &CString| skip_paths.iter().any(|s| s == p);
    // Cable and Bluetooth devices first, so their receiver slots can step aside.
    let mut list = interfaces(all);
    list.sort_by_key(|i| receiver::kind(i.main.product_id()).is_some());
    for i in list {
        let path = i.main.path().to_owned();
        let pid = i.main.product_id();
        if receiver_child(path.to_bytes()) {
            continue;
        }
        if let Some((rkind, slots, _)) = receiver::kind(pid) {
            // The receiver itself.
            if !held(&path) {
                if let Some(ch) = open_channel(api, &i, hidpp::DIRECT) {
                    let id = ids.next(receiver::Receiver::slug(pid));
                    match receiver::Receiver::open(ch, pid, id.clone()) {
                        Ok(r) => {
                            log::info!("opened Logitech {} ({id})", rkind.name());
                            out.push((path.clone(), Box::new(r)));
                        }
                        Err(e) => log::warn!("logitech receiver {pid:04x}: {e}"),
                    }
                }
            }
            // Its paired devices.
            let Some(mut ch) = open_channel(api, &i, hidpp::DIRECT) else { continue };
            receiver::enable_notifications(&mut ch);
            for n in 1..=slots {
                let slot_path = CString::new([path.to_bytes(), format!("#{n}").as_bytes()].concat()).expect("no NUL");
                if held(&slot_path) {
                    continue;
                }
                let Some(s) = receiver::slot(&mut ch, rkind, n) else { continue };
                if connected_directly(s.serial.as_ref()) {
                    continue;
                }
                let Some(dch) = open_channel(api, &i, n) else { continue };
                let desc = devices::by_wpid(s.wpid);
                let model = desc.map(|d| d.name.to_string()).or(s.codename.clone()).unwrap_or_else(|| format!("Logitech device {:04X}", s.wpid));
                let kind = desc.map(|d| device_kind(d.kind)).unwrap_or_else(|| device_kind(devices::kind_from(s.kind, false)));
                let registers = desc.map(|d| d.registers).unwrap_or(0);
                let id = ids.next(&slug(desc.map(|d| d.codename).filter(|c| !c.is_empty()).unwrap_or(&model)));
                let p = Peripheral::open(dch, Some(n), id.clone(), model, kind, s.wpid, s.serial.clone(), registers);
                log::info!(
                    "opened Logitech {} ({id}) in slot {n} of the {}{}",
                    p.info.model,
                    rkind.name(),
                    if p.online { "" } else { ", asleep or switched off" }
                );
                out.push((slot_path, Box::new(p)));
            }
            receiver::request_connections(&mut ch);
            continue;
        }
        // A device on a cable or Bluetooth.
        if held(&path) {
            continue;
        }
        let Some(mut ch) = open_channel(api, &i, hidpp::DIRECT) else { continue };
        match ch.protocol() {
            Ok((major, _)) if major >= 2 => {}
            Ok(_) => continue,
            Err(e) => {
                log::debug!("logitech {pid:04x}: no HID++ answer: {e}");
                continue;
            }
        }
        let desc = devices::by_product(pid);
        let product = i.main.product_string().map(|s| s.trim_start_matches("Logitech ").trim().to_string()).filter(|s| !s.is_empty());
        let model = desc.map(|d| d.name.to_string()).or(product).unwrap_or_else(|| format!("Logitech device {pid:04X}"));
        let kind = desc.map(|d| device_kind(d.kind)).unwrap_or(DeviceKind::Other);
        // Named like the same device in a receiver slot, so it keeps its
        // settings whichever way it connects.
        let id = ids.next(&slug(desc.map(|d| d.codename).filter(|c| !c.is_empty()).unwrap_or(&model)));
        let p = Peripheral::open(ch, None, id.clone(), model, kind, pid, None, 0);
        if !p.online {
            continue;
        }
        log::info!("opened Logitech {} ({id}) over {}", p.info.model, if i.ble { "Bluetooth" } else { "USB" });
        out.push((path, Box::new(p)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_paths() {
        assert_eq!(slug("MX Master 3S"), "mx-master-3s");
        assert_eq!(slug("Performance MX"), "performance-mx");
        assert_eq!(
            interface_key(br"\\?\hid#vid_046d&pid_c548&mi_02&col01#7&1a2b&0&0000#{4d1e55b2}"),
            interface_key(br"\\?\hid#vid_046d&pid_c548&mi_02&col02#7&1a2b&0&0001#{4d1e55b2}")
        );
        assert_ne!(interface_key(b"/dev/hidraw2"), interface_key(b"/dev/hidraw3"));
    }

    #[test]
    fn battery_decoding() {
        assert_eq!(unified_battery(&[55, 4, 0, 0]).level, Some(55.0));
        assert!(unified_battery(&[55, 4, 1, 1]).charging);
        assert_eq!(voltage_percent(4100), 93);
        assert_eq!(voltage_percent(3400), 0);
        assert_eq!(voltage_percent(4300), 100);
    }
}
