# BMAP notes: Status, DeviceManagement, AudioManagement, Control, Notification, errors, product id

Source: Bose Music 13.4.2 (`com.bose.bosemusic`), `classes2.dex` (package `com.bose.bmap.*`, the BMAP
library) and `classes.dex`/`classes3.dex`/`classes4.dex` (app layer, obfuscated `defpackage.*`).

Bluetooth addresses in the examples are replaced with `11:22:33:44:55:0x` placeholders.

Tooling: this part was read from a JADX decompile of the app (classes2 for the BMAP
library, classes/classes3/classes4 for the app layer). Where JADX failed it was
cross-checked against the `dexdump -d` disassembly.

Live data: a Get sweep of every function on a Bose QC Headphones, product 0x4075 variant 1,
fw 1.0.6, BMAP version "1.1.0". The sweep sent **Get (op 1)** to every function.

## 0. Framing recap (BmapPacket)

```
[0] fblock  [1] function  [2] (deviceId<<6)|(port<<4)|operator  [3] payloadLen  [4..] payload
```

`BmapPacket.toByteArray()` / `createFromByteArray()`. Every packet the app builds uses
deviceId 0, port 0 (the constructor default mask 12/44 sets both to 0), except
`DeviceManagementRoutingStartPacket` which uses port 1. Max payload 255; the SPP reader buffer
is 259 bytes (`SppConnectionManager.MAX_READ_BUFFER_SIZE`).

The app's SPP reader (`SppConnectionManager.ConnectedThread.receiveDataPackets`) splits one
`read()` into packets by the length byte and **drops any partial packet at the end of a read**.
A reimplementation should keep a proper reassembly buffer instead.

Multi-byte integers are **big endian** everywhere below. `ByteUtils.convertBytesToInt(a,b)` returns a
**signed** 16-bit value (`ByteBuffer.getShort`), so `ff ff` decodes as -1.

Bitsets (`BitSetUtil.fromByteArray` / `toByteArray`) are big-endian byte arrays: bit 0 is the
LSB of the **last** byte. Example: `86 cc 03 ff` = 0x86CC03FF, bit n set means id n is present.
Bitmasks built from an int (`BitSetUtil.fromInt`) are LSB-first.

Request/response matching in the app (`BmapInterfaceImplementation.sendCommandForResponse`) is by
response class only (no sequence numbers). The default timeout is 4000 ms.

---

## 1. Status, fblock 2

Parser: `messages/parsers/BmapStatusResponseParser.parse`. The app only parses these Status
functions: FblockInfo, GetAll, BatteryLevel, BatteryLog, AuxCableDetection, ChargerDetect,
MoistureDetection, MicMute, InEar, OperationMode, UnifiedCommunicationsLink, Button,
ChargingCaseLid. **BatteryCondition (15) and BatteryCapacity (17) are named in `BmapFunction`
but nothing in the app builds, parses or reads them** (grep finds them only in the enum).

### 1.1 BatteryLevel (2.2)

- Request: Get, no payload: `02 02 01 00`
- Reply: Status (op 3). Parsed by `StatusBatteryLevelResponse.Companion.getBatteryLevel`.
  - payload length 1..4: one entry; length > 4: consecutive 4-byte entries (one per component,
    used by earbuds and cases).
  - Entry layout:

| byte | field | notes |
|------|-------|-------|
| 0 | `percentage` | u8, 0..100 |
| 1..2 | `remainingPlayTimeInMinutes` | u16 BE; `0xFFFF` means unknown (null); absent if the entry is shorter |
| 3 | `componentId` | `ComponentId`: -1 Unknown, 0 Single, 1 RightBud, 2 LeftBud, 3 ChargingCase |

- Live: `02 02 03 04 46 ff ff 00` decodes as **70 %, remaining time unknown, component 0 = Single** (a
  headphone has only one battery).
- Unsolicited: the parser accepts a Status at any time and updates the battery relay, and the app
  enables Status notifications on connect (see section 5), so battery changes are pushed as Status
  packets in this exact format.

### 1.2 ChargerDetect (2.5)

- Get: `02 05 01 00`. Reply Status, 1 byte: `01` = charger plugged in, anything else = not
  (`StatusChargerDetectResponse`: `payload[0] == 1`).
- Live: `00`, not charging.

### 1.3 ChargingCaseLid (2.10)

