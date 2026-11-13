//! The USB cable link (`258a:010c`, 520-byte feature reports; see
//! [`super::proto::wired`]).

use super::keyboard::{Link, Sector};
use super::proto::{wired, Config};
use crate::model::Rgb;
use crate::{Error, Result};
use std::time::{Duration, Instant};

pub struct Wired {
    dev: hidapi::HidDevice,
    /// The collection carrying input report 3 (host keys, "settings
    /// changed"); on Linux the same interface node as `dev`.
    events: Option<hidapi::HidDevice>,
    /// Linux: the keyboard interface (0), whose key reports hidraw also
    /// gets, so typing is seen (see [`TYPING_HOLD`]). Windows keeps
    /// keyboards to itself; [`crate::typing`] uses Raw Input there.
    keys: Option<hidapi::HidDevice>,
    battery: Option<(u8, u8)>,
    next_battery_check: Instant,
    settings_changed: bool,
    host: Vec<Option<u16>>,
    /// When the last lighting frame was started.
    last_show: Option<Instant>,
}

/// How often to read the battery level.
const BATTERY_CHECK: Duration = Duration::from_secs(30);
/// While lighting frames stream, the battery read (which waits behind a
/// 67 ms frame and stalls the next one) is put off, up to this long.
const BATTERY_DEFER: Duration = Duration::from_secs(300);
/// Least time between lighting frames. The keyboard's control endpoint
/// takes 8-byte packets, and its firmware receives all 65 of a 520-byte
/// report inside its USB interrupt, busy-waiting about 67 ms with key
/// handling stalled (V14 firmware, 0xADD5). Frames back to back left it
/// little time for keys; the official driver starts one about every 95 ms.
pub const FRAME_GAP: Duration = Duration::from_millis(100);
/// The service renders frames at [`FRAME_GAP`]; allow for timer jitter.
const FRAME_SLACK: Duration = Duration::from_millis(15);
/// While keys were pressed this recently, hold lighting frames back: each
/// one stalls the keyboard's key handling for about 67 ms.
const TYPING_HOLD: Duration = Duration::from_millis(400);
/// But still send one this often while typing: the lights keep reacting,
/// and about one key in eight lands in a frame and waits up to 67 ms.
/// (Under 2 s also keeps the keyboard in live mode.)
const TYPING_KEEPALIVE: Duration = Duration::from_millis(500);

