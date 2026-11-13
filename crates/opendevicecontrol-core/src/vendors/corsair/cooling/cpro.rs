//! Corsair Commander Pro (and the Obsidian 1000D's built-in one), Lighting
//! Node Pro and Lighting Node Core.
//!
//! **Experimental, untested on hardware.** 64-byte output reports `[cmd,
//! data…]`, 16-byte replies `[status, data…]`. Fan, probe and voltage
//! commands follow liquidctl `commander_pro.py` (Marshall Asch and
//! contributors) and the Linux `corsair-cpro` driver; direct LED control
//! follows OpenLinkHub `src/devices/{cpro,lnpro,lncore}` (Nikola Jurkovic).
//! All GPL-3.0-or-later.
//!
//! The hardware cannot tell what is plugged into an LED channel, so the LED
//! count of each channel is a setting (remembered by opendevicecontrol).
//!
//! The Commander Pro has no "hardware mode" switch: for `Hardware` and on
//! exit opendevicecontrol programs a temperature → RPM profile on the first
//! connected probe into the firmware (or a fixed 70 % duty without probes),
//! so fans keep reacting to temperature after opendevicecontrol stops.

use super::{rgb_bytes, FAN_MIN};
use crate::model::*;
use crate::{Device, Error, Result};

pub struct CproModel {
    pub pid: u16,
    pub name: &'static str,
    pub slug: &'static str,
    pub fans: usize,
    pub probes: usize,
    pub led_channels: usize,
}

pub static MODELS: &[CproModel] = &[
    CproModel { pid: 0x0c10, name: "Commander Pro", slug: "commander-pro", fans: 6, probes: 4, led_channels: 2 },
    CproModel { pid: 0x1d00, name: "Obsidian 1000D", slug: "obsidian-1000d", fans: 6, probes: 4, led_channels: 2 },
    CproModel { pid: 0x0c0b, name: "Lighting Node Pro", slug: "lighting-node-pro", fans: 0, probes: 0, led_channels: 2 },
    CproModel { pid: 0x1d04, name: "Lighting Node Pro", slug: "lighting-node-pro", fans: 0, probes: 0, led_channels: 2 },
    CproModel { pid: 0x0c1a, name: "Lighting Node Core", slug: "lighting-node-core", fans: 0, probes: 0, led_channels: 1 },
];

pub fn model_for_pid(pid: u16) -> Option<&'static CproModel> {
    MODELS.iter().find(|m| m.pid == pid)
}

mod cmd {
    pub const GET_FIRMWARE: u8 = 0x02;
    pub const GET_TEMP_CONFIG: u8 = 0x10;
    pub const GET_TEMP: u8 = 0x11;
    pub const GET_VOLTS: u8 = 0x12;
    pub const GET_FAN_MODES: u8 = 0x20;
    pub const GET_FAN_RPM: u8 = 0x21;
    pub const SET_FAN_DUTY: u8 = 0x23;
    pub const SET_FAN_PROFILE: u8 = 0x25;
    pub const WRITE_COLOR: u8 = 0x32;
    pub const COMMIT: u8 = 0x33;
    pub const REFRESH: u8 = 0x34;
    pub const LED_CONFIG: u8 = 0x35;
    pub const LED_RESET: u8 = 0x37;
    pub const PORT_STATE: u8 = 0x38;
}

/// Most LEDs one channel can drive.
pub const MAX_LEDS: usize = 204;
/// LEDs per colour-plane chunk.
const CHUNK: usize = 50;
/// Built-in RPM profile used for `Hardware` and on exit: probe °C → RPM.
pub const DEFAULT_PROFILE: [(u16, u16); 6] = [(25, 600), (30, 800), (35, 1000), (40, 1300), (45, 1700), (50, 5000)];
const NO_PROBE_DUTY: u8 = 70;

