//! Corsair peripherals using the "Bragi" protocol (2020+): keyboards (K55
//! RGB PRO/XT, K57, K60, K65 MINI, K70 PRO/TKL/CORE, K95 PLATINUM XT, K100),
//! mice (M55, Katar PRO, Dark Core RGB PRO, Harpoon/Ironclaw RGB Wireless,
//! Sabre RGB PRO, M65 RGB ULTRA, Nightsabre, Scimitar Elite), Slipstream
//! wireless receivers and Bragi headsets (HS80, Virtuoso). **Experimental:
//! none of these has been tested on real hardware.**
//!
//! Protocol summary (see [`proto`] for the packet layout):
//! * commands go to the vendor interface (usage page 0xFF42, interface 1 on
//!   mice/keyboards, 3 or 4 on headsets); each is answered on the same
//!   handle;
//! * key/button events and notifications arrive on a second vendor
//!   interface (interface 2) as `?? 02 <bitmap>`, `?? 05 …` (dial) and
//!   `?? 01 <property> 00 <value>`;
//! * software mode = property 0x03 set to 2; lighting is streamed to a
//!   handle opened on resource 0x01 (planar) or 0x22 (`12 00` + triplets);
//!   hardware mode (0x03 = 1) hands everything back to the firmware;
//! * devices behind a receiver are addressed with header `0x08 | slot`;
//!   property 0x36 of the receiver is a bitmap of connected slots and
//!   property 0x12 of a slot its USB PID;
//! * `08 12` is a keepalive.
//!
//! References (GPL): ckb-next (`usb_bragi.c`, `bragi_proto.h`,
//! `bragi_common.c`, `device_bragi.c`, `led_bragi.c`, `dpi_bragi.c`,
//! `bragi_notification.c`, `keymap.c`), OpenRGB
//! (`CorsairPeripheralV2Controller`, `CorsairK55RGBPROXTController`),
//! OpenLinkHub (`src/devices/*`, including `slipstream` and
//! `headsetdongle`).

mod bus;
pub mod keyboards;
pub mod models;
pub mod proto;

use self::bus::Bus;
use self::keyboards::{SpecialKey, KEY_DIAL_CCW, KEY_DIAL_CW, KEY_WINLOCK};
use self::models::{Lighting, Model, Role};
use crate::model::*;
use crate::{Device, Error, Result};
use std::ffi::{CStr, CString};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use self::models::{model_for_pid, receiver_for_pid};

/// Vendor usage page of the Bragi command interface.
pub const USAGE_PAGE: u16 = 0xff42;

pub const CTRL_WIN_LOCK: &str = "win_lock";
pub const CTRL_LOCK_ALT_TAB: &str = "lock_alt_tab";
pub const CTRL_LOCK_ALT_F4: &str = "lock_alt_f4";
pub const CTRL_LOCK_SHIFT_TAB: &str = "lock_shift_tab";
pub const CTRL_DPI: &str = "dpi";
pub const CTRL_POLL_RATE: &str = "poll_rate";
pub const CTRL_ANGLE_SNAP: &str = "angle_snap";
pub const CTRL_SIDETONE: &str = "sidetone";
pub const CTRL_SIDETONE_LEVEL: &str = "sidetone_level";

const POLL_RATES: [&str; 7] = ["125 Hz", "250 Hz", "500 Hz", "1000 Hz", "2000 Hz", "4000 Hz", "8000 Hz"];
/// DPI stages the DPI buttons step through (clamped to the sensor range).
const DPI_STAGES: [u16; 6] = [400, 800, 1200, 1600, 3200, 6400];
const SNIPER_DPI: u16 = 400;
/// Property 0x45: enables the "performance" key locks of property 0x4A.
const PROP_PERFORMANCE: u8 = 0x45;
/// Mouse DPI as one property (models without separate X/Y).
const PROP_DPI: u8 = 0x20;
const PROP_SIDETONE_OFF: u8 = 0x46;
const PROP_SIDETONE_LEVEL: u8 = 0x47;
/// Handle used for the key-assignment table.
const HANDLE_ASSIGN: u8 = 0x01;

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// One entry of the input list: a keyboard special key or mouse button.
struct Input {
    index: u16,
    id: &'static str,
    name: &'static str,
    assignable: bool,
    role: Role,
}

pub struct BragiDevice {
    bus: Arc<Bus>,
    slot: u8,
    model: &'static Model,
    info: DeviceInfo,
    /// Lighting format in use (`None` if lighting is unavailable).
    lighting: Lighting,
    inputs: Vec<Input>,
    pressed: Vec<bool>,
    win_lock: bool,
    lock_alt_tab: bool,
    lock_alt_f4: bool,
    lock_shift_tab: bool,
    dpi: Option<u16>,
    sniper_restore: Option<u16>,
    poll_code: Option<u8>,
    angle_snap: Option<bool>,
    sidetone: Option<bool>,
    sidetone_level: Option<f64>,
    battery: Option<(f64, proto::BatteryStatus)>,
    last_colors: Vec<Rgb>,
    last_light_write: Option<Instant>,
}