/// Send a read request and fetch its answer.
fn read(dev: &hidapi::HidDevice, request: &[u8]) -> Result<Vec<u8>> {
    dev.send_feature_report(request)?;
    std::thread::sleep(Duration::from_millis(10));
    let mut buf = vec![0u8; wired::LEN];
    buf[0] = wired::REPORT_ID;
    let n = dev.get_feature_report(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

impl Wired {
    pub fn new(dev: hidapi::HidDevice, events: Option<hidapi::HidDevice>, keys: Option<hidapi::HidDevice>) -> Self {
        for h in events.iter().chain(&keys) {
            let _ = h.set_blocking_mode(false);
        }
        Self { dev, events, keys, battery: None, next_battery_check: Instant::now(), settings_changed: false, host: Vec::new(), last_show: None }
    }

    /// Read input report 3 (`03 type a b`): host keys and change notices.
    fn read_events(&mut self) {
        let mut buf = [0u8; 64];
        if let Some(k) = &self.keys {
            while let Ok(n) = k.read(&mut buf) {
                if n == 0 {
                    break;
                }
                crate::typing::note_key();
            }
        }
        let Some(e) = &self.events else { return };
        while let Ok(n) = e.read(&mut buf) {
            if n == 0 {
                break;
            }
            // Linux: interface 1's node also carries its keyboard (4),
            // system (1) and consumer (2) reports.
            if cfg!(target_os = "linux") && n > 1 && buf[0] != 3 {
                crate::typing::note_key();
            }
            if n >= 4 && buf[0] == 3 {
                match buf[1] {
                    4 => {
                        let code = u16::from_be_bytes([buf[2], buf[3]]);
                        self.host.push((code != 0).then_some(code));
                    }
                    8 | 9 => self.settings_changed = true,
                    _ => {}
                }
            }
        }
    }

    fn sector_cmd(s: Sector) -> Result<(u8, u8, u8)> {
        // (command, sub, page), clamped as the firmware doesn't check.
        match s {
            Sector::Layer(l) if l <= 3 => Ok((wired::CMD_LAYER, l, 0)),
            Sector::Macros(p) if p <= 7 => Ok((wired::CMD_MACROS, 0, p)),
            _ => Err(Error::NotSupported(": out of range")),
        }
    }

    pub fn query_model(dev: &hidapi::HidDevice) -> Option<u8> {
        (0..3).find_map(|_| read(dev, &wired::model_request()).ok().and_then(|r| wired::parse_model(&r)))
    }
}

impl Link for Wired {
    fn poll(&mut self) -> Result<()> {
        self.read_events();
        let streaming = self.last_show.is_some_and(|t| t.elapsed() < Duration::from_secs(1));
        let overdue = Instant::now() >= self.next_battery_check + BATTERY_DEFER;
        if Instant::now() >= self.next_battery_check && (self.battery.is_none() || !streaming || overdue) {
            self.next_battery_check = Instant::now() + BATTERY_CHECK;
            match read(&self.dev, &wired::battery_request()) {
                Ok(r) => self.battery = wired::parse_battery(&r),
                Err(e) => log::debug!("battery read: {e}"),
            }
        }
        Ok(())
    }

    fn battery(&self) -> Option<(u8, u8)> {
        self.battery
    }

    fn take_settings_changed(&mut self) -> bool {
        std::mem::take(&mut self.settings_changed)
    }

    fn host_keys(&self) -> bool {
        self.events.is_some()
    }

    fn poll_host_keys(&mut self) -> Vec<Option<u16>> {
        self.read_events();
        std::mem::take(&mut self.host)
    }

    fn read_sector(&mut self, s: Sector) -> Result<Vec<u8>> {
        let (cmd, sub, page) = Self::sector_cmd(s)?;
        let mut req = wired::read_request(cmd, sub, 512);
        req[5] = page;
        let r = read(&self.dev, &req)?;
        if r.len() < wired::DATA + 512 || r[1] & 0x7f != cmd {
            return Err(Error::NoReply { cmd });
        }
        Ok(r[wired::DATA..wired::DATA + 512].to_vec())
    }

    fn write_sector(&mut self, s: Sector, data: &[u8]) -> Result<()> {
        let (cmd, sub, page) = Self::sector_cmd(s)?;
        if data.len() != 512 {
            return Err(Error::InvalidValue("sector length".into()));
        }
        self.dev.send_feature_report(&wired::report_page(cmd, sub, page, 512, data))?;
        // Give the keyboard time to erase and program the flash sector.
        std::thread::sleep(Duration::from_millis(100));
        Ok(())
    }

    fn read_config(&mut self) -> Result<Option<Config>> {
        for _ in 0..3 {
            if let Some(c) = wired::parse_config(&read(&self.dev, &wired::config_request())?) {
                return Ok(Some(c));
            }
        }
        Ok(None)
    }

    fn write_config(&mut self, c: &Config) -> Result<()> {
        self.dev.send_feature_report(&wired::config_write(c))?;
        // The keyboard has no ack here; read back to confirm.
        std::thread::sleep(Duration::from_millis(20));
        match self.read_config()? {
            Some(back) if back == *c => Ok(()),
            _ => Err(Error::Unsupported { cmd: wired::CMD_CONFIG }),
        }
    }

    fn show(&mut self, leds: &[(u8, Rgb)], _uniform: Option<Rgb>) -> Result<bool> {
        self.read_events(); // Linux: notice typing first
        let typing = crate::typing::since_last_key().is_some_and(|d| d < TYPING_HOLD);
        if typing && self.last_show.is_some_and(|t| t.elapsed() < TYPING_KEEPALIVE) {
            return Ok(false);
        }
        if self.last_show.is_some_and(|t| t.elapsed() + FRAME_SLACK < FRAME_GAP) {
            return Ok(false);
        }
        self.last_show = Some(Instant::now());
        self.dev.send_feature_report(&wired::lights(leds))?;
        Ok(true)
    }
}
