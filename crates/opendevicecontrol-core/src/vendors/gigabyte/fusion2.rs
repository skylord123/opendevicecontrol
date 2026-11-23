//! The motherboard's RGB Fusion 2 controller (see [`super::proto`]).
//!
//! OpenDeviceControl lighting streams colours to the addressable headers and
//! holds the single-colour LEDs on static colours; with it off, each zone
//! runs its built-in effect. The controller can't report its effects, so
//! they show as unknown until set here; the front-end re-applies the ones
//! set here when the device opens again. Nothing is saved to the
//! controller's flash.

use super::proto::{self, Effect, Header, Kind, Layout};
use crate::devices::{Device, HW_LIGHTING_PREFIX, SOFTWARE_LIGHTING};
use crate::model::*;
use crate::{Error, Result};
use std::time::{Duration, Instant};

/// Least time between direct-colour frames.
const FRAME_GAP: Duration = Duration::from_millis(50);
/// Resend the last frame this often while nothing changes (the controller
/// resets itself on resume from sleep).
const REFRESH: Duration = Duration::from_secs(2);
/// Shown while a zone's effect isn't known.
const UNKNOWN: &str = "Unknown";

struct Zone {
    /// `d1`… for headers, `led1`… for single LEDs (control ids).
    key: String,
    name: String,
    /// LED index its built-in effect is set on.
    led: u8,
    header: Option<&'static Header>,
    /// Headers: LEDs on it, set by the user.
    leds: u16,
    order: proto::Order,
    effect: Option<Effect>,
}

impl Zone {
    /// LEDs OpenDeviceControl lighting drives.
    fn lit(&self) -> usize {
        if self.header.is_some() { self.leds as usize } else { 1 }
    }
}

pub struct Fusion2 {
    info: DeviceInfo,
    dev: hidapi::HidDevice,
    layout: Layout,
    lamp_array: bool,
    zones: Vec<Zone>,
    /// Header length classes the controller holds (`34`).
    classes: [u8; 6],
    sync: bool,
    software: bool,
    /// Beat and lamp-array modes have been turned off.
    ready: bool,
    /// The last `32` mask sent.
    direct_mask: Option<u8>,
    frame: Vec<Rgb>,
    last_sent: Option<Instant>,
}

/// Send info command `cmd` and read its reply.
fn read_report(dev: &hidapi::HidDevice, cmd: u8) -> Result<Vec<u8>> {
    dev.send_feature_report(&proto::command(cmd, &[]))?;
    let mut buf = [0u8; proto::LEN];
    buf[0] = proto::REPORT_ID;
    let n = dev.get_feature_report(&mut buf)?;
    Ok(buf[..n].to_vec())
}

fn bad_reply(cmd: u8, r: &[u8]) -> Error {
    Error::Other(format!("RGB Fusion 2 controller sent an unexpected reply to {cmd:#04x} ({} bytes)", r.len()))
}

impl Fusion2 {
    pub fn open(dev: hidapi::HidDevice, id: String, hid: &hidapi::DeviceInfo) -> Result<Self> {
        let pid = hid.product_id();
        let r = read_report(&dev, proto::CMD_INFO)?;
        let mut report = proto::parse_info(&r).ok_or_else(|| bad_reply(proto::CMD_INFO, &r))?;
        if pid == 0x5711 && report.device_num == 0 {
            let r = read_report(&dev, proto::CMD_INFO2)?;
            match proto::parse_info2(&r) {
                Some(cal) => report.cal[2..].copy_from_slice(&cal),
                None => log::warn!("{id}: no colour order for headers 3 and 4 ({} byte reply)", r.len()),
            }
        }
        let layout = proto::layout(pid, report.device_num);
        let mut zones = Vec::new();
        for h in layout.headers {
            let order = proto::decode_order(report.cal[h.slot]).unwrap_or_else(|| {
                log::warn!("{id}: {} has no valid colour order ({:#010x}), using RGB", h.name, report.cal[h.slot]);
                proto::DEFAULT_ORDER
            });
            zones.push(Zone { key: format!("d{}", h.slot + 1), name: h.name.into(), led: h.led, header: Some(h), leds: 0, order, effect: None });
        }
        for &led in layout.singles {
            let n = led + 1;
            zones.push(Zone { key: format!("led{n}"), name: format!("LED {n}"), led, header: None, leds: 0, order: proto::DEFAULT_ORDER, effect: None });
        }
        log::debug!(
            "{id}: {} (product {}, controller {}, flags {:#04x}), length classes {:?}, colour orders {:?}",
            report.name,
            report.product,
            report.device_num,
            report.flags,
            report.counts,
            zones.iter().filter(|z| z.header.is_some()).map(|z| proto::order_name(z.order)).collect::<Vec<_>>()
        );
        let hid_serial = hid.serial_number().map(str::to_owned).filter(|s| !s.trim().is_empty());
        Ok(Fusion2 {
            info: DeviceInfo {
                id,
                kind: DeviceKind::Other,
                manufacturer: "Gigabyte".into(),
                model: "RGB Fusion 2".into(),
                vendor_string: hid.manufacturer_string().map(str::to_owned),
                product_string: Some(report.name.clone()).filter(|s| !s.is_empty()).or_else(|| hid.product_string().map(str::to_owned)),
                usb_vid: proto::VID,
                usb_pid: pid,
                serial: hid_serial.or_else(|| (report.chip_id != 0).then(|| format!("{:08X}", report.chip_id))),
                firmware: Some(report.firmware_string()),
            },
            dev,
            layout,
            lamp_array: report.flags & proto::FLAG_LAMP_ARRAY != 0,
            zones,
            classes: report.counts,
            sync: false,
            software: false,
            ready: false,
            direct_mask: None,
            frame: Vec::new(),
            last_sent: None,
        })
    }