- Get: `02 0a 01 00`. Reply Status, 1 byte `LidStatus`: 0 Unknown, 1 Open, 2 Closed (unknown
  values map to Unknown).
- Live: `00` Unknown, as expected since a headphone has no case. The Get packet class is
  `StatusChargingCaseLidGetPack`. I did not trace its callers; it is presumably used for earbud cases.

### 1.4 BatteryCondition (2.15): not used by the app

- Live Get reply: Status `00 60 00 00`. The same 4 bytes appear in the undocumented fblock 25
  function 59 (`25.59`) and inside `25.62`.
- No parser in the app, so the meaning is unknown. A possible reading is a u32 BE `0x00600000`, or
  flags/state plus a percentage-like 0x60 = 96 (battery health 96 %?). **Unverified.**

### 1.5 BatteryCapacity (2.17): not used by the app

- Live Get reply: Status `01`. No parser, so the meaning is unknown (maybe a capacity class or
  index). **Unverified.**

### 1.6 Other Status functions

- GetAll (2.1): the app sends **Start** `02 01 05 00` (`StatusGetAllStartPacket`). The device
  answers with Processing, then a Status packet for each supported Status function, then Result
  (`StatusGetAllCompletedResponse`).
- BatteryLog (2.13) / BatteryLogRaw (2.14): ASCII text, for example `"3 626,9,25,255,0,194,0,0,0,0,Y 621,..."`, NUL
  terminated. The app keeps it as a string (`sendGetBatteryLog`) and never parses it further.

---

## 2. DeviceManagement, fblock 4

Parser: `BmapDeviceManagementResponseParser`. Packets: `messages/packets/DeviceManagement*Packet`.
State handling: `defpackage/Lm2` (`C0156Lm2.a(...)` dispatch, `r()`, `s()`).
MAC addresses are 6 bytes in normal (display) order, e.g. `11 22 33 44 55 01` =
`11:22:33:44:55:01`.

### 2.1 Connect (4.1)

Get (capability query): `04 01 01 00`, reply Status (`DeviceManagementConnectStatusResponse`):

| byte | meaning |
|------|---------|
| 0 | bit5 = MusicShareMode.MusicSamePid supported, bit6 = MusicSameType supported |
| 1 | `> 0` means DWS (Bose "party mode"/music share) supported |
| 2.. | not parsed |

Live `00 00 03`: no music share, no DWS, byte 2 = 3 not parsed (maybe a maximum connection count,
**unverified**).

Start (connect a paired source), `DeviceManagementConnectStartPacket.create`:

- by MAC (what the app uses): payload `00 <mac6>` → `04 01 05 07 00 aa bb cc dd ee ff`
- by name: `01 <utf8 name>`
- Bose product to product: `((productType<<7)|0x10) <peer mac6> <local mac6>` (13 bytes),
  productType 0 Headphones, 1 Speaker.

Responses: Processing (empty) first, then Result (`DeviceManagementConnectResultResponse`):

| byte | meaning |
|------|---------|
| 0..5 | MAC |
| 6 | bit0 A2DP, bit1 HFP, bit2 AVRCP, bit3 SPP, bit4 iAP connected |
| 7 (optional) | bit0 LE Audio unicast, bit1 LE Audio broadcast |

Error replies use the generic error format (section 6), for example DeviceNotFound 11 or
NoconnTimeout 13.

### 2.2 Disconnect (4.2)

- Start `04 02 05 06 <mac6>` (`DeviceManagementDisconnectStartPacket`).
- Processing (`DeviceManagementDisconnectProcessingResponse`), exactly 7 bytes:
  `<reason u8> <mac6>`. `DisconnectReasonCode`: 0 Unknown, 1 ManualPowerOff, 2 AutoOff,
  3 BatteryOff, 4 Restart, 5 Safety, 6 OtaUpdate, 7 Charging, 0x10 PeerLinkLoss,
  0x20 InsufficientSlots, 0x21 AppTriggered.
- Result: `<mac6>`.
- The device may also send Processing/Result unsolicited when a source drops, and `Lm2.m()/n()`
  handles them without a request.

### 2.3 RemoveDevice (4.3)

- Start `04 03 05 06 <mac6>` → Processing (empty) → Result `<mac6>`.
- This unpairs the device from the headphone's paired device list.

### 2.4 ListDevices (4.4)

