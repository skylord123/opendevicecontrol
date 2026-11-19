# BMAP Settings function block (fblock 0x01), remaining functions

Source: com.bose.bosemusic 13.4.2, static analysis of dexdump bytecode only. Nothing here was
tested on hardware. This page continues [settings.md](settings.md), which covers functions 1, 2,
3, 4, 5, 7, 9, 10, 11, 12 and 14 and explains the packet header, `BitSetUtil` and the signed-byte
convention. The same rules apply here: deviceId 0 and port 0, so the third byte is the operator
(Get `0x01`, SetGet `0x02`, Start `0x05`; replies Status `0x03`, Error `0x04`, Result `0x06`,
Processing `0x07`). Every write is SetGet; the app never sends a plain Set.

Dispatch: `parsers/BmapSettingsResponseParser.parse`. For every function on this page except
VolumeControl, a reply with operator Status **or** SetGet (`0x02`) goes to the same
`<Response>$Companion.createFromPacket`, and Error goes to `BmapErrorResponse`. VolumeControl only
accepts Status and Error. The RCPresetInfo function also accepts Processing and Result.

Unless a section says otherwise, Get has no payload and returns the same Status as SetGet.

The QC Headphones used for settings.md replied Error to every function on this page
(`01 06 04 01 04` is Anr, FuncNotSupp), so all layouts below come from the code only.

| fn | hex | name | user-facing name (UI string / analytics key) |
|---|---|---|---|
| 6 | 06 | Anr | noise reduction mode (Off / High / Wind / Low) |
| 8 | 08 | Alerts | ringtone and haptic alerts |
| 13 | 0d | ConversationMode | "Conversation Mode" |
| 15 | 0f | CncPresets | noise-cancellation presets (`product_setting_noise_cancellation_presents`) |
| 16 | 10 | OnHeadDetection | "On-Head Detection" with "Auto Play/Pause", "Auto Answer Call", "Auto Transparency" |
| 19 | 13 | LedBrightness | no classes in the app |
| 20 | 14 | MotionInactivityAutoOff | "Motion Detection" auto-off (`product_setting_motion_auto_off`) |
| 22 | 16 | FlipToOff | "Flip-to-Off" / "Lay Flat to Disconnect" |
| 24 | 18 | AutoPlayPause | "Auto Play/Pause" |
| 26 | 1a | TeamsButtonMode | "Control Teams Only" / "Control Teams and Voice Assistant" |
| 27 | 1b | AutoAnswer | "Auto Answer Call" |
| 28 | 1c | VolumeControl | "Volume Touch Control" / "Volume Control" |
| 29 | 1d | AutoAwareMode | `product_settings_auto_aware_mode` |
| 30 | 1e | SourceBargeIn | no classes in the app |
| 32 | 20 | AutoVolumeLevel | "Auto Volume" |
| 33 | 21 | Grouping | speaker Stereo / Party linking |
| 34 | 22 | DisableCaptouch | "Touch Control" |
| 37 | 25 | LeAudioControl | "LE Audio" / "Use LE Audio When Available" |
| 38 | 26 | LeAudioBroadcastConfig | Auracast broadcast quality |
| 39 | 27 | HeadGesture | `product_setting_head_gesture` |
| 41 | 29 | LeaGrouping | LE Audio speaker Stereo / Party grouping |
| 50 | 32 | RCPresetInfo | EQ preset list |
| 51 | 33 | RCPresetSave | save EQ preset |
| 52 | 34 | RCPresetSelect | select EQ preset |
| 53 | 35 | RCPresetDelete | delete EQ preset |
| 55 | 37 | RCPresetRename | rename EQ preset |

Product hints: the app does not keep a table of which product supports which function. The UI
shows a setting when the device answers for it (GetAll, see settings.md). The product hints below
come from onboarding video names (enum `IO`, e.g. `vedder_tenor_flip_to_off.mp4`) and UI strings,
matched against the `BoseProductId` code names (Goodyear = NC 700, Olivia/Vedder = Bose Frames,
Lonestarr = QC Ultra Headphones, Wolverine = QC Ultra 2 HP, Mariah = QC 2 HP, Scotty = QC Ultra
Earbuds, Edith = QC Ultra 2 Earbuds, Serena = Ultra Open Earbuds, Aubrey = Ultra Open 2 Earbuds,
Spice = Sport Open Earbuds). Treat them as hints, not as a support list.

---

