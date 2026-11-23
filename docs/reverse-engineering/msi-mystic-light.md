# MSI Mystic Light (USB, motherboards)

Driver: `vendors/msi/`. The LED controller on MSI motherboards, `1462:xxxx`
where the product id is the board's MS-number (`7c91` is the MS-7C91, MAG
B550 TOMAHAWK), or `0db0:0076` on recent boards, whose serial starts with
the board's id.

**Not tested on hardware.** Everything here comes from OpenRGB's protocol
work (`Controllers/MSIMotherboardController/`, read as documentation, no
code taken) and the "MSI Motherboards Mystic Light Knowledge Collection" in
Thomas Boos's OpenRGB wiki, which documents Wireshark captures of MSI's
software. The board table (zones, LED counts, direct mode per board id) is
OpenRGB's.

## Reports

Everything is a HID feature report. Report `52` holds every zone's built-in
effect. Its length tells the protocol apart, so the driver reads it before
it writes anything:

| `52` length | Protocol | Driver |
|---|---|---|
| 112 | oldest boards | not supported (no known board ids) |
| 162 | "V1", Z390/X399/B450 era | effects, zone-based direct colours |
| 185 | "V2", X570 and later | effects, per-LED or zone-based direct colours |
| other (761-byte controller) | X870/B850/Z890 on `0db0:0076` | not supported, see below |

Writing `52` applies the effects. Its last byte is the save flag: 1 stores
them in flash. Reports go out at exactly their length (162 or 185 bytes,
report ID included). A 184-byte write is what bricked controllers in
OpenRGB's early days.

### Zone record

10 bytes: `effect r g b sb r2 g2 b2 flags pad`.

- `sb`: speed in bits 0-1 (0 slow to 2 fast), brightness in bits 2-6 (0 to
  10, 10 % steps), bit 7 "JRGB follows on-board".
- `flags`: bit 7 uses the colour (clear: the effect cycles colours). Bits
  0-5 are sync flags: on-board, JRAINBOW1, JRAINBOW2, JCORSAIR, JPIPE1,
  JPIPE2.
- On 185-byte boards the JRAINBOW records have an 11th byte, the header's
  LED count. JRAINBOW3 uses the JCORSAIR record with `pad` = 4.

### 162-byte layout

| Offset | Record |
|---|---|
| 1 | JRGB1 |
| 11 | JRAINBOW1 |
| 21 | JCORSAIR |
| 31 | JCORSAIR outer (LL120) |
| 41 | on-board (leads the others) |
| 51 + 10·i | on-board LED i+1 (1-10) |
| 141 | on-board LED 10, or JRAINBOW2 |
| 151 | JRGB2 |
| 161 | save flag |

### 185-byte layout

| Offset | Record |
|---|---|
| 1 | JRGB1 |
| 11 / 21 | JPIPE1 / JPIPE2 |
| 31 / 42 | JRAINBOW1 / JRAINBOW2 (11 bytes, count at +10) |
| 53 | JCORSAIR, or JRAINBOW3 (11 bytes) |
| 64 | JCORSAIR outer (LL120) |
| 74 | on-board (leads the others) |
| 84 + 10·(i-1) | on-board LED i (1-9) |
| 174 | JRGB2 |
| 184 | save flag |

Boards from `7d03` on put JRGB2 in on-board LED 6's record (134) and that
LED in the JCORSAIR outer record (64).

MSI's software sends two `52` reports for each change: the board's current
one, then the new one. The driver does the same on 185-byte boards.

## Effects

Effects 1-5 (1-4 on 162-byte boards) play in one zone. Later ones are
"multi-zone": the on-board record leads, with its JRGB and pipe sync flags
set (185), or the zone's own `sb` bit 7 and flags bit 0 set (162). Boards
without an on-board zone still lead from the on-board record: OpenRGB copies
the JRGB or pipe zone's effect into it. The driver follows OpenRGB here.

