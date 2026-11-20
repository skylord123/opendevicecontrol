//! BMAP (Bose Messaging and Protocol), as spoken over the headphones' SPP
//! channel. Pure byte handling; see docs/reverse-engineering/bose-qc-headphones.md.
//!
//! Every packet is `[fblock, function, flags, length, payload…]` where
//! `flags = (device << 6) | (port << 4) | operator`. The app always sends
//! device 0, port 0. Reads are `Get` with an empty payload; writes are
//! `SetGet`, answered with the new `Status` (or an `Error`).

/// Function blocks (`BmapFunctionBlock`).
pub mod fblock {
    pub const PRODUCT_INFO: u8 = 0;
    pub const SETTINGS: u8 = 1;
    pub const STATUS: u8 = 2;
    pub const DEVICE_MANAGEMENT: u8 = 4;
    pub const AUDIO_MANAGEMENT: u8 = 5;
    pub const CONTROL: u8 = 7;
    pub const AUDIO_MODES: u8 = 31;
}

/// Operators (`BmapOperator`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Set = 0,
    Get = 1,
    SetGet = 2,
    Status = 3,
    Error = 4,
    Start = 5,
    Result = 6,
    Processing = 7,
}

impl Op {
    fn from(b: u8) -> Option<Op> {
        Some(match b & 0x0f {
            0 => Op::Set,
            1 => Op::Get,
            2 => Op::SetGet,
            3 => Op::Status,
            4 => Op::Error,
            5 => Op::Start,
            6 => Op::Result,
            7 => Op::Processing,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub fblock: u8,
    pub function: u8,
    pub op: Op,
    pub payload: Vec<u8>,
}

impl Packet {
    pub fn new(fblock: u8, function: u8, op: Op, payload: &[u8]) -> Packet {
        Packet { fblock, function, op, payload: payload.to_vec() }
    }

    pub fn get(fblock: u8, function: u8) -> Packet {
        Packet::new(fblock, function, Op::Get, &[])
    }

    pub fn set_get(fblock: u8, function: u8, payload: &[u8]) -> Packet {
        Packet::new(fblock, function, Op::SetGet, payload)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![self.fblock, self.function, self.op as u8, self.payload.len() as u8];
        out.extend_from_slice(&self.payload);
        out
    }

    /// The error code of an `Error` packet (`BmapPacketError`).
    pub fn error(&self) -> Option<u8> {
        (self.op == Op::Error).then(|| self.payload.first().copied().unwrap_or(0))
    }
}

/// Split a byte stream into packets; returns the packets and how many
/// bytes they used (the rest is an incomplete packet).
pub fn decode(buf: &[u8]) -> (Vec<Packet>, usize) {
    let mut out = Vec::new();
    let mut i = 0;
    while buf.len() - i >= 4 {
        let len = buf[i + 3] as usize;
        if buf.len() - i < 4 + len {
            break;
        }
        if let Some(op) = Op::from(buf[i + 2]) {
            out.push(Packet { fblock: buf[i], function: buf[i + 1], op, payload: buf[i + 4..i + 4 + len].to_vec() });
        }
        i += 4 + len;
    }
    (out, i)
}

/// Readable names for `BmapPacketError` codes.
pub fn error_name(code: u8) -> &'static str {
    match code {
        1 => "invalid length",
        2 => "invalid checksum",
        3 => "function block not supported",
        4 => "function not supported",
        5 => "operator not supported",
        6 => "invalid data",
        7 => "data not available",
        8 => "temporary read/write failure",
        9 => "timed out",
        10 => "not possible right now",
        11 => "device not in the paired list",
        12 => "busy",
        13 => "couldn't connect to the paired device",
        14 => "couldn't connect: the pairing was deleted on the other device",
        15 => "a firmware update is in progress",
        16 => "battery too low",
        17 => "needs a charger",
        18 => "not allowed",
        19 => "unknown port",
        20 => "insecure transport",
        21 => "invalid key",
        22 => "out of memory",
        _ => "unknown error",
    }
}

pub fn be32(b: &[u8]) -> u32 {
    b.iter().take(4).fold(0, |acc, &x| (acc << 8) | x as u32)
}

/// Settings fblock (1) functions and their payloads.
pub mod settings {
    use super::be32;

    pub const PRODUCT_NAME: u8 = 2;
    pub const VOICE_PROMPTS: u8 = 3;
    pub const STANDBY_TIMER: u8 = 4;
    pub const CNC: u8 = 5;
    pub const RANGE_CONTROL: u8 = 7;
    pub const BUTTONS: u8 = 9;
    pub const MULTIPOINT: u8 = 10;
    pub const SIDETONE: u8 = 11;

    /// The name follows a flags byte (bit 0: it is the factory name).
    pub fn product_name(p: &[u8]) -> Option<String> {
        Some(String::from_utf8_lossy(p.get(1..)?).trim_end_matches('\0').to_string())
    }

    /// Voice prompt languages (`VoicePromptLanguage`), by value.
    pub const LANGUAGES: [&str; 23] = [
        "English (UK)",
        "English (US)",
        "French",
        "Italian",
        "German",
        "Spanish (Europe)",
        "Spanish (Mexico)",
        "Portuguese (Brazil)",
        "Mandarin",
        "Korean",
        "Russian",
        "Polish",
        "Hebrew",
        "Turkish",
        "Dutch",
        "Japanese",
        "Cantonese",
        "Arabic",
        "Swedish",
        "Danish",
        "Norwegian",
        "Finnish",
        "Hindi",
    ];

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct VoicePrompts {
        pub enabled: bool,
        pub language: u8,
        /// Bit n: language n is available.
        pub supported: u32,
    }

    pub fn voice_prompts(p: &[u8]) -> Option<VoicePrompts> {
        let b = *p.first()?;
        Some(VoicePrompts { enabled: b & 0x20 != 0, language: b & 0x1f, supported: be32(p.get(1..5)?) })
    }

    /// The language is always sent with the on/off bit.
    pub fn set_voice_prompts(enabled: bool, language: u8) -> Vec<u8> {
        vec![((enabled as u8) << 5) | (language & 0x1f)]
    }

    /// Auto-off in minutes, 0 = never.
    pub fn standby_timer(p: &[u8]) -> Option<u16> {
        match p {
            [m] => Some(*m as u16),
            [lo, _flags, hi, ..] => Some(u16::from_le_bytes([*lo, *hi])),
            _ => None,
        }
    }

    pub fn set_standby_timer(minutes: u16) -> Vec<u8> {
        if minutes <= 255 { vec![minutes as u8] } else { minutes.to_le_bytes().to_vec() }
    }

    /// The minutes the Bose app offers (0 = never).
    pub const STANDBY_CHOICES: [u16; 7] = [5, 20, 40, 60, 180, 1440, 0];

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Cnc {
        pub steps: u8,
        pub step: u8,
        pub enabled: bool,
    }

    pub fn cnc(p: &[u8]) -> Option<Cnc> {
        match p {
            [steps, step, flags, ..] => Some(Cnc { steps: *steps, step: *step, enabled: flags & 1 != 0 }),
            _ => None,
        }
    }

    /// The headphones' level counts awareness: 0 is full noise cancelling
    /// (the Quiet mode stores 0, Aware stores 10 of 11 steps, checked on the
    /// QC Headphones). Shown the other way round, as the Bose app's slider
    /// does (`mapBetweenCncLevelAndWorldVolume`); the mapping is its own
    /// inverse.
    pub fn cnc_display(level: u8, steps: u8) -> u8 {
        steps.saturating_sub(1).saturating_sub(level)
    }

    pub fn set_cnc(step: u8, enabled: bool) -> Vec<u8> {
        vec![step, enabled as u8]
    }

    /// One equaliser band (`RangeControlIdentifier`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Band {
        pub id: u8,
        pub min: i8,
        pub max: i8,
        pub value: i8,
    }

    pub fn band_name(id: u8) -> &'static str {
        match id {
            0 => "Bass",
            1 => "Mid",
            2 => "Treble",
            3 => "Low mid",
            4 => "High mid",
            _ => "Band",
        }
    }

