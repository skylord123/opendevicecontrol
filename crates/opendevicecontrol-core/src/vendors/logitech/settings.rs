//! Device settings: each reads its value(s) over HID++ and turns them into
//! controls, and writes a changed control back.
//!
//! The set follows Solaar's (`settings_templates.py`): which feature or
//! register holds the value, which functions read and write it, and how
//! the bytes encode it. Control names are "Part: setting" so the GUI shows
//! a card per part (pointer, scroll wheel, keyboard…).

use super::features as f;
use super::link::{at, be, to_be, u16_at, Link};
use crate::model::{Control, ControlKind, ControlValue};
use crate::{Error, Result};

pub trait Setting: Send {
    /// Whether control `id` belongs to this setting.
    fn owns(&self, id: &str) -> bool;
    fn controls(&self) -> Vec<Control>;
    /// Read the current value(s) from the device.
    fn read(&mut self, l: &mut Link) -> Result<()>;
    fn write(&mut self, l: &mut Link, id: &str, v: &ControlValue) -> Result<()>;
    /// Re-applied when the device reconnects (false for actions such as
    /// switching hosts).
    fn persist(&self) -> bool {
        true
    }
}

pub const POINTER: &str = "Pointer";
pub const WHEEL: &str = "Scroll wheel";
pub const THUMB: &str = "Thumb wheel";
pub const KEYBOARD: &str = "Keyboard";
pub const LIGHTING: &str = "Lighting";
pub const DEVICE: &str = "Device";
pub const AUDIO: &str = "Audio";

/// Where a value lives.
#[derive(Debug, Clone)]
pub enum Io {
    /// A HID++ 2.0 feature: read with `read` (none: write-only, assumed
    /// 0), write with `write`. `prefix` goes before the value in both,
    /// `suffix` after it when writing.
    Feature { feature: u16, read: Option<u8>, write: u8, prefix: Vec<u8>, suffix: Vec<u8>, no_reply: bool },
    /// A HID++ 1.0 register.
    Register(u16),
}

impl Io {
    pub fn feature(feature: u16, read: u8, write: u8) -> Io {
        Io::Feature { feature, read: Some(read), write, prefix: Vec::new(), suffix: Vec::new(), no_reply: false }
    }

    fn read(&self, l: &mut Link) -> Result<Vec<u8>> {
        match self {
            Io::Feature { read: None, .. } => Ok(vec![0; 16]),
            Io::Feature { feature, read: Some(func), prefix, .. } => l.call(*feature, *func, prefix),
            Io::Register(r) => l.read_register(*r, &[]),
        }
    }

    fn write(&self, l: &mut Link, data: &[u8]) -> Result<()> {
        match self {
            Io::Feature { feature, write, prefix, suffix, no_reply, .. } => {
                let mut p = prefix.clone();
                p.extend_from_slice(data);
                p.extend_from_slice(suffix);
                if *no_reply {
                    l.call_no_reply(*feature, *write, &p)
                } else {
                    l.call(*feature, *write, &p).map(|_| ())
                }
            }
            Io::Register(r) => l.write_register(*r, data).map(|_| ()),
        }
    }
}

/// How the bytes encode the value.
#[derive(Debug, Clone)]
pub enum Codec {
    /// On/off: the `bytes` bytes after `skip` (masked) equal `on` or `off`.
    /// A mask that doesn't cover every bit keeps the other bits as read.
    Toggle { on: u32, off: u32, mask: u32, bytes: usize, skip: usize, prefix: Vec<u8> },
    Range { min: i64, max: i64, bytes: usize, skip: usize, prefix: Vec<u8>, unit: &'static str },
    Choice { options: Vec<(u32, String)>, bytes: usize, skip: usize, prefix: Vec<u8> },
}

impl Codec {
    pub fn toggle(on: u32, mask: u32) -> Codec {
        Codec::Toggle { on, off: 0, mask, bytes: 1, skip: 0, prefix: Vec::new() }
    }

    pub fn flag() -> Codec {
        Codec::toggle(1, 0xFF)
    }

    pub fn range(min: i64, max: i64, bytes: usize) -> Codec {
        Codec::Range { min, max, bytes, skip: 0, prefix: Vec::new(), unit: "" }
    }

    pub fn choice(options: Vec<(u32, String)>, bytes: usize) -> Codec {
        Codec::Choice { options, bytes, skip: 0, prefix: Vec::new() }
    }

    fn kind(&self) -> ControlKind {
        match self {
            Codec::Toggle { .. } => ControlKind::Toggle,
            Codec::Range { min, max, unit, .. } => {
                ControlKind::Number { min: *min as f64, max: *max as f64, step: 1.0, unit: unit.to_string() }
            }
            Codec::Choice { options, .. } => ControlKind::Choice { options: options.iter().map(|o| o.1.clone()).collect() },
        }
    }

    /// The control value from a reply.
    fn decode(&self, reply: &[u8]) -> Option<ControlValue> {
        match self {
            Codec::Toggle { on, mask, bytes, skip, .. } => {
                let v = be(reply.get(*skip..*skip + *bytes)?) & mask;
                Some(ControlValue::Bool(v == *on))
            }
            Codec::Range { min, max, bytes, skip, .. } => {
                let v = be(reply.get(*skip..*skip + *bytes)?) as i64;
                Some(ControlValue::Number(v.clamp(*min, *max) as f64))
            }
            Codec::Choice { options, bytes, skip, .. } => {
                let v = be(reply.get(*skip..*skip + *bytes)?);
                let o = options.iter().find(|o| o.0 == v)?;
                Some(ControlValue::Choice(o.1.clone()))
            }
        }
    }

    /// Bytes to write for `v`; `current` is the last reply (for toggles
    /// that only own some bits).
    fn encode(&self, v: &ControlValue, current: Option<&[u8]>) -> Option<Vec<u8>> {
        match (self, v) {
            (Codec::Toggle { on, off, mask, bytes, skip, prefix }, ControlValue::Bool(b)) => {
                let full = if *bytes >= 4 { u32::MAX } else { (1u32 << (8 * bytes)) - 1 };
                let mut w = if *b { *on } else { *off };
                if *mask != full {
                    let cur = current.and_then(|c| c.get(*skip..*skip + *bytes)).map(be).unwrap_or(0);
                    w |= cur & !mask & full;
                }
                let mut out = prefix.clone();
                out.extend(to_be(w, *bytes));
                Some(out)
            }
            (Codec::Range { min, max, bytes, prefix, .. }, ControlValue::Number(n)) => {
                let n = (n.round() as i64).clamp(*min, *max);
                let mut out = prefix.clone();
                out.extend(to_be(n as u32, *bytes));
                Some(out)
            }
            (Codec::Choice { options, bytes, prefix, .. }, ControlValue::Choice(_) | ControlValue::Number(_)) => {
                // A number picks the option it names ("1000" DPI).
                let label = match v {
                    ControlValue::Number(n) => format!("{n}"),
                    ControlValue::Choice(c) => c.clone(),
                    _ => return None,
                };
                let o = options.iter().find(|o| o.1 == label)?;
                let mut out = prefix.clone();
                out.extend(to_be(o.0, *bytes));
                Some(out)
            }
            _ => None,
        }
    }
}

/// A setting with one control, one place and one encoding.
pub struct Simple {
    pub id: String,
    pub name: String,
    pub help: Option<String>,
    pub io: Io,
    pub codec: Codec,
    pub persist: bool,
    reply: Option<Vec<u8>>,
    value: Option<ControlValue>,
}

impl Simple {
    pub fn new(id: &str, part: &str, label: &str, help: &str, io: Io, codec: Codec) -> Simple {
        Simple {
            id: id.into(),
            name: format!("{part}: {label}"),
            help: (!help.is_empty()).then(|| help.to_string()),
            io,
            codec,
            persist: true,
            reply: None,
            value: None,
        }
    }

    pub fn boxed(self) -> Box<dyn Setting> {
        Box::new(self)
    }

    fn write_only(&self) -> bool {
        matches!(self.io, Io::Feature { read: None, .. })
    }
}

impl Setting for Simple {
    fn owns(&self, id: &str) -> bool {
        self.id == id
    }

