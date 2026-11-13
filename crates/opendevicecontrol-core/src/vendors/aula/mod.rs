//! Aula keyboards, over their CX 2.4 GHz receiver (`3554:fa09`) or a USB
//! cable (Sinowealth `258a:010c`), like the official driver. Bluetooth is not
//! supported.
//!
//! | Module | Contents |
//! |---|---|
//! | `proto` | packet formats (receiver and cable), settings block |
//! | `layout` | per-key LED maps |
//! | `preview` | on-screen approximation of the built-in effects |
//! | `keyboard` | the [`Device`](crate::devices::Device) implementation |
//! | `receiver`, `wired` | the two links |

pub mod keyboard;
pub mod keys;
pub mod layout;
pub mod preview;
pub mod proto;
pub mod receiver;
pub mod wired;

use crate::devices::{Device, Ids};
use std::ffi::CString;

/// The receiver's USB vendor ID.
pub const VID: u16 = 0x3554;
/// The 2.4 GHz receiver shipped with the F99 (and other CX-based boards).
pub const PID_RECEIVER: u16 = 0xfa09;
/// Wired mode: the keyboard's own Sinowealth controller.
pub const VID_WIRED: u16 = 0x258a;
pub const PID_WIRED: u16 = 0x010c;

/// The receiver's USB interface carrying the command collection.
pub fn is_command_interface(d: &hidapi::DeviceInfo) -> bool {
    d.vendor_id() == VID && d.product_id() == PID_RECEIVER && d.interface_number() == 1 && vendor_or_whole(d, 0xff02)
}

/// The cable connection's USB interface carrying the command collection.
pub fn is_wired_interface(d: &hidapi::DeviceInfo) -> bool {
    d.vendor_id() == VID_WIRED && d.product_id() == PID_WIRED && d.interface_number() == 1 && vendor_or_whole(d, 0xff00)
}

/// Windows lists each top-level collection separately (keep the vendor
/// ones); Linux hidraw has one node per interface, listed under whichever
/// collection comes first.
fn vendor_or_whole(d: &hidapi::DeviceInfo, page: u16) -> bool {
    !cfg!(windows) || d.usage_page() == page
}

/// Whether the opened collection/interface declares report `id` (HID item
/// `85 id`). Several vendor collections share a page; only one carries the
/// command reports. Reading the descriptor sends nothing to the device.
fn declares_report(dev: &hidapi::HidDevice, id: u8) -> bool {
    let mut buf = [0u8; 4096];
    match dev.get_report_descriptor(&mut buf) {
        Ok(n) => buf[..n].windows(2).any(|w| w == [0x85, id]),
        Err(_) => true, // can't tell: let the model query decide
    }
}

/// The path the service holds the keyboard under. It is one device whichever
/// link it uses (it moves between the receiver and the cable by itself), so
/// it isn't tied to either one's HID path.
pub const PATH: &str = "aula:keyboard";

/// A link opened to a keyboard.
pub(crate) struct Opened {
    pub model: Option<u8>,
    pub link: Box<dyn keyboard::Link>,
    pub info: hidapi::DeviceInfo,
    pub wired: bool,
}

