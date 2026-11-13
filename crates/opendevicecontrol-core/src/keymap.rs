//! Device-side key remapping: what each key does in each layer, and macros
//! stored in the device. Generic over devices; a driver says what it
//! supports with [`KeymapInfo`].
//!
//! Keys are addressed by their position in the device's lighting zone
//! order, the same index as [`crate::model::LedShape::led`], so front-ends
//! can draw the keyboard and edit it in place.

use serde::{Deserialize, Serialize};

/// HID keyboard modifier bits (the order of a boot keyboard report).
pub mod modifier {
    pub const LCTRL: u8 = 0x01;
    pub const LSHIFT: u8 = 0x02;
    pub const LALT: u8 = 0x04;
    pub const LGUI: u8 = 0x08;
    pub const RCTRL: u8 = 0x10;
    pub const RSHIFT: u8 = 0x20;
    pub const RALT: u8 = 0x40;
    pub const RGUI: u8 = 0x80;
}

/// A mouse action bound to a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseAction {
    /// Button 1–5 (left, right, middle, back, forward).
    Button(u8),
    /// Wheel up (+) / down (−).
    Wheel(i8),
    /// Horizontal wheel right (+) / left (−).
    Pan(i8),
    /// Pointer movement right (+) / left (−).
    MoveX(i8),
    /// Pointer movement down (+) / up (−).
    MoveY(i8),
}

/// How a key plays its macro.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MacroMode {
    /// Play `n` times per press.
    Times(u8),
    /// Repeat while the key is held.
    WhileHeld,
    /// Start on one press, stop on the next.
    Toggle,
}

/// What a key does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyBinding {
    /// Nothing (or, in an overlay layer, the key below shows through).
    None,
    /// A keyboard key (HID usage, 0 = modifiers only) with modifiers held.
    Key { modifiers: u8, usage: u8 },
    /// A consumer-control usage (media, volume, browser keys…).
    Media(u16),
    Mouse(MouseAction),
    /// Macro stored in the device.
    Macro { slot: u8, mode: MacroMode },
    /// Reported to OpenDeviceControl (no key is typed), so it can run OpenDeviceControl actions
    /// and Home Assistant events.
    Host(u16),
    /// A device function from [`KeymapInfo::functions`].
    Function(u32),
    /// Switch to layer `n` while held.
    Layer(u8),
    /// A device action OpenDeviceControl doesn't understand, kept as is.
    Raw([u8; 4]),
}

/// One editable layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerInfo {
    pub id: u8,
    pub name: String,
    pub help: String,
}

/// A device function that keys can be bound to (lighting controls,
/// Bluetooth slots, Win-key lock…).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionInfo {
    pub id: u32,
    pub name: String,
    /// A few characters for drawing on a key (e.g. "Bri+").
    #[serde(default)]
    pub short: String,
    /// Group shown in the picker (e.g. "Lighting", "Connection").
    pub group: String,
}

/// A key whose action the device firmware forces, whatever its layer says
/// (e.g. Fn+F1 in Windows mode).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixedKey {
    pub layer: u8,
    /// Zone index of the key.
    pub key: usize,
    /// What it actually does.
    pub does: KeyBinding,
    /// Why, for the user.
    pub reason: String,
}

/// What a device's key remapping supports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct KeymapInfo {
    pub layers: Vec<LayerInfo>,
    pub functions: Vec<FunctionInfo>,
    /// Number of macro slots (0 = no macros).
    pub macro_slots: u8,
    /// Most steps one macro can hold.
    pub macro_max_steps: usize,
    /// Whether `Host` bindings reach OpenDeviceControl on the current connection.
    pub host_keys: bool,
    /// Keys the firmware overrides in the current mode.
    pub fixed: Vec<FixedKey>,
}

/// One macro step: something pressed or released, then a pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroStep {
    pub action: MacroAction,
    /// Pause after this step, in milliseconds.
    pub delay_ms: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MacroAction {
    KeyDown(u8),
    KeyUp(u8),
    /// Modifier bit (see [`modifier`]) pressed / released.
    ModifierDown(u8),
    ModifierUp(u8),
    /// Mouse button bits pressed / released.
    MouseDown(u8),
    MouseUp(u8),
    MoveX(i8),
    MoveY(i8),
    Wheel(i8),
    Pan(i8),
}

