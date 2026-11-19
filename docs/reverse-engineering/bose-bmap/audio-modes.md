# BMAP AudioModes function block (fblock 31 / 0x1F)

Source: Bose Music 13.4.2, static analysis of dexdump bytecode only. Live values from
the live Get sweep (Bose QC Headphones, PID 0x4075 var 1, fw 1.0.6).

Packet header reminder: `[fblock, function, (dev<<6)|(port<<4)|op, len, payload...]`.
All AudioModes packets the app builds use dev 0 / port 0, so byte 2 is just the operator:
Set 0, Get 1, SetGet 2, Status 3, Error 4, Start 5, Result 6, Processing 7.

Classes read (all under `com/bose/bmap/messages/` unless noted):

- packets: `AudioModes*Packet`, `FBlockAudioModesKt.createAudioModesConfigSetGetPayload`,
  `AudioModesModeConfigSetGetPacket$Companion.getModeName$messages_release`,
  `AudioModesFavoritesSetGetPacket$Companion.getPayload`
- parser: `parsers/BmapAudioModesResponseParser.parse` + `$WhenMappings.<clinit>`
- responses: `responses/AudioModes*Response$Companion.createFromPacket` and their `<init>`s
- models/enums: `models/audiomodes/AudioModesPrompt` (+`$Companion.find`), `AudioModesSettingsConfigInfo`,
  `enums/SpatialAudioModeTypes`, `enums/WindBlockEnabledTypes`, `enums/AncToggleEnableTypes`
- service: `com/bose/bmap/service/bmap/BmapInterfaceImplementation` (getAllAudioModeInfo,
  setCurrentAudioMode, setGetModesFavorites, ...), `parseBmapPacket` -> `YM6.b` (device state
  updater), `lR` (audioModesInfoManager)
- UI (obfuscated): `YQ4` (Mode Details view model: W3 delete, c2/o4/n2/h2 clear-mode chain,
  I3 ANC toggle, M4 CNC), `UR4.F` (mode naming), `zT4` (Modes Selection), `ye7`/`ff7`/`of7`/`qT6`
  command wrappers, `ModeDetailsActivity.onCreate`
- notifications: `qm0.B0/D0`, `RW8.<clinit>`, `FBlockNotificationKt.createNotificationByFBlockPacketPayload`,
  `BitSetUtil.toByteArray`, `ByteUtils.convertIntToBytes`

## Function table

| fn | name | operators the app sends | reply the app expects |
|----|------|------------------------|-----------------------|
| 0 | FblockInfo | Get (no payload) | Status -> `FunctionBlockInfoResponse` (payload ignored) |
| 1 | GetAll | Start (no payload) | Processing, then Result (empty). Device emits Status for the fblock's functions in between (inferred, standard GET_ALL; log "Audio Modes GET_ALL not responding with Result") |
| 2 | Capabilities | Get | Status |
| 3 | CurrentMode | Get; Start `[idx, playPrompt]` | Status `[idx]`; for Start: Processing (empty) and Result `[idx]` (Result and Status both parse as `AudioModesCurrentModeResponse`) |
| 4 | DefaultMode | Get; SetGet `[idx]` | Status `[idx]` |
| 5 | Persistence | Get; SetGet `[0/1]` | Status `[0/1]` |
| 6 | ModeConfig | Get `[idx]`; SetGet (see below); Start (no payload) = dump all modes | Status (48 bytes) per mode; Start: Processing, N x Status, Result (empty) |
| 7 | UserIndices | Get; SetGet `[i0, i1, ...]` | Status `[i0, i1, ...]` |
| 8 | Favorites | Get; SetGet `[numModes, bitmask...]` | Status same format |
| 9 | Reset | Start `[idx]` | Result (empty) -> `AudioModesResetResultResponse` |
| 10 | SettingsConfig | Get; SetGet `[cnc, autoCnc, spatial, wind, anc]` | Status same 5 bytes |
| 11 | NamesSupported | Get | Status: prompt bitmask |

