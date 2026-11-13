# The sensor tree

OpenDeviceControl shows sensors the way hardware monitors such as HWMonitor do: a tree
of hardware, each with its sensors grouped by type, and the current, lowest
and highest value of each. It is the **📊 Sensors** tab (every device) and
the lower half of the **This PC** page (the computer only).

```
This PC
├─ System                         locked, in use, idle time, uptime
├─ ASRock B650E PG Riptide WiFi   (motherboard)
│  └─ Nuvoton NCT6799D            voltages, temperatures, fans, fan control
├─ AMD Ryzen 7 7800X3D            voltages, temperatures, powers, clocks, load
├─ Memory, each DIMM
├─ NVIDIA GeForce RTX 4070 Ti     temperatures, powers, clocks, load, fans, data
├─ Drives, each drive
├─ Network, each adapter
└─ Batteries
Corsair HX850i                    every device OpenDeviceControl drives, with its sensors
```

## Using it

| Action | Result |
|---|---|
| Click a group | expand / collapse it |
| Click a sensor | show it in the graph at the top (5 min – 1 hour) |
| Ctrl+click (or Shift+click) a sensor | add it to / remove it from the graph (up to 12, any devices) |
| Double-click a type group (e.g. *Temperatures*) | graph the whole group |
| Right-click a sensor | graph it, add/remove it, send it to Home Assistant, copy its value or id |
| Right-click a group | graph it, send all / none of it to Home Assistant, expand everything below |
| House column (when Home Assistant is on) | sent (filled), not sent (outline), some (half); click to toggle |
| Filter box | by sensor name, hardware name, type ("temp", "fan") or device |
| *Collapse all* | the overview: one line per piece of hardware |
| *Reset min/max* | forget the lowest and highest values |
| *Copy report* | the whole tree as text (value, min, max) |
| This PC page: click a sensor | chart it in the page's chart (Shift+click adds it) |

Min/max are kept since OpenDeviceControl started; graphs and trend lines use the last
hour kept in memory.

## Home Assistant: choosing what is sent

A PC has hundreds of sensors; Home Assistant gets only those you choose.

* **Default:** everything OpenDeviceControl has always sent — presence, CPU load and
  `cpu_temp`, memory, GPU, disks, network, uptime, the Linux kernel's
  temperatures (`temp_*`, up to 32) and every sensor of every other device.
  Their entity ids are unchanged.
* **Detail sensors** (per thread, per voltage, fans and clocks of the
  motherboard chip, LibreHardwareMonitor/HWiNFO readings, per drive and
  adapter…) are listed but not sent until you pick them.
* Pick them in the tree's house column, or in *Settings → Home Assistant →
  Choose sensors…* (the same tree with check boxes, a filter, *Send all
  shown*, *Send none* and *Defaults*). Changes apply immediately: new
  entities appear, switched-off ones are removed from Home Assistant (their
  discovery config is cleared).
* The choice is stored in `config.toml` as overrides of the default:
  `[ha_sensors] "this_pc/lhm_amdcpu_0_temperature_3" = true`.
* Units and classes follow Home Assistant's own definitions: data in
  `GiB` and `MiB/s` (the values are binary), fan speeds in `rpm`, idle time
  and uptime as `timestamp` sensors (*Last input*, *Last boot*: they change
  only when the event happens, not every poll), the session lock as a `lock`
  binary sensor, key presses as `button` events, and This PC named after
  the computer's host name.
* Detail sensors are named after their hardware in Home Assistant
  ("AMD Ryzen 7 7800X3D Core #1 VID").

## Where the sensors come from

| Hardware | Linux | Windows |
|---|---|---|
| CPU load, clocks | sysinfo | sysinfo |
| CPU temperature | `k10temp` / `coretemp` / `zenpower` (hwmon) | sensor service |
| CPU power, per-core clocks, VID | hwmon where the driver has them | sensor service |
| Motherboard (Super I/O) | hwmon (`nct6775`, `it87`, `f71882fg`, `asus-ec-sensors`…) | sensor service (Nuvoton, ITE, Fintek chips; ASUS EC sensors) |
| NVIDIA GPU | NVML | NVML |
| AMD GPU | amdgpu sysfs + hwmon | AMD Display Library (temperatures, power, clocks, fan) + Windows' GPU counters |
| Intel GPU | i915 / xe hwmon | load and memory from Windows' GPU counters, power from the sensor service |
| Drives | sysinfo; NVMe/`drivetemp` temperatures from hwmon | sysinfo; health (SMART / NVMe) from the sensor service |
| DIMMs | `spd5118` / `jc42` (hwmon) | sensor service (SPD5118 / JC-42.4) |
| Network | sysinfo | sysinfo |
| Batteries | `/sys/class/power_supply` | battery class driver |

On Windows nothing else needs to be installed or running: the installer
sets up the sensor service and the PawnIO driver it uses. HWiNFO and
LibreHardwareMonitor are still read when they run, for hardware OpenDeviceControl
doesn't read itself (rare motherboard chips, Intel discrete GPUs).

Nothing needs root or administrator rights except the opt-in PawnIO reader
on Windows. See [windows-sensors.md](windows-sensors.md) for the Windows
sources.

The same hardware seen by several sources is shown once: a source's node is
merged into OpenDeviceControl's own when it is the same kind and name (or the only CPU
or GPU on both sides). LibreHardwareMonitor and HWiNFO see the same
hardware, so only one of them is used (HWiNFO when both run). A chip the
sensor service reads (the motherboard's Super I/O) is taken from the service
only; the other tools' copy of it is left out. Devices
OpenDeviceControl drives itself (a Corsair PSU, for example) are left out of This PC
when another source lists them too (LibreHardwareMonitor, HWiNFO, or Linux
drivers such as `corsair-psu`): they have their own entry with OpenDeviceControl's
own readings.

### Linux notes

* Each `/sys/class/hwmon/hwmon*` chip is one node, named from its driver
  (`CPU (k10temp)`, `NCT6799`) or its device model (NVMe drives, DIMMs).
  Super I/O chips are placed under the motherboard (from
  `/sys/class/dmi/id/board_*`).
* Inputs: `temp*` (°C), `in*` (V), `fan*` (RPM, hidden until the fan has
  turned once, so unconnected headers don't clutter the list), `pwm*` with
  `pwm*_enable` (fan control %), `power*` (W), `curr*` (A), `freq*` (MHz).
  Labels come from `*_label` files.
* Temperatures keep the ids OpenDeviceControl always used (`temp_<chip>_<label>`, as
  sysinfo names them), so Home Assistant history continues.
* No motherboard voltages or fans? Run `sudo sensors-detect` (lm-sensors)
  and load the module it suggests, e.g. `nct6775` or `it87`.

## Checking

`cargo run -p opendevicecontrol-core --example thispc` prints This PC's tree with ids,
values and which sensors go to Home Assistant by default.
`cargo test -p opendevicecontrol render_screenshots -- --ignored --nocapture` draws the
real UI in software (no window or GPU needed) and writes PNG screenshots and
a text report to the temp directory's `opendevicecontrol-shots` folder.
