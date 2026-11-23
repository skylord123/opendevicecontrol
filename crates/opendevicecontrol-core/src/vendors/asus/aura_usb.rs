//! ASUS motherboards' Aura lighting, over the board's USB LED controller
//! (see [`super::aura_usb_proto`]).
//!
//! OpenDeviceControl lighting streams colours to the motherboard's LEDs and
//! to each addressable header with LEDs set; with it off, each zone runs its
//! built-in effect. The controller can't report its effects, so the ones
//! shown are the last set here (remembered by the front-end) and re-applied
//! on opening. Effects changed here are saved to the board when
//! OpenDeviceControl lets go of it.

use super::aura_usb_proto::{self as proto, Effect, Kind, Zone, MODES};
use crate::devices::{Device, Ids, HW_LIGHTING_PREFIX, SOFTWARE_LIGHTING};
use crate::model::*;
use crate::{Error, Result};
use std::ffi::CString;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_millis(500);
/// Least time between direct-colour frames.
const FRAME_GAP: Duration = Duration::from_millis(50);
/// Resend the last frame this often while nothing changes.
const REFRESH: Duration = Duration::from_secs(1);
/// Most LEDs offered per addressable header (the stream allows 511).
const MAX_LEDS: u16 = 120;

pub(crate) fn discover(
    api: &hidapi::HidApi,
    all: &[&hidapi::DeviceInfo],
    ids: &mut Ids,
    skip_paths: &[CString],
    out: &mut Vec<(CString, Box<dyn Device>)>,
) {
    let mut pids: Vec<u16> = all.iter().filter(|d| d.vendor_id() == proto::VID && proto::kind(d.product_id()).is_some()).map(|d| d.product_id()).collect();
    pids.sort_unstable();
    pids.dedup();
    for pid in pids {
        let matches: Vec<&hidapi::DeviceInfo> = all.iter().copied().filter(|d| d.vendor_id() == proto::VID && d.product_id() == pid).collect();
        // The vendor-defined lighting interface where there is more than one.
        let Some(&d) = matches.iter().find(|d| d.usage_page() == proto::USAGE_PAGE).or(matches.first()) else { continue };
        if skip_paths.iter().any(|p| p.as_c_str() == d.path()) {
            continue;
        }
        let id = ids.next("asus-aura-usb");
        match api.open_path(d.path()).map_err(Error::from).and_then(|dev| AuraUsb::open(dev, id.clone(), d)) {
            Ok(a) => {
                log::info!("opened ASUS Aura USB lighting ({id}), firmware {}", a.info().firmware.as_deref().unwrap_or("unknown"));
                out.push((d.path().to_owned(), Box::new(a)));
            }
            Err(e) => log::warn!("failed to open ASUS Aura USB lighting ({:04x}:{pid:04x}): {e}", proto::VID),
        }
    }
}

pub struct AuraUsb {
    info: DeviceInfo,
    dev: hidapi::HidDevice,
    kind: Kind,
    cfg: proto::Config,
    zones: Vec<Zone>,
    effects: Vec<Effect>,
    /// LEDs on each addressable header (set by the user).
    counts: Vec<u16>,
    /// Every zone runs one effect.
    sync: bool,
    /// Polled at least once: set_control calls before that restore
    /// remembered settings, which are on the board already.
    polled: bool,
    /// Built-in effects changed since opening (saved on release).
    changed: bool,
    /// Zones were handed to direct colours this session.
    streamed: bool,
    gen1_sent: bool,
    software: bool,
    frame: Vec<Rgb>,
    last_sent: Option<Instant>,
}

impl AuraUsb {
    pub fn open(dev: hidapi::HidDevice, id: String, d: &hidapi::DeviceInfo) -> Result<Self> {
        let kind = proto::kind(d.product_id()).ok_or(Error::NotSupported(": unknown product"))?;
        let firmware = query(&dev, proto::CMD_FIRMWARE, proto::REPLY_FIRMWARE)?;
        let firmware = proto::parse_firmware(&firmware).ok_or(Error::NoReply { cmd: proto::CMD_FIRMWARE })?;
        let cfg = query(&dev, proto::CMD_CONFIG, proto::REPLY_CONFIG)?;
        let cfg = proto::parse_config(&cfg).ok_or(Error::NoReply { cmd: proto::CMD_CONFIG })?;
        log::debug!("{id}: firmware {firmware}, config table {:02x?}", cfg.raw);
        let zones = proto::zones(kind, &cfg, &firmware).map_err(|e| Error::Other(format!("ASUS Aura USB: {e} (config table {:02x?})", cfg.raw)))?;
        if zones.is_empty() {
            return Err(Error::Other("ASUS Aura USB: the config table lists no lighting".into()));
        }
        Ok(AuraUsb {
            info: DeviceInfo {
                id,
                kind: DeviceKind::Other,
                manufacturer: "ASUS".into(),
                model: match kind {
                    Kind::Mainboard => "Aura motherboard lighting".into(),
                    Kind::Addressable => "Aura addressable header controller".into(),
                },
                vendor_string: d.manufacturer_string().map(str::to_owned),
                product_string: d.product_string().map(str::to_owned),
                usb_vid: proto::VID,
                usb_pid: d.product_id(),
                serial: d.serial_number().map(str::to_owned).filter(|s| !s.trim().is_empty()),
                firmware: Some(firmware),
            },
            dev,
            kind,
            effects: vec![Effect::default(); zones.len()],
            counts: vec![0; cfg.headers],
            cfg,
            zones,
            sync: false,
            polled: false,
            changed: false,
            streamed: false,
            gen1_sent: false,
            software: false,
            frame: Vec::new(),
            last_sent: None,
        })
    }

