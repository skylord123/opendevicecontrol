# Corsair K55 RGB keyboard (NXP firmware)

This is the deepest firmware study in the project. The driver is in
`vendors/corsair/nxp.rs` and `nxp/models.rs`. The K55 is hardware-verified.

## Where the firmware came from

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

## Chip and memory map

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

## Command dispatch

USB reports are 64 bytes with no report id. The main dispatcher is at `0xd9d2`.
It switches on the command byte and, for most commands, a sub-command in the
next byte. The command families are:

- `07` set (write a setting)
- `0E` get (read a value back; the reply echoes bytes 0 to 3)
- `7F` data (fill a staging buffer)
- `A0`, `A1`, `A2` factory and test commands

A `0E` or `7F` command is ignored while a previous `0E` reply is still unread.

## `07` set commands

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
| `07 40 ...` | Key routing table (see [Corsair NXP family](corsair-nxp.md)) | We use this |
| `07 48 f` | Key locks: bit 7 master, bit 0 Win, bit 1 Alt+Tab, bit 2 Alt+F4, bit 3 Shift+Tab | We use this |
| `7F i ...` | Put 60 bytes (from byte 4) into the 256-byte staging buffer; `i` = 1 to 4 for each chunk, 5 for the last 16 bytes | Used by the update path |

## `0E` get commands

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

## The macro question: nothing is stored on the keyboard

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

## What the keyboard does store (EEPROM)

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

## Gotchas (things that look useful but are not)

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

## Dangerous commands (never send these)

These were found so the driver can be sure to avoid them. None is used anywhere
in the code.

- `07 02 03`, `07 02 04`, `07 02 F0`: write EEPROM flags and reset into the
  bootloader.
- `07 02 AA`: shut the peripherals down and re-enter the NXP ROM bootloader.
- `07 0C 02`: erase the EEPROM settings.
- `07 0C F0`: erase the firmware-update staging flash.

## What OpenDeviceControl does with this

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