    pub fn range_control(p: &[u8]) -> Vec<Band> {
        p.as_chunks::<4>().0.iter().map(|c| Band { min: c[0] as i8, max: c[1] as i8, value: c[2] as i8, id: c[3] }).collect()
    }

    pub fn set_band(id: u8, value: i8) -> Vec<u8> {
        vec![value as u8, id]
    }

    /// Action button modes (`ActionButtonMode`), by value.
    pub const BUTTON_MODES: [&str; 28] = [
        "Not configured",
        "Voice assistant",
        "Noise cancelling on/off",
        "Battery level",
        "Play/pause",
        "More noise cancelling",
        "Less noise cancelling",
        "Wake word on/off",
        "Switch source device",
        "Conversation mode",
        "Next track",
        "Previous track",
        "Read notifications",
        "Wind mode",
        "Disabled",
        "Client interaction",
        "Spotify Tap",
        "Cycle modes",
        "Cycle volume presets",
        "Spatial audio mode",
        "Line-in switch",
        "Linking",
        "Panic (local)",
        "Panic (remote)",
        "Panic (both)",
        "Third-party service",
        "Smart action",
        "Alexa",
    ];

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Button {
        pub id: u8,
        pub event: u8,
        pub mode: u8,
        /// Bit n: mode n can be chosen (bit 0 is ignored).
        pub supported: u32,
    }

