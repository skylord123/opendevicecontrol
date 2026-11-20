//! HID++ framing and request/reply matching, for HID++ 1.0 (registers)
//! and HID++ 2.0 (features), over a receiver or a direct link.
//!
//! A report is `[report id, device index, sub id / feature index,
//! address / function | software id, parameters…]`. Short reports (`0x10`)
//! carry 3 parameter bytes, long ones (`0x11`) 16. Device index `0xFF` is
//! the receiver itself, or the device on a USB cable or Bluetooth link;
//! `1..=6` are the slots of a receiver.

use crate::{Error, Result};
use hidapi::HidDevice;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const SHORT: u8 = 0x10;
pub const LONG: u8 = 0x11;
/// Up to 64 bytes; only some gaming devices send them.
pub const VERY_LONG: u8 = 0x12;
pub const SHORT_LEN: usize = 7;
pub const LONG_LEN: usize = 20;

/// The receiver, or a device on a cable or Bluetooth.
pub const DIRECT: u8 = 0xFF;

/// Our software id, in the low nibble of every request's function byte, so
/// replies to us can be told from those to other programs (Solaar uses
/// 2..=15 in turn, the Linux kernel 1) and from notifications (0).
pub const SW_ID: u8 = 0x0A;

/// HID++ 1.0 sub ids.
pub const SET_REGISTER: u8 = 0x80;
pub const GET_REGISTER: u8 = 0x81;
pub const SET_LONG_REGISTER: u8 = 0x82;
pub const GET_LONG_REGISTER: u8 = 0x83;
pub const ERROR_10: u8 = 0x8F;
/// HID++ 2.0 error reports use this in place of the feature index.
pub const ERROR_20: u8 = 0xFF;

/// Receiver notifications (HID++ 1.0 sub ids sent with software id 0).
pub const DEVICE_DISCONNECTED: u8 = 0x40;
pub const DEVICE_CONNECTED: u8 = 0x41;

const REPLY_TIMEOUT: Duration = Duration::from_millis(1500);

/// One HID++ report as received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub id: u8,
    pub device: u8,
    /// Sub id (HID++ 1.0) or feature index (2.0).
    pub sub: u8,
    /// Register address (1.0) or function << 4 | software id (2.0).
    pub address: u8,
    pub data: Vec<u8>,
}

impl Report {
    pub fn parse(buf: &[u8]) -> Option<Report> {
        let len = match *buf.first()? {
            SHORT => SHORT_LEN,
            LONG => LONG_LEN,
            VERY_LONG => buf.len().min(64),
            _ => return None,
        };
        if buf.len() < 4 {
            return None;
        }
        let end = len.min(buf.len());
        Some(Report { id: buf[0], device: buf[1], sub: buf[2], address: buf[3], data: buf[4..end].to_vec() })
    }

    /// Sent by the device on its own (not a reply to anyone).
    pub fn is_notification(&self) -> bool {
        // Feature events carry software id 0; receiver events (connection,
        // pairing lock, Bolt discovery) are sub ids 0x40..0x5F.
        self.sub < 0x80 && self.address & 0x0F == 0 || (0x40..=0x5F).contains(&self.sub)
    }

    /// HID++ 2.0 function number of a feature report.
    pub fn function(&self) -> u8 {
        self.address >> 4
    }

    /// Parameter byte `i`, 0 past the end.
    pub fn byte(&self, i: usize) -> u8 {
        self.data.get(i).copied().unwrap_or(0)
    }
}

/// What a received report means for a pending request.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    /// The reply, with its parameters.
    Reply(Vec<u8>),
    /// An error code (HID++ 1.0 or 2.0).
    Error(u8),
    /// Something else (a notification, or traffic for another program).
    Other,
}

/// Match `r` against a request to `device` with `sub` (sub id or feature
/// index) and `address` (register or function | software id).
pub fn answer(r: &Report, device: u8, sub: u8, address: u8) -> Answer {
    if r.device != device {
        return Answer::Other;
    }
    if r.sub == sub && r.address == address {
        return Answer::Reply(r.data.clone());
    }
    if (r.sub == ERROR_10 || r.sub == ERROR_20) && r.address == sub && r.byte(0) == address {
        return Answer::Error(r.byte(1));
    }
    Answer::Other
}

/// HID++ 2.0 error names.
pub fn error_name(code: u8) -> &'static str {
    match code {
        0 => "no error",
        1 => "unknown",
        2 => "invalid argument",
        3 => "out of range",
        4 => "hardware error",
        5 => "Logitech internal",
        6 => "invalid feature index",
        7 => "invalid function",
        8 => "busy",
        9 => "unsupported",
        _ => "error",
    }
}

