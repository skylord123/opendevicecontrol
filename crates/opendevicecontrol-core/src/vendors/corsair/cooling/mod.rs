//! Corsair cooling devices: iCUE LINK System Hub, Commander Core / Core XT /
//! ST / DUO, Commander Pro, Lighting Node Pro / Core and Hydro Platinum /
//! Pro XT / Elite RGB AIO coolers.
//!
//! **Experimental**: none of these drivers has been tested on real hardware
//! by opendevicecontrol. They follow the byte layouts of liquidctl and OpenLinkHub
//! (both GPL-3.0-or-later, credited in each module).
//!
//! Devices behind a hub (LINK fans, LINK AIOs, fans and pumps on a
//! Commander) appear as channels, sensors and lighting zones of **one
//! opendevicecontrol device per hub**, named after what is attached. A hub is one USB
//! device with one command stream, one software/hardware mode and one
//! colour frame for everything behind it; splitting it into several
//! opendevicecontrol devices would need several workers sharing one HID handle.
//!
//! Fan safety: every driver clamps duties to the channel minimum (pumps
//! never below [`AIO_PUMP_MIN`] on AIOs), and `release()` hands control back
//! to the hardware (hub hardware mode, or a firmware-run default curve
//! where the device has no such mode).

pub mod commander;
pub mod cpro;
pub mod endpoint;
pub mod link;
pub mod platinum;

use crate::model::*;
use crate::Device;
use std::ffi::CString;

/// Lowest pump duty opendevicecontrol will ever command on an AIO.
pub const AIO_PUMP_MIN: u8 = 50;
/// Lowest duty for pumps in custom loops (LINK XD5/XD6).
pub const LOOP_PUMP_MIN: u8 = 30;
/// Lowest duty for fans on hubs (no zero-RPM from opendevicecontrol).
pub const FAN_MIN: u8 = 20;
/// Pump duty used while opendevicecontrol holds a hub in software mode and the pump
/// is set to "Hardware".
pub const DEFAULT_PUMP_DUTY: u8 = 75;

/// Coolant temperature (°C) → fan duty used in place of the firmware curve
/// while a hub is in software mode and a fan is set to "Hardware". Also
/// programmed into AIOs as their firmware fan profile.
pub const DEFAULT_FAN_CURVE: [(f64, u8); 5] = [(25.0, 30), (30.0, 40), (35.0, 60), (40.0, 85), (45.0, 100)];

/// Duty for fans without a coolant sensor in that situation.
pub const DEFAULT_FAN_DUTY: u8 = 50;

pub fn interpolate(points: &[(f64, u8)], t: f64) -> u8 {
    let Some(first) = points.first() else { return 100 };
    if t <= first.0 {
        return first.1;
    }
    for w in points.windows(2) {
        let ((t0, d0), (t1, d1)) = (w[0], w[1]);
        if t <= t1 {
            let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 1.0 };
            return (d0 as f64 + (d1 as f64 - d0 as f64) * f).round() as u8;
        }
    }
    points.last().map(|p| p.1).unwrap_or(100)
}

/// A fan or pump on a hub that opendevicecontrol drives with duty writes.
#[derive(Debug, Clone)]
pub struct HubChannel {
    /// Index the hub uses for this channel.
    pub hw: u8,
    pub id: String,
    pub name: String,
    pub pump: bool,
    pub min: u8,
    pub curve_sensor: String,
    pub target: FanTarget,
}

impl HubChannel {
    /// Duty to send: the requested one, or opendevicecontrol's stand-in for the
    /// firmware curve when the channel is set to `Hardware`.
    pub fn duty(&self, coolant: Option<f64>) -> u8 {
        let d = match self.target {
            FanTarget::Duty(d) => d,
            FanTarget::Hardware if self.pump => DEFAULT_PUMP_DUTY,
            FanTarget::Hardware => coolant.map(|t| interpolate(&DEFAULT_FAN_CURVE, t)).unwrap_or(DEFAULT_FAN_DUTY),
        };
        d.clamp(self.min, 100)
    }

    pub fn fan_channel(&self) -> FanChannel {
        FanChannel { id: self.id.clone(), name: self.name.clone(), min_duty: self.min, curve_sensor: self.curve_sensor.clone() }
    }
}

/// Flatten colours to `r, g, b` bytes, padding/truncating to `leds` LEDs.
pub fn rgb_bytes(colors: &[Rgb], leds: usize) -> Vec<u8> {
    (0..leds).map(|i| colors.get(i).copied().unwrap_or_default()).flat_map(|c| [c.r, c.g, c.b]).collect()
}

