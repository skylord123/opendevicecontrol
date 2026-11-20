//! Known devices (from Solaar's `descriptors.py`, GPL-2.0-or-later): names
//! for devices that report only a codename, kinds, and which HID++ 1.0
//! registers the older models have. HID++ 2.0 devices describe themselves,
//! so most need no entry.

use super::settings::reg::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Mouse,
    Keyboard,
    Numpad,
    Touchpad,
    Trackball,
    Headset,
    Presenter,
    Remote,
    Gamepad,
    Other,
}

pub struct Descriptor {
    pub name: &'static str,
    pub codename: &'static str,
    pub kind: Kind,
    /// Wireless product ids (as paired to a receiver).
    pub wpids: &'static [u16],
    pub usbid: u16,
    pub btid: u16,
    pub hidpp10: bool,
    /// HID++ 1.0 settings and battery registers (`settings::reg`).
    pub registers: u32,
}

pub fn by_wpid(wpid: u16) -> Option<&'static Descriptor> {
    DESCRIPTORS.iter().find(|d| d.wpids.contains(&wpid))
}

pub fn by_product(pid: u16) -> Option<&'static Descriptor> {
    DESCRIPTORS.iter().find(|d| d.usbid == pid || d.btid == pid)
}

/// Kind from the HID++ device kind number (receiver pairing info and
/// DEVICE_NAME use different numbers; `hidpp20` picks the table).
pub fn kind_from(n: u8, hidpp20: bool) -> Kind {
    if hidpp20 {
        match n {
            0 => Kind::Keyboard,
            1 => Kind::Remote,
            2 => Kind::Numpad,
            3 => Kind::Mouse,
            4 => Kind::Touchpad,
            5 => Kind::Trackball,
            6 => Kind::Presenter,
            8 => Kind::Headset,
            _ => Kind::Other,
        }
    } else {
        match n {
            1 => Kind::Keyboard,
            2 => Kind::Mouse,
            3 => Kind::Numpad,
            4 => Kind::Presenter,
            7 | 0x0E => Kind::Remote,
            8 => Kind::Trackball,
            9 => Kind::Touchpad,
            0x0B | 0x0C => Kind::Gamepad,
            0x0D => Kind::Headset,
            _ => Kind::Other,
        }
    }
}

