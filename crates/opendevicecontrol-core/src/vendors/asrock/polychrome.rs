//! The motherboard's Polychrome RGB controller (see [`super::proto`]).
//!
//! OpenDeviceControl lighting streams colours to every LED of every channel;
//! with it off, each channel runs its built-in effect. Built-in effects set
//! here apply at once and are saved to the board when OpenDeviceControl lets
//! go of it (like ASRock's utility, which saves on exit).

use super::proto::{self, Effect, CHANNELS, MODES};
use crate::devices::{Device, HW_LIGHTING_PREFIX, SOFTWARE_LIGHTING};
use crate::model::*;
use crate::{Error, Result};
use std::time::{Duration, Instant};

/// How long to wait for a reply (ASRock's utility uses 500 ms).
const TIMEOUT_MS: i32 = 500;
/// Least time between direct-colour frames (the utility streams every 50 ms).
const FRAME_GAP: Duration = Duration::from_millis(50);
/// Resend the last frame this often while nothing changes.
const REFRESH: Duration = Duration::from_secs(1);

pub struct Polychrome {
    info: DeviceInfo,
    dev: hidapi::HidDevice,
    /// Channels the board has.
    active: u8,
    counts: [u8; CHANNELS],
    rg_swap: u8,
    effects: [Effect; CHANNELS],
    /// Built-in effects changed since opening (saved on release).
    changed: bool,
    software: bool,
    frame: Vec<Rgb>,
    last_sent: Option<Instant>,
}

impl Polychrome {
    pub fn open(dev: hidapi::HidDevice, id: String, path_info: &hidapi::DeviceInfo) -> Result<Self> {
        let mut p = Polychrome {
            info: DeviceInfo {
                id,
                kind: DeviceKind::Other,
                manufacturer: "ASRock".into(),
                model: "Polychrome RGB".into(),
                vendor_string: path_info.manufacturer_string().map(str::to_owned),
                product_string: path_info.product_string().map(str::to_owned),
                usb_vid: proto::VID,
                usb_pid: proto::PID,
                serial: path_info.serial_number().map(str::to_owned).filter(|s| !s.trim().is_empty()),
                firmware: None,
            },
            dev,
            active: 0,
            counts: [0; CHANNELS],
            rg_swap: 0,
            effects: Default::default(),
            changed: false,
            software: false,
            frame: Vec::new(),
            last_sent: None,
        };
        p.read_state()?;
        Ok(p)
    }

    fn send(&self, report: &[u8]) -> Result<()> {
        self.dev.write(report)?;
        Ok(())
    }

    /// Send a read command and return its reply data.
    fn query(&self, cmd: u8, a: u8, b: u8) -> Result<Vec<u8>> {
        // Drop anything left over from earlier commands.
        let mut buf = [0u8; proto::LEN + 1];
        while self.dev.read_timeout(&mut buf, 0).is_ok_and(|n| n > 0) {}
        self.send(&proto::request(cmd, a, b))?;
        let deadline = Instant::now() + Duration::from_millis(TIMEOUT_MS as u64);
        while Instant::now() < deadline {
            let n = self.dev.read_timeout(&mut buf, TIMEOUT_MS)?;
            if n == 0 {
                break;
            }
            // Replies repeat the sub-command or channel (not checked where
            // it isn't known to: the version, the gradient).
            let expect = match cmd {
                proto::CMD_READ => Some(a),
                proto::CMD_QUERY if a != proto::SUB_GRADIENT => Some(a),
                _ => None,
            };
            if let Some(d) = proto::reply_data(&buf[..n], expect) {
                return Ok(d.to_vec());
            }
        }
        Err(Error::NoReply { cmd })
    }

