# Aula F99 keyboard

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
