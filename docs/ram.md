# Corsair RGB memory (SMBus) — design and safety

Status: **experimental, untested on hardware.** Disabled by default.

This document covers Corsair RGB DIMMs (Vengeance RGB, Vengeance RGB PRO /
PRO SL / RT / RS, Vengeance RGB DDR5, Dominator Platinum RGB DDR4/DDR5,
Dominator Titanium RGB DDR5) and the DIMM temperature sensors. Unlike every
other device opendevicecontrol supports, these are not USB devices: they sit on the
motherboard's **SMBus**, the same two-wire bus that carries the memory's SPD
EEPROM (the chip the BIOS reads the memory timings from).

The protocol knowledge comes from:

- [OpenRGB](https://gitlab.com/CalcProgrammer1/OpenRGB) (GPL-2.0-or-later):
  `Controllers/CorsairDRAMController` (Adam Honse, Erik Gilling),
  `Controllers/CorsairVengeanceController`, `SPDAccessor/*`, and its SMBus
  layer `i2c_smbus/Linux/i2c_smbus_linux.cpp` and
  `i2c_smbus/Windows/i2c_smbus_pawnio.cpp`. Checked against commit
  `1efdbb9` (2026-11-11).
- [OpenLinkHub](https://github.com/jurkovic-nikola/OpenLinkHub)
  (GPL-3.0-or-later): `src/devices/memory/memory.go`, `src/smbus/smbus.go`.
- The Linux kernel's `jc42` (DDR4 TSE2004av thermal sensor) and `spd5118`
  (DDR5 SPD hub) hwmon drivers, for the temperature register formats.

## 1. Why this is risky, and the rules we follow

The SMBus is shared by the SPD EEPROMs, the DIMM thermal sensors, often the
clock generator and voltage regulators, and sometimes vendor chips. A stray
write can:

- **corrupt an SPD EEPROM** (the module then fails POST or trains at wrong
  timings; recovering needs an external programmer), or flip its write
  protection;
- change the state of a DDR4 SPD *page* (writes to 0x36/0x37), confusing the
  BIOS and other tools (memtest, HWiNFO) until the next power cycle;
- reconfigure a thermal sensor or regulator.

opendevicecontrol therefore follows these rules, enforced in code, not just by
convention:

1. **Opt-in.** Nothing touches the SMBus unless *Settings → Memory (RAM)
   lighting* is enabled (`[ram] enabled = true` in `config.toml`). It is off
   by default, and the checkbox carries a warning.
2. **Never write to SPD space.** The SMBus layer (`smbus::Guarded`) has a
   hard deny-list for writes to `0x30–0x37` (DDR4 SPD page/write-protect
   commands) and `0x50–0x57` (SPD EEPROMs / DDR5 SPD hubs). No code path,
   driver bug or allow-list mistake can write there. opendevicecontrol never switches
   SPD pages, never enables a disabled DDR5 thermal sensor, and never writes
   the DDR5 hub's page register (OpenRGB does, in `DDR5DirectAccessor`; we
   don't need the SPD contents at all).
3. **Writes only to positively identified controllers.** Every other address
   is read-only too, until the RAM driver has identified a Corsair
   controller there and explicitly allows writes to that single address.
   Identification = the reads in §3, *all* of which must match.
4. **Only the address ranges and write sequences of the references.**
   Addresses `0x58–0x5F` (DDR4) and `0x18–0x1F` (DDR5), registers and
   sequences exactly as OpenRGB writes them. Where we deviate, it is to do
   *less* (listed in §3.5).
5. **Only the chipset SMBus controllers.** Like OpenRGB's `IF_DRAM_SMBUS`
   macro, we only look at Intel i801-family and AMD FCH (PIIX4) SMBus
   controllers, identified by PCI vendor/device ID — never GPU DDC/I²C,
   display, or other buses.
6. **Volatile only.** We use the controllers' *direct* (streaming) colour
   mode only. We never write the effect configuration (register `0x82`),
   which the controller saves internally.
7. **Read-only probes are minimal.** No SMBus "quick write" probes (they are
   known to corrupt some Atmel AT24RF08 EEPROMs; OpenRGB uses one for the
   original Vengeance RGB, we use plain reads instead).

## 2. Hardware model

Each DIMM slot `n` (0–7) has fixed SMBus addresses:

| What | DDR4 | DDR5 |
|---|---|---|
| SPD EEPROM / SPD5 hub | `0x50 + n` (EE1004) | `0x50 + n` (SPD5118 hub) |
| Thermal sensor | `0x18 + n` (TSE2004av, "TS") | inside the hub at `0x50 + n` (MR49/50) |
| Corsair RGB controller | `0x58 + n` | `0x18 + n` |

The DDR5 RGB controller sits where DDR4's thermal sensor would be, because
JEDEC moved the DDR5 sensor into the hub. OpenLinkHub picks the range from the
memory type; OpenRGB probes both ranges. **We pick the range by memory type
(like OpenLinkHub) and only probe slots whose SPD answers**, so on a DDR4
system nothing is ever sent to the DDR4 thermal sensors' addresses except
the read-only temperature reads below.

### Memory type detection (read-only)

For each slot, read byte `0x00` and `0x01` at `0x50 + n`:

- `0x51` and `(b1 & 0xEF) == 0x08` → **DDR5** SPD hub (MR0/MR1 device type,
  same test as OpenRGB's `DDR5DirectAccessor::isAvailable`, minus its page
  write).
- any other successful read → **DDR4** (or older) SPD present.
- NACK / error → empty slot.

On Linux, if a kernel driver (`ee1004`, `spd5118`, `jc42`, `at24`) is bound
to the address, i2c-dev refuses access (`EBUSY`). We then take the driver
name from `/sys/bus/i2c/devices/<bus>-00<addr>/name`: `spd5118` → DDR5,
`ee1004` → DDR4, anything else → present, type unknown (treated as DDR4).

## 3. Protocols

### 3.1 Corsair DRAM controller (Vengeance RGB PRO / PRO SL / RT / RS, Vengeance RGB DDR5, Dominator Platinum/Titanium RGB)

From OpenRGB `CorsairDRAMController` (rewritten 2026, superseding the old
`CorsairVengeancePro` / `CorsairDominatorPlatinum` controllers) and
OpenLinkHub.

**Identify** (read-only), at the slot's RGB address:

1. read byte `0x43` → must be `0x1A`, `0x1B` or `0x1C`;
2. read byte `0x44` → must be `0x01`, `0x03` or `0x04`.

**Device info** (only after step 1–2 matched; these two writes are the first
writes the address ever gets, exactly as OpenRGB `ReadDeviceInfo`):

1. write byte `0x61` ← `0x00` (select device-info buffer);
2. write byte `0x21` ← `0x00` (start binary transfer);
3. read byte `0x40` × 32 (buffer bytes);
4. read byte `0x42` → CRC-8 of the 32 bytes (poly `0x07`, init `0x00`).

Buffer layout: `[0..2]` VID (LE, `0x1B1C`), `[2..4]` PID (LE), `[8]` minor,
`[9]` major, `[10..12]` build → firmware `major.minor.build`; `[28]`
protocol version.

**We require** (OpenRGB only logs a CRC mismatch and falls back to the
first device in its table): CRC match, VID `0x1B1C`, and a PID in the table
below. Otherwise the address is not used for anything further.

| PID | Model | LEDs | Reversed |
|---|---|---|---|
| `0x0100`, `0x0101` | Vengeance RGB PRO DDR4 | 10 | no |
| `0x0200`, `0x0201` | Dominator Platinum RGB DDR4 | 12 | yes |
| `0x0300`, `0x0301` | Vengeance RGB PRO SL DDR4 | 10 | no |
| `0x0400`, `0x0401` | Vengeance RGB RS DDR4 | 6 | no |
| `0x0600`, `0x0601` | Dominator Platinum RGB DDR5 | 12 | yes |
| `0x0700`, `0x0701`, `0x0900`, `0x0901`, `0x0910`, `0x0911` | Vengeance RGB DDR5 | 10 | no |
| `0x0800`, `0x0801`, `0x0810`, `0x0811` | Dominator Titanium RGB DDR5 | 12 | yes |
| `0x0A00`, `0x0A01`, `0x0A10`, `0x0A11` | Vengeance Shugo Series DDR5 | 10 | no |
| `0x0B00`, `0x0B01` | Vengeance RGB RS DDR5 | 6 | no |

**Direct colour mode** (protocol version ≥ 4 only; OpenRGB refuses direct
mode below that, so do we — such modules get temperature only):

```
packet = [led_count, R0, G0, B0, R1, G1, B1, …, crc8(packet[0 .. n-1])]
```

- SMBus *block write* of `packet[0..min(32, n)]` to register `0x31`;
- if `n > 32` (12-LED modules: 38 bytes), block write of the rest to `0x32`.

"Reversed" modules take LED colours in reverse order (OpenRGB's `reverse`
flag), so that LED 0 in opendevicecontrol is the same physical end on every model.

**Fallback when the controller cannot do block writes** (e.g. some i2c
adapters): the packet is sent as SMBus *word writes*, 5 bytes per pair:
bytes `p0 p1 p2 p3 p4` → write word `(p1<<8)|p0` to register
`0x90|(p2 & 0x0F)` (first chunk) or `0xA0|(p2 & 0x0F)` (later chunks), then
word `(p4<<8)|p3` to register `0xA0|(p2 >> 4)`. Exactly as OpenRGB.

### 3.2 Original Vengeance RGB (DDR4, single zone)

From OpenRGB `CorsairVengeanceController`.

**Identify** (read-only): at `0x58 + n` (DDR4 slots only), read bytes
`0xA0..0xAF`; **all 16** must read `0xBA`.

**Set colour** (one LED for the whole module), five byte writes:
`0xA4` ← `0x00` (fade time 0 = static), `0xB0` ← R, `0xB1` ← G,
`0xB2` ← B, `0xA6` ← `0x00` (mode: single colour). These registers are
volatile.

### 3.3 Temperatures (read-only)

- **DDR5**: SPD5118 hub at `0x50 + n`. Read MR26 (`0x1A`); if bit 0 (TS
  disable) is set the sensor is off and we leave it off. Otherwise read MR49
  (`0x31`, LSB) and MR50 (`0x32`, MSB): `t = sign_extend11((raw >> 2) &
  0x7FF) × 0.25 °C` (kernel `spd5118_temp_from_reg`). These are hub
  registers (address bit 7 clear) and need no page switching.
- **DDR4**: TSE2004av at `0x18 + n`. Read words `0x00` (capabilities) and
  `0x01` (configuration) (big-endian on the wire). Accept only if
  `cap & 0xFF00 == 0` and `config & 0xF800 == 0` (kernel `jc42_detect`).
  Temperature: word `0x05`, `t = sign_extend13(raw & 0x1FFF) × 0.0625 °C`.
- **Linux, kernel driver bound** (`jc42`, `spd5118`): we read the kernel's
  hwmon instead: `/sys/bus/i2c/devices/<bus>-00<addr>/hwmon/hwmon*/temp1_input`
  (what OpenLinkHub does when `ramTempViaHwmon` is set).

Temperatures are only reported for slots that hold an identified Corsair
RGB module (the device *is* the Corsair memory kit).

### 3.4 Bus access pattern

- Detection runs once when RAM support is enabled (and again on *Rescan
  devices*); there is no periodic probing — DIMMs are not hot-pluggable.
- 10 ms pause between probed addresses (as OpenRGB).
- Animated effects send ~30 frames/s. Each DIMM costs one (two for 12-LED
  modules) 32-byte block write, ≈3–4 ms at 100 kHz; frames identical to the
  previous one for a DIMM are skipped (as OpenLinkHub does).
- Temperatures are read at the normal poll interval (default 2 s).

### 3.5 Deliberate deviations from OpenRGB (all are "do less")

| OpenRGB | opendevicecontrol | Why |
|---|---|---|
| Probes `0x58–0x5F` *and* `0x18–0x1F` on every system | Only the range of the detected memory type, only populated slots | Never poke DDR4 thermal sensors with writes |
| Unknown PID → treated as Vengeance PRO | Unknown PID / bad CRC / wrong VID → ignored | Positive identification |
| Original Vengeance: `write_quick` probe, SPD manufacturer check via SPD page writes | Plain reads of the 16-byte `0xBA` signature + SPD presence | No quick-write, no SPD page writes |
| First block write always 32 bytes, even for 6-LED modules (reads past its buffer) | Writes exactly the packet length | Correctness |
| Writes effect configuration (`0x82`), saved in the module | Direct mode only | Volatile; no persistent state |
| DDR5 SPD page register written during detection | Never | SPD safety |

### 3.6 Exit behaviour

The controllers have no "return to hardware effect" command in direct mode
that the references know of. On exit opendevicecontrol simply stops sending frames;
the modules keep showing the last frame until the PC is power-cycled, after
which they run the effect stored in them (by iCUE or OpenRGB). opendevicecontrol does
not change that stored effect.

## 4. Device model in opendevicecontrol

**One `Device` per SMBus controller ("Corsair memory"), with one light zone
and one temperature sensor per DIMM**, rather than one device per DIMM:

- All DIMMs are on one bus. One device = one worker thread = one owner of
  the bus handle, so transfers are naturally serialised (no locking between
  workers) and an effect frame for all sticks goes out back-to-back.
- Effects flow across the whole kit (rainbow wave over four sticks), with one
  lighting setting and one Home Assistant `light` entity — what users expect
  from a matched RAM kit, and what iCUE shows.
- A kit is bought, installed and removed as a unit; per-DIMM firmware
  versions and models are still shown in the zone names / log.

Zone ids are `dimm1`…`dimm8` (slot number + 1; the slot numbering is the
SMBus address order, which usually but not always matches the board's
A1/A2/B1/B2 labels). Sensors: `dimm<N>_temperature`.

The device id is `memory` (`memory-2` for a second SMBus, which is
unusual). Its "path" for the service's bookkeeping is `smbus:<bus name>`.

## 5. Linux

### Requirements

- The `i2c-dev` module loaded (it exposes `/dev/i2c-*`). Pop!_OS/Ubuntu do
  not load it by default.
- The SMBus controller driver: `i2c-i801` (Intel) or `i2c-piix4` (AMD).
  Both load automatically on almost all systems.
- Read/write access to the SMBus's `/dev/i2c-N` for the logged-in user.

### Packaging (proposed, included in this change)

`packaging/linux/modules-load.d/opendevicecontrol-i2c.conf` (installed to
`/usr/lib/modules-load.d/`):

```
i2c-dev
```

and in `60-opendevicecontrol.rules`, give the logged-in user access **only to the
chipset SMBus adapters**, not every I²C bus (GPU, display DDC…):

```
# Corsair RGB memory (only used when enabled in Settings): chipset SMBus only.
SUBSYSTEM=="i2c-dev", ATTRS{name}=="SMBus I801 adapter*", TAG+="uaccess"
SUBSYSTEM=="i2c-dev", ATTRS{name}=="SMBus PIIX4 adapter*", TAG+="uaccess"
```

`uaccess` grants access to the user at the active local seat only (same
mechanism as the existing hidraw rule), not to remote/SSH users. The .deb
`postinst` runs `modprobe i2c-dev` so it works without a reboot.

**Security note:** this gives the desktop user raw SMBus access, the same as
membership in the `i2c` group that OpenRGB's instructions recommend. A user
process could write SPD EEPROMs (unless the BIOS write-protects them) or
regulators. That is why the rule is restricted to the chipset adapters and
why we document it rather than hide it; users who don't want RAM lighting
can delete those two lines.

### Caveats (honestly)

- **ACPI resource conflicts.** On many boards the BIOS declares the SMBus
  I/O ports as ACPI-owned; the kernel then refuses to bind `i2c-piix4` /
  `i2c-i801` (`dmesg`: *"ACPI: resource … conflicts with OpRegion"*). The
  usual workaround is the kernel parameter `acpi_enforce_resources=lax`.
  That lets Linux drivers *and* firmware (ACPI AML, SMM) use the same
  controller without coordination; collisions can garble transfers. It is a
  system-wide decision we do **not** make for the user; opendevicecontrol only
  explains it when no SMBus adapter is found.
- **Kernel drivers own some addresses.** `ee1004` / `spd5118` / `jc42` bind
  to the SPD and sensor addresses on newer kernels. That is fine: we never
  write those addresses, and we read temperatures through their hwmon
  interface instead.
- **AMD (piix4)**: the DIMMs are on port 0 (`SMBus PIIX4 adapter port 0`).
  We scan every port of a matching controller, as OpenRGB does; detection
  is read-only until a controller is identified.
- **Bus sharing**: other tools (OpenRGB, OpenLinkHub, `sensors`) may use the
  same bus. The kernel serialises individual transfers, but two RGB programs
  will fight over the colours. Run one.

## 6. Windows

Windows has no user-mode SMBus API. Access needs a kernel driver that maps
the SMBus controller's I/O ports.

| Option | Status | Verdict |
|---|---|---|
| **WinRing0** (`WinRing0x64.sys`, used by old OpenRGB, FanControl, LibreHardwareMonitor) | Unmaintained since 2010s. **CVE-2020-14979**: any local user can read/write arbitrary physical memory, I/O ports and MSRs → trivial privilege escalation. On Microsoft's **vulnerable driver blocklist**; **Microsoft Defender flags/quarantines it** (`HackTool:Win32/Winring0`, 2025). | **Never.** Not added. |
| **inpoutx64** | Same design flaw: raw I/O-port and physical-memory access for any caller of the device; a classic "bring your own vulnerable driver" target. | **No.** |
| **PawnIO** (namazso; used by current OpenRGB, LibreHardwareMonitor, FanControl) | Signed, maintained kernel driver that only runs small **signed modules** (e.g. `SmbusI801.bin`, `SmbusPIIX4.bin`) exposing narrow ioctls such as `ioctl_smbus_xfer`, instead of raw port/memory access. Installed separately from <https://pawnio.eu> (or winget), which needs admin once. Its device is **restricted to administrators**: an unelevated process gets `E_ACCESSDENIED` from `pawnio_open`. | **Recommended.** |

### What opendevicecontrol implements

`smbus::pawnio` — a PawnIO backend, loaded dynamically (no build- or
run-time dependency when RAM support is off):

- loads `PawnIOLib.dll` from the PawnIO install directory
  (`%ProgramFiles%\PawnIO\`), falling back to the DLL search path;
- loads the SMBus modules `SmbusI801.bin` / `SmbusPIIX4.bin` from
  `<opendevicecontrol.exe dir>\pawnio\` or `%APPDATA%\opendevicecontrol\pawnio\` (the signed
  binaries from the PawnIO.Modules release; we don't build them);
- identifies the controller (`ioctl_identity` → PCI IDs, same allow-list as
  Linux), selects PIIX4 port 0/1 (`ioctl_piix4_port_sel`);
- every transfer (`ioctl_smbus_xfer`) holds the system-wide mutex
  `Global\Access_SMBUS.HTP.Method`, the convention shared by HWiNFO,
  AIDA64, LibreHardwareMonitor and OpenRGB, so opendevicecontrol does not collide
  with monitoring tools;
- every error is reported in Settings in plain words: PawnIO not installed,
  module missing, **needs administrator**, no supported controller.

It is code-complete but **untested on hardware**, like the RAM drivers.

### What this means for the per-user, unprivileged install

- opendevicecontrol itself stays unprivileged; nothing changes for the PSU,
  keyboard and other USB devices.
- **RAM lighting on Windows requires (a) PawnIO installed (once, by an
  admin) and (b) opendevicecontrol running elevated.** Today that means starting
  opendevicecontrol "as administrator" (or an autostart scheduled task with "run with
  highest privileges"). We don't do this automatically, and the installer
  stays per-user and admin-free.
- We do **not** bundle or silently install a kernel driver.

### Recommended next step (not implemented): an elevated helper service

Running the whole GUI elevated is a poor trade. The proper fix is a tiny
optional **Windows service** `opendevicecontrol-smbus` (installed only by an
"all users"/admin install option, runs as LocalSystem, talks to PawnIO) that
exposes a **narrow** named-pipe API to the unprivileged app:

- `list` → identified Corsair DIMMs (address, model, LED count) and temperatures;
- `set_colors(dimm, [rgb; n])` → builds the packet itself.

It must **never** offer raw SMBus reads/writes to the pipe client — that
would re-create the WinRing0 privilege-escalation hole for any local user.
The pipe ACL allows interactive users; the service enforces the same
identification and deny-list rules as the in-process code (they live in
`opendevicecontrol-core` and would be shared). With that, the per-user installer
remains admin-free for everything else, and RAM support becomes "install
PawnIO + tick *Install SMBus helper (admin)*".

## 7. Code map

```
crates/opendevicecontrol-core/src/smbus/
  mod.rs      SmBus trait, Guarded (deny-list + per-address write allow-list),
              PCI allow-list, MockBus (tests), open_dram_buses()
  linux.rs    /dev/i2c-* via I2C_SLAVE / I2C_SMBUS / I2C_FUNCS ioctls; sysfs
              adapter discovery, PCI IDs, bound-driver names, hwmon temps
  pawnio.rs   Windows: PawnIOLib.dll + signed SMBus modules
crates/opendevicecontrol-core/src/devices/ram.rs
              CRC-8, packet builders, identification, CorsairMemory (Device),
              discover(); unit tests against MockBus
crates/opendevicecontrol/src/config.rs   [ram] enabled = false
crates/opendevicecontrol/src/service.rs  RAM scan only when enabled; once + on Rescan
crates/opendevicecontrol/src/gui.rs      Settings → "Memory (RAM) lighting" checkbox + warning + status
crates/opendevicecontrol/src/gui_peripheral.rs  "Sensors" section (DIMM temperatures)
crates/opendevicecontrol/src/main.rs     --probe also lists RAM when enabled in config.toml
```
