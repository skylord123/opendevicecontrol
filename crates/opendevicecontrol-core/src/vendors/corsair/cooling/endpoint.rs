//! The "endpoint" protocol spoken by Corsair's newer cooling hubs: the iCUE
//! LINK System Hub, Commander Core / Core XT / ST and Commander DUO.
//!
//! Every request is one HID output report: a model-specific prefix
//! (`[0x08]` on the Commander family, `[0x00, 0x01]` on the LINK hub), a
//! command and its data. Every request is answered by one input report that
//! starts with the same number of prefix bytes, then echoes the command's
//! first byte, then a status byte. Reading or writing a value goes through an
//! "endpoint" (a numbered register block):
//!
//! ```text
//! close  05 01 01 <ep>      open  0d 01 <ep>
//! read   08 01 <ep>   ->    <echo> <status> <type lo> <type hi> <count> <data…>
//! write  06 01 <len lo> <len hi> 00 00 <type lo> <type hi> <data…>
//! close  05 01 01 <ep>
//! ```
//!
//! Colours go through the colour endpoint (`0d 00 22`) with the first chunk
//! sent as `06 00` and further chunks as `07 00`.
//!
//! References (GPL-3.0-or-later): OpenLinkHub `src/devices/{lsh,cc,ccxt,cduo}`
//! by Nikola Jurkovic, liquidctl `commander_core.py` by ParkerMc and
//! contributors. Byte layouts are copied from those projects; nothing here
//! has been verified on real hardware by opendevicecontrol.

use crate::{Error, Result};
use hidapi::HidDevice;
use std::time::{Duration, Instant};

pub const SOFTWARE_MODE: &[u8] = &[0x01, 0x03, 0x00, 0x02];
pub const HARDWARE_MODE: &[u8] = &[0x01, 0x03, 0x00, 0x01];
pub const GET_FIRMWARE: &[u8] = &[0x02, 0x13];
const OPEN: &[u8] = &[0x0d, 0x01];
const OPEN_COLOR: &[u8] = &[0x0d, 0x00];
const CLOSE: &[u8] = &[0x05, 0x01, 0x01];
const READ: &[u8] = &[0x08, 0x01];
const WRITE: &[u8] = &[0x06, 0x01];
pub const WRITE_COLOR: &[u8] = &[0x06, 0x00];
pub const WRITE_COLOR_NEXT: &[u8] = &[0x07, 0x00];

/// Endpoints ("modes").
pub mod ep {
    pub const GET_SPEEDS: u8 = 0x17;
    pub const SET_SPEED: u8 = 0x18;
    pub const GET_FANS: u8 = 0x1a;
    pub const SET_LED_PORTS: u8 = 0x1e;
    pub const GET_LEDS: u8 = 0x20;
    pub const GET_TEMPS: u8 = 0x21;
    pub const SET_COLOR: u8 = 0x22;
    pub const GET_DEVICES: u8 = 0x36;
    pub const DEVICE_MODEL: u8 = 0x40;
}

/// Data types carried in read replies and write payloads.
pub mod dtype {
    pub const SPEEDS: [u8; 2] = [0x25, 0x00];
    pub const TEMPS: [u8; 2] = [0x10, 0x00];
    pub const DEVICES: [u8; 2] = [0x21, 0x00];
    pub const SET_SPEED: [u8; 2] = [0x07, 0x00];
    pub const SET_COLOR: [u8; 2] = [0x12, 0x00];
}

/// Speed mode byte in a SET_SPEED entry: duty in percent.
pub const SPEED_MODE_PERCENT: u8 = 0x00;

/// Build an output report: report id 0, prefix, command, data, zero padded
/// to `packet` bytes after the report id.
pub fn encode(prefix: &[u8], cmd: &[u8], data: &[u8], packet: usize) -> Vec<u8> {
    let mut out = vec![0u8; packet + 1];
    let body: Vec<u8> = prefix.iter().chain(cmd).chain(data).copied().collect();
    let n = body.len().min(packet);
    out[1..1 + n].copy_from_slice(&body[..n]);
    out
}

/// Payload of a `06 01` endpoint write: length, two zero bytes, the data
/// type and the data. `extra` adds the two data-type bytes to the length
/// (the hubs expect that for typed writes).
pub fn write_payload(data_type: Option<[u8; 2]>, data: &[u8], extra: bool) -> Vec<u8> {
    let len = data.len() + if extra { 2 } else { 0 };
    let mut b = Vec::with_capacity(data.len() + 6);
    b.extend_from_slice(&(len as u16).to_le_bytes());
    b.extend_from_slice(&[0, 0]);
    if let Some(t) = data_type {
        b.extend_from_slice(&t);
    }
    b.extend_from_slice(data);
    b
}