    fn read_state(&mut self) -> Result<()> {
        let mask = self.query(proto::CMD_QUERY, proto::SUB_CHANNELS, 0)?;
        self.active = mask.first().copied().unwrap_or(0);
        let counts = self.query(proto::CMD_QUERY, proto::SUB_LED_COUNTS, 0)?;
        for (c, n) in self.counts.iter_mut().zip(counts.iter()) {
            *c = *n;
        }
        self.rg_swap = self.query(proto::CMD_QUERY, proto::SUB_RG_SWAP, 0)?.first().copied().unwrap_or(0);
        self.info.firmware = self.query(proto::CMD_VERSION, 0, 0).ok().and_then(|d| proto::parse_version(&d));
        for ch in 0..CHANNELS {
            if !self.has(ch) {
                continue;
            }
            let mut e = proto::parse_effect(&self.query(proto::CMD_READ, ch as u8, 0)?).ok_or(Error::NoReply { cmd: proto::CMD_READ })?;
            if proto::has_gradient(e.mode) {
                if let Some((stops, cycle)) = self.query(proto::CMD_QUERY, proto::SUB_GRADIENT, ch as u8).ok().and_then(|d| proto::parse_gradient(&d)) {
                    if !stops.is_empty() {
                        e.stops = stops;
                    }
                    e.cycle = cycle.max(1);
                }
            }
            // A direct state left behind by a program: back to static.
            if proto::DIRECT_STATES.contains(&e.mode) {
                e.mode = 1;
            }
            e.level = 0xff;
            self.effects[ch] = e;
        }
        log::debug!("{}: channels {:#04x}, LEDs {:?}, R/G swap {:#04x}", self.info.id, self.active, self.counts, self.rg_swap);
        Ok(())
    }

    fn has(&self, ch: usize) -> bool {
        self.active & (1 << ch) != 0
    }

