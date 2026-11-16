//! ASUS graphics card RGB lighting: the ENE controller (see [`super::ene`])
//! on the card's I²C bus.
//!
//! OpenDeviceControl lighting writes direct colours; with it off the card
//! runs a built-in effect. Built-in effects set here are applied but never
//! saved to the card's flash: the front-end remembers them and sets them
//! again when OpenDeviceControl starts. Without OpenDeviceControl running
//! the card shows whatever effect was last saved to it (by ASUS software,
//! or the factory rainbow).

use super::ene::{self, Ene, MODES};
use crate::devices::{Device, HW_LIGHTING_PREFIX, SOFTWARE_LIGHTING};
use crate::model::*;
use crate::smbus::{Guarded, SmBus};
use crate::{Error, Result};
use std::time::{Duration, Instant};

/// Least time between direct-colour frames.
const FRAME_GAP: Duration = Duration::from_millis(33);
/// Resend the last frame this often while nothing changes, in case another
/// program took the lights over.
const REFRESH: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq)]
pub struct Effect {
    pub mode: u8,
    pub speed: u8,
    pub reverse: bool,
    pub color: Rgb,
}

pub struct AuraGpu {
    info: DeviceInfo,
    bus: Guarded,
    addr: u8,
    leds: usize,
    effect: Effect,
    software: bool,
    frame: Vec<Rgb>,
    last_sent: Option<Instant>,
}

impl AuraGpu {
    /// Open the controller at `addr` on `bus`, which [`ene::identify`] has
    /// already recognised. `model` names the card.
    pub fn open(bus: Box<dyn SmBus>, addr: u8, id: String, model: String) -> Result<Self> {
        let mut bus = Guarded::new(bus);
        // Identified by its signature: reading its registers takes pointer
        // writes, so writes to this one address are allowed from here on
        // (and only colour and effect registers are ever written).
        bus.allow_writes(addr)?;
        let mut e = Ene { bus: &mut bus, addr };
        let name = e.name()?;
        if !name.starts_with("AUMA0-E6K5") {
            return Err(Error::Bus(format!("unknown ENE controller '{name}'")));
        }
        let config = e.read_n(ene::REG_CONFIG, 16)?;
        let leds = config[ene::CONFIG_LED_COUNT] as usize;
        if leds == 0 || leds > ene::MAX_LEDS {
            return Err(Error::Bus(format!("unexpected LED count {leds}")));
        }
        let ctl = e.read_n(ene::REG_DIRECT, 4)?;
        let colors = ene::from_rbg(&e.read_n(ene::REG_COLORS_EFFECT, 3)?);
        let mode = if ene::mode(ctl[1]).is_some() { ctl[1] } else { 0x01 };
        let effect = Effect {
            mode,
            speed: ctl[2].min(ene::SPEED_SLOWEST),
            reverse: ctl[3] != 0,
            color: colors.first().copied().filter(|c| *c != Rgb::BLACK).unwrap_or(Rgb::new(255, 0, 0)),
        };
        let mut g = AuraGpu {
            info: DeviceInfo {
                id,
                kind: DeviceKind::GraphicsCard,
                manufacturer: "ASUS".into(),
                model,
                vendor_string: None,
                product_string: Some(name),
                // Not a USB device.
                usb_vid: 0,
                usb_pid: 0,
                serial: None,
                firmware: None,
            },
            bus,
            addr,
            leds,
            effect,
            software: false,
            frame: Vec::new(),
            last_sent: None,
        };
        // A direct state left behind by another program: back to the effect.
        if ctl[0] != 0 {
            g.apply_effect()?;
        }
        log::debug!("{}: {} LEDs, effect {:?}", g.info.id, g.leds, g.effect);
        Ok(g)
    }

