# Supported devices

✅ tested on real hardware · 🧪 experimental: written from other open-source
projects' protocol documentation, not yet tested on hardware (reports welcome:
run `opendevicecontrol --probe` and attach the log) · ⚠️ experimental and off
by default

Every device also appears in Home Assistant; see
[home-assistant.md](home-assistant.md).

## Overview

| Device | USB ID | Status |
|---|---|---|
| **This PC** (CPU, GPU, motherboard, drives, network, presence) | n/a | ✅ see [sensors.md](sensors.md) |
| HX550i, HX650i, HX750i, **HX850i**, HX1000i, HX1200i, HX1500i (incl. 2023/2025 revisions) | `1b1c:1c03–1c08, 1c1e, 1c1f, 1c23, 1c27` | ✅ full |
| RM550i, RM650i, RM750i, RM850i, RM1000i | `1b1c:1c09–1c0d` | ✅ full |
| **K55 RGB** keyboard | `1b1c:1b3d` | ✅ lighting, G-keys, Win lock |
| **Aula F99** keyboard (2.4 GHz receiver or USB cable) | `3554:fa09`, `258a:010c` | ✅ per-key and sound-reactive lighting, built-in effects, battery, key remapping, macros, OS mode, Win-key lock, sleep, debounce. Bluetooth: not supported |
| **ASRock Polychrome RGB** (motherboard lighting on recent ASRock boards) | `26ce:01a2` | ✅ every zone and addressable header, OpenDeviceControl and sound-reactive lighting, all 18 built-in effects with any colour, apply to all channels, LED counts |
| ASUS Aura motherboard lighting (AURA LED Controller, AURA MAINBOARD) | `0b05:18f3, 1939, 19af, 1aa6, 1bed, 1867, 1872, 18a3, 18a5` | 🧪 zones the board reports, OpenDeviceControl and sound-reactive lighting, 18 built-in effects with colour, LED counts |
| MSI Mystic Light motherboard lighting (162- and 185-byte boards, about 2018 to 2024) | `1462:*`, `0db0:0076` | 🧪 zones of about 100 known boards, OpenDeviceControl lighting where the board has a direct mode, built-in effects with colour, speed and brightness, LED counts |
| Gigabyte RGB Fusion 2 motherboard lighting | `048d:8297, 5702, 5711, 8950` | 🧪 addressable headers and single LEDs, OpenDeviceControl and sound-reactive lighting, built-in effects with colour, speed and brightness, LED counts |
| **ASUS GeForce RTX 4070 Ti SUPER** lighting (PCI `10de:2705`, subsystem `1043:8957`) | n/a | ✅ all 4 LEDs, OpenDeviceControl and sound-reactive lighting, 14 built-in effects with colour, speed and direction. Linux: 🧪 same protocol, not yet tested on hardware |
| **System76 laptop keyboards** with an RGB backlight (tested: Gazelle `gaze15`) | n/a | ✅ colour and brightness, OpenDeviceControl and sound-reactive lighting. Linux only |
| **Speedlink ACCUSOR** keyboard (SL-670005-BK) | `264a:3008` | ✅ profiles, backlight level and colour, key remapping on all 3 profiles, macros stored in the keyboard, OpenDeviceControl keys |
| **Bose QuietComfort Headphones** (2023), over Bluetooth | Bluetooth `009e:4075` | ✅ modes, EQ, shortcut button (also as an OpenDeviceControl key), voice prompts, auto-off, multipoint, self voice, battery, paired devices |
| Other Bose headphones, earbuds and Bluetooth speakers of the Bose Music app (QC Ultra, QC 45, NC 700, QC Earbuds, Ultra Open, SoundLink Flex…) | Bluetooth `009e:*` | 🧪 whatever each model reports: the same settings, plus earbud batteries, in-ear detection, spatial audio, voice assistant and more |
| **Logitech MX Master 3S** (Bluetooth or Bolt receiver), **Performance Mouse MX** (Unifying receiver) | `046d:b034`, `046d:c548`, `046d:c52b` | ✅ DPI, SmartShift, scroll and thumb wheel, button diversion (buttons, gestures and wheels as OpenDeviceControl keys), button remapping, Easy-Switch host, battery, receivers' paired devices |
| Other Logitech HID++ mice, keyboards and headsets (Unifying, Bolt, Nano, Lightspeed, USB cable, Bluetooth) | `046d:*` | 🧪 whatever each device offers, like Solaar: report rate, onboard profiles, keyboard backlight, Fn keys, disable keys, OS layout, crown, G-keys, haptics, sidetone, EQ; pairing on receivers |
| K65 RGB / LUX RGB / RGB RAPIDFIRE keyboards | `1b1c:1b17, 1b37, 1b39` | 🧪 per-key lighting, special keys, Win lock |
| K68 RGB keyboard | `1b1c:1b4f` | 🧪 per-key lighting, special keys, Win lock, polling rate |
| K70 RGB / LUX RGB / RGB RAPIDFIRE keyboards | `1b1c:1b13, 1b33, 1b38` | 🧪 per-key lighting, special keys, Win lock |
| K70 RGB MK.2 / MK.2 SE / MK.2 Low Profile keyboards | `1b1c:1b49, 1b6b, 1b55` | 🧪 per-key lighting, profile key, Win lock, polling rate |
| K95 RGB keyboard | `1b1c:1b11` | 🧪 per-key lighting, G1–G18, MR, M1–M3, Win lock |
| K95 RGB PLATINUM / PLATINUM SE keyboards | `1b1c:1b2d, 1b82` | 🧪 per-key + light-bar lighting, G1–G6, profile key, Win lock |
| STRAFE RGB / STRAFE RGB MK.2 keyboards | `1b1c:1b20, 1b48` | 🧪 per-key lighting, side lights, Win lock, polling rate |
| M65 RGB / PRO RGB / RGB ELITE mice | `1b1c:1b12, 1b2e, 1b5a` | 🧪 zone lighting, DPI stages, sniper, polling rate |
| SABRE RGB mice | `1b1c:1b14, 1b19, 1b2f, 1b32` | 🧪 zone lighting, DPI, extra button, polling rate |
| SCIMITAR RGB / PRO RGB / RGB ELITE mice | `1b1c:1b1e, 1b3e, 1b8b` | 🧪 zone lighting, DPI, 12 side keys, polling rate |
| HARPOON RGB / RGB PRO (wired) mice | `1b1c:1b3c, 1b75` | 🧪 logo lighting, DPI, polling rate |
| GLAIVE RGB / RGB PRO mice | `1b1c:1b34, 1b74` | 🧪 zone lighting, DPI, polling rate |
| IRONCLAW RGB (wired) mouse | `1b1c:1b5d` | 🧪 zone lighting, DPI, profile button, polling rate |
| NIGHTSWORD RGB mouse | `1b1c:1b5c` | 🧪 zone lighting, DPI, sniper, profile buttons, polling rate |
| DARK CORE RGB / RGB SE (USB cable only) mice | `1b1c:1b35, 1b4b` | 🧪 zone lighting, DPI, polling rate |
| MM800 RGB POLARIS mousepad | `1b1c:1b3b` | 🧪 15-zone lighting |
| ST100 RGB headset stand | `1b1c:0a34` | 🧪 8-zone base + logo lighting |
| VOID / VOID RGB Wireless, VOID PRO Wireless, VOID (RGB) ELITE Wireless headsets | `1b1c:1b27, 0a2b, 0a14, 0a16, 0a1a, 0a51, 0a55, 0a75` | 🧪 battery, charging, on/off state, logo lighting, mic-mute LED, sidetone, notification sounds |
| VOID RGB USB, VOID PRO USB / Surround, VOID ELITE USB headsets | `1b1c:1b1c, 1b23, 1b29, 1b2a, 0a17, 0a1d, 0a52, 0a56` | 🧪 logo lighting, mic-mute LED, sidetone, notification sounds |
| HS70 Wireless, HS70 PRO Wireless headsets | `1b1c:0a38, 0a4f` | 🧪 battery, charging, on/off state, sidetone, notification sounds |
| K55 RGB PRO, K55 RGB PRO XT (Bragi) | `1b1c:1ba4, 1ba1` | 🧪 lighting, G-keys, MR, Win lock |
| K57 RGB Wireless (Bragi) | `1b1c:1b6e`, via Slipstream | 🧪 lighting (cable only), G-keys, battery |
| K60 RGB PRO / LP / SE / TKL (Bragi) | `1b1c:1ba0, 1bad, 1b8d, 1bc7, 1bed` | 🧪 lighting, Win lock |
| K65 RGB MINI (Bragi) | `1b1c:1baf, 1bc3, 1bcf` | 🧪 lighting, Win lock, polling rate |
| K70 RGB PRO, K70 RGB TKL (+Champion), K70 CORE RGB (+TKL) (Bragi) | `1b1c:1bc4, 1bc6, 1bb3, 1bd4, 1b73, 1bb9, 1bfd, 1bff, 2b0a, 2b01` | 🧪 lighting, Win lock, dial (CORE), polling rate |
| K95 RGB PLATINUM XT, K100 RGB (Bragi) | `1b1c:1b89, 1b7c, 1bc5, 1b7d` | 🧪 lighting, G-keys, dial (K100), Win lock |
| K100 AIR Wireless (Bragi) | `1b1c:1bab`, via Slipstream | 🧪 lighting (cable only), G-keys, battery |
| M55, M55 RGB PRO, Katar PRO / PRO XT (Bragi) | `1b1c:2b03, 1bfb, 1b70, 1b93, 1bba, 1bac` | 🧪 lighting, DPI, polling rate, buttons |
| Dark Core RGB PRO (+SE), Harpoon RGB Wireless, Ironclaw RGB Wireless (Bragi) | `1b1c:1b80, 1b7e, 1b5e, 1b4c`, via Slipstream | 🧪 lighting, DPI, buttons, battery |
| Sabre RGB PRO (+Wireless), M65 RGB ULTRA (+Wireless), Nightsabre Wireless (Bragi) | `1b1c:1b79, 1b7a, 1b98, 1b9e, 1bb5, 1bd6, 1bb8` | 🧪 lighting, DPI, polling rate (up to 8000 Hz), buttons, battery |
| Scimitar Elite RGB (+Wireless, SE) (Bragi) | `1b1c:1be3, 1bdb, 2b22` | 🧪 lighting, DPI, 12 side buttons, battery |
| Slipstream wireless receivers | `1b1c:1ba6, 1b65, 1b66, 1b62, 1b81, 1b7f, 1bdc, 2b00, 1b94` | 🧪 each paired device appears as its own device |
| HS80 RGB Wireless, HS80 MAX, Virtuoso RGB Wireless (+SE, XT), Virtuoso MAX (Bragi headsets) | `1b1c:0a69, 0a71, 0a43, 0a41, 0a3d, 0a3f, 0a62` and their receivers | 🧪 lighting, battery, sidetone (MAX/XT) |
| iCUE LINK System Hub + LINK fans (QX, LX, RX), LINK AIOs (H100i–H170i, TITAN), XD5/XD6 pumps, XC7/XG3/XG7 blocks | `1b1c:0c3f` | 🧪 sensors, fans/pumps, lighting |
| Commander Core / Commander ST with H100i–H170i Elite Capellix / Elite LCD (XT) | `1b1c:0c1c, 0c32` | 🧪 coolant + probes, pump/fans, lighting |
| Commander Core XT | `1b1c:0c2a` | 🧪 probes, fans, lighting |
| Commander DUO | `1b1c:0c56` | 🧪 probes, fans (no lighting yet) |
| Commander Pro, Obsidian 1000D hub | `1b1c:0c10, 1d00` | 🧪 probes, fans, rail voltages, lighting |
| Lighting Node Pro, Lighting Node Core | `1b1c:0c0b, 1d04, 0c1a` | 🧪 lighting |
| Hydro H100i/H115i Platinum (SE), H60i–H150i Pro XT, iCUE H100i–H150i Elite RGB | `1b1c:0c17–0c19, 0c20–0c22, 0c29, 0c2d, 0c35–0c37, 0c40, 0c41` | 🧪 coolant, pump mode, fans, lighting |
| Vengeance RGB PRO / PRO SL / RS DDR4, Dominator Platinum RGB DDR4 (RAM) | SMBus `0x58–0x5F` | ⚠️ lighting, DIMM temperature |
| Vengeance RGB DDR5 / RS DDR5, Dominator Platinum / Titanium RGB DDR5 (RAM) | SMBus `0x18–0x1F` | ⚠️ lighting, DIMM temperature |
| Vengeance RGB DDR4, original single-zone (RAM) | SMBus `0x58–0x5F` | ⚠️ lighting, DIMM temperature |