/// SET_FAN_PROFILE data: fan, probe, six big-endian temperatures in
/// centi-degrees, six big-endian RPM targets (liquidctl `set_speed_profile`).
pub fn profile_data(fan: u8, probe: u8, points: &[(u16, u16); 6]) -> Vec<u8> {
    let mut b = vec![0u8; 26];
    b[0] = fan;
    b[1] = probe;
    for (i, (t, rpm)) in points.iter().enumerate() {
        b[2 + i * 2..4 + i * 2].copy_from_slice(&(t * 100).to_be_bytes());
        b[14 + i * 2..16 + i * 2].copy_from_slice(&rpm.to_be_bytes());
    }
    b
}

/// The WRITE_COLOR packets for one channel: per chunk of up to 50 LEDs,
/// one packet per colour plane `[channel, start, count, plane, values…]`.
pub fn color_packets(channel: u8, colors: &[Rgb]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for (ci, chunk) in colors.chunks(CHUNK).enumerate() {
        for plane in 0..3u8 {
            let mut p = vec![channel, (ci * CHUNK) as u8, chunk.len() as u8, plane];
            p.extend(chunk.iter().map(|c| match plane {
                0 => c.r,
                1 => c.g,
                _ => c.b,
            }));
            out.push(p);
        }
    }
    out
}

pub struct CommanderPro {
    dev: hidapi::HidDevice,
    info: DeviceInfo,
    model: &'static CproModel,
    probes: Vec<usize>,
    fans: Vec<usize>,
    leds: Vec<usize>,
    software_leds: Vec<bool>,
    duties: Vec<Option<u8>>,
}

impl CommanderPro {
    pub fn open(dev: hidapi::HidDevice, model: &'static CproModel, id: String, serial: Option<String>) -> Result<Self> {
        let mut c = Self {
            dev,
            info: DeviceInfo {
                id,
                kind: if model.fans > 0 { DeviceKind::FanHub } else { DeviceKind::Other },
                manufacturer: "Corsair".into(),
                model: model.name.into(),
                vendor_string: None,
                product_string: None,
                usb_vid: crate::CORSAIR_VID,
                usb_pid: model.pid,
                serial,
                firmware: None,
            },
            model,
            probes: Vec::new(),
            fans: Vec::new(),
            leds: vec![0; model.led_channels],
            software_leds: vec![false; model.led_channels],
            duties: vec![None; model.fans],
        };
        let fw = c.send(cmd::GET_FIRMWARE, &[])?;
        c.info.firmware = Some(format!("{}.{}.{}", fw[1], fw[2], fw[3]));
        if model.probes > 0 {
            let r = c.send(cmd::GET_TEMP_CONFIG, &[])?;
            c.probes = (0..model.probes).filter(|&i| r.get(1 + i) == Some(&1)).collect();
        }
        if model.fans > 0 {
            let r = c.send(cmd::GET_FAN_MODES, &[])?;
            // 0 = disconnected, 1 = DC (3-pin), 2 = PWM (4-pin).
            c.fans = (0..model.fans).filter(|&i| matches!(r.get(1 + i), Some(1) | Some(2))).collect();
        }
        Ok(c)
    }

    fn send(&self, command: u8, data: &[u8]) -> Result<Vec<u8>> {
        let mut out = [0u8; 65];
        out[1] = command;
        let n = data.len().min(63);
        out[2..2 + n].copy_from_slice(&data[..n]);
        let mut junk = [0u8; 64];
        while matches!(self.dev.read_timeout(&mut junk, 0), Ok(n) if n > 0) {}
        self.dev.write(&out)?;
        let mut buf = [0u8; 64];
        let n = self.dev.read_timeout(&mut buf, 250)?;
        if n == 0 {
            return Err(Error::NoReply { cmd: command });
        }
        Ok(buf[..n.max(16)].to_vec())
    }

    fn be16(r: &[u8]) -> f64 {
        u16::from_be_bytes([r[1], r[2]]) as f64
    }