- Get `04 04 01 00` → Status (`DeviceManagementListDevicesResponse.Companion.createFromPacket`):
  - payload length 1 means the list is empty (`StatusEmptyList`).
  - otherwise byte 0 is a connected bitmask, then N x 6-byte MACs. Device i is connected when
    `(byte0 >> i) & 1`. The app then sends **Info (4.5) for every MAC** (`Lm2.r()` →
    `requestGetInfo`) to fill in names and flags.
- **Live decode** `04 04 03 19 01 11 22 33 44 55 01 11 22 33 44 55 02 11 22 33 44 55 03 11 22 33 44 55 04`:

| idx | MAC | connected bit |
|-----|-----|---------------|
| 0 | 11:22:33:44:55:01 | 1 (connected) |
| 1 | 11:22:33:44:55:02 | 0 |
| 2 | 11:22:33:44:55:03 | 0 |
| 3 | 11:22:33:44:55:04 | 0 |

  `11:22:33:44:55:02` is this PC's controller (`bluetoothctl list`) and equals the AppAddress
  reply (4.9), so it is the host the RFCOMM link runs on. Its connected bit is 0, although the PC is
  linked over SPP. Device 0 is also the current audio source (AudioManagement Source reply, 3.1). So
  the bit probably means "audio profile connected" and not "any link". **Treat this as unverified.** For
  exact per-profile state, use ExtendedInfo (4.6).

### 2.5 Info (4.5): how to get a paired device's name

- Get `04 05 01 06 <mac6>`. The reply can come as Processing, Status or Result; the app parses all three
  the same way (`DeviceManagementInfoProcessingStatusResultResponse`):

| byte | meaning |
|------|---------|
| 0..5 | MAC |
| 6 | flags: bit0 connected, bit1 isLocalDevice (the device this BMAP link runs on), bit2 isBoseProduct, bit3 isComponent, bit7 ProductType (only if Bose product: 0 headphones, 1 speaker) |
| 7..8 | if Bose product: BoseProductId u16 BE; otherwise Bluetooth major (7) and minor (8) device class |
| 9 | if Bose product: product variant u8; otherwise the name starts here |
| 10.. / 9.. | **device name, UTF-8, to the end of the payload** (offset 10 for Bose products, 9 for everything else) |

Example (made up): `04 05 03 0e 11 22 33 44 55 01 01 05 0c 50 69 78 65 6c` gives MAC
11:22:33:44:55:01, connected, not Bose, class 05/0c, name "Pixel".

### 2.6 ExtendedInfo (4.6)

- Get `04 06 01 06 <mac6>` → Status (`DeviceManagementExtendedInfoStatusResponse`), at least 8 bytes.
  **No name in this reply.** Use Info (4.5) for the name.

| byte | meaning |
|------|---------|
| 0..5 | MAC |
| 6 | paired profiles: bit0 A2DP, bit1 HFP, bit2 AVRCP, bit3 SPP, bit4 iAP |
| 7 | connected profiles: same bit layout |
| 8..9 | not parsed |
| 10 (opt) | paired LE: bit0 LE unicast, bit1 LE broadcast |
| 11 (opt) | connected LE: bit0 unicast, bit1 broadcast |

### 2.7 ClearDeviceList (4.7)

- Start `04 07 05 00` (`DeviceManagementClearDeviceListStartPacket`). The parser returns
  `BmapNotImplementedResponse` and the app never calls it. It probably wipes all pairings, so **do not use
  it casually**.

### 2.8 PairingMode (4.8)

- Get `04 08 01 00`. Reply (`DeviceManagementPairingModeStatusResultResponse`): byte 0 `== 1` means
  in pairing mode.
  - Live: the device answered the Get with **Result** (op 6), not Status: `04 08 06 02 00 03`, not in
    pairing mode. Byte 1 (`03`) is not parsed. The parser accepts Status or Result.
- Start `04 08 05 01 01` enters pairing mode (`togglePairingMode(true)`), `... 01 00` leaves it. The
  device sends Processing, then Status or Result with the new state.

### 2.9 AppAddress (4.9)

- Get `04 09 01 00` → Status, exactly 6 bytes: the Bluetooth address the headphone sees for the
  host running BMAP. Live: `11 22 33 44 55 02` = 11:22:33:44:55:02 (this PC).

### 2.10 Features (4.14)

- Get `04 0e 01 00` → Status, 1 byte (`FeatureInfo`): bit0 CTKD supported, bit1 device carousel
  supported, bit2 source barge-in supported.
- Live `02`: device carousel only.

