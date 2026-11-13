//! Persistent user configuration (`config.toml`) and runtime state (`state.toml`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The project's previous names, newest first; their settings are migrated
/// on first start, and their environment variables still work.
pub const LEGACY_NAMES: [&str; 2] = ["rigmeld", "ferrocue"];

/// An environment variable `OPENDEVICECONTROL_<suffix>`, or its value under
/// a previous name.
fn env_dir(suffix: &str) -> Option<PathBuf> {
    std::iter::once("OPENDEVICECONTROL")
        .chain(LEGACY_NAMES.iter().map(|n| if *n == "rigmeld" { "RIGMELD" } else { "FERROCUE" }))
        .find_map(|p| std::env::var_os(format!("{p}_{suffix}")))
        .map(PathBuf::from)
}

/// `OPENDEVICECONTROL_CONFIG_DIR` overrides the location (portable installs, testing).
pub fn config_dir() -> PathBuf {
    env_dir("CONFIG_DIR").unwrap_or_else(|| dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("opendevicecontrol"))
}

/// `OPENDEVICECONTROL_DATA_DIR` overrides the location (portable installs, testing).
pub fn data_dir() -> PathBuf {
    env_dir("DATA_DIR").unwrap_or_else(|| dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("opendevicecontrol"))
}

/// One-time migration from a previous name: copy `config.toml`, `state.toml`
/// (energy counters) and the keymap backups if OpenDeviceControl has none
/// yet, from the newest previous name that has them. Returns true when
/// something was migrated. Skipped when the locations are overridden.
pub fn migrate_legacy() -> bool {
    if env_dir("CONFIG_DIR").is_some() || env_dir("DATA_DIR").is_some() {
        return false;
    }
    let mut migrated = false;
    let mut copy = |old: PathBuf, new: PathBuf| {
        if new.exists() || !old.exists() {
            return;
        }
        if let Some(parent) = new.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::copy(&old, &new) {
            Ok(_) => {
                log::info!("migrated {} -> {}", old.display(), new.display());
                migrated = true;
            }
            Err(e) => log::warn!("could not migrate {}: {e}", old.display()),
        }
    };
    for name in LEGACY_NAMES {
        if let Some(d) = dirs::config_dir() {
            copy(d.join(name).join("config.toml"), config_dir().join("config.toml"));
        }
        if let Some(d) = dirs::data_local_dir() {
            copy(d.join(name).join("state.toml"), data_dir().join("state.toml"));
            // The device's original keymaps, saved before the first change.
            if let Ok(entries) = std::fs::read_dir(d.join(name).join("keymap-backup")) {
                for e in entries.flatten() {
                    copy(e.path(), data_dir().join("keymap-backup").join(e.file_name()));
                }
            }
        }
    }
    migrated
}

/// The id a retired device id now goes by: the Aula cable connection used
/// to be a device of its own (`aula-f99-usb`), now both links are one
/// keyboard under the plain id.
pub(crate) fn merged_id(id: &str) -> Option<&'static str> {
    use opendevicecontrol_core::vendors::aula::keyboard::{MODELS, RETIRED_IDS};
    RETIRED_IDS.contains(&id).then(|| MODELS.iter().find(|m| id.strip_suffix("-usb") == Some(m.slug)).map(|m| m.slug)).flatten()
}

/// Move settings of retired device ids to the id that replaced them.
/// Keys are `<device>` or `<device>/<what>`; where both exist, the
/// replacement's own setting wins.
fn merge_devices<V>(map: &mut BTreeMap<String, V>) {
    let moved: Vec<(String, String)> = map
        .keys()
        .filter_map(|k| {
            let (dev, rest) = k.split_once('/').map_or((k.as_str(), None), |(d, r)| (d, Some(r)));
            let new = merged_id(dev)?;
            Some((k.clone(), rest.map_or(new.to_string(), |r| format!("{new}/{r}"))))
        })
        .collect();
    for (old, new) in moved {
        if let Some(v) = map.remove(&old) {
            map.entry(new).or_insert(v);
        }
    }
}

/// Keymap backups saved under a retired device id keep their place under
/// the new id unless it has its own (the first backup is the original).
pub fn merge_retired_backups() {
    let dir = data_dir().join("keymap-backup");
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some((dev, what)) = opendevicecontrol_core::vendors::aula::keyboard::RETIRED_IDS
            .iter()
            .find_map(|d| name.strip_prefix(&format!("{d}-")).map(|w| (*d, w)))
        else {
            continue;
        };
        let Some(new) = merged_id(dev) else { continue };
        let target = dir.join(format!("{new}-{what}"));
        if !target.exists() {
            match std::fs::rename(e.path(), &target) {
                Ok(()) => log::info!("keymap backup {name} now belongs to {new}"),
                Err(e) => log::warn!("could not move keymap backup {name}: {e}"),
            }
        }
    }
}