Any function: Error (op 4) -> `BmapErrorResponse`. Any operator not listed above for a
function -> `BadBmapOperatorError` in the app (e.g. there is no Status handling for Reset,
no Result handling for ModeConfig other than the empty end-of-dump marker).

Every request packet with a payload is built as "SetGet"/"Start"/"Get" exactly as listed;
there is no plain Set (op 0) anywhere in this fblock.

---

## Enums

### AudioModesPrompt (2 bytes, `byte1`, `byte2`)
Voice prompt the headphones play when the mode is selected; the app also uses it to
pick the mode's displayed (localized) name. `byte1` must be 0 (`find()` returns NONE
for any non-zero byte1), `byte2` is the id. Effectively a big-endian u16 where only
0..36 are known.

| id | name | id | name | id | name | id | name |
|----|------|----|------|----|------|----|------|
| 0 | None (empty slot) | 10 | Home | 20 | Run | 30 | Calm |
| 1 | Quiet | 11 | Work | 21 | Walk | 31 | Sleep |
| 2 | Aware | 12 | Music | 22 | Hike | 32 | Meditate |
| 3 | Transparent | 13 | Focus | 23 | Talk | 33 | Yoga |
| 4 | Transparency | 14 | Relax | 24 | Call | 34 | Immersion |
| 5 | Masking | 15 | Flight | 25 | Whisper | 35 | Stereo |
| 6 | Comfort | 16 | Airport | 26 | Hearing | 36 | Cinema |
| 7 | Commute | 17 | Driving | 27 | Learn | | |
| 8 | Outdoor | 18 | Training | 28 | Podcast | | |
| 9 | Workout | 19 | Gym | 29 | Audiobook | | |

Unknown ids decode to None in the app.

### SpatialAudioModeTypes (1 byte)
| value | name |
|-------|------|
| 0x00 | SpatialDisabled |
| 0x01 | SpatialAudioFixedToRoom |
| 0x02 | SpatialAudioFixedToHead |
| 0xFF (-1) | Unknown (app-side sentinel; means "field absent") |

### WindBlockEnabledTypes (1 byte)
0x00 WindBlockDisabled, 0x01 WindBlockEnabled, Unknown = field absent (sentinel -1).

### AncToggleEnableTypes (1 byte)
0x00 AncToggleDisabled, 0x01 AncToggleEnabled, Unknown = field absent (sentinel -1).

### Booleans
All booleans are parsed as `byte == 1` (anything else is false) and written as 0/1.

### CNC level
Plain u8. Range is `0 .. totalSteps-1`, where totalSteps comes from Settings fblock
`SettingsCnc` (01.05). Live: `SettingsCnc` = `0b 0a 03` -> 11 steps, so 0..10.
The UI converts with `BmapCncLevelConfig.mapBetweenCncLevelAndWorldVolume(level, steps)
= |level - (steps-1)|` (symmetric, used both directions). The app uses `5` as the default
level for a newly named mode (`UR4.F`) and for a manually cleared mode (`YQ4.n2`).
Direction, checked on hardware: 0 = full noise cancelling (Quiet stores 0), the top
step = most ambient sound (Aware stores 10).

---

## 2 Capabilities  (Get: `1f 02 01 00`)

Status payload (`AudioModesCapabilitiesResponse$Companion.createFromPacket`):

| byte | field |
|------|-------|
| 0 | boseModes (u8, number of fixed Bose modes) |
| 1 | userModes (u8, number of user mode slots) |
| 2..4 | not read by the app |
| 5 | flags: bit0 cncSupported, bit1 autoCNCSupported, bit2 spatialAudioFixedToHeadAndFixedToRoomSupported, bit3 anrWindToggleSupported (wind block), bit4 userFavoritesSupported, bit5 ancToggleSupported |
| 6 | minimumAllowableSimultaneouslyFavoriteModes (only if len >= 7, else 0) |

