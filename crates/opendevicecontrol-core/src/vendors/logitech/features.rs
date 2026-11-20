//! HID++ 2.0 feature ids and the feature table of a device.

use super::hidpp::Channel;
use crate::{Error, Result};

pub const ROOT: u16 = 0x0000;
pub const FEATURE_SET: u16 = 0x0001;
pub const DEVICE_FW_VERSION: u16 = 0x0003;
pub const DEVICE_NAME: u16 = 0x0005;
pub const DEVICE_FRIENDLY_NAME: u16 = 0x0007;
pub const BATTERY_STATUS: u16 = 0x1000;
pub const BATTERY_VOLTAGE: u16 = 0x1001;
pub const UNIFIED_BATTERY: u16 = 0x1004;
pub const CHANGE_HOST: u16 = 0x1814;
pub const HOSTS_INFO: u16 = 0x1815;
pub const BACKLIGHT2: u16 = 0x1982;
pub const BACKLIGHT3: u16 = 0x1983;
pub const HAPTIC: u16 = 0x19B0;
pub const FORCE_SENSING_BUTTON: u16 = 0x19C0;
pub const REPROG_CONTROLS_V4: u16 = 0x1B04;
pub const PERSISTENT_REMAPPABLE_ACTION: u16 = 0x1C00;
pub const WIRELESS_DEVICE_STATUS: u16 = 0x1D4B;
pub const ADC_MEASUREMENT: u16 = 0x1F20;
pub const SMART_SHIFT: u16 = 0x2110;
pub const SMART_SHIFT_ENHANCED: u16 = 0x2111;
pub const HI_RES_SCROLLING: u16 = 0x2120;
pub const HIRES_WHEEL: u16 = 0x2121;
pub const LOWRES_WHEEL: u16 = 0x2130;
pub const THUMB_WHEEL: u16 = 0x2150;
pub const ADJUSTABLE_DPI: u16 = 0x2201;
pub const EXTENDED_ADJUSTABLE_DPI: u16 = 0x2202;
pub const POINTER_SPEED: u16 = 0x2205;
pub const FN_INVERSION: u16 = 0x40A0;
pub const NEW_FN_INVERSION: u16 = 0x40A2;
pub const K375S_FN_INVERSION: u16 = 0x40A3;
pub const KEYBOARD_DISABLE_KEYS: u16 = 0x4521;
pub const DUALPLATFORM: u16 = 0x4530;
pub const MULTIPLATFORM: u16 = 0x4531;
pub const CROWN: u16 = 0x4600;
pub const GESTURE_2: u16 = 0x6501;
pub const GKEY: u16 = 0x8010;
pub const MKEYS: u16 = 0x8020;
pub const MR: u16 = 0x8030;
pub const BRIGHTNESS_CONTROL: u16 = 0x8040;
pub const REPORT_RATE: u16 = 0x8060;
pub const EXTENDED_ADJUSTABLE_REPORT_RATE: u16 = 0x8061;
pub const COLOR_LED_EFFECTS: u16 = 0x8070;
pub const RGB_EFFECTS: u16 = 0x8071;
pub const PER_KEY_LIGHTING_V2: u16 = 0x8081;
pub const ONBOARD_PROFILES: u16 = 0x8100;
pub const SIDETONE: u16 = 0x8300;
pub const EQUALIZER: u16 = 0x8310;

