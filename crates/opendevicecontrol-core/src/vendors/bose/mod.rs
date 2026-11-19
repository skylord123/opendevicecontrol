//! Bose headphones over Bluetooth (BMAP, the protocol of the Bose Music
//! app). Tested with the Bose QuietComfort Headphones (2023, product id
//! 0x4075); other BMAP products should work for the functions they report.
//!
//! The headphones are found through the OS Bluetooth stack (paired and
//! connected), then reached over their Serial Port service. Everything the
//! driver shows is read from the headphones, and they push changes made on
//! the headphones themselves once notifications are enabled, so the state
//! here follows the buttons too. See
//! docs/reverse-engineering/bose-qc-headphones.md.

pub mod proto;
pub mod rfcomm;

use crate::devices::{Device, Ids};
use crate::model::*;
use crate::{Error, Result};
use proto::{fblock, Op, Packet};
use rfcomm::Socket;
use std::collections::HashMap;
use std::ffi::CString;
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Most replies come within ~100 ms; Start functions answer Processing
/// first and take longer.
const REPLY_TIMEOUT: Duration = Duration::from_millis(1500);
/// Start functions (connect, disconnect, pairing) can take a while.
const START_TIMEOUT: Duration = Duration::from_secs(15);
/// The headphones push mode changes, but not settings another connected
/// app changes (EQ, self voice…), so everything is re-read this often.
const REFRESH: Duration = Duration::from_secs(60);
/// Spotify Tap (ActionButtonMode 16): presses then arrive on the TAP service.
const SPOTIFY_TAP: u8 = 16;
/// Wait this long before trying the TAP service again after a failure.
const TAP_RETRY: Duration = Duration::from_secs(30);
/// The input key the shortcut button reports.
const SHORTCUT_KEY: &str = "shortcut";

/// The link is gone after this many failed exchanges in a row.
const MAX_FAILURES: u32 = 3;

/// Function blocks the Bose app asks updates for (`RW8.b`), if supported.
const NOTIFY: [u8; 9] = [0, 1, 2, 4, 5, 9, 16, 18, 31];

/// What the headphones last told us, keyed by (fblock, function).
type State = HashMap<(u8, u8), Vec<u8>>;

pub struct Headphones {
    info: DeviceInfo,
    sock: Socket,
    buf: Vec<u8>,
    state: State,
    /// Function blocks the headphones support (`ProductInfo.AllFblocks`).
    fblocks: u32,
    /// Functions that answered a Get (others are not shown).
    supported: Vec<(u8, u8)>,
    modes: modes::Modes,
    failures: u32,
    refreshed: Instant,
    /// The headphones' paired devices, in their order.
    paired: Vec<proto::devices::Info>,
    pairing: Option<bool>,
    /// A connection changed: re-read the paired list on the next poll.
    devices_dirty: bool,
    address: [u8; 6],
    /// The shortcut-press channel, open while the button is set to Spotify Tap.
    tap: Option<Socket>,
    tap_buf: Vec<u8>,
    tap_retry: Option<Instant>,
    input: Vec<InputEvent>,
}

/// Functions read at start and on refresh, when the headphones have them.
const READ: [(u8, u8); 18] = [
    (fblock::STATUS, proto::status::IN_EAR),
    (fblock::STATUS, proto::status::CASE_LID),
    (fblock::AUDIO_MANAGEMENT, proto::audio::SPATIAL),
    (16, proto::vpa::ASSISTANT),
    (16, proto::vpa::WAKE_WORD),
    (fblock::AUDIO_MANAGEMENT, 1),
    (fblock::SETTINGS, proto::settings::PRODUCT_NAME),
    (fblock::SETTINGS, proto::settings::VOICE_PROMPTS),
    (fblock::SETTINGS, proto::settings::STANDBY_TIMER),
    (fblock::SETTINGS, proto::settings::CNC),
    (fblock::SETTINGS, proto::settings::RANGE_CONTROL),
    (fblock::SETTINGS, proto::settings::BUTTONS),
    (fblock::SETTINGS, proto::settings::MULTIPOINT),
    (fblock::SETTINGS, proto::settings::SIDETONE),
    (fblock::STATUS, proto::status::BATTERY_LEVEL),
    (fblock::STATUS, proto::status::CHARGER),
    (fblock::AUDIO_MANAGEMENT, proto::audio::VOLUME),
    (fblock::AUDIO_MANAGEMENT, proto::audio::STATUS),
];

/// Everything read at start: the fixed list plus the settings other
/// models have (only those that answer are kept).
fn reads() -> Vec<(u8, u8)> {
    use proto::settings as s;
    let mut v = READ.to_vec();
    v.extend(s::TOGGLES.iter().map(|t| (fblock::SETTINGS, t.0)));
    v.extend([s::ANR, s::ON_HEAD, s::TEAMS_BUTTON, s::VOLUME_CONTROL].map(|f| (fblock::SETTINGS, f)));
    v
}

