//! Buttons and wheels: REPROG_CONTROLS_V4 (remapping a button to another
//! button's action, or diverting it so its presses come to OpenDeviceControl
//! as HID++ events), mouse gestures and sliding DPI on a button that can
//! report raw movement, and the events of diverted wheels, crowns and
//! G-keys.
//!
//! Diverted controls show up as keys on the Keys tab, where they get
//! actions (hotkeys, text, programs, macros).

use super::features as f;
use super::hidpp::Report;
use super::keys::{control_name, task_name};
use super::link::{at, u16_at, Link};
use super::settings::Dpi;
use crate::model::{Control, ControlKind, ControlValue, InputEvent, InputKey};
use crate::{Error, Result};
use std::collections::VecDeque;

pub const WHEEL_DIVERT: &str = "wheel_divert";
pub const THUMB_DIVERT: &str = "thumb_divert";
pub const CROWN_DIVERT: &str = "crown_divert";
pub const GKEYS_DIVERT: &str = "gkeys_divert";

/// Key flags (getCidInfo bytes 4 and 8).
pub mod flag {
    pub const MSE: u16 = 0x01;
    pub const REPROGRAMMABLE: u16 = 0x10;
    pub const DIVERTABLE: u16 = 0x20;
    pub const PERSISTENTLY_DIVERTABLE: u16 = 0x40;
    pub const VIRTUAL: u16 = 0x80;
    pub const RAW_XY: u16 = 0x100;
    pub const FORCE_RAW_XY: u16 = 0x200;
}

/// Reporting flags (get/setCidReporting): each has a "valid" bit just
/// above it that says the write changes it.
pub mod report {
    pub const DIVERTED: u8 = 0x01;
    pub const PERSISTENTLY_DIVERTED: u8 = 0x04;
    pub const RAW_XY: u8 = 0x10;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub cid: u16,
    pub task: u16,
    pub flags: u16,
    pub pos: u8,
    pub group: u8,
    pub group_mask: u8,
    /// The control whose action it does now (itself unless remapped).
    pub mapped: u16,
    pub reporting: u8,
}

impl Key {
    pub fn parse(info: &[u8]) -> Key {
        let cid = u16_at(info, 0);
        Key {
            cid,
            task: u16_at(info, 2),
            flags: at(info, 4) as u16 | (at(info, 8) as u16) << 8,
            pos: at(info, 5),
            group: at(info, 6),
            group_mask: at(info, 7),
            mapped: cid,
            reporting: 0,
        }
    }

    pub fn divertable(&self) -> bool {
        self.flags & flag::DIVERTABLE != 0 && self.flags & flag::VIRTUAL == 0
    }

    pub fn raw_xy(&self) -> bool {
        self.flags & flag::RAW_XY != 0
    }
}

/// What OpenDeviceControl does with a divertable button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The button does its normal job (or what it is remapped to).
    Normal,
    /// Presses come to OpenDeviceControl as a key.
    Diverted,
    /// Hold and move the mouse: up, down, left, right or a click, each a key.
    Gestures,
    /// Hold and slide sideways to step through the DPI list; a click
    /// switches between the current and the last DPI.
    SlidingDpi,
}

const MODE_NORMAL: &str = "Normal";
const MODE_DIVERTED: &str = "Send to OpenDeviceControl";
const MODE_GESTURES: &str = "Mouse gestures";
const MODE_SLIDING: &str = "Sliding DPI";


/// How far the mouse has to move (in DPI-independent units, about 5 per
/// cm) before a held gesture button counts as a swipe.
const GESTURE_THRESHOLD: f64 = 1.5;

/// A held raw-XY button being tracked.
#[derive(Debug, Clone)]
struct Track {
    cid: u16,
    dx: f64,
    dy: f64,
    /// Sliding DPI: whether it moved (no move: swap to the other DPI).
    moved: bool,
    /// Index into the DPI list while sliding.
    index: Option<usize>,
    /// The first movement report after a press is spurious on some mice.
    first: bool,
}