/// HID++ 1.0 error names (also what a receiver answers for a device in a
/// slot that is asleep or out of range).
pub fn error10_name(code: u8) -> &'static str {
    match code {
        0x01 => "invalid sub id",
        0x02 => "invalid address",
        0x03 => "invalid value",
        0x04 => "connection request failed",
        0x05 => "too many devices",
        0x06 => "already exists",
        0x07 => "busy",
        0x08 => "unknown device",
        0x09 => "resource error",
        0x0A => "request unavailable",
        0x0B => "unsupported parameter value",
        0x0C => "wrong PIN code",
        _ => "error",
    }
}

/// An error answer from the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused {
    pub code: u8,
    /// HID++ 1.0 error (0x8F), else 2.0 (0xFF).
    pub old: bool,
}

/// A HID++ link to one device index: the open HID handle(s) plus the
/// notifications that arrived while waiting for replies.
pub struct Channel {
    /// The HID++ collection (both report sizes on Linux; short ones on
    /// Windows, which opens each collection separately).
    main: HidDevice,
    /// Windows: the long-report collection.
    long: Option<HidDevice>,
    pub device: u8,
    /// Whether the link takes short reports (Bluetooth devices only take
    /// long ones).
    pub short_ok: bool,
    queue: VecDeque<Report>,
}

impl Channel {
    pub fn new(main: HidDevice, long: Option<HidDevice>, device: u8, short_ok: bool) -> Channel {
        let _ = main.set_blocking_mode(false);
        if let Some(l) = &long {
            let _ = l.set_blocking_mode(false);
        }
        Channel { main, long, device, short_ok, queue: VecDeque::new() }
    }

    /// Notifications received so far (and anything else not yet taken).
    pub fn take_notifications(&mut self) -> Vec<Report> {
        self.read_pending();
        self.queue.drain(..).collect()
    }

    fn read_pending(&mut self) {
        while let Ok(Some(r)) = self.read(Duration::ZERO) {
            if r.is_notification() && r.device == self.device {
                self.queue.push_back(r);
            }
            if self.queue.len() > 512 {
                self.queue.pop_front();
            }
        }
    }

    /// One report, waiting up to `timeout`.
    fn read(&self, timeout: Duration) -> Result<Option<Report>> {
        let mut buf = [0u8; 64];
        let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        let handles: Vec<&HidDevice> = std::iter::once(&self.main).chain(self.long.iter()).collect();
        if handles.len() == 1 {
            let n = self.main.read_timeout(&mut buf, ms).map_err(gone)?;
            return Ok(if n == 0 { None } else { Report::parse(&buf[..n]) });
        }
        // Two collections: poll both in short slices.
        let deadline = Instant::now() + timeout;
        loop {
            for h in &handles {
                let n = h.read_timeout(&mut buf, 0).map_err(gone)?;
                if n > 0 {
                    return Ok(Report::parse(&buf[..n]));
                }
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn write(&self, long: bool, sub: u8, address: u8, params: &[u8]) -> Result<()> {
        let long = long || !self.short_ok || params.len() > 3;
        let len = if long { LONG_LEN } else { SHORT_LEN };
        let mut out = vec![0u8; len];
        out[0] = if long { LONG } else { SHORT };
        out[1] = self.device;
        out[2] = sub;
        out[3] = address;
        let n = params.len().min(len - 4);
        out[4..4 + n].copy_from_slice(&params[..n]);
        let h = match (&self.long, long) {
            (Some(l), true) => l,
            _ => &self.main,
        };
        h.write(&out).map_err(gone)?;
        Ok(())
    }

    /// Send a request and wait for its answer. `Ok(Err(..))` is the
    /// device's error answer.
    fn exchange(&mut self, long: bool, sub: u8, address: u8, params: &[u8], timeout: Duration) -> Result<std::result::Result<Vec<u8>, Refused>> {
        self.read_pending();
        self.write(long, sub, address, params)?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::NoReply { cmd: sub });
            }
            let Some(r) = self.read(left)? else { continue };
            match answer(&r, self.device, sub, address) {
                Answer::Reply(d) => return Ok(Ok(d)),
                Answer::Error(code) => return Ok(Err(Refused { code, old: r.sub == ERROR_10 })),
                Answer::Other => {
                    if r.is_notification() && r.device == self.device {
                        self.queue.push_back(r);
                    }
                }
            }
        }
    }

    /// A HID++ 2.0 feature call: function `function` of the feature at
    /// `index`.
    pub fn feature(&mut self, index: u8, function: u8, params: &[u8]) -> Result<Vec<u8>> {
        self.feature_timeout(index, function, params, REPLY_TIMEOUT)
    }

    pub fn feature_timeout(&mut self, index: u8, function: u8, params: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        let address = (function << 4) | SW_ID;
        match self.exchange(true, index, address, params, timeout)? {
            Ok(d) => Ok(d),
            Err(r) => Err(refused(r, index)),
        }
    }