    pub fn buttons(p: &[u8]) -> Option<Button> {
        Some(Button { id: *p.first()?, event: *p.get(1)?, mode: *p.get(2)?, supported: be32(p.get(3..7)?) & !1 })
    }

    pub fn set_button(id: u8, event: u8, mode: u8) -> Vec<u8> {
        vec![id, event, mode]
    }

    /// (enabled, can be switched off).
    pub fn multipoint(p: &[u8]) -> Option<(bool, bool)> {
        let b = *p.first()?;
        Some((b & 1 != 0, b & 4 != 0))
    }

    pub const ANR: u8 = 6;
    pub const ON_HEAD: u8 = 16;
    pub const TEAMS_BUTTON: u8 = 26;
    pub const VOLUME_CONTROL: u8 = 28;

    /// On/off settings: `[0/1]` to set, Status byte 0 bit 0 is the state.
    /// (function, control id, name, help). None of these exist on the QC
    /// Headphones; layouts are from the app's code (settings-more.md).
    pub const TOGGLES: [(u8, &str, &str, &str); 10] = [
        (13, "conversation_mode", "Conversation mode", "Pause audio and let voices in when you start talking."),
        (20, "motion_auto_off", "Motion detection", "Switch off when the headphones haven't moved for a while."),
        (22, "flip_to_off", "Lay flat to disconnect", "Disconnect when the frames are laid flat."),
        (24, "auto_play_pause", "Auto play/pause", "Pause when you take an earbud out and resume when you put it back."),
        (27, "auto_answer", "Auto answer calls", "Answer a call when you put an earbud in."),
        (29, "auto_aware", "Auto Aware mode", "Switch to Aware mode automatically."),
        (32, "auto_volume", "Auto volume", "Adjust the volume to the noise around you."),
        (34, "touch_controls", "Touch controls", "Tap and swipe controls on the touch surface."),
        (37, "le_audio", "LE Audio", "Use Bluetooth LE Audio where the source supports it."),
        (39, "head_gestures", "Head gestures", "Nod or shake your head to answer or decline calls."),
    ];

    pub fn toggle(p: &[u8]) -> Option<bool> {
        p.first().map(|b| b & 1 != 0)
    }

    /// Discrete noise reduction of older models (`AnrMode`), by value.
    pub const ANR_MODES: [&str; 4] = ["Off", "High", "Wind", "Low"];

