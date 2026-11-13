//! Corsair HXi / RMi digital power supplies ("Corsair Link" USB HID).
//!
//! Protocol: 64-byte HID reports carrying PMBus-style commands.
//!
//! * read:  `[0x03, cmd]`        -> reply `[0x03, cmd, d0, d1, ...]`
//! * write: `[0x02, cmd, data…]` -> reply `[0x02, cmd, ...]`
//! * init:  `[0xfe, 0x03]`
//!
//! Most values are PMBus LINEAR11. Per-rail values are selected with the
//! PMBus PAGE command (0x00). Input power and efficiency are *not* reported
//! by the hardware and are estimated from output power and input voltage
//! with the per-model efficiency curves measured by the liquidctl project.
//!
//! References: Linux `drivers/hwmon/corsair-psu.c`,
//! liquidctl `corsair_hid_psu.py`, and observation of Corsair iCUE / LINK.

use crate::linear11;
use crate::model::*;
use crate::transport::HidTransport;
use crate::{Device, Error, Result};

const HDR_READ: u8 = 0x03;
const HDR_WRITE: u8 = 0x02;

mod cmd {
    pub const PAGE: u8 = 0x00;
    pub const FAN_COMMAND_1: u8 = 0x3b;
    pub const VOUT_OV_FAULT_LIMIT: u8 = 0x40;
    pub const VOUT_UV_FAULT_LIMIT: u8 = 0x44;
    pub const IOUT_OC_FAULT_LIMIT: u8 = 0x46;
    pub const OT_FAULT_LIMIT: u8 = 0x4f;
    pub const READ_VIN: u8 = 0x88;
    pub const READ_IIN: u8 = 0x89;
    pub const READ_VOUT: u8 = 0x8b;
    pub const READ_IOUT: u8 = 0x8c;
    pub const READ_TEMPERATURE_1: u8 = 0x8d;
    pub const READ_TEMPERATURE_2: u8 = 0x8e;
    pub const READ_FAN_SPEED_1: u8 = 0x90;
    pub const READ_POUT: u8 = 0x96;
    pub const MFR_ID: u8 = 0x99;
    pub const MFR_MODEL: u8 = 0x9a;
    pub const TOTAL_UPTIME: u8 = 0xd1;
    pub const UPTIME: u8 = 0xd2;
    pub const OCP_MODE: u8 = 0xd8;
    pub const TOTAL_POUT: u8 = 0xee;
    pub const FAN_CONTROL_MODE: u8 = 0xf0;
}

/// Lowest duty accepted in software fan mode (matches iCUE and liquidctl).
pub const MIN_FAN_DUTY: u8 = 30;

pub struct PsuModel {
    pub pid: u16,
    pub name: &'static str,
    pub slug: &'static str,
    /// Quadratic fits `a*x² + b*x + c` of input power vs output power, at 115 V and 230 V.
    pub fpowin115: (f64, f64, f64),
    pub fpowin230: (f64, f64, f64),
}