/// SET_SPEED data: count, then `[channel, mode, duty, 0]` per channel,
/// sorted by channel.
pub fn speed_data(duties: &[(u8, u8)]) -> Vec<u8> {
    let mut d: Vec<(u8, u8)> = duties.to_vec();
    d.sort_by_key(|x| x.0);
    let mut b = vec![d.len() as u8];
    for (ch, duty) in d {
        b.extend_from_slice(&[ch, SPEED_MODE_PERCENT, duty.min(100), 0]);
    }
    b
}

/// Split a colour payload (already wrapped by [`write_payload`]) into the
/// commands and chunks the hub expects.
pub fn color_chunks(payload: &[u8], max: usize) -> Vec<(&'static [u8], Vec<u8>)> {
    payload
        .chunks(max.max(1))
        .enumerate()
        .map(|(i, c)| (if i == 0 { WRITE_COLOR } else { WRITE_COLOR_NEXT }, c.to_vec()))
        .collect()
}

/// `major.minor.patch` from a GET_FIRMWARE reply (normalised, see [`EndpointIo::transfer`]).
pub fn parse_firmware(r: &[u8]) -> Option<String> {
    if r.len() < 6 {
        return None;
    }
    Some(format!("{}.{}.{}", r[2], r[3], u16::from_le_bytes([r[4], r[5]])))
}

/// Data type of a read reply.
pub fn reply_type(r: &[u8]) -> Option<[u8; 2]> {
    Some([*r.get(2)?, *r.get(3)?])
}

/// `(count, entries)` of a read reply.
pub fn reply_entries(r: &[u8]) -> (usize, &[u8]) {
    match r.get(4) {
        Some(&n) => (n as usize, r.get(5..).unwrap_or(&[])),
        None => (0, &[]),
    }
}

/// Speeds as `[status, rpm lo, rpm hi]` triples (LINK hub). `None` for
/// channels whose status is not 0 (nothing connected).
pub fn parse_speeds_with_status(r: &[u8]) -> Vec<Option<u16>> {
    let (n, d) = reply_entries(r);
    (0..n)
        .map(|i| d.get(i * 3..i * 3 + 3).filter(|e| e[0] == 0).map(|e| u16::from_le_bytes([e[1], e[2]])))
        .collect()
}

/// Speeds as plain little-endian u16 (Commander family).
pub fn parse_speeds_plain(r: &[u8]) -> Vec<u16> {
    let (n, d) = reply_entries(r);
    (0..n).filter_map(|i| d.get(i * 2..i * 2 + 2).map(|e| u16::from_le_bytes([e[0], e[1]]))).collect()
}

/// Temperatures as `[status, t lo, t hi]` triples in tenths of °C; `None`
/// for disconnected probes.
pub fn parse_temps(r: &[u8]) -> Vec<Option<f64>> {
    let (n, d) = reply_entries(r);
    (0..n)
        .map(|i| {
            d.get(i * 3..i * 3 + 3)
                .filter(|e| e[0] == 0)
                .map(|e| i16::from_le_bytes([e[1], e[2]]) as f64 / 10.0)
        })
        .collect()
}

/// LED ports from GET_LEDS: `[status u16, leds u16]` per port; `Some(leds)`
/// when status is 2 (connected).
pub fn parse_led_ports(r: &[u8]) -> Vec<Option<u16>> {
    let (n, d) = reply_entries(r);
    (0..n)
        .map(|i| {
            d.get(i * 4..i * 4 + 4)
                .filter(|e| u16::from_le_bytes([e[0], e[1]]) == 2)
                .map(|e| u16::from_le_bytes([e[2], e[3]]))
        })
        .collect()
}

/// How one input report relates to the command just sent.
#[derive(Debug, PartialEq)]
enum Reply {
    /// The answer: `[echo, status, payload...]` (prefix stripped).
    Match(Vec<u8>),
    /// An answer to some other command (stale or foreign); keep reading.
    Foreign,
    /// Echoes the command but stops before its status byte.
    Truncated,
}

fn match_reply(prefix_len: usize, cmd: &[u8], raw: &[u8]) -> Reply {
    let r = raw.get(prefix_len..).unwrap_or(&[]);
    if r.is_empty() || r.first() != cmd.first() {
        return Reply::Foreign;
    }
    if r.len() < 2 {
        return Reply::Truncated;
    }
    Reply::Match(r.to_vec())
}

pub struct EndpointIo {
    dev: HidDevice,
    prefix: Vec<u8>,
    packet: usize,
    timeout: Duration,
}

impl EndpointIo {
    pub fn new(dev: HidDevice, prefix: &[u8], packet: usize) -> Self {
        Self { dev, prefix: prefix.to_vec(), packet, timeout: Duration::from_millis(500) }
    }

