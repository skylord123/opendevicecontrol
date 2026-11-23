//! The motherboard's Mystic Light controller (see [`super::proto`]).
//!
//! OpenDeviceControl lighting streams colours, per LED on boards with the
//! per-LED mode, otherwise one colour per zone (and per on-board LED). With
//! it off, each zone runs its built-in effect. Built-in effects set here
//! apply at once; they are saved to the board's flash only when
//! OpenDeviceControl lets go of it, and only if they were changed.

use super::proto::{self, Board, Colour, Direct, Effect, Variant, Zone, MAX_BRIGHTNESS, STATIC, ZONES};
use crate::devices::{Device, HW_LIGHTING_PREFIX, SOFTWARE_LIGHTING};
use crate::model::*;
use crate::{Error, Result};
use std::time::{Duration, Instant};

/// How long to wait for a firmware version reply.
const TIMEOUT_MS: i32 = 300;
/// Gap OpenRGB leaves between the reports of one unsynchronised frame.
const REPORT_GAP: Duration = Duration::from_millis(13);
/// Resend the last per-LED frame this often while nothing changes.
const REFRESH: Duration = Duration::from_secs(1);
const SPEEDS: [&str; 3] = ["Slow", "Medium", "Fast"];
const OFF: &str = "Off";

pub struct MysticLight {
    info: DeviceInfo,
    dev: hidapi::HidDevice,
    board: Board,
    /// The 0x52 report as read, with the effects changed here.
    report: Vec<u8>,
    /// Built-in effect of each zone, indexed like [`ZONES`].
    effects: [Effect; ZONES.len()],
    /// One effect for every zone.
    sync: bool,
    /// JCORSAIR LEDs (not stored on the board).
    corsair_leds: u8,
    /// Built-in effects changed since opening (saved on release).
    changed: bool,
    software: bool,
    /// Per-LED mode switched on: whether synchronised.
    per_led: Option<bool>,
    frame: Vec<Rgb>,
    last_sent: Option<Instant>,
}

fn zi(z: Zone) -> usize {
    ZONES.iter().position(|&x| x == z).unwrap_or(0)
}

impl MysticLight {
    /// `report`: the 0x52 report just read, exactly the variant's length.
    pub fn open(dev: hidapi::HidDevice, id: String, path_info: &hidapi::DeviceInfo, variant: Variant, report: &[u8], pid: u16) -> Result<Self> {
        if report.len() != variant.report_len() || report[0] != proto::REPORT_EFFECTS {
            return Err(Error::Other(format!("MSI Mystic Light: unexpected {}-byte effects report", report.len())));
        }
        let board = proto::board(variant, pid);
        let mut m = MysticLight {
            info: DeviceInfo {
                id,
                kind: DeviceKind::Other,
                manufacturer: "MSI".into(),
                model: match board.name {
                    Some(n) => format!("{n} Mystic Light"),
                    None => format!("Mystic Light (MS-{pid:04X})"),
                },
                vendor_string: path_info.manufacturer_string().map(str::to_owned),
                product_string: path_info.product_string().map(str::to_owned),
                usb_vid: path_info.vendor_id(),
                usb_pid: path_info.product_id(),
                serial: path_info.serial_number().map(str::to_owned).filter(|s| !s.trim().is_empty()),
                firmware: None,
            },
            dev,
            board,
            report: report.to_vec(),
            effects: [Effect::default(); ZONES.len()],
            sync: false,
            corsair_leds: 0,
            changed: false,
            software: false,
            per_led: None,
            frame: Vec::new(),
            last_sent: None,
        };
        let aprom = m.version(proto::VERSION_APROM);
        let ldrom = m.version(proto::VERSION_LDROM);
        m.info.firmware = aprom.map(|a| match ldrom {
            Some(l) => format!("{} (loader {})", proto::version_string(a), proto::version_string(l)),
            None => proto::version_string(a),
        });
        if board.min_fw > 0 && aprom.is_none_or(|v| v < board.min_fw) {
            return Err(Error::Other(format!(
                "MSI Mystic Light: firmware {} is older than this board needs (OpenRGB's minimum)",
                m.info.firmware.as_deref().unwrap_or("unknown")
            )));
        }
        for z in m.zones() {
            let o = board.offset(z).expect("zones of the board have records");
            let mut e = proto::parse_effect(&m.report[o..]).unwrap_or_default();
            // Disabled shows as Off; a per-LED state left behind by a
            // program, or an unknown byte, as static.
            if e.mode == 0 {
                e = Effect { mode: STATIC, brightness: 0, ..e };
            } else if proto::mode(variant, e.mode).is_none() {
                e.mode = STATIC;
            }
            m.effects[zi(z)] = e;
        }
        let present = m.zones();
        m.sync = present.len() > 1 && present.iter().all(|&z| m.effects[zi(z)] == m.effects[zi(present[0])]);
        log::debug!("{}: {}-byte protocol, board {:#06x}, zones {:?}, direct {:?}", m.info.id, variant.report_len(), pid, board.zones, board.direct);
        Ok(m)
    }

