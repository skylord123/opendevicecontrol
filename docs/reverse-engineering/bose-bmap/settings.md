# BMAP Settings function block (fblock 0x01)

Source: com.bose.bosemusic 13.4.2, static analysis of dexdump bytecode only.
Live replies are from Bose QC Headphones (product 0x4075 variant 1, fw 1.0.6), the live Get sweep.

Packet header: `[fblock, function, (deviceId<<6)|(port<<4)|operator, len, payload...]`.
Every packet class below goes through the same `BmapPacket.<init>` with deviceId 0 and port 0,
so the third byte is just the operator: Get = `0x01`, SetGet = `0x02`, Start = `0x05`.
Replies: Status = `0x03`, Error = `0x04`, Result = `0x06`, Processing = `0x07`.

Dispatch: `com.bose.bmap.messages.parsers.BmapSettingsResponseParser.parse` switches on the
function and then on the operator. For all functions in this note, Status goes to
`<Response>$Companion.createFromPacket`, Error goes to `BmapErrorResponse`, and anything else
throws `BadBmapOperatorError`. SetGet requests get a Status reply. The app never uses a plain
Set (operator 0) for these functions; every write is SetGet.

Conventions used below: multi-byte integers are big-endian unless stated otherwise. Bytes read
with `aget-byte` and not masked are **signed** (s8).

`BitSetUtil` helpers (com/bose/bmap/messages/utils/BitSetUtil):
- `fromInt(i)`: bit n of the set = bit n of the integer (LSB = bit 0).
- `fromByteArray(ba)`: big-endian bit numbering. Bit n = `ba[len-1-n/8] >> (n%8) & 1`, so bit 0
  is the LSB of the **last** byte. For a 4-byte field this is the same as reading a BE u32 and
  testing `1<<n`.

---

## 1  SettingsGetAll (function 0x01)

- `SettingsGetAllStartPacket`: Start, no payload: `01 01 05 00`.
- `SettingsGetAllGetPacket`: Get, no payload: `01 01 01 00`. This class exists but nothing in the
  app constructs it.
- `BmapInterfaceImplementation.getAllSettings()` sends the **Start** packet, then waits until
  `SettingsGetAllResultResponse` arrives (`sendCommandForCompletion`).
- Reply handling in the parser for function 1: Processing (`01 01 07 ..`) gives
  `SettingsAllFunctionsProcessingResponse`, Result (`01 01 06 ..`) gives
  `SettingsGetAllResultResponse` (empty, payload ignored), Error gives `BmapErrorResponse`.
- Expected flow: the device acks with Processing, sends an ordinary Status packet for each
  supported Settings function (each parsed by its own parser below), then sends Result. The sweep
  did not exercise function 1, so this order is inferred from the code and not observed.

(Function 0x00 `SettingsFblockInfo`: `SettingsFBlockInfoGetPacket` is Get with no payload, and the
reply is parsed into an empty `SettingsFBlockInfoResponse`.)

---

## 2  SettingsProductName (function 0x02)

Classes: `packets/SettingsProductNameGetPacket`, `packets/SettingsProductNameSetGetPacket`,
`responses/SettingsProductNameResponse$Companion.createFromPacket`, `models/settings/ProductName`.

**Get:** `01 02 01 00`.

**SetGet (rename):** the payload is the new name as raw UTF-8 bytes (`Zf1.b` = `Charset.forName("UTF-8")`),
with **no** flag byte, no length prefix and no NUL terminator.

```
rename to "My QC":  01 02 02 05 4d 79 20 51 43
```

**Status payload:**

| offset | size | field |
|---|---|---|
| 0 | 1 | flags. bit0 = `isDefault` (the name is the factory default) |
| 1.. | n | name, UTF-8, the rest of the payload |

Live: `00 42 6f 73 65 20 51 43 20 48 65 61 64 70 68 6f 6e 65 73` gives isDefault = false and
name = "Bose QC Headphones". The flag reads false even though this looks like the factory name.