Other functions: PrepareP2p (10), P2pFeatures (13) return NotImplemented. P2pMode (11) Get/SetGet
uses 1 byte `P2PConnectionType` (0 None, 1 MusicStereo, 2 MusicMonoLeft, 3 MusicMonoRight,
4 VoiceSameType, 5 VoiceDifferentType). Routing (12) Start uses port 1 with payload
`((RoutingAction.Up=1)<<7 | 2) <mac6>`.

---

## 3. AudioManagement, fblock 5

Parser: `BmapAudioManagementResponseParser`. State: `defpackage/fR`. The app's BmapFunction enum
defines functions 0..11 and 14..17; **12 and 13 have no name**.

### 3.1 Source (5.1)

- Get `05 01 01 00` → Status (`AudioManagementSourceResponse`):

| byte | meaning |
|------|---------|
| 0..1 | supported-sources bitmask, u16 BE, LSB-first bit index = source type value |
| 2 | current `AudioControlSourceType`: 0 None, 1 Bluetooth, 2 Auxiliary, 3 Usb, 4 LeaUnicast, 5 LeaBroadcast |
| 3.. | source identifier (for Bluetooth: the source MAC) |

- Live `00 02 01 11 22 33 44 55 01`: only Bluetooth is supported (0x0002), the current source is
  Bluetooth, device 11:22:33:44:55:01.

### 3.2 Control (5.3): play/pause/skip

- Get `05 03 01 00` → Status: supported-controls bitmask, 1 or 2 bytes, u16 BE, LSB-first, bit
  index = `AudioControlValue` value.
  - Live `01 ff` = 0x01FF: all of values 0..8 are supported.
- Start `05 03 05 01 <value>` → Result `05 03 06 01 <value>` echoes the value
  (`AudioManagementControlResultResponse`).
- `AudioControlValue`:

| value | name |
|-------|------|
| 0 | Stop |
| 1 | Play |
| 2 | Pause |
| 3 | TrackForward (next) |
| 4 | TrackBack (previous) |
| 5 | FastForwardPress |
| 6 | FastForwardRelease |
| 7 | RewindPress |
| 8 | RewindRelease |

  There is no toggle value. Read Status (5.4) and send Play or Pause. Examples: play
  `05 03 05 01 01`, pause `05 03 05 01 02`, next `05 03 05 01 03`, previous `05 03 05 01 04`.

### 3.3 Status (5.4)

- Get `05 04 01 00` → Status: byte 0 `AudioStatusValue` (0 Stop, 1 Play, 2 Pause, 3 FastForward,
  4 Rewind), bytes 1..2 track position as a signed i16 BE (units not given; -1 = unknown).
- Live `01 ff ff`: playing, position unknown.

### 3.4 Volume (5.5)

- Get `05 05 01 00` → Status `<max u8> <current u8>` (`AudioManagementVolumeResponse`: payload[0]
  is maxVolume, payload[1] is currentLevel).
- Live `20 17`: max 32, current 23.
- Set: SetGet `05 05 02 01 <level>` → Status `<max> <current>`. Example: set 16 is
  `05 05 02 01 10`, reply `05 05 03 02 20 10`.

### 3.5 NowPlaying (5.6): title/artist

- The app sends Start with no payload, `05 06 05 00` (`AudioManagementNowPlayingStartPacket`,
  `getNowPlayingInfo`).
- The device answers with a sequence (`fR.p` dispatch):
  1. Processing `05 06 07 00`: the app clears its accumulator (`fR.i`).
  2. N x Status `05 06 03 <len> <attr u8> <text>`: one attribute each (`fR.h`).
  3. Result `05 06 06 00`: the app publishes the collected map as `NowPlayingInfo` (`fR.g`).
- `NowPlayingAttribute` (encoding): 0 SongTitle (UTF-8), 1 Artist (UTF-8), 2 Album (UTF-8),
  3 TrackNumber (ASCII), 4 TotalNumberOfTracks (ASCII), 5 Genre (UTF-8), 6 PlayingTime (ASCII),
  7 ContainerName (UTF-8). The text is the rest of the payload with no terminator. The parser
  decodes everything with the default charset (UTF-8).
- Example: `05 06 03 06 00 48 65 6c 6c 6f` = SongTitle "Hello".
- There is no separate subscribe call. The accumulator handles these packets whenever they arrive,
  so with AudioManagement notifications enabled (section 5) track changes should arrive as the same
  Processing/Status.../Result burst. Not observed live yet.