    fn controls(&self) -> Vec<Control> {
        let Some(value) = self.value.clone() else { return Vec::new() };
        vec![Control { id: self.id.clone(), name: self.name.clone(), kind: self.codec.kind(), value, help: self.help.clone() }]
    }

    fn read(&mut self, l: &mut Link) -> Result<()> {
        if self.write_only() {
            // Nothing to read back: show what was last written (or off).
            if self.value.is_none() {
                self.value = self.codec.decode(&[0; 16]).or_else(|| match &self.codec {
                    Codec::Choice { options, .. } => options.first().map(|o| ControlValue::Choice(o.1.clone())),
                    _ => None,
                });
            }
            return Ok(());
        }
        let r = self.io.read(l)?;
        self.value = self.codec.decode(&r);
        if self.value.is_none() {
            log::debug!("{}: unexpected value {:02x?}", self.id, r);
        }
        self.reply = Some(r);
        Ok(())
    }

    fn write(&mut self, l: &mut Link, _id: &str, v: &ControlValue) -> Result<()> {
        if let Codec::Toggle { mask, bytes, .. } = &self.codec {
            let full = if *bytes >= 4 { u32::MAX } else { (1u32 << (8 * bytes)) - 1 };
            if *mask != full && !self.write_only() {
                self.reply = Some(self.io.read(l)?);
            }
        }
        let data = self.codec.encode(v, self.reply.as_deref()).ok_or_else(|| Error::InvalidValue(self.id.clone()))?;
        self.io.write(l, &data)?;
        self.value = Some(match (&self.codec, v) {
            (Codec::Choice { .. }, ControlValue::Number(n)) => ControlValue::Choice(format!("{n}")),
            _ => v.clone(),
        });
        Ok(())
    }

    fn persist(&self) -> bool {
        self.persist
    }
}

fn opt(v: u32, s: &str) -> (u32, String) {
    (v, s.to_string())
}

/// Every setting the device's features (or, for HID++ 1.0 devices, its
/// known registers) offer. Each builder asks the device what it supports,
/// so the device must be online.
pub fn build(l: &mut Link, registers: u32) -> Vec<Box<dyn Setting>> {
    let mut out: Vec<Box<dyn Setting>> = Vec::new();
    if l.hidpp10() {
        registers_settings(registers, &mut out);
        return out;
    }
    type Builder = fn(&mut Link, &mut Vec<Box<dyn Setting>>) -> Result<()>;
    let builders: &[(&str, Builder)] = &[
        ("extended dpi", extended_dpi),
        ("pointer speed", pointer_speed),
        ("report rate", report_rate),
        ("extended report rate", extended_report_rate),
        ("onboard profiles", onboard_profiles),
        ("smart shift", smart_shift),
        ("hi-res scrolling", hi_res_scrolling),
        ("lowres wheel", lowres_wheel),
        ("hires wheel", hires_wheel),
        ("thumb wheel", thumb_wheel),
        ("fn inversion", fn_inversion),
        ("backlight", backlight2),
        ("backlight3", backlight3),
        ("disable keys", disable_keys),
        ("multiplatform", multiplatform),
        ("dualplatform", dualplatform),
        ("crown", crown),
        ("gkeys", gkeys),
        ("mkeys", mkeys),
        ("brightness", brightness),
        ("haptic", haptic),
        ("force sensing", force_sensing),
        ("sidetone", sidetone),
        ("equalizer", equalizer),
        ("adc power", adc_power),
        ("change host", change_host),
    ];
    for (name, b) in builders {
        if let Err(e) = b(l, &mut out) {
            log::debug!("logitech: {name} setting unavailable: {e}");
        }
    }
    out
}

/// HID++ 1.0 register flags known per model (see `devices.rs`).
pub mod reg {
    pub const DPI_PMX: u32 = 1;
    pub const SMOOTH_SCROLL: u32 = 2;
    pub const SIDE_SCROLL: u32 = 4;
    pub const FN_SWAP: u32 = 8;
    pub const HAND_DETECTION: u32 = 16;
    pub const BATTERY_STATUS: u32 = 32;
    pub const BATTERY_CHARGE: u32 = 64;
}

fn registers_settings(registers: u32, out: &mut Vec<Box<dyn Setting>>) {
    if registers & reg::DPI_PMX != 0 {
        let options = (0x81..=0x8F).map(|v| (v, format!("{}", (v - 0x80) * 100))).collect();
        out.push(Simple::new("dpi", POINTER, "Sensitivity (DPI)", "How far the pointer moves per inch of mouse travel.", Io::Register(0x63), Codec::choice(options, 1)).boxed());
    }
    if registers & reg::SMOOTH_SCROLL != 0 {
        out.push(Simple::new("smooth_scroll", WHEEL, "Smooth scrolling", "High-sensitivity mode for vertical scroll with the wheel.", Io::Register(0x01), Codec::toggle(0x40, 0x40)).boxed());
    }
    if registers & reg::SIDE_SCROLL != 0 {
        out.push(
            Simple::new(
                "side_scroll",
                WHEEL,
                "Side scrolling",
                "Off: tilting the wheel sideways sends button presses instead of horizontal scrolling.",
                Io::Register(0x01),
                Codec::toggle(0x02, 0x02),
            )
            .boxed(),
        );
    }
    if registers & reg::FN_SWAP != 0 {
        out.push(
            Simple::new("fn_swap", KEYBOARD, "Swap Fx function", FN_HELP, Io::Register(0x09), Codec::Toggle { on: 0x0001, off: 0, mask: 0x0001, bytes: 2, skip: 0, prefix: vec![] })
                .boxed(),
        );
    }
    if registers & reg::HAND_DETECTION != 0 {
        out.push(
            Simple::new(
                "hand_detection",
                KEYBOARD,
                "Hand detection",
                "Turn on the illumination when hands hover over the keyboard.",
                Io::Register(0x01),
                Codec::Toggle { on: 0x000000, off: 0x000030, mask: 0x0000FF, bytes: 3, skip: 0, prefix: vec![] },
            )
            .boxed(),
        );
    }
}

const FN_HELP: &str = "On: F1..F12 do their special function and Fn+F1..F12 the standard one. Off: the other way round.";

/// The DPI list of ADJUSTABLE_DPI (function `function`, data after
/// `skip` bytes) or EXTENDED_ADJUSTABLE_DPI: 16-bit values, a value with
/// the top bits `111` is a step to the next value, ended by 0.
pub fn dpi_list(l: &mut Link, feature: u16, function: u8, skip: usize, direction: u8) -> Result<Vec<u32>> {
    let mut bytes = Vec::new();
    for i in 0..16u8 {
        let r = l.call(feature, function, &[0, direction, i])?;
        bytes.extend_from_slice(r.get(skip..).unwrap_or(&[]));
        if bytes.len() >= 2 && bytes[bytes.len() - 2..] == [0, 0] {
            break;
        }
    }
    Ok(parse_dpi_list(&bytes))
}

pub fn parse_dpi_list(bytes: &[u8]) -> Vec<u32> {
    let mut list: Vec<u32> = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let v = u16_at(bytes, i) as u32;
        if v == 0 {
            break;
        }
        if v >> 13 == 0b111 {
            let step = v & 0x1FFF;
            let last = u16_at(bytes, i + 2) as u32;
            if let Some(&prev) = list.last() {
                if step > 0 && last > prev {
                    let mut x = prev + step;
                    while x <= last {
                        list.push(x);
                        x += step;
                    }
                }
            }
            i += 4;
        } else {
            list.push(v);
            i += 2;
        }
    }
    list
}

/// ADJUSTABLE_DPI: one sensor, a list of resolutions.
pub struct Dpi {
    pub list: Vec<u32>,
    pub value: Option<u32>,
}

pub const DPI_ID: &str = "dpi";

impl Setting for Dpi {
    fn owns(&self, id: &str) -> bool {
        id == DPI_ID
    }

