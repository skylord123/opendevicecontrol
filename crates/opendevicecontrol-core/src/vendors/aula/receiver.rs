//! The 2.4 GHz receiver link (`3554:fa09`, report 0x13; see [`super::proto`]).

use super::keyboard::{Link, Sector};
use super::proto::{self, Ack, Config, Packet, Reassembly};
use crate::model::Rgb;
use crate::{Error, Result};
use std::time::{Duration, Instant};

/// Most distinct colours per frame. Each group costs 4 bytes on top of one
/// byte per lit key; a full keyboard stays near 10 packets.
const MAX_GROUPS: usize = 12;
/// Gap before every packet. The receiver loses packets sent closer
/// together; the official driver waits exactly this long (busy-waiting on a
/// high-resolution clock) before each one, with one report in flight.
const PACKET_GAP: Duration = Duration::from_millis(10);
/// How often to ask the receiver whether the keyboard is awake.
const ONLINE_CHECK: Duration = Duration::from_secs(2);
/// How often to read the battery level.
const BATTERY_CHECK: Duration = Duration::from_secs(30);
/// The official driver waits this long for an ack (KB.ini `IC2481=1`).
const ACK_TIMEOUT: Duration = Duration::from_secs(7);
const READ_TIMEOUT: Duration = Duration::from_secs(1);
/// A 512-byte read is 36–37 packets.
const SECTOR_TIMEOUT: Duration = Duration::from_secs(4);

pub struct Receiver {
    dev: hidapi::HidDevice,
    id: String,
    online: bool,
    reconnected: bool,
    next_online_check: Instant,
    /// When the last packet was written (for [`PACKET_GAP`]).
    last_write: Instant,
    battery: Option<(u8, u8)>,
    next_battery_check: Instant,
    settings_changed: bool,
}

fn timeout_ms(d: Duration) -> i32 {
    d.as_millis().min(i32::MAX as u128) as i32
}

impl Receiver {
    pub fn new(dev: hidapi::HidDevice, id: String, online: bool) -> Self {
        let now = Instant::now();
        Self { dev, id, online, reconnected: false, next_online_check: now, last_write: now, battery: None, next_battery_check: now, settings_changed: false }
    }

    /// Ask the keyboard for its model id (`None` if it is asleep or away).
    pub fn query_model(dev: &hidapi::HidDevice) -> Option<u8> {
        for _ in 0..3 {
            dev.write(&proto::request(proto::CMD_MODEL)).ok()?;
            let end = Instant::now() + Duration::from_millis(300);
            let mut buf = [0u8; 64];
            while Instant::now() < end {
                match dev.read_timeout(&mut buf, 50) {
                    Ok(n) if n > 0 => {
                        if let Some(m) = proto::parse_model(&buf[..n]) {
                            return Some(m);
                        }
                    }
                    Ok(_) => {}
                    Err(_) => return None,
                }
            }
        }
        None
    }

    /// Write one packet, [`PACKET_GAP`] after the previous one.
    fn write(&mut self, p: &Packet) -> Result<()> {
        let due = self.last_write + PACKET_GAP;
        // Sleep most of the wait, then spin the last stretch for accuracy
        // (sleep granularity is about 1 ms at best, 15.6 ms on old Windows).
        if let Some(left) = due.checked_duration_since(Instant::now()) {
            if left > Duration::from_millis(2) {
                std::thread::sleep(left - Duration::from_millis(2));
            }
            while Instant::now() < due {
                std::hint::spin_loop();
            }
        }
        // hidapi's write waits for the report to be sent: one in flight.
        self.dev.write(p)?;
        self.last_write = Instant::now();
        Ok(())
    }

    /// Read one report (waiting at most `wait`), tracking online events.
    fn read(&mut self, wait: Duration) -> Result<Option<Vec<u8>>> {
        let mut buf = [0u8; 64];
        let n = self.dev.read_timeout(&mut buf, timeout_ms(wait))?;
        if n == 0 {
            return Ok(None);
        }
        let r = buf[..n].to_vec();
        if let Some(b) = proto::parse_battery(&r) {
            self.battery = Some(b);
        }
        self.settings_changed |= proto::parse_settings_changed(&r);
        if let Some(on) = proto::parse_online(&r) {
            if on != self.online {
                log::info!("{}: keyboard {}", self.id, if on { "online" } else { "asleep or away" });
                self.reconnected |= on;
                if on {
                    self.next_battery_check = Instant::now();
                }
            }
            self.online = on;
        }
        Ok(Some(r))
    }

    fn drain(&mut self) -> Result<()> {
        while self.read(Duration::ZERO)?.is_some() {}
        Ok(())
    }
}

impl Link for Receiver {
    fn poll(&mut self) -> Result<()> {
        self.drain()?;
        if Instant::now() >= self.next_online_check {
            self.next_online_check = Instant::now() + ONLINE_CHECK;
            self.write(&proto::request(proto::CMD_ONLINE))?;
            let end = Instant::now() + Duration::from_millis(200);
            while let Some(left) = end.checked_duration_since(Instant::now()) {
                match self.read(left)? {
                    Some(r) if r.get(1) == Some(&proto::CMD_ONLINE) => break,
                    Some(_) => {}
                    None => break,
                }
            }
        }
        if self.online && Instant::now() >= self.next_battery_check {
            self.next_battery_check = Instant::now() + BATTERY_CHECK;
            self.write(&proto::request(proto::CMD_BATTERY))?;
            let end = Instant::now() + Duration::from_millis(200);
            while let Some(left) = end.checked_duration_since(Instant::now()) {
                match self.read(left)? {
                    Some(r) if r.get(1) == Some(&proto::CMD_BATTERY) => break,
                    Some(_) => {}
                    None => break,
                }
            }
        }
        Ok(())
    }

