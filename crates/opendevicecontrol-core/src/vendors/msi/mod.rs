//! MSI motherboards' Mystic Light RGB, over the board's USB LED controller.

pub mod mystic_light;
pub mod proto;

use crate::devices::{Device, Ids};
use std::ffi::CString;

/// HID usage page of the 761-byte controller's interface on `0db0:0076`
/// boards (not supported).
const USAGE_PAGE_761: u16 = 0xff00;

pub(crate) fn discover(
    api: &hidapi::HidApi,
    all: &[&hidapi::DeviceInfo],
    ids: &mut Ids,
    skip_paths: &[CString],
    out: &mut Vec<(CString, Box<dyn Device>)>,
) {
    let candidate = |d: &hidapi::DeviceInfo| {
        (d.vendor_id() == proto::VID && proto::PIDS.contains(&d.product_id()))
            || (d.vendor_id() == proto::COMMON_VID && d.product_id() == proto::COMMON_PID)
    };
    // One controller per board; it may list more than one interface.
    let mut seen: Vec<(u16, u16, Option<String>)> = Vec::new();
    for d in all.iter().filter(|d| candidate(d) && d.usage_page() != USAGE_PAGE_761) {
        let key = (d.vendor_id(), d.product_id(), d.serial_number().map(str::to_owned));
        if seen.contains(&key) {
            continue;
        }
        if skip_paths.iter().any(|p| p.as_c_str() == d.path()) {
            seen.push(key);
            continue;
        }
        let dev = match api.open_path(d.path()) {
            Ok(dev) => dev,
            Err(e) => {
                log::warn!("failed to open MSI Mystic Light: {e}");
                continue;
            }
        };
        // Identify the protocol by reading the effects report (a read;
        // nothing is written to a board that isn't recognised).
        let mut buf = [0u8; 200];
        buf[0] = proto::REPORT_EFFECTS;
        let n = dev.get_feature_report(&mut buf).unwrap_or(0);
        let Some(variant) = proto::Variant::from_len(n) else {
            log::info!(
                "MSI Mystic Light {:04x}:{:04x}: effects report is {n} bytes; only the 162- and 185-byte protocols are supported",
                d.vendor_id(),
                d.product_id()
            );
            continue;
        };
        seen.push(key);
        let pid = match d.vendor_id() {
            proto::COMMON_VID => d.serial_number().and_then(proto::virtual_pid).unwrap_or(proto::COMMON_PID),
            _ => d.product_id(),
        };
        let id = ids.next("msi-mystic-light");
        match mystic_light::MysticLight::open(dev, id.clone(), d, variant, &buf[..variant.report_len()], pid) {
            Ok(m) => {
                log::info!("opened {} ({id}), firmware {}", m.info().model, m.info().firmware.as_deref().unwrap_or("unknown"));
                out.push((d.path().to_owned(), Box::new(m)));
            }
            Err(e) => log::warn!("failed to open MSI Mystic Light: {e}"),
        }
    }
}