## 1  SettingsAnr (function 0x06), noise-reduction mode

Classes: `packets/SettingsAnrGetPacket`, `packets/SettingsAnrSetGetPacket(ncLevel:int)`,
`responses/SettingsAnrResponse$Companion`, `models/settings/SupportedAnrModes`,
`models/settings/AnrConfig`, `enums/AnrMode`.

This is the older discrete noise-reduction mode, not the 0..10 slider (that one is SettingsCnc,
function 5). It probably belongs to QC35-era headphones, which have Off/High/Low. The BmapInterface
methods `getAnr`/`setAnr` exist, but nothing in the app UI calls them in 13.4.2.

**AnrMode:** Off = 0, High = 1, Wind = 2, Low = 3. `getByValue` falls back to **High** when the
value does not match.

**Get:** `01 06 01 00`.

**SetGet** `(ncLevel)`: `[mode]`, 1 byte.
```
Off:   01 06 02 01 00
High:  01 06 02 01 01
Wind:  01 06 02 01 02
Low:   01 06 02 01 03
```

**Status payload:**

| offset | size | field |
|---|---|---|
| 0 | 1 | current mode (signed byte, AnrMode) |
| 1.. | n | supported-modes bitmask, the rest of the payload, `fromByteArray` (BE, bit n = AnrMode value n) |

Example (built from the code, not captured): `01 0b` gives mode High, supported {Off, High, Low}
(0x0b = bits 0, 1, 3).

---

## 2  SettingsAlerts (function 0x08)

Classes: `packets/SettingsAlertsGetPacket`, `packets/SettingsAlertsSetGetPacket(isRingtoneEnabled, areHapticsEnabled)`,
`responses/SettingsAlertsResponse$Companion`, `models/settings/AlertsConfig`, `event/external/settings/AlertsEvent`.

Two switches for incoming-call alerts: a ringtone and a haptic (vibration) alert. I could not tie
it to a UI screen or a product; `getAlerts`/`setAlerts` have no callers in the app UI.

**Get:** `01 08 01 00`.

**SetGet:** one byte, `ringtone | (haptics << 1)`.
```
ringtone on, haptics on:   01 08 02 01 03
ringtone off, haptics on:  01 08 02 01 02
```

**Status** byte0: bit0 `ringtoneEnabled`, bit1 `hapticsEnabled`. The other bits are ignored.

---

## 3  SettingsConversationMode (function 0x0D)

Classes: `packets/SettingsConversationModeGetPacket`, `packets/SettingsConversationModeSetGetPacket(isEnabled)`,
`responses/SettingsConversationModeResponse$Companion`.

UI: "Conversation Mode". "Press and hold the Noise Control button to turn on Conversation Mode. In
this mode, audio pauses and noise cancellation automatically adjusts to the lowest level so you can
hear the world around you." The onboarding video is `goodyear_nc_conversation_mode.mp4`, so this is
the NC 700 feature. The BmapInterface getter and setter exist but nothing in the UI calls them; the
headphone button turns it on.

- **Get:** `01 0d 01 00`.
- **SetGet:** `[0/1]`, for example `01 0d 02 01 01` (on).
- **Status:** byte0 bit0 = `isEnabled`.

---

## 4  SettingsCncPresets (function 0x0F), noise-cancellation presets

Classes: `packets/SettingsCncPresetsGetPacket`, `packets/SettingsCncPresetsSetGetPacket(selectedIndex:int, presetValues:int[])`,
`responses/SettingsCncPresetsResponse$Companion`, `models/settings/CncPresets`,
`productcommunication/models/bmap/BmapCncPresetsConfig`. Analytics key
`product_setting_noise_cancellation_presents` (sic).

A short list of noise-cancellation levels that the headphone button cycles through (the NC 700
button steps through three levels), plus which one is active.

**Get:** `01 0f 01 00`.

**SetGet:** `[selectedIndex, preset0, preset1, ...]`. Each value is cast to a byte. The app sends
the whole list every time.
```
presets 0, 5, 10; select index 1:  01 0f 02 04 01 00 05 0a
```

**Status payload:**

| offset | field |
|---|---|
| 0 | `selectedIndex` (s8) |
| 1.. | one preset value per byte (s8), the rest of the payload |

The values are probably SettingsCnc steps (0..numSteps-1, see settings.md). I did not find code
that converts them.

---

## 5  SettingsOnHeadDetection (function 0x10)

