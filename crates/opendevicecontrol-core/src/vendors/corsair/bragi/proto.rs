//! Bragi packet encoding and decoding (pure functions, unit-tested).
//!
//! Every packet is one HID output report of the device's packet size (64 or
//! 1024 bytes, zero padded):
//!
//! | Byte | Meaning |
//! |---|---|
//! | 0 | `0x08 | slot`: slot 0 is the device itself, 1..=7 a device behind a wireless receiver |
//! | 1 | command (`CMD_*`) |
//! | 2.. | arguments |
//!
//! Replies echo bytes 0 and 1; byte 2 is a status code (0 = success) and a
//! property value follows in bytes 3..7 (little endian).

pub const MAGIC: u8 = 0x08;

// Commands.
pub const CMD_SET: u8 = 0x01;
pub const CMD_GET: u8 = 0x02;
pub const CMD_CLOSE_HANDLE: u8 = 0x05;
pub const CMD_WRITE: u8 = 0x06;
pub const CMD_WRITE_MORE: u8 = 0x07;
pub const CMD_READ: u8 = 0x08;
pub const CMD_PROBE_HANDLE: u8 = 0x09;
pub const CMD_OPEN_HANDLE: u8 = 0x0d;
/// Keepalive / ping (CUE sends it about every 50 s; ckb-next too).
pub const CMD_PING: u8 = 0x12;

// Properties.
pub const PROP_POLL_RATE: u8 = 0x01;
pub const PROP_BRIGHTNESS: u8 = 0x02;
pub const PROP_MODE: u8 = 0x03;
pub const PROP_ANGLE_SNAP: u8 = 0x07;
pub const PROP_BATTERY_LEVEL: u8 = 0x0f;
pub const PROP_BATTERY_STATUS: u8 = 0x10;
pub const PROP_VID: u8 = 0x11;
pub const PROP_PID: u8 = 0x12;
pub const PROP_APP_VERSION: u8 = 0x13;
pub const PROP_DPI_X: u8 = 0x21;
pub const PROP_DPI_Y: u8 = 0x22;
pub const PROP_SUBDEVICES: u8 = 0x36;
pub const PROP_HW_LAYOUT: u8 = 0x41;
pub const PROP_BRIGHTNESS_COARSE: u8 = 0x44;
pub const PROP_KEY_LOCKS: u8 = 0x4a;

pub const MODE_HARDWARE: u32 = 0x01;
pub const MODE_SOFTWARE: u32 = 0x02;

// Resources opened on a handle.
pub const RES_LIGHTING: u16 = 0x01;
pub const RES_LIGHTING_MONO: u16 = 0x10;
/// Lighting with a 2-byte header, used by the K100/K70 PRO family.
pub const RES_LIGHTING_ALT: u16 = 0x22;
pub const RES_LIGHTING_EXTRA: u16 = 0x2e;
pub const RES_KEY_ASSIGNMENT: u16 = 0x02;

/// Handles used by opendevicecontrol.
pub const HANDLE_LIGHTING: u8 = 0x00;
pub const HANDLE_GENERIC: u8 = 0x01;

// Status codes.
pub const STATUS_OK: u8 = 0x00;
pub const STATUS_BUSY: u8 = 0x03;
pub const STATUS_UNSUPPORTED: u8 = 0x05;

/// Unsolicited packet types (byte 1 of an input report).
pub const IN_NOTIFY: u8 = 0x01;
pub const IN_KEYS: u8 = 0x02;
/// Notification id (byte 2) for receiver connection changes.
pub const NOTIFY_SUBDEVICES: u8 = 0x36;

/// Header byte for `slot` (0 = wired/direct).
pub const fn header(slot: u8) -> u8 {
    MAGIC | (slot & 0x07)
}

/// Slot a received packet belongs to, or `None` if it is not Bragi.
pub fn slot_of(b0: u8) -> Option<u8> {
    (b0 & 0xf8 == MAGIC).then_some(b0 & 0x07)
}

fn packet(size: usize, bytes: &[u8]) -> Vec<u8> {
    let mut p = vec![0u8; size];
    p[..bytes.len()].copy_from_slice(bytes);
    p
}