pub struct Buttons {
    pub keys: Vec<Key>,
    version: u8,
    modes: Vec<(u16, Mode)>,
    pressed: Vec<u16>,
    track: Option<Track>,
    /// Sliding DPI: the DPI to switch back to on a click.
    other_dpi: Option<u32>,
    events: VecDeque<InputEvent>,
}

pub fn key_id(cid: u16) -> String {
    let slug: String = control_name(cid)
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    format!("{slug}_{cid:x}")
}

fn mode_id(cid: u16) -> String {
    format!("button_{cid:x}")
}

/// Left and right button: never offered.
const MAIN_BUTTONS: [u16; 2] = [0x0050, 0x0051];
const ACT_AS: &str = "Act as: ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    Mode(Mode),
    Remap(u16),
}

/// "Mouse Gesture Button" → "Gesture button", "Back Button" → "Back".
fn short_name(cid: u16) -> String {
    let n = control_name(cid);
    let n = n.strip_prefix("Mouse ").unwrap_or(&n).to_string();
    let n = n.strip_suffix(" Button").map(str::to_string).unwrap_or(n);
    let mut c = n.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
        None => n,
    }
}

fn parse_cid(id: &str, prefix: &str) -> Option<u16> {
    u16::from_str_radix(id.strip_prefix(prefix)?, 16).ok()
}

impl Buttons {
    pub fn read(l: &mut Link) -> Result<Option<Buttons>> {
        if !l.has(f::REPROG_CONTROLS_V4) {
            return Ok(None);
        }
        let count = at(&l.call(f::REPROG_CONTROLS_V4, 0, &[])?, 0);
        let mut keys = Vec::new();
        for i in 0..count {
            keys.push(Key::parse(&l.call(f::REPROG_CONTROLS_V4, 1, &[i])?));
        }
        let mut b = Buttons {
            keys,
            version: l.version(f::REPROG_CONTROLS_V4),
            modes: Vec::new(),
            pressed: Vec::new(),
            track: None,
            other_dpi: None,
            events: VecDeque::new(),
        };
        b.refresh(l);
        Ok(Some(b))
    }

    /// Read each key's current mapping and reporting flags.
    pub fn refresh(&mut self, l: &mut Link) {
        for k in &mut self.keys {
            let [hi, lo] = k.cid.to_be_bytes();
            match l.call(f::REPROG_CONTROLS_V4, 2, &[hi, lo]) {
                Ok(r) => {
                    k.reporting = at(&r, 2);
                    let m = u16_at(&r, 3);
                    k.mapped = if m == 0 { k.cid } else { m };
                }
                Err(e) => log::debug!("logitech: reporting of {:#06x}: {e}", k.cid),
            }
        }
        // A button diverted by someone else (or still diverted from an
        // earlier run) shows as diverted.
        for k in &self.keys {
            if k.reporting & report::DIVERTED != 0 && !self.modes.iter().any(|m| m.0 == k.cid) {
                self.modes.push((k.cid, Mode::Diverted));
            }
            if k.reporting & report::DIVERTED == 0 {
                self.modes.retain(|m| m.0 != k.cid || m.1 == Mode::Normal);
            }
        }
    }

    pub fn mode(&self, cid: u16) -> Mode {
        self.modes.iter().find(|m| m.0 == cid).map(|m| m.1).unwrap_or(Mode::Normal)
    }

    /// Targets a key can be remapped to: its own task first, then the
    /// tasks of the controls in the groups its mask names.
    pub fn targets(&self, k: &Key) -> Vec<(u16, String)> {
        if k.group_mask == 0 {
            return Vec::new();
        }
        let mut out = vec![(k.cid, task_name(k.task))];
        for t in &self.keys {
            if t.cid != k.cid && t.group != 0 && k.group_mask & (1 << (t.group - 1)) != 0 {
                out.push((t.cid, task_name(t.task)));
            }
        }
        // Two controls with the same task: tell them apart.
        let names: Vec<String> = out.iter().map(|o| o.1.clone()).collect();
        for (cid, name) in out.iter_mut() {
            if names.iter().filter(|n| *n == name).count() > 1 {
                *name = format!("{name} ({})", control_name(*cid));
            }
        }
        out
    }