    fn controls(&self) -> Vec<Control> {
        let Some(v) = self.value else { return Vec::new() };
        vec![Control {
            id: DPI_ID.into(),
            name: format!("{POINTER}: Sensitivity (DPI)"),
            kind: ControlKind::Choice { options: self.list.iter().map(|d| d.to_string()).collect() },
            value: ControlValue::Choice(v.to_string()),
            help: None,
        }]
    }

    fn read(&mut self, l: &mut Link) -> Result<()> {
        let r = l.call(f::ADJUSTABLE_DPI, 2, &[0])?;
        let mut v = u16_at(&r, 1) as u32;
        if v == 0 {
            v = u16_at(&r, 3) as u32; // the default
        }
        self.value = Some(v);
        Ok(())
    }

    fn write(&mut self, l: &mut Link, _id: &str, v: &ControlValue) -> Result<()> {
        let dpi: u32 = match v {
            ControlValue::Choice(s) => s.parse().map_err(|_| Error::InvalidValue(DPI_ID.into()))?,
            ControlValue::Number(n) => n.round() as u32,
            _ => return Err(Error::InvalidValue(DPI_ID.into())),
        };
        if !self.list.contains(&dpi) {
            return Err(Error::InvalidValue(DPI_ID.into()));
        }
        self.set(l, dpi)
    }
}

impl Dpi {
    pub fn set(&mut self, l: &mut Link, dpi: u32) -> Result<()> {
        let [hi, lo] = (dpi as u16).to_be_bytes();
        l.call(f::ADJUSTABLE_DPI, 3, &[0, hi, lo])?;
        self.value = Some(dpi);
        Ok(())
    }
}

/// EXTENDED_ADJUSTABLE_DPI: X (and maybe separate Y) resolution plus lift-off
/// distance.
struct ExtendedDpi {
    x: Vec<u32>,
    y: Option<Vec<u32>>,
    lod: bool,
    value: Option<(u32, u32, u8)>,
}

impl Setting for ExtendedDpi {
    fn owns(&self, id: &str) -> bool {
        matches!(id, "dpi" | "dpi_y" | "lift_off")
    }

    fn controls(&self) -> Vec<Control> {
        let Some((x, y, lod)) = self.value else { return Vec::new() };
        let mut out = vec![Control {
            id: "dpi".into(),
            name: format!("{POINTER}: Sensitivity (DPI){}", if self.y.is_some() { " X" } else { "" }),
            kind: ControlKind::Choice { options: self.x.iter().map(|d| d.to_string()).collect() },
            value: ControlValue::Choice(x.to_string()),
            help: Some("How far the pointer moves per inch of mouse travel.".into()),
        }];
        if let Some(ys) = &self.y {
            out.push(Control {
                id: "dpi_y".into(),
                name: format!("{POINTER}: Sensitivity (DPI) Y"),
                kind: ControlKind::Choice { options: ys.iter().map(|d| d.to_string()).collect() },
                value: ControlValue::Choice(y.to_string()),
                help: None,
            });
        }
        if self.lod {
            out.push(Control {
                id: "lift_off".into(),
                name: format!("{POINTER}: Lift-off distance"),
                kind: ControlKind::Choice { options: LOD.iter().map(|s| s.to_string()).collect() },
                value: ControlValue::Choice(LOD.get(lod as usize).unwrap_or(&"Low").to_string()),
                help: Some("How high the mouse can be lifted before it stops tracking.".into()),
            });
        }
        out
    }

    fn read(&mut self, l: &mut Link) -> Result<()> {
        let r = l.call(f::EXTENDED_ADJUSTABLE_DPI, 5, &[0])?;
        let pick = |a: u16, b: u16| if a == 0 { b } else { a } as u32;
        let x = pick(u16_at(&r, 1), u16_at(&r, 3));
        let y = pick(u16_at(&r, 5), u16_at(&r, 7));
        self.value = Some((x, y, at(&r, 9)));
        Ok(())
    }

    fn write(&mut self, l: &mut Link, id: &str, v: &ControlValue) -> Result<()> {
        let (mut x, mut y, mut lod) = self.value.unwrap_or((800, 800, 0));
        match (id, v) {
            ("dpi", ControlValue::Choice(s)) => x = s.parse().map_err(|_| Error::InvalidValue(id.into()))?,
            ("dpi_y", ControlValue::Choice(s)) => y = s.parse().map_err(|_| Error::InvalidValue(id.into()))?,
            ("lift_off", ControlValue::Choice(s)) => lod = LOD.iter().position(|o| o == s).ok_or_else(|| Error::InvalidValue(id.into()))? as u8,
            _ => return Err(Error::InvalidValue(id.into())),
        }
        let mut p = vec![0];
        p.extend((x as u16).to_be_bytes());
        p.extend(if self.y.is_some() { (y as u16).to_be_bytes() } else { [0, 0] });
        p.push(if self.lod { lod } else { 0 });
        l.call(f::EXTENDED_ADJUSTABLE_DPI, 6, &p)?;
        self.value = Some((x, y, lod));
        Ok(())
    }
}

const LOD: [&str; 3] = ["Low", "Medium", "High"];

fn extended_dpi(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if !l.has(f::EXTENDED_ADJUSTABLE_DPI) || l.has(f::ADJUSTABLE_DPI) {
        return Ok(());
    }
    let caps = l.call(f::EXTENDED_ADJUSTABLE_DPI, 1, &[0])?;
    let (has_y, lod) = (at(&caps, 2) & 1 != 0, at(&caps, 2) & 2 != 0);
    let x = dpi_list(l, f::EXTENDED_ADJUSTABLE_DPI, 2, 3, 0)?;
    let y = if has_y { Some(dpi_list(l, f::EXTENDED_ADJUSTABLE_DPI, 2, 3, 1)?) } else { None };
    if !x.is_empty() {
        out.push(Box::new(ExtendedDpi { x, y, lod, value: None }));
    }
    Ok(())
}

fn pointer_speed(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if l.has(f::POINTER_SPEED) {
        out.push(
            Simple::new(
                "pointer_speed",
                POINTER,
                "Pointer speed",
                "Speed multiplier for the pointer; 256 is normal.",
                Io::feature(f::POINTER_SPEED, 0, 1),
                Codec::range(0x2E, 0x1FF, 2),
            )
            .boxed(),
        );
    }
    Ok(())
}

const RATE_HELP: &str = "How often the mouse reports movement. With onboard profiles on, the profile may set it instead.";

fn report_rate(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if !l.has(f::REPORT_RATE) {
        return Ok(());
    }
    let flags = at(&l.call(f::REPORT_RATE, 0, &[])?, 0);
    let options: Vec<(u32, String)> = (0..8).filter(|i| flags >> i & 1 != 0).map(|i| (i + 1, format!("{} ms", i + 1))).collect();
    if !options.is_empty() {
        out.push(Simple::new("report_rate", POINTER, "Report rate", RATE_HELP, Io::feature(f::REPORT_RATE, 1, 2), Codec::choice(options, 1)).boxed());
    }
    Ok(())
}

fn extended_report_rate(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if !l.has(f::EXTENDED_ADJUSTABLE_REPORT_RATE) {
        return Ok(());
    }
    const RATES: [&str; 7] = ["8 ms", "4 ms", "2 ms", "1 ms", "500 µs", "250 µs", "125 µs"];
    let flags = u16_at(&l.call(f::EXTENDED_ADJUSTABLE_REPORT_RATE, 1, &[])?, 0);
    let options: Vec<(u32, String)> = (0..7u32).filter(|i| flags >> i & 1 != 0).map(|i| (i, RATES[i as usize].to_string())).collect();
    if !options.is_empty() {
        out.push(
            Simple::new("report_rate", POINTER, "Report rate", RATE_HELP, Io::feature(f::EXTENDED_ADJUSTABLE_REPORT_RATE, 2, 3), Codec::choice(options, 1))
                .boxed(),
        );
    }
    Ok(())
}

/// ONBOARD_PROFILES: which stored profile is active, or none (host mode).
struct OnboardProfiles {
    options: Vec<(u16, String)>,
    value: Option<u16>,
}

impl Setting for OnboardProfiles {
    fn owns(&self, id: &str) -> bool {
        id == "onboard_profile"
    }