/// Efficiency curves from liquidctl (GPL-3.0-or-later), derived from
/// Corsair's published efficiency data. liquidctl has no curves for the
/// HX550i/HX650i or RM550i; they reuse the HX750i / RM650i curves, so input
/// power estimates for those models are approximate.
#[allow(clippy::excessive_precision)] // coefficients kept verbatim from the source
pub static MODELS: &[PsuModel] = &[
    PsuModel { pid: 0x1c03, name: "HX550i", slug: "hx550i",
        fpowin115: (0.00013153276902318052, 1.0118732314945875, 9.783796618886313),
        fpowin230: (9.268856467314546e-05, 1.0183515407387007, 8.279822175342481) },
    PsuModel { pid: 0x1c04, name: "HX650i", slug: "hx650i",
        fpowin115: (0.00013153276902318052, 1.0118732314945875, 9.783796618886313),
        fpowin230: (9.268856467314546e-05, 1.0183515407387007, 8.279822175342481) },
    PsuModel { pid: 0x1c05, name: "HX750i", slug: "hx750i",
        fpowin115: (0.00013153276902318052, 1.0118732314945875, 9.783796618886313),
        fpowin230: (9.268856467314546e-05, 1.0183515407387007, 8.279822175342481) },
    PsuModel { pid: 0x1c06, name: "HX850i", slug: "hx850i",
        fpowin115: (0.00011552923724840388, 1.0111311876704099, 12.015296651918918),
        fpowin230: (8.126644224872423e-05, 1.0176256272095185, 10.290640442373850) },
    PsuModel { pid: 0x1c07, name: "HX1000i", slug: "hx1000i",
        fpowin115: (9.48609754417109e-05, 1.0170509865269720, 11.619826520447452),
        fpowin230: (9.649987544008507e-05, 1.0018241767296636, 12.759957859756842) },
    PsuModel { pid: 0x1c08, name: "HX1200i", slug: "hx1200i",
        fpowin115: (6.244705156199815e-05, 1.0234738310580973, 15.293509559389241),
        fpowin230: (5.9413179794350966e-05, 1.0023670927127724, 15.886126793547152) },
    PsuModel { pid: 0x1c09, name: "RM550i", slug: "rm550i",
        fpowin115: (0.00017323493381072683, 1.0047044721686030, 12.376592422281606),
        fpowin230: (0.00012413136310310370, 1.0284317478987164, 9.465259079360674) },
    PsuModel { pid: 0x1c0a, name: "RM650i", slug: "rm650i",
        fpowin115: (0.00017323493381072683, 1.0047044721686030, 12.376592422281606),
        fpowin230: (0.00012413136310310370, 1.0284317478987164, 9.465259079360674) },
    PsuModel { pid: 0x1c0b, name: "RM750i", slug: "rm750i",
        fpowin115: (0.00015013694263596336, 1.0047044721686027, 14.280683564171110),
        fpowin230: (0.00010460621468919797, 1.0173089573727216, 11.495900706372142) },
    PsuModel { pid: 0x1c0c, name: "RM850i", slug: "rm850i",
        fpowin115: (0.00012280002467981107, 1.0159421430340847, 13.555472968718759),
        fpowin230: (8.816054254801031e-05, 1.0234738318592156, 10.832902491655597) },
    PsuModel { pid: 0x1c0d, name: "RM1000i", slug: "rm1000i",
        fpowin115: (0.00010018433053123574, 1.0272313660072225, 14.092187353321624),
        fpowin230: (8.600634771656125e-05, 1.0289245073649413, 13.701515390258626) },
    PsuModel { pid: 0x1c1e, name: "HX1000i (2023)", slug: "hx1000i",
        fpowin115: (0.00012038623467957958, 0.9899868099948035, 13.125601514017152),
        fpowin230: (8.725695209710315e-05, 1.0017598021499974, 9.789546063300154) },
    PsuModel { pid: 0x1c1f, name: "HX1500i", slug: "hx1500i",
        fpowin115: (6.605968230747892e-05, 1.0125991461405333, 17.96728350708451),
        fpowin230: (4.634428233657273e-05, 1.0183515407387007, 16.559644350684962) },
    PsuModel { pid: 0x1c23, name: "HX1200i (2023)", slug: "hx1200i",
        fpowin115: (9.930197967499293e-05, 1.003634953854399, 13.956713659543981),
        fpowin230: (4.716701557627399e-05, 1.031689131040792, 8.562560345390088) },
    PsuModel { pid: 0x1c27, name: "HX1200i (2025)", slug: "hx1200i",
        fpowin115: (8.701178559061476e-05, 1.0119502460041445, 12.725770701505295),
        fpowin230: (3.4692421780176756e-05, 1.0391630676290817, 7.429785098514605) },
];

pub fn model_for_pid(pid: u16) -> Option<&'static PsuModel> {
    MODELS.iter().find(|m| m.pid == pid)
}

impl PsuModel {
    /// Estimated AC input power for a given DC output power and mains voltage.
    pub fn estimate_input_power(&self, output_w: f64, input_v: f64) -> f64 {
        let q = |(a, b, c): (f64, f64, f64)| a * output_w * output_w + b * output_w + c;
        let p115 = q(self.fpowin115);
        let p230 = q(self.fpowin230);
        let v = input_v.clamp(90.0, 264.0);
        p115 + (p230 - p115) * (v - 115.0) / 115.0
    }
}

