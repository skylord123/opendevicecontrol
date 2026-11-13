//! Corsair Hydro H100i/H115i Platinum (and Platinum SE), H60i–H150i Pro XT
//! and iCUE H100i–H150i Elite RGB all-in-one liquid coolers.
//!
//! **Experimental, untested on hardware.** Protocol from liquidctl
//! `hydro_platinum.py` (Jonas Malaco and contributors, GPL-3.0-or-later):
//! 64-byte reports `3f <seq<<3 | feature> <command> <data…> <crc-8>`.
//! The cooler has no host-independent mode switch; every fan setting is a
//! firmware setting (fixed duty or a 7-point coolant-temperature profile),
//! and the pump runs one of three firmware modes (quiet / balanced /
//! extreme). `Hardware` and `release()` program a default coolant profile
//! and keep the selected pump mode, so the cooler keeps regulating itself
//! after opendevicecontrol exits.

use super::{interpolate, DEFAULT_FAN_CURVE, FAN_MIN};
use crate::model::*;
use crate::{Device, Error, Result};

pub struct PlatinumModel {
    pub pid: u16,
    pub name: &'static str,
    pub slug: &'static str,
    pub fans: usize,
    /// LEDs per fan (fans that plug into the pump's RGB chain).
    pub fan_leds: usize,
}

const fn m(pid: u16, name: &'static str, slug: &'static str, fans: usize, fan_leds: usize) -> PlatinumModel {
    PlatinumModel { pid, name, slug, fans, fan_leds }
}

pub static MODELS: &[PlatinumModel] = &[
    m(0x0c18, "H100i Platinum", "h100i-platinum", 2, 4),
    m(0x0c19, "H100i Platinum SE", "h100i-platinum-se", 2, 16),
    m(0x0c17, "H115i Platinum", "h115i-platinum", 2, 4),
    m(0x0c29, "H60i Pro XT", "h60i-pro-xt", 2, 0),
    m(0x0c20, "H100i Pro XT", "h100i-pro-xt", 2, 0),
    m(0x0c2d, "H100i RGB Pro XT", "h100i-pro-xt", 2, 0),
    m(0x0c21, "H115i Pro XT", "h115i-pro-xt", 2, 0),
    m(0x0c22, "H150i Pro XT", "h150i-pro-xt", 3, 0),
    m(0x0c35, "H100i Elite RGB", "h100i-elite-rgb", 2, 0),
    m(0x0c36, "H115i Elite RGB", "h115i-elite-rgb", 2, 0),
    m(0x0c37, "H150i Elite RGB", "h150i-elite-rgb", 3, 0),
    m(0x0c40, "H100i Elite RGB White", "h100i-elite-rgb", 2, 0),
    m(0x0c41, "H150i Elite RGB White", "h150i-elite-rgb", 3, 0),
];

pub fn model_for_pid(pid: u16) -> Option<&'static PlatinumModel> {
    MODELS.iter().find(|m| m.pid == pid)
}

const PREFIX: u8 = 0x3f;
const FEATURE_COOLING: u8 = 0b000;
const FEATURE_COOLING2: u8 = 0b011;
const CMD_GET_STATUS: u8 = 0xff;
const CMD_SET_COOLING: u8 = 0x14;
const CMD_LIGHTING: [u8; 3] = [0b100, 0b101, 0b110];
const PUMP_LEDS: usize = 16;

const COOLING_PREFIX: [u8; 8] = [0x00, 0xff, 0x05, 0xff, 0xff, 0xff, 0xff, 0xff];
const FAN_MODE_OFFSETS: [usize; 2] = [0x0b - 3, 0x11 - 3];
const FAN_DUTY_OFFSETS: [usize; 2] = [0x0b - 3 + 5, 0x11 - 3 + 5];
const FAN_PROFILE_OFFSETS: [usize; 2] = [0x1e - 3, 0x2c - 3];
const PUMP_MODE_OFFSET: usize = 0x17 - 3;
const PROFILE_LEN_OFFSET: usize = 0x1d - 3;
const PROFILE_LEN: usize = 7;
const CRITICAL_TEMP: u8 = 60;

const MODE_PROFILE: u8 = 0x0;
const MODE_FIXED_DUTY: u8 = 0x2;

pub const PUMP_MODES: [&str; 3] = ["Quiet", "Balanced", "Extreme"];
pub const CTRL_PUMP: &str = "pump_mode";

/// CRC-8 (poly 0x07, init 0, not reflected), as liquidctl's `crc-8`.
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