    /// (mode, supported-mode mask).
    pub fn anr(p: &[u8]) -> Option<(u8, u32)> {
        Some((*p.first()?, be32(p.get(1..).unwrap_or(&[]))))
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct OnHead {
        pub enabled: bool,
        /// Auto play/pause, auto answer, auto transparency: (supported, on).
        pub features: [(bool, bool); 3],
    }

    pub const ON_HEAD_FEATURES: [(&str, &str); 3] =
        [("on_head_auto_play", "Auto play/pause"), ("on_head_auto_answer", "Auto answer calls"), ("on_head_auto_aware", "Auto transparency")];

    pub fn on_head(p: &[u8]) -> Option<OnHead> {
        let (a, b) = (*p.first()?, *p.get(1)?);
        Some(OnHead { enabled: a & 1 != 0, features: [0, 1, 2].map(|i| (a & (2 << i) != 0, b & (1 << i) != 0)) })
    }

    pub fn set_on_head(h: &OnHead) -> Vec<u8> {
        let flags = h.features.iter().enumerate().fold(0u8, |acc, (i, &(sup, on))| acc | (((sup && on) as u8) << i));
        vec![h.enabled as u8, flags]
    }

    pub const TEAMS_MODES: [&str; 2] = ["Teams and voice assistant", "Teams only"];

    pub const VOLUME_CONTROL_MODES: [&str; 4] = ["Off", "Button", "Swipe", "Motion"];

    /// (current, supported-mode mask with bit n = value n; Off always).
    /// The app reads "motion" as `(b & 3) == 3`, which looks like a bug;
    /// bit 2 is taken here.
    pub fn volume_control(p: &[u8]) -> Option<(u8, u8)> {
        let cur = *p.first()?;
        let sup = p.get(1).map_or(0x0f, |b| 1 | ((b & 7) << 1));
        Some((cur, sup))
    }

    pub const SIDETONE_MODES: [&str; 4] = ["Off", "High", "Medium", "Low"];

    /// (mode, supported-mode mask).
    pub fn sidetone(p: &[u8]) -> Option<(u8, u8)> {
        Some((*p.get(1)?, p.get(2).copied().unwrap_or(0x0f)))
    }

    pub fn set_sidetone(mode: u8) -> Vec<u8> {
        vec![1, mode]
    }
}

/// Status fblock (2).
pub mod status {
    pub const BATTERY_LEVEL: u8 = 2;
    pub const CHARGER: u8 = 5;
    pub const IN_EAR: u8 = 9;
    pub const CASE_LID: u8 = 10;

    /// One battery of a reply: (`ComponentId`, percent, minutes left).
    /// Components: 0 the product itself, 1 right bud, 2 left bud, 3 case.
    /// A part that isn't reachable (a bud in a closed case) is left out.
    pub fn batteries(p: &[u8]) -> Vec<(u8, u8, Option<u16>)> {
        p.as_chunks::<4>().0.iter()
            .filter(|c| c[0] <= 100)
            .map(|c| {
                let m = i16::from_be_bytes([c[1], c[2]]);
                (c[3], c[0], (m >= 0).then_some(m as u16))
            })
            .collect()
    }

    /// (left in ear, right in ear).
    pub fn in_ear(p: &[u8]) -> Option<(bool, bool)> {
        p.first().map(|b| (b & 1 != 0, b & 2 != 0))
    }

    /// The charging case lid: Some(true) open, Some(false) closed.
    pub fn lid_open(p: &[u8]) -> Option<bool> {
        match p.first() {
            Some(1) => Some(true),
            Some(2) => Some(false),
            _ => None,
        }
    }

    /// Percent of the first (or only) battery.
    pub fn battery(p: &[u8]) -> Option<u8> {
        p.first().copied().filter(|&b| b <= 100)
    }

    /// Minutes left, when the headphones estimate it.
    pub fn minutes_left(p: &[u8]) -> Option<u16> {
        let v = i16::from_be_bytes([*p.get(1)?, *p.get(2)?]);
        (v >= 0).then_some(v as u16)
    }

    pub fn charging(p: &[u8]) -> Option<bool> {
        p.first().map(|&b| b == 1)
    }
}

/// DeviceManagement fblock (4): the headphones' paired devices.
pub mod devices {
    pub const CONNECT: u8 = 1;
    pub const DISCONNECT: u8 = 2;
    pub const LIST: u8 = 4;
    pub const INFO: u8 = 5;
    pub const PAIRING_MODE: u8 = 8;

