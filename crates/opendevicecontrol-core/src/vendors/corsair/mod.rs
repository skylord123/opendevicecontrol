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
        let id = ids.next(model.slug);
        if skip_paths.iter().any(|p| p.as_c_str() == d.path()) {
            continue;
        }
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
        let id = ids.next(model.slug);
        if skip_paths.iter().any(|p| p.as_c_str() == d.path()) {
            continue;
        }
        let input_paths: Vec<&hidapi::DeviceInfo> = list.iter().filter(|i| nxp::is_input_interface(model, d, i)).copied().collect();
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
