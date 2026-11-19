//! Speedlink ACCUSOR gaming keyboard (USB `264a:3008`; it calls itself a
//! "Tt eSPORTS CHALLENGER Prime RGB", the OEM board it is built on).
//!
//! The keyboard keeps three profiles (P1..P3). Each has a lighting setting
//! (off, 25/50/100 % or breathing, in red, blue or purple) and a 160-entry
//! key table; M1..M6 and any other key can play macros stored in the
//! keyboard. Nothing but the active profile can be read back, so
//! OpenDeviceControl keeps its own copy of the tables, macros and lighting
//! (in the front-end's state folder) and writes them whole.
//!
//! | Module | Contents |
//! |---|---|
//! | `proto` | commands, key-table entries, macro encoding, events |
//! | `layout` | where each key is and its key-table index |
//!
//! Protocol notes: docs/reverse-engineering/speedlink-accusor.md.

pub mod layout;
pub mod proto;

use crate::devices::{Device, Ids};
use crate::keymap::{FunctionInfo, KeyBinding, KeymapInfo, LayerInfo, Macro};
use crate::model::*;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::time::Duration;

pub const VID: u16 = 0x264a;
pub const PID: u16 = 0x3008;
/// USB interface with the command collection (`0xFF01`).
const COMMAND_INTERFACE: i32 = 2;
/// USB interface whose `0xFF00` collection carries event reports `04 ..`.
const EVENT_INTERFACE: i32 = 1;

/// Pauses the official driver leaves around uploads.
const AFTER_COMMAND: Duration = Duration::from_millis(100);
const BEFORE_TABLE_DATA: Duration = Duration::from_millis(200);
const BETWEEN_TABLE_REPORTS: Duration = Duration::from_millis(30);
const AFTER_MACRO_REPORT: Duration = Duration::from_millis(100);

const CTRL_PROFILE: &str = "profile";
const CTRL_LEVEL: &str = "lighting_level";
const CTRL_COLOUR: &str = "lighting_colour";

/// Windows lists each top-level collection separately (keep the one with
/// `page`); Linux hidraw has one node per interface.
fn collection(d: &hidapi::DeviceInfo, interface: i32, page: u16) -> bool {
    d.vendor_id() == VID && d.product_id() == PID && d.interface_number() == interface && (!cfg!(windows) || d.usage_page() == page)
}

/// What OpenDeviceControl last wrote (the keyboard can't be read back).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Saved {
    /// Key table per profile (1..3 at 0..2), 160 entries each.
    tables: Vec<Vec<[u8; 4]>>,
    /// Macros 0..17 (device slots 6..23).
    macros: Vec<Macro>,
    /// (level, colour) per profile.
    lighting: Vec<(u8, u8)>,
}

impl Default for Saved {
    fn default() -> Self {
        Saved {
            tables: vec![proto::default_table(); proto::PROFILES as usize],
            macros: vec![Macro::default(); proto::MACRO_SLOTS as usize],
            lighting: vec![(3, 0); proto::PROFILES as usize],
        }
    }
}

impl Saved {
    fn path(id: &str) -> Option<std::path::PathBuf> {
        crate::state_dir().map(|d| d.join(format!("speedlink-{id}.json")))
    }

    fn load(id: &str) -> Saved {
        let Some(path) = Saved::path(id) else { return Saved::default() };
        let Ok(text) = std::fs::read_to_string(&path) else { return Saved::default() };
        match serde_json::from_str::<Saved>(&text) {
            Ok(mut s) => {
                s.tables.resize(proto::PROFILES as usize, proto::default_table());
                for t in &mut s.tables {
                    t.resize(proto::TABLE_ENTRIES, [0; 4]);
                }
                s.macros.resize(proto::MACRO_SLOTS as usize, Macro::default());
                s.lighting.resize(proto::PROFILES as usize, (3, 0));
                s
            }
            Err(e) => {
                log::warn!("ignoring unreadable {}: {e}", path.display());
                Saved::default()
            }
        }
    }

    fn store(&self, id: &str) -> Result<()> {
        let Some(path) = Saved::path(id) else { return Ok(()) };
        let err = |e: std::io::Error| Error::Other(format!("couldn't save {}: {e}", path.display()));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| Error::Other(e.to_string()))?;
        std::fs::write(&path, text).map_err(err)
    }
}