impl Headphones {
    fn open(address: [u8; 6], name: &str, id: String) -> Result<Headphones> {
        // SDP first (any BMAP product); the QC Headphones use channel 8.
        let sock = match Socket::connect_service(&address, &rfcomm::SPP, CONNECT_TIMEOUT) {
            Ok(s) => s,
            Err(e) => {
                log::debug!("{}: SDP lookup failed ({e}); trying channel 8", rfcomm::format_address(&address));
                Socket::connect(&address, 8, CONNECT_TIMEOUT)?
            }
        };
        let mut h = Headphones {
            info: DeviceInfo {
                id,
                kind: DeviceKind::Headset,
                manufacturer: "Bose".into(),
                model: name.trim_start_matches("LE-").trim_start_matches("Bose ").to_string(),
                vendor_string: None,
                product_string: Some(name.to_string()),
                usb_vid: 0,
                usb_pid: 0,
                serial: None,
                firmware: None,
            },
            sock,
            buf: Vec::new(),
            state: State::new(),
            fblocks: 0,
            supported: Vec::new(),
            modes: modes::Modes::default(),
            failures: 0,
            refreshed: Instant::now(),
            paired: Vec::new(),
            pairing: None,
            devices_dirty: false,
            address,
            tap: None,
            tap_buf: Vec::new(),
            tap_retry: None,
            input: Vec::new(),
        };
        // Only BMAP products answer the version query this way.
        let version = h.request(&Packet::get(fblock::PRODUCT_INFO, 1))?;
        log::info!("{name}: BMAP {}", String::from_utf8_lossy(&version));
        h.fblocks = h.request(&Packet::get(fblock::PRODUCT_INFO, 2)).map(|p| proto::be32(&p)).unwrap_or(u32::MAX);
        if let Ok(p) = h.request(&Packet::get(fblock::PRODUCT_INFO, 3)) {
            if p.len() >= 2 {
                h.info.usb_vid = 0x009e;
                h.info.usb_pid = u16::from_be_bytes([p[0], p[1]]);
            }
            h.info.model = model_name(h.info.usb_pid).map(str::to_string).unwrap_or(h.info.model);
            if is_speaker(h.info.usb_pid) {
                h.info.kind = DeviceKind::Other;
            }
        }
        h.info.firmware = h.request(&Packet::get(fblock::PRODUCT_INFO, 5)).ok().map(|p| String::from_utf8_lossy(&p).to_string());
        h.info.serial = h.request(&Packet::get(fblock::PRODUCT_INFO, 7)).ok().map(|p| String::from_utf8_lossy(&p).trim_end_matches('\0').to_string());
        // Ask for updates, as the app does right after connecting.
        let wanted: Vec<u8> = NOTIFY.iter().copied().filter(|&f| h.has_fblock(f)).collect();
        if h.has_fblock(9) {
            if let Err(e) = h.request(&Packet::set_get(9, proto::notify::BY_FBLOCK, &proto::notify::enable(&wanted))) {
                log::info!("{name}: couldn't enable notifications ({e}); changes on the headphones show on refresh only");
            }
        }
        for (f, func) in reads() {
            if h.has_fblock(f) && h.request(&Packet::get(f, func)).is_ok() {
                h.supported.push((f, func));
            }
        }
        if h.has_fblock(fblock::AUDIO_MODES) {
            h.modes = modes::Modes::read(&mut h);
        }
        h.read_devices();
        h.failures = 0;
        Ok(h)
    }

    fn has_fblock(&self, f: u8) -> bool {
        f < 32 && self.fblocks & (1 << f) != 0
    }

    /// Read what has arrived (waiting up to `wait`), store every Status.
    fn pump(&mut self, wait: Duration) -> Result<Vec<Packet>> {
        let mut chunk = [0u8; 1024];
        let n = self.sock.recv(&mut chunk, wait)?;
        self.buf.extend_from_slice(&chunk[..n]);
        let (packets, used) = proto::decode(&self.buf);
        self.buf.drain(..used);
        for p in &packets {
            log::debug!("bose rx {:02}.{:02} {:?} {:02x?}", p.fblock, p.function, p.op, p.payload);
            // Sources connecting and dropping (also unprompted).
            if p.fblock == fblock::DEVICE_MANAGEMENT && matches!(p.function, proto::devices::CONNECT | proto::devices::DISCONNECT) && p.op != Op::Error {
                self.devices_dirty = true;
            }
            if p.fblock == fblock::DEVICE_MANAGEMENT && p.function == proto::devices::PAIRING_MODE && matches!(p.op, Op::Status | Op::Result) {
                self.pairing = Some(proto::devices::pairing(&p.payload));
            }
            if p.op == Op::Status {
                self.state.insert((p.fblock, p.function), p.payload.clone());
                if p.fblock == fblock::AUDIO_MODES {
                    self.modes.update(p);
                }
            }
        }
        Ok(packets)
    }

    /// Send `p` and wait for its answer: the payload of the Status (or
    /// Result) for the same function, or the Error.
    fn request(&mut self, p: &Packet) -> Result<Vec<u8>> {
        let r = self.exchange(p);
        match &r {
            Err(Error::Offline) | Err(Error::NoReply { .. }) | Err(Error::Other(_)) => self.failures += 1,
            _ => self.failures = 0,
        }
        r
    }

    /// The paired list, each device's name and state, and pairing mode.
    fn read_devices(&mut self) {
        use proto::devices as d;
        if !self.has_fblock(fblock::DEVICE_MANAGEMENT) {
            return;
        }
        let Ok(list) = self.request(&Packet::get(fblock::DEVICE_MANAGEMENT, d::LIST)) else { return };
        let mut paired = Vec::new();
        for (address, connected) in d::list(&list) {
            let info = self.request(&Packet::new(fblock::DEVICE_MANAGEMENT, d::INFO, Op::Get, &address)).ok().and_then(|p| d::info(&p));
            paired.push(info.unwrap_or(d::Info { address, connected, local: false, name: String::new() }));
        }
        self.paired = paired;
        if let Ok(p) = self.request(&Packet::get(fblock::DEVICE_MANAGEMENT, d::PAIRING_MODE)) {
            self.pairing = Some(d::pairing(&p));
        }
        self.devices_dirty = false;
    }

    /// The shortcut button can send presses to us (it offers Spotify Tap).
    fn has_tap(&self) -> bool {
        self.get(fblock::SETTINGS, proto::settings::BUTTONS).and_then(proto::settings::buttons).is_some_and(|b| b.mode == SPOTIFY_TAP || b.supported & (1 << SPOTIFY_TAP) != 0)
    }

    fn wants_tap(&self) -> bool {
        self.get(fblock::SETTINGS, proto::settings::BUTTONS).and_then(proto::settings::buttons).is_some_and(|b| b.mode == SPOTIFY_TAP)
    }