    fn hardware_fan(&self, fan: usize) -> Result<()> {
        match self.probes.first() {
            Some(&p) => self.send(cmd::SET_FAN_PROFILE, &profile_data(fan as u8, p as u8, &DEFAULT_PROFILE)).map(|_| ()),
            None => self.send(cmd::SET_FAN_DUTY, &[fan as u8, NO_PROBE_DUTY]).map(|_| ()),
        }
    }

    fn led_control_id(ch: usize) -> String {
        format!("led_count_{}", ch + 1)
    }

    /// Prepare a channel for direct colour writes (OpenLinkHub `setColorEndpoint`).
    fn init_led_channel(&mut self, ch: usize) -> Result<()> {
        let n = self.leds[ch] as u8;
        self.send(cmd::LED_RESET, &[ch as u8])?;
        self.send(cmd::LED_CONFIG, &[ch as u8, 0x00, n, 0x04, 0, 0, 0, 0, 0xff, 0xff, 0xff])?;
        self.software_leds[ch] = false;
        Ok(())
    }

    /// Hand a channel back to the built-in rainbow effect (OpenLinkHub `ShutdownLed`).
    fn hardware_leds(&self, ch: usize) -> Result<()> {
        let n = self.leds[ch] as u8;
        self.send(cmd::LED_RESET, &[ch as u8])?;
        self.send(cmd::REFRESH, &[ch as u8])?;
        self.send(cmd::PORT_STATE, &[ch as u8, 0x01])?;
        self.send(cmd::LED_CONFIG, &[ch as u8, 0x00, n, 0x00, 0x01, 0x01, 0x01, 0x00, 0x00])?;
        Ok(())
    }
}

