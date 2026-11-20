//! Logitech receivers (Unifying, Bolt, Nano, Lightspeed): which devices are
//! paired in which slot, and pairing new ones.
//!
//! The receiver answers HID++ 1.0 register requests at device index 0xFF.
//! Long register 0x2B5 holds per-slot pairing information (sub-registers
//! 0x2N/0x3N/0x4N for Unifying-style receivers, 0x5N/0x6N for Bolt);
//! register 0xB2 opens the pairing lock, Bolt uses discovery (0xC0) and
//! an explicit pair command (0x2C1) instead.

use super::hidpp::{Channel, Report};
use super::link::{at, u16_at};
use crate::devices::Device;
use crate::model::*;
use crate::{Error, Result};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Unifying,
    Bolt,
    Nano,
    Lightspeed,
}

/// Known receivers (Solaar's `base_usb.py`): product id, kind, slots, and
/// whether devices can be unpaired.
pub fn kind(pid: u16) -> Option<(Kind, u8, bool)> {
    Some(match pid {
        0xC548 => (Kind::Bolt, 6, true),
        0xC52B | 0xC532 => (Kind::Unifying, 6, true),
        0xC52F | 0xC52E => (Kind::Nano, 1, false),
        0xC518 | 0xC51A | 0xC51B | 0xC521 | 0xC525 | 0xC526 | 0xC531 | 0xC535 | 0xC537 => (Kind::Nano, 1, false),
        0xC534 => (Kind::Nano, 2, false),
        0xC539 | 0xC53A | 0xC53D | 0xC53F | 0xC541 | 0xC545 | 0xC547 | 0xC54D => (Kind::Lightspeed, 2, false),
        _ => return None,
    })
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Unifying => "Unifying Receiver",
            Kind::Bolt => "Bolt Receiver",
            Kind::Nano => "Nano Receiver",
            Kind::Lightspeed => "Lightspeed Receiver",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Kind::Unifying => "logitech-unifying",
            Kind::Bolt => "logitech-bolt",
            Kind::Nano => "logitech-nano",
            Kind::Lightspeed => "logitech-lightspeed",
        }
    }
}

const RECEIVER_INFO: u16 = 0x2B5;
const PAIRING_LOCK: u16 = 0xB2;
const CONNECTIONS: u16 = 0x02;
const NOTIFICATIONS: u16 = 0x00;
const BOLT_DISCOVERY: u16 = 0xC0;
const BOLT_PAIRING: u16 = 0x2C1;
const BOLT_UNIQUE_ID: u16 = 0x2FB;
/// Wireless (connection) and software-present notification flags.
const NOTIFY_WIRELESS: [u8; 3] = [0x00, 0x09, 0x00];
const PAIRING_TIMEOUT: u8 = 30;

/// What the pairing registers say about one slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub slot: u8,
    pub wpid: u16,
    /// HID++ 1.0 device kind number.
    pub kind: u8,
    pub serial: Option<String>,
    pub codename: Option<String>,
}

