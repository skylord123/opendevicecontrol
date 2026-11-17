//! Corsair devices (USB vendor ID 0x1b1c).
//!
//! | Module | Devices |
//! |---|---|
//! | `hxi` | HXi / RMi power supplies |
//! | `nxp` | legacy NXP-protocol keyboards, mice, mousepads, ST100 |
//! | `bragi` | Bragi-protocol keyboards, mice, Slipstream receivers, headsets |
//! | `void` | legacy (pre-Bragi) headsets |
//! | `cooling` | iCUE LINK hub, Commander Core/XT/Pro/DUO, Hydro AIOs, Lighting Nodes |
//! | `ram` | RGB memory over SMBus (opt-in; not discovered here) |

pub mod bragi;
pub mod cooling;
pub mod hxi;
pub mod nxp;
pub mod ram;
pub mod void;

// Shared with the driver modules (`super::Ids`, `super::Device`).
pub(crate) use crate::devices::{Device, Ids};

use std::ffi::CString;

/// Corsair's USB vendor ID.
pub const VID: u16 = 0x1b1c;

type Opened = Vec<(CString, Box<dyn Device>)>;

/// The serial number of `d`, if it reports a non-blank one.
pub(crate) fn serial_of(d: &hidapi::DeviceInfo) -> Option<&str> {
    d.serial_number().filter(|s| !s.trim().is_empty())
}

/// Whether two interfaces (with these serials) belong to the same physical
/// unit, when `units` units of that model are plugged in. Without serials
/// there is no portable way to tell units apart, so interfaces are only
/// paired when a single unit is present: two serial-less units are never
/// cross-wired.
pub(crate) fn same_unit(a: Option<&str>, b: Option<&str>, units: usize) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a == b,
        (None, None) => units == 1,
        _ => false,
    }
}

/// Identifies one USB interface: on Windows each top-level collection of an
/// interface gets its own path (`...&Col02#8&1a2b3c4d&0&0001#{guid}`), so
/// the collection parts are dropped; elsewhere the path already names one
/// interface.
pub(crate) fn interface_key(path: &std::ffi::CStr) -> String {
    let s = path.to_string_lossy();
    let parts: Vec<&str> = s.split('#').collect();
    if parts.len() >= 3 && parts[0].to_ascii_uppercase().ends_with("HID") {
        let hw: Vec<&str> = parts[1].split('&').filter(|p| !p.to_ascii_uppercase().starts_with("COL")).collect();
        let inst = parts[2].rsplit_once('&').map_or(parts[2], |x| x.0);
        return format!("{}#{inst}", hw.join("&")).to_ascii_uppercase();
    }
    s.into_owned()
}

/// Number of distinct physical interfaces among `list` entries matching `f`.
pub(crate) fn count_units(list: &[&hidapi::DeviceInfo], f: impl Fn(&hidapi::DeviceInfo) -> bool) -> usize {
    let mut keys: Vec<String> = list.iter().filter(|d| f(d)).map(|d| interface_key(d.path())).collect();
    keys.sort();
    keys.dedup();
    keys.len()
}