Live: `1f 02 03 06 | 02 02 00 00 00 09`
-> boseModes 2, userModes 2 (4 modes total), flags 0x09 = CNC supported + wind block
supported; no auto-CNC, no spatial, no user favorites flag, no ANC toggle; min favorites 0
(byte 6 absent).

## 3 CurrentMode

- Get: `1f 03 01 00`
- Status/Result payload: `[modeIndex]` (only byte 0 read).
- Live: `1f 03 03 01 | 01` -> current mode index 1.

Switch mode (`AudioModesCurrentModeStartPacket(modeIndex, playVoicePrompt)`):
`1f 03 05 02 | idx playPrompt` e.g. switch to mode 0 announcing it: `1f 03 05 02 00 01`.
The app almost always passes playVoicePrompt = 1 (`ff7` callers in `zT4.k2`, `YQ4.r4`,
`YQ4.h2`, `PY8.n1`); one path (`Jz7.j0`) passes 0.
Expected reply: Processing `1f 03 07 00`, then Result `1f 03 06 01 <idx>` (or a Status);
the app accepts either Result or Status as `AudioModesCurrentModeResponse`.

## 4 DefaultMode
- Get `1f 04 01 00`; SetGet `1f 04 02 01 <idx>`. Status payload `[idx]`.
- Live: `01` -> default (power-on) mode index 1. (The app has a setter in
  BmapInterfaceImplementation but no UI caller of `setDefaultAudioMode` was found.)

## 5 Persistence
- Get `1f 05 01 00`; SetGet `1f 05 02 01 <0|1>`. Status payload `[enabled]` (==1).
- Live: `01` -> enabled (presumably: last used mode is kept across power cycles instead of
  falling back to DefaultMode).

## 6 ModeConfig (one mode's full config)

### Get one mode
`1f 06 01 01 <idx>` (`AudioModesModeConfigGetPacket(modeIndex)`).

### Get all modes
`1f 06 05 00` (`AudioModesModeConfigStartPacket`, used by `getAllAudioModeInfo` with 30 s
timeout). Device answers Processing `1f 06 07 00` (app clears its list, `lR.c`), then one
Status per mode, then Result `1f 06 06 00` (app returns the collected list).

### Status payload (`AudioModesModeConfigResponse$Companion.createFromPacket`)

Full length is 48 bytes in the parser; trailing fields are optional (length-gated). The
QC Headphones (fw 1.0.6) send **47 bytes**: no ANC toggle byte.

| offset | len | field |
|--------|-----|-------|
| 0 | 1 | modeIndex |
| 1 | 2 | prompt (`byte1`, `byte2`; see AudioModesPrompt) |
| 3 | 1 | userConfigurable (==1): slot is a user slot (editable/deletable) |
| 4 | 1 | userConfigured (==1): user slot currently holds a mode |
| 5 | 1 | favorite (==1) |
| 6 | 32 | name, UTF-8, NUL-terminated/padded (decoded up to first 0x00) |
| 38 | 3 | not read by the app |
| 41 | 1 | mutability flags: bit0 cncMutable, bit1 autoCNCMutable, bit2 spatialAudioMutable, bit3 anrWindToggleMutable, bit4 ancToggleMutable |
| 42 | 1 | cncLevel (u8) |
| 43 | 1 | autoCNCEnabled (==1) |
| 44 | 1 | spatial audio mode (SpatialAudioModeTypes) if len >= 45, else Unknown |
| 45 | 1 | not read by the app |
| 46 | 1 | wind block (WindBlockEnabledTypes) if len >= 47, else Unknown |
| 47 | 1 | ANC toggle (AncToggleEnableTypes) if len >= 48, else Unknown |

Example (constructed, not live; QC-like Quiet mode, CNC 10, wind block on):
```
1f 06 03 30
00                      mode 0
00 01                   prompt Quiet
00 00 01                not user-configurable, not user-configured, favorite
51 75 69 65 74 00 ...   "Quiet" + NULs (32 bytes total)
00 00 00                unread
09                      cncMutable | anrWindToggleMutable
0a                      cncLevel 10
00                      autoCNC off
00                      spatial disabled
00                      unread
01                      wind block enabled
00                      ANC toggle disabled
```
(No ModeConfig replies were captured in the sweep because it needs an index argument.)

