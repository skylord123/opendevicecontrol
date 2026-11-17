# Reverse engineering notes

This file collects the protocol and firmware notes behind OpenDeviceControl's
device support, so the work does not live only in code comments and commit
messages. It is meant for anyone (people or language models) who wants to
understand, check, or extend a driver.

How the work was done, in general:

- Static analysis of shipped firmware and of vendor software, plus the public
  protocol work of OpenRGB, ckb-next, liquidctl and OpenLinkHub.
- On-device confirmation over USB or I2C where the hardware was on hand. Each
  section says what was checked on hardware and what is inference.
- No firmware was ever flashed, and no bootloader or update command was sent to
  a real device. Commands that would write flash or enter a bootloader are
  documented so we can be sure to avoid them, not to use them.

The driver code is under `crates/opendevicecontrol-core/src/vendors/`.

## Contents

- [Corsair K55 RGB keyboard (NXP firmware)](#corsair-k55-rgb-keyboard-nxp-firmware)
- [Corsair NXP family (shared protocol)](#corsair-nxp-family-shared-protocol)
- [ASRock Polychrome RGB (USB)](#asrock-polychrome-rgb-usb)
- [ASUS Aura (ENE) lighting](#asus-aura-ene-lighting)
- [Aula F99 keyboard](#aula-f99-keyboard)

## Corsair K55 RGB keyboard (NXP firmware)

This is the deepest firmware study in the project. The driver is in
`vendors/corsair/nxp.rs` and `nxp/models.rs`. The K55 is hardware-verified.

### Where the firmware came from

iCUE does not ship the K55 firmware on disk. It downloads it on demand. The
app keeps its file-server base URL in an AES encrypted `tokens.bin`, which the
`DeviceFirmware.dll` updater decrypts at runtime. The firmware index for a
model is `{base}/fw/{FirmwareServerIndex}.json`, and the K55 RGB's index key is
`K55RGB`. The index names the current package:

```
https://www3.corsair.com/software/CUE_V5/public/fw/K55RGB.json
  -> { "firmware-image": { "version": "30.09", "filename": "K55RGB_30.09.zip" } }
```

The zip holds one image, `K55_App_v3009.bin` (41,012 bytes), the application
half of the firmware. It is not encrypted. Everything below comes from static
disassembly of that image with capstone (Thumb), cross-checked against the
device over USB.

### Chip and memory map

- MCU: an NXP LPC11U3x class Cortex-M0 (128 KB flash, 4 KB EEPROM, a few KB of
  RAM), running its own USB stack (no USB ROM driver).
- The application image loads at `0x6000`. The reset vector is `0x60d5`, the
  initial stack pointer `0x10001338`, and every exception and IRQ vector lands
  in that image, which confirms the base. Flash below `0x6000` is the
  bootloader and is not in this file.
- On-chip EEPROM is reached through IAP at `0x1FFF1FF1`. The firmware's IAP
  wrappers are around `0x6ed0` to `0x707a`: prepare/copy/erase (commands 0x32,
  0x33, 0x34), read UID (0x3A), and EEPROM read/write (0x3E at `0x707a`, 0x3D
  at `0x704c`).
- LEDs are driven over I2C by a controller at address `0x78` whose register map
  matches an ISSI IS31FL3236 (0x00 shutdown, 0x01+ PWM, 0x25 update, 0x26+ LED
  control). There are 9 RGB LEDs, three per zone, in G, R, B channel order.
  Every software path writes the same colour to all three LEDs of a zone, so
  per-LED control inside a zone is not possible on this board.

### Command dispatch

USB reports are 64 bytes with no report id. The main dispatcher is at `0xd9d2`.
It switches on the command byte and, for most commands, a sub-command in the
next byte. The command families are:

- `07` set (write a setting)
- `0E` get (read a value back; the reply echoes bytes 0 to 3)
- `7F` data (fill a staging buffer)
- `A0`, `A1`, `A2` factory and test commands

A `0E` or `7F` command is ignored while a previous `0E` reply is still unread.

### `07` set commands

| Bytes | Meaning | Notes |
|---|---|---|
| `07 04 01` | Hardware mode: clears the lock flags, resets the key table to defaults, re-inits the LEDs | We send this on release |
| `07 04 02` | Software mode | We send this on open |
| `07 05 02 00 n` | Brightness level 0 to 3 (0 off, 1 = 33 %, 2 = 66 %, 3 = 100 %). The firmware scales software colours by it too | We expose this as a control; see below |
| `07 05 05 00 mask` | Lock indicator LEDs: bit 7 Caps, bit 6 Num, bit 0 Scroll. The next host LED report overrides it | Not used (the OS owns these LEDs) |
| `07 05 06 00 x` | Scroll Lock LED | Not used |
| `07 05 09 00 x` | Win-lock indicator LED | We use this |
| `07 05 08 ...` | No-op on the K55 | Harmless; we still send it, see the gotchas |
| `07 25 00 00` + RGB x3 | The three zone colours, scaled by the brightness level | We use this |
| `07 40 ...` | Key routing table (see the shared-protocol section) | We use this |
| `07 48 f` | Key locks: bit 7 master, bit 0 Win, bit 1 Alt+Tab, bit 2 Alt+F4, bit 3 Shift+Tab | We use this |
| `7F i ...` | Put 60 bytes (from byte 4) into the 256-byte staging buffer; `i` = 1 to 4 for each chunk, 5 for the last 16 bytes | Used by the update path |

### `0E` get commands

| Bytes | Reply |
|---|---|
| `0E 01` | Device info (layout below) |
| `0E 00` | Byte 4 = current mode (1 hardware, 2 software, 0x11) |
| `0E 25` | Readback of the current zone colours: R1 G1 B1 R2 G2 B2 R3 G3 B3 |
| `0E 48` | Byte 2 packs the lock flags, same bit layout as `07 48` |
| `0E 15 01 {00\|01}` | 32 bytes of a stored profile block (see note) |
| `0E 16 01 {00\|01}` | 32 bytes: the UTF-16 strings "HwProfile1" / "HwMode1" |
| `0E 0E` | Byte 4 = status of the last EEPROM or flash operation (0 = OK) |
| any other `0E` | Zero-filled reply with the echoed header |

`0E 01` reply layout (bytes within the 64-byte reply):

- `[4]` HID protocol state
- `[8..9]` firmware version, little-endian (0x3009 = 3.09)
- `[10..11]` bootloader version
- `[12..15]` VID and PID
- `[16]` poll interval in ms (always 1 on this build)
- `[23]` keyboard layout (0 ANSI, 1 ISO, 2 ABNT, 3 JIS)
- `[24]` current brightness level 0 to 3 (live; the brightness key changes it)
- `[29]` host LED state (bit 6 Num Lock, bit 7 Caps Lock)

### The macro question: nothing is stored on the keyboard

The reason this study started was to see whether the keyboard stores macros or
profiles we could read back for a better macro editor. It does not.

- The command set has no macro, profile, or key-action storage command.
- G1 to G6 and MR are ordinary keys set to "report to host only" in the default
  key table. The firmware does nothing for them in hardware mode. iCUE (and
  OpenDeviceControl) record and play macros entirely in software.
- The `0E 15` and `0E 16` "profile" reads return hard-coded constants built at
  `0xe91c` and `0xe994` (an id block and the fixed strings "HwProfile1" and
  "HwMode1"), not stored user data.

So a macro editor has to stay software-side, which is what OpenDeviceControl
already does.

### What the keyboard does store (EEPROM)

The only user state the keyboard persists is its built-in lighting, in a small
EEPROM settings block at offset 0x64 (marker byte 0x47), read at boot near
`0x6266` and saved automatically when effects are changed from the keyboard:

- `[1]` built-in effect id (default 0x0D)
- `[2]` brightness level
- `[3]` speed (0 to 2)
- `[4]` direction / variant bits
- `[5..7]` palette indices (0 to 8) for the three zones
- `[8]` effect slot
- `[9]` flags

There are 14 built-in effect handlers (default 0x0D). In hardware mode, holding
the brightness key and pressing another key edits them: number keys 1 to 0
select effects 0 to 9, minus and equals select the rest, F1/F2/F3 cycle each
zone's palette (slot 7 is a custom colour set by the `A1` command), and the
arrow keys change speed and direction (handler at `0x8cec`). There is no USB
command to pick a built-in effect directly; writing the EEPROM block and
resetting would do it, but that is untested and needs hardware.

### Gotchas (things that look useful but are not)

- `07 40` key routing uses the top two bits of each mode byte: `0xC0` default,
  `0x80` default HID, `0x40` report to host only, and top bits `00` means the
  key is ignored. The shared driver's hardware-action values (ACT_LIGHT 0x01,
  ACT_LOCK 0x08, ACT_MR 0x09) all have top bits `00`, so on the K55 the
  firmware does not act on them. It does not matter in practice because
  `07 04 01` (sent first on release) resets the key table to defaults anyway.
- `07 05 08` is a no-op on the K55. The driver still sends it (it is needed on
  other NXP keyboards and is harmless here), so we left the verified sequence
  alone.
- Do not use the `0E 40` routing readback. The firmware indexes its internal
  table by the raw key index without the mapping it uses on the write path, so
  the values come back wrong for most keys.

### Dangerous commands (never send these)

These were found so the driver can be sure to avoid them. None is used anywhere
in the code.

- `07 02 03`, `07 02 04`, `07 02 F0`: write EEPROM flags and reset into the
  bootloader.
- `07 02 AA`: shut the peripherals down and re-enter the NXP ROM bootloader.
- `07 0C 02`: erase the EEPROM settings.
- `07 0C F0`: erase the firmware-update staging flash.

### What OpenDeviceControl does with this

Added from this study (`nxp.rs`, K55 only):

- Brightness is exposed as a control and read back from `0E 01` byte 24 on
  open, so it matches whatever the keyboard's brightness key last set instead of
  being forced to full. Changes are sent as `07 05 02 00 n` on top of the
  verified software-mode sequence, which is left byte-for-byte unchanged.
- The key-lock state is read from `0E 48` on open, so taking the keyboard over
  keeps the user's current Win-lock state rather than clearing it.

Left for a hardware test on the K55 (documented here so it can be tried on the
machine the keyboard is plugged into):

- Dropping the no-op `07 05 08` for the K55.
- Setting the default built-in effect by writing the EEPROM settings block.
- Reading the EEPROM block to show which built-in effect is set.

## Corsair NXP family (shared protocol)

The K55 driver is one model in a shared NXP driver that also covers, from
ckb-next and OpenRGB and untested here, the per-key K65/K68/K70/K95 and STRAFE
keyboards and a range of Corsair mice, the MM800 mousepad and the ST100 stand.
The full command table for the family is in the module comment at the top of
`vendors/corsair/nxp.rs`. In short:

- 64-byte reports, first byte is the opcode, replies to `0E` queries come back
  on the same handle (as feature reports on Windows).
- `0E 01` device info, `07 04` mode, `07 05` lighting enable, `07 40` key
  routing, `07 48` key locks, `07 25`/`07 28`/`07 27`/`07 22` colours by model
  type, `07 13` mouse DPI, `07 0A` poll rate, `7F` per-key colour stream.

The per-key keyboards and mice use hardware-action and `07 05 08` bytes that
the K55 ignores; those are kept because they are believed correct for the
models they came from. Treat anything not marked hardware-verified as drawn
from ckb-next or OpenRGB and not confirmed here.

## ASRock Polychrome RGB (USB)

Driver: `vendors/asrock/`. Device `26ce:01a2`, the Nuvoton LED controller on
recent ASRock boards. Fully reverse engineered from ASRock's Polychrome RGB
utility and confirmed on hardware.

Every message is a 64-byte output report (65 with the leading zero):
`cmd 00 a b ...`. Only the read commands (0x11, 0x14, 0xA6) answer, with a
64-byte input report `a6 00 00 a data...`.

| Command | Request | Meaning |
|---|---|---|
| `10` | `ch mode r g b speed ff sync 00 [gradient]` | set a channel's effect |
| `10` | `ff e3 00 00 nlo nhi rgb...` then `ff e4 rgb...` | direct colours, all channels |
| `11` | `ch` | read a channel's effect |
| `12` | | save the current effects to flash |
| `14` | `01` / `02` / `03` / `0a ch` | read channel mask / LED counts / R-G swap / gradient |
| `15` | `02 n0..n7` / `03 mask` | write LED counts / R-G swap |
| `a6` | | firmware version |

Details:

- The channel order is RGB LED 1, RGB LED 2, Addressable RGB LED 1, Addressable
  RGB LED 2, PCH heatsink, I/O cover, PCB, Addressable RGB LED 3. Channels 2, 3
  and 7 are addressable.
- Effect mode bytes run 0x00 to 0x11; the speed byte is `255 - slider`.
- The direct-colour stream is `10 00 ff e3 00 00 totalLo totalHi` plus 18 RGB
  triples, then `e4` packets of 19 triples each. To leave direct mode the
  firmware needs an Off effect sent first.
- Command 0x30 and the ISP commands belong to the firmware updater and are
  never sent.

## ASUS Aura (ENE) lighting

Driver: `vendors/asus/ene.rs` (the controller) and `gpu.rs` (graphics card
lighting over the card's I2C bus). Register layout as documented by OpenRGB's
`ENESMBusController`, confirmed on hardware.

The controller has a 16-bit register space reached through an address pointer:

| SMBus command | Meaning |
|---|---|
| write word `00` | set the pointer (register address, big-endian) |
| read byte `81` | read the register at the pointer |
| write byte `01` | write the register at the pointer |
| write block `03` | write consecutive registers from the pointer |

| Register | Meaning |
|---|---|
| `1000` | device name, 16 bytes of ASCII |
| `1C00` | configuration table, 64 bytes |
| `8020` | 1: direct colours, 0: built-in effect |
| `8021` / `8022` / `8023` | effect mode / speed / direction |
| `80A0` | write `01` to apply an effect (`AA` would also save it to flash) |
| `8100` | direct colours, 3 bytes per LED |
| `8160` | effect colours, 3 bytes per LED |

Colours are stored red, blue, green. The controller is identified first by
registers `A0` to `AF` reading back 0 to 15 (read directly, no pointer).
OpenDeviceControl writes direct colours for its own effects and never writes
`AA`, so it never saves over the card's stored effect.

## Aula F99 keyboard

Driver: `vendors/aula/`. The CX / Sinowealth 2.4 GHz receiver is `3554:fa09`
(vendor collection usage page 0xFF02, report 0x13); the same keyboard also
works over USB cable. Sources: OpenRGB issue #5166 and draft MR !3027 for live
lighting, plus static analysis of the official AULA F99 driver for the settings
block, acknowledgements and events. Checked on an F99.

Every packet is 20 bytes including the report id:

| byte | meaning |
|---|---|
| 0 | report id `0x13` |
| 1 | command |
| 2 | packets in this transfer (replies: bit 7 = checksum error) |
| 3 | index of this packet (0-based) |
| 4 | sub-command (high nibble) + payload length (low nibble, <= 14) |
| 5..19 | payload |
| 19 | checksum: sum of bytes 0 to 18, mod 256 |

| Command | Meaning |
|---|---|
| `07` | is the keyboard linked and awake? reply `[5]` = 1/0 |
| `05` | model query, reply `[10]` = model id |
| `44` / `04` | read / write the 128-byte settings block |
| `88` | live lighting, sub 1 = per-key, sub 2 = solid (not acknowledged) |
| `4A` | battery: reply `[5]` = percent, `[6]` = flags (bit 0 not full, bit 4 on USB power) |
| `0A` (input) | event: `[5]=2` keyboard online, `[5]=5` battery, `[5]=7` settings reloaded |

Details:

- Writes other than `0x88` are acknowledged packet by packet. Read requests are
  one empty packet; the answer comes back in as many packets as needed.
- Per-key lighting is a run of groups `R G B n led1..ledn` split across packets;
  LEDs not named in a transfer are switched off. The keyboard falls back to its
  own effect unless live lighting is refreshed regularly.
- The keyboard's buffer holds 199 payload bytes per frame; the official driver's
  200-byte cap overruns it, so OpenDeviceControl caps at 196.
- The firmware exposes a battery read (`4A`) that the official driver does not
  use. A firmware timing issue with sound-reactive lighting was reported to
  Aula separately.