Classes: `packets/SettingsOnHeadDetectionGetPacket`, `packets/SettingsOnHeadDetectionSetGetPacket(ZZZZ)`,
`responses/SettingsOnHeadDetectionResponse$Companion`, `models/settings/OnHeadDetection`
(`isEnabled`, `isAutoPlayEnabled`, `isAutoAnswerEnabled`, `isAutoTransparencyEnabled`). Caller:
`BmapInterfaceImplementation.setOnHeadDetection(enabled, autoPlay, autoAnswer, autoTransparency)`.
Analytics key `product_setting_on_head_detection`.

UI: "On-Head Detection. On-Head detection uses sensors to know when you're wearing your headphones."
Sub-switches: "Auto Play/Pause" ("Audio pauses when you remove the headphones and resumes when you
wear the headphones."), "Auto Answer Call" ("Answers a phone call when you wear the headphones."),
and "Auto Transparency". This is the headphone version. Earbuds use the separate functions
AutoPlayPause (0x18) and AutoAnswer (0x1B).

**Get:** `01 10 01 00`.

**SetGet:** 2 bytes.
- byte0 = master `isEnabled` (0/1).
- byte1 = `autoPlay | (autoAnswer << 1) | (autoTransparency << 2)`.

The app takes the current model and sends every flag. A sub-feature the device does not support
is sent as 0.
```
all on:                              01 10 02 02 01 07
detection on, only auto play/pause:  01 10 02 02 01 01
detection off:                       01 10 02 02 00 00
```

**Status payload:** 2 bytes. byte1 is always read, so a 1-byte reply throws.

| byte | bit | field |
|---|---|---|
| 0 | 0 | `isEnabled` (master switch) |
| 0 | 1 | auto play/pause **supported** |
| 0 | 2 | auto answer **supported** |
| 0 | 3 | auto transparency **supported** |
| 1 | 0 | auto play/pause enabled |
| 1 | 1 | auto answer enabled |
| 1 | 2 | auto transparency enabled |

The response stores each sub-feature as `Boolean?`, and it is null when its supported bit is 0.
Example: `07 01` means detection on, play/pause and auto answer supported (transparency not),
play/pause on, auto answer off.

---

## 6  SettingsMotionInactivityAutoOff (function 0x14)

Classes: `packets/SettingsMotionInactivityAutoOffGetPacket`, `packets/SettingsMotionInactivityAutoOffSetGetPacket(isEnabled)`,
`responses/SettingsMotionInactivityAutoOffResponse$Companion`, `models/settings/MotionDetection`.
Analytics key `product_setting_motion_auto_off`.

UI: "Motion Detection". The related strings: "Headphones are optimized to preserve battery life by
auto turning off after 1 hour of no motion. This occurs regardless of your current auto-off timer."
and "Earbuds are ... after 10 minutes of no motion." It is a switch, separate from the
StandbyTimer minutes in settings.md.

- **Get:** `01 14 01 00`.
- **SetGet:** `[0/1]`, for example `01 14 02 01 01`.
- **Status:** byte0 bit0 = `isEnabled`.

---

## 7  SettingsFlipToOff (function 0x16)

Classes: `packets/SettingsFlipToOffGetPacket`, `packets/SettingsFlipToOffSetGetPacket(isEnabled)`,
`responses/SettingsFlipToOffResponse$Companion`, `models/settings/FlipToOff`. Analytics key
`product_setting_flip_to_off`.

UI: "Flip-to-Off" / "Lay Flat to Disconnect". Onboarding videos `olivia_flip_to_off.mp4`,
`vedder_tenor_flip_to_off.mp4` and `vedder_soprano_flip_to_off.mp4`, so this is Bose Frames:
laying the glasses upside down turns them off.

- **Get:** `01 16 01 00`.
- **SetGet:** `[0/1]`, for example `01 16 02 01 01`.
- **Status:** byte0 bit0 = `isEnabled`.

---

## 8  SettingsAutoPlayPause (function 0x18)

Classes: `packets/SettingsAutoPlayPauseGetPacket`, `packets/SettingsAutoPlayPauseSetGetPacket(isEnabled)`,
`responses/SettingsAutoPlayPauseStatusResponse$Companion`. Analytics key `product_settings_auto_play_pause`.

UI: "Auto Play/Pause". The earbud version: "Audio pauses when you remove an earbud and resumes when
you re-insert the earbud." (also "...the right earbud..." on products where only one bud has the
sensor).

