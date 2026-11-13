//! Corsair iCUE Commander Core (bundled with H100i–H170i Elite Capellix /
//! Elite LCD AIOs), Commander Core XT, Commander ST and Commander DUO.
//!
//! **Experimental, untested on hardware.** Uses the endpoint protocol (see
//! [`super::endpoint`]) with prefix `0x08`. Initialisation follows
//! OpenLinkHub `src/devices/cc`, `ccxt` and `cduo` (Nikola Jurkovic,
//! GPL-3.0-or-later); liquidctl `commander_core.py` (ParkerMc and
//! contributors, GPL-3.0-or-later) documents the same reply layouts.
//!
//! Channel 0 of a Commander Core / ST is the AIO pump and temperature 0 its
//! coolant sensor; the Core XT and DUO have no pump. While opendevicecontrol runs,
//! the hub is in software mode (needed for lighting); `release()` puts it
//! back in hardware mode, where the firmware's stored curves take over.

use super::endpoint::{self, ep, EndpointIo};
use super::{rgb_bytes, HubChannel, AIO_PUMP_MIN, FAN_MIN};
use crate::model::*;
use crate::{Device, Error, Result};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Commander Core / ST: pump on channel 0, 6 fans, 7 LED ports (0 = pump).
    Core,
    /// Commander Core XT: 6 fans, external LED port + 6 LED ports.
    CoreXt,
    /// Commander DUO: 2 fans, 2 probes (lighting not supported yet).
    Duo,
}

pub struct CommanderModel {
    pub pid: u16,
    pub name: &'static str,
    pub slug: &'static str,
    /// HID report size used by OpenLinkHub for this model.
    pub packet: usize,
    pub family: Family,
}

pub static MODELS: &[CommanderModel] = &[
    CommanderModel { pid: 0x0c1c, name: "Commander Core", slug: "commander-core", packet: 96, family: Family::Core },
    CommanderModel { pid: 0x0c32, name: "Commander Core", slug: "commander-core", packet: 64, family: Family::Core },
    CommanderModel { pid: 0x0c2a, name: "Commander Core XT", slug: "commander-core-xt", packet: 384, family: Family::CoreXt },
    CommanderModel { pid: 0x0c56, name: "Commander DUO", slug: "commander-duo", packet: 64, family: Family::Duo },
];

pub fn model_for_pid(pid: u16) -> Option<&'static CommanderModel> {
    MODELS.iter().find(|m| m.pid == pid)
}

/// AIOs that ship with a Commander Core, by (pump version, radiator mm, LCD).
const AIOS: &[(u8, u16, bool, &str)] = &[
    (1, 240, false, "H100i Elite Capellix"),
    (2, 240, false, "H100i Elite Capellix"),
    (1, 280, false, "H115i Elite Capellix"),
    (1, 360, false, "H150i Elite Capellix"),
    (2, 360, false, "H150i Elite Capellix"),
    (1, 420, false, "H170i Elite Capellix"),
    (3, 240, true, "H100i Elite LCD"),
    (3, 360, true, "H150i Elite LCD"),
    (3, 420, true, "H170i Elite LCD"),
    (3, 240, false, "H100i Elite Capellix"),
    (3, 360, false, "H150i Elite Capellix"),
    (3, 420, false, "H170i Elite Capellix"),
    (5, 240, true, "H100i Elite LCD XT"),
    (6, 240, true, "H100i Elite LCD XT"),
    (5, 280, true, "H115i Elite LCD XT"),
    (5, 360, true, "H150i Elite LCD XT"),
    (6, 360, true, "H150i Elite LCD XT"),
    (5, 420, true, "H170i Elite LCD XT"),
    (5, 240, false, "H100i Elite Capellix XT"),
    (6, 240, false, "H100i Elite Capellix XT"),
    (5, 280, false, "H115i Elite Capellix XT"),
    (5, 360, false, "H150i Elite Capellix XT"),
    (6, 360, false, "H150i Elite Capellix XT"),
    (5, 420, false, "H170i Elite Capellix XT"),
];