fn fraction(duty: u8) -> u8 {
    ((duty.min(100) as f64) / 100.0 * 255.0).round() as u8
}

/// Build an output report (with leading report id 0). `feature` `None`
/// puts the command in the low header bits (lighting).
pub fn encode(seq: u8, feature: Option<u8>, command: u8, data: &[u8]) -> [u8; 65] {
    let mut b = [0u8; 65];
    b[1] = PREFIX;
    b[2] = seq << 3;
    let start = match feature {
        Some(f) => {
            b[2] |= f;
            b[3] = command;
            4
        }
        None => {
            b[2] |= command;
            3
        }
    };
    let n = data.len().min(64 - start);
    b[start..start + n].copy_from_slice(&data[..n]);
    b[64] = crc8(&b[2..64]);
    b
}

#[derive(Debug, Clone, PartialEq)]
pub enum FanSetting {
    Fixed(u8),
    /// (°C, duty %) points.
    Profile(Vec<(u8, u8)>),
}

/// Seven profile points, sorted, padded with (60 °C, 100 %).
pub fn normalize_profile(points: &[(u8, u8)]) -> Vec<(u8, u8)> {
    let mut p: Vec<(u8, u8)> = points.iter().map(|&(t, d)| (t, d.min(100))).collect();
    p.sort_by_key(|x| x.0);
    p.truncate(PROFILE_LEN - 1);
    while p.len() < PROFILE_LEN {
        p.push((CRITICAL_TEMP, 100));
    }
    p
}

/// SET_COOLING data for up to two fans and the pump mode (`0xff` leaves it).
pub fn cooling_data(fans: &[FanSetting], pump_mode: u8) -> Vec<u8> {
    let mut d = vec![0u8; 60];
    d[..COOLING_PREFIX.len()].copy_from_slice(&COOLING_PREFIX);
    d[PROFILE_LEN_OFFSET] = PROFILE_LEN as u8;
    for (i, f) in fans.iter().take(2).enumerate() {
        match f {
            FanSetting::Fixed(duty) => {
                d[FAN_MODE_OFFSETS[i]] = MODE_FIXED_DUTY;
                d[FAN_DUTY_OFFSETS[i]] = fraction(*duty);
            }
            FanSetting::Profile(points) => {
                d[FAN_MODE_OFFSETS[i]] = MODE_PROFILE;
                for (j, (t, duty)) in normalize_profile(points).into_iter().enumerate() {
                    d[FAN_PROFILE_OFFSETS[i] + j * 2] = t;
                    d[FAN_PROFILE_OFFSETS[i] + j * 2 + 1] = fraction(duty);
                }
            }
        }
    }
    d[PUMP_MODE_OFFSET] = pump_mode;
    d
}

/// Decoded status reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub coolant: f64,
    /// (rpm, duty %) per fan, then the pump last.
    pub fans: Vec<(u16, u8)>,
    pub pump: (u16, u8),
}

pub fn parse_status(r: &[u8], fans: usize) -> Option<Status> {
    if r.len() < 48 {
        return None;
    }
    let at = |base: usize| (u16::from_le_bytes([r[base + 1], r[base + 2]]), (r[base] as f64 / 255.0 * 100.0).round() as u8);
    Some(Status {
        coolant: r[8] as f64 + r[7] as f64 / 255.0,
        fans: [14, 21, 42].iter().take(fans).map(|&b| at(b)).collect(),
        pump: at(28),
    })
}

fn default_profile() -> Vec<(u8, u8)> {
    DEFAULT_FAN_CURVE.iter().map(|&(t, d)| (t as u8, d)).collect()
}

pub struct HydroPlatinum {
    dev: hidapi::HidDevice,
    info: DeviceInfo,
    model: &'static PlatinumModel,
    seq: u8,
    fans: Vec<FanSetting>,
    pump_mode: u8,
    leds_ready: bool,
}

