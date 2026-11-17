//! The NXP model table. Everything model-specific lives here as data; the
//! driver in `nxp.rs` only interprets it.
//!
//! USB IDs and quirks follow ckb-next (`usb.h`, `usb.c`) and OpenRGB
//! (`CorsairPeripheralControllerDetect.cpp`); keyboard LEDs and keys are
//! generated from ckb-next into `layouts.rs`; mouse zones and buttons follow
//! ckb-next (`keymap.c`, `led_mouse.c`, `src/gui/keymap.cpp`).

use super::layouts::*;
use super::*;

const fn key(bit: u8, id: &'static str, name: &'static str, assignable: bool) -> SpecialKey {
    SpecialKey { bit, id, name, assignable, hw_action: None, route: ROUTE_HOST }
}

const K55_KEYS: &[SpecialKey] = &[
    key(120, "g1", "G1", true),
    key(121, "g2", "G2", true),
    key(122, "g3", "G3", true),
    key(123, "g4", "G4", true),
    key(124, "g5", "G5", true),
    key(125, "g6", "G6", true),
    SpecialKey { hw_action: Some(ACT_MR), ..key(132, "mr", "MR (macro record)", true) },
    SpecialKey { hw_action: Some(ACT_LIGHT), ..key(71, KEY_BRIGHTNESS, "Brightness", false) },
    SpecialKey { hw_action: Some(ACT_LOCK), ..key(96, KEY_WINLOCK, "Win lock", false) },
];

// ---- mice: Corsair button report bits (ckb-next keymap.c, relative to
// MOUSE_BUTTON_FIRST) ----
const M_DPI_UP: SpecialKey = key(5, KEY_DPI_UP, "DPI up", false);
const M_DPI_DOWN: SpecialKey = key(6, KEY_DPI_DOWN, "DPI down", false);
const M_DPI_CYCLE: SpecialKey = key(5, KEY_DPI_UP, "DPI cycle", false);
const M_SNIPER: SpecialKey = key(7, KEY_SNIPER, "Sniper", false);
const M_PROFILE: SpecialKey = key(24, "profile", "Profile", true);
const M_PROFILE_UP: SpecialKey = key(24, "profile", "Profile up", true);
const M_PROFILE_DOWN: SpecialKey = key(23, "profile_down", "Profile down", true);

const M65_KEYS: &[SpecialKey] = &[M_DPI_UP, M_DPI_DOWN, M_SNIPER];
const SABRE_KEYS: &[SpecialKey] = &[M_DPI_UP, M_DPI_DOWN, key(8, "extra", "Extra button", true)];
const SCIMITAR_KEYS: &[SpecialKey] = &[
    M_DPI_UP,
    M_DPI_DOWN,
    key(8, "thumb1", "Side 1", true),
    key(9, "thumb2", "Side 2", true),
    key(10, "thumb3", "Side 3", true),
    key(11, "thumb4", "Side 4", true),
    key(12, "thumb5", "Side 5", true),
    key(13, "thumb6", "Side 6", true),
    key(14, "thumb7", "Side 7", true),
    key(15, "thumb8", "Side 8", true),
    key(16, "thumb9", "Side 9", true),
    key(17, "thumb10", "Side 10", true),
    key(18, "thumb11", "Side 11", true),
    key(19, "thumb12", "Side 12", true),
];
const DPI_CYCLE_KEYS: &[SpecialKey] = &[M_DPI_CYCLE];
const GLAIVE_PRO_KEYS: &[SpecialKey] = &[M_DPI_UP, M_DPI_DOWN];
const IRONCLAW_KEYS: &[SpecialKey] = &[M_DPI_CYCLE, M_PROFILE];
const NIGHTSWORD_KEYS: &[SpecialKey] = &[M_DPI_UP, M_DPI_DOWN, M_SNIPER, M_PROFILE_UP, M_PROFILE_DOWN];
const DARK_CORE_KEYS: &[SpecialKey] = &[M_DPI_UP, M_DPI_DOWN, M_SNIPER, M_PROFILE];