    /// Open or close the press channel to match the button's setting.
    fn manage_tap(&mut self) {
        if !self.wants_tap() {
            if self.tap.take().is_some() {
                log::debug!("{}: shortcut no longer on Spotify Tap; stopped listening for presses", self.info.model);
            }
            return;
        }
        if self.tap.is_some() || self.tap_retry.is_some_and(|t| t.elapsed() < TAP_RETRY) {
            return;
        }
        match Socket::connect_service(&self.address, &rfcomm::TAP, CONNECT_TIMEOUT) {
            Ok(s) => {
                log::info!("{}: listening for shortcut button presses", self.info.model);
                self.tap = Some(s);
                self.tap_buf.clear();
                self.tap_retry = None;
            }
            Err(e) => {
                log::info!("{}: can't listen for shortcut presses: {e}", self.info.model);
                self.tap_retry = Some(Instant::now());
            }
        }
    }

    /// Turn what arrived on the press channel into key presses. Each press
    /// is one message `[01, length, client id, 0, model, 0, maker, 0]`.
    fn read_tap(&mut self) {
        let Some(sock) = &self.tap else { return };
        let mut chunk = [0u8; 512];
        match sock.recv(&mut chunk, Duration::ZERO) {
            Ok(n) => self.tap_buf.extend_from_slice(&chunk[..n]),
            Err(e) => {
                log::info!("{}: shortcut press channel closed ({e})", self.info.model);
                self.tap = None;
                self.tap_retry = Some(Instant::now());
                return;
            }
        }
        while self.tap_buf.len() >= 2 && self.tap_buf.len() >= 2 + self.tap_buf[1] as usize {
            let n = 2 + self.tap_buf[1] as usize;
            log::debug!("{}: shortcut press {:02x?}", self.info.model, &self.tap_buf[..n.min(8)]);
            self.tap_buf.drain(..n);
            self.input.push(InputEvent::Pressed(SHORTCUT_KEY.into()));
            self.input.push(InputEvent::Released(SHORTCUT_KEY.into()));
        }
    }

    fn device_label(&self, i: &proto::devices::Info) -> String {
        let name = if i.name.is_empty() { rfcomm::format_address(&i.address) } else { i.name.clone() };
        let source = self.get(fblock::AUDIO_MANAGEMENT, proto::audio::SOURCE).and_then(proto::audio::source);
        let mut tags = Vec::new();
        if i.local {
            tags.push("this computer");
        }
        if source == Some(i.address) {
            tags.push("current source");
        }
        if tags.is_empty() { name } else { format!("{name} ({})", tags.join(", ")) }
    }

    fn exchange(&mut self, p: &Packet) -> Result<Vec<u8>> {
        self.sock.send(&p.encode())?;
        let end = Instant::now() + if p.op == Op::Start { START_TIMEOUT } else { REPLY_TIMEOUT };
        while Instant::now() < end {
            for r in self.pump(end.saturating_duration_since(Instant::now()))? {
                if r.fblock != p.fblock || r.function != p.function {
                    continue;
                }
                match r.op {
                    Op::Status | Op::Result => return Ok(r.payload),
                    Op::Error => {
                        let code = r.error().unwrap_or(0);
                        let code = if code == 0xff { r.payload.get(1).copied().unwrap_or(0) } else { code };
                        return Err(if code == 4 || code == 5 {
                            Error::NotSupported("")
                        } else {
                            Error::Other(format!("headphones refused: {}", proto::error_name(code)))
                        });
                    }
                    _ => {}
                }
            }
        }
        Err(Error::NoReply { cmd: p.function })
    }

    fn get(&self, f: u8, func: u8) -> Option<&[u8]> {
        self.state.get(&(f, func)).map(|v| v.as_slice())
    }