impl BragiDevice {
    /// Open the device at `slot` of `bus` (0 = wired) and put it in
    /// software mode.
    pub fn open(bus: Arc<Bus>, slot: u8, model: &'static Model, id: String, serial: Option<String>) -> Result<Self> {
        let mut inputs: Vec<Input> = Vec::new();
        if let Some(kb) = model.keyboard {
            inputs.extend(kb.keys.iter().map(|k: &SpecialKey| Input {
                index: k.index,
                id: k.id,
                name: k.name,
                assignable: k.assignable,
                role: Role::Key,
            }));
            if kb.dial {
                for (id, name) in [(KEY_DIAL_CW, "Dial clockwise"), (KEY_DIAL_CCW, "Dial counter-clockwise")] {
                    inputs.push(Input { index: u16::MAX, id, name, assignable: true, role: Role::Key });
                }
            }
        }
        for b in model.buttons.iter().filter(|b| b.role != Role::Os) {
            inputs.push(Input { index: b.bit as u16, id: b.id, name: b.name, assignable: b.role == Role::Key, role: b.role });
        }
        let mut d = Self {
            bus,
            slot,
            model,
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
            lighting: Lighting::None,
            pressed: vec![false; inputs.len()],
            inputs,
            win_lock: false,
            lock_alt_tab: true,
            lock_alt_f4: true,
            lock_shift_tab: false,
            dpi: None,
            sniper_restore: None,
            poll_code: None,
            angle_snap: None,
            sidetone: None,
            sidetone_level: None,
            battery: None,
            last_colors: Vec::new(),
            last_light_write: None,
        };
        let fw = d.bus.transact(&d.pkt_get(proto::PROP_APP_VERSION))?;
        d.info.firmware = proto::firmware_string(&fw);
        d.enter_software_mode()?;
        Ok(d)
    }

    fn size(&self) -> usize {
        self.bus.size
    }
    fn pkt_get(&self, prop: u8) -> Vec<u8> {
        proto::get(self.size(), self.slot, prop)
    }
    fn get(&self, prop: u8) -> Result<u32> {
        Ok(self.bus.command(&self.pkt_get(prop))?.value)
    }
    fn set(&self, prop: u8, value: u32) -> Result<()> {
        self.bus.command(&proto::set(self.size(), self.slot, prop, value)).map(|_| ())
    }

    fn open_handle(&self, handle: u8, resource: u16) -> Result<u8> {
        let p = proto::open_handle(self.size(), self.slot, handle, resource);
        let mut r = self.bus.command_status(&p)?;
        if r.status == proto::STATUS_BUSY {
            // Already open (e.g. left over by another program): reopen.
            self.close_handle(handle);
            r = self.bus.command_status(&p)?;
        }
        Ok(r.status)
    }

    fn close_handle(&self, handle: u8) {
        let _ = self.bus.command_status(&proto::close_handle(self.size(), self.slot, handle));
    }

    /// Write `data` to an open handle: the first packet is acknowledged,
    /// continuation packets may not be.
    fn write_handle(&self, handle: u8, data: &[u8]) -> Result<()> {
        let packets = proto::write_handle(self.size(), self.slot, handle, data);
        let r = self.bus.command_status(&packets[0])?;
        if r.status != proto::STATUS_OK {
            return Err(Error::Unsupported { cmd: proto::CMD_WRITE });
        }
        for p in &packets[1..] {
            self.bus.send_unacked(p)?;
        }
        Ok(())
    }

    fn lighting_enabled(&self) -> bool {
        if self.model.lighting == Lighting::None || self.model.led_count == 0 {
            return false;
        }
        !(self.slot != 0 && self.model.keyboard.is_some_and(|k| !k.wireless_lighting))
    }

    /// Open the lighting handle; ckb-next / OpenRGB fall back between the
    /// planar (0x01) and triplet (0x22) resources when one is refused.
    fn open_lighting(&mut self) -> Result<()> {
        self.lighting = Lighting::None;
        if !self.lighting_enabled() {
            return Ok(());
        }
        let order = match self.model.lighting {
            Lighting::Triplets => [Lighting::Triplets, Lighting::Planar],
            _ => [Lighting::Planar, Lighting::Triplets],
        };
        for fmt in order {
            let res = if fmt == Lighting::Triplets { proto::RES_LIGHTING_ALT } else { proto::RES_LIGHTING };
            match self.open_handle(proto::HANDLE_LIGHTING, res)? {
                proto::STATUS_OK => {
                    self.lighting = fmt;
                    return Ok(());
                }
                s => log::debug!("{}: lighting resource {res:#04x} refused ({s:#04x})", self.info.id),
            }
        }
        log::warn!("{}: device refused both lighting resources", self.info.id);
        Ok(())
    }

