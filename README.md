# OpenDeviceControl

One open-source app to monitor and control your PC's hardware and
peripherals, on **Windows and Linux**, with a built-in **Home Assistant**
integration.

## Why

I wanted something like HWMonitor or LibreHardwareMonitor that could also
control my stuff: my Corsair power supply (normally an iCUE thing), my
keyboards, and so on, all in one app. A lot of these programs only work on one
OS, and I dual boot, so I needed it on Linux too. I also wanted to send all of
it to Home Assistant. Nothing did all of that, so I wrote OpenDeviceControl.

## What it does

- **Sensors like HWMonitor:** CPU, GPU, motherboard, memory, drives, network,
  batteries, with current, min and max values and graphs. See [docs/sensors.md](docs/sensors.md).
- **Device control:** fan curves, lighting, key remapping, macros, G-keys,
  DPI and more, depending on the device.
- **Lighting across the whole PC:** sync devices to one effect, lay out your
  fans and strips like they sit in the case, and pick from over 25 effects,
  many reacting to the music (including the notes being played). See
  [docs/lighting.md](docs/lighting.md).
- **Home Assistant:** devices and entities appear automatically over MQTT,
  with two-way control. See [docs/home-assistant.md](docs/home-assistant.md).
- **Runs in the background:** tray icon, starts at login, about 30 MB of RAM
  while the window is closed.
- **Dual-boot friendly:** both installs share one Home Assistant device.

**[Supported devices →](docs/devices.md)** Corsair power supplies, keyboards,
mice, headsets, coolers and RAM, the Aula F99 keyboard, ASRock motherboard
lighting, ASUS graphics card lighting, System76 laptop keyboards, and the
PC's own sensors.

> Not affiliated with or endorsed by Corsair or any other device maker.
> "Corsair", "iCUE" and "HXi" are trademarks of Corsair Gaming, Inc.

## Install

Download the package for your system from the
[latest release](https://github.com/skylord123/opendevicecontrol/releases/latest).

| System | File |
|---|---|
| Windows 10/11 | `opendevicecontrol-windows-x86_64-setup.exe` (installer) or `opendevicecontrol-windows-x86_64.zip` (portable) |
| Pop!_OS, Ubuntu, Debian | `opendevicecontrol-amd64.deb` |
| Fedora, openSUSE | `opendevicecontrol-x86_64.rpm` |
| Other Linux | `opendevicecontrol-linux-x86_64.tar.gz` |

- **Windows:** installs per user and can start OpenDeviceControl at login. It
  asks for administrator rights once, to set up the small sensor service that
  reads CPU and motherboard sensors; the app itself runs without them (see
  [docs/windows-sensors.md](docs/windows-sensors.md)).
- **Linux:** `sudo apt install ./opendevicecontrol-amd64.deb` or
  `sudo dnf install ./opendevicecontrol-x86_64.rpm`. The package's udev rule
  gives the logged-in user access to devices without root. Turn on
  *Settings → Start OpenDeviceControl when I log in* for autostart.

If you use iCUE or Corsair LINK, close them: only one program should talk to a
Corsair device at a time ([details](docs/devices.md#running-alongside-icue--corsair-link)).

## Command line

```
opendevicecontrol [--minimized] [--headless] [--probe] [--quit] [-v] [--help] [--version]
```

On Windows use `opendevicecontrol-cli` from a terminal. `--probe` prints one
reading of every device, handy for checking permissions and support. It
refuses to run while OpenDeviceControl is running, since that copy owns the
devices. `--quit` asks a running copy to exit and hand its devices back to
hardware control; `--help` lists its exit codes.

Config lives in `%APPDATA%\opendevicecontrol\` or `~/.config/opendevicecontrol/`;
state and logs in `%LOCALAPPDATA%\opendevicecontrol\` or
`~/.local/share/opendevicecontrol/`.

## Contributing

Contributions are welcome, especially support for more devices. A lot of the
devices are marked experimental because I wrote them from protocol docs without
having the hardware. If you own one, a report with the output of
`opendevicecontrol --probe` helps a lot. Building, architecture and credits are in
[docs/development.md](docs/development.md).

## Licence

GPL-3.0-or-later. Built on the protocol work of liquidctl, OpenRGB,
OpenLinkHub, ckb-next and others; see [credits](docs/development.md#credits).