    fn online(&self) -> bool {
        self.online
    }

    fn battery(&self) -> Option<(u8, u8)> {
        self.battery.filter(|_| self.online)
    }

    fn take_settings_changed(&mut self) -> bool {
        std::mem::take(&mut self.settings_changed)
    }

    fn take_reconnected(&mut self) -> bool {
        std::mem::take(&mut self.reconnected)
    }

    fn read_config(&mut self) -> Result<Option<Config>> {
        self.drain()?;
        for _ in 0..3 {
            self.write(&proto::request(proto::CMD_READ_CONFIG))?;
            let mut asm = Reassembly::new(proto::CMD_READ_CONFIG);
            let end = Instant::now() + READ_TIMEOUT;
            while let Some(left) = end.checked_duration_since(Instant::now()) {
                let Some(r) = self.read(left.min(Duration::from_millis(100)))? else { continue };
                if let Some(data) = asm.push(&r) {
                    if let Some(c) = Config::parse(&data) {
                        return Ok(Some(c));
                    }
                    break; // bad magic: ask again
                }
            }
        }
        Ok(None)
    }

    fn write_config(&mut self, c: &Config) -> Result<()> {
        self.write_acked(proto::CMD_WRITE_CONFIG, &c.packets())
    }

    fn read_sector(&mut self, s: Sector) -> Result<Vec<u8>> {
        let (cmd, sub) = match s {
            Sector::Layer(l) if l <= 3 => (proto::CMD_READ_LAYER, l),
            Sector::Macros(p) if p <= 7 => (proto::CMD_READ_MACROS, p),
            _ => return Err(Error::NotSupported(": out of range")),
        };
        self.drain()?;
        for _ in 0..3 {
            self.write(&proto::request_sub(cmd, sub))?;
            let mut asm = Reassembly::new(cmd);
            let end = Instant::now() + SECTOR_TIMEOUT;
            while let Some(left) = end.checked_duration_since(Instant::now()) {
                let Some(r) = self.read(left.min(Duration::from_millis(100)))? else { continue };
                if let Some(data) = asm.push(&r) {
                    return Ok(data);
                }
            }
        }
        Err(Error::Offline)
    }

    fn write_sector(&mut self, s: Sector, data: &[u8]) -> Result<()> {
        let (cmd, sub) = match s {
            Sector::Layer(l) if l <= 3 => (proto::CMD_WRITE_LAYER, l),
            Sector::Macros(p) if p <= 7 => (proto::CMD_WRITE_MACROS, p),
            _ => return Err(Error::NotSupported(": out of range")),
        };
        if data.len() != 512 {
            return Err(Error::InvalidValue("sector length".into()));
        }
        self.write_acked(cmd, &proto::transfer(cmd, sub, data))
    }

    fn show(&mut self, leds: &[(u8, Rgb)], uniform: Option<Rgb>) -> Result<bool> {
        self.show_frame(leds, uniform)
    }
}

impl Receiver {
    /// Write packets the keyboard acknowledges one by one.
    fn write_acked(&mut self, cmd: u8, packets: &[Packet]) -> Result<()> {
        self.drain()?;
        for (i, p) in packets.iter().enumerate() {
            let mut tries = 0;
            'packet: loop {
                tries += 1;
                self.write(p)?;
                let end = Instant::now() + ACK_TIMEOUT;
                while let Some(left) = end.checked_duration_since(Instant::now()) {
                    let Some(r) = self.read(left.min(Duration::from_millis(100)))? else { continue };
                    match proto::ack(&r, cmd, i as u8) {
                        Ack::Ok => break 'packet,
                        Ack::Bad if tries < 3 => continue 'packet,
                        Ack::Bad => return Err(Error::Unsupported { cmd }),
                        Ack::No => {}
                    }
                }
                return Err(Error::Offline);
            }
        }
        Ok(())
    }
}

impl Receiver {
    fn show_frame(&mut self, leds: &[(u8, Rgb)], uniform: Option<Rgb>) -> Result<bool> {
        // One colour everywhere fits a single packet; per-key frames take
        // about nine, which the 2.4 GHz link delivers far more slowly.
        let frame = match uniform {
            Some(c) => vec![proto::solid(c)],
            None => {
                // Fewer colour groups until the frame fits the keyboard's buffer.
                let mut groups = MAX_GROUPS;
                loop {
                    let g = proto::group_colors(leds, groups);
                    let bytes: usize = g.iter().map(|(_, l)| 4 + l.len()).sum();
                    if bytes <= proto::MAX_LIVE_PAYLOAD || groups <= 1 {
                        break proto::per_key(&g);
                    }
                    groups -= 1;
                }
            }
        };
        for p in &frame {
            self.write(p)?;
        }
        Ok(true)
    }
}
