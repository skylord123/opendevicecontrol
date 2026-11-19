//! Drivers, grouped by device maker. Each vendor module exposes a
//! `discover()` that `crate::devices::discover` calls with the shared HID
//! device list.

pub mod asrock;
pub mod asus;
pub mod aula;
pub mod corsair;
pub mod speedlink;
pub mod system76;
