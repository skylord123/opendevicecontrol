//! Corsair iCUE LINK System Hub and the LINK devices behind it (QX/LX/RX
//! fans, H-series LINK and TITAN AIOs, XD5/XD6 pumps, XC7 blocks, XG3/XG7,
//! LINK adapters, Commander DUO on LINK, LINK PSUs).
//!
//! **Experimental, untested on hardware.** Uses the endpoint protocol (see
//! [`super::endpoint`]) with prefix `00 01` and 512-byte reports. The hub
//! reports every attached device with a type/model byte pair; the device
//! table below (names, LED counts, pump/probe flags) is OpenLinkHub's
//! `database/external/lsh.json`, and the protocol follows OpenLinkHub
//! `src/devices/lsh` (Nikola Jurkovic, GPL-3.0-or-later) and OpenRGB's iCUE
//! LINK controller.
//!
//! One opendevicecontrol device represents the hub; each attached device becomes a
//! fan/pump channel, sensors and a lighting zone named after it.
//!
//! Not supported yet: LCD screens (separate USB device), LINK adapter LED
//! strips (their LED counts are user-configured in iCUE), LINK PSU rails.

use super::endpoint::{self, dtype, ep, EndpointIo};
use super::{rgb_bytes, HubChannel, AIO_PUMP_MIN, FAN_MIN, LOOP_PUMP_MIN};
use crate::model::*;
use crate::{Device, Error, Result};
use std::time::{Duration, Instant};

pub struct LinkModel {
    pub pid: u16,
    pub name: &'static str,
    pub slug: &'static str,
}

pub static MODELS: &[LinkModel] = &[LinkModel { pid: 0x0c3f, name: "iCUE LINK System Hub", slug: "link-hub" }];

pub fn model_for_pid(pid: u16) -> Option<&'static LinkModel> {
    MODELS.iter().find(|m| m.pid == pid)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Fan,
    Aio,
    LoopPump,
    Block,
    Adapter,
    Duo,
    Psu,
    Lcd,
}

/// A LINK device kind: (type, model) → name, LEDs, role, has a temperature
/// probe, has a speed channel.
pub struct LinkKind {
    pub dtype: u8,
    pub model: Option<u8>,
    pub name: &'static str,
    pub leds: usize,
    pub role: Role,
    pub probe: bool,
    pub speed: bool,
}

const fn k(dtype: u8, model: Option<u8>, name: &'static str, leds: usize, role: Role, probe: bool, speed: bool) -> LinkKind {
    LinkKind { dtype, model, name, leds, role, probe, speed }
}

use Role::*;
/// `model: None` matches any model of that type.
pub static KINDS: &[LinkKind] = &[
    k(1, None, "QX RGB", 34, Fan, true, true),
    k(2, None, "LX RGB", 18, Fan, false, true),
    k(3, None, "RX RGB MAX", 8, Fan, false, true),
    k(4, None, "RX MAX", 0, Fan, false, true),
    k(15, None, "RX RGB", 8, Fan, false, true),
    k(19, None, "RX", 0, Fan, false, true),
    k(16, None, "VRM cooler", 0, Fan, false, true),
    k(7, Some(0), "H100i", 20, Aio, false, true),
    k(7, Some(1), "H115i", 20, Aio, false, true),
    k(7, Some(2), "H150i", 20, Aio, false, true),
    k(7, Some(3), "H170i", 20, Aio, false, true),
    k(7, Some(4), "H100i", 20, Aio, false, true),
    k(7, Some(5), "H150i", 20, Aio, false, true),
    k(17, Some(0), "TITAN 240", 20, Aio, false, true),
    k(17, Some(1), "TITAN 280", 20, Aio, false, true),
    k(17, Some(2), "TITAN 360", 20, Aio, false, true),
    k(17, Some(3), "TITAN 420", 20, Aio, false, true),
    k(17, Some(4), "TITAN 240", 20, Aio, false, true),
    k(17, Some(5), "TITAN 360", 20, Aio, false, true),
    k(9, None, "XC7 ELITE", 24, Block, true, false),
    k(10, None, "XG3 HYBRID", 22, Fan, false, true),
    k(13, None, "XG7 RGB", 16, Block, false, false),
    k(12, None, "XD5 ELITE", 22, LoopPump, true, true),
    k(25, None, "XD6 ELITE", 22, LoopPump, true, true),
    k(5, None, "LINK adapter", 0, Adapter, false, false),
    k(27, None, "Commander DUO", 0, Duo, true, true),
    k(11, Some(0), "HX1000i", 0, Psu, true, true),
    k(11, Some(1), "HX1200i", 0, Psu, true, true),
    k(11, Some(2), "HX1500i", 0, Psu, true, true),
    k(26, Some(8), "LCD screen module", 32, Lcd, false, false),
    k(26, Some(1), "AIO pump", 20, Aio, false, true),
    k(26, Some(0), "LINK fan", 8, Fan, false, true),
];

