# CPU and motherboard temperatures on Windows

On Linux, the kernel's hwmon drivers (`k10temp`, `coretemp`, `nct6775`…)
publish CPU and motherboard temperatures to every program, and OpenDeviceControl reads
them through `sysinfo`. Windows has no equivalent: the CPU's digital thermal
sensor sits behind model-specific registers (Intel) or the SMN bus (AMD),
both reachable only from kernel mode, and ACPI thermal zones are missing or
meaningless on most desktop boards ("not supported" on an AMD Ryzen 7
7800X3D, for example). So This PC's `cpu_temp` needs one of the sources below.
With none of them OpenDeviceControl behaves exactly as before: no CPU temperature.

Code: `crates/opendevicecontrol-core/src/system/external.rs` (decoders and parsers,
tested on every platform) and `system/windows.rs` (readers). Every sensor of
HWiNFO or LibreHardwareMonitor (temperatures, voltages, fans, clocks, loads,
data…) shows up in This PC's sensor tree; see [sensors.md](sensors.md).

## Sources, in priority order

`cpu_temp` is the hottest CPU temperature of the **first** source that has
any (sources don't get mixed). All other readings appear in the sensor tree
as detail sensors (sent to Home Assistant only when picked). Results are
cached for 1.5 s, because This PC and the fan-curve "system" source both
poll. HWiNFO and LibreHardwareMonitor see the same hardware, so only one is
read: HWiNFO when it shares its sensors.

| Source | What it gives | Requires |
|---|---|---|
| OS (`sysinfo`) | whatever Windows reports (usually nothing) | — |
| **OpenDeviceControl sensor service** | CPU temperatures, power, clocks and VIDs; the motherboard's Nuvoton / ITE / Fintek chip and ASUS EC sensors; memory module temperatures (through PawnIO); drive health (SMART / NVMe) | the service, set up by the installer |
| PawnIO in OpenDeviceControl itself | the same | **opt-in**, PawnIO installed, module file, OpenDeviceControl **as administrator** |
| HWiNFO | every sensor it shows | HWiNFO running, *Shared Memory Support* on |
| LibreHardwareMonitor (web server) | every sensor it shows (`data.json`) | LHM running, *Remote Web Server* on |
| LibreHardwareMonitor (WMI) | same (`Hardware` and `Sensor` classes) | LHM ≤ 0.9.4 running (it registers the namespace) |

### OpenDeviceControl sensor service

OpenDeviceControl runs without administrator rights, but PawnIO only lets
administrators in. So the installer (task *Install the hardware sensor
service*, on by default) sets up `opendevicecontrol-sensors.exe` as a Windows service
running as LocalSystem:

* `opendevicecontrol-sensors --install` (elevated; the installer and *Settings →
  Hardware sensors → Install sensor service…* run it with a UAC prompt)
  copies itself and the PawnIO modules to `%ProgramFiles%\OpenDeviceControl\Sensors`
  (only administrators can change that folder, which matters for a program
  running as LocalSystem), runs the bundled `PawnIO_setup.exe -install
  -silent` when PawnIO is missing, and registers and starts the service
  `OpenDeviceControlSensors` (automatic start). Nothing the service runs is
  used from OpenDeviceControl's own folder, which a per-user install lets
  the user change: the PawnIO installer is copied under `%ProgramFiles%` and
  checked against its pinned SHA-256 before it runs, `%ProgramFiles%` comes
  from the machine's registry rather than the environment, and
  `PawnIOLib.dll` is only loaded from PawnIO's own folder.
* An update is copied next to the installed one first
  (`Sensors.new`). Only then is the running service stopped (it has to
  really stop), the folders swapped (the old one kept as `Sensors.old`),
  and the new version started. It counts as installed once the service is
  running and answers on its pipe as the new version; otherwise the
  previous version is put back and started again.
* The service reads the sensors through PawnIO and serves them on the local
  pipe `\\.\pipe\opendevicecontrol-sensors`: a client connects, gets one JSON
  snapshot, and the pipe closes. It takes no commands. Signed-in users may
  read it; remote clients are rejected. A client gets 3 s to take its
  snapshot and a reader waits at most 3 s for one, so neither side can hold
  the other up; OpenDeviceControl reads it without holding its sensor cache,
  so a slow source never blocks This PC or device commands.
* OpenDeviceControl reads the pipe on every poll (looking for the service again every
  20 s when it isn't there) and uses it before everything else; with the
  service running, the in-process PawnIO option isn't needed.
* `opendevicecontrol-sensors --status` prints what the service reads;
  `--uninstall` (elevated; the uninstaller runs it) removes the service and
  its folder. PawnIO stays installed, since other tools may use it; it has
  its own uninstaller.
* Log: `%ProgramFiles%\OpenDeviceControl\Sensors\sensors.log`.

#### Motherboard chip (Super I/O)

The service finds the chip through PawnIO's `LpcIO` module (config ports
0x2E and 0x4E: enter with 0x87 0x87, chip id and revision in registers
0x20/0x21, exit with 0xAA), reads the hardware monitor's I/O base from
logical device 0x0B (register 0x60, read twice), clears the I/O-space lock
(register 0x28 bit 4) when a board sets it, and checks Nuvoton's vendor id
(0x5CA3) before using it. Registers are banked: bank number to index 0x4E,
then the register, through base + 5 / base + 6. Every access holds
`Global\Access_ISABUS.HTP.Method`, the lock LibreHardwareMonitor and
others use for the same chip.

Supported, with the register layouts of LibreHardwareMonitor's
`Nct677X.cs` and `IT87XX.cs` (MPL-2.0):

* **Nuvoton** NCT6779D, NCT6791D, NCT6792D(-A), NCT6793D, NCT6795D,
  NCT6796D(-R), NCT6797D, NCT6798D and NCT6799D (most ASUS, ASRock and
  MSI boards), and the NCT6683D / NCT6686D / NCT6687D embedded
  controllers of MSI boards (read through their page / index / data
  window at base + 4 / 5 / 6; read-only, OpenDeviceControl doesn't set up their fan
  control the way LibreHardwareMonitor does).
* **Fintek** F71808E, F71858, F71862, F71869(A), F71878AD, F71882 and
  F71889AD/ED/F (vendor id 0x1934 in register 0x23).
* **ITE** IT8613E, IT8620E, IT8625E, IT8628E, IT8655E, IT8665E, IT8686E,
  IT8688E, IT8689E, IT8728F and IT87952E (Gigabyte, some ASRock), both
  chips on boards that have two. ITE: enter with 0x87 0x01 0x55 0x55
  (0x2E) or 0x87 0x01 0x55 0xAA (0x4E), leave by writing 0x02 to
  register 0x02; environment controller at logical device 0x04; vendor
  id 0x90 / 0x7F in register 0x58. Voltages at 0x20–0x28, 0x2F, 0x2C–0x2E
  (10.9–12 mV per step by chip, shown as VIN0…), temperatures from 0x29
  (whole degrees), 16-bit fan counts (1.35 MHz / 2 / count) and fixed fan
  duties (automatic mode has none to show).

Not supported yet: NCT6701D, the NCT6687D-R of 2025 MSI boards, older
Winbond chips and boards whose sensors sit behind other embedded
controllers.

#### ASUS embedded-controller sensors

Many ASUS boards have sensors the Super I/O chip doesn't see: chipset,
VRM, T_Sensor header, water in / out, CPU_OPT and chipset fans, water
flow, CPU current and voltage. Which ones, and where, depends on the board:
OpenDeviceControl uses LibreHardwareMonitor's table of about 50 boards (ROG,
TUF, ProArt, Prime; AMD 400–800 and Intel 100–800 series), matched by the
BIOS's board name. They are read through PawnIO's `LpcACPIEC` module (the
ACPI EC ports 0x62 / 0x66: command 0x80 read, 0x81 write, bank in EC
register 0xFF), holding `Global\Access_EC`.

