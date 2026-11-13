//! Shared, thread-safe access to one Bragi command interface.
//!
//! A wired device owns its bus alone. A wireless receiver's bus is shared by
//! every device paired to it (one opendevicecontrol device, and so one worker thread,
//! per slot): the mutex serialises their request/reply exchanges, and
//! unsolicited packets (notifications, key reports) read while waiting for a
//! reply are queued for the slot they belong to.

use super::proto::{self, Reply};
use crate::{Error, Result};
use hidapi::HidDevice;
use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

const QUEUE_MAX: usize = 64;
const READ_BUF: usize = 1025;
/// First byte of headset notification reports (replies start with 0x02).
const HEADSET_NOTIFY: u8 = 0x03;

struct Inner {
    dev: HidDevice,
    /// Vendor event interfaces (key bitmaps, notifications).
    events: Vec<HidDevice>,
    queued: [VecDeque<Vec<u8>>; 8],
    last_receiver_ping: Instant,
}

pub struct Bus {
    inner: Mutex<Inner>,
    /// Bragi packet size (64, 128 or 1024; 63 on headsets, whose packets
    /// carry a leading 0x02 byte).
    pub size: usize,
    pub path: CString,
    /// Headset framing: every packet is prefixed with this byte.
    prefix: Option<u8>,
    /// Event packets carry their slot in byte 0 (receivers).
    route_events: bool,
    timeout: Duration,
    /// Sent when the bus is dropped (receiver back to hardware mode).
    on_drop: Mutex<Option<Vec<u8>>>,
}

impl Bus {
    pub fn new(dev: HidDevice, size: usize, path: CString, prefix: Option<u8>) -> Self {
        Self {
            inner: Mutex::new(Inner {
                dev,
                events: Vec::new(),
                queued: Default::default(),
                last_receiver_ping: Instant::now(),
            }),
            size,
            path,
            prefix,
            route_events: false,
            timeout: Duration::from_millis(500),
            on_drop: Mutex::new(None),
        }
    }

    /// Attach vendor event interfaces; `route` = byte 0 is the slot.
    pub fn with_events(mut self, events: Vec<HidDevice>, route: bool) -> Self {
        self.inner.get_mut().unwrap_or_else(|e| e.into_inner()).events = events;
        self.route_events = route;
        self
    }

