# Development

Contributions are welcome, especially support for more devices. Reports from
owners of the experimental devices (working or not) help just as much: run
`opendevicecontrol --probe` and attach the log to an issue.

## Building from source

```sh
# Linux build dependencies (Debian/Ubuntu names)
sudo apt install build-essential pkg-config libudev-dev libgtk-3-dev libxdo-dev libayatana-appindicator3-dev libpulse-dev
cargo build --release -p opendevicecontrol
sh packaging/linux/install.sh      # Linux: installs to ~/.local + udev rule
```

On Windows, `cargo build --release -p opendevicecontrol` produces
`target\release\opendevicecontrol.exe`.

Every feature should work on both Windows and Linux.

## Architecture

```
crates/
  opendevicecontrol-core/   protocol drivers; no UI. Generic Device trait:
                            sensors, controls, fan channels, limits
    vendors/corsair/        Corsair drivers: hxi.rs (PSUs), nxp/ (legacy keyboards
                            and mice), bragi/, void.rs (headsets), cooling/, ram.rs
    vendors/aula/           Aula keyboards
    system/                 This PC's sensors
    smbus/                  SMBus access (RAM lighting)
    transport.rs            request/echo-checked reply with conflict-tolerant retries
    linear11.rs             PMBus LINEAR11 codec
  opendevicecontrol/        the application
    service.rs              device thread: discovery/hotplug, polling, fan policies, energy
    hass.rs                 Home Assistant MQTT discovery + command handling
    gui*.rs                 egui window, device pages, sensors, keymap editor
    tray.rs                 tray icon (own thread)
    autostart.rs            Run key / XDG autostart
    config.rs               TOML config + state
```

Every front-end works on the generic `Device` model, so a new device driver
shows up in the GUI and in Home Assistant without any UI code. That makes
adding a device mostly a matter of writing its protocol driver in
`opendevicecontrol-core`.

## Releases

`.github/workflows/ci.yml` runs on every push and pull request: Clippy and
tests on Linux and Windows, every package built and kept as an artifact, and a
test install/uninstall of the Windows installer.

Pushing a tag `vX.Y.Z` (matching the version in `Cargo.toml`) publishes a
GitHub release with all packages and `SHA256SUMS.txt`.

**Windows installer locally** (needs [Inno Setup 6](https://jrsoftware.org/isinfo.php)):

```powershell
cargo build --release -p opendevicecontrol --features console
Copy-Item target\release\opendevicecontrol.exe target\release\opendevicecontrol-cli.exe
cargo build --release -p opendevicecontrol
iscc /DAppVersion=0.1.0 packaging\windows\opendevicecontrol.iss   # -> target\installer\
```

**Linux packages locally:**

```sh
cargo install cargo-deb cargo-generate-rpm
cargo deb -p opendevicecontrol --no-build
cargo generate-rpm -p crates/opendevicecontrol
```

`assets/` holds the package icons, generated with
`opendevicecontrol --export-icon assets/icons/opendevicecontrol-256.png 256`
(and `assets/opendevicecontrol.ico`).

## Linux notes

- **Headless:** `packaging/linux/opendevicecontrol.service` is a systemd user
  unit that runs `opendevicecontrol --headless`.
- **Tray icon:** uses StatusNotifierItem/AppIndicator. Supported natively on
  COSMIC; stock GNOME needs the *AppIndicator and KStatusNotifierItem Support*
  extension.
- **Kernel `corsair-psu` driver:** coexists with OpenDeviceControl; collisions
  are detected and retried.

## HXi/RMi protocol notes

The PSU speaks 64-byte HID reports carrying PMBus commands:

- Read: `[0x03, cmd]`. Write: `[0x02, cmd, data…]`. Handshake: `[0xfe, 0x03]`.
- Replies echo the first two bytes.
- Rails are selected with PMBus `PAGE` (0x00).
- Values use LINEAR11, except uptime (a u32 in seconds) and the mode bytes.

| Register | Meaning |
|---|---|
| `0xD1` / `0xD2` | Total uptime / uptime |
| `0xD8` | OCP mode (1 = single rail, 2 = multi rail) |
| `0xEE` | Total output power |
| `0xF0` | Fan control mode (0 = hardware, 1 = software) |
| `0x3B` | Fan duty (%) |

`PAGE` is shared state inside the PSU. If another program selects a rail
between our `PAGE` write and our reads, we'd get the wrong rail's data, so
OpenDeviceControl reads `PAGE` back after every rail read.

## Credits

- PSU protocol and efficiency curves: [liquidctl](https://github.com/liquidctl/liquidctl)
  (GPL-3.0) and the Linux kernel `corsair-psu` driver by Wilken Gottwalt
  (GPL-2.0-or-later).
- Corsair RGB memory: [OpenRGB](https://gitlab.com/CalcProgrammer1/OpenRGB)
  (GPL-2.0-or-later; `CorsairDRAMController` by Adam Honse and Erik Gilling,
  its SMBus and PawnIO layers) and OpenLinkHub's memory driver; temperature
  formats from the Linux `jc42` and `spd5118` drivers. Windows SMBus access
  through [PawnIO](https://pawnio.eu) by namazso.
- Windows CPU temperatures: register formats from the Linux `k10temp` and
  `coretemp` drivers, read through PawnIO's `AMDFamily17`/`IntelMSR` modules
  (LGPL-2.1-or-later); HWiNFO's shared-memory layout (`hwisenssm2.h`);
  LibreHardwareMonitor's web-server JSON (MPL-2.0).
- Bragi devices: ported from [ckb-next](https://github.com/ckb-next/ckb-next)
  (GPL-2.0), [OpenRGB](https://gitlab.com/CalcProgrammer1/OpenRGB)
  (GPL-2.0-or-later) and [OpenLinkHub](https://github.com/jurkovic-nikola/OpenLinkHub)
  (GPL-3.0).
- Legacy headsets: HeadsetControl, OpenRGB and OpenLinkHub.
- Aula F99: the receiver's live-lighting format from OpenRGB issue #5166 and
  draft merge requests by Rodrigo Tavares (GPL-2.0-or-later). The settings
  block, cable framing and key map come from studying the official AULA F99
  driver's configuration files and program, checked on a real F99.
- Cooling: liquidctl's `commander_core`, `commander_pro` and `hydro_platinum`
  drivers (ParkerMc, Marshall Asch, Jonas Malaco and contributors) and
  OpenLinkHub's `lsh`, `cc`, `ccxt`, `cduo`, `cpro` and `lnpro` drivers and
  LINK device table by Nikola Jurkovic, all GPL-3.0-or-later, plus OpenRGB's
  documentation of the same devices.