/// Rename keys of settings stored under ids that changed with the project
/// name (`<device>/rigmeld_lighting`, `<device>/rigmeld_key_N`).
fn migrate_ids<V>(map: &mut BTreeMap<String, V>) {
    let renamed: Vec<(String, String)> = map
        .keys()
        .filter_map(|k| {
            let n = k.replace("/rigmeld_lighting", "/software_lighting").replace("/rigmeld_key_", "/host_key_");
            (n != *k).then(|| (k.clone(), n))
        })
        .collect();
    for (old, new) in renamed {
        if let Some(v) = map.remove(&old) {
            map.entry(new).or_insert(v);
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub mqtt: Mqtt,
    /// Fan policy per `"<device id>/<fan channel>"`.
    pub fans: BTreeMap<String, FanPolicy>,
    /// Per `"<device id>/<fan channel>"`: the settings of each fan mode, so
    /// switching to another mode and back restores them.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub fan_memory: BTreeMap<String, FanMemory>,
    /// Lighting per device id.
    pub lighting: BTreeMap<String, Lighting>,
    /// Action per `"<device id>/<key id>"` (G-keys etc.).
    pub keys: BTreeMap<String, KeyAction>,
    /// Remembered on/off settings per `"<device id>/<control id>"`, re-applied
    /// when the device is opened (devices forget them on power loss).
    pub toggles: BTreeMap<String, bool>,
    /// Other remembered settings per `"<device id>/<control id>"`, for the
    /// controls a driver lists in `Device::remembered_controls` (pump mode,
    /// LED counts…).
    pub settings: BTreeMap<String, opendevicecontrol_core::ControlValue>,
    /// Corsair RGB memory over the SMBus (experimental, off by default).
    pub ram: Ram,
    /// Extra temperature sources on Windows (CPU temperature).
    pub sensors: Sensors,
    /// Home Assistant: sensors sent (true) or not (false) per
    /// `"<device id>/<sensor id>"`, where the user changed the default
    /// (everything except detail sensors is sent by default).
    pub ha_sensors: BTreeMap<String, bool>,
}

/// Where This PC gets temperatures on Windows besides the OS (which has
/// almost none). HWiNFO and LibreHardwareMonitor are always read when they
/// share their sensors; PawnIO is opt-in. See `docs/windows-sensors.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sensors {
    /// Read the CPU's thermal registers through the PawnIO driver (needs
    /// PawnIO, its AMDFamily17/IntelMSR module and administrator rights).
    pub pawnio: bool,
    /// LibreHardwareMonitor's web server port ("Remote Web Server").
    pub lhm_port: u16,
}

impl Default for Sensors {
    fn default() -> Self {
        Sensors { pawnio: false, lhm_port: 8085 }
    }
}

impl Sensors {
    /// Hand the settings to opendevicecontrol-core.
    pub fn apply(&self) {
        opendevicecontrol_core::system::configure_sensors(self.pawnio, self.lhm_port);
    }
}

/// RAM lighting talks to the motherboard SMBus, which also carries the
/// memory's SPD EEPROMs, so it is opt-in. See `docs/ram.md`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ram {
    pub enabled: bool,
}

/// Lighting effect for a device. Colours are `#rrggbb` strings in the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum Effect {
    /// One colour per zone (a single colour applies to every zone).
    Static { colors: Vec<HexColor> },
    /// Fades a colour in and out.
    Breathing { color: HexColor, speed: f32 },
    /// Whole device cycles through the hue wheel.
    ColorCycle { speed: f32 },
    /// Rainbow moving across the device, left to right.
    RainbowWave { speed: f32 },
    /// Two colours sweeping across the device.
    ColorWave { color: HexColor, color2: HexColor, speed: f32 },
    // ---- sound-reactive (what the computer is playing) ----
    /// Frequency bars, bass on the left (bars rise on keyboards).
    Spectrum(SoundStyle),
    /// Whole device flashes on the beat.
    BeatPulse(SoundStyle),
    /// Rings spread from the centre on every beat.
    Ripple(SoundStyle),
    /// Loudness flows across the device, newest on the left.
    SoundStream(SoundStyle),
    /// Random LEDs light up on the beat and fade.
    Rain(SoundStyle),
    /// Fills from the left with the loudness, like a VU meter.
    LevelMeter(SoundStyle),
    /// A beam sweeping around the centre, as long as the sound is loud.
    Radar(SoundStyle),
    /// The whole device in one colour picked by the loudness.
    LevelColor(SoundStyle),
}