/// A macro stored in the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Macro {
    pub name: String,
    pub steps: Vec<MacroStep>,
}

/// HID usage names for keyboard keys (page 7), for front-ends.
pub fn usage_name(usage: u8) -> Option<&'static str> {
    Some(match usage {
        0x04..=0x1d => ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z"]
            [(usage - 0x04) as usize],
        0x1e..=0x27 => ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"][(usage - 0x1e) as usize],
        0x28 => "Enter",
        0x29 => "Esc",
        0x2a => "Backspace",
        0x2b => "Tab",
        0x2c => "Space",
        0x2d => "-",
        0x2e => "=",
        0x2f => "[",
        0x30 => "]",
        0x31 => "\\",
        0x33 => ";",
        0x34 => "'",
        0x35 => "`",
        0x36 => ",",
        0x37 => ".",
        0x38 => "/",
        0x39 => "Caps Lock",
        0x3a..=0x45 => ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"][(usage - 0x3a) as usize],
        0x46 => "Print Screen",
        0x47 => "Scroll Lock",
        0x48 => "Pause",
        0x49 => "Insert",
        0x4a => "Home",
        0x4b => "Page Up",
        0x4c => "Delete",
        0x4d => "End",
        0x4e => "Page Down",
        0x4f => "Right",
        0x50 => "Left",
        0x51 => "Down",
        0x52 => "Up",
        0x53 => "Num Lock",
        0x54 => "Num /",
        0x55 => "Num *",
        0x56 => "Num -",
        0x57 => "Num +",
        0x58 => "Num Enter",
        0x59..=0x62 => ["Num 1", "Num 2", "Num 3", "Num 4", "Num 5", "Num 6", "Num 7", "Num 8", "Num 9", "Num 0"][(usage - 0x59) as usize],
        0x63 => "Num .",
        0x64 => "ISO \\",
        0x65 => "Menu",
        0x68..=0x73 => ["F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24"][(usage - 0x68) as usize],
        0xe0 => "Left Ctrl",
        0xe1 => "Left Shift",
        0xe2 => "Left Alt",
        0xe3 => "Left Win",
        0xe4 => "Right Ctrl",
        0xe5 => "Right Shift",
        0xe6 => "Right Alt",
        0xe7 => "Right Win",
        _ => return None,
    })
}

/// Common consumer-control usages (media keys), for front-ends.
pub const MEDIA_USAGES: &[(u16, &str)] = &[
    (0x00cd, "Play / Pause"),
    (0x00b5, "Next track"),
    (0x00b6, "Previous track"),
    (0x00b7, "Stop"),
    (0x00e2, "Mute"),
    (0x00e9, "Volume up"),
    (0x00ea, "Volume down"),
    (0x006f, "Brightness up"),
    (0x0070, "Brightness down"),
    (0x0183, "Media player"),
    (0x018a, "Mail"),
    (0x0192, "Calculator"),
    (0x0194, "My computer"),
    (0x0221, "Search"),
    (0x0223, "Browser home"),
    (0x0224, "Browser back"),
    (0x0225, "Browser forward"),
    (0x0227, "Browser refresh"),
    (0x022a, "Bookmarks"),
];

/// A short form of a consumer usage for drawing on a key.
pub fn media_short(u: u16) -> &'static str {
    match u {
        0x00cd => "Play",
        0x00b5 => "Next",
        0x00b6 => "Prev",
        0x00b7 => "Stop",
        0x00e2 => "Mute",
        0x00e9 => "Vol+",
        0x00ea => "Vol−",
        0x006f => "Bri+",
        0x0070 => "Bri−",
        0x0183 => "Music",
        0x018a => "Mail",
        0x0192 => "Calc",
        0x0194 => "PC",
        0x0221 => "Find",
        0x0223 => "Web",
        0x0224 => "Back",
        0x0225 => "Fwd",
        0x0227 => "Rfsh",
        0x022a => "Bkmk",
        _ => "Media",
    }
}