    fn controls(&self) -> Vec<Control> {
        use proto::settings as s;
        let mut out = Vec::new();
        out.extend(self.modes.controls());
        if let Some(c) = self.get(fblock::SETTINGS, s::CNC).and_then(s::cnc) {
            // With modes, noise cancelling belongs to the current mode.
            if !self.modes.present() && c.steps > 1 {
                out.push(Control {
                    id: "noise_cancelling".into(),
                    name: "Noise cancelling".into(),
                    kind: ControlKind::Number { min: 0.0, max: (c.steps - 1) as f64, step: 1.0, unit: String::new() },
                    value: ControlValue::Number(proto::settings::cnc_display(c.step, c.steps) as f64),
                    help: Some("0 lets the most sound in; the highest step blocks the most.".into()),
                });
            }
        }
        if let Some(bands) = self.get(fblock::SETTINGS, s::RANGE_CONTROL).map(s::range_control) {
            for b in bands {
                out.push(Control {
                    id: format!("eq_{}", b.id),
                    name: format!("EQ {}", s::band_name(b.id).to_lowercase()),
                    kind: ControlKind::Number { min: b.min as f64, max: b.max as f64, step: 1.0, unit: String::new() },
                    value: ControlValue::Number(b.value as f64),
                    help: None,
                });
            }
        }
        if let Some(b) = self.get(fblock::SETTINGS, s::BUTTONS).and_then(s::buttons) {
            let options: Vec<String> =
                (0..s::BUTTON_MODES.len() as u8).filter(|&m| m == b.mode || b.supported & (1 << m) != 0).map(|m| s::BUTTON_MODES[m as usize].to_string()).collect();
            if options.len() > 1 {
                out.push(Control {
                    id: "shortcut".into(),
                    name: "Shortcut (press and hold)".into(),
                    kind: ControlKind::Choice { options },
                    value: ControlValue::Choice(s::BUTTON_MODES.get(b.mode as usize).unwrap_or(&"Not configured").to_string()),
                    help: Some(if self.has_tap() {
                        "What pressing and holding the headphones' shortcut button does. On \"Spotify Tap\" the press also reaches OpenDeviceControl: give the Shortcut button an action on the Keys tab.".into()
                    } else {
                        "What pressing and holding the headphones' shortcut button does.".into()
                    }),
                });
            }
        }
        if let Some(v) = self.get(fblock::SETTINGS, s::VOICE_PROMPTS).and_then(s::voice_prompts) {
            out.push(Control {
                id: "voice_prompts".into(),
                name: "Voice prompts".into(),
                kind: ControlKind::Toggle,
                value: ControlValue::Bool(v.enabled),
                help: Some("Spoken announcements such as battery level and connections.".into()),
            });
            let options: Vec<String> =
                (0..s::LANGUAGES.len() as u8).filter(|&l| l == v.language || v.supported & (1 << l) != 0).map(|l| s::LANGUAGES[l as usize].to_string()).collect();
            if options.len() > 1 {
                out.push(Control {
                    id: "voice_prompt_language".into(),
                    name: "Voice prompt language".into(),
                    kind: ControlKind::Choice { options },
                    value: ControlValue::Choice(s::LANGUAGES.get(v.language as usize).unwrap_or(&"English (US)").to_string()),
                    help: None,
                });
            }
        }
        if let Some(m) = self.get(fblock::SETTINGS, s::STANDBY_TIMER).and_then(s::standby_timer) {
            let mut choices: Vec<u16> = s::STANDBY_CHOICES.to_vec();
            if !choices.contains(&m) {
                choices.insert(0, m);
            }
            out.push(Control {
                id: "auto_off".into(),
                name: "Auto-off".into(),
                kind: ControlKind::Choice { options: choices.iter().map(|&m| minutes_label(m)).collect() },
                value: ControlValue::Choice(minutes_label(m)),
                help: Some("Switch the headphones off after this long without audio.".into()),
            });
        }
        if let Some((mode, mask)) = self.get(fblock::SETTINGS, s::ANR).and_then(s::anr) {
            out.push(Control {
                id: "anr".into(),
                name: "Noise cancelling".into(),
                kind: ControlKind::Choice { options: choices(&s::ANR_MODES, mode, mask) },
                value: ControlValue::Choice(s::ANR_MODES.get(mode as usize).unwrap_or(&"High").to_string()),
                help: None,
            });
        }
        if let Some(h) = self.get(fblock::SETTINGS, s::ON_HEAD).and_then(s::on_head) {
            out.push(Control {
                id: "on_head".into(),
                name: "On-head detection".into(),
                kind: ControlKind::Toggle,
                value: ControlValue::Bool(h.enabled),
                help: Some("Sensors notice when you put the headphones on or take them off.".into()),
            });
            for (i, (id, name)) in s::ON_HEAD_FEATURES.iter().enumerate() {
                if h.features[i].0 {
                    out.push(Control { id: id.to_string(), name: name.to_string(), kind: ControlKind::Toggle, value: ControlValue::Bool(h.features[i].1), help: None });
                }
            }
        }
        for (func, id, name, help) in s::TOGGLES {
            if let Some(on) = self.get(fblock::SETTINGS, func).and_then(s::toggle) {
                out.push(Control { id: id.into(), name: name.into(), kind: ControlKind::Toggle, value: ControlValue::Bool(on), help: Some(help.into()) });
            }
        }
        if let Some(&mode) = self.get(fblock::SETTINGS, s::TEAMS_BUTTON).and_then(|p| p.first()) {
            out.push(Control {
                id: "teams_button".into(),
                name: "Teams button".into(),
                kind: ControlKind::Choice { options: s::TEAMS_MODES.iter().map(|m| m.to_string()).collect() },
                value: ControlValue::Choice(s::TEAMS_MODES.get(mode as usize).unwrap_or(&s::TEAMS_MODES[0]).to_string()),
                help: Some("What the voice assistant button does while the Bose USB Link is connected.".into()),
            });
        }
        if let Some((mode, mask)) = self.get(fblock::SETTINGS, s::VOLUME_CONTROL).and_then(s::volume_control) {
            out.push(Control {
                id: "volume_control".into(),
                name: "Volume control".into(),
                kind: ControlKind::Choice { options: choices(&s::VOLUME_CONTROL_MODES, mode, mask as u32) },
                value: ControlValue::Choice(s::VOLUME_CONTROL_MODES.get(mode as usize).unwrap_or(&"Off").to_string()),
                help: Some("How you change the volume on the headphones themselves.".into()),
            });
        }
        if let Some((on, can_disable)) = self.get(fblock::SETTINGS, s::MULTIPOINT).and_then(s::multipoint) {
            if can_disable {
                out.push(Control {
                    id: "multipoint".into(),
                    name: "Multipoint".into(),
                    kind: ControlKind::Toggle,
                    value: ControlValue::Bool(on),
                    help: Some("Stay connected to two devices at once.".into()),
                });
            }
        }
        if let Some((mode, mask)) = self.get(fblock::SETTINGS, s::SIDETONE).and_then(s::sidetone) {
            // Shown in the order Off, Low, Medium, High.
            let options: Vec<String> = [0u8, 3, 2, 1].iter().filter(|&&m| m == mode || mask & (1 << m) != 0).map(|&m| s::SIDETONE_MODES[m as usize].to_string()).collect();
            out.push(Control {
                id: "self_voice".into(),
                name: "Self voice".into(),
                kind: ControlKind::Choice { options },
                value: ControlValue::Choice(s::SIDETONE_MODES.get(mode as usize).unwrap_or(&"Off").to_string()),
                help: Some("How much of your own voice you hear during calls.".into()),
            });
        }
        if let Some(&mode) = self.get(fblock::AUDIO_MANAGEMENT, proto::audio::SPATIAL).and_then(|p| p.first()) {
            out.push(Control {
                id: "spatial_audio".into(),
                name: "Spatial audio".into(),
                kind: ControlKind::Choice { options: proto::audio::SPATIAL_MODES.iter().map(|m| m.to_string()).collect() },
                value: ControlValue::Choice(proto::audio::SPATIAL_MODES.get(mode as usize).unwrap_or(&"Off").to_string()),
                help: Some("Still: the sound stays in front of you. Motion: it moves with your head.".into()),
            });
        }
        if let Some((sel, sup)) = self.get(16, proto::vpa::ASSISTANT).and_then(proto::vpa::assistant) {
            let options: Vec<String> =
                proto::vpa::ASSISTANTS.iter().filter(|(v, _)| *v == sel || *v == 127 || (*v < 32 && sup & (1 << v) != 0)).map(|(_, n)| n.to_string()).collect();
            let current = proto::vpa::ASSISTANTS.iter().find(|(v, _)| *v == sel).map_or("None", |(_, n)| n);
            out.push(Control {
                id: "voice_assistant".into(),
                name: "Voice assistant".into(),
                kind: ControlKind::Choice { options },
                value: ControlValue::Choice(current.into()),
                help: Some("The assistant the headphones' button talks to.".into()),
            });
        }
        if let Some((_, on, read_only)) = self.get(16, proto::vpa::WAKE_WORD).and_then(proto::vpa::wake_word) {
            if !read_only {
                out.push(Control {
                    id: "wake_word".into(),
                    name: "Wake word".into(),
                    kind: ControlKind::Toggle,
                    value: ControlValue::Bool(on),
                    help: Some("Start the voice assistant by saying its name.".into()),
                });
            }
        }
        for i in &self.paired {
            out.push(Control {
                id: format!("paired_{}", rfcomm::format_address(&i.address).replace(':', "").to_lowercase()),
                name: format!("Paired devices: {}", self.device_label(i)),
                kind: ControlKind::Toggle,
                value: ControlValue::Bool(i.connected),
                help: Some(if i.local {
                    "This computer. Turning it off disconnects the headphones' audio from it.".into()
                } else {
                    "Connect or disconnect the headphones from this device.".into()
                }),
            });
        }
        if let Some(on) = self.pairing {
            out.push(Control {
                id: "pairing_mode".into(),
                name: "Paired devices: Pairing mode".into(),
                kind: ControlKind::Toggle,
                value: ControlValue::Bool(on),
                help: Some("Let a new device pair with the headphones.".into()),
            });
        }
        if let Some((max, vol)) = self.get(fblock::AUDIO_MANAGEMENT, proto::audio::VOLUME).and_then(proto::audio::volume) {
            if max > 0 {
                let source = self.get(fblock::AUDIO_MANAGEMENT, proto::audio::SOURCE).and_then(proto::audio::source);
                let source = self.paired.iter().find(|i| Some(i.address) == source).map(|i| if i.name.is_empty() { rfcomm::format_address(&i.address) } else { i.name.clone() });
                out.push(Control {
                    id: "volume".into(),
                    name: match &source {
                        Some(s) => format!("Volume ({s})"),
                        None => "Volume".into(),
                    },
                    kind: ControlKind::Number { min: 0.0, max: max as f64, step: 1.0, unit: String::new() },
                    value: ControlValue::Number(vol as f64),
                    help: None,
                });
            }
        }
        out
    }