Want another device supported? Contributions are welcome; see
[development.md](development.md).

## This PC

OpenDeviceControl always shows the computer it runs on as **This PC**, with
every sensor the machine has, laid out like HWMonitor. The **📊 Sensors** tab
shows the same tree for every device at once, with graphs. Details, including
what each OS can read and how, are in [sensors.md](sensors.md) and
[windows-sensors.md](windows-sensors.md).

Presence sensors for Home Assistant:

- **Locked:** whether the session is locked (Windows: the real session lock,
  so a UAC prompt doesn't count; Linux: logind's lock hint, which GNOME and
  KDE set, or on COSMIC the lock file its lock screen keeps while locked).
- **Last input:** when the keyboard or mouse was last used (Windows:
  `GetLastInputInfo`; Linux: GNOME's idle monitor, the compositor's
  `ext-idle-notify` on COSMIC, KDE and wlroots desktops, else logind's idle
  hint).
- **In use:** on while the PC is unlocked and there was input in the last N
  minutes (*Count as in use for*, default 5). Use it for "I'm at my desk"
  automations.

On a dual-boot machine This PC has the same id on both systems.

## Corsair HXi / RMi power supplies

Tested on an **HX850i** (Windows 11). The other models use the same protocol.

- Input voltage (and current where reported); per-rail voltage, current and
  power for +12V, +5V and +3.3V; total output power.
