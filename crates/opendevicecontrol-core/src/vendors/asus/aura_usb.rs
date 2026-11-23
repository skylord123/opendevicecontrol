//! ASUS motherboards' Aura lighting, over the board's USB LED controller.

use crate::devices::{Device, Ids};
use std::ffi::CString;

pub(crate) fn discover(
    _api: &hidapi::HidApi,
    _all: &[&hidapi::DeviceInfo],
    _ids: &mut Ids,
    _skip_paths: &[CString],
    _out: &mut Vec<(CString, Box<dyn Device>)>,
) {
}
