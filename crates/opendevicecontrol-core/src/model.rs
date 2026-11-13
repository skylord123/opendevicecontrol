//! Device-agnostic data model shared by all drivers and front-ends.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeviceKind {
    PowerSupply,
    Keyboard,
    Mouse,
    Mousepad,
    Headset,
    Cooler,
    FanHub,
    Memory,
    /// The computer OpenDeviceControl runs on (system stats and presence).
    Computer,
    Other,
}

/// An RGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };
    pub const WHITE: Rgb = Rgb { r: 255, g: 255, b: 255 };

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Scale by `f` in 0..=1.
    pub fn scale(self, f: f32) -> Self {
        let f = f.clamp(0.0, 1.0);
        let s = |c: u8| (c as f32 * f).round() as u8;
        Self::new(s(self.r), s(self.g), s(self.b))
    }

    /// Linear blend towards `other` by `t` in 0..=1.
    pub fn lerp(self, other: Rgb, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Self::new(m(self.r, other.r), m(self.g, other.g), m(self.b, other.b))
    }

    /// From hue (0..360), saturation and value (0..1).
    pub fn from_hsv(h: f32, s: f32, v: f32) -> Self {
        let h = h.rem_euclid(360.0) / 60.0;
        let c = v * s;
        let x = c * (1.0 - (h % 2.0 - 1.0).abs());
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = v - c;
        let to = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        Self::new(to(r), to(g), to(b))
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// A group of LEDs that a device can colour. `leds` LEDs, laid out left to
/// right (position 0..1 across the zone) for effects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightZone {
    pub id: String,
    pub name: String,
    pub leds: usize,
}

/// Where one LED sits on a device, for drawing it (e.g. a keyboard key).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedShape {
    /// Index into the colours passed to `set_lights` (all zones, in order).
    pub led: usize,
    /// Full name, e.g. "Left Shift".
    pub name: String,
    /// Short label drawn on the shape, e.g. "Shift" (may be empty).
    pub label: String,
    /// Left, top, width, height in device-specific units (only the
    /// proportions matter).
    pub rect: [f32; 4],
}

/// A key or button whose presses the device reports to opendevicecontrol (G-keys,
/// macro keys, profile buttons…), usable for macros and Home Assistant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputKey {
    pub id: String,
    pub name: String,
    /// True for keys meant to be user-assigned (G-keys), false for keys
    /// with a built-in meaning that are still reported (brightness, MR…).
    pub assignable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    Pressed(String),
    Released(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Stable identifier, e.g. `hx850i` (suffixed `-2`, `-3`… for duplicates).
    pub id: String,
    pub kind: DeviceKind,
    pub manufacturer: String,
    pub model: String,
    /// Strings reported by the device firmware itself.
    pub vendor_string: Option<String>,
    pub product_string: Option<String>,
    pub usb_vid: u16,
    pub usb_pid: u16,
    pub serial: Option<String>,
    #[serde(default)]
    pub firmware: Option<String>,
}