/// LINK "v2" devices report their real name and LED count themselves.
const V2_TYPE: u8 = 26;
/// Extra LEDs of an AIO whose pump wears an LCD cap.
const LCD_CAP_LEDS: usize = 24;

pub fn kind(dtype: u8, model: u8) -> Option<&'static LinkKind> {
    KINDS
        .iter()
        .find(|k| k.dtype == dtype && k.model == Some(model))
        .or_else(|| KINDS.iter().find(|k| k.dtype == dtype && k.model.is_none()))
}

/// One entry of the hub's device table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub channel: u8,
    pub dtype: u8,
    pub model: u8,
    pub serial: String,
}

/// Parse a GET_DEVICES reply (normalised): `count`, then per channel 8
/// header bytes (`[2]` type, `[3]` model, `[7]` id length) and the id.
/// Returns the devices and whether an LCD pump cap is present.
pub fn parse_devices(r: &[u8]) -> (Vec<Entry>, bool) {
    let (count, data) = endpoint::reply_entries(r);
    let mut out = Vec::new();
    let mut lcd = false;
    let mut pos = 0;
    for ch in 1..=count {
        let Some(h) = data.get(pos..pos + 8) else { break };
        let len = h[7] as usize;
        if len == 0 {
            pos += 8;
            continue;
        }
        if h[2] == 6 || h[2] == 14 {
            lcd = true; // LINK cooler pump LCD / XD5 Elite LCD
        }
        let serial = data.get(pos + 8..pos + 8 + len).map(|s| String::from_utf8_lossy(s).trim_end_matches('\0').to_string());
        out.push(Entry { channel: ch as u8, dtype: h[2], model: h[3], serial: serial.unwrap_or_default() });
        pos += 8 + len;
    }
    (out, lcd)
}

/// Parse a v2 device model reply (normalised): payload length at `[6..8]`,
/// then `[field id, length, value…]` fields from `[8]`. Field 3 is the
/// name; field 12 has one byte per LED.
pub fn parse_v2_model(r: &[u8]) -> Option<(String, usize)> {
    let len = u16::from_le_bytes([*r.get(6)?, *r.get(7)?]) as usize;
    let end = (8 + len).min(r.len());
    let mut off = 8;
    let (mut name, mut leds) = (None, None);
    while off + 2 <= end {
        let (id, l) = (r[off], r[off + 1] as usize);
        off += 2;
        let v = r.get(off..off + l)?;
        off += l;
        match id {
            3 => name = Some(String::from_utf8_lossy(v).trim_end_matches('\0').to_string()),
            12 => leds = Some(l),
            _ => {}
        }
    }
    Some((name?, leds?))
}

struct Attached {
    channel: u8,
    kind: &'static LinkKind,
    name: String,
    leds: usize,
}

pub struct LinkHub {
    io: EndpointIo,
    info: DeviceInfo,
    attached: Vec<Attached>,
    channels: Vec<HubChannel>,
    coolant_channel: Option<u8>,
    coolant: Option<f64>,
    applied: Vec<(u8, u8)>,
    applied_at: Option<Instant>,
}

fn sid(ch: u8, what: &str) -> String {
    format!("ch{ch}_{what}")
}