    /// Channels lit by OpenDeviceControl lighting: present, with LEDs.
    fn lit(&self) -> impl Iterator<Item = usize> + '_ {
        (0..CHANNELS).filter(|&ch| self.has(ch) && self.counts[ch] > 0)
    }

    fn apply_effect(&self, ch: usize) -> Result<()> {
        self.send(&proto::set_effect(ch as u8, &self.effects[ch]))
    }

    /// Every channel back to its built-in effect. After direct colours the
    /// firmware only restarts an effect whose mode changes, so each channel
    /// is switched off first.
    fn apply_effects(&self) -> Result<()> {
        let present: Vec<usize> = (0..CHANNELS).filter(|&ch| self.has(ch)).collect();
        let mut first = Ok(());
        let mut note = |r: Result<()>| {
            if let (Err(e), true) = (r, first.is_ok()) {
                first = Err(e);
            }
        };
        for &ch in &present {
            let off = Effect { mode: 0, ..self.effects[ch].clone() };
            note(self.send(&proto::set_effect(ch as u8, &off)));
        }
        std::thread::sleep(Duration::from_millis(50));
        for &ch in &present {
            note(self.apply_effect(ch));
        }
        first
    }

    fn send_frame(&mut self) -> Result<()> {
        for r in proto::direct(&self.frame) {
            self.send(&r)?;
        }
        self.last_sent = Some(Instant::now());
        Ok(())
    }

    /// All present channels share one effect (ASRock's "Sync all").
    fn synced(&self) -> bool {
        let mut present = (0..CHANNELS).filter(|&ch| self.has(ch)).peekable();
        present.peek().is_some() && present.all(|ch| self.effects[ch].sync)
    }

    /// The effect controls of one channel, or of all of them (`ch` None)
    /// while they are synced.
    fn effect_controls(&self, ch: Option<usize>, v: &mut Vec<Control>) {
        let first = (0..CHANNELS).find(|&c| self.has(c)).unwrap_or(0);
        let e = &self.effects[ch.unwrap_or(first)];
        let (zone, key) = match ch {
            Some(c) => (proto::CHANNEL_NAMES[c], c.to_string()),
            None => ("All channels", "all".to_string()),
        };
        let id = |what: &str| format!("{HW_LIGHTING_PREFIX}{key}_{what}");
        let mode = proto::mode(e.mode);
        v.push(Control {
            id: id("mode"),
            name: format!("{zone}: Effect"),
            kind: ControlKind::Choice { options: MODES.iter().map(|m| m.1.to_string()).collect() },
            value: ControlValue::Choice(mode.map(|m| m.1.to_string()).unwrap_or_else(|| format!("Other ({:#04x})", e.mode))),
            help: None,
        });
        if mode.is_some_and(|m| m.2) {
            v.push(Control { id: id("color"), name: format!("{zone}: Colour"), kind: ControlKind::Color, value: ControlValue::Choice(e.color.to_hex()), help: None });
        }
        if proto::has_gradient(e.mode) {
            for (i, st) in e.stops.iter().enumerate() {
                v.push(Control {
                    id: id(&format!("stop{i}")),
                    name: format!("{zone}: Gradient colour {}", i + 1),
                    kind: ControlKind::Color,
                    value: ControlValue::Choice(st.color.to_hex()),
                    help: None,
                });
            }
        }
        if e.mode == 0x11 {
            v.push(Control {
                id: id("cycle"),
                name: format!("{zone}: Cycle LEDs"),
                kind: ControlKind::Number { min: 1.0, max: 100.0, step: 1.0, unit: String::new() },
                value: ControlValue::Number(e.cycle as f64),
                help: Some("How many LEDs one cycle of the gradient spans.".into()),
            });
        }
        if mode.is_some_and(|m| m.3) {
            v.push(Control {
                id: id("speed"),
                name: format!("{zone}: Speed"),
                kind: ControlKind::Number { min: 0.0, max: 100.0, step: 1.0, unit: "%".into() },
                value: ControlValue::Number(proto::speed_percent(e.speed)),
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
            help: Some("On: OpenDeviceControl's lighting effects drive the board's LEDs. Off: each zone runs its built-in effect, saved to the board when OpenDeviceControl closes.".into()),
        }];
        let synced = self.synced();
        v.push(Control {
            id: format!("{HW_LIGHTING_PREFIX}sync"),
            name: "Apply to all channels".into(),
            kind: ControlKind::Toggle,
            value: ControlValue::Bool(synced),
            help: Some("Every channel runs one effect, in step, like Select All Channel in ASRock's utility.".into()),
        });
        if synced {
            self.effect_controls(None, &mut v);
        } else {
            for ch in (0..CHANNELS).filter(|&ch| self.has(ch)) {
                self.effect_controls(Some(ch), &mut v);
            }
        }
        for ch in (0..CHANNELS).filter(|&ch| self.has(ch) && proto::addressable(ch)) {
            let zone = proto::CHANNEL_NAMES[ch];
            v.push(Control {
                id: format!("leds_{ch}"),
                name: format!("{zone}: LEDs"),
                kind: ControlKind::Number { min: 0.0, max: 100.0, step: 1.0, unit: String::new() },
                value: ControlValue::Number(self.counts[ch] as f64),
                help: Some("How many LEDs the strip or fans on this header have (stored on the board).".into()),
            });
            v.push(Control {
                id: format!("rg_swap_{ch}"),
                name: format!("{zone}: Swap red and green"),
                kind: ControlKind::Toggle,
                value: ControlValue::Bool(self.rg_swap & (1 << ch) != 0),
                help: Some("For strips and fans that show red as green (stored on the board).".into()),
            });
        }
        v
    }

    /// Change the built-in effect of channel `ch` (or of every channel) and
    /// apply it unless OpenDeviceControl lighting is on.
    fn update_effect(&mut self, ch: Option<usize>, f: impl Fn(&mut Effect)) -> Result<()> {
        let targets: Vec<usize> = match ch {
            Some(c) => vec![c],
            None => (0..CHANNELS).filter(|&c| self.has(c)).collect(),
        };
        for &c in &targets {
            f(&mut self.effects[c]);
        }
        self.changed = true;
        if self.software {
            return Ok(());
        }
        targets.iter().try_for_each(|&c| self.apply_effect(c))
    }

    /// Sync every zone to the first zone's effect, or let them go.
    fn set_sync(&mut self, on: bool) -> Result<()> {
        let first = (0..CHANNELS).find(|&c| self.has(c)).ok_or(Error::NotSupported(": no zones"))?;
        let base = self.effects[first].clone();
        self.update_effect(None, |e| {
            if on {
                *e = base.clone();
            }
            e.sync = on;
        })
    }
}