/// Read slot `n` (1-based); `None` when it is empty.
pub fn slot(ch: &mut Channel, kind: Kind, n: u8) -> Option<Slot> {
    if kind == Kind::Bolt {
        let r = ch.read_register(RECEIVER_INFO, &[0x50 + n]).ok()?;
        let codename = ch.read_register(RECEIVER_INFO, &[0x60 + n, 0x01]).ok().map(|c| {
            let len = (at(&c, 2) as usize).min(14);
            String::from_utf8_lossy(c.get(3..3 + len).unwrap_or(&[])).to_string()
        });
        return Some(Slot {
            slot: n,
            wpid: u16::from_be_bytes([at(&r, 3), at(&r, 2)]),
            kind: at(&r, 1) & 0x0F,
            serial: r.get(4..8).map(hex),
            codename,
        });
    }
    let r = ch.read_register(RECEIVER_INFO, &[0x20 + n - 1]).ok()?;
    let wpid = u16_at(&r, 3);
    if wpid == 0 {
        return None;
    }
    let serial = ch.read_register(RECEIVER_INFO, &[0x30 + n - 1]).ok().and_then(|e| e.get(1..5).map(hex));
    let codename = ch.read_register(RECEIVER_INFO, &[0x40 + n - 1]).ok().map(|c| {
        let len = (at(&c, 1) as usize).min(14);
        String::from_utf8_lossy(c.get(2..2 + len).unwrap_or(&[])).to_string()
    });
    Some(Slot { slot: n, wpid, kind: at(&r, 7) & 0x0F, serial, codename })
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

/// Turn on connection notifications so slots report coming and going.
pub fn enable_notifications(ch: &mut Channel) {
    if let Err(e) = ch.write_register(NOTIFICATIONS, &NOTIFY_WIRELESS) {
        log::debug!("logitech receiver: enabling notifications: {e}");
    }
}

/// Ask the receiver to report every connected device (each answers with a
/// connection notification).
pub fn request_connections(ch: &mut Channel) {
    let _ = ch.write_register(CONNECTIONS, &[0x02]);
}

#[derive(Debug, Clone, Default, PartialEq)]
enum Pairing {
    #[default]
    Idle,
    /// Lock open (Unifying-style) or discovering (Bolt), until `until`.
    Waiting,
    /// Bolt: a device answered discovery and pairing was asked for.
    Requested { name: String },
    /// Bolt: the device must enter this passkey.
    Passkey { name: String, passkey: String, keyboard: bool },
    Done(String),
    Failed(String),
}

/// The receiver itself, as a device: pairing and the list of slots.
pub struct Receiver {
    info: DeviceInfo,
    ch: Channel,
    kind: Kind,
    slots: u8,
    may_unpair: bool,
    paired: Vec<Slot>,
    pairing: Pairing,
    until: Option<Instant>,
    /// Bolt discovery: the found device (address, authentication, kind).
    found: Option<([u8; 6], u8, u8)>,
    refreshed: Instant,
}

impl Receiver {
    pub fn open(mut ch: Channel, pid: u16, id: String) -> Result<Receiver> {
        let (kind, slots, may_unpair) = kind(pid).ok_or(Error::NotSupported(""))?;
        let serial = if kind == Kind::Bolt {
            ch.read_register(BOLT_UNIQUE_ID, &[]).ok().map(|r| hex(&r))
        } else {
            ch.read_register(RECEIVER_INFO, &[0x03]).ok().and_then(|r| r.get(1..5).map(hex))
        };
        let firmware = ch.read_register(0xF1, &[0x01]).ok().map(|r| format!("{:02X}.{:02X}", at(&r, 1), at(&r, 2)));
        enable_notifications(&mut ch);
        let info = DeviceInfo {
            id,
            kind: DeviceKind::Other,
            manufacturer: "Logitech".into(),
            model: kind.name().into(),
            vendor_string: None,
            product_string: None,
            usb_vid: super::VID,
            usb_pid: pid,
            serial,
            firmware,
        };
        let mut r = Receiver {
            info,
            ch,
            kind,
            slots,
            may_unpair,
            paired: Vec::new(),
            pairing: Pairing::Idle,
            until: None,
            found: None,
            refreshed: Instant::now(),
        };
        r.read_slots();
        Ok(r)
    }

    pub fn slug(pid: u16) -> &'static str {
        kind(pid).map(|k| k.0.slug()).unwrap_or("logitech-receiver")
    }

    fn read_slots(&mut self) {
        self.paired = (1..=self.slots).filter_map(|n| slot(&mut self.ch, self.kind, n)).collect();
        self.refreshed = Instant::now();
    }

    fn label(&self, s: &Slot) -> String {
        let name = super::devices::by_wpid(s.wpid)
            .map(|d| d.name.to_string())
            .or_else(|| s.codename.clone().filter(|c| !c.is_empty()))
            .unwrap_or_else(|| format!("device {:04X}", s.wpid));
        format!("Slot {}: {name}", s.slot)
    }

    fn start_pairing(&mut self) -> Result<()> {
        self.found = None;
        if self.kind == Kind::Bolt {
            self.ch.write_register(BOLT_DISCOVERY, &[PAIRING_TIMEOUT, 0x01])?;
        } else {
            self.ch.write_register(PAIRING_LOCK, &[0x01, 0x00, PAIRING_TIMEOUT])?;
        }
        self.pairing = Pairing::Waiting;
        self.until = Some(Instant::now() + Duration::from_secs(PAIRING_TIMEOUT as u64 + 5));
        Ok(())
    }

    fn stop_pairing(&mut self) {
        let r = if self.kind == Kind::Bolt {
            let _ = self.ch.write_register(BOLT_PAIRING, &[0x02]);
            self.ch.write_register(BOLT_DISCOVERY, &[0, 0x02])
        } else {
            self.ch.write_register(PAIRING_LOCK, &[0x02, 0x00, 0x00])
        };
        if let Err(e) = r {
            log::debug!("logitech receiver: closing pairing: {e}");
        }
        self.pairing = Pairing::Idle;
        self.until = None;
    }

    fn unpair(&mut self, slot: u8) -> Result<()> {
        if self.kind == Kind::Bolt {
            self.ch.write_register(BOLT_PAIRING, &[0x03, slot])?;
        } else {
            self.ch.write_register(PAIRING_LOCK, &[0x03, slot])?;
        }
        log::info!("logitech receiver: unpaired slot {slot}");
        self.read_slots();
        Ok(())
    }

    fn notify(&mut self, r: &Report) {
        match r.sub {
            // Pairing lock (Unifying-style): address bit 0 open; data[0] error.
            0x4A => {
                let open = r.address & 0x01 != 0;
                let err = at(&r.data, 0);
                if err != 0 {
                    self.pairing = Pairing::Failed(pairing_error(err).into());
                } else if !open && self.pairing == Pairing::Waiting {
                    self.pairing = Pairing::Idle;
                }
            }
            // Bolt discovery status: address 0 discovering.
            0x53 => {
                let err = at(&r.data, 0);
                if err != 0 {
                    self.pairing = Pairing::Failed(bolt_error(err).into());
                }
            }
            // Bolt device discovered: part 0 kind/address/auth, part 1 name.
            0x4F => match at(&r.data, 1) {
                0 => {
                    let mut addr = [0u8; 6];
                    addr.copy_from_slice(r.data.get(6..12).unwrap_or(&[0; 6]));
                    self.found = Some((addr, at(&r.data, 14), at(&r.data, 3)));
                }
                1 => {
                    let len = at(&r.data, 2) as usize;
                    let name = String::from_utf8_lossy(r.data.get(3..3 + len).unwrap_or(&[])).to_string();
                    if let (Some((addr, auth, kind)), Pairing::Waiting) = (self.found, &self.pairing) {
                        let entropy = if kind == 1 { 20 } else { 10 };
                        let mut p = vec![0x01, 0x00];
                        p.extend_from_slice(&addr);
                        p.push(auth);
                        p.push(entropy);
                        match self.ch.write_register(BOLT_PAIRING, &p) {
                            Ok(_) => self.pairing = Pairing::Requested { name },
                            Err(e) => self.pairing = Pairing::Failed(format!("could not pair {name}: {e}")),
                        }
                    }
                }
                _ => {}
            },
            // Bolt passkey to enter on the new device.
            0x4D => {
                let passkey = String::from_utf8_lossy(r.data.get(0..6).unwrap_or(&[])).to_string();
                let name = match &self.pairing {
                    Pairing::Requested { name } => name.clone(),
                    _ => "the device".into(),
                };
                let keyboard = self.found.is_some_and(|f| f.1 & 0x01 != 0);
                self.pairing = Pairing::Passkey { name, passkey, keyboard };
            }
            // Bolt pairing status: address 2 done; data[0] error.
            0x54 => {
                let err = at(&r.data, 0);
                if err != 0 {
                    self.pairing = Pairing::Failed(bolt_error(err).into());
                } else if r.address == 0x02 {
                    self.read_slots();
                    self.pairing = Pairing::Done(format!("Paired in slot {}.", at(&r.data, 7)));
                    self.until = None;
                }
            }
            // A device paired (Unifying-style) or connected.
            0x41 => {
                if self.pairing == Pairing::Waiting && self.kind != Kind::Bolt {
                    self.read_slots();
                    self.pairing = Pairing::Done(format!("Paired in slot {}.", r.device));
                    self.until = None;
                } else if !self.paired.iter().any(|s| s.slot == r.device) {
                    self.read_slots();
                }
            }
            // A device was unpaired.
            0x40 => self.read_slots(),
            _ => {}
        }
    }

    fn status(&self) -> String {
        match &self.pairing {
            Pairing::Idle => "Turn on, then switch the new device on (or press its connect button).".into(),
            Pairing::Waiting => format!("Waiting for a device… Switch it on or press its connect button. Stops after {PAIRING_TIMEOUT} seconds."),
            Pairing::Requested { name } => format!("Pairing {name}…"),
            Pairing::Passkey { name, passkey, keyboard: true } => format!("On {name}, type {passkey} and press Enter."),
            Pairing::Passkey { name, passkey, .. } => {
                let clicks: Vec<&str> =
                    passkey.parse::<u32>().map(|n| format!("{n:010b}")).unwrap_or_default().chars().map(|c| if c == '1' { "right" } else { "left" }).collect();
                format!("On {name}, click {} and then both buttons together.", clicks.join(", "))
            }
            Pairing::Done(s) => s.clone(),
            Pairing::Failed(s) => format!("Pairing failed: {s}"),
        }
    }
}