pub fn get(size: usize, slot: u8, prop: u8) -> Vec<u8> {
    packet(size, &[header(slot), CMD_GET, prop, 0x00])
}

pub fn set(size: usize, slot: u8, prop: u8, value: u32) -> Vec<u8> {
    let v = value.to_le_bytes();
    packet(size, &[header(slot), CMD_SET, prop, 0x00, v[0], v[1], v[2], v[3]])
}

pub fn ping(size: usize, slot: u8) -> Vec<u8> {
    packet(size, &[header(slot), CMD_PING])
}

pub fn open_handle(size: usize, slot: u8, handle: u8, resource: u16) -> Vec<u8> {
    let r = resource.to_le_bytes();
    packet(size, &[header(slot), CMD_OPEN_HANDLE, handle, r[0], r[1]])
}

pub fn close_handle(size: usize, slot: u8, handle: u8) -> Vec<u8> {
    packet(size, &[header(slot), CMD_CLOSE_HANDLE, 0x01, handle])
}

/// Split `data` into the packets that write it to `handle`: the first one
/// (`06`) carries the total length (u32 LE) and data from byte 7, the
/// following ones (`07`) data from byte 3.
pub fn write_handle(size: usize, slot: u8, handle: u8, data: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let len = (data.len() as u32).to_le_bytes();
    let mut first = packet(size, &[header(slot), CMD_WRITE, handle, len[0], len[1], len[2], len[3]]);
    let n = data.len().min(size - 7);
    first[7..7 + n].copy_from_slice(&data[..n]);
    out.push(first);
    let mut rest = &data[n..];
    while !rest.is_empty() {
        let mut p = packet(size, &[header(slot), CMD_WRITE_MORE, handle]);
        let n = rest.len().min(size - 3);
        p[3..3 + n].copy_from_slice(&rest[..n]);
        out.push(p);
        rest = &rest[n..];
    }
    out
}

/// A parsed reply to a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reply {
    pub slot: u8,
    pub cmd: u8,
    pub status: u8,
    /// Bytes 3..7, little endian.
    pub value: u32,
}

pub fn parse_reply(r: &[u8]) -> Option<Reply> {
    if r.len() < 7 {
        return None;
    }
    let slot = slot_of(r[0])?;
    Some(Reply { slot, cmd: r[1], status: r[2], value: u32::from_le_bytes([r[3], r[4], r[5], r[6]]) })
}

/// `major.minor.patch` from a `PROP_APP_VERSION` reply (bytes 3, 4, 5..7).
pub fn firmware_string(r: &[u8]) -> Option<String> {
    if r.len() < 7 || r[3..7].iter().all(|&b| b == 0) {
        return None;
    }
    Some(format!("{}.{}.{}", r[3], r[4], u16::from_le_bytes([r[5], r[6]])))
}

/// Colour data for `RES_LIGHTING`: planar, all reds, then greens, then blues.
pub fn planar_colors(colors: &[(u8, u8, u8)]) -> Vec<u8> {
    let n = colors.len();
    let mut d = vec![0u8; n * 3];
    for (i, c) in colors.iter().enumerate() {
        d[i] = c.0;
        d[n + i] = c.1;
        d[2 * n + i] = c.2;
    }
    d
}

/// Colour data for `RES_LIGHTING_ALT`: `12 00` then RGB triplets.
pub fn triplet_colors(colors: &[(u8, u8, u8)]) -> Vec<u8> {
    let mut d = Vec::with_capacity(2 + colors.len() * 3);
    d.extend_from_slice(&[0x12, 0x00]);
    for c in colors {
        d.extend_from_slice(&[c.0, c.1, c.2]);
    }
    d
}

/// Battery state reported by `PROP_BATTERY_STATUS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryStatus {
    Charging,
    Discharging,
    Full,
    Unknown,
}

impl BatteryStatus {
    pub fn from_raw(v: u32) -> Self {
        match v {
            1 => BatteryStatus::Charging,
            2 => BatteryStatus::Discharging,
            3 => BatteryStatus::Full,
            _ => BatteryStatus::Unknown,
        }
    }
}