**Length limit:** neither the packet nor the parser checks one. The UI strings include
"Names must be 26 characters or less." and "Editable. Create a name with up to 26 characters"
(resources.arsc). Treat 26 characters as the app's limit, but I could not tie the string to the
headphone rename screen (see Uncertain). For comparison, RC preset names are limited to 31 UTF-8
bytes ("Preset name must be at most 31 bytes when UTF-8 encoded"). The payload length byte caps a
name at 255 bytes.

---

## 3  SettingsVoicePrompts (function 0x03)

Classes: `packets/SettingsVoicePromptsSetGetPacket(+$Companion.getByteArray)`,
`responses/SettingsVoicePromptsResponse$Companion.createFromPacket`, `models/settings/VoicePromptConfig`,
`models/settings/SupportedVoicePromptLanguages`, `enums/VoicePromptLanguage`.

**Get:** `01 03 01 00`.

**SetGet:** `SettingsVoicePromptsSetGetPacket(promptsEnabled, language, setBatteryLevelEnabled, batteryLevelEnabled)`.
- byte0 = `(enabled << 5) | (language.value & 0x1F)`.
- byte1 = `batteryLevelEnabled` (0/1). It is appended only when `setBatteryLevelEnabled` is true.

The language is always sent, so a toggle has to resend the current language.
```
prompts on, UsEnglish:        01 03 02 01 21
prompts off, UsEnglish:       01 03 02 01 01
on, French, battery prompt on: 01 03 02 02 22 01
```

**Status payload** (`ByteBuffer`, big-endian):

| offset | size | field |
|---|---|---|
| 0 | 1 | bits0-4 language value; bit5 `isEnabled`; bit6 `isDefaultLanguage`; bit7 `userCanToggle` |
| 1 | 4 | u32 BE supported-language bitmask. Bit n set means the language with value n is supported (`fromInt`; `isLanguageSupported` tests bit `adjustedOrdinal` = ordinal-2 = value) |
| 5 | 1 | bit0 `isBatteryLevelSupported`. Read **only if the payload length is exactly 7** |
| 6 | 1 | bit0 `isBatteryLevelEnabled`. Read only when battery level is supported |

**VoicePromptLanguage** (value is the wire value; Unknown and Invalid have no value. `getByValue`
returns Unknown when nothing matches):

| value | name | | value | name |
|---|---|---|---|---|
| 0 | UkEnglish | | 12 | Hebrew |
| 1 | UsEnglish | | 13 | Turkish |
| 2 | French | | 14 | Dutch |
| 3 | Italian | | 15 | Japanese |
| 4 | German | | 16 | Cantonese |
| 5 | EuropeanSpanish | | 17 | Arabic |
| 6 | MexicanSpanish | | 18 | Swedish |
| 7 | BrazilianPortuguese | | 19 | Danish |
| 8 | MandarinChinese | | 20 | Norwegian |
| 9 | Korean | | 21 | Finnish |
| 10 | Russian | | 22 | Hindi |
| 11 | Polish | | | |

Live: `e1 00 01 81 5e 00 00` decodes as follows.
- `e1`: language 1 = UsEnglish, enabled = 1, isDefaultLanguage = 1, userCanToggle = 1.
- mask `0x0001815E`: bits 1, 2, 3, 4, 6, 8, 15, 16, which are UsEnglish, French, Italian, German,
  MexicanSpanish, MandarinChinese, Japanese and Cantonese.
- The length is 7. byte5 = 0 (battery-level prompt not supported) and byte6 = 0.

---

## 4  SettingsStandbyTimer (function 0x04), the auto-off timer

Classes: `packets/SettingsStandbyTimerSetGetPacket(+$Companion.getByteArray)`,
`responses/SettingsStandbyTimerStatusResponse$Companion.createFromPacket`, `utils/ByteUtils`.

The unit is **minutes**. 0 means Never (disabled).

**Get:** `01 04 01 00`.