/// Colours and sensitivity shared by the sound-reactive effects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundStyle {
    /// Rainbow colours instead of `color` → `color2`.
    pub rainbow: bool,
    pub color: HexColor,
    pub color2: HexColor,
    /// Gain applied to the sound levels (0.25–4; 1 = automatic level).
    pub sensitivity: f32,
}

impl Default for SoundStyle {
    fn default() -> Self {
        Self {
            rainbow: true,
            color: HexColor(opendevicecontrol_core::Rgb::new(0, 200, 255)),
            color2: HexColor(opendevicecontrol_core::Rgb::new(255, 0, 120)),
            sensitivity: 1.0,
        }
    }
}

impl SoundStyle {
    /// Colour at `t` in 0..1 along the style's palette.
    pub fn pick(&self, t: f32) -> opendevicecontrol_core::Rgb {
        let t = t.clamp(0.0, 1.0);
        if self.rainbow { opendevicecontrol_core::Rgb::from_hsv(t * 300.0, 1.0, 1.0) } else { self.color.0.lerp(self.color2.0, t) }
    }
}

impl Effect {
    /// Every effect (Home Assistant's effect list).
    pub const NAMES: [&'static str; 13] = [
        "Static",
        "Breathing",
        "Color cycle",
        "Rainbow wave",
        "Color wave",
        "Spectrum",
        "Beat pulse",
        "Ripple",
        "Sound stream",
        "Rain",
        "Level meter",
        "Radar",
        "Level colour",
    ];
    /// The plain (not sound-reactive) effects.
    pub const LIGHT_NAMES: &'static [&'static str] = &["Static", "Breathing", "Color cycle", "Rainbow wave", "Color wave"];
    /// The sound-reactive effects.
    pub const SOUND_NAMES: &'static [&'static str] =
        &["Spectrum", "Beat pulse", "Ripple", "Sound stream", "Rain", "Level meter", "Radar", "Level colour"];

    /// Whether the effect follows what the computer is playing.
    pub fn is_sound(&self) -> bool {
        self.sound_style().is_some()
    }

    pub fn sound_style(&self) -> Option<&SoundStyle> {
        match self {
            Effect::Spectrum(s)
            | Effect::BeatPulse(s)
            | Effect::Ripple(s)
            | Effect::SoundStream(s)
            | Effect::Rain(s)
            | Effect::LevelMeter(s)
            | Effect::Radar(s)
            | Effect::LevelColor(s) => Some(s),
            _ => None,
        }
    }

    pub fn sound_style_mut(&mut self) -> Option<&mut SoundStyle> {
        match self {
            Effect::Spectrum(s)
            | Effect::BeatPulse(s)
            | Effect::Ripple(s)
            | Effect::SoundStream(s)
            | Effect::Rain(s)
            | Effect::LevelMeter(s)
            | Effect::Radar(s)
            | Effect::LevelColor(s) => Some(s),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Effect::Static { .. } => "Static",
            Effect::Breathing { .. } => "Breathing",
            Effect::ColorCycle { .. } => "Color cycle",
            Effect::RainbowWave { .. } => "Rainbow wave",
            Effect::ColorWave { .. } => "Color wave",
            Effect::Spectrum(_) => "Spectrum",
            Effect::BeatPulse(_) => "Beat pulse",
            Effect::Ripple(_) => "Ripple",
            Effect::SoundStream(_) => "Sound stream",
            Effect::Rain(_) => "Rain",
            Effect::LevelMeter(_) => "Level meter",
            Effect::Radar(_) => "Radar",
            Effect::LevelColor(_) => "Level colour",
        }
    }

    /// The effect called `name`, keeping this effect's colour where it applies.
    pub fn switch_to(&self, name: &str) -> Option<Effect> {
        let c = self.primary_color();
        // Sound effects keep the current sound style.
        let style = self.sound_style().cloned().unwrap_or_default();
        Some(match name {
            "Spectrum" => Effect::Spectrum(style),
            "Beat pulse" => Effect::BeatPulse(style),
            "Ripple" => Effect::Ripple(style),
            "Sound stream" => Effect::SoundStream(style),
            "Rain" => Effect::Rain(style),
            "Level meter" => Effect::LevelMeter(style),
            "Radar" => Effect::Radar(style),
            "Level colour" => Effect::LevelColor(style),
            "Static" => match self {
                Effect::Static { .. } => self.clone(),
                _ => Effect::Static { colors: vec![c] },
            },
            "Breathing" => Effect::Breathing { color: c, speed: 1.0 },
            "Color cycle" => Effect::ColorCycle { speed: 1.0 },
            "Rainbow wave" => Effect::RainbowWave { speed: 1.0 },
            "Color wave" => Effect::ColorWave { color: c, color2: HexColor(opendevicecontrol_core::Rgb::new(0, 0, 255)), speed: 1.0 },
            _ => return None,
        })
    }

    pub fn primary_color(&self) -> HexColor {
        match self {
            Effect::Static { colors } => colors.first().copied().unwrap_or_default(),
            Effect::Breathing { color, .. } | Effect::ColorWave { color, .. } => *color,
            other => other.sound_style().map(|s| s.color).unwrap_or_default(),
        }
    }

    /// Set the main colour (used by Home Assistant's colour picker).
    pub fn set_color(&mut self, c: HexColor) {
        match self {
            Effect::Static { colors } => *colors = vec![c],
            Effect::Breathing { color, .. } | Effect::ColorWave { color, .. } => *color = c,
            other => match other.sound_style_mut() {
                Some(s) => {
                    s.color = c;
                    s.rainbow = false;
                }
                None => *other = Effect::Static { colors: vec![c] },
            },
        }
    }
}