    fn send(&self, report: &[u8]) -> Result<()> {
        self.dev.write(report)?;
        Ok(())
    }

    fn send_all(&self, reports: &[[u8; proto::LEN]]) -> Result<()> {
        reports.iter().try_for_each(|r| self.send(r))
    }

    /// LEDs zone `z` has on the direct stream.
    fn leds(&self, z: &Zone) -> usize {
        match z.header {
            Some(h) => self.counts[h] as usize,
            None => self.cfg.mainboard_leds,
        }
    }

    /// Zones lit by OpenDeviceControl lighting, in stream order.
    fn lit(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.zones.len()).filter(|&z| self.leds(&self.zones[z]) > 0)
    }

    fn apply_effect(&self, z: usize) -> Result<()> {
        self.send_all(&proto::set_effect(self.kind, &self.zones[z], &self.effects[z]))
    }

    /// Every zone back to its built-in effect; tries them all.
    fn apply_effects(&self) -> Result<()> {
        let mut first = Ok(());
        for z in 0..self.zones.len() {
            if let Err(e) = self.apply_effect(z) {
                first = first.and(Err(e));
            }
        }
        first
    }

    /// Hand zone `z` to the direct stream.
    fn enter_direct(&mut self, z: usize) -> Result<()> {
        if self.kind == Kind::Mainboard && !self.gen1_sent && self.zones[z].header.is_some() {
            self.send(&proto::gen1())?;
            self.gen1_sent = true;
        }
        self.streamed = true;
        self.send(&proto::enter_direct(self.kind, &self.zones[z]))
    }

    fn send_frame(&mut self) -> Result<()> {
        let mut at = 0;
        for z in self.lit() {
            let n = self.leds(&self.zones[z]);
            let Some(colors) = self.frame.get(at..at + n) else { break };
            self.send_all(&proto::direct(self.zones[z].direct_ch, colors))?;
            at += n;
        }
        self.last_sent = Some(Instant::now());
        Ok(())
    }

    /// The effect controls of zone `z`, or of all of them (`z` None) while
    /// they are synced.
    fn effect_controls(&self, z: Option<usize>, v: &mut Vec<Control>) {
        let e = &self.effects[z.unwrap_or(0)];
        let (zone, key) = match z {
            Some(z) => (self.zones[z].name.as_str(), self.zones[z].key.as_str()),
            None => ("All zones", "all"),
        };
        let id = |what: &str| format!("{HW_LIGHTING_PREFIX}{key}_{what}");
        let mode = proto::mode(e.mode);
        v.push(Control {
            id: id("mode"),
            name: format!("{zone}: Effect"),
            kind: ControlKind::Choice { options: MODES.iter().map(|m| m.1.to_string()).collect() },
            value: ControlValue::Choice(mode.map(|m| m.1.to_string()).unwrap_or_else(|| format!("Other ({:#04x})", e.mode))),
            help: Some("The board can't report its effect: this is the last one set here.".into()),
        });
        if mode.is_some_and(|m| m.2) {
            v.push(Control { id: id("color"), name: format!("{zone}: Colour"), kind: ControlKind::Color, value: ControlValue::Choice(e.color.to_hex()), help: None });
        }
    }

    fn controls(&self) -> Vec<Control> {
        let mut v = vec![Control {
            id: SOFTWARE_LIGHTING.into(),
            name: "OpenDeviceControl lighting".into(),
            kind: ControlKind::Toggle,
            value: ControlValue::Bool(self.software),
            help: Some("On: OpenDeviceControl's lighting effects drive the board's LEDs and the addressable headers with LEDs set. Off: each zone runs its built-in effect, saved to the board when OpenDeviceControl closes.".into()),
        }];
        if self.zones.len() > 1 {
            v.push(Control {
                id: format!("{HW_LIGHTING_PREFIX}sync"),
                name: "Apply to all channels".into(),
                kind: ControlKind::Toggle,
                value: ControlValue::Bool(self.sync),
                help: Some("Every zone runs one effect.".into()),
            });
        }
        if self.sync {
            self.effect_controls(None, &mut v);
        } else {
            for z in 0..self.zones.len() {
                self.effect_controls(Some(z), &mut v);
            }
        }
        for z in &self.zones {
            let Some(h) = z.header else { continue };
            v.push(Control {
                id: format!("leds_{h}"),
                name: format!("{}: LEDs", z.name),
                kind: ControlKind::Number { min: 0.0, max: MAX_LEDS as f64, step: 1.0, unit: String::new() },
                value: ControlValue::Number(self.counts[h] as f64),
                help: Some("How many LEDs the strip or fans on this header have, for OpenDeviceControl lighting (the board can't tell).".into()),
            });
        }
        v
    }

    /// Change the built-in effect of zone `z` (or of every zone) and apply
    /// it unless OpenDeviceControl lighting is on.
    fn update_effect(&mut self, z: Option<usize>, f: impl Fn(&mut Effect)) -> Result<()> {
        let targets: Vec<usize> = match z {
            Some(z) => vec![z],
            None => (0..self.zones.len()).collect(),
        };
        for &t in &targets {
            let before = self.effects[t];
            f(&mut self.effects[t]);
            self.changed |= self.polled && self.effects[t] != before;
        }
        if self.software {
            return Ok(());
        }
        targets.iter().try_for_each(|&t| self.apply_effect(t))
    }

    fn set_leds(&mut self, h: usize, n: u16) -> Result<()> {
        let was = self.counts[h];
        self.counts[h] = n;
        let Some(z) = self.zones.iter().position(|z| z.header == Some(h)) else { return Ok(()) };
        if !self.software || (was > 0) == (n > 0) {
            return Ok(());
        }
        // A header joins or leaves the stream.
        if n > 0 {
            self.enter_direct(z)
        } else {
            self.apply_effect(z)
        }
    }
}