pub struct Accusor {
    info: DeviceInfo,
    cmd: hidapi::HidDevice,
    events: Option<hidapi::HidDevice>,
    saved: Saved,
    /// Active profile, 1..3.
    profile: u8,
    /// Key-table index of each keymap position.
    order: Vec<usize>,
    /// "Start application" key held down (released on the next idle report).
    host_down: Option<u8>,
    input: Vec<InputEvent>,
}

impl Accusor {
    fn send(&self, c: [u8; 9]) -> Result<()> {
        self.cmd.send_feature_report(&c)?;
        Ok(())
    }

    /// The active profile (1..3), or `None` if the keyboard doesn't answer
    /// like an ACCUSOR.
    fn query_profile(cmd: &hidapi::HidDevice) -> Option<u8> {
        cmd.send_feature_report(&proto::query_profile()).ok()?;
        std::thread::sleep(Duration::from_millis(20));
        let mut buf = [0u8; 9];
        let n = cmd.get_feature_report(&mut buf).ok()?;
        // Linux and Windows both return the report ID (0) first.
        let r = &buf[..n];
        let at = r.iter().position(|&b| b == 0x81)?;
        r.get(at + 1).copied().filter(|p| (1..=proto::PROFILES).contains(p))
    }

    fn write_reports(&self, data: &[u8], gap: Duration) -> Result<()> {
        for chunk in data.chunks(proto::REPORT) {
            let mut r = [0u8; proto::REPORT + 1];
            r[1..1 + chunk.len()].copy_from_slice(chunk);
            self.cmd.write(&r)?;
            std::thread::sleep(gap);
        }
        Ok(())
    }

    /// Upload profile `p`'s key table. The keyboard drops its macros when a
    /// table arrives (seen on the hardware), so they are sent again after.
    fn upload_table(&self, p: u8) -> Result<()> {
        self.send(proto::hello())?;
        std::thread::sleep(AFTER_COMMAND);
        self.send(proto::key_table(p))?;
        std::thread::sleep(BEFORE_TABLE_DATA);
        self.write_reports(&proto::table_bytes(&self.saved.tables[p as usize - 1]), BETWEEN_TABLE_REPORTS)?;
        std::thread::sleep(AFTER_COMMAND);
        self.upload_macros()?;
        self.send(proto::select_profile(self.profile))
    }

    /// Upload every macro slot that holds something.
    fn upload_macros(&self) -> Result<()> {
        for (n, m) in self.saved.macros.iter().enumerate() {
            if m.steps.is_empty() {
                continue;
            }
            let bytes = proto::macro_bytes(m).map_err(Error::Bus)?;
            self.send(proto::macro_slot(proto::FIRST_MACRO_SLOT + n as u8))?;
            self.write_reports(&bytes, AFTER_MACRO_REPORT)?;
        }
        Ok(())
    }

    fn apply_lighting(&self, p: u8) -> Result<()> {
        let (level, colour) = self.saved.lighting[p as usize - 1];
        self.send(proto::lighting(p, level, colour))
    }

    /// Read the event reports waiting, keeping state in step with what the
    /// Fn and P keys change.
    fn drain_events(&mut self) {
        let Some(ev) = &self.events else { return };
        let mut buf = [0u8; 64];
        let mut got = Vec::new();
        while let Ok(n) = ev.read_timeout(&mut buf, 0) {
            if n == 0 {
                break;
            }
            if let Some(e) = proto::parse_event(&buf[..n]) {
                got.push(e);
            }
        }
        for e in got {
            match e {
                proto::Event::Profile(p) if (1..=proto::PROFILES).contains(&p) => self.profile = p,
                proto::Event::Level(l) if l <= 4 => self.saved.lighting[self.profile as usize - 1].0 = l,
                proto::Event::Colour(c) if c <= 2 => self.saved.lighting[self.profile as usize - 1].1 = c,
                proto::Event::HostKey(n) => {
                    if let Some(prev) = self.host_down.replace(n) {
                        self.input.push(InputEvent::Released(format!("host_key_{prev}")));
                    }
                    self.input.push(InputEvent::Pressed(format!("host_key_{n}")));
                }
                proto::Event::Idle => {
                    if let Some(prev) = self.host_down.take() {
                        self.input.push(InputEvent::Released(format!("host_key_{prev}")));
                    }
                }
                e => log::debug!("{}: event {e:?}", self.info.id),
            }
        }
    }