- **Get:** `01 18 01 00`.
- **SetGet:** `[0/1]`, for example `01 18 02 01 01`.
- **Status:** byte0 bit0 = `isEnabled`.

---

## 9  SettingsTeamsButtonMode (function 0x1A)

Classes: `packets/SettingsTeamsButtonModeGetPacket`, `packets/SettingsTeamsButtonModeSetGetPacket(TeamsButtonMode)`,
`responses/SettingsTeamsButtonModeStatusResponse$Companion`, `enums/TeamsButtonMode`. Analytics key
`product_setting_teams_button_mode`.

For Microsoft Teams (UC) headsets used with the Bose USB Link dongle. "You can decide how you want
the Voice Assistant button (right earcup, lower button) to work with Teams." That button layout is
the NC 700 (UC).

**TeamsButtonMode:**

| value | name | UI string (matched by meaning) |
|---|---|---|
| 0 | CombinationButtonMode | "Control Teams and Voice Assistant": the button controls Teams while the USB Link is connected and the voice assistant otherwise |
| 1 | DedicatedButtonMode | "Control Teams Only" |

`getByValue` falls back to CombinationButtonMode.

- **Get:** `01 1a 01 00`.
- **SetGet:** `[mode]`, for example `01 1a 02 01 01` (Teams only).
- **Status:** byte0 = mode. Nothing else is read.

---

## 10  SettingsAutoAnswer (function 0x1B)

Classes: `packets/SettingsAutoAnswerGetPacket`, `packets/SettingsAutoAnswerSetGetPacket(isEnabled)`,
`responses/SettingsAutoAnswerStatusResponse$Companion`. Analytics key `product_settings_auto_answer`.

