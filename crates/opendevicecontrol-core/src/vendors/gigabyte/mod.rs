//! Gigabyte motherboards' RGB Fusion 2, over the board's USB LED controller
//! (ITE `048d:8297`, `048d:5702`, `048d:5711`, `048d:8950`).

pub mod fusion2;
pub mod proto;

use crate::devices::{Device, Ids};
use std::ffi::{CStr, CString};

pub(crate) fn discover(
    api: &hidapi::HidApi,
    all: &[&hidapi::DeviceInfo],
    ids: &mut Ids,
    skip_paths: &[CString],
    out: &mut Vec<(CString, Box<dyn Device>)>,
) {
    let ours: Vec<&hidapi::DeviceInfo> = all.iter().copied().filter(|d| d.vendor_id() == proto::VID && proto::PIDS.contains(&d.product_id())).collect();
    // The lighting collection is usage page 0xFF89, usage 0xCC (the
    // interface also has a usage 0x10 one). Where the backend reports no
    // usages for a path, take its first entry.
    let mut seen: Vec<&CStr> = Vec::new();
    for d in &ours {
        let path = d.path();
        if seen.contains(&path) {
            continue;
        }
        let same: Vec<&&hidapi::DeviceInfo> = ours.iter().filter(|o| o.path() == path).collect();
        let lighting = (d.usage_page(), d.usage()) == (proto::USAGE_PAGE, proto::USAGE);
        let no_usages = same.iter().all(|o| o.usage_page() != proto::USAGE_PAGE);
        let first = std::ptr::eq(*same[0], *d);
        if !(lighting || (no_usages && first)) {
            continue;
        }
        seen.push(path);
        if skip_paths.iter().any(|p| p.as_c_str() == path) {
            continue;
        }
        let id = ids.next("gigabyte-rgb-fusion2");
        match api.open_path(path).map_err(crate::Error::from).and_then(|dev| fusion2::Fusion2::open(dev, id.clone(), d)) {
            Ok(f) => {
                log::info!(
                    "opened Gigabyte RGB Fusion 2 ({id}, {:04x}:{:04x}), firmware {}",
                    proto::VID,
                    d.product_id(),
                    f.info().firmware.as_deref().unwrap_or("unknown")
                );
                out.push((path.to_owned(), Box::new(f)));
            }
            Err(e) => log::warn!("failed to open Gigabyte RGB Fusion 2 ({:04x}:{:04x}): {e}", proto::VID, d.product_id()),
        }
    }
}