    fn ene(&mut self) -> Ene<'_> {
        Ene { bus: &mut self.bus, addr: self.addr }
    }

    /// Leave direct mode and run the built-in effect.
    fn apply_effect(&mut self) -> Result<()> {
        let e = self.effect.clone();
        let colors = ene::to_rbg(&vec![e.color; self.leds]);
        let mut c = self.ene();
        c.write(ene::REG_DIRECT, 0)?;
        c.write_n(ene::REG_COLORS_EFFECT, &colors)?;
        c.write(ene::REG_MODE, e.mode)?;
        c.write(ene::REG_SPEED, e.speed)?;
        c.write(ene::REG_DIRECTION, e.reverse as u8)?;
        c.write(ene::REG_APPLY, ene::APPLY)
    }

    fn enter_direct(&mut self) -> Result<()> {
        let mut c = self.ene();
        c.write(ene::REG_DIRECT, 1)?;
        c.write(ene::REG_APPLY, ene::APPLY)
    }

    fn send_frame(&mut self) -> Result<()> {
        let data = ene::to_rbg(&self.frame);
        self.ene().write_n(ene::REG_COLORS_DIRECT, &data)?;
        self.last_sent = Some(Instant::now());
        Ok(())
    }

    fn update_effect(&mut self, f: impl FnOnce(&mut Effect)) -> Result<()> {
        f(&mut self.effect);
        if self.software {
            Ok(())
        } else {
            self.apply_effect()
        }
    }

    fn controls(&self) -> Vec<Control> {
        let e = &self.effect;
        let id = |what: &str| format!("{HW_LIGHTING_PREFIX}{what}");
        let mut v = vec![
            Control {
                id: SOFTWARE_LIGHTING.into(),
                name: "OpenDeviceControl lighting".into(),
                kind: ControlKind::Toggle,
                value: ControlValue::Bool(self.software),
                help: Some("On: OpenDeviceControl's lighting effects drive the card's LEDs. Off: the card runs its built-in effect.".into()),
            },
            Control {
                id: id("mode"),
                name: "Effect".into(),
                kind: ControlKind::Choice { options: MODES.iter().map(|m| m.1.to_string()).collect() },
                value: ControlValue::Choice(ene::mode(e.mode).map(|m| m.1).unwrap_or("Static").to_string()),
                help: Some("Not saved to the card: without OpenDeviceControl running it shows the effect last saved by ASUS software.".into()),
            },
        ];
        let m = ene::mode(e.mode);
        if m.is_some_and(|m| m.2) {
            v.push(Control { id: id("color"), name: "Colour".into(), kind: ControlKind::Color, value: ControlValue::Choice(e.color.to_hex()), help: None });
        }
        if m.is_some_and(|m| m.3) {
            v.push(Control {
                id: id("speed"),
                name: "Speed".into(),
                kind: ControlKind::Number { min: 0.0, max: 100.0, step: 25.0, unit: "%".into() },
                value: ControlValue::Number(ene::speed_percent(e.speed)),
                help: None,
            });
        }
        if m.is_some_and(|m| m.4) {
            v.push(Control { id: id("reverse"), name: "Reverse direction".into(), kind: ControlKind::Toggle, value: ControlValue::Bool(e.reverse), help: None });
        }
        v
    }
}