    /// Key-assignment table: keyboards of the assignment family and mice
    /// (1 = native, 0 = report to software only).
    fn assignment_table(&self, restore: bool) -> Option<Vec<u8>> {
        if let Some(kb) = self.model.keyboard {
            return keyboards::assignment_table(kb, restore);
        }
        models::mouse_assignment_table(self.model.buttons, restore)
    }

    fn write_assignment(&self, restore: bool) -> Result<()> {
        let Some(table) = self.assignment_table(restore) else { return Ok(()) };
        if self.open_handle(HANDLE_ASSIGN, proto::RES_KEY_ASSIGNMENT)? != proto::STATUS_OK {
            log::warn!("{}: key assignment not supported", self.info.id);
            return Ok(());
        }
        let r = self.write_handle(HANDLE_ASSIGN, &table);
        self.close_handle(HANDLE_ASSIGN);
        r
    }

    fn enter_software_mode(&mut self) -> Result<()> {
        self.set(proto::PROP_MODE, proto::MODE_SOFTWARE)?;
        pause(20);
        self.open_lighting()?;
        if self.lighting != Lighting::None {
            // Firmware brightness to full: opendevicecontrol dims in software.
            // Devices with only coarse brightness ignore it.
            let _ = self.set(proto::PROP_BRIGHTNESS, 1000);
        }
        if let Err(e) = self.write_assignment(false) {
            log::warn!("{}: writing key assignment: {e}", self.info.id);
        }
        self.read_settings();
        self.apply_key_locks();
        self.last_colors.clear();
        Ok(())
    }

    /// Read the current values of the device settings opendevicecontrol exposes.
    /// Anything the device refuses is simply not offered.
    fn read_settings(&mut self) {
        if self.model.max_poll > 0 {
            self.poll_code = self.get(proto::PROP_POLL_RATE).ok().map(|v| v as u8).filter(|v| (1..=7).contains(v));
        }
        if let Some(dpi) = self.model.dpi {
            let prop = if dpi.xy { proto::PROP_DPI_X } else { PROP_DPI };
            self.dpi = self.get(prop).ok().map(|v| v as u16).filter(|v| *v > 0);
            self.angle_snap = self.get(proto::PROP_ANGLE_SNAP).ok().map(|v| v != 0);
        }
        if self.model.headset.is_some_and(|h| h.sidetone) {
            self.sidetone = self.get(PROP_SIDETONE_OFF).ok().map(|v| v == 0);
            self.sidetone_level = self.get(PROP_SIDETONE_LEVEL).ok().map(|v| (v as f64 / 10.0).clamp(0.0, 100.0));
        }
    }

    fn has_key_locks(&self) -> bool {
        self.model.keyboard.is_some_and(|k| k.key_locks)
    }

    fn apply_key_locks(&self) {
        if !self.has_key_locks() {
            return;
        }
        let mask = keyboards::key_lock_mask(self.win_lock, self.lock_alt_tab, self.lock_alt_f4, self.lock_shift_tab);
        let _ = self.set(PROP_PERFORMANCE, u32::from(self.win_lock));
        if let Err(e) = self.set(proto::PROP_KEY_LOCKS, mask) {
            log::warn!("{}: key locks: {e}", self.info.id);
        }
    }

    fn set_dpi(&mut self, value: u16) -> Result<()> {
        let Some(spec) = self.model.dpi else { return Err(Error::UnknownControl(CTRL_DPI.into())) };
        let v = value.clamp(spec.min, spec.max) as u32;
        if spec.xy {
            self.set(proto::PROP_DPI_X, v)?;
            self.set(proto::PROP_DPI_Y, v)?;
        } else {
            self.set(PROP_DPI, v)?;
        }
        self.dpi = Some(v as u16);
        Ok(())
    }

