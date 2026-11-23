# ASUS Aura motherboard lighting (USB)

Driver: `vendors/asus/aura_usb.rs`, packets in `vendors/asus/aura_usb_proto.rs`.

**Not tested on hardware**; based on OpenRGB's protocol work
(`Controllers/AsusAuraUSBController/`), cross-checked against liquidctl's
`aura_led` driver. No code was taken from either.

Two USB controllers, both vendor `0b05`:

| Kind | PIDs | Product string | Effects |
|---|---|---|---|
| Motherboard LED controller | `18f3`, `1939`, `19af`, `1aa6`, `1bed` | "AURA LED Controller" | `35` / `36`, saved with `3f` |
| Addressable header controller (older boards) | `1867`, `1872`, `18a3`, `18a5` | "AURA MAINBOARD" | `3b` |

`19af` boards expose more than one HID interface; the lighting one has
usage page `ff72` (usage `a1`). The ROG Aura Terminal (`1889`) and ROG
Strix LC III (`1b29`) use the same protocol but are not motherboards and are
not handled here.

Every message is report `ec` plus 64 bytes: `ec cmd ...`, zero-padded. Only
the two reads answer, with input report `ec reply ...`.

| Command | Request | Meaning |
|---|---|---|
| `82` | | firmware version; reply `ec 02 ascii[16]`, e.g. `AULA3-AR32-0207` |
| `b0` | | config table; reply `ec 30 00 00 table[60]` |
| `35` | `ch 00 shutdown mode` | mode of a motherboard effect channel |
| `36` | `maskHi maskLo shutdown rgb[...]` | effect colour of the LED slots set in the 16-bit mask; slot `i` at byte `5 + 3i` |
| `3b` | `ch 00 mode r g b` | mode and colour of a header (addressable controller) |
| `3f` | `55` | save the current effects to flash (motherboard controller) |
| `40` | `flags offset count rgb[count]` | direct colours, at most 20 LEDs per report |
| `52` | `53 00 01` | drive the addressable headers as Gen 1 |

`shutdown` = 1 sets the effect shown while the PC is off (only for the
motherboard's own LEDs); this driver always sends 0.

## Config table

Bytes of the 60-byte table (reply bytes 4–63):

| Byte | Meaning |
|---|---|
| `02` | addressable (5 V) headers |
| `1b` | motherboard LEDs, including one per 12 V RGB header |
| `1d` | 12 V RGB headers: the last LEDs of `1b` (ignored if larger than `1b`) |

The rest is not understood. The driver logs the table and refuses tables
with more than 8 headers, or (motherboard controller) more motherboard LEDs
plus headers than the 16 slots of `36`.

## Zones

Motherboard controller:

- The motherboard zone exists if byte `1b` is non-zero. Direct channel `04`
  covers all its LEDs (own LEDs first, then the RGB headers); colour slots
  `0..leds`.
- Header `i` (0-based): direct channel `i`, colour slot `leds + i` (one slot
  per header: the whole strip takes one colour).
- Effect channels: firmware `AULA3-AR32*` numbers the headers from 0 and gives
  the motherboard the next number; other firmware gives the motherboard 0
  and the headers 1 up. Without motherboard LEDs, headers start at 0.

Addressable controller: header `i` is effect channel and direct channel `i`.

The controller doesn't know how many LEDs a strip has; the user sets it per
header (0 by default, at most 120 here), and the front-end remembers it.

## Effects

Mode byte of `35` / `3b`. There is no speed or direction.

| Mode | Name | Colour |
|---|---|---|
| `00` | Off | |
| `01` | Static | yes |
| `02` | Breathing | yes |
| `03` | Flashing | yes |
| `04` | Spectrum cycle | |
| `05` | Rainbow | |
| `06` | Spectrum cycle breathing | |
| `07` | Chase fade | yes |
| `08` | Spectrum cycle chase fade | |
| `09` | Chase | yes |
| `0a` | Spectrum cycle chase | |
| `0b` | Spectrum cycle wave | |
| `0c` | Chase rainbow pulse | |
| `0d` | Random flicker | |
| `0e` | Music (not offered) | |
| `10` | Gentle transition | |
| `11` | Wave propagation | |
| `12` | Wave propagation pause | |
| `13` | Red pulse | |
| `ff` | Direct (hands the channel to `40`) | |

On the motherboard controller an effect is `35` (mode) followed by `36`
(colour for the zone's slots). It shows at once; `3f 55` stores it.

## Direct colours

`ec 40 flags offset count rgb...`: `flags` is the direct channel, OR `80` on
the last report of a frame (which shows it) and OR `10` once the LED offset
passes 255 (the offset byte then wraps). Each zone is first switched to mode
`ff`. Before streaming to addressable headers the driver sends `52 53 00 01`
once: Gen 2 headers otherwise size the strip themselves.

## What the driver does

- Opening reads `82` and `b0` and fails if either doesn't answer with the
  expected reply byte and length.
- Effects can't be read back. The driver shows the last effects set in
  OpenDeviceControl (the front-end remembers them and re-applies them on
  opening), Rainbow before that.
- Built-in effects apply at once. `3f 55` is sent on release, only on the
  motherboard controller and only if an effect was changed in the session.
- Release puts every zone back on its built-in effect if direct colours or
  effect changes were sent.
- Never sent: firmware update or bootloader commands, the shutdown effect.

## Uncertain

- Everything above comes from OpenRGB and liquidctl, not from captures of
  Armoury Crate made for this driver.
- Whether `52 53 00 01` (Gen 1) persists, and how to switch back to Gen 2.
- What the firmware does with an effect channel that isn't on the board.
- liquidctl notes that animated modes may run on all channels at once and
  only Static can be set per channel; per-zone animated effects may not work.
- Whether the addressable controller keeps effects across power cycles
  (there's no known save command).
- How long the controller keeps direct colours without new frames; the
  driver resends the last frame every second.
- The previews of built-in effects are approximations.