    fn sensors(&self) -> Vec<Sensor> {
        let mut out = Vec::new();
        if let Some(p) = self.get(fblock::STATUS, proto::status::BATTERY_LEVEL) {
            for (component, percent, minutes) in proto::status::batteries(p) {
                let (id, name) = match component {
                    1 => ("right", "Right earbud"),
                    2 => ("left", "Left earbud"),
                    3 => ("case", "Case"),
                    _ => ("", ""),
                };
                let (bid, bname) = if id.is_empty() { ("battery".to_string(), "Battery".to_string()) } else { (format!("battery_{id}"), format!("{name} battery")) };
                out.push(Sensor::new(&bid, &bname, SensorKind::Battery, percent as f64));
                if let Some(m) = minutes {
                    out.push(Sensor::new(&format!("{bid}_time"), &format!("{bname} time left"), SensorKind::Duration, m as f64 * 60.0));
                }
            }
        }
        if let Some((left, right)) = self.get(fblock::STATUS, proto::status::IN_EAR).and_then(proto::status::in_ear) {
            out.push(Sensor::new("left_in_ear", "Left earbud in ear", SensorKind::State, left as u8 as f64).icon("mdi:earbuds"));
            out.push(Sensor::new("right_in_ear", "Right earbud in ear", SensorKind::State, right as u8 as f64).icon("mdi:earbuds"));
        }
        if let Some(open) = self.get(fblock::STATUS, proto::status::CASE_LID).and_then(proto::status::lid_open) {
            out.push(Sensor::new("case_open", "Case open", SensorKind::State, open as u8 as f64).icon("mdi:package-variant"));
        }
        if let Some(c) = self.get(fblock::STATUS, proto::status::CHARGER).and_then(proto::status::charging) {
            out.push(Sensor::new("charging", "Charging", SensorKind::Charging, c as u8 as f64));
        }
        if let Some(p) = self.get(fblock::AUDIO_MANAGEMENT, proto::audio::STATUS).and_then(proto::audio::playing) {
            out.push(Sensor::new("playing", "Playing", SensorKind::State, p as u8 as f64).icon("mdi:play"));
        }
        out
    }

