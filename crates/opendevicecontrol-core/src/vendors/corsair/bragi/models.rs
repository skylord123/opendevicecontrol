//! Per-model data for Bragi devices (mice, headsets, receivers; keyboards
//! are in `keyboards.rs`).
//!
//! Sources: OpenLinkHub `src/devices/<model>/<model>.go` (LED order, button
//! bits, DPI ranges, polling rates), OpenRGB `CorsairPeripheralV2Devices.cpp`
//! (LED counts, packet sizes) and ckb-next `led_bragi.c` / `keymap.c`.

use crate::model::DeviceKind;

/// How colours are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lighting {
    None,
    /// Resource 0x01: planar RRR…GGG…BBB.
    Planar,
    /// Resource 0x22: `12 00` + RGB triplets (K100/K70 PRO class, newer
    /// headsets); falls back to [`Lighting::Planar`] if the device refuses it.
    Triplets,
}

/// What a mouse button does while opendevicecontrol drives the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Left the OS's normal button (key-assignment byte 1); not reported.
    Os,
    /// Reported to opendevicecontrol only (macro / Home Assistant key).
    Key,
    DpiUp,
    DpiDown,
    DpiCycle,
    /// Lower DPI while held.
    Sniper,
}

#[derive(Debug, Clone, Copy)]
pub struct Button {
    /// Bit in the software-mode button bitmap (`?? 02 <bitmap LE>`).
    pub bit: u8,
    pub id: &'static str,
    pub name: &'static str,
    pub role: Role,
}

const fn b(bit: u8, id: &'static str, name: &'static str, role: Role) -> Button {
    Button { bit, id, name, role }
}