/// Battery level in percent from `PROP_BATTERY_LEVEL` (tenths of a percent).
pub fn battery_percent(raw: u32) -> f64 {
    (raw as f64 / 10.0).clamp(0.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_and_set_layout() {
        // ckb-next bragi_get_property / bragi_set_property.
        assert_eq!(&get(64, 0, PROP_MODE)[..5], &[0x08, 0x02, 0x03, 0x00, 0x00]);
        assert_eq!(&set(64, 0, PROP_MODE, MODE_SOFTWARE)[..7], &[0x08, 0x01, 0x03, 0x00, 0x02, 0x00, 0x00]);
        // Brightness 1000 = 0x03e8.
        assert_eq!(&set(64, 0, PROP_BRIGHTNESS, 1000)[..6], &[0x08, 0x01, 0x02, 0x00, 0xe8, 0x03]);
        // OpenRGB: wireless sub-device 1 uses 0x09.
        assert_eq!(get(64, 1, PROP_PID)[0], 0x09);
        assert_eq!(get(1024, 0, PROP_PID).len(), 1024);
    }

    #[test]
    fn handles() {
        assert_eq!(&open_handle(64, 0, 0, RES_LIGHTING_ALT)[..6], &[0x08, 0x0d, 0x00, 0x22, 0x00, 0x00]);
        assert_eq!(&close_handle(64, 0, 0)[..5], &[0x08, 0x05, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn write_single_packet() {
        let p = write_handle(64, 0, 0, &[1, 2, 3]);
        assert_eq!(p.len(), 1);
        assert_eq!(&p[0][..11], &[0x08, 0x06, 0x00, 0x03, 0x00, 0x00, 0x00, 1, 2, 3, 0]);
    }

    #[test]
    fn write_multi_packet() {
        // 57 bytes fit in the first 64-byte packet, 61 in each further one.
        let data: Vec<u8> = (0..130u32).map(|i| i as u8).collect();
        let p = write_handle(64, 2, 0, &data);
        assert_eq!(p.len(), 3);
        assert_eq!(&p[0][..7], &[0x0a, 0x06, 0x00, 130, 0, 0, 0]);
        assert_eq!(p[0][7], 0);
        assert_eq!(p[0][63], 56);
        assert_eq!(&p[1][..4], &[0x0a, 0x07, 0x00, 57]);
        assert_eq!(p[1][63], 117);
        assert_eq!(&p[2][..4], &[0x0a, 0x07, 0x00, 118]);
        assert_eq!(p[2][3 + 11], 129);
        assert_eq!(p[2][3 + 12], 0);
        assert!(p.iter().all(|x| x.len() == 64));
    }

    #[test]
    fn reply_parsing() {
        let mut r = [0u8; 64];
        r[..7].copy_from_slice(&[0x09, 0x02, 0x00, 0xb5, 0x1b, 0x00, 0x00]);
        let rep = parse_reply(&r).unwrap();
        assert_eq!(rep, Reply { slot: 1, cmd: 0x02, status: 0, value: 0x1bb5 });
        assert!(parse_reply(&[0x03, 0, 0, 0, 0, 0, 0]).is_none());
        assert_eq!(slot_of(0x0f), Some(7));
        assert_eq!(slot_of(0x01), None);
    }

    #[test]
    fn firmware() {
        // OpenLinkHub: "%d.%d.%d" of fw[3], fw[4], LE u16 fw[5..7].
        assert_eq!(firmware_string(&[0x08, 0x02, 0x00, 1, 20, 0x2a, 0x01]).as_deref(), Some("1.20.298"));
        assert_eq!(firmware_string(&[0x08, 0x02, 0x00, 0, 0, 0, 0]), None);
    }

    #[test]
    fn colour_layouts() {
        let c = [(1, 2, 3), (4, 5, 6)];
        assert_eq!(planar_colors(&c), vec![1, 4, 2, 5, 3, 6]);
        assert_eq!(triplet_colors(&c), vec![0x12, 0x00, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn battery() {
        assert_eq!(battery_percent(1000), 100.0);
        assert_eq!(battery_percent(455), 45.5);
        assert_eq!(battery_percent(5000), 100.0);
        assert_eq!(BatteryStatus::from_raw(1), BatteryStatus::Charging);
        assert_eq!(BatteryStatus::from_raw(3), BatteryStatus::Full);
    }
}