    fn set_choice(&mut self, id: &str, value: &ControlValue) -> Result<Option<Packet>> {
        use proto::settings as s;
        let choice = |v: &ControlValue| match v {
            ControlValue::Choice(c) => Ok(c.clone()),
            _ => Err(Error::InvalidValue(id.to_string())),
        };
        let number = |v: &ControlValue| match v {
            ControlValue::Number(n) => Ok(*n),
            _ => Err(Error::InvalidValue(id.to_string())),
        };
        let toggle = |v: &ControlValue| match v {
            ControlValue::Bool(b) => Ok(*b),
            _ => Err(Error::InvalidValue(id.to_string())),
        };
        Ok(Some(match id {
            "noise_cancelling" => {
                let steps = self.get(fblock::SETTINGS, s::CNC).and_then(s::cnc).map_or(11, |c| c.steps);
                let level = s::cnc_display(number(value)?.round().clamp(0.0, (steps - 1) as f64) as u8, steps);
                Packet::set_get(fblock::SETTINGS, s::CNC, &s::set_cnc(level, true))
            }
            _ if id.starts_with("eq_") => {
                let band: u8 = id[3..].parse().map_err(|_| Error::UnknownControl(id.into()))?;
                Packet::set_get(fblock::SETTINGS, s::RANGE_CONTROL, &s::set_band(band, number(value)?.round() as i8))
            }
            "shortcut" => {
                let b = self.get(fblock::SETTINGS, s::BUTTONS).and_then(s::buttons).ok_or(Error::NotSupported(""))?;
                let want = choice(value)?;
                let mode = s::BUTTON_MODES.iter().position(|m| *m == want).ok_or_else(|| Error::InvalidValue(id.into()))?;
                Packet::set_get(fblock::SETTINGS, s::BUTTONS, &s::set_button(b.id, b.event, mode as u8))
            }
            "voice_prompts" => {
                let v = self.get(fblock::SETTINGS, s::VOICE_PROMPTS).and_then(s::voice_prompts).ok_or(Error::NotSupported(""))?;
                Packet::set_get(fblock::SETTINGS, s::VOICE_PROMPTS, &s::set_voice_prompts(toggle(value)?, v.language))
            }
            "voice_prompt_language" => {
                let v = self.get(fblock::SETTINGS, s::VOICE_PROMPTS).and_then(s::voice_prompts).ok_or(Error::NotSupported(""))?;
                let want = choice(value)?;
                let lang = s::LANGUAGES.iter().position(|l| *l == want).ok_or_else(|| Error::InvalidValue(id.into()))?;
                Packet::set_get(fblock::SETTINGS, s::VOICE_PROMPTS, &s::set_voice_prompts(v.enabled, lang as u8))
            }
            "auto_off" => {
                let want = choice(value)?;
                let m = s::STANDBY_CHOICES.iter().copied().chain(self.get(fblock::SETTINGS, s::STANDBY_TIMER).and_then(s::standby_timer)).find(|&m| minutes_label(m) == want);
                let m = m.ok_or_else(|| Error::InvalidValue(id.into()))?;
                Packet::set_get(fblock::SETTINGS, s::STANDBY_TIMER, &s::set_standby_timer(m))
            }
            "multipoint" => Packet::set_get(fblock::SETTINGS, s::MULTIPOINT, &[toggle(value)? as u8]),
            "anr" => {
                let want = choice(value)?;
                let mode = s::ANR_MODES.iter().position(|m| *m == want).ok_or_else(|| Error::InvalidValue(id.into()))?;
                Packet::set_get(fblock::SETTINGS, s::ANR, &[mode as u8])
            }
            "on_head" | "on_head_auto_play" | "on_head_auto_answer" | "on_head_auto_aware" => {
                let mut h = self.get(fblock::SETTINGS, s::ON_HEAD).and_then(s::on_head).ok_or(Error::NotSupported(""))?;
                let on = toggle(value)?;
                match s::ON_HEAD_FEATURES.iter().position(|(f, _)| *f == id) {
                    Some(i) => h.features[i].1 = on,
                    None => h.enabled = on,
                }
                Packet::set_get(fblock::SETTINGS, s::ON_HEAD, &s::set_on_head(&h))
            }
            "teams_button" => {
                let want = choice(value)?;
                let mode = s::TEAMS_MODES.iter().position(|m| *m == want).ok_or_else(|| Error::InvalidValue(id.into()))?;
                Packet::set_get(fblock::SETTINGS, s::TEAMS_BUTTON, &[mode as u8])
            }
            "volume_control" => {
                let want = choice(value)?;
                let mode = s::VOLUME_CONTROL_MODES.iter().position(|m| *m == want).ok_or_else(|| Error::InvalidValue(id.into()))?;
                Packet::set_get(fblock::SETTINGS, s::VOLUME_CONTROL, &[mode as u8])
            }
            _ if s::TOGGLES.iter().any(|t| t.1 == id) => {
                let func = s::TOGGLES.iter().find(|t| t.1 == id).map(|t| t.0).unwrap_or_default();
                Packet::set_get(fblock::SETTINGS, func, &[toggle(value)? as u8])
            }
            "self_voice" => {
                let want = choice(value)?;
                let mode = s::SIDETONE_MODES.iter().position(|m| *m == want).ok_or_else(|| Error::InvalidValue(id.into()))?;
                Packet::set_get(fblock::SETTINGS, s::SIDETONE, &s::set_sidetone(mode as u8))
            }
            "spatial_audio" => {
                let want = choice(value)?;
                let mode = proto::audio::SPATIAL_MODES.iter().position(|m| *m == want).ok_or_else(|| Error::InvalidValue(id.into()))?;
                Packet::set_get(fblock::AUDIO_MANAGEMENT, proto::audio::SPATIAL, &[mode as u8])
            }
            "voice_assistant" => {
                let want = choice(value)?;
                let v = proto::vpa::ASSISTANTS.iter().find(|(_, n)| *n == want).ok_or_else(|| Error::InvalidValue(id.into()))?.0;
                Packet::set_get(16, proto::vpa::ASSISTANT, &[v])
            }
            "wake_word" => {
                let (word, _, _) = self.get(16, proto::vpa::WAKE_WORD).and_then(proto::vpa::wake_word).ok_or(Error::NotSupported(""))?;
                Packet::set_get(16, proto::vpa::WAKE_WORD, &proto::vpa::set_wake_word(word, toggle(value)?))
            }
            "pairing_mode" => Packet::new(fblock::DEVICE_MANAGEMENT, proto::devices::PAIRING_MODE, Op::Start, &[toggle(value)? as u8]),
            _ if id.starts_with("paired_") => {
                let i = self
                    .paired
                    .iter()
                    .find(|i| format!("paired_{}", rfcomm::format_address(&i.address).replace(':', "").to_lowercase()) == id)
                    .ok_or_else(|| Error::UnknownControl(id.into()))?;
                self.devices_dirty = true;
                if toggle(value)? {
                    Packet::new(fblock::DEVICE_MANAGEMENT, proto::devices::CONNECT, Op::Start, &proto::devices::connect(&i.address))
                } else {
                    Packet::new(fblock::DEVICE_MANAGEMENT, proto::devices::DISCONNECT, Op::Start, &i.address)
                }
            }
            "volume" => Packet::set_get(fblock::AUDIO_MANAGEMENT, proto::audio::VOLUME, &[number(value)?.round().clamp(0.0, 255.0) as u8]),
            _ => return Ok(None),
        }))
    }
}