impl HydroPlatinum {
    pub fn open(dev: hidapi::HidDevice, model: &'static PlatinumModel, id: String, serial: Option<String>) -> Result<Self> {
        let mut h = Self {
            dev,
            info: DeviceInfo {
                id,
                kind: DeviceKind::Cooler,
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
            seq: 0,
            fans: vec![FanSetting::Profile(default_profile()); model.fans],
            pump_mode: 1,
            leds_ready: false,
        };
        let r = h.apply_cooling()?;
        h.info.firmware = Some(format!("{}.{}.{}", r[2] >> 4, r[2] & 0xf, r[3]));
        Ok(h)
    }

    fn next_seq(&mut self) -> u8 {
        self.seq = self.seq % 31 + 1;
        self.seq
    }

    fn send(&mut self, feature: Option<u8>, command: u8, data: &[u8]) -> Result<Vec<u8>> {
        let seq = self.next_seq();
        let out = encode(seq, feature, command, data);
        let mut junk = [0u8; 64];
        while matches!(self.dev.read_timeout(&mut junk, 0), Ok(n) if n > 0) {}
        self.dev.write(&out)?;
        let mut buf = [0u8; 64];
        let n = self.dev.read_timeout(&mut buf, 500)?;
        if n == 0 {
            return Err(Error::NoReply { cmd: command });
        }
        if crc8(&buf[1..n]) != 0 {
            log::debug!("{}: reply checksum mismatch", self.model.name);
        }
        Ok(buf[..n.max(48)].to_vec())
    }

    fn apply_cooling(&mut self) -> Result<Vec<u8>> {
        if self.fans.len() > 2 {
            let d2 = cooling_data(&self.fans[2..], 0xff);
            self.send(Some(FEATURE_COOLING2), CMD_SET_COOLING, &d2)?;
        }
        let d = cooling_data(&self.fans[..self.fans.len().min(2)], self.pump_mode);
        self.send(Some(FEATURE_COOLING), CMD_SET_COOLING, &d)
    }

    fn led_count(&self) -> usize {
        PUMP_LEDS + self.model.fans * self.model.fan_leds
    }
}

impl Device for HydroPlatinum {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        let r = self.send(Some(FEATURE_COOLING), CMD_GET_STATUS, &[])?;
        let st = parse_status(&r, self.model.fans).ok_or(Error::Unsupported { cmd: CMD_GET_STATUS })?;
        let mut s = vec![Sensor::new("coolant_temp", "Coolant temperature", SensorKind::Temperature, st.coolant)];
        for (i, (rpm, duty)) in st.fans.iter().enumerate() {
            s.push(Sensor::new(&format!("fan{}_speed", i + 1), &format!("Fan {} speed", i + 1), SensorKind::FanSpeed, *rpm as f64));
            s.push(Sensor::new(&format!("fan{}_duty", i + 1), &format!("Fan {} duty", i + 1), SensorKind::Duty, *duty as f64));
        }
        s.push(Sensor::new("pump_speed", "Pump speed", SensorKind::FanSpeed, st.pump.0 as f64));
        s.push(Sensor::new("pump_duty", "Pump duty", SensorKind::Duty, st.pump.1 as f64));
        let controls = vec![Control {
            id: CTRL_PUMP.into(),
            name: "Pump mode".into(),
            kind: ControlKind::Choice { options: PUMP_MODES.iter().map(|s| s.to_string()).collect() },
            value: ControlValue::Choice(PUMP_MODES[self.pump_mode.min(2) as usize].into()),
            help: Some("Pump speed chosen by the cooler's firmware. Balanced is the default.".into()),
        }];
        Ok(Snapshot { sensors: s, controls })
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        match (id, value) {
            (CTRL_PUMP, ControlValue::Choice(c)) => {
                let m = PUMP_MODES.iter().position(|x| x == c).ok_or_else(|| Error::InvalidValue(id.into()))?;
                self.pump_mode = m as u8;
                self.apply_cooling().map(|_| ())
            }
            (CTRL_PUMP, _) => Err(Error::InvalidValue(id.into())),
            _ => Err(Error::UnknownControl(id.into())),
        }
    }

    fn remembered_controls(&self) -> Vec<String> {
        vec![CTRL_PUMP.into()]
    }

    fn fan_channels(&self) -> Vec<FanChannel> {
        (0..self.model.fans)
            .map(|i| FanChannel { id: format!("fan{}", i + 1), name: format!("Fan {}", i + 1), min_duty: FAN_MIN, curve_sensor: "coolant_temp".into() })
            .collect()
    }

    fn set_fan(&mut self, channel: &str, target: FanTarget) -> Result<()> {
        let i = (0..self.model.fans).find(|i| format!("fan{}", i + 1) == channel).ok_or_else(|| Error::UnknownControl(channel.into()))?;
        let want = match target {
            FanTarget::Hardware => FanSetting::Profile(default_profile()),
            FanTarget::Duty(d) => FanSetting::Fixed(d.clamp(FAN_MIN, 100)),
        };
        self.fans[i] = want;
        self.apply_cooling().map(|_| ())
    }