impl Device for CommanderPro {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        let mut s = Vec::new();
        for &p in &self.probes {
            let r = self.send(cmd::GET_TEMP, &[p as u8])?;
            s.push(Sensor::new(&format!("probe{}_temp", p + 1), &format!("Temperature probe {}", p + 1), SensorKind::Temperature, Self::be16(&r) / 100.0));
        }
        for &f in &self.fans {
            let r = self.send(cmd::GET_FAN_RPM, &[f as u8])?;
            s.push(Sensor::new(&format!("fan{}_speed", f + 1), &format!("Fan {} speed", f + 1), SensorKind::FanSpeed, Self::be16(&r)));
            if let Some(d) = self.duties[f] {
                s.push(Sensor::new(&format!("fan{}_duty", f + 1), &format!("Fan {} duty", f + 1), SensorKind::Duty, d as f64));
            }
        }
        if self.model.fans > 0 {
            for (i, rail) in ["12v", "5v", "3v3"].iter().enumerate() {
                if let Ok(r) = self.send(cmd::GET_VOLTS, &[i as u8]) {
                    let label = ["+12V", "+5V", "+3.3V"][i];
                    s.push(Sensor::new(&format!("rail_{rail}_voltage"), &format!("{label} rail"), SensorKind::Voltage, Self::be16(&r) / 1000.0).diagnostic());
                }
            }
        }
        let controls = (0..self.model.led_channels)
            .map(|ch| Control {
                id: Self::led_control_id(ch),
                name: format!("LEDs on channel {}", ch + 1),
                kind: ControlKind::Number { min: 0.0, max: MAX_LEDS as f64, step: 1.0, unit: "LEDs".into() },
                value: ControlValue::Number(self.leds[ch] as f64),
                help: Some("Total LEDs of the strips/fans on this channel (e.g. 8 per SP RGB Elite fan, 10 per RGB strip).".into()),
            })
            .collect();
        Ok(Snapshot { sensors: s, controls })
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        let ch = (0..self.model.led_channels).find(|&c| Self::led_control_id(c) == id).ok_or_else(|| Error::UnknownControl(id.into()))?;
        let ControlValue::Number(n) = value else { return Err(Error::InvalidValue(id.into())) };
        if !n.is_finite() || *n < 0.0 {
            return Err(Error::InvalidValue(id.into()));
        }
        self.leds[ch] = (n.round() as usize).min(MAX_LEDS);
        self.init_led_channel(ch)
    }

    fn remembered_controls(&self) -> Vec<String> {
        (0..self.model.led_channels).map(Self::led_control_id).collect()
    }

    fn fan_channels(&self) -> Vec<FanChannel> {
        let curve = self.probes.first().map(|p| format!("probe{}_temp", p + 1)).unwrap_or_default();
        self.fans
            .iter()
            .map(|f| FanChannel { id: format!("fan{}", f + 1), name: format!("Fan {}", f + 1), min_duty: FAN_MIN, curve_sensor: curve.clone() })
            .collect()
    }

    fn set_fan(&mut self, channel: &str, target: FanTarget) -> Result<()> {
        let f = self.fans.iter().copied().find(|f| format!("fan{}", f + 1) == channel).ok_or_else(|| Error::UnknownControl(channel.into()))?;
        match target {
            FanTarget::Hardware => {
                self.duties[f] = None;
                self.hardware_fan(f)
            }
            FanTarget::Duty(d) => {
                let d = d.clamp(FAN_MIN, 100);
                self.send(cmd::SET_FAN_DUTY, &[f as u8, d])?;
                self.duties[f] = Some(d);
                Ok(())
            }
        }
    }

    fn light_zones(&self) -> Vec<LightZone> {
        (0..self.model.led_channels)
            .filter(|&c| self.leds[c] > 0)
            .map(|c| LightZone { id: format!("channel{}", c + 1), name: format!("LED channel {}", c + 1), leds: self.leds[c] })
            .collect()
    }

    fn set_lights(&mut self, colors: &[Rgb]) -> Result<()> {
        let mut offset = 0;
        for ch in 0..self.model.led_channels {
            let n = self.leds[ch];
            if n == 0 {
                continue;
            }
            let bytes = rgb_bytes(colors.get(offset..).unwrap_or(&[]), n);
            offset += n;
            let frame: Vec<Rgb> = bytes.chunks(3).map(|c| Rgb::new(c[0], c[1], c[2])).collect();
            self.send(cmd::PORT_STATE, &[ch as u8, 0x02])?;
            if !self.software_leds[ch] {
                self.send(cmd::PORT_STATE, &[ch as u8])?;
                self.software_leds[ch] = true;
            }
            for p in color_packets(ch as u8, &frame) {
                self.send(cmd::WRITE_COLOR, &p)?;
            }
            self.send(cmd::COMMIT, &[0xff])?;
        }
        Ok(())
    }

    fn release(&mut self) {
        for f in self.fans.clone() {
            if let Err(e) = self.hardware_fan(f) {
                log::warn!("{}: fan {}: could not restore a firmware curve: {e}", self.model.name, f + 1);
            }
        }
        let mut any = false;
        for ch in 0..self.model.led_channels {
            if self.leds[ch] > 0 {
                any = true;
                if let Err(e) = self.hardware_leds(ch) {
                    log::warn!("{}: LED channel {}: {e}", self.model.name, ch + 1);
                }
            }
        }
        if any {
            let _ = self.send(cmd::COMMIT, &[0xff]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_layout() {
        let b = profile_data(2, 0, &DEFAULT_PROFILE);
        assert_eq!(b.len(), 26);
        assert_eq!(&b[..4], &[2, 0, 0x09, 0xc4]); // 25 °C * 100 = 2500
        assert_eq!(&b[14..16], &[0x02, 0x58]); // 600 RPM
        assert_eq!(&b[24..26], &[0x13, 0x88]); // 5000 RPM
    }

    #[test]
    fn color_planes_are_chunked() {
        let colors = vec![Rgb::new(1, 2, 3); 60];
        let p = color_packets(1, &colors);
        assert_eq!(p.len(), 6);
        assert_eq!(&p[0][..5], &[1, 0, 50, 0, 1]);
        assert_eq!(&p[1][..5], &[1, 0, 50, 1, 2]);
        assert_eq!(&p[3][..5], &[1, 50, 10, 0, 1]);
        assert_eq!(p[5].len(), 14);
    }
}