- **AC input power, efficiency and energy (kWh), estimated.** These PSUs don't
  measure the AC side; OpenDeviceControl estimates it from per-model efficiency
  curves, like iCUE and liquidctl. The energy counter survives restarts and
  works with HA's Energy dashboard.
- VRM and case temperature, fan speed and duty, uptime and lifetime uptime,
  all protection limits.
- **Fan control:** the PSU's own curve (incl. zero-RPM), a fixed duty
  (30–100 %), or a temperature → duty curve. Control goes back to the PSU's
  curve when OpenDeviceControl exits.
- **+12V OCP mode:** single rail or multi rail.

## Corsair K55 RGB

- Lighting: static, breathing, colour cycle, rainbow wave, colour wave;
  brightness (the brightness key cycles 100 → 66 → 33 → off); live preview.
- **G1–G6 and MR** can press a hotkey, type text, launch a program/file/URL,
  send a media key, or play a recorded **macro**. Every press is also a Home
  Assistant event.
- **Macros** are recorded and kept in OpenDeviceControl (the K55 does not store
  macros on the keyboard), then assigned to a key with the *Macro* action.
  Record one on the keyboard's page: click Record and type the sequence, which
  captures the keystrokes and their timing.
- **Brightness** (the keyboard's own level, 0 to 100 %) is a control and
  follows the brightness key.
- **Win lock** disables the Windows key, and optionally Alt+Tab, Alt+F4 and
  Shift+Tab. The lock key and Home Assistant toggle it.
- On exit the keyboard returns to its built-in lighting and keys.

## Aula F99

Everything comes from the keyboard's own firmware (studied from the official
updater, never flashed), so OpenDeviceControl offers more than the official
driver.