Nuvoton details:

* 16 voltage inputs at 0x480–0x48F, 8 mV per step; AVSB, 3VCC, 3VSB and
  VBAT have the chip's internal 1:2 divider. Inputs on external dividers
  (+12V, +5V on most boards) are shown as their raw pin (VIN0…VIN9),
  since the divider depends on the board. VBAT only when its monitoring is
  enabled (0x5D bit 0).
* Temperatures by source (SYSTIN, CPUTIN, AUXTIN0–4, TSENSOR, SMBus
  master, PECI…), each once, in 0.5 °C steps; inputs reading exactly 0 °C
  are unused and left out.
* Seven fan inputs (13-bit count, 1.35 MHz / count) with their PWM duty;
  a fan shows up once it has turned.

Tested on an ASRock B650E PG Riptide WiFi (NCT6799D at 0x290) against
LibreHardwareMonitor 0.9.6 at the same moment: the same voltages (to the
mV), temperatures and fan speeds, except Vcore, where LibreHardwareMonitor
applies a board-specific factor.

#### CPU power, clocks and voltages

Read on each physical core in turn (the reading thread is pinned to the
core's first logical processor) through the `AMDFamily17` / `IntelMSR`
modules, formulas as in LibreHardwareMonitor's `Amd17Cpu.cs` /
`IntelCpu.cs`:

* **AMD** (Zen 1–5): package and per-core power from the energy counters
  (`0xC001029B`, `0xC001029A`, unit from `0xC0010299`), per-core clock
  while running (P0 clock × ΔAPERF / ΔMPERF) and effective clock (ΔAPERF
  per second, idle included), per-core VID up to Zen 3 (`0xC0010293`,
  1.55 V − 6.25 mV × VID; Zen 4 and 5 encode it differently, so it is
  left out).
* **Intel**: package, cores, integrated graphics (PP1) and memory power
  (RAPL), per-core
  temperatures (`IA32_THERM_STATUS`), clocks as above, VID
  (`IA32_PERF_STATUS`).

#### Memory module temperatures

Over the chipset SMBus (PawnIO's `SmbusPIIX4` / `SmbusI801` modules, the
same as RAM lighting, holding `Global\Access_SMBUS.HTP.Method`): DDR5 SPD
hubs (SPD5118 at 0x50–0x57, MR49/MR50) and DDR4 thermal sensors (JC-42.4
at 0x18–0x1F). Read-only.

#### Drive health

No driver needed, but administrator rights: the NVMe SMART / health log
(`IOCTL_STORAGE_QUERY_PROPERTY`, protocol-specific property) gives
temperatures, life remaining, available spare, data read / written and
power-on time; SATA drives give temperature, life remaining (SSD wear
attributes 231 / 177 / 202 / 233) and power-on hours through
`SMART_RCV_DRIVE_DATA`. Read every 30 s.

#### GPUs other than NVIDIA

OpenDeviceControl itself (no service needed) reads every non-NVIDIA GPU's load and
memory use from Windows' GPU performance counters, as Task Manager shows
them. AMD GPUs add temperatures (edge, hot spot, memory, VRM), power,
clocks, fan and voltages from the AMD Display Library (`atiadlxx.dll`,
part of every AMD driver; Overdrive 8 PMLog, or Overdrive N on older
cards), loaded at runtime like NVIDIA's NVML. Intel's integrated graphics
power comes from the sensor service (PP1 energy counter).

#### Laptop batteries

OpenDeviceControl itself reads them through the battery class driver's IOCTLs
(`IOCTL_BATTERY_QUERY_STATUS` / `_INFORMATION`): charge level, charging,
charge / discharge rate, voltage, wear level and temperature, with the
same ids as on Linux.

The installer bundles PawnIO 2.2.0 and the PawnIO.Modules 0.2.11 release
(fetched with pinned SHA-256 by `packaging\windows\fetch-pawnio.ps1`).

### HWiNFO shared memory

HWiNFO publishes its sensors in the file mapping
`Global\HWiNFO_SENS_SM2` when *Settings → Shared Memory Support* is enabled
(the free version turns it off again after 12 hours). OpenDeviceControl maps it
read-only, copies it while holding `Global\HWiNFO_SM2_MUTEX` (when it can
open it), and parses the layout of HWiNFO's `hwisenssm2.h` (version 1 and 2
elements; the element sizes in the header are the stride). Every HWiNFO
"sensor" becomes a node, classified by its original English name (`CPU [`,
`GPU [`, Super I/O / EC chips such as Nuvoton or ITE, `S.M.A.R.T.`, DIMMs,
network, battery); the extra sensors HWiNFO lists per CPU or GPU
(`CPU [#0]: Ryzen 7 7800X3D: Enhanced`) become children of the first. Readings
of type temperature, voltage, fan, current, power, clock and usage are used,
and "other" readings with a data, rate, percent, power or flow unit; `°F`
readings are converted. No privileges are needed.