/// An approximation of what a built-in effect shows on `leds` LEDs at `t`
/// seconds, for the preview (the firmware's exact animations are unknown).
fn preview(e: &Effect, leds: usize, t: f64) -> Vec<Rgb> {
    // Speed byte 0 is fastest: one cycle in about 1 s, up to about 10 s.
    let period = 1.0 + e.speed as f64 / 255.0 * 9.0;
    let phase = (t / period).fract();
    let n = leds.max(1);
    let hue = |offset: f64| Rgb::from_hsv(((phase + offset).fract() * 360.0) as f32, 1.0, 1.0);
    let along = |i: usize| i as f64 / n as f64;
    let gradient = |x: f64| {
        let stops = if e.stops.is_empty() { proto::default_stops() } else { e.stops.clone() };
        let x = x.fract() * 100.0;
        let mut prev = stops[0];
        for s in stops.iter().copied().chain(std::iter::once(proto::Stop { color: stops[0].color, pos: 100 })) {
            if x <= s.pos as f64 {
                let span = (s.pos as f64 - prev.pos as f64).max(1.0);
                return prev.color.lerp(s.color, ((x - prev.pos as f64) / span) as f32);
            }
            prev = s;
        }
        stops[0].color
    };
    (0..leds)
        .map(|i| match e.mode {
            0x00 => Rgb::BLACK,
            0x01 => e.color,
            0x02 => e.color.scale((0.5 - 0.5 * (phase * std::f64::consts::TAU).cos()) as f32),
            0x03 => if phase < 0.15 { e.color } else { Rgb::BLACK },
            0x04 => hue(0.0),
            0x05 => {
                // A colour per LED that changes once a cycle.
                let k = (t / period).floor() as u64 * 7919 + i as u64 * 104729;
                Rgb::from_hsv((k % 360) as f32, 1.0, 1.0)
            }
            0x06 => hue(along(i)).scale(0.6),
            0x07 | 0x0d | 0x0e | 0x0f => hue(along(i) * if e.mode == 0x0f { -1.0 } else { 1.0 } + 1.0),
            0x08..=0x0c => {
                // Moving patterns in one colour: a lit band running along.
                let d = (along(i) - phase).rem_euclid(1.0);
                if d < 0.3 { e.color } else { e.color.scale(0.1) }
            }
            0x10 | 0x11 => gradient(along(i) + phase),
            _ => e.color,
        })
        .collect()
}