/// Pick one HID path per physical device: interface 0 (or the lowest
/// interface), preferring a vendor-defined usage page.
fn primary<'a>(list: &[&'a hidapi::DeviceInfo], pid: u16) -> Vec<&'a hidapi::DeviceInfo> {
    let mut picked: Vec<&hidapi::DeviceInfo> = Vec::new();
    for d in list.iter().filter(|d| d.product_id() == pid) {
        let same = picked.iter().position(|p| p.serial_number() == d.serial_number() && serial_ok(d));
        let better = |old: &hidapi::DeviceInfo| {
            let key = |x: &hidapi::DeviceInfo| (x.interface_number().max(0), x.usage_page() < 0xff00);
            key(d) < key(old)
        };
        match same {
            Some(i) if better(picked[i]) => picked[i] = d,
            Some(_) => {}
            None => picked.push(d),
        }
    }
    picked
}

/// Devices without a serial cannot be told apart; treat every path as its
/// own device only when serials are present.
fn serial_ok(d: &hidapi::DeviceInfo) -> bool {
    d.serial_number().is_some_and(|s| !s.trim().is_empty())
}

type Opened = Vec<(CString, Box<dyn Device>)>;

/// Discover cooling devices. Called from [`super::discover`].
pub(super) fn discover(api: &hidapi::HidApi, list: &[&hidapi::DeviceInfo], skip: &[CString], ids: &mut super::Ids, out: &mut Opened) {
    let serial = |d: &hidapi::DeviceInfo| d.serial_number().map(str::to_owned).filter(|s| !s.trim().is_empty());
    let mut pids: Vec<u16> = Vec::new();
    pids.extend(link::MODELS.iter().map(|m| m.pid));
    pids.extend(commander::MODELS.iter().map(|m| m.pid));
    pids.extend(cpro::MODELS.iter().map(|m| m.pid));
    pids.extend(platinum::MODELS.iter().map(|m| m.pid));

    for pid in pids {
        for d in primary(list, pid) {
            let (slug, name) = if let Some(m) = link::model_for_pid(pid) {
                (m.slug, m.name)
            } else if let Some(m) = commander::model_for_pid(pid) {
                (m.slug, m.name)
            } else if let Some(m) = cpro::model_for_pid(pid) {
                (m.slug, m.name)
            } else if let Some(m) = platinum::model_for_pid(pid) {
                (m.slug, m.name)
            } else {
                continue;
            };
            if super::taken(d.path(), skip, out) {
                continue;
            }
            let id = ids.next(slug);
            let dev = match api.open_path(d.path()) {
                Ok(h) => h,
                Err(e) => {
                    log::warn!("failed to open {name}: {e}");
                    continue;
                }
            };
            let opened: crate::Result<Box<dyn Device>> = if let Some(m) = link::model_for_pid(pid) {
                link::LinkHub::open(dev, m, id.clone(), serial(d)).map(|x| Box::new(x) as Box<dyn Device>)
            } else if let Some(m) = commander::model_for_pid(pid) {
                commander::Commander::open(dev, m, id.clone(), serial(d)).map(|x| Box::new(x) as Box<dyn Device>)
            } else if let Some(m) = cpro::model_for_pid(pid) {
                cpro::CommanderPro::open(dev, m, id.clone(), serial(d)).map(|x| Box::new(x) as Box<dyn Device>)
            } else if let Some(m) = platinum::model_for_pid(pid) {
                platinum::HydroPlatinum::open(dev, m, id.clone(), serial(d)).map(|x| Box::new(x) as Box<dyn Device>)
            } else {
                continue;
            };
            match opened {
                Ok(dev) => {
                    log::info!(
                        "opened {name} ({id}, experimental), firmware {}",
                        dev.info().firmware.as_deref().unwrap_or("?")
                    );
                    out.push((d.path().to_owned(), dev));
                }
                Err(e) => log::warn!("failed to open {name}: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_curve_and_pump_floor() {
        assert_eq!(interpolate(&DEFAULT_FAN_CURVE, 20.0), 30);
        assert_eq!(interpolate(&DEFAULT_FAN_CURVE, 32.5), 50);
        assert_eq!(interpolate(&DEFAULT_FAN_CURVE, 60.0), 100);
        let mut ch = HubChannel {
            hw: 0,
            id: "pump".into(),
            name: "Pump".into(),
            pump: true,
            min: AIO_PUMP_MIN,
            curve_sensor: String::new(),
            target: FanTarget::Duty(0),
        };
        assert_eq!(ch.duty(None), AIO_PUMP_MIN, "a pump is never stopped");
        ch.target = FanTarget::Hardware;
        assert_eq!(ch.duty(Some(50.0)), DEFAULT_PUMP_DUTY);
        ch.pump = false;
        ch.min = FAN_MIN;
        assert_eq!(ch.duty(None), DEFAULT_FAN_DUTY);
        assert_eq!(ch.duty(Some(40.0)), 85);
    }

    #[test]
    fn rgb_bytes_pads_and_truncates() {
        let c = [Rgb::new(1, 2, 3)];
        assert_eq!(rgb_bytes(&c, 2), vec![1, 2, 3, 0, 0, 0]);
        assert_eq!(rgb_bytes(&[Rgb::WHITE, Rgb::WHITE], 1).len(), 3);
    }
}