    fn controls(&self) -> Vec<Control> {
        let Some(v) = self.value else { return Vec::new() };
        let cur = self.options.iter().find(|o| o.0 == v).map(|o| o.1.clone()).unwrap_or_else(|| "Off".into());
        vec![Control {
            id: "onboard_profile".into(),
            name: format!("{DEVICE}: Onboard profile"),
            kind: ControlKind::Choice { options: self.options.iter().map(|o| o.1.clone()).collect() },
            value: ControlValue::Choice(cur),
            help: Some("A profile stored in the device sets report rate, sensitivity and buttons. Off lets the settings here apply.".into()),
        }]
    }

    fn read(&mut self, l: &mut Link) -> Result<()> {
        let mode = at(&l.call(f::ONBOARD_PROFILES, 2, &[])?, 0);
        self.value = Some(if mode == 1 { u16_at(&l.call(f::ONBOARD_PROFILES, 4, &[])?, 0) } else { 0 });
        Ok(())
    }

    fn write(&mut self, l: &mut Link, id: &str, v: &ControlValue) -> Result<()> {
        let ControlValue::Choice(s) = v else { return Err(Error::InvalidValue(id.into())) };
        let sector = self.options.iter().find(|o| &o.1 == s).ok_or_else(|| Error::InvalidValue(id.into()))?.0;
        if sector == 0 {
            l.call(f::ONBOARD_PROFILES, 1, &[2])?;
        } else {
            l.call(f::ONBOARD_PROFILES, 1, &[1])?;
            l.call(f::ONBOARD_PROFILES, 3, &sector.to_be_bytes())?;
        }
        self.value = Some(sector);
        Ok(())
    }
}

fn onboard_profiles(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if !l.has(f::ONBOARD_PROFILES) {
        return Ok(());
    }
    // The profile directory: sector 0, or the ROM copy (0x0100) when that
    // is empty. Entries of 4 bytes: sector (2), enabled, reserved; 0xFFFF
    // ends it. Each read returns 16 bytes from the given offset.
    let mut base = 0u8;
    let first = l.call(f::ONBOARD_PROFILES, 5, &[0, 0, 0, 0])?;
    if first.get(..4).is_none_or(|h| h == [0; 4] || h == [0xFF; 4]) {
        base = 1;
    }
    let mut options = vec![(0u16, "Off".to_string())];
    for i in 0..16u16 {
        let off = i * 4;
        let e = l.call(f::ONBOARD_PROFILES, 5, &[base, 0, (off >> 8) as u8, off as u8])?;
        let sector = u16_at(&e, 0);
        if sector == 0xFFFF {
            break;
        }
        if at(&e, 2) != 0 && !options.iter().any(|o| o.0 == sector) {
            let name = if sector >= 0x100 { format!("Read-only profile {}", sector & 0xFF) } else { format!("Profile {sector}") };
            options.push((sector, name));
        }
    }
    if options.len() > 1 {
        out.push(Box::new(OnboardProfiles { options, value: None }));
    }
    Ok(())
}

/// SMART_SHIFT(_ENHANCED): ratchet mode, the speed at which the wheel
/// switches to free-spinning, and (enhanced, some models) the torque.
struct SmartShift {
    feature: u16,
    torque: bool,
    value: Option<(u8, u8, u8)>,
}

impl SmartShift {
    fn functions(&self) -> (u8, u8) {
        if self.feature == f::SMART_SHIFT_ENHANCED { (1, 2) } else { (0, 1) }
    }
}

impl Setting for SmartShift {
    fn owns(&self, id: &str) -> bool {
        matches!(id, "scroll_ratchet" | "smart_shift" | "ratchet_torque")
    }

    fn controls(&self) -> Vec<Control> {
        let Some((mode, threshold, torque)) = self.value else { return Vec::new() };
        let mut out = vec![
            Control {
                id: "scroll_ratchet".into(),
                name: format!("{WHEEL}: Mode"),
                kind: ControlKind::Choice { options: vec!["Ratcheted".into(), "Free-spinning".into()] },
                value: ControlValue::Choice(if mode == 1 { "Free-spinning" } else { "Ratcheted" }.into()),
                help: None,
            },
            Control {
                id: "smart_shift".into(),
                name: format!("{WHEEL}: SmartShift sensitivity"),
                kind: ControlKind::Number { min: 1.0, max: 50.0, step: 1.0, unit: String::new() },
                value: ControlValue::Number(threshold.clamp(1, 50) as f64),
                help: Some("How fast a flick switches the wheel to free-spinning; 50 never switches.".into()),
            },
        ];
        if self.torque {
            out.push(Control {
                id: "ratchet_torque".into(),
                name: format!("{WHEEL}: Ratchet force"),
                kind: ControlKind::Number { min: 1.0, max: 100.0, step: 1.0, unit: "%".into() },
                value: ControlValue::Number(torque.clamp(1, 100) as f64),
                help: Some("How hard the ratchet clicks.".into()),
            });
        }
        out
    }

    fn read(&mut self, l: &mut Link) -> Result<()> {
        let r = l.call(self.feature, self.functions().0, &[])?;
        // Mode (1 free-spin, 2 ratchet), auto-disengage threshold, default
        // threshold; enhanced: torque in byte 2.
        let torque = if self.torque { at(&r, 2) } else { 0 };
        self.value = Some((at(&r, 0), at(&r, 1), torque));
        Ok(())
    }

    fn write(&mut self, l: &mut Link, id: &str, v: &ControlValue) -> Result<()> {
        let (mut mode, mut threshold, mut torque) = self.value.unwrap_or((2, 10, 50));
        match (id, v) {
            ("scroll_ratchet", ControlValue::Choice(s)) => mode = if s == "Free-spinning" { 1 } else { 2 },
            ("smart_shift", ControlValue::Number(n)) => threshold = n.round().clamp(1.0, 50.0) as u8,
            ("ratchet_torque", ControlValue::Number(n)) => torque = n.round().clamp(1.0, 100.0) as u8,
            _ => return Err(Error::InvalidValue(id.into())),
        }
        // 50 means "always ratcheted": the device takes 255 for that.
        let t = if threshold >= 50 { 255 } else { threshold };
        let w = self.functions().1;
        match id {
            "scroll_ratchet" => l.call(self.feature, w, &[mode, 0])?,
            "smart_shift" => l.call(self.feature, w, &[0, t])?,
            // The torque only takes effect written along with the mode.
            _ => l.call(self.feature, w, &[mode, 0, torque])?,
        };
        self.value = Some((mode, threshold, torque));
        Ok(())
    }
}

fn smart_shift(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    let feature = if l.has(f::SMART_SHIFT_ENHANCED) {
        f::SMART_SHIFT_ENHANCED
    } else if l.has(f::SMART_SHIFT) {
        f::SMART_SHIFT
    } else {
        return Ok(());
    };
    let torque = feature == f::SMART_SHIFT_ENHANCED && at(&l.call(feature, 0, &[])?, 0) & 1 != 0;
    out.push(Box::new(SmartShift { feature, torque, value: None }));
    Ok(())
}


fn hi_res_scrolling(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if l.has(f::HI_RES_SCROLLING) && !l.has(f::HIRES_WHEEL) {
        out.push(
            Simple::new("hires_scroll", WHEEL, "High resolution", "Finer scrolling steps. Turn off if scrolling is far too fast or slow.", Io::feature(f::HI_RES_SCROLLING, 0, 1), Codec::flag())
                .boxed(),
        );
    }
    Ok(())
}

fn lowres_wheel(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if l.has(f::LOWRES_WHEEL) && !l.has(f::HIRES_WHEEL) {
        out.push(
            Simple::new(
                super::buttons::WHEEL_DIVERT,
                WHEEL,
                "Send to OpenDeviceControl",
                "Turns become Scroll up / Scroll down keys on the Keys tab.",
                Io::feature(f::LOWRES_WHEEL, 0, 1),
                Codec::flag(),
            )
            .boxed(),
        );
    }
    Ok(())
}