const RAILS: [(u8, &str, &str); 3] = [(0, "12v", "+12V"), (1, "5v", "+5V"), (2, "3v3", "+3.3V")];

pub const CTRL_OCP: &str = "ocp_mode";
pub const OCP_SINGLE: &str = "Single rail";
pub const OCP_MULTI: &str = "Multi rail";
pub const FAN_CHANNEL: &str = "fan";

pub struct HxiPsu {
    io: HidTransport,
    info: DeviceInfo,
    model: &'static PsuModel,
    has_input_current: bool,
    limits: Vec<Limit>,
    software_fan: bool,
}

impl HxiPsu {
    pub fn open(dev: hidapi::HidDevice, model: &'static PsuModel, id: String, serial: Option<String>) -> Result<Self> {
        let io = HidTransport::new(dev);
        // Handshake used by both iCUE and liquidctl. Some firmware does not
        // echo it, so a missing reply is not fatal.
        if let Err(e) = io.transact(&[0xfe, 0x03], 2) {
            log::debug!("init handshake: {e}");
        }
        let mut psu = Self {
            io,
            info: DeviceInfo {
                id,
                kind: DeviceKind::PowerSupply,
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
            has_input_current: false,
            limits: Vec::new(),
            software_fan: false,
        };
        psu.info.vendor_string = psu.read_string(cmd::MFR_ID).ok();
        psu.info.product_string = psu.read_string(cmd::MFR_MODEL).ok();
        // Must answer at least one real register to count as alive.
        psu.read_linear(cmd::READ_VIN)?;
        psu.has_input_current = psu.read_linear(cmd::READ_IIN).map(|v| v > 0.0 && v < 30.0).unwrap_or(false);
        psu.software_fan = psu.read_byte(cmd::FAN_CONTROL_MODE).map(|b| b == 1).unwrap_or(false);
        psu.read_limits();
        Ok(psu)
    }

    fn read(&self, c: u8) -> Result<[u8; 64]> {
        self.io.transact(&[HDR_READ, c], 2)
    }

    fn write(&self, c: u8, data: &[u8]) -> Result<()> {
        let mut p = vec![HDR_WRITE, c];
        p.extend_from_slice(data);
        self.io.transact(&p, 2).map(|_| ())
    }

    fn read_linear(&self, c: u8) -> Result<f64> {
        let r = self.read(c)?;
        Ok(linear11::decode(u16::from_le_bytes([r[2], r[3]])))
    }

    fn read_byte(&self, c: u8) -> Result<u8> {
        Ok(self.read(c)?[2])
    }

    fn read_u32(&self, c: u8) -> Result<u32> {
        let r = self.read(c)?;
        Ok(u32::from_le_bytes([r[2], r[3], r[4], r[5]]))
    }

    fn read_string(&self, c: u8) -> Result<String> {
        let r = self.read(c)?;
        let s: String = r[2..]
            .iter()
            .take_while(|&&b| b != 0)
            .filter(|b| b.is_ascii_graphic() || **b == b' ')
            .map(|&b| b as char)
            .collect();
        Ok(s.trim().to_string())
    }

    fn select_rail(&self, rail: u8) -> Result<()> {
        self.write(cmd::PAGE, &[rail])
    }

    /// Read (voltage, current, power) of one rail.
    ///
    /// PAGE is shared state inside the PSU: if another program (iCUE, LINK)
    /// selects a different rail between our PAGE write and our reads, we
    /// would silently label e.g. +3.3V data as +5V. Guard against that by
    /// reading PAGE back after the reads and checking the voltage is
    /// plausible for the rail, retrying otherwise.
    fn read_rail(&self, page: u8) -> Result<(f64, f64, f64)> {
        let nominal = [12.0, 5.0, 3.3][page as usize];
        let mut last = None;
        for attempt in 0..6u64 {
            if attempt > 0 {
                // iCUE polls all rails in a ~100 ms burst; wait it out.
                std::thread::sleep(std::time::Duration::from_millis(40 * attempt));
            }
            self.select_rail(page)?;
            let v = self.read_linear(cmd::READ_VOUT)?;
            let a = self.read_linear(cmd::READ_IOUT)?;
            let w = self.read_linear(cmd::READ_POUT)?;
            let page_ok = match self.read_byte(cmd::PAGE) {
                Ok(p) => p == page,
                Err(_) => true, // firmware without PAGE readback
            };
            let plausible = (v - nominal).abs() <= nominal * 0.2;
            if page_ok && plausible {
                return Ok((v, a, w));
            }
            log::debug!("rail {page}: inconsistent read (page_ok={page_ok}, {v:.3} V), retry {attempt}");
            last = Some((v, a, w));
        }
        // Still inconsistent: report what we have rather than nothing; a
        // genuinely out-of-range voltage must remain visible.
        Ok(last.unwrap())
    }

    fn read_limits(&mut self) {
        let mut l = Vec::new();
        if let Ok(v) = self.read_linear(cmd::OT_FAULT_LIMIT) {
            l.push(Limit { name: "Over-temperature limit".into(), kind: SensorKind::Temperature, value: v });
        }
        for (page, _, label) in RAILS {
            // Same PAGE race as in `read_rail`: verify the page afterwards.
            for attempt in 0..6u64 {
                if attempt > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(40 * attempt));
                }
                if self.select_rail(page).is_err() {
                    continue;
                }
                let ov = self.read_linear(cmd::VOUT_OV_FAULT_LIMIT).ok();
                let uv = self.read_linear(cmd::VOUT_UV_FAULT_LIMIT).ok();
                let oc = self.read_linear(cmd::IOUT_OC_FAULT_LIMIT).ok();
                if self.read_byte(cmd::PAGE).map(|p| p != page).unwrap_or(false) {
                    continue;
                }
                if let Some(v) = ov {
                    l.push(Limit { name: format!("{label} over-voltage limit"), kind: SensorKind::Voltage, value: v });
                }
                if let Some(v) = uv {
                    l.push(Limit { name: format!("{label} under-voltage limit"), kind: SensorKind::Voltage, value: v });
                }
                if let Some(v) = oc {
                    l.push(Limit { name: format!("{label} over-current limit"), kind: SensorKind::Current, value: v });
                }
                break;
            }
        }
        let _ = self.select_rail(0);
        self.limits = l;
    }

    pub fn model(&self) -> &'static PsuModel {
        self.model
    }
}

