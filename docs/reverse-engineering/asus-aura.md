# ASUS Aura (ENE) lighting

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