    /// The buttons shown: those that can be diverted or remapped, except
    /// the left and right buttons (remapping those can leave you without a
    /// click).
    fn shown(&self) -> impl Iterator<Item = &Key> {
        self.keys.iter().filter(|k| !MAIN_BUTTONS.contains(&k.cid) && (k.divertable() || self.targets(k).len() > 1))
    }

    /// The options of a button's picker: (label, what it does).
    fn options(&self, k: &Key, has_dpi: bool) -> Vec<(String, Choice)> {
        let mut out = vec![(MODE_NORMAL.to_string(), Choice::Mode(Mode::Normal))];
        for (cid, task) in self.targets(k).into_iter().skip(1) {
            out.push((format!("{ACT_AS}{task}"), Choice::Remap(cid)));
        }
        if k.divertable() {
            out.push((MODE_DIVERTED.into(), Choice::Mode(Mode::Diverted)));
            if k.raw_xy() {
                out.push((MODE_GESTURES.into(), Choice::Mode(Mode::Gestures)));
                if has_dpi {
                    out.push((MODE_SLIDING.into(), Choice::Mode(Mode::SlidingDpi)));
                }
            }
        }
        out
    }

    pub fn controls(&self, has_dpi: bool) -> Vec<Control> {
        let mut out = Vec::new();
        for (i, k) in self.shown().enumerate() {
            let options = self.options(k, has_dpi);
            let mode = self.mode(k.cid);
            let current = if mode != Mode::Normal {
                Choice::Mode(mode)
            } else if k.mapped != k.cid {
                Choice::Remap(k.mapped)
            } else {
                Choice::Mode(Mode::Normal)
            };
            let value = options.iter().find(|o| o.1 == current).map(|o| o.0.clone()).unwrap_or_else(|| MODE_NORMAL.into());
            out.push(Control {
                id: mode_id(k.cid),
                name: format!("Buttons: {}", short_name(k.cid)),
                kind: ControlKind::Choice { options: options.into_iter().map(|o| o.0).collect() },
                value: ControlValue::Choice(value),
                // Once for the whole card.
                help: (i == 0).then(|| {
                    "\"Act as\" makes the button do another button's job. \"Send to OpenDeviceControl\" turns it into a key \
                     you give an action on the Keys tab. Mouse gestures: hold it and move up, down, left or right, or just \
                     click; each is a key. Sliding DPI: hold it and slide sideways to change the sensitivity."
                        .into()
                }),
            });
        }
        out
    }

    pub fn owns(&self, id: &str) -> bool {
        parse_cid(id, "button_").is_some_and(|c| self.keys.iter().any(|k| k.cid == c))
    }

    fn set_reporting(&mut self, l: &mut Link, cid: u16, divert: bool, raw_xy: bool, remap: u16) -> Result<()> {
        let k = self.keys.iter().find(|k| k.cid == cid).ok_or(Error::UnknownControl(cid.to_string()))?.clone();
        let mut bits = 0u8;
        if k.divertable() {
            bits |= report::DIVERTED << 1 | if divert { report::DIVERTED } else { 0 };
        }
        if k.raw_xy() {
            bits |= report::RAW_XY << 1 | if raw_xy { report::RAW_XY } else { 0 };
        }
        let [hi, lo] = cid.to_be_bytes();
        let [rhi, rlo] = remap.to_be_bytes();
        l.call(f::REPROG_CONTROLS_V4, 3, &[hi, lo, bits, rhi, rlo])?;
        if let Some(k) = self.keys.iter_mut().find(|k| k.cid == cid) {
            if k.divertable() {
                k.reporting = (k.reporting & !report::DIVERTED) | if divert { report::DIVERTED } else { 0 };
            }
            if k.raw_xy() {
                k.reporting = (k.reporting & !report::RAW_XY) | if raw_xy { report::RAW_XY } else { 0 };
            }
            if remap != 0 {
                k.mapped = remap;
            }
        }
        Ok(())
    }