- Works over the 2.4 GHz receiver and the USB cable, as one device: it uses
  the cable while that's plugged in and switches back to the receiver when you
  unplug it. One F99 at a time is supported. **Battery level and charging**
  either way.
- **Lighting:** OpenDeviceControl effects and sound-reactive effects per key
  (smooth over the cable, about 10 fps for moving effects over the radio), or
  one of the keyboard's 14 built-in effects with a preview that simulates the
  firmware.
- **Key remapping** on four layers (Base, Fn, Fn2, Tap for tap/hold keys): keys,
  shortcuts, media, mouse, macros, layer switches, keyboard functions, or an
  *OpenDeviceControl key* that runs an action or goes to Home Assistant (over
  the cable). The original keymap is backed up first; Fn+Esc held for 3 s
  resets to factory.
- **Macros:** 32 slots in the keyboard, recorded or edited step by step.
- **Settings:** sleep time, debounce, OS mode, Win-key lock, tap/hold time.
- Shows as offline while the keyboard sleeps and comes back by itself; changes
  made with Fn keys show up in the app. On exit the keyboard returns to its
  built-in effect.
- Not available: polling rate and NKRO (no firmware setting), remapping the
  Mac modes' base layer, Bluetooth.

## ASRock Polychrome RGB

The lighting controller on recent ASRock motherboards (a USB device inside
the board, `26ce:01a2`). Tested on a B650E and a B760M board; other boards
with this controller should work too.