- NowPlaying did not appear in the Get sweep because it is Start-only.

### 3.6 Latency (5.7)

- Get `05 07 01 00` → Status, 3 x i16 BE. The app names them **current, min, max** in that order
  (`AudioManagementLatencyResponse`).
- SetGet `05 07 02 02 <i16 BE>` (`AudioManagementLatencySetGetPacket`).
- Live `00 28 01 2c 00 90`. Read in the app's order this is current 40, min 300, max 144, which is
  impossible. Read as **min 40, max 300, current 144** (ms?) it makes sense. The app never shows latency
  in its UI, so it would not notice a wrong order. **Treat the field order as unverified.**

### 3.7 Function 13: unknown

- Not in the app's enum, no parser. Live Get reply (Status, 10 bytes):
  `00 97 00 00 01 c2 00 28 01 2c`.
- Read as u16 BE values: 151, 0, 450, 40, 300. The last two equal Latency's first two words
  (0x0028, 0x012C), so it is probably a latency/codec-delay table (for example per-mode or
  per-codec delays). **Unidentified.**

Other functions: GetAll (5.2) is Start `05 02 05 00` → Processing → Status... → Result. **The
app only sends it when the BMAP version is >= 1.2.0** (`qm0.x1`). This device reports 1.1.0, so the
app skips it here.

---

## 4. Control, fblock 7

Parser: `BmapControlResponseParser`. It parses only FblockInfo, GetAll, Chirp, BmapRole, Power,
ClientInteraction, FactoryDefault and FitQuality.

### 4.1 Power (7.4)

- Get `07 04 01 00` → Status 1 byte `PowerState`: 0 Off, 1 On (0xFF Unknown). Live `01`.
- **Power off: Start `07 04 05 01 00`** (`ControlPowerStartPacket(PowerState.Off)`) → Result
  `07 04 06 00` (`ControlPowerResultResponse`, payload ignored).
- `BmapInterfaceImplementation.setPowerState` exists, but the Bose Music app layer never calls it. It
  is library API only, so it has not been tested by the app on this product.

### 4.2 Function 7 "ControlANROnly" and function 8 "ControlMaxSourceCurrent"

- The names come only from the `BmapFunction` enum. No packet class, parser or caller exists.
- Live Get: 7.7 → `00`, 7.8 → `02`.
- A guess: 7.7 is a boolean "ANR-only mode" (noise cancelling works without Bluetooth audio, or
  power-on in ANR-only mode), and 7.8 is a USB charge current limit level (enum index 2). **Both
  unverified. Do not Set them.**

### 4.3 FactoryDefault (7.5): never send

- Start `07 05 05 00` (`ControlFactoryDefaultStartPacket`) → Result. It erases pairings and settings.
  The app calls it from a "reset product" UI. Keep it out of any generic code path. Never sweep
  Start operators.

### 4.4 Chirp (7.2): locate tone

- Get `07 02 01 00` → Status 1 byte: bit0 inProgress; `byte >>> 1` is `ChirpStopReasonCode`
  (0 NoEnd, 1 DeviceEnd, 2 TimerEnd, 3 AppEnd, 4 CradleEnd).
- Start `07 02 05 01 <ChirpStartValue>` (0 End, 1 Begin, 2 Continue) → Result `<duration u8>`.
- The sweep shows no reply for 7.2, so it returned a filtered error (1/4/5). It is probably not
  supported on this headphone.

Others: GetAll (7.1) is **Get** `07 01 01 00` → Status bitset of supported Control functions.
BmapRole (7.3): 0 Master, 1 Puppet. ShipMode (6), Reset (11) and PrepareToPowerOff (13) have no
packet classes in the app.

---

## 5. Notifications, startup sequence, authentication

### 5.1 Notification fblock 9

Packets: `messages/packets/Notification*`, payload builders in `FBlockNotificationKt`.

- Reset (9.1): Start `09 01 05 00` → Result. It presumably restores the default notification
  config. **The sweep's Get on 9.1 got a Result back, so it may have performed the reset.**
- ByFblock (9.2):
  - Get `09 02 01 00` → Status: the whole payload is an fblock bitset. Live `00 00 00 00 00` (5
    bytes): no fblock enabled, read after the Reset above.
  - SetGet `09 02 02 <n> <mask u8> <fblock bitset BE, at least 2 bytes>`. Mask
    (`NotificationBitmask`): 0 Overwrite, 1 Enable (add to the set), 2 Disable (remove). The bitset is
    `ceil(maxbit/8)` bytes, padded to 2. Reply: Status with the resulting bitset.
