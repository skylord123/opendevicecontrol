# Bose QuietComfort Headphones (BMAP)

Bose QuietComfort Headphones (2023), Bluetooth product id `0x4075`, variant 1
(black), firmware 1.0.6. Bose only ships an Android/iOS app (Bose Music) to
configure them. The protocol here, BMAP (Bose Messaging and Protocol), was
recovered from the Android app and tested against real headphones from Linux.
OpenDeviceControl's driver is `crates/opendevicecontrol-core/src/vendors/bose/`.

BMAP is shared by Bose's recent headphones and earbuds, so most of this applies
to other models too. The app has no per-model feature table: it asks the
device which function blocks it has and which functions answer, and so does
the driver.

Detailed, byte-level notes per function block:

- [Settings](bose-bmap/settings.md): name, voice prompts, auto-off, noise
  cancelling, EQ, shortcut button, multipoint, self voice.
- [More settings](bose-bmap/settings-more.md): settings other models have
  (ANR, on-head detection, conversation mode, auto play/pause, auto answer,
  touch and volume controls, Teams button, EQ presets, speaker grouping).
  From the app's code only; the QC Headphones have none of them.
- [Audio modes](bose-bmap/audio-modes.md): Quiet, Aware and user modes,
  favourites, creating and deleting modes.
- [Products and earbuds](bose-bmap/products-and-earbuds.md): every Bose
  product id and name, which ones use SPP or BLE (with the GATT and LE-CoC
  transports), earbud batteries, in-ear and case lid, find-my-buds, spatial
  audio, the voice assistant.
- [Status, devices, audio, control](bose-bmap/status-devices-audio.md):
  battery, paired devices, playback, volume, power, notifications, the
  connection sequence, error codes and product ids.

## How it was done

1. The Bose Music app (`com.bose.bosemusic` 13.4.2, Android) was pulled from a
   phone with `adb pull` (base APK and splits).
2. The `classes*.dex` files were disassembled with the Android SDK's
   `dexdump -d`. The BMAP library (`com.bose.bmap.*`, in `classes2.dex`) is not
   obfuscated: packet builders are `messages/packets/*Packet`, reply parsers
   `messages/parsers/*` and `messages/responses/*`, enums `messages/enums/*`.
   The app layer around it is obfuscated (`defpackage.*`). Parts of the
   status/device notes were also read from a JADX decompile.
3. Enum values were taken from the enums' static initialisers
   (`BmapFunctionBlock`, `BmapFunction` with 289 functions, `BmapOperator`,
   and so on).
4. The headphones were then queried live from Linux over RFCOMM with
   read-only `Get` packets, one per function, to see what this model supports
   and to check every parser against real replies.

Never sweep with `Start` (op 5): `Start` makes the device do things. Even a
`Get` on a Start-only function can act; a `Get` of Notification `Reset` (9.1)
answered `Result` and apparently reset the notification settings. Never send
Control `FactoryDefault` (7.5) or DeviceManagement `ClearDeviceList` (4.7).

## Transport

BMAP runs over Bluetooth Classic RFCOMM, on the headphones' standard Serial
Port Profile service (UUID `00001101-0000-1000-8000-00805f9b34fb`, record name
"SPP Dev", channel 8 on this model). The app connects with
`createRfcommSocketToServiceRecord(SPP)` in
`com.bose.bmap.service.SppConnectionManager`.

The SDP records of the QC Headphones:

