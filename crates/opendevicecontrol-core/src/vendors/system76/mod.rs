//! System76 laptop keyboard backlights (Linux): the `system76` /
//! `system76_acpi` kernel drivers' `system76*::kbd_backlight` LED.
//!
//! The LED has a `brightness` (0..=`max_brightness`) and either one `color`
//! attribute or the zone attributes `color_left`, `color_center`,
//! `color_right` and `color_extra`, each `RRGGBB` in hex. Single-colour RGB
//! keyboards (Gazelle, Oryx…) listen to the zone attributes too, so every
//! one present gets the same colour and the keyboard is one lighting zone.
//!
//! The attributes are root-only by default; the udev rule makes these few
//! writable. The Fn keys change brightness and colour in firmware, so the
//! current values are read back on every poll.

use crate::devices::{Device, Ids, HW_LIGHTING_PREFIX, SOFTWARE_LIGHTING};
use crate::model::*;
use crate::{Error, Result};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Least time between frames: every write is a call into the embedded
/// controller.
const FRAME_GAP: Duration = Duration::from_millis(50);
/// Resend the last frame this often, in case the Fn keys changed it.
const REFRESH: Duration = Duration::from_secs(2);
/// The embedded controller applies writes in the background: reading
/// `brightness` right after a write still gives the old value, so for this
/// long after writing, what was written is taken as the current state.
const SETTLE: Duration = Duration::from_secs(1);
const ZONE_FILES: [&str; 4] = ["color_left", "color_center", "color_right", "color_extra"];

/// What the keyboard shows: brightness (0..=max) and colour.
#[derive(Debug, Clone, Copy, PartialEq)]
struct State {
    brightness: u32,
    color: Rgb,
}

pub struct Backlight {
    info: DeviceInfo,
    dir: PathBuf,
    color_files: Vec<&'static str>,
    max: u32,
    /// Last values read from (or written to) the LED.
    current: State,
    software: bool,
    /// What the keyboard showed before OpenDeviceControl's lighting took
    /// over, put back when it hands over again.
    saved: Option<State>,
    frame: Option<Rgb>,
    last_write: Option<Instant>,
}

fn read(dir: &Path, file: &str) -> Result<String> {
    std::fs::read_to_string(dir.join(file)).map(|s| s.trim().to_string()).map_err(|e| Error::Sysfs(format!("{}/{file}: {e}", dir.display())))
}

fn write(dir: &Path, file: &str, value: &str) -> Result<()> {
    std::fs::write(dir.join(file), value).map_err(|e| {
        let hint = if e.kind() == std::io::ErrorKind::PermissionDenied { " (install OpenDeviceControl's udev rule)" } else { "" };
        Error::Sysfs(format!("{}/{file}: {e}{hint}", dir.display()))
    })
}