/// Readable name of a feature id, for logs and the protocol notes.
pub fn name(id: u16) -> &'static str {
    match id {
        0x0000 => "ROOT",
        0x0001 => "FEATURE_SET",
        0x0002 => "FEATURE_INFO",
        0x0003 => "DEVICE_FW_VERSION",
        0x0004 => "DEVICE_UNIT_ID",
        0x0005 => "DEVICE_NAME",
        0x0006 => "DEVICE_GROUPS",
        0x0007 => "DEVICE_FRIENDLY_NAME",
        0x0008 => "KEEP_ALIVE",
        0x0020 => "CONFIG_CHANGE",
        0x0021 => "CRYPTO_ID",
        0x0080 => "WIRELESS_SIGNAL_STRENGTH",
        0x00C2 | 0x00C3 => "DFUCONTROL",
        0x00D0 => "DFU",
        0x1000 => "BATTERY_STATUS",
        0x1001 => "BATTERY_VOLTAGE",
        0x1004 => "UNIFIED_BATTERY",
        0x1010 => "CHARGING_CONTROL",
        0x1300 => "LED_CONTROL",
        0x1802 => "DEVICE_RESET",
        0x1805 => "OOBSTATE",
        0x1806 => "CONFIG_DEVICE_PROPS",
        0x1814 => "CHANGE_HOST",
        0x1815 => "HOSTS_INFO",
        0x1981 => "BACKLIGHT",
        0x1982 => "BACKLIGHT2",
        0x1983 => "BACKLIGHT3",
        0x1990 => "ILLUMINATION",
        0x19B0 => "HAPTIC",
        0x19C0 => "FORCE_SENSING_BUTTON",
        0x1B00 => "REPROG_CONTROLS",
        0x1B01 => "REPROG_CONTROLS_V2",
        0x1B04 => "REPROG_CONTROLS_V4",
        0x1BC0 => "REPORT_HID_USAGE",
        0x1C00 => "PERSISTENT_REMAPPABLE_ACTION",
        0x1D4B => "WIRELESS_DEVICE_STATUS",
        0x1DF0 => "REMAINING_PAIRING",
        0x1F20 => "ADC_MEASUREMENT",
        0x2001 => "LEFT_RIGHT_SWAP",
        0x2006 => "POINTER_AXIS_ORIENTATION",
        0x2100 => "VERTICAL_SCROLLING",
        0x2110 => "SMART_SHIFT",
        0x2111 => "SMART_SHIFT_ENHANCED",
        0x2120 => "HI_RES_SCROLLING",
        0x2121 => "HIRES_WHEEL",
        0x2130 => "LOWRES_WHEEL",
        0x2150 => "THUMB_WHEEL",
        0x2200 => "MOUSE_POINTER",
        0x2201 => "ADJUSTABLE_DPI",
        0x2202 => "EXTENDED_ADJUSTABLE_DPI",
        0x2205 => "POINTER_SPEED",
        0x2230 => "ANGLE_SNAPPING",
        0x2240 => "SURFACE_TUNING",
        0x2250 => "XY_STATS",
        0x2251 => "WHEEL_STATS",
        0x40A0 => "FN_INVERSION",
        0x40A2 => "NEW_FN_INVERSION",
        0x40A3 => "K375S_FN_INVERSION",
        0x4100 => "ENCRYPTION",
        0x4220 => "LOCK_KEY_STATE",
        0x4301 => "SOLAR_DASHBOARD",
        0x4520 => "KEYBOARD_LAYOUT",
        0x4521 => "KEYBOARD_DISABLE_KEYS",
        0x4522 => "KEYBOARD_DISABLE_BY_USAGE",
        0x4530 => "DUALPLATFORM",
        0x4531 => "MULTIPLATFORM",
        0x4540 => "KEYBOARD_LAYOUT_2",
        0x4600 => "CROWN",
        0x6010 => "TOUCHPAD_FW_ITEMS",
        0x6011 => "TOUCHPAD_SW_ITEMS",
        0x6012 => "TOUCHPAD_WIN8_FW_ITEMS",
        0x6020 => "TAP_ENABLE",
        0x6021 => "TAP_ENABLE_EXTENDED",
        0x6030 => "CURSOR_BALLISTIC",
        0x6040 => "TOUCHPAD_RESOLUTION",
        0x6100 => "TOUCHPAD_RAW_XY",
        0x6110 => "TOUCHMOUSE_RAW_POINTS",
        0x6500 => "GESTURE",
        0x6501 => "GESTURE_2",
        0x8010 => "GKEY",
        0x8020 => "MKEYS",
        0x8030 => "MR",
        0x8040 => "BRIGHTNESS_CONTROL",
        0x8060 => "REPORT_RATE",
        0x8061 => "EXTENDED_ADJUSTABLE_REPORT_RATE",
        0x8070 => "COLOR_LED_EFFECTS",
        0x8071 => "RGB_EFFECTS",
        0x8080 => "PER_KEY_LIGHTING",
        0x8081 => "PER_KEY_LIGHTING_V2",
        0x8090 => "MODE_STATUS",
        0x8100 => "ONBOARD_PROFILES",
        0x8110 => "MOUSE_BUTTON_SPY",
        0x8111 => "LATENCY_MONITORING",
        0x8120 => "GAMING_ATTACHMENTS",
        0x8123 => "FORCE_FEEDBACK",
        0x8300 => "SIDETONE",
        0x8310 => "EQUALIZER",
        0x8320 => "HEADSET_OUT",
        _ => "?",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feature {
    pub id: u16,
    pub index: u8,
    /// Obsolete 0x80, hidden 0x40, engineering 0x20.
    pub flags: u8,
    pub version: u8,
}

/// The features of a HID++ 2.0 device, read once per connection.
#[derive(Debug, Clone, Default)]
pub struct Features {
    list: Vec<Feature>,
}

impl Features {
    /// Read the whole table through FEATURE_SET.
    pub fn read(ch: &mut Channel) -> Result<Features> {
        let set = ch.feature(0, 0, &[(FEATURE_SET >> 8) as u8, FEATURE_SET as u8])?;
        let set_index = set.first().copied().unwrap_or(0);
        if set_index == 0 {
            return Err(Error::NotSupported(" (no feature table)"));
        }
        let count = ch.feature(set_index, 0, &[])?.first().copied().unwrap_or(0);
        let mut list = vec![Feature { id: ROOT, index: 0, flags: 0, version: 0 }];
        for i in 1..=count {
            let r = ch.feature(set_index, 1, &[i])?;
            if r.len() >= 4 {
                list.push(Feature { id: u16::from_be_bytes([r[0], r[1]]), index: i, flags: r[2], version: r[3] });
            }
        }
        Ok(Features { list })
    }

    pub fn get(&self, id: u16) -> Option<Feature> {
        self.list.iter().find(|f| f.id == id).copied()
    }

    pub fn has(&self, id: u16) -> bool {
        self.get(id).is_some()
    }

    pub fn version(&self, id: u16) -> u8 {
        self.get(id).map(|f| f.version).unwrap_or(0)
    }

    pub fn by_index(&self, index: u8) -> Option<Feature> {
        self.list.iter().find(|f| f.index == index).copied()
    }

    pub fn all(&self) -> &[Feature] {
        &self.list
    }
}