- ByFunction (9.3): Get `09 03 01 01 <fblock>` → Status `<fblock> <function bitset>`. SetGet
  `09 03 02 <n> <mask> <fblock> <function-id bitset BE, at least 2 bytes>`. The app uses it only once,
  for one fblock.
- Periodic (9.4): Get `09 04 01 01 <fblock>`. SetGet `<period_ms u16 BE> <fblock> <function bitset>`.
  Status reply has the same layout.

**What the app does on connect** (`app defpackage/qm0.A0 → B0 → D0`, called from `in0.O0` right
after the SPP connection is up): if the device lists the Notification fblock, it sends ByFblock
SetGet with **Enable** for the intersection of the supported fblocks with
`RW8.b = {Status, Settings, ProductInfo, DeviceManagement, AudioManagement, Authentication, Vpa,
AudioModes, Notification}`.

For this headphone (supported fblocks below) that is ProductInfo 0, Settings 1, Status 2,
DeviceManagement 4, AudioManagement 5, Notification 9, Authentication 18 and AudioModes 31, which gives
bitset 0x80040237:

```
-> 09 02 02 05 01 80 04 02 37
<- 09 02 03 nn <bitset>        (expected: the enabled set)
```

So **yes, the device pushes unsolicited Status packets, and notifications must be enabled per
fblock.** Every received packet goes through `BmapInterfaceImplementation.onBmapPacketReceivedEvent →
parseBmapPacket` whether or not a request is pending. The per-fblock managers (`fR` audio, `Lm2`
device management, the Status parser's BatteryLevelEvent, the Settings `ActionButtonPressEvent`, and
so on) update state from these unsolicited Status/Processing/Result packets. Button presses arrive as
Settings/Status updates for the affected function (for example a new AudioModes CurrentMode, or
Status Button 2.12 where supported). This product returns FuncNotSupp for 2.12.

Because the live ByFblock state is all zero after the sweep's Reset, a client should **always send
the Enable SetGet after connecting**. Do not rely on a default.

### 5.2 Connection / startup sequence (SPP)

Code: `BluetoothServiceCore.onSppConnectedEvent → sendBmapVersionPacket`,
`ProductInformationBmapPacketParser.parseBmapVersion`, `BmapInterfaceImplementation.getAllFunctionBlocks`,
app `qm0`/`in0`.

1. RFCOMM via SDP to the standard SPP UUID `00001101-0000-1000-8000-00805F9B34FB`
   (`createRfcommSocketToServiceRecord`). The device must already be bonded.
2. `00 01 01 00` ProductInfo.BmapVersion Get → `00 01 03 05 "1.1.0"`. The app treats this
   reply as "BMAP ready".
3. `00 02 01 00` ProductInfo.AllFblocks Get → `86 cc 03 ff`, which means fblocks {0 ProductInfo, 1 Settings,
   2 Status, 3 FirmwareUpdate, 4 DeviceManagement, 5 AudioManagement, 6 CallManagement, 7 Control,
   8 Debug, 9 Notification, 18 Authentication, 19 Experimental, 22 Print, 23 ?, 25 ?, 26 ?,
   31 AudioModes}. (ProductInfo is always treated as supported.)
4. The app posts BoseDeviceConnectedEvent, then `00 03 01 00` ProductInfo.ProductIdVariants Get.
5. Notification ByFblock SetGet Enable (5.1).
6. Capability discovery "GET_ALL" (`qm0.O0 → D1`), sequentially over the supported subset of
   `RW8.c = {Status, Settings, ProductInfo, AudioManagement, Vpa, Authentication, AudioModes,
   DataCollection, Control, Wifi}`. For this headphone:
   - `02 01 05 00` Status.GetAll Start
   - `01 01 05 00` Settings.GetAll Start
   - `00 04 05 00` ProductInfo.GetAllFunctions Start
   - (AudioManagement.GetAll skipped, since BMAP < 1.2.0)
   - `12 01 01 00` Authentication.GetAll **Get** → Status bitset (live `07 fb fe fc` = functions
     2-7, 9-17, 19-26)
   - `1f 01 05 00` AudioModes.GetAll Start
   - `07 01 01 00` Control.GetAll **Get** → Status bitset
   Start-style GetAlls reply Processing, then one Status per function, then Result.