    pub fn set_mode(&mut self, l: &mut Link, cid: u16, mode: Mode) -> Result<()> {
        let divert = mode != Mode::Normal;
        let raw = matches!(mode, Mode::Gestures | Mode::SlidingDpi);
        self.set_reporting(l, cid, divert, raw, 0)?;
        self.modes.retain(|m| m.0 != cid);
        self.modes.push((cid, mode));
        if self.track.as_ref().is_some_and(|t| t.cid == cid) {
            self.track = None;
        }
        Ok(())
    }

    pub fn write(&mut self, l: &mut Link, id: &str, v: &ControlValue, has_dpi: bool) -> Result<()> {
        let ControlValue::Choice(s) = v else { return Err(Error::InvalidValue(id.into())) };
        let cid = parse_cid(id, "button_").ok_or_else(|| Error::UnknownControl(id.into()))?;
        let k = self.keys.iter().find(|k| k.cid == cid).ok_or_else(|| Error::UnknownControl(id.into()))?.clone();
        let choice = self.options(&k, has_dpi).into_iter().find(|o| &o.0 == s).ok_or_else(|| Error::InvalidValue(id.into()))?.1;
        // Undo any remapping unless one is chosen (diversion overrides it anyway).
        let remap = match choice {
            Choice::Remap(t) => t,
            _ => k.cid,
        };
        if remap != k.mapped && !self.targets(&k).is_empty() {
            let divert = k.reporting & report::DIVERTED != 0;
            let raw = k.reporting & report::RAW_XY != 0;
            self.set_reporting(l, cid, divert, raw, remap)?;
        }
        let mode = match choice {
            Choice::Mode(m) => m,
            Choice::Remap(_) => Mode::Normal,
        };
        if mode != self.mode(cid) || (mode == Mode::Normal && k.reporting & report::DIVERTED != 0) {
            self.set_mode(l, cid, mode)?;
        }
        Ok(())
    }