/// A short key name for drawing on a key ("Backspace" → "Bksp").
pub fn usage_short(usage: u8) -> String {
    let n = usage_name(usage).unwrap_or("?");
    match n {
        "Backspace" => "Bksp".into(),
        "Caps Lock" => "Caps".into(),
        "Print Screen" => "PrtSc".into(),
        "Scroll Lock" => "ScrLk".into(),
        "Page Up" => "PgUp".into(),
        "Page Down" => "PgDn".into(),
        "Delete" => "Del".into(),
        "Insert" => "Ins".into(),
        "Num Lock" => "Num".into(),
        "Num Enter" => "Ent".into(),
        "Enter" => "Ent".into(),
        "Space" => "Spc".into(),
        "Right" => "→".into(),
        "Left" => "←".into(),
        "Up" => "↑".into(),
        "Down" => "↓".into(),
        "Left Ctrl" | "Right Ctrl" => "Ctrl".into(),
        "Left Shift" | "Right Shift" => "Shift".into(),
        "Left Alt" | "Right Alt" => "Alt".into(),
        "Left Win" | "Right Win" => "Win".into(),
        n if n.starts_with("Num ") => format!("N{}", &n[4..]),
        n => n.into(),
    }
}

/// Modifier names, in bit order.
pub const MODIFIER_NAMES: [(u8, &str); 8] = [
    (modifier::LCTRL, "Ctrl"),
    (modifier::LSHIFT, "Shift"),
    (modifier::LALT, "Alt"),
    (modifier::LGUI, "Win"),
    (modifier::RCTRL, "Right Ctrl"),
    (modifier::RSHIFT, "Right Shift"),
    (modifier::RALT, "Right Alt"),
    (modifier::RGUI, "Right Win"),
];

impl KeyBinding {
    /// Labels for drawing on a key, longest first; front-ends use the first
    /// that fits.
    pub fn key_labels(&self, info: &KeymapInfo) -> Vec<String> {
        let mods = |m: u8, long: bool| -> String {
            MODIFIER_NAMES
                .iter()
                .filter(|(b, _)| m & b != 0)
                .map(|(_, n)| if long { (*n).to_string() } else { n.trim_start_matches("Right ").chars().next().unwrap_or('?').to_string() })
                .collect::<Vec<_>>()
                .join(if long { "+" } else { "" })
        };
        match self {
            KeyBinding::None => vec!["–".into()],
            KeyBinding::Key { modifiers, usage } => {
                let key = usage_short(*usage);
                match (*modifiers, *usage) {
                    (0, 0) => vec!["–".into()],
                    (0, _) => vec![key],
                    (m, 0) => vec![mods(m, true), mods(m, false)],
                    (m, _) => vec![format!("{}+{key}", mods(m, true)), format!("{}+{key}", mods(m, false)), format!("+{key}"), key],
                }
            }
            KeyBinding::Media(u) => vec![self.describe(info), media_short(*u).into()],
            KeyBinding::Mouse(m) => {
                let s = match m {
                    MouseAction::Button(1) => "LMB",
                    MouseAction::Button(2) => "RMB",
                    MouseAction::Button(3) => "MMB",
                    MouseAction::Button(4) => "M4",
                    MouseAction::Button(_) => "M5",
                    MouseAction::Wheel(v) => if *v > 0 { "Whl↑" } else { "Whl↓" },
                    MouseAction::Pan(v) => if *v > 0 { "Whl→" } else { "Whl←" },
                    MouseAction::MoveX(v) => if *v > 0 { "Ptr→" } else { "Ptr←" },
                    MouseAction::MoveY(v) => if *v > 0 { "Ptr↓" } else { "Ptr↑" },
                };
                vec![s.into()]
            }
            KeyBinding::Macro { slot, .. } => vec![format!("Macro {}", slot + 1), format!("M{}", slot + 1)],
            KeyBinding::Host(n) => vec![format!("ODC {n}"), format!("ODC{n}"), format!("#{n}")],
            KeyBinding::Function(id) => match info.functions.iter().find(|f| f.id == *id) {
                Some(f) if !f.short.is_empty() => vec![f.short.clone()],
                Some(f) => vec![f.name.clone(), "ƒ".into()],
                None => vec!["ƒ".into()],
            },
            KeyBinding::Layer(l) => {
                let name = info.layers.iter().find(|x| x.id == *l).map(|x| x.name.clone()).unwrap_or_else(|| "Fn".into());
                vec![name, "Fn".into()]
            }
            KeyBinding::Raw(_) => vec!["?".into()],
        }
    }

