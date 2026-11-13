//! opendevicecontrol-core: device protocols for Corsair hardware.
//!
//! The crate is UI-agnostic. Every supported device implements [`Device`],
//! which exposes a generic list of sensors and controls so that front-ends
//! (GUI, Home Assistant bridge, CLI) never need device-specific code.

pub mod devices;
pub mod keymap;
pub mod linear11;
pub mod model;
pub mod smbus;
pub mod system;
pub mod transport;
pub mod vendors;

pub use devices::{discover, Device};
pub use model::*;

/// Corsair's USB vendor ID.
pub const CORSAIR_VID: u16 = 0x1b1c;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HID error: {0}")]
    Hid(#[from] hidapi::HidError),
    #[error("device did not answer command {cmd:#04x} (another program such as iCUE may be talking to it)")]
    NoReply { cmd: u8 },
    #[error("device rejected command {cmd:#04x}")]
    Unsupported { cmd: u8 },
    #[error("unknown control '{0}'")]
    UnknownControl(String),
    #[error("invalid value for control '{0}'")]
    InvalidValue(String),
    /// SMBus / I²C access (RAM lighting).
    #[error("SMBus: {0}")]
    Bus(String),
    /// A wireless device that is switched off, asleep or out of range.
    #[error("device is asleep or out of range")]
    Offline,
    #[error("not supported by this device{0}")]
    NotSupported(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;
