//! A device's HID++ channel together with its feature table.

use super::features::Features;
use super::hidpp::Channel;
use crate::{Error, Result};

pub struct Link {
    pub ch: Channel,
    pub features: Features,
    /// HID++ protocol version, `(1, 0)` for register-based devices.
    pub protocol: (u8, u8),
}

impl Link {
    pub fn hidpp10(&self) -> bool {
        self.protocol.0 < 2
    }

    pub fn has(&self, feature: u16) -> bool {
        self.features.has(feature)
    }

    pub fn version(&self, feature: u16) -> u8 {
        self.features.version(feature)
    }

    /// Call `function` of `feature`.
    pub fn call(&mut self, feature: u16, function: u8, params: &[u8]) -> Result<Vec<u8>> {
        let f = self.features.get(feature).ok_or(Error::NotSupported(""))?;
        self.ch.feature(f.index, function, params)
    }

    /// Call without waiting for the answer.
    pub fn call_no_reply(&mut self, feature: u16, function: u8, params: &[u8]) -> Result<()> {
        let f = self.features.get(feature).ok_or(Error::NotSupported(""))?;
        self.ch.feature_no_reply(f.index, function, params)
    }

    pub fn read_register(&mut self, register: u16, params: &[u8]) -> Result<Vec<u8>> {
        self.ch.read_register(register, params)
    }

    pub fn write_register(&mut self, register: u16, value: &[u8]) -> Result<Vec<u8>> {
        self.ch.write_register(register, value)
    }

    /// The feature id of a notification's feature index.
    pub fn feature_of(&self, index: u8) -> Option<u16> {
        self.features.by_index(index).map(|f| f.id)
    }
}

/// Big-endian integer from up to 4 bytes.
pub fn be(bytes: &[u8]) -> u32 {
    bytes.iter().take(4).fold(0, |a, &b| (a << 8) | b as u32)
}

/// `v` as `n` big-endian bytes.
pub fn to_be(v: u32, n: usize) -> Vec<u8> {
    (0..n).rev().map(|i| (v >> (8 * i)) as u8).collect()
}

/// Byte `i` of `d`, 0 past the end.
pub fn at(d: &[u8], i: usize) -> u8 {
    d.get(i).copied().unwrap_or(0)
}

/// Big-endian u16 at `i`.
pub fn u16_at(d: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([at(d, i), at(d, i + 1)])
}