    /// (address, connected) for every paired device. Byte 0 is a bitmask
    /// of which entries have audio connected (bit n = entry n).
    pub fn list(p: &[u8]) -> Vec<([u8; 6], bool)> {
        let Some((&mask, rest)) = p.split_first() else { return Vec::new() };
        rest.as_chunks::<6>().0.iter().enumerate().map(|(i, c)| (*c, i < 8 && mask & (1 << i) != 0)).collect()
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Info {
        pub address: [u8; 6],
        pub connected: bool,
        /// The device this BMAP link runs on.
        pub local: bool,
        pub name: String,
    }

    /// An Info reply: `[mac6, flags, …]`, flags bit 0 connected, bit 1 the
    /// local device, bit 2 a Bose product (the name then starts at 10, else 9).
    pub fn info(p: &[u8]) -> Option<Info> {
        let address: [u8; 6] = p.get(..6)?.try_into().ok()?;
        let flags = *p.get(6)?;
        let at = if flags & 4 != 0 { 10 } else { 9 };
        let name = p.get(at..).map(|n| String::from_utf8_lossy(n).trim_end_matches('\0').trim().to_string()).unwrap_or_default();
        Some(Info { address, connected: flags & 1 != 0, local: flags & 2 != 0, name })
    }

    /// Pairing mode state from a Status or Result: byte 0 == 1.
    pub fn pairing(p: &[u8]) -> bool {
        p.first() == Some(&1)
    }

    pub fn connect(address: &[u8; 6]) -> Vec<u8> {
        let mut v = vec![0];
        v.extend_from_slice(address);
        v
    }
}

/// AudioManagement fblock (5): playback and volume.
pub mod audio {
    pub const SOURCE: u8 = 1;
    pub const SPATIAL: u8 = 15;

    /// Spatial audio (`SpatialAudioModeTypes`), by value.
    pub const SPATIAL_MODES: [&str; 3] = ["Off", "Still", "Motion"];

    /// The address of the current audio source, from a Source Status
    /// `[supported u16, type, mac6]` (type 1 = Bluetooth).
    pub fn source(p: &[u8]) -> Option<[u8; 6]> {
        (p.get(2) == Some(&1)).then(|| p.get(3..9)?.try_into().ok()).flatten()
    }

    pub const CONTROL: u8 = 3;
    pub const STATUS: u8 = 4;
    pub const VOLUME: u8 = 5;

    pub const PLAY: u8 = 1;
    pub const PAUSE: u8 = 2;
    pub const NEXT: u8 = 3;
    pub const PREVIOUS: u8 = 4;

    /// (max, current).
    pub fn volume(p: &[u8]) -> Option<(u8, u8)> {
        Some((*p.first()?, *p.get(1)?))
    }

    /// True while playing (`AudioStatusValue` 1).
    pub fn playing(p: &[u8]) -> Option<bool> {
        p.first().map(|&b| b == 1)
    }
}

/// Vpa fblock (16): the voice assistant.
pub mod vpa {
    pub const ASSISTANT: u8 = 2;
    pub const WAKE_WORD: u8 = 3;

    /// `VoicePersonalAssistant` values and names.
    pub const ASSISTANTS: [(u8, &str); 5] = [(0, "Google Assistant"), (1, "Alexa"), (2, "Phone's assistant"), (3, "Xiaowei"), (127, "None")];

    /// (selected, supported bitset over values 0..=31). None is always offered.
    pub fn assistant(p: &[u8]) -> Option<(u8, u32)> {
        let sel = *p.first()? & 0x7f;
        Some((sel, super::be32(p.get(1..).unwrap_or(&[]))))
    }

    /// (word, enabled, read only).
    pub fn wake_word(p: &[u8]) -> Option<(u8, bool, bool)> {
        let b = *p.first()?;
        Some((b & 0x3f, b & 0x40 != 0, b & 0x80 != 0))
    }

