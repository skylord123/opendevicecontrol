# BMAP notes: products, transports (SPP / BLE / LE-CoC), earbuds, PA mixer, voice assistant

Source: Bose Music 13.4.2 (`com.bose.bosemusic`), static analysis of the `dexdump -d` bytecode only
(`classes2.dex` holds the unobfuscated `com.bose.bmap.*` library, `classes4.dex` the obfuscated app
layer). Product display strings come from the default-locale string table in `resources.arsc`.
Nothing here was sent to a device; byte examples are built from the code, not captured, unless
they say otherwise.

Packet header reminder: `[fblock, function, (dev<<6)|(port<<4)|op, len, payload...]`, operators
Set 0, Get 1, SetGet 2, Status 3, Error 4, Start 5, Result 6, Processing 7. Every packet in this
note is built with dev 0 / port 0 (constructor mask 44 or 12), so byte 2 is just the operator.
Multi-byte integers are big endian. "Bitset" means `BitSetUtil.fromByteArray`: big-endian byte
array, bit 0 is the LSB of the **last** byte.

Function blocks used here: ProductInfo 0, Status 2, AudioManagement 5, Control 7,
HearingAssistance 12 (0x0C), Vpa 16 (0x10).

---

## 1. Products

### 1.1 Where the product id and name come from

- `BoseProductId` (enum, `messages/enums`) has, per product: `bleValue` (id used in BLE adverts),
  `value` (the u16 that ProductInfo.ProductIdVariants 0.3 returns), `originalName`,
  `supportedProfiles` (default 3, 1 for a few old speakers and Frames; **never read anywhere**),
  `hardwareRevParser` (only Stetson has a real one) and `specialCaseType` (0, -1 for Unknown).
- Lookup by product id: `BoseProductIdSupport.getByValue(int)` walks `values()` in declaration order
  and returns the **first** match, else `Unknown`. Several ids are shared (left/right/case of the
  first true-wireless earbuds, and `Leo` shares 0x4034 with the older `BudliteDev`), so the later
  entry is unreachable through this lookup ("shadowed" in the table).