impl LinkHub {
    pub fn open(dev: hidapi::HidDevice, model: &'static LinkModel, id: String, serial: Option<String>) -> Result<Self> {
        let io = EndpointIo::new(dev, &[0x00, 0x01], 512);
        let firmware = io.firmware().ok_or(Error::NoReply { cmd: endpoint::GET_FIRMWARE[0] })?;
        io.transfer(endpoint::SOFTWARE_MODE, &[])?;
        std::thread::sleep(Duration::from_millis(500));

        let (entries, lcd) = parse_devices(&io.read_endpoint(ep::GET_DEVICES, Some(dtype::DEVICES))?);
        let mut attached = Vec::new();
        for e in &entries {
            let Some(kind) = kind(e.dtype, e.model) else {
                log::info!("{}: unknown LINK device type {} model {} on channel {}", model.name, e.dtype, e.model, e.channel);
                continue;
            };
            let (mut name, mut leds) = (kind.name.to_string(), kind.leds);
            if e.dtype == V2_TYPE {
                if let Some((n, l)) = io.read_endpoint_addressed(ep::DEVICE_MODEL, e.channel).ok().and_then(|r| parse_v2_model(&r)) {
                    name = n.trim_start_matches("iCUE LINK ").to_string();
                    leds = l;
                }
            }
            if kind.role == Role::Aio && lcd {
                leds += LCD_CAP_LEDS;
                name.push_str(" LCD");
            }
            attached.push(Attached { channel: e.channel, kind, name: format!("{name} ({})", e.channel), leds });
        }

        // Curve input: AIO coolant, else a loop pump's or block's probe.
        let coolant_channel = [Role::Aio, Role::LoopPump, Role::Block]
            .iter()
            .find_map(|r| attached.iter().find(|a| a.kind.role == *r && (a.kind.probe || *r == Role::Aio)).map(|a| a.channel));
        let curve = coolant_channel.map(|c| sid(c, "temp")).unwrap_or_default();

        let channels: Vec<HubChannel> = attached
            .iter()
            .filter(|a| a.kind.speed && !matches!(a.kind.role, Role::Psu | Role::Lcd | Role::Adapter))
            .map(|a| {
                let pump = matches!(a.kind.role, Role::Aio | Role::LoopPump);
                let min = match a.kind.role {
                    Role::Aio if a.kind.dtype == 17 => 31, // TITAN pumps run lower
                    Role::Aio => AIO_PUMP_MIN,
                    Role::LoopPump => LOOP_PUMP_MIN,
                    _ => FAN_MIN,
                };
                let name = if pump { format!("{} pump", a.name) } else { a.name.clone() };
                HubChannel {
                    hw: a.channel,
                    id: format!("ch{}", a.channel),
                    name,
                    pump,
                    min,
                    curve_sensor: curve.clone(),
                    target: FanTarget::Hardware,
                }
            })
            .collect();

        let mut hub = Self {
            io,
            info: DeviceInfo {
                id,
                kind: DeviceKind::FanHub,
                manufacturer: "Corsair".into(),
                model: model.name.into(),
                vendor_string: None,
                product_string: None,
                usb_vid: crate::CORSAIR_VID,
                usb_pid: model.pid,
                serial,
                firmware: Some(firmware),
            },
            attached,
            channels,
            coolant_channel,
            coolant: None,
            applied: Vec::new(),
            applied_at: None,
        };
        if hub.total_leds() > 0 {
            hub.io.open_color_endpoint()?;
            let black = vec![0u8; hub.total_leds() * 3];
            let _ = hub.io.write_colors(&black);
            std::thread::sleep(Duration::from_millis(40));
        }
        hub.push_speeds(true)?;
        Ok(hub)
    }

    fn total_leds(&self) -> usize {
        self.attached.iter().map(|a| a.leds).sum()
    }