/// A colour stored as `"#rrggbb"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HexColor(pub opendevicecontrol_core::Rgb);

impl Default for HexColor {
    fn default() -> Self {
        HexColor(opendevicecontrol_core::Rgb::new(34, 190, 178))
    }
}

impl Serialize for HexColor {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_hex())
    }
}

impl<'de> Deserialize<'de> for HexColor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let h = s.trim_start_matches('#');
        let v = u32::from_str_radix(h, 16).map_err(serde::de::Error::custom)?;
        if h.len() != 6 {
            return Err(serde::de::Error::custom("expected #rrggbb"));
        }
        Ok(HexColor(opendevicecontrol_core::Rgb::new((v >> 16) as u8, (v >> 8) as u8, v as u8)))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lighting {
    pub on: bool,
    /// 0–100 %.
    pub brightness: u8,
    pub effect: Effect,
    /// The last effect of the other family (plain vs sound-reactive), so
    /// switching between them in the GUI comes back to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other: Option<Effect>,
}

impl Default for Lighting {
    fn default() -> Self {
        Self { on: true, brightness: 100, effect: Effect::RainbowWave { speed: 1.0 }, other: None }
    }
}

/// What a G-key (or other assignable key) does. Every press is also sent to
/// Home Assistant as an event, whatever the action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum KeyAction {
    /// Nothing locally (Home Assistant event only).
    #[default]
    None,
    /// A key combination, e.g. `ctrl+shift+esc`.
    Hotkey { keys: String },
    /// Type a piece of text.
    Text { text: String },
    /// Run a program or command.
    Launch { command: String },
    /// A media key.
    Media { key: String },
}

impl KeyAction {
    pub const NAMES: [&'static str; 5] = ["None", "Hotkey", "Text", "Launch", "Media"];

    pub fn name(&self) -> &'static str {
        match self {
            KeyAction::None => "None",
            KeyAction::Hotkey { .. } => "Hotkey",
            KeyAction::Text { .. } => "Text",
            KeyAction::Launch { .. } => "Launch",
            KeyAction::Media { .. } => "Media",
        }
    }