- **Zones:** whatever the board has of RGB LED 1 and 2, addressable RGB LED
  1 to 3, the PCH heatsink, the I/O cover and the PCB, named as in ASRock's
  utility. The board reports which ones exist and how many LEDs each has.
  Zones can be renamed (the pencil next to each one), say "Rear case fan".
- **Lighting:** OpenDeviceControl effects and sound-reactive effects on every
  LED, or a built-in effect per zone: off, static, breathing, strobe, colour
  cycle, random, music, wave, spring, stack, cram, scan, neon, water,
  rainbow, rainbow 2, glow stream, luminous tide. Built-in effects are saved
  to the board when OpenDeviceControl closes, so they stay after a reboot.
- **Apply to all channels:** one effect on every zone, running in step.
- **Addressable headers:** set how many LEDs are plugged in, and swap red and
  green for strips that need it. Both are stored on the board.
- Close ASRock Polychrome RGB first: two programs driving the board at once
  fight over it.

## ASUS, MSI and Gigabyte motherboards (untested)

Drivers for the USB lighting controllers on most ASUS, MSI and Gigabyte
boards from about 2018 on. They follow the protocol work of OpenRGB (and
liquidctl for Gigabyte) and have **not been tried on hardware yet**: reports
from owners are welcome. Protocol notes and open questions are in
[reverse-engineering/](reverse-engineering/README.md).

- **ASUS Aura** (`0b05:…`): the zones the board's config table lists, 18
  built-in effects with a colour, OpenDeviceControl lighting on every zone.
  The board can't report its effect, so the one shown is the last set in
  OpenDeviceControl. Effects changed here are saved to the board when
  OpenDeviceControl closes. Set how many LEDs each addressable header has.
- **MSI Mystic Light** (`1462:…`, `0db0:0076`): boards using the 162- and
  185-byte protocols (Z390/X399/B450/B460 through X570 and later), with
  zones from a per-board table. Built-in effects with colour, speed and
  brightness; OpenDeviceControl lighting where the board has a direct mode.
  The newest boards with the 761-byte controller (X870, B850, Z890) are not
  supported.
- **Gigabyte RGB Fusion 2** (`048d:…`): the addressable (D_LED) headers and
  the board's single-colour LEDs, with generic names (the controller doesn't
  say which LEDs a board has). Built-in effects with colour, speed and
  brightness, and OpenDeviceControl lighting. Effects are not saved to the
  board; OpenDeviceControl applies them again when it starts.