impl Device for HxiPsu {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        let mut s = Vec::with_capacity(24);

        let vin = self.read_linear(cmd::READ_VIN)?;
        s.push(Sensor::new("input_voltage", "Input voltage", SensorKind::Voltage, vin));
        if self.has_input_current {
            if let Ok(iin) = self.read_linear(cmd::READ_IIN) {
                s.push(Sensor::new("input_current", "Input current", SensorKind::Current, iin));
            }
        }

        let mut rails_sum = 0.0;
        for (page, key, label) in RAILS {
            let (v, a, w) = self.read_rail(page)?;
            rails_sum += w;
            s.push(Sensor::new(&format!("rail_{key}_voltage"), &format!("{label} voltage"), SensorKind::Voltage, v));
            s.push(Sensor::new(&format!("rail_{key}_current"), &format!("{label} current"), SensorKind::Current, a));
            s.push(Sensor::new(&format!("rail_{key}_power"), &format!("{label} power"), SensorKind::Power, w));
        }
        self.select_rail(0)?;

        // Total output power register; fall back to the rail sum if absent.
        let pout = self.read_linear(cmd::TOTAL_POUT).unwrap_or(rails_sum);
        let pin = self.model.estimate_input_power(pout, vin).max(pout);
        let eff = if pin > 0.0 { (pout / pin * 100.0).clamp(0.0, 100.0) } else { 0.0 };
        s.push(Sensor::new("output_power", "Output power", SensorKind::Power, pout));
        s.push(Sensor::new("input_power", "Input power", SensorKind::Power, pin).estimated());
        s.push(Sensor::new("efficiency", "Efficiency", SensorKind::Efficiency, eff).estimated());