/// AIO name from pump version and radiator size (LCD presence unknown to
/// opendevicecontrol: the non-LCD variant is preferred).
pub fn aio_name(pump_version: u8, radiator: u16) -> Option<&'static str> {
    let find = |lcd| AIOS.iter().find(|a| a.0 == pump_version && a.1 == radiator && a.2 == lcd).map(|a| a.3);
    find(false).or_else(|| find(true))
}

/// Per-port LED device command code, chosen by the LED count the port
/// reports (OpenLinkHub `getLedDevices`).
pub fn led_command(leds: u16) -> Option<(u8, &'static str)> {
    Some(match leds {
        4 => (0x03, "ML PRO RGB fan"),
        8 => (0x05, "8-LED fan"),
        10 => (0x01, "RGB LED strip"),
        12 => (0x04, "HD RGB fan"),
        16 => (0x02, "LL RGB fan"),
        21 | 24 | 29 => (0x08, "Pump"),
        34 => (0x06, "QL RGB fan"),
        _ => return None,
    })
}

/// Data for the SET_LED_PORTS write: `0d 00 07`, then per port either
/// `01 <command>` (active) or `00`. The Core XT has an external-hub slot
/// first (always disabled here).
pub fn led_ports_data(family: Family, ports: &[Option<u16>]) -> Vec<u8> {
    let mut b = vec![0x0d, 0x00, 0x07];
    if family == Family::CoreXt {
        b.push(0x00);
    }
    for p in ports {
        match p.and_then(led_command) {
            Some((cmd, _)) => b.extend_from_slice(&[0x01, cmd]),
            None => b.push(0x00),
        }
    }
    b
}

fn connected(status: u8) -> bool {
    matches!(status, 0x02 | 0x03 | 0x07)
}

struct LedPort {
    name: String,
    leds: usize,
}

pub struct Commander {
    io: EndpointIo,
    info: DeviceInfo,
    model: &'static CommanderModel,
    channels: Vec<HubChannel>,
    /// Hub speed index → sensor id prefix for connected fans/pump.
    speed_ids: Vec<(usize, String, String)>,
    /// Temperature index → (sensor id, name).
    temp_ids: Vec<(usize, String, String)>,
    ports: Vec<LedPort>,
    coolant: Option<f64>,
    applied: Vec<(u8, u8)>,
    applied_at: Option<Instant>,
}