- Close the vendor's own utility (Armoury Crate, MSI Center, RGB Fusion /
  Gigabyte Control Center) first: two programs driving the board fight over
  it.

## ASUS graphics cards

The Aura lighting on ASUS GeForce cards: an ENE controller on the card's
own I²C bus. Tested on an RTX 4070 Ti SUPER (subsystem `1043:8957`). Other
ASUS cards with this controller probably work too, but each one has to be
added to the list in the driver (`vendors/asus/mod.rs`), since the driver
only talks to cards it knows.

- **Lighting:** OpenDeviceControl effects and sound-reactive effects on the
  card's 4 LEDs, or a built-in effect: off, static, breathing, flashing,
  spectrum cycle, rainbow, spectrum cycle breathing, chase fade, spectrum
  cycle chase fade, chase, spectrum cycle chase, spectrum cycle wave, chase
  rainbow pulse and random flicker, with a colour, speed and direction where
  the effect has them.
- **Nothing is saved to the card.** OpenDeviceControl remembers the built-in
  effect you pick and sets it again each time it starts. While it isn't
  running (or before it starts), the card shows the effect last saved to it
  by ASUS software, or the factory rainbow.
- **Windows:** works through NVIDIA's own driver (NvAPI). No extra driver
  and no administrator rights.
- **Linux:** needs the NVIDIA driver, the `i2c-dev` module and the udev rule
  shipped in `packaging/linux` (it opens up only the NVIDIA bus that carries
  the lighting controller).
- Close Armoury Crate, GPU Tweak or OpenRGB first: two programs driving the
  card at once fight over it.

## System76 laptop keyboards

The RGB keyboard backlight of System76 laptops, through the `system76` /
`system76_acpi` kernel drivers that Pop!_OS ships. Tested on a Gazelle
(`gaze15`), a single-colour RGB keyboard; models with one `color`
attribute or several colour zones are handled the same way, as one zone.

- **Colour and brightness** as controls (also in Home Assistant). The Fn
  keys still work and the controls follow them.
- **OpenDeviceControl lighting:** off by default. When on, effects, sync
  and sound-reactive lighting drive the backlight; a colour's brightness
  becomes the backlight's. Turning it off, or quitting, puts back what the
  keyboard showed before.
- **Linux only.** On Windows the backlight goes through Clevo's own driver,
  which isn't supported.
- Needs the udev rule shipped in `packaging/linux`, which makes just the
  backlight's brightness and colour attributes writable.

## Speedlink ACCUSOR

A membrane gaming keyboard with M1–M6 macro keys, P1–P3 profile keys and a
single-colour backlight. It reports itself as a "Tt eSPORTS CHALLENGER Prime
RGB", the OEM board it is built on; OpenDeviceControl only takes boards that
answer the ACCUSOR's profile query. Protocol notes:
[reverse-engineering/speedlink-accusor.md](reverse-engineering/speedlink-accusor.md).

- **Profiles:** the keyboard stores three. Pick one in the app or with P1–P3;
  the app follows the keys.
- **Backlight:** off, 25 %, 50 %, 100 % or breathing, in red, blue or purple,
  per profile. There is no other colour and no per-key lighting, so
  OpenDeviceControl effects don't apply. Fn+Scroll Lock switches the light off
  and on by itself; while it is off, the app's setting has no visible effect.
- **Key remapping** on each profile: keys, media keys, mouse buttons, macros,
  the keyboard's own functions (profile keys, F-keys with their Fn extras,
  the gaming-mode WASD/arrow keys), or an *OpenDeviceControl key* that runs an
  action or goes to Home Assistant. A key sends one key; for a shortcut such
  as Ctrl+C, record a macro.
- **Macros:** 18, up to 63 presses and releases each, played by the keyboard
  itself.