/// Send a read command and return the reply that answers it.
fn query(dev: &hidapi::HidDevice, cmd: u8, reply: u8) -> Result<Vec<u8>> {
    let mut buf = [0u8; proto::LEN];
    // Drop anything left over from earlier commands.
    while dev.read_timeout(&mut buf, 0).is_ok_and(|n| n > 0) {}
    dev.write(&proto::request(cmd))?;
    let deadline = Instant::now() + TIMEOUT;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let n = dev.read_timeout(&mut buf, left.as_millis().max(1) as i32)?;
        if n == 0 {
            break;
        }
        if n >= 2 && buf[1] == reply {
            return Ok(buf[..n].to_vec());
        }
    }
    Err(Error::NoReply { cmd })
}

/// An approximation of what a built-in effect shows on `leds` LEDs at `t`
/// seconds, for the preview (the firmware's animations and pace are
/// unknown; this assumes one cycle in about 4 s).
fn preview(e: &Effect, leds: usize, t: f64) -> Vec<Rgb> {
    let phase = (t / 4.0).fract();
    let n = leds.max(1);
    let along = |i: usize| i as f64 / n as f64;
    let hue = |x: f64| Rgb::from_hsv((x.rem_euclid(1.0) * 360.0) as f32, 1.0, 1.0);
    let breath = (0.5 - 0.5 * (phase * std::f64::consts::TAU).cos()) as f32;
    let band = |i: usize| (along(i) - phase).rem_euclid(1.0) < 0.3;
    (0..leds)
        .map(|i| match e.mode {
            0x00 => Rgb::BLACK,
            0x01 => e.color,
            0x02 => e.color.scale(breath),
            0x03 => if phase < 0.15 { e.color } else { Rgb::BLACK },
            0x04 | 0x10 => hue(phase),
            0x05 | 0x0b | 0x11 | 0x12 => hue(along(i) + phase),
            0x06 => hue(phase).scale(breath),
            0x07 | 0x09 => if band(i) { e.color } else { e.color.scale(0.1) },
            0x08 | 0x0a | 0x0c => if band(i) { hue(phase) } else { Rgb::BLACK },
            0x0d => {
                // A colour per LED that changes a few times a second.
                let k = (t * 4.0).floor() as u64 * 7919 + i as u64 * 104729;
                Rgb::from_hsv((k % 360) as f32, 1.0, 1.0)
            }
            0x13 => Rgb::new(255, 0, 0).scale(breath),
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

impl Device for AuraUsb {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        self.polled = true;
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

    fn remembered_controls(&self) -> Vec<String> {
        // Restored in this order: LED counts, sync, then the synced or the
        // per-zone effects (whichever doesn't apply is ignored).
        let hw = |key: &str, what: &str| format!("{HW_LIGHTING_PREFIX}{key}_{what}");
        let mut v: Vec<String> = self.zones.iter().filter_map(|z| z.header).map(|h| format!("leds_{h}")).collect();
        v.push(format!("{HW_LIGHTING_PREFIX}sync"));
        for key in std::iter::once("all").chain(self.zones.iter().map(|z| z.key.as_str())) {
            v.push(hw(key, "mode"));
            v.push(hw(key, "color"));
        }
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
                return self.apply_effects();
            }
            for z in self.lit().collect::<Vec<_>>() {
                self.enter_direct(z)?;
            }
            return if self.frame.is_empty() { Ok(()) } else { self.send_frame() };
        }
        if id == format!("{HW_LIGHTING_PREFIX}sync") {
            let ControlValue::Bool(on) = value else { return Err(bad()) };
            if *on == self.sync {
                return Ok(());
            }
            self.sync = *on;
            if !*on {
                return Ok(());
            }
            let base = self.effects[0];
            return self.update_effect(None, |e| *e = base);
        }
        if let Some(rest) = id.strip_prefix(HW_LIGHTING_PREFIX) {
            let (key, what) = rest.split_once('_').ok_or_else(|| Error::UnknownControl(id.to_string()))?;
            let z = if key == "all" {
                None
            } else {
                Some(self.zones.iter().position(|z| z.key == key).ok_or_else(|| Error::UnknownControl(id.to_string()))?)
            };
            // Only the controls on show apply (see `remembered_controls`).
            if z.is_some() == self.sync {
                return Ok(());
            }
            return match (what, value) {
                ("mode", ControlValue::Choice(name)) => {
                    let m = MODES.iter().find(|m| m.1 == name).ok_or_else(bad)?.0;
                    self.update_effect(z, |e| e.mode = m)
                }
                ("color", ControlValue::Choice(hex)) => {
                    let c = from_hex(hex).ok_or_else(bad)?;
                    self.update_effect(z, |e| e.color = c)
                }
                _ => Err(bad()),
            };
        }
        if let Some(h) = id.strip_prefix("leds_").and_then(|h| h.parse::<usize>().ok()).filter(|&h| h < self.counts.len()) {
            let ControlValue::Number(n) = value else { return Err(bad()) };
            return self.set_leds(h, n.round().clamp(0.0, MAX_LEDS as f64) as u16);
        }
        Err(Error::UnknownControl(id.to_string()))
    }

    fn preview_lights(&mut self, t: f64) -> Option<Vec<Rgb>> {
        if self.software {
            return None;
        }
        Some(self.lit().flat_map(|z| preview(&self.effects[z], self.leds(&self.zones[z]), t)).collect())
    }

    fn light_zones(&self) -> Vec<LightZone> {
        let mut v = Vec::new();
        for z in self.lit() {
            let zone = &self.zones[z];
            match zone.header {
                Some(h) => v.push(LightZone { id: zone.key.clone(), name: zone.name.clone(), leds: self.counts[h] as usize }),
                None => {
                    // The 12 V RGB headers are the last motherboard LEDs.
                    let own = self.cfg.mainboard_leds - self.cfg.rgb_headers;
                    if own > 0 {
                        v.push(LightZone { id: "mb".into(), name: "Motherboard".into(), leds: own });
                    }
                    v.extend((0..self.cfg.rgb_headers).map(|i| LightZone { id: format!("rgb{}", i + 1), name: format!("RGB header {}", i + 1), leds: 1 }));
                }
            }
        }
        v
    }

    fn set_lights(&mut self, colors: &[Rgb]) -> Result<()> {
        if !self.software {
            return Ok(());
        }
        let total: usize = self.lit().map(|z| self.leds(&self.zones[z])).sum();
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
        let restored = if self.streamed || self.changed { self.apply_effects() } else { Ok(()) };
        // Keep effects changed here across reboots. Only the motherboard
        // controller has a save command.
        let saved = if self.changed && self.kind == Kind::Mainboard {
            std::thread::sleep(Duration::from_millis(50));
            self.send(&proto::commit())
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
    fn preview_sizes_and_colours() {
        let e = Effect { mode: 1, color: Rgb::new(1, 2, 3) };
        assert_eq!(preview(&e, 3, 0.5), [Rgb::new(1, 2, 3); 3]);
        assert!(preview(&Effect { mode: 0, ..e }, 2, 1.0).iter().all(|c| *c == Rgb::BLACK));
        for m in MODES {
            assert_eq!(preview(&Effect { mode: m.0, ..e }, 7, 2.3).len(), 7);
        }
        assert_eq!(from_hex("#ff8000"), Some(Rgb::new(255, 128, 0)));
        assert_eq!(from_hex("ff8000"), None);
    }
}