/// Open the command interface `d` (from `all`, the HID device list) as a
/// link. `None` if it can't be opened or isn't an Aula keyboard.
fn open_link(api: &hidapi::HidApi, all: &[&hidapi::DeviceInfo], d: &hidapi::DeviceInfo) -> Option<Opened> {
    let wired = is_wired_interface(d);
    let dev = match api.open_path(d.path()) {
        Ok(dev) => dev,
        Err(e) => {
            log::warn!("failed to open Aula keyboard interface: {e}");
            return None;
        }
    };
    let report = if wired { proto::wired::REPORT_ID } else { proto::REPORT_ID };
    if !declares_report(&dev, report) {
        return None;
    }
    let (model, link): (Option<u8>, Box<dyn keyboard::Link>) = if wired {
        let m = wired::Wired::query_model(&dev);
        // The cable VID/PID is a generic Sinowealth one: only take
        // keyboards that identify as a known Aula model.
        if !m.is_some_and(|m| keyboard::MODELS.iter().any(|x| x.id == m)) {
            log::debug!("258a:010c interface did not identify as an Aula keyboard ({m:?})");
            return None;
        }
        // Input report 3 (host keys, change notices) has its own
        // collection on Windows; on Linux it comes on the same node.
        let events = all
            .iter()
            .filter(|e| e.vendor_id() == VID_WIRED && e.product_id() == PID_WIRED && e.interface_number() == d.interface_number())
            .filter(|e| !cfg!(windows) || e.usage_page() == 0xff00)
            .filter_map(|e| api.open_path(e.path()).ok())
            .find(|h| declares_report(h, 0x03) && !declares_report(h, proto::wired::REPORT_ID) || !cfg!(windows));
        // Linux: the keyboard interface (0), to notice typing. Windows
        // doesn't let programs read keyboards.
        let keys = all
            .iter()
            .filter(|e| cfg!(target_os = "linux") && e.vendor_id() == VID_WIRED && e.product_id() == PID_WIRED && e.interface_number() == 0)
            .find_map(|e| api.open_path(e.path()).ok());
        (m, Box::new(wired::Wired::new(dev, events, keys)))
    } else {
        let m = receiver::Receiver::query_model(&dev);
        if m.is_none() {
            log::info!("Aula receiver: keyboard asleep or away");
        }
        (m, Box::new(receiver::Receiver::new(dev, keyboard::model(m).slug.into(), m.is_some())))
    };
    Some(Opened { model, link, info: d.clone(), wired })
}

/// The command interfaces of one link that are plugged in. Refreshes `api`
/// for that VID/PID only (cheaper than a full HID scan).
fn interfaces(api: &mut hidapi::HidApi, wired: bool) -> Vec<hidapi::DeviceInfo> {
    let (vid, pid) = if wired { (VID_WIRED, PID_WIRED) } else { (VID, PID_RECEIVER) };
    if api.reset_devices().and_then(|_| api.add_devices(vid, pid)).is_err() {
        return Vec::new();
    }
    api.device_list().filter(|d| if wired { is_wired_interface(d) } else { is_command_interface(d) }).cloned().collect()
}

/// Paths of the plugged-in command interfaces of one link.
pub(crate) fn present(api: &mut hidapi::HidApi, wired: bool) -> Vec<std::ffi::CString> {
    interfaces(api, wired).iter().map(|d| d.path().to_owned()).collect()
}

/// Open the first working link of one kind, skipping `skip` paths.
pub(crate) fn find(api: &mut hidapi::HidApi, wired: bool, skip: &[std::ffi::CString]) -> Option<Opened> {
    let list = interfaces(api, wired);
    let all: Vec<hidapi::DeviceInfo> = api.device_list().cloned().collect();
    let refs: Vec<&hidapi::DeviceInfo> = all.iter().collect();
    list.iter().filter(|d| !skip.iter().any(|p| p.as_c_str() == d.path())).find_map(|d| open_link(api, &refs, d))
}

/// Open the Aula keyboard in `all` (the full HID device list), over the
/// cable if it is plugged in, else over the receiver. It is one device
/// either way; later it switches links by itself, so nothing is opened
/// while the service already holds it.
pub(crate) fn discover(
    api: &hidapi::HidApi,
    all: &[&hidapi::DeviceInfo],
    ids: &mut Ids,
    skip_paths: &[CString],
    out: &mut Vec<(CString, Box<dyn Device>)>,
) {
    if skip_paths.iter().any(|p| p.as_bytes() == PATH.as_bytes()) {
        return;
    }
    let wired = all.iter().filter(|d| is_wired_interface(d));
    let receivers = all.iter().filter(|d| is_command_interface(d));
    let Some(o) = wired.chain(receivers).find_map(|d| open_link(api, all, d)) else { return };
    let id = ids.next(keyboard::model(o.model).slug);
    let (model, wired) = (o.model, o.wired);
    let kb = keyboard::AulaKeyboard::open(o, id.clone());
    log::info!(
        "opened Aula {} ({id}) over {}, model id {:?}",
        kb.info().model,
        if wired { "USB cable" } else { "2.4 GHz receiver" },
        model.map(|m| format!("{m:#04x}"))
    );
    out.push((CString::new(PATH).expect("no NUL"), Box::new(kb)));
}
