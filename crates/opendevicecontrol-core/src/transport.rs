//! Thin wrapper around a HID handle that implements Corsair's
//! "request / echo-checked reply" pattern with conflict-tolerant retries.

use crate::{Error, Result};
use hidapi::HidDevice;
use std::time::{Duration, Instant};

pub const REPORT_LEN: usize = 64;

pub struct HidTransport {
    dev: HidDevice,
    timeout: Duration,
    retries: usize,
}

impl HidTransport {
    pub fn new(dev: HidDevice) -> Self {
        Self { dev, timeout: Duration::from_millis(250), retries: 3 }
    }

    /// Discard any input reports already queued (e.g. replies meant for
    /// another program that is polling the same device).
    fn drain(&self) {
        let mut buf = [0u8; REPORT_LEN];
        for _ in 0..32 {
            match self.dev.read_timeout(&mut buf, 0) {
                Ok(n) if n > 0 => continue,
                _ => break,
            }
        }
    }

    pub fn write_raw(&self, payload: &[u8]) -> Result<()> {
        debug_assert!(payload.len() <= REPORT_LEN);
        // Byte 0 is the report ID; these devices do not use numbered reports.
        let mut out = [0u8; REPORT_LEN + 1];
        out[1..1 + payload.len()].copy_from_slice(payload);
        self.dev.write(&out)?;
        Ok(())
    }

    /// Send `payload` and wait for a reply whose first `echo` bytes match the
    /// request. Mismatching replies are skipped; the request is retried a
    /// few times before giving up.
    pub fn transact(&self, payload: &[u8], echo: usize) -> Result<[u8; REPORT_LEN]> {
        self.transact_min(payload, echo, echo)
    }

    /// Like [`HidTransport::transact`], but a reply also has to be at least
    /// `need` bytes long (what the caller will decode); shorter ones are
    /// skipped instead of being read as zero padding.
    pub fn transact_min(&self, payload: &[u8], echo: usize, need: usize) -> Result<[u8; REPORT_LEN]> {
        let cmd = payload.get(1).copied().unwrap_or(0);
        let mut saw_reply = false;
        for _ in 0..self.retries {
            self.drain();
            self.write_raw(payload)?;
            let deadline = Instant::now() + self.timeout;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                let mut buf = [0u8; REPORT_LEN];
                let n = self.dev.read_timeout(&mut buf, left.as_millis().max(1) as i32)?;
                if n == 0 {
                    break;
                }
                saw_reply = true;
                if accept(payload, echo, need, &buf, n) {
                    return Ok(buf);
                }
                log::trace!("skipping foreign or short reply ({n} bytes) {:02x?}", &buf[..4]);
            }
        }
        if saw_reply {
            Err(Error::Unsupported { cmd })
        } else {
            Err(Error::NoReply { cmd })
        }
    }
}

/// Whether a reply of `n` bytes in `buf` answers `payload`: it echoes the
/// first `echo` bytes and is at least `need` bytes long.
fn accept(payload: &[u8], echo: usize, need: usize, buf: &[u8], n: usize) -> bool {
    let n = n.min(buf.len());
    n >= echo.max(need) && echo <= payload.len() && buf[..echo] == payload[..echo]
}

#[cfg(test)]
mod tests {
    use super::accept;

    #[test]
    fn short_or_foreign_replies_are_skipped() {
        let mut buf = [0u8; 64];
        buf[..4].copy_from_slice(&[0x03, 0x8b, 0x12, 0x34]);
        assert!(accept(&[0x03, 0x8b], 2, 4, &buf, 64));
        assert!(accept(&[0x03, 0x8b], 2, 4, &buf, 4));
        // Truncated: the data bytes we would decode were never received.
        assert!(!accept(&[0x03, 0x8b], 2, 4, &buf, 3));
        // Truncated inside the echo: zero padding must not match a zero header.
        let zeros = [0u8; 64];
        assert!(!accept(&[0x00, 0x00], 2, 2, &zeros, 1));
        // Reply to another register.
        assert!(!accept(&[0x03, 0x8c], 2, 4, &buf, 64));
    }
}
