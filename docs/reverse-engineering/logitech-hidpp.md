# Logitech HID++ (mice, keyboards, receivers)

Logitech devices speak HID++ over a vendor HID collection, whether they sit
behind a Unifying, Bolt, Nano or Lightspeed receiver, hang on a USB cable or
connect over Bluetooth. OpenDeviceControl's driver
(`crates/opendevicecontrol-core/src/vendors/logitech/`) talks HID++ itself;
nothing from Logitech or Solaar has to be installed.

Sources: the protocol is public through
[Solaar](https://github.com/pwr-Solaar/Solaar) (GPL-2.0-or-later). The
driver follows Solaar 1.1.19's `logitech_receiver` package for which feature
holds which setting and how its bytes are encoded. The name tables in
`keys.rs` and the model table in `devices.rs` were generated from Solaar's
`special_keys.py` and `descriptors.py`. Everything listed under
"Checked on hardware" was confirmed with an MX Master 3S (Bluetooth, and
paired to a Bolt receiver) and a Performance Mouse MX on a Unifying receiver.

## Transport

| Report | Id | Length | Use |
|---|---|---|---|
| Short | `0x10` | 7 bytes | HID++ 1.0 register access, receiver notifications |
| Long | `0x11` | 20 bytes | HID++ 2.0 feature calls (always used here), long registers |
| Very long | `0x12` | 64 bytes | some gaming devices |

Layout: `[report id, device index, sub id | feature index, address | function<<4 | software id, params…]`.

- **Device index:** `0xFF` is the receiver itself, or the device on a cable or
  Bluetooth link. `1..=6` are a receiver's slots.
- **Software id:** the low nibble of the function byte. Replies echo it, and the
  device's own notifications carry 0. OpenDeviceControl uses `0x0A`; the Linux
  kernel uses 1 and Solaar cycles through 2..15. That keeps replies apart when
  the programs share a receiver.
- **Errors:**
  - HID++ 2.0: `[.., 0xFF, feature index, function|swid, code]`. Codes: 2 invalid
    argument, 3 out of range, 5 Logitech internal, 7 invalid function,
    8 busy, 9 unsupported.
  - HID++ 1.0: `[.., 0x8F, sub id, address, code]`. For a slot whose device is
    asleep or switched off, the receiver answers 0x04, 0x08 or 0x09; the
    driver reads those as "offline".
- **Protocol version:** a ping (root feature, function 1, `00 00 5A`) returns the
  HID++ 2.0 version. A HID++ 1.0 device answers with 1.0 error 0x01 "invalid
  sub id".
- **Collections:**
  - Linux hidraw: one node per interface carries both report sizes.
  - Windows: separate HID paths for the short (`FF00:0001`) and long
    (`FF00:0002`) collections. The driver opens both and groups them by path
    minus `&colNN` and the last instance-id part.
  - Bluetooth devices expose only long reports, on usage page `0xFF43`.
- **Linux, logitech-dj:** the driver also makes a hidraw node for each device
  behind a receiver. Those nodes sit under the receiver's HID device in sysfs
  and are skipped; the devices are reached through the receiver at their
  slot index.

## Receivers (HID++ 1.0 registers, device index 0xFF)

Register numbers `0x2xx` are "long" registers: read with sub id 0x83,
written with 0x82 and a long report.

| Register | Read / write | Meaning |
|---|---|---|
| `0x00` | w `00 09 00` | notification flags: wireless (connections) + software present |
| `0x02` | r; w `02` | `[_, connected count, _]`; writing 2 makes every connected device send a connection notification |
| `0x2B5` sub `0x03` | r | receiver info: serial in bytes 1..5, max devices in byte 6 |
| `0x2B5` sub `0x20+n-1` | r | Unifying-style pairing info of slot n: wpid in bytes 3..5 (BE), kind in byte 7 low nibble, report interval in ms in byte 2 |
| `0x2B5` sub `0x30+n-1` | r | serial (bytes 1..5) and power-switch location |
| `0x2B5` sub `0x40+n-1` | r | codename: length in byte 1, text after |
| `0x2B5` sub `0x50+n` | r | **Bolt** pairing info of slot n: kind (byte 1 low nibble), wpid bytes 2..4 little-endian, serial bytes 4..8 |
| `0x2B5` sub `0x60+n`, param 1 | r | **Bolt** codename: length in byte 2, text from byte 3 |
| `0x2FB` | r | **Bolt** unique id (its serial) |
| `0xB2` | w `01 00 tt` / `02 00 00` / `03 slot` | open pairing lock for tt s / close / unpair slot |
| `0xC0` | w `tt 01` / `00 02` | **Bolt** start / cancel discovery |
| `0x2C1` | w `01 00 addr[6] auth entropy` / `02` / `03 slot` | **Bolt** pair the discovered device / cancel / unpair |
| `0xF1` | r `01` | firmware version (bytes 1..3) |

Receiver notifications (short reports, software id 0):

| Sub id | Meaning |
|---|---|
| `0x41` | device connection: `address` = protocol, data[0] bit 6 set = link down, data[1..3] wpid (LE) |
| `0x40` | address 2: the slot was unpaired |
| `0x4A` | pairing lock: address bit 0 = open; data[0] error (1 timeout, 2 unsupported, 3 too many, 6 sequence timeout) |
| `0x4F` | **Bolt** device found: data[1]=0 → kind data[3], address data[6..12], authentication data[14]; data[1]=1 → name (length data[2]) |
| `0x4D` | **Bolt** passkey (6 ASCII digits) to type on a keyboard, or for a mouse to click as 10 bits, left = 0 and right = 1 |
| `0x53` | **Bolt** discovery status, data[0] error |
| `0x54` | **Bolt** pairing status: address 2 = paired (slot in data[7]); data[0] error (1 timeout, 2 failed) |

Bolt pairing flow:

1. Start discovery.
2. On 0x4F, pair with the address and authentication it gave. Entropy is 20 for keyboards and 10 for others.
3. Show the 0x4D passkey.
4. 0x54 ends it.

Unifying-style receivers just open the lock, and the new device appears with 0x41.

## HID++ 2.0 features

Root (index 0) function 0 maps a feature id to its index. FEATURE_SET
(`0x0001`) function 0 gives the count and function 1 the id, flags and
version at each index. Feature calls use function numbers 0..15. Settings
and their byte layouts, as used by the driver:

| Feature | Read fn → layout | Write fn ← layout |
|---|---|---|
| `0x0003` FW version | 0 → count, unit id [1..5]; 1(i) → type, prefix[3], major, minor, build(2) | |
| `0x0005` name | 0 → length; 1(offset) → text; 2 → kind (0 keyboard, 3 mouse, 4 touchpad, 5 trackball…) | |
| `0x1004` unified battery | 1 → percent, level bits (8 full, 4 good, 2 low, 1 critical), status (1 charging, 2 almost full, 3 full, 4 slow charge), external power | event fn 0 same |
| `0x1000` battery | 0 → level, next level, status | |
| `0x1001` battery voltage | 0 → mV(2), flags (bit 7 charging) | |
| `0x1814` change host | 0 → host count, current host | 1(host), no reply |
| `0x1815` hosts info | 0 → flags (bit 0 names readable), _, count, current; 1(host) → name length in [4]; 3(host, offset) → 14-byte name pieces from [2] | |
| `0x1B04` reprog controls v4 | 0 → count; 1(i) → cid(2), task(2), flags, pos, group, group mask, flags2; 2(cid) → cid, reporting flags, mapped-to(2) | 3 ← cid(2), flags, remap(2) |
| `0x1D4B` wireless status | | event fn 0: data[1]=1 asks for software reconfiguration (settings were lost) |
| `0x2110`/`0x2111` SmartShift | 0/1 → mode (1 free-spin, 2 ratchet), threshold, default; enhanced: torque | 1/2 ← mode, threshold (255 always ratchet), torque |
| `0x2121` hi-res wheel | 0 → caps; 1 → mode bits: 1 diverted, 2 hi-res, 4 inverted | 2 ← mode |
| `0x2150` thumb wheel | 0 → native res(2), diverted res(2), default direction, caps (bit 3 single tap); 1 → diverted, inverted | 2 ← diverted, inverted |
| `0x2201` adjustable DPI | 1(sensor, _, page) → sensor, DPI list; 2 → sensor, DPI(2), default(2) | 3 ← sensor, DPI(2) |
| `0x2202` extended DPI | 1 → caps (bit 0 separate Y, bit 1 lift-off); 2(…, direction, page) → list; 5 → X, Y, lift-off | 6 ← 0, X(2), Y(2), LOD |
| `0x2205` pointer speed | 0 → speed(2) (256 normal) | 1 ← speed(2) |
| `0x8060` report rate | 0 → bitmap of supported ms (bit n = n+1 ms); 1 → current | 2 ← ms |
| `0x8061` extended report rate | 1 → bitmap (8 ms, 4, 2, 1, 500 µs, 250, 125); 2 → index | 3 ← index |
| `0x8100` onboard profiles | 2 → mode (1 onboard); 4 → current sector; 5(page, offset) → directory, 4-byte entries sector(2), enabled, _ | 1 ← mode (1 onboard, 2 host); 3 ← sector(2) |
| `0x40A0`/`0x40A2` Fn inversion | 0 → inverted | 1 ← inverted |
| `0x40A3` K375s Fn inversion | 0(host) → host, inverted | 1 ← host, inverted (current host from `0x1815`) |
| `0x1982` backlight 2 | 0 → enabled, options (mode bits 3..4), supported, effects(2 LE), level, delays hands-out/in/powered (2 LE each, 5 s units); 2 → levels | 1 ← enabled, options, 0xFF, level, three delays |
| `0x1983` backlight 3 | 1 → seconds(2) | 2 ← seconds(2), 0x09 |
| `0x4521` disable keys | 0 → supported bits; 1 → disabled bits | 2 ← bits |
| `0x4531` multiplatform | 0 → flags (bit 1 settable), _, descriptor count, …, current [6]; 1(i) → platform, _, OS bits(2), versions | 3 ← 0xFF, platform |
| `0x4530` dual platform | 0 → platform | 2 ← platform |
| `0x4600` crown | 1 → mode (1 normal, 2 diverted), smooth (1 on, 2 off) | 2 ← mode / 0, smooth |
| `0x8010` G-keys | 0 → count | 2 ← divert (no read) |
| `0x8020` M-key LEDs, `0x8030` MR LED | 0 → count | 1 / 0 ← bits (no read) |
| `0x8040` brightness | 0 → max(2), _, flags (bit 2 separate on/off), min(2); 1 → value; 3 → on | 2 ← value; 4 ← on |
| `0x19B0` haptic | 1 → enabled, level, flags (bit 0 four steps only) | 2 ← enabled, level |
| `0x19C0` force sensing | 0 → count; 1(i) → caps, default(2), max(2), min(2); 2(i) → current | 3 ← i, value(2) |
| `0x8300` sidetone | 0 → level | 1 ← level |
| `0x8310` equalizer | 0 → bands, dB range, _, min, max; 1(first) → frequencies; 2 → gains | 3 ← 2, gains |
| `0x1F20` ADC | 0 → mV(2), flags; 1 → power-off minutes (v2+) | 2 ← minutes |

DPI lists are 16-bit values ended by 0. A value whose top three bits are `111`
is a step: the next value is the end of a range from the previous value in
that step.

### Buttons (`0x1B04`)

- **Key flags** (byte 4, plus byte 8 << 8): 0x01 mouse button,
  0x10 reprogrammable, 0x20 divertable, 0x40 persistently divertable,
  0x80 virtual, 0x100 raw XY, 0x200 force raw XY.
- **Setting reporting:** each reporting bit has a "valid" bit just above it.
  Diverted = 0x01 (valid 0x02), persistently diverted = 0x04 (0x08), raw XY =
  0x10 (0x20). Writing `cid, 0x03, 0, 0` diverts a button. A remap target
  of 0 leaves the mapping as it is.
- **Remapping:** a control can take the task of any control whose group (1..8)
  has its bit set in the control's group mask.
- **Events:**
  - Function 0 lists up to four diverted controls held down (cid, 2 bytes each);
    presses and releases are the differences between reports.
  - Function 1 gives raw movement dx, dy (signed 16-bit, big-endian) while a
    raw-XY button is held. The first report after the press is spurious on
    v5+ (Solaar skips it too).

Gestures in OpenDeviceControl:

- The movement is scaled by `15 / DPI` (about 5 units per cm). Under 1.5 units
  of travel counts as a click; otherwise the main axis picks up, down, left or
  right.
- **Sliding DPI** steps through the DPI list by one entry per unit. A click
  without movement swaps back to the previous DPI.

**Wheel events** (feature function 0, software id 0):

- **Thumb wheel:** rotation (signed 16-bit), time (2), status (0/1 start,
  2 rotating, 3 stop), flags: 0x02 proximity, 0x04 touch, 0x08 single tap.
  One step is emitted per diverted-resolution / native-resolution units
  (120 / 18 → 6 on the MX Master 3S).
- **Hi-res wheel:** flags, delta (signed 16-bit).
- **Low-res wheel:** delta (signed 8-bit).
- **Crown:** turn in data[1], press state in data[6] (1..4 held, 5 released).
- **G-keys:** a 32-bit little-endian bitmap. M-keys: a byte. MR: a byte.

## HID++ 1.0 devices (registers on the device's slot)

| Register | Meaning |
|---|---|
| `0x01` | mouse flags: 0x40 smooth scroll, 0x02 side scroll; keyboards: hand detection (byte 2, `0x30` off) |
| `0x07` | battery: level (7 full, 5 good, 3 low, 1 critical), charging byte (0x21 charging, 0x22 full) |
| `0x0D` | battery: percent, _, status in the high nibble (0x50 charging) |
| `0x09` | Fn swap (bit 0 of byte 1) |
| `0x63` | DPI: `0x81..0x8F` = 100..1500 (Performance MX family) |
| `0xF1` | firmware: `01` major/minor, `02` build |

Which of these a model has comes from Solaar's descriptor table (`devices.rs`).

## Behaviour notes

- **Lost settings:** devices forget diversion, SmartShift and most other settings
  when they sleep, switch hosts or power off. OpenDeviceControl keeps what the
  user set in its state folder, keyed by the device's serial number. It
  re-applies those settings when the device connects (0x41 link up, or a
  `0x1D4B` reconfiguration request).
- **One device, two links:** a device seen on Bluetooth or a cable is the same
  one as its receiver slot if the serial numbers match (the Bolt slot serial
  equals the device's `0x0003` unit id). The slot then steps aside while the
  direct link is up, so the device appears once.
- **Releasing:** when OpenDeviceControl closes a device it un-diverts its buttons
  and wheels, so they work normally without it.
- **Solaar:** run one of the two programs, not both. Solaar re-applies its own
  diversion settings when a device connects.

## Checked on hardware

- **MX Master 3S over Bluetooth:**
  - Feature table: HID++ 4.5, 37 features.
  - Battery, name, firmware, hosts list.
  - DPI write and read-back (1000 → 1200 → 1000).
  - SmartShift threshold and ratchet mode.
  - Wheel and thumb-wheel inversion.
  - Gesture button in gesture mode: click, swipe up and left.
  - Back diverted.
  - Thumb wheel diverted: left/right steps, and touch not mistaken for a tap.
  - Everything un-diverted again on release.
- **MX Master 3S on a Bolt receiver:** slot info (wpid `B034`, kind 2, serial =
  unit id) and codename.
- **Performance Mouse MX on a Unifying receiver (HID++ 1.0):**
  - Pairing registers and connection notifications.
  - Battery register `0x07`.
  - DPI register `0x63` (900 ↔ 1000).
  - Smooth-scroll bit.

Not yet tested on hardware: pairing and unpairing, button remapping, sliding
DPI, keyboards (backlight, Fn keys, disable keys, multiplatform), gaming
features (report rate, onboard profiles, G-keys, brightness), headsets,
Windows.