    fn controls(&self) -> Vec<Control> {
        let (level, colour) = self.saved.lighting[self.profile as usize - 1];
        let choice = |id: &str, name: &str, options: Vec<String>, value: String, help: &str| Control {
            id: id.into(),
            name: name.into(),
            kind: ControlKind::Choice { options },
            value: ControlValue::Choice(value),
            help: Some(help.into()),
        };
        vec![
            choice(
                CTRL_PROFILE,
                "Profile",
                (1..=proto::PROFILES).map(|p| format!("Profile {p}")).collect(),
                format!("Profile {}", self.profile),
                "The keyboard's active profile, also chosen with P1, P2 and P3. Each has its own keys and lighting.",
            ),
            choice(
                CTRL_LEVEL,
                "Backlight",
                proto::LEVELS.iter().map(|s| s.to_string()).collect(),
                proto::LEVELS[level.min(4) as usize].into(),
                "Brightness of the active profile. Fn+Scroll Lock switches the backlight off and on independently of this.",
            ),
            choice(
                CTRL_COLOUR,
                "Backlight colour",
                proto::COLOURS.iter().map(|s| s.to_string()).collect(),
                proto::COLOURS[colour.min(2) as usize].into(),
                "Colour of the active profile.",
            ),
        ]
    }
}

impl Device for Accusor {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        self.drain_events();
        Ok(Snapshot { sensors: Vec::new(), controls: self.controls() })
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        self.drain_events();
        let ControlValue::Choice(v) = value else { return Err(Error::InvalidValue(id.into())) };
        let pick = |list: &[&str]| list.iter().position(|s| *s == v).map(|i| i as u8).ok_or_else(|| Error::InvalidValue(id.into()));
        match id {
            CTRL_PROFILE => {
                let p = v.strip_prefix("Profile ").and_then(|n| n.parse::<u8>().ok()).filter(|p| (1..=proto::PROFILES).contains(p));
                let p = p.ok_or_else(|| Error::InvalidValue(id.into()))?;
                self.send(proto::select_profile(p))?;
                self.profile = p;
            }
            CTRL_LEVEL => {
                self.saved.lighting[self.profile as usize - 1].0 = pick(&proto::LEVELS)?;
                self.apply_lighting(self.profile)?;
                self.saved.store(&self.info.id)?;
            }
            CTRL_COLOUR => {
                self.saved.lighting[self.profile as usize - 1].1 = pick(&proto::COLOURS)?;
                self.apply_lighting(self.profile)?;
                self.saved.store(&self.info.id)?;
            }
            _ => return Err(Error::UnknownControl(id.into())),
        }
        Ok(())
    }

    fn led_layout(&self) -> Vec<LedShape> {
        layout::shapes()
    }

    fn keymap_info(&self) -> Option<KeymapInfo> {
        Some(KeymapInfo {
            layers: (1..=proto::PROFILES)
                .map(|p| LayerInfo {
                    id: p,
                    name: format!("Profile {p}"),
                    help: format!(
                        "What each key does in profile {p} (P{p}). The keyboard can't report its keys, so this shows what \
                         OpenDeviceControl last saved, or the factory keys."
                    ),
                })
                .collect(),
            functions: proto::FUNCTIONS
                .iter()
                .map(|(e, name, short, group)| FunctionInfo {
                    id: u32::from_be_bytes(*e),
                    name: (*name).into(),
                    short: (*short).into(),
                    group: (*group).into(),
                })
                .collect(),
            macro_slots: proto::MACRO_SLOTS,
            macro_max_steps: proto::MACRO_MAX_STEPS,
            host_keys: self.events.is_some(),
            fixed: Vec::new(),
        })
    }

    fn read_keymap(&mut self, layer: u8) -> Result<Vec<KeyBinding>> {
        let t = self.saved.tables.get((layer as usize).wrapping_sub(1)).ok_or(Error::NotSupported(": no such profile"))?;
        Ok(self.order.iter().map(|&i| proto::binding(t[i])).collect())
    }

    fn write_keymap(&mut self, layer: u8, keys: &[KeyBinding]) -> Result<()> {
        if !(1..=proto::PROFILES).contains(&layer) {
            return Err(Error::NotSupported(": no such profile"));
        }
        let mut table = self.saved.tables[layer as usize - 1].clone();
        for (k, b) in keys.iter().enumerate() {
            if let Some(&i) = self.order.get(k) {
                table[i] = proto::entry(b).map_err(|e| Error::Other(format!("{}: {e}", layout::shapes()[k].name)))?;
            }
        }
        self.saved.tables[layer as usize - 1] = table;
        self.upload_table(layer)?;
        self.saved.store(&self.info.id)
    }

    fn read_macros(&mut self) -> Result<Vec<Macro>> {
        Ok(self.saved.macros.clone())
    }

    fn write_macros(&mut self, macros: &[Macro]) -> Result<()> {
        if macros.len() > proto::MACRO_SLOTS as usize {
            return Err(Error::Other(format!("the keyboard holds {} macros", proto::MACRO_SLOTS)));
        }
        for m in macros {
            proto::macro_bytes(m).map_err(Error::Bus)?;
        }
        let mut all = macros.to_vec();
        all.resize(proto::MACRO_SLOTS as usize, Macro::default());
        self.saved.macros = all;
        self.send(proto::hello())?;
        std::thread::sleep(AFTER_COMMAND);
        self.upload_macros()?;
        self.send(proto::select_profile(self.profile))?;
        self.saved.store(&self.info.id)
    }

    fn input_keys(&self) -> Vec<InputKey> {
        (1..=proto::HOST_KEYS)
            .map(|n| InputKey { id: format!("host_key_{n}"), name: format!("OpenDeviceControl key {n}"), assignable: true })
            .collect()
    }

    fn poll_input(&mut self) -> Vec<InputEvent> {
        self.drain_events();
        std::mem::take(&mut self.input)
    }
}