**SetGet** `(minutes)`:
- minutes <= 255: 1 byte, `[minutes]`.
- minutes > 255: 2 bytes, **little-endian** `[lo, hi]`. `convertIntToBytes(m, 2)` gives BE
  `[hi, lo]` and the code then swaps the two bytes.
```
20 min:   01 04 02 01 14
never:    01 04 02 01 00
24 h:     01 04 02 02 a0 05      (1440 = 0x05A0)
```

**Status payload:**
- len < 3: `minutes = u8 byte0` and `supportsTwoByteMinutes = false`.
- len >= 3: `minutes = (byte2 << 8) | byte0` (via `convertBytesToInt(b2, b0)`, a BE short, so the
  value is signed 16-bit) and `supportsTwoByteMinutes = true`.
- len > 1: `isUserConfigurable = !(byte1 & 1)`. bit0 of byte1 set means the timer is **not**
  user-configurable. With len <= 1 it is configurable.

So the layout is `[minutes_lo, flags, minutes_hi]`.

Live: `00` gives 0 minutes (Never), one-byte format, user-configurable.

**Values the app offers** (`cX.<clinit>` list `cX.o`, labels in `cX$a.a`): `5, 20, 40, 60, 180,
1440, 0` minutes, which is 5 min, 20 min, 40 min, 1 h, 3 h, 24 h and Never. There is also a
"10min" string resource (`settings_product_standby_timer_10min`), but that list does not
include 10.

---

## 5  SettingsCnc (function 0x05), noise-cancellation level

Classes: `packets/SettingsCncSetGetPacket`, `responses/SettingsCncResponse$Companion.createFromPacket`,
`models/settings/CncLevel`, `productcommunication/models/bmap/BmapCncLevelConfig$Companion`.

**Get:** `01 05 01 00`.

**SetGet** `(cncSetting:int, isEnabled:bool)`: `[step, enabled(0/1)]`.
```
step 10, enabled: 01 05 02 02 0a 01
```

**Status payload:**

| offset | field |
|---|---|
| 0 | `numSteps` (u8) |
| 1 | `currentStep` (u8), 0..numSteps-1 |
| 2 | bit0 `isEnabled`; bit1 **inverted**: `userEnableDisable = (bit1 == 0)` |

Direction: `mapBetweenCncLevelAndWorldVolume(level, steps) = |level - (steps-1)|`, so
world (ambient) volume = (numSteps-1) - currentStep. **Checked on hardware: the
other way round.** Step 0 is full noise cancellation (the Quiet mode stores
level 0, and Cnc reads `0b 00 03` in Quiet) and the highest step lets the most
sound in (Aware stores 10, Cnc reads `0b 0a 03`). The function's name says what
the app's slider shows (world volume), not what the level is.

Live: `0b 0a 03` gives numSteps 11, currentStep 10 (max NC), enabled = 1, bit1 = 1, so
userEnableDisable = false (the user cannot toggle CNC on or off).

---

## 6  SettingsRangeControl (function 0x07), the bass/mid/treble EQ

Classes: `packets/SettingsRangeControlSetGetPacket`, `responses/SettingsRangeControlResponse$Companion.createFromPacket`,
`models/settings/RangeLevel`, `models/settings/RangeControlStatus`, `enums/RangeControlIdentifier`.

**RangeControlIdentifier** (Byte value): Unknown = -1, **Bass = 0, Mid = 1, Treble = 2**,
LowMid = 3, HighMid = 4.

**Get:** `01 07 01 00`.

**SetGet** `(targetStep:int, range)`: `[targetStep (s8), identifier]`. Each packet sets one band.
```
Bass = +8:    01 07 02 02 08 00
Treble = -3:  01 07 02 02 fd 02
```

**Status payload:** repeating 4-byte records, all signed bytes:

| +0 | +1 | +2 | +3 |
|---|---|---|---|
| min (s8) | max (s8) | current (s8) | identifier |

