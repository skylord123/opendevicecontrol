# Corsair NXP family (shared protocol)

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