impl DeviceInfo {
    /// "Corsair HX850i", or just the model when there is no manufacturer.
    pub fn display_name(&self) -> String {
        if self.manufacturer.is_empty() { self.model.clone() } else { format!("{} {}", self.manufacturer, self.model) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SensorKind {
    Voltage,
    Current,
    Power,
    Energy,
    Temperature,
    FanSpeed,
    Duty,
    Efficiency,
    Duration,
    /// Battery charge in percent (wireless peripherals).
    Battery,
    /// 1 while the battery is charging, 0 otherwise.
    Charging,
    /// Utilisation / fill level in percent (CPU load, disk usage…).
    Load,
    /// Gibibytes (memory, disk space).
    DataSize,
    /// Mebibytes per second (network, disk throughput).
    DataRate,
    /// Megahertz (clocks).
    Frequency,
    /// On/off state as 1/0 (locked, active…); a binary sensor in HA.
    State,
    /// Coolant flow in litres per hour.
    Flow,
}

impl SensorKind {
    pub fn unit(self) -> &'static str {
        match self {
            SensorKind::Voltage => "V",
            SensorKind::Current => "A",
            SensorKind::Power => "W",
            SensorKind::Energy => "kWh",
            SensorKind::Temperature => "°C",
            SensorKind::FanSpeed => "RPM",
            SensorKind::Duty => "%",
            SensorKind::Efficiency => "%",
            SensorKind::Duration => "s",
            SensorKind::Battery => "%",
            SensorKind::Charging => "",
            SensorKind::Load => "%",
            SensorKind::DataSize => "GiB",
            SensorKind::DataRate => "MiB/s",
            SensorKind::Frequency => "MHz",
            SensorKind::State => "",
            SensorKind::Flow => "L/h",
        }
    }

    /// Sensor group heading, as hardware monitors list them.
    pub fn category(self) -> &'static str {
        match self {
            SensorKind::Temperature => "Temperatures",
            SensorKind::Voltage => "Voltages",
            SensorKind::Current => "Currents",
            SensorKind::Power => "Powers",
            SensorKind::Energy => "Energy",
            SensorKind::FanSpeed => "Fans",
            SensorKind::Duty => "Fan control",
            SensorKind::Flow => "Flow",
            SensorKind::Frequency => "Clocks",
            SensorKind::Load | SensorKind::Efficiency => "Utilization",
            SensorKind::Battery => "Levels",
            SensorKind::DataSize => "Data",
            SensorKind::DataRate => "Throughput",
            SensorKind::Duration => "Times",
            SensorKind::Charging | SensorKind::State => "States",
        }
    }

    /// Order of the groups under a piece of hardware.
    pub fn category_order(self) -> u8 {
        match self {
            SensorKind::State | SensorKind::Charging => 0,
            SensorKind::Voltage => 1,
            SensorKind::Temperature => 2,
            SensorKind::Power => 3,
            SensorKind::Current => 4,
            SensorKind::FanSpeed => 5,
            SensorKind::Duty => 6,
            SensorKind::Flow => 7,
            SensorKind::Frequency => 8,
            SensorKind::Load | SensorKind::Efficiency => 9,
            SensorKind::Battery => 10,
            SensorKind::DataSize => 11,
            SensorKind::DataRate => 12,
            SensorKind::Energy => 13,
            SensorKind::Duration => 14,
        }
    }

    /// On/off kinds, published to Home Assistant as binary sensors.
    pub fn is_binary(self) -> bool {
        matches!(self, SensorKind::Charging | SensorKind::State)
    }

    pub fn precision(self) -> usize {
        match self {
            SensorKind::Voltage => 2,
            SensorKind::Current => 2,
            SensorKind::Power => 1,
            SensorKind::Energy => 3,
            SensorKind::Temperature => 1,
            SensorKind::FanSpeed | SensorKind::Duty | SensorKind::Efficiency | SensorKind::Duration => 0,
            SensorKind::Battery | SensorKind::Charging => 0,
            SensorKind::Load => 1,
            SensorKind::DataSize => 2,
            SensorKind::DataRate => 2,
            SensorKind::Frequency | SensorKind::State => 0,
            SensorKind::Flow => 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sensor {
    /// Machine id, unique per device (`rail_12v_power`).
    pub id: String,
    /// Human name (`+12V power`).
    pub name: String,
    pub kind: SensorKind,
    pub value: f64,
    /// True if the value is computed by opendevicecontrol rather than measured.
    pub estimated: bool,
    /// Diagnostic sensors are hidden from the main dashboard by default.
    pub diagnostic: bool,
    /// Home Assistant device class override (e.g. `occupancy`), when the
    /// kind alone doesn't say what the value means.
    #[serde(default)]
    pub class: Option<String>,
    /// Icon hint (Material Design Icons name, e.g. `mdi:lock`).
    #[serde(default)]
    pub icon: Option<String>,
    /// The [`Hardware`] node it belongs to (This PC's sensor tree).
    #[serde(default)]
    pub hardware: Option<String>,
    /// Detail sensor: shown everywhere, but only sent to Home Assistant when
    /// the user picks it (a PC has hundreds).
    #[serde(default)]
    pub extra: bool,
    /// Where the value comes from (`LibreHardwareMonitor`, `hwmon`…).
    #[serde(default)]
    pub source: Option<String>,
    /// For a time elapsed since an event (idle time, uptime): the event
    /// itself. Home Assistant gets this as a timestamp sensor, which only
    /// changes when the event happens again, instead of a counter that
    /// changes every poll.
    #[serde(default)]
    pub since: Option<Since>,
}

/// The moment a duration counts from, for Home Assistant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Since {
    /// Unix time, seconds.
    pub at: i64,
    /// Home Assistant entity name ("Last input", "Last boot").
    pub name: String,
}

impl Sensor {
    pub fn new(id: &str, name: &str, kind: SensorKind, value: f64) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind,
            value,
            estimated: false,
            diagnostic: false,
            class: None,
            icon: None,
            hardware: None,
            extra: false,
            source: None,
            since: None,
        }
    }
    /// Published to Home Assistant as the timestamp `at` (Unix seconds)
    /// named `name`, instead of this duration.
    pub fn since(mut self, at: i64, name: &str) -> Self {
        self.since = Some(Since { at, name: name.into() });
        self
    }
    /// Place in the hardware tree.
    pub fn on(mut self, hardware: &str) -> Self {
        self.hardware = Some(hardware.into());
        self
    }
    /// Not sent to Home Assistant unless picked.
    pub fn extra(mut self) -> Self {
        self.extra = true;
        self
    }
    pub fn from_source(mut self, source: &str) -> Self {
        self.source = Some(source.into());
        self
    }
    pub fn class(mut self, class: &str) -> Self {
        self.class = Some(class.into());
        self
    }
    pub fn icon(mut self, icon: &str) -> Self {
        self.icon = Some(icon.into());
        self
    }
    pub fn estimated(mut self) -> Self {
        self.estimated = true;
        self
    }
    pub fn diagnostic(mut self) -> Self {
        self.diagnostic = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ControlKind {
    Choice { options: Vec<String> },
    Number { min: f64, max: f64, step: f64, unit: String },
    Toggle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ControlValue {
    Bool(bool),
    Number(f64),
    Choice(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Control {
    pub id: String,
    pub name: String,
    pub kind: ControlKind,
    pub value: ControlValue,
    /// Short explanation shown in the GUI.
    #[serde(default)]
    pub help: Option<String>,
}

/// A fan that opendevicecontrol can drive (hardware curve or software duty).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanChannel {
    pub id: String,
    pub name: String,
    /// Lowest duty the firmware accepts in software mode.
    pub min_duty: u8,
    /// Sensor used as input for software fan curves.
    pub curve_sensor: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanTarget {
    /// Let the device firmware control the fan.
    Hardware,
    /// Fixed duty in percent.
    Duty(u8),
}

/// Static, rarely changing limits reported by the device (OCP / OVP / OTP).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limit {
    pub name: String,
    pub kind: SensorKind,
    pub value: f64,
}

/// What a piece of hardware is, for icons and grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HardwareKind {
    Computer,
    Motherboard,
    /// Super I/O, embedded controller or other monitoring chip.
    Chip,
    Cpu,
    Memory,
    Gpu,
    Storage,
    Network,
    Battery,
    PowerSupply,
    Cooler,
    Other,
}

impl HardwareKind {
    /// Order of top-level nodes, as hardware monitors list them.
    pub fn order(self) -> u8 {
        match self {
            HardwareKind::Computer => 0,
            HardwareKind::Motherboard => 1,
            HardwareKind::Chip => 2,
            HardwareKind::Cpu => 3,
            HardwareKind::Memory => 4,
            HardwareKind::Gpu => 5,
            HardwareKind::Storage => 6,
            HardwareKind::Network => 7,
            HardwareKind::Battery => 8,
            HardwareKind::PowerSupply => 9,
            HardwareKind::Cooler => 10,
            HardwareKind::Other => 11,
        }
    }
}

/// A node of This PC's sensor tree (CPU, a GPU, a drive, a Super I/O chip…).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hardware {
    /// Unique within the device, `[a-z0-9_]`.
    pub id: String,
    pub name: String,
    pub kind: HardwareKind,
    /// Parent node id (a Super I/O chip under the motherboard…).
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub sensors: Vec<Sensor>,
    pub controls: Vec<Control>,
}

impl Snapshot {
    pub fn sensor(&self, id: &str) -> Option<f64> {
        self.sensors.iter().find(|s| s.id == id).map(|s| s.value)
    }
}