Live: `f6 0a 08 00 | f6 0a 00 01 | f6 0a 03 02` gives:
- Bass: range -10..+10, current +8
- Mid: range -10..+10, current 0
- Treble: range -10..+10, current +3

---

## 7  SettingsButtons (function 0x09), configurable "shortcut" button

Classes: `packets/SettingsActionButtonGetPacket`, `packets/SettingsActionButtonSetGetPacket`,
`responses/SettingsActionButtonResponse$Companion` (`createFromPacket`, `createActionButtonModeList`,
`parseUnavailableFunctionReasons`), `models/settings/ActionButton`, `models/settings/ButtonConfig`,
`enums/ActionButtonMode`, `enums/ConfigurableButtonId`, `enums/ButtonUnavailableReason`,
`productcommunication/models/configurablebutton/ConfigurableButtonEventTypeKt`.

**Get:** `01 09 01 00`. There is no request payload; the device reports its configurable button.

**SetGet** `(ActionButton)`: `[buttonId, buttonEventType, buttonMode]`, 3 bytes.
```
assign BatteryLevel to Shortcut / long press:  01 09 02 03 80 09 03
disable shortcut:                              01 09 02 03 80 09 0e
```

**Status payload:**

| offset | size | field |
|---|---|---|
| 0 | 1 | `buttonId` (u8): ConfigurableButtonId |
| 1 | 1 | `buttonEventType`: ConfigurableButtonEventType |
| 2 | 1 | configured mode: ActionButtonMode (signed byte, unknown values give NotConfigured) |
| 3..6 | 4 | supported-modes bitmask, `fromByteArray` (BE). Bit n = ActionButtonMode value n. **Bit 0 is skipped.** An empty list becomes `[NotConfigured]` |
| 7..10 | 4 | unavailable-modes bitmask, same encoding. Present only if len > 7 |
| 11..74 | 2*k | pairs `[mode, reason]` (u8, u8) for unavailable modes. Present only if len > 11, and only pairs whose mode is in the unavailable list are kept |

Note: `createActionButtonModeList` inserts at index 0, so the app's lists end up in descending
order. The order has no meaning on the wire.

**ConfigurableButtonId:** Unknown = -1, DistalCnc = 0, Reserved = 1, Vpa = 2, RightShortcut = 3,
LeftShortcut = 4, **Shortcut = 0x80**.

**ConfigurableButtonEventType** (int constants):
- 0 Reserved, 1 RisingEdge, 2 FallingEdge, 3 ShortPress, 4 SinglePress
- 5 SinglePressAndHold, 6 DoublePress, 7 DoublePressAndHold, 8 TriplePress
- **9 LongPress**, 10 VeryLongPress, 11 VeryVeryLongPress, 12 VeryVeryVeryLongPress

**ActionButtonMode:**

| v | name | v | name | v | name |
|---|---|---|---|---|---|
| 0 | NotConfigured | 10 | TrackForward | 20 | LineInSwitch |
| 1 | Vpa (voice assistant) | 11 | TrackBack | 21 | Linking |
| 2 | Anr | 12 | FetchNotifications | 22 | PanicLocal |
| 3 | BatteryLevel | 13 | WindMode | 23 | PanicRemote |
| 4 | PlayPause | 14 | Disabled | 24 | PanicBoth |
| 5 | IncreaseCnc | 15 | ClientInteraction | 25 | ThirdPartyService |
| 6 | DecreaseCnc | 16 | SpotifyGoMode (Spotify Tap) | 26 | SmartAction |
| 7 | ToggleWuw (wake-up word) | 17 | ModesCarousel | 27 | Alexa |
| 8 | SwitchSourceDevice | 18 | VolumePresetCarousel | | |
| 9 | ConversationMode | 19 | SpatialAudioMode | | |

**ButtonUnavailableReason:** 0 Reserved, 1 UnavailableLEAudioActive.