/// The names of the modes in `mask` (bit n = value n), always including
/// the current one.
fn choices(names: &[&str], current: u8, mask: u32) -> Vec<String> {
    (0..names.len().min(32) as u8).filter(|&m| m == current || mask & (1 << m) != 0).map(|m| names[m as usize].to_string()).collect()
}

fn minutes_label(m: u16) -> String {
    match m {
        0 => "Never".into(),
        1440 => "24 hours".into(),
        m if m >= 60 && m % 60 == 0 => format!("{} hour{}", m / 60, if m == 60 { "" } else { "s" }),
        m => format!("{m} minutes"),
    }
}

/// Bose products that speak BMAP over SPP (`BoseProductId`), with the name
/// Bose sells them under and whether they are a speaker. Wi-Fi speakers,
/// soundbars and the PA speakers only speak BMAP over BLE and are left out
/// (see docs/reverse-engineering/bose-bmap/products-and-earbuds.md).
const PRODUCTS: &[(u16, &str, bool)] = &[
    (0x400A, "AE2 SoundLink", false),
    (0x400C, "QuietComfort 35", false),
    (0x400D, "SoundLink Color II", true),
    (0x4010, "SoundLink Revolve", true),
    (0x4011, "SoundLink Revolve+", true),
    (0x4012, "SoundSport", false),
    (0x4013, "SoundSport Pulse", false),
    (0x4014, "QuietControl 30", false),
    (0x4015, "Hearphones", false),
    (0x4017, "SoundWear", true),
    (0x4018, "SoundSport Free", false),
    (0x4019, "SoundSport Free", false),
    (0x4020, "QuietComfort 35 II", false),
    (0x4021, "A20 Aviation Headset", false),
    (0x4022, "SoundLink Micro", true),
    (0x4024, "Noise Cancelling Headphones 700", false),
    (0x402B, "Hearphones II", false),
    (0x402C, "Frames", false),
    (0x402D, "Sport Earbuds", false),
    (0x402E, "Sport Earbuds", false),
    (0x402F, "QuietComfort Earbuds", false),
    (0x4030, "QuietComfort Earbuds", false),
    (0x4037, "SoundControl Hearing Aids", false),
    (0x4039, "QuietComfort 45", false),
    (0x403A, "Sport Open Earbuds", false),
    (0x404C, "Frames", false),
    (0x4060, "Frames", false),
    (0x4061, "Frames", false),
    (0x4062, "QuietComfort Ultra Earbuds (2nd Gen)", false),
    (0x4064, "QuietComfort Earbuds II", false),
    (0x4066, "QuietComfort Ultra Headphones", false),
    (0x4068, "Ultra Open Earbuds", false),
    (0x4072, "QuietComfort Ultra Earbuds", false),
    (0x4075, "QuietComfort Headphones", false),
    (0x4082, "QuietComfort Ultra Headphones (2nd Gen)", false),
    (0x4083, "QuietComfort Headphones 2", false),
    (0x4085, "SoundLink Plus", true),
    (0x4089, "Ultra Open Earbuds 2", false),
    (0x4097, "Sport Open Earbuds", false),
    (0x094F, "SoundLink Max", true),
    (0xBC58, "SoundLink Micro (2nd Gen)", true),
    (0xBC61, "SoundLink Flex (2nd Gen)", true),
    (0xBC63, "SoundLink Flex SE", true),
];

/// Bose's name for a BMAP product id.
fn model_name(pid: u16) -> Option<&'static str> {
    PRODUCTS.iter().find(|p| p.0 == pid).map(|p| p.1)
}

fn is_speaker(pid: u16) -> bool {
    PRODUCTS.iter().any(|p| p.0 == pid && p.2)
}

impl Device for Headphones {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        // Take whatever the headphones pushed since the last poll.
        loop {
            match self.pump(Duration::ZERO) {
                Ok(p) if p.is_empty() => break,
                Ok(_) => {}
                Err(e) => return Err(e),
            }
        }
        if self.refreshed.elapsed() >= REFRESH || self.devices_dirty {
            self.read_devices();
        }
        if self.refreshed.elapsed() >= REFRESH {
            self.refreshed = Instant::now();
            for (f, func) in self.supported.clone() {
                let _ = self.request(&Packet::get(f, func));
            }
            if self.modes.present() {
                modes::Modes::refresh(self);
            }
        }
        self.manage_tap();
        if self.failures >= MAX_FAILURES {
            return Err(Error::Offline);
        }
        Ok(Snapshot { sensors: self.sensors(), controls: self.controls() })
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        if let Some(r) = modes::Modes::set(self, id, value) {
            return r;
        }
        match self.set_choice(id, value)? {
            Some(p) => self.request(&p).map(|_| ()),
            None => Err(Error::UnknownControl(id.to_string())),
        }
    }

    /// The headphones keep their settings, the phone app can change them,
    /// and some toggles connect devices: never replay old values.
    fn restore_toggles(&self) -> bool {
        false
    }

    fn input_keys(&self) -> Vec<InputKey> {
        if self.has_tap() {
            vec![InputKey { id: SHORTCUT_KEY.into(), name: "Shortcut button".into(), assignable: true }]
        } else {
            Vec::new()
        }
    }

    fn poll_input(&mut self) -> Vec<InputEvent> {
        self.read_tap();
        std::mem::take(&mut self.input)
    }

    fn poll_interval(&self) -> Option<Duration> {
        Some(Duration::from_secs(1))
    }
}

/// Open every connected Bose product the OS Bluetooth stack knows about.
pub(crate) fn discover(ids: &mut Ids, skip_paths: &[CString], out: &mut Vec<(CString, Box<dyn Device>)>) {
    for c in rfcomm::candidates() {
        let path = CString::new(format!("bluetooth:{}", rfcomm::format_address(&c.address))).expect("no NUL");
        if skip_paths.contains(&path) {
            continue;
        }
        match Headphones::open(c.address, &c.name, String::new()) {
            Ok(mut h) => {
                // Only take an id for headphones that answered.
                h.info.id = ids.next("bose");
                log::info!("Bose {} ({}) connected", h.info.model, rfcomm::format_address(&c.address));
                out.push((path, Box::new(h)));
            }
            Err(e) => log::info!("{} ({}): not reachable over BMAP: {e}", c.name, rfcomm::format_address(&c.address)),
        }
    }
}