- The keyboard can't report its keys, macros or lighting, so the app shows
  what it last saved (or the factory setup) and keeps that copy in its data
  folder.
- Not available: polling rate (the official driver's command has no
  effect; Fn+F3/F4 still work), Win-key lock (Fn+Win), key response time
  (Fn+F1/F2), gaming mode (Fn+Print Screen).
- Linux needs the udev rule shipped in `packaging/linux`.

## Bose headphones

Bose headphones, earbuds and Bluetooth speakers that the Bose Music app
controls, without the app: OpenDeviceControl speaks their Bluetooth protocol
(BMAP) directly. Tested with the QuietComfort Headphones (2023); other models
show the settings they report. Protocol notes:
[reverse-engineering/bose-qc-headphones.md](reverse-engineering/bose-qc-headphones.md).

- Pair and connect the headphones to the computer as usual. OpenDeviceControl
  finds them by itself once they are connected; they can stay connected to a
  phone at the same time, and the Bose app on the phone keeps working.
- **Modes:** switch between Quiet, Aware and your own modes. A mode changed
  with the headphones' button shows in the app straight away.
- **Settings:** EQ (bass, mid, treble), the shortcut button, voice prompts
  and their language, auto-off, multipoint, self voice, and on other models
  noise cancelling, spatial audio, on-head detection, touch controls and so
  on. Settings changed from the phone show here within a minute.
- **Paired devices:** connect or disconnect each paired device and turn on
  pairing mode. The device the headphones treat as the current source is
  marked; the volume control changes that device's volume.
- **Battery** level and charging (per earbud and case on earbuds).
- **Shortcut button:** set it to "Spotify Tap" and its press and hold becomes
  a key on the Keys tab that can run any OpenDeviceControl action (type text,
  run a command, Home Assistant…).
- Not available: renaming the headphones, creating or deleting modes,
  firmware updates.
- Linux: works with BlueZ (any distribution); no udev rule needed. Products
  that only connect over Bluetooth Low Energy (Bose Wi-Fi speakers, soundbars,
  L1 Pro / S1 Pro+) are not supported.

## Logitech mice, keyboards and receivers

Logitech devices that Solaar or Logi Options+ would control, without either:
OpenDeviceControl speaks HID++ directly, through a Unifying, Bolt, Nano or
Lightspeed receiver, on a USB cable or over Bluetooth. Protocol notes:
[reverse-engineering/logitech-hidpp.md](reverse-engineering/logitech-hidpp.md).

- **Settings** follow what the device offers, in cards: pointer (DPI, pointer
  speed, report rate), scroll wheel (ratchet, SmartShift, direction, high
  resolution), thumb wheel, keyboard (backlight, Fn keys, disabled keys, OS
  layout), device (Easy-Switch computer, onboard profile, haptics, power-off).
- **Buttons:** each button can stay normal, act as another button (in the
  device), or be sent to OpenDeviceControl. A sent button becomes a key on the
  Keys tab that can run any action. Buttons that report movement (the MX
  gesture button) also offer **mouse gestures** (click, swipe up, down, left
  and right, each its own key) and **sliding DPI**. Wheels, the crown and
  G-keys can be sent to OpenDeviceControl the same way.
- **Battery** level and charging.
- **Receivers** show their paired devices, can unpair them (Unifying and Bolt),
  and pair new ones (Bolt shows the passkey to type or click).
- Devices forget most settings when they sleep or switch off; what you set
  here is put back each time they connect.
- Quit Solaar while OpenDeviceControl runs: both would fight over the buttons.
- Linux: install the udev rules (`packaging/linux/60-opendevicecontrol.rules`)
  or Solaar's, for access to the receivers and devices.

## Sound-reactive lighting

Every device with lighting can follow what the computer is playing (the
*Sound reactive* tab of its lighting section), with rainbow, custom or
note colours and a sensitivity; see [lighting.md](lighting.md) for the
effects. OpenDeviceControl listens to the default
output device (WASAPI loopback on Windows, the PulseAudio/PipeWire monitor on
Linux), records nothing, and only captures while a sound effect is shown.