    pub fn set_wake_word(word: u8, enabled: bool) -> Vec<u8> {
        vec![((enabled as u8) << 6) | (word & 0x3f)]
    }
}

/// Control fblock (7).
pub mod control {
    pub const POWER: u8 = 4;
    /// Start payload that switches the headphones off.
    pub const POWER_OFF: [u8; 1] = [0];
}

/// Notification fblock (9): ask for unsolicited Status packets.
pub mod notify {
    pub const BY_FBLOCK: u8 = 2;

    /// ByFblock SetGet that adds `fblocks` (Enable mask 1) to the set the
    /// headphones push updates for. The bitset is big-endian with bit 0 in
    /// the last byte, at least 2 bytes long.
    pub fn enable(fblocks: &[u8]) -> Vec<u8> {
        let top = fblocks.iter().copied().max().unwrap_or(0) as usize;
        let len = (top / 8 + 1).max(2);
        let mut set = vec![0u8; len];
        for &f in fblocks {
            set[len - 1 - f as usize / 8] |= 1 << (f % 8);
        }
        let mut out = vec![1];
        out.extend(set);
        out
    }
}

/// AudioModes fblock (31): Quiet, Aware and the user's own modes.
pub mod modes {
    pub const CAPABILITIES: u8 = 2;
    pub const CURRENT: u8 = 3;
    pub const CONFIG: u8 = 6;
    pub const FAVORITES: u8 = 8;

    /// Mode prompts (`AudioModesPrompt`): the name the app shows for a mode.
    pub const PROMPTS: [&str; 37] = [
        "", "Quiet", "Aware", "Transparent", "Transparency", "Masking", "Comfort", "Commute", "Outdoor", "Workout", "Home", "Work", "Music",
        "Focus", "Relax", "Flight", "Airport", "Driving", "Training", "Gym", "Run", "Walk", "Hike", "Talk", "Call", "Whisper", "Hearing",
        "Learn", "Podcast", "Audiobook", "Calm", "Sleep", "Meditate", "Yoga", "Immersion", "Stereo", "Cinema",
    ];

    /// (Bose modes, user slots): the number of modes is their sum.
    pub fn capabilities(p: &[u8]) -> Option<(u8, u8)> {
        Some((*p.first()?, *p.get(1)?))
    }