impl Commander {
    pub fn open(dev: hidapi::HidDevice, model: &'static CommanderModel, id: String, serial: Option<String>) -> Result<Self> {
        let io = EndpointIo::new(dev, &[0x08], model.packet);
        let firmware = io.firmware();
        if firmware.is_none() {
            return Err(Error::NoReply { cmd: endpoint::GET_FIRMWARE[0] });
        }
        io.transfer(endpoint::SOFTWARE_MODE, &[])?;

        let has_pump = model.family == Family::Core;
        let mut product = model.name.to_string();
        let mut aio = None;
        if has_pump {
            let pump = io.transfer(&[0x02, 0x57], &[]).ok().and_then(|r| r.get(2).copied());
            let rad = io.transfer(&[0x02, 0x58], &[]).ok().and_then(|r| Some(u16::from_le_bytes([*r.get(2)?, *r.get(3)?])));
            aio = pump.zip(rad).and_then(|(p, r)| aio_name(p, r));
            if let Some(a) = aio {
                product = format!("{} ({a})", model.name);
            }
        }

        // LED ports must be initialised before they report what is attached.
        let mut ports = Vec::new();
        if model.family != Family::Duo {
            let range = if model.family == Family::Core { 0..=6u8 } else { 1..=6u8 };
            for i in range {
                let _ = io.transfer(&[0x14, i, 0x01], &[]);
            }
            std::thread::sleep(Duration::from_millis(500));
            let leds = io.read_endpoint(ep::GET_LEDS, None).map(|r| endpoint::parse_led_ports(&r)).unwrap_or_default();
            let leds: Vec<Option<u16>> = match model.family {
                Family::Core => leds.into_iter().take(7).collect(),
                _ => leds.into_iter().skip(1).take(6).collect(),
            };
            let data = led_ports_data(model.family, &leds);
            if let Err(e) = io.write_endpoint(ep::SET_LED_PORTS, &endpoint::write_payload(None, &data, false)) {
                log::warn!("{}: configuring LED ports: {e}", model.name);
            }
            if model.family == Family::CoreXt {
                let _ = io.transfer(&[0x15, 0x01], &[]);
            }
            for (i, l) in leds.iter().enumerate() {
                if let Some((_, kind)) = l.and_then(led_command) {
                    let n = l.unwrap() as usize;
                    let name = if model.family == Family::Core && i == 0 {
                        format!("{} pump", aio.unwrap_or("AIO"))
                    } else {
                        let port = if model.family == Family::Core { i } else { i + 1 };
                        format!("RGB port {port} ({kind})")
                    };
                    ports.push(LedPort { name, leds: n });
                }
            }
            if !ports.is_empty() {
                io.open_color_endpoint()?;
            }
        }

        // Fans / pump.
        let fans = io.read_endpoint(ep::GET_FANS, None).unwrap_or_default();
        let (n, status) = endpoint::reply_entries(&fans);
        let temps = io.read_endpoint(ep::GET_TEMPS, None).map(|r| endpoint::parse_temps(&r)).unwrap_or_default();

        let mut temp_ids = Vec::new();
        for (i, t) in temps.iter().enumerate() {
            if t.is_none() {
                continue;
            }
            if has_pump && i == 0 {
                temp_ids.push((i, "coolant_temp".to_string(), "Coolant temperature".to_string()));
            } else {
                let k = if has_pump { i } else { i + 1 };
                temp_ids.push((i, format!("probe{k}_temp"), format!("Temperature probe {k}")));
            }
        }
        let curve = temp_ids.first().map(|t| t.1.clone()).unwrap_or_default();

        let mut channels = Vec::new();
        let mut speed_ids = Vec::new();
        for i in 0..n {
            if !status.get(i).copied().is_some_and(connected) {
                continue;
            }
            let (cid, name, pump) = if has_pump && i == 0 {
                ("pump".to_string(), format!("{} pump", aio.unwrap_or("AIO")), true)
            } else {
                let k = if has_pump { i } else { i + 1 };
                (format!("fan{k}"), format!("Fan {k}"), false)
            };
            speed_ids.push((i, cid.clone(), name.clone()));
            channels.push(HubChannel {
                hw: i as u8,
                id: cid,
                name,
                pump,
                min: if pump { AIO_PUMP_MIN } else { FAN_MIN },
                curve_sensor: curve.clone(),
                target: FanTarget::Hardware,
            });
        }

        let mut c = Self {
            io,
            info: DeviceInfo {
                id,
                kind: if has_pump { DeviceKind::Cooler } else { DeviceKind::FanHub },
                manufacturer: "Corsair".into(),
                model: product,
                vendor_string: None,
                product_string: None,
                usb_vid: crate::CORSAIR_VID,
                usb_pid: model.pid,
                serial,
                firmware,
            },
            model,
            channels,
            speed_ids,
            temp_ids,
            ports,
            coolant: None,
            applied: Vec::new(),
            applied_at: None,
        };
        c.coolant = temps.first().copied().flatten().filter(|_| has_pump);
        c.push_speeds(true)?;
        Ok(c)
    }

    fn duties(&self) -> Vec<(u8, u8)> {
        self.channels.iter().map(|c| (c.hw, c.duty(self.coolant))).collect()
    }

    /// Send duties if they changed (or `force`, or every 10 s).
    fn push_speeds(&mut self, force: bool) -> Result<()> {
        if self.channels.is_empty() {
            return Ok(());
        }
        let d = self.duties();
        let stale = self.applied_at.is_none_or(|t| t.elapsed() > Duration::from_secs(10));
        let changed = d.len() != self.applied.len() || d.iter().zip(&self.applied).any(|(a, b)| a.1.abs_diff(b.1) >= 2);
        if force || changed || stale {
            self.io.set_speeds(&d)?;
            self.applied = d;
            self.applied_at = Some(Instant::now());
        }
        Ok(())
    }
}