    /// Firmware version byte (`01 sub` request on the output report).
    fn version(&self, sub: u8) -> Option<u8> {
        let mut buf = [0u8; 64];
        while self.dev.read_timeout(&mut buf, 0).is_ok_and(|n| n > 0) {}
        self.dev.write(&proto::version_request(sub)).ok()?;
        let deadline = Instant::now() + Duration::from_millis(TIMEOUT_MS as u64);
        while Instant::now() < deadline {
            let n = self.dev.read_timeout(&mut buf, TIMEOUT_MS).ok()?;
            if n == 0 {
                break;
            }
            if let Some(v) = proto::parse_version(&buf[..n], sub) {
                return Some(v);
            }
        }
        None
    }

    /// The zones the board has, in display order.
    fn zones(&self) -> Vec<Zone> {
        ZONES.into_iter().filter(|&z| self.board.has(z)).collect()
    }

    fn send(&self, report: &[u8]) -> Result<()> {
        self.dev.send_feature_report(report)?;
        Ok(())
    }

    /// The board's current 0x52 report, if it reads back whole.
    fn read_report(&self) -> Option<Vec<u8>> {
        let mut buf = [0u8; 200];
        buf[0] = proto::REPORT_EFFECTS;
        let n = self.dev.get_feature_report(&mut buf).ok()?;
        (Variant::from_len(n) == Some(self.board.variant) && buf[0] == proto::REPORT_EFFECTS).then(|| buf[..self.board.variant.report_len()].to_vec())
    }

    /// Apply the built-in effects (and leave per-LED mode). 185-byte boards
    /// get their current report first, then the new one, as MSI's software
    /// sends them.
    fn send_effects(&self, save: bool) -> Result<()> {
        if self.board.variant == Variant::V185 {
            if let Some(cur) = self.read_report() {
                self.send(&proto::effects_report(&self.board, &cur, false))?;
            }
        }
        self.send(&proto::effects_report(&self.board, &self.report, save))
    }

    /// LEDs a header drives: the board's count, or ours for JCORSAIR.
    fn header_leds(&self, z: Zone) -> usize {
        match z {
            Zone::Jcorsair => self.corsair_leds as usize,
            _ => self.board.led_count(&self.report, z).unwrap_or(0) as usize,
        }
    }

    /// Zones lit by OpenDeviceControl lighting, with their LEDs.
    fn direct_zones(&self) -> Vec<(Zone, usize)> {
        let zones = self.zones().into_iter();
        let v: Vec<(Zone, usize)> = match self.board.direct {
            Direct::None => Vec::new(),
            Direct::Zone => zones.map(|z| (z, if z == Zone::Onboard { self.board.onboard_led_count() } else { 1 })).collect(),
            Direct::PerLed => zones.map(|z| (z, self.board.per_led_leds(z, self.header_leds(z)))).collect(),
        };
        v.into_iter().filter(|&(_, n)| n > 0).collect()
    }

