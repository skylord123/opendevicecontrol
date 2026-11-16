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
  so a UAC prompt doesn't count; Linux: logind's lock hint).
- **Last input:** when the keyboard or mouse was last used (Windows:
  `GetLastInputInfo`; Linux: GNOME's idle monitor, or logind's idle hint).
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
- **G1–G6 and MR** can press a hotkey, type text, launch a program/file/URL or
  send a media key. Every press is also a Home Assistant event.
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
the board, `26ce:01a2`). Tested on a B650E board; other boards with this
controller should work too.

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

## Sound-reactive lighting

Every device with lighting can follow what the computer is playing (the
*Sound reactive* tab of its lighting section): Spectrum, Beat pulse, Ripple,
Sound stream, Rain, Level meter, Radar and Level colour, with rainbow or
custom colours and a sensitivity. OpenDeviceControl listens to the default
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