fn parse_hex(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(Rgb::new((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// A colour from the effects engine as backlight brightness (its brightest
/// channel) and a full-brightness colour.
fn split(c: Rgb, max: u32) -> State {
    let peak = c.r.max(c.g).max(c.b);
    if peak == 0 {
        return State { brightness: 0, color: Rgb::new(255, 255, 255) };
    }
    let up = |v: u8| ((v as u32 * 255 + peak as u32 / 2) / peak as u32) as u8;
    State { brightness: (peak as u32 * max + 127) / 255, color: Rgb::new(up(c.r), up(c.g), up(c.b)) }
}

impl Backlight {
    pub fn open(dir: PathBuf, id: String, model: String) -> Result<Self> {
        let color_files: Vec<&'static str> =
            if dir.join("color").exists() { vec!["color"] } else { ZONE_FILES.into_iter().filter(|f| dir.join(f).exists()).collect() };
        if color_files.is_empty() {
            return Err(Error::NotSupported(" (no colour attributes: a single-colour backlight)"));
        }
        let max = read(&dir, "max_brightness")?.parse().map_err(|_| Error::Sysfs("unreadable max_brightness".into()))?;
        let mut b = Backlight {
            info: DeviceInfo {
                id,
                kind: DeviceKind::Keyboard,
                manufacturer: "System76".into(),
                model,
                vendor_string: None,
                product_string: dir.file_name().map(|n| n.to_string_lossy().into_owned()),
                usb_vid: 0,
                usb_pid: 0,
                serial: None,
                firmware: None,
            },
            color_files,
            max,
            current: State { brightness: 0, color: Rgb::new(255, 255, 255) },
            software: false,
            saved: None,
            frame: None,
            last_write: None,
            dir,
        };
        b.current = b.read_state()?;
        Ok(b)
    }

    /// Pick up changes made outside OpenDeviceControl (the Fn keys), once
    /// our own last write has settled.
    fn refresh(&mut self) -> Result<()> {
        if self.last_write.is_none_or(|t| t.elapsed() >= SETTLE) {
            self.current = self.read_state()?;
        }
        Ok(())
    }

    fn read_state(&self) -> Result<State> {
        let brightness = read(&self.dir, "brightness")?.parse().unwrap_or(0);
        let color = parse_hex(&read(&self.dir, self.color_files[0])?).unwrap_or(Rgb::new(255, 255, 255));
        Ok(State { brightness, color })
    }

    /// Write what differs from `current` (everything when `force`).
    fn apply(&mut self, want: State, force: bool) -> Result<()> {
        // Colour first, so a brightness change never flashes the old colour.
        if force || want.color != self.current.color {
            let hex = want.color.to_hex();
            let hex = hex.trim_start_matches('#').to_ascii_uppercase();
            for f in &self.color_files {
                write(&self.dir, f, &hex)?;
            }
        }
        if force || want.brightness != self.current.brightness {
            write(&self.dir, "brightness", &want.brightness.min(self.max).to_string())?;
        }
        self.current = want;
        self.last_write = Some(Instant::now());
        Ok(())
    }

    fn send_frame(&mut self, force: bool) -> Result<()> {
        match self.frame {
            Some(c) => self.apply(split(c, self.max), force),
            None => Ok(()),
        }
    }

    fn hand_back(&mut self) -> Result<()> {
        self.software = false;
        self.frame = None;
        match self.saved.take() {
            Some(s) => self.apply(s, true),
            None => Ok(()),
        }
    }

    fn percent(&self) -> f64 {
        if self.max == 0 {
            0.0
        } else {
            (self.current.brightness as f64 * 100.0 / self.max as f64).round()
        }
    }

    fn controls(&self) -> Vec<Control> {
        let mut v = vec![Control {
            id: SOFTWARE_LIGHTING.into(),
            name: "OpenDeviceControl lighting".into(),
            kind: ControlKind::Toggle,
            value: ControlValue::Bool(self.software),
            help: Some("On: OpenDeviceControl's lighting effects drive the keyboard backlight. Off: the colour and brightness below, which the Fn keys change too.".into()),
        }];
        if !self.software {
            v.push(Control {
                id: format!("{HW_LIGHTING_PREFIX}color"),
                name: "Colour".into(),
                kind: ControlKind::Color,
                value: ControlValue::Choice(self.current.color.to_hex()),
                help: None,
            });
            v.push(Control {
                id: format!("{HW_LIGHTING_PREFIX}brightness"),
                name: "Brightness".into(),
                kind: ControlKind::Number { min: 0.0, max: 100.0, step: 1.0, unit: "%".into() },
                value: ControlValue::Number(self.percent()),
                help: None,
            });
        }
        v
    }
}

impl Device for Backlight {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn poll(&mut self) -> Result<Snapshot> {
        if self.software {
            if self.last_write.is_none_or(|t| t.elapsed() >= REFRESH) {
                self.send_frame(true)?;
            }
        } else {
            self.refresh()?;
        }
        Ok(Snapshot { sensors: Vec::new(), controls: self.controls() })
    }

    fn poll_interval(&self) -> Option<Duration> {
        Some(Duration::from_secs(1))
    }

    fn frame_interval(&self) -> Option<Duration> {
        Some(FRAME_GAP)
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()> {
        let bad = || Error::InvalidValue(id.to_string());
        if id == SOFTWARE_LIGHTING {
            let ControlValue::Bool(on) = value else { return Err(bad()) };
            if *on == self.software {
                return Ok(());
            }
            if !*on {
                return self.hand_back();
            }
            self.refresh()?;
            self.saved = Some(self.current);
            self.software = true;
            return self.send_frame(true);
        }
        match (id.strip_prefix(HW_LIGHTING_PREFIX), value) {
            (Some("color"), ControlValue::Choice(hex)) => {
                let color = parse_hex(hex).ok_or_else(bad)?;
                let want = State { color, ..self.current };
                self.apply(want, false)
            }
            (Some("brightness"), ControlValue::Number(p)) if p.is_finite() => {
                let brightness = (p.clamp(0.0, 100.0) * self.max as f64 / 100.0).round() as u32;
                let want = State { brightness, ..self.current };
                self.apply(want, false)
            }
            (Some(_), _) => Err(bad()),
            (None, _) => Err(Error::UnknownControl(id.to_string())),
        }
    }

    fn light_zones(&self) -> Vec<LightZone> {
        vec![LightZone { id: "keyboard".into(), name: "Keyboard".into(), leds: 1 }]
    }

    fn set_lights(&mut self, colors: &[Rgb]) -> Result<()> {
        if !self.software {
            return Ok(());
        }
        let c = colors.first().copied().unwrap_or(Rgb::BLACK);
        if self.frame == Some(c) {
            return Ok(());
        }
        self.frame = Some(c);
        self.send_frame(false)
    }

    fn preview_lights(&mut self, _t: f64) -> Option<Vec<Rgb>> {
        (!self.software).then(|| {
            let s = self.current;
            vec![s.color.scale(if self.max == 0 { 0.0 } else { s.brightness as f32 / self.max as f32 })]
        })
    }

    fn release(&mut self) -> Result<()> {
        self.hand_back()
    }
}

/// The laptop's model from DMI, e.g. "Gazelle (gaze15)".
#[cfg(target_os = "linux")]
fn laptop_model() -> String {
    let dmi = |f: &str| std::fs::read_to_string(format!("/sys/class/dmi/id/{f}")).map(|s| s.trim().to_string()).unwrap_or_default();
    match (dmi("product_name"), dmi("product_version")) {
        (n, v) if !n.is_empty() && !v.is_empty() && n != v => format!("{n} ({v}) keyboard"),
        (n, _) if !n.is_empty() => format!("{n} keyboard"),
        _ => "Laptop keyboard".into(),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn discover(ids: &mut Ids, skip_paths: &[CString], out: &mut Vec<(CString, Box<dyn Device>)>) {
    let Ok(entries) = std::fs::read_dir("/sys/class/leds") else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("system76") && name.ends_with("::kbd_backlight")) {
            continue;
        }
        let path = CString::new(format!("sysfs:/sys/class/leds/{name}")).expect("no NUL");
        if skip_paths.contains(&path) {
            continue;
        }
        let id = ids.next("system76-keyboard");
        match Backlight::open(e.path(), id.clone(), laptop_model()) {
            Ok(b) => {
                log::info!("opened {} backlight ({id}), {} colour attribute(s)", b.info.model, b.color_files.len());
                out.push((path, Box::new(b)));
            }
            Err(Error::NotSupported(why)) => log::debug!("{name}: not an RGB backlight{why}"),
            Err(e) => log::warn!("failed to open {name}: {e}"),
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn discover(_ids: &mut Ids, _skip_paths: &[CString], _out: &mut Vec<(CString, Box<dyn Device>)>) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(files: &[(&str, &str)]) -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("odc-system76-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (f, v) in files {
            std::fs::write(dir.join(f), v).unwrap();
        }
        dir
    }

    fn zones(brightness: &str) -> PathBuf {
        fake(&[
            ("brightness", brightness),
            ("max_brightness", "255\n"),
            ("color_left", "FFFFFF\n"),
            ("color_center", "FFFFFF\n"),
            ("color_right", "FFFFFF\n"),
            ("color_extra", "FFFFFF\n"),
        ])
    }

    fn get(dir: &Path, f: &str) -> String {
        std::fs::read_to_string(dir.join(f)).unwrap().trim().to_string()
    }

    #[test]
    fn colours_split_into_brightness_and_full_colour() {
        assert_eq!(split(Rgb::new(255, 0, 0), 255), State { brightness: 255, color: Rgb::new(255, 0, 0) });
        assert_eq!(split(Rgb::new(128, 64, 0), 255), State { brightness: 128, color: Rgb::new(255, 128, 0) });
        assert_eq!(split(Rgb::new(255, 255, 255), 100), State { brightness: 100, color: Rgb::new(255, 255, 255) });
        assert_eq!(split(Rgb::BLACK, 255).brightness, 0);
    }

    #[test]
    fn hex_round_trip() {
        assert_eq!(parse_hex("FF8000\n"), Some(Rgb::new(255, 128, 0)));
        assert_eq!(parse_hex("#00ff00"), Some(Rgb::new(0, 255, 0)));
        assert_eq!(parse_hex("FFF"), None);
    }

    #[test]
    fn hardware_controls_write_every_zone() {
        let dir = zones("0\n");
        let mut b = Backlight::open(dir.clone(), "kb".into(), "Test".into()).unwrap();
        b.set_control("hw_color", &ControlValue::Choice("#ff0000".into())).unwrap();
        b.set_control("hw_brightness", &ControlValue::Number(50.0)).unwrap();
        for f in ZONE_FILES {
            assert_eq!(get(&dir, f), "FF0000");
        }
        assert_eq!(get(&dir, "brightness"), "128");
        // Like the embedded controller, the attribute still reads the old
        // value right after the write: what was written counts.
        std::fs::write(dir.join("brightness"), "0").unwrap();
        let brightness = |b: &mut Backlight| b.poll().unwrap().controls.into_iter().find(|c| c.id == "hw_brightness").unwrap().value;
        assert_eq!(brightness(&mut b), ControlValue::Number(50.0));
        // Later changes (the Fn keys) are picked up.
        b.last_write = Some(Instant::now() - SETTLE);
        std::fs::write(dir.join("brightness"), "255").unwrap();
        assert_eq!(brightness(&mut b), ControlValue::Number(100.0));
    }

    #[test]
    fn software_lighting_hands_back_what_was_there() {
        let dir = zones("40\n");
        let mut b = Backlight::open(dir.clone(), "kb".into(), "Test".into()).unwrap();
        // Off: frames are ignored.
        b.set_lights(&[Rgb::new(0, 0, 255)]).unwrap();
        assert_eq!(get(&dir, "color_left"), "FFFFFF");
        b.set_control(SOFTWARE_LIGHTING, &ControlValue::Bool(true)).unwrap();
        b.set_lights(&[Rgb::new(0, 0, 128)]).unwrap();
        assert_eq!(get(&dir, "color_left"), "0000FF");
        assert_eq!(get(&dir, "brightness"), "128");
        b.release().unwrap();
        assert_eq!(get(&dir, "color_left"), "FFFFFF");
        assert_eq!(get(&dir, "brightness"), "40");
    }

    #[test]
    fn single_color_attribute_and_plain_backlights() {
        let dir = fake(&[("brightness", "1"), ("max_brightness", "5"), ("color", "00FF00")]);
        let mut b = Backlight::open(dir.clone(), "kb".into(), "Test".into()).unwrap();
        assert_eq!(b.color_files, vec!["color"]);
        b.set_control("hw_brightness", &ControlValue::Number(100.0)).unwrap();
        assert_eq!(get(&dir, "brightness"), "5");
        let plain = fake(&[("brightness", "1"), ("max_brightness", "5")]);
        assert!(matches!(Backlight::open(plain, "kb".into(), "Test".into()), Err(Error::NotSupported(_))));
    }
}