    fn per_led_sync(&self) -> bool {
        let headers: Vec<(Zone, usize)> = self.zones().into_iter().filter(|z| z.addressable()).map(|z| (z, self.header_leds(z))).collect();
        self.board.per_led_sync(&headers)
    }

    /// Switch per-LED mode on (again, when the report layout changes).
    fn enter_per_led(&mut self) -> Result<()> {
        let sync = self.per_led_sync();
        if self.per_led != Some(sync) {
            self.send(&proto::per_led_enable(&self.board, sync))?;
            self.per_led = Some(sync);
        }
        Ok(())
    }

    fn send_frame(&mut self) -> Result<()> {
        let zones = self.direct_zones();
        let mut slices: Vec<(Zone, &[Rgb])> = Vec::new();
        let mut at = 0;
        for &(z, n) in &zones {
            let end = (at + n).min(self.frame.len());
            slices.push((z, &self.frame[at.min(end)..end]));
            at = end;
        }
        match self.board.direct {
            Direct::None => return Ok(()),
            Direct::Zone => self.send(&proto::zone_frame(&self.board, &self.report, &slices))?,
            Direct::PerLed => {
                let sync = self.per_led.unwrap_or(true);
                for (i, r) in proto::per_led_frame(&self.board, &slices, sync).iter().enumerate() {
                    if i > 0 {
                        std::thread::sleep(REPORT_GAP);
                    }
                    self.send(r)?;
                }
            }
        }
        self.last_sent = Some(Instant::now());
        Ok(())
    }

    /// The effect controls of one zone, or of all of them (`z` None).
    fn effect_controls(&self, z: Option<Zone>, v: &mut Vec<Control>) {
        let first = self.zones().first().copied().unwrap_or(Zone::Onboard);
        let e = &self.effects[zi(z.unwrap_or(first))];
        let (zone, key) = match z {
            Some(z) => (z.name(), z.key()),
            None => ("All zones", "all"),
        };
        let id = |what: &str| format!("{HW_LIGHTING_PREFIX}{key}_{what}");
        let off = e.mode == STATIC && e.brightness == 0;
        let mode = proto::mode(self.board.variant, e.mode);
        let mut options = vec![OFF.to_string()];
        options.extend(proto::modes(self.board.variant).iter().map(|m| m.1.to_string()));
        v.push(Control {
            id: id("mode"),
            name: format!("{zone}: Effect"),
            kind: ControlKind::Choice { options },
            value: ControlValue::Choice(if off { OFF.into() } else { mode.map_or_else(|| format!("Other ({:#04x})", e.mode), |m| m.1.to_string()) }),
            help: None,
        });
        let Some(&(_, _, colour, speed)) = mode.filter(|_| !off) else { return };
        if colour == Colour::Optional {
            v.push(Control {
                id: id("cycle"),
                name: format!("{zone}: Cycle colours"),
                kind: ControlKind::Toggle,
                value: ControlValue::Bool(!e.custom),
                help: Some("The effect picks its own colours instead of the chosen one.".into()),
            });
        }
        if colour == Colour::Fixed || (colour == Colour::Optional && e.custom) {
            v.push(Control { id: id("color"), name: format!("{zone}: Colour"), kind: ControlKind::Color, value: ControlValue::Choice(e.color.to_hex()), help: None });
        }
        if speed {
            v.push(Control {
                id: id("speed"),
                name: format!("{zone}: Speed"),
                kind: ControlKind::Choice { options: SPEEDS.iter().map(|s| s.to_string()).collect() },
                value: ControlValue::Choice(SPEEDS[e.speed.min(2) as usize].into()),
                help: None,
            });
        }
        v.push(Control {
            id: id("brightness"),
            name: format!("{zone}: Brightness"),
            kind: ControlKind::Number { min: 0.0, max: 100.0, step: 10.0, unit: "%".into() },
            value: ControlValue::Number(e.brightness as f64 * 10.0),
            help: None,
        });
    }