fn hires_wheel(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if !l.has(f::HIRES_WHEEL) {
        return Ok(());
    }
    let io = || Io::feature(f::HIRES_WHEEL, 1, 2);
    out.push(Simple::new("hires_invert", WHEEL, "Reverse direction", "", io(), Codec::toggle(0x04, 0x04)).boxed());
    out.push(
        Simple::new("hires_resolution", WHEEL, "High resolution", "Finer scrolling steps. Turn off if scrolling is far too fast or slow.", io(), Codec::toggle(0x02, 0x02))
            .boxed(),
    );
    out.push(
        Simple::new(
            super::buttons::WHEEL_DIVERT,
            WHEEL,
            "Send to OpenDeviceControl",
            "Turns become Scroll up / Scroll down keys on the Keys tab.",
            io(),
            Codec::toggle(0x01, 0x01),
        )
        .boxed(),
    );
    Ok(())
}

fn thumb_wheel(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if !l.has(f::THUMB_WHEEL) {
        return Ok(());
    }
    let io = || Io::feature(f::THUMB_WHEEL, 1, 2);
    let bit = |on: u32| Codec::Toggle { on, off: 0, mask: on, bytes: 2, skip: 0, prefix: vec![] };
    out.push(Simple::new("thumb_invert", THUMB, "Reverse direction", "", io(), bit(0x0001)).boxed());
    out.push(
        Simple::new(
            super::buttons::THUMB_DIVERT,
            THUMB,
            "Send to OpenDeviceControl",
            "Turns become Thumb wheel left / right keys on the Keys tab.",
            io(),
            bit(0x0100),
        )
        .boxed(),
    );
    Ok(())
}

fn fn_inversion(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if l.has(f::K375S_FN_INVERSION) {
        // Per host: prefix the current host (0xFF does not work on some).
        let host = l.call(f::HOSTS_INFO, 0, &[]).map(|r| at(&r, 3)).unwrap_or(0xFF);
        let io = Io::Feature { feature: f::K375S_FN_INVERSION, read: Some(0), write: 1, prefix: vec![host], suffix: vec![], no_reply: false };
        out.push(Simple::new("fn_swap", KEYBOARD, "Swap Fx function", FN_HELP, io, Codec::Toggle { on: 1, off: 0, mask: 0xFF, bytes: 1, skip: 1, prefix: vec![] }).boxed());
    } else if l.has(f::NEW_FN_INVERSION) {
        out.push(Simple::new("fn_swap", KEYBOARD, "Swap Fx function", FN_HELP, Io::feature(f::NEW_FN_INVERSION, 0, 1), Codec::flag()).boxed());
    } else if l.has(f::FN_INVERSION) {
        out.push(Simple::new("fn_swap", KEYBOARD, "Swap Fx function", FN_HELP, Io::feature(f::FN_INVERSION, 0, 1), Codec::flag()).boxed());
    }
    Ok(())
}

/// BACKLIGHT2: mode, level and the fade-out delays, all in one record
/// (read with function 0, written whole with function 1).
struct Backlight2 {
    auto: bool,
    temp: bool,
    perm: bool,
    levels: u8,
    version: u8,
    /// enabled, options, supported, effects, level, dho, dhi, dpow (5 s units)
    state: Option<Backlight2State>,
}

#[derive(Debug, Clone, Copy)]
struct Backlight2State {
    enabled: bool,
    options: u8,
    mode: u8,
    level: u8,
    dho: u16,
    dhi: u16,
    dpow: u16,
}

impl Backlight2 {
    fn modes(&self) -> Vec<(u8, &'static str)> {
        let mut m = vec![(0xFF, "Off")];
        if self.auto {
            m.push((1, "Automatic"));
        }
        if self.perm {
            m.push((3, "Manual"));
        }
        if !(self.auto || self.temp || self.perm) {
            m.push((0, "On"));
        }
        m
    }
}

impl Setting for Backlight2 {
    fn owns(&self, id: &str) -> bool {
        id.starts_with("backlight")
    }

    fn controls(&self) -> Vec<Control> {
        let Some(s) = self.state else { return Vec::new() };
        let modes = self.modes();
        let cur = if s.enabled { s.mode } else { 0xFF };
        let mut out = vec![Control {
            id: "backlight".into(),
            name: format!("{KEYBOARD}: Backlight"),
            kind: ControlKind::Choice { options: modes.iter().map(|m| m.1.to_string()).collect() },
            value: ControlValue::Choice(modes.iter().find(|m| m.0 == cur).map(|m| m.1).unwrap_or("On").into()),
            help: Some("Keyboard illumination. The level only applies in Manual mode.".into()),
        }];
        if self.version >= 3 {
            if self.levels > 1 {
                out.push(Control {
                    id: "backlight_level".into(),
                    name: format!("{KEYBOARD}: Backlight level"),
                    kind: ControlKind::Number { min: 0.0, max: (self.levels - 1) as f64, step: 1.0, unit: String::new() },
                    value: ControlValue::Number(s.level as f64),
                    help: None,
                });
            }
            for (id, label, v) in [
                ("backlight_hands_out", "Backlight delay, hands away", s.dho),
                ("backlight_hands_in", "Backlight delay, hands near", s.dhi),
                ("backlight_powered", "Backlight delay, on power", s.dpow),
            ] {
                out.push(Control {
                    id: id.into(),
                    name: format!("{KEYBOARD}: {label}"),
                    kind: ControlKind::Number { min: 5.0, max: 600.0, step: 5.0, unit: "s".into() },
                    value: ControlValue::Number((v as f64 * 5.0).clamp(5.0, 600.0)),
                    help: Some("Seconds until the backlight fades out.".into()),
                });
            }
        }
        out
    }

    fn read(&mut self, l: &mut Link) -> Result<()> {
        let r = l.call(f::BACKLIGHT2, 0, &[])?;
        // Little-endian: enabled, options (mode in bits 3..4), supported,
        // effects (2), level, delays hands-out, hands-in, powered (2 each).
        let le = |i: usize| u16::from_le_bytes([at(&r, i), at(&r, i + 1)]);
        self.state = Some(Backlight2State {
            enabled: at(&r, 0) != 0,
            options: at(&r, 1),
            mode: (at(&r, 1) >> 3) & 0x03,
            level: at(&r, 5),
            dho: le(6),
            dhi: le(8),
            dpow: le(10),
        });
        self.auto = at(&r, 2) & 0x08 != 0;
        self.temp = at(&r, 2) & 0x10 != 0;
        self.perm = at(&r, 2) & 0x20 != 0;
        Ok(())
    }

    fn write(&mut self, l: &mut Link, id: &str, v: &ControlValue) -> Result<()> {
        let mut s = self.state.ok_or(Error::Offline)?;
        match (id, v) {
            ("backlight", ControlValue::Choice(c)) => {
                let m = self.modes().into_iter().find(|m| m.1 == c).ok_or_else(|| Error::InvalidValue(id.into()))?.0;
                s.enabled = m != 0xFF;
                if m != 0xFF {
                    s.mode = m;
                }
            }
            ("backlight_level", ControlValue::Number(n)) => s.level = *n as u8,
            ("backlight_hands_out", ControlValue::Number(n)) => s.dho = ((*n as u16) + 4) / 5,
            ("backlight_hands_in", ControlValue::Number(n)) => s.dhi = ((*n as u16) + 4) / 5,
            ("backlight_powered", ControlValue::Number(n)) => s.dpow = ((*n as u16) + 4) / 5,
            _ => return Err(Error::InvalidValue(id.into())),
        }
        s.options = (s.options & 0x07) | ((s.mode & 0x03) << 3);
        let level = if s.mode == 3 { s.level } else { 0 };
        let mut p = vec![s.enabled as u8, s.options, 0xFF, level];
        p.extend(s.dho.to_le_bytes());
        p.extend(s.dhi.to_le_bytes());
        p.extend(s.dpow.to_le_bytes());
        l.call(f::BACKLIGHT2, 1, &p)?;
        self.state = Some(s);
        Ok(())
    }
}

fn backlight2(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if !l.has(f::BACKLIGHT2) {
        return Ok(());
    }
    let version = l.version(f::BACKLIGHT2);
    let levels = if version >= 3 { at(&l.call(f::BACKLIGHT2, 2, &[])?, 0) } else { 0 };
    let mut b = Backlight2 { auto: false, temp: false, perm: false, levels, version, state: None };
    b.read(l)?;
    out.push(Box::new(b));
    Ok(())
}

