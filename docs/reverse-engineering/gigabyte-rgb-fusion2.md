# Gigabyte RGB Fusion 2 (USB)

Driver: `vendors/gigabyte/`. The ITE LED controller on Gigabyte
motherboards: `048d:8297` (IT8297, X570 / Z390 era), `048d:5702` (IT5702,
Z490 / B550 era), `048d:5711` (IT5711, Z890 / X870 era) and `048d:8950`
(IT82950, H810 boards). Motherboards only; Gigabyte's `0414:a100` (GC-USB)
is left out.

**Not tested on hardware**; based on liquidctl's and OpenRGB's protocol
work. See "Uncertain" below.

Everything is a 64-byte feature report with report ID 0xCC on the vendor
collection (usage page 0xFF89, usage 0xCC; the interface also has a usage
0x10 collection that is not used): `cc cmd args...`. Only the info commands
answer, through a feature-report read.

| Command | Request | Meaning |
|---|---|---|
| `20`–`27`, `90`–`92` | effect block, below | set the built-in effect of LED index 0–7, 8–10 |
| `28` | `mask:u32` | apply the effects set since the last apply |
| `31` | `0` / `1` | audio "beat" mode off / on |
| `32` | `mask` | addressable headers whose built-in effect is off (direct colours) |
| `34` | `n1n0 n3n2 n5n4` | length class of header slots 0–5, a nibble each |
| `48` | `0` / `1` | Windows Dynamic Lighting (lamp array) off / on |
| `58`, `59`, `62`, `63` | `offset:u16 bytes rgb...` | direct colours for header slot 0, 1, 2, 3 |
| `60` | | info report |
| `61` | | IT5711: colour order of header slots 2–5 |

Never sent by the driver: `33` (write the colour-order calibration), `47`
(lighting persistence) and `5e` (save to flash), `38`–`3d` (Gen2 strip
detection), and anything belonging to the firmware updater.

## Info report (`60`)

Read back with a feature-report read of ID 0xCC. The IT8297 answers with 0
in place of the report ID.

| Bytes | Field |
|---|---|
| 0 | report ID (0xCC, or 0) |
| 1 | product (1 on IT5702 / IT8297) |
| 2 | controller number: 0, or 1 for the second controller of dual-controller boards |
| 3 | Gen2 strip detection |
| 4–7 | firmware version, printed `b4.b5.b6.b7` |
| 8–10 | length class of header slots 0–5, low nibble first |
| 11 | flags: bit 0 persistence / Gen2 commands, bit 1 lamp array |
| 12–39 | name, NUL-padded, e.g. `IT5702-GIGABYTE V1.0.10.0` |
| 40, 52, 60 | calibration of spare / onboard LEDs (u32, unused) |
| 44, 48 | calibration of header slots 0, 1 (u32) |
| 56 | chip ID (u32) |

`61` (IT5711 only) has the calibration of header slots 2–5 at bytes 4, 8,
12, 16.

A calibration word is `b g r 00`: the byte position (0–2) of blue, green and
red within each LED of a direct-colour report, so `02 01 00 00` is RGB and
`02 00 01 00` is GRB. Zero means not calibrated.

Length classes: 0 up to 32 LEDs, 1 up to 64, 2 up to 256, 3 up to 512, 4 up
to 1024. The IT57xx drive at most 256 LEDs per header.

## Zones

The controller can't say which LEDs a board has, and what each LED index
lights differs from board to board (OpenRGB keeps a table by board name).
The driver uses the generic layouts:

| Controller | Addressable headers (LED index, direct command, `32` bit) | Single-colour LED indices |
|---|---|---|
| IT8297, IT5702 | D_LED1 (5, `58`, 0x01), D_LED2 (6, `59`, 0x02) | 0–4, 7 |
| IT5711 | ARGB_V2_1–4 (5, 6, 7, 8; `58`, `59`, `62`, `63`; 0x01, 0x02, 0x08, 0x10) | 0–4 |
| IT82950 | ARGB_V2_1 (2, `58`, 0x01), ARGB_V2_2 (3, `59`, 0x02) | none |
| any, controller 1 | none | 0–7 (IT5711: 0–4, 8–10) |

On the IT5711, LED indices 9 and 10 are onboard addressable strips of
board-specific length (direct commands `64`, `65`, `32` bits 0x20, 0x40);
the driver leaves them alone.

## Effects

Effect block, after `cc cmd`:

| Bytes | Field |
|---|---|
| 2–5 | zone mask, `1 << led` (u32) |
| 6–9 | second zone mask, 0 |
| 11 | effect type |
| 12 | maximum brightness |
| 13 | minimum brightness |
| 14–17 | colour, `b g r 00` |
| 18–21 | second colour |
| 22–29 | periods 0–3 in ms (u16 each): fade in, fade out, hold, extra |
| 30–33 | parameters 0–3 |

Then `28` with the zone masks applies them (`28 ff` applies LED indices
0–7).

| Effect | Type | Periods (speed step s, 0 fastest to 9) | Parameters |
|---|---|---|---|
| Off | 1 | | brightness 0 |
| Static | 1 | | |
| Breathing | 2 | 400 + 100s (s ≤ 6, then 1000 + 200(s−6)) twice, 200 | |
| Flash | 3 | 100, 100, 700 + 200s | 0, 1, 1 |
| Double flash | 3 | 100, 100, 800 + 200s | 0, 1, 2 |
| Color cycle | 4 | 300 + 100s (+1300 past s = 8), that − 200 | 7 |
| Random | 8 | 100 | 1, 5 |

Brightness runs to 255, except Breathing, which misbehaves above 100. The
IT82950 boards have no Random. Wave (6) and the board-wide Wave 1–4 (9–12)
are not offered.

## Direct colours

1. `48 00` (when flag bit 1 is set) and `31 00`, so neither Dynamic
   Lighting nor beat mode overrides the LEDs.
2. `32 mask` with the bits of the headers to drive.
3. Per header: `cmd offLo offHi count` and up to 19 LEDs, where the offset
   and count are in bytes (LED × 3), each LED's bytes ordered by the
   header's calibration.
4. `34` with each header's length class, when the user changes a header's
   LED count.

Single-colour LEDs have no direct mode: they get a Static effect in the
frame's colour, then `28`. Leaving direct mode is `32 00`, then the
effects again.

## Uncertain

- No read-back of effects exists, so effects show as unknown until set;
  the front-end re-applies the ones set here when the device opens again.
- Effect timings and parameters follow OpenRGB for most effects; Flash
  uses liquidctl's parameters (OpenRGB sends zeros). Whether every board
  accepts brightness 255 (liquidctl uses at most 100) is unknown.
- Whether `28` is needed after direct colours (OpenRGB sends one with an
  empty mask; the driver doesn't).
- The colour order used when a header has no calibration (RGB is assumed).
- The IT5711 and IT82950 layouts come from OpenRGB only; its zone
  definitions for these overlap (LED index 8 is both a single LED and the
  fourth header on the IT5711).
- Resume from sleep resets the controller (to static blue, per liquidctl);
  the driver resends the direct-colour state every 2 s while streaming but
  does not re-apply built-in effects.