    /// Zones with an LED count control.
    fn counted(&self) -> Vec<Zone> {
        self.zones()
            .into_iter()
            .filter(|&z| match z {
                Zone::Jcorsair => self.board.direct == Direct::PerLed,
                _ => self.board.led_count(&self.report, z).is_some(),
            })
            .collect()
    }

    fn controls(&self) -> Vec<Control> {
        let mut v = Vec::new();
        if self.board.direct != Direct::None {
            v.push(Control {
                id: SOFTWARE_LIGHTING.into(),
                name: "OpenDeviceControl lighting".into(),
                kind: ControlKind::Toggle,
                value: ControlValue::Bool(self.software),
                help: Some(match self.board.direct {
                    Direct::PerLed => "On: OpenDeviceControl's lighting effects drive every LED. Off: each zone runs its built-in effect, saved to the board when OpenDeviceControl closes.".into(),
                    _ => "On: OpenDeviceControl's lighting effects drive the board, one colour per zone (and per on-board LED). Off: each zone runs its built-in effect, saved to the board when OpenDeviceControl closes.".into(),
                }),
            });
        }
        let zones = self.zones();
        if zones.len() > 1 {
            v.push(Control {
                id: format!("{HW_LIGHTING_PREFIX}sync"),
                name: "Apply to all zones".into(),
                kind: ControlKind::Toggle,
                value: ControlValue::Bool(self.sync),
                help: Some("Every zone runs one effect. With it off, multi-zone effects on the on-board LEDs still lead the JRGB and light pipe zones (the firmware's sync).".into()),
            });
        }
        if self.sync {
            self.effect_controls(None, &mut v);
        } else {
            for &z in &zones {
                self.effect_controls(Some(z), &mut v);
            }
        }
        for z in self.counted() {
            v.push(Control {
                id: format!("leds_{}", z.key()),
                name: format!("{}: LEDs", z.name()),
                kind: ControlKind::Number { min: 0.0, max: proto::per_led_max(z, false) as f64, step: 1.0, unit: String::new() },
                value: ControlValue::Number(self.header_leds(z) as f64),
                help: Some(if z == Zone::Jcorsair {
                    "How many LEDs the strip or fans on this header have.".into()
                } else {
                    "How many LEDs the strip or fans on this header have (stored on the board).".into()
                }),
            });
        }
        v
    }

    /// Change the built-in effect of zone `z` (or of every zone) and apply
    /// it unless OpenDeviceControl lighting is on.
    fn update_effect(&mut self, z: Option<Zone>, f: impl Fn(&mut Effect)) -> Result<()> {
        let targets: Vec<Zone> = z.map_or_else(|| self.zones(), |z| vec![z]);
        for &t in &targets {
            let e = &mut self.effects[zi(t)];
            f(e);
            // Keep the colour flag to what the effect allows.
            match proto::mode(self.board.variant, e.mode).map(|m| m.2) {
                Some(Colour::Fixed) => e.custom = true,
                Some(Colour::None) => e.custom = false,
                _ => {}
            }
            proto::write_effect(&self.board, &mut self.report, t, e);
        }
        self.changed = true;
        if self.software {
            return Ok(());
        }
        self.send_effects(false)
    }

    fn set_sync(&mut self, on: bool) -> Result<()> {
        self.sync = on;
        if !on {
            return Ok(());
        }
        let first = *self.zones().first().ok_or(Error::NotSupported(": no zones"))?;
        let base = self.effects[zi(first)];
        self.update_effect(None, |e| *e = base)
    }
}