    fn send(&self, report: &[u8]) -> Result<()> {
        self.dev.send_feature_report(report)?;
        Ok(())
    }

    /// Before the first change: turn off audio beat mode and Dynamic
    /// Lighting, either of which would override the effects set here.
    fn ensure_ready(&mut self) -> Result<()> {
        if !self.ready {
            if self.lamp_array {
                self.send(&proto::command(proto::CMD_LAMP_ARRAY, &[0]))?;
            }
            self.send(&proto::command(proto::CMD_BEAT, &[0]))?;
            self.ready = true;
        }
        Ok(())
    }

    fn header_mask(&self) -> u8 {
        self.layout.headers.iter().fold(0, |m, h| m | h.bit)
    }

    /// `32`: which headers take direct colours instead of their effect.
    fn set_direct_mask(&mut self, mask: u8) -> Result<()> {
        if self.direct_mask != Some(mask) {
            self.send(&proto::command(proto::CMD_DIRECT_MASK, &[mask]))?;
            self.direct_mask = Some(mask);
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    /// Send the known effects of `targets` and apply them. Attempts every
    /// zone and returns the first error.
    fn apply_effects(&self, targets: &[usize]) -> Result<()> {
        let mut first = Ok(());
        let mut mask = 0u32;
        for &i in targets {
            let z = &self.zones[i];
            if let Some(e) = &z.effect {
                first = first.and(self.send(&proto::effect(z.led, e)));
                mask |= 1 << z.led;
            }
        }
        if mask != 0 {
            first = first.and(self.send(&proto::apply(mask)));
        }
        first
    }

    /// Every header back to its built-in effect, every known effect re-sent.
    fn leave_direct(&mut self) -> Result<()> {
        let unmasked = self.set_direct_mask(0);
        let all: Vec<usize> = (0..self.zones.len()).collect();
        unmasked.and(self.apply_effects(&all))
    }

    fn lit(&self) -> impl Iterator<Item = &Zone> + '_ {
        self.zones.iter().filter(|z| z.lit() > 0)
    }

    /// Send the zones of `frame` that changed (all with `force`).
    fn send_frame(&mut self, frame: Vec<Rgb>, force: bool) -> Result<()> {
        let mut at = 0;
        let mut mask = 0u32;
        for z in self.lit() {
            let n = z.lit();
            let new = &frame[at..at + n];
            if force || self.frame.get(at..at + n) != Some(new) {
                match z.header {
                    Some(h) => {
                        for r in proto::direct(h.direct, z.order, new) {
                            self.send(&r)?;
                        }
                    }
                    None => {
                        self.send(&proto::effect(z.led, &Effect::fixed(new[0])))?;
                        mask |= 1 << z.led;
                    }
                }
            }
            at += n;
        }
        if mask != 0 {
            self.send(&proto::apply(mask))?;
        }
        self.frame = frame;
        self.last_sent = Some(Instant::now());
        Ok(())
    }

    fn zone_index(&self, key: &str) -> Option<usize> {
        self.zones.iter().position(|z| z.key == key)
    }

    /// The effect controls of one zone, or of all of them (`zone` None)
    /// while they are synced.
    fn effect_controls(&self, zone: Option<usize>, v: &mut Vec<Control>) {
        let z = &self.zones[zone.unwrap_or(0)];
        let (name, key) = match zone {
            Some(_) => (z.name.as_str(), z.key.as_str()),
            None => ("All zones", "all"),
        };
        let id = |what: &str| format!("{HW_LIGHTING_PREFIX}{key}_{what}");
        v.push(Control {
            id: id("mode"),
            name: format!("{name}: Effect"),
            kind: ControlKind::Choice { options: self.layout.modes.iter().map(|k| k.name().to_string()).collect() },
            value: ControlValue::Choice(z.effect.map_or(UNKNOWN, |e| e.kind.name()).to_string()),
            help: z.effect.is_none().then(|| "The controller can't report its current effect; pick one to set it.".into()),
        });
        let Some(e) = z.effect else { return };
        if e.kind.uses_color() {
            v.push(Control { id: id("color"), name: format!("{name}: Colour"), kind: ControlKind::Color, value: ControlValue::Choice(e.color.to_hex()), help: None });
        }
        if e.kind.uses_speed() {
            v.push(Control {
                id: id("speed"),
                name: format!("{name}: Speed"),
                kind: ControlKind::Number { min: 0.0, max: 100.0, step: 100.0 / proto::SPEED_STEPS as f64, unit: "%".into() },
                value: ControlValue::Number(proto::speed_percent(e.speed)),
                help: None,
            });
        }
        if e.kind != Kind::Off {
            v.push(Control {
                id: id("brightness"),
                name: format!("{name}: Brightness"),
                kind: ControlKind::Number { min: 0.0, max: 100.0, step: 1.0, unit: "%".into() },
                value: ControlValue::Number(e.brightness as f64),
                help: None,
            });
        }
    }

    fn controls(&self) -> Vec<Control> {
        let mut v = vec![Control {
            id: SOFTWARE_LIGHTING.into(),
            name: "OpenDeviceControl lighting".into(),
            kind: ControlKind::Toggle,
            value: ControlValue::Bool(self.software),
            help: Some("On: OpenDeviceControl's lighting effects drive the board's LEDs. Off: each zone runs its built-in effect.".into()),
        }];
        if self.zones.is_empty() {
            return v;
        }
        v.push(Control {
            id: format!("{HW_LIGHTING_PREFIX}sync"),
            name: "Apply to all zones".into(),
            kind: ControlKind::Toggle,
            value: ControlValue::Bool(self.sync),
            help: Some("Every zone runs one effect.".into()),
        });
        if self.sync {
            self.effect_controls(None, &mut v);
        } else {
            for i in 0..self.zones.len() {
                self.effect_controls(Some(i), &mut v);
            }
        }
        for z in self.zones.iter().filter(|z| z.header.is_some()) {
            v.push(Control {
                id: format!("leds_{}", z.key),
                name: format!("{}: LEDs", z.name),
                kind: ControlKind::Number { min: 0.0, max: self.layout.max_leds as f64, step: 1.0, unit: String::new() },
                value: ControlValue::Number(z.leds as f64),
                help: Some("How many LEDs the strip or fans on this header have.".into()),
            });
        }
        v
    }

    /// Change the built-in effect of `targets` and apply it unless
    /// OpenDeviceControl lighting is on.
    fn update_effect(&mut self, targets: Vec<usize>, f: impl Fn(&mut Option<Effect>)) -> Result<()> {
        for &i in &targets {
            f(&mut self.zones[i].effect);
        }
        if self.software {
            return Ok(());
        }
        self.ensure_ready()?;
        self.apply_effects(&targets)
    }

    /// Give every zone the first zone's effect, or let them go.
    fn set_sync(&mut self, on: bool) -> Result<()> {
        self.sync = on;
        if !on || self.zones.is_empty() {
            return Ok(());
        }
        let base = self.zones[0].effect;
        self.update_effect((0..self.zones.len()).collect(), |e| *e = base)
    }

    fn set_hw(&mut self, key: &str, what: &str, value: &ControlValue, bad: impl Fn() -> Error) -> Result<()> {
        let targets: Vec<usize> = match key {
            // Only shown while synced; a stale remembered value is ignored.
            "all" if !self.sync => return Ok(()),
            "all" => (0..self.zones.len()).collect(),
            k => vec![self.zone_index(k).ok_or_else(&bad)?],
        };
        match (what, value) {
            ("mode", ControlValue::Choice(name)) if name == UNKNOWN => Ok(()),
            ("mode", ControlValue::Choice(name)) => {
                let kind = *self.layout.modes.iter().find(|k| k.name() == name).ok_or_else(bad)?;
                self.update_effect(targets, |e| e.get_or_insert_with(Effect::default).kind = kind)
            }
            ("color", ControlValue::Choice(hex)) => {
                let c = from_hex(hex).ok_or_else(bad)?;
                self.update_effect(targets, |e| if let Some(e) = e { e.color = c })
            }
            ("speed", ControlValue::Number(n)) => {
                let s = proto::speed_step(*n);
                self.update_effect(targets, |e| if let Some(e) = e { e.speed = s })
            }
            ("brightness", ControlValue::Number(n)) => {
                let b = n.round().clamp(0.0, 100.0) as u8;
                self.update_effect(targets, |e| if let Some(e) = e { e.brightness = b })
            }
            _ => Err(bad()),
        }
    }

    fn set_leds(&mut self, i: usize, n: f64) -> Result<()> {
        let Some(h) = self.zones[i].header else { return Err(Error::UnknownControl(format!("leds_{}", self.zones[i].key))) };
        let n = n.round().clamp(0.0, self.layout.max_leds as f64) as u16;
        let class = proto::count_class(n);
        if self.classes[h.slot] != class {
            let mut classes = self.classes;
            classes[h.slot] = class;
            self.send(&proto::led_counts(&classes))?;
            self.classes = classes;
        }
        self.zones[i].leds = n;
        // The zones moved: send the next frame in full.
        self.frame.clear();
        Ok(())
    }
}

/// An approximation of what a built-in effect shows on `leds` LEDs at `t`
/// seconds, for the preview (the firmware's exact animations are unknown).
fn preview(e: Option<&Effect>, leds: usize, t: f64) -> Vec<Rgb> {
    let Some(e) = e else { return vec![Rgb::BLACK; leds] };
    let (p, _) = e.timing();
    let period = (p.iter().map(|&x| x as f64).sum::<f64>() / 1000.0).max(0.2);
    let x = t.rem_euclid(period);
    let c = e.color.scale(e.brightness as f32 / 100.0);
    let v = e.brightness as f32 / 100.0;
    (0..leds)
        .map(|i| match e.kind {
            Kind::Off => Rgb::BLACK,
            Kind::Static => c,
            Kind::Breathing => c.scale((0.5 - 0.5 * (x / period * std::f64::consts::TAU).cos()) as f32),
            Kind::Flash => if x < 0.2 { c } else { Rgb::BLACK },
            Kind::DoubleFlash => if x < 0.1 || (0.2..0.3).contains(&x) { c } else { Rgb::BLACK },
            // Seven colours, one fade each.
            Kind::ColorCycle => Rgb::from_hsv((t / (period * 7.0)).fract() as f32 * 360.0, 1.0, v),
            Kind::Random => {
                let k = (t * 2.0).floor() as u64 * 7919 + i as u64 * 104729;
                Rgb::from_hsv((k % 360) as f32, 1.0, v)
            }
        })
        .collect()
}

/// `#rrggbb`.
fn from_hex(s: &str) -> Option<Rgb> {
    let h = s.trim().strip_prefix('#').filter(|h| h.len() == 6)?;
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Rgb::new((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// `hw_<zone>_<what>` -> (zone key or `all`, what).
fn hw_control(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix(HW_LIGHTING_PREFIX)?.split_once('_')
}

impl Device for Fusion2 {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        if self.software && !self.frame.is_empty() && self.last_sent.is_none_or(|t| t.elapsed() >= REFRESH) {
            self.direct_mask = None;
            self.set_direct_mask(self.header_mask())?;
            let frame = std::mem::take(&mut self.frame);
            self.send_frame(frame, true)?;
        }
        Ok(Snapshot { sensors: Vec::new(), controls: self.controls() })
    }

    fn poll_interval(&self) -> Option<Duration> {
        Some(if self.software { REFRESH / 2 } else { Duration::from_secs(2) })
    }

    fn frame_interval(&self) -> Option<Duration> {
        Some(FRAME_GAP)
    }

    fn remembered_controls(&self) -> Vec<String> {
        // Restored in this order: LED counts, each zone's effect, then the
        // synced effect (the controller forgets effects when powered off and
        // can't report them).
        let what = ["mode", "color", "speed", "brightness"];
        let mut v: Vec<String> = self.zones.iter().filter(|z| z.header.is_some()).map(|z| format!("leds_{}", z.key)).collect();
        for key in self.zones.iter().map(|z| z.key.as_str()) {
            v.extend(what.iter().map(|w| format!("{HW_LIGHTING_PREFIX}{key}_{w}")));
        }
        v.push(format!("{HW_LIGHTING_PREFIX}sync"));
        v.extend(what.iter().map(|w| format!("{HW_LIGHTING_PREFIX}all_{w}")));
        v
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        let bad = || Error::InvalidValue(id.to_string());
        if id == SOFTWARE_LIGHTING {
            let ControlValue::Bool(on) = value else { return Err(bad()) };
            if *on == self.software {
                return Ok(());
            }
            self.software = *on;
            self.last_sent = None;
            if !*on {
                return self.leave_direct();
            }
            self.ensure_ready()?;
            self.set_direct_mask(self.header_mask())?;
            if self.frame.is_empty() {
                return Ok(());
            }
            let frame = std::mem::take(&mut self.frame);
            return self.send_frame(frame, true);
        }
        if id == format!("{HW_LIGHTING_PREFIX}sync") {
            let ControlValue::Bool(on) = value else { return Err(bad()) };
            return self.set_sync(*on);
        }
        if let Some((key, what)) = hw_control(id) {
            return self.set_hw(key, what, value, bad);
        }
        if let Some(i) = id.strip_prefix("leds_").and_then(|k| self.zone_index(k)) {
            let ControlValue::Number(n) = value else { return Err(bad()) };
            return self.set_leds(i, *n);
        }
        Err(Error::UnknownControl(id.to_string()))
    }

    fn preview_lights(&mut self, t: f64) -> Option<Vec<Rgb>> {
        if self.software {
            return None;
        }
        Some(self.lit().flat_map(|z| preview(z.effect.as_ref(), z.lit(), t)).collect())
    }

    fn light_zones(&self) -> Vec<LightZone> {
        self.lit().map(|z| LightZone { id: z.key.clone(), name: z.name.clone(), leds: z.lit() }).collect()
    }

    fn set_lights(&mut self, colors: &[Rgb]) -> Result<()> {
        if !self.software {
            return Ok(());
        }
        let total: usize = self.lit().map(Zone::lit).sum();
        let mut frame = colors.to_vec();
        frame.resize(total, Rgb::default());
        if frame == self.frame && self.last_sent.is_some_and(|t| t.elapsed() < REFRESH) {
            return Ok(());
        }
        self.send_frame(frame, false)
    }

    fn release(&mut self) -> Result<()> {
        self.frame.clear();
        // Untouched since opening: leave the board as it was.
        if !self.software && self.direct_mask.is_none_or(|m| m == 0) {
            return Ok(());
        }
        self.software = false;
        self.leave_direct()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hw_control_ids() {
        assert_eq!(hw_control("hw_d1_mode"), Some(("d1", "mode")));
        assert_eq!(hw_control("hw_led8_brightness"), Some(("led8", "brightness")));
        assert_eq!(hw_control("hw_all_color"), Some(("all", "color")));
        assert_eq!(hw_control("leds_d1"), None);
        assert_eq!(from_hex("#ff8000"), Some(Rgb::new(255, 128, 0)));
        assert_eq!(from_hex("ff8000"), None);
    }

    #[test]
    fn preview_lengths() {
        assert_eq!(preview(None, 3, 1.0), vec![Rgb::BLACK; 3]);
        let e = Effect { kind: Kind::Static, color: Rgb::new(200, 0, 0), speed: 0, brightness: 50 };
        assert_eq!(preview(Some(&e), 2, 0.0), vec![Rgb::new(100, 0, 0); 2]);
        for kind in [Kind::Off, Kind::Breathing, Kind::Flash, Kind::DoubleFlash, Kind::ColorCycle, Kind::Random] {
            assert_eq!(preview(Some(&Effect { kind, ..e }), 5, 3.3).len(), 5);
        }
    }
}