    /// DPI buttons: step through [`DPI_STAGES`].
    fn dpi_button(&mut self, role: Role, pressed: bool) {
        let Some(spec) = self.model.dpi else { return };
        let stages: Vec<u16> = DPI_STAGES.iter().copied().filter(|s| (spec.min..=spec.max).contains(s)).collect();
        let cur = self.dpi.unwrap_or(800);
        let next = match (role, pressed) {
            (Role::DpiUp, true) => stages.iter().copied().find(|&s| s > cur).unwrap_or(cur),
            (Role::DpiDown, true) => stages.iter().rev().copied().find(|&s| s < cur).unwrap_or(cur),
            (Role::DpiCycle, true) => stages.iter().copied().find(|&s| s > cur).or(stages.first().copied()).unwrap_or(cur),
            (Role::Sniper, true) => {
                self.sniper_restore.get_or_insert(cur);
                SNIPER_DPI.max(spec.min)
            }
            (Role::Sniper, false) => match self.sniper_restore.take() {
                Some(v) => v,
                None => return,
            },
            _ => return,
        };
        if next != cur {
            if let Err(e) = self.set_dpi(next) {
                log::warn!("{}: DPI: {e}", self.info.id);
            }
        }
    }

    fn toggle(id: &str, name: &str, value: bool, help: &str) -> Control {
        Control {
            id: id.into(),
            name: name.into(),
            kind: ControlKind::Toggle,
            value: ControlValue::Bool(value),
            help: (!help.is_empty()).then(|| help.into()),
        }
    }

    fn on_notification(&mut self, p: &[u8]) {
        // `?? 01 <property> 00 <value LE>`
        if p.len() < 6 {
            return;
        }
        let value = u16::from_le_bytes([p[4], p[5]]) as u32;
        match p[2] {
            proto::PROP_BATTERY_LEVEL => {
                let status = self.battery.map_or(proto::BatteryStatus::Unknown, |b| b.1);
                self.battery = Some((proto::battery_percent(value), status));
            }
            proto::PROP_BATTERY_STATUS => {
                let level = self.battery.map_or(0.0, |b| b.0);
                self.battery = Some((level, proto::BatteryStatus::from_raw(p[4] as u32)));
            }
            _ => {}
        }
    }
}