| Byte | 185-byte name | 162-byte name | Colour |
|---|---|---|---|
| 01 | Static | Static | fixed |
| 02 | Breathing | Breathing | fixed |
| 03 | Flashing | Flashing | optional |
| 04 | Double flashing | Double flashing | optional |
| 05 | Lightning | Lightning | fixed |
| 07 | Meteor | Meteor | optional |
| 08 | | Stack | optional |
| 0f | Color ring | Rainbow | none / optional |
| 10 | Planetary | Planetary | none |
| 11 | Double meteor | Double meteor | none |
| 12 | Energy | Energy | none |
| 13 | Blink | Blink | optional |
| 14 | Clock | Clock | none |
| 15 | Color pulse | Color pulse | optional |
| 16 | Color shift | Color shift | none |
| 17 | Color wave | Color wave | optional |
| 18 | Marquee | Marquee | fixed |
| 1a | Rainbow wave | Rainbow wave | none |
| 1b | Visor | Visor | optional |
| 1d | Rainbow flashing | Rainbow flashing | none |
| 1e | | Rainbow double flashing | none |
| 23 | Color ring double flashing | | none |
| 24 | Stack | | optional |
| 25 | (per-LED mode) | | |

"Off" in the driver is Static at brightness 0. Effect 0 (disabled) is read
as Off and never written. Fire is left out: OpenRGB sends 0x26 but the wiki
lists 0x28.

## Direct colours

**Per-LED mode** (185-byte boards marked per-LED in the table). A `52`
report with every record static and fixed flags, the on-board record
`25 00 00 00 a9 00 00 00 bf 00`, switches it on. After that, 725-byte `53`
reports carry `53 25 area index 00` and 240 RGB triples:

- Synchronised (JRAINBOW1/2 up to 40 LEDs, JCORSAIR/JRAINBOW3 up to 120):
  one report, area 06. LEDs in order: on-board, pipe 1, pipe 2, JRGBs (per
  the board table), then fixed slots of 40, 40 and 120 for the headers.
  The switch report sets the JRAINBOW counts to 40 and the JCORSAIR one to
  120.
- Unsynchronised (larger headers): the headers' records get effect 25 and
  counts 200, 240, 240, and the on-board flags `b1`. Each header has its
  own report (area 04 index 0 or 1 for JRAINBOW1/2, area 05 for
  JCORSAIR/JRAINBOW3), then the on-board one (area 06). OpenRGB waits 13 ms
  between them.

The next ordinary `52` report brings the built-in effects back.

**Zone-based** (162-byte boards and older 185-byte ones). The driver writes
`52` reports with every zone static at full brightness in its colour and
each on-board LED in its own record. That gives one colour per header.

## Other commands

Firmware version: a 64-byte output report `01 b0 cc...` (application) or
`01 b6 cc...` (loader). The reply's byte 2 is the version, high nibble
major. OpenRGB requires 6 or later on `7c56` before it drives that board,
and the driver does the same. No other command is sent. In particular,
nothing touches the controller's bootloader.

## Not supported

- **761-byte controller** (`0db0:0076` interface on usage page `ff00`, MSI
  X870/B850/Z890). OpenRGB drives it with a 290-byte `50` configuration
  report copied from SignalRGB and 761-byte `51` per-LED reports, and
  matches boards by DMI name. Its detection writes a zeroed `51` report.
  There is no known way to read the current configuration back, and no way
  to return to the built-in effects. The driver can't do that safely, so it
  skips these boards (it reads `52` and finds no supported length).
- **112-byte boards**: same records as 162 with fewer zones, but no known
  board ids or zone table.
- OpenRGB's untested ids (`3ea4`, `4459`, `7b10`, `7b94`, `7b96`, `7c42`,
  `7c70`, `7c85`, `7c88`, `7c89`, `7c96`, `7c99`, `905d`) are not opened.

## Uncertain

- Which multi-zone effects take over which zones when zones run different
  effects. OpenRGB only ever sets one effect for every zone.
- The JRAINBOW count byte in built-in effects: an LED count or a cycle
  length.
- JCORSAIR: its LED count isn't stored on the board, and its fan flags are
  left as read.
- Whether per-LED mode times out. The driver resends the last frame every
  second.
- Boards missing from the table: 185-byte boards show every zone and get no
  direct colours. 162-byte boards get the common Z390 zone set with 10
  on-board LEDs.