### LibreHardwareMonitor

LibreHardwareMonitor dropped its WMI provider in 0.9.5 (December 2025, with
the .NET 10 build), so current versions are read through their web server:
`GET http://127.0.0.1:<port>/data.json` (HTTP/1.0, 200 ms connect timeout),
walking the tree of hardware nodes (`HardwareId`, e.g. `/amdcpu/0`,
`/lpc/nct6799d/0` under `/motherboard`) and sensors (`SensorId`, `Type`,
and `RawValue` — a number or "4749312.0 B/s" in the base unit — or the
formatted `Value` on versions without `RawValue`). Types Temperature,
Voltage, Current, Power, Clock, Load, Level, Control, Fan, Flow, Data,
SmallData, Throughput and TimeSpan are used (Factor, Timing and Noise are
not). Turn it on under
*Options → Remote Web Server → Run*; the port (default 8085) is OpenDeviceControl's
*Settings → Hardware sensors* setting (`[sensors] lhm_port`). Web-server
authentication is not supported, and the server must listen on localhost
(its default). Older versions (≤ 0.9.4) are read from WMI:
`SELECT Identifier, Name, Parent FROM Hardware` and
`SELECT Identifier, Name, SensorType, Value, Parent FROM Sensor`
in `root\LibreHardwareMonitor`. When a source isn't there it is retried
every 20 s (WMI: 60 s).

