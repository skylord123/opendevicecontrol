//! ASRock motherboards' Polychrome RGB lighting, over the board's USB LED
//! controller (`26ce:01a2`).

pub mod polychrome;
pub mod proto;

use crate::devices::{Device, Ids};
use std::ffi::CString;

pub(crate) fn discover(
    api: &hidapi::HidApi,
    all: &[&hidapi::DeviceInfo],
    ids: &mut Ids,
    skip_paths: &[CString],
    out: &mut Vec<(CString, Box<dyn Device>)>,
) {
    // One HID interface; ASRock's utility opens the first match.
    let Some(d) = all.iter().find(|d| d.vendor_id() == proto::VID && d.product_id() == proto::PID) else { return };
    if skip_paths.iter().any(|p| p.as_c_str() == d.path()) {
        return;
    }
    let id = ids.next("asrock-polychrome");
    match api.open_path(d.path()).map_err(crate::Error::from).and_then(|dev| polychrome::Polychrome::open(dev, id.clone(), d)) {
        Ok(p) => {
            log::info!("opened ASRock Polychrome RGB ({id}), firmware {}", p.info().firmware.as_deref().unwrap_or("unknown"));
            out.push((d.path().to_owned(), Box::new(p)));
        }
        Err(e) => log::warn!("failed to open ASRock Polychrome RGB: {e}"),
    }
}