- Lookup by advert id: `getByBleValue(int)` (first match on `bleValue`, negative input = Unknown).
- `originalName` is the factory name. The name the app shows for a connected product is the
  user-set name read with Settings.ProductName (1.2, see `settings.md`). The app's own product
  pages use string resources keyed by codename: `product_help_<codename>_title` and
  `accessibility_help_<codename>_item` (for example `product_help_prince_title` = "QuietComfort
  Headphones", `product_help_mariah_title` = "QuietComfort Headphones 2"). Those are listed in the
  "App help name" column. There is no product catalog JSON in the APK assets (the only JSON assets
  are the PA tone-match preset lists, `voice_prompt_languages.json` and a mock firmware file).
- `BoseProductVariant` (255 entries) is `(product, value)`; `forValue(product, v)` maps Celine and
  CelineIi both to Celine first, then matches `value`, else `Unknown`. The variant byte is byte 2 of
  ProductIdVariants (0.3) and also 3 bits of the BLE advert.
- The app keeps its own integer "device type" per product (`aL0$a.a`, a switch on
  `BoseProductId`), used for the cloud/catalog and to choose the connection path (section 2). It is
  in the "App type" column. Categories (headphones / earbuds / speaker ...) are **not** in the BMAP
  library; they are sets of these app type codes in the app class `zr2`:

| `zr2` set | app type codes | meaning (from membership) |
|---|---|---|
| b | 1,10,13,22,23,25,27,32,37,38,40,41,44,45,46,52-56,59,60,62,66,68-72,75-79 | all Bluetooth (SPP) BMAP products |
| m | 1,27,37,40,54,56,68,69,70,77 | headphones |
| e | 10,13,25,32,38,41,46,52,60,62,66,71,72 | earbuds |
| g | 32,38,41,46,60,62,66,71,72 | earbuds with a BMAP-visible charging case |
| f | 44,45,53,55,59,75,76,78,79 | Bluetooth speakers |
| d | 22,23 | Frames (audio sunglasses) |
| c | 16,29 | PA speakers (L1 Pro, S1 Pro+): "only supports BLE" |
| h | 4,5,21,24,30,31,36,48,50,64,65 | Wi-Fi soundbars |
| i | 3,7,8,9,49,51 | Wi-Fi smart speakers |

- `ProductType` (`Unknown -1, Headphones 0, Speaker 1`) is only filled from the BLE advert: the
  legacy, 1.0.4 and Rio advert formats have a speaker bit (bit 2 of advert byte 3); the 1.2.0 advert
  format always reports `Headphones`. It does not separate earbuds from headphones; use the table.
- `YK0.a()` is the app's whitelist of products it offers to set up (the "Supported" column).
  `Brussels` (QuietComfort Earbuds, id 0x0023) is listed but the app shows
  `brussels_redirect_description` ("pair your Bose QuietComfort Earbuds with the QCE app").

### 1.2 Product table

Columns: enum ordinal, product id as returned by ProductInfo 0.3 (hex, decimal), BLE advert id
(`-` = never advertised / -1), `originalName`, app help name (string resources), app device-type
code, category (derived from section 1.1 and 2), and whether the product is in the app's whitelist.
Older products have no help strings. Topaz, Emerald, Opal, Sapphire, Quartz, Clyde, Gracie, Hawkeye
and Leo have `originalName` "Bose <codename>" and help strings equal to the codename, and KarenMok,
MichaelCheung and Abasi have no help string at all: these look like unreleased products whose
names were not filled in yet.

| Codename | ordinal | Product id | BLE id | `originalName` | App help/marketing name | App type | Category | Supported |
|---|---|---|---|---|---|---|---|---|
| Isaac | 0 | 0x400A (16394) | 0 | Bose AE2 SoundLink |  | - |  |  |
| Wolfcastle | 1 | 0x400C (16396) | 1 | Bose QuietComfort 35 |  | - |  |  |
| Ice | 2 | 0x4012 (16402) | 2 | Bose SoundSport |  | - |  |  |
| Foreman | 3 | 0x400D (16397) | 5 | Bose SoundLink Color II |  | - |  |  |
| Powder | 4 | 0x4014 (16404) | 4 | Bose QuietControl 30 |  | - |  |  |
| Flurry | 5 | 0x4013 (16403) | 3 | Bose SoundSport Pulse |  | - |  |  |
| Harvey | 6 | 0x4011 (16401) | 7 | Bose Revolve+ Soundlink |  | - |  |  |
| Folgers | 7 | 0x4010 (16400) | 6 | Bose Revolve Soundlink |  | - |  |  |
| Kleos | 8 | 0x4017 (16407) | 8 | Bose SoundWear |  | - |  |  |
| Levi | 9 | 0x4018 (16408) | 10 | Bose SoundSport Free |  | - |  |  |
| LeviSlave | 10 | 0x4019 (16409) | 11 | Bose SoundSport Free |  | - |  |  |
| Minnow | 11 | 0x4022 (16418) | 12 | Bose SoundLink Micro |  | - |  |  |
| Baywolf | 12 | 0x4020 (16416) | 9 | Bose QuietComfort 35 Series 2 |  | 1 | headphones |  |
| Atlas | 13 | 0x4021 (16417) | 13 | Bose Aviation Headset |  | - |  |  |
| Bb2 | 14 | 0x1872 (6258) | 14 | Bosebuild 2 |  | - |  |  |
| Goodyear | 15 | 0x4024 (16420) | 15 | Bose NC 700 Headphones |  | 1 | headphones | yes |
| Rio | 16 | 0x4025 (16421) | - | Rio |  | 2 | platform (dev) |  |
| Eddie | 17 | 0x4026 (16422) | 16 | Bose Home Speaker 500 |  | 3 | Wi-Fi (BMAP over BLE only) | yes |
| Professor | 18 | 0x4027 (16423) | 17 | Bose Soundbar 500 |  | 4 | Wi-Fi (BMAP over BLE only) | yes |
| GingerCheevers | 19 | 0x4028 (16424) | 18 | Bose Soundbar 700 |  | 5 | Wi-Fi (BMAP over BLE only) | yes |
| SanDiego | 20 | 0x403D (16445) | 41 | Bose Soundbar 300 |  | 21 | Wi-Fi (BMAP over BLE only) | yes |
| Exeter | 21 | 0x403E (16446) | 42 | Bose Music Amplifier | Bose Music Amplifier | 20 | Wi-Fi (BMAP over BLE only) | yes |
| Woobie | 22 | 0x4029 (16425) | 19 | Woobie |  | - |  |  |
| Flipper | 23 | 0x402A (16426) | 20 | Bose Home Speaker 300 |  | 7 | Wi-Fi (BMAP over BLE only) | yes |
| Chibi | 24 | 0xA211 (41489) | 21 | Chibi |  | - |  |  |
| Celine | 25 | 0x402C (16428) | 22 | Bose Frames |  | - |  |  |
| CelineIi | 26 | 0x404C (16460) | 32 | Bose Frames |  | - |  |  |
| Taylor | 27 | 0x4033 (16435) | 23 | Bose Portable Home Speaker | Portable Smart Speaker | 8 | Wi-Fi (BMAP over BLE only) | yes |
| EddieClub | 28 | 0x4031 (16433) | 24 | Bose Home Speaker 450 |  | 9 | Wi-Fi (BMAP over BLE only) | yes |
| Angus | 29 | 0x094D (2381) | 51 | Bose Smart Soundbar 900 | Smart Soundbar | 24 | Wi-Fi (BMAP over BLE only) | yes |
| AngusChina | 30 | 0x1873 (6259) | 62 | Bose Soundbar 850 |  | - |  |  |
| Duran | 31 | 0x4039 (16441) | 55 | Bose QC 45 | QC 45 | 27 | headphones | yes |
| Zakim | 32 | 0x094C (2380) | 60 | Bose Zakim |  | 28 | Wi-Fi (BMAP over BLE only) | yes |
| Malcolm | 33 | 0x094E (2382) | 61 | Bose Smart Soundbar 600 |  | 30 | Wi-Fi (BMAP over BLE only) | yes |
| MalcolmClub | 34 | 0x1875 (6261) | 64 | Bose Soundbar 550 |  | 31 | Wi-Fi (BMAP over BLE only) | yes |
| MalcolmChina | 35 | 0x1874 (6260) | 63 | Bose Soundbar 550 |  | - |  |  |
| Stevie | 36 | 0x4069 (16489) | 68 | Bose Smart Ultra Soundbar | Smart Ultra Soundbar | 36 | Wi-Fi (BMAP over BLE only) | yes |
| StevieClub | 37 | 0x4070 (16496) | 70 | Stevie Club |  | - |  |  |
| StevieChina | 38 | 0x4071 (16497) | 69 | Bose Ultra Soundbar |  | - |  |  |
| Prince | 39 | 0x4075 (16501) | 71 | Bose QuietComfort Headphones | QuietComfort Headphones | 40 | headphones | yes |
| LamarrSb | 40 | 0x4080 (16512) | 72 | Lamarr Smart Soundbar |  | 48 | Wi-Fi (BMAP over BLE only) | yes |
| LamarrSs | 41 | 0x4077 (16503) | 73 | Lamarr Smart Speaker |  | 49 | Wi-Fi (BMAP over BLE only) | yes |
| Luca | 42 | 0x4078 (16504) | 74 | Bose LS Ultra Soundbar | Lifestyle Ultra Soundbar | 50 | Wi-Fi (BMAP over BLE only) | yes |
| LucaChina | 43 | 0x4091 (16529) | 78 | Bose LS Ultra Soundbar | Lifestyle Ultra Soundbar | - |  |  |
| Donnie | 44 | 0x4079 (16505) | 75 | Bose LS Ultra Speaker | Lifestyle Ultra Speaker | 51 | Wi-Fi (BMAP over BLE only) | yes |
| DonnieChina | 45 | 0x4092 (16530) | 79 | Bose LS Ultra Speaker | Lifestyle Ultra Speaker | - |  |  |
| Paulie | 46 | 0x4087 (16519) | 77 | Bose LS Ultra Subwoofer | Lifestyle Ultra Subwoofer | 57 | Wi-Fi subwoofer accessory |  |
| PaulieChina | 47 | 0x4093 (16531) | 80 | Bose LS Ultra Subwoofer | Lifestyle Ultra Subwoofer | - |  |  |
| Abasi | 48 | 0x4094 (16532) | 81 | Bose Abasi |  | 64 | Wi-Fi (BMAP over BLE only) | yes |
| AbasiChina | 49 | 0x4095 (16533) | 82 | Bose Abasi China |  | - |  |  |
| AbasiClub | 50 | 0x4096 (16534) | 83 | Bose Abasi Club |  | 65 | Wi-Fi (BMAP over BLE only) | yes |
| RevelLeft | 51 | 0x402D (16429) | 26 | Bose Sport Earbuds | Sport Earbuds | - |  |  |
| RevelRight | 52 | 0x402D (16429) (shadowed by RevelLeft) | 25 | Bose Sport Earbuds | Sport Earbuds | 10 | earbuds | yes |
| RevelCase | 53 | 0x402E (16430) | 27 | Bose Sport Earbuds | Sport Earbuds | - |  |  |
| LandoLeft | 54 | 0x402F (16431) | 29 | Bose QuietComfort Earbuds | QC Earbuds | - |  |  |
| LandoRight | 55 | 0x402F (16431) (shadowed by LandoLeft) | 28 | Bose QuietComfort Earbuds | QC Earbuds | 13 | earbuds | yes |
| LandoCase | 56 | 0x4030 (16432) | 30 | Bose QuietComfort Earbuds | QC Earbuds | - |  |  |
| GwenLeftHook | 57 | 0x403A (16442) | 34 | Bose Sport Open Earbuds | Sport Open Earbuds | - |  |  |
| GwenRightHook | 58 | 0x403A (16442) (shadowed by GwenLeftHook) | 33 | Bose Sport Open Earbuds | Sport Open Earbuds | 25 | earbuds | yes |
| Olivia | 59 | 0x4060 (16480) | 44 | Bose Frames |  | 22 | frames | yes |
| Vedder | 60 | 0x4061 (16481) | 45 | Bose Frames |  | 23 | frames | yes |
| Grandprix | 61 | 0xA300 (41728) | 36 | Bose L1 Pro |  | 16 | PA speaker (BLE only) | yes |
| Babyyoda | 62 | 0xA213 (41491) | 56 | Bose S1 Pro+ |  | 29 | PA speaker (BLE only) | yes |
| Stetson | 63 | 0x4015 (16405) | - | Bose Hearphones |  | - |  |  |
| BeanieDev | 64 | 0x402B (16427) | - | Bose Hearphones II |  | - |  |  |
| BudliteDev | 65 | 0x4034 (16436) | - | Bose Budlite |  | - |  |  |
| IndyRight | 66 | 0x4037 (16439) | 49 | Bose SoundControl Hearing Aids |  | - |  |  |
| IndyLeft | 67 | 0x4037 (16439) (shadowed by IndyRight) | 50 | Bose SoundControl Hearing Aids |  | - |  |  |
| LeviCase | 68 | 0x401A (16410) | - | Levi Case |  | - |  |  |
| Moonraker | 69 | 0x4008 (16392) | - | SoundLink (UNLISTED; DEV ONLY) |  | - |  |  |
| Champ | 70 | 0x4006 (16390) | - | SoundLink Color (UNLISTED; DEV ONLY) |  | - |  |  |
| Kcup | 71 | 0x4009 (16393) | - | SoundLink Mini II (UNLISTED; DEV ONLY) |  | - |  |  |
| Bb1 | 72 | 0x1869 (6249) | - | BOSEbuild (UNLISTED; DEV ONLY) |  | - |  |  |
| Unknown | 73 | 0x0000 (0) | - | Unknown Device |  | - |  |  |
| Smalls | 74 | 0x4064 (16484) | 101 | Bose QC Earbuds II | QC Earbuds II | 32 | earbuds | yes |
| SmallsCase | 75 | 0x4065 (16485) | - | Bose QC Earbuds II Case |  | - |  |  |
| Lonestarr | 76 | 0x4066 (16486) | 103 | Bose QC Ultra Headphones | QC Ultra Headphones | 37 | headphones | yes |
| Edelman | 77 | 0x094F (2383) | 105 | Bose SoundLink Max | SoundLink Max | 44 | BT speaker | yes |
| Mathers | 78 | 0xBC61 (48225) | 107 | Bose SoundLink Flex 2 | Flex 2 SoundLink | 45 | BT speaker | yes |
| Stan | 79 | 0xBC63 (48227) | 115 | Bose SL Flex SE 2 | SL Flex SE 2 | 59 | BT speaker | yes |
| Serena | 80 | 0x4068 (16488) | 102 | Bose Ultra Open Earbuds | Ultra Open Earbuds | 41 | earbuds | yes |
| SerenaCase | 81 | 0x4067 (16487) | - | Bose Ultra Open Earbuds Case |  | - |  |  |
| Scotty | 82 | 0x4072 (16498) | 104 | Bose QC Ultra Earbuds | QC Ultra Earbuds | 38 | earbuds | yes |
| ScottyChargingCase | 83 | 0x4073 (16499) | - | Bose QC Ultra Earbuds Case |  | - |  |  |
| Edith | 84 | 0x4062 (16482) | 108 | Bose QC Ultra 2 Earbuds | QC Ultra 2 Earbuds | 46 | earbuds | yes |
| EdithChargingCase | 85 | 0x4063 (16483) | - | Bose QC Ultra 2 Earbuds Case |  | - |  |  |
| Leo | 86 | 0x4034 (16436) (shadowed by BudliteDev) | 117 | Bose Leo | Leo | 66 | earbuds | yes |
| LeoChargingCase | 87 | 0x4035 (16437) | - | Bose Leo Earbuds Case |  | - |  |  |
| Hawkeye | 88 | 0x4032 (16434) | 118 | Bose Hawkeye | Hawkeye | 77 | headphones | yes |
| MichaelCheung | 89 | 0x403C (16444) | 119 | Bose MichaelCheung |  | 76 | BT speaker | yes |
| KarenMok | 90 | 0x4036 (16438) | 120 | Bose KarenMok |  | 75 | BT speaker | yes |
| Topaz | 91 | 0x4042 (16450) | 121 | Bose Topaz | Topaz | 69 | headphones | yes |
| Emerald | 92 | 0x4043 (16451) | 124 | Bose Emerald | Emerald | 68 | headphones | yes |
| Opal | 93 | 0x404D (16461) | 122 | Bose Opal | Opal | 70 | headphones | yes |
| Sapphire | 94 | 0x404E (16462) | 125 | Bose Sapphire | Sapphire | 72 | earbuds | yes |
| Quartz | 95 | 0x404F (16463) | 126 | Bose Quartz | Quartz | 71 | earbuds | yes |
| Clyde | 96 | 0x4103 (16643) | 127 | Bose Clyde | Clyde | 79 | BT speaker | yes |
| Gracie | 97 | 0x4104 (16644) | 128 | Bose Gracie | Gracie | 78 | BT speaker | yes |
| Brussels | 98 | 0x0023 (35) | 223 | Bose QuietComfort Earbuds | QC Earbuds | 52 | earbuds | yes |
| Troy | 99 | 0x4085 (16517) | 110 | Bose SL Plus | SL Plus | 53 | BT speaker | yes |
| Billie | 100 | 0xBC58 (48216) | 111 | Bose SL Micro 2 | SL Micro 2 | 55 | BT speaker | yes |
| Wolverine | 101 | 0x4082 (16514) | 112 | Bose QC Ultra 2 HP | QC Ultra 2 HP | 54 | headphones | yes |
| Mariah | 102 | 0x4083 (16515) | 113 | Bose QC 2 HP | QuietComfort Headphones 2 | 56 | headphones | yes |
| Aubrey | 103 | 0x4089 (16521) | 114 | Bose Ultra Open 2 Earbuds | Ultra Open 2 Earbuds | 60 | earbuds | yes |
| AubreyCase | 104 | 0x4090 (16528) | - | Bose Ultra Open 2 Earbuds Case |  | - |  |  |
| Spice | 105 | 0x4097 (16535) | 116 | Bose Sport Open Earbuds | Spice | 62 | earbuds | yes |
| SpiceCase | 106 | 0x4098 (16536) | - | Bose Sport Open Earbuds Case |  | - |  |  |
| Unknown120 | 107 | 0x0000 (0) (shadowed by Unknown) | 100 | Unknown Device |  | - |  |  |
| Unknown0241 | 108 | 0x0000 (0) (shadowed by Unknown) | 200 | Unknown Device |  | - |  |  |


Notes on the table:

- Mathers 0xBC61, Stan 0xBC63 and Billie 0xBC58 are built with a `const` that dexdump prints as a
  float (`#float 6.75776e-41 // #0000bc61`); the hex in the comment is the int value. A naive
  register evaluator gets these wrong (the earlier quick script reported 16486/2383/35 for them).
- Grandprix 0xA300, Babyyoda 0xA213 and Chibi 0xA211 are outside the usual 0x40xx range.
- Brussels (0x0023) is the 2024 re-release of the QuietComfort Earbuds; Lando (0x402F/0x4030) is
  the 2020 model with the same `originalName`.
- Duplicate product ids: RevelLeft/RevelRight 0x402D, LandoLeft/LandoRight 0x402F,
  GwenLeftHook/GwenRightHook 0x403A, IndyRight/IndyLeft 0x4037, BudliteDev/Leo 0x4034,
  Unknown/Unknown120/Unknown0241 0. The app whitelists the **Right** entries
  (RevelRight, LandoRight, GwenRightHook) but `getByValue` returns the Left ones; it only matters
  for naming. The bud side of these older earbuds is visible only in the BLE advert id
  (left/right/case have different `bleValue`s).

### 1.3 Variants (`BoseProductVariant`, value = byte 2 of ProductInfo 0.3)

Name prefixes equal to the codename are dropped. Products not listed have only `Unknown` (0).

- **Isaac** (0x400A): 1 Black, 2 White
- **Wolfcastle** (0x400C): 1 Black, 2 Silver, 3 Ornament, 4 Greta
- **Ice** (0x4012): 1 Black, 2 Blue, 3 Yellow
- **Foreman** (0x400D): 1 Black, 2 White, 3 Red, 4 Blue, 5 MidnightBlue, 6 YellowCitron
- **Powder** (0x4014): 1 Black, 2 White, 3 StarkBlack, 4 StarkWhite
- **Flurry** (0x4013): 1 Red
- **Harvey** (0x4011): 1 Black, 2 White
- **Folgers** (0x4010): 1 Black, 2 White
- **Kleos** (0x4017): 1 Black
- **Levi** (0x4018): 1 Black, 2 Citron, 3 Orange, 4 Purple
- **Minnow** (0x4022): 1 Black, 2 Blue, 3 Orange
- **Baywolf** (0x4020): 1 Black, 2 Silver, 3 Navy, 4 Peacock, 5 Nymeria
- **Atlas** (0x4021): 1 Black
- **Goodyear** (0x4024): 1 Black, 2 Silver, 3 Soapstone, 4 Eclipse, 6 MidnightBlue
- **Eddie** (0x4026): 1 Black, 2 White, 3 Silver
- **Professor** (0x4027): 1 Black
- **GingerCheevers** (0x4028): 1 Black, 2 White
- **SanDiego** (0x403D): 1 Black
- **Exeter** (0x403E): 1 Black
- **Flipper** (0x402A): 1 Black, 2 White, 3 Silver
- **Chibi** (0xA211): 1 Black
- **Celine** (0x402C): 1 Alto, 2 Rondo
- **CelineIi** (0x404C): 1 Alto, 2 Rondo
- **Taylor** (0x4033): 1 Black, 2 White, 3 Silver
- **EddieClub** (0x4031): 1 Black
- **Angus** (0x094D): 1 Black, 2 White
- **AngusChina** (0x1873): 1 Black, 2 White
- **Duran** (0x4039): 1 Black, 2 WhiteSmoke, 3 MidnightBlue, 4 EclipseGrey
- **Zakim** (0x094C): 1 Black
- **Malcolm** (0x094E): 1 Black, 2 ThunderstruckBlack
- **MalcolmClub** (0x1875): 1 Black
- **MalcolmChina** (0x1874): 1 Black
- **Stevie** (0x4069): 1 Black, 2 White
- **StevieClub** (0x4070): 1 Black, 2 White
- **StevieChina** (0x4071): 1 Black, 2 White
- **Prince** (0x4075): 1 Black, 2 WhiteSmoke, 3 MidnightBlue, 4 EclipseGrey, 5 CypressGreen, 6 MoonstoneBlue, 7 BlueDusk, 8 ChilledLilac, 9 Sandstone, 10 PetalPink, 11 TwilightBlue, 12 MoonlightGrey, 13 IceBlue
- **LamarrSb** (0x4080): 1 Black
- **LamarrSs** (0x4077): 1 Black
- **Luca** (0x4078): 1 Black, 2 White, 3 BoucleLimitedEdition, 4 KithSilver
- **LucaChina** (0x4091): 1 Black, 2 White
- **Donnie** (0x4079): 1 Black, 2 WhiteSmoke, 3 SandDriftwood, 4 BoucleLimitedEdition, 5 KithSilver
- **DonnieChina** (0x4092): 1 Black, 2 Silver
- **Paulie** (0x4087): 1 Black, 2 White, 3 BoucleLimitedEdition, 4 KithSilver
- **PaulieChina** (0x4093): 1 Black, 2 White
- **Abasi** (0x4094): 1 Black, 2 White
- **AbasiChina** (0x4095): 1 Black, 2 White
- **AbasiClub** (0x4096): 1 Black, 2 White
- **RevelRight** (0x402D): 1 RevelBlack, 2 RevelBalticBlue, 3 RevelGlacierLime
- **LandoRight** (0x402F): 1 LandoBlack, 2 LandoSoapstone, 5 LandoStoneBlue, 6 LandoSandstone
- **GwenRightHook** (0x403A): 1 GwenBlack
- **Olivia** (0x4060): 1 Sports
- **Vedder** (0x4061): 1 CatEye, 2 MinimalSquare
- **Grandprix** (0xA300): 1 Pro8, 2 Pro16, 3 Pro32
- **Babyyoda** (0xA213): 1 Black
- **Stetson** (0x4015): 1 Black
- **Moonraker** (0x4008): 1 Black, 2 White
- **Smalls** (0x4064): 1 Black, 2 Soapstone, 3 MidnightBlue, 4 EclipseGrey
- **Lonestarr** (0x4066): 1 Black, 2 WhiteSmoke, 3 Sandstone, 4 DiamondSixtieth, 5 LunarBlue, 6 DeepPlum
- **Edelman** (0x094F): 1 NueBoseBlack, 2 BlueDusk, 3 CitrusYellow, 4 ClubBlack, 5 ClubBlueDusk
- **Mathers** (0xBC61): 1 NueBoseBlack, 2 BlueDusk, 3 Sandstone, 4 AlpineSage, 5 TwilightBlue, 6 PetalPink, 7 CitrusYellow, 8 FogGrey, 9 SunsetPeach, 10 RosewoodMauve, 11 DewdropMint
- **Stan** (0xBC63): 1 NueBoseBlack, 2 BlueDusk
- **Serena** (0x4068): 1 Black, 2 WhiteSmoke, 3 Kith, 4 DiamondSixtieth, 5 MoonstoneBlue, 6 Sandstone, 7 LunarBlue, 8 ChilledLilac, 9 CarbonBlue, 10 SunsetIridescence, 11 DeepPlum, 12 MidnightViolet, 13 DesertGold, 14 MoonLightGrey, 15 DriftwoodSand, 16 LisaCollab, 17 GoldFiligree, 18 RhodiumFiligree, 19 IvoryMotherOfPearl
- **Scotty** (0x4072): 1 TripleBlack, 2 WhiteSmoke, 3 MoonstoneBlue, 4 DiamondSixtieth, 5 LunarBlue
- **Edith** (0x4062): 1 Black, 2 WhiteSmoke, 3 DeepPlum, 4 MidnightViolet, 5 DesertGold
- **Leo** (0x4034): 1 Black, 2 White
- **Hawkeye** (0x4032): 1 Black
- **MichaelCheung** (0x403C): 1 Black
- **KarenMok** (0x4036): 1 Black
- **Topaz** (0x4042): 1 Black
- **Emerald** (0x4043): 1 Black
- **Opal** (0x404D): 1 Black
- **Sapphire** (0x404E): 1 Black
- **Quartz** (0x404F): 1 Black
- **Clyde** (0x4103): 1 NueBoseBlack
- **Gracie** (0x4104): 1 NueBoseBlack
- **Brussels** (0x0023): 1 Black
- **Troy** (0x4085): 1 Black, 2 BlueDusk, 3 Yellow, 4 SunsetPeach, 5 KITHRed, 6 KITHBlue, 7 KITHGreen
- **Billie** (0xBC58): 1 NueBoseBlack, 2 BlueDusk, 3 YellowCitron, 4 Sandstone, 5 TwilightBlue, 6 PetalPink, 7 SunsetPeach, 8 DewdropMint
- **Wolverine** (0x4082): 1 Black, 2 WhiteSmoke, 3 DriftwoodSand, 4 MidnightViolet, 5 DesertGold, 6 ThomBrowneCollab, 7 MeltedChocolate, 8 OliveGreen, 9 CherryChocolate
- **Mariah** (0x4083): 1 Black, 2 WhiteSmoke, 3 EucalyptusGreen, 4 DewdropMint, 5 RosewoodMauve, 6 VanillaCream, 7 SkyPink, 8 HazelnutTaupe, 9 BlueDusk, 10 VividCobalt
- **Aubrey** (0x4089): 1 Black, 2 WhiteSmoke, 3 CherryChocolate, 4 OliveGreen, 5 SkyPink
- **Spice** (0x4097): 1 Black, 2 WhiteSmoke, 3 EucalyptusGreen, 4 CoastalBlue, 5 IceBlue

---

## 2. Transports: SPP, BLE GATT and LE-CoC

BMAP packets are identical on all three transports; only the framing around them differs.

### 2.1 How the app picks a transport (`Yp0$a.P0`)

In order:

1. **"Product only supports BLE or is already LeAudio connected so only connecting to BLE"**
   (`ux2.a()`): true when the app device type is in `zr2.c` = {16, 29} (Grandprix L1 Pro,
   Babyyoda S1 Pro+), or `Br2.h(type) == 6`, which is true for types
   {3-9, 20, 21, 24, 28, 30, 31, 36, 48-51, 64, 65}: every Wi-Fi soundbar, smart speaker and the Music
   Amplifier. For those products **the app's BMAP traffic goes over BLE GATT** (or LE-CoC, 2.3),
   never over SPP.
2. If the product advertises LE Audio support (`Yp0.W1()`), open BLE first and then continue.
3. If the product "is available to connect to SPP directly" (`Yp0.V1`: bonded / already connected
   classic device), connect RFCOMM SPP only (`kG7.G` -> `BluetoothServiceCore.connectToScannedBoseDevice`).
4. Otherwise open BLE first (for discovery, static MAC address and pairing), then SPP.

All headphones, earbuds, Frames and Bluetooth speakers in the table are controlled over SPP in
normal use (the SPP channel is described in `status-devices-audio.md` section 5.2). The
`BleBoseDevice.shouldSegmentPacket` list (Authentication, Wi-Fi EncryptedConfigureNetwork,
AudioManagement ToneMatchPreset, firmware data transfer) shows which large packets the app expects
to send over BLE: Wi-Fi setup and the PA speaker tone-match presets.

### 2.2 BLE GATT (`ble/BleConnectionManager`)

| UUID | name in code | use |
|---|---|---|
| `0000febe-0000-0000-0000-000000000000` | `SERVICE_UUID` | defined but never used (note the non-standard base; Bose's 16-bit service is 0xFEBE). Characteristics are looked up by UUID alone. |
| `C65B8F2F-AEE2-4C89-B758-BC4892D6F2D8` | `RWN_SECURE_CHARACTERISTIC_UUID` | BMAP read/write/notify, needs bonding (the app bonds first, `pairWithDeviceIfSecure`) |
| `D417C028-9818-4354-99D1-2AC09D074591` | `RWN_UNSECURE_CHARACTERISTIC_UUID` | same, unencrypted. Used if the secure one is missing or fails, or when `forceUnsecureCharacteristic` is set |
| `089CA084-6721-4419-9003-5E14D5AB587D` | `RN_LECOC_PSM_CHARACTERISTIC_UUID` | read: L2CAP PSM for LE-CoC (section 2.3) |

Connection sequence (`callConnect`): connect (25 s timeout, 3 retries 2 s apart), request MTU
(`defaultMtuSize` = 55, `GLOBAL_MTU_SAFE_SIZE`; constants also define 23 default and 104 large),
wait 1000 ms (`discoverServicesDelay`), pick the secure or unsecure characteristic, enable
notifications on it (8 s timeout), set write type 2 (`WRITE_TYPE_DEFAULT`, with response), then
send ProductInfo BmapVersion Get (`00 01 01 00`, 2 s timeout, one retry), then
ProductIdVariants and AllFblocks.

**Framing on the characteristic.** Each write and each notification carries a one-byte segment
header before the BMAP bytes:

```
header = (lastSegmentIndex << 4) | segmentIndex        (both 0..15)
```

- A packet that fits (`len(packet) <= mtu - 4`) is written as `00 <packet>` (`asBmapWriteData`).
- Otherwise `PacketSegmentationUtil.segmentPacketData(packet, mtu - 3)` cuts it into chunks of
  `mtu - 4` bytes; segment *i* of *n* is `[(n-1)<<4 | i] <chunk>`. Max 16 segments.
- Incoming notifications are buffered until `isLastDataSegment`: header `00`, or high nibble ==
  low nibble. `getDataFromSegments` checks that every high nibble equals n-1, strips the header and
  places segment *i* at offset `i * (len(first segment) - 1)`.
- App bug, do not copy: when the packet length is an exact multiple of `mtu - 4`, the last
  segment is computed as `len % chunk + 1` = 1 byte (header only), so the final chunk is lost.
  Send `n = ceil(len / chunk)` full segments instead.

Example, MTU 55 (52-byte ATT payload, 51 BMAP bytes per segment):

```
-> 00 00 01 01 00                 BmapVersion Get, single segment
<- 00 00 01 03 05 31 2e 32 2e 30  BmapVersion Status "1.2.0"
104-byte ToneMatchPreset SetGet (108 bytes with header) ->
-> 20 05 0b 02 68 <47 bytes>      segment 0 of 3
-> 21 <51 bytes>
-> 22 <6 bytes>
```

### 2.3 LE-CoC (L2CAP connection-oriented channel)

After the GATT session is up, the app reads the PSM characteristic (`089CA084-...`), converts the
bytes big-endian to an int, and treats LE-CoC as supported if 128 <= PSM <= 255
(`LECOC_MIN_L2CAP_PSM`/`MAX`). It then opens an L2CAP channel to that PSM
(`createInsecureL2capChannel`, or on Android 16+ a socket with encryption required, authentication
not required). On that socket BMAP packets are written raw, **without** the segment header, and the
reader splits reads by the BMAP length byte exactly like SPP (8192-byte read buffer). So LE-CoC is
"SPP over BLE".

### 2.4 BLE advert (manufacturer data, AD type 0xFF)

`ManufacturerDataParser.get` picks a parser from the first byte of the manufacturer data:
`0x9E` = BMAP 1.2.0 format (`ManufacturerDataParser120`), `0x10`/`0x00` legacy, `0x01` 1.0.4,
`0x02` ITH, `0x03` Rio, `0x04` charging case, `0x05` Indy (hearing aid), `0x41` Bragi 0241. For the
1.2.0 format: byte 2 = format (0 = SPITFIRE), byte 3 bits 0-4 = BLE product id **minus 100**,
bits 5-7 = variant, bytes 5-7 = `prand`, bytes 8-10 = `resolvableMacAddress` (when the data is
long enough). So
`bleValue` 101..128 in the table are 1.2.0-format adverts; the Bragi format adds 200 (Brussels =
223).

---

## 3. Earbuds and multi-component products

### 3.1 Status.BatteryLevel (2.2) with several components

Already decoded in `status-devices-audio.md` section 1.1; summary for earbuds. Get `02 02 01 00`.
Payload of more than 4 bytes = consecutive 4-byte entries
`[percent u8][remaining minutes u16 BE, 0xFFFF = unknown][ComponentId u8]`.

`ComponentId`: -1 Unknown, **0 Single, 1 RightBud, 2 LeftBud, 3 ChargingCase**.

```
<- 02 02 03 0c  50 00 b4 01  4b 00 aa 02  3c ff ff 03
   right 80 % 180 min, left 75 % 170 min, case 60 % (time unknown)     (illustrative)
```

A bud that is not reported (out of range, in a closed case) is simply absent.

### 3.2 Status.InEar (2.9)

- Get `02 09 01 00` (`StatusInEarGetPacket`). Reply Status, 1 byte (`StatusInEarResponse`):
  **bit 0 = left bud in ear, bit 1 = right bud in ear**. Empty payload throws.
- `02 09 03 01 03` = both in, `02 09 03 01 02` = only right in.
- Parsed whenever it arrives, so with Status notifications enabled (`status-devices-audio.md` 5.1)
  it is pushed on every insert/remove.

### 3.3 Status.ChargingCaseLid (2.10)

Get `02 0a 01 00` -> 1 byte `LidStatus` 0 Unknown, 1 Open, 2 Closed (see
`status-devices-audio.md` 1.3). Control function 9 is "ChargingCaseLidStatus" in the enum but the
app never builds or parses it.

### 3.4 Status.MoistureDetection (2.19) and MicMute (2.21)

- Moisture: Get `02 13 01 00` -> Status 1 byte, `== 1` means moisture detected
  (`isMoistureDetectedIn`). Empty payload = false.
- MicMute: Get `02 15 01 00` -> Status 1 byte, `== 1` means the microphone is muted
  (`StatusMicMuteEnabledResponse`). Read only; the app has no Set for it here.

### 3.5 HearingAssistance.Muting (12.11): mute one earbud

`getMutedEarbuds` / `setAndGetMutedEarbuds` (app classes `Fu3`/`Gu3`/`Hu3`):

- Get `0c 0b 01 00`, SetGet `0c 0b 02 01 <v>`, reply Status 1 byte.
- `HearingAssistanceMuting`: **0 NoEar, 1 LeftEar, 2 RightEar, 3 BothEar** (unknown values -> NoEar).
  The naming suggests "which ears are muted"; whether the value means muted or active ears is not
  stated in the code.

### 3.6 ProductInfo functions for components

| fn | name | request the app sends | reply payload |
|---|---|---|---|
| 0.11 | ComponentDevices | Get `00 0b 01 00` | N x 6 bytes, each a Bluetooth address (`formatMacAddress`); length must be a multiple of 6 |
| 0.17 | ComponentId | none (parsed only) | 1 byte `ComponentId`, or 7 bytes `ComponentId` + 6-byte address; other lengths throw |
| 0.18 | EarbudMode | none (parsed only) | 1 byte `ProductInfoComponentRole`: 0 Unknown, 1 Master, 2 Puppet |
| 0.19 | PeerId | Get `00 13 01 00` | u32 BE (first 4 bytes; fewer than 4 throws) |
| 0.20 | MacAddressForRole | none (parsed only) | 6-byte address |
| 0.21 | MacAddressForComponent | none (parsed only) | 6-byte address |

- ComponentDevices: the order of the addresses (right/left/case) is not stated anywhere in the
  app; it is just a list.
- ComponentId tells which physical part the phone is talking to (right bud, left bud, case);
  EarbudMode which bud is the primary ("Master") that relays BMAP for the other.
- The request formats for 0.17/0.18/0.20/0.21 are not in the app. 0.17/0.18 are presumably plain
  Get (`00 11 01 00`, `00 12 01 00`); 0.20/0.21 probably take a role/component byte. They also
  arrive in a ProductInfo GetAll.
- **Port addressing:** `ProductInfoPackets.getSerialNumberPacket(port)` and
  `getFirmwareVersionPacket(port)` put a port number in bits 4-5 of byte 2 (e.g. port 1 serial number
  Get = `00 07 11 00`). The app uses this for per-component serial / firmware version (and firmware
  update uses ports too). Port 0 is the product itself. Which port is which bud is not in the code;
  try 1 and 2.

### 3.7 Control.Chirp (7.2): find my buds

Documented in `status-devices-audio.md` 4.4: Get `07 02 01 00` -> Status 1 byte (bit 0 in progress,
bits 1+ `ChirpStopReasonCode` 0 NoEnd, 1 DeviceEnd, 2 TimerEnd, 3 AppEnd, 4 CradleEnd); Start
`07 02 05 01 <ChirpStartValue>` with 0 End, 1 Begin, 2 Continue -> Result `<duration u8>`. There
is no per-bud target in the packet; the product decides which bud(s) chirp.

### 3.8 Control.FitQuality (7.15): ear tip fit test

- Start `07 0f 05 00` (`ControlFitQualityStartPacket`); the app waits up to **30 s** for the Result.
- Replies: Processing `07 0f 07 00`; Status `07 0f 03 04 <o> <r> <d> <l>`
  (`FBlockControlKt.parseFitQualityInfo`); Result `07 0f 06 ..` (no payload parsed).
  Presumably one Status per bud, keyed by the orientation byte.

| byte | field | values |
|---|---|---|
| 0 | `FitQualityBudOrientation` | 0 Unspecified, 1 Left, 2 Right |
| 1 | `FitQualityBudPersonalizationResult` | 0 PersonalizationComplete, 1 ErrorNotDonned, 2 ErrorInCall, 3 ErrorAudioStreaming, 4 ErrorDisconnected, 5 ErrorTimeout, 6 ErrorBusy, 7 ErrorNotSupported |
| 2 | `FitQualityPersonalizationDebugInfo` | 0 Success, 1 Error |
| 3 | `FitQualityLevel` | 0 FreeAir, 1 NonCustomizable, 2 Customizable |

Unknown values map to each enum's `Unknown`.

---

## 4. AudioManagement extras (fblock 5)

The parser (`BmapAudioManagementResponseParser`) accepts Status and Error for all of these, plus
Processing and Result for SpatialAudioCalibration.

### 4.1 SpatialAudioMode (5.15) and SpatialAudioStatus (5.16)

- Mode Get `05 0f 01 00`, SetGet `05 0f 02 01 <mode>`, Status 1 byte `SpatialAudioModeTypes`:
  **0 SpatialDisabled, 1 SpatialAudioFixedToRoom, 2 SpatialAudioFixedToHead**
  (presumably the app's "Still" and "Motion"),
  anything else Unknown. Same enum as the AudioModes per-mode spatial field (`audio-modes.md`).
  On products with the AudioModes block, spatial audio is normally part of the mode config;
  5.15 is the standalone setting.
- Status Get `05 10 01 00` -> 1 byte `SpatialAudioStatusTypes`: 0 SpatialAudioAsDesired,
  1 SpatialAudioNotAsDesiredGeneric (e.g. unsupported source), else Unknown. Read only.

```
-> 05 0f 02 01 02     set head-tracked spatial audio
<- 05 0f 03 01 02
```

### 4.2 SpatialAudioCalibration (5.17)

- Get `05 11 01 00` -> Status `[type][n][n bytes calibration data]`
  (`SpatialAudioCalibrationInfo`; data is bytes 2 .. 2+n).
- Start `05 11 05 00` -> Processing `05 11 07 00`, then Result `05 11 06 01 <type>`.
- `SpatialAudioCalibrationTypes`: 0 Factory, 1 Manual, 2 Automatic. The data blob is not
  interpreted by the app.

### 4.3 PA speaker mixer: ChannelCount (5.10), ChannelParameter (5.8), capabilities (5.14)

These are used by the PA speakers (L1 Pro "Grandprix", S1 Pro+ "Babyyoda", over BLE). The strings
`ppa_volume_parameter_name` "VOLUME", `ppa_treble_parameter_name`, `ppa_bass_parameter_name`,
`ppa_reverb_parameter_name` belong to this screen. They are **not** a general speaker EQ;
Bluetooth speakers and headphones use Settings.RangeControl (1.7, `settings.md`) for bass/mid/treble.

- **ChannelCount** Get `05 0a 01 00` -> Status 1 byte = number of input channels.
- **ChannelParameter** Get `05 08 01 02 <channel> <param>` -> Status
  `[channel][param][totalSteps][currentStep]` (4 bytes, signed bytes in the app).
  SetGet `05 08 02 03 <channel> <param> <step>`.
- `ChannelParameterType`: 0 Volume, 1 Treble, 2 Bass, 3 Reverb, 4 Mute, 5 ReverbMute,
  6 EqPreset, 7 PhantomPower, 8 HouseCurve, 9 EffectsInsert, 10 RfWireless, 11 PositionEq,
  12 LiveStream, 13 SubEq.
- **ChannelParameterCapabilities** (5.14) Get `05 0e 01 00` -> Status, one byte per parameter type,
  indexed by the enum ordinal 0..13 (so 14 bytes; a shorter payload crashes the app's parser).
  In each byte, **bit j set = channel j supports that parameter** (the app does
  `Integer.toString(b, 2).reversed()` and collects indexes of `'1'`; a byte with bit 7 set is
  negative in Java and becomes `"-..."`, so channel 7 is effectively never reported).
  Only requested for BMAP >= 1.2.0 (`status-devices-audio.md` section 7).

```
-> 05 08 01 02 00 00            channel 0, Volume
<- 05 08 03 04 00 00 1f 0a      31 steps, at step 10                 (illustrative)
-> 05 08 02 03 00 00 0c         set step 12
```

### 4.4 ToneMatchPreset (5.11)

ToneMatch is the PA speakers' per-channel instrument/microphone EQ; the presets ship as
`assets/ppa_gp_tonematch_01_instruments.json` and `..._microphones.json`.

- Get `05 0b 01 02 <channel> <slot>`.
- SetGet `05 0b 02 68 <channel> <slot> <102 bytes>` (104-byte payload, segmented over BLE).
- Status: same 104-byte layout.

`ToneMatchPresetData` (102 bytes, all big endian):

| offset | size | field |
|---|---|---|
| 0 | 2 | `identifier`, signed 16-bit (preset id) |
| 2 | 4 | `b0Combined` float32 |
| 6 + 16*k | 4 x float32 | biquad section k = 0..5: `b1Norm`, `b2Norm`, `a1Neg`, `a2Neg` |

So the preset is a 6-stage biquad cascade with one shared gain `b0` and normalised coefficients.

---

## 5. Vpa fblock (16 / 0x10): voice assistant

Parser `BmapVPAResponseParser` handles only FblockInfo (16.0), GetAll (16.1), SupportedVpas (16.2)
and Wuw (16.3). **Language (16.4) and Trigger (16.5) exist in `BmapFunction` but the app has no
packet, parser or model for them**; their formats are unknown.

- FblockInfo Get `10 00 01 00` (generic FunctionBlockInfo reply).
- GetAll Start `10 01 05 00` -> Processing, a Status for each supported function, Result.

### 5.1 SupportedVpas (16.2): which assistant the action button talks to

- Get `10 02 01 00`. Status payload:

| byte | field |
|---|---|
| 0 | bits 0-6 selected `VoicePersonalAssistant`, bit 7 `isConnected` |
| 1.. | bitset of supported assistants (bit n = assistant value n) |

- SetGet `10 02 02 01 <assistant>` (no connected bit).
- `VoicePersonalAssistant`: **0 GoogleAssistant, 1 Alexa, 2 Native (presumably the phone's own
  assistant), 3 Xiaowei, 127 None**; anything else Unknown (-1).

```
<- 10 02 03 02 82 0d      selected Native, connected; supported {0,2,3}=Google, Native, Xiaowei
                          (bitset 0x0d)                                  (illustrative)
-> 10 02 02 01 02         select Native
```

### 5.2 Wuw (16.3): wake-up word

- Get `10 03 01 00`. Status payload:

| byte | field |
|---|---|
| 0 | bits 0-5 selected `WakeUpWord`, bit 6 `isEnabled`, bit 7 `isReadOnly` |
| 1.. | bitset of supported wake words |

- SetGet `10 03 02 01 <(enabled << 6) | word>`.
- `WakeUpWord`: **0 OK_GOOGLE, 1 ALEXA, 63 NONE**; anything else UNKNOWN.

```
-> 10 03 02 01 41    "Alexa" wake word, enabled
-> 10 03 02 01 01    "Alexa" selected but disabled
```

---

## Uncertain / not established

1. The SPP-vs-BLE decision was read from the app layer (`Yp0$a.P0`, `ux2`, `zr2`, `Br2`), which is
   obfuscated; the meaning of the `zr2` sets is inferred from which products they contain.
2. App device-type codes: Goodyear and Baywolf both come out as 1 and Rio as 2 from the switch
   table. Prince = 40 matches the earlier note; the others were not cross-checked.
3. Order of addresses in ComponentDevices (0.11), and which port number addresses which bud.
4. Request formats of ProductInfo 0.17, 0.18, 0.20, 0.21 (never built by the app).
5. Whether FitQuality sends one Status per bud.
6. HearingAssistance.Muting value semantics (muted ears vs active ears).
7. VPA Language (16.4) and Trigger (16.5): not implemented in the app at all.
8. ChannelParameter step semantics (signed? 0-based?) and the meaning of `channel` numbers on each
   PA product.
9. The BLE example in 2.2 is built from the code; no BLE traffic was captured.

---

## Classes read

- `com.bose.bmap.messages.enums`: `BoseProductId` (+`$1`), `BoseProductIdSupport`,
  `BoseProductVariant` (+`$Companion.forValue`), `ProductType`, `ComponentId`, `ChannelParameterType`,
  `SpatialAudioModeTypes`, `SpatialAudioStatusTypes`, `SpatialAudioCalibrationTypes`,
  `FitQualityBudOrientation`, `FitQualityBudPersonalizationResult`, `FitQualityLevel`,
  `FitQualityPersonalizationDebugInfo`, `VoicePersonalAssistant`, `WakeUpWord`, `ChirpStartValue`,
  `ChirpStopReasonCode`, `HearingAssistanceMuting`, `spec.BmapFunctionBlock`
- `messages.packets`: `StatusInEarGetPacket`, `StatusMicMuteEnabledGetPacket`,
  `StatusMoistureDetectionGetPacket`, `AudioManagement{ChannelCount,ChannelParameter,ChannelParameterCapabilities,SpatialAudioMode,SpatialAudioStatus,SpatialAudioCalibration,ToneMatchPreset}*Packet`,
  `ControlFitQualityStartPacket`, `VPA*Packet`
- `messages.responses`: `StatusInEar/MicMuteEnabled/MoistureDetection`, `ProductInfo{ComponentDevices,ComponentId,EarbudMode,PeerId,MacAddress}Response`,
  `ProductInfoComponentRole`, `AudioManagement*Response` for the functions above,
  `ControlFitQuality*Response`, `FBlockControlKt.parseFitQualityInfo`, `VPASupportedVPAsStatusResponse`,
  `VPAWUWStatusResponse`
- `messages.models`: `audio/{ChannelParameterCapabilities,ToneMatchPresetData,SpatialAudioCalibrationInfo}`,
  `control/FitQualityInfo`, `vpa/{SupportedVpas,SupportedWakeUpWords,WakeUpWordInfo,SupportedVoicePersonalAssistantInfo}`,
  `utils/ByteUtils.convertBytesToFloat`
- `messages.parsers`: `BmapProductInfoResponseParser`, `BmapStatusResponseParser`,
  `BmapAudioManagementResponseParser`, `BmapControlResponseParser` (+`$WhenMappings`), `BmapVPAResponseParser`
- `model.factories.ProductInfoPackets`; `model.parsers.{AdvertisingDataParser,ManufacturerDataParser,ManufacturerDataParser120,ManufacturerDataParser104,LegacyManufacturerDataParser,RioDataParser,BragiDataParser0241,BmapVersion120FormatTypes}`
- `ble.{BleConnectionManager,BleBoseDevice,LecocConnectionManager(+$ConnectingThread,$ConnectedThread)}`,
  `utils.PacketSegmentationUtil`, `service.BluetoothServiceCore`, `service.bmap.BmapInterfaceImplementation`
- App layer (obfuscated): `YK0` (whitelist), `aL0`/`aL0$a`/`aL0$a$a` (product -> device type),
  `zr2` (device-type sets), `Br2.h`, `ux2`, `Yp0`/`Yp0$a.P0`/`Yp0.p3` (transport choice, LE-CoC),
  `tg0.C`, `nw0.j`, `nK0.n0`, `dw0.k1`, `gB6`, `Fu3`/`Gu3`/`Hu3` (earbud muting)
- `resources.arsc` default string table (`product_help_*_title`, `accessibility_help_*_item`,
  `brussels_*`), APK `assets/`