    /// Short description, e.g. "Ctrl+C", "Volume up", "Macro 2".
    pub fn describe(&self, info: &KeymapInfo) -> String {
        match self {
            KeyBinding::None => "Nothing".into(),
            KeyBinding::Key { modifiers, usage } => {
                let mut parts: Vec<String> =
                    MODIFIER_NAMES.iter().filter(|(b, _)| modifiers & b != 0).map(|(_, n)| (*n).to_string()).collect();
                if *usage != 0 {
                    parts.push(usage_name(*usage).map(str::to_string).unwrap_or_else(|| format!("Key 0x{usage:02X}")));
                }
                if parts.is_empty() { "Nothing".into() } else { parts.join("+") }
            }
            KeyBinding::Media(u) => {
                MEDIA_USAGES.iter().find(|(c, _)| c == u).map(|(_, n)| (*n).to_string()).unwrap_or_else(|| format!("Media 0x{u:04X}"))
            }
            KeyBinding::Mouse(m) => match m {
                MouseAction::Button(b) => {
                    ["Left click", "Right click", "Middle click", "Back button", "Forward button"].get(*b as usize - 1).copied().unwrap_or("Mouse button").into()
                }
                MouseAction::Wheel(v) => (if *v > 0 { "Scroll up" } else { "Scroll down" }).into(),
                MouseAction::Pan(v) => (if *v > 0 { "Scroll right" } else { "Scroll left" }).into(),
                MouseAction::MoveX(v) => (if *v > 0 { "Move pointer right" } else { "Move pointer left" }).into(),
                MouseAction::MoveY(v) => (if *v > 0 { "Move pointer down" } else { "Move pointer up" }).into(),
            },
            KeyBinding::Macro { slot, .. } => format!("Macro {}", slot + 1),
            KeyBinding::Host(n) => format!("OpenDeviceControl key {n}"),
            KeyBinding::Function(id) => {
                info.functions.iter().find(|f| f.id == *id).map(|f| f.name.clone()).unwrap_or_else(|| format!("Function {id}"))
            }
            KeyBinding::Layer(l) => {
                info.layers.iter().find(|x| x.id == *l).map(|x| format!("{} layer", x.name)).unwrap_or_else(|| format!("Layer {l}"))
            }
            KeyBinding::Raw(b) => format!("Device action {:02X} {:02X} {:02X} {:02X}", b[0], b[1], b[2], b[3]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_bindings() {
        let info = KeymapInfo::default();
        assert_eq!(KeyBinding::Key { modifiers: modifier::LCTRL, usage: 0x06 }.describe(&info), "Ctrl+C");
        assert_eq!(KeyBinding::Key { modifiers: 0, usage: 0x29 }.describe(&info), "Esc");
        assert_eq!(KeyBinding::Media(0xe9).describe(&info), "Volume up");
        assert_eq!(KeyBinding::Mouse(MouseAction::Button(2)).describe(&info), "Right click");
        assert_eq!(KeyBinding::Macro { slot: 0, mode: MacroMode::Times(1) }.describe(&info), "Macro 1");
    }

    #[test]
    fn key_labels_get_shorter() {
        let info = KeymapInfo::default();
        let l = KeyBinding::Key { modifiers: modifier::LCTRL | modifier::LSHIFT, usage: 0x29 }.key_labels(&info);
        assert_eq!(l, vec!["Ctrl+Shift+Esc", "CS+Esc", "+Esc", "Esc"]);
        assert_eq!(KeyBinding::Media(0xe9).key_labels(&info), vec!["Volume up", "Vol+"]);
        assert_eq!(KeyBinding::Key { modifiers: 0, usage: 0x2a }.key_labels(&info), vec!["Bksp"]);
        for l in [KeyBinding::Host(3).key_labels(&info), KeyBinding::Macro { slot: 9, mode: MacroMode::Toggle }.key_labels(&info)] {
            assert!(l.windows(2).all(|w| w[0].chars().count() >= w[1].chars().count()));
        }
    }

    #[test]
    fn usage_names() {
        assert_eq!(usage_name(0x04), Some("A"));
        assert_eq!(usage_name(0x27), Some("0"));
        assert_eq!(usage_name(0x45), Some("F12"));
        assert_eq!(usage_name(0x62), Some("Num 0"));
        assert_eq!(usage_name(0xe7), Some("Right Win"));
        assert_eq!(usage_name(0xff), None);
    }
}