### SetGet (create / edit / clear a mode)
`AudioModesModeConfigSetGetPacket(modeIndex, prompt, name, cncLevel, autoCNCEnabled,
spatialAudioType, windBlockToggleEnabled, ancToggleEnable)` ->
`FBlockAudioModesKt.createAudioModesConfigSetGetPayload`. Note the request layout differs
from the Status layout (no flag bytes, no gaps):

| offset | len | field |
|--------|-----|-------|
| 0 | 1 | modeIndex |
| 1 | 2 | prompt byte1, byte2 |
| 3 | 32 | name: UTF-8 bytes, first `min(len(name),31)` bytes copied, then 0x00, rest 0x00 (`getModeName$messages_release`; truncation counts Java chars, so cut on a UTF-8 boundary yourself and keep <= 31 bytes) |
| 35 | 1 | cncLevel |
| 36 | 1 | autoCNCEnabled (0/1) |
| 37 | 0/1 | spatial (optional) |
| 38 | 0/1 | wind block (optional) |
| 39 | 0/1 | ANC toggle (optional) |

Optional tail rules, exactly as coded:
- spatial != Unknown: append spatial byte; if wind == Unknown append `00` (wind disabled
  placeholder) and stop the wind step.
- else if wind != Unknown: append `00` (SpatialDisabled placeholder) then the wind byte.
- if spatial != Unknown and wind != Unknown: append spatial then wind.
- ANC toggle != Unknown: append `01`/`00` (appended directly after whatever came before;
  if spatial and wind are both Unknown it lands at offset 37).

So lengths are 37, 39 (spatial+wind) or 40 (spatial+wind+anc) in practice. Rule for a
reimplementation: send a trailing field exactly when the mode's Status reply contained it
(the app passes the values it read back; when clearing it maps every present value to
Disabled and keeps absent ones Unknown, `YQ4.e3/c3/z2`).

Reply: Status with the 48-byte layout above (the app waits for `AudioModesModeConfigResponse`).

Example: give user slot 2 the prompt/name "Commute", CNC 5, auto-CNC off, wind block on,
device without spatial/ANC-toggle (spatial Unknown, wind Enabled, ANC Unknown):
```
1f 06 02 27
02                                  index 2
00 07                               prompt Commute
43 6f 6d 6d 75 74 65 00 x25         "Commute" + 25 NULs (32 bytes)
05                                  cncLevel 5
00                                  autoCNC off
00                                  spatial placeholder (SpatialDisabled)
01                                  wind block on
```
len = 3 + 32 + 2 + 2 = 39 = 0x27.

### How the app uses it
- **Display name** of a mode comes from the prompt id (localized string), not the `name`
  field (`zT4.K1` -> prompt). Slots with prompt None are shown as empty/"add mode".
- **Create / rename** (`UR4.F`): user picks a prompt from the NamesSupported list; app sends
  ModeConfig SetGet with that prompt, `name` = the localized string of that prompt, cnc = 5
  for a new mode (existing cnc when renaming), other fields copied from the current config.
  Target slot = a mode whose `userConfigurable` is set (empty when prompt is None /
  `userConfigured` false).
- **Edit a setting** (`YQ4.I3` ANC toggle, `M4` CNC, wind, spatial `g4`, ...): read-modify-
  write: resend the whole config with one field changed.
- **Delete** ("Clear mode", `YQ4.W3` -> `c2`):
  1. If the mode is a favourite: remove it from the favourites list, but refuse (show a
     dialog) if the current favourite count is already <= `minimumAllowableSimultaneouslyFavoriteModes`.
  2. `AudioModes.Reset` Start `1f 09 05 01 <idx>` -> Result `1f 09 06 00`.
  3. If Reset fails (Error), fallback "manually clear": ModeConfig SetGet with prompt None
     (`00 00`), name "" (32 x 00), cnc 5, autoCNC unchanged, and every present spatial/wind/
     ANC field set to Disabled (`00`).
  4. Favorites SetGet with the reduced list (`of7`).
  5. CurrentMode Start to the first favourite (if `userFavoritesSupported`) else to mode 0,
     with playVoicePrompt = 1 (`YQ4.h2`, `ff7(idx, true, ...)`).