/// An approximation of a built-in effect on `leds` LEDs at `t` seconds,
/// for the preview (the firmware's exact animations are unknown).
fn preview(e: &Effect, leds: usize, t: f64) -> Vec<Rgb> {
    // Speed 0 is fastest: one cycle in about 2 s, up to about 10 s.
    let period = 2.0 + e.speed as f64 * 2.0;
    let phase = (t / period).fract();
    let n = leds.max(1) as f64;
    let dir = if e.reverse { -1.0 } else { 1.0 };
    let hue = |offset: f64| Rgb::from_hsv(((phase * dir + offset).rem_euclid(1.0) * 360.0) as f32, 1.0, 1.0);
    let breathe = (0.5 - 0.5 * (phase * std::f64::consts::TAU).cos()) as f32;
    (0..leds)
        .map(|i| {
            let along = i as f64 / n;
            let band = (along - phase * dir).rem_euclid(1.0) < 0.35;
            match e.mode {
                0x00 => Rgb::BLACK,
                0x01 => e.color,
                0x02 => e.color.scale(breathe),
                0x03 => if phase < 0.5 { e.color } else { Rgb::BLACK },
                0x04 => hue(0.0),
                0x05 | 0x0b | 0x0c => hue(along),
                0x06 => hue(0.0).scale(breathe),
                0x07 | 0x09 => if band { e.color } else { e.color.scale(0.1) },
                0x08 | 0x0a => if band { hue(0.0) } else { Rgb::BLACK },
                0x0d => {
                    let k = (t * 8.0).floor() as u64 * 7919 + i as u64 * 104_729;
                    Rgb::from_hsv((k % 360) as f32, 1.0, if k.is_multiple_of(3) { 0.2 } else { 1.0 })
                }
                _ => e.color,
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

impl Device for AuraGpu {
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
        Some(if self.software { REFRESH / 2 } else { Duration::from_secs(5) })
    }

    fn frame_interval(&self) -> Option<Duration> {
        Some(FRAME_GAP)
    }

    fn remembered_controls(&self) -> Vec<String> {
        // The card forgets effects set without saving them when it powers off.
        ["mode", "color", "speed", "reverse"].iter().map(|w| format!("{HW_LIGHTING_PREFIX}{w}")).collect()
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
            return if *on {
                self.enter_direct()?;
                if self.frame.is_empty() {
                    Ok(())
                } else {
                    self.send_frame()
                }
            } else {
                self.apply_effect()
            };
        }
        match (id.strip_prefix(HW_LIGHTING_PREFIX), value) {
            (Some("mode"), ControlValue::Choice(name)) => {
                let m = MODES.iter().find(|m| m.1 == name).ok_or_else(bad)?.0;
                self.update_effect(|e| e.mode = m)
            }
            (Some("color"), ControlValue::Choice(hex)) => {
                let c = from_hex(hex).ok_or_else(bad)?;
                self.update_effect(|e| e.color = c)
            }
            (Some("speed"), ControlValue::Number(n)) => {
                let s = ene::speed_value(*n);
                self.update_effect(|e| e.speed = s)
            }
            (Some("reverse"), ControlValue::Bool(on)) => {
                let on = *on;
                self.update_effect(|e| e.reverse = on)
            }
            (Some(_), _) => Err(bad()),
            (None, _) => Err(Error::UnknownControl(id.to_string())),
        }
    }

    fn preview_lights(&mut self, t: f64) -> Option<Vec<Rgb>> {
        (!self.software).then(|| preview(&self.effect, self.leds, t))
    }

    fn light_zones(&self) -> Vec<LightZone> {
        vec![LightZone { id: "card".into(), name: "Card".into(), leds: self.leds }]
    }

    fn set_lights(&mut self, colors: &[Rgb]) -> Result<()> {
        if !self.software {
            return Ok(());
        }
        let mut frame = colors.to_vec();
        frame.resize(self.leds, Rgb::BLACK);
        if frame == self.frame && self.last_sent.is_some_and(|t| t.elapsed() < REFRESH) {
            return Ok(());
        }
        self.frame = frame;
        self.send_frame()
    }

    fn release(&mut self) -> Result<()> {
        self.frame.clear();
        self.software = false;
        self.apply_effect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smbus::mock::{MockBus, Op};

    fn card() -> MockBus {
        let mut bus = MockBus { present: vec![0x67], ..Default::default() };
        // The mock answers 0x81 with one value whatever the pointer, so
        // the name and table read as repeats of it: use the pointer-less
        // reads to check what the driver does, not the values.
        bus.bytes.insert((0x67, 0x81), b'A');
        bus
    }

    #[test]
    fn refuses_unknown_controllers_without_writing_colours() {
        let bus = card();
        let log = bus.log.clone();
        assert!(AuraGpu::open(Box::new(bus), 0x67, "gpu".into(), "Card".into()).is_err());
        // Only pointer writes went out.
        assert!(log.lock().unwrap().iter().all(|op| matches!(op, Op::Word(0x67, 0x00, _))));
    }

    #[test]
    fn preview_has_one_colour_per_led() {
        let e = Effect { mode: 0x05, speed: 2, reverse: false, color: Rgb::WHITE };
        assert_eq!(preview(&e, 4, 1.3).len(), 4);
        let off = Effect { mode: 0x00, ..e };
        assert!(preview(&off, 4, 0.0).iter().all(|c| *c == Rgb::BLACK));
    }
}