Live: `80 09 03 00 01 40 08 00 00 00 80` decodes as follows.
- buttonId 0x80 = Shortcut, event 9 = LongPress (press and hold), configured = 3 BatteryLevel.
- supported `00 01 40 08` = 0x00014008, bits 3, 14 and 16: BatteryLevel, Disabled, SpotifyGoMode.
- unavailable `00 00 00 80`, bit 7: ToggleWuw.
- There are no reason pairs (len = 11).

---

## 8  SettingsMultipoint (function 0x0A)

Classes: `packets/SettingsMultipointSetGetPacket`, `responses/SettingsMultipointResponse$Companion`.

**Get:** `01 0a 01 00`.

**SetGet** `(isEnabled)`: `[0/1]`, giving `01 0a 02 01 01` (on) or `01 0a 02 01 00` (off).

**Status** byte0:
- bit0 `isEnabled`
- bit1 `isSupported`
- bit2 `isDisableSupported`

Live: `07` means multipoint is on, supported, and can be disabled.

---

## 9  SettingsSidetone (function 0x0B), self-voice

Classes: `packets/SettingsSidetoneSetGetPacket`, `responses/SettingsSidetoneResponse$Companion`,
`models/settings/SupportedSidetoneModes`, `enums/SidetoneMode`.

**SidetoneMode:** Off = 0, High = 1, Medium = 2, Low = 3. `getByValue` falls back to Off.

**Get:** `01 0b 01 00`.

**SetGet** `(sidetoneMode:int, isPersisted:bool)`: `[persisted(0/1), mode]`. The persist flag
comes **first**.
```
Low, persisted: 01 0b 02 02 01 03
```

**Status payload:**

| offset | field |
|---|---|
| 0 | bit0 `isPersisted` |
| 1 | current mode (signed byte → SidetoneMode) |
| 2.. | supported-modes bitmask, the rest of the payload, `fromByteArray` (BE, bit n = mode value n) |

Live: `01 02 0f` gives persisted, mode Medium, supported {Off, High, Medium, Low}.

---

## 10  SettingsSetupComplete (function 0x0C)

Classes: `packets/SettingsSetupCompleteSetGetPacket`, `responses/SettingsSetupCompleteResponse$Companion`.

- **Get:** `01 0c 01 00`.
- **SetGet:** `[0/1]`, for example `01 0c 02 01 01`.
- **Status:** byte0 bit0 = `isComplete` (out-of-box setup has been done).

Live: `01` = complete.

---

## 11  SettingsCncPersistence (function 0x0E)

Classes: `packets/SettingsCncPersistenceSetGetPacket`, `responses/SettingsCncPersistenceResponse$Companion`,
`models/settings/CncPersistence`. Analytics key `product_setting_noise_cancellation_persistence`:
whether the NC level is kept across power cycles.

- **Get:** `01 0e 01 00`.
- **SetGet:** `[0/1]`, for example `01 0e 02 01 01`.
- **Status:** byte0 bit0 = `isEnabled`.

Live: `01` = enabled.

---

## Other functions seen in the sweep

These replied Error `04` on this device: Anr (6), Alerts (8), ConversationMode (13),
CncPresets (15), OnHeadDetection (16), and every function above 16. On this firmware they are
unsupported or need a payload, so they are not described here.

## Uncertain / not verified

- GetAll response sequence (Processing, then per-function Status, then Result) comes from the
  parser and `getAllSettings()`. It was not observed live.
- The product name 26-character limit comes from UI strings only. I could not tie it to the BMAP
  rename screen; resource ids are obfuscated. Nothing in the BMAP layer checks the length.
- The ProductName status bit0 (`isDefault`) reads 0 for the name "Bose QC Headphones". The
  meaning comes from the field name, not from observation.
- StandbyTimer three-byte status layout `[lo, flags, hi]` comes from the code, since the live
  device sent a one-byte reply. The 2-byte SetGet is little-endian `[lo, hi]`.
- What the device does with a VoicePrompts SetGet whose language is not in the supported mask
  is unknown.