        s.push(Sensor::new("temp_vrm", "VRM temperature", SensorKind::Temperature, self.read_linear(cmd::READ_TEMPERATURE_1)?));
        s.push(Sensor::new("temp_case", "Case temperature", SensorKind::Temperature, self.read_linear(cmd::READ_TEMPERATURE_2)?));
        s.push(Sensor::new("fan_speed", "Fan speed", SensorKind::FanSpeed, self.read_linear(cmd::READ_FAN_SPEED_1)?));
        if let Ok(d) = self.read_linear(cmd::FAN_COMMAND_1) {
            s.push(Sensor::new("fan_duty", "Fan duty", SensorKind::Duty, d));
        }
        if let Ok(b) = self.read_byte(cmd::FAN_CONTROL_MODE) {
            self.software_fan = b == 1;
        }
        if let Ok(u) = self.read_u32(cmd::UPTIME) {
            s.push(Sensor::new("uptime", "Uptime", SensorKind::Duration, u as f64).diagnostic());
        }
        if let Ok(u) = self.read_u32(cmd::TOTAL_UPTIME) {
            s.push(Sensor::new("total_uptime", "Total uptime", SensorKind::Duration, u as f64).diagnostic());
        }

        let mut controls = Vec::new();
        if let Ok(b) = self.read_byte(cmd::OCP_MODE) {
            controls.push(Control {
                id: CTRL_OCP.into(),
                name: "+12V OCP mode".into(),
                kind: ControlKind::Choice { options: vec![OCP_SINGLE.into(), OCP_MULTI.into()] },
                value: ControlValue::Choice(if b == 2 { OCP_MULTI } else { OCP_SINGLE }.into()),
                help: Some(
                    "Multi rail: each +12V output connector trips at its own 40 A limit (safer). \
                     Single rail: one combined limit for the whole +12V rail."
                        .into(),
                ),
            });
        }

        Ok(Snapshot { sensors: s, controls })
    }

    fn limits(&self) -> &[Limit] {
        &self.limits
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        match (id, value) {
            (CTRL_OCP, ControlValue::Choice(c)) => {
                let mode = match c.as_str() {
                    OCP_SINGLE => 1,
                    OCP_MULTI => 2,
                    _ => return Err(Error::InvalidValue(id.into())),
                };
                self.write(cmd::OCP_MODE, &[mode])
            }
            (CTRL_OCP, _) => Err(Error::InvalidValue(id.into())),
            _ => Err(Error::UnknownControl(id.into())),
        }
    }

    fn fan_channels(&self) -> Vec<FanChannel> {
        vec![FanChannel {
            id: FAN_CHANNEL.into(),
            name: "PSU fan".into(),
            min_duty: MIN_FAN_DUTY,
            curve_sensor: "temp_vrm".into(),
        }]
    }

    fn set_fan(&mut self, _channel: &str, target: FanTarget) -> Result<()> {
        match target {
            FanTarget::Hardware => {
                if self.software_fan {
                    self.write(cmd::FAN_CONTROL_MODE, &[0])?;
                    self.software_fan = false;
                }
            }
            FanTarget::Duty(d) => {
                if !self.software_fan {
                    self.write(cmd::FAN_CONTROL_MODE, &[1])?;
                    self.software_fan = true;
                }
                self.write(cmd::FAN_COMMAND_1, &[d.clamp(MIN_FAN_DUTY, 100)])?;
            }
        }
        Ok(())
    }

    fn release(&mut self) -> Result<()> {
        // Never leave the PSU fan pinned at a fixed duty without us running.
        let r = self.write(cmd::FAN_CONTROL_MODE, &[0]);
        self.software_fan = false;
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hx850i_input_estimate_is_sane() {
        let m = model_for_pid(0x1c06).unwrap();
        let pin = m.estimate_input_power(400.0, 230.0);
        let eff = 400.0 / pin;
        assert!(eff > 0.88 && eff < 0.96, "efficiency {eff}");
        // Interpolation between curves.
        let p120 = m.estimate_input_power(400.0, 120.0);
        assert!(p120 > pin);
    }

    #[test]
    fn all_pids_unique() {
        let mut v: Vec<_> = MODELS.iter().map(|m| m.pid).collect();
        v.sort();
        v.dedup();
        assert_eq!(v.len(), MODELS.len());
    }
}