fn backlight3(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if l.has(f::BACKLIGHT3) {
        let io = Io::Feature { feature: f::BACKLIGHT3, read: Some(1), write: 2, prefix: vec![], suffix: vec![0x09], no_reply: false };
        let mut c = Codec::range(0, 1000, 2);
        if let Codec::Range { unit, .. } = &mut c {
            *unit = "s";
        }
        out.push(Simple::new("backlight_time", KEYBOARD, "Backlight time", "How long the keyboard stays lit.", io, c).boxed());
    }
    Ok(())
}

/// A bit field shown as one toggle per bit.
struct Bits {
    id: &'static str,
    part: &'static str,
    io: Io,
    /// (bit value, label)
    bits: Vec<(u32, String)>,
    help: &'static str,
    value: Option<u32>,
    bytes: usize,
}

impl Setting for Bits {
    fn owns(&self, id: &str) -> bool {
        id.strip_prefix(self.id).is_some_and(|r| r.starts_with('_'))
    }

    fn controls(&self) -> Vec<Control> {
        let Some(v) = self.value else { return Vec::new() };
        self.bits
            .iter()
            .map(|(bit, label)| Control {
                id: format!("{}_{bit:x}", self.id),
                name: format!("{}: {label}", self.part),
                kind: ControlKind::Toggle,
                value: ControlValue::Bool(v & bit != 0),
                help: Some(self.help.into()),
            })
            .collect()
    }

    fn read(&mut self, l: &mut Link) -> Result<()> {
        if matches!(self.io, Io::Feature { read: None, .. }) {
            self.value.get_or_insert(0);
            return Ok(());
        }
        let r = self.io.read(l)?;
        self.value = Some(be(r.get(..self.bytes).unwrap_or(&[])));
        Ok(())
    }

    fn write(&mut self, l: &mut Link, id: &str, v: &ControlValue) -> Result<()> {
        let bit = id.rsplit('_').next().and_then(|b| u32::from_str_radix(b, 16).ok()).ok_or_else(|| Error::InvalidValue(id.into()))?;
        let ControlValue::Bool(on) = v else { return Err(Error::InvalidValue(id.into())) };
        let cur = self.value.unwrap_or(0);
        let new = if *on { cur | bit } else { cur & !bit };
        self.io.write(l, &to_be(new, self.bytes))?;
        self.value = Some(new);
        Ok(())
    }
}

fn disable_keys(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if !l.has(f::KEYBOARD_DISABLE_KEYS) {
        return Ok(());
    }
    let mask = at(&l.call(f::KEYBOARD_DISABLE_KEYS, 0, &[])?, 0) as u32;
    let bits: Vec<(u32, String)> = super::keys::DISABLE_KEYS
        .iter()
        .filter(|(b, _)| mask & *b as u32 != 0)
        .map(|(b, n)| (*b as u32, format!("Disable {n}")))
        .collect();
    if !bits.is_empty() {
        out.push(Box::new(Bits {
            id: "disable_key",
            part: KEYBOARD,
            io: Io::feature(f::KEYBOARD_DISABLE_KEYS, 1, 2),
            bits,
            help: "Turn this key off.",
            value: None,
            bytes: 1,
        }));
    }
    Ok(())
}

fn multiplatform(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if !l.has(f::MULTIPLATFORM) {
        return Ok(());
    }
    const OSS: [(&str, u16); 9] = [
        ("Linux", 0x0400),
        ("macOS", 0x2000),
        ("Windows", 0x0100),
        ("iOS", 0x4000),
        ("Android", 0x1000),
        ("WebOS", 0x8000),
        ("Chrome OS", 0x0800),
        ("Windows Embedded", 0x0200),
        ("Tizen", 0x0001),
    ];
    let info = l.call(f::MULTIPLATFORM, 0, &[])?;
    if at(&info, 0) & 0x02 == 0 {
        return Ok(()); // can't be set
    }
    let mut descriptors = Vec::new();
    for i in 0..at(&info, 2) {
        let d = l.call(f::MULTIPLATFORM, 1, &[i])?;
        descriptors.push((at(&d, 0), u16_at(&d, 2), u16_at(&d, 4), u16_at(&d, 6)));
    }
    let ver = |v: u16| if v == 0 { String::new() } else if v & 0xFF != 0 { format!("{}.{}", v >> 8, v & 0xFF) } else { (v >> 8).to_string() };
    let mut options: Vec<(u32, String)> = Vec::new();
    for (os, bit) in OSS {
        for &(platform, flags, low, high) in &descriptors {
            let name = if low == 0 && high == 0 { os.to_string() } else { format!("{os} {}-{}", ver(low), ver(high)) };
            if flags & bit != 0 && !options.iter().any(|o| o.0 == platform as u32 || o.1 == name) {
                options.push((platform as u32, name));
            }
        }
    }
    if !options.is_empty() {
        let io = Io::Feature { feature: f::MULTIPLATFORM, read: Some(0), write: 3, prefix: vec![], suffix: vec![], no_reply: false };
        let codec = Codec::Choice { options, bytes: 1, skip: 6, prefix: vec![0xFF] };
        out.push(Simple::new("platform", KEYBOARD, "Operating system", "Which system's key layout the keys follow.", io, codec).boxed());
    }
    Ok(())
}

fn dualplatform(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if l.has(f::DUALPLATFORM) && !l.has(f::MULTIPLATFORM) {
        let options = vec![opt(0, "iOS, macOS"), opt(1, "Android, Windows")];
        out.push(Simple::new("platform", KEYBOARD, "Operating system", "Which system's key layout the keys follow.", Io::feature(f::DUALPLATFORM, 0, 2), Codec::choice(options, 1)).boxed());
    }
    Ok(())
}

fn crown(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if !l.has(f::CROWN) {
        return Ok(());
    }
    out.push(
        Simple::new(
            super::buttons::CROWN_DIVERT,
            "Crown",
            "Send to OpenDeviceControl",
            "The crown's turns and presses become keys on the Keys tab.",
            Io::feature(f::CROWN, 1, 2),
            Codec::Toggle { on: 0x02, off: 0x01, mask: 0xFF, bytes: 1, skip: 0, prefix: vec![] },
        )
        .boxed(),
    );
    out.push(
        Simple::new(
            "crown_smooth",
            "Crown",
            "Smooth scrolling",
            "",
            Io::feature(f::CROWN, 1, 2),
            Codec::Toggle { on: 0x01, off: 0x02, mask: 0xFF, bytes: 1, skip: 1, prefix: vec![0x00] },
        )
        .boxed(),
    );
    Ok(())
}

fn gkeys(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if l.has(f::GKEY) {
        let io = Io::Feature { feature: f::GKEY, read: None, write: 2, prefix: vec![], suffix: vec![], no_reply: false };
        out.push(Simple::new(super::buttons::GKEYS_DIVERT, KEYBOARD, "Send G and M keys to OpenDeviceControl", "The G-keys become keys on the Keys tab.", io, Codec::flag()).boxed());
    }
    Ok(())
}

fn mkeys(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if l.has(f::MKEYS) {
        let n = at(&l.call(f::MKEYS, 0, &[])?, 0).min(8);
        let bits = (0..n).map(|i| (1u32 << i, format!("M{} LED", i + 1))).collect();
        out.push(Box::new(Bits {
            id: "mkey_led",
            part: KEYBOARD,
            io: Io::Feature { feature: f::MKEYS, read: None, write: 1, prefix: vec![], suffix: vec![], no_reply: false },
            bits,
            help: "Light up this M-key.",
            value: None,
            bytes: 1,
        }));
    }
    if l.has(f::MR) {
        let io = Io::Feature { feature: f::MR, read: None, write: 0, prefix: vec![], suffix: vec![], no_reply: false };
        out.push(Simple::new("mr_led", KEYBOARD, "MR LED", "Light up the MR key.", io, Codec::flag()).boxed());
    }
    Ok(())
}