#[derive(Debug, Clone, Copy)]
pub struct Dpi {
    pub min: u16,
    pub max: u16,
    /// Separate X/Y properties (0x21/0x22) instead of the single 0x20.
    pub xy: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Headset {
    /// Sidetone on/off (0x46, inverted) and level (0x47, 0..1000).
    pub sidetone: bool,
}

pub struct Model {
    pub pid: u16,
    pub name: &'static str,
    pub slug: &'static str,
    pub kind: DeviceKind,
    /// Packet size on a wired connection (behind a receiver the receiver's
    /// size is used).
    pub packet: usize,
    pub lighting: Lighting,
    /// Total LEDs in the colour buffer.
    pub led_count: usize,
    /// Named LEDs, in buffer order (the first `leds.len()` of `led_count`).
    /// Empty with `led_count > 0` means one "Keys" zone of all LEDs.
    pub leds: &'static [(&'static str, &'static str)],
    pub buttons: &'static [Button],
    pub dpi: Option<Dpi>,
    /// Highest polling-rate code (4 = 1000 Hz, 7 = 8000 Hz).
    pub max_poll: u8,
    pub wireless: bool,
    pub keyboard: Option<&'static super::keyboards::Keyboard>,
    pub headset: Option<Headset>,
}

impl Model {
    const fn mouse(pid: u16, name: &'static str, slug: &'static str) -> Self {
        Model {
            pid,
            name,
            slug,
            kind: DeviceKind::Mouse,
            packet: 64,
            lighting: Lighting::Planar,
            led_count: 0,
            leds: &[],
            buttons: &[],
            dpi: None,
            max_poll: 4,
            wireless: false,
            keyboard: None,
            headset: None,
        }
    }
    const fn leds(mut self, count: usize, leds: &'static [(&'static str, &'static str)]) -> Self {
        self.led_count = count;
        self.leds = leds;
        self
    }
    const fn buttons(mut self, buttons: &'static [Button]) -> Self {
        self.buttons = buttons;
        self
    }
    const fn dpi(mut self, min: u16, max: u16, xy: bool) -> Self {
        self.dpi = Some(Dpi { min, max, xy });
        self
    }
    const fn wireless(mut self) -> Self {
        self.wireless = true;
        self
    }
    const fn packet(mut self, size: usize) -> Self {
        self.packet = size;
        self
    }
    const fn poll(mut self, max: u8) -> Self {
        self.max_poll = max;
        self
    }
    const fn headset(pid: u16, name: &'static str, slug: &'static str, lighting: Lighting, leds: &'static [(&'static str, &'static str)], sidetone: bool) -> Self {
        Model {
            pid,
            name,
            slug,
            kind: DeviceKind::Headset,
            packet: 64,
            lighting,
            led_count: leds.len(),
            leds,
            buttons: &[],
            dpi: None,
            max_poll: 0,
            wireless: true,
            keyboard: None,
            headset: Some(Headset { sidetone }),
        }
    }
}

use Role::*;

const L: Button = b(0, "left", "Left button", Os);
const R: Button = b(1, "right", "Right button", Os);
const M: Button = b(2, "middle", "Middle button", Os);

// Button tables (OpenLinkHub `KeyAssignment` / ckb-next LUTs).
const BTN_FWD_BACK_DPI: &[Button] =
    &[L, R, M, b(3, "forward", "Forward", Os), b(4, "back", "Back", Os), b(5, "dpi", "DPI", DpiCycle)];
const BTN_BACK_FWD_DPI: &[Button] =
    &[L, R, M, b(3, "back", "Back", Os), b(4, "forward", "Forward", Os), b(5, "dpi", "DPI", DpiCycle)];
const BTN_M55_PRO: &[Button] = &[
    L,
    R,
    M,
    b(3, "back", "Left back", Os),
    b(4, "forward", "Left forward", Os),
    b(5, "right_back", "Right back", Key),
    b(6, "right_forward", "Right forward", Key),
    b(7, "dpi", "DPI", DpiCycle),
];
const BTN_DARK_CORE: &[Button] = &[
    L,
    R,
    M,
    b(3, "forward", "Forward", Os),
    b(4, "back", "Back", Os),
    b(5, "dpi_up", "DPI up", DpiUp),
    b(6, "dpi_down", "DPI down", DpiDown),
    b(7, "profile", "Profile", Key),
];
const BTN_HARPOON: &[Button] =
    &[L, R, M, b(3, "dpi", "DPI", DpiCycle), b(4, "back", "Back", Os), b(5, "forward", "Forward", Os)];
const BTN_IRONCLAW: &[Button] = &[
    L,
    R,
    M,
    b(3, "forward", "Forward", Os),
    b(4, "back", "Back", Os),
    b(5, "dpi_up", "DPI up", DpiUp),
    b(6, "dpi_down", "DPI down", DpiDown),
    b(7, "profile_up", "Profile up", Key),
    b(8, "profile_down", "Profile down", Key),
    b(9, "option", "Option", Key),
];
const BTN_SABRE_W: &[Button] = &[
    L,
    R,
    M,
    b(3, "forward", "Forward", Os),
    b(4, "back", "Back", Os),
    b(5, "dpi", "DPI", DpiCycle),
    b(6, "profile", "Profile", Key),
];
const BTN_M65_ULTRA: &[Button] = &[
    L,
    R,
    M,
    b(3, "back", "Back", Os),
    b(4, "forward", "Forward", Os),
    b(5, "dpi_up", "DPI up", DpiUp),
    b(6, "dpi_down", "DPI down", DpiDown),
    b(7, "sniper", "Sniper", Sniper),
];
const BTN_M65_ULTRA_W: &[Button] = &[
    L,
    R,
    M,
    b(3, "forward", "Forward", Os),
    b(4, "back", "Back", Os),
    b(5, "dpi_up", "DPI up", DpiUp),
    b(6, "dpi_down", "DPI down", DpiDown),
    b(7, "sniper", "Sniper", Sniper),
];
const BTN_NIGHTSABRE: &[Button] = &[
    L,
    R,
    M,
    b(3, "scroll_left", "Scroll left", Key),
    b(4, "scroll_right", "Scroll right", Key),
    b(5, "forward", "Forward", Os),
    b(6, "back", "Back", Os),
    b(7, "dpi_up", "DPI up", DpiUp),
    b(8, "dpi_down", "DPI down", DpiDown),
    b(9, "profile_up", "Profile up", Key),
    b(10, "profile_down", "Profile down", Key),
];
const BTN_SCIMITAR: &[Button] = &[
    L,
    R,
    M,
    b(3, "dpi", "DPI", DpiCycle),
    b(4, "profile", "Profile", Key),
    b(5, "g1", "Side 1", Key),
    b(6, "g2", "Side 2", Key),
    b(7, "g3", "Side 3", Key),
    b(8, "g4", "Side 4", Key),
    b(9, "g5", "Side 5", Key),
    b(10, "g6", "Side 6", Key),
    b(11, "g7", "Side 7", Key),
    b(12, "g8", "Side 8", Key),
    b(13, "g9", "Side 9", Key),
    b(14, "g10", "Side 10", Key),
    b(15, "g11", "Side 11", Key),
    b(16, "g12", "Side 12", Key),
];

const LED_DPI: &[(&str, &str)] = &[("dpi", "DPI indicator")];
const LED_LOGO: &[(&str, &str)] = &[("logo", "Logo")];
const LED_DPI_LOGO: &[(&str, &str)] = &[("dpi", "DPI indicator"), ("logo", "Logo")];
const LED_LOGO_DPI: &[(&str, &str)] = &[("logo", "Logo"), ("dpi", "DPI indicator")];
const LED_DARK_CORE: &[(&str, &str)] = &[
    ("scroll", "Scroll wheel"),
    ("side1", "Side 1"),
    ("side2", "Side 2"),
    ("side3", "Side 3"),
    ("side4", "Side 4"),
    ("side5", "Side 5"),
    ("logo", "Logo"),
    ("side6", "Side 6"),
    ("dpi1", "DPI 1"),
    ("dpi2", "DPI 2"),
    ("dpi3", "DPI 3"),
    ("indicator", "Indicator"),
];
const LED_IRONCLAW: &[(&str, &str)] = &[
    ("logo", "Logo"),
    ("scroll", "Scroll wheel"),
    ("front", "Front"),
    ("dpi1", "DPI 1"),
    ("dpi2", "DPI 2"),
    ("dpi3", "DPI 3"),
];
const LED_SABRE: &[(&str, &str)] =
    &[("logo", "Logo"), ("scroll", "Scroll wheel"), ("dpi1", "DPI 1"), ("dpi2", "DPI 2"), ("dpi3", "DPI 3")];
const LED_M65_ULTRA: &[(&str, &str)] = &[("logo", "Logo"), ("scroll", "Scroll wheel"), ("dpi", "DPI indicator")];
const LED_NIGHTSABRE: &[(&str, &str)] = &[
    ("front_left", "Front left"),
    ("front_right", "Front right"),
    ("palm_left1", "Left palm 1"),
    ("palm_left2", "Left palm 2"),
    ("palm_left3", "Left palm 3"),
    ("palm_right1", "Right palm 1"),
    ("palm_right2", "Right palm 2"),
    ("palm_right3", "Right palm 3"),
    ("logo", "Logo"),
    ("rear1", "Rear 1"),
    ("rear2", "Rear 2"),
    ("rear3", "Rear 3"),
    ("dpi1", "DPI 1"),
    ("dpi2", "DPI 2"),
    ("dpi3", "DPI 3"),
];
const LED_SCIMITAR: &[(&str, &str)] =
    &[("logo", "Logo"), ("scroll", "Scroll wheel"), ("front", "Front"), ("side", "Side panel"), ("dpi", "DPI indicator")];
const LED_SCIMITAR_W: &[(&str, &str)] = &[("logo", "Logo"), ("side", "Side panel"), ("dpi", "DPI indicator")];
const LED_HEADSET: &[(&str, &str)] = &[("logo", "Logo"), ("indicator", "Status indicator"), ("mic", "Microphone")];
const LED_HS80_MAX: &[(&str, &str)] = &[("logo", "Logo"), ("mic", "Microphone")];
const LED_VIRTUOSO_MAX: &[(&str, &str)] = &[
    ("left1", "Left 1"),
    ("left2", "Left 2"),
    ("left3", "Left 3"),
    ("right1", "Right 1"),
    ("right2", "Right 2"),
    ("right3", "Right 3"),
];

pub static MICE: &[Model] = &[
    Model::mouse(0x2b03, "M55", "m55").packet(128).leds(1, LED_DPI).buttons(BTN_FWD_BACK_DPI).dpi(100, 16000, false),
    Model::mouse(0x1bfb, "M55 Wireless", "m55-wireless").leds(1, LED_DPI).buttons(BTN_FWD_BACK_DPI).dpi(100, 16000, false).wireless(),
    Model::mouse(0x1b70, "M55 RGB PRO", "m55-rgb-pro").leds(2, LED_DPI_LOGO).buttons(BTN_M55_PRO).dpi(200, 12400, false),
    Model::mouse(0x1b93, "Katar PRO", "katar-pro").leds(1, LED_LOGO).buttons(BTN_FWD_BACK_DPI).dpi(200, 12400, false),
    Model::mouse(0x1bba, "Katar PRO", "katar-pro").leds(1, LED_LOGO).buttons(BTN_FWD_BACK_DPI).dpi(200, 12400, false),
    Model::mouse(0x1bac, "Katar PRO XT", "katar-pro-xt").leds(1, LED_LOGO).buttons(BTN_BACK_FWD_DPI).dpi(100, 18000, false),
    Model::mouse(0x1b80, "Dark Core RGB PRO", "dark-core-rgb-pro").leds(12, LED_DARK_CORE).buttons(BTN_DARK_CORE).dpi(100, 18000, true).wireless(),
    Model::mouse(0x1b7e, "Dark Core RGB PRO SE", "dark-core-rgb-pro-se").leds(12, LED_DARK_CORE).buttons(BTN_DARK_CORE).dpi(100, 18000, true).wireless(),
    Model::mouse(0x1b5e, "Harpoon RGB Wireless", "harpoon-rgb-wireless").leds(2, LED_DPI_LOGO).buttons(BTN_HARPOON).dpi(200, 10000, false).wireless(),
    Model::mouse(0x1b4c, "Ironclaw RGB Wireless", "ironclaw-rgb-wireless").leds(6, LED_IRONCLAW).buttons(BTN_IRONCLAW).dpi(200, 18000, true).wireless(),
    Model::mouse(0x1b79, "Sabre RGB PRO", "sabre-rgb-pro").leds(5, LED_SABRE).buttons(BTN_BACK_FWD_DPI).dpi(100, 18000, true).poll(7),
    Model::mouse(0x1b7a, "Sabre RGB PRO Champion", "sabre-rgb-pro").leds(5, LED_SABRE).buttons(BTN_BACK_FWD_DPI).dpi(100, 18000, true).poll(7),
    Model::mouse(0x1b98, "Sabre RGB PRO Wireless", "sabre-rgb-pro-wireless").leds(4, LED_LOGO_DPI).buttons(BTN_SABRE_W).dpi(100, 26000, false).wireless(),
    Model::mouse(0x1b9e, "M65 RGB ULTRA", "m65-rgb-ultra").leds(3, LED_M65_ULTRA).buttons(BTN_M65_ULTRA).dpi(100, 26000, true).poll(7),
    Model::mouse(0x1bb5, "M65 RGB ULTRA Wireless", "m65-rgb-ultra-wireless").leds(2, LED_LOGO_DPI).buttons(BTN_M65_ULTRA_W).dpi(100, 26000, true).wireless(),
    Model::mouse(0x1bd6, "M65 RGB ULTRA Wireless", "m65-rgb-ultra-wireless").leds(2, LED_LOGO_DPI).buttons(BTN_M65_ULTRA_W).dpi(100, 26000, true).wireless(),
    Model::mouse(0x1bb8, "Nightsabre Wireless", "nightsabre-wireless").leds(15, LED_NIGHTSABRE).buttons(BTN_NIGHTSABRE).dpi(100, 26000, false).wireless(),
    Model::mouse(0x1be3, "Scimitar Elite RGB", "scimitar-elite-rgb").packet(128).leds(5, LED_SCIMITAR).buttons(BTN_SCIMITAR).dpi(100, 18000, true),
    Model::mouse(0x1bdb, "Scimitar Elite Wireless", "scimitar-elite-wireless").leds(4, LED_SCIMITAR_W).buttons(BTN_SCIMITAR).dpi(100, 26000, false).wireless(),
    Model::mouse(0x2b22, "Scimitar Elite Wireless SE", "scimitar-elite-wireless-se").leds(4, LED_SCIMITAR_W).buttons(BTN_SCIMITAR).dpi(100, 33000, false).wireless(),
];

pub static HEADSETS: &[Model] = &[
    Model::headset(0x0a69, "HS80 RGB Wireless", "hs80-rgb-wireless", Lighting::Planar, LED_HEADSET, false),
    Model::headset(0x0a71, "HS80 RGB Wireless", "hs80-rgb-wireless", Lighting::Planar, LED_HEADSET, false),
    Model::headset(0x0a43, "Virtuoso RGB Wireless", "virtuoso-rgb-wireless", Lighting::Planar, LED_HEADSET, false),
    Model::headset(0x0a41, "Virtuoso RGB Wireless", "virtuoso-rgb-wireless", Lighting::Planar, LED_HEADSET, false),
    Model::headset(0x0a3d, "Virtuoso RGB Wireless SE", "virtuoso-rgb-wireless-se", Lighting::Planar, LED_HEADSET, false),
    Model::headset(0x0a3f, "Virtuoso RGB Wireless SE", "virtuoso-rgb-wireless-se", Lighting::Planar, LED_HEADSET, false),
    Model::headset(0x0a62, "Virtuoso RGB Wireless XT", "virtuoso-rgb-wireless-xt", Lighting::Planar, LED_HEADSET, true),
    Model::headset(0x0a96, "HS80 MAX Wireless", "hs80-max-wireless", Lighting::Triplets, LED_HS80_MAX, true),
    Model::headset(0x2a0e, "HS80 MAX Wireless", "hs80-max-wireless", Lighting::Triplets, LED_HS80_MAX, true),
    Model::headset(0x2a00, "Virtuoso MAX Wireless", "virtuoso-max-wireless", Lighting::Triplets, LED_VIRTUOSO_MAX, true),
    Model::headset(0x2a01, "Virtuoso MAX Wireless", "virtuoso-max-wireless", Lighting::Triplets, LED_VIRTUOSO_MAX, true),
];

/// A wireless receiver: its sub-devices are found through property 0x36.
pub struct Receiver {
    pub pid: u16,
    pub name: &'static str,
    /// Model of the paired device when its PID cannot be read (receivers
    /// sold with exactly one device).
    pub default_child: Option<u16>,
    /// Headset receivers frame every packet with a leading `0x02` byte
    /// (replies `02 …`, notifications `03 …`).
    pub headset: bool,
    /// HID interface number of the command interface, if not the usual
    /// vendor page 0xFF42 one.
    pub interface: Option<i32>,
}

const fn rx(pid: u16, name: &'static str, default_child: Option<u16>) -> Receiver {
    Receiver { pid, name, default_child, headset: false, interface: None }
}
const fn hrx(pid: u16, default_child: Option<u16>, interface: i32) -> Receiver {
    Receiver { pid, name: "Wireless headset receiver", default_child, headset: true, interface: Some(interface) }
}

pub static RECEIVERS: &[Receiver] = &[
    rx(0x1ba6, "Slipstream receiver", None),
    rx(0x1b65, "Slipstream receiver (Harpoon)", Some(0x1b5e)),
    rx(0x1b66, "Slipstream receiver (Ironclaw)", Some(0x1b4c)),
    rx(0x1b62, "Slipstream receiver (K57)", Some(0x1b6e)),
    rx(0x1b81, "Slipstream receiver (Dark Core RGB PRO)", Some(0x1b80)),
    rx(0x1b7f, "Slipstream receiver (Dark Core RGB PRO SE)", Some(0x1b7e)),
    rx(0x1bdc, "Slipstream receiver (M75)", None),
    rx(0x2b00, "Slipstream receiver (Ironclaw SE)", None),
    rx(0x1b94, "Slipstream receiver (Katar PRO Wireless)", Some(0x1b93)),
    hrx(0x0a64, None, 3),
    hrx(0x0a6b, None, 3),
    hrx(0x0a44, None, 3),
    hrx(0x0a42, None, 3),
    hrx(0x0a73, None, 3),
    hrx(0x0a3e, None, 3),
    hrx(0x0a40, None, 3),
    hrx(0x0a97, Some(0x0a96), 4),
    hrx(0x2a0f, Some(0x2a0e), 4),
    hrx(0x2a02, Some(0x2a00), 4),
    hrx(0x2a03, Some(0x2a01), 4),
];

/// Mouse key-assignment table, one byte per button in bit order: 1 keeps
/// the OS's normal button, 0 reports to software only. With `restore`,
/// everything native again.
pub fn mouse_assignment_table(buttons: &[Button], restore: bool) -> Option<Vec<u8>> {
    if buttons.is_empty() {
        return None;
    }
    Some(buttons.iter().map(|b| u8::from(restore || b.role == Role::Os)).collect())
}

pub fn receiver_for_pid(pid: u16) -> Option<&'static Receiver> {
    RECEIVERS.iter().find(|r| r.pid == pid)
}

/// Every directly connected Bragi device, and every device that can sit
/// behind a receiver.
pub fn model_for_pid(pid: u16) -> Option<&'static Model> {
    MICE.iter()
        .chain(HEADSETS.iter())
        .chain(super::keyboards::KEYBOARDS.iter())
        .find(|m| m.pid == pid)
}