impl Device for BragiDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        // Keepalive (CUE: every ~50 s; doubles as a presence check: a
        // device that went to sleep behind its receiver stops answering).
        self.bus.transact(&proto::ping(self.size(), self.slot))?;
        if self.slot != 0 {
            self.bus.keepalive_receiver(Duration::from_secs(20));
        }

        let mut sensors = Vec::new();
        if self.model.wireless {
            let level = self.get(proto::PROP_BATTERY_LEVEL);
            let status = self.get(proto::PROP_BATTERY_STATUS);
            match (level, status) {
                (Ok(l), Ok(s)) => self.battery = Some((proto::battery_percent(l), proto::BatteryStatus::from_raw(s))),
                (Err(e), _) | (_, Err(e)) if self.slot != 0 => return Err(e),
                _ => {}
            }
            let (level, status) = self.battery.unwrap_or((0.0, proto::BatteryStatus::Unknown));
            sensors.push(Sensor::new("battery", "Battery", SensorKind::Battery, level));
            let charging = matches!(status, proto::BatteryStatus::Charging);
            sensors.push(Sensor::new("charging", "Charging", SensorKind::Charging, f64::from(u8::from(charging))));
        }

        let mut controls = Vec::new();
        if self.has_key_locks() {
            controls.push(Self::toggle(
                CTRL_WIN_LOCK,
                "Win lock",
                self.win_lock,
                "Disables the Windows key (and the shortcuts below). Also toggled by the keyboard's lock key.",
            ));
            controls.push(Self::toggle(CTRL_LOCK_ALT_TAB, "Win lock also blocks Alt+Tab", self.lock_alt_tab, ""));
            controls.push(Self::toggle(CTRL_LOCK_ALT_F4, "Win lock also blocks Alt+F4", self.lock_alt_f4, ""));
            controls.push(Self::toggle(CTRL_LOCK_SHIFT_TAB, "Win lock also blocks Shift+Tab", self.lock_shift_tab, ""));
        }
        if let (Some(spec), Some(dpi)) = (self.model.dpi, self.dpi) {
            controls.push(Control {
                id: CTRL_DPI.into(),
                name: "DPI".into(),
                kind: ControlKind::Number { min: spec.min as f64, max: spec.max as f64, step: 50.0, unit: "DPI".into() },
                value: ControlValue::Number(dpi as f64),
                help: Some("Sensor resolution. The DPI buttons step through 400/800/1200/1600/3200/6400.".into()),
            });
        }
        if let Some(code) = self.poll_code {
            let options: Vec<String> = POLL_RATES[..self.model.max_poll.clamp(1, 7) as usize].iter().map(|s| s.to_string()).collect();
            let value = POLL_RATES.get(code as usize - 1).copied().unwrap_or("1000 Hz").to_string();
            controls.push(Control {
                id: CTRL_POLL_RATE.into(),
                name: "Polling rate".into(),
                kind: ControlKind::Choice { options },
                value: ControlValue::Choice(value),
                help: None,
            });
        }
        if let Some(snap) = self.angle_snap {
            controls.push(Self::toggle(CTRL_ANGLE_SNAP, "Angle snapping", snap, "Straightens nearly horizontal/vertical movements."));
        }
        if let Some(on) = self.sidetone {
            controls.push(Self::toggle(CTRL_SIDETONE, "Sidetone", on, "Hear your own voice through the headset."));
        }
        if let Some(level) = self.sidetone_level {
            controls.push(Control {
                id: CTRL_SIDETONE_LEVEL.into(),
                name: "Sidetone level".into(),
                kind: ControlKind::Number { min: 0.0, max: 100.0, step: 1.0, unit: "%".into() },
                value: ControlValue::Number(level),
                help: None,
            });
        }
        Ok(Snapshot { sensors, controls })
    }

    fn poll_interval(&self) -> Option<Duration> {
        Some(Duration::from_secs(10))
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        let bad = || Error::InvalidValue(id.into());
        match (id, value) {
            (CTRL_WIN_LOCK, ControlValue::Bool(v)) => self.win_lock = *v,
            (CTRL_LOCK_ALT_TAB, ControlValue::Bool(v)) => self.lock_alt_tab = *v,
            (CTRL_LOCK_ALT_F4, ControlValue::Bool(v)) => self.lock_alt_f4 = *v,
            (CTRL_LOCK_SHIFT_TAB, ControlValue::Bool(v)) => self.lock_shift_tab = *v,
            (CTRL_DPI, ControlValue::Number(v)) if self.dpi.is_some() => return self.set_dpi(v.round().clamp(0.0, 65535.0) as u16),
            (CTRL_POLL_RATE, ControlValue::Choice(s)) if self.poll_code.is_some() => {
                let pos = POLL_RATES[..self.model.max_poll.clamp(1, 7) as usize].iter().position(|r| r == s).ok_or_else(bad)?;
                let code = pos as u8 + 1;
                self.set(proto::PROP_POLL_RATE, code as u32)?;
                self.poll_code = Some(code);
                return Ok(());
            }
            (CTRL_ANGLE_SNAP, ControlValue::Bool(v)) if self.angle_snap.is_some() => {
                self.set(proto::PROP_ANGLE_SNAP, u32::from(*v))?;
                self.angle_snap = Some(*v);
                return Ok(());
            }
            (CTRL_SIDETONE, ControlValue::Bool(v)) if self.sidetone.is_some() => {
                // Inverted: 0 = sidetone on (OpenLinkHub hs80maxW).
                self.set(PROP_SIDETONE_OFF, u32::from(!*v))?;
                self.sidetone = Some(*v);
                return Ok(());
            }
            (CTRL_SIDETONE_LEVEL, ControlValue::Number(v)) if self.sidetone_level.is_some() => {
                let v = v.clamp(0.0, 100.0);
                self.set(PROP_SIDETONE_LEVEL, (v * 10.0).round() as u32)?;
                self.sidetone_level = Some(v);
                return Ok(());
            }
            (CTRL_WIN_LOCK | CTRL_LOCK_ALT_TAB | CTRL_LOCK_ALT_F4 | CTRL_LOCK_SHIFT_TAB, _) => return Err(bad()),
            _ => return Err(Error::UnknownControl(id.into())),
        }
        if !self.has_key_locks() {
            return Err(Error::UnknownControl(id.into()));
        }
        self.apply_key_locks();
        Ok(())
    }

    fn light_zones(&self) -> Vec<LightZone> {
        if self.lighting == Lighting::None {
            return Vec::new();
        }
        if self.model.leds.is_empty() {
            return vec![LightZone { id: "keys".into(), name: "Keys".into(), leds: self.model.led_count }];
        }
        self.model
            .leds
            .iter()
            .map(|(id, name)| LightZone { id: (*id).into(), name: (*name).into(), leds: 1 })
            .collect()
    }

    fn set_lights(&mut self, colors: &[Rgb]) -> Result<()> {
        if self.lighting == Lighting::None {
            return Ok(());
        }
        // Skip identical frames, but refresh once a second in case another
        // program (or a reconnect) changed the LEDs.
        let fresh = self.last_light_write.is_some_and(|t| t.elapsed() < Duration::from_secs(1));
        if colors == self.last_colors.as_slice() && fresh {
            return Ok(());
        }
        let mut buf: Vec<(u8, u8, u8)> = vec![(0, 0, 0); self.model.led_count];
        for (dst, c) in buf.iter_mut().zip(colors) {
            *dst = (c.r, c.g, c.b);
        }
        let data = match self.lighting {
            Lighting::Triplets => proto::triplet_colors(&buf),
            _ => proto::planar_colors(&buf),
        };
        if let Err(e) = self.write_handle(proto::HANDLE_LIGHTING, &data) {
            // The handle may have been closed (device reset, sleep): reopen
            // once before reporting the failure.
            log::debug!("{}: colour write failed ({e}), reopening lighting", self.info.id);
            self.open_lighting()?;
            self.write_handle(proto::HANDLE_LIGHTING, &data)?;
        }
        self.last_colors = colors.to_vec();
        self.last_light_write = Some(Instant::now());
        Ok(())
    }

    fn input_keys(&self) -> Vec<InputKey> {
        self.inputs.iter().map(|k| InputKey { id: k.id.into(), name: k.name.into(), assignable: k.assignable }).collect()
    }

    fn poll_input(&mut self) -> Vec<InputEvent> {
        let mut events = Vec::new();
        for p in self.bus.take_unsolicited(self.slot) {
            if p.len() < 3 {
                continue;
            }
            match p[1] {
                proto::IN_NOTIFY => self.on_notification(&p),
                proto::IN_KEYS => {
                    for i in 0..self.inputs.len() {
                        let input = &self.inputs[i];
                        if input.index == u16::MAX {
                            continue;
                        }
                        let down = keyboards::key_down(&p, input.index);
                        if down == self.pressed[i] {
                            continue;
                        }
                        self.pressed[i] = down;
                        let (id, role) = (input.id, input.role);
                        events.push(if down { InputEvent::Pressed(id.into()) } else { InputEvent::Released(id.into()) });
                        if role != Role::Key {
                            self.dpi_button(role, down);
                        } else if down && id == KEY_WINLOCK && self.has_key_locks() {
                            // The lock key works like it does under iCUE.
                            self.win_lock = !self.win_lock;
                            self.apply_key_locks();
                        }
                    }
                }
                _ => {
                    if let Some(step) = keyboards::dial_step(&p) {
                        let id = if step > 0 { KEY_DIAL_CW } else { KEY_DIAL_CCW };
                        events.push(InputEvent::Pressed(id.into()));
                        events.push(InputEvent::Released(id.into()));
                    }
                }
            }
        }
        events
    }

    fn release(&mut self) -> Result<()> {
        let restored = self.write_assignment(true);
        if let Err(e) = &restored {
            log::warn!("{}: restoring key assignment: {e}", self.info.id);
        }
        if self.has_key_locks() && self.win_lock {
            let _ = self.set(proto::PROP_KEY_LOCKS, 0);
        }
        self.close_handle(proto::HANDLE_LIGHTING);
        // Hardware mode matters most, so its error wins.
        self.set(proto::PROP_MODE, proto::MODE_HARDWARE).and(restored)
    }
}