/// `#rrggbb`.
fn from_hex(s: &str) -> Option<Rgb> {
    let h = s.trim().strip_prefix('#').filter(|h| h.len() == 6)?;
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Rgb::new((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// `hw_<ch>_<what>` / `hw_all_<what>` -> (channel, or None for all; what).
fn hw_control(id: &str) -> Option<(Option<usize>, &str)> {
    let rest = id.strip_prefix(HW_LIGHTING_PREFIX)?;
    let (ch, what) = rest.split_once('_')?;
    if ch == "all" {
        return Some((None, what));
    }
    let ch: usize = ch.parse().ok()?;
    (ch < CHANNELS).then_some((Some(ch), what))
}

impl Device for Polychrome {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        if self.software && !self.frame.is_empty() && self.last_sent.is_none_or(|t| t.elapsed() >= REFRESH) {
            self.send_frame()?;
        }
        Ok(Snapshot { sensors: Vec::new(), controls: self.controls() })
    }

    fn poll_interval(&self) -> Option<Duration> {
        Some(if self.software { REFRESH / 2 } else { Duration::from_secs(2) })
    }

    fn frame_interval(&self) -> Option<Duration> {
        Some(FRAME_GAP)
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        let bad = || Error::InvalidValue(id.to_string());
        if id == SOFTWARE_LIGHTING {
            let ControlValue::Bool(on) = value else { return Err(bad()) };
            self.software = *on;
            self.last_sent = None;
            return if *on {
                if self.frame.is_empty() {
                    Ok(())
                } else {
                    self.send_frame()
                }
            } else {
                self.apply_effects()
            };
        }
        if id == format!("{HW_LIGHTING_PREFIX}sync") {
            let ControlValue::Bool(on) = value else { return Err(bad()) };
            return self.set_sync(*on);
        }
        if let Some((ch, what)) = hw_control(id).filter(|(ch, _)| ch.is_none_or(|c| self.has(c))) {
            return match (what, value) {
                ("mode", ControlValue::Choice(name)) => {
                    let m = MODES.iter().find(|m| m.1 == name).ok_or_else(bad)?.0;
                    self.update_effect(ch, |e| e.mode = m)
                }
                ("color", ControlValue::Choice(hex)) => {
                    let c = from_hex(hex).ok_or_else(bad)?;
                    self.update_effect(ch, |e| e.color = c)
                }
                ("speed", ControlValue::Number(n)) => {
                    let s = proto::speed_byte(*n);
                    self.update_effect(ch, |e| e.speed = s)
                }
                ("cycle", ControlValue::Number(n)) => {
                    let n = n.round().clamp(1.0, 100.0) as u8;
                    self.update_effect(ch, |e| e.cycle = n)
                }
                (w, ControlValue::Choice(hex)) if w.starts_with("stop") => {
                    let i: usize = w[4..].parse().map_err(|_| bad())?;
                    let c = from_hex(hex).ok_or_else(bad)?;
                    self.update_effect(ch, |e| {
                        if let Some(st) = e.stops.get_mut(i) {
                            st.color = c;
                        }
                    })
                }
                _ => Err(bad()),
            };
        }
        if let Some(ch) = id.strip_prefix("leds_").and_then(|c| c.parse::<usize>().ok()).filter(|&c| c < CHANNELS && proto::addressable(c)) {
            let ControlValue::Number(n) = value else { return Err(bad()) };
            let mut counts = self.counts;
            counts[ch] = n.round().clamp(0.0, 100.0) as u8;
            self.send(&proto::write_led_counts(&counts))?;
            self.counts = counts;
            return Ok(());
        }
        if let Some(ch) = id.strip_prefix("rg_swap_").and_then(|c| c.parse::<usize>().ok()).filter(|&c| c < CHANNELS && proto::addressable(c)) {
            let ControlValue::Bool(on) = value else { return Err(bad()) };
            let mask = if *on { self.rg_swap | 1 << ch } else { self.rg_swap & !(1 << ch) };
            self.send(&proto::write_rg_swap(mask))?;
            self.rg_swap = mask;
            return Ok(());
        }
        Err(Error::UnknownControl(id.to_string()))
    }

    fn preview_lights(&mut self, t: f64) -> Option<Vec<Rgb>> {
        if self.software {
            return None;
        }
        Some(self.lit().flat_map(|ch| preview(&self.effects[ch], self.counts[ch] as usize, t)).collect())
    }

    fn light_zones(&self) -> Vec<LightZone> {
        self.lit()
            .map(|ch| LightZone { id: format!("ch{ch}"), name: proto::CHANNEL_NAMES[ch].into(), leds: self.counts[ch] as usize })
            .collect()
    }

    fn set_lights(&mut self, colors: &[Rgb]) -> Result<()> {
        if !self.software {
            return Ok(());
        }
        let total: usize = self.lit().map(|ch| self.counts[ch] as usize).sum();
        let mut frame = colors.to_vec();
        frame.resize(total, Rgb::default());
        if frame == self.frame && self.last_sent.is_some_and(|t| t.elapsed() < REFRESH) {
            return Ok(());
        }
        self.frame = frame;
        self.send_frame()
    }

    fn release(&mut self) -> Result<()> {
        self.frame.clear();
        let restored = self.apply_effects();
        // Keep built-in effects changed here across reboots, as ASRock's
        // utility does when it exits.
        let saved = if self.changed {
            std::thread::sleep(Duration::from_millis(200));
            self.send(&proto::request(proto::CMD_SAVE, 0, 0))
        } else {
            Ok(())
        };
        restored.and(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hw_control_ids() {
        assert_eq!(hw_control("hw_2_mode"), Some((Some(2), "mode")));
        assert_eq!(hw_control("hw_7_speed"), Some((Some(7), "speed")));
        assert_eq!(hw_control("hw_all_stop1"), Some((None, "stop1")));
        assert_eq!(from_hex("#ff8000"), Some(Rgb::new(255, 128, 0)));
        assert_eq!(hw_control("hw_8_mode"), None);
        assert_eq!(hw_control("hw_x_mode"), None);
    }
}