    fn push_speeds(&mut self, force: bool) -> Result<()> {
        if self.channels.is_empty() {
            return Ok(());
        }
        let d: Vec<(u8, u8)> = self.channels.iter().map(|c| (c.hw, c.duty(self.coolant))).collect();
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

impl Device for LinkHub {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        let speeds = endpoint::parse_speeds_with_status(&self.io.read_endpoint(ep::GET_SPEEDS, Some(dtype::SPEEDS))?);
        let temps = endpoint::parse_temps(&self.io.read_endpoint(ep::GET_TEMPS, Some(dtype::TEMPS))?);
        let mut s = Vec::new();
        for a in &self.attached {
            let i = a.channel as usize;
            if let Some(Some(t)) = temps.get(i) {
                let label = match a.kind.role {
                    Role::Aio | Role::LoopPump | Role::Block => format!("{} coolant temperature", a.name),
                    Role::Psu => format!("{} PSU temperature", a.name),
                    _ => format!("{} temperature", a.name),
                };
                s.push(Sensor::new(&sid(a.channel, "temp"), &label, SensorKind::Temperature, *t));
            }
            if a.kind.speed {
                if let Some(Some(rpm)) = speeds.get(i) {
                    let label = match a.kind.role {
                        Role::Aio | Role::LoopPump => format!("{} pump speed", a.name),
                        _ => format!("{} speed", a.name),
                    };
                    s.push(Sensor::new(&sid(a.channel, "speed"), &label, SensorKind::FanSpeed, *rpm as f64));
                }
            }
        }
        self.coolant = self.coolant_channel.and_then(|c| temps.get(c as usize).copied().flatten());
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
        self.attached
            .iter()
            .filter(|a| a.leds > 0)
            .map(|a| LightZone { id: format!("ch{}", a.channel), name: a.name.clone(), leds: a.leds })
            .collect()
    }

    fn set_lights(&mut self, colors: &[Rgb]) -> Result<()> {
        let total = self.total_leds();
        if total == 0 {
            return Ok(());
        }
        self.io.write_colors(&rgb_bytes(colors, total))
    }

    fn release(&mut self) {
        if let Err(e) = self.io.transfer(endpoint::HARDWARE_MODE, &[]) {
            log::warn!("LINK hub: could not return to hardware mode: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_device_table() {
        // echo, status, type 21 00, count 3; ch1 empty; ch2 QX (type 1); ch3 H150i (7/2).
        let mut r = vec![0x08, 0x00, 0x21, 0x00, 3];
        r.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        r.extend_from_slice(&[0, 0, 1, 0, 0, 0, 0, 4]);
        r.extend_from_slice(b"QX01");
        r.extend_from_slice(&[0, 0, 7, 2, 0, 0, 0, 3]);
        r.extend_from_slice(b"AIO");
        let (e, lcd) = parse_devices(&r);
        assert!(!lcd);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0], Entry { channel: 2, dtype: 1, model: 0, serial: "QX01".into() });
        assert_eq!((e[1].channel, e[1].dtype, e[1].model), (3, 7, 2));
        assert_eq!(kind(1, 0).unwrap().leds, 34);
        assert_eq!(kind(7, 2).unwrap().name, "H150i");
        assert_eq!(kind(7, 9).map(|k| k.name), None);
        assert_eq!(kind(17, 3).unwrap().role, Role::Aio);
    }

    #[test]
    fn lcd_entry_flags_cap() {
        let mut r = vec![0x08, 0x00, 0x21, 0x00, 1];
        r.extend_from_slice(&[0, 0, 6, 0, 0, 0, 0, 1, b'x']);
        assert!(parse_devices(&r).1);
    }

    #[test]
    fn parses_v2_model() {
        // 6 header bytes, payload length, then fields: 3 = name, 12 = one byte per LED.
        let mut r = vec![0x08, 0x00, 0, 0, 0, 0];
        let mut payload = vec![3, 8];
        payload.extend_from_slice(b"LINK FAN");
        payload.extend_from_slice(&[12, 8, 0, 0, 0, 0, 0, 0, 0, 0]);
        r.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        r.extend_from_slice(&payload);
        assert_eq!(parse_v2_model(&r), Some(("LINK FAN".to_string(), 8)));
        assert_eq!(parse_v2_model(&r[..9]), None);
    }
}