### PawnIO (opt-in)

[PawnIO](https://pawnio.eu) is the signed kernel driver that current
LibreHardwareMonitor, FanControl and OpenRGB use instead of the vulnerable,
now blocked WinRing0 (which OpenDeviceControl never uses). It runs small signed
modules that only allow specific operations. OpenDeviceControl uses two of them,
read-only:

- **`AMDFamily17.bin`** (AMD family 17h–1Ah: Ryzen, Threadripper, EPYC):
  `ioctl_read_smn` reads SMN `0x00059800` (`THM_TCON_CUR_TMP`): Tctl =
  `(reg >> 21) × 0.125 °C`, minus 49 °C if bit 19 or both bits 17:16 are set,
  minus the Tctl offset of some first/second generation parts
  (1600X/1700X/1800X +20, 2700X +10, Threadripper 1900/2900 +27). Per-CCD
  temperatures are read from `0x00059800 + offset + 4 × ccd`
  (offset `0x154`, `0x300` or `0x308` by family/model), valid when bit 11 is
  set, `(reg & 0x7ff) × 0.125 − 49 °C`. All from Linux `k10temp`. Each read
  holds `Global\Access_PCI`, as the module requires.
- **`IntelMSR.bin`**: `ioctl_read_msr` of `IA32_PACKAGE_THERM_STATUS`
  (`0x1B1`) and `MSR_TEMPERATURE_TARGET` (`0x1A2`): package =
  TjMax − bits 22:16, TjMax from bits 23:16 (100 °C if zero).

Requirements:

1. PawnIO installed (`%ProgramFiles%\PawnIO\PawnIOLib.dll`), its service
   running.
2. The signed module from the
   [PawnIO.Modules](https://github.com/namazso/PawnIO.Modules) release next
   to `opendevicecontrol.exe`, in `<exe dir>\pawnio\`, or in `%APPDATA%\opendevicecontrol\pawnio\`
   (the same places as the SMBus modules for RAM lighting).
3. OpenDeviceControl running **as administrator**: PawnIO only lets administrators open
   it. OpenDeviceControl itself runs unprivileged by default, which is why this is
   opt-in (*Settings → Hardware sensors → Read the CPU temperature through
   PawnIO*, or `[sensors] pawnio = true` in `config.toml`).

If any of this is missing, Settings and `opendevicecontrol --probe` say which part,
and OpenDeviceControl falls back to HWiNFO / LibreHardwareMonitor.

## Tested

On Windows 11, AMD Ryzen 7 7800X3D (family 19h model 61h), ASUS board with
a Nuvoton NCT6799D:

- **LibreHardwareMonitor 0.9.6** (winget), web server on: `cpu_temp` from
  *Core (Tctl/Tdie)*, plus *CCD1 (Tdie)* and the NCT6799D's six
  temperatures. Unconnected Super I/O inputs show up too (8–20 °C here);
  they are diagnostic sensors.
- **PawnIO 2.2.0** (winget) with `AMDFamily17.bin` from PawnIO.Modules
  0.2.11, elevated: Tctl 57.25 °C against LHM's 57.4 °C read at the same
  moment; exactly one CCD found. Not elevated, it reports
  "PawnIO refused access: OpenDeviceControl must run as administrator".
- Not tested on hardware: HWiNFO (parser tested with a synthetic mapping),
  LHM ≤ 0.9.4 WMI, Intel.

LHM's web server listens on every interface unless *listenerIp* is set; to
keep it local, set `<add key="listenerIp" value="127.0.0.1" />` in
`LibreHardwareMonitor.config` (or pick it under *Options → Remote Web
Server → Interface*).

## Checking

`opendevicecontrol --probe` prints each source's status and This PC's temperatures
(`cargo run -p opendevicecontrol-core --example thispc` prints the whole tree):

```
Sensor source: PawnIO: off
Sensor source: LibreHardwareMonitor web server (283 sensors): connected
This PC CPU temperature                      61.5 °C  (cpu_temp)
This PC Core (Tctl/Tdie)                     61.5 °C  (lhm_amdcpu_0_temperature_2)
This PC CCD1 (Tdie)                          58.0 °C  (lhm_amdcpu_0_temperature_3)
This PC Temperature #1                       35.0 °C  (lhm_lpc_nct6799d_0_temperature_0)
```
