//! ASUS graphics cards' Aura RGB lighting: an ENE controller on the card's
//! I²C bus, reached through NvAPI on Windows and the NVIDIA driver's i2c
//! adapters (i2c-dev) on Linux.

pub mod ene;
pub mod aura_usb;
pub mod gpu;

use crate::devices::{Device, Ids};
use crate::smbus::SmBus;
use std::ffi::CString;

/// I²C address of the lighting controller on ASUS NVIDIA cards.
pub const GPU_ADDR: u8 = 0x67;

/// Cards known to carry the controller at [`GPU_ADDR`], by PCI device and
/// subsystem (vendor 0x1043): (NVIDIA device, ASUS subsystem device, name).
/// Only these are probed, so nothing is ever sent to other cards' buses.
pub const CARDS: &[(u16, u16, &str)] = &[(0x2705, 0x8957, "GeForce RTX 4070 Ti SUPER")];

const ASUS: u16 = 0x1043;
const NVIDIA: u16 = 0x10de;

fn card(vendor: u16, device: u16, sub_vendor: u16, sub_device: u16) -> Option<&'static str> {
    if vendor != NVIDIA || sub_vendor != ASUS {
        return None;
    }
    CARDS.iter().find(|c| c.0 == device && c.1 == sub_device).map(|c| c.2)
}

/// Identify the controller on `bus` and open it.
fn open(mut bus: Box<dyn SmBus>, name: &str, path: CString, ids: &mut Ids, skip_paths: &[CString], out: &mut Vec<(CString, Box<dyn Device>)>) {
    if skip_paths.contains(&path) {
        return;
    }
    if !ene::identify(bus.as_mut(), GPU_ADDR) {
        log::debug!("ASUS {name}: no lighting controller answers on {}", bus.name());
        return;
    }
    let id = ids.next("asus-gpu");
    match gpu::AuraGpu::open(bus, GPU_ADDR, id.clone(), name.to_string()) {
        Ok(g) => {
            log::info!("opened ASUS {name} lighting ({id}), controller {}", g.info().product_string.as_deref().unwrap_or("?"));
            out.push((path, Box::new(g)));
        }
        Err(e) => log::warn!("failed to open ASUS {name} lighting: {e}"),
    }
}

#[cfg(windows)]
pub(crate) fn discover(ids: &mut Ids, skip_paths: &[CString], out: &mut Vec<(CString, Box<dyn Device>)>) {
    use crate::smbus::nvapi;
    let gpus = match nvapi::gpus() {
        Ok(g) => g,
        Err(e) => {
            log::debug!("NvAPI: {e}");
            return;
        }
    };
    for (i, g) in gpus.into_iter().enumerate() {
        let Some(name) = card(g.vendor, g.device, g.sub_vendor, g.sub_device) else { continue };
        let path = CString::new(format!("nvapi-i2c:{}", g.bus.unwrap_or(i as u32))).expect("no NUL");
        open(Box::new(nvapi::NvBus::open(g)), name, path, ids, skip_paths, out);
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn discover(ids: &mut Ids, skip_paths: &[CString], out: &mut Vec<(CString, Box<dyn Device>)>) {
    use crate::smbus::linux;
    // Port 1 of the card, as on Windows.
    for a in linux::nvidia_adapters("NVIDIA i2c adapter 1 ") {
        let (v, d, sv, sd) = a.pci;
        let Some(name) = card(v, d, sv, sd) else { continue };
        let path = CString::new(format!("/dev/i2c-{}", a.number)).expect("no NUL");
        if skip_paths.contains(&path) {
            continue;
        }
        match linux::LinuxBus::open(a.number, &a.name) {
            Ok(bus) => open(Box::new(bus), name, path, ids, skip_paths, out),
            Err(e) => log::debug!("ASUS {name} lighting: /dev/i2c-{}: {e} (load i2c-dev and install the udev rule)", a.number),
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn discover(_ids: &mut Ids, _skip_paths: &[CString], _out: &mut Vec<(CString, Box<dyn Device>)>) {}

#[cfg(test)]
mod tests {
    #[test]
    fn only_listed_asus_cards_are_probed() {
        assert_eq!(super::card(0x10de, 0x2705, 0x1043, 0x8957), Some("GeForce RTX 4070 Ti SUPER"));
        assert_eq!(super::card(0x10de, 0x2705, 0x1462, 0x8957), None);
        assert_eq!(super::card(0x10de, 0x2704, 0x1043, 0x8957), None);
    }
}