| Service | RFCOMM channel / PSM |
|---|---|
| Serial Port "SPP Dev" (listed twice) | 8: **BMAP** |
| `00000000-deca-fade-deca-deafdecacaff` | 14: answers `ff 55 …`, Apple iAP2, not BMAP |
| `9b26d8c0-a8ed-440b-95b0-c4714a518bcc` "SRfcomm" | 25 or 26 (it moved between two lookups): **Spotify Tap**, see [Shortcut button presses](#shortcut-button-presses-spotify-tap) |
| Hands-Free unit | 10 |
| Headset | 11 |
| A2DP sink, AVRCP | L2CAP PSM 0x19, 0x17 |

Channel 9 also answered BMAP queries though no record lists it; the driver
uses the SDP record.

The device id (BlueZ `Modalias`) is `bluetooth:v009Ep4075d0106`: `009E` is
Bose's Bluetooth SIG company id and `4075` the same product id BMAP reports.
The driver only talks to devices with that company id (Linux) or "Bose" in the
name (Windows), and only after the BMAP version query answers.

Platform notes:

- **Linux**: a plain user can open `AF_BLUETOOTH` RFCOMM and L2CAP sockets, so
  no udev rule or group is needed. Connected devices come from BlueZ over
  D-Bus (`org.bluez` `GetManagedObjects`). BlueZ has no client-side SDP lookup
  over D-Bus and `sdptool` is deprecated (it hung on the test machine), so the
  driver sends one SDP `ServiceSearchAttributeRequest` for UUID 0x1101,
  attribute 0x0004, over L2CAP PSM 1 itself, and reads the RFCOMM channel from
  the protocol descriptor list. It falls back to channel 8.
- **Windows**: Winsock `AF_BTH` with `SOCKADDR_BTH.serviceClassId` set to the
  SPP UUID lets Windows do the SDP lookup. Devices come from
  `BluetoothFindFirstDevice` (connected, audio/video class).

## Packets

```
[fblock, function, flags, length, payload…]
flags = (device << 6) | (port << 4) | operator
```

The app always sends device 0, port 0. Operators (`BmapOperator`):

| Op | Name | Use |
|---|---|---|
| 0 | Set | not used by the app for the functions here |
| 1 | Get | read; empty payload (a few take an index) |
| 2 | SetGet | write; answered with the new Status |
| 3 | Status | the device's answer, also sent unprompted |
| 4 | Error | `[code]`, or `[ff, block-specific code]` |
| 5 | Start | actions (switch mode, power off, connect…) |
| 6 | Result | end of a Start |
| 7 | Processing | a Start is under way |

Several packets can arrive in one read, and the device sends Status packets
on its own, so a reader must split the stream by the length byte and treat
every Status as an update whether or not it answers a request.

Function blocks (`BmapFunctionBlock`) and what the QC Headphones have
(`ProductInfo.AllFblocks` = `86 cc 03 ff`, a big-endian bitmask):

| Id | Block | QC Headphones |
|---|---|---|
| 0 | ProductInfo | yes |
| 1 | Settings | yes |
| 2 | Status | yes |
| 3 | FirmwareUpdate | yes (not used) |
| 4 | DeviceManagement | yes |
| 5 | AudioManagement | yes |
| 6 | CallManagement | yes (no functions answered) |
| 7 | Control | yes |
| 8 | Debug | listed, no answer |
| 9 | Notification | yes |
| 12–17 | HearingAssistance, DataCollection, HeartRate, PeerBud, Vpa, Wifi | no |
| 18 | Authentication | yes (cloud activation only) |
| 19, 22, 23, 26 | Experimental, Print, … | listed, no answer |
| 25 | (not in the app) | answers: battery diagnostics, see below |
| 31 | AudioModes | yes |

Error code 4 (`FuncNotSupp`) means the model doesn't have that function; 5
(`OpNotSupp`) that the operator is wrong for it. The full table is in
[status-devices-audio.md](bose-bmap/status-devices-audio.md#6-errors-bmappacketerror).

## Connecting

What the app does once RFCOMM is up (`qm0`, `in0` in the app layer):

1. ProductInfo BmapVersion Get `00 01 01 00` → `"1.1.0"`.
2. ProductInfo AllFblocks Get `00 02 01 00`.
3. ProductInfo ProductIdVariants Get `00 03 01 00` → `40 75 01`.
4. Notification ByFblock SetGet with Enable and the blocks it wants updates
   for, limited to those the device has. For the QC Headphones:
   `09 02 02 05 01 80 04 02 37` (blocks 0, 1, 2, 4, 5, 9, 18, 31).
   Without this the headphones don't push changes made with their buttons.
5. A GetAll (Start) per block to fill its state.

No authentication is needed for any of the settings, playback or mode
functions. The app uses the Authentication block only for Bose cloud
activation.

## What the QC Headphones support

From the Get sweep (`Get` on every function of every block, firmware 1.0.6).
Functions that answered `Error 04` are left out.

| Function | Live reply | Meaning |
|---|---|---|
| 0.1 BmapVersion | `"1.1.0"` | |
| 0.3 ProductIdVariants | `40 75 01` | `BoseProductId.Prince` ("Bose QuietComfort Headphones"), variant PrinceBlack |
| 0.5 FirmwareVersion | `"1.0.6-80+…"` | |
| 0.6, 0.7, 0.10, 0.12 | | MAC address, serial number, hardware revision "SOR", GUID |
| 1.2 ProductName | `00` "Bose QC Headphones" | flags byte, then UTF-8 |
| 1.3 VoicePrompts | `e1 00 01 81 5e 00 00` | on, US English, 8 languages available |
| 1.4 StandbyTimer | `00` | auto-off never |
| 1.5 Cnc | `0b 0a 03` | 11 steps, at step 10 |
| 1.7 RangeControl | `f6 0a 08 00 f6 0a 00 01 f6 0a 03 02` | EQ bass +8, mid 0, treble +3, each −10..+10 |
| 1.9 Buttons | `80 09 03 00 01 40 08 00 00 00 80` | shortcut button, long press = battery level |
| 1.10 Multipoint | `07` | on, can be switched off |
| 1.11 Sidetone | `01 02 0f` | self voice medium |
| 1.12 SetupComplete, 1.14 CncPersistence | `01` | |
| 2.2 BatteryLevel | `46 ff ff 00` | 70 %, time left unknown |
| 2.5 ChargerDetect | `00` | not charging |
| 2.13, 2.14 BatteryLog | ASCII log | |
| 4.1, 4.4, 4.8, 4.9, 4.14 | | connection, paired device list, pairing mode, this host's address, features |
| 5.1 Source, 5.3 Control, 5.4 Status, 5.5 Volume, 5.7 Latency | | `5.5 = 20 17`: volume 23 of 32 |
| 7.4 Power, 7.7, 7.8 | `01`, `00`, `02` | |
| 31.2 Capabilities | `02 02 00 00 00 09` | 2 Bose modes, 2 user slots; CNC and wind block per mode |
| 31.3 CurrentMode | `01` | |
| 31.8 Favorites | `04 03` | the button cycles modes 0 and 1 |

Block 25 is not in the app's `BmapFunctionBlock` enum, but answers about 50
functions: battery voltages and counters, a battery log, and component names
(`[0]Simulated,[1]QCC5120`, the Qualcomm QCC5120 Bluetooth SoC). It looks
like a factory diagnostics block; nothing in it is used.

## What OpenDeviceControl does with it

- **Sensors**: battery (per earbud and case on earbuds), battery time left
  (when known), charging, in-ear, case lid, playing.
- **Controls**: mode (the modes in the headphones, switched with Start
  `1f 03 05 02 <idx> 01`); noise cancelling and wind block of the current mode
  when it is a user mode; EQ bands; shortcut button action; voice prompts and
  their language; auto-off; multipoint; self voice; volume of the current
  source; spatial audio; voice assistant and wake word; and every other
  setting in [settings-more.md](bose-bmap/settings-more.md) that a model
  answers. Noise cancelling from Settings is shown instead when the
  headphones have no audio modes.
- **Shortcut button**: while it is set to Spotify Tap, its presses are an
  OpenDeviceControl key (see above).
- **Paired devices**: a switch per paired device (connect / disconnect with
  DeviceManagement Connect and Disconnect Start), marked "this computer" and
  "current source", and pairing mode.
- Changes made on the headphones arrive as unprompted Status packets after
  the notification Enable and update the controls; everything is also re-read
  every minute, since settings changed by another client are not pushed.
- The front-end never replays remembered toggle values for these devices
  (`Device::restore_toggles`): the headphones keep their own settings, and
  some toggles connect devices.
- If the link drops, polling fails and the device is closed; it is opened
  again when the OS reports the headphones connected.

Not exposed (yet): renaming, creating or deleting modes, favourites,
removing a pairing, now-playing text, power off, playback buttons, EQ
presets, find-my-buds, products that only speak BMAP over BLE. The protocol
for each is in the notes.

## Hardware results

Write tests on the QC Headphones from Linux, firmware 1.0.6, 2026-11-19:

| What | Result |
|---|---|
| Notification ByFblock SetGet `09 02 02 05 01 80 04 02 37` | Status `00 80 04 02 37`: accepted, the reply is the enabled set |
| Sidetone SetGet `01 0b 02 02 01 03`, then `… 01 02` | Status `01 03 0f`, then `01 02 0f`: Low, then back to Medium |
| RangeControl SetGet `01 07 02 02 07 00`, then `08 00` | Bass +7, then back to +8; mid and treble unchanged |
| ModeConfig Get `1f 06 01 01 <idx>` | **47-byte** Status (not 48): Quiet (prompt 1, level 0), Aware (prompt 2, level 10), two empty user slots (user-configurable, mutable `09`: CNC and wind block) |
| CurrentMode Start `1f 03 05 02 00 01` | Processing, then Status of **DefaultMode** (`1f 04`) = 0, then Result `[00]`. The headphones announce the mode. Switching mode also changes the default (start-up) mode |
| Settings Cnc after switching | `0b 00 03` in Quiet, `0b 0a 03` in Aware |

With the PC and a phone (running the Bose app) connected at once, 2026-11-19:

| What | Result |
|---|---|
| Mode changed on the headphones' button or from the phone | Pushed at once (`1f 03 03 01 <idx>`) to every connected BMAP client after the notification Enable |
| EQ, self voice and other Settings changed from the other client | **Not pushed**: neither client sees the other's change until it reads again. OpenDeviceControl re-reads every setting once a minute |
| AudioManagement Source (5.1) while the PC played audio | Still named the phone. Volume (5.5) sets the volume of that source, so it changed the phone's volume, while the headphones' own volume buttons changed the PC's. OpenDeviceControl labels the control with the source's name |
| ListDevices + Info | Phone: flags `01` (connected), the phone's Bluetooth name. This PC: flags `03` (connected, local), its Bluetooth name. Two other paired devices: flags `00`. The order of the list changes (most recently connected first) |
| Shortcut set to Spotify Tap, then pressed and held | Nothing arrives over BMAP (Status Button 2.12, Control BmapRole 7.3 and ClientInteraction 7.10 all answer `Error 04`; `ActionButtonPressEvent` in the app fires when the app *changes* the button). Nothing arrives as an AVRCP key on the PC either. The press goes to the "SRfcomm" service instead, see below |

So the noise-cancelling level counts *awareness*: 0 is full noise cancelling
(Quiet) and the top step lets the most sound in (Aware). The app's
`mapBetweenCncLevelAndWorldVolume` = `|level − (steps − 1)|` turns it into its
slider; OpenDeviceControl shows it the same way (higher = more noise
cancelling). In Settings Cnc the level follows the current mode.

## Shortcut button presses (Spotify Tap)

With the shortcut button set to Spotify Tap (ActionButtonMode 16), every
press and hold sends one message on the "SRfcomm" RFCOMM service
(`9b26d8c0-a8ed-440b-95b0-c4714a518bcc`) to whoever is connected to it,
normally the Spotify app on the phone. The Bose app has nothing to do with
it: it only checks that Spotify is installed. Nothing is sent on connect;
four presses gave four identical messages:

```
01 3e  "<32 hex digits>" 00  "QuietComfort Headphones" 00  "Bose" 00
type   length (62)
```

The 32-hex-digit string is presumably a Spotify Tap client id; it is left out
here because it may be specific to the unit.
No answer was sent and the headphones didn't repeat or complain.

OpenDeviceControl connects to that service (found by its UUID over SDP, as
the channel number moves) while the button is on Spotify Tap, and reports
each message as a press of a "Shortcut button" key that can run any
OpenDeviceControl action. Tested: a "type text" action ran on every hold.
Whether the phone's Spotify app still gets the press while OpenDeviceControl
is connected was not tested.

## Not verified

- The order of the three Latency fields (the app's order gives min > max).
- Formats only seen in code: StandbyTimer's 2-byte form, ModeConfig SetGet,
  SettingsConfig, NamesSupported.
- That the headphones push Status packets once notifications are enabled (the
  app relies on it; not yet observed).
- Other BMAP models: the driver reads what each model reports, but only the
  QC Headphones were tested.