// ------------------------------------------------------------- discovery

/// Path that identifies the device in `slot` of a receiver.
fn slot_path(receiver: &CStr, slot: u8) -> CString {
    let mut v = receiver.to_bytes().to_vec();
    v.extend_from_slice(format!("#bragi-slot{slot}").as_bytes());
    CString::new(v).expect("no interior NUL")
}

/// Packet sizes to try, the model's first. On Windows a write longer than
/// the output report fails, which tells us to try the next size.
fn size_candidates(first: usize) -> Vec<usize> {
    let mut v = vec![first];
    for s in [1024, 128, 64] {
        if !v.contains(&s) {
            v.push(s);
        }
    }
    v
}

fn open_bus(api: &hidapi::HidApi, path: &CStr, first_size: usize, prefix: Option<u8>) -> Result<Bus> {
    let mut last = None;
    let sizes = if prefix.is_some() { vec![first_size] } else { size_candidates(first_size) };
    for size in sizes {
        let dev = api.open_path(path)?;
        let bus = Bus::new(dev, size, path.to_owned(), prefix);
        match bus.transact(&proto::get(size, 0, proto::PROP_MODE)) {
            Ok(_) => return Ok(bus),
            Err(e @ Error::Hid(_)) => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or(Error::NoReply { cmd: proto::CMD_GET }))
}

/// Vendor event interfaces (interface 2) of the same physical device.
fn event_interfaces(api: &hidapi::HidApi, list: &[&hidapi::DeviceInfo], d: &hidapi::DeviceInfo) -> Vec<hidapi::HidDevice> {
    list.iter()
        .filter(|i| {
            i.product_id() == d.product_id()
                && i.interface_number() == 2
                && i.usage_page() >= 0xff00
                && i.serial_number() == d.serial_number()
                && i.path() != d.path()
        })
        .filter_map(|i| api.open_path(i.path()).map_err(|e| log::debug!("bragi event interface: {e}")).ok())
        .collect()
}

fn is_command_interface(d: &hidapi::DeviceInfo, headset_interface: Option<i32>) -> bool {
    if d.usage_page() == USAGE_PAGE {
        return true;
    }
    headset_interface.is_some_and(|i| d.interface_number() == i && d.usage_page() >= 0xff00)
}

/// Find and open Bragi devices; called from [`super::discover`].
pub(super) fn discover(
    api: &hidapi::HidApi,
    list: &[&hidapi::DeviceInfo],
    ids: &mut super::Ids,
    skip_paths: &[CString],
    out: &mut Vec<(CString, Box<dyn Device>)>,
) {
    let held = |p: &CStr| skip_paths.iter().any(|s| s.as_c_str() == p);
    let serial = |d: &hidapi::DeviceInfo| d.serial_number().map(str::to_owned).filter(|s| !s.trim().is_empty());
    let mut done: Vec<(u16, Option<String>)> = Vec::new();

    // Wired devices.
    for d in list {
        let Some(model) = model_for_pid(d.product_id()) else { continue };
        let headset_if = (model.kind == DeviceKind::Headset).then_some(3);
        if !is_command_interface(d, headset_if) {
            continue;
        }
        let key = (d.product_id(), serial(d));
        if done.contains(&key) {
            continue; // another collection of the same device
        }
        done.push(key);
        let id = ids.next(model.slug);
        if held(d.path()) {
            continue;
        }
        let prefix = (model.kind == DeviceKind::Headset).then_some(0x02);
        let size = if prefix.is_some() { 63 } else { model.packet };
        let opened = open_bus(api, d.path(), size, prefix).and_then(|bus| {
            let bus = Arc::new(bus.with_events(event_interfaces(api, list, d), false));
            BragiDevice::open(bus, 0, model, id.clone(), serial(d))
        });
        match opened {
            Ok(dev) => {
                log::info!(
                    "opened {} ({id}, Bragi, experimental), firmware {}",
                    model.name,
                    dev.info().firmware.as_deref().unwrap_or("?")
                );
                out.push((d.path().to_owned(), Box::new(dev)));
            }
            Err(e) => log::warn!("failed to open {}: {e}", model.name),
        }
    }

    // Wireless receivers and the devices paired to them.
    let mut done_rx: Vec<(u16, Option<String>)> = Vec::new();
    for d in list {
        let Some(rx) = receiver_for_pid(d.product_id()) else { continue };
        if !is_command_interface(d, rx.interface) {
            continue;
        }
        let key = (d.product_id(), serial(d));
        if done_rx.contains(&key) {
            continue;
        }
        done_rx.push(key);
        let read_bits = |bus: &Bus| bus.command(&proto::get(bus.size, 0, proto::PROP_SUBDEVICES)).map(|r| r.value);
        let (bus, bits) = match bus::find_open(d.path()) {
            Some(b) => {
                let bits = read_bits(&b);
                (b, bits)
            }
            None => {
                let prefix = rx.headset.then_some(0x02);
                let size = if rx.headset { 63 } else { 64 };
                let events = if rx.headset { Vec::new() } else { event_interfaces(api, list, d) };
                let bus = match open_bus(api, d.path(), size, prefix) {
                    Ok(b) => Arc::new(b.with_events(events, true)),
                    Err(e) => {
                        log::warn!("failed to open {}: {e}", rx.name);
                        continue;
                    }
                };
                // Nothing connected: leave the receiver alone (no mode
                // change) until something pairs or wakes up.
                let first = read_bits(&bus);
                if first.as_ref().is_ok_and(|b| b & 0xfe == 0) {
                    continue;
                }
                // Receivers are driven in software mode while opendevicecontrol uses
                // a device behind them, and handed back when the last one
                // is closed (the bus is dropped).
                if let Err(e) = bus.command(&proto::set(bus.size, 0, proto::PROP_MODE, proto::MODE_SOFTWARE)) {
                    log::warn!("{}: software mode: {e}", rx.name);
                    continue;
                }
                bus.set_on_drop(Some(proto::set(bus.size, 0, proto::PROP_MODE, proto::MODE_HARDWARE)));
                bus::register(&bus);
                let bits = first.or_else(|_| read_bits(&bus));
                (bus, bits)
            }
        };
        let bits = match bits {
            Ok(b) => b,
            // Receivers sold with one device may not report the bitmap.
            Err(_) if rx.default_child.is_some() => 0b10,
            Err(e) => {
                log::debug!("{}: reading paired devices: {e}", rx.name);
                0
            }
        };
        for slot in 1..=7u8 {
            if bits & (1 << slot) == 0 {
                continue;
            }
            let path = slot_path(d.path(), slot);
            // A device already held keeps its id; don't query it again
            // (a sleeping one would stall the scan).
            if held(&path) {
                if let Some(slug) = known_slug(&path) {
                    ids.next(slug);
                    continue;
                }
            }
            let pid = read_child_pid(&bus, slot).or(rx.default_child);
            let Some(model) = pid.and_then(model_for_pid) else {
                log::info!("{}: unsupported device {:04x?} in slot {slot}", rx.name, pid);
                continue;
            };
            let id = ids.next(model.slug);
            remember_slug(&path, model.slug);
            if held(&path) {
                continue;
            }
            match BragiDevice::open(bus.clone(), slot, model, id.clone(), None) {
                Ok(dev) => {
                    log::info!("opened {} ({id}) via {} slot {slot} (Bragi, experimental)", model.name, rx.name);
                    out.push((path, Box::new(dev)));
                }
                Err(e) => log::info!("{} via {}: not answering ({e})", model.name, rx.name),
            }
        }
    }
}

fn slug_cache() -> &'static std::sync::Mutex<Vec<(CString, &'static str)>> {
    static C: std::sync::OnceLock<std::sync::Mutex<Vec<(CString, &'static str)>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// Model slug of the device last seen at a receiver slot path.
fn known_slug(path: &CStr) -> Option<&'static str> {
    let c = slug_cache().lock().unwrap_or_else(|e| e.into_inner());
    c.iter().find(|(p, _)| p.as_c_str() == path).map(|(_, s)| *s)
}

fn remember_slug(path: &CStr, slug: &'static str) {
    let mut c = slug_cache().lock().unwrap_or_else(|e| e.into_inner());
    c.retain(|(p, _)| p.as_c_str() != path);
    c.push((path.to_owned(), slug));
}

/// PID of the device in `slot`; a device that is just waking up answers 0
/// or an error for a moment (OpenRGB retries for about a second).
fn read_child_pid(bus: &Bus, slot: u8) -> Option<u16> {
    for _ in 0..4 {
        if let Ok(r) = bus.command(&proto::get(bus.size, slot, proto::PROP_PID)) {
            let pid = r.value as u16;
            if pid != 0 && pid != 0xffff {
                return Some(pid);
            }
        }
        pause(120);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_paths_are_distinct() {
        let base = CString::new("\\\\?\\hid#vid_1b1c&pid_1ba6&mi_01").unwrap();
        let a = slot_path(&base, 1);
        let b = slot_path(&base, 2);
        assert_ne!(a, b);
        assert!(a.to_bytes().starts_with(base.to_bytes()));
    }

    #[test]
    fn sizes() {
        assert_eq!(size_candidates(64), vec![64, 1024, 128]);
        assert_eq!(size_candidates(1024), vec![1024, 128, 64]);
    }

    #[test]
    fn model_tables_are_consistent() {
        let all = models::MICE.iter().chain(models::HEADSETS.iter()).chain(keyboards::KEYBOARDS.iter());
        let mut pids = std::collections::HashSet::new();
        for m in all {
            assert!(pids.insert(m.pid), "duplicate pid {:04x}", m.pid);
            assert!(m.leds.is_empty() || m.leds.len() <= m.led_count, "{}", m.name);
            // Buttons are contiguous from bit 0 (the key-assignment table
            // has one byte per button in bit order).
            for (i, b) in m.buttons.iter().enumerate() {
                assert_eq!(b.bit as usize, i, "{}", m.name);
            }
            if let Some(d) = m.dpi {
                assert!(d.min < d.max);
            }
        }
        for r in models::RECEIVERS {
            assert!(model_for_pid(r.pid).is_none(), "receiver pid {:04x} is also a device", r.pid);
            if let Some(c) = r.default_child {
                assert!(model_for_pid(c).is_some(), "default child {c:04x}");
            }
        }
        // No overlap with the hardware-verified K55 RGB (NXP) and PSUs.
        for nxp in crate::devices::nxp::MODELS {
            assert!(model_for_pid(nxp.pid).is_none());
        }
        assert!(model_for_pid(0x1c05).is_none());
    }

    #[test]
    fn mouse_assignment_keeps_os_buttons_native() {
        let m = model_for_pid(0x1b9e).unwrap(); // M65 RGB ULTRA
        assert_eq!(models::mouse_assignment_table(m.buttons, false).unwrap(), vec![1, 1, 1, 1, 1, 0, 0, 0]);
        assert!(models::mouse_assignment_table(m.buttons, true).unwrap().iter().all(|&b| b == 1));
        assert!(models::mouse_assignment_table(&[], false).is_none());
    }

    #[test]
    fn special_key_ids_match_service_hooks() {
        // The service reacts to these ids (brightness cycling, Win-lock
        // refresh) using the NXP constants.
        assert_eq!(keyboards::KEY_BRIGHTNESS, crate::devices::nxp::KEY_BRIGHTNESS);
        assert_eq!(keyboards::KEY_WINLOCK, crate::devices::nxp::KEY_WINLOCK);
    }
}