/// An approximation of a built-in effect on `leds` LEDs at `t` seconds,
/// for the preview (the firmware's animations are not known exactly).
fn preview(e: &Effect, leds: usize, t: f64) -> Vec<Rgb> {
    let period = [4.0, 2.5, 1.5][e.speed.min(2) as usize];
    let phase = (t / period).fract();
    let n = leds.max(1) as f64;
    let level = e.brightness as f32 / MAX_BRIGHTNESS as f32;
    let hue = |x: f64| Rgb::from_hsv((x.rem_euclid(1.0) * 360.0) as f32, 1.0, 1.0);
    let base = if e.custom { e.color } else { hue(phase) };
    let pulse = (0.5 - 0.5 * (phase * std::f64::consts::TAU).cos()) as f32;
    let gate = |on: bool, c: Rgb| if on { c } else { Rgb::BLACK };
    (0..leds)
        .map(|i| {
            let along = i as f64 / n;
            let c = match e.mode {
                0x01 => e.color,
                0x02 | 0x15 => base.scale(pulse),
                0x03 => gate(phase < 0.15, base),
                0x04 | 0x05 => gate(phase < 0.08 || (0.2..0.28).contains(&phase), base),
                0x13 => gate(phase < 0.5, base),
                0x1d | 0x1e | 0x23 => gate(phase < 0.15 || (e.mode != 0x1d && (0.25..0.4).contains(&phase)), hue(along)),
                0x07 | 0x08 | 0x11 | 0x18 | 0x1b | 0x24 => {
                    // A lit band running along the LEDs.
                    let c = if e.custom { e.color } else { hue(along) };
                    let d = (along - phase).rem_euclid(1.0);
                    if d < 0.25 { c } else { c.scale(0.08) }
                }
                0x17 if e.custom => e.color.scale((0.5 + 0.5 * ((along - phase) * std::f64::consts::TAU).cos()) as f32),
                _ => hue(along + phase),
            };
            c.scale(level)
        })
        .collect()
}