    /// Switch modes (Start), with the headphones announcing the new one.
    pub fn select(index: u8) -> Vec<u8> {
        vec![index, 1]
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Mode {
        pub index: u8,
        pub prompt: u8,
        pub name: String,
        pub user_configurable: bool,
        pub favorite: bool,
        /// Which settings can be changed (bit 0 CNC, 1 auto-CNC, 2 spatial,
        /// 3 wind block, 4 ANC toggle).
        pub mutable: u8,
        pub cnc: u8,
        pub auto_cnc: bool,
        pub spatial: Option<u8>,
        pub wind: Option<bool>,
        pub anc: Option<bool>,
    }

    impl Mode {
        /// An empty user slot.
        pub fn empty(&self) -> bool {
            self.prompt == 0 && self.name.is_empty()
        }

        /// The name to show: the stored name, else the prompt's.
        pub fn label(&self) -> String {
            if !self.name.is_empty() {
                self.name.clone()
            } else {
                PROMPTS.get(self.prompt as usize).filter(|p| !p.is_empty()).map(|p| p.to_string()).unwrap_or_else(|| format!("Mode {}", self.index + 1))
            }
        }

        /// The ModeConfig SetGet payload that stores this mode as it is.
        /// Optional fields are sent exactly when the headphones reported
        /// them, with the app's placeholders for the gaps.
        pub fn encode(&self) -> Vec<u8> {
            let mut v = vec![self.index, 0, self.prompt];
            let mut name = [0u8; 32];
            let mut n = self.name.len().min(31);
            while !self.name.is_char_boundary(n) {
                n -= 1;
            }
            name[..n].copy_from_slice(&self.name.as_bytes()[..n]);
            v.extend_from_slice(&name);
            v.push(self.cnc);
            v.push(self.auto_cnc as u8);
            match (self.spatial, self.wind) {
                (Some(sp), Some(w)) => v.extend_from_slice(&[sp, w as u8]),
                (Some(sp), None) => v.extend_from_slice(&[sp, 0]),
                (None, Some(w)) => v.extend_from_slice(&[0, w as u8]),
                (None, None) => {}
            }
            if let Some(a) = self.anc {
                v.push(a as u8);
            }
            v
        }
    }

    /// A ModeConfig Status (47 bytes on the QC Headphones; the optional
    /// tail fields may be missing on other models).
    pub fn config(p: &[u8]) -> Option<Mode> {
        if p.len() < 44 {
            return None;
        }
        let name = &p[6..38];
        let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        Some(Mode {
            index: p[0],
            prompt: if p[1] == 0 { p[2] } else { 0 },
            name: String::from_utf8_lossy(&name[..end]).trim().to_string(),
            user_configurable: p[3] == 1,
            favorite: p[5] == 1,
            mutable: p[41],
            cnc: p[42],
            auto_cnc: p[43] == 1,
            spatial: p.get(44).copied().filter(|&b| b != 0xff),
            wind: p.get(46).filter(|&&b| b != 0xff).map(|&b| b == 1),
            anc: p.get(47).filter(|&&b| b != 0xff).map(|&b| b == 1),
        })
    }

    /// Favourites: `[count, bitmask…]`, mode i at bit i%8 of the byte
    /// `ceil(count/8) - i/8` (big-endian).
    pub fn favorites(p: &[u8]) -> Vec<u8> {
        let Some((&n, mask)) = p.split_first() else { return Vec::new() };
        (0..n).filter(|&i| mask.len().checked_sub(1 + i as usize / 8).is_some_and(|b| mask[b] & (1 << (i % 8)) != 0)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let p = Packet::set_get(fblock::SETTINGS, settings::CNC, &[5, 1]);
        assert_eq!(p.encode(), [1, 5, 2, 2, 5, 1]);
        let mut stream = p.encode();
        stream.extend_from_slice(&[0x01, 0x05, 0x03]);
        let (got, used) = decode(&stream);
        assert_eq!(got, vec![p]);
        assert_eq!(used, 6);
    }

    #[test]
    fn decodes_the_headphones_settings() {
        // Live replies from Bose QC Headphones (0x4075), firmware 1.0.6.
        let name = hex("00426f7365205143204865616470686f6e6573");
        assert_eq!(settings::product_name(&name).unwrap(), "Bose QC Headphones");
        let vp = settings::voice_prompts(&hex("e10001815e0000")).unwrap();
        assert!(vp.enabled);
        assert_eq!(settings::LANGUAGES[vp.language as usize], "English (US)");
        assert_eq!(settings::cnc(&hex("0b0a03")), Some(settings::Cnc { steps: 11, step: 10, enabled: true }));
        // Aware (level 10) shows as no noise cancelling, Quiet (0) as full.
        assert_eq!(settings::cnc_display(10, 11), 0);
        assert_eq!(settings::cnc_display(0, 11), 10);
        let eq = settings::range_control(&hex("f60a0800f60a0001f60a0302"));
        assert_eq!(eq.iter().map(|b| (b.id, b.value)).collect::<Vec<_>>(), [(0, 8), (1, 0), (2, 3)]);
        assert_eq!(eq[0].min, -10);
        let b = settings::buttons(&hex("8009030001400800000080")).unwrap();
        assert_eq!((b.id, b.event, b.mode), (0x80, 9, 3));
        assert_eq!(settings::multipoint(&hex("07")), Some((true, true)));
        assert_eq!(settings::sidetone(&hex("01020f")), Some((2, 0x0f)));
        assert_eq!(settings::standby_timer(&hex("00")), Some(0));
    }

    #[test]
    fn decodes_the_headphones_status() {
        assert_eq!(status::battery(&hex("46ffff00")), Some(70));
        assert_eq!(status::minutes_left(&hex("46ffff00")), None);
        // A ListDevices reply (addresses replaced): four devices, the first connected.
        let l = devices::list(&hex("01112233445501112233445502112233445503112233445504"));
        assert_eq!(l.len(), 4);
        assert_eq!(l[0], ([0x11, 0x22, 0x33, 0x44, 0x55, 0x01], true));
        assert!(!l[1].1);
        assert_eq!(audio::volume(&hex("2017")), Some((32, 23)));
        assert_eq!(audio::source(&hex("000201112233445501")), Some([0x11, 0x22, 0x33, 0x44, 0x55, 0x01]));
        let i = devices::info(&hex("1122334455010105 0c50697865 6c".replace(' ', "").as_str())).unwrap();
        assert_eq!((i.connected, i.local, i.name.as_str()), (true, false, "Pixel"));
        let bose = devices::info(&hex("112233445501 06 4075 01 5143".replace(' ', "").as_str())).unwrap();
        assert_eq!((bose.local, bose.name.as_str()), (true, "QC"));
    }

    #[test]
    fn other_settings() {
        // Examples from the app's code (settings-more.md), not captured.
        assert_eq!(settings::anr(&hex("010b")), Some((1, 0x0b)));
        let h = settings::on_head(&hex("0701")).unwrap();
        assert!(h.enabled);
        assert_eq!(h.features, [(true, true), (true, false), (false, false)]);
        assert_eq!(settings::set_on_head(&h), [1, 1]);
        assert_eq!(settings::volume_control(&hex("0202")), Some((2, 0b101)));
    }

    #[test]
    fn earbuds() {
        // Illustrative replies from the app's code (products-and-earbuds.md).
        let b = status::batteries(&hex("5000b4014b00aa023cffff03"));
        assert_eq!(b, vec![(1, 80, Some(180)), (2, 75, Some(170)), (3, 60, None)]);
        assert_eq!(status::in_ear(&hex("02")), Some((false, true)));
        assert_eq!(status::lid_open(&hex("02")), Some(false));
        assert_eq!(vpa::assistant(&hex("820d")), Some((2, 0x0d)));
        assert_eq!(vpa::set_wake_word(1, true), [0x41]);
    }

    #[test]
    fn notification_bitset_matches_the_app() {
        // ProductInfo, Settings, Status, DeviceManagement, AudioManagement,
        // Notification, Authentication, AudioModes → 80 04 02 37.
        assert_eq!(notify::enable(&[0, 1, 2, 4, 5, 9, 18, 31]), hex("0180040237"));
    }

    #[test]
    fn reads_the_headphones_modes() {
        // Live ModeConfig replies (47 bytes): Aware, and an empty user slot.
        let aware = hex("0100020000014177617265000000000000000000000000000000000000000000000000000000000000000a00000000");
        let m = modes::config(&aware).unwrap();
        assert_eq!((m.index, m.prompt, m.label().as_str(), m.cnc, m.mutable, m.user_configurable), (1, 2, "Aware", 10, 0, false));
        let slot = hex("02000001000000000000000000000000000000000000000000000000000000000000000000000000000900000000 00".replace(' ', "").as_str());
        let m = modes::config(&slot).unwrap();
        assert!(m.empty() && m.user_configurable);
        assert_eq!((m.mutable, m.wind), (9, Some(false)));
    }

    #[test]
    fn modes() {
        assert_eq!(modes::capabilities(&hex("020200000009")), Some((2, 2)));
        assert_eq!(modes::favorites(&hex("0403")), vec![0, 1]);
        assert_eq!(modes::favorites(&hex("040b")), vec![0, 1, 3]);
        // Status layout → SetGet layout, wind block present, spatial and
        // ANC toggle absent (the agent's worked example).
        let mut st = vec![2, 0, 7, 1, 1, 0];
        let mut name = b"Commute".to_vec();
        name.resize(32, 0);
        st.extend(name);
        st.extend_from_slice(&[0, 0, 0, 0x09, 5, 0, 0xff, 0, 1, 0xff]);
        let m = modes::config(&st).unwrap();
        assert_eq!((m.index, m.prompt, m.label().as_str(), m.cnc, m.wind), (2, 7, "Commute", 5, Some(true)));
        let enc = m.encode();
        assert_eq!(enc.len(), 39);
        assert_eq!(&enc[35..], &[5, 0, 0, 1]);
    }

    pub(crate) fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }
}