// ---- mouse lighting zones (ckb-next ids: 1 front, 2 back/logo, 3 dpi,
// 4 wheel, 5 thumb, 6 side) ----
const fn zone(id: &'static str, name: &'static str, leds: &'static [u8]) -> ZoneDef {
    ZoneDef { id, name, leds }
}
const Z_FRONT: ZoneDef = zone("front", "Front", &[1]);
const Z_LOGO: ZoneDef = zone("logo", "Logo", &[2]);
const Z_DPI: ZoneDef = zone("dpi", "DPI indicator", &[3]);
const Z_WHEEL: ZoneDef = zone("wheel", "Wheel", &[4]);
const Z_THUMB: ZoneDef = zone("side_keys", "Side keys", &[5]);
const Z_SIDE: ZoneDef = zone("side", "Side", &[6]);

const M65_ZONES: &[ZoneDef] = &[zone("wheel", "Wheel", &[1]), Z_DPI, Z_LOGO];
const M65E_ZONES: &[ZoneDef] = &[Z_WHEEL, Z_DPI, Z_LOGO];
const SABRE_ZONES: &[ZoneDef] = &[Z_FRONT, Z_WHEEL, Z_DPI, Z_LOGO];
const SCIMITAR_ZONES: &[ZoneDef] = &[Z_THUMB, Z_DPI, Z_WHEEL, Z_FRONT, Z_LOGO];
const HARPOON_ZONES: &[ZoneDef] = &[zone("logo", "Logo", &[3])];
const GLAIVE_ZONES: &[ZoneDef] = &[Z_SIDE, Z_FRONT, Z_LOGO];
const IRONCLAW_ZONES: &[ZoneDef] = &[Z_WHEEL, Z_LOGO];
const NIGHTSWORD_ZONES: &[ZoneDef] = &[Z_FRONT, Z_WHEEL, Z_LOGO, zone("bottom", "Bottom", &[6])];
/// Dark Core hardware-animation zones (ckb-next `led_wireless.c`): bit 0 is
/// driven from the logo colour, bit 1 from the side, bit 2 from the wheel.
const DARK_CORE_ZONES: &[ZoneDef] = &[zone("side", "Side", &[1]), zone("wheel", "Wheel", &[2]), zone("logo", "Logo", &[0])];

// ---- mousepads / stands: device zone indices, left to right ----
/// MM800: zones 1–5 run down the right edge, 6–10 along the bottom (right to
/// left), 11–15 up the left edge (ckb-next GUI `PolarisZones`).
const MM800_ZONES: &[ZoneDef] = &[zone("edge", "Edge", &[14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 0, 1, 2, 3, 4])];
/// ST100: device zones 0–8; zone 4 is the logo (OpenRGB / ckb-next).
const ST100_ZONES: &[ZoneDef] = &[zone("base", "Base", &[8, 7, 6, 0, 5, 1, 2, 3]), zone("logo", "Logo", &[4])];

const KEYBOARD: NxpModel = NxpModel {
    pid: 0,
    name: "",
    slug: "",
    kind: DeviceKind::Keyboard,
    command_interface: 1,
    input_interface: Some(0),
    zones: &[],
    lighting: Lighting::ThreeZone,
    keys: &[],
    win_lock: WinLock::Lock48,
    lighting_08: false,
    brightness: false,
    side_lights: false,
    poll_rate: false,
    max_dpi: 0,
    dpi_cycle: false,
    dpi_rgb: false,
};

const fn per_key(leds: &'static [KbLed], depth: ColorDepth) -> Lighting {
    Lighting::PerKey { leds, depth }
}

const MOUSE: NxpModel = NxpModel {
    kind: DeviceKind::Mouse,
    win_lock: WinLock::None,
    poll_rate: true,
    max_dpi: 12000,
    lighting: Lighting::Mouse,
    ..KEYBOARD
};

const PAD: NxpModel = NxpModel {
    kind: DeviceKind::Mousepad,
    command_interface: -1,
    input_interface: None,
    win_lock: WinLock::None,
    lighting: Lighting::Pad { count: 15, control: 0x04 },
    ..KEYBOARD
};