## 7 UserIndices
- Get `1f 07 01 00`; SetGet `1f 07 02 <n> i0 i1 ...` (one byte per int).
- Status payload: every byte is one int (signed byte -> int).
- Live: `00 01 02 03`.
- The app only stores it (`ConnectedBoseDevice.audioModesUserIndicesBehaviorRelay`); no
  UI caller of `setModesUserIndices` was found. Meaning not established (likely the
  ordering/list of mode slots); see Uncertain.

## 8 Favorites
Favourites are the modes the headphone button cycles through ("Cycle Through Favorites").

Payload format (both SetGet request and Status reply):
`[N, b_{k-1}, ..., b_1, b_0]` where N = total number of modes and k = ceil(N/8).
Mode index i is bit (i % 8) of byte at payload offset `k - floor(i/8)` (i.e. the bitmask
is big-endian: the last byte holds indices 0..7).

Encoder (`AudioModesFavoritesSetGetPacket$Companion.getPayload(numModes, favs[])`):
`size = ceil(N/8)+1; p[0]=N; for i in favs: p[size-1-i/8] |= 1<<(i%8)`.
Decoder (`AudioModesFavoritesResponse$Companion.createFromPacket`): for j = k down to 1,
for bit b in 0..7: if set, index = (k-j)*8 + b.

The app passes N = `KEY_NUMBER_OF_MODES` (the number of modes, i.e. boseModes+userModes /
number of ModeConfig entries) (`ModeDetailsActivity.onCreate` -> `YQ4.g0` -> `of7`).

- Get `1f 08 01 00`
- Live: `1f 08 03 02 | 04 03` -> N = 4, mask 0x03 -> favourites = modes 0 and 1.
- Example set favourites {0, 1, 3} of 4 modes: `1f 08 02 02 04 0b`.
- Example N = 10, favourites {0, 9}: payload `0a 02 01`.

Also each ModeConfig Status carries a per-mode `favorite` byte (offset 5).

## 9 Reset
Start `1f 09 05 01 <idx>` -> Result (empty). Used to delete a user mode (see above).
(Bytes show fblock 0x1f, function 0x09.)

## 10 SettingsConfig
Live settings of the active mode, used for "CNC out of modes" (adjusting the slider
without editing a stored mode) (`yB0.w7`, `yB0.l6`, log strings "CNC Out Of Modes: ...").
- Get `1f 0a 01 00`
- SetGet `1f 0a 02 05 | cnc autoCnc spatial wind anc` (all 5 bytes always present; spatial
  is written raw, so Unknown would be 0xFF; send 00 when unsupported).
- Status payload (5 bytes): `[0] cncLevel, [1] autoCNCEnabled, [2] spatial (enum),
  [3] anrWindToggleEnabled, [4] ancToggleEnabled` (booleans == 1).
- Not present in the sweep (probably Error/unsupported on this device, or not swept).

## 11 NamesSupported
- Get `1f 0b 01 00`
- Status payload: little-endian bitmask of supported prompts, max 5 bytes: byte i bit j set
  -> prompt id `8*i + j` (byte 4 only bits 0..4, i.e. ids up to 36). Note the byte order is
  the opposite of Favorites. Bit 0 of byte 0 is prompt None.
- The app uses it for the list of names offered when creating/renaming a mode.
- Not present in the sweep.

## 1 GetAll
Start `1f 01 05 00` -> Processing `1f 01 07 00` ... Result `1f 01 06 00`. Used during
connection setup (`qm0.D1`, "Starting GET_ALL commands.") for every supported fblock; all
Status packets that arrive meanwhile update device state through `parseBmapPacket` ->
`YM6.b`.

