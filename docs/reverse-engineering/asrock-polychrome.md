# ASRock Polychrome RGB (USB)

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