/// BRIGHTNESS_CONTROL, with a separate on/off on some models.
struct Brightness {
    min: u16,
    max: u16,
    on_off: bool,
    value: Option<u16>,
}

impl Setting for Brightness {
    fn owns(&self, id: &str) -> bool {
        id == "brightness"
    }

    fn controls(&self) -> Vec<Control> {
        let Some(v) = self.value else { return Vec::new() };
        let min = if self.on_off { 0 } else { self.min };
        vec![Control {
            id: "brightness".into(),
            name: format!("{LIGHTING}: Brightness"),
            kind: ControlKind::Number { min: min as f64, max: self.max as f64, step: 1.0, unit: String::new() },
            value: ControlValue::Number(v as f64),
            help: Some("Overall brightness of the lighting.".into()),
        }]
    }

    fn read(&mut self, l: &mut Link) -> Result<()> {
        if self.on_off && at(&l.call(f::BRIGHTNESS_CONTROL, 3, &[])?, 0) & 1 == 0 {
            self.value = Some(0);
            return Ok(());
        }
        self.value = Some(u16_at(&l.call(f::BRIGHTNESS_CONTROL, 1, &[])?, 0));
        Ok(())
    }

    fn write(&mut self, l: &mut Link, id: &str, v: &ControlValue) -> Result<()> {
        let ControlValue::Number(n) = v else { return Err(Error::InvalidValue(id.into())) };
        let n = (*n as u16).min(self.max);
        if self.on_off {
            let off = n < self.min;
            l.call(f::BRIGHTNESS_CONTROL, 4, &[if off { 0 } else { 1 }])?;
            if off {
                self.value = Some(0);
                return Ok(());
            }
        }
        l.call(f::BRIGHTNESS_CONTROL, 2, &n.max(self.min).to_be_bytes())?;
        self.value = Some(n.max(self.min));
        Ok(())
    }
}

fn brightness(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if l.has(f::BRIGHTNESS_CONTROL) {
        let r = l.call(f::BRIGHTNESS_CONTROL, 0, &[])?;
        out.push(Box::new(Brightness { max: u16_at(&r, 0), min: u16_at(&r, 4), on_off: at(&r, 3) & 0x04 != 0, value: None }));
    }
    Ok(())
}

/// HAPTIC: feedback strength (0 turns it off), in steps or a range.
struct Haptic {
    stepped: bool,
    value: Option<u8>,
}

const HAPTIC_LEVELS: [(u8, &str); 5] = [(0, "Off"), (25, "Low"), (50, "Medium"), (75, "High"), (100, "Maximum")];

impl Setting for Haptic {
    fn owns(&self, id: &str) -> bool {
        id == "haptic_level"
    }

    fn controls(&self) -> Vec<Control> {
        let Some(v) = self.value else { return Vec::new() };
        let (kind, value) = if self.stepped {
            let cur = HAPTIC_LEVELS.iter().min_by_key(|l| (l.0 as i32 - v as i32).abs()).map(|l| l.1).unwrap_or("Off");
            (ControlKind::Choice { options: HAPTIC_LEVELS.iter().map(|l| l.1.to_string()).collect() }, ControlValue::Choice(cur.into()))
        } else {
            (ControlKind::Number { min: 0.0, max: 100.0, step: 1.0, unit: "%".into() }, ControlValue::Number(v as f64))
        };
        vec![Control { id: "haptic_level".into(), name: format!("{DEVICE}: Haptic feedback"), kind, value, help: Some("Strength of the haptic feedback; 0 turns it off.".into()) }]
    }

    fn read(&mut self, l: &mut Link) -> Result<()> {
        let r = l.call(f::HAPTIC, 1, &[])?;
        self.value = Some(if at(&r, 0) & 1 == 0 { 0 } else { at(&r, 1) });
        Ok(())
    }

    fn write(&mut self, l: &mut Link, id: &str, v: &ControlValue) -> Result<()> {
        let level = match v {
            ControlValue::Number(n) => n.round().clamp(0.0, 100.0) as u8,
            ControlValue::Choice(c) => HAPTIC_LEVELS.iter().find(|l| l.1 == c).ok_or_else(|| Error::InvalidValue(id.into()))?.0,
            _ => return Err(Error::InvalidValue(id.into())),
        };
        let p = if level == 0 { [0x00, 0x32] } else { [0x01, level] };
        l.call(f::HAPTIC, 2, &p)?;
        self.value = Some(level);
        Ok(())
    }
}

fn haptic(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if l.has(f::HAPTIC) {
        let r = l.call(f::HAPTIC, 1, &[])?;
        out.push(Box::new(Haptic { stepped: at(&r, 2) & 1 != 0, value: None }));
    }
    Ok(())
}

/// FORCE_SENSING_BUTTON: the force each button needs.
struct ForceSensing {
    /// (button, min, max)
    buttons: Vec<(u8, u16, u16)>,
    values: Vec<u16>,
}

impl Setting for ForceSensing {
    fn owns(&self, id: &str) -> bool {
        id.starts_with("force_")
    }

    fn controls(&self) -> Vec<Control> {
        self.buttons
            .iter()
            .zip(&self.values)
            .map(|(&(i, min, max), &v)| Control {
                id: format!("force_{i}"),
                name: if self.buttons.len() == 1 { format!("{DEVICE}: Button force") } else { format!("{DEVICE}: Button {} force", i + 1) },
                kind: ControlKind::Number { min: min as f64, max: max as f64, step: 1.0, unit: String::new() },
                value: ControlValue::Number(v as f64),
                help: Some("The force needed to press the button.".into()),
            })
            .collect()
    }

    fn read(&mut self, l: &mut Link) -> Result<()> {
        let mut values = Vec::new();
        for &(i, _, _) in &self.buttons {
            values.push(u16_at(&l.call(f::FORCE_SENSING_BUTTON, 2, &[i])?, 0));
        }
        self.values = values;
        Ok(())
    }

    fn write(&mut self, l: &mut Link, id: &str, v: &ControlValue) -> Result<()> {
        let i: u8 = id.trim_start_matches("force_").parse().map_err(|_| Error::InvalidValue(id.into()))?;
        let ControlValue::Number(n) = v else { return Err(Error::InvalidValue(id.into())) };
        let Some(pos) = self.buttons.iter().position(|b| b.0 == i) else { return Err(Error::UnknownControl(id.into())) };
        let (_, min, max) = self.buttons[pos];
        let n = (*n as u16).clamp(min, max);
        let mut p = vec![i];
        p.extend(n.to_be_bytes());
        l.call(f::FORCE_SENSING_BUTTON, 3, &p)?;
        if let Some(slot) = self.values.get_mut(pos) {
            *slot = n;
        }
        Ok(())
    }
}

fn force_sensing(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if !l.has(f::FORCE_SENSING_BUTTON) {
        return Ok(());
    }
    let count = at(&l.call(f::FORCE_SENSING_BUTTON, 0, &[])?, 0);
    let mut buttons = Vec::new();
    for i in 0..count {
        // capabilities, default (2), max (2), min (2)
        let r = l.call(f::FORCE_SENSING_BUTTON, 1, &[i])?;
        if at(&r, 0) & 1 != 0 {
            buttons.push((i, u16_at(&r, 5), u16_at(&r, 3)));
        }
    }
    if !buttons.is_empty() {
        out.push(Box::new(ForceSensing { buttons, values: Vec::new() }));
    }
    Ok(())
}

fn sidetone(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if l.has(f::SIDETONE) {
        let mut c = Codec::range(0, 100, 1);
        if let Codec::Range { unit, .. } = &mut c {
            *unit = "%";
        }
        out.push(Simple::new("sidetone", AUDIO, "Sidetone", "How much of your own voice you hear.", Io::feature(f::SIDETONE, 0, 1), c).boxed());
    }
    Ok(())
}

/// EQUALIZER: a gain per band, read and written together.
struct Equalizer {
    bands: Vec<String>,
    min: i8,
    max: i8,
    values: Vec<i8>,
}

impl Setting for Equalizer {
    fn owns(&self, id: &str) -> bool {
        id.starts_with("eq_")
    }