/// Open the ACCUSOR(s) in `all` (the HID device list).
pub(crate) fn discover(
    api: &hidapi::HidApi,
    all: &[&hidapi::DeviceInfo],
    ids: &mut Ids,
    skip_paths: &[CString],
    out: &mut Vec<(CString, Box<dyn Device>)>,
) {
    for d in all.iter().filter(|d| collection(d, COMMAND_INTERFACE, 0xFF01)) {
        if skip_paths.iter().any(|p| p.as_c_str() == d.path()) || out.iter().any(|(p, _)| p.as_c_str() == d.path()) {
            continue;
        }
        let cmd = match api.open_path(d.path()) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("failed to open the Speedlink ACCUSOR command interface: {e}");
                continue;
            }
        };
        // The VID/PID is the OEM's; only take boards that answer like an ACCUSOR.
        let Some(profile) = Accusor::query_profile(&cmd) else {
            log::info!("264a:3008 keyboard did not answer the ACCUSOR profile query; skipped");
            continue;
        };
        // The event collection of the same keyboard (same USB device path
        // apart from the interface).
        let events = all
            .iter()
            .filter(|e| collection(e, EVENT_INTERFACE, 0xFF00) && (!cfg!(windows) || e.usage() == 1))
            .filter(|e| same_device(e, d))
            .find_map(|e| api.open_path(e.path()).ok());
        if events.is_none() {
            log::warn!("Speedlink ACCUSOR: couldn't open its event interface; P-key and Fn changes won't be noticed");
        }
        let id = ids.next("accusor");
        let saved = Saved::load(&id);
        let kb = Accusor {
            info: DeviceInfo {
                id: id.clone(),
                kind: DeviceKind::Keyboard,
                manufacturer: "Speedlink".into(),
                model: "ACCUSOR".into(),
                vendor_string: d.manufacturer_string().map(str::to_string),
                product_string: d.product_string().map(str::to_string),
                usb_vid: VID,
                usb_pid: PID,
                serial: d.serial_number().map(str::to_string).filter(|s| !s.is_empty()),
                firmware: Some(format!("{:x}.{:02x}", d.release_number() >> 8, d.release_number() & 0xff)),
            },
            cmd,
            events,
            saved,
            profile,
            order: layout::table_indices(),
            host_down: None,
            input: Vec::new(),
        };
        // Turn on event reports.
        if let Err(e) = kb.send(proto::hello()) {
            log::warn!("Speedlink ACCUSOR: {e}");
        }
        log::info!("opened Speedlink ACCUSOR ({id}), profile {profile}");
        out.push((d.path().to_owned(), Box::new(kb)));
    }
}

/// Whether two interfaces belong to the same keyboard: their paths match
/// up to the interface part (Linux: the USB port path in the hidraw device
/// path isn't exposed, so with one ACCUSOR any match will do).
fn same_device(a: &hidapi::DeviceInfo, b: &hidapi::DeviceInfo) -> bool {
    match (a.serial_number(), b.serial_number()) {
        (Some(x), Some(y)) if !x.is_empty() => x == y,
        _ => true,
    }
}