/// `#rrggbb`.
fn from_hex(s: &str) -> Option<Rgb> {
    let h = s.trim().strip_prefix('#').filter(|h| h.len() == 6)?;
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Rgb::new((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// `hw_<zone>_<what>` / `hw_all_<what>` -> (zone, or None for all; what).
fn hw_control(id: &str) -> Option<(Option<Zone>, &str)> {
    let (zone, what) = id.strip_prefix(HW_LIGHTING_PREFIX)?.split_once('_')?;
    if zone == "all" {
        return Some((None, what));
    }
    Some((Some(Zone::from_key(zone)?), what))
}

impl Device for MysticLight {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        if self.software && self.per_led.is_some() && !self.frame.is_empty() && self.last_sent.is_none_or(|t| t.elapsed() >= REFRESH) {
            self.send_frame()?;
        }
        Ok(Snapshot { sensors: Vec::new(), controls: self.controls() })
    }

    fn poll_interval(&self) -> Option<Duration> {
        Some(if self.software { REFRESH / 2 } else { Duration::from_secs(2) })
    }

    fn frame_interval(&self) -> Option<Duration> {
        Some(Duration::from_millis(match (self.board.direct, self.per_led) {
            (Direct::PerLed, Some(true)) => 40,
            _ => 100,
        }))
    }

    fn remembered_controls(&self) -> Vec<String> {
        self.counted().into_iter().filter(|&z| z == Zone::Jcorsair).map(|z| format!("leds_{}", z.key())).collect()
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        let bad = || Error::InvalidValue(id.to_string());
        if id == SOFTWARE_LIGHTING && self.board.direct != Direct::None {
            let ControlValue::Bool(on) = value else { return Err(bad()) };
            if *on == self.software {
                return Ok(());
            }
            self.software = *on;
            self.last_sent = None;
            return if *on {
                if self.board.direct == Direct::PerLed {
                    self.enter_per_led()?;
                }
                if self.frame.is_empty() {
                    Ok(())
                } else {
                    self.send_frame()
                }
            } else {
                self.per_led = None;
                self.send_effects(false)
            };
        }
        if id == format!("{HW_LIGHTING_PREFIX}sync") {
            let ControlValue::Bool(on) = value else { return Err(bad()) };
            return self.set_sync(*on);
        }
        if let Some((z, what)) = hw_control(id).filter(|(z, _)| z.is_none_or(|z| self.board.has(z))) {
            let v = self.board.variant;
            return match (what, value) {
                ("mode", ControlValue::Choice(name)) if name == OFF => self.update_effect(z, |e| {
                    e.mode = STATIC;
                    e.brightness = 0;
                }),
                ("mode", ControlValue::Choice(name)) => {
                    let m = proto::modes(v).iter().find(|m| m.1 == name).ok_or_else(bad)?.0;
                    self.update_effect(z, |e| {
                        e.mode = m;
                        if e.brightness == 0 {
                            e.brightness = MAX_BRIGHTNESS;
                        }
                    })
                }
                ("color", ControlValue::Choice(hex)) => {
                    let c = from_hex(hex).ok_or_else(bad)?;
                    self.update_effect(z, |e| e.color = c)
                }
                ("cycle", ControlValue::Bool(on)) => {
                    let on = *on;
                    self.update_effect(z, |e| e.custom = !on)
                }
                ("speed", ControlValue::Choice(s)) => {
                    let s = SPEEDS.iter().position(|x| x == s).ok_or_else(bad)? as u8;
                    self.update_effect(z, |e| e.speed = s)
                }
                ("brightness", ControlValue::Number(n)) => {
                    let b = (n.clamp(0.0, 100.0) / 10.0).round() as u8;
                    self.update_effect(z, |e| e.brightness = b)
                }
                _ => Err(bad()),
            };
        }
        if let Some(z) = id.strip_prefix("leds_").and_then(Zone::from_key).filter(|z| self.counted().contains(z)) {
            let ControlValue::Number(n) = value else { return Err(bad()) };
            let n = n.round().clamp(0.0, proto::per_led_max(z, false) as f64) as u8;
            if z == Zone::Jcorsair {
                self.corsair_leds = n;
            } else {
                self.board.set_led_count(&mut self.report, z, n);
                self.changed = true;
                if !self.software {
                    self.send_effects(false)?;
                }
            }
            if self.software && self.per_led.is_some() {
                self.enter_per_led()?;
            }
            return Ok(());
        }
        Err(Error::UnknownControl(id.to_string()))
    }

    fn preview_lights(&mut self, t: f64) -> Option<Vec<Rgb>> {
        if self.software || self.board.direct == Direct::None {
            return None;
        }
        Some(self.direct_zones().into_iter().flat_map(|(z, n)| preview(&self.effects[zi(z)], n, t)).collect())
    }

    fn light_zones(&self) -> Vec<LightZone> {
        self.direct_zones().into_iter().map(|(z, n)| LightZone { id: z.key().into(), name: z.name().into(), leds: n }).collect()
    }

    fn set_lights(&mut self, colors: &[Rgb]) -> Result<()> {
        if !self.software {
            return Ok(());
        }
        let total: usize = self.direct_zones().iter().map(|z| z.1).sum();
        let mut frame = colors.to_vec();
        frame.resize(total, Rgb::default());
        let fresh = self.last_sent.is_some_and(|t| self.per_led.is_none() || t.elapsed() < REFRESH);
        if frame == self.frame && fresh {
            return Ok(());
        }
        self.frame = frame;
        self.send_frame()
    }

    fn release(&mut self) -> Result<()> {
        self.frame.clear();
        let was_software = std::mem::take(&mut self.software);
        self.per_led = None;
        // Back to the built-in effects; store them only if changed here.
        if was_software || self.changed {
            self.send_effects(self.changed)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hw_control_ids() {
        assert_eq!(hw_control("hw_jrainbow1_mode"), Some((Some(Zone::Jrainbow1), "mode")));
        assert_eq!(hw_control("hw_all_brightness"), Some((None, "brightness")));
        assert_eq!(hw_control("hw_onboard_color"), Some((Some(Zone::Onboard), "color")));
        assert_eq!(hw_control("hw_jfoo_mode"), None);
        assert_eq!(from_hex("#ff8000"), Some(Rgb::new(255, 128, 0)));
    }

    #[test]
    fn preview_lengths_and_brightness() {
        let e = Effect { brightness: 0, ..Effect::default() };
        assert!(preview(&e, 4, 1.0).iter().all(|&c| c == Rgb::BLACK));
        let e = Effect { mode: 0x1a, custom: false, ..Effect::default() };
        assert_eq!(preview(&e, 7, 0.3).len(), 7);
    }
}