    /// The keys diverted buttons report: the button itself, and its
    /// swipes in gesture mode.
    pub fn input_keys(&self) -> Vec<InputKey> {
        let mut out = Vec::new();
        for k in self.keys.iter().filter(|k| k.divertable()) {
            let name = control_name(k.cid);
            match self.mode(k.cid) {
                Mode::Diverted => out.push(InputKey { id: key_id(k.cid), name, assignable: true }),
                Mode::Gestures => {
                    out.push(InputKey { id: key_id(k.cid), name: format!("{name} click"), assignable: true });
                    for dir in ["up", "down", "left", "right"] {
                        out.push(InputKey { id: format!("{}_{dir}", key_id(k.cid)), name: format!("{name} swipe {dir}"), assignable: true });
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn tap(&mut self, id: String) {
        self.events.push_back(InputEvent::Pressed(id.clone()));
        self.events.push_back(InputEvent::Released(id));
    }

    /// Handle a REPROG_CONTROLS_V4 notification. `dpi` is the DPI
    /// setting, for sliding DPI.
    pub fn notify(&mut self, l: &mut Link, r: &Report, mut dpi: Option<&mut Dpi>) {
        match r.function() {
            0 => {
                let now: Vec<u16> = (0..4).map(|i| u16_at(&r.data, 2 * i)).filter(|&c| c != 0).collect();
                let down: Vec<u16> = now.iter().copied().filter(|c| !self.pressed.contains(c)).collect();
                let up: Vec<u16> = self.pressed.iter().copied().filter(|c| !now.contains(c)).collect();
                self.pressed = now;
                for cid in down {
                    match self.mode(cid) {
                        Mode::Gestures | Mode::SlidingDpi if self.track.is_none() => {
                            self.track = Some(Track { cid, dx: 0.0, dy: 0.0, moved: false, index: None, first: true });
                        }
                        Mode::Gestures | Mode::SlidingDpi => {}
                        _ => self.events.push_back(InputEvent::Pressed(key_id(cid))),
                    }
                }
                for cid in up {
                    match self.mode(cid) {
                        Mode::Gestures => {
                            if let Some(t) = self.track.take_if(|t| t.cid == cid) {
                                self.tap(gesture_key(cid, t.dx, t.dy));
                            }
                        }
                        Mode::SlidingDpi => {
                            if let Some(t) = self.track.take_if(|t| t.cid == cid) {
                                if let Some(d) = dpi.as_deref_mut() {
                                    self.slide_end(l, d, &t);
                                }
                            }
                        }
                        _ => self.events.push_back(InputEvent::Released(key_id(cid))),
                    }
                }
            }
            1 => {
                let dx = u16_at(&r.data, 0) as i16 as f64;
                let dy = u16_at(&r.data, 2) as i16 as f64;
                let Some(mut t) = self.track.take() else { return };
                // The first report after the press is spurious on newer mice.
                if t.first && self.version >= 5 {
                    t.first = false;
                    self.track = Some(t);
                    return;
                }
                t.first = false;
                let cur = dpi.as_ref().and_then(|d| d.value).unwrap_or(1000).max(100) as f64;
                // About 5 units per cm whatever the DPI.
                t.dx += dx / cur * 15.0;
                t.dy += dy / cur * 15.0;
                if self.mode(t.cid) == Mode::SlidingDpi {
                    if let Some(d) = dpi.as_deref_mut() {
                        self.slide(l, d, &mut t);
                    }
                }
                self.track = Some(t);
            }
            _ => {}
        }
    }

    fn slide(&mut self, l: &mut Link, d: &mut Dpi, t: &mut Track) {
        let Some(cur) = d.value else { return };
        let Some(ci) = d.list.iter().position(|&v| v == cur) else { return };
        if !t.moved && t.dx.abs() >= 1.0 {
            t.moved = true;
            t.index = Some(ci);
            self.other_dpi = Some(cur);
        }
        if t.moved {
            let base = t.index.unwrap_or(ci) as i64;
            let want = (base + t.dx as i64).clamp(0, d.list.len() as i64 - 1) as usize;
            if want != ci {
                if let Err(e) = d.set(l, d.list[want]) {
                    log::warn!("logitech: sliding DPI: {e}");
                }
            }
        }
    }

    fn slide_end(&mut self, l: &mut Link, d: &mut Dpi, t: &Track) {
        if t.moved {
            return;
        }
        // A click: back to the other DPI.
        let Some(cur) = d.value else { return };
        let other = self.other_dpi.replace(cur).unwrap_or(cur);
        if other != cur {
            if let Err(e) = d.set(l, other) {
                log::warn!("logitech: switching DPI: {e}");
            }
        }
    }

    /// Re-apply the modes after the device reconnected (it forgets them).
    pub fn restore(&mut self, l: &mut Link) {
        let modes: Vec<(u16, Mode)> = self.modes.iter().copied().filter(|m| m.1 != Mode::Normal).collect();
        self.pressed.clear();
        self.track = None;
        for (cid, m) in modes {
            if let Err(e) = self.set_mode(l, cid, m) {
                log::warn!("logitech: restoring button {cid:#06x}: {e}");
            }
        }
    }

    pub fn take_events(&mut self) -> Vec<InputEvent> {
        self.events.drain(..).collect()
    }
}

/// The key a finished gesture sends: a click when it barely moved, else
/// the main direction.
fn gesture_key(cid: u16, dx: f64, dy: f64) -> String {
    let id = key_id(cid);
    if dx.abs().max(dy.abs()) < GESTURE_THRESHOLD {
        return id;
    }
    let dir = if dx.abs() > dy.abs() {
        if dx > 0.0 { "right" } else { "left" }
    } else if dy > 0.0 {
        "down"
    } else {
        "up"
    };
    format!("{id}_{dir}")
}

/// Events of wheels, crowns and G-keys while they are diverted.
#[derive(Default)]
pub struct Wheels {
    /// Thumb wheel units per step (diverted resolution / native).
    thumb_step: i32,
    thumb_acc: i32,
    thumb_present: bool,
    /// The thumb wheel reports single taps (capability bit 3).
    thumb_tap: bool,
    pub thumb_diverted: bool,
    wheel_present: bool,
    pub wheel_diverted: bool,
    crown_present: bool,
    pub crown_diverted: bool,
    gkeys: u8,
    mkeys: u8,
    mr: bool,
    pub gkeys_diverted: bool,
    g_down: u32,
    m_down: u8,
    mr_down: bool,
    crown_down: bool,
    events: VecDeque<InputEvent>,
}

impl Wheels {
    pub fn read(l: &mut Link) -> Wheels {
        let mut w = Wheels { thumb_step: 1, ..Default::default() };
        if l.has(f::THUMB_WHEEL) {
            w.thumb_present = true;
            if let Ok(r) = l.call(f::THUMB_WHEEL, 0, &[]) {
                let native = u16_at(&r, 0).max(1) as i32;
                let diverted = u16_at(&r, 2).max(1) as i32;
                w.thumb_step = (diverted / native).max(1);
                w.thumb_tap = at(&r, 4) & 0x08 != 0;
            }
        }
        w.wheel_present = l.has(f::HIRES_WHEEL) || l.has(f::LOWRES_WHEEL);
        w.crown_present = l.has(f::CROWN);
        if l.has(f::GKEY) {
            w.gkeys = l.call(f::GKEY, 0, &[]).map(|r| at(&r, 0)).unwrap_or(0).min(32);
        }
        if l.has(f::MKEYS) {
            w.mkeys = l.call(f::MKEYS, 0, &[]).map(|r| at(&r, 0)).unwrap_or(0).min(8);
        }
        w.mr = l.has(f::MR);
        w
    }

    pub fn input_keys(&self) -> Vec<InputKey> {
        let mut out = Vec::new();
        let mut add = |id: &str, name: &str| out.push(InputKey { id: id.into(), name: name.into(), assignable: true });
        if self.thumb_present && self.thumb_diverted {
            add("thumb_left", "Thumb wheel left");
            add("thumb_right", "Thumb wheel right");
            if self.thumb_tap {
                add("thumb_tap", "Thumb wheel tap");
            }
        }
        if self.wheel_present && self.wheel_diverted {
            add("scroll_up", "Scroll up");
            add("scroll_down", "Scroll down");
        }
        if self.crown_present && self.crown_diverted {
            add("crown_left", "Crown left");
            add("crown_right", "Crown right");
            add("crown_press", "Crown press");
        }
        if self.gkeys_diverted {
            for i in 1..=self.gkeys {
                add(&format!("g{i}"), &format!("G{i}"));
            }
            for i in 1..=self.mkeys {
                add(&format!("m{i}"), &format!("M{i}"));
            }
            if self.mr {
                add("mr", "MR");
            }
        }
        out
    }

    fn tap(&mut self, id: &str) {
        self.events.push_back(InputEvent::Pressed(id.into()));
        self.events.push_back(InputEvent::Released(id.into()));
    }

    fn edge(&mut self, id: String, was: bool, is: bool) {
        if is && !was {
            self.events.push_back(InputEvent::Pressed(id));
        } else if was && !is {
            self.events.push_back(InputEvent::Released(id));
        }
    }

    /// Handle a notification of `feature`.
    pub fn notify(&mut self, feature: u16, r: &Report) {
        if r.function() != 0 {
            return;
        }
        let d = &r.data;
        match feature {
            f::THUMB_WHEEL if self.thumb_diverted => {
                // rotation (i16), time (2), rotation status, flags (touch,
                // proximity 0x02, touch 0x04, single tap 0x08)
                if at(d, 4) <= 1 {
                    self.thumb_acc = 0;
                }
                self.thumb_acc += u16_at(d, 0) as i16 as i32;
                while self.thumb_acc >= self.thumb_step {
                    self.thumb_acc -= self.thumb_step;
                    self.tap("thumb_right");
                }
                while self.thumb_acc <= -self.thumb_step {
                    self.thumb_acc += self.thumb_step;
                    self.tap("thumb_left");
                }
                if self.thumb_tap && at(d, 5) & 0x08 != 0 {
                    self.tap("thumb_tap");
                }
            }
            f::HIRES_WHEEL if self.wheel_diverted => {
                let delta = u16_at(d, 1) as i16;
                if delta > 0 {
                    self.tap("scroll_up");
                } else if delta < 0 {
                    self.tap("scroll_down");
                }
            }
            f::LOWRES_WHEEL if self.wheel_diverted => {
                let delta = at(d, 0) as i8;
                if delta > 0 {
                    self.tap("scroll_up");
                } else if delta < 0 {
                    self.tap("scroll_down");
                }
            }
            f::CROWN if self.crown_diverted => {
                let turn = at(d, 1) as i8;
                if turn > 0 {
                    self.tap("crown_right");
                } else if turn < 0 {
                    self.tap("crown_left");
                }
                let press = at(d, 6);
                let down = (1..=4).contains(&press);
                let was = self.crown_down;
                self.crown_down = down;
                self.edge("crown_press".into(), was, down);
            }
            f::GKEY => {
                let now = u32::from_le_bytes([at(d, 0), at(d, 1), at(d, 2), at(d, 3)]);
                for i in 0..32 {
                    let (was, is) = (self.g_down >> i & 1 != 0, now >> i & 1 != 0);
                    self.edge(format!("g{}", i + 1), was, is);
                }
                self.g_down = now;
            }
            f::MKEYS => {
                let now = at(d, 0);
                for i in 0..8 {
                    let (was, is) = (self.m_down >> i & 1 != 0, now >> i & 1 != 0);
                    self.edge(format!("m{}", i + 1), was, is);
                }
                self.m_down = now;
            }
            f::MR => {
                let now = at(d, 0) != 0;
                let was = self.mr_down;
                self.mr_down = now;
                self.edge("mr".into(), was, now);
            }
            _ => {}
        }
    }

    pub fn take_events(&mut self) -> Vec<InputEvent> {
        self.events.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mx_master_3s_keys() {
        // getCidInfo replies from an MX Master 3S.
        let gesture = Key::parse(&[0x00, 0xc3, 0x00, 0xa9, 0x31, 0x00, 0x02, 0x03, 0x05]);
        assert_eq!(gesture.cid, 0xc3);
        assert!(gesture.divertable() && gesture.raw_xy());
        let left = Key::parse(&[0x00, 0x50, 0x00, 0x38, 0x01, 0x00, 0x01, 0x01, 0x04]);
        assert!(!left.divertable());
        let virt = Key::parse(&[0x00, 0xd7, 0x00, 0xb4, 0xa0, 0x00, 0x03, 0x00, 0x03]);
        assert!(!virt.divertable(), "virtual controls are not offered");
    }

    #[test]
    fn gestures_pick_the_main_direction() {
        assert_eq!(gesture_key(0xc3, 0.2, -0.3), key_id(0xc3));
        assert_eq!(gesture_key(0xc3, 4.0, 1.0), format!("{}_right", key_id(0xc3)));
        assert_eq!(gesture_key(0xc3, 0.5, -3.0), format!("{}_up", key_id(0xc3)));
        assert_eq!(key_id(0xc3), "mouse_gesture_button_c3");
    }
}
