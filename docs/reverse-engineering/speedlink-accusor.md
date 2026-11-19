# Speedlink ACCUSOR keyboard

USB `264a:3008`, product string "Tt eSPORTS CHALLENGER Prime RGB" (the same
OEM board as the Thermaltake keyboard). Speedlink part SL-670005-BK. The
protocol was recovered from the official Windows driver (V1.0) by static
analysis, then tested on a real keyboard on Linux (hidraw). OpenDeviceControl's
driver is `crates/opendevicecontrol-core/src/vendors/speedlink/`.

"Verified in the code" below means read directly from the vendor binaries;
"on hardware" means tested on the keyboard. The section
[Hardware results](#hardware-results) lists what was tested and what came out
differently from the code.

Hardware, from the manuals: top row P1 P2 P3, three icon keys, then media keys
(play/pause, previous, next, volume down, volume up, mute); a left column of
six macro keys M1 to M6. The driver UI shows "PROFILE 1/3". The firmware has
an Fn layer (key response time, polling rate, brightness, breathing, colour
cycle, gaming mode, WASD/arrow swap and so on) that the host software never
touches.

The backlight is one colour for the whole board: only red and blue LEDs
(purple is both, and looks pinkish). There is no per-key or arbitrary RGB
colour despite the "RGB" in the product string, and no live/streaming
lighting command; `07` changes a profile's stored setting.

## Hardware results

Tested on an SL-670005-BK from Linux with the hidraw ioctls, 2026-11-17.

| What | Result |
|---|---|
| `81` + GetFeature | Works. Reply `00 81 03 ..`: report id, echo, active profile (here 3) |
| Hello `02 09 70 07` | Needed: before it the keyboard sends no `04` events at all; after it, every P-key press, Fn lighting change, Fn polling change and Fn+Win reports one |
| `07 pp 00 LL 00 CC 00` | Works and applies at once to the active profile (no `01` needed). `LL` 0..4 and `CC` 0 red, 1 blue, 2 purple confirmed by eye; 4 = breathing |
| Fn+Scroll Lock | A separate backlight on/off switch. It reports `04 06 03 00`, but a later `07` with `LL` > 0 does **not** turn the light back on while it is off; only Fn+Scroll Lock (or Fn+F9..F12) does |
| `0C` key table | Works. The default table with one entry changed changes only that key, only in that profile |
| Key positions | Mapped by uploading letters to every special entry and pressing the keys (table below) |
| Macros `10` | Work, but only when sent **after** the key table. Table then macro then `11` then `01` played; macro then table played nothing. Uploading a table seems to discard the macros |
| `11 00 RR 01` polling | No visible effect: the Fn+F3/F4 events kept counting from their own state after sending `RR` = 0 and 1. Not used by OpenDeviceControl |
| Win lock event | `04 13 00 01` locked / `04 13 00 00` unlocked, not `50 00` as Monitor's handler suggests |
| Media keys | Arrive as consumer reports (`03 lo hi`) on interface 1, like a normal keyboard; they are not `04` events |
| P1..P3 | Firmware profile keys: switch profile, flash the backlight, report `04 05 03 nn`. Nothing reaches the host as a key |
| Factory M keys | Each pressed only Ctrl on profiles 1 and 3 (the factory macro slots 0..5 seem to hold just that) |

Keys whose table entries are not plain HID usages, from the hardware test:

| Key | Index | Default entry |
|---|---|---|
| F1..F12 | 17, 25, 129, 49, 18, 26, 58, 66, 74, 98, 82, 90 | `12 01 00` .. `12 01 0B` |
| Print Screen | 83 | `12 01 0E` |
| Scroll Lock | 91 | `12 01 0C` |
| Pause | 92 | `12 01 0F` |
| Page Up / Page Down | 93 / 102 | `12 01 13` / `12 01 14` |
| Left Windows | 43 | `12 01 0D` |
| The three icon keys right of P3 | 138, 39, 44 | consumer My Computer (`194`), Mail (`18A`), WWW Home (`223`) |
| P1 / P2 / P3 | 114 / 122 / 130 | `05 00 03 01..03` |
| M1..M6 | 10, 22, 63, 106, 111, 135 | (code; M1 confirmed) |

The system-control entries (`02 00 01/02/04`, indices 2, 6, 7) and the
consumer entries at 13, 15, 19, 21, 23, 51, 140, 143 are not physical keys on
this board; they are probably what the Fn layer uses (Fn+F5..F8 are
calculator, refresh, back, forward).

## Software architecture

- `Option.exe` is the UI. It has no HID code at all. It reads and writes the
  profile files (`config\Default0..4.gkp`, `Config.ini`) and a 0x206E0-byte
  shared-memory block named `SOAI-862_Gaming_Keyboard_Share_Data` (guarded by
  the mutex `SOAI-862_Gaming_Keyboard_Mutex_Object`). On OK/Apply it fills the
  block, sets dirty flags in it, and posts window message `0x4C8` to the
  Monitor window ("ACCUSOR Advanced Gaming Keyboard Monitor"). `0x4CA` and
  `0x4CB` make Monitor re-read the block without touching the device.
- `Monitor.EXE` is the tray process and the only thing that talks to the
  keyboard, through `hiddriver.dll`. The `0x4C8` handler (`FUN_00404280`)
  copies the shared block and posts thread message `0x671` wParam 1 to a worker
  thread (`FUN_00406670`), which runs the "apply all" sequence below
  (`FUN_004057d0`).
- Firmware version shown in the UI is just `FWVersion=101` from `Config.ini`.
  There is no version read from the device.
- A software macro player thread (`FUN_00403e90`, thread message `0x670`)
  exists but nothing posts to it. Macros are played by the keyboard.

## Device access (hiddriver.dll)

`Set_VIDPID(0x264A, 0x3008)` is called with constants from Monitor
(`FUN_004043d0`). Device matching is in `hiddriver.dll` `0x10001c00` (VID, PID,
usage page, usage):

| Mode | Collection | Used for |
|---|---|---|
| 0 (`Open_DevMonitor`) | usage page `0xFF00`, usage `0x01` | Reader thread for event input reports (below) |
| 2 (`Open_ReportDevice`) | usage page `0xFF01`, usage `0x01` | All feature reports and 64-byte output reports |
| 1 | usage page `0xFFFF`, usage `0xFF` | Opened, handle stored, never used by these exports |

`Open_FeatureDevice` and `Close_FeatureDevice` are stubs that return 1.

- `SetFeature(buf, 8)`: `HidD_SetFeature` with a 9-byte buffer, report id 0 +
  8 bytes.
- `GetFeature(buf, 8)`: `HidD_GetFeature`, 9 bytes, returns bytes 1..8.
- `WriteUSB(buf, 64)`: `WriteFile` of 65 bytes, report id 0 + 64 bytes.
- `ReadUSB` is loaded by Monitor but never called.

On Linux this means: features via `HIDIOCSFEATURE`/`HIDIOCGFEATURE` with a
leading 0 byte (9 bytes total), output via `write()` of 65 bytes with a leading
0, all on the hidraw node of the `0xFF01` collection. Events arrive as input
reports on the `0xFF00` collection's hidraw node.

## Feature report format

Every feature command is 8 bytes. Byte 7 is a checksum:

```
b7 = ((b0 + b1 + b2 + b3 + b4 + b5 + b6) & 0xFF) ^ 0xAA
```

Verified in every builder (`FUN_00405720`, `FUN_004057d0`, `FUN_004061a0`,
`FUN_00406260`, `FUN_00405a40`, `FUN_00406670`, `FUN_004043d0`). The 64-byte
output reports carry raw data with no checksum. No reply or ack is read after
any write; the only `GetFeature` is the profile read.

## Commands

| Command (b0..b6) | Meaning | Where |
|---|---|---|
| `81 00 00 00 00 00 00` then GetFeature | Read active profile: reply byte 1 = profile 1..3 (1-based) | `FUN_00405720` |
| `01 pp 00 00 00 00 00` | Select active profile, `pp` = 1..3 | `FUN_00406670` (wParam 3), `FUN_004057d0` |
| `02 09 70 07 00 00 00` | Constant "hello". Sent once 1 s after the device is opened on start-up, and at the start of every apply. Meaning not known | `FUN_004043d0`, `FUN_004057d0` |
| `07 pp 00 LL 00 CC 00` | Lighting for profile `pp` (1..3). `LL` = level, `CC` = colour | `FUN_004061a0` |
| `0C pp 00 0A 00 00 00` + 10 x 64 bytes | Key table for profile `pp` (640 bytes) | `FUN_00406260` |
| `10 00 ii 02 00 00 00` + 2 x 64 bytes | Macro slot `ii` (128 bytes) | `FUN_00405a40` |
| `11 00 RR 01 00 00 00` | USB polling rate (global, not per profile) | `FUN_004057d0` |

Byte 3 of `0C` (`0x0A`) and of `10` (`0x02`) equals the number of 64-byte
output reports that follow, so it is probably a packet count.

Checked examples (b0..b7):

```
81 00 00 00 00 00 00 2B   read profile (then GetFeature)
01 01 00 00 00 00 00 A8   select profile 1      (02 -> A9, 03 -> AE)
02 09 70 07 00 00 00 28   hello
0C 01 00 0A 00 00 00 BD   key table, profile 1  (p2 -> B2, p3 -> B3)
10 00 06 02 00 00 00 B2   macro slot 6
11 00 00 01 00 00 00 B8   1000 Hz
```

### Lighting (`07`)

`LL` is the profile's `LtControl`, `CC` its `LtColor`:

| `LL` | UI |
|---|---|
| 0 | Off |
| 1 | 25 % |
| 2 | 50 % |
| 3 | 100 % (default) |
| 4 | Breathing |

| `CC` | Colour |
|---|---|
| 0 | Red (default) |
| 1 | Blue |
| 2 | Purple |

Monitor clamps `LL`: 0..3 pass through, anything else becomes 4. The UI
handler in Option.exe (`0x428fd0`) writes 0..4 into `LtControl`; the 0..4 to
button mapping above follows the button order on screen (Off, 25, 50, 100,
Breathing) and the default `LtControl=3`. The colour order follows the button
order Red, Blue, Purple and the event range 0..2. Both confirmed on
hardware.

The other lighting keys in the `.gkp` (`LtSwitch`, `LtEffectType`, `LtFlag`,
`LtSaving`, `LtTimeout`, `apmLightInfo`) are loaded by Monitor
(`FUN_00407650`) but never sent to the device. There is no power-saving or
timeout command.

Examples:

```
07 01 00 03 00 00 00 A1   profile 1, 100 %, red
07 01 00 02 00 01 00 A1   profile 1, 50 %, blue
07 01 00 04 00 02 00 A4   profile 1, breathing, purple
07 01 00 00 00 00 00 A2   profile 1, off
```

### Polling rate (`11`)

`RR` = `PollingRate` from `Config.ini` (one global value, `this+0x20758` in
Monitor). Option.exe writes 0..3 from the four buttons (`0x428de1` to
`0x428e58`). Mapping, from the button order and the default value 0:

| `RR` | Rate |
|---|---|
| 0 | 1000 Hz (default) |
| 1 | 500 Hz |
| 2 | 250 Hz |
| 3 | 125 Hz |

i.e. interval = 2^RR ms. Confidence: medium-high (the order is inferred from
the button layout, not from a string table).

On hardware the command was accepted but showed no effect that could be
measured: the endpoint interval stays 1 ms (the firmware throttles inside),
the keyboard did not re-enumerate, and the Fn+F3/F4 events (`04 14 dd rr`,
`dd` 0 = slower, 1 = faster, `rr` the new level, slower = higher) carried on
from the firmware's own level after `RR` = 0 and `RR` = 1 were sent. Whether
`11` sets something else, or is ignored by this firmware, is unknown.

After the apply sequence sends `11`, Monitor waits up to 1 s (10 x 100 ms) to
see whether the device disconnects (`DAT_0059d378` is set by the removal
handler while an apply is running). Expect the keyboard to re-enumerate after
a polling-rate change.

### Profiles

- The keyboard holds 3 profiles. Monitor only ever uploads lighting, key tables
  and macros for profiles 1 to 3 (all loops in `FUN_004057d0` stop at 3),
  even though the UI and the 5 `.gkp` files suggest 5.
- Read the active profile with `81` + GetFeature (byte 1, 1-based). Monitor
  does this 200 ms after start-up (timer `0x1F6` -> `FUN_00405620`).
- Select with `01 pp`. Monitor sends it when the user picks a profile, when a
  game executable linked to a profile starts (polled every 2 s,
  `FUN_00402170` -> `FUN_00405570`), and at the end of every apply.
- Game profiles (`GameFile=` in the `.gkp`) are pure software.

### Windows key lock, gaming mode, key response time, factory reset

None of these are sent by Monitor:

- The UI's Windows key ON/OFF (Option.exe `0x429320`) only writes the
  profile's `ModifyKey2` field. Nothing in Monitor reads it for a USB command.
- Gaming mode (Fn+PrtSc), WASD/arrow swap (Fn+Win), key response time
  (Fn+F1/F2; `ReportDelay=8` in the `.gkp` is never read by Monitor) are
  firmware-only.
- "Reset" in the UI resets the files and then runs a normal apply. There is no
  reset command.

## The apply sequence (`FUN_004057d0`)

Triggered by Option's OK/Apply (`0x4C8`). In order:

1. If the active profile changed: `01 pp`, sleep 100 ms.
2. `02 09 70 07`, sleep 100 ms.
3. For profile 1, 2, 3: `07 pp 00 LL 00 CC 00`, sleep 100 ms each. Always sent.
4. For profile 1, 2, 3, only if that profile's key table or any of its six
   macros is dirty: build the 640-byte table (`FUN_004031c0`), send
   `0C pp 00 0A`, sleep 200 ms, then 10 output reports of 64 bytes with 30 ms
   between them, then sleep 100 ms.
5. For each dirty macro slot (6..23): `10 00 ii 02`, then 2 output reports of
   64 bytes, sleeping 100 ms after each report. There is no delay between the
   feature and the first report.
6. `11 00 RR 01` (no delay).
7. `01 pp` (current profile).
8. Wait up to 1 s for a possible re-enumeration, then clear all dirty flags.

The delays are what the vendor uses; whether the firmware needs them is not
known.

## Key table (640 bytes, `0C`)

160 entries of 4 bytes, indexed by key-matrix position (not by HID usage).
Monitor starts from a built-in default table (`FUN_00405b50`, below), applies
the profile's remaps, then fills the six macro-key entries.

Entry encodings seen in the code and the default table:

| Entry | Meaning |
|---|---|
| `00 00 uu 00` | Keyboard key, HID usage `uu` (modifiers are `E0`..`E7`) |
| `03 00 lo hi` | Consumer control usage `0xhilo` (e.g. `03 00 CD 00` play/pause, `03 00 92 01` calculator) |
| `02 00 bb 00` | System control, bit `bb` (1, 2, 4; probably power, sleep, wake) |
| `01 00 Fn 00` | Mouse: `F0` left, `F1` right, `F2` wheel button, `F3`/`F4` back/forward |
| `13 F0 32 02` | Mouse "double click left" (UI type `F5`) |
| `13 F0 32 00` | Mouse "fire key", repeated left click (UI type `F6`) |
| `13 03 aa bb` | Dual key: `aa` normally, `bb` when the WASD/arrow swap is on (W/Up, A/Left, S/Down, D/Right) |
| `12 01 nn 00` | Firmware special key `nn` (00..14): F1..F12, PrtSc, Scroll Lock, Pause and others with Fn functions |
| `12 00 00 00` | Probably the Fn key (index 79) |
| `05 00 03 0n` | Profile select `n` = 1..3: these are P1, P2, P3 |
| `0F 80 ii 00` | Play on-board macro slot `ii` |
| `12 03 nn 00` | Default content of M-key entries; Monitor always overwrites them |
| `11 00 0A jj` | "Start application" `jj` (1..15): the keyboard sends an event, Monitor launches the program |
| `00 00 00 00` | Disabled |

The `13 F0 32 ..` meanings are from the UI order "Double click left", "Fire
key" and the remap code at `FUN_00402660`; `0x32` is likely 50 ms. Medium
confidence.

### Default table

From `FUN_00405b50` (assembly listing, exact):

```
  0: 00 00 1D 00  00 00 1E 00  02 00 01 00  03 00 B6 00  00 00 E6 00  03 00 B5 00  02 00 02 00  02 00 04 00
  8: 00 00 06 00  00 00 20 00  0F 80 00 00  03 00 E9 00  03 00 EA 00  03 00 27 02  00 00 E4 00  03 00 2A 02
 16: 00 00 09 00  12 01 00 00  12 01 04 00  03 00 24 02  00 00 E2 00  03 00 25 02  12 03 01 00  03 00 21 02
 24: 00 00 28 00  12 01 01 00  12 01 05 00  13 03 1A 52  13 03 04 50  00 00 E1 00  13 03 16 51  13 03 07 4F
 32: 00 00 29 00  00 00 0E 00  00 00 13 00  00 00 65 00  00 00 0A 00  00 00 39 00  00 00 E0 00  03 00 8A 01
 40: 00 00 2C 00  00 00 21 00  00 00 87 00  12 01 0D 00  03 00 23 02  00 00 8B 00  00 00 8A 00  00 00 88 00
 48: 13 03 50 04  12 01 03 00  00 00 12 00  03 00 92 01  00 00 64 00  00 00 91 00  00 00 90 00  00 00 E7 00
 56: 00 00 2B 00  00 00 35 00  12 01 06 00  00 00 1C 00  00 00 4D 00  00 00 24 00  00 00 33 00  12 03 03 00
 64: 13 03 51 16  00 00 2D 00  12 01 07 00  00 00 18 00  00 00 2F 00  00 00 25 00  00 00 49 00  00 00 11 00
 72: 13 03 4F 07  00 00 2E 00  12 01 08 00  00 00 0C 00  00 00 30 00  00 00 26 00  00 00 34 00  12 00 00 00
 80: 13 03 52 1A  00 00 2A 00  12 01 0A 00  12 01 0E 00  00 00 4A 00  00 00 4C 00  00 00 0B 00  00 00 37 00
 88: 00 00 08 00  00 00 0F 00  12 01 0B 00  12 01 0C 00  12 01 0F 00  12 01 13 00  00 00 32 00  00 00 38 00
 96: 00 00 14 00  00 00 0D 00  12 01 09 00  00 00 89 00  00 00 31 00  00 00 27 00  12 01 14 00  00 00 36 00
104: 00 00 05 00  00 00 10 00  00 00 00 00  00 00 53 00  00 00 5F 00  00 00 5C 00  00 00 59 00  12 03 04 00
112: 00 00 19 00  00 00 22 00  05 00 03 01  00 00 54 00  00 00 60 00  00 00 5D 00  00 00 5A 00  00 00 62 00
120: 00 00 15 00  00 00 23 00  05 00 03 02  00 00 55 00  00 00 61 00  00 00 5E 00  00 00 5B 00  00 00 63 00
128: 00 00 17 00  12 01 02 00  05 00 03 03  00 00 56 00  00 00 57 00  00 00 85 00  00 00 58 00  12 03 05 00
136: 00 00 1B 00  00 00 1F 00  03 00 94 01  03 00 CD 00  03 00 83 01  00 00 E5 00  03 00 E2 00  03 00 B7 00
144..159: all zero
```

Index 106 is never written by the vendor code, although it is an M-key slot
(see below); Monitor fills it on upload anyway.

Notable positions:

- M1..M6: indices **10, 22, 63, 106, 111, 135** (hard-coded list in
  `FUN_004031c0`).
- P1, P2, P3: indices **114, 122, 130** (`05 00 03 01/02/03`).
- Media keys: play/pause 139 (`CD`), previous 3 (`B6`), next 5 (`B5`),
  volume down 12 (`EA`), volume up 11 (`E9`), mute 142 (`E2`). Other consumer
  entries: stop 143, media select 140, my computer 138, mail 39, calculator
  51, browser search 23, home 44, back 19, forward 21, refresh 13,
  favorites 15. Which of these are the three icon keys and which are
  Fn-layer positions is not known.

### Remaps from the profile

`standardkey` in the `.gkp` is 104 bytes (the 105th hex byte is the
`WritePrivateProfileStruct` checksum, see below). Position `k` holds the new
usage for the key whose default usage is the `k`-th byte of the table at
`0x547ea8` (same as the default `standardkey`: Esc, F1..F12, PrtSc, ScrLk,
Pause, \` 1..0 - = Bksp, Tab Q..., and so on in the US layout order).
`FUN_00402660` finds that key by searching the default key table for
`00 00 <default usage> xx` and then:

- usage `F0`..`F4` (mouse): entry becomes `01 00 Fn ..`
- `F5`: entry becomes `13 F0 32 02`; `F6`: `13 F0 32 00`
- anything else: only byte 2 is replaced (so the entry type stays `00 00`)

Keys whose default entry is not a plain `00 00 uu` cannot be found and are
silently skipped: F1..F12, PrtSc, Scroll Lock, Pause, W, A, S, D, the arrows
and the Win keys. The UI also greys these out (`layout.ini` gives them a
zero-size button). A new driver can write any entry directly, but whether the
firmware honours other types at those positions is untested.

`GKey=3A..41` in the `.gkp` (F1..F8, plus checksum `EC`) is read into the
profile but never used by Monitor.

### M-key entries and macro slots

For each of the six M keys (`MacroKeyInfo0..5` of the profile; slots 6..31 in
the `.gkp` are unused by Monitor) the entry comes from a lookup on the action
type (table at `0x547f10`, 43 rows of 5 dwords):

| UI type | Key entry | Macro slot content |
|---|---|---|
| `01` | `00 00 00 00` disabled | - |
| `02` key combo | `0F 80 ii 00` | modifier downs, key down, key up, modifier ups |
| `03` recorded macro | `0F 80 ii 00` | the recorded events |
| `0B` start application | `11 00 0A jj` (`jj` = M-key number 1..6) | - |
| `18`..`1F`, `20`..`29` multimedia | `03 00 lo hi`: `18` mute E2, `19` vol+ E9, `1A` vol- EA, `1B` media 183, `1C` prev B6, `1D` next B5, `1E` play CD, `1F` stop B7, `20` search 221, `21` home 223, `22` back 224, `23` forward 225, `24` stop 226, `25` refresh 227, `26` favorites 22A, `27` mail 18A, `28` explorer 194, `29` calculator 192 | - |
| `40`..`4D` Windows/Office functions | `0F 80 ii 00` | fixed combos (below) |
| `F0`..`F6` mouse | as in the key table types | - |

Types `06` (seen in `MacroKeyInfo6..14` of the default `.gkp`) have no row and
produce a zero entry.

The macro slot index is `ii = 6 * profile + m` with profile 1..3 and `m` =
0..5 (M1..M6), so slots 6..23. The 128-byte slot images live at
`0x59d888 + ii * 0x80`. Slots 0..5 are never written (the default table's
`0F 80 00 00` at index 10 refers to slot 0, presumably a factory macro).

Fixed combos (verified, `FUN_004031c0`):

| Type | Combo | Type | Combo |
|---|---|---|---|
| `40` | Ctrl+X | `47` | Ctrl+S |
| `41` | Ctrl+C | `48` | Ctrl+Z |
| `42` | Ctrl+V | `49` | Alt+Tab (switch applications) |
| `43` | Ctrl+A | `4A` | Alt+F4 (close window) |
| `44` | Ctrl+F | `4B` | Win+L (lock PC) |
| `45` | Ctrl+N | `4C` | Win+R |
| `46` | Ctrl+P | `4D` | Win+D (show desktop) |

### Macro slot format (128 bytes, `10`)

```
[0] header A     [1] header B     then events, zero padded to 128 bytes
```

Header: for combos and fixed functions `00 01`. For recorded macros
`[0]` = byte `0x31` and `[1]` = byte `0x30` of the `MacroKeyInfo` record
(probably loop mode and repeat count; not confirmed).

Events, two forms (`FUN_004031c0` `0x403724..0x4037b4`, from the assembly):

- Short (delay / 20 <= 127): 2 bytes
  `[ (delay_ms / 20 & 0x7F) | 0x01 | (release ? 0x80 : 0), usage ]`.
  The code really ORs 0x01 into the delay bits, so odd and even delay units
  collide; for delay 0 this is `01` = press, `81` = release.
- Long (delay / 20 > 127): 4 bytes
  `[ release ? 0x81 : 0x01, usage, delay_ms / 50, 0x02 ]`.

Key combo example, Ctrl+C:

```
00 01  01 E0  01 06  81 06  81 E0  00 ... (128 bytes)
```

Modifier bits in the `MacroKeyInfo` record: bit 0 Ctrl (`E0`), bit 1 Shift
(`E1`), bit 2 Alt (`E2`), bit 3 Win (`E3`); pressed in bit order 0..3 and
released in reverse.

Mouse events in recorded macros: the UI offers "Insert mouse command", but the
encoder only copies a key byte and an up/down flag, so how mouse buttons are
stored is not known (not visible in Monitor).

### `MacroKeyInfo` record (772 bytes, `.gkp`)

The `.gkp` is an INI file whose hex values are written with
`WritePrivateProfileStruct`, which appends one checksum byte (sum of the data
bytes mod 256). So `MacroKeyInfoN` is 772 data bytes + 1, `standardkey` 104 + 1,
`GKey` 8 + 1, `apmLightInfo` 16 + 1.

| Offset | Meaning |
|---|---|
| `0x00` | record index |
| `0x01` | action type (table above) |
| `0x02` | modifier bits (type 2) |
| `0x04` | u16 key usage (type 2) |
| `0x06..0x2D` | not read by Monitor (likely the name, 20 UTF-16 chars) |
| `0x2E` | u16 event count (type 3) |
| `0x30`, `0x31` | copied to macro header bytes 1 and 0 (type 3) |
| `0x34 + 4n` | event n: `[usage, 1 = press / other = release, u16 delay_ms]`, up to 180 |

Default: M1..M6 = Ctrl+1 .. Ctrl+6 (`00 02 01 00 1E ...`), matching the UI
strings "Ctrl+1".."Ctrl+5".

A full profile upload therefore sends 640 bytes of key table plus up to six
128-byte macro slots; a recorded macro is limited to 63 short events by the
128-byte slot (Monitor does not check this).

## Events from the keyboard

Input reports on the `0xFF00` collection: byte 0 = report id `0x04`, then three
bytes. `hiddriver.dll` (`0x100020d0`) ignores reports whose first byte is not
4 and posts `(b1 << 8 | b2, b3)` to Monitor (`0x52D`), which handles them in
`FUN_00404db0` / `FUN_00405290`:

| `b1 b2` | `b3` | Meaning / Monitor action |
|---|---|---|
| `05 03` | 1..5 | Active profile changed to `b3`; OSD "Profile" |
| `08 02` | 0..2 | Colour changed (stored as `LtColor`) |
| `06 03` | 0..4 | Lighting level changed (stored as `LtControl`) |
| `14 00` / `14 01` | 0..3 | Polling rate changed |
| `50 00` | 0 / 1 | Win key locked / unlocked; OSD (in Monitor's handler; the hardware sends `13 00` instead, see below) |
| `13 00` | 0 / 1 | Win key unlocked / locked (Fn+Win), seen on hardware |
| `00 0A` | 1..15 | Start application `b3` (software) |
| `00 A0` | any | Open the configuration UI |
| `60 00` | `20`..`24` | Media-player helpers (open player, play/pause, stop for WMP, iTunes, Gemini or Winamp, chosen by `MediaOption`) |
| `70 00` | any | Forwarded to the worker thread and ignored |
| `00 00` | any | OSD only |

Arrival and removal are delivered separately (`0x52E`) and mapped to code
`0x7F00` 1 / 2.

On hardware: no events at all until the hello `02 09 70 07` is sent; after it
each event comes as `04 b1 b2 b3` followed by `04 00 00 00` about 100 ms later.
Seen: `05 03 nn` (P1..P3), `06 03 LL` (Fn+F9..F12, Fn+Scroll Lock), `14 dd rr`
(Fn+F3/F4), `13 00 v` (Fn+Win).

## Minimal sequences

Set profile 1 to 50 % blue and make it active:

```
SetFeature 00 | 02 09 70 07 00 00 00 28
SetFeature 00 | 07 01 00 02 00 01 00 A1
SetFeature 00 | 01 01 00 00 00 00 00 A8
```

Upload a key table to profile 1 (example: Caps Lock, index 37, becomes Left
Ctrl, and M1 plays Ctrl+C from slot 6):

```
table = default table (above)
table[37*4 .. +4] = 00 00 E0 00
table[10*4 .. +4] = 0F 80 06 00
for M2..M6 (indices 22, 63, 106, 111, 135): 0F 80 07..0B 00, or 00 00 00 00

SetFeature 00 | 02 09 70 07 00 00 00 28
SetFeature 00 | 0C 01 00 0A 00 00 00 BD
sleep 200 ms
Output 00 | table[0..64]      sleep 30 ms
...                           (10 reports)
Output 00 | table[576..640]
sleep 100 ms
SetFeature 00 | 10 00 06 02 00 00 00 B2
Output 00 | 00 01 01 E0 01 06 81 06 81 E0 00 .. 00     (64 bytes)   sleep 100 ms
Output 00 | 00 x 64                                                   sleep 100 ms
SetFeature 00 | 11 00 00 01 00 00 00 B8   (polling rate, always sent here)
SetFeature 00 | 01 01 00 00 00 00 00 A8
```

The order matters as far as we can tell from the vendor code: Monitor always
sends a profile's key table before its macros (the `0C` loop at
`0x405897..0x4058fd` finishes before the `10` loop at `0x405910..0x405941`),
and the only things sent after the last macro report are `11 00 RR 01`
(`0x40597d`) and `01 pp` (`0x4059be`). There is no other end or commit
command.

Hardware test result (profile 3): the key table upload works (index 10 =
`00 00 1B 00` made M1 type "x"). A macro test that sent hello, `10 00 12 02`,
two 64-byte reports `00 01 01 0B 81 0B 01 0C 81 0C ...`, then the key table
with index 10 = `0F 80 12 00`, then `01 03` produced nothing. The bytes match
what Monitor generates exactly (re-checked in the assembly, see below). That
test differed from Monitor in two ways: the key table went after the macro
(Monitor sends it before), and `11 00 RR 01` was left out. Both are untested
explanations. The likeliest is that `0C` clears the profile's macro storage.
`11`'s byte 3 = 1 could also be a save flag.

Retried in Monitor's order (hello, `0C` with index 10 = `0F 80 12 00`, the
macro, `11 00 00 01`, `01 03`): M1 typed "hi". So the order is what matters.
OpenDeviceControl re-sends every macro after each key-table upload, and does
not send `11`.

Assembly re-check (Monitor.EXE):

- `10` builder `FUN_00405a40` (`ret 4`, slot in EBX): `0x405a86` word
  `0x0010` gives b0 = `10`, b1 = `00`. `0x405aa0` b2 = slot. `0x405aa6`
  b3 = `02`. `0x405a70..0x405a93` checksum. SetFeature at `0x405aad`. It
  then copies 128 bytes from `0x59d888 + slot * 0x80` (`0x405ac6..0x405adf`)
  and sends them as two raw 64-byte WriteUSB buffers (`0x405b0d`/`0x405b1b`).
  Monitor sleeps 100 ms after each one (`0x405b26`) and puts no delay between
  the feature and the first buffer. The 64 bytes contain no page or index
  byte.
- Slot numbering: the apply loop starts at 6 (`0x4058ff`) and passes
  `base + m` (`0x405919`), with base 6/12/18 for profiles 1/2/3
  (`0x405932`). The key entry uses the same number: `0x40330c..0x40331d`
  computes `6 * p + m` (p = 1-based profile from the argument) into byte 2,
  `0x40331f`/`0x40332a` set bytes 0/1 to `0F 80`. Profile 3 M1 = slot 18
  (`0x12`) in both places.
- Type 2 macro image (`0x40364c..0x4036d0`): word `0x0100` at offset 0 gives
  `00 01`. Then `01, E0+bit` for each modifier bit 0..3 that is set, `01 key`,
  `81 key`, and `81, E0+bit` in reverse bit order. Ctrl+1 (default M1) is
  `00 01 01 E0 01 1E 81 1E 81 E0`, zero padded to 128 bytes.
- `0C` builder `FUN_00406260`: `0x4062cf` b0 = `0C`, `0x4062b3` b1 =
  q + 1, `0x4062d6` word `0x0A00` gives b2 = `00`, b3 = `0A`. Sleep 200 ms
  (`0x4062fe`), then ten raw 64-byte slices of the 640-byte table
  (`0x40632f`/`0x40633d`) with 30 ms between them (`0x406346`).

Monitor builds the M-key entries for every upload, so the six M-key entries
should always point at their slots (or be a non-macro action).

## Gotchas

- The hello `02 09 70 07` is undocumented. It is harmless as far as the vendor
  driver is concerned (sent on every start), but its exact effect is unknown.
- Profiles 4 and 5 in the UI do not exist on the device.
- Monitor waits for a re-enumeration after `11`; on hardware none happened.
- No firmware-update, bootloader or erase command exists anywhere in
  Monitor.EXE or hiddriver.dll, and there is no updater in the package. Only
  the seven commands above are ever sent, so anything else (other `b0`
  values, other usage pages) is unexplored and should not be tried casually.
- Do not send a key table without the matching macro slots if it references
  `0F 80 ii`; what the firmware plays from an empty slot is unknown.

- Fn+Scroll Lock's "light off" survives `07` writes; check it before
  concluding a lighting command failed.
- The keyboard can't report its key tables, macros or lighting. Only the
  active profile can be read. A host has to keep its own copy.

## Not verified on hardware

- The `RR` values and what `11` really does (no visible effect, above).
- The meaning of the macro header bytes and of the delay encoding (the `| 1`),
  and whether a delay byte is the pause before or after its event. Only
  zero-delay macros were tested; OpenDeviceControl puts a step's pause on the
  next event.
- Whether any key (not just M keys) can play `0F 80 ii`, and slots beyond 23.
- Mouse entries `01 00 Fx`, `13 F0 32 xx` as double click / fire key, `02 00
  bb 00` as system control, and the "start application" entry `11 00 0A jj`
  (and its `04 00 0A jj` event).
- What the Fn-layer entries at the unplaced indices do.
- ISO / other layouts: the matrix has `0x32`, `0x64` and Japanese positions;
  only the US layout was mapped.

## Analysis method

- Package: `SL-670005-BK_Driver_V1.0.zip`, an InstallShield 12 installer. The
  cabinet (`data1.hdr` + `data1.cab`/`data2.cab`) was unpacked with a small
  Python script (`isx.py`) that follows unshield's format: the file table in
  `data1.hdr`, 0x57-byte file descriptors, and file data stored as chunks of a
  u16 length followed by raw deflate.
- `Monitor.EXE` (sha256
  `67c212d46a7294a7d4d0f98e44f29a75a41016faad089c27acd4b09340b16306`) analysed
  in Ghidra 12.1 through REA: decompilation plus assembly where the
  decompiler was unclear (default key table, macro encoder). `Option.exe`
  (sha256 `57492bfdd22cc7715e2c4eaeff20d4ad96456a1476f362a4f1ed58b5888010ad`)
  only for the UI value mapping. `hiddriver.dll` disassembled with objdump.
- Lang_EN.dll strings, `skins/main.ini` and `skins/LAYOUT/US/layout.ini` for
  UI labels and button order.
- None of the Windows binaries were run.