    fn controls(&self) -> Vec<Control> {
        self.bands
            .iter()
            .zip(&self.values)
            .enumerate()
            .map(|(i, (b, &v))| Control {
                id: format!("eq_{i}"),
                name: format!("Equalizer: {b}"),
                kind: ControlKind::Number { min: self.min as f64, max: self.max as f64, step: 1.0, unit: "dB".into() },
                value: ControlValue::Number(v as f64),
                help: None,
            })
            .collect()
    }

    fn read(&mut self, l: &mut Link) -> Result<()> {
        let r = l.call(f::EQUALIZER, 2, &[0])?;
        self.values = (0..self.bands.len()).map(|i| at(&r, 1 + i) as i8).collect();
        Ok(())
    }

    fn write(&mut self, l: &mut Link, id: &str, v: &ControlValue) -> Result<()> {
        let i: usize = id.trim_start_matches("eq_").parse().map_err(|_| Error::InvalidValue(id.into()))?;
        let ControlValue::Number(n) = v else { return Err(Error::InvalidValue(id.into())) };
        if i >= self.values.len() {
            return Err(Error::UnknownControl(id.into()));
        }
        self.values[i] = (n.round() as i64).clamp(self.min as i64, self.max as i64) as i8;
        let mut p = vec![0x02];
        p.extend(self.values.iter().map(|&g| g as u8));
        l.call(f::EQUALIZER, 3, &p)?;
        Ok(())
    }
}

fn equalizer(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if !l.has(f::EQUALIZER) {
        return Ok(());
    }
    let r = l.call(f::EQUALIZER, 0, &[])?;
    let (count, range) = (at(&r, 0) as usize, at(&r, 1) as i8);
    let (mut min, mut max) = (at(&r, 3) as i8, at(&r, 4) as i8);
    if min == 0 {
        min = -range;
    }
    if max == 0 {
        max = range;
    }
    let mut bands = Vec::new();
    for g in 0..count.div_ceil(7) {
        let fr = l.call(f::EQUALIZER, 1, &[(g * 7) as u8])?;
        for b in 0..7 {
            if g * 7 + b >= count {
                break;
            }
            bands.push(format!("{} Hz", u16_at(&fr, 2 * b + 1)));
        }
    }
    if !bands.is_empty() {
        out.push(Box::new(Equalizer { bands, min, max, values: Vec::new() }));
    }
    Ok(())
}

fn adc_power(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if l.has(f::ADC_MEASUREMENT) && l.version(f::ADC_MEASUREMENT) >= 2 {
        let mut c = Codec::range(0, 255, 1);
        if let Codec::Range { unit, .. } = &mut c {
            *unit = "min";
        }
        out.push(Simple::new("power_off", DEVICE, "Power off after", "Minutes without use before the device turns off; 0 never.", Io::feature(f::ADC_MEASUREMENT, 1, 2), c).boxed());
    }
    Ok(())
}

/// CHANGE_HOST: switch to another paired computer (an action, not kept).
struct ChangeHost {
    options: Vec<String>,
    current: Option<u8>,
}

impl Setting for ChangeHost {
    fn owns(&self, id: &str) -> bool {
        id == "change_host"
    }

    fn controls(&self) -> Vec<Control> {
        let Some(c) = self.current else { return Vec::new() };
        vec![Control {
            id: "change_host".into(),
            name: format!("{DEVICE}: Connected computer"),
            kind: ControlKind::Choice { options: self.options.clone() },
            value: ControlValue::Choice(self.options.get(c as usize).cloned().unwrap_or_default()),
            help: Some("Switches the device to that computer (Easy-Switch).".into()),
        }]
    }

    fn read(&mut self, l: &mut Link) -> Result<()> {
        self.current = Some(at(&l.call(f::CHANGE_HOST, 0, &[])?, 1));
        Ok(())
    }

    fn write(&mut self, l: &mut Link, id: &str, v: &ControlValue) -> Result<()> {
        let ControlValue::Choice(s) = v else { return Err(Error::InvalidValue(id.into())) };
        let host = self.options.iter().position(|o| o == s).ok_or_else(|| Error::InvalidValue(id.into()))? as u8;
        if Some(host) != self.current {
            l.call_no_reply(f::CHANGE_HOST, 1, &[host])?;
            self.current = Some(host);
        }
        Ok(())
    }

    fn persist(&self) -> bool {
        false
    }
}

/// Names of the computers the device is paired with (HOSTS_INFO), by host.
fn host_names(l: &mut Link) -> Vec<String> {
    let Ok(state) = l.call(f::HOSTS_INFO, 0, &[]) else { return Vec::new() };
    if at(&state, 0) & 0x01 == 0 {
        return Vec::new();
    }
    let mut names = Vec::new();
    for host in 0..at(&state, 2) {
        let Ok(info) = l.call(f::HOSTS_INFO, 1, &[host]) else { break };
        let len = at(&info, 4) as usize;
        let mut name = Vec::new();
        while name.len() < len {
            let Ok(piece) = l.call(f::HOSTS_INFO, 3, &[host, name.len() as u8]) else { break };
            let take = (len - name.len()).min(14);
            name.extend_from_slice(piece.get(2..2 + take).unwrap_or(&[]));
            if take == 0 {
                break;
            }
        }
        names.push(String::from_utf8_lossy(&name).trim_end_matches('\0').to_string());
    }
    names
}

fn change_host(l: &mut Link, out: &mut Vec<Box<dyn Setting>>) -> Result<()> {
    if !l.has(f::CHANGE_HOST) {
        return Ok(());
    }
    let r = l.call(f::CHANGE_HOST, 0, &[])?;
    let (count, current) = (at(&r, 0), at(&r, 1));
    if count < 2 {
        return Ok(());
    }
    let names = host_names(l);
    let options = (0..count)
        .map(|h| {
            let name = names.get(h as usize).filter(|n| !n.is_empty());
            let this = if h == current { " (this computer)" } else { "" };
            match name {
                Some(n) => format!("{}: {n}{this}", h + 1),
                None => format!("{}{this}", h + 1),
            }
        })
        .collect();
    out.push(Box::new(ChangeHost { options, current: None }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpi_lists_expand_steps() {
        // 200, step 50 up to 400, then 800, end.
        let bytes = [0x00, 0xC8, 0xE0, 0x32, 0x01, 0x90, 0x03, 0x20, 0x00, 0x00];
        assert_eq!(parse_dpi_list(&bytes), vec![200, 250, 300, 350, 400, 800]);
        assert!(parse_dpi_list(&[0, 0]).is_empty());
    }

    #[test]
    fn masked_toggles_keep_other_bits() {
        let c = Codec::toggle(0x40, 0x40);
        assert_eq!(c.decode(&[0x42, 0, 0]), Some(ControlValue::Bool(true)));
        assert_eq!(c.encode(&ControlValue::Bool(false), Some(&[0x42, 0, 0])), Some(vec![0x02]));
        assert_eq!(c.encode(&ControlValue::Bool(true), Some(&[0x02, 0, 0])), Some(vec![0x42]));
        // Two-byte thumb wheel invert bit.
        let t = Codec::Toggle { on: 0x0001, off: 0, mask: 0x0001, bytes: 2, skip: 0, prefix: vec![] };
        assert_eq!(t.encode(&ControlValue::Bool(true), Some(&[0x01, 0x00])), Some(vec![0x01, 0x01]));
    }

    #[test]
    fn choices_and_ranges_round_trip() {
        let c = Codec::Choice { options: vec![opt(0x81, "100"), opt(0x89, "900")], bytes: 1, skip: 0, prefix: vec![] };
        assert_eq!(c.decode(&[0x89, 0, 0]), Some(ControlValue::Choice("900".into())));
        assert_eq!(c.encode(&ControlValue::Choice("100".into()), None), Some(vec![0x81]));
        let r = Codec::Range { min: 0x2E, max: 0x1FF, bytes: 2, skip: 0, prefix: vec![], unit: "" };
        assert_eq!(r.decode(&[0x01, 0x00]), Some(ControlValue::Number(256.0)));
        assert_eq!(r.encode(&ControlValue::Number(300.0), None), Some(vec![0x01, 0x2C]));
    }
}