    /// Largest colour chunk that fits in one report.
    pub fn max_chunk(&self) -> usize {
        self.packet - self.prefix.len() - 2
    }

    fn drain(&self) {
        let mut buf = vec![0u8; self.packet];
        for _ in 0..32 {
            match self.dev.read_timeout(&mut buf, 0) {
                Ok(n) if n > 0 => continue,
                _ => break,
            }
        }
    }

    /// Send one command and return the reply without its prefix bytes:
    /// `[echo, status, payload…]`.
    pub fn transfer(&self, cmd: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        let prefix = self.prefix.clone();
        self.transfer_with_prefix(&prefix, cmd, data)
    }

    pub fn transfer_with_prefix(&self, prefix: &[u8], cmd: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        self.drain();
        self.dev.write(&encode(prefix, cmd, data, self.packet))?;
        let deadline = Instant::now() + self.timeout;
        let no_reply = Error::NoReply { cmd: cmd.first().copied().unwrap_or(0) };
        for _ in 0..4 {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let mut buf = vec![0u8; self.packet];
            let n = self.dev.read_timeout(&mut buf, left.as_millis().max(1) as i32)?;
            if n == 0 {
                break;
            }
            buf.truncate(n);
            match match_reply(prefix.len(), cmd, &buf) {
                Reply::Match(r) => return Ok(r),
                Reply::Truncated => {
                    log::debug!("endpoint: truncated reply {:02x?}", &buf[..buf.len().min(6)]);
                    return Err(no_reply);
                }
                Reply::Foreign => log::trace!("endpoint: unexpected reply {:02x?}", &buf[..buf.len().min(6)]),
            }
        }
        // Only replies to other commands (or none): this one wasn't answered.
        Err(no_reply)
    }

    /// Read an endpoint. When `multi` names the expected data type and the
    /// first reply carries it, a second report is read and appended (the
    /// LINK hub splits long lists such as its device table).
    pub fn read_endpoint(&self, endpoint: u8, multi: Option<[u8; 2]>) -> Result<Vec<u8>> {
        let _ = self.transfer(CLOSE, &[endpoint]);
        self.transfer(OPEN, &[endpoint])?;
        let mut r = self.transfer(READ, &[endpoint]);
        if let (Ok(first), Some(t)) = (&mut r, multi) {
            if reply_type(first) == Some(t) {
                if let Ok(next) = self.transfer(READ, &[endpoint]) {
                    first.extend_from_slice(next.get(2..).unwrap_or(&[]));
                }
            }
        }
        let _ = self.transfer(CLOSE, &[endpoint]);
        r
    }

    /// Read an endpoint with a different first prefix byte (LINK "v2"
    /// devices are addressed by putting their channel there).
    pub fn read_endpoint_addressed(&self, endpoint: u8, first_prefix: u8) -> Result<Vec<u8>> {
        let mut prefix = self.prefix.clone();
        if let Some(p) = prefix.first_mut() {
            *p = first_prefix;
        }
        let _ = self.transfer_with_prefix(&prefix, CLOSE, &[endpoint]);
        self.transfer_with_prefix(&prefix, OPEN, &[endpoint])?;
        let r = self.transfer_with_prefix(&prefix, READ, &[endpoint]);
        let _ = self.transfer_with_prefix(&prefix, CLOSE, &[endpoint]);
        r
    }

    /// Write `payload` (see [`write_payload`]) to an endpoint; returns the reply.
    pub fn write_endpoint(&self, endpoint: u8, payload: &[u8]) -> Result<Vec<u8>> {
        let _ = self.transfer(CLOSE, &[endpoint]);
        self.transfer(OPEN, &[endpoint])?;
        let r = self.transfer(WRITE, payload);
        let _ = self.transfer(CLOSE, &[endpoint]);
        r
    }

    /// Open the colour endpoint (done once, before colour writes).
    pub fn open_color_endpoint(&self) -> Result<()> {
        let _ = self.transfer(CLOSE, &[ep::SET_COLOR]);
        self.transfer(OPEN_COLOR, &[ep::SET_COLOR]).map(|_| ())
    }

    /// Write one frame of colours (`r, g, b` per LED, all LEDs in order).
    pub fn write_colors(&self, rgb: &[u8]) -> Result<()> {
        let payload = write_payload(Some(dtype::SET_COLOR), rgb, true);
        for (cmd, chunk) in color_chunks(&payload, self.max_chunk()) {
            self.transfer(cmd, &chunk)?;
        }
        Ok(())
    }