    /// Send a feature call without waiting for an answer (changing host:
    /// the device leaves before it could reply).
    pub fn feature_no_reply(&mut self, index: u8, function: u8, params: &[u8]) -> Result<()> {
        self.write(true, index, (function << 4) | SW_ID, params)
    }

    /// Read HID++ 1.0 register `register` (`0x2xx`: a long register).
    pub fn read_register(&mut self, register: u16, params: &[u8]) -> Result<Vec<u8>> {
        let sub = if register & 0x200 != 0 { GET_LONG_REGISTER } else { GET_REGISTER };
        match self.exchange(false, sub, register as u8, params, REPLY_TIMEOUT)? {
            Ok(d) => Ok(d),
            Err(r) => Err(refused(r, sub)),
        }
    }

    /// Write HID++ 1.0 register `register` (`0x2xx`: a long register).
    pub fn write_register(&mut self, register: u16, value: &[u8]) -> Result<Vec<u8>> {
        let long = register & 0x200 != 0;
        let sub = if long { SET_LONG_REGISTER } else { SET_REGISTER };
        match self.exchange(long, sub, register as u8, value, REPLY_TIMEOUT)? {
            Ok(d) => Ok(d),
            Err(r) => Err(refused(r, sub)),
        }
    }

    /// The device's HID++ protocol version (major, minor), `(1, 0)` for
    /// HID++ 1.0 devices.
    pub fn protocol(&mut self) -> Result<(u8, u8)> {
        // Root feature, function 1 (ping); a HID++ 1.0 device answers with
        // "invalid sub id" for feature index 0.
        let address = (1 << 4) | SW_ID;
        match self.exchange(!self.short_ok, 0, address, &[0, 0, 0x5A], REPLY_TIMEOUT)? {
            Ok(d) => Ok((d.first().copied().unwrap_or(0), d.get(1).copied().unwrap_or(0))),
            Err(Refused { code: 0x01, old: true }) => Ok((1, 0)),
            Err(r) => Err(refused(r, 0)),
        }
    }
}

/// A HID error from the handle: the device was unplugged or its Bluetooth
/// link went away.
fn gone(e: hidapi::HidError) -> Error {
    Error::Hid(e)
}

fn refused(r: Refused, cmd: u8) -> Error {
    // A receiver answers for a device that is asleep or out of range.
    if r.old && matches!(r.code, 0x04 | 0x08 | 0x09) {
        return Error::Offline;
    }
    if !r.old && r.code == 9 {
        return Error::NotSupported("");
    }
    log::debug!(
        "HID++ request {cmd:#04x} refused: {} ({:#04x})",
        if r.old { error10_name(r.code) } else { error_name(r.code) },
        r.code
    );
    Error::Unsupported { cmd }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(bytes: &[u8]) -> Report {
        Report::parse(bytes).unwrap()
    }

    #[test]
    fn parses_short_and_long_reports() {
        let s = r(&[0x10, 0x01, 0x81, 0x07, 0x01, 0x00, 0x00]);
        assert_eq!((s.device, s.sub, s.address, s.data.len()), (1, 0x81, 0x07, 3));
        let mut long = vec![0x11, 0xff, 0x09, 0x1a];
        long.extend([0u8; 16]);
        let l = r(&long);
        assert_eq!((l.device, l.sub, l.function(), l.data.len()), (0xff, 9, 1, 16));
        assert!(Report::parse(&[0x02, 1, 2, 3]).is_none());
    }

    #[test]
    fn matches_replies_errors_and_notifications() {
        // Reply to function 1 of feature 9 with our software id.
        let reply = r(&[0x11, 0xff, 0x09, 0x1a, 0xaa, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(answer(&reply, 0xff, 9, 0x1a), Answer::Reply(reply.data.clone()));
        assert_eq!(answer(&reply, 0x01, 9, 0x1a), Answer::Other);
        // HID++ 2.0 error: 0xFF, feature index, function | swid, code.
        let err = r(&[0x11, 0xff, 0xff, 0x09, 0x1a, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(answer(&err, 0xff, 9, 0x1a), Answer::Error(2));
        // HID++ 1.0 error for a register read.
        let err10 = r(&[0x10, 0xff, 0x8f, 0x83, 0xb5, 0x08, 0x00]);
        assert_eq!(answer(&err10, 0xff, 0x83, 0xb5), Answer::Error(8));
        // Diverted button notification: software id 0.
        let note = r(&[0x11, 0xff, 0x09, 0x00, 0x00, 0xc3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(note.is_notification());
        assert!(!reply.is_notification());
        assert!(r(&[0x10, 0x01, 0x41, 0x04, 0x02, 0x1a, 0x10]).is_notification());
    }
}