fn pairing_error(code: u8) -> &'static str {
    match code {
        0x01 => "the device did not answer in time",
        0x02 => "the device is not supported",
        0x03 => "too many devices",
        0x06 => "the pairing sequence timed out",
        _ => "error",
    }
}

fn bolt_error(code: u8) -> &'static str {
    match code {
        0x01 => "the device did not answer in time",
        0x02 => "pairing failed",
        _ => "error",
    }
}

impl Device for Receiver {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        for r in self.ch.take_notifications() {
            self.notify(&r);
        }
        if self.until.is_some_and(|t| Instant::now() >= t) {
            if matches!(self.pairing, Pairing::Waiting | Pairing::Requested { .. } | Pairing::Passkey { .. }) {
                self.stop_pairing();
                self.pairing = Pairing::Failed("no device was found".into());
            }
            self.until = None;
        }
        if self.refreshed.elapsed() > Duration::from_secs(60) {
            // Also notices the receiver going away.
            let n = self.ch.read_register(CONNECTIONS, &[]).map(|r| at(&r, 1))?;
            log::trace!("logitech receiver: {n} connected");
            self.read_slots();
        }
        let mut controls = vec![Control {
            id: "pair".into(),
            name: "Pairing: Pair a new device".into(),
            kind: ControlKind::Toggle,
            value: ControlValue::Bool(matches!(self.pairing, Pairing::Waiting | Pairing::Requested { .. } | Pairing::Passkey { .. })),
            help: Some(self.status()),
        }];
        for s in &self.paired {
            let mut options = vec!["Paired".to_string()];
            if self.may_unpair {
                options.push("Unpair".into());
            }
            controls.push(Control {
                id: format!("slot_{}", s.slot),
                name: format!("Paired devices: {}", self.label(s)),
                kind: ControlKind::Choice { options },
                value: ControlValue::Choice("Paired".into()),
                help: Some("Unpair removes the device from this receiver; pair it again to use it here.".into()),
            });
        }
        let sensors = vec![Sensor::new("paired", "Paired devices", SensorKind::State, self.paired.len() as f64)];
        Ok(Snapshot { sensors, controls })
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        match (id, value) {
            ("pair", ControlValue::Bool(true)) => self.start_pairing(),
            ("pair", ControlValue::Bool(false)) => {
                self.stop_pairing();
                Ok(())
            }
            (id, ControlValue::Choice(c)) if id.starts_with("slot_") => {
                let slot: u8 = id[5..].parse().map_err(|_| Error::UnknownControl(id.into()))?;
                if c == "Unpair" && self.may_unpair {
                    self.unpair(slot)
                } else {
                    Ok(())
                }
            }
            _ => Err(Error::UnknownControl(id.into())),
        }
    }

    fn restore_toggles(&self) -> bool {
        false
    }

    fn poll_interval(&self) -> Option<Duration> {
        Some(Duration::from_secs(1))
    }

    fn release(&mut self) -> Result<()> {
        if !matches!(self.pairing, Pairing::Idle | Pairing::Done(_) | Pairing::Failed(_)) {
            self.stop_pairing();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_receivers() {
        assert_eq!(kind(0xC548).map(|k| k.0), Some(Kind::Bolt));
        assert_eq!(kind(0xC52B).map(|k| (k.0, k.1)), Some((Kind::Unifying, 6)));
        assert!(kind(0xC529).is_none());
    }
}