    pub fn from_name(name: &str) -> KeyAction {
        match name {
            "Hotkey" => KeyAction::Hotkey { keys: String::new() },
            "Text" => KeyAction::Text { text: String::new() },
            "Launch" => KeyAction::Launch { command: String::new() },
            "Media" => KeyAction::Media { key: "play_pause".into() },
            _ => KeyAction::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub poll_interval_ms: u64,
    pub start_minimized: bool,
    pub close_to_tray: bool,
    pub autostart: bool,
    /// Allow Home Assistant to change fan modes and device settings.
    pub allow_remote_control: bool,
}

impl Default for General {
    fn default() -> Self {
        Self { poll_interval_ms: 2000, start_minimized: false, close_to_tray: true, autostart: false, allow_remote_control: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mqtt {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// Identifies this computer in Home Assistant. Use the SAME value on every
    /// OS you dual-boot so entities and history are shared.
    pub node_id: String,
    pub discovery_prefix: String,
    pub base_topic: String,
    /// Minimum seconds between state publishes (rate limit for HA recorder).
    pub publish_interval_s: u64,
    /// The previous name whose Home Assistant discovery entries and retained
    /// topics still have to be removed (set when migrating, cleared once
    /// done).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_cleanup: Option<String>,
}

impl Default for Mqtt {
    fn default() -> Self {
        Self {
            enabled: false,
            host: "homeassistant.local".into(),
            port: 1883,
            username: String::new(),
            password: String::new(),
            node_id: default_node_id(),
            discovery_prefix: "homeassistant".into(),
            base_topic: "opendevicecontrol".into(),
            publish_interval_s: 5,
            legacy_cleanup: None,
        }
    }
}

pub fn default_node_id() -> String {
    let host = gethostname::gethostname().to_string_lossy().to_lowercase();
    sanitize_id(&host)
}

pub fn sanitize_id(s: &str) -> String {
    let id: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let id = id.trim_matches('_').to_string();
    if id.is_empty() { "pc".into() } else { id }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum FanPolicy {
    /// Firmware fan curve (default, includes zero-RPM mode on HXi).
    #[default]
    Hardware,
    /// Fixed duty, percent.
    Fixed { duty: u8 },
    /// Software curve: (temperature °C, duty %) points, interpolated linearly.
    Curve {
        points: Vec<(f64, u8)>,
        /// Curve input as `"<device id>/<sensor id>"` (any device, or
        /// `system/<sensor id>` for [`SYSTEM_SOURCE`] sensors such as the CPU
        /// temperature). `None`: the channel's own `curve_sensor`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
}

/// The last settings used in each fan mode of a channel.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FanMemory {
    /// Last Fixed duty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duty: Option<u8>,
    /// Last Curve points; `None` while no curve was ever set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub points: Option<Vec<(f64, u8)>>,
    /// Last Curve input (meaningful only with `points`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl FanMemory {
    fn remember(&mut self, p: &FanPolicy) {
        match p {
            FanPolicy::Hardware => {}
            FanPolicy::Fixed { duty } => self.duty = Some(*duty),
            FanPolicy::Curve { points, source } => {
                self.points = Some(points.clone());
                self.source = source.clone();
            }
        }
    }
}

/// Pseudo device id for host sensors (CPU temperature…) usable as curve input.
pub const SYSTEM_SOURCE: &str = "system";

/// Split a curve source into (device id, sensor id).
pub fn split_source(source: &str) -> Option<(&str, &str)> {
    source.split_once('/').filter(|(d, s)| !d.is_empty() && !s.is_empty())
}

impl FanPolicy {
    pub const MODES: [&'static str; 3] = ["Hardware", "Fixed", "Curve"];

    pub fn mode_name(&self) -> &'static str {
        match self {
            FanPolicy::Hardware => "Hardware",
            FanPolicy::Fixed { .. } => "Fixed",
            FanPolicy::Curve { .. } => "Curve",
        }
    }

    pub fn default_curve() -> Vec<(f64, u8)> {
        vec![(30.0, 30), (40.0, 35), (50.0, 50), (60.0, 75), (65.0, 100)]
    }
}

/// Evaluate a fan curve; points need not be sorted.
pub fn eval_curve(points: &[(f64, u8)], temp: f64) -> u8 {
    let mut p: Vec<_> = points.to_vec();
    p.sort_by(|a, b| a.0.total_cmp(&b.0));
    match p.as_slice() {
        [] => 100,
        [only] => only.1,
        _ => {
            if temp <= p[0].0 {
                return p[0].1;
            }
            for w in p.windows(2) {
                let ((t0, d0), (t1, d1)) = (w[0], w[1]);
                if temp <= t1 {
                    let f = if t1 > t0 { (temp - t0) / (t1 - t0) } else { 1.0 };
                    return (d0 as f64 + (d1 as f64 - d0 as f64) * f).round() as u8;
                }
            }
            p.last().unwrap().1
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("config.toml")
    }

    pub fn load() -> Self {
        let path = Self::path();
        let mut c: Self = match std::fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                log::error!("invalid config {}: {e}; using defaults", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        };
        let before = c.clone();
        c.upgrade();
        if c != before && path.exists() {
            if let Err(e) = c.save() {
                log::warn!("could not save the upgraded config: {e}");
            }
        }
        c
    }

    /// Bring a config written under a previous name up to date.
    fn upgrade(&mut self) {
        // Configs migrated from a previous name still carry its default MQTT
        // topic: move to the new default, and have Home Assistant's old
        // discovery entries cleaned up once (see `hass`).
        if LEGACY_NAMES.contains(&self.mqtt.base_topic.as_str()) {
            self.mqtt.legacy_cleanup = Some(self.mqtt.base_topic.clone());
            self.mqtt.base_topic = "opendevicecontrol".into();
        }
        migrate_ids(&mut self.toggles);
        migrate_ids(&mut self.keys);
        migrate_ids(&mut self.settings);
        merge_devices(&mut self.fans);
        merge_devices(&mut self.fan_memory);
        merge_devices(&mut self.lighting);
        merge_devices(&mut self.keys);
        merge_devices(&mut self.toggles);
        merge_devices(&mut self.settings);
        merge_devices(&mut self.ha_sensors);
    }

    /// Write the file. Saves are serialized (see [`save_shared`]).
    pub fn save(&self) -> std::io::Result<()> {
        let _g = SAVE_LOCK.lock();
        self.save_unlocked()
    }

    fn save_unlocked(&self) -> std::io::Result<()> {
        let path = Self::path();
        std::fs::create_dir_all(path.parent().unwrap())?;
        let s = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        write_private(&path, s.as_bytes())
    }

    /// Whether a sensor goes to Home Assistant.
    pub fn sends(&self, device: &str, s: &opendevicecontrol_core::Sensor) -> bool {
        self.ha_sensors.get(&format!("{device}/{}", s.id)).copied().unwrap_or(!s.extra)
    }

    /// Choose whether a sensor goes to Home Assistant (stores only changes
    /// from the default).
    pub fn set_sends(&mut self, device: &str, s: &opendevicecontrol_core::Sensor, on: bool) {
        let key = format!("{device}/{}", s.id);
        if on == !s.extra {
            self.ha_sensors.remove(&key);
        } else {
            self.ha_sensors.insert(key, on);
        }
    }

    pub fn fan_policy(&self, device: &str, channel: &str) -> FanPolicy {
        self.fans.get(&format!("{device}/{channel}")).cloned().unwrap_or_default()
    }

    pub fn set_fan_policy(&mut self, device: &str, channel: &str, p: FanPolicy) {
        let key = format!("{device}/{channel}");
        let mem = self.fan_memory.entry(key.clone()).or_default();
        // Configs written before `fan_memory` existed only have the
        // current mode's settings: keep them too.
        if let Some(old) = self.fans.get(&key) {
            mem.remember(old);
        }
        mem.remember(&p);
        if *mem == FanMemory::default() {
            self.fan_memory.remove(&key);
        }
        self.fans.insert(key, p);
    }

    /// The policy for switching a channel to `mode` ("Hardware", "Fixed",
    /// "Curve"), with that mode's last settings. `fixed_fallback` is the
    /// duty used when Fixed was never set.
    pub fn fan_policy_for_mode(&self, device: &str, channel: &str, mode: &str, fixed_fallback: u8) -> Option<FanPolicy> {
        let key = format!("{device}/{channel}");
        let current = self.fan_policy(device, channel);
        if current.mode_name() == mode {
            return Some(current);
        }
        let mut mem = self.fan_memory.get(&key).cloned().unwrap_or_default();
        mem.remember(&current);
        match mode {
            "Hardware" => Some(FanPolicy::Hardware),
            "Fixed" => Some(FanPolicy::Fixed { duty: mem.duty.unwrap_or(fixed_fallback) }),
            "Curve" => Some(match mem.points {
                Some(points) => FanPolicy::Curve { points, source: mem.source },
                None => FanPolicy::Curve { points: FanPolicy::default_curve(), source: None },
            }),
            _ => None,
        }
    }

    pub fn lighting(&self, device: &str) -> Lighting {
        self.lighting.get(device).cloned().unwrap_or_default()
    }

    pub fn key_action(&self, device: &str, key: &str) -> KeyAction {
        self.keys.get(&format!("{device}/{key}")).cloned().unwrap_or_default()
    }
}

/// Serializes config file writes, so they reach the disk in order.
static SAVE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Save the shared configuration. The snapshot is taken while holding the
/// save lock, so whichever save runs last writes the newest settings, even
/// when several threads change and save the config at once.
pub fn save_shared(config: &parking_lot::RwLock<Config>) -> std::io::Result<()> {
    let _g = SAVE_LOCK.lock();
    let snapshot = config.read().clone();
    snapshot.save_unlocked()
}

/// Write a file readable only by the current user (it may hold the MQTT
/// password), atomically through a temporary file of its own.
fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.{}.tmp", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
    let res = (|| {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// Runtime state that survives restarts (energy counters).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    /// Accumulated estimated AC input energy per device, kWh.
    pub energy_kwh: BTreeMap<String, f64>,
}

impl State {
    fn path() -> PathBuf {
        data_dir().join("state.toml")
    }
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self) {
        let p = Self::path();
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        if let Ok(s) = toml::to_string(self) {
            let _ = write_private(&p, s.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_a_rigmeld_config() {
        let mut c: Config = toml::from_str(
            r#"
            [mqtt]
            base_topic = "rigmeld"
            [toggles]
            "aula-f99/rigmeld_lighting" = false
            "k55-rgb/win_lock" = true
            [keys."aula-f99-usb/rigmeld_key_3"]
            action = "none"
            "#,
        )
        .unwrap();
        c.upgrade();
        assert_eq!(c.mqtt.base_topic, "opendevicecontrol");
        assert_eq!(c.mqtt.legacy_cleanup.as_deref(), Some("rigmeld"));
        assert_eq!(c.toggles.get("aula-f99/software_lighting"), Some(&false));
        assert_eq!(c.toggles.get("k55-rgb/win_lock"), Some(&true));
        // The cable's own id is merged into the keyboard's.
        assert!(c.keys.contains_key("aula-f99/host_key_3"));
        // A custom topic is left alone.
        let mut c = Config::default();
        c.mqtt.base_topic = "mine".into();
        c.upgrade();
        assert_eq!((c.mqtt.base_topic.as_str(), c.mqtt.legacy_cleanup.as_deref()), ("mine", None));
    }

    #[test]
    fn merges_the_aula_cable_id() {
        let mut c: Config = toml::from_str(
            r#"
            [toggles]
            "aula-f99/win_lock" = false
            "aula-f99-usb/win_lock" = true
            "aula-f99-usb/software_lighting" = false
            [lighting.aula-f99-usb]
            on = false
            "#,
        )
        .unwrap();
        c.upgrade();
        assert_eq!(c.toggles.get("aula-f99/win_lock"), Some(&false));
        assert_eq!(c.toggles.get("aula-f99/software_lighting"), Some(&false));
        assert!(c.lighting.contains_key("aula-f99") && !c.lighting.contains_key("aula-f99-usb"));
        assert!(!c.toggles.keys().any(|k| k.starts_with("aula-f99-usb")));
        assert_eq!(merged_id("aula-f75-usb"), Some("aula-f75"));
        assert_eq!(merged_id("k55-rgb"), None);
    }

    #[test]
    fn curve_interpolates_and_clamps() {
        let c = vec![(30.0, 30), (50.0, 50), (70.0, 100)];
        assert_eq!(eval_curve(&c, 10.0), 30);
        assert_eq!(eval_curve(&c, 40.0), 40);
        assert_eq!(eval_curve(&c, 60.0), 75);
        assert_eq!(eval_curve(&c, 90.0), 100);
    }

    #[test]
    fn ram_is_off_by_default_and_old_configs_load() {
        assert!(!Config::default().ram.enabled);
        let c: Config = toml::from_str("[general]
poll_interval_ms = 1000
").unwrap();
        assert!(!c.ram.enabled);
        let c: Config = toml::from_str("[ram]
enabled = true
").unwrap();
        assert!(c.ram.enabled);
    }

    #[test]
    fn pawnio_sensors_are_off_by_default() {
        assert_eq!(Config::default().sensors, Sensors { pawnio: false, lhm_port: 8085 });
        let c: Config = toml::from_str("[sensors]
pawnio = true
").unwrap();
        assert_eq!(c.sensors, Sensors { pawnio: true, lhm_port: 8085 });
    }

    #[test]
    fn home_assistant_sensor_choice() {
        use opendevicecontrol_core::{Sensor, SensorKind};
        let standard = Sensor::new("cpu_temp", "CPU temperature", SensorKind::Temperature, 50.0);
        let detail = Sensor::new("lhm_x", "Core #1", SensorKind::Temperature, 50.0).extra();
        let mut c = Config::default();
        assert!(c.sends("this_pc", &standard) && !c.sends("this_pc", &detail));
        c.set_sends("this_pc", &detail, true);
        c.set_sends("this_pc", &standard, false);
        assert!(!c.sends("this_pc", &standard) && c.sends("this_pc", &detail));
        assert!(c.sends("other", &standard), "per device");
        // Saved as overrides only; back to default removes the entry.
        let back: Config = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.ha_sensors, c.ha_sensors);
        c.set_sends("this_pc", &detail, false);
        c.set_sends("this_pc", &standard, true);
        assert!(c.ha_sensors.is_empty());
    }

    #[test]
    fn config_roundtrip() {
        let mut c = Config::default();
        c.set_fan_policy("hx850i", "fan", FanPolicy::Curve { points: FanPolicy::default_curve(), source: None });
        c.set_fan_policy("link-hub", "ch3", FanPolicy::Curve { points: FanPolicy::default_curve(), source: Some("hx850i/temp_vrm".into()) });
        c.settings.insert("h150i-elite-rgb/pump_mode".into(), opendevicecontrol_core::ControlValue::Choice("Quiet".into()));
        c.settings.insert("commander-pro/led_count_1".into(), opendevicecontrol_core::ControlValue::Number(24.0));
        let s = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn fan_modes_remember_their_settings() {
        let curve = FanPolicy::Curve { points: vec![(35.0, 40), (70.0, 100)], source: Some("system/cpu_temp".into()) };
        let mut c = Config::default();
        c.set_fan_policy("hx850i", "fan", curve.clone());
        let to = |c: &Config, m| c.fan_policy_for_mode("hx850i", "fan", m, 50).unwrap();
        // Curve -> Fixed (never set: fallback) -> Fixed 70 -> Curve.
        assert_eq!(to(&c, "Fixed"), FanPolicy::Fixed { duty: 50 });
        c.set_fan_policy("hx850i", "fan", FanPolicy::Fixed { duty: 70 });
        assert_eq!(to(&c, "Curve"), curve);
        c.set_fan_policy("hx850i", "fan", to(&c, "Curve"));
        // ... -> Hardware -> Fixed comes back at 70, and survives a restart.
        c.set_fan_policy("hx850i", "fan", FanPolicy::Hardware);
        let back: Config = toml::from_str(&toml::to_string_pretty(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        assert_eq!(to(&back, "Fixed"), FanPolicy::Fixed { duty: 70 });
        assert_eq!(to(&back, "Curve"), curve);
        assert_eq!(to(&back, "Hardware"), FanPolicy::Hardware);
        assert_eq!(back.fan_policy_for_mode("hx850i", "fan", "Bogus", 50), None);
        // A channel never configured gets the defaults.
        assert_eq!(
            back.fan_policy_for_mode("other", "fan", "Curve", 50),
            Some(FanPolicy::Curve { points: FanPolicy::default_curve(), source: None })
        );
    }

    #[test]
    fn old_fan_config_keeps_its_curve_when_switching() {
        // Written before `fan_memory` existed: only the current mode.
        let old = "[fans.\"hx850i/fan\"]
mode = \"curve\"
points = [[30.0, 30], [60.0, 100]]
source = \"system/cpu_temp\"
";
        let mut c: Config = toml::from_str(old).unwrap();
        let curve = c.fan_policy("hx850i", "fan");
        c.set_fan_policy("hx850i", "fan", FanPolicy::Fixed { duty: 60 });
        assert_eq!(c.fan_policy_for_mode("hx850i", "fan", "Curve", 50), Some(curve));
    }

    #[test]
    fn concurrent_private_writes_do_not_collide() {
        let dir = std::env::temp_dir().join(format!("odc-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for j in 0..20 {
                        write_private(&path, format!("writer {i} pass {j}\n").repeat(200).as_bytes()).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        // The file holds one writer's complete data, and no temp files remain.
        let s = std::fs::read_to_string(&path).unwrap();
        let first = s.lines().next().unwrap().to_string();
        assert!(s.lines().all(|l| l == first) && s.lines().count() == 200);
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "{left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_curve_config_still_loads() {
        // Written by opendevicecontrol before curve sources existed.
        let old = "[fans.\"hx850i/fan\"]
mode = \"curve\"
points = [[30.0, 30], [60.0, 100]]
";
        let c: Config = toml::from_str(old).unwrap();
        assert_eq!(c.fan_policy("hx850i", "fan"), FanPolicy::Curve { points: vec![(30.0, 30), (60.0, 100)], source: None });
        assert_eq!(split_source("hx850i/temp_vrm"), Some(("hx850i", "temp_vrm")));
        assert_eq!(split_source("nonsense"), None);
    }
}