/// Open every supported Corsair USB device in `list` (the full HID device
/// list; non-Corsair entries are ignored). Devices whose primary path is in
/// `skip_paths` are already held by the caller.
pub(crate) fn discover(
    api: &hidapi::HidApi,
    all: &[&hidapi::DeviceInfo],
    ids: &mut Ids,
    skip_paths: &[CString],
    out: &mut Opened,
) {
    let list: Vec<&hidapi::DeviceInfo> = all.iter().copied().filter(|d| d.vendor_id() == VID).collect();
    let serial = |d: &hidapi::DeviceInfo| d.serial_number().map(str::to_owned).filter(|s| !s.trim().is_empty());

    // HXi / RMi power supplies: one HID interface each.
    for d in list.iter().filter(|d| hxi::model_for_pid(d.product_id()).is_some()) {
        let model = hxi::model_for_pid(d.product_id()).unwrap();
        if skip_paths.iter().any(|p| p.as_c_str() == d.path()) {
            continue;
        }
        let id = ids.next(model.slug);
        match api.open_path(d.path()).map_err(crate::Error::from).and_then(|dev| hxi::HxiPsu::open(dev, model, id.clone(), serial(d))) {
            Ok(psu) => {
                log::info!("opened {} ({id})", model.name);
                out.push((d.path().to_owned(), Box::new(psu)));
            }
            Err(e) => log::warn!("failed to open {}: {e}", model.name),
        }
    }

    // NXP peripherals: a command interface (vendor page 0xFFC2) plus input
    // collections on interface 0 (vendor pages, carrying the key bitmap).
    for d in list.iter().filter(|d| nxp::model_for_pid(d.product_id()).is_some_and(|m| nxp::is_command_interface(m, d))) {
        let model = nxp::model_for_pid(d.product_id()).unwrap();
        if skip_paths.iter().any(|p| p.as_c_str() == d.path()) {
            continue;
        }
        let id = ids.next(model.slug);
        let units = count_units(&list, |c| c.product_id() == d.product_id() && nxp::is_command_interface(model, c));
        let input_paths: Vec<&hidapi::DeviceInfo> = list.iter().filter(|i| nxp::is_input_interface(model, d, i, units)).copied().collect();
        let inputs: Vec<hidapi::HidDevice> = input_paths
            .iter()
            .filter_map(|i| match api.open_path(i.path()) {
                Ok(h) => Some(h),
                Err(e) => {
                    log::warn!("{}: cannot open input interface: {e}", model.name);
                    None
                }
            })
            .collect();
        match api
            .open_path(d.path())
            .map_err(crate::Error::from)
            .and_then(|cmd| nxp::NxpDevice::open(cmd, inputs, model, id.clone(), serial(d)))
        {
            Ok(dev) => {
                log::info!(
                    "opened {} ({id}), firmware {}, {} input interface(s)",
                    model.name,
                    dev.info().firmware.as_deref().unwrap_or("?"),
                    input_paths.len()
                );
                out.push((d.path().to_owned(), Box::new(dev)));
            }
            Err(e) => log::warn!("failed to open {}: {e}", model.name),
        }
    }

    // Legacy (pre-Bragi) headsets: VOID / VOID PRO / VOID ELITE, HS70.
    void::discover(api, &list, ids, skip_paths, out);
    // Bragi peripherals (experimental).
    bragi::discover(api, &list, ids, skip_paths, out);
    // Cooling devices (hubs, AIOs, lighting nodes).
    cooling::discover(api, &list, skip_paths, ids, out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn serial_less_units_are_only_paired_when_alone() {
        assert!(same_unit(Some("A1"), Some("A1"), 2));
        assert!(!same_unit(Some("A1"), Some("B2"), 2));
        assert!(same_unit(None, None, 1));
        // Two identical units without serials: never attach one's
        // interfaces to the other.
        assert!(!same_unit(None, None, 2));
        assert!(!same_unit(Some("A1"), None, 1));
    }

    #[test]
    fn windows_collections_share_an_interface_key() {
        let k = |p: &str| interface_key(&CString::new(p).unwrap());
        let c1 = k(r"\\?\HID#VID_1B1C&PID_1B2D&MI_01&Col01#8&1a2b3c4d&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}");
        let c2 = k(r"\\?\HID#VID_1B1C&PID_1B2D&MI_01&Col02#8&1a2b3c4d&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}");
        let other_unit = k(r"\\?\HID#VID_1B1C&PID_1B2D&MI_01&Col01#8&11aa22bb&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}");
        assert_eq!(c1, c2);
        assert_ne!(c1, other_unit);
        // Linux hidraw: one node per interface.
        assert_ne!(k("/dev/hidraw3"), k("/dev/hidraw4"));
    }
}