UI: "Auto Answer Call". "Answers a phone call when you insert an earbud." (or "...the right
earbud.").

- **Get:** `01 1b 01 00`.
- **SetGet:** `[0/1]`, for example `01 1b 02 01 01`.
- **Status:** byte0 bit0 = `isEnabled`.

---

## 11  SettingsVolumeControl (function 0x1C)

Classes: `packets/SettingsVolumeControlGetPacket`, `packets/SettingsVolumeControlSetGetPacket(VolumeControlValue)`,
`responses/SettingsVolumeControlStatusResponse$Companion`, `enums/VolumeControlValue`. Analytics key
`product_setting_volume_control`.

UI: "Volume Touch Control" ("Swipe up on the right earbud to increase the volume, and swipe down to
decrease the volume. To start using this feature, go to Settings and turn on Volume Touch
Control."). Onboarding videos exist for Lonestarr, Wolverine, Mariah, Scotty, Edith, Serena, Aubrey
and Spice, so this covers the QC Ultra / QC headphones and the QC Ultra / Ultra Open / Sport Open
earbuds. It picks which input changes the volume.

**VolumeControlValue:**

| value | name |
|---|---|
| 0 | DisabledVolumeControl |
| 1 | ButtonVolumeControl |
| 2 | CapTouchVolumeControl (swipe on the touch surface) |
| 3 | ImuVolumeControl (motion sensor; the gesture is not described anywhere I found) |

`getByValue` falls back to DisabledVolumeControl.

**Get:** `01 1c 01 00`.

**SetGet:** `[value]`.
```
touch (swipe) volume on: 01 1c 02 01 02
volume control off:      01 1c 02 01 00
```

**Status payload:**

| offset | field |
|---|---|
| 0 | current VolumeControlValue |
| 1 | supported methods, present only if len > 1. bit0 = Button supported, bit1 = CapTouch supported, Imu: see below |

The app builds the list `[(Button, b1&1), (CapTouch, b1&2), (Imu, (b1&3)==3)]`. The Imu test masks
with **3**, not 4, so the app reports Imu as supported exactly when Button and CapTouch are both
supported. That looks like an app bug; the device probably means bit2. Treat bit2 as Imu and
verify on hardware.

Example: `02 02` = touch volume on, only CapTouch supported.

---

## 12  SettingsAutoAwareMode (function 0x1D)

Classes: `packets/SettingsAutoAwareModeGetPacket`, `packets/SettingsAutoAwareModeSetGetPacket(isEnabled)`,
`responses/SettingsAutoAwareModeStatusResponse$Companion`. Analytics key `product_settings_auto_aware_mode`.

A switch that changes to Aware mode automatically. I found no UI string with "Auto Aware"; the
nearest one is "Auto Transparency", which is already a bit of OnHeadDetection. It may be the same
feature on products without on-head detection, but that is a guess.

- **Get:** `01 1d 01 00`.
- **SetGet:** `[0/1]`, for example `01 1d 02 01 01`.
- **Status:** byte0 bit0 = `isEnabled`.

---

## 13  SettingsAutoVolumeLevel (function 0x20)

Classes: `packets/SettingsAutoVolumeLevelGetPacket`, `packets/SettingsAutoVolumeLevelSetGetPacket(isEnabled)`,
`responses/SettingsAutoVolumeLevelResponse$Companion`.

UI: "Auto Volume". Not to be confused with `madrid/settings/AutoVolumeSettingsActivity`, which is
the Wi-Fi soundbar "autovolume" REST setting (`AutoVolumeRequestGson`) and not BMAP.

- **Get:** `01 20 01 00`.
- **SetGet:** `[0/1]`, for example `01 20 02 01 01`.
- **Status:** byte0 bit0 = `isEnabled`.

---

## 14  SettingsGrouping (function 0x21), speaker Stereo/Party link

Classes: `packets/SettingsGroupingGetPacket`, `packets/SettingsGroupingSetGetPacket(Z, LinkingStatusTypes)`,
`responses/SettingsGroupingResponse$Companion`, `models/settings/SettingsLinkingInfo`,
`models/settings/LinkMemberInfo`, `enums/LinkingStatusTypes`, `enums/LinkingModeTypes`.
Analytics key `product_setting_grouping`.

Bluetooth speaker linking (SoundLink Stereo or Party). Not used by headphones.

**LinkingStatusTypes** (byte value): Unknown = -1 (0xff), SettingsGroupingUngrouped = 0,
SettingsGroupingStereo = 1, SettingsGroupingParty = 2. Unknown values decode as Unknown.

**Get:** `01 21 01 00`.

**SetGet:** `[flag(0/1), linkingStatus]`. The flag is probably `persistLinking`, by analogy with
the Status layout. The app passes it in from a caller I did not trace.
```
persist, Stereo: 01 21 02 02 01 01
ungroup:         01 21 02 02 00 00
```

**Status payload:**

| offset | size | field |
|---|---|---|
| 0 | 1 | bit0 `persistLinking`, bit1 `linkLoss` |
| 1 | 1 | `currentLinkingMode` (LinkingStatusTypes) |
| 2 | 1 | supported modes: bit0 Stereo, bit1 Party |
| 3 | 1 | `deviceCount` (s8) |
| 4.. | var | member records, repeated while offset < len-1 |

Member record (`LinkMemberInfo`):

| +0 | +n | +n+1..n+2 | +n+3 | +n+4..n+19 |
|---|---|---|---|---|
| product name, UTF-8, n bytes | 0x00 | product id (BE u16, `BoseProductIdSupport.getByValue`) | variant (`BoseProductVariant.forValue`) | 16-byte GUID (`GuidUtil.formatGuidBytes`) |

Record length = n + 20. Any index error is reported as `BmapErrorResponse` with `BmapPacketError.Length`.

---

## 15  SettingsDisableCaptouch (function 0x22), touch control

Classes: `packets/SettingsDisableCaptouchGetPacket`, `packets/SettingsDisableCaptouchSetGetPacket(isEnabled)`,
`responses/SettingsDisableCaptouchResponse$Companion`. Analytics key `product_setting_disable_captouch`.
UI view model `l88` ("Touch Control Setting", "Couldn't update the Touch Control setting").

UI: "Touch Control". "Use touch control by tapping or swiping the touch surface of your %s. If
touch control isn't for you, you can turn it off."

- **Get:** `01 22 01 00`.
- **SetGet:** `[0/1]`.
- **Status:** byte0 bit0 = `isEnabled`.

**Polarity is not confirmed.** The function is named *Disable*Captouch but the field is
`isEnabled`. I found no inversion (`xor`) between the "Touch Control" switch in `l88` and
`setDisableCaptouchEnabled`, so the switch state may go on the wire as-is. That would make
1 = touch control **on**, but it is just as possible that 1 = "disable" is on. Check on a device:
send `01 22 01 00` with touch control on in the Bose app.

---

## 16  SettingsLeAudioControl (function 0x25)

Classes: `packets/SettingsLeAudioControlGetPacket`, `packets/SettingsLeAudioControlSetGetPacket(isEnabled)`,
`responses/SettingsLeAudioControlResponse$Companion`. BmapInterface method `setGetLeAudioControl`.
UI `madrid/ui/settings/LeAudioToggleSectionView`.

UI: "LE Audio", "Use LE Audio When Available", "Turn On LE Audio" / "Turn Off LE Audio". "LE Audio
provides enhanced Bluetooth audio over a low latency connection and introduces new features like
Auracast broadcast audio." "Your product restarts then automatically powers back on when you turn
LE Audio off." "Some Shortcuts do not work while connected over LE Audio (Voice Assistant, Alexa
and Spotify)."

- **Get:** `01 25 01 00`.
- **SetGet:** `[0/1]`, for example `01 25 02 01 01`. Expect the product to reboot when LE Audio is
  turned off.
- **Status:** byte0 bit0 = `isEnabled`.

---

## 17  SettingsLeAudioBroadcastConfig (function 0x26)

Classes: `packets/SettingsLeAudioBroadcastConfigGetPacket`, `packets/SettingsLeAudioBroadcastConfigSetGetPacket(LeAudioBroadcastConfigType)`,
`responses/SettingsLeAudioBroadcastConfigResponse$Companion`, `enums/LeAudioBroadcastConfigType`.

Quality of the device's own Auracast broadcast.

**LeAudioBroadcastConfigType** (byte value; unknown values decode as Unknown):

| value | name |
|---|---|
| -1 (0xff) | Unknown |
| 0 | CompatibilityHighQuality48kHz |
| 1 | CompatibilityLowQuality16kHz |
| 2 | BoseLowQuality16kHz |
| 3 | BoseHighQuality48kHz |
| 4 | CompatibilityNormalQuality24kHz |

The app only exposes a switch (`yB0.Z6(Boolean)`): on sends CompatibilityNormalQuality24kHz (4),
off sends BoseHighQuality48kHz (3).

- **Get:** `01 26 01 00`.
- **SetGet:** `[value]`, for example `01 26 02 01 04` (compatibility, 24 kHz).
- **Status:** byte0 = value.

---

## 18  SettingsHeadGesture (function 0x27)

Classes: `packets/SettingsHeadGestureGetPacket`, `packets/SettingsHeadGestureSetGetPacket(isEnabled)`,
`responses/SettingsHeadGestureResponse$Companion`. Analytics key `product_setting_head_gesture`.

A switch for head gestures (nod or shake). The BmapInterface getter and setter exist but nothing in
the 13.4.2 UI calls them, and I found no UI string for the feature. Bose Frames are the likely
users (nod to answer a call, shake to decline), but that comes from product knowledge, not from
the app.

- **Get:** `01 27 01 00`.
- **SetGet:** `[0/1]`, for example `01 27 02 01 01`.
- **Status:** byte0 bit0 = `isEnabled`.

---

## 19  SettingsLeaGrouping (function 0x29), LE Audio speaker grouping

Classes: `packets/SettingsLeaGroupingGetPacket`, `packets/SettingsLeaGroupingSetGetPacket(Z, LeaGroupingModeTypes)`
(and `(Z, byte)`), `responses/SettingsLeaGroupingResponse$Companion`,
`models/settings/SettingsLeaGroupingInfo`, `enums/LeaGroupingModeTypes`.

The LE Audio (Auracast) version of Grouping for newer speakers. Not used by headphones.

**LeaGroupingModeTypes** (byte value): Unknown = -1, Ungrouped = 0, Stereo = 1, Party = 2.

**Get:** `01 29 01 00`.

**SetGet:** `[persistGrouping(0/1), mode]`.
- `endLeaGrouping()` sends `01 29 02 02 00 00` (no persist, Ungrouped).
- The UI switch (`yB0.j7`) keeps the current `persistGrouping` and sends Stereo (true) or Party
  (false), for example `01 29 02 02 01 01`.

**Status payload:**

| offset | field |
|---|---|
| 0 | bit0 `persistGrouping`, bit1 `linkLoss` |
| 1 | `currentGroupingMode` (LeaGroupingModeTypes) |
| 2 | supported modes: bit0 Stereo, bit1 Party |
| 3 | bits0-6 `deviceCount`, bit7 `deviceCountUndetermined` |

(Function 47 `SettingsLeaGroupMember` is a Start/Processing/Status/Result query of the group
members. It is outside this page.)

---

## 20  RC presets (functions 0x32-0x37), EQ presets

Classes: `packets/SettingsRCPresetInfoGetPacket(presetName)`, `packets/SettingsRCPresetInfoStartPacket`,
`packets/SettingsRCPreset{Save,Select,Delete,Rename}SetGetPacket`, `packets/SettingsRCPresetSelectGetPacket`,
`responses/SettingsRCPreset*Response$Companion`, `models/settings/RCPresetInfo`,
`models/settings/RCPresetRename`, `enums/RCPresetType`, `utils/RCPresetNameUtil`.

"RC" is RangeControl, so these are named presets of the RangeControl EQ (settings.md section 6),
with up to five bands. Function 54 does not exist. The BmapInterface methods exist
(`getAllRCPresets`, `getRCPreset`, `getRCPresetSelect`, `setRCPresetSave/Select/Delete/Rename`), but
nothing in the 13.4.2 UI calls them.

**Name encoding** (`RCPresetNameUtil.encode`): UTF-8 followed by one `0x00`. An empty name or a
name of 32 bytes or more throws ("Preset name must be at most 31 bytes when UTF-8 encoded").
`decode(payload, offset)` reads from `offset` up to the first `0x00` (or the end).

**RCPresetType:** Predefined = 0, Custom = 1.

### RCPresetInfo (0x32)

- **Start** `01 32 05 00`: list every preset. `getAllRCPresets()` waits up to **30 s** for the
  Result (`01 32 06 ..`). The parser also handles Processing (`01 32 07`) and a Status per preset.
- **Get** with a name: one preset. `01 32 01 05 4d 69 6e 65 00` asks for "Mine".

Status payload:

| offset | size | field |
|---|---|---|
| 0 | 1 | bit0 type (RCPresetType), bit1 `isSelected`, bit2 `isDefault` |
| 1..20 | 5 x 4 | five range records `[min, max, current, identifier]` (s8 each), the same layout as RangeControl |
| 21.. | | preset name, NUL-terminated |

### RCPresetSave (0x33)

SetGet `(bass, mid, treble, lowMid, highMid, name)`: `[bass, mid, treble, lowMid, highMid, name..., 00]`,
each step a signed byte.
```
save "Mine" = bass +3, mid 0, treble -2, lowMid 0, highMid 0:
01 33 02 0a 03 00 fe 00 00 4d 69 6e 65 00
```
Reply parsing depends on the reply's operator. If it is **SetGet** (`0x02`), bytes 0-4 are the
five steps (keyed by RangeControlIdentifier 0..4) and the name starts at offset 5. If it is
Status, the steps map is empty and the name starts at offset 0.

### RCPresetSelect (0x34)

- **Get** `01 34 01 00`: Status = name of the selected preset (NUL-terminated, offset 0).
- **SetGet** `[name..., 00]`: `01 34 02 05 4d 69 6e 65 00`.

### RCPresetDelete (0x35)

SetGet `[name..., 00]`: `01 35 02 05 4d 69 6e 65 00`. Status = the deleted name.

### RCPresetRename (0x37)

SetGet `[old..., 00, new..., 00]`: `01 37 02 06 4d 69 6e 65 00 41 00` renames "Mine" to "A".
Status: old name from offset 0, new name after the first `0x00` (`RCPresetNameUtil.indexAfter`).

---

## Functions with no implementation in the app

- **LedBrightness (19, 0x13)** and **SourceBargeIn (30, 0x1E)** appear only in `BmapFunction`.
  There are no packet, response or model classes, and `BmapSettingsResponseParser` has no case for
  them, so a reply would throw `InvalidBmapFunctionError`. Their payloads are unknown.

## Uncertain / not verified

- None of these functions was exercised on hardware; the test headphones reply FuncNotSupp.
- DisableCaptouch polarity (section 15).
- VolumeControl Imu support bit: the app tests `(b1 & 3) == 3`; bit2 is the likely meaning.
- The CncPresets values are assumed to be SettingsCnc steps.
- Grouping SetGet first byte is assumed to be `persistLinking`.
- TeamsButtonMode value-to-string mapping is by meaning (the enum names match the two UI strings).
- AutoAwareMode and HeadGesture have no UI strings, and Alerts has no UI or product link.
- The RCPresetSave reply format that depends on the reply operator is odd. It is described as
  coded, and I did not find which operator the device actually uses.
- Product families come from onboarding video names and UI strings, not from a support table.