mod modes {
    //! AudioModes (fblock 31): the mode list, the current mode, and the
    //! settings of the current mode where the headphones let them change.
    use super::*;
    use proto::modes as m;

    #[derive(Default)]
    pub struct Modes {
        list: Vec<m::Mode>,
        current: Option<u8>,
        /// Noise-cancelling steps (Settings Cnc), for the level control.
        steps: u8,
    }

    impl Modes {
        pub fn read(h: &mut Headphones) -> Modes {
            let mut modes = Modes { steps: 11, ..Modes::default() };
            if let Some(c) = h.get(fblock::SETTINGS, proto::settings::CNC).and_then(proto::settings::cnc) {
                modes.steps = c.steps.max(1);
            }
            let Ok(caps) = h.request(&Packet::get(fblock::AUDIO_MODES, m::CAPABILITIES)) else { return modes };
            let Some((bose, user)) = m::capabilities(&caps) else { return modes };
            for i in 0..bose.saturating_add(user) {
                if let Some(mode) = h.request(&Packet::new(fblock::AUDIO_MODES, m::CONFIG, Op::Get, &[i])).ok().and_then(|p| m::config(&p)) {
                    modes.list.push(mode);
                }
            }
            modes.current = h.request(&Packet::get(fblock::AUDIO_MODES, m::CURRENT)).ok().and_then(|p| p.first().copied());
            modes
        }

        pub fn refresh(h: &mut Headphones) {
            let fresh = Modes::read(h);
            if !fresh.list.is_empty() {
                h.modes = fresh;
            }
        }

        /// Keep up with Status packets (pushed or answering a request).
        pub fn update(&mut self, p: &Packet) {
            match p.function {
                m::CURRENT => self.current = p.payload.first().copied().or(self.current),
                m::CONFIG => {
                    if let Some(mode) = m::config(&p.payload) {
                        match self.list.iter_mut().find(|x| x.index == mode.index) {
                            Some(x) => *x = mode,
                            None => self.list.push(mode),
                        }
                    }
                }
                _ => {}
            }
        }

        pub fn present(&self) -> bool {
            !self.list.is_empty()
        }

        fn current_mode(&self) -> Option<&m::Mode> {
            self.list.iter().find(|x| Some(x.index) == self.current)
        }

        /// Labels of the modes that can be selected; duplicates get the slot number.
        fn labels(&self) -> Vec<(u8, String)> {
            let used: Vec<&m::Mode> = self.list.iter().filter(|x| !x.empty()).collect();
            used.iter()
                .map(|x| {
                    let l = x.label();
                    let dup = used.iter().filter(|y| y.label() == l).count() > 1;
                    (x.index, if dup { format!("{l} ({})", x.index + 1) } else { l })
                })
                .collect()
        }

        pub fn controls(&self) -> Vec<Control> {
            let mut out = Vec::new();
            let labels = self.labels();
            if labels.is_empty() {
                return out;
            }
            let current = labels.iter().find(|(i, _)| Some(*i) == self.current).map(|(_, l)| l.clone()).unwrap_or_default();
            out.push(Control {
                id: "mode".into(),
                name: "Mode".into(),
                kind: ControlKind::Choice { options: labels.iter().map(|(_, l)| l.clone()).collect() },
                value: ControlValue::Choice(current),
                help: Some("The headphones' noise-cancelling mode, as switched with their button.".into()),
            });
            if let Some(c) = self.current_mode().filter(|c| c.user_configurable) {
                if c.mutable & 1 != 0 {
                    out.push(Control {
                        id: "mode_noise_cancelling".into(),
                        name: "Noise cancelling".into(),
                        kind: ControlKind::Number { min: 0.0, max: (self.steps - 1) as f64, step: 1.0, unit: String::new() },
                        value: ControlValue::Number(proto::settings::cnc_display(c.cnc, self.steps) as f64),
                        help: Some("Noise cancelling of the current mode: 0 lets the most sound in, the highest step blocks the most. Only your own modes can change it.".into()),
                    });
                }
                if let (true, Some(w)) = (c.mutable & 8 != 0, c.wind) {
                    out.push(Control {
                        id: "mode_wind_block".into(),
                        name: "Wind block".into(),
                        kind: ControlKind::Toggle,
                        value: ControlValue::Bool(w),
                        help: Some("Reduce wind noise in the current mode.".into()),
                    });
                }
            }
            out
        }

        pub fn set(h: &mut Headphones, id: &str, v: &ControlValue) -> Option<Result<()>> {
            let r = match (id, v) {
                ("mode", ControlValue::Choice(want)) => {
                    let Some(&(index, _)) = h.modes.labels().iter().find(|(_, l)| l == want) else {
                        return Some(Err(Error::InvalidValue(id.into())));
                    };
                    h.request(&Packet::new(fblock::AUDIO_MODES, m::CURRENT, Op::Start, &m::select(index))).map(|_| h.modes.current = Some(index))
                }
                ("mode_noise_cancelling" | "mode_wind_block", _) => {
                    let Some(mut mode) = h.modes.current_mode().filter(|c| c.user_configurable).cloned() else {
                        return Some(Err(Error::NotSupported(" for this mode")));
                    };
                    match v {
                        ControlValue::Number(n) => {
                            mode.cnc = proto::settings::cnc_display(n.round().clamp(0.0, (h.modes.steps - 1) as f64) as u8, h.modes.steps)
                        }
                        ControlValue::Bool(b) => mode.wind = Some(*b),
                        _ => return Some(Err(Error::InvalidValue(id.into()))),
                    }
                    h.request(&Packet::set_get(fblock::AUDIO_MODES, m::CONFIG, &mode.encode())).map(|_| ())
                }
                _ => return None,
            };
            Some(r)
        }
    }
}