pub const DESCRIPTORS: &[Descriptor] = &[
    Descriptor { name: "Wireless Keyboard EX110", codename: "EX110", kind: Kind::Keyboard, wpids: &[0x0055], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "Wireless Keyboard S510", codename: "S510", kind: Kind::Keyboard, wpids: &[0x0056], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "Wireless Wave Keyboard K550", codename: "K550", kind: Kind::Keyboard, wpids: &[0x0060], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: FN_SWAP | BATTERY_STATUS },
    Descriptor { name: "Wireless Keyboard EX100", codename: "EX100", kind: Kind::Keyboard, wpids: &[0x0065], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "Wireless Keyboard MK300", codename: "MK300", kind: Kind::Keyboard, wpids: &[0x0068], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "Number Pad N545", codename: "N545", kind: Kind::Numpad, wpids: &[0x2006], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "Wireless Compact Keyboard K340", codename: "K340", kind: Kind::Keyboard, wpids: &[0x2007], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "Wireless Keyboard MK700", codename: "MK700", kind: Kind::Keyboard, wpids: &[0x2008], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: FN_SWAP | BATTERY_STATUS },
    Descriptor { name: "Wireless Wave Keyboard K350", codename: "K350", kind: Kind::Keyboard, wpids: &[0x200a], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "Wireless Keyboard MK320", codename: "MK320", kind: Kind::Keyboard, wpids: &[0x200f], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "Wireless Illuminated Keyboard K800", codename: "K800", kind: Kind::Keyboard, wpids: &[0x2010], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: FN_SWAP | HAND_DETECTION | BATTERY_STATUS },
    Descriptor { name: "Wireless Keyboard K520", codename: "K520", kind: Kind::Keyboard, wpids: &[0x2011], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: FN_SWAP | BATTERY_STATUS },
    Descriptor { name: "Wireless Solar Keyboard K750", codename: "K750", kind: Kind::Keyboard, wpids: &[0x4002], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Keyboard K270 (unifying)", codename: "K270", kind: Kind::Keyboard, wpids: &[0x4003], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Keyboard K360", codename: "K360", kind: Kind::Keyboard, wpids: &[0x4004], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: DPI_PMX | SMOOTH_SCROLL | SIDE_SCROLL },
    Descriptor { name: "Wireless Keyboard K230", codename: "K230", kind: Kind::Keyboard, wpids: &[0x400d], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Touch Keyboard K400", codename: "K400", kind: Kind::Keyboard, wpids: &[0x400e, 0x4024], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Keyboard MK270", codename: "MK270", kind: Kind::Keyboard, wpids: &[0x4023], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Illuminated Living-Room Keyboard K830", codename: "K830", kind: Kind::Keyboard, wpids: &[0x4032], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Touch Keyboard K400 Plus", codename: "K400 Plus", kind: Kind::Keyboard, wpids: &[0x404d], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Multi-Device Keyboard K780", codename: "K780", kind: Kind::Keyboard, wpids: &[0x405b], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Keyboard K375s", codename: "K375s", kind: Kind::Keyboard, wpids: &[0x4061], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Craft Advanced Keyboard", codename: "Craft", kind: Kind::Keyboard, wpids: &[0x4066], usbid: 0x0000, btid: 0xb350, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Illuminated Keyboard K800 new", codename: "K800 new", kind: Kind::Keyboard, wpids: &[0x406e], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Keyboard K470", codename: "K470", kind: Kind::Keyboard, wpids: &[0x4075], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "MX Keys Keyboard", codename: "MX Keys", kind: Kind::Keyboard, wpids: &[0x408a], usbid: 0x0000, btid: 0xb35b, hidpp10: false, registers: 0 },
    Descriptor { name: "G915 TKL LIGHTSPEED Wireless RGB Mechanical Gaming Keyboard", codename: "G915 TKL", kind: Kind::Keyboard, wpids: &[0x408e], usbid: 0xc343, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Illuminated Keyboard", codename: "Illuminated", kind: Kind::Keyboard, wpids: &[], usbid: 0xc318, btid: 0x0000, hidpp10: true, registers: FN_SWAP },
    Descriptor { name: "G213 Prodigy Gaming Keyboard", codename: "G213", kind: Kind::Keyboard, wpids: &[], usbid: 0xc336, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G512 RGB Mechanical Gaming Keyboard", codename: "G512", kind: Kind::Keyboard, wpids: &[], usbid: 0xc33c, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G815 Mechanical Keyboard", codename: "G815", kind: Kind::Keyboard, wpids: &[], usbid: 0xc33f, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "diNovo Edge Keyboard", codename: "diNovo", kind: Kind::Keyboard, wpids: &[0xc714], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: FN_SWAP },
    Descriptor { name: "K845 Mechanical Keyboard", codename: "K845", kind: Kind::Keyboard, wpids: &[], usbid: 0xc341, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "LX5 Cordless Mouse", codename: "LX5", kind: Kind::Mouse, wpids: &[0x0036], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "LX7 Cordless Laser Mouse", codename: "LX7", kind: Kind::Mouse, wpids: &[0x0039], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "Wireless Wave Mouse M550", codename: "M550", kind: Kind::Mouse, wpids: &[0x003c], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "Wireless Mouse EX100", codename: "EX100m", kind: Kind::Mouse, wpids: &[0x003f], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "Wireless Mouse M30", codename: "M30", kind: Kind::Mouse, wpids: &[0x0085], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "MX610 Laser Cordless Mouse", codename: "MX610", kind: Kind::Mouse, wpids: &[0x1001], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "G7 Cordless Laser Mouse", codename: "G7", kind: Kind::Mouse, wpids: &[0x1002], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "V400 Laser Cordless Mouse", codename: "V400", kind: Kind::Mouse, wpids: &[0x1003], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "MX610 Left-Handled Mouse", codename: "MX610L", kind: Kind::Mouse, wpids: &[0x1004], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "V450 Laser Cordless Mouse", codename: "V450", kind: Kind::Mouse, wpids: &[0x1005], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "VX Revolution", codename: "VX Revolution", kind: Kind::Mouse, wpids: &[0x1006, 0x100d, 0x0612], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_CHARGE },
    Descriptor { name: "MX Air", codename: "MX Air", kind: Kind::Mouse, wpids: &[0x1007, 0x100e], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_CHARGE },
    Descriptor { name: "MX Revolution", codename: "MX Revolution", kind: Kind::Mouse, wpids: &[0x1008, 0x100c], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_CHARGE },
    Descriptor { name: "MX620 Laser Cordless Mouse", codename: "MX620", kind: Kind::Mouse, wpids: &[0x100a, 0x1016], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_CHARGE },
    Descriptor { name: "VX Nano Cordless Laser Mouse", codename: "VX Nano", kind: Kind::Mouse, wpids: &[0x100b, 0x100f], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: SMOOTH_SCROLL | SIDE_SCROLL | BATTERY_CHARGE },
    Descriptor { name: "V450 Nano Cordless Laser Mouse", codename: "V450 Nano", kind: Kind::Mouse, wpids: &[0x1011], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_CHARGE },
    Descriptor { name: "V550 Nano Cordless Laser Mouse", codename: "V550 Nano", kind: Kind::Mouse, wpids: &[0x1013], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: SMOOTH_SCROLL | SIDE_SCROLL | BATTERY_CHARGE },
    Descriptor { name: "MX 1100 Cordless Laser Mouse", codename: "MX 1100", kind: Kind::Mouse, wpids: &[0x1014], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: SMOOTH_SCROLL | SIDE_SCROLL | BATTERY_CHARGE },
    Descriptor { name: "Anywhere Mouse MX", codename: "Anywhere MX", kind: Kind::Mouse, wpids: &[0x1017], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: SMOOTH_SCROLL | SIDE_SCROLL | BATTERY_CHARGE },
    Descriptor { name: "Performance Mouse MX", codename: "Performance MX", kind: Kind::Mouse, wpids: &[0x101a], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: DPI_PMX | SMOOTH_SCROLL | SIDE_SCROLL | BATTERY_STATUS },
    Descriptor { name: "Marathon Mouse M705 (M-R0009)", codename: "M705 (M-R0009)", kind: Kind::Mouse, wpids: &[0x101b], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: SMOOTH_SCROLL | SIDE_SCROLL | BATTERY_CHARGE },
    Descriptor { name: "Wireless Mouse M350", codename: "M350", kind: Kind::Mouse, wpids: &[0x101c], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_CHARGE },
    Descriptor { name: "Wireless Mouse M505", codename: "M505/B605", kind: Kind::Mouse, wpids: &[0x101d], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: SMOOTH_SCROLL | SIDE_SCROLL | BATTERY_CHARGE },
    Descriptor { name: "Wireless Mouse M305", codename: "M305", kind: Kind::Mouse, wpids: &[0x101f], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: SIDE_SCROLL | BATTERY_STATUS },
    Descriptor { name: "Wireless Mouse M215", codename: "M215", kind: Kind::Mouse, wpids: &[0x1020], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: 0 },
    Descriptor { name: "G700 Gaming Mouse", codename: "G700", kind: Kind::Mouse, wpids: &[0x1023], usbid: 0xc06b, btid: 0x0000, hidpp10: true, registers: SMOOTH_SCROLL | SIDE_SCROLL | BATTERY_STATUS },
    Descriptor { name: "Wireless Mouse M310", codename: "M310", kind: Kind::Mouse, wpids: &[0x1024], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: BATTERY_STATUS },
    Descriptor { name: "Wireless Mouse M510", codename: "M510", kind: Kind::Mouse, wpids: &[0x1025], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: SIDE_SCROLL | BATTERY_STATUS },
    Descriptor { name: "Fujitsu Sonic Mouse", codename: "Sonic", kind: Kind::Mouse, wpids: &[0x1029], usbid: 0x0000, btid: 0x0000, hidpp10: true, registers: 0 },
    Descriptor { name: "G700s Gaming Mouse", codename: "G700s", kind: Kind::Mouse, wpids: &[0x102a], usbid: 0xc07c, btid: 0x0000, hidpp10: true, registers: SMOOTH_SCROLL | SIDE_SCROLL | BATTERY_STATUS },
    Descriptor { name: "Couch Mouse M515", codename: "M515", kind: Kind::Mouse, wpids: &[0x4007], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Mouse M325", codename: "M325", kind: Kind::Mouse, wpids: &[0x400a], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Mouse M525", codename: "M525", kind: Kind::Mouse, wpids: &[0x4013], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Mouse M345", codename: "M345", kind: Kind::Mouse, wpids: &[0x4017], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Mouse M187", codename: "M187", kind: Kind::Mouse, wpids: &[0x4019], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Touch Mouse M600", codename: "M600", kind: Kind::Mouse, wpids: &[0x401a], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Mouse M150", codename: "M150", kind: Kind::Mouse, wpids: &[0x4022], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Mouse M185", codename: "M185", kind: Kind::Mouse, wpids: &[0x4038], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Mouse MX Master", codename: "MX Master", kind: Kind::Mouse, wpids: &[0x4041], usbid: 0x0000, btid: 0xb012, hidpp10: false, registers: 0 },
    Descriptor { name: "Anywhere Mouse MX 2", codename: "Anywhere MX 2", kind: Kind::Mouse, wpids: &[0x404a], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Mouse M510", codename: "M510v2", kind: Kind::Mouse, wpids: &[0x4051], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Mouse M185 new", codename: "M185n", kind: Kind::Mouse, wpids: &[0x4054], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Mouse M185/M235/M310", codename: "M185/M235/M310", kind: Kind::Mouse, wpids: &[0x4055], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Mouse MX Master 2S", codename: "MX Master 2S", kind: Kind::Mouse, wpids: &[0x4069], usbid: 0x0000, btid: 0xb019, hidpp10: false, registers: 0 },
    Descriptor { name: "Multi Device Silent Mouse M585/M590", codename: "M585/M590", kind: Kind::Mouse, wpids: &[0x406b], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Marathon Mouse M705 (M-R0073)", codename: "M705 (M-R0073)", kind: Kind::Mouse, wpids: &[0x406d], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "MX Vertical Wireless Mouse", codename: "MX Vertical", kind: Kind::Mouse, wpids: &[0x407b], usbid: 0xc08a, btid: 0xb020, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Mouse Pebble M350", codename: "Pebble", kind: Kind::Mouse, wpids: &[0x4080], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "MX Master 3 Wireless Mouse", codename: "MX Master 3", kind: Kind::Mouse, wpids: &[0x4082], usbid: 0x0000, btid: 0xb023, hidpp10: false, registers: 0 },
    Descriptor { name: "PRO X Wireless", codename: "PRO X", kind: Kind::Mouse, wpids: &[0x4093], usbid: 0xc094, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G9 Laser Mouse", codename: "G9", kind: Kind::Mouse, wpids: &[], usbid: 0xc048, btid: 0x0000, hidpp10: true, registers: DPI_PMX | SMOOTH_SCROLL | SIDE_SCROLL },
    Descriptor { name: "G9x Laser Mouse", codename: "G9x", kind: Kind::Mouse, wpids: &[], usbid: 0xc066, btid: 0x0000, hidpp10: true, registers: DPI_PMX | SMOOTH_SCROLL | SIDE_SCROLL },
    Descriptor { name: "G502 Gaming Mouse", codename: "G502", kind: Kind::Mouse, wpids: &[], usbid: 0xc07d, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G402 Gaming Mouse", codename: "G402", kind: Kind::Mouse, wpids: &[], usbid: 0xc07e, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G900 Chaos Spectrum Gaming Mouse", codename: "G900", kind: Kind::Mouse, wpids: &[], usbid: 0xc081, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G403 Gaming Mouse", codename: "G403", kind: Kind::Mouse, wpids: &[], usbid: 0xc082, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G903 Lightspeed Gaming Mouse", codename: "G903", kind: Kind::Mouse, wpids: &[], usbid: 0xc086, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G703 Lightspeed Gaming Mouse", codename: "G703", kind: Kind::Mouse, wpids: &[], usbid: 0xc087, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "GPro Gaming Mouse", codename: "GPro", kind: Kind::Mouse, wpids: &[], usbid: 0xc088, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G502 SE Hero Gaming Mouse", codename: "G502 Hero", kind: Kind::Mouse, wpids: &[], usbid: 0xc08b, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G502 Lightspeed Gaming Mouse", codename: "G502 Lightspeed", kind: Kind::Mouse, wpids: &[], usbid: 0xc08d, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "MX518 Gaming Mouse", codename: "MX518", kind: Kind::Mouse, wpids: &[], usbid: 0xc08e, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G703 Hero Gaming Mouse", codename: "G703 Hero", kind: Kind::Mouse, wpids: &[], usbid: 0xc090, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G903 Hero Gaming Mouse", codename: "G903 Hero", kind: Kind::Mouse, wpids: &[], usbid: 0xc091, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "M500S Mouse", codename: "M500S", kind: Kind::Mouse, wpids: &[], usbid: 0xc093, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G500s Gaming Mouse", codename: "G500s Gaming", kind: Kind::Mouse, wpids: &[], usbid: 0xc24e, btid: 0x0000, hidpp10: true, registers: 0 },
    Descriptor { name: "G502 Proteus Spectrum Optical Mouse", codename: "G502 Proteus Spectrum", kind: Kind::Mouse, wpids: &[], usbid: 0xc332, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Logitech PRO Gaming Keyboard", codename: "PRO Gaming Keyboard", kind: Kind::Keyboard, wpids: &[], usbid: 0xc339, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Logitech MX Revolution Mouse M-RCL 124", codename: "M-RCL 124", kind: Kind::Mouse, wpids: &[], usbid: 0x0000, btid: 0xb007, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Trackball M570", codename: "M570", kind: Kind::Trackball, wpids: &[], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Touchpad", codename: "Wireless Touch", kind: Kind::Touchpad, wpids: &[0x4011], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "Wireless Rechargeable Touchpad T650", codename: "T650", kind: Kind::Touchpad, wpids: &[0x4101], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G Powerplay", codename: "Powerplay", kind: Kind::Touchpad, wpids: &[0x405f], usbid: 0x0000, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G533 Gaming Headset", codename: "G533 Headset", kind: Kind::Headset, wpids: &[], usbid: 0x0a66, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G535 Gaming Headset", codename: "G535 Headset", kind: Kind::Headset, wpids: &[], usbid: 0x0ac4, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G935 Gaming Headset", codename: "G935 Headset", kind: Kind::Headset, wpids: &[], usbid: 0x0a87, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G733 Gaming Headset", codename: "G733 Headset", kind: Kind::Headset, wpids: &[], usbid: 0x0ab5, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "G733 Gaming Headset", codename: "G733 Headset New", kind: Kind::Headset, wpids: &[], usbid: 0x0afe, btid: 0x0000, hidpp10: false, registers: 0 },
    Descriptor { name: "PRO X Wireless Gaming Headset", codename: "PRO Headset", kind: Kind::Headset, wpids: &[], usbid: 0x0aba, btid: 0x0000, hidpp10: false, registers: 0 },
];