On the Aula F99's USB cable, live lighting runs at about 10 frames a second,
the same rate as the official driver. The keyboard's firmware spends about
67 ms receiving each frame, and it can't handle keys during that time. So
OpenDeviceControl pauses lighting frames while you type and picks them up
again right after; sound effects show the music's peak since the last frame,
so beats aren't lost. Over 2.4 GHz none of this applies.

## Corsair RGB memory (experimental, off by default)

RAM is controlled over the motherboard's SMBus, which also holds the memory's
SPD chips, so it is off by default. Enable it in **Settings → Memory (RAM)
lighting**: one "Corsair memory" device appears with a lighting zone and a
temperature per DIMM.

- **Linux:** needs `i2c-dev`; the packages load it and grant the desktop user
  access to the chipset SMBus only.
- **Windows:** needs [PawnIO](https://pawnio.eu) with its signed
  `SmbusI801.bin`/`SmbusPIIX4.bin` modules in `pawnio\` and OpenDeviceControl
  running as administrator. WinRing0 and inpoutx64 are never used.

Read [ram.md](ram.md) for the safety rules and caveats.

## Corsair legacy headsets (VOID family, HS70; experimental)

- Battery and charging (wireless); offline while switched off, back by itself.
- Logo lighting, mic-mute LED (lit while the boom is up), sidetone 0–100 %
  (re-applied on reconnect), built-in notification sounds.
- Not supported: auto-shutdown timer and equaliser; HS60 HAPTIC, HS50/HS60.

## Corsair Bragi devices (experimental)

Corsair's 2020+ keyboards, mice, Slipstream receivers and wireless headsets.
While OpenDeviceControl runs they are in software mode: lighting comes from
the app, G-keys / extra buttons / the iCUE dial work as macro keys and Home
Assistant events, and normal mouse buttons stay normal. DPI buttons step
through 400–6400 DPI; sniper lowers DPI while held. Wireless devices report
battery and charging. On exit everything returns to hardware mode.

Known gaps: per-key effects follow key-code order, not the physical layout;
K57 and K100 AIR lighting works only over the cable; Slipstream V2
(`1b1c:2b1b`), the K70 PRO light bar and on-board profiles are not supported.

## Corsair cooling (experimental)

- **One device per hub.** Fans, pumps and AIOs behind a LINK System Hub or a
  Commander are channels, sensors and lighting zones of the hub, named after
  what is attached. Lighting Node Pro / Commander Pro can't detect LED counts:
  set them per channel in the device page.
- **Sensors:** coolant, temperature probes, fan and pump RPM, applied duty,
  and (Commander Pro) rail voltages.
- **Fans and pumps:** Hardware, Fixed or Curve. A curve can follow any
  temperature of any device, or the CPU. If the input is missing the fan runs
  at 100 %.
- **Pumps never stop:** AIO pumps stay ≥ 50 %, custom-loop pumps ≥ 30 %.
- **On exit** hubs return to hardware mode; the Commander Pro and Hydro coolers
  get a temperature-based default curve programmed instead.
- Not supported yet: LINK LCDs and pump-cap displays, LINK adapter strips,
  LINK PSU rail readings, Commander DUO lighting, legacy Asetek and older
  CoolIT Hydro coolers.

## Running alongside iCUE / Corsair LINK

Only one program should talk to a Corsair device at a time. OpenDeviceControl
verifies every reply and retries when another program interferes, and shows a
warning while iCUE or LINK is running. For a clean setup, stop Corsair's
services from an **elevated** PowerShell (or uninstall iCUE / LINK):

```powershell
Stop-Process -Name iCUE -ErrorAction SilentlyContinue
Stop-Service CorsairService, CLink4Service
Set-Service CorsairService -StartupType Disabled
Set-Service CLink4Service -StartupType Disabled
```

To undo, set the services back to `Automatic` and start iCUE.