    pub fn set_on_drop(&self, packet: Option<Vec<u8>>) {
        *self.on_drop.lock().unwrap_or_else(|e| e.into_inner()) = packet;
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves no broken invariant.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn write_locked(&self, inner: &Inner, packet: &[u8]) -> Result<()> {
        // hidapi: byte 0 is the report id (0, unnumbered).
        let mut out = Vec::with_capacity(packet.len() + 2);
        out.push(0);
        if let Some(p) = self.prefix {
            out.push(p);
        }
        out.extend_from_slice(packet);
        inner.dev.write(&out)?;
        Ok(())
    }

    /// Strip headset framing. Returns the Bragi packet and whether it is a
    /// notification.
    fn unframe<'a>(&self, buf: &'a [u8]) -> Option<&'a [u8]> {
        match self.prefix {
            None => Some(buf),
            Some(p) if buf.first() == Some(&p) || buf.first() == Some(&HEADSET_NOTIFY) => Some(&buf[1..]),
            Some(_) => None,
        }
    }

    /// Queue an unsolicited packet for its slot.
    fn stash(inner: &mut Inner, slot: u8, buf: &[u8]) {
        let q = &mut inner.queued[slot as usize & 7];
        if q.len() >= QUEUE_MAX {
            q.pop_front();
        }
        q.push_back(buf.to_vec());
    }

    fn stash_command_packet(inner: &mut Inner, buf: &[u8]) {
        if let Some(slot) = buf.first().and_then(|&b| proto::slot_of(b)) {
            Self::stash(inner, slot, buf);
        }
    }

    /// Wait up to `wait` for the reply to (`b0`, `cmd`).
    fn await_reply(&self, inner: &mut Inner, b0: u8, cmd: u8, wait: Duration) -> Result<(Option<Vec<u8>>, bool)> {
        let deadline = Instant::now() + wait;
        let mut saw_any = false;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok((None, saw_any));
            }
            let mut buf = vec![0u8; READ_BUF];
            let n = inner.dev.read_timeout(&mut buf, left.as_millis().max(1) as i32)?;
            if n == 0 {
                return Ok((None, saw_any));
            }
            buf.truncate(n);
            saw_any = true;
            let Some(pkt) = self.unframe(&buf) else { continue };
            if pkt.len() >= 3 && pkt[0] == b0 && pkt[1] == cmd {
                return Ok((Some(pkt.to_vec()), true));
            }
            let pkt = pkt.to_vec();
            Self::stash_command_packet(inner, &pkt);
        }
    }

    /// Send a command and wait for the reply that echoes its header and
    /// command bytes. Returns the raw reply (without headset framing).
    pub fn transact(&self, packet: &[u8]) -> Result<Vec<u8>> {
        let (b0, cmd) = (packet[0], packet[1]);
        let mut inner = self.lock();
        let mut saw_reply = false;
        for _ in 0..2 {
            self.write_locked(&inner, packet)?;
            let (reply, saw) = self.await_reply(&mut inner, b0, cmd, self.timeout)?;
            saw_reply |= saw;
            if let Some(r) = reply {
                return Ok(r);
            }
        }
        if saw_reply {
            Err(Error::Unsupported { cmd })
        } else {
            Err(Error::NoReply { cmd })
        }
    }

    /// Send a packet the device may or may not acknowledge (`07`
    /// continuation writes); waits briefly for a reply but never fails on
    /// a missing one.
    pub fn send_unacked(&self, packet: &[u8]) -> Result<()> {
        let mut inner = self.lock();
        self.write_locked(&inner, packet)?;
        self.await_reply(&mut inner, packet[0], packet[1], Duration::from_millis(15))?;
        Ok(())
    }

    /// Like [`transact`](Self::transact), but parsed and with a non-zero
    /// status turned into an error.
    pub fn command(&self, packet: &[u8]) -> Result<Reply> {
        let r = self.command_status(packet)?;
        if r.status != proto::STATUS_OK {
            return Err(Error::Unsupported { cmd: packet[1] });
        }
        Ok(r)
    }

    /// Parsed reply, whatever its status.
    pub fn command_status(&self, packet: &[u8]) -> Result<Reply> {
        let raw = self.transact(packet)?;
        proto::parse_reply(&raw).ok_or(Error::NoReply { cmd: packet[1] })
    }

    /// Read whatever is waiting on the interfaces (non-blocking) and return
    /// the unsolicited packets queued for `slot`.
    pub fn take_unsolicited(&self, slot: u8) -> Vec<Vec<u8>> {
        let mut inner = self.lock();
        let mut buf = vec![0u8; READ_BUF];
        for _ in 0..32 {
            match inner.dev.read_timeout(&mut buf, 0) {
                Ok(n) if n > 0 => {
                    if let Some(pkt) = self.unframe(&buf[..n]) {
                        let pkt = pkt.to_vec();
                        Self::stash_command_packet(&mut inner, &pkt);
                    }
                }
                _ => break,
            }
        }
        for i in 0..inner.events.len() {
            for _ in 0..32 {
                let n = match inner.events[i].read_timeout(&mut buf, 0) {
                    Ok(n) if n > 0 => n,
                    _ => break,
                };
                let Some(pkt) = self.unframe(&buf[..n]) else { continue };
                if pkt.len() < 2 {
                    continue;
                }
                let target = if self.route_events { pkt[0] & 7 } else { 0 };
                let pkt = pkt.to_vec();
                Self::stash(&mut inner, target, &pkt);
            }
        }
        inner.queued[slot as usize & 7].drain(..).collect()
    }

    /// Ping the receiver itself if it has not been pinged for `every`.
    pub fn keepalive_receiver(&self, every: Duration) {
        let due = {
            let mut inner = self.lock();
            let due = inner.last_receiver_ping.elapsed() >= every;
            if due {
                inner.last_receiver_ping = Instant::now();
            }
            due
        };
        if due {
            if let Err(e) = self.transact(&proto::ping(self.size, 0)) {
                log::debug!("receiver keepalive: {e}");
            }
        }
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let packet = self.on_drop.get_mut().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(p) = packet {
            let _ = self.transact(&p);
        }
    }
}

/// Receivers that are currently open, so a rescan can find newly connected
/// devices behind a receiver whose handle is already held.
fn registry() -> &'static Mutex<Vec<Weak<Bus>>> {
    static R: OnceLock<Mutex<Vec<Weak<Bus>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn find_open(path: &CStr) -> Option<Arc<Bus>> {
    let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
    r.retain(|w| w.strong_count() > 0);
    r.iter().filter_map(Weak::upgrade).find(|b| b.path.as_c_str() == path)
}

pub fn register(bus: &Arc<Bus>) {
    let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
    r.retain(|w| w.strong_count() > 0);
    r.push(Arc::downgrade(bus));
}