7. `Authentication.UnencryptedProductIrk` Get (18.24). On error the app just drops that capability.
8. Then on demand: ListDevices plus Info per MAC, battery, and so on.

### 5.3 Authentication: not required for normal control

- In the app layer, the only Authentication calls are `getAllAuthenticationByRelay` (GetAll),
  `getAuthenticationOtpKeyType`, `getUnencryptedProductIrk`, and the cloud activation helpers
  `getActivation` / `getCloudActivate` / `getCondensedChallengeMessage` (`app defpackage/cD0`), which
  are used for cloud product activation and ownership.
- No challenge, ECDHE or signature step runs before Settings/AudioManagement/Control SetGet or Start. The
  connect flow above never waits on authentication.
- Live: Activation (18.17) = `00`, not activated. ProductIrk (18.7) → Error `ff 03`, which is the
  fblock-specific AuthenticationError 3 MissingSharedKey (needs an ECDHE session first). A
  reimplementation can ignore fblock 18.
- The relevant generic error is `InsecureTransport` (20), "Packet transmission attempted on insecure
  transport". It applies to unencrypted BLE. Over bonded SPP it is not an issue.

---

## 6. Errors: BmapPacketError

Error packets use operator 4. Decoding is in `BmapPacket.getErrorCode` / `BmapErrorExtKt.getBmapError`:

- payload length 1: `payload[0]` is a BmapPacketError
- payload length > 1 and `payload[0] == 0xFF` (FblockSpecific): `payload[1]` is an fblock-specific
  code. Only Authentication (18) has its own table (`AuthenticationError`). Every other fblock looks
  `payload[1]` up in BmapPacketError again.
- payload length > 1 and `payload[0] != 0xFF`: `payload[0]` is a BmapPacketError and the rest is
  ignored.

| code | name | meaning (app string) |
|------|------|----------------------|
| 0 | Unknown | Unknown error |
| 1 | Length | Invalid length |
| 2 | Chksum | Invalid checksum |
| 3 | FblockNotSupp | FBlock not supported |
| 4 | FuncNotSupp | Function not supported |
| 5 | OpNotSupp | Operator not supported for that function |
| 6 | InvalidData | Data values sent are incorrect |
| 7 | DataUnavailable | Requested data not available |
| 8 | Runtime | Temporary read/write failure |
| 9 | Timeout | Timeout |
| 10 | InvalidState | Not applicable in the current state |
| 11 | DeviceNotFound | Device not in the paired device list |
| 12 | Busy | Device busy |
| 13 | NoconnTimeout | Failed to connect to a paired device |
| 14 | NoconnKey | Failed to connect, the pairing key was deleted on the source |
| 15 | OtaUpdate | OTA already in progress |
| 16 | OtaLowBatt | OTA blocked, battery too low |
| 17 | OtaNoCharger | OTA apply needs a charger |
| 18 | OtaUpdateNotAllowed | OTA not allowed |
| 19 | UnknownPortNumber | Nonzero port not set up via DeviceManagement.Routing Start |
| 20 | InsecureTransport | Sent on an insecure transport |
| 21 | InvalidOtpKey | OTP key invalid |
| 22 | OutOfMemory | Out of memory |
| 0xFF | FblockSpecific | The next byte is an fblock-specific code |

AuthenticationError (fblock 18, after `ff`): 0 InvalidSignature, 1 RotatingKeyNotSet,
2 MissingEcdheKey, 3 MissingSharedKey, 4 PermissionError, 5 CryptoProcessingError,
6 PublicKeyNotFound, 7 FeatureLockFailed, 8 FeatureUnlockFailed, 9 InvalidOtpKey,
10 AuthNotInitiatedByThisConnection, 11 AlreadyActivated, 12 ComponentMissing.

Live example: `01 06 04 01 04` is a Settings.Anr Error, FuncNotSupp.

---

## 7. ProductInfo.ProductIdVariants and the product

- Get `00 03 01 00` → Status (`ProductInfoProductIdVariantResponse`): bytes 0..1 product id u16 BE,
  byte 2 variant u8.
- Live `40 75 01`: product id **0x4075 = 16501 = `BoseProductId.Prince`**. Its `originalName` is
  **"Bose QuietComfort Headphones"**, its BLE id is 71, and `supportedProfiles` is the default 3. Codename
  "Prince" matches Authentication.ProductName (18.12) = "Prince", and PlatformName (18.13) = "RIO".