---

## Live sweep decode summary

| packet | decode |
|--------|--------|
| `1f 02 03 06 02 02 00 00 00 09` | 2 Bose modes + 2 user slots; CNC + wind block supported; no auto-CNC/spatial/ANC toggle/user-favourites flag |
| `1f 03 03 01 01` | current mode = 1 |
| `1f 04 03 01 01` | default mode = 1 |
| `1f 05 03 01 01` | persistence on |
| `1f 07 03 04 00 01 02 03` | user indices [0,1,2,3] |
| `1f 08 03 02 04 03` | 4 modes, favourites {0,1} |

Cross-check: `SettingsCnc` (01.05) `0b 0a 03` -> 11 steps, current step 10, while current
mode = 1. If mode 1 is Aware (typical: 0 Quiet, 1 Aware), step 10 = full world.

## Mode changes made on the headphones

- Every incoming packet goes through `BmapInterfaceImplementation.parseBmapPacket` ->
  parser -> `YM6.b(device, response)`, which updates per-device relays regardless of
  whether the packet answers a request: CurrentMode Status/Result -> `currentAudioModeRelay`,
  ModeConfig Status -> `audioModesRelay` (replace entry for that index) and `audioModeRelay`,
  Favorites -> `audioModesFavoritesBehaviorRelay`, SettingsConfig -> `audioModesSettingsConfigRelay`,
  UserIndices, NamesSupported, Capabilities, DefaultMode, Persistence likewise. UI code
  subscribes to these ("Mode Details: setupCurrentModeListener, current mode changed to index",
  "Modes Selection: mode update listener for index").
- So a button press is reported as an unsolicited `1f 03 03 01 <idx>` Status (and, if the
  user edited a mode on-device, `1f 06 03 ...`). The app does not poll.
- To receive them the app enables notifications at connect (`qm0.B0` -> `qm0.D0`):
  Notification fblock 9, function 2 (NotificationByFblock), SetGet, payload
  `[mask, bitset]` with mask `01` = Enable (00 Overwrite, 02 Disable) and bitset =
  big-endian integer of `1 << fblockId` for each fblock in
  {ProductInfo 0, Settings 1, Status 2, DeviceManagement 4, AudioManagement 5,
  Notification 9, Vpa 16, Authentication 18, AudioModes 31} intersected with the device's
  supported fblocks (ProductInfo AllFblocks). Byte length = (highest bit+8)/8 (4 bytes when
  AudioModes is included).
  For this device (AllFblocks `86 cc 03 ff` = bits 0-9,18,19,22,23,25,26,31) that gives bits
  {0,1,2,4,5,9,18,31} = 0x80040237:
  `09 02 02 05 01 80 04 02 37` (derived, not captured).
  Live `NotificationByFblock` Get returned `00 00 00 00 00` (5 bytes); its layout is outside
  this note's scope.

## Uncertain / not established

1. ModeConfig Status bytes 38-40 and 45 are never read by the app; meaning unknown.
2. Capabilities bytes 2-4 are never read.
3. ~~CNC direction~~ settled on hardware: level 0 is full noise cancelling (Quiet stores
   0), the top step the most ambient sound (Aware stores 10).
4. UserIndices meaning (ordering? slot list?) unknown; app never sets it.
5. ~~CurrentMode Start reply~~ seen on hardware: Processing, then a DefaultMode Status
   (switching also changes the default mode), then Result `[idx]`.
6. Favorites `N` is "number of modes" as passed through `KEY_NUMBER_OF_MODES`; the live
   reply `04` matches boseModes+userModes = 4.
7. GetAll exact behaviour (which Statuses are emitted) inferred from the GET_ALL pattern.
8. Whether this firmware sends unsolicited AudioModes Status without the
   NotificationByFblock enable is unknown; send the enable to be safe.
9. SettingsConfig and NamesSupported replies were not in the sweep (may be unsupported on
   this firmware; an Error op 4 would mean that).