    /// Set duties (percent) per channel.
    pub fn set_speeds(&self, duties: &[(u8, u8)]) -> Result<()> {
        let payload = write_payload(Some(dtype::SET_SPEED), &speed_data(duties), true);
        // `transfer` only returns replies that echo the command and carry
        // a status byte, so `r[1]` is the status.
        let r = self.write_endpoint(ep::SET_SPEED, &payload)?;
        if r[1] != 0 {
            // The LINK hub occasionally rejects a speed write; retry once.
            let r = self.write_endpoint(ep::SET_SPEED, &payload)?;
            if r[1] != 0 {
                return Err(Error::Unsupported { cmd: ep::SET_SPEED });
            }
        }
        Ok(())
    }

    pub fn firmware(&self) -> Option<String> {
        self.transfer(GET_FIRMWARE, &[]).ok().and_then(|r| parse_firmware(&r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_commander_and_link_frames() {
        // Commander Core: 08 prefix (liquidctl `_send_command`).
        let f = encode(&[0x08], &[0x0d, 0x01], &[0x17], 64);
        assert_eq!(f.len(), 65);
        assert_eq!(&f[..5], &[0x00, 0x08, 0x0d, 0x01, 0x17]);
        // LINK hub: bufferW[2] = 0x01, endpoint from byte 3 (OpenLinkHub `transfer`).
        let f = encode(&[0x00, 0x01], &[0x08, 0x01], &[0x36], 512);
        assert_eq!(f.len(), 513);
        assert_eq!(&f[..6], &[0x00, 0x00, 0x01, 0x08, 0x01, 0x36]);
    }

    #[test]
    fn speed_payload_matches_reference() {
        // OpenLinkHub setSpeed: [count, ch, mode, value, 0 …], wrapped by write().
        let p = write_payload(Some(dtype::SET_SPEED), &speed_data(&[(2, 40), (0, 70)]), true);
        assert_eq!(p, vec![11, 0, 0, 0, 0x07, 0x00, 2, 0, 0, 70, 0, 2, 0, 40, 0]);
    }

    #[test]
    fn color_payload_is_chunked() {
        let rgb = vec![0xaa; 300];
        let p = write_payload(Some(dtype::SET_COLOR), &rgb, true);
        assert_eq!(&p[..6], &[0x2e, 0x01, 0, 0, 0x12, 0x00]); // 302 = 0x012e
        let c = color_chunks(&p, 61);
        assert_eq!(c.len(), 6); // 306 bytes in 61-byte chunks
        assert_eq!(c[0].0, WRITE_COLOR);
        assert!(c[1..].iter().all(|x| x.0 == WRITE_COLOR_NEXT));
        assert_eq!(c.iter().map(|x| x.1.len()).sum::<usize>(), 306);
    }

    #[test]
    fn parses_replies() {
        // Normalised replies: echo, status, type(2), count, data.
        let fw = [0x02, 0x00, 2, 4, 0xb6, 0x01];
        assert_eq!(parse_firmware(&fw).unwrap(), "2.4.438");

        let speeds = [0x08, 0, 0x25, 0, 3, 1, 0, 0, 0, 0xdc, 0x05, 0, 0x10, 0x27];
        assert_eq!(parse_speeds_with_status(&speeds), vec![None, Some(1500), Some(10000)]);

        let cc = [0x08, 0, 0x06, 0, 2, 0x10, 0x0e, 0xe8, 0x03];
        assert_eq!(parse_speeds_plain(&cc), vec![3600, 1000]);

        let temps = [0x08, 0, 0x10, 0, 2, 0, 0x2c, 0x01, 1, 0, 0];
        assert_eq!(parse_temps(&temps), vec![Some(30.0), None]);

        let leds = [0x08, 0, 0x0f, 0, 2, 2, 0, 29, 0, 3, 0, 0, 0];
        assert_eq!(parse_led_ports(&leds), vec![Some(29), None]);
    }

    #[test]
    fn replies_must_echo_the_command_and_carry_a_status() {
        // LINK hub prefix is two bytes; the echo follows it.
        assert_eq!(match_reply(2, WRITE, &[0x00, 0x01, 0x06, 0x00, 0xaa]), Reply::Match(vec![0x06, 0x00, 0xaa]));
        // A reply to another command (e.g. a stale CLOSE) is not ours.
        assert_eq!(match_reply(2, WRITE, &[0x00, 0x01, 0x05, 0x00]), Reply::Foreign);
        // Nothing after the prefix.
        assert_eq!(match_reply(2, WRITE, &[0x00, 0x01]), Reply::Foreign);
        // Echo without a status byte.
        assert_eq!(match_reply(1, WRITE, &[0x08, 0x06]), Reply::Truncated);
        // Commander family: one-byte prefix.
        assert_eq!(match_reply(1, GET_FIRMWARE, &[0x08, 0x02, 0x00, 2, 4]), Reply::Match(vec![0x02, 0x00, 2, 4]));
    }
}