const fn kb(pid: u16, name: &'static str, slug: &'static str, lighting: Lighting, keys: &'static [SpecialKey]) -> NxpModel {
    NxpModel { pid, name, slug, lighting, keys, ..KEYBOARD }
}

const fn mouse(
    pid: u16,
    name: &'static str,
    slug: &'static str,
    zones: &'static [ZoneDef],
    keys: &'static [SpecialKey],
    max_dpi: u16,
) -> NxpModel {
    NxpModel { pid, name, slug, zones, keys, max_dpi, ..MOUSE }
}

pub static MODELS: &[NxpModel] = &[
    // ---- K55: hardware-verified, keep exactly as is ----
    NxpModel {
        pid: 0x1b3d,
        name: "K55 RGB",
        slug: "k55",
        kind: DeviceKind::Keyboard,
        command_interface: 1,
        input_interface: Some(0),
        zones: &[zone("left", "Left", &[0]), zone("middle", "Middle", &[1]), zone("right", "Right", &[2])],
        lighting: Lighting::ThreeZone,
        keys: K55_KEYS,
        win_lock: WinLock::Performance,
        lighting_08: true,
        brightness: true,
        ..KEYBOARD
    },
    // ---- per-key keyboards (ckb-next IS_FULLRANGE decides 9- vs 24-bit) ----
    kb(0x1b17, "K65 RGB", "k65", per_key(K65_LEDS, ColorDepth::Nine), K65_KEYS),
    kb(0x1b37, "K65 LUX RGB", "k65", per_key(K65_LEDS, ColorDepth::Full), K65_KEYS),
    kb(0x1b39, "K65 RGB RAPIDFIRE", "k65", per_key(K65_LEDS, ColorDepth::Full), K65_KEYS),
    NxpModel { lighting_08: true, poll_rate: true, ..kb(0x1b4f, "K68 RGB", "k68", per_key(K68_LEDS, ColorDepth::Full), K68_KEYS) },
    kb(0x1b13, "K70 RGB", "k70", per_key(K70_LEDS, ColorDepth::Nine), K70_KEYS),
    kb(0x1b33, "K70 LUX RGB", "k70", per_key(K70_LEDS, ColorDepth::Full), K70_KEYS),
    NxpModel { lighting_08: true, ..kb(0x1b38, "K70 RGB RAPIDFIRE", "k70", per_key(K70_LEDS, ColorDepth::Full), K70_KEYS) },
    NxpModel {
        lighting_08: true,
        poll_rate: true,
        ..kb(0x1b49, "K70 RGB MK.2", "k70mk2", per_key(K70MK2_LEDS, ColorDepth::Full), K70MK2_KEYS)
    },
    NxpModel {
        lighting_08: true,
        poll_rate: true,
        ..kb(0x1b6b, "K70 RGB MK.2 SE", "k70mk2", per_key(K70MK2_LEDS, ColorDepth::Full), K70MK2_KEYS)
    },
    NxpModel {
        lighting_08: true,
        poll_rate: true,
        ..kb(0x1b55, "K70 RGB MK.2 Low Profile", "k70mk2", per_key(K70MK2_LEDS, ColorDepth::Full), K70MK2_KEYS)
    },
    kb(0x1b11, "K95 RGB", "k95", per_key(K95_LEDS, ColorDepth::Nine), K95_KEYS),
    NxpModel {
        lighting_08: true,
        poll_rate: true,
        ..kb(0x1b2d, "K95 RGB PLATINUM", "k95p", per_key(K95P_LEDS, ColorDepth::Full), K95P_KEYS)
    },
    // The SE is not in ckb-next; OpenRGB drives it like the K95 Platinum
    // except for the extra init (its type byte is not special-cased there).
    kb(0x1b82, "K95 RGB PLATINUM SE", "k95p", per_key(K95P_LEDS, ColorDepth::Full), K95P_KEYS),
    NxpModel {
        side_lights: true,
        poll_rate: true,
        ..kb(0x1b20, "STRAFE RGB", "strafe", per_key(STRAFE_LEDS, ColorDepth::Full), STRAFE_KEYS)
    },
    NxpModel {
        side_lights: true,
        lighting_08: true,
        poll_rate: true,
        ..kb(0x1b48, "STRAFE RGB MK.2", "strafe_mk2", per_key(STRAFE_MK2_LEDS, ColorDepth::Full), STRAFE_MK2_KEYS)
    },
    // ---- mice (wired NXP; DPI limits from ckb-next usb.c) ----
    mouse(0x1b12, "M65 RGB", "m65", M65_ZONES, M65_KEYS, 8200),
    mouse(0x1b2e, "M65 PRO RGB", "m65", M65_ZONES, M65_KEYS, 12000),
    mouse(0x1b5a, "M65 RGB ELITE", "m65", M65E_ZONES, M65_KEYS, 18000),
    mouse(0x1b14, "SABRE RGB (optical)", "sabre", SABRE_ZONES, SABRE_KEYS, 6400),
    mouse(0x1b19, "SABRE RGB (laser)", "sabre", SABRE_ZONES, SABRE_KEYS, 8200),
    mouse(0x1b2f, "SABRE RGB", "sabre", SABRE_ZONES, SABRE_KEYS, 10000),
    mouse(0x1b32, "SABRE RGB (optical)", "sabre", SABRE_ZONES, SABRE_KEYS, 6400),
    mouse(0x1b1e, "SCIMITAR RGB", "scimitar", SCIMITAR_ZONES, SCIMITAR_KEYS, 12000),
    mouse(0x1b3e, "SCIMITAR PRO RGB", "scimitar", SCIMITAR_ZONES, SCIMITAR_KEYS, 16000),
    mouse(0x1b8b, "SCIMITAR RGB ELITE", "scimitar", SCIMITAR_ZONES, SCIMITAR_KEYS, 18000),
    NxpModel { dpi_cycle: true, ..mouse(0x1b3c, "HARPOON RGB", "harpoon", HARPOON_ZONES, DPI_CYCLE_KEYS, 6000) },
    NxpModel { dpi_cycle: true, ..mouse(0x1b75, "HARPOON RGB PRO", "harpoon", HARPOON_ZONES, DPI_CYCLE_KEYS, 12000) },
    NxpModel {
        dpi_cycle: true,
        ..mouse(0x1b34, "GLAIVE RGB", "glaive", GLAIVE_ZONES, DPI_CYCLE_KEYS, 16000)
    },
    NxpModel {
        dpi_rgb: true,
        ..mouse(0x1b74, "GLAIVE RGB PRO", "glaive", GLAIVE_ZONES, GLAIVE_PRO_KEYS, 16000)
    },
    NxpModel { dpi_cycle: true, ..mouse(0x1b5d, "IRONCLAW RGB", "ironclaw", IRONCLAW_ZONES, IRONCLAW_KEYS, 18000) },
    mouse(0x1b5c, "NIGHTSWORD RGB", "nightsword", NIGHTSWORD_ZONES, NIGHTSWORD_KEYS, 18000),
    NxpModel {
        dpi_rgb: true,
        lighting: Lighting::DarkCore,
        ..mouse(0x1b35, "DARK CORE RGB (wired)", "darkcore", DARK_CORE_ZONES, DARK_CORE_KEYS, 16000)
    },
    NxpModel {
        dpi_rgb: true,
        lighting: Lighting::DarkCore,
        ..mouse(0x1b4b, "DARK CORE RGB SE (wired)", "darkcore", DARK_CORE_ZONES, DARK_CORE_KEYS, 16000)
    },
    // ---- mousepad and headset stand ----
    NxpModel { pid: 0x1b3b, name: "MM800 RGB POLARIS", slug: "mm800", zones: MM800_ZONES, ..PAD },
    NxpModel {
        pid: 0x0a34,
        name: "ST100 RGB",
        slug: "st100",
        kind: DeviceKind::Other,
        zones: ST100_ZONES,
        lighting: Lighting::Pad { count: 9, control: 0x03 },
        ..PAD
    },
];