impl Device for Commander {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        let mut s = Vec::new();
        let speeds = endpoint::parse_speeds_plain(&self.io.read_endpoint(ep::GET_SPEEDS, None)?);
        let temps = endpoint::parse_temps(&self.io.read_endpoint(ep::GET_TEMPS, None)?);
        for (i, id, name) in &self.temp_ids {
            if let Some(Some(t)) = temps.get(*i) {
                s.push(Sensor::new(id, name, SensorKind::Temperature, *t));
            }
        }
        if self.model.family == Family::Core {
            self.coolant = temps.first().copied().flatten();
        }
        for (i, id, name) in &self.speed_ids {
            if let Some(&rpm) = speeds.get(*i) {
                s.push(Sensor::new(&format!("{id}_speed"), &format!("{name} speed"), SensorKind::FanSpeed, rpm as f64));
            }
        }
        self.push_speeds(false)?;
        for c in &self.channels {
            s.push(Sensor::new(&format!("{}_duty", c.id), &format!("{} duty", c.name), SensorKind::Duty, c.duty(self.coolant) as f64));
        }
        Ok(Snapshot { sensors: s, controls: Vec::new() })
    }

    fn set_control(&mut self, id: &str, _value: &ControlValue) -> Result<()> {
        Err(Error::UnknownControl(id.into()))
    }

    fn fan_channels(&self) -> Vec<FanChannel> {
        self.channels.iter().map(HubChannel::fan_channel).collect()
    }

    fn set_fan(&mut self, channel: &str, target: FanTarget) -> Result<()> {
        let c = self.channels.iter_mut().find(|c| c.id == channel).ok_or_else(|| Error::UnknownControl(channel.into()))?;
        c.target = target;
        self.push_speeds(true)
    }

    fn light_zones(&self) -> Vec<LightZone> {
        self.ports
            .iter()
            .enumerate()
            .map(|(i, p)| LightZone { id: format!("port{i}"), name: p.name.clone(), leds: p.leds })
            .collect()
    }

    fn set_lights(&mut self, colors: &[Rgb]) -> Result<()> {
        if self.ports.is_empty() {
            return Ok(());
        }
        let total: usize = self.ports.iter().map(|p| p.leds).sum();
        self.io.write_colors(&rgb_bytes(colors, total))
    }

    fn release(&mut self) {
        if let Err(e) = self.io.transfer(endpoint::HARDWARE_MODE, &[]) {
            log::warn!("{}: could not return to hardware mode: {e}", self.model.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn led_ports_payload() {
        // Core: pump (29 LEDs) on port 0, QL fan on port 2.
        let ports = [Some(29), None, Some(34), None, None, None, None];
        assert_eq!(led_ports_data(Family::Core, &ports), vec![0x0d, 0x00, 0x07, 0x01, 0x08, 0x00, 0x01, 0x06, 0, 0, 0, 0]);
        // XT: external slot first.
        let ports = [Some(8), None, None, None, None, None];
        assert_eq!(led_ports_data(Family::CoreXt, &ports), vec![0x0d, 0x00, 0x07, 0x00, 0x01, 0x05, 0, 0, 0, 0, 0]);
        // Unknown LED counts are not activated.
        assert_eq!(led_ports_data(Family::Core, &[Some(7)]), vec![0x0d, 0x00, 0x07, 0x00]);
    }

    #[test]
    fn aio_lookup() {
        assert_eq!(aio_name(1, 360), Some("H150i Elite Capellix"));
        assert_eq!(aio_name(5, 280), Some("H115i Elite Capellix XT"));
        assert_eq!(aio_name(9, 360), None);
    }

    #[test]
    fn pids_unique() {
        let mut v: Vec<_> = MODELS.iter().map(|m| m.pid).collect();
        v.sort();
        v.dedup();
        assert_eq!(v.len(), MODELS.len());
    }
}