- Variant 1 = **`BoseProductVariant.PrinceBlack`**. The full Prince table is 1 Black, 2 WhiteSmoke,
  3 MidnightBlue, 4 EclipseGrey, 5 CypressGreen, 6 MoonstoneBlue, 7 BlueDusk, 8 ChilledLilac,
  9 Sandstone, 10 PetalPink, 11 TwilightBlue, 12 MoonlightGrey, 13 IceBlue.
- The app maps Prince to the internal/cloud product type code 40 (`app gB6.f`, `BmapProductInfo`,
  `aL0.a`). Prince is in the app's supported-product whitelist (`app YK0.a`).

**Feature/capability flags:** the app has **no per-product feature table** for BMAP features. Capabilities
(numeric ids such as 30401/30402 for audio controls and 31305 for latency, built in `app
defpackage/el0`) come from:
1. the AllFblocks bitset (`el0.s`, one capability group per supported fblock),
2. the GET_ALL replies, meaning which functions answered with Status (for example audio controls are
   added only if the Control status relay is filled, and latency only if the Latency relay is),
3. BMAP version gates (for example AudioManagement GetAll and the channel capabilities only for BMAP
   >= 1.2.0. For a 1.1.0 device `el0.d` adds a fixed AudioManagement capability set: 31301-31306,
   30401, 30402, 30301, 30303, 30305).

The product id is used for naming, colour images, the onboarding UI (`com.bose.madrid.ui.onboarding.prince.*`)
and cloud lookups, not for protocol gating. A reimplementation should discover features the same way:
read AllFblocks, then probe or GetAll each fblock.

---

## Classes read

- `com.bose.bmap.messages.packets.BmapPacket`, `FBlockDeviceManagementKt`, `FBlockNotificationKt`,
  `DeviceManagement*Packet`, `AudioManagement*Packet`, `Control{Power,Chirp,FactoryDefault}*Packet`,
  `Notification*Packet`, `SupportedAudioSources`
- `com.bose.bmap.messages.parsers.Bmap{Status,DeviceManagement,AudioManagement,Control}ResponseParser`
- `com.bose.bmap.messages.responses.Status{BatteryLevel,ChargerDetect,ChargingCaseLid}Response`,
  `DeviceManagement{Connect*,Disconnect*,RemoveDevice*,ListDevices,InfoProcessingStatusResult,ExtendedInfoStatus,PairingModeStatusResult,AppAddressStatus,FeaturesStatus}Response`,
  `AudioManagement{Source,ControlStatus,ControlResult,Status,Volume,NowPlayingStatus,Latency}Response`,
  `AudioManagementSupportedFunctions`, `ControlPowerStatusResponse`, `ControlChirp*Response`,
  `Notification*Response`, `ProductInfoProductIdVariantResponse`
- `com.bose.bmap.messages.enums.*`: `BmapPacketError`, `AuthenticationError`, `BmapErrorExtKt`,
  `BmapFunctionBlock`, `BoseProductId`, `BoseProductVariant`, `BoseProductIdSupport`, `ComponentId`,
  `LidStatus`, `AudioControlValue`, `AudioControlSourceType`, `AudioStatusValue`, `NowPlayingAttribute`,
  `PowerState`, `ChirpStartValue`, `ChirpStopReasonCode`, `DisconnectReasonCode`, `P2PConnectionType`,
  `NotificationBitmask`, `ProductType`
- `com.bose.bmap.messages.utils.{ByteUtils,BitSetUtil}`, `messages.models.FunctionBlocksBitSet`
- `com.bose.bmap.service.{SppConnectionManager,BluetoothServiceCore,SppBluetoothQueue}`,
  `service.bmap.BmapInterfaceImplementation`, `model.parsers.ProductInformationBmapPacketParser`,
  `model.factories.ProductInfoPackets`
- library `defpackage.fR` (audio state), `defpackage.Lm2` (device management state)
- app `defpackage.qm0` (`C9072qm0`: notification enable and GET_ALL), `RW8` (fblock lists), `in0`
  (connect flow), `el0` (capabilities), `cD0` (pairing/connect/auth calls), `YK0`, `gB6`, `aL0`,
  `com.bose.mobile.productcommunication.models.bmap.BmapProductInfo`