    fn light_zones(&self) -> Vec<LightZone> {
        let mut z = vec![LightZone { id: "pump".into(), name: "Pump".into(), leds: PUMP_LEDS }];
        if self.model.fan_leds > 0 {
            for i in 0..self.model.fans {
                z.push(LightZone { id: format!("fan{}", i + 1), name: format!("Fan {}", i + 1), leds: self.model.fan_leds });
            }
        }
        z
    }

    fn set_lights(&mut self, colors: &[Rgb]) -> Result<()> {
        if !self.leds_ready {
            // One-time LED setup, copied byte for byte from liquidctl.
            let init: [&str; 3] = [
                "0101ffffffffffffffffffffffffff7f7f7f7fff00ffffffff00ffffffff00ffffffff00ffffffff00ffffffff00ffffffffffffffffffffffffffffff",
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f2021222324252627ffffffffffffffffffffffffffffffffffffffffff",
                "28292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4fffffffffffffffffffffffffffffffffffffffffff",
            ];
            for (i, hex) in init.iter().enumerate() {
                let data: Vec<u8> = (0..hex.len()).step_by(2).map(|j| u8::from_str_radix(&hex[j..j + 2], 16).unwrap_or(0)).collect();
                self.send(None, (i + 1) as u8, &data)?;
            }
            self.leds_ready = true;
        }
        let n = self.led_count();
        // Blue, green, red order; 20 LEDs per packet.
        let bgr: Vec<u8> = (0..n).map(|i| colors.get(i).copied().unwrap_or_default()).flat_map(|c| [c.b, c.g, c.r]).collect();
        for (k, chunk) in bgr.chunks(60).enumerate().take(3) {
            self.send(None, CMD_LIGHTING[k], chunk)?;
        }
        Ok(())
    }

    fn release(&mut self) {
        for f in self.fans.iter_mut() {
            *f = FanSetting::Profile(default_profile());
        }
        if let Err(e) = self.apply_cooling() {
            log::warn!("{}: could not restore the default fan profile: {e}", self.model.name);
        }
    }
}

/// Expected fan duty from the default profile (for display/tests).
pub fn default_duty(coolant: f64) -> u8 {
    interpolate(&DEFAULT_FAN_CURVE, coolant)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_crc8_check_value() {
        // CRC-8 (SMBus) check value for "123456789".
        assert_eq!(crc8(b"123456789"), 0xf4);
    }

    #[test]
    fn frame_layout() {
        let f = encode(1, Some(FEATURE_COOLING), CMD_GET_STATUS, &[]);
        assert_eq!(&f[..4], &[0x00, 0x3f, 0x08, 0xff]);
        assert_eq!(crc8(&f[2..65]), 0, "trailing CRC makes the frame check to zero");
        let f = encode(31, None, CMD_LIGHTING[1], &[1, 2, 3]);
        assert_eq!(&f[1..6], &[0x3f, 0xfd, 1, 2, 3]);
    }

    #[test]
    fn cooling_payload_offsets() {
        let d = cooling_data(&[FanSetting::Fixed(50), FanSetting::Profile(vec![(30, 40), (20, 20)])], 2);
        assert_eq!(&d[..8], &COOLING_PREFIX);
        assert_eq!(d[8], MODE_FIXED_DUTY); // 0x0b - 3
        assert_eq!(d[13], 128); // 50 % of 255
        assert_eq!(d[14], MODE_PROFILE); // 0x11 - 3
        assert_eq!(d[20], 2); // pump mode
        assert_eq!(d[26], 7); // profile length
        assert_eq!(&d[41..45], &[20, 51, 30, 102]); // sorted, scaled
        assert_eq!(&d[53..55], &[60, 255]); // padded
    }

    #[test]
    fn status_decoding() {
        let mut r = vec![0u8; 64];
        r[7] = 128;
        r[8] = 31;
        r[14] = 255;
        r[15..17].copy_from_slice(&1200u16.to_le_bytes());
        r[28] = 128;
        r[29..31].copy_from_slice(&2400u16.to_le_bytes());
        let s = parse_status(&r, 2).unwrap();
        assert!((s.coolant - 31.502).abs() < 0.01);
        assert_eq!(s.fans[0], (1200, 100));
        assert_eq!(s.pump, (2400, 50));
        assert_eq!(default_duty(30.0), 40);
    }
}
